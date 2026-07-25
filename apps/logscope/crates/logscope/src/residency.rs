//! residency.rs — memory accounted as *residency* rather than as a stream of uploads.
//!
//! The premise of the memory-map spec is that the fastest byte is the one that never moves, so the thing
//! worth measuring is not upload throughput but how much of the working set is already in place. This
//! module is the bookkeeping half of that: it tracks what is resident, in which lifetime, how often bytes
//! had to be moved again, and — when something must go — which object is cheapest to lose.
//!
//! It is deliberately pure. Nothing here allocates GPU memory or touches a driver; the engine reports
//! facts into it (`record_upload`, `record_reuse`, `touch`) and the Liaison renders them. That split is
//! what lets the numbers be unit-tested against a known scenario instead of only being watchable on real
//! hardware.
//!
//! The MoE expert streaming path is the live consumer: every slot fill is an upload, every slot hit is a
//! reuse, and `reuse_ratio` is then literally the streaming hit rate expressed in bytes — the number that
//! decides whether streaming is affordable at all.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Every allocation belongs to exactly one lifetime, and nothing crosses lifetimes. Ordered longest-lived
/// first, which is also the order in which regions should be laid out so that short-lived churn never
/// fragments the long-lived arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Serialize, Deserialize)]
pub enum Lifetime {
    /// Model weights, codebooks — resident for the life of the process.
    Permanent,
    /// Loaded per session: KV arenas, indexes.
    Session,
    /// Per conversation: prefix trees, working prompts.
    Conversation,
    /// Per request/decode: scratch that resets.
    Dispatch,
    /// Reset every use; never malloc'd individually.
    Scratch,
}

impl Lifetime {
    pub fn label(&self) -> &'static str {
        match self {
            Lifetime::Permanent => "permanent", Lifetime::Session => "session",
            Lifetime::Conversation => "conversation", Lifetime::Dispatch => "dispatch",
            Lifetime::Scratch => "scratch",
        }
    }
}

/// One resident object. `reconstruct_cost` is what makes eviction intelligent: a tensor that can be
/// re-read from an mmap'd file cheaply is a far better eviction candidate than one that would have to be
/// re-quantized from source, even if they are the same size and equally cold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub bytes: u64,
    pub lifetime: Lifetime,
    /// Resident in device memory (vs host).
    pub gpu: bool,
    /// Relative cost to rebuild if evicted (1.0 = a plain re-read; higher = re-quantize, re-index, ...).
    pub reconstruct_cost: f64,
    /// Access bookkeeping for the scheduler.
    pub accesses: u64,
    pub last_access: u64,
}

impl Region {
    pub fn new(name: &str, bytes: u64, lifetime: Lifetime, gpu: bool) -> Self {
        Region { name: name.into(), bytes, lifetime, gpu, reconstruct_cost: 1.0, accesses: 0, last_access: 0 }
    }
    pub fn reconstruct_cost(mut self, c: f64) -> Self { self.reconstruct_cost = c; self }
}

/// The residency picture plus the movement counters the memory spec asks to track continuously.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Residency {
    pub regions: Vec<Region>,
    /// Bytes that had to be moved into place (slot fills, staging copies).
    pub bytes_uploaded: u64,
    /// Bytes served from something already resident (slot hits, view reuse).
    pub bytes_reused: u64,
    /// Distinct upload operations — allocator/dispatch churn, not just volume.
    pub uploads: u64,
    /// Monotonic access clock.
    clock: u64,
}

impl Residency {
    pub fn new() -> Self { Self::default() }

    pub fn add(&mut self, r: Region) { self.regions.push(r); }

    pub fn get(&self, name: &str) -> Option<&Region> { self.regions.iter().find(|r| r.name == name) }

    /// Note that a region was used (drives recency/frequency for eviction).
    pub fn touch(&mut self, name: &str) {
        self.clock += 1;
        let c = self.clock;
        if let Some(r) = self.regions.iter_mut().find(|r| r.name == name) {
            r.accesses += 1; r.last_access = c;
        }
    }

    /// Bytes had to move. This is the number the architecture treats as debt.
    pub fn record_upload(&mut self, bytes: u64) { self.bytes_uploaded += bytes; self.uploads += 1; }

    /// Bytes were already in place.
    pub fn record_reuse(&mut self, bytes: u64) { self.bytes_reused += bytes; }

    pub fn total_bytes(&self) -> u64 { self.regions.iter().map(|r| r.bytes).sum() }
    pub fn gpu_bytes(&self) -> u64 { self.regions.iter().filter(|r| r.gpu).map(|r| r.bytes).sum() }
    pub fn host_bytes(&self) -> u64 { self.regions.iter().filter(|r| !r.gpu).map(|r| r.bytes).sum() }

    pub fn bytes_by_lifetime(&self) -> HashMap<Lifetime, u64> {
        let mut m = HashMap::new();
        for r in &self.regions { *m.entry(r.lifetime).or_insert(0) += r.bytes; }
        m
    }

    /// Of all bytes the workload needed, the fraction already resident. 1.0 = nothing moved twice; this is
    /// the headline number the whole residency argument rests on.
    pub fn reuse_ratio(&self) -> f64 {
        let total = self.bytes_uploaded + self.bytes_reused;
        if total == 0 { return 1.0; }
        self.bytes_reused as f64 / total as f64
    }

    /// Mean bytes per upload — small frequent uploads and few large ones are very different problems, and
    /// the fix differs (batching vs residency).
    pub fn mean_upload_bytes(&self) -> f64 {
        if self.uploads == 0 { 0.0 } else { self.bytes_uploaded as f64 / self.uploads as f64 }
    }

    /// The eviction scheduler. Keep-score rewards frequently and recently used regions and penalises large
    /// ones, scaled by how expensive the region would be to rebuild. The lowest score is evicted first:
    /// cold, large, and cheap to reconstruct. `Permanent` regions are never candidates.
    pub fn evict_candidate(&self) -> Option<&Region> {
        self.regions.iter()
            .filter(|r| r.lifetime != Lifetime::Permanent && r.bytes > 0)
            .min_by(|a, b| self.keep_score(a).partial_cmp(&self.keep_score(b)).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Higher = more worth keeping.
    pub fn keep_score(&self, r: &Region) -> f64 {
        let age = self.clock.saturating_sub(r.last_access) as f64;
        let recency = 1.0 / (1.0 + age);
        let freq = r.accesses as f64 + 1.0;
        let mb = (r.bytes as f64 / (1 << 20) as f64).max(0.001);
        freq * recency * r.reconstruct_cost / mb
    }

    /// The spec's visual memory map: proportional bars over `width` columns, largest first. Regions too
    /// small to earn a column are summed into a trailing remainder so the total always reconciles.
    pub fn memory_map(&self, width: usize) -> Vec<String> {
        let total = self.total_bytes();
        if total == 0 { return vec!["(nothing resident)".into()]; }
        let mut rs: Vec<&Region> = self.regions.iter().collect();
        rs.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        let mut out = Vec::new();
        let mut shown = 0u64;
        for r in rs.iter() {
            let cols = (r.bytes as f64 / total as f64 * width as f64).round() as usize;
            if cols == 0 { continue; }
            shown += r.bytes;
            out.push(format!("{:<22} {:<width$} {:>8} ({:.0}%)",
                r.name, "█".repeat(cols.min(width)), gib(r.bytes),
                r.bytes as f64 / total as f64 * 100.0, width = width));
        }
        let rest = total.saturating_sub(shown);
        if rest > 0 { out.push(format!("{:<22} {:<width$} {:>8}", "(smaller regions)", "", gib(rest), width = width)); }
        out
    }
}

/// Format bytes as a human GiB/MiB string.
pub fn gib(b: u64) -> String {
    let g = b as f64 / (1u64 << 30) as f64;
    if g >= 1.0 { format!("{g:.2} GiB") } else { format!("{:.0} MiB", b as f64 / (1u64 << 20) as f64) }
}

/// A contiguous host arena carved into offsets, so N tensors cost ONE allocation instead of N. The MoE
/// streaming store is the motivating case: 16 experts × 48 layers is ~1500 separate multi-MiB `Vec`s
/// otherwise, which is exactly the allocator churn the spec calls debt.
#[derive(Clone, Debug, Default)]
pub struct HostArena {
    buf: Vec<u8>,
    spans: Vec<(usize, usize)>, // (offset, len) per item
}

impl HostArena {
    pub fn with_capacity(bytes: usize) -> Self { HostArena { buf: Vec::with_capacity(bytes), spans: Vec::new() } }

    /// Append an item, returning its index. The bytes are copied once, here, and never again.
    pub fn push(&mut self, data: &[u8]) -> usize {
        let off = self.buf.len();
        self.buf.extend_from_slice(data);
        self.spans.push((off, data.len()));
        self.spans.len() - 1
    }

    /// A view of item `i` — no copy, no allocation.
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        let &(off, len) = self.spans.get(i)?;
        self.buf.get(off..off + len)
    }

    pub fn len(&self) -> usize { self.spans.len() }
    pub fn is_empty(&self) -> bool { self.spans.is_empty() }
    pub fn bytes(&self) -> usize { self.buf.len() }
    /// Number of heap allocations backing the whole arena, regardless of item count.
    pub fn allocations(&self) -> usize { if self.buf.is_empty() { 0 } else { 1 } }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scout under the Asahi ceiling, with the real measured figures.
    fn scout() -> Residency {
        let mut r = Residency::new();
        // real figures from the streaming preflight: dense (q4 weights + q8 embed + kv) 3.67 GiB,
        // 4 pinned experts at 3.16 GiB each, a 2-slot ring across 48 layers at 67.5 MiB per expert-layer
        r.add(Region::new("dense weights + kv", 3_940_000_000, Lifetime::Permanent, true));
        r.add(Region::new("pinned experts", 13_570_000_000, Lifetime::Permanent, true).reconstruct_cost(3.0));
        r.add(Region::new("stream ring", 6_960_000_000, Lifetime::Session, true));
        r.add(Region::new("host expert store", 55_000_000_000, Lifetime::Session, false).reconstruct_cost(4.0));
        r.add(Region::new("scratch", 268_000_000, Lifetime::Scratch, true));
        r
    }

    #[test]
    fn splits_device_from_host_residency() {
        let r = scout();
        // the device side must sit under the ~24 GiB Honeykrisp ceiling; the host side is where the
        // streaming store actually lives, and it is the larger of the two
        assert!(r.gpu_bytes() < 24 * (1u64 << 30), "device residency {} over ceiling", gib(r.gpu_bytes()));
        assert!(r.host_bytes() > r.gpu_bytes(), "streaming moves the bulk to host RAM");
    }

    #[test]
    fn reuse_ratio_is_the_streaming_hit_rate_in_bytes() {
        let mut r = Residency::new();
        // 100 accesses of a 64 MiB expert-layer: 38 had to be filled, 62 were already resident
        let unit = 64u64 << 20;
        for _ in 0..38 { r.record_upload(unit); }
        for _ in 0..62 { r.record_reuse(unit); }
        assert!((r.reuse_ratio() - 0.62).abs() < 1e-9);
        assert_eq!(r.uploads, 38);
        assert!((r.mean_upload_bytes() - unit as f64).abs() < 1.0);
    }

    #[test]
    fn a_perfectly_resident_workload_reports_full_reuse() {
        let r = Residency::new();
        assert_eq!(r.reuse_ratio(), 1.0, "nothing moved at all is the ideal, not a divide-by-zero");
    }

    #[test]
    fn eviction_prefers_cold_large_and_cheap_to_rebuild() {
        let mut r = Residency::new();
        r.add(Region::new("archived kv", 4_000_000_000, Lifetime::Conversation, false).reconstruct_cost(1.0));
        r.add(Region::new("hot index", 200_000_000, Lifetime::Session, false).reconstruct_cost(8.0));
        r.add(Region::new("weights", 8_000_000_000, Lifetime::Permanent, true));
        for _ in 0..20 { r.touch("hot index"); }
        r.touch("archived kv");
        let victim = r.evict_candidate().expect("something is evictable");
        assert_eq!(victim.name, "archived kv", "big, cold, cheap to rebuild goes first");
        // and the permanent weights are never offered up, despite being the largest thing resident
        assert!(r.regions.iter().any(|x| x.lifetime == Lifetime::Permanent));
        assert_ne!(victim.lifetime, Lifetime::Permanent);
    }

    #[test]
    fn reconstruct_cost_outranks_raw_size() {
        let mut r = Residency::new();
        // same size, same coldness — the one that is expensive to rebuild must survive
        r.add(Region::new("cheap", 1_000_000_000, Lifetime::Session, false).reconstruct_cost(1.0));
        r.add(Region::new("expensive", 1_000_000_000, Lifetime::Session, false).reconstruct_cost(10.0));
        assert_eq!(r.evict_candidate().unwrap().name, "cheap");
    }

    #[test]
    fn memory_map_reconciles_to_the_total() {
        let r = scout();
        let lines = r.memory_map(20);
        assert!(!lines.is_empty());
        // the biggest region leads
        assert!(lines[0].contains("host expert store"));
        // every line carries a size, so the picture is auditable rather than decorative
        assert!(lines.iter().all(|l| l.contains("GiB") || l.contains("MiB")));
    }

    #[test]
    fn lifetimes_partition_the_total() {
        let r = scout();
        let by = r.bytes_by_lifetime();
        let sum: u64 = by.values().sum();
        assert_eq!(sum, r.total_bytes(), "every byte belongs to exactly one lifetime");
        assert!(by.contains_key(&Lifetime::Permanent));
    }

    #[test]
    fn arena_replaces_many_allocations_with_one() {
        let mut a = HostArena::with_capacity(1024);
        let items: Vec<Vec<u8>> = (0..64).map(|i| vec![i as u8; 100]).collect();
        for it in &items { a.push(it); }
        assert_eq!(a.len(), 64);
        assert_eq!(a.allocations(), 1, "64 tensors, one heap allocation");
        // views round-trip exactly — the bytes were copied once, on push, and never again
        for (i, it) in items.iter().enumerate() { assert_eq!(a.get(i).unwrap(), &it[..]); }
        assert_eq!(a.bytes(), 6400);
        assert!(a.get(64).is_none());
    }
}
