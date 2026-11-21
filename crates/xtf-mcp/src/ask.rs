//! Asking the System One server, bounded in time, and batching many
//! questions on one state into as few requests as the limits allow.
//!
//! What jevbase's `Client` does on its own (client.rs, v0.1.0-rc.1): a
//! refused connection fails at once (`Unreachable`); an address that does
//! not answer fails after its connect timeout, fixed at 2 s; a server that
//! accepts and then says nothing is waited for up to the read timeout (300 s
//! by default, and per `read`, so a server that trickles bytes is waited for
//! longer); a 429 is retried up to three times after waits of up to 60 s
//! each. `jevbase serve` answers one connection at a time, so a request also
//! waits behind other sessions' requests. A tool call must come back while
//! an agent waits for it, so every request here gets a read timeout sized to
//! its questions (`budget`) and runs on its own thread behind a hard
//! deadline a little past it (`bounded`): whatever the server does, refuse,
//! stall, trickle or rate-limit, the call returns by then.
//!
//! The read timeout alone is not exact: Linux runs a socket timeout on its
//! timer wheel, whose granularity grows with the timeout (at HZ=1000: 64 ms
//! up to 4 s, 512 ms up to 32 s, 4,096 ms up to 4.4 min), so a 36 s read
//! timeout was measured ending after 36.85 s, 37.9 s, and later than 39 s.
//! The hard deadline is a futex wait on a high-resolution timer: measured
//! at 39,000.3 ms for a 39 s deadline.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use jevbase::protocol::Request;
use jevbase::{Client, ClientError, Evaluation};
use serde_json::{Map, Value};

/// Seconds allowed before this request's first question is read: requests
/// of other sessions queued ahead of it on the one-at-a-time server.
const QUEUE_WAIT: f64 = 20.0;
/// Seconds allowed per question (a question measured 3.2 s on 8 threads,
/// SPEC.md "Calibration"; the server runs at a lower CPU weight than
/// interactive work, so it slows when the machine is busy).
const PER_QUESTION: f64 = 8.0;
/// Past the read timeout, before the hard deadline: jevbase's fixed 2 s
/// connect timeout plus 1 s.
const GRACE: f64 = 3.0;

/// Most questions put into one request, even when the token limits would
/// allow more. `jevbase serve` answers one request at a time, in the order
/// they connect, and every session's hook gives up on a decision after 15 s
/// (xtf-hook's `MODEL_BUDGET`). Measured with 32: a 32-question request held
/// the server 41.5 s (1.30 s a question), and the two hook requests queued
/// behind it were answered after their clients had gone. With 8 a request
/// measured 14.1 s and 14.8 s in a real plan_tree (8 questions, 2,908 input
/// tokens, 2026-11-21), which leaves a hook queued behind it no time inside
/// its 15 s; with 4 a request holds the server about half that, so a hook
/// queued behind one still gets its own decision within the budget.
pub const MAX_QUESTIONS: usize = 4;
/// Token budget of one request: TypeSafe's documented per-request context
/// (64,000 tokens, jevbase `Limits::DEFAULT`) with a quarter kept as margin
/// for the estimate (`state_tokens`, `question_tokens`). A request the
/// server still finds too long is split in two and sent again.
const REQUEST_TOKENS: usize = 48_000;

/// Most question bytes (each question's JSON and id) put into one request:
/// what holds the one shared server is a request's size, not its question
/// count. Measured on `jevbase serve` (Qwen3.8 2B Q8_0, 8 threads,
/// 2026-11-21): requests of 750 to 2,908 server input tokens took 3.6 to
/// 14.1 s, about 4.9 ms a token. The same 12-path plan_tree: 4 questions a
/// request, longest 10.554 s; 1,800 question bytes, longest 7.82 s; 1,400,
/// longest 4.844 s in 4 requests (16.98 s in all, no slower), the same
/// choices. So a hook queued behind one keeps about 10 s of its 15 s. A
/// question longer than this alone still goes, alone.
pub const REQUEST_BYTES: usize = 1_400;

/// Why no evaluation came back.
#[derive(Debug, Clone, PartialEq)]
pub enum AskError {
    /// The client's own error: refused, rejected, failed.
    Server(ClientError),
    /// No answer by the hard deadline (seconds).
    Deadline(f64),
}

/// The read timeout of a request of `questions` questions:
/// `XTF_MCP_TIMEOUT` (seconds) when set, else `QUEUE_WAIT + PER_QUESTION *
/// questions`.
pub fn budget(questions: usize) -> Duration {
    let fixed = std::env::var("XTF_MCP_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0);
    Duration::from_secs_f64(fixed.unwrap_or(QUEUE_WAIT + PER_QUESTION * questions as f64))
}

/// Run `f` (calls through a `Client` whose read timeout is `read`) on its
/// own thread and give up `GRACE` past `read`. Ok: its value and the wall
/// time in seconds. A failure that comes at or after `read` is the read
/// timeout (the client reports it as a failed read) and is told as a
/// `Deadline`. A call given up on keeps its thread until the client's own
/// timeouts end it; its result is dropped.
pub fn bounded<T: Send + 'static>(
    read: Duration,
    f: impl FnOnce() -> Result<T, ClientError> + Send + 'static,
) -> Result<(T, f64), AskError> {
    let hard = read + Duration::from_secs_f64(GRACE);
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(hard) {
        Ok(Ok(v)) => Ok((v, t0.elapsed().as_secs_f64())),
        Ok(Err(ClientError::Failed(_))) if t0.elapsed() >= read => {
            Err(AskError::Deadline(t0.elapsed().as_secs_f64()))
        }
        Ok(Err(e)) => Err(AskError::Server(e)),
        Err(_) => Err(AskError::Deadline(hard.as_secs_f64())),
    }
}

/// One request through `bounded`, its read timeout from `budget`.
pub fn ask(client: &Client, req: Request) -> Result<(Evaluation, f64), AskError> {
    let read = budget(req.questions.len());
    let client = client.clone().with_timeout(read);
    bounded(read, move || client.evaluate(&req))
}

/// `e` as a sentence about the server at `base`.
pub fn describe(e: &AskError, base: &str) -> String {
    match e {
        AskError::Server(ClientError::Unreachable(m)) => format!(
            "no System One server answers at {base} ({m}); the decisions come from it \
             (by default `jevbase serve`, the jevbase.service user unit)"
        ),
        AskError::Server(ClientError::Rejected(code, m)) => {
            format!("the System One server at {base} refused the request (HTTP {code}): {m}")
        }
        AskError::Server(ClientError::Failed(m)) => {
            format!("the System One server at {base} failed: {m}")
        }
        AskError::Deadline(s) => format!(
            "the System One server at {base} gave no answer within {s:.0} s \
             (busy with other sessions' requests, or stalled)"
        ),
    }
}

/// Estimated tokens of the state, `bytes` long: it is read once, and Qwen's
/// tokenizer reads English and JSON at about 4 bytes a token, so half its
/// bytes keeps the estimate above the count.
pub fn state_tokens(bytes: usize) -> usize {
    bytes / 2 + 16
}

/// Estimated tokens of a question whose JSON is `bytes` long. The server
/// reads a Choice in several rotations of its options (jevbase's
/// `permutations`, 3 by default), each with the whole question: measured on
/// `jevbase serve`, 9 file.root questions in a 4,745-byte request counted
/// 3,003 input tokens with the state cached: 0.63 a byte of the whole body.
/// One token a byte, plus 64, keeps the estimate above that.
pub fn question_tokens(bytes: usize) -> usize {
    bytes + 64
}

/// What a batch came back with.
#[derive(Debug, Default)]
pub struct Batch {
    /// Answer by question id, every question of every request.
    pub answers: Map<String, Value>,
    /// Per request: its question ids, wall seconds, input tokens the
    /// server counted (cached state excluded) and body bytes.
    pub requests: Vec<Sent>,
}

#[derive(Debug, Clone)]
pub struct Sent {
    pub ids: Vec<String>,
    pub seconds: f64,
    pub input_tokens: u64,
    pub bytes: usize,
}

/// The questions in requests on `state`: in order, at most `MAX_QUESTIONS`,
/// `REQUEST_BYTES` of questions and `REQUEST_TOKENS` (estimated) each; a
/// request always takes at least one question.
pub fn chunks(state: &str, questions: Vec<(String, Value)>) -> Vec<Vec<(String, Value)>> {
    let base = state_tokens(state.len());
    let mut out: Vec<Vec<(String, Value)>> = Vec::new();
    let mut cur: Vec<(String, Value)> = Vec::new();
    let mut used = base;
    let mut bytes = 0;
    for (id, q) in questions {
        let b = q.to_string().len() + id.len();
        let t = question_tokens(b);
        if !cur.is_empty()
            && (cur.len() == MAX_QUESTIONS
                || used + t > REQUEST_TOKENS
                || bytes + b > REQUEST_BYTES)
        {
            out.push(std::mem::take(&mut cur));
            used = base;
            bytes = 0;
        }
        used += t;
        bytes += b;
        cur.push((id, q));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Ask every question on `state`, in `chunks`; a request the server finds
/// past its per-request context is split in two and sent again. The first
/// failure ends the batch.
pub fn ask_all(
    client: &Client,
    state: &str,
    questions: Vec<(String, Value)>,
) -> Result<Batch, AskError> {
    let mut batch = Batch::default();
    let mut todo: Vec<Vec<(String, Value)>> = chunks(state, questions);
    todo.reverse();
    while let Some(chunk) = todo.pop() {
        let ids: Vec<String> = chunk.iter().map(|(id, _)| id.clone()).collect();
        let req = Request {
            model: None,
            state: Value::String(state.to_string()),
            questions: chunk.iter().cloned().collect(),
        };
        let bytes = serde_json::to_vec(&req).map_or(0, |b| b.len());
        match ask(client, req) {
            Ok((eval, seconds)) => {
                batch.answers.extend(eval.answers);
                batch.requests.push(Sent {
                    ids,
                    seconds,
                    input_tokens: eval.usage.input_tokens,
                    bytes,
                });
            }
            Err(AskError::Server(ClientError::Rejected(400, m)))
                if chunk.len() > 1 && m.contains("per-request context") =>
            {
                let mut first = chunk;
                let second = first.split_off(first.len() / 2);
                todo.push(second);
                todo.push(first);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn chunks_respect_the_question_cap_and_the_token_budget() {
        let q = |n: usize| {
            (
                format!("q{n}"),
                json!({"type": "noul", "instructions": "x"}),
            )
        };
        let small: Vec<_> = (0..70).map(q).collect();
        let sizes: Vec<usize> = chunks("state", small).iter().map(Vec::len).collect();
        // Full requests of MAX_QUESTIONS, then the rest.
        let mut want = vec![MAX_QUESTIONS; 70 / MAX_QUESTIONS];
        if 70 % MAX_QUESTIONS != 0 {
            want.push(70 % MAX_QUESTIONS);
        }
        assert_eq!(sizes, want);
        // Questions of about 20,000 bytes: each past REQUEST_BYTES alone, so
        // each goes in a request of its own (never dropped).
        let big = |n: usize| {
            (
                format!("b{n}"),
                json!({"type": "noul", "instructions": "y".repeat(20_000)}),
            )
        };
        let sizes: Vec<usize> = chunks("s", (0..12).map(big).collect())
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(sizes, [1; 12]);
        // Questions of about 600 bytes: two fit REQUEST_BYTES (1,400), the
        // third starts a request, though MAX_QUESTIONS would allow four.
        let mid = |n: usize| {
            let q = json!({"type": "noul", "instructions": "z".repeat(560)});
            let id = format!("m{n}");
            let pad = 600 - (q.to_string().len() + id.len());
            (
                id,
                json!({"type": "noul", "instructions": "z".repeat(560 + pad)}),
            )
        };
        let sizes: Vec<usize> = chunks("s", (0..7).map(mid).collect())
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(sizes, [2, 2, 2, 1]);
        assert!(chunks("s", Vec::new()).is_empty());
    }

    #[test]
    fn a_refused_connection_fails_at_once() {
        let t0 = Instant::now();
        let e = bounded(Duration::from_secs(5), || {
            Client::new("127.0.0.1:9").health()
        })
        .unwrap_err();
        assert!(matches!(e, AskError::Server(ClientError::Unreachable(_))));
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        assert!(describe(&e, "http://127.0.0.1:9").starts_with("no System One server"));
    }

    #[test]
    fn a_read_timeout_is_told_as_a_deadline() {
        // Listens and never accepts: the connection waits in the backlog and
        // the client's read times out (it reports a failed read).
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let read = Duration::from_millis(300);
        let client = Client::new(addr).with_timeout(read);
        let t0 = Instant::now();
        let e = bounded(read, move || client.health()).unwrap_err();
        let waited = t0.elapsed();
        drop(l);
        assert!(matches!(e, AskError::Deadline(_)), "{e:?}");
        assert!(
            waited >= read && waited < Duration::from_millis(1_500),
            "{waited:?}"
        );
        assert!(describe(&e, "http://x").contains("gave no answer within"));
    }

    #[test]
    fn a_server_that_never_answers_is_given_up_on() {
        // Accepts, reads, never replies, and trickles nothing: only the
        // deadline ends the wait (the client's read timeout here is 60 s).
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            while s.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
        });
        let client = Client::new(addr).with_timeout(Duration::from_secs(60));
        let t0 = Instant::now();
        let e = bounded(Duration::from_millis(200), move || client.health()).unwrap_err();
        let waited = t0.elapsed();
        assert!(matches!(e, AskError::Deadline(_)));
        assert!(
            waited >= Duration::from_millis(3_200) && waited < Duration::from_millis(4_200),
            "{waited:?}"
        );
    }
}
