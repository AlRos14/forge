use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum NormalizedEntry {
    Assistant {
        payload: Value,
        content: Option<String>,
        session_id: Option<String>,
    },
    ToolCall {
        payload: Value,
        session_id: Option<String>,
    },
    ToolResult {
        payload: Value,
        session_id: Option<String>,
    },
    SessionInfo {
        payload: Value,
        session_id: Option<String>,
    },
    Stderr {
        payload: Value,
    },
}

pub fn normalize(line: &str) -> Option<NormalizedEntry> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let payload = match serde_json::from_str::<Value>(trimmed) {
        Ok(payload) => payload,
        Err(_) => {
            return Some(NormalizedEntry::Stderr {
                payload: serde_json::json!({ "line": line }),
            });
        }
    };

    let event_type = payload.get("type").and_then(Value::as_str);
    let session_id = extract_session_id(&payload);

    match event_type {
        Some("assistant") => normalize_assistant(payload, session_id),
        Some("user") => normalize_user(payload, session_id),
        Some("tool_use" | "tool_call") => Some(NormalizedEntry::ToolCall {
            session_id,
            payload,
        }),
        Some("tool_result") => Some(NormalizedEntry::ToolResult {
            session_id,
            payload,
        }),
        Some("session_info" | "system" | "result") => Some(NormalizedEntry::SessionInfo {
            session_id,
            payload,
        }),
        Some("stream_event" | "rate_limit_event") => None,
        _ => Some(NormalizedEntry::Stderr {
            payload: serde_json::json!({ "line": line }),
        }),
    }
}

fn normalize_assistant(payload: Value, session_id: Option<String>) -> Option<NormalizedEntry> {
    if let Some(content) = extract_assistant_content(&payload) {
        return Some(NormalizedEntry::Assistant {
            content: Some(content),
            session_id,
            payload,
        });
    }

    if let Some(tool_call) = extract_assistant_tool_call(&payload) {
        return Some(NormalizedEntry::ToolCall {
            session_id,
            payload: tool_call,
        });
    }

    None
}

fn normalize_user(payload: Value, session_id: Option<String>) -> Option<NormalizedEntry> {
    extract_user_tool_result(&payload).map(|tool_result| NormalizedEntry::ToolResult {
        session_id,
        payload: tool_result,
    })
}

fn extract_session_id(payload: &Value) -> Option<String> {
    payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn extract_assistant_content(payload: &Value) -> Option<String> {
    let content = payload
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| payload.get("content"))?;

    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn extract_assistant_tool_call(payload: &Value) -> Option<Value> {
    let item = payload
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| payload.get("content"))?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))?;
    Some(serde_json::json!({
        "tool": item.get("name").and_then(Value::as_str).unwrap_or("unknown"),
        "name": item.get("name").and_then(Value::as_str).unwrap_or("unknown"),
        "call_id": item.get("id").and_then(Value::as_str),
        "params": item.get("input").cloned().unwrap_or(Value::Null),
        "original": item,
    }))
}

fn extract_user_tool_result(payload: &Value) -> Option<Value> {
    let item = payload
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| payload.get("content"))?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))?;
    Some(serde_json::json!({
        "call_id": item.get("tool_use_id").and_then(Value::as_str),
        "success": !item.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        "content": item.get("content").cloned().unwrap_or(Value::Null),
        "is_error": item.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        "original": item,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_event_extracts_content_and_session_id() {
        let entry = normalize(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]},"session_id":"abc"}"#,
        )
        .unwrap();

        assert_eq!(
            entry,
            NormalizedEntry::Assistant {
                payload: serde_json::json!({
                    "type": "assistant",
                    "message": { "content": [{ "type": "text", "text": "Hello" }] },
                    "session_id": "abc"
                }),
                content: Some("Hello".to_owned()),
                session_id: Some("abc".to_owned()),
            }
        );
    }

    #[test]
    fn system_event_maps_to_session_info() {
        let entry = normalize(r#"{"type":"system","subtype":"init","session_id":"abc"}"#).unwrap();

        assert!(matches!(
            entry,
            NormalizedEntry::SessionInfo {
                session_id: Some(ref session_id),
                ..
            } if session_id == "abc"
        ));
    }

    #[test]
    fn parse_failure_maps_to_stderr() {
        let entry = normalize("not-json").unwrap();

        assert_eq!(
            entry,
            NormalizedEntry::Stderr {
                payload: serde_json::json!({ "line": "not-json" }),
            }
        );
    }

    #[test]
    fn assistant_tool_use_maps_to_tool_call() {
        let entry = normalize(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo ok"}}]},"session_id":"abc"}"#,
        )
        .unwrap();

        assert_eq!(
            entry,
            NormalizedEntry::ToolCall {
                payload: serde_json::json!({
                    "tool": "Bash",
                    "name": "Bash",
                    "call_id": "toolu_1",
                    "params": { "command": "echo ok" },
                    "original": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "Bash",
                        "input": { "command": "echo ok" }
                    }
                }),
                session_id: Some("abc".to_owned()),
            }
        );
    }

    #[test]
    fn user_tool_result_maps_to_tool_result() {
        let entry = normalize(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":false}]},"session_id":"abc"}"#,
        )
        .unwrap();

        assert_eq!(
            entry,
            NormalizedEntry::ToolResult {
                payload: serde_json::json!({
                    "call_id": "toolu_1",
                    "success": true,
                    "content": "ok",
                    "is_error": false,
                    "original": {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "ok",
                        "is_error": false
                    }
                }),
                session_id: Some("abc".to_owned()),
            }
        );
    }

    #[test]
    fn stream_event_is_ignored() {
        assert!(normalize(r#"{"type":"stream_event","event":{"type":"message_start"}}"#).is_none());
    }
}
