//! Screen-space packing for the spatial view's HUD slabs.
//!
//! The spatial chat scene pins several "physical UI" slabs to window fractions
//! (composer at (0.5, 0.90), button dock at (0.5, 0.87), action row at (0.5, 0.985),
//! reading box top-centre, dial/track panels in the corners). Their SIZES clamp in
//! world units, so on short or narrow windows the projected rects collide — the
//! reported "overlapping UI components". This module resolves that in the one space
//! where overlap is actually defined: screen pixels. Each site projects its desired
//! rect, calls [`place`], then un-projects the resolved centre so the 3D mesh and
//! the egui overlay stay glued together.
//!
//! Design constraints, in order:
//!   1. NEVER overlap an already-placed slab (earlier placement = higher priority —
//!      call order in the scene is the priority order).
//!   2. Stay inside the available rect (minus any reserved chrome band).
//!   3. Move as little as possible, and only along one axis at a time (these are
//!      anchored HUD elements: sliding them keeps spatial identity, teleporting
//!      them across the screen does not).
//!   4. If a slab simply cannot fit without overlap (pathologically small window),
//!      shrink it to the largest gap rather than stacking it on a neighbour.
//! The function is pure and deterministic — unit tests below drive the exact
//! configurations that overlapped in the wild.

use eframe::egui::{Rect, pos2, vec2};

/// One already-placed slab (screen rect, post-resolution).
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub rect: Rect,
}

/// Resolve `desired` against everything already in `placed`, inside `avail`.
/// Returns the final rect and records it. `pad` is the minimum gap kept between
/// slabs (and the border), in px.
pub fn place(placed: &mut Vec<Placed>, avail: Rect, desired: Rect, pad: f32) -> Rect {
    let mut r = clamp_into(shrink_to_fit(desired, avail, pad), avail, pad);
    // Up to a few passes: each collision pushes along the cheapest axis; a push can
    // create a new collision with a different neighbour, so iterate to a fixpoint.
    for _ in 0..8 {
        let Some(hit) = placed.iter().find(|p| intersects(p.rect, r, pad)) else { break };
        r = push_apart(r, hit.rect, avail, pad);
        r = clamp_into(r, avail, pad);
    }
    // Fixpoint failed (window too small for everyone at full size): fit the slab
    // into the largest free horizontal band instead of accepting an overlap.
    if placed.iter().any(|p| intersects(p.rect, r, pad)) {
        r = fit_largest_gap(placed, avail, r, pad);
    }
    placed.push(Placed { rect: r });
    r
}

fn intersects(a: Rect, b: Rect, pad: f32) -> bool {
    a.expand(pad * 0.5).intersects(b.expand(pad * 0.5))
}

/// A slab bigger than the stage shrinks (uniformly) until it fits — a rect that
/// cannot fit cannot be made non-overlapping by any translation.
fn shrink_to_fit(r: Rect, avail: Rect, pad: f32) -> Rect {
    let max_w = (avail.width() - 2.0 * pad).max(8.0);
    let max_h = (avail.height() - 2.0 * pad).max(8.0);
    let s = (max_w / r.width()).min(max_h / r.height()).min(1.0);
    if s >= 1.0 { return r; }
    Rect::from_center_size(r.center(), vec2(r.width() * s, r.height() * s))
}

fn clamp_into(r: Rect, avail: Rect, pad: f32) -> Rect {
    let inner = avail.shrink(pad);
    let mut c = r.center();
    c.x = c.x.clamp(inner.left() + r.width() * 0.5, (inner.right() - r.width() * 0.5).max(inner.left() + r.width() * 0.5));
    c.y = c.y.clamp(inner.top() + r.height() * 0.5, (inner.bottom() - r.height() * 0.5).max(inner.top() + r.height() * 0.5));
    Rect::from_center_size(c, r.size())
}

/// Push `r` out of `other` along the axis needing the smallest move, preferring a
/// direction that stays inside `avail`. Vertical is tried first when the two slabs
/// are roughly the same column (the bottom dock/composer/action-row family) — a
/// horizontal shove there would fling a centred slab into a corner.
fn push_apart(r: Rect, other: Rect, avail: Rect, pad: f32) -> Rect {
    let up = other.top() - r.height() * 0.5 - pad;      // centre.y if placed above `other`
    let down = other.bottom() + r.height() * 0.5 + pad; // centre.y if placed below
    let left = other.left() - r.width() * 0.5 - pad;
    let right = other.right() + r.width() * 0.5 + pad;
    let inner = avail.shrink(pad);
    let fits_y = |cy: f32| cy - r.height() * 0.5 >= inner.top() && cy + r.height() * 0.5 <= inner.bottom();
    let fits_x = |cx: f32| cx - r.width() * 0.5 >= inner.left() && cx + r.width() * 0.5 <= inner.right();
    let c = r.center();
    // candidate moves: (cost, new centre), only those that stay inside
    let mut cands: Vec<(f32, f32, f32)> = Vec::new();
    if fits_y(up) { cands.push(((c.y - up).abs(), c.x, up)); }
    if fits_y(down) { cands.push(((c.y - down).abs(), c.x, down)); }
    let same_column = (c.x - other.center().x).abs() < (r.width() + other.width()) * 0.25;
    let x_penalty = if same_column { 4.0 } else { 1.0 }; // strongly prefer vertical for stacked HUD
    if fits_x(left) { cands.push(((c.x - left).abs() * x_penalty, left, c.y)); }
    if fits_x(right) { cands.push(((c.x - right).abs() * x_penalty, right, c.y)); }
    match cands.into_iter().min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()) {
        Some((_, cx, cy)) => Rect::from_center_size(pos2(cx, cy), r.size()),
        // nowhere inside fits on either axis — take the cheapest even if it clamps
        None => Rect::from_center_size(pos2(c.x, up), r.size()),
    }
}

/// Last resort: find the tallest horizontal band of `avail` free of every placed
/// slab and fit `r` inside it — SHRINKING to the band if needed. The band is the
/// contract: the returned rect is fully inside it, so it cannot overlap anything.
fn fit_largest_gap(placed: &[Placed], avail: Rect, r: Rect, pad: f32) -> Rect {
    let mut edges: Vec<f32> = vec![avail.top() + pad, avail.bottom() - pad];
    for p in placed {
        edges.push(p.rect.top() - pad);
        edges.push(p.rect.bottom() + pad);
    }
    edges.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (mut best_h, mut best_band) = (0.0f32, (avail.top() + pad, avail.bottom() - pad));
    for w in edges.windows(2) {
        let (a, b) = (w[0].max(avail.top() + pad), w[1].min(avail.bottom() - pad));
        if b <= a { continue; }
        let band = Rect::from_min_max(pos2(avail.left() + pad, a), pos2(avail.right() - pad, b));
        if placed.iter().any(|p| p.rect.expand(pad * 0.5).intersects(band)) { continue; }
        if b - a > best_h { best_h = b - a; best_band = (a, b); }
    }
    let (top, bot) = best_band;
    // NO floor here: on a stage too packed to hold this slab, it collapses into the
    // sliver (effectively hidden) — the one honest alternative to overlapping a
    // higher-priority slab. rect ⊆ band is the invariant; a floor would break it.
    let h = r.height().min((bot - top).max(0.0));
    let w = r.width().min(avail.width() - 2.0 * pad).max(0.0);
    let cy = ((top + bot) * 0.5).clamp(top + h * 0.5, (bot - h * 0.5).max(top + h * 0.5));
    let cx = r.center().x.clamp(avail.left() + pad + w * 0.5,
                                (avail.right() - pad - w * 0.5).max(avail.left() + pad + w * 0.5));
    Rect::from_center_size(pos2(cx, cy), vec2(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disjoint(placed: &[Placed], pad: f32) -> bool {
        for i in 0..placed.len() {
            for j in i + 1..placed.len() {
                if intersects(placed[i].rect, placed[j].rect, pad * 0.9) { return false; }
            }
        }
        true
    }
    fn inside(placed: &[Placed], avail: Rect) -> bool {
        placed.iter().all(|p| avail.expand(0.5).contains_rect(p.rect))
    }

    /// The reported bug, exactly: dock (0.87), composer (0.90), action row (0.985)
    /// on a SHORT window — the three bottom-centre slabs collided. After packing,
    /// pairwise disjoint, all inside, and the composer stays in the bottom half
    /// (slid, not teleported).
    #[test]
    fn bottom_trio_short_window() {
        let avail = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 320.0));
        let mut placed = Vec::new();
        let dock = place(&mut placed, avail, Rect::from_center_size(pos2(450.0, 0.87 * 320.0), vec2(420.0, 64.0)), 6.0);
        let comp = place(&mut placed, avail, Rect::from_center_size(pos2(450.0, 0.90 * 320.0), vec2(560.0, 88.0)), 6.0);
        let arow = place(&mut placed, avail, Rect::from_center_size(pos2(450.0, 0.985 * 320.0), vec2(500.0, 40.0)), 6.0);
        assert!(disjoint(&placed, 6.0), "bottom trio still overlaps: {dock:?} {comp:?} {arow:?}");
        assert!(inside(&placed, avail));
        eprintln!("trio: dock={dock:?} comp={comp:?} arow={arow:?}");
        assert!(comp.center().y > avail.height() * 0.35,
                "composer teleported to the top: {comp:?}");
    }

    /// Narrow window: the right column (dial at (0.86, 0.13), files at (0.86, 0.42))
    /// vs the top-centre reading box (0.5, 0.17) whose width min-clamp reaches into
    /// the column. Must resolve without pushing anything off-screen.
    #[test]
    fn right_column_vs_reading_box_narrow() {
        let avail = Rect::from_min_size(pos2(0.0, 0.0), vec2(520.0, 700.0));
        let mut placed = Vec::new();
        place(&mut placed, avail, Rect::from_center_size(pos2(0.86 * 520.0, 0.13 * 700.0), vec2(150.0, 120.0)), 6.0);
        place(&mut placed, avail, Rect::from_center_size(pos2(0.86 * 520.0, 0.42 * 700.0), vec2(150.0, 220.0)), 6.0);
        place(&mut placed, avail, Rect::from_center_size(pos2(260.0, 0.17 * 700.0), vec2(430.0, 160.0)), 6.0);
        assert!(disjoint(&placed, 6.0));
        assert!(inside(&placed, avail));
    }

    /// A slab wider than the whole stage shrinks to fit instead of overhanging.
    #[test]
    fn oversized_slab_shrinks() {
        let avail = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
        let mut placed = Vec::new();
        let r = place(&mut placed, avail, Rect::from_center_size(pos2(150.0, 100.0), vec2(900.0, 90.0)), 6.0);
        assert!(r.width() <= 300.0 && inside(&placed, avail));
    }

    /// Pathological: five slabs on a tiny stage — still pairwise disjoint (the
    /// largest-gap fallback shrinks rather than stacks).
    #[test]
    fn tiny_window_no_overlap_ever() {
        let avail = Rect::from_min_size(pos2(0.0, 0.0), vec2(360.0, 240.0));
        let mut placed = Vec::new();
        for (fx, fy, w, h) in [(0.5, 0.87, 260.0, 50.0), (0.5, 0.90, 300.0, 70.0),
                               (0.5, 0.985, 280.0, 36.0), (0.86, 0.13, 120.0, 90.0),
                               (0.5, 0.17, 260.0, 90.0)] {
            place(&mut placed, avail, Rect::from_center_size(pos2(fx * 360.0, fy * 240.0), vec2(w, h)), 5.0);
        }
        assert!(disjoint(&placed, 5.0), "tiny-window packing overlaps: {placed:?}");
        assert!(inside(&placed, avail));
    }

    /// Determinism: identical inputs, identical outputs.
    #[test]
    fn deterministic() {
        let avail = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 300.0));
        let run = || {
            let mut placed = Vec::new();
            for (fx, fy) in [(0.5, 0.87), (0.5, 0.90), (0.5, 0.985)] {
                place(&mut placed, avail, Rect::from_center_size(pos2(fx * 800.0, fy * 300.0), vec2(420.0, 70.0)), 6.0);
            }
            placed.iter().map(|p| (p.rect.min.x, p.rect.min.y, p.rect.max.x, p.rect.max.y)).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
