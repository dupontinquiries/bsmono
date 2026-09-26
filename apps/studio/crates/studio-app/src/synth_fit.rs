//! synth_fit -- a synth's TEXTURE measured from the artist's own render.
//!
//! The imported synths are stand-ins (studio_live::SynthKind::Poly). Their
//! timbre can come from the song itself: where a synth plays ALONE in the
//! render -- no other rack channel sounding, no audio clip -- the level of
//! each harmonic at the notes it plays IS its timbre. `harmonic_profile`
//! measures harmonics 1..=16 (Goertzel at k x the note's pitch, past the
//! attack), averages over the notes (each normalised first), and returns a
//! profile for `PolyPatch::partials` -- 16 bytes, "math referencing samples".
//!
//! What it does not capture: movement (filter sweeps, evolving wavetables),
//! noise, effects inside the plugin. A snapshot of the steady tone.

/// Goertzel: the magnitude of frequency `f` in `v`.
pub fn goertzel(v: &[f32], f: f32, rate: f32) -> f32 {
    let w = std::f32::consts::TAU * f / rate;
    let (c, mut s1, mut s2) = (2.0 * w.cos(), 0.0f32, 0.0f32);
    for x in v { let s0 = x + c * s1 - s2; s2 = s1; s1 = s0; }
    (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0).sqrt() / v.len().max(1) as f32
}

/// A note to measure: seconds in the audio, and its pitch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteAt { pub start: f64, pub end: f64, pub hz: f32 }

/// Harmonics 1..=16 of `notes` in `mono`, as a `PolyPatch::partials` profile
/// (the strongest harmonic = 255). `None` when no note gave a usable window
/// (at least 100 ms of sustain after skipping 60 ms of attack).
pub fn harmonic_profile(mono: &[f32], rate: u32, notes: &[NoteAt]) -> Option<[u8; 16]> {
    let r = rate as f64;
    let mut acc = [0.0f64; 16];
    let mut used = 0;
    for n in notes {
        let (a, b) = (n.start + 0.06, n.end.min(n.start + 0.8));
        if b - a < 0.1 { continue; }
        let (i0, i1) = ((a * r) as usize, ((b * r) as usize).min(mono.len()));
        if i1 <= i0 + 64 { continue; }
        let w = &mono[i0..i1];
        let mut m = [0.0f64; 16];
        for (k, v) in m.iter_mut().enumerate() {
            let f = n.hz * (k + 1) as f32;
            if f < 0.45 * rate as f32 { *v = goertzel(w, f, rate as f32) as f64; }
        }
        let peak = m.iter().cloned().fold(0.0, f64::max);
        if peak <= 1e-9 { continue; }
        for k in 0..16 { acc[k] += m[k] / peak; }
        used += 1;
    }
    if used == 0 { return None; }
    let peak = acc.iter().cloned().fold(0.0, f64::max);
    let mut out = [0u8; 16];
    for k in 0..16 { out[k] = (acc[k] / peak * 255.0).round().clamp(0.0, 255.0) as u8; }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two notes of a known timbre (1 : 0.5 : 0.25 on harmonics 1-3) at two
    /// pitches: the profile comes back as ~255 : 128 : 64, the rest ~0.
    #[test]
    fn a_known_timbre_is_measured_back() {
        let rate = 48_000u32;
        let mut audio = vec![0.0f32; rate as usize * 3];
        let notes = [NoteAt { start: 0.2, end: 1.2, hz: 220.0 }, NoteAt { start: 1.6, end: 2.6, hz: 330.0 }];
        for n in &notes {
            for i in (n.start * rate as f64) as usize..(n.end * rate as f64) as usize {
                let t = i as f32 / rate as f32;
                audio[i] += [1.0f32, 0.5, 0.25].iter().enumerate()
                    .map(|(k, a)| a * (std::f32::consts::TAU * n.hz * (k + 1) as f32 * t).sin()).sum::<f32>();
            }
        }
        let p = harmonic_profile(&audio, rate, &notes).expect("a profile");
        assert!(p[0] == 255 && (p[1] as i32 - 128).abs() <= 8 && (p[2] as i32 - 64).abs() <= 8, "{p:?}");
        assert!(p[3..].iter().all(|v| *v <= 6), "{p:?}");
        // too short to measure: nothing
        assert_eq!(harmonic_profile(&audio, rate, &[NoteAt { start: 0.2, end: 0.3, hz: 220.0 }]), None);
    }
}
