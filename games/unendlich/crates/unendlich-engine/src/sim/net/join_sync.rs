//! `sim::net::join_sync` — join a server **at any time** and receive the
//! world's edits in the **timebending storage layout**.
//!
//! The problem: the LAN handshake transfers `(world_name, seed)` and the
//! joiner regenerates — correct only for a pristine world at genesis. Every
//! block placed before the join is silently missing on the joiner. "Join at
//! any time" means the host must hand over its divergence from the seed.
//!
//! The layout (per `docs/design/timebending_storage_integration.md` §2c and
//! the set-algebra doc): the world is *reconstructive* — procedural base +
//! a per-chunk edit overlay (`storage.rs` `.mks`: `(index, block)`); deep
//! time upgrades that overlay to a **tick-stamped edit log** `(tick, index,
//! block)` with the rule *un-stamped edit = tick 0*, and a timebender reads
//! it through **scoops** (bounded tick-interval sets, `time_interval_set`)
//! and **forks** (`sim::timeline`'s branch DAG). This wire format IS that
//! layout: today's timeless overlay ships with `tick: 0`, and the moment
//! storage grows its time axis the same records carry real stamps — the
//! join pipe, the save file, and the timebender's queries stay one shape.
//! `edits_at_or_before(T)` is the doc's "load tick T" rule; [`scoop`] is the
//! timebender's interval query, verbatim over `TimeSet`.
//!
//! Pure: codec + filters + pacing plan. The sockets live in `lan_host_pump`
//! / the fullgame loop; a paced [`SyncPlan`] keeps catch-up from stealing
//! the frame (a heavily built world is thousands of edits).

use crate::sim::time_interval_set::TimeSet;
use crate::world::chunk::ChunkCoord;

/// One stamped edit inside a chunk — the storage-integration doc's
/// `(tick, index, block)` row. `tick: 0` = the legacy timeless overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StampedEdit {
    pub tick: u64,
    pub index: u16,
    pub block: u8,
}

/// A chunk's edit log — the `.mks` record shape with the time axis added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkEditRecord {
    pub coord: ChunkCoord,
    pub edits: Vec<StampedEdit>,
}

/// Wire magic for a join-sync batch ("UJS1": Unendlich Join Sync v1).
const MAGIC: &[u8; 4] = b"UJS1";

/// Encode a batch of records. Layout, all little-endian:
/// `MAGIC` · `chunk_count u16` · per chunk: `x i32, y i32, z i32,
/// edit_count u16`, then per edit `tick u64, index u16, block u8`.
pub fn encode_records(records: &[ChunkEditRecord]) -> Vec<u8> {
    let mut o = Vec::with_capacity(8 + records.len() * 32);
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&(records.len() as u16).to_le_bytes());
    for r in records {
        o.extend_from_slice(&r.coord.x.to_le_bytes());
        o.extend_from_slice(&r.coord.y.to_le_bytes());
        o.extend_from_slice(&r.coord.z.to_le_bytes());
        o.extend_from_slice(&(r.edits.len() as u16).to_le_bytes());
        for e in &r.edits {
            o.extend_from_slice(&e.tick.to_le_bytes());
            o.extend_from_slice(&e.index.to_le_bytes());
            o.push(e.block);
        }
    }
    o
}

/// Decode a batch. Tolerates trailing bytes (the wire crate's convention);
/// returns None on truncation or bad magic — a malformed datagram must never
/// corrupt a world.
pub fn decode_records(data: &[u8]) -> Option<Vec<ChunkEditRecord>> {
    if data.len() < 6 || &data[0..4] != MAGIC {
        return None;
    }
    let mut i = 4usize;
    let take = |i: &mut usize, n: usize| -> Option<&[u8]> {
        let s = data.get(*i..*i + n)?;
        *i += n;
        Some(s)
    };
    let count = u16::from_le_bytes(take(&mut i, 2)?.try_into().ok()?) as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let x = i32::from_le_bytes(take(&mut i, 4)?.try_into().ok()?);
        let y = i32::from_le_bytes(take(&mut i, 4)?.try_into().ok()?);
        let z = i32::from_le_bytes(take(&mut i, 4)?.try_into().ok()?);
        let n = u16::from_le_bytes(take(&mut i, 2)?.try_into().ok()?) as usize;
        let mut edits = Vec::with_capacity(n);
        for _ in 0..n {
            let tick = u64::from_le_bytes(take(&mut i, 8)?.try_into().ok()?);
            let index = u16::from_le_bytes(take(&mut i, 2)?.try_into().ok()?);
            let block = *take(&mut i, 1)?.first()?;
            edits.push(StampedEdit { tick, index, block });
        }
        out.push(ChunkEditRecord { coord: ChunkCoord::new(x, y, z), edits });
    }
    Some(out)
}

/// The storage doc's load rule: to materialise tick `t`, apply only edits with
/// `edit_tick <= t`. Records that end up empty are dropped. Today's overlay is
/// all tick-0, so any `t` keeps everything — exactly the backward-compat rule.
pub fn edits_at_or_before(records: &[ChunkEditRecord], t: u64) -> Vec<ChunkEditRecord> {
    records
        .iter()
        .filter_map(|r| {
            let edits: Vec<StampedEdit> = r.edits.iter().copied().filter(|e| e.tick <= t).collect();
            if edits.is_empty() {
                None
            } else {
                Some(ChunkEditRecord { coord: r.coord, edits })
            }
        })
        .collect()
}

/// The timebender's **scoop**: keep only the edits whose ticks fall inside the
/// interval set. This is how "impact the world across time" reads the log —
/// union/intersect/subtract `TimeSet`s upstream, then scoop the stamped edits
/// the resulting set covers. (Ticks beyond i64::MAX are outside any bounded
/// scoop by definition.)
pub fn scoop(records: &[ChunkEditRecord], set: &TimeSet) -> Vec<ChunkEditRecord> {
    records
        .iter()
        .filter_map(|r| {
            let edits: Vec<StampedEdit> = r
                .edits
                .iter()
                .copied()
                .filter(|e| i64::try_from(e.tick).map(|t| set.contains(t)).unwrap_or(false))
                .collect();
            if edits.is_empty() {
                None
            } else {
                Some(ChunkEditRecord { coord: r.coord, edits })
            }
        })
        .collect()
}

/// A paced transmission plan: batches sized under a datagram budget, drained a
/// few per frame so a big catch-up doesn't stall the host's frame or flood the
/// joiner. Deterministic given the same records.
pub struct SyncPlan {
    batches: Vec<Vec<u8>>,
    next: usize,
    pub batches_per_frame: usize,
}

/// Bytes of encoded payload we allow per datagram, leaving room for the
/// channel byte + compression header under a typical 1500-byte MTU.
pub const BATCH_BUDGET_BYTES: usize = 1100;

impl SyncPlan {
    /// Split records into MTU-sized encoded batches. A single chunk whose log
    /// exceeds the budget is split across batches by edit runs (same coord
    /// repeated) — decoders just apply both.
    pub fn new(records: &[ChunkEditRecord], batches_per_frame: usize) -> SyncPlan {
        let mut batches = Vec::new();
        let mut cur: Vec<ChunkEditRecord> = Vec::new();
        let mut cur_bytes = 6usize; // magic + count
        let mut push_cur = |cur: &mut Vec<ChunkEditRecord>, cur_bytes: &mut usize, batches: &mut Vec<Vec<u8>>| {
            if !cur.is_empty() {
                batches.push(encode_records(cur));
                cur.clear();
                *cur_bytes = 6;
            }
        };
        for r in records {
            // Split oversized chunk logs into budget-sized runs.
            let per_edit = 11usize; // tick 8 + index 2 + block 1
            let header = 14usize; // coord 12 + count 2
            let max_edits_per_run = ((BATCH_BUDGET_BYTES - 6 - header) / per_edit).max(1);
            for run in r.edits.chunks(max_edits_per_run) {
                let run_bytes = header + run.len() * per_edit;
                if cur_bytes + run_bytes > BATCH_BUDGET_BYTES {
                    push_cur(&mut cur, &mut cur_bytes, &mut batches);
                }
                cur.push(ChunkEditRecord { coord: r.coord, edits: run.to_vec() });
                cur_bytes += run_bytes;
            }
        }
        push_cur(&mut cur, &mut cur_bytes, &mut batches);
        SyncPlan { batches, next: 0, batches_per_frame: batches_per_frame.max(1) }
    }

    /// The next frame's worth of datagram payloads (empty when done).
    pub fn drain_frame(&mut self) -> &[Vec<u8>] {
        let start = self.next;
        let end = (start + self.batches_per_frame).min(self.batches.len());
        self.next = end;
        &self.batches[start..end]
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.batches.len()
    }

    pub fn remaining(&self) -> usize {
        self.batches.len() - self.next
    }

    pub fn total_batches(&self) -> usize {
        self.batches.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::time_interval_set::{Interval, TimeSet};

    fn rec(x: i32, edits: &[(u64, u16, u8)]) -> ChunkEditRecord {
        ChunkEditRecord {
            coord: ChunkCoord::new(x, 0, 0),
            edits: edits.iter().map(|&(tick, index, block)| StampedEdit { tick, index, block }).collect(),
        }
    }

    #[test]
    fn roundtrips_the_storage_layout() {
        let records = vec![
            rec(0, &[(0, 5, 3), (0, 9, 0)]),
            rec(-7, &[(1200, 4095, 12)]),
        ];
        let enc = encode_records(&records);
        assert_eq!(decode_records(&enc).as_deref(), Some(&records[..]));
        // Wire convention: trailing bytes are tolerated (future extension).
        let mut padded = enc.clone();
        padded.extend_from_slice(&[0xAA; 7]);
        assert_eq!(decode_records(&padded).as_deref(), Some(&records[..]));
    }

    #[test]
    fn truncation_and_bad_magic_decode_to_none() {
        let enc = encode_records(&[rec(1, &[(0, 1, 1)])]);
        for cut in 1..enc.len() {
            // Any truncation must fail cleanly, never panic or half-apply.
            assert_eq!(decode_records(&enc[..cut]), None, "cut at {cut}");
        }
        let mut bad = enc.clone();
        bad[0] = b'X';
        assert_eq!(decode_records(&bad), None);
    }

    #[test]
    fn unstamped_overlay_is_tick_zero_and_survives_any_load_tick() {
        // The storage doc's backward-compat rule: un-stamped edit = tick 0,
        // so a timeless overlay is visible at EVERY requested tick.
        let records = vec![rec(0, &[(0, 1, 1), (0, 2, 2)])];
        for t in [0u64, 1, 10_000, u64::MAX] {
            assert_eq!(edits_at_or_before(&records, t), records, "t={t}");
        }
    }

    #[test]
    fn load_tick_filter_drops_the_future() {
        let records = vec![rec(0, &[(0, 1, 1), (500, 2, 2), (900, 3, 3)])];
        let at_600 = edits_at_or_before(&records, 600);
        assert_eq!(at_600, vec![rec(0, &[(0, 1, 1), (500, 2, 2)])]);
        // Before the first stamped edit: only the tick-0 base overlay.
        assert_eq!(edits_at_or_before(&records, 0), vec![rec(0, &[(0, 1, 1)])]);
    }

    #[test]
    fn a_scoop_reads_exactly_its_intervals() {
        // The timebender's query: a TimeSet scoop over the stamped log.
        let records = vec![rec(0, &[(100, 1, 1), (250, 2, 2), (400, 3, 3), (990, 4, 4)])];
        let set = TimeSet::of(&[(200, 500), (950, 1000)]);
        let scooped = scoop(&records, &set);
        assert_eq!(scooped, vec![rec(0, &[(250, 2, 2), (400, 3, 3), (990, 4, 4)])]);
        // An empty scoop keeps nothing.
        assert!(scoop(&records, &TimeSet::empty()).is_empty());
        // Interval bounds are honoured exactly — TimeSet's convention is
        // half-open [start, end), so a single-tick scoop is (t, t+1) and a
        // zero-length interval scoops nothing.
        let edge = TimeSet::from_interval(Interval::new(100, 101));
        assert_eq!(scoop(&records, &edge), vec![rec(0, &[(100, 1, 1)])]);
        assert!(scoop(&records, &TimeSet::from_interval(Interval::new(100, 100))).is_empty());
    }

    #[test]
    fn sync_plan_batches_stay_under_the_datagram_budget() {
        // A heavily built chunk: 4000 edits (44 KB encoded) must split.
        let big: Vec<(u64, u16, u8)> = (0..4000u16).map(|i| (0u64, i, 7u8)).collect();
        let records = vec![rec(0, &big), rec(1, &[(0, 1, 1)])];
        let mut plan = SyncPlan::new(&records, 3);
        assert!(plan.total_batches() > 1, "must have split");
        let mut decoded_edits = 0usize;
        while !plan.is_done() {
            let frame: Vec<Vec<u8>> = plan.drain_frame().to_vec();
            assert!(frame.len() <= 3, "pacing: at most batches_per_frame");
            for b in &frame {
                assert!(b.len() <= BATCH_BUDGET_BYTES, "batch {} bytes over budget", b.len());
                let recs = decode_records(b).expect("every batch decodes");
                decoded_edits += recs.iter().map(|r| r.edits.len()).sum::<usize>();
            }
        }
        assert_eq!(decoded_edits, 4001, "every edit arrives exactly once");
    }

    #[test]
    fn an_empty_world_is_one_empty_plan() {
        let mut plan = SyncPlan::new(&[], 4);
        assert!(plan.is_done());
        assert!(plan.drain_frame().is_empty());
    }
}
