use executors::LogKind;
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct NormalizedEvent {
    pub kind: LogKind,
    pub payload: Value,
    pub thread_id: Option<String>,
    pub assistant_message: Option<String>,
}

pub fn normalize_event(raw: Value) -> NormalizedEvent {
    let event_name = event_name(&raw).unwrap_or_default();
    let lower = event_name.to_ascii_lowercase();
    if is_agent_message_delta(&lower) {
        return NormalizedEvent {
            thread_id: extract_thread_id(&raw),
            kind: LogKind::AssistantDelta,
            payload: assistant_delta_payload(&raw),
            assistant_message: None,
        };
    }
    if is_agent_message_completed(&lower, &raw) {
        let payload = completed_agent_message_payload(&raw);
        let assistant_message = payload
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned);
        return NormalizedEvent {
            thread_id: extract_thread_id(&raw),
            kind: LogKind::Assistant,
            payload,
            assistant_message,
        };
    }

    let kind = if lower.contains("agentmessage") || lower == "assistant" {
        LogKind::Assistant
    } else if lower.contains("sessioninfo") || lower.contains("thread/started") {
        LogKind::SessionInfo
    } else if lower.contains("execcommandstart")
        || lower.contains("execcommandbegin")
        || lower.contains("commandexecution/requestapproval")
        || lower.contains("commandexecution/start")
        || lower.contains("item/tool/call")
        || lower.contains("toolcall")
    {
        LogKind::ToolCall
    } else if lower.contains("execcommandend")
        || lower.contains("commandexecution/end")
        || lower.contains("commandexecution/completed")
        || lower.contains("toolresult")
    {
        LogKind::ToolResult
    } else if lower.contains("filechange")
        || lower.contains("patchapply")
        || lower.contains("file_change")
    {
        LogKind::FileChange
    } else if extract_thread_id(&raw).is_some() {
        LogKind::SessionInfo
    } else {
        LogKind::Stderr
    };

    let assistant_message = if kind == LogKind::Assistant {
        extract_assistant_message(&raw)
    } else {
        None
    };

    NormalizedEvent {
        thread_id: extract_thread_id(&raw),
        kind,
        payload: raw,
        assistant_message,
    }
}

pub fn extract_thread_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["thread_id", "threadId"] {
                if let Some(id) = map.get(key).and_then(Value::as_str) {
                    return Some(id.to_owned());
                }
            }
            if let Some(id) = map
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(Value::as_str)
            {
                return Some(id.to_owned());
            }
            map.values().find_map(extract_thread_id)
        }
        Value::Array(items) => items.iter().find_map(extract_thread_id),
        _ => None,
    }
}

pub fn is_turn_completed(value: &Value) -> bool {
    event_name(value)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower == "turn/completed" || lower == "turncompleted" || lower == "turn_completed"
        })
        .unwrap_or(false)
}

fn event_name(value: &Value) -> Option<&str> {
    value
        .get("method")
        .and_then(Value::as_str)
        .or_else(|| value.get("type").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("type"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .get("params")
                .and_then(|params| params.get("msg"))
                .and_then(|msg| msg.get("type"))
                .and_then(Value::as_str)
        })
}

fn extract_assistant_message(value: &Value) -> Option<String> {
    for path in [
        &["params", "message"][..],
        &["params", "text"],
        &["params", "content"],
        &["params", "msg", "message"],
        &["params", "msg", "text"],
        &["message"],
        &["text"],
        &["content"],
    ] {
        #[allow(clippy::collapsible_if)] // pre-existing warning, out of scope for this change
        if let Some(text) = value_at_path(value, path).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_owned());
            }
        }
    }

    value
        .get("params")
        .and_then(|params| params.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!text.trim().is_empty()).then_some(text)
        })
}

fn is_agent_message_delta(event_name: &str) -> bool {
    event_name == "item/agentmessage/delta"
        || (event_name.contains("agentmessage") && event_name.contains("delta"))
}

fn is_agent_message_completed(event_name: &str, value: &Value) -> bool {
    event_name == "item/completed"
        && value_at_path(value, &["params", "item", "type"])
            .and_then(Value::as_str)
            .is_some_and(|item_type| item_type == "agentMessage")
}

fn assistant_delta_payload(value: &Value) -> Value {
    let mut payload = common_item_payload(value);
    insert_payload_value(
        &mut payload,
        "delta",
        value,
        &[&["params", "delta"][..], &["delta"]],
    );
    Value::Object(payload)
}

fn completed_agent_message_payload(value: &Value) -> Value {
    let mut payload = common_item_payload(value);
    insert_payload_value(
        &mut payload,
        "text",
        value,
        &[
            &["params", "item", "text"][..],
            &["params", "text"],
            &["text"],
        ],
    );
    Value::Object(payload)
}

fn common_item_payload(value: &Value) -> Map<String, Value> {
    let mut payload = Map::new();
    insert_payload_value(
        &mut payload,
        "itemId",
        value,
        &[
            &["params", "itemId"][..],
            &["params", "item", "id"],
            &["itemId"],
            &["item", "id"],
        ],
    );
    insert_payload_value(
        &mut payload,
        "threadId",
        value,
        &[
            &["params", "threadId"][..],
            &["params", "thread_id"],
            &["params", "item", "threadId"],
            &["params", "item", "thread_id"],
            &["threadId"],
            &["thread_id"],
        ],
    );
    insert_payload_value(
        &mut payload,
        "turnId",
        value,
        &[
            &["params", "turnId"][..],
            &["params", "turn_id"],
            &["params", "item", "turnId"],
            &["params", "item", "turn_id"],
            &["turnId"],
            &["turn_id"],
        ],
    );
    payload
}

fn insert_payload_value(
    payload: &mut Map<String, Value>,
    key: &str,
    value: &Value,
    paths: &[&[&str]],
) {
    if let Some(found) = paths
        .iter()
        .find_map(|path| value_at_path(value, path).cloned())
    {
        payload.insert(key.to_owned(), found);
    }
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_agent_message_deltas_and_completed_text_are_normalized_separately() {
        let events = [
            json!({
                "jsonrpc": "2.0",
                "method": "item/agentMessage/delta",
                "params": {
                    "itemId": "item-1",
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "delta": "Done"
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "item/agentMessage/delta",
                "params": {
                    "itemId": "item-1",
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "delta": "."
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "item/agentMessage/delta",
                "params": {
                    "itemId": "item-1",
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "delta": " PASS"
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "item/completed",
                "params": {
                    "itemId": "item-1",
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "item": {
                        "type": "agentMessage",
                        "text": "Done. PASS"
                    }
                }
            }),
        ];

        let normalized = events.into_iter().map(normalize_event).collect::<Vec<_>>();

        assert_eq!(normalized[0].kind, LogKind::AssistantDelta);
        assert_eq!(normalized[1].kind, LogKind::AssistantDelta);
        assert_eq!(normalized[2].kind, LogKind::AssistantDelta);
        assert_eq!(normalized[3].kind, LogKind::Assistant);
        assert_eq!(normalized[0].payload["delta"], "Done");
        assert_eq!(normalized[1].payload["delta"], ".");
        assert_eq!(normalized[2].payload["delta"], " PASS");
        assert_eq!(normalized[3].payload["text"], "Done. PASS");
        assert_eq!(
            normalized[3].assistant_message.as_deref(),
            Some("Done. PASS")
        );
        assert_eq!(normalized[3].payload["itemId"], "item-1");
        assert_eq!(normalized[3].payload["threadId"], "thread-1");
        assert_eq!(normalized[3].payload["turnId"], "turn-1");
    }
}
