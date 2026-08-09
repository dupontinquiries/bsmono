//! nat-kit — STUN messaging, and nothing else.
//!
//! The counterpart to `dht-kit`, and the same seam. A DHT that cannot learn its
//! own public address cannot be reached by peers behind NAT, so STUN is not an
//! optional extra in this stack — it is the step before a routing table means
//! anything.
//!
//! STUN already existed in `apps/snapclone/rust/snapclone-nat`, ported from
//! `NatTraversal.cpp`. That crate pulls **tokio, anyhow, bytes, rand and
//! tracing**, which is right for an application and wrong for a layer two
//! engines with different toolchains have to share. This kit is the codec with
//! none of them:
//!
//! * **no sockets, no async** — bytes in, message out
//! * **no randomness** — the caller supplies the transaction id, because a
//!   crate that generated one would either drag in `rand` or ship a weak PRNG,
//!   and the id is a security-relevant value the caller may already have a
//!   source for
//! * **no dependencies at all**
//!
//! ## The format (RFC 5389 §6)
//!
//! ```text
//! 0                   1                   2                   3
//! |0 0|     type      |         length        |
//! |            magic cookie  0x2112A442        |
//! |            transaction id (96 bits)        |
//! |            attributes...                   |
//! ```
//!
//! Attributes are type/length/value, padded to a 4-byte boundary. The one that
//! matters is **XOR-MAPPED-ADDRESS** (0x0020): the reflexive address with the
//! magic cookie XORed over it, which exists precisely because NATs rewrite
//! addresses they recognise in packet bodies — a plain MAPPED-ADDRESS gets
//! mangled in transit by the very device it is describing.

pub const MAGIC_COOKIE: u32 = 0x2112_A442;
pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_SUCCESS: u16 = 0x0101;
pub const BINDING_ERROR: u16 = 0x0111;

pub const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
pub const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
pub const ATTR_ERROR_CODE: u16 = 0x0009;

/// A 96-bit transaction id, supplied by the caller.
pub type TxId = [u8; 12];

/// A reflexive address as STUN reports it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Addr {
    V4([u8; 4], u16),
    V6([u8; 16], u16),
}

impl Addr {
    pub fn port(&self) -> u16 {
        match self { Addr::V4(_, p) | Addr::V6(_, p) => *p }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Msg {
    /// What a client sends. No attributes: a bare binding request is the whole
    /// of what this stack needs from STUN.
    BindingRequest { tx: TxId },
    /// What a server answers, carrying the reflexive address.
    BindingSuccess { tx: TxId, addr: Addr },
    BindingError { tx: TxId, code: u16, reason: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StunError {
    TooShort,
    NotStun,
    BadCookie,
    UnknownType(u16),
    /// The header's declared length disagrees with the bytes present.
    LengthMismatch,
    /// A success response with no address is useless and is refused rather than
    /// returned as a half-answer.
    NoAddress,
    BadAttribute,
    /// A **comprehension-required** attribute (type < 0x8000) we do not
    /// implement. RFC 5389 §7.3.1 is explicit that these must not be ignored:
    /// the sender is saying "you must understand this to understand me", and
    /// processing the message anyway means acting on a meaning we did not read.
    UnknownRequired(u16),
}

/// Is this attribute one a receiver may skip?
///
/// RFC 5389 §15 splits the space at 0x8000: **0x0000–0x7FFF are
/// comprehension-required**, 0x8000–0xFFFF are comprehension-optional. That
/// single bit is the difference between an extension point and a security hole,
/// and a parser that skips both treats every future mandatory attribute as
/// decoration.
pub fn is_comprehension_optional(attr: u16) -> bool {
    attr >= 0x8000
}

fn put_header(out: &mut Vec<u8>, kind: u16, tx: &TxId, body_len: u16) {
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(tx);
}

/// XOR an address with the cookie and transaction id, per RFC 5389 §15.2.
///
/// Symmetric — encoding and decoding are the same operation, which is why one
/// function serves both and there is no chance of the two drifting.
fn xor_addr(addr: &Addr, tx: &TxId) -> (u8, Vec<u8>, u16) {
    let cookie = MAGIC_COOKIE.to_be_bytes();
    match addr {
        Addr::V4(ip, port) => {
            let p = port ^ (MAGIC_COOKIE >> 16) as u16;
            let mut x = [0u8; 4];
            for i in 0..4 { x[i] = ip[i] ^ cookie[i]; }
            (0x01, x.to_vec(), p)
        }
        Addr::V6(ip, port) => {
            let p = port ^ (MAGIC_COOKIE >> 16) as u16;
            let mut key = [0u8; 16];
            key[..4].copy_from_slice(&cookie);
            key[4..].copy_from_slice(tx);
            let mut x = [0u8; 16];
            for i in 0..16 { x[i] = ip[i] ^ key[i]; }
            (0x02, x.to_vec(), p)
        }
    }
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Msg::BindingRequest { tx } => put_header(&mut out, BINDING_REQUEST, tx, 0),
            Msg::BindingSuccess { tx, addr } => {
                let (fam, xip, xport) = xor_addr(addr, tx);
                let vlen = 4 + xip.len();
                put_header(&mut out, BINDING_SUCCESS, tx, (4 + vlen) as u16);
                out.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
                out.extend_from_slice(&(vlen as u16).to_be_bytes());
                out.push(0);
                out.push(fam);
                out.extend_from_slice(&xport.to_be_bytes());
                out.extend_from_slice(&xip);
            }
            Msg::BindingError { tx, code, reason } => {
                let r = reason.as_bytes();
                let vlen = 4 + r.len();
                let pad = (4 - vlen % 4) % 4;
                put_header(&mut out, BINDING_ERROR, tx, (4 + vlen + pad) as u16);
                out.extend_from_slice(&ATTR_ERROR_CODE.to_be_bytes());
                out.extend_from_slice(&(vlen as u16).to_be_bytes());
                out.extend_from_slice(&[0, 0]);
                out.push((code / 100) as u8);
                out.push((code % 100) as u8);
                out.extend_from_slice(r);
                out.extend(std::iter::repeat(0).take(pad));
            }
        }
        out
    }

    pub fn decode(b: &[u8]) -> Result<Msg, StunError> {
        if b.len() < 20 { return Err(StunError::TooShort); }
        // The two most significant bits must be zero — this is what lets STUN
        // share a socket with other protocols, and checking it is how a DHT
        // packet arriving on the STUN port is rejected instead of misparsed.
        if b[0] & 0xC0 != 0 { return Err(StunError::NotStun); }
        let kind = u16::from_be_bytes([b[0], b[1]]);
        let len = u16::from_be_bytes([b[2], b[3]]) as usize;
        if u32::from_be_bytes([b[4], b[5], b[6], b[7]]) != MAGIC_COOKIE {
            return Err(StunError::BadCookie);
        }
        // Declared length must match exactly. A trailing-garbage tolerance here
        // would let an attacker append attributes a later parser might read.
        if b.len() != 20 + len { return Err(StunError::LengthMismatch); }
        let mut tx = [0u8; 12];
        tx.copy_from_slice(&b[8..20]);

        let mut addr = None;
        let mut code = None;
        let mut reason = String::new();
        let mut i = 20;
        while i + 4 <= b.len() {
            let at = u16::from_be_bytes([b[i], b[i + 1]]);
            let al = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            i += 4;
            if i + al > b.len() { return Err(StunError::BadAttribute); }
            let v = &b[i..i + al];
            match at {
                ATTR_XOR_MAPPED_ADDRESS | ATTR_MAPPED_ADDRESS => {
                    if v.len() < 4 { return Err(StunError::BadAttribute); }
                    let fam = v[1];
                    let raw_port = u16::from_be_bytes([v[2], v[3]]);
                    let xored = at == ATTR_XOR_MAPPED_ADDRESS;
                    let port = if xored { raw_port ^ (MAGIC_COOKIE >> 16) as u16 } else { raw_port };
                    let cookie = MAGIC_COOKIE.to_be_bytes();
                    match fam {
                        0x01 if v.len() >= 8 => {
                            let mut ip = [0u8; 4];
                            for k in 0..4 {
                                ip[k] = if xored { v[4 + k] ^ cookie[k] } else { v[4 + k] };
                            }
                            addr = Some(Addr::V4(ip, port));
                        }
                        0x02 if v.len() >= 20 => {
                            let mut key = [0u8; 16];
                            key[..4].copy_from_slice(&cookie);
                            key[4..].copy_from_slice(&tx);
                            let mut ip = [0u8; 16];
                            for k in 0..16 {
                                ip[k] = if xored { v[4 + k] ^ key[k] } else { v[4 + k] };
                            }
                            addr = Some(Addr::V6(ip, port));
                        }
                        _ => return Err(StunError::BadAttribute),
                    }
                }
                ATTR_ERROR_CODE => {
                    if v.len() < 4 { return Err(StunError::BadAttribute); }
                    code = Some(v[2] as u16 * 100 + v[3] as u16);
                    reason = String::from_utf8_lossy(&v[4..]).trim_end_matches('\0').to_string();
                }
                // The split RFC 5389 §7.3.1 requires, and which the first
                // version of this kit got wrong by skipping everything.
                //
                // Comprehension-OPTIONAL (>= 0x8000) is skipped: a STUN server
                // we do not own will send SOFTWARE, FINGERPRINT and others we
                // have never heard of, and refusing them would reject valid
                // responses from most of the internet.
                //
                // Comprehension-REQUIRED (< 0x8000) is refused. The sender is
                // saying "you must understand this to understand me" —
                // MESSAGE-INTEGRITY and USERNAME live in this range, so
                // silently skipping would mean accepting an authenticated
                // message while ignoring its authentication.
                other if is_comprehension_optional(other) => {}
                other => return Err(StunError::UnknownRequired(other)),
            }
            i += al + (4 - al % 4) % 4;
        }

        match kind {
            BINDING_REQUEST => Ok(Msg::BindingRequest { tx }),
            BINDING_SUCCESS => match addr {
                Some(a) => Ok(Msg::BindingSuccess { tx, addr: a }),
                None => Err(StunError::NoAddress),
            },
            BINDING_ERROR => Ok(Msg::BindingError {
                tx,
                code: code.unwrap_or(0),
                reason,
            }),
            other => Err(StunError::UnknownType(other)),
        }
    }

    pub fn tx(&self) -> &TxId {
        match self {
            Msg::BindingRequest { tx }
            | Msg::BindingSuccess { tx, .. }
            | Msg::BindingError { tx, .. } => tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TX: TxId = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
                      0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC];

    #[test]
    fn a_binding_request_is_twenty_bytes_and_round_trips() {
        // RFC 5389: header only, no attributes. If this grows, something is
        // adding attributes a plain STUN server may not expect.
        let m = Msg::BindingRequest { tx: TX };
        let b = m.encode();
        assert_eq!(b.len(), 20);
        assert_eq!(Msg::decode(&b), Ok(m));
    }

    #[test]
    fn an_ipv4_reflexive_address_round_trips_through_the_xor() {
        let m = Msg::BindingSuccess { tx: TX, addr: Addr::V4([203, 0, 113, 7], 51234) };
        assert_eq!(Msg::decode(&m.encode()), Ok(m));
    }

    #[test]
    fn the_address_really_is_xored_on_the_wire() {
        // The whole point of XOR-MAPPED-ADDRESS: NATs rewrite addresses they
        // recognise inside packet bodies, so the plaintext must not appear. If
        // these bytes ever show up literally, the obfuscation was dropped and
        // the address will be mangled in transit by the device describing it.
        let ip = [203u8, 0, 113, 7];
        let b = Msg::BindingSuccess { tx: TX, addr: Addr::V4(ip, 51234) }.encode();
        assert!(!b.windows(4).any(|w| w == ip),
                "the raw address appears on the wire");
        assert!(!b.windows(2).any(|w| w == 51234u16.to_be_bytes()),
                "the raw port appears on the wire");
    }

    #[test]
    fn an_ipv6_address_xors_with_the_transaction_id_too() {
        // v4 uses the cookie alone; v6 uses cookie ++ tx. Getting that wrong
        // decodes to a plausible but wrong address rather than failing.
        let ip = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let m = Msg::BindingSuccess { tx: TX, addr: Addr::V6(ip, 4433) };
        let encoded = m.encode();
        assert_eq!(Msg::decode(&encoded), Ok(m));
        let other_tx: TxId = [9u8; 12];
        let wrong = Msg::BindingSuccess { tx: other_tx, addr: Addr::V6(ip, 4433) };
        assert_ne!(wrong.encode()[20..], encoded[20..],
                   "the same address under a different tx must encode differently");
    }

    #[test]
    fn a_plain_mapped_address_is_also_accepted() {
        // Older servers send MAPPED-ADDRESS unxored. Refusing it would drop
        // responses from real deployments.
        let mut b = Vec::new();
        put_header(&mut b, BINDING_SUCCESS, &TX, 12);
        b.extend_from_slice(&ATTR_MAPPED_ADDRESS.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&[0, 0x01]);
        b.extend_from_slice(&4433u16.to_be_bytes());
        b.extend_from_slice(&[198, 51, 100, 9]);
        assert_eq!(Msg::decode(&b), Ok(Msg::BindingSuccess {
            tx: TX, addr: Addr::V4([198, 51, 100, 9], 4433) }));
    }

    #[test]
    fn a_success_with_no_address_is_refused_rather_than_half_answered() {
        let mut b = Vec::new();
        put_header(&mut b, BINDING_SUCCESS, &TX, 0);
        assert_eq!(Msg::decode(&b), Err(StunError::NoAddress));
    }

    #[test]
    fn a_wrong_magic_cookie_is_refused() {
        let mut b = Msg::BindingRequest { tx: TX }.encode();
        b[4] ^= 0xFF;
        assert_eq!(Msg::decode(&b), Err(StunError::BadCookie));
    }

    #[test]
    fn a_packet_whose_top_bits_are_set_is_not_stun() {
        // This is what lets STUN share a socket: a DHT packet arriving on the
        // STUN port must be rejected, not misparsed into a routing decision.
        let mut b = Msg::BindingRequest { tx: TX }.encode();
        b[0] |= 0x80;
        assert_eq!(Msg::decode(&b), Err(StunError::NotStun));
    }

    #[test]
    fn a_declared_length_that_disagrees_with_the_bytes_is_refused() {
        // Trailing-garbage tolerance would let an attacker append attributes a
        // later parser might read.
        let mut b = Msg::BindingSuccess { tx: TX, addr: Addr::V4([1, 2, 3, 4], 99) }.encode();
        b.push(0);
        assert_eq!(Msg::decode(&b), Err(StunError::LengthMismatch));
        let full = Msg::BindingRequest { tx: TX }.encode();
        for cut in 0..full.len() {
            assert!(Msg::decode(&full[..cut]).is_err(), "{cut}-byte prefix decoded");
        }
    }

    #[test]
    fn unknown_attributes_are_skipped_rather_than_refused() {
        // The opposite choice from dht-kit, and deliberately: we control both
        // ends of the DHT and neither end of a public STUN server. Refusing
        // unknown comprehension-optional attributes would reject valid
        // responses from most of the internet.
        let mut b = Vec::new();
        put_header(&mut b, BINDING_SUCCESS, &TX, 12 + 8);
        b.extend_from_slice(&0x8022u16.to_be_bytes());   // SOFTWARE, optional
        b.extend_from_slice(&4u16.to_be_bytes());
        b.extend_from_slice(b"abcd");
        b.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        let (fam, xip, xport) = xor_addr(&Addr::V4([198, 51, 100, 9], 4433), &TX);
        b.push(0);
        b.push(fam);
        b.extend_from_slice(&xport.to_be_bytes());
        b.extend_from_slice(&xip);
        assert_eq!(Msg::decode(&b), Ok(Msg::BindingSuccess {
            tx: TX, addr: Addr::V4([198, 51, 100, 9], 4433) }));
    }

    #[test]
    fn an_error_response_round_trips_with_its_padded_reason() {
        // Reasons are variable length and attributes pad to 4 bytes; an
        // off-by-one in the padding shifts every following attribute.
        for reason in ["", "Bad Request", "Try Alternate"] {
            let m = Msg::BindingError { tx: TX, code: 400, reason: reason.into() };
            assert_eq!(Msg::decode(&m.encode()), Ok(m), "reason {reason:?}");
        }
    }


    fn success_with_attr(attr: u16, value: &[u8]) -> Vec<u8> {
        let (fam, xip, xport) = xor_addr(&Addr::V4([198, 51, 100, 9], 4433), &TX);
        let pad = (4 - value.len() % 4) % 4;
        let body = 4 + value.len() + pad + 12;
        let mut b = Vec::new();
        put_header(&mut b, BINDING_SUCCESS, &TX, body as u16);
        b.extend_from_slice(&attr.to_be_bytes());
        b.extend_from_slice(&(value.len() as u16).to_be_bytes());
        b.extend_from_slice(value);
        b.extend(std::iter::repeat(0).take(pad));
        b.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.push(0);
        b.push(fam);
        b.extend_from_slice(&xport.to_be_bytes());
        b.extend_from_slice(&xip);
        b
    }

    #[test]
    fn the_split_is_at_0x8000_exactly() {
        // One bit separates an extension point from a security hole.
        assert!(!is_comprehension_optional(0x0000));
        assert!(!is_comprehension_optional(0x7FFF));
        assert!(is_comprehension_optional(0x8000));
        assert!(is_comprehension_optional(0xFFFF));
        // The real attributes either side of the line.
        assert!(!is_comprehension_optional(0x0008), "MESSAGE-INTEGRITY is required");
        assert!(!is_comprehension_optional(0x0006), "USERNAME is required");
        assert!(is_comprehension_optional(0x8022), "SOFTWARE is optional");
        assert!(is_comprehension_optional(0x8028), "FINGERPRINT is optional");
    }

    #[test]
    fn an_unknown_optional_attribute_is_skipped() {
        // SOFTWARE from a server we do not own. Refusing these would reject
        // valid responses from most of the internet.
        let b = success_with_attr(0x8022, b"coturn");
        assert_eq!(Msg::decode(&b), Ok(Msg::BindingSuccess {
            tx: TX, addr: Addr::V4([198, 51, 100, 9], 4433) }));
    }

    #[test]
    fn an_unknown_required_attribute_is_refused() {
        // The bug this kit shipped with: skipping everything. An attribute
        // below 0x8000 means "you must understand this to understand me", and
        // processing the message anyway acts on a meaning we did not read.
        let b = success_with_attr(0x0025, b"\x00\x00\x00\x00");
        assert_eq!(Msg::decode(&b), Err(StunError::UnknownRequired(0x0025)));
    }

    #[test]
    fn message_integrity_is_never_silently_ignored() {
        // The case that makes this a security matter rather than a spec
        // detail. MESSAGE-INTEGRITY is 0x0008 — comprehension-required — so a
        // parser that skips unknown attributes accepts an AUTHENTICATED message
        // while ignoring its authentication, which is strictly worse than not
        // supporting authentication at all.
        let b = success_with_attr(0x0008, &[0u8; 20]);
        assert_eq!(Msg::decode(&b), Err(StunError::UnknownRequired(0x0008)),
                   "an unimplemented MESSAGE-INTEGRITY must stop the parse");
    }

    #[test]
    fn the_transaction_id_survives_and_identifies_the_exchange() {
        // A response is matched to its request by tx alone; losing it means
        // accepting an answer to somebody else's question.
        let m = Msg::BindingSuccess { tx: TX, addr: Addr::V4([1, 1, 1, 1], 1) };
        assert_eq!(Msg::decode(&m.encode()).unwrap().tx(), &TX);
    }
}
