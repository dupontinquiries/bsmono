//! mic_topology: classify a microphone setup from pairwise acoustic
//! stats into a pipeline topology — phase array (e.g. phone top+bottom
//! → phase cancellation) vs independent room mics (e.g. podcast in
//! separate rooms) vs a mix — and report a deviation metric so the
//! dynamic FSM (F8 stage 3) knows when to rebuild. (See
//! docs/design/multi_mic_pipeline.md, stage 1.)
//!
//! PURE: the inputs (per-pair correlation + inter-mic delay) are
//! computed elsewhere from the raw streams; this module only decides
//! the SHAPE. No audio, no DSP here.
//!
//! Core rule: two mics are PHASE-COUPLED if they hear the same sound
//! with high correlation and a small time-of-arrival difference (close
//! together — a phase array). Low correlation / large delay = they're
//! in different acoustic spaces (room mics). Mics are grouped by
//! transitive coupling (union-find); the topology is read off the
//! resulting groups.

/// Pairwise acoustic relationship between two mics (computed upstream).
#[derive(Clone, Copy, Debug)]
pub struct PairStat {
    pub a: usize,
    pub b: usize,
    /// Normalized cross-correlation of the two streams, 0..1.
    pub correlation: f32,
    /// Estimated inter-mic delay (time-of-arrival difference), ms.
    pub delay_ms: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct TopologyConfig {
    /// Correlation at/above which a pair may be coupled.
    pub corr_high: f32,
    /// Delay at/below which a coupled pair is "close" (a phase array
    /// rather than the same sound reaching distant mics).
    pub delay_low_ms: f32,
    /// Fraction of pairs that must be coupled to call the whole set a
    /// pure PhaseArray (vs Mixed).
    pub array_fraction: f32,
    /// Fraction at/below which the set is pure RoomMics.
    pub room_fraction: f32,
}

impl Default for TopologyConfig {
    fn default() -> Self {
        // Starting points — tune on device (design open question 4).
        Self { corr_high: 0.8, delay_low_ms: 1.0, array_fraction: 0.8, room_fraction: 0.2 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Topology {
    /// One mic — nothing to combine.
    Single,
    /// All mics tightly coupled — one phase array (phase-cancel/beamform).
    PhaseArray { group: Vec<usize> },
    /// Mics are acoustically independent — treat each as a room mic.
    RoomMics { mics: Vec<usize> },
    /// Some mics cluster into phase array(s), others are independent.
    /// `arrays` = coupled groups (size >= 2); `rooms` = lone mics.
    Mixed { arrays: Vec<Vec<usize>>, rooms: Vec<usize> },
    /// No mics.
    None,
}

/// Simple union-find over mic indices.
struct UnionFind { parent: Vec<usize> }
impl UnionFind {
    fn new(n: usize) -> Self { Self { parent: (0..n).collect() } }
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.parent[r] != r { r = self.parent[r]; }
        // path compression
        let mut c = x;
        while self.parent[c] != r { let n = self.parent[c]; self.parent[c] = r; c = n; }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb { self.parent[ra] = rb; }
    }
}

fn is_coupled(p: &PairStat, cfg: &TopologyConfig) -> bool {
    p.correlation >= cfg.corr_high && p.delay_ms <= cfg.delay_low_ms
}

/// Classify the mic set into a topology. Pure.
pub fn classify(n_mics: usize, pairs: &[PairStat], cfg: &TopologyConfig) -> Topology {
    if n_mics == 0 { return Topology::None; }
    if n_mics == 1 { return Topology::Single; }

    // Group mics by transitive coupling.
    let mut uf = UnionFind::new(n_mics);
    let mut coupled_pairs = 0usize;
    for p in pairs {
        if p.a < n_mics && p.b < n_mics && is_coupled(p, cfg) {
            uf.union(p.a, p.b);
            coupled_pairs += 1;
        }
    }

    // Collect groups.
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for m in 0..n_mics {
        let r = uf.find(m);
        groups.entry(r).or_default().push(m);
    }
    let mut arrays: Vec<Vec<usize>> = Vec::new();
    let mut rooms: Vec<usize> = Vec::new();
    for (_, mut members) in groups {
        members.sort_unstable();
        if members.len() >= 2 { arrays.push(members); }
        else { rooms.extend(members); }
    }
    arrays.sort();
    rooms.sort_unstable();

    // Read off the topology.
    let total_pairs = pairs.len().max(1);
    let coupled_frac = coupled_pairs as f32 / total_pairs as f32;

    if arrays.len() == 1 && rooms.is_empty() {
        // every mic in one coupled group
        Topology::PhaseArray { group: arrays.into_iter().next().unwrap() }
    } else if arrays.is_empty() {
        // no group of size >= 2 — all independent
        Topology::RoomMics { mics: rooms }
    } else if coupled_frac >= cfg.array_fraction && rooms.is_empty() {
        // (defensive) multiple arrays but no lone mics, mostly coupled
        Topology::Mixed { arrays, rooms }
    } else if coupled_frac <= cfg.room_fraction && arrays.is_empty() {
        Topology::RoomMics { mics: rooms }
    } else {
        Topology::Mixed { arrays, rooms }
    }
}

/// Deviation metric between a previously-classified topology and fresh
/// pair stats: the fraction of pairs whose coupled/uncoupled state
/// DIFFERS from what the current grouping implies. 0 = no change, 1 =
/// every pair flipped. The FSM rebuilds when this exceeds a threshold
/// (so a stable setup isn't reshaped every tick, but a real change —
/// someone moves rooms, a mic is unplugged — triggers a rebuild).
pub fn deviation(prev: &Topology, fresh_pairs: &[PairStat], cfg: &TopologyConfig) -> f32 {
    if fresh_pairs.is_empty() { return 0.0; }
    // Which mics does `prev` consider in the same array group?
    let same_group = |a: usize, b: usize| -> bool {
        match prev {
            Topology::PhaseArray { group } => group.contains(&a) && group.contains(&b),
            Topology::Mixed { arrays, .. } =>
                arrays.iter().any(|g| g.contains(&a) && g.contains(&b)),
            _ => false,
        }
    };
    let mut diffs = 0usize;
    for p in fresh_pairs {
        let now_coupled = is_coupled(p, cfg);
        let was_grouped = same_group(p.a, p.b);
        if now_coupled != was_grouped { diffs += 1; }
    }
    diffs as f32 / fresh_pairs.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TopologyConfig { TopologyConfig::default() }
    fn pair(a: usize, b: usize, c: f32, d: f32) -> PairStat {
        PairStat { a, b, correlation: c, delay_ms: d }
    }

    #[test]
    fn none_and_single() {
        assert_eq!(classify(0, &[], &cfg()), Topology::None);
        assert_eq!(classify(1, &[], &cfg()), Topology::Single);
    }

    #[test]
    fn phone_pair_is_phase_array() {
        let t = classify(2, &[pair(0, 1, 0.95, 0.3)], &cfg());
        assert_eq!(t, Topology::PhaseArray { group: vec![0, 1] });
    }

    #[test]
    fn podcast_rooms_are_room_mics() {
        let pairs = [pair(0,1,0.2,8.0), pair(0,2,0.15,12.0), pair(1,2,0.25,9.0)];
        let t = classify(3, &pairs, &cfg());
        assert_eq!(t, Topology::RoomMics { mics: vec![0, 1, 2] });
    }

    #[test]
    fn phone_plus_room_is_mixed() {
        // mics 0,1 are a coupled phone pair; mic 2 is a distant room mic
        let pairs = [pair(0,1,0.95,0.2), pair(0,2,0.1,15.0), pair(1,2,0.12,14.0)];
        let t = classify(3, &pairs, &cfg());
        match t {
            Topology::Mixed { arrays, rooms } => {
                assert_eq!(arrays, vec![vec![0, 1]]);
                assert_eq!(rooms, vec![2]);
            }
            other => panic!("expected Mixed, got {:?}", other),
        }
    }

    #[test]
    fn four_mic_phased_array() {
        let pairs = [
            pair(0,1,0.9,0.4), pair(0,2,0.88,0.6), pair(0,3,0.91,0.5),
            pair(1,2,0.9,0.3), pair(1,3,0.87,0.4), pair(2,3,0.92,0.2),
        ];
        let t = classify(4, &pairs, &cfg());
        assert_eq!(t, Topology::PhaseArray { group: vec![0, 1, 2, 3] });
    }

    #[test]
    fn transitive_coupling_groups_chain() {
        // 0-1 coupled, 1-2 coupled, but 0-2 not directly measured high;
        // union-find should still group {0,1,2}.
        let pairs = [pair(0,1,0.9,0.3), pair(1,2,0.9,0.4), pair(0,2,0.5,3.0)];
        let t = classify(3, &pairs, &cfg());
        assert_eq!(t, Topology::PhaseArray { group: vec![0, 1, 2] });
    }

    #[test]
    fn high_corr_but_large_delay_not_coupled() {
        // same sound reaching two DISTANT mics: high corr, big delay →
        // NOT a phase array (they're room mics hearing the same source).
        let t = classify(2, &[pair(0, 1, 0.95, 9.0)], &cfg());
        assert_eq!(t, Topology::RoomMics { mics: vec![0, 1] });
    }

    #[test]
    fn deviation_zero_when_stable() {
        let prev = Topology::PhaseArray { group: vec![0, 1] };
        // fresh stats still show the pair coupled → no deviation
        let d = deviation(&prev, &[pair(0, 1, 0.94, 0.3)], &cfg());
        assert_eq!(d, 0.0);
    }

    #[test]
    fn deviation_high_when_decoupled() {
        // was a phase array; now the pair reads uncoupled (mic moved
        // away) → deviation 1.0 (the FSM should rebuild).
        let prev = Topology::PhaseArray { group: vec![0, 1] };
        let d = deviation(&prev, &[pair(0, 1, 0.2, 10.0)], &cfg());
        assert_eq!(d, 1.0);
    }

    #[test]
    fn deviation_partial() {
        // prev grouped {0,1}; two fresh pairs, one still coupled, one
        // newly coupled (not in prev group) → 1 of 2 differ = 0.5.
        let prev = Topology::PhaseArray { group: vec![0, 1] };
        let pairs = [pair(0, 1, 0.95, 0.3), pair(1, 2, 0.95, 0.3)];
        let d = deviation(&prev, &pairs, &cfg());
        assert_eq!(d, 0.5);
    }
}
