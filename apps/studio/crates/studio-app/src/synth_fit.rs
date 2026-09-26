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

/// Every Poly channel's notes that sound ALONE -- no other (unmuted) rack
/// channel's note overlapping, no audio clip on an unmuted track -- from the
/// project's own pattern clips: (channel index, notes, with pitch from the
/// patch's `hz` and the note's key). The fitter and the scorecard use the SAME
/// notes. Channels with none are listed with none.
pub fn solo_notes(project: &studio_core::Project, rack: &studio_live::Rack) -> Vec<(usize, Vec<NoteAt>)> {
    use studio_core::SourceKind;
    let step = 60.0 / rack.tempo_bpm as f64 / 4.0;
    let muted: std::collections::BTreeSet<_> = project.tracks.iter().filter(|t| t.muted).map(|t| t.id).collect();
    let mut notes: Vec<Vec<(f64, f64, i32)>> = vec![Vec::new(); rack.channels.len()];
    let mut audio_busy: Vec<(f64, f64)> = Vec::new();
    for c in project.clips.values().filter(|c| !muted.contains(&c.track)) {
        let (c0, c1, off) = (c.pos.to_seconds(), c.end().to_seconds(), c.src_in.to_seconds());
        match project.sources.get(&c.source).map(|s| &s.kind) {
            Some(SourceKind::Pattern { pattern, .. }) => {
                let Some(pt) = rack.patterns.get(*pattern) else { continue };
                for ch in 0..rack.channels.len() {
                    if rack.channels[ch].muted { continue; }
                    for n in pt.roll(ch) {
                        let t0 = c0 + n.start as f64 * step - off;
                        let t1 = t0 + n.len as f64 * step;
                        if t0 >= c0 && t0 < c1 { notes[ch].push((t0, t1.min(c1), n.key as i32)); }
                    }
                }
            }
            _ => audio_busy.push((c0, c1)),
        }
    }
    let overlaps = |a: (f64, f64), b: (f64, f64)| a.0 < b.1 && b.0 < a.1;
    (0..rack.channels.len()).filter_map(|ch| {
        let studio_live::SynthKind::Poly { hz, .. } = rack.channels[ch].kind else { return None };
        let alone = notes[ch].iter().filter(|n| {
            !audio_busy.iter().any(|b| overlaps((n.0, n.1), *b))
                && !notes.iter().enumerate().any(|(o, v)| o != ch && v.iter().any(|m| overlaps((n.0, n.1), (m.0, m.1))))
        }).map(|n| NoteAt { start: n.0, end: n.1, hz: hz * 2f32.powf(n.2 as f32 / 12.0) }).collect();
        Some((ch, alone))
    }).collect()
}

/// A profile from separately rendered windows: (samples, rate, pitch), each
/// measured past its first 60 ms, averaged like `harmonic_profile`.
pub fn profile_of(windows: &[(Vec<f32>, u32, f32)]) -> Option<[u8; 16]> {
    let mut acc = [0.0f64; 16];
    let mut used = 0;
    for (w, rate, hz) in windows {
        let skip = (*rate as usize * 6 / 100).min(w.len());
        let w = &w[skip..];
        if w.len() < *rate as usize / 10 { continue; }
        let mut m = [0.0f64; 16];
        for (k, v) in m.iter_mut().enumerate() {
            let f = hz * (k + 1) as f32;
            if f < 0.45 * *rate as f32 { *v = goertzel(w, f, *rate as f32) as f64; }
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

/// How far two profiles are apart: RMS dB difference over harmonics 1..=8,
/// each floored at -40 dB below the strongest (a near-zero harmonic in both
/// is a match, not a large ratio). The TIMBRE metric -- octave bands measure
/// tonal balance, and two sounds with the same balance can differ entirely
/// in harmonic make-up (Blue Moon's keys: guessed and fitted both ~2 dB RMS
/// in octave bands).
pub fn profile_error_db(a: &[u8; 16], b: &[u8; 16]) -> f32 {
    let db = |v: u8| (20.0 * (v.max(1) as f32 / 255.0).log10()).max(-40.0);
    let e: f32 = (0..8).map(|k| (db(a[k]) - db(b[k])).powi(2)).sum::<f32>() / 8.0;
    e.sqrt()
}

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
        // the same notes as separate windows give the same profile
        let wins: Vec<(Vec<f32>, u32, f32)> = notes.iter().map(|n| (audio[(n.start * 48_000.0) as usize..(n.end * 48_000.0) as usize].to_vec(), rate, n.hz)).collect();
        let q = profile_of(&wins).unwrap();
        assert!(profile_error_db(&p, &q) < 0.5, "{p:?} vs {q:?}");
    }

    #[test]
    fn the_profile_error_is_zero_for_the_same_timbre_and_large_for_another() {
        let a = [255, 128, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let b = [255, 20, 200, 90, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(profile_error_db(&a, &a) < 1e-6);
        assert!(profile_error_db(&a, &b) > 8.0, "{}", profile_error_db(&a, &b));
        let c = [255, 128, 64, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(profile_error_db(&a, &c) < 1.0, "a harmonic at the -40 dB floor in both is a match");
    }
}
