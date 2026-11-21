# xks-toolintercept-folder_mngmt

Folder and naming decisions for coding agents, asked of a Jev-like model the
same way in every project.

A coding agent creates files and folders all day, and every project ends up
laid out by whatever the agent guessed at the time. This project fixes the
questions instead: a project describes its layout once, in a small JSON
manifest, and every "where does this new file go, and is it named right?"
becomes the same fixed, typed question, answered by a System One model
(TypeSafe's Jev, or a local Jev-like model), acted on only when the model is
confident, and written down.

**Status: 0.2.0.** The standard ([xks-folder/1](SPEC.md)), its Rust
library and `xtf` command line, its calibration on six real repositories,
and, new in 0.2.0, the **Claude Code plugin** that puts it in front of every
session automatically: a hook on every new file and folder, an MCP server
with five tools (new projects and their folder structure among them), and
one shared model instance as a user service ([Claude Code](#claude-code)).

## How a decision is made

1. **Provided JSON, the manifest** (`xks-project.json` at the project root):
   name, purpose, languages, and which of 13 fixed roles (source, tests,
   docs, tools, out, ...) each top-level folder plays.
2. **Projected JSON, the operation**: a file or folder about to be created
   (or moved, renamed), with a short summary of what it holds.
3. **Rules first**, no model: deletions and generated or vendored paths are
   left alone; conventional top-level files (README, LICENSE, Makefile,
   Cargo.toml, dotfiles) are fine where they are; test and benchmark files
   are recognised by their names; a file's extension rules out roles it
   cannot play; source file names are checked against the project's naming
   style (snake_case, kebab-case, ...).
4. **Fixed questions**, when the rules leave it open: "Which part of the
   project does it belong in?", its options the project's roles with fixed
   descriptions; "Does the name tell a reader what the file holds?". The same
   wording in every project. The model never writes a path or a name; it
   picks among options, and its probabilities say how sure it is.
5. **Policy**: a different place is suggested only when the Choice
   confidence is at least 0.5 (TypeSafe's floor for "genuinely unsure", and
   the calibrated value below); otherwise the operation is left as it is and
   marked uncertain.
6. **Record**: every decision goes to `.xks/decisions.jsonl`, the model's
   answers verbatim.

The full standard is [SPEC.md](SPEC.md).

## Requirements

- Rust 1.85 or newer (to build).
- A System One decision server:
  - [`jevbase serve`](https://github.com/Lasimeri/XKS_Jev-like_Model_Base)
    (a local Jev-like model on the CPU; the default, at
    `http://127.0.0.1:8097`), or
  - Intel-Phi-Jev's `xks serve`, or
  - TypeSafe's hosted Jev (`https://api.typesafe.ai`, with an API key; the
    `curl` program must be installed for https).

`xtf init` needs no server.

## Install

```sh
git clone https://github.com/Lasimeri/xks-toolintercept-folder_mngmt
cd xks-toolintercept-folder_mngmt
make                # target/release/xtf
make install        # ~/.local/bin/xtf (PREFIX=... for another place)
make check          # unit tests and clippy, no model needed
```

The binary is small and links no model: the model runs in the decision
server.

## Quick start

```sh
jevbase serve &                                   # the decision server (XKS_Jev-like_Model_Base)
cd my-project
xtf init . --purpose "A CLI that syncs photo folders to a NAS." --lang rust
$EDITOR xks-project.json                          # check the layout it guessed
xtf check src/thumbnails.rs --summary "//! Builds JPEG thumbnails for the sync preview."
```

`check` prints the decision as JSON and appends it to `.xks/decisions.jsonl`.
Every command is described in [docs/cli.md](docs/cli.md).

## Claude Code

One command makes it automatic in every Claude Code session and project on
the machine:

```sh
make install-claude
```

It installs a Claude Code plugin (a directory marketplace under
`~/.local/share/xtf/claude-code`, enabled at user scope) and the decision
server as a systemd user service (`jevbase.service`: `jevbase serve` on
127.0.0.1:8097, the model held in RAM, started at login). `jevbase` itself
comes from [XKS_Jev-like_Model_Base](https://github.com/Lasimeri/XKS_Jev-like_Model_Base)
(its release archive, or `make install` there). Every session then shares
that one model instance.

What every session gets:

| piece | when | what it does |
|---|---|---|
| `xtf-hook session-start` | a session starts, resumes, clears or compacts | tells Claude the project's layout and naming from `xks-project.json` (or that the project has none, and how one is written); no model call, under 1 ms |
| `xtf-hook pre-tool-use` | every `Write` of a new file, every `mkdir` | the model decides the new path against the manifest; when it is confident (>= 0.5) the file belongs elsewhere, or a name breaks the naming style, the first attempt is denied with the suggested path, and the same attempt again goes through (`XTF_HOOK_MODE=enforce`, the default; `advise` adds the suggestion as context only; `off`) |
| MCP tool `new_project` | Claude starts a project | the model decides the project's kind, the standard lays out its folders for that kind and language, the manifest is written and the folders created |
| MCP tools `place_file`, `plan_tree`, `pick_name`, `project_info` | Claude asks | one placement, a whole planned folder tree, a name among candidates, the project's manifest and decision count |

Outside any project, a new file or folder gets a note (once per session and
folder) that `new_project` or `xtf init` writes a manifest; nothing is
blocked. Every decision is appended to the project's `.xks/decisions.jsonl`.
A decision server that is down, busy past 15 s or refusing never blocks
Claude: the hook lets the operation through and records why in
`~/.cache/xtf/hook/errors.log`. Details: [docs/hook.md](docs/hook.md),
[docs/mcp.md](docs/mcp.md).

Checked with Claude Code 2.1.316 itself (`claude -p` on this machine,
2026-11-21, Qwen3.8 2B Q8_0 behind the server):

| test | result |
|---|---|
| a session in a project | plugin loaded, MCP server connected, the five tools offered, the layout given at session start |
| a Rust source file written into `docs/` | decided by the model: source, confidence 0.47, under 0.5, so left as is (4.2 s) |
| `src/ParseFlags.rs` in a snake_case project | denied with `src/parse_flags.rs` suggested (3.3 s); the session wrote the same path again, as its prompt named it, and the hook let it through |
| "lay out a new project" through `new_project` | kind cli-tool decided by the model at confidence 0.85, manifest written, `src/ tests/ docs/ out/` created |
| three sessions writing new files at the same time | all three decided (4.2 s, 5.3 s, 5.7 s, the queue included), none timed out, the shared decision log intact |

## Configuration

The decision server is configured the way TypeSafe's SDKs are:

| variable | default |
|---|---|
| `TYPESAFE_BASE_URL` | `http://127.0.0.1:8097` (`jevbase serve`) |
| `TYPESAFE_API_KEY` | none (needed for the hosted Jev) |
| `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |

Any server that speaks TypeSafe's documented `POST /v1/systemone` answers
the standard's questions; the thresholds were calibrated with `jevbase
serve` and its default model (Qwen3.8 2B Q8_0), so re-run the calibration
when you use another.

## The manifest

```json
{
  "standard": "xks-folder/1",
  "name": "photo-sync",
  "purpose": "A CLI that syncs photo folders to a NAS.",
  "languages": ["rust"],
  "layout": {
    "source": ["src"],
    "tests": ["tests"],
    "docs": ["docs"],
    "tools": ["scripts"],
    "out": ["out"]
  },
  "naming": { "files": "snake_case", "dirs": "any" },
  "ignore": [".git", "target"]
}
```

Roles: source, include, tests, benches, examples, docs, tools,
integrations, data, assets, third_party, out, and config (the root itself).
A role may map to several folders (`"source": ["src", "cli", "tui"]`) or to a
nested one (`"src/tui"`). `out`, `third_party` and `ignore` paths are never
decided on. Naming styles: `snake_case`, `kebab-case`, `PascalCase`,
`camelCase`, `lowercase`, `any`; the file style applies to the project's own
source files only. Details in [SPEC.md](SPEC.md#the-documents).

## Calibration

Measured with `jevbase serve` (Qwen3.8 2B Q8_0, a Ryzen 7 5800X, 8 threads)
on 141 real files of six public repositories, each labelled by the role of
the folder it lives in; 23 conventional top-level files are settled by the
rules, the other 118 are counted (those a name or extension rule leaves one
role for are settled without asking, and counted):

| standard version | right overall | at confidence >= 0.5: answered / right / would move a right file | speed |
|---|---|---|---|
| v1, every role offered | 51.7% | 25.4% / 80.0% / 5.1% | 0.22 decisions/s |
| v2, options narrowed by extension | 59.3% | 32.2% / 89.5% / 3.4% | 0.28 decisions/s |
| v3, v2 + test and benchmark file names (0.1.0) | 61.0% | 33.1% / 92.3% / 2.5% | 0.33 decisions/s |

In plain words: when the model is at least 0.5 confident (a third of new
files), 36 of its 39 answers are right, and it would flag 3 of the 118
correctly placed files (one in forty). Below that it is left alone. The set, the
labels, the per-file results and how to reproduce them are in
[`calibration/`](calibration) and [SPEC.md](SPEC.md#calibration).

## Using the library

```rust
use std::path::Path;
use xtf_standard::{Action, Decider, Manifest, Op, Projection};

let path = Path::new("my-project/src/thumbnails.rs");
let content = "//! Builds JPEG thumbnails for the sync preview.";
let root = Manifest::find_root(path).ok_or("no xks-project.json above the path")?;
let manifest = Manifest::load(&root)?;
let projection = Projection {
    op: Op::CreateFile,
    path: "src/thumbnails.rs".into(),
    from: None,
    summary: xtf_standard::projection::summary_of(content),
    tool: None,
};
let decision = Decider::new(xtf_standard::client(None)).decide(&manifest, &projection)?;
xtf_standard::decide::log(&root, &projection, &decision)?;
if decision.verdict.action == Action::Suggest {
    println!("the standard suggests {:?}", decision.verdict.suggested_path);
}
```

(The same code is the crate's doc example and is compiled by `cargo test`.)

`catalog` builds the questions, `policy` turns answers into verdicts,
`naming` checks and converts names; every module is documented in its source.

## Limitations

- **Accuracy of the default model.** The 2B model is right a little more
  often than not overall; the confidence gate is what makes it usable.
  Tests, examples and data files that are only recognisable by their folder
  are its weakest cases.
- **Latency.** A placement question takes about 3.2 s on an 8-core CPU (v3
  calibration: 356.6 s for 111 questions sent five to a request; each is
  read three times, once per rotation of its options). Questions about one
  project share a cached state, so the first decision on a project costs
  more than the next.
- **One request at a time.** `jevbase serve` answers requests in turn, so
  sessions deciding at once queue behind each other (three at once: 4.2 to
  5.7 s each). A hook that waits past 15 s lets its operation through
  undecided; that takes about five decisions arriving together, or one
  behind a long `plan_tree` (requests carry at most 8 questions, so none
  holds the server longer than about 10 s).
- **Folders are judged by name.** `file.root` sees a new path's name and
  summary, not the folder it is about to go in, and the folders decided so
  far (with only a name and the shell command's description to go on) all
  stayed under 0.5 confidence.
- **Local sessions only.** Claude Code's cloud sessions do not read the
  local `~/.claude`, and the server listens on 127.0.0.1.
- **Text only.** Images and binary files are not read.
- **The layout is yours to write.** `xtf init` maps common folder names; a
  manifest is only as good as its layout.

## Roadmap

- A decision server that serves several requests at once, each project's
  state cached in its own sequence (XKS_Jev-like_Model_Base).
- A placement question that sees where the path is going, and richer
  evidence for new folders, so folder decisions can reach the confidence
  the policy acts on (a new standard version, recalibrated).
- Calibration against more repositories and other decision models.

## License

MIT or Apache-2.0, at your option. See [NOTICE](NOTICE).
