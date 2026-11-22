#[cfg(not(target_arch = "wasm32"))]
use crate::license::require_pro;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkProfile {
    pub id: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub bandwidth_hz: f64,
    pub snr_db: f64,
    /// Realistic protocol payload ceiling after MAC/framing/duty-cycle limits
    pub effective_goodput_bps: f64,
    pub max_payload_bytes: usize,
    pub header_overhead_bytes: usize,
    pub base_latency_ms: f64,
    #[serde(default)]
    pub jitter_ms: f64,
    pub queue_capacity_bytes: usize,
    #[serde(default)]
    pub notes: Cow<'static, str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadBudgetReport {
    pub verdict: String,
    pub profile_id: String,
    pub payload_bytes: usize,
    pub assumed_hz: f64,
    pub fragments_per_message: usize,
    pub wire_bytes_per_message: usize,
    pub header_overhead_pct: f64,
    pub required_wire_bps: f64,
    pub effective_goodput_bps: f64,
    pub shannon_ceiling_bps: f64,
    pub utilization_pct: f64,
    pub exceeds_effective_capacity: bool,
    pub exceeds_shannon_ceiling: bool,
    pub airtime_ms_per_message: f64,
    pub max_sustainable_hz: f64,
    pub estimated_ber: f64,
    pub packet_loss_prob_pct: f64,
}

pub fn builtin_profiles() -> Vec<LinkProfile> {
    vec![
        LinkProfile {
            id: Cow::Borrowed("lora-sf12"),
            name: Cow::Borrowed("LoRa SF12 (125 kHz, Long-Range)"),
            bandwidth_hz: 125_000.0,
            snr_db: -17.5,
            effective_goodput_bps: 293.0,
            max_payload_bytes: 51,
            header_overhead_bytes: 13,
            base_latency_ms: 400.0,
            jitter_ms: 40.0,
            queue_capacity_bytes: 512,
            notes: Cow::Borrowed(
                "DR0 max 51B payload; 13B LoRaWAN MAC header; regional duty-cycle limits apply.",
            ),
        },
        LinkProfile {
            id: Cow::Borrowed("ble-1m"),
            name: Cow::Borrowed("BLE 1M PHY (ATT/GATT)"),
            bandwidth_hz: 1_000_000.0,
            snr_db: 14.0,
            effective_goodput_bps: 700_000.0,
            max_payload_bytes: 247,
            header_overhead_bytes: 10,
            base_latency_ms: 15.0,
            jitter_ms: 5.0,
            queue_capacity_bytes: 4_096,
            notes: Cow::Borrowed(
                "15ms connection-interval floor; 247B max ATT MTU (20B on legacy peers).",
            ),
        },
        LinkProfile {
            id: Cow::Borrowed("wifi-24g-congested"),
            name: Cow::Borrowed("Congested 2.4 GHz Wi-Fi"),
            bandwidth_hz: 20_000_000.0,
            snr_db: 11.0,
            effective_goodput_bps: 4_500_000.0,
            max_payload_bytes: 1_460,
            header_overhead_bytes: 64,
            base_latency_ms: 28.0,
            jitter_ms: 35.0,
            queue_capacity_bytes: 16_384,
            notes: Cow::Borrowed(
                "Heavy CSMA/CA airtime contention, TCP/IP + MAC headers, tail-drop bufferbloat.",
            ),
        },
        LinkProfile {
            id: Cow::Borrowed("leo-sat"),
            name: Cow::Borrowed("LEO Satellite Link (Ka-Band)"),
            bandwidth_hz: 50_000_000.0,
            snr_db: 8.5,
            effective_goodput_bps: 12_000_000.0,
            max_payload_bytes: 1_460,
            header_overhead_bytes: 54,
            base_latency_ms: 45.0,
            jitter_ms: 18.0,
            queue_capacity_bytes: 65_536,
            notes: Cow::Borrowed(
                "High bandwidth-delay product; beam handoffs and rain fade spike jitter/loss.",
            ),
        },
        LinkProfile {
            id: Cow::Borrowed("uart-9600"),
            name: Cow::Borrowed("UART Serial (9600 baud 8N1)"),
            bandwidth_hz: 9_600.0,
            snr_db: 30.0,
            effective_goodput_bps: 7_680.0,
            max_payload_bytes: 64,
            header_overhead_bytes: 2,
            base_latency_ms: 2.0,
            jitter_ms: 1.0,
            queue_capacity_bytes: 256,
            notes: Cow::Borrowed(
                "8N1 framing uses 10 bits per byte (max 960 B/s); small hardware FIFO overflows easily.",
            ),
        },
    ]
}

pub fn get_profile(id: &str) -> Option<LinkProfile> {
    builtin_profiles().into_iter().find(|p| p.id == id)
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Deserialize)]
struct CustomProfilesFile {
    #[serde(default, rename = "profile")]
    profiles: Vec<LinkProfile>,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_custom_profiles(path: &Path) -> Result<Vec<LinkProfile>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let parsed: CustomProfilesFile =
        toml::from_str(&raw).map_err(|e| format!("Invalid {}: {e}", path.display()))?;
    if parsed.profiles.is_empty() {
        return Err(format!("No [[profile]] entries found in {}", path.display()));
    }
    Ok(parsed.profiles)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn all_profiles(custom_path: Option<&Path>) -> Result<Vec<LinkProfile>, String> {
    let mut profiles = builtin_profiles();
    let resolved_path = match custom_path {
        Some(p) => Some(p),
        None if Path::new("profiles.toml").is_file() => Some(Path::new("profiles.toml")),
        None => None,
    };

    if let Some(path) = resolved_path {
        require_pro("Custom profiles.toml")?;
        let custom = load_custom_profiles(path)?;
        for cp in custom {
            if let Some(existing) = profiles.iter_mut().find(|p| p.id == cp.id) {
                *existing = cp;
            } else {
                profiles.push(cp);
            }
        }
    }
    Ok(profiles)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_profile(id: &str, custom_path: Option<&Path>) -> Result<LinkProfile, String> {
    all_profiles(custom_path)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Unknown profile '{id}'. Run `linkguard profiles` to list options."))
}

/// Shannon-Hartley theorem: C = B * log2(1 + SNR_linear)
pub fn shannon_capacity_bps(bandwidth_hz: f64, snr_db: f64) -> f64 {
    let snr_linear = 10.0_f64.powf(snr_db / 10.0);
    bandwidth_hz * (1.0 + snr_linear).log2()
}

/// Pure-Rust Abramowitz & Stegun 7.1.26 approximation of erfc(x)
fn erfc_approx(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let poly = t
        * (0.254829592
            + t * (-0.284496736
                + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    let res = poly * (-x * x).exp();
    if x >= 0.0 {
        res
    } else {
        2.0 - res
    }
}

/// Approximate post-FEC BER from SNR
pub fn estimate_ber_from_snr(snr_db: f64) -> f64 {
    if snr_db < -10.0 {
        // LoRa SF12 CSS processing + FEC gain (~25 dB effective margin)
        let effective_snr_lin = 10.0_f64.powf((snr_db + 25.0) / 10.0);
        return (0.5 * erfc_approx(effective_snr_lin.sqrt())).clamp(1e-6, 0.25);
    }
    let snr_lin = 10.0_f64.powf(snr_db / 10.0);
    (0.5 * erfc_approx((snr_lin * 0.5).sqrt())).clamp(1e-8, 0.25)
}

pub fn evaluate_payload(
    profile: &LinkProfile,
    payload_bytes: usize,
    assumed_hz: f64,
) -> PayloadBudgetReport {
    let p = payload_bytes.max(1);
    let mtu = profile.max_payload_bytes.max(1);
    let fragments = p.div_ceil(mtu);
    let wire_bytes = p + fragments * profile.header_overhead_bytes;
    let header_pct = ((fragments * profile.header_overhead_bytes) as f64 / wire_bytes as f64) * 100.0;

    let required_wire_bps = (wire_bytes as f64) * 8.0 * assumed_hz;
    let shannon_bps = shannon_capacity_bps(profile.bandwidth_hz, profile.snr_db);
    let utilization_pct = (required_wire_bps / profile.effective_goodput_bps) * 100.0;

    let tx_ms = ((wire_bytes as f64 * 8.0) / profile.effective_goodput_bps) * 1000.0;
    let airtime_ms = tx_ms + profile.base_latency_ms;
    let max_hz = profile.effective_goodput_bps / ((wire_bytes as f64) * 8.0);

    let ber = estimate_ber_from_snr(profile.snr_db);
    let packet_loss_prob = 1.0 - (1.0 - ber).powi((wire_bytes * 8) as i32);

    let verdict = if required_wire_bps > profile.effective_goodput_bps {
        format!(
            "FAIL (OVER CAPACITY): {} B at {} Hz needs {:.0} bps ({:.1}x over {}'s {:.0} bps limit). Shrink payload or slow to <= {:.2} Hz.",
            p,
            assumed_hz,
            required_wire_bps,
            required_wire_bps / profile.effective_goodput_bps,
            profile.id,
            profile.effective_goodput_bps,
            max_hz
        )
    } else if fragments > 1 {
        format!(
            "WARN (FRAGMENTED): Fits bandwidth ({:.1}% utilization), but {} B splits into {} MTU fragments ({} B max) raising packet loss to {:.2}%.",
            utilization_pct,
            p,
            fragments,
            mtu,
            packet_loss_prob * 100.0
        )
    } else {
        format!(
            "PASS: Fits in 1 MTU fragment ({} B <= {} B) at {:.1}% capacity ({:.0}/{:.0} bps).",
            p, mtu, utilization_pct, required_wire_bps, profile.effective_goodput_bps
        )
    };

    PayloadBudgetReport {
        verdict,
        profile_id: profile.id.to_string(),
        payload_bytes: p,
        assumed_hz,
        fragments_per_message: fragments,
        wire_bytes_per_message: wire_bytes,
        header_overhead_pct: round2(header_pct),
        required_wire_bps: round2(required_wire_bps),
        effective_goodput_bps: profile.effective_goodput_bps,
        shannon_ceiling_bps: round2(shannon_bps),
        utilization_pct: round2(utilization_pct),
        exceeds_effective_capacity: required_wire_bps > profile.effective_goodput_bps,
        exceeds_shannon_ceiling: required_wire_bps > shannon_bps,
        airtime_ms_per_message: round2(airtime_ms),
        max_sustainable_hz: round2(max_hz),
        estimated_ber: ber,
        packet_loss_prob_pct: round2(packet_loss_prob * 100.0),
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shannon_and_ber_are_sane() {
        let cap = shannon_capacity_bps(125_000.0, -17.5);
        assert!(cap > 3_000.0 && cap < 3_500.0);

        let lora_ber = estimate_ber_from_snr(-17.5);
        assert!(lora_ber > 1e-5 && lora_ber < 1e-2);

        let uart_ber = estimate_ber_from_snr(30.0);
        assert!(uart_ber <= 1e-7);
    }

    #[test]
    fn budget_verdict_distinguishes_pass_warn_fail() {
        let lora = get_profile("lora-sf12").unwrap();
        let pass = evaluate_payload(&lora, 20, 0.25);
        assert!(pass.verdict.starts_with("PASS"));
        assert!(!pass.exceeds_effective_capacity);
        assert_eq!(pass.fragments_per_message, 1);

        let warn = evaluate_payload(&lora, 80, 0.1);
        assert!(warn.verdict.starts_with("WARN"));
        assert_eq!(warn.fragments_per_message, 2);

        let fail = evaluate_payload(&lora, 96, 10.0);
        assert!(fail.verdict.starts_with("FAIL"));
        assert!(fail.exceeds_effective_capacity);
    }
}