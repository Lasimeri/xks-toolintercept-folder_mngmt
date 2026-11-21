# Acceptance scenarios

The flow of setting up a project and placing its files, run through the
real `xtf-mcp` binary over stdio, the way a coding agent drives it. The
runner is `crates/xtf-mcp/tests/acceptance.rs`; it needs a System One
server (`jevbase serve`, or `TYPESAFE_BASE_URL`), so it is ignored by
default:

```sh
cargo test -p xtf-mcp --test acceptance -- --ignored --nocapture
```

`XTF_ACCEPTANCE_LOCK=FILE` makes each scenario hold an exclusive flock(2)
on FILE, for a machine where other sessions share the one server.

Each `*.json` file here is one scenario, run in name order. A scenario gets
a fresh folder, which is the server's working directory (relative paths in
its steps resolve there), and one server process, like one Claude Code
session. The output prints every step with its wall time and summary line,
the failed checks, and the recorded values.

```json
{
  "name": "04-extend-then-place",
  "about": "one sentence: what the scenario shows",
  "steps": [
    {
      "tool": "extend_layout",
      "args": {"path_or_root": "flacopus", "role": "tools", "dirs": ["scripts"]},
      "expect": {"error": false, "eq": {"/added": ["scripts/"]}},
      "record": ["/layout"]
    }
  ]
}
```

`expect` checks the contract; every key is optional:

| key | checks |
|---|---|
| `error` | the result's `isError` |
| `line_has` | substrings of the summary line (for an error: of the message) |
| `eq` | JSON Pointer (RFC 6901) into the result's JSON -> the exact value |
| `present` | JSON Pointers that must hold a value other than null |
| `pluck` | JSON Pointer to an array -> `{"field": F, "eq": [...]}`: field F of each element, in order |
| `dirs`, `files` | paths (relative to the scenario folder) that must be a folder or a file afterwards |

`record` lists JSON Pointers whose values are printed and not checked: the
model's answers (roles, confidences, probabilities, request times). They
belong to the standard's question wording and the model, which change
between versions, so a scenario asserts what the tools promise and records
what the model said.
