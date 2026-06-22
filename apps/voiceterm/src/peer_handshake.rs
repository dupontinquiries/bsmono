//! v0.1.711: peer handshake foundations. Two developers establish a connection
//! by exchanging a small blob over the acoustic dial-up modem (audio_dialup),
//! with a spoken phrase carrying a voice-print for identity. This module owns:
//!
//!   - the handshake PAYLOAD (peer id, key fingerprint, workspace manifest
//!     hash, voiceprint ref) and its compact wire encoding,
//!   - the handshake STATE MACHINE (idle → dialing → listening → received →
//!     verifying voice → trusted / rejected),
//!   - voice-print VERIFICATION binding (compare the live embedding against the
//!     peer's claimed print via cosine distance).
//!
//! Transport-agnostic: the payload bytes go through audio_dialup, but could go
//! over any channel. PURE + tested. Actual mic/speaker + git live elsewhere.

use crate::audio_dialup;

/// What one peer announces in the handshake.
#[derive(Debug, Clone, PartialEq)]
pub struct HandshakePayload {
    /// human handle, e.g. "alice".
    pub peer_id: String,
    /// short hex fingerprint of the peer's key (identity, 8+ hex chars).
    pub key_fp: String,
    /// hash of the shared workspace manifest (so both sides confirm the same
    /// snapshot). hex.
    pub workspace_hash: String,
    /// id of the voice profile the peer will speak with, for the voice-print
    /// verification step. Empty = no voice binding.
    pub voiceprint_ref: String,
}

impl HandshakePayload {
    /// Compact, human-inspectable wire form: `k=v;k=v;…`. (Values can't contain
    /// ';' or '='; they're ids/hex so that's safe.)
    pub fn to_wire(&self) -> String {
        format!("v=1;id={};fp={};wh={};vp={}",
            sanitize(&self.peer_id), sanitize(&self.key_fp),
            sanitize(&self.workspace_hash), sanitize(&self.voiceprint_ref))
    }

    pub fn from_wire(s: &str) -> Option<HandshakePayload> {
        let mut id = None; let mut fp = None; let mut wh = None; let mut vp = None;
        let mut version_ok = false;
        for kv in s.split(';') {
            let mut it = kv.splitn(2, '=');
            match (it.next(), it.next()) {
                (Some("v"), Some("1")) => version_ok = true,
                (Some("id"), Some(v)) => id = Some(v.to_string()),
                (Some("fp"), Some(v)) => fp = Some(v.to_string()),
                (Some("wh"), Some(v)) => wh = Some(v.to_string()),
                (Some("vp"), Some(v)) => vp = Some(v.to_string()),
                _ => {}
            }
        }
        if !version_ok { return None; }
        Some(HandshakePayload {
            peer_id: id?, key_fp: fp?,
            workspace_hash: wh.unwrap_or_default(),
            voiceprint_ref: vp.unwrap_or_default(),
        })
    }

    /// Encode straight to acoustic symbols (wire bytes through the modem).
    pub fn to_symbols(&self) -> Vec<u8> {
        audio_dialup::encode_payload(self.to_wire().as_bytes())
    }

    /// Decode a payload from acoustic symbols. None if the frame is bad or the
    /// wire doesn't parse.
    pub fn from_symbols(symbols: &[u8]) -> Option<HandshakePayload> {
        let bytes = audio_dialup::decode_payload(symbols)?;
        let s = String::from_utf8(bytes).ok()?;
        HandshakePayload::from_wire(&s)
    }
}

fn sanitize(s: &str) -> String {
    s.chars().filter(|c| *c != ';' && *c != '=').collect()
}

// ── handshake state machine ──

#[derive(Debug, Clone, PartialEq)]
pub enum HsState {
    Idle,
    /// we're transmitting our payload (playing tones).
    Dialing,
    /// listening for the peer's tones.
    Listening,
    /// got + parsed the peer's payload; awaiting voice verification.
    Received(HandshakePayload),
    /// comparing the live voice embedding to the claimed print.
    VerifyingVoice(HandshakePayload),
    /// handshake complete, peer trusted.
    Trusted(HandshakePayload),
    /// failed (bad frame, voice mismatch, user abort) with a reason.
    Rejected(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HsEvent {
    StartDial,
    DialDone,
    StartListen,
    /// raw acoustic symbols captured from the mic.
    SymbolsCaptured(Vec<u8>),
    /// result of the voice-print compare (true = matched).
    VoiceVerified(bool),
    Abort(String),
}

/// Advance the handshake. Pure: (state, event) → new state. The driver (QML/
/// backend) feeds events from the modem + mic + voiceprint check.
pub fn step(state: HsState, event: HsEvent) -> HsState {
    use HsState::*;
    use HsEvent::*;
    match (state, event) {
        (Idle, StartDial) => Dialing,
        (Dialing, DialDone) => Listening,
        (Idle, StartListen) | (_, StartListen) => Listening,
        (Listening, SymbolsCaptured(syms)) => {
            match HandshakePayload::from_symbols(&syms) {
                Some(p) => {
                    if p.voiceprint_ref.is_empty() {
                        // no voice binding → trust on payload alone.
                        Trusted(p)
                    } else {
                        VerifyingVoice(p)
                    }
                }
                None => Rejected("could not decode peer handshake".into()),
            }
        }
        (Received(p), VoiceVerified(true)) => Trusted(p),
        (VerifyingVoice(p), VoiceVerified(true)) => Trusted(p),
        (VerifyingVoice(_), VoiceVerified(false)) =>
            Rejected("voice print did not match".into()),
        (_, Abort(why)) => Rejected(why),
        // any other event leaves state unchanged
        (s, _) => s,
    }
}

/// Has the handshake reached a terminal state?
pub fn is_terminal(state: &HsState) -> bool {
    matches!(state, HsState::Trusted(_) | HsState::Rejected(_))
}

// ── voice-print verification ──

/// Decide whether a live voice embedding matches the peer's claimed print.
/// Uses cosine distance (same metric as the speaker layer); `threshold` is the
/// max distance to accept (smaller = stricter). Returns (matched, distance).
pub fn voice_matches(live: &[f32], claimed: &[f32], threshold: f32) -> (bool, f32) {
    if live.is_empty() || claimed.is_empty() || live.len() != claimed.len() {
        return (false, 1.0);
    }
    let d = crate::speaker_embed::cosine_distance(live, claimed);
    (d <= threshold, d)
}

/// Serialize an embedding to a compact CSV wire (6-dp floats) so a voiceprint
/// can ride alongside the handshake payload or be stored in the marker.
pub fn embedding_to_wire(emb: &[f32]) -> String {
    emb.iter().map(|x| format!("{:.6}", x)).collect::<Vec<_>>().join(",")
}

/// Parse an embedding CSV wire back to a vector. Empty/garbage → empty vec.
pub fn embedding_from_wire(s: &str) -> Vec<f32> {
    if s.trim().is_empty() { return Vec::new(); }
    s.split(',').filter_map(|t| t.trim().parse::<f32>().ok()).collect()
}

/// Embed a flat MFCC buffer (n_frames × N_MFCC) into a speaker embedding,
/// deriving n_frames from the length. Thin wrapper over speaker_embed so the
/// handshake's voiceprint path has one entry point. Empty mfcc → empty vec.
pub fn embed_mfcc(mfcc: &[f32]) -> Vec<f32> {
    let n = crate::speaker_embed::N_MFCC;
    if n == 0 || mfcc.len() < n { return Vec::new(); }
    let frames = mfcc.len() / n;
    crate::speaker_embed::embed_from_mfcc(mfcc, frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HandshakePayload {
        HandshakePayload {
            peer_id: "alice".into(),
            key_fp: "ab12cd34".into(),
            workspace_hash: "deadbeef".into(),
            voiceprint_ref: "spk_001".into(),
        }
    }

    #[test]
    fn wire_roundtrip() {
        let p = sample();
        let w = p.to_wire();
        assert!(w.starts_with("v=1;"));
        assert_eq!(HandshakePayload::from_wire(&w), Some(p));
    }

    #[test]
    fn wire_rejects_no_version() {
        assert_eq!(HandshakePayload::from_wire("id=x;fp=y"), None);
    }

    #[test]
    fn acoustic_payload_roundtrip() {
        // payload → tones-symbols → payload (through the modem framing).
        let p = sample();
        let syms = p.to_symbols();
        assert_eq!(HandshakePayload::from_symbols(&syms), Some(p));
    }

    #[test]
    fn fsm_happy_path_with_voice() {
        let mut st = HsState::Idle;
        st = step(st, HsEvent::StartListen);
        assert_eq!(st, HsState::Listening);
        let syms = sample().to_symbols();
        st = step(st, HsEvent::SymbolsCaptured(syms));
        assert!(matches!(st, HsState::VerifyingVoice(_)));
        st = step(st, HsEvent::VoiceVerified(true));
        assert!(matches!(st, HsState::Trusted(_)));
        assert!(is_terminal(&st));
    }

    #[test]
    fn fsm_no_voice_binding_trusts_on_payload() {
        let mut p = sample();
        p.voiceprint_ref = String::new();
        let st = step(HsState::Listening, HsEvent::SymbolsCaptured(p.to_symbols()));
        assert!(matches!(st, HsState::Trusted(_)));
    }

    #[test]
    fn fsm_voice_mismatch_rejects() {
        let st = HsState::VerifyingVoice(sample());
        let st = step(st, HsEvent::VoiceVerified(false));
        assert!(matches!(st, HsState::Rejected(_)));
    }

    #[test]
    fn fsm_bad_frame_rejects() {
        let st = step(HsState::Listening, HsEvent::SymbolsCaptured(vec![0, 1, 2]));
        assert!(matches!(st, HsState::Rejected(_)));
    }

    #[test]
    fn fsm_dial_then_listen() {
        let mut st = HsState::Idle;
        st = step(st, HsEvent::StartDial);
        assert_eq!(st, HsState::Dialing);
        st = step(st, HsEvent::DialDone);
        assert_eq!(st, HsState::Listening);
    }

    #[test]
    fn voiceprint_match_logic() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let (m, d) = voice_matches(&a, &b, 0.2);
        assert!(m && d < 0.01);
        let c = vec![0.0, 1.0, 0.0];     // orthogonal → distance ~1
        let (m2, _) = voice_matches(&a, &c, 0.2);
        assert!(!m2);
        // length mismatch / empty → no match
        assert!(!voice_matches(&a, &[], 0.5).0);
    }

    #[test]
    fn embedding_wire_roundtrip() {
        let emb = vec![0.125, -0.5, 1.0, 0.0, 0.333333];
        let wire = embedding_to_wire(&emb);
        let back = embedding_from_wire(&wire);
        assert_eq!(back.len(), emb.len());
        for (a, b) in emb.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
        assert!(embedding_from_wire("").is_empty());
    }

    #[test]
    fn embed_mfcc_shapes() {
        // N_MFCC frames worth of data → an embedding of EMBED_DIM
        let n = crate::speaker_embed::N_MFCC;
        let mfcc = vec![0.5f32; n * 4]; // 4 frames
        let emb = embed_mfcc(&mfcc);
        assert_eq!(emb.len(), crate::speaker_embed::EMBED_DIM);
        // too-short buffer → empty
        assert!(embed_mfcc(&[0.1, 0.2]).is_empty());
    }
}
