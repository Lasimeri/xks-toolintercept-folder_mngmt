//! SessionStart: what Claude should know about the working directory's
//! project before the first prompt, with no model call. With an
//! `xks-project.json` at or above `cwd`: a factual summary of the manifest
//! (standard, name, kind, purpose, role -> folders, naming styles, what is
//! never decided on) and of how the hook and the MCP tools use it. Without
//! one, inside a project (a marker such as `.git` or `Cargo.toml` at or
//! above `cwd`, the home folder excepted): a note that it has no manifest
//! and how one is written. Elsewhere: nothing. Whenever it says something,
//! it also says whether the decision server answers `GET /health` within
//! `HEALTH_BUDGET`.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use jevbase::ClientError;
use serde_json::Value;
use xtf_standard::{MANIFEST_FILE, Manifest, STANDARD};

use crate::Mode;
use crate::event::{self, Event};
use crate::paths;
use crate::pre_tool_use::MODEL_BUDGET;

/// How long the server's `GET /health` may take. It answers in under a
/// millisecond when idle, but it serves one request at a time, so a
/// decision for another session delays it (measured 2026-11-21: 698 ms
/// behind a 706 ms decision).
pub const HEALTH_BUDGET: Duration = Duration::from_millis(300);

/// The longest purpose quoted, in characters.
const PURPOSE_CHARS: usize = 400;

pub fn run(ev: &Event, mode: Mode, errors: &mut Vec<String>) -> Option<Value> {
    let cwd = Path::new(&ev.cwd);
    if !cwd.is_absolute() {
        errors.push(format!(
            "SessionStart with a cwd that is not absolute: `{}`",
            ev.cwd
        ));
        return None;
    }
    let cwd = paths::normalize(cwd);
    let mut lines = Vec::new();
    if let Some(root) = Manifest::find_root(&cwd) {
        match Manifest::load(&root) {
            Ok(m) => lines.push(summary(&m, &root, mode)),
            Err(e) => {
                lines.push(format!(
                    "{STANDARD}: the {MANIFEST_FILE} at `{}` could not be read ({e}); the xtf \
                     hook decides nothing in this project until it parses.",
                    root.display()
                ));
                errors.push(e);
            }
        }
    } else if let Some((dir, markers)) = paths::project_above(&cwd, paths::home().as_deref()) {
        lines.push(format!(
            "{STANDARD}: `{}` has no {MANIFEST_FILE} (it holds {}), so the xtf hook does not \
             decide new files and folders in it. `xtf init` writes one for an existing project; \
             the xtf MCP tool new_project lays out a new project's folders and writes one.",
            dir.display(),
            markers.join(", ")
        ));
    } else {
        return None;
    }
    lines.push(server_status());
    Some(event::context("SessionStart", &lines.join("\n")))
}

fn summary(m: &Manifest, root: &Path, mode: Mode) -> String {
    let kind = m
        .kind
        .as_deref()
        .map_or_else(|| "kind not set".to_string(), |k| format!("kind {k}"));
    let mut purpose: String = m.purpose.chars().take(PURPOSE_CHARS).collect();
    if m.purpose.chars().count() > PURPOSE_CHARS {
        purpose.push_str(" (cut)");
    }
    let layout = m
        .layout
        .iter()
        .map(|(r, dirs)| format!("{}: {}", r.key(), dirs.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    let style = |s: xtf_standard::naming::Style| {
        serde_json::to_value(s)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    };
    let languages = if m.languages.is_empty() {
        "no languages given".to_string()
    } else {
        m.languages.join(", ")
    };
    let ignore = if m.ignore.is_empty() {
        String::new()
    } else {
        format!("{}, ", m.ignore.join(", "))
    };
    let acting = match mode {
        Mode::Enforce => {
            "a confident suggestion denies the first attempt at a path once per session, and \
             the same attempt again goes through"
        }
        _ => "a confident suggestion is added as context and nothing is denied",
    };
    format!(
        "{STANDARD} project `{}` ({kind}; {languages}) at `{}`: {purpose}\n\
         Layout, role: folders: {layout}; files directly in the root play the config role.\n\
         Naming: source files {}, new folders {}. Never decided on: {ignore}the out and \
         third_party folders.\n\
         The xtf hook (mode {}) decides new files (Write) and new folders (mkdir in Bash) \
         against this layout, rules first and then the decision server's model; {acting}. The \
         xtf MCP tools place_file, plan_tree, pick_name and new_project answer placement, layout \
         and naming questions against the same standard.",
        m.name,
        root.display(),
        style(m.naming.files),
        style(m.naming.dirs),
        mode.name()
    )
}

/// Whether the decision server answers `GET /health`, in one sentence. A
/// worker thread asks; the answer is waited for `HEALTH_BUDGET` at most
/// (the client's own connect timeout is 2 s, too long here).
fn server_status() -> String {
    let client = xtf_standard::client(None).with_timeout(HEALTH_BUDGET);
    let base = client.base().to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(client.health());
    });
    let ms = HEALTH_BUDGET.as_millis();
    let budget = MODEL_BUDGET.as_secs();
    match rx.recv_timeout(HEALTH_BUDGET) {
        Ok(Ok(v)) => format!("The decision server at {base} answers (GET /health: {v})."),
        Ok(Err(ClientError::Rejected(404, _))) => {
            format!("The decision server at {base} answers; it has no /health endpoint.")
        }
        Ok(Err(ClientError::Unreachable(why))) => format!(
            "The decision server at {base} is not reachable ({why}): folder decisions are \
             unavailable, and the xtf hook lets every new file and folder through undecided \
             (fail open)."
        ),
        Ok(Err(e)) => format!(
            "The decision server at {base} answered GET /health with an error ({e}): folder \
             decisions may fail, and the xtf hook lets what it cannot decide through (fail open)."
        ),
        Err(_) => format!(
            "The decision server at {base} did not answer GET /health within {ms} ms (it serves \
             one request at a time, so a decision for another session delays it): the xtf hook \
             lets a new file or folder it cannot decide within {budget} s through undecided."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use xtf_standard::Role;
    use xtf_standard::naming::{Naming, Style};

    #[test]
    fn the_summary_states_the_manifest_and_the_mode() {
        let mut layout = BTreeMap::new();
        layout.insert(Role::Source, vec!["src".to_string(), "cli".to_string()]);
        layout.insert(Role::Out, vec!["out".to_string()]);
        let m = Manifest {
            standard: STANDARD.into(),
            name: "demo".into(),
            purpose: "p".repeat(500),
            languages: vec!["rust".into()],
            kind: Some("cli-tool".into()),
            layout,
            naming: Naming {
                files: Style::Snake,
                dirs: Style::Kebab,
            },
            ignore: vec![".git".into()],
        };
        let s = summary(&m, Path::new("/r"), Mode::Enforce);
        assert!(s.starts_with("xks-folder/1 project `demo` (kind cli-tool; rust) at `/r`"));
        assert!(s.contains("source: src, cli; out: out;"), "{s}");
        assert!(
            s.contains("source files snake_case, new folders kebab-case"),
            "{s}"
        );
        assert!(s.contains("Never decided on: .git, the out"), "{s}");
        assert!(s.contains("(mode enforce)") && s.contains("denies the first attempt"));
        assert!(s.contains(" (cut)\n"));
        let a = summary(&m, Path::new("/r"), Mode::Advise);
        assert!(a.contains("(mode advise)") && a.contains("nothing is denied"));
    }
}
