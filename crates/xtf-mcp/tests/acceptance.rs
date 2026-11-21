//! Acceptance scenarios: the flow of setting up a project and placing its
//! files, through the real `xtf-mcp` binary over stdio, the way a coding
//! agent drives it. The scenarios are data (`tests/acceptance/*.json` at the
//! repository root, described in its README); this file runs them.
//!
//! Each scenario gets a fresh folder (the server's working directory, so
//! relative paths in its steps resolve there) and one server process, like
//! one Claude Code session. A step is one `tools/call`; its checks are on the
//! contract (an error or not, the summary line, JSON fields, files on disk).
//! Model answers are recorded, not asserted: they belong to the standard's
//! question wording and the model, which change between versions.
//!
//! Needs a System One server (`jevbase serve`, `TYPESAFE_BASE_URL`), so it is
//! ignored by default:
//!
//! ```sh
//! cargo test -p xtf-mcp --test acceptance -- --ignored --nocapture
//! ```
//!
//! `XTF_ACCEPTANCE_LOCK=FILE` takes an exclusive flock(2) on FILE around each
//! scenario, for machines where other sessions share the one server.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use serde_json::{Value, json};

mod sys {
    unsafe extern "C" {
        /// flock(2): `fd` is only read as a number; a bad one is EBADF.
        pub safe fn flock(fd: i32, operation: i32) -> i32;
    }
}

/// flock(2) LOCK_EX.
const LOCK_EX: i32 = 2;

/// One server process and its pipes.
struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start(dir: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtf-mcp"))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start xtf-mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut s = Server {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let init = s.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "acceptance", "version": "0"}}),
        );
        assert!(init.get("result").is_some(), "initialize: {init}");
        s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        s
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").expect("write to xtf-mcp");
        self.stdin.flush().expect("flush");
    }

    /// Send a request and read lines until its response.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line).expect("read from xtf-mcp");
            assert!(n > 0, "xtf-mcp closed its output before answering {method}");
            let v: Value = serde_json::from_str(line.trim()).expect("a JSON-RPC line");
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // End of input ends the server (it finishes calls in progress).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The text of a tool result split into its summary line and its JSON.
fn split(text: &str) -> (String, Value) {
    match text.split_once('\n') {
        Some((line, rest)) => (
            line.to_string(),
            serde_json::from_str(rest).unwrap_or(Value::Null),
        ),
        None => (text.to_string(), Value::Null),
    }
}

/// The checks of one step against its result; the failures, one per line.
fn check(expect: &Value, is_error: bool, line: &str, data: &Value, dir: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    // No let chains: they need Rust 1.88 and the workspace says 1.85.
    match expect.get("error").and_then(Value::as_bool) {
        Some(e) if e != is_error => bad.push(format!("error: want {e}, got {is_error}: {line}")),
        _ => {}
    }
    for s in expect
        .get("line_has")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !line.contains(s) {
            bad.push(format!("line lacks `{s}`: {line}"));
        }
    }
    if let Some(eq) = expect.get("eq").and_then(Value::as_object) {
        for (ptr, want) in eq {
            let got = data.pointer(ptr).cloned().unwrap_or(Value::Null);
            if &got != want {
                bad.push(format!("{ptr}: want {want}, got {got}"));
            }
        }
    }
    for ptr in expect
        .get("present")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if data.pointer(ptr).is_none_or(Value::is_null) {
            bad.push(format!("{ptr}: missing"));
        }
    }
    if let Some(pl) = expect.get("pluck").and_then(Value::as_object) {
        for (ptr, spec) in pl {
            let field = spec.get("field").and_then(Value::as_str).unwrap_or("");
            let got: Vec<Value> = data
                .pointer(ptr)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| x.get(field).cloned().unwrap_or(Value::Null))
                        .collect()
                })
                .unwrap_or_default();
            let want = spec.get("eq").cloned().unwrap_or(Value::Null);
            if json!(got) != want {
                bad.push(format!("{ptr}[].{field}: want {want}, got {}", json!(got)));
            }
        }
    }
    for (key, want_dir) in [("dirs", true), ("files", false)] {
        for p in expect
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let at = dir.join(p);
            let ok = if want_dir { at.is_dir() } else { at.is_file() };
            if !ok {
                bad.push(format!(
                    "{p}: want a {} on disk",
                    if want_dir { "folder" } else { "file" }
                ));
            }
        }
    }
    bad
}

/// Run one scenario file; the number of failed steps.
fn run(file: &Path, base: &Path) -> usize {
    let sc: Value = serde_json::from_str(&std::fs::read_to_string(file).expect("read scenario"))
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let name = sc["name"].as_str().unwrap_or("?");
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).expect("scenario folder");
    let _lock = std::env::var_os("XTF_ACCEPTANCE_LOCK").map(|p| {
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .expect("open XTF_ACCEPTANCE_LOCK");
        assert_eq!(
            sys::flock(f.as_raw_fd(), LOCK_EX),
            0,
            "flock XTF_ACCEPTANCE_LOCK"
        );
        f
    });
    println!("== {name}: {}", sc["about"].as_str().unwrap_or(""));
    let mut server = Server::start(&dir);
    let mut failed = 0;
    for (i, step) in sc["steps"].as_array().expect("steps").iter().enumerate() {
        let tool = step["tool"].as_str().expect("tool");
        let t0 = Instant::now();
        let r = server.request(
            "tools/call",
            json!({"name": tool, "arguments": step["args"]}),
        );
        let ms = t0.elapsed().as_millis();
        let result = r
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("{tool}: {r}"));
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        let (line, data) = split(text);
        let bad = check(&step["expect"], is_error, &line, &data, &dir);
        println!(
            "  {} {:>2} {tool} {ms} ms: {line}",
            if bad.is_empty() { "PASS" } else { "FAIL" },
            i + 1
        );
        for b in &bad {
            println!("       {b}");
        }
        for ptr in step["record"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            println!(
                "       record {ptr} = {}",
                data.pointer(ptr).cloned().unwrap_or(Value::Null)
            );
        }
        failed += usize::from(!bad.is_empty());
    }
    failed
}

#[test]
#[ignore = "needs a System One server; cargo test -p xtf-mcp --test acceptance -- --ignored"]
fn acceptance_scenarios() {
    let set = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/acceptance");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&set)
        .expect("tests/acceptance")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no scenarios in {}", set.display());
    let base = std::env::temp_dir().join(format!("xtf-acceptance-{}", std::process::id()));
    let mut failed = 0;
    for f in &files {
        failed += run(f, &base);
    }
    println!(
        "{} scenarios, {failed} failed steps; folders in {}",
        files.len(),
        base.display()
    );
    assert_eq!(
        failed, 0,
        "{failed} acceptance steps failed (see the output)"
    );
    std::fs::remove_dir_all(&base).ok();
}
