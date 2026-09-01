//! dht-kit — OpenDHT-compatible messaging, and nothing else.
//!
//! The fleet grew two Kademlia engines and one wire format, with the format
//! buried inside `apps/snapclone/rust/snapclone-dht` where no other crate could
//! reach it. Meanwhile `shared/snapcore` — the engine five other crates already
//! use — has no wire at all, so it cannot put a packet on a socket.
//!
//! This kit is the seam between those facts. It is **the protocol layer alone**:
//!
//! * **no transport** — no sockets, no async, no runtime. A caller hands in
//!   bytes and gets a message, or hands in a message and gets bytes.
//! * **no routing table, no engine, no storage.** Those differ between
//!   implementations and are not what interoperability is made of.
//! * **no dependencies.** A protocol crate that drags in serde forces a version
//!   choice on every consumer, and two engines with different toolchains is
//!   exactly the situation this is meant to serve.
//!
//! ## The format, from the reference rather than from memory
//!
//! The first version of this crate described a format that was *shaped like*
//! OpenDHT's and was not it. Read against `vendor/opendht/src/network_engine.cpp`
//! (the packers) and `parsed_message.h` (the parser), every message it produced
//! would have been rejected by a real node, and every message a real node sent
//! would have been rejected here — for several independent reasons at once:
//! a token packed as an integer where the reference reads a blob, `find`'s
//! target under the wrong key, nodes as an array of blobs instead of one blob,
//! values in a two-key map instead of the reference's `id`/`dat`/`body`
//! nesting, and a decoder that refused the `v` user-agent key present on
//! **every** message the reference sends. The empty lookup that results is
//! also what a healthy DHT returns for an offline peer, which is how it went
//! unnoticed.
//!
//! What follows is the format the reference actually packs. Every key name is
//! a constant from `parsed_message.h`; every layout is a `pk.pack(...)`
//! sequence in `network_engine.cpp`.
//!
//! ```text
//! envelope   {"a"|"r"|"e": ..., ["q": name], "t": tid, "y": "q"|"r"|"e"|"p",
//!             "v": user-agent, ["n": netid], ["s": is_client]}
//!
//! "a" (query args)
//!   ping     {"id"}
//!   find     {"id", "target", ["w": [2|10, ...]]}
//!   get      {"id", "h", ["q": filter], ["w"]}
//!   put      {"id", "h", "values": [Value...], ["c": created], "token": bin}
//!   listen   {"id", "ve": 1, "h", "token", "sid", ["q"]}
//!   refresh  {"id", "h", "vid": u64, "token"}
//!   update   {"id", "ve": 1, "h", "sid", "values", ["c"], "token"}   (listener push)
//!
//! "r" (reply)   {"id", "sa": bin4|bin16, ["n4": bin], ["n6": bin], ["token"],
//!                ["values": [Value|size...]], ["vid"]}
//! "e" (error)   ["e": [code, message], ["r": {"id"}]]
//!
//! Value          {"id": u64, "dat": {"body": {"type": u16, "data": bin,
//!                 ["utype"], ["seq","owner","to"]}, ["sig"]} | bin(cypher),
//!                 ["p"], ["pt"]}
//! n4 record      20-byte id ++ 4-byte ip ++ 2-byte port, big-endian, concatenated
//! n6 record      20-byte id ++ 16-byte ip ++ 2-byte port
//! "sa"           the REQUESTER's address as the responder saw it — ip only,
//!                no port. This is how a node behind NAT learns its public ip.
//! ```
//!
//! ## What is accepted versus what is produced
//!
//! Encoding mirrors the reference key for key and in the same order, so a
//! byte comparison against a capture is meaningful. Decoding is as tolerant
//! as the reference's parser: unknown keys at any level are skipped
//! structurally, a `token` may arrive as bin, str or array-of-u8 (the
//! reference's `unpackBlob` accepts all three), a transaction id may be an
//! integer or the four-byte blob older versions sent, a value that cannot be
//! read (encrypted, or signed by a scheme we do not implement) is dropped
//! from the list rather than failing the message, exactly as the reference
//! does per value.
//!
//! ## What this crate does NOT establish
//!
//! Round-tripping our own encoder proves self-consistency. The tests here that
//! are pinned to hand-assembled bytes are derived from reading the packer
//! source, which is stronger than a round trip and weaker than a capture.
//! The test that settles it is a real node on the other end of a socket —
//! `shared/snapcore/tests/opendht_interop.rs` runs one when `dhtnode` is on
//! `PATH`.

pub const HASH_LEN: usize = 20;

/// The user-agent string the reference sends under `v`.
///
/// `parsed_message.h` reads it into a field that nothing in `dht.cpp` or
/// `network_engine.cpp` branches on, so any string works. Sending the
/// reference's own makes our envelope byte-identical to its, which is worth
/// more here than a distinguishing name.
pub const USER_AGENT: &str = "o2";

/// `want` bits — which address families a reply should list nodes for.
pub const WANT4: u8 = 1;
pub const WANT6: u8 = 2;

/// The largest message that reliably crosses a path without fragmenting.
///
/// 1200 bytes is the conservative figure QUIC uses for its own initial packets
/// (RFC 9000 §14) and is safe on any IPv6 path, whose minimum MTU is 1280 with
/// 40 bytes of header and 8 of UDP on top. A DHT reply that exceeds it is not
/// an error — it will usually arrive — but it is the point where delivery
/// starts depending on the path rather than on us.
pub const SAFE_DATAGRAM: usize = 1200;

/// The reference's ceiling on one value's payload (`Value::MAX_VALUE_SIZE`).
/// A node refuses larger; we refuse to encode larger so the failure is local.
pub const MAX_VALUE_SIZE: usize = 64 * 1024;

/// How deeply an unknown value may nest before it is refused.
///
/// A bound rather than a guideline: recursion without one is a remote abort,
/// which no caller can catch. See `Rd::skip_depth`.
pub const MAX_NESTING: usize = 64;

/// A 160-bit id — OpenDHT's `InfoHash` width, used for both node ids and keys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct InfoHash(pub [u8; HASH_LEN]);

impl InfoHash {
    pub fn zero() -> InfoHash { InfoHash([0; HASH_LEN]) }

    pub fn is_zero(&self) -> bool { self.0.iter().all(|b| *b == 0) }

    /// Deterministic id from a seed, for tests and for deriving a key from an
    /// application string. Not a cryptographic hash and named so it cannot be
    /// mistaken for one.
    ///
    /// **`from_seed(2n) == from_seed(2n + 1)`**: the seed is OR'd with 1 so a
    /// zero seed does not produce a zero id. A test that wants distinct ids
    /// should use odd seeds — one already did not, and its "two peers" were
    /// one peer at two addresses.
    pub fn from_seed(seed: u64) -> InfoHash {
        let mut h = [0u8; HASH_LEN];
        let mut x = seed | 1;
        for b in h.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x >> 24) as u8;
        }
        InfoHash(h)
    }

    /// XOR distance — the Kademlia metric, big-endian so the array's natural
    /// ordering *is* distance ordering.
    pub fn distance(&self, other: &InfoHash) -> [u8; HASH_LEN] {
        let mut d = [0u8; HASH_LEN];
        for i in 0..HASH_LEN { d[i] = self.0[i] ^ other.0[i]; }
        d
    }
}

/// A stored value. `id` is OpenDHT's per-value identifier, which is what makes
/// a key hold a *set* rather than a single item — two writers under one key
/// coexist instead of overwriting. **Zero is `Value::INVALID_ID` in the
/// reference**: a put whose value has id 0 is acknowledged as "unknown
/// announce" and stored nowhere.
///
/// `vtype` is the reference's `ValueType::Id`; 0 is `USER_DATA`, which is what
/// an application value is. Carried so a value read from a real node comes
/// back with the type it had, and so the encoder writes the type the reference
/// requires (its parser throws on a body with no `type`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Value {
    pub id: u64,
    pub data: Vec<u8>,
    pub vtype: u16,
}

impl Value {
    pub fn new(id: u64, data: Vec<u8>) -> Value { Value { id, data, vtype: 0 } }
}

/// An ip without a port — what `sa` carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ip {
    V4([u8; 4]),
    V6([u8; 16]),
}

/// A peer address as the wire carries it — **both families**.
///
/// The reference keeps the families in separate reply keys — `n4` and `n6`
/// — rather than tagging each entry, so a v4-only implementation reads the
/// key it understands and ignores the other. That is why this enum encodes
/// into two blobs instead of one tagged list: a tag would make every old
/// client fail on a mixed reply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Addr {
    V4([u8; 4], u16),
    V6([u8; 16], u16),
}

/// Bytes on the wire for one node entry: 20-byte id plus the address.
pub const NODE_ENTRY_V4: usize = HASH_LEN + 6;
pub const NODE_ENTRY_V6: usize = HASH_LEN + 18;

impl Addr {
    /// Kept for callers written against the v4-only version.
    pub fn new(ip: [u8; 4], port: u16) -> Addr { Addr::V4(ip, port) }

    pub fn v6(ip: [u8; 16], port: u16) -> Addr { Addr::V6(ip, port) }

    pub fn port(&self) -> u16 {
        match self { Addr::V4(_, p) | Addr::V6(_, p) => *p }
    }

    pub fn is_v6(&self) -> bool { matches!(self, Addr::V6(..)) }

    pub fn ip(&self) -> Ip {
        match self { Addr::V4(ip, _) => Ip::V4(*ip), Addr::V6(ip, _) => Ip::V6(*ip) }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Addr::V4(ip, p) => { b.extend_from_slice(ip); b.extend_from_slice(&p.to_be_bytes()); }
            Addr::V6(ip, p) => { b.extend_from_slice(ip); b.extend_from_slice(&p.to_be_bytes()); }
        }
        b
    }

    /// Length decides the family — 6 bytes is v4, 18 is v6. No tag byte,
    /// because the reference does not send one and inventing one here would
    /// make our replies unreadable to it.
    pub fn from_bytes(b: &[u8]) -> Option<Addr> {
        match b.len() {
            6 => Some(Addr::V4([b[0], b[1], b[2], b[3]],
                               u16::from_be_bytes([b[4], b[5]]))),
            18 => {
                let mut ip = [0u8; 16];
                ip.copy_from_slice(&b[..16]);
                Some(Addr::V6(ip, u16::from_be_bytes([b[16], b[17]])))
            }
            _ => None,
        }
    }
}

/// The query names the reference sends. `Update` is the listener push: a
/// node that accepted a `listen` sends `update` queries carrying new values,
/// and the listener acknowledges each like any other query.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Query { Ping, Find, Get, Put, Listen, Refresh, Update }

impl Query {
    pub fn name(self) -> &'static str {
        match self {
            Query::Ping => "ping",
            Query::Find => "find",
            Query::Get => "get",
            Query::Put => "put",
            Query::Listen => "listen",
            Query::Refresh => "refresh",
            Query::Update => "update",
        }
    }

    pub fn from_name(s: &str) -> Option<Query> {
        Some(match s {
            "ping" => Query::Ping,
            "find" => Query::Find,
            "get" => Query::Get,
            "put" => Query::Put,
            "listen" => Query::Listen,
            "refresh" => Query::Refresh,
            "update" => Query::Update,
            _ => return None,
        })
    }
}

/// One protocol message.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Msg {
    Query {
        tid: u32,
        from: InfoHash,
        query: Query,
        /// `target` on the wire for `find`, `h` for everything else. Zero for
        /// a ping, which carries neither.
        target: InfoHash,
        /// Empty means none, and none is omitted from the wire — the
        /// reference only packs `token` on put/listen/refresh/update.
        token: Vec<u8>,
        /// put and update. Others carry none.
        values: Vec<Value>,
        /// `WANT4 | WANT6`; zero omits the key, which the reference reads as
        /// "the family I came from".
        want: u8,
        /// listen and update: the listener-side socket id the pushes are
        /// addressed to.
        sid: Option<u32>,
        /// refresh: which value to keep alive.
        vid: Option<u64>,
        /// `s: true` — we are a client node: answer us, do not route to us.
        /// The right flag for anything behind a NAT that never answers a
        /// query, which is every phone.
        client: bool,
    },
    Reply {
        tid: u32,
        from: InfoHash,
        /// Our own address as the responder saw it. Port is never carried.
        sa: Option<Ip>,
        /// Both families in one list; `Addr` says which. Encoded as `n4`
        /// and `n6`.
        nodes: Vec<(InfoHash, Addr)>,
        values: Vec<Value>,
        token: Vec<u8>,
        /// A put or refresh acknowledgement names the value it accepted.
        vid: Option<u64>,
    },
    /// Fragments of a value too large for one datagram (`y: "p"`). Carried
    /// so a decoder sees it for what it is; reassembly is not implemented,
    /// which is a stated limit rather than a parse failure.
    Parts {
        tid: u32,
        /// (value index, byte offset, bytes)
        fragments: Vec<(u32, u64, Vec<u8>)>,
    },
    Error {
        tid: u32,
        code: u16,
        from: Option<InfoHash>,
    },
}

/// Reference error codes (`DhtProtocolException`).
pub const ERR_NON_AUTHORITATIVE: u16 = 201;
pub const ERR_UNAUTHORIZED: u16 = 401;
pub const ERR_NOT_FOUND: u16 = 404;
pub const ERR_UNKNOWN_TID: u16 = 500;
pub const ERR_TRUNCATED: u16 = 501;

// ── msgpack ──────────────────────────────────────────────────────────────────
//
// The writer is a subset — the cases these maps need. The reader's *skip* is
// complete over the msgpack format table, because an unknown key may carry
// any value at all and the reader must step over it without guessing.

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    if b.len() < 32 { out.push(0xA0 | b.len() as u8); }
    else if b.len() <= 0xFF { out.push(0xD9); out.push(b.len() as u8); }
    else { out.push(0xDA); out.extend_from_slice(&(b.len() as u16).to_be_bytes()); }
    out.extend_from_slice(b);
}

fn put_bin(out: &mut Vec<u8>, b: &[u8]) {
    if b.len() <= 0xFF { out.push(0xC4); out.push(b.len() as u8); }
    else if b.len() <= 0xFFFF { out.push(0xC5); out.extend_from_slice(&(b.len() as u16).to_be_bytes()); }
    else { out.push(0xC6); out.extend_from_slice(&(b.len() as u32).to_be_bytes()); }
    out.extend_from_slice(b);
}

fn put_uint(out: &mut Vec<u8>, v: u64) {
    if v < 128 { out.push(v as u8); }
    else if v <= 0xFF { out.push(0xCC); out.push(v as u8); }
    else if v <= 0xFFFF { out.push(0xCD); out.extend_from_slice(&(v as u16).to_be_bytes()); }
    else if v <= 0xFFFF_FFFF { out.push(0xCE); out.extend_from_slice(&(v as u32).to_be_bytes()); }
    else { out.push(0xCF); out.extend_from_slice(&v.to_be_bytes()); }
}

fn put_bool(out: &mut Vec<u8>, v: bool) { out.push(if v { 0xC3 } else { 0xC2 }); }

fn put_map(out: &mut Vec<u8>, n: usize) {
    if n < 16 { out.push(0x80 | n as u8); }
    else { out.push(0xDE); out.extend_from_slice(&(n as u16).to_be_bytes()); }
}

fn put_arr(out: &mut Vec<u8>, n: usize) {
    if n < 16 { out.push(0x90 | n as u8); }
    else { out.push(0xDC); out.extend_from_slice(&(n as u16).to_be_bytes()); }
}

struct Rd<'a> { b: &'a [u8], i: usize }

impl<'a> Rd<'a> {
    fn peek(&self) -> Option<u8> { self.b.get(self.i).copied() }
    fn byte(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        // The single most important line in the decoder: a length prefix that
        // promises more than arrived must fail here, not produce a short slice.
        if self.i + n > self.b.len() { return None; }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Some(s)
    }
    fn be16(&mut self) -> Option<usize> { Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize) }
    fn be32(&mut self) -> Option<usize> { Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?) as usize) }

    /// A non-negative integer. The reference packs every integer we read
    /// as the positive family; a negative one here is a foreign message.
    fn uint(&mut self) -> Option<u64> {
        let t = self.byte()?;
        Some(match t {
            0x00..=0x7F => t as u64,
            0xCC => self.byte()? as u64,
            0xCD => self.be16()? as u64,
            0xCE => self.be32()? as u64,
            0xCF => u64::from_be_bytes(self.take(8)?.try_into().ok()?),
            // int8..int64 with a non-negative value: msgpack-c never emits
            // these for a positive number, but another packer might.
            0xD0 => { let v = self.byte()? as i8; if v < 0 { return None } v as u64 }
            0xD1 => { let v = i16::from_be_bytes(self.take(2)?.try_into().ok()?); if v < 0 { return None } v as u64 }
            0xD2 => { let v = i32::from_be_bytes(self.take(4)?.try_into().ok()?); if v < 0 { return None } v as u64 }
            0xD3 => { let v = i64::from_be_bytes(self.take(8)?.try_into().ok()?); if v < 0 { return None } v as u64 }
            _ => return None,
        })
    }
    fn bool(&mut self) -> Option<bool> {
        match self.byte()? { 0xC2 => Some(false), 0xC3 => Some(true), _ => None }
    }
    fn str(&mut self) -> Option<String> {
        let t = self.byte()?;
        let n = match t {
            0xA0..=0xBF => (t & 0x1F) as usize,
            0xD9 => self.byte()? as usize,
            0xDA => self.be16()?,
            0xDB => self.be32()?,
            _ => return None,
        };
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn bin(&mut self) -> Option<Vec<u8>> {
        let t = self.byte()?;
        let n = match t {
            0xC4 => self.byte()? as usize,
            0xC5 => self.be16()?,
            0xC6 => self.be32()?,
            _ => return None,
        };
        Some(self.take(n)?.to_vec())
    }
    /// The reference's `unpackBlob`: bin, str, or an array of small ints.
    fn blob(&mut self) -> Option<Vec<u8>> {
        match self.peek()? {
            0xC4..=0xC6 => self.bin(),
            0xA0..=0xBF | 0xD9..=0xDB => Some(self.str()?.into_bytes()),
            0x90..=0x9F | 0xDC | 0xDD => {
                let n = self.arr_len()?;
                // A declared count is not a byte count — bound it by what is
                // actually in the packet, or a `[]` header claiming a billion
                // elements is a billion-element allocation.
                if n > self.b.len() - self.i { return None; }
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    let v = self.uint()?;
                    if v > 0xFF { return None; }
                    out.push(v as u8);
                }
                Some(out)
            }
            _ => None,
        }
    }
    fn map_len(&mut self) -> Option<usize> {
        let t = self.byte()?;
        match t {
            0x80..=0x8F => Some((t & 0x0F) as usize),
            0xDE => self.be16(),
            0xDF => self.be32(),
            _ => None,
        }
    }
    fn arr_len(&mut self) -> Option<usize> {
        let t = self.byte()?;
        match t {
            0x90..=0x9F => Some((t & 0x0F) as usize),
            0xDC => self.be16(),
            0xDD => self.be32(),
            _ => None,
        }
    }
    /// Skip one value of any msgpack type.
    ///
    /// Needed because the reference's maps carry keys we do not implement —
    /// `n` (network id), `q` (a value filter), `c` (creation time), `p`,
    /// `pt`, `sig`, `owner` — and whatever a later version adds. Refusing
    /// unknown keys sounds strict and means the first real message a node
    /// sends is dropped whole. Strictness against a protocol you do not
    /// control is not safety, it is a guarantee of eventual incompatibility.
    ///
    /// Skipping is only safe because it is *structural*: the value is parsed
    /// well enough to know its length and then discarded, so a malformed one
    /// still fails rather than desynchronising the reader.
    fn skip(&mut self) -> Option<()> { self.skip_depth(0) }

    /// Depth-bounded skip.
    ///
    /// Found by a hostile-input probe rather than by reading: a 100KB packet
    /// containing nothing but 100,000 nested `fixarray(1)` headers **aborted
    /// the process with a stack overflow**. One unauthenticated datagram, and
    /// `Option` cannot express it — a caller cannot catch an abort, so this is
    /// a remote denial of service, not a parse failure.
    fn skip_depth(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_NESTING { return None; }
        let t = self.byte()?;
        match t {
            0x00..=0x7F | 0xE0..=0xFF | 0xC0 | 0xC2 | 0xC3 => {}
            0xCC | 0xD0 | 0xD4 => { self.take(1)?; }
            0xCD | 0xD1 | 0xD5 => { self.take(2)?; }
            0xD6 => { self.take(3)?; }
            0xCE | 0xD2 | 0xCA => { self.take(4)?; }
            0xD7 => { self.take(5)?; }
            0xCF | 0xD3 | 0xCB => { self.take(8)?; }
            0xD8 => { self.take(9)?; }
            0xA0..=0xBF => { let n = (t & 0x1F) as usize; self.take(n)?; }
            0xD9 | 0xC4 => { let n = self.byte()? as usize; self.take(n)?; }
            0xDA | 0xC5 => { let n = self.be16()?; self.take(n)?; }
            0xDB | 0xC6 => { let n = self.be32()?; self.take(n)?; }
            0xC7 => { let n = self.byte()? as usize; self.take(1 + n)?; }
            0xC8 => { let n = self.be16()?; self.take(1 + n)?; }
            0xC9 => { let n = self.be32()?; self.take(1 + n)?; }
            0x90..=0x9F | 0xDC | 0xDD => {
                self.i -= 1;
                let n = self.arr_len()?;
                for _ in 0..n { self.skip_depth(depth + 1)?; }
            }
            0x80..=0x8F | 0xDE | 0xDF => {
                self.i -= 1;
                let n = self.map_len()?;
                for _ in 0..n { self.skip_depth(depth + 1)?; self.skip_depth(depth + 1)?; }
            }
            0xC1 => return None,
        }
        Some(())
    }

    fn hash(&mut self) -> Option<InfoHash> {
        // The reference's `Hash::msgpack_unpack`: bin of exactly N, nothing
        // else. A str here is not a hash.
        let b = self.bin()?;
        if b.len() != HASH_LEN { return None; }
        let mut h = [0u8; HASH_LEN];
        h.copy_from_slice(&b);
        Some(InfoHash(h))
    }

    /// `unpackTid`: an integer, or the four-byte blob older versions sent
    /// (read big-endian, as `ntohl` does).
    fn tid(&mut self) -> Option<u32> {
        match self.peek()? {
            0xC4..=0xC6 | 0xA0..=0xBF | 0xD9..=0xDB => {
                let b = self.blob()?;
                if b.len() != 4 { return None; }
                Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            }
            _ => Some(u32::try_from(self.uint()?).ok()?),
        }
    }
}

fn put_ip(out: &mut Vec<u8>, ip: &Ip) {
    match ip {
        Ip::V4(b) => put_bin(out, b),
        Ip::V6(b) => put_bin(out, b),
    }
}

/// `Value::msgpack_pack` for an unsigned, unencrypted value:
/// `{"id": id, "dat": {"body": {"type": type, "data": data}}}`.
fn put_value(out: &mut Vec<u8>, v: &Value) {
    put_map(out, 2);
    put_str(out, "id"); put_uint(out, v.id);
    put_str(out, "dat");
    put_map(out, 1);
    put_str(out, "body");
    put_map(out, 2);
    put_str(out, "type"); put_uint(out, v.vtype as u64);
    put_str(out, "data"); put_bin(out, &v.data);
}

/// `Value::msgpack_unpack`. `Ok(None)` is a value the reference would
/// accept but we cannot use — encrypted, or a body we could not read — and
/// the caller drops it from the list the way the reference's per-value
/// `try` does. `Err` is a malformed packet.
fn read_value(r: &mut Rd) -> Result<Option<Value>, ()> {
    let n = r.map_len().ok_or(())?;
    let mut id = None;
    let mut body: Option<Option<(u16, Vec<u8>)>> = None;
    for _ in 0..n {
        match r.str().ok_or(())?.as_str() {
            "id" => id = Some(r.uint().ok_or(())?),
            "dat" => body = Some(read_dat(r)?),
            _ => r.skip().ok_or(())?,
        }
    }
    let id = id.ok_or(())?;
    match body {
        Some(Some((vtype, data))) => Ok(Some(Value { id, data, vtype })),
        // Encrypted, or signed in a way we do not read.
        Some(None) => Ok(None),
        // The reference throws on a value with no `dat`.
        None => Err(()),
    }
}

/// `Value::msgpack_unpack_body`: a bin is a cypher (unreadable here); a map
/// carries `body` and maybe `sig`.
fn read_dat(r: &mut Rd) -> Result<Option<(u16, Vec<u8>)>, ()> {
    if matches!(r.peek(), Some(0xC4..=0xC6)) {
        r.bin().ok_or(())?;
        return Ok(None);
    }
    let n = r.map_len().ok_or(())?;
    let mut out = None;
    for _ in 0..n {
        match r.str().ok_or(())?.as_str() {
            "body" => {
                let m = r.map_len().ok_or(())?;
                let mut data = None;
                let mut vtype = None;
                for _ in 0..m {
                    match r.str().ok_or(())?.as_str() {
                        "data" => data = Some(r.blob().ok_or(())?),
                        "type" => vtype = Some(u16::try_from(r.uint().ok_or(())?).map_err(|_| ())?),
                        _ => r.skip().ok_or(())?,
                    }
                }
                // The reference requires both.
                out = Some((vtype.ok_or(())?, data.ok_or(())?));
            }
            _ => r.skip().ok_or(())?,
        }
    }
    Ok(out)
}

fn read_nodes(blob: &[u8], entry: usize, out: &mut Vec<(InfoHash, Addr)>) -> Option<()> {
    // The reference floors (`size / NODE4_INFO_BUF_LEN`) and ignores a
    // ragged tail. A blob whose length is not a multiple of the record was
    // not produced by a compliant encoder, and flooring a mis-framed blob
    // attributes ids to the wrong addresses — refuse instead.
    if blob.len() % entry != 0 { return None; }
    for rec in blob.chunks_exact(entry) {
        let mut h = [0u8; HASH_LEN];
        h.copy_from_slice(&rec[..HASH_LEN]);
        out.push((InfoHash(h), Addr::from_bytes(&rec[HASH_LEN..])?));
    }
    Some(())
}

/// Linux `AF_INET6`. macOS packs 30 and Windows 23 into the same array —
/// the reference literally packs the platform constant — so the decoder
/// accepts all three.
const AF_INET: u64 = 2;
const AF_INET6_LINUX: u64 = 10;

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(128);
        match self {
            Msg::Query { tid, from, query, target, token, values, want, sid, vid, client } => {
                put_map(&mut o, 5 + *client as usize);
                put_str(&mut o, "a");
                encode_args(&mut o, *query, from, target, token, values, *want, *sid, *vid);
                put_str(&mut o, "q"); put_str(&mut o, query.name());
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "q");
                put_str(&mut o, "v"); put_str(&mut o, USER_AGENT);
                if *client { put_str(&mut o, "s"); put_bool(&mut o, true); }
            }
            Msg::Reply { tid, from, sa, nodes, values, token, vid } => {
                put_map(&mut o, 4);
                put_str(&mut o, "r");
                let v4: Vec<_> = nodes.iter().filter(|(_, a)| !a.is_v6()).collect();
                let v6: Vec<_> = nodes.iter().filter(|(_, a)| a.is_v6()).collect();
                let n = 1 + sa.is_some() as usize + (!v4.is_empty()) as usize
                    + (!v6.is_empty()) as usize + (!token.is_empty()) as usize
                    + (!values.is_empty()) as usize + vid.is_some() as usize;
                put_map(&mut o, n);
                put_str(&mut o, "id"); put_bin(&mut o, &from.0);
                if let Some(vid) = vid { put_str(&mut o, "vid"); put_uint(&mut o, *vid); }
                if let Some(sa) = sa { put_str(&mut o, "sa"); put_ip(&mut o, sa); }
                if !v4.is_empty() {
                    let mut blob = Vec::with_capacity(v4.len() * NODE_ENTRY_V4);
                    for (h, a) in &v4 { blob.extend_from_slice(&h.0); blob.extend_from_slice(&a.to_bytes()); }
                    put_str(&mut o, "n4"); put_bin(&mut o, &blob);
                }
                if !v6.is_empty() {
                    let mut blob = Vec::with_capacity(v6.len() * NODE_ENTRY_V6);
                    for (h, a) in &v6 { blob.extend_from_slice(&h.0); blob.extend_from_slice(&a.to_bytes()); }
                    put_str(&mut o, "n6"); put_bin(&mut o, &blob);
                }
                if !token.is_empty() { put_str(&mut o, "token"); put_bin(&mut o, token); }
                if !values.is_empty() {
                    put_str(&mut o, "values");
                    put_arr(&mut o, values.len());
                    for v in values { put_value(&mut o, v); }
                }
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "r");
                put_str(&mut o, "v"); put_str(&mut o, USER_AGENT);
            }
            Msg::Parts { tid, fragments } => {
                put_map(&mut o, 4);
                put_str(&mut o, "y"); put_str(&mut o, "p");
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "p");
                put_map(&mut o, fragments.len());
                for (idx, off, data) in fragments {
                    put_uint(&mut o, *idx as u64);
                    put_map(&mut o, 2);
                    put_str(&mut o, "o"); put_uint(&mut o, *off);
                    put_str(&mut o, "d"); put_bin(&mut o, data);
                }
                put_str(&mut o, "v"); put_str(&mut o, USER_AGENT);
            }
            Msg::Error { tid, code, from } => {
                put_map(&mut o, 4 + from.is_some() as usize);
                put_str(&mut o, "e");
                put_arr(&mut o, 2);
                put_uint(&mut o, *code as u64);
                put_str(&mut o, error_text(*code));
                if let Some(id) = from {
                    put_str(&mut o, "r");
                    put_map(&mut o, 1);
                    put_str(&mut o, "id"); put_bin(&mut o, &id.0);
                }
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "e");
                put_str(&mut o, "v"); put_str(&mut o, USER_AGENT);
            }
        }
        o
    }

    pub fn decode(bytes: &[u8]) -> Option<Msg> {
        let mut r = Rd { b: bytes, i: 0 };
        let n = r.map_len()?;

        let mut tid = None;
        let mut kind: Option<String> = None;
        let mut qname: Option<String> = None;
        let mut client = false;
        // Which top-level payload key was present. The reference's precedence
        // is e > r > p > u > (y == "q" and q names the query).
        let mut has_e = false;
        let mut has_r = false;
        let mut has_p = false;
        let mut has_u = false;
        let mut has_a = false;
        let mut code = None;

        let mut from = None;
        let mut target = None;
        let mut token = Vec::new();
        let mut values = Vec::new();
        let mut nodes = Vec::new();
        let mut sa = None;
        let mut want = 0u8;
        let mut sid = None;
        let mut vid = None;
        let mut fragments = Vec::new();

        for _ in 0..n {
            // A non-string key is skipped whole, as the reference does.
            if !matches!(r.peek()?, 0xA0..=0xBF | 0xD9..=0xDB) {
                r.skip()?; r.skip()?; continue;
            }
            match r.str()?.as_str() {
                "t" => tid = Some(r.tid()?),
                "y" => kind = Some(r.str()?),
                "q" => qname = Some(r.str()?),
                "s" => client = r.bool()?,
                "e" => {
                    has_e = true;
                    let k = r.arr_len()?;
                    if k == 0 { return None; }
                    code = Some(u16::try_from(r.uint()?).ok()?);
                    for _ in 1..k { r.skip()?; }
                }
                "p" => {
                    has_p = true;
                    let m = r.map_len()?;
                    for _ in 0..m {
                        let idx = u32::try_from(r.uint()?).ok()?;
                        let k = r.map_len()?;
                        let mut off = None;
                        let mut data = None;
                        for _ in 0..k {
                            match r.str()?.as_str() {
                                "o" => off = Some(r.uint()?),
                                "d" => data = Some(r.blob()?),
                                _ => r.skip()?,
                            }
                        }
                        if let (Some(o), Some(d)) = (off, data) { fragments.push((idx, o, d)); }
                    }
                }
                k @ ("a" | "r" | "u") => {
                    match k { "a" => has_a = true, "r" => has_r = true, _ => has_u = true }
                    let m = r.map_len()?;
                    for _ in 0..m {
                        if !matches!(r.peek()?, 0xA0..=0xBF | 0xD9..=0xDB) {
                            r.skip()?; r.skip()?; continue;
                        }
                        match r.str()?.as_str() {
                            "id" => from = Some(r.hash()?),
                            "h" | "target" => target = Some(r.hash()?),
                            "token" => token = r.blob()?,
                            "sid" => sid = Some(r.tid()?),
                            "vid" => vid = Some(r.uint()?),
                            "sa" => {
                                let b = r.bin()?;
                                sa = match b.len() {
                                    4 => Some(Ip::V4([b[0], b[1], b[2], b[3]])),
                                    16 => { let mut ip = [0u8; 16]; ip.copy_from_slice(&b); Some(Ip::V6(ip)) }
                                    // The reference leaves the address unset
                                    // for any other width rather than failing.
                                    _ => None,
                                };
                            }
                            "n4" => read_nodes(&r.blob()?, NODE_ENTRY_V4, &mut nodes)?,
                            "n6" => read_nodes(&r.blob()?, NODE_ENTRY_V6, &mut nodes)?,
                            "values" => {
                                let k = r.arr_len()?;
                                if k > r.b.len() - r.i { return None; }
                                for _ in 0..k {
                                    // An integer here is the size of a value
                                    // arriving separately as parts. Noted by
                                    // skipping: reassembly is not implemented.
                                    if matches!(r.peek()?, 0x00..=0x7F | 0xCC..=0xCF) {
                                        r.uint()?;
                                        continue;
                                    }
                                    match read_value(&mut r) {
                                        Ok(Some(v)) => values.push(v),
                                        Ok(None) => {}
                                        Err(()) => return None,
                                    }
                                }
                            }
                            "w" => {
                                let k = r.arr_len()?;
                                for _ in 0..k {
                                    match r.uint()? {
                                        AF_INET => want |= WANT4,
                                        AF_INET6_LINUX | 23 | 30 => want |= WANT6,
                                        _ => {}
                                    }
                                }
                            }
                            _ => r.skip()?,
                        }
                    }
                }
                _ => r.skip()?,
            }
        }

        let tid = tid?;
        if has_e {
            return Some(Msg::Error { tid, code: code?, from });
        }
        if has_r {
            return Some(Msg::Reply { tid, from: from?, sa, nodes, values, token, vid });
        }
        if has_p {
            return Some(Msg::Parts { tid, fragments });
        }
        // A version-0 listener push: `u` args, `y: "r"`, no query name.
        let query = if has_u {
            Query::Update
        } else {
            if let Some(y) = &kind { if y != "q" { return None; } }
            Query::from_name(&qname?)?
        };
        if !has_a && !has_u { return None; }
        Some(Msg::Query {
            tid,
            from: from?,
            query,
            target: target.unwrap_or_default(),
            token,
            values,
            want,
            sid,
            vid,
            client,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_args(o: &mut Vec<u8>, query: Query, from: &InfoHash, target: &InfoHash,
               token: &[u8], values: &[Value], want: u8, sid: Option<u32>, vid: Option<u64>) {
    let put_want = |o: &mut Vec<u8>| {
        put_str(o, "w");
        put_arr(o, (want & WANT4 != 0) as usize + (want & WANT6 != 0) as usize);
        if want & WANT4 != 0 { put_uint(o, AF_INET); }
        if want & WANT6 != 0 { put_uint(o, AF_INET6_LINUX); }
    };
    let put_values = |o: &mut Vec<u8>| {
        put_str(o, "values");
        put_arr(o, values.len());
        for v in values { put_value(o, v); }
    };
    let put_token = |o: &mut Vec<u8>| { put_str(o, "token"); put_bin(o, token); };
    match query {
        Query::Ping => {
            put_map(o, 1);
            put_str(o, "id"); put_bin(o, &from.0);
        }
        Query::Find => {
            put_map(o, 2 + (want != 0) as usize);
            put_str(o, "id"); put_bin(o, &from.0);
            put_str(o, "target"); put_bin(o, &target.0);
            if want != 0 { put_want(o); }
        }
        Query::Get => {
            put_map(o, 2 + (want != 0) as usize);
            put_str(o, "id"); put_bin(o, &from.0);
            put_str(o, "h"); put_bin(o, &target.0);
            if want != 0 { put_want(o); }
        }
        Query::Put => {
            // The reference always packs `token` on a put, empty or not.
            put_map(o, 4);
            put_str(o, "id"); put_bin(o, &from.0);
            put_str(o, "h"); put_bin(o, &target.0);
            put_values(o);
            put_token(o);
        }
        Query::Listen => {
            put_map(o, 5);
            put_str(o, "id"); put_bin(o, &from.0);
            put_str(o, "ve"); put_uint(o, 1);
            put_str(o, "h"); put_bin(o, &target.0);
            put_token(o);
            put_str(o, "sid"); put_uint(o, sid.unwrap_or(0) as u64);
        }
        Query::Refresh => {
            put_map(o, 4);
            put_str(o, "id"); put_bin(o, &from.0);
            put_str(o, "h"); put_bin(o, &target.0);
            put_str(o, "vid"); put_uint(o, vid.unwrap_or(0));
            put_token(o);
        }
        Query::Update => {
            put_map(o, 6);
            put_str(o, "id"); put_bin(o, &from.0);
            put_str(o, "ve"); put_uint(o, 1);
            put_str(o, "h"); put_bin(o, &target.0);
            put_str(o, "sid"); put_uint(o, sid.unwrap_or(0) as u64);
            put_values(o);
            put_token(o);
        }
    }
}

fn error_text(code: u16) -> &'static str {
    match code {
        ERR_NON_AUTHORITATIVE => "Non-authoritative",
        ERR_UNAUTHORIZED => "Unauthorized",
        ERR_NOT_FOUND => "Not found",
        ERR_UNKNOWN_TID => "Unknown transaction id",
        ERR_TRUNCATED => "Truncated message",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> InfoHash {
        let mut x = [0u8; HASH_LEN];
        x[0] = b;
        x[HASH_LEN - 1] = b ^ 0xFF;
        InfoHash(x)
    }

    fn q(query: Query) -> Msg {
        // A ping carries no target on the wire, so the fixture gives it none:
        // a round trip cannot restore what the reference never packs.
        let target = if query == Query::Ping { InfoHash::zero() } else { h(2) };
        // Likewise listen/update always pack a `sid` and refresh a `vid`.
        let sid = matches!(query, Query::Listen | Query::Update).then_some(0);
        let vid = (query == Query::Refresh).then_some(0);
        Msg::Query { tid: 77, from: h(1), query, target, token: Vec::new(),
                     values: Vec::new(), want: 0, sid, vid, client: false }
    }

    // ── layout pinned to the reference's packer calls ───────────────────

    /// `sendPing`: `{"a": {"id": bin20}, "q": "ping", "t": tid, "y": "q", "v": "o2"}`.
    #[test]
    fn a_ping_is_laid_out_as_send_ping_packs_it() {
        let mut want = vec![0x85];
        want.extend_from_slice(b"\xA1a\x81\xA2id\xC4\x14"); want.extend_from_slice(&h(1).0);
        want.extend_from_slice(b"\xA1q\xA4ping");
        want.extend_from_slice(b"\xA1t\x4D");
        want.extend_from_slice(b"\xA1y\xA1q");
        want.extend_from_slice(b"\xA1v\xA2o2");
        assert_eq!(q(Query::Ping).encode(), want);
    }

    /// `sendFindNode`: target under `target`, want as an array of address
    /// families — the two things the previous codec got wrong here.
    #[test]
    fn a_find_uses_target_and_want() {
        let mut m = q(Query::Find);
        if let Msg::Query { want, .. } = &mut m { *want = WANT4 | WANT6; }
        let b = m.encode();
        let mut a = vec![0x83];
        a.extend_from_slice(b"\xA2id\xC4\x14"); a.extend_from_slice(&h(1).0);
        a.extend_from_slice(b"\xA6target\xC4\x14"); a.extend_from_slice(&h(2).0);
        a.extend_from_slice(b"\xA1w\x92\x02\x0A");
        assert!(b.windows(a.len()).any(|w| w == &a[..]), "args map not as sendFindNode packs it");
        assert!(!b.windows(2).any(|w| w == b"\xA1h"), "a find must not carry `h`");
        assert_eq!(Msg::decode(&b), Some(m));
    }

    /// `sendPong`: `{"r": {"id", "sa"}, "t", "y": "r", "v"}` — `sa` is the ip
    /// bytes alone, which is how a node learns its public address.
    #[test]
    fn a_pong_carries_the_requesters_address_as_ip_bytes() {
        let m = Msg::Reply { tid: 9, from: h(3), sa: Some(Ip::V4([203, 0, 113, 7])),
                             nodes: vec![], values: vec![], token: vec![], vid: None };
        let mut want = vec![0x84];
        want.extend_from_slice(b"\xA1r\x82\xA2id\xC4\x14"); want.extend_from_slice(&h(3).0);
        want.extend_from_slice(b"\xA2sa\xC4\x04\xCB\x00\x71\x07");
        want.extend_from_slice(b"\xA1t\x09\xA1y\xA1r\xA1v\xA2o2");
        assert_eq!(m.encode(), want);
        assert_eq!(Msg::decode(&want), Some(m));
    }

    /// `sendNodesValues`: `n4` is ONE blob of 26-byte records, the token is a
    /// blob, and each value is `{"id", "dat": {"body": {"type", "data"}}}`.
    #[test]
    fn a_get_reply_matches_send_nodes_values() {
        let m = Msg::Reply {
            tid: 300, from: h(3), sa: Some(Ip::V4([10, 0, 0, 1])),
            nodes: vec![(h(4), Addr::new([192, 168, 1, 2], 4222)),
                        (h(5), Addr::new([192, 168, 1, 3], 4223))],
            values: vec![Value::new(0x1234, b"hello".to_vec())],
            token: vec![0xDE, 0xAD, 0xBE, 0xEF], vid: None,
        };
        let b = m.encode();
        // n4: 2 × 26 bytes, one bin.
        let mut n4 = vec![0xA2, b'n', b'4', 0xC4, 52];
        n4.extend_from_slice(&h(4).0); n4.extend_from_slice(&[192, 168, 1, 2, 0x10, 0x7E]);
        n4.extend_from_slice(&h(5).0); n4.extend_from_slice(&[192, 168, 1, 3, 0x10, 0x7F]);
        assert!(b.windows(n4.len()).any(|w| w == &n4[..]), "n4 must be one concatenated blob");
        assert!(b.windows(11).any(|w| w == b"\xA5token\xC4\x04\xDE\xAD\xBE"), "token must be bin");
        let val = b"\xA6values\x91\x82\xA2id\xCD\x12\x34\xA3dat\x81\xA4body\x82\xA4type\x00\xA4data\xC4\x05hello";
        assert!(b.windows(val.len()).any(|w| w == &val[..]), "value must nest id/dat/body/type/data");
        assert_eq!(Msg::decode(&b), Some(m));
    }

    /// `sendAnnounceValue`: `{"id", "h", "values": [...], "token": bin}`.
    #[test]
    fn a_put_carries_values_as_an_array_and_the_token_as_a_blob() {
        let mut m = q(Query::Put);
        if let Msg::Query { values, token, .. } = &mut m {
            values.push(Value::new(7, b"payload".to_vec()));
            *token = vec![1, 2, 3];
        }
        let b = m.encode();
        assert!(b.windows(9).any(|w| w == b"\xA5token\xC4\x03\x01"));
        assert!(b.windows(8).any(|w| w == b"\xA6values\x91"));
        assert_eq!(Msg::decode(&b), Some(m));
    }

    /// `sendValueAnnounced`: `{"r": {"id", "vid", "sa"}}` — the put ack.
    #[test]
    fn a_put_ack_names_the_value_it_accepted() {
        let m = Msg::Reply { tid: 1, from: h(3), sa: Some(Ip::V6([1; 16])), nodes: vec![],
                             values: vec![], token: vec![], vid: Some(u64::MAX) };
        let b = m.encode();
        assert!(b.windows(13).any(|w| w == b"\xA3vid\xCF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF"));
        assert_eq!(Msg::decode(&b), Some(m));
    }

    /// `sendError`: `e: [code, message]`, optionally `r: {id}`.
    #[test]
    fn an_error_is_an_array_of_code_and_message() {
        let m = Msg::Error { tid: 5, code: ERR_UNAUTHORIZED, from: Some(h(9)) };
        let b = m.encode();
        assert!(b.windows(6).any(|w| w == b"\xA1e\x92\xCD\x01\x91"));
        assert_eq!(Msg::decode(&b), Some(m));
        let anon = Msg::Error { tid: 5, code: 404, from: None };
        assert_eq!(Msg::decode(&anon.encode()), Some(anon));
    }

    #[test]
    fn listen_refresh_and_update_carry_their_extra_fields() {
        let mut l = q(Query::Listen);
        if let Msg::Query { token, sid, .. } = &mut l { *token = vec![9; 20]; *sid = Some(4242); }
        assert_eq!(Msg::decode(&l.encode()), Some(l.clone()));
        assert!(l.encode().windows(4).any(|w| w == b"\xA2ve\x01"));

        let mut r = q(Query::Refresh);
        if let Msg::Query { token, vid, .. } = &mut r { *token = vec![9]; *vid = Some(77); }
        assert_eq!(Msg::decode(&r.encode()), Some(r));

        let mut u = q(Query::Update);
        if let Msg::Query { token, sid, values, .. } = &mut u {
            *token = vec![9]; *sid = Some(1); values.push(Value::new(3, vec![1, 2]));
        }
        assert_eq!(Msg::decode(&u.encode()), Some(u));
    }

    #[test]
    fn a_client_flag_travels_as_s_true() {
        let mut m = q(Query::Get);
        if let Msg::Query { client, .. } = &mut m { *client = true; }
        let b = m.encode();
        assert_eq!(b[0], 0x86, "six top-level keys with s");
        assert!(b.ends_with(b"\xA1s\xC3"));
        assert_eq!(Msg::decode(&b), Some(m));
    }

    // ── captured from the reference itself ───────────────────────────────
    //
    // Recorded with `vendor/opendht`'s `dhtnode` (built in-sandbox, see
    // maintenance/OPENDHT_INTEROP.md) answering the ping and find that the
    // tests above assemble by hand. These are the bytes a real node sent;
    // decoding them and getting the same bytes back is the compliance claim.

    const CAPTURED_PONG: &str = "84a17282a26964c414c6af394ee428cb0c7ce4629259b12b115139784aa27361c4047f000001a1744da179a172a176a26f32";
    const CAPTURED_FIND_REPLY: &str = "84a17283a26964c414c6af394ee428cb0c7ce4629259b12b115139784aa27361c4047f000001a5746f6b656ec42034e811f13bc487b9c60721e3af24f2d99a8473d50429597c633c3b7c5ce4513aa1744ea179a172a176a26f32";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn a_captured_reference_pong_decodes_and_re_encodes_identically() {
        let bytes = unhex(CAPTURED_PONG);
        let m = Msg::decode(&bytes).expect("a real pong must decode");
        match &m {
            Msg::Reply { tid: 77, sa: Some(Ip::V4([127, 0, 0, 1])), nodes, values, token, vid: None, from } => {
                assert!(nodes.is_empty() && values.is_empty() && token.is_empty());
                assert_eq!(from.0[0], 0xc6);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.encode(), bytes, "our encoder does not reproduce the reference's pong byte for byte");
    }

    #[test]
    fn a_captured_reference_find_reply_decodes_and_re_encodes_identically() {
        let bytes = unhex(CAPTURED_FIND_REPLY);
        let m = Msg::decode(&bytes).expect("a real find reply must decode");
        match &m {
            Msg::Reply { tid: 78, sa: Some(Ip::V4([127, 0, 0, 1])), token, nodes, .. } => {
                assert_eq!(token.len(), 32, "the reference's tokens are 32 bytes — a u32 could never have carried one");
                assert!(nodes.is_empty(), "a lone node knows nobody");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.encode(), bytes);
    }

    // ── what the decoder tolerates, because the reference sends it ──────

    #[test]
    fn every_query_name_round_trips() {
        for query in [Query::Ping, Query::Find, Query::Get, Query::Put,
                      Query::Listen, Query::Refresh, Query::Update] {
            let m = q(query);
            assert_eq!(Msg::decode(&m.encode()), Some(m), "{query:?}");
        }
    }

    #[test]
    fn an_unknown_query_name_is_refused() {
        let mut b = q(Query::Ping).encode();
        let i = b.windows(4).position(|w| w == b"ping").unwrap();
        b[i..i + 4].copy_from_slice(b"pong");
        assert_eq!(Msg::decode(&b), None);
    }

    #[test]
    fn unknown_keys_at_every_level_are_skipped() {
        // Top level `n` (network id) and `x`; args `c` (creation), `q` (a
        // filter map); a value's `p`, `pt`, and a `sig` next to the body.
        let mut b = vec![0x87];
        b.extend_from_slice(b"\xA1n\xCD\x12\x34");          // network id
        b.extend_from_slice(b"\xA1x\x93\x01\x02\x03");      // something new
        b.extend_from_slice(b"\xA1a");
        b.push(0x85);
        b.extend_from_slice(b"\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
        b.extend_from_slice(b"\xA1h\xC4\x14"); b.extend_from_slice(&h(2).0);
        b.extend_from_slice(b"\xA1c\xCE\x66\x00\x00\x00");  // creation time
        b.extend_from_slice(b"\xA1q\x82\xA1w\x90\xA1s\x90"); // empty filter
        b.extend_from_slice(b"\xA6values\x91");
        b.push(0x84);
        b.extend_from_slice(b"\xA2id\x05");
        b.extend_from_slice(b"\xA3dat\x82\xA4body\x83\xA4type\x00\xA4data\xC4\x02ok\xA5utype\xA1u\xA3sig\xC4\x01\x00");
        b.extend_from_slice(b"\xA1p\x01\xA2pt\xA0");
        b.extend_from_slice(b"\xA1q\xA3put\xA1t\x02\xA1y\xA1q\xA1v\xA2o2");
        match Msg::decode(&b) {
            Some(Msg::Query { query: Query::Put, values, target, .. }) => {
                assert_eq!(target, h(2));
                assert_eq!(values, vec![Value::new(5, b"ok".to_vec())]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_token_arrives_as_bin_str_or_array_and_reads_the_same() {
        let base = |tok: &[u8]| {
            let mut b = vec![0x85];
            b.extend_from_slice(b"\xA1a\x83\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
            b.extend_from_slice(b"\xA1h\xC4\x14"); b.extend_from_slice(&h(2).0);
            b.extend_from_slice(b"\xA5token"); b.extend_from_slice(tok);
            b.extend_from_slice(b"\xA1q\xA6listen\xA1t\x01\xA1y\xA1q\xA1v\xA2o2");
            b
        };
        for tok in [&b"\xC4\x03abc"[..], b"\xA3abc", b"\x93\x61\x62\x63"] {
            match Msg::decode(&base(tok)) {
                Some(Msg::Query { token, .. }) => assert_eq!(token, b"abc"),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_legacy_four_byte_tid_is_read_big_endian() {
        let mut b = vec![0x84];
        b.extend_from_slice(b"\xA1r\x81\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
        b.extend_from_slice(b"\xA1t\xC4\x04\x00\x00\x01\x02\xA1y\xA1r\xA1v\xA2o2");
        assert!(matches!(Msg::decode(&b), Some(Msg::Reply { tid: 258, .. })));
    }

    #[test]
    fn an_unreadable_value_is_dropped_not_fatal() {
        // The reference wraps each value in try/catch. An encrypted value
        // (dat is a bin) sits next to a plain one; the plain one survives.
        let m = Msg::Reply { tid: 1, from: h(1), sa: None, nodes: vec![], token: vec![], vid: None,
                             values: vec![Value::new(1, b"a".to_vec()), Value::new(2, b"b".to_vec())] };
        let mut b = m.encode();
        // Replace the SECOND value's `dat` map with a bin cypher of the same
        // total length: {"body":{"type":0,"data":bin1"b"}} = 1+5+1+5+1+5+3 = 21 bytes.
        let pat = b"\xA3dat\x81\xA4body\x82\xA4type\x00\xA4data\xC4\x01b";
        let i = b.windows(pat.len()).rposition(|w| w == &pat[..]).unwrap();
        let mut repl = b"\xA3dat\xC4\x13".to_vec();
        repl.extend_from_slice(&[0xEE; 19]);
        b.splice(i..i + pat.len(), repl);
        match Msg::decode(&b) {
            Some(Msg::Reply { values, .. }) => assert_eq!(values, vec![Value::new(1, b"a".to_vec())]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_values_entry_that_is_a_size_marks_parts_and_is_skipped() {
        let mut b = vec![0x84];
        b.extend_from_slice(b"\xA1r\x82\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
        b.extend_from_slice(b"\xA6values\x92\xCD\x40\x00");   // 16384-byte value coming as parts
        b.extend_from_slice(b"\x82\xA2id\x01\xA3dat\x81\xA4body\x82\xA4type\x00\xA4data\xC4\x01z");
        b.extend_from_slice(b"\xA1t\x01\xA1y\xA1r\xA1v\xA2o2");
        match Msg::decode(&b) {
            Some(Msg::Reply { values, .. }) => assert_eq!(values.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn value_parts_decode_to_their_own_kind() {
        let m = Msg::Parts { tid: 8, fragments: vec![(0, 0, vec![1, 2, 3]), (0, 3, vec![4])] };
        assert_eq!(Msg::decode(&m.encode()), Some(m));
    }

    #[test]
    fn a_version_zero_listener_push_is_an_update() {
        // Older nodes push with `u` args and `y: "r"` and no query name.
        let mut b = vec![0x84];
        b.extend_from_slice(b"\xA1u\x81\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
        b.extend_from_slice(b"\xA1t\x07\xA1y\xA1r\xA1v\xA2o2");
        assert!(matches!(Msg::decode(&b), Some(Msg::Query { query: Query::Update, tid: 7, .. })));
    }

    #[test]
    fn want_accepts_every_platforms_af_inet6() {
        for af in [10u8, 23, 30] {
            let mut b = vec![0x85];
            b.extend_from_slice(b"\xA1a\x83\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
            b.extend_from_slice(b"\xA6target\xC4\x14"); b.extend_from_slice(&h(2).0);
            b.extend_from_slice(&[0xA1, b'w', 0x92, 0x02, af]);
            b.extend_from_slice(b"\xA1q\xA4find\xA1t\x01\xA1y\xA1q\xA1v\xA2o2");
            match Msg::decode(&b) {
                Some(Msg::Query { want, .. }) => assert_eq!(want, WANT4 | WANT6, "af {af}"),
                other => panic!("{other:?}"),
            }
        }
    }

    // ── robustness ──────────────────────────────────────────────────────

    #[test]
    fn truncation_at_every_length_is_refused() {
        // A decoder returning a partial message puts attacker-controlled
        // short reads into the routing table.
        let m = Msg::Reply {
            tid: 300, from: h(3), sa: Some(Ip::V4([1, 2, 3, 4])),
            nodes: vec![(h(4), Addr::new([10, 0, 0, 1], 4222)), (h(5), Addr::v6([7; 16], 1))],
            values: vec![Value::new(9, vec![0xAA; 100])], token: vec![1, 2, 3, 4], vid: None,
        };
        let b = m.encode();
        for n in 0..b.len() {
            assert_eq!(Msg::decode(&b[..n]), None, "prefix of {n} bytes decoded");
        }
        assert_eq!(Msg::decode(&b), Some(m));
    }

    #[test]
    fn trailing_garbage_is_ignored() {
        let mut b = q(Query::Ping).encode();
        b.extend_from_slice(&[0; 40]);
        assert!(Msg::decode(&b).is_some());
    }

    #[test]
    fn values_straddling_every_bin_header_class_survive() {
        for n in [0usize, 1, 31, 32, 254, 255, 256, 65535, 65536] {
            let m = Msg::Reply { tid: 1, from: h(1), sa: None, nodes: vec![], token: vec![], vid: None,
                                 values: vec![Value::new(1, vec![0x5A; n])] };
            assert_eq!(Msg::decode(&m.encode()), Some(m), "value of {n} bytes");
        }
    }

    #[test]
    fn a_transaction_id_survives_the_full_u32_range() {
        for tid in [0u32, 1, 127, 128, 255, 256, 65535, 65536, u32::MAX] {
            let mut m = q(Query::Ping);
            if let Msg::Query { tid: t, .. } = &mut m { *t = tid; }
            assert_eq!(Msg::decode(&m.encode()), Some(m), "tid {tid}");
        }
    }

    #[test]
    fn a_value_id_at_the_u64_boundary_round_trips() {
        for id in [1u64, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            let m = Msg::Reply { tid: 1, from: h(1), sa: None, nodes: vec![], token: vec![], vid: None,
                                 values: vec![Value::new(id, vec![1])] };
            assert_eq!(Msg::decode(&m.encode()), Some(m), "id {id}");
        }
    }

    #[test]
    fn v4_and_v6_nodes_travel_in_separate_keys_and_come_back_together() {
        let m = Msg::Reply {
            tid: 1, from: h(1), sa: None, token: vec![], vid: None, values: vec![],
            nodes: vec![(h(2), Addr::new([1, 1, 1, 1], 1)), (h(3), Addr::v6([2; 16], 2)),
                        (h(4), Addr::new([3, 3, 3, 3], 3))],
        };
        let b = m.encode();
        assert!(b.windows(3).any(|w| w == b"\xA2n4"));
        assert!(b.windows(3).any(|w| w == b"\xA2n6"));
        // Order within a family is kept; families are grouped.
        match Msg::decode(&b) {
            Some(Msg::Reply { nodes, .. }) => {
                assert_eq!(nodes.len(), 3);
                assert_eq!(nodes[0].0, h(2)); assert_eq!(nodes[1].0, h(4)); assert_eq!(nodes[2].0, h(3));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_node_blob_with_a_ragged_tail_is_refused() {
        let mut b = vec![0x84];
        b.extend_from_slice(b"\xA1r\x82\xA2id\xC4\x14"); b.extend_from_slice(&h(1).0);
        b.extend_from_slice(b"\xA2n4\xC4\x1B"); b.extend_from_slice(&[0; 27]);   // 26 + 1
        b.extend_from_slice(b"\xA1t\x01\xA1y\xA1r\xA1v\xA2o2");
        assert_eq!(Msg::decode(&b), None);
    }

    #[test]
    fn a_hash_must_be_a_bin_of_exactly_twenty_bytes() {
        for bad in [&b"\xC4\x13"[..], b"\xC4\x15", b"\xB4"] {
            let mut b = vec![0x85];
            b.extend_from_slice(b"\xA1a\x81\xA2id"); b.extend_from_slice(bad); b.extend_from_slice(&[0; 21]);
            b.extend_from_slice(b"\xA1q\xA4ping\xA1t\x01\xA1y\xA1q\xA1v\xA2o2");
            assert_eq!(Msg::decode(&b), None);
        }
    }

    #[test]
    fn skipping_is_structural_so_a_malformed_unknown_still_fails() {
        // An unknown key whose value claims 200 bytes with 3 present.
        let mut b = vec![0x82];
        b.extend_from_slice(b"\xA1x\xC4\xC8\x01\x02\x03");
        assert_eq!(Msg::decode(&b), None);
    }

    #[test]
    fn a_full_reply_fits_in_one_safe_datagram() {
        // The reference sends up to 8 nodes (SEND_NODES); a token and address
        // on top. Well under the path MTU with a value beside it.
        let nodes: Vec<_> = (0..8u8).map(|i| (h(i), Addr::v6([i; 16], 4222))).collect();
        let m = Msg::Reply { tid: 1, from: h(1), sa: Some(Ip::V6([9; 16])), nodes, token: vec![0; 20],
                             vid: None, values: vec![Value::new(1, vec![0; 700])] };
        assert!(m.encode().len() < SAFE_DATAGRAM, "{}", m.encode().len());
    }

    #[test]
    fn xor_distance_is_symmetric_and_zero_to_self() {
        let a = InfoHash::from_seed(1);
        let b = InfoHash::from_seed(2);
        assert_eq!(a.distance(&b), b.distance(&a));
        assert_eq!(a.distance(&a), [0; HASH_LEN]);
        assert!(!a.is_zero() && InfoHash::zero().is_zero());
    }
}
