//! reference_check — hold an imported (or any) project up against the song as
//! it really came out: the artist's own render.
//!
//! An import can be wrong in ways no unit test sees -- a clip a beat late, a
//! tempo read wrong, a vocal a bus too quiet. The render is the ground truth,
//! and three questions of it are cheap and robust even though Studio's synths
//! are stand-ins and its effects run at defaults:
//!
//! * **Duration**: does the song end where it ended?
//! * **Timing**: onsets (the rises of a 10 ms loudness envelope) are
//!   cross-correlated, over the whole song and per section. A misplaced clip
//!   shows as a section whose best lag is not the song's, a tempo error as a
//!   lag that drifts steadily. The correlation says how much the section's
//!   rhythm is shared at all -- low where the stand-ins carry the part.
//! * **Level**: the loudness difference per section (ours minus the
//!   render, dB, over frames where the render is not silent). A steady offset
//!   is gain staging; one section far off the rest is a clip or bus level.
//!
//! Pure over sample buffers; `examples/compare_reference.rs` renders a
//! project and runs it against a file.

/// Loudness per `hop` seconds (dB RMS of the mono sum, floored at -90).
pub fn envelope_db(mono: &[f32], rate: u32, hop: f64) -> Vec<f32> {
    let n = ((rate as f64 * hop) as usize).max(1);
    mono.chunks(n).map(|c| {
        let ms = c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32;
        (10.0 * ms.max(1e-9).log10()).max(-90.0)
    }).collect()
}

/// Onset strength: how much the envelope RISES frame to frame.
pub fn flux(env: &[f32]) -> Vec<f32> {
    std::iter::once(0.0).chain(env.windows(2).map(|w| (w[1] - w[0]).max(0.0))).collect()
}

/// The lag (in frames, within ±`max`) at which `ours` best matches `reference`
/// -- positive = ours is LATE -- and the normalised correlation there.
pub fn best_lag(reference: &[f32], ours: &[f32], max: usize) -> (i64, f32) {
    let n = reference.len().min(ours.len());
    if n < 4 { return (0, 0.0); }
    let mean = |v: &[f32]| v[..n].iter().sum::<f32>() / n as f32;
    let (ma, mb) = (mean(reference), mean(ours));
    let (a, b): (Vec<f32>, Vec<f32>) = (reference[..n].iter().map(|x| x - ma).collect(), ours[..n].iter().map(|x| x - mb).collect());
    let (na, nb) = (a.iter().map(|x| x * x).sum::<f32>().sqrt(), b.iter().map(|x| x * x).sum::<f32>().sqrt());
    if na == 0.0 || nb == 0.0 { return (0, 0.0); }
    let mut best = (0i64, f32::MIN);
    for lag in -(max as i64)..=(max as i64) {
        let mut s = 0.0f32;
        for i in 0..n {
            let j = i as i64 + lag;
            if j >= 0 && (j as usize) < n { s += a[i] * b[j as usize]; }
        }
        if s > best.1 { best = (lag, s); }
    }
    (best.0, best.1 / (na * nb))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section { pub from: f64, pub lag: f64, pub corr: f32, pub level_db: f32 }

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub reference_len: f64,
    pub ours_len: f64,
    pub lag: f64,
    pub corr: f32,
    pub sections: Vec<Section>,
}

impl Report {
    /// A readable report, with the sections that stand out marked.
    pub fn text(&self, section_label: &dyn Fn(f64) -> String) -> String {
        let mut t = format!("duration: render {:.2} s, ours {:.2} s ({:+.2} s)\n", self.reference_len, self.ours_len, self.ours_len - self.reference_len);
        t.push_str(&format!("timing, whole song: ours is {:+.0} ms against the render (onset correlation {:.2})\n", self.lag * 1000.0, self.corr));
        let lv: Vec<f32> = self.sections.iter().map(|s| s.level_db).filter(|l| l.is_finite()).collect();
        let median = { let mut v = lv.clone(); v.sort_by(f32::total_cmp); v.get(v.len() / 2).copied().unwrap_or(0.0) };
        t.push_str(&format!("level: ours is {median:+.1} dB against the render (median over sections)\n\nsection            lag      corr   level   \n"));
        for s in &self.sections {
            let mut flags = Vec::new();
            if (s.lag - self.lag).abs() > 0.03 && s.corr > 0.3 { flags.push(format!("timing {:+.0} ms off the song", (s.lag - self.lag) * 1000.0)); }
            if s.level_db.is_finite() && (s.level_db - median).abs() > 4.0 { flags.push(format!("level {:+.1} dB off the song", s.level_db - median)); }
            if s.corr < 0.2 { flags.push("little shared rhythm".into()); }
            t.push_str(&format!("{:<16} {:+6.0} ms  {:5.2}  {:+6.1} dB  {}\n", section_label(s.from), s.lag * 1000.0, s.corr,
                                s.level_db, flags.join("; ")));
        }
        t
    }
}

/// Compare two mono signals at one rate. Sections of `window` seconds; lags
/// searched within ±`max_lag` seconds; envelope hop 10 ms.
pub fn compare(reference: &[f32], ours: &[f32], rate: u32, window: f64, max_lag: f64) -> Report {
    const HOP: f64 = 0.01;
    let (er, eo) = (envelope_db(reference, rate, HOP), envelope_db(ours, rate, HOP));
    let (fr, fo) = (flux(&er), flux(&eo));
    let maxf = (max_lag / HOP) as usize;
    let (lag, corr) = best_lag(&fr, &fo, maxf);
    let w = (window / HOP) as usize;
    let mut sections = Vec::new();
    let mut k = 0;
    while k + w / 2 < fr.len() {
        let end = (k + w).min(fr.len());
        let (l, c) = best_lag(&fr[k..end], &fo[k.min(fo.len())..end.min(fo.len())], maxf);
        // Level as ENERGY over the section: a frame-by-frame dB difference
        // compares a burst with the silence beside it the moment the two are
        // a little misaligned (the first version read -65 dB for a -6 dB
        // copy 120 ms late). Mean power is indifferent to where in the
        // section the sound sits. NaN where the render is near silent.
        let power = |e: &[f32]| e.iter().map(|d| 10f32.powf(d / 10.0)).sum::<f32>();
        let (pr, po) = (power(&er[k..end]), power(&eo[k.min(eo.len())..end.min(eo.len())]));
        let level = if pr / (end - k) as f32 > 1e-6 { 10.0 * (po.max(1e-12) / pr).log10() } else { f32::NAN };
        sections.push(Section { from: k as f64 * HOP, lag: l as f64 * HOP, corr: c, level_db: level });
        k += w;
    }
    Report { reference_len: reference.len() as f64 / rate as f64, ours_len: ours.len() as f64 / rate as f64,
             lag: lag as f64 * HOP, corr, sections }
}

/// A CLICK: a lone high-frequency spike -- a discontinuity in the waveform.
///
/// The first detector (a sample-to-sample jump 12x its 10 ms neighbourhood)
/// could not tell a click from a DRUM ATTACK, and on Blue Moon's files-only
/// render most of what it found were drum hits. What separates them is what
/// comes AFTER: an attack is followed by more high-frequency energy (the
/// hit's noise and ring); a click is one spike and then the waveform goes on
/// smoothly. So a spike in the second difference counts only when it is
/// `ratio` times the second difference's RMS on BOTH sides of it (5 ms each,
/// the spike's own 0.3 ms left out). A clip that starts mid-waveform out of
/// silence still counts -- smooth after, silent before: that IS a click.
pub fn clicks(mono: &[f32], rate: u32, ratio: f32) -> Vec<f64> {
    let d2: Vec<f32> = (2..mono.len()).map(|n| mono[n] - 2.0 * mono[n - 1] + mono[n - 2]).collect();
    let (w, guard) = ((rate as usize) / 200, (rate as usize) / 3000 + 1);
    let rms = |a: usize, b: usize| {
        let (a, b) = (a.min(d2.len()), b.min(d2.len()));
        if b <= a { return 0.0; }
        (d2[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a) as f32).sqrt()
    };
    let mut out = Vec::new();
    let mut n = w + guard;
    while n + w + guard < d2.len() {
        let v = d2[n].abs();
        if v > 1e-3 {
            let (before, after) = (rms(n - w - guard, n - guard), rms(n + guard, n + guard + w));
            if v > ratio * before.max(after).max(1e-6) {
                out.push((n + 2) as f64 / rate as f64);
                n += w; // one click, not its neighbours
                continue;
            }
        }
        n += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bursts of noise at irregular times: a rhythm with one best alignment.
    fn bursts(rate: u32, secs: f64, seed: u64) -> Vec<f32> {
        let mut x = seed;
        let mut rnd = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 10_000) as f32 / 10_000.0 };
        let n = (rate as f64 * secs) as usize;
        let mut v = vec![0.0f32; n];
        let mut t = 0usize;
        while t < n {
            let len = (rate as f32 * (0.05 + 0.1 * rnd())) as usize;
            for i in t..(t + len).min(n) { v[i] = (rnd() - 0.5) * (1.0 - (i - t) as f32 / len as f32); }
            t += (rate as f32 * (0.2 + 0.5 * rnd())) as usize;
        }
        v
    }

    #[test]
    fn a_whole_song_offset_and_a_level_offset_are_measured() {
        let rate = 8000;
        let r = bursts(rate, 20.0, 7);
        let shift = (0.12 * rate as f64) as usize;
        let mut o = vec![0.0; shift];
        o.extend(r.iter().map(|x| x * 0.5)); // 120 ms late, -6 dB
        let rep = compare(&r, &o, rate, 5.0, 0.5);
        assert!((rep.lag - 0.12).abs() < 0.011, "{}", rep.lag);
        assert!(rep.corr > 0.8, "{}", rep.corr);
        let lv: Vec<f32> = rep.sections.iter().map(|s| s.level_db).collect();
        assert!(lv.iter().all(|l| (l + 6.0).abs() < 1.0), "{lv:?}");
    }

    #[test]
    fn one_misplaced_section_stands_out_from_the_song() {
        let rate = 8000;
        let r = bursts(rate, 20.0, 11);
        let mut o = r.clone();
        // section 10-15 s arrives 250 ms late (a clip placed a beat off)
        let (a, b, d) = (10 * rate as usize, 15 * rate as usize, (0.25 * rate as f64) as usize);
        let late: Vec<f32> = (a..b).map(|i| if i >= a + d { r[i - d] } else { 0.0 }).collect();
        o[a..b].copy_from_slice(&late);
        let rep = compare(&r, &o, rate, 5.0, 0.5);
        assert!(rep.lag.abs() < 0.011, "the song as a whole is aligned: {}", rep.lag);
        let s = rep.sections.iter().find(|s| (s.from - 10.0).abs() < 0.01).unwrap();
        assert!((s.lag - 0.25).abs() < 0.02, "{s:?}");
        let text = rep.text(&|t| format!("{t:.0} s"));
        assert!(text.lines().any(|l| l.starts_with("10 s") && l.contains("timing +250 ms off the song")), "{text}");
        assert!(!text.lines().any(|l| l.starts_with("0 s") && l.contains("timing")), "{text}");
    }

    fn tone(rate: u32, secs: f32, hz: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| 0.4 * (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin()).collect()
    }

    #[test]
    fn a_clean_tone_has_no_clicks_and_a_discontinuity_is_one() {
        let rate = 48_000;
        let t = tone(rate, 1.0, 220.0);
        assert!(clicks(&t, rate, 8.0).is_empty(), "{:?}", clicks(&t, rate, 8.0));
        let mut j = t.clone();
        for x in &mut j[24_000..] { *x = -*x; } // a polarity flip mid-cycle: a jump
        let c = clicks(&j, rate, 8.0);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!((c[0] - 0.5).abs() < 0.001, "{c:?}");
    }

    #[test]
    fn drum_attacks_are_not_clicks_but_a_hard_start_out_of_silence_is() {
        let rate = 48_000;
        // noise bursts with a fast decay: attacks, each followed by more HF
        let mut x = 7u64;
        let mut rnd = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 20_000) as f32 / 10_000.0 - 1.0 };
        let mut drums = tone(rate, 1.0, 110.0);
        for hit in [0.2f32, 0.45, 0.7] {
            let at = (hit * rate as f32) as usize;
            for k in 0..4800 { drums[at + k] += 0.6 * rnd() * (-(k as f32) / 600.0).exp(); }
        }
        assert!(clicks(&drums, rate, 8.0).is_empty(), "attacks read as clicks: {:?}", clicks(&drums, rate, 8.0));
        // a tone that starts mid-cycle out of silence: a clip cut hard
        let mut edge = vec![0.0f32; 24_000];
        edge.extend(tone(rate, 0.5, 220.0).iter().skip(50));
        assert_eq!(clicks(&edge, rate, 8.0).len(), 1, "{:?}", clicks(&edge, rate, 8.0));
    }
}
