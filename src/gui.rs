use crate::license::{current_unix_secs, require_pro};
use crate::physics::{shannon_capacity_bps, LinkProfile};
use crate::proxy::{
    run_proxy_live, spawn_metrics_sampler, LinkShaper, PacketDecision, ProxyMetrics, ProxyTransport,
};
use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use std::collections::VecDeque;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

const MAX_PLOT_SAMPLES: usize = 300; // 60s @ 200ms

pub fn run_dashboard(
    initial_profile: LinkProfile,
    listen: String,
    target: Option<String>,
    seed: u64,
    udp: bool,
) -> Result<(), String> {
    let claims = require_pro("Pro Desktop Dashboard")?;

    let transport = if udp {
        ProxyTransport::Udp
    } else {
        ProxyTransport::Tcp
    };
    let (metrics_tx, metrics_rx) = watch::channel(ProxyMetrics::from_profile(&initial_profile));
    let (profile_tx, profile_rx) = watch::channel(initial_profile.clone());
    let shaper = Arc::new(LinkShaper::new(transport, initial_profile.clone(), seed));

    let synth_default_on = target.is_none();
    let (synth_tx, mut synth_rx) = watch::channel((synth_default_on, 64_usize, 5.0_f64));

    if let Some(target_addr) = target.clone() {
        let sh = Arc::clone(&shaper);
        let mtx = metrics_tx.clone();
        let prx = profile_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = run_proxy_live(&listen, &target_addr, sh, transport, mtx, prx).await {
                eprintln!("[gui proxy error] {e}");
            }
        });
    } else {
        spawn_metrics_sampler(Arc::clone(&shaper), metrics_tx, profile_rx);
    }

    // Synthetic packet load exercises the exact same LinkShaper token bucket & queue
    let synth_shaper = Arc::clone(&shaper);
    tokio::spawn(async move {
        loop {
            let (enabled, payload_bytes, hz) = *synth_rx.borrow();
            if !enabled {
                if synth_rx.changed().await.is_err() {
                    break;
                }
                continue;
            }
            let interval = Duration::from_secs_f64((1.0 / hz.max(0.1)).clamp(0.005, 5.0));
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    let sh = Arc::clone(&synth_shaper);
                    tokio::spawn(async move {
                        if let PacketDecision::Forward { delay, wire_bytes } = sh.admit_and_shape(payload_bytes).await {
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            sh.release_queue(wire_bytes).await;
                        }
                    });
                }
                res = synth_rx.changed() => {
                    if res.is_err() { break; }
                }
            }
        }
    });

    let app = DashboardApp {
        holder: claims.holder,
        profile: initial_profile,
        profile_tx,
        metrics_rx,
        synth_tx,
        synth_enabled: synth_default_on,
        synth_bytes: 64,
        synth_hz: 5.0,
        history: VecDeque::with_capacity(MAX_PLOT_SAMPLES),
        full_capture: Vec::new(),
        status_msg: if let Some(t) = target {
            format!("Live proxy active -> {t}")
        } else {
            "Simulator active (toggle Synthetic Load or pass --target to proxy live traffic)".into()
        },
        last_timestamp: -1.0,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1140.0, 740.0]),
        ..Default::default()
    };

    eframe::run_native(
        "LinkGuard Pro Dashboard",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| format!("GUI error: {e}"))
}

struct DashboardApp {
    holder: String,
    profile: LinkProfile,
    profile_tx: watch::Sender<LinkProfile>,
    metrics_rx: watch::Receiver<ProxyMetrics>,
    synth_tx: watch::Sender<(bool, usize, f64)>,
    synth_enabled: bool,
    synth_bytes: usize,
    synth_hz: f64,
    history: VecDeque<ProxyMetrics>,
    full_capture: Vec<ProxyMetrics>,
    status_msg: String,
    last_timestamp: f64,
}

impl eframe::App for DashboardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(100));

        let latest = self.metrics_rx.borrow().clone();
        if latest.timestamp_s > self.last_timestamp {
            self.last_timestamp = latest.timestamp_s;
            if self.history.len() >= MAX_PLOT_SAMPLES {
                self.history.pop_front();
            }
            self.history.push_back(latest.clone());
            self.full_capture.push(latest.clone());
        }

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("LinkGuard Pro Dashboard");
                ui.separator();
                ui.label(format!("Profile: {}", self.profile.id));
                ui.separator();
                ui.label(format!("Licensed to: {}", self.holder));
            });
        });

        egui::SidePanel::left("controls")
            .min_width(310.0)
            .show(ctx, |ui| {
                ui.heading("Channel Parameters");
                let mut changed = false;

                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.profile.bandwidth_hz, 1_000.0..=50_000_000.0)
                            .logarithmic(true)
                            .text("Bandwidth (Hz)"),
                    )
                    .changed();

                changed |= ui
                    .add(egui::Slider::new(&mut self.profile.snr_db, -25.0..=40.0).text("SNR (dB)"))
                    .changed();

                let shannon = shannon_capacity_bps(self.profile.bandwidth_hz, self.profile.snr_db);
                ui.label(format!("Shannon Ceiling: {:.0} bps", shannon));

                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.profile.effective_goodput_bps,
                            100.0..=20_000_000.0,
                        )
                        .logarithmic(true)
                        .text("Effective Goodput (bps)"),
                    )
                    .changed();

                if ui.button("Sync Goodput to 50% Shannon").clicked() {
                    self.profile.effective_goodput_bps = (shannon * 0.5).max(100.0);
                    changed = true;
                }

                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.profile.max_payload_bytes, 8..=1500)
                            .text("MTU (bytes)"),
                    )
                    .changed();

                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.profile.header_overhead_bytes, 0..=128)
                            .text("Header (bytes)"),
                    )
                    .changed();

                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.profile.queue_capacity_bytes, 64..=65_536)
                            .logarithmic(true)
                            .text("Queue Cap (bytes)"),
                    )
                    .changed();

                if changed {
                    let _ = self.profile_tx.send(self.profile.clone());
                }

                ui.separator();
                ui.heading("Synthetic Load Generator");
                let mut synth_changed = ui
                    .checkbox(&mut self.synth_enabled, "Inject Periodic Packets")
                    .changed();
                synth_changed |= ui
                    .add(egui::Slider::new(&mut self.synth_bytes, 8..=512).text("Payload (B)"))
                    .changed();
                synth_changed |= ui
                    .add(
                        egui::Slider::new(&mut self.synth_hz, 0.2..=200.0)
                            .logarithmic(true)
                            .text("Rate (Hz)"),
                    )
                    .changed();
                if synth_changed {
                    let _ = self
                        .synth_tx
                        .send((self.synth_enabled, self.synth_bytes, self.synth_hz));
                }

                ui.separator();
                ui.heading("Replay Capture Export");
                ui.label(format!("Captured samples: {}", self.full_capture.len()));
                ui.horizontal(|ui| {
                    if ui.button("Export CSV").clicked() {
                        match export_csv(&self.full_capture) {
                            Ok(path) => self.status_msg = format!("Exported CSV: {path}"),
                            Err(e) => self.status_msg = format!("Export error: {e}"),
                        }
                    }
                    if ui.button("Export JSON").clicked() {
                        match export_json(&self.profile, &self.full_capture) {
                            Ok(path) => self.status_msg = format!("Exported JSON: {path}"),
                            Err(e) => self.status_msg = format!("Export error: {e}"),
                        }
                    }
                    if ui.button("Clear").clicked() {
                        self.full_capture.clear();
                        self.history.clear();
                    }
                });
                ui.separator();
                ui.label(&self.status_msg);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Offered: {:.0} bps", latest.offered_load_bps));
                ui.separator();
                ui.label(format!("Delivered: {:.0} bps", latest.delivered_goodput_bps));
                ui.separator();
                ui.label(format!(
                    "Forwarded: {}/{} pkts",
                    latest.packets_forwarded, latest.packets_in
                ));
                ui.separator();
                let total_drops = latest.queue_drops + latest.ber_drops;
                ui.colored_label(
                    if total_drops > 0 {
                        egui::Color32::LIGHT_RED
                    } else {
                        egui::Color32::LIGHT_GREEN
                    },
                    format!(
                        "Drops: {} (queue={}, ber={})",
                        total_drops, latest.queue_drops, latest.ber_drops
                    ),
                );
            });

            let q_frac = if latest.queue_capacity_bytes > 0 {
                (latest.queued_bytes as f32 / latest.queue_capacity_bytes as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ui.add(
                egui::ProgressBar::new(q_frac).text(format!(
                    "Queue Depth: {} / {} B ({:.0}%)",
                    latest.queued_bytes,
                    latest.queue_capacity_bytes,
                    q_frac * 100.0
                )),
            );

            ui.add_space(8.0);

            let offered_pts: PlotPoints = self
                .history
                .iter()
                .map(|m| [m.timestamp_s, m.offered_load_bps])
                .collect();
            let delivered_pts: PlotPoints = self
                .history
                .iter()
                .map(|m| [m.timestamp_s, m.delivered_goodput_bps])
                .collect();
            let effective_pts: PlotPoints = self
                .history
                .iter()
                .map(|m| [m.timestamp_s, m.effective_goodput_bps])
                .collect();
            let shannon_pts: PlotPoints = self
                .history
                .iter()
                .map(|m| [m.timestamp_s, m.shannon_ceiling_bps])
                .collect();

            Plot::new("throughput_plot")
                .legend(Legend::default())
                .x_axis_label("Time (s)")
                .y_axis_label("bps")
                .show(ui, |plot_ui| {
                    plot_ui.line(Line::new(offered_pts).name("Offered Load (bps)"));
                    plot_ui.line(Line::new(delivered_pts).name("Delivered Goodput (bps)"));
                    plot_ui.line(Line::new(effective_pts).name("Effective Goodput Ceiling"));
                    plot_ui.line(Line::new(shannon_pts).name("Shannon Ceiling (bps)"));
                });
        });
    }
}

fn export_csv(samples: &[ProxyMetrics]) -> Result<String, String> {
    require_pro("Replay Capture Export")?;
    let path = format!("linkguard_capture_{}.csv", current_unix_secs());
    let mut out = String::from(
        "timestamp_s,profile_id,offered_load_bps,delivered_goodput_bps,effective_goodput_bps,shannon_ceiling_bps,queued_bytes,queue_capacity_bytes,packets_in,packets_forwarded,queue_drops,ber_drops,bytes_in,bytes_forwarded,fragments\n",
    );
    for s in samples {
        out.push_str(&format!(
            "{:.3},{},{:.2},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{},{}\n",
            s.timestamp_s,
            s.profile_id,
            s.offered_load_bps,
            s.delivered_goodput_bps,
            s.effective_goodput_bps,
            s.shannon_ceiling_bps,
            s.queued_bytes,
            s.queue_capacity_bytes,
            s.packets_in,
            s.packets_forwarded,
            s.queue_drops,
            s.ber_drops,
            s.bytes_in,
            s.bytes_forwarded,
            s.fragments
        ));
    }
    fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(path)
}

fn export_json(profile: &LinkProfile, samples: &[ProxyMetrics]) -> Result<String, String> {
    require_pro("Replay Capture Export")?;
    let path = format!("linkguard_capture_{}.json", current_unix_secs());
    let payload = serde_json::json!({
        "exported_unix": current_unix_secs(),
        "profile": profile,
        "samples": samples,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}