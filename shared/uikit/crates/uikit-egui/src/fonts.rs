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
    // THE ON-SCREEN KEYBOARD, from the same hook.
    //
    // Every app in the fleet reaches this function — that is what
    // `font_install_check.py` enforces — which makes it the one place a
    // window-level setting can be applied without twenty-three apps
    // remembering to. See `crate::ime` for why IME needs asking for at all.
    //
    // It is its own idempotent flag, not this one: a context that already had
    // fonts installed by an earlier version must still get the IME request.
    crate::ime::allow_ime(ctx);
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
    fn installing_the_fonts_also_asks_for_ime() {
        // The WIRING, not the mechanism. `ime`'s own tests cover the request;
        // this covers the fact that `fonts::install` makes it — which is the
        // whole distribution strategy, because every app in the fleet reaches
        // this function (font_install_check.py enforces that) and none of them
        // would remember a second call.
        //
        // Written because deleting the call from `install` failed no test,
        // exactly like the unendlich consent seed in 62474b370. The mechanism
        // and its wiring need separate assertions; a test of the mechanism
        // passes perfectly with nothing calling it.
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        assert!(!crate::ime::ime_requested(&ctx));
        install(&ctx);
        assert!(
            crate::ime::ime_requested(&ctx),
            "fonts::install no longer asks for IME — on-screen keyboards will \
             not reach a text field until it already has focus"
        );
        let _ = ctx.end_pass();
    }
}
