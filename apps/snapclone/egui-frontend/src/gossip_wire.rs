//! gossip_wire — the datagram format peers actually exchange, and the socket
//! that carries it.
//!
//! `gossip.rs` has the POLICY — a partial view, TTL pruning, fanout selection,
//! an admission check — and no way to get a record from another machine. There
//! is no encoder, no decoder and no socket anywhere in this frontend, so
//! `PeerRecord` could only ever be constructed in a test. Every contact loaded
//! from disk stays offline forever, because nothing is listening.
//!
//! ## The format is Qt's, not a new one
//!
//! JSON, as documented in `apps/snapclone/rust/snapclone-gossip/src/message.rs`
//! and originally `GossipDiscovery.cpp::processDatagram`:
//!
//! ```json
//! {"id":"sc-32a51661","host":"192.168.1.5","port":9876,"ts":1700000000000,
//!  "peers":[{"id":"sc-aabbccdd","host":"192.168.1.6","port":9876,"ts":...}]}
//! ```
//!
//! That crate does the same job with serde_json, and using it was the first
//! instinct. It is not in this frontend's lock and pulls serde, serde_json,
//! chrono, anyhow, rand and tokio behind it — a tokio runtime to read one
//! datagram. The schema is five fields and an array of five-field objects, so
//! it is hand-written here against std, and
//! `the_shape_matches_what_the_serde_crate_documents` pins the field names and
//! types that make the two interoperable.
//!
//! Hand-writing a JSON reader is normally a bad trade. It earns itself here for
//! a specific reason: **this input is hostile by definition.** Gossip arrives
//! from anyone on the subnet, so the parser is the attack surface, and one that
//! refuses depth, refuses trailing data, caps every length and allocates
//! nothing before it has checked a bound is worth more than one that is
//! permissive because a general-purpose library has to be.
//!
//! ## The signature gap, which is real and is NOT papered over
//!
//! `gossip::accept` refuses any record whose `verified` flag is false, citing
//! the design note that gossip is signed so nodes cannot impersonate each
//! other. **This wire format carries no signature.** Neither does Qt's. So a
//! record decoded here is `verified: false` and `accept` rejects it, which
//! means this socket does not yet make anyone appear online.
//!
//! Setting `verified: true` at the decoder would have made presence work this
//! afternoon and would have been a lie in the one place the codebase
//! explicitly says not to lie — one host on the subnet announcing itself as
//! every peer is exactly the attack the flag exists for. So the loop is wired
//! end to end and visibly refuses: the host gets `GossipAdmitted(false)` for
//! every datagram and can say why. `a_decoded_record_is_unsigned_and_therefore_
//! refused` pins that, and it is the test to delete when signing lands —
//! signing needs a keypair in `identity.conf` and `crypto-kit` as a dependency,
//! which is a lock change and a decision about the wire format, not something
//! to slip in under a networking commit.

use crate::gossip::{PeerRecord, Reject, MAX_DATAGRAM_BYTES, MAX_FIELD_LEN};

/// Qt's `kMaxGossipPeers`. A sample, not a full view.
pub const MAX_GOSSIP_PEERS: usize = 8;

/// How deep a value may nest. The format needs two (object → array → object);
/// anything more is someone probing for a stack overflow.
const MAX_DEPTH: usize = 4;

/// One announcement: who is sending, and a sample of who they know.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Announce {
    pub id: String,
    pub host: String,
    pub port: u16,
    /// Unix ms, as the sender reports it.
    pub ts: u64,
    pub peers: Vec<PeerRecord>,
    /// The sender's Ed25519 verifying key, when the datagram carried one.
    /// `None` for a Qt sender, which does not sign.
    pub public: Option<[u8; 32]>,
    /// Set by [`decode`] only: the signature checked out AND the sender's
    /// sc-ID is the fingerprint of the key that made it.
    ///
    /// Not a field a caller fills in. It is `false` on anything constructed by
    /// hand, which is the safe direction — the one way to get `true` is to have
    /// been decoded from bytes that verified.
    pub verified: bool,
}

impl Announce {
    /// The sender's own record.
    ///
    /// `verified` is false and stays false until the format carries a
    /// signature — see the module header. This is the one place a caller might
    /// be tempted to set it, so it is a function rather than a struct literal
    /// at four call sites.
    /// The sender's record, with the address the datagram actually arrived
    /// from substituted when the announcement did not name one.
    ///
    /// A sender cannot reliably know which of its own interfaces a peer can
    /// reach, so this app announces an empty `host` rather than guessing — and
    /// the receiver already holds the answer, because the UDP source address is
    /// where the packet demonstrably came from.
    ///
    /// The source address is NOT covered by the signature, and that is the
    /// right way round: it is the transport's own observation, which makes it
    /// trustworthy for "where do I dial this peer" and worthless for "who is
    /// this peer". Identity comes from the signed id and key; reachability
    /// comes from here. A declared dotted-quad host wins when there is one,
    /// because a peer behind a router may legitimately know a better address
    /// for itself than the one its packets appear to come from.
    pub fn sender_record_from(&self, src: std::net::IpAddr) -> PeerRecord {
        let mut rec = self.sender_record();
        if rec.host.is_empty() {
            rec.host = src.to_string();
        }
        rec
    }

    pub fn sender_record(&self) -> PeerRecord {
        PeerRecord {
            id: self.id.clone(),
            host: self.host.clone(),
            port: self.port,
            last_seen_ms: self.ts,
            verified: self.verified,
        }
    }
}

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 { return None; }
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

// ---------------------------------------------------------------- encoding

fn push_escaped(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters must be escaped or the output is not JSON and
            // a strict receiver drops the whole datagram.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Serialise, refusing anything that would not fit in a datagram.
///
/// The cap is checked on the RESULT rather than estimated from the peer count,
/// because a long hostname makes a short list large and the receiving side
/// enforces the same number.
pub fn encode(a: &Announce) -> Result<Vec<u8>, Reject> {
    let mut s = String::with_capacity(256);
    s.push_str("{\"id\":");
    push_escaped(&mut s, &a.id);
    s.push_str(",\"host\":");
    push_escaped(&mut s, &a.host);
    s.push_str(&format!(",\"port\":{},\"ts\":{},\"peers\":[", a.port, a.ts));
    for (i, p) in a.peers.iter().take(MAX_GOSSIP_PEERS).enumerate() {
        if i > 0 { s.push(','); }
        s.push_str("{\"id\":");
        push_escaped(&mut s, &p.id);
        s.push_str(",\"host\":");
        push_escaped(&mut s, &p.host);
        s.push_str(&format!(",\"port\":{},\"ts\":{}}}", p.port, p.last_seen_ms));
    }
    s.push_str("]}");
    if s.len() > MAX_DATAGRAM_BYTES { return Err(Reject::TooLarge); }
    Ok(s.into_bytes())
}

/// Sign an announcement and emit the datagram peers actually see.
///
/// The signed bytes are exactly what [`encode`] produces — the message WITHOUT
/// `pk` and `sig` — and the two fields are appended after. That makes the
/// signing input a function of the parsed value rather than of where a field
/// happened to sit in the text, so a verifier re-derives it by re-encoding what
/// it parsed instead of trying to snip a suffix off the bytes it received.
///
/// It also means the whole message is covered, `peers` included. An unsigned
/// sample would let anyone rewrite who a node claims to know, and third-hand
/// entries are how a peer learns about a peer it has never heard broadcast.
///
/// `pk` and `sig` are ADDITIVE to Qt's schema. serde ignores unknown fields, so
/// a Qt receiver still reads these datagrams; it simply will not check them.
pub fn encode_signed(a: &Announce, secret: &[u8; 32], public: &[u8; 32])
    -> Result<Vec<u8>, Reject> {
    let body = encode(a)?;
    let sig = crypto_kit::sign(secret, &body);
    let mut s = String::from_utf8(body).map_err(|_| Reject::FieldTooLong)?;
    s.pop(); // the closing brace
    s.push_str(&format!(",\"pk\":\"{}\",\"sig\":\"{}\"}}", hex(public), hex(&sig)));
    if s.len() > MAX_DATAGRAM_BYTES { return Err(Reject::TooLarge); }
    Ok(s.into_bytes())
}

// ---------------------------------------------------------------- decoding

/// Just enough JSON. Numbers are unsigned integers because every number in this
/// schema is one; a float where a port belongs is malformed input, not a value
/// to round.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Str(String),
    Num(u64),
    Bool(bool),
    Null,
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            // Last wins on a duplicate key, matching what most parsers do, but
            // the duplicate is not an error: rejecting on it would be stricter
            // than the senders this has to interoperate with.
            Json::Obj(kv) => kv.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self { Some(s) } else { None }
    }
    fn as_num(&self) -> Option<u64> {
        if let Json::Num(n) = self { Some(*n) } else { None }
    }
}

struct P<'a> { b: &'a [u8], i: usize }

impl<'a> P<'a> {
    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) { self.i += 1; }
    }
    fn byte(&self) -> Option<u8> { self.b.get(self.i).copied() }
    fn eat(&mut self, c: u8) -> Option<()> {
        if self.byte() == Some(c) { self.i += 1; Some(()) } else { None }
    }
    fn lit(&mut self, s: &[u8]) -> Option<()> {
        if self.b[self.i..].starts_with(s) { self.i += s.len(); Some(()) } else { None }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH { return None; }
        self.ws();
        match self.byte()? {
            b'"' => self.string().map(Json::Str),
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b't' => { self.lit(b"true")?; Some(Json::Bool(true)) }
            b'f' => { self.lit(b"false")?; Some(Json::Bool(false)) }
            b'n' => { self.lit(b"null")?; Some(Json::Null) }
            b'0'..=b'9' => self.number(),
            // A leading '-' is not accepted: every number here is a port, a
            // count or a millisecond timestamp, and a negative one is either
            // malformed or an attempt to wrap an unsigned field.
            _ => None,
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        while matches!(self.byte(), Some(b'0'..=b'9')) { self.i += 1; }
        if self.i == start { return None; }
        // A run of digits longer than u64 is rejected rather than saturated:
        // saturating turns a hostile timestamp into u64::MAX, which reads as a
        // peer seen infinitely recently and never prunes.
        std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok().map(Json::Num)
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            // Capped as it is built, not after: the length field of a hostile
            // datagram must not be able to make us allocate first and check
            // later.
            if out.len() > MAX_FIELD_LEN { return None; }
            match self.byte()? {
                b'"' => { self.i += 1; return Some(out); }
                b'\\' => {
                    self.i += 1;
                    let e = self.byte()?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let h = self.b.get(self.i..self.i + 4)?;
                            self.i += 4;
                            let n = u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?;
                            // A lone surrogate (any \uXXXX in D800-DFFF on its
                            // own) makes char::from_u32 return None here,
                            // which the `?` propagates all the way out to
                            // decode() rejecting the WHOLE datagram, not just
                            // skipping this one character — verified by
                            // a_lone_surrogate_escape_rejects_the_whole_
                            // datagram_not_just_that_character, since the
                            // wording alone invites the opposite reading.
                            // Consecutive \u escapes are never recombined
                            // into a UTF-16 surrogate PAIR either, so even a
                            // well-formed encoding of an astral-plane
                            // character (an emoji, say) is rejected the same
                            // way — see a_well_formed_surrogate_pair_is_not_
                            // recombined_into_one_astral_character for why
                            // that's a documented limitation rather than a
                            // fix: this schema's only string fields are
                            // sc-IDs and hostnames, neither of which needs
                            // anything outside the BMP.
                            out.push(char::from_u32(n)?);
                        }
                        _ => return None,
                    }
                }
                // Raw control bytes are not legal in a JSON string and are the
                // usual shape of a truncated or injected datagram.
                c if c < 0x20 => return None,
                _ => {
                    // Multi-byte UTF-8 has to travel intact.
                    let rest = std::str::from_utf8(&self.b[self.i..]).ok()?;
                    let c = rest.chars().next()?;
                    out.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Json> {
        self.eat(b'[')?;
        let mut out = Vec::new();
        self.ws();
        if self.eat(b']').is_some() { return Some(Json::Arr(out)); }
        loop {
            // Bounded during the walk. An array of ten thousand entries must
            // not be built before being rejected for being too long.
            if out.len() > MAX_GOSSIP_PEERS * 2 { return None; }
            out.push(self.value(depth + 1)?);
            self.ws();
            match self.byte()? {
                b',' => { self.i += 1; }
                b']' => { self.i += 1; return Some(Json::Arr(out)); }
                _ => return None,
            }
        }
    }

    fn object(&mut self, depth: usize) -> Option<Json> {
        self.eat(b'{')?;
        let mut out: Vec<(String, Json)> = Vec::new();
        self.ws();
        if self.eat(b'}').is_some() { return Some(Json::Obj(out)); }
        loop {
            if out.len() > 32 { return None; }
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(b':')?;
            let v = self.value(depth + 1)?;
            out.push((k, v));
            self.ws();
            match self.byte()? {
                b',' => { self.i += 1; }
                b'}' => { self.i += 1; return Some(Json::Obj(out)); }
                _ => return None,
            }
        }
    }
}

fn parse_json(b: &[u8]) -> Option<Json> {
    let mut p = P { b, i: 0 };
    let v = p.value(0)?;
    p.ws();
    // Trailing bytes mean the datagram is not what it claims to be. Accepting
    // a valid prefix is how a parser gets used to smuggle a second message.
    if p.i != b.len() { return None; }
    Some(v)
}

/// Why a datagram could not be read.
///
/// Separate from [`Reject`], which is about whether a well-formed record is
/// ADMISSIBLE. Malformed and inadmissible are different events: one is a bad
/// actor or a bug, the other is normal operation (your own echo arrives every
/// tick).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    /// Bigger than we will read.
    TooLarge,
    /// Not JSON, or not the shape this schema requires.
    NotOurSchema,
    /// The sender's id is not an sc-ID.
    BadSenderId,
    /// Port zero is not a port, and Qt rejects it too.
    BadPort,
}

/// Read a datagram.
///
/// Invalid entries inside `peers` are dropped rather than failing the whole
/// message — Qt does the same, and one bad neighbour in a sample should not
/// cost the sender's own record, which is the part that matters.
pub fn decode(data: &[u8]) -> Result<Announce, Malformed> {
    if data.len() > MAX_DATAGRAM_BYTES { return Err(Malformed::TooLarge); }
    let v = parse_json(data).ok_or(Malformed::NotOurSchema)?;

    let id = v.get("id").and_then(Json::as_str).ok_or(Malformed::NotOurSchema)?.to_string();
    if !crate::identity::is_valid(&id) { return Err(Malformed::BadSenderId); }
    let host = v.get("host").and_then(Json::as_str).ok_or(Malformed::NotOurSchema)?.to_string();
    let port = v.get("port").and_then(Json::as_num).ok_or(Malformed::NotOurSchema)?;
    if port == 0 || port > u16::MAX as u64 { return Err(Malformed::BadPort); }
    let ts = v.get("ts").and_then(Json::as_num).ok_or(Malformed::NotOurSchema)?;

    // `peers` absent is a valid message: a node that knows nobody still
    // announces itself, and that is exactly the first node on a network.
    let mut peers = Vec::new();
    if let Some(Json::Arr(items)) = v.get("peers") {
        for it in items {
            let (Some(pid), Some(phost), Some(pport)) = (
                it.get("id").and_then(Json::as_str),
                it.get("host").and_then(Json::as_str),
                it.get("port").and_then(Json::as_num),
            ) else { continue };
            if !crate::identity::is_valid(pid) || phost.is_empty()
                || pport == 0 || pport > u16::MAX as u64 { continue; }
            peers.push(PeerRecord {
                id: pid.to_string(),
                host: phost.to_string(),
                port: pport as u16,
                last_seen_ms: it.get("ts").and_then(Json::as_num).unwrap_or(0),
                verified: false,
            });
            if peers.len() == MAX_GOSSIP_PEERS { break; }
        }
    }
    let mut a = Announce { id, host, port: port as u16, ts, peers,
                           public: None, verified: false };

    // Verification, and note the ORDER. The fingerprint check comes with the
    // signature check, not instead of it: a valid signature over a message
    // claiming someone else's sc-ID is exactly the impersonation the binding
    // exists to stop, and checking only the signature would wave it through.
    let (Some(pk_hex), Some(sig_hex)) = (
        v.get("pk").and_then(Json::as_str),
        v.get("sig").and_then(Json::as_str),
    ) else { return Ok(a) };
    let (Some(pk), Some(sig)) = (unhex::<32>(pk_hex), unhex::<64>(sig_hex))
        else { return Ok(a) };
    a.public = Some(pk);
    if crate::identity::fingerprint(&pk) != a.id { return Ok(a); }
    // Re-encode what we parsed to get the bytes that were signed. Sound only
    // because decode->encode round-trips exactly, which
    // `an_announcement_round_trips` pins; if that ever stops holding, every
    // signature stops verifying and this is why.
    let Ok(body) = encode(&a) else { return Ok(a) };
    a.verified = crypto_kit::verify(&pk, &body, &sig);
    Ok(a)
}

// ---------------------------------------------------------------- the socket

/// A non-blocking UDP socket for gossip.
///
/// No thread and no runtime. `poll` drains whatever has arrived and returns
/// immediately, so the caller drives it from wherever it already has a loop —
/// which for this app is the frame. That keeps the whole path visible in one
/// place and steppable in a debugger; a background thread would buy nothing
/// here except a channel to get the datagrams back out of.
pub struct GossipSocket {
    sock: std::net::UdpSocket,
    joined: bool,
    /// Reused across polls. A 4 KiB allocation per datagram, several times a
    /// second, for the life of the process is a lot of nothing.
    buf: Vec<u8>,
}

impl GossipSocket {
    /// Bind on all interfaces at `port` and join the discovery group.
    ///
    /// MULTICAST, not the limited broadcast this first used.
    /// `gossip::MULTICAST_GROUP` — 224.0.0.251 — has been sitting in the
    /// constants unused since the module was written, and it is the right
    /// answer: 255.255.255.255 is link-local-only, is dropped by plenty of
    /// consumer APs, and is delivered to every host on the segment whether or
    /// not it runs this app. Broadcast stays enabled because
    /// [`send_to`](Self::send_to) can still address one, but the group is what
    /// the app announces to.
    ///
    /// Joining is best effort. A machine with no multicast-capable interface up
    /// yet — a phone with wifi still associating — must still get a socket it
    /// can receive unicast replies on, so a failed join is reported rather than
    /// fatal.
    ///
    /// **Two instances on one host share the port.** SO_REUSEADDR and (on
    /// Linux) SO_REUSEPORT are set before the bind, so a second copy on the
    /// same machine binds too and, with multicast loopback left on, both
    /// hear each other's announces. That is the whole end-to-end walk —
    /// scan, add, send, open — on one desk without two phones. std's
    /// UdpSocket cannot set either option, hence socket2.
    pub fn bind(port: u16) -> std::io::Result<GossipSocket> {
        use socket2::{Domain, Protocol, Socket, Type};
        let raw = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        raw.set_reuse_address(true)?;
        #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
        raw.set_reuse_port(true)?;
        let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
        raw.bind(&addr.into())?;
        let sock: std::net::UdpSocket = raw.into();
        sock.set_nonblocking(true)?;
        sock.set_broadcast(true)?;
        let joined = crate::gossip::MULTICAST_GROUP.parse::<std::net::Ipv4Addr>().ok()
            .map(|g| sock.join_multicast_v4(&g, &std::net::Ipv4Addr::UNSPECIFIED).is_ok())
            .unwrap_or(false);
        Ok(GossipSocket { sock, buf: vec![0u8; MAX_DATAGRAM_BYTES], joined })
    }

    /// Did the multicast join succeed? False means this socket only sees
    /// unicast, which is worth surfacing: discovery will look broken and the
    /// cause is the interface, not the app.
    pub fn joined_group(&self) -> bool { self.joined }

    /// Where an announcement should go.
    pub fn group_addr(port: u16) -> String {
        format!("{}:{port}", crate::gossip::MULTICAST_GROUP)
    }

    /// The port actually bound. Differs from the requested one when 0 was
    /// asked for, which is how a test gets two of these talking.
    pub fn port(&self) -> std::io::Result<u16> { Ok(self.sock.local_addr()?.port()) }

    /// Everything that has arrived, with the address it came from.
    ///
    /// `limit` caps one drain. Without it a subnet under load — or one host
    /// deliberately flooding — keeps this loop running past the frame it was
    /// called from, and the UI stops drawing while the app services the
    /// network. Leftovers stay in the kernel buffer for the next call.
    ///
    /// A malformed datagram is returned rather than swallowed, because "sixty
    /// unreadable packets a second from 192.168.1.9" is a thing an operator
    /// needs to be able to see.
    pub fn poll(&mut self, limit: usize) -> Vec<(std::net::SocketAddr, Result<Announce, Malformed>)> {
        let mut out = Vec::new();
        for _ in 0..limit {
            match self.sock.recv_from(&mut self.buf) {
                Ok((n, from)) => out.push((from, decode(&self.buf[..n]))),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // Any other error ends this drain rather than the socket. On a
                // phone, an interface going away mid-poll is routine.
                Err(_) => break,
            }
        }
        out
    }

    /// Send one SIGNED announcement.
    ///
    /// The signing variant is the one the app uses; `send_to` stays for tests
    /// and for anything deliberately emitting the Qt-compatible unsigned form.
    /// Keeping both is not indecision: an unsigned announcement is refused by
    /// every correctly-configured receiver, so the choice has to be explicit at
    /// the call site rather than implied by whether a key happened to be around.
    pub fn send_signed_to(&self, a: &Announce, secret: &[u8; 32], public: &[u8; 32],
                          to: std::net::SocketAddr) -> std::io::Result<usize> {
        let bytes = encode_signed(a, secret, public).map_err(|_| std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "announcement exceeds the datagram cap"))?;
        self.sock.send_to(&bytes, to)
    }

    /// Send one unsigned announcement to one address.
    pub fn send_to(&self, a: &Announce, to: std::net::SocketAddr) -> std::io::Result<usize> {
        let bytes = encode(a).map_err(|_| std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "announcement exceeds the datagram cap"))?;
        self.sock.send_to(&bytes, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announce() -> Announce {
        Announce {
            id: "sc-32a51661".into(), host: "192.168.1.5".into(), port: 9876, ts: 1_700_000_000_000,
            peers: vec![PeerRecord {
                id: "sc-aabbccdd".into(), host: "192.168.1.6".into(), port: 9876,
                last_seen_ms: 1_699_999_999_000, verified: false,
            }],
            public: None, verified: false,
        }
    }

    /// An announcement whose id really is its key's fingerprint, which is the
    /// only kind that can verify.
    fn keyed() -> (Announce, crate::identity::Identity) {
        let id = crate::identity::create().expect("entropy");
        let mut a = announce();
        a.id = id.sc_id.clone();
        (a, id)
    }

    #[test]
    fn the_shape_matches_what_the_serde_crate_documents() {
        // The interoperability contract, pinned as literal text. snapclone-
        // gossip and GossipDiscovery.cpp emit these field names and types; a
        // rename here is a peer that can no longer see us, and nothing else
        // would notice until two machines were in the same room.
        let s = String::from_utf8(encode(&announce()).unwrap()).unwrap();
        assert!(s.starts_with("{\"id\":\"sc-32a51661\""), "{s}");
        assert!(s.contains("\"host\":\"192.168.1.5\""), "{s}");
        assert!(s.contains("\"port\":9876"), "{s}");
        assert!(s.contains("\"ts\":1700000000000"), "{s}");
        assert!(s.contains("\"peers\":[{\"id\":\"sc-aabbccdd\""), "{s}");
        assert!(s.ends_with("}]}"), "{s}");
    }

    #[test]
    fn an_announcement_round_trips() {
        assert_eq!(decode(&encode(&announce()).unwrap()).unwrap(), announce());
    }

    #[test]
    fn two_instances_on_one_machine_can_both_bind_the_port_and_hear_each_other() {
        // The one-desk walk. Before SO_REUSEADDR the second bind failed
        // with "address in use", so a send could only ever be tried
        // against a second machine. A high port that nothing else uses
        // here, and unicast to loopback so the test does not depend on
        // multicast routing in the sandbox.
        let port = 49_877;
        let a = GossipSocket::bind(port).expect("first bind");
        let b = GossipSocket::bind(port).expect("second bind on the same port");
        let mut b = b;
        let to: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
        a.send_to(&announce(), to).unwrap();
        let mut got = Vec::new();
        for _ in 0..100 {
            got = b.poll(8);
            if !got.is_empty() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // With both sockets on the port the kernel delivers a unicast
        // datagram to one of them; either way the bind succeeded, which
        // is the claim. Multicast (the real discovery path) fans out to
        // both, and loopback stays on for that.
        let a_heard = { let mut a = a; !a.poll(8).is_empty() };
        assert!(!got.is_empty() || a_heard, "the datagram reached neither socket");
        if let Some((_, r)) = got.first() {
            assert_eq!(r.as_ref().ok().map(|x| x.id.clone()), Some("sc-32a51661".to_string()));
        }
    }

    #[test]
    fn a_node_that_knows_nobody_still_announces_itself() {
        // The first node on a network. An absent or empty `peers` must not make
        // the message unreadable, or the second machine to start can never find
        // the first.
        let d = decode(br#"{"id":"sc-32a51661","host":"10.0.0.1","port":9876,"ts":1}"#).unwrap();
        assert!(d.peers.is_empty());
        assert_eq!(d.id, "sc-32a51661");
    }

    #[test]
    fn a_signed_announcement_verifies_and_is_admitted() {
        // The whole point. This is what `a_decoded_record_is_unsigned_and_
        // therefore_refused` was waiting for.
        let (a, id) = keyed();
        let bytes = encode_signed(&a, &id.secret, &id.public).unwrap();
        let got = decode(&bytes).unwrap();
        assert!(got.verified, "a signature we just made did not verify");
        assert_eq!(got.public, Some(id.public));
        assert_eq!(crate::gossip::accept(&got.sender_record(), bytes.len(), "sc-00000000"),
                   Ok(()));
    }

    #[test]
    fn a_signature_over_someone_elses_sc_id_is_refused() {
        // THE attack the fingerprint binding exists for, and the reason the
        // order in `decode` matters: this signature is perfectly valid. An
        // impersonator generates their own keypair, puts the victim's sc-ID in
        // the id field, and signs with theirs. Checking only the signature
        // waves it straight through.
        let (mut a, attacker) = keyed();
        a.id = "sc-deadbeef".into();
        let bytes = encode_signed(&a, &attacker.secret, &attacker.public).unwrap();
        let got = decode(&bytes).unwrap();
        assert!(!got.verified, "an impersonated sc-ID was admitted");
        assert!(crypto_kit::verify(&attacker.public, &encode(&got).unwrap(),
                                   &unhex::<64>(&hex(&crypto_kit::sign(
                                       &attacker.secret, &encode(&got).unwrap()))).unwrap()),
                "the signature itself really is valid — that is the point");
    }

    #[test]
    fn a_tampered_field_breaks_the_signature() {
        // Including inside `peers`, which is why the signed bytes are the whole
        // message rather than the header. An unsigned sample lets anyone
        // rewrite who a node claims to know, and third-hand entries are how a
        // peer learns about one it never heard broadcast.
        let (a, id) = keyed();
        let good = encode_signed(&a, &id.secret, &id.public).unwrap();
        assert!(decode(&good).unwrap().verified);

        for (from, to) in [("\"port\":9876", "\"port\":9999"),
                           ("sc-aabbccdd", "sc-11223344"),
                           ("192.168.1.6", "192.168.9.9")] {
            let s = String::from_utf8(good.clone()).unwrap().replacen(from, to, 1);
            assert!(!decode(s.as_bytes()).unwrap().verified,
                    "tampering with {from} still verified");
        }
    }

    #[test]
    fn a_garbage_signature_is_unverified_rather_than_unreadable() {
        // A malformed signature must not cost the whole datagram: the sender's
        // host and port are still useful, they are just not authenticated.
        // Failing the parse would let one bad byte deny a peer's discovery.
        let (a, id) = keyed();
        let good = String::from_utf8(encode_signed(&a, &id.secret, &id.public).unwrap()).unwrap();
        for bad in ["\"sig\":\"00\"", "\"sig\":\"zz\"", "\"pk\":\"beef\""] {
            let key = &bad[..bad.find(':').unwrap()];
            let start = good.find(key).unwrap();
            let end = good[start..].find(',').map(|i| start + i).unwrap_or(good.len() - 1);
            let s = format!("{}{}{}", &good[..start], bad, &good[end..]);
            let got = decode(s.as_bytes()).expect("a bad signature made the message unreadable");
            assert!(!got.verified, "{bad} verified");
        }
    }

    #[test]
    fn an_unsigned_qt_datagram_still_parses_and_is_simply_not_verified() {
        // Interop in the direction that matters: a Qt peer sends no pk or sig.
        // We must read it — host and port are real information — and must not
        // pretend it was authenticated.
        let d = decode(br#"{"id":"sc-32a51661","host":"10.0.0.1","port":9876,"ts":1}"#).unwrap();
        assert!(!d.verified);
        assert_eq!(d.public, None);
    }

    #[test]
    fn a_decoded_record_is_unsigned_and_therefore_refused() {
        // What remains of the gap. Signing landed, so a datagram from THIS app
        // now verifies — but an UNSIGNED one (a Qt peer, or an attacker who
        // simply omits the fields) is still refused by gossip::accept, and that
        // refusal is the security property, not a bug to route around.
        let rec = decode(&encode(&announce()).unwrap()).unwrap().sender_record();
        assert!(!rec.verified);
        assert_eq!(crate::gossip::accept(&rec, 200, "sc-00000000"),
                   Err(crate::gossip::Reject::BadSignature));
    }

    #[test]
    fn one_bad_neighbour_does_not_cost_the_senders_own_record() {
        // Qt drops invalid peer entries silently. The sender's record is the
        // part that matters — it is how they become reachable — and failing the
        // whole datagram over a garbled third-hand entry would let any peer
        // deny another peer's discovery.
        let d = decode(br#"{"id":"sc-32a51661","host":"h","port":1,"ts":1,"peers":[
            {"id":"not-an-id","host":"h","port":1,"ts":1},
            {"id":"sc-aabbccdd","host":"","port":1,"ts":1},
            {"id":"sc-11223344","host":"h","port":0,"ts":1},
            {"id":"sc-99887766","host":"h","port":7,"ts":1}]}"#).unwrap();
        assert_eq!(d.peers.len(), 1);
        assert_eq!(d.peers[0].id, "sc-99887766");
    }

    #[test]
    fn a_sender_without_a_valid_sc_id_is_refused_outright() {
        // Unlike a neighbour entry: the sender's id is the routing key for
        // everything that follows.
        assert_eq!(decode(br#"{"id":"nope","host":"h","port":1,"ts":1}"#),
                   Err(Malformed::BadSenderId));
        assert_eq!(decode(br#"{"id":"sc-32a51661","host":"h","port":0,"ts":1}"#),
                   Err(Malformed::BadPort));
        assert_eq!(decode(br#"{"id":"sc-32a51661","host":"h","port":70000,"ts":1}"#),
                   Err(Malformed::BadPort));
    }

    #[test]
    fn hostile_input_is_refused_rather_than_partially_believed() {
        // This parser IS the attack surface: gossip arrives from anyone on the
        // subnet. Each of these is a shape that a permissive reader accepts and
        // a strict one must not.
        let cases: &[(&[u8], &str)] = &[
            (b"", "empty"),
            (b"not json at all", "garbage"),
            (br#"{"id":"sc-32a51661","host":"h","port":1,"ts":1}trailing"#,
             "a valid prefix followed by a second message"),
            (br#"{"id":"sc-32a51661","host":"h","port":1,"ts":1"#, "truncated mid-object"),
            (br#"{"id":"sc-32a51661","host":"h","port":-1,"ts":1}"#, "a negative port"),
            (br#"{"id":"sc-32a51661","host":"h","port":1.5,"ts":1}"#, "a fractional port"),
            (br#"{"id":"sc-32a51661","host":"h","port":99999999999999999999,"ts":1}"#,
             "a number wider than u64"),
            (br#"[[[[[[[[1]]]]]]]]"#, "nesting past the depth limit"),
        ];
        for (bytes, why) in cases {
            assert!(decode(bytes).is_err(), "accepted {why}: {:?}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn an_oversized_datagram_is_refused_before_it_is_parsed() {
        // Both directions. The encoder refuses to produce one so we never
        // become the flooder, and the decoder refuses to read one.
        let big = vec![b'x'; MAX_DATAGRAM_BYTES + 1];
        assert_eq!(decode(&big), Err(Malformed::TooLarge));

        let mut a = announce();
        a.peers = (0..MAX_GOSSIP_PEERS).map(|i| PeerRecord {
            id: format!("sc-0000000{i}"), host: "x".repeat(MAX_FIELD_LEN), port: 9876,
            last_seen_ms: 1, verified: false }).collect();
        a.host = "y".repeat(MAX_FIELD_LEN);
        // Not asserting which side of the cap this lands on — asserting that
        // whichever it is, the two agree.
        match encode(&a) {
            Ok(bytes) => assert!(decode(&bytes).is_ok(), "we emitted what we will not read"),
            Err(Reject::TooLarge) => {}
            Err(e) => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn an_over_long_field_is_refused_while_it_is_being_read() {
        // Capped as the string is built, not after. A length field is
        // attacker-controlled here, so allocating first and checking later is
        // the whole bug.
        let d = format!(r#"{{"id":"sc-32a51661","host":"{}","port":1,"ts":1}}"#,
                        "h".repeat(MAX_FIELD_LEN + 10));
        assert!(decode(d.as_bytes()).is_err());
    }

    #[test]
    fn escapes_and_multibyte_survive_a_round_trip() {
        // A hostname will not usually contain a quote. A hostile sender's will.
        let mut a = announce();
        a.host = "a\"b\\c\td—é".into();
        let back = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(back.host, a.host);
        // and a raw control byte inside a string is not legal JSON
        assert!(decode(b"{\"id\":\"sc-32a51661\",\"host\":\"a\nb\",\"port\":1,\"ts\":1}").is_err());
    }

    #[test]
    fn a_lone_surrogate_escape_rejects_the_whole_datagram_not_just_that_character() {
        // `string()`'s own comment says a lone surrogate is "dropped rather
        // than substituted" -- true of the CHARACTER (char::from_u32 on a
        // standalone surrogate value returns None), but the `?` on that
        // call propagates all the way out of string() -> object() ->
        // parse_json() -> decode(), rejecting the ENTIRE announce, not
        // just the one malformed character. Locking in the actual
        // behavior rather than trusting the comment's wording, which
        // reads as "skips the character and keeps going."
        let with_lone_high_surrogate =
            b"{\"id\":\"sc-32a51661\",\"host\":\"a\\ud800b\",\"port\":1,\"ts\":1}";
        assert!(decode(with_lone_high_surrogate).is_err(),
            "a lone surrogate makes the whole datagram Malformed, per the parser's actual behavior");
    }

    #[test]
    fn a_well_formed_surrogate_pair_is_not_recombined_into_one_astral_character() {
        // This parser's own schema (id/host/port/ts/peers) has no free-text
        // field that would legitimately carry an emoji or other astral-
        // plane character -- sc-IDs and hostnames are the only strings,
        // and neither needs it. Documenting the actual limitation rather
        // than silently having it: a WELL-FORMED UTF-16 surrogate pair
        // (high surrogate immediately followed by its matching low
        // surrogate, together encoding one codepoint above the BMP) is
        // treated here as two INDEPENDENT \u escapes, and the first one
        // alone is a lone (invalid standalone) surrogate -- so a
        // technically-legal JSON encoding of, say, an emoji is rejected
        // as malformed rather than decoded. Real, but not reachable
        // through this schema's own fields, which is why this is
        // recorded as a documented limitation rather than fixed.
        let emoji_via_valid_surrogate_pair =
            b"{\"id\":\"sc-32a51661\",\"host\":\"\\ud83d\\ude00\",\"port\":1,\"ts\":1}";
        assert!(decode(emoji_via_valid_surrogate_pair).is_err());
    }

    #[test]
    fn two_sockets_on_loopback_actually_exchange_a_record() {
        // The end-to-end claim, and the reason this module is not another
        // tested-but-unreachable data structure: real sockets, real bytes, real
        // decode. Port 0 lets the OS pick, so the test does not fight anything
        // else on the machine.
        let mut rx = GossipSocket::bind(0).expect("bind receiver");
        let tx = GossipSocket::bind(0).expect("bind sender");
        let to = format!("127.0.0.1:{}", rx.port().unwrap()).parse().unwrap();
        tx.send_to(&announce(), to).expect("send");

        // UDP on loopback is effectively immediate but not synchronous.
        let mut got = Vec::new();
        for _ in 0..50 {
            got = rx.poll(8);
            if !got.is_empty() { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(got.len(), 1, "nothing arrived");
        let (from, decoded) = &got[0];
        assert_eq!(from.ip().to_string(), "127.0.0.1");
        assert_eq!(decoded.as_ref().unwrap(), &announce());
    }

    #[test]
    fn an_empty_host_is_filled_from_where_the_packet_came_from() {
        // A sender cannot know which of its interfaces a peer can reach, so it
        // announces nothing and the receiver supplies the answer it already
        // has. Without this the record's host is empty, parse_dotted_quad
        // declines it, and a peer that verified perfectly is still not
        // dialable — which is indistinguishable from discovery not working.
        let mut a = announce();
        a.host = String::new();
        let src: std::net::IpAddr = "10.0.0.7".parse().unwrap();
        assert_eq!(a.sender_record_from(src).host, "10.0.0.7");
    }

    #[test]
    fn a_declared_address_beats_the_source_address() {
        // A peer behind a router may legitimately know a better address for
        // itself than the one its packets appear to come from, so a stated
        // host is not overridden.
        let a = announce(); // host: 192.168.1.5
        let src: std::net::IpAddr = "10.0.0.7".parse().unwrap();
        assert_eq!(a.sender_record_from(src).host, "192.168.1.5");
    }

    #[test]
    fn the_group_address_is_the_constant_the_module_already_had() {
        // 224.0.0.251 sat unused in gossip.rs since the module was written
        // while this sent to 255.255.255.255, which is link-local-only and is
        // dropped by plenty of consumer APs.
        assert_eq!(GossipSocket::group_addr(9877), "224.0.0.251:9877");
        assert_eq!(crate::gossip::MULTICAST_GROUP, "224.0.0.251");
    }

    #[test]
    fn polling_an_idle_socket_returns_immediately_and_empty() {
        // Non-blocking is the whole reason this can live on the frame loop. If
        // this ever blocks, the UI stops drawing whenever the subnet is quiet —
        // which is most of the time.
        let mut s = GossipSocket::bind(0).expect("bind");
        let t = std::time::Instant::now();
        assert!(s.poll(16).is_empty());
        assert!(t.elapsed() < std::time::Duration::from_millis(100), "poll blocked");
    }

    #[test]
    fn a_flood_is_capped_at_the_limit_rather_than_draining_the_frame() {
        // One host deliberately flooding must not keep this loop running past
        // the frame that called it. The leftovers stay in the kernel buffer.
        let mut rx = GossipSocket::bind(0).expect("bind receiver");
        let tx = GossipSocket::bind(0).expect("bind sender");
        let to = format!("127.0.0.1:{}", rx.port().unwrap()).parse().unwrap();
        for _ in 0..20 { let _ = tx.send_to(&announce(), to); }
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(rx.poll(4).len() <= 4, "the cap was not honoured");
    }

    #[test]
    fn a_garbled_datagram_is_reported_rather_than_dropped_on_the_floor() {
        // "Sixty unreadable packets a second from 192.168.1.9" is a thing an
        // operator has to be able to see. Silently discarding them makes a
        // misconfigured or hostile host indistinguishable from a quiet network.
        let mut rx = GossipSocket::bind(0).expect("bind receiver");
        let tx = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind raw sender");
        let to = format!("127.0.0.1:{}", rx.port().unwrap());
        tx.send_to(b"definitely not json", &to).expect("send");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let got = rx.poll(4);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, Err(Malformed::NotOurSchema));
    }
}
