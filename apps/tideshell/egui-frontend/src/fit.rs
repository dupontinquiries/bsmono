//! Putting a camera frame on screen without lying about the world.
//!
//! A camera sensor delivers landscape; a phone pane is portrait. Painting
//! the whole frame into the whole pane makes both axes scale by different
//! factors, so a face becomes a tall oval and a doorway leans. The frame is
//! also delivered in the SENSOR's orientation, which on a phone is a quarter
//! turn away from the screen's.
//!
//! Two rules, and everything here follows from them:
//!
//! 1. **One scale factor for both axes.** Whatever the aspect mismatch, the
//!    fix is to show LESS of the frame, never to squash it. Real-world
//!    proportions survive; the edges are cropped.
//! 2. **Crop from the centre.** The middle of the frame is what a phone is
//!    pointed at.
//!
//! This is "cover" rather than "contain": the pane is filled and the frame
//! overflows. For a lock-screen tint, letterbox bars would be worse than a
//! crop — they are a visible frame around something that is meant to read as
//! light coming through glass.

/// The sensor's orientation relative to the screen. Which one applies is a
/// property of how the camera is mounted, not of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quarter {
    /// The sensor is mounted the same way up as the screen. The default
    /// because it is the only assumption that cannot itself introduce a
    /// rotation; a wrong guess here is worse than none.
    #[default]
    None,
    /// Rotate the image 90 degrees clockwise when drawing.
    Cw90,
    Ccw90,
    Half,
}

impl Quarter {
    /// Does this turn swap width and height?
    pub fn swaps_axes(self) -> bool {
        matches!(self, Quarter::Cw90 | Quarter::Ccw90)
    }

    pub fn parse(s: &str) -> Option<Quarter> {
        match s.trim() {
            "0" | "none" => Some(Quarter::None),
            "90" | "cw90" => Some(Quarter::Cw90),
            "270" | "-90" | "ccw90" => Some(Quarter::Ccw90),
            "180" | "half" => Some(Quarter::Half),
            _ => None,
        }
    }
}

/// The part of the frame to sample, in texture coordinates: `(u0, v0, u1,
/// v1)`, all 0..1.
///
/// `src` is the frame in pixels, `dst` the pane. Rotation is applied
/// conceptually BEFORE fitting, so a quarter turn compares the frame's
/// height against the pane's width.
pub fn cover_uv(src: (f32, f32), dst: (f32, f32), rot: Quarter) -> (f32, f32, f32, f32) {
    if src.0 <= 0.0 || src.1 <= 0.0 || dst.0 <= 0.0 || dst.1 <= 0.0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    // The frame's aspect as it will APPEAR, once turned.
    let (ew, eh) = if rot.swaps_axes() { (src.1, src.0) } else { (src.0, src.1) };
    let src_aspect = ew / eh;
    let dst_aspect = dst.0 / dst.1;
    // Fraction of the appeared frame that is kept on each axis. One is
    // always 1.0: cropping both would be zooming, which changes scale.
    let (keep_w, keep_h) = if src_aspect > dst_aspect {
        (dst_aspect / src_aspect, 1.0)
    } else {
        (1.0, src_aspect / dst_aspect)
    };
    // Back into the frame's own axes.
    let (ku, kv) = if rot.swaps_axes() { (keep_h, keep_w) } else { (keep_w, keep_h) };
    let (u0, v0) = ((1.0 - ku) / 2.0, (1.0 - kv) / 2.0);
    (u0, v0, u0 + ku, v0 + kv)
}

/// The four texture coordinates for a quad's corners, in the order
/// top-left, top-right, bottom-right, bottom-left of the DESTINATION.
///
/// Rotation happens here, by handing each destination corner a different
/// corner of the source rectangle — no pixels are moved and no second
/// texture is needed.
pub fn corner_uvs(src: (f32, f32), dst: (f32, f32), rot: Quarter) -> [[f32; 2]; 4] {
    let (u0, v0, u1, v1) = cover_uv(src, dst, rot);
    let (tl, tr, br, bl) = ([u0, v0], [u1, v0], [u1, v1], [u0, v1]);
    match rot {
        Quarter::None => [tl, tr, br, bl],
        // Turning the IMAGE clockwise means each destination corner reads
        // from the source corner one step anticlockwise.
        Quarter::Cw90 => [bl, tl, tr, br],
        Quarter::Ccw90 => [tr, br, bl, tl],
        Quarter::Half => [br, bl, tl, tr],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(src: (f32, f32), dst: (f32, f32), rot: Quarter) -> (f32, f32) {
        let (u0, v0, u1, v1) = cover_uv(src, dst, rot);
        ((u1 - u0) * src.0, (v1 - v0) * src.1)
    }

    #[test]
    fn nothing_is_ever_squashed() {
        // The sampled region, once turned, must have the pane's aspect —
        // that is what makes one scale factor serve both axes.
        for (src, dst, rot) in [
            ((640.0, 480.0), (240.0, 320.0), Quarter::None),
            ((640.0, 480.0), (240.0, 320.0), Quarter::Cw90),
            ((480.0, 640.0), (1080.0, 2400.0), Quarter::Ccw90),
            ((1920.0, 1080.0), (1080.0, 1920.0), Quarter::Cw90),
            ((320.0, 320.0), (240.0, 320.0), Quarter::Half),
        ] {
            let (kw, kh) = kept(src, dst, rot);
            let shown = if rot.swaps_axes() { kh / kw } else { kw / kh };
            let want = dst.0 / dst.1;
            assert!((shown - want).abs() < 1e-4, "{src:?} -> {dst:?} {rot:?}: showed aspect {shown}, pane is {want}");
        }
    }

    #[test]
    fn the_crop_is_centred_and_only_one_axis_is_cut() {
        let (u0, v0, u1, v1) = cover_uv((640.0, 480.0), (240.0, 320.0), Quarter::None);
        assert!((u0 - (1.0 - (u1 - u0)) / 2.0).abs() < 1e-6, "centred horizontally");
        assert!((v0 - (1.0 - (v1 - v0)) / 2.0).abs() < 1e-6, "centred vertically");
        // A landscape frame in a portrait pane loses the sides, all of the
        // height is kept: cropping both axes would be a zoom.
        assert!(u1 - u0 < 1.0);
        assert_eq!((v0, v1), (0.0, 1.0));
        // Matching aspects crop nothing at all.
        assert_eq!(cover_uv((480.0, 640.0), (240.0, 320.0), Quarter::None), (0.0, 0.0, 1.0, 1.0));
        // Turning a 4:3 sensor into a 3:4 pane is an EXACT fit: nothing is
        // cropped, which is worth knowing before anyone tunes a crop that
        // should not exist.
        assert_eq!(cover_uv((640.0, 480.0), (240.0, 320.0), Quarter::Cw90), (0.0, 0.0, 1.0, 1.0));
        // With a real mismatch, a quarter turn moves which axis is cut: the
        // frame's WIDTH now spans the pane's height, so the cut is in v.
        let (a0, b0, a1, b1) = cover_uv((640.0, 480.0), (240.0, 400.0), Quarter::Cw90);
        assert_eq!((a0, a1), (0.0, 1.0), "the full frame height spans the pane width");
        assert!(b1 - b0 < 1.0, "and the frame's width is cropped");
    }

    #[test]
    fn a_quarter_turn_permutes_the_corners_and_nothing_else() {
        let src = (640.0, 480.0);
        let dst = (480.0, 640.0);
        for rot in [Quarter::None, Quarter::Cw90, Quarter::Ccw90, Quarter::Half] {
            let c = corner_uvs(src, dst, rot);
            let mut sorted = c;
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut base = corner_uvs(src, dst, rot);
            base.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(sorted, base, "same four points, different order");
            // Four distinct corners: a degenerate quad would mean a fold.
            for i in 0..4 {
                for j in i + 1..4 {
                    assert_ne!(c[i], c[j], "{rot:?} corners {i},{j} coincide");
                }
            }
        }
        // Clockwise and anticlockwise are genuinely different.
        assert_ne!(corner_uvs(src, dst, Quarter::Cw90), corner_uvs(src, dst, Quarter::Ccw90));
        // And a half turn is two of either.
        assert_eq!(corner_uvs(src, dst, Quarter::Half)[0], corner_uvs(src, dst, Quarter::None)[2]);
    }

    #[test]
    fn degenerate_sizes_do_not_produce_nonsense() {
        for bad in [(0.0, 480.0), (640.0, 0.0), (-1.0, 5.0)] {
            assert_eq!(cover_uv(bad, (240.0, 320.0), Quarter::None), (0.0, 0.0, 1.0, 1.0));
            assert_eq!(cover_uv((640.0, 480.0), bad, Quarter::Cw90), (0.0, 0.0, 1.0, 1.0));
        }
    }

    #[test]
    fn orientation_is_read_from_a_setting() {
        assert_eq!(Quarter::parse("90"), Some(Quarter::Cw90));
        assert_eq!(Quarter::parse("270"), Some(Quarter::Ccw90));
        assert_eq!(Quarter::parse("-90"), Some(Quarter::Ccw90));
        assert_eq!(Quarter::parse("180"), Some(Quarter::Half));
        assert_eq!(Quarter::parse("0"), Some(Quarter::None));
        assert_eq!(Quarter::parse("sideways"), None);
        assert!(Quarter::Cw90.swaps_axes() && !Quarter::Half.swaps_axes());
    }
}
