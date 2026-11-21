//! The question catalog of standard `xks-folder/1`: every question the
//! Jev-like model is ever asked about a project's folders and names, with a
//! stable id, fixed wording and options drawn only from fixed sets (the
//! roles, the kinds) or from candidates the calling agent supplies. The
//! model never writes a name or a path; it picks.
//!
//! | id               | type   | asked when                                   |
//! |------------------|--------|----------------------------------------------|
//! | `file.root`      | choice | a new file or folder: which role it plays    |
//! | `file.name_clear`| noul   | a new file: does its name say what it holds  |
//! | `name.pick`      | choice | the agent offers candidate names: which one  |
//! | `project.kind`   | choice | a new manifest: what kind of project         |
//!
//! Every question is asked against the project's manifest as the state
//! (`Manifest::state`), so a running server reuses it from one decision to
//! the next.

use serde_json::{Map, Value, json};

use crate::manifest::Manifest;
use crate::projection::{Op, Projection};
use crate::roles::{self, Role};

pub const FILE_ROOT: &str = "file.root";
pub const FILE_NAME_CLEAR: &str = "file.name_clear";
pub const NAME_PICK: &str = "name.pick";
pub const PROJECT_KIND: &str = "project.kind";

/// Project kinds, in option order, with their fixed descriptions.
pub const KINDS: &[(&str, &str)] = &[
    ("library", "code other programs link to or import"),
    ("cli-tool", "a command-line program people run"),
    ("service", "a server or daemon other programs talk to"),
    (
        "interactive-app",
        "a terminal or graphical application people use directly",
    ),
    ("website", "a site or web front end"),
    (
        "firmware-driver",
        "code for a device: firmware, a kernel module or a driver",
    ),
    ("research", "experiments, measurements and their write-ups"),
    (
        "data-pipeline",
        "programs that move or transform data in stages",
    ),
    (
        "documents",
        "mostly writing: notes, a book, a specification",
    ),
    (
        "configuration",
        "settings and dotfiles for a machine or a tool",
    ),
    ("mixed", "several of these at once, side by side"),
];

/// The thing a projection is, in a sentence the questions start with.
fn subject(p: &Projection) -> String {
    let (what, holds) = match p.op {
        Op::CreateDir => ("A new folder", "What it will hold"),
        Op::Move | Op::Rename => (
            "A file or folder is about to be moved; its new",
            "What it holds",
        ),
        _ => ("A new file", "Beginning of its content"),
    };
    let summary = if p.summary.trim().is_empty() {
        "(not given)".to_string()
    } else {
        p.summary.clone()
    };
    if matches!(p.op, Op::Move | Op::Rename) {
        format!("{what} name: {}\n{holds}:\n{summary}", p.name())
    } else {
        format!(
            "{what} is about to be added to this project.\nName: {}\n{holds}:\n{summary}",
            p.name()
        )
    }
}

/// The roles `file.root` offers: the project's (its layout's, `out`,
/// `config`). For a file: the one its name gives by convention
/// (`roles::by_name`, a test or benchmark file) when the layout has it;
/// else those its extension makes plausible (`roles::plausible`), `out`
/// always kept. One role left means a rule decides and the question is not
/// asked.
pub fn root_options(m: &Manifest, p: &Projection) -> Vec<Role> {
    let roles = m.roles();
    if p.op == Op::CreateDir {
        return roles;
    }
    if let Some(r) = roles::by_name(p.name()).filter(|r| roles.contains(r)) {
        return vec![r];
    }
    match roles::plausible(p.name()) {
        None => roles,
        Some(ok) => roles
            .into_iter()
            .filter(|r| *r == Role::Out || ok.contains(r))
            .collect(),
    }
}

/// `file.root`: which of `root_options` the new file or folder plays.
pub fn file_root(m: &Manifest, p: &Projection) -> Value {
    let criteria: Map<String, Value> = root_options(m, p)
        .into_iter()
        .map(|r: Role| (r.key().to_string(), json!(r.description())))
        .collect();
    json!({
        "type": "choice",
        "instructions": format!("{}\nWhich part of the project does it belong in?", subject(p)),
        "criteria": criteria,
    })
}

/// `file.name_clear`: whether a new file's name says what it holds.
pub fn file_name_clear(p: &Projection) -> Value {
    json!({
        "type": "noul",
        "instructions": format!("{}\nDoes the name tell a reader what the file holds?", subject(p)),
    })
}

/// `name.pick`: the best of the agent's candidate names for `what`.
pub fn name_pick(what: &str, candidates: &[String]) -> Value {
    let criteria: Map<String, Value> = candidates
        .iter()
        .map(|c| (c.clone(), Value::Null))
        .collect();
    json!({
        "type": "choice",
        "instructions": format!(
            "A name is needed for: {what}\nWhich name fits best: short, says what it holds, \
             and reads like the project's other names?"
        ),
        "criteria": criteria,
    })
}

/// `project.kind`: what kind of project the manifest describes.
pub fn project_kind() -> Value {
    let criteria: Map<String, Value> = KINDS
        .iter()
        .map(|(k, d)| (k.to_string(), json!(d)))
        .collect();
    json!({
        "type": "choice",
        "instructions": "What kind of project is this?",
        "criteria": criteria,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevbase::protocol::{Question, parse_questions};

    #[test]
    fn every_question_parses_as_system_one() {
        let m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        let p = Projection {
            op: Op::CreateFile,
            path: "src/x.rs".into(),
            from: None,
            summary: "//! x".into(),
            tool: None,
        };
        let mut qs = Map::new();
        qs.insert(FILE_ROOT.into(), file_root(&m, &p));
        qs.insert(FILE_NAME_CLEAR.into(), file_name_clear(&p));
        qs.insert(
            NAME_PICK.into(),
            name_pick("a parser", &["parse.rs".into(), "p.rs".into()]),
        );
        qs.insert(PROJECT_KIND.into(), project_kind());
        let parsed = parse_questions(&qs).unwrap();
        assert_eq!(parsed.len(), 4);
        match &parsed[0].1 {
            Question::Choice { criteria, .. } => {
                // A .rs file: source, tests, tools of the layout, and out.
                let keys: Vec<&String> = criteria.keys().collect();
                assert_eq!(keys, ["source", "tests", "tools", "out"]);
            }
            other => panic!("file.root is a choice, not {other:?}"),
        }
    }
}
