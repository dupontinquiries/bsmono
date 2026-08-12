//! # ink-kit
//!
//! The freehand-drawing backend for the portfolio, shaped by three consumers:
//!
//! * **CADscope** — infinite(-ish) canvas sketching (camera + canvas + undo),
//! * **voiceterm** — a hand-drawing panel whose strokes land in the mind
//!   palace and export to logscope as **niftyplot** graphs (recognize module),
//! * **bot-arena / model finetuning** — the `.ink` corpus format, deterministic
//!   splits, and a rasterizer producing training images (dataset + raster).
//!
//! ## The touchkit seam (the compatibility question)
//!
//! `touchkit` is a *gesture* recognizer: it consumes the whole live pointer set
//! each frame and collapses it into pan/pinch/swipe/fling semantics. Ink wants
//! the opposite — raw per-pointer polylines with pressure, and *no* collapsing.
//! The two are reconciled by [`route::PointerRouter`], which sits in front of
//! both: each frame it partitions the live contacts into an **ink stream**
//! (this crate) and a **nav set** (feed it verbatim to
//! `touchkit::GestureRecognizer::update`). touchkit's frame-driven contract is
//! untouched; the router owns the ambiguity window ("is this first finger a
//! stroke or the start of a two-finger pan?") by buffering provisional points.
//!
//! Reference for the interaction model: InfiniPaint (GPL, *not* copied — its
//! C++/Skia code stays out of this tree; what we take is the shape of the
//! design: a coord-space helper between screen and world, pen-priority palm
//! rejection, second-finger-cancels-stroke).
//!
//! ## Layer map
//!
//! ```text
//! stroke   points/strokes/styles          (data)
//! filter   one-euro smoothing, RDP        (pure funcs)
//! camera   f64 world <-> f32 screen       (CADscope viewport)
//! canvas   store + grid index + undo      (document)
//! route    pen/touch/mouse -> ink | nav   (the touchkit seam)
//! recognize strokes -> boxes/edges -> niftyplot text
//! raster   strokes -> Gray8 bitmap -> PGM (training images, thumbnails)
//! io       .ink text format v1            (corpus + mind-palace files)
//! dataset  samples, manifest, split, export
//! ```
//!
//! Everything is std-only. The egui adapter (pressure extraction, pointer-kind
//! detection, sharing the live-touch map with `uikit-egui::touch::TouchSource`)
//! is `sketches/egui_ink_panel.rs`, following the touchkit convention.

pub mod air;
pub mod camera;
pub mod canvas;
pub mod dataset;
pub mod filter;
pub mod io;
pub mod raster;
pub mod recognize;
pub mod route;
pub mod stroke;

/// f32 screen-space vector (matches touchkit's shape so adapters are trivial).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2f {
    pub x: f32,
    pub y: f32,
}
impl Vec2f {
    pub const ZERO: Vec2f = Vec2f { x: 0.0, y: 0.0 };
    #[inline]
    pub fn new(x: f32, y: f32) -> Self {
        Vec2f { x, y }
    }
    #[inline]
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    #[inline]
    pub fn dist(self, o: Vec2f) -> f32 {
        (self - o).length()
    }
}
impl std::ops::Sub for Vec2f {
    type Output = Vec2f;
    #[inline]
    fn sub(self, o: Vec2f) -> Vec2f {
        Vec2f::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Add for Vec2f {
    type Output = Vec2f;
    #[inline]
    fn add(self, o: Vec2f) -> Vec2f {
        Vec2f::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Mul<f32> for Vec2f {
    type Output = Vec2f;
    #[inline]
    fn mul(self, s: f32) -> Vec2f {
        Vec2f::new(self.x * s, self.y * s)
    }
}

/// f64 world-space vector. World coordinates are f64 so a phone-sized canvas
/// can zoom ~2^40 before precision artifacts — not InfiniPaint's unbounded
/// zoom, but a deliberate cut: the day someone needs more, `WorldScalar`
/// becomes a fixed-point/bignum newtype *here* and the camera API is the seam
/// that hides it (no caller stores raw world floats except through these types).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2d {
    pub x: f64,
    pub y: f64,
}
impl Vec2d {
    pub const ZERO: Vec2d = Vec2d { x: 0.0, y: 0.0 };
    #[inline]
    pub fn new(x: f64, y: f64) -> Self {
        Vec2d { x, y }
    }
    #[inline]
    pub fn dist(self, o: Vec2d) -> f64 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        (dx * dx + dy * dy).sqrt()
    }
}
impl std::ops::Sub for Vec2d {
    type Output = Vec2d;
    #[inline]
    fn sub(self, o: Vec2d) -> Vec2d {
        Vec2d::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Add for Vec2d {
    type Output = Vec2d;
    #[inline]
    fn add(self, o: Vec2d) -> Vec2d {
        Vec2d::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Mul<f64> for Vec2d {
    type Output = Vec2d;
    #[inline]
    fn mul(self, s: f64) -> Vec2d {
        Vec2d::new(self.x * s, self.y * s)
    }
}

/// Axis-aligned world rect (min/max corners).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectD {
    pub min: Vec2d,
    pub max: Vec2d,
}
impl RectD {
    pub const EMPTY: RectD = RectD {
        min: Vec2d {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        max: Vec2d {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    };
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }
    #[inline]
    pub fn include(&mut self, p: Vec2d) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }
    #[inline]
    pub fn expand(&self, m: f64) -> RectD {
        RectD {
            min: Vec2d::new(self.min.x - m, self.min.y - m),
            max: Vec2d::new(self.max.x + m, self.max.y + m),
        }
    }
    #[inline]
    pub fn intersects(&self, o: &RectD) -> bool {
        !(self.is_empty() || o.is_empty())
            && self.min.x <= o.max.x
            && o.min.x <= self.max.x
            && self.min.y <= o.max.y
            && o.min.y <= self.max.y
    }
    #[inline]
    pub fn contains(&self, p: Vec2d) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }
    #[inline]
    pub fn center(&self) -> Vec2d {
        Vec2d::new((self.min.x + self.max.x) * 0.5, (self.min.y + self.max.y) * 0.5)
    }
    #[inline]
    pub fn width(&self) -> f64 {
        (self.max.x - self.min.x).max(0.0)
    }
    #[inline]
    pub fn height(&self) -> f64 {
        (self.max.y - self.min.y).max(0.0)
    }
}
