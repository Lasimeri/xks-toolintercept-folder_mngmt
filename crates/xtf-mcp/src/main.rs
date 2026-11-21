//! `xtf-mcp`: the xks-folder/1 standard as an MCP server for Claude Code
//! and other MCP clients, over stdio: JSON-RPC 2.0, one message per line on
//! standard input and output, one line per tool call on standard error.
//!
//! Methods: `initialize`, `ping`, `tools/list`, `tools/call`; notifications
//! (`notifications/initialized`, `notifications/cancelled`, ...) are taken
//! and never answered; any other method is answered with -32601, a line
//! that is not JSON with -32700. A batch (a JSON array, protocol 2025-03-26)
//! is answered with an array.
//!
//! Every `tools/call` runs on its own thread, so `ping` and `tools/list`
//! are answered while a decision is waiting for the model, and a tool that
//! fails (no manifest, the decision server down or slow) comes back as a
//! result marked `isError` with a plain message, never as a crash: a panic
//! in a tool is caught and reported the same way. At the end of input the
//! server finishes the calls in progress, writes their answers and exits.
//!
//! The decision server is any System One server: `TYPESAFE_BASE_URL`
//! (default http://127.0.0.1:8097, `jevbase serve`), `TYPESAFE_API_KEY`,
//! `TYPESAFE_DEFAULT_MODEL`; `XTF_MCP_TIMEOUT` (seconds) fixes the time a
//! request may take, otherwise 20 s plus 8 s per question (`ask::budget`).

mod ask;
mod layout;
mod protocol;
mod rpc;
mod tools;

use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::JoinHandle;
use std::time::Instant;

use serde_json::{Map, Value, json};

/// What to do with one message.
enum Act {
    Reply(Value),
    Call(Value, String, Map<String, Value>),
    Nothing,
}

fn handle(v: Value) -> Act {
    use rpc::Message;
    match rpc::classify(v) {
        Message::Response | Message::Notification { .. } => Act::Nothing,
        Message::Invalid { id, why } => Act::Reply(rpc::error(
            id,
            rpc::INVALID_REQUEST,
            &format!("invalid request: {why}"),
        )),
        Message::Request { id, method, params } => match method.as_str() {
            "initialize" => Act::Reply(rpc::result(id, protocol::initialize(&params))),
            "ping" => Act::Reply(rpc::result(id, json!({}))),
            "tools/list" => Act::Reply(rpc::result(id, protocol::tools_list())),
            "tools/call" => match protocol::call_params(&params) {
                Ok((name, args)) => Act::Call(id, name, args),
                Err(m) => Act::Reply(rpc::error(id, rpc::INVALID_PARAMS, &m)),
            },
            other => Act::Reply(rpc::error(
                id,
                rpc::METHOD_NOT_FOUND,
                &format!("method not found: {other}"),
            )),
        },
    }
}

/// Run one tool call; its JSON-RPC response.
fn call(id: Value, name: &str, args: &Map<String, Value>) -> Value {
    let t0 = Instant::now();
    let out = catch_unwind(AssertUnwindSafe(|| tools::call(name, args))).unwrap_or_else(|_| {
        tools::Outcome {
            text: format!(
                "xtf-mcp: internal error in {name} (reported on the server's standard error)"
            ),
            is_error: true,
        }
    });
    eprintln!(
        "xtf-mcp: {name} {:.3} s{}",
        t0.elapsed().as_secs_f64(),
        if out.is_error { " (isError)" } else { "" }
    );
    rpc::result(
        id,
        json!({"content": [{"type": "text", "text": out.text}], "isError": out.is_error}),
    )
}

/// Write one message as one line; false when the client is gone.
fn send(v: &Value) -> bool {
    let mut line = serde_json::to_string(v).expect("a JSON value serializes");
    line.push('\n');
    let mut out = std::io::stdout().lock();
    out.write_all(line.as_bytes())
        .and_then(|()| out.flush())
        .is_ok()
}

fn main() {
    let mut input = std::io::stdin().lock();
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("xtf-mcp: standard input: {e}");
                break;
            }
        }
        let line = buf.trim_ascii();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Err(e) => {
                send(&rpc::error(
                    Value::Null,
                    rpc::PARSE_ERROR,
                    &format!("parse error: {e}"),
                ));
            }
            Ok(Value::Array(items)) => {
                if items.is_empty() {
                    send(&rpc::error(
                        Value::Null,
                        rpc::INVALID_REQUEST,
                        "invalid request: an empty batch",
                    ));
                    continue;
                }
                let replies: Vec<Value> = items
                    .into_iter()
                    .filter_map(|m| match handle(m) {
                        Act::Reply(r) => Some(r),
                        Act::Call(id, name, args) => Some(call(id, &name, &args)),
                        Act::Nothing => None,
                    })
                    .collect();
                if !replies.is_empty() {
                    send(&Value::Array(replies));
                }
            }
            Ok(v) => match handle(v) {
                Act::Reply(r) => {
                    send(&r);
                }
                Act::Call(id, name, args) => workers.push(std::thread::spawn(move || {
                    send(&call(id, &name, &args));
                })),
                Act::Nothing => {}
            },
        }
        workers.retain(|w| !w.is_finished());
    }
    for w in workers {
        let _ = w.join();
    }
}
