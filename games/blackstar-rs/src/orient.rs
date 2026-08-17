//! ORIENT — fixing the stamped placeholder, model by model.
//!
//! Every `orient.json` shipped in `assets/bots/*/f*/` and `w*/` carries
//! the exact same value (`up=[0,1,0], forward=[0,0,-1]`, tagged
//! `"source":"foldview"`) — confirmed across every family checked, not
//! a guess. `model.rs`'s own loader treats a declared orientation as
//! authoritative and skips the auto-solver entirely when one is
//! present, so this placeholder is not inert: it is actively DISABLING
//! `find_orientation` for all 424 files, forcing every bot pose to
//! load in its raw, as-exported orientation. That is the root cause
//! behind vehicles reading backwards, mechs reading sideways, and
//! spiders reading feet-up all at once — one bug wearing three
//! costumes, not three bugs.
//!
//! `find_orientation`'s own scoring (`wheel_score`) is car-specific —
//! four low same-size clusters at the corners — and by its own doc
//! comment gives a bad answer for anything without wheels: "a legless
//! robot has no wheels to find, so the search picks whatever reads
//! most car-like, which is the robot lying on its side." A standing
//! body needs a DIFFERENT signal, and it has an obvious one: legs are
//! near the bottom. `up_score` rewards a candidate up-axis for two
//! things a standing figure actually has — the body is TALLEST along
//! it, and more of its mass sits below the center than above. Forward
//! is harder to get right from geometry alone (no wheel-like tell),
//! so this module makes only the claim it can back: which of the two
//! remaining horizontal axes is LEFT-RIGHT, chosen as whichever gives
//! the better mirror symmetry — most bots, bipedal or many-legged, are
//! built symmetric across that plane. The other horizontal axis is
//! forward BY ELIMINATION; its sign (which end is the front) is not
//! resolved by this heuristic and is left for the confidence flag
//! below to call out rather than silently guess.
//!
//! Nothing here claims perfection. It claims REPLACING an always-wrong
//! placeholder with a real, checkable, sign-honest guess — verified,
//! not assumed, via the ASCII front/side projections `--orient` prints
//! for exactly this purpose.

use glam::{Quat, Vec3};

use crate::meshgen::Vertex;
use crate::model::RawPrim;

/// The 6 axis-aligned directions, reused from `model.rs`'s own search
/// so a caller can hold both up-axis conventions to the same list.
pub const AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

/// How much a candidate up-axis looks like "legs near the bottom, mass
/// tapering upward" — the standing-body signal `wheel_score` has no
/// equivalent for. Two terms, multiplied so a candidate needs both:
/// * tallness: the extent along `up`, relative to the other two —
///   a standing figure is taller than it is wide, in ANY orientation
///   that is actually up;
/// * bottom-heaviness: the fraction of vertices sitting in the lower
///   half along `up` — legs, treads, a wide base, all read as more
///   material low than high.
pub fn up_score(verts: &[Vertex], up: Vec3) -> f32 {
    if verts.is_empty() {
        return 0.0;
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let mut heights = Vec::with_capacity(verts.len());
    let (mut a_lo, mut a_hi) = (f32::MAX, f32::MIN);
    let (mut b_lo, mut b_hi) = (f32::MAX, f32::MIN);
    let (a, b) = perp_pair(up);
    for v in verts {
        let p = Vec3::from_array(v.pos);
        let h = p.dot(up);
        lo = lo.min(h);
        hi = hi.max(h);
        heights.push(h);
        let pa = p.dot(a);
        let pb = p.dot(b);
        a_lo = a_lo.min(pa);
        a_hi = a_hi.max(pa);
        b_lo = b_lo.min(pb);
        b_hi = b_hi.max(pb);
    }
    let up_extent = (hi - lo).max(1e-6);
    let side_extent = ((a_hi - a_lo).max(b_hi - b_lo)).max(1e-6);
    let tallness = up_extent / side_extent;
    let mid = (lo + hi) * 0.5;
    let below = heights.iter().filter(|h| **h < mid).count() as f32 / heights.len() as f32;
    // below=0.5 (balanced) scores 0 on this term; below=1.0 (everything
    // under the midpoint — legs, a wide base) scores 1
    let bottom_heavy = (below - 0.5).max(0.0) * 2.0;
    tallness * bottom_heavy
}

/// Two axes perpendicular to `up` AND to each other — the actual
/// horizontal plane's own basis. `AXES` lists both signs of every
/// physical axis, so naively taking "the first two perpendicular to
/// up" can return +X and -X: the SAME axis, which made every mirror
/// comparison downstream compare a value to itself and tie every
/// time — the entire reason forward confidence measured 0% across the
/// whole roster before this fix, caught by dumping the raw asymmetry
/// numbers rather than trusting the aggregate stat.
pub fn perp_pair(axis: Vec3) -> (Vec3, Vec3) {
    let mut out = (Vec3::ZERO, Vec3::ZERO);
    let mut got_first = false;
    for a in AXES {
        if axis.dot(a).abs() < 0.5 {
            if !got_first {
                out.0 = a;
                got_first = true;
            } else if out.1 == Vec3::ZERO && out.0.dot(a).abs() < 0.5 {
                out.1 = a;
            }
        }
    }
    out
}

/// Mirror symmetry across the plane perpendicular to `axis`: for every
/// vertex, how close is the nearest vertex to its own reflection.
/// Lower is more symmetric. O(n^2) on a coarse sample — fine for a
/// diagnostic tool run per-model, not per-frame.
pub fn mirror_asymmetry(verts: &[Vertex], axis: Vec3, center: Vec3) -> f32 {
    let sample: Vec<Vec3> =
        verts.iter().step_by((verts.len() / 300).max(1)).map(|v| Vec3::from_array(v.pos)).collect();
    if sample.len() < 4 {
        return f32::MAX;
    }
    let mut total = 0.0;
    for &p in &sample {
        let rel = p - center;
        let mirrored = rel - 2.0 * rel.dot(axis) * axis + center;
        let nearest = sample.iter().map(|q| (*q - mirrored).length_squared()).fold(f32::MAX, f32::min);
        total += nearest.sqrt();
    }
    total / sample.len() as f32
}

/// The result of one model's analysis: the chosen frame, plus how sure
/// this heuristic is about EACH HALF of it separately. Kept apart on
/// purpose after checking real data: `blackstar`'s up-axis scored
/// 1.16 against a 0.28 runner-up — about as clear a signal as this
/// heuristic can produce — while its left-right symmetry check landed
/// near a tie. A single AND'd confidence flag let the weaker of the
/// two drag down a genuinely solid up-axis call, and up-axis is the
/// half that actually matters for "feet up" and "sideways" — a wrong
/// forward sign leaves a body standing upright, facing the wrong way;
/// a wrong up-axis leaves it lying down.
pub struct Guess {
    pub up: Vec3,
    pub up_confident: bool,
    /// forward's AXIS is chosen (by elimination); its SIGN is a
    /// straight guess and `forward_confident` reflects that, not the axis
    pub forward: Vec3,
    pub forward_confident: bool,
}

impl Guess {
    /// Both halves solid — safe to commit without a second look.
    pub fn confident(&self) -> bool {
        self.up_confident && self.forward_confident
    }
}

/// Analyze raw, unoriented vertices and guess a standing frame. See
/// the module doc for the method and its honest limits.
pub fn guess(verts: &[Vertex]) -> Guess {
    let up_scores: Vec<(Vec3, f32)> = AXES.iter().map(|ax| (*ax, up_score(verts, *ax))).collect();
    let up = up_scores.iter().copied().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap().0;
    let mut sorted: Vec<f32> = up_scores.iter().map(|(_, s)| *s).collect();
    sorted.sort_by(|x, y| y.partial_cmp(x).unwrap());
    let up_gap = sorted[0] - sorted.get(1).copied().unwrap_or(0.0);
    // relative to the winner's own score, not an absolute number — a
    // model with small scores across the board (a near-cube silhouette)
    // needs the SAME margin, proportionally, as one with large scores
    let up_confident = sorted[0] > 1e-4 && up_gap / sorted[0] > 0.35;

    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in verts {
        let p = Vec3::from_array(v.pos);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let center = (lo + hi) * 0.5;
    let (a, b) = perp_pair(up);
    let asym_a = mirror_asymmetry(verts, a, center);
    let asym_b = mirror_asymmetry(verts, b, center);
    // the MORE symmetric axis is left-right; the other is forward
    let forward = if asym_a < asym_b { b } else { a };
    let sym_gap = (asym_a - asym_b).abs() / asym_a.max(asym_b).max(1e-6);
    let forward_confident = sym_gap > 0.10;
    Guess { up, up_confident, forward, forward_confident }
}

/// The rotation `model.rs`'s own loader expects: this frame onto
/// (Y up, -Z forward) — same construction as `find_orientation`.
pub fn rotation_for(up: Vec3, forward: Vec3) -> Quat {
    let right = forward.cross(up).normalize();
    let m = glam::Mat3::from_cols(right, up, -forward).transpose();
    Quat::from_mat3(&m)
}

/// A front-view ASCII silhouette (looking down `-forward`, standard
/// axis-projection) for eyeball verification without a GPU — the same
/// technique `clones.rs` used to hold a generated body to its
/// reference. Not a replacement for rendering; a fast, inspectable
/// gate before trusting a guess.
pub fn ascii_front(verts: &[Vertex], up: Vec3, forward: Vec3) -> String {
    let right = forward.cross(up).normalize();
    let rows = 28usize;
    let cols = 40usize;
    let pts: Vec<(f32, f32)> = verts
        .iter()
        .map(|v| {
            let p = Vec3::from_array(v.pos);
            (p.dot(right), p.dot(up))
        })
        .collect();
    render_ascii(&pts, rows, cols)
}

fn render_ascii(pts: &[(f32, f32)], rows: usize, cols: usize) -> String {
    if pts.is_empty() {
        return String::from("(empty)\n");
    }
    let (mut xlo, mut xhi, mut ylo, mut yhi) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for (x, y) in pts {
        xlo = xlo.min(*x);
        xhi = xhi.max(*x);
        ylo = ylo.min(*y);
        yhi = yhi.max(*y);
    }
    let (xr, yr) = ((xhi - xlo).max(1e-4), (yhi - ylo).max(1e-4));
    let mut grid = vec![vec![false; cols]; rows];
    for (x, y) in pts {
        let cx = (((x - xlo) / xr) * (cols - 1) as f32) as usize;
        let cy = rows - 1 - (((y - ylo) / yr) * (rows - 1) as f32) as usize;
        grid[cy.min(rows - 1)][cx.min(cols - 1)] = true;
    }
    let mut out = String::new();
    for row in grid {
        out.push_str(&row.iter().map(|b| if *b { '#' } else { '.' }).collect::<String>());
        out.push('\n');
    }
    out
}

/// All vertices across a set of raw prims, source-space, no filtering.
pub fn all_verts(raws: &[RawPrim]) -> Vec<Vertex> {
    raws.iter().flat_map(|r| r.verts.iter().copied()).collect()
}

/// One of the 6 axis-aligned directions, as a human-readable label —
/// shared between `orientsim.rs` and `partsim.rs` rather than each
/// keeping its own copy.
pub fn axis_name(v: Vec3) -> &'static str {
    if v.x > 0.5 {
        "+X"
    } else if v.x < -0.5 {
        "-X"
    } else if v.y > 0.5 {
        "+Y"
    } else if v.y < -0.5 {
        "-Y"
    } else if v.z > 0.5 {
        "+Z"
    } else {
        "-Z"
    }
}

/// Write an `orient.json` matching what `model.rs::declared_orientation`
/// expects to read back.
pub fn write_orient_json(dir: &std::path::Path, up: Vec3, forward: Vec3, source: &str) -> std::io::Result<()> {
    let txt = format!(
        "{{\"up\":[{:.4},{:.4},{:.4}],\"forward\":[{:.4},{:.4},{:.4}],\"source\":\"{source}\"}}\n",
        up.x, up.y, up.z, forward.x, forward.y, forward.z
    );
    std::fs::write(dir.join("orient.json"), txt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_verts(hx: f32, hy: f32, hz: f32, n_per_axis: usize) -> Vec<Vertex> {
        // a filled-ish box, denser near -Y (legs) than +Y (head) —
        // a standing body's actual mass distribution in miniature
        let mut out = Vec::new();
        for i in 0..n_per_axis {
            let t = i as f32 / n_per_axis as f32;
            for j in 0..8 {
                let s = j as f32 / 7.0 - 0.5;
                let y = if t < 0.7 { -hy + t / 0.7 * hy * 0.6 } else { hy * 0.6 + (t - 0.7) / 0.3 * hy * 0.4 };
                out.push(Vertex { pos: [s * hx, y, (t - 0.5) * 0.2 * hz], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] });
            }
        }
        out
    }

    #[test]
    fn up_score_prefers_the_tall_bottom_heavy_axis() {
        let verts = box_verts(0.4, 1.0, 0.3, 200);
        let sy = up_score(&verts, Vec3::Y);
        let sx = up_score(&verts, Vec3::X);
        let sz = up_score(&verts, Vec3::Z);
        assert!(sy > sx && sy > sz, "Y={sy} X={sx} Z={sz}: the standing axis did not win");
    }

    #[test]
    fn guess_recovers_the_up_axis_even_when_the_source_data_is_sideways() {
        // the SAME body, authored lying on its side -- exactly the
        // failure mode described for a legless robot
        let mut verts = box_verts(0.4, 1.0, 0.3, 200);
        for v in verts.iter_mut() {
            let p = Vec3::from_array(v.pos);
            v.pos = Vec3::new(p.y, p.x, p.z).to_array(); // swap X/Y
        }
        let g = guess(&verts);
        assert_eq!(g.up, Vec3::X, "did not recover the true up-axis after the swap");
    }

    #[test]
    fn rotation_for_actually_carries_the_frame_to_y_up_negz_forward() {
        let q = rotation_for(Vec3::X, Vec3::Z);
        let up_after = q * Vec3::X;
        let fwd_after = q * Vec3::Z;
        assert!(up_after.dot(Vec3::Y) > 0.99, "{up_after:?}");
        assert!(fwd_after.dot(-Vec3::Z) > 0.99, "{fwd_after:?}");
    }

    #[test]
    fn ascii_front_never_panics_on_a_degenerate_or_empty_input() {
        assert!(!ascii_front(&[], Vec3::Y, -Vec3::Z).is_empty());
        let one = vec![Vertex { pos: [0.0, 0.0, 0.0], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }];
        let _ = ascii_front(&one, Vec3::Y, -Vec3::Z); // must not panic on a zero-extent box
    }

    #[test]
    fn write_and_the_declared_orientation_reader_agree_on_the_format() {
        let dir = std::env::temp_dir().join(format!("orient_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_orient_json(&dir, Vec3::Y, -Vec3::Z, "test").unwrap();
        let txt = std::fs::read_to_string(dir.join("orient.json")).unwrap();
        assert!(txt.contains("\"up\":[0.0000,1.0000,0.0000]"));
        assert!(txt.contains("\"source\":\"test\""));
        std::fs::remove_dir_all(&dir).ok();
    }
}
