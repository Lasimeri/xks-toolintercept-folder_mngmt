# xtf-mcp: the MCP server

`xtf-mcp` puts the xks-folder/1 standard (SPEC.md) behind six MCP tools,
so a coding agent asks where new files and folders belong, what to name
them, and how to lay out a new project, and gets the standard's verdicts.
Every model question goes to one System One server (by default `jevbase
serve` on http://127.0.0.1:8097), the same one the Claude Code hook uses.

| tool | what it does | model questions | writes |
|---|---|---|---|
| `new_project` | sets up a new project folder: kind, manifest, folder layout | `project.kind`, unless `kind` is given | `xks-project.json` (never over an existing one), the folders with `create_dirs` (mkdir only), the kind decision |
| `plan_tree` | places 1 to 64 planned files and folders in one call, returns the resulting tree | `file.root` for each path the rules leave open, batched | one log line per planned path |
| `place_file` | one new, moved or renamed file or folder | `file.root`, `file.name_clear` | one log line |
| `extend_layout` | adds folders to the layout under one role | none (the caller names the role) | the manifest's layout (never a mapping removed), the folders with `create_dirs`, one log line per added folder |
| `pick_name` | the best of 2 to 26 candidate names | `name.pick` | nothing |
| `project_info` | the nearest manifest and the size of its decision log | none | nothing |

Decisions are appended to `.xks/decisions.jsonl` under the project root, in
the format of SPEC.md ("Decision record").

The usual order: `new_project` sets up the folder and its layout;
`plan_tree` places the first set of files; its `layout_gaps` name the
planned paths no layout folder holds (a `scripts/` folder, a script in the
root), and `extend_layout` adds the folder they need; `place_file` checks a
later file on its own.

## Running it

```sh
cargo build --release -p xtf-mcp     # target/release/xtf-mcp
```

The binary speaks MCP over stdio: JSON-RPC 2.0, one message per line on
standard input and standard output. Claude Code starts it from the plugin's
`.mcp.json`; by hand:

```sh
claude mcp add xtf -- ~/.local/bin/xtf-mcp
```

| | |
|---|---|
| methods | `initialize`, `ping`, `tools/list`, `tools/call`; notifications (`notifications/initialized`, `notifications/cancelled`, ...) are never answered; any other method: error -32601; a line that is not JSON: -32700; a `tools/call` of an unknown tool or with `arguments` that are not an object: -32602; a batch (JSON array) is answered with an array |
| protocol versions | 2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05: the client's when it asks for one of these, otherwise 2025-11-25 |
| capabilities | `{"tools": {}}`, `serverInfo.name` `xtf`, and `instructions` (below) |
| tool failures | a result with `isError: true` and a plain message (no manifest, arguments that do not fit, the decision server down, refusing or too slow), never a crash; a panic inside a tool is caught and reported the same way |
| concurrency | every `tools/call` runs on its own thread: `ping` and `tools/list` are answered while a decision waits for the model (measured: a `ping` sent after a `place_file` was answered first) |
| end of input | the calls in progress finish (each is bounded, below), their replies are written, exit status 0 |
| standard error | one line per tool call: `xtf-mcp: place_file 3.068 s`, `(isError)` appended on failure |
| relative paths | resolved against the server's working directory; `~/` against `HOME` |

Environment:

| variable | default | meaning |
|---|---|---|
| `TYPESAFE_BASE_URL` | `http://127.0.0.1:8097` | the System One server (`jevbase serve`, Intel-Phi-Jev's `xks serve`, or https://api.typesafe.ai) |
| `TYPESAFE_API_KEY` | none | sent as `Authorization: Bearer` |
| `TYPESAFE_DEFAULT_MODEL` | `jev-latest` | the `model` of every request |
| `XTF_MCP_TIMEOUT` | 20 s + 8 s a question | seconds one request may take (the read timeout); the call gives up 3 s after it |

The `initialize` result (recorded):

```json
{
  "protocolVersion": "2025-11-25",
  "capabilities": {
    "tools": {}
  },
  "serverInfo": {
    "name": "xtf",
    "version": "0.2.0"
  },
  "instructions": "xtf answers folder and naming questions for projects that follow the xks-folder/1 standard: an xks-project.json manifest at the project root maps fixed roles (source, tests, docs, tools, data, out, ...) to the project's folders. Rules settle what needs no model; a local Jev-like model on a System One server (default http://127.0.0.1:8097, one shared instance) answers the rest (1 to 4 s a call), and answers under 0.5 confidence are reported as unsure and not acted on. Tools: new_project sets up a new project folder (decides its kind, writes the manifest with a conventional folder layout, can create the folders); plan_tree places a whole set of planned files and folders in one call and returns the resulting tree; place_file checks where one new, moved or renamed file or folder belongs; both report layout gaps (planned paths outside every layout folder), which extend_layout closes by adding a folder to the layout under a role; pick_name chooses among candidate names; project_info reads the nearest manifest. They help when starting a project or a folder structure, when a new file's place is not obvious, and when choosing a name. Every placement is logged in the project's .xks/decisions.jsonl."
}
```

With Claude Code's tool search only these instructions and the tool names
reach Claude at the start of a session; a tool's description and schema
load when a search finds it.

## Tools

### new_project

Sets up a new project folder. Without `kind`, the model chooses one of
the eleven kinds (`project.kind`) on a state of the project's name,
purpose and languages; a choice under 0.5 confidence is not acted on: the
manifest then leaves `kind` unset and starts from the general layout (the
one of `mixed`). The layout comes from a fixed table,
`xtf_standard::scaffold` (no model; the same kind and languages always give
the same tree): the roles of the kind (every kind has `docs` and `out`),
in the folders the first language uses (Rust `src` `tests` `benches`; C
`src` `include` `bench` `build`; Python `src` `tests` `benchmarks`
`scripts`; Go `cmd` `internal`, no tests folder; TypeScript and JavaScript
`src` `test` `public` `dist`; Java and Kotlin `src/main` `src/test`). The
manifest gets the languages' naming styles and `.xks` in `ignore`. The
folder is created when missing; with `create_dirs` the layout's folders are
too (create only: nothing is removed or overwritten). An existing
`xks-project.json` is returned unchanged:

```text
xks-folder/1: ~/work/new-rust-cli already holds xks-project.json (project new-rust-cli, kind cli-tool); it is returned unchanged and nothing was written
```

| argument | type | required | meaning |
|---|---|---|---|
| `dir` | string | yes | The project folder (absolute, ~/..., or relative to the server's working directory); created when missing. |
| `purpose` | string | yes | One or two sentences: what the project is for. The model reads it. |
| `languages` | array | yes | Programming languages, the main one first (e.g. ["rust"], ["c", "python"]); [] for none. The first sets folder and naming conventions. |
| `name` | string | no | Project name; default: the folder's name. |
| `kind` | string: library, cli-tool, service, interactive-app, website, firmware-driver, research, data-pipeline, documents, configuration, mixed | no | The project kind; when omitted the model decides it. |
| `create_dirs` | boolean | no | Also create the layout's folders (mkdir only). |

Hints: readOnlyHint false, destructiveHint false, idempotentHint true, openWorldHint false.

Recorded (the model chose the kind; `create_dirs: true`):

```text
xks-folder/1: wrote ~/work/new-rust-cli/xks-project.json; kind cli-tool (decided by the model, confidence 0.90); folders docs/ out/ src/ tests/ (4 created)
```

```json
{
  "standard": "xks-folder/1",
  "root": "~/work/new-rust-cli",
  "manifest": {
    "standard": "xks-folder/1",
    "name": "new-rust-cli",
    "purpose": "A command-line program that converts a folder of FLAC files to Opus in parallel and keeps the folder structure.",
    "languages": [
      "rust"
    ],
    "kind": "cli-tool",
    "layout": {
      "source": [
        "src"
      ],
      "tests": [
        "tests"
      ],
      "docs": [
        "docs"
      ],
      "out": [
        "out"
      ]
    },
    "naming": {
      "files": "snake_case",
      "dirs": "any"
    },
    "ignore": [
      ".git",
      "target",
      "node_modules",
      ".xks"
    ]
  },
  "kind": {
    "kind": "cli-tool",
    "layout_from": "cli-tool",
    "by": "model",
    "confidence": 0.8965,
    "probabilities": {
      "library": 0.0248,
      "cli-tool": 0.9059,
      "service": 0.0099,
      "interactive-app": 0.0136,
      "website": 0.0286,
      "firmware-driver": 0.0022,
      "research": 0.0013,
      "data-pipeline": 0.0015,
      "documents": 0.0012,
      "configuration": 0.0041,
      "mixed": 0.0068
    },
    "acted_on": true,
    "seconds": 3.203
  },
  "tree": [
    {
      "folder": "docs/",
      "state": "created"
    },
    {
      "folder": "out/",
      "state": "created"
    },
    {
      "folder": "src/",
      "state": "created"
    },
    {
      "folder": "tests/",
      "state": "created"
    }
  ],
  "created": [
    "./",
    "docs/",
    "out/",
    "src/",
    "tests/"
  ],
  "inside_project": null,
  "logged": ".xks/decisions.jsonl"
}
```

The same session, other purposes: a Python project ("Nightly jobs that
download weather station CSV files, clean them and load them into a
Postgres database in stages") was decided `data-pipeline` (confidence
0.59) with `data/ docs/ out/ src/ tests/`; a TypeScript one ("The public
website of a bakery") `website` (0.72) with `dist/ docs/ public/ src/
test/`; a Go `service` given in the call got `cmd/ docs/ integrations/
internal/ out/`.

### plan_tree

Places a whole set of planned paths of one project before they are
written: the rules first (deletions, generated and vendored paths,
conventional top-level files, the naming style, the name and extension
rules), then `file.root` for the rest, in requests on the project's manifest
of at most 8 questions each. Returns each verdict, the resulting tree (the
paths after the suggestions, with their parent folders) and any final path
claimed by more than one planned path. `file.name_clear` is not asked here
(one question per path keeps a plan affordable); `place_file` asks it.

| argument | type | required | meaning |
|---|---|---|---|
| `root` | string | yes | The folder the planned paths are relative to (absolute, ~/..., or relative to the server's working directory); the manifest is found at or above it. |
| `files` | array of {path, summary} (1 to 64) | yes | The planned paths: each a path relative to root (ending in / for a folder) and a summary of what it will hold. |

Hints: readOnlyHint false, destructiveHint false, idempotentHint true, openWorldHint false.

Recorded (12 planned paths of this repository; its layout maps `source`,
`docs`, `data`, `out`):

```text
xks-folder/1 plan of 12 paths in xks-toolintercept-folder_mngmt: 6 allow, 1 suggest, 5 uncertain; 3 settled by the rules, 9 asked in 1 request (16.0 s)
```

```json
{
  "standard": "xks-folder/1",
  "root": "~/work/repo",
  "project": "xks-toolintercept-folder_mngmt",
  "files": [
    {"path":"crates/xtf-mcp/src/rpc.rs","by":"model","action":"allow","notes":["it belongs in source (confidence 0.44)"],"role":"source","confidence":0.4447,"final":"crates/xtf-mcp/src/rpc.rs"},
    {"path":"crates/xtf-mcp/src/ask.rs","by":"model","action":"allow","notes":["it belongs in source (confidence 0.36)"],"role":"source","confidence":0.3646,"final":"crates/xtf-mcp/src/ask.rs"},
    {"path":"crates/xtf-mcp/tests/stdio_session.rs","by":"model","action":"uncertain","notes":["the model leans to out but is unsure (confidence 0.14): left as is"],"role":"out","confidence":0.1403,"final":"crates/xtf-mcp/tests/stdio_session.rs"},
    {"path":"crates/xtf-mcp/benches/bench_latency.rs","by":"model","action":"allow","notes":["it belongs in source (confidence 0.08)"],"role":"source","confidence":0.0831,"final":"crates/xtf-mcp/benches/bench_latency.rs"},
    {"path":"docs/mcp.md","by":"model","action":"uncertain","notes":["the model leans to out but is unsure (confidence 0.22): left as is"],"role":"out","confidence":0.216,"final":"docs/mcp.md"},
    {"path":"crates/latency-notes.md","by":"model","action":"uncertain","notes":["the model leans to out but is unsure (confidence 0.21): left as is"],"role":"out","confidence":0.2105,"final":"crates/latency-notes.md"},
    {"path":"calibration/results-v4.txt","by":"model","action":"uncertain","notes":["the model leans to out but is unsure (confidence 0.43): left as is"],"role":"out","confidence":0.434,"final":"calibration/results-v4.txt"},
    {"path":"tools/gen_session.sh","by":"rules","action":"suggest","notes":["`tools` is not a directory of the project's layout","it belongs in out (out/), not where it is going (confidence 1.00)"],"suggested_path":"out/gen_session.sh","role":"out","confidence":1.0,"final":"out/gen_session.sh"},
    {"path":"integrations/claude-code/plugin/.mcp.json","by":"model","action":"uncertain","notes":["`integrations` is not a directory of the project's layout","the model leans to config but is unsure (confidence 0.19): left as is"],"role":"config","confidence":0.1854,"final":"integrations/claude-code/plugin/.mcp.json"},
    {"path":"out/session.log","by":"rules","action":"allow","notes":["the path is generated or vendored: not decided on"],"final":"out/session.log"},
    {"path":"CHANGELOG.md","by":"rules","action":"allow","notes":["a conventional top-level file"],"final":"CHANGELOG.md"},
    {"path":"crates/xtf-mcp/src","by":"model","action":"allow","notes":["it belongs in source (confidence 0.31)"],"role":"source","confidence":0.308,"final":"crates/xtf-mcp/src/"}
  ],
  "tree": [
    "CHANGELOG.md",
    "calibration/",
    "calibration/results-v4.txt",
    "crates/",
    "crates/latency-notes.md",
    "crates/xtf-mcp/",
    "crates/xtf-mcp/benches/",
    "crates/xtf-mcp/benches/bench_latency.rs",
    "crates/xtf-mcp/src/",
    "crates/xtf-mcp/src/ask.rs",
    "crates/xtf-mcp/src/rpc.rs",
    "crates/xtf-mcp/tests/",
    "crates/xtf-mcp/tests/stdio_session.rs",
    "docs/",
    "docs/mcp.md",
    "integrations/",
    "integrations/claude-code/",
    "integrations/claude-code/plugin/",
    "integrations/claude-code/plugin/.mcp.json",
    "out/",
    "out/gen_session.sh",
    "out/session.log"
  ],
  "collisions": [],
  "requests": [
    {
      "questions": 9,
      "seconds": 15.989,
      "input_tokens": 3003,
      "bytes": 4745
    }
  ],
  "logged": ".xks/decisions.jsonl"
}
```

`tools/gen_session.sh` shows the one-role rule at work in a layout without
a tools, tests, integrations or examples folder: `out` is the only role
left for a script, so the rule decides it (see Known problems).

#### Layout gaps

`layout_gaps` (also in every `place_file` reply, `[]` when there is none)
lists the planned paths that lie outside every folder of the layout, one
entry per top-level folder or root file. They are facts read from the
manifest and the standard's extension table, not decisions:

- `unmapped_folder`: a new top-level folder, or paths under a top-level
  folder the layout does not map (`scripts/`);
- `root_file`: a file in the project root whose type is not one a root
  holds (a script, a source file; conventional files such as `README.md`
  or `Cargo.toml`, configuration files and names without a known type are
  not gaps).

Each entry gives the planned `paths` in it, `layout_lacks` (the roles their
names or extensions make plausible that the layout has no folder for),
`layout_has` (those it has, with their folders) and `next`, the step that
closes it: `extend_layout` with the role, or planning the paths in a folder
the layout has. The summary line ends with `layout gaps ... (see
layout_gaps)` when there are any.

Recorded (4 planned paths in `flacopus`, a Rust command-line tool whose
layout maps `source`, `tests`, `docs`, `out`):

```text
xks-folder/1 plan of 4 paths in flacopus: 1 allow, 1 suggest, 2 uncertain; 0 settled by the rules, 4 asked in 1 request (9.7 s); layout gaps bench.sh, scripts/ (see layout_gaps)
```

```json
[
  {"at":"bench.sh","kind":"root_file","paths":["bench.sh"],"layout_lacks":["examples","tools","integrations"],"layout_has":[{"role":"tests","dir":"tests/"}],"next":"bench.sh is not a file the project root holds; roles it plays that the layout lacks: examples (examples/), tools (tools/), integrations (integrations/): add one with extend_layout {role, dirs}, then plan it in that folder; or plan it in tests/"},
  {"at":"scripts/","kind":"unmapped_folder","paths":["scripts/","scripts/release.sh"],"layout_lacks":["examples","tools","integrations"],"layout_has":[{"role":"tests","dir":"tests/"}],"next":"the layout does not map scripts/: record the role it plays with extend_layout {role, dirs: [\"scripts\"]}; roles its planned files' types play that the layout lacks: examples, tools, integrations; or plan them in tests/ instead"}
]
```

In the same plan the model, offered only `tests` and `out` for the scripts,
chose `out` for both: `bench.sh` unsure (0.12), `scripts/release.sh`
confidently (0.83, a suggestion to move it to `out/release.sh`). After
`extend_layout` mapped `scripts/` as `tools`, `place_file` on `bench.sh`
leaned to `tools` (probability 0.64, confidence 0.46): the right role, once
the layout offers it.

### place_file

One projected operation through the library's `Decider`: the rules, then
`file.root` and `file.name_clear` in one request, then the policy. The
summary is `summary`, else the start of `content`, else the start of the
file at the path (or at `from` for a move or rename). The model reads the
file's name and summary, not its folder, so a file projected to a
different folder gets the same answer, and the policy compares the chosen
role with where the path goes. The reply's `layout_gaps` is that of
`plan_tree` for the one path.

| argument | type | required | meaning |
|---|---|---|---|
| `path` | string | yes | The path being created (for a move or rename: the new path); absolute, ~/..., or relative to the server's working directory. Ending in / means a folder. |
| `summary` | string | no | What it holds, a sentence. Default: the start of content, or of the existing file. |
| `content` | string | no | The content about to be written; its start (240 characters) is read. |
| `op` | string: create_file, create_dir, move, rename, delete | no | Default create_file (create_dir for a path ending in /). |
| `from` | string | no | For move and rename: the current path. |

Hints: readOnlyHint false, destructiveHint false, idempotentHint true, openWorldHint false.

Recorded (the content of `calibration/results-v3.txt`, projected to
`crates/results-v3.txt`):

```text
xks-folder/1: crates/results-v3.txt: suggest docs/results-v3.txt; it belongs in docs (docs/), not where it is going (confidence 0.61)
```

```json
{
  "standard": "xks-folder/1",
  "root": "~/work/repo",
  "project": "xks-toolintercept-folder_mngmt",
  "projected": {
    "op": "create_file",
    "path": "crates/results-v3.txt",
    "summary": "141 items: 23 settled by the rules (conventional top-level files), 118 asked\ngold\tchosen\tconfidence\tpath\ndocs\tdocs\t0.0251\tdocs/subprojects/01-bluebird-baseline.md\ndocs\tdocs\t0.1203\tdocs/subprojects/04-long-sessions.md\ndocs\tconfig\t0.3869\tdocs"
  },
  "verdict": {
    "action": "suggest",
    "notes": [
      "it belongs in docs (docs/), not where it is going (confidence 0.61)"
    ],
    "suggested_path": "docs/results-v3.txt",
    "role": "docs",
    "confidence": 0.6107
  },
  "answers": {
    "file.root": {
      "type": "choice",
      "choice": "docs",
      "probabilities": {
        "docs": 0.7405,
        "data": 0.0947,
        "out": 0.1648
      },
      "confidence": 0.6107
    },
    "file.name_clear": {
      "type": "noul",
      "noul": 0.6485
    }
  },
  "seconds": 3.59,
  "logged": ".xks/decisions.jsonl"
}
```

### extend_layout

Adds folders to the layout under one role, the step that closes a layout
gap. The role is the caller's: the one a gap's `layout_lacks` or a
verdict names, or the agent's own reading of the folder. Each folder is
checked first, and the call fails with a message (the manifest untouched)
when one is absolute or leaves the project, lies under an `ignore` entry,
is already another role's folder (a folder plays one role), exists as a
file, or breaks the project's folder naming style (the message names the
styled form). A folder that already plays the role is reported under
`already`; nothing is ever removed or moved. The manifest is reloaded,
changed and written under an exclusive lock on `.xks/layout.lock`, through
a temporary file renamed over it, so concurrent sessions never lose each
other's folders and a reader never sees half a file; the text is the one
`Manifest::save` writes, and an unchanged manifest is not rewritten. Each
added folder is one log line (`"tool": "extend_layout"`, action `allow`,
the role). No model question.

| argument | type | required | meaning |
|---|---|---|---|
| `path_or_root` | string | yes | A path inside the project (or its root); the manifest is found at or above it. |
| `role` | string: source, include, tests, benches, examples, docs, tools, integrations, data, assets, third_party, out | yes | The role the folders play. `config` is the project root itself and takes no folder. |
| `dirs` | array of strings (1 to 8) | yes | Folders relative to the project root, e.g. `["scripts"]` or `["src/tui"]`. |
| `create_dirs` | boolean | no | Also create the folders (mkdir only). Default false. |

Hints: readOnlyHint false, destructiveHint false, idempotentHint true, openWorldHint false.

Recorded (`flacopus` above, `scripts/` mapped as `tools`, 4 ms):

```text
xks-folder/1: layout of flacopus: tools -> scripts/ (added scripts/; 1 folder created)
```

```json
{
  "standard": "xks-folder/1",
  "root": "~/work/flacopus",
  "project": "flacopus",
  "role": "tools",
  "added": ["scripts/"],
  "already": [],
  "layout": {
    "source": ["src"],
    "tests": ["tests"],
    "docs": ["docs"],
    "tools": ["scripts"],
    "out": ["out"]
  },
  "folders": [{"folder": "scripts/", "state": "created"}],
  "logged": ".xks/decisions.jsonl"
}
```

(Arrays are shown on one line.) `logged` is `null` when nothing was added.

### pick_name

`name.pick` on the project's manifest: the candidates are the options, so
the model picks among names the agent proposes and never writes one. The
answer is acted on (`acted_on`) at a Choice confidence of 0.5 or more.
`style_notes` names candidates that break the project's file naming style
(a rule, no model).

| argument | type | required | meaning |
|---|---|---|---|
| `path_or_root` | string | yes | A path inside the project (or its root); the manifest is found at or above it. |
| `what` | string | yes | What needs the name, a sentence. |
| `candidates` | array (2 to 26) | yes | The candidate names. |

Hints: readOnlyHint true, openWorldHint false.

Recorded:

```text
xks-folder/1 name.pick: `scaffold.rs` (confidence 0.51, at or above 0.5: acted on)
```

```json
{
  "standard": "xks-folder/1",
  "root": "~/work/repo",
  "project": "xks-toolintercept-folder_mngmt",
  "what": "the Rust module that turns a project kind and language into a starting folder layout",
  "candidates": [
    "scaffold.rs",
    "layout_table.rs",
    "util.rs"
  ],
  "choice": "scaffold.rs",
  "confidence": 0.5072,
  "probabilities": {
    "scaffold.rs": 0.6715,
    "layout_table.rs": 0.1803,
    "util.rs": 0.1482
  },
  "acted_on": true,
  "style_notes": [],
  "seconds": 1.229
}
```

### project_info

The manifest nearest at or above a path, and the number of lines in its
decision log. No model; a path with no manifest above it is an answer
(`"manifest": null`), not an error.

| argument | type | required | meaning |
|---|---|---|---|
| `path` | string | yes | Any path inside the project, or its root. |

Hints: readOnlyHint true, openWorldHint false.

Recorded:

```text
xks-folder/1: project new-rust-cli at ~/work/new-rust-cli: kind cli-tool, languages rust, layout source src; tests tests; docs docs; out out; 1 decision in .xks/decisions.jsonl
```

```json
{
  "standard": "xks-folder/1",
  "root": "~/work/new-rust-cli",
  "name": "new-rust-cli",
  "purpose": "A command-line program that converts a folder of FLAC files to Opus in parallel and keeps the folder structure.",
  "kind": "cli-tool",
  "languages": [
    "rust"
  ],
  "layout": {
    "source": [
      "src"
    ],
    "tests": [
      "tests"
    ],
    "docs": [
      "docs"
    ],
    "out": [
      "out"
    ]
  },
  "naming": {
    "files": "snake_case",
    "dirs": "any"
  },
  "ignore": [
    ".git",
    "target",
    "node_modules",
    ".xks"
  ],
  "decisions": 1,
  "log": ".xks/decisions.jsonl"
}
```

## When the decision server is down, refusing or slow

Every request gets a read timeout of 20 s plus 8 s a question
(`XTF_MCP_TIMEOUT` fixes it) and runs on its own thread behind a hard
deadline 3 s past that. The read timeout alone is not exact: Linux runs a
socket timeout on its timer wheel, whose granularity grows with the
timeout (at HZ=1000: 64 ms up to 4 s, 512 ms up to 32 s, 4,096 ms up to 4.4
min), and a server that sends a byte now and then resets it; the deadline
is a futex wait on a high-resolution timer. Recorded results, as Claude
receives them:

- No manifest (`place_file`, 0.1 ms):

  ```text
  no xks-project.json at or above ~/work/no-manifest/notes.md: the xks-folder/1 decisions are made inside a project that has one; new_project writes one (purpose, languages, kind, folder layout)
  ```

- Server unreachable (`place_file`, `TYPESAFE_BASE_URL=http://127.0.0.1:9`, 0.3 ms):

  ```text
  no System One server answers at http://127.0.0.1:9 (127.0.0.1:9: Connection refused (os error 111)); the decisions come from it (by default `jevbase serve`, the jevbase.service user unit). Without the model, the rules note: `NewThing.rs` does not follow the project's naming style; `new_thing.rs` does.
  ```

- Server unreachable (`plan_tree`, 0.2 ms):

  ```text
  no System One server answers at http://127.0.0.1:9 (127.0.0.1:9: Connection refused (os error 111)); the decisions come from it (by default `jevbase serve`, the jevbase.service user unit). The rules settle 1 of the 3 planned paths; the other 2 need the model, so no verdicts are returned and nothing was logged.
  ```

- A server that sends a byte a second and never finishes (`pick_name`, `XTF_MCP_TIMEOUT=2`, 5000.3 ms):

  ```text
  the System One server at http://127.0.0.1:18763 gave no answer within 5 s (busy with other sessions' requests, or stalled)
  ```

- An argument the tool does not take (`place_file`):

  ```text
  unknown argument `summmary`; this tool takes path, summary, content, op, from
  ```

Measured bounds (every model-dependent tool, the server replaced as
stated; `project_info`, `new_project` with `kind`, and paths the rules
settle need no server and kept working):

| server | `XTF_MCP_TIMEOUT` | wall time per call |
|---|---|---|
| nothing listening (connection refused) | unset | 0.1 to 0.3 ms |
| listening, never accepting (a busy one-at-a-time server) | 2 | 2,047.9 to 2,061.8 ms |
| accepting, reading, never answering | 2 | 2,046.4 to 2,048.0 ms |
| answering one byte a second, never finishing | 2 | 5,000.3 ms (the deadline) |
| listening, never accepting | unset (`place_file`: 36 s) | 36,855.1; 37,453.9; 37,940.4; 39,000.3; 39,000.3 ms |

JSON-RPC errors (recorded):

```json
{"jsonrpc":"2.0","id":9,"error":{"code":-32601,"message":"method not found: resources/list"}}
{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error: EOF while parsing a value at line 1 column 34"}}
{"jsonrpc":"2.0","id":11,"error":{"code":-32602,"message":"unknown tool `rm_rf`; the tools are new_project, plan_tree, place_file, pick_name, project_info"}}
```

## Latency

Measured 2026-11-21 on an AMD Ryzen 7 5800X against `jevbase serve`
(Qwen3.8 2B Q8_0, 8 threads), the one decision server every Claude Code
session on the machine shares: a driver wrote one JSON-RPC line and timed
until the reply's newline, one call at a time. A request that arrives while
another session's request is being answered waits for it.

| call | n | wall time per call |
|---|---|---|
| `project_info` | 6 | 0.1 to 0.6 ms |
| `new_project`, `kind` given | 5 | 0.2 to 0.8 ms |
| `new_project`, the model decides the kind | 3 | 3,199.1; 3,203.3; 3,249.3 ms |
| `pick_name`, 3 candidates | 3 | 1,229.1; 1,245.9; 1,257.1 ms |
| `place_file`, the model asked (2 questions) | 11 | min 2,052.6, median 3,245.5, max 4,108.1 ms |
| `place_file`, `file.root` settled by a rule | 1 | 757.4 ms |
| `plan_tree`, 12 paths (9 asked, 1 request) | 1 | 15,990.5 ms |
| `plan_tree`, 34 paths (34 asked, 5 requests) | 1 | 41,950.8 ms (requests 9.4, 10.1, 10.2, 9.7, 2.5 s) |

`place_file` against the command line's `xtf check` on the same file
(the same request), interleaved: 3,092.1 / 2,104; 2,062.0 / 2,079; 6,588.1 /
2,059 ms. The server's own log gives the cause of the spread: 2,061 ms of
model work for the 2,062.0 ms call (the server adds about 1 ms); about 1.0 s
more when another session's request on a different state came just before
(the state is read again); and 3,378 ms waiting behind another session's
request in the 6,588.1 ms call.

Why at most 8 questions a request: `jevbase serve` answers one request at a
time, in the order they connect, and the hook gives up on a decision after
15 s. With 32, a 32-question request held the server 41.5 s (1.30 s a
question) and the two hook requests queued behind it were answered after
their clients had gone (the server logged `Broken pipe` twice). With 8 the
same 34 paths took 41.95 s in five requests of at most 10.2 s, the state
staying cached between them.

## Known problems

- When a project's layout has none of the roles a file's extension allows
  (a `.sh` script in a layout without tools, tests, integrations or
  examples), `out` is the only role left and the one-role rule suggests
  moving the file to `out/` at confidence 1.00: recorded for
  `calibration/make-set.sh` in this repository (`catalog::root_options`
  with `Decider`, in xtf-standard).
- `decide::log` writes one decision line in many small writes (84 for a
  250-byte line, counted with strace), so two processes appending to the
  same log at the same moment (the hook and this server) can interleave
  their lines. Within this server the appends are serialized.
- `notifications/cancelled` is not acted on: a cancelled call runs to its
  bound and its reply is written (a client ignores replies to cancelled
  requests).
- A request given up on at the deadline keeps its thread until the
  client's own timeouts end it, or the process exits.
