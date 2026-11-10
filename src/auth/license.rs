//! License blob verification and offline caching
//!
//! The server returns an HMAC-SHA256 signed blob containing plan limits as
//! concrete values (render_scale, max_fps, watermark, etc.). The client
//! verifies the signature, then uses the values directly in computations.
//!
//! Offline grace: the blob is cached in SQLite. If the blob is less than
//! 7 days old, it's used even without network. After 7 days without a
//! refresh, entitlements fall back to degraded defaults.

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::time::Duration;

use super::Entitlements;
use crate::config;

/// 7 days in seconds
const OFFLINE_GRACE_SECS: i64 = 7 * 24 * 3600;

// HMAC key — supplied at build time, never stored in the source tree.
//
// build.rs reads `REACT_LICENSE_HMAC_KEY` (hex) and writes it XOR-split
// across `HMAC_KEY_XOR` / `HMAC_KEY_ROT`, so the plaintext never appears
// contiguously in the binary. This is NOT crypto-grade protection — it's a
// speed bump against casual hex-editor patching. The real protection is
// "derivation over gating": the license values are used directly in
// computations.
//
// Builds made without the variable carry an empty key and reject every
// signed blob (see `verify_and_parse`).
include!(concat!(env!("OUT_DIR"), "/license_key.rs"));

fn hmac_key() -> Vec<u8> {
    // black_box stops the optimizer from folding the two halves back into the
    // plaintext key at compile time, which would put it in the binary verbatim
    // (and did: it was readable with `strings` before this guard).
    std::hint::black_box(HMAC_KEY_XOR)
        .iter()
        .zip(std::hint::black_box(HMAC_KEY_ROT).iter())
        .map(|(a, b)| a ^ b)
        .collect()
}

/// License blob payload from server
#[derive(Debug, Deserialize)]
struct LicenseBlob {
    account_id: i64,
    plan_code: String,
    issued_at: i64,
    expires_at: i64,
    render_scale: f32,
    max_fps: u32,
    watermark: bool,
    max_record_minutes: u32,
    max_streams: u32,
    lighting_enabled: bool,
    youtube_enabled: bool,
}

/// Server response for /api/licensing/blob
#[derive(Debug, Deserialize)]
struct LicenseBlobResponse {
    license: String,   // base64-encoded JSON
    signature: String, // hex HMAC-SHA256
}

/// Fetch license blob from server, verify HMAC, cache to SQLite, return Entitlements.
pub fn fetch_license_blob(access_token: &str, db: &crate::db::Database) -> Result<Entitlements> {
    let url = format!("{}/licensing/blob", config::api_base());

    let response = ureq::get(&url)
        .timeout(Duration::from_secs(15))
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => anyhow!("Unauthorized"),
            ureq::Error::Status(code, _) => anyhow!("Server error ({})", code),
            ureq::Error::Transport(t) => anyhow!("Network error: {}", t),
        })?;

    let blob_response: LicenseBlobResponse = response
        .into_json()
        .map_err(|e| anyhow!("Invalid license response: {}", e))?;

    let entitlements = verify_and_parse(&blob_response.license, &blob_response.signature)?;

    // Cache to SQLite for offline use
    if let Err(e) = db.save_license_blob(&blob_response.license, &blob_response.signature) {
        log::error!("[License] Failed to cache blob: {}", e);
    }

    Ok(entitlements)
}

/// Verify HMAC signature and parse blob into Entitlements.
fn verify_and_parse(blob_b64: &str, signature_hex: &str) -> Result<Entitlements> {
    // Verify HMAC
    type HmacSha256 = Hmac<Sha256>;
    let key = hmac_key();
    if key.is_empty() {
        // HMAC accepts a zero-length key, so without this guard a blob signed
        // with the empty key would verify in builds that carry no key.
        return Err(anyhow!(
            "License verification unavailable: built without REACT_LICENSE_HMAC_KEY"
        ));
    }
    let mut mac = HmacSha256::new_from_slice(&key)
        .map_err(|e| anyhow!("HMAC init error: {}", e))?;
    mac.update(blob_b64.as_bytes());

    let expected_sig = hex_decode(signature_hex)?;
    mac.verify_slice(&expected_sig)
        .map_err(|_| anyhow!("License signature verification failed"))?;

    // Decode base64 → JSON → LicenseBlob
    let json_bytes = base64_decode(blob_b64)?;
    let blob: LicenseBlob = serde_json::from_slice(&json_bytes)
        .map_err(|e| anyhow!("License blob parse error: {}", e))?;

    // Check expiration
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if blob.expires_at < now {
        return Err(anyhow!("License blob expired"));
    }

    Ok(Entitlements {
        account_id: blob.account_id as u32,
        plan_code: blob.plan_code,
        render_scale: blob.render_scale,
        max_fps: blob.max_fps,
        watermark: blob.watermark,
        max_record_minutes: blob.max_record_minutes,
        max_streams: blob.max_streams,
        lighting_enabled: blob.lighting_enabled,
        youtube_enabled: blob.youtube_enabled,
    })
}

/// Load cached license blob from SQLite.
/// Returns None only if no blob is cached or its signature fails — a STALE
/// blob is still honored. DJs authenticate once and then run venues with no
/// internet for weeks; offline must never degrade a signed-in session
/// (Koob, 2026-06-12). Staleness is logged and the 24h online refresh cycle
/// replaces the blob whenever connectivity returns — so a real plan change
/// lands on the next online session, it just can't punish being offline.
pub fn load_cached_entitlements(db: &crate::db::Database) -> Option<Entitlements> {
    let (blob_b64, signature, fetched_at) = db.get_license_blob()?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if now - fetched_at > OFFLINE_GRACE_SECS {
        log::warn!(
            "[License] Cached blob is stale ({}s old, refresh window {}s) — honoring it anyway (offline-first)",
            now - fetched_at,
            OFFLINE_GRACE_SECS
        );
    }

    match verify_and_parse(&blob_b64, &signature) {
        Ok(ent) => {
            log::info!("[License] Loaded cached entitlements: plan={}", ent.plan_code);
            Some(ent)
        }
        Err(e) => {
            log::warn!("[License] Cached blob verification failed: {}", e);
            None
        }
    }
}

// ========== Simple codec helpers (no extra dependencies) ==========

fn base64_decode(input: &str) -> Result<Vec<u8>> {
    // Standard base64 decode (RFC 4648)
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let input = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(input.len() * 3 / 4);

    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &byte in input.as_bytes() {
        let val = TABLE.iter().position(|&c| c == byte)
            .ok_or_else(|| anyhow!("Invalid base64 character: {}", byte as char))? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

fn hex_decode(input: &str) -> Result<Vec<u8>> {
    if input.len() % 2 != 0 {
        return Err(anyhow!("Odd-length hex string"));
    }
    (0..input.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&input[i..i + 2], 16)
                .map_err(|e| anyhow!("Invalid hex: {}", e))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base64_encode(input: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
        }
        out
    }

    /// A well-formed, unexpired license blob (base64 JSON).
    fn valid_blob() -> String {
        let json = r#"{"account_id":7,"plan_code":"PRO","issued_at":1,"expires_at":9999999999,
            "render_scale":1.0,"max_fps":60,"watermark":false,"max_record_minutes":120,
            "max_streams":2,"lighting_enabled":true,"youtube_enabled":true}"#;
        base64_encode(json.as_bytes())
    }

    fn sign(key: &[u8], blob_b64: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        mac.update(blob_b64.as_bytes());
        mac.finalize().into_bytes().iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn key_halves_are_the_same_length() {
        assert_eq!(HMAC_KEY_XOR.len(), HMAC_KEY_ROT.len());
    }

    #[test]
    fn blob_signed_with_empty_key_never_verifies() {
        // Must hold both with a build key (signature mismatch) and without one
        // (the empty-key guard) — otherwise keyless builds accept forged blobs.
        let blob = valid_blob();
        assert!(verify_and_parse(&blob, &sign(&[], &blob)).is_err());
    }

    #[test]
    fn blob_signed_with_wrong_key_is_rejected() {
        let blob = valid_blob();
        assert!(verify_and_parse(&blob, &sign(b"not-the-key", &blob)).is_err());
    }

    #[test]
    fn blob_signed_with_build_key_verifies() {
        let key = hmac_key();
        if key.is_empty() {
            return; // built without REACT_LICENSE_HMAC_KEY — nothing to verify against
        }
        let blob = valid_blob();
        let ent = verify_and_parse(&blob, &sign(&key, &blob)).expect("valid signature");
        assert_eq!(ent.plan_code, "PRO");
        assert_eq!(ent.max_fps, 60);
        assert!(!ent.watermark);
    }
}
