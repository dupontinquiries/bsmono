//! nodelabel — what a node's label says, and which labels get to be drawn.
//!
//! ## Both defects are in the code, which is where the evidence should have come from
//!
//! A CORRECTION WORTH KEEPING, because the reasoning was wrong even though the fix was right. I
//! first justified this module from one of the "reference images", believing it to be a screenshot
//! of the running app, and quoted its text as observed output — "create a rust crate th", "rust use
//! std::colle", "4. Phow do you add an w". Those are not real strings. That image was generated,
//! and garbled text is exactly what generation produces; the operator had to tell me. Quoting a
//! picture of a program as if it were the program is the same one-step-removed evidence this
//! codebase keeps getting caught by, and I was the one holding it.
//!
//! The defects are real anyway, and here is the actual evidence, which is checkable forever:
//!
//! * `chat.rs` cut labels with `preview.chars().take(16)` — a hard character slice with no word
//!   boundary. Read it in the source, not off a picture.
//! * The declutter measures the wrong thing. The existing pass dims a label when a NEARER PEARL
//!   overlaps it on screen (`r2.3 < q.3 - 0.1`, then a fixed 78px radius around the node body).
//!   Two labels at similar depth fail that test, stay equally bright, and their TEXT overlaps. The
//!   pass is still there and still correct for what it does — occlusion by a node in front — which
//!   is why this module handles the other case rather than replacing it.
//!
//! The third thing this found — the declutter was keeping the FARTHEST label of every cluster,
//! because I had the depth sign backwards — came from realistic test fixtures and has nothing to do
//! with any image.
//!
//! ## Both halves are pure
//!
//! `elide` is a string function and `visible` takes projected positions. Neither needs a frame, a
//! GPU or a scene, so the crowding cases that are hard to eyeball — a tight cluster, a tie in depth,
//! a label wider than the gap — are unit tests rather than something to squint at.
use crate::spatial::Vec3;

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
    // threshold here was half the budget, and on "rust use std::collections::HashMap" it left the
    // boundary too early and sliced the token. A long code token is precisely the case where
    // backing up matters most:
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
    /// Distance in front of the camera: SMALLER IS NEARER.
    ///
    /// This is `spatial::Camera::project`'s third return, `dist - z2`, and the sign caught me out.
    /// I sorted it descending on the assumption that larger meant closer, which is backwards, and
    /// the fixtures hid it because they used 1.0 and 0.2 as priority scores rather than distances.
    /// Giving them real world depths turned two passing tests red and named the bug — the declutter
    /// had been keeping the FARTHEST label of every cluster.
    pub depth: f32,
    /// The label's own pixel extents.
    ///
    /// PIXELS, NOT A CHARACTER COUNT. The first version took `chars` and multiplied by a global
    /// `char_px`, which quietly asserts that a label ten metres away is as wide on screen as one at
    /// arm's length. It is not: these labels are 3D cell text and the projection is
    /// `scale / depth`, so a far label is narrower in exactly the proportion the constant ignored.
    /// The effect was a declutter that over-suppressed distant labels and under-suppressed near
    /// ones — the two cases it exists to get right.
    ///
    /// The caller knows the projection and computes these; see `px_extents`.
    pub w: f32,
    pub h: f32,
}

/// Screen extents of a cell-text label, from the same projection that placed it.
///
/// `mesh3d::label_cells` advances `6.0 * cell` world units per character and stands `7.0 * cell`
/// tall; `spatial::Camera::project` scales world to screen by `view_scale / depth`. Those are the
/// two facts, multiplied — not an estimate.
pub fn px_extents(chars: usize, cell: f32, view_scale: f32, depth: f32) -> (f32, f32) {
    // A label at or behind the camera plane has no meaningful extent; `project` already refuses
    // those, and this keeps the arithmetic finite if one ever arrives.
    if depth <= 0.1 || !depth.is_finite() { return (0.0, 0.0); }
    let per_px = view_scale / depth;
    (chars as f32 * 6.0 * cell * per_px, 7.0 * cell * per_px)
}

/// (There is no `centroid` in this module. `spatial::centroid` already computes the mean position
/// of the nodes and is what the caller passes in — a second one here, over screen pairs instead of
/// `Vec3`s, is the duplication `dupscan` has now caught me at three times in one session.)
///
/// Where a label sits: pushed OUTWARD from the cluster centre, in the ground plane.
///
/// This is what the references do — their cards ring the node cluster with a short connector to
/// each node, so nothing is drawn on top of anything and nothing has to be hidden. `visible` below
/// still resolves what remains crowded, but as a fallback rather than the whole strategy: a label
/// you cannot see is a turn you cannot read.
///
/// Radial because that is the direction with room. A graph drawn around a centre has its empty
/// space outside, and any fixed axis would crowd one side while leaving the other bare. Ground
/// plane only because the graph spreads in x/z — lifting a label vertically moves it off its node
/// without gaining any.
///
/// ONE FUNCTION, TWO READERS. The label text and its connector line both call this, so they cannot
/// disagree about where the label is. It took being wrong twice to arrive here: the first version
/// pushed the SCREEN anchor, which meant the declutter measured boxes at one place while the glyphs
/// drew at another, and the second left this as a 2D helper with no callers at all after the world
/// version was inlined into the caller — dead code I wrote in the commit that fixed the first
/// mistake.
///
/// A node AT the centroid has no outward direction. Normalising a zero vector gives NaN, and a NaN
/// position reaches `Color32` arithmetic and panics a frame later — the projection loop already
/// carries a guard against exactly that. Those keep their own position; one label in the middle is
/// not a crowding problem.
pub fn outward(node: Vec3, centre: Vec3, push: f32) -> Vec3 {
    let (dx, dz) = (node.x - centre.x, node.z - centre.z);
    let len = (dx * dx + dz * dz).sqrt();
    if !len.is_finite() || len < 1e-3 { return node; }
    Vec3::new(node.x + dx / len * push, node.y, node.z + dz / len * push)
}

/// Which labels to draw, nearest first, dropping any whose box would overlap one already kept.
///
/// Greedy by depth, which is the right priority here: the label you are closest to is the one you
/// are looking at. Nearest means the SMALLEST depth — see the field's note. Ties are broken by id so
/// the choice is stable frame to frame; a declutter that flickers between two equally-close labels
/// is worse than either choice.
///
/// Boxes are the label's own extent, not a fixed radius around the node. That is the correction:
/// the existing pass used a 78px circle around the PEARL, which is blind to a long label beside a
/// short one.
pub fn visible(mut placed: Vec<LabelBox>) -> Vec<usize> {
    // Nearest first: ASCENDING depth, because depth is distance in front of the camera.
    placed.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.id.cmp(&b.id)));
    let mut kept: Vec<(f32, f32, f32, f32)> = Vec::new(); // x0,y0,x1,y1
    let mut out = Vec::new();
    for p in placed {
        if p.w <= 0.0 || p.h <= 0.0 { continue; }
        // The label is drawn centred on the node, so the box straddles it.
        let (x0, x1) = (p.x - p.w * 0.5, p.x + p.w * 0.5);
        let (y0, y1) = (p.y - p.h * 0.5, p.y + p.h * 0.5);
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

    /// The `take(16)` defect, by example. These inputs are constructed to exercise the boundary
    /// rule, not transcribed from anywhere.
    #[test]
    fn labels_are_cut_on_words_not_mid_word() {
        // "create a rust crate th" is what the 16-char cut produced.
        assert_eq!(elide_words("create a rust crate that parses gguf", 22), "create a rust crate…");
        // A long code token backs up to the last real word rather than being sliced mid-identifier.
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

    /// A box sized as the real projection would size it, so the tests exercise the same arithmetic
    /// the caller uses rather than a stand-in.
    fn p(id: usize, x: f32, y: f32, depth: f32, chars: usize) -> LabelBox {
        let (w, h) = px_extents(chars, 0.034, 900.0, depth);
        LabelBox { id, x, y, depth, w, h }
    }

    #[test]
    fn far_apart_labels_all_survive() {
        // Depths are WORLD UNITS now, not priority scores — the fixtures below used 1.0/0.9/0.5,
        // which under the real projection is a label a metre from the camera and three thousand
        // pixels wide. The tests failed on that and were right to: a fixture that cannot occur is
        // not evidence about a function that now does real arithmetic.
        let v = visible(vec![p(0, 0.0, 0.0, 20.0, 10), p(1, 500.0, 0.0, 19.0, 10),
                             p(2, 0.0, 300.0, 18.0, 10)]);
        assert_eq!(v.len(), 3);
    }

    /// Several turns close together, labels on top of each other — the case the pearl-radius pass
    /// in `chat.rs` cannot see, because those labels are at similar depth.
    #[test]
    fn a_crowded_cluster_keeps_the_nearest_and_drops_the_rest() {
        let cluster: Vec<LabelBox> = (0..5)
            // id 0 is nearest (smallest depth), so it is the one that must survive
            .map(|i| p(i, 100.0 + i as f32 * 6.0, 200.0 + i as f32 * 2.0, 20.0 + i as f32, 16))
            .collect();
        let v = visible(cluster);
        assert_eq!(v, vec![0], "only the nearest of a stacked cluster should be drawn");
    }

    /// Priority is depth, not order of arrival — the label you are closest to is the one you are
    /// looking at.
    #[test]
    fn the_nearer_label_wins_a_collision_whatever_the_input_order() {
        let a = p(7, 100.0, 100.0, 60.0, 12);  // far (large depth)
        let b = p(3, 104.0, 100.0, 20.0, 12);  // near (small depth)
        assert_eq!(visible(vec![a, b]), vec![3]);
        assert_eq!(visible(vec![b, a]), vec![3]);
    }

    /// A declutter that flickers between two equally-close labels is worse than either choice.
    #[test]
    fn a_depth_tie_is_broken_stably() {
        let a = p(9, 100.0, 100.0, 25.0, 12);
        let b = p(4, 103.0, 100.0, 25.0, 12);
        assert_eq!(visible(vec![a, b]), vec![4], "lower id wins a tie");
        assert_eq!(visible(vec![b, a]), vec![4], "and does so regardless of order");
    }

    /// The correction over the old rule: width matters. A long label and a short one at the same
    /// separation are not the same case, and a fixed radius around the node cannot tell them apart.
    #[test]
    fn label_width_decides_the_collision_not_a_fixed_radius() {
        let gap = 60.0;
        let short = vec![p(0, 0.0, 0.0, 20.0, 4), p(1, gap, 0.0, 19.0, 4)];
        let long = vec![p(0, 0.0, 0.0, 20.0, 20), p(1, gap, 0.0, 19.0, 20)];
        assert_eq!(visible(short).len(), 2, "short labels fit side by side");
        assert_eq!(visible(long).len(), 1, "wide ones at the same gap do not");
    }

    #[test]
    fn an_empty_label_takes_no_space_and_is_not_drawn() {
        let v = visible(vec![p(0, 0.0, 0.0, 1.0, 0), p(1, 2.0, 0.0, 25.0, 8)]);
        assert_eq!(v, vec![1], "a node with nothing to say must not block its neighbour");
    }

    /// THE BUG THE CHARACTER-COUNT VERSION HAD. Perspective is the whole reason these boxes cannot
    /// share a width: the same text at twice the distance covers half the screen.
    #[test]
    fn a_far_label_occupies_less_screen_than_a_near_one() {
        let (near_w, near_h) = px_extents(16, 0.034, 900.0, 5.0);
        let (far_w, far_h) = px_extents(16, 0.034, 900.0, 20.0);
        assert!((near_w / far_w - 4.0).abs() < 0.01, "4x the depth should be 1/4 the width");
        assert!(near_h > far_h);
        // and the extents are the projection's own arithmetic, not an estimate
        assert!((near_w - 16.0 * 6.0 * 0.034 * 900.0 / 5.0).abs() < 0.01);
    }

    /// Two labels the old constant would have called colliding do not, once the far one is measured
    /// at its real size — which is the over-suppression that version caused.
    #[test]
    fn distance_can_resolve_a_collision_the_flat_measure_would_report() {
        // Same text, same screen gap; one of them is far away and therefore small.
        let near = p(0, 0.0, 0.0, 30.0, 18);      // ~110px wide
        let far = p(1, 80.0, 0.0, 120.0, 18);     // same text, four times further, ~28px
        assert_eq!(visible(vec![near, far]).len(), 2, "the far label is narrow and clears the gap");
        // bring it to the same depth and the identical screen gap now collides
        let both_near = vec![p(0, 0.0, 0.0, 30.0, 18), p(1, 80.0, 0.0, 30.0, 18)];
        assert_eq!(visible(both_near).len(), 1);
    }

    #[test]
    fn a_label_at_the_camera_plane_has_no_extent_and_is_skipped() {
        assert_eq!(px_extents(10, 0.034, 900.0, 0.0), (0.0, 0.0));
        assert_eq!(px_extents(10, 0.034, 900.0, f32::NAN), (0.0, 0.0));
        let v = visible(vec![LabelBox { id: 0, x: 0.0, y: 0.0, depth: 0.05, w: 0.0, h: 0.0 },
                             p(1, 0.0, 0.0, 25.0, 8)]);
        assert_eq!(v, vec![1]);
    }

    #[test]
    fn a_label_is_pushed_away_from_the_middle() {
        let c = Vec3::new(10.0, 2.0, 10.0);
        // due +x of centre → pushed further +x, same z
        let r = outward(Vec3::new(15.0, 2.0, 10.0), c, 2.0);
        assert!((r.x - 17.0).abs() < 0.001 && (r.z - 10.0).abs() < 0.001, "{r:?}");
        // diagonal keeps its direction and gains exactly `push` in ground-plane length
        let n = Vec3::new(13.0, 2.0, 14.0);
        let d = outward(n, c, 2.5);
        let before = (n.x - c.x).hypot(n.z - c.z);
        let after = (d.x - c.x).hypot(d.z - c.z);
        assert!((after - (before + 2.5)).abs() < 0.001, "{after} vs {}", before + 2.5);
    }

    /// GROUND PLANE ONLY. The graph spreads in x/z, so lifting a label vertically moves it off its
    /// node without gaining any room.
    #[test]
    fn the_push_never_changes_the_height() {
        let c = Vec3::new(0.0, 0.0, 0.0);
        for n in [Vec3::new(3.0, 7.5, 4.0), Vec3::new(-2.0, -1.0, 0.5), Vec3::new(0.0, 9.0, 6.0)] {
            assert_eq!(outward(n, c, 1.0).y, n.y, "{n:?} changed height");
        }
    }

    /// The centre has no outward direction. Normalising a zero vector gives NaN, and a NaN position
    /// reaches Color32 arithmetic and panics a frame later — the projection loop already carries a
    /// guard against exactly that.
    #[test]
    fn a_node_at_the_centre_keeps_its_position_rather_than_becoming_nan() {
        let c = Vec3::new(10.0, 2.0, 10.0);
        let same = outward(c, c, 3.0);
        assert!((same.x - c.x).abs() < 1e-6 && (same.z - c.z).abs() < 1e-6);
        let almost = outward(Vec3::new(10.0, 2.0, 10.00002), c, 3.0);
        assert!(almost.x.is_finite() && almost.y.is_finite() && almost.z.is_finite(), "{almost:?}");
    }

    /// The point of pushing outward: a cluster that collides on the nodes stops colliding once the
    /// labels ring it, so nothing has to be hidden.
    #[test]
    fn pushing_outward_resolves_collisions_that_hiding_would_have_cost_information() {
        // Five nodes in a TIGHT ring, all at the same depth. The radius matters and the first
        // version of this fixture got it wrong: at 26px the labels are 46px wide but only 5px
        // tall, so the ring separated them vertically and nothing collided — the test passed the
        // packed case and proved nothing. 6px puts them genuinely on top of each other.
        let c = (300.0, 300.0);
        let at = |i: usize| {
            let a = i as f32 * std::f32::consts::TAU / 5.0;
            (c.0 + a.cos() * 6.0, c.1 + a.sin() * 6.0)
        };
        let packed: Vec<LabelBox> = (0..5).map(|i| {
            let (x, y) = at(i);
            let (w, h) = px_extents(10, 0.034, 900.0, 40.0);
            LabelBox { id: i, x, y, depth: 40.0, w, h }
        }).collect();
        let kept_packed = visible(packed).len();
        assert!(kept_packed < 5, "premise: the packed ring must actually collide, kept {kept_packed}");

        let spread: Vec<LabelBox> = (0..5).map(|i| {
            let w = outward(Vec3::new(at(i).0, 0.0, at(i).1), Vec3::new(c.0, 0.0, c.1), 90.0);
            let (x, y) = (w.x, w.z);
            let (w, h) = px_extents(10, 0.034, 900.0, 40.0);
            LabelBox { id: i, x, y, depth: 40.0, w, h }
        }).collect();
        let kept_spread = visible(spread).len();

        assert!(kept_spread > kept_packed,
                "pushing outward should keep more labels: {kept_spread} vs {kept_packed}");
    }

    #[test]
    fn nothing_placed_is_nothing_drawn() {
        assert!(visible(Vec::new()).is_empty());
    }
}
