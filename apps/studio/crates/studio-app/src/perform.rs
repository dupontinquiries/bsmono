//! perform — playing a synth, and getting a pattern back.
//!
//! ## The gap this closes
//!
//! Every piece existed and nothing joined them. `midi::parse` turns wire
//! bytes into `MidiMessage`. `studio_live::Pattern` holds a piano roll of
//! `Note`s indexed in steps. The rack plays patterns. What was missing is
//! the arrow: nothing turned notes a person PLAYED into a pattern they
//! could then edit.
//!
//! So a synth could be played and the performance evaporated.
//!
//! Named `perform` rather than `capture` because `capture` is already
//! the microphone — audio takes, not note takes. Two different things
//! that both record.
//!
//! ## Quantisation is the whole design
//!
//! A performance arrives in milliseconds; a pattern is indexed in steps.
//! Rounding is not a detail — it decides whether the thing you played is
//! the thing you get back. Three rules, each of which could reasonably
//! go the other way:
//!
//! * **Round to nearest, not down.** A note struck two milliseconds
//!   early is a note on the beat. Flooring puts it a whole step earlier,
//!   which is audibly wrong in a way that being slightly late is not.
//! * **A note shorter than a step still gets length 1.** `Note::len`
//!   documents `>= 1`, and a zero-length note does not sound. A staccato
//!   stab is short, not absent.
//! * **Notes past the pattern's end are KEPT.** `Pattern`'s own rule is
//!   that a note at or past `steps` is hidden rather than deleted, so
//!   lengthening brings it back. Discarding on capture would break that
//!   round-trip at the one moment the data is irreplaceable.
//!
//! ## Velocity, and where the Wooting comes in
//!
//! `analog-kit` reports how FAR a key is pressed, not merely that it is
//! down — so a keyboard that is not velocity-sensitive becomes one that
//! is. That depth is the same 0..1 a MIDI velocity normalises to, which
//! is why this takes a velocity rather than a source: by the time a note
//! arrives here, one played on an analog keyboard and one played on a
//! MIDI controller are the same note, and neither this module nor the
//! pattern needs to know which was which.

use studio_live::{Note, Pattern};

/// A note as it was played: real time, real duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Played {
    /// Semitone offset from the channel's pitch, matching `Note::key`.
    pub key: i8,
    pub start_ms: u64,
    pub len_ms: u64,
    /// 0..1. From MIDI velocity, or from analog key depth.
    pub vel: f32,
}

/// Milliseconds per step.
///
/// `steps_per_beat` is 4 for sixteenths. A parameter rather than a
/// constant so a triplet grid is the caller's argument instead of a
/// second copy of this arithmetic somewhere else.
pub fn step_ms(tempo_bpm: f32, steps_per_beat: u32) -> f32 {
    (60_000.0 / tempo_bpm.max(1.0)) / steps_per_beat.max(1) as f32
}

/// Turn a performance into notes on a step grid.
///
/// `origin_ms` is when recording started — subtracted first, so a take
/// beginning ten minutes into a session still starts at step 0.
pub fn quantise(played: &[Played], origin_ms: u64, tempo_bpm: f32, steps_per_beat: u32)
    -> Vec<Note>
{
    let ms = step_ms(tempo_bpm, steps_per_beat);
    let mut out: Vec<Note> = played
        .iter()
        .map(|p| Note {
            key: p.key,
            start: ((p.start_ms.saturating_sub(origin_ms) as f32 / ms).round()).max(0.0) as u32,
            len: ((p.len_ms as f32 / ms).round() as u32).max(1),
            // Velocity lives on the NOTE, not in `mods`. `Note` carries
            // its own `vel` for exactly this, and `mods` is keyed by
            // (channel, STEP) — so two notes of a chord, which share a
            // step, would overwrite each other's velocity there. A chord
            // played with one voice leaning is the ordinary case.
            vel: p.vel.clamp(0.0, 1.0),
            // Slide is a performance gesture this cannot observe: it is
            // "glide from the previous note" and nothing in a note-on
            // stream says whether the player meant that. Left false for
            // the editor to set rather than guessed from note overlap,
            // which would turn every legato passage into a portamento.
            slide: false, nudge: 0.0 })
        .collect();
    // `Pattern::rolls` documents its notes as sorted by (start, key). A
    // caller must not have to know to re-sort.
    out.sort_by_key(|n| (n.start, n.key));
    out
}

/// Record a take onto a channel.
///
/// Notes are APPENDED, not replaced. Overdubbing is how parts are built,
/// and replacing would make every take destructive — recoverable only by
/// undo, and only if the person noticed.
///
/// Returns how many notes landed.
pub fn record_into(
    pattern: &mut Pattern,
    channel: usize,
    played: &[Played],
    origin_ms: u64,
    tempo_bpm: f32,
    steps_per_beat: u32,
) -> usize {
    let notes = quantise(played, origin_ms, tempo_bpm, steps_per_beat);
    let n = notes.len();
    let roll = pattern.rolls.entry(channel).or_default();
    roll.extend(notes);
    // Re-sorted after the append: an overdub interleaves with what was
    // already there.
    roll.sort_by_key(|x| (x.start, x.key));
    n
}

/// A take in progress.
///
/// Held notes are separate from finished ones because a note has no
/// length until it is released — and a take can legitimately end while
/// keys are still down, which is what `finish` is for.
#[derive(Debug, Default)]
pub struct Take {
    held: Vec<(i8, u64, f32)>,
    done: Vec<Played>,
}

impl Take {
    pub fn new() -> Take { Take::default() }

    pub fn note_on(&mut self, key: i8, vel: f32, now_ms: u64) {
        // A second note-on for a held key without a note-off happens on
        // real hardware — a dropped message, a fast re-strike. Closing
        // the earlier one makes it short instead of making it vanish.
        self.note_off(key, now_ms);
        self.held.push((key, now_ms, vel));
    }

    pub fn note_off(&mut self, key: i8, now_ms: u64) {
        if let Some(i) = self.held.iter().position(|(k, _, _)| *k == key) {
            let (key, start, vel) = self.held.remove(i);
            self.done.push(Played {
                key, start_ms: start, len_ms: now_ms.saturating_sub(start), vel,
            });
        }
    }

    /// End the take, closing anything still held.
    ///
    /// Still-held notes are closed at `now_ms`, not discarded: stopping
    /// with a chord ringing is ordinary, and dropping them would lose
    /// the last thing played, reliably, every time.
    pub fn finish(&mut self, now_ms: u64) -> Vec<Played> {
        let keys: Vec<i8> = self.held.iter().map(|(k, _, _)| *k).collect();
        for k in keys { self.note_off(k, now_ms); }
        self.done.sort_by_key(|p| (p.start_ms, p.key));
        std::mem::take(&mut self.done)
    }

    pub fn is_empty(&self) -> bool { self.held.is_empty() && self.done.is_empty() }
    pub fn holding(&self) -> usize { self.held.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(key: i8, start_ms: u64, len_ms: u64) -> Played {
        Played { key, start_ms, len_ms, vel: 1.0 }
    }

    /// 120 bpm sixteenths: 125 ms per step.
    const BPM: f32 = 120.0;
    const SPB: u32 = 4;

    #[test]
    fn step_length_is_the_tempo_arithmetic() {
        assert!((step_ms(120.0, 4) - 125.0).abs() < 0.01);
        assert!((step_ms(60.0, 4) - 250.0).abs() < 0.01);
    }

    #[test]
    fn a_note_struck_slightly_early_lands_on_the_beat() {
        // Flooring would put it a whole step earlier, which is audibly
        // wrong in a way that being slightly late is not.
        assert_eq!(quantise(&[p(0, 248, 125)], 0, BPM, SPB)[0].start, 2);
    }

    #[test]
    fn a_stab_shorter_than_a_step_still_sounds() {
        // Note::len documents >= 1; a zero-length note does not sound.
        assert_eq!(quantise(&[p(0, 0, 10)], 0, BPM, SPB)[0].len, 1);
    }

    #[test]
    fn a_take_that_starts_late_in_the_session_still_starts_at_step_zero() {
        let t = 600_000;
        assert_eq!(quantise(&[p(0, t, 125)], t, BPM, SPB)[0].start, 0);
    }

    #[test]
    fn notes_past_the_end_are_kept_not_dropped() {
        // Pattern hides a note at or past `steps` rather than deleting
        // it, so lengthening brings it back. Discarding here would break
        // that round-trip when the data cannot be recovered.
        let n = quantise(&[p(0, 0, 125), p(0, 10_000, 125)], 0, BPM, SPB);
        assert_eq!(n.len(), 2, "a late note was discarded: {n:?}");
        assert_eq!(n[1].start, 80);
    }

    #[test]
    fn a_held_note_is_closed_when_the_take_ends() {
        // Stopping with a chord ringing is ordinary; dropping those
        // notes would lose the last thing played, every time.
        let mut t = Take::new();
        t.note_on(0, 1.0, 0);
        t.note_on(4, 1.0, 0);
        t.note_off(0, 100);
        let out = t.finish(500);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out.iter().find(|x| x.key == 4).unwrap().len_ms, 500);
    }

    #[test]
    fn a_retrigger_without_a_note_off_does_not_lose_the_first_note() {
        let mut t = Take::new();
        t.note_on(7, 1.0, 0);
        t.note_on(7, 1.0, 200);
        let out = t.finish(400);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].len_ms, 200);
    }

    #[test]
    fn recording_appends_rather_than_replaces() {
        let mut pat = Pattern::new("p", 16, 1);
        record_into(&mut pat, 0, &[p(0, 0, 125)], 0, BPM, SPB);
        record_into(&mut pat, 0, &[p(7, 250, 125)], 0, BPM, SPB);
        assert_eq!(pat.rolls[&0].len(), 2, "{:?}", pat.rolls[&0]);
    }

    #[test]
    fn an_overdub_leaves_the_roll_sorted() {
        let mut pat = Pattern::new("p", 16, 1);
        record_into(&mut pat, 0, &[p(0, 500, 125)], 0, BPM, SPB);
        record_into(&mut pat, 0, &[p(3, 0, 125)], 0, BPM, SPB);
        let r = &pat.rolls[&0];
        assert!(r.windows(2).all(|w| (w[0].start, w[0].key) <= (w[1].start, w[1].key)),
                "{r:?}");
    }

    #[test]
    fn an_analog_depth_becomes_the_notes_velocity() {
        // The Wooting case: a key pressed halfway is a quieter note, and
        // it arrives as the same 0..1 a MIDI velocity normalises to.
        let mut pat = Pattern::new("p", 16, 1);
        let soft = Played { key: 0, start_ms: 0, len_ms: 125, vel: 0.4 };
        record_into(&mut pat, 0, &[soft], 0, BPM, SPB);
        assert!((pat.rolls[&0][0].vel - 0.4).abs() < 0.01, "{:?}", pat.rolls[&0]);
    }

    #[test]
    fn two_notes_of_a_chord_keep_their_own_velocities() {
        // The reason velocity is on the NOTE and not in `mods`: mods are
        // keyed by (channel, step), so a chord's notes share a key there
        // and would overwrite each other. A chord with one voice leaning
        // is the ordinary case, not an edge one.
        let mut pat = Pattern::new("p", 16, 1);
        let a = Played { key: 0, start_ms: 0, len_ms: 125, vel: 0.9 };
        let b = Played { key: 4, start_ms: 0, len_ms: 125, vel: 0.3 };
        record_into(&mut pat, 0, &[a, b], 0, BPM, SPB);
        let r = &pat.rolls[&0];
        assert_eq!(r.len(), 2, "{r:?}");
        assert!((r.iter().find(|n| n.key == 4).unwrap().vel - 0.3).abs() < 0.01, "{r:?}");
        assert!((r.iter().find(|n| n.key == 0).unwrap().vel - 0.9).abs() < 0.01, "{r:?}");
    }
}
