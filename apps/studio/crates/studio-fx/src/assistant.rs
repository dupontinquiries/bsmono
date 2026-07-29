//! assistant — diagnosis first, prescription second, and much smaller.
//!
//! ## What the iZotope assistants got right and wrong
//!
//! Their **critique** was genuinely good: fed a mix, they identified real,
//! specific problems — a low-mid build-up, a harsh upper-mid, a low end that
//! was wide when it should not be — and the findings were reliable across
//! wildly different material.
//!
//! Their **prescription** was the problem: heavy-handed, especially on EQ and
//! compression. The common experience is dialling their result back to somewhere
//! between 15% and 50% and preferring that to what they proposed.
//!
//! So this splits the two. [`analyze`] produces [`Finding`]s — what was observed,
//! where, and how confident — and that is the part worth having on its own; you
//! can read it and fix things yourself. [`propose`] turns findings into moves,
//! scaled by a **strength** that defaults to [`DEFAULT_STRENGTH`] = 0.3, sitting
//! in the middle of the range people actually use.
//!
//! Two rules follow from that, and both are tested:
//!
//! 1. **Strength 0 must change nothing.** The assistant has to be able to
//!    propose *nothing*, or "apply" becomes a button you can never fully undo
//!    by turning the dial down.
//! 2. **A clean signal must produce no findings.** This is the actual root of
//!    heavy-handedness: an assistant that always finds something will always do
//!    something, and a mix that is already fine gets damaged by a tool trying to
//!    justify itself. Every threshold here has a band where the answer is "this
//!    is fine" — and a test feeds it a well-balanced signal and requires silence.

use crate::analysis::Spectrum;

/// Where the assistant defaults to: 30% of what a full correction would be.
///
/// Chosen from how people actually used the tools this borrows from — dialled
/// back to roughly 15–50%. Starting in the middle of that band means the common
/// adjustment is a nudge rather than an undo.
pub const DEFAULT_STRENGTH: f32 = 0.3;

/// What the analysis noticed. The valuable half.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub kind: FindingKind,
    /// Where, when it is frequency-specific.
    pub center_hz: Option<f32>,
    /// How far from ideal, in dB or dB-equivalent — the size of the problem,
    /// kept separate from the size of the fix.
    pub excess_db: f32,
    /// 0..1. Reported so the UI can hedge a marginal call instead of stating it
    /// as fact, which is how a tool earns trust over a session.
    pub confidence: f32,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// Build-up around 200–500 Hz: the most common single problem in a mix.
    LowMidBuildup,
    /// 2–5 kHz forwardness that fatigues on long listens.
    Harshness,
    /// Not enough below ~80 Hz relative to the rest.
    ThinLowEnd,
    /// Very little dynamic range left — already squashed.
    OverCompressed,
    /// Unusually wide dynamic range for the material.
    UnderCompressed,
    /// Low end wide enough to smear the mix or trouble a vinyl cut.
    WideLowEnd,
    /// Negative correlation: the mix will lose material folded to mono.
    PhaseRisk,
}

/// A proposed move. Deliberately a *description*, so the UI can show it before
/// anything is applied — "here is what I would do" rather than a fait accompli.
#[derive(Debug, Clone, PartialEq)]
pub enum Move {
    Eq { hz: f32, gain_db: f32, q: f32 },
    Compress { threshold_db: f32, ratio: f32 },
    BandWidth { band_hz: f32, width: f32 },
    Gain { db: f32 },
}

/// Analyse a spectrum plus a couple of whole-signal measures.
///
/// `crest_db` is peak-minus-RMS: a rough dynamic-range figure, where ~14 dB is
/// a lively mix and under ~8 dB is already heavily compressed.
pub fn analyze(spec: &Spectrum, crest_db: f32, correlation: f32, low_width: f32) -> Vec<Finding> {
    let mut out = Vec::new();

    // Reference each band against the MEDIAN OF COMPARABLE BANDS, not against a
    // broadband average.
    //
    // The first cut compared a narrow band (200–500 Hz) against a very wide one
    // (100 Hz–8 kHz), and that is biased on any signal that is not perfectly
    // dense: the wide average is dragged down by the quiet bins between
    // partials, so the narrow band always looks hot. It duly diagnosed a
    // low-mid build-up in a deliberately balanced mix — which is precisely the
    // heavy-handedness this module exists to avoid, caught by its own test.
    //
    // Comparing octave bands against the median of the other octave bands is
    // both fair and robust: the median ignores one loud band rather than being
    // pulled by it, which is what a tonal-balance judgement actually wants.
    let octaves: [(f32, f32); 8] = [
        (40.0, 80.0), (80.0, 160.0), (160.0, 320.0), (320.0, 640.0),
        (640.0, 1280.0), (1280.0, 2560.0), (2560.0, 5120.0), (5120.0, 10_240.0),
    ];
    // Octave TOTALS, not per-bin averages. A flat spectrum measured with the
    // averaging version slopes down ~20 dB from 40 Hz to 5 kHz simply because
    // higher octaves hold more bins — so the analyser diagnosed every mix as
    // bright-thin, or missed real harshness entirely. Found by measuring the
    // octave levels of a deliberately flat signal and seeing them decline.
    let levels: Vec<f32> = octaves.iter().map(|(a, b)| spec.band_power_db(*a, *b)).collect();
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let overall = (sorted[3] + sorted[4]) * 0.5; // median of eight
    if overall < -70.0 { return out; } // nothing playing: say nothing
    // Findings compare OCTAVE totals against the median octave, so every
    // comparison is like-for-like in width.
    let oct = |i: usize| levels[i];

    let low_mid = (oct(2) + oct(3)) * 0.5; // 160–640 Hz
    let mud = low_mid - overall;
    // 3 dB of low-mid over the broadband average is normal and musical; only
    // past ~5 is it a problem worth mentioning.
    if mud > 5.0 {
        out.push(Finding {
            kind: FindingKind::LowMidBuildup,
            center_hz: Some(320.0),
            excess_db: mud - 3.0,
            confidence: ((mud - 5.0) / 6.0).clamp(0.3, 0.95),
            note: "low-mid build-up around 300 Hz — the usual cause of a mix sounding boxy".into(),
        });
    }

    let upper = (oct(5) + oct(6)) * 0.5; // 1.28–5.12 kHz
    let harsh = upper - overall;
    if harsh > 4.0 {
        out.push(Finding {
            kind: FindingKind::Harshness,
            center_hz: Some(3200.0),
            excess_db: harsh - 2.0,
            confidence: ((harsh - 4.0) / 6.0).clamp(0.3, 0.9),
            note: "forward 2–5 kHz — reads as presence at first and fatigue after ten minutes".into(),
        });
    }

    let low = oct(0); // 40–80 Hz
    if overall - low > 12.0 {
        out.push(Finding {
            kind: FindingKind::ThinLowEnd,
            center_hz: Some(60.0),
            excess_db: (overall - low - 10.0).min(8.0),
            confidence: 0.55,
            note: "little energy below 90 Hz relative to the rest".into(),
        });
    }

    if crest_db < 8.0 {
        out.push(Finding {
            kind: FindingKind::OverCompressed,
            center_hz: None,
            excess_db: 8.0 - crest_db,
            confidence: 0.8,
            note: "crest factor is already low — further compression will flatten it".into(),
        });
    } else if crest_db > 20.0 {
        out.push(Finding {
            kind: FindingKind::UnderCompressed,
            center_hz: None,
            excess_db: crest_db - 20.0,
            confidence: 0.5,
            note: "unusually wide dynamic range — intentional on some material".into(),
        });
    }

    if low_width > 0.5 {
        out.push(Finding {
            kind: FindingKind::WideLowEnd,
            center_hz: Some(80.0),
            excess_db: (low_width - 0.5) * 10.0,
            confidence: 0.7,
            note: "wide low end — smears the centre and troubles a vinyl cut".into(),
        });
    }

    if correlation < -0.1 {
        out.push(Finding {
            kind: FindingKind::PhaseRisk,
            center_hz: None,
            excess_db: -correlation * 10.0,
            confidence: 0.85,
            note: "negative correlation — material will disappear folded to mono".into(),
        });
    }
    out
}

/// Turn findings into moves at `strength` (0..1).
///
/// Every correction is scaled AND capped. The caps exist because the failure
/// this whole module is a reaction to is a tool that, given a big number,
/// proposes a big move: 9 dB of EQ and 6:1 of compression on a mix that needed
/// a nudge. Even at full strength nothing here proposes more than a competent
/// engineer would reach for on a first pass.
pub fn propose(findings: &[Finding], strength: f32) -> Vec<Move> {
    let s = strength.clamp(0.0, 1.0);
    if s == 0.0 { return Vec::new(); }
    let mut out = Vec::new();
    for f in findings {
        match f.kind {
            FindingKind::LowMidBuildup => out.push(Move::Eq {
                hz: f.center_hz.unwrap_or(320.0),
                // cap at 4 dB of cut even at full strength
                gain_db: -(f.excess_db * s).min(4.0),
                q: 1.1,
            }),
            FindingKind::Harshness => out.push(Move::Eq {
                hz: f.center_hz.unwrap_or(3200.0),
                gain_db: -(f.excess_db * s).min(3.5),
                q: 0.9,
            }),
            FindingKind::ThinLowEnd => out.push(Move::Eq {
                hz: f.center_hz.unwrap_or(60.0),
                gain_db: (f.excess_db * s).min(3.0),
                q: 0.7,
            }),
            FindingKind::UnderCompressed => out.push(Move::Compress {
                // gentle and high-threshold: catching peaks, not levelling
                threshold_db: -12.0 - f.excess_db * s,
                ratio: 1.0 + 1.5 * s,
            }),
            FindingKind::WideLowEnd => out.push(Move::BandWidth {
                band_hz: f.center_hz.unwrap_or(80.0),
                // toward mono, never past it
                width: (1.0 - s * 0.8).max(0.0),
            }),
            // Deliberately no move. An over-compressed master cannot be fixed by
            // more processing, and a phase problem is a mix decision, not a
            // mastering one — proposing a "fix" for either would be exactly the
            // over-reach this module exists to avoid. The finding still shows.
            FindingKind::OverCompressed | FindingKind::PhaseRisk => {}
        }
    }
    out
}

/// A one-line summary for the strip, including the honest case.
pub fn summary(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "nothing worth changing — this is already balanced".into();
    }
    let mut v: Vec<&Finding> = findings.iter().collect();
    v.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
    format!("{} ({} finding{})", v[0].note, findings.len(),
            if findings.len() == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    /// A DENSE signal following a tonal curve — sixty partials spread
    /// logarithmically, each scaled by `shape(hz)`.
    ///
    /// The first version of these tests used eight discrete sines, which is a
    /// sparse spectrum and not what a mix looks like: whether a band reads hot
    /// then depends on how many partials happen to fall inside it, and the
    /// analyser was judged against a signal no music resembles. Real material is
    /// broadband, so the test material is too.
    fn dense(shape: impl Fn(f32) -> f32, n: usize) -> Spectrum {
        let mut buf = vec![0.0f32; n];
        for k in 0..60 {
            let hz = 40.0 * (500.0f32).powf(k as f32 / 59.0); // 40 Hz → 20 kHz
            let amp = shape(hz) * 0.08;
            let phase = (k as f32) * 1.7; // decorrelate so they do not all peak together
            for (i, v) in buf.iter_mut().enumerate() {
                *v += (TAU * hz * i as f32 / RATE + phase).sin() * amp;
            }
        }
        Spectrum::analyze(&buf, RATE)
    }

    /// Broadly even across the spectrum — a mix with nothing wrong with it.
    fn balanced() -> Spectrum { dense(|_| 1.0, 8192) }

    #[test]
    fn a_balanced_mix_produces_no_findings_at_all() {
        // THE fix for heavy-handedness. An assistant that always finds something
        // will always DO something, and a mix that is already fine gets damaged
        // by a tool trying to justify itself. It has to be able to say "this is
        // fine" — and then propose nothing.
        let f = analyze(&balanced(), 13.0, 0.55, 0.28);
        assert!(f.is_empty(), "invented problems in a balanced mix: {f:?}");
        assert!(propose(&f, 1.0).is_empty(), "and proposed moves for them");
        assert!(summary(&f).contains("already balanced"));
    }

    #[test]
    fn silence_is_not_diagnosed() {
        let quiet = Spectrum::analyze(&vec![0.0f32; 8192], RATE);
        assert!(analyze(&quiet, 12.0, 1.0, 0.0).is_empty(), "nothing playing, nothing to say");
    }

    #[test]
    fn it_finds_the_problems_it_is_shown() {
        // The critique is the valuable half, so it has to actually work on real
        // signal characteristics rather than fire on everything.
        let muddy = dense(|hz| if (200.0..500.0).contains(&hz) { 4.0 } else { 1.0 }, 8192);
        let f = analyze(&muddy, 12.0, 0.5, 0.2);
        assert!(f.iter().any(|x| x.kind == FindingKind::LowMidBuildup), "{f:?}");

        let harsh = dense(|hz| if (2000.0..5000.0).contains(&hz) { 3.5 } else { 1.0 }, 8192);
        assert!(analyze(&harsh, 12.0, 0.5, 0.2).iter().any(|x| x.kind == FindingKind::Harshness));

        // whole-signal measures
        assert!(analyze(&balanced(), 5.0, 0.5, 0.2).iter()
                .any(|x| x.kind == FindingKind::OverCompressed), "crest 5 dB is squashed");
        assert!(analyze(&balanced(), 13.0, -0.4, 0.2).iter()
                .any(|x| x.kind == FindingKind::PhaseRisk), "negative correlation");
        assert!(analyze(&balanced(), 13.0, 0.5, 0.8).iter()
                .any(|x| x.kind == FindingKind::WideLowEnd), "a very wide low end");
    }

    #[test]
    fn strength_zero_changes_nothing_and_strength_scales_linearly() {
        // "Apply" must be fully undoable by turning the dial down, or the
        // assistant is a one-way door.
        let f = analyze(&dense(|hz| if (200.0..500.0).contains(&hz) { 4.0 } else { 1.0 }, 8192),
                        12.0, 0.5, 0.2);
        assert!(!f.is_empty());
        assert!(propose(&f, 0.0).is_empty(), "zero strength must propose nothing");

        let cut_at = |s: f32| -> f32 {
            propose(&f, s).iter().find_map(|m| match m {
                Move::Eq { gain_db, .. } if *gain_db < 0.0 => Some(*gain_db),
                _ => None,
            }).unwrap_or(0.0)
        };
        let (quarter, half) = (cut_at(0.25), cut_at(0.5));
        assert!(quarter < 0.0 && half < quarter, "more strength, more cut: {quarter} then {half}");
        assert!((half / quarter - 2.0).abs() < 0.3, "and roughly linear: {quarter} vs {half}");
    }

    #[test]
    fn the_default_sits_where_people_actually_dial_these_tools() {
        // The tools this borrows from were routinely dialled back to 15–50%.
        // Starting in the middle of that band makes the common adjustment a
        // nudge rather than an undo.
        assert!(DEFAULT_STRENGTH >= 0.15 && DEFAULT_STRENGTH <= 0.5);
    }

    #[test]
    fn even_at_full_strength_nothing_proposes_a_drastic_move() {
        // The failure being corrected: given a big number, propose a big move —
        // 9 dB of EQ and 6:1 on a mix that needed a nudge.
        let extreme = dense(|hz| if (200.0..500.0).contains(&hz) || (2000.0..5000.0).contains(&hz)
                            { 8.0 } else { 1.0 }, 8192);
        let f = analyze(&extreme, 25.0, 0.5, 0.95);
        assert!(!f.is_empty());
        for m in propose(&f, 1.0) {
            match m {
                Move::Eq { gain_db, .. } => assert!(gain_db.abs() <= 4.0,
                    "{gain_db} dB of EQ is more than a first pass should reach for"),
                Move::Compress { ratio, .. } => assert!(ratio <= 2.6,
                    "{ratio}:1 is heavy-handed for an automatic suggestion"),
                Move::BandWidth { width, .. } => assert!((0.0..=1.0).contains(&width)),
                Move::Gain { db } => assert!(db.abs() <= 6.0),
            }
        }
    }

    #[test]
    fn some_findings_deliberately_propose_nothing() {
        // An over-compressed master cannot be fixed by more processing, and a
        // phase problem is a mix decision rather than a mastering one. Proposing
        // a "fix" for either is exactly the over-reach this module exists to
        // avoid — but the finding still shows, because knowing is the point.
        let squashed = analyze(&balanced(), 4.0, 0.5, 0.2);
        assert!(squashed.iter().any(|f| f.kind == FindingKind::OverCompressed));
        assert!(propose(&squashed, 1.0).is_empty(), "nothing to do about it with more processing");

        let phasey = analyze(&balanced(), 13.0, -0.5, 0.2);
        assert!(phasey.iter().any(|f| f.kind == FindingKind::PhaseRisk));
        assert!(propose(&phasey, 1.0).is_empty());
        // and the critique is still delivered
        assert!(summary(&phasey).contains("mono"));
    }

    #[test]
    fn findings_carry_their_confidence_so_the_ui_can_hedge() {
        // A marginal call stated as fact is how a tool loses trust in one
        // session; reporting confidence lets the display say "possibly".
        let f = analyze(&dense(|hz| if (200.0..500.0).contains(&hz) { 3.5 } else { 1.0 }, 8192),
                        12.0, 0.5, 0.2);
        for x in &f {
            assert!(x.confidence > 0.0 && x.confidence <= 1.0, "{x:?}");
            assert!(!x.note.is_empty(), "every finding explains itself");
        }
    }
}
