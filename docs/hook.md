# xtf-hook: the Claude Code hook

`xtf-hook` (crate `crates/xtf-hook`) puts standard xks-folder/1 into
Claude Code. Claude Code runs it as a command hook: one event as JSON on
standard input, at most one JSON object on standard output, exit code 0
always. The input and output follow Claude Code's hooks reference (read
2026-11-21, Claude Code 2.1.316): "Common input fields", "PreToolUse input"
(the `Write` and `Bash` tool inputs), "SessionStart input", "JSON output",
"Add context for Claude", "PreToolUse decision control".

| subcommand | event | matcher | what it does |
|---|---|---|---|
| `xtf-hook pre-tool-use` | PreToolUse | `Write`, `Bash` | decides new files and new folders before they are made |
| `xtf-hook session-start` | SessionStart | `startup\|resume\|clear\|compact` | summarises the project's manifest, checks the server |

The plugin (`integrations/claude-code/plugin/hooks/hooks.json`) runs
`${CLAUDE_PLUGIN_ROOT}/bin/xtf-hook` with these subcommands; it gives
PreToolUse a 30 s timeout. The hook keeps every run under 16 s on its own
(see Fail open), so Claude Code never has to cancel it.

## PreToolUse

### Which paths are decided

- **Write**: `tool_input.file_path` (absolute, as Claude Code delivers it;
  `..` is removed lexically). A file that already exists is an overwrite,
  not a placement: no output. A file named `xks-project.json` is the fix
  the notes below ask for: no output.
- **Bash**: a command whose text does not contain `mkdir` ends the run at
  once (no output). Otherwise the command is read the way the shell splits
  it, without running anything: words with quotes and escapes, comments,
  here-document bodies (a `mkdir` inside a heredoc being written to a file
  is not a command), redirections, and simple commands separated by `;`,
  `&&`, `||`, `|`, `&`, newlines and parentheses. Each `mkdir` gives its
  operands: options (`-p`, `--parents`, `-m MODE`, `-mMODE`, `--mode MODE`,
  `-v`, `--`) are read as mkdir reads them, relative operands resolve
  against the event's `cwd` (a literal `cd DIR` before the `mkdir` moves
  it), `~/` is the home folder, and brace lists expand (`src/{a,b}`).
  Operands holding `$`, backquotes, a glob, `~user` or a sequence
  (`{1..3}`) are skipped. From each operand the hook takes the folder where
  the new tree attaches to the existing one (for `mkdir -p a/b/c` with none
  of it there: `a`; without `-p`: the operand, when its parent exists). The
  folder itself is never created by the hook.

Never decided on (no output): paths under the folders of `XTF_HOOK_SKIP`
(default `/tmp:~/.claude`), under the event's `scratchpad_dir`, and under
any `.git` or `.xks` folder.

### Inside a project

The project is the nearest folder above the path holding an
`xks-project.json` (`Manifest::find_root`). The path becomes a projection
(`create_file` with the start of the content as its summary, or
`create_dir` with the Bash call's `description`), relative to the project
root, and `xtf_standard::Decider` decides it: the rules first, the model
only when they leave the question open. Every decision is appended to the
project's `.xks/decisions.jsonl` (the standard's decision record). All the
decisions of one run share one budget of 15 s.

What the hook says:

| verdict | `enforce` (default) | `advise` |
|---|---|---|
| suggest: the model chose another role with confidence >= 0.5, or the name breaks the project's naming style | deny, once per session and path; the same attempt again goes through, with a note | the suggestion as context |
| uncertain (the model is unsure) or allow | nothing | nothing |
| `file.name_clear` P(yes) <= 0.30 | a note naming the MCP tool `pick_name` | same |
| the operation creates a top-level folder the layout does not list | a note with the layout (no model needed) | same |
| the model reads a folder outside the layout (a new one, or the one a file goes under) as a role the layout lacks, confidence >= 0.5 (since 0.4.0) | recorded in xks-project.json (locked, atomic, never moving a mapping) and said as context; never denied | same |

Measured (2026-11-21, Qwen3.8 2B Q8_0, a manifest with an empty layout):
`mkdir scripts` read as tools at 0.9022 and recorded (`{"tools":
["scripts"]}`, two decision lines: the reading, then "recorded by the xtf
hook"); `mkdir scripts` again took 2 ms, allowed by rule as a folder the
manifest now names.

Three verdicts of the library are not acted on, because the suggested path
would not be an improvement:

- the suggested folder already holds the path (`integrations/x/y.json`
  read as integrations, whose folder is `integrations`): the library puts
  the file at the top of the role's folder and drops the subpath;
- `out` decided by the extension rule only because the project's layout
  has none of the roles the extension allows (a `.sh` file in a project
  without tools, tests, integrations or examples folders gets `out` with
  confidence 1.00; a `.log`, whose only role is `out`, is still moved);
- a new folder the manifest already declares (a layout or ignore entry
  that does not exist yet): nothing to decide.

A rename (naming style) is still suggested in the first two cases, in the
path's own folder.

### Outside any project (the projects function)

No `xks-project.json` above the path: no decision, never a denial. A note,
once per session for each folder, says how a manifest is written:

- inside a project that has none (a `.git`, `Cargo.toml`, `package.json`,
  `Makefile`, `pyproject.toml`, `go.mod`, `CMakeLists.txt` or similar at or
  above the path; the home folder and `/` never count): `xtf init` for an
  existing project, the MCP tool `new_project` for a new one;
- outside any project: a `Write` gets the note for the new folder it
  creates (or the file's folder); an `mkdir` of a new folder gets the note
  that, when it is a new project's root, `new_project` lays out its folders
  and writes its manifest.

## SessionStart

No model call. With an `xks-project.json` at or above `cwd`: the standard,
the project's name, kind, languages, root and purpose (at most 400
characters), role -> folders, naming styles, what is never decided on, the
hook's mode, and the MCP tools `place_file`, `plan_tree`, `pick_name`,
`new_project`. Without one, inside a project: the note that it has none.
Elsewhere (for example the home folder): nothing. Whenever it says
something, it adds whether the decision server answers `GET /health`
within 300 ms: up (with the server's answer), not reachable, or no answer
in time. The server serves one request at a time, so a decision for
another session delays `/health` (measured: 698.5 ms behind a 706.1 ms
decision, against 0.11 to 0.22 ms idle); the text says so instead of
calling the server down.

## Modes and settings

| variable | values |
|---|---|
| `XTF_HOOK_MODE` | `enforce` (default), `advise`, `off` (the hook exits at once). Any other value runs as `advise`, so a typo never blocks, and is logged. |
| `XTF_HOOK_SKIP` | colon-separated absolute or `~/` folders never decided on; replaces the default `/tmp:~/.claude`. The scratchpad, `.git` and `.xks` are always skipped. |
| `TYPESAFE_BASE_URL`, `TYPESAFE_API_KEY`, `TYPESAFE_DEFAULT_MODEL` | the decision server (`xtf_standard::client`; default `http://127.0.0.1:8097`, `jevbase serve`) |
| `XDG_CACHE_HOME` | the state folder is `$XDG_CACHE_HOME/xtf/hook`, else `~/.cache/xtf/hook` |

## State

The state folder holds one line file per session, `<session_id>.state`
(the id with anything but letters, digits, `-` and `_` replaced by `_`),
written only when there is something to remember, and `errors.log`:

```
denied	~/hook-test/proj/docs/parse_flags.rs
noted	~/hook-test/newproj
```

`denied` paths go through on the next attempt in the same session;
`noted` folders get no second note. A session file past 64 KiB, and
`errors.log` past 256 KiB, keep their newer half (written to a temporary
file and renamed over the old one). The hook never removes a file. A
denial that cannot be recorded is given as context instead: unrecorded, it
would deny every repeat.

## Fail open

Every failure ends the run with nothing on standard output and exit code 0,
and one line in `errors.log` (UTC time, subcommand, session, message):
unreadable or malformed input, an event of the wrong kind, a manifest that
does not parse, a decision server that refuses, fails or does not answer,
an I/O error, a panic. A denial is a `permissionDecision`, never exit code
2.

The model's budget is enforced by the hook, not by the HTTP client:
jevbase's `Client` (v0.1.0-rc.1) connects with a 2 s timeout and fails at
once on a refused connection, but its read timeout (300 s by default) is
per read call, it sleeps up to 60 s between up to 3 retries of a 429, and
name lookup has no timeout. The hook asks on a worker thread and waits on
a channel until 15 s after the run started; whatever is not decided by
then is logged, and the process exits without waiting for the thread. The
client's own timeout is set to 15 s as well, which bounds `curl`'s
`max-time` for an `https://` server. SessionStart waits 300 ms for
`/health` the same way.

```
2026-11-21T21:29:00Z pre-tool-use session=rec-9 ~/hook-test/proj/crates/demo/src/flags.rs: no System One server: 127.0.0.1:9: Connection refused (os error 111) (start one with `jevbase serve`, or set TYPESAFE_BASE_URL)
2026-11-21T21:27:08Z pre-tool-use session=s-down ~/hook-test/proj/crates/demo/src/flags.rs: no decision within 15 s
```

## Recorded outputs

Recorded 2026-11-21 with the release binary against `jevbase serve`
(Qwen3.8-2B-Q8_0, CPU), in a test project holding this repository's
`xks-project.json` (layout source: crates, docs: docs, data: calibration,
out: out). The test folder is shown as `~/hook-test`. Each block is one run's
standard output, one line as printed.

Deny: a Rust source file written into `docs/` (`file.root`: source 0.7883,
out 0.2117, Choice confidence 0.5765):

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"xks-folder/1, project `xks-toolintercept-folder_mngmt`: new file `docs/parse_flags.rs`: the model reads it as source (the program's own source code), confidence 0.58; source of this project goes in `crates/`. Suggested path: `crates/parse_flags.rs` (~/hook-test/proj/crates/parse_flags.rs).\nThe xtf hook denies a path once per session: the same Write again goes through as is."}}
```

The same Write again in the same session (no model call):

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"xks-folder/1, project `xks-toolintercept-folder_mngmt`: `docs/parse_flags.rs` stays where it is: the xtf hook suggested another path for it earlier in this session, and this repeated Write goes through as is (recorded in .xks/decisions.jsonl)."}}
```

`XTF_HOOK_MODE=advise`, the same file in a new session:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"xks-folder/1, project `xks-toolintercept-folder_mngmt`: new file `docs/parse_flags.rs`: the model reads it as source (the program's own source code), confidence 0.58; source of this project goes in `crates/`. Suggested path: `crates/parse_flags.rs` (~/hook-test/proj/crates/parse_flags.rs)."}}
```

Naming style (`ParseFlags.rs` in a snake_case Rust project):

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"xks-folder/1, project `xks-toolintercept-folder_mngmt`: new file `crates/demo/src/ParseFlags.rs`: `ParseFlags.rs` does not follow the project's naming style for source files (snake_case); `parse_flags.rs` does. Suggested path: `crates/demo/src/parse_flags.rs` (~/hook-test/proj/crates/demo/src/parse_flags.rs).\nThe xtf hook denies a path once per session: the same Write again goes through as is."}}
```

Uncertain, no output: a `# Design notes` markdown file written into
`crates/demo/src` (`file.root`: docs 0.6059, config 0.2857, out 0.1084,
Choice confidence 0.4088, under 0.5).

`mkdir -p misc/sub` in the project (`misc` decided as well: out 0.3217,
confidence 0.1522, uncertain):

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"xks-folder/1, project `xks-toolintercept-folder_mngmt`: `misc/` is a new top-level folder that the project's layout does not list (layout in xks-project.json: source: crates; docs: docs; data: calibration; out: out; files directly in the root play the config role)."}}
```

`mkdir -p ~/hook-test/newproj/src && cd ~/hook-test/newproj && git init -q`
(first time in the session; the second time: no output):

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"xks-folder/1: `~/hook-test/newproj` is a new folder outside any project (no xks-project.json and no project files such as .git, Cargo.toml or package.json above it). When it is a new project's root, the xtf MCP tool new_project lays out its folders and writes its xks-project.json; `xtf init` writes one for an existing project."}}
```

Write with no manifest (first time; the second time: no output):

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"xks-folder/1: `~/hook-test/noproj/notes` is outside any project (no xks-project.json and no project files such as .git, Cargo.toml or package.json at or above it), so the xtf hook does not decide new files there. The xtf MCP tool new_project lays out a new project's folders and writes its xks-project.json; `xtf init` writes one for an existing project."}}
```

`mkdir` inside a folder with `.git` and no manifest:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"xks-folder/1: `~/hook-test/gitproj` has no xks-project.json (it holds .git), so the xtf hook does not decide new files and folders in it. `xtf init` writes one for an existing project; the xtf MCP tool new_project lays out a new project's folders and writes one."}}
```

SessionStart in the project, server up:

```json
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"xks-folder/1 project `xks-toolintercept-folder_mngmt` (kind not set; rust) at `~/hook-test/proj`: The xks-folder/1 standard, its Rust library and the xtf command line: they ask a Jev-like model, through any System One server, where a project's new files and folders belong and what to name them. An MCP server and a Claude Code hook are planned on top.\nLayout, role: folders: source: crates; docs: docs; data: calibration; out: out; files directly in the root play the config role.\nNaming: source files snake_case, new folders any. Never decided on: .git, target, node_modules, .xks, the out and third_party folders.\nThe xtf hook (mode enforce) decides new files (Write) and new folders (mkdir in Bash) against this layout, rules first and then the decision server's model; a confident suggestion denies the first attempt at a path once per session, and the same attempt again goes through. The xtf MCP tools place_file, plan_tree, pick_name and new_project answer placement, layout and naming questions against the same standard.\nThe decision server at http://127.0.0.1:8097 answers (GET /health: {\"status\":\"ok\",\"subject\":\"Qwen3.8-2B-Q8_0\",\"site\":\"cpu\"})."}}
```

The last line with the server down (`TYPESAFE_BASE_URL=http://127.0.0.1:9`)
and with a server that accepts and never answers:

```
The decision server at http://127.0.0.1:9 is not reachable (127.0.0.1:9: Connection refused (os error 111)): folder decisions are unavailable, and the xtf hook lets every new file and folder through undecided (fail open).
The decision server at http://127.0.0.1:18731 did not answer GET /health within 300 ms (it serves one request at a time, so a decision for another session delays it): the xtf hook lets a new file or folder it cannot decide within 15 s through undecided.
```

## Latency

Measured 2026-11-21 on an AMD Ryzen 7 5800X, `jevbase serve` (Qwen3.8 2B
Q8_0, 8 threads) on 127.0.0.1:8097, release build. Wall time from fork to
exit of the hook process, fed the event file on standard input, by a small
C harness (`hookrun N EVENT [VAR=VALUE ...] -- xtf-hook SUBCOMMAND`; it
also checks exit code 0 and that standard output is empty or one JSON
object on one line, every run below passed). Fork and exec of `/bin/echo`
by the same harness: 0.458 to 0.695 ms.

| case | runs | min | median | max | output |
|---|---|---|---|---|---|
| Write of an existing file | 20 | 0.699 ms | 0.826 ms | 1.542 ms | none |
| Bash without mkdir | 20 | 0.659 ms | 0.790 ms | 1.098 ms | none |
| Bash, mkdir only inside a heredoc | 10 | 0.809 ms | 0.823 ms | 0.909 ms | none |
| path in the scratchpad (default skip list) | 10 | 0.786 ms | 0.823 ms | 0.875 ms | none |
| `XTF_HOOK_MODE=off` | 20 | 0.645 ms | 0.770 ms | 0.995 ms | none |
| malformed input | 10 | 0.674 ms | 0.816 ms | 1.050 ms | none (logged) |
| relative `file_path` | 3 | 0.558 ms | 0.565 ms | 0.598 ms | none (logged) |
| server down (port 9) | 10 | 0.733 ms | 0.782 ms | 0.901 ms | none (logged) |
| server accepts, never answers | 2 | 15001.284 ms | 15001.302 ms | 15001.319 ms | none (logged) |
| new file in its place (model: allow) | 3 | 1623.571 ms | 1677.895 ms | 1716.789 ms | none |
| markdown into `crates/demo/src` (model: uncertain) | 3 | 2541.046 ms | 2549.997 ms | 2605.463 ms | none |
| Rust source into `docs/` (model: suggest) | 1 | 1863.758 ms | | | deny |
| the same Write again, same session | 5 | 0.660 ms | 0.816 ms | 0.902 ms | context |
| advise mode (model: suggest) | 2 | 1692.299 ms | 1733.687 ms | 1775.076 ms | context |
| `.sh` into `crates/demo` (name question only) | 2 | 461.543 ms | 475.439 ms | 489.336 ms | none |
| `ParseFlags.rs`: first, then again | 1 + 1 | 1723.592 ms, then 1.076 ms | | | deny, then context |
| Write with no manifest, same session | 5 | 0.921 ms | 0.964 ms | 1.058 ms | context once |
| `mkdir -p misc/sub` (model) | 2 | 2071.626 ms | 2157.675 ms | 2243.724 ms | context |
| `mkdir -p` of a new project root, same session | 5 | 0.829 ms | 0.886 ms | 0.983 ms | context once |
| `mkdir` in a `.git` project without manifest | 3 | 0.670 ms | 0.776 ms | 0.822 ms | context once |
| one command creating 4 new folders (4 decisions) | 1 | 9513.937 ms | | | context |
| two hooks at once (two sessions, model) | 1 + 1 | 1721.423 ms and 3443.028 ms | | | none |
| session-start, manifest, server up | 10 | 0.721 ms | 0.840 ms | 1.058 ms | context |
| session-start, manifest, server down | 10 | 0.656 ms | 0.717 ms | 1.119 ms | context |
| session-start, server never answers | 3 | 301.031 ms | 301.105 ms | 301.133 ms | context |
| session-start, `.git` project, no manifest | 10 | 0.768 ms | 0.820 ms | 0.889 ms | context |
| session-start, not a project | 10 | 0.714 ms | 0.821 ms | 0.890 ms | none |

What the numbers say:

- Without a model call the hook costs about 0.3 ms over a bare process
  start (median 0.77 to 0.96 ms against 0.46 to 0.70 ms).
- A decision costs the server's time, 0.46 s (one `file.name_clear`
  question) to 3.1 s, and the server answers one request at a time: two
  hooks at once took 1.72 s and 3.44 s.
- New folders: all 6 `create_dir` decisions measured (misc twice,
  `crates/demo/src/net`, `crates/demo/src/store`, `misc2`, `docs/guides`)
  had Choice confidence 0.1182 to 0.1671, so none was acted on, and each
  cost 2.1 to 3.1 s. The top-level note, which needs no model, is what an
  `mkdir` gets in practice with this model.
- Markdown and CSV files written into the source folder (4 files) or the
  project root (1 file) were not confident (0.3451 to 0.4433), so they pass
  without a word, as the standard's 0.5 floor intends (SPEC.md,
  Calibration).
