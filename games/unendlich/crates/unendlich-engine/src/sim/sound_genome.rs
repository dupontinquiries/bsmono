//! `sim::sound_genome` — a creature's heritable **voice**, and the Mimic
//! horror that learns to wear someone else's.
//!
//! [`voice_from`] turns a six-byte genome into a [`Voice`]: pitch, timbre,
//! roughness, and three ascending formants. The mapping is injective — every
//! byte lands in a distinct part of the voice — so two different genomes always
//! sound different.
//!
//! [`mimic`] is the Mimic's party trick: starting from its own natural voice it
//! interpolates toward a `target` voice by a `skill` in `0..1`. At `skill = 0`
//! it still sounds like itself; at `skill = 1` it is indistinguishable from the
//! target; in between it slides linearly, and because a blend of two ascending
//! formant sets stays ascending, the result is always a well-formed voice.
//! Pure and deterministic: no clock, no randomness.

/// A voice. Frequencies are in hertz; `timbre` and `roughness` are `0..1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voice {
    /// Fundamental pitch, ~`80..400` Hz.
    pub pitch: f32,
    /// Bright/dark colour, `0..1`.
    pub timbre: f32,
    /// Smooth/harsh texture, `0..1`.
    pub roughness: f32,
    /// Three strictly ascending formant frequencies, Hz.
    pub formants: [f32; 3],
}

/// The Mimic's own untrained voice — what it sounds like at `skill = 0`.
const MIMIC_BASE: Voice = Voice {
    pitch: 150.0,
    timbre: 0.40,
    roughness: 0.30,
    formants: [300.0, 600.0, 900.0],
};

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

fn b01(byte: u8) -> f32 {
    byte as f32 / 255.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    // Exact at the endpoints so a fully-skilled mimic matches the target
    // bit-for-bit (plain `a + (b-a)*t` can be a ULP off at t == 1.0).
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        a + (b - a) * t
    }
}

/// Decode a six-byte genome into a voice.
///
/// Each byte drives one voice quantity; the three formant bytes are summed
/// cumulatively with a positive floor between them, guaranteeing the formants
/// strictly ascend while keeping the decode injective in every byte.
pub fn voice_from(genome: [u8; 6]) -> Voice {
    let pitch = 80.0 + b01(genome[0]) * 320.0; // 80..400
    let timbre = b01(genome[1]);
    let roughness = b01(genome[2]);

    // Cumulative build with a fixed gap so formants strictly ascend.
    let f0 = 200.0 + b01(genome[3]) * 300.0;
    let f1 = f0 + 200.0 + b01(genome[4]) * 300.0;
    let f2 = f1 + 200.0 + b01(genome[5]) * 300.0;

    Voice {
        pitch,
        timbre,
        roughness,
        formants: [f0, f1, f2],
    }
}

/// Blend the Mimic's own voice toward `target` by `skill` (`0..1`, clamped).
pub fn mimic(target: &Voice, skill: f32) -> Voice {
    let t = clamp01(skill);
    Voice {
        pitch: lerp(MIMIC_BASE.pitch, target.pitch, t),
        timbre: lerp(MIMIC_BASE.timbre, target.timbre, t),
        roughness: lerp(MIMIC_BASE.roughness, target.roughness, t),
        formants: [
            lerp(MIMIC_BASE.formants[0], target.formants[0], t),
            lerp(MIMIC_BASE.formants[1], target.formants[1], t),
            lerp(MIMIC_BASE.formants[2], target.formants[2], t),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_genome(seed: &mut u32) -> [u8; 6] {
        let mut g = [0u8; 6];
        let mut i = 0;
        while i < 6 {
            g[i] = (lcg(seed) & 0xFF) as u8;
            i += 1;
        }
        g
    }

    fn ascending(v: &Voice) -> bool {
        v.formants[0] < v.formants[1] && v.formants[1] < v.formants[2]
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x5017_u32;
        for _ in 0..2000 {
            let g = rand_genome(&mut seed);
            assert_eq!(voice_from(g), voice_from(g));
            let t = voice_from(rand_genome(&mut seed));
            let s = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(mimic(&t, s), mimic(&t, s));
        }
    }

    #[test]
    fn fields_bounded() {
        let mut seed = 0xB0DD_u32;
        for _ in 0..3000 {
            let v = voice_from(rand_genome(&mut seed));
            assert!(v.pitch >= 80.0 && v.pitch <= 400.0);
            assert!(v.timbre >= 0.0 && v.timbre <= 1.0);
            assert!(v.roughness >= 0.0 && v.roughness <= 1.0);
            // formants live within their cumulative envelope
            assert!(v.formants[0] >= 200.0 && v.formants[2] <= 1500.0);
        }
    }

    #[test]
    fn formants_ascending_from_genome_and_after_mimic() {
        let mut seed = 0xA5C0_u32;
        for _ in 0..3000 {
            let v = voice_from(rand_genome(&mut seed));
            assert!(ascending(&v), "genome voice not ascending: {:?}", v.formants);
            let s = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let m = mimic(&v, s);
            assert!(ascending(&m), "mimic voice not ascending: {:?}", m.formants);
        }
    }

    #[test]
    fn distinct_genomes_give_distinct_voices() {
        let mut seed = 0xD157_u32;
        for _ in 0..4000 {
            let a = rand_genome(&mut seed);
            let b = rand_genome(&mut seed);
            if a != b {
                assert_ne!(voice_from(a), voice_from(b), "collision for {:?} {:?}", a, b);
            }
        }
    }

    #[test]
    fn skill_zero_is_the_mimics_own_voice() {
        let mut seed = 0x0000_1234_u32;
        for _ in 0..1000 {
            let target = voice_from(rand_genome(&mut seed));
            assert_eq!(mimic(&target, 0.0), MIMIC_BASE);
        }
    }

    #[test]
    fn mimic_reaches_target_and_approaches_monotonically() {
        let mut seed = 0xE2E2_u32;
        for _ in 0..2000 {
            let target = voice_from(rand_genome(&mut seed));
            // skill 1 lands exactly on the target
            assert_eq!(mimic(&target, 1.0), target);

            // more skill never increases the gap to target (pitch as proxy)
            let s1 = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let s2 = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let lo = s1.min(s2);
            let hi = s1.max(s2);
            let gap_lo = (mimic(&target, lo).pitch - target.pitch).abs();
            let gap_hi = (mimic(&target, hi).pitch - target.pitch).abs();
            assert!(gap_hi <= gap_lo + 1e-3, "gap grew with skill");
        }
    }
}
