//! wire — OpenDHT-shaped messages over a hand-rolled msgpack subset.
//!
//! The reference (src/net.h) speaks msgpack maps keyed with single letters:
//! "t" transaction id, "y" message kind ("q" query / "r" reply / "e" error), "q" query
//! name, "a" arguments map, "r" reply map. Query names mirror `MessageType`:
//! ping, find, get, put (AnnounceValue), listen, refresh. Arguments carry "id" (sender
//! InfoHash), "target"/"h" (key), "token", "values", "nodes".
//!
//! The codec below implements exactly the msgpack cases these maps need — fixmap/map16,
//! fixstr/str8, bin8/bin16, positive fixint/u8/u16/u32/u64, fixarray/array16 — with a
//! strict round-trip test. It is NOT a general msgpack library and says so.

use crate::core::{Addr, InfoHash, Value, HASH_LEN};

#[derive(Clone, Debug, PartialEq)]
pub enum Q { Ping, Find, Get, Put, Listen }

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Query { tid: u32, from: InfoHash, q: Q, target: InfoHash, token: u32, value: Option<Value> },
    Reply { tid: u32, from: InfoHash, nodes: Vec<(InfoHash, Addr)>, values: Vec<Value>, token: u32 },
    /// Listener push (reference MessageType::ValueData): unsolicited values for a key.
    Push  { tid: u32, from: InfoHash, target: InfoHash, values: Vec<Value> },
    Error { tid: u32, code: u32 },
}

// ── msgpack subset ──────────────────────────────────────────────────────────────

pub fn mp_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    if b.len() < 32 { out.push(0xA0 | b.len() as u8); } else { out.push(0xD9); out.push(b.len() as u8); }
    out.extend_from_slice(b);
}
pub fn mp_bin(out: &mut Vec<u8>, b: &[u8]) {
    if b.len() < 256 { out.push(0xC4); out.push(b.len() as u8); }
    else { out.push(0xC5); out.extend_from_slice(&(b.len() as u16).to_be_bytes()); }
    out.extend_from_slice(b);
}
pub fn mp_uint(out: &mut Vec<u8>, v: u64) {
    match v {
        0..=0x7F => out.push(v as u8),
        0x80..=0xFF => { out.push(0xCC); out.push(v as u8); }
        0x100..=0xFFFF => { out.push(0xCD); out.extend_from_slice(&(v as u16).to_be_bytes()); }
        0x1_0000..=0xFFFF_FFFF => { out.push(0xCE); out.extend_from_slice(&(v as u32).to_be_bytes()); }
        _ => { out.push(0xCF); out.extend_from_slice(&v.to_be_bytes()); }
    }
}
pub fn mp_map(out: &mut Vec<u8>, n: usize) {
    if n < 16 { out.push(0x80 | n as u8); } else { out.push(0xDE); out.extend_from_slice(&(n as u16).to_be_bytes()); }
}
pub fn mp_arr(out: &mut Vec<u8>, n: usize) {
    if n < 16 { out.push(0x90 | n as u8); } else { out.push(0xDC); out.extend_from_slice(&(n as u16).to_be_bytes()); }
}

pub struct R<'a> { pub b: &'a [u8], pub i: usize }
impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self { R { b, i: 0 } }
    fn u8(&mut self) -> Option<u8> { let v = *self.b.get(self.i)?; self.i += 1; Some(v) }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.i + n > self.b.len() { return None; }
        let s = &self.b[self.i..self.i + n]; self.i += n; Some(s)
    }
    pub fn uint(&mut self) -> Option<u64> {
        let t = self.u8()?;
        Some(match t {
            0..=0x7F => t as u64,
            0xCC => self.u8()? as u64,
            0xCD => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as u64,
            0xCE => u32::from_be_bytes(self.take(4)?.try_into().ok()?) as u64,
            0xCF => u64::from_be_bytes(self.take(8)?.try_into().ok()?),
            _ => return None,
        })
    }
    pub fn str(&mut self) -> Option<&'a str> {
        let t = self.u8()?;
        let n = match t { 0xA0..=0xBF => (t & 0x1F) as usize, 0xD9 => self.u8()? as usize, _ => return None };
        std::str::from_utf8(self.take(n)?).ok()
    }
    pub fn bin(&mut self) -> Option<&'a [u8]> {
        let t = self.u8()?;
        let n = match t { 0xC4 => self.u8()? as usize,
                          0xC5 => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize,
                          _ => return None };
        self.take(n)
    }
    pub fn map(&mut self) -> Option<usize> {
        let t = self.u8()?;
        Some(match t { 0x80..=0x8F => (t & 0x0F) as usize,
                       0xDE => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize,
                       _ => return None })
    }
    pub fn arr(&mut self) -> Option<usize> {
        let t = self.u8()?;
        Some(match t { 0x90..=0x9F => (t & 0x0F) as usize,
                       0xDC => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize,
                       _ => return None })
    }
}

fn put_hash(out: &mut Vec<u8>, h: &InfoHash) { mp_bin(out, &h.0); }
fn get_hash(r: &mut R) -> Option<InfoHash> {
    let b = r.bin()?;
    if b.len() != HASH_LEN { return None; }
    let mut h = [0u8; HASH_LEN]; h.copy_from_slice(b); Some(InfoHash(h))
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(96);
        match self {
            Msg::Query { tid, from, q, target, token, value } => {
                mp_map(&mut o, 4);
                mp_str(&mut o, "t"); mp_uint(&mut o, *tid as u64);
                mp_str(&mut o, "y"); mp_str(&mut o, "q");
                mp_str(&mut o, "q"); mp_str(&mut o, match q {
                    Q::Ping => "ping", Q::Find => "find", Q::Get => "get",
                    Q::Put => "put", Q::Listen => "listen" });
                mp_str(&mut o, "a");
                let n = 2 + usize::from(*token != 0) + usize::from(value.is_some());
                mp_map(&mut o, n);
                mp_str(&mut o, "id"); put_hash(&mut o, from);
                mp_str(&mut o, "target"); put_hash(&mut o, target);
                if *token != 0 { mp_str(&mut o, "token"); mp_uint(&mut o, *token as u64); }
                if let Some(v) = value {
                    mp_str(&mut o, "value"); mp_arr(&mut o, 2);
                    mp_uint(&mut o, v.id); mp_bin(&mut o, &v.data);
                }
            }
            Msg::Reply { tid, from, nodes, values, token } => {
                mp_map(&mut o, 3);
                mp_str(&mut o, "t"); mp_uint(&mut o, *tid as u64);
                mp_str(&mut o, "y"); mp_str(&mut o, "r");
                mp_str(&mut o, "r");
                mp_map(&mut o, 4);
                mp_str(&mut o, "id"); put_hash(&mut o, from);
                mp_str(&mut o, "token"); mp_uint(&mut o, *token as u64);
                mp_str(&mut o, "nodes"); mp_arr(&mut o, nodes.len());
                for (id, addr) in nodes { mp_arr(&mut o, 2); put_hash(&mut o, id); mp_uint(&mut o, *addr); }
                mp_str(&mut o, "values"); mp_arr(&mut o, values.len());
                for v in values { mp_arr(&mut o, 2); mp_uint(&mut o, v.id); mp_bin(&mut o, &v.data); }
            }
            Msg::Push { tid, from, target, values } => {
                mp_map(&mut o, 4);
                mp_str(&mut o, "t"); mp_uint(&mut o, *tid as u64);
                mp_str(&mut o, "y"); mp_str(&mut o, "u"); // KEY_U: value data push
                mp_str(&mut o, "id"); put_hash(&mut o, from);
                mp_str(&mut o, "p");
                mp_map(&mut o, 2);
                mp_str(&mut o, "target"); put_hash(&mut o, target);
                mp_str(&mut o, "values"); mp_arr(&mut o, values.len());
                for v in values { mp_arr(&mut o, 2); mp_uint(&mut o, v.id); mp_bin(&mut o, &v.data); }
            }
            Msg::Error { tid, code } => {
                mp_map(&mut o, 3);
                mp_str(&mut o, "t"); mp_uint(&mut o, *tid as u64);
                mp_str(&mut o, "y"); mp_str(&mut o, "e");
                mp_str(&mut o, "e"); mp_uint(&mut o, *code as u64);
            }
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<Msg> {
        let mut r = R::new(b);
        let n = r.map()?;
        let mut tid = 0u32; let mut y = ""; let mut qname = "";
        let mut from = InfoHash::zero(); let mut target = InfoHash::zero();
        let mut token = 0u32; let mut value = None;
        let mut nodes = Vec::new(); let mut values = Vec::new(); let mut ecode = 0u32;
        for _ in 0..n {
            match r.str()? {
                "t" => tid = r.uint()? as u32,
                "y" => y = r.str()?,
                "q" => qname = r.str()?,
                "e" => ecode = r.uint()? as u32,
                "id" => from = get_hash(&mut r)?,
                "a" | "r" | "p" => {
                    let m = r.map()?;
                    for _ in 0..m {
                        match r.str()? {
                            "id" => from = get_hash(&mut r)?,
                            "target" => target = get_hash(&mut r)?,
                            "token" => token = r.uint()? as u32,
                            "value" => {
                                let _ = r.arr()?;
                                let id = r.uint()?;
                                let data = r.bin()?.to_vec();
                                value = Some(Value { id, data });
                            }
                            "nodes" => {
                                let k = r.arr()?;
                                for _ in 0..k {
                                    let _ = r.arr()?;
                                    let id = get_hash(&mut r)?;
                                    let addr = r.uint()?;
                                    nodes.push((id, addr));
                                }
                            }
                            "values" => {
                                let k = r.arr()?;
                                for _ in 0..k {
                                    let _ = r.arr()?;
                                    let id = r.uint()?;
                                    let data = r.bin()?.to_vec();
                                    values.push(Value { id, data });
                                }
                            }
                            _ => return None,
                        }
                    }
                }
                _ => return None,
            }
        }
        Some(match y {
            "q" => Msg::Query { tid, from, target, token, value,
                q: match qname { "ping" => Q::Ping, "find" => Q::Find, "get" => Q::Get,
                                 "put" => Q::Put, "listen" => Q::Listen, _ => return None } },
            "r" => Msg::Reply { tid, from, nodes, values, token },
            "u" => Msg::Push { tid, from, target, values },
            "e" => Msg::Error { tid, code: ecode },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The codec claimed OpenDHT compatibility with no test behind it. These
    // pin the claim: the message shapes the reference (`src/net.h`) uses, and
    // the failure modes a hand-rolled msgpack subset actually has.

    fn hash(b: u8) -> InfoHash {
        let mut h = [0u8; HASH_LEN];
        h[0] = b;
        h[HASH_LEN - 1] = b ^ 0xFF;
        InfoHash(h)
    }

    // Addr is a packed u64 in this crate, not a SocketAddr — checked rather
    // than assumed, after writing the test against the wrong type first.
    fn addr(p: u16) -> Addr { 0x7F00_0001_0000_0000u64 | p as u64 }

    #[test]
    fn every_query_kind_round_trips() {
        // ping, find, get, put, listen — the query names the reference sends.
        for q in [Q::Ping, Q::Find, Q::Get, Q::Put, Q::Listen] {
            let m = Msg::Query { tid: 7, from: hash(1), q: q.clone(), target: hash(2),
                                 token: 0, value: None };
            let back = Msg::decode(&m.encode()).expect("round trip");
            assert_eq!(back, m, "{q:?} did not survive");
        }
    }

    #[test]
    fn a_put_carries_its_value() {
        // The one query whose arguments include a value; dropping it would make
        // a put that stores nothing and reports success.
        let v = Value { id: 0xDEAD_BEEF, data: b"payload".to_vec() };
        let m = Msg::Query { tid: 1, from: hash(3), q: Q::Put, target: hash(4),
                             token: 99, value: Some(v.clone()) };
        match Msg::decode(&m.encode()).expect("round trip") {
            Msg::Query { value: Some(got), token, .. } => {
                assert_eq!(got, v);
                assert_eq!(token, 99);
            }
            other => panic!("decoded as {other:?}"),
        }
    }

    #[test]
    fn a_reply_round_trips_nodes_and_values_together() {
        // A find reply carries nodes; a get reply carries values; the reference
        // permits both in one message and a codec that assumed either/or would
        // silently truncate.
        let m = Msg::Reply {
            tid: 42,
            from: hash(5),
            nodes: vec![(hash(6), addr(4222)), (hash(7), addr(4223))],
            values: vec![Value { id: 1, data: vec![0xAA; 40] }],
            token: 1234,
        };
        assert_eq!(Msg::decode(&m.encode()).expect("round trip"), m);
    }

    #[test]
    fn a_listener_push_round_trips() {
        // MessageType::ValueData — unsolicited values for a key. Without it a
        // listen is a subscription nothing can deliver to.
        let m = Msg::Push { tid: 9, from: hash(8), target: hash(9),
                            values: vec![Value { id: 2, data: b"pushed".to_vec() }] };
        assert_eq!(Msg::decode(&m.encode()).expect("round trip"), m);
    }

    #[test]
    fn an_error_round_trips() {
        let m = Msg::Error { tid: 3, code: 401 };
        assert_eq!(Msg::decode(&m.encode()).expect("round trip"), m);
    }

    #[test]
    fn truncation_at_every_length_is_refused_rather_than_half_decoded() {
        // The failure mode a hand-rolled msgpack subset actually has: a length
        // prefix promising more bytes than arrived. A decoder that returned a
        // partial message would put attacker-controlled short reads into the
        // routing table.
        let m = Msg::Reply { tid: 11, from: hash(10),
                             nodes: vec![(hash(11), addr(5000))],
                             values: vec![Value { id: 3, data: vec![1, 2, 3] }],
                             token: 7 };
        let full = m.encode();
        for cut in 0..full.len() {
            assert!(Msg::decode(&full[..cut]).is_none(),
                    "a {cut}-byte prefix decoded to something");
        }
        assert!(Msg::decode(&full).is_some());
    }

    #[test]
    fn trailing_garbage_does_not_change_the_message() {
        // Datagrams get padded; the reference reads one map and stops.
        let m = Msg::Query { tid: 5, from: hash(12), q: Q::Find, target: hash(13),
                             token: 0, value: None };
        let mut b = m.encode();
        b.extend_from_slice(&[0xFF; 8]);
        assert_eq!(Msg::decode(&b), Some(m));
    }

    #[test]
    fn a_value_larger_than_a_fixstr_uses_the_wide_form() {
        // fixstr caps at 31 bytes and bin8 at 255. A value crossing both is
        // where a subset codec quietly writes the wrong header byte.
        for n in [30usize, 32, 254, 256, 4000] {
            let m = Msg::Push { tid: 1, from: hash(14), target: hash(15),
                                values: vec![Value { id: 4, data: vec![0x5A; n] }] };
            let back = Msg::decode(&m.encode()).unwrap_or_else(|| panic!("{n} bytes failed"));
            assert_eq!(back, m, "{n} bytes round-tripped wrong");
        }
    }

    #[test]
    fn a_transaction_id_survives_the_full_u32_range() {
        // tid is how a reply is matched to its query; a codec that narrowed it
        // would mismatch replies under load rather than fail.
        for tid in [0u32, 1, 255, 256, 65535, 65536, u32::MAX] {
            let m = Msg::Error { tid, code: 0 };
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }
}
