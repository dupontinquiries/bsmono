//! v0.1.733: graph card density modes. The default card sizing shows one
//! capped line at a narrow clamp — fine for short flowchart labels, but too
//! small for context-rich mindmap nodes (whole paragraphs), which all truncate
//! to the same cramped box. This module makes density an explicit, inline-
//! switchable choice:
//!
//!   Compact  — current behavior: one line, narrow clamp, short cap. Best for
//!              big graphs where you want the SHAPE, not the text.
//!   Expanded — wider box that wraps a few lines; shows enough context to read
//!              a node without opening it. The sensible default for mindmaps.
//!   Literal  — show (nearly) the whole label, wrapped; the box grows to fit.
//!              Best for reading a small graph in full.
//!
//! Pure: given a label + mode, compute the displayed title cap and the card
//! half-extents. The layout + QML wiring live in main.rs/main.qml.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,
    Expanded,
    Literal,
}

impl Density {
    /// Parse the inline hint token (from the QML toggle / a voice command).
    pub fn from_token(s: &str) -> Density {
        match s.trim().to_lowercase().as_str() {
            "expanded" | "expand" | "medium" | "context" => Density::Expanded,
            "literal" | "full" | "all" | "verbatim" => Density::Literal,
            _ => Density::Compact, // default + "compact"/"small"/unknown
        }
    }

    pub fn token(&self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Expanded => "expanded",
            Density::Literal => "literal",
        }
    }

    /// Cycle order for a single toggle button: compact → expanded → literal →
    /// compact.
    pub fn next(&self) -> Density {
        match self {
            Density::Compact => Density::Expanded,
            Density::Expanded => Density::Literal,
            Density::Literal => Density::Compact,
        }
    }
}

/// Computed display box for a card under a given density.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeBox {
    /// How many chars of the label to show (cap; 0 means "all").
    pub title_cap: usize,
    /// Half width / half height in layout px.
    pub half_w: f32,
    pub half_h: f32,
    /// Max wrapped lines the QML delegate should allow (for elide).
    pub max_lines: u32,
}

// Tuning constants. CHAR_W approximates the glyph advance at the card font.
const CHAR_W: f32 = 5.5;
// v0.1.747: more horizontal padding (was 24) so labels have breathing room to
// the card edge — the "increase space to content" dogfood ask. Compact height
// nudged up too (in node_box).
const PAD_W: f32 = 34.0;
const LINE_H: f32 = 18.0;
const PAD_H: f32 = 12.0;

/// Compute the card box for `label` under `mode`. `id_len` is the node id's
/// char count — a card is never narrower than its id needs (matches the old
/// `.max(id_chars)` behavior so id-only nodes still fit).
pub fn node_box(label: &str, id_len: usize, mode: Density) -> NodeBox {
    let label_len = label.chars().count();
    match mode {
        Density::Compact => {
            // one line, narrow clamp, short cap — the original behavior.
            let cap = 64usize;
            let shown = label_len.min(cap).max(id_len);
            let w = (shown as f32 * CHAR_W + PAD_W).clamp(70.0, 360.0);
            NodeBox { title_cap: cap, half_w: w * 0.5, half_h: 28.0, max_lines: 1 }
        }
        Density::Expanded => {
            // wider box, wrap up to 4 lines. Cap longer so there's real
            // context, but still bounded so cards stay comparable.
            let cap = 180usize;
            let shown = label_len.min(cap).max(id_len);
            // target a wider column; wrap the shown text across lines.
            let col_w = 240.0f32;
            let chars_per_line = ((col_w - PAD_W) / CHAR_W).floor().max(1.0);
            let lines = ((shown as f32) / chars_per_line).ceil().clamp(1.0, 4.0);
            let w = col_w.min((shown as f32 * CHAR_W + PAD_W).max(120.0));
            let h = lines * LINE_H + PAD_H;
            NodeBox { title_cap: cap, half_w: w * 0.5, half_h: h * 0.5, max_lines: 4 }
        }
        Density::Literal => {
            // show (almost) everything, wrapped; box grows to fit, up to a
            // generous ceiling so a runaway paragraph can't dominate.
            let cap = 600usize;
            let shown = label_len.min(cap).max(id_len);
            let col_w = 300.0f32;
            let chars_per_line = ((col_w - PAD_W) / CHAR_W).floor().max(1.0);
            let lines = ((shown as f32) / chars_per_line).ceil().clamp(1.0, 14.0);
            let w = col_w.min((shown as f32 * CHAR_W + PAD_W).max(160.0));
            let h = lines * LINE_H + PAD_H;
            NodeBox { title_cap: cap, half_w: w * 0.5, half_h: h * 0.5, max_lines: 14 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_default() {
        assert_eq!(Density::from_token("compact"), Density::Compact);
        assert_eq!(Density::from_token("Expanded"), Density::Expanded);
        assert_eq!(Density::from_token("LITERAL"), Density::Literal);
        assert_eq!(Density::from_token("full"), Density::Literal);
        assert_eq!(Density::from_token("context"), Density::Expanded);
        assert_eq!(Density::from_token("garbage"), Density::Compact);
        for d in [Density::Compact, Density::Expanded, Density::Literal] {
            assert_eq!(Density::from_token(d.token()), d);
        }
    }

    #[test]
    fn cycle_order() {
        assert_eq!(Density::Compact.next(), Density::Expanded);
        assert_eq!(Density::Expanded.next(), Density::Literal);
        assert_eq!(Density::Literal.next(), Density::Compact);
    }

    #[test]
    fn compact_sizing_formula() {
        // v0.1.747: padding bumped for breathing room — w=(shown*5.5+34)
        // .clamp(70,360), half_h=28. (Was +24/clamp 60/h25 pre-v747.)
        let label = "a fairly long mindmap node label that exceeds the cap by quite a lot of characters";
        let b = node_box(label, 5, Density::Compact);
        let shown = label.chars().count().min(64).max(5);
        let w = (shown as f32 * 5.5 + 34.0).clamp(70.0, 360.0);
        assert_eq!(b.half_w, w * 0.5);
        assert_eq!(b.half_h, 28.0);
        assert_eq!(b.max_lines, 1);
    }

    #[test]
    fn expanded_is_taller_and_wraps() {
        let label = "A context-rich node with a couple of sentences worth of detail that should wrap across multiple lines instead of truncating.";
        let compact = node_box(label, 4, Density::Compact);
        let expanded = node_box(label, 4, Density::Expanded);
        // expanded shows more text and is taller (multi-line) than compact.
        assert!(expanded.title_cap > compact.title_cap);
        assert!(expanded.half_h > compact.half_h, "expanded should be taller");
        assert!(expanded.max_lines > 1);
    }

    #[test]
    fn literal_grows_for_long_content() {
        let short = "Short";
        let long = "x".repeat(500);
        let lit_short = node_box(short, 4, Density::Literal);
        let lit_long = node_box(&long, 4, Density::Literal);
        // a long label produces a taller literal box (more wrapped lines).
        assert!(lit_long.half_h > lit_short.half_h);
        assert!(lit_long.title_cap >= 500);
    }

    #[test]
    fn never_narrower_than_id() {
        // an empty label with a long id still sizes to the id.
        let b = node_box("", 40, Density::Compact);
        let id_w = (40.0f32 * CHAR_W + PAD_W).clamp(60.0, 360.0);
        assert_eq!(b.half_w, id_w * 0.5);
    }

    #[test]
    fn dimensions_are_bounded() {
        // no mode produces absurd extents even for a huge label.
        let huge = "z".repeat(5000);
        for d in [Density::Compact, Density::Expanded, Density::Literal] {
            let b = node_box(&huge, 4, d);
            assert!(b.half_w <= 360.0, "{:?} half_w bounded", d);
            assert!(b.half_h <= 200.0, "{:?} half_h bounded", d);
        }
    }
}
