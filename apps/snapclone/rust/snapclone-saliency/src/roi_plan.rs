//! roi_plan.rs — turn a raw saliency rectangle into a video-encoder
//! BIT-ALLOCATION PLAN for face-aware compression.
//!
//! # The gain we're improving
//!
//! The original face-aware path applied a single `addroi=...:-1` filter: it
//! only BOOSTED quality inside the face box (lower QP) and did NOTHING to the
//! background. Under bitrate rate-control that "steals" a few bits from the
//! background, but the net file barely shrinks — and `qoffset=-1` (the maximum
//! boost) over-spends on the face. The design doc PROMISED "compresses the
//! background harder"; the code never did it.
//!
//! This planner makes the bit reallocation explicit and tunable:
//!   * a BACKGROUND penalty (positive QP offset over the WHOLE frame) that
//!     compresses everything harder, applied FIRST so it's the baseline,
//!   * a FACE boost (negative QP offset) over a PADDED box that overrides the
//!     background penalty, so the face is PROTECTED from the global degradation
//!     rather than merely sharpened beyond the background.
//!
//! Net effect: with the global penalty doing the saving and the face boost
//! protecting the subject, the encoder spends its bits where a human looks
//! (the face) and starves the parts they don't — a real bandwidth win at equal
//! perceived quality, which is the whole point of face-aware compression.
//!
//! Padding matters: a box tight to the saliency rect clips ears/chin/hair and
//! puts the QP discontinuity right at the face edge (visible blocking). We pad
//! outward by a fraction of the box size so the protected region comfortably
//! contains the head, and the QP step lands in low-detail background.
//!
//! All geometry is integer pixel math, frame-clamped, and unit-tested — the
//! ffmpeg `addroi` arg-building in C++ just consumes this plan.

/// A single ROI directive: a rectangle and the QP offset to apply inside it.
/// `qp_offset` is added to the region's quantizer — NEGATIVE = better quality
/// (more bits), POSITIVE = worse (fewer bits). Mirrors ffmpeg `addroi`'s
/// `qoffset`, but we express it as an integer "QP delta" the C++ layer maps to
/// the codec's units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiDirective {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// QP delta in 1/100ths (so we can express e.g. -0.30 as -30 without
    /// floats crossing the FFI). ffmpeg `addroi` qoffset is a rational in
    /// [-1, 1]; the C++ layer divides by 100.0.
    pub qp_delta_centi: i32,
}

/// A complete plan: zero or more directives applied IN ORDER (later ones
/// override earlier ones where they overlap, matching ffmpeg's addroi-chain
/// semantics). Empty = no ROI work (fall back to plain encode).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoiPlan {
    pub directives: Vec<RoiDirective>,
}

impl RoiPlan {
    pub fn is_empty(&self) -> bool {
        self.directives.is_empty()
    }
}

/// Tunables for the plan. Defaults chosen to net-SAVE bits at equal perceived
/// quality on talking-head / selfie video (the common SnapClone case).
#[derive(Debug, Clone, Copy)]
pub struct RoiTuning {
    /// QP delta (centi) applied to the FACE box. Negative = protect/sharpen.
    /// -30 (= -0.30) is a meaningful boost without the over-spend of the old
    /// -1.0 maximum.
    pub face_qp_centi: i32,
    /// QP delta (centi) applied to the WHOLE FRAME as the background baseline.
    /// Positive = compress harder. +25 saves real bits on backgrounds, which a
    /// viewer fixated on the face won't notice.
    pub background_qp_centi: i32,
    /// Fractional outward padding of the face box, in 1/100ths of the box's
    /// own width/height. 30 = pad by 30% each side so the protected region
    /// comfortably contains the whole head, not just the detected rect.
    pub face_pad_centi: u32,
    /// If the saliency box already covers more than this fraction (centi) of
    /// the frame area, the "background" is tiny and a background penalty would
    /// just degrade most of the visible image — so we SKIP the background
    /// penalty (keep only the face boost). 60 = if face >60% of frame, no
    /// global penalty.
    pub max_face_area_for_bg_penalty_centi: u32,
}

impl Default for RoiTuning {
    fn default() -> Self {
        Self {
            face_qp_centi: -30,
            background_qp_centi: 25,
            face_pad_centi: 30,
            max_face_area_for_bg_penalty_centi: 60,
        }
    }
}

/// Pad a box outward by `pad_centi`% of its own size, clamped to the frame.
fn pad_box(
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    pad_centi: u32,
    fw: u32,
    fh: u32,
) -> (u32, u32, u32, u32) {
    let pad_x = (w as u64 * pad_centi as u64 / 100) as u32;
    let pad_y = (h as u64 * pad_centi as u64 / 100) as u32;
    let nx = x.saturating_sub(pad_x);
    let ny = y.saturating_sub(pad_y);
    // Right/bottom edges, clamped to frame.
    let right = (x + w + pad_x).min(fw);
    let bottom = (y + h + pad_y).min(fh);
    let nw = right.saturating_sub(nx);
    let nh = bottom.saturating_sub(ny);
    (nx, ny, nw, nh)
}

/// Build a bit-allocation plan from a raw saliency box (in frame pixel coords)
/// and the frame dimensions. Returns an empty plan when there's no usable ROI
/// (so the caller skips the whole thing and encodes plainly).
///
/// Order of directives (ffmpeg addroi-chain: later overrides earlier):
///   1. background penalty over the full frame (if the face isn't huge),
///   2. face boost over the padded face box.
pub fn plan_from_bbox(
    bx: u32,
    by: u32,
    bw: u32,
    bh: u32,
    frame_w: u32,
    frame_h: u32,
    t: &RoiTuning,
) -> RoiPlan {
    // No usable box, or a degenerate frame → no plan.
    if bw == 0 || bh == 0 || frame_w == 0 || frame_h == 0 {
        return RoiPlan::default();
    }
    // Clamp the incoming box to the frame (defends against probe rounding /
    // rotation-mismatch over-shoot — the C++ side hit exactly this bug before).
    let cx = bx.min(frame_w.saturating_sub(1));
    let cy = by.min(frame_h.saturating_sub(1));
    let cw = bw.min(frame_w - cx);
    let ch = bh.min(frame_h - cy);
    if cw == 0 || ch == 0 {
        return RoiPlan::default();
    }

    let mut plan = RoiPlan::default();

    // Face-area fraction (centi) for the "is the face basically the whole
    // frame?" decision.
    let frame_area = frame_w as u64 * frame_h as u64;
    let face_area = cw as u64 * ch as u64;
    let face_area_centi = (face_area * 100 / frame_area) as u32;

    // 1. Background penalty over the whole frame — unless the face dominates.
    if t.background_qp_centi != 0
        && face_area_centi <= t.max_face_area_for_bg_penalty_centi
    {
        plan.directives.push(RoiDirective {
            x: 0,
            y: 0,
            w: frame_w,
            h: frame_h,
            qp_delta_centi: t.background_qp_centi,
        });
    }

    // 2. Face boost over the padded box (overrides the background where it
    //    overlaps).
    let (px, py, pw, ph) = pad_box(cx, cy, cw, ch, t.face_pad_centi, frame_w, frame_h);
    plan.directives.push(RoiDirective {
        x: px,
        y: py,
        w: pw,
        h: ph,
        qp_delta_centi: t.face_qp_centi,
    });

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const FW: u32 = 1280;
    const FH: u32 = 720;

    #[test]
    fn empty_box_yields_empty_plan() {
        let p = plan_from_bbox(0, 0, 0, 0, FW, FH, &RoiTuning::default());
        assert!(p.is_empty());
    }

    #[test]
    fn typical_face_gets_background_penalty_then_boost() {
        // A ~30%-of-frame face box near center.
        let p = plan_from_bbox(480, 200, 320, 320, FW, FH, &RoiTuning::default());
        assert_eq!(p.directives.len(), 2, "background penalty + face boost");
        // First directive is the full-frame background penalty (positive QP).
        let bg = p.directives[0];
        assert_eq!((bg.x, bg.y, bg.w, bg.h), (0, 0, FW, FH));
        assert!(bg.qp_delta_centi > 0, "background must be penalized (fewer bits)");
        // Second is the face boost (negative QP), and it's PADDED beyond the
        // original 320×320.
        let face = p.directives[1];
        assert!(face.qp_delta_centi < 0, "face must be protected (more bits)");
        assert!(face.w > 320 && face.h > 320, "face box should be padded outward");
    }

    #[test]
    fn padding_is_clamped_to_frame() {
        // Box hugging the top-left corner: padding can't go negative or exceed
        // the frame.
        let p = plan_from_bbox(0, 0, 200, 200, FW, FH, &RoiTuning::default());
        let face = p.directives.last().unwrap();
        assert_eq!(face.x, 0, "left pad clamps at 0");
        assert_eq!(face.y, 0, "top pad clamps at 0");
        assert!(face.x + face.w <= FW);
        assert!(face.y + face.h <= FH);
    }

    #[test]
    fn box_overshooting_frame_is_clamped() {
        // The exact failure the C++ side hit: a box taller than the frame
        // (rotation-mismatch). The planner must clamp, never produce w/h beyond
        // the frame.
        let p = plan_from_bbox(100, 100, 5000, 5000, FW, FH, &RoiTuning::default());
        assert!(!p.is_empty());
        for d in &p.directives {
            assert!(d.x + d.w <= FW, "directive width must stay in frame");
            assert!(d.y + d.h <= FH, "directive height must stay in frame");
        }
    }

    #[test]
    fn huge_face_skips_background_penalty() {
        // A face covering ~78% of the frame: penalizing the "background" would
        // just wreck most of the visible image, so only the boost remains.
        let p = plan_from_bbox(100, 50, 1000, 640, FW, FH, &RoiTuning::default());
        assert_eq!(p.directives.len(), 1, "no background penalty when face dominates");
        assert!(p.directives[0].qp_delta_centi < 0, "only the face boost remains");
    }

    #[test]
    fn bit_reallocation_has_opposite_signs() {
        // The core of the gain: background loses bits, face gains them. Confirm
        // the two directives push QP in OPPOSITE directions (a real
        // reallocation, not just a one-sided boost).
        let p = plan_from_bbox(500, 250, 280, 300, FW, FH, &RoiTuning::default());
        let bg = p.directives[0].qp_delta_centi;
        let face = p.directives[1].qp_delta_centi;
        assert!(bg > 0 && face < 0, "bits move FROM background TO face");
    }

    #[test]
    fn tunable_zero_background_keeps_only_boost() {
        let mut t = RoiTuning::default();
        t.background_qp_centi = 0; // disable background penalty
        let p = plan_from_bbox(480, 200, 320, 320, FW, FH, &t);
        assert_eq!(p.directives.len(), 1);
        assert!(p.directives[0].qp_delta_centi < 0);
    }

    #[test]
    fn directive_order_background_first() {
        // ffmpeg addroi semantics: later filters override earlier. The face
        // boost MUST come after the background penalty so it wins in the
        // overlap region.
        let p = plan_from_bbox(480, 200, 320, 320, FW, FH, &RoiTuning::default());
        assert!(p.directives[0].qp_delta_centi > 0, "background penalty first");
        assert!(p.directives.last().unwrap().qp_delta_centi < 0, "face boost last");
    }
}
