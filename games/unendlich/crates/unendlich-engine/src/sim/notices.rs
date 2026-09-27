//! Short messages to the player on the HUD: "your torch goes out", "the air
//! is thin", "the stone tells of dead air". Until this existed those events
//! reached only the log, and several of the docs' warnings (the torch as the
//! CO₂ canary, earth sense) are only warnings if the player sees them.
//!
//! Pure bookkeeping, no rendering: the game pushes `(key, text)` with the
//! current time and asks for what is still showing. A key names a topic: a
//! new message on the same key replaces the old one (the breath stage moving
//! from "thin" to "short of breath" is one line that changes, not two), and
//! repeating the same text only refreshes it. Time is passed in (UI seconds)
//! to `active`, which is also when a notice's clock starts.

/// Seconds a notice stays up.
pub const TTL_S: f32 = 6.0;
/// Most lines shown at once. Older notices wait, unexpired, until a newer
/// one goes.
pub const MAX_SHOWN: usize = 3;

#[derive(Clone, Debug, PartialEq)]
struct Notice {
    key: &'static str,
    text: String,
    /// When it was first handed to the HUD. The clock starts there, not at
    /// the push, so a frame hitch (a chunk load, a slow device) cannot eat a
    /// message before it was ever drawn.
    shown_from: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Notices {
    items: Vec<Notice>,
    /// Bumped by every push and clear, so a HUD that redraws its text only
    /// now and then (the game rebuilds its status lines every 30 frames) can
    /// tell that it must redraw now.
    revision: u64,
}

impl Notices {
    pub fn new() -> Notices {
        Notices::default()
    }

    /// Queue `text` under `key`. Replaces any notice with the same key and
    /// moves it to the front; its clock starts when the HUD first shows it.
    pub fn push(&mut self, key: &'static str, text: impl Into<String>) {
        self.items.retain(|n| n.key != key);
        self.items.push(Notice { key, text: text.into(), shown_from: None });
        self.revision += 1;
    }

    /// Take down the notice under `key`, if any (a warning whose cause ended).
    pub fn clear(&mut self, key: &'static str) {
        let before = self.items.len();
        self.items.retain(|n| n.key != key);
        if self.items.len() != before {
            self.revision += 1;
        }
    }

    /// Changes whenever what is showing changes by a push or a clear.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// When the next shown notice expires (so the HUD can redraw then), if any.
    pub fn next_expiry(&self) -> Option<f32> {
        self.items
            .iter()
            .filter_map(|n| n.shown_from.map(|t| t + TTL_S))
            .fold(None, |m, t| Some(m.map_or(t, |m: f32| m.min(t))))
    }

    /// What to show at `now`, newest first, at most [`MAX_SHOWN`]. A notice
    /// stays for [`TTL_S`] from the first call that returned it; expired ones
    /// are dropped. Call this only when the lines are actually drawn.
    pub fn active(&mut self, now: f32) -> Vec<&str> {
        self.items.retain(|n| n.shown_from.map_or(true, |t| now - t < TTL_S));
        let len = self.items.len();
        for n in self.items.iter_mut().skip(len.saturating_sub(MAX_SHOWN)) {
            n.shown_from.get_or_insert(now);
        }
        self.items.iter().rev().take(MAX_SHOWN).map(|n| n.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_shows_for_the_ttl_from_its_first_draw_then_expires() {
        let mut n = Notices::new();
        assert!(n.active(0.0).is_empty());
        n.push("flame", "your torch goes out");
        // A 20 s hitch before the HUD draws again: the message still shows.
        assert_eq!(n.active(21.0), vec!["your torch goes out"]);
        assert_eq!(n.active(21.0 + TTL_S - 0.1), vec!["your torch goes out"]);
        assert!(n.active(21.0 + TTL_S).is_empty(), "gone TTL after it was first shown");
    }

    #[test]
    fn one_line_per_key_newest_first_and_capped() {
        let mut n = Notices::new();
        n.push("breath", "the air is thin");
        n.push("flame", "your torch sputters");
        assert_eq!(n.active(0.5), vec!["your torch sputters", "the air is thin"]);
        n.push("breath", "you are short of breath");
        assert_eq!(n.active(1.0), vec!["you are short of breath", "your torch sputters"]);
        n.push("heat", "a");
        n.push("earth", "b");
        n.push("death", "c");
        let shown = n.active(2.0);
        assert_eq!(shown.len(), MAX_SHOWN);
        assert_eq!(shown[0], "c");
        // The replaced breath line (first shown at 1.0) outlives the flame
        // line (first shown at 0.5).
        n.clear("heat");
        n.clear("earth");
        n.clear("death");
        assert_eq!(n.active(0.5 + TTL_S), vec!["you are short of breath"]);
    }

    #[test]
    fn the_hud_can_tell_when_to_redraw() {
        let mut n = Notices::new();
        let r0 = n.revision();
        assert_eq!(n.next_expiry(), None);
        n.push("flame", "x");
        assert_ne!(n.revision(), r0, "a push is a change");
        assert_eq!(n.next_expiry(), None, "not shown yet, so no clock running");
        n.active(2.5);
        assert_eq!(n.next_expiry(), Some(2.5 + TTL_S));
        n.push("breath", "y");
        n.active(3.0);
        assert_eq!(n.next_expiry(), Some(2.5 + TTL_S), "the earliest one");
        let r1 = n.revision();
        n.clear("nothing-here");
        assert_eq!(n.revision(), r1, "clearing an absent key changes nothing");
        n.clear("flame");
        assert_ne!(n.revision(), r1);
    }

    #[test]
    fn clearing_a_key_takes_its_line_down_now() {
        let mut n = Notices::new();
        n.push("heat", "the heat is punishing");
        n.clear("heat");
        assert!(n.active(0.1).is_empty());
    }
}
