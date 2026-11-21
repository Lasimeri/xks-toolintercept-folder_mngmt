//! The MCP side: the `initialize` answer (protocol version, capabilities,
//! server instructions), the `tools/list` entries (names, descriptions,
//! input schemas, hints) and the `tools/call` params.
//!
//! With Claude Code's tool search (on by default) only the server
//! instructions and the tool names reach Claude up front; a tool's
//! description and schema load when a search finds it. So the instructions
//! say, in a few factual sentences, what the tools are for and when they
//! help, and each description leads with the words a search would match.
//! Claude Code cuts instructions past 4,096 characters and a searched tool's
//! description past 16,384 (docs read 2026-11-21).

use serde_json::{Map, Value, json};
use xtf_standard::catalog::KINDS;

/// Protocol revisions this server speaks, newest first. A client asking for
/// one of them gets it; any other request gets the newest.
pub const SUPPORTED: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The server instructions: factual, under 1,500 characters.
pub const INSTRUCTIONS: &str = "\
xtf answers folder and naming questions for projects that follow the xks-folder/1 standard: \
an xks-project.json manifest at the project root maps fixed roles (source, tests, docs, tools, \
data, out, ...) to the project's folders. Rules settle what needs no model; a local Jev-like \
model on a System One server (default http://127.0.0.1:8097, one shared instance) answers the \
rest (1 to 4 s a call), and answers under 0.5 confidence are reported as unsure and not \
acted on. Tools: new_project sets up a new project folder (decides its kind, writes the \
manifest with a conventional folder layout, can create the folders); plan_tree places a whole \
set of planned files and folders in one call and returns the resulting tree; place_file checks \
where one new, moved or renamed file or folder belongs; both report layout gaps (planned paths \
outside every layout folder), which extend_layout closes by adding a folder to the layout under \
a role; pick_name chooses among candidate names; project_info reads the nearest manifest. They \
help when starting a project or a folder structure, when a new file's place is not obvious, and \
when choosing a name. Every placement is logged in the project's .xks/decisions.jsonl.";

/// The `initialize` result for a client that sent `params`.
pub fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| SUPPORTED.contains(v))
        .unwrap_or(SUPPORTED[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "xtf", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

/// The tools, in the order listed: the order of setting up a project.
pub const TOOLS: [&str; 6] = [
    "new_project",
    "plan_tree",
    "place_file",
    "extend_layout",
    "pick_name",
    "project_info",
];

/// The `tools/list` result.
pub fn tools_list() -> Value {
    let kinds: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
    let kinds_text: Vec<String> = KINDS.iter().map(|(k, d)| format!("{k} ({d})")).collect();
    let ops = ["create_file", "create_dir", "move", "rename", "delete"];
    json!({"tools": [
        {
            "name": "new_project",
            "title": "New project: manifest and folder layout",
            "description": format!(
                "Sets up a new project folder and its folder structure: decides the project kind \
                 when not given (the model chooses among the kinds), writes xks-project.json with \
                 a conventional folder layout for that kind and the first language, and with \
                 create_dirs creates those folders (mkdir only, nothing is removed or \
                 overwritten). An existing xks-project.json is never overwritten: it is returned \
                 unchanged. The kind decision is logged in the project's .xks/decisions.jsonl. \
                 Kinds: {}.",
                kinds_text.join("; ")
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "The project folder (absolute, ~/..., or relative to the server's working directory); created when missing."},
                    "purpose": {"type": "string", "minLength": 1, "description": "One or two sentences: what the project is for. The model reads it."},
                    "languages": {"type": "array", "items": {"type": "string", "minLength": 1}, "description": "Programming languages, the main one first (e.g. [\"rust\"], [\"c\", \"python\"]); [] for none. The first sets folder and naming conventions."},
                    "name": {"type": "string", "minLength": 1, "description": "Project name; default: the folder's name."},
                    "kind": {"type": "string", "enum": kinds, "description": "The project kind; when omitted the model decides it."},
                    "create_dirs": {"type": "boolean", "default": false, "description": "Also create the layout's folders (mkdir only)."}
                },
                "required": ["dir", "purpose", "languages"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "plan_tree",
            "title": "Plan a folder tree",
            "description": "Places a whole set of planned files and folders of one project (1 to 64; a path ending in / is a folder) in one call, before they are written: the rules first, then the model's file.root question for the rest, batched on the project's manifest in requests of up to 8 questions (the one shared server answers one request at a time; other sessions' hooks wait behind each). Returns each verdict (allow, suggest with suggested_path, or uncertain under 0.5 confidence), the resulting tree (the paths after the suggestions), and layout_gaps: planned paths outside every layout folder (a new top-level folder, a path under an unmapped one, or a root file such as a script), each with the roles its files' types play that the layout lacks and the next step; extend_layout closes a gap. Each decision is logged in .xks/decisions.jsonl. About 1.3 to 2 s per file the rules leave open.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": {"type": "string", "description": "The folder the planned paths are relative to (absolute, ~/..., or relative to the server's working directory); the manifest is found at or above it."},
                    "files": {
                        "description": "The planned paths: each a path relative to root (ending in / for a folder) and a summary of what it will hold.",
                        "type": "array", "minItems": 1, "maxItems": 64,
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {"type": "string", "minLength": 1, "description": "Planned path relative to root; ending in / for a folder."},
                                "summary": {"type": "string", "description": "What it will hold, a sentence or the first lines of its content (up to 240 characters are read)."}
                            },
                            "required": ["path", "summary"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["root", "files"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "place_file",
            "title": "Place one file or folder",
            "description": "Where one new, moved or renamed file or folder belongs in a project with an xks-project.json manifest, and whether its name follows the project's naming style. Rules settle deletions, generated or vendored paths and conventional top-level files; otherwise the model answers file.root (which role: source, tests, docs, tools, ...) and file.name_clear (does the name say what it holds). Verdict: allow, suggest (with suggested_path), or uncertain (confidence under 0.5, nothing suggested); layout_gaps when the path lies outside every layout folder (extend_layout closes it). Logged in .xks/decisions.jsonl. About 3 s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "minLength": 1, "description": "The path being created (for a move or rename: the new path); absolute, ~/..., or relative to the server's working directory. Ending in / means a folder."},
                    "summary": {"type": "string", "description": "What it holds, a sentence. Default: the start of content, or of the existing file."},
                    "content": {"type": "string", "description": "The content about to be written; its start (240 characters) is read."},
                    "op": {"type": "string", "enum": ops, "description": "Default create_file (create_dir for a path ending in /)."},
                    "from": {"type": "string", "description": "For move and rename: the current path."}
                },
                "required": ["path"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "extend_layout",
            "title": "Add folders to the layout",
            "description": format!(
                "Adds folders to a project's layout in xks-project.json under one role, so \
                 placements can use them: for example tools -> [\"scripts\"] after plan_tree or \
                 place_file report a layout gap. The role is the caller's choice (the layout's \
                 roles: {}; config is the project root itself). Each folder is checked against the \
                 project's folder naming style, the ignore list and the existing mappings (a \
                 folder plays one role); no mapping is ever removed or moved. The manifest is \
                 rewritten whole under a lock, so concurrent sessions never lose a change; with \
                 create_dirs the folders are created (mkdir only). Each added folder is logged in \
                 .xks/decisions.jsonl. Repeating a call changes nothing. No model.",
                crate::tools::layout_roles().join(", ")
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path_or_root": {"type": "string", "minLength": 1, "description": "A path inside the project (or its root); the manifest is found at or above it."},
                    "role": {"type": "string", "enum": crate::tools::layout_roles(), "description": "The role the folders play."},
                    "dirs": {"type": "array", "minItems": 1, "maxItems": crate::tools::MAX_EXTEND, "items": {"type": "string", "minLength": 1}, "description": "Folders relative to the project root, e.g. [\"scripts\"] or [\"src/tui\"]."},
                    "create_dirs": {"type": "boolean", "default": false, "description": "Also create the folders (mkdir only)."}
                },
                "required": ["path_or_root", "role", "dirs"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "pick_name",
            "title": "Pick a name",
            "description": "Chooses the best of 2 to 26 candidate names for a new file, folder, module or other part of a project (question name.pick: short, says what it holds, reads like the project's other names), asked on the project's manifest. Returns the choice, the probabilities, the confidence, whether it reaches 0.5 (the level at which the standard acts on an answer), and which candidates break the project's file naming style. About 1.2 s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path_or_root": {"type": "string", "description": "A path inside the project (or its root); the manifest is found at or above it."},
                    "what": {"type": "string", "minLength": 1, "description": "What needs the name, a sentence."},
                    "candidates": {"type": "array", "minItems": 2, "maxItems": 26, "uniqueItems": true, "items": {"type": "string", "minLength": 1}, "description": "The candidate names."}
                },
                "required": ["path_or_root", "what", "candidates"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "project_info",
            "title": "Project manifest",
            "description": "The xks-project.json manifest nearest at or above a path: project root, name, purpose, kind, languages, layout (role to folders), naming styles, and how many decisions its .xks/decisions.jsonl holds; or that there is none. No model.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "minLength": 1, "description": "Any path inside the project, or its root."}
                },
                "required": ["path"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        }
    ]})
}

/// The tool name and arguments of a `tools/call`; Err for an unknown tool
/// or arguments that are not an object (JSON-RPC Invalid params).
pub fn call_params(params: &Value) -> Result<(String, Map<String, Value>), String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call needs `name`")?;
    if !TOOLS.contains(&name) {
        return Err(format!(
            "unknown tool `{name}`; the tools are {}",
            TOOLS.join(", ")
        ));
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => return Err("`arguments` must be an object".into()),
    };
    Ok((name.to_string(), args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_negotiated() {
        let v = |p: Value| initialize(&p)["protocolVersion"].clone();
        assert_eq!(v(json!({"protocolVersion": "2025-06-18"})), "2025-06-18");
        assert_eq!(v(json!({"protocolVersion": "2099-01-01"})), SUPPORTED[0]);
        assert_eq!(v(json!({})), SUPPORTED[0]);
        let r = initialize(&json!({}));
        assert_eq!(r["serverInfo"]["name"], "xtf");
        assert_eq!(r["capabilities"], json!({"tools": {}}));
    }

    #[test]
    fn tools_are_listed_with_valid_schemas() {
        let list = tools_list();
        let tools = list["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOLS);
        for t in tools {
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object");
            // Claude Code's rule: property names of 1 to 64 characters of
            // ASCII letters, digits, `_`, `.` and `-`.
            for key in schema["properties"].as_object().unwrap().keys() {
                assert!((1..=64).contains(&key.len()));
                assert!(
                    key.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                );
            }
            for r in schema["required"].as_array().unwrap() {
                assert!(schema["properties"].get(r.as_str().unwrap()).is_some());
            }
            let d = t["description"].as_str().unwrap();
            assert!(d.len() < 4_096, "{}", t["name"]);
        }
        assert!(INSTRUCTIONS.len() < 1_500, "{}", INSTRUCTIONS.len());
        let all = format!("{list}{INSTRUCTIONS}");
        assert!(!all.contains('\u{2014}') && !all.contains('\u{2013}'));
    }

    #[test]
    fn call_params_name_a_known_tool() {
        let ok = call_params(&json!({"name": "project_info", "arguments": {"path": "."}}));
        assert_eq!(ok.unwrap().0, "project_info");
        assert!(call_params(&json!({"name": "rm_rf"})).is_err());
        assert!(call_params(&json!({"name": "plan_tree", "arguments": [1]})).is_err());
        assert!(
            call_params(&json!({"name": "plan_tree"}))
                .unwrap()
                .1
                .is_empty()
        );
    }
}
