//! The toroidal chunk window — where a chunk lives in memory, and what
//! changes when the player moves.
//!
//! A fixed box of `nx × ny × nz` slots is centred on the player's chunk.
//! Chunk `(x, y, z)` lives at slot `(x mod nx, y mod ny, z mod nz)` and
//! never moves: when the centre advances by one along X the plane of
//! chunks at `cx − rx` leaves and the plane at `cx + rx + 1` enters, and
//! with `nx = 2·rx + 1` those two planes have the same slot indices. So a
//! move is a write into slots that already exist and nothing is
//! reallocated, rehashed, or freed. See
//! `docs/claude-notes/chunk-window-design.md`.
//!
//! This module is the headless core: coordinates, slots, generations,
//! and the diff a recentre produces. It owns no chunk data — a caller
//! keeps parallel `Vec<T>`s indexed by `SlotIndex`. It has no
//! dependency on the rest of the engine so it can be tested with
//! `rustc --test` alone, and it uses its own tiny `Coord` rather than
//! `ChunkCoord` for the same reason; the wrapper that maps one to the
//! other is a few lines in the store.
//!
//! Generations: every slot carries a `generation` that increments each
//! time the slot changes occupant. A result produced for an occupant
//! (a mesh, a generated chunk) carries the generation it was produced
//! for; on arrival, `Window::accepts(slot, generation)` says whether it
//! is still for the current occupant. This is the existing epoch/version
//! idea made structural.

#![allow(dead_code)]

/// A chunk coordinate. Deliberately not `ChunkCoord` (no engine dep).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Coord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Coord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } }
    /// Chebyshev distance — the box metric the window is defined in.
    pub fn box_distance(self, o: Coord) -> i32 {
        (self.x - o.x).abs().max((self.y - o.y).abs()).max((self.z - o.z).abs())
    }
    pub fn distance_sq(self, o: Coord) -> i64 {
        let dx = (self.x - o.x) as i64;
        let dy = (self.y - o.y) as i64;
        let dz = (self.z - o.z) as i64;
        dx * dx + dy * dy + dz * dz
    }
}

/// Index into the caller's parallel arrays. `0 .. Window::slot_count()`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SlotIndex(pub u32);

/// What one slot holds right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotState {
    /// The chunk this slot represents. Valid iff `occupied`.
    pub coord: Coord,
    /// Increments on every occupant change (including → empty).
    pub generation: u32,
    /// False after the occupant left and before the next one is
    /// installed. The caller's data for the slot is stale but present.
    pub occupied: bool,
}

/// The slots a recentre touched. `left` and `entered` are the same
/// slots in the same order (that is the point); `entered` is sorted by
/// distance to the new centre so it can be handed straight to a loader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recentre {
    pub left: Vec<(SlotIndex, Coord)>,
    pub entered: Vec<(SlotIndex, Coord)>,
}

#[derive(Clone, Debug)]
pub struct Window {
    nx: i32,
    ny: i32,
    nz: i32,
    /// Half-extents: nx = 2*rx + 1, etc.
    rx: i32,
    ry: i32,
    rz: i32,
    centre: Coord,
    slots: Vec<SlotState>,
}

impl Window {
    /// A window of half-extent `(rx, ry, rz)` chunks around `centre`. All
    /// slots start empty (generation 0, unoccupied) and `initial_fill()`
    /// lists them by distance so the first load is the same code as a
    /// recentre.
    pub fn new(rx: i32, ry: i32, rz: i32, centre: Coord) -> Self {
        assert!(rx >= 0 && ry >= 0 && rz >= 0);
        let (nx, ny, nz) = (2 * rx + 1, 2 * ry + 1, 2 * rz + 1);
        let n = (nx as usize) * (ny as usize) * (nz as usize);
        let mut w = Self {
            nx, ny, nz, rx, ry, rz, centre,
            slots: vec![SlotState { coord: Coord::default(), generation: 0, occupied: false }; n],
        };
        // Pre-assign the coordinate every slot *would* hold so
        // `coord_at(slot)` is meaningful even before install.
        for z in centre.z - rz..=centre.z + rz {
            for y in centre.y - ry..=centre.y + ry {
                for x in centre.x - rx..=centre.x + rx {
                    let c = Coord::new(x, y, z);
                    let s = w.slot_of(c);
                    w.slots[s.0 as usize].coord = c;
                }
            }
        }
        w
    }

    pub fn centre(&self) -> Coord { self.centre }
    pub fn dims(&self) -> (i32, i32, i32) { (self.nx, self.ny, self.nz) }
    pub fn half_extents(&self) -> (i32, i32, i32) { (self.rx, self.ry, self.rz) }
    pub fn slot_count(&self) -> usize { self.slots.len() }

    /// Is `c` inside the current box?
    pub fn contains(&self, c: Coord) -> bool {
        (c.x - self.centre.x).abs() <= self.rx
            && (c.y - self.centre.y).abs() <= self.ry
            && (c.z - self.centre.z).abs() <= self.rz
    }

    /// The slot `c` maps to — pure arithmetic, defined for every coord.
    /// Whether the slot currently *holds* `c` is `slot(c)`.
    #[inline]
    pub fn slot_of(&self, c: Coord) -> SlotIndex {
        let sx = c.x.rem_euclid(self.nx);
        let sy = c.y.rem_euclid(self.ny);
        let sz = c.z.rem_euclid(self.nz);
        SlotIndex(((sz * self.ny + sy) * self.nx + sx) as u32)
    }

    /// The slot holding `c`, if `c` is in the box and installed.
    pub fn slot(&self, c: Coord) -> Option<SlotIndex> {
        if !self.contains(c) { return None; }
        let s = self.slot_of(c);
        let st = &self.slots[s.0 as usize];
        if st.occupied && st.coord == c { Some(s) } else { None }
    }

    /// The coordinate slot `s` represents (installed or destined).
    pub fn coord_at(&self, s: SlotIndex) -> Coord { self.slots[s.0 as usize].coord }
    pub fn state(&self, s: SlotIndex) -> SlotState { self.slots[s.0 as usize] }

    /// Mark `s` as holding its destined coordinate; bumps the generation.
    /// Returns the generation the caller should stamp on anything it
    /// derives from this occupant.
    pub fn install(&mut self, s: SlotIndex) -> u32 {
        let st = &mut self.slots[s.0 as usize];
        st.generation = st.generation.wrapping_add(1);
        st.occupied = true;
        st.generation
    }

    /// Does a result produced for `(s, generation)` still belong to the
    /// current occupant of `s`?
    pub fn accepts(&self, s: SlotIndex, generation: u32) -> bool {
        let st = &self.slots[s.0 as usize];
        st.occupied && st.generation == generation
    }

    /// Everything the box wants, by distance to the centre — the first
    /// load. Same shape as `Recentre::entered`.
    pub fn initial_fill(&self) -> Vec<(SlotIndex, Coord)> {
        let mut v: Vec<(SlotIndex, Coord)> = self.slots.iter().enumerate()
            .filter(|(_, st)| !st.occupied)
            .map(|(i, st)| (SlotIndex(i as u32), st.coord))
            .collect();
        let c = self.centre;
        v.sort_by_key(|(_, k)| (k.distance_sq(c), k.x, k.y, k.z));
        v
    }

    /// Move the box so it is centred on `new_centre`. Returns the slots
    /// whose occupant left and the coordinates that now belong in them,
    /// nearest first. The caller owes the loader `entered` and may drop or
    /// reuse whatever it held for `left`; the slot generations are bumped
    /// here so in-flight results for the old occupants are rejected by
    /// `accepts`.
    ///
    /// A jump larger than the box (teleport) simply turns over every slot.
    pub fn recentre(&mut self, new_centre: Coord) -> Recentre {
        if new_centre == self.centre { return Recentre::default(); }
        let old = self.centre;
        self.centre = new_centre;
        let mut left = Vec::new();
        let mut entered = Vec::new();
        // Walk the NEW box; any slot whose destined coord differs from
        // what it held is a turnover. This is O(box) for a jump and — the
        // common case — the same cost as walking just the entering
        // planes would be, because the check per slot is one compare.
        // A plane-only walk is a micro-optimisation for later; the
        // *allocation* behaviour is what this module fixes.
        for z in new_centre.z - self.rz..=new_centre.z + self.rz {
            for y in new_centre.y - self.ry..=new_centre.y + self.ry {
                for x in new_centre.x - self.rx..=new_centre.x + self.rx {
                    let c = Coord::new(x, y, z);
                    let s = self.slot_of(c);
                    let st = &mut self.slots[s.0 as usize];
                    if st.coord != c {
                        let was = st.coord;
                        // The old coord was in the old box by construction.
                        debug_assert!(
                            (was.x - old.x).abs() <= self.rx
                                && (was.y - old.y).abs() <= self.ry
                                && (was.z - old.z).abs() <= self.rz
                        );
                        if st.occupied {
                            left.push((s, was));
                        }
                        st.coord = c;
                        st.occupied = false;
                        st.generation = st.generation.wrapping_add(1);
                        entered.push((s, c));
                    }
                }
            }
        }
        entered.sort_by_key(|(_, k)| (k.distance_sq(new_centre), k.x, k.y, k.z));
        Recentre { left, entered }
    }

    /// Slots in the box by distance to the centre — the render/simulate
    /// order. Allocation-free iteration for callers that want it.
    pub fn slots_by_distance(&self) -> Vec<(SlotIndex, Coord, i64)> {
        let c = self.centre;
        let mut v: Vec<(SlotIndex, Coord, i64)> = self.slots.iter().enumerate()
            .map(|(i, st)| (SlotIndex(i as u32), st.coord, st.coord.distance_sq(c)))
            .collect();
        v.sort_by_key(|(_, k, d)| (*d, k.x, k.y, k.z));
        v
    }

    /// How many slots are installed.
    pub fn occupied_count(&self) -> usize {
        self.slots.iter().filter(|s| s.occupied).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(rx: i32, ry: i32, rz: i32, c: Coord) -> Window {
        let mut w = Window::new(rx, ry, rz, c);
        for (s, _) in w.initial_fill() { w.install(s); }
        w
    }

    #[test]
    fn every_coord_in_the_box_has_a_distinct_slot() {
        let w = Window::new(3, 1, 2, Coord::new(-7, 4, 100));
        let mut seen = std::collections::HashSet::new();
        for z in 98..=102 { for y in 3..=5 { for x in -10..=-4 {
            let s = w.slot_of(Coord::new(x, y, z));
            assert!(seen.insert(s), "slot {:?} used twice", s);
            assert_eq!(w.coord_at(s), Coord::new(x, y, z));
        }}}
        assert_eq!(seen.len(), w.slot_count());
        assert_eq!(w.slot_count(), 7 * 3 * 5);
    }

    #[test]
    fn negative_coords_wrap_correctly() {
        let w = Window::new(2, 0, 2, Coord::new(0, 0, 0));
        // -2 mod 5 = 3, 3 mod 5 = 3: the two planes that swap on a move.
        assert_eq!(w.slot_of(Coord::new(-2, 0, 0)), w.slot_of(Coord::new(3, 0, 0)));
        assert_ne!(w.slot_of(Coord::new(-2, 0, 0)), w.slot_of(Coord::new(2, 0, 0)));
    }

    #[test]
    fn slot_lookup_needs_install() {
        let mut w = Window::new(1, 1, 1, Coord::new(0, 0, 0));
        let c = Coord::new(1, -1, 0);
        assert_eq!(w.slot(c), None, "in box but not installed");
        let s = w.slot_of(c);
        let g = w.install(s);
        assert_eq!(w.slot(c), Some(s));
        assert!(w.accepts(s, g));
        assert!(!w.accepts(s, g - 1));
        assert_eq!(w.slot(Coord::new(2, 0, 0)), None, "outside the box");
    }

    #[test]
    fn one_step_turns_over_exactly_one_plane_and_reuses_its_slots() {
        let mut w = filled(3, 1, 3, Coord::new(0, 0, 0));
        let r = w.recentre(Coord::new(1, 0, 0));
        assert_eq!(r.left.len(), 7 * 3, "one YZ plane leaves");
        assert_eq!(r.entered.len(), 7 * 3);
        for (s, c) in &r.left { assert_eq!(c.x, -3); let _ = s; }
        for (s, c) in &r.entered { assert_eq!(c.x, 4); let _ = s; }
        let left: std::collections::HashSet<_> = r.left.iter().map(|(s, _)| *s).collect();
        let entered: std::collections::HashSet<_> = r.entered.iter().map(|(s, _)| *s).collect();
        assert_eq!(left, entered, "entering chunks land in the leaving chunks' slots");
        assert_eq!(w.occupied_count(), w.slot_count() - 21);
        // Nothing else moved: the chunk at (0,0,0) is where it was.
        assert_eq!(w.slot(Coord::new(0, 0, 0)), Some(w.slot_of(Coord::new(0, 0, 0))));
    }

    #[test]
    fn entered_is_nearest_first() {
        let mut w = filled(3, 1, 3, Coord::new(0, 0, 0));
        let r = w.recentre(Coord::new(0, 0, 1));
        let d: Vec<i64> = r.entered.iter().map(|(_, c)| c.distance_sq(Coord::new(0, 0, 1))).collect();
        let mut sorted = d.clone(); sorted.sort();
        assert_eq!(d, sorted);
        assert_eq!(r.entered[0].1, Coord::new(0, 0, 4));
    }

    #[test]
    fn diagonal_step_turns_over_three_planes_minus_overlap() {
        let mut w = filled(2, 2, 2, Coord::new(0, 0, 0));
        let r = w.recentre(Coord::new(1, 1, 1));
        // 125 - 64 (the 4×4×4 cube that stays) = 61.
        assert_eq!(r.entered.len(), 61);
        assert_eq!(r.left.len(), 61);
    }

    #[test]
    fn recentre_bumps_generation_so_stale_results_are_rejected() {
        let mut w = Window::new(1, 0, 1, Coord::new(0, 0, 0));
        let s = w.slot_of(Coord::new(-1, 0, 0));
        let g_old = w.install(s);
        let r = w.recentre(Coord::new(1, 0, 0));
        assert!(r.left.iter().any(|(ls, lc)| *ls == s && *lc == Coord::new(-1, 0, 0)));
        assert!(!w.accepts(s, g_old), "a mesh for (-1,0,0) must not land in the slot now destined for (2,0,0)");
        let g_new = w.install(s);
        assert!(g_new > g_old);
        assert!(w.accepts(s, g_new));
        assert_eq!(w.slot(Coord::new(2, 0, 0)), Some(s));
    }

    #[test]
    fn teleport_turns_over_everything() {
        let mut w = filled(2, 1, 2, Coord::new(0, 0, 0));
        let r = w.recentre(Coord::new(1000, 50, -1000));
        assert_eq!(r.left.len(), w.slot_count());
        assert_eq!(r.entered.len(), w.slot_count());
        assert_eq!(w.occupied_count(), 0);
    }

    #[test]
    fn no_op_recentre_is_empty() {
        let mut w = filled(1, 1, 1, Coord::new(5, 5, 5));
        assert_eq!(w.recentre(Coord::new(5, 5, 5)), Recentre::default());
        assert_eq!(w.occupied_count(), 27);
    }

    #[test]
    fn walking_far_never_double_counts_a_slot() {
        // 200 unit steps in a spiral; every step's left and entered sets
        // must be equal and every slot must hold a coord inside the box.
        let mut w = filled(4, 2, 4, Coord::new(0, 0, 0));
        let mut c = Coord::new(0, 0, 0);
        let dirs = [(1, 0, 0), (0, 0, 1), (-1, 0, 0), (0, 0, -1), (0, 1, 0), (0, -1, 0)];
        for i in 0..200 {
            let d = dirs[i % dirs.len()];
            c = Coord::new(c.x + d.0, c.y + d.1, c.z + d.2);
            let r = w.recentre(c);
            let l: std::collections::HashSet<_> = r.left.iter().map(|(s, _)| *s).collect();
            let e: std::collections::HashSet<_> = r.entered.iter().map(|(s, _)| *s).collect();
            assert_eq!(l, e);
            for (s, _) in r.entered { w.install(s); }
            for i in 0..w.slot_count() {
                let st = w.state(SlotIndex(i as u32));
                assert!(w.contains(st.coord));
                assert_eq!(w.slot_of(st.coord), SlotIndex(i as u32));
            }
            assert_eq!(w.occupied_count(), w.slot_count());
        }
    }

    #[test]
    fn slots_by_distance_starts_at_the_centre() {
        let w = filled(2, 2, 2, Coord::new(3, -3, 3));
        let v = w.slots_by_distance();
        assert_eq!(v[0].1, Coord::new(3, -3, 3));
        assert_eq!(v[0].2, 0);
        assert_eq!(v.last().unwrap().2, 12);
    }
}
