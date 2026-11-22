use crate::analyzer::{analyze_path, analyze_source, Lang};
use crate::physics::{builtin_profiles, evaluate_payload, get_profile};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;

pub fn run_mcp_stdio() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line_res in stdin.lock().lines() {
        let line = line_res.map_err(|e| e.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("Parse error: {e}") }
                });
                writeln!(stdout, "{err}").map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
                continue;
            }
        };

        let Some(id) = req.get("id").cloned() else {
            continue;
        };

        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        let resp = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "linkguard",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "list_link_profiles",
                            "description": "List built-in physical-layer RF/serial link profiles (LoRa SF12, BLE 1M, congested Wi-Fi, LEO sat, UART 9600).",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "evaluate_payload",
                            "description": "Compute Shannon-Hartley ceiling, protocol goodput utilization, MTU fragmentation, airtime, and BER loss probability for a payload size and send rate.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "profile": { "type": "string", "description": "Profile ID (e.g. lora-sf12, ble-1m, wifi-24g-congested, leo-sat, uart-9600)" },
                                    "payload_bytes": { "type": "integer", "minimum": 1 },
                                    "hz": { "type": "number", "minimum": 0.001 }
                                },
                                "required": ["profile", "payload_bytes", "hz"]
                            }
                        },
                        {
                            "name": "analyze_code",
                            "description": "Run deterministic tree-sitter AST + physical-layer link rules on Python or Rust source code or a file/directory path.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "File or directory path on disk (e.g. './bad_telemetry.py'). Use this for existing files." },
                                    "code": { "type": "string", "description": "Raw source code text ONLY (do not pass a filename here)." },
                                    "language": { "type": "string", "enum": ["python", "rust"] },
                                    "profile": { "type": "string", "default": "lora-sf12" },
                                    "hz": { "type": "number", "default": 10.0 }
                                },
                                "required": ["profile"]
                            }
                        }
                    ]
                }
            }),
            "tools/call" => handle_tool_call(id, &params),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }),
        };

        writeln!(stdout, "{resp}").map_err(|e| e.to_string())?;
        stdout.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn handle_tool_call(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let result = match name {
        "list_link_profiles" => Ok(json!(builtin_profiles())),
        "evaluate_payload" => {
            let pid = args.get("profile").and_then(|v| v.as_str()).unwrap_or("lora-sf12");
            let bytes = args.get("payload_bytes").and_then(|v| v.as_u64()).unwrap_or(64) as usize;
            let hz = args.get("hz").and_then(|v| v.as_f64()).unwrap_or(1.0);
            match get_profile(pid) {
                Some(p) => Ok(json!(evaluate_payload(&p, bytes, hz))),
                None => Err(format!("Unknown profile: {pid}")),
            }
        }
        "analyze_code" => {
            let pid = args.get("profile").and_then(|v| v.as_str()).unwrap_or("lora-sf12");
            let hz = args.get("hz").and_then(|v| v.as_f64()).unwrap_or(10.0);
            match get_profile(pid) {
                None => Err(format!("Unknown profile: {pid}")),
                Some(profile) => {
                    let explicit_path = args.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let raw_code = args.get("code").and_then(|v| v.as_str());

                    let inferred_path = if explicit_path.is_none() {
                        raw_code.and_then(|c| {
                            let trimmed = c.trim();
                            if !trimmed.contains('\n') {
                                let stripped = trimmed
                                    .strip_prefix("file=")
                                    .or_else(|| trimmed.strip_prefix("path="))
                                    .unwrap_or(trimmed);
                                if Path::new(stripped).exists() {
                                    return Some(stripped.to_string());
                                }
                            }
                            None
                        })
                    } else {
                        None
                    };

                    if let Some(path_str) = explicit_path.or(inferred_path) {
                        analyze_path(Path::new(&path_str), &profile, hz).map(|r| {
                            let count = r.diagnostics.len();
                            json!({
                                "status": if count == 0 { "PASS" } else { "FAIL" },
                                "summary": format!("{count} issue(s) found against '{}'.", profile.id),
                                "target_profile": r.target_profile,
                                "files_scanned": r.files_scanned,
                                "diagnostics": r.diagnostics
                            })
                        })
                    } else if let Some(code) = raw_code {
                        let lang_str = args.get("language").and_then(|v| v.as_str()).unwrap_or("python");
                        match Lang::from_name(lang_str) {
                            None => Err(format!("Unsupported language: {lang_str}")),
                            Some(lang) => analyze_source(code, lang, "<inline>", &profile, hz).map(|diags| {
                                let count = diags.len();
                                json!({
                                    "status": if count == 0 { "PASS" } else { "FAIL" },
                                    "summary": format!("{count} issue(s) found against '{}'.", profile.id),
                                    "target_profile": profile.id,
                                    "language": lang.as_str(),
                                    "diagnostics": diags
                                })
                            }),
                        }
                    } else {
                        Err("Provide either 'path' or 'code'".into())
                    }
                }
            }
        }
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(val) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [
                    { "type": "text", "text": serde_json::to_string_pretty(&val).unwrap_or_default() }
                ]
            }
        }),
        Err(msg) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "isError": true,
                "content": [{ "type": "text", "text": msg }]
            }
        }),
    }
}