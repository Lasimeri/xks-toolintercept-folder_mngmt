//! Paths as the hook reads them: lexically normalised, the folders it never
//! decides on, the files that mark a project's root, and where a new tree
//! attaches to the existing one. Nothing here follows symbolic links or
//! touches the file system beyond `lstat`.

use std::path::{Component, Path, PathBuf};

/// `p` with `.` dropped and `..` applied to the components before it (never
/// above the root). Claude Code makes `Write` paths absolute; it does not
/// promise to remove `..`.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether anything (file, folder, link, even a dangling one) is at `p`.
pub fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// `$HOME`, when it is set to an absolute path.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .map(|h| normalize(&h))
}

/// The folders never decided on when `XTF_HOOK_SKIP` is unset.
pub const DEFAULT_SKIP: &str = "/tmp:~/.claude";

/// The folders the hook never decides on: the `XTF_HOOK_SKIP` list (or
/// `DEFAULT_SKIP`), the session's scratchpad, and every `.git` and `.xks`
/// folder wherever it is.
#[derive(Debug, Clone)]
pub struct Skip {
    prefixes: Vec<PathBuf>,
}

impl Skip {
    /// `list`: colon-separated absolute or `~/` paths; other entries are
    /// ignored.
    pub fn new(list: &str, scratchpad: Option<&str>, home: Option<&Path>) -> Skip {
        let mut prefixes: Vec<PathBuf> = list
            .split(':')
            .filter_map(|e| expand(e.trim(), home))
            .collect();
        if let Some(s) = scratchpad.map(Path::new).filter(|s| s.is_absolute()) {
            prefixes.push(normalize(s));
        }
        Skip { prefixes }
    }

    pub fn from_env(scratchpad: Option<&str>, home: Option<&Path>) -> Skip {
        let list = std::env::var("XTF_HOOK_SKIP").unwrap_or_else(|_| DEFAULT_SKIP.to_string());
        Skip::new(&list, scratchpad, home)
    }

    pub fn skips(&self, p: &Path) -> bool {
        self.prefixes.iter().any(|d| p.starts_with(d))
            || p.components()
                .any(|c| matches!(c, Component::Normal(n) if n == ".git" || n == ".xks"))
    }
}

/// An absolute path, `~` or `~/...`, as a path; None otherwise.
fn expand(entry: &str, home: Option<&Path>) -> Option<PathBuf> {
    if entry == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = entry.strip_prefix("~/") {
        return home.map(|h| normalize(&h.join(rest)));
    }
    let p = Path::new(entry);
    p.is_absolute().then(|| normalize(p))
}

/// Files and folders whose presence makes a folder a project's root.
pub const MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".jj",
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "CMakeLists.txt",
    "meson.build",
    "Makefile",
    "build.zig",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
    "mix.exs",
    "deno.json",
    "flake.nix",
    "dune-project",
];

/// The markers directly in `dir`.
pub fn markers_in(dir: &Path) -> Vec<&'static str> {
    MARKERS
        .iter()
        .copied()
        .filter(|m| exists(&dir.join(m)))
        .collect()
}

/// The nearest existing folder at or above `start` that holds a marker,
/// with its markers. `home` and `/` never count: a home folder kept under
/// version control is not a project.
pub fn project_above(start: &Path, home: Option<&Path>) -> Option<(PathBuf, Vec<&'static str>)> {
    start
        .ancestors()
        .filter(|d| d.parent().is_some() && Some(*d) != home && d.is_dir())
        .find_map(|d| {
            let m = markers_in(d);
            (!m.is_empty()).then(|| (d.to_path_buf(), m))
        })
}

/// The highest folder of `p`'s ancestry (`p` included) that does not exist
/// yet: where a new tree attaches to the existing one. None when `p`
/// exists.
pub fn top_new(p: &Path) -> Option<PathBuf> {
    if exists(p) {
        return None;
    }
    let mut top = p;
    while let Some(parent) = top.parent() {
        if parent.as_os_str().is_empty() || exists(parent) {
            break;
        }
        top = parent;
    }
    Some(top.to_path_buf())
}

/// `p` relative to `root`, `/`-separated; None when it is not under it or
/// is the root itself.
pub fn rel(root: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(root).ok()?;
    let parts: Vec<String> = r
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_lexically() {
        assert_eq!(
            normalize(Path::new("/p/src/../docs/./a.md")),
            PathBuf::from("/p/docs/a.md")
        );
        assert_eq!(normalize(Path::new("/../../x")), PathBuf::from("/x"));
        assert_eq!(normalize(Path::new("/a/b/")), PathBuf::from("/a/b"));
    }

    #[test]
    fn skips_the_listed_folders_the_scratchpad_git_and_xks() {
        let home = Path::new("/u");
        let s = Skip::new(DEFAULT_SKIP, Some("/scratch/s1"), Some(home));
        assert!(s.skips(Path::new("/tmp/x.rs")));
        assert!(s.skips(Path::new("/u/.claude/settings.json")));
        assert!(s.skips(Path::new("/scratch/s1/a/b.md")));
        assert!(s.skips(Path::new("/p/.git/hooks/pre-commit")));
        assert!(s.skips(Path::new("/p/.xks/decisions.jsonl")));
        assert!(!s.skips(Path::new("/tmpfoo/x.rs")));
        assert!(!s.skips(Path::new("/u/.claudette/x")));
        assert!(!s.skips(Path::new("/p/src/git.rs")));
        // A list replaces the default; relative entries are ignored.
        let s = Skip::new("~/scratch: relative :/srv/x", None, Some(home));
        assert!(!s.skips(Path::new("/tmp/x.rs")));
        assert!(s.skips(Path::new("/u/scratch/a")));
        assert!(s.skips(Path::new("/srv/x/y")));
        assert!(!s.skips(Path::new("/p/relative/y")));
    }

    #[test]
    fn finds_markers_and_attach_points_in_this_checkout() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (dir, m) = project_above(&crate_dir.join("src/new/deeper"), None).unwrap();
        assert_eq!(dir, crate_dir);
        assert_eq!(m, ["Cargo.toml"]);
        // The excluded folder is skipped and the walk goes on above it.
        let (dir, _) = project_above(crate_dir, Some(crate_dir)).unwrap();
        assert_ne!(dir, crate_dir);
        let fresh = crate_dir.join("no-such-dir/a/b");
        assert_eq!(top_new(&fresh), Some(crate_dir.join("no-such-dir")));
        assert_eq!(top_new(crate_dir), None);
        assert_eq!(
            rel(crate_dir, &crate_dir.join("src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(rel(crate_dir, crate_dir), None);
        assert_eq!(rel(crate_dir, Path::new("/elsewhere")), None);
    }
}
