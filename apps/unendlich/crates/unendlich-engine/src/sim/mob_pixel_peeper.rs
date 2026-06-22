//! `sim::mob_pixel_peeper` — a pure "pixel peeper" that rasterises a voxel mob's
//! boxes to a 2D coverage mask and detects **interior holes**: uncovered pixels
//! that are fully enclosed by mesh, i.e. places where a white background would
//! bleed through a gap that opened between boxes. Used to catch limbs that pull
//! apart mid-animation: a watertight pose has zero interior holes; a gap that
//! opens when a leg swings shows up as an enclosed hole. View-independent of the
//! silhouette moving (a limb swinging away leaves *exterior* background, which is
//! reachable from the border and not counted). Pure, deterministic, panic-free.

use crate::sim::dragon_forge::ForgeBox;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Side,  // drop X — see Z(forward)×Y(up)
    Front, // drop Z — see X×Y
    Top,   // drop Y — see X×Z
}

// project a box's extent onto the two kept axes → (u0,u1,v0,v1)
fn project(b: &ForgeBox, view: View) -> (f32, f32, f32, f32) {
    let (ui, vi) = match view {
        View::Side => (2usize, 1usize),
        View::Front => (0, 1),
        View::Top => (0, 2),
    };
    (
        b.center[ui] - b.half[ui],
        b.center[ui] + b.half[ui],
        b.center[vi] - b.half[vi],
        b.center[vi] + b.half[vi],
    )
}

fn bounds2d(boxes: &[ForgeBox], view: View) -> (f32, f32, f32, f32) {
    let mut umin = f32::MAX;
    let mut umax = f32::MIN;
    let mut vmin = f32::MAX;
    let mut vmax = f32::MIN;
    for b in boxes {
        let (u0, u1, v0, v1) = project(b, view);
        umin = umin.min(u0);
        umax = umax.max(u1);
        vmin = vmin.min(v0);
        vmax = vmax.max(v1);
    }
    (umin, umax, vmin, vmax)
}

fn raster(boxes: &[ForgeBox], view: View, bnd: (f32, f32, f32, f32), w: usize, h: usize) -> Vec<bool> {
    let (umin, umax, vmin, vmax) = bnd;
    let uspan = (umax - umin).max(1e-3);
    let vspan = (vmax - vmin).max(1e-3);
    let mut mask = vec![false; w * h];
    for b in boxes {
        let (u0, u1, v0, v1) = project(b, view);
        let i0 = (((u0 - umin) / uspan) * w as f32).floor().max(0.0) as usize;
        let i1 = (((u1 - umin) / uspan) * w as f32).ceil() as usize;
        let j0 = (((v0 - vmin) / vspan) * h as f32).floor().max(0.0) as usize;
        let j1 = (((v1 - vmin) / vspan) * h as f32).ceil() as usize;
        for j in j0..j1.min(h) {
            for i in i0..i1.min(w) {
                mask[j * w + i] = true;
            }
        }
    }
    mask
}

fn grid_dims(bnd: (f32, f32, f32, f32), res: usize) -> (usize, usize) {
    let res = res.max(8);
    let uspan = (bnd.1 - bnd.0).max(1e-3);
    let vspan = (bnd.3 - bnd.2).max(1e-3);
    let cell = vspan / res as f32;
    let w = ((uspan / cell).ceil() as usize).clamp(8, 4 * res);
    (w, res)
}

/// Rasterise the boxes to a `res`-tall coverage mask (width scaled to aspect).
/// Conservative fill — a cell is covered if any box's projected rect overlaps it,
/// so a watertight model never shows rasterisation holes; only real gaps do.
pub fn coverage(boxes: &[ForgeBox], view: View, res: usize) -> (Vec<bool>, usize, usize) {
    let res = res.max(8);
    if boxes.is_empty() {
        return (vec![false; res * res], res, res);
    }
    let bnd = bounds2d(boxes, view);
    let (w, h) = grid_dims(bnd, res);
    (raster(boxes, view, bnd, w, h), w, h)
}

/// Count uncovered cells NOT reachable from the border — enclosed holes where the
/// background would bleed through the mesh. Flood-fills exterior background from
/// the edges (4-connectivity); whatever uncovered stays unvisited is interior.
pub fn interior_holes(mask: &[bool], w: usize, h: usize) -> usize {
    if w == 0 || h == 0 || mask.len() < w * h {
        return 0;
    }
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let push = |x: usize, y: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
        let k = y * w + x;
        if !mask[k] && !seen[k] {
            seen[k] = true;
            stack.push(k);
        }
    };
    for x in 0..w {
        push(x, 0, &mut seen, &mut stack);
        push(x, h - 1, &mut seen, &mut stack);
    }
    for y in 0..h {
        push(0, y, &mut seen, &mut stack);
        push(w - 1, y, &mut seen, &mut stack);
    }
    while let Some(k) = stack.pop() {
        let (x, y) = (k % w, k / w);
        if x > 0 { push(x - 1, y, &mut seen, &mut stack); }
        if x + 1 < w { push(x + 1, y, &mut seen, &mut stack); }
        if y > 0 { push(x, y - 1, &mut seen, &mut stack); }
        if y + 1 < h { push(x, y + 1, &mut seen, &mut stack); }
    }
    let mut holes = 0;
    for k in 0..w * h {
        if !mask[k] && !seen[k] {
            holes += 1;
        }
    }
    holes
}

/// Convenience: the worst interior-hole count across the standard views.
pub fn worst_holes(boxes: &[ForgeBox], res: usize) -> usize {
    let mut worst = 0;
    for v in [View::Side, View::Front, View::Top] {
        let (m, w, h) = coverage(boxes, v, res);
        worst = worst.max(interior_holes(&m, w, h));
    }
    worst
}

/// THE bleed test, to the spec: count pixels that were **covered at rest** but in
/// the animated pose are **uncovered AND enclosed by mesh** — i.e. a white
/// background bleeding through a seam that tore open mid-animation. A limb swinging
/// away from its rest spot leaves *exterior* background (reachable from the border),
/// which is correctly NOT counted. Robust to inflation sealing exterior concavities
/// (those were uncovered at rest, so they can't be "newly visible").
pub fn bleed_pixels(rest: &[bool], anim: &[bool], w: usize, h: usize) -> usize {
    if w == 0 || h == 0 || rest.len() < w * h || anim.len() < w * h {
        return 0;
    }
    // exterior of the animated pose = background reachable from the border
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let push = |x: usize, y: usize, seen: &mut [bool], st: &mut Vec<usize>| {
        let k = y * w + x;
        if !anim[k] && !seen[k] {
            seen[k] = true;
            st.push(k);
        }
    };
    for x in 0..w {
        push(x, 0, &mut seen, &mut stack);
        push(x, h - 1, &mut seen, &mut stack);
    }
    for y in 0..h {
        push(0, y, &mut seen, &mut stack);
        push(w - 1, y, &mut seen, &mut stack);
    }
    while let Some(k) = stack.pop() {
        let (x, y) = (k % w, k / w);
        if x > 0 { push(x - 1, y, &mut seen, &mut stack); }
        if x + 1 < w { push(x + 1, y, &mut seen, &mut stack); }
        if y > 0 { push(x, y - 1, &mut seen, &mut stack); }
        if y + 1 < h { push(x, y + 1, &mut seen, &mut stack); }
    }
    let mut bleed = 0;
    for k in 0..w * h {
        // covered at rest, now uncovered, and enclosed (not exterior) ⇒ a torn seam
        if rest[k] && !anim[k] && !seen[k] {
            bleed += 1;
        }
    }
    bleed
}

/// Worst bleed across the standard views for an animated pose vs the rest pose.
pub fn worst_bleed(rest_boxes: &[ForgeBox], anim_boxes: &[ForgeBox], res: usize) -> usize {
    let mut worst = 0;
    for v in [View::Side, View::Front, View::Top] {
        let br = bounds2d(rest_boxes, v);
        let ba = bounds2d(anim_boxes, v);
        // shared bounds (union) so the two masks align pixel-for-pixel
        let bnd = (br.0.min(ba.0), br.1.max(ba.1), br.2.min(ba.2), br.3.max(ba.3));
        let (w, h) = grid_dims(bnd, res);
        let r = raster(rest_boxes, v, bnd, w, h);
        let a = raster(anim_boxes, v, bnd, w, h);
        worst = worst.max(bleed_pixels(&r, &a, w, h));
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::dragon_forge::{build, skin, ForgeAnim, ForgeBox};
    use crate::sim::dragon_archetype::Genome;

    fn solid_block() -> Vec<ForgeBox> {
        // a 3x3 watertight wall of unit boxes → no interior holes
        let mut v = Vec::new();
        for y in 0..3 {
            for x in 0..3 {
                v.push(ForgeBox { center: [x as f32, y as f32, 0.0], half: [0.5, 0.5, 0.5], glow: false, mouth: false, joint: 0 });
            }
        }
        v
    }

    #[test]
    fn solid_has_no_holes_a_ring_has_one() {
        assert_eq!(worst_holes(&solid_block(), 48), 0, "a solid block is watertight");
        // remove the centre → an enclosed hole
        let mut ring = solid_block();
        ring.retain(|b| !(b.center[0] == 1.0 && b.center[1] == 1.0));
        let (m, w, h) = coverage(&ring, View::Front, 48);
        assert!(interior_holes(&m, w, h) > 0, "a ring around a gap is an interior hole");
    }

    #[test]
    fn moving_limb_away_is_not_a_hole() {
        // two separate boxes far apart: the gap between them is exterior, not a hole
        let v = vec![
            ForgeBox { center: [0.0, 0.0, 0.0], half: [0.5, 0.5, 0.5], glow: false, mouth: false, joint: 0 },
            ForgeBox { center: [5.0, 0.0, 0.0], half: [0.5, 0.5, 0.5], glow: false, mouth: false, joint: 0 },
        ];
        assert_eq!(worst_holes(&v, 48), 0, "a wide gap is open background, not an enclosed hole");
    }

    #[test]
    fn dragon_does_not_bleed_across_the_animation_sweep() {
        // THE pixel-peeper: a white background must never show through the mesh in a
        // region that was covered at rest. Build several dragons, sweep every
        // animation state, and assert no torn-seam bleed.
        let res = 56;
        let tol = 8; // no throat occluder (it rendered as a black box) — an open maw
                     // showing a few px of background is acceptable; tol covers it
        for seed in [1u64, 7, 13, 21] {
            let m = build(&Genome::from_seed(seed), 64);
            let rest = m.boxes.clone();
            for gi in 0..8 {
                for ti in 0..6 {
                    let gait = gi as f32 * std::f32::consts::FRAC_PI_4;
                    let t = ti as f32 * 0.6;
                    let boxes = skin(&m, &ForgeAnim { gait_phase: gait, anim_time: t, ..Default::default() });
                    let bleed = worst_bleed(&rest, &boxes, res);
                    assert!(bleed <= tol, "seed {seed} gait {gait:.2} t {t:.2}: {bleed} bleed px (white showed through a torn seam)");
                }
            }
        }
    }
}
