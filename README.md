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

**Status: 0.1.0, the first release.** It contains the standard
([xks-folder/1](SPEC.md)), its implementation as a Rust library and the
`xtf` command line, and its calibration on six real repositories. The MCP
server and the Claude Code hook that put it in front of an agent are the next
step ([Roadmap](#roadmap)); they are not in this release.

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
- **Text only.** Images and binary files are not read.
- **The layout is yours to write.** `xtf init` maps common folder names; a
  manifest is only as good as its layout.

## Roadmap

- `xtf-mcp`: an MCP server so the agent asks before it acts (place a file,
  plan a whole tree in one request, pick a name among its candidates).
- `xtf-hook`: a Claude Code hook that intercepts tool calls creating files
  and folders, advises the agent, or stops the call when the standard says
  the file belongs elsewhere.
- Calibration against more repositories and other decision models.

## License

MIT or Apache-2.0, at your option. See [NOTICE](NOTICE).
