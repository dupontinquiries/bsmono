//! Wire protocol — pure (de)serialization, no I/O.
//!
//! Two packet kinds travel between peers:
//!
//! - **Input**: a batch of one player's recent inputs. Rollback wants the
//!   *freshest* inputs, so we use UDP and tolerate loss by **redundancy** —
//!   every packet repeats the last several ticks of input. A lost packet is
//!   recovered by the next one without any retransmission handshake, as long
//!   as the redundancy window is larger than the loss burst. Inputs are tiny,
//!   so resending a dozen per packet is free on a LAN.
//! - **Sync**: a `(tick, checksum)` pair for a *confirmed* tick, exchanged
//!   periodically so peers can detect a desync (the two simulations diverging
//!   despite identical inputs — which should never happen, but if a
//!   determinism bug slips in, this is how it surfaces).
//!
//! Everything is fixed-layout little-endian and version-tagged. [`decode`]
//! never panics and never trusts the input: a malformed, truncated, or
//! hostile packet returns `None`, and the declared input count is capped so a
//! bad packet can't trigger a huge allocation.

use crate::sim::fixed::Fixed;
use crate::sim::state::Input;

/// Identifies our packets ("UN"). First two bytes of every datagram.
pub const PROTOCOL_MAGIC: u16 = 0x554E;
/// Bumped on any wire-format change; mismatched versions are dropped.
pub const PROTOCOL_VERSION: u8 = 1;

const KIND_INPUT: u8 = 0;
const KIND_SYNC: u8 = 1;

/// Wire size of one [`Input`]: buttons(4) + move_x(8) + move_z(8) + yaw(2).
const INPUT_WIRE_LEN: usize = 22;
/// Cap on inputs per packet, so a malformed `count` can't blow up allocation.
const MAX_INPUTS_PER_PACKET: usize = 256;

/// A decoded packet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Packet {
    /// A batch of `player`'s inputs for ticks `start_tick .. start_tick+len`.
    /// `ack` is the sender's confirmed tick (lets the receiver prune the
    /// inputs it no longer needs to resend).
    Input {
        player: u8,
        ack: u64,
        start_tick: u64,
        inputs: Vec<Input>,
    },
    /// The sender's checksum for confirmed `tick`.
    Sync { tick: u64, checksum: u64 },
}

/// Encode a batch of one player's inputs covering contiguous ticks starting
/// at `start_tick`.
pub fn encode_input_packet(player: u8, ack: u64, start_tick: u64, inputs: &[Input]) -> Vec<u8> {
    let mut b = Vec::with_capacity(15 + inputs.len() * INPUT_WIRE_LEN);
    b.extend_from_slice(&PROTOCOL_MAGIC.to_le_bytes());
    b.push(PROTOCOL_VERSION);
    b.push(KIND_INPUT);
    b.push(player);
    b.extend_from_slice(&ack.to_le_bytes());
    b.extend_from_slice(&start_tick.to_le_bytes());
    b.extend_from_slice(&(inputs.len() as u16).to_le_bytes());
    for inp in inputs {
        write_input(&mut b, inp);
    }
    b
}

/// Encode a confirmed-tick checksum for desync detection.
pub fn encode_sync_packet(tick: u64, checksum: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(20);
    b.extend_from_slice(&PROTOCOL_MAGIC.to_le_bytes());
    b.push(PROTOCOL_VERSION);
    b.push(KIND_SYNC);
    b.extend_from_slice(&tick.to_le_bytes());
    b.extend_from_slice(&checksum.to_le_bytes());
    b
}

/// Decode a datagram. Returns `None` on bad magic/version, unknown kind,
/// truncation, or an implausible input count.
pub fn decode(bytes: &[u8]) -> Option<Packet> {
    let mut r = Reader::new(bytes);
    if r.u16()? != PROTOCOL_MAGIC {
        return None;
    }
    if r.u8()? != PROTOCOL_VERSION {
        return None;
    }
    match r.u8()? {
        KIND_INPUT => {
            let player = r.u8()?;
            let ack = r.u64()?;
            let start_tick = r.u64()?;
            let count = r.u16()? as usize;
            if count > MAX_INPUTS_PER_PACKET {
                return None;
            }
            let mut inputs = Vec::with_capacity(count);
            for _ in 0..count {
                inputs.push(read_input(&mut r)?);
            }
            Some(Packet::Input { player, ack, start_tick, inputs })
        }
        KIND_SYNC => {
            let tick = r.u64()?;
            let checksum = r.u64()?;
            Some(Packet::Sync { tick, checksum })
        }
        _ => None,
    }
}

fn write_input(b: &mut Vec<u8>, i: &Input) {
    b.extend_from_slice(&i.buttons.to_le_bytes());
    b.extend_from_slice(&i.move_x.0.to_le_bytes());
    b.extend_from_slice(&i.move_z.0.to_le_bytes());
    b.extend_from_slice(&i.yaw.to_le_bytes());
}

fn read_input(r: &mut Reader) -> Option<Input> {
    let buttons = r.u32()?;
    let mx = r.i64()?;
    let mz = r.i64()?;
    let yaw = r.u16()?;
    Some(Input { buttons, move_x: Fixed(mx), move_z: Fixed(mz), yaw })
}

/// Bounds-checked little-endian cursor. Every read returns `None` rather than
/// panicking if the buffer is too short.
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(buttons: u32, mx: i64, mz: i64, yaw: u16) -> Input {
        Input { buttons, move_x: Fixed(mx), move_z: Fixed(mz), yaw }
    }

    #[test]
    fn input_packet_round_trips() {
        let batch = vec![inp(1, 65536, -65536, 100), inp(0, 0, 0, 0), inp(2, 123, 456, 65535)];
        let bytes = encode_input_packet(1, 42, 1000, &batch);
        match decode(&bytes) {
            Some(Packet::Input { player, ack, start_tick, inputs }) => {
                assert_eq!(player, 1);
                assert_eq!(ack, 42);
                assert_eq!(start_tick, 1000);
                assert_eq!(inputs, batch);
            }
            other => panic!("expected Input, got {:?}", other),
        }
    }

    #[test]
    fn sync_packet_round_trips() {
        let bytes = encode_sync_packet(7777, 0xdead_beef_cafe_f00d);
        assert_eq!(decode(&bytes), Some(Packet::Sync { tick: 7777, checksum: 0xdead_beef_cafe_f00d }));
    }

    #[test]
    fn empty_input_batch_round_trips() {
        let bytes = encode_input_packet(0, 0, 0, &[]);
        assert_eq!(decode(&bytes), Some(Packet::Input { player: 0, ack: 0, start_tick: 0, inputs: vec![] }));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode_sync_packet(1, 2);
        bytes[0] ^= 0xff;
        assert_eq!(decode(&bytes), None);
    }

    #[test]
    fn rejects_wrong_version() {
        let mut bytes = encode_sync_packet(1, 2);
        bytes[2] = PROTOCOL_VERSION.wrapping_add(1);
        assert_eq!(decode(&bytes), None);
    }

    #[test]
    fn rejects_truncation_at_every_length() {
        let full = encode_input_packet(1, 5, 10, &[inp(1, 2, 3, 4), inp(5, 6, 7, 8)]);
        // Any prefix shorter than the whole must decode to None, never panic.
        for n in 0..full.len() {
            assert_eq!(decode(&full[..n]), None, "prefix len {} should be rejected", n);
        }
        assert!(decode(&full).is_some(), "the full packet still decodes");
    }

    #[test]
    fn rejects_absurd_count_without_allocating() {
        // Hand-craft an input header claiming 60000 inputs but with no body.
        let mut b = Vec::new();
        b.extend_from_slice(&PROTOCOL_MAGIC.to_le_bytes());
        b.push(PROTOCOL_VERSION);
        b.push(KIND_INPUT);
        b.push(0);
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&60000u16.to_le_bytes());
        assert_eq!(decode(&b), None);
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let mut b = Vec::new();
        b.extend_from_slice(&PROTOCOL_MAGIC.to_le_bytes());
        b.push(PROTOCOL_VERSION);
        b.push(99); // unknown kind
        assert_eq!(decode(&b), None);
    }
}
