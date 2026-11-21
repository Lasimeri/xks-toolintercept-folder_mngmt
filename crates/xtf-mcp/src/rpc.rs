//! JSON-RPC 2.0 as MCP's stdio transport carries it: one JSON message per
//! line. A message is a request (answered under its id), a notification (no
//! id, never answered), a response (this server sends no requests, so any
//! response is ignored), or none of these (answered with Invalid Request).

use serde_json::{Value, json};

/// Not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON, but not a JSON-RPC 2.0 request.
pub const INVALID_REQUEST: i64 = -32600;
/// No such method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The method exists, its params do not fit (e.g. an unknown tool).
pub const INVALID_PARAMS: i64 = -32602;

/// One incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
    },
    Response,
    Invalid {
        /// The message's id when it has a usable one, else null.
        id: Value,
        why: String,
    },
}

/// What kind of message `v` is.
pub fn classify(v: Value) -> Message {
    let Value::Object(mut o) = v else {
        return Message::Invalid {
            id: Value::Null,
            why: "a JSON-RPC message is an object".into(),
        };
    };
    let id = o.remove("id");
    let usable = |id: &Option<Value>| match id {
        Some(v @ (Value::String(_) | Value::Number(_))) => v.clone(),
        _ => Value::Null,
    };
    if o.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Message::Invalid {
            id: usable(&id),
            why: "`jsonrpc` must be \"2.0\"".into(),
        };
    }
    let Some(method) = o.get("method") else {
        if o.contains_key("result") || o.contains_key("error") {
            return Message::Response;
        }
        return Message::Invalid {
            id: usable(&id),
            why: "no `method`".into(),
        };
    };
    let Some(method) = method.as_str().map(str::to_string) else {
        return Message::Invalid {
            id: usable(&id),
            why: "`method` must be a string".into(),
        };
    };
    match id {
        None => Message::Notification { method },
        Some(id @ (Value::String(_) | Value::Number(_))) => Message::Request {
            id,
            method,
            params: o.remove("params").unwrap_or(Value::Null),
        },
        Some(_) => Message::Invalid {
            id: Value::Null,
            why: "`id` must be a string or a number".into(),
        },
    }
}

/// A success response.
pub fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// An error response.
pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Message {
        classify(serde_json::from_str(s).unwrap())
    }

    #[test]
    fn messages_are_told_apart() {
        assert_eq!(
            c(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#),
            Message::Request {
                id: json!(7),
                method: "ping".into(),
                params: Value::Null
            }
        );
        assert_eq!(
            c(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Message::Notification {
                method: "notifications/initialized".into()
            }
        );
        assert_eq!(
            c(r#"{"jsonrpc":"2.0","id":"a","result":{}}"#),
            Message::Response
        );
        assert!(matches!(
            c(r#"{"id":3,"method":"ping"}"#),
            Message::Invalid { id, .. } if id == json!(3)
        ));
        assert!(matches!(
            c(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#),
            Message::Invalid {
                id: Value::Null,
                ..
            }
        ));
        assert!(matches!(c("[1]"), Message::Invalid { .. }));
        let e = error(json!(1), METHOD_NOT_FOUND, "no");
        assert_eq!(e["error"]["code"], json!(-32601));
        assert_eq!(result(json!("x"), json!({}))["id"], json!("x"));
    }
}
