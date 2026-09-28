//! morse — Morse code as an input method, and as feedback you can feel.
//!
//! Two uses, one table:
//!
//! * **The Morse board** (`layout.experimental_mode = morse`): three keys.
//!   The CENTRE is the keyer — a tap is a dot, a hold is a dash — and its
//!   face shows the pattern so far and the letter it spells. A pause
//!   commits the letter; a longer pause, a space. The side keys are the two
//!   corrections a keyer needs most: LEFT is the pattern with a dash (a long
//!   hold) in FRONT of it, RIGHT is the pattern INVERTED (dots↔dashes). With
//!   nothing pending they rewrite the last letter instead. Big targets, few
//!   of them, no looking required: one thumb, or a foot pedal, or a switch.
//! * **Feel it while swiping** (`typing.morse_feel`): on any other board, a
//!   swiped word plays its Morse on the vibration motor, so the code is
//!   learned in passing — the path to typing hands-free.
//!
//! Pure: the keyer takes press durations and a clock and returns what to
//! type; `pattern_for` turns text into motor pulses for `haptics`.

use std::fmt;

/// (letter, code). Letters and digits, plus the punctuation a message needs.
const TABLE: &[(char, &str)] = &[
    ('a', ".-"), ('b', "-..."), ('c', "-.-."), ('d', "-.."), ('e', "."), ('f', "..-."),
    ('g', "--."), ('h', "...."), ('i', ".."), ('j', ".---"), ('k', "-.-"), ('l', ".-.."),
    ('m', "--"), ('n', "-."), ('o', "---"), ('p', ".--."), ('q', "--.-"), ('r', ".-."),
    ('s', "..."), ('t', "-"), ('u', "..-"), ('v', "...-"), ('w', ".--"), ('x', "-..-"),
    ('y', "-.--"), ('z', "--.."),
    ('1', ".----"), ('2', "..---"), ('3', "...--"), ('4', "....-"), ('5', "....."),
    ('6', "-...."), ('7', "--..."), ('8', "---.."), ('9', "----."), ('0', "-----"),
    ('.', ".-.-.-"), (',', "--..--"), ('?', "..--.."), ('\'', ".----."), ('!', "-.-.--"),
    ('/', "-..-."), ('(', "-.--."), (')', "-.--.-"), (':', "---..."), ('=', "-...-"),
    ('+', ".-.-."), ('-', "-....-"), ('"', ".-..-."), ('@', ".--.-."),
];

pub fn decode(code: &str) -> Option<char> {
    TABLE.iter().find(|(_, c)| *c == code).map(|(l, _)| *l)
}

pub fn encode(ch: char) -> Option<&'static str> {
    let ch = ch.to_ascii_lowercase();
    TABLE.iter().find(|(l, _)| *l == ch).map(|(_, c)| *c)
}

/// Dots ↔ dashes.
pub fn invert(code: &str) -> String {
    code.chars().map(|c| if c == '.' { '-' } else { '.' }).collect()
}

/// A dash (a long hold) in front.
pub fn dash_front(code: &str) -> String {
    format!("-{code}")
}

/// Pretty form for a key face: `·−·`.
pub struct Pretty<'a>(pub &'a str);
impl fmt::Display for Pretty<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            f.write_str(if c == '.' { "·" } else { "−" })?;
        }
        Ok(())
    }
}

/// Timing, in ms. Defaults are a comfortable ~12 wpm for a thumb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    /// A press at least this long is a dash.
    pub dash_ms: f64,
    /// Quiet this long after the last symbol commits the letter.
    pub letter_gap_ms: f64,
    /// Quiet this long after a letter adds a space.
    pub word_gap_ms: f64,
}

impl Timing {
    /// `typing.morse_speed`: slow / normal / fast.
    pub fn for_speed(speed: &str) -> Timing {
        match speed {
            "slow" => Timing { dash_ms: 300.0, letter_gap_ms: 900.0, word_gap_ms: 2200.0 },
            "fast" => Timing { dash_ms: 160.0, letter_gap_ms: 450.0, word_gap_ms: 1100.0 },
            _ => Timing::default(),
        }
    }

    /// The motor's dot length for feedback at this speed.
    pub fn unit_ms(&self) -> u32 {
        (self.dash_ms * 0.27).round() as u32
    }
}

impl Default for Timing {
    fn default() -> Self {
        Timing { dash_ms: 220.0, letter_gap_ms: 650.0, word_gap_ms: 1600.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Type this.
    Text(String),
    /// Delete the last letter first (a side key rewriting it).
    Replace(String),
    /// A pattern that spells nothing: shown, and the buzz says "no".
    Unknown(String),
}

#[derive(Clone, Debug, Default)]
pub struct Keyer {
    pub timing: Timing,
    /// Dots and dashes entered for the letter in progress.
    pending: String,
    /// When the last symbol or letter happened.
    last_ms: Option<f64>,
    /// The code of the last committed letter (for the side keys).
    last_code: Option<String>,
    /// A space is owed after the last letter, once the word gap passes.
    space_owed: bool,
}

impl Keyer {
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// A keyer press of `held_ms` ended at `now_ms`.
    pub fn press(&mut self, held_ms: f64, now_ms: f64) {
        self.pending.push(if held_ms >= self.timing.dash_ms { '-' } else { '.' });
        self.last_ms = Some(now_ms);
    }

    /// Call every frame: commits the letter / space when the gaps pass.
    pub fn tick(&mut self, now_ms: f64) -> Option<Out> {
        let quiet = now_ms - self.last_ms?;
        if !self.pending.is_empty() && quiet >= self.timing.letter_gap_ms {
            let code = std::mem::take(&mut self.pending);
            return Some(self.commit(code, now_ms, false));
        }
        if self.pending.is_empty() && self.space_owed && quiet >= self.timing.word_gap_ms {
            self.space_owed = false;
            self.last_code = None;
            return Some(Out::Text(" ".into()));
        }
        None
    }

    /// Whether frames are needed for `tick` to fire on time.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty() || self.space_owed
    }

    fn commit(&mut self, code: String, now_ms: f64, replace: bool) -> Out {
        self.last_ms = Some(now_ms);
        match decode(&code) {
            Some(ch) => {
                self.last_code = Some(code);
                self.space_owed = true;
                if replace { Out::Replace(ch.to_string()) } else { Out::Text(ch.to_string()) }
            }
            None => Out::Unknown(code),
        }
    }

    /// The code a side key works on: the pending pattern, or else the last
    /// letter's (to rewrite it).
    fn subject(&self) -> Option<(String, bool)> {
        if !self.pending.is_empty() {
            Some((self.pending.clone(), false))
        } else {
            self.last_code.clone().map(|c| (c, true))
        }
    }

    /// What LEFT would give: (code, letter) — for its key face.
    pub fn preview_dash_front(&self) -> Option<(String, Option<char>)> {
        let (c, _) = self.subject()?;
        let d = dash_front(&c);
        let l = decode(&d);
        Some((d, l))
    }

    /// What RIGHT would give.
    pub fn preview_invert(&self) -> Option<(String, Option<char>)> {
        let (c, _) = self.subject()?;
        let i = invert(&c);
        let l = decode(&i);
        Some((i, l))
    }

    pub fn dash_front(&mut self, now_ms: f64) -> Option<Out> {
        let (c, replace) = self.subject()?;
        self.pending.clear();
        Some(self.commit(dash_front(&c), now_ms, replace))
    }

    pub fn invert(&mut self, now_ms: f64) -> Option<Out> {
        let (c, replace) = self.subject()?;
        self.pending.clear();
        Some(self.commit(invert(&c), now_ms, replace))
    }

    /// Another key is about to type (space, ⏎, a symbol): the pending
    /// letter commits NOW rather than after the gap, and the side keys stop
    /// rewriting the last letter — it is no longer the last thing typed.
    pub fn flush(&mut self, now_ms: f64) -> Option<Out> {
        let out = if self.pending.is_empty() {
            None
        } else {
            let code = std::mem::take(&mut self.pending);
            Some(self.commit(code, now_ms, false))
        };
        self.space_owed = false;
        self.last_code = None;
        out
    }

    /// Backspace on the Morse board: drop the last symbol if one is
    /// pending; otherwise the caller deletes a character.
    pub fn backspace(&mut self) -> bool {
        self.last_code = None;
        self.space_owed = false;
        self.pending.pop().is_some()
    }
}

/// Motor pulses spelling `text` in Morse at `unit_ms` per dot: dot = 1
/// unit on, dash = 3, 1 unit between symbols, 3 between letters, 7 at a
/// space. Characters with no code are skipped.
pub fn pattern_for(text: &str, unit_ms: u32) -> haptic_kit::Pattern {
    let mut pulses: Vec<haptic_kit::Pulse> = Vec::new();
    for ch in text.chars() {
        if ch == ' ' {
            if let Some(last) = pulses.last_mut() {
                last.off_ms = 7 * unit_ms;
            }
            continue;
        }
        let Some(code) = encode(ch) else { continue };
        for s in code.chars() {
            let on = if s == '.' { unit_ms } else { 3 * unit_ms };
            pulses.push(haptic_kit::Pulse { on_ms: on, off_ms: unit_ms });
        }
        if let Some(last) = pulses.last_mut() {
            last.off_ms = 3 * unit_ms;
        }
    }
    if let Some(last) = pulses.last_mut() {
        last.off_ms = 0;
    }
    haptic_kit::Pattern { pulses }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_a_bijection() {
        for (l, c) in TABLE {
            assert_eq!(decode(c), Some(*l), "{c}");
            assert_eq!(encode(*l), Some(*c));
            assert_eq!(TABLE.iter().filter(|(_, x)| x == c).count(), 1, "{c} is used once");
        }
        assert_eq!(encode('S'), Some("..."), "case-insensitive");
    }

    fn key(k: &mut Keyer, ms: &[f64], t0: f64) -> f64 {
        let mut t = t0;
        for m in ms {
            t += m;
            k.press(*m, t);
            t += 100.0;
        }
        t
    }

    #[test]
    fn taps_and_holds_spell_a_letter_after_the_gap_and_a_space_after_more() {
        let mut k = Keyer::default();
        let t = key(&mut k, &[80.0, 400.0, 80.0], 0.0); // . - .
        assert_eq!(k.pending(), ".-.");
        assert_eq!(k.tick(t + 100.0), None, "still inside the letter gap");
        assert_eq!(k.tick(t + 700.0), Some(Out::Text("r".into())));
        assert_eq!(k.tick(t + 1000.0), None);
        assert_eq!(k.tick(t + 2400.0), Some(Out::Text(" ".into())), "the word gap");
        assert_eq!(k.tick(t + 5000.0), None, "one space, not one per frame");
    }

    #[test]
    fn left_puts_a_dash_in_front_and_right_inverts() {
        let mut k = Keyer::default();
        let t = key(&mut k, &[80.0, 80.0], 0.0); // ..  = i
        assert_eq!(k.preview_dash_front(), Some(("-..".into(), Some('d'))));
        assert_eq!(k.preview_invert(), Some(("--".into(), Some('m'))));
        assert_eq!(k.invert(t), Some(Out::Text("m".into())));
        // With nothing pending the side keys REWRITE the last letter.
        assert_eq!(k.dash_front(t + 10.0), Some(Out::Replace("o".into())), "-- with a dash in front is ---");
    }

    #[test]
    fn another_key_commits_the_pending_letter_at_once_and_owes_no_space() {
        let mut k = Keyer::default();
        let t = key(&mut k, &[80.0, 400.0], 0.0); // .- = a
        assert_eq!(k.flush(t), Some(Out::Text("a".into())));
        assert!(!k.waiting(), "the space was typed by hand; none is owed");
        assert_eq!(k.invert(t + 10.0), None, "the side keys no longer rewrite it");
        assert_eq!(k.flush(t + 20.0), None);
    }

    #[test]
    fn backspace_drops_a_symbol_before_it_deletes_a_letter() {
        let mut k = Keyer::default();
        key(&mut k, &[80.0, 80.0], 0.0);
        assert!(k.backspace());
        assert_eq!(k.pending(), ".");
        assert!(k.backspace());
        assert!(!k.backspace(), "nothing pending: the caller deletes a character");
    }

    #[test]
    fn a_pattern_that_spells_nothing_is_reported_not_typed() {
        let mut k = Keyer::default();
        let t = key(&mut k, &[400.0; 7], 0.0);
        assert_eq!(k.tick(t + 700.0), Some(Out::Unknown("-------".into())));
    }

    #[test]
    fn a_word_becomes_motor_pulses_with_morse_spacing() {
        let p = pattern_for("et", 60); // .   -
        assert_eq!(p.pulses.len(), 2);
        assert_eq!((p.pulses[0].on_ms, p.pulses[0].off_ms), (60, 180), "dot, letter gap");
        assert_eq!((p.pulses[1].on_ms, p.pulses[1].off_ms), (180, 0), "dash, end");
        let words = pattern_for("e e", 60);
        assert_eq!(words.pulses[0].off_ms, 420, "a space is seven units");
    }
}
