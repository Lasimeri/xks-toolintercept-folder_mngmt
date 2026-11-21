//! The six tools. Each checks its arguments against its schema here (a
//! client may send anything) and returns an `Outcome`: a one-line summary,
//! then the result as compact JSON; or a plain message marked as an error
//! when it cannot answer (no manifest, arguments that do not fit, the
//! decision server down, refusing or too slow). Paths in arguments are
//! absolute, `~/...`, or relative to the server's working directory.
//!
//! | tool           | model questions                       | writes                         |
//! |----------------|---------------------------------------|--------------------------------|
//! | `place_file`   | file.root, file.name_clear (`Decider`) | one line of .xks/decisions.jsonl |
//! | `plan_tree`    | file.root per open file, batched      | one line per planned path      |
//! | `pick_name`    | name.pick                             | nothing                        |
//! | `new_project`  | project.kind, unless `kind` is given  | the manifest, folders (mkdir), the kind decision |
//! | `extend_layout`| none (the caller names the role)      | the manifest's layout, folders (mkdir), one line per folder |
//! | `project_info` | none                                  | nothing                        |
//!
//! `place_file` and `plan_tree` also report layout gaps (`layout`): planned
//! paths outside every layout folder, which `extend_layout` closes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use jevbase::protocol::{Answer, Request};
use serde_json::{Map, Value, json};
use xtf_standard::decide::{self, Decision, LOG_FILE};
use xtf_standard::policy::{self, Pre};
use xtf_standard::projection::summary_of;
use xtf_standard::{
    Action, Decider, LayoutChange, MANIFEST_FILE, Manifest, Op, Projection, Role, STANDARD,
    Thresholds, Verdict, catalog, naming, scaffold,
};

use crate::{ask, layout};

/// Most planned paths in one `plan_tree` call.
pub const MAX_PLAN: usize = 64;
/// Candidate names `pick_name` takes (the options are lettered A to Z).
const CANDIDATES: std::ops::RangeInclusive<usize> = 2..=26;
/// Longest candidate name, in characters.
const CANDIDATE_CHARS: usize = 200;
/// Most folders one `extend_layout` call adds.
pub const MAX_EXTEND: usize = 8;

/// The roles `extend_layout` takes: every role but `config` (the root).
pub fn layout_roles() -> Vec<&'static str> {
    Role::ALL
        .into_iter()
        .filter(|r| *r != Role::Config)
        .map(Role::key)
        .collect()
}

/// A tool's answer: the text Claude reads, and whether the call failed.
#[derive(Debug)]
pub struct Outcome {
    pub text: String,
    pub is_error: bool,
}

impl Outcome {
    fn ok(summary: &str, data: &Value) -> Outcome {
        Outcome {
            text: format!("{summary}\n{data}"),
            is_error: false,
        }
    }

    fn fail(message: String) -> Outcome {
        Outcome {
            text: message,
            is_error: true,
        }
    }
}

/// Run tool `name` on `args`.
pub fn call(name: &str, args: &Map<String, Value>) -> Outcome {
    let a = Args(args);
    let r = match name {
        "new_project" => new_project(&a),
        "plan_tree" => plan_tree(&a),
        "place_file" => place_file(&a),
        "pick_name" => pick_name(&a),
        "extend_layout" => extend_layout(&a),
        "project_info" => project_info(&a),
        other => Err(format!("unknown tool `{other}`")),
    };
    r.unwrap_or_else(Outcome::fail)
}

/// A call's arguments, read as its schema says.
struct Args<'a>(&'a Map<String, Value>);

impl Args<'_> {
    /// Err naming the first argument the tool does not take.
    fn only(&self, keys: &[&str]) -> Result<(), String> {
        match self.0.keys().find(|k| !keys.contains(&k.as_str())) {
            Some(k) => Err(format!(
                "unknown argument `{k}`; this tool takes {}",
                keys.join(", ")
            )),
            None => Ok(()),
        }
    }

    fn text(&self, k: &str) -> Result<Option<String>, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("`{k}` must be a string")),
        }
    }

    fn need(&self, k: &str) -> Result<String, String> {
        self.text(k)?
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("`{k}` is required (a non-empty string)"))
    }

    fn texts(&self, k: &str) -> Result<Option<Vec<String>>, String> {
        let bad = || format!("`{k}` must be a list of strings");
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or_else(bad))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(bad()),
        }
    }

    fn flag(&self, k: &str) -> Result<Option<bool>, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(format!("`{k}` must be true or false")),
        }
    }
}

/// `raw` (absolute, `~/...`, or relative to the working directory) as an
/// absolute path, `.` and `..` resolved by name (the path need not exist).
fn absolute(raw: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("working directory: {e}"))?;
    absolute_from(&cwd, raw)
}

/// `raw` against `base`, as `absolute`.
fn absolute_from(base: &Path, raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    let p = if raw == "~" || raw.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or("the path starts with `~` and HOME is unset")?;
        PathBuf::from(home).join(raw[1..].trim_start_matches('/'))
    } else {
        PathBuf::from(raw)
    };
    let joined = if p.is_absolute() { p } else { base.join(p) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// `abs` relative to `root`, `/`-separated; None outside it or at it.
fn inside(root: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(root)
        .ok()
        .map(|r| r.to_string_lossy().into_owned())
        .filter(|r| !r.is_empty())
}

fn shown(p: &Path) -> String {
    p.display().to_string()
}

/// The project at or above `abs`: its root and manifest.
fn project(abs: &Path) -> Result<(PathBuf, Manifest), String> {
    let root = Manifest::find_root(abs).ok_or_else(|| {
        format!(
            "no {MANIFEST_FILE} at or above {}: the xks-folder/1 decisions are made inside a \
             project that has one; new_project writes one (purpose, languages, kind, folder layout)",
            abs.display()
        )
    })?;
    let m = Manifest::load(&root)?;
    Ok((root, m))
}

/// The start of an existing regular file, as a summary.
fn head_of(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    f.take(8192).read_to_end(&mut buf).ok()?;
    Some(summary_of(&String::from_utf8_lossy(&buf)))
}

/// This process's appends to decision logs, one at a time.
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Append `d` to the project's decision log; the log's path, or why not.
fn log(root: &Path, p: &Projection, d: &Decision) -> Value {
    let _one = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match decide::log(root, p, d) {
        Ok(()) => json!(LOG_FILE),
        Err(e) => json!(format!("not logged: {e}")),
    }
}

fn round3(x: f64) -> f64 {
    (x * 1e3).round() / 1e3
}

fn op_key(op: Op) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn parse_op(o: &str) -> Result<Op, String> {
    serde_json::from_value(json!(o.trim())).map_err(|_| {
        format!("unknown op `{o}`; the ops are create_file, create_dir, move, rename, delete")
    })
}

/// `path: action; notes` of one verdict.
fn verdict_text(path: &str, v: &Verdict) -> String {
    let action = match v.action {
        Action::Allow => "allow".to_string(),
        Action::Uncertain => "uncertain".to_string(),
        Action::Suggest => format!("suggest {}", v.suggested_path.as_deref().unwrap_or("")),
    };
    if v.notes.is_empty() {
        format!("{path}: {action}")
    } else {
        format!("{path}: {action}; {}", v.notes.join("; "))
    }
}

/// What the rules say about `p` without the model, as a sentence (empty
/// when they say nothing).
fn rules_alone(m: &Manifest, p: &Projection) -> String {
    let v = match policy::rules(m, p) {
        Pre::Done(v) => v,
        Pre::Ask { styled } => policy::decide(m, p, &styled, None, None, &Thresholds::default()),
    };
    if v.notes.is_empty() {
        String::new()
    } else {
        format!(
            " Without the model, the rules note: {}.",
            v.notes.join("; ")
        )
    }
}

/// `place_file`: one projected operation through `Decider`, logged.
fn place_file(a: &Args) -> Result<Outcome, String> {
    a.only(&["path", "summary", "content", "op", "from"])?;
    let raw = a.need("path")?;
    let abs = absolute(&raw)?;
    let op = match a.text("op")? {
        Some(o) if !o.trim().is_empty() => parse_op(&o)?,
        _ if raw.trim_end().ends_with('/') => Op::CreateDir,
        _ => Op::CreateFile,
    };
    let from_abs = match a.text("from")? {
        Some(f) if !f.trim().is_empty() => Some(absolute(&f)?),
        _ => None,
    };
    if matches!(op, Op::Move | Op::Rename) && from_abs.is_none() {
        return Err(format!(
            "op `{}` needs `from`, the current path",
            op_key(op)
        ));
    }
    let (root, m) = project(&abs)?;
    let rel = inside(&root, &abs)
        .ok_or_else(|| format!("{} is the project root itself", abs.display()))?;
    let from = from_abs
        .as_ref()
        .map(|f| inside(&root, f).unwrap_or_else(|| shown(f)));
    let summary = match (a.text("summary")?, a.text("content")?) {
        (Some(s), _) if !s.trim().is_empty() => summary_of(&s),
        (_, Some(c)) => summary_of(&c),
        _ => head_of(from_abs.as_deref().unwrap_or(&abs)).unwrap_or_default(),
    };
    let p = Projection {
        op,
        path: rel,
        from,
        summary,
        tool: None,
    };
    let client = xtf_standard::client(None);
    let base = client.base().to_string();
    let read = ask::budget(2);
    let (m2, p2) = (m.clone(), p.clone());
    let d = ask::bounded(read, move || {
        Decider::new(client.with_timeout(read)).decide(&m2, &p2)
    })
    .map(|(d, _)| d)
    .map_err(|e| format!("{}.{}", ask::describe(&e, &base), rules_alone(&m, &p)))?;
    let logged = log(&root, &p, &d);
    let gaps = layout::gaps(&m, [(&p, Some(&d.verdict))]);
    let line = format!(
        "xks-folder/1: {}{}",
        verdict_text(&p.path, &d.verdict),
        layout::line_note(&gaps)
    );
    let data = json!({
        "standard": STANDARD,
        "root": shown(&root),
        "project": m.name,
        "projected": p,
        "verdict": d.verdict,
        "answers": d.answers,
        "layout_gaps": gaps,
        "seconds": round3(d.seconds),
        "logged": logged,
    });
    Ok(Outcome::ok(&line, &data))
}

/// One planned path of `plan_tree`.
struct Planned {
    p: Projection,
    /// The name in the project's style, when the model is asked.
    styled: Option<String>,
    verdict: Option<Verdict>,
    /// The model's file.root answer.
    answer: Option<Value>,
    /// `rules` (settled without asking) or `model`.
    by: &'static str,
    seconds: f64,
}

/// `plan_tree`: many planned paths of one project, rules first, the rest in
/// batched requests on the manifest's state, each decision logged.
fn plan_tree(a: &Args) -> Result<Outcome, String> {
    a.only(&["root", "files"])?;
    let base = absolute(&a.need("root")?)?;
    let files = match a.0.get("files") {
        Some(Value::Array(f)) => f,
        _ => return Err("`files` must be a list of {path, summary}".into()),
    };
    if files.is_empty() || files.len() > MAX_PLAN {
        return Err(format!(
            "`files` holds 1 to {MAX_PLAN} entries, not {}",
            files.len()
        ));
    }
    let (root, m) = project(&base)?;
    let th = Thresholds::default();
    let mut plan: Vec<Planned> = Vec::with_capacity(files.len());
    let mut questions: Vec<(String, Value)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let at = |e: String| format!("files[{i}]: {e}");
        let f = f
            .as_object()
            .ok_or_else(|| at("must be an object {path, summary}".into()))?;
        let fa = Args(f);
        fa.only(&["path", "summary"]).map_err(at)?;
        let raw = fa.need("path").map_err(at)?;
        let abs = absolute_from(&base, &raw)?;
        let rel = inside(&root, &abs).ok_or_else(|| {
            at(format!(
                "{} is not inside the project at {}",
                abs.display(),
                root.display()
            ))
        })?;
        let op = if raw.trim_end().ends_with('/') {
            Op::CreateDir
        } else {
            Op::CreateFile
        };
        let summary = summary_of(&fa.text("summary").map_err(at)?.unwrap_or_default());
        let p = Projection {
            op,
            path: rel,
            from: None,
            summary,
            tool: None,
        };
        let mut item = Planned {
            p,
            styled: None,
            verdict: None,
            answer: None,
            by: "rules",
            seconds: 0.0,
        };
        match policy::rules(&m, &item.p) {
            Pre::Done(v) => item.verdict = Some(v),
            Pre::Ask { styled } => match catalog::root_options(&m, &item.p).as_slice() {
                // One plausible role: the name or extension rule answers.
                [only] => {
                    let mut probabilities = Map::new();
                    probabilities.insert(only.key().to_string(), json!(1.0));
                    let ruled = Answer::Choice {
                        choice: only.key().to_string(),
                        probabilities,
                        confidence: 1.0,
                    };
                    item.verdict = Some(policy::decide(
                        &m,
                        &item.p,
                        &styled,
                        Some(&ruled),
                        None,
                        &th,
                    ));
                }
                _ => {
                    questions.push((format!("q{i}"), catalog::file_root(&m, &item.p)));
                    item.styled = Some(styled);
                    item.by = "model";
                }
            },
        }
        plan.push(item);
    }
    let asked = questions.len();
    let client = xtf_standard::client(None);
    let base_url = client.base().to_string();
    let batch = if questions.is_empty() {
        ask::Batch::default()
    } else {
        ask::ask_all(&client, &m.state(), questions).map_err(|e| {
            format!(
                "{}. The rules settle {} of the {} planned paths; the other {asked} need the model, \
                 so no verdicts are returned and nothing was logged.",
                ask::describe(&e, &base_url),
                files.len() - asked,
                files.len()
            )
        })?
    };
    let mut seconds_of: HashMap<&str, f64> = HashMap::new();
    for s in &batch.requests {
        for id in &s.ids {
            seconds_of.insert(id, s.seconds);
        }
    }
    for (i, item) in plan.iter_mut().enumerate() {
        let Some(styled) = &item.styled else {
            continue;
        };
        let id = format!("q{i}");
        let raw = batch.answers.get(&id).cloned();
        let answer: Option<Answer> = raw.clone().and_then(|v| serde_json::from_value(v).ok());
        item.verdict = Some(policy::decide(
            &m,
            &item.p,
            styled,
            answer.as_ref(),
            None,
            &th,
        ));
        item.answer = raw;
        item.seconds = seconds_of.get(id.as_str()).copied().unwrap_or(0.0);
    }
    // Log, and build the result: verdicts, the resulting tree, collisions.
    let mut tree: BTreeSet<String> = BTreeSet::new();
    let mut claimed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut out = Vec::with_capacity(plan.len());
    let mut counts = [0usize; 3];
    let mut logged = json!(LOG_FILE);
    for item in &plan {
        let v = item
            .verdict
            .clone()
            .expect("every planned path has a verdict");
        counts[v.action as usize] += 1;
        let mut answers = Map::new();
        if let Some(a) = &item.answer {
            answers.insert(catalog::FILE_ROOT.into(), a.clone());
        }
        let d = Decision {
            verdict: v.clone(),
            answers,
            seconds: item.seconds,
        };
        let l = log(&root, &item.p, &d);
        if l != json!(LOG_FILE) {
            logged = l;
        }
        let path = v
            .suggested_path
            .clone()
            .unwrap_or_else(|| item.p.path.clone());
        let dir = item.p.op == Op::CreateDir;
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for k in 1..parts.len() {
            tree.insert(format!("{}/", parts[..k].join("/")));
        }
        let shown_path = if dir {
            format!("{}/", parts.join("/"))
        } else {
            parts.join("/")
        };
        tree.insert(shown_path.clone());
        claimed
            .entry(shown_path.clone())
            .or_default()
            .push(item.p.path.clone());
        let mut o = Map::new();
        o.insert("path".into(), json!(item.p.path));
        o.insert("by".into(), json!(item.by));
        if let Value::Object(vo) = json!(v) {
            o.extend(vo);
        }
        o.insert("final".into(), json!(shown_path));
        out.push(Value::Object(o));
    }
    let collisions: Vec<Value> = claimed
        .into_iter()
        .filter(|(_, from)| from.len() > 1)
        .map(|(path, from)| json!({"final": path, "planned": from}))
        .collect();
    let requests: Vec<Value> = batch
        .requests
        .iter()
        .map(|s| {
            json!({"questions": s.ids.len(), "seconds": round3(s.seconds),
                   "input_tokens": s.input_tokens, "bytes": s.bytes})
        })
        .collect();
    let total: f64 = batch.requests.iter().map(|s| s.seconds).sum();
    let mut line = format!(
        "xks-folder/1 plan of {} paths in {}: {} allow, {} suggest, {} uncertain; {} settled by \
         the rules, {asked} asked in {} request{} ({:.1} s)",
        plan.len(),
        m.name,
        counts[Action::Allow as usize],
        counts[Action::Suggest as usize],
        counts[Action::Uncertain as usize],
        plan.len() - asked,
        batch.requests.len(),
        if batch.requests.len() == 1 { "" } else { "s" },
        total
    );
    if !collisions.is_empty() {
        line.push_str(&format!("; {} final paths claimed twice", collisions.len()));
    }
    let gaps = layout::gaps(&m, plan.iter().map(|item| (&item.p, item.verdict.as_ref())));
    line.push_str(&layout::line_note(&gaps));
    let data = json!({
        "standard": STANDARD,
        "root": shown(&root),
        "project": m.name,
        "files": out,
        "tree": tree,
        "collisions": collisions,
        "layout_gaps": gaps,
        "requests": requests,
        "logged": logged,
    });
    Ok(Outcome::ok(&line, &data))
}

/// `pick_name`: name.pick on the project's manifest.
fn pick_name(a: &Args) -> Result<Outcome, String> {
    a.only(&["path_or_root", "what", "candidates"])?;
    let abs = absolute(&a.need("path_or_root")?)?;
    let what = summary_of(&a.need("what")?);
    let candidates: Vec<String> = a
        .texts("candidates")?
        .ok_or("`candidates` is required: 2 to 26 names")?
        .into_iter()
        .map(|c| c.trim().to_string())
        .collect();
    if !CANDIDATES.contains(&candidates.len()) {
        return Err(format!(
            "`candidates` holds 2 to 26 names, not {}",
            candidates.len()
        ));
    }
    if let Some(c) = candidates
        .iter()
        .find(|c| c.is_empty() || c.chars().count() > CANDIDATE_CHARS)
    {
        return Err(format!(
            "every candidate is 1 to {CANDIDATE_CHARS} characters; `{c}` is not"
        ));
    }
    let distinct: BTreeSet<&String> = candidates.iter().collect();
    if distinct.len() != candidates.len() {
        return Err("the candidates must differ from each other".into());
    }
    let (root, m) = project(&abs)?;
    let client = xtf_standard::client(None);
    let base = client.base().to_string();
    let mut questions = Map::new();
    questions.insert(
        catalog::NAME_PICK.into(),
        catalog::name_pick(&what, &candidates),
    );
    let req = Request {
        model: None,
        state: Value::String(m.state()),
        questions,
    };
    let (eval, seconds) = ask::ask(&client, req).map_err(|e| ask::describe(&e, &base))?;
    let Some(Answer::Choice {
        choice,
        probabilities,
        confidence,
    }) = eval.answer(catalog::NAME_PICK)
    else {
        return Err(format!(
            "the System One server at {base} answered name.pick with something other than a choice"
        ));
    };
    let floor = Thresholds::default().root_min_confidence;
    let acted = confidence >= floor;
    let style = m.naming.files;
    let style_notes: Vec<String> = candidates
        .iter()
        .filter(|c| naming::governs(c, &m.languages) && !naming::follows(c, style))
        .map(|c| {
            format!(
                "`{c}` does not follow the project's file naming style; `{}` does",
                naming::convert(c, style)
            )
        })
        .collect();
    let line = if acted {
        format!(
            "xks-folder/1 name.pick: `{choice}` (confidence {confidence:.2}, at or above {floor}: acted on)"
        )
    } else {
        format!(
            "xks-folder/1 name.pick: the model leans to `{choice}` but is unsure (confidence \
             {confidence:.2}, under {floor}): not acted on"
        )
    };
    let data = json!({
        "standard": STANDARD,
        "root": shown(&root),
        "project": m.name,
        "what": what,
        "candidates": candidates,
        "choice": choice,
        "confidence": confidence,
        "probabilities": probabilities,
        "acted_on": acted,
        "style_notes": style_notes,
        "seconds": round3(seconds),
    });
    Ok(Outcome::ok(&line, &data))
}

/// The manifest already in `dir`, returned unchanged.
fn existing(dir: &Path) -> Result<Outcome, String> {
    let m = Manifest::load(dir)?;
    let line = format!(
        "xks-folder/1: {} already holds {MANIFEST_FILE} (project {}, kind {}); it is returned \
         unchanged and nothing was written",
        dir.display(),
        m.name,
        m.kind.as_deref().unwrap_or("not set")
    );
    let data = json!({
        "standard": STANDARD,
        "root": shown(dir),
        "existing": true,
        "manifest": m,
        "tree": scaffold::tree(&m.layout).iter().map(|f| format!("{f}/")).collect::<Vec<_>>(),
    });
    Ok(Outcome::ok(&line, &data))
}

/// `new_project`: the kind (given, or project.kind), the layout from
/// `scaffold`, the manifest (never over an existing one), the folders.
fn new_project(a: &Args) -> Result<Outcome, String> {
    a.only(&["dir", "purpose", "languages", "name", "kind", "create_dirs"])?;
    let dir = absolute(&a.need("dir")?)?;
    let purpose = a.need("purpose")?.trim().to_string();
    let languages: Vec<String> = a
        .texts("languages")?
        .ok_or("`languages` is required: a list such as [\"rust\"], or [] for none")?
        .into_iter()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
        .collect();
    let create_dirs = a.flag("create_dirs")?.unwrap_or(false);
    let given = a
        .text("kind")?
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    if let Some(k) = &given {
        scaffold::layout(k, &languages)?;
    }
    let name = match a.text("name")? {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("`dir` has no folder name; `name` gives one")?,
    };
    if dir.join(MANIFEST_FILE).exists() {
        return existing(&dir);
    }
    let outer = Manifest::find_root(&dir);
    // The kind: given, or the model's project.kind on the project as
    // described (name, purpose, languages), acted on at 0.5 or above.
    let floor = Thresholds::default().root_min_confidence;
    let mut asked: Option<(Value, f64, f64)> = None;
    let kind: Option<String> = match given.clone() {
        Some(k) => Some(k),
        None => {
            let client = xtf_standard::client(None);
            let base = client.base().to_string();
            let state = serde_json::to_string_pretty(
                &json!({"project": name, "purpose": purpose, "languages": languages}),
            )
            .expect("state JSON");
            let mut questions = Map::new();
            questions.insert(catalog::PROJECT_KIND.into(), catalog::project_kind());
            let req = Request {
                model: None,
                state: Value::String(state),
                questions,
            };
            let (eval, seconds) = ask::ask(&client, req).map_err(|e| {
                format!(
                    "{}. The kind is the model's question; a `kind` given in the call needs no \
                     model.",
                    ask::describe(&e, &base)
                )
            })?;
            let raw = eval
                .answers
                .get(catalog::PROJECT_KIND)
                .cloned()
                .unwrap_or(Value::Null);
            let Some(Answer::Choice {
                choice, confidence, ..
            }) = eval.answer(catalog::PROJECT_KIND)
            else {
                return Err(format!(
                    "the System One server at {base} answered project.kind with something other than a choice"
                ));
            };
            asked = Some((raw, confidence, seconds));
            (confidence >= floor).then_some(choice)
        }
    };
    // An unsure kind is not acted on: the manifest leaves `kind` unset and
    // starts from the general layout (the one of kind `mixed`).
    let layout_kind = kind.clone().unwrap_or_else(|| "mixed".into());
    let mut m = Manifest::new(&name, &purpose, languages.clone());
    m.kind = kind.clone();
    m.layout = scaffold::layout(&layout_kind, &languages)?;
    if !m.ignore.iter().any(|i| i == ".xks") {
        m.ignore.push(".xks".into());
    }
    let mut created: Vec<String> = Vec::new();
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        created.push("./".into());
    }
    let text = serde_json::to_string_pretty(&m).expect("manifest JSON") + "\n";
    let path = dir.join(MANIFEST_FILE);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => f
            .write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return existing(&dir),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut tree = Vec::new();
    for f in scaffold::tree(&m.layout) {
        let p = dir.join(&f);
        let state = if p.is_dir() {
            "exists".to_string()
        } else if !create_dirs {
            "planned".to_string()
        } else {
            match std::fs::create_dir_all(&p) {
                Ok(()) => {
                    created.push(format!("{f}/"));
                    "created".to_string()
                }
                Err(e) => format!("not created: {e}"),
            }
        };
        tree.push(json!({"folder": format!("{f}/"), "state": state}));
    }
    let mut kind_json = json!({
        "kind": kind,
        "layout_from": layout_kind,
        "by": if given.is_some() { "given" } else { "model" },
    });
    let mut logged = Value::Null;
    if let Some((raw, confidence, seconds)) = &asked {
        let choice = raw.get("choice").and_then(Value::as_str).unwrap_or("");
        kind_json["confidence"] = json!(confidence);
        kind_json["probabilities"] = raw.get("probabilities").cloned().unwrap_or(Value::Null);
        kind_json["acted_on"] = json!(kind.is_some());
        kind_json["seconds"] = json!(round3(*seconds));
        let note = if kind.is_some() {
            format!("project.kind: {choice} (confidence {confidence:.2})")
        } else {
            format!(
                "project.kind: the model leans to {choice} but is unsure (confidence \
                 {confidence:.2}): kind left unset, general layout"
            )
        };
        let mut answers = Map::new();
        answers.insert(catalog::PROJECT_KIND.into(), raw.clone());
        let d = Decision {
            verdict: Verdict {
                action: if kind.is_some() {
                    Action::Allow
                } else {
                    Action::Uncertain
                },
                notes: vec![note],
                suggested_path: None,
                role: None,
                confidence: Some(*confidence),
                layout_change: None,
            },
            answers,
            seconds: *seconds,
        };
        let p = Projection {
            op: Op::CreateFile,
            path: MANIFEST_FILE.into(),
            from: None,
            summary: summary_of(&purpose),
            tool: None,
        };
        logged = log(&dir, &p, &d);
    }
    let kind_text = match (&kind, &asked) {
        (Some(k), None) => format!("kind {k} (given)"),
        (Some(k), Some((_, c, _))) => format!("kind {k} (decided by the model, confidence {c:.2})"),
        (None, Some((raw, c, _))) => format!(
            "kind not set: the model leans to {} but is unsure (confidence {c:.2}), so the \
             general layout",
            raw.get("choice").and_then(Value::as_str).unwrap_or("?")
        ),
        (None, None) => "kind not set".into(),
    };
    let folders: Vec<String> = scaffold::tree(&m.layout)
        .iter()
        .map(|f| format!("{f}/"))
        .collect();
    let made = created.iter().filter(|c| c.as_str() != "./").count();
    let mut line = format!(
        "xks-folder/1: wrote {}; {kind_text}; folders {} ({})",
        path.display(),
        folders.join(" "),
        if create_dirs {
            format!("{made} created")
        } else {
            "planned, not created".into()
        }
    );
    if let Some(o) = &outer {
        line.push_str(&format!("; it lies inside the project at {}", o.display()));
    }
    let data = json!({
        "standard": STANDARD,
        "root": shown(&dir),
        "manifest": m,
        "kind": kind_json,
        "tree": tree,
        "created": created,
        "inside_project": outer.as_deref().map(shown),
        "logged": logged,
    });
    Ok(Outcome::ok(&line, &data))
}

/// The serde name of a naming style (`kebab-case`, ...).
fn style_key(s: naming::Style) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Whether `dir` may join the layout as `role` in `m`: Ok(true) when it
/// already plays that role, Ok(false) when it is new, Err why not.
fn layout_check(m: &Manifest, role: Role, dir: &str) -> Result<bool, String> {
    let same = |x: &String| layout::plain_dir(x).is_ok_and(|x| x == dir);
    let under = |x: &String| {
        layout::plain_dir(x).is_ok_and(|x| x == dir || dir.starts_with(&format!("{x}/")))
    };
    if let Some(i) = m.ignore.iter().find(|i| under(i)) {
        return Err(format!(
            "`{dir}/` is under `{i}` in the manifest's ignore list: nothing there is decided \
             on, so it cannot play a role"
        ));
    }
    for (r, ds) in &m.layout {
        if ds.iter().any(same) {
            if *r == role {
                return Ok(true);
            }
            return Err(format!(
                "`{dir}/` is already the layout's {} folder; a folder plays one role",
                r.key()
            ));
        }
    }
    let style = m.naming.dirs;
    if let Some(c) = dir.split('/').find(|c| !naming::follows(c, style)) {
        return Err(format!(
            "`{c}` does not follow the project's folder naming style ({}); `{}` does",
            style_key(style),
            naming::convert(c, style)
        ));
    }
    Ok(false)
}

/// `extend_layout`: folders added to the layout under the role the caller
/// names (never a mapping removed or moved), the manifest rewritten whole
/// under a lock, the folders created on request, each addition logged.
fn extend_layout(a: &Args) -> Result<Outcome, String> {
    a.only(&["path_or_root", "role", "dirs", "create_dirs"])?;
    let abs = absolute(&a.need("path_or_root")?)?;
    let key = a.need("role")?.trim().to_string();
    let role = Role::from_key(&key)
        .filter(|r| *r != Role::Config)
        .ok_or_else(|| {
            format!(
                "unknown role `{key}`; the layout's roles are {} (config is the project root \
                 itself and takes no folder)",
                layout_roles().join(", ")
            )
        })?;
    let raw = a.texts("dirs")?.ok_or_else(|| {
        format!("`dirs` is required: 1 to {MAX_EXTEND} folders relative to the project root")
    })?;
    if raw.is_empty() || raw.len() > MAX_EXTEND {
        return Err(format!(
            "`dirs` holds 1 to {MAX_EXTEND} folders, not {}",
            raw.len()
        ));
    }
    let mut dirs: Vec<String> = Vec::with_capacity(raw.len());
    for d in &raw {
        let d = layout::plain_dir(d)?;
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    let create_dirs = a.flag("create_dirs")?.unwrap_or(false);
    let (root, _) = project(&abs)?;
    for d in &dirs {
        let p = root.join(d);
        if p.exists() && !p.is_dir() {
            return Err(format!("{} exists and is not a folder", p.display()));
        }
    }
    let (added, already, m) = layout::update_manifest(&root, |m| {
        let mut added = Vec::new();
        let mut already = Vec::new();
        for d in &dirs {
            if layout_check(m, role, d)? {
                already.push(d.clone());
            } else {
                added.push(d.clone());
            }
        }
        m.layout
            .entry(role)
            .or_default()
            .extend(added.iter().cloned());
        Ok((added, already, m.clone()))
    })?;
    let mut logged = json!(LOG_FILE);
    for dir in &added {
        let p = Projection {
            op: Op::CreateDir,
            path: dir.clone(),
            from: None,
            summary: format!("a {key} folder of the layout"),
            tool: Some("extend_layout".into()),
        };
        // The record is a layout change, the caller's: no model answer.
        let decision = Decision {
            verdict: Verdict {
                action: Action::Allow,
                notes: vec![format!(
                    "added to the layout as {key} (the role the caller named)"
                )],
                suggested_path: None,
                role: Some(key.clone()),
                confidence: None,
                layout_change: Some(LayoutChange {
                    role: key.clone(),
                    dir: dir.clone(),
                }),
            },
            answers: Map::new(),
            seconds: 0.0,
        };
        let l = log(&root, &p, &decision);
        if l != json!(LOG_FILE) {
            logged = l;
        }
    }
    let mut folders = Vec::with_capacity(dirs.len());
    let mut made = 0usize;
    for d in &dirs {
        let p = root.join(d);
        let state = if p.is_dir() {
            "exists".to_string()
        } else if !create_dirs {
            "planned".to_string()
        } else {
            match std::fs::create_dir_all(&p) {
                Ok(()) => {
                    made += 1;
                    "created".to_string()
                }
                Err(e) => format!("not created: {e}"),
            }
        };
        folders.push(json!({"folder": format!("{d}/"), "state": state}));
    }
    let slash = |v: &[String]| v.iter().map(|d| format!("{d}/")).collect::<Vec<_>>();
    let now = m.layout.get(&role).map(|v| slash(v)).unwrap_or_default();
    let mut what = Vec::new();
    if !added.is_empty() {
        what.push(format!("added {}", slash(&added).join(" ")));
    }
    if !already.is_empty() {
        what.push(format!("already there: {}", slash(&already).join(" ")));
    }
    if create_dirs {
        what.push(format!(
            "{made} folder{} created",
            if made == 1 { "" } else { "s" }
        ));
    }
    let line = format!(
        "xks-folder/1: layout of {}: {key} -> {} ({})",
        m.name,
        now.join(" "),
        what.join("; ")
    );
    let data = json!({
        "standard": STANDARD,
        "root": shown(&root),
        "project": m.name,
        "role": key,
        "added": slash(&added),
        "already": slash(&already),
        "layout": m.layout,
        "folders": folders,
        "logged": if added.is_empty() { Value::Null } else { logged },
    });
    Ok(Outcome::ok(&line, &data))
}

/// `project_info`: the nearest manifest and the size of its decision log.
fn project_info(a: &Args) -> Result<Outcome, String> {
    a.only(&["path"])?;
    let abs = absolute(&a.need("path")?)?;
    let Some(root) = Manifest::find_root(&abs) else {
        let line = format!(
            "xks-folder/1: no {MANIFEST_FILE} at or above {}; new_project writes one",
            abs.display()
        );
        let data =
            json!({"standard": STANDARD, "path": shown(&abs), "root": null, "manifest": null});
        return Ok(Outcome::ok(&line, &data));
    };
    let m = Manifest::load(&root)?;
    let decisions = std::fs::read(root.join(LOG_FILE))
        .map(|b| b.iter().filter(|&&c| c == b'\n').count())
        .unwrap_or(0);
    let layout: Vec<String> = m
        .layout
        .iter()
        .map(|(r, d)| format!("{} {}", r.key(), d.join(",")))
        .collect();
    let line = format!(
        "xks-folder/1: project {} at {}: kind {}, languages {}, layout {}; {decisions} \
         decision{} in {LOG_FILE}",
        m.name,
        root.display(),
        m.kind.as_deref().unwrap_or("not set"),
        if m.languages.is_empty() {
            "none".to_string()
        } else {
            m.languages.join(", ")
        },
        layout.join("; "),
        if decisions == 1 { "" } else { "s" }
    );
    let data = json!({
        "standard": STANDARD,
        "root": shown(&root),
        "name": m.name,
        "purpose": m.purpose,
        "kind": m.kind,
        "languages": m.languages,
        "layout": m.layout,
        "naming": m.naming,
        "ignore": m.ignore,
        "decisions": decisions,
        "log": LOG_FILE,
    });
    Ok(Outcome::ok(&line, &data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_by_name() {
        let base = Path::new("/w/proj");
        let r = |s: &str| absolute_from(base, s).unwrap();
        assert_eq!(r("src/a.rs"), Path::new("/w/proj/src/a.rs"));
        assert_eq!(r("./src/../docs/x.md"), Path::new("/w/proj/docs/x.md"));
        assert_eq!(r("/abs/x"), Path::new("/abs/x"));
        assert_eq!(r("../../../.."), Path::new("/"));
        assert_eq!(
            inside(base, Path::new("/w/proj/src/a.rs")).unwrap(),
            "src/a.rs"
        );
        assert_eq!(inside(base, base), None);
        assert_eq!(inside(base, Path::new("/w/other")), None);
    }

    #[test]
    fn arguments_are_checked() {
        let args = json!({"path": "x", "extra": 1});
        let a = Args(args.as_object().unwrap());
        assert!(a.only(&["path"]).unwrap_err().contains("`extra`"));
        assert_eq!(a.need("path").unwrap(), "x");
        assert!(a.need("summary").is_err());
        let bad = json!({"candidates": ["a", 3], "flag": "yes"});
        let b = Args(bad.as_object().unwrap());
        assert!(b.texts("candidates").is_err());
        assert!(b.flag("flag").is_err());
        assert!(parse_op("move").is_ok() && parse_op("copy").is_err());
        assert_eq!(op_key(Op::CreateDir), "create_dir");
    }

    #[test]
    fn layouts_are_extended_without_the_model() {
        let root = std::env::temp_dir().join(format!("xtf-extend-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        // No language: kebab-case folders, so the naming check has teeth.
        let mut m = Manifest::new(
            "t",
            "A set of notes and the scripts that build them.",
            vec![],
        );
        m.layout = [(Role::Source, vec!["src".to_string()])]
            .into_iter()
            .collect();
        m.save(&root).unwrap();
        std::fs::write(root.join("notes"), "a file, not a folder\n").unwrap();
        let r = root.to_string_lossy().into_owned();
        let ext = |args: Value| call("extend_layout", args.as_object().unwrap());
        let o = ext(
            json!({"path_or_root": r, "role": "tools", "dirs": ["./scripts/", "scripts"],
                           "create_dirs": true}),
        );
        assert!(!o.is_error, "{}", o.text);
        assert!(
            o.text.starts_with(
                "xks-folder/1: layout of t: tools -> scripts/ (added scripts/; 1 folder created)\n"
            ),
            "{}",
            o.text
        );
        assert!(root.join("scripts").is_dir());
        assert_eq!(
            Manifest::load(&root).unwrap().layout[&Role::Tools],
            ["scripts"]
        );
        // The same call again changes nothing, not even the file.
        let before = std::fs::read(root.join(MANIFEST_FILE)).unwrap();
        let o = ext(json!({"path_or_root": r, "role": "tools", "dirs": ["scripts"]}));
        assert!(
            !o.is_error && o.text.contains("already there: scripts/"),
            "{}",
            o.text
        );
        assert!(o.text.contains("\"logged\":null"), "{}", o.text);
        for (args, want) in [
            (
                json!({"path_or_root": r, "role": "docs", "dirs": ["src"]}),
                "already the layout's source folder",
            ),
            (
                json!({"path_or_root": r, "role": "config", "dirs": ["etc"]}),
                "unknown role `config`",
            ),
            (
                json!({"path_or_root": r, "role": "tool", "dirs": ["x"]}),
                "unknown role `tool`",
            ),
            (
                json!({"path_or_root": r, "role": "docs", "dirs": ["DesignNotes"]}),
                "`design-notes` does",
            ),
            (
                json!({"path_or_root": r, "role": "out", "dirs": ["target/x"]}),
                "ignore list",
            ),
            (
                json!({"path_or_root": r, "role": "docs", "dirs": ["../x"]}),
                "cannot leave the project",
            ),
            (
                json!({"path_or_root": r, "role": "docs", "dirs": []}),
                "1 to 8 folders",
            ),
            (
                json!({"path_or_root": r, "role": "docs", "dirs": ["notes"]}),
                "is not a folder",
            ),
            (
                json!({"path_or_root": r, "role": "docs", "dirs": ["a"], "x": 1}),
                "unknown argument `x`",
            ),
        ] {
            let o = ext(args);
            assert!(o.is_error && o.text.contains(want), "{want}: {}", o.text);
        }
        assert_eq!(std::fs::read(root.join(MANIFEST_FILE)).unwrap(), before);
        let log = std::fs::read_to_string(root.join(LOG_FILE)).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("\"tool\":\"extend_layout\""), "{log}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn calls_without_a_project_fail_with_a_message() {
        let o = call("place_file", json!({"path": "/"}).as_object().unwrap());
        assert!(o.is_error);
        let o = call(
            "place_file",
            json!({"path": "/nonexistent-xtf-test/a/b.rs"})
                .as_object()
                .unwrap(),
        );
        assert!(o.is_error && o.text.contains("new_project"), "{}", o.text);
        let o = call(
            "project_info",
            json!({"path": "/nonexistent-xtf-test/a"})
                .as_object()
                .unwrap(),
        );
        assert!(
            !o.is_error && o.text.contains("\"manifest\":null"),
            "{}",
            o.text
        );
        let o = call(
            "pick_name",
            json!({"path_or_root": "/", "what": "x", "candidates": ["a"]})
                .as_object()
                .unwrap(),
        );
        assert!(o.is_error && o.text.contains("2 to 26"), "{}", o.text);
        let o = call(
            "new_project",
            json!({"dir": "/nonexistent-xtf-test", "purpose": "p", "languages": [], "kind": "game"})
                .as_object()
                .unwrap(),
        );
        assert!(o.is_error && o.text.contains("cli-tool"), "{}", o.text);
    }
}
