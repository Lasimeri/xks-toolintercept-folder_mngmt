//! The provided JSON: a project's manifest, `xks-project.json` at its root.
//! It names the standard, says what the project is, and maps the fixed roles
//! to the project's own directories. Its rendering is the state of every
//! question about the project, and it stays the same from one question to
//! the next, so a running server keeps it cached and each decision costs
//! only its own question.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::naming::Naming;
use crate::roles::Role;

/// The standard this crate implements.
pub const STANDARD: &str = "xks-folder/1";
/// The manifest's file name, at the project root.
pub const MANIFEST_FILE: &str = "xks-project.json";
/// The lock every writer of a manifest holds, under the project root.
pub const LAYOUT_LOCK: &str = ".xks/layout.lock";

mod sys {
    unsafe extern "C" {
        /// flock(2): `fd` is only read as a number; a bad one is EBADF.
        pub safe fn flock(fd: i32, operation: i32) -> i32;
    }
}
/// flock(2) LOCK_EX.
const LOCK_EX: i32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// `xks-folder/1`.
    pub standard: String,
    pub name: String,
    /// One or two sentences: what the project is for.
    pub purpose: String,
    #[serde(default)]
    pub languages: Vec<String>,
    /// The project kind (`catalog::KINDS`), once decided or given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Role -> directories relative to the root (a nested path such as
    /// `crates` or `src/tui` is allowed). `config` is the root itself and
    /// never listed.
    pub layout: BTreeMap<Role, Vec<String>>,
    pub naming: Naming,
    /// Paths relative to the root that are never decided on, besides the
    /// `out` and `third_party` directories (e.g. `target`, `.git`).
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// Where a path falls in a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A directory of the layout, playing this role.
    Role(Role),
    /// Directly in the project root (the `config` role).
    Root,
    /// Under an ignored path, or under `out` / `third_party`.
    Unmanaged,
    /// Under a top-level directory the layout does not map.
    Unmapped(String),
}

impl Manifest {
    /// A new manifest with the usual layout (source, tests, docs, tools, out)
    /// and the languages' naming styles.
    pub fn new(name: &str, purpose: &str, languages: Vec<String>) -> Manifest {
        let layout = [
            Role::Source,
            Role::Tests,
            Role::Docs,
            Role::Tools,
            Role::Out,
        ]
        .into_iter()
        .map(|r| (r, vec![r.default_dir().to_string()]))
        .collect();
        Manifest {
            standard: STANDARD.to_string(),
            name: name.to_string(),
            purpose: purpose.to_string(),
            naming: Naming::for_languages(&languages),
            languages,
            kind: None,
            layout,
            ignore: vec![".git".into(), "target".into(), "node_modules".into()],
        }
    }

    pub fn load(root: &Path) -> Result<Manifest, String> {
        let path = root.join(MANIFEST_FILE);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m: Manifest =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if m.standard != STANDARD {
            return Err(format!(
                "{}: standard `{}`, this tool implements `{STANDARD}`",
                path.display(),
                m.standard
            ));
        }
        Ok(m)
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let path = root.join(MANIFEST_FILE);
        let text = serde_json::to_string_pretty(self).expect("manifest to JSON") + "\n";
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Load the manifest of `root`, let `change` edit it, and write it back,
    /// holding an exclusive flock(2) on `LAYOUT_LOCK` throughout, so two
    /// sessions (a hook and an MCP server, or two hooks) changing one
    /// manifest never lose each other's change. The new text goes to a
    /// temporary file that replaces the manifest in one rename: a reader
    /// sees the old manifest or the new one, never a part. The text is what
    /// `save` writes; when it equals the file's, nothing is written.
    /// `change` returns its result, or Err to leave the manifest as it was.
    /// (First written as xtf-mcp's `update_manifest` for `extend_layout`.)
    pub fn update_locked<T>(
        root: &Path,
        change: impl FnOnce(&mut Manifest) -> Result<T, String>,
    ) -> Result<T, String> {
        let lock_path = root.join(LAYOUT_LOCK);
        if let Some(d) = lock_path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&lock_path)
            .map_err(|e| format!("{}: {e}", lock_path.display()))?;
        loop {
            if sys::flock(lock.as_raw_fd(), LOCK_EX) == 0 {
                break;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("{}: {e}", lock_path.display()));
            }
        }
        let mut m = Manifest::load(root)?;
        let out = change(&mut m)?;
        let text = serde_json::to_string_pretty(&m).expect("manifest to JSON") + "\n";
        let path = root.join(MANIFEST_FILE);
        if std::fs::read(&path).is_ok_and(|now| now == text.as_bytes()) {
            return Ok(out);
        }
        let tmp = root.join(format!(".{MANIFEST_FILE}.{}.tmp", std::process::id()));
        // Only the lock holder writes this name (it carries the process id,
        // and the lock is held), so truncating a leftover of a crashed run
        // is safe.
        let written = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = written {
            // The temporary file is this call's own; the manifest is untouched.
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {e}", path.display()));
        }
        drop(lock);
        Ok(out)
    }

    /// Map `dir` (relative to the root) as `role`, unless the layout maps it
    /// already, to this role or another (a folder plays one role; a mapping
    /// is never moved). True when it was added.
    pub fn map_folder(&mut self, role: Role, dir: &str) -> bool {
        let dir = dir.trim_matches('/').to_string();
        if dir.is_empty()
            || self
                .layout
                .values()
                .flatten()
                .any(|d| d.trim_matches('/') == dir)
        {
            return false;
        }
        self.layout.entry(role).or_default().push(dir);
        true
    }

    /// The nearest directory at or above `path` holding a manifest.
    pub fn find_root(path: &Path) -> Option<PathBuf> {
        let start = if path.is_dir() { path } else { path.parent()? };
        start
            .ancestors()
            .find(|d| d.join(MANIFEST_FILE).is_file())
            .map(Path::to_path_buf)
    }

    /// The roles a placement question offers: the layout's, plus `out` and
    /// `config`, which every project has.
    pub fn roles(&self) -> Vec<Role> {
        Role::ALL
            .into_iter()
            .filter(|r| matches!(r, Role::Out | Role::Config) || self.layout.contains_key(r))
            .collect()
    }

    /// The directory new files of `role` go to (`.` for config).
    pub fn dir_for(&self, role: Role) -> String {
        if role == Role::Config {
            return ".".into();
        }
        self.layout
            .get(&role)
            .and_then(|d| d.first())
            .cloned()
            .unwrap_or_else(|| role.default_dir().to_string())
    }

    /// Whether `rel` (a path relative to the root) is a folder the manifest
    /// names itself: a layout directory or an `ignore` entry. Creating one is
    /// what the manifest already says, so there is nothing to decide.
    pub fn declares(&self, rel: &str) -> bool {
        let plain = |d: &str| {
            d.split('/')
                .filter(|s| !s.is_empty() && *s != ".")
                .collect::<Vec<_>>()
                .join("/")
        };
        let rel = plain(rel);
        !rel.is_empty()
            && self
                .layout
                .values()
                .flatten()
                .chain(&self.ignore)
                .any(|d| plain(d) == rel)
    }

    /// Where `rel` (a path relative to the root) falls: the layout directory
    /// with the longest matching prefix decides.
    pub fn place(&self, rel: &Path) -> Place {
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if parts.len() <= 1 {
            return Place::Root;
        }
        let under = |dir: &str| {
            let d: Vec<&str> = dir
                .split('/')
                .filter(|s| !s.is_empty() && *s != ".")
                .collect();
            !d.is_empty() && d.len() < parts.len() && d.iter().zip(&parts).all(|(a, b)| a == b)
        };
        if self.ignore.iter().any(|i| under(i)) {
            return Place::Unmanaged;
        }
        let best = self
            .layout
            .iter()
            .flat_map(|(r, dirs)| dirs.iter().map(move |d| (*r, d)))
            .filter(|(_, d)| under(d))
            .max_by_key(|(_, d)| d.split('/').count());
        match best {
            Some((r, _)) if r.is_unmanaged() => Place::Unmanaged,
            Some((r, _)) => Place::Role(r),
            None => Place::Unmapped(parts[0].clone()),
        }
    }

    /// The state text of every question about this project: the manifest
    /// minus what is not about the project (the standard name, the ignore
    /// list), pretty JSON, keys in a fixed order.
    pub fn state(&self) -> String {
        let layout: serde_json::Map<String, serde_json::Value> = self
            .layout
            .iter()
            .map(|(r, d)| (r.key().to_string(), serde_json::json!(d)))
            .collect();
        let mut v = serde_json::json!({
            "project": self.name,
            "purpose": self.purpose,
            "languages": self.languages,
        });
        if let Some(k) = &self.kind {
            v["kind"] = serde_json::json!(k);
        }
        v["layout"] = serde_json::Value::Object(layout);
        serde_json::to_string_pretty(&v).expect("state to JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Manifest {
        let mut m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        m.layout.insert(
            Role::Source,
            vec!["src".into(), "crates".into(), "src/tui".into()],
        );
        m.layout
            .insert(Role::ThirdParty, vec!["third_party".into()]);
        m
    }

    #[test]
    fn places_paths() {
        let m = m();
        assert_eq!(m.place(Path::new("Cargo.toml")), Place::Root);
        assert_eq!(m.place(Path::new("src/main.rs")), Place::Role(Role::Source));
        assert_eq!(
            m.place(Path::new("crates/a/src/lib.rs")),
            Place::Role(Role::Source)
        );
        assert_eq!(m.place(Path::new("tests/x.rs")), Place::Role(Role::Tests));
        assert_eq!(m.place(Path::new("out/run.log")), Place::Unmanaged);
        assert_eq!(m.place(Path::new("target/debug/x")), Place::Unmanaged);
        assert_eq!(
            m.place(Path::new("third_party/llama.cpp/x.c")),
            Place::Unmanaged
        );
        assert_eq!(m.place(Path::new("misc/x")), Place::Unmapped("misc".into()));
        assert_eq!(m.dir_for(Role::Tools), "tools");
        assert_eq!(m.dir_for(Role::Config), ".");
    }

    #[test]
    fn state_is_stable_and_roles_include_out_and_config() {
        let m = m();
        assert_eq!(m.state(), m.clone().state());
        let roles = m.roles();
        assert!(roles.contains(&Role::Out) && roles.contains(&Role::Config));
        assert!(!roles.contains(&Role::Assets));
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_folder_is_mapped_once_and_never_moved() {
        let mut m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        m.layout.clear();
        assert!(m.map_folder(Role::Tools, "scripts/"));
        assert_eq!(m.layout[&Role::Tools], ["scripts"]);
        // The same folder again, to the same role or another: refused.
        assert!(!m.map_folder(Role::Tools, "scripts"));
        assert!(!m.map_folder(Role::Source, "scripts"));
        assert!(!m.map_folder(Role::Docs, ""));
        assert!(m.map_folder(Role::Tools, "bin"));
        assert_eq!(m.layout[&Role::Tools], ["scripts", "bin"]);
    }

    #[test]
    fn concurrent_locked_updates_lose_nothing() {
        let root = std::env::temp_dir().join(format!("xtf-std-lock-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        m.layout.clear();
        m.save(&root).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    Manifest::update_locked(&root, |m| {
                        Ok(m.map_folder(Role::Data, &format!("d{i}")))
                    })
                    .unwrap()
                })
            })
            .collect();
        assert!(threads.into_iter().all(|t| t.join().unwrap()));
        let after = Manifest::load(&root).unwrap();
        let mut got = after.layout[&Role::Data].clone();
        got.sort();
        let want: Vec<String> = (0..8).map(|i| format!("d{i}")).collect();
        assert_eq!(got, want);
        // A change that fails leaves the file as it was.
        let before = std::fs::read(root.join(MANIFEST_FILE)).unwrap();
        let r: Result<(), String> = Manifest::update_locked(&root, |m| {
            m.map_folder(Role::Docs, "docs");
            Err("refused".into())
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read(root.join(MANIFEST_FILE)).unwrap(), before);
    }
}
