//! MCP permission proxy — hidden subcommand for the `nexus` binary.
//!
//! The Claude CLI spawns this as an MCP stdio server. When the CLI wants to
//! execute a dangerous tool (Bash, Edit, Write, etc.), it calls our single
//! `permission_prompt` tool. We forward the request over TCP to the Nexus UI,
//! which shows a permission dialog and returns Allow/Deny.
//!
//! Protocol:
//!   CLI ←JSON-RPC 2.0 stdio→ this process ←JSON line TCP→ Nexus UI

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;

/// Run the MCP permission proxy. Blocks forever (CLI manages our lifetime).
pub fn run(port: u16) -> ! {
    eprintln!("[mcp-proxy] started, port={port}");
    let stdin = io::stdin();
    let reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[mcp-proxy] stdin read error: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        eprintln!("[mcp-proxy] recv: {}", &line[..line.len().min(200)]);

        let (id, method, msg) = match parse_request(&line) {
            Some((id, method, msg)) => (id, method, msg),
            None => {
                // Either invalid JSON or notification (no id)
                eprintln!("[mcp-proxy] skipping line (parse error or notification)");
                continue;
            }
        };

        match method.as_str() {
            "initialize" => {
                eprintln!("[mcp-proxy] -> initialize");
                let resp = build_initialize_response(&id);
                write_response(&mut out, &resp);
            }

            "tools/list" => {
                eprintln!("[mcp-proxy] -> tools/list");
                let resp = build_tools_list_response(&id);
                write_response(&mut out, &resp);
            }

            "tools/call" => {
                let args = msg
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));

                eprintln!("[mcp-proxy] -> tools/call args={}", serde_json::to_string(&args).unwrap_or_default().chars().take(500).collect::<String>());

                // Extract the original tool input (CLI sends "input", not "tool_input")
                let tool_input = args.get("input").cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));

                eprintln!("[mcp-proxy] tool_input keys: {:?}", tool_input.as_object().map(|o| o.keys().collect::<Vec<_>>()));

                let ui_resp = match ask_ui(port, &args) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[mcp-proxy] TCP error: {e}");
                        UiResponse { allow: false, updated_input: None }
                    }
                };

                let result_text = build_permission_result(ui_resp.allow, tool_input, ui_resp.updated_input);
                eprintln!("[mcp-proxy] <- response: {}", result_text);

                let resp = build_tools_call_response(&id, &result_text);
                write_response(&mut out, &resp);
            }

            _ => {
                eprintln!("[mcp-proxy] unknown method: {method}");
                let resp = build_error_response(&id, &method);
                write_response(&mut out, &resp);
            }
        }
    }

    std::process::exit(0);
}

/// Response from the Nexus UI permission server.
struct UiResponse {
    allow: bool,
    /// If present, the updated tool input (e.g. with AskUserQuestion answers injected).
    updated_input: Option<serde_json::Value>,
}

/// Send a permission request to the Nexus UI over TCP and wait for the response.
fn ask_ui(port: u16, args: &serde_json::Value) -> io::Result<UiResponse> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))?;

    // Send request as JSON line
    let request = serde_json::to_string(args).unwrap();
    writeln!(stream, "{request}")?;
    stream.flush()?;

    // Read response JSON line
    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    let resp: serde_json::Value =
        serde_json::from_str(response.trim()).unwrap_or(serde_json::json!({"allow": false}));
    let (allow, updated_input) = parse_ui_response(&resp);
    Ok(UiResponse { allow, updated_input })
}

/// Write a JSON-RPC response as a single line to stdout.
fn write_response(out: &mut impl Write, resp: &serde_json::Value) {
    let s = serde_json::to_string(resp).unwrap();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}

// ============================================================================
// Pure functions extracted for testability
// ============================================================================

/// Build the JSON-RPC response for `initialize`.
fn build_initialize_response(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "nexus_perm",
                "version": "0.1.0"
            }
        }
    })
}

/// Build the JSON-RPC response for `tools/list`.
fn build_tools_list_response(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [{
                "name": "permission_prompt",
                "description": "Prompt the user for permission to execute a tool",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": { "type": "string" },
                        "input": { "type": "object" },
                        "tool_use_id": { "type": "string" }
                    },
                    "required": ["tool_name", "input"]
                }
            }]
        }
    })
}

/// Build a JSON-RPC error response for an unknown method.
fn build_error_response(id: &serde_json::Value, method: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!("Unknown method: {method}")
        }
    })
}

/// Build the permission result for a tools/call response.
/// Returns the inner "text" content (behavior + updatedInput or message).
fn build_permission_result(allow: bool, tool_input: serde_json::Value, updated_input: Option<serde_json::Value>) -> serde_json::Value {
    if allow {
        let updated = updated_input.unwrap_or(tool_input);
        serde_json::json!({ "behavior": "allow", "updatedInput": updated })
    } else {
        serde_json::json!({ "behavior": "deny", "message": "User denied permission" })
    }
}

/// Build the full JSON-RPC response for a tools/call result.
fn build_tools_call_response(id: &serde_json::Value, result_text: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": result_text.to_string()
            }]
        }
    })
}

/// Parse a JSON line into a request, extracting id and method.
/// Returns None for notifications (no id) or parse errors.
fn parse_request(line: &str) -> Option<(serde_json::Value, String, serde_json::Value)> {
    let msg: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    Some((id, method, msg))
}

/// Parse a UI response JSON into UiResponse fields.
fn parse_ui_response(json: &serde_json::Value) -> (bool, Option<serde_json::Value>) {
    let allow = json.get("allow").and_then(|v| v.as_bool()).unwrap_or(false);
    let updated_input = json.get("updatedInput").cloned();
    (allow, updated_input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------------------------------------------------------------------------
    // build_initialize_response tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_initialize_response_structure() {
        let id = json!(1);
        let resp = build_initialize_response(&id);

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
        assert_eq!(resp["result"]["serverInfo"]["name"], "nexus_perm");
    }

    #[test]
    fn test_initialize_response_with_string_id() {
        let id = json!("abc-123");
        let resp = build_initialize_response(&id);

        assert_eq!(resp["id"], "abc-123");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    // -------------------------------------------------------------------------
    // build_tools_list_response tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tools_list_response_structure() {
        let id = json!(2);
        let resp = build_tools_list_response(&id);

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 2);

        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "permission_prompt");
    }

    #[test]
    fn test_tools_list_has_input_schema() {
        let id = json!(1);
        let resp = build_tools_list_response(&id);

        let schema = &resp["result"]["tools"][0]["inputSchema"];
        assert_eq!(schema["type"], "object");

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("tool_name")));
        assert!(required.contains(&json!("input")));
    }

    // -------------------------------------------------------------------------
    // build_error_response tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_error_response_unknown_method() {
        let id = json!(5);
        let resp = build_error_response(&id, "foo/bar");

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 5);
        assert_eq!(resp["error"]["code"], -32601);
        assert!(resp["error"]["message"].as_str().unwrap().contains("foo/bar"));
    }

    #[test]
    fn test_error_response_empty_method() {
        let id = json!(1);
        let resp = build_error_response(&id, "");

        assert!(resp["error"]["message"].as_str().unwrap().contains("Unknown method"));
    }

    // -------------------------------------------------------------------------
    // build_permission_result tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_permission_result_allow_no_update() {
        let tool_input = json!({"command": "ls -la"});
        let result = build_permission_result(true, tool_input.clone(), None);

        assert_eq!(result["behavior"], "allow");
        assert_eq!(result["updatedInput"], tool_input);
    }

    #[test]
    fn test_permission_result_allow_with_update() {
        let tool_input = json!({"command": "ls"});
        let updated = json!({"command": "ls -la", "extra": true});
        let result = build_permission_result(true, tool_input, Some(updated.clone()));

        assert_eq!(result["behavior"], "allow");
        assert_eq!(result["updatedInput"], updated);
    }

    #[test]
    fn test_permission_result_deny() {
        let tool_input = json!({"command": "rm -rf /"});
        let result = build_permission_result(false, tool_input, None);

        assert_eq!(result["behavior"], "deny");
        assert_eq!(result["message"], "User denied permission");
        assert!(result.get("updatedInput").is_none());
    }

    // -------------------------------------------------------------------------
    // build_tools_call_response tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tools_call_response_structure() {
        let id = json!(10);
        let result_text = json!({"behavior": "allow", "updatedInput": {}});
        let resp = build_tools_call_response(&id, &result_text);

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 10);

        let content = resp["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn test_tools_call_response_text_is_stringified() {
        let id = json!(1);
        let result_text = json!({"behavior": "deny", "message": "nope"});
        let resp = build_tools_call_response(&id, &result_text);

        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        // The text field should be the JSON stringified
        assert!(text.contains("deny"));
        assert!(text.contains("nope"));
    }

    // -------------------------------------------------------------------------
    // parse_request tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_request_valid() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let result = parse_request(line);

        assert!(result.is_some());
        let (id, method, _) = result.unwrap();
        assert_eq!(id, json!(1));
        assert_eq!(method, "initialize");
    }

    #[test]
    fn test_parse_request_string_id() {
        let line = r#"{"jsonrpc":"2.0","id":"abc","method":"tools/list"}"#;
        let result = parse_request(line);

        assert!(result.is_some());
        let (id, method, _) = result.unwrap();
        assert_eq!(id, json!("abc"));
        assert_eq!(method, "tools/list");
    }

    #[test]
    fn test_parse_request_notification_no_id() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let result = parse_request(line);

        assert!(result.is_none());
    }

    #[test]
    fn test_parse_request_invalid_json() {
        let line = "not valid json";
        let result = parse_request(line);

        assert!(result.is_none());
    }

    #[test]
    fn test_parse_request_missing_method() {
        let line = r#"{"jsonrpc":"2.0","id":1}"#;
        let result = parse_request(line);

        assert!(result.is_some());
        let (_, method, _) = result.unwrap();
        assert_eq!(method, "");
    }

    // -------------------------------------------------------------------------
    // parse_ui_response tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_ui_response_allow_true() {
        let resp = json!({"allow": true});
        let (allow, updated) = parse_ui_response(&resp);

        assert!(allow);
        assert!(updated.is_none());
    }

    #[test]
    fn test_parse_ui_response_allow_false() {
        let resp = json!({"allow": false});
        let (allow, updated) = parse_ui_response(&resp);

        assert!(!allow);
        assert!(updated.is_none());
    }

    #[test]
    fn test_parse_ui_response_with_updated_input() {
        let resp = json!({
            "allow": true,
            "updatedInput": {"command": "ls", "extra": "value"}
        });
        let (allow, updated) = parse_ui_response(&resp);

        assert!(allow);
        assert!(updated.is_some());
        assert_eq!(updated.unwrap()["command"], "ls");
    }

    #[test]
    fn test_parse_ui_response_missing_allow_defaults_false() {
        let resp = json!({});
        let (allow, _) = parse_ui_response(&resp);

        assert!(!allow);
    }

    #[test]
    fn test_parse_ui_response_invalid_allow_type() {
        let resp = json!({"allow": "yes"});
        let (allow, _) = parse_ui_response(&resp);

        // Non-boolean "allow" should default to false
        assert!(!allow);
    }
}
