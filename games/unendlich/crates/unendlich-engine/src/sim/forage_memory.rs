//! Forage memory — a creature's small, fading recollection of *where it has
//! seen food*. It's the food-tagged sibling of [`crate::sim::landscape_memory`]:
//! while looking around, a creature notes grass it spots (with coordinates);
//! when hunger sets in it asks this memory for the nearest place it remembers
//! food and heads there. Nothing here scans the world — a site is only known if
//! the creature actually saw it, and the memory fades, so eaten or stale grass
//! drops out and gets re-discovered by looking again.
//!
//! Pure + deterministic. Bounded (forgets the oldest site past capacity).

/// A remembered food location (a cell) and the tick it was last seen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoodSite {
    pub cell: (i32, i32),
    pub last_seen: u32,
}

/// A creature's fading map of known food sites.
#[derive(Debug, Clone)]
pub struct ForageMemory {
    sites: Vec<FoodSite>,
    capacity: usize,
}

impl ForageMemory {
    pub fn new(capacity: usize) -> ForageMemory {
        ForageMemory { sites: Vec::new(), capacity: capacity.max(1) }
    }

    pub fn known_count(&self) -> usize {
        self.sites.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    /// Note that food was seen at `cell` at tick `now`. Refreshes an existing
    /// site or records a new one; past capacity, the oldest site is forgotten.
    pub fn remember(&mut self, cell: (i32, i32), now: u32) {
        if let Some(s) = self.sites.iter_mut().find(|s| s.cell == cell) {
            s.last_seen = now;
            return;
        }
        if self.sites.len() >= self.capacity {
            if let Some((idx, _)) = self.sites.iter().enumerate().min_by_key(|(_, s)| s.last_seen) {
                self.sites.swap_remove(idx);
            }
        }
        self.sites.push(FoodSite { cell, last_seen: now });
    }

    /// Forget a specific site — e.g. the creature ate the grass and it's gone.
    pub fn forget(&mut self, cell: (i32, i32)) {
        self.sites.retain(|s| s.cell != cell);
    }

    /// Drop sites not seen within `horizon` ticks (memory fades).
    pub fn forget_stale(&mut self, now: u32, horizon: u32) {
        self.sites.retain(|s| now.saturating_sub(s.last_seen) <= horizon);
    }

    /// The nearest still-fresh known food site to `from`, if any. Stale sites
    /// (older than `horizon`) are ignored. This is the goal a hungry creature
    /// heads toward.
    pub fn nearest(&self, from: (f32, f32), now: u32, horizon: u32) -> Option<(i32, i32)> {
        self.sites
            .iter()
            .filter(|s| now.saturating_sub(s.last_seen) <= horizon)
            .min_by(|a, b| {
                let da = cell_dist2(from, a.cell);
                let db = cell_dist2(from, b.cell);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|s| s.cell)
    }
}

/// Squared distance from a float position to a cell's center.
fn cell_dist2(from: (f32, f32), cell: (i32, i32)) -> f32 {
    let cx = cell.0 as f32 + 0.5;
    let cz = cell.1 as f32 + 0.5;
    (from.0 - cx).powi(2) + (from.1 - cz).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_food_and_returns_the_nearest() {
        let mut m = ForageMemory::new(16);
        m.remember((10, 0), 1);
        m.remember((2, 0), 1);
        m.remember((30, 0), 1);
        // From the origin, (2,0) is closest.
        assert_eq!(m.nearest((0.0, 0.0), 1, 1000), Some((2, 0)));
        // From near (30,0), that one wins.
        assert_eq!(m.nearest((29.0, 0.0), 1, 1000), Some((30, 0)));
        // Empty memory → no goal.
        assert_eq!(ForageMemory::new(4).nearest((0.0, 0.0), 1, 1000), None);
    }

    #[test]
    fn stale_sites_are_ignored_and_pruned() {
        let mut m = ForageMemory::new(16);
        m.remember((5, 5), 10);
        // Within horizon → known.
        assert_eq!(m.nearest((0.0, 0.0), 30, 50), Some((5, 5)));
        // Past horizon → not offered as a goal...
        assert_eq!(m.nearest((0.0, 0.0), 100, 50), None);
        // ...and forget_stale removes it.
        m.forget_stale(100, 50);
        assert!(m.is_empty());
    }

    #[test]
    fn eaten_grass_is_forgotten_and_reseeing_refreshes() {
        let mut m = ForageMemory::new(16);
        m.remember((7, 7), 1);
        m.forget((7, 7)); // ate it
        assert_eq!(m.nearest((0.0, 0.0), 1, 1000), None);
        // Re-seeing the same cell records it again at the new time.
        m.remember((7, 7), 5);
        m.remember((7, 7), 9); // refresh, not duplicate
        assert_eq!(m.known_count(), 1);
        assert_eq!(m.nearest((0.0, 0.0), 9, 1000), Some((7, 7)));
    }

    #[test]
    fn forgets_the_oldest_site_past_capacity() {
        let mut m = ForageMemory::new(2);
        m.remember((0, 0), 1);
        m.remember((1, 0), 2);
        m.remember((2, 0), 3); // evicts (0,0), the oldest
        assert_eq!(m.known_count(), 2);
        // (0,0) is gone; the two newest remain.
        let n = m.nearest((0.0, 0.0), 3, 1000);
        assert!(n == Some((1, 0)), "oldest forgotten, nearest of the rest is (1,0), got {n:?}");
    }
}
