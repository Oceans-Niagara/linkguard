import init, {
  list_profiles_json,
  evaluate_payload_json,
  evaluate_custom_json,
} from "./pkg/linkguard_wasm.js";

await init();
const profiles = JSON.parse(list_profiles_json());
const profileMap = Object.fromEntries(profiles.map((p) => [p.id, p]));

const PLAY_PRESETS = {
  lora_json: { text: '{"dev_id":"node-04","temp_c":23.84,"humidity":48.2,"vbat_mv":3712,"seq":10492,"status":"nominal"}', profile: "lora-sf12", hz: 10 },
  lora_bin:  { text: "N4:23.8,48,3712,10492", profile: "lora-sf12", hz: 0.2 },
  ble_frag:  { text: JSON.stringify({ device: "wearable-01", samples: Array.from({ length: 14 }, (_, i) => ({ i, v: +(Math.sin(i) * 10).toFixed(2) })) }), profile: "ble-1m", hz: 20 },
  leo_burst: { text: '{"frame":"telemetry_batch","seq":8821,"ok":true,"metrics":[12.4,19.8,44.1,98.2,101.0,15.3]}', profile: "leo-sat", hz: 40 },
};

const FULL_PRESETS = {
  imu_json: '{"t":1710420,"ax":0.012,"ay":-0.034,"az":9.806,"gx":0.001,"gy":0.002,"gz":-0.001,"q":[0.99,0.01,0.0,0.02]}',
  gps_json: '{"dev_id":"rover-2","lat":43.096214,"lon":-79.037739,"alt_m":182.4,"fix":3,"sats":11,"hdop":0.84,"vbat":11.82}',
  packed_struct: "R2:43.096,-79.037,182,11.8",
  verbose_log: '{"level":"INFO","service":"telemetry-daemon","timestamp":"2026-03-04T12:00:01Z","msg":"heartbeat ok","uptime_sec":48291,"free_heap":41200}',
};

const state = {
  view: "play",
  source: "text",
  linkMode: "builtin",
  cmpMode: "baseline",
  snapshots: [],
  compareIdx: "",
  lastCtx: null,
  lastSweep: [],
  lastSnrCurve: [],
};

const $ = (id) => document.getElementById(id);

for (const sel of [$("play-profile-select"), $("profile-select")]) {
  for (const p of profiles) {
    const opt = document.createElement("option");
    opt.value = p.id;
    opt.textContent = `${p.name} (MTU ${p.max_payload_bytes}B, ${Math.round(p.effective_goodput_bps)} bps)`;
    sel.appendChild(opt);
  }
}

function loadProfileIntoCustomInputs(pid) {
  const p = profileMap[pid] || profiles[0];
  $("c-snr-slider").value = p.snr_db;
  $("c-snr-val").textContent = `${p.snr_db} dB`;
  $("c-bw").value = p.bandwidth_hz;
  $("c-goodput").value = p.effective_goodput_bps;
  $("c-mtu").value = p.max_payload_bytes;
  $("c-hdr").value = p.header_overhead_bytes;
  $("c-lat").value = p.base_latency_ms;
}
loadProfileIntoCustomInputs($("profile-select").value);

function utf8Bytes(str) {
  return new TextEncoder().encode(str || "").length;
}

function estimatePackedBinary(str) {
  const trimmed = (str || "").trim();
  if (!trimmed) return 1;
  try {
    const obj = JSON.parse(trimmed);
    const walk = (v) => {
      if (v === null || v === undefined) return 1;
      if (typeof v === "number") return 4;
      if (typeof v === "boolean") return 1;
      if (typeof v === "string") return Math.max(1, Math.min(v.length, 8));
      if (Array.isArray(v)) return 2 + v.reduce((a, x) => a + walk(x), 0);
      if (typeof v === "object") return 2 + Object.values(v).reduce((a, x) => a + walk(x), 0);
      return 4;
    };
    return Math.max(2, walk(obj));
  } catch {
    return Math.max(1, Math.round(utf8Bytes(trimmed) * 0.45));
  }
}

function getActiveLink() {
  if (state.view === "play" || state.linkMode === "builtin") {
    const pid = state.view === "play" ? $("play-profile-select").value : $("profile-select").value;
    const p = profileMap[pid] || profiles[0];
    return { mode: "builtin", profile: p, label: p.id };
  }
  const p = {
    id: "custom",
    name: "Custom RF / Serial Link",
    bandwidth_hz: Math.max(100, parseFloat($("c-bw").value || "125000")),
    snr_db: parseFloat($("c-snr-slider").value || "-10"),
    effective_goodput_bps: Math.max(1, parseFloat($("c-goodput").value || "250")),
    max_payload_bytes: Math.max(1, parseInt($("c-mtu").value || "51", 10)),
    header_overhead_bytes: Math.max(0, parseInt($("c-hdr").value || "13", 10)),
    base_latency_ms: Math.max(0, parseFloat($("c-lat").value || "0")),
    notes: "Custom WASM link model",
  };
  return { mode: "custom", profile: p, label: `custom (${p.max_payload_bytes}B MTU, ${p.snr_db}dB)` };
}

function evalLink(link, bytes, hz) {
  if (link.mode === "builtin") {
    return JSON.parse(evaluate_payload_json(link.profile.id, bytes, hz));
  }
  const p = link.profile;
  return JSON.parse(
    evaluate_custom_json(
      p.bandwidth_hz,
      p.snr_db,
      p.effective_goodput_bps,
      p.max_payload_bytes,
      p.header_overhead_bytes,
      p.base_latency_ms,
      bytes,
      hz,
    ),
  );
}

function getInputs() {
  const link = getActiveLink();
  if (state.view === "play") {
    const text = $("play-payload-input").value;
    const rawBytes = Math.max(1, utf8Bytes(text));
    const hz = Math.max(0.1, parseFloat($("play-hz-slider").value || "10"));
    return { link, bytes: rawBytes, rawBytes, packedBytes: estimatePackedBinary(text), hz, encoding: "Raw UTF-8" };
  }
  const hz = Math.max(0.01, parseFloat($("hz-number").value || "1"));
  if (state.source === "bytes") {
    const b = Math.max(1, parseInt($("bytes-number").value || "1", 10));
    return { link, bytes: b, rawBytes: b, packedBytes: b, hz, encoding: "Manual Bytes" };
  }
  const text = $("text-input").value;
  const rawBytes = Math.max(1, utf8Bytes(text));
  const packedBytes = estimatePackedBinary(text);
  $("payload-size-info").textContent = `Raw: ${rawBytes} B | Packed binary est: ~${packedBytes} B`;
  const simBin = $("simulate-binary").checked;
  return {
    link,
    bytes: simBin ? packedBytes : rawBytes,
    rawBytes,
    packedBytes,
    hz,
    encoding: simBin ? "Packed Binary (est)" : "Raw JSON/Text",
  };
}

function fmtRate(bps) {
  if (bps >= 1e6) return `${(bps / 1e6).toFixed(2)} Mbps`;
  if (bps >= 1e3) return `${(bps / 1e3).toFixed(2)} kbps`;
  return `${bps.toFixed(1)} bps`;
}

function verdictKind(v) {
  if (v.startsWith("PASS")) return "PASS";
  if (v.startsWith("WARN")) return "WARN";
  return "FAIL";
}

function verdictColor(kind) {
  if (kind === "PASS") return "#22c55e";
  if (kind === "WARN") return "#f59e0b";
  return "#ef4444";
}

function prepCanvas(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(100, canvas.clientWidth) * dpr;
  const h = Math.max(60, canvas.clientHeight) * dpr;
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = "#0b0d13";
  ctx.fillRect(0, 0, w, h);
  return { ctx, w, h, dpr };
}

function drawFrameDiagram(canvas, profile, report, bytes) {
  const { ctx, w, h, dpr } = prepCanvas(canvas);
  const pad = 14 * dpr;
  const frags = Math.max(1, report.fragments_per_message);
  const shown = Math.min(frags, 10);
  const hdr = profile.header_overhead_bytes;
  const mtu = Math.max(1, profile.max_payload_bytes);
  const barY = h * 0.34;
  const barH = Math.max(22 * dpr, h * 0.28);
  const availW = w - pad * 2;
  const gap = Math.max(4 * dpr, availW * 0.012);
  const segW = (availW - gap * (shown - 1)) / shown;

  ctx.font = `${Math.round(11 * dpr)}px ui-monospace, monospace`;
  ctx.fillStyle = "#98a2b3";
  ctx.fillText(
    `${bytes}B payload -> ${frags} frag${frags > 1 ? "s" : ""} (${report.wire_bytes_per_message}B wire | MTU ${mtu}B + ${hdr}B hdr)`,
    pad,
    pad + 10 * dpr,
  );

  let rem = bytes;
  for (let i = 0; i < shown; i++) {
    const x = pad + i * (segW + gap);
    const chunk = i < frags - 1 ? Math.min(mtu, rem) : Math.max(1, rem);
    rem = Math.max(0, rem - chunk);
    const hdrFrac = hdr / Math.max(1, hdr + chunk);
    const hdrW = hdr > 0 ? Math.max(4 * dpr, segW * hdrFrac) : 0;

    if (hdrW > 0) {
      ctx.fillStyle = "#f59e0b";
      ctx.fillRect(x, barY, hdrW, barH);
    }
    ctx.fillStyle = frags > 1 ? "#ef4444" : "#3b82f6";
    ctx.fillRect(x + hdrW, barY, Math.max(2 * dpr, segW - hdrW), barH);
    ctx.strokeStyle = "#2c3242";
    ctx.strokeRect(x, barY, segW, barH);

    if (segW > 42 * dpr) {
      ctx.fillStyle = "#eef1f7";
      ctx.fillText(`#${i + 1}`, x + 5 * dpr, barY + barH * 0.58);
    }
  }

  if (frags > shown) {
    ctx.fillStyle = "#ef4444";
    ctx.fillText(`+${frags - shown} more fragments...`, pad, barY + barH + 16 * dpr);
  }

  const legY = h - pad;
  ctx.fillStyle = "#f59e0b";
  ctx.fillRect(pad, legY - 9 * dpr, 9 * dpr, 9 * dpr);
  ctx.fillStyle = "#98a2b3";
  ctx.fillText(`Header (${hdr}B/frag)`, pad + 14 * dpr, legY - 1 * dpr);
  const x2 = pad + 145 * dpr;
  ctx.fillStyle = frags > 1 ? "#ef4444" : "#3b82f6";
  ctx.fillRect(x2, legY - 9 * dpr, 9 * dpr, 9 * dpr);
  ctx.fillStyle = "#98a2b3";
  ctx.fillText(frags > 1 ? "Fragmented Payload" : "Single-MTU Payload", x2 + 14 * dpr, legY - 1 * dpr);
}

function drawEncodingComparison(canvas, link, rawBytes, packedBytes, hz) {
  const { ctx, w, h, dpr } = prepCanvas(canvas);
  const pad = 16 * dpr;
  const rawR = evalLink(link, rawBytes, hz);
  const binR = evalLink(link, packedBytes, hz);
  const maxWire = Math.max(rawR.wire_bytes_per_message, binR.wire_bytes_per_message, 1);
  const barMaxW = w - pad * 2 - 110 * dpr;

  const drawRow = (label, r, payloadB, y, color) => {
    ctx.font = `${Math.round(11 * dpr)}px ui-monospace, monospace`;
    ctx.fillStyle = "#eef1f7";
    ctx.fillText(label, pad, y);
    const barW = Math.max(8 * dpr, (r.wire_bytes_per_message / maxWire) * barMaxW);
    ctx.fillStyle = color;
    ctx.fillRect(pad, y + 6 * dpr, barW, 20 * dpr);
    ctx.fillStyle = "#98a2b3";
    ctx.fillText(
      `${payloadB}B -> ${r.wire_bytes_per_message}B wire (${r.fragments_per_message} frag, ${r.utilization_pct}% util)`,
      pad,
      y + 39 * dpr,
    );
  };

  drawRow("Raw JSON / Text", rawR, rawBytes, pad + 14 * dpr, "#ef4444");
  drawRow("Packed Binary (est)", binR, packedBytes, h * 0.56, "#22c55e");
  const saved = rawR.wire_bytes_per_message > 0
    ? (((rawR.wire_bytes_per_message - binR.wire_bytes_per_message) / rawR.wire_bytes_per_message) * 100).toFixed(0)
    : "0";
  return { rawR, binR, saved };
}

function drawSweepChart(canvas, link, bytes, curHz, maxSweepHz, overlaySnap = null) {
  const { ctx, w, h, dpr } = prepCanvas(canvas);
  const maxHz = Math.max(1, maxSweepHz, curHz * 1.25);
  const steps = 40;
  const pts = [];
  for (let i = 0; i <= steps; i++) {
    const hz = Math.max(0.05, (i / steps) * maxHz);
    const r = evalLink(link, bytes, hz);
    pts.push({ hz, wire: r.required_wire_bps, goodput: r.effective_goodput_bps, shannon: r.shannon_ceiling_bps });
  }
  state.lastSweep = pts;

  const goodput = pts[0].goodput;
  const shannon = pts[0].shannon;
  let maxY = Math.max(goodput * 1.4, pts[pts.length - 1].wire * 1.08, 100);
  if (overlaySnap) {
    maxY = Math.max(maxY, overlaySnap.effective_goodput_bps * 1.3);
  }

  const padL = 58 * dpr, padR = 16 * dpr, padT = 20 * dpr, padB = 28 * dpr;
  const cw = w - padL - padR;
  const ch = h - padT - padB;
  const xOf = (hz) => padL + (hz / maxHz) * cw;
  const yOf = (bps) => padT + ch - Math.min(1, bps / maxY) * ch;

  ctx.strokeStyle = "#202532";
  ctx.lineWidth = 1 * dpr;
  for (let g = 0; g <= 4; g++) {
    const y = padT + (g / 4) * ch;
    ctx.beginPath(); ctx.moveTo(padL, y); ctx.lineTo(w - padR, y); ctx.stroke();
  }

  ctx.setLineDash([5 * dpr, 4 * dpr]);
  ctx.strokeStyle = "#22c55e";
  ctx.beginPath(); ctx.moveTo(padL, yOf(goodput)); ctx.lineTo(w - padR, yOf(goodput)); ctx.stroke();

  if (shannon <= maxY) {
    ctx.strokeStyle = "#f59e0b";
    ctx.beginPath(); ctx.moveTo(padL, yOf(shannon)); ctx.lineTo(w - padR, yOf(shannon)); ctx.stroke();
  }
  ctx.setLineDash([]);

  if (overlaySnap) {
    ctx.strokeStyle = "#94a3b8";
    ctx.lineWidth = 1.6 * dpr;
    ctx.setLineDash([3 * dpr, 3 * dpr]);
    ctx.beginPath();
    for (let i = 0; i <= steps; i++) {
      const hz = Math.max(0.05, (i / steps) * maxHz);
      const wire = overlaySnap.wire_bytes_per_message * 8 * hz;
      const x = xOf(hz), y = yOf(wire);
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.stroke();
    ctx.setLineDash([]);
  }

  ctx.strokeStyle = "#3b82f6";
  ctx.lineWidth = 2.2 * dpr;
  ctx.beginPath();
  pts.forEach((p, idx) => {
    const x = xOf(p.hz), y = yOf(p.wire);
    if (idx === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();

  const curR = evalLink(link, bytes, curHz);
  ctx.fillStyle = curR.exceeds_effective_capacity ? "#ef4444" : "#22c55e";
  ctx.beginPath();
  ctx.arc(xOf(curHz), yOf(curR.required_wire_bps), 5 * dpr, 0, Math.PI * 2);
  ctx.fill();

  ctx.font = `${Math.round(10.5 * dpr)}px ui-monospace, monospace`;
  ctx.fillStyle = "#22c55e";
  ctx.fillText(`Goodput ${fmtRate(goodput)}`, padL + 4 * dpr, Math.max(padT + 11 * dpr, yOf(goodput) - 4 * dpr));
  if (shannon <= maxY) {
    ctx.fillStyle = "#f59e0b";
    ctx.fillText(`Shannon ${fmtRate(shannon)}`, padL + 4 * dpr, Math.max(padT + 11 * dpr, yOf(shannon) - 4 * dpr));
  }
  ctx.fillStyle = "#98a2b3";
  ctx.fillText("0 Hz", padL, h - 8 * dpr);
  ctx.fillText(`${maxHz.toFixed(1)} Hz`, w - padR - 50 * dpr, h - 8 * dpr);
  ctx.fillText(fmtRate(maxY), 4 * dpr, padT + 10 * dpr);
}

function drawSnrWaterfall(canvas, profile, bytes, hz) {
  const { ctx, w, h, dpr } = prepCanvas(canvas);
  const minSnr = -25, maxSnr = 35;
  const pts = [];
  for (let snr = minSnr; snr <= maxSnr; snr += 1.5) {
    const r = JSON.parse(
      evaluate_custom_json(
        profile.bandwidth_hz,
        snr,
        profile.effective_goodput_bps,
        profile.max_payload_bytes,
        profile.header_overhead_bytes,
        profile.base_latency_ms,
        bytes,
        hz,
      ),
    );
    pts.push({ snr, loss: r.packet_loss_prob_pct, shannon: r.shannon_ceiling_bps });
  }
  state.lastSnrCurve = pts;

  const padL = 46 * dpr, padR = 16 * dpr, padT = 20 * dpr, padB = 28 * dpr;
  const cw = w - padL - padR;
  const ch = h - padT - padB;
  const xOf = (s) => padL + ((s - minSnr) / (maxSnr - minSnr)) * cw;
  const yOf = (pct) => padT + ch - (Math.min(100, Math.max(0, pct)) / 100) * ch;

  ctx.strokeStyle = "#202532";
  ctx.lineWidth = 1 * dpr;
  for (let g = 0; g <= 4; g++) {
    const y = padT + (g / 4) * ch;
    ctx.beginPath(); ctx.moveTo(padL, y); ctx.lineTo(w - padR, y); ctx.stroke();
  }

  ctx.strokeStyle = "#ef4444";
  ctx.lineWidth = 2.2 * dpr;
  ctx.beginPath();
  pts.forEach((p, idx) => {
    const x = xOf(p.snr), y = yOf(p.loss);
    if (idx === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();

  const curR = evalLink({ mode: state.linkMode, profile }, bytes, hz);
  const cx = xOf(Math.min(maxSnr, Math.max(minSnr, profile.snr_db)));
  ctx.strokeStyle = "#3b82f6";
  ctx.setLineDash([4 * dpr, 3 * dpr]);
  ctx.beginPath(); ctx.moveTo(cx, padT); ctx.lineTo(cx, padT + ch); ctx.stroke();
  ctx.setLineDash([]);
  ctx.fillStyle = "#3b82f6";
  ctx.beginPath(); ctx.arc(cx, yOf(curR.packet_loss_prob_pct), 5 * dpr, 0, Math.PI * 2); ctx.fill();

  ctx.font = `${Math.round(10.5 * dpr)}px ui-monospace, monospace`;
  ctx.fillStyle = "#98a2b3";
  ctx.fillText("100%", 6 * dpr, padT + 10 * dpr);
  ctx.fillText("0%", 14 * dpr, padT + ch);
  ctx.fillText("-25 dB", padL, h - 8 * dpr);
  ctx.fillText("+35 dB", w - padR - 42 * dpr, h - 8 * dpr);
  ctx.fillStyle = "#3b82f6";
  ctx.fillText(`SNR ${profile.snr_db} dB (${curR.packet_loss_prob_pct}% drop)`, Math.min(w - 175 * dpr, Math.max(padL, cx + 6 * dpr)), padT + 12 * dpr);
}

function setDelta(elId, cur, base, lowerIsBetter = true, suffix = "") {
  const el = $(elId);
  if (base === undefined || base === null) {
    el.textContent = "";
    el.className = "metric-delta neutral";
    return;
  }
  const diff = cur - base;
  if (Math.abs(diff) < 0.005) {
    el.textContent = `0${suffix}`;
    el.className = "metric-delta neutral";
    return;
  }
  const sign = diff > 0 ? "+" : "";
  const good = lowerIsBetter ? diff < 0 : diff > 0;
  el.textContent = `${sign}${diff.toFixed(2)}${suffix}`;
  el.className = `metric-delta ${good ? "pos" : "neg"}`;
}

function render() {
  const { link, bytes, rawBytes, packedBytes, hz, encoding } = getInputs();
  const profile = link.profile;
  const r = evalLink(link, bytes, hz);
  const kind = verdictKind(r.verdict);
  const maxSweep = Math.max(10, parseFloat($("sweep-max-hz")?.value || "50"), hz * 1.5);
  const base = state.compareIdx !== "" ? state.snapshots[Number(state.compareIdx)] : null;
  const overlaySnap = base && state.cmpMode === "overlay" ? base : null;

  state.lastCtx = { ...r, profile, linkLabel: link.label, bytes, hz, encoding, kind };

  if (state.view !== "play") {
    $("profile-notes").textContent = profile.notes || "";
  }

  if (state.view === "play") {
    drawFrameDiagram($("play-frame-canvas"), profile, r, bytes);
    drawSweepChart($("play-curve-canvas"), link, bytes, hz, Math.max(25, hz * 1.5));
    $("play-frame-cap").textContent =
      `${bytes} payload B -> ${r.fragments_per_message} fragment(s), ${r.wire_bytes_per_message} B on wire (${r.header_overhead_pct}% header tax).`;
    $("play-live-caption").textContent =
      `${kind}: ${fmtRate(r.required_wire_bps)} offered (${r.utilization_pct}% of ${fmtRate(r.effective_goodput_bps)}) | Max ${r.max_sustainable_hz} Hz | ${r.packet_loss_prob_pct}% BER loss`;
  } else if (state.view === "simple") {
    const pill = $("regime-pill");
    pill.className = `regime-pill ${kind}`;
    pill.textContent = `Link Verdict: ${kind} (${link.label})`;
    $("regime-meta").textContent = r.verdict;

    drawFrameDiagram($("simple-frame-canvas"), profile, r, bytes);
    drawSweepChart($("simple-curve-canvas"), link, bytes, hz, maxSweep, overlaySnap);
    $("simple-left-cap").textContent = `${encoding}: ${bytes} B payload -> ${r.wire_bytes_per_message} B wire (${r.fragments_per_message} MTU chunk${r.fragments_per_message > 1 ? "s" : ""}).`;
    $("simple-right-cap").textContent = `Offered ${fmtRate(r.required_wire_bps)} vs. ${fmtRate(r.effective_goodput_bps)} goodput (Shannon ${fmtRate(r.shannon_ceiling_bps)}).`;

    const showBaseBelow = Boolean(base && state.cmpMode === "baseline");
    $("simple-compare-row").classList.toggle("hidden", !showBaseBelow);
    if (showBaseBelow) {
      drawFrameDiagram($("simple-base-frame-canvas"), base.profile, base, base.bytes);
      drawSweepChart($("simple-base-curve-canvas"), { mode: "custom", profile: base.profile }, base.bytes, base.hz, maxSweep);
      $("simple-base-left-cap").textContent = `Baseline: ${base.linkLabel} | ${base.encoding} (${base.bytes} B)`;
      $("simple-base-right-cap").textContent = `Baseline: ${fmtRate(base.required_wire_bps)} (${base.utilization_pct}% util)`;
    }

    $("m-util").textContent = `${r.utilization_pct}%`;
    $("p-util").style.width = `${Math.min(100, r.utilization_pct)}%`;
    $("p-util").style.background = verdictColor(kind);
    setDelta("d-util", r.utilization_pct, base?.utilization_pct, true, "%");

    $("m-tax").textContent = `${r.header_overhead_pct}%`;
    $("p-tax").style.width = `${Math.min(100, r.header_overhead_pct)}%`;
    setDelta("d-tax", r.header_overhead_pct, base?.header_overhead_pct, true, "%");

    $("m-loss").textContent = `${r.packet_loss_prob_pct}%`;
    $("p-loss").style.width = `${Math.min(100, r.packet_loss_prob_pct * 4)}%`;
    setDelta("d-loss", r.packet_loss_prob_pct, base?.packet_loss_prob_pct, true, "%");

    $("m-maxhz").textContent = `${r.max_sustainable_hz} Hz`;
    setDelta("d-maxhz", r.max_sustainable_hz, base?.max_sustainable_hz, false, " Hz");
    $("simple-swatch").style.background = verdictColor(kind);
    $("simple-status-txt").textContent = kind;
    $("m-airtime-note").textContent = `Airtime: ${r.airtime_ms_per_message} ms/msg`;
  } else {
    $("b-wire").textContent = fmtRate(r.required_wire_bps);
    $("b-goodput").textContent = fmtRate(r.effective_goodput_bps);
    $("b-shannon").textContent = fmtRate(r.shannon_ceiling_bps);
    $("b-util").textContent = `${r.utilization_pct}%`;
    $("b-frags").textContent = `${r.fragments_per_message}`;
    $("bench-swatch").style.background = verdictColor(kind);
    $("bench-verdict-short").textContent = kind;
    $("b-wirebytes").textContent = `${r.wire_bytes_per_message} B`;
    $("b-tax").textContent = `${r.header_overhead_pct}%`;
    $("b-airtime").textContent = `${r.airtime_ms_per_message} ms`;
    $("b-loss-hz").textContent = `${r.packet_loss_prob_pct}% / ${r.max_sustainable_hz}Hz`;

    setDelta("bd-wire", r.required_wire_bps, base?.required_wire_bps, true, " bps");
    setDelta("bd-util", r.utilization_pct, base?.utilization_pct, true, "%");
    setDelta("bd-frags", r.fragments_per_message, base?.fragments_per_message, true, "");
    setDelta("bd-wirebytes", r.wire_bytes_per_message, base?.wire_bytes_per_message, true, " B");
    setDelta("bd-tax", r.header_overhead_pct, base?.header_overhead_pct, true, "%");
    setDelta("bd-airtime", r.airtime_ms_per_message, base?.airtime_ms_per_message, true, " ms");
    setDelta("bd-maxhz", r.max_sustainable_hz, base?.max_sustainable_hz, false, " Hz");

    drawFrameDiagram($("bench-frame-canvas"), profile, r, bytes);
    $("bench-left-cap").textContent = `${encoding} on ${link.label}: ${r.verdict}`;
    const cmp = drawEncodingComparison($("bench-encoding-canvas"), link, rawBytes, packedBytes, hz);
    $("bench-right-cap").textContent = `Packing reduces wire bytes by ${cmp.saved}% (${cmp.rawR.wire_bytes_per_message}B -> ${cmp.binR.wire_bytes_per_message}B).`;

    const showBaseBelow = Boolean(base && state.cmpMode === "baseline");
    $("bench-compare-row").classList.toggle("hidden", !showBaseBelow);
    if (showBaseBelow) {
      drawFrameDiagram($("bench-base-frame-canvas"), base.profile, base, base.bytes);
      drawSweepChart($("bench-base-curve-canvas"), { mode: "custom", profile: base.profile }, base.bytes, base.hz, maxSweep);
      $("bench-base-left-cap").textContent = `Baseline: ${base.linkLabel} | ${base.bytes} B @ ${base.hz} Hz`;
      $("bench-base-right-cap").textContent = `Baseline Sweep: ${fmtRate(base.required_wire_bps)} (${base.utilization_pct}% util)`;
    }

    drawSweepChart($("plot-sweep-canvas"), link, bytes, hz, maxSweep, overlaySnap);
    drawSnrWaterfall($("plot-snr-canvas"), profile, bytes, hz);
    $("bench-json-details").textContent = JSON.stringify(r, null, 2);
  }
}

function setView(v) {
  state.view = v;
  document.querySelector(".app-shell").classList.toggle("play-mode", v === "play");
  document.querySelectorAll("#view-mode-tabs .seg-btn").forEach((b) => b.classList.toggle("active", b.dataset.view === v));
  $("sidebar-play-controls").classList.toggle("hidden", v !== "play");
  $("sidebar-full-controls").classList.toggle("hidden", v === "play");
  document.querySelector(".snapshot-bar").classList.toggle("hidden", v === "play");
  $("play-view").classList.toggle("hidden", v !== "play");
  $("simple-view").classList.toggle("hidden", v !== "simple");
  $("bench-view").classList.toggle("hidden", v !== "bench");
  render();
}

document.querySelectorAll("#view-mode-tabs .seg-btn").forEach((b) =>
  b.addEventListener("click", () => setView(b.dataset.view)),
);

document.querySelectorAll("#source-type-tabs .seg-btn").forEach((btn) =>
  btn.addEventListener("click", () => {
    state.source = btn.dataset.source;
    document.querySelectorAll("#source-type-tabs .seg-btn").forEach((b) => b.classList.toggle("active", b === btn));
    $("text-controls").classList.toggle("hidden", state.source !== "text");
    $("bytes-controls").classList.toggle("hidden", state.source !== "bytes");
    render();
  }),
);

document.querySelectorAll("#visual-cmp-tabs .seg-btn").forEach((btn) =>
  btn.addEventListener("click", () => {
    state.cmpMode = btn.dataset.cmp;
    document.querySelectorAll("#visual-cmp-tabs .seg-btn").forEach((b) => b.classList.toggle("active", b === btn));
    render();
  }),
);

document.querySelectorAll("[data-play-preset]").forEach((btn) =>
  btn.addEventListener("click", () => {
    const p = PLAY_PRESETS[btn.dataset.playPreset];
    $("play-payload-input").value = p.text;
    $("play-profile-select").value = p.profile;
    $("play-hz-slider").value = p.hz;
    $("play-hz-val").textContent = `${p.hz} Hz`;
    render();
  }),
);

document.querySelectorAll("[data-preset]").forEach((btn) =>
  btn.addEventListener("click", () => {
    $("text-input").value = FULL_PRESETS[btn.dataset.preset];
    $("payload-file-info").classList.add("hidden");
    render();
  }),
);

$("payload-file-input").addEventListener("change", async (e) => {
  const f = e.target.files?.[0];
  if (!f) return;
  const txt = await f.text();
  $("text-input").value = txt;
  $("payload-file-info").textContent = `Loaded ${f.name} (${utf8Bytes(txt)} bytes)`;
  $("payload-file-info").classList.remove("hidden");
  render();
});

document.querySelectorAll('input[name="link-mode"]').forEach((r) =>
  r.addEventListener("change", (e) => {
    state.linkMode = e.target.value;
    $("custom-link-controls").classList.toggle("hidden", state.linkMode !== "custom");
    if (state.linkMode === "custom") loadProfileIntoCustomInputs($("profile-select").value);
    render();
  }),
);

$("profile-select").addEventListener("change", () => {
  if (state.linkMode === "custom") loadProfileIntoCustomInputs($("profile-select").value);
  render();
});

$("reset-custom-btn").addEventListener("click", () => {
  loadProfileIntoCustomInputs($("profile-select").value);
  render();
});

for (const id of ["play-payload-input", "play-profile-select", "text-input", "simulate-binary", "sweep-max-hz", "c-bw", "c-goodput", "c-mtu", "c-hdr", "c-lat"]) {
  $(id).addEventListener("input", render);
  $(id).addEventListener("change", render);
}

$("play-hz-slider").addEventListener("input", (e) => {
  $("play-hz-val").textContent = `${e.target.value} Hz`;
  render();
});

$("c-snr-slider").addEventListener("input", (e) => {
  $("c-snr-val").textContent = `${e.target.value} dB`;
  render();
});

$("bytes-slider").addEventListener("input", (e) => {
  $("bytes-number").value = e.target.value;
  $("bytes-val").textContent = `${e.target.value} B`;
  render();
});
$("bytes-number").addEventListener("input", (e) => {
  const v = Math.max(1, parseInt(e.target.value || "1", 10));
  $("bytes-slider").value = Math.min(1500, v);
  $("bytes-val").textContent = `${v} B`;
  render();
});

$("hz-slider").addEventListener("input", (e) => {
  const hz = Math.pow(10, parseFloat(e.target.value));
  const txt = hz.toFixed(hz < 10 ? 2 : 1);
  $("hz-number").value = txt;
  $("hz-val").textContent = `${txt} Hz`;
  render();
});
$("hz-number").addEventListener("input", (e) => {
  const hz = Math.max(0.01, parseFloat(e.target.value || "1"));
  $("hz-slider").value = Math.min(3, Math.max(-1, Math.log10(hz)));
  $("hz-val").textContent = `${hz} Hz`;
  render();
});

function refreshSnapshots() {
  const has = state.snapshots.length > 0;
  $("snapshots-section").classList.toggle("hidden", !has);
  $("compare-select").classList.toggle("hidden", !has);
  $("visual-cmp-tabs").classList.toggle("hidden", !has);
  $("clear-snapshots-btn").classList.toggle("hidden", !has);
  $("compare-tip").classList.toggle("hidden", has);
  $("snapshots-title").textContent = `Pinned Snapshots (${state.snapshots.length})`;

  const sel = $("compare-select");
  const prev = sel.value;
  sel.innerHTML = `<option value="">None (no comparison)</option>` +
    state.snapshots.map((s, i) => `<option value="${i}">#${i + 1}: ${s.linkLabel} (${s.bytes}B @ ${s.hz}Hz, ${s.encoding})</option>`).join("");
  sel.value = prev && state.snapshots[Number(prev)] ? prev : "";
  state.compareIdx = sel.value;

  $("snapshots-table").querySelector("tbody").innerHTML = state.snapshots.map((s, i) => `
    <tr>
      <td>#${i + 1}</td>
      <td>${s.linkLabel}</td>
      <td>${s.encoding}</td>
      <td>${s.bytes}</td>
      <td>${s.hz}</td>
      <td>${s.required_wire_bps}</td>
      <td>${s.utilization_pct}%</td>
      <td>${s.fragments_per_message}</td>
      <td>${s.header_overhead_pct}%</td>
      <td>${s.packet_loss_prob_pct}%</td>
      <td>${s.airtime_ms_per_message}</td>
      <td>${s.max_sustainable_hz}</td>
      <td>${s.kind}</td>
    </tr>`).join("");
}

$("pin-snapshot-btn").addEventListener("click", () => {
  if (!state.lastCtx) return;
  state.snapshots.push({ ...state.lastCtx, profile: { ...state.lastCtx.profile } });
  const shouldSelectFirst = state.snapshots.length === 1;
  refreshSnapshots();
  if (shouldSelectFirst) {
    $("compare-select").value = "0";
    state.compareIdx = "0";
  }
  render();
});

$("compare-select").addEventListener("change", (e) => {
  state.compareIdx = e.target.value;
  render();
});

$("clear-snapshots-btn").addEventListener("click", () => {
  state.snapshots = [];
  state.compareIdx = "";
  refreshSnapshots();
  render();
});

$("download-csv-btn").addEventListener("click", () => {
  const hdr = "id,link,encoding,bytes,hz,required_wire_bps,utilization_pct,fragments,header_tax_pct,ber_loss_pct,airtime_ms,max_hz,verdict\n";
  const rows = state.snapshots.map((s, i) =>
    [i + 1, `"${s.linkLabel}"`, `"${s.encoding}"`, s.bytes, s.hz, s.required_wire_bps, s.utilization_pct, s.fragments_per_message, s.header_overhead_pct, s.packet_loss_prob_pct, s.airtime_ms_per_message, s.max_sustainable_hz, s.kind].join(","),
  ).join("\n");
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([hdr + rows], { type: "text/csv" }));
  a.download = "linkguard_snapshots.csv";
  a.click();
});

$("copy-json-btn").addEventListener("click", async () => {
  await navigator.clipboard.writeText($("bench-json-details").textContent);
  $("copy-json-btn").textContent = "Copied!";
  setTimeout(() => ($("copy-json-btn").textContent = "Copy JSON"), 1200);
});

$("copy-sweep-csv").addEventListener("click", async () => {
  const csv = "hz,required_wire_bps,effective_goodput_bps,shannon_ceiling_bps\n" +
    state.lastSweep.map((p) => `${p.hz.toFixed(3)},${p.wire.toFixed(2)},${p.goodput.toFixed(2)},${p.shannon.toFixed(2)}`).join("\n");
  await navigator.clipboard.writeText(csv);
  $("copy-sweep-csv").textContent = "Copied!";
  setTimeout(() => ($("copy-sweep-csv").textContent = "Copy CSV"), 1200);
});

$("copy-snr-csv").addEventListener("click", async () => {
  const csv = "snr_db,packet_loss_prob_pct,shannon_ceiling_bps\n" +
    state.lastSnrCurve.map((p) => `${p.snr.toFixed(1)},${p.loss.toFixed(3)},${p.shannon.toFixed(2)}`).join("\n");
  await navigator.clipboard.writeText(csv);
  $("copy-snr-csv").textContent = "Copied!";
  setTimeout(() => ($("copy-snr-csv").textContent = "Copy CSV"), 1200);
});

for (const [btnId, canvasId, fname] of [
  ["save-frame-png", "bench-frame-canvas", "linkguard_frame.png"],
  ["save-sweep-png", "plot-sweep-canvas", "linkguard_sweep.png"],
  ["save-snr-png", "plot-snr-canvas", "linkguard_snr_waterfall.png"],
]) {
  $(btnId).addEventListener("click", () => {
    const a = document.createElement("a");
    a.href = $(canvasId).toDataURL("image/png");
    a.download = fname;
    a.click();
  });
}

$("plot-sweep-canvas").addEventListener("mousemove", (e) => {
  if (!state.lastSweep.length) return;
  const r = e.target.getBoundingClientRect();
  const frac = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  const p = state.lastSweep[Math.min(state.lastSweep.length - 1, Math.round(frac * (state.lastSweep.length - 1)))];
  $("plot-sweep-readout").textContent = `${p.hz.toFixed(2)} Hz -> ${fmtRate(p.wire)} (Goodput: ${fmtRate(p.goodput)})`;
});

$("plot-snr-canvas").addEventListener("mousemove", (e) => {
  if (!state.lastSnrCurve.length) return;
  const r = e.target.getBoundingClientRect();
  const frac = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  const p = state.lastSnrCurve[Math.min(state.lastSnrCurve.length - 1, Math.round(frac * (state.lastSnrCurve.length - 1)))];
  $("plot-snr-readout").textContent = `SNR ${p.snr.toFixed(1)} dB -> ${p.loss.toFixed(2)}% packet loss | Shannon ${fmtRate(p.shannon)}`;
});

window.addEventListener("resize", render);
render();