//! Hostile input for the STUN codec.
//!
//! Same contract as dht-kit's: this parser reads packets from anyone who can
//! send a datagram, so `Result` is the way to say no and a panic is not.
//!
//! nat-kit's parser is flat where dht-kit's recurses, so the stack-overflow
//! class cannot apply — but "cannot apply" is a claim worth testing rather than
//! asserting, and the attribute walk has its own arithmetic to get wrong.

use nat_kit::*;

fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

const TX: TxId = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

#[test]
fn pure_noise_never_panics() {
    for seed in 0..3_000u64 {
        for len in [0usize, 1, 19, 20, 21, 64, 300] {
            let _ = Msg::decode(&noise(seed * 17 + len as u64, len));
        }
    }
}

#[test]
fn noise_with_a_valid_header_never_panics() {
    // The dangerous shape: past the cheap checks — length, cookie, top bits —
    // and into the attribute walk, which is where the arithmetic lives.
    for seed in 0..3_000u64 {
        let body = noise(seed, 40);
        let mut b = Vec::with_capacity(60);
        b.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
        b.extend_from_slice(&(body.len() as u16).to_be_bytes());
        b.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        b.extend_from_slice(&TX);
        b.extend_from_slice(&body);
        let _ = Msg::decode(&b);
    }
}

#[test]
fn every_bit_flip_in_a_valid_message_never_panics() {
    for m in [
        Msg::BindingRequest { tx: TX },
        Msg::BindingSuccess { tx: TX, addr: Addr::V4([1, 2, 3, 4], 5678) },
        Msg::BindingSuccess { tx: TX, addr: Addr::V6([9; 16], 443) },
        Msg::BindingError { tx: TX, code: 400, reason: "Bad Request".into() },
    ] {
        let good = m.encode();
        for i in 0..good.len() {
            for bit in 0..8u8 {
                let mut b = good.clone();
                b[i] ^= 1 << bit;
                let _ = Msg::decode(&b);
            }
        }
    }
}

#[test]
fn an_attribute_length_beyond_the_packet_is_refused_not_indexed() {
    // The obvious out-of-bounds: a 4-byte attribute header claiming 65,535
    // bytes of value inside a 24-byte packet.
    let mut b = Vec::new();
    b.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    b.extend_from_slice(&4u16.to_be_bytes());
    b.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    b.extend_from_slice(&TX);
    b.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
    b.extend_from_slice(&65_535u16.to_be_bytes());
    assert!(Msg::decode(&b).is_err());
}

#[test]
fn a_zero_length_attribute_does_not_loop_forever() {
    // The subtler one: an attribute of length zero advances the cursor by the
    // 4-byte header alone. If padding arithmetic ever returned 0 for the header
    // too, the walk would spin on the same offset and hang the reader — a
    // livelock is a denial of service that no timeout on the socket catches.
    let mut b = Vec::new();
    let body: Vec<u8> = (0..8).flat_map(|_| {
        let mut a = Vec::new();
        a.extend_from_slice(&0x8022u16.to_be_bytes());
        a.extend_from_slice(&0u16.to_be_bytes());
        a
    }).collect();
    b.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    b.extend_from_slice(&(body.len() as u16).to_be_bytes());
    b.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    b.extend_from_slice(&TX);
    b.extend_from_slice(&body);
    // Terminates, and refuses because no address ever arrived.
    assert_eq!(Msg::decode(&b), Err(StunError::NoAddress));
}

#[test]
fn a_maximally_padded_attribute_walk_terminates() {
    // Every attribute length mod 4 exercised, since the pad is `(4 - n % 4) % 4`
    // and a wrong branch either overshoots the buffer or stalls.
    for n in 0..16usize {
        let mut body = Vec::new();
        body.extend_from_slice(&0x8022u16.to_be_bytes());
        body.extend_from_slice(&(n as u16).to_be_bytes());
        body.extend(std::iter::repeat(0x41).take(n));
        body.extend(std::iter::repeat(0).take((4 - n % 4) % 4));
        let mut b = Vec::new();
        b.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
        b.extend_from_slice(&(body.len() as u16).to_be_bytes());
        b.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        b.extend_from_slice(&TX);
        b.extend_from_slice(&body);
        let _ = Msg::decode(&b);
    }
}

#[test]
fn a_header_claiming_more_than_the_datagram_is_refused() {
    // The length field is attacker-controlled and 16 bits wide; the packet is
    // whatever arrived.
    let mut b = Msg::BindingRequest { tx: TX }.encode();
    b[2] = 0xFF;
    b[3] = 0xFF;
    assert_eq!(Msg::decode(&b), Err(StunError::LengthMismatch));
}
