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
    notes_with_company(project, rack).into_iter()
        .map(|(ch, v)| (ch, v.into_iter().filter(|c| c.others.is_empty()).map(|c| c.note).collect()))
        .collect()
}

/// A channel this quiet in FL (by its gain in the rack) is inaudible there
/// too: fitting it would measure whatever ELSE the render has at its notes.
/// (wavydrizzy's "Omnisphere #24": channel volume 900 of 12800, ~-56 dB --
/// its first "fit" scored 15.6 -> 6.4 dB against sound that was not it.)
pub const INAUDIBLE_GAIN: f32 = 0.01;

/// A note and what sounds with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Company {
    pub note: NoteAt,
    /// Pitches of the other SYNTH notes sounding with it (empty = alone).
    pub others: Vec<f32>,
    /// Time BLOCKED by drum / sampler hits: their spectra are broad and cannot
    /// be masked harmonic by harmonic, so those stretches are not measured.
    pub blocked: Vec<(f64, f64)>,
}

/// How long a short hit (kick, snare, hat -- a sample under
/// `LONG_SAMPLE` s, or a built-in drum voice) blocks from its onset: its
/// energy is in the attack. A LONG sample (an 808, a loop -- tonal, and
/// long) blocks its whole length. (Excluding whole notes that met any drum
/// hit left four of seven songs -- beats -- with nothing to measure.)
pub const HIT_BLOCK: f64 = 0.12;
pub const LONG_SAMPLE: f64 = 0.6;

/// The longest stretch of `c.note` (from its start, at most `max` s) not
/// blocked by a hit; `None` if under 0.16 s (the measurement skips 60 ms).
pub fn clean_window(c: &Company, max: f64) -> Option<(f64, f64)> {
    let (a, b) = (c.note.start, c.note.end.min(c.note.start + max));
    let mut cuts: Vec<(f64, f64)> = c.blocked.iter().filter(|x| x.0 < b && x.1 > a).copied().collect();
    cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (mut best, mut from) = (None::<(f64, f64)>, a);
    for (x0, x1) in cuts.iter().chain(std::iter::once(&(b, b))) {
        let seg = (from, x0.min(b));
        if seg.1 - seg.0 > best.map(|w| w.1 - w.0).unwrap_or(0.0) { best = Some(seg); }
        from = from.max(*x1);
    }
    best.filter(|w| w.1 - w.0 >= 0.16)
}

/// Every Poly channel's notes, each with its `Company`. A note that overlaps
/// an AUDIO clip is left out (a vocal is broadband and long). Channels below
/// `INAUDIBLE_GAIN` are left out.
pub fn notes_with_company(project: &studio_core::Project, rack: &studio_live::Rack) -> Vec<(usize, Vec<Company>)> {
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
    let pitch = |ch: usize, key: i32| match rack.channels[ch].kind {
        studio_live::SynthKind::Poly { hz, .. } | studio_live::SynthKind::Sine { hz } => Some(hz * 2f32.powf(key as f32 / 12.0)),
        _ => None, // a drum / sampler: broadband
    };
    // how long a hit of channel `o` blocks
    let block_len = |o: usize| match &rack.channels[o].kind {
        studio_live::SynthKind::Sampler { source } => {
            let len = project.sources.get(source).map(|s| s.len.to_seconds()).unwrap_or(HIT_BLOCK);
            if len > LONG_SAMPLE { len } else { HIT_BLOCK }
        }
        studio_live::SynthKind::Sub { decay_ms, .. } => (*decay_ms as f64 / 1000.0).max(HIT_BLOCK),
        _ => HIT_BLOCK,
    };
    (0..rack.channels.len()).filter_map(|ch| {
        let studio_live::SynthKind::Poly { hz, .. } = rack.channels[ch].kind else { return None };
        if rack.channels[ch].gain < INAUDIBLE_GAIN { return None; }
        let v = notes[ch].iter().filter_map(|n| {
            if audio_busy.iter().any(|b| overlaps((n.0, n.1), *b)) { return None; }
            let (mut others, mut blocked) = (Vec::new(), Vec::new());
            for (o, list) in notes.iter().enumerate() {
                if o == ch { continue; }
                let bl = block_len(o);
                for m in list.iter() {
                    match pitch(o, m.2) {
                        Some(p) => if overlaps((n.0, n.1), (m.0, m.1)) { others.push(p) },
                        None => if overlaps((n.0, n.1), (m.0, m.0 + bl)) { blocked.push((m.0, m.0 + bl)) },
                    }
                }
            }
            Some(Company { note: NoteAt { start: n.0, end: n.1, hz: hz * 2f32.powf(n.2 as f32 / 12.0) }, others, blocked })
        }).collect();
        Some((ch, v))
    }).collect()
}

/// Is harmonic `f` clear of every harmonic (1..=16) of the `others`, by more
/// than `tol` (relative: 0.015 = ~25 cents)?
pub fn clear_of(f: f32, others: &[f32], tol: f32) -> bool {
    others.iter().all(|o| (1..=16).all(|k| ((o * k as f32) - f).abs() > tol * f))
}

/// A MASKED profile: at each window, only harmonics clear of the other
/// sounding notes' harmonics count, each relative to the note's own
/// FUNDAMENTAL (a window whose fundamental collides is skipped); averaged per
/// harmonic over the windows where it was clear (at least 2), else 0 -- which
/// then means UNKNOWN, not absent: the scorer masks both renders alike, but a
/// FITTED patch lacks a harmonic that was never clear.
/// Windows: (samples, rate, pitch, other pitches). With no others it
/// measures like `profile_of`, relative to the fundamental.
pub fn masked_profile(windows: &[(Vec<f32>, u32, f32, Vec<f32>)]) -> Option<[u8; 16]> {
    const TOL: f32 = 0.015;
    let (mut sum, mut cnt) = ([0.0f64; 16], [0u32; 16]);
    for (w, rate, hz, others) in windows {
        let skip = (*rate as usize * 6 / 100).min(w.len());
        let w = &w[skip..];
        if w.len() < *rate as usize / 10 || !clear_of(*hz, others, TOL) { continue; }
        let f0 = goertzel(w, *hz, *rate as f32) as f64;
        if f0 <= 1e-9 { continue; }
        for k in 0..16 {
            let f = hz * (k + 1) as f32;
            if f >= 0.45 * *rate as f32 || !clear_of(f, others, TOL) { continue; }
            sum[k] += goertzel(w, f, *rate as f32) as f64 / f0;
            cnt[k] += 1;
        }
    }
    if cnt[0] == 0 { return None; }
    let rel: Vec<f64> = (0..16).map(|k| if cnt[k] >= 2 || k == 0 { sum[k] / cnt[k] as f64 } else { 0.0 }).collect();
    let peak = rel.iter().cloned().fold(0.0, f64::max).max(1e-12);
    let mut out = [0u8; 16];
    for k in 0..16 { out[k] = (rel[k] / peak * 255.0).round().clamp(0.0, 255.0) as u8; }
    Some(out)
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

    /// LAYERED: notes of timbre 1 : 0.5 : 0.25 under ANOTHER note (1 : 0.8 :
    /// 0.6 : 0.4) at a major third, a fifth, a fourth -- as a song varies its
    /// intervals. Each harmonic is clear in SOME windows (a fifth masks the 3rd
    /// harmonic -- it IS the other note's 2nd -- the third and fourth do not),
    /// and the masked profile gets the timbre back. (The first version used a
    /// fifth in every window: the 3rd was never clear and came out 0.)
    #[test]
    fn a_layered_note_is_measured_away_from_the_other_notes_harmonics() {
        let rate = 48_000u32;
        let tone = |hz: f32, amps: &[f32], n: usize| -> Vec<f32> { (0..n).map(|i| { let t = i as f32 / rate as f32;
            amps.iter().enumerate().map(|(k, a)| a * (std::f32::consts::TAU * hz * (k + 1) as f32 * t).sin()).sum::<f32>() }).collect() };
        let wins: Vec<(Vec<f32>, u32, f32, Vec<f32>)> = [(220.0f32, 1.2599f32), (247.0, 1.4983), (196.0, 1.3348), (233.0, 1.2599)].iter().map(|(hz, iv)| {
            let other = hz * iv;
            let a = tone(*hz, &[1.0, 0.5, 0.25], 38_400);
            let b = tone(other, &[1.0, 0.8, 0.6, 0.4], 38_400);
            (a.iter().zip(&b).map(|(x, y)| x + y).collect(), rate, *hz, vec![other])
        }).collect();
        let p = masked_profile(&wins).expect("a profile");
        let want = [255u8, 128, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(profile_error_db(&p, &want) < 3.0, "{p:?}");
        // and a harmonic that collides with the other note's is NOT counted
        assert!(!clear_of(660.0, &[330.0], 0.015) && clear_of(440.0, &[330.0], 0.015));
    }

    /// A kick in the middle of a note: the note still counts, measured in its
    /// longest stretch clear of the hit's 120 ms; a note hit every 150 ms has
    /// no stretch long enough.
    #[test]
    fn a_drum_hit_blocks_a_stretch_not_the_note() {
        let n = NoteAt { start: 10.0, end: 11.0, hz: 220.0 };
        let c = Company { note: n, others: vec![], blocked: vec![(10.3, 10.42)] };
        let w = clean_window(&c, 0.8).expect("a window");
        assert!((w.0 - 10.42).abs() < 1e-9 && (w.1 - 10.8).abs() < 1e-9, "after the hit, to the 0.8 s limit: {w:?}");
        let busy = Company { note: n, others: vec![], blocked: (0..7).map(|k| (10.0 + 0.15 * k as f64, 10.0 + 0.15 * k as f64 + 0.12)).collect() };
        assert_eq!(clean_window(&busy, 0.8), None, "hats every 150 ms: nothing clean long enough");
        assert_eq!(clean_window(&Company { note: n, others: vec![], blocked: vec![] }, 0.8), Some((10.0, 10.8)));
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

/// The complex DFT of `v` at frequency `f` (re, im) -- per channel, so the
/// SIDE (L - R) can be told from the MID.
pub fn dft(v: &[f32], f: f32, rate: f32) -> (f32, f32) {
    let w = std::f32::consts::TAU * f / rate;
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (n, x) in v.iter().enumerate() { let a = w * n as f32; re += x * a.cos(); im -= x * a.sin(); }
    (re / v.len().max(1) as f32, im / v.len().max(1) as f32)
}

/// A channel's stereo IMAGE, measured at its notes' clear harmonics.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Image {
    /// -1 (left) ..= 1 (right): the mean balance.
    pub pan: f32,
    /// |side| / |mid| above and below 250 Hz (0 = mono; 1 = as much side as mid).
    pub width_high: f32,
    pub width_low: f32,
    /// Mean |pan change| from a note's first half to its second: TRAVEL.
    pub travel: f32,
    /// Pan per octave from middle C (least squares over the notes).
    pub key_pan: f32,
    pub notes: usize,
}

/// Measure the image from stereo windows (left, right, rate, pitch, others).
pub fn stereo_image(windows: &[(Vec<f32>, Vec<f32>, u32, f32, Vec<f32>)]) -> Option<Image> {
    let (mut pans, mut octs, mut travel) = (Vec::new(), Vec::new(), Vec::new());
    let (mut sh, mut mh, mut sl, mut ml) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (l, r, rate, hz, others) in windows {
        let skip = (*rate as usize * 6 / 100).min(l.len());
        let (l, r) = (&l[skip..], &r[skip..r.len().min(l.len() + skip)]);
        if l.len() < *rate as usize / 10 { continue; }
        let half = l.len() / 2;
        let bal = |a: &[f32], b: &[f32]| -> Option<f32> {
            let (mut el, mut er) = (0.0f32, 0.0f32);
            for k in 1..=8 {
                let f = hz * k as f32;
                if f >= 0.45 * *rate as f32 || !clear_of(f, others, 0.015) { continue; }
                let (x, y) = (dft(a, f, *rate as f32), dft(b, f, *rate as f32));
                el += x.0 * x.0 + x.1 * x.1; er += y.0 * y.0 + y.1 * y.1;
            }
            (el + er > 1e-12).then(|| (er - el) / (er + el))
        };
        let Some(p) = bal(l, r) else { continue };
        if let (Some(a), Some(b)) = (bal(&l[..half], &r[..half]), bal(&l[half..], &r[half..])) { travel.push((b - a).abs()); }
        for k in 1..=8 {
            let f = hz * k as f32;
            if f >= 0.45 * *rate as f32 || !clear_of(f, others, 0.015) { continue; }
            let (x, y) = (dft(l, f, *rate as f32), dft(r, f, *rate as f32));
            let m = ((x.0 + y.0) as f64).powi(2) + ((x.1 + y.1) as f64).powi(2);
            let s = ((x.0 - y.0) as f64).powi(2) + ((x.1 - y.1) as f64).powi(2);
            if f < 250.0 { sl += s; ml += m; } else { sh += s; mh += m; }
        }
        pans.push(p);
        octs.push((hz / 261.63).log2());
    }
    if pans.is_empty() { return None; }
    let n = pans.len() as f32;
    let (mp, mo) = (pans.iter().sum::<f32>() / n, octs.iter().sum::<f32>() / n);
    let var: f32 = octs.iter().map(|o| (o - mo).powi(2)).sum();
    let key_pan = if var > 0.25 { pans.iter().zip(&octs).map(|(p, o)| (p - mp) * (o - mo)).sum::<f32>() / var } else { 0.0 };
    // all side and no mid (fully out of phase -- the artist's wide bass
    // texture) is not "mono": capped at 100, not 0
    let w = |s: f64, m: f64| if m > 1e-18 { ((s / m).sqrt() as f32).min(100.0) } else if s > 1e-18 { 100.0 } else { 0.0 };
    Some(Image { pan: mp, width_high: w(sh, mh), width_low: w(sl, ml),
                 travel: if travel.is_empty() { 0.0 } else { travel.iter().sum::<f32>() / travel.len() as f32 }, key_pan, notes: pans.len() })
}

#[cfg(test)]
mod image_tests {
    use super::*;

    fn tone(hz: f32, n: usize) -> Vec<f32> { (0..n).map(|i| (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect() }

    /// Hard right is pan ~1 and no width; a note with an out-of-phase
    /// (inverted) right channel is ALL side; pan rising with pitch is key_pan.
    #[test]
    fn pan_width_and_key_pan_are_read_from_left_and_right() {
        let t = tone(440.0, 24_000);
        let right = stereo_image(&[(vec![0.0; 24_000], t.clone(), 48_000, 440.0, vec![])]).unwrap();
        assert!(right.pan > 0.95, "{right:?}");
        let mono = stereo_image(&[(t.clone(), t.clone(), 48_000, 440.0, vec![])]).unwrap();
        assert!(mono.pan.abs() < 0.01 && mono.width_high < 0.01, "{mono:?}");
        let inv: Vec<f32> = t.iter().map(|x| -x).collect();
        let wide = stereo_image(&[(t.clone(), inv, 48_000, 440.0, vec![])]).unwrap();
        assert!(wide.width_high > 10.0, "out of phase: all side {wide:?}");
        // three notes, an octave apart, panned -0.5, 0, +0.5 by pitch
        let wins: Vec<_> = [(130.8f32, -0.5f32), (261.6, 0.0), (523.3, 0.5)].iter().map(|(hz, p)| {
            let (gl, gr) = studio_live::pan_gains(*p);
            let s = tone(*hz, 24_000);
            (s.iter().map(|x| x * gl).collect::<Vec<f32>>(), s.iter().map(|x| x * gr).collect::<Vec<f32>>(), 48_000u32, *hz, vec![])
        }).collect();
        let k = stereo_image(&wins).unwrap();
        assert!(k.key_pan > 0.3, "pan rises with pitch: {k:?}");
    }
}
