//! One decision: the rules, then (when they leave it open) one System One
//! request to a running `jevbase serve` with the manifest as its state and
//! the catalog's questions, then the policy; every decision is appended to
//! `.xks/decisions.jsonl` under the project root.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use jevbase::protocol::{Answer, Request};
use jevbase::{Client, ClientError};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::catalog;
use crate::manifest::{Manifest, STANDARD};
use crate::policy::{self, Pre, Thresholds, Verdict};
use crate::projection::{Op, Projection};

/// The decision log, relative to the project root.
pub const LOG_FILE: &str = ".xks/decisions.jsonl";

pub struct Decider {
    pub client: Client,
    pub thresholds: Thresholds,
    /// Also ask `file.name_clear` for new files (one more read each).
    pub ask_name: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub verdict: Verdict,
    /// The model's answers by question id (empty when the rules decided).
    pub answers: Map<String, Value>,
    /// Wall time of the request to the server, 0 when none was made.
    pub seconds: f64,
}

impl Decider {
    pub fn new(client: Client) -> Decider {
        Decider {
            client,
            thresholds: Thresholds::default(),
            ask_name: true,
        }
    }

    /// Decide one projected operation in the project `m` describes.
    pub fn decide(&self, m: &Manifest, p: &Projection) -> Result<Decision, ClientError> {
        let styled = match policy::rules(m, p) {
            Pre::Done(verdict) => {
                return Ok(Decision {
                    verdict,
                    answers: Map::new(),
                    seconds: 0.0,
                });
            }
            Pre::Ask { styled } => styled,
        };
        // One plausible role: the extension rule answers file.root.
        let options = catalog::root_options(m, p);
        let ruled = (options.len() == 1).then(|| {
            let key = options[0].key().to_string();
            let mut probabilities = Map::new();
            probabilities.insert(key.clone(), Value::from(1.0));
            Answer::Choice {
                choice: key,
                probabilities,
                confidence: 1.0,
            }
        });
        let mut questions = Map::new();
        if ruled.is_none() {
            questions.insert(catalog::FILE_ROOT.into(), catalog::file_root(m, p));
        }
        if self.ask_name && p.op == Op::CreateFile {
            questions.insert(catalog::FILE_NAME_CLEAR.into(), catalog::file_name_clear(p));
        }
        let (answers, seconds) = if questions.is_empty() {
            (Map::new(), 0.0)
        } else {
            let req = Request {
                model: None,
                state: Value::String(m.state()),
                questions,
            };
            let t0 = Instant::now();
            let eval = self.client.evaluate(&req)?;
            (eval.answers, t0.elapsed().as_secs_f64())
        };
        let answer = |id: &str| -> Option<Answer> {
            answers
                .get(id)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        };
        let root: Option<Answer> = ruled.or_else(|| answer(catalog::FILE_ROOT));
        let name: Option<Answer> = answer(catalog::FILE_NAME_CLEAR);
        let verdict = policy::decide(
            m,
            p,
            &styled,
            root.as_ref(),
            name.as_ref(),
            &self.thresholds,
        );
        Ok(Decision {
            verdict,
            answers,
            seconds,
        })
    }
}

/// Append one decision to the project's log (`LOG_FILE`).
pub fn log(root: &Path, p: &Projection, d: &Decision) -> std::io::Result<()> {
    let path = root.join(LOG_FILE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = serde_json::json!({
        "standard": STANDARD,
        "at": utc_now(),
        "projected": p,
        "verdict": d.verdict,
        "answers": d.answers,
        "seconds": (d.seconds * 1e3).round() / 1e3,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

/// The time now in UTC, `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates_are_civil() {
        let s = utc_now();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T');
        let year: i32 = s[..4].parse().unwrap();
        assert!(year >= 2026);
    }
}
