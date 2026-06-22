//! v0.1.601: UI landmark resolution — let voice reference UI elements
//! by WHERE they are ("top right", "the bottom"), their COLOR ("the red
//! button"), or their SHAPE ("the round one"), not just their label.
//!
//! This module is the PURE core of that feature. It does NOT touch QML
//! or do any highlighting — it only answers "given a spoken phrase and
//! the set of currently-registered UI elements, which element(s) does
//! the user most likely mean, and how confident are we?" The first
//! consumer (v0.1.601 plan) is a non-destructive HIGHLIGHT: glow the
//! resolved element so the user can SEE whether resolution is correct
//! before any action is bolted on. Resolution quality can thus be
//! dogfooded harmlessly.
//!
//! Seam: the QML side registers elements (label + bounding box + color
//! name + shape) into an `Element` list; the resolver is called with a
//! phrase and returns ranked `Match`es. Highlighting and, later,
//! acting on the result live on the QML/Backend side (build-gated).
//!
//! Design echoes entity_extract::resolve_reference (ranked matching with
//! reasons) but scores on SPATIAL ZONES + VISUAL ATTRS instead of text
//! aliases, so it's a separate module rather than a bent EntityKind.

/// A 3x3 spatial zone derived from an element's position in the
/// viewport. Voice words map to these. Corners + edges + center.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    TopLeft, TopCenter, TopRight,
    MidLeft, Center, MidRight,
    BottomLeft, BottomCenter, BottomRight,
}

impl Zone {
    /// Derive the zone from a normalized center point (cx, cy in 0..1,
    /// origin top-left). Thirds along each axis.
    pub fn from_norm(cx: f32, cy: f32) -> Zone {
        let col = if cx < 1.0 / 3.0 { 0 } else if cx < 2.0 / 3.0 { 1 } else { 2 };
        let row = if cy < 1.0 / 3.0 { 0 } else if cy < 2.0 / 3.0 { 1 } else { 2 };
        match (row, col) {
            (0, 0) => Zone::TopLeft,    (0, 1) => Zone::TopCenter,   (0, 2) => Zone::TopRight,
            (1, 0) => Zone::MidLeft,    (1, 1) => Zone::Center,      (1, 2) => Zone::MidRight,
            (2, 0) => Zone::BottomLeft, (2, 1) => Zone::BottomCenter, _      => Zone::BottomRight,
        }
    }

    fn row(self) -> u8 {
        match self {
            Zone::TopLeft | Zone::TopCenter | Zone::TopRight => 0,
            Zone::MidLeft | Zone::Center | Zone::MidRight => 1,
            _ => 2,
        }
    }
    fn col(self) -> u8 {
        match self {
            Zone::TopLeft | Zone::MidLeft | Zone::BottomLeft => 0,
            Zone::TopCenter | Zone::Center | Zone::BottomCenter => 1,
            _ => 2,
        }
    }

    /// Does this zone satisfy a spoken spatial term? "top" matches any
    /// top-row zone; "right" any right-column; "top right" only the
    /// corner; "center"/"middle" the center. Returns a match strength:
    /// 2 = exact (both axes named or it's a single-axis term fully
    /// satisfied), 1 = partial (one of two named axes matches), 0 = no.
    fn satisfies(self, term: &SpatialTerm) -> u8 {
        let row_ok = match term.row {
            Some(0) => self.row() == 0,
            Some(2) => self.row() == 2,
            Some(1) => self.row() == 1,
            _ => true, // unspecified row → any
        };
        let col_ok = match term.col {
            Some(0) => self.col() == 0,
            Some(2) => self.col() == 2,
            Some(1) => self.col() == 1,
            _ => true,
        };
        let named = (term.row.is_some() as u8) + (term.col.is_some() as u8);
        match (row_ok, col_ok, named) {
            (true, true, 2) => 2, // both axes named and satisfied → exact
            (true, true, 1) => 2, // single-axis term fully satisfied → exact
            (true, true, 0) => 0, // nothing named → not a spatial reference
            // one axis named-and-ok but the other named axis failed:
            _ => 0,
        }
    }
}

/// A parsed spatial term from the spoken phrase. row/col are 0 (top/
/// left), 1 (center/middle), 2 (bottom/right), or None (unspecified).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SpatialTerm {
    pub row: Option<u8>,
    pub col: Option<u8>,
}

impl SpatialTerm {
    pub fn is_empty(&self) -> bool {
        self.row.is_none() && self.col.is_none()
    }
}

/// Parse spatial words out of a phrase into a SpatialTerm. Recognizes
/// top/bottom/upper/lower (row), left/right (col), center/middle
/// (either axis, treated as the center of whichever it qualifies).
/// "corner" alone is ambiguous → no axis set (needs a side too).
pub fn parse_spatial(phrase: &str) -> SpatialTerm {
    let p = phrase.to_lowercase();
    let mut t = SpatialTerm::default();
    // rows
    if p.contains("top") || p.contains("upper") || p.contains("above") {
        t.row = Some(0);
    } else if p.contains("bottom") || p.contains("lower") || p.contains("below") {
        t.row = Some(2);
    }
    // cols
    if p.contains("left") {
        t.col = Some(0);
    } else if p.contains("right") {
        t.col = Some(2);
    }
    // center/middle: if no row/col yet, it's the dead center; if one
    // axis is set, "center" fixes the OTHER axis to middle.
    if p.contains("center") || p.contains("centre") || p.contains("middle") {
        if t.row.is_none() && t.col.is_none() {
            t.row = Some(1);
            t.col = Some(1);
        } else if t.row.is_none() {
            t.row = Some(1);
        } else if t.col.is_none() {
            t.col = Some(1);
        }
    }
    t
}

/// A registered UI element the resolver can target.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// Stable id the QML side uses to find + highlight the element.
    pub id: String,
    /// Visible/accessible label ("Send", "Settings", "mic").
    pub label: String,
    /// Spatial zone derived from its position.
    pub zone: Zone,
    /// Semantic color NAME if themed/known ("red", "blue", "green").
    /// Empty if the element has no distinctive color.
    pub color: String,
    /// Shape descriptor if distinctive ("round", "square", "pill").
    /// Empty if not notable.
    pub shape: String,
}

/// A resolved match: the element id, a 0..1 score, and human-readable
/// reasons (for debugging / a "why did it pick this" tooltip).
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub id: String,
    pub score: f32,
    pub reasons: Vec<String>,
}

/// Resolve a spoken phrase against the registered elements. Returns
/// matches best-first, only those scoring above a floor. Scoring sums
/// independent descriptor signals so "the red button on the right"
/// out-scores an element matching only one descriptor:
///   - label word match: strong
///   - color name present in phrase AND matches element: strong
///   - shape word present AND matches: medium
///   - spatial term satisfied by the element's zone: medium (exact) /
///     small (partial)
/// An element that CONTRADICTS a stated descriptor (phrase says "red"
/// but element is blue, or says "left" but element is right) is
/// penalized so it can't win on an unrelated signal.
pub fn resolve(phrase: &str, elements: &[Element], top_n: usize) -> Vec<Match> {
    let p = phrase.to_lowercase();
    let spatial = parse_spatial(&p);
    let phrase_color = COLOR_WORDS.iter().find(|c| word_present(&p, c)).copied();
    let phrase_shape = SHAPE_WORDS.iter().find(|s| word_present(&p, s)).copied();

    let mut out: Vec<Match> = Vec::new();
    for el in elements {
        let mut score = 0.0f32;
        let mut reasons: Vec<String> = Vec::new();
        let mut contradicted = false;
        let mut label_matched = false;

        // Label: any non-trivial label word present in the phrase.
        // v0.1.604: floor is 2 chars (was 3) so short button labels like
        // "OK"/"Go"/"hx" are reachable by voice. 1-char labels ("X") are
        // still excluded — too collision-prone.
        for w in el.label.to_lowercase().split_whitespace() {
            if w.len() >= 2 && word_present(&p, w) {
                score += 0.5;
                reasons.push(format!("label '{}'", w));
                label_matched = true;
                break;
            }
        }

        // Color.
        if let Some(pc) = phrase_color {
            if !el.color.is_empty() {
                if el.color.eq_ignore_ascii_case(pc) || color_synonym(&el.color, pc) {
                    score += 0.5;
                    reasons.push(format!("color {}", pc));
                } else {
                    contradicted = true; // phrase named a color; element is a different known color
                }
            }
        }

        // Shape.
        if let Some(ps) = phrase_shape {
            if !el.shape.is_empty() {
                if el.shape.eq_ignore_ascii_case(ps) || shape_synonym(&el.shape, ps) {
                    score += 0.35;
                    reasons.push(format!("shape {}", ps));
                } else {
                    contradicted = true;
                }
            }
        }

        // Spatial.
        if !spatial.is_empty() {
            match el.zone.satisfies(&spatial) {
                2 => { score += 0.4; reasons.push("position".into()); }
                1 => { score += 0.2; reasons.push("position~".into()); }
                _ => {
                    // phrase named a position the element doesn't occupy.
                    // v0.1.604: but if the LABEL already matched, the
                    // spatial word was probably part of the element's
                    // NAME (e.g. a button labeled "Right Panel" sitting
                    // on the left) — don't let the element's own name
                    // sabotage its match. Only contradict on position
                    // when the label didn't match.
                    if !label_matched {
                        contradicted = true;
                    }
                }
            }
        }

        if contradicted {
            score -= 0.5;
        }
        if score > 0.0 {
            out.push(Match { id: el.id.clone(), score, reasons });
        }
    }

    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(top_n);
    out
}

/// v0.1.626: how a "click X" voice command should be PRESENTED after
/// resolving the phrase against page elements. Replaces blind immediate
/// clicking (the old executeIntent kind=click_link) with a search-then-
/// act flow. Pure decision; the QML side renders each variant.
#[derive(Clone, Debug, PartialEq)]
pub enum ClickPlan {
    /// No element matched well enough — tell the user, click nothing.
    NoMatch,
    /// Exactly one strong, unambiguous match. Show a small confirm
    /// affordance ("Click 'Configure now'?") before acting — NOT a silent
    /// auto-click, so a wrong resolve is catchable.
    ConfirmOne { id: String, label_score: f32 },
    /// A few plausible matches (numbered list): user picks by voice
    /// ("two") or tap. Carries the ranked ids, best-first.
    ChooseFew { ids: Vec<String> },
    /// Many matches: show numbered badges on the elements themselves plus
    /// rotating word suggestions; live voice picks by number. Carries the
    /// ranked ids (already capped).
    ManyBadges { ids: Vec<String> },
}

/// Decide how to present a "click X" command.
///   - one match clearly ahead (strong score AND a real gap to #2)
///     → ConfirmOne
///   - 2..=few_max above the floor → ChooseFew (numbered list)
///   - more than few_max → ManyBadges (badges + rotating suggestions)
///   - nothing above floor → NoMatch
/// `few_max` is the list/badge cutover (the demo's "few vs many"); a
/// sensible default is 5.
pub fn plan_click(phrase: &str, elements: &[Element], few_max: usize) -> ClickPlan {
    // Resolve generously (cap high so "many" is detectable), then bucket.
    let matches = resolve(phrase, elements, 24);
    if matches.is_empty() {
        return ClickPlan::NoMatch;
    }
    // "One strong match": top is strong on its own AND clearly ahead of
    // the runner-up (or there is no runner-up). Strong = >= 0.6 (a label
    // hit 0.5 plus any second signal, or a multi-word label). Clear gap =
    // top beats #2 by >= 0.3 so we don't auto-confirm a near-tie.
    let top = &matches[0];
    let second = matches.get(1).map(|m| m.score).unwrap_or(0.0);
    let strong = top.score >= 0.6;
    let clear_gap = (top.score - second) >= 0.3;
    if matches.len() == 1 || (strong && clear_gap) {
        return ClickPlan::ConfirmOne { id: top.id.clone(), label_score: top.score };
    }
    let ids: Vec<String> = matches.iter().map(|m| m.id.clone()).collect();
    if ids.len() <= few_max {
        ClickPlan::ChooseFew { ids }
    } else {
        ClickPlan::ManyBadges { ids }
    }
}

const COLOR_WORDS: &[&str] = &[
    "red", "green", "blue", "yellow", "orange", "purple", "pink",
    "cyan", "magenta", "white", "black", "gray", "grey", "teal",
];
const SHAPE_WORDS: &[&str] = &["round", "circular", "square", "pill", "rectangular", "rounded"];

fn color_synonym(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> &str {
        match s {
            "grey" => "gray",
            "cyan" => "teal",
            other => other,
        }
    }
    norm(a) == norm(b)
}

fn shape_synonym(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> &str {
        match s {
            "circular" => "round",
            "rounded" => "round",
            "rectangular" => "square",
            other => other,
        }
    }
    norm(a) == norm(b)
}

/// Whole-word containment: `needle` appears in `hay` bounded by
/// non-alphanumerics (so "right" doesn't match "brightness").
fn word_present(hay: &str, needle: &str) -> bool {
    hay.split(|c: char| !c.is_alphanumeric()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: &str, label: &str, zone: Zone, color: &str, shape: &str) -> Element {
        Element { id: id.into(), label: label.into(), zone, color: color.into(), shape: shape.into() }
    }

    #[test]
    fn plan_click_no_match() {
        let els = vec![el("save", "Save", Zone::BottomRight, "", "")];
        assert_eq!(plan_click("click frobnicate", &els, 5), ClickPlan::NoMatch);
    }

    #[test]
    fn plan_click_one_strong_match_confirms() {
        // a unique multi-word label match → ConfirmOne (the "configure
        // now" case from the demo log).
        let els = vec![
            el("cfg", "Configure now", Zone::Center, "", ""),
            el("save", "Save", Zone::BottomRight, "", ""),
            el("cancel", "Cancel", Zone::BottomLeft, "", ""),
        ];
        match plan_click("click configure now", &els, 5) {
            ClickPlan::ConfirmOne { id, .. } => assert_eq!(id, "cfg"),
            other => panic!("expected ConfirmOne, got {:?}", other),
        }
    }

    #[test]
    fn plan_click_few_matches_numbered() {
        // several elements share the word "settings" → a few, numbered.
        let els = vec![
            el("s1", "Account settings", Zone::TopLeft, "", ""),
            el("s2", "Privacy settings", Zone::TopRight, "", ""),
            el("s3", "Display settings", Zone::Center, "", ""),
        ];
        match plan_click("settings", &els, 5) {
            ClickPlan::ChooseFew { ids } => {
                assert!(ids.len() >= 2 && ids.len() <= 5, "few bucket, got {}", ids.len());
            }
            other => panic!("expected ChooseFew, got {:?}", other),
        }
    }

    #[test]
    fn plan_click_many_matches_badges() {
        // 8 elements all matching "item" → exceeds few_max(5) → ManyBadges
        let els: Vec<Element> = (0..8)
            .map(|i| el(&format!("i{}", i), &format!("Item {}", i), Zone::Center, "", ""))
            .collect();
        match plan_click("item", &els, 5) {
            ClickPlan::ManyBadges { ids } => assert!(ids.len() > 5, "many bucket, got {}", ids.len()),
            other => panic!("expected ManyBadges, got {:?}", other),
        }
    }

    #[test]
    fn plan_click_near_tie_does_not_auto_confirm() {
        // two elements with the SAME label → near tie → must NOT
        // ConfirmOne (no clear gap); should offer a choice instead.
        let els = vec![
            el("a", "Open", Zone::TopLeft, "", ""),
            el("b", "Open", Zone::BottomRight, "", ""),
        ];
        match plan_click("open", &els, 5) {
            ClickPlan::ChooseFew { .. } | ClickPlan::ManyBadges { .. } => {}
            other => panic!("near-tie must not auto-confirm, got {:?}", other),
        }
    }

    #[test]
    fn short_labels_are_reachable() {
        // v0.1.604: 2-char labels like "OK"/"Go" must match (were
        // silently unreachable with the old 3-char floor).
        let els = vec![
            el("ok", "OK", Zone::BottomRight, "", ""),
            el("go", "Go", Zone::BottomLeft, "", ""),
        ];
        assert_eq!(resolve("click ok", &els, 3)[0].id, "ok");
        assert_eq!(resolve("the go button", &els, 3)[0].id, "go");
    }

    #[test]
    fn label_with_spatial_word_not_self_sabotaged() {
        // v0.1.604: a button LABELED "Right Panel" sitting on the LEFT
        // must still match "the right panel" — the label match suppresses
        // the spatial contradiction (the word "right" was part of the
        // name, not a position claim). Previously returned nothing.
        let els = vec![el("rp", "Right Panel", Zone::MidLeft, "", "")];
        let m = resolve("the right panel", &els, 3);
        assert_eq!(m.len(), 1, "should match, got {:?}", m);
        assert_eq!(m[0].id, "rp");
    }

    #[test]
    fn spatial_contradiction_still_applies_without_label_match() {
        // The fix must NOT disable contradiction generally: "the red
        // button on the left" still must not pick a red button on the
        // RIGHT when no label word matched.
        let els = vec![el("stop", "Stop", Zone::BottomRight, "red", "")];
        let m = resolve("the red button on the left", &els, 3);
        // red(+0.5) but left contradicts right(-0.5) → net 0 → dropped
        assert!(m.is_empty() || m[0].score <= 0.0,
            "contradiction must still bite when label didn't match, got {:?}", m);
    }

    #[test]
    fn zone_from_norm_corners() {
        assert_eq!(Zone::from_norm(0.1, 0.1), Zone::TopLeft);
        assert_eq!(Zone::from_norm(0.9, 0.1), Zone::TopRight);
        assert_eq!(Zone::from_norm(0.5, 0.5), Zone::Center);
        assert_eq!(Zone::from_norm(0.1, 0.9), Zone::BottomLeft);
        assert_eq!(Zone::from_norm(0.9, 0.9), Zone::BottomRight);
    }

    #[test]
    fn parse_spatial_terms() {
        assert_eq!(parse_spatial("the top right button"),
            SpatialTerm { row: Some(0), col: Some(2) });
        assert_eq!(parse_spatial("on the left"),
            SpatialTerm { row: None, col: Some(0) });
        assert_eq!(parse_spatial("the bottom"),
            SpatialTerm { row: Some(2), col: None });
        assert_eq!(parse_spatial("in the middle"),
            SpatialTerm { row: Some(1), col: Some(1) });
        assert!(parse_spatial("the send button").is_empty());
    }

    #[test]
    fn resolve_by_position() {
        let els = vec![
            el("send", "Send", Zone::BottomRight, "", ""),
            el("settings", "Settings", Zone::TopRight, "", ""),
            el("back", "Back", Zone::TopLeft, "", ""),
        ];
        let m = resolve("the top right one", &els, 3);
        assert_eq!(m[0].id, "settings", "top right → Settings, got {:?}", m);
    }

    #[test]
    fn resolve_by_color() {
        let els = vec![
            el("send", "Send", Zone::BottomRight, "green", "pill"),
            el("delete", "Delete", Zone::BottomLeft, "red", "pill"),
        ];
        let m = resolve("the red button", &els, 3);
        assert_eq!(m[0].id, "delete");
    }

    #[test]
    fn resolve_combines_signals() {
        // "red button on the left" — delete is red AND left. The others
        // each CONTRADICT one stated descriptor (stop is red but right;
        // back is left but gray), so they're penalized below the floor
        // and drop out — delete wins cleanly and alone.
        let els = vec![
            el("delete", "Delete", Zone::BottomLeft, "red", ""),
            el("stop", "Stop", Zone::BottomRight, "red", ""),   // red but right
            el("back", "Back", Zone::TopLeft, "gray", ""),       // left but gray
        ];
        let m = resolve("the red button on the left", &els, 3);
        assert_eq!(m[0].id, "delete", "got {:?}", m);
        // delete scores on BOTH color + position
        assert!(m[0].score >= 0.8, "combined signals → high score, got {}", m[0].score);
        // the contradicted elements must not outrank or tie delete
        assert!(m.iter().skip(1).all(|x| x.score < m[0].score));
    }

    #[test]
    fn contradiction_penalized() {
        // phrase says "red" but the only candidate is blue → it should
        // not win on a weak position signal alone.
        let els = vec![el("ok", "OK", Zone::BottomRight, "blue", "")];
        let m = resolve("the red button on the right", &els, 3);
        // blue element contradicted on color (-0.5) but +0.4 position →
        // net low; still returned but low-confidence.
        if let Some(top) = m.first() {
            assert!(top.score < 0.4, "contradicted color should keep score low, got {}", top.score);
        }
    }

    #[test]
    fn word_present_is_bounded() {
        // "right" must not match inside "brightness"
        assert!(!word_present("adjust the brightness", "right"));
        assert!(word_present("on the right", "right"));
    }

    #[test]
    fn color_and_shape_synonyms() {
        let els = vec![el("m", "mic", Zone::BottomCenter, "grey", "circular")];
        // "gray" matches stored "grey"; "round" matches stored "circular"
        let m = resolve("the gray round button", &els, 3);
        assert_eq!(m.len(), 1);
        assert!(m[0].reasons.iter().any(|r| r.contains("color")));
        assert!(m[0].reasons.iter().any(|r| r.contains("shape")));
    }

    #[test]
    fn no_match_returns_empty() {
        let els = vec![el("send", "Send", Zone::BottomRight, "green", "")];
        // a phrase with no matching descriptor
        assert!(resolve("the purple thing on the left", &els, 3)
            .iter().all(|m| m.score <= 0.0) ||
            resolve("xyzzy", &els, 3).is_empty());
    }
}
