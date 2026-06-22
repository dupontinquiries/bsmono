//! horn_fractal.rs — pure recursive horn branch geometry (exotic tier).
//!
//! A deterministic L-system-ish expansion: `pairs` symmetric horn pairs, each a
//! base horn that recursively forks into two swept children down to `depth`.
//! Left/right horns of a pair are exact mirror images across X. Output is
//! capped so a deep recursion can't explode. Pure, deterministic, panic-free
//! (R1/R11/R21).

/// One horn segment: where it starts, its (unit) direction, length and a scale
/// (taper) factor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Branch {
    pub base: [f32; 3],
    pub dir: [f32; 3],
    pub len: f32,
    pub scale: f32,
}

/// Hard cap on total branches.
pub const MAX_BRANCHES: usize = 4096;

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn rotate_z(v: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
}

fn grow(
    out: &mut Vec<Branch>,
    base: [f32; 3],
    dir: [f32; 3],
    len: f32,
    scale: f32,
    depth_left: u8,
    bf: f32,
    sweep: f32,
    cap: usize,
) {
    if out.len() >= cap {
        return;
    }
    out.push(Branch { base, dir, len, scale });
    if depth_left == 0 {
        return;
    }
    let tip = [base[0] + dir[0] * len, base[1] + dir[1] * len, base[2] + dir[2] * len];
    for &s in &[1.0f32, -1.0] {
        if out.len() >= cap {
            return;
        }
        let child_dir = normalize(rotate_z(dir, s * sweep));
        grow(out, tip, child_dir, len * bf, scale * bf, depth_left - 1, bf, sweep, cap);
    }
}

/// Build the horn geometry. `pairs` symmetric pairs, recursion `depth`, children
/// scaled by `branch_factor`, bent by `sweep` radians. Total branches are capped
/// at [`MAX_BRANCHES`].
pub fn branches(pairs: u8, depth: u8, branch_factor: f32, sweep: f32) -> Vec<Branch> {
    let bf = branch_factor;
    let mut out: Vec<Branch> = Vec::new();
    for p in 0..pairs {
        if out.len() >= MAX_BRANCHES {
            break;
        }
        let y = p as f32 * 0.3;
        // build the LEFT horn into a temp, then append it and its X-mirror
        let mut left: Vec<Branch> = Vec::new();
        grow(
            &mut left,
            [0.2, y, 0.0],
            normalize([0.5, 1.0, 0.0]),
            1.0,
            1.0,
            depth,
            bf,
            sweep,
            MAX_BRANCHES, // bound the subtree generously; final cap applied on append
        );
        for b in &left {
            if out.len() >= MAX_BRANCHES {
                break;
            }
            out.push(*b);
        }
        for b in &left {
            if out.len() >= MAX_BRANCHES {
                break;
            }
            out.push(Branch {
                base: [-b.base[0], b.base[1], b.base[2]],
                dir: [-b.dir[0], b.dir[1], b.dir[2]],
                len: b.len,
                scale: b.scale,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_zero_is_base_horns_only() {
        for pairs in 1..=5u8 {
            let b = branches(pairs, 0, 0.7, 0.3);
            assert_eq!(b.len(), 2 * pairs as usize, "depth 0 should be 2 per pair");
        }
    }

    #[test]
    fn deeper_makes_more_branches_bounded() {
        let d0 = branches(2, 0, 0.7, 0.3).len();
        let d1 = branches(2, 1, 0.7, 0.3).len();
        let d2 = branches(2, 2, 0.7, 0.3).len();
        assert!(d1 > d0 && d2 > d1, "deeper should add branches: {} {} {}", d0, d1, d2);
        // very deep stays capped
        assert!(branches(4, 30, 0.7, 0.3).len() <= MAX_BRANCHES, "cap exceeded");
    }

    #[test]
    fn branch_factor_scales_children() {
        // child len == parent len * branch_factor
        let bf = 0.6f32;
        let b = branches(1, 1, bf, 0.2);
        // b[0] = base horn (len 1.0), b[1] = its first child
        assert!((b[0].len - 1.0).abs() < 1e-6);
        assert!((b[1].len - bf).abs() < 1e-6, "child len not scaled: {}", b[1].len);
        assert!((b[1].scale - bf).abs() < 1e-6);
    }

    #[test]
    fn sweep_bends_child_dirs() {
        let straight = branches(1, 1, 0.7, 0.0);
        // with no sweep, child dir == parent dir
        assert!((straight[1].dir[0] - straight[0].dir[0]).abs() < 1e-5);
        let bent = branches(1, 1, 0.7, 0.5);
        // with sweep, at least one child differs from the parent direction
        let diff = (bent[1].dir[0] - bent[0].dir[0]).abs() + (bent[1].dir[1] - bent[0].dir[1]).abs();
        assert!(diff > 1e-3, "sweep did not bend the child");
    }

    #[test]
    fn pairs_are_x_mirror_symmetric() {
        // for one pair (no cap hit), the second half mirrors the first across X
        let b = branches(1, 3, 0.7, 0.4);
        assert_eq!(b.len() % 2, 0, "expected even count");
        let k = b.len() / 2;
        for i in 0..k {
            let l = b[i];
            let r = b[k + i];
            assert!((r.base[0] + l.base[0]).abs() < 1e-6, "base.x not mirrored");
            assert_eq!(r.base[1], l.base[1]);
            assert!((r.dir[0] + l.dir[0]).abs() < 1e-6, "dir.x not mirrored");
            assert_eq!(r.len, l.len);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(branches(3, 2, 0.65, 0.35), branches(3, 2, 0.65, 0.35));
    }
}
