# `xtf` reference

`xtf` is the command line of the xks-folder/1 standard ([SPEC.md](../SPEC.md)).
It writes a project's manifest, decides single operations, and measures the
standard on labelled repositories. Every decision it asks is an ordinary
System One request to a decision server.

```
xtf [--server URL] <command> [args]
```

## The decision server

| variable | default | meaning |
|---|---|---|
| `TYPESAFE_BASE_URL` | `http://127.0.0.1:8097` | the System One server: `jevbase serve` (XKS_Jev-like_Model_Base), Intel-Phi-Jev's `xks serve`, or `https://api.typesafe.ai` |
| `TYPESAFE_API_KEY` | none | sent as `Authorization: Bearer KEY` (required by the hosted Jev) |
| `TYPESAFE_DEFAULT_MODEL` | `jev-latest` | the `model` every request names |

`--server URL` overrides `TYPESAFE_BASE_URL` for one run. These are the
variables TypeSafe's own SDKs read. `https://` servers are reached through
the `curl` program, which must be installed; the key is passed to curl on its
standard input, never on its command line.

`init` needs no server. `check` asks it only when the rules leave the
decision open. `calibrate` asks it for every item the rules do not settle.

## `xtf init`

```
xtf init [DIR] --purpose TEXT [--name NAME] [--lang LANG]... [--force]
```

Writes `DIR/xks-project.json` (DIR defaults to the current directory):

- `name`: `--name`, else the folder's name;
- `purpose`: one or two sentences, what the project is for (it is part of
  every question's state, so it is worth writing well);
- `languages`: each `--lang` in order (the first sets the naming styles:
  Rust, C, C++, Python, Go, Zig name source files in snake_case; anything
  else in kebab-case);
- `layout`: the existing top-level folders mapped to roles by their names
  (`src`, `crates`, `cli`, `tui` are source; `tests`, `test`, `spec` tests;
  `scripts`, `tools` tools; `docs`, `reports`, `notes` docs; and so on);
  folders with other names are listed on standard error, to be added to the
  layout by hand. With no recognised folder the layout is the default one:
  `src`, `tests`, `docs`, `tools`, `out`.

An existing manifest is never replaced without `--force`. Review the written
file: the layout is the one thing the standard cannot guess.

Example, in a checkout of XKS_Jev-like_Model_Base (every top-level folder is
recognised, so nothing is listed on standard error; the manifest written is
also printed):

```
$ xtf init --purpose "Jev-like typed judgments from a local LLM on the CPU: a Rust crate and binary over a pinned llama.cpp." --lang rust
```

```json
{
  "standard": "xks-folder/1",
  "name": "XKS_Jev-like_Model_Base",
  "purpose": "Jev-like typed judgments from a local LLM on the CPU: a Rust crate and binary over a pinned llama.cpp.",
  "languages": ["rust"],
  "layout": {
    "source": ["csrc", "src"],
    "tests": ["tests"],
    "examples": ["examples"],
    "third_party": ["third_party"],
    "out": ["out"]
  },
  "naming": { "files": "snake_case", "dirs": "any" },
  "ignore": [".git", "target", "node_modules"]
}
```

(The run was in a folder named after its commit; `name` is the folder's
name, shown here as the repository's. Arrays are shown on one line.)

## `xtf check`

```
xtf check PATH [--op OP] [--summary TEXT | --content FILE]
```

Decides one projected operation: PATH is the file or folder about to be
created (absolute, or relative to the current directory), in the project
whose manifest is the nearest `xks-project.json` at or above it.

- `--op`: `create_file` (default), `create_dir`, `move`, `rename`, `delete`.
- What it holds: `--summary TEXT`, or the start of `--content FILE`, or, when
  neither is given and PATH exists, the start of PATH itself (at most 240
  characters are used).

It prints `{"projected": ..., "decision": ...}`: the projection, the verdict
(`action`: `allow`, `uncertain` or `suggest`; `notes`; `suggested_path`;
`role` and `confidence` when the model was asked), the model's answers
exactly as the server returned them, and the seconds the request took. The
decision is appended to `.xks/decisions.jsonl` under the project root.

Examples, in this repository, against `jevbase serve` (Qwen3.8 2B Q8_0, 8
threads; recorded 2026-11-21, the output shown through `jq`).

A file where it belongs. The summary is the start of the file itself:

```
$ xtf check crates/xtf-standard/src/naming.rs
```

```json
{
  "projected": {
    "op": "create_file",
    "path": "crates/xtf-standard/src/naming.rs",
    "summary": "//! Naming styles: a rule, never a model question. The file style governs\n//! the project's own source files (`.rs` in a Rust project, `.c` and `.h` in\n//! a C project, ...), not its scripts, documents or data; the folder style\n//! governs "
  },
  "decision": {
    "verdict": {
      "action": "allow",
      "notes": ["it belongs in source (confidence 0.74)"],
      "role": "source",
      "confidence": 0.7383
    },
    "answers": {
      "file.root": {
        "type": "choice",
        "choice": "source",
        "probabilities": { "source": 0.8692, "out": 0.1308 },
        "confidence": 0.7383
      },
      "file.name_clear": { "type": "noul", "noul": 0.5876 }
    },
    "seconds": 3.23621906
  }
}
```

A design note about to be written among the sources. The extension leaves
`docs`, `config` and `out`; the model leans to `docs`, but below the 0.5
confidence the policy acts on, so nothing is suggested:

```
$ xtf check crates/xtf-standard/src/how_the_catalog_was_worded.md \
    --summary "Design notes: why each question of the catalog is worded as it is, and the answers that changed when the wording did."
```

```json
{
  "projected": {
    "op": "create_file",
    "path": "crates/xtf-standard/src/how_the_catalog_was_worded.md",
    "summary": "Design notes: why each question of the catalog is worded as it is, and the answers that changed when the wording did."
  },
  "decision": {
    "verdict": {
      "action": "uncertain",
      "notes": ["the model leans to docs but is unsure (confidence 0.31): left as is"],
      "role": "docs",
      "confidence": 0.307
    },
    "answers": {
      "file.root": {
        "type": "choice",
        "choice": "docs",
        "probabilities": { "docs": 0.538, "out": 0.1689, "config": 0.2931 },
        "confidence": 0.307
      },
      "file.name_clear": { "type": "noul", "noul": 0.7589 }
    },
    "seconds": 2.762311072
  }
}
```

## `xtf calibrate`

```
xtf calibrate SET.jsonl --repos DIR [--batch N]
```

Measures `file.root` on labelled real files. Each line of SET.jsonl is
`{"manifest": "manifests/x.json", "root": "RepoName", "path":
"src/a.rs", "gold": "source"}`: the manifest (relative to the set's folder)
declares the repository's layout, `root` names its folder under `--repos
DIR`, `gold` is the role the file's folder plays. Conventional top-level
files are counted as settled by the rules, files whose name or extension
leaves one role are counted as settled by those rules, and the rest are
asked, `N` questions per request (default 5) on the repository's manifest as
the state.

It prints one row per file (`gold`, `chosen`, `confidence`, `path`), the
accuracy overall and per role, the confusions, and for confidence thresholds
from 0 to 0.7 how many answers clear it, how many of those are right, and how
many would have moved a file that was already where it belongs.

The committed set and its results are in [`calibration/`](../calibration);
`calibration/make-set.sh REPOS_DIR` draws a new set from checkouts of the
same repositories.

## Exit status

0 on success; 1 with a message on standard error otherwise (no manifest
above PATH, an unreadable file, a server that is unreachable or refuses the
request, with the server's reason).
