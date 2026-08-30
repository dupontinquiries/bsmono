//! playkeys — the keyboard as an instrument, without losing it as a keyboard.
//!
//! Two modes, because one keyboard has two jobs: `Motions` is the Helix-style
//! editing keymap ([`crate::keys`]), and `Playing` turns the same keys into a
//! two-octave piano. The switch is explicit and visible — a modal editor that
//! silently eats `d` because you were noodling is worse than either mode
//! alone.
//!
//! ## Locale adaptive means PHYSICAL, not logical
//!
//! The note layout is built on physical key POSITIONS, not on the letters
//! printed on them. On AZERTY the key where QWERTY's `q` sits reports the
//! logical char `a`; a layout keyed on characters would scatter that
//! player's C-major across the board. egui carries `physical_key` alongside
//! the logical one for exactly this, so [`note_for`] takes the physical key
//! and every layout on earth gets the same shape under the fingers. When a
//! platform reports no physical key (some web backends), the logical key is
//! the fallback: a QWERTY-shaped guess beats no notes at all.
//!
//! ## The layout
//!
//! The tracker/DAW convention, one octave per hand-row pair:
//!
//! ```text
//!   W E   T Y U        <- black keys (C# D#   F# G# A#)
//!  A S D F G H J K     <- white keys (C D E F G A B C)
//! ```
//!
//! and the row below it repeats an octave down (`Z X C V B N M`), so a bass
//! line and its melody are reachable without touching the octave control.
//!
//! ## Velocity
//!
//! An analog keyboard reports how FAR a key is pressed; that is the whole
//! reason to want one for playing synths. [`AnalogSource`] is the seam:
//! `NullAnalog` (any ordinary keyboard) reports full velocity for a held
//! key, and a Wooting implementation reports travel. The reference binding
//! is `games/skydribble/src/analog.rs` — it dlopens the Wooting Analog SDK
//! behind a cargo feature and no-ops when the runtime is absent, which is
//! the right shape here too; lifting it into a shared crate (rather than
//! copying 200 lines of FFI that no test in this repo can exercise without
//! the hardware) is the honest next step, and is named in the README.

use crate::keys::Key;

/// What the keyboard currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Helix motions and edits — the editor keymap.
    #[default]
    Motions,
    /// A piano. Editing keys are inert here, by design.
    Playing,
}

impl InputMode {
    pub fn label(self) -> &'static str {
        match self { InputMode::Motions => "motions", InputMode::Playing => "playing" }
    }
    pub fn toggled(self) -> InputMode {
        match self { InputMode::Motions => InputMode::Playing, InputMode::Playing => InputMode::Motions }
    }
}

/// A note the player asked for: semitones relative to the channel's own
/// tuning (0 = the channel as configured), plus how hard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayNote {
    pub semitone: i8,
    /// 0..=1. Analog travel where available, 1.0 on a digital keyboard.
    pub velocity: f32,
}

/// Where velocity comes from. Object-safe so the shell can hold a boxed one
/// and never know whether analog hardware exists.
pub trait AnalogSource {
    /// How far `key` is pressed, 0..=1. `held` is the digital state, so an
    /// implementation with nothing to say can pass it through.
    fn travel(&self, key: &Key, held: bool) -> f32;
    fn active(&self) -> bool { false }
}

/// Every ordinary keyboard: a held key is a full-velocity key.
pub struct NullAnalog;
impl AnalogSource for NullAnalog {
    fn travel(&self, _key: &Key, held: bool) -> f32 { if held { 1.0 } else { 0.0 } }
}

/// The white keys of the upper row, in order, as PHYSICAL key characters
/// (named by their QWERTY position — the position is what matters).
const WHITE_UPPER: [(char, i8); 8] = [
    ('a', 0), ('s', 2), ('d', 4), ('f', 5), ('g', 7), ('h', 9), ('j', 11), ('k', 12),
];
/// Black keys sit between them, on the row above.
const BLACK_UPPER: [(char, i8); 5] = [('w', 1), ('e', 3), ('t', 6), ('y', 8), ('u', 10)];
/// The lower row is the same shape an octave down.
const WHITE_LOWER: [(char, i8); 7] = [
    ('z', -12), ('x', -10), ('c', -8), ('v', -7), ('b', -5), ('n', -3), ('m', -1),
];

/// The semitone a physical key plays, if any. `octave` shifts by 12s.
///
/// Pure and position-based: see the module docs on why the PHYSICAL key is
/// the argument. Returns `None` for keys that are not part of the layout, so
/// a caller can leave them alone rather than swallowing every keystroke.
pub fn note_for(physical: char, octave: i8) -> Option<i8> {
    let base = WHITE_UPPER.iter()
        .chain(BLACK_UPPER.iter())
        .chain(WHITE_LOWER.iter())
        .find(|(c, _)| *c == physical.to_ascii_lowercase())
        .map(|(_, s)| *s)?;
    Some((base as i16 + octave as i16 * 12).clamp(-24, 24) as i8)
}

/// One key press in `Playing` mode, resolved to a note with velocity.
/// `zones` are the device's calibrated regions; empty means the tracker
/// layout alone, exactly as before calibration existed.
pub fn play_note(
    physical: char,
    octave: i8,
    key: &Key,
    analog: &dyn AnalogSource,
    zones: &Zones,
) -> Option<PlayNote> {
    let semitone = note_for_zoned(zones, physical, octave)?;
    // A note with zero velocity is not a note; a digital keyboard's held key
    // is 1.0, so this only filters analog keys resting above the actuation
    // point the SDK reports.
    let velocity = analog.travel(key, true).clamp(0.0, 1.0);
    (velocity > 0.0).then_some(PlayNote { semitone, velocity })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HalfPressed;
    impl AnalogSource for HalfPressed {
        fn travel(&self, _k: &Key, _held: bool) -> f32 { 0.5 }
        fn active(&self) -> bool { true }
    }

    #[test]
    fn the_layout_is_a_piano_and_the_octave_control_moves_it() {
        assert_eq!(note_for('a', 0), Some(0), "home row starts at the root");
        assert_eq!(note_for('w', 0), Some(1), "the key above and right is the sharp");
        assert_eq!(note_for('k', 0), Some(12), "the row ends an octave up");
        assert_eq!(note_for('z', 0), Some(-12), "the lower row is an octave down");
        assert_eq!(note_for('a', 1), Some(12), "octave up shifts the whole board");
        assert_eq!(note_for('a', -1), Some(-12));
        // Non-layout keys stay the caller's business, not swallowed.
        assert_eq!(note_for('1', 0), None);
        assert_eq!(note_for('p', 0), None);
    }

    #[test]
    fn the_octave_control_cannot_walk_off_the_end() {
        // i8 semitones and StepMod::tune's range are finite; clamping here
        // beats a wrapped note playing a wrong pitch at the far edge.
        assert_eq!(note_for('k', 8), Some(24));
        assert_eq!(note_for('z', -8), Some(-24));
    }

    #[test]
    fn case_does_not_change_the_note() {
        // Shift is a modifier a player will hit by accident; it must not
        // transpose or mute the note.
        assert_eq!(note_for('A', 0), note_for('a', 0));
    }

    #[test]
    fn an_analog_keyboard_supplies_velocity_and_a_plain_one_supplies_full() {
        let k = Key::c('a');
        let plain = play_note('a', 0, &k, &NullAnalog, &Zones::default()).expect("a note");
        assert_eq!(plain.velocity, 1.0, "a digital press is a full press");
        let analog = play_note('a', 0, &k, &HalfPressed, &Zones::default()).expect("a note");
        assert_eq!(analog.velocity, 0.5, "travel becomes velocity");
        assert_eq!(analog.semitone, plain.semitone, "same key, same pitch");
    }

    #[test]
    fn modes_toggle_and_say_which_they_are() {
        let m = InputMode::default();
        assert_eq!(m, InputMode::Motions, "the editor is the default; playing is opt-in");
        assert_eq!(m.toggled(), InputMode::Playing);
        assert_eq!(m.toggled().toggled(), m);
        assert_eq!(InputMode::Playing.label(), "playing");
    }
}

// ── calibrated zones: YOUR keyboard's shape, not the tracker's ─────────────

/// Per-device physical-key → semitone zones, layered over the built-in
/// tracker layout. The built-in rows are already physical-position based
/// (Dvorak/AZERTY safe); zones exist for the step beyond that — a hand
/// that wants piano-like spacing, or SEVERAL adjacent physical keys per
/// piano key (wide regions, so a fingertip landing between two keycaps
/// still plays the note it aimed at). Calibrated once, saved in the
/// per-device sidecar like every other "this hardware, this hand" fact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Zones {
    /// physical key char → semitone offset from the base C. Many keys may
    /// map to ONE note — that is the region idea, not a conflict.
    map: std::collections::BTreeMap<char, i8>,
}

impl Zones {
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
    pub fn assign(&mut self, physical: char, note: i8) { self.map.insert(physical, note); }
    pub fn resolve(&self, physical: char) -> Option<i8> { self.map.get(&physical).copied() }
    /// Sidecar form: codepoint>note pairs — the same reasoning as key
    /// rebinds, because the keys people reach for include punctuation.
    pub fn encode(&self) -> String {
        self.map.iter().map(|(k, v)| format!("{}>{v}", *k as u32))
            .collect::<Vec<_>>().join(",")
    }
    pub fn decode(s: &str) -> Zones {
        let mut z = Zones::default();
        for pair in s.split(',') {
            let Some((a, b)) = pair.split_once('>') else { continue };
            let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<i8>())
                else { continue };
            if let Some(a) = char::from_u32(a) { z.assign(a, b); }
        }
        z
    }
}

/// The one-time walk: for each semitone from C upward, press every
/// physical key that should mean it (one, or a whole region), then
/// advance. The state is just "which note are we on" — the UI shows
/// [`Calibration::note_name`], the typed keys land here, Enter advances.
#[derive(Debug, Clone, Default)]
pub struct Calibration {
    pub zones: Zones,
    pub note: i8,
}

impl Calibration {
    pub fn press(&mut self, physical: char) { self.zones.assign(physical, self.note); }
    pub fn advance(&mut self) { self.note = self.note.saturating_add(1); }
    pub fn note_name(&self) -> String {
        const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F",
                               "F#", "G", "G#", "A", "A#", "B"];
        format!("{}{}", N[self.note.rem_euclid(12) as usize], self.note.div_euclid(12) + 1)
    }
}

/// The note a physical key means, zones first, tracker layout as the
/// fallback — so a half-finished calibration still plays: calibrated keys
/// answer from the zones, everything else answers as before.
pub fn note_for_zoned(zones: &Zones, physical: char, octave: i8) -> Option<i8> {
    match zones.resolve(physical) {
        Some(n) => Some(n.saturating_add(octave.saturating_mul(12))),
        None => note_for(physical, octave),
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    /// The region idea: several physical keys, one piano key — and the
    /// tracker layout keeps answering for everything uncalibrated.
    #[test]
    fn regions_map_many_keys_to_one_note_and_fall_back_elsewhere() {
        let mut cal = Calibration::default();
        assert_eq!(cal.note_name(), "C1");
        // Two adjacent keys both mean C — the fingertip-between-keycaps case.
        cal.press('a');
        cal.press('s');
        cal.advance();
        assert_eq!(cal.note_name(), "C#1");
        cal.press('w');
        let z = cal.zones;
        assert_eq!(z.resolve('a'), Some(0));
        assert_eq!(z.resolve('s'), Some(0), "same note, second key of the region");
        assert_eq!(z.resolve('w'), Some(1));
        assert_eq!(note_for_zoned(&z, 'a', 0), Some(0));
        assert_eq!(note_for_zoned(&z, 'a', 1), Some(12), "octave shifts zones too");
        // 'h' was never calibrated: the tracker layout answers.
        assert_eq!(note_for_zoned(&z, 'h', 0), note_for('h', 0));
        assert!(note_for_zoned(&z, 'h', 0).is_some(),
                "the fallback is the layout, not silence");
    }

    /// Sidecar round trip, punctuation included — the same keys that
    /// forced rebinds into codepoints.
    #[test]
    fn zones_survive_the_sidecar() {
        let mut z = Zones::default();
        z.assign(',', 3);
        z.assign('a', 0);
        z.assign(';', -2);
        assert_eq!(Zones::decode(&z.encode()), z);
        assert_eq!(Zones::decode("garbage,44>3,x>y,>"),
                   { let mut w = Zones::default(); w.assign(',', 3); w },
                   "each bad pair is skipped alone");
        assert!(Zones::decode("").is_empty());
    }
}

