//! chrome — page dots, the now-playing bar, the safety toast, identicons.
//!
//! The small surfaces that appear on every page. Small does not mean trivial:
//! the identicon has an exact hash and the toast has an exact timeout, and both
//! are the kind of thing a port silently changes and nobody can say why the app
//! feels different.
//!
//! ## The identicon hash must match to the bit
//!
//! `Identicon.qml`'s `_fnv32` is FNV-1a with a deliberate low-16/high-16 split
//! multiply, because JavaScript's numbers lose bits above 2^53 and a plain
//! `h * prime` would drift. Rust has no such problem — but the ANSWER has to be
//! identical, or every avatar in the app changes colour and pattern the day the
//! port ships. A wrapping multiply reproduces exactly what that split computes.

/// FNV-1a 32-bit, matching `Identicon.qml::_fnv32` bit for bit.
///
/// The QML splits the multiply to stay inside JavaScript's safe integer range;
/// `wrapping_mul` on a `u32` is the same arithmetic without the workaround. It
/// matters because the hash picks the avatar's colour and cells — get it wrong
/// and every contact in the app looks like a different person.
pub fn fnv32(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    // char code, not byte: the QML walks UTF-16 code units, and a name with an
    // accent must hash the same in both.
    for c in s.encode_utf16() {
        h ^= c as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// An identicon: a symmetric 5x5 grid plus a colour, both from the hash.
#[derive(Debug, Clone, PartialEq)]
pub struct Identicon {
    pub hue: f32,
    /// Row-major 5x5. Mirrored left-to-right, which is what makes a random grid
    /// read as a face rather than as noise.
    pub cells: [bool; 25],
}

impl Identicon {
    pub fn of(title: &str) -> Identicon {
        let h = fnv32(title);
        let mut cells = [false; 25];
        for row in 0..5 {
            for col in 0..3 {
                let bit = (h >> ((row * 3 + col) % 32)) & 1 == 1;
                cells[row * 5 + col] = bit;
                cells[row * 5 + (4 - col)] = bit;   // mirror
            }
        }
        Identicon { hue: (h % 360) as f32, cells }
    }

    /// An empty title still produces something. A blank avatar looks like a
    /// failed image load, which is worse than an arbitrary pattern.
    pub fn is_blank(&self) -> bool { self.cells.iter().all(|c| !c) }
}

/// Page dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageDots {
    pub count: usize,
    pub index: usize,
}

impl PageDots {
    /// The dot to fill. Clamped rather than wrapping: an index past the end is
    /// a bug upstream, and wrapping it to 0 hides that by showing a plausible
    /// dot on the wrong page.
    pub fn active(&self) -> usize {
        if self.count == 0 { 0 } else { self.index.min(self.count - 1) }
    }
    /// One page needs no dots — a single dot tells you nothing and takes space.
    pub fn visible(&self) -> bool { self.count > 1 }
}

/// The now-playing bar.
#[derive(Debug, Clone, Default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub source: String,
    pub playing: bool,
}

impl NowPlaying {
    /// Is there anything to show? The bar hides entirely when nothing is loaded,
    /// rather than showing an empty strip that looks like a rendering fault.
    pub fn active(&self) -> bool {
        !self.title.trim().is_empty() || !self.artist.trim().is_empty()
    }

    /// One line: "Artist — Title", or whichever half exists. Joining with a dash
    /// when one side is empty leaves a dangling separator, which looks like
    /// missing data rather than absent data.
    pub fn line(&self) -> String {
        let (a, t) = (self.artist.trim(), self.title.trim());
        match (a.is_empty(), t.is_empty()) {
            (false, false) => format!("{a} — {t}"),
            (true, false) => t.to_string(),
            (false, true) => a.to_string(),
            (true, true) => String::new(),
        }
    }
}

/// A transient warning. `interval: 8000` in the QML.
pub const TOAST_SECONDS: f32 = 8.0;

#[derive(Debug, Clone, Default)]
pub struct Toast {
    message: Option<String>,
    age: f32,
}

impl Toast {
    /// Show a message, restarting the timer.
    ///
    /// A second warning while one is up REPLACES it and restarts the clock,
    /// rather than queueing: the newest safety message is the relevant one, and
    /// a queue means the user reads a stale warning about a state that has
    /// already changed.
    pub fn show(&mut self, message: &str) {
        self.message = Some(message.to_string());
        self.age = 0.0;
    }

    pub fn tick(&mut self, dt: f32) {
        if self.message.is_some() {
            self.age += dt.max(0.0);
            if self.age >= TOAST_SECONDS { self.message = None; }
        }
    }

    pub fn visible(&self) -> Option<&str> { self.message.as_deref() }
    pub fn dismiss(&mut self) { self.message = None; }

    /// Fade for the last second, so it leaves rather than vanishing.
    pub fn opacity(&self) -> f32 {
        match self.message {
            None => 0.0,
            Some(_) => ((TOAST_SECONDS - self.age) / 1.0).clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identicon_hash_is_fnv1a_and_stable() {
        // If this drifts, every avatar in the app changes the day the port
        // ships. The offset basis alone is checkable by hand.
        assert_eq!(fnv32(""), 0x811c_9dc5);
        // FNV-1a of "a" is the documented 0xe40c292c
        assert_eq!(fnv32("a"), 0xe40c_292c);
        assert_eq!(fnv32("foobar"), 0xbf9c_f968);
        // and it is deterministic across calls
        assert_eq!(fnv32("Ada"), fnv32("Ada"));
        assert_ne!(fnv32("Ada"), fnv32("Bo"));
    }

    #[test]
    fn an_identicon_is_mirrored_so_it_reads_as_a_face() {
        let i = Identicon::of("Ada");
        for row in 0..5 {
            for col in 0..2 {
                assert_eq!(i.cells[row * 5 + col], i.cells[row * 5 + (4 - col)],
                           "row {row} col {col} not mirrored");
            }
        }
    }

    #[test]
    fn different_names_give_different_icons() {
        assert_ne!(Identicon::of("Ada"), Identicon::of("Bo"));
        assert_eq!(Identicon::of("Ada"), Identicon::of("Ada"));
    }

    #[test]
    fn an_empty_title_still_produces_an_icon() {
        // A blank avatar looks like a failed image load, which is worse than an
        // arbitrary pattern.
        let i = Identicon::of("");
        assert!(i.hue >= 0.0 && i.hue < 360.0);
        assert!(!i.is_blank(), "the empty-string hash has bits set");
    }

    #[test]
    fn one_page_shows_no_dots() {
        // A single dot tells you nothing and takes space.
        assert!(!PageDots { count: 1, index: 0 }.visible());
        assert!(!PageDots { count: 0, index: 0 }.visible());
        assert!(PageDots { count: 3, index: 0 }.visible());
    }

    #[test]
    fn an_index_past_the_end_clamps_rather_than_wrapping() {
        // Wrapping to 0 hides an upstream bug by showing a plausible dot on the
        // wrong page.
        assert_eq!(PageDots { count: 3, index: 9 }.active(), 2);
        assert_eq!(PageDots { count: 0, index: 9 }.active(), 0, "and does not divide by zero");
    }

    #[test]
    fn the_now_playing_line_never_dangles_a_separator() {
        // A trailing dash looks like missing data rather than absent data.
        let np = |a: &str, t: &str| NowPlaying { artist: a.into(), title: t.into(), ..Default::default() };
        assert_eq!(np("Ada", "Song").line(), "Ada — Song");
        assert_eq!(np("", "Song").line(), "Song");
        assert_eq!(np("Ada", "").line(), "Ada");
        assert_eq!(np("", "").line(), "");
        assert_eq!(np("  ", " ").line(), "");
    }

    #[test]
    fn the_bar_hides_when_nothing_is_loaded() {
        // An empty strip looks like a rendering fault.
        assert!(!NowPlaying::default().active());
        assert!(NowPlaying { title: "x".into(), ..Default::default() }.active());
    }

    #[test]
    fn a_new_warning_replaces_the_old_one_and_restarts_the_clock() {
        // The newest safety message is the relevant one; queueing means reading
        // a stale warning about a state that has already changed.
        let mut t = Toast::default();
        t.show("first");
        t.tick(7.0);
        t.show("second");
        assert_eq!(t.visible(), Some("second"));
        t.tick(2.0);
        assert_eq!(t.visible(), Some("second"), "the clock restarted");
    }

    #[test]
    fn a_toast_expires_after_the_qmls_eight_seconds() {
        let mut t = Toast::default();
        t.show("careful");
        t.tick(TOAST_SECONDS - 0.1);
        assert!(t.visible().is_some());
        assert!(t.opacity() < 1.0, "already fading");
        t.tick(0.2);
        assert_eq!(t.visible(), None);
        assert_eq!(t.opacity(), 0.0);
    }

    #[test]
    fn a_toast_can_be_dismissed_early_and_ticking_after_is_harmless() {
        let mut t = Toast::default();
        t.show("careful");
        t.dismiss();
        assert_eq!(t.visible(), None);
        t.tick(1.0);
        assert_eq!(t.visible(), None);
    }
}
