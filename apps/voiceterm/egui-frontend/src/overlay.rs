//! overlay.rs — how each transient surface DRAWS: the QML's modal forms.
//!
//! `surface.rs` is the which; this is the how. Every transient surface renders
//! through [`show`], which wraps egui's built-in [`egui::Modal`]: a backdrop
//! that dims and blocks pointer input, click-outside closes, Esc closes (and
//! is CONSUMED from the event list, so the ESC byte cannot leak into the PTY
//! drain — see `InputState::consume_key`, which `Modal::should_close` uses).
//!
//! The one thing the backdrop cannot do is capture the KEYBOARD: the terminal
//! reads raw `ctx.input` events, and egui does not remove events because some
//! widget looked at them. That half of modality is a state gate —
//! [`crate::surface::Surfaces::captures_input`] — at the `drain_egui` call
//! site. Scrim for the pointer, gate for the keys; both are needed, and only
//! together do they equal what QML's `modal: true` gives for free.
//!
//! ## Tuning on device
//!
//! Everything you would want to nudge while holding the phone is in one
//! place: the [`form`] table (which edge, how wide, QML source lines cited),
//! and the two constants below it. Change a row, rebuild, look. The
//! `qml_still_says_what_the_table_copies` test pins each cited number to the
//! QML text, so if main.qml moves, the drift shows up as a named failure here
//! rather than as silent disagreement between port and original.

use crate::surface::Surface;
use egui::{Align2, Color32, Id, Vec2};

/// The render form of one surface: which edge it enters from, and its size
/// policy. Fractions are of the screen dimension, caps are points — the QML
/// pattern `Math.min(cap, root.dim * frac)` throughout.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Form {
    LeftDrawer { max_w: f32, frac: f32 },
    RightDrawer { max_w: f32, frac: f32 },
    /// Full-width, slides up from the bottom edge.
    BottomSheet { max_h: f32, frac: f32 },
    /// Centered, no slide; the backdrop fade is its whole entrance.
    CenterPopup { w: f32, max_h: f32 },
}

/// THE TABLE. One row per surface, each with its QML source.
pub fn form(s: Surface) -> Form {
    match s {
        // blocksDrawer main.qml:30637 — width: Math.min(540, root.width*0.95), LeftEdge
        Surface::Blocks => Form::LeftDrawer { max_w: 540.0, frac: 0.95 },
        // missingPkgDrawer main.qml:30036 — Math.min(420, root.width * 0.85), RightEdge
        Surface::MissingPkg => Form::RightDrawer { max_w: 420.0, frac: 0.85 },
        // No QML counterpart (the QML has a workspaces PAGE, not a drawer).
        // Chosen to match missingPkg's right-edge shape; adjust freely.
        Surface::Workspaces => Form::RightDrawer { max_w: 420.0, frac: 0.85 },
        // settingsPopup main.qml:27729 — width: 320, height: Math.min(root.height - 80, 760).
        // The QML also anchors it near the nav bar (x depends on _navAtTop);
        // centered here until a device says otherwise.
        Surface::Settings => Form::CenterPopup { w: 320.0, max_h: 760.0 },
        // favoritesPopup main.qml:30479 — width: Math.min(380, root.width - 32), centered
        Surface::Favorites => Form::CenterPopup { w: 380.0, max_h: 640.0 },
        // No QML counterpart — this surface is new. A centred popup like
        // settings, and NARROWER: it holds a handful of rows for the
        // current site plus the globals, and a 320-wide panel of four
        // toggles reads as a dialog rather than a page of preferences.
        Surface::WebSettings => Form::CenterPopup { w: 340.0, max_h: 520.0 },
        // cmdSheetPanel main.qml:1543 — height: Math.min(parent.height * 0.72, 640), full width
        Surface::CmdSheet => Form::BottomSheet { max_h: 640.0, frac: 0.72 },
        // paletteDrawer main.qml:31310 — Math.min(360, root.width*0.85), RightEdge
        Surface::Palette => Form::RightDrawer { max_w: 360.0, frac: 0.85 },
    }
}

/// Drawer/sheet slide duration. QML: `Behavior on position { NumberAnimation
/// { duration: 180; easing.type: Easing.OutCubic } }` (main.qml:30042). The
/// easing is applied at the [`openness`] call via `emath::easing::cubic_out`.
pub const SLIDE_SECS: f32 = 0.18;

/// Backdrop at full openness; multiplied by `t` while sliding. egui Modal's
/// own default (black at 100/255) — the QML `dim: true` color is
/// style-derived, so there is no literal to copy; tune by eye.
pub const BACKDROP: Color32 = Color32::from_black_alpha(100);

/// Resolved content size for a form on a given screen. Pure, unit-tested.
pub fn resolved_size(f: Form, screen: Vec2) -> Vec2 {
    match f {
        Form::LeftDrawer { max_w, frac } | Form::RightDrawer { max_w, frac } => {
            Vec2::new(max_w.min(screen.x * frac), screen.y)
        }
        Form::BottomSheet { max_h, frac } => Vec2::new(screen.x, max_h.min(screen.y * frac)),
        Form::CenterPopup { w, max_h } => {
            // The QML caps the popup's width to the screen (favoritesPopup:
            // root.width - 32) and its height to root.height - 80; keep both
            // so a phone in landscape does not clip.
            Vec2::new(w.min(screen.x - 32.0), max_h.min(screen.y - 80.0))
        }
    }
}

/// Where the surface's anchor sits at openness `t` (0 = fully off-screen,
/// 1 = resting). Pure: the slide travels the size along the entry axis.
pub fn slide_offset(f: Form, size: Vec2, t: f32) -> Vec2 {
    let out = 1.0 - t.clamp(0.0, 1.0);
    match f {
        Form::LeftDrawer { .. } => Vec2::new(-size.x * out, 0.0),
        Form::RightDrawer { .. } => Vec2::new(size.x * out, 0.0),
        Form::BottomSheet { .. } => Vec2::new(0.0, size.y * out),
        Form::CenterPopup { .. } => Vec2::ZERO,
    }
}

fn anchor_of(f: Form) -> Align2 {
    match f {
        Form::LeftDrawer { .. } => Align2::LEFT_TOP,
        Form::RightDrawer { .. } => Align2::RIGHT_TOP,
        Form::BottomSheet { .. } => Align2::CENTER_BOTTOM,
        Form::CenterPopup { .. } => Align2::CENTER_CENTER,
    }
}

/// QML's `Easing.OutCubic`, verbatim: `1 - (1-t)^3`. Defined here because
/// egui 0.34 does not re-export the `emath` crate (only selected items), and
/// adding a whole dependency for one polynomial moves the lockfile for
/// nothing. Byte-for-byte the same curve as `emath::easing::cubic_out`.
fn cubic_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Animated openness for a surface: drive with the state model's answer, get
/// back the 0..=1 the render gate and [`show`] consume. Render while
/// `t > 0.0`, not while `is(s)` — the close animation happens after the state
/// has already said no.
pub fn openness(ctx: &egui::Context, s: Surface, open: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(
        Id::new(("surface-openness", s)),
        open,
        SLIDE_SECS,
        cubic_out,
    )
}

pub struct ShowOut {
    /// Backdrop clicked, a `ui.close()` inside, or Esc (consumed) — the
    /// caller should `surfaces.close()`.
    pub closed: bool,
}

/// Render one surface in its QML form. The content closure is the same body
/// the old SidePanel/Window sites had; only the container changed.
pub fn show(
    ctx: &egui::Context,
    s: Surface,
    t: f32,
    content: impl FnOnce(&mut egui::Ui),
) -> ShowOut {
    let f = form(s);
    let screen = ctx.content_rect().size();
    let size = resolved_size(f, screen);
    let area = egui::Modal::default_area(Id::new(("surface-modal", s)))
        .anchor(anchor_of(f), slide_offset(f, size, t));
    let frame = match f {
        // Edge-flush surfaces: square the corners (QML drawers have none; the
        // cmd sheet's radius-20 top is a device-tuning candidate, not ported).
        Form::LeftDrawer { .. } | Form::RightDrawer { .. } | Form::BottomSheet { .. } => {
            egui::Frame::popup(&ctx.global_style()).corner_radius(0.0)
        }
        Form::CenterPopup { .. } => egui::Frame::popup(&ctx.global_style()),
    };
    let resp = egui::Modal::new(Id::new(("surface", s)))
        .area(area)
        .frame(frame)
        .backdrop_color(BACKDROP.gamma_multiply(t))
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            content(ui);
        });
    ShowOut { closed: resp.should_close() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: Vec2 = Vec2::new(380.0, 800.0);
    const DESKTOP: Vec2 = Vec2::new(1280.0, 800.0);

    /// Every surface has a form; the match in `form` enforces exhaustiveness
    /// at compile time, so this loop exists to exercise `resolved_size` and
    /// `slide_offset` over the full table without naming rows twice.
    #[test]
    fn every_form_resolves_on_phone_and_desktop() {
        let mut rows = 0;
        for s in Surface::ALL {
            for screen in [PHONE, DESKTOP] {
                let size = resolved_size(form(s), screen);
                assert!(size.x > 0.0 && size.y > 0.0, "{s:?} on {screen:?}");
                assert!(size.x <= screen.x && size.y <= screen.y, "{s:?} overflows {screen:?}");
                // fully closed = fully off along the entry axis; open = at rest
                let off = slide_offset(form(s), size, 0.0);
                assert_eq!(slide_offset(form(s), size, 1.0), Vec2::ZERO,
                           "{s:?} must rest at its anchor");
                match form(s) {
                    Form::LeftDrawer { .. } => assert_eq!(off, Vec2::new(-size.x, 0.0)),
                    Form::RightDrawer { .. } => assert_eq!(off, Vec2::new(size.x, 0.0)),
                    Form::BottomSheet { .. } => assert_eq!(off, Vec2::new(0.0, size.y)),
                    Form::CenterPopup { .. } => assert_eq!(off, Vec2::ZERO),
                }
                rows += 1;
            }
        }
        assert_eq!(rows, Surface::ALL.len() * 2, "the loop covered the table");
    }

    #[test]
    fn drawer_caps_and_fractions_match_the_qml_numbers() {
        // blocksDrawer: min(540, 0.95w) → phone 361, desktop 540
        assert_eq!(resolved_size(form(Surface::Blocks), PHONE).x, 380.0 * 0.95);
        assert_eq!(resolved_size(form(Surface::Blocks), DESKTOP).x, 540.0);
        // cmdSheet: min(640, 0.72h) → 576 on both these heights
        assert_eq!(resolved_size(form(Surface::CmdSheet), PHONE).y, 800.0 * 0.72);
        // settings: 320 wide, height capped to screen - 80
        let s = resolved_size(form(Surface::Settings), PHONE);
        assert_eq!((s.x, s.y), (320.0, 720.0));
    }

    /// The table cites main.qml line by line; this pins each cited expression
    /// to the QML text, so a QML change surfaces as a NAMED failure here
    /// instead of the port and the original quietly disagreeing. The count
    /// guard first: a QML restructure that removed every expression would
    /// otherwise pass a loop over nothing.
    #[test]
    fn qml_still_says_what_the_table_copies() {
        let qml = include_str!("../../qml/main.qml");
        let cited = [
            ("blocksDrawer", "Math.min(540, root.width*0.95)"),
            ("missingPkgDrawer", "Math.min(420, root.width * 0.85)"),
            ("paletteDrawer", "Math.min(360,root.width*0.85)"),
            ("settingsPopup width", "width: 320"),
            ("settingsPopup height", "Math.min(root.height - 80, 760)"),
            ("favoritesPopup", "Math.min(380, root.width - 32)"),
            ("cmdSheetPanel", "Math.min(parent.height * 0.72, 640)"),
            ("drawer slide", "duration: 180; easing.type: Easing.OutCubic"),
        ];
        assert_eq!(cited.len(), 8);
        for (what, expr) in cited {
            assert!(
                qml.contains(expr),
                "main.qml no longer contains `{expr}` ({what}) — the QML moved; \
                 update overlay.rs's form table (and its comment) to match, or \
                 this port now differs from the original silently."
            );
        }
    }
}
