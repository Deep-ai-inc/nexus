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

        let msg: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp-proxy] JSON parse error: {e}");
                continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // Notifications (no id) — ignore silently
        if id.is_none() {
            eprintln!("[mcp-proxy] notification (no id): {method}");
            continue;
        }
        let id = id.unwrap();

        match method {
            "initialize" => {
                eprintln!("[mcp-proxy] -> initialize");
                let resp = serde_json::json!({
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
                });
                write_response(&mut out, &resp);
            }

            "tools/list" => {
                eprintln!("[mcp-proxy] -> tools/list");
                let resp = serde_json::json!({
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
                });
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

                let allowed = match ask_ui(port, &args) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[mcp-proxy] TCP error: {e}");
                        false
                    }
                };

                // CLI schema (zod discriminated union):
                //   Allow: {"behavior": "allow", "updatedInput": {<original tool input>}}
                //   Deny:  {"behavior": "deny", "message": "reason"}
                let result_text = if allowed {
                    serde_json::json!({ "behavior": "allow", "updatedInput": tool_input })
                } else {
                    serde_json::json!({ "behavior": "deny", "message": "User denied permission" })
                };

                eprintln!("[mcp-proxy] <- response: {}", result_text);

                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{
                            "type": "text",
                            "text": result_text.to_string()
                        }]
                    }
                });
                write_response(&mut out, &resp);
            }

            _ => {
                eprintln!("[mcp-proxy] unknown method: {method}");
                // Unknown method — return error
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Unknown method: {method}")
                    }
                });
                write_response(&mut out, &resp);
            }
        }
    }

    std::process::exit(0);
}

/// Send a permission request to the Nexus UI over TCP and wait for the response.
fn ask_ui(port: u16, args: &serde_json::Value) -> io::Result<bool> {
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
    Ok(resp.get("allow").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Write a JSON-RPC response as a single line to stdout.
fn write_response(out: &mut impl Write, resp: &serde_json::Value) {
    let s = serde_json::to_string(resp).unwrap();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}
