//! Naming styles: a rule, never a model question. The file style governs
//! the project's own source files (`.rs` in a Rust project, `.c` and `.h` in
//! a C project, ...), not its scripts, documents or data; the folder style
//! governs new folders. A governed name's stem (the part before its first
//! dot) is checked against the style and converted when it does not follow
//! it. Names whose form is fixed by convention (README, LICENSE, Makefile,
//! Cargo.toml, dotfiles) are exempt.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[serde(rename = "kebab-case")]
    Kebab,
    #[serde(rename = "snake_case")]
    Snake,
    #[serde(rename = "PascalCase")]
    Pascal,
    #[serde(rename = "camelCase")]
    Camel,
    #[serde(rename = "lowercase")]
    Lower,
    /// Not checked (Rust keeps crate folders in kebab-case and module
    /// folders in snake_case side by side).
    #[serde(rename = "any")]
    Any,
}

/// The project's styles for file and directory names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Naming {
    pub files: Style,
    pub dirs: Style,
}

impl Naming {
    /// The usual styles of a project's first language (kebab-case when none
    /// is known): Rust, C, C++, Python, Go and Zig name source files in
    /// snake_case and leave folders free; JavaScript, TypeScript, web and
    /// documentation projects use kebab-case for both.
    pub fn for_languages(languages: &[String]) -> Naming {
        let first = languages.first().map(|l| l.to_ascii_lowercase());
        match first.as_deref() {
            Some("rust" | "c" | "c++" | "cpp" | "python" | "go" | "zig") => Naming {
                files: Style::Snake,
                dirs: Style::Any,
            },
            _ => Naming {
                files: Style::Kebab,
                dirs: Style::Kebab,
            },
        }
    }
}

/// The source-file extensions of a language.
fn extensions(language: &str) -> &'static [&'static str] {
    match language.to_ascii_lowercase().as_str() {
        "rust" => &["rs"],
        "c" => &["c", "h"],
        "c++" | "cpp" => &["cc", "cpp", "cxx", "hh", "hpp", "hxx", "h"],
        "python" => &["py"],
        "go" => &["go"],
        "zig" => &["zig"],
        "typescript" => &["ts", "tsx", "mts", "cts"],
        "javascript" => &["js", "jsx", "mjs", "cjs"],
        _ => &[],
    }
}

/// Whether the file style governs `name`: a source file of one of the
/// project's languages. With no languages known, every file is governed.
pub fn governs(name: &str, languages: &[String]) -> bool {
    if languages.is_empty() {
        return true;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    languages.iter().any(|l| extensions(l).contains(&ext))
}

/// Names that keep their conventional form whatever the style.
const CONVENTIONAL: &[&str] = &[
    "README",
    "LICENSE",
    "LICENCE",
    "NOTICE",
    "COPYING",
    "AUTHORS",
    "CHANGELOG",
    "CONTRIBUTING",
    "SECURITY",
    "CLAUDE",
    "AGENTS",
    "Makefile",
    "GNUmakefile",
    "Cargo",
    "CMakeLists",
    "Dockerfile",
    "Containerfile",
    "Justfile",
    "build",
    "package",
    "tsconfig",
    "go",
    "UPSTREAM",
    "SPEC",
];

/// Whether `name` keeps a conventional form (exempt from styles).
pub fn is_conventional(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    let stem = stem(name);
    CONVENTIONAL.iter().any(|c| {
        stem == *c || stem.starts_with(&format!("{c}-")) || stem.starts_with(&format!("{c}_"))
    })
}

/// The part of a name before its first dot.
pub fn stem(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// The words of a name's stem: split on `-`, `_`, spaces and lower-to-upper
/// case changes, lowercased.
pub fn words(stem: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in stem.chars() {
        if c == '-' || c == '_' || c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether `name` (its stem) follows `style`; conventional names always do.
pub fn follows(name: &str, style: Style) -> bool {
    if is_conventional(name) {
        return true;
    }
    stem(name) == convert_stem(stem(name), style)
}

fn convert_stem(stem: &str, style: Style) -> String {
    if style == Style::Any {
        return stem.to_string();
    }
    let w = words(stem);
    let cap = |s: &String| {
        let mut c = s.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    };
    match style {
        Style::Kebab => w.join("-"),
        Style::Snake => w.join("_"),
        Style::Lower => w.concat(),
        Style::Pascal => w.iter().map(cap).collect(),
        Style::Camel => w
            .iter()
            .enumerate()
            .map(|(i, s)| if i == 0 { s.clone() } else { cap(s) })
            .collect(),
        Style::Any => unreachable!("returned above"),
    }
}

/// `name` in `style`, its extension kept; conventional names unchanged.
pub fn convert(name: &str, style: Style) -> String {
    if is_conventional(name) {
        return name.to_string();
    }
    let s = stem(name);
    format!("{}{}", convert_stem(s, style), &name[s.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_check_and_convert() {
        assert!(follows("serve_and_ask.sh", Style::Snake));
        assert!(!follows("serve-and-ask.sh", Style::Snake));
        assert_eq!(
            convert("serve-and-ask.sh", Style::Snake),
            "serve_and_ask.sh"
        );
        assert_eq!(
            convert("ToolIntercept.rs", Style::Kebab),
            "tool-intercept.rs"
        );
        assert_eq!(convert("tool_intercept", Style::Pascal), "ToolIntercept");
        assert_eq!(convert("tool intercept", Style::Camel), "toolIntercept");
        assert_eq!(
            convert("jevbase.conf.example", Style::Kebab),
            "jevbase.conf.example"
        );
        assert!(follows("README.md", Style::Snake));
        assert!(follows("LICENSE-MIT", Style::Kebab));
        assert!(follows(".gitignore", Style::Pascal));
        assert!(follows("Cargo.toml", Style::Kebab));
        assert_eq!(words("parseHTTPReply"), vec!["parse", "httpreply"]);
        assert!(follows("xtf-mcp", Style::Any) && follows("my_module", Style::Any));
        assert_eq!(convert("Some-Dir", Style::Any), "Some-Dir");
    }

    #[test]
    fn styles_govern_source_files_only() {
        let rust = vec!["rust".to_string()];
        assert!(governs("tool_call.rs", &rust));
        assert!(!governs("make-set.sh", &rust));
        assert!(!governs("SPEC.md", &rust));
        assert!(!governs("Makefile", &rust));
        let c = vec!["c".to_string(), "rust".to_string()];
        assert!(governs("ring.h", &c) && governs("main.rs", &c));
        assert!(governs("anything.md", &[]));
    }

    #[test]
    fn language_defaults() {
        assert_eq!(Naming::for_languages(&["rust".into()]).files, Style::Snake);
        assert_eq!(
            Naming::for_languages(&["typescript".into()]).dirs,
            Style::Kebab
        );
        assert_eq!(Naming::for_languages(&[]).files, Style::Kebab);
    }
}
