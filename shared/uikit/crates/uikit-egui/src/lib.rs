//! uikit-egui — the shared egui UI kit for the portfolio.
//!
//! Three things live here, each generalized out of an app that already proved
//! it:
//!
//! 1. **The carved widget surface** ([`skin`] + [`carved`]) — ported from
//!    `logscope-egui`. A five-layer sandwich (L0 egui → L1 helpers → L2 carved
//!    painters → L3 semantic widgets → L4 [`Skin`] tokens) so restyling edits
//!    values, never call sites.
//! 2. **The gesture layer** ([`gesture_layer`]) — adapts `egui::InputState` to
//!    `uikit-gesture::TwoTierGesture`, bringing snapclone's FP5-tuned swipe
//!    feel to any egui app with no per-app gesture code.
//! 3. **The theme bridge** ([`theme_bridge`]) — applies a
//!    `snapclone-theme::Palette` to an [`egui::Context`].
//!
//! See `apps/snapclone-egui` for the front-end built on this, and
//! `examples/logscope_touch.rs` for the reverse port (logscope's scene gaining
//! the touch layer).
//!
//! ## `SidePanel::default_width` does not set a width
//!
//! Worth knowing before writing a port comment that claims it does. Measured on
//! egui 0.34, laying out a real frame headlessly on a 420-point surface:
//!
//! ```text
//! .resizable(true).default_width(168.0).width_range(110.0..=340.0)  ->  110
//! .resizable(true).default_width(168.0)                             ->   96
//! ```
//!
//! 110 is the range MINIMUM; 96 is the content width of a one-word label.
//! Stable across three frames, and confirmed both by `response.rect.width()`
//! and by the drop in `Context::available_rect()`.
//!
//! `default_size` — which `default_width` is a deprecated alias for — only
//! WIDENS the size range to admit its argument (egui-0.34 `panel.rs:404`). The
//! panel is content-driven and clamped to that range. So the call is a hint
//! about permitted extent, not a starting width, and a panel written
//! `.default_width(300.0)` will be 300 wide only if its contents happen to be.
//!
//! Use `exact_width` to pin one, at the cost of the drag handle. That trade is
//! the reason this is documented rather than mass-corrected: several call sites
//! in this tree name a width they do not get, and whether each WANTS the pin or
//! the drag is a per-surface decision.
//!
//! Found while checking a voiceterm port comment that cited
//! `main.qml`'s fixed `width: root.termSidebarWidth` (168) against a
//! `.default_width(168.0)` that reads like the same statement and is not one.

pub mod fonts;
pub mod ime;
pub mod input_probe;
pub mod desktop;
pub mod notify;
pub mod single_instance;
pub mod a11y;
pub mod adaptive;
pub mod carved;
pub mod dock;
pub mod vertical_tabs;
pub mod glass;
/// Text and edges on the device-pixel grid (the "tall blurry font" report).
pub mod crisp;
/// Say why the window will not open, before the panic in a dependency does.
pub mod backend_check;
/// The finish snapclone has, for any app: a live backdrop behind the glass panels, one line a frame.
pub mod polish;
/// A theme-aware, custom-drawn QR-shaped identicon and its expanded overlay (`QrCodeRenderer.cpp`, ported).
pub mod qr;
/// A real QR code and sequences of them for data that will not fit in one (feature `qrbig`).
#[cfg(feature = "qrbig")]
pub mod qrbig;
/// A strip that says which step of a sequence you are on (snapclone's send flow, extracted).
pub mod steps;
/// A row of options with the chosen one filled (the Qt duration selector, extracted).
pub mod chips;
pub mod gesture_layer;
pub mod touch;
pub mod zoom;    // digital zoom: a pinch-driven window into a live picture, shared by the camera apps
pub mod skin;
pub mod livebg;
pub mod nav;
pub mod pager;   // pages as data: hysteretic mobile/dashboard plan + the shared swipe (c11)
pub mod icons;   // glyphs that actually render, or an honest word (c11 tofu report)
pub mod path_picker;
/// Reaching the command palette on a touch screen: hold-to-open, plus a
/// bottom strip of recents and quick actions.
pub mod cmdstrip;
/// voiceterm's `settingsPopup`, ported: four doors to other screens plus
/// voice activation and theme. NOT preferences — see the module docs.
pub mod quick_launcher;
pub mod segmented_tabs;
/// Tabs vs. pages, decided from width, as data — not a per-app
/// `if adaptive.is_compact()` guess. See its own module docs.
pub mod responsive_nav;
pub mod scroll;
pub mod fuzzy;
pub mod command_bar;
pub mod focus;
pub mod handed;
pub mod sections;
pub mod two_pane;
pub mod sidebar; // a side column whose width is the user's, never its content's
pub mod layout_probe;
/// Render a frame to pixels in a test and assert properties of the picture.
pub mod uiverify;
/// A UI audit an agent can run in a loop: findings with stable ids, and a fixed/introduced diff.
pub mod uxaudit;
pub mod overlay;
// Text inputs that cannot be built without an id — see the module docs for
// why this is a kit concern and not a per-app one (the single-character
// input bug, found and re-found across 68 call sites).
pub mod text_field;
// Dialog width that fits a phone + in-bounds keyboard list selection. Both
// were re-implemented per app; dialog_width had three copies and tomescope
// had none (it hard-coded a 400pt minimum and overflowed).
pub mod dialog;
// Material model for the physical UI: specular sheen, gilt corner brackets and
// weight-based motion durations. Geometry and maths only — painting stays with
// the caller.
pub mod lux;
pub mod fmt;
pub mod glove;
pub mod upscale;
pub mod menu;
pub mod motion;
pub mod confirm_dialog;
pub mod repl_voice_glow;
pub mod setup_wizard;
pub mod chat_panel;
pub mod model_picker;
pub mod stable_pick;
pub mod log_view;
pub mod mono_tile;
pub mod slide_menu;
pub mod toast;
/// A message with a severity that cannot be mistaken for the text around it:
/// tinted surface, accent stripe, severity border, chip, tagged lines.
pub mod callout;
pub mod split_pane;
pub mod toggle_grid;
pub mod theme_bridge;
/// Tap-vs-hold classification for a key binding that means two different
/// things depending on press duration (Ctrl+P toggling a palette on tap,
/// opening voice on hold, is the first real consumer). Pure, zero-cost,
/// unconditional -- unlike voice_popup below, nothing here pulls in a
/// heavy dependency, so there is no reason to gate it behind a feature.
pub mod ctrl_p_gesture;
/// The idle-dwell state machine that replaced hold-duration as Ctrl+P's
/// voice trigger -- see the module doc for the full flow and why.
/// `ctrl_p_gesture` above is not removed: it is real, tested, reusable
/// keyboard-hold infrastructure over `cmdstrip::Hold`, just no longer
/// what drives voiceterm's own Ctrl+P after this redesign. Left in place
/// rather than deleted on the chance a future binding wants a hold
/// gesture specifically -- flagged here rather than left silently
/// orphaned-looking, since nothing currently calls it.
pub mod palette_flow;
/// The voice popup: tap-to-listen, cancel/finish, drawn state. Gated
/// behind the `voice-popup` feature since it depends on
/// voice-engine-kit's whisper-rs/cpal -- a real cost most apps using this
/// kit have no use for. First moved here from voiceterm-egui
/// (`maintenance/C18_TODO.md` -- the Ctrl+P wiring work), where it lived
/// as a local, non-reusable module before this extraction.
/// Warming/listening indicator with a hold-to-cancel gesture. Deliberately
/// NOT gated behind `voice-popup` -- it depends only on egui, PaletteColors,
/// and cmdstrip::Hold, none of which need voice-engine-kit, so an app
/// drawing its own capture UI can reuse this widget without pulling in
/// whisper-rs/cpal for a widget that never touches either.
pub mod mic_indicator;
#[cfg(feature = "voice-popup")]
pub mod voice_popup;
pub mod tokens;
/// Who owns a keystroke: a focused text field, or an app shortcut.
pub mod typing_guard;

pub use a11y::{Action as A11yAction, A11yScene, Outcome as A11yOutcome, Role, Semantics};
pub use adaptive::{Adaptive, DeviceClass, SafeArea};
pub use glass::{glass_key, surface_fill, key_fill, key_fill_hover, key_fill_modifier, key_fill_modifier_hover, key_fill_pressed, key_border, key_border_soft, opaque_strip_fill, paint_top_hairline, tint as glass_tint, GlassAlphas};
pub use gesture_layer::{Axis, GestureFrame, GestureLayer};
pub use skin::Skin;
pub use livebg::{BgStyle, LiveBackground};
pub use polish::Backdrop;
pub use nav::Nav;
pub use theme_bridge::{apply_palette, filter_color, PaletteColors};

/// The default palette's egui colours (the first palette in the theme set). Replaces the verbose
/// `PaletteColors::from_palette(snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]))`
/// incantation that was copy-pasted into every front-end.
pub fn default_palette() -> PaletteColors {
    PaletteColors::from_palette(snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]))
}

/// The `PaletteColors` for a named palette id, falling back to the default on an unknown name — so a
/// front-end can honour a `prefs.palette` string without re-implementing the lookup.
pub fn palette_colors(id: &str) -> PaletteColors {
    let pid = snapclone_theme::PaletteId::all()
        .iter()
        .copied()
        .find(|p| p.as_str() == id)
        .unwrap_or_else(|| snapclone_theme::PaletteId::all()[0]);
    PaletteColors::from_palette(snapclone_theme::palette_for(pid))
}
/// [`palette_colors`], plus the symbol fonts.
///
/// The safe conversion for the eleven apps `font_install_check.py` names.
///
/// `palette_colors` is pure — no `Context`, which is exactly why twenty-nine
/// apps reach for it and exactly why none of them can install a font through
/// it. The fonts arrive with `apply_palette`, and telling those apps to call
/// that instead is not a one-line change: `apply_palette` also SETS VISUALS,
/// so it recolours an app that was theming itself by hand. That is the reason
/// ten of the eleven are still on the list rather than fixed in a sweep.
///
/// This is the version that changes nothing visible. Same colours out, fonts
/// in, one word different at the call site. An app that wants the full theme
/// should still call `apply_palette`; an app that just wants its arrows to
/// stop being boxes should call this.
pub fn palette_colors_ctx(ctx: &egui::Context, id: &str) -> PaletteColors {
    crate::fonts::install(ctx);
    palette_colors(id)
}

pub use uikit_gesture::{Direction, GestureConfig, Outcome, ReleaseOutcome, TwoTierGesture};

/// Apply the theme + touch parts of cross-app [`settings_kit::Prefs`] to an egui context:
/// select the palette and, if set, widen the minimum interactive size. (The gesture part is
/// applied to a `GestureConfig` via `Prefs::apply_to_gesture`.) Call this after
/// `Settings::poll()` reports a change so the whole UI reflects new prefs live.
/// The smallest an interactive control should be, in points, for this surface.
///
/// Existed only as `TouchPrefs::min_target`, defaulting to 0, which meant the
/// effective floor was egui's desktop-shaped `interact_size.y` of 24pt on
/// every device including a phone. 24pt is a mouse target; the reported
/// "buttons are still a bit small on mobile" is that number, not a theme
/// problem.
///
/// Resolution order, most specific first:
///
/// 1. An explicit `min_target` from settings. A user who has set a number
///    means it, on any device, and nothing here second-guesses them.
/// 2. Otherwise derived from the surface. Both inputs matter and neither is
///    sufficient alone:
///    * `surface_pt` — the usable width in POINTS. Already scale-corrected,
///      so a phone at 300% and a small desktop window both read as narrow,
///      which is right: a cramped window wants bigger targets whatever is
///      driving it.
///    * `ppp` — the compositor's scale factor. A high one on a WIDE surface
///      is a HiDPI desktop, which does not want phone-sized buttons; a high
///      one on a narrow surface is a phone. Using width alone would inflate
///      controls on a tiled desktop window; using ppp alone would inflate
///      them on every Retina display.
///
/// 44pt is the touch figure both Apple's HIG (44pt) and Material (48dp)
/// land near, and is roughly a 9mm fingertip at typical phone densities.
pub fn touch_floor_pt(explicit_min_target: f32, surface_pt: f32, ppp: f32) -> f32 {
    if explicit_min_target > 0.0 {
        return explicit_min_target;
    }
    // A surface this narrow is being held, not pointed at.
    let narrow = surface_pt > 0.0 && surface_pt < 600.0;
    // Fractional scaling below 1.25 is a desktop convention; at or above it
    // the compositor is telling us the pixels are small.
    let dense = ppp >= 1.25;
    if narrow && dense {
        44.0
    } else if narrow {
        // Narrow but not dense: a small desktop window. Some extra room
        // helps, but it is a pointer, so not the full touch figure.
        32.0
    } else {
        24.0 // egui's default: a mouse target on a roomy surface
    }
}

/// The area a surface should lay out inside, with the first-frame case handled.
///
/// `Context::screen_rect` is deprecated: egui split it into `viewport_rect` (everything,
/// including what sits under a status bar or a notch) and `content_rect` (the usable part).
/// Almost everything wants the usable part — a QR code under the status bar is unscannable
/// and a dialog under it is untappable.
///
/// The catch is that `content_rect` is EMPTY on the very first frame, before any layout has
/// happened. Reading zero there and concluding "narrow" is how an app comes up in its
/// compact layout on a desktop and then jumps. So: content when it has been established,
/// the full viewport until then.
pub fn usable_rect(ctx: &egui::Context) -> egui::Rect {
    let c = ctx.content_rect();
    if c.width() > 1.0 && c.height() > 1.0 { c } else { ctx.viewport_rect() }
}

pub fn apply_prefs(ctx: &egui::Context, prefs: &settings_kit::Prefs) {
    let pid = snapclone_theme::PaletteId::from_str(&prefs.palette);
    let colors = apply_palette(ctx, snapclone_theme::palette_for(pid));
    // Display accessibility (color-vision filter / lowered white point): apply_palette above
    // always sets unfiltered Visuals (its own callers rely on that), so when a filter is
    // configured, re-derive Visuals from filtered colors here. Gated on white_point/color_filter
    // specifically (not the broader `is_active()`, which also covers opacity-only prefs) so an
    // opacity-only change doesn't send colors through filter_color's float round-trip for
    // nothing — that roundtrip is a color-preserving identity mathematically, but the
    // u8 -> f32/255.0 -> *255.0 -> u8 conversion truncates rather than rounds, so an
    // unnecessary pass can drift a channel by ±1 for no reason. Default Prefs has both at their
    // neutral value, so this is a no-op for the overwhelming majority of users.
    if prefs.display.white_point < 0.999 || prefs.display.color_filter != settings_kit::ColorFilter::None {
        let filtered = theme_bridge::filter_palette_colors(&colors, &prefs.display);
        ctx.set_visuals(theme_bridge::build_visuals(&filtered));
    }
    let text = prefs.touch.text_scale.max(0.5);
    let btn = prefs.touch.button_scale.max(0.5);
    // content_rect is the usable area (excluding what sits under system
    // bars). It is empty on the very first frame, before any layout has
    // happened, so fall back to the raw screen rect rather than reading 0
    // and concluding "narrow".
    let surface_pt = usable_rect(ctx).width();
    let floor = touch_floor_pt(prefs.touch.min_target, surface_pt, ctx.pixels_per_point());
    ctx.global_style_mut(|style| {
        // text size across the app
        for (_, font) in style.text_styles.iter_mut() {
            font.size *= text;
        }
        // control/button size (respect an explicit min target too)
        style.spacing.button_padding = egui::vec2(8.0 * btn, 4.0 * btn);
        // The floor is derived per-surface rather than a flat 24pt — see
        // touch_floor_pt. `button_scale` still multiplies on top, so the
        // settings app's slider keeps working and now moves a number that
        // already suits the device.
        style.spacing.interact_size.y = (floor * btn).max(prefs.touch.min_target);
        // animation feel: the "snappy / smooth / heavy / reduced" picker. Scales egui's global
        // animation_time so every animate_bool() across the kit respects it (Reduced → 0 = instant).
        style.animation_time = BASE_ANIM_SECS * prefs.gesture.animation.time_scale();
    });
    // surface transparency: make panels/cards translucent so a LiveBackground (or the desktop,
    // if the window is transparent) shows through. Applied once here → every adopter gets it.
    let opacity = prefs.display.opacity.clamp(0.3, 1.0);
    if opacity < 0.999 {
        let a = (opacity * 255.0) as u8;
        ctx.global_style_mut(|style| {
            let v = &mut style.visuals;
            v.panel_fill = with_alpha(v.panel_fill, a);
            v.window_fill = with_alpha(v.window_fill, a);
            v.extreme_bg_color = with_alpha(v.extreme_bg_color, a);
            v.widgets.noninteractive.bg_fill = with_alpha(v.widgets.noninteractive.bg_fill, a);
        });
    }
}

/// The baseline egui `animation_time` at `Smooth` (scale 1.0). Snappy halves it, Heavy lengthens
/// it, Reduced zeroes it.
pub const BASE_ANIM_SECS: f32 = 0.15;

/// Scale an *explicit* animation duration (e.g. a carved sheen's) by the active AnimationStyle, so
/// hard-coded `animate_bool_with_time` durations track the picker too. Returns 0 under Reduced
/// motion (instant). Reads the scale back from the `animation_time` that `apply_prefs` set.
pub fn anim_secs(ctx: &egui::Context, base: f32) -> f32 {
    let scale = ctx.global_style().animation_time / BASE_ANIM_SECS;
    (base * scale).max(0.0)
}

/// Animation duration for a surface of this SIZE, already scaled by the user's
/// motion preference.
///
/// The companion to `anim_secs` for callers that have a rect rather than a
/// number. `anim_secs(ctx, BASE_ANIM_SECS)` gives every surface the same 150ms,
/// and uniform durations are one of the two things that make a UI feel
/// weightless — a toggle and a whole page settling at identical speed is not
/// something any physical object does. `lux::Weight` classifies from area so
/// callers cannot drift apart on what counts as heavy.
///
/// Still honours Reduced motion, because that setting means instant and a
/// weight model must not smuggle animation back in.
pub fn anim_secs_for(ctx: &egui::Context, rect: egui::Rect) -> f32 {
    anim_secs(ctx, lux::Weight::of(rect.width() * rect.height()).secs())
}

/// Replace a colour's alpha (0..255), preserving RGB — the primitive behind translucent surfaces.
pub fn with_alpha(c: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// A translucent version of a colour at `opacity` (0.0..1.0) — for apps drawing their own cards
/// or panels over a live background. Pairs with `DisplayPrefs::opacity`.
pub fn translucent(c: egui::Color32, opacity: f32) -> egui::Color32 {
    with_alpha(c, (opacity.clamp(0.0, 1.0) * 255.0) as u8)
}

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};

// ── L1 · common egui patterns, written once (mirrors logscope ui::row/combo) ──

/// A standard left-to-right control cluster; returns the body's value.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(add).inner
}

/// A labelled dropdown with standard sizing; folds the ComboBox ritual.
pub fn combo(ui: &mut Ui, id: &str, selected: impl Into<String>, width: f32, body: impl FnOnce(&mut Ui)) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected.into())
        .width(width)
        .show_ui(ui, body);
}

/// Paint the detent "barrier" — a ring on `edge` that fills with `fraction`
/// (0..1). This is the egui equivalent of snapclone's PagePeelHint/ChevronConfirm:
/// the visible "almost there" feedback as the accumulator climbs toward a
/// sub-page step. Drive it from [`GestureFrame::barrier_fraction`].
pub fn paint_barrier(ui: &Ui, surface: Rect, fraction: f32, edge: Direction, accent: Color32) {
    if fraction <= 0.01 {
        return;
    }
    let r = 14.0;
    let cx = match edge {
        Direction::Next => surface.right() - 28.0,
        Direction::Prev => surface.left() + 28.0,
    };
    let center = Pos2::new(cx, surface.center().y);
    let painter = ui.painter_at(surface);

    // faint track
    painter.circle_stroke(center, r, Stroke::new(3.0_f32, accent.gamma_multiply(0.25)));
    // filled arc proxied by an alpha-scaled ring (egui has no arc primitive;
    // the app's real impl draws a true arc — this keeps the kit dependency-free)
    painter.circle_stroke(center, r, Stroke::new(3.0 * fraction, accent));

    // a chevron pointing the travel direction, brightening with fraction
    let dir = match edge {
        Direction::Next => 1.0,
        Direction::Prev => -1.0,
    };
    let col = accent.gamma_multiply(0.4 + 0.6 * fraction);
    let s = 5.0;
    let tip = center + Vec2::new(dir * s, 0.0);
    painter.line_segment([center + Vec2::new(-dir * s, -s), tip], Stroke::new(2.0_f32, col));
    painter.line_segment([center + Vec2::new(-dir * s, s), tip], Stroke::new(2.0_f32, col));
}

#[cfg(test)]
mod anim_apply_tests {
    use super::*;
    #[test]
    fn animation_style_drives_context_animation_time() {
        let ctx = egui::Context::default();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Reduced;
        apply_prefs(&ctx, &prefs);
        assert_eq!(ctx.global_style().animation_time, 0.0, "reduced motion is instant");
        assert_eq!(anim_secs(&ctx, 0.25), 0.0, "explicit durations collapse too");
        prefs.gesture.animation = settings_kit::AnimationStyle::Heavy;
        apply_prefs(&ctx, &prefs);
        assert!(ctx.global_style().animation_time > BASE_ANIM_SECS, "heavy lingers");
        assert!(anim_secs(&ctx, 0.25) > 0.25, "carved sheen lengthens under heavy");
    }
}

#[cfg(test)]
mod display_filter_apply_tests {
    use super::*;

    /// The gap this closes: `apply_prefs` called `apply_palette` and threw away the
    /// `PaletteColors` it returned, so `prefs.display.color_filter` / `white_point` were
    /// computed by `theme_bridge::filter_color` (unit-tested in isolation) but never actually
    /// reached the egui `Visuals` a real app paints with. This test goes through the real
    /// entry point (`apply_prefs`, not `filter_color` directly) and checks the *rendered*
    /// override text color, so a future refactor that reintroduces the gap fails here.
    #[test]
    fn grayscale_prefs_reach_the_actual_visuals() {
        let ctx = egui::Context::default();

        // Baseline: no display adjustment, unfiltered accent color.
        let mut prefs = settings_kit::Prefs::default();
        apply_prefs(&ctx, &prefs);
        let unfiltered_text = ctx.style().visuals.override_text_color;

        // A palette's text color is never perfectly gray by coincidence in this kit's palettes,
        // so if grayscale is applied, r/g/b must converge; if it's silently dropped (the bug),
        // this will fail because unfiltered_text and filtered_text will be identical instead.
        prefs.display.color_filter = settings_kit::ColorFilter::Grayscale;
        apply_prefs(&ctx, &prefs);
        let filtered_text = ctx.style().visuals.override_text_color
            .expect("apply_palette always sets an override text color");

        assert!(
            (filtered_text.r() as i32 - filtered_text.g() as i32).abs() <= 1
                && (filtered_text.g() as i32 - filtered_text.b() as i32).abs() <= 1,
            "color_filter = Grayscale must produce a near-gray override text color \
             (got r={}, g={}, b={}) — if this equals the unfiltered color ({:?}), the filter \
             is not reaching Visuals",
            filtered_text.r(), filtered_text.g(), filtered_text.b(), unfiltered_text
        );
    }

    /// Opacity-only prefs (the common case: translucent panels with no color-vision filter)
    /// must NOT be routed through filter_color's float round-trip — that conversion is only a
    /// mathematical no-op, not a bit-exact one (u8 -> f32/255.0 -> *255.0 -> u8 truncates), so
    /// running it unnecessarily can drift a channel by ±1 for no reason.
    #[test]
    fn opacity_only_prefs_do_not_perturb_unrelated_colors() {
        let ctx = egui::Context::default();

        let mut baseline = settings_kit::Prefs::default();
        apply_prefs(&ctx, &baseline);
        let before = ctx.style().visuals.override_text_color;

        baseline.display.opacity = 0.6; // is_active() would be true from this alone
        apply_prefs(&ctx, &baseline);
        let after = ctx.style().visuals.override_text_color;

        assert_eq!(before, after,
            "opacity-only prefs changed the text color ({before:?} -> {after:?}); the color \
             re-derivation should be gated on white_point/color_filter, not opacity");
    }
}

/// The `default_width` measurement in this module's docs, kept honest.
///
/// A documented gotcha is only as good as the version it was measured on. If a
/// future egui makes `default_width` pin the width, this fails and the docs
/// above are wrong rather than merely stale — which is the outcome worth
/// catching, since call sites in this tree are written around the current
/// behaviour.
#[cfg(test)]
mod panel_width_behaviour {
    fn laid_out(build: impl Fn() -> egui::SidePanel) -> f32 {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO, egui::vec2(420.0, 900.0)));
        let mut w = 0.0;
        let _ = ctx.run(input, |ctx| {
            w = build().show(ctx, |ui| { ui.label("x"); }).response.rect.width();
        });
        w
    }

    #[test]
    fn default_width_is_a_range_hint_not_a_width() {
        let ranged = laid_out(|| egui::SidePanel::left("a")
            .resizable(true).default_width(168.0).width_range(110.0..=340.0));
        assert!((ranged - 110.0).abs() < 1.0,
            "laid out at {ranged}; the docs above say default_width yields the range MINIMUM \
(110), not its argument (168). If this now gives 168, egui changed and the module docs need \
correcting rather than this assertion relaxing.");

        let bare = laid_out(|| egui::SidePanel::left("b").resizable(true).default_width(168.0));
        assert!(bare < 168.0,
            "laid out at {bare}; without a range the panel should size to CONTENT, well under \
the 168 requested.");
    }

    #[test]
    fn exact_width_does_pin() {
        let w = laid_out(|| egui::SidePanel::left("c").resizable(false).exact_width(168.0));
        assert!((w - 168.0).abs() < 1.0,
                "exact_width is the documented way to pin a width and gave {w}");
    }
}

#[cfg(test)]
mod touch_floor_tests {
    use super::touch_floor_pt;

    #[test]
    fn an_explicit_setting_wins_on_every_device() {
        // Someone who typed a number means it. A phone must not override it
        // upward, and a desktop must not override it downward.
        assert_eq!(touch_floor_pt(60.0, 360.0, 3.0), 60.0);
        assert_eq!(touch_floor_pt(18.0, 1920.0, 1.0), 18.0);
    }

    #[test]
    fn a_fairphone_gets_a_real_touch_target() {
        // The reported case: narrow surface, compositor at 200-300%.
        assert_eq!(touch_floor_pt(0.0, 360.0, 3.0), 44.0);
        assert_eq!(touch_floor_pt(0.0, 411.0, 2.0), 44.0);
    }

    #[test]
    fn a_hidpi_desktop_does_not_get_phone_buttons() {
        // The failure using ppp alone: every Retina/4k display would inflate
        // its controls to finger size for a mouse user.
        assert_eq!(touch_floor_pt(0.0, 1920.0, 2.0), 24.0);
    }

    #[test]
    fn a_narrow_desktop_window_gets_a_middle_value_not_the_full_touch_size() {
        // The failure using width alone: a tiled window on a 1x display is
        // narrow but still driven by a pointer.
        assert_eq!(touch_floor_pt(0.0, 420.0, 1.0), 32.0);
    }

    #[test]
    fn an_unmeasured_surface_falls_back_to_the_desktop_default() {
        // Width 0 means layout has not run yet; concluding "narrow" from that
        // would flash oversized controls on the first frame of every app.
        assert_eq!(touch_floor_pt(0.0, 0.0, 3.0), 24.0);
    }
}
