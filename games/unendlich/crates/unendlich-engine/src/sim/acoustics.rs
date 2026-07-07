//! `sim::acoustics` — how a creature HEARS: classify a sound by its acoustic
//! fingerprint, attenuate hearing by the local ambient noise floor (a creature at
//! a waterfall is half-deaf), and read echo + volume as distance/space cues.
//!
//! Adapted from the voice-fingerprinting approach (`voiceterm`'s `speaker_embed`):
//! a sound's identity is a **region in feature space**, not a scalar. A fingerprint
//! is a short L2-normalised vector; a known sound CLASS (predator-roar, conspecific
//! call, waterfall, wind, footstep…) is a stored prototype vector; classification
//! is **nearest prototype by cosine distance, gated by a confidence MARGIN** — if
//! the sound sits between two classes the creature refuses to commit (stays wary)
//! rather than guessing. The prototypes are the "sound DNA database" the design
//! calls for; a fingerprint is the key into it.
//!
//! Hearing ACUITY and the prototype set a creature is born knowing are intended to
//! be GENETIC (heritable hearing range + an innate predator-class prototype),
//! shaped over deep time — see `docs/design/acoustic_perception_and_poi.md`. This
//! module is the pure, deterministic mechanism: vectors + distances + masking math.
//! No clock/RNG/IO, no panic.

/// Fingerprint dimensionality (a handful of pooled spectral stats — e.g. band
/// energies + centroid + zero-crossing + roll-off). Small + fixed so it's cheap
/// to compare and stable to store as a creature's innate "known sound".
pub const FINGERPRINT_DIM: usize = 8;

/// L2-normalise in place; a zero/degenerate vector is left zero so it reads as
/// "no usable signal" (cosine distance 1.0 to everything — neither match nor
/// anti-match), exactly as the voice embedder treats garbage.
pub fn l2_normalize(v: &mut [f32]) {
    let mut n = 0.0f32;
    for &x in v.iter() {
        n += x * x;
    }
    n = n.sqrt();
    if n > 1e-12 && n.is_finite() {
        for x in v.iter_mut() {
            *x /= n;
        }
    } else {
        for x in v.iter_mut() {
            *x = 0.0;
        }
    }
}

/// Cosine distance in `[0,2]` (0 = same direction). Mismatched lengths, empty, or
/// a zero/non-finite vector → 1.0 ("no information"). NaN-safe (a corrupt stored
/// prototype can't poison a creature's hearing into a false match).
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na < 1e-12 || nb < 1e-12 || !na.is_finite() || !nb.is_finite() || !dot.is_finite() {
        return 1.0;
    }
    let cos = dot / (na.sqrt() * nb.sqrt());
    if !cos.is_finite() {
        return 1.0;
    }
    (1.0 - cos).clamp(0.0, 2.0)
}

/// A heard sound after it reaches the creature's ear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeardSound {
    /// Loudness at the ear in `[0,1]` (already distance-attenuated by the caller).
    pub volume: f32,
    /// Echo / reverberation amount in `[0,1]` — high = enclosed space or canyon,
    /// low = open ground. A spatial cue the creature reacts to.
    pub echo: f32,
}

/// One known sound class the creature can recognise: a prototype fingerprint plus
/// an id the caller maps to meaning (predator, kin, water, wind, …). The innate
/// set is intended to be partly genetic.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundClass {
    pub id: u16,
    pub prototype: Vec<f32>, // FINGERPRINT_DIM, L2-normalised
}

/// Result of classifying a sound against the known set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Classified {
    /// Best-matching class id.
    pub id: u16,
    /// Cosine distance to it (smaller = better).
    pub distance: f32,
    /// Confidence margin = (2nd-nearest distance − nearest distance). Large =
    /// confident; small = ambiguous (the sound sits between two classes).
    pub margin: f32,
}

/// Classify `query` against `classes` by nearest prototype + ambiguity margin.
/// Returns `None` for an empty set. The CALLER decides whether `margin` clears a
/// (possibly genetic) confidence threshold; below it the creature should treat the
/// sound as unidentified and stay wary rather than commit to a reaction.
pub fn classify(query: &[f32], classes: &[SoundClass]) -> Option<Classified> {
    if classes.is_empty() {
        return None;
    }
    let mut best = (f32::INFINITY, 0u16);
    let mut second = f32::INFINITY;
    for c in classes {
        let d = cosine_distance(query, &c.prototype);
        if d < best.0 {
            second = best.0;
            best = (d, c.id);
        } else if d < second {
            second = d;
        }
    }
    let margin = if second.is_finite() {
        (second - best.0).max(0.0)
    } else {
        // only one class — margin is how far the query is from "no information"
        (1.0 - best.0).max(0.0)
    };
    Some(Classified {
        id: best.1,
        distance: best.0,
        margin,
    })
}

/// Effective hearing after environmental MASKING. A creature's usable hearing is
/// its acuity scaled down by how much the local ambient noise floods the channel:
/// near a waterfall (`ambient` high) even a loud sound is hard to pick out. Models
/// signal-to-noise: a sound is heard to the degree its volume rises above the
/// ambient floor, scaled by the creature's acuity.
///
/// - `volume`  — the sound's loudness at the ear `[0,1]`.
/// - `ambient` — local ambient noise floor `[0,1]` (waterfall, wind, surf).
/// - `acuity`  — the creature's hearing acuity `[0,1]` (intended genetic).
///
/// Returns audibility in `[0,1]`: 0 = lost in the noise / deaf, 1 = crystal clear.
pub fn effective_hearing(volume: f32, ambient: f32, acuity: f32) -> f32 {
    let v = volume.clamp(0.0, 1.0);
    let a = ambient.clamp(0.0, 1.0);
    let k = acuity.clamp(0.0, 1.0);
    // headroom above the noise floor, then scale by acuity. A sound at or below the
    // ambient level is inaudible regardless of acuity.
    let headroom = (v - a).max(0.0) / (1.0 - a).max(1e-3);
    (headroom * k).clamp(0.0, 1.0)
}

/// A coarse distance/space impression from echo + volume — the cue a creature uses
/// to react differently to a near open-ground sound vs a far echoing one. Returns
/// `(perceived_distance, enclosure)` both in `[0,1]`:
/// - perceived_distance: louder ⇒ nearer; modulated up by echo (reverb reads as
///   "far / large space").
/// - enclosure: just the echo amount — high ⇒ canyon/cave, low ⇒ open.
///
/// Reaction policy lives with the caller (e.g. a high-echo predator sound from an
/// enclosed space might mean "ambush ahead — avoid", vs a near dry one "right
/// here — flee now"); this only turns the physical cues into perceptual scalars.
pub fn spatial_impression(s: HeardSound) -> (f32, f32) {
    let v = s.volume.clamp(0.0, 1.0);
    let e = s.echo.clamp(0.0, 1.0);
    // nearer when louder; echo pushes the impression outward (bigger space / farther)
    let near = (1.0 - v) * 0.7 + e * 0.3;
    (near.clamp(0.0, 1.0), e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(mut v: Vec<f32>) -> Vec<f32> {
        l2_normalize(&mut v);
        v
    }

    #[test]
    fn cosine_distance_basics() {
        let a = norm(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let b = norm(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(cosine_distance(&a, &b) < 1e-6, "identical → ~0");
        let c = norm(vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((cosine_distance(&a, &c) - 1.0).abs() < 1e-6, "orthogonal → 1");
    }

    #[test]
    fn zero_vector_is_no_information() {
        let z = vec![0.0; FINGERPRINT_DIM];
        let a = norm(vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(cosine_distance(&z, &a), 1.0);
    }

    #[test]
    fn nan_is_safe() {
        let bad = vec![f32::NAN; FINGERPRINT_DIM];
        let a = norm(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(cosine_distance(&bad, &a), 1.0, "corrupt fingerprint can't false-match");
    }

    #[test]
    fn classify_picks_nearest_with_margin() {
        let predator = SoundClass { id: 1, prototype: norm(vec![1.0, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]) };
        let water = SoundClass { id: 2, prototype: norm(vec![0.0, 0.0, 1.0, 0.1, 0.0, 0.0, 0.0, 0.0]) };
        let classes = vec![predator, water];
        // a roar-like query close to predator
        let q = norm(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let c = classify(&q, &classes).unwrap();
        assert_eq!(c.id, 1, "nearest is predator");
        assert!(c.margin > 0.0, "confident: predator clearly closer than water");
    }

    #[test]
    fn ambiguous_sound_has_small_margin() {
        // two classes equidistant from the query → tiny margin → caller stays wary.
        let a = SoundClass { id: 1, prototype: norm(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]) };
        let b = SoundClass { id: 2, prototype: norm(vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]) };
        let q = norm(vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]); // 45° between
        let c = classify(&q, &[a, b]).unwrap();
        assert!(c.margin < 0.05, "between two classes → ambiguous, got margin {}", c.margin);
    }

    #[test]
    fn classify_empty_is_none() {
        assert_eq!(classify(&[1.0; FINGERPRINT_DIM], &[]), None);
    }

    #[test]
    fn waterfall_floods_hearing() {
        // a moderate sound (0.5) is clear in quiet, nearly lost at a waterfall.
        let quiet = effective_hearing(0.5, 0.05, 1.0);
        let waterfall = effective_hearing(0.5, 0.6, 1.0);
        assert!(quiet > 0.4, "clear in quiet, got {quiet}");
        assert!(waterfall < quiet, "waterfall masks: {waterfall} < {quiet}");
        // a sound at/below the ambient floor is inaudible regardless of acuity
        assert_eq!(effective_hearing(0.4, 0.4, 1.0), 0.0);
    }

    #[test]
    fn acuity_scales_hearing() {
        let sharp = effective_hearing(0.5, 0.1, 1.0);
        let dull = effective_hearing(0.5, 0.1, 0.3);
        assert!(sharp > dull, "sharper ears hear more: {sharp} > {dull}");
    }

    #[test]
    fn echo_reads_as_distance_and_enclosure() {
        let near_dry = spatial_impression(HeardSound { volume: 0.9, echo: 0.05 });
        let far_echo = spatial_impression(HeardSound { volume: 0.2, echo: 0.8 });
        assert!(near_dry.0 < far_echo.0, "loud+dry reads nearer than quiet+echoey");
        assert!(far_echo.1 > near_dry.1, "echo reads as enclosure");
    }
}
