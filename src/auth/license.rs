//! License blob verification and offline caching
//!
//! The server returns a license blob containing plan limits as concrete
//! values (render_scale, max_fps, watermark, etc.), signed with Ed25519. The
//! client verifies the signature against the public key(s) below, then uses
//! the values directly in computations.
//!
//! Only the server holds the private key. Nothing in this file, or anywhere
//! in a built app, is secret, and nothing in it can be used to produce a blob
//! this code accepts.
//!
//! Offline: the verified blob and its signature are cached in SQLite and
//! re-verified on every load. See `load_cached_entitlements`.

use anyhow::{anyhow, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::time::Duration;

use super::Entitlements;
use crate::config;

/// 7 days in seconds
const OFFLINE_GRACE_SECS: i64 = 7 * 24 * 3600;

/// Ed25519 public keys trusted to sign license blobs (32 bytes each, hex).
///
/// These are public keys, not secrets. A blob is accepted if any one of them
/// verifies it, which is what makes rotating the signing key possible: ship a
/// build that trusts the old and the new key, then switch the server over.
/// With no entries, every blob is rejected.
const LICENSE_PUBLIC_KEYS: &[&str] = &[
    // compeller.ai license signing key, 2026-11
    "f92c554b1645b80270a61e2dcfe448f5614f5b1717d6dae04a6626c8a8a94495",
];

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
///
/// Servers still supporting pre-Ed25519 builds also send a `signature` field
/// (an HMAC). It is deliberately not read here: an HMAC key has to ship inside
/// every build that checks it, so it cannot prove who signed a blob.
#[derive(Debug, Deserialize)]
struct LicenseBlobResponse {
    license: String, // base64-encoded JSON
    /// Hex Ed25519 signature over the `license` string.
    #[serde(default)]
    signature_ed25519: Option<String>,
}

/// Fetch license blob from server, verify its signature, cache to SQLite, return Entitlements.
pub fn fetch_license_blob(access_token: &str, db: &crate::db::Database) -> Result<Entitlements> {
    let url = format!("{}/licensing/blob", config::api_base());

    let response = ureq::get(&url)
        // Tells the server this build verifies Ed25519 (it logs who still doesn't).
        .query("sig", "ed25519")
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

    let signature = blob_response
        .signature_ed25519
        .ok_or_else(|| anyhow!("License response carries no Ed25519 signature"))?;

    let entitlements = verify_and_parse(&blob_response.license, &signature)?;

    // Cache to SQLite for offline use
    if let Err(e) = db.save_license_blob(&blob_response.license, &signature) {
        log::error!("[License] Failed to cache blob: {}", e);
    }

    Ok(entitlements)
}

/// Verify the Ed25519 signature and parse the blob into Entitlements.
fn verify_and_parse(blob_b64: &str, signature_hex: &str) -> Result<Entitlements> {
    verify_and_parse_with(blob_b64, signature_hex, LICENSE_PUBLIC_KEYS)
}

/// `verify_and_parse` against an explicit set of trusted keys.
fn verify_and_parse_with(
    blob_b64: &str,
    signature_hex: &str,
    trusted_keys: &[&str],
) -> Result<Entitlements> {
    verify_signature(blob_b64, signature_hex, trusted_keys)?;

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

/// Check that `signature_hex` is an Ed25519 signature over the bytes of
/// `blob_b64` made by one of `trusted_keys`.
///
/// The signature covers the base64 text exactly as received, so nothing is
/// decoded or parsed before it has been authenticated.
fn verify_signature(blob_b64: &str, signature_hex: &str, trusted_keys: &[&str]) -> Result<()> {
    // Anything that is not 64 bytes of hex fails here — including the 32-byte
    // HMAC that older builds cached.
    let signature = Signature::from_slice(&hex_decode(signature_hex)?)
        .map_err(|_| anyhow!("License signature is not an Ed25519 signature"))?;

    for key_hex in trusted_keys {
        let key = match public_key(key_hex) {
            Ok(key) => key,
            Err(e) => {
                log::error!("[License] Skipping unusable trusted key: {}", e);
                continue;
            }
        };
        // verify_strict also rejects weak keys and non-canonical signatures.
        if key.verify_strict(blob_b64.as_bytes(), &signature).is_ok() {
            return Ok(());
        }
    }

    Err(anyhow!("License signature verification failed"))
}

fn public_key(key_hex: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = hex_decode(key_hex)?
        .try_into()
        .map_err(|_| anyhow!("Ed25519 public key must be 32 bytes"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| anyhow!("Invalid Ed25519 public key: {}", e))
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
    fn nibble(byte: u8) -> Result<u8> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(anyhow!("Invalid hex")),
        }
    }

    let bytes = input.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err(anyhow!("Odd-length hex string"));
    }
    bytes
        .chunks_exact(2)
        .map(|pair| Ok((nibble(pair[0])? << 4) | nibble(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

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

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    fn blob(plan_code: &str, expires_at: i64) -> String {
        let json = format!(
            r#"{{"account_id":7,"plan_code":"{plan_code}","issued_at":1,"expires_at":{expires_at},
            "render_scale":1.0,"max_fps":60,"watermark":false,"max_record_minutes":120,
            "max_streams":2,"lighting_enabled":true,"youtube_enabled":true}}"#
        );
        base64_encode(json.as_bytes())
    }

    /// A well-formed, unexpired license blob (base64 JSON).
    fn valid_blob() -> String {
        blob("PRO", 9_999_999_999)
    }

    /// A signing key that exists only for this test run. No private key is
    /// ever stored in the repository.
    fn throwaway_key() -> SigningKey {
        SigningKey::from_bytes(&rand::random::<[u8; 32]>())
    }

    fn public_hex(key: &SigningKey) -> String {
        hex(key.verifying_key().as_bytes())
    }

    fn sign(key: &SigningKey, blob_b64: &str) -> String {
        hex(&key.sign(blob_b64.as_bytes()).to_bytes())
    }

    #[test]
    fn committed_public_keys_are_usable() {
        // An official build with no working key would silently reject every
        // paid plan, so a typo in the list must fail here instead.
        assert!(!LICENSE_PUBLIC_KEYS.is_empty());
        for key_hex in LICENSE_PUBLIC_KEYS {
            public_key(key_hex).expect("LICENSE_PUBLIC_KEYS entry must be a valid Ed25519 public key");
        }
    }

    #[test]
    fn blob_signed_with_a_trusted_key_verifies() {
        let key = throwaway_key();
        let blob = valid_blob();
        let ent = verify_and_parse_with(&blob, &sign(&key, &blob), &[&public_hex(&key)])
            .expect("valid signature");
        assert_eq!(ent.account_id, 7);
        assert_eq!(ent.plan_code, "PRO");
        assert_eq!(ent.max_fps, 60);
        assert!(!ent.watermark);
    }

    #[test]
    fn any_one_trusted_key_is_enough() {
        // Key rotation: a build trusts the outgoing and the incoming key.
        let (old, new) = (throwaway_key(), throwaway_key());
        let trusted = [public_hex(&old), public_hex(&new)];
        let trusted: Vec<&str> = trusted.iter().map(String::as_str).collect();
        let blob = valid_blob();
        assert!(verify_and_parse_with(&blob, &sign(&old, &blob), &trusted).is_ok());
        assert!(verify_and_parse_with(&blob, &sign(&new, &blob), &trusted).is_ok());
    }

    #[test]
    fn blob_signed_with_wrong_key_is_rejected() {
        let blob = valid_blob();
        let forged = sign(&throwaway_key(), &blob);
        // Against the keys this build ships with...
        assert!(verify_and_parse(&blob, &forged).is_err());
        // ...and against some other trusted key.
        assert!(verify_and_parse_with(&blob, &forged, &[&public_hex(&throwaway_key())]).is_err());
    }

    #[test]
    fn build_with_no_trusted_keys_rejects_everything() {
        let key = throwaway_key();
        let blob = valid_blob();
        assert!(verify_and_parse_with(&blob, &sign(&key, &blob), &[]).is_err());
    }

    #[test]
    fn unusable_trusted_keys_never_verify_anything() {
        let key = throwaway_key();
        let blob = valid_blob();
        let signature = sign(&key, &blob);
        for bad in ["", "not hex", "abcd", &"00".repeat(32), &"00".repeat(33)] {
            assert!(verify_and_parse_with(&blob, &signature, &[bad]).is_err(), "trusted key {:?}", bad);
        }
    }

    #[test]
    fn tampered_blob_is_rejected() {
        let key = throwaway_key();
        let trusted = public_hex(&key);
        let signature = sign(&key, &blob("FREE", 9_999_999_999));

        // The signature is for a FREE blob; presenting it with a PRO blob must fail.
        assert!(verify_and_parse_with(&valid_blob(), &signature, &[&trusted]).is_err());

        // So must any single-character change to a correctly signed blob.
        let blob = valid_blob();
        let signature = sign(&key, &blob);
        let mut chars: Vec<char> = blob.chars().collect();
        chars[10] = if chars[10] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert!(verify_and_parse_with(&tampered, &signature, &[&trusted]).is_err());
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let key = throwaway_key();
        let blob = valid_blob();
        let mut signature = sign(&key, &blob).into_bytes();
        signature[0] = if signature[0] == b'0' { b'1' } else { b'0' };
        let signature = String::from_utf8(signature).unwrap();
        assert!(verify_and_parse_with(&blob, &signature, &[&public_hex(&key)]).is_err());
    }

    #[test]
    fn legacy_hmac_signature_is_rejected() {
        // What a pre-Ed25519 build left in the cache: 32 bytes of hex.
        let blob = valid_blob();
        assert!(verify_and_parse(&blob, &"ab".repeat(32)).is_err());
    }

    #[test]
    fn malformed_signatures_are_rejected_without_panicking() {
        let blob = valid_blob();
        for bad in ["", "zz", "abc", &"0".repeat(128), &"ab".repeat(65), &"é".repeat(64), "ab\u{00e9}"] {
            assert!(verify_and_parse(&blob, bad).is_err(), "signature {:?}", bad);
        }
    }

    #[test]
    fn expired_blob_is_rejected_even_when_correctly_signed() {
        let key = throwaway_key();
        let blob = blob("PRO", 1);
        let err = verify_and_parse_with(&blob, &sign(&key, &blob), &[&public_hex(&key)]).unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    // An envelope produced by the server's own signing code (compeller.ai's
    // LicenseSigner on the JVM) with a key pair that was generated for this
    // vector and then discarded. It pins the wire format across the two
    // codebases: base64 payload, hex signature over the base64 text.
    const SERVER_VECTOR_PUBLIC_KEY: &str = 
        "28435fbdc02580f6b3080b6e6d0c88492c99ed76bc7ba86fe38847d7f306ed64";
    const SERVER_VECTOR_LICENSE: &str = "eyJhY2NvdW50X2lkIjo3LCJwbGFuX2NvZGUiOiJQUk8iLCJpc3N1ZWRfYXQiOjE3OTQzNDU3NDEsImV4cGlyZXNfYXQiOjk5OTk5OTk5OTksInJlbmRlcl9zY2FsZSI6MS4wLCJtYXhfZnBzIjo2MCwid2F0ZXJtYXJrIjpmYWxzZSwibWF4X3JlY29yZF9taW51dGVzIjowLCJtYXhfc3RyZWFtcyI6MywibGlnaHRpbmdfZW5hYmxlZCI6dHJ1ZSwieW91dHViZV9lbmFibGVkIjp0cnVlfQ==";
    const SERVER_VECTOR_SIGNATURE: &str = "8406bbbf09e677bffe812da3815e650a5d44c7d659a40601a06fe94b6889af1b06617f2461a758050965c12d02da46705780085427b90bf76cfc39948e224102";

    #[test]
    fn envelope_from_the_server_implementation_verifies() {
        let ent = verify_and_parse_with(
            SERVER_VECTOR_LICENSE,
            SERVER_VECTOR_SIGNATURE,
            &[SERVER_VECTOR_PUBLIC_KEY],
        )
        .expect("server-produced envelope");
        assert_eq!(ent.account_id, 7);
        assert_eq!(ent.plan_code, "PRO");
        assert_eq!(ent.max_streams, 3);
        assert!(ent.lighting_enabled);

        // Its key is not one this build trusts.
        assert!(verify_and_parse(SERVER_VECTOR_LICENSE, SERVER_VECTOR_SIGNATURE).is_err());
    }

    /// Check a real `/api/licensing/blob` response against the keys this build
    /// ships with, e.g. after a server deploy and before releasing a build:
    ///
    ///   REACT_LICENSE_ENVELOPE=/path/to/response.json \
    ///     cargo test --features camera -- --ignored saved_server_response_verifies
    #[test]
    #[ignore = "needs REACT_LICENSE_ENVELOPE pointing at a saved /api/licensing/blob response"]
    fn saved_server_response_verifies() {
        let path = std::env::var("REACT_LICENSE_ENVELOPE").expect("REACT_LICENSE_ENVELOPE not set");
        let body = std::fs::read(&path).expect("read envelope file");
        let response: LicenseBlobResponse = serde_json::from_slice(&body).expect("envelope JSON");
        let signature = response.signature_ed25519.expect("response has no signature_ed25519");

        let ent = verify_and_parse(&response.license, &signature)
            .expect("signature must verify against LICENSE_PUBLIC_KEYS");
        println!("verified: plan={} account_id={}", ent.plan_code, ent.account_id);

        // And the same response must stop verifying once the blob is altered.
        let mut chars: Vec<char> = response.license.chars().collect();
        chars[10] = if chars[10] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert!(verify_and_parse(&tampered, &signature).is_err());
    }
}
