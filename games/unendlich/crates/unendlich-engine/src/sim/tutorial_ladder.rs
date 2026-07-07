//! `sim::tutorial_ladder` — the practice-world training ladder: the seven
//! per-element "seven beat" tutorial sequences, Choir harmonisation, and the
//! partial-coupling carry-forward, as a pure, runnable state machine.
//!
//! Beyond onboarding, this doubles as a **debug suite for the bending system**:
//! each element's ladder walks the same seven beats — Arrival, Sense, Voice
//! shape, First technique, Choir note, **Harmonisation**, What-comes-next — and
//! the harmonisation beat exercises the real frequency-match mechanic (match the
//! mentor's fundamental within ±15 cents, sustain 3 s → partial coupling +5%,
//! tutorial cap 15%). Scripting a run through the ladder ([`run_element`]) is a
//! deterministic way to check that the bending pitch/coupling rules behave, which
//! is otherwise hard to poke at by hand.
//!
//! Design rules from the spec that this encodes:
//! * **No beat is gated on perfection** — [`LadderState::advance`] always moves to
//!   the next beat; failing a harmonisation just means no coupling, not a block.
//! * **Any element order; none gated on another.**
//! * **Choir unlocks after any one element is completed.**
//! * Coupling earned here is the carry-forward value (0..=15%) for the main world.
//!
//! Pure/std-only `f32` math. Frequencies are the mentor fundamentals from §7.2.

/// The seven practice-world elements, each with a mentor and a Choir fundamental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Earth,
    Water,
    Fire,
    Air,
    Storm,
    Brine,
    Information,
}

impl Element {
    pub const ALL: [Element; 7] = [
        Element::Earth,
        Element::Water,
        Element::Fire,
        Element::Air,
        Element::Storm,
        Element::Brine,
        Element::Information,
    ];

    /// The mentor's demonstrated Choir fundamental (Hz) — the harmonisation target.
    pub fn fundamental_hz(self) -> f32 {
        match self {
            Element::Earth => 220.0,       // Miren's Earthstone (octave of the 110 Hz geological bass)
            Element::Water => 293.0,
            Element::Fire => 440.0,        // Daro: "the most honest frequency"
            Element::Air => 528.0,         // bright, open throat, upper chest
            Element::Storm => 174.0,       // Sable: the electrical-hum register
            Element::Brine => 396.0,
            Element::Information => 741.0, // the Scholar's mid-range teal tone
        }
    }

    /// The element's first taught voice shape (spoken/tokened).
    pub fn voice_shape(self) -> &'static str {
        match self {
            Element::Earth => "RISE",
            Element::Water => "SURGE",
            Element::Fire => "EXHALE",
            Element::Air => "GUST",
            Element::Storm => "ARC",
            Element::Brine => "SUSPEND",
            Element::Information => "ECHO",
        }
    }

    /// The first technique taught in the element's ladder.
    pub fn first_technique(self) -> &'static str {
        match self {
            Element::Earth => "Pillar Rise",
            Element::Water => "Tidal Surge",
            Element::Fire => "Directed Exhale",
            Element::Air => "Pressure Step",
            Element::Storm => "Arc Path",
            Element::Brine => "Quiet Lift",
            Element::Information => "Temporal Echo Read",
        }
    }

    /// The mentor NPC stationed in the element's zone.
    pub fn mentor(self) -> &'static str {
        match self {
            Element::Earth => "Miren the Lapidary",
            Element::Water => "Lienne of the Brine Shore",
            Element::Fire => "Daro the Forgemaster",
            Element::Air => "Verath",
            Element::Storm => "Sable",
            Element::Brine => "Carrier Designation 7",
            Element::Information => "The Scholar",
        }
    }
}

/// The seven beats every element's tutorial follows, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    Arrival,
    Sense,
    VoiceShape,
    Technique,
    ChoirNote,
    Harmonisation,
    WhatComesNext,
}

impl Beat {
    pub const ALL: [Beat; 7] = [
        Beat::Arrival,
        Beat::Sense,
        Beat::VoiceShape,
        Beat::Technique,
        Beat::ChoirNote,
        Beat::Harmonisation,
        Beat::WhatComesNext,
    ];

    pub fn index(self) -> usize {
        Beat::ALL.iter().position(|&b| b == self).unwrap()
    }

    /// The Choir display is only instructive on beats 5 and 6.
    pub fn uses_choir(self) -> bool {
        matches!(self, Beat::ChoirNote | Beat::Harmonisation)
    }
}

// --- harmonisation constants (§7) ---
/// Match window: within this many cents of the fundamental the indicator appears.
pub const CENTS_TOLERANCE: f32 = 15.0;
/// Seconds the match must be sustained for harmonisation to trigger.
pub const SUSTAIN_REQUIRED: f32 = 3.0;
/// Coupling gained per harmonisation (bands 1–4).
pub const COUPLING_PER_HARMONISE: f32 = 0.05;
/// Maximum coupling reachable without main-world practice (tutorial cap).
pub const COUPLING_CAP: f32 = 0.15;

/// Pitch difference in cents: `1200 · log2(f / f_ref)`. Positive = sharp.
pub fn cents_off(freq: f32, f_ref: f32) -> f32 {
    if freq <= 0.0 || f_ref <= 0.0 {
        return f32::INFINITY;
    }
    1200.0 * (freq / f_ref).log2()
}

/// Whether a sung `freq` held for `sustain_s` seconds harmonises with `f_ref`:
/// within ±[`CENTS_TOLERANCE`] and sustained ≥ [`SUSTAIN_REQUIRED`].
pub fn is_harmonised(freq: f32, f_ref: f32, sustain_s: f32) -> bool {
    cents_off(freq, f_ref).abs() <= CENTS_TOLERANCE && sustain_s >= SUSTAIN_REQUIRED
}

/// Per-element ladder progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementProgress {
    /// Index into [`Beat::ALL`] of the *current* beat; `7` once finished.
    pub beat: usize,
    /// Partial coupling earned so far, `0.0..=COUPLING_CAP`.
    pub coupling: f32,
    /// How many times harmonisation has triggered here.
    pub harmonised: u32,
}

impl Default for ElementProgress {
    fn default() -> Self {
        ElementProgress { beat: 0, coupling: 0.0, harmonised: 0 }
    }
}

impl ElementProgress {
    pub fn current_beat(&self) -> Option<Beat> {
        Beat::ALL.get(self.beat).copied()
    }
    pub fn complete(&self) -> bool {
        self.beat >= Beat::ALL.len()
    }
}

/// The whole practice-world progress across all seven elements.
#[derive(Debug, Clone, Default)]
pub struct LadderState {
    progress: [ElementProgress; 7],
}

fn slot(e: Element) -> usize {
    Element::ALL.iter().position(|&x| x == e).unwrap()
}

/// Outcome of a harmonisation attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Harmonisation {
    pub triggered: bool,
    pub cents_off: f32,
    /// Coupling after this attempt.
    pub coupling: f32,
}

impl LadderState {
    pub fn new() -> Self {
        LadderState::default()
    }

    pub fn progress(&self, e: Element) -> ElementProgress {
        self.progress[slot(e)]
    }

    pub fn current_beat(&self, e: Element) -> Option<Beat> {
        self.progress[slot(e)].current_beat()
    }

    /// Advance to the next beat. Never gated on perfection — always moves forward
    /// (up to completion). Returns the new current beat, or `None` when finished.
    pub fn advance(&mut self, e: Element) -> Option<Beat> {
        let p = &mut self.progress[slot(e)];
        if p.beat < Beat::ALL.len() {
            p.beat += 1;
        }
        p.current_beat()
    }

    /// Attempt a harmonisation with a sung `freq` sustained `sustain_s` seconds.
    /// Only the harmonisation-capable beats grant coupling; on success coupling
    /// rises by [`COUPLING_PER_HARMONISE`] up to the [`COUPLING_CAP`]. Failing does
    /// not block progress (the caller may still [`advance`](Self::advance)).
    pub fn attempt_harmonise(&mut self, e: Element, freq: f32, sustain_s: f32) -> Harmonisation {
        let f_ref = e.fundamental_hz();
        let cents = cents_off(freq, f_ref);
        let p = &mut self.progress[slot(e)];
        let on_choir_beat = p.current_beat().map_or(false, |b| b.uses_choir());
        let triggered = on_choir_beat && is_harmonised(freq, f_ref, sustain_s);
        if triggered {
            p.coupling = (p.coupling + COUPLING_PER_HARMONISE).min(COUPLING_CAP);
            p.harmonised += 1;
        }
        Harmonisation { triggered, cents_off: cents, coupling: p.coupling }
    }

    pub fn element_complete(&self, e: Element) -> bool {
        self.progress[slot(e)].complete()
    }

    pub fn completed_count(&self) -> usize {
        self.progress.iter().filter(|p| p.complete()).count()
    }

    /// The Chorus Guide (Choir tutorial) unlocks once any one element is done.
    pub fn choir_unlocked(&self) -> bool {
        self.completed_count() >= 1
    }

    /// Coupling carried into the main world for an element (0..=15%).
    pub fn carry_forward_coupling(&self, e: Element) -> f32 {
        self.progress[slot(e)].coupling
    }
}

/// Report from a scripted ladder run — the debug-suite output.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementRun {
    pub element: Element,
    /// Beats visited, in order.
    pub beats: Vec<Beat>,
    /// Whether harmonisation triggered on each Choir beat visited.
    pub harmonisations: Vec<bool>,
    pub final_coupling: f32,
    pub completed: bool,
}

/// Run an element's full seven-beat ladder deterministically, attempting a
/// harmonisation with `(sing_hz, sing_sustain)` on each Choir beat. This is the
/// debug harness: feed a pitch/sustain and see whether the bending harmonisation
/// mechanic accepts it and how coupling accrues. `sing_sustain` in seconds.
pub fn run_element(state: &mut LadderState, element: Element, sing_hz: f32, sing_sustain: f32) -> ElementRun {
    let mut beats = Vec::new();
    let mut harmonisations = Vec::new();
    // walk from the current beat to completion
    while let Some(beat) = state.current_beat(element) {
        beats.push(beat);
        if beat.uses_choir() {
            let h = state.attempt_harmonise(element, sing_hz, sing_sustain);
            harmonisations.push(h.triggered);
        }
        state.advance(element);
    }
    ElementRun {
        element,
        beats,
        harmonisations,
        final_coupling: state.carry_forward_coupling(element),
        completed: state.element_complete(element),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn cents_math_anchors() {
        assert!(approx(cents_off(440.0, 440.0), 0.0, 1e-4), "same pitch = 0 cents");
        assert!(approx(cents_off(880.0, 440.0), 1200.0, 1e-3), "octave = 1200 cents");
        assert!(cents_off(444.0, 440.0) > 0.0, "sharp is positive");
        // ~15.6 cents for a 4 Hz error at 440 — just outside tolerance
        assert!(cents_off(444.0, 440.0) > CENTS_TOLERANCE);
    }

    #[test]
    fn harmonisation_needs_pitch_and_sustain() {
        let f = Element::Fire.fundamental_hz(); // 440
        assert!(is_harmonised(440.0, f, 3.0), "on pitch, sustained → yes");
        assert!(!is_harmonised(440.0, f, 2.0), "not sustained long enough → no");
        assert!(!is_harmonised(460.0, f, 5.0), "off pitch → no");
        // within ±15 cents (≈ 441.9 Hz is ~7.5 cents) still counts
        assert!(is_harmonised(441.9, f, 3.0), "small detune inside window → yes");
    }

    #[test]
    fn advancing_is_never_gated_on_perfection() {
        // walk all seven beats with NO harmonisation at all — still completes.
        let mut s = LadderState::new();
        assert_eq!(s.current_beat(Element::Earth), Some(Beat::Arrival));
        for _ in 0..7 {
            s.advance(Element::Earth);
        }
        assert!(s.element_complete(Element::Earth), "completes without harmonising");
        assert_eq!(s.carry_forward_coupling(Element::Earth), 0.0, "no coupling earned, but not blocked");
        assert_eq!(s.current_beat(Element::Earth), None);
    }

    #[test]
    fn coupling_accrues_and_caps_at_15_percent() {
        let mut s = LadderState::new();
        // jump to the Choir note beat where harmonisation is valid
        while s.current_beat(Element::Earth) != Some(Beat::ChoirNote) {
            s.advance(Element::Earth);
        }
        let f = Element::Earth.fundamental_hz();
        // harmonise repeatedly on the (Choir) beat — coupling rises 5% each, caps 15%
        for i in 1..=5 {
            let h = s.attempt_harmonise(Element::Earth, f, SUSTAIN_REQUIRED);
            assert!(h.triggered, "on-pitch harmonisation triggers");
            let expected = (COUPLING_PER_HARMONISE * i as f32).min(COUPLING_CAP);
            assert!(approx(h.coupling, expected, 1e-6), "coupling {} vs {}", h.coupling, expected);
        }
        assert!(approx(s.carry_forward_coupling(Element::Earth), COUPLING_CAP, 1e-6), "capped at 15%");
    }

    #[test]
    fn harmonisation_only_on_choir_beats() {
        let mut s = LadderState::new();
        // on Arrival (not a Choir beat) an on-pitch attempt earns nothing
        let f = Element::Earth.fundamental_hz();
        let h = s.attempt_harmonise(Element::Earth, f, SUSTAIN_REQUIRED);
        assert!(!h.triggered, "no harmonisation off the Choir beats");
        assert_eq!(h.coupling, 0.0);
    }

    #[test]
    fn choir_unlocks_after_first_element() {
        let mut s = LadderState::new();
        assert!(!s.choir_unlocked(), "locked at start");
        for _ in 0..7 {
            s.advance(Element::Storm);
        }
        assert!(s.element_complete(Element::Storm));
        assert!(s.choir_unlocked(), "any one element done → Choir guide available");
    }

    #[test]
    fn run_element_is_a_clean_debug_pass() {
        // a perfect run of the Information ladder, singing its 741 Hz on pitch
        let mut s = LadderState::new();
        let run = run_element(&mut s, Element::Information, 741.0, SUSTAIN_REQUIRED);
        assert_eq!(run.beats, Beat::ALL.to_vec(), "visited all seven beats in order");
        // two Choir beats (ChoirNote + Harmonisation) → two triggers
        assert_eq!(run.harmonisations, vec![true, true]);
        assert!(approx(run.final_coupling, 2.0 * COUPLING_PER_HARMONISE, 1e-6), "two harmonisations → 10%");
        assert!(run.completed);

        // a "player who can't hold the note" run still completes, with no coupling
        let mut s2 = LadderState::new();
        let bad = run_element(&mut s2, Element::Information, 741.0, 1.0 /* too short */);
        assert_eq!(bad.harmonisations, vec![false, false]);
        assert_eq!(bad.final_coupling, 0.0);
        assert!(bad.completed, "failure never blocks the ladder");
    }

    #[test]
    fn every_element_has_distinct_mentor_and_fundamental() {
        // sanity for the debug suite: no two elements collide on the target pitch
        for (i, a) in Element::ALL.iter().enumerate() {
            for b in &Element::ALL[i + 1..] {
                assert!(a.fundamental_hz() != b.fundamental_hz(), "{a:?}/{b:?} share a pitch");
                assert!(a.mentor() != b.mentor());
                assert!(a.voice_shape() != b.voice_shape());
            }
        }
    }

    #[test]
    fn elements_are_independent() {
        // completing one element leaves the others untouched (any order allowed)
        let mut s = LadderState::new();
        run_element(&mut s, Element::Earth, 220.0, SUSTAIN_REQUIRED);
        assert!(s.element_complete(Element::Earth));
        assert!(!s.element_complete(Element::Water));
        assert_eq!(s.current_beat(Element::Water), Some(Beat::Arrival));
        assert_eq!(s.completed_count(), 1);
    }
}
