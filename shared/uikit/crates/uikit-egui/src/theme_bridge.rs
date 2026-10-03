//! theme_bridge — turns a `snapclone-theme::Palette` into live egui styling.
//!
//! snapclone-theme already states it is egui-ready ("compatible with egui
//! Color32::from_hex"); this is the small glue that actually applies a palette
//! to an [`egui::Context`] and hands the carved painters their accent. The 9
//! palettes (Dark = the SnapClone warm-sepia look, plus Pitch Black, Paper,
//! Neon, Minimal, Modern, Earth, Pastels, Metallic) therefore drive the egui
//! front-end with no duplicated color tables.

use egui::{Color32, Context, CornerRadius, Stroke, Visuals};
use snapclone_theme::Palette;

use crate::tokens;

/// Parse a `#rrggbb` (or `#rrggbbaa`) hex string from the palette into a
/// `Color32`. Falls back to magenta on malformed input so mistakes are loud.
pub fn hex(s: &str) -> Color32 {
    let h = s.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(0xff);
    match h.len() {
        6 => Color32::from_rgb(b(0), b(2), b(4)),
        8 => Color32::from_rgba_unmultiplied(b(0), b(2), b(4), b(6)),
        _ => Color32::from_rgb(0xff, 0x00, 0xff),
    }
}

/// The colors a carved surface needs, resolved from a palette.
#[derive(Clone, Copy)]
pub struct PaletteColors {
    pub bg_base: Color32,
    pub bg_card: Color32,
    pub bg_raised: Color32,
    pub bg_hover: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub border: Color32,
    pub glow: Color32,
}

impl PaletteColors {
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            bg_base: hex(p.bg_base),
            bg_card: hex(p.bg_card),
            bg_raised: hex(p.bg_raised),
            bg_hover: hex(p.bg_hover),
            text: hex(p.text_primary),
            text_weak: hex(p.text_secondary),
            accent: hex(p.accent),
            accent_soft: hex(p.accent_soft),
            danger: hex(p.danger),
            success: hex(p.success),
            warning: hex(p.warning),
            border: hex(p.border),
            glow: hex(p.glow_online),
        }
    }
}

/// Perceived luminance, 0..255.
fn luma(c: Color32) -> f32 {
    0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
}

fn scaled(c: Color32, k: f32, floor: u8) -> Color32 {
    let f = |v: u8| ((v as f32 * k).round() as u8).max(floor);
    Color32::from_rgba_unmultiplied(f(c.r()), f(c.g()), f(c.b()), c.a())
}

/// True black for an OLED panel. A "dark" theme's ground is typically (20, 20, 25): on an OLED that is
/// a visibly lit grey over the whole screen, every pixel burning, when black is OFF. This maps a DARK
/// palette's layers down so the ground (`bg_base`) is exactly (0, 0, 0) and the layers above it step
/// up from there in the same order (card < raised < hover), still separable from the ground and from
/// each other; text, accents and borders are untouched (the border is what outlines a card that is
/// now nearly the colour of the ground). A LIGHT palette is returned unchanged: a light theme
/// is light on purpose and has nothing to save.
pub fn oled_black(c: &PaletteColors) -> PaletteColors {
    if luma(c.bg_base) > 110.0 {
        return *c;
    }
    PaletteColors {
        bg_base: Color32::from_rgba_unmultiplied(0, 0, 0, c.bg_base.a()),
        bg_card: scaled(c.bg_card, 0.45, 6),
        bg_raised: scaled(c.bg_raised, 0.7, 14),
        bg_hover: scaled(c.bg_hover, 0.85, 22),
        ..*c
    }
}

/// Whether this user wants true black: `UIKIT_OLED=1|on|true` in the environment, or a
/// `~/.config/uikit/oled` file saying `on`. Opt-in per user, because a laptop LCD gains nothing from it.
/// Read by `apply_palette` (every themed app) and by tideshell's published palette.
pub fn oled_enabled() -> bool {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")));
    let file = base.and_then(|b| std::fs::read_to_string(b.join("uikit/oled")).ok());
    oled_switch(std::env::var("UIKIT_OLED").ok().as_deref(), file.as_deref())
}

/// The decision, pure: the environment wins over the file, so a process can be forced either way.
pub fn oled_switch(env: Option<&str>, file: Option<&str>) -> bool {
    let on = |v: &str| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "on" | "true" | "yes");
    match (env, file) {
        (Some(v), _) => on(v),
        (None, Some(f)) => on(f),
        (None, None) => false,
    }
}

/// Apply a palette to the whole context: base Visuals + the app's token radius
/// and spacing. Call when the user picks a theme in Settings ▸ Look.
/// Touch-sized controls, without touching a single colour.
///
/// The third tier of what this kit gives an app, after fonts and IME. Those two
/// ride `fonts::install` and reach everything; this one cannot, because it
/// CHANGES LAYOUT — every button gets taller, and a dense panel reflows.
///
/// It is the fix for "buttons are still a bit small on mobile", and the number
/// is the reason: egui's default `interact_size.y` is 24pt, which is a mouse
/// target. `tokens::TOUCH_MIN` is 40. An app that themes itself by hand and
/// will never call `apply_palette` still wants this on a phone, and until now
/// the two were welded together — you took the colours or you took nothing.
///
/// Opt-in and separate for the same reason `palette_colors_ctx` is separate
/// from `apply_palette`: an app should be able to fix one thing without
/// accepting a change to another. Six apps in this fleet install the fonts and
/// theme themselves; this is what they can take next, one at a time, with
/// somebody looking at the screen.
pub fn touch_targets(ctx: &Context) {
    ctx.global_style_mut(|s| {
        s.spacing.interact_size.y = tokens::TOUCH_MIN as f32;
    });
}

pub fn apply_palette(ctx: &Context, p: &Palette) -> PaletteColors {
    apply_palette_oled(ctx, p, oled_enabled())
}

/// `apply_palette` with the OLED choice made by the caller (tests, and anyone with their own switch).
pub fn apply_palette_oled(ctx: &Context, p: &Palette, oled: bool) -> PaletteColors {
    // The symbol fallbacks, here rather than in every app's startup, because
    // this is the one function a themed app already calls and a font nobody
    // installs is how 132 characters came to render as boxes fleet-wide.
    // Idempotent — see `fonts::install`.
    crate::fonts::install(ctx);
    let c = PaletteColors::from_palette(p);
    let c = if oled { oled_black(&c) } else { c };
    ctx.set_visuals(build_visuals(&c));
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(tokens::SPACE_SM as f32, tokens::SPACE_SM as f32);
        s.spacing.button_padding = egui::vec2(tokens::SPACE_MD as f32, tokens::SPACE_SM as f32);
    });
    // Through the shared function, so a change to the touch floor cannot apply
    // to themed apps and miss the ones that opted in by hand.
    touch_targets(ctx);
    c
}

/// Build the egui [`Visuals`] for a resolved set of palette colors. Split out of
/// [`apply_palette`] so [`crate::apply_prefs`] can re-derive Visuals from a
/// *filtered* [`PaletteColors`] (see [`filter_palette_colors`]) without
/// duplicating this color -> Visuals mapping, and without [`apply_palette`]'s
/// own public (unfiltered) behavior changing for its existing callers.
pub(crate) fn build_visuals(c: &PaletteColors) -> Visuals {
    use crate::glass::{tint, GlassAlphas};
    let mut v = Visuals::dark(); // start dark; pale palettes override below

    v.override_text_color = Some(c.text);
    // THE GLASS FINISH, for every themed app at once. snapclone's surfaces are
    // what the fleet's other egui fronts were being compared against ("it
    // literally has a shine"), and the difference was never the palette — it
    // was these: panels that are a high-alpha TINT rather than a flat slab,
    // so a backdrop (`polish::Backdrop`) shows through them as a glow; windows
    // and popups on the same glass with a soft, dropped shadow and a hairline
    // border; widgets as glass keys (0.78 resting, 0.88 hovered, the accent as
    // a solid flip when pressed) rather than opaque rectangles. The alphas are
    // `GlassAlphas`, the same table the keyboard and the panels use.
    v.panel_fill = tint(c.bg_base, GlassAlphas::SURFACE);
    v.window_fill = tint(c.bg_card, GlassAlphas::SURFACE);
    v.extreme_bg_color = c.bg_base;
    v.faint_bg_color = tint(c.bg_raised, GlassAlphas::KEY_MODIFIER);
    v.hyperlink_color = c.accent;
    v.selection.bg_fill = c.accent.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, c.accent);
    v.window_corner_radius = CornerRadius::same(tokens::RADIUS_MD as u8);
    v.menu_corner_radius = CornerRadius::same(tokens::RADIUS_MD as u8);
    v.window_stroke = Stroke::new(1.0_f32, tint(c.border, GlassAlphas::BORDER_SOFT));
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 22,
        spread: 0,
        color: Color32::from_black_alpha(96),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(80),
    };

    let r = CornerRadius::same(tokens::RADIUS_SM as u8);
    let widgets = &mut v.widgets;
    for (state, fill, alpha) in [
        (&mut widgets.noninteractive, c.bg_card, GlassAlphas::KEY_MODIFIER),
        (&mut widgets.inactive, c.bg_card, GlassAlphas::KEY),
        (&mut widgets.hovered, c.bg_hover, GlassAlphas::KEY_HOVER),
        (&mut widgets.active, c.bg_hover, GlassAlphas::KEY_HOVER),
        (&mut widgets.open, c.bg_raised, GlassAlphas::KEY_HOVER),
    ] {
        state.bg_fill = tint(fill, alpha);
        state.weak_bg_fill = tint(fill, alpha);
        state.bg_stroke = Stroke::new(1.0_f32, tint(c.border, GlassAlphas::BORDER));
        state.fg_stroke = Stroke::new(1.0_f32, c.text);
        state.corner_radius = r;
    }
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, c.text);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, c.accent);
    v
}

/// Apply the display accessibility filter (color-vision filter + lowered white
/// point) to every color in a resolved palette. A no-op copy when
/// `d.is_active()` is false, so callers can run this unconditionally.
pub(crate) fn filter_palette_colors(c: &PaletteColors, d: &settings_kit::DisplayPrefs) -> PaletteColors {
    PaletteColors {
        bg_base: filter_color(c.bg_base, d),
        bg_card: filter_color(c.bg_card, d),
        bg_raised: filter_color(c.bg_raised, d),
        bg_hover: filter_color(c.bg_hover, d),
        text: filter_color(c.text, d),
        text_weak: filter_color(c.text_weak, d),
        accent: filter_color(c.accent, d),
        accent_soft: filter_color(c.accent_soft, d),
        danger: filter_color(c.danger, d),
        success: filter_color(c.success, d),
        warning: filter_color(c.warning, d),
        border: filter_color(c.border, d),
        glow: filter_color(c.glow, d),
    }
}

#[cfg(test)]
mod tests {
    fn lum(c: Color32) -> f32 {
        super::luma(c)
    }

    #[test]
    fn oled_black_makes_the_ground_exactly_black_and_keeps_the_layers_in_order_on_every_dark_palette() {
        let mut dark = 0;
        for id in snapclone_theme::PaletteId::all() {
            let c = PaletteColors::from_palette(snapclone_theme::palette_for(*id));
            if lum(c.bg_base) > 110.0 {
                continue; // a light palette: covered by its own test below
            }
            dark += 1;
            let o = oled_black(&c);
            assert_eq!((o.bg_base.r(), o.bg_base.g(), o.bg_base.b()), (0, 0, 0), "{id:?} ground is black");
            assert!(lum(o.bg_base) < lum(o.bg_card), "{id:?}: the card is still above the ground");
            assert!(lum(o.bg_card) < lum(o.bg_raised), "{id:?}: raised above card");
            assert!(lum(o.bg_raised) < lum(o.bg_hover), "{id:?}: hover above raised");
            assert!(lum(o.bg_hover) <= lum(c.bg_hover), "{id:?}: never brighter than it was");
            // what is read on the surfaces is untouched
            assert_eq!((o.text, o.accent, o.border, o.danger), (c.text, c.accent, c.border, c.danger), "{id:?}");
        }
        assert!(dark >= 10, "the scan found the dark palettes: {dark}");
    }

    #[test]
    fn a_light_palette_is_left_light() {
        let c = PaletteColors::from_palette(snapclone_theme::palette_by_id("paper"));
        assert!(lum(c.bg_base) > 110.0, "paper is a light palette");
        let o = oled_black(&c);
        assert_eq!(o.bg_base, c.bg_base);
        assert_eq!(o.bg_card, c.bg_card);
    }

    #[test]
    fn the_oled_switch_prefers_the_environment_to_the_file_and_defaults_off() {
        assert!(!oled_switch(None, None), "off by default: a laptop LCD gains nothing");
        assert!(oled_switch(Some("1"), None));
        assert!(oled_switch(Some(" On\n"), None), "case and whitespace do not matter");
        assert!(oled_switch(None, Some("on\n")), "the per-user file");
        assert!(!oled_switch(Some("0"), Some("on")), "the environment forces it off over the file");
        assert!(oled_switch(Some("true"), Some("off")), "and on over the file");
        assert!(!oled_switch(Some("maybe"), None), "anything unrecognised is off");
    }

    #[test]
    fn apply_palette_follows_the_flag_in_the_egui_visuals_too() {
        let ctx = Context::default();
        let p = snapclone_theme::palette_by_id("dark");
        let plain = apply_palette_oled(&ctx, p, false);
        assert_ne!((plain.bg_base.r(), plain.bg_base.g(), plain.bg_base.b()), (0, 0, 0), "dark's ground is not black");
        assert_ne!(ctx.global_style().visuals.panel_fill.r(), 0);
        let black = apply_palette_oled(&ctx, p, true);
        assert_eq!((black.bg_base.r(), black.bg_base.g(), black.bg_base.b()), (0, 0, 0));
        assert_eq!(ctx.global_style().visuals.panel_fill.r(), 0, "the panels the apps paint are black");
    }

    use super::*;

    /// The finish is glass: panels and widgets are high-alpha tints, never opaque slabs, and
    /// the alphas are the shared `GlassAlphas` table — so a retune there reaches every app.
    #[test]
    fn the_visuals_are_glass_not_slab() {
        use crate::glass::GlassAlphas;
        let c = crate::default_palette();
        let v = build_visuals(&c);
        assert_eq!(v.panel_fill.a(), GlassAlphas::SURFACE, "panel is a tint the backdrop shows through");
        assert_eq!(v.window_fill.a(), GlassAlphas::SURFACE);
        assert_eq!(v.widgets.inactive.bg_fill.a(), GlassAlphas::KEY);
        assert_eq!(v.widgets.hovered.bg_fill.a(), GlassAlphas::KEY_HOVER);
        assert!(v.window_shadow.blur > 0 && v.window_shadow.offset[1] > 0, "windows drop a soft shadow");
        assert_eq!(v.window_corner_radius, CornerRadius::same(tokens::RADIUS_MD as u8));
        // and the tint keeps the palette's colour, so a re-theme still re-themes
        // (Color32 stores premultiplied; the unmultiplied channels come back within the
        // rounding of one premultiply/unmultiply round trip)
        let [r, g, b, _] = v.panel_fill.to_srgba_unmultiplied();
        for (got, want) in [(r, c.bg_base.r()), (g, c.bg_base.g()), (b, c.bg_base.b())] {
            assert!(got.abs_diff(want) <= 1, "{got} vs {want}");
        }
    }

    #[test]
    fn hex_parses_six_and_eight() {
        assert_eq!(hex("#1A1917"), Color32::from_rgb(0x1A, 0x19, 0x17));
        assert_eq!(hex("#C9B99A"), Color32::from_rgb(0xC9, 0xB9, 0x9A)); // DARK accent
        let a = hex("#11111080");
        assert_eq!(a.a(), 0x80);
    }
}

/// Apply the display accessibility filter (lowered white point + colour filter) to a single
/// colour — use on app content colours. `is_active()==false` returns the colour unchanged.
pub fn filter_color(c: Color32, d: &settings_kit::DisplayPrefs) -> Color32 {
    if !d.is_active() {
        return c;
    }
    let (r, g, b) = d.apply((c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0));
    Color32::from_rgba_unmultiplied((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, c.a())
}

#[cfg(test)]
mod touch_target_tests {
    use super::*;

    #[test]
    fn touch_targets_raises_the_floor_without_touching_colours() {
        // The whole reason it is its own function: an app that themes itself by
        // hand can take the 40pt floor without accepting a recolour.
        let ctx = Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let before = ctx.global_style().visuals.panel_fill;
        let default_h = ctx.global_style().spacing.interact_size.y;
        assert!(default_h < tokens::TOUCH_MIN as f32,
                "egui's default is already {default_h} — this function does nothing");
        touch_targets(&ctx);
        assert_eq!(ctx.global_style().spacing.interact_size.y, tokens::TOUCH_MIN as f32);
        assert_eq!(ctx.global_style().visuals.panel_fill, before,
                   "touch_targets changed a colour");
        let _ = ctx.end_pass();
    }

    #[test]
    fn apply_palette_goes_through_the_same_function() {
        // So a change to the touch floor cannot reach themed apps and miss the
        // ones that opted in by hand — which is how two numbers that are meant
        // to be one number drift apart.
        let ctx = Context::default();
        ctx.begin_pass(egui::RawInput::default());
        apply_palette(&ctx, &snapclone_theme::palette_for(Default::default()));
        assert_eq!(ctx.global_style().spacing.interact_size.y, tokens::TOUCH_MIN as f32);
        let _ = ctx.end_pass();
    }
}

#[cfg(test)]
mod prefs_tests {
    use super::*;

    #[test]
    fn apply_prefs_scales_text() {
        let ctx = Context::default();
        let before = ctx.global_style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size).unwrap_or(0.0);
        let mut prefs = settings_kit::Prefs::default();
        prefs.touch.text_scale = 2.0;
        crate::apply_prefs(&ctx, &prefs);
        let after = ctx.global_style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size).unwrap_or(0.0);
        assert!(after > before * 1.5, "text_scale 2.0 should roughly double body text ({before} -> {after})");
    }

    #[test]
    fn filter_color_grayscale_collapses_channels() {
        let d = settings_kit::DisplayPrefs { white_point: 1.0, color_filter: settings_kit::ColorFilter::Grayscale, opacity: 1.0 };
        let out = filter_color(Color32::from_rgb(255, 0, 0), &d);
        assert!((out.r() as i32 - out.g() as i32).abs() <= 1 && (out.g() as i32 - out.b() as i32).abs() <= 1);
        // inactive filter is a no-op
        let none = settings_kit::DisplayPrefs::default();
        assert_eq!(filter_color(Color32::from_rgb(10, 20, 30), &none), Color32::from_rgb(10, 20, 30));
    }
}

/// Visuals from resolved colours, public: for a palette that did not come
/// from a [`Palette`] -- tideshell's living one, via [`ambient`].
pub fn apply_colors(ctx: &Context, c: &PaletteColors) {
    ctx.set_visuals(build_visuals(c));
}

/// tideshell's living palette <-> [`PaletteColors`]. One mapping, used by the
/// shell to write and by apps to read.
pub mod ambient {
    use super::PaletteColors;
    use ambient_palette::{AmbientPalette, Rgba};
    use egui::Color32;

    fn out(c: Color32) -> Option<Rgba> {
        Some(c.to_srgba_unmultiplied())
    }
    fn inn(c: Option<Rgba>, keep: Color32) -> Color32 {
        c.map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a)).unwrap_or(keep)
    }

    /// What the shell publishes. danger/success/warning/glow are not sent:
    /// they carry meaning, and an app's "error red" should not drift warm
    /// because the evening did.
    pub fn to_ambient(c: &PaletteColors, light: bool) -> AmbientPalette {
        AmbientPalette {
            light,
            seq: 0,
            bg_base: out(c.bg_base),
            bg_card: out(c.bg_card),
            bg_raised: out(c.bg_raised),
            bg_hover: out(c.bg_hover),
            text: out(c.text),
            text_weak: out(c.text_weak),
            accent: out(c.accent),
            accent_soft: out(c.accent_soft),
            border: out(c.border),
        }
    }

    /// The app's own palette with the shell's roles laid over it.
    pub fn merge(base: &PaletteColors, a: &AmbientPalette) -> PaletteColors {
        PaletteColors {
            bg_base: inn(a.bg_base, base.bg_base),
            bg_card: inn(a.bg_card, base.bg_card),
            bg_raised: inn(a.bg_raised, base.bg_raised),
            bg_hover: inn(a.bg_hover, base.bg_hover),
            text: inn(a.text, base.text),
            text_weak: inn(a.text_weak, base.text_weak),
            accent: inn(a.accent, base.accent),
            accent_soft: inn(a.accent_soft, base.accent_soft),
            border: inn(a.border, base.border),
            ..*base
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_wire_carries_the_look_and_not_the_semantics() {
            let base = PaletteColors::from_palette(snapclone_theme::palette_by_id("dark"));
            let mut shell = base;
            shell.accent = Color32::from_rgb(217, 144, 90);
            shell.bg_base = Color32::from_rgb(30, 24, 20);
            shell.danger = Color32::from_rgb(1, 2, 3);
            let line = to_ambient(&shell, false).to_line();
            let back = merge(&base, &AmbientPalette::from_line(&line).unwrap());
            assert_eq!(back.accent, shell.accent);
            assert_eq!(back.bg_base, shell.bg_base);
            assert_eq!(back.danger, base.danger, "the app's own danger red is kept");
            // An empty palette changes nothing.
            let same = merge(&base, &AmbientPalette::default());
            assert_eq!(same.accent, base.accent);
        }
    }
}
