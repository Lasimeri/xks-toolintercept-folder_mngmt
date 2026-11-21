//! The provided JSON: a project's manifest, `xks-project.json` at its root.
//! It names the standard, says what the project is, and maps the fixed roles
//! to the project's own directories. Its rendering is the state of every
//! question about the project, and it stays the same from one question to
//! the next, so a running server keeps it cached and each decision costs
//! only its own question.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::naming::Naming;
use crate::roles::Role;

/// The standard this crate implements.
pub const STANDARD: &str = "xks-folder/1";
/// The manifest's file name, at the project root.
pub const MANIFEST_FILE: &str = "xks-project.json";

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
}
