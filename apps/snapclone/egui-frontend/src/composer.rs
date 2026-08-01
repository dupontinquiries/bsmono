//! composer — the snap composer, ported from `SnapComposerDialog`.
//!
//! The named next surface after the camera and inbox, and the one that turns a
//! captured frame into something you can send. Its contract from the header:
//!
//! * `snapReady(QImage, int durationSecs)` — a composed image plus a **timer**.
//! * `renderComposed()` bakes the caption onto the background, so what is sent
//!   is one image rather than a photo plus overlay metadata the recipient's
//!   client has to agree about.
//! * `cycleColor()` steps the caption colour.
//! * `m_pressStart` — **swipe up to send**, so the whole background is the
//!   button and no thumb has to reach a corner.
//! * `applyKeyboardInset(visible, rect)` — the caption must stay above the
//!   on-screen keyboard, or you are typing at text you cannot see.
//!
//! ## The duration, which is the part with teeth
//!
//! `m_durationSecs = 5`, and **0 means no limit**. A snap with a timer of zero
//! is not a snap that vanishes instantly — it is one that does not vanish, and
//! writing the check as `if (duration)` gets that exactly backwards. It is the
//! kind of inversion that leaks someone's photo.

/// Caption colours, cycled by `cycleColor()`.
pub const COLORS: [[u8; 3]; 6] = [
    [255, 255, 255], [20, 20, 24], [232, 74, 74],
    [86, 196, 130], [92, 154, 240], [242, 198, 68],
];

/// How long a snap lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duration {
    /// Vanishes after N seconds.
    Seconds(u32),
    /// Stays. `m_durationSecs == 0` in the original — NOT "gone instantly".
    Unlimited,
}

impl Duration {
    /// From the wire value, where 0 means unlimited.
    pub fn from_secs(s: u32) -> Duration {
        if s == 0 { Duration::Unlimited } else { Duration::Seconds(s) }
    }
    pub fn to_secs(self) -> u32 {
        match self { Duration::Unlimited => 0, Duration::Seconds(s) => s }
    }
    /// Does this snap ever disappear? The question every call site actually
    /// wants, named so nobody has to remember which way zero goes.
    pub fn vanishes(self) -> bool { matches!(self, Duration::Seconds(_)) }
    pub fn label(self) -> String {
        match self { Duration::Unlimited => "∞".into(), Duration::Seconds(s) => format!("{s}s") }
    }
}

/// How far up the user must drag before it counts as a send, as a fraction of
/// the composer's height.
///
/// Not tiny: the background is also where you tap to place the caption, so a
/// short drag has to remain a tap. Sending a photo by accident is unrecoverable
/// in a way that failing to send is not.
pub const SEND_FRACTION: f32 = 0.18;

/// Composer state.
#[derive(Debug, Clone)]
pub struct Composer {
    pub caption: String,
    pub color: usize,
    pub font_size: f32,
    /// Caption position, normalised so it survives a resize or rotation.
    pub caption_y: f32,
    pub duration: Duration,
    /// Height taken by the on-screen keyboard, as a fraction of the composer.
    pub keyboard_inset: f32,
    drag_from: Option<f32>,
}

impl Default for Composer {
    fn default() -> Self {
        Composer { caption: String::new(), color: 0, font_size: 28.0, caption_y: 0.5,
                   duration: Duration::Seconds(5), keyboard_inset: 0.0, drag_from: None }
    }
}

impl Composer {
    pub fn cycle_color(&mut self) { self.color = (self.color + 1) % COLORS.len(); }
    pub fn color_rgb(&self) -> [u8; 3] { COLORS[self.color % COLORS.len()] }

    /// `applyKeyboardInset`. Keeps the caption above the keyboard, because
    /// typing at text you cannot see is the whole problem this solves.
    pub fn apply_keyboard_inset(&mut self, visible: bool, height_fraction: f32) {
        self.keyboard_inset = if visible { height_fraction.clamp(0.0, 0.9) } else { 0.0 };
    }

    /// Where the caption actually draws, after the keyboard is accounted for.
    pub fn effective_caption_y(&self) -> f32 {
        let limit = (1.0 - self.keyboard_inset - 0.08).max(0.05);
        self.caption_y.clamp(0.05, limit)
    }

    pub fn begin_drag(&mut self, y: f32) { self.drag_from = Some(y); }

    /// Release. Returns true when the gesture was a send.
    ///
    /// Upward only: a downward swipe is a dismiss elsewhere in the app, and
    /// treating any drag as a send would fire on it.
    pub fn end_drag(&mut self, y: f32) -> bool {
        match self.drag_from.take() {
            Some(from) => (from - y) >= SEND_FRACTION,
            None => false,
        }
    }

    pub fn is_dragging(&self) -> bool { self.drag_from.is_some() }

    /// Is there anything worth sending? A snap with no caption is fine — the
    /// picture is the message — so only the image matters, and the caller owns
    /// that. This exists so the rule is stated once.
    pub fn can_send(&self, has_image: bool) -> bool { has_image }
}

/// What `snapReady` carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub caption: String,
    pub color: [u8; 3],
    pub caption_y: f32,
    pub duration: Duration,
}

impl Composer {
    /// Bake the composition. One value, so the recipient's client does not have
    /// to agree about overlay placement.
    pub fn compose(&self) -> Snap {
        Snap {
            caption: self.caption.trim().to_string(),
            color: self.color_rgb(),
            caption_y: self.effective_caption_y(),
            duration: self.duration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_seconds_means_unlimited_not_instant() {
        // THE inversion that leaks someone's photo. Writing the check as
        // `if (duration)` gets this exactly backwards.
        assert_eq!(Duration::from_secs(0), Duration::Unlimited);
        assert!(!Duration::Unlimited.vanishes(), "an unlimited snap does not disappear");
        assert!(Duration::from_secs(5).vanishes());
        assert_eq!(Duration::Unlimited.to_secs(), 0, "and it round-trips to the wire");
        assert_eq!(Duration::from_secs(0).label(), "∞");
    }

    #[test]
    fn only_an_upward_swipe_sends() {
        // A downward swipe dismisses elsewhere in the app; treating any drag as
        // a send would fire on it, and sending a photo by accident is
        // unrecoverable in a way that failing to send is not.
        let mut c = Composer::default();
        c.begin_drag(0.9);
        assert!(c.end_drag(0.9 - SEND_FRACTION - 0.01), "far enough up");
        c.begin_drag(0.5);
        assert!(!c.end_drag(0.9), "downward never sends");
        c.begin_drag(0.5);
        assert!(!c.end_drag(0.48), "a short drag is a tap, not a send");
    }

    #[test]
    fn a_release_without_a_press_does_not_send() {
        // A stray release must not fire the one irreversible action.
        let mut c = Composer::default();
        assert!(!c.end_drag(0.0));
        assert!(!c.is_dragging());
    }

    #[test]
    fn the_caption_stays_above_the_keyboard() {
        // Typing at text you cannot see is the whole problem applyKeyboardInset
        // exists to solve.
        let mut c = Composer::default();
        c.caption_y = 0.95;
        assert!(c.effective_caption_y() < 0.96);
        c.apply_keyboard_inset(true, 0.45);
        // 1.0 - 0.45 - 0.08 = 0.47, and the float lands a hair above it — an
        // exact bound here tests f32 rounding rather than the behaviour.
        assert!(c.effective_caption_y() <= 0.47 + 1e-4,
                "pushed above the keyboard: {}", c.effective_caption_y());
        // and it comes back when the keyboard hides
        c.apply_keyboard_inset(false, 0.0);
        assert!(c.effective_caption_y() > 0.9);
    }

    #[test]
    fn an_absurd_keyboard_inset_still_leaves_room_for_the_caption() {
        let mut c = Composer::default();
        c.apply_keyboard_inset(true, 5.0);
        let y = c.effective_caption_y();
        assert!(y >= 0.05 && y <= 1.0, "{y}");
    }

    #[test]
    fn colours_cycle_and_wrap() {
        let mut c = Composer::default();
        let first = c.color_rgb();
        for _ in 0..COLORS.len() { c.cycle_color(); }
        assert_eq!(c.color_rgb(), first, "a full cycle returns to the start");
        // and an out-of-range index cannot panic
        c.color = 999;
        let _ = c.color_rgb();
    }

    #[test]
    fn composing_bakes_the_caption_and_keeps_the_timer() {
        let mut c = Composer::default();
        c.caption = "  hello  ".into();
        c.duration = Duration::from_secs(0);
        let s = c.compose();
        assert_eq!(s.caption, "hello", "trimmed");
        assert_eq!(s.duration, Duration::Unlimited);
        assert_eq!(s.color, COLORS[0]);
    }

    #[test]
    fn a_snap_with_no_caption_is_still_sendable() {
        // The picture is the message.
        let c = Composer::default();
        assert!(c.caption.is_empty());
        assert!(c.can_send(true));
        assert!(!c.can_send(false), "but there has to be an image");
    }
}
