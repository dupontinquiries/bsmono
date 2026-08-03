//! mobile_dialog — `MobileDialog`. One dialog that is a window on a desktop and
//! a bottom sheet on a phone.
//!
//! Not a styling preference. The Qt header names the failure it exists to
//! avoid: on mobile Linux, separate OS windows frequently appear BEHIND the
//! main window, lose focus, or never receive keyboard input, because the
//! compositor treats them as floating panels rather than child surfaces. A
//! dialog you cannot see or type into is worse than no dialog. Inlining as a
//! sheet inside the existing window sidesteps all three.
//!
//! Everything else in the port already assumes the sheet form — `modal.rs`
//! presents into layers of one surface rather than opening windows — so this is
//! the piece that says WHEN that assumption is the right one, and what a
//! desktop should do instead.
//!
//! ## The detection ladder, in order
//!
//! 1. **Explicit environment.** `PLASMA_PLATFORM` containing "phone",
//!    `XDG_SESSION_DESKTOP` containing "phosh" or "mobile", `GTK_THEME`
//!    containing "mobile". A compositor that says what it is should be believed
//!    before anything is inferred.
//! 2. **Screen width.** Under 500 logical pixels is a portrait phone. Logical,
//!    not physical — a 1080px-wide phone at 3× is 360 logical pixels, and
//!    testing raw pixels would classify every modern phone as a desktop.
//! 3. **Fullscreen.** A window filling the screen has no room beside it for a
//!    floating dialog regardless of what the device is.
//!
//! Environment first matters: a phone plugged into a monitor still wants
//! sheets, and a 480px desktop window does not stop being a desktop.
//!
//! ## exec() is deliberately not offered
//!
//! Qt's `exec()` spins a nested event loop, which is what makes a dialog block
//! its caller. A sheet has no separate loop to spin, so the API exposes only
//! `open()` — and the Qt header says so explicitly rather than providing an
//! `exec()` that quietly behaves differently on one form factor.

/// What the environment says about the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    pub plasma_platform: String,
    pub xdg_session_desktop: String,
    pub gtk_theme: String,
}

/// The width, in LOGICAL pixels, under which a screen is a portrait phone.
pub const PHONE_MAX_LOGICAL_WIDTH: f32 = 500.0;

/// How a dialog should present itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// A floating window beside the app.
    Window,
    /// A sheet inlined at the bottom of the main window.
    Sheet,
}

impl Form {
    pub fn is_sheet(self) -> bool { matches!(self, Form::Sheet) }
}

/// `isMobileFormFactor`, with every input passed in rather than read from the
/// process — which is what lets the ladder be tested at all.
///
/// `logical_width` is screen width divided by the device pixel ratio.
pub fn form_factor(env: &Environment, logical_width: f32, fullscreen: bool) -> Form {
    let has = |s: &str, needle: &str| s.to_lowercase().contains(needle);

    // 1. A compositor that says what it is gets believed before anything is
    //    inferred — a phone on an external monitor is still a phone.
    if has(&env.plasma_platform, "phone") { return Form::Sheet; }
    if has(&env.xdg_session_desktop, "phosh") { return Form::Sheet; }
    if has(&env.xdg_session_desktop, "mobile") { return Form::Sheet; }
    if has(&env.gtk_theme, "mobile") { return Form::Sheet; }

    // 2. Logical width, not physical: a 1080px phone at 3x is 360 logical
    //    pixels, and testing raw pixels calls every modern phone a desktop.
    if logical_width < PHONE_MAX_LOGICAL_WIDTH { return Form::Sheet; }

    // 3. A window filling the screen has nowhere to put a floating dialog.
    if fullscreen { return Form::Sheet; }

    Form::Window
}

/// How a dialog ended. Mirrors `QDialog::result()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogResult { Accepted, Rejected }

/// The dialog's lifecycle, shared by both forms so a caller's handling cannot
/// depend on which one it got.
#[derive(Debug, Clone, PartialEq)]
pub struct MobileDialog {
    pub form: Form,
    open: bool,
    result: Option<DialogResult>,
    /// Sheet slide progress, 0 closed to 1 fully in.
    t: f32,
}

/// The sheet's entry animation, in milliseconds.
pub const SHEET_MS: f32 = 220.0;

impl MobileDialog {
    pub fn new(form: Form) -> Self {
        MobileDialog { form, open: false, result: None, t: 0.0 }
    }

    /// `open()`. Note there is no `exec()`: it spins a nested event loop, and a
    /// sheet has none to spin. Offering one that behaved differently per form
    /// factor would be worse than not offering it.
    pub fn open(&mut self) {
        self.open = true;
        // The result is cleared on OPEN, not on close. A caller reading
        // result() after close must still see how it ended.
        self.result = None;
        if !self.form.is_sheet() { self.t = 1.0; }
    }

    pub fn is_open(&self) -> bool { self.open }
    pub fn result(&self) -> Option<DialogResult> { self.result }

    /// `accept()` — closes with Accepted, once.
    pub fn accept(&mut self) -> bool { self.finish(DialogResult::Accepted) }
    /// `reject()` — closes with Rejected. Back, Escape and tap-outside all land
    /// here, which is why they need no separate handling.
    pub fn reject(&mut self) -> bool { self.finish(DialogResult::Rejected) }

    fn finish(&mut self, r: DialogResult) -> bool {
        if !self.open { return false; }
        self.open = false;
        self.result = Some(r);
        true
    }

    /// Advance the sheet slide. A window form has no slide — it is either there
    /// or not, because an OS window animating itself in fights the
    /// compositor's own animation.
    pub fn tick(&mut self, dt_ms: f32) {
        if !self.form.is_sheet() { self.t = if self.open { 1.0 } else { 0.0 }; return; }
        let target = if self.open { 1.0 } else { 0.0 };
        let step = dt_ms / SHEET_MS;
        if self.t < target { self.t = (self.t + step).min(target); }
        else if self.t > target { self.t = (self.t - step).max(target); }
    }

    /// How far in the sheet is, eased.
    pub fn slide(&self) -> f32 { uikit_egui::motion::ease::out_cubic(self.t.clamp(0.0, 1.0)) }

    /// Still drawn while sliding out, or the sheet vanishes instead of leaving.
    pub fn drawn(&self) -> bool { self.open || self.t > 0.0 }

    /// Where the sheet sits: full width at the bottom, or a centred panel.
    ///
    /// The sheet is anchored to the BOTTOM because that is where a thumb is;
    /// a centred sheet on a phone puts the buttons in the middle of the screen
    /// and the keyboard covers them when a field is focused.
    pub fn geometry(&self, screen_w: f32, screen_h: f32, content_h: f32) -> (f32, f32, f32, f32) {
        match self.form {
            Form::Sheet => {
                let h = content_h.min(screen_h * 0.9);
                let y = screen_h - h * self.slide();
                (0.0, y, screen_w, h)
            }
            Form::Window => {
                let w = (screen_w * 0.6).min(480.0);
                let h = content_h.min(screen_h * 0.8);
                ((screen_w - w) * 0.5, (screen_h - h) * 0.5, w, h)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_env() -> Environment {
        Environment { plasma_platform: "desktop".into(),
                      xdg_session_desktop: "KDE".into(), gtk_theme: "Breeze".into() }
    }

    #[test]
    fn a_wide_desktop_gets_a_window() {
        assert_eq!(form_factor(&desktop_env(), 1920.0, false), Form::Window);
    }

    #[test]
    fn a_compositor_that_says_it_is_a_phone_is_believed_first() {
        // A phone plugged into a monitor is still a phone, so the environment
        // outranks the width it reports.
        let mut e = desktop_env();
        e.plasma_platform = "phone".into();
        assert_eq!(form_factor(&e, 1920.0, false), Form::Sheet);
    }

    #[test]
    fn phosh_and_mobile_sessions_are_recognised() {
        for session in ["phosh", "Phosh", "gnome-mobile", "MOBILE"] {
            let mut e = desktop_env();
            e.xdg_session_desktop = session.into();
            assert_eq!(form_factor(&e, 1920.0, false), Form::Sheet, "session {session}");
        }
    }

    #[test]
    fn a_mobile_gtk_theme_counts_too() {
        let mut e = desktop_env();
        e.gtk_theme = "adwaita-mobile".into();
        assert_eq!(form_factor(&e, 1920.0, false), Form::Sheet);
    }

    #[test]
    fn the_environment_check_is_case_insensitive() {
        let mut e = desktop_env();
        e.plasma_platform = "PHONE".into();
        assert_eq!(form_factor(&e, 1920.0, false), Form::Sheet);
    }

    #[test]
    fn width_is_measured_in_logical_pixels_not_physical_ones() {
        // A 1080px phone at 3x is 360 logical pixels. Testing raw pixels would
        // classify every modern phone as a desktop and hand it the exact dialog
        // form that breaks there.
        assert_eq!(form_factor(&desktop_env(), 360.0, false), Form::Sheet);
        assert_eq!(form_factor(&desktop_env(), 1080.0, false), Form::Window,
                   "the same phone's PHYSICAL width would be misread");
    }

    #[test]
    fn the_phone_width_threshold_is_where_it_says_it_is() {
        assert_eq!(form_factor(&desktop_env(), PHONE_MAX_LOGICAL_WIDTH - 1.0, false), Form::Sheet);
        assert_eq!(form_factor(&desktop_env(), PHONE_MAX_LOGICAL_WIDTH, false), Form::Window);
    }

    #[test]
    fn a_fullscreen_window_gets_a_sheet_whatever_the_device() {
        // There is nowhere to put a floating dialog beside a window that fills
        // the screen.
        assert_eq!(form_factor(&desktop_env(), 1920.0, true), Form::Sheet);
    }

    #[test]
    fn a_narrow_desktop_window_does_not_make_it_a_phone() {
        // Width is the SCREEN's, not the window's — resizing an app narrow on a
        // desktop should not start inlining its dialogs.
        assert_eq!(form_factor(&desktop_env(), 1920.0, false), Form::Window);
    }

    // ── lifecycle ───────────────────────────────────────────────────────────

    #[test]
    fn accept_and_reject_both_close_and_record_how() {
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        assert!(d.is_open());
        assert_eq!(d.result(), None, "an open dialog has no result yet");
        assert!(d.accept());
        assert!(!d.is_open());
        assert_eq!(d.result(), Some(DialogResult::Accepted));

        let mut e = MobileDialog::new(Form::Sheet);
        e.open();
        e.reject();
        assert_eq!(e.result(), Some(DialogResult::Rejected));
    }

    #[test]
    fn a_closed_dialog_cannot_be_closed_again() {
        // Two accepts would fire the caller's handler twice — a contact saved
        // twice, a snap sent twice.
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        assert!(d.accept());
        assert!(!d.accept());
        assert!(!d.reject(), "and a late reject cannot overwrite the accept");
        assert_eq!(d.result(), Some(DialogResult::Accepted));
    }

    #[test]
    fn the_result_survives_closing_and_is_cleared_by_the_next_open() {
        // A caller reads result() AFTER close; clearing it there would lose the
        // answer it was waiting for.
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        d.accept();
        assert_eq!(d.result(), Some(DialogResult::Accepted));
        d.open();
        assert_eq!(d.result(), None, "a fresh open starts with no answer");
    }

    #[test]
    fn a_dialog_that_was_never_opened_reports_nothing() {
        let mut d = MobileDialog::new(Form::Sheet);
        assert!(!d.accept());
        assert_eq!(d.result(), None);
    }

    #[test]
    fn a_sheet_slides_in_and_stays_drawn_while_it_slides_out() {
        // Dropping it at open=false makes the sheet vanish rather than leave.
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        assert!(d.drawn());
        d.tick(SHEET_MS / 2.0);
        assert!(d.slide() > 0.0 && d.slide() < 1.0);
        d.tick(SHEET_MS);
        assert_eq!(d.slide(), 1.0);
        d.accept();
        d.tick(SHEET_MS / 4.0);
        assert!(d.drawn(), "still leaving");
        d.tick(SHEET_MS);
        assert!(!d.drawn());
    }

    #[test]
    fn a_window_form_does_not_animate_itself() {
        // An OS window sliding itself in fights the compositor's own animation.
        let mut d = MobileDialog::new(Form::Window);
        d.open();
        assert_eq!(d.slide(), 1.0, "there immediately");
        d.accept();
        d.tick(1.0);
        assert_eq!(d.slide(), 0.0);
        assert!(!d.drawn());
    }

    #[test]
    fn a_sheet_is_anchored_to_the_bottom_where_a_thumb_is() {
        // A centred sheet on a phone puts the buttons mid-screen, and the
        // keyboard covers them the moment a field is focused.
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        d.tick(SHEET_MS * 2.0);
        let (x, y, w, h) = d.geometry(400.0, 800.0, 300.0);
        assert_eq!((x, w), (0.0, 400.0), "full width");
        assert!((y + h - 800.0).abs() < 0.01, "flush with the bottom edge");
    }

    #[test]
    fn a_sheet_taller_than_the_screen_is_capped() {
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        d.tick(SHEET_MS * 2.0);
        let (_, _, _, h) = d.geometry(400.0, 800.0, 5000.0);
        assert!(h <= 800.0 * 0.9, "a sheet must not cover the whole app");
    }

    #[test]
    fn a_window_is_centred_and_bounded() {
        let mut d = MobileDialog::new(Form::Window);
        d.open();
        let (x, y, w, h) = d.geometry(1920.0, 1080.0, 400.0);
        assert!(w <= 480.0, "a dialog does not grow with the monitor");
        assert!((x + w / 2.0 - 960.0).abs() < 0.01, "horizontally centred");
        assert!((y + h / 2.0 - 540.0).abs() < 0.01, "vertically centred");
    }

    #[test]
    fn a_mid_slide_sheet_is_still_offscreen_by_the_right_amount() {
        let mut d = MobileDialog::new(Form::Sheet);
        d.open();
        d.tick(SHEET_MS / 2.0);
        let (_, y, _, h) = d.geometry(400.0, 800.0, 300.0);
        assert!(y > 800.0 - h, "part of it is still below the edge");
        assert!(y < 800.0, "and part of it is on screen");
    }
}
