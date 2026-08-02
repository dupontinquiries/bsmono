//! ring — `IncomingCallDialog` and `RingtoneSynth`.
//!
//! `group_voice.rs` ported the in-call surface, and voice packets had nowhere to
//! arrive. This is the prompt that appears when a voice or floor packet lands
//! for a conversation whose thread is not open.
//!
//! ## The prompt carries no call state
//!
//! It is a pure question with three answers — Accept, Decline, and *nothing*.
//! The owner does the work: opening the conversation in-call, or dropping the
//! packets buffered while it rang. That separation is why the model below is a
//! handful of fields and an outcome enum rather than anything resembling a call.
//!
//! Two of the three answers are the same answer. Decline and the 30-second
//! timeout both reject, because a call nobody picked up was declined — the
//! caller should not be left listening to a ring that will never end, and the
//! buffered packets need dropping either way.
//!
//! ## Stopping the ring is the part that gets missed
//!
//! Qt stops it on the accept path, the decline path, the timeout path, and again
//! in the destructor as "a safety net for close paths that bypass the buttons".
//! Four calls for one rule, and `stopRingtone()` is idempotent precisely because
//! they overlap. A ringtone that outlives its dialog is the worst possible bug
//! in this file: the prompt is gone, so there is nothing left to press, and the
//! phone simply will not stop making noise. Here `answer()` is the single exit
//! and the ring state is derived from the outcome, so there is no path that can
//! forget.
//!
//! ## The ring is synthesised, not an asset
//!
//! Three bell notes — E5, C#5, A4, an A-major arpeggio settling on its root —
//! with an exponential decay, two harmonics for warmth, and a feedback comb for
//! the echo. Worth porting rather than shipping a WAV: it adapts to whatever
//! sample rate the device offers, it costs no download, and the fade-out exists
//! for a specific reason the tests pin — the loop point must land on silence, or
//! every repeat starts with a click.

use std::f64::consts::PI;

const TWO_PI: f64 = 2.0 * PI;

/// `kRingTimeoutMs` — how long a call rings before it declines itself.
pub const RING_TIMEOUT_MS: f32 = 30_000.0;

/// One note of the motif.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub freq_hz: f64,
    /// Offset into the cycle where it starts.
    pub start_sec: f64,
}

/// The synth's parameters, mirroring `RingtoneParams`.
#[derive(Debug, Clone, PartialEq)]
pub struct RingtoneParams {
    pub notes: Vec<Note>,
    /// Per-note ring length.
    pub note_sec: f64,
    /// Soft attack, anti-click.
    pub attack_sec: f64,
    /// `exp(-t * decay_rate)` bell decay.
    pub decay_rate: f64,
    /// Second-harmonic mix, for warmth.
    pub harm2: f64,
    pub harm3: f64,
    pub echo_delay_sec: f64,
    pub echo_gain: f64,
    /// Total loop length.
    pub cycle_sec: f64,
    pub fade_in_sec: f64,
    /// Long, so the loop point is silent.
    pub fade_out_sec: f64,
    pub master: f64,
}

impl Default for RingtoneParams {
    /// The app's ring: E5 → C#5 → A4.
    fn default() -> Self {
        RingtoneParams {
            notes: vec![
                Note { freq_hz: 659.25, start_sec: 0.00 },   // E5
                Note { freq_hz: 554.37, start_sec: 0.24 },   // C#5
                Note { freq_hz: 440.00, start_sec: 0.48 },   // A4
            ],
            note_sec: 0.30,
            attack_sec: 0.006,
            decay_rate: 6.0,
            harm2: 0.25,
            harm3: 0.08,
            echo_delay_sec: 0.28,
            echo_gain: 0.40,
            cycle_sec: 2.60,
            fade_in_sec: 0.005,
            fade_out_sec: 0.60,
            master: 0.60,
        }
    }
}

/// Render one loop cycle as mono samples in -1..=1.
///
/// A bad sample rate returns empty rather than panicking or guessing: the device
/// picks the rate, and a headset that reports something absurd should silence
/// the ring, not crash the call prompt.
pub fn render_mono(p: &RingtoneParams, sample_rate: i32) -> Vec<f32> {
    if sample_rate <= 0 { return Vec::new(); }
    let sr = sample_rate as f64;
    let n = (sr * p.cycle_sec) as usize;
    if n == 0 { return Vec::new(); }
    let mut buf = vec![0.0f32; n];

    // The motif: bell-like notes, each an exponentially decaying sine with two
    // quiet harmonics stacked on it. The harmonics are what stop it sounding
    // like a test tone.
    for note in &p.notes {
        let s0 = (note.start_sec * sr) as usize;
        let ns = (p.note_sec * sr) as usize;
        for k in 0..ns {
            let idx = s0 + k;
            if idx >= n { continue; }
            let t = k as f64 / sr;
            let mut env = (-t * p.decay_rate).exp();
            // A note that starts at full amplitude clicks; the attack ramp is
            // six milliseconds and entirely audible by its absence.
            if p.attack_sec > 0.0 && t < p.attack_sec { env *= t / p.attack_sec; }
            let f = note.freq_hz;
            let w = (TWO_PI * f * t).sin()
                + p.harm2 * (TWO_PI * 2.0 * f * t).sin()
                + p.harm3 * (TWO_PI * 3.0 * f * t).sin();
            buf[idx] += (w * env) as f32;
        }
    }

    // Feedback comb: each sample picks up a scaled copy of itself one delay
    // back. Because it reads from the buffer it is already writing, the echo
    // feeds its own repeats — one pass gives an infinite decaying series, which
    // is the point.
    let d = (p.echo_delay_sec * sr) as usize;
    if d > 0 && p.echo_gain != 0.0 {
        for i in d..n {
            buf[i] += (p.echo_gain as f32) * buf[i - d];
        }
    }

    // Master level and the fades. The fade-out is long because the loop point
    // must land on silence: without it every repeat begins with a click, and a
    // ring that clicks thirty times is worse than no ring.
    let fade_in = ((p.fade_in_sec * sr) as usize).max(1);
    let fade_out = ((p.fade_out_sec * sr) as usize).max(1);
    for i in 0..n {
        let mut a = p.master;
        if i < fade_in { a *= i as f64 / fade_in as f64; }
        if i > n - fade_out { a *= (n - i) as f64 / fade_out as f64; }
        buf[i] = ((buf[i] as f64 * a).clamp(-1.0, 1.0)) as f32;
    }
    buf
}

/// Duplicate a mono cycle across channels, for a device that wants stereo.
pub fn to_channels(mono: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 { return mono.to_vec(); }
    let mut out = Vec::with_capacity(mono.len() * channels);
    for s in mono {
        for _ in 0..channels { out.push(*s); }
    }
    out
}

// ── the prompt ──────────────────────────────────────────────────────────────

/// How the ring ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// `QDialog::accept()` — open the conversation in-call.
    Accepted,
    /// `reject()` from the button.
    Declined,
    /// `reject()` from the timeout. Distinct from Declined ONLY so the owner can
    /// log a missed call differently; both drop the buffered packets.
    TimedOut,
}

impl Answer {
    /// Both rejections are rejections. A call nobody picked up was declined.
    pub fn is_reject(self) -> bool { !matches!(self, Answer::Accepted) }
}

/// The ring prompt. Holds no call state — it is a question with three answers.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingCall {
    pub caller_name: String,
    /// Milliseconds left before it declines itself.
    remaining_ms: f32,
    answer: Option<Answer>,
}

pub const KICKER: &str = "INCOMING VOICE";
pub const UNKNOWN_CALLER: &str = "Unknown";

impl IncomingCall {
    pub fn new(caller_name: &str) -> Self {
        IncomingCall {
            caller_name: caller_name.to_string(),
            remaining_ms: RING_TIMEOUT_MS,
            answer: None,
        }
    }

    /// An unnamed caller is "Unknown", not a blank line. A prompt with no name
    /// on it looks like a rendering fault at exactly the moment the user has to
    /// decide something.
    pub fn display_name(&self) -> &str {
        if self.caller_name.trim().is_empty() { UNKNOWN_CALLER } else { &self.caller_name }
    }

    pub fn ringing(&self) -> bool { self.answer.is_none() }
    pub fn answer_given(&self) -> Option<Answer> { self.answer }

    /// The ring plays exactly while the question is open. Derived rather than
    /// tracked: Qt needs four `stopRingtone()` calls to cover accept, decline,
    /// timeout and the destructor, and a ringtone that outlives its dialog
    /// leaves the phone making noise with nothing left to press.
    pub fn ringtone_playing(&self) -> bool { self.ringing() }

    /// Seconds left, for a countdown ring or a progress arc.
    pub fn remaining_secs(&self) -> f32 { (self.remaining_ms / 1000.0).max(0.0) }

    /// Fraction of the ring window left, 1.0 → 0.0.
    pub fn progress(&self) -> f32 { (self.remaining_ms / RING_TIMEOUT_MS).clamp(0.0, 1.0) }

    /// Advance. Returns the answer at the moment it is decided, and only then —
    /// so a caller can act on it once without tracking edges itself.
    pub fn tick(&mut self, dt_ms: f32) -> Option<Answer> {
        if self.answer.is_some() { return None; }
        self.remaining_ms = (self.remaining_ms - dt_ms).max(0.0);
        if self.remaining_ms > 0.0 { return None; }
        self.answer = Some(Answer::TimedOut);
        Some(Answer::TimedOut)
    }

    /// Answer it. The single exit: idempotent, and the first answer wins.
    ///
    /// The first-answer-wins rule matters because Accept and the timeout can
    /// land in the same frame — the user presses at 29.99 seconds — and letting
    /// the timeout overwrite an accept would drop a call the user took.
    pub fn answer(&mut self, a: Answer) -> Option<Answer> {
        if self.answer.is_some() { return None; }
        self.answer = Some(a);
        self.remaining_ms = 0.0;
        Some(a)
    }

    pub fn accept(&mut self) -> Option<Answer> { self.answer(Answer::Accepted) }
    pub fn decline(&mut self) -> Option<Answer> { self.answer(Answer::Declined) }
}

/// Draw the bottom sheet. Returns the answer the moment one is given.
pub fn ring_ui(ui: &mut egui::Ui, call: &mut IncomingCall,
               c: &uikit_egui::PaletteColors) -> Option<Answer> {
    if !call.ringing() { return None; }
    let mut out = None;
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);
        ui.colored_label(c.text_weak, egui::RichText::new(KICKER).small());
        ui.heading(call.display_name());
        // The countdown is shown, not hidden. A prompt that vanishes without
        // warning reads as a dropped call rather than an unanswered one.
        ui.colored_label(c.text_weak, format!("{:.0}s", call.remaining_secs()));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Decline").clicked() { out = call.decline(); }
            if ui.add(egui::Button::new(
                egui::RichText::new("Accept").color(c.bg_base)).fill(c.accent)).clicked() {
                out = call.accept();
            }
        });
        ui.add_space(8.0);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_starts_ringing_with_the_full_window() {
        let c = IncomingCall::new("Ada");
        assert!(c.ringing());
        assert!(c.ringtone_playing());
        assert_eq!(c.progress(), 1.0);
        assert_eq!(c.remaining_secs(), 30.0);
    }

    #[test]
    fn an_unnamed_caller_is_unknown_rather_than_a_blank_line() {
        // A prompt with no name looks like a rendering fault at exactly the
        // moment the user has to decide something.
        assert_eq!(IncomingCall::new("   ").display_name(), UNKNOWN_CALLER);
        assert_eq!(IncomingCall::new("Ada").display_name(), "Ada");
    }

    #[test]
    fn an_ignored_ring_declines_itself_after_thirty_seconds() {
        let mut c = IncomingCall::new("Ada");
        assert_eq!(c.tick(29_000.0), None, "still ringing");
        assert_eq!(c.tick(1_000.0), Some(Answer::TimedOut));
        assert!(!c.ringing());
    }

    #[test]
    fn the_answer_is_reported_once_and_not_every_frame_after() {
        // So a caller can act on it without tracking edges itself.
        let mut c = IncomingCall::new("Ada");
        c.tick(RING_TIMEOUT_MS);
        assert_eq!(c.tick(1_000.0), None);
        assert_eq!(c.answer_given(), Some(Answer::TimedOut));
    }

    #[test]
    fn declining_and_timing_out_are_both_rejections() {
        // A call nobody picked up was declined: the caller must not be left
        // listening to a ring that never ends, and the buffered packets need
        // dropping either way.
        assert!(Answer::Declined.is_reject());
        assert!(Answer::TimedOut.is_reject());
        assert!(!Answer::Accepted.is_reject());
    }

    #[test]
    fn the_two_rejections_stay_distinguishable_for_the_log() {
        // Same effect, different history: a missed call and a refused one are
        // not the same thing to the person reading the list later.
        assert_ne!(Answer::Declined, Answer::TimedOut);
    }

    #[test]
    fn an_accept_in_the_last_moment_beats_the_timeout() {
        // Both can land in the same frame — the user presses at 29.99 seconds —
        // and letting the timeout overwrite the accept drops a call the user
        // took.
        let mut c = IncomingCall::new("Ada");
        c.tick(29_990.0);
        assert_eq!(c.accept(), Some(Answer::Accepted));
        assert_eq!(c.tick(1_000.0), None, "the timeout no longer fires");
        assert_eq!(c.answer_given(), Some(Answer::Accepted));
    }

    #[test]
    fn answering_twice_changes_nothing() {
        let mut c = IncomingCall::new("Ada");
        assert_eq!(c.accept(), Some(Answer::Accepted));
        assert_eq!(c.decline(), None);
        assert_eq!(c.answer_given(), Some(Answer::Accepted));
    }

    #[test]
    fn the_ringtone_stops_on_every_exit_path() {
        // Qt needs four stopRingtone() calls to cover accept, decline, timeout
        // and the destructor. Here it is derived from the outcome, so no path
        // can forget — and a ringtone outliving its dialog leaves the phone
        // making noise with nothing left to press.
        for answer in [Answer::Accepted, Answer::Declined] {
            let mut c = IncomingCall::new("Ada");
            assert!(c.ringtone_playing());
            c.answer(answer);
            assert!(!c.ringtone_playing(), "{answer:?} left the ring playing");
        }
        let mut timed = IncomingCall::new("Ada");
        timed.tick(RING_TIMEOUT_MS);
        assert!(!timed.ringtone_playing());
    }

    #[test]
    fn the_countdown_runs_from_one_to_zero() {
        let mut c = IncomingCall::new("Ada");
        c.tick(15_000.0);
        assert!((c.progress() - 0.5).abs() < 0.01);
        c.tick(99_000.0);
        assert_eq!(c.progress(), 0.0);
        assert_eq!(c.remaining_secs(), 0.0, "never negative");
    }

    // ── the synth ───────────────────────────────────────────────────────────

    #[test]
    fn the_motif_is_an_a_major_arpeggio_settling_on_its_root() {
        let p = RingtoneParams::default();
        let f: Vec<f64> = p.notes.iter().map(|n| n.freq_hz).collect();
        assert_eq!(f, vec![659.25, 554.37, 440.00], "E5, C#5, A4");
        // Descending, so it resolves downward onto the root rather than
        // climbing away from it.
        assert!(f[0] > f[1] && f[1] > f[2]);
        // Each note starts before the previous has decayed, which is what makes
        // it a motif rather than three beeps.
        for w in p.notes.windows(2) {
            assert!(w[1].start_sec - w[0].start_sec < p.note_sec);
        }
    }

    #[test]
    fn a_cycle_is_the_length_the_parameters_ask_for() {
        let p = RingtoneParams::default();
        let buf = render_mono(&p, 48_000);
        assert_eq!(buf.len(), (48_000.0 * 2.60) as usize);
    }

    #[test]
    fn the_synth_follows_whatever_rate_the_device_offers() {
        // No asset means no resampling: the ring is generated at the hardware's
        // rate whatever that turns out to be.
        for sr in [8_000, 22_050, 44_100, 48_000] {
            let buf = render_mono(&RingtoneParams::default(), sr);
            assert_eq!(buf.len(), (sr as f64 * 2.60) as usize, "at {sr}Hz");
            assert!(buf.iter().any(|s| s.abs() > 0.01), "silent at {sr}Hz");
        }
    }

    #[test]
    fn an_absurd_sample_rate_gives_silence_rather_than_a_panic() {
        // The device picks the rate; a headset reporting nonsense should
        // silence the ring, not crash the call prompt.
        assert!(render_mono(&RingtoneParams::default(), 0).is_empty());
        assert!(render_mono(&RingtoneParams::default(), -1).is_empty());
    }

    #[test]
    fn the_loop_point_lands_on_silence_so_repeats_do_not_click() {
        // THE reason the fade-out is 0.6s. A ring that clicks thirty times is
        // worse than no ring.
        let buf = render_mono(&RingtoneParams::default(), 48_000);
        let last = *buf.last().unwrap();
        let first = buf[0];
        assert!(last.abs() < 0.001, "cycle ends loud: {last}");
        assert!(first.abs() < 0.001, "cycle starts loud: {first}");
    }

    #[test]
    fn nothing_clips() {
        // Three notes plus a feedback comb plus harmonics overshoot easily; the
        // clamp is what keeps it from tearing on a loud device.
        let buf = render_mono(&RingtoneParams::default(), 48_000);
        assert!(buf.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn the_echo_puts_energy_where_the_notes_are_not() {
        // The comb is what makes it a ring rather than three notes and silence.
        let sr = 48_000;
        let mut dry = RingtoneParams::default();
        dry.echo_gain = 0.0;
        let wet = render_mono(&RingtoneParams::default(), sr);
        let dry_buf = render_mono(&dry, sr);
        // A second in: past every note's 0.3s tail, so anything here is echo.
        let at = sr as usize;
        let energy = |b: &[f32], from: usize| -> f64 {
            b[from..from + 4_000].iter().map(|s| (*s as f64).abs()).sum()
        };
        assert!(energy(&wet, at) > energy(&dry_buf, at) * 2.0,
                "the echo should still be sounding after the notes stop");
    }

    #[test]
    fn the_comb_feeds_itself_so_one_pass_gives_repeated_echoes() {
        // Reading from the buffer being written is deliberate: it turns a single
        // delay line into an infinite decaying series.
        let sr = 8_000;
        let mut p = RingtoneParams::default();
        p.fade_out_sec = 0.001;   // do not let the fade hide the later repeats
        let buf = render_mono(&p, sr);
        let d = (p.echo_delay_sec * sr as f64) as usize;
        let win = |from: usize| -> f64 {
            buf[from..(from + 200).min(buf.len())].iter().map(|s| (*s as f64).abs()).sum()
        };
        // Three successive delay taps should each still carry energy.
        assert!(win(d) > 0.0 && win(2 * d) > 0.0 && win(3 * d) > 0.0);
    }

    #[test]
    fn the_harmonics_are_quiet_enough_to_be_warmth_not_a_chord() {
        let p = RingtoneParams::default();
        assert!(p.harm2 < 0.5 && p.harm3 < p.harm2);
    }

    #[test]
    fn the_attack_ramp_stops_each_note_starting_with_a_click() {
        // Six milliseconds, and entirely audible by its absence.
        let p = RingtoneParams::default();
        assert!(p.attack_sec > 0.0);
        let sr = 48_000;
        let buf = render_mono(&p, sr);
        // Just inside the first note's attack, well past the master fade-in.
        let i = (p.fade_in_sec * sr as f64) as usize + 8;
        let j = (p.attack_sec * sr as f64) as usize;
        assert!(buf[i].abs() < buf[j].abs(), "the note should ramp, not jump");
    }

    #[test]
    fn stereo_duplicates_the_mono_cycle_without_changing_its_length_in_frames() {
        let mono = vec![0.5f32, -0.5, 0.25];
        assert_eq!(to_channels(&mono, 1), mono);
        assert_eq!(to_channels(&mono, 2), vec![0.5, 0.5, -0.5, -0.5, 0.25, 0.25]);
    }
}
