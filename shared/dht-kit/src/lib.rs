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
//! ## The format
//!
//! OpenDHT's `src/net.h` speaks msgpack maps keyed with single letters:
//!
//! ```text
//! "t"  transaction id
//! "y"  message kind — "q" query, "r" reply, "e" error
//! "q"  query name — ping, find, get, put, listen, refresh
//! "a"  arguments map: "id", "target"/"h", "token", "values", "want"
//! "r"  reply map:     "id", "token", "nodes", "values"
//! "e"  error array:   [code, message]
//! ```
//!
//! Implemented as an exact msgpack **subset** — fixmap/map16, fixstr/str8,
//! bin8/bin16/bin32, positive fixint/u8/u16/u32/u64, fixarray/array16 — because
//! those are the cases these maps need. It is not a general msgpack library and
//! does not pretend to be: a general one would accept encodings the reference
//! never emits, and accepting more than the reference is how a parser becomes an
//! attack surface rather than a compatibility layer.
//!
//! ## What this crate does NOT establish
//!
//! Round-tripping our own encoder proves self-consistency, not compliance. Until
//! bytes from a real OpenDHT node go through `Msg::decode` and come back
//! identical, every claim here is "shaped like the reference" rather than
//! "interoperates with it". `docs/` records that gap rather than papering it.

pub const HASH_LEN: usize = 20;

/// A 160-bit id — OpenDHT's `InfoHash` width, used for both node ids and keys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct InfoHash(pub [u8; HASH_LEN]);

impl InfoHash {
    pub fn zero() -> InfoHash { InfoHash([0; HASH_LEN]) }

    /// Deterministic id from a seed, for tests and for deriving a key from an
    /// application string. Not a cryptographic hash and named so it cannot be
    /// mistaken for one.
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

/// A stored value. `id` is OpenDHT's per-value identifier, which is what makes a
/// key hold a *set* rather than a single item — two writers under one key
/// coexist instead of overwriting.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Value {
    pub id: u64,
    pub data: Vec<u8>,
}

/// A peer address as the wire carries it: 4-byte IPv4 plus port.
///
/// IPv4-only, deliberately and visibly. The reference carries both, and a kit
/// that silently truncated a v6 address would produce a routing table full of
/// plausible wrong peers. `Addr::from_bytes` refuses a length it does not
/// understand instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Addr {
    pub ip: [u8; 4],
    pub port: u16,
}

impl Addr {
    pub fn new(ip: [u8; 4], port: u16) -> Addr { Addr { ip, port } }

    pub fn to_bytes(&self) -> [u8; 6] {
        let mut b = [0u8; 6];
        b[..4].copy_from_slice(&self.ip);
        b[4..].copy_from_slice(&self.port.to_be_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Addr> {
        if b.len() != 6 { return None; }
        Some(Addr {
            ip: [b[0], b[1], b[2], b[3]],
            port: u16::from_be_bytes([b[4], b[5]]),
        })
    }
}

/// The query names the reference sends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Query { Ping, Find, Get, Put, Listen, Refresh }

impl Query {
    pub fn name(self) -> &'static str {
        match self {
            Query::Ping => "ping",
            Query::Find => "find",
            Query::Get => "get",
            Query::Put => "put",
            Query::Listen => "listen",
            Query::Refresh => "refresh",
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
        target: InfoHash,
        token: u32,
        value: Option<Value>,
    },
    Reply {
        tid: u32,
        from: InfoHash,
        nodes: Vec<(InfoHash, Addr)>,
        values: Vec<Value>,
        token: u32,
    },
    /// A listener push — the reference's `ValueData`: values arriving for a key
    /// nobody asked for right now. Without it a `listen` is a subscription with
    /// no delivery path.
    Push {
        tid: u32,
        from: InfoHash,
        target: InfoHash,
        values: Vec<Value>,
    },
    Error { tid: u32, code: u32 },
}

// ── msgpack subset ──────────────────────────────────────────────────────────

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    if b.len() < 32 { out.push(0xA0 | b.len() as u8); }
    else { out.push(0xD9); out.push(b.len() as u8); }
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
    fn uint(&mut self) -> Option<u64> {
        let t = self.byte()?;
        Some(match t {
            0x00..=0x7F => t as u64,
            0xCC => self.byte()? as u64,
            0xCD => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as u64,
            0xCE => u32::from_be_bytes(self.take(4)?.try_into().ok()?) as u64,
            0xCF => u64::from_be_bytes(self.take(8)?.try_into().ok()?),
            _ => return None,
        })
    }
    fn str(&mut self) -> Option<String> {
        let t = self.byte()?;
        let n = match t {
            0xA0..=0xBF => (t & 0x1F) as usize,
            0xD9 => self.byte()? as usize,
            0xDA => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize,
            _ => return None,
        };
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn bin(&mut self) -> Option<Vec<u8>> {
        let t = self.byte()?;
        let n = match t {
            0xC4 => self.byte()? as usize,
            0xC5 => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize,
            0xC6 => u32::from_be_bytes(self.take(4)?.try_into().ok()?) as usize,
            _ => return None,
        };
        Some(self.take(n)?.to_vec())
    }
    fn map_len(&mut self) -> Option<usize> {
        let t = self.byte()?;
        match t {
            0x80..=0x8F => Some((t & 0x0F) as usize),
            0xDE => Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize),
            _ => None,
        }
    }
    fn arr_len(&mut self) -> Option<usize> {
        let t = self.byte()?;
        match t {
            0x90..=0x9F => Some((t & 0x0F) as usize),
            0xDC => Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize),
            _ => None,
        }
    }
    fn hash(&mut self) -> Option<InfoHash> {
        let b = self.bin()?;
        if b.len() != HASH_LEN { return None; }
        let mut h = [0u8; HASH_LEN];
        h.copy_from_slice(&b);
        Some(InfoHash(h))
    }
}

fn put_value(out: &mut Vec<u8>, v: &Value) {
    put_map(out, 2);
    put_str(out, "id");
    put_uint(out, v.id);
    put_str(out, "d");
    put_bin(out, &v.data);
}

fn read_value(r: &mut Rd) -> Option<Value> {
    let n = r.map_len()?;
    let mut id = None;
    let mut data = None;
    for _ in 0..n {
        match r.str()?.as_str() {
            "id" => id = Some(r.uint()?),
            "d" => data = Some(r.bin()?),
            _ => return None,
        }
    }
    Some(Value { id: id?, data: data? })
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match self {
            Msg::Query { tid, from, query, target, token, value } => {
                put_map(&mut o, 4);
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "q");
                put_str(&mut o, "q"); put_str(&mut o, query.name());
                put_str(&mut o, "a");
                let n = 3 + value.is_some() as usize;
                put_map(&mut o, n);
                put_str(&mut o, "id"); put_bin(&mut o, &from.0);
                put_str(&mut o, "h"); put_bin(&mut o, &target.0);
                put_str(&mut o, "token"); put_uint(&mut o, *token as u64);
                if let Some(v) = value { put_str(&mut o, "v"); put_value(&mut o, v); }
            }
            Msg::Reply { tid, from, nodes, values, token } => {
                put_map(&mut o, 3);
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "r");
                put_str(&mut o, "r");
                put_map(&mut o, 4);
                put_str(&mut o, "id"); put_bin(&mut o, &from.0);
                put_str(&mut o, "token"); put_uint(&mut o, *token as u64);
                put_str(&mut o, "nodes");
                put_arr(&mut o, nodes.len());
                for (h, a) in nodes {
                    let mut packed = Vec::with_capacity(HASH_LEN + 6);
                    packed.extend_from_slice(&h.0);
                    packed.extend_from_slice(&a.to_bytes());
                    put_bin(&mut o, &packed);
                }
                put_str(&mut o, "values");
                put_arr(&mut o, values.len());
                for v in values { put_value(&mut o, v); }
            }
            Msg::Push { tid, from, target, values } => {
                put_map(&mut o, 4);
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "v");
                put_str(&mut o, "h"); put_bin(&mut o, &target.0);
                put_str(&mut o, "r");
                put_map(&mut o, 2);
                put_str(&mut o, "id"); put_bin(&mut o, &from.0);
                put_str(&mut o, "values");
                put_arr(&mut o, values.len());
                for v in values { put_value(&mut o, v); }
            }
            Msg::Error { tid, code } => {
                put_map(&mut o, 3);
                put_str(&mut o, "t"); put_uint(&mut o, *tid as u64);
                put_str(&mut o, "y"); put_str(&mut o, "e");
                put_str(&mut o, "e");
                put_arr(&mut o, 1);
                put_uint(&mut o, *code as u64);
            }
        }
        o
    }

    pub fn decode(bytes: &[u8]) -> Option<Msg> {
        let mut r = Rd { b: bytes, i: 0 };
        let n = r.map_len()?;
        let mut tid = None;
        let mut kind = None;
        let mut qname = None;
        let mut from = None;
        let mut target = None;
        let mut token = 0u32;
        let mut value = None;
        let mut values = Vec::new();
        let mut nodes = Vec::new();
        let mut code = None;

        for _ in 0..n {
            match r.str()?.as_str() {
                "t" => tid = Some(r.uint()? as u32),
                "y" => kind = Some(r.str()?),
                "q" => qname = Some(r.str()?),
                "h" => target = Some(r.hash()?),
                "a" | "r" => {
                    let m = r.map_len()?;
                    for _ in 0..m {
                        match r.str()?.as_str() {
                            "id" => from = Some(r.hash()?),
                            "h" => target = Some(r.hash()?),
                            "token" => token = r.uint()? as u32,
                            "v" => value = Some(read_value(&mut r)?),
                            "values" => {
                                let k = r.arr_len()?;
                                for _ in 0..k { values.push(read_value(&mut r)?); }
                            }
                            "nodes" => {
                                let k = r.arr_len()?;
                                for _ in 0..k {
                                    let b = r.bin()?;
                                    if b.len() != HASH_LEN + 6 { return None; }
                                    let mut h = [0u8; HASH_LEN];
                                    h.copy_from_slice(&b[..HASH_LEN]);
                                    nodes.push((InfoHash(h), Addr::from_bytes(&b[HASH_LEN..])?));
                                }
                            }
                            _ => return None,
                        }
                    }
                }
                "e" => {
                    let k = r.arr_len()?;
                    if k == 0 { return None; }
                    code = Some(r.uint()? as u32);
                    for _ in 1..k { let _ = r.str(); }
                }
                _ => return None,
            }
        }

        let tid = tid?;
        match kind?.as_str() {
            "q" => Some(Msg::Query {
                tid,
                from: from?,
                query: Query::from_name(&qname?)?,
                target: target?,
                token,
                value,
            }),
            "r" => Some(Msg::Reply { tid, from: from?, nodes, values, token }),
            "v" => Some(Msg::Push { tid, from: from?, target: target?, values }),
            "e" => Some(Msg::Error { tid, code: code? }),
            _ => None,
        }
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

    #[test]
    fn every_query_name_round_trips() {
        // ping, find, get, put, listen, refresh — the six the reference sends.
        // refresh is the one snapcore lacks entirely, and a kit that omitted it
        // would make "compatible" mean "compatible with what we already did".
        for q in [Query::Ping, Query::Find, Query::Get, Query::Put,
                  Query::Listen, Query::Refresh] {
            let m = Msg::Query { tid: 7, from: h(1), query: q, target: h(2),
                                 token: 0, value: None };
            assert_eq!(Msg::decode(&m.encode()), Some(m), "{q:?}");
            assert_eq!(Query::from_name(q.name()), Some(q));
        }
    }

    #[test]
    fn an_unknown_query_name_is_refused() {
        // Accepting more than the reference emits is how a parser becomes an
        // attack surface rather than a compatibility layer.
        assert_eq!(Query::from_name("announce_peer"), None);
        assert_eq!(Query::from_name(""), None);
    }

    #[test]
    fn a_put_carries_its_value_and_token() {
        let v = Value { id: 0xDEAD_BEEF_CAFE, data: b"payload".to_vec() };
        let m = Msg::Query { tid: 1, from: h(3), query: Query::Put, target: h(4),
                             token: 99, value: Some(v.clone()) };
        match Msg::decode(&m.encode()).expect("round trip") {
            Msg::Query { value: Some(got), token, .. } => {
                assert_eq!(got, v);
                assert_eq!(token, 99, "a put without its token is an unauthenticated write");
            }
            other => panic!("decoded as {other:?}"),
        }
    }

    #[test]
    fn a_reply_carries_nodes_and_values_together() {
        // The reference permits both in one message; an either/or codec would
        // silently truncate a reply that had both.
        let m = Msg::Reply {
            tid: 42,
            from: h(5),
            nodes: vec![(h(6), Addr::new([192, 168, 1, 5], 4222)),
                        (h(7), Addr::new([10, 0, 0, 2], 4223))],
            values: vec![Value { id: 1, data: vec![0xAA; 40] }],
            token: 1234,
        };
        assert_eq!(Msg::decode(&m.encode()), Some(m));
    }

    #[test]
    fn a_listener_push_round_trips() {
        let m = Msg::Push { tid: 9, from: h(8), target: h(9),
                            values: vec![Value { id: 2, data: b"pushed".to_vec() }] };
        assert_eq!(Msg::decode(&m.encode()), Some(m));
    }

    #[test]
    fn truncation_at_every_length_is_refused() {
        // The failure mode a hand-rolled subset actually has. A decoder that
        // returned a partial message would put attacker-controlled short reads
        // into a routing table.
        let m = Msg::Reply { tid: 11, from: h(10),
                             nodes: vec![(h(11), Addr::new([127, 0, 0, 1], 5000))],
                             values: vec![Value { id: 3, data: vec![1, 2, 3] }],
                             token: 7 };
        let full = m.encode();
        for cut in 0..full.len() {
            assert!(Msg::decode(&full[..cut]).is_none(), "a {cut}-byte prefix decoded");
        }
        assert!(Msg::decode(&full).is_some());
    }

    #[test]
    fn values_straddling_every_bin_header_class_survive() {
        // bin8 caps at 255 and bin16 at 65535 — the boundaries where a subset
        // codec writes the wrong header byte and the payload shifts by one.
        for n in [0usize, 1, 254, 255, 256, 65_535, 65_536] {
            let m = Msg::Push { tid: 1, from: h(12), target: h(13),
                                values: vec![Value { id: 4, data: vec![0x5A; n] }] };
            assert_eq!(Msg::decode(&m.encode()), Some(m), "{n} bytes");
        }
    }

    #[test]
    fn a_transaction_id_survives_the_full_u32_range() {
        // tid matches a reply to its query; a narrowed one mismatches under
        // load rather than failing.
        for tid in [0u32, 127, 128, 255, 256, 65_535, 65_536, u32::MAX] {
            assert_eq!(Msg::decode(&Msg::Error { tid, code: 0 }.encode()),
                       Some(Msg::Error { tid, code: 0 }));
        }
    }

    #[test]
    fn an_address_that_is_not_four_bytes_plus_a_port_is_refused() {
        // IPv4-only is a stated limit, not a silent one: truncating a v6
        // address would fill a routing table with plausible wrong peers.
        assert!(Addr::from_bytes(&[1, 2, 3, 4, 0, 80]).is_some());
        assert!(Addr::from_bytes(&[1, 2, 3, 4]).is_none());
        assert!(Addr::from_bytes(&[0; 18]).is_none());
    }

    #[test]
    fn a_node_entry_of_the_wrong_width_is_refused() {
        // 20-byte hash + 6-byte addr. Anything else means a different address
        // family or a different id width, and guessing is worse than failing.
        let mut bad = Msg::Reply { tid: 1, from: h(14),
                                   nodes: vec![(h(15), Addr::new([1, 1, 1, 1], 1))],
                                   values: vec![], token: 0 }.encode();
        // Corrupt the node blob's declared length by one.
        if let Some(p) = bad.windows(2).position(|w| w == [0xC4, 26]) {
            bad[p + 1] = 25;
            assert!(Msg::decode(&bad).is_none(), "a 25-byte node entry decoded");
        }
    }

    #[test]
    fn an_unknown_map_key_is_refused_rather_than_ignored() {
        // Skipping unknown keys sounds tolerant and is how two implementations
        // silently disagree about what a message meant. Better to fail loudly
        // while the format is still being pinned down.
        let mut b = Vec::new();
        put_map(&mut b, 2);
        put_str(&mut b, "t"); put_uint(&mut b, 1);
        put_str(&mut b, "zz"); put_uint(&mut b, 0);
        assert!(Msg::decode(&b).is_none());
    }

    #[test]
    fn the_hash_width_matches_opendht() {
        // 160 bits. A kit that used 256 would be internally consistent and
        // unable to speak to anything.
        assert_eq!(HASH_LEN, 20);
        assert_eq!(InfoHash::zero().0.len(), 20);
        assert_ne!(InfoHash::from_seed(1), InfoHash::from_seed(2));
    }

    #[test]
    fn xor_distance_is_symmetric_and_zero_to_self() {
        let a = InfoHash::from_seed(0xA);
        let b = InfoHash::from_seed(0xB);
        assert_eq!(a.distance(&b), b.distance(&a));
        assert_eq!(a.distance(&a), [0u8; HASH_LEN]);
    }
}
