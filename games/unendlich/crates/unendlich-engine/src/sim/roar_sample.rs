//! Roar genomic sampling — a dragon's roar derived from its genome.
//!
//! Body size sets the fundamental (a bigger dragon roars lower, longer, louder);
//! the jaw/teeth genes set how much growl/rasp rides on top; the dominant
//! element flavours the timbre. Deterministic, so the same dragon always sounds
//! like itself.

use crate::sim::dragon_archetype::{Genome, NUM_ELEMENTS};

/// Acoustic parameters for one roar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Roar {
    /// Fundamental pitch in Hz (big dragons are low).
    pub pitch_hz: f32,
    /// Length of the roar in seconds.
    pub duration_s: f32,
    /// Growl / rasp amount `0..1` (harmonic roughness).
    pub growl: f32,
    /// Loudness `0..1`.
    pub volume: f32,
    /// Dominant element index (`0=Wood..5=Storm`) — drives timbre/FX selection.
    pub element: u8,
}

#[inline]
fn lerp(t: f32, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * t.clamp(0.0, 1.0)
}

/// Sample the roar for a genome.
pub fn roar(g: &Genome) -> Roar {
    // Bulk dominates pitch and loudness; a long neck/throat lowers and lengthens.
    let size = (g.bulk * 0.75 + g.neck_len * 0.25).clamp(0.0, 1.0);
    let pitch_hz = lerp(size, 420.0, 55.0); // small → high, huge → subsonic-ish
    let duration_s = lerp(g.bulk * 0.6 + g.neck_len * 0.4, 0.6, 2.6);
    let growl = (g.jaw_depth * 0.6 + g.teeth * 0.4).clamp(0.0, 1.0);
    let volume = lerp(g.bulk, 0.45, 1.0);

    // Dominant element.
    let mut element = 0u8;
    let mut best = f32::MIN;
    for (i, &w) in g.element.iter().enumerate().take(NUM_ELEMENTS) {
        if w > best {
            best = w;
            element = i as u8;
        }
    }

    Roar { pitch_hz, duration_s, growl, volume, element }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_bulk(b: f32) -> Genome {
        let mut g = Genome::from_seed(7);
        g.bulk = b;
        g.neck_len = 0.5;
        g
    }

    #[test]
    fn bigger_dragons_roar_lower_louder_longer() {
        let small = roar(&with_bulk(0.05));
        let big = roar(&with_bulk(0.95));
        assert!(big.pitch_hz < small.pitch_hz, "bigger → lower pitch");
        assert!(big.volume > small.volume, "bigger → louder");
        assert!(big.duration_s > small.duration_s, "bigger → longer");
    }

    #[test]
    fn growl_tracks_jaw_and_teeth() {
        let mut tame = Genome::from_seed(1);
        tame.jaw_depth = 0.0;
        tame.teeth = 0.0;
        let mut fierce = tame;
        fierce.jaw_depth = 1.0;
        fierce.teeth = 1.0;
        assert!(roar(&fierce).growl > roar(&tame).growl);
    }

    #[test]
    fn element_is_the_dominant_channel() {
        let mut g = Genome::from_seed(2);
        g.element = [0.1, 0.1, 0.1, 0.1, 0.9, 0.2]; // Fire dominant (index 4)
        assert_eq!(roar(&g).element, 4);
    }

    #[test]
    fn pitch_and_volume_in_range() {
        for s in 0..100u64 {
            let r = roar(&Genome::from_seed(s.wrapping_mul(2_654_435_761)));
            assert!(r.pitch_hz >= 55.0 && r.pitch_hz <= 420.0);
            assert!((0.0..=1.0).contains(&r.volume));
            assert!((0.0..=1.0).contains(&r.growl));
            assert!(r.duration_s > 0.0);
        }
    }

    #[test]
    fn deterministic() {
        let g = Genome::from_seed(99);
        assert_eq!(roar(&g), roar(&g));
    }
}
