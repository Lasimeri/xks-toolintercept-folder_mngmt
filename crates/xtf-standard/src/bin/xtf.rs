//! `xtf`: the xks-folder/1 standard on the command line.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jevbase::Client;
use jevbase::protocol::{Answer, Request};
use serde_json::{Map, Value, json};
use xtf_standard::{
    Decider, MANIFEST_FILE, Manifest, Op, Projection, Role, catalog, decide, naming,
};

const HELP: &str = "\
xtf: folder and naming decisions by the xks-folder/1 standard

usage: xtf [--server URL] <command> [args]

commands:
  init [DIR] --purpose TEXT [--name NAME] [--lang LANG]... [--force]
        write DIR/xks-project.json; its layout maps DIR's existing top-level
        folders to roles by their names (unknown folders are listed)
  check PATH [--op create_file|create_dir] [--summary TEXT | --content FILE]
        decide one projected operation in the project above PATH; print the
        decision as JSON and append it to .xks/decisions.jsonl
  calibrate SET.jsonl --repos DIR [--batch N]
        ask file.root for every labelled real file of SET.jsonl ({\"manifest\",
        \"root\", \"path\", \"gold\"} a line; `root` a repository folder name
        under DIR) and report accuracy and how often a confident answer would
        have moved a file that was where it belongs

The decision server is any System One server, configured as TypeSafe's SDKs
are: TYPESAFE_BASE_URL (default http://127.0.0.1:8097, `jevbase serve`; or
Intel-Phi-Jev's `xks serve`, or https://api.typesafe.ai), TYPESAFE_API_KEY,
TYPESAFE_DEFAULT_MODEL (default jev-latest). --server overrides the URL.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtf: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag` -> its values, in order.
type Flags = HashMap<String, Vec<String>>;

/// Split `--flag value` pairs (and the lone `--force`, `--help`, `-h`)
/// from positional args.
fn options(args: &[String]) -> Result<(Vec<String>, Flags), String> {
    let mut pos = Vec::new();
    let mut opt: Flags = HashMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if matches!(a.as_str(), "--force" | "--help" | "-h") {
            let key = if a == "-h" { "--help" } else { a.as_str() };
            opt.entry(key.to_string()).or_default().push(String::new());
        } else if a.starts_with("--") {
            let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
            opt.entry(a.clone()).or_default().push(v.clone());
        } else {
            pos.push(a.clone());
        }
    }
    Ok((pos, opt))
}

fn run(args: Vec<String>) -> Result<(), String> {
    let (mut pos, mut opt) = options(&args)?;
    if pos.is_empty() || opt.contains_key("--help") || pos[0] == "help" {
        print!("{HELP}");
        return Ok(());
    }
    let server = opt.remove("--server").and_then(|v| v.last().cloned());
    let client = xtf_standard::client(server.as_deref());
    let cmd = pos.remove(0);
    let one =
        |opt: &HashMap<String, Vec<String>>, k: &str| opt.get(k).and_then(|v| v.last().cloned());
    match cmd.as_str() {
        "init" => init(
            Path::new(pos.first().map_or(".", String::as_str)),
            one(&opt, "--name"),
            one(&opt, "--purpose").ok_or("init needs --purpose TEXT (one or two sentences)")?,
            opt.get("--lang").cloned().unwrap_or_default(),
            opt.contains_key("--force"),
        ),
        "check" => check(
            &client,
            Path::new(pos.first().ok_or("check needs a PATH")?),
            one(&opt, "--op"),
            one(&opt, "--summary"),
            one(&opt, "--content"),
        ),
        "calibrate" => calibrate(
            &client,
            Path::new(pos.first().ok_or("calibrate needs a SET.jsonl")?),
            Path::new(&one(&opt, "--repos").ok_or(
                "calibrate needs --repos DIR: the folder holding the repositories the set names",
            )?),
            one(&opt, "--batch").map_or(Ok(5), |b| b.parse().map_err(|_| "--batch N"))?,
        ),
        other => Err(format!("unknown command `{other}` (xtf --help)")),
    }
}

/// The role a folder name usually plays, when it is a common one.
fn known_role(dir: &str) -> Option<Role> {
    let r = match dir.to_ascii_lowercase().as_str() {
        "src" | "lib" | "crates" | "cli" | "tui" | "app" | "cmd" | "pkg" | "internal" | "csrc"
        | "backends" => Role::Source,
        "include" => Role::Include,
        "tests" | "test" | "spec" => Role::Tests,
        "benches" | "benchmarks" | "bench" => Role::Benches,
        "examples" | "example" | "samples" => Role::Examples,
        "docs" | "doc" | "reports" | "notes" => Role::Docs,
        "tools" | "scripts" => Role::Tools,
        "integrations" | "plugins" | "hooks" | "contrib" => Role::Integrations,
        "data" | "evidence" | "datasets" => Role::Data,
        "assets" | "static" | "public" | "images" => Role::Assets,
        "third_party" | "vendor" | "external" | "deps" => Role::ThirdParty,
        "out" | "build" | "dist" => Role::Out,
        _ => return None,
    };
    Some(r)
}

fn init(
    dir: &Path,
    name: Option<String>,
    purpose: String,
    languages: Vec<String>,
    force: bool,
) -> Result<(), String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    if dir.join(MANIFEST_FILE).exists() && !force {
        return Err(format!(
            "{} already has {MANIFEST_FILE} (--force replaces it)",
            dir.display()
        ));
    }
    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut m = Manifest::new(&name, &purpose, languages);
    let mut found: BTreeMap<Role, Vec<String>> = BTreeMap::new();
    let mut unknown = Vec::new();
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !m.ignore.contains(n))
        .collect();
    entries.sort();
    for d in entries {
        match known_role(&d) {
            Some(r) => found.entry(r).or_default().push(d),
            None => unknown.push(d),
        }
    }
    if !found.is_empty() {
        found.entry(Role::Out).or_insert_with(|| vec!["out".into()]);
        m.layout = found;
    }
    m.save(&dir)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&m).expect("manifest JSON")
    );
    if !unknown.is_empty() {
        eprintln!(
            "xtf: not mapped to a role (add them to `layout` by hand): {}",
            unknown.join(", ")
        );
    }
    Ok(())
}

fn check(
    client: &Client,
    path: &Path,
    op: Option<String>,
    summary: Option<String>,
    content: Option<String>,
) -> Result<(), String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let root = Manifest::find_root(&abs).ok_or_else(|| {
        format!(
            "no {MANIFEST_FILE} at or above {} (run `xtf init`)",
            abs.display()
        )
    })?;
    let m = Manifest::load(&root)?;
    let rel = abs
        .strip_prefix(&root)
        .map_err(|_| "the path is outside its project".to_string())?
        .to_string_lossy()
        .into_owned();
    let op: Op = match op.as_deref() {
        None => Op::CreateFile,
        Some(o) => serde_json::from_value(json!(o)).map_err(|_| format!("unknown --op `{o}`"))?,
    };
    let summary = match (summary, content) {
        (Some(s), _) => s,
        (None, Some(f)) => xtf_standard::projection::summary_of(
            &std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?,
        ),
        (None, None) => std::fs::read_to_string(&abs)
            .map(|c| xtf_standard::projection::summary_of(&c))
            .unwrap_or_default(),
    };
    let p = Projection {
        op,
        path: rel,
        from: None,
        summary,
        tool: None,
    };
    let d = Decider::new(client.clone())
        .decide(&m, &p)
        .map_err(|e| e.to_string())?;
    decide::log(&root, &p, &d).map_err(|e| format!("decision log: {e}"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"projected": p, "decision": d})).expect("JSON")
    );
    Ok(())
}

/// One labelled real file.
struct Item {
    manifest: PathBuf,
    root: PathBuf,
    path: String,
    gold: Role,
}

/// The labelled files of `set`; each `root` (a repository folder name) is
/// found under `repos`, each `manifest` beside the set.
fn read_set(set: &Path, repos: &Path) -> Result<Vec<Item>, String> {
    let base = set.parent().unwrap_or(Path::new("."));
    let text = std::fs::read_to_string(set).map_err(|e| format!("{}: {e}", set.display()))?;
    let mut items = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |e: String| format!("{}:{}: {e}", set.display(), n + 1);
        let v: Value = serde_json::from_str(line).map_err(|e| at(e.to_string()))?;
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .ok_or_else(|| at(format!("no `{k}`")))
        };
        let gold = Role::from_key(s("gold")?).ok_or_else(|| at("unknown gold role".into()))?;
        items.push(Item {
            manifest: base.join(s("manifest")?),
            root: repos.join(s("root")?),
            path: s("path")?.to_string(),
            gold,
        });
    }
    Ok(items)
}

fn calibrate(client: &Client, set: &Path, repos: &Path, batch: usize) -> Result<(), String> {
    // Conventional top-level files are settled by the rules before any
    // question, as in the hook: they are counted, not asked.
    let all = read_set(set, repos)?;
    let total = all.len();
    let items: Vec<Item> = all
        .into_iter()
        .filter(|it| {
            let name = it.path.rsplit('/').next().unwrap_or(&it.path);
            !(it.gold == Role::Config && !it.path.contains('/') && naming::is_conventional(name))
        })
        .collect();
    println!(
        "{total} items: {} settled by the rules (conventional top-level files), {} asked",
        total - items.len(),
        items.len()
    );
    let mut manifests: HashMap<PathBuf, Manifest> = HashMap::new();
    for it in &items {
        if !manifests.contains_key(&it.manifest) {
            let text = std::fs::read_to_string(&it.manifest)
                .map_err(|e| format!("{}: {e}", it.manifest.display()))?;
            let m: Manifest = serde_json::from_str(&text)
                .map_err(|e| format!("{}: {e}", it.manifest.display()))?;
            manifests.insert(it.manifest.clone(), m);
        }
    }
    // (gold, chosen, confidence) per item, in set order.
    let mut rows: Vec<(Role, String, f64)> = Vec::new();
    let t0 = std::time::Instant::now();
    println!("gold\tchosen\tconfidence\tpath");
    // Consecutive items of one manifest go in requests of `batch` questions
    // on the same state.
    // Items whose extension leaves one plausible role are settled by that
    // rule (confidence 1), as in the hook; the rest are asked.
    let mut ruled = 0;
    let mut i = 0;
    while i < items.len() {
        let m = &manifests[&items[i].manifest];
        let mut j = i;
        let mut questions = Map::new();
        let mut settled: HashMap<usize, String> = HashMap::new();
        while j < items.len() && j - i < batch.max(1) && items[j].manifest == items[i].manifest {
            let it = &items[j];
            let content =
                std::fs::read(it.root.join(&it.path)).map_err(|e| format!("{}: {e}", it.path))?;
            let p = Projection {
                op: Op::CreateFile,
                path: it.path.clone(),
                from: None,
                summary: xtf_standard::projection::summary_of(&String::from_utf8_lossy(&content)),
                tool: None,
            };
            match catalog::root_options(m, &p).as_slice() {
                [only] => {
                    settled.insert(j, only.key().to_string());
                }
                _ => {
                    questions.insert(format!("q{j}"), catalog::file_root(m, &p));
                }
            }
            j += 1;
        }
        let eval = if questions.is_empty() {
            None
        } else {
            let req = Request {
                model: None,
                state: Value::String(m.state()),
                questions,
            };
            Some(client.evaluate(&req).map_err(|e| e.to_string())?)
        };
        for (k, it) in items.iter().enumerate().take(j).skip(i) {
            let (chosen, conf) = match settled.remove(&k) {
                Some(role) => {
                    ruled += 1;
                    (role, 1.0)
                }
                None => match eval.as_ref().and_then(|e| e.answer(&format!("q{k}"))) {
                    Some(Answer::Choice {
                        choice, confidence, ..
                    }) => (choice, confidence),
                    other => return Err(format!("q{k}: unexpected answer {other:?}")),
                },
            };
            println!("{}\t{chosen}\t{conf:.4}\t{}", it.gold.key(), it.path);
            rows.push((it.gold, chosen, conf));
        }
        i = j;
    }
    println!("\n{ruled} settled by the name or extension rules (one role left)");
    let secs = t0.elapsed().as_secs_f64();
    let n = rows.len();
    let right = rows.iter().filter(|(g, c, _)| g.key() == c).count();
    println!(
        "\nfile.root: {right} of {n} right ({:.1}%), {:.1} s, {:.2} decisions/s",
        100.0 * right as f64 / n as f64,
        secs,
        n as f64 / secs
    );
    let mut per: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (g, c, _) in &rows {
        let e = per.entry(g.key()).or_default();
        e.1 += 1;
        if g.key() == c {
            e.0 += 1;
        }
    }
    for (g, (r, t)) in &per {
        println!("  gold {g:<13} {r:>3} of {t:>3}");
    }
    let mut confusion: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (g, c, _) in rows.iter().filter(|(g, c, _)| g.key() != c) {
        *confusion
            .entry((g.key().to_string(), c.clone()))
            .or_default() += 1;
    }
    for ((g, c), k) in &confusion {
        println!("  confused {g} -> {c}: {k}");
    }
    println!("\nthreshold  answered  right-of-answered  would-move-a-right-file");
    for t in [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7] {
        let sure: Vec<_> = rows.iter().filter(|r| r.2 >= t).collect();
        let ok = sure.iter().filter(|(g, c, _)| g.key() == c).count();
        println!(
            "  >= {t:.1}   {:>4} {:>5.1}%   {:>5.1}%            {:>3} ({:.1}% of all)",
            sure.len(),
            100.0 * sure.len() as f64 / n as f64,
            if sure.is_empty() {
                0.0
            } else {
                100.0 * ok as f64 / sure.len() as f64
            },
            sure.len() - ok,
            100.0 * (sure.len() - ok) as f64 / n as f64
        );
    }
    Ok(())
}
