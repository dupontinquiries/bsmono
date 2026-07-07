//! Sound localization — turning *what a creature hears* into a *guess at where
//! the sound came from*, with error.
//!
//! The old behaviour read the emitter's true position straight off, so animals
//! were effectively telepathic. Real ears give a fuzzy fix: you orient toward
//! the loudest thing you hear, but the quieter and farther it is, the worse you
//! place it — and through walls a sound seems to come from the wrong spot
//! entirely. [`localize`] takes the cues a listener perceives (each a true
//! source position paired with its already-attenuated [perceived
//! loudness](crate::sim::sound::perceived_loudness)), picks the dominant one, and
//! returns an *estimate* offset from the truth by an error that shrinks with
//! loudness. Feeding that estimate (instead of the real position) into the brain
//! makes a creature investigate the wrong bush, lose a trail, or be drawn toward
//! the louder of two noises.
//!
//! Pure and deterministic (sqrt only, no trig).

/// A listener's guess at a sound's origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeardEstimate {
    /// The estimated source position (the truth, displaced by the hearing error).
    pub pos: [f32; 3],
    /// Perceived loudness of the dominant source that produced this estimate.
    pub loudness: i32,
    /// How sure the listener is, `[0,1]` — high when the source was loud/close.
    pub confidence: f32,
}

/// Loudness scale for positional error: error fraction = REF / (loud + REF), so
/// a loud source localizes tightly and a faint one vaguely.
const JITTER_REF: f32 = 50.0;

fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn unit(seed: u64) -> f32 {
    (mix(seed) >> 40) as f32 / (1u64 << 24) as f32
}

/// A deterministic (no-trig) jitter direction: mostly horizontal, a little
/// vertical, unit length.
fn jitter_dir(seed: u64) -> [f32; 3] {
    let x = unit(seed) * 2.0 - 1.0;
    let z = unit(seed ^ 0xABCD) * 2.0 - 1.0;
    let y = (unit(seed ^ 0x1234) * 2.0 - 1.0) * 0.3;
    let len = (x * x + y * y + z * z).sqrt().max(1e-6);
    [x / len, y / len, z / len]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Localize from perceived cues: each `cues[i]` is `(true_source_pos,
/// perceived_loudness)`. Cues below `threshold` are ignored; if none clear it,
/// the listener has no fix (`None`). Otherwise the loudest cue dominates and is
/// returned displaced by an error proportional to distance and inversely to
/// loudness, jittered deterministically by `jitter_seed`.
pub fn localize(
    listener: [f32; 3],
    cues: &[([f32; 3], i32)],
    threshold: i32,
    jitter_seed: u64,
) -> Option<HeardEstimate> {
    let (true_pos, loud) = cues
        .iter()
        .copied()
        .filter(|&(_, l)| l >= threshold)
        .max_by_key(|&(_, l)| l)?;

    let d = dist(listener, true_pos);
    let loud_f = (loud.max(1)) as f32;
    let error_frac = (JITTER_REF / (loud_f + JITTER_REF)).clamp(0.05, 0.8);
    let err = error_frac * d;
    let j = jitter_dir(jitter_seed ^ (loud as u64) ^ true_pos[0].to_bits() as u64);
    Some(HeardEstimate {
        pos: [true_pos[0] + j[0] * err, true_pos[1] + j[1] * err, true_pos[2] + j[2] * err],
        loudness: loud,
        confidence: 1.0 - error_frac,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_audible_gives_no_fix() {
        let cues = [([10.0, 0.0, 0.0], 4), ([5.0, 0.0, 0.0], 9)];
        assert!(localize([0.0; 3], &cues, 20, 1).is_none(), "all cues below threshold");
    }

    #[test]
    fn a_loud_close_source_is_placed_tightly_a_faint_far_one_vaguely() {
        let listener = [0.0; 3];
        // Loud + close.
        let near = [6.0, 0.0, 0.0];
        let est_near = localize(listener, &[(near, 130)], 10, 7).unwrap();
        let err_near = dist(est_near.pos, near);
        // Faint + far.
        let far = [40.0, 0.0, 0.0];
        let est_far = localize(listener, &[(far, 22)], 10, 7).unwrap();
        let err_far = dist(est_far.pos, far);
        assert!(err_near < err_far, "loud/close localizes tighter ({err_near}) than faint/far ({err_far})");
        assert!(err_near > 0.0, "even a clear fix carries some error");
        assert!(est_near.confidence > est_far.confidence, "and more confidence");
    }

    #[test]
    fn the_louder_of_two_sources_dominates_the_estimate() {
        let listener = [0.0; 3];
        let loud_here = [8.0, 0.0, 0.0];
        let quiet_there = [0.0, 0.0, 40.0];
        let est = localize(listener, &[(loud_here, 120), (quiet_there, 30)], 10, 3).unwrap();
        assert_eq!(est.loudness, 120);
        assert!(dist(est.pos, loud_here) < dist(est.pos, quiet_there), "estimate sits near the louder source");
    }

    #[test]
    fn localization_is_deterministic() {
        let listener = [0.0; 3];
        let cues = [([12.0, 1.0, -3.0], 80)];
        assert_eq!(localize(listener, &cues, 10, 99), localize(listener, &cues, 10, 99));
    }
}
