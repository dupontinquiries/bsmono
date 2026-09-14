//! segment — giving rav1e's segmentation something to think with.
//!
//! rav1e has segmentation plumbing. `get_initial_segmentation` reads
//! the previous frame's map and hands it forward:
//!
//! ```text
//! let segmentation = if fi.primary_ref_frame == PRIMARY_REF_NONE {
//!   None
//! } else {
//!   ref_frame.map(|rec| rec.segmentation)
//! };
//! segmentation.unwrap_or_default()
//! ```
//!
//! Inherit, or start empty. Nothing ever ANALYSES a frame and decides
//! what the segments should be, so the feature is inert the same way
//! global motion was: the syntax works and nothing drives it.
//!
//! ## What segmentation is for
//!
//! AV1 carries up to eight segments per frame, each with its own
//! quantiser delta, loop-filter delta and reference override. That is
//! per-region rate control: the face can have bits and the
//! out-of-focus background can go without, in the same frame, at no
//! signalling cost beyond a small map.
//!
//! Uniform quantisation spends the same on every part of a picture
//! regardless of whether anyone is looking at it. On the footage this
//! was written for -- shallow depth of field, a subject against a soft
//! background -- that is most of the frame wasted.
//!
//! ## It costs no new analysis
//!
//! `local_field` already computes a per-block motion vector and how
//! well it matched, and `fit_affine` already produces the global model.
//! A block whose motion DISAGREES with the global model is moving
//! independently -- that is the subject. A block that agrees is the
//! background the camera is panning across.
//!
//! That is the same measurement feeding a third use, after the
//! encoder's global motion parameters and the timeline bucketing.

use super::{LocalMv, Warp};

/// AV1 allows eight. Using all eight costs map bits for distinctions
/// nobody sees, so this uses four and says why.
pub const MAX_SEGMENTS: usize = 8;

/// What a region of the frame is doing, which decides what it deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Region {
    /// Moving against the camera: the subject. Bits go here.
    Subject,
    /// Follows the global model, and is sharp.
    Background,
    /// Follows the global model and matched poorly even so -- soft,
    /// out of focus, or featureless. The cheapest place in the frame.
    Soft,
    /// Matched so badly that no motion explains it. Occlusion, or a
    /// cut. Treated as subject, because guessing wrong here shows.
    Uncertain,
}

impl Region {
    /// Quantiser delta, relative to the frame's base.
    ///
    /// NEGATIVE MEANS BETTER QUALITY in AV1's convention: a lower
    /// quantiser index is finer. These are the judgement and are
    /// deliberately modest -- a large delta makes the segment boundary
    /// visible, which is worse than the uniform quantisation it
    /// replaced.
    pub fn qp_delta(self) -> i16 {
        match self {
            Region::Subject => -8,
            Region::Background => 0,
            // out of focus already: the lens threw the detail away and
            // the encoder should not pay to reconstruct blur
            Region::Soft => 10,
            Region::Uncertain => -4,
        }
    }

    /// Loop filter delta. A softened region can take more filtering;
    /// the subject should not be smeared.
    pub fn lf_delta(self) -> i8 {
        match self {
            Region::Subject => -2,
            Region::Background => 0,
            Region::Soft => 3,
            Region::Uncertain => 0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Region::Subject => "subject", Region::Background => "background",
            Region::Soft => "soft", Region::Uncertain => "uncertain",
        }
    }
}

/// Thresholds, gathered so they can be tuned per project rather than
/// buried in the classifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// Pixels of disagreement with the global model before a block
    /// counts as moving independently.
    pub disagree_px: f64,
    /// Match error above which a block is soft or featureless.
    pub soft_sad: f64,
    /// Match error above which nothing explains it.
    pub hopeless_sad: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds { disagree_px: 2.5, soft_sad: 6.0, hopeless_sad: 22.0 }
    }
}

/// Classify one block against the frame's global model.
pub fn region_of(m: &LocalMv, global: &Warp, b: &Bounds) -> Region {
    // NOTHING EXPLAINS IT is checked first: a block that matched
    // terribly has a meaningless motion vector, and measuring its
    // disagreement with the global model would be measuring noise.
    if m.sad >= b.hopeless_sad { return Region::Uncertain; }
    let (gx, gy) = global.apply(m.x, m.y);
    let disagree = ((gx - (m.x + m.dx)).powi(2) + (gy - (m.y + m.dy)).powi(2)).sqrt();
    if disagree >= b.disagree_px { return Region::Subject; }
    // agrees with the camera, but matched poorly: soft or featureless
    if m.sad >= b.soft_sad { return Region::Soft; }
    Region::Background
}

/// A frame's segment map.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentMap {
    /// Segment id per block, row-major over the analysis grid.
    pub ids: Vec<u8>,
    pub cols: usize,
    pub rows: usize,
    /// What each id means, indexed by id.
    pub regions: Vec<Region>,
}

impl SegmentMap {
    pub fn at(&self, col: usize, row: usize) -> Option<Region> {
        if col >= self.cols || row >= self.rows { return None; }
        self.regions.get(self.ids[row * self.cols + col] as usize).copied()
    }

    /// The deltas rav1e's `SegmentationState` needs.
    pub fn deltas(&self) -> Vec<(u8, i16, i8)> {
        self.regions.iter().enumerate()
            .map(|(i, r)| (i as u8, r.qp_delta(), r.lf_delta())).collect()
    }

    /// How much of the frame is in each region.
    pub fn share(&self, r: Region) -> f64 {
        if self.ids.is_empty() { return 0.0; }
        let id = self.regions.iter().position(|x| *x == r);
        match id {
            None => 0.0,
            Some(i) => self.ids.iter().filter(|x| **x == i as u8).count() as f64
                / self.ids.len() as f64,
        }
    }

    /// IS THIS MAP WORTH SENDING?
    ///
    /// A map where everything is one region costs the signalling and
    /// changes nothing. And a map that is mostly subject is not a
    /// segmentation, it is a frame with a lower quantiser -- which
    /// should be done by lowering the quantiser, not by pretending the
    /// whole frame is a subject.
    pub fn worth_sending(&self) -> bool {
        let distinct = {
            let mut v: Vec<u8> = self.ids.clone();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        distinct >= 2 && self.share(Region::Subject) < 0.75
    }
}

/// Build a segment map from a motion field and the frame's global
/// model.
pub fn build(field: &[LocalMv], global: &Warp, cols: usize, rows: usize, b: &Bounds)
    -> SegmentMap {
    // FOUR REGIONS, NOT EIGHT. AV1 allows eight, and using them all
    // costs map bits for distinctions nobody sees. Four is what the
    // motion field can actually justify: moving, still, soft, unknown.
    let regions = vec![Region::Subject, Region::Background, Region::Soft, Region::Uncertain];
    let index = |r: Region| regions.iter().position(|x| *x == r).unwrap() as u8;
    let mut ids = vec![index(Region::Background); cols * rows];
    for (i, m) in field.iter().enumerate() {
        if i >= ids.len() { break; }
        ids[i] = index(region_of(m, global, b));
    }
    SegmentMap { ids, cols, rows, regions }
}

/// Smooth a map so single blocks do not flicker between segments.
///
/// An isolated block of a different segment costs map bits and shows
/// as a quantisation patch. Majority filtering removes them; the
/// subject survives because a subject is never one block.
pub fn smooth(map: &SegmentMap) -> SegmentMap {
    let mut out = map.clone();
    for row in 0..map.rows {
        for col in 0..map.cols {
            let mut count = [0usize; MAX_SEGMENTS];
            let mut n = 0;
            for dr in -1i32..=1 {
                for dc in -1i32..=1 {
                    let (r, c) = (row as i32 + dr, col as i32 + dc);
                    if r < 0 || c < 0 || r >= map.rows as i32 || c >= map.cols as i32 { continue; }
                    let id = map.ids[r as usize * map.cols + c as usize] as usize;
                    if id < MAX_SEGMENTS { count[id] += 1; n += 1; }
                }
            }
            if n == 0 { continue; }
            let (best, votes) = count.iter().enumerate()
                .max_by_key(|(_, v)| **v).map(|(i, v)| (i as u8, *v)).unwrap();
            // only override when the neighbourhood clearly disagrees,
            // so a genuine two-block detail is not erased
            if votes * 2 > n { out.ids[row * map.cols + col] = best; }
        }
    }
    out
}

/// What this map is expected to buy, so it can be argued with.
pub fn describe(map: &SegmentMap) -> String {
    let mut s = String::new();
    if !map.worth_sending() {
        s.push_str("not worth signalling: ");
        if map.share(Region::Subject) >= 0.75 {
            s.push_str("most of the frame is subject -- lower the frame quantiser instead\n");
        } else {
            s.push_str("only one region present\n");
        }
        return s;
    }
    for r in [Region::Subject, Region::Background, Region::Soft, Region::Uncertain] {
        let pct = map.share(r) * 100.0;
        if pct < 0.5 { continue; }
        s.push_str(&format!("  {:<11} {:>5.1}%  qp {:+}  lf {:+}\n",
                            r.name(), pct, r.qp_delta(), r.lf_delta()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(x: f64, y: f64, dx: f64, dy: f64, sad: f64) -> LocalMv {
        LocalMv { x, y, dx, dy, sad }
    }

    #[test]
    fn a_block_moving_against_the_camera_is_the_subject() {
        let b = Bounds::default();
        // the camera pans +6; this block moves -10, so it disagrees by
        // 16 pixels -- it is moving on its own
        let global = Warp::translation(6.0, 0.0);
        let subject = mv(100.0, 50.0, -10.0, 0.0, 3.0);
        assert_eq!(region_of(&subject, &global, &b), Region::Subject);
        // a block that follows the camera is background
        let bg = mv(100.0, 50.0, 6.0, 0.0, 2.0);
        assert_eq!(region_of(&bg, &global, &b), Region::Background);
    }

    #[test]
    fn a_soft_region_is_the_cheapest_place_in_the_frame() {
        // the lens already threw the detail away; the encoder should
        // not pay to reconstruct blur
        let b = Bounds::default();
        let global = Warp::translation(4.0, 0.0);
        let soft = mv(10.0, 10.0, 4.0, 0.0, 12.0);
        assert_eq!(region_of(&soft, &global, &b), Region::Soft);
        assert!(Region::Soft.qp_delta() > 0, "soft gives bits back");
        assert!(Region::Subject.qp_delta() < 0, "the subject takes them");
        // and the deltas are modest: a large one makes the boundary
        // visible, which is worse than the uniform quantisation it
        // replaced
        for r in [Region::Subject, Region::Background, Region::Soft, Region::Uncertain] {
            assert!(r.qp_delta().abs() <= 12, "{} delta too large", r.name());
        }
    }

    #[test]
    fn a_block_nothing_explains_is_checked_before_its_motion_is_trusted() {
        // a block that matched terribly has a MEANINGLESS motion
        // vector, and measuring its disagreement with the global model
        // would be measuring noise
        let b = Bounds::default();
        let global = Warp::translation(5.0, 0.0);
        // agrees with the camera perfectly, but matched hopelessly
        let occluded = mv(20.0, 20.0, 5.0, 0.0, 40.0);
        assert_eq!(region_of(&occluded, &global, &b), Region::Uncertain);
        // and uncertain gets bits, because guessing wrong there shows
        assert!(Region::Uncertain.qp_delta() < 0);
    }

    #[test]
    fn a_map_with_one_region_is_not_worth_signalling() {
        // it costs the signalling and changes nothing
        let global = Warp::translation(3.0, 0.0);
        let field: Vec<LocalMv> = (0..36)
            .map(|i| mv((i % 6) as f64 * 32.0, (i / 6) as f64 * 32.0, 3.0, 0.0, 2.0))
            .collect();
        let m = build(&field, &global, 6, 6, &Bounds::default());
        assert!(!m.worth_sending(), "{}", describe(&m));
        assert!(describe(&m).contains("only one region"), "{}", describe(&m));
    }

    #[test]
    fn a_frame_that_is_mostly_subject_should_lower_its_quantiser_instead() {
        // that is not a segmentation, it is a frame with a lower
        // quantiser wearing a costume
        let global = Warp::translation(3.0, 0.0);
        let field: Vec<LocalMv> = (0..36)
            .map(|i| mv((i % 6) as f64 * 32.0, (i / 6) as f64 * 32.0, -20.0, 0.0, 2.0))
            .collect();
        let m = build(&field, &global, 6, 6, &Bounds::default());
        assert!(m.share(Region::Subject) > 0.9);
        assert!(!m.worth_sending());
        assert!(describe(&m).contains("lower the frame quantiser instead"), "{}", describe(&m));
    }

    #[test]
    fn a_real_mix_produces_a_map_worth_sending() {
        let global = Warp::translation(5.0, 0.0);
        // a subject in the middle third, soft at the edges
        let field: Vec<LocalMv> = (0..64).map(|i| {
            let (c, r) = (i % 8, i / 8);
            let (x, y) = (c as f64 * 40.0, r as f64 * 40.0);
            if (2..6).contains(&c) && (2..6).contains(&r) { mv(x, y, -8.0, 2.0, 3.0) }
            else if c == 0 || c == 7 { mv(x, y, 5.0, 0.0, 12.0) }
            else { mv(x, y, 5.0, 0.0, 2.0) }
        }).collect();
        let m = build(&field, &global, 8, 8, &Bounds::default());
        assert!(m.worth_sending(), "{}", describe(&m));
        assert!(m.share(Region::Subject) > 0.15 && m.share(Region::Subject) < 0.35,
                "{:.2}", m.share(Region::Subject));
        assert!(m.share(Region::Soft) > 0.1);
        let d = describe(&m);
        assert!(d.contains("subject") && d.contains("soft"), "{d}");
        // the deltas are what rav1e's segmentation state wants
        let deltas = m.deltas();
        assert_eq!(deltas.len(), 4);
        assert!(deltas.iter().any(|(_, q, _)| *q < 0), "something gets bits");
        assert!(deltas.iter().any(|(_, q, _)| *q > 0), "something gives them back");
    }

    #[test]
    fn an_isolated_block_is_smoothed_away_but_a_real_detail_is_not() {
        // a single block of a different segment costs map bits and
        // shows as a quantisation patch
        let global = Warp::translation(4.0, 0.0);
        let mut field: Vec<LocalMv> = (0..49)
            .map(|i| mv((i % 7) as f64 * 40.0, (i / 7) as f64 * 40.0, 4.0, 0.0, 2.0))
            .collect();
        field[24] = mv(160.0, 120.0, -12.0, 0.0, 3.0);   // one lonely block
        let m = build(&field, &global, 7, 7, &Bounds::default());
        assert_eq!(m.at(3, 3), Some(Region::Subject));
        let s = smooth(&m);
        assert_eq!(s.at(3, 3), Some(Region::Background), "the speck is gone");
        // a 3x3 subject survives, because a subject is never one block
        let mut f2 = field.clone();
        for r in 2..5 { for c in 2..5 {
            f2[r * 7 + c] = mv(c as f64 * 40.0, r as f64 * 40.0, -12.0, 0.0, 3.0);
        }}
        let m2 = smooth(&build(&f2, &global, 7, 7, &Bounds::default()));
        assert_eq!(m2.at(3, 3), Some(Region::Subject), "the real subject survives");
    }

    #[test]
    fn the_map_reuses_the_measurement_the_encoder_already_made() {
        // local_field and fit_affine already ran for global motion;
        // this is the same data serving a third purpose
        let global = Warp::translation(6.0, -2.0);
        let field = vec![
            mv(0.0, 0.0, 6.0, -2.0, 1.0),
            mv(40.0, 0.0, -9.0, 4.0, 2.0),
        ];
        let m = build(&field, &global, 2, 1, &Bounds::default());
        assert_eq!(m.at(0, 0), Some(Region::Background));
        assert_eq!(m.at(1, 0), Some(Region::Subject));
        assert!(m.at(9, 9).is_none(), "out of range is None, not a panic");
    }
}
