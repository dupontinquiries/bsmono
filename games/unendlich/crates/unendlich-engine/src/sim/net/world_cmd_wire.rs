//! `sim::net::world_cmd_wire` — wire codec for authority commands + remote apply.
//!
//! [`world_authority`](crate::sim::world_authority) is the command-sourced world
//! model: clients send *commands* ("I placed block X at Y"), the host validates
//! each against canonical state. But unlike `chunk_stream` / `entity_snapshot`,
//! the commands had no wire form, so they couldn't cross the reliable channel.
//! This is that codec, plus the host-side compose: decode an ordered command
//! frame and fold it through [`CanonicalWorld::apply`], yielding per-command
//! accept/reject. World edits ride the **reliable-ordered** channel (vs the
//! unreliable-sequenced transform path in `entity_replication`), so a frame is an
//! ordered batch and the batch preserves submission order.
//!
//! Pure: fixed little-endian layout (magic `0x554E` "UN", v1), counts/fields
//! length-capped, `decode_*` returns `None` on any malformed/truncated/hostile
//! input — never panics, never over-allocates.

use crate::sim::world_authority::{BlockId, PlayerId, Pos, Reject, WorldCmd};

pub const CMD_MAGIC: u16 = 0x554E;
pub const CMD_VERSION: u8 = 1;

/// Cap on commands in one frame so a hostile count can't blow allocation.
pub const MAX_CMDS_PER_FRAME: usize = 4096;

const TAG_PLACE: u8 = 0;
const TAG_REMOVE: u8 = 1;
const TAG_GRANT: u8 = 2;

fn write_one(b: &mut Vec<u8>, player: PlayerId, cmd: &WorldCmd) {
    b.extend_from_slice(&player.to_le_bytes());
    match cmd {
        WorldCmd::Place { pos, block } => {
            b.push(TAG_PLACE);
            write_pos(b, *pos);
            b.extend_from_slice(&block.to_le_bytes());
        }
        WorldCmd::Remove { pos } => {
            b.push(TAG_REMOVE);
            write_pos(b, *pos);
        }
        WorldCmd::Grant { amount } => {
            b.push(TAG_GRANT);
            b.extend_from_slice(&amount.to_le_bytes());
        }
    }
}

fn write_pos(b: &mut Vec<u8>, p: Pos) {
    b.extend_from_slice(&p.0.to_le_bytes());
    b.extend_from_slice(&p.1.to_le_bytes());
    b.extend_from_slice(&p.2.to_le_bytes());
}

/// Encode an ordered batch of `(player, command)` for the reliable channel.
/// Commands beyond [`MAX_CMDS_PER_FRAME`] are dropped (split across frames).
pub fn encode_batch(cmds: &[(PlayerId, WorldCmd)]) -> Vec<u8> {
    let n = cmds.len().min(MAX_CMDS_PER_FRAME) as u32;
    let mut b = Vec::with_capacity(7 + n as usize * 23);
    b.extend_from_slice(&CMD_MAGIC.to_le_bytes());
    b.push(CMD_VERSION);
    b.extend_from_slice(&n.to_le_bytes());
    for (p, c) in cmds.iter().take(n as usize) {
        write_one(&mut b, *p, c);
    }
    b
}

/// Encode a single command (a batch of one).
pub fn encode_cmd(player: PlayerId, cmd: &WorldCmd) -> Vec<u8> {
    encode_batch(&[(player, *cmd)])
}

// ── Bounded reader (mirrors sim::net::wire; never panics) ────────────────────
struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, i: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.i.checked_add(n)?;
        let s = self.b.get(self.i..end)?;
        self.i = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|s| {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_le_bytes(a)
        })
    }
}

fn read_one(r: &mut Reader) -> Option<(PlayerId, WorldCmd)> {
    let player = r.u64()?;
    let cmd = match r.u8()? {
        TAG_PLACE => {
            let pos = (r.i32()?, r.i32()?, r.i32()?);
            let block: BlockId = r.u16()?;
            WorldCmd::Place { pos, block }
        }
        TAG_REMOVE => {
            let pos = (r.i32()?, r.i32()?, r.i32()?);
            WorldCmd::Remove { pos }
        }
        TAG_GRANT => WorldCmd::Grant { amount: r.u32()? },
        _ => return None,
    };
    Some((player, cmd))
}

/// Decode an ordered command batch. `None` on bad magic/version, an over-cap
/// count, or any truncation.
pub fn decode_batch(buf: &[u8]) -> Option<Vec<(PlayerId, WorldCmd)>> {
    let mut r = Reader::new(buf);
    if r.u16()? != CMD_MAGIC || r.u8()? != CMD_VERSION {
        return None;
    }
    let n = r.u32()? as usize;
    if n > MAX_CMDS_PER_FRAME {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(read_one(&mut r)?);
    }
    Some(out)
}

/// Decode a frame expected to hold exactly one command (`None` otherwise).
pub fn decode_cmd(buf: &[u8]) -> Option<(PlayerId, WorldCmd)> {
    let v = decode_batch(buf)?;
    match v.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// Host: decode a command frame and fold it through canonical state in order,
/// returning each command's outcome (`Ok` applied, `Err` rejected with reason).
/// `None` if the frame itself was malformed (decoded nothing to apply).
pub fn apply_remote(
    world: &mut crate::sim::world_authority::CanonicalWorld,
    buf: &[u8],
) -> Option<Vec<(PlayerId, WorldCmd, Result<(), Reject>)>> {
    let cmds = decode_batch(buf)?;
    Some(
        cmds.into_iter()
            .map(|(p, c)| {
                let r = world.apply(p, c);
                (p, c, r)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::world_authority::CanonicalWorld;

    #[test]
    fn roundtrips_each_command_variant() {
        let cmds = vec![
            (1u64, WorldCmd::Place { pos: (1, -2, 3), block: 42 }),
            (2u64, WorldCmd::Remove { pos: (-9, 8, -7) }),
            (3u64, WorldCmd::Grant { amount: 100 }),
        ];
        let got = decode_batch(&encode_batch(&cmds)).expect("decodes");
        assert_eq!(got, cmds);
    }

    #[test]
    fn single_cmd_helper() {
        let c = WorldCmd::Place { pos: (5, 5, 5), block: 7 };
        assert_eq!(decode_cmd(&encode_cmd(9, &c)), Some((9, c)));
        // a 3-command batch is not a single cmd
        assert!(decode_cmd(&encode_batch(&[(1, c), (2, c), (3, c)])).is_none());
    }

    #[test]
    fn rejects_bad_header_and_truncation() {
        assert!(decode_batch(b"").is_none());
        assert!(decode_batch(&[0, 0, 1, 0, 0, 0, 0]).is_none()); // bad magic
        let mut good = encode_cmd(1, &WorldCmd::Remove { pos: (0, 0, 0) });
        good.pop(); // truncate
        assert!(decode_batch(&good).is_none());
        // unknown tag
        let mut bad = Vec::new();
        bad.extend_from_slice(&CMD_MAGIC.to_le_bytes());
        bad.push(CMD_VERSION);
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&7u64.to_le_bytes());
        bad.push(250); // unknown tag
        assert!(decode_batch(&bad).is_none());
    }

    #[test]
    fn overlong_count_rejected_without_allocating() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&CMD_MAGIC.to_le_bytes());
        buf.push(CMD_VERSION);
        buf.extend_from_slice(&(MAX_CMDS_PER_FRAME as u32 + 1).to_le_bytes());
        assert!(decode_batch(&buf).is_none());
    }

    #[test]
    fn apply_remote_validates_against_canonical_state() {
        let mut world = CanonicalWorld::new();
        // No resources yet → a Place is rejected (NoResource); Grant then Place succeeds.
        let frame = encode_batch(&[
            (1, WorldCmd::Place { pos: (0, 0, 0), block: 1 }), // rejected: no resource
            (1, WorldCmd::Grant { amount: 5 }),                // ok
            (1, WorldCmd::Place { pos: (0, 0, 0), block: 1 }), // ok now
            (1, WorldCmd::Place { pos: (0, 0, 0), block: 2 }), // rejected: occupied
        ]);
        let results = apply_remote(&mut world, &frame).expect("frame decodes");
        assert!(matches!(results[0].2, Err(Reject::NoResource)));
        assert!(results[1].2.is_ok());
        assert!(results[2].2.is_ok());
        assert!(matches!(results[3].2, Err(Reject::Occupied)));
        assert_eq!(world.blocks.get(&(0, 0, 0)), Some(&1));
    }

    #[test]
    fn apply_remote_preserves_order_and_is_replayable() {
        // Same frame applied to a fresh world reproduces the same digest — the
        // ordered reliable channel + canonical apply is deterministic.
        let frame = encode_batch(&[
            (7, WorldCmd::Grant { amount: 3 }),
            (7, WorldCmd::Place { pos: (2, 0, 1), block: 9 }),
            (7, WorldCmd::Place { pos: (2, 1, 1), block: 9 }),
        ]);
        let mut a = CanonicalWorld::new();
        let mut b = CanonicalWorld::new();
        apply_remote(&mut a, &frame).unwrap();
        apply_remote(&mut b, &frame).unwrap();
        assert_eq!(a.digest(), b.digest());
        assert!(apply_remote(&mut a, b"garbage").is_none());
    }
}
