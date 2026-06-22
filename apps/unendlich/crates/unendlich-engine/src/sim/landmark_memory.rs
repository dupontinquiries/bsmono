//! `sim::landmark_memory` — a dragon's spatial memory: a small set of salient
//! places it has noticed, kept by importance and faded by time.
//!
//! The memory holds at most [`CAPACITY`] landmarks. [`Memory::observe`] records
//! a place (reinforcing salience if it is already known); when full it keeps the
//! most salient and drops the weakest. [`Memory::recall_nearest`] answers "where
//! is the closest thing of this kind I remember?", and [`Memory::decay`] fades
//! everything over time, forgetting whatever falls below [`SALIENCE_FLOOR`].
//!
//! All integer/`f32` arithmetic, no clock, no randomness, no heap surprises:
//! the same sequence of calls always yields the same memory and the same
//! recalls on every peer. Self-contained for standalone compilation.

/// Maximum number of landmarks retained.
pub const CAPACITY: usize = 8;
/// Salience at or below which a landmark is forgotten.
pub const SALIENCE_FLOOR: f32 = 0.05;
/// Salience lost per second of [`Memory::decay`].
pub const DECAY_PER_SEC: f32 = 0.1;

/// One remembered place.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Landmark {
    x: i32,
    z: i32,
    kind: u8,
    salience: f32,
}

/// A dragon's bounded, decaying spatial memory.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Memory {
    marks: Vec<Landmark>,
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

impl Memory {
    /// An empty memory.
    pub fn new() -> Self {
        Memory { marks: Vec::new() }
    }

    /// How many landmarks are currently held.
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// Whether the memory holds nothing.
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Index of the least-salient landmark, if any (lowest index breaks ties).
    fn weakest(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut i = 0;
        while i < self.marks.len() {
            match best {
                Some(b) if self.marks[b].salience <= self.marks[i].salience => {}
                _ => best = Some(i),
            }
            i += 1;
        }
        best
    }

    /// Record (or reinforce) a landmark. Salience is clamped to `[0,1]`.
    ///
    /// If a landmark at the same `(x, z, kind)` exists, its salience is raised
    /// to the stronger of the two. Otherwise it is inserted; if that would
    /// exceed [`CAPACITY`], the weakest landmark is dropped (and a brand-new one
    /// weaker than everything already held is simply not kept).
    pub fn observe(&mut self, x: i32, z: i32, kind: u8, salience: f32) {
        let s = clamp01(salience);

        // Reinforce an existing memory of the same place+kind.
        let mut i = 0;
        while i < self.marks.len() {
            let m = self.marks[i];
            if m.x == x && m.z == z && m.kind == kind {
                if s > m.salience {
                    self.marks[i].salience = s;
                }
                return;
            }
            i += 1;
        }

        if self.marks.len() < CAPACITY {
            self.marks.push(Landmark {
                x,
                z,
                kind,
                salience: s,
            });
            return;
        }

        // Full: replace the weakest, but only if the newcomer beats it.
        if let Some(w) = self.weakest() {
            if s > self.marks[w].salience {
                self.marks[w] = Landmark {
                    x,
                    z,
                    kind,
                    salience: s,
                };
            }
        }
    }

    /// Nearest remembered landmark of `kind` to `(x, z)`, by squared distance.
    /// Lowest index breaks ties. `None` if none of that kind is remembered.
    pub fn recall_nearest(&self, x: i32, z: i32, kind: u8) -> Option<(i32, i32)> {
        let mut best: Option<(i64, i32, i32)> = None;
        let mut i = 0;
        while i < self.marks.len() {
            let m = self.marks[i];
            if m.kind == kind {
                let dx = (m.x - x) as i64;
                let dz = (m.z - z) as i64;
                let d2 = dx * dx + dz * dz;
                match best {
                    Some((bd, _, _)) if bd <= d2 => {}
                    _ => best = Some((d2, m.x, m.z)),
                }
            }
            i += 1;
        }
        match best {
            Some((_, bx, bz)) => Some((bx, bz)),
            None => None,
        }
    }

    /// Fade all salience by `dt` seconds and forget anything that drops to or
    /// below [`SALIENCE_FLOOR`]. Negative `dt` is treated as zero.
    pub fn decay(&mut self, dt: f32) {
        let dt = if dt > 0.0 { dt } else { 0.0 };
        let drop = DECAY_PER_SEC * dt;
        let mut kept: Vec<Landmark> = Vec::new();
        let mut i = 0;
        while i < self.marks.len() {
            let mut m = self.marks[i];
            m.salience -= drop;
            if m.salience > SALIENCE_FLOOR {
                kept.push(m);
            }
            i += 1;
        }
        self.marks = kept;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn observe_then_recall_nearest_of_kind() {
        let mut m = Memory::new();
        m.observe(10, 0, 1, 0.9);
        m.observe(3, 0, 1, 0.5);
        m.observe(100, 0, 1, 0.7);
        // nearest kind-1 landmark to the origin is the one at x=3
        assert_eq!(m.recall_nearest(0, 0, 1), Some((3, 0)));
    }

    #[test]
    fn kind_filter_works() {
        let mut m = Memory::new();
        m.observe(5, 5, 2, 0.8); // kind 2
        m.observe(1, 1, 7, 0.8); // kind 7
        // asking for kind 7 ignores the closer kind-2 mark
        assert_eq!(m.recall_nearest(0, 0, 7), Some((1, 1)));
        // a kind nobody remembers → None
        assert_eq!(m.recall_nearest(0, 0, 9), None);
    }

    #[test]
    fn decay_lowers_salience_and_drops_below_floor() {
        let mut m = Memory::new();
        m.observe(0, 0, 1, 0.2);
        m.observe(50, 0, 1, 0.9);
        // a big decay wipes the weak mark but keeps the strong one
        m.decay(2.0); // -0.2 each: 0.0 (dropped) and 0.7 (kept)
        assert_eq!(m.recall_nearest(0, 0, 1), Some((50, 0)));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn capacity_keeps_the_most_salient() {
        let mut m = Memory::new();
        // observe CAPACITY+4 distinct places with ascending salience
        let n = CAPACITY + 4;
        let mut i = 0;
        while i < n {
            // salience from 0.10 upward so all clear the floor
            let s = 0.10 + (i as f32) * 0.05;
            m.observe(i as i32, 0, 1, s);
            i += 1;
        }
        assert_eq!(m.len(), CAPACITY);
        // the survivors must be the top-CAPACITY salience values, i.e. the last
        // CAPACITY indices observed (which had the highest salience)
        let mut j = n - CAPACITY;
        while j < n {
            assert_eq!(m.recall_nearest(j as i32, 0, 1), Some((j as i32, 0)));
            j += 1;
        }
        // the weakest early ones are gone
        assert_ne!(m.recall_nearest(0, 0, 1), Some((0, 0)));
    }

    #[test]
    fn observing_same_place_reinforces_not_duplicates() {
        let mut m = Memory::new();
        m.observe(7, 7, 3, 0.3);
        m.observe(7, 7, 3, 0.6);
        assert_eq!(m.len(), 1);
        // after reinforcement to 0.6, a 0.2 decay leaves 0.4 (still present)
        m.decay(2.0);
        assert_eq!(m.recall_nearest(0, 0, 3), Some((7, 7)));
    }

    #[test]
    fn deterministic() {
        // replay the same random call sequence into two memories
        fn run(seed_init: u32) -> Memory {
            let mut seed = seed_init;
            let mut m = Memory::new();
            let mut step = 0;
            while step < 200 {
                let op = lcg(&mut seed) % 3;
                if op == 0 {
                    let x = (lcg(&mut seed) % 40) as i32 - 20;
                    let z = (lcg(&mut seed) % 40) as i32 - 20;
                    let kind = (lcg(&mut seed) % 3) as u8;
                    let s = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
                    m.observe(x, z, kind, s);
                } else if op == 1 {
                    m.decay((lcg(&mut seed) % 100) as f32 / 100.0);
                } else {
                    let _ = m.recall_nearest(0, 0, 1);
                }
                step += 1;
            }
            m
        }
        assert_eq!(run(0xC0DE), run(0xC0DE));
    }
}
