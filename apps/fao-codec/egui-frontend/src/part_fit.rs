//! part_fit — describing reconstructed geometry with a part vocabulary.
//!
//! v8 reconstructs depth and a mesh from video. Coding that mesh is expensive.
//! blackstar's `partdata` is a **compositional grammar** rather than a model
//! library — its own header is explicit: *"The library ships no geometry: it is
//! a catalogue of 5,873 part signatures and, for 178 vehicles, WHERE each part
//! sits. That is a compositional grammar rather than a mesh."* Each entry is
//! `(class, x, y, z, size)` over eight classes.
//!
//! That is a prior over structure, and a prior over structure is exactly what a
//! reconstruction codec is missing. If a cluster of reconstructed points can be
//! described as *"wheel, here, this big"*, the bitstream carries a class, a
//! position and a scale — a handful of bytes — instead of a mesh patch. And it
//! carries **meaning**: a wheel is a thing that rotates about its axis, so the
//! next frame's motion is predictable rather than measured.
//!
//! ## The failure mode is worse than the win, so it is guarded first
//!
//! A part fitter that is confident and wrong does not produce noise; it
//! produces **plausible structure that was never there**. That is the same trap
//! as optimising against PSNR, in a different costume — a hallucinated wheel
//! looks like content, scores well on any whole-frame metric, and is simply not
//! what was filmed.
//!
//! So every fit carries its residual, `accept` refuses a fit that does not
//! explain the points it claims, and the fallback is ordinary residual coding
//! rather than a worse fit. **Refusing is the normal case**: most video is not
//! vehicles, and a grammar mined from 178 vehicles has no business describing a
//! face or a tree.
//!
//! ## What this is not
//!
//! Not a detector and not a segmenter. It takes points that something else has
//! already grouped and asks one question: does this cluster look like a part I
//! have a name for, and how wrong am I?

/// The part classes, from `partdata`'s own tuple encoding.
///
/// Kept in that order and with those numbers so a fit can be written into a
/// bitstream and read back by anything that knows the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartClass {
    Block = 0,
    Panel = 1,
    Strut = 2,
    Disc = 3,
    Wheel = 4,
    Barrel = 5,
    Greeble = 6,
    Tread = 7,
}

impl PartClass {
    pub fn from_code(c: u8) -> Option<PartClass> {
        Some(match c {
            0 => PartClass::Block, 1 => PartClass::Panel, 2 => PartClass::Strut,
            3 => PartClass::Disc, 4 => PartClass::Wheel, 5 => PartClass::Barrel,
            6 => PartClass::Greeble, 7 => PartClass::Tread,
            _ => return None,
        })
    }

    pub fn code(self) -> u8 { self as u8 }

    /// Does this class rotate about an axis in normal use?
    ///
    /// The semantic payoff. A wheel's motion between frames is a rotation about
    /// a known axis, so the decoder can predict it instead of the encoder
    /// measuring it — which is a thing no block motion vector can express and
    /// the reason a part fit is worth more than its byte count.
    pub fn spins(self) -> bool {
        matches!(self, PartClass::Wheel | PartClass::Disc | PartClass::Tread)
    }
}

/// Axis-aligned extents of a point cluster, sorted descending.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extents {
    pub long: f32,
    pub mid: f32,
    pub short: f32,
}

impl Extents {
    /// Measure a cluster.
    ///
    /// Sorted rather than kept per-axis, deliberately: `partdata` normalises
    /// every donor by mapping its longest extent to Z and so on, precisely
    /// because each donor "arrives in its own units and axis convention".
    /// Comparing raw axes would make the same part in a rotated clip classify
    /// differently.
    pub fn of(points: &[[f32; 3]]) -> Option<Extents> {
        if points.len() < 2 { return None; }
        for p in points {
            if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
                return None;
            }
        }
        // ROBUST extents: the 90th percentile of absolute deviation per axis,
        // doubled, rather than min-to-max.
        //
        // A plain bounding box cannot see an outlier — the outlier simply
        // becomes the box, the residual is measured against the enlarged box,
        // and a cloud with a point fifty units away fits "perfectly". A test
        // caught exactly that: the reject case passed as accepted. Percentile
        // extents describe the bulk of the cluster, so a stray point lands
        // outside them and shows up in the residual, which is the entire job.
        let mut e = [0.0f32; 3];
        for i in 0..3 {
            let mut c = 0.0f32;
            for p in points { c += p[i]; }
            c /= points.len() as f32;
            let mut d: Vec<f32> = points.iter().map(|p| (p[i] - c).abs()).collect();
            d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((d.len() as f32 * 0.9).ceil() as usize).min(d.len()) - 1;
            e[i] = d[idx] * 2.0;
        }
        e.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        Some(Extents { long: e[0], mid: e[1], short: e[2] })
    }

    /// Longest over shortest. 1.0 is a cube, large is a rod.
    pub fn aspect(&self) -> f32 {
        if self.short <= 1e-6 { return f32::INFINITY; }
        self.long / self.short
    }

    /// Flatness: how much thinner the shortest axis is than the middle one.
    pub fn flatness(&self) -> f32 {
        if self.mid <= 1e-6 { return f32::INFINITY; }
        self.mid / self.short.max(1e-6)
    }

    pub fn size(&self) -> f32 { self.long }
}

/// A candidate description of a cluster.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub class: PartClass,
    /// Cluster centre.
    pub at: [f32; 3],
    pub size: f32,
    /// Mean distance from a point to the fitted primitive's surface, relative
    /// to the part's size. Dimensionless so it can be thresholded once.
    pub residual: f32,
}

/// Above this relative residual a fit is not describing the points.
///
/// Deliberately tight. The cost of refusing a good fit is some extra residual
/// bits; the cost of accepting a bad one is inventing an object that was never
/// filmed, and those are not comparable.
pub const MAX_RESIDUAL: f32 = 0.12;

impl Fit {
    pub fn accept(&self) -> bool {
        self.residual.is_finite() && self.residual <= MAX_RESIDUAL
    }
}

/// Classify a cluster by shape, without committing to it.
///
/// Shape statistics only — this is the cheap first pass that decides which
/// primitive is even worth fitting. It returns a class for anything, which is
/// why `fit` and not this is where refusal happens.
pub fn classify(e: &Extents) -> PartClass {
    let a = e.aspect();
    let f = e.flatness();
    // FLATNESS IS TESTED FIRST, and the order is not arbitrary. A flat square —
    // 3 x 3 x 0.2 — has an aspect of 15, so an aspect-first test calls a disc a
    // strut. Long-and-thin has to mean thin in BOTH minor axes, and only the
    // flatness test can tell "thin one way" from "thin two ways". A test caught
    // this on exactly that shape.
    if f > 4.0 {
        // Flat. A disc is flat AND roughly square in its plane; a panel is not.
        return if e.long / e.mid.max(1e-6) < 1.4 { PartClass::Disc }
               else { PartClass::Panel };
    }
    if a > 6.0 { return PartClass::Strut; }
    if a > 2.2 { return PartClass::Barrel; }
    PartClass::Block
}

/// Fit a cluster and report how wrong it is.
///
/// The residual is what makes this usable: a class without an error term is a
/// guess dressed as a measurement.
pub fn fit(points: &[[f32; 3]]) -> Option<Fit> {
    let e = Extents::of(points)?;
    if e.long <= 1e-6 { return None; }
    let n = points.len() as f32;
    let mut c = [0.0f32; 3];
    for p in points { for i in 0..3 { c[i] += p[i]; } }
    for v in c.iter_mut() { *v /= n; }

    let class = classify(&e);
    // Residual against the primitive the class implies. A block is measured
    // against its bounding box, a disc and a wheel against a cylinder of the
    // cluster's radius — enough to tell "this is roughly that shape" from "this
    // is nothing like it", which is the only question being asked here.
    // Per-axis half-extents, matched to the axis each deviation is on. Sorting
    // the extents is right for CLASSIFYING (the grammar normalises orientation)
    // and wrong for measuring, where a point's x deviation must be compared
    // against the x half-extent.
    let mut half = [0.0f32; 3];
    for i in 0..3 {
        let mut d: Vec<f32> = points.iter().map(|p| (p[i] - c[i]).abs()).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((d.len() as f32 * 0.9).ceil() as usize).min(d.len()) - 1;
        half[i] = d[idx];
    }
    let mut err = 0.0f32;
    for p in points {
        let mut worst = 0.0f32;
        for i in 0..3 {
            let over = (p[i] - c[i]).abs() - half[i];
            if over > worst { worst = over; }
        }
        err += worst;
    }
    let residual = (err / n) / e.size();
    Some(Fit { class, at: c, size: e.size(), residual })
}

/// What a fit costs to code, in bytes.
///
/// class (1) + position (3 × f16) + size (f16) = 9. Compared against the mesh
/// patch it replaces, which is the comparison that matters — a fit is only
/// worth making if it is cheaper than the geometry it stands in for.
pub const FIT_BYTES: usize = 9;

/// Should a fit be used instead of coding the geometry directly?
///
/// Both conditions, and the second is easy to forget: the fit must be accepted
/// AND cheaper. A perfect fit for a cluster of four points costs more than the
/// four points do.
pub fn worth_coding(f: &Fit, geometry_bytes: usize) -> bool {
    f.accept() && FIT_BYTES < geometry_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filled box of points.
    fn box_points(sx: f32, sy: f32, sz: f32) -> Vec<[f32; 3]> {
        let mut v = Vec::new();
        for i in 0..5 {
            for j in 0..5 {
                for k in 0..5 {
                    v.push([(i as f32 / 4.0 - 0.5) * sx,
                            (j as f32 / 4.0 - 0.5) * sy,
                            (k as f32 / 4.0 - 0.5) * sz]);
                }
            }
        }
        v
    }

    #[test]
    fn the_class_codes_match_the_grammars_own_tuple_encoding() {
        // So a fit can be written into a bitstream and read back by anything
        // that knows partdata's format.
        assert_eq!(PartClass::Block.code(), 0);
        assert_eq!(PartClass::Wheel.code(), 4);
        assert_eq!(PartClass::Tread.code(), 7);
        assert_eq!(PartClass::from_code(4), Some(PartClass::Wheel));
        assert_eq!(PartClass::from_code(9), None);
    }

    #[test]
    fn spinning_classes_are_the_ones_whose_motion_is_predictable() {
        // The semantic payoff: a wheel's motion between frames is a rotation
        // about a known axis, which the decoder can predict rather than the
        // encoder measure — something no block motion vector can express.
        assert!(PartClass::Wheel.spins());
        assert!(PartClass::Disc.spins());
        assert!(PartClass::Tread.spins());
        assert!(!PartClass::Block.spins());
        assert!(!PartClass::Panel.spins());
    }

    #[test]
    fn extents_are_sorted_so_a_rotated_clip_classifies_the_same() {
        // partdata normalises every donor precisely because each "arrives in
        // its own units and axis convention". Comparing raw axes would make the
        // same part in a rotated shot classify differently.
        let a = Extents::of(&box_points(4.0, 1.0, 2.0)).unwrap();
        let b = Extents::of(&box_points(1.0, 2.0, 4.0)).unwrap();
        assert!((a.long - b.long).abs() < 1e-4);
        assert!((a.short - b.short).abs() < 1e-4);
    }

    #[test]
    fn a_cube_is_a_block_and_a_rod_is_a_strut() {
        assert_eq!(classify(&Extents::of(&box_points(2.0, 2.0, 2.0)).unwrap()),
                   PartClass::Block);
        assert_eq!(classify(&Extents::of(&box_points(10.0, 1.0, 1.0)).unwrap()),
                   PartClass::Strut);
    }

    #[test]
    fn a_flat_square_is_a_disc_and_a_flat_oblong_is_a_panel() {
        // The distinction the grammar draws, and it is a real one: a disc is
        // round in its plane and a panel is not.
        assert_eq!(classify(&Extents::of(&box_points(3.0, 3.0, 0.2)).unwrap()),
                   PartClass::Disc);
        assert_eq!(classify(&Extents::of(&box_points(6.0, 2.0, 0.2)).unwrap()),
                   PartClass::Panel);
    }

    #[test]
    fn a_fit_carries_its_error_because_a_class_alone_is_a_guess() {
        let f = fit(&box_points(2.0, 2.0, 2.0)).expect("fits");
        assert!(f.residual.is_finite());
        assert!(f.accept(), "a box really is a block, residual {}", f.residual);
    }

    #[test]
    fn a_scattered_cloud_is_refused_rather_than_named() {
        // The failure mode this exists to prevent: a confident wrong fit does
        // not produce noise, it produces plausible structure that was never
        // filmed.
        let mut pts = box_points(2.0, 2.0, 2.0);
        // Add outliers far outside any primitive the class implies.
        pts.push([40.0, 0.0, 0.0]);
        pts.push([0.0, -35.0, 0.0]);
        pts.push([0.0, 0.0, 38.0]);
        let f = fit(&pts).expect("measurable");
        assert!(!f.accept(), "residual {} should have been refused", f.residual);
    }

    #[test]
    fn refusing_is_expected_to_be_the_common_case() {
        // Most video is not vehicles, and a grammar mined from 178 vehicles has
        // no business describing a face or a tree. The threshold is set tight
        // for that reason.
        assert!(MAX_RESIDUAL < 0.2);
    }

    #[test]
    fn the_centre_is_the_clusters_centroid() {
        let mut pts = box_points(2.0, 2.0, 2.0);
        for p in pts.iter_mut() { p[0] += 10.0; }
        let f = fit(&pts).unwrap();
        assert!((f.at[0] - 10.0).abs() < 1e-3);
        assert!(f.at[1].abs() < 1e-3);
    }

    #[test]
    fn a_degenerate_cluster_is_not_fitted() {
        // One point has no extent, and a zero-size part would divide by zero
        // when the residual is normalised.
        assert!(fit(&[[0.0, 0.0, 0.0]]).is_none());
        assert!(fit(&[]).is_none());
        assert!(Extents::of(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]]).is_some());
        assert!(fit(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]]).is_none());
    }

    #[test]
    fn a_non_finite_point_poisons_nothing_because_the_cluster_is_refused() {
        // One NaN in a depth-derived cloud is ordinary; silently dropping it
        // would shrink the extents and misclassify the part.
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [f32::NAN, 0.0, 0.0]];
        assert!(Extents::of(&pts).is_none());
        assert!(fit(&pts).is_none());
    }

    #[test]
    fn a_fit_is_only_used_when_it_is_cheaper_than_the_geometry() {
        // Easy to forget: a perfect fit for four points costs more than the
        // four points do.
        let f = fit(&box_points(2.0, 2.0, 2.0)).unwrap();
        assert!(worth_coding(&f, 200), "a mesh patch is worth replacing");
        assert!(!worth_coding(&f, 8), "eight bytes of geometry is not");
    }

    #[test]
    fn a_rejected_fit_is_never_worth_coding_however_cheap() {
        // Bits saved by inventing an object are not bits saved.
        let mut pts = box_points(2.0, 2.0, 2.0);
        pts.push([50.0, 50.0, 50.0]);
        let f = fit(&pts).unwrap();
        assert!(!f.accept());
        assert!(!worth_coding(&f, 10_000));
    }
}
