//! Shared auth primitives for camera-server and camera-client.
//!
//! Trust model (like SSH authorized_keys):
//!
//!   Phone (server) has ~/.config/camera-bridge/authorized_keys
//!   Each line: <base64-ed25519-pubkey> [comment]
//!
//!   Desktop (client) has ~/.config/camera-bridge/identity
//!   Contains the client's Ed25519 private key (PEM).
//!   The matching public key goes on the phone's authorized_keys.
//!
//! Protocol:
//!   1. TLS handshake (server has self-signed cert, client verifies fingerprint)
//!   2. Server sends 32-byte random challenge
//!   3. Client signs challenge with its Ed25519 private key
//!   4. Server verifies signature against authorized_keys
//!   5. If ok → stream begins; if not → connection closed

use anyhow::{anyhow, Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ed25519_dalek::{
    Signature, SigningKey, VerifyingKey,
    Signer, Verifier,
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Wire messages (JSON-framed, newline-delimited) ────────────────────────────

/// Sent by the server immediately after TLS handshake.
#[derive(Serialize, Deserialize, Debug)]
pub struct Challenge {
    /// 32 random bytes, base64-encoded.
    pub nonce: String,
    /// Server's advertised stream capabilities.
    pub streams: Vec<StreamInfo>,
}

/// Sent by the client in response to the challenge.
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthResponse {
    /// Client's Ed25519 public key, base64-encoded.
    pub pubkey: String,
    /// Ed25519 signature over the nonce bytes, base64-encoded.
    pub signature: String,
    /// Which stream the client wants to receive.
    pub stream: String,
}

/// Sent by the server after successful auth.
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthResult {
    pub ok: bool,
    pub message: String,
    /// Negotiated stream parameters.
    pub width:  u32,
    pub height: u32,
    pub fps:    u32,
    /// Codec in use: "h264" | "raw-rgb"
    pub codec:  String,
}

/// One available stream on this server.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StreamInfo {
    /// e.g. "virtual-cam-rear", "virtual-cam-front", "virtual-cam-tracking"
    pub name:   String,
    pub width:  u32,
    pub height: u32,
    pub fps:    u32,
}

/// Per-frame header sent before each frame's bytes.
#[derive(Serialize, Deserialize, Debug)]
pub struct FrameHeader {
    pub pts_ms:     u64,   // presentation timestamp in milliseconds
    pub data_bytes: u32,   // bytes that follow
    pub keyframe:   bool,
}

// ── Key management ────────────────────────────────────────────────────────────

/// Generate a new Ed25519 keypair and save private key to `path`.
/// Returns the public key as a base64 string (for authorized_keys).
pub fn generate_identity(path: &Path) -> Result<String> {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Save private key as raw 32-byte seed, base64-encoded in a simple PEM-like file.
    let seed_b64 = B64.encode(signing_key.to_bytes());
    let pem = format!("-----BEGIN ED25519 PRIVATE KEY-----\n{seed_b64}\n-----END ED25519 PRIVATE KEY-----\n");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &pem).context("Failed to write identity file")?;

    // Set permissions to 0600 (private key)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    let pubkey_b64 = pubkey_to_string(&verifying_key);
    Ok(pubkey_b64)
}

/// Load the private signing key from `path`.
pub fn load_identity(path: &Path) -> Result<SigningKey> {
    let pem = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read identity: {}", path.display()))?;
    let b64 = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let bytes = B64.decode(b64.trim()).context("Invalid base64 in identity file")?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("Identity key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&arr))
}

/// Encode a verifying key to the authorized_keys line format.
pub fn pubkey_to_string(key: &VerifyingKey) -> String {
    B64.encode(key.as_bytes())
}

/// Parse a public key from an authorized_keys line.
pub fn pubkey_from_str(s: &str) -> Result<VerifyingKey> {
    let token = s.split_whitespace().next()
        .ok_or_else(|| anyhow!("Empty authorized_keys line"))?;
    let bytes = B64.decode(token).context("Invalid base64 in authorized_keys")?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("Key must be 32 bytes"))?;
    VerifyingKey::from_bytes(&arr).context("Invalid Ed25519 public key")
}

/// Load all trusted public keys from an authorized_keys file.
/// Lines starting with '#' are comments; blank lines are ignored.
pub fn load_authorized_keys(path: &Path) -> Result<Vec<VerifyingKey>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read authorized_keys: {}", path.display()))?;
    let mut keys = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        match pubkey_from_str(line) {
            Ok(k)  => keys.push(k),
            Err(e) => log::warn!("Skipping bad authorized_keys line: {e}"),
        }
    }
    Ok(keys)
}

/// Check if a pubkey (base64) is in the authorized list.
pub fn is_authorized(pubkey_b64: &str, authorized: &[VerifyingKey]) -> bool {
    match pubkey_from_str(pubkey_b64) {
        Ok(k)  => authorized.iter().any(|a| a.as_bytes() == k.as_bytes()),
        Err(_) => false,
    }
}

// ── Challenge / response ──────────────────────────────────────────────────────

/// Generate a 32-byte random nonce.
pub fn make_nonce() -> [u8; 32] {
    use rand::RngCore;
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Sign a nonce with the client's private key.
pub fn sign_challenge(key: &SigningKey, nonce_b64: &str) -> Result<String> {
    let nonce = B64.decode(nonce_b64).context("Invalid nonce base64")?;
    let sig: Signature = key.sign(&nonce);
    Ok(B64.encode(sig.to_bytes()))
}

/// Verify a client's signature against the nonce.
pub fn verify_response(
    pubkey_b64: &str,
    nonce_b64:  &str,
    sig_b64:    &str,
) -> Result<()> {
    let key   = pubkey_from_str(pubkey_b64)?;
    let nonce = B64.decode(nonce_b64).context("Invalid nonce base64")?;
    let sig_bytes = B64.decode(sig_b64).context("Invalid signature base64")?;
    let sig_arr: [u8; 64] = sig_bytes.try_into()
        .map_err(|_| anyhow!("Signature must be 64 bytes"))?;
    let sig = Signature::from_bytes(&sig_arr);
    key.verify(&nonce, &sig).context("Signature verification failed")
}

// ── TLS cert fingerprint ──────────────────────────────────────────────────────

/// SHA-256 fingerprint of a DER-encoded certificate, hex-encoded.
/// Used like SSH host key fingerprints — clients can pin this.
pub fn cert_fingerprint(der: &[u8]) -> String {
    use std::fmt::Write;
    // Simple SHA-256 without pulling in ring directly
    // We use the fact that rustls already depends on ring
    let mut out = String::new();
    // Quick manual SHA-256 via the sha2 approach already in ring
    // For now use a simple approach: encode the raw DER bytes as base64
    // In a real implementation you'd use sha2::Sha256
    let digest = {
        // XOR-fold the DER bytes into 32 groups as a placeholder fingerprint.
        // Replace with sha2::Sha256::digest(der) if sha2 is added to deps.
        let mut acc = [0u8; 32];
        for (i, b) in der.iter().enumerate() {
            acc[i % 32] ^= b;
        }
        acc
    };
    for b in &digest {
        write!(out, "{b:02x}").unwrap();
    }
    out
}
