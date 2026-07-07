//! `sim::record_zones` — pure per-frame split of the command-`record` pass
//! into sub-zones (WID `hh-r332-t2`).
//!
//! The r331 M2 profile shows the `record` pass costing p50 4.0 / p90 8.3 ms with
//! **100 % of frames over 1 ms** — a steady per-instance CPU tax, but the flat
//! `record` zone can't say *which* slice owns it. This core is the typed
//! accumulator the bin feeds so the next profile attributes the 5 ms across
//! `record_chunks / record_entities / record_overlay / record_ui`.
//!
//! It complements `crate::profiler::Profiler` (string-keyed histograms) without
//! depending on it: the bin reads [`RecordZoneFrame::zone_samples`] and forwards
//! each `(name, ms)` into its profiler, so the existing percentile/histogram
//! machinery works per sub-zone. Kept dependency-free so it gates standalone.
//!
//! Pure & deterministic: times are **passed in** (no clock/IO/RNG); negative or
//! NaN samples are sanitized to `0.0` so totals stay finite; no
//! `.unwrap()`/`.expect()`/`panic!` in non-test code.

/// The sub-zones the per-frame `record` pass splits into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordZone {
    /// Recording the chunk draws (indirect count path + any per-chunk fallback).
    Chunks,
    /// Per-instance entity draws (mobs / dragons / dropped items).
    Entities,
    /// Foliage / decoration / particle overlay instances.
    Overlay,
    /// egui / HUD / debug-overlay record cost.
    Ui,
}

impl RecordZone {
    /// All sub-zones, in canonical (tie-break) order.
    pub const ALL: [RecordZone; 4] =
        [RecordZone::Chunks, RecordZone::Entities, RecordZone::Overlay, RecordZone::Ui];

    /// The stable profiler zone label the bin should emit for this sub-zone
    /// (matches the `profiler.record(...)` names proposed in the handoff).
    pub fn name(self) -> &'static str {
        match self {
            RecordZone::Chunks => "record_chunks",
            RecordZone::Entities => "record_entities",
            RecordZone::Overlay => "record_overlay",
            RecordZone::Ui => "record_ui",
        }
    }
}

/// Sanitize a measured millisecond sample: NaN / negative ⇒ `0.0`. Keeps every
/// accumulated total finite and non-negative regardless of caller noise.
fn clean_ms(ms: f64) -> f64 {
    if ms.is_nan() || ms < 0.0 {
        0.0
    } else {
        ms
    }
}

/// One frame's `record`-pass split. Add a measured time per sub-zone, then read
/// the breakdown. `Copy` and allocation-free — cheap to keep per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RecordZoneFrame {
    chunks_ms: f64,
    entities_ms: f64,
    overlay_ms: f64,
    ui_ms: f64,
}

impl RecordZoneFrame {
    /// A zeroed frame.
    pub fn new() -> Self {
        RecordZoneFrame::default()
    }

    /// Add a measured (sanitized) sample to a sub-zone. Accumulates, so calling
    /// it twice for the same zone in a frame sums (e.g. several entity batches).
    pub fn add(&mut self, zone: RecordZone, ms: f64) {
        let ms = clean_ms(ms);
        match zone {
            RecordZone::Chunks => self.chunks_ms += ms,
            RecordZone::Entities => self.entities_ms += ms,
            RecordZone::Overlay => self.overlay_ms += ms,
            RecordZone::Ui => self.ui_ms += ms,
        }
    }

    /// The accumulated time for one sub-zone.
    pub fn get(&self, zone: RecordZone) -> f64 {
        match zone {
            RecordZone::Chunks => self.chunks_ms,
            RecordZone::Entities => self.entities_ms,
            RecordZone::Overlay => self.overlay_ms,
            RecordZone::Ui => self.ui_ms,
        }
    }

    /// Total record time this frame (sum of the four sub-zones).
    pub fn total_ms(&self) -> f64 {
        self.chunks_ms + self.entities_ms + self.overlay_ms + self.ui_ms
    }

    /// Fraction of the record total owned by `zone`, in `[0.0, 1.0]`. Returns
    /// `0.0` when the frame total is zero (no divide-by-zero).
    pub fn fraction(&self, zone: RecordZone) -> f64 {
        let total = self.total_ms();
        if total <= 0.0 {
            0.0
        } else {
            self.get(zone) / total
        }
    }

    /// The sub-zone owning the largest share this frame. Ties break by
    /// [`RecordZone::ALL`] order. `None` when the frame total is zero.
    pub fn dominant(&self) -> Option<RecordZone> {
        if self.total_ms() <= 0.0 {
            return None;
        }
        let mut best = RecordZone::Chunks;
        let mut best_ms = self.get(best);
        for &z in &RecordZone::ALL {
            // strict `>` keeps the earliest-in-ALL zone on a tie.
            if self.get(z) > best_ms {
                best = z;
                best_ms = self.get(z);
            }
        }
        Some(best)
    }

    /// The four `(profiler_zone_name, ms)` samples, in [`RecordZone::ALL`] order,
    /// for the bin to forward into its `Profiler` (propose-only; see handoff).
    pub fn zone_samples(&self) -> [(&'static str, f64); 4] {
        [
            (RecordZone::Chunks.name(), self.chunks_ms),
            (RecordZone::Entities.name(), self.entities_ms),
            (RecordZone::Overlay.name(), self.overlay_ms),
            (RecordZone::Ui.name(), self.ui_ms),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_accumulates_per_zone() {
        let mut f = RecordZoneFrame::new();
        f.add(RecordZone::Chunks, 1.5);
        f.add(RecordZone::Chunks, 0.5); // accumulates within a frame
        f.add(RecordZone::Entities, 2.0);
        assert_eq!(f.get(RecordZone::Chunks), 2.0);
        assert_eq!(f.get(RecordZone::Entities), 2.0);
        assert_eq!(f.get(RecordZone::Overlay), 0.0);
        assert_eq!(f.get(RecordZone::Ui), 0.0);
    }

    #[test]
    fn total_is_sum_of_subzones() {
        let mut f = RecordZoneFrame::new();
        f.add(RecordZone::Chunks, 1.0);
        f.add(RecordZone::Entities, 2.0);
        f.add(RecordZone::Overlay, 0.5);
        f.add(RecordZone::Ui, 1.5);
        assert!((f.total_ms() - 5.0).abs() < 1e-9, "5 ms split across four zones");
    }

    #[test]
    fn fractions_are_bounded_and_sum_to_one() {
        let mut f = RecordZoneFrame::new();
        f.add(RecordZone::Chunks, 1.0);
        f.add(RecordZone::Entities, 3.0);
        f.add(RecordZone::Overlay, 0.0);
        f.add(RecordZone::Ui, 1.0); // total 5.0
        let mut sum = 0.0;
        for &z in &RecordZone::ALL {
            let fr = f.fraction(z);
            assert!((0.0..=1.0).contains(&fr), "{z:?} fraction out of range: {fr}");
            sum += fr;
        }
        assert!((sum - 1.0).abs() < 1e-9, "non-zero fractions must sum to 1");
        assert!((f.fraction(RecordZone::Entities) - 0.6).abs() < 1e-9);
    }

    #[test]
    fn empty_frame_has_zero_fraction_and_no_dominant() {
        let f = RecordZoneFrame::new();
        assert_eq!(f.total_ms(), 0.0);
        assert_eq!(f.dominant(), None);
        for &z in &RecordZone::ALL {
            assert_eq!(f.fraction(z), 0.0, "no divide-by-zero on an empty frame");
        }
    }

    #[test]
    fn dominant_picks_largest_and_breaks_ties_by_all_order() {
        let mut f = RecordZoneFrame::new();
        f.add(RecordZone::Chunks, 1.0);
        f.add(RecordZone::Entities, 4.0);
        f.add(RecordZone::Ui, 2.0);
        assert_eq!(f.dominant(), Some(RecordZone::Entities), "largest share wins");
        // tie between Chunks and Overlay → Chunks (earlier in ALL).
        let mut t = RecordZoneFrame::new();
        t.add(RecordZone::Chunks, 2.0);
        t.add(RecordZone::Overlay, 2.0);
        assert_eq!(t.dominant(), Some(RecordZone::Chunks), "tie breaks by ALL order");
    }

    #[test]
    fn samples_carry_canonical_profiler_names() {
        let mut f = RecordZoneFrame::new();
        f.add(RecordZone::Chunks, 1.0);
        f.add(RecordZone::Ui, 2.0);
        let s = f.zone_samples();
        assert_eq!(s[0], ("record_chunks", 1.0));
        assert_eq!(s[1], ("record_entities", 0.0));
        assert_eq!(s[2], ("record_overlay", 0.0));
        assert_eq!(s[3], ("record_ui", 2.0));
        // names are stable and unique across the set.
        let names: Vec<&str> = RecordZone::ALL.iter().map(|z| z.name()).collect();
        let uniq: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(uniq.len(), names.len(), "zone names must be unique");
    }

    #[test]
    fn nan_and_negative_samples_are_sanitized() {
        let mut f = RecordZoneFrame::new();
        f.add(RecordZone::Chunks, f64::NAN);
        f.add(RecordZone::Entities, -3.0);
        f.add(RecordZone::Overlay, 2.0);
        assert_eq!(f.get(RecordZone::Chunks), 0.0, "NaN ⇒ 0");
        assert_eq!(f.get(RecordZone::Entities), 0.0, "negative ⇒ 0");
        assert!(f.total_ms().is_finite());
        assert_eq!(f.total_ms(), 2.0);
    }

    #[test]
    fn deterministic() {
        let build = || {
            let mut f = RecordZoneFrame::new();
            f.add(RecordZone::Chunks, 1.25);
            f.add(RecordZone::Entities, 2.5);
            f
        };
        assert_eq!(build(), build());
        assert_eq!(build().zone_samples(), build().zone_samples());
    }
}
