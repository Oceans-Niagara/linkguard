use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Replace this placeholder by running:
///   cargo run --features dev-keygen --bin linkguard-keygen -- init
pub const EMBEDDED_PUBLIC_KEY_B64: &str = "TMUezvEDd_pBdrqZR3_2qfVn48lD7azxhWGMn9Ph308";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LicenseTier {
    Free,
    Pro,
    Team,
}

impl LicenseTier {
    pub fn is_paid(self) -> bool {
        matches!(self, Self::Pro | Self::Team)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseClaims {
    pub tier: LicenseTier,
    pub holder: String,
    pub issued_unix: u64,
    pub expires_unix: Option<u64>,
    pub license_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LicenseStatus {
    pub tier: LicenseTier,
    pub valid_paid: bool,
    pub source: String,
    pub claims: Option<LicenseClaims>,
    pub error: Option<String>,
}

pub fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn embedded_verifying_key() -> Result<VerifyingKey, String> {
    let raw = URL_SAFE_NO_PAD
        .decode(EMBEDDED_PUBLIC_KEY_B64.trim())
        .map_err(|e| format!("Invalid embedded public key base64: {e}"))?;
    let arr: [u8; 32] = raw
        .try_into()
        .map_err(|_| "Embedded public key must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("Invalid Ed25519 public key: {e}"))
}

pub fn verify_token_with_key(
    token: &str,
    key: &VerifyingKey,
    now_unix: u64,
) -> Result<LicenseClaims, String> {
    let trimmed = token.trim();
    let mut parts = trimmed.split('.');
    let prefix = parts.next().unwrap_or("");
    let payload_b64 = parts.next().ok_or("Malformed license: missing payload")?;
    let sig_b64 = parts.next().ok_or("Malformed license: missing signature")?;
    if prefix != "LG1" || parts.next().is_some() {
        return Err("Invalid license format (expected LG1.<payload>.<sig>)".into());
    }

    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|e| format!("Invalid license signature encoding: {e}"))?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "Invalid license signature length (expected 64 bytes)".to_string())?;
    let sig = Signature::from_bytes(&sig_arr);

    let signed_msg = format!("LG1.{payload_b64}");
    key.verify_strict(signed_msg.as_bytes(), &sig)
        .map_err(|_| "License signature verification failed".to_string())?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|e| format!("Invalid license payload encoding: {e}"))?;
    let claims: LicenseClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|e| format!("Invalid license JSON claims: {e}"))?;

    if let Some(exp) = claims.expires_unix {
        if now_unix > exp {
            return Err(format!(
                "License '{}' expired at unix timestamp {exp}",
                claims.license_id
            ));
        }
    }

    Ok(claims)
}

fn default_license_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("linkguard.lic")];
    if let Some(appdata) = env::var_os("APPDATA") {
        paths.push(PathBuf::from(appdata).join("linkguard").join("license.lic"));
    }
    if let Some(home) = env::var_os("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".config")
                .join("linkguard")
                .join("license.lic"),
        );
    }
    paths
}

pub fn check_license() -> LicenseStatus {
    let key = match embedded_verifying_key() {
        Ok(k) => k,
        Err(e) => {
            return LicenseStatus {
                tier: LicenseTier::Free,
                valid_paid: false,
                source: "none".into(),
                claims: None,
                error: Some(e),
            }
        }
    };

    if let Ok(env_val) = env::var("LINKGUARD_LICENSE") {
        let trimmed = env_val.trim();
        if !trimmed.is_empty() {
            let (token, src) = if !trimmed.starts_with("LG1.") && Path::new(trimmed).is_file() {
                match fs::read_to_string(trimmed) {
                    Ok(contents) => (contents, format!("LINKGUARD_LICENSE ({trimmed})")),
                    Err(e) => {
                        return LicenseStatus {
                            tier: LicenseTier::Free,
                            valid_paid: false,
                            source: format!("LINKGUARD_LICENSE ({trimmed})"),
                            claims: None,
                            error: Some(format!("Failed to read license file: {e}")),
                        };
                    }
                }
            } else {
                (trimmed.to_string(), "LINKGUARD_LICENSE".to_string())
            };
            return evaluate_token(&token, &src, &key);
        }
    }

    for path in default_license_paths() {
        if path.is_file() {
            let src = path.display().to_string();
            match fs::read_to_string(&path) {
                Ok(contents) => return evaluate_token(&contents, &src, &key),
                Err(e) => {
                    return LicenseStatus {
                        tier: LicenseTier::Free,
                        valid_paid: false,
                        source: src,
                        claims: None,
                        error: Some(format!("Failed to read license file: {e}")),
                    };
                }
            }
        }
    }

    LicenseStatus {
        tier: LicenseTier::Free,
        valid_paid: false,
        source: "none".into(),
        claims: None,
        error: None,
    }
}

fn evaluate_token(token: &str, source: &str, key: &VerifyingKey) -> LicenseStatus {
    match verify_token_with_key(token, key, current_unix_secs()) {
        Ok(claims) => {
            let valid_paid = claims.tier.is_paid();
            LicenseStatus {
                tier: claims.tier,
                valid_paid,
                source: source.to_string(),
                claims: Some(claims),
                error: None,
            }
        }
        Err(e) => LicenseStatus {
            tier: LicenseTier::Free,
            valid_paid: false,
            source: source.to_string(),
            claims: None,
            error: Some(e),
        },
    }
}

pub fn require_pro(feature_name: &str) -> Result<LicenseClaims, String> {
    let status = check_license();
    if status.valid_paid {
        if let Some(claims) = status.claims {
            return Ok(claims);
        }
    }
    let detail = status
        .error
        .map(|e| format!(" ({e})"))
        .unwrap_or_default();
    Err(format!(
        "'{feature_name}' requires a LinkGuard Pro or Team license{detail}. Set LINKGUARD_LICENSE or place a signed license at ./linkguard.lic."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn mint_test_token(sk: &SigningKey, claims: &LicenseClaims) -> String {
        let json = serde_json::to_vec(claims).unwrap();
        let payload_b64 = URL_SAFE_NO_PAD.encode(json);
        let msg = format!("LG1.{payload_b64}");
        let sig = sk.sign(msg.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{msg}.{sig_b64}")
    }

    #[test]
    fn ed25519_license_verifies_valid_rejects_tampered_and_expired() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();

        let claims = LicenseClaims {
            tier: LicenseTier::Pro,
            holder: "Test Lab".into(),
            issued_unix: 1_700_000_000,
            expires_unix: Some(1_800_000_000),
            license_id: "lic-001".into(),
        };
        let token = mint_test_token(&sk, &claims);

        let verified = verify_token_with_key(&token, &vk, 1_750_000_000).unwrap();
        assert_eq!(verified.tier, LicenseTier::Pro);
        assert_eq!(verified.holder, "Test Lab");

        // Expired token rejected
        assert!(verify_token_with_key(&token, &vk, 1_900_000_000).is_err());

        // Tampered payload rejected
        let mut tampered = token.clone();
        tampered.replace_range(6..7, if &token[6..7] == "A" { "B" } else { "A" });
        assert!(verify_token_with_key(&tampered, &vk, 1_750_000_000).is_err());
    }
}