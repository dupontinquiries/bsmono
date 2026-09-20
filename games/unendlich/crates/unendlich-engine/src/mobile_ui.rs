//! mobile_ui — is this a phone, how big should the UI be, and where do the
//! on-screen controls go. No window in it.
//!
//! On mobile Linux (postmarketOS/Phosh, Plasma Mobile, SXMO) the game came up
//! at `ui_scale` 2.0 whatever the panel was — the r200 GUI-scale slider is the
//! only way to change it, and it is unreachable behind a 5 mm hotbar — and
//! with no touch controls at all: `touch_controls` has the model, `InputCache`
//! has the seam, and the fullgame bin never fed either. GLFW has no touch API;
//! the compositor emulates a mouse from the first finger, so every tap was a
//! click that punched the nearest creature.
//!
//! Two pure pieces:
//!
//! * [`detect`] — from what the platform CAN tell us (GLFW content scale,
//!   framebuffer size, the session's environment) decide `ui_scale` and
//!   whether to run the touch controls. Explicit env overrides win; a saved
//!   setting wins over detection; detection is the default.
//! * [`TouchLayout`] — the on-screen stick zone and action buttons in
//!   framebuffer pixels, sized by the scale so a button is a thumb-sized
//!   target on any DPI, with [`TouchLayout::hit`] for the pointer-as-touch
//!   adapter. Buttons map to the SAME game actions the keyboard binds
//!   (`ButtonKind::action`), so pressing one is a key press.
//!
//! Pure, deterministic, no panics.

/// What the platform layer could find out. Everything optional: a missing
/// hint is "unknown", never a guess.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hints {
    /// GLFW `get_content_scale().0` (1.0 on an unscaled desktop, 2.0 on a
    /// Phosh phone with scale 2).
    pub content_scale: Option<f32>,
    /// Framebuffer size in pixels.
    pub fb: Option<(u32, u32)>,
    /// `UNENDLICH_UI_SCALE` — an explicit override.
    pub env_ui_scale: Option<f32>,
    /// `UNENDLICH_TOUCH` — "1"/"0" forces touch controls on/off.
    pub env_touch: Option<bool>,
    /// `GDK_SCALE` / `QT_SCALE_FACTOR` if set (a desktop already scaled).
    pub toolkit_scale: Option<f32>,
    /// `XDG_CURRENT_DESKTOP` / `DESKTOP_SESSION`, lower-cased.
    pub desktop: Option<String>,
    /// `XDG_SESSION_TYPE` ("wayland" / "x11" / "tty").
    pub session_type: Option<String>,
    /// A saved `gui_scale` from settings, if the player ever set one.
    pub saved_ui_scale: Option<f32>,
}

/// The decision, with the rule that made it (for the log).
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub ui_scale: f32,
    pub touch: bool,
    pub phone: bool,
    pub reason: &'static str,
}

/// The UI-scale range the r200 slider allows.
pub const UI_SCALE_MIN: f32 = 1.0;
pub const UI_SCALE_MAX: f32 = 4.0;
/// A framebuffer whose shorter side is under this is treated as a phone.
pub const PHONE_SHORT_SIDE_PX: u32 = 1200;
/// Extra scale on top of the compositor's for touch targets.
pub const TOUCH_SCALE_BOOST: f32 = 1.25;

fn clamp_scale(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(UI_SCALE_MIN, UI_SCALE_MAX)
    } else {
        2.0
    }
}

/// Does the desktop string name a mobile shell?
/// Is this a shell we KNOW is a desktop?
///
/// The inverse of [`is_mobile_shell`] is not this: an unset or unfamiliar
/// `XDG_CURRENT_DESKTOP` is unknown, not desktop. Naming the desktops
/// explicitly is what lets a small window on a laptop be treated as a small
/// window rather than as a phone, without that decision leaking onto a phone
/// whose shell we have not heard of.
pub fn is_desktop_shell(desktop: &str) -> bool {
    let d = desktop.to_ascii_lowercase();
    ["gnome", "kde", "plasma", "xfce", "sway", "hyprland", "i3", "cinnamon",
     "mate", "lxqt", "budgie", "unity", "x-cinnamon", "wayfire", "river"]
        .iter()
        // `plasma` matches plasma-mobile too, so a mobile shell always wins:
        // the caller checks `is_mobile_shell` first and this never sees it.
        .any(|k| d.contains(k))
}

pub fn is_mobile_shell(desktop: &str) -> bool {
    let d = desktop.to_ascii_lowercase();
    ["phosh", "plasma-mobile", "plasma mobile", "sxmo", "lomiri", "phoc", "gnome-mobile"]
        .iter()
        .any(|k| d.contains(k))
}

/// Decide `ui_scale` and touch mode. Precedence:
/// 1. `UNENDLICH_UI_SCALE` / `UNENDLICH_TOUCH` (explicit, each independently).
/// 2. A saved `gui_scale` keeps its value; touch still auto-detects.
/// 3. A mobile shell, or a portrait/small framebuffer: phone → touch on,
///    scale = content_scale × boost (content scale 1 → 2.5).
/// 4. Otherwise: scale from the content/toolkit scale (≥1), touch off.
pub fn detect(h: &Hints) -> Detection {
    let phone_by_shell = h.desktop.as_deref().map_or(false, is_mobile_shell);
    let phone_by_fb = match h.fb {
        Some((w, ha)) if w > 0 && ha > 0 => {
            let short = w.min(ha);
            let portrait = ha > w;
            // portrait is the strong signal; a landscape phone (2340×1080) is
            // indistinguishable from a 1080p desktop by size, so it needs
            // the shell hint. Tiny landscape panels (handhelds) still count.
            // A SMALL WINDOW IS NOT A PHONE. `short < 600` fired on any
            // laptop window under 600px tall, which is how touch controls
            // appeared "on a small window on my laptop with no touch monitor"
            // — and with them the stuck mine button, since a mouse cannot
            // release a touch it never started.
            //
            // A framebuffer is a WINDOW on a desktop and a PANEL on a phone,
            // and nothing about its size tells the two apart. The shell does,
            // so on a known desktop the size heuristic is skipped entirely.
            // An unknown shell keeps the old behaviour: a phone we have not
            // heard of should still get its controls, and a desktop we have
            // not heard of can set UNENDLICH_TOUCH=0.
            let known_desktop = h.desktop.as_deref().map_or(false, is_desktop_shell);
            if known_desktop {
                false
            } else {
                (portrait && short < PHONE_SHORT_SIDE_PX) || short < 600
            }
        }
        _ => false,
    };
    let phone = phone_by_shell || phone_by_fb;
    let cs = h.content_scale.filter(|s| s.is_finite() && *s > 0.0);
    let ts = h.toolkit_scale.filter(|s| s.is_finite() && *s > 0.0);
    let platform_scale = cs.or(ts).unwrap_or(1.0).max(1.0);

    let touch = match h.env_touch {
        Some(t) => t,
        None => phone,
    };
    let (ui_scale, reason) = if let Some(s) = h.env_ui_scale.filter(|s| s.is_finite()) {
        (clamp_scale(s), "UNENDLICH_UI_SCALE")
    } else if let Some(s) = h.saved_ui_scale.filter(|s| s.is_finite() && *s > 0.0) {
        (clamp_scale(s), "saved gui_scale")
    } else if phone {
        // a phone at content scale 1 still needs big targets
        (clamp_scale((platform_scale * TOUCH_SCALE_BOOST).max(2.5)), if phone_by_shell { "mobile shell" } else { "small/portrait framebuffer" })
    } else if cs.is_some() {
        (clamp_scale(platform_scale), "content scale")
    } else if ts.is_some() {
        (clamp_scale(platform_scale), "toolkit scale")
    } else {
        (2.0, "default")
    };
    Detection { ui_scale, touch, phone, reason }
}

/// The on-screen buttons. Each maps to a game action the keyboard also binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonKind {
    Jump,
    Attack,
    Use,
    Sprint,
    Sneak,
    Inventory,
    Menu,
}

impl ButtonKind {
    pub const ALL: [ButtonKind; 7] = [
        ButtonKind::Jump,
        ButtonKind::Attack,
        ButtonKind::Use,
        ButtonKind::Sprint,
        ButtonKind::Sneak,
        ButtonKind::Inventory,
        ButtonKind::Menu,
    ];

    /// One-glyph label for the button face.
    pub fn glyph(self) -> &'static str {
        match self {
            ButtonKind::Jump => "▲",
            ButtonKind::Attack => "⚔",
            ButtonKind::Use => "✦",
            ButtonKind::Sprint => "»",
            ButtonKind::Sneak => "▾",
            ButtonKind::Inventory => "▦",
            ButtonKind::Menu => "≡",
        }
    }

    /// Held while the finger is down (movement modifiers, attack) vs. a tap.
    pub fn is_held(self) -> bool {
        matches!(self, ButtonKind::Jump | ButtonKind::Attack | ButtonKind::Sprint | ButtonKind::Sneak)
    }
}

/// A circle in framebuffer pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

impl Circle {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (dx, dy) = (x - self.cx, y - self.cy);
        dx * dx + dy * dy <= self.r * self.r
    }
}

/// Where the controls are.
#[derive(Debug, Clone, PartialEq)]
pub struct TouchLayout {
    /// The rest position of the virtual stick (a hint; the real stick
    /// anchors where the thumb lands — `touch_controls`).
    pub stick_home: Circle,
    pub buttons: Vec<(ButtonKind, Circle)>,
    /// Pixels per point used, so a painter can scale glyphs.
    pub scale: f32,
}

/// Thumb-sized button radius in points.
pub const BUTTON_RADIUS_PT: f32 = 26.0;
pub const STICK_RADIUS_PT: f32 = 44.0;
pub const EDGE_MARGIN_PT: f32 = 22.0;

/// Lay the controls out for a `w`×`h` framebuffer at `ui_scale`
/// pixels-per-point. Stick home bottom-left; a 2×2 action cluster bottom-
/// right (Jump / Attack on the outer column, Use / Sprint inside); Sneak
/// under the stick; Inventory and Menu along the top-right edge. Every
/// circle stays inside the framebuffer for any size ≥ 200 px.
pub fn layout(w: f32, h: f32, ui_scale: f32) -> TouchLayout {
    let s = clamp_scale(ui_scale);
    let w = if w.is_finite() { w.max(200.0) } else { 800.0 };
    let h = if h.is_finite() { h.max(200.0) } else { 600.0 };
    let r = BUTTON_RADIUS_PT * s;
    let m = EDGE_MARGIN_PT * s;
    let sr = STICK_RADIUS_PT * s;
    // shrink if the screen is too small for the nominal sizes
    let fit = (w / (2.0 * (sr + m) + 4.0 * r + 2.0 * m)).min(h / (sr * 2.0 + 4.0 * r + 3.0 * m)).min(1.0);
    let (r, m, sr) = (r * fit, m * fit, sr * fit);
    let stick_home = Circle { cx: m + sr, cy: h - m - sr - 2.0 * r - m * 0.5, r: sr };
    let gap = 2.2 * r;
    let bx = w - m - r; // outer column
    let by = h - m - r; // bottom row
    let buttons = vec![
        (ButtonKind::Attack, Circle { cx: bx, cy: by, r }),
        (ButtonKind::Jump, Circle { cx: bx, cy: by - gap, r }),
        (ButtonKind::Use, Circle { cx: bx - gap, cy: by, r }),
        (ButtonKind::Sprint, Circle { cx: bx - gap, cy: by - gap, r }),
        (ButtonKind::Sneak, Circle { cx: m + sr, cy: h - m - r, r: r * 0.85 }),
        (ButtonKind::Inventory, Circle { cx: w - m - r, cy: m + r, r: r * 0.85 }),
        (ButtonKind::Menu, Circle { cx: w - m - r - gap, cy: m + r, r: r * 0.85 }),
    ];
    TouchLayout { stick_home, buttons, scale: s * fit }
}

impl TouchLayout {
    /// The button under `(x, y)`, if any.
    pub fn hit(&self, x: f32, y: f32) -> Option<ButtonKind> {
        self.buttons.iter().find(|(_, c)| c.contains(x, y)).map(|(k, _)| *k)
    }
}

/// The pointer-as-touch adapter: GLFW gives one emulated mouse for the first
/// finger. A press on a button owns that button until release; any other
/// press is the one finger `touch_controls` sees (stick on the left half,
/// look on the right). Feed it the raw pointer events; read what to do.
#[derive(Debug, Clone, Default)]
pub struct PointerTouch {
    pub down: bool,
    pub held_button: Option<ButtonKind>,
    /// Buttons pressed this frame (edge), for tap-style actions.
    pub tapped: Vec<ButtonKind>,
    /// Buttons released this frame.
    pub released: Vec<ButtonKind>,
    /// Touch events to feed `touch_controls` this frame.
    pub touches: Vec<crate::touch_controls::Touch>,
    last: (f32, f32),
}

impl PointerTouch {
    pub fn press(&mut self, x: f32, y: f32, layout: &TouchLayout) {
        if self.down {
            // A SECOND PRESS PROVES THE RELEASE WAS LOST. Returning early
            // here made `down` a latch: GLFW delivers no mouse-up if the
            // window loses focus mid-drag, and after that every press was
            // ignored and `held_button` stayed held forever. Reported as
            // "the mine block button was stuck".
            //
            // A pointer cannot be pressed twice without an intervening
            // release, so this is not a state to defend against — it is
            // evidence that the release happened somewhere we did not see.
            // Treat it as one and carry on, rather than wedging until restart.
            self.cancel();
        }
        self.down = true;
        self.last = (x, y);
        match layout.hit(x, y) {
            Some(b) => {
                self.held_button = Some(b);
                self.tapped.push(b);
            }
            None => self.touches.push(crate::touch_controls::Touch { id: 1, x, y, phase: crate::touch_controls::Phase::Start }),
        }
    }

    pub fn moved(&mut self, x: f32, y: f32) {
        if !self.down || self.held_button.is_some() {
            return;
        }
        if (x - self.last.0).abs() < 0.01 && (y - self.last.1).abs() < 0.01 {
            return;
        }
        self.last = (x, y);
        self.touches.push(crate::touch_controls::Touch { id: 1, x, y, phase: crate::touch_controls::Phase::Move });
    }

    pub fn release(&mut self) {
        if !self.down {
            return;
        }
        self.down = false;
        match self.held_button.take() {
            Some(b) => self.released.push(b),
            None => self.touches.push(crate::touch_controls::Touch {
                id: 1,
                x: self.last.0,
                y: self.last.1,
                phase: crate::touch_controls::Phase::End,
            }),
        }
    }

    /// Let go of everything, as if the pointer had been released off-window.
    ///
    /// For the events that end a drag without a mouse-up: focus loss, a
    /// window unmap, a modal taking over. The held button is RELEASED rather
    /// than dropped, so a consumer holding "mine" sees it stop instead of
    /// mining forever.
    pub fn cancel(&mut self) {
        if !self.down {
            return;
        }
        self.down = false;
        if let Some(b) = self.held_button.take() {
            self.released.push(b);
        } else {
            self.touches.push(crate::touch_controls::Touch {
                id: 1,
                x: self.last.0,
                y: self.last.1,
                phase: crate::touch_controls::Phase::End,
            });
        }
    }

    /// Drain the per-frame lists after consumers read them.
    pub fn end_frame(&mut self) {
        self.tapped.clear();
        self.released.clear();
        self.touches.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phosh_phone_gets_touch_and_a_big_scale() {
        let d = detect(&Hints {
            content_scale: Some(2.0),
            fb: Some((1080, 2340)),
            desktop: Some("Phosh:GNOME".into()),
            session_type: Some("wayland".into()),
            ..Default::default()
        });
        assert!(d.touch && d.phone);
        assert!((d.ui_scale - 2.5).abs() < 1e-5, "{d:?}");
        assert_eq!(d.reason, "mobile shell");
        // a portrait framebuffer with no shell hint is still a phone
        let d = detect(&Hints { content_scale: Some(1.0), fb: Some((720, 1440)), ..Default::default() });
        assert!(d.phone && d.touch && d.ui_scale >= 2.5);
        assert_eq!(d.reason, "small/portrait framebuffer");
    }

    #[test]
    fn a_desktop_follows_its_content_scale_and_never_gets_touch() {
        let d = detect(&Hints { content_scale: Some(1.0), fb: Some((2560, 1440)), desktop: Some("GNOME".into()), ..Default::default() });
        assert!(!d.touch && !d.phone);
        assert_eq!(d.ui_scale, 1.0);
        assert_eq!(d.reason, "content scale");
        let hi = detect(&Hints { content_scale: Some(2.0), fb: Some((3840, 2160)), ..Default::default() });
        assert_eq!(hi.ui_scale, 2.0);
        let tk = detect(&Hints { toolkit_scale: Some(1.5), fb: Some((1920, 1080)), ..Default::default() });
        assert_eq!((tk.ui_scale, tk.reason), (1.5, "toolkit scale"));
        let none = detect(&Hints::default());
        assert_eq!((none.ui_scale, none.reason, none.touch), (2.0, "default", false));
        // a small landscape laptop panel is not a phone
        let lap = detect(&Hints { content_scale: Some(1.0), fb: Some((1366, 768)), ..Default::default() });
        assert!(!lap.phone, "{lap:?}");
    }

    #[test]
    fn explicit_env_and_saved_settings_win_in_that_order() {
        let base = Hints { content_scale: Some(2.0), fb: Some((1080, 2340)), desktop: Some("phosh".into()), ..Default::default() };
        let env = detect(&Hints { env_ui_scale: Some(1.5), env_touch: Some(false), ..base.clone() });
        assert_eq!((env.ui_scale, env.touch, env.reason), (1.5, false, "UNENDLICH_UI_SCALE"));
        let saved = detect(&Hints { saved_ui_scale: Some(3.0), ..base.clone() });
        assert_eq!((saved.ui_scale, saved.reason), (3.0, "saved gui_scale"));
        assert!(saved.touch, "a saved scale does not turn touch off on a phone");
        let both = detect(&Hints { env_ui_scale: Some(9.0), saved_ui_scale: Some(3.0), ..base });
        assert_eq!((both.ui_scale, both.reason), (UI_SCALE_MAX, "UNENDLICH_UI_SCALE"));
        let nan = detect(&Hints { env_ui_scale: Some(f32::NAN), content_scale: Some(f32::INFINITY), ..Default::default() });
        assert!(nan.ui_scale.is_finite());
    }

    #[test]
    fn layout_fits_any_screen_and_buttons_do_not_overlap() {
        for &(w, h, s) in &[(1080.0, 2340.0, 2.5), (2340.0, 1080.0, 2.5), (800.0, 480.0, 2.0), (200.0, 200.0, 4.0), (1920.0, 1080.0, 1.0), (f32::NAN, -5.0, f32::NAN)] {
            let l = layout(w, h, s);
            let (w, h) = (if w.is_finite() { w.max(200.0) } else { 800.0 }, if h.is_finite() { h.max(200.0) } else { 600.0 });
            let inside = |c: &Circle| c.cx - c.r >= -1e-3 && c.cy - c.r >= -1e-3 && c.cx + c.r <= w + 1e-3 && c.cy + c.r <= h + 1e-3;
            assert!(inside(&l.stick_home), "stick off screen at {w}x{h}: {:?}", l.stick_home);
            for (k, c) in &l.buttons {
                assert!(inside(c), "{k:?} off screen at {w}x{h}: {c:?}");
                assert!(c.r > 4.0);
            }
            for i in 0..l.buttons.len() {
                for j in i + 1..l.buttons.len() {
                    let (a, b) = (l.buttons[i].1, l.buttons[j].1);
                    let d = ((a.cx - b.cx).powi(2) + (a.cy - b.cy).powi(2)).sqrt();
                    assert!(d >= a.r + b.r - 1e-3, "{:?} overlaps {:?} at {w}x{h}", l.buttons[i].0, l.buttons[j].0);
                }
            }
            assert_eq!(l.buttons.len(), ButtonKind::ALL.len());
        }
        // on a phone at 2.5 a button is a thumb: ≥ 60 px across
        let l = layout(1080.0, 2340.0, 2.5);
        assert!(l.buttons[0].1.r * 2.0 >= 60.0);
    }

    #[test]
    fn hit_finds_the_button_under_the_finger_and_nothing_elsewhere() {
        let l = layout(1080.0, 2340.0, 2.5);
        let (k, c) = l.buttons[0];
        assert_eq!(l.hit(c.cx, c.cy), Some(k));
        assert_eq!(l.hit(c.cx + c.r * 0.7, c.cy), Some(k));
        assert_eq!(l.hit(540.0, 1170.0), None, "screen centre is free");
    }

    #[test]
    fn pointer_adapter_owns_a_button_press_and_routes_the_rest_to_touch() {
        use crate::touch_controls::Phase;
        let l = layout(1080.0, 2340.0, 2.5);
        let mut p = PointerTouch::default();
        // press on the Jump button: a tap, no touch event, moves ignored
        let (_, jump) = l.buttons.iter().find(|(k, _)| *k == ButtonKind::Jump).copied().unwrap();
        p.press(jump.cx, jump.cy, &l);
        p.moved(jump.cx + 300.0, jump.cy);
        assert_eq!(p.tapped, vec![ButtonKind::Jump]);
        assert!(p.touches.is_empty() && p.held_button == Some(ButtonKind::Jump));
        p.release();
        assert_eq!(p.released, vec![ButtonKind::Jump]);
        assert!(p.held_button.is_none() && !p.down);
        p.end_frame();
        // press in the open: Start, Move, End touches
        p.press(200.0, 1500.0, &l);
        p.moved(260.0, 1480.0);
        p.moved(260.0, 1480.0); // no motion: no event
        p.release();
        let phases: Vec<Phase> = p.touches.iter().map(|t| t.phase).collect();
        assert_eq!(phases, vec![Phase::Start, Phase::Move, Phase::End]);
        assert!(p.touches.iter().all(|t| t.id == 1));
        // a second press while down is ignored
        p.end_frame();
        p.press(100.0, 100.0, &l);
        p.press(900.0, 900.0, &l);
        assert_eq!(p.touches.len(), 1);
    }

    #[test]
    fn mobile_shells_are_recognised() {
        for d in ["Phosh:GNOME", "KDE:plasma-mobile", "sxmo", "Lomiri", "GNOME"] {
            let m = is_mobile_shell(d);
            assert_eq!(m, d != "GNOME", "{d}");
        }
    }

    /// A SMALL WINDOW IS NOT A PHONE. Reported: touch controls "appeared on a
    /// small window on my laptop with no touch monitor".
    #[test]
    fn a_small_window_on_a_desktop_shell_gets_no_touch_controls() {
        for shell in ["GNOME", "KDE", "sway", "Hyprland", "XFCE"] {
            let d = detect(&Hints {
                fb: Some((900, 540)),           // under the old `short < 600`
                desktop: Some(shell.to_ascii_lowercase()),
                ..Default::default()
            });
            assert!(!d.touch, "{shell}: a 900x540 window is a window, not a phone");
        }
    }

    /// A phone still gets them -- the fix must not cure the false positive by
    /// creating a false negative.
    #[test]
    fn a_phone_still_gets_touch_controls() {
        let phosh = detect(&Hints {
            fb: Some((720, 1440)),
            desktop: Some("phosh".into()),
            ..Default::default()
        });
        assert!(phosh.touch, "a mobile shell must still be a phone");
        // And an UNKNOWN shell keeps the size heuristic: a phone we have not
        // heard of should still work out of the box.
        let unknown = detect(&Hints { fb: Some((720, 1440)), desktop: None, ..Default::default() });
        assert!(unknown.touch, "portrait and small, on an unnamed shell");
    }

    /// The env override still wins over all of it, in both directions.
    #[test]
    fn the_env_override_beats_the_shell() {
        let forced_on = detect(&Hints {
            fb: Some((1920, 1080)), desktop: Some("gnome".into()),
            env_touch: Some(true), ..Default::default()
        });
        assert!(forced_on.touch, "UNENDLICH_TOUCH=1 on a desktop");
        let forced_off = detect(&Hints {
            fb: Some((720, 1440)), desktop: Some("phosh".into()),
            env_touch: Some(false), ..Default::default()
        });
        assert!(!forced_off.touch, "UNENDLICH_TOUCH=0 on a phone");
    }

    /// THE STUCK BUTTON. `press` returned early while `down`, so one lost
    /// mouse-up latched the button and every later press was ignored.
    #[test]
    fn a_lost_release_does_not_wedge_the_pointer() {
        let l = layout(1080.0, 2340.0, 2.5);
        let Some((_, c)) = l.buttons.first() else { return };
        let (bx, by) = (c.cx, c.cy);
        let mut p = PointerTouch::default();
        p.press(bx, by, &l);
        assert!(p.held_button.is_some(), "the press took the button");
        p.end_frame();
        // The mouse-up never arrives -- focus went elsewhere mid-drag.
        // The NEXT press must work rather than being swallowed forever.
        p.press(bx, by, &l);
        assert!(p.held_button.is_some(), "the second press was ignored: wedged");
        assert!(!p.released.is_empty() || !p.tapped.is_empty(),
                "the recovered press must be observable");
    }

    /// `cancel` RELEASES the held button rather than dropping it, so a
    /// consumer holding "mine" sees it stop instead of mining forever.
    #[test]
    fn cancelling_a_drag_reports_the_release() {
        let l = layout(1080.0, 2340.0, 2.5);
        let Some((_, c)) = l.buttons.first() else { return };
        let b = (c.cx, c.cy);
        let mut p = PointerTouch::default();
        p.press(b.0, b.1, &l);
        let held = p.held_button;
        p.end_frame();
        p.cancel();
        assert!(!p.down);
        assert!(p.held_button.is_none(), "nothing is still held");
        assert_eq!(p.released.first().copied(), held,
                   "the button must be RELEASED, not silently dropped");
    }

    /// Cancelling when nothing is down is a no-op, not a spurious release.
    #[test]
    fn cancelling_an_idle_pointer_reports_nothing() {
        let mut p = PointerTouch::default();
        p.cancel();
        assert!(p.released.is_empty() && p.touches.is_empty());
    }
}
