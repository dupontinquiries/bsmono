//! stunwire.rs — the socket `reflexive` was written against, and the one thing the whole
//! phone→cluster flow was blocked on.
//!
//! `docs/phone-cluster-flow.md` lists eight stages, seven of them built and tested, and says
//! the remaining blockers are all one thing: **no socket has ever been opened**. It names
//! stage 5 as the move to make first, because it is a single request and its answer decides
//! whether stage 6 ever needs a relay — "the largest unknown in the whole flow, and a
//! measurement, not a design question".
//!
//! This is that request. `reflexive::classify` already knows what to do with the answers;
//! it had no way to get one.
//!
//! ## STUN, the two percent of it that is needed
//!
//! A Binding request is a 20-byte header and nothing else. The reply carries
//! `XOR-MAPPED-ADDRESS`: the source address the server saw, XORed with the magic cookie and
//! the transaction id. That XOR exists because middleboxes used to rewrite anything that
//! looked like an IP inside a packet body — which is exactly the rewriting this request is
//! trying to measure, so a plain `MAPPED-ADDRESS` can be a lie told by the NAT itself.
//! `MAPPED-ADDRESS` is still parsed as a fallback for servers that only send it, and it is
//! marked as the less trustworthy of the two.
//!
//! Authentication, `ALTERNATE-SERVER`, TURN relaying, ICE: none of it. `peerauth` already
//! proves identity and the mailbox carries the signalling, so what remains of STUN is the
//! address echo.
//!
//! ## The transaction id is a check, not a formality
//!
//! It is 96 random bits echoed back verbatim, and it is the only thing distinguishing our
//! reply from a stray datagram arriving at a UDP socket that anyone can send to. A reply
//! whose id does not match is dropped rather than believed — otherwise an off-path attacker
//! who can guess the port gets to choose what this host believes its own address is, which
//! propagates into `natpath` and out into a mailbox that peers read.
//!
//! ## Two servers, always
//!
//! `reflexive::classify` returns `Unknown` for a single observation on purpose: one reply
//! looks like success and cannot tell endpoint-independent from symmetric. [`observe`] takes
//! a list and reports what came back; [`probe_pair`] is the ordinary call, and the pair is
//! the point.
//!
//! ## What is judgement and what is wire
//!
//! Like `lanwire`, this contains none of the former. Parsing a reply into an address is
//! wire work and is pure and tested here; deciding what the addresses MEAN is
//! `reflexive::classify`, which was already tested without a socket.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// STUN's magic cookie (RFC 5389 §6). Its presence in a reply is how a STUN message is told
/// apart from the RFC 3489 messages that had no cookie.
pub const MAGIC_COOKIE: u32 = 0x2112_A442;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// Public STUN servers, as a starting pair. Two different operators on purpose: two
/// addresses at one operator can share a front end and answer identically, which would make
/// a symmetric NAT look endpoint-independent — the classification failure that matters most,
/// because it sends `natpath` to punch when punching cannot work.
pub const DEFAULT_SERVERS: [&str; 2] = ["stun.l.google.com:19302", "stun.cloudflare.com:3478"];

/// A 96-bit transaction id.
pub type TxId = [u8; 12];

/// Build a Binding request: header only, no attributes.
pub fn binding_request(tx: &TxId) -> [u8; HEADER_LEN] {
    let mut b = [0u8; HEADER_LEN];
    b[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // length 0: no attributes
    b[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    b[8..20].copy_from_slice(tx);
    b
}

/// Where an address in a reply came from. `Xor` is the one to trust — see the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddrSource {
    Xor,
    /// Plain `MAPPED-ADDRESS`: a NAT that rewrites addresses in packet bodies can have
    /// rewritten this one, and such a NAT is precisely what we are measuring.
    Mapped,
}

/// What a reply said.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reply {
    pub addr: String,
    pub port: u16,
    pub source: AddrSource,
}

/// Why a datagram was not a usable reply.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    TooShort,
    /// Not a Binding success — an error response, or another method entirely.
    NotBindingSuccess(u16),
    NoCookie,
    /// The transaction id did not match the request. Someone else's datagram, or someone
    /// else's idea of what our address is.
    WrongTransaction,
    /// A success with no address attribute in it.
    NoAddress,
    /// An attribute claiming more bytes than the message holds.
    Truncated,
}

/// Parse a reply against the transaction id it must echo.
pub fn parse_reply(buf: &[u8], tx: &TxId) -> Result<Reply, ParseError> {
    if buf.len() < HEADER_LEN {
        return Err(ParseError::TooShort);
    }
    let kind = u16::from_be_bytes([buf[0], buf[1]]);
    if kind != BINDING_SUCCESS {
        return Err(ParseError::NotBindingSuccess(kind));
    }
    if u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) != MAGIC_COOKIE {
        return Err(ParseError::NoCookie);
    }
    if &buf[8..20] != tx.as_slice() {
        return Err(ParseError::WrongTransaction);
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let end = HEADER_LEN + len;
    if end > buf.len() {
        return Err(ParseError::Truncated);
    }
    let mut mapped: Option<Reply> = None;
    let mut i = HEADER_LEN;
    while i + 4 <= end {
        let ty = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let alen = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        let val_start = i + 4;
        let val_end = val_start + alen;
        if val_end > end {
            return Err(ParseError::Truncated);
        }
        let v = &buf[val_start..val_end];
        match ty {
            ATTR_XOR_MAPPED_ADDRESS => {
                if let Some((addr, port)) = decode_address(v, Some(tx)) {
                    // XOR wins immediately: it is the trustworthy one, and a reply carrying
                    // both should not depend on attribute order.
                    return Ok(Reply { addr, port, source: AddrSource::Xor });
                }
            }
            ATTR_MAPPED_ADDRESS => {
                if mapped.is_none() {
                    if let Some((addr, port)) = decode_address(v, None) {
                        mapped = Some(Reply { addr, port, source: AddrSource::Mapped });
                    }
                }
            }
            _ => {}
        }
        // Attributes are padded to a 4-byte boundary; the padding is not in the length.
        i = val_end + ((4 - (alen % 4)) % 4);
    }
    mapped.ok_or(ParseError::NoAddress)
}

/// Decode a MAPPED-ADDRESS-shaped value: `0x00, family, port(2), addr`. With `xor`, the
/// port and address are XORed with the cookie (and, for IPv6, the transaction id).
fn decode_address(v: &[u8], xor: Option<&TxId>) -> Option<(String, u16)> {
    if v.len() < 4 {
        return None;
    }
    let family = v[1];
    let mut port = u16::from_be_bytes([v[2], v[3]]);
    let cookie = MAGIC_COOKIE.to_be_bytes();
    if xor.is_some() {
        port ^= (MAGIC_COOKIE >> 16) as u16;
    }
    match family {
        0x01 => {
            let raw = v.get(4..8)?;
            let mut o = [raw[0], raw[1], raw[2], raw[3]];
            if xor.is_some() {
                for k in 0..4 {
                    o[k] ^= cookie[k];
                }
            }
            Some((format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]), port))
        }
        0x02 => {
            let raw = v.get(4..20)?;
            let mut o = [0u8; 16];
            o.copy_from_slice(raw);
            if let Some(tx) = xor {
                for k in 0..4 {
                    o[k] ^= cookie[k];
                }
                for k in 0..12 {
                    o[4 + k] ^= tx[k];
                }
            }
            let segs: Vec<String> = (0..8)
                .map(|k| format!("{:x}", u16::from_be_bytes([o[k * 2], o[k * 2 + 1]])))
                .collect();
            Some((segs.join(":"), port))
        }
        _ => None,
    }
}

/// A transaction id from the clock and the address of a stack local.
///
/// Not `getrandom`: this crate is std-only by rule (see its Cargo.toml), and the id's job is
/// to make a stray datagram unlikely to match, not to resist an attacker who can already see
/// the request — such an attacker can read the id off the wire whatever generated it.
pub fn new_tx() -> TxId {
    let mut id = [0u8; 12];
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    id[0..8].copy_from_slice(&t.to_le_bytes());
    let stack = &id as *const _ as usize as u32;
    id[8..12].copy_from_slice(&stack.to_le_bytes());
    id
}

/// One server's answer, or why there wasn't one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Probe {
    pub server: String,
    pub result: Result<Reply, String>,
}

/// Ask one server. `timeout` bounds the whole call.
///
/// The socket is bound by the caller so that a pair of probes goes out from the SAME local
/// port — which is the entire basis of the classification. Two probes from two sockets would
/// get two different external ports from any NAT, endpoint-independent or not, and every
/// network would look symmetric.
pub fn ask(sock: &UdpSocket, server: &str, timeout: Duration) -> Probe {
    let result = ask_inner(sock, server, timeout);
    Probe { server: server.to_string(), result: result.map_err(|e| e.to_string()) }
}

fn ask_inner(sock: &UdpSocket, server: &str, timeout: Duration) -> std::io::Result<Reply> {
    let dest: SocketAddr = server
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address for server"))?;
    let tx = new_tx();
    let req = binding_request(&tx);
    sock.set_read_timeout(Some(timeout))?;
    sock.send_to(&req, dest)?;
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1500];
    loop {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "no reply"));
        }
        let (n, from) = sock.recv_from(&mut buf)?;
        // A datagram from somewhere else, or with the wrong id, is dropped and the wait
        // continues — the deadline, not the first stray packet, ends this.
        if from.ip() != dest.ip() {
            continue;
        }
        match parse_reply(&buf[..n], &tx) {
            Ok(r) => return Ok(r),
            Err(ParseError::WrongTransaction) => continue,
            Err(e) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{e:?} from {server}"),
                ))
            }
        }
    }
}

/// Ask several servers from one socket and return every answer, in order.
pub fn observe(servers: &[&str], timeout: Duration) -> std::io::Result<Vec<Probe>> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    Ok(servers.iter().map(|s| ask(&sock, s, timeout)).collect())
}

/// The ordinary call: two servers, five seconds each, turned into the observations
/// `reflexive::classify` consumes. Probes that failed are dropped from the observation list
/// but kept in the returned probes, so a caller can tell "no NAT information" from "one
/// server was down".
pub fn probe_pair(timeout: Duration) -> std::io::Result<(Vec<Probe>, Vec<crate::reflexive::Observation>)> {
    let probes = observe(&DEFAULT_SERVERS, timeout)?;
    let obs = probes
        .iter()
        .filter_map(|p| {
            p.result.as_ref().ok().map(|r| crate::reflexive::Observation {
                from: p.server.clone(),
                addr: r.addr.clone(),
                port: r.port,
            })
        })
        .collect();
    Ok((probes, obs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xor_reply(tx: &TxId, ip: [u8; 4], port: u16) -> Vec<u8> {
        let cookie = MAGIC_COOKIE.to_be_bytes();
        let mut v = vec![0x00, 0x01];
        v.extend_from_slice(&(port ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
        for k in 0..4 {
            v.push(ip[k] ^ cookie[k]);
        }
        let mut msg = vec![0x01, 0x01];
        msg.extend_from_slice(&((4 + v.len()) as u16).to_be_bytes());
        msg.extend_from_slice(&cookie);
        msg.extend_from_slice(tx);
        msg.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        msg.extend_from_slice(&(v.len() as u16).to_be_bytes());
        msg.extend_from_slice(&v);
        msg
    }

    #[test]
    fn a_binding_request_is_a_header_and_nothing_else() {
        let tx = new_tx();
        let r = binding_request(&tx);
        assert_eq!(r.len(), 20);
        assert_eq!(u16::from_be_bytes([r[0], r[1]]), BINDING_REQUEST);
        assert_eq!(u16::from_be_bytes([r[2], r[3]]), 0, "no attributes");
        assert_eq!(u32::from_be_bytes([r[4], r[5], r[6], r[7]]), MAGIC_COOKIE);
        assert_eq!(&r[8..20], tx.as_slice());
        // two ids in quick succession must differ, or the id checks nothing
        assert_ne!(new_tx(), new_tx());
    }

    #[test]
    fn an_xor_mapped_address_decodes_to_what_the_server_saw() {
        let tx = new_tx();
        let msg = xor_reply(&tx, [203, 0, 113, 7], 54321);
        let got = parse_reply(&msg, &tx).unwrap();
        assert_eq!(got.addr, "203.0.113.7");
        assert_eq!(got.port, 54321);
        assert_eq!(got.source, AddrSource::Xor);
    }

    /// The id is the only thing separating our reply from a stray datagram. An off-path
    /// sender who guesses the port must not get to choose what this host believes its own
    /// address is — that belief propagates into `natpath` and out into a mailbox peers read.
    #[test]
    fn a_reply_for_another_transaction_is_refused() {
        let mine = new_tx();
        let theirs = {
            let mut t = mine;
            t[0] ^= 0xff;
            t
        };
        let msg = xor_reply(&theirs, [198, 51, 100, 9], 1234);
        assert_eq!(parse_reply(&msg, &mine), Err(ParseError::WrongTransaction));
        // and the right one still works, so the check is not simply refusing everything
        assert!(parse_reply(&msg, &theirs).is_ok());
    }

    #[test]
    fn a_plain_mapped_address_is_taken_but_marked_as_the_weaker_source() {
        let tx = new_tx();
        let mut v = vec![0x00, 0x01];
        v.extend_from_slice(&8080u16.to_be_bytes());
        v.extend_from_slice(&[192, 0, 2, 33]);
        let mut msg = vec![0x01, 0x01];
        msg.extend_from_slice(&((4 + v.len()) as u16).to_be_bytes());
        msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&tx);
        msg.extend_from_slice(&ATTR_MAPPED_ADDRESS.to_be_bytes());
        msg.extend_from_slice(&(v.len() as u16).to_be_bytes());
        msg.extend_from_slice(&v);
        let got = parse_reply(&msg, &tx).unwrap();
        assert_eq!((got.addr.as_str(), got.port), ("192.0.2.33", 8080));
        assert_eq!(got.source, AddrSource::Mapped);
    }

    /// A reply carrying both must not depend on which came first: XOR is the trustworthy
    /// one because a body-rewriting NAT is exactly what this request measures.
    #[test]
    fn xor_wins_over_mapped_in_either_order() {
        let tx = new_tx();
        let cookie = MAGIC_COOKIE.to_be_bytes();
        let mut xor_v = vec![0x00, 0x01];
        xor_v.extend_from_slice(&(4444u16 ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
        for k in 0..4 {
            xor_v.push([203, 0, 113, 1][k] ^ cookie[k]);
        }
        let mut map_v = vec![0x00, 0x01];
        map_v.extend_from_slice(&9999u16.to_be_bytes());
        map_v.extend_from_slice(&[10, 0, 0, 1]); // what a lying NAT rewrote it to
        for xor_first in [true, false] {
            let mut attrs = Vec::new();
            let push = |attrs: &mut Vec<u8>, ty: u16, v: &[u8]| {
                attrs.extend_from_slice(&ty.to_be_bytes());
                attrs.extend_from_slice(&(v.len() as u16).to_be_bytes());
                attrs.extend_from_slice(v);
            };
            if xor_first {
                push(&mut attrs, ATTR_XOR_MAPPED_ADDRESS, &xor_v);
                push(&mut attrs, ATTR_MAPPED_ADDRESS, &map_v);
            } else {
                push(&mut attrs, ATTR_MAPPED_ADDRESS, &map_v);
                push(&mut attrs, ATTR_XOR_MAPPED_ADDRESS, &xor_v);
            }
            let mut msg = vec![0x01, 0x01];
            msg.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
            msg.extend_from_slice(&cookie);
            msg.extend_from_slice(&tx);
            msg.extend_from_slice(&attrs);
            let got = parse_reply(&msg, &tx).unwrap();
            assert_eq!(got.addr, "203.0.113.1", "xor_first={xor_first}");
            assert_eq!(got.source, AddrSource::Xor);
        }
    }

    #[test]
    fn junk_is_refused_by_shape_before_it_is_believed() {
        let tx = new_tx();
        assert_eq!(parse_reply(&[], &tx), Err(ParseError::TooShort));
        assert_eq!(parse_reply(&[0u8; 19], &tx), Err(ParseError::TooShort));
        // an error response, not a success
        let mut err = xor_reply(&tx, [1, 2, 3, 4], 1);
        err[0..2].copy_from_slice(&0x0111u16.to_be_bytes());
        assert_eq!(parse_reply(&err, &tx), Err(ParseError::NotBindingSuccess(0x0111)));
        // no cookie: an RFC 3489 message, or noise
        let mut nc = xor_reply(&tx, [1, 2, 3, 4], 1);
        nc[4] ^= 0xff;
        assert_eq!(parse_reply(&nc, &tx), Err(ParseError::NoCookie));
        // an attribute claiming more than the message holds: the attribute length sits at
        // 22..24, right after the 20-byte header and the attribute's type
        let mut tr = xor_reply(&tx, [1, 2, 3, 4], 1);
        tr[22..24].copy_from_slice(&0x00ffu16.to_be_bytes());
        assert_eq!(parse_reply(&tr, &tx), Err(ParseError::Truncated));
        // and a MESSAGE length longer than the bytes present
        let mut tm = xor_reply(&tx, [1, 2, 3, 4], 1);
        tm[2..4].copy_from_slice(&0x0fffu16.to_be_bytes());
        assert_eq!(parse_reply(&tm, &tx), Err(ParseError::Truncated));
        // a success with no address in it
        let mut empty = vec![0x01, 0x01, 0x00, 0x00];
        empty.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        empty.extend_from_slice(&tx);
        assert_eq!(parse_reply(&empty, &tx), Err(ParseError::NoAddress));
    }

    /// The two answers feed `reflexive::classify` unchanged — this is the join the whole
    /// module exists for, and it is checked without a network.
    #[test]
    fn two_observations_classify_and_one_does_not() {
        use crate::natpath::Mapping;
        let obs = |port: u16, from: &str| crate::reflexive::Observation {
            from: from.into(),
            addr: "203.0.113.7".into(),
            port,
        };
        let same = vec![obs(54321, "a"), obs(54321, "b")];
        assert_eq!(crate::reflexive::classify(&same, &["192.168.1.5".into()]), Mapping::EndpointIndependent);
        let differ = vec![obs(54321, "a"), obs(54322, "b")];
        assert_eq!(crate::reflexive::classify(&differ, &["192.168.1.5".into()]), Mapping::Symmetric);
        assert_eq!(crate::reflexive::classify(&same[..1], &[]), Mapping::Unknown, "one probe decides nothing");
    }

    /// The socket half, against a server in this process: bind, ask, parse. No internet.
    #[test]
    fn a_local_server_is_asked_and_its_answer_read() {
        let server = UdpSocket::bind("127.0.0.1:0").expect("bind server");
        let server_addr = server.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            let (n, from) = server.recv_from(&mut buf).expect("recv");
            assert_eq!(n, 20, "a binding request is header-only");
            let mut tx = [0u8; 12];
            tx.copy_from_slice(&buf[8..20]);
            let port = from.port();
            let reply = xor_reply(&tx, [203, 0, 113, 7], port);
            server.send_to(&reply, from).expect("send");
        });
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind client");
        let mine = sock.local_addr().unwrap().port();
        let p = ask(&sock, &server_addr.to_string(), Duration::from_secs(5));
        t.join().unwrap();
        let r = p.result.expect("a reply");
        assert_eq!(r.addr, "203.0.113.7");
        assert_eq!(r.port, mine, "the server echoed the port it saw");
        assert_eq!(r.source, AddrSource::Xor);
    }

    #[test]
    fn a_server_that_says_nothing_times_out_rather_than_hanging() {
        let dead = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let addr = dead.local_addr().unwrap().to_string();
        drop(dead);
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind client");
        let start = Instant::now();
        let p = ask(&sock, &addr, Duration::from_millis(300));
        assert!(p.result.is_err());
        assert!(start.elapsed() < Duration::from_secs(3), "took {:?}", start.elapsed());
        assert_eq!(p.server, addr, "the probe says which server failed");
    }
}
