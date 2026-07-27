//! styles.rs — WORK STYLES for the coordinator's worker threads: each parallel worker gets not just
//! a different directive TEXT but a different way of WORKING — sampling included. Where
//! `directives.rs` (egui side) varies what a branch is told, a style varies how its decoder behaves:
//! temperature and min-p move together with the instruction, so a "careful" thread really is
//! low-temperature and a "wild" one really does explore.
//!
//! Deterministic in the seed, spread so no two workers of one unit share a style until the palette
//! is exhausted. Used by `coordinator::plan()` for the Aggressive mode's best-of-K (K distinct
//! styles instead of K identical clones) and by the chat fan-out to vary threads.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub name: &'static str,
    /// Appended to the worker's system prompt — the behavioural half.
    pub system: &'static str,
    /// Added to the session temperature (clamped by the caller to a sane range).
    pub temp_delta: f32,
    /// Overrides min_p when Some — the sampling half.
    pub min_p: Option<f32>,
}

pub const PALETTE: &[Style] = &[
    Style { name: "careful",   system: "Work carefully and conventionally; prefer the obviously correct construction.",
            temp_delta: -0.25, min_p: Some(0.10) },
    Style { name: "canonical", system: "Produce the textbook solution a maintainer would expect to find.",
            temp_delta: -0.10, min_p: Some(0.05) },
    Style { name: "balanced",  system: "Balance clarity and brevity; no heroics.",
            temp_delta: 0.0,   min_p: None },
    Style { name: "explorer",  system: "Take the less obvious angle; if a cleverer structure exists, find it.",
            temp_delta: 0.20,  min_p: Some(0.02) },
    Style { name: "wild",      system: "Be inventive. Unusual decompositions and representations are welcome if they work.",
            temp_delta: 0.40,  min_p: Some(0.01) },
    Style { name: "terse",     system: "Say and write the minimum that fully solves it.",
            temp_delta: -0.15, min_p: Some(0.08) },
];

/// K distinct styles, deterministically rotated by `seed`; cycles only past the palette size.
pub fn assign(k: usize, seed: u64) -> Vec<Style> {
    (0..k).map(|i| PALETTE[(i + seed as usize) % PALETTE.len()]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assignment_is_distinct_until_the_palette_cycles() {
        let s = assign(PALETTE.len(), 3);
        let names: std::collections::BTreeSet<_> = s.iter().map(|s| s.name).collect();
        assert_eq!(names.len(), PALETTE.len());
    }
    #[test]
    fn deterministic_in_seed() {
        assert_eq!(assign(4, 7), assign(4, 7));
        assert_ne!(assign(4, 7)[0].name, assign(4, 8)[0].name);
    }
    #[test]
    fn styles_really_differ_in_sampling_not_just_words() {
        let s = assign(5, 0);
        let temps: std::collections::BTreeSet<i32> = s.iter().map(|x| (x.temp_delta * 100.0) as i32).collect();
        assert!(temps.len() >= 4, "sampling must vary across threads: {temps:?}");
    }
}
