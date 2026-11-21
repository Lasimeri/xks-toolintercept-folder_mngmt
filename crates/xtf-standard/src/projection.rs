//! The projected JSON: one operation an agent is about to make in a project
//! (create a file or a folder, move, rename, delete), with what it holds.
//! The hook builds it from an intercepted tool call; an agent hands it to
//! the MCP server.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    CreateFile,
    CreateDir,
    Move,
    Rename,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub op: Op,
    /// The path the operation creates or changes, relative to the project
    /// root (for a move or rename: the new path).
    pub path: String,
    /// For a move or rename: the old path, relative to the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// What the file or folder holds: the agent's description, or the start
    /// of the content being written (`summary_of`).
    #[serde(default)]
    pub summary: String,
    /// The tool call it came from (`Write`, `Bash`, ...), when intercepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Longest summary put into a question (characters). Every character is
/// read again for each question and rotation, so it stays short.
pub const SUMMARY_CHARS: usize = 240;

/// The start of `content` as a summary: control characters other than
/// newlines dropped, blank lines collapsed, at most `SUMMARY_CHARS`.
pub fn summary_of(content: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in content.lines() {
        let line: String = line
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect::<String>()
            .trim_end()
            .to_string();
        if line.trim().is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank {
                out.push('\n');
            }
        }
        blank = false;
        out.push_str(&line);
        if out.chars().count() >= SUMMARY_CHARS {
            break;
        }
    }
    out.chars().take(SUMMARY_CHARS).collect()
}

impl Projection {
    /// The last component of `path`.
    pub fn name(&self) -> &str {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_short_and_clean() {
        let s = summary_of("\n\n//! Title\n\n\n//! more\u{7}\n");
        assert_eq!(s, "//! Title\n\n//! more");
        assert_eq!(summary_of(&"x".repeat(1000)).chars().count(), SUMMARY_CHARS);
        let p = Projection {
            op: Op::CreateFile,
            path: "crates/a/src/lib.rs".into(),
            from: None,
            summary: String::new(),
            tool: None,
        };
        assert_eq!(p.name(), "lib.rs");
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("\"create_file\"") && !j.contains("from"));
    }
}
