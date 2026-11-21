//! The layout a new project starts with: for each project kind
//! (`catalog::KINDS`) the roles it begins with, and for each role the folder
//! the project's first language conventionally uses. A fixed table, never a
//! model question: the same kind and languages always give the same tree.
//! `new_project` (the MCP server) writes it into a new manifest's `layout`;
//! the project changes its layout freely afterwards.
//!
//! Roles by kind (every kind also gets `docs` and `out`):
//!
//! | kind            | roles                                               |
//! |-----------------|-----------------------------------------------------|
//! | library         | source, tests, benches, examples (+ include: C/C++) |
//! | cli-tool        | source, tests                                       |
//! | service         | source, tests, integrations                         |
//! | interactive-app | source, tests, assets                               |
//! | website         | source, tests, assets                               |
//! | firmware-driver | source, tests, tools (+ include: C/C++)             |
//! | research        | source, data, tools                                 |
//! | data-pipeline   | source, tests, data                                 |
//! | documents       | (docs and out only)                                 |
//! | configuration   | integrations, tools                                 |
//! | mixed           | source, tests, tools, data                          |
//!
//! Folders by first language (blank: the role's default folder,
//! `Role::default_dir`):
//!
//! | role     | Rust    | C, C++  | Python     | Go                  | JS, TS  | Java, Kotlin        | Zig     |
//! |----------|---------|---------|------------|---------------------|---------|---------------------|---------|
//! | source   | src     | src     | src        | cmd, internal (1)   | src     | src/main            | src     |
//! | tests    | tests   | tests   | tests      | (2)                 | test    | src/test            | tests   |
//! | benches  | benches | bench   | benchmarks | (2)                 | bench   | src/jmh             | bench   |
//! | tools    |         |         | scripts    |                     | scripts |                     |         |
//! | assets   |         |         |            |                     | public  | src/main/resources  |         |
//! | out      |         | build   |            |                     | dist    | build               | zig-out |
//!
//! (1) A Go library: `pkg`, `internal`. (2) Go keeps `_test.go` files, tests
//! and benchmarks alike, beside the code they test, so a Go layout has no
//! `tests` or `benches` folder (with one, the test-name rule of `roles.rs`
//! would send every `_test.go` file there). `include` is `include`, and
//! `examples`, `docs`, `integrations`, `data` keep their default folders in
//! every language. Python's source folder is `src` (the src layout the
//! Python Packaging Authority's guide recommends); the package folder goes
//! inside it.

use std::collections::{BTreeMap, BTreeSet};

use crate::catalog::KINDS;
use crate::roles::Role;

/// A manifest's `layout`: role -> folders relative to the root.
pub type Layout = BTreeMap<Role, Vec<String>>;

/// The language families whose folder conventions differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Rust,
    C,
    Python,
    Go,
    Web,
    Jvm,
    Zig,
    Other,
}

impl Family {
    /// The family of a project's first language (case ignored).
    fn of(languages: &[String]) -> Family {
        let first = languages
            .first()
            .map(|l| l.trim().to_ascii_lowercase())
            .unwrap_or_default();
        match first.as_str() {
            "rust" | "rs" => Family::Rust,
            "c" | "c++" | "cpp" | "cxx" | "cc" => Family::C,
            "python" | "py" => Family::Python,
            "go" | "golang" => Family::Go,
            "javascript" | "js" | "typescript" | "ts" => Family::Web,
            "java" | "kotlin" | "kt" => Family::Jvm,
            "zig" => Family::Zig,
            _ => Family::Other,
        }
    }
}

/// The roles a new project of `kind` starts with, before the language
/// adjusts them (`include` added for C and C++, tests and benches dropped
/// for Go); None for a kind that is not in `catalog::KINDS`.
pub fn roles(kind: &str) -> Option<&'static [Role]> {
    use Role::*;
    let roles: &'static [Role] = match kind {
        "library" => &[Source, Tests, Benches, Examples, Docs, Out],
        "cli-tool" => &[Source, Tests, Docs, Out],
        "service" => &[Source, Tests, Integrations, Docs, Out],
        "interactive-app" => &[Source, Tests, Assets, Docs, Out],
        "website" => &[Source, Tests, Assets, Docs, Out],
        "firmware-driver" => &[Source, Tests, Tools, Docs, Out],
        "research" => &[Source, Data, Tools, Docs, Out],
        "data-pipeline" => &[Source, Tests, Data, Docs, Out],
        "documents" => &[Docs, Out],
        "configuration" => &[Integrations, Tools, Docs, Out],
        "mixed" => &[Source, Tests, Tools, Data, Docs, Out],
        _ => return None,
    };
    Some(roles)
}

/// The folders of `role` in a project of `family` (and `kind`, which only
/// matters for a Go library).
fn folders(role: Role, family: Family, kind: &str) -> Vec<&'static str> {
    use Family::*;
    let one = |d: &'static str| vec![d];
    match (role, family) {
        (Role::Source, Go) if kind == "library" => vec!["pkg", "internal"],
        (Role::Source, Go) => vec!["cmd", "internal"],
        (Role::Source, Jvm) => one("src/main"),
        (Role::Tests, Web) => one("test"),
        (Role::Tests, Jvm) => one("src/test"),
        (Role::Benches, C | Web | Zig) => one("bench"),
        (Role::Benches, Python) => one("benchmarks"),
        (Role::Benches, Jvm) => one("src/jmh"),
        (Role::Tools, Python | Web) => one("scripts"),
        (Role::Assets, Web) => one("public"),
        (Role::Assets, Jvm) => one("src/main/resources"),
        (Role::Out, C | Jvm) => one("build"),
        (Role::Out, Web) => one("dist"),
        (Role::Out, Zig) => one("zig-out"),
        (r, _) => one(r.default_dir()),
    }
}

/// The starting layout of a project of `kind` whose first language is
/// `languages[0]`: a fixed table (module docs), no model. Err names the
/// kinds when `kind` is not one of them.
pub fn layout(kind: &str, languages: &[String]) -> Result<Layout, String> {
    let base = roles(kind).ok_or_else(|| {
        let kinds: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
        format!("unknown kind `{kind}`; the kinds are {}", kinds.join(", "))
    })?;
    let family = Family::of(languages);
    let mut roles: BTreeSet<Role> = base.iter().copied().collect();
    if family == Family::C && matches!(kind, "library" | "firmware-driver") {
        roles.insert(Role::Include);
    }
    if family == Family::Go {
        roles.remove(&Role::Tests);
        roles.remove(&Role::Benches);
    }
    Ok(roles
        .into_iter()
        .map(|r| {
            let dirs = folders(r, family, kind)
                .into_iter()
                .map(str::to_string)
                .collect();
            (r, dirs)
        })
        .collect())
}

/// Every folder of `layout`, sorted, each once.
pub fn tree(layout: &Layout) -> Vec<String> {
    let set: BTreeSet<&String> = layout.values().flatten().collect();
    set.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::manifest::{Manifest, Place};
    use crate::naming::{self, Naming};

    /// One language of every family, and none.
    const LANGUAGES: &[&[&str]] = &[
        &["rust"],
        &["c"],
        &["C++"],
        &["python"],
        &["go"],
        &["typescript"],
        &["javascript"],
        &["java"],
        &["kotlin"],
        &["zig"],
        &["haskell"],
        &[],
        &["rust", "c"],
    ];

    #[test]
    fn every_kind_and_language_gives_a_valid_layout() {
        for (kind, _) in KINDS {
            for langs in LANGUAGES {
                let langs: Vec<String> = langs.iter().map(|l| l.to_string()).collect();
                let at = format!("{kind} {langs:?}");
                let l = layout(kind, &langs).unwrap();
                // Roles of roles.rs only (the type guarantees it), never
                // config (the root itself), always docs and out.
                assert!(!l.contains_key(&Role::Config), "{at}");
                assert!(
                    l.contains_key(&Role::Docs) && l.contains_key(&Role::Out),
                    "{at}"
                );
                // Each folder relative, plain, and claimed by one role only.
                let mut seen = BTreeSet::new();
                for dirs in l.values() {
                    assert!(!dirs.is_empty(), "{at}");
                    for d in dirs {
                        assert!(!d.starts_with('/') && !d.ends_with('/'), "{at}: {d}");
                        assert!(d.split('/').all(|c| !c.is_empty() && c != "." && c != ".."));
                        assert!(seen.insert(d.clone()), "{at}: {d} twice");
                    }
                }
                // A manifest with this layout places a file in each folder
                // in that folder's role (out is unmanaged by design).
                let mut m = Manifest::new("demo", "A demo.", langs.clone());
                m.layout = l.clone();
                for (role, dirs) in &l {
                    for d in dirs {
                        let want = if role.is_unmanaged() {
                            Place::Unmanaged
                        } else {
                            Place::Role(*role)
                        };
                        assert_eq!(m.place(Path::new(&format!("{d}/x"))), want, "{at}: {d}");
                    }
                }
                // Folder names follow the language's folder naming style.
                let style = Naming::for_languages(&langs).dirs;
                for d in tree(&l) {
                    for part in d.split('/') {
                        assert!(naming::follows(part, style), "{at}: {part} vs {style:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn language_conventions() {
        let l = |k: &str, lang: &str| layout(k, &[lang.to_string()]).unwrap();
        let rust = l("library", "rust");
        assert_eq!(rust[&Role::Source], ["src"]);
        assert_eq!(rust[&Role::Benches], ["benches"]);
        assert!(!rust.contains_key(&Role::Include));
        assert_eq!(l("library", "c")[&Role::Include], ["include"]);
        assert!(!l("cli-tool", "c").contains_key(&Role::Include));
        let go = l("service", "go");
        assert_eq!(go[&Role::Source], ["cmd", "internal"]);
        assert!(!go.contains_key(&Role::Tests));
        assert_eq!(l("library", "go")[&Role::Source], ["pkg", "internal"]);
        assert_eq!(l("website", "typescript")[&Role::Assets], ["public"]);
        assert_eq!(l("website", "typescript")[&Role::Out], ["dist"]);
        assert_eq!(l("research", "python")[&Role::Tools], ["scripts"]);
        assert_eq!(l("documents", "rust").len(), 2);
        assert_eq!(
            tree(&l("cli-tool", "rust")),
            ["docs", "out", "src", "tests"]
        );
        assert!(layout("game", &[]).unwrap_err().contains("cli-tool"));
        // The same inputs give the same tree.
        assert_eq!(l("mixed", "zig"), l("mixed", "zig"));
    }
}
