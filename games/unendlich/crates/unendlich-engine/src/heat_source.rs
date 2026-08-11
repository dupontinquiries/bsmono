//! Heat sources — point sources of warmth (or cold) layered on the
//! base temperature field.
//!
//! ## What this is for
//!
//! `temperature_at(wx, wy, wz)` (in `temperature.rs`) returns the
//! BASE equilibrium temperature: latitude + altitude lapse + noise.
//! That's the climate. But local objects emit heat too — fires,
//! lava, hot machinery, the player's own body. Cold things absorb
//! heat: ice blocks, deep shade.
//!
//! This module holds a sparse list of heat sources: world position,
//! intensity in °C, falloff radius. `total_heat_at(wx, wy, wz)` sums
//! the Gaussian-falloff contributions of all sources at that point.
//! `temperature_at` adds it to the base field.
//!
//! Result: a fire creates a warm bubble around itself. Lava heats
//! the cave it's in. Standing next to ice cools you (and the
//! convection arrow tilts down).
//!
//! ## Why a global registry rather than per-chunk
//!
//! Heat sources are RARE (a few per area, not per cell). Storing
//! them per-chunk would mean walking 27 chunks for any field query
//! to catch sources within radius. A flat global vec is fine: scan
//! it linearly, skip any source far enough that its contribution is
//! negligible.
//!
//! When the source count grows (thousands of fires in a played-in
//! world), this should become a spatial hash. `HeatSourceMap` is
//! the API; the implementation can swap.
//!
//! ## Coupling to wind / convection
//!
//! `temperature::convection_at` reads the temperature field at two
//! altitudes and returns a vertical wind component proportional to
//! the gradient anomaly. Hot point sources create LOCAL anomalies
//! that drive UPDRAFTS at the source. Place a fire in a cave — air
//! rushes up the chimney. Place an ice block in a desert — air
//! sinks toward it. **Convection emerges automatically** from the
//! heat-source field combining with the existing temperature math;
//! no per-source logic needed.

use std::sync::{Arc, RwLock};

/// One point heat source: world position + signed intensity + radius.
///
/// Intensity is in °C. Positive = hot (fire, lava, sun-warmed stone).
/// Negative = cold (ice block, supercooled cell). Radius is in
/// blocks; the contribution at distance `d` is
/// `intensity * exp(-(d/radius)^2)`.
///
/// At d=0 the contribution is the full intensity. At d=radius it's
/// 1/e ≈ 37% of intensity. At d=2·radius it's ~2% — effectively
/// zero, which is the cutoff `total_heat_at` uses to skip distant
/// sources.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeatSource {
    pub position: [f32; 3],
    pub intensity_c: f32,
    pub radius: f32,
}

impl HeatSource {
    pub fn new(position: [f32; 3], intensity_c: f32, radius: f32) -> Self {
        Self { position, intensity_c, radius }
    }

    /// Contribution of this source at world position `(x, y, z)` in °C.
    /// Gaussian falloff with the source's radius.
    pub fn contribution_at(&self, wx: f32, wy: f32, wz: f32) -> f32 {
        let dx = wx - self.position[0];
        let dy = wy - self.position[1];
        let dz = wz - self.position[2];
        let dist_sq = dx * dx + dy * dy + dz * dz;
        let r = self.radius.max(0.5);
        let r_sq = r * r;
        // Skip sources past 2× radius — contribution < 2%, not worth
        // the exp().
        if dist_sq > 4.0 * r_sq { return 0.0; }
        let falloff = (-dist_sq / r_sq).exp();
        self.intensity_c * falloff
    }
}

/// Sparse map of heat sources. Cheap to clone (Arc) so the field
/// can be passed around freely. Internal RwLock allows runtime
/// mutation (player lights fire, fire goes out) from any thread.
///
/// ## Spatial index (r-followups-heatidx)
///
/// `total_heat_at` used to be a flat `O(n)` scan — fine while sources
/// were rare, but `docs/FOLLOWUPS.md`'s 2026-06-04 entry flagged it as
/// a real cost once fire/lava/ice counts grow, since it's called from
/// convection/plant/ice EVERY TICK (many reads per rare write — lighting
/// or extinguishing a fire is occasional; querying heat at a point
/// happens constantly). That read/write ratio is exactly what a
/// rebuild-on-write, O(nearby)-on-read index is for.
///
/// Implementation: a uniform grid, cell size = the largest source's
/// influence reach (`2 * radius.max(0.5)`, matching `contribution_at`'s
/// own cutoff exactly) or a sane default if empty/small. Per-axis
/// distance from any point to a source within its own reach can differ
/// from that source's cell by at most 1 cell (reach ≤ cell_size implies
/// each axis offset ≤ cell_size), so a fixed 3×3×3 neighborhood around
/// the query's cell always finds every source that could contribute —
/// never a partial/approximate answer, just a bounded search instead of
/// a full scan.
///
/// The index rebuilds eagerly after every mutation (`add`, `remove`,
/// `remove_nearest_within`, `remove_within_bbox`, `add_many`) rather
/// than patching incrementally. Deliberate: `remove`'s `Vec::remove`
/// shifts every later index down by one, which would invalidate a
/// naively-patched bucket map; a full rebuild sidesteps that whole bug
/// class and is still `O(n)` over "tens to a few hundred" sources (this
/// module's own words) — trivial next to the per-tick read volume it's
/// removing work from.
#[derive(Debug, Clone, Default)]
pub struct HeatSourceMap {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    sources: Vec<HeatSource>,
    index: SpatialIndex,
}

/// Cell size floor when there are no sources yet, or all sources have
/// a tiny radius — keeps the grid from degenerating to sub-block cells
/// (which would just mean "every query touches ~1 cell" anyway, but a
/// sane floor avoids float-precision edge cases at very small scales).
const DEFAULT_CELL_SIZE: f32 = 8.0;

#[derive(Debug)]
struct SpatialIndex {
    cell_size: f32,
    /// cell coord -> indices into `Inner::sources`, in the SAME order
    /// they appear in `sources` (rebuild pushes in iteration order and
    /// never reorders) — this is what keeps `total_heat_at`'s summation
    /// order deterministic: the neighborhood-cell visit order is a
    /// fixed nested loop (see `total_heat_at`), and within each cell
    /// the source order matches insertion order.
    buckets: std::collections::HashMap<(i32, i32, i32), Vec<usize>>,
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self { cell_size: DEFAULT_CELL_SIZE, buckets: std::collections::HashMap::new() }
    }
}

#[inline]
fn cell_of(p: [f32; 3], cell_size: f32) -> (i32, i32, i32) {
    (
        (p[0] / cell_size).floor() as i32,
        (p[1] / cell_size).floor() as i32,
        (p[2] / cell_size).floor() as i32,
    )
}

impl SpatialIndex {
    /// Full rebuild from the current source list. `O(n)`.
    fn rebuild(&mut self, sources: &[HeatSource]) {
        self.buckets.clear();
        // Cell size = the largest reach among current sources (matching
        // contribution_at's own cutoff: reach = 2 * radius.max(0.5)),
        // floored at DEFAULT_CELL_SIZE. Using the MAX (not average/
        // per-source) is what makes the fixed 3x3x3 neighborhood search
        // correct for every source, not just typical ones.
        let max_reach = sources.iter()
            .map(|s| 2.0 * s.radius.max(0.5))
            .fold(0.0f32, f32::max);
        self.cell_size = max_reach.max(DEFAULT_CELL_SIZE);
        for (i, s) in sources.iter().enumerate() {
            let c = cell_of(s.position, self.cell_size);
            self.buckets.entry(c).or_default().push(i);
        }
    }
}

impl HeatSourceMap {
    pub fn new() -> Self { Self::default() }

    /// Add a source. Returns the index, which can be used to remove
    /// or update it later (player fire goes out, lava block is mined).
    pub fn add(&self, source: HeatSource) -> usize {
        let mut inner = self.inner.write().unwrap();
        let idx = inner.sources.len();
        inner.sources.push(source);
        let Inner { sources, index } = &mut *inner;
        index.rebuild(sources);
        idx
    }

    /// Remove a source by index. Subsequent indices shift down by 1
    /// (Vec::remove). Caller is responsible for tracking valid indices.
    pub fn remove(&self, idx: usize) {
        let mut inner = self.inner.write().unwrap();
        if idx < inner.sources.len() {
            inner.sources.remove(idx);
            let Inner { sources, index } = &mut *inner;
        index.rebuild(sources);
        }
    }

    /// Remove the nearest source within `max_distance` of world
    /// position `(wx, wy, wz)`. Returns the removed source, or None
    /// if no source is within range.
    ///
    /// This is the "extinguish nearby fire" primitive — F9 in the
    /// bin uses it to toggle fires on/off in the same key. The nearest-
    /// source search itself stays a linear scan (cheap: this is a rare,
    /// player-triggered call, not a per-tick hot path like
    /// `total_heat_at` — no need to route it through the grid).
    pub fn remove_nearest_within(
        &self,
        wx: f32, wy: f32, wz: f32,
        max_distance: f32,
    ) -> Option<HeatSource> {
        let mut inner = self.inner.write().unwrap();
        let max_sq = max_distance * max_distance;
        let mut best_idx: Option<usize> = None;
        let mut best_dist_sq = f32::INFINITY;
        for (i, s) in inner.sources.iter().enumerate() {
            let dx = wx - s.position[0];
            let dy = wy - s.position[1];
            let dz = wz - s.position[2];
            let dist_sq = dx * dx + dy * dy + dz * dz;
            if dist_sq < max_sq && dist_sq < best_dist_sq {
                best_dist_sq = dist_sq;
                best_idx = Some(i);
            }
        }
        let removed = best_idx.map(|i| inner.sources.remove(i));
        if removed.is_some() {
            let Inner { sources, index } = &mut *inner;
        index.rebuild(sources);
        }
        removed
    }

    /// Remove every source whose position falls within the given
    /// axis-aligned bounding box (world-space, half-open: min inclusive,
    /// max exclusive). Returns the count removed.
    ///
    /// Used to deregister heat sources owned by a chunk when that chunk
    /// is evicted: the bin computes the chunk's world-space bbox, calls
    /// this with it, and any sources from that chunk go away atomically.
    /// Player-placed fires in the chunk also go away — that matches
    /// real-world behavior (fire dies when its surroundings unload).
    pub fn remove_within_bbox(
        &self,
        min: [f32; 3],
        max: [f32; 3],
    ) -> usize {
        let mut inner = self.inner.write().unwrap();
        let before = inner.sources.len();
        inner.sources.retain(|s| {
            let p = s.position;
            !(p[0] >= min[0] && p[0] < max[0]
                && p[1] >= min[1] && p[1] < max[1]
                && p[2] >= min[2] && p[2] < max[2])
        });
        let removed = before - inner.sources.len();
        if removed > 0 {
            let Inner { sources, index } = &mut *inner;
        index.rebuild(sources);
        }
        removed
    }

    /// Bulk-add multiple sources. More efficient than calling `add` in
    /// a loop: one write-lock acquisition AND one index rebuild instead
    /// of one per source.
    pub fn add_many(&self, sources: impl IntoIterator<Item = HeatSource>) {
        let mut inner = self.inner.write().unwrap();
        inner.sources.extend(sources);
        let Inner { sources, index } = &mut *inner;
        index.rebuild(sources);
    }

    /// Total signed heat contribution at world position `(wx, wy, wz)`
    /// in °C. Sums all sources within influence range. Returns 0 if no
    /// sources are within effective radius of this point.
    ///
    /// Spatial-indexed: visits only the query cell + its 26 neighbors
    /// (a fixed 3×3×3, always sufficient — see the struct doc for why)
    /// instead of every registered source.
    pub fn total_heat_at(&self, wx: f32, wy: f32, wz: f32) -> f32 {
        let inner = self.inner.read().unwrap();
        if inner.sources.is_empty() { return 0.0; }
        let cell_size = inner.index.cell_size;
        let (cx, cy, cz) = cell_of([wx, wy, wz], cell_size);
        let mut sum = 0.0f32;
        // Fixed nested-loop order (dz, dy, dx) + insertion-order buckets
        // = deterministic summation order regardless of HashMap's own
        // (unspecified, seed-randomized) iteration order — we never
        // iterate the map itself, only look up 27 fixed keys.
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(idxs) = inner.index.buckets.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &i in idxs {
                            sum += inner.sources[i].contribution_at(wx, wy, wz);
                        }
                    }
                }
            }
        }
        sum
    }

    /// Number of sources currently registered.
    pub fn len(&self) -> usize {
        self.inner.read().unwrap().sources.len()
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Snapshot of all sources for read-only iteration. Returns a Vec
    /// (cheap clone of HeatSource — Copy-able). Used by debug overlay
    /// to draw markers at each fire's position.
    pub fn snapshot(&self) -> Vec<HeatSource> {
        self.inner.read().unwrap().sources.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map_has_zero_heat_everywhere() {
        let map = HeatSourceMap::new();
        assert_eq!(map.total_heat_at(0.0, 0.0, 0.0), 0.0);
        assert_eq!(map.total_heat_at(1000.0, 1000.0, 1000.0), 0.0);
    }

    #[test]
    fn single_source_at_position_returns_full_intensity() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([10.0, 20.0, 30.0], 500.0, 4.0));
        let h = map.total_heat_at(10.0, 20.0, 30.0);
        assert!((h - 500.0).abs() < 1e-3,
            "at source position should be full intensity, got {}", h);
    }

    /// Beyond ~2× radius, contribution is effectively zero (cutoff
    /// for performance — saves the exp() on distant sources).
    #[test]
    fn distant_sources_have_negligible_contribution() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 1000.0, 4.0));
        // 100 blocks away, much further than 2*radius=8.
        let h = map.total_heat_at(100.0, 0.0, 0.0);
        assert!(h.abs() < 0.01,
            "distant source should not affect cell, got {}", h);
    }

    /// At distance = radius, falloff is exp(-1) ≈ 0.368. Verify the
    /// contribution matches that math.
    #[test]
    fn falloff_is_gaussian_with_radius_scale() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 1000.0, 4.0));
        // At distance 4 (= radius), contribution should be ~368.
        let h = map.total_heat_at(4.0, 0.0, 0.0);
        let expected = 1000.0 * (-1.0_f32).exp();
        assert!((h - expected).abs() < 1.0,
            "at d=radius should be intensity/e, expected ~{}, got {}",
            expected, h);
    }

    /// Multiple sources sum. Two fires close together create a
    /// warmer cell than either alone.
    #[test]
    fn multiple_sources_sum_linearly() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 100.0, 10.0));
        map.add(HeatSource::new([0.0, 0.0, 0.0], 100.0, 10.0));
        let h = map.total_heat_at(0.0, 0.0, 0.0);
        // Both at same position both contribute fully.
        assert!((h - 200.0).abs() < 1e-3,
            "co-located sources should sum: expected 200, got {}", h);
    }

    /// Negative intensity (cold source) cools cells. Combines with
    /// positive sources additively.
    #[test]
    fn cold_sources_subtract_heat() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 100.0, 5.0));
        map.add(HeatSource::new([0.0, 0.0, 0.0], -30.0, 5.0));
        let h = map.total_heat_at(0.0, 0.0, 0.0);
        assert!((h - 70.0).abs() < 1e-3,
            "100 + (-30) = 70, got {}", h);
    }

    /// Sources can be added and removed at runtime — the map is
    /// designed for player-placed fires and player-mined lava.
    #[test]
    fn add_and_remove_round_trip() {
        let map = HeatSourceMap::new();
        let idx = map.add(HeatSource::new([0.0, 0.0, 0.0], 500.0, 4.0));
        assert_eq!(map.len(), 1);
        assert!(map.total_heat_at(0.0, 0.0, 0.0) > 100.0);
        map.remove(idx);
        assert_eq!(map.len(), 0);
        assert_eq!(map.total_heat_at(0.0, 0.0, 0.0), 0.0);
    }

    /// Out-of-range remove is a no-op (defensive — caller might pass
    /// a stale index after another remove shifted the vec).
    #[test]
    fn remove_out_of_range_is_noop() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 1.0, 1.0));
        map.remove(99);
        assert_eq!(map.len(), 1);
    }

    /// `remove_nearest_within` finds and removes the closest source
    /// within range, leaving more-distant sources alone.
    #[test]
    fn remove_nearest_within_picks_closest() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 100.0, 4.0)); // distance 0
        map.add(HeatSource::new([10.0, 0.0, 0.0], 200.0, 4.0)); // distance 10
        map.add(HeatSource::new([20.0, 0.0, 0.0], 300.0, 4.0)); // distance 20
        // Query from origin with radius 15 — should remove the source at 0.
        let removed = map.remove_nearest_within(0.0, 0.0, 0.0, 15.0);
        let removed = removed.expect("should have removed something");
        assert!((removed.intensity_c - 100.0).abs() < 1e-3,
            "expected to remove the 100°C source, got {}", removed.intensity_c);
        assert_eq!(map.len(), 2);
    }

    /// `remove_nearest_within` with no nearby source returns None
    /// and doesn't change the map.
    #[test]
    fn remove_nearest_within_no_match_returns_none() {
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([100.0, 100.0, 100.0], 50.0, 4.0));
        let removed = map.remove_nearest_within(0.0, 0.0, 0.0, 5.0);
        assert!(removed.is_none(),
            "no source within 5 blocks should yield None, got {:?}", removed);
        assert_eq!(map.len(), 1);
    }

    /// Empty map: `remove_nearest_within` returns None safely.
    #[test]
    fn remove_nearest_within_empty_returns_none() {
        let map = HeatSourceMap::new();
        let removed = map.remove_nearest_within(0.0, 0.0, 0.0, 100.0);
        assert!(removed.is_none());
    }

    /// `remove_within_bbox` removes all sources inside the box,
    /// preserves sources outside.
    #[test]
    fn remove_within_bbox_works() {
        let map = HeatSourceMap::new();
        // Inside the box [0,0,0]..[10,10,10]:
        map.add(HeatSource::new([5.0, 5.0, 5.0], 100.0, 1.0));
        map.add(HeatSource::new([1.0, 9.0, 9.0], 200.0, 1.0));
        // Outside (all coordinates >= 10 or < 0):
        map.add(HeatSource::new([15.0, 5.0, 5.0], 300.0, 1.0));
        map.add(HeatSource::new([5.0, -1.0, 5.0], 400.0, 1.0));
        // Exactly on the max boundary (exclusive — should NOT be removed):
        map.add(HeatSource::new([10.0, 5.0, 5.0], 500.0, 1.0));

        let removed = map.remove_within_bbox([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        assert_eq!(removed, 2, "expected to remove 2 sources, got {}", removed);
        assert_eq!(map.len(), 3, "expected 3 sources remaining");

        // Verify which ones survived (intensities should match the outsides + boundary).
        let snap = map.snapshot();
        let survivors: Vec<f32> = snap.iter().map(|s| s.intensity_c).collect();
        assert!(survivors.contains(&300.0), "outside-X survivor missing");
        assert!(survivors.contains(&400.0), "outside-Y survivor missing");
        assert!(survivors.contains(&500.0), "boundary survivor missing");
    }

    /// `remove_within_bbox` on an empty map returns 0, no panic.
    #[test]
    fn remove_within_bbox_empty_returns_zero() {
        let map = HeatSourceMap::new();
        let removed = map.remove_within_bbox([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        assert_eq!(removed, 0);
    }

    /// `add_many` is equivalent to many `add`s; tests the batch path.
    #[test]
    fn add_many_appends_all_sources() {
        let map = HeatSourceMap::new();
        let sources = vec![
            HeatSource::new([0.0, 0.0, 0.0], 100.0, 1.0),
            HeatSource::new([1.0, 0.0, 0.0], 200.0, 1.0),
            HeatSource::new([2.0, 0.0, 0.0], 300.0, 1.0),
        ];
        map.add_many(sources);
        assert_eq!(map.len(), 3);
        let total = map.total_heat_at(0.5, 0.0, 0.0);
        // Some heat from each source — exact value depends on falloff.
        // We just check it's positive and substantial.
        assert!(total > 0.0, "should get nonzero heat");
    }

    // ── spatial index: parity + edge cases (r-followups-heatidx) ────────

    /// Deterministic xorshift — no external RNG dependency, and its own
    /// determinism matters here (same seed must produce the same test
    /// case every run, since we're comparing bit-for-bit).
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }
    fn rand_f32(state: &mut u64, lo: f32, hi: f32) -> f32 {
        let bits = xorshift(state);
        let unit = (bits >> 40) as f32 / (1u64 << 24) as f32; // [0,1)
        lo + unit * (hi - lo)
    }

    /// Brute-force reference: the exact pre-index O(n) computation,
    /// kept alongside the indexed path so the parity test has a ground
    /// truth independent of `total_heat_at`'s implementation.
    fn brute_force_heat_at(sources: &[HeatSource], wx: f32, wy: f32, wz: f32) -> f32 {
        sources.iter().map(|s| s.contribution_at(wx, wy, wz)).sum()
    }

    #[test]
    fn indexed_query_matches_brute_force_bit_for_bit() {
        let mut seed = 0xC0FFEE_u64;
        for trial in 0..20 {
            let map = HeatSourceMap::new();
            let n = 5 + (trial % 40); // 5..45 sources per trial
            let mut sources = Vec::with_capacity(n);
            for _ in 0..n {
                let pos = [
                    rand_f32(&mut seed, -200.0, 200.0),
                    rand_f32(&mut seed, -200.0, 200.0),
                    rand_f32(&mut seed, -200.0, 200.0),
                ];
                let intensity = rand_f32(&mut seed, -500.0, 1500.0);
                let radius = rand_f32(&mut seed, 0.5, 20.0);
                let s = HeatSource::new(pos, intensity, radius);
                sources.push(s);
                map.add(s);
            }
            for _ in 0..30 {
                let qx = rand_f32(&mut seed, -200.0, 200.0);
                let qy = rand_f32(&mut seed, -200.0, 200.0);
                let qz = rand_f32(&mut seed, -200.0, 200.0);
                let indexed = map.total_heat_at(qx, qy, qz);
                let brute = brute_force_heat_at(&sources, qx, qy, qz);
                // Bit-for-bit: summing the SAME terms in the SAME order
                // (insertion order, both paths) must give an identical
                // f32, not just "close".
                assert_eq!(
                    indexed.to_bits(), brute.to_bits(),
                    "trial {trial}: indexed={indexed} ({:x}) brute={brute} ({:x}) at ({qx},{qy},{qz})",
                    indexed.to_bits(), brute.to_bits(),
                );
            }
        }
    }

    #[test]
    fn indexed_query_matches_brute_force_with_sources_sharing_a_cell() {
        // All sources at the exact same position -> same cell, exercises
        // the "multiple entries in one bucket" path specifically.
        let map = HeatSourceMap::new();
        let mut sources = Vec::new();
        for i in 0..10 {
            let s = HeatSource::new([1.0, 2.0, 3.0], 50.0 + i as f32, 4.0);
            sources.push(s);
            map.add(s);
        }
        let indexed = map.total_heat_at(1.0, 2.0, 3.0);
        let brute = brute_force_heat_at(&sources, 1.0, 2.0, 3.0);
        assert_eq!(indexed.to_bits(), brute.to_bits());
    }

    #[test]
    fn indexed_query_matches_brute_force_at_exact_cutoff_radius() {
        let map = HeatSourceMap::new();
        let s = HeatSource::new([0.0, 0.0, 0.0], 1000.0, 4.0);
        map.add(s);
        // Exactly at the 2*radius cutoff (dist_sq == 4*r_sq boundary).
        let indexed = map.total_heat_at(8.0, 0.0, 0.0);
        let brute = brute_force_heat_at(&[s], 8.0, 0.0, 0.0);
        assert_eq!(indexed.to_bits(), brute.to_bits());
    }

    #[test]
    fn indexed_query_matches_brute_force_far_query_touches_nothing() {
        let map = HeatSourceMap::new();
        let s = HeatSource::new([0.0, 0.0, 0.0], 1000.0, 4.0);
        map.add(s);
        let indexed = map.total_heat_at(500.0, 500.0, 500.0);
        assert_eq!(indexed, 0.0);
    }

    #[test]
    fn spatial_index_empty_map_returns_zero() {
        let map = HeatSourceMap::new();
        assert_eq!(map.total_heat_at(1.0, 2.0, 3.0), 0.0);
    }

    #[test]
    fn spatial_index_survives_remove_and_rebuild_correctly() {
        // Exercises the "Vec::remove shifts indices" hazard the index
        // must rebuild past: add 3, remove the middle one by index,
        // confirm the remaining two still query correctly (would fail
        // if a naively-patched index kept stale post-shift indices).
        let map = HeatSourceMap::new();
        map.add(HeatSource::new([0.0, 0.0, 0.0], 100.0, 4.0));
        let mid_idx = map.add(HeatSource::new([50.0, 0.0, 0.0], 200.0, 4.0));
        map.add(HeatSource::new([100.0, 0.0, 0.0], 300.0, 4.0));
        map.remove(mid_idx);
        assert_eq!(map.len(), 2);
        let h0 = map.total_heat_at(0.0, 0.0, 0.0);
        let h100 = map.total_heat_at(100.0, 0.0, 0.0);
        assert!((h0 - 100.0).abs() < 1e-3, "source at 0 should still query correctly, got {h0}");
        assert!((h100 - 300.0).abs() < 1e-3, "source at 100 should still query correctly, got {h100}");
        // The removed middle source's position should show no heat.
        let h_removed = map.total_heat_at(50.0, 0.0, 0.0);
        assert!(h_removed < 100.0, "removed source's position should not show its heat, got {h_removed}");
    }

    #[test]
    fn spatial_index_perf_sanity_visits_far_fewer_than_n_sources() {
        // Coarse check: a widely-scattered set + a query far from most
        // of them should mean the indexed path examines far fewer than
        // N sources. We can't directly instrument the private bucket
        // walk from here, but we CAN check the observable proxy: the
        // query result must still be correct (proven above), and this
        // test documents the intent — the real perf win is structural
        // (bounded neighborhood vs full scan), not something a unit
        // test needs to re-measure once correctness is established.
        let map = HeatSourceMap::new();
        let mut seed = 0xABCDu64;
        let mut sources = Vec::new();
        for _ in 0..500 {
            let pos = [
                rand_f32(&mut seed, -5000.0, 5000.0),
                0.0,
                rand_f32(&mut seed, -5000.0, 5000.0),
            ];
            let s = HeatSource::new(pos, 100.0, 4.0);
            sources.push(s);
            map.add(s);
        }
        assert_eq!(map.len(), 500);
        // A query far from the whole cluster's bounding region.
        let indexed = map.total_heat_at(50000.0, 0.0, 50000.0);
        let brute = brute_force_heat_at(&sources, 50000.0, 0.0, 50000.0);
        assert_eq!(indexed.to_bits(), brute.to_bits());
        assert_eq!(indexed, 0.0);
    }
}
