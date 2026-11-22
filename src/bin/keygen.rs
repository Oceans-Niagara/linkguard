use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "linkguard-keygen", about = "Dev-only offline Ed25519 license minter")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a fresh Ed25519 keypair (saves private key, prints public key for src/license.rs)
    Init {
        #[arg(long, default_value = "linkguard-private.key")]
        out: PathBuf,
    },
    /// Mint a signed LG1 offline license token
    Mint {
        #[arg(long, default_value = "linkguard-private.key")]
        key: PathBuf,
        #[arg(long, default_value = "pro")]
        tier: String,
        #[arg(long)]
        holder: String,
        #[arg(long)]
        days: Option<u64>,
        #[arg(long, default_value = "lic-1")]
        id: String,
    },
}

#[derive(Serialize)]
struct Claims {
    tier: String,
    holder: String,
    issued_unix: u64,
    expires_unix: Option<u64>,
    license_id: String,
}

fn main() {
    let cli = Cli::parse();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    match cli.command {
        Cmd::Init { out } => {
            let mut secret = [0u8; 32];
            OsRng.fill_bytes(&mut secret);
            let sk = SigningKey::from_bytes(&secret);
            let vk = sk.verifying_key();

            let sk_b64 = URL_SAFE_NO_PAD.encode(sk.to_bytes());
            let vk_b64 = URL_SAFE_NO_PAD.encode(vk.to_bytes());

            fs::write(&out, sk_b64).expect("Failed to write private key");
            println!("Saved private key to {} (NEVER commit this file!)", out.display());
            println!("\nPaste this into src/license.rs:");
            println!("pub const EMBEDDED_PUBLIC_KEY_B64: &str = \"{vk_b64}\";");
        }
        Cmd::Mint { key, tier, holder, days, id } => {
            let sk_raw = fs::read_to_string(&key).expect("Failed to read private key");
            let sk_bytes = URL_SAFE_NO_PAD
                .decode(sk_raw.trim())
                .expect("Invalid private key base64");
            let sk_arr: [u8; 32] = sk_bytes.try_into().expect("Private key must be 32 bytes");
            let sk = SigningKey::from_bytes(&sk_arr);

            let expires_unix = days.map(|d| now + d * 86_400);
            let claims = Claims {
                tier: tier.to_lowercase(),
                holder,
                issued_unix: now,
                expires_unix,
                license_id: id,
            };

            let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
            let msg = format!("LG1.{payload_b64}");
            let sig = sk.sign(msg.as_bytes());
            let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
            println!("{msg}.{sig_b64}");
        }
    }
}