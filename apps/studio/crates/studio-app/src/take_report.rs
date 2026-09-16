//! take_report — which of these eight takes is the usable one.
//!
//! `takes.rs` puts every take of the same phrase on its own lane and
//! `comp_ops` copies chosen spans onto the parent. What neither does is help
//! you CHOOSE. A vocal session is eight takes of the same two bars, and
//! picking between them by soloing each in turn is most of the time comping
//! costs.
//!
//! Four faults decide a take, and each is cheap to measure:
//!
//! | fault | how it is heard | measured as |
//! |---|---|---|
//! | clipping | crunch on the loud words | samples at or past full scale |
//! | plosives | a thud that pumps the whole mix | sub-120 Hz energy in bursts |
//! | sibilance | harsh esses that need a de-esser | 5–9 kHz against the body |
//! | a noisy room | hiss under everything | the quietest tenth's level |
//!
//! ## Nothing here says "the best take"
//!
//! [`Report::rank`] orders by how much is WRONG, which is not the same thing,
//! and the module is careful not to pretend otherwise. The best take is the
//! one that was performed best, and no measurement of a waveform knows that.
//! A clean take of a bad read ranks first here and belongs in the bin.
//!
//! So the ranking answers a narrower question — "which of these will fight me
//! in the mix" — and the report keeps the per-fault numbers so a person can
//! disagree with the order for a reason. A take that clips twice on one word
//! is a punch-in, not a reject.
//!
//! ## Windowed, not whole-take
//!
//! Every measure is per window and then aggregated, because a fault in one
//! word is the normal case and a whole-take average hides it. A single
//! plosive on "peace" averages to nothing across four bars and is exactly
//! what you need told.

use studio_fx::analysis::Spectrum;

/// Analysis window, in samples. A POWER OF TWO, always.
///
/// `analysis::fft` is radix-2 and asserts on anything else, so a window
/// expressed in milliseconds would panic at every rate where the arithmetic
/// did not happen to land on one — 40 ms at 48 kHz is 1920 samples, and that
/// is a crash rather than a bad reading.
///
/// 2048 is ~43 ms at 48 kHz and ~46 ms at 44.1: long enough that a 120 Hz
/// measurement has several cycles to work with, short enough that one word is
/// several windows, which is the point of windowing at all.
pub const WINDOW: usize = 2048;

/// At or above this, a sample counts as clipped.
///
/// Slightly under full scale: converters and float rendering leave peaks at
/// 0.999-something, and a threshold of exactly 1.0 finds nothing on material
/// that audibly crunches.
pub const CLIP_LEVEL: f32 = 0.992;

/// Plosive energy lives below this.
pub const PLOSIVE_HZ: f32 = 120.0;

/// Sibilance band.
pub const SIB_LO_HZ: f32 = 5_000.0;
pub const SIB_HI_HZ: f32 = 9_000.0;

/// What one take is like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// Fraction of samples at or past [`CLIP_LEVEL`], 0..1.
    pub clipped: f32,
    /// Worst single window's sub-120 Hz level above the take's own median,
    /// in dB. A plosive is a BURST, so this is a peak-over-typical rather
    /// than an absolute: a voice with a lot of chest is not a plosive.
    pub plosive_db: f32,
    /// Worst window's 5–9 kHz level relative to the body, in dB.
    pub sibilance_db: f32,
    /// The quietest tenth of windows, in dBFS — the room under the take.
    pub noise_floor_db: f32,
    /// Windows analysed. Zero means the take was too short to judge.
    pub windows: usize,
}

impl Report {
    /// A single number for ordering, lower is cleaner.
    ///
    /// The weights are a judgement and are written here rather than buried:
    /// clipping is unfixable and weighted hardest, a plosive is fixable with a
    /// high-pass and weighted least, sibilance sits between because a de-esser
    /// costs quality, and a noisy room taints every quiet passage.
    ///
    /// Deliberately NOT normalised to 0..1. A score of 12 against a score of 3
    /// should read as four times worse, and squashing it into a percentage
    /// would hide exactly that.
    pub fn penalty(&self) -> f32 {
        if self.windows == 0 {
            return f32::INFINITY;
        }
        // Clipping as a percentage, so one clipped sample in a million is not
        // a fault and one in fifty is.
        let clip = self.clipped * 100.0 * 4.0;
        let plosive = (self.plosive_db - 6.0).max(0.0) * 0.5;
        let sib = (self.sibilance_db + 10.0).max(0.0) * 0.8;
        let noise = (self.noise_floor_db + 60.0).max(0.0) * 0.6;
        clip + plosive + sib + noise
    }

    /// Faults worth naming, worst first. Empty when the take is clean.
    pub fn faults(&self) -> Vec<&'static str> {
        let mut v: Vec<(f32, &'static str)> = Vec::new();
        if self.clipped > 0.0001 {
            v.push((self.clipped * 400.0, "clipping"));
        }
        if self.plosive_db > 6.0 {
            v.push(((self.plosive_db - 6.0) * 0.5, "plosives"));
        }
        if self.sibilance_db > -10.0 {
            v.push(((self.sibilance_db + 10.0) * 0.8, "sibilance"));
        }
        if self.noise_floor_db > -60.0 {
            v.push(((self.noise_floor_db + 60.0) * 0.6, "noisy room"));
        }
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v.into_iter().map(|(_, n)| n).collect()
    }
}

/// Measure one take.
pub fn analyse(samples: &[f32], rate: f32) -> Report {
    let w = WINDOW;
    if samples.len() < w {
        return Report {
            clipped: 0.0,
            plosive_db: 0.0,
            sibilance_db: f32::NEG_INFINITY,
            noise_floor_db: f32::NEG_INFINITY,
            windows: 0,
        };
    }
    let clipped =
        samples.iter().filter(|s| s.abs() >= CLIP_LEVEL).count() as f32 / samples.len() as f32;

    let mut lows = Vec::new();
    let mut sibs = Vec::new();
    let mut rms_db = Vec::new();
    for chunk in samples.chunks(w) {
        if chunk.len() < w {
            break; // a partial tail window would read as a fade
        }
        let s = Spectrum::analyze(chunk, rate);
        lows.push(s.band_db(0.0, PLOSIVE_HZ));
        // Against the body rather than absolute: a loud take is not a sibilant
        // one, and the ratio is what a de-esser actually responds to.
        sibs.push(s.band_db(SIB_LO_HZ, SIB_HI_HZ) - s.band_db(200.0, 4_000.0));
        let power: f32 = chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32;
        rms_db.push(10.0 * (power + 1e-12).log10());
    }
    if lows.is_empty() {
        return Report { clipped, plosive_db: 0.0, sibilance_db: f32::NEG_INFINITY,
                        noise_floor_db: f32::NEG_INFINITY, windows: 0 };
    }

    let median = {
        let mut v = lows.clone();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let plosive_db = lows.iter().cloned().fold(f32::MIN, f32::max) - median;
    let sibilance_db = sibs.iter().cloned().fold(f32::MIN, f32::max);

    // The QUIETEST TENTH, not the minimum: one digital-silence window at the
    // head would report a perfect room on a hissy take.
    let noise_floor_db = {
        let mut v = rms_db.clone();
        v.sort_by(f32::total_cmp);
        let n = (v.len() / 10).max(1);
        v[..n].iter().sum::<f32>() / n as f32
    };

    Report { clipped, plosive_db, sibilance_db, noise_floor_db, windows: lows.len() }
}

/// Order takes by how much is wrong, cleanest first.
///
/// Returns indices into `takes`. Stable on ties so a rebuild does not reshuffle
/// a list someone is reading.
pub fn rank(takes: &[Report]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..takes.len()).collect();
    idx.sort_by(|&a, &b| takes[a].penalty().total_cmp(&takes[b].penalty()));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (RATE * secs) as usize;
        (0..n).map(|i| amp * (TAU * hz * i as f32 / RATE).sin()).collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// A clean take has nothing to report.
    #[test]
    fn a_clean_take_has_no_faults() {
        let r = analyse(&tone(220.0, 0.3, 1.0), RATE);
        assert!(r.windows > 10);
        assert_eq!(r.clipped, 0.0);
        assert!(r.faults().is_empty(), "{:?} from {r:?}", r.faults());
    }

    /// CLIPPING is the unfixable one and is weighted hardest.
    #[test]
    fn clipping_is_detected_and_dominates_the_penalty() {
        let mut hot = tone(220.0, 1.4, 1.0);
        for s in hot.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        let clipped = analyse(&hot, RATE);
        let clean = analyse(&tone(220.0, 0.3, 1.0), RATE);
        assert!(clipped.clipped > 0.05, "got {}", clipped.clipped);
        assert!(clipped.faults().contains(&"clipping"));
        assert!(clipped.penalty() > clean.penalty() * 2.0);
    }

    /// A PLOSIVE IS A BURST, so it is measured against the take's own median
    /// rather than absolutely — a voice with a lot of chest is not a plosive.
    #[test]
    fn a_chesty_voice_is_not_mistaken_for_a_plosive() {
        // Constant low energy throughout: lots of bottom, no burst.
        let chesty = mix(&tone(80.0, 0.4, 1.0), &tone(400.0, 0.3, 1.0));
        let r = analyse(&chesty, RATE);
        assert!(!r.faults().contains(&"plosives"),
                "plosive_db {} on steady low end", r.plosive_db);
    }

    /// ...but a burst IS one.
    #[test]
    fn a_low_frequency_burst_reads_as_a_plosive() {
        let mut v = tone(400.0, 0.3, 1.0);
        // A 40 Hz thump over two windows, a third of the way in.
        let start = v.len() / 3;
        let thump = tone(40.0, 0.9, 0.08);
        for (i, t) in thump.iter().enumerate() {
            if start + i < v.len() {
                v[start + i] += t;
            }
        }
        let r = analyse(&v, RATE);
        assert!(r.plosive_db > 6.0, "plosive_db {}", r.plosive_db);
        assert!(r.faults().contains(&"plosives"));
    }

    /// Sibilance is RELATIVE to the body: a loud take is not a sibilant one.
    #[test]
    fn loudness_alone_does_not_read_as_sibilance() {
        let quiet = analyse(&tone(300.0, 0.05, 1.0), RATE);
        let loud = analyse(&tone(300.0, 0.7, 1.0), RATE);
        assert!((quiet.sibilance_db - loud.sibilance_db).abs() < 3.0,
                "{} vs {}", quiet.sibilance_db, loud.sibilance_db);
    }

    /// A 7 kHz shriek over a body tone is.
    #[test]
    fn energy_in_the_ess_band_reads_as_sibilance() {
        let body = tone(300.0, 0.3, 1.0);
        let hissy = mix(&body, &tone(7_000.0, 0.3, 1.0));
        let a = analyse(&body, RATE);
        let b = analyse(&hissy, RATE);
        assert!(b.sibilance_db > a.sibilance_db + 10.0,
                "{} vs {}", a.sibilance_db, b.sibilance_db);
    }

    /// THE QUIETEST TENTH, not the minimum: one silent window at the head
    /// would otherwise report a perfect room on a hissy take.
    #[test]
    fn a_silent_lead_in_does_not_hide_a_noisy_room() {
        let mut v = vec![0.0f32; (RATE * 0.05) as usize];
        // Hiss-ish: a steady mid tone well below the signal.
        v.extend(mix(&tone(300.0, 0.3, 1.0), &tone(3_000.0, 0.02, 1.0)));
        let r = analyse(&v, RATE);
        assert!(r.noise_floor_db > -90.0,
                "a couple of silent windows should not read as a silent room: {}",
                r.noise_floor_db);
    }

    /// Ranking orders by how much is wrong. Stable, so a list someone is
    /// reading does not reshuffle on a rebuild.
    #[test]
    fn takes_rank_cleanest_first_and_ties_keep_their_order() {
        let clean = analyse(&tone(220.0, 0.3, 1.0), RATE);
        let mut hot = tone(220.0, 1.4, 1.0);
        for s in hot.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        let bad = analyse(&hot, RATE);
        assert_eq!(rank(&[bad, clean, clean]), vec![1, 2, 0]);
    }

    /// A take too short to judge is reported as unjudged rather than perfect —
    /// zero windows must not rank first.
    #[test]
    fn a_take_too_short_to_judge_ranks_last() {
        let stub = analyse(&[0.1, 0.2, 0.3], RATE);
        assert_eq!(stub.windows, 0);
        assert!(stub.penalty().is_infinite());
        let ok = analyse(&tone(220.0, 0.3, 1.0), RATE);
        assert_eq!(rank(&[stub, ok]), vec![1, 0]);
    }

    /// Faults come out worst-first, so a UI can show one line and be right.
    #[test]
    fn faults_are_ordered_by_severity() {
        let r = Report { clipped: 0.02, plosive_db: 7.0, sibilance_db: -9.0,
                         noise_floor_db: -59.0, windows: 50 };
        let f = r.faults();
        assert_eq!(f.first(), Some(&"clipping"), "{f:?}");
        assert!(f.len() >= 2);
    }
}
