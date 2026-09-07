//! xref — cross-referencing between segment workers.
//!
//! ## What already worked, and the gap
//!
//! A `-j N` run splits the video at keyframes and gives each segment its own worker. Two
//! kinds of reuse exist today:
//!
//! * the **reference ring**, per worker: a block that reappears near its old position in a
//!   later frame, accepted by the quality gate on a bounded SSE. Near-matches allowed;
//!   confined to one worker's own frames.
//! * the **[`BlockCache`](crate::blockcache)**, shared across workers: a cell whose EXACT
//!   bytes were upscaled before is served from memory, wherever and whenever it appeared.
//!
//! So cross-worker reuse exists and is exact-only. That is the gap. Video repeats
//! approximately far more often than it repeats exactly: a cut back to the same shot, a
//! character against the same wall two minutes later, a slow pan revisiting scenery. Every
//! one of those is a near-match the ring would happily take — and cannot, because it is in
//! another segment, in another worker, on another thread. On a `-j 8` run of a dialogue
//! scene the same two shoulders are upscaled eight times over.
//!
//! ## What this adds
//!
//! A shared store keyed by a **coarse fingerprint** of the low-res cell rather than by its
//! exact bytes. A fingerprint collects candidates; the existing SSE budget decides. Nothing
//! is served on the strength of the hash — the fingerprint's only job is to make the
//! candidate list short, and a collision costs one bounded comparison and is rejected.
//!
//! That ordering is the whole safety argument. The gate is unchanged and still the sole
//! authority on whether a substitution is allowed, so a cross-worker hit is exactly as good
//! as an in-worker one by the same measure the user set with `--quality`. This cannot make
//! output worse than the gate permits; it can only find more places the gate says yes.
//!
//! ## Why a mean-and-slope fingerprint
//!
//! The key is 4×4 block means quantised to 5 bits, plus the cell's overall mean. Coarse
//! enough that a small brightness or compression difference lands in the same bucket, sharp
//! enough that unrelated content usually does not. It is deliberately NOT a perceptual hash
//! with a similarity metric of its own: a second similarity measure that disagrees with the
//! gate would be a second policy, and the failure mode would be a block the gate would have
//! rejected being offered as "close enough" by something else.
//!
//! ## Memory, and why entries carry the low-res cell too
//!
//! An entry is `(low-res cell, upscaled block)`. The low-res copy is what the gate compares
//! against, so it has to be stored — an upscaled-only store could not verify anything and
//! would be back to trusting the hash. At `block=32`, `bleed=4` and `scale=2` that is 4.8 kB
//! of low-res and 12 kB of output per entry, so a 256 MB budget holds roughly fifteen
//! thousand distinct cells. FIFO, like `BlockCache`, for the same reason: repeats are bursty
//! and cheap to re-learn.

use std::collections::HashMap;
use std::sync::Arc;

/// A coarse content key. Not a hash of the bytes — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(u64);

/// How many buckets across and down the fingerprint divides a cell into.
const FP_GRID: u32 = 4;
/// Bits kept per bucket mean. 5 bits = 32 levels ≈ 8 grey levels of tolerance.
const FP_BITS: u32 = 5;

impl Fingerprint {
    /// From a `cell`×`cell` RGB cell laid out row-major with `stride` bytes per row.
    ///
    /// Luma-only on purpose: a colour shift big enough to matter will move the luma too,
    /// and keying on three channels triples the chance that an irrelevant difference splits
    /// two cells that the gate would have accepted for each other.
    pub fn of_cell(cell_rgb: &[u8], cell: u32, stride: usize) -> Fingerprint {
        let mut key: u64 = 0;
        let step = (cell as f32 / FP_GRID as f32).max(1.0);
        for by in 0..FP_GRID {
            for bx in 0..FP_GRID {
                let (x0, y0) = ((bx as f32 * step) as u32, (by as f32 * step) as u32);
                let (x1, y1) = (((bx + 1) as f32 * step) as u32).min(cell).max(x0 + 1).into_pair(
                    (((by + 1) as f32 * step) as u32).min(cell).max(y0 + 1),
                );
                let mut sum: u64 = 0;
                let mut n: u64 = 0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y as usize * stride + x as usize * 3;
                        if i + 2 < cell_rgb.len() {
                            // luma, integer: (2R + 5G + B) / 8 — close enough to Rec.601 for
                            // a bucket key and exact across platforms.
                            let p = &cell_rgb[i..i + 3];
                            sum += (2 * p[0] as u64 + 5 * p[1] as u64 + p[2] as u64) / 8;
                            n += 1;
                        }
                    }
                }
                let mean = if n > 0 { (sum / n) as u32 } else { 0 };
                let q = (mean >> (8 - FP_BITS)) as u64;
                key = (key << FP_BITS) | q;
            }
        }
        Fingerprint(key)
    }

    pub fn bits(self) -> u64 {
        self.0
    }
}

/// Tiny helper so the tuple above reads in one line without a temporary.
trait IntoPair {
    fn into_pair(self, other: u32) -> (u32, u32);
}
impl IntoPair for u32 {
    fn into_pair(self, other: u32) -> (u32, u32) {
        (self, other)
    }
}

/// One remembered cell: what it looked like, and what came out.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The low-res cell's bytes, for the gate to compare against.
    pub lores: Arc<Vec<u8>>,
    /// The upscaled block that was produced from it.
    pub up: Arc<Vec<u8>>,
}

/// Why a lookup did not return anything. Kept as an enum because the three cases mean
/// different things when you are trying to work out why a run got less reuse than expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// Nothing has that fingerprint.
    NoCandidate,
    /// Candidates existed and every one failed the gate. The fingerprint is coarser than
    /// the gate, which is the intended direction — this is a collision, not a fault.
    GateRejected,
    /// The store is switched off.
    Disabled,
}

/// What a lookup found, and what it cost.
#[derive(Debug, Clone)]
pub enum Hit {
    Found(Arc<Vec<u8>>),
    Missed(Miss),
}

/// Counters, so a run can report whether cross-referencing is paying for itself. Without
/// these the feature is unfalsifiable: more memory, and no way to say what it bought.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub gate_rejected: u64,
    pub no_candidate: u64,
    pub inserted: u64,
    pub evicted: u64,
    /// Candidate cells compared against — the cost side. A high ratio to `hits` means the
    /// fingerprint is too coarse for this content.
    pub compared: u64,
}

impl Stats {
    /// Fraction of lookups that produced a block, 0.0 when nothing was asked.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.gate_rejected + self.no_candidate;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// The shared store. One per job, behind a mutex, handed to every worker.
#[derive(Debug)]
pub struct Xref {
    buckets: HashMap<Fingerprint, Vec<Entry>>,
    /// Insertion order, for FIFO eviction.
    order: std::collections::VecDeque<(Fingerprint, u64)>,
    bytes: u64,
    budget: u64,
    /// A cap on how many entries one fingerprint may hold. Without it a flat-grey
    /// fingerprint — sky, letterbox, a fade — collects thousands of near-identical cells and
    /// every lookup against it walks the lot. The gate would still be correct; the run
    /// would just be slower than not caching at all.
    per_bucket: usize,
    stats: Stats,
}

/// Beyond this many candidates in one bucket, the oldest are dropped.
pub const DEFAULT_PER_BUCKET: usize = 8;

impl Xref {
    pub fn new(budget: u64) -> Xref {
        Xref {
            buckets: HashMap::new(),
            order: std::collections::VecDeque::new(),
            bytes: 0,
            budget,
            per_bucket: DEFAULT_PER_BUCKET,
            stats: Stats::default(),
        }
    }

    pub fn with_per_bucket(mut self, n: usize) -> Xref {
        self.per_bucket = n.max(1);
        self
    }

    pub fn enabled(&self) -> bool {
        self.budget > 0
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.buckets.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Look for a block that can stand in for `lores`.
    ///
    /// `budget_sse` is the gate's threshold for THIS cell — the same number the reference
    /// ring is tested against. `sse` computes the error between two low-res cells; it is
    /// passed in rather than imported so the caller's bounded/early-exit implementation is
    /// the one that runs.
    pub fn lookup(
        &mut self,
        fp: Fingerprint,
        lores: &[u8],
        budget_sse: u64,
        sse: impl Fn(&[u8], &[u8]) -> u64,
    ) -> Hit {
        if !self.enabled() {
            return Hit::Missed(Miss::Disabled);
        }
        let Some(cands) = self.buckets.get(&fp) else {
            self.stats.no_candidate += 1;
            return Hit::Missed(Miss::NoCandidate);
        };
        // Newest first: the most recently seen cell is the likeliest match, and finding it
        // early is what keeps the comparison count near one per hit.
        let mut best: Option<(u64, Arc<Vec<u8>>)> = None;
        for e in cands.iter().rev() {
            if e.lores.len() != lores.len() {
                continue;
            }
            self.stats.compared += 1;
            let err = sse(&e.lores, lores);
            if err <= budget_sse && best.as_ref().is_none_or(|(b, _)| err < *b) {
                best = Some((err, e.up.clone()));
                if err == 0 {
                    break; // an exact match cannot be beaten
                }
            }
        }
        match best {
            Some((_, up)) => {
                self.stats.hits += 1;
                Hit::Found(up)
            }
            None => {
                self.stats.gate_rejected += 1;
                Hit::Missed(Miss::GateRejected)
            }
        }
    }

    /// Remember a cell and the block it produced.
    pub fn insert(&mut self, fp: Fingerprint, lores: Arc<Vec<u8>>, up: Arc<Vec<u8>>) {
        if !self.enabled() {
            return;
        }
        let cost = (lores.len() + up.len()) as u64;
        if cost > self.budget {
            return; // one entry larger than the whole budget: refuse rather than thrash
        }
        let bucket = self.buckets.entry(fp).or_default();
        bucket.push(Entry { lores, up });
        self.bytes += cost;
        self.stats.inserted += 1;
        self.order.push_back((fp, cost));
        // Cap the bucket first — a flat fingerprint would otherwise evict useful entries
        // from elsewhere to keep collecting copies of the sky.
        while bucket.len() > self.per_bucket {
            let e = bucket.remove(0);
            let freed = (e.lores.len() + e.up.len()) as u64;
            self.bytes = self.bytes.saturating_sub(freed);
            self.stats.evicted += 1;
            // The order queue keeps a stale entry for this; `evict_one` tolerates it.
        }
        while self.bytes > self.budget {
            if !self.evict_one() {
                break;
            }
        }
    }

    /// Drop the oldest surviving entry. `false` when there is nothing left to drop.
    fn evict_one(&mut self) -> bool {
        while let Some((fp, cost)) = self.order.pop_front() {
            if let Some(b) = self.buckets.get_mut(&fp) {
                if !b.is_empty() {
                    let e = b.remove(0);
                    let freed = (e.lores.len() + e.up.len()) as u64;
                    self.bytes = self.bytes.saturating_sub(freed);
                    self.stats.evicted += 1;
                    if b.is_empty() {
                        self.buckets.remove(&fp);
                    }
                    return true;
                }
                self.buckets.remove(&fp);
            }
            let _ = cost; // the queue entry was already dropped by the per-bucket cap
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain SSE over two equal-length byte slices — the shape the caller supplies.
    fn sse(a: &[u8], b: &[u8]) -> u64 {
        a.iter().zip(b).map(|(x, y)| {
            let d = *x as i64 - *y as i64;
            (d * d) as u64
        }).sum()
    }

    /// A cell of `cell`×`cell` RGB at a given grey, with an optional gradient so it is not
    /// degenerate.
    fn cell_bytes(cell: u32, base: u8, slope: i32) -> Vec<u8> {
        let mut v = Vec::with_capacity((cell * cell * 3) as usize);
        for y in 0..cell {
            for x in 0..cell {
                let g = (base as i32 + slope * (x as i32 + y as i32) / cell.max(1) as i32).clamp(0, 255) as u8;
                v.extend_from_slice(&[g, g, g]);
            }
        }
        v
    }

    fn fp(bytes: &[u8], cell: u32) -> Fingerprint {
        Fingerprint::of_cell(bytes, cell, cell as usize * 3)
    }

    /// The point of the whole module: a worker finds a block ANOTHER worker upscaled, for a
    /// cell that is close but not byte-identical — which the exact-match BlockCache cannot
    /// do and which is how video actually repeats.
    #[test]
    fn a_near_match_from_another_worker_is_found() {
        let cell = 16;
        let mine = cell_bytes(cell, 100, 40);
        // "another worker's" cell: the same content one grey level darker, as a second
        // encode of the same shot would be.
        let theirs: Vec<u8> = mine.iter().map(|b| b.saturating_sub(1)).collect();
        let up = Arc::new(vec![7u8; 64 * 3]);

        let mut x = Xref::new(1 << 20);
        x.insert(fp(&theirs, cell), Arc::new(theirs.clone()), up.clone());

        // a generous budget: one level over 768 subpixels is 768 units of SSE
        let budget = 2_000;
        match x.lookup(fp(&mine, cell), &mine, budget, sse) {
            Hit::Found(got) => assert_eq!(got, up, "the wrong block came back"),
            other => panic!("no cross-worker hit: {other:?}"),
        }
        assert_eq!(x.stats().hits, 1);
        // and the exact-match cache could NOT have served this — the bytes differ
        assert_ne!(mine, theirs);
    }

    /// The gate is the only authority. A candidate the fingerprint groups together but the
    /// SSE budget rejects must not be served — that is what keeps this incapable of making
    /// output worse than `--quality` allows.
    #[test]
    fn the_gate_and_not_the_fingerprint_decides() {
        let cell = 16;
        let a = cell_bytes(cell, 100, 40);
        let b: Vec<u8> = a.iter().map(|v| v.saturating_add(20)).collect();
        let up = Arc::new(vec![1u8; 32]);
        let mut x = Xref::new(1 << 20);
        x.insert(fp(&b, cell), Arc::new(b.clone()), up.clone());

        // A tight budget rejects it even if the fingerprints happen to agree.
        let tight = 10;
        let got = x.lookup(fp(&a, cell), &a, tight, sse);
        assert!(matches!(got, Hit::Missed(Miss::GateRejected) | Hit::Missed(Miss::NoCandidate)), "{got:?}");
        // A loose budget accepts it — same store, same fingerprint, different policy.
        let loose = u64::MAX / 4;
        if fp(&a, cell) == fp(&b, cell) {
            assert!(matches!(x.lookup(fp(&a, cell), &a, loose, sse), Hit::Found(_)));
        }
    }

    /// A fingerprint is coarser than the bytes, on purpose: a cell one grey level different
    /// must land in the same bucket, or nothing would ever be a candidate.
    #[test]
    fn small_differences_share_a_fingerprint_and_unrelated_content_usually_does_not() {
        let cell = 16;
        let a = cell_bytes(cell, 100, 40);
        let a1: Vec<u8> = a.iter().map(|b| b.saturating_sub(1)).collect();
        assert_eq!(fp(&a, cell), fp(&a1, cell), "a one-level difference split the bucket");

        let dark = cell_bytes(cell, 20, 40);
        let bright = cell_bytes(cell, 220, 10);
        assert_ne!(fp(&dark, cell), fp(&bright, cell), "unrelated cells collided");
        // a flat cell and a strong gradient of the same MEAN are also distinguishable,
        // which a single average would not manage
        let flat = cell_bytes(cell, 128, 0);
        let ramp = cell_bytes(cell, 60, 130);
        assert_ne!(fp(&flat, cell), fp(&ramp, cell), "the fingerprint is only a mean");
    }

    #[test]
    fn a_miss_says_which_kind_it_was() {
        let cell = 16;
        let a = cell_bytes(cell, 100, 40);
        let mut off = Xref::new(0);
        assert!(matches!(off.lookup(fp(&a, cell), &a, 100, sse), Hit::Missed(Miss::Disabled)));
        off.insert(fp(&a, cell), Arc::new(a.clone()), Arc::new(vec![0u8; 8]));
        assert!(off.is_empty(), "a disabled store must not grow");

        let mut x = Xref::new(1 << 20);
        assert!(matches!(x.lookup(fp(&a, cell), &a, 100, sse), Hit::Missed(Miss::NoCandidate)));
        assert_eq!(x.stats().no_candidate, 1);
    }

    /// A flat fingerprint — sky, letterbox, a fade to black — would otherwise collect
    /// thousands of near-identical cells and make every lookup against it walk the lot.
    #[test]
    fn one_fingerprint_cannot_grow_without_bound() {
        let cell = 8;
        let flat = cell_bytes(cell, 128, 0);
        let mut x = Xref::new(1 << 20).with_per_bucket(4);
        for i in 0..50u8 {
            x.insert(fp(&flat, cell), Arc::new(flat.clone()), Arc::new(vec![i; 16]));
        }
        assert_eq!(x.len(), 4, "the bucket grew past its cap");
        // the survivors are the NEWEST, and a lookup still works
        assert!(matches!(x.lookup(fp(&flat, cell), &flat, 1_000, sse), Hit::Found(_)));
        assert!(x.stats().evicted >= 46);
    }

    #[test]
    fn the_budget_is_respected_and_an_oversized_entry_is_refused() {
        let cell = 8;
        let budget = 20_000u64;
        let mut x = Xref::new(budget);
        for i in 0..200u8 {
            let c = cell_bytes(cell, i.wrapping_mul(7), 30);
            x.insert(fp(&c, cell), Arc::new(c.clone()), Arc::new(vec![i; 256]));
            assert!(x.bytes() <= budget, "over budget at {i}: {} > {budget}", x.bytes());
        }
        assert!(x.stats().evicted > 0, "nothing was ever evicted — the test never filled it");
        // an entry bigger than the whole budget is refused rather than thrashing
        let before = x.len();
        let huge = vec![0u8; budget as usize * 2];
        x.insert(fp(&huge[..192], cell), Arc::new(huge.clone()), Arc::new(huge));
        assert_eq!(x.len(), before);
    }

    /// The counters are what make this falsifiable: more memory, and a number saying what it
    /// bought.
    #[test]
    fn the_stats_report_what_cross_referencing_bought() {
        let cell = 16;
        let a = cell_bytes(cell, 100, 40);
        let mut x = Xref::new(1 << 20);
        assert_eq!(x.stats().hit_rate(), 0.0, "no lookups is not a hit rate of anything");
        x.insert(fp(&a, cell), Arc::new(a.clone()), Arc::new(vec![3u8; 8]));
        let _ = x.lookup(fp(&a, cell), &a, 10, sse); // hit, exact
        let b = cell_bytes(cell, 20, 5);
        let _ = x.lookup(fp(&b, cell), &b, 10, sse); // no candidate
        let s = x.stats();
        assert_eq!((s.hits, s.no_candidate), (1, 1));
        assert!((s.hit_rate() - 0.5).abs() < 1e-9);
        assert!(s.compared >= 1, "a hit that compared nothing means the gate was skipped");
    }
}
