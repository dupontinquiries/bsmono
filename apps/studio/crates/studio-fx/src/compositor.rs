//! compositor — stacking video layers into one frame.
//!
//! The viewport showed the FIRST video clip under the playhead; a second
//! video track was invisible. Compositing is what makes the second track
//! mean something: a title over footage, a texture multiplied in, a light
//! leak screened on top. This is the CPU compositor for that — pure over
//! RGB byte frames, so a two-layer stack is a test — and the `blend` effect
//! kind that gives a clip its blend mode and opacity as ordinary params,
//! editable in the fx editor like everything else.
//!
//! Layers are painted bottom to top. Each layer is sampled through its
//! `Transform` (inverse mapping, nearest neighbour: a preview compositor, not
//! a renderer) and blended onto the canvas with its mode at its opacity.
//! Coordinates in `Transform` are the ones the effect already uses: `x`/`y`
//! as fractions of the canvas, `scale` about the anchor, rotation in
//! degrees.
//!
//! Blend modes are the classic five with the formulas everyone agrees on
//! (Porter–Duff over for Normal; Add clamps; Multiply and Screen are
//! symmetric; Overlay is Multiply below mid-grey and Screen above).

use crate::Transform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Add,
    Multiply,
    Screen,
    Overlay,
}

impl BlendMode {
    pub const ALL: [BlendMode; 5] = [BlendMode::Normal, BlendMode::Add, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay];

    pub fn from_index(i: usize) -> BlendMode { Self::ALL.get(i).copied().unwrap_or(BlendMode::Normal) }

    /// One channel, both in 0..1. `b` is the layer being painted, `a` the canvas.
    #[inline]
    pub fn apply(self, a: f32, b: f32) -> f32 {
        match self {
            BlendMode::Normal => b,
            BlendMode::Add => (a + b).min(1.0),
            BlendMode::Multiply => a * b,
            BlendMode::Screen => 1.0 - (1.0 - a) * (1.0 - b),
            BlendMode::Overlay => if a < 0.5 { 2.0 * a * b } else { 1.0 - 2.0 * (1.0 - a) * (1.0 - b) },
        }
    }
}

/// One video layer: an RGB frame and how it sits on the canvas.
pub struct Layer<'a> {
    pub rgb: &'a [u8],
    pub w: u32,
    pub h: u32,
    pub transform: Transform,
    pub mode: BlendMode,
    /// Multiplied with `transform.opacity`; the blend effect's own dial.
    pub opacity: f32,
}

/// Paint `layers` bottom-to-top onto a `w`×`h` canvas starting from
/// `background` (an RGB triple). Returns RGB bytes.
pub fn composite(w: u32, h: u32, background: [u8; 3], layers: &[Layer<'_>]) -> Vec<u8> {
    let (wi, hi) = (w as usize, h as usize);
    let mut canvas: Vec<f32> = Vec::with_capacity(wi * hi * 3);
    for _ in 0..wi * hi {
        canvas.extend_from_slice(&[background[0] as f32 / 255.0, background[1] as f32 / 255.0, background[2] as f32 / 255.0]);
    }
    for layer in layers {
        let alpha = (layer.opacity * layer.transform.opacity).clamp(0.0, 1.0);
        if alpha <= 0.0 || layer.w == 0 || layer.h == 0 { continue; }
        let t = &layer.transform;
        // (cos θ, sin θ): the canvas-to-layer map below is then a turn of
        // -rotation, the inverse of the model's clockwise +rotation
        // (`Transform::apply` / `invert`). It was (-θ) here until c20, which
        // turned the preview the wrong way.
        let (cos, sin) = t.rotation.to_radians().sin_cos().swap();
        let (ax, ay) = (t.anchor_x * w as f32, t.anchor_y * h as f32);
        let (sx, sy) = (t.scale_x.max(1e-4), t.scale_y.max(1e-4));
        let (ox, oy) = (t.x * w as f32, t.y * h as f32);
        // Crop as a fraction of the layer's own frame.
        let (cl, ct, cr, cb) = t.crop;
        let lw = layer.w as f32;
        let lh = layer.h as f32;
        for py in 0..hi {
            for px in 0..wi {
                // Canvas → layer: undo the offset, un-rotate about the anchor, un-scale, then map the
                // canvas-sized layer space onto the layer's own pixel grid.
                let dx = px as f32 + 0.5 - ox - ax;
                let dy = py as f32 + 0.5 - oy - ay;
                let rx = dx * cos + dy * sin;
                let ry = -dx * sin + dy * cos;
                let ux = (rx / sx + ax) / w as f32; // 0..1 across the canvas-sized layer
                let uy = (ry / sy + ay) / h as f32;
                if !(cl..1.0 - cr).contains(&ux) || !(ct..1.0 - cb).contains(&uy) { continue; }
                let lx = (ux * lw) as usize;
                let ly = (uy * lh) as usize;
                if lx >= layer.w as usize || ly >= layer.h as usize { continue; }
                let li = (ly * layer.w as usize + lx) * 3;
                let ci = (py * wi + px) * 3;
                for c in 0..3 {
                    let a = canvas[ci + c];
                    let b = layer.rgb[li + c] as f32 / 255.0;
                    let blended = layer.mode.apply(a, b);
                    canvas[ci + c] = a + (blended - a) * alpha;
                }
            }
        }
    }
    canvas.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect()
}

trait Swap { fn swap(self) -> Self; }
impl Swap for (f32, f32) { fn swap(self) -> Self { (self.1, self.0) } }

/// The `blend` effect's params, read the way `instantiate` reads every other kind.
pub fn blend_of(effect: &studio_core::Effect) -> (BlendMode, f32) {
    let mode = effect.params.get("mode").copied().unwrap_or(0.0) as usize;
    let opacity = effect.params.get("opacity").copied().unwrap_or(1.0).clamp(0.0, 1.0);
    (BlendMode::from_index(mode), opacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rotation is "degrees, clockwise" (`Transform::rotation`), and the
    /// model's own forward map (`Transform::apply`) turns a point RIGHT of
    /// the anchor to BELOW it at +90 on a y-down screen. The compositor must
    /// agree: its canvas-to-layer map is the INVERSE, a turn of -rotation.
    /// Until c20 it turned by +rotation there, so the preview spun every
    /// layer the wrong way (and the export had no rotation to disagree with).
    #[test]
    fn a_positive_rotation_turns_clockwise_as_the_model_says() {
        let n = 9u32;
        let mut rgb = vec![0u8; (n * n * 3) as usize];
        for x in 6..9 { let i = ((4 * n + x) * 3) as usize; rgb[i..i + 3].copy_from_slice(&[255, 255, 255]); }
        let t = Transform { rotation: 90.0, ..Default::default() };
        let out = composite(n, n, [0, 0, 0], &[Layer { rgb: &rgb, w: n, h: n, transform: t, mode: BlendMode::Normal, opacity: 1.0 }]);
        let lit = |x: u32, y: u32| out[((y * n + x) * 3) as usize] > 128;
        // the model's forward map, for the same point
        let (fx, fy) = t.apply(7.5 / 9.0, 4.5 / 9.0);
        assert!((fx - 0.5).abs() < 1e-4 && fy > 0.5, "the model: right of centre goes BELOW ({fx}, {fy})");
        assert!(lit(4, 7), "the compositor puts it below too");
        assert!(!lit(4, 1), "and not above");
    }

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| rgb).collect()
    }
    fn layer<'a>(rgb: &'a [u8], w: u32, h: u32, mode: BlendMode, opacity: f32) -> Layer<'a> {
        Layer { rgb, w, h, transform: Transform::default(), mode, opacity }
    }
    fn px(frame: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 3) as usize;
        [frame[i], frame[i + 1], frame[i + 2]]
    }

    #[test]
    fn each_blend_mode_does_what_its_name_says() {
        let base = solid(4, 4, [128, 128, 128]);
        let top = solid(4, 4, [128, 128, 128]);
        let out = |m: BlendMode| px(&composite(4, 4, [0, 0, 0], &[layer(&base, 4, 4, BlendMode::Normal, 1.0), layer(&top, 4, 4, m, 1.0)]), 4, 1, 1)[0];
        assert_eq!(out(BlendMode::Normal), 128);
        assert_eq!(out(BlendMode::Add), 255, "add clamps at white");
        assert!((out(BlendMode::Multiply) as i32 - 64).abs() <= 1, "multiply darkens: {}", out(BlendMode::Multiply));
        assert!((out(BlendMode::Screen) as i32 - 192).abs() <= 1, "screen lightens: {}", out(BlendMode::Screen));
        let ov = out(BlendMode::Overlay);
        assert!((ov as i32 - 128).abs() <= 2, "overlay at mid-grey is mid-grey: {ov}");
    }

    #[test]
    fn opacity_mixes_and_a_missing_second_track_is_the_first_alone() {
        let base = solid(2, 2, [0, 0, 0]);
        let top = solid(2, 2, [255, 255, 255]);
        let half = composite(2, 2, [0, 0, 0], &[layer(&base, 2, 2, BlendMode::Normal, 1.0), layer(&top, 2, 2, BlendMode::Normal, 0.5)]);
        assert!((px(&half, 2, 0, 0)[0] as i32 - 128).abs() <= 1);
        let alone = composite(2, 2, [9, 9, 9], &[layer(&top, 2, 2, BlendMode::Normal, 1.0)]);
        assert_eq!(px(&alone, 2, 1, 1), [255, 255, 255]);
        let none = composite(2, 2, [9, 9, 9], &[]);
        assert_eq!(px(&none, 2, 0, 0), [9, 9, 9], "no layers: the background");
    }

    #[test]
    fn a_transform_moves_scales_and_crops_the_layer() {
        // An 8×8 white layer shifted right by half the canvas: left half stays background.
        let top = solid(8, 8, [255, 255, 255]);
        let mut t = Transform::default();
        t.x = 0.5;
        let moved = composite(8, 8, [0, 0, 0], &[Layer { rgb: &top, w: 8, h: 8, transform: t, mode: BlendMode::Normal, opacity: 1.0 }]);
        assert_eq!(px(&moved, 8, 1, 4), [0, 0, 0]);
        assert_eq!(px(&moved, 8, 6, 4), [255, 255, 255]);
        // Scaled to half about the centre: corners are background, the middle is white.
        let mut s = Transform::default();
        s.scale_x = 0.5; s.scale_y = 0.5;
        let small = composite(8, 8, [0, 0, 0], &[Layer { rgb: &top, w: 8, h: 8, transform: s, mode: BlendMode::Normal, opacity: 1.0 }]);
        assert_eq!(px(&small, 8, 0, 0), [0, 0, 0]);
        assert_eq!(px(&small, 8, 4, 4), [255, 255, 255]);
        // Cropped on the left by a quarter.
        let mut c = Transform::default();
        c.crop = (0.25, 0.0, 0.0, 0.0);
        let cropped = composite(8, 8, [0, 0, 0], &[Layer { rgb: &top, w: 8, h: 8, transform: c, mode: BlendMode::Normal, opacity: 1.0 }]);
        assert_eq!(px(&cropped, 8, 0, 3), [0, 0, 0]);
        assert_eq!(px(&cropped, 8, 3, 3), [255, 255, 255]);
        // A layer smaller than the canvas is stretched to it (nearest).
        let tiny = solid(2, 2, [10, 20, 30]);
        let up = composite(8, 8, [0, 0, 0], &[layer(&tiny, 2, 2, BlendMode::Normal, 1.0)]);
        assert_eq!(px(&up, 8, 7, 7), [10, 20, 30]);
    }

    #[test]
    fn the_blend_effect_reads_its_params() {
        let mut fx = studio_core::Effect { kind: "blend".into(), params: Default::default(), enabled: true };
        assert_eq!(blend_of(&fx), (BlendMode::Normal, 1.0));
        fx.params.insert("mode".into(), 2.0);
        fx.params.insert("opacity".into(), 0.4);
        assert_eq!(blend_of(&fx), (BlendMode::Multiply, 0.4));
        fx.params.insert("mode".into(), 99.0);
        assert_eq!(blend_of(&fx).0, BlendMode::Normal, "an unknown index is Normal, not a panic");
    }
}
