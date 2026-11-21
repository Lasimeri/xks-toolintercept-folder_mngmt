//! PreToolUse: the new paths a `Write` or a `Bash` `mkdir` is about to
//! create, decided before the tool runs.
//!
//! - `Write` of a file that does not exist yet (an existing file is an
//!   overwrite, not a placement): one `create_file` projection, its summary
//!   the start of the content.
//! - `Bash`: a command without `mkdir` in its text ends the run at once.
//!   Otherwise every `mkdir` operand (`mkdir::targets`) gives the folder
//!   where the new tree attaches to the existing one (for `mkdir -p a/b/c`
//!   with nothing of it there: `a`), one `create_dir` projection each.
//!
//! Paths under the skip list (`paths::Skip`) are never decided on. Inside a
//! project (an `xks-project.json` above the path) the decision is
//! `xtf_standard::Decider`'s (rules first, the model only when they leave it
//! open), logged to the project's `.xks/decisions.jsonl`, all of a run's
//! decisions within `MODEL_BUDGET`. Outside any project the projects
//! function speaks instead: a note, once per session and folder, that
//! `new_project` and `xtf init` write a manifest.
//!
//! A verdict that suggests another path (the model's confident choice of
//! another role, or a name in the project's naming style) denies the first
//! attempt at that path in the session (mode `enforce`; the same attempt
//! again goes through, with a note that the suggestion was overridden) or
//! is added as context (mode `advise`). Three verdicts the library gives
//! are not acted on, because they would not move anything useful (see
//! `suggestion` and `declared`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use jevbase::protocol::Answer;
use serde_json::{Map, Value};
use xtf_standard::projection::summary_of;
use xtf_standard::{
    Action, Decider, Decision, MANIFEST_FILE, Manifest, Op, Projection, Role, Thresholds, Verdict,
    catalog, decide, roles,
};

use crate::Mode;
use crate::event::{self, Event};
use crate::mkdir;
use crate::paths::{self, Skip};
use crate::state::{self, Kind, Session};

/// The time the model may take in one run, all its decisions together.
/// The plugin gives the hook 30 s; a decision takes about 3 s and the
/// server answers one request at a time, so 15 s leaves room for a short
/// queue and keeps the whole run under 20 s.
pub const MODEL_BUDGET: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Write,
    Bash,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Write => "Write",
            Tool::Bash => "Bash",
        }
    }

    /// What the agent repeats to keep its path.
    fn attempt(self) -> &'static str {
        match self {
            Tool::Write => "Write",
            Tool::Bash => "mkdir",
        }
    }
}

/// A new path about to be created.
#[derive(Debug, Clone)]
struct Target {
    abs: PathBuf,
    op: Op,
    summary: String,
}

/// A target inside a project, for the decider.
struct Ask {
    abs: PathBuf,
    root: PathBuf,
    manifest: Manifest,
    projection: Projection,
}

pub fn run(ev: &Event, mode: Mode, errors: &mut Vec<String>) -> Option<Value> {
    let started = Instant::now();
    let tool = match ev.tool_name.as_str() {
        "Write" => Tool::Write,
        "Bash" => Tool::Bash,
        _ => return None,
    };
    let home = paths::home();
    let targets = match tool {
        Tool::Write => match write_target(ev) {
            Ok(t) => t.into_iter().collect(),
            Err(e) => {
                errors.push(e);
                return None;
            }
        },
        Tool::Bash => {
            let command = ev.input_str("command");
            if !command.contains("mkdir") {
                return None;
            }
            bash_targets(ev, command, home.as_deref())
        }
    };
    let skip = Skip::from_env(ev.scratchpad_dir.as_deref(), home.as_deref());
    let targets: Vec<Target> = targets
        .into_iter()
        .filter(|t| !skip.skips(&t.abs))
        .collect();
    if targets.is_empty() {
        return None;
    }

    let mut session = Session::open(state::dir().as_deref(), &ev.session_id);
    let mut denies: Vec<(PathBuf, String)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut manifests: HashMap<PathBuf, Option<Manifest>> = HashMap::new();
    let mut asks = Vec::new();
    for t in targets {
        let Some(root) = Manifest::find_root(&t.abs) else {
            notes.extend(outside_note(
                &t,
                tool,
                home.as_deref(),
                &mut session,
                errors,
            ));
            continue;
        };
        let loaded = manifests
            .entry(root.clone())
            .or_insert_with(|| Manifest::load(&root).map_err(|e| errors.push(e)).ok());
        let Some(m) = loaded.clone() else { continue };
        let Some(rel) = paths::rel(&root, &t.abs) else {
            continue;
        };
        let projection = Projection {
            op: t.op,
            path: rel,
            from: None,
            summary: t.summary,
            tool: Some(tool.name().into()),
        };
        notes.extend(new_top_note(&m, &root, &projection));
        if declared(&m, &projection) {
            continue;
        }
        if session.has(Kind::Denied, &t.abs) {
            notes.push(format!(
                "xks-folder/1, project `{}`: `{}` stays where it is: the xtf hook suggested \
                 another path for it earlier in this session, and this repeated {} goes \
                 through as is (recorded in .xks/decisions.jsonl).",
                m.name,
                projection.path,
                tool.attempt()
            ));
            log_kept(&root, &projection, errors);
            continue;
        }
        asks.push(Ask {
            abs: t.abs,
            root,
            manifest: m,
            projection,
        });
    }

    let results = decide_all(&asks, started + MODEL_BUDGET);
    for (ask, result) in asks.iter().zip(results) {
        let d = match result {
            Some(Ok(d)) => d,
            Some(Err(e)) => {
                errors.push(format!("{}: {e}", ask.abs.display()));
                continue;
            }
            None => {
                errors.push(format!(
                    "{}: no decision within {} s",
                    ask.abs.display(),
                    MODEL_BUDGET.as_secs()
                ));
                continue;
            }
        };
        if let Err(e) = decide::log(&ask.root, &ask.projection, &d) {
            errors.push(format!("decision log under {}: {e}", ask.root.display()));
        }
        if let Some(text) = suggestion(ask, &d) {
            match mode {
                Mode::Enforce => denies.push((ask.abs.clone(), text)),
                _ => notes.push(text),
            }
        }
        notes.extend(name_note(ask, &d));
    }
    output(denies, notes, tool, &mut session, errors)
}

/// The new file a `Write` creates; None when the file exists (an overwrite)
/// or is a project's manifest (writing one is what the notes ask for).
fn write_target(ev: &Event) -> Result<Option<Target>, String> {
    let file = ev.input_str("file_path");
    let p = Path::new(file);
    if !p.is_absolute() {
        return Err(format!(
            "Write with a file_path that is not absolute: `{file}`"
        ));
    }
    let abs = paths::normalize(p);
    if paths::exists(&abs) || abs.file_name().is_some_and(|n| n == MANIFEST_FILE) {
        return Ok(None);
    }
    Ok(Some(Target {
        abs,
        op: Op::CreateFile,
        summary: summary_of(ev.input_str("content")),
    }))
}

/// The folders a command's `mkdir`s create, each where its new tree
/// attaches, without repeats.
fn bash_targets(ev: &Event, command: &str, home: Option<&Path>) -> Vec<Target> {
    let cwd = Path::new(&ev.cwd);
    let cwd = cwd.is_absolute().then(|| paths::normalize(cwd));
    let summary = summary_of(ev.input_str("description"));
    let mut out: Vec<Target> = Vec::new();
    for t in mkdir::targets(command, cwd.as_deref(), home) {
        let new = if t.parents {
            paths::top_new(&t.path)
        } else {
            // Without -p only the operand is made, and only under a folder
            // that exists.
            let parent_there = t.path.parent().is_some_and(paths::exists);
            (parent_there && !paths::exists(&t.path)).then_some(t.path)
        };
        if let Some(abs) = new.filter(|a| !out.iter().any(|o| &o.abs == a)) {
            out.push(Target {
                abs,
                op: Op::CreateDir,
                summary: summary.clone(),
            });
        }
    }
    out
}

/// The projects function: a path with no manifest above it. Once per
/// session and folder: the project folder that has no manifest, or, outside
/// any project, the new folder (or the file's folder).
fn outside_note(
    t: &Target,
    tool: Tool,
    home: Option<&Path>,
    session: &mut Session,
    errors: &mut Vec<String>,
) -> Option<String> {
    let parent = t.abs.parent()?;
    let (key, text) = match paths::project_above(parent, home) {
        Some((dir, markers)) => {
            let text = format!(
                "xks-folder/1: `{}` has no {MANIFEST_FILE} (it holds {}), so the xtf hook does \
                 not decide new files and folders in it. `xtf init` writes one for an existing \
                 project; the xtf MCP tool new_project lays out a new project's folders and \
                 writes one.",
                dir.display(),
                markers.join(", ")
            );
            (dir, text)
        }
        None if tool == Tool::Bash => {
            let text = format!(
                "xks-folder/1: `{}` is a new folder outside any project (no {MANIFEST_FILE} and \
                 no project files such as .git, Cargo.toml or package.json above it). When it is \
                 a new project's root, the xtf MCP tool new_project lays out its folders and \
                 writes its {MANIFEST_FILE}; `xtf init` writes one for an existing project.",
                t.abs.display()
            );
            (t.abs.clone(), text)
        }
        None => {
            let key = paths::top_new(parent).unwrap_or_else(|| parent.to_path_buf());
            let text = format!(
                "xks-folder/1: `{}` is outside any project (no {MANIFEST_FILE} and no project \
                 files such as .git, Cargo.toml or package.json at or above it), so the xtf hook \
                 does not decide new files there. The xtf MCP tool new_project lays out a new \
                 project's folders and writes its {MANIFEST_FILE}; `xtf init` writes one for an \
                 existing project.",
                key.display()
            );
            (key, text)
        }
    };
    if session.has(Kind::Noted, &key) {
        return None;
    }
    if let Err(e) = session.add(Kind::Noted, &key) {
        errors.push(format!("state: {e}"));
    }
    Some(text)
}

/// The layout as one line: `source: crates; docs: docs; ...`.
fn layout_text(m: &Manifest) -> String {
    m.layout
        .iter()
        .map(|(r, dirs)| format!("{}: {}", r.key(), dirs.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The first component of a layout or ignore entry (`./src/tui` -> `src`).
fn first_component(d: &str) -> Option<&str> {
    d.split('/').find(|s| !s.is_empty() && *s != ".")
}

/// A layout or ignore entry as a plain relative path.
fn plain(d: &str) -> String {
    d.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether a new folder is one the manifest already declares (a layout or
/// ignore entry that does not exist yet): nothing to decide. The library
/// would read a declared top-level folder as a root file and could suggest
/// `scripts/scripts` for `mkdir scripts` when the layout maps scripts.
fn declared(m: &Manifest, p: &Projection) -> bool {
    p.op == Op::CreateDir
        && m.layout
            .values()
            .flatten()
            .chain(&m.ignore)
            .any(|d| plain(d) == p.path)
}

/// A new top-level folder the layout does not list (rules only): the
/// operation creates it, it is not a dot-folder, no layout or ignore entry
/// starts with it.
fn new_top_note(m: &Manifest, root: &Path, p: &Projection) -> Option<String> {
    let mut parts = p.path.split('/');
    let top = parts.next()?;
    if parts.next().is_none() && p.op != Op::CreateDir {
        return None;
    }
    if top.starts_with('.') || paths::exists(&root.join(top)) {
        return None;
    }
    let mapped = m
        .layout
        .values()
        .flatten()
        .chain(&m.ignore)
        .any(|d| first_component(d) == Some(top));
    if mapped {
        return None;
    }
    Some(format!(
        "xks-folder/1, project `{}`: `{top}/` is a new top-level folder that the project's \
         layout does not list (layout in {MANIFEST_FILE}: {}; files directly in the root play \
         the config role).",
        m.name,
        layout_text(m)
    ))
}

/// `path` split into its folder (`.` at the top) and its name.
fn split(path: &str) -> (&str, &str) {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .unwrap_or((".", path))
}

fn join(dir: &str, name: &str) -> String {
    if dir == "." {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The text of a suggestion that moves or renames the path; None when the
/// verdict does not suggest, or its suggestion changes nothing useful:
///
/// - the suggested folder already holds the path (`integrations/x/y.json`
///   read as integrations, whose folder is `integrations`): the library
///   puts the file at the top of the role's folder and drops the subpath;
/// - `out` decided by the extension rule only because the layout lacks
///   every role the extension allows (a `.sh` in a project without tools,
///   tests, integrations or examples): elimination, not a reading of the
///   file. A `.log`, whose only role is `out`, is still moved.
///
/// A rename (the naming style) is kept in both cases, in the path's own
/// folder.
fn suggestion(ask: &Ask, d: &Decision) -> Option<String> {
    let v = &d.verdict;
    if v.action != Action::Suggest {
        return None;
    }
    let p = &ask.projection;
    let m = &ask.manifest;
    let (old_dir, old_name) = split(&p.path);
    let (new_dir, new_name) = split(v.suggested_path.as_deref()?);
    let asked = d.answers.contains_key(catalog::FILE_ROOT);
    let by_elimination = !asked
        && v.role.as_deref() == Some(Role::Out.key())
        && roles::plausible(p.name()).is_some_and(|r| !r.is_empty());
    let inside =
        new_dir == old_dir || (new_dir != "." && old_dir.starts_with(&format!("{new_dir}/")));
    let moves = !by_elimination && !inside;
    let renames = new_name != old_name;
    if !moves && !renames {
        return None;
    }
    let dir = if moves { new_dir } else { old_dir };
    let path = join(dir, new_name);
    let mut why = Vec::new();
    if moves {
        let role = v.role.as_deref().unwrap_or("?");
        let desc = Role::from_key(role).map_or("", Role::description);
        let at = if dir == "." {
            "the project root".to_string()
        } else {
            format!("`{dir}/`")
        };
        if asked {
            why.push(format!(
                "the model reads it as {role} ({desc}), confidence {:.2}; {role} of this project \
                 goes in {at}",
                v.confidence.unwrap_or(0.0)
            ));
        } else {
            why.push(format!(
                "by its name or extension it is {role} ({desc}); {role} of this project goes in {at}"
            ));
        }
    }
    if renames {
        let (what, style) = if p.op == Op::CreateDir {
            ("folders", m.naming.dirs)
        } else {
            ("source files", m.naming.files)
        };
        let style = serde_json::to_value(style)
            .ok()
            .and_then(|s| s.as_str().map(str::to_string))
            .unwrap_or_default();
        why.push(format!(
            "`{old_name}` does not follow the project's naming style for {what} ({style}); \
             `{new_name}` does"
        ));
    }
    Some(format!(
        "xks-folder/1, project `{}`: new {} `{}`: {}. Suggested path: `{path}` ({}).",
        m.name,
        if p.op == Op::CreateDir {
            "folder"
        } else {
            "file"
        },
        p.path,
        why.join("; "),
        ask.root.join(&path).display()
    ))
}

/// `file.name_clear` at or below the policy's threshold: the name may not
/// say what the file holds.
fn name_note(ask: &Ask, d: &Decision) -> Option<String> {
    let at = Thresholds::default().name_unclear_at;
    let v = d.answers.get(catalog::FILE_NAME_CLEAR)?;
    let Ok(Answer::Noul { noul }) = serde_json::from_value::<Answer>(v.clone()) else {
        return None;
    };
    (noul <= at).then(|| {
        format!(
            "xks-folder/1, project `{}`: for `{}`, the model gives P {noul:.2} that the name \
             tells a reader what the file holds ({at:.2} or below reads as no); the xtf MCP tool \
             pick_name compares candidate names.",
            ask.manifest.name, ask.projection.path
        )
    })
}

/// The decision record of a path kept after a suggestion: allowed, by the
/// agent's repeated attempt, with no question asked.
fn log_kept(root: &Path, p: &Projection, errors: &mut Vec<String>) {
    let d = Decision {
        verdict: Verdict {
            action: Action::Allow,
            notes: vec![
                "kept by the agent: the same operation again after the xtf hook's suggestion \
                 earlier in the session"
                    .into(),
            ],
            suggested_path: None,
            role: None,
            confidence: None,
        },
        answers: Map::new(),
        seconds: 0.0,
    };
    if let Err(e) = decide::log(root, p, &d) {
        errors.push(format!("decision log under {}: {e}", root.display()));
    }
}

/// Every ask decided in order on one worker thread; what is not decided by
/// `deadline` stays None. The worker is never joined: when the run ends the
/// process exits and takes it along, whatever it was waiting for (a
/// refused, slow or rate-limited server, a name lookup).
fn decide_all(asks: &[Ask], deadline: Instant) -> Vec<Option<Result<Decision, String>>> {
    let mut out: Vec<Option<Result<Decision, String>>> = asks.iter().map(|_| None).collect();
    if asks.is_empty() {
        return out;
    }
    let jobs: Vec<(Manifest, Projection)> = asks
        .iter()
        .map(|a| (a.manifest.clone(), a.projection.clone()))
        .collect();
    let client = xtf_standard::client(None).with_timeout(MODEL_BUDGET);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let decider = Decider::new(client);
        for (i, (m, p)) in jobs.into_iter().enumerate() {
            let r = decider.decide(&m, &p).map_err(|e| e.to_string());
            if tx.send((i, r)).is_err() {
                return;
            }
        }
    });
    for _ in 0..asks.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((i, r)) => out[i] = Some(r),
            Err(_) => break,
        }
    }
    out
}

/// The one JSON object of the run: a denial when a suggestion denies a
/// path for the first time in the session, else the notes as context, else
/// nothing. A denial that cannot be recorded is given as context instead:
/// unrecorded, it would deny every repeat.
fn output(
    denies: Vec<(PathBuf, String)>,
    mut notes: Vec<String>,
    tool: Tool,
    session: &mut Session,
    errors: &mut Vec<String>,
) -> Option<Value> {
    let mut denied = Vec::new();
    let mut unrecorded = Vec::new();
    for (abs, text) in denies {
        match session.add(Kind::Denied, &abs) {
            Ok(()) => denied.push(text),
            Err(e) => {
                errors.push(format!(
                    "state: {e}; the suggestion for {} is context, not a denial",
                    abs.display()
                ));
                unrecorded.push(text);
            }
        }
    }
    if !denied.is_empty() {
        denied.push(format!(
            "The xtf hook denies a path once per session: the same {} again goes through as is.",
            tool.attempt()
        ));
        denied.extend(unrecorded);
        denied.extend(notes);
        return Some(event::deny(&denied.join("\n")));
    }
    unrecorded.append(&mut notes);
    (!unrecorded.is_empty()).then(|| event::context("PreToolUse", &unrecorded.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use xtf_standard::naming::{Naming, Style};

    fn manifest() -> Manifest {
        let mut layout = BTreeMap::new();
        layout.insert(Role::Source, vec!["crates".to_string()]);
        layout.insert(Role::Docs, vec!["docs".to_string()]);
        layout.insert(Role::Tools, vec!["scripts".to_string()]);
        Manifest {
            standard: xtf_standard::STANDARD.into(),
            name: "demo".into(),
            purpose: "A demo.".into(),
            languages: vec!["rust".into()],
            kind: None,
            layout,
            naming: Naming {
                files: Style::Snake,
                dirs: Style::Any,
            },
            ignore: vec!["target".into(), "./build/".into()],
        }
    }

    fn ask(op: Op, path: &str) -> Ask {
        Ask {
            abs: PathBuf::from("/no/such/root").join(path),
            root: PathBuf::from("/no/such/root"),
            manifest: manifest(),
            projection: Projection {
                op,
                path: path.into(),
                from: None,
                summary: String::new(),
                tool: Some("Write".into()),
            },
        }
    }

    fn decision(path: Option<&str>, role: &str, conf: f64, asked: bool) -> Decision {
        let mut answers = Map::new();
        if asked {
            answers.insert(
                catalog::FILE_ROOT.into(),
                serde_json::json!({"type":"choice","choice":role,"probabilities":{},"confidence":conf}),
            );
        }
        answers.insert(
            catalog::FILE_NAME_CLEAR.into(),
            serde_json::json!({"type":"noul","noul":0.12}),
        );
        Decision {
            verdict: Verdict {
                action: if path.is_some() {
                    Action::Suggest
                } else {
                    Action::Allow
                },
                notes: vec![],
                suggested_path: path.map(str::to_string),
                role: Some(role.into()),
                confidence: Some(conf),
            },
            answers,
            seconds: 3.0,
        }
    }

    #[test]
    fn suggestions_name_the_role_the_confidence_and_the_path() {
        let a = ask(Op::CreateFile, "crates/core/src/design.md");
        let s = suggestion(&a, &decision(Some("docs/design.md"), "docs", 0.834, true)).unwrap();
        assert!(s.contains("new file `crates/core/src/design.md`"), "{s}");
        assert!(s.contains("as docs (documentation, design notes"), "{s}");
        assert!(s.contains("confidence 0.83"), "{s}");
        assert!(s.contains("Suggested path: `docs/design.md` (/no/such/root/docs/design.md)"));
        assert!(suggestion(&a, &decision(None, "source", 0.9, true)).is_none());
        let n = name_note(&a, &decision(None, "source", 0.9, true)).unwrap();
        assert!(n.contains("P 0.12") && n.contains("pick_name"), "{n}");
    }

    #[test]
    fn suggestions_that_move_nothing_useful_are_dropped_and_renames_kept() {
        // The suggested folder already holds the path.
        let deep = ask(Op::CreateFile, "scripts/ci/run.sh");
        assert!(suggestion(&deep, &decision(Some("scripts/run.sh"), "tools", 0.9, true)).is_none());
        // `out` by elimination of the extension's roles: not moved...
        let sh = ask(Op::CreateFile, "crates/gen.sh");
        assert!(suggestion(&sh, &decision(Some("out/gen.sh"), "out", 1.0, false)).is_none());
        // ...but a log, whose only role is out, is.
        let log = ask(Op::CreateFile, "crates/run.log");
        let s = suggestion(&log, &decision(Some("out/run.log"), "out", 1.0, false)).unwrap();
        assert!(s.contains("by its name or extension it is out"), "{s}");
        // A rename survives a dropped move, in the path's own folder.
        let pascal = ask(Op::CreateFile, "scripts/ci/MakeSet.rs");
        let s = suggestion(
            &pascal,
            &decision(Some("scripts/make_set.rs"), "tools", 0.9, true),
        )
        .unwrap();
        assert!(
            s.contains("Suggested path: `scripts/ci/make_set.rs`"),
            "{s}"
        );
        assert!(
            s.contains("naming style for source files (snake_case)"),
            "{s}"
        );
        // A top-level suggestion reads as the root.
        let top = ask(Op::CreateFile, "docs/Cargo.toml");
        let s = suggestion(&top, &decision(Some("Cargo.toml"), "config", 0.7, true)).unwrap();
        assert!(s.contains("goes in the project root"), "{s}");
    }

    #[test]
    fn declared_folders_and_new_top_level_folders() {
        let m = manifest();
        let dir = |p: &str| Projection {
            op: Op::CreateDir,
            path: p.into(),
            from: None,
            summary: String::new(),
            tool: None,
        };
        assert!(declared(&m, &dir("scripts")));
        assert!(declared(&m, &dir("build")));
        assert!(!declared(&m, &dir("misc")));
        let root = Path::new("/no/such/root");
        let note = new_top_note(&m, root, &dir("misc")).unwrap();
        assert!(note.contains("`misc/` is a new top-level folder"), "{note}");
        assert!(note.contains("docs: docs; tools: scripts"), "{note}");
        assert!(new_top_note(&m, root, &dir(".github")).is_none());
        assert!(new_top_note(&m, root, &dir("crates")).is_none());
        let file = |p: &str| Projection {
            op: Op::CreateFile,
            ..dir(p)
        };
        assert!(new_top_note(&m, root, &file("notes.md")).is_none());
        assert!(new_top_note(&m, root, &file("misc/notes.md")).is_some());
        assert_eq!(split("a/b/c.rs"), ("a/b", "c.rs"));
        assert_eq!(split("c.rs"), (".", "c.rs"));
    }
}
