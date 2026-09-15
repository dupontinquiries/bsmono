//! scenelight — what the video is lit like, from what the encoder already knew.
//!
//! ## The free lunch
//!
//! `fao_codec_core::region_activity::BlockActivity` carries, per block:
//!
//! ```text
//! avg_samples: [f32; 16]   a 4x4 luma grid, smoothed over time
//! min_samples, max_samples dynamic range per sample
//! total_motion: u64        accumulated SAD
//! layer: BlockLayer        static / alpha-overlay / moving
//! ```
//!
//! It keeps those to decide what to spend bits on. But a grid of block luma
//! **is a downsampled luminance image of the frame** — already computed,
//! already temporally smoothed, sitting in memory. Reading a key light out of
//! it costs a pass over a few hundred floats, not a decode.
//!
//! That is the whole idea: a physical UI lit by whatever is playing, for
//! approximately free, because the expensive part was done for another
//! reason.
//!
//! ## What is derived, and how honestly
//!
//! * **Direction** — the luma gradient across the frame. A window on the left
//!   makes the left blocks brighter; that is a key light bearing left. Two
//!   dimensions only: a single image cannot tell a light in front from a light
//!   behind, and [`SceneLight`] does not pretend otherwise.
//! * **Intensity** — mean luma, on the encoder's own 0..255 scale.
//! * **Contrast** — how far the bright blocks sit above the dark ones. A
//!   sunlit exterior and an evenly-lit interior can share a mean and look
//!   nothing alike.
//! * **Restlessness** — motion per block. Drives wind and animation, not
//!   light.
//! * **Confidence** — how much of the frame carried usable signal. A blank or
//!   nearly-uniform frame yields a direction that is pure noise, and the
//!   caller needs to know to hold its previous value rather than swing the
//!   sun to a random bearing.
//!
//! ## Chroma is NOT guessed
//!
//! Warmth would be the obvious fifth field and it is absent. The activity
//! tracker samples LUMA only; inferring colour temperature from brightness is
//! a fabrication that would look plausible and be wrong, and a scene lit warm
//! because the video happened to be bright is worse than one lit neutral.
//! [`SceneLight::warmth`] is an `Option` the caller fills from real chroma if
//! it has any.

/// One block's contribution, as the encoder already has it.
///
/// Plain arrays on purpose: this crate must not depend on `fao-codec`, so the
/// caller copies the three fields it needs out of `BlockActivity`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockStats {
    /// Block position in block units, not pixels.
    pub bx: u16,
    pub by: u16,
    /// Mean luma 0..255 — average of `avg_samples`.
    pub luma: f32,
    /// Max minus min across the block's samples: local contrast.
    pub range: f32,
    /// Accumulated motion. Scale is the encoder's; only ratios are used.
    pub motion: u64,
}

/// The light a frame implies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLight {
    /// Unit bearing toward the brighter side, `[x, y]`, screen axes with +y
    /// down. Zero when there is no usable gradient.
    pub dir: [f32; 2],
    /// Mean luma, 0..1.
    pub intensity: f32,
    /// Spread between the bright and dark ends, 0..1.
    pub contrast: f32,
    /// Motion across the frame, 0..1.
    pub restlessness: f32,
    /// How much to trust `dir`, 0..1. See the module header.
    pub confidence: f32,
    /// Colour temperature, −1 cool to +1 warm. `None` unless the caller had
    /// real chroma; never inferred from luma.
    pub warmth: Option<f32>,
}

impl Default for SceneLight {
    /// Flat, neutral, trusted-not-at-all — what to show before any frame has
    /// arrived, and what a blank frame collapses to.
    fn default() -> Self {
        SceneLight {
            dir: [0.0, 0.0],
            intensity: 0.5,
            contrast: 0.0,
            restlessness: 0.0,
            confidence: 0.0,
            warmth: None,
        }
    }
}

/// Below this fraction of full-scale spread, a frame's gradient is noise.
///
/// A flat grey frame still has a gradient if you divide by a small enough
/// number, and it points nowhere in particular — but it points there very
/// confidently, which is how a scene's sun ends up strobing around a wall.
pub const MIN_GRADIENT: f32 = 0.02;

/// Read the light from a frame's block statistics.
///
/// `blocks` may be in any order and need not cover the frame; missing blocks
/// are simply absent rather than assumed dark, because a partial update is
/// the normal case for a codec that only re-analyses what changed.
pub fn light_from_blocks(blocks: &[BlockStats]) -> SceneLight {
    if blocks.is_empty() {
        return SceneLight::default();
    }
    let n = blocks.len() as f32;
    let mean_luma: f32 = blocks.iter().map(|b| b.luma).sum::<f32>() / n;

    // Centroid of position, so the gradient is measured about the frame's
    // middle rather than about the origin -- otherwise every frame appears
    // lit from the top-left simply because block indices start there.
    let cx: f32 = blocks.iter().map(|b| b.bx as f32).sum::<f32>() / n;
    let cy: f32 = blocks.iter().map(|b| b.by as f32).sum::<f32>() / n;

    // Luma-weighted offset from the centroid: where the brightness actually
    // sits. Weighted by luma ABOVE the mean, so dark regions do not pull the
    // bearing by being numerous.
    let (mut gx, mut gy, mut wsum) = (0.0f32, 0.0f32, 0.0f32);
    for b in blocks {
        let w = (b.luma - mean_luma).max(0.0);
        gx += (b.bx as f32 - cx) * w;
        gy += (b.by as f32 - cy) * w;
        wsum += w;
    }
    let (dir, confidence) = if wsum > 1e-6 {
        let (ox, oy) = (gx / wsum, gy / wsum);
        let len = (ox * ox + oy * oy).sqrt();
        // Normalise the offset by the frame's own half-extent so confidence
        // means "the bright side is genuinely to one side", not "the frame is
        // large".
        let span = frame_span(blocks).max(1.0);
        let strength = (len / (span * 0.5)).clamp(0.0, 1.0);
        if len > 1e-6 && strength >= MIN_GRADIENT {
            ([ox / len, oy / len], strength)
        } else {
            ([0.0, 0.0], 0.0)
        }
    } else {
        ([0.0, 0.0], 0.0)
    };

    let lo = blocks.iter().map(|b| b.luma).fold(f32::INFINITY, f32::min);
    let hi = blocks.iter().map(|b| b.luma).fold(f32::NEG_INFINITY, f32::max);
    let motion_mean: f64 = blocks.iter().map(|b| b.motion as f64).sum::<f64>() / n as f64;

    SceneLight {
        dir,
        intensity: (mean_luma / 255.0).clamp(0.0, 1.0),
        contrast: ((hi - lo) / 255.0).clamp(0.0, 1.0),
        // Saturating rather than normalised against a peak: a scene that got
        // windier because one frame was still is worse than one that pins.
        restlessness: (motion_mean / 4096.0).clamp(0.0, 1.0) as f32,
        confidence,
        warmth: None,
    }
}

/// The larger of the frame's two extents, in blocks.
fn frame_span(blocks: &[BlockStats]) -> f32 {
    let (mut x0, mut x1, mut y0, mut y1) = (u16::MAX, 0u16, u16::MAX, 0u16);
    for b in blocks {
        x0 = x0.min(b.bx);
        x1 = x1.max(b.bx);
        y0 = y0.min(b.by);
        y1 = y1.max(b.by);
    }
    ((x1 - x0) as f32).max((y1 - y0) as f32)
}

/// How much of a scene's character comes from the video.
///
/// A switch, not a boolean, because the interesting position is the middle:
/// the physical UI keeps its own identity and the video tints it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Theme {
    /// The scene's own light. The video is not consulted.
    Designed,
    /// Blend, `0.0` designed to `1.0` video.
    Blended(f32),
    /// The video's light, fully.
    Video,
}

impl Theme {
    /// Weight given to the video, 0..1.
    pub fn video_weight(self) -> f32 {
        match self {
            Theme::Designed => 0.0,
            Theme::Blended(w) => w.clamp(0.0, 1.0),
            Theme::Video => 1.0,
        }
    }

    /// Is the video consulted at all? Lets a caller skip the analysis pass
    /// entirely when it is not.
    pub fn uses_video(self) -> bool {
        self.video_weight() > 0.0
    }
}

/// Scene generator parameters this crate is willing to influence.
///
/// A neutral subset of what a scene like logscope's `EnvParams` carries —
/// deliberately not that type. A kit that named another crate's struct would
/// have to move whenever that struct did, and the scene owns its own
/// vocabulary. The caller maps these four onto its own fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Character {
    /// Ambient light level for the scene, 0..1.
    pub ambient: f32,
    /// Key light bearing, `[x, y]`, zero when unknown.
    pub key_dir: [f32; 2],
    /// How hard the shadows are, 0..1. From contrast.
    pub hardness: f32,
    /// Wind and animation vigour, 0..1. From motion.
    pub liveliness: f32,
}

impl Default for Character {
    fn default() -> Self {
        Character { ambient: 0.5, key_dir: [0.0, 0.0], hardness: 0.3, liveliness: 0.2 }
    }
}

/// Blend a designed character with one read from video.
///
/// `confidence` gates the DIRECTION separately from the rest, because it is
/// the only field that can be actively wrong: a low-confidence intensity is
/// merely an average, but a low-confidence bearing is a random number, and
/// swinging the key light to a random bearing reads as a fault rather than as
/// dim lighting.
pub fn blend(designed: Character, light: &SceneLight, theme: Theme) -> Character {
    let w = theme.video_weight();
    if w <= 0.0 {
        return designed;
    }
    let mix = |a: f32, b: f32| a + (b - a) * w;
    let dir_w = w * light.confidence;
    Character {
        ambient: mix(designed.ambient, light.intensity),
        key_dir: [
            designed.key_dir[0] + (light.dir[0] - designed.key_dir[0]) * dir_w,
            designed.key_dir[1] + (light.dir[1] - designed.key_dir[1]) * dir_w,
        ],
        hardness: mix(designed.hardness, light.contrast),
        liveliness: mix(designed.liveliness, light.restlessness),
    }
}

/// Smooth a light toward a new reading.
///
/// Per-frame readings jitter, and a scene whose sun moves every frame looks
/// broken in a way a slightly stale one does not. `alpha` is the weight of
/// the NEW reading; 0.05 at 60fps settles in about a second.
///
/// The direction is slewed by `alpha * confidence`, so an unusable frame
/// leaves the bearing where it was instead of dragging it toward zero — a
/// blank frame should not mean "the sun is overhead", it should mean "no news".
pub fn slew(current: &mut SceneLight, target: &SceneLight, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    current.intensity += (target.intensity - current.intensity) * a;
    current.contrast += (target.contrast - current.contrast) * a;
    current.restlessness += (target.restlessness - current.restlessness) * a;
    let da = a * target.confidence;
    current.dir[0] += (target.dir[0] - current.dir[0]) * da;
    current.dir[1] += (target.dir[1] - current.dir[1]) * da;
    current.confidence += (target.confidence - current.confidence) * a;
    if target.warmth.is_some() {
        current.warmth = match (current.warmth, target.warmth) {
            (Some(c), Some(t)) => Some(c + (t - c) * a),
            (None, t) => t,
            (c, None) => c,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with a bright left edge, in block coordinates.
    fn lit_from(side: &str) -> Vec<BlockStats> {
        let (w, h) = (8u16, 6u16);
        let mut out = Vec::new();
        for by in 0..h {
            for bx in 0..w {
                let t = match side {
                    "left" => 1.0 - bx as f32 / (w - 1) as f32,
                    "right" => bx as f32 / (w - 1) as f32,
                    "top" => 1.0 - by as f32 / (h - 1) as f32,
                    _ => 0.5,
                };
                out.push(BlockStats { bx, by, luma: 20.0 + t * 200.0, range: 30.0, motion: 100 });
            }
        }
        out
    }

    /// The core claim: a window on the left reads as a key light to the left.
    #[test]
    fn a_bright_edge_becomes_a_key_light_bearing() {
        let l = light_from_blocks(&lit_from("left"));
        assert!(l.dir[0] < -0.8, "bearing should point left: {:?}", l.dir);
        assert!(l.confidence > 0.1, "and be trusted: {}", l.confidence);

        let r = light_from_blocks(&lit_from("right"));
        assert!(r.dir[0] > 0.8, "and right when the light is: {:?}", r.dir);

        let t = light_from_blocks(&lit_from("top"));
        assert!(t.dir[1] < -0.8, "+y is down, so a bright top is negative: {:?}", t.dir);
    }

    /// THE ONE THAT MATTERS. A flat frame has a gradient if you divide by a
    /// small enough number, and it points nowhere -- but confidently, which is
    /// how a scene's sun ends up strobing around a wall.
    #[test]
    fn a_flat_frame_yields_no_direction_and_no_confidence() {
        let flat: Vec<BlockStats> = (0..48)
            .map(|i| BlockStats { bx: i % 8, by: i / 8, luma: 128.0, range: 0.0, motion: 0 })
            .collect();
        let l = light_from_blocks(&flat);
        assert_eq!(l.dir, [0.0, 0.0]);
        assert_eq!(l.confidence, 0.0);
        assert!((l.intensity - 128.0 / 255.0).abs() < 1e-3, "but the level is still read");
    }

    /// An empty set is "no news", not "black".
    #[test]
    fn no_blocks_is_the_neutral_default() {
        assert_eq!(light_from_blocks(&[]), SceneLight::default());
        assert_eq!(SceneLight::default().intensity, 0.5, "neutral, not dark");
    }

    /// Mean and contrast are different questions: a sunlit exterior and an
    /// evenly-lit interior can share a mean and look nothing alike.
    #[test]
    fn contrast_is_measured_apart_from_intensity() {
        let even: Vec<BlockStats> = (0..16)
            .map(|i| BlockStats { bx: i, by: 0, luma: 128.0, range: 5.0, motion: 0 })
            .collect();
        let harsh: Vec<BlockStats> = (0..16)
            .map(|i| BlockStats { bx: i, by: 0,
                                  luma: if i % 2 == 0 { 8.0 } else { 248.0 },
                                  range: 200.0, motion: 0 })
            .collect();
        let (a, b) = (light_from_blocks(&even), light_from_blocks(&harsh));
        assert!((a.intensity - b.intensity).abs() < 0.02, "same mean");
        assert!(b.contrast > 0.8 && a.contrast < 0.1, "different light: {} vs {}", a.contrast, b.contrast);
    }

    /// Warmth is never invented from brightness.
    #[test]
    fn warmth_is_never_guessed_from_luma() {
        for side in ["left", "right", "flat"] {
            assert_eq!(light_from_blocks(&lit_from(side)).warmth, None);
        }
    }

    /// Designed mode must not read the video at all, so the caller can skip
    /// the pass entirely.
    #[test]
    fn the_designed_theme_ignores_the_video_completely() {
        let d = Character { ambient: 0.2, key_dir: [1.0, 0.0], hardness: 0.9, liveliness: 0.1 };
        let bright = light_from_blocks(&lit_from("left"));
        assert_eq!(blend(d, &bright, Theme::Designed), d);
        assert!(!Theme::Designed.uses_video());
        assert!(Theme::Blended(0.5).uses_video());
        assert!(!Theme::Blended(0.0).uses_video(), "a zero blend is Designed in practice");
    }

    /// The interesting position is the middle.
    #[test]
    fn blending_moves_partway() {
        let d = Character { ambient: 0.0, key_dir: [0.0, 0.0], hardness: 0.0, liveliness: 0.0 };
        let l = SceneLight { intensity: 1.0, contrast: 1.0, restlessness: 1.0,
                             confidence: 1.0, dir: [1.0, 0.0], warmth: None };
        let half = blend(d, &l, Theme::Blended(0.5));
        assert!((half.ambient - 0.5).abs() < 1e-6);
        assert!((half.hardness - 0.5).abs() < 1e-6);
        let full = blend(d, &l, Theme::Video);
        assert!((full.ambient - 1.0).abs() < 1e-6);
    }

    /// A low-confidence bearing must not move the key light, even at full
    /// video weight -- it is the one field that can be actively wrong.
    #[test]
    fn an_untrusted_bearing_does_not_move_the_key_light() {
        let d = Character { ambient: 0.5, key_dir: [1.0, 0.0], hardness: 0.5, liveliness: 0.5 };
        let murky = SceneLight { dir: [-1.0, 0.0], confidence: 0.0, intensity: 0.9,
                                 ..SceneLight::default() };
        let out = blend(d, &murky, Theme::Video);
        assert_eq!(out.key_dir, [1.0, 0.0], "bearing held");
        assert!((out.ambient - 0.9).abs() < 1e-6, "but the level still followed");
    }

    /// A scene whose sun moves every frame looks broken; a slightly stale one
    /// does not.
    #[test]
    fn slewing_approaches_the_target_without_jumping() {
        let mut cur = SceneLight::default();
        let tgt = SceneLight { intensity: 1.0, confidence: 1.0, dir: [1.0, 0.0],
                               ..SceneLight::default() };
        slew(&mut cur, &tgt, 0.1);
        assert!(cur.intensity > 0.5 && cur.intensity < 0.6, "moved, did not jump");
        for _ in 0..200 {
            slew(&mut cur, &tgt, 0.1);
        }
        assert!((cur.intensity - 1.0).abs() < 1e-3, "and gets there");
        assert!((cur.dir[0] - 1.0).abs() < 1e-3);
    }

    /// A blank frame means "no news", not "the sun is overhead" -- it must
    /// leave the bearing alone rather than dragging it to zero.
    #[test]
    fn an_unusable_frame_leaves_the_bearing_where_it_was() {
        let mut cur = SceneLight { dir: [1.0, 0.0], confidence: 1.0, ..SceneLight::default() };
        let blank = SceneLight::default();
        for _ in 0..50 {
            slew(&mut cur, &blank, 0.2);
        }
        assert!((cur.dir[0] - 1.0).abs() < 1e-3, "bearing held: {:?}", cur.dir);
        assert!(cur.confidence < 0.1, "though it says it is no longer trusted");
    }

    /// Partial coverage is the normal case for a codec that only re-analyses
    /// what changed; missing blocks must not read as black.
    #[test]
    fn a_partial_frame_is_not_treated_as_dark() {
        let bright: Vec<BlockStats> = (0..4)
            .map(|i| BlockStats { bx: i, by: 0, luma: 240.0, range: 10.0, motion: 0 })
            .collect();
        let l = light_from_blocks(&bright);
        assert!(l.intensity > 0.9, "four bright blocks are a bright frame: {}", l.intensity);
    }

    /// Motion drives liveliness and nothing else -- a busy frame is not a
    /// brighter one.
    #[test]
    fn motion_reaches_liveliness_only() {
        let still: Vec<BlockStats> = (0..16)
            .map(|i| BlockStats { bx: i, by: 0, luma: 128.0, range: 10.0, motion: 0 })
            .collect();
        let busy: Vec<BlockStats> = still.iter().map(|b| BlockStats { motion: 8192, ..*b }).collect();
        let (a, b) = (light_from_blocks(&still), light_from_blocks(&busy));
        assert_eq!(a.intensity, b.intensity);
        assert_eq!(a.restlessness, 0.0);
        assert_eq!(b.restlessness, 1.0, "saturates rather than scaling off a peak");
    }
}
