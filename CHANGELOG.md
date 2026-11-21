# Changelog

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
  the standard in front of an agent ([Roadmap](README.md#roadmap)).
