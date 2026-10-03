//! The symbol fonts the UI actually draws with.
//!
//! # Why a font, and not SVG icons
//!
//! The Qt frontends got icons from `QIcon::fromTheme` — Breeze or Adwaita,
//! SVGs on disk, resolved by the desktop. The egui ports replaced that with
//! Unicode characters typed into string literals and drawn as text, so an icon
//! is present if and only if a loaded font has that glyph. No icon theme is
//! involved anywhere, and authoring SVGs would mean rewriting every one of
//! ~2750 call sites from text into images.
//!
//! Before this module, nothing in the tree called `FontDefinitions` at all.
//! Every app ran on egui's bundled defaults — Ubuntu-Light, NotoEmoji, and a
//! small hand-picked emoji-icon font — a set nobody had ever compared against
//! the characters the apps use. `examples/glyph_audit.rs` measured the gap:
//! **371 distinct non-ASCII characters drawn, 132 with no glyph anywhere,
//! across 2754 call sites.** The worst offender was `→` at 1376 uses in 114
//! crates, followed by `✓`, `✗` and `●`. Not decoration — the basic vocabulary.
//!
//! # Blocks, not characters
//!
//! The two files here are subsets, but subset by BLOCK rather than by the
//! exact 132 characters the audit found. Subsetting to today's list would be
//! smaller and would break the next time somebody types an arrow that happens
//! not to be in it — a failure that reappears one glyph at a time and looks
//! like a new bug each time. Whole blocks cost about 300 KB more and make the
//! whole vocabulary available; the audit stays as the guard either way.
//!
//! What is covered: arrows, math operators, misc technical, control pictures,
//! enclosed alphanumerics, geometric shapes, misc symbols, dingbats, the
//! supplemental arrow and math blocks, and the monochrome pictographs.
//!
//! **What is NOT covered, deliberately: CJK.** 28 of the remaining characters
//! are Han, kana and fullwidth forms, and they are in tokenizer fixtures in
//! gpu-gemm and voiceterm rather than in any UI label. Noto Sans CJK is
//! 10–20 MB against a 500 MiB repo budget currently sitting near 370, which is
//! two orders of magnitude more than the rest of this module for text no user
//! reads. If a CJK-facing surface ever ships, that is the moment to revisit it,
//! and the audit will still be naming the characters.
//!
//! Measured after this lands: 89 of the 132 covered, 2414 of the 2754 call
//! sites. `U+2038 ‸ CARET` (214 uses, logscope) is the largest single thing
//! still missing and is in none of the Noto symbol faces.
//!
//! # Licensing
//!
//! Both faces are SIL Open Font License 1.1; `OFL-NotoSansSymbols2.txt` and
//! `OFL-NotoSansMath.txt` sit beside them. `subset.py` records exactly how the
//! files were derived from upstream, because a subsetted binary with no recipe
//! is not reproducible and the next person cannot widen it.

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// Marker so repeated calls do not rebuild the atlas.
const INSTALLED: &str = "uikit-egui/fonts-installed";

/// Add the symbol faces as FALLBACKS behind whatever egui already has.
///
/// Appended, never prepended: Ubuntu-Light stays the face that draws Latin, so
/// nothing about existing text changes weight or metrics. These are consulted
/// only for a character the earlier fonts do not have, which is exactly the
/// set that was rendering as a box.
///
/// Installed into both the proportional and monospace families. A terminal
/// grid draws `→` too, and a fallback present in one family and not the other
/// is the kind of gap that shows up in one app and not its neighbour.
///
/// Idempotent: rebuilding the font atlas is not free, and this is called from
/// [`crate::apply_palette`], which an app may call on every settings change.
pub fn install(ctx: &Context) {
    // NOT `ime::allow_ime` any more. It was called from here, and because every app reaches this
    // function that made EVERY window ask the compositor for text input permanently at startup: the
    // on-screen keyboard came up on vidkit-player (and any other app) with no field focused. egui-winit
    // already enables IME when a text field gains focus and drops it when none has, and the one
    // character bug it was aimed at was a different fault (`ime::ImeRepair`). See `crate::ime`.
    if ctx.data(|d| d.get_temp::<bool>(egui::Id::new(INSTALLED))).unwrap_or(false) {
        return;
    }
    let defs = definitions();
    ctx.set_fonts(defs);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(INSTALLED), true));
}

/// egui's defaults plus the symbol fallbacks: what [`install`] sets. Public
/// so an app that sets its own fonts ([`set_primary_font`]) starts from the
/// same list instead of silently dropping the fallbacks.
pub fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, bytes) in [
        (
            "noto-symbols2",
            &include_bytes!("../fonts/NotoSansSymbols2-uisubset.ttf")[..],
        ),
        (
            "noto-math",
            &include_bytes!("../fonts/NotoSansMath-uisubset.ttf")[..],
        ),
    ] {
        defs.font_data
            .insert(name.to_owned(), std::sync::Arc::new(FontData::from_static(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.entry(family).or_default().push(name.to_owned());
        }
    }
    defs
}

/// A user-chosen face FIRST in the proportional family (it draws Latin), with
/// egui's faces and the symbol fallbacks behind it for anything it lacks — a
/// text face with no `⇧` or `→` still gets them. `None` goes back to the
/// defaults. Marks the context installed, so a later [`install`] (every
/// `apply_palette`) does not reset it.
pub fn set_primary_font(ctx: &Context, face: Option<(&str, Vec<u8>)>) {
    let mut defs = definitions();
    if let Some((name, bytes)) = face {
        defs.font_data.insert(name.to_owned(), std::sync::Arc::new(FontData::from_owned(bytes)));
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, name.to_owned());
    }
    ctx.set_fonts(defs);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(INSTALLED), true));
}

#[cfg(test)]
mod ime_hook_tests {
    use super::*;

    #[test]
    fn installing_the_fonts_does_not_make_the_window_claim_text_input() {
        // THE OPPOSITE of what this test used to say. `install` used to ask for IME (every app reaches it,
        // so that was "the distribution strategy"), and that made every window claim text input at
        // startup and for good: on the phone the on-screen keyboard rose over apps with no field focused
        // (vidkit-player, an empty home screen). The one-character bug it was aimed at was a different
        // fault, fixed by `ime::ImeRepair`; egui-winit enables IME by itself when a field has focus.
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        install(&ctx);
        assert!(
            !crate::ime::ime_requested(&ctx),
            "fonts::install must not enable IME: the on-screen keyboard would open over every app"
        );
        let _ = ctx.end_pass();
    }
}
