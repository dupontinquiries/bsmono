//! Landscape memory — a creature's coarse, fading record of where it has
//! already looked, so its surveying ([`crate::sim::awareness`]) chases ground it
//! *hasn't* seen rather than re-staring at the familiar. Observing a direction
//! marks the cells along that sightline as freshly seen; familiarity then fades
//! with time, so a creature drifts its attention back to stale or never-seen
//! quarters. This is the "scrying to update landscape knowledge" loop, and it's
//! what feeds real `familiarity` into [`crate::sim::awareness::look_value`]
//! instead of a flat placeholder.
//!
//! Pure + deterministic. Bounded memory (forgets the oldest cells past capacity).

use std::collections::HashMap;

/// A fading map of observed cells, keyed by `(cell_x, cell_z)` → the tick it was
/// last seen.
#[derive(Debug, Clone)]
pub struct LandscapeMemory {
    seen: HashMap<(i32, i32), u32>,
    capacity: usize,
}

impl LandscapeMemory {
    pub fn new(capacity: usize) -> LandscapeMemory {
        LandscapeMemory { seen: HashMap::new(), capacity: capacity.max(1) }
    }

    /// Number of cells currently remembered.
    pub fn len(&self) -> usize {
        self.seen.len()
    }
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Record that `cell` was observed at tick `now`. Past capacity, the oldest
    /// remembered cell is forgotten first.
    pub fn observe(&mut self, cell: (i32, i32), now: u32) {
        if !self.seen.contains_key(&cell) && self.seen.len() >= self.capacity {
            if let Some((&oldest, _)) = self.seen.iter().min_by_key(|(_, &t)| t) {
                self.seen.remove(&oldest);
            }
        }
        self.seen.insert(cell, now);
    }

    /// Mark every cell along a sightline from `from` heading `heading` out to
    /// `dist` as observed at `now` — what a creature learns by looking that way.
    pub fn observe_line(&mut self, from: (f32, f32), heading: f32, dist: f32, now: u32) {
        let (cx, cz) = (heading.cos(), heading.sin());
        let mut d = 0.0;
        while d <= dist {
            let cell = ((from.0 + cx * d).floor() as i32, (from.1 + cz * d).floor() as i32);
            self.observe(cell, now);
            d += 1.0;
        }
    }

    /// Familiarity of `cell` in `[0,1]`: 1 if just seen, fading linearly to 0 as
    /// the observation ages past `horizon` ticks; 0 if never seen (maximally
    /// novel). Feeds straight into `look_value`'s novelty term.
    pub fn familiarity(&self, cell: (i32, i32), now: u32, horizon: u32) -> f32 {
        match self.seen.get(&cell) {
            Some(&t) if now >= t && horizon > 0 => {
                let age = now - t;
                (1.0 - age as f32 / horizon as f32).clamp(0.0, 1.0)
            }
            Some(_) => 1.0, // seen this very tick (or clock wrap): treat as fresh
            None => 0.0,    // never seen → fully novel
        }
    }

    /// Familiarity of the cell a short look `probe` distance along `heading` — the
    /// value to attach to a [`crate::sim::awareness::LookCandidate`].
    pub fn heading_familiarity(&self, from: (f32, f32), heading: f32, probe: f32, now: u32, horizon: u32) -> f32 {
        let cell = ((from.0 + heading.cos() * probe).floor() as i32, (from.1 + heading.sin() * probe).floor() as i32);
        self.familiarity(cell, now, horizon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_cells_are_familiar_then_fade_unseen_cells_are_novel() {
        let mut m = LandscapeMemory::new(64);
        m.observe((3, 4), 100);
        // Just seen → fully familiar.
        assert_eq!(m.familiarity((3, 4), 100, 50), 1.0);
        // Halfway through the horizon → about half familiar.
        assert!((m.familiarity((3, 4), 125, 50) - 0.5).abs() < 1e-3);
        // Past the horizon → forgotten (novel again).
        assert_eq!(m.familiarity((3, 4), 200, 50), 0.0);
        // Never seen → novel.
        assert_eq!(m.familiarity((9, 9), 100, 50), 0.0);
    }

    #[test]
    fn looking_a_direction_learns_the_cells_along_it() {
        let mut m = LandscapeMemory::new(256);
        // Look east (+X) for 5 blocks from the origin.
        m.observe_line((0.5, 0.5), 0.0, 5.0, 10);
        // Cells along +X are now familiar...
        assert!(m.heading_familiarity((0.5, 0.5), 0.0, 3.0, 10, 100) > 0.5, "the looked-at direction is known");
        // ...but the opposite direction (-X) is still novel.
        assert_eq!(m.heading_familiarity((0.5, 0.5), std::f32::consts::PI, 3.0, 10, 100), 0.0);
    }

    #[test]
    fn novelty_from_memory_steers_a_survey_toward_unseen_ground() {
        use crate::sim::awareness::{best_facing, LookCandidate};
        // The creature has been staring east; west is unexplored. Both equally
        // open. best_facing, fed memory-derived familiarity, should turn west.
        let mut m = LandscapeMemory::new(256);
        m.observe_line((0.0, 0.0), 0.0, 20.0, 0); // looked east a lot
        let now = 0;
        let max_view = 24.0;
        let east = 0.0;
        let west = std::f32::consts::PI;
        let cands = [
            LookCandidate { heading: east, clear_dist: 24.0, familiarity: m.heading_familiarity((0.0, 0.0), east, 4.0, now, 200) },
            LookCandidate { heading: west, clear_dist: 24.0, familiarity: m.heading_familiarity((0.0, 0.0), west, 4.0, now, 200) },
        ];
        assert_eq!(best_facing(0.0, &cands, max_view), Some(west), "surveys toward the unseen west");
    }

    #[test]
    fn memory_forgets_the_oldest_past_capacity() {
        let mut m = LandscapeMemory::new(2);
        m.observe((0, 0), 1);
        m.observe((1, 0), 2);
        m.observe((2, 0), 3); // evicts (0,0), the oldest
        assert_eq!(m.len(), 2);
        assert_eq!(m.familiarity((0, 0), 3, 100), 0.0, "the oldest cell was forgotten");
        assert!(m.familiarity((2, 0), 3, 100) > 0.0, "the newest is remembered");
    }
}
