//! safety — the volume clamp, and the unlock that is deliberately awkward.
//!
//! Ported from `SafetyToast.qml` and the controller's clamp state. A manual
//! volume change above the per-product unsafe threshold (≈95 dB) is **clamped**,
//! and a slide-down toast offers two things:
//!
//! * **Dismiss** — accept the cap.
//! * **Unlock** — accept the warning, unlock high volume *for the rest of this
//!   device session*, and **re-apply the requested value**.
//!
//! It auto-dismisses after 8 seconds.
//!
//! ## Why the request is held rather than discarded
//!
//! `volume_pending_request` exists so Unlock can complete the action the user
//! originally asked for. Without it, unlocking leaves the volume at the cap and
//! the user has to make the same gesture again — having just been told the
//! system stopped them, which reads as the unlock not working.
//!
//! ## Why the unlock is per-session and not persisted
//!
//! Hearing damage is cumulative and a preference is forever. A setting that
//! survives reboots means one impatient tap disables the protection permanently,
//! long after the reason for it is forgotten. Per-session means the default is
//! re-asserted every time the device is picked up, which is the only point at
//! which the question is meaningful.
//!
//! ## Why the loud band exists separately
//!
//! 88–95 dB is loud but permitted, so it is not a clamp — it is a state worth
//! showing. Collapsing it into "clamped or fine" loses the warning that matters
//! most: the level you can reach without being stopped.

/// The band the current volume sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loudness {
    Normal,
    /// 88–95 dB: loud, permitted, worth showing.
    Loud,
    /// Above the unsafe threshold. Only reachable while unlocked.
    Unsafe,
}

/// Auto-dismiss, from the QML.
pub const TOAST_SECONDS: f32 = 8.0;

/// The volume safety state for one device session.
#[derive(Debug, Clone)]
pub struct Safety {
    /// Volume at which ≈95 dB starts, per product.
    pub unsafe_threshold: u8,
    /// Volume at which the loud band starts (≈88 dB).
    pub loud_threshold: u8,
    /// Unlocked for THIS device session only.
    unlocked: bool,
    /// The value the user asked for and did not get.
    pending: Option<u8>,
    toast_age: f32,
}

impl Default for Safety {
    fn default() -> Self {
        Safety { unsafe_threshold: 85, loud_threshold: 72, unlocked: false,
                 pending: None, toast_age: 0.0 }
    }
}

/// What a volume request produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Applied as asked.
    Applied(u8),
    /// Capped, and the toast is now showing.
    Clamped { applied: u8, requested: u8 },
}

impl Safety {
    pub fn band(&self, volume: u8) -> Loudness {
        if volume > self.unsafe_threshold { Loudness::Unsafe }
        else if volume >= self.loud_threshold { Loudness::Loud }
        else { Loudness::Normal }
    }

    pub fn unlocked(&self) -> bool { self.unlocked }

    /// Request a volume.
    pub fn request(&mut self, volume: u8) -> Outcome {
        if volume <= self.unsafe_threshold || self.unlocked {
            self.pending = None;
            return Outcome::Applied(volume);
        }
        // Held, not discarded: Unlock has to be able to complete the action the
        // user actually asked for.
        self.pending = Some(volume);
        self.toast_age = 0.0;
        Outcome::Clamped { applied: self.unsafe_threshold, requested: volume }
    }

    /// Is the toast up?
    pub fn toast_visible(&self) -> bool { self.pending.is_some() }

    /// `(applied, requested)` while a request is held back.
    ///
    /// The renderer needs both numbers — "asked for 100, held at 95" is the
    /// message, and either half alone does not explain the cap. `pending`
    /// stores only the request; the applied value is the threshold, which is
    /// what `request` returned in the Clamped outcome.
    pub fn pending_pair(&self) -> Option<(u8, u8)> {
        self.pending.map(|want| (self.unsafe_threshold, want))
    }

    pub fn tick(&mut self, dt: f32) {
        if self.pending.is_none() { return; }
        self.toast_age += dt.max(0.0);
        if self.toast_age >= TOAST_SECONDS {
            // Timing out is a DISMISS, not an unlock. Silence must never grant
            // the more dangerous option.
            self.dismiss();
        }
    }

    /// Accept the cap.
    pub fn dismiss(&mut self) {
        self.pending = None;
        self.toast_age = 0.0;
    }

    /// Accept the warning and unlock for this session, re-applying the request.
    ///
    /// Returns the volume to apply, or `None` if there was nothing pending —
    /// so a stray Unlock cannot raise the volume on its own.
    pub fn unlock(&mut self) -> Option<u8> {
        let want = self.pending.take()?;
        self.unlocked = true;
        self.toast_age = 0.0;
        Some(want)
    }

    /// A new device session: the protection is re-asserted.
    ///
    /// Called on connect rather than on app start, because "session" here means
    /// the device — moving to another speaker should not carry an unlock granted
    /// for the first one.
    pub fn new_session(&mut self) {
        self.unlocked = false;
        self.pending = None;
        self.toast_age = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_above_the_threshold_is_capped_not_refused() {
        // The user gets the loudest safe volume, not their previous one: being
        // stopped should still move the control in the direction asked for.
        let mut s = Safety::default();
        assert_eq!(s.request(90), Outcome::Clamped { applied: 85, requested: 90 });
        assert!(s.toast_visible());
    }

    #[test]
    fn unlocking_re_applies_what_was_asked_for() {
        // Without the held request, unlocking leaves the volume at the cap and
        // the user repeats the gesture they were just stopped from making —
        // which reads as the unlock not working.
        let mut s = Safety::default();
        s.request(92);
        assert_eq!(s.unlock(), Some(92));
        assert!(s.unlocked());
        assert!(!s.toast_visible());
        // and subsequent requests go straight through
        assert_eq!(s.request(95), Outcome::Applied(95));
    }

    #[test]
    fn a_timeout_dismisses_rather_than_unlocks() {
        // Silence must never grant the more dangerous option.
        let mut s = Safety::default();
        s.request(92);
        s.tick(TOAST_SECONDS + 0.1);
        assert!(!s.toast_visible());
        assert!(!s.unlocked(), "eight seconds of nothing is not consent");
        assert_eq!(s.request(92), Outcome::Clamped { applied: 85, requested: 92 });
    }

    #[test]
    fn dismissing_keeps_the_cap() {
        let mut s = Safety::default();
        s.request(92);
        s.dismiss();
        assert!(!s.unlocked());
        assert_eq!(s.request(92), Outcome::Clamped { applied: 85, requested: 92 },
                   "the next attempt is clamped again");
    }

    #[test]
    fn a_stray_unlock_cannot_raise_the_volume_on_its_own() {
        // Nothing pending means nothing to grant.
        let mut s = Safety::default();
        assert_eq!(s.unlock(), None);
        assert!(!s.unlocked());
    }

    #[test]
    fn the_unlock_does_not_survive_a_new_device_session() {
        // Hearing damage is cumulative and a preference is forever: a persisted
        // unlock means one impatient tap disables the protection permanently.
        let mut s = Safety::default();
        s.request(92);
        s.unlock();
        assert!(s.unlocked());
        s.new_session();
        assert!(!s.unlocked(), "the default is re-asserted");
        assert_eq!(s.request(92), Outcome::Clamped { applied: 85, requested: 92 });
    }

    #[test]
    fn the_loud_band_is_a_state_not_a_clamp() {
        // 88-95 dB is loud but permitted, and it is the warning that matters
        // most: the level you can reach WITHOUT being stopped.
        let s = Safety::default();
        assert_eq!(s.band(50), Loudness::Normal);
        assert_eq!(s.band(72), Loudness::Loud, "the loud band starts here");
        assert_eq!(s.band(85), Loudness::Loud, "still permitted at the threshold");
        assert_eq!(s.band(86), Loudness::Unsafe);
    }

    #[test]
    fn a_request_at_exactly_the_threshold_is_allowed() {
        // Off-by-one here either blocks the loudest safe volume or permits the
        // first unsafe one.
        let mut s = Safety::default();
        assert_eq!(s.request(85), Outcome::Applied(85));
        assert!(!s.toast_visible());
    }
}

/// What the toast's buttons produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastAction {
    /// Accept the cap.
    Dismiss,
    /// Accept the warning and unlock for this session. The caller applies the
    /// volume `unlock()` hands back — this only reports the press, so the
    /// "nothing pending" guard stays in one place.
    Unlock,
}

/// The slide-down safety toast. Draws nothing when nothing is clamped.
///
/// Ported from `SafetyToast.qml`. The model was here and tested; there was no
/// renderer and no caller, so a manual volume above the threshold went
/// straight through — this module's whole reason for existing did not happen.
pub fn safety_toast_ui(
    ui: &mut egui::Ui,
    s: &Safety,
    c: &uikit_egui::PaletteColors,
) -> Option<ToastAction> {
    let (applied, requested) = match s.pending_pair() {
        Some(p) => p,
        None => return None,
    };
    let mut act = None;
    egui::Frame::popup(ui.style()).fill(c.bg_raised).show(ui, |ui| {
        ui.label(egui::RichText::new("volume limited").strong().color(c.warning));
        ui.label(egui::RichText::new(format!(
            "asked for {requested}, held at {applied} — sustained listening above this level \
damages hearing")).small());
        ui.horizontal(|ui| {
            if ui.button("keep the limit").clicked() { act = Some(ToastAction::Dismiss); }
            // NOT styled as the primary action. The QML calls this unlock
            // "deliberately awkward", and a warning whose accept button is the
            // prettiest thing on screen is a warning that gets accepted.
            if ui.small_button("unlock for this session").clicked() {
                act = Some(ToastAction::Unlock);
            }
        });
    });
    act
}

#[cfg(test)]
mod toast_ui_tests {
    use super::*;

    #[test]
    fn the_toast_is_absent_when_nothing_is_clamped() {
        // Not an empty frame: a toast that renders its own chrome with no
        // message is a permanent band across the UI.
        let s = Safety::default();
        assert!(s.pending_pair().is_none());
    }

    #[test]
    fn the_unlock_is_not_the_primary_action() {
        // The module's own doc calls the unlock "deliberately awkward". A
        // warning whose accept button is the prettiest thing on screen is a
        // warning that gets accepted.
        let src = include_str!("safety.rs");
        // The QUOTED label, not the phrase: two doc comments above the
        // renderer contain the same words, and matching prose found the first
        // of those and failed a correct button.
        let quoted = format!("{}unlock for this session{}", '"', '"');
        let at = src.find(quoted.as_str()).expect("the label moved");
        let line_start = src[..at].rfind('\n').unwrap_or(0);
        assert!(src[line_start..at].contains("small_button"),
                "the unlock is rendered as a full-weight button");
    }

    #[test]
    fn the_toast_reports_the_press_and_does_not_apply_the_volume() {
        // unlock() guards against "nothing pending" so a stray Unlock cannot
        // raise the volume on its own. A renderer that applied the value
        // itself would need that guard a second time.
        let src = include_str!("safety.rs");
        let at = src.find("pub fn safety_toast_ui").unwrap();
        let body = &src[at..src[at..].find("\n}\n").map(|i| at + i).unwrap_or(src.len())];
        assert!(!body.contains(".unlock()"), "the renderer applies the unlock itself");
    }
}
