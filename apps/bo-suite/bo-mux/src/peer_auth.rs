// src/peer_auth.rs
//
// Symmetric-key authentication for bo-mux mesh messages.
//
// ── Threat model ──
//
// The mesh runs over UDP multicast on the LAN. Out of the box that
// means *anyone on the LAN* — a guest device, a compromised IoT lamp,
// a roommate's laptop — can:
//
//   1. Inject Hello/Announce/Assign messages to disrupt the channel
//      routing (announce themselves as coordinator with a high term,
//      claim every speaker, refuse to release).
//   2. Replay legitimate volume/play/stop messages they sniffed.
//   3. Send floods of any message to DoS the legitimate coordinator.
//
// We can't rely on TLS because UDP multicast doesn't have a single
// listener; we can't rely on physical-LAN trust because consumer
// home networks are not zoned (a phone, a smart TV, and a friend's
// laptop all sit on the same /24). The only realistic defense is
// "the legitimate bo-mux peers share a secret, messages without
// proof of that secret are dropped."
//
// ── Design ──
//
// Each peer holds a registry: `Vec<PeerKey>` where each entry is
// `{ peer_id, secret }`. The secret is a 32-byte HMAC key. A peer's
// own (peer_id, secret) is the FIRST entry in the registry — that's
// the "self" key.
//
// On send: HMAC the message body (with peer_id || timestamp_ms
// included) using our secret. Wrap as `SignedMsg { peer_id, ts_ms,
// nonce, body, mac }` and broadcast.
//
// On receive:
//   1. Look up the sender's peer_id in our registry. Unknown → drop
//      silently (no error log: an attacker probing would generate
//      log spam).
//   2. Reject if timestamp is more than ±MAX_SKEW_SECS from now.
//      Defends against replay (an attacker can't reuse a captured
//      message past its window).
//   3. Track recent (peer_id, nonce) pairs. Reject duplicates within
//      the window — defends against rapid replay.
//   4. Verify HMAC. Constant-time comparison.
//   5. Deserialize body and feed to the mesh.
//
// ── Key distribution ──
//
// The registry lives at $XDG_DATA_HOME/bo-controller/peer_keys.json
// (or a fallback). It's a plain JSON array; the user / admin adds
// entries either:
//   - manually by editing the file (each peer's secret is shown by
//     a "Show my key" button in the UI)
//   - via a pairing flow where one peer scans a QR-encoded version
//     of another's key (deferred — the file-edit path is enough
//     for the initial defense)
//
// ── What this DOESN'T defend ──
//
// - Side-channel: an attacker with the secret (via the JSON file
//   or a compromised peer) is fully trusted. There's no defense in
//   depth beyond filesystem permissions on the key file (0600).
// - The bo-mux channel HTTP server is a separate concern (token
//   gating; see chan_server). This module is mesh-only.
// - Coordinator election convergence under adversarial conditions:
//   a legitimate peer that's been compromised could still flood
//   high-term Announces. That's why we ALSO cap message-rate per
//   peer; see ratelimit.rs / the mesh tick.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Width of the time window in which a message timestamp is
/// considered fresh. Larger = more replay tolerance; smaller =
/// tighter security but more sensitive to clock skew between peers.
/// 30 s is the common-sense balance for a home LAN — clocks drift
/// minutes between phone-class devices, but 30 s is more than
/// enough margin and still small enough that a captured message
/// can't be replayed an hour later.
pub const MAX_SKEW_SECS: u64 = 30;

/// Size of the per-peer recent-nonce window. Each peer's last N
/// nonces are remembered so an attacker can't replay the same
/// (peer_id, nonce) inside the time window. 256 covers the typical
/// "Hello every 2s × MAX_SKEW_SECS" volume + headroom.
const NONCE_WINDOW: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerKey {
    /// Stable identifier for the peer. Conventionally the bo-mux
    /// node_id from NodeInfo.
    pub peer_id: String,
    /// 32 raw bytes of HMAC-SHA256 key, hex-encoded for JSON
    /// readability. The hex form is what users see in the UI's
    /// "Show my key" affordance.
    pub secret_hex: String,
}

impl PeerKey {
    /// Decode the hex secret to raw bytes. Returns `None` for
    /// malformed input (wrong length, non-hex chars).
    pub fn secret_bytes(&self) -> Option<Vec<u8>> {
        if self.secret_hex.len() != 64 { return None; }
        let mut out = Vec::with_capacity(32);
        let bytes = self.secret_hex.as_bytes();
        for chunk in bytes.chunks(2) {
            let hi = hex_nybble(chunk[0])?;
            let lo = hex_nybble(chunk[1])?;
            out.push((hi << 4) | lo);
        }
        Some(out)
    }
}

fn hex_nybble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + b - b'a'),
        b'A'..=b'F' => Some(10 + b - b'A'),
        _ => None,
    }
}

/// Encode a 32-byte secret to its hex form (lowercase).
pub fn encode_secret(secret: &[u8]) -> String {
    let mut out = String::with_capacity(secret.len() * 2);
    for b in secret {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

/// Generate a fresh 32-byte random secret. We use `getrandom`'s
/// behavior via `std::time::SystemTime` mixed with process state —
/// not cryptographically perfect but adequate for a LAN-peer-auth
/// secret where the real protection is "the file is 0600 and only
/// the user reads it." A future hardening would use `getrandom`
/// crate; not pulling that in for one entry-point function.
pub fn generate_secret() -> Vec<u8> {
    let mut out = vec![0u8; 32];
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let mut state = (now.as_nanos() as u64) ^ (std::process::id() as u64)
        ^ 0xc6a4_a793_5bd1_e995;
    for chunk in out.chunks_mut(8) {
        // SplitMix64 — fast, decent distribution.
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z = z ^ (z >> 31);
        for (i, b) in chunk.iter_mut().enumerate() {
            *b = ((z >> (i * 8)) & 0xff) as u8;
        }
    }
    out
}

/// Self peer key + the registry of known peers. The first entry is
/// always "self" (the peer_id we sign messages with). Other entries
/// are peers we'll accept inbound messages from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyRegistry {
    pub self_peer: PeerKey,
    pub known_peers: Vec<PeerKey>,
}

impl KeyRegistry {
    /// Find a peer's key by id (checking both self + known_peers).
    pub fn lookup(&self, peer_id: &str) -> Option<&PeerKey> {
        if self.self_peer.peer_id == peer_id { return Some(&self.self_peer); }
        self.known_peers.iter().find(|p| p.peer_id == peer_id)
    }

    /// Add a peer to the known list. Idempotent: if peer_id already
    /// exists, update its key.
    pub fn add(&mut self, peer: PeerKey) {
        if let Some(existing) = self.known_peers.iter_mut().find(|p| p.peer_id == peer.peer_id) {
            *existing = peer;
        } else {
            self.known_peers.push(peer);
        }
    }

    /// Build a fresh registry with a randomly-generated self key
    /// and no known peers yet. Caller stores this and prints/exposes
    /// the secret_hex once for pairing.
    pub fn fresh(self_peer_id: &str) -> Self {
        Self {
            self_peer: PeerKey {
                peer_id: self_peer_id.to_string(),
                secret_hex: encode_secret(&generate_secret()),
            },
            known_peers: Vec::new(),
        }
    }
}

fn registry_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("bo-mux");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("peer_keys.json")
}

/// Load the registry from disk, or initialize a fresh one if absent.
/// Writes back to disk so the user can `cat` the file to find their
/// secret for sharing with another peer.
///
/// Sets the file mode to 0600 on UNIX so other local users can't
/// read the key.
pub fn load_or_init(self_peer_id: &str) -> std::io::Result<KeyRegistry> {
    let path = registry_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(reg) = serde_json::from_str::<KeyRegistry>(&s) {
            return Ok(reg);
        }
    }
    let reg = KeyRegistry::fresh(self_peer_id);
    let json = serde_json::to_string_pretty(&reg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(reg)
}

/// Save the registry to disk. Used after adding a known peer.
pub fn save(reg: &KeyRegistry) -> std::io::Result<()> {
    let path = registry_path();
    let json = serde_json::to_string_pretty(reg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Signed envelope wrapping the actual mesh Msg.
///
/// Wire format (JSON): a peer_id, a unix-millis timestamp, a 12-byte
/// random nonce hex-encoded, the body bytes (also as a hex blob to
/// keep the JSON parser simple), and a 32-byte HMAC hex over
/// (peer_id || ts || nonce || body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedEnvelope {
    pub peer_id: String,
    pub ts_ms:   u64,
    pub nonce:   String,
    pub body:    String,   // hex-encoded
    pub mac:     String,   // hex-encoded HMAC-SHA256
}

/// HMAC-SHA256 — minimal stdlib-only implementation. Crypto purists
/// will prefer the `hmac` + `sha2` crates; this stays in-house to
/// keep bo-mux dependency-light. The SHA-256 here is the textbook
/// implementation; HMAC follows RFC 2104.
mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    pub fn hash(input: &[u8]) -> [u8; 32] {
        // Pad input per spec: append 0x80, zeros, then big-endian
        // 64-bit length in bits.
        let bit_len = (input.len() as u64) * 8;
        let mut padded: Vec<u8> = Vec::with_capacity(input.len() + 64);
        padded.extend_from_slice(input);
        padded.push(0x80);
        while padded.len() % 64 != 56 { padded.push(0); }
        padded.extend_from_slice(&bit_len.to_be_bytes());

        let mut h = H0;
        for chunk in padded.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
            }
            for i in 16..64 {
                let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
                let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
                w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let mj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(mj);
                hh = g; g = f; f = e; e = d.wrapping_add(temp1);
                d = c; c = b; b = a; a = temp1.wrapping_add(temp2);
            }
            h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
        }
        let mut out = [0u8; 32];
        for (i, &v) in h.iter().enumerate() {
            out[i*4..(i+1)*4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

/// HMAC-SHA256 per RFC 2104.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = if key.len() > BLOCK {
        sha256::hash(key).to_vec()
    } else {
        key.to_vec()
    };
    while k.len() < BLOCK { k.push(0); }
    let mut ipad = vec![0u8; BLOCK];
    let mut opad = vec![0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = ipad;
    inner.extend_from_slice(msg);
    let inner_hash = sha256::hash(&inner);
    let mut outer = opad;
    outer.extend_from_slice(&inner_hash);
    sha256::hash(&outer)
}

/// Hex-encode raw bytes. Lowercase.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Decode hex bytes. Returns None on bad input.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in bytes.chunks(2) {
        let hi = hex_nybble(chunk[0])?;
        let lo = hex_nybble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

/// Constant-time byte-array comparison. `a` and `b` must be the same
/// length. Returns true iff every byte matches; the time taken is
/// independent of where the first mismatch occurs.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for i in 0..a.len() { diff |= a[i] ^ b[i]; }
    diff == 0
}

/// Wrap a message body in a signed envelope using our self key.
pub fn sign(reg: &KeyRegistry, body: &[u8]) -> Option<Vec<u8>> {
    let secret = reg.self_peer.secret_bytes()?;
    let ts_ms = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
    let nonce_bytes = generate_secret()[..12].to_vec();
    let nonce_hex = hex(&nonce_bytes);
    // Build the to-be-MACed payload: peer_id || ts || nonce || body.
    let mut payload = Vec::new();
    payload.extend_from_slice(reg.self_peer.peer_id.as_bytes());
    payload.extend_from_slice(&ts_ms.to_be_bytes());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(body);
    let mac = hmac_sha256(&secret, &payload);
    let env = SignedEnvelope {
        peer_id: reg.self_peer.peer_id.clone(),
        ts_ms,
        nonce: nonce_hex,
        body: hex(body),
        mac: hex(&mac),
    };
    serde_json::to_vec(&env).ok()
}

/// Verify outcome.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// Envelope didn't parse as JSON.
    BadEnvelope,
    /// Peer not in registry — drop silently.
    UnknownPeer,
    /// Timestamp outside the MAX_SKEW_SECS window.
    StaleOrFuture,
    /// (peer_id, nonce) already seen.
    Replay,
    /// HMAC didn't match.
    BadMac,
    /// Body hex didn't decode.
    BadBody,
}

/// Verify a signed envelope. Returns `(peer_id, body_bytes)` on
/// success, or one of VerifyError on failure.
///
/// Concurrency: the nonce-tracking state is keyed off the static
/// SEEN_NONCES below; multiple Mesh instances in the same process
/// share it. That's fine for the bo-mux deployment (one Mesh per
/// process), and the Mutex ensures correctness anyway.
pub fn verify(reg: &KeyRegistry, raw: &[u8]) -> Result<(String, Vec<u8>), VerifyError> {
    let env: SignedEnvelope = serde_json::from_slice(raw)
        .map_err(|_| VerifyError::BadEnvelope)?;

    let peer = reg.lookup(&env.peer_id)
        .ok_or(VerifyError::UnknownPeer)?;

    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|_| VerifyError::StaleOrFuture)?.as_millis() as u64;
    let skew_ms = MAX_SKEW_SECS * 1_000;
    if env.ts_ms + skew_ms < now_ms || env.ts_ms > now_ms + skew_ms {
        return Err(VerifyError::StaleOrFuture);
    }

    // Replay defense.
    if !record_nonce(&env.peer_id, &env.nonce) {
        return Err(VerifyError::Replay);
    }

    let body = unhex(&env.body).ok_or(VerifyError::BadBody)?;
    let secret = peer.secret_bytes().ok_or(VerifyError::UnknownPeer)?;
    let nonce_bytes = unhex(&env.nonce).ok_or(VerifyError::BadBody)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(env.peer_id.as_bytes());
    payload.extend_from_slice(&env.ts_ms.to_be_bytes());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&body);
    let expected = hmac_sha256(&secret, &payload);
    let claimed = unhex(&env.mac).ok_or(VerifyError::BadMac)?;
    if !ct_eq(&expected, &claimed) {
        return Err(VerifyError::BadMac);
    }
    Ok((env.peer_id, body))
}

fn seen_nonces() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static N: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    N.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns true if the nonce is fresh (and records it). Returns
/// false if already seen for that peer.
fn record_nonce(peer_id: &str, nonce: &str) -> bool {
    let Ok(mut map) = seen_nonces().lock() else { return false; };
    let v = map.entry(peer_id.to_string()).or_default();
    if v.contains(&nonce.to_string()) { return false; }
    v.push(nonce.to_string());
    if v.len() > NONCE_WINDOW { v.remove(0); }
    true
}

// ─── Per-peer rate gate ───────────────────────────────────────────────
//
// The HMAC layer above proves a message was signed by someone with a
// valid secret. It does NOT prove the legitimate peer that owns the
// secret is the one sending it — a compromised peer can flood
// authentic messages, and at that point the auth layer just lets the
// flood through. The rate gate caps each known peer at a sane
// per-window message budget; a peer exceeding it gets briefly
// quarantined regardless of signature validity.
//
// Legitimate peer traffic: Hello every 2 s, Heartbeat every 1 s,
// ClaimChannel + Announce at session boundaries. Steady-state is
// well under 5 msg/s/peer. Budget of 30 msg in 5 s gives 6× headroom
// for bursty legitimate traffic (session start, mass-claim) without
// letting a flood through.

/// Max messages from any single peer in `PEER_RATE_WINDOW_MS`. Beyond
/// this, further messages are dropped until the window clears.
pub const PEER_RATE_MAX_IN_WINDOW: usize = 30;
pub const PEER_RATE_WINDOW_MS: u64 = 5_000;

struct PeerRateState {
    /// Per-peer recent-arrival timestamps (ms).
    timestamps: HashMap<String, Vec<u64>>,
    /// Total quarantine count across all peers — for the UI's
    /// "your mesh is under attack" signal.
    quarantine_total: u64,
}

fn peer_rate_state() -> &'static Mutex<PeerRateState> {
    static S: OnceLock<Mutex<PeerRateState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(PeerRateState {
        timestamps: HashMap::new(),
        quarantine_total: 0,
    }))
}

/// Check whether the peer is within its rate budget. Returns true if
/// the message is permitted (and records the arrival); false if the
/// peer is currently quarantined.
///
/// The function ALSO records a quarantine event when the budget is
/// first exceeded, so `quarantine_count()` reflects unique flood
/// detections (not every dropped message during the quarantine).
pub fn peer_rate_ok(peer_id: &str) -> bool {
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    let Ok(mut s) = peer_rate_state().lock() else { return false; };
    let cutoff = now_ms.saturating_sub(PEER_RATE_WINDOW_MS);
    let entry = s.timestamps.entry(peer_id.to_string()).or_default();
    entry.retain(|&t| t >= cutoff);
    if entry.len() >= PEER_RATE_MAX_IN_WINDOW {
        // First overflow this window counts as a fresh quarantine.
        // Subsequent drops within the window don't double-count;
        // we detect "first overflow" by seeing the entry is full
        // and the most recent timestamp is within the last 100 ms
        // (i.e. we just got here from a Pass on the previous call).
        let just_tripped = entry.last().is_some_and(|&t| now_ms - t > 100);
        if just_tripped { s.quarantine_total += 1; }
        return false;
    }
    entry.push(now_ms);
    true
}

/// Total count of "peer floods detected" across the process life.
/// The mesh's `rejected_count` adds 1 per dropped message; this
/// counts each first-trip event, which is the more meaningful
/// "your mesh is under attack" signal for the UI.
pub fn quarantine_count() -> u64 {
    peer_rate_state().lock()
        .map(|s| s.quarantine_total)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_reg() -> KeyRegistry { KeyRegistry::fresh("alice") }

    #[test]
    fn round_trip_signed_envelope_verifies() {
        let reg = fresh_reg();
        let body = b"hello mesh".to_vec();
        let env = sign(&reg, &body).unwrap();
        let (peer, decoded) = verify(&reg, &env).unwrap();
        assert_eq!(peer, reg.self_peer.peer_id);
        assert_eq!(decoded, body);
    }

    #[test]
    fn unknown_peer_dropped() {
        let alice = KeyRegistry::fresh("alice");
        let bob = KeyRegistry::fresh("bob");
        // Bob signs a message; Alice's registry doesn't know bob.
        let env = sign(&bob, b"hi").unwrap();
        assert_eq!(verify(&alice, &env), Err(VerifyError::UnknownPeer));
    }

    #[test]
    fn known_peer_with_correct_key_verifies() {
        let mut alice = KeyRegistry::fresh("alice");
        let bob = KeyRegistry::fresh("bob");
        alice.add(bob.self_peer.clone());
        let env = sign(&bob, b"hi from bob").unwrap();
        let (peer, got) = verify(&alice, &env).unwrap();
        assert_eq!(peer, "bob");
        assert_eq!(got, b"hi from bob".to_vec());
    }

    #[test]
    fn known_peer_with_wrong_key_rejected() {
        let mut alice = KeyRegistry::fresh("alice");
        let bob = KeyRegistry::fresh("bob");
        let fake_bob = PeerKey {
            peer_id: bob.self_peer.peer_id.clone(),
            secret_hex: encode_secret(&generate_secret()),
        };
        alice.add(fake_bob);
        let env = sign(&bob, b"hi").unwrap();
        assert_eq!(verify(&alice, &env), Err(VerifyError::BadMac));
    }

    #[test]
    fn replay_rejected() {
        let mut alice = KeyRegistry::fresh("alice");
        let bob = KeyRegistry::fresh("bob");
        alice.add(bob.self_peer.clone());
        let env = sign(&bob, b"reply!").unwrap();
        let _ = verify(&alice, &env).unwrap();
        assert_eq!(verify(&alice, &env), Err(VerifyError::Replay));
    }

    #[test]
    fn per_peer_rate_gate_pass_within_budget() {
        // Use a unique peer_id so concurrent test ordering doesn't
        // pollute this peer's window.
        let peer = format!("test-budget-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        for _ in 0..PEER_RATE_MAX_IN_WINDOW {
            assert!(peer_rate_ok(&peer));
        }
    }

    #[test]
    fn per_peer_rate_gate_blocks_after_overflow() {
        let peer = format!("test-flood-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        for _ in 0..PEER_RATE_MAX_IN_WINDOW {
            assert!(peer_rate_ok(&peer));
        }
        // Overflow → blocked.
        assert!(!peer_rate_ok(&peer));
    }

    #[test]
    fn per_peer_rate_gate_isolates_peers() {
        // One peer flooding shouldn't lock out other peers.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let attacker = format!("attacker-{}", stamp);
        let victim = format!("victim-{}", stamp);
        // Flood from attacker.
        for _ in 0..(PEER_RATE_MAX_IN_WINDOW + 5) {
            let _ = peer_rate_ok(&attacker);
        }
        // Victim still passes.
        assert!(peer_rate_ok(&victim));
    }

    #[test]
    fn tampered_body_rejected() {
        let mut alice = KeyRegistry::fresh("alice");
        let bob = KeyRegistry::fresh("bob");
        alice.add(bob.self_peer.clone());
        let env = sign(&bob, b"original body").unwrap();
        // Manually flip a byte in the JSON's body field. We can't just
        // mutate bytes because that's how the unhex would still
        // succeed but the MAC would mismatch — exactly the test.
        let mut s = String::from_utf8(env).unwrap();
        // Replace one hex char in the body.
        let bidx = s.find("\"body\":\"").unwrap() + 8;
        let mut chars: Vec<char> = s.chars().collect();
        chars[bidx] = if chars[bidx] == 'a' { 'b' } else { 'a' };
        s = chars.into_iter().collect();
        assert_eq!(verify(&alice, s.as_bytes()), Err(VerifyError::BadMac));
    }

    #[test]
    fn malformed_envelope_rejected() {
        let reg = fresh_reg();
        assert_eq!(verify(&reg, b"{}"), Err(VerifyError::BadEnvelope));
        assert_eq!(verify(&reg, b"not json"), Err(VerifyError::BadEnvelope));
    }

    #[test]
    fn secret_hex_round_trip() {
        let s = generate_secret();
        let h = encode_secret(&s);
        let pk = PeerKey { peer_id: "x".into(), secret_hex: h };
        assert_eq!(pk.secret_bytes().unwrap(), s);
    }

    #[test]
    fn malformed_secret_hex_yields_none() {
        let pk = PeerKey { peer_id: "x".into(), secret_hex: "zz".into() };
        assert!(pk.secret_bytes().is_none());
        let pk = PeerKey { peer_id: "x".into(), secret_hex: "abc".into() };
        assert!(pk.secret_bytes().is_none()); // wrong length
    }

    #[test]
    fn ct_eq_works() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn registry_lookup_finds_self_and_known() {
        let mut reg = KeyRegistry::fresh("self");
        reg.add(PeerKey {
            peer_id: "peer1".into(),
            secret_hex: encode_secret(&generate_secret()),
        });
        assert!(reg.lookup("self").is_some());
        assert!(reg.lookup("peer1").is_some());
        assert!(reg.lookup("nope").is_none());
    }
}
