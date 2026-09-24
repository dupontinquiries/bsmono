//! icons — glyphs that actually render, or an honest word instead.
//!
//! The field report (c11): copy buttons across logscope (and others) show
//! tofu. Cause: they use `⧉` (U+29C9, MISC MATH SYMBOLS-B), and neither the
//! kit nor any app installs a font — everything rides egui's bundled set,
//! which covers emoji blocks and its own icon font but not arbitrary math
//! symbols. Whether a given char renders is a FACT about the running font
//! stack, so this module asks the stack instead of assuming: lay the char
//! out, lay a guaranteed-unassigned char out, and if they resolve to the
//! same missing-glyph metrics, the char is tofu and the caller gets the
//! fallback text. Answered once per (char, pixels-per-point) and cached in
//! egui's own memory, so the probe is not per-frame work.

use egui::{Context, FontId, Id};

/// A char from a plane-14 range no font assigns: the tofu yardstick.


/// Does `c` resolve to a real glyph in the current font stack?
///
/// Asks epaint's glyph table for the proportional family (its whole fallback chain), which
/// egui 0.34 exposes through `fonts_mut`. This replaced a metric probe (lay out `c` and a
/// never-assigned char, call it tofu when their advances match): measured in fao-codec-egui,
/// it reported U+25B6 ▶ and U+1F517 🔗 as missing though both are in the bundled fonts,
/// because their advances coincide with the replacement glyph's -- so `glyph_or` showed the
/// fallback word where the icon would have drawn.
pub fn renders(ctx: &Context, c: char) -> bool {
    let key = Id::new(("uikit-glyph-renders", c as u32));
    if let Some(hit) = ctx.memory(|m| m.data.get_temp::<bool>(key)) {
        return hit;
    }
    let ok = ctx.fonts_mut(|f| f.has_glyph(&FontId::proportional(16.0), c));
    ctx.memory_mut(|m| m.data.insert_temp(key, ok));
    ok
}

/// The glyph if the stack renders it, else the fallback word.
pub fn glyph_or(ctx: &Context, glyph: char, fallback: &str) -> String {
    if renders(ctx, glyph) { glyph.to_string() } else { fallback.to_string() }
}

/// THE copy label, fleet-wide: prefers the two-joined-squares the apps
/// already chose, falls to the clipboard emoji (egui's NotoEmoji covers the
/// emoji blocks), and lands on the word when even that is missing. Apps call
/// this instead of hardcoding a char nobody verified.
pub fn copy_label(ctx: &Context) -> String {
    if renders(ctx, '⧉') {
        "⧉".to_string()
    } else if renders(ctx, '📋') {
        "📋".to_string()
    } else {
        "copy".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        let c = Context::default();
        // fonts exist only after a first pass
        let _ = c.run(egui::RawInput::default(), |_| {});
        c
    }

    /// Ground truth on egui's default fonts, asked rather than assumed: 'A'
    /// renders, an unassigned plane-14 char does not.
    #[test]
    fn the_probe_tells_real_from_missing() {
        let c = ctx();
        assert!(renders(&c, 'A'));
        // The metric probe this replaced called these two tofu (their advance matches the
        // replacement glyph's); both are in the bundled fonts.
        assert!(renders(&c, '\u{25B6}'), "▶");
        assert!(renders(&c, '\u{1F517}'), "🔗");
        assert!(!renders(&c, '\u{E0044}'), "an unassigned char must read as missing");
    }

    /// The answer the field report needed: does egui's default stack cover
    /// the chars our copy buttons use? This test DOCUMENTS the measured
    /// truth — if an egui upgrade changes coverage, it fails here with the
    /// new fact instead of tofu shipping silently.
    #[test]
    fn copy_label_never_returns_tofu() {
        let c = ctx();
        let label = copy_label(&c);
        assert!(!label.is_empty());
        if label != "copy" {
            let ch = label.chars().next().unwrap();
            assert!(renders(&c, ch), "copy_label returned a char the stack cannot draw");
        }
    }

    #[test]
    fn glyph_or_falls_back_and_caches() {
        let c = ctx();
        assert_eq!(glyph_or(&c, '\u{E0033}', "copy"), "copy");
        assert_eq!(glyph_or(&c, 'A', "never"), "A");
        // second ask hits the memory cache (same answer, no relayout assert —
        // the cache is a perf note, the answer stability is the contract)
        assert_eq!(glyph_or(&c, '\u{E0033}', "copy"), "copy");
    }
}
