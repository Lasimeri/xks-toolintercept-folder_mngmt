//! The roles a top-level directory of a project plays. Every project maps
//! these fixed roles to its own directory names (`src`, `crates`, `cli`, ...),
//! and every placement question offers roles, with the fixed descriptions
//! below, as its options: the same question, worded the same way, in every
//! project. The wording is part of the standard (`xks-folder/1`): changing a
//! description changes the answers, so it means a new standard version.

use serde::{Deserialize, Serialize};

/// A role, in the order options are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Source,
    Include,
    Tests,
    Benches,
    Examples,
    Docs,
    Tools,
    Integrations,
    Data,
    Assets,
    ThirdParty,
    Out,
    Config,
}

impl Role {
    pub const ALL: [Role; 13] = [
        Role::Source,
        Role::Include,
        Role::Tests,
        Role::Benches,
        Role::Examples,
        Role::Docs,
        Role::Tools,
        Role::Integrations,
        Role::Data,
        Role::Assets,
        Role::ThirdParty,
        Role::Out,
        Role::Config,
    ];

    /// The option key the model answers with.
    pub fn key(self) -> &'static str {
        match self {
            Role::Source => "source",
            Role::Include => "include",
            Role::Tests => "tests",
            Role::Benches => "benches",
            Role::Examples => "examples",
            Role::Docs => "docs",
            Role::Tools => "tools",
            Role::Integrations => "integrations",
            Role::Data => "data",
            Role::Assets => "assets",
            Role::ThirdParty => "third_party",
            Role::Out => "out",
            Role::Config => "config",
        }
    }

    /// The option's description (fixed wording, standard xks-folder/1).
    pub fn description(self) -> &'static str {
        match self {
            Role::Source => "the program's own source code",
            Role::Include => "public header files for C or C++ code",
            Role::Tests => "tests, test fixtures and test runners",
            Role::Benches => "benchmarks and the inputs they run on",
            Role::Examples => "examples and sample inputs for people to try",
            Role::Docs => "documentation, design notes and written-up results",
            Role::Tools => {
                "helper scripts and programs for building, running or analysing the project"
            }
            Role::Integrations => {
                "files that connect the project to other software: plugins, hooks, service units, other tools' settings"
            }
            Role::Data => "input data the project reads: datasets, reference files, evidence",
            Role::Assets => "images, fonts and other static files the program ships",
            Role::ThirdParty => "code fetched or copied from other projects",
            Role::Out => "generated output: builds, logs, caches, run results (never tracked)",
            Role::Config => {
                "project-level files at the top: build manifest, README, licences, ignore files, settings"
            }
        }
    }

    /// The directory a new project gets for this role (`config` is the root).
    pub fn default_dir(self) -> &'static str {
        match self {
            Role::Source => "src",
            Role::Include => "include",
            Role::Tests => "tests",
            Role::Benches => "benches",
            Role::Examples => "examples",
            Role::Docs => "docs",
            Role::Tools => "tools",
            Role::Integrations => "integrations",
            Role::Data => "data",
            Role::Assets => "assets",
            Role::ThirdParty => "third_party",
            Role::Out => "out",
            Role::Config => ".",
        }
    }

    pub fn from_key(key: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.key() == key)
    }

    /// Roles whose files are never tracked and never decided on: what is
    /// written there is the program's or another project's, not a layout
    /// choice.
    pub fn is_unmanaged(self) -> bool {
        matches!(self, Role::Out | Role::ThirdParty)
    }
}

/// The roles a file can play, by what its extension says it is (a fixed
/// table, part of the standard). `out` can hold any kind of file and is
/// always added by the caller. None: the extension says nothing (no
/// extension, or an unknown one), so every role stays open.
pub fn plausible(name: &str) -> Option<&'static [Role]> {
    use Role::*;
    let (_, ext) = name.rsplit_once('.')?;
    let roles: &'static [Role] = match ext.to_ascii_lowercase().as_str() {
        // Program source.
        "rs" | "c" | "cc" | "cpp" | "cxx" | "py" | "go" | "ts" | "tsx" | "js" | "jsx" | "mjs"
        | "cjs" | "zig" | "java" | "kt" | "swift" | "cs" | "rb" | "lua" | "cu" | "cl" | "s"
        | "asm" => &[Source, Tests, Benches, Examples, Tools, Integrations],
        // Headers.
        "h" | "hh" | "hpp" | "hxx" => &[Source, Include, Tests],
        // Scripts.
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "awk" | "sed" | "jq" | "mk" => {
            &[Tools, Tests, Integrations, Examples]
        }
        // Writing.
        "md" | "rst" | "adoc" | "tex" | "pdf" | "org" => &[Docs, Config, Examples],
        // Structured data and settings.
        "json" | "jsonl" | "ndjson" | "toml" | "yaml" | "yml" | "csv" | "tsv" | "xml" | "ini"
        | "conf" | "cfg" | "example" => &[Config, Data, Examples, Tests, Integrations, Benches],
        // Plain text: notes, data, fixtures.
        "txt" => &[Docs, Data, Tests, Examples],
        // Logs are always a run's output.
        "log" => &[],
        // Pictures and fonts.
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" | "ttf" | "otf" | "woff"
        | "woff2" => &[Assets, Docs],
        // Service units and desktop entries.
        "service" | "socket" | "timer" | "desktop" | "plist" => &[Integrations],
        _ => return None,
    };
    Some(roles)
}

/// The role a file's name alone gives, by a convention shared across
/// languages (C, Go, Python, Rust, JavaScript): test files (`test_*`,
/// `*_test.*`, `*_tests.*`, `*.test.*`, `*_spec.*`, `*.spec.*`) and benchmark
/// files (`bench_*`, `*_bench.*`, `*.bench.*`). None: the name says nothing.
pub fn by_name(name: &str) -> Option<Role> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or(&lower);
    let dotted = |part: &str| lower.contains(&format!(".{part}."));
    if stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with("_spec")
        || dotted("test")
        || dotted("spec")
    {
        return Some(Role::Tests);
    }
    if stem.starts_with("bench_") || stem.ends_with("_bench") || dotted("bench") {
        return Some(Role::Benches);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_serde_agrees() {
        for r in Role::ALL {
            assert_eq!(Role::from_key(r.key()), Some(r));
            assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(r.key()));
            assert!(!r.description().is_empty());
        }
    }

    #[test]
    fn extensions_narrow_the_roles() {
        assert!(plausible("judge.rs").unwrap().contains(&Role::Source));
        assert!(!plausible("make-set.sh").unwrap().contains(&Role::Source));
        assert_eq!(plausible("run.log"), Some(&[][..]));
        assert_eq!(plausible("Makefile"), None);
        assert_eq!(plausible("weird.xyz"), None);
        assert!(plausible("ring.H").unwrap().contains(&Role::Include));
    }

    #[test]
    fn test_and_bench_names_are_conventions() {
        assert_eq!(by_name("test_search.c"), Some(Role::Tests));
        assert_eq!(by_name("cal_test.c"), Some(Role::Tests));
        assert_eq!(by_name("parser_test.go"), Some(Role::Tests));
        assert_eq!(by_name("app.spec.ts"), Some(Role::Tests));
        assert_eq!(by_name("bench_get.c"), Some(Role::Benches));
        assert_eq!(by_name("latest.rs"), None);
        assert_eq!(by_name("contest.md"), None);
    }
}
