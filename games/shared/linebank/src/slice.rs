//! SLICING — one long recording into addressable takes.
//!
//! An actor records a session as a continuous file: slate the id, read the
//! line, beat of silence, next. This turns that into spans, and then maps the
//! spans onto the script that produced them.
//!
//! Pure arithmetic over samples. No decoding, no files — a caller hands over
//! `f32` mono and a sample rate, which means the whole thing is testable
//! against synthetic signals in microseconds and cannot be wrong in a way that
//! only shows up on real audio.
//!
//! ## Alignment is by ORDER, and mismatches are loud
//!
//! The tempting design is to detect the slate heuristically — it is short, the
//! line is long. That fails immediately on this project, because half the bank
//! is [`Kind::Blip`](crate::Kind::Blip) and a blip is shorter than the slate
//! announcing it.
//!
//! So there is no heuristic. [`assign`] takes the expected sequence in script
//! order and walks it, and if the count does not match it says so with both
//! numbers rather than silently pairing things up wrong. A misaligned session
//! that imports "successfully" costs a day of hunting; one that refuses costs
//! a minute of counting.
//!
//! `voiceprint-kit`'s fingerprint is the second line of defence: once spans are
//! assigned, comparing each against a reference read catches the case where the
//! COUNT is right and the ORDER is not. That belongs in whatever runs the
//! import, not here — this crate does not touch samples beyond their
//! amplitude.
//!
//! ## The thresholds are the whole job
//!
//! Studio silence is not zero — it is room tone, typically −60 to −45 dBFS,
//! and breaths sit above that. Cut too eagerly and a breath becomes its own
//! take; too slowly and two reads merge. Both defaults here are conservative
//! toward MERGING, because a merged pair is obvious on the first listen while a
//! stray breath-take hides in a bank of hundreds.

use crate::Kind;

/// A span of the recording, in samples, `[start, end)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn seconds(&self, sr: u32) -> f32 {
        self.len() as f32 / sr.max(1) as f32
    }
}

/// How to cut.
#[derive(Clone, Copy, Debug)]
pub struct Segmenter {
    /// Amplitude at or below which a window counts as silence, as dBFS.
    /// Room tone in a treated booth sits around −60; −50 leaves headroom
    /// without eating breaths.
    pub floor_db: f32,
    /// Silence shorter than this does not split. A comma is ~0.2 s and must
    /// not become a boundary; the script asks for a clear beat between takes.
    pub min_gap_s: f32,
    /// Spans shorter than this are discarded as lip noise. A short blip is
    /// ~0.25 s, so this has to stay well under that.
    pub min_take_s: f32,
    /// Kept either side of a span so a soft onset is not clipped.
    pub pad_s: f32,
}

impl Default for Segmenter {
    fn default() -> Segmenter {
        Segmenter { floor_db: -50.0, min_gap_s: 0.35, min_take_s: 0.12, pad_s: 0.03 }
    }
}

fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

impl Segmenter {
    /// Cut `samples` into spans of sound.
    pub fn segment(&self, samples: &[f32], sr: u32) -> Vec<Span> {
        // A zero sample rate makes every duration meaningless: the gap
        // threshold rounds to zero samples, so ANY quiet sample becomes a
        // boundary and a clean two-line session comes back as hundreds of
        // spans. Refusing is right — there is no sensible answer, and the
        // nonsense answer looks like a segmenter bug rather than a caller one.
        if sr == 0 || samples.is_empty() {
            return Vec::new();
        }
        let floor = db_to_amp(self.floor_db);
        // at least one sample, so a small rate cannot degenerate the same way
        let gap = ((self.min_gap_s * sr as f32) as usize).max(1);
        let min_len = ((self.min_take_s * sr as f32) as usize).max(1);
        let pad = (self.pad_s * sr as f32) as usize;

        let mut out: Vec<Span> = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut quiet_since: Option<usize> = None;

        for (i, s) in samples.iter().enumerate() {
            // rectified instantaneous level. A window RMS would be smoother,
            // but the gap requirement already supplies the smoothing and this
            // keeps the pass single and exact.
            if s.abs() > floor {
                if run_start.is_none() {
                    run_start = Some(i);
                }
                quiet_since = None;
            } else if run_start.is_some() && quiet_since.is_none() {
                quiet_since = Some(i);
            }

            if let (Some(start), Some(q)) = (run_start, quiet_since) {
                if i - q >= gap {
                    out.push(Span { start, end: q });
                    run_start = None;
                    quiet_since = None;
                }
            }
        }
        if let Some(start) = run_start {
            out.push(Span { start, end: quiet_since.unwrap_or(samples.len()) });
        }

        out.into_iter()
            .filter(|s| s.len() >= min_len)
            .map(|s| Span {
                start: s.start.saturating_sub(pad),
                end: (s.end + pad).min(samples.len()),
            })
            .collect()
    }
}

/// One expected item in the recording, in script order.
#[derive(Clone, Debug, PartialEq)]
pub struct Expected {
    pub unit: String,
    pub n: u8,
    pub kind: Kind,
}

/// A span matched to what it should be.
#[derive(Clone, Debug, PartialEq)]
pub struct Assigned {
    pub unit: String,
    pub n: u8,
    pub span: Span,
}

/// Why an import was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mismatch {
    /// The recording holds a different number of spans than the script
    /// expects. Both numbers, because the difference is the diagnosis: off by
    /// one is a missed slate, off by double is slating turned on or off
    /// halfway.
    Count { found: usize, expected: usize },
}

/// Map spans onto the script.
///
/// `slated` means the actor announced each id before reading it, so spans
/// alternate slate, take, slate, take. That is what `Bank::script_for`
/// instructs, and it is what makes an import checkable at all — without it a
/// dropped take shifts everything after it silently.
pub fn assign(
    spans: &[Span],
    expected: &[Expected],
    slated: bool,
) -> Result<Vec<Assigned>, Mismatch> {
    let want = if slated { expected.len() * 2 } else { expected.len() };
    if spans.len() != want {
        return Err(Mismatch::Count { found: spans.len(), expected: want });
    }
    let step = if slated { 2 } else { 1 };
    let offset = if slated { 1 } else { 0 };
    Ok(expected
        .iter()
        .enumerate()
        .map(|(i, e)| Assigned { unit: e.unit.clone(), n: e.n, span: spans[i * step + offset] })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Sound of a given length at a given level.
    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        let n = (secs * SR as f32) as usize;
        (0..n).map(|i| amp * ((i as f32) * 0.05).sin()).collect()
    }
    /// Room tone: quiet, but NOT zero. The case a naive `!= 0.0` test misses.
    fn room(secs: f32) -> Vec<f32> {
        let n = (secs * SR as f32) as usize;
        (0..n).map(|i| 0.0008 * ((i as f32) * 0.31).sin()).collect()
    }

    fn session(parts: &[(f32, f32)]) -> Vec<f32> {
        let mut v = Vec::new();
        for (secs, amp) in parts {
            if *amp <= 0.0 {
                v.extend(room(*secs));
            } else {
                v.extend(tone(*secs, *amp));
            }
        }
        v
    }

    #[test]
    fn silence_is_room_tone_and_not_zero() {
        // The bug a synthetic test with pure zeros would never find: a booth
        // floor is around -60 dBFS, so a threshold of "any nonzero sample"
        // finds one span covering the entire session.
        let s = session(&[(0.5, 0.0), (0.4, 0.6), (0.6, 0.0), (0.4, 0.6), (0.5, 0.0)]);
        assert_eq!(Segmenter::default().segment(&s, SR).len(), 2);
        let naive = Segmenter { floor_db: -120.0, ..Segmenter::default() };
        assert_eq!(naive.segment(&s, SR).len(), 1, "a -120 dB floor should hear room tone as speech");
    }

    #[test]
    fn a_short_gap_inside_a_line_does_not_split_it() {
        // "You have one shot." has a comma in it. A 0.15 s pause is
        // punctuation, not a boundary.
        let s = session(&[(0.4, 0.0), (0.5, 0.6), (0.15, 0.0), (0.5, 0.6), (0.6, 0.0)]);
        let spans = Segmenter::default().segment(&s, SR);
        assert_eq!(spans.len(), 1, "punctuation became a take boundary");
        assert!(spans[0].seconds(SR) > 1.1, "the two halves were not joined");
    }

    #[test]
    fn lip_noise_is_discarded_but_a_short_blip_is_not() {
        // These are close together, which is why min_take_s has to sit well
        // under the shortest real blip rather than just under it.
        let s = session(&[
            (0.4, 0.0),
            (0.03, 0.5), // a click
            (0.5, 0.0),
            (0.25, 0.5), // a real blip
            (0.5, 0.0),
        ]);
        let spans = Segmenter::default().segment(&s, SR);
        assert_eq!(spans.len(), 1, "kept the click or dropped the blip: {spans:?}");
        assert!(spans[0].seconds(SR) > 0.2);
    }

    #[test]
    fn padding_never_runs_off_either_end() {
        // A session that starts talking immediately and is cut off at the end
        // — common, because actors start before the engineer expects.
        let mut s = tone(0.5, 0.6);
        s.extend(room(0.6));
        s.extend(tone(0.5, 0.6));
        let spans = Segmenter::default().segment(&s, SR);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, 0, "padding produced a negative start");
        assert!(spans[1].end <= s.len(), "padding ran past the buffer");
    }

    #[test]
    fn an_empty_or_silent_recording_yields_nothing_rather_than_panicking() {
        let seg = Segmenter::default();
        assert!(seg.segment(&[], SR).is_empty());
        assert!(seg.segment(&room(3.0), SR).is_empty());
        // A zero sample rate is refused rather than answered. The first
        // version clamped it to 1 Hz, which made the gap threshold zero
        // samples, so every quiet sample split and a clean session came back
        // as hundreds of spans — nonsense that reads as a segmenter bug.
        assert!(seg.segment(&tone(0.5, 0.6), 0).is_empty());
    }

    fn expected(items: &[(&str, u8)]) -> Vec<Expected> {
        items
            .iter()
            .map(|(u, n)| Expected { unit: u.to_string(), n: *n, kind: Kind::Blip })
            .collect()
    }

    #[test]
    fn a_slated_session_assigns_the_reads_and_drops_the_slates() {
        // Six spans: slate, take, slate, take, slate, take.
        let spans: Vec<Span> =
            (0..6).map(|i| Span { start: i * 1000, end: i * 1000 + 500 }).collect();
        let e = expected(&[("billy_ack", 1), ("billy_ack", 2), ("billy_hit", 1)]);
        let got = assign(&spans, &e, true).unwrap();
        assert_eq!(got.len(), 3);
        // the ODD spans are the reads; the even ones are the announcements
        assert_eq!(got[0].span, spans[1]);
        assert_eq!(got[2].span, spans[5]);
        assert_eq!(got[1].n, 2);
    }

    #[test]
    fn an_unslated_session_assigns_one_to_one() {
        let spans: Vec<Span> = (0..3).map(|i| Span { start: i * 10, end: i * 10 + 5 }).collect();
        let e = expected(&[("a", 1), ("b", 1), ("c", 1)]);
        let got = assign(&spans, &e, false).unwrap();
        assert_eq!(got.iter().map(|a| a.span).collect::<Vec<_>>(), spans);
    }

    #[test]
    fn a_miscount_refuses_with_both_numbers_instead_of_guessing() {
        // The failure this module exists to make loud. A dropped slate shifts
        // every pairing after it, so an import that "succeeds" here mislabels
        // the rest of the session and the error surfaces weeks later as a
        // character saying the wrong line.
        let spans: Vec<Span> =
            (0..5).map(|i| Span { start: i * 10, end: i * 10 + 5 }).collect();
        let e = expected(&[("a", 1), ("b", 1), ("c", 1)]);
        assert_eq!(
            assign(&spans, &e, true),
            Err(Mismatch::Count { found: 5, expected: 6 })
        );
    }

    #[test]
    fn slating_turned_off_halfway_reads_as_a_count_error_not_a_shift() {
        // Off-by-double rather than off-by-one, which is the diagnosis: the
        // numbers tell you WHICH mistake it was.
        let spans: Vec<Span> = (0..4).map(|i| Span { start: i, end: i + 1 }).collect();
        let e = expected(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)]);
        match assign(&spans, &e, true) {
            Err(Mismatch::Count { found, expected }) => {
                assert_eq!((found, expected), (4, 8));
            }
            other => panic!("expected a count mismatch, got {other:?}"),
        }
    }

    #[test]
    fn segmenting_then_assigning_survives_a_realistic_session() {
        // End to end on synthetic audio shaped like the script asks for:
        // slate, line, beat, for two units.
        let s = session(&[
            (0.5, 0.0),
            (0.30, 0.5), // "billy ack one"
            (0.5, 0.0),
            (0.28, 0.6), // the blip
            (0.5, 0.0),
            (0.30, 0.5), // "billy ack two"
            (0.5, 0.0),
            (0.26, 0.6), // the blip
            (0.5, 0.0),
        ]);
        let spans = Segmenter::default().segment(&s, SR);
        assert_eq!(spans.len(), 4, "{spans:?}");
        let e = expected(&[("billy_ack", 1), ("billy_ack", 2)]);
        let got = assign(&spans, &e, true).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|a| a.span.seconds(SR) > 0.2));
    }
}
