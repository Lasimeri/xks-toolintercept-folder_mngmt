//! `xtf-hook`: standard `xks-folder/1` inside Claude Code. Claude Code runs
//! it as a command hook with one event as JSON on standard input:
//!
//! - `xtf-hook pre-tool-use` (PreToolUse, matcher `Write|Bash`): a new file
//!   (`Write`) or new folders (`mkdir` in a `Bash` command) inside a project
//!   with an `xks-project.json` are decided against the project's layout,
//!   rules first and the model only when the rules leave the question open.
//!   A confident suggestion denies the first attempt at that path once per
//!   session (mode `enforce`) or is added as context (mode `advise`). A new
//!   path outside any project gets a note, once per session and folder, on
//!   how a project's manifest is written (the projects function).
//! - `xtf-hook session-start` (SessionStart): a factual summary of the
//!   project's manifest and whether the decision server answers, without a
//!   model call.
//!
//! It never stands between the agent and its work when something fails: a
//! server that is down, slow or refusing, malformed input, an I/O error or
//! a panic ends the run with nothing on standard output, exit code 0 (fail
//! open), and one line in `errors.log` of the state folder (`state::dir`).
//! The exit code is 0 on every path: a denial travels as
//! `permissionDecision`, never as exit code 2.

mod event;
mod mkdir;
mod paths;
mod pre_tool_use;
mod session_start;
mod state;

use std::io::{Read, Write};
use std::process::ExitCode;

use event::Event;

const USAGE: &str = "\
xtf-hook: the Claude Code hook of standard xks-folder/1

usage:
  xtf-hook pre-tool-use    a PreToolUse event (Write, Bash) as JSON on standard input
  xtf-hook session-start   a SessionStart event as JSON on standard input
  xtf-hook --version

environment:
  XTF_HOOK_MODE      enforce (default): a confident suggestion denies the first
                     attempt at a path once per session; advise: suggestions are
                     context only; off: the hook does nothing
  XTF_HOOK_SKIP      colon-separated folders never decided on (default
                     /tmp:~/.claude); the session's scratchpad and every .git and
                     .xks folder are always skipped
  TYPESAFE_BASE_URL, TYPESAFE_API_KEY, TYPESAFE_DEFAULT_MODEL
                     the decision server (default http://127.0.0.1:8097)
  XDG_CACHE_HOME     the state and error log go to $XDG_CACHE_HOME/xtf/hook
                     (default ~/.cache/xtf/hook)

It prints at most one JSON object and always exits 0 (fail open).
";

/// How a suggestion is acted on (`XTF_HOOK_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The first attempt at a path the standard would put elsewhere is
    /// denied, once per session and path; the same attempt again goes
    /// through.
    Enforce,
    /// Suggestions are added as context; nothing is denied.
    Advise,
    /// The hook does nothing.
    Off,
}

impl Mode {
    /// The mode `XTF_HOOK_MODE` names: unset or empty is `enforce`; an
    /// unknown value runs as `advise` (a typo never blocks) and says why.
    pub fn parse(value: Option<&str>) -> (Mode, Option<String>) {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("" | "enforce") => (Mode::Enforce, None),
            Some("advise") => (Mode::Advise, None),
            Some("off") => (Mode::Off, None),
            Some(other) => (
                Mode::Advise,
                Some(format!(
                    "XTF_HOOK_MODE `{other}` is not enforce, advise or off: running as advise"
                )),
            ),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Enforce => "enforce",
            Mode::Advise => "advise",
            Mode::Off => "off",
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("", String::as_str);
    match command {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" => {
            println!("xtf-hook {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "pre-tool-use" | "session-start" => {}
        _ => {
            eprint!("{USAGE}");
            state::log_error(command, "", &format!("unknown command `{command}`"));
            return ExitCode::SUCCESS;
        }
    }
    let (mode, warning) = Mode::parse(std::env::var("XTF_HOOK_MODE").ok().as_deref());
    if mode == Mode::Off {
        return ExitCode::SUCCESS;
    }
    let mut errors: Vec<String> = warning.into_iter().collect();
    let mut session = String::new();
    let mut input = Vec::new();
    let output = match std::io::stdin().read_to_end(&mut input) {
        Err(e) => {
            errors.push(format!("reading standard input: {e}"));
            None
        }
        Ok(_) => match serde_json::from_slice::<Event>(&input) {
            Err(e) => {
                errors.push(format!("event JSON: {e}"));
                None
            }
            Ok(ev) => {
                session.clone_from(&ev.session_id);
                let expected = match command {
                    "pre-tool-use" => "PreToolUse",
                    _ => "SessionStart",
                };
                if !ev.hook_event_name.is_empty() && ev.hook_event_name != expected {
                    errors.push(format!(
                        "`xtf-hook {command}` got a {} event",
                        ev.hook_event_name
                    ));
                    None
                } else {
                    // A panic is one more failure to fail open on.
                    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut errs = Vec::new();
                        let out = match command {
                            "pre-tool-use" => pre_tool_use::run(&ev, mode, &mut errs),
                            _ => session_start::run(&ev, mode, &mut errs),
                        };
                        (out, errs)
                    }));
                    match run {
                        Ok((out, errs)) => {
                            errors.extend(errs);
                            out
                        }
                        Err(panic) => {
                            let what = panic
                                .downcast_ref::<&str>()
                                .map(|s| s.to_string())
                                .or_else(|| panic.downcast_ref::<String>().cloned())
                                .unwrap_or_default();
                            errors.push(format!("panic: {what}"));
                            None
                        }
                    }
                }
            }
        },
    };
    if let Some(v) = output {
        let mut out = std::io::stdout().lock();
        // Claude Code may have stopped reading (a timeout): nothing to do then.
        let _ = writeln!(out, "{v}").and_then(|_| out.flush());
    }
    for e in errors {
        state::log_error(command, &session, &e);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_and_unknown_values_never_block() {
        assert_eq!(Mode::parse(None), (Mode::Enforce, None));
        assert_eq!(Mode::parse(Some("")), (Mode::Enforce, None));
        assert_eq!(Mode::parse(Some(" Enforce ")), (Mode::Enforce, None));
        assert_eq!(Mode::parse(Some("advise")), (Mode::Advise, None));
        assert_eq!(Mode::parse(Some("OFF")), (Mode::Off, None));
        let (m, why) = Mode::parse(Some("advice"));
        assert_eq!(m, Mode::Advise);
        assert!(why.unwrap().contains("`advice`"));
        assert_eq!(Mode::Enforce.name(), "enforce");
    }
}
