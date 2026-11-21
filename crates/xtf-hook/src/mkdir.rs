//! The folders a shell command creates with `mkdir`, read from the
//! command's text without running anything. Best effort, and silent about
//! what it cannot read: the command is split into words as the shell would
//! (quotes, escapes, comments, here-document bodies and redirections
//! understood), cut into simple commands at `;`, `&&`, `||`, `|`, `&`,
//! newlines and parentheses, and each `mkdir` gives its operands, resolved
//! against the working directory (which a literal `cd` changes for the
//! commands after it). Operands holding an expansion (`$`, backquotes), a
//! glob, a `~user` or a brace sequence (`{1..3}`) are skipped; brace lists
//! (`src/{a,b}`) are expanded.

use std::path::{Path, PathBuf};

use crate::paths::normalize;

/// One `mkdir` operand, resolved to an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// `-p` / `--parents`: missing parent folders are created too.
    pub parents: bool,
}

/// Most paths one operand's brace lists may expand to before it is skipped.
const MAX_EXPANSION: usize = 64;

/// A word of the command: its characters, each marked literal (quoted or
/// escaped) or not, and whether it holds an expansion whose value is only
/// known when the command runs.
#[derive(Debug, Default, Clone)]
struct Word {
    chars: Vec<(char, bool)>,
    dynamic: bool,
}

impl Word {
    /// The text as the program receives it, when the shell would pass it on
    /// unchanged: no expansion, no unquoted glob, brace or tilde.
    fn literal(&self) -> Option<String> {
        let special =
            |&(c, quoted): &(char, bool)| !quoted && matches!(c, '*' | '?' | '[' | '{' | '}' | '~');
        (!self.dynamic && !self.chars.iter().any(special)).then(|| self.text())
    }

    /// The characters, quoting removed.
    fn text(&self) -> String {
        self.chars.iter().map(|&(c, _)| c).collect()
    }

    /// `NAME=value` before the command name.
    fn is_assignment(&self) -> bool {
        let Some(eq) = self.chars.iter().position(|&(c, q)| c == '=' && !q) else {
            return false;
        };
        let name = &self.chars[..eq];
        !name.is_empty()
            && !name[0].0.is_ascii_digit()
            && name
                .iter()
                .all(|&(c, q)| !q && (c.is_ascii_alphanumeric() || c == '_'))
    }
}

#[derive(Debug)]
enum Tok {
    Word(Word),
    /// The end of a simple command.
    Sep,
}

/// Words and separators of a command, as the shell splits it.
struct Lexer {
    c: Vec<char>,
    i: usize,
    toks: Vec<Tok>,
    word: Option<Word>,
    /// The next word is a redirection's target, not an argument.
    drop_next: bool,
    /// The next word is a here-document's delimiter (`<<-`: tabs stripped).
    heredoc_next: Option<bool>,
    /// Here-documents whose bodies start after the current line.
    heredocs: Vec<(String, bool)>,
}

impl Lexer {
    fn new(s: &str) -> Lexer {
        Lexer {
            c: s.chars().collect(),
            i: 0,
            toks: Vec::new(),
            word: None,
            drop_next: false,
            heredoc_next: None,
            heredocs: Vec::new(),
        }
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.c.get(self.i + k).copied()
    }

    fn word(&mut self) -> &mut Word {
        self.word.get_or_insert_with(Word::default)
    }

    fn push(&mut self, c: char, quoted: bool) {
        self.word().chars.push((c, quoted));
    }

    fn finish(&mut self) {
        let Some(w) = self.word.take() else { return };
        if let Some(strip) = self.heredoc_next.take() {
            self.heredocs.push((w.text(), strip));
        } else if self.drop_next {
            self.drop_next = false;
        } else {
            self.toks.push(Tok::Word(w));
        }
    }

    fn sep(&mut self) {
        self.finish();
        self.drop_next = false;
        if matches!(self.toks.last(), Some(Tok::Word(_))) {
            self.toks.push(Tok::Sep);
        }
    }

    /// A newline ends the command; the bodies of the here-documents opened
    /// on its line follow and are skipped.
    fn newline(&mut self) {
        self.sep();
        self.i += 1;
        for (delim, strip) in std::mem::take(&mut self.heredocs) {
            while self.i < self.c.len() {
                let end = self.c[self.i..]
                    .iter()
                    .position(|&ch| ch == '\n')
                    .map_or(self.c.len(), |k| self.i + k);
                let line: String = self.c[self.i..end].iter().collect();
                self.i = end + 1;
                let line = if strip {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if line == delim {
                    break;
                }
            }
        }
    }

    fn redirection(&mut self) {
        // A descriptor number right before the operator (`2>`) belongs to it.
        if self.word.as_ref().is_some_and(|w| {
            !w.dynamic
                && !w.chars.is_empty()
                && w.chars.iter().all(|&(c, q)| !q && c.is_ascii_digit())
        }) {
            self.word = None;
        }
        self.finish();
        if self.peek(0) == Some('<') && self.peek(1) == Some('<') {
            if self.peek(2) == Some('<') {
                // A here-string: its word is input, not an argument.
                self.i += 3;
                self.drop_next = true;
                return;
            }
            self.i += 2;
            let strip = self.peek(0) == Some('-');
            if strip {
                self.i += 1;
            }
            self.heredoc_next = Some(strip);
            return;
        }
        self.i += 1;
        while matches!(self.peek(0), Some('<' | '>' | '|')) {
            self.i += 1;
        }
        if self.peek(0) == Some('&') {
            self.i += 1;
            // `>&2`, `<&-`: a descriptor, no file name follows.
            if matches!(self.peek(0), Some('0'..='9' | '-')) {
                while matches!(self.peek(0), Some('0'..='9' | '-')) {
                    self.i += 1;
                }
                return;
            }
        }
        self.drop_next = true;
    }

    /// From `$`: a parameter, `$(...)`, `${...}` or `$'...'`, skipped whole.
    fn expansion(&mut self) {
        self.word().dynamic = true;
        self.i += 1;
        match self.peek(0) {
            Some('(') => self.skip_balanced('(', ')'),
            Some('{') => self.skip_balanced('{', '}'),
            Some('\'') => {
                self.i += 1;
                while let Some(c) = self.peek(0) {
                    self.i += 1;
                    if c == '\\' {
                        self.i += 1;
                    } else if c == '\'' {
                        break;
                    }
                }
            }
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => {
                while matches!(self.peek(0), Some(c) if c.is_ascii_alphanumeric() || c == '_') {
                    self.i += 1;
                }
            }
            Some('?' | '$' | '!' | '#' | '@' | '*' | '-') => self.i += 1,
            _ => {}
        }
    }

    fn skip_balanced(&mut self, open: char, close: char) {
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            self.i += 1;
            match c {
                '\\' => self.i += 1,
                '\'' => {
                    while let Some(q) = self.peek(0) {
                        self.i += 1;
                        if q == '\'' {
                            break;
                        }
                    }
                }
                '"' => {
                    while let Some(q) = self.peek(0) {
                        self.i += 1;
                        if q == '\\' {
                            self.i += 1;
                        } else if q == '"' {
                            break;
                        }
                    }
                }
                c if c == open => depth += 1,
                c if c == close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn backquoted(&mut self) {
        self.word().dynamic = true;
        self.i += 1;
        while let Some(c) = self.peek(0) {
            self.i += 1;
            if c == '\\' {
                self.i += 1;
            } else if c == '`' {
                break;
            }
        }
    }

    fn double_quoted(&mut self) {
        self.word();
        self.i += 1;
        while let Some(c) = self.peek(0) {
            match c {
                '"' => {
                    self.i += 1;
                    return;
                }
                '\\' => {
                    match self.peek(1) {
                        Some(n @ ('$' | '`' | '"' | '\\')) => self.push(n, true),
                        Some('\n') => {}
                        Some(n) => {
                            self.push('\\', true);
                            self.push(n, true);
                        }
                        None => self.push('\\', true),
                    }
                    self.i += 2;
                }
                '$' => self.expansion(),
                '`' => self.backquoted(),
                _ => {
                    self.push(c, true);
                    self.i += 1;
                }
            }
        }
    }

    fn run(mut self) -> Vec<Tok> {
        while let Some(ch) = self.peek(0) {
            match ch {
                ' ' | '\t' | '\r' => {
                    self.finish();
                    self.i += 1;
                }
                '\n' => self.newline(),
                ';' | '(' | ')' => {
                    self.sep();
                    self.i += 1;
                }
                '|' => {
                    self.sep();
                    self.i += 1;
                    if matches!(self.peek(0), Some('|' | '&')) {
                        self.i += 1;
                    }
                }
                '&' if self.peek(1) == Some('>') => {
                    self.finish();
                    self.i += 2;
                    if self.peek(0) == Some('>') {
                        self.i += 1;
                    }
                    self.drop_next = true;
                }
                '&' => {
                    self.sep();
                    self.i += 1;
                    if self.peek(0) == Some('&') {
                        self.i += 1;
                    }
                }
                '<' | '>' => self.redirection(),
                '#' if self.word.is_none() => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                '\'' => {
                    self.word();
                    self.i += 1;
                    while let Some(c) = self.peek(0) {
                        self.i += 1;
                        if c == '\'' {
                            break;
                        }
                        self.push(c, true);
                    }
                }
                '"' => self.double_quoted(),
                '\\' => {
                    match self.peek(1) {
                        Some('\n') | None => {}
                        Some(c) => self.push(c, true),
                    }
                    self.i += 2;
                }
                '$' => self.expansion(),
                '`' => self.backquoted(),
                _ => {
                    self.push(ch, false);
                    self.i += 1;
                }
            }
        }
        self.finish();
        self.toks
    }
}

/// Words before the command name that are not it: assignments, keywords
/// that start a command list, and wrappers that run the command after them
/// (with their options).
const PREFIXES: &[&str] = &[
    "!", "{", "do", "then", "else", "if", "elif", "while", "until", "time", "nohup", "command",
    "builtin", "exec", "sudo", "doas", "env", "nice", "ionice",
];

/// Brace lists of `w` expanded (`a{b,c}d` -> `abd`, `acd`); braces that
/// list nothing stay literal. None: a sequence (`{1..3}`) or too many
/// results.
fn braces(w: &[(char, bool)]) -> Option<Vec<Vec<(char, bool)>>> {
    for open in 0..w.len() {
        if w[open] != ('{', false) {
            continue;
        }
        let mut depth = 0usize;
        let mut commas = Vec::new();
        let mut close = None;
        for (j, &cq) in w.iter().enumerate().skip(open) {
            match cq {
                ('{', false) => depth += 1,
                ('}', false) => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                (',', false) if depth == 1 => commas.push(j),
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        if commas.is_empty() {
            if w[open + 1..close]
                .windows(2)
                .any(|p| p[0].0 == '.' && p[1].0 == '.')
            {
                return None;
            }
            continue;
        }
        let mut out = Vec::new();
        let mut start = open + 1;
        for end in commas.into_iter().chain([close]) {
            let mut v = w[..open].to_vec();
            v.extend_from_slice(&w[start..end]);
            v.extend_from_slice(&w[close + 1..]);
            out.extend(braces(&v)?);
            if out.len() > MAX_EXPANSION {
                return None;
            }
            start = end + 1;
        }
        return Some(out);
    }
    Some(vec![w.to_vec()])
}

/// The absolute paths an operand names; None when it cannot be read.
fn operand_paths(w: &Word, dir: Option<&Path>, home: Option<&Path>) -> Option<Vec<PathBuf>> {
    if w.dynamic {
        return None;
    }
    let mut out = Vec::new();
    for e in braces(&w.chars)? {
        if e.iter().any(|&(c, q)| !q && matches!(c, '*' | '?' | '[')) {
            return None;
        }
        let text: String = e.iter().map(|&(c, _)| c).collect();
        if text.is_empty() {
            continue;
        }
        let path = if e[0] == ('~', false) {
            let home = home?;
            match text.strip_prefix('~')? {
                "" => home.to_path_buf(),
                rest => home.join(rest.strip_prefix('/')?),
            }
        } else {
            PathBuf::from(text)
        };
        let abs = if path.is_absolute() {
            path
        } else {
            dir?.join(path)
        };
        out.push(normalize(&abs));
    }
    Some(out)
}

fn mkdir(args: &[&Word], dir: Option<&Path>, home: Option<&Path>) -> Vec<Target> {
    let mut parents = false;
    let mut options = true;
    let mut skip = false;
    let mut paths = Vec::new();
    for w in args {
        if skip {
            skip = false;
            continue;
        }
        if w.dynamic {
            continue;
        }
        // Options are read after quote removal, as mkdir sees them.
        let text = w.text();
        if options && text.len() > 1 && text.starts_with('-') {
            if text == "--" {
                options = false;
            } else if let Some(long) = text.strip_prefix("--") {
                match long {
                    "parents" => parents = true,
                    "mode" => skip = true,
                    _ => {}
                }
            } else {
                let cluster = &text[1..];
                for (k, ch) in cluster.char_indices() {
                    match ch {
                        'p' => parents = true,
                        // `-m MODE` or `-mMODE`: the rest is the mode.
                        'm' => {
                            skip = k + 1 == cluster.len();
                            break;
                        }
                        _ => {}
                    }
                }
            }
            continue;
        }
        paths.extend(operand_paths(w, dir, home).unwrap_or_default());
    }
    paths
        .into_iter()
        .map(|path| Target { path, parents })
        .collect()
}

/// The working directory after `cd ARG`; None when it cannot be known.
fn cd(dir: Option<&Path>, args: &[&Word], home: Option<&Path>) -> Option<PathBuf> {
    let arg = args.iter().find(|w| {
        let t = w.text();
        w.dynamic || !t.starts_with('-') || t == "-"
    });
    let Some(w) = arg else {
        return home.map(Path::to_path_buf);
    };
    if w.text() == "-" {
        return None;
    }
    match operand_paths(w, dir, home)?.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// Every `mkdir` operand of `command`, in order; `cwd` is where it runs.
pub fn targets(command: &str, cwd: Option<&Path>, home: Option<&Path>) -> Vec<Target> {
    let toks = Lexer::new(command).run();
    let mut dir = cwd.map(Path::to_path_buf);
    let mut out = Vec::new();
    for seg in toks.split(|t| matches!(t, Tok::Sep)) {
        let words: Vec<&Word> = seg
            .iter()
            .filter_map(|t| match t {
                Tok::Word(w) => Some(w),
                Tok::Sep => None,
            })
            .collect();
        let mut k = 0;
        while k < words.len() {
            let w = words[k];
            if w.is_assignment() {
                k += 1;
            } else if !w.dynamic && PREFIXES.contains(&w.text().as_str()) {
                k += 1;
                while k < words.len() && words[k].text().starts_with('-') {
                    k += 1;
                }
            } else {
                break;
            }
        }
        let Some((name, args)) = words[k.min(words.len())..].split_first() else {
            continue;
        };
        let Some(name) = name.literal() else { continue };
        match name.rsplit('/').next().unwrap_or("") {
            "cd" | "pushd" => dir = cd(dir.as_deref(), args, home),
            "mkdir" => out.extend(mkdir(args, dir.as_deref(), home)),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(cmd: &str) -> Vec<(String, bool)> {
        targets(cmd, Some(Path::new("/w")), Some(Path::new("/h")))
            .into_iter()
            .map(|t| (t.path.to_string_lossy().into_owned(), t.parents))
            .collect()
    }

    fn p(paths: &[&str], parents: bool) -> Vec<(String, bool)> {
        paths.iter().map(|s| (s.to_string(), parents)).collect()
    }

    #[test]
    fn plain_and_flagged_mkdirs() {
        assert_eq!(t("mkdir a"), p(&["/w/a"], false));
        assert_eq!(t("mkdir -p a/b /abs/c"), p(&["/w/a/b", "/abs/c"], true));
        assert_eq!(t("mkdir -pv -m 755 x"), p(&["/w/x"], true));
        assert_eq!(t("mkdir -m755 -v x"), p(&["/w/x"], false));
        assert_eq!(t("mkdir --mode 700 --parents x"), p(&["/w/x"], true));
        assert_eq!(t("mkdir --mode=700 x"), p(&["/w/x"], false));
        assert_eq!(t("mkdir -pm 755 x"), p(&["/w/x"], true));
        assert_eq!(t("mkdir x -p"), p(&["/w/x"], true));
        assert_eq!(t("mkdir -- -odd"), p(&["/w/-odd"], false));
        assert_eq!(t("/usr/bin/mkdir ../up/./x"), p(&["/up/x"], false));
        assert_eq!(t("sudo -E mkdir /srv/x"), p(&["/srv/x"], false));
        assert_eq!(t("LANG=C mkdir x"), p(&["/w/x"], false));
    }

    #[test]
    fn separators_quotes_and_cd() {
        assert_eq!(
            t("cd proj && mkdir -p src; mkdir 'a b' \"c;d\" || echo no | mkdir e"),
            p(
                &["/w/proj/src", "/w/proj/a b", "/w/proj/c;d", "/w/proj/e"],
                false
            )
            .into_iter()
            .enumerate()
            .map(|(i, (s, _))| (s, i == 0))
            .collect::<Vec<_>>()
        );
        assert_eq!(t("cd /x\nmkdir y"), p(&["/x/y"], false));
        assert_eq!(t("cd && mkdir y"), p(&["/h/y"], false));
        assert_eq!(t("cd - && mkdir y"), p(&[], false));
        assert_eq!(t("cd \"$D\" && mkdir y /z"), p(&["/z"], false));
        assert_eq!(t("(mkdir a) & mkdir b"), p(&["/w/a", "/w/b"], false));
        assert_eq!(t("mkdir a\\ b"), p(&["/w/a b"], false));
        assert_eq!(t("if mkdir x; then echo; fi"), p(&["/w/x"], false));
        assert_eq!(t("{ mkdir x; }"), p(&["/w/x"], false));
    }

    #[test]
    fn expansions_globs_tildes_and_braces() {
        assert_eq!(
            t("mkdir $D \"$HOME/x\" `pwd`/y $(pwd)/z ok"),
            p(&["/w/ok"], false)
        );
        assert_eq!(t("mkdir '$D' \"\\$E\""), p(&["/w/$D", "/w/$E"], false));
        assert_eq!(t("mkdir *.d a? [ab]"), p(&[], false));
        assert_eq!(t("mkdir '*.d'"), p(&["/w/*.d"], false));
        assert_eq!(
            t("mkdir ~ ~/x ~bob/y '~'"),
            p(&["/h", "/h/x", "/w/~"], false)
        );
        assert_eq!(
            t("mkdir -p src/{core,cli/{a,b}} x{1..3} {lit}"),
            p(
                &["/w/src/core", "/w/src/cli/a", "/w/src/cli/b", "/w/{lit}"],
                true
            )
        );
        assert_eq!(t("mkdir ~/{a,b}"), p(&["/h/a", "/h/b"], false));
    }

    #[test]
    fn redirections_comments_and_heredocs() {
        assert_eq!(t("mkdir a 2>/dev/null >out &>both"), p(&["/w/a"], false));
        assert_eq!(t("mkdir a 2>&1 b"), p(&["/w/a", "/w/b"], false));
        assert_eq!(t("echo mkdir x # mkdir y"), p(&[], false));
        assert_eq!(t("mkdir a#b"), p(&["/w/a#b"], false));
        let heredoc = "cat > s.sh <<'EOF'\nmkdir -p inside\nEOF\nmkdir after";
        assert_eq!(t(heredoc), p(&["/w/after"], false));
        let tabs = "cat <<-END > f\n\tmkdir no\n\tEND\nmkdir yes";
        assert_eq!(t(tabs), p(&["/w/yes"], false));
        assert_eq!(t("cat <<< 'mkdir no'; mkdir yes"), p(&["/w/yes"], false));
        assert_eq!(t("printf 'mkdir x\\n' | sh"), p(&[], false));
    }

    #[test]
    fn unreadable_input_gives_nothing_and_never_panics() {
        for cmd in [
            "",
            "mkdir",
            "mkdir -p",
            "'unterminated mkdir x",
            "\"also",
            "$(",
            "mkdir {",
            "mkdir }{,",
            "\\",
            "<<",
            "mkdir a <<",
            "cd",
            "mkdir \u{1F600}/x",
        ] {
            let _ = t(cmd);
        }
        assert_eq!(t("mkdir \u{e9}t\u{e9}"), p(&["/w/\u{e9}t\u{e9}"], false));
        // No working directory: relative operands are unknown.
        assert_eq!(targets("mkdir a /b", None, None).len(), 1);
    }
}
