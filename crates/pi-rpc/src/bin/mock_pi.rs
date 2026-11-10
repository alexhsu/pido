//! A mock `pi --mode rpc` for tests and manual protocol experimentation.
//!
//! It speaks just enough of the JSONL protocol to exercise the client:
//! - `get_state` → a fixed state payload
//! - `prompt` → response, then a scripted streaming burst ending in
//!   `agent_settled`
//! - `set_model` → fails when the id contains `invalid`
//! - everything else → a plain success response
//!
//! Runs until stdin closes, then exits 0 — matching Pi's graceful-shutdown
//! behavior.

use std::io::{BufRead, BufReader, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());

    for line in reader.split(b'\n') {
        let bytes = match line {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        if bytes.is_empty() {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            let _ = writeln!(
                stdout,
                r#"{{"type":"response","command":"parse","success":false,"error":"bad json"}}"#
            );
            let _ = stdout.flush();
            continue;
        };
        let id = value.get("id").cloned();
        let record_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let reply = match record_type {
            "get_state" => respond(
                id,
                "get_state",
                serde_json::json!({
                    "model": {
                        "id": "mock-model",
                        "name": "Mock Model",
                        "reasoning": true,
                        "provider": "mock",
                    },
                    "thinkingLevel": "medium",
                    "isStreaming": false,
                    "isCompacting": false,
                    "sessionId": "mock-session",
                    "messageCount": 0,
                }),
            ),
            "prompt" => {
                // Response first (accepted), then a scripted streaming burst.
                let record = respond_ok(id, "prompt").expect("respond_ok is always Some");
                let burst = [
                    r#"{"type":"agent_start"}"#.to_string(),
                    r#"{"type":"message_start","message":{"role":"assistant","content":[],"timestamp":1}}"#.to_string(),
                    r#"{"type":"message_update","usage":{"input":10,"output":1},"assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#.to_string(),
                    r#"{"type":"message_update","usage":{"input":10,"output":2},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello "}}"#.to_string(),
                    r#"{"type":"message_update","usage":{"input":10,"output":3},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"world"}}"#.to_string(),
                    r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"}],"timestamp":2}}"#.to_string(),
                    r#"{"type":"agent_end","messages":[],"willRetry":false}"#.to_string(),
                    r#"{"type":"agent_settled"}"#.to_string(),
                ];
                let _ = writeln!(stdout, "{record}");
                for event in burst {
                    let _ = writeln!(stdout, "{event}");
                }
                None
            }
            "set_model" => {
                let model_id = value
                    .get("modelId")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default();
                if model_id.contains("invalid") {
                    respond_err(id, "set_model", "Model not found: invalid/model")
                } else {
                    respond(
                        id,
                        "set_model",
                        serde_json::json!({"id": model_id, "name": model_id}),
                    )
                }
            }
            other => respond_ok(id, other),
        };
        if let Some(record) = reply {
            let _ = writeln!(stdout, "{record}");
        }
        let _ = stdout.flush();
    }
}

fn respond(
    id: Option<serde_json::Value>,
    command: &str,
    data: serde_json::Value,
) -> Option<String> {
    let mut record = serde_json::json!({
        "type": "response",
        "command": command,
        "success": true,
        "data": data,
    });
    if let Some(id) = id {
        record["id"] = id;
    }
    Some(record.to_string())
}

fn respond_ok(id: Option<serde_json::Value>, command: &str) -> Option<String> {
    let mut record = serde_json::json!({
        "type": "response",
        "command": command,
        "success": true,
    });
    if let Some(id) = id {
        record["id"] = id;
    }
    Some(record.to_string())
}

fn respond_err(id: Option<serde_json::Value>, command: &str, error: &str) -> Option<String> {
    let mut record = serde_json::json!({
        "type": "response",
        "command": command,
        "success": false,
        "error": error,
    });
    if let Some(id) = id {
        record["id"] = id;
    }
    Some(record.to_string())
}
