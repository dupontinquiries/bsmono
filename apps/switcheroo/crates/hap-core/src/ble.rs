//! HAP over BLE (spec chapter 7): the PDU framing, fragmentation to the
//! MTU, the encrypted-session wrapping, and a [`Controller`] that drives
//! pair-setup, pair-verify and characteristic reads/writes over a
//! [`Transport`].
//!
//! # The seam
//!
//! [`Transport`] is three synchronous calls — write a characteristic, read
//! it, tell me its HAP instance id — plus the MTU. That is everything HAP
//! needs from GATT, and it is the same shape as switcheroo-core's
//! `Backend`: the radio thread lives behind it, tests put an in-process
//! accessory ([`crate::sim::Accessory`]) behind it, and this file never
//! learns which. No async, so `dyn Transport` works without boxing.
//!
//! # PDU layout (spec §7.3.3)
//!
//! ```text
//!   request   ctl=0x00 | opcode | tid | iid(2 LE) | [len(2 LE) | body]
//!   response  ctl=0x02 | tid    | status        | [len(2 LE) | body]
//!   fragment  ctl|0x80 | tid    | data...                 (continuation of either)
//! ```
//!
//! Once a session is up, every fragment is encrypted on its own (spec
//! §7.4.6): plaintext is cut so `chunk + 16-byte tag` fits the MTU, and
//! the counter advances per fragment, both directions.

use crate::keys::Pairing;
use crate::pair_setup::{self, PairSetup, PairSetupError};
use crate::pair_verify::{self, PairVerify, PairVerifyError};
use crate::session::SecureSession;
use crate::tlv8::{Tlv, TlvError};

// ── HAP UUIDs ──────────────────────────────────────────────────────────────

/// Apple's base UUID: `0000XXXX-0000-1000-8000-0026BB765291`.
pub fn hap_uuid(short: u16) -> String {
    format!("{short:08X}-0000-1000-8000-0026BB765291")
}

/// The short (HAP-assigned) type of a full UUID, if it is a HAP one.
pub fn hap_short(uuid: &str) -> Option<u16> {
    let u = uuid.to_ascii_uppercase();
    let (head, tail) = u.split_at(u.find('-')?);
    if tail != "-0000-1000-8000-0026BB765291" || head.len() != 8 || !head.starts_with("0000") {
        return None;
    }
    u16::from_str_radix(&head[4..], 16).ok()
}

pub mod service {
    pub const ACCESSORY_INFORMATION: u16 = 0x3E;
    pub const PROTOCOL_INFORMATION: u16 = 0xA2;
    pub const PAIRING: u16 = 0x55;
    pub const LIGHTBULB: u16 = 0x43;
    pub const OUTLET: u16 = 0x47;
    pub const SWITCH: u16 = 0x49;
}

pub mod characteristic {
    pub const IDENTIFY: u16 = 0x14;
    pub const NAME: u16 = 0x23;
    pub const ON: u16 = 0x25;
    pub const OUTLET_IN_USE: u16 = 0x26;
    pub const BRIGHTNESS: u16 = 0x08;
    pub const PAIR_SETUP: u16 = 0x4C;
    pub const PAIR_VERIFY: u16 = 0x4E;
    pub const PAIRING_FEATURES: u16 = 0x4F;
    pub const PAIRINGS: u16 = 0x50;
    pub const SERVICE_SIGNATURE: u16 = 0xA5;
    /// Descriptor on every HAP characteristic carrying its instance id.
    pub const INSTANCE_ID_DESCRIPTOR: &str = "DC46F0FE-81D2-4616-B5D9-6ABDD796939A";
    /// Characteristic in every HAP service carrying the service instance id.
    pub const SERVICE_INSTANCE_ID: &str = "E604E95D-A759-4817-87D3-AA005083A0D1";
}

pub mod opcode {
    pub const CHARACTERISTIC_SIGNATURE_READ: u8 = 0x01;
    pub const CHARACTERISTIC_WRITE: u8 = 0x02;
    pub const CHARACTERISTIC_READ: u8 = 0x03;
    pub const CHARACTERISTIC_TIMED_WRITE: u8 = 0x04;
    pub const CHARACTERISTIC_EXECUTE_WRITE: u8 = 0x05;
    pub const SERVICE_SIGNATURE_READ: u8 = 0x06;
    pub const CHARACTERISTIC_CONFIGURATION: u8 = 0x07;
    pub const PROTOCOL_CONFIGURATION: u8 = 0x08;
}

/// Body TLV parameter types (spec table 7-10).
pub mod param {
    pub const VALUE: u8 = 0x01;
    pub const ADDITIONAL_AUTHORIZATION_DATA: u8 = 0x02;
    pub const ORIGIN: u8 = 0x03;
    pub const CHARACTERISTIC_TYPE: u8 = 0x04;
    pub const CHARACTERISTIC_INSTANCE_ID: u8 = 0x05;
    pub const SERVICE_TYPE: u8 = 0x06;
    pub const SERVICE_INSTANCE_ID: u8 = 0x07;
    pub const TTL: u8 = 0x08;
    pub const RETURN_RESPONSE: u8 = 0x09;
    pub const CHARACTERISTIC_PROPERTIES: u8 = 0x0A;
    pub const GATT_USER_DESCRIPTION: u8 = 0x0B;
    pub const GATT_PRESENTATION_FORMAT: u8 = 0x0C;
    pub const GATT_VALID_RANGE: u8 = 0x0D;
    pub const STEP_VALUE: u8 = 0x0E;
    pub const SERVICE_PROPERTIES: u8 = 0x0F;
    pub const LINKED_SERVICES: u8 = 0x10;
    pub const VALID_VALUES: u8 = 0x11;
    pub const VALID_VALUES_RANGE: u8 = 0x12;
}

/// Response status codes (spec table 7-37).
pub mod status {
    pub const SUCCESS: u8 = 0x00;
    pub const UNSUPPORTED_PDU: u8 = 0x01;
    pub const MAX_PROCEDURES: u8 = 0x02;
    pub const INSUFFICIENT_AUTHORIZATION: u8 = 0x03;
    pub const INVALID_INSTANCE_ID: u8 = 0x04;
    pub const INSUFFICIENT_AUTHENTICATION: u8 = 0x05;
    pub const INVALID_REQUEST: u8 = 0x06;

    pub fn describe(s: u8) -> &'static str {
        match s {
            SUCCESS => "success",
            UNSUPPORTED_PDU => "unsupported PDU",
            MAX_PROCEDURES => "too many concurrent procedures",
            INSUFFICIENT_AUTHORIZATION => "insufficient authorization",
            INVALID_INSTANCE_ID => "invalid instance id",
            INSUFFICIENT_AUTHENTICATION => "insufficient authentication (no secure session)",
            INVALID_REQUEST => "invalid request",
            _ => "unknown status",
        }
    }
}

const CTL_REQUEST: u8 = 0x00;
const CTL_RESPONSE: u8 = 0x02;
const CTL_CONTINUATION: u8 = 0x80;
const TAG_LEN: usize = 16;

// ── PDUs ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub opcode: u8,
    pub tid: u8,
    pub iid: u16,
    pub body: Vec<u8>,
}

impl Request {
    /// The unfragmented PDU.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![CTL_REQUEST, self.opcode, self.tid];
        out.extend_from_slice(&self.iid.to_le_bytes());
        if !self.body.is_empty() {
            out.extend_from_slice(&(self.body.len() as u16).to_le_bytes());
            out.extend_from_slice(&self.body);
        }
        out
    }

    /// Split for a transport whose writes carry at most `max` bytes each.
    /// The first fragment keeps the full header (with the TOTAL body
    /// length); continuations are `ctl|0x80, tid, data`.
    pub fn fragments(&self, max: usize) -> Vec<Vec<u8>> {
        let whole = self.encode();
        if whole.len() <= max {
            return vec![whole];
        }
        let mut out = Vec::new();
        let (head, rest) = whole.split_at(max);
        out.push(head.to_vec());
        for chunk in rest.chunks(max.saturating_sub(2).max(1)) {
            let mut f = vec![CTL_REQUEST | CTL_CONTINUATION, self.tid];
            f.extend_from_slice(chunk);
            out.push(f);
        }
        out
    }

    /// Parse a complete (already reassembled) request. For the simulator.
    pub fn decode(b: &[u8]) -> Result<Request, HapError> {
        if b.len() < 5 || b[0] & !CTL_CONTINUATION != CTL_REQUEST {
            return Err(HapError::Protocol("not a request PDU".into()));
        }
        let iid = u16::from_le_bytes([b[3], b[4]]);
        let body = if b.len() >= 7 {
            let len = u16::from_le_bytes([b[5], b[6]]) as usize;
            if b.len() < 7 + len { return Err(HapError::Protocol("request body short".into())); }
            b[7..7 + len].to_vec()
        } else { Vec::new() };
        Ok(Request { opcode: b[1], tid: b[2], iid, body })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub tid: u8,
    pub status: u8,
    pub body: Vec<u8>,
}

impl Response {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![CTL_RESPONSE, self.tid, self.status];
        if !self.body.is_empty() {
            out.extend_from_slice(&(self.body.len() as u16).to_le_bytes());
            out.extend_from_slice(&self.body);
        }
        out
    }

    pub fn fragments(&self, max: usize) -> Vec<Vec<u8>> {
        let whole = self.encode();
        if whole.len() <= max {
            return vec![whole];
        }
        let mut out = Vec::new();
        let (head, rest) = whole.split_at(max);
        out.push(head.to_vec());
        for chunk in rest.chunks(max.saturating_sub(2).max(1)) {
            let mut f = vec![CTL_RESPONSE | CTL_CONTINUATION, self.tid];
            f.extend_from_slice(chunk);
            out.push(f);
        }
        out
    }
}

/// Reassembles a fragmented PDU from successive reads. Feed the first
/// read, then keep feeding until [`Reassembler::done`].
#[derive(Debug, Default)]
pub struct Reassembler {
    buf: Vec<u8>,
    expected_total: Option<usize>,
    tid: Option<u8>,
}

impl Reassembler {
    /// Header length before the body-length field: 5 for requests, 3 for responses.
    fn header_len(first_ctl: u8) -> usize {
        if first_ctl & 0x02 != 0 { 3 } else { 5 }
    }

    pub fn feed(&mut self, fragment: &[u8]) -> Result<(), HapError> {
        if fragment.is_empty() {
            return Err(HapError::Protocol("empty fragment".into()));
        }
        if self.expected_total.is_none() {
            // First fragment: full header.
            let hl = Self::header_len(fragment[0]);
            if fragment[0] & CTL_CONTINUATION != 0 {
                return Err(HapError::Protocol("first fragment flagged as continuation".into()));
            }
            self.tid = Some(if hl == 3 { fragment[1] } else { fragment[2] });
            let total = if fragment.len() >= hl + 2 {
                hl + 2 + u16::from_le_bytes([fragment[hl], fragment[hl + 1]]) as usize
            } else {
                // No body-length field: the PDU is just the header.
                fragment.len()
            };
            self.expected_total = Some(total);
            self.buf.extend_from_slice(fragment);
        } else {
            if fragment.len() < 2 || fragment[0] & CTL_CONTINUATION == 0 {
                return Err(HapError::Protocol("expected a continuation fragment".into()));
            }
            if Some(fragment[1]) != self.tid {
                return Err(HapError::Protocol(format!("continuation tid {} != {}", fragment[1], self.tid.unwrap_or(0))));
            }
            self.buf.extend_from_slice(&fragment[2..]);
        }
        Ok(())
    }

    pub fn done(&self) -> bool {
        matches!(self.expected_total, Some(t) if self.buf.len() >= t)
    }

    pub fn into_bytes(self) -> Vec<u8> { self.buf }
}

fn decode_response(b: &[u8]) -> Result<Response, HapError> {
    if b.len() < 3 || b[0] & !CTL_CONTINUATION != CTL_RESPONSE {
        return Err(HapError::Protocol(format!("not a response PDU (ctl={:#04x})", b.first().copied().unwrap_or(0))));
    }
    let body = if b.len() >= 5 {
        let len = u16::from_le_bytes([b[3], b[4]]) as usize;
        if b.len() < 5 + len { return Err(HapError::Protocol("response body short".into())); }
        b[5..5 + len].to_vec()
    } else { Vec::new() };
    Ok(Response { tid: b[1], status: b[2], body })
}

// ── the seam ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

/// What HAP needs from GATT. Characteristics are named by their HAP short
/// type; the transport owns the mapping to handles.
pub trait Transport {
    fn write(&mut self, char_type: u16, data: &[u8]) -> Result<(), TransportError>;
    fn read(&mut self, char_type: u16) -> Result<Vec<u8>, TransportError>;
    /// From the characteristic's Instance ID descriptor.
    fn instance_id(&mut self, char_type: u16) -> Result<u16, TransportError>;
    /// Largest write/read payload. 512 is the BLE ceiling; real links are
    /// often 20-ish until the MTU is negotiated up.
    fn mtu(&self) -> usize;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HapError {
    Transport(String),
    /// The accessory answered with a non-success status.
    Status { opcode: u8, status: u8 },
    Protocol(String),
    Tlv(TlvError),
    PairSetup(PairSetupError),
    PairVerify(PairVerifyError),
    /// A session frame failed to authenticate. The session is dead; a new
    /// pair-verify is the only recovery.
    SessionBroken,
    /// The operation needs a verified session and there is none.
    NotVerified,
}

impl std::fmt::Display for HapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HapError::Transport(s) => write!(f, "transport: {s}"),
            HapError::Status { opcode, status } => write!(f, "accessory returned {} to opcode {opcode:#04x}", status::describe(*status)),
            HapError::Protocol(s) => write!(f, "HAP-BLE protocol: {s}"),
            HapError::Tlv(e) => write!(f, "HAP-BLE: {e}"),
            HapError::PairSetup(e) => write!(f, "{e}"),
            HapError::PairVerify(e) => write!(f, "{e}"),
            HapError::SessionBroken => write!(f, "secure session frame failed to authenticate; re-verify"),
            HapError::NotVerified => write!(f, "no secure session; pair-verify first"),
        }
    }
}

impl From<TransportError> for HapError { fn from(e: TransportError) -> Self { HapError::Transport(e.0) } }
impl From<TlvError> for HapError { fn from(e: TlvError) -> Self { HapError::Tlv(e) } }
impl From<PairSetupError> for HapError { fn from(e: PairSetupError) -> Self { HapError::PairSetup(e) } }
impl From<PairVerifyError> for HapError { fn from(e: PairVerifyError) -> Self { HapError::PairVerify(e) } }

// ── the controller ─────────────────────────────────────────────────────────

/// Runs HAP procedures over one connection. Owns the transaction id
/// counter and, after pair-verify, the session.
pub struct Controller<'t> {
    t: &'t mut dyn Transport,
    tid: u8,
    session: Option<SecureSession>,
    /// Every PDU exchanged, for the UI's journal: (direction, opcode, status, body length).
    pub trace: Vec<String>,
}

impl<'t> Controller<'t> {
    pub fn new(t: &'t mut dyn Transport) -> Self {
        Controller { t, tid: 0, session: None, trace: Vec::new() }
    }

    pub fn is_secure(&self) -> bool { self.session.is_some() }

    fn next_tid(&mut self) -> u8 {
        self.tid = self.tid.wrapping_add(1);
        self.tid
    }

    /// One request/response round trip on `char_type`. Handles
    /// fragmentation both ways and the session, and turns a non-success
    /// status into an error. Returns the response body.
    pub fn exchange(&mut self, char_type: u16, opcode: u8, body: Vec<u8>) -> Result<Vec<u8>, HapError> {
        let iid = self.t.instance_id(char_type)?;
        let tid = self.next_tid();
        let req = Request { opcode, tid, iid, body };
        let mtu = self.t.mtu();
        // With a session, each fragment grows by the tag, so cut smaller.
        let max = if self.session.is_some() { mtu.saturating_sub(TAG_LEN).max(3) } else { mtu };
        let frags = req.fragments(max);
        self.trace.push(format!("-> op {opcode:#04x} tid {tid} iid {iid} body {}B in {} fragment(s)", req.body.len(), frags.len()));
        for f in frags {
            let bytes = match &mut self.session {
                Some(s) => s.seal_ble(&f),
                None => f,
            };
            self.t.write(char_type, &bytes)?;
        }
        let mut asm = Reassembler::default();
        loop {
            let raw = self.t.read(char_type)?;
            let plain = match &mut self.session {
                Some(s) => s.open_ble(&raw).map_err(|_| HapError::SessionBroken)?,
                None => raw,
            };
            asm.feed(&plain)?;
            if asm.done() { break; }
        }
        let resp = decode_response(&asm.into_bytes())?;
        self.trace.push(format!("<- tid {} status {} body {}B", resp.tid, status::describe(resp.status), resp.body.len()));
        if resp.tid != tid {
            return Err(HapError::Protocol(format!("response tid {} for request tid {tid}", resp.tid)));
        }
        if resp.status != status::SUCCESS {
            return Err(HapError::Status { opcode, status: resp.status });
        }
        Ok(resp.body)
    }

    /// Write a pairing TLV to a pairing characteristic and get the reply
    /// TLV back — the shape both pair-setup and pair-verify use.
    fn pairing_round(&mut self, char_type: u16, tlv_bytes: Vec<u8>) -> Result<Vec<u8>, HapError> {
        let body = Tlv::new().with(param::VALUE, tlv_bytes).with(param::RETURN_RESPONSE, [1u8]).encode();
        let reply = self.exchange(char_type, opcode::CHARACTERISTIC_WRITE, body)?;
        let tlv = Tlv::decode(&reply)?;
        tlv.get(param::VALUE).map(|v| v.to_vec()).ok_or_else(|| HapError::Protocol("pairing reply has no value".into()))
    }

    /// One Pairings round trip: the M1 body in, the M2 body out. Same shape as a pairing
    /// round because the Pairings characteristic speaks the same write-with-response
    /// protocol as pair-setup and pair-verify.
    pub(crate) fn pairings_exchange(&mut self, tlv_bytes: Vec<u8>) -> Result<Vec<u8>, HapError> {
        self.pairing_round(characteristic::PAIRINGS, tlv_bytes)
    }

    /// Forget the session without touching the accessory: after a Remove Pairing the
    /// accessory has already dropped its side, and a controller holding a session it
    /// cannot use would report `SessionBroken` on the next call instead of `NotVerified`.
    pub(crate) fn drop_session(&mut self) {
        self.session = None;
    }

    /// Pair Setup with the accessory's setup code. On success the returned
    /// record must be persisted by the caller — it is the only copy.
    pub fn pair_setup(&mut self, controller_id: &str, controller_ltsk: [u8; 32], setup_code: &str) -> Result<Pairing, HapError> {
        let mut ps = PairSetup::new(controller_id, controller_ltsk, setup_code)?;
        let mut out = ps.start();
        loop {
            let reply = self.pairing_round(characteristic::PAIR_SETUP, out)?;
            match ps.handle(&reply)? {
                pair_setup::Outcome::Send(next) => out = next,
                pair_setup::Outcome::Done(p) => return Ok(p),
            }
        }
    }

    /// Pair Verify with a stored pairing. On success the controller holds a
    /// session and reads/writes are encrypted from here on.
    pub fn pair_verify(&mut self, pairing: &Pairing) -> Result<(), HapError> {
        let mut pv = PairVerify::new(pairing.clone());
        let mut out = pv.start();
        loop {
            let reply = self.pairing_round(characteristic::PAIR_VERIFY, out)?;
            match pv.handle(&reply)? {
                pair_verify::Outcome::Send(next) => out = next,
                pair_verify::Outcome::Done(s) => {
                    self.session = Some(s);
                    return Ok(());
                }
            }
        }
    }

    /// Characteristic-Write of a raw value. Needs a session.
    pub fn write_value(&mut self, char_type: u16, value: &[u8]) -> Result<(), HapError> {
        if self.session.is_none() { return Err(HapError::NotVerified); }
        let body = Tlv::new().with(param::VALUE, value).encode();
        self.exchange(char_type, opcode::CHARACTERISTIC_WRITE, body)?;
        Ok(())
    }

    /// Characteristic-Read → the raw value. Needs a session.
    pub fn read_value(&mut self, char_type: u16) -> Result<Vec<u8>, HapError> {
        if self.session.is_none() { return Err(HapError::NotVerified); }
        let reply = self.exchange(char_type, opcode::CHARACTERISTIC_READ, Vec::new())?;
        let tlv = Tlv::decode(&reply)?;
        tlv.get(param::VALUE).map(|v| v.to_vec()).ok_or_else(|| HapError::Protocol("read reply has no value".into()))
    }

    /// The `On` characteristic (0x25), a one-byte bool.
    pub fn set_on(&mut self, on: bool) -> Result<(), HapError> {
        self.write_value(characteristic::ON, &[on as u8])
    }

    pub fn get_on(&mut self) -> Result<bool, HapError> {
        let v = self.read_value(characteristic::ON)?;
        match v.as_slice() {
            [b] => Ok(*b != 0),
            _ => Err(HapError::Protocol(format!("On is {} bytes, expected 1", v.len()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hap_uuids_round_trip() {
        assert_eq!(hap_uuid(0x4C), "0000004C-0000-1000-8000-0026BB765291");
        assert_eq!(hap_short("0000004c-0000-1000-8000-0026bb765291"), Some(0x4C));
        assert_eq!(hap_short("DC46F0FE-81D2-4616-B5D9-6ABDD796939A"), None);
        assert_eq!(hap_short("00002A57-0000-1000-8000-00805F9B34FB"), None, "the Bluetooth SIG base is not HAP's");
    }

    #[test]
    fn request_layout_matches_the_spec() {
        let r = Request { opcode: opcode::CHARACTERISTIC_WRITE, tid: 7, iid: 0x0102, body: vec![1, 2, 3] };
        assert_eq!(r.encode(), vec![0x00, 0x02, 7, 0x02, 0x01, 3, 0, 1, 2, 3]);
        let bare = Request { opcode: opcode::CHARACTERISTIC_READ, tid: 1, iid: 9, body: vec![] };
        assert_eq!(bare.encode(), vec![0x00, 0x03, 1, 9, 0], "no body → no length field");
        assert_eq!(Request::decode(&r.encode()).unwrap(), r);
    }

    #[test]
    fn a_big_request_fragments_and_a_big_response_reassembles() {
        let body: Vec<u8> = (0..500u32).map(|i| i as u8).collect();
        let r = Request { opcode: 2, tid: 3, iid: 1, body: body.clone() };
        let frags = r.fragments(100);
        assert!(frags.len() > 1);
        assert_eq!(frags[0].len(), 100);
        assert_eq!(frags[1][0], 0x80);
        assert_eq!(frags[1][1], 3);
        // The transport-independent reassembly is the same code path the
        // controller uses on responses.
        let resp = Response { tid: 3, status: 0, body };
        let mut asm = Reassembler::default();
        for f in resp.fragments(64) {
            assert!(!asm.done());
            asm.feed(&f).unwrap();
        }
        assert!(asm.done());
        assert_eq!(decode_response(&asm.into_bytes()).unwrap(), resp);
    }

    #[test]
    fn a_response_with_no_body_is_complete_on_the_first_read() {
        let mut asm = Reassembler::default();
        asm.feed(&[0x02, 5, 0x00]).unwrap();
        assert!(asm.done());
        assert_eq!(decode_response(&asm.into_bytes()).unwrap(), Response { tid: 5, status: 0, body: vec![] });
    }

    #[test]
    fn a_continuation_with_the_wrong_tid_is_refused() {
        let mut asm = Reassembler::default();
        asm.feed(&[0x02, 5, 0x00, 10, 0, 1, 2, 3]).unwrap();
        assert!(matches!(asm.feed(&[0x82, 6, 4, 5]).unwrap_err(), HapError::Protocol(_)));
    }
}
