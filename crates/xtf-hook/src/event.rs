//! The hook's input and output, as Claude Code's hooks reference documents
//! them (read 2026-11-21, Claude Code 2.1.316): one event object on standard
//! input; at most one JSON object on standard output, `hookSpecificOutput`
//! with the event's name and either a permission decision (PreToolUse) or
//! `additionalContext`. Text that reaches Claude is capped below Claude
//! Code's 10,000-character limit, past which it would be replaced by a file
//! path and a preview.

use serde::Deserialize;
use serde_json::{Value, json};

/// The fields of an event the hook reads: the common input fields and
/// PreToolUse's tool fields. Every other field is ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Event {
    #[serde(default)]
    pub session_id: String,
    /// The working directory when the hook runs (relative `mkdir` operands
    /// resolve against it).
    #[serde(default)]
    pub cwd: String,
    /// The session's scratchpad (Claude Code 2.1.257 and later; absent when
    /// there is none). Paths under it are never decided on.
    #[serde(default)]
    pub scratchpad_dir: Option<String>,
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Value,
}

impl Event {
    /// A string field of `tool_input` (empty when absent or not a string).
    pub fn input_str(&self, key: &str) -> &str {
        self.tool_input
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
    }
}

/// The most characters the hook puts into one field Claude reads.
pub const MAX_TEXT: usize = 9_000;

fn capped(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text.to_string();
    }
    let mut s: String = text.chars().take(MAX_TEXT).collect();
    s.push_str("\n(cut at 9000 characters)");
    s
}

/// PreToolUse: the tool call does not run; Claude reads `reason`.
pub fn deny(reason: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": capped(reason),
        }
    })
}

/// Any event: `text` is added to Claude's context; no decision is made.
pub fn context(event: &str, text: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": capped(text),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_have_the_documented_shapes() {
        assert_eq!(
            deny("r").to_string(),
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"r"}}"#
        );
        assert_eq!(
            context("SessionStart", "c").to_string(),
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"c"}}"#
        );
        let long = context("PreToolUse", &"x".repeat(20_000));
        let text = long["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(text.chars().count() < 10_000 && text.ends_with("characters)"));
    }

    #[test]
    fn events_parse_with_missing_and_extra_fields() {
        let ev: Event = serde_json::from_str(
            r#"{"session_id":"s","cwd":"/p","hook_event_name":"PreToolUse","tool_name":"Write",
                "tool_input":{"file_path":"/p/a.rs","content":"x"},"permission_mode":"default","effort":{"level":"high"}}"#,
        )
        .unwrap();
        assert_eq!(ev.input_str("file_path"), "/p/a.rs");
        assert_eq!(ev.input_str("missing"), "");
        assert!(ev.scratchpad_dir.is_none());
        let bare: Event = serde_json::from_str("{}").unwrap();
        assert!(bare.tool_name.is_empty() && bare.tool_input.is_null());
    }
}
