use crate::analyzer::{analyze_path, report_exceeds_threshold, Diagnostic, FailThreshold};
use crate::license::require_pro;
use crate::physics::{evaluate_payload, resolve_profile, PayloadBudgetReport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

const SARIF_SCHEMA: &str =
    "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json";

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
    pub suppressed_by_baseline: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub payload_results: Vec<PayloadCheckResult>,
}

#[derive(Debug, Deserialize)]
struct BaselineFile {
    #[serde(default)]
    diagnostics: Vec<Diagnostic>,
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

#[allow(dead_code)]
pub fn run_budget_file(config_path: &Path) -> Result<CheckSummary, String> {
    run_budget_file_with_options(config_path, None, None)
}

pub fn run_budget_file_with_options(
    config_path: &Path,
    custom_profiles: Option<&Path>,
    baseline_path: Option<&Path>,
) -> Result<CheckSummary, String> {
    let baseline_diags = if let Some(bpath) = baseline_path {
        require_pro("CI Regression Baseline (--baseline)")?;
        let braw = fs::read_to_string(bpath)
            .map_err(|e| format!("Failed to read baseline {}: {e}", bpath.display()))?;
        let parsed: BaselineFile = serde_json::from_str(braw.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("Invalid baseline JSON {}: {e}", bpath.display()))?;
        Some(parsed.diagnostics)
    } else {
        None
    };

    let raw = fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
    evaluate_budget_toml_with_options(raw.trim_start_matches('\u{feff}'), custom_profiles, baseline_diags.as_deref())
}

#[allow(dead_code)]
pub fn evaluate_budget_toml(raw_toml: &str) -> Result<CheckSummary, String> {
    evaluate_budget_toml_with_options(raw_toml, None, None)
}

pub fn evaluate_budget_toml_with_options(
    raw_toml: &str,
    custom_profiles: Option<&Path>,
    baseline_diags: Option<&[Diagnostic]>,
) -> Result<CheckSummary, String> {
    let cfg: BudgetConfig =
        toml::from_str(raw_toml).map_err(|e| format!("Invalid budgets.toml: {e}"))?;
    let profile = resolve_profile(&cfg.profile, custom_profiles)?;
    let threshold = FailThreshold::from_str(&cfg.fail_on)
        .ok_or_else(|| format!("Invalid fail_on '{}'", cfg.fail_on))?;

    let mut all_diags = Vec::new();
    for p in &cfg.paths {
        let rep = analyze_path(Path::new(p), &profile, cfg.assumed_hz)?;
        all_diags.extend(rep.diagnostics);
    }

    let (all_diags, suppressed_by_baseline) = match baseline_diags {
        Some(base) => filter_against_baseline(all_diags, base),
        None => (all_diags, 0),
    };

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
        suppressed_by_baseline,
        diagnostics: all_diags,
        payload_results,
    })
}

fn baseline_key(d: &Diagnostic) -> (String, String, String) {
    (
        normalize_sarif_uri(&d.file),
        d.rule_id.clone(),
        d.snippet.clone(),
    )
}

pub fn filter_against_baseline(
    current: Vec<Diagnostic>,
    baseline: &[Diagnostic],
) -> (Vec<Diagnostic>, usize) {
    let mut counts: HashMap<(String, String, String), usize> = HashMap::new();
    for b in baseline {
        *counts.entry(baseline_key(b)).or_insert(0) += 1;
    }

    let mut regressions = Vec::new();
    let mut suppressed = 0;
    for d in current {
        let key = baseline_key(&d);
        if let Some(c) = counts.get_mut(&key) {
            if *c > 0 {
                *c -= 1;
                suppressed += 1;
                continue;
            }
        }
        regressions.push(d);
    }
    (regressions, suppressed)
}

fn sarif_level(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" | "high" => "error",
        "medium" => "warning",
        _ => "note",
    }
}

fn normalize_sarif_uri(path: &str) -> String {
    let forward = path.replace('\\', "/");
    forward.strip_prefix("./").unwrap_or(&forward).to_string()
}

pub fn diagnostics_to_sarif(diags: &[Diagnostic]) -> Value {
    build_sarif(diags, &[], None)
}

pub fn check_summary_to_sarif(summary: &CheckSummary, config_path: &Path) -> Value {
    let failed_payloads: Vec<&PayloadCheckResult> =
        summary.payload_results.iter().filter(|p| !p.passed).collect();
    let cfg_uri = normalize_sarif_uri(&config_path.display().to_string());
    build_sarif(&summary.diagnostics, &failed_payloads, Some(&cfg_uri))
}

fn build_sarif(
    diags: &[Diagnostic],
    failed_payloads: &[&PayloadCheckResult],
    config_uri: Option<&str>,
) -> Value {
    let mut rules_map: BTreeMap<String, Value> = BTreeMap::new();
    let mut results: Vec<Value> = Vec::with_capacity(diags.len() + failed_payloads.len());

    for d in diags {
        let level = sarif_level(&d.severity);
        rules_map.entry(d.rule_id.clone()).or_insert_with(|| {
            json!({
                "id": d.rule_id,
                "name": d.rule_id,
                "shortDescription": { "text": d.physics_law },
                "fullDescription": { "text": d.recommended_fix },
                "help": {
                    "text": format!("Physics Law: {}\nRecommended Fix: {}", d.physics_law, d.recommended_fix)
                },
                "defaultConfiguration": { "level": level },
                "properties": {
                    "tags": ["linkguard", "physical-layer", d.severity]
                }
            })
        });

        results.push(json!({
            "ruleId": d.rule_id,
            "level": level,
            "message": {
                "text": format!(
                    "{} | Physics: {} | Fix: {}",
                    d.explanation, d.physics_law, d.recommended_fix
                )
            },
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": {
                        "uri": normalize_sarif_uri(&d.file),
                        "uriBaseId": "%SRCROOT%"
                    },
                    "region": {
                        "startLine": d.line.max(1),
                        "startColumn": d.column.max(1)
                    }
                }
            }]
        }));
    }

    if !failed_payloads.is_empty() {
        let rule_id = "PAYLOAD_BUDGET_EXCEEDED";
        rules_map.entry(rule_id.to_string()).or_insert_with(|| {
            json!({
                "id": rule_id,
                "name": rule_id,
                "shortDescription": { "text": "Configured payload_checks budget exceeded physical channel MTU or goodput" },
                "fullDescription": { "text": "Reduce payload bytes, lower transmit Hz, or batch frames to fit channel limits." },
                "defaultConfiguration": { "level": "error" },
                "properties": { "tags": ["linkguard", "physical-layer", "budget"] }
            })
        });

        let uri = config_uri.unwrap_or("budgets.toml");
        for p in failed_payloads {
            results.push(json!({
                "ruleId": rule_id,
                "level": "error",
                "message": {
                    "text": format!(
                        "Payload check '{}' ({} B @ {} Hz) failed: {}",
                        p.name, p.report.payload_bytes, p.report.assumed_hz, p.reason
                    )
                },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": {
                            "uri": uri,
                            "uriBaseId": "%SRCROOT%"
                        },
                        "region": {
                            "startLine": 1,
                            "startColumn": 1
                        }
                    }
                }]
            }));
        }
    }

    let rules: Vec<Value> = rules_map.into_values().collect();

    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "LinkGuard",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules
                }
            },
            "results": results
        }]
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

    #[test]
    fn sarif_output_maps_levels_normalizes_paths_and_dedups_rules() {
        let diags = vec![
            Diagnostic {
                rule_id: "JSON_IN_LOOP".into(),
                severity: "critical".into(),
                file: r".\src\bad_telemetry.py".into(),
                line: 12,
                column: 9,
                snippet: "json.dumps(x)".into(),
                explanation: "JSON bloat".into(),
                physics_law: "Shannon ceiling".into(),
                recommended_fix: "Use struct.pack".into(),
                budget: None,
            },
            Diagnostic {
                rule_id: "JSON_IN_LOOP".into(),
                severity: "medium".into(),
                file: r"firmware\node.cpp".into(),
                line: 24,
                column: 5,
                snippet: "snprintf(...)".into(),
                explanation: "Text formatting".into(),
                physics_law: "Shannon ceiling".into(),
                recommended_fix: "Use packed struct".into(),
                budget: None,
            },
        ];

        let sarif = diagnostics_to_sarif(&diags);
        assert_eq!(sarif["version"], "2.1.0");
        assert_eq!(sarif["runs"][0]["tool"]["driver"]["name"], "LinkGuard");
        assert_eq!(sarif["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap().len(), 1);

        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["level"], "error");
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/bad_telemetry.py"
        );
        assert_eq!(results[1]["level"], "warning");
        assert_eq!(
            results[1]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "firmware/node.cpp"
        );
    }

    #[test]
    fn baseline_suppresses_shifted_legacy_findings_and_catches_new_regressions() {
        let legacy = Diagnostic {
            rule_id: "JSON_IN_LOOP".into(),
            severity: "critical".into(),
            file: r".\src\app.py".into(),
            line: 10,
            column: 5,
            snippet: "json.dumps(x)".into(),
            explanation: "Legacy".into(),
            physics_law: "Shannon".into(),
            recommended_fix: "Pack".into(),
            budget: None,
        };
        let mut shifted = legacy.clone();
        shifted.file = "src/app.py".into();
        shifted.line = 18; // shifted down by 8 lines

        let mut new_regression = legacy.clone();
        new_regression.snippet = "json.dumps(y)".into();
        new_regression.line = 42;

        let (kept, suppressed) = filter_against_baseline(vec![shifted, new_regression], &[legacy]);
        assert_eq!(suppressed, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].snippet, "json.dumps(y)");
    }
}