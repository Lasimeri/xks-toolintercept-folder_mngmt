//! What the hook remembers between its runs, and where it writes its
//! errors: the folder `xtf/hook` under the user's cache (`$XDG_CACHE_HOME`,
//! else `~/.cache`). It holds one line file per session, `<session>.state`
//! (the paths denied once and the folders noted once in that session), and
//! `errors.log`. Every file is bounded: past its limit the older half is
//! dropped, by writing the newer half to a temporary file and renaming it
//! over the old one. The hook never removes a file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Largest session file (bytes) before its older half is dropped: about
/// 600 entries of 100 bytes.
pub const STATE_MAX: u64 = 64 * 1024;
/// Largest `errors.log` (bytes) before its older half is dropped.
pub const ERRORS_MAX: u64 = 256 * 1024;

/// The state folder: `$XDG_CACHE_HOME/xtf/hook`, else `$HOME/.cache/xtf/hook`.
pub fn dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))?;
    Some(base.join("xtf").join("hook"))
}

/// Append `line` (ending in a newline) to `path`, creating its folder; past
/// `max` bytes keep only the newer half, cut at a line start.
pub fn append_bounded(path: &Path, line: &str, max: u64) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let len = {
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        f.write_all(line.as_bytes())?;
        f.metadata()?.len()
    };
    if len <= max {
        return Ok(());
    }
    let text = fs::read(path)?;
    let half = text.len().saturating_sub((max / 2) as usize);
    let start = text[half..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(text.len(), |i| half + i + 1);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp.{}", std::process::id()));
    let tmp = path.with_file_name(name);
    fs::write(&tmp, &text[start..])?;
    fs::rename(&tmp, path)
}

/// One line in `errors.log`: UTC time, the command, the session, the error.
/// Failing to write it has nowhere to be reported, so it is dropped.
pub fn log_error(command: &str, session: &str, message: &str) {
    let Some(d) = dir() else { return };
    let line = format!(
        "{} {command} session={} {}\n",
        xtf_standard::decide::utc_now(),
        if session.is_empty() { "-" } else { session },
        message.replace('\n', " | ")
    );
    let _ = append_bounded(&d.join("errors.log"), &line, ERRORS_MAX);
}

/// What a session's entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A path whose first attempt was denied with a suggestion.
    Denied,
    /// A folder the projects-function note was given for.
    Noted,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Denied => "denied",
            Kind::Noted => "noted",
        }
    }
}

/// One session's entries, read once per run.
#[derive(Debug)]
pub struct Session {
    path: Option<PathBuf>,
    entries: Vec<(String, String)>,
}

/// A session id as a file name: letters, digits, `-` and `_` kept, anything
/// else `_`, at most 100 characters.
fn file_id(id: &str) -> String {
    let s: String = id
        .chars()
        .take(100)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "no-session".into() } else { s }
}

/// Backslash, tab and newline escaped, so one entry stays one line.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

impl Session {
    /// The session `id`'s entries in `dir` (none when the file is missing,
    /// unreadable, or there is no state folder).
    pub fn open(dir: Option<&Path>, id: &str) -> Session {
        let path = dir.map(|d| d.join(format!("{}.state", file_id(id))));
        let entries = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|t| {
                t.lines()
                    .filter_map(|l| l.split_once('\t'))
                    .map(|(k, v)| (k.to_string(), unescape(v)))
                    .collect()
            })
            .unwrap_or_default();
        Session { path, entries }
    }

    pub fn has(&self, kind: Kind, key: &Path) -> bool {
        let key = key.to_string_lossy();
        self.entries
            .iter()
            .any(|(k, v)| k == kind.key() && *v == key)
    }

    /// Record an entry (appended to the session's file at once).
    pub fn add(&mut self, kind: Kind, key: &Path) -> Result<(), String> {
        let value = key.to_string_lossy().into_owned();
        let path = self.path.as_ref().ok_or("no state folder (HOME unset)")?;
        let line = format!("{}\t{}\n", kind.key(), escape(&value));
        append_bounded(path, &line, STATE_MAX).map_err(|e| format!("{}: {e}", path.display()))?;
        self.entries.push((kind.key().to_string(), value));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new folder under the build directory for one test (never removed:
    /// `cargo clean` owns it).
    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/xtf-hook-tests")
            .join(format!(
                "{name}-{}-{}",
                std::process::id(),
                xtf_standard::decide::utc_now().replace(':', "")
            ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn session_entries_persist_and_escape() {
        let d = scratch("session");
        let odd = Path::new("/p/a\tb\nc\\d.md");
        let mut s = Session::open(Some(&d), "abc/../123");
        assert!(!s.has(Kind::Denied, odd));
        s.add(Kind::Denied, odd).unwrap();
        s.add(Kind::Noted, Path::new("/p")).unwrap();
        assert!(s.has(Kind::Denied, odd) && s.has(Kind::Noted, Path::new("/p")));
        assert!(!s.has(Kind::Noted, odd));
        // A second run reads what the first wrote, under a safe file name.
        let again = Session::open(Some(&d), "abc/../123");
        assert!(again.has(Kind::Denied, odd) && again.has(Kind::Noted, Path::new("/p")));
        assert!(d.join("abc____123.state").is_file());
        assert!(!Session::open(Some(&d), "other").has(Kind::Denied, odd));
        assert_eq!(file_id(""), "no-session");
        let mut none = Session::open(None, "x");
        assert!(none.add(Kind::Noted, odd).is_err());
    }

    #[test]
    fn files_stay_bounded_and_keep_the_newest_lines() {
        let d = scratch("bounded");
        let f = d.join("log");
        for i in 0..1000 {
            append_bounded(&f, &format!("line {i:04} {}\n", "x".repeat(40)), 4096).unwrap();
        }
        let text = fs::read_to_string(&f).unwrap();
        assert!(text.len() as u64 <= 4096, "{} bytes", text.len());
        assert!(text.starts_with("line ") && text.ends_with("x\n"));
        assert!(text.contains("line 0999"));
        assert!(!text.contains("line 0000"));
        // Every kept line is whole.
        assert!(text.lines().all(|l| l.len() == 50));
    }
}
