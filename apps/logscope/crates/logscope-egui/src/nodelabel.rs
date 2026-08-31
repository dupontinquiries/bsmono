//! nodelabel — what a node's label says, and which labels get to be drawn.
//!
//! ## Read off a screenshot of the running app, not the art references
//!
//! The Mistfall capture shows the two defects plainly. Labels are cut mid-word — "create a rust
//! crate th", "rust use std::colle", "4. Phow do you add an w" — and neighbouring labels sit on top
//! of each other, so a cluster of turns reads as one smear of text.
//!
//! Neither is a rendering problem. The first is `preview.chars().take(16)`, a hard character cut.
//! The second is a declutter rule that measures the wrong thing: the existing pass dims a label when
//! a NEARER PEARL overlaps it on screen, which is about the node bodies. Two labels at similar depth
//! never trigger it, stay equally bright, and their TEXT overlaps — which is exactly the case in the
//! capture.
//!
//! ## Both halves are pure
//!
//! `elide` is a string function and `visible` takes projected positions. Neither needs a frame, a
//! GPU or a scene, so the crowding cases that are hard to eyeball — a tight cluster, a tie in depth,
//! a label wider than the gap — are unit tests rather than something to squint at.

/// Trim a turn preview to a label: single-spaced, cut on a WORD boundary, ellipsis when cut.
///
/// NAMED `elide_words` because two other `elide`s already exist and neither does this:
/// `diagreport::elide` truncates with a "[+N chars elided]" count for diagnostic values, and
/// `debatepanel::elide` cuts from the START to keep the tail, because in a live strip the most
/// recent words matter most. Three functions called `elide` with three different behaviours is a
/// trap the repo's `dupscan` ratchet caught before it shipped; the fix is a name that says which
/// one this is, not a raised ceiling.
///
/// Mid-word cuts are what makes the current labels read as damaged rather than abbreviated. A word
/// boundary costs a few characters and the result is a phrase; "create a rust…" is a label and
/// "create a rust crate th" is a mistake.
pub fn elide_words(preview: &str, max_chars: usize) -> String {
    // Collapse whitespace first: a preview carries newlines and runs of spaces from the original
    // turn, and those become gaps or line breaks in a single-line label.
    let flat: String = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = flat.trim();
    if max_chars == 0 || flat.is_empty() { return String::new(); }
    if flat.chars().count() <= max_chars { return flat.to_string(); }

    // Reserve one char for the ellipsis, then back up to the last space.
    let budget = max_chars.saturating_sub(1).max(1);
    let cut: String = flat.chars().take(budget).collect();
    // Honour the word boundary whenever what remains is still a readable phrase. The first
    // threshold here was half the budget, and it produced `rust use std::colle…` from
    // "rust use std::collections::HashMap" — which is the exact string in the screenshot that
    // started this. A long code token is precisely the case where backing up matters most:
    // "rust use…" says what the turn was about and "std::colle" says nothing at all.
    //
    // Six characters is the floor. Below that the label carries less than the ellipsis costs, and
    // cutting the word is the lesser evil.
    const MIN_HEAD: usize = 6;
    let head = match cut.rfind(' ') {
        Some(i) if i >= MIN_HEAD => &cut[..i],
        _ => cut.as_str(),
    };
    format!("{}…", head.trim_end_matches([' ', ',', '.', ':', ';', '-']))
}

/// One label's screen placement: node id, projected x/y in pixels, and depth (larger = nearer).
///
/// `LabelBox` rather than `Placed`, which is `hud_pack`'s. That module is the established
/// screen-space overlap resolver here and it takes the OPPOSITE approach deliberately: HUD slabs
/// are anchored UI that must all stay visible, so it nudges them apart and shrinks them as a last
/// resort. A node label cannot be moved — sliding it away from its node makes it point at the wrong
/// turn — so crowding is resolved by hiding the lower-priority label instead. Same problem space,
/// different correct answer, and worth stating so neither looks redundant beside the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabelBox {
    pub id: usize,
    pub x: f32,
    pub y: f32,
    pub depth: f32,
    /// Characters the label will occupy, after `elide`.
    pub chars: usize,
}

/// Which labels to draw, nearest first, dropping any whose box would overlap one already kept.
///
/// Greedy by depth, which is the right priority here: the label you are closest to is the one you
/// are looking at. Ties are broken by id so the choice is stable frame to frame — a declutter that
/// flickers between two equally-close labels is worse than either choice.
///
/// Boxes are the label's own extent, not a fixed radius around the node. That is the correction:
/// the existing pass used a 78px circle around the PEARL, which is blind to a long label beside a
/// short one.
pub fn visible(mut placed: Vec<LabelBox>, char_px: f32, line_px: f32) -> Vec<usize> {
    placed.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.id.cmp(&b.id)));
    let mut kept: Vec<(f32, f32, f32, f32)> = Vec::new(); // x0,y0,x1,y1
    let mut out = Vec::new();
    for p in placed {
        if p.chars == 0 { continue; }
        let w = p.chars as f32 * char_px;
        // The label is drawn centred on the node, so the box straddles it.
        let (x0, x1) = (p.x - w * 0.5, p.x + w * 0.5);
        let (y0, y1) = (p.y - line_px * 0.5, p.y + line_px * 0.5);
        let clash = kept.iter().any(|&(a0, b0, a1, b1)| x0 < a1 && a0 < x1 && y0 < b1 && b0 < y1);
        if !clash {
            kept.push((x0, y0, x1, y1));
            out.push(p.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_preview_is_left_alone() {
        assert_eq!(elide_words("hello there", 24), "hello there");
        assert_eq!(elide_words("exactly ten", 11), "exactly ten");
    }

    /// The defect from the screenshot, by example.
    #[test]
    fn labels_are_cut_on_words_not_mid_word() {
        // "create a rust crate th" is what the 16-char cut produced.
        assert_eq!(elide_words("create a rust crate that parses gguf", 22), "create a rust crate…");
        // The screenshot's other one: a long code token backs up to the last real word rather than
        // being sliced into "std::colle".
        assert_eq!(elide_words("rust use std::collections::HashMap", 20), "rust use…");
        assert_eq!(elide_words("4. How do you add another node", 20), "4. How do you add…");
        for (text, max) in [("create a rust crate that parses gguf", 22),
                            ("rust use std::collections::HashMap", 20),
                            ("why is this better than the alternative", 18)] {
            let out = elide_words(text, max);
            assert!(out.chars().count() <= max, "{out:?} exceeds {max}");
            let body = out.trim_end_matches('…');
            assert!(text.starts_with(body), "{out:?} is not a prefix of the original");
            // the cut point is a word boundary in the source
            let n = body.chars().count();
            assert!(text.chars().nth(n).map(|c| c == ' ').unwrap_or(true),
                    "{out:?} cut mid-word out of {text:?}");
        }
    }

    /// A single long word has no boundary to back up to, and backing up to nothing is worse than
    /// cutting it.
    #[test]
    fn one_enormous_word_is_cut_rather_than_erased() {
        let out = elide_words("Antidisestablishmentarianism", 12);
        assert_eq!(out.chars().count(), 12);
        assert!(out.starts_with("Antidisest"), "{out}");
        // and a long word after a very short one does not collapse to two characters
        let out2 = elide_words("a supercalifragilistic thing", 14);
        assert!(out2.chars().count() > 4, "backed up too far: {out2:?}");
    }

    #[test]
    fn whitespace_and_newlines_are_flattened() {
        assert_eq!(elide_words("  hello\n\n   world  ", 30), "hello world");
        assert_eq!(elide_words("\n\t \n", 30), "");
        assert_eq!(elide_words("anything", 0), "");
    }

    #[test]
    fn trailing_punctuation_does_not_precede_the_ellipsis() {
        assert_eq!(elide_words("first thing, second thing", 16), "first thing…");
        assert!(!elide_words("a b c d e f g h i j", 12).contains(" …"));
    }

    fn p(id: usize, x: f32, y: f32, depth: f32, chars: usize) -> LabelBox {
        LabelBox { id, x, y, depth, chars }
    }

    #[test]
    fn far_apart_labels_all_survive() {
        let v = visible(vec![p(0, 0.0, 0.0, 1.0, 10), p(1, 500.0, 0.0, 0.9, 10),
                             p(2, 0.0, 300.0, 0.8, 10)], 7.0, 14.0);
        assert_eq!(v.len(), 3);
    }

    /// The screenshot case: several turns close together, labels on top of each other.
    #[test]
    fn a_crowded_cluster_keeps_the_nearest_and_drops_the_rest() {
        let cluster: Vec<LabelBox> = (0..5)
            .map(|i| p(i, 100.0 + i as f32 * 6.0, 200.0 + i as f32 * 2.0, 1.0 - i as f32 * 0.1, 16))
            .collect();
        let v = visible(cluster, 7.0, 14.0);
        assert_eq!(v, vec![0], "only the nearest of a stacked cluster should be drawn");
    }

    /// Priority is depth, not order of arrival — the label you are closest to is the one you are
    /// looking at.
    #[test]
    fn the_nearer_label_wins_a_collision_whatever_the_input_order() {
        let a = p(7, 100.0, 100.0, 0.2, 12);   // far
        let b = p(3, 104.0, 100.0, 0.9, 12);   // near
        assert_eq!(visible(vec![a, b], 7.0, 14.0), vec![3]);
        assert_eq!(visible(vec![b, a], 7.0, 14.0), vec![3]);
    }

    /// A declutter that flickers between two equally-close labels is worse than either choice.
    #[test]
    fn a_depth_tie_is_broken_stably() {
        let a = p(9, 100.0, 100.0, 0.5, 12);
        let b = p(4, 103.0, 100.0, 0.5, 12);
        assert_eq!(visible(vec![a, b], 7.0, 14.0), vec![4], "lower id wins a tie");
        assert_eq!(visible(vec![b, a], 7.0, 14.0), vec![4], "and does so regardless of order");
    }

    /// The correction over the old rule: width matters. A long label and a short one at the same
    /// separation are not the same case, and a fixed radius around the node cannot tell them apart.
    #[test]
    fn label_width_decides_the_collision_not_a_fixed_radius() {
        let gap = 60.0;
        let short = vec![p(0, 0.0, 0.0, 1.0, 4), p(1, gap, 0.0, 0.9, 4)];
        let long = vec![p(0, 0.0, 0.0, 1.0, 20), p(1, gap, 0.0, 0.9, 20)];
        assert_eq!(visible(short, 7.0, 14.0).len(), 2, "short labels fit side by side");
        assert_eq!(visible(long, 7.0, 14.0).len(), 1, "wide ones at the same gap do not");
    }

    #[test]
    fn an_empty_label_takes_no_space_and_is_not_drawn() {
        let v = visible(vec![p(0, 0.0, 0.0, 1.0, 0), p(1, 2.0, 0.0, 0.5, 8)], 7.0, 14.0);
        assert_eq!(v, vec![1], "a node with nothing to say must not block its neighbour");
    }

    #[test]
    fn nothing_placed_is_nothing_drawn() {
        assert!(visible(Vec::new(), 7.0, 14.0).is_empty());
    }
}
