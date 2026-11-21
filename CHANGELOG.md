# Changelog

## 0.3.1 (2026-11-21)

### Changed

- **Requests to the server are cut by size as well as count**: at most 4
  questions and 1,400 bytes of questions each. What holds the one shared
  server is a request's size (measured: about 4.9 ms per server input token
  for requests of 750 to 2,908 tokens), and 0.3.0's longer folder questions
  made a 4-question request take 10.6 s. On the same 12-path plan the
  longest request now takes 4.8 s (4 requests, 16.98 s in all, against 18.2
  s before), with the same choices; a hook queued behind one keeps about
  10 s of its 15 s.
- With lane/ux merged (0.3.0 tail): `extend_layout`, layout gaps in
  `plan_tree` and `place_file`, and the acceptance scenarios
  (`tests/acceptance/`, run with `cargo test -p xtf-mcp --test acceptance --
  --ignored`).

## 0.3.0 (2026-11-21)

The layout grows with the project: paths outside it are asked what they
are, and a confident answer the layout has no folder for becomes a
suggested layout change.

### Changed

- **Outside the layout** (a non-conventional file at the root, or a path
  under a top-level folder the manifest does not map), `file.root` offers
  the roles the path can play, not only the layout's: a new folder every
  role but config, a file the roles its name or extension makes plausible
  with out and config. Before, `scripts/release.sh` in a layout without a
  tools folder was offered only tests and out, and was confidently
  suggested to `out/` (0.83). Live on the same 12-path plan: `scripts/` is
  now read as tools at 0.89 (was source 0.26), `bench.sh` as tools 0.49
  (was out 0.15).
- **Layout changes**: a confident role the layout lacks gives
  `layout_change: {role, dir}` in the verdict (the folder mapped where it
  is, or a root file's role folder), never a move of the folder. The hook
  adds it as context and never blocks on it: `mkdir scripts` read as tools
  at 0.92 was let through with `"tools": ["scripts"]` named.
- **Requests to the server carry at most 4 questions** (was 8): a real
  8-question request held the one server 14.1 s, so a hook queued behind it
  had no time left of its 15 s. Measured with 4: 7.6 s and 10.6 s (the
  second with a 12-option folder question).
- Under the 0.5 floor, a path whose role the model agrees with is said to
  stay, not to belong ("stays in source: the model leans the same way").

### Added

- `roles::by_folder_name`, the folder-name convention table `xtf init`
  uses, in the library.

## 0.2.0 (2026-11-21)

Claude Code, automatic in every session: the standard in front of every new
file, folder and project, decided by one shared local model instance.

### Added

- **`xtf-hook`** (`crates/xtf-hook`): `session-start` gives Claude the
  project's layout and naming (or notes that it has no manifest);
  `pre-tool-use` decides every new file (`Write`) and new folder (`mkdir`)
  with the model, denies the first attempt at a path the model confidently
  places elsewhere or whose name breaks the naming style, with the
  suggested path, and lets the same attempt again go through
  (`XTF_HOOK_MODE` enforce, advise, off). Fails open: a server down, busy
  past 15 s or refusing never blocks Claude. Without a model call it costs
  0.77 to 0.96 ms. See [docs/hook.md](docs/hook.md).
- **`xtf-mcp`** (`crates/xtf-mcp`): an MCP server over stdio with five
  tools: `new_project` (the model decides the kind; the folders follow from
  the kind and language; the manifest is written and the folders created),
  `place_file`, `plan_tree` (a whole planned tree, at most 8 questions per
  request so no request holds the shared server long), `pick_name`,
  `project_info`. See [docs/mcp.md](docs/mcp.md).
- **The new-project layouts** (`xtf_standard::scaffold`): a fixed table of
  roles by kind and folders by language ([SPEC.md](SPEC.md#new-projects)).
- **The Claude Code plugin** (`integrations/claude-code`) and the decision
  server as a systemd user service (`integrations/systemd/jevbase.service`),
  both installed by `make install-claude`.

### Fixed

- `out` was left by elimination: a file whose extension allows only roles
  the layout lacks (a script in a project without tools, tests,
  integrations or examples folders) was settled to `out` at confidence 1.00
  and suggested to move there. Every role is offered instead; only `.log`
  settles to `out`. No calibration item was affected.
- `mkdir` of a folder the manifest names (a layout or ignore entry) was read
  as a file at the root; it is now allowed by rule.
- The decision log wrote each record in many small writes (84 for a
  250-byte line under strace), so two sessions appending at once could split
  each other's lines; each record is now one write.

### Measured with Claude Code 2.1.316

`claude -p` sessions on the machine the plugin was installed on, against
`jevbase serve` (Qwen3.8 2B Q8_0, 8 threads): the plugin loaded and its MCP
server connected; a naming violation was denied and its override let
through; `new_project` decided the kind cli-tool at confidence 0.85 and
created the folders; three sessions writing new files at once were all
decided (4.2 to 5.7 s each, the queue included) with the shared log intact.

### Known limits

- The decision server answers one request at a time; a hook queued past
  15 s lets its operation through undecided.
- The folders decided so far stayed under the 0.5 confidence the policy
  acts on: the placement question sees a name and a summary, not where the
  path is going.

## 0.1.0 (2026-11-21)

The first release: the standard, its library and command line, and its
calibration.

### Added

- **Standard xks-folder/1** ([SPEC.md](SPEC.md)): the project manifest
  (`xks-project.json`, the provided JSON), projected operations (the
  projected JSON), 13 fixed roles with fixed wording, the question catalog
  (`file.root`, `file.name_clear`, `name.pick`, `project.kind`), the rules
  that settle decisions without the model (conventional files, naming
  styles, test and benchmark file names, extensions), the policy (act on a
  Choice at confidence 0.5 or more; Noul yes above 0.7, no below 0.3, as
  TypeSafe documents), and the decision log (`.xks/decisions.jsonl`).
- **`xtf-standard`**, the library, and **`xtf`**, its command line: `init`
  (write a manifest from a project's folders), `check` (decide one
  operation and log it), `calibrate` (measure the standard on labelled
  repositories). See [docs/cli.md](docs/cli.md).
- **Any System One server decides**: configured as TypeSafe's SDKs are
  (`TYPESAFE_BASE_URL`, `TYPESAFE_API_KEY`, `TYPESAFE_DEFAULT_MODEL`), the
  same requests go to `jevbase serve` (a local Jev-like model), Intel-Phi-Jev's
  `xks serve`, or TypeSafe's hosted Jev.
- **Built on XKS_Jev-like_Model_Base v0.1.0-rc.1**, client only: `xtf`
  carries no model and no llama.cpp. `make build` writes no path of the
  building machine into the binary.
- **Calibration** ([calibration/](calibration)): 141 real files of six public
  repositories, labelled by the role of their folder, with the manifests,
  the sampler and the per-file results of each version of the standard.

### Measured

With `jevbase serve` (Qwen3.8 2B Q8_0, 8 CPU threads), 118 of the 141 files
counted (23 conventional files settled by the rules; in v3, 7 of the 118 are
settled by a name or extension rule and 111 asked):

| version | right overall | confidence >= 0.5: answered / right / false moves | decisions/s |
|---|---|---|---|
| v1, every role offered | 51.7% | 25.4% / 80.0% / 5.1% | 0.22 |
| v2, options narrowed by extension | 59.3% | 32.2% / 89.5% / 3.4% | 0.28 |
| v3, v2 + test and benchmark file names (this release) | 61.0% | 33.1% / 92.3% / 2.5% | 0.33 |

### Not in this release

- The MCP server (`xtf-mcp`) and the Claude Code hook (`xtf-hook`) that put
  the standard in front of an agent (added in 0.2.0).
