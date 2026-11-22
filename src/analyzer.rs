use crate::physics::{evaluate_payload, LinkProfile, PayloadBudgetReport};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    Rust,
}

impl Lang {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("py") => Some(Self::Python),
            Some("rs") => Some(Self::Rust),
            _ => None,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "py" | "python" => Some(Self::Python),
            "rs" | "rust" => Some(Self::Rust),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Rust => "rust",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailThreshold {
    None = 0,
    Critical = 1,
    High = 2,
    Medium = 3,
}

impl FailThreshold {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" => Some(Self::None),
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            _ => None,
        }
    }
}

pub fn severity_rank(sev: &str) -> u8 {
    match sev {
        "critical" => 1,
        "high" => 2,
        "medium" => 3,
        _ => 4,
    }
}

pub fn report_exceeds_threshold(diags: &[Diagnostic], threshold: FailThreshold) -> bool {
    if threshold == FailThreshold::None {
        return false;
    }
    let max_rank = threshold as u8;
    diags.iter().any(|d| severity_rank(&d.severity) <= max_rank)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub rule_id: String,
    pub severity: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub snippet: String,
    pub explanation: String,
    pub physics_law: String,
    pub recommended_fix: String,
    pub budget: Option<PayloadBudgetReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub target_profile: String,
    pub files_scanned: usize,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Default)]
struct WalkContext {
    in_loop: bool,
    in_async: bool,
    in_batch_guard: bool,
    loop_hz_inferred: Option<f64>,
    has_timeout_in_file: bool,
}

pub fn analyze_path(
    path: &Path,
    profile: &LinkProfile,
    assumed_hz: f64,
) -> Result<AnalysisReport, String> {
    let mut files = Vec::new();
    collect_files(path, &mut files)?;

    let mut diagnostics = Vec::new();
    for file in &files {
        if let Some(lang) = Lang::from_path(file) {
            let src = fs::read_to_string(file)
                .map_err(|e| format!("Failed to read {}: {}", file.display(), e))?;
            let file_str = file.display().to_string();
            diagnostics.extend(analyze_source(&src, lang, &file_str, profile, assumed_hz)?);
        }
    }

    Ok(AnalysisReport {
        target_profile: profile.id.to_string(),
        files_scanned: files.len(),
        diagnostics,
    })
}

pub fn analyze_source(
    source: &str,
    lang: Lang,
    file_label: &str,
    profile: &LinkProfile,
    assumed_hz: f64,
) -> Result<Vec<Diagnostic>, String> {
    let mut parser = Parser::new();
    match lang {
        Lang::Python => parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .map_err(|e| format!("Python grammar error: {e}"))?,
        Lang::Rust => parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| format!("Rust grammar error: {e}"))?,
    }

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "Tree-sitter failed to parse source".to_string())?;

    let has_timeout_in_file = source.contains("settimeout")
        || source.contains("set_read_timeout")
        || source.contains("set_write_timeout")
        || source.contains("tokio::time::timeout");

    let mut diags = Vec::new();
    let ctx = WalkContext {
        in_loop: false,
        in_async: false,
        in_batch_guard: false,
        loop_hz_inferred: None,
        has_timeout_in_file,
    };

    walk_node(
        tree.root_node(),
        source,
        lang,
        file_label,
        profile,
        assumed_hz,
        ctx,
        &mut diags,
    );
    Ok(diags)
}

fn collect_files(path: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    if path.is_file() {
        if Lang::from_path(path).is_some() {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name == "target" || name == "node_modules" || name.starts_with('.') {
                    continue;
                }
            }
            collect_files(&p, out)?;
        }
    }
    Ok(())
}

fn is_loop_kind(kind: &str) -> bool {
    matches!(
        kind,
        "for_statement" | "while_statement" | "for_expression" | "while_expression" | "loop_expression"
    )
}

fn is_batch_if_guard(node: Node, source: &str) -> bool {
    let kind = node.kind();
    if kind != "if_statement" && kind != "if_expression" {
        return false;
    }
    if let Some(cond) = node.child_by_field_name("condition") {
        let cond_txt = cond.utf8_text(source.as_bytes()).unwrap_or("");
        let has_len = cond_txt.contains("len(") || cond_txt.contains(".len()");
        let has_cmp = cond_txt.contains(">=") || cond_txt.contains("==") || cond_txt.contains('>');
        return has_len && has_cmp;
    }
    false
}

/// Scan the immediate loop body for `sleep(...)` literals and infer Hz = 1 / seconds.
fn infer_loop_hz(loop_node: Node, source: &str, lang: Lang) -> Option<f64> {
    fn scan(node: Node, source: &str, lang: Lang, is_root: bool) -> Option<f64> {
        let kind = node.kind();
        if !is_root && is_loop_kind(kind) {
            return None;
        }
        if matches!(kind, "call" | "call_expression") {
            let text = node.utf8_text(source.as_bytes()).unwrap_or("");
            let callee = node
                .child_by_field_name("function")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .unwrap_or("");

            match lang {
                Lang::Python => {
                    if callee.ends_with("time.sleep") || callee.ends_with("asyncio.sleep") {
                        if let Some(secs) = extract_single_float_in_parens(text) {
                            if secs > 0.0 {
                                return Some((1.0 / secs).clamp(0.001, 100_000.0));
                            }
                        }
                    }
                }
                Lang::Rust => {
                    if callee.ends_with("thread::sleep") || callee.ends_with("time::sleep") {
                        if let Some(secs) = extract_rust_duration_secs(text) {
                            if secs > 0.0 {
                                return Some((1.0 / secs).clamp(0.001, 100_000.0));
                            }
                        }
                    }
                }
            }
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if let Some(hz) = scan(child, source, lang, false) {
                    return Some(hz);
                }
            }
        }
        None
    }

    scan(loop_node, source, lang, true)
}

fn extract_single_float_in_parens(call_text: &str) -> Option<f64> {
    let open = call_text.rfind('(')?;
    let close = call_text[open + 1..].find(')')?;
    let inner = call_text[open + 1..open + 1 + close].trim();
    inner.parse::<f64>().ok()
}

fn extract_rust_duration_secs(text: &str) -> Option<f64> {
    if let Some(pos) = text.find("from_millis(") {
        let rest = &text[pos + "from_millis(".len()..];
        let end = rest.find(')')?;
        let ms = rest[..end].trim().replace('_', "").parse::<f64>().ok()?;
        return Some(ms / 1000.0);
    }
    if let Some(pos) = text.find("from_micros(") {
        let rest = &text[pos + "from_micros(".len()..];
        let end = rest.find(')')?;
        let us = rest[..end].trim().replace('_', "").parse::<f64>().ok()?;
        return Some(us / 1_000_000.0);
    }
    for prefix in ["from_secs_f64(", "from_secs_f32(", "from_secs("] {
        if let Some(pos) = text.find(prefix) {
            let rest = &text[pos + prefix.len()..];
            let end = rest.find(')')?;
            return rest[..end].trim().replace('_', "").parse::<f64>().ok();
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn walk_node(
    node: Node,
    source: &str,
    lang: Lang,
    file_label: &str,
    profile: &LinkProfile,
    assumed_hz: f64,
    ctx: WalkContext,
    diags: &mut Vec<Diagnostic>,
) {
    let kind = node.kind();
    let text = node.utf8_text(source.as_bytes()).unwrap_or("");

    let is_loop = is_loop_kind(kind);
    let inferred_hz = if is_loop {
        infer_loop_hz(node, source, lang).or(ctx.loop_hz_inferred)
    } else {
        ctx.loop_hz_inferred
    };

    let is_async_fn = match lang {
        Lang::Python => kind == "function_definition" && text.trim_start().starts_with("async "),
        Lang::Rust => {
            kind == "function_item"
                && text
                    .split('{')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|w| w == "async")
        }
    };

    let in_batch_guard = ctx.in_batch_guard || is_batch_if_guard(node, source);

    let next_ctx = WalkContext {
        in_loop: ctx.in_loop || is_loop,
        in_async: ctx.in_async || is_async_fn,
        in_batch_guard,
        loop_hz_inferred: inferred_hz,
        has_timeout_in_file: ctx.has_timeout_in_file,
    };

    let is_call = matches!(kind, "call" | "call_expression" | "macro_invocation");
    if is_call {
        inspect_call(
            node,
            source,
            text,
            lang,
            file_label,
            profile,
            assumed_hz,
            &next_ctx,
            diags,
        );
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_node(
                child,
                source,
                lang,
                file_label,
                profile,
                assumed_hz,
                next_ctx.clone(),
                diags,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect_call(
    node: Node,
    source: &str,
    text: &str,
    lang: Lang,
    file_label: &str,
    profile: &LinkProfile,
    assumed_hz: f64,
    ctx: &WalkContext,
    diags: &mut Vec<Diagnostic>,
) {
    let pos = node.start_position();
    let line = pos.row + 1;
    let col = pos.column + 1;
    let snippet = text.lines().next().unwrap_or(text).trim().to_string();

    let callee = node
        .child_by_field_name("function")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .unwrap_or("");

    let effective_hz = ctx.loop_hz_inferred.unwrap_or(assumed_hz);
    let hz_source = if let Some(h) = ctx.loop_hz_inferred {
        format!("{:.2} Hz (inferred from loop sleep)", (h * 100.0).round() / 100.0)
    } else {
        format!("{assumed_hz} Hz")
    };

    // Rule 1: JSON serialization inside a loop
    let is_json_ser = callee.ends_with("json.dumps")
        || callee.ends_with("serde_json::to_string")
        || callee.ends_with("serde_json::to_vec");

    if is_json_ser && ctx.in_loop {
        let estimated_json_bytes = 96;
        let budget = evaluate_payload(profile, estimated_json_bytes, effective_hz);
        let over_msg = if budget.exceeds_effective_capacity {
            format!(
                "At {hz_source}, a ~{estimated_json_bytes} B JSON frame requires {:.0} bps ({:.1}% of {}'s {:.0} bps effective goodput).",
                budget.required_wire_bps,
                budget.utilization_pct,
                profile.id,
                profile.effective_goodput_bps
            )
        } else {
            format!(
                "JSON keys/delimiters inflate small frames (~70-80% syntax tax) and trigger {} MTU fragment(s) on {}.",
                budget.fragments_per_message, profile.id
            )
        };

        diags.push(Diagnostic {
            rule_id: "JSON_IN_LOOP".into(),
            severity: if budget.exceeds_effective_capacity { "critical" } else { "high" }.into(),
            file: file_label.into(),
            line,
            column: col,
            snippet: snippet.clone(),
            explanation: format!("Text/JSON serialization inside a loop. {over_msg}"),
            physics_law: format!(
                "Information Theory & Framing Tax (Shannon ceiling: {:.0} bps, Protocol goodput: {:.0} bps, MTU: {} B)",
                budget.shannon_ceiling_bps, budget.effective_goodput_bps, profile.max_payload_bytes
            ),
            recommended_fix: match lang {
                Lang::Python => "Pack numeric fields with struct.pack() or CBOR/Protobuf and batch frames before sending.".into(),
                Lang::Rust => "Serialize with postcard or bincode into a reusable buffer instead of allocating serde_json strings per iteration.".into(),
            },
            budget: Some(budget),
        });
    }

    // Rule 2: Transport send/write inside a loop
    let is_send = callee.ends_with(".send")
        || callee.ends_with(".sendall")
        || callee.ends_with(".sendto")
        || callee.ends_with(".write")
        || callee.ends_with(".write_all")
        || callee.ends_with(".send_to");

    if is_send && ctx.in_loop {
        if let Some(lit_len) = extract_string_literal_len(text) {
            if lit_len > profile.max_payload_bytes {
                let budget = evaluate_payload(profile, lit_len, effective_hz);
                diags.push(Diagnostic {
                    rule_id: "MTU_EXCEEDED_LITERAL".into(),
                    severity: "high".into(),
                    file: file_label.into(),
                    line,
                    column: col,
                    snippet: snippet.clone(),
                    explanation: format!(
                        "Literal payload ({} B) exceeds {} MTU ({} B), splitting into {} fragments and multiplying packet-loss probability to {:.2}%.",
                        lit_len, profile.id, profile.max_payload_bytes, budget.fragments_per_message, budget.packet_loss_prob_pct
                    ),
                    physics_law: "Fragmentation & BER Amplification: P(loss) = 1 - (1 - BER)^(wire_bits)".into(),
                    recommended_fix: "Truncate, compress, or switch to binary schema so each message fits inside a single link MTU.".into(),
                    budget: Some(budget),
                });
            }
        } else if !ctx.in_batch_guard {
            let small_probe = 16;
            let budget = evaluate_payload(profile, small_probe, effective_hz);
            diags.push(Diagnostic {
                rule_id: "UNBATCHED_SEND_IN_LOOP".into(),
                severity: if budget.exceeds_effective_capacity { "high" } else { "medium" }.into(),
                file: file_label.into(),
                line,
                column: col,
                snippet: snippet.clone(),
                explanation: format!(
                    "Unbatched transport write inside a loop at {hz_source}. On {} ({} B header/frame), a 16 B payload wastes {:.1}% of airtime on headers alone.",
                    profile.id, profile.header_overhead_bytes, budget.header_overhead_pct
                ),
                physics_law: "Framing Efficiency = payload / (payload + header_overhead)".into(),
                recommended_fix: "Coalesce multiple samples into a buffered ring/batch up to the link MTU before calling send/write.".into(),
                budget: Some(budget),
            });
        }
    }

    // Rule 3: Blocking sleep / network I/O inside an async function
    let is_blocking_call = callee.ends_with("time.sleep")
        || callee.ends_with("thread::sleep")
        || callee.ends_with("requests.get")
        || callee.ends_with("requests.post");

    if is_blocking_call && ctx.in_async {
        diags.push(Diagnostic {
            rule_id: "BLOCKING_IO_IN_ASYNC".into(),
            severity: "high".into(),
            file: file_label.into(),
            line,
            column: col,
            snippet: snippet.clone(),
            explanation: "Blocking sleep or synchronous I/O inside an async function stalls the executor thread and starves sibling socket/heartbeat tasks.".into(),
            physics_law: "Queueing Delay & Executor Starvation".into(),
            recommended_fix: match lang {
                Lang::Python => "Replace with `await asyncio.sleep(...)` or an async client (aiohttp/httpx).".into(),
                Lang::Rust => "Replace with `tokio::time::sleep(...).await` or `tokio::task::spawn_blocking`.".into(),
            },
            budget: None,
        });
    }

    // Rule 4: Timeout configured shorter than physical link latency
    let timeout_ms = if callee.ends_with(".settimeout") {
        extract_single_float_in_parens(text).map(|s| s * 1000.0)
    } else if callee.ends_with(".set_read_timeout") || callee.ends_with(".set_write_timeout") {
        extract_rust_duration_secs(text).map(|s| s * 1000.0)
    } else {
        None
    };

    if let Some(t_ms) = timeout_ms {
        if t_ms > 0.0 && t_ms < profile.base_latency_ms {
            diags.push(Diagnostic {
                rule_id: "TIMEOUT_BELOW_LINK_LATENCY".into(),
                severity: "high".into(),
                file: file_label.into(),
                line,
                column: col,
                snippet: snippet.clone(),
                explanation: format!(
                    "Socket timeout ({:.1} ms) is shorter than '{}' base propagation/airtime latency ({:.0} ms)—packets will spuriously time out before arrival.",
                    t_ms, profile.id, profile.base_latency_ms
                ),
                physics_law: "Timeout Floor > Physical Base RTT + Serialization Airtime".into(),
                recommended_fix: format!(
                    "Set timeout well above {:.0} ms (e.g. >= {:.1} s for {}).",
                    profile.base_latency_ms,
                    (profile.base_latency_ms * 3.0) / 1000.0,
                    profile.id
                ),
                budget: None,
            });
        }
    }

    // Rule 5: Transport socket creation without timeout on lossy/high-latency links
    let is_socket_init = callee.ends_with("socket.socket")
        || callee.ends_with("TcpStream::connect")
        || callee.ends_with("UdpSocket::bind");

    if is_socket_init && !ctx.has_timeout_in_file && profile.base_latency_ms >= 20.0 {
        diags.push(Diagnostic {
            rule_id: "NO_TIMEOUT_ON_LOSSY_LINK".into(),
            severity: "medium".into(),
            file: file_label.into(),
            line,
            column: col,
            snippet,
            explanation: format!(
                "Socket initialized with no timeout detected in file while targeting high-latency/lossy profile '{}' (base RTT/latency {:.0} ms).",
                profile.id, profile.base_latency_ms
            ),
            physics_law: "Head-of-Line Blocking & Unbounded Stall on Lossy RF Channels".into(),
            recommended_fix: "Configure explicit read/write timeouts and bounded retry backoff.".into(),
            budget: None,
        });
    }
}

fn extract_string_literal_len(call_text: &str) -> Option<usize> {
    let start = call_text.find('"')?;
    let rest = &call_text[start + 1..];
    let end = rest.find('"')?;
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::get_profile;

    #[test]
    fn python_chained_json_emits_once_and_infers_hz() {
        let src = r#"
import json, socket, asyncio
async def run():
    s = socket.socket()
    s.settimeout(0.01)
    while True:
        p = json.dumps({"x": 1}).encode("utf-8")
        s.send(p)
        await asyncio.sleep(0.033)
"#;
        let lora = get_profile("lora-sf12").unwrap();
        let diags = analyze_source(src, Lang::Python, "test.py", &lora, 0.2).unwrap();

        let json_hits: Vec<_> = diags.iter().filter(|d| d.rule_id == "JSON_IN_LOOP").collect();
        assert_eq!(json_hits.len(), 1);
        let b = json_hits[0].budget.as_ref().unwrap();
        assert!(b.assumed_hz > 30.0);

        assert!(diags.iter().any(|d| d.rule_id == "TIMEOUT_BELOW_LINK_LATENCY"));
    }

    #[test]
    fn batch_guard_suppresses_unbatched_send() {
        let src = r#"
import socket, asyncio
async def run():
    s = socket.socket()
    s.settimeout(2.0)
    buf = []
    while True:
        buf.append(b"abc")
        if len(buf) >= 4:
            s.send(b"".join(buf))
            buf.clear()
        await asyncio.sleep(4.0)
"#;
        let lora = get_profile("lora-sf12").unwrap();
        let diags = analyze_source(src, Lang::Python, "ok.py", &lora, 1.0).unwrap();
        assert!(diags.is_empty(), "Expected 0 diagnostics, got: {diags:?}");
    }

    #[test]
    fn rust_async_blocking_and_serde_detected() {
        let src = r#"
use std::io::Write;
async fn pump(mut stream: std::net::TcpStream) {
    loop {
        let s = serde_json::to_string(&42).unwrap();
        stream.write_all(s.as_bytes()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
"#;
        let ble = get_profile("ble-1m").unwrap();
        let diags = analyze_source(src, Lang::Rust, "lib.rs", &ble, 1.0).unwrap();
        assert!(diags.iter().any(|d| d.rule_id == "JSON_IN_LOOP"));
        assert!(diags.iter().any(|d| d.rule_id == "BLOCKING_IO_IN_ASYNC"));
        let json_hit = diags.iter().find(|d| d.rule_id == "JSON_IN_LOOP").unwrap();
        assert!((json_hit.budget.as_ref().unwrap().assumed_hz - 20.0).abs() < 0.5);
    }
}