//! What the volume keys do beyond volume.
//!
//! A phone has two buttons that always work: they need no screen, no aim and
//! no warm fingers. That makes them the right controls for exactly the
//! situations where the touchscreen is not — gloves, rain, cold, a screen
//! that is off or wet — and the wrong place to put anything a tap already
//! does well.
//!
//! Three behaviours, chosen by what the shell is doing rather than by a
//! setting, because a setting would mean remembering which mode you are in
//! while wearing gloves:
//!
//! * **Cold or glove mode:** the keys STEER. Up and down move a selection
//!   through whatever is on screen and a hold on either activates. Volume
//!   is given up entirely while this is true — a touchscreen that is not
//!   answering is a worse problem than not being able to change the volume,
//!   and a modifier chord would need two hands.
//! * **Otherwise, a hold** runs whatever the user bound to it (a camera, a
//!   torch, a note). A press stays volume, because that is what a volume key
//!   is for and surprising someone reaching for volume is unforgivable.
//! * **Both keys together** is always the same thing regardless of mode, so
//!   there is one gesture that behaves identically everywhere.
//!
//! Pure: presses and releases in, intents out, `now_ms` supplied.

use crate::Millis;

/// How long a key must be held before it stops being a press.
pub const HOLD_MS: Millis = 500;
/// Repeat interval while steering with a held key.
pub const REPEAT_MS: Millis = 220;
/// Both keys count as together if the second arrives within this.
pub const CHORD_MS: Millis = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VolKey {
    Up,
    Down,
}

/// What the shell should do about a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Ordinary volume, one step.
    Volume(VolKey),
    /// Move the steering selection.
    Step(VolKey),
    /// Activate what is selected.
    Activate,
    /// Run the user's bound action for a long press.
    Bound(VolKey),
    /// Both keys at once.
    Chord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    key: VolKey,
    since: Millis,
    /// Already reported as a hold, so the release is not also a press.
    fired: bool,
    last_repeat: Millis,
}

#[derive(Debug, Clone, Default)]
pub struct VolKeys {
    /// Steering mode: cold, gloves, or anything else that makes the
    /// touchscreen unreliable.
    pub steering: bool,
    held: Option<Held>,
    /// The other key went down while one was already held.
    chorded: bool,
}

impl VolKeys {
    pub fn press(&mut self, key: VolKey, now: Millis) -> Vec<Intent> {
        if let Some(h) = self.held {
            // Two keys down: one gesture, and neither key's own behaviour
            // fires. Reported once, on the second press, so it does not wait
            // for a release that may never come cleanly.
            if h.key != key && now.saturating_sub(h.since) <= CHORD_MS.max(HOLD_MS) {
                self.chorded = true;
                return vec![Intent::Chord];
            }
        }
        self.held = Some(Held { key, since: now, fired: false, last_repeat: now });
        vec![]
    }

    /// Call every frame: holds and repeats are time, not events.
    pub fn tick(&mut self, now: Millis) -> Vec<Intent> {
        let Some(h) = &mut self.held else { return vec![] };
        if self.chorded {
            return vec![];
        }
        let down_for = now.saturating_sub(h.since);
        if down_for < HOLD_MS {
            return vec![];
        }
        if !h.fired {
            h.fired = true;
            h.last_repeat = now;
            return vec![if self.steering { Intent::Step(h.key) } else { Intent::Bound(h.key) }];
        }
        // Only steering repeats. A bound action firing over and over because
        // a button is still down would be a disaster.
        if self.steering && now.saturating_sub(h.last_repeat) >= REPEAT_MS {
            h.last_repeat = now;
            return vec![Intent::Step(h.key)];
        }
        vec![]
    }

    pub fn release(&mut self, key: VolKey, now: Millis) -> Vec<Intent> {
        let Some(h) = self.held else {
            self.chorded = false;
            return vec![];
        };
        if self.chorded {
            // Wait for both to come up before arming again.
            if h.key == key {
                self.held = None;
            }
            self.chorded = false;
            return vec![];
        }
        if h.key != key {
            return vec![];
        }
        self.held = None;
        if h.fired {
            // The hold already did something; the release must not also act.
            return vec![];
        }
        let quick = now.saturating_sub(h.since) < HOLD_MS;
        match (self.steering, quick) {
            (true, _) => vec![Intent::Step(key)],
            (false, _) => vec![Intent::Volume(key)],
        }
    }

    /// Whether `key` is down right now (pressed, not yet released).
    pub fn is_down(&self, key: VolKey) -> bool {
        self.held.is_some_and(|h| h.key == key)
    }

    /// The held `key` was used by a chord with ANOTHER button (power + volume down is a
    /// screenshot): its hold and its release must do nothing, in every mode.
    pub fn consume(&mut self, key: VolKey) {
        if self.is_down(key) {
            self.chorded = true;
        }
    }

    /// In steering mode a held key activates instead of repeating forever.
    /// Called by the shell when the selection should be taken.
    pub fn is_holding(&self) -> bool {
        self.held.is_some_and(|h| h.fired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k() -> VolKeys {
        VolKeys::default()
    }

    #[test]
    fn a_press_is_still_volume() {
        // Surprising someone reaching for the volume is unforgivable, so the
        // short press keeps its meaning in the normal case.
        let mut v = k();
        assert!(v.press(VolKey::Up, 0).is_empty(), "nothing on the way down: it might become a hold");
        assert_eq!(v.release(VolKey::Up, 100), vec![Intent::Volume(VolKey::Up)]);
        assert_eq!(v.release(VolKey::Down, 200), vec![], "a release with nothing held does nothing");
    }

    #[test]
    fn a_hold_runs_the_bound_action_once_and_the_release_is_silent() {
        let mut v = k();
        v.press(VolKey::Down, 0);
        assert!(v.tick(400).is_empty(), "not yet");
        assert_eq!(v.tick(HOLD_MS), vec![Intent::Bound(VolKey::Down)]);
        // Held down for another two seconds: a bound action must not repeat.
        for t in 1..40 {
            assert!(v.tick(HOLD_MS + t * 50).is_empty(), "repeated at {t}");
        }
        assert_eq!(v.release(VolKey::Down, 3_000), vec![], "the hold already acted");
    }

    #[test]
    fn cold_mode_takes_the_keys_over_entirely() {
        // A touchscreen that will not answer is a worse problem than not
        // being able to change the volume.
        let mut v = k();
        v.steering = true;
        v.press(VolKey::Up, 0);
        assert_eq!(v.release(VolKey::Up, 100), vec![Intent::Step(VolKey::Up)], "no volume at all in this mode");
        // A held key steers repeatedly, which is what moving through a list
        // wants.
        v.press(VolKey::Down, 1_000);
        assert_eq!(v.tick(1_000 + HOLD_MS), vec![Intent::Step(VolKey::Down)]);
        assert!(v.tick(1_000 + HOLD_MS + 50).is_empty());
        assert_eq!(v.tick(1_000 + HOLD_MS + REPEAT_MS), vec![Intent::Step(VolKey::Down)], "and repeats");
        assert!(v.is_holding());
    }

    #[test]
    fn a_key_used_by_a_chord_with_another_button_stays_silent_until_released() {
        for steering in [false, true] {
            let mut v = k();
            v.steering = steering;
            assert!(!v.is_down(VolKey::Down));
            v.press(VolKey::Down, 0);
            assert!(v.is_down(VolKey::Down) && !v.is_down(VolKey::Up));
            v.consume(VolKey::Down); // power arrived while it was held
            assert!(v.tick(HOLD_MS + 10).is_empty(), "no hold fires after a chord");
            assert!(v.tick(HOLD_MS + 5 * REPEAT_MS).is_empty(), "and no steering repeat either");
            assert_eq!(v.release(VolKey::Down, 2_000), vec![], "the release is not a volume step");
            assert!(!v.is_down(VolKey::Down));
            // the next ordinary press works again
            v.press(VolKey::Down, 3_000);
            assert_eq!(v.release(VolKey::Down, 3_050), vec![if steering { Intent::Step(VolKey::Down) } else { Intent::Volume(VolKey::Down) }]);
        }
        // consuming a key that is not held does nothing
        let mut v = k();
        v.consume(VolKey::Down);
        v.press(VolKey::Down, 0);
        assert_eq!(v.release(VolKey::Down, 50), vec![Intent::Volume(VolKey::Down)]);
    }

    #[test]
    fn both_keys_are_one_gesture_in_every_mode() {
        for steering in [false, true] {
            let mut v = k();
            v.steering = steering;
            v.press(VolKey::Up, 0);
            assert_eq!(v.press(VolKey::Down, 50), vec![Intent::Chord], "steering={steering}");
            // Neither key's own behaviour fires afterwards.
            assert!(v.tick(2_000).is_empty());
            assert!(v.release(VolKey::Up, 2_100).is_empty());
            assert!(v.release(VolKey::Down, 2_200).is_empty());
            // And the next press is a clean one.
            v.press(VolKey::Up, 3_000);
            let after = v.release(VolKey::Up, 3_050);
            assert_eq!(after.len(), 1, "steering={steering}: {after:?}");
        }
    }

    #[test]
    fn switching_modes_mid_hold_does_not_fire_both_behaviours() {
        // Cold mode can turn itself on from a temperature reading at any
        // moment, including while a key is down.
        let mut v = k();
        v.press(VolKey::Up, 0);
        v.steering = true;
        assert_eq!(v.tick(HOLD_MS), vec![Intent::Step(VolKey::Up)], "the mode at the moment it fires decides");
        assert_eq!(v.release(VolKey::Up, HOLD_MS + 10), vec![], "and only once");
    }
}
