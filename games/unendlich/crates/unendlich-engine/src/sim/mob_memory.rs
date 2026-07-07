//! mob_memory — a creature's long-term memory of places. A bounded store of
//! remembered landmarks/events `{landmark_id, pos, salience, last_seen_tick}` that
//! fades slowly over a long horizon and can be recalled by proximity — the
//! substrate for "a dragon remembers the peaks and kills it has flown over and
//! steers back toward the salient ones." Distinct from `forage_memory` (food
//! cells, short horizon). Pure + deterministic; bounded (the least-salient memory
//! is forgotten past capacity).

/// One remembered place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Memory {
    pub landmark_id: u32,
    pub pos: [f32; 3],
    /// How strongly it's held, `>= 0`; decays over ticks, refreshed on re-sighting.
    pub salience: f32,
    pub last_seen: u64,
}

/// A capacity-bounded long-term memory.
#[derive(Clone, Debug)]
pub struct MobMemory {
    entries: Vec<Memory>,
    capacity: usize,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

impl MobMemory {
    pub fn new(capacity: usize) -> MobMemory {
        MobMemory { entries: Vec::new(), capacity: capacity.max(1) }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> &[Memory] {
        &self.entries
    }

    /// Index of the least-salient entry (ties → lowest index), if any.
    fn weakest(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, m) in self.entries.iter().enumerate() {
            let take = match best {
                None => true,
                Some(j) => m.salience < self.entries[j].salience,
            };
            if take {
                best = Some(i);
            }
        }
        best
    }

    /// Record (or refresh) a landmark. An existing `landmark_id` has its position
    /// and `last_seen` updated and its salience raised by `salience` (capped). A
    /// new landmark is inserted; if that would exceed capacity, the least-salient
    /// existing memory is evicted first (and the newcomer only displaces it when
    /// it is at least as salient).
    pub fn remember(&mut self, landmark_id: u32, pos: [f32; 3], salience: f32, now: u64) {
        let add = salience.max(0.0);
        for m in self.entries.iter_mut() {
            if m.landmark_id == landmark_id {
                m.pos = pos;
                m.last_seen = now;
                m.salience = (m.salience + add).min(1_000_000.0);
                return;
            }
        }
        let fresh = Memory { landmark_id, pos, salience: add, last_seen: now };
        if self.entries.len() < self.capacity {
            self.entries.push(fresh);
            return;
        }
        // full: evict the weakest, but only if the newcomer is at least as salient
        if let Some(w) = self.weakest() {
            if fresh.salience >= self.entries[w].salience {
                self.entries[w] = fresh;
            }
        }
    }

    /// Long-horizon decay: every memory's salience is multiplied by `(1 - rate)`
    /// once (call per long tick). Monotone non-increasing. `rate` clamped to
    /// `[0, 1]`.
    pub fn decay(&mut self, rate: f32) {
        let keep = (1.0 - rate.clamp(0.0, 1.0)).max(0.0);
        for m in self.entries.iter_mut() {
            m.salience *= keep;
        }
    }

    /// Recall the most-salient remembered landmark within `radius` of `from`
    /// (ties → lowest index). `None` if nothing is in range.
    pub fn recall(&self, from: [f32; 3], radius: f32) -> Option<&Memory> {
        let r2 = radius * radius;
        let mut best: Option<usize> = None;
        for (i, m) in self.entries.iter().enumerate() {
            if dist2(from, m.pos) <= r2 {
                let take = match best {
                    None => true,
                    Some(j) => m.salience > self.entries[j].salience,
                };
                if take {
                    best = Some(i);
                }
            }
        }
        best.map(|i| &self.entries[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn refreshing_a_known_landmark_does_not_grow_the_store() {
        let mut m = MobMemory::new(8);
        m.remember(1, [0.0, 0.0, 0.0], 0.5, 10);
        m.remember(1, [1.0, 0.0, 0.0], 0.5, 20);
        assert_eq!(m.len(), 1, "same id updates in place");
        assert_eq!(m.entries()[0].last_seen, 20);
        assert!(m.entries()[0].salience > 0.5, "re-sighting reinforces");
    }

    #[test]
    fn decay_is_monotone_over_ticks() {
        let mut m = MobMemory::new(4);
        m.remember(1, [0.0; 3], 1.0, 0);
        let mut last = m.entries()[0].salience;
        for _ in 0..50 {
            m.decay(0.1);
            let s = m.entries()[0].salience;
            assert!(s <= last + 1e-9, "salience never rises under decay: {s} > {last}");
            last = s;
        }
        assert!(last < 0.01, "long-horizon decay fades it: {last}");
    }

    #[test]
    fn capacity_keeps_the_most_salient() {
        let mut m = MobMemory::new(3);
        // insert five landmarks with increasing salience
        for i in 0..5u32 {
            m.remember(i, [i as f32, 0.0, 0.0], 0.1 * (i as f32 + 1.0), i as u64);
        }
        assert_eq!(m.len(), 3, "bounded to capacity");
        // the survivors should be the three most salient (ids 2,3,4)
        let mut ids: Vec<u32> = m.entries().iter().map(|e| e.landmark_id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![2, 3, 4], "least-salient were evicted, got {ids:?}");
    }

    #[test]
    fn recall_returns_highest_salience_in_radius() {
        let mut m = MobMemory::new(8);
        m.remember(1, [0.0, 0.0, 0.0], 0.3, 0); // near, weak
        m.remember(2, [2.0, 0.0, 0.0], 0.9, 0); // near, strong
        m.remember(3, [100.0, 0.0, 0.0], 1.0, 0); // strongest but far away
        let got = m.recall([0.0, 0.0, 0.0], 10.0);
        assert!(got.is_some(), "something in range");
        if let Some(mem) = got {
            assert_eq!(mem.landmark_id, 2, "the strongest within radius, not the far one");
        }
        assert!(m.recall([0.0, 0.0, 0.0], 1.0).map(|x| x.landmark_id) == Some(1), "tighter radius → only the near weak one");
    }

    #[test]
    fn prop_invariants_hold_over_random_ops() {
        let mut seed = 0x1357_9BDFu32;
        for _ in 0..2000 {
            let cap = 1 + (lcg(&mut seed) * 8.0) as usize;
            let mut m = MobMemory::new(cap);
            let mut tick = 0u64;
            for _ in 0..40 {
                let id = (lcg(&mut seed) * 12.0) as u32;
                let pos = [lcg(&mut seed) * 200.0 - 100.0, lcg(&mut seed) * 200.0 - 100.0, lcg(&mut seed) * 200.0 - 100.0];
                m.remember(id, pos, lcg(&mut seed), tick);
                if lcg(&mut seed) < 0.3 {
                    m.decay(lcg(&mut seed) * 0.5);
                }
                tick += 1;
                assert!(m.len() <= cap, "never exceeds capacity");
                for e in m.entries() {
                    assert!(e.salience.is_finite() && e.salience >= 0.0, "salience finite & non-negative");
                }
                // recall, if any, must be within radius and finite
                if let Some(r) = m.recall([0.0; 3], 50.0) {
                    assert!(dist2([0.0; 3], r.pos) <= 50.0 * 50.0 + 1e-3, "recall within radius");
                }
            }
        }
    }
}
