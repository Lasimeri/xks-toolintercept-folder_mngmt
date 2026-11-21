# Standard xks-folder/1

How an agent's folder and naming decisions are asked of a Jev-like model,
the same way in every project. Implemented by `crates/xtf-standard` (the
library and the `xtf` command line); the MCP server and the Claude Code hook
on the roadmap will only call it.

## The decision server: any System One server

Every question of this standard is an ordinary System One request (`POST
/v1/systemone`, as TypeSafe documents it: OpenAPI 0.2.0,
https://docs.typesafe.ai/api/system-one, read 2026-11-21): the manifest's
rendering as the `state`, the catalog's questions as `questions`, a `model`
name. So the server answering it can be any System One server:

- `jevbase serve` (XKS_Jev-like_Model_Base, the default: a local Jev-like
  model on the CPU, checked against TypeSafe's documented contract);
- Intel-Phi-Jev's `xks serve`;
- TypeSafe's hosted Jev (`https://api.typesafe.ai`).

The tools are configured as TypeSafe's SDKs are: `TYPESAFE_BASE_URL`
(default `http://127.0.0.1:8097`), `TYPESAFE_API_KEY`,
`TYPESAFE_DEFAULT_MODEL` (default `jev-latest`). The answers they read are
the documented answer objects (`noul`; `choice`, `confidence`,
`probabilities`; `score`, `legend`, `confidence`, `probabilities`), and every
decision record keeps them verbatim. The thresholds the policy applies follow
TypeSafe's guidance on confidence (https://docs.typesafe.ai/confidence.md):
a Choice under 0.5 confidence is "genuinely unsure" and is not acted on; a
Noul reads yes above 0.7 and no below 0.3, the band between its own outcome.

The calibration below was measured with `jevbase serve` (Qwen3.8 2B Q8_0).
A different model behind the same wire answers differently: re-run
`make calibrate` against it before trusting the thresholds.

## What the model can and cannot do

The model behind `jevbase serve` (Qwen3.8 2B Q8_0 by default) never writes:
it picks one of the options it is shown, or gives P(yes). It answers about
one question a second on an 8-core CPU, and its confidence is informative
(see Calibration). So:

1. Everything a rule can settle is settled by a rule, with no question.
2. Every question has a stable id, fixed wording and options drawn from
   fixed sets (the roles, the kinds) or from candidates the calling agent
   supplies. Names and paths are proposed by the agent; the model picks.
3. An answer is acted on only when it is confident; otherwise the verdict
   says the model is unsure and nothing is suggested.
4. Every decision is written down, so a project's decisions can be read
   back, audited and used to re-calibrate.

## The documents

### Provided: the manifest, `xks-project.json` at the project root

This repository's own:

```json
{
  "standard": "xks-folder/1",
  "name": "xks-toolintercept-folder_mngmt",
  "purpose": "The xks-folder/1 standard, its Rust library and the xtf command line: they ask a Jev-like model, through any System One server, where a project's new files and folders belong and what to name them. An MCP server and a Claude Code hook are planned on top.",
  "languages": ["rust"],
  "layout": {
    "source": ["crates"],
    "docs": ["docs"],
    "data": ["calibration"],
    "out": ["out"]
  },
  "naming": { "files": "snake_case", "dirs": "any" },
  "ignore": [".git", "target", "node_modules", ".xks"]
}
```

- `layout` maps roles (below) to the project's own directories; a role may
  have several (`"source": ["src", "cli", "tui"]`), a directory may be nested
  (`"src/tui"`; the longest match wins). Files directly in the root play the
  `config` role, which is never listed.
- `naming.files` governs the project's own source files only (`.rs` in Rust,
  `.c`/`.h` in C, ...); scripts, documents and data keep their names.
  `naming.dirs` governs new folders; `any` leaves them free (Rust keeps crate
  folders in kebab-case and module folders in snake_case side by side).
  Styles: `kebab-case`, `snake_case`, `PascalCase`, `camelCase`,
  `lowercase`, `any`. Defaults by first language: Rust, C, C++, Python, Go,
  Zig: files `snake_case`, folders `any`; everything else `kebab-case`.
- `kind` (optional) is one of the kinds below, given or decided once.
- `ignore`, `out` and `third_party` paths are never decided on.

`xtf init` writes a manifest, mapping existing top-level folders to roles by
their names. The rendering of the manifest (name, purpose, languages, kind,
layout; `Manifest::state`) is the **state** of every question about the
project. It does not change between questions, so the server keeps it cached
and each decision costs only its own question.

### Projected: one operation about to happen

```json
{ "op": "create_file", "path": "crates/xtf-mcp/src/tools.rs",
  "summary": "//! The MCP tools: place_file, pick_name, ...", "tool": "Write" }
```

`op` is `create_file`, `create_dir`, `move`, `rename` or `delete`; `path` is
relative to the project root (for a move: the new path, with `from`);
`summary` is what it holds: the agent's description, or the start of the
content being written (`summary_of`, at most 240 characters, since every
character is read again for each question and rotation).

### Decision record: `.xks/decisions.jsonl` under the project root

One line per decision: the standard, the UTC time, the projection, the
verdict (action, notes, suggested path, chosen role, confidence), the
model's answers by question id, and the seconds the request took.

## Roles

The options of every placement question, in this order, worded exactly so:

| key | description (the model reads this) |
|---|---|
| source | the program's own source code |
| include | public header files for C or C++ code |
| tests | tests, test fixtures and test runners |
| benches | benchmarks and the inputs they run on |
| examples | examples and sample inputs for people to try |
| docs | documentation, design notes and written-up results |
| tools | helper scripts and programs for building, running or analysing the project |
| integrations | files that connect the project to other software: plugins, hooks, service units, other tools' settings |
| data | input data the project reads: datasets, reference files, evidence |
| assets | images, fonts and other static files the program ships |
| third_party | code fetched or copied from other projects |
| out | generated output: builds, logs, caches, run results (never tracked) |
| config | project-level files at the top: build manifest, README, licences, ignore files, settings |

A question offers the roles of the project's layout plus `out` and `config`,
narrowed for a file by two fixed rules of the standard (`roles.rs`):

- **its name**, when a convention shared across languages gives the role:
  `test_*`, `*_test.*`, `*_tests.*`, `*.test.*`, `*_spec.*`, `*.spec.*` are
  `tests`; `bench_*`, `*_bench.*`, `*.bench.*` are `benches` (when the layout
  has that role, the rule decides and nothing is asked);
- **its extension**, which rules roles out: program source can be source,
  tests, benches, examples, tools or integrations; headers source, include or
  tests; scripts tools, tests, integrations or examples; writing docs, config
  or examples; structured data config, data, examples, tests, integrations
  or benches; plain text docs, data, tests or examples; pictures and fonts
  assets or docs; service units integrations; a `.log` only `out`. `out` is
  always offered. One role left: the rule decides, nothing is asked.

## Question catalog

| id | type | asked when | wording (state: the manifest) |
|---|---|---|---|
| `file.root` | choice over roles | a new file or folder | "A new file is about to be added to this project. Name: N. Beginning of its content: S. Which part of the project does it belong in?" |
| `file.name_clear` | noul | a new file (optional) | "... Does the name tell a reader what the file holds?" |
| `name.pick` | choice over the agent's candidates | the agent asks for a name | "A name is needed for: W. Which name fits best: short, says what it holds, and reads like the project's other names?" |
| `project.kind` | choice over kinds | a new manifest without `kind` | "What kind of project is this?" |

Kinds: library, cli-tool, service, interactive-app, website,
firmware-driver, research, data-pipeline, documents, configuration, mixed
(descriptions in `catalog::KINDS`).

Wording, option sets and their order are part of the standard: changing any
of them changes the answers, so it makes a new standard version.

## Rules (no model)

In order, for each projected operation:

1. A deletion: allow.
2. A path under `ignore`, `out` or `third_party`: allow.
3. A conventional file in the root (README, LICENSE, NOTICE, Makefile,
   Cargo.toml, CLAUDE.md, dotfiles, ...): allow.
4. A governed name that breaks the naming style: the converted name is
   suggested (and the questions are still asked for the place).
5. A path under a top-level folder the layout does not map: noted.

## Policy (answers into actions)

- `file.root` chose the role the path already plays: allow.
- It chose another role with Choice confidence at or above **0.5**
  (`root_min_confidence`, TypeSafe's floor for "genuinely unsure", and the
  calibrated value below): suggest `<that role's first directory>/<name>`.
- Below it: uncertain; nothing is suggested.
- `file.name_clear` P(yes) at or below **0.3** (TypeSafe's "no" threshold):
  uncertain, and the agent is told to offer candidates to `name.pick`.

Actions are `allow` < `uncertain` < `suggest`; the strongest wins. How a tool
acts on them (advise or enforce) is the tool's setting, not the standard's.

## Calibration

`calibration/make-set.sh REPOS_DIR` draws a labelled set from checkouts of
real repositories (their manifests in `calibration/manifests/` declare, by
hand and before any run, the role each top-level folder plays) and `xtf
calibrate SET --repos REPOS_DIR` (`make calibrate REPOS=REPOS_DIR`) asks
`file.root` for every file the rules do not settle, reporting accuracy and,
for each confidence threshold, how often a confident answer would have moved
a file that was already where it belongs. The committed set is
`calibration/file-root.jsonl` (repositories named by folder, so it runs on
any machine that has them checked out); results: `calibration/results-*.txt`.

Measured 2026-11-21, `jevbase serve` with Qwen3.8 2B Q8_0 on a Ryzen 7 5800X
(8 threads), 141 real files of six repositories (Intel-Phi-Jev,
Mechanical-Jev, XKS_Jev-like_Model_Base, XKS-Memory, xks-id, x1tt3r): 23
conventional top-level files are settled by the rules; the other 118 are
counted below, those a name or extension rule leaves one role for included
(settled without asking). Speed is items per second of the run's wall time.

| | v1: every role offered | v2: options narrowed by extension | v3 (0.1.0): v2 + test and benchmark file names |
|---|---|---|---|
| settled by a name or extension rule | 0 | 2 | 7 |
| right overall | 61 of 118 (51.7%) | 70 of 118 (59.3%) | 72 of 118 (61.0%) |
| time | 533.8 s (0.22 decisions/s) | 417.6 s (0.28 decisions/s) | 356.6 s (0.33 decisions/s) |
| confidence >= 0.3: answered / right / would move a right file | 47.5% / 78.6% / 10.2% | 55.9% / 75.8% / 13.6% | 55.9% / 78.8% / 11.9% |
| confidence >= 0.4 | 38.1% / 82.2% / 6.8% | 41.5% / 83.7% / 6.8% | 42.4% / 86.0% / 5.9% |
| **confidence >= 0.5** | 25.4% / 80.0% / 5.1% | 32.2% / 89.5% / 3.4% | **33.1% / 92.3% / 2.5%** |
| confidence >= 0.6 | 12.7% / 80.0% / 2.5% | 22.9% / 88.9% / 2.5% | 24.6% / 89.7% / 2.5% |
| confidence >= 0.7 | | | 10.2% / 91.7% / 0.8% |

v3 against v2, file by file: the name rule settles five more files, the
three `bench_*` files (right in v2 as well) and two tests (`test_search.c`,
`cal_test.c`, which v2 placed in `source`). The 111 files asked in both runs
get the same choice; their confidence differs by at most 0.0255. (The
requests are built five questions at a time from the files the rules leave,
so the batches differ between the runs; that this is the cause of the
difference was not isolated.)

Per role in v3: source 31 of 38, docs 11 of 19, tools 7 of 14,
integrations 6 of 8, out 6 of 6, benches 3 of 4, examples 3 of 12, tests 2
of 12 (the other ten are tests only by their folder: golden files, scripts,
fixtures), data 1 of 3, config 1 of 1, include 1 of 1.

What it means: the 2B model is right about where a file belongs a little
more often than not, and its confidence says when it is: at 0.5 and above,
36 of 39 answers are right and 3 of the 118 files that were already in the
right place (one in forty) would be flagged. Below 0.5 the standard does not
act. A question costs about 3.2 s on this CPU (356.6 s for the 111 asked in
v3), so the tools ask only for new paths, batch questions about one project
into one request where they can, and keep the manifest (the state) unchanged
so the server reuses it.
