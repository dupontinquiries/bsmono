//! anatomy — read a vehicle's body plan off its own symmetry.
//!
//! ## What the previous attempts got wrong
//!
//! Descriptor matching scored 87% between *unrelated* tanks, so it was matching
//! noise. Folding a skeleton into a volume produced "a tank, then a slightly
//! different tank", and the sweep showed why — I had assumed a vehicle is wide
//! and low without ever checking the model's axes.
//!
//! Looking at Brawl's tank by hand takes about a minute and gives the answer
//! directly:
//!
//! ```text
//!  #  verts   centre(X,Y,Z)      extent(X,Y,Z)
//!  1    145  ( 0.30, 0.68, 0.20)  (0.24,0.07,0.07)
//!  2    145  (-0.30, 0.68, 0.20)  (0.24,0.07,0.07)   <- mirror of 1
//!  8    105  (-0.32,-0.12, 0.22)  (0.11,1.41,0.22)
//!  9    105  ( 0.32,-0.12, 0.22)  (0.11,1.41,0.22)   <- mirror of 8: THE TREADS
//! ```
//!
//! Identical vertex counts, identical extents, opposite signs on one axis. A
//! vehicle is **built symmetrically**, and that symmetry is the body plan: what
//! comes in a mirrored pair is a limb, what sits on the centreline is a spine.
//! No shape descriptor, no threshold, no fitting — just arithmetic on positions
//! that a person can check by reading the table.
//!
//! ## Axes are measured, never assumed
//!
//! Brawl's tank is longest in **Y**; his robot is longest in **Z**. The two
//! forms use different conventions, which is exactly the assumption that broke
//! the fold approach. [`Axes::of`] derives them per model from the mass
//! distribution rather than trusting a convention.

/// Which axis is which, for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axes {
    /// Longest: a vehicle's length, a robot's height.
    pub long: usize,
    /// The mirror axis — where paired parts sit at ±. For a vehicle this is
    /// width, and it is the axis symmetry is measured across.
    pub lateral: usize,
    /// Shortest.
    pub short: usize,
}

impl Axes {
    /// Derive from the bounding span. Measured rather than assumed, because
    /// Brawl's two forms disagree about which axis is up.
    pub fn of(span: [f64; 3]) -> Axes {
        let mut i = [0usize, 1, 2];
        i.sort_by(|a, b| span[*b].partial_cmp(&span[*a]).unwrap());
        Axes { long: i[0], lateral: i[1], short: i[2] }
    }
}

/// One component, reduced to what the body plan needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub index: usize,
    pub verts: usize,
    pub centre: [f64; 3],
    pub extent: [f64; 3],
}

impl Part {
    pub fn from_points(index: usize, pts: &[[f64; 3]]) -> Part {
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for p in pts { for i in 0..3 { lo[i] = lo[i].min(p[i]); hi[i] = hi[i].max(p[i]); } }
        Part {
            index, verts: pts.len(),
            centre: [(lo[0]+hi[0])*0.5, (lo[1]+hi[1])*0.5, (lo[2]+hi[2])*0.5],
            extent: [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]],
        }
    }
}

/// A mirrored pair, or a part on the centreline.
#[derive(Debug, Clone, PartialEq)]
pub enum Limb {
    /// Two parts at ±lateral: a limb, a tread, a pair of doors.
    Pair { left: usize, right: usize, verts: usize },
    /// On the centreline: spine, turret, cockpit.
    Spine { part: usize, verts: usize },
    /// Neither mirrored nor central — an asymmetric detail.
    Odd { part: usize, verts: usize },
}

impl Limb {
    pub fn verts(&self) -> usize {
        match self { Limb::Pair { verts, .. } | Limb::Spine { verts, .. }
                     | Limb::Odd { verts, .. } => *verts }
    }
}

/// How close two parts must be to count as mirrors, as a fraction of the
/// model's size. Tight, because a real mirrored pair matches almost exactly —
/// the tank's treads agree to two decimal places — and a loose tolerance is how
/// the descriptor work ended up matching unrelated tanks.
pub const MIRROR_TOL: f64 = 0.04;

/// Within this fraction of the centreline, a part is central rather than
/// unpaired. A turret sits at 0.00 and a tread at ±0.32, so the gap is wide.
pub const CENTRE_TOL: f64 = 0.06;

/// Find the body plan.
///
/// Pairs are matched greedily from the largest part down, because a big part
/// mis-paired is a visible error and a small one is a greeble.
pub fn body_plan(parts: &[Part], span: [f64; 3], axes: Axes) -> Vec<Limb> {
    let scale = span[axes.long].max(1e-9);
    let lat = axes.lateral;
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(parts[*i].verts));
    let mut used = vec![false; parts.len()];
    let mut out = Vec::new();

    // the model's lateral centre, from the parts themselves rather than the
    // bounding box — an asymmetric bumper should not shift the centreline
    let mid: f64 = if parts.is_empty() { 0.0 } else {
        parts.iter().map(|p| p.centre[lat]).sum::<f64>() / parts.len() as f64
    };

    for &i in &order {
        if used[i] { continue; }
        let a = &parts[i];
        let off = a.centre[lat] - mid;
        // PAIRING IS TRIED FIRST, before the centreline test.
        //
        // The other order looks natural and is wrong: Brawl's parts 11 and 12
        // sit at ±0.12, which is exactly CENTRE_TOL of a 2.00 model, so they
        // were claimed as two separate spine parts and the pair was lost. A
        // matching mirror is far stronger evidence than proximity to zero — a
        // part that HAS a twin is a limb whatever its offset.
        let mut best: Option<(f64, usize)> = None;
        for &j in &order {
            if used[j] || j == i { continue; }
            let b = &parts[j];
            if b.verts != a.verts { continue; }   // a true mirror is the same mesh
            let boff = b.centre[lat] - mid;
            if boff * off > 0.0 { continue; }     // same side: not a mirror
            let d = ((boff + off).abs()
                   + (0..3).map(|k| if k == lat { 0.0 }
                                    else { (a.centre[k] - b.centre[k]).abs() }).sum::<f64>()
                   + (0..3).map(|k| (a.extent[k] - b.extent[k]).abs()).sum::<f64>()) / scale;
            if d <= MIRROR_TOL && best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, j));
            }
        }
        match best {
            Some((_, j)) => {
                used[i] = true; used[j] = true;
                let (l, r) = if off < 0.0 { (i, j) } else { (j, i) };
                out.push(Limb::Pair { left: l, right: r, verts: a.verts * 2 });
            }
            None => {
                used[i] = true;
                // No twin. On the centreline it is spine; off it, an
                // asymmetric detail.
                out.push(if off.abs() / scale <= CENTRE_TOL {
                    Limb::Spine { part: i, verts: a.verts }
                } else {
                    Limb::Odd { part: i, verts: a.verts }
                });
            }
        }
    }
    out
}

/// A readable summary — what the plan found, in the terms a person checking it
/// by hand would use.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub pairs: usize,
    pub spine: usize,
    pub odd: usize,
    /// Fraction of vertices explained by mirrored pairs. High means the model
    /// has a clear body plan; low means it is a pile of asymmetric detail and
    /// this approach will not help.
    pub paired_fraction: f64,
}

pub fn summarise(limbs: &[Limb]) -> Plan {
    let total: usize = limbs.iter().map(|l| l.verts()).sum();
    let paired: usize = limbs.iter()
        .filter(|l| matches!(l, Limb::Pair { .. })).map(|l| l.verts()).sum();
    Plan {
        pairs: limbs.iter().filter(|l| matches!(l, Limb::Pair { .. })).count(),
        spine: limbs.iter().filter(|l| matches!(l, Limb::Spine { .. })).count(),
        odd: limbs.iter().filter(|l| matches!(l, Limb::Odd { .. })).count(),
        paired_fraction: if total == 0 { 0.0 } else { paired as f64 / total as f64 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(i: usize, verts: usize, c: [f64; 3], e: [f64; 3]) -> Part {
        Part { index: i, verts, centre: c, extent: e }
    }

    /// Brawl's tank, transcribed from the real component table — the same
    /// numbers a person reads off the model.
    fn brawl_tank() -> (Vec<Part>, [f64; 3], Axes) {
        let parts = vec![
            part(0, 272, [ 0.00,-0.11, 0.15], [0.91,1.52,0.25]),   // hull, centre
            part(1, 145, [ 0.30, 0.68, 0.20], [0.24,0.07,0.07]),   // R
            part(2, 145, [-0.30, 0.68, 0.20], [0.24,0.07,0.07]),   // L  <- pair
            part(3, 127, [ 0.00,-0.25,-0.02], [0.79,0.99,0.16]),   // centre
            part(5, 113, [ 0.15, 0.25,-0.15], [0.03,0.06,0.03]),
            part(6, 113, [-0.15, 0.25,-0.15], [0.03,0.06,0.03]),   // pair
            part(8, 105, [-0.32,-0.12, 0.22], [0.11,1.41,0.22]),   // TREAD L
            part(9, 105, [ 0.32,-0.12, 0.22], [0.11,1.41,0.22]),   // TREAD R
            part(11, 68, [ 0.12,-0.94, 0.07], [0.16,0.12,0.12]),
            part(12, 68, [-0.12,-0.94, 0.07], [0.16,0.12,0.12]),   // pair
        ];
        (parts, [0.91, 2.00, 0.66], Axes::of([0.91, 2.00, 0.66]))
    }

    #[test]
    fn axes_are_measured_because_the_two_forms_disagree() {
        // Brawl's tank is longest in Y; his robot is longest in Z. Assuming a
        // convention is what broke the fold approach.
        let tank = Axes::of([0.91, 2.00, 0.66]);
        assert_eq!(tank.long, 1, "tank: Y is length");
        assert_eq!(tank.lateral, 0, "X is width — the mirror axis");
        assert_eq!(tank.short, 2);

        let robot = Axes::of([1.77, 0.76, 2.00]);
        assert_eq!(robot.long, 2, "robot: Z is height");
        assert_eq!(robot.lateral, 0);
        assert_eq!(robot.short, 1, "and Y is depth");
    }

    #[test]
    fn the_treads_are_found_as_a_mirrored_pair() {
        // The thing a person sees instantly in the table: two 105-vert parts,
        // 1.41 long, at ±0.32. That is the tread assembly, and it is a limb.
        let (parts, span, axes) = brawl_tank();
        let plan = body_plan(&parts, span, axes);
        let tread = plan.iter().find(|l| matches!(l, Limb::Pair { verts: 210, .. }))
            .expect("the treads must pair");
        match tread {
            Limb::Pair { left, right, .. } => {
                assert!(parts[*left].centre[0] < 0.0, "left is at -X");
                assert!(parts[*right].centre[0] > 0.0, "right is at +X");
                assert!((parts[*left].extent[1] - 1.41).abs() < 1e-9, "and it is the long one");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_whole_tank_reads_as_four_pairs_and_two_spine_parts() {
        // No thresholds tuned, no descriptor: this is what the geometry says.
        let (parts, span, axes) = brawl_tank();
        let s = summarise(&body_plan(&parts, span, axes));
        assert_eq!(s.pairs, 4, "four mirrored pairs");
        assert_eq!(s.spine, 2, "the hull and the belly plate");
        assert_eq!(s.odd, 0, "nothing unexplained");
        assert!(s.paired_fraction > 0.4, "most of the model is paired: {}", s.paired_fraction);
    }

    #[test]
    fn a_part_on_the_centreline_is_spine_not_an_unpaired_limb() {
        let (parts, span, axes) = brawl_tank();
        let plan = body_plan(&parts, span, axes);
        // the 272-vert hull sits at X=0.00
        assert!(plan.iter().any(|l| matches!(l, Limb::Spine { verts: 272, .. })));
        assert!(!plan.iter().any(|l| matches!(l, Limb::Odd { verts: 272, .. })));
    }

    #[test]
    fn a_near_miss_does_not_pair() {
        // The tolerance is tight on purpose: a loose one is how the descriptor
        // work ended up matching unrelated tanks at 87%.
        let parts = vec![
            part(0, 100, [ 0.30, 0.0, 0.0], [0.1, 0.1, 0.1]),
            // same size, but 40% of the model away in Y — not a mirror
            part(1, 100, [-0.30, 0.8, 0.0], [0.1, 0.1, 0.1]),
        ];
        let span = [1.0, 2.0, 1.0];
        let plan = body_plan(&parts, span, Axes::of(span));
        assert_eq!(summarise(&plan).pairs, 0, "different positions are not mirrors");
        assert_eq!(summarise(&plan).odd, 2);
    }

    #[test]
    fn different_vertex_counts_never_pair() {
        // A true mirror is the SAME mesh reflected, so the counts match exactly.
        // Allowing a tolerance here is how a wheel pairs with a headlight.
        let parts = vec![
            part(0, 100, [ 0.30, 0.0, 0.0], [0.1, 0.1, 0.1]),
            part(1,  99, [-0.30, 0.0, 0.0], [0.1, 0.1, 0.1]),
        ];
        let span = [1.0, 2.0, 1.0];
        assert_eq!(summarise(&body_plan(&parts, span, Axes::of(span))).pairs, 0);
    }

    #[test]
    fn an_asymmetric_model_reports_a_low_paired_fraction_rather_than_guessing() {
        // The honest signal that this approach will not help: a pile of
        // asymmetric detail has no body plan to read.
        let parts: Vec<Part> = (0..8).map(|i|
            part(i, 50 + i * 7, [0.1 * i as f64 + 0.2, i as f64 * 0.1, 0.0], [0.1, 0.1, 0.1])
        ).collect();
        let span = [1.0, 2.0, 1.0];
        let s = summarise(&body_plan(&parts, span, Axes::of(span)));
        assert!(s.paired_fraction < 0.2, "should find almost nothing: {s:?}");
        assert!(s.odd >= 6);
    }
}
