//! Pure tooltip composition for items and armour — WID `r330-items`.
//!
//! Turns an assembled item *view* (name + rarity + affixes + lore) into the
//! ordered, styled lines a menu renderer draws on hover. This is the data layer
//! only: no fonts, no layout, no `egui`, no IO — just the *order and roles* that
//! `render/world_renderer.rs` later paints (see `HANDOFF_items_tooltip.md`).
//!
//! Grounded in `docs/design/item_armor_lore_tooltips.extracted.md`, whose
//! "Tooltip Format Specification" is canonical: every item reads as
//!
//! ```text
//!   <Name>                              (Title, strong)
//!   <Kind> · <Rarity> · <Slot>          (Title, muted — the rarity line)
//!   +0.14 to effective S threshold      (Affix, sorted by magnitude desc)
//!   …
//!                                       (blank separator, present iff lore)
//!   "Hold it long enough and you…"      (Lore, italic — wrapped)
//!     — Teaching inscription, …         (Flavor — the attribution)
//! ```
//!
//! The view is assembled upstream from the existing pure cores —
//! `sim::affix_count` / `sim::affix_magnitude` for the affix list and its
//! magnitudes, `sim::rarity_tier` for the tier, and `item.rs`
//! (`ItemKind`/`MetalKind`) for name/kind/slot. Keeping those out of this module
//! (the caller flattens them into `&str`/`f32`) lets this core compile and test
//! standalone, dependency-free — the documented fast gate (SHARED_RULES §3).
//!
//! Purity: no `unwrap`/`expect`/`panic!`, no allocation surprises, no float
//! formatting that can panic (display text for each affix is supplied by the
//! caller; magnitudes are used only for ordering, and NaN is handled). Output is
//! deterministic for a given view.

/// What a tooltip line *is*, so the renderer can style it. The four roles map
/// 1:1 onto the design spec's line kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TooltipRole {
    /// The item name and the `kind · rarity · slot` subtitle beneath it.
    Title,
    /// A mechanical effect / stat line ("+0.14 to effective S threshold").
    Affix,
    /// Italic lore body text (wrapped), and the blank separator above it.
    Lore,
    /// The lore attribution ("— Teaching inscription, Earthbound Dojo").
    Flavor,
}

/// A styling hint, independent of role — the renderer maps these to weight/tint
/// (e.g. rarity color is applied by the renderer using the tier; here we only
/// say "this line wants emphasis X").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Emphasis {
    /// The headline (item name).
    Strong,
    /// Ordinary body weight (affixes, lore body).
    Normal,
    /// De-emphasised (subtitle, separator, attribution).
    Muted,
}

/// One composed line: ready-to-draw text plus how to style it. Owns its `text`
/// so the renderer needn't keep the source view alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipLine {
    pub text: String,
    pub role: TooltipRole,
    pub emphasis: Emphasis,
}

/// A single affix as the tooltip needs it: the already-formatted display label
/// (so this core never does panicking float formatting) and the numeric
/// `magnitude` used purely to order the lines (strongest first).
#[derive(Debug, Clone, Copy)]
pub struct AffixView<'a> {
    pub label: &'a str,
    pub magnitude: f32,
}

/// The flattened item the tooltip is composed from. The caller assembles this
/// from `item.rs` + the affix/rarity cores; everything here is plain borrowed
/// data so the module stays self-contained.
#[derive(Debug, Clone)]
pub struct ItemTooltipView<'a> {
    /// Display name, e.g. `Seismic Focus Stone` (`ItemKind::display_name`).
    pub name: &'a str,
    /// Rarity tier 0..=5 from `sim::rarity_tier::tier`.
    pub rarity: u8,
    /// Item category for the subtitle, e.g. `Bending Instrument`. `None` omits it.
    pub kind_label: Option<&'a str>,
    /// Equip slot for the subtitle, e.g. `Off-hand / Focus slot`. `None` omits it.
    pub slot_label: Option<&'a str>,
    /// Affixes in *natural* (caller) order; `compose` sorts a copy by magnitude.
    pub affixes: &'a [AffixView<'a>],
    /// Raw lore body (un-wrapped). Empty/whitespace-only ⇒ no lore block.
    pub lore: &'a str,
    /// Attribution line for the lore. `None`/empty ⇒ omitted.
    pub attribution: Option<&'a str>,
    /// Column budget for wrapping the lore body. `0` ⇒ no wrapping.
    pub lore_cols: usize,
}

/// Human-readable rarity for tier 0..=5, matching `sim::rarity_tier`
/// (common, uncommon, rare, epic, legendary, mythic). Out-of-range saturates to
/// the top tier rather than panicking.
pub fn rarity_name(tier: u8) -> &'static str {
    match tier {
        0 => "Common",
        1 => "Uncommon",
        2 => "Rare",
        3 => "Epic",
        4 => "Legendary",
        _ => "Mythic",
    }
}

/// NaN-safe sort key for descending magnitude order: real values keep their
/// value; NaN is treated as the smallest so it sorts to the *end* of a
/// strongest-first list (and never participates in `partial_cmp` panics).
fn desc_key(m: f32) -> f32 {
    if m.is_nan() {
        f32::NEG_INFINITY
    } else {
        m
    }
}

/// Compose the ordered, styled tooltip lines for `item`.
///
/// Order (always): title → subtitle/rarity → affixes (magnitude desc, stable for
/// ties) → blank separator (only if lore) → wrapped lore body → attribution
/// (only if present). A title line is always emitted, even for an affix-less,
/// lore-less item. Deterministic.
pub fn compose(item: &ItemTooltipView) -> Vec<TooltipLine> {
    // Pre-size: title + subtitle + affixes + (sep + ~lore lines) + attribution.
    let mut out: Vec<TooltipLine> = Vec::with_capacity(item.affixes.len().saturating_add(4));

    // 1. Title (always present).
    out.push(TooltipLine {
        text: item.name.to_string(),
        role: TooltipRole::Title,
        emphasis: Emphasis::Strong,
    });

    // 2. Subtitle carrying the rarity: "<kind> · <Rarity> · <slot>".
    let mut subtitle = String::new();
    if let Some(k) = item.kind_label {
        if !k.is_empty() {
            subtitle.push_str(k);
            subtitle.push_str(" · ");
        }
    }
    subtitle.push_str(rarity_name(item.rarity));
    if let Some(s) = item.slot_label {
        if !s.is_empty() {
            subtitle.push_str(" · ");
            subtitle.push_str(s);
        }
    }
    out.push(TooltipLine {
        text: subtitle,
        role: TooltipRole::Title,
        emphasis: Emphasis::Muted,
    });

    // 3. Affixes, strongest first. Stable sort keeps caller order for ties, and
    //    `desc_key` keeps NaN out of the comparator (NaN sinks to the bottom).
    let mut ordered: Vec<&AffixView> = item.affixes.iter().collect();
    ordered.sort_by(|a, b| desc_key(b.magnitude).total_cmp(&desc_key(a.magnitude)));
    for a in ordered {
        out.push(TooltipLine {
            text: a.label.to_string(),
            role: TooltipRole::Affix,
            emphasis: Emphasis::Normal,
        });
    }

    // 4. Lore block (separator + wrapped body), only when there's lore to show.
    let lore_lines = wrap_lore(item.lore, item.lore_cols);
    if !lore_lines.is_empty() {
        out.push(TooltipLine {
            text: String::new(),
            role: TooltipRole::Lore,
            emphasis: Emphasis::Muted,
        }); // separator
        for line in lore_lines {
            out.push(TooltipLine {
                text: line,
                role: TooltipRole::Lore,
                emphasis: Emphasis::Normal,
            });
        }
    }

    // 5. Attribution (Flavor), if any.
    if let Some(attr) = item.attribution {
        if !attr.trim().is_empty() {
            out.push(TooltipLine {
                text: attr.to_string(),
                role: TooltipRole::Flavor,
                emphasis: Emphasis::Muted,
            });
        }
    }

    out
}

/// Greedy word-wrap of lore to at most `max_cols` columns per line, measured in
/// `char`s (so multibyte text is never split mid-`char`).
///
/// - Words are never split: a single word longer than `max_cols` occupies its
///   own line intact (so ASCII never splits mid-word and Unicode never splits
///   mid-grapheme — we only ever break on whitespace).
/// - Internal whitespace is normalised to single spaces.
/// - Empty / whitespace-only input ⇒ empty `Vec`.
/// - `max_cols == 0` ⇒ no wrapping: the whole (whitespace-normalised) text on
///   one line. (Defined explicitly so there's no division/loop hazard.)
///
/// Pure, allocation-bounded, no `unwrap`.
pub fn wrap_lore(text: &str, max_cols: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut words = text.split_whitespace().peekable();
    if words.peek().is_none() {
        return lines; // empty / whitespace-only
    }

    if max_cols == 0 {
        // No wrapping: collapse to a single normalised line.
        let mut single = String::new();
        for (i, w) in text.split_whitespace().enumerate() {
            if i > 0 {
                single.push(' ');
            }
            single.push_str(w);
        }
        lines.push(single);
        return lines;
    }

    let mut current = String::new();
    let mut current_cols: usize = 0;
    for w in words {
        let w_cols = w.chars().count();
        if current.is_empty() {
            // First word on the line — always placed, even if over budget.
            current.push_str(w);
            current_cols = w_cols;
        } else if current_cols.saturating_add(1).saturating_add(w_cols) <= max_cols {
            // Fits with a separating space.
            current.push(' ');
            current.push_str(w);
            current_cols = current_cols.saturating_add(1).saturating_add(w_cols);
        } else {
            // Doesn't fit — flush and start a new line with this word.
            lines.push(std::mem::take(&mut current));
            current.push_str(w);
            current_cols = w_cols;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn affix(label: &'static str, magnitude: f32) -> AffixView<'static> {
        AffixView { label, magnitude }
    }

    // A representative "Seismic Focus Stone"-style view (from the design doc).
    fn focus_stone() -> ItemTooltipView<'static> {
        static AFFIXES: [AffixView<'static>; 3] = [
            AffixView { label: "+0.14 to effective S threshold", magnitude: 0.14 },
            AffixView { label: "Seismic Read range: +8 blocks", magnitude: 8.0 },
            AffixView { label: "Earth voice shapes amplified at S < 0.30", magnitude: 0.30 },
        ];
        ItemTooltipView {
            name: "Seismic Focus Stone",
            rarity: 2, // Rare
            kind_label: Some("Bending Instrument"),
            slot_label: Some("Off-hand / Focus slot"),
            affixes: &AFFIXES,
            lore: "Hold it long enough and you stop feeling the stone in your hand. \
                   You start feeling the stone in the ground.",
            attribution: Some("— Teaching inscription, Earthbound Dojo, Greyholt"),
            lore_cols: 28,
        }
    }

    #[test]
    fn composes_in_documented_order() {
        let lines = compose(&focus_stone());
        // Title (strong) first.
        assert_eq!(lines[0].role, TooltipRole::Title);
        assert_eq!(lines[0].emphasis, Emphasis::Strong);
        assert_eq!(lines[0].text, "Seismic Focus Stone");
        // Subtitle carries the rarity word, in kind · rarity · slot form.
        assert_eq!(lines[1].role, TooltipRole::Title);
        assert_eq!(lines[1].emphasis, Emphasis::Muted);
        assert_eq!(
            lines[1].text,
            "Bending Instrument · Rare · Off-hand / Focus slot"
        );
        // Roles appear in the canonical grouping: Title,Title,Affix*,Lore*,Flavor.
        let roles: Vec<TooltipRole> = lines.iter().map(|l| l.role).collect();
        let first_affix = roles.iter().position(|r| *r == TooltipRole::Affix).unwrap_or(0);
        let first_lore = roles.iter().position(|r| *r == TooltipRole::Lore).unwrap_or(0);
        let first_flavor = roles.iter().position(|r| *r == TooltipRole::Flavor).unwrap_or(0);
        assert!(first_affix < first_lore && first_lore < first_flavor);
        // Exactly one strong title.
        assert_eq!(
            lines.iter().filter(|l| l.emphasis == Emphasis::Strong).count(),
            1
        );
    }

    #[test]
    fn affixes_sorted_by_magnitude_desc_stably() {
        // Two ties at 0.30 in caller order A then B must stay A then B.
        let affixes = [
            affix("low", 0.10),
            affix("tie-A", 0.30),
            affix("high", 8.0),
            affix("tie-B", 0.30),
        ];
        let view = ItemTooltipView {
            name: "x",
            rarity: 0,
            kind_label: None,
            slot_label: None,
            affixes: &affixes,
            lore: "",
            attribution: None,
            lore_cols: 40,
        };
        let texts: Vec<String> = compose(&view)
            .into_iter()
            .filter(|l| l.role == TooltipRole::Affix)
            .map(|l| l.text)
            .collect();
        assert_eq!(texts, vec!["high", "tie-A", "tie-B", "low"]);
    }

    #[test]
    fn lore_wraps_without_splitting_words_on_ascii() {
        let text = "Hold it long enough and you stop feeling the stone in your hand.";
        let cols = 20;
        let wrapped = wrap_lore(text, cols);
        assert!(wrapped.len() > 1, "should wrap onto multiple lines");
        for line in &wrapped {
            assert!(line.chars().count() <= cols, "line over budget: {line:?}");
        }
        // No word was split: rejoining the wrapped words reproduces the input
        // word sequence exactly.
        let rejoined: Vec<&str> = wrapped.iter().flat_map(|l| l.split(' ')).collect();
        let original: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(rejoined, original);
    }

    #[test]
    fn empty_affix_item_still_yields_title_line() {
        let view = ItemTooltipView {
            name: "Plain Rock",
            rarity: 0,
            kind_label: None,
            slot_label: None,
            affixes: &[],
            lore: "",
            attribution: None,
            lore_cols: 40,
        };
        let lines = compose(&view);
        // Title + subtitle only; no affix / lore / flavor lines.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].role, TooltipRole::Title);
        assert_eq!(lines[0].text, "Plain Rock");
        assert_eq!(lines[1].role, TooltipRole::Title);
        assert_eq!(lines[1].text, "Common");
        assert!(lines.iter().all(|l| l.role != TooltipRole::Affix));
        assert!(lines.iter().all(|l| l.role != TooltipRole::Lore));
        assert!(lines.iter().all(|l| l.role != TooltipRole::Flavor));
    }

    #[test]
    fn compose_is_deterministic() {
        let v = focus_stone();
        assert_eq!(compose(&v), compose(&v));
    }

    #[test]
    fn nan_magnitude_does_not_panic_and_sorts_last() {
        let affixes = [
            affix("nan", f32::NAN),
            affix("mid", 1.0),
            affix("top", 5.0),
        ];
        let view = ItemTooltipView {
            name: "x",
            rarity: 1,
            kind_label: None,
            slot_label: None,
            affixes: &affixes,
            lore: "",
            attribution: None,
            lore_cols: 40,
        };
        let texts: Vec<String> = compose(&view)
            .into_iter()
            .filter(|l| l.role == TooltipRole::Affix)
            .map(|l| l.text)
            .collect();
        assert_eq!(texts, vec!["top", "mid", "nan"]);
        // And it's stable across runs.
        assert_eq!(compose(&view), compose(&view));
    }

    #[test]
    fn wrap_lore_handles_zero_cols_empty_and_long_word() {
        // Empty / whitespace-only ⇒ no lines.
        assert!(wrap_lore("", 10).is_empty());
        assert!(wrap_lore("   \t\n ", 10).is_empty());
        // max_cols == 0 ⇒ single normalised line, no panic / no loop.
        assert_eq!(wrap_lore("a  b   c", 0), vec!["a b c".to_string()]);
        // A word longer than the budget gets its own line, intact.
        let long = "supercalifragilistic";
        let wrapped = wrap_lore(&format!("hi {long} ok"), 8);
        assert!(wrapped.iter().any(|l| l == long), "long word kept intact");
    }

    #[test]
    fn rarity_name_covers_all_tiers_and_saturates() {
        let names: Vec<&str> = (0u8..=5).map(rarity_name).collect();
        assert_eq!(
            names,
            ["Common", "Uncommon", "Rare", "Epic", "Legendary", "Mythic"]
        );
        // Distinct + non-empty.
        for (i, n) in names.iter().enumerate() {
            assert!(!n.is_empty());
            assert!(!names[i + 1..].contains(n));
        }
        // Out of range saturates rather than panicking.
        assert_eq!(rarity_name(200), "Mythic");
    }

    #[test]
    fn wrap_lore_never_splits_a_multibyte_char() {
        // "naïve café résumé" — each accented char is 2 bytes; wrapping on word
        // boundaries must keep every line valid UTF-8 with whole words.
        let text = "naïve café résumé piñata";
        for cols in [1usize, 4, 6, 10, 50] {
            let wrapped = wrap_lore(text, cols);
            let rejoined: Vec<&str> = wrapped.iter().flat_map(|l| l.split(' ')).collect();
            let original: Vec<&str> = text.split_whitespace().collect();
            assert_eq!(rejoined, original, "words preserved at cols={cols}");
            // Width is counted in chars, so accented words fit a 6-col line.
            if cols >= 6 {
                assert!(wrapped.iter().all(|l| l.chars().count() <= cols));
            }
        }
    }

    #[test]
    fn separator_present_only_with_lore() {
        // With lore: exactly one blank Lore separator immediately before body.
        let with = focus_stone();
        let lines = compose(&with);
        let seps = lines
            .iter()
            .filter(|l| l.role == TooltipRole::Lore && l.text.is_empty())
            .count();
        assert_eq!(seps, 1);
        // Without lore: no Lore lines at all.
        let mut without = focus_stone();
        without.lore = "";
        without.attribution = None;
        assert!(compose(&without).iter().all(|l| l.role != TooltipRole::Lore));
    }
}
