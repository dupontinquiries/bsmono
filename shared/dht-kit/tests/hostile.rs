//! Hostile input: the decoder must never panic, whatever arrives.
//!
//! A DHT decoder reads packets from anyone who can send a datagram. Every
//! `unwrap`, every slice index, every `a - b` on unsigned integers is a remote
//! denial of service if it can be reached from the wire. `Option` return is the
//! contract; a panic is not a way of saying no.
//!
//! Deterministic rather than randomised: a fuzz that finds a crash on Tuesday
//! and not on Wednesday cannot be a regression test. The generator is a
//! xorshift with fixed seeds, so a failure here reproduces exactly.

use dht_kit::*;

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len).map(|_| (xorshift(&mut s) >> 24) as u8).collect()
}

#[test]
fn pure_noise_never_panics() {
    // The baseline. Most of these fail at the first byte, which is the point:
    // failing fast must still be failing, not indexing.
    for seed in 0..2_000u64 {
        for len in [0usize, 1, 3, 20, 64, 300] {
            let b = noise(seed * 31 + len as u64, len);
            let _ = Msg::decode(&b);
        }
    }
}

#[test]
fn valid_messages_with_one_byte_corrupted_never_panic() {
    // The dangerous shape: a packet that gets far enough into the parser to
    // reach the interesting code before going wrong. Pure noise rarely does.
    let msgs = vec![
        Msg::Query { tid: 7, from: InfoHash::from_seed(1), query: Query::Get,
                     target: InfoHash::from_seed(2), token: 5,
                     value: Some(Value { id: 9, data: vec![1, 2, 3, 4] }) },
        Msg::Reply { tid: 8, from: InfoHash::from_seed(3),
                     nodes: vec![(InfoHash::from_seed(4), Addr::new([10, 0, 0, 1], 4222))],
                     values: vec![Value { id: 1, data: vec![0xAB; 300] }], token: 1 },
        Msg::Push { tid: 9, from: InfoHash::from_seed(5), target: InfoHash::from_seed(6),
                    values: vec![Value { id: 2, data: vec![7; 70_000] }] },
        Msg::Error { tid: 10, code: 401 },
    ];
    for m in &msgs {
        let good = m.encode();
        for i in 0..good.len().min(400) {
            for bit in 0..8u8 {
                let mut b = good.clone();
                b[i] ^= 1 << bit;
                let _ = Msg::decode(&b);
            }
        }
    }
}

#[test]
fn every_truncation_of_a_valid_message_never_panics() {
    let m = Msg::Reply { tid: 8, from: InfoHash::from_seed(3),
                         nodes: vec![(InfoHash::from_seed(4), Addr::new([10, 0, 0, 1], 4222));
                                     8],
                         values: vec![Value { id: 1, data: vec![0xAB; 500] }], token: 1 };
    let good = m.encode();
    for cut in 0..good.len() {
        assert!(Msg::decode(&good[..cut]).is_none(), "a {cut}-byte prefix decoded");
    }
}

#[test]
fn a_declared_length_far_beyond_the_packet_allocates_nothing() {
    // The classic hand-rolled-parser bomb: a 5-byte packet claiming a 4GB
    // value. If the decoder reserves before reading, one datagram is an OOM.
    let mut b = Vec::new();
    b.push(0x81);                       // fixmap(1)
    b.extend_from_slice(&[0xA1, b't']); // "t"
    b.push(0xC6);                       // bin32
    b.extend_from_slice(&u32::MAX.to_be_bytes());
    assert!(Msg::decode(&b).is_none());
    assert!(b.len() < 16, "the attack packet is tiny: {} bytes", b.len());
}

#[test]
fn a_huge_declared_array_count_does_not_allocate_per_element() {
    // array16 promising 65,535 values inside a packet with none. A decoder
    // that sized a Vec from the count would allocate megabytes from 30 bytes.
    let mut b = Vec::new();
    b.push(0x83);
    b.extend_from_slice(&[0xA1, b't']); b.push(1);
    b.extend_from_slice(&[0xA1, b'y']); b.extend_from_slice(&[0xA1, b'r']);
    b.extend_from_slice(&[0xA1, b'r']);
    b.push(0x81);
    b.extend_from_slice(&[0xA6]); b.extend_from_slice(b"values");
    b.push(0xDC); b.extend_from_slice(&65_535u16.to_be_bytes());
    assert!(Msg::decode(&b).is_none());
    assert!(b.len() < 40);
}

#[test]
fn deeply_nested_unknown_values_do_not_blow_the_stack() {
    // `skip` recurses through nested maps and arrays. A packet that is nothing
    // but nesting is the cheapest possible stack-overflow attempt, and a
    // decoder that recursed without bound would abort the process — which
    // `Option` cannot express and a caller cannot catch.
    // 100,000 was found to abort the process before MAX_NESTING existed: a
    // 100KB packet of nothing but fixarray headers. The depths here bracket
    // that, and 500,000 is well past any plausible stack.
    for depth in [8usize, 64, 512, 1_000, 100_000, 500_000] {
        let mut b = Vec::new();
        b.push(0x84);
        b.extend_from_slice(&[0xA1, b't']); b.push(1);
        b.extend_from_slice(&[0xA1, b'y']); b.extend_from_slice(&[0xA1, b'q']);
        b.extend_from_slice(&[0xA1, b'q']); b.extend_from_slice(&[0xA4]);
        b.extend_from_slice(b"find");
        b.extend_from_slice(&[0xA1, b'a']);
        b.push(0x81);
        b.extend_from_slice(&[0xA1, b'x']);
        for _ in 0..depth { b.push(0x91); }   // fixarray(1), repeatedly
        b.push(0xC0);                          // nil at the bottom
        // Refused rather than crashed, and refusing is the only outcome a
        // caller can act on — a stack overflow aborts the process and no
        // `Option` can report it.
        assert!(Msg::decode(&b).is_none(), "depth {depth} decoded");
    }
}

#[test]
fn nesting_just_inside_the_limit_is_still_accepted() {
    // The bound must not be so tight it rejects legitimate growth. OpenDHT
    // nests one level today; this proves a message nesting a few deep inside an
    // unknown key still parses.
    let mut b = Vec::new();
    b.push(0x84);
    b.extend_from_slice(&[0xA1, b't']); b.push(1);
    b.extend_from_slice(&[0xA1, b'y']); b.extend_from_slice(&[0xA1, b'q']);
    b.extend_from_slice(&[0xA1, b'q']); b.extend_from_slice(&[0xA4]);
    b.extend_from_slice(b"find");
    b.extend_from_slice(&[0xA1, b'a']);
    b.push(0x83);
    b.extend_from_slice(&[0xA1, b'x']);
    for _ in 0..8 { b.push(0x91); }
    b.push(0xC0);
    b.extend_from_slice(&[0xA2, b'i', b'd']);
    b.push(0xC4); b.push(20); b.extend_from_slice(&[7u8; 20]);
    b.extend_from_slice(&[0xA1, b'h']);
    b.push(0xC4); b.push(20); b.extend_from_slice(&[8u8; 20]);
    assert!(Msg::decode(&b).is_some(), "eight levels inside an unknown key must parse");
}

#[test]
fn a_value_id_at_the_u64_boundary_round_trips() {
    // Per-value ids are u64 on the wire. A decoder narrowing to u32 would make
    // two distinct values collide under one key, which is data loss rather
    // than a parse error.
    for id in [0u64, 1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
        let m = Msg::Push { tid: 1, from: InfoHash::from_seed(1),
                            target: InfoHash::from_seed(2),
                            values: vec![Value { id, data: vec![1] }] };
        assert_eq!(Msg::decode(&m.encode()), Some(m), "id {id}");
    }
}
