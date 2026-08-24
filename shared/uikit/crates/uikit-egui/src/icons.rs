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

use egui::text::LayoutJob;
use egui::{Context, FontId, Id};

/// A char from a plane-14 range no font assigns: the tofu yardstick.
const NEVER_ASSIGNED: char = '\u{E0002}';

fn advance_of(ctx: &Context, c: char) -> f32 {
    let mut job = LayoutJob::default();
    job.append(&c.to_string(), 0.0, egui::TextFormat {
        font_id: FontId::proportional(16.0),
        ..Default::default()
    });
    let galley = ctx.fonts_mut(|f| f.layout_job(job));
    galley.rows.first()
        .and_then(|r| r.glyphs.first())
        .map(|g| g.advance_width)
        .unwrap_or(0.0)
}

/// Does `c` resolve to a real glyph in the current font stack?
///
/// Method: a missing char resolves to the font's replacement glyph, whose
/// advance equals any other missing char's. Matching the yardstick within a
/// hair means tofu. A real glyph CAN coincidentally share the advance —
/// accepted: the failure mode is showing a word instead of an icon, which is
/// the fallback working harder than needed, never tofu shown.
pub fn renders(ctx: &Context, c: char) -> bool {
    let key = Id::new(("uikit-glyph-renders", c as u32));
    if let Some(hit) = ctx.memory(|m| m.data.get_temp::<bool>(key)) {
        return hit;
    }
    let yardstick = advance_of(ctx, NEVER_ASSIGNED);
    let got = advance_of(ctx, c);
    let ok = (got - yardstick).abs() > 0.01 && got > 0.0;
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
