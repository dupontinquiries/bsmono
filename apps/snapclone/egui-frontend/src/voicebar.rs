//! voicebar — `GroupVoiceBar`. The calm on-air control surface.
//!
//! One bar that surfaces the whole voice stack at a glance: who holds the
//! floor, your live mic level, the security tier, and a press-and-hold talk
//! button. It is a PURE VIEW — it owns no audio and no sockets. The controller
//! wires it to the pieces that do:
//!
//! ```text
//!   floorHolderChanged   → set_talker        talk_pressed  → pressTalk
//!   transmitStateChanged → set_on_air        talk_released → releaseTalk
//!   micLevel             → set_mic_level     private_requested → negotiation
//! ```
//!
//! This also puts the join / mute / push-to-talk controls back somewhere
//! reachable. The previous commit took them off the Wayfinder page — where they
//! never belonged — and left them with no home, which is a regression I made
//! and this closes.
//!
//! ## Swipe-to-latch is the interaction worth getting right
//!
//! Half-duplex voice means holding a button to talk. Holding a phone to your
//! face while holding a button is awkward for anything longer than a sentence,
//! so an upward swipe of 44px while holding LATCHES transmit on, hands-free,
//! and a tap then releases it. Two rules follow from that and both matter:
//!
//! * A latched bar must not un-latch when the finger lifts — that is the whole
//!   point of latching.
//! * Losing the floor (`set_on_air(false)` from the session) clears the latch
//!   AND the press. Otherwise the UI shows you transmitting while the session
//!   has moved on, and the next tap "releases" a floor you never held.
//!
//! ## The mic level is eased, and only while on air
//!
//! `display += (target - display) * 0.25` per frame, with the target forced to
//! zero when off air. A meter that tracks the raw level jitters at every
//! consonant; one that keeps moving after you stop transmitting is showing the
//! room your microphone can still hear, which is not what a transmit meter
//! means.

/// `kLatchSwipePx` — upward travel on the talk button that latches transmit.
pub const LATCH_SWIPE_PX: f32 = 44.0;
/// Per-frame easing toward the live level.
pub const LEVEL_EASE: f32 = 0.25;
/// Pulse advance per frame; wraps at 2π.
pub const PULSE_STEP: f32 = 0.12;
/// `FrequentButton::kGrandScale` — the talk button in push-to-talk, where you
/// hold it constantly.
pub const GRAND_SCALE: f32 = 1.50;
/// `kComfortableScale` — present but not the focus, in the auto modes.
pub const COMFORTABLE_SCALE: f32 = 1.25;

/// How private the channel actually is.
///
/// `Securing` is its own tier, not an optimistic `Private`: private mode has
/// been agreed and the handshake is still running. Drawing a closed lock then
/// would tell the user the channel is protected while it is not yet, which is a
/// lie with consequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Security { #[default] Public, Securing, Private }

impl Security {
    /// Only `Private` closes the lock.
    pub fn locked(self) -> bool { matches!(self, Security::Private) }
    /// `Securing` pulses, because a handshake is a process and a still glyph
    /// reads as a finished state.
    pub fn pulses(self) -> bool { matches!(self, Security::Securing) }
    pub fn label(self) -> &'static str {
        match self { Security::Public => "Public",
                     Security::Securing => "Securing…",
                     Security::Private => "Private" }
    }
}

/// What the bar reports upward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    TalkPressed,
    TalkReleased,
    /// Tapping the lock requests private mode, or withdraws the request.
    PrivateRequested(bool),
}

/// The bar's state.
#[derive(Debug, Clone)]
pub struct VoiceBar {
    pub security: Security,
    /// Who holds the floor. Empty means the floor is open.
    talker: String,
    talker_is_self: bool,
    mic_level: f32,
    display_level: f32,
    on_air: bool,
    ptt_down: bool,
    latched: bool,
    press_y: f32,
    /// Talk-button size factor.
    talk_scale: f32,
    /// Shared oscillator; faster and brighter than a container's glow, because
    /// a securing handshake is a status and not a decoration.
    breath: uikit_egui::motion::Breath,
}

impl Default for VoiceBar {
    fn default() -> Self {
        VoiceBar { security: Security::default(), talker: String::new(), talker_is_self: false,
                   mic_level: 0.0, display_level: 0.0, on_air: false, ptt_down: false,
                   latched: false, press_y: 0.0, talk_scale: GRAND_SCALE,
                   breath: uikit_egui::motion::Breath::new(1000.0, 0.0, 1.0) }
    }
}

impl VoiceBar {
    pub fn new() -> Self {
        VoiceBar { talk_scale: GRAND_SCALE,
                   breath: uikit_egui::motion::Breath::new(1000.0, 0.0, 1.0),
                   ..Default::default() }
    }

    /// `setTalker`. `is_self` styles it as "You".
    pub fn set_talker(&mut self, name: &str, is_self: bool) {
        self.talker = name.to_string();
        self.talker_is_self = is_self;
    }

    /// The centre line: who is speaking, or that nobody is.
    pub fn talker_label(&self) -> String {
        if self.talker.trim().is_empty() { return "Floor open".into(); }
        if self.talker_is_self { "You".into() } else { self.talker.clone() }
    }

    pub fn floor_open(&self) -> bool { self.talker.trim().is_empty() }

    /// `setMicLevel`, clamped — a level above 1.0 would overrun the meter track.
    pub fn set_mic_level(&mut self, level: f32) {
        self.mic_level = level.clamp(0.0, 1.0);
    }

    /// `setOnAir`. Losing the floor clears the latch and the press, or the bar
    /// shows you transmitting after the session has moved on and the next tap
    /// "releases" a floor you never held.
    pub fn set_on_air(&mut self, on_air: bool) {
        self.on_air = on_air;
        if !on_air { self.latched = false; self.ptt_down = false; }
    }

    pub fn on_air(&self) -> bool { self.on_air }
    pub fn latched(&self) -> bool { self.latched }

    /// `setTalkProminent`: grand where you hold it constantly, comfortable where
    /// it is rarely touched.
    pub fn set_talk_prominent(&mut self, primary: bool) {
        self.talk_scale = if primary { GRAND_SCALE } else { COMFORTABLE_SCALE };
    }
    pub fn talk_scale(&self) -> f32 { self.talk_scale }

    /// Advance the eased level and the pulse phase.
    pub fn tick(&mut self) {
        self.breath.tick(PULSE_STEP * 1000.0 / std::f32::consts::TAU);
        // Target zero when off air: a meter still moving after you stop
        // transmitting is showing the room the mic can hear, which is not what
        // a TRANSMIT meter means.
        let target = if self.on_air { self.mic_level } else { 0.0 };
        self.display_level += (target - self.display_level) * LEVEL_EASE;
    }

    /// The meter fill, 0..=1.
    pub fn meter(&self) -> f32 { if self.on_air { self.display_level.clamp(0.0, 1.0) } else { 0.0 } }

    /// 0..=1 pulse, for the securing lock and the on-air glow.
    pub fn pulse(&self) -> f32 { self.breath.value() }

    /// True while anything is live, which is what the glow follows.
    pub fn live(&self) -> bool { self.on_air || self.ptt_down || self.latched }

    /// Press the talk button.
    ///
    /// While latched, a press is a RELEASE — the button has become a stop
    /// control, and a second latch-on would leave no way to stop without
    /// leaving the screen.
    pub fn talk_press(&mut self, y: f32) -> Event {
        if self.latched {
            self.latched = false;
            self.ptt_down = false;
            return Event::TalkReleased;
        }
        self.ptt_down = true;
        self.press_y = y;
        Event::TalkPressed
    }

    /// Drag while holding. An upward swipe past the threshold latches.
    pub fn talk_move(&mut self, y: f32) -> bool {
        if !self.ptt_down || self.latched { return false; }
        // Upward only: y decreasing. A downward swipe is a scroll and must not
        // latch a live microphone.
        if self.press_y - y >= LATCH_SWIPE_PX { self.latched = true; return true; }
        false
    }

    /// Lift the finger. A latched bar keeps transmitting — that is the point.
    pub fn talk_release(&mut self) -> Option<Event> {
        if !self.ptt_down { return None; }
        if self.latched { return None; }
        self.ptt_down = false;
        Some(Event::TalkReleased)
    }

    /// Tap the lock: request private mode, or withdraw the request.
    ///
    /// Withdrawing from `Securing` is allowed on purpose — a handshake that is
    /// taking too long should be abandonable without leaving the call.
    pub fn tap_lock(&mut self) -> Event {
        Event::PrivateRequested(!matches!(self.security, Security::Securing | Security::Private))
    }
}

/// Draw the bar. Returns whatever the interaction produced.
pub fn voicebar_ui(ui: &mut egui::Ui, b: &mut VoiceBar,
                   c: &uikit_egui::PaletteColors) -> Option<Event> {
    b.tick();
    let mut ev = None;
    ui.horizontal(|ui| {
        // Lock. Closed only on Private; Securing pulses so a handshake reads as
        // a process rather than a finished state.
        let glyph = if b.security.locked() { "🔒" } else { "🔓" };
        let col = match b.security {
            Security::Private => c.success,
            Security::Securing => {
                let t = b.pulse();
                egui::Color32::from_rgba_unmultiplied(
                    c.warning.r(), c.warning.g(), c.warning.b(), (128.0 + 127.0 * t) as u8)
            }
            Security::Public => c.text_weak,
        };
        if ui.add(egui::Button::new(egui::RichText::new(glyph).color(col)).frame(false))
            .on_hover_text(b.security.label()).clicked() {
            ev = Some(b.tap_lock());
        }

        ui.vertical(|ui| {
            ui.colored_label(if b.floor_open() { c.text_weak } else { c.text }, b.talker_label());
            // The meter track, filled by the eased level.
            let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 4.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, c.bg_raised);
            let lvl = b.meter();
            if lvl > 0.001 {
                let mut fill = rect;
                fill.set_width(rect.width() * lvl);
                ui.painter().rect_filled(fill, 2.0, c.accent);
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let d = 28.0 * b.talk_scale();
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::click_and_drag());
            let base = if b.live() { c.danger } else { c.bg_raised };
            ui.painter().circle_filled(rect.center(), d * 0.5, base);
            if b.latched {
                // A small badge: transmit is latched on, tap to release.
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "🔒",
                                  egui::FontId::proportional(10.0), c.text);
            }
            if resp.drag_started() {
                ev = Some(b.talk_press(resp.interact_pointer_pos().map(|p| p.y).unwrap_or(0.0)));
            }
            if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() { b.talk_move(p.y); }
            }
            if resp.drag_stopped() {
                if let Some(e) = b.talk_release() { ev = Some(e); }
            }
        });
    });
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> VoiceBar { VoiceBar::new() }

    #[test]
    fn securing_is_its_own_tier_and_does_not_close_the_lock() {
        // Drawing a closed lock while the handshake is still running tells the
        // user the channel is protected when it is not yet.
        assert!(!Security::Public.locked());
        assert!(!Security::Securing.locked());
        assert!(Security::Private.locked());
        assert!(Security::Securing.pulses(), "a handshake is a process");
        assert!(!Security::Private.pulses());
    }

    #[test]
    fn an_empty_talker_means_the_floor_is_open() {
        let mut b = bar();
        assert!(b.floor_open());
        assert_eq!(b.talker_label(), "Floor open");
        b.set_talker("   ", false);
        assert!(b.floor_open(), "whitespace is not a talker");
    }

    #[test]
    fn your_own_turn_is_labelled_you() {
        let mut b = bar();
        b.set_talker("ada@10.0.0.2", true);
        assert_eq!(b.talker_label(), "You");
        b.set_talker("Ada", false);
        assert_eq!(b.talker_label(), "Ada");
    }

    #[test]
    fn the_mic_level_is_clamped_so_it_cannot_overrun_the_meter() {
        let mut b = bar();
        b.set_on_air(true);
        b.set_mic_level(3.0);
        for _ in 0..40 { b.tick(); }
        assert!(b.meter() <= 1.0);
        b.set_mic_level(-1.0);
        for _ in 0..40 { b.tick(); }
        assert!(b.meter() >= 0.0);
    }

    #[test]
    fn the_meter_eases_toward_the_level_rather_than_snapping() {
        // A meter that tracks the raw level jitters at every consonant.
        let mut b = bar();
        b.set_on_air(true);
        b.set_mic_level(1.0);
        b.tick();
        let first = b.meter();
        assert!(first > 0.0 && first < 0.5, "one step should be partial, got {first}");
        for _ in 0..40 { b.tick(); }
        assert!(b.meter() > 0.95, "and it converges");
    }

    #[test]
    fn the_meter_falls_to_zero_off_air_even_while_the_mic_is_loud() {
        // Otherwise it shows the room the microphone can still hear, which is
        // not what a TRANSMIT meter means.
        let mut b = bar();
        b.set_on_air(true);
        b.set_mic_level(1.0);
        for _ in 0..40 { b.tick(); }
        b.set_on_air(false);
        for _ in 0..40 { b.tick(); }
        assert_eq!(b.meter(), 0.0);
    }

    #[test]
    fn an_upward_swipe_while_holding_latches_transmit() {
        // Holding a phone to your face while holding a button is awkward for
        // anything longer than a sentence.
        let mut b = bar();
        assert_eq!(b.talk_press(300.0), Event::TalkPressed);
        assert!(!b.talk_move(280.0), "20px is not a swipe");
        assert!(b.talk_move(300.0 - LATCH_SWIPE_PX), "44px latches");
        assert!(b.latched());
    }

    #[test]
    fn a_downward_swipe_never_latches_a_live_microphone() {
        let mut b = bar();
        b.talk_press(300.0);
        assert!(!b.talk_move(400.0), "downward is a scroll, not a latch");
        assert!(!b.latched());
    }

    #[test]
    fn a_latched_bar_keeps_transmitting_when_the_finger_lifts() {
        // That is the entire point of latching.
        let mut b = bar();
        b.talk_press(300.0);
        b.talk_move(300.0 - LATCH_SWIPE_PX);
        assert_eq!(b.talk_release(), None, "no release event while latched");
        assert!(b.latched());
    }

    #[test]
    fn a_tap_while_latched_releases_rather_than_latching_again() {
        // The button has become a stop control; latching twice would leave no
        // way to stop without leaving the screen.
        let mut b = bar();
        b.talk_press(300.0);
        b.talk_move(300.0 - LATCH_SWIPE_PX);
        assert_eq!(b.talk_press(300.0), Event::TalkReleased);
        assert!(!b.latched());
    }

    #[test]
    fn an_ordinary_press_and_release_is_push_to_talk() {
        let mut b = bar();
        assert_eq!(b.talk_press(300.0), Event::TalkPressed);
        assert_eq!(b.talk_release(), Some(Event::TalkReleased));
        assert!(!b.latched());
    }

    #[test]
    fn a_release_without_a_press_reports_nothing() {
        // A stray pointer-up from elsewhere must not release a floor.
        assert_eq!(bar().talk_release(), None);
    }

    #[test]
    fn losing_the_floor_clears_the_latch_and_the_press() {
        // Otherwise the UI shows you transmitting after the session moved on,
        // and the next tap "releases" a floor you never held.
        let mut b = bar();
        b.talk_press(300.0);
        b.talk_move(300.0 - LATCH_SWIPE_PX);
        b.set_on_air(true);
        assert!(b.latched());
        b.set_on_air(false);
        assert!(!b.latched());
        assert!(!b.live());
        assert_eq!(b.talk_release(), None, "and there is no press left to release");
    }

    #[test]
    fn the_glow_follows_anything_live_not_just_the_session() {
        // Pressing should light immediately rather than waiting for the round
        // trip that grants the floor.
        let mut b = bar();
        assert!(!b.live());
        b.talk_press(300.0);
        assert!(b.live(), "pressing is live before the session answers");
        b.talk_release();
        assert!(!b.live());
        b.set_on_air(true);
        assert!(b.live());
    }

    #[test]
    fn tapping_the_lock_requests_privacy_and_tapping_again_withdraws_it() {
        // Withdrawing from Securing is deliberate: a handshake taking too long
        // should be abandonable without leaving the call.
        let mut b = bar();
        assert_eq!(b.tap_lock(), Event::PrivateRequested(true));
        b.security = Security::Securing;
        assert_eq!(b.tap_lock(), Event::PrivateRequested(false));
        b.security = Security::Private;
        assert_eq!(b.tap_lock(), Event::PrivateRequested(false));
    }

    #[test]
    fn the_talk_button_is_grand_in_push_to_talk_and_comfortable_otherwise() {
        let mut b = bar();
        assert_eq!(b.talk_scale(), GRAND_SCALE, "push-to-talk is the default");
        b.set_talk_prominent(false);
        assert_eq!(b.talk_scale(), COMFORTABLE_SCALE);
        assert!(COMFORTABLE_SCALE < GRAND_SCALE);
    }

    #[test]
    fn the_pulse_stays_in_range_and_wraps() {
        let mut b = bar();
        for _ in 0..500 {
            b.tick();
            let p = b.pulse();
            assert!((0.0..=1.0).contains(&p), "pulse out of range: {p}");
        }
    }
}
