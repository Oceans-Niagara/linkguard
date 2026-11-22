#[path = "../../src/physics.rs"]
mod physics;

use physics::{builtin_profiles, evaluate_payload, get_profile, LinkProfile};
use std::borrow::Cow;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn list_profiles_json() -> String {
    serde_json::to_string(&builtin_profiles()).unwrap_or_else(|_| "[]".into())
}

#[wasm_bindgen]
pub fn evaluate_payload_json(profile_id: &str, payload_bytes: usize, hz: f64) -> String {
    match get_profile(profile_id) {
        Some(prof) => {
            let report = evaluate_payload(&prof, payload_bytes, hz);
            serde_json::to_string(&report).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
        }
        None => format!("{{\"error\":\"Unknown profile: {profile_id}\"}}"),
    }
}

#[wasm_bindgen]
pub fn evaluate_custom_json(
    bandwidth_hz: f64,
    snr_db: f64,
    effective_goodput_bps: f64,
    max_payload_bytes: usize,
    header_overhead_bytes: usize,
    base_latency_ms: f64,
    payload_bytes: usize,
    hz: f64,
) -> String {
    let prof = LinkProfile {
        id: Cow::Borrowed("custom"),
        name: Cow::Borrowed("Custom Sandbox Link"),
        bandwidth_hz: bandwidth_hz.max(1.0),
        snr_db,
        effective_goodput_bps: effective_goodput_bps.max(1.0),
        max_payload_bytes: max_payload_bytes.max(1),
        header_overhead_bytes,
        base_latency_ms: base_latency_ms.max(0.0),
        jitter_ms: 0.0,
        queue_capacity_bytes: 4096,
        notes: Cow::Borrowed(""),
    };
    let report = evaluate_payload(&prof, payload_bytes, hz);
    serde_json::to_string(&report).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}