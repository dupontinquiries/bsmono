//! relay — the offload wire protocol. An origin device frames an [`OffloadRequest`] (its id, the
//! utterance signature, and the raw audio), which the transport **seals** (crypto-kit) and sends to
//! a trusted Heavy partner chosen by [`crate::mesh::stt_host_ranked`]. The partner opens it,
//! transcribes, and returns an [`OffloadResponse`]; the origin folds the transcript into its DB via
//! [`crate::reconcile`].
//!
//! This module is the **pure framing + host handler** — no crypto, no sockets. Sealing is a one-
//! liner the transport applies (`crypto_kit::seal(key, &req.encode())`); the tests here prove that
//! composition round-trips. Keeping it pure means the protocol is testable without a network and
//! voice-kit stays dependency-light.

/// Wire version byte — lets the format evolve without silently misparsing.
const WIRE_V: u8 = 1;

/// A request to transcribe audio on a remote partner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffloadRequest {
    /// Who is asking (trusted-origin check + telemetry).
    pub origin: String,
    /// Stable utterance signature, echoed in the reply so the origin can reconcile it.
    pub utterance_key: String,
    /// Raw audio frames (PCM/opus). Sealed on the wire by the transport.
    pub audio: Vec<u8>,
}

/// A partner's transcription reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffloadResponse {
    /// Echoes the request's key so out-of-order replies match their utterance.
    pub utterance_key: String,
    pub transcript: String,
    /// Which partner produced it.
    pub host: String,
}

fn put_bytes(buf: &mut Vec<u8>, b: &[u8]) {
    buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
    buf.extend_from_slice(b);
}
fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_bytes(buf, s.as_bytes());
}
fn take_bytes<'a>(buf: &mut &'a [u8]) -> Option<&'a [u8]> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_le_bytes(buf[..4].try_into().ok()?) as usize;
    let rest = &buf[4..];
    if rest.len() < len {
        return None;
    }
    let (field, tail) = rest.split_at(len);
    *buf = tail;
    Some(field)
}
fn take_str(buf: &mut &[u8]) -> Option<String> {
    Some(String::from_utf8_lossy(take_bytes(buf)?).into_owned())
}

impl OffloadRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![WIRE_V];
        put_str(&mut b, &self.origin);
        put_str(&mut b, &self.utterance_key);
        put_bytes(&mut b, &self.audio);
        b
    }
    pub fn decode(bytes: &[u8]) -> Option<OffloadRequest> {
        let mut b = bytes;
        let (&v, rest) = b.split_first()?;
        if v != WIRE_V {
            return None;
        }
        b = rest;
        Some(OffloadRequest {
            origin: take_str(&mut b)?,
            utterance_key: take_str(&mut b)?,
            audio: take_bytes(&mut b)?.to_vec(),
        })
    }
}

impl OffloadResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![WIRE_V];
        put_str(&mut b, &self.utterance_key);
        put_str(&mut b, &self.transcript);
        put_str(&mut b, &self.host);
        b
    }
    pub fn decode(bytes: &[u8]) -> Option<OffloadResponse> {
        let mut b = bytes;
        let (&v, rest) = b.split_first()?;
        if v != WIRE_V {
            return None;
        }
        b = rest;
        Some(OffloadResponse {
            utterance_key: take_str(&mut b)?,
            transcript: take_str(&mut b)?,
            host: take_str(&mut b)?,
        })
    }
}

/// Host side: transcribe a request with an injected transcriber (the real one runs whisper), and
/// build the reply. Pure — the transcriber is the only side-effect, and it's the caller's.
pub fn handle<F: Fn(&[u8]) -> String>(req: &OffloadRequest, host_id: &str, transcribe: F) -> OffloadResponse {
    OffloadResponse {
        utterance_key: req.utterance_key.clone(),
        transcript: transcribe(&req.audio),
        host: host_id.to_string(),
    }
}

/// Where the *reference* transcription for an utterance should run, deciding the origin's next move
/// after it has already served the local fastpath result. Ties [`crate::reconcile`] (is a reference
/// even wanted?) to [`crate::mesh`] (is a partner available?).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferencePlan<'a> {
    /// The local mapping is authoritative — trust it, spend nothing.
    Skip,
    /// Offload to this trusted remote partner (build + seal + send an `OffloadRequest`).
    Offload(&'a crate::mesh::VoiceDevice),
    /// The origin is itself Heavy and no remote partner is preferred — run the reference locally.
    Local,
    /// No reference path right now — the fastpath result stands until a partner appears.
    Unavailable,
}

/// Decide the reference path for an utterance the origin just served locally. Never blocks the
/// command (that already ran); this only plans the background learning step.
pub fn reference_plan<'a>(
    origin: &str,
    entry: &crate::reconcile::Entry,
    devices: &'a [crate::mesh::VoiceDevice],
    partners: &[String],
) -> ReferencePlan<'a> {
    if !entry.needs_reference() {
        return ReferencePlan::Skip;
    }
    match crate::mesh::stt_host_ranked(origin, devices, partners) {
        Some(host) if host.id != origin => ReferencePlan::Offload(host),
        Some(_) => ReferencePlan::Local, // stt_host_ranked returned the origin → it's Heavy
        None => ReferencePlan::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let r = OffloadRequest {
            origin: "phone".into(),
            utterance_key: "sig-42".into(),
            audio: vec![0, 1, 2, 250, 3],
        };
        assert_eq!(OffloadRequest::decode(&r.encode()), Some(r));
    }

    #[test]
    fn response_round_trips() {
        let r = OffloadResponse {
            utterance_key: "sig-42".into(),
            transcript: "open the garage".into(),
            host: "desktop".into(),
        };
        assert_eq!(OffloadResponse::decode(&r.encode()), Some(r));
    }

    #[test]
    fn decode_rejects_garbage_and_truncation() {
        assert!(OffloadRequest::decode(&[]).is_none());
        assert!(OffloadRequest::decode(&[9, 9, 9]).is_none()); // wrong version
        let good = OffloadRequest { origin: "a".into(), utterance_key: "b".into(), audio: vec![1, 2, 3] }.encode();
        assert!(OffloadRequest::decode(&good[..good.len() - 2]).is_none()); // truncated
    }

    #[test]
    fn handle_echoes_key_and_transcribes() {
        let req = OffloadRequest { origin: "phone".into(), utterance_key: "k".into(), audio: vec![1, 2] };
        let resp = handle(&req, "desktop", |audio| format!("{} bytes heard", audio.len()));
        assert_eq!(resp.utterance_key, "k");
        assert_eq!(resp.host, "desktop");
        assert_eq!(resp.transcript, "2 bytes heard");
    }

    // Prove the sealed round-trip composes (crypto-kit is a dev-dependency only).
    #[test]
    fn sealed_request_round_trips_over_the_wire() {
        let key = [7u8; crypto_kit::KEY_LEN];
        let req = OffloadRequest { origin: "phone".into(), utterance_key: "k".into(), audio: vec![9, 8, 7] };
        let sealed = crypto_kit::seal(&key, &req.encode());
        // wrong key can't open
        let wrong = [8u8; crypto_kit::KEY_LEN];
        assert!(crypto_kit::open(&wrong, &sealed).is_none());
        // right key opens + decodes
        let opened = crypto_kit::open(&key, &sealed).expect("open");
        assert_eq!(OffloadRequest::decode(&opened), Some(req));
    }

    use crate::mesh::{SttClass, VoiceDevice};
    use crate::reconcile::Entry;
    fn dev(id: &str, stt: SttClass) -> VoiceDevice {
        VoiceDevice { id: id.into(), trusted: true, reachable: true, has_mic: true, stt }
    }

    #[test]
    fn reference_plan_offloads_unvalidated_to_ranked_partner() {
        let mesh = vec![dev("phone", SttClass::Fast), dev("desktop", SttClass::Heavy)];
        let entry = Entry::new("k", "hello"); // unvalidated
        let plan = reference_plan("phone", &entry, &mesh, &[]);
        match plan {
            ReferencePlan::Offload(h) => assert_eq!(h.id, "desktop"),
            other => panic!("expected offload, got {other:?}"),
        }
    }

    #[test]
    fn reference_plan_skips_authoritative_and_handles_no_partner() {
        let mesh = vec![dev("phone", SttClass::Fast)];
        // authoritative entry → Skip regardless of mesh
        let mut auth = Entry::new("k", "hi");
        auth.confirmations = crate::reconcile::CONFIRM_THRESHOLD;
        assert_eq!(reference_plan("phone", &auth, &mesh, &[]), ReferencePlan::Skip);
        // unvalidated but no heavy partner → Unavailable (fastpath stands)
        let entry = Entry::new("k", "hi");
        assert_eq!(reference_plan("phone", &entry, &mesh, &[]), ReferencePlan::Unavailable);
    }

    #[test]
    fn reference_plan_runs_local_when_origin_is_heavy_alone() {
        let mesh = vec![dev("desktop", SttClass::Heavy)];
        let entry = Entry::new("k", "hi");
        assert_eq!(reference_plan("desktop", &entry, &mesh, &[]), ReferencePlan::Local);
    }
}
