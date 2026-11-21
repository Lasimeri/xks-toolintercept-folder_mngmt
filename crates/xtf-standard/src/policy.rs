//! From an operation and the model's answers to an action. Rules first,
//! without the model: deletions and paths the project does not manage are
//! allowed, conventional root files are allowed, a name that breaks the
//! project's naming style gets the converted name. Then the answers:
//! `file.root` names the role the new path should play; when it differs
//! from where the path is going and the model is confident enough, the
//! verdict suggests the role's directory; when it is not, the verdict is
//! uncertain and nothing is suggested.

use jevbase::protocol::Answer;
use serde::Serialize;

use crate::manifest::{Manifest, Place};
use crate::naming;
use crate::projection::{Op, Projection};
use crate::roles::Role;

/// When an answer is strong enough to act on, as TypeSafe's guidance on
/// confidence puts it (https://docs.typesafe.ai/confidence.md, read
/// 2026-11-21): a Choice under 0.5 confidence is "genuinely unsure" and is
/// not acted on; a Noul reads yes above 0.7 and no below 0.3, the band
/// between its own outcome.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// `file.root`: the least Choice confidence (0 uniform, 1 certain) at
    /// which a different role is suggested. 0.5 is the documented floor and
    /// the calibrated one: on 118 real files (calibration/results-v2.txt) it
    /// answered 32% of them, 89.5% right, and would have moved 3.4% of files
    /// that were already where they belong.
    pub root_min_confidence: f64,
    /// `file.name_clear`: P(yes) at or below which the name reads as a no.
    pub name_unclear_at: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            root_min_confidence: 0.50,
            name_unclear_at: 0.30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Fits the standard, or nothing to decide.
    Allow,
    /// The model could not tell; nothing is suggested.
    Uncertain,
    /// A different path is suggested (`suggested_path`).
    Suggest,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub action: Action,
    /// Why, one sentence each.
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_path: Option<String>,
    /// The role `file.root` chose, and its Choice confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl Verdict {
    fn allow(note: &str) -> Verdict {
        Verdict {
            action: Action::Allow,
            notes: vec![note.to_string()],
            suggested_path: None,
            role: None,
            confidence: None,
        }
    }
}

/// What the rules leave for the model.
#[derive(Debug, Clone, PartialEq)]
pub enum Pre {
    /// Decided without the model.
    Done(Verdict),
    /// Ask `file.root` (and maybe `file.name_clear`); `styled` is the name
    /// in the project's naming style.
    Ask { styled: String },
}

/// The rules, before any question.
pub fn rules(m: &Manifest, p: &Projection) -> Pre {
    if p.op == Op::Delete {
        return Pre::Done(Verdict::allow("deletions are not decided on"));
    }
    let place = m.place(std::path::Path::new(&p.path));
    if place == Place::Unmanaged {
        return Pre::Done(Verdict::allow(
            "the path is generated or vendored: not decided on",
        ));
    }
    let name = p.name();
    if place == Place::Root && naming::is_conventional(name) {
        return Pre::Done(Verdict::allow("a conventional top-level file"));
    }
    let style = if p.op == Op::CreateDir {
        Some(m.naming.dirs)
    } else if naming::governs(name, &m.languages) {
        Some(m.naming.files)
    } else {
        None
    };
    Pre::Ask {
        styled: style.map_or_else(|| name.to_string(), |s| naming::convert(name, s)),
    }
}

/// The parent directory of `path` (`.` for a top-level name).
fn parent(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map_or(".", |(d, _)| d)
}

fn join(dir: &str, name: &str) -> String {
    if dir == "." || dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{name}", dir.trim_end_matches('/'))
    }
}

/// The verdict once the questions are answered.
pub fn decide(
    m: &Manifest,
    p: &Projection,
    styled: &str,
    root: Option<&Answer>,
    name_clear: Option<&Answer>,
    th: &Thresholds,
) -> Verdict {
    let mut v = Verdict {
        action: Action::Allow,
        notes: Vec::new(),
        suggested_path: None,
        role: None,
        confidence: None,
    };
    let name = p.name();
    let mut dir = parent(&p.path).to_string();
    if styled != name {
        v.action = Action::Suggest;
        v.notes.push(format!(
            "`{name}` does not follow the project's naming style; `{styled}` does"
        ));
    }
    let place = m.place(std::path::Path::new(&p.path));
    if let Place::Unmapped(top) = &place {
        v.notes.push(format!(
            "`{top}` is not a directory of the project's layout"
        ));
    }
    if let Some(Answer::Choice {
        choice, confidence, ..
    }) = root
    {
        v.role = Some(choice.clone());
        v.confidence = Some(*confidence);
        let chosen = Role::from_key(choice);
        let fits = match (&place, chosen) {
            (Place::Root, Some(Role::Config)) => true,
            (Place::Role(r), Some(c)) => *r == c,
            _ => false,
        };
        if fits {
            v.notes.push(format!(
                "it belongs in {choice} (confidence {confidence:.2})"
            ));
        } else if *confidence >= th.root_min_confidence {
            let target = chosen.map(|c| m.dir_for(c)).unwrap_or_else(|| ".".into());
            v.notes.push(format!(
                "it belongs in {choice} ({}), not where it is going (confidence {confidence:.2})",
                if target == "." {
                    "the project root".to_string()
                } else {
                    format!("{target}/")
                }
            ));
            dir = target;
            v.action = Action::Suggest;
        } else {
            v.notes.push(format!(
                "the model leans to {choice} but is unsure (confidence {confidence:.2}): left as is"
            ));
            v.action = v.action.max(Action::Uncertain);
        }
    }
    if let Some(Answer::Noul { noul }) = name_clear {
        if *noul <= th.name_unclear_at {
            v.notes.push(format!(
                "the name may not say what the file holds (P {noul:.2}); offer candidates to name.pick"
            ));
            v.action = v.action.max(Action::Uncertain);
        }
    }
    if v.action == Action::Suggest {
        v.suggested_path = Some(join(&dir, styled));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn proj(op: Op, path: &str) -> Projection {
        Projection {
            op,
            path: path.into(),
            from: None,
            summary: String::new(),
            tool: None,
        }
    }

    fn choice(key: &str, confidence: f64) -> Answer {
        Answer::Choice {
            choice: key.into(),
            probabilities: Map::new(),
            confidence,
        }
    }

    #[test]
    fn rules_settle_what_needs_no_model() {
        let m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        assert!(matches!(
            rules(&m, &proj(Op::Delete, "src/a.rs")),
            Pre::Done(_)
        ));
        assert!(matches!(
            rules(&m, &proj(Op::CreateFile, "out/run.log")),
            Pre::Done(_)
        ));
        assert!(matches!(
            rules(&m, &proj(Op::CreateFile, "README.md")),
            Pre::Done(_)
        ));
        assert_eq!(
            rules(&m, &proj(Op::CreateFile, "src/ParseIt.rs")),
            Pre::Ask {
                styled: "parse_it.rs".into()
            }
        );
    }

    #[test]
    fn answers_become_actions() {
        let m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        let th = Thresholds::default();
        let p = proj(Op::CreateFile, "src/bench_tables.rs");
        let fits = decide(
            &m,
            &p,
            "bench_tables.rs",
            Some(&choice("source", 0.9)),
            None,
            &th,
        );
        assert_eq!(fits.action, Action::Allow);
        let moved = decide(
            &m,
            &p,
            "bench_tables.rs",
            Some(&choice("tools", 0.6)),
            None,
            &th,
        );
        assert_eq!(moved.action, Action::Suggest);
        assert_eq!(
            moved.suggested_path.as_deref(),
            Some("tools/bench_tables.rs")
        );
        let unsure = decide(
            &m,
            &p,
            "bench_tables.rs",
            Some(&choice("tools", 0.1)),
            None,
            &th,
        );
        assert_eq!(unsure.action, Action::Uncertain);
        assert!(unsure.suggested_path.is_none());
        let renamed = decide(
            &m,
            &proj(Op::CreateFile, "src/BenchTables.rs"),
            "bench_tables.rs",
            Some(&choice("source", 0.9)),
            None,
            &th,
        );
        assert_eq!(
            renamed.suggested_path.as_deref(),
            Some("src/bench_tables.rs")
        );
        let top = decide(
            &m,
            &proj(Op::CreateFile, "notes.md"),
            "notes.md",
            Some(&choice("docs", 0.5)),
            None,
            &th,
        );
        assert_eq!(top.suggested_path.as_deref(), Some("docs/notes.md"));
    }
}
