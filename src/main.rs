#[cfg(feature = "gui")]
mod gui;
mod analyzer;
mod budgets;
mod license; // <-- 1. add module
mod mcp;
mod physics;
mod proxy;

use crate::analyzer::{analyze_path, report_exceeds_threshold, FailThreshold};
use crate::budgets::{check_summary_to_sarif, diagnostics_to_sarif, run_budget_file_with_options};
use crate::license::check_license; // <-- 2. import license check
use crate::physics::{all_profiles, evaluate_payload, resolve_profile}; // <-- 3. replace builtin_profiles/get_profile
use crate::proxy::ProxyTransport;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "linkguard", version, about = "Deterministic AST + physical-layer packet analyzer & proxy")]
struct Cli {
    /// Optional custom profiles.toml path (requires Pro/Team license)
    #[arg(long, global = true)]
    profiles: Option<PathBuf>, // <-- 4. global --profiles flag

    #[command(subcommand)]
    command: Commands,
}

#[derive(Copy, Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Sarif,
}

#[derive(Copy, Clone, ValueEnum)]
enum FailLevel {
    None,
    Critical,
    High,
    Medium,
}

impl From<FailLevel> for FailThreshold {
    fn from(v: FailLevel) -> Self {
        match v {
            FailLevel::None => FailThreshold::None,
            FailLevel::Critical => FailThreshold::Critical,
            FailLevel::High => FailThreshold::High,
            FailLevel::Medium => FailThreshold::Medium,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// List built-in (and licensed custom) RF / serial link profiles
    Profiles,

    /// Inspect offline LinkGuard license status (Free / Pro / Team)
    License, // <-- 5. new subcommand

    /// Analyze Python, Rust, C/C++, TypeScript, or JavaScript files against physical-layer constraints
    Analyze {
        path: PathBuf,
        #[arg(short, long, default_value = "lora-sf12")]
        profile: String,
        #[arg(long, default_value_t = 10.0)]
        hz: f64,
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        #[arg(long, value_enum, default_value_t = FailLevel::None)]
        fail_on: FailLevel,
    },

    /// Enforce a budgets.toml policy file (CI/CD gate)
    Check {
        #[arg(short, long, default_value = "budgets.toml")]
        config: PathBuf,
        /// Optional JSON baseline from an earlier run (Pro/Team: fails only on new regressions)
        #[arg(long)]
        baseline: Option<PathBuf>,
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },

    /// Compute Shannon ceiling, MTU fragmentation, airtime, and BER loss for a payload
    Budget {
        #[arg(short, long, default_value = "lora-sf12")]
        profile: String,
        #[arg(short, long)]
        bytes: usize,
        #[arg(long, default_value_t = 1.0)]
        hz: f64,
    },

    /// Run a seeded deterministic physical-layer TCP or UDP proxy
    Proxy {
        #[arg(long, default_value = "127.0.0.1:9000")]
        listen: String,
        #[arg(long)]
        target: String,
        #[arg(short, long, default_value = "wifi-24g-congested")]
        profile: String,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = false)]
        udp: bool,
    },

    /// Start stdio MCP server for IDE or local LLM agents
    Mcp,

    /// Launch the Pro Desktop Dashboard (requires `--features gui` & Pro/Team license)
    Gui {
        #[arg(short, long, default_value = "lora-sf12")]
        profile: String,
        #[arg(long, default_value = "127.0.0.1:9000")]
        listen: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = false)]
        udp: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let custom_profiles = cli.profiles.as_deref(); // <-- 6. pass to profile lookups below

    let res = match cli.command {
        Commands::Profiles => all_profiles(custom_profiles).map(|profiles| {
            for p in profiles {
                println!(
                    "{:<20} | goodput={:>9.0} bps | MTU={:>4} B | header={:>2} B | base_lat={:>5.1} ms | SNR={:>5.1} dB",
                    p.id, p.effective_goodput_bps, p.max_payload_bytes, p.header_overhead_bytes, p.base_latency_ms, p.snr_db
                );
                println!("   {}", p.notes);
            }
        }),
        Commands::License => {
            let status = check_license();
            println!("{}", serde_json::to_string_pretty(&status).unwrap_or_default());
            Ok(())
        }
        Commands::Analyze { path, profile, hz, format, fail_on } => {
            resolve_profile(&profile, custom_profiles).and_then(|prof| {
                analyze_path(&path, &prof, hz).and_then(|report| {
                    match format {
                        OutputFormat::Json => {
                            println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                        }
                        OutputFormat::Sarif => {
                            let sarif = diagnostics_to_sarif(&report.diagnostics);
                            println!("{}", serde_json::to_string_pretty(&sarif).unwrap_or_default());
                        }
                        OutputFormat::Text => {
                            println!(
                                "Scanned {} file(s) against '{}' ({} findings):\n",
                                report.files_scanned, report.target_profile, report.diagnostics.len()
                            );
                            for d in &report.diagnostics {
                                println!("[{}] {} ({}:{}:{})", d.severity.to_uppercase(), d.rule_id, d.file, d.line, d.column);
                                println!("  Code:   {}", d.snippet);
                                println!("  Issue:  {}", d.explanation);
                                println!("  Law:    {}", d.physics_law);
                                println!("  Fix:    {}\n", d.recommended_fix);
                            }
                        }
                    }
                    if report_exceeds_threshold(&report.diagnostics, fail_on.into()) {
                        Err("Diagnostics exceeded --fail-on threshold.".into())
                    } else {
                        Ok(())
                    }
                })
            })
        }
        Commands::Check { config, baseline, format } => {
            run_budget_file_with_options(&config, custom_profiles, baseline.as_deref()).and_then(|summary| {
                match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
                    }
                    OutputFormat::Sarif => {
                        let sarif = check_summary_to_sarif(&summary, &config);
                        println!("{}", serde_json::to_string_pretty(&sarif).unwrap_or_default());
                    }
                    OutputFormat::Text => {
                        println!(
                            "Budget Check [{}] | profile={} | diagnostics={} (suppressed={}) | fail_on={}",
                            if summary.passed { "PASS" } else { "FAIL" },
                            summary.profile,
                            summary.total_diagnostics,
                            summary.suppressed_by_baseline,
                            summary.fail_on
                        );
                        for p in &summary.payload_results {
                            println!("  [{}] {}: {}", if p.passed { "PASS" } else { "FAIL" }, p.name, p.reason);
                        }
                        for d in &summary.diagnostics {
                            println!("  [{}] {} ({}:{}) - {}", d.severity.to_uppercase(), d.rule_id, d.file, d.line, d.explanation);
                        }
                    }
                }
                if summary.passed {
                    Ok(())
                } else {
                    Err("Budget check failed.".into())
                }
            })
        }
        Commands::Budget { profile, bytes, hz } => {
            resolve_profile(&profile, custom_profiles).map(|prof| {
                let rep = evaluate_payload(&prof, bytes, hz);
                println!("{}", serde_json::to_string_pretty(&rep).unwrap_or_default());
            })
        }
        Commands::Proxy { listen, target, profile, seed, udp } => {
            match resolve_profile(&profile, custom_profiles) {
                Err(e) => Err(e),
                Ok(prof) => {
                    let transport = if udp { ProxyTransport::Udp } else { ProxyTransport::Tcp };
                    proxy::run_proxy(&listen, &target, prof, seed, transport).await
                }
            }
        }
        Commands::Mcp => mcp::run_mcp_stdio(),
        Commands::Gui { profile, listen, target, seed, udp } => {
            #[cfg(feature = "gui")]
            {
                resolve_profile(&profile, custom_profiles)
                    .and_then(|prof| gui::run_dashboard(prof, listen, target, seed, udp))
            }
            #[cfg(not(feature = "gui"))]
            {
                let _ = (profile, listen, target, seed, udp);
                Err("GUI support is not compiled in this headless build. Run with: cargo run --features gui -- gui".into())
            }
        }
    };

    if let Err(e) = res {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}