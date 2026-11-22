use crate::analyzer::{analyze_path, report_exceeds_threshold, Diagnostic, FailThreshold};
use crate::physics::{evaluate_payload, get_profile, PayloadBudgetReport};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct BudgetConfig {
    pub profile: String,
    #[serde(default = "default_hz")]
    pub assumed_hz: f64,
    #[serde(default = "default_fail_on")]
    pub fail_on: String,
    pub max_diagnostics: Option<usize>,
    #[serde(default = "default_paths")]
    pub paths: Vec<String>,
    #[serde(default)]
    pub payload_checks: Vec<PayloadCheckRule>,
}

#[derive(Debug, Deserialize)]
pub struct PayloadCheckRule {
    pub name: String,
    pub bytes: usize,
    pub hz: f64,
    #[serde(default)]
    pub require_single_mtu: bool,
}

#[derive(Debug, Serialize)]
pub struct PayloadCheckResult {
    pub name: String,
    pub passed: bool,
    pub reason: String,
    pub report: PayloadBudgetReport,
}

#[derive(Debug, Serialize)]
pub struct CheckSummary {
    pub passed: bool,
    pub profile: String,
    pub fail_on: String,
    pub total_diagnostics: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub payload_results: Vec<PayloadCheckResult>,
}

fn default_hz() -> f64 {
    1.0
}
fn default_fail_on() -> String {
    "high".to_string()
}
fn default_paths() -> Vec<String> {
    vec![".".to_string()]
}

pub fn run_budget_file(config_path: &Path) -> Result<CheckSummary, String> {
    let raw = fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
    evaluate_budget_toml(&raw)
}

pub fn evaluate_budget_toml(raw_toml: &str) -> Result<CheckSummary, String> {
    let cfg: BudgetConfig =
        toml::from_str(raw_toml).map_err(|e| format!("Invalid budgets.toml: {e}"))?;
    let profile = get_profile(&cfg.profile)
        .ok_or_else(|| format!("Unknown profile '{}' in budgets.toml", cfg.profile))?;
    let threshold = FailThreshold::from_str(&cfg.fail_on)
        .ok_or_else(|| format!("Invalid fail_on '{}'", cfg.fail_on))?;

    let mut all_diags = Vec::new();
    for p in &cfg.paths {
        let rep = analyze_path(Path::new(p), &profile, cfg.assumed_hz)?;
        all_diags.extend(rep.diagnostics);
    }

    let mut payload_results = Vec::new();
    for rule in &cfg.payload_checks {
        let rep = evaluate_payload(&profile, rule.bytes, rule.hz);
        let mut passed = !rep.exceeds_effective_capacity;
        let mut reason = rep.verdict.clone();
        if rule.require_single_mtu && rep.fragments_per_message > 1 {
            passed = false;
            reason = format!(
                "FAIL (MTU): {} B splits into {} fragments (max MTU {} B)",
                rule.bytes, rep.fragments_per_message, profile.max_payload_bytes
            );
        }
        payload_results.push(PayloadCheckResult {
            name: rule.name.clone(),
            passed,
            reason,
            report: rep,
        });
    }

    let ast_failed = report_exceeds_threshold(&all_diags, threshold);
    let count_failed = cfg
        .max_diagnostics
        .map(|cap| all_diags.len() > cap)
        .unwrap_or(false);
    let payload_failed = payload_results.iter().any(|r| !r.passed);

    Ok(CheckSummary {
        passed: !ast_failed && !count_failed && !payload_failed,
        profile: cfg.profile,
        fail_on: cfg.fail_on,
        total_diagnostics: all_diags.len(),
        diagnostics: all_diags,
        payload_results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_toml_evaluates_payload_limits() {
        let toml_pass = r#"
profile = "lora-sf12"
paths = []

[[payload_checks]]
name = "compact_beacon"
bytes = 20
hz = 0.2
require_single_mtu = true
"#;
        let res = evaluate_budget_toml(toml_pass).unwrap();
        assert!(res.passed);

        let toml_fail = r#"
profile = "lora-sf12"
paths = []

[[payload_checks]]
name = "oversized_beacon"
bytes = 60
hz = 0.1
require_single_mtu = true
"#;
        let res_fail = evaluate_budget_toml(toml_fail).unwrap();
        assert!(!res_fail.passed);
    }
}