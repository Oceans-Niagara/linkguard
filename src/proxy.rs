use crate::physics::{estimate_ber_from_snr, LinkProfile};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy)]
pub enum ProxyTransport {
    Tcp,
    Udp,
}

#[derive(Debug, Serialize)]
pub struct ProxySummary {
    pub transport: String,
    pub profile: String,
    pub seed: u64,
    pub active_secs: f64,
    pub packets_in: u64,
    pub packets_forwarded: u64,
    pub bytes_in: u64,
    pub bytes_forwarded: u64,
    pub wire_bytes_billed: u64,
    pub fragments: u64,
    pub queue_drops: u64,
    pub ber_drops: u64,
    pub offered_load_bps: f64,
    pub delivered_goodput_bps: f64,
}

struct ShaperState {
    tokens_bits: f64,
    last_refill: Instant,
    first_packet_at: Option<Instant>,
    queued_bytes: usize,
    rng: ChaCha8Rng,
    packets_in: u64,
    packets_forwarded: u64,
    bytes_in: u64,
    bytes_forwarded: u64,
    wire_bytes_billed: u64,
    fragments: u64,
    queue_drops: u64,
    ber_drops: u64,
}

struct LinkShaper {
    transport: ProxyTransport,
    profile: LinkProfile,
    seed: u64,
    started_at: Instant,
    state: Mutex<ShaperState>,
}

enum PacketDecision {
    Forward { delay: Duration, wire_bytes: usize },
    DropQueue,
    DropBer,
}

impl LinkShaper {
    fn new(transport: ProxyTransport, profile: LinkProfile, seed: u64) -> Self {
        let now = Instant::now();
        Self {
            transport,
            profile: profile.clone(),
            seed,
            started_at: now,
            state: Mutex::new(ShaperState {
                tokens_bits: (profile.max_payload_bytes * 8) as f64,
                last_refill: now,
                first_packet_at: None,
                queued_bytes: 0,
                rng: ChaCha8Rng::seed_from_u64(seed),
                packets_in: 0,
                packets_forwarded: 0,
                bytes_in: 0,
                bytes_forwarded: 0,
                wire_bytes_billed: 0,
                fragments: 0,
                queue_drops: 0,
                ber_drops: 0,
            }),
        }
    }

    async fn admit_and_shape(&self, payload_len: usize) -> PacketDecision {
        let mut st = self.state.lock().await;
        let now = Instant::now();
        if st.first_packet_at.is_none() {
            st.first_packet_at = Some(now);
        }
        let dt = now.duration_since(st.last_refill).as_secs_f64();
        st.last_refill = now;

        let max_bucket = (self.profile.max_payload_bytes.max(128) * 16) as f64;
        st.tokens_bits = (st.tokens_bits + dt * self.profile.effective_goodput_bps).min(max_bucket);

        st.packets_in += 1;
        st.bytes_in += payload_len as u64;

        let mtu = self.profile.max_payload_bytes.max(1);
        let frags = payload_len.div_ceil(mtu);
        let wire_bytes = payload_len + frags * self.profile.header_overhead_bytes;
        let wire_bits = (wire_bytes * 8) as f64;
        st.fragments += frags as u64;
        st.wire_bytes_billed += wire_bytes as u64;

        if st.queued_bytes + wire_bytes > self.profile.queue_capacity_bytes {
            st.queue_drops += 1;
            return PacketDecision::DropQueue;
        }

        let ber = estimate_ber_from_snr(self.profile.snr_db);
        let p_loss = 1.0 - (1.0 - ber).powi((wire_bytes * 8) as i32);
        let roll: f64 = st.rng.gen();
        if roll < p_loss {
            st.ber_drops += 1;
            return PacketDecision::DropBer;
        }

        st.tokens_bits -= wire_bits;
        let deficit_secs = if st.tokens_bits < 0.0 {
            (-st.tokens_bits) / self.profile.effective_goodput_bps
        } else {
            0.0
        };
        let jitter_factor: f64 = st.rng.gen();
        let total_ms = self.profile.base_latency_ms
            + jitter_factor * self.profile.jitter_ms
            + deficit_secs * 1000.0;

        st.queued_bytes += wire_bytes;
        st.packets_forwarded += 1;
        st.bytes_forwarded += payload_len as u64;

        PacketDecision::Forward {
            delay: Duration::from_secs_f64((total_ms / 1000.0).max(0.0)),
            wire_bytes,
        }
    }

    async fn release_queue(&self, wire_bytes: usize) {
        let mut st = self.state.lock().await;
        st.queued_bytes = st.queued_bytes.saturating_sub(wire_bytes);
    }

    async fn summary(&self) -> ProxySummary {
        let st = self.state.lock().await;
        let active_secs = st
            .first_packet_at
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or_else(|| self.started_at.elapsed().as_secs_f64())
            .max(1e-3);

        ProxySummary {
            transport: match self.transport {
                ProxyTransport::Tcp => "tcp".into(),
                ProxyTransport::Udp => "udp".into(),
            },
            profile: self.profile.id.to_string(),
            seed: self.seed,
            active_secs: (active_secs * 100.0).round() / 100.0,
            packets_in: st.packets_in,
            packets_forwarded: st.packets_forwarded,
            bytes_in: st.bytes_in,
            bytes_forwarded: st.bytes_forwarded,
            wire_bytes_billed: st.wire_bytes_billed,
            fragments: st.fragments,
            queue_drops: st.queue_drops,
            ber_drops: st.ber_drops,
            offered_load_bps: ((st.bytes_in as f64 * 8.0 / active_secs) * 100.0).round() / 100.0,
            delivered_goodput_bps: ((st.bytes_forwarded as f64 * 8.0 / active_secs) * 100.0).round() / 100.0,
        }
    }
}

pub async fn run_proxy(
    listen_addr: &str,
    target_addr: &str,
    profile: LinkProfile,
    seed: u64,
    transport: ProxyTransport,
) -> Result<(), String> {
    match transport {
        ProxyTransport::Tcp => run_tcp_proxy(listen_addr, target_addr, profile, seed).await,
        ProxyTransport::Udp => run_udp_proxy(listen_addr, target_addr, profile, seed).await,
    }
}

async fn run_tcp_proxy(
    listen_addr: &str,
    target_addr: &str,
    profile: LinkProfile,
    seed: u64,
) -> Result<(), String> {
    let listener = TcpListener::bind(listen_addr)
        .await
        .map_err(|e| format!("Failed to bind TCP {listen_addr}: {e}"))?;

    let shaper = Arc::new(LinkShaper::new(ProxyTransport::Tcp, profile.clone(), seed));
    eprintln!(
        "[linkguard tcp proxy] {} -> {} | profile={} ({:.0} bps, MTU={} B, seed={})",
        listen_addr, target_addr, profile.id, profile.effective_goodput_bps, profile.max_payload_bytes, seed
    );
    eprintln!("[linkguard tcp proxy] Press Ctrl-C to stop and print deterministic JSON summary.");

    let target_owned = target_addr.to_string();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let summary = shaper.summary().await;
                println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
                break;
            }
            accepted = listener.accept() => {
                let (client, peer) = accepted.map_err(|e| e.to_string())?;
                eprintln!("[linkguard tcp proxy] accepted connection from {peer}");
                let tgt = target_owned.clone();
                let shaper_conn = Arc::clone(&shaper);
                tokio::spawn(async move {
                    if let Err(e) = handle_tcp_connection(client, &tgt, shaper_conn).await {
                        eprintln!("[linkguard tcp proxy] connection error: {e}");
                    }
                });
            }
        }
    }
    Ok(())
}

async fn handle_tcp_connection(
    client: TcpStream,
    target_addr: &str,
    shaper: Arc<LinkShaper>,
) -> Result<(), String> {
    let server = TcpStream::connect(target_addr)
        .await
        .map_err(|e| format!("Failed to connect to {target_addr}: {e}"))?;

    let (mut c_read, mut c_write) = client.into_split();
    let (mut s_read, mut s_write) = server.into_split();

    let s_up = Arc::clone(&shaper);
    let up = tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            let n = match c_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            match s_up.admit_and_shape(n).await {
                PacketDecision::Forward { delay, wire_bytes } => {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    s_up.release_queue(wire_bytes).await;
                    if s_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                PacketDecision::DropQueue | PacketDecision::DropBer => continue,
            }
        }
    });

    let down = tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            let n = match s_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            match shaper.admit_and_shape(n).await {
                PacketDecision::Forward { delay, wire_bytes } => {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    shaper.release_queue(wire_bytes).await;
                    if c_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                PacketDecision::DropQueue | PacketDecision::DropBer => continue,
            }
        }
    });

    let _ = tokio::join!(up, down);
    Ok(())
}

async fn run_udp_proxy(
    listen_addr: &str,
    target_addr: &str,
    profile: LinkProfile,
    seed: u64,
) -> Result<(), String> {
    let listener = Arc::new(
        UdpSocket::bind(listen_addr)
            .await
            .map_err(|e| format!("Failed to bind UDP {listen_addr}: {e}"))?,
    );
    let shaper = Arc::new(LinkShaper::new(ProxyTransport::Udp, profile.clone(), seed));
    eprintln!(
        "[linkguard udp proxy] {} -> {} | profile={} ({:.0} bps, MTU={} B, seed={})",
        listen_addr, target_addr, profile.id, profile.effective_goodput_bps, profile.max_payload_bytes, seed
    );
    eprintln!("[linkguard udp proxy] Press Ctrl-C to stop and print deterministic JSON summary.");

    let mut peers: HashMap<SocketAddr, Arc<UdpSocket>> = HashMap::new();
    let mut buf = vec![0u8; 65_535];

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let summary = shaper.summary().await;
                println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
                break;
            }
            recv = listener.recv_from(&mut buf) => {
                let (n, peer_addr) = recv.map_err(|e| e.to_string())?;
                let pkt = buf[..n].to_vec();

                let upstream = if let Some(sock) = peers.get(&peer_addr) {
                    Arc::clone(sock)
                } else {
                    let sock = Arc::new(UdpSocket::bind("0.0.0.0:0").await.map_err(|e| e.to_string())?);
                    sock.connect(target_addr).await.map_err(|e| format!("Failed to connect UDP to {target_addr}: {e}"))?;
                    peers.insert(peer_addr, Arc::clone(&sock));

                    // Spawn return-path reader for this peer
                    let up_rx = Arc::clone(&sock);
                    let down_tx = Arc::clone(&listener);
                    let shaper_down = Arc::clone(&shaper);
                    tokio::spawn(async move {
                        let mut rbuf = vec![0u8; 65_535];
                        while let Ok(rn) = up_rx.recv(&mut rbuf).await {
                            let rpkt = rbuf[..rn].to_vec();
                            let sh = Arc::clone(&shaper_down);
                            let tx = Arc::clone(&down_tx);
                            tokio::spawn(async move {
                                if let PacketDecision::Forward { delay, wire_bytes } = sh.admit_and_shape(rn).await {
                                    if !delay.is_zero() {
                                        tokio::time::sleep(delay).await;
                                    }
                                    sh.release_queue(wire_bytes).await;
                                    let _ = tx.send_to(&rpkt, peer_addr).await;
                                }
                            });
                        }
                    });
                    sock
                };

                let sh = Arc::clone(&shaper);
                tokio::spawn(async move {
                    if let PacketDecision::Forward { delay, wire_bytes } = sh.admit_and_shape(n).await {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        sh.release_queue(wire_bytes).await;
                        let _ = upstream.send(&pkt).await;
                    }
                });
            }
        }
    }
    Ok(())
}