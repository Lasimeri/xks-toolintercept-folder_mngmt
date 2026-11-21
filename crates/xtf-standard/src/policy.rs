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
    /// A different path (`suggested_path`), a change of the layout
    /// (`layout_change`), or both, are suggested.
    Suggest,
}

/// A mapping the manifest's `layout` lacks: `dir` (relative to the root)
/// plays `role`. Suggested, never written by the standard itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutChange {
    pub role: String,
    pub dir: String,
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
    /// The layout mapping that would make the chosen role a folder of the
    /// project (since 0.3.0): a confident role the layout lacks, for a path
    /// outside the layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_change: Option<LayoutChange>,
}

impl Verdict {
    fn allow(note: &str) -> Verdict {
        Verdict {
            action: Action::Allow,
            notes: vec![note.to_string()],
            suggested_path: None,
            role: None,
            confidence: None,
            layout_change: None,
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
    if p.op == Op::CreateDir && m.declares(&p.path) {
        return Pre::Done(Verdict::allow(
            "a folder the manifest names (layout or ignore): nothing to decide",
        ));
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
        layout_change: None,
    };
    let name = p.name();
    let here = parent(&p.path).to_string();
    let mut dir = here.clone();
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
        let sure = *confidence >= th.root_min_confidence;
        let lacking =
            chosen.filter(|c| !matches!(c, Role::Config | Role::Out) && !m.layout.contains_key(c));
        if fits && sure {
            v.notes.push(format!(
                "it belongs in {choice} (confidence {confidence:.2})"
            ));
        } else if fits {
            v.notes.push(format!(
                "stays in {choice}: the model leans the same way (confidence {confidence:.2})"
            ));
        } else if let (true, Some(c)) = (sure, lacking) {
            // A role the layout lacks (a path outside the layout, whose
            // options are not limited to the layout's): the layout is what
            // changes. A folder outside the layout, or the top-level folder
            // a file goes under, is mapped as that role where it is; a file
            // at the root goes to the role's usual folder, mapped as well.
            let top = match &place {
                Place::Unmapped(top) => Some(top.clone()),
                Place::Root if p.op == Op::CreateDir => Some(p.path.trim_matches('/').to_string()),
                _ => None,
            };
            let map_dir = top.unwrap_or_else(|| {
                dir = c.default_dir().to_string();
                dir.clone()
            });
            v.notes.push(format!(
                "it reads as {choice} (confidence {confidence:.2}); the layout has no {choice} \
                 folder: `{map_dir}/` would play that role"
            ));
            v.layout_change = Some(LayoutChange {
                role: choice.clone(),
                dir: map_dir,
            });
            v.action = Action::Suggest;
        } else if sure {
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
    if v.action == Action::Suggest && (dir != here || styled != name) {
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
        // A folder the manifest names is what the manifest already says; a
        // folder it does not name is decided (the model is asked).
        assert!(matches!(
            rules(&m, &proj(Op::CreateDir, "tests")),
            Pre::Done(_)
        ));
        assert!(matches!(
            rules(&m, &proj(Op::CreateDir, "./tests/")),
            Pre::Done(_)
        ));
        assert!(matches!(
            rules(&m, &proj(Op::CreateDir, "misc")),
            Pre::Ask { .. }
        ));
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

    #[test]
    fn a_confident_role_the_layout_lacks_changes_the_layout() {
        let mut m = Manifest::new("demo", "A demo.", vec!["rust".into()]);
        m.layout.clear();
        m.layout.insert(Role::Source, vec!["crates".into()]);
        m.layout.insert(Role::Docs, vec!["docs".into()]);
        let th = Thresholds::default();
        let ask = |op: Op, path: &str, role: &str, conf: f64| {
            let p = proj(op, path);
            decide(&m, &p, p.name(), Some(&choice(role, conf)), None, &th)
        };
        let change = |role: &str, dir: &str| {
            Some(LayoutChange {
                role: role.into(),
                dir: dir.into(),
            })
        };
        // A new top-level folder: mapped where it is, never moved.
        let d = ask(Op::CreateDir, "scripts", "tools", 0.7);
        assert_eq!(d.action, Action::Suggest);
        assert_eq!(d.suggested_path, None);
        assert_eq!(d.layout_change, change("tools", "scripts"));
        // A file under an unmapped top-level folder: the folder is mapped.
        let d = ask(Op::CreateFile, "scripts/build.sh", "tools", 0.7);
        assert_eq!(d.suggested_path, None);
        assert_eq!(d.layout_change, change("tools", "scripts"));
        // A file at the root: the role's usual folder, mapped as well.
        let d = ask(Op::CreateFile, "bench.sh", "tools", 0.7);
        assert_eq!(d.suggested_path.as_deref(), Some("tools/bench.sh"));
        assert_eq!(d.layout_change, change("tools", "tools"));
        // A role the layout has: moved into it, the layout untouched.
        let d = ask(Op::CreateFile, "scripts/guide.md", "docs", 0.7);
        assert_eq!(d.suggested_path.as_deref(), Some("docs/guide.md"));
        assert_eq!(d.layout_change, None);
        // Unsure: nothing changes.
        let d = ask(Op::CreateDir, "scripts", "tools", 0.3);
        assert_eq!(d.action, Action::Uncertain);
        assert_eq!(d.layout_change, None);
        // Where it is going, under the floor: says it stays, claims no more.
        let d = ask(Op::CreateFile, "crates/a/x.rs", "source", 0.45);
        assert_eq!(d.action, Action::Allow);
        assert!(d.notes[0].starts_with("stays in source: the model leans the same way"));
    }
}
