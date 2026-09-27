//! The cover clock in 3D, for a phone lying face-up on a table.
//!
//! ## The effect
//!
//! A phone flat on a table is looked at from past its bottom edge, at an
//! angle. So "3D" here is anamorphic, like a pavement drawing: every point of
//! a 3D clock that stands ON the glass is drawn where the line from the
//! viewer's eye through that point meets the screen plane. From the eye the
//! digits then appear to stand up off the glass; from anywhere else they look
//! stretched, which is the point. One projection, four ways to use it
//! ([`Style`]):
//!
//! * `Standing`: seven-segment digits stand upright on the glass like a desk
//!   clock, with a shadow cast on the "table" by a light behind the viewer.
//! * `Extruded`: the digits lie flat but are extruded up out of the glass,
//!   so their side walls show.
//! * `Hologram`: translucent slices of the face stacked above the glass,
//!   bobbing slowly.
//! * `Carousel`: the digits stand on a slowly turning ring.
//!
//! ## Flat detection
//!
//! [`FlatDetector`] prefers a gravity vector (IIO accelerometer) and falls
//! back to iio-sensor-proxy's orientation string, which reads "undefined"
//! when the phone is flat. On a Fairphone 5 WITHOUT a sensor-hub client
//! neither source exists (see camera.rs: the accelerometer sits behind the
//! ADSP), so `Trigger::Auto` never fires there; `Trigger::Always` shows the
//! effect regardless, and the detector's `source()` says which case applies.
//!
//! Everything here is pure: geometry in, polygons out, painted by the
//! frontend as one egui mesh. No allocation beyond the output list.

use crate::Millis;

/// When the 3D clock is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Off,
    /// Only while the phone lies face-up (needs an orientation source).
    Auto,
    /// Always on the cover (for phones without a usable sensor, and for trying styles).
    Always,
}

impl Trigger {
    pub const ALL: [Trigger; 3] = [Trigger::Off, Trigger::Auto, Trigger::Always];
    pub fn token(self) -> &'static str {
        match self {
            Trigger::Off => "off",
            Trigger::Auto => "auto",
            Trigger::Always => "always",
        }
    }
    pub fn from_token(v: &str) -> Option<Trigger> {
        Trigger::ALL.into_iter().find(|t| t.token() == v)
    }
    pub fn label(self) -> &'static str {
        match self {
            Trigger::Off => "Off",
            Trigger::Auto => "When lying flat",
            Trigger::Always => "Always",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Standing,
    Extruded,
    Hologram,
    Carousel,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Standing, Style::Extruded, Style::Hologram, Style::Carousel];
    pub fn token(self) -> &'static str {
        match self {
            Style::Standing => "standing",
            Style::Extruded => "extruded",
            Style::Hologram => "hologram",
            Style::Carousel => "carousel",
        }
    }
    pub fn from_token(v: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.token() == v)
    }
    pub fn label(self) -> &'static str {
        match self {
            Style::Standing => "Standing",
            Style::Extruded => "Extruded",
            Style::Hologram => "Hologram",
            Style::Carousel => "Carousel",
        }
    }
    /// Whether the style moves on its own (and so needs frames while shown).
    /// Standing and Extruded are still between minute changes: no frames.
    pub fn animates(self) -> bool {
        matches!(self, Style::Hologram | Style::Carousel)
    }
    /// Frame interval an animating style needs. Deliberately low: slow motion
    /// reads as smooth at 20 Hz, and this runs on a lock screen.
    pub fn frame_ms(self) -> Option<u64> {
        self.animates().then_some(50)
    }
}

// ---- flat detection ----------------------------------------------------------

/// Tilt from face-up under which the phone counts as lying flat, and the one
/// it must exceed to stop counting (hysteresis: a hand resting on the table
/// edge must not flicker the clock).
pub const FLAT_ENTER_DEG: f32 = 12.0;
pub const FLAT_EXIT_DEG: f32 = 20.0;
/// How long the reading must hold before the clock tips into 3D.
pub const FLAT_HOLD_MS: Millis = 800;
/// The orientation string must read "undefined" this long (it is also what a
/// phone reports mid-rotation).
pub const UNDEFINED_HOLD_MS: Millis = 1500;
/// An accelerometer reading older than this no longer outranks the orientation string.
pub const ACCEL_STALE_MS: Millis = 2000;
/// Tip-in / tip-out duration.
pub const TRANSITION_MS: f32 = 450.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatSource {
    None,
    Accelerometer,
    Orientation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlatDetector {
    flat: bool,
    /// When the current candidate state (flat or not) started holding.
    candidate: Option<(bool, Millis)>,
    last_accel: Option<Millis>,
    undefined_since: Option<Millis>,
    /// 0 = drawn flat (2D), 1 = full 3D; eased toward `flat`.
    pub amount: f32,
}

impl Default for FlatDetector {
    fn default() -> Self {
        FlatDetector { flat: false, candidate: None, last_accel: None, undefined_since: None, amount: 0.0 }
    }
}

/// Degrees between the screen normal and "up", for a gravity reading in the
/// device frame (x right, y up the screen, z out of the screen). Face-up on a
/// table reads about (0, 0, +9.8). `None` for a reading too weak to trust
/// (free fall, a zeroed sensor).
pub fn tilt_from_up_deg(a: [f32; 3]) -> Option<f32> {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if !(n > 3.0) {
        return None;
    }
    Some((a[2] / n).clamp(-1.0, 1.0).acos().to_degrees())
}

impl FlatDetector {
    pub fn is_flat(&self) -> bool {
        self.flat
    }

    pub fn source(&self, now: Millis) -> FlatSource {
        if self.last_accel.is_some_and(|t| now.saturating_sub(t) < ACCEL_STALE_MS) {
            FlatSource::Accelerometer
        } else if self.undefined_since.is_some() || self.candidate.is_some() {
            FlatSource::Orientation
        } else {
            FlatSource::None
        }
    }

    fn propose(&mut self, flat: bool, now: Millis, hold: Millis) {
        if flat == self.flat {
            self.candidate = None;
            return;
        }
        match self.candidate {
            Some((c, since)) if c == flat => {
                if now.saturating_sub(since) >= hold {
                    self.flat = flat;
                    self.candidate = None;
                }
            }
            _ => self.candidate = Some((flat, now)),
        }
    }

    /// A gravity reading (m/s², device frame).
    pub fn on_accel(&mut self, a: [f32; 3], now: Millis) {
        let Some(tilt) = tilt_from_up_deg(a) else { return };
        self.last_accel = Some(now);
        let limit = if self.flat { FLAT_EXIT_DEG } else { FLAT_ENTER_DEG };
        // Leaving flat is immediate-ish (the phone was picked up); entering waits.
        let hold = if self.flat { 150 } else { FLAT_HOLD_MS };
        self.propose(tilt <= limit, now, hold);
    }

    /// iio-sensor-proxy's AccelerometerOrientation. Ignored while a fresh
    /// accelerometer reading exists (the vector is the better source).
    pub fn on_orientation(&mut self, o: &str, now: Millis) {
        if self.source(now) == FlatSource::Accelerometer {
            return;
        }
        if o == "undefined" {
            let since = *self.undefined_since.get_or_insert(now);
            if now.saturating_sub(since) >= UNDEFINED_HOLD_MS {
                self.flat = true;
                self.candidate = None;
            } else {
                self.candidate = Some((true, since));
            }
        } else {
            self.undefined_since = None;
            self.flat = false;
            self.candidate = None;
        }
    }

    /// Advance time (for the orientation hold, which has no further readings
    /// while the phone stays put, and for the tip-in easing). Returns whether
    /// anything visible changed.
    pub fn tick(&mut self, dt_ms: f32, now: Millis) -> bool {
        if let Some(since) = self.undefined_since {
            if !self.flat && now.saturating_sub(since) >= UNDEFINED_HOLD_MS && self.source(now) != FlatSource::Accelerometer {
                self.flat = true;
                self.candidate = None;
            }
        }
        let target = if self.flat { 1.0 } else { 0.0 };
        let before = self.amount;
        let step = (dt_ms / TRANSITION_MS).max(0.0);
        self.amount = if self.amount < target { (self.amount + step).min(target) } else { (self.amount - step).max(target) };
        self.amount != before
    }

    pub fn is_transitioning(&self) -> bool {
        let target = if self.flat { 1.0 } else { 0.0 };
        self.amount != target
    }
}

/// How much 3D to draw, from the trigger and the detector (0..1, eased).
pub fn amount(trigger: Trigger, det: &FlatDetector) -> f32 {
    match trigger {
        Trigger::Off => 0.0,
        Trigger::Always => 1.0,
        Trigger::Auto => smoothstep(det.amount),
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

// ---- geometry ----------------------------------------------------------------

pub type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let n = dot(a, a).sqrt();
    if n > 0.0 {
        scale(a, 1.0 / n)
    } else {
        a
    }
}

/// A polygon ready to paint, in screen points. `shade` 0..1 multiplies the
/// face colour; `alpha` 0..1 is its opacity; `shadow` marks shadow polygons
/// (painted black).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Poly {
    pub pts: [(f32, f32); 4],
    pub shade: f32,
    pub alpha: f32,
    pub shadow: bool,
    /// Distance from the eye, for back-to-front order (larger = drawn first).
    pub depth: f32,
}

/// Seven-segment encoding, bits a..g = top, top-right, bottom-right, bottom,
/// bottom-left, top-left, middle.
pub fn segments(c: char) -> u8 {
    match c {
        '0' => 0b0111111,
        '1' => 0b0000110,
        '2' => 0b1011011,
        '3' => 0b1001111,
        '4' => 0b1100110,
        '5' => 0b1101101,
        '6' => 0b1111101,
        '7' => 0b0000111,
        '8' => 0b1111111,
        '9' => 0b1101111,
        _ => 0,
    }
}

/// Digit cell, in units of the clock's height (1.0).
const CELL_W: f32 = 0.56;
const GAP: f32 = 0.14;
const COLON_W: f32 = 0.22;
const T: f32 = 0.11; // segment thickness

/// Axis-aligned boxes making up `text` ("HH:MM") in clock-local units:
/// x to the right from 0, y DOWN from 0 (top) to 1 (bottom), as rectangles
/// (x0, y0, x1, y1). Anything other than digits and ':' is a space. Returns
/// the rectangles and the total width.
pub fn glyph_rects(text: &str) -> (Vec<[f32; 4]>, f32) {
    let mut out = Vec::new();
    let mut x = 0.0;
    for (i, c) in text.chars().enumerate() {
        if i > 0 {
            x += GAP;
        }
        if c == ':' {
            let s = T * 1.3;
            let cx = x + COLON_W / 2.0;
            out.push([cx - s / 2.0, 0.30 - s / 2.0, cx + s / 2.0, 0.30 + s / 2.0]);
            out.push([cx - s / 2.0, 0.70 - s / 2.0, cx + s / 2.0, 0.70 + s / 2.0]);
            x += COLON_W;
            continue;
        }
        let bits = segments(c);
        let (w, h) = (CELL_W, 1.0);
        let segs: [[f32; 4]; 7] = [
            [T, 0.0, w - T, T],                       // a top
            [w - T, T, w, h / 2.0 - T / 2.0],         // b top-right
            [w - T, h / 2.0 + T / 2.0, w, h - T],     // c bottom-right
            [T, h - T, w - T, h],                     // d bottom
            [0.0, h / 2.0 + T / 2.0, T, h - T],       // e bottom-left
            [0.0, T, T, h / 2.0 - T / 2.0],           // f top-left
            [T, h / 2.0 - T / 2.0, w - T, h / 2.0 + T / 2.0], // g middle
        ];
        for (b, r) in segs.iter().enumerate() {
            if bits & (1 << b) != 0 {
                out.push([x + r[0], r[1], x + r[2], r[3]]);
            }
        }
        x += CELL_W;
    }
    (out, x)
}

/// The viewer, for a phone flat on a table: past the bottom edge, above the
/// glass. Screen frame: x right, y down, z out of the glass, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    pub pos: V3,
}

impl Eye {
    /// A seated viewer: `elevation_deg` above the table, `dist` points from
    /// the screen's bottom-centre.
    pub fn seated(screen: (f32, f32), elevation_deg: f32, dist: f32) -> Eye {
        let e = elevation_deg.to_radians();
        Eye { pos: [screen.0 / 2.0, screen.1 + dist * e.cos(), dist * e.sin()] }
    }

    /// Where `p` must be drawn on the glass (z = 0) to be seen at `p` from
    /// this eye. `None` when `p` is at or above the eye (not drawable).
    pub fn project(&self, p: V3) -> Option<(f32, f32)> {
        let e = self.pos;
        let dz = e[2] - p[2];
        if dz <= 1e-3 {
            return None;
        }
        let t = e[2] / dz;
        let s = add(e, scale(sub(p, e), t));
        Some((s[0], s[1]))
    }
}

/// The light: from behind and above the viewer, slightly left.
const LIGHT: V3 = [-0.35, 0.55, 0.76];

/// One box's visible faces (and, when asked, its shadow on the glass).
fn push_box(out: &mut Vec<Poly>, eye: &Eye, corners: [V3; 8], alpha: f32, shadow: bool) {
    push_box_with(out, eye, corners, alpha, shadow, true)
}

/// Painter's order is decided PER BOX, not per face: every face of a box gets
/// the box centre's depth, and the faces go out walls-first, far-to-near, cap
/// last (`build` sorts stably, so that order survives). Sorting faces
/// individually let a raised cap sort behind its own walls, which then
/// painted over it (the extruded digits read as smears).
fn push_box_with(out: &mut Vec<Poly>, eye: &Eye, corners: [V3; 8], alpha: f32, shadow: bool, walls: bool) {
    // Corner order: bottom face (z low) 0..3, top face 4..7.
    const FACES: [[usize; 4]; 6] = [[0, 1, 2, 3], [4, 7, 6, 5], [0, 4, 5, 1], [1, 5, 6, 2], [2, 6, 7, 3], [3, 7, 4, 0]];
    // Outward normals from the box's own edges (u along x, v along y, w along
    // the extrusion). A centroid test fails for a box extruded ~0: the
    // bottom face's centre coincides with the box's and its normal flips.
    let (u, v, w) = (norm(sub(corners[1], corners[0])), norm(sub(corners[3], corners[0])), norm(sub(corners[4], corners[0])));
    let outward = [scale(w, -1.0), w, scale(v, -1.0), u, v, scale(u, -1.0)];
    let light = norm(LIGHT);
    let centre = scale(corners.iter().fold([0.0; 3], |a, c| add(a, *c)), 1.0 / 8.0);
    let cd = sub(centre, eye.pos);
    let box_depth = dot(cd, cd).sqrt();
    let mut faces: Vec<(f32, bool, Poly)> = Vec::with_capacity(3);
    for (fi, f) in FACES.iter().enumerate() {
        if !walls && fi >= 2 {
            continue;
        }
        let q = [corners[f[0]], corners[f[1]], corners[f[2]], corners[f[3]]];
        let fc = scale(add(add(q[0], q[1]), add(q[2], q[3])), 0.25);
        let n = outward[fi];
        if dot(n, sub(eye.pos, fc)) <= 0.0 {
            continue; // facing away
        }
        let (Some(p0), Some(p1), Some(p2), Some(p3)) = (eye.project(q[0]), eye.project(q[1]), eye.project(q[2]), eye.project(q[3])) else {
            continue;
        };
        // Edge-on faces project to slivers: nothing to see, still triangles to fill.
        // (2 pt²: a flat clock's walls project to 1-2 pt² here, as the
        // anamorphic stretch magnifies them; nothing that small is visible.)
        if quad_area([p0, p1, p2, p3]) < 2.0 {
            continue;
        }
        let shade = 0.35 + 0.65 * dot(n, light).max(0.0);
        let d = sub(fc, eye.pos);
        faces.push((dot(d, d).sqrt(), fi == 1, Poly { pts: [p0, p1, p2, p3], shade, alpha, shadow: false, depth: box_depth }));
    }
    // Walls far-to-near, then the cap. Every face carries the BOX's depth, so
    // `build`'s stable sort orders boxes and keeps this order inside each.
    faces.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)));
    out.extend(faces.into_iter().map(|f| f.2));
    if shadow {
        // Cast along -LIGHT onto z = 0: the top face's footprint.
        let l = norm(LIGHT);
        let cast = |p: V3| -> (f32, f32) {
            let t = p[2] / l[2];
            (p[0] - l[0] * t, p[1] - l[1] * t)
        };
        let top = [corners[4], corners[5], corners[6], corners[7]];
        let bot = [corners[0], corners[1], corners[2], corners[3]];
        // The shadow of a box is the hull of both faces' casts; two quads cover it well enough.
        for face in [top, bot] {
            let pts = [cast(face[0]), cast(face[1]), cast(face[2]), cast(face[3])];
            out.push(Poly { pts, shade: 0.0, alpha: 0.16 * alpha, shadow: true, depth: f32::INFINITY });
        }
        let _ = eye;
    }
}

/// Absolute area of a quad (shoelace), in square points.
pub fn quad_area(p: [(f32, f32); 4]) -> f32 {
    let mut a = 0.0;
    for i in 0..4 {
        let (x0, y0) = p[i];
        let (x1, y1) = p[(i + 1) % 4];
        a += x0 * y1 - x1 * y0;
    }
    (a / 2.0).abs()
}

/// A box from a rectangle on a plane: `origin` + u*x + v*y + w*z.
fn box_on(origin: V3, u: V3, v: V3, w: V3, r: [f32; 4], z0: f32, z1: f32) -> [V3; 8] {
    let at = |x: f32, y: f32, z: f32| add(origin, add(add(scale(u, x), scale(v, y)), scale(w, z)));
    [
        at(r[0], r[1], z0),
        at(r[2], r[1], z0),
        at(r[2], r[3], z0),
        at(r[0], r[3], z0),
        at(r[0], r[1], z1),
        at(r[2], r[1], z1),
        at(r[2], r[3], z1),
        at(r[0], r[3], z1),
    ]
}

/// Parameters of one clock scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    pub style: Style,
    /// 0 = flat 2D digits, 1 = full effect.
    pub amount: f32,
    /// Seconds, for the animating styles.
    pub t: f32,
    /// Screen size in points.
    pub screen: (f32, f32),
    /// Where the clock's base centre sits on the glass, in points.
    pub anchor: (f32, f32),
    /// Digit height in points.
    pub height: f32,
}

/// Polygons for `text` in `scene`, back to front.
pub fn build(text: &str, scene: &Scene, eye: &Eye) -> Vec<Poly> {
    let (rects, width) = glyph_rects(text);
    let h = scene.height;
    let k = scene.amount.clamp(0.0, 1.0);
    let (ax, ay) = scene.anchor;
    let left = ax - width * h / 2.0;
    let mut out = Vec::with_capacity(rects.len() * 8);
    let x_axis = [h, 0.0, 0.0];
    match scene.style {
        Style::Standing => {
            // Tipped up about the base line by k * 90°: flat on the glass at 0,
            // upright at 1. The digits' "down" goes toward the glass.
            let a = k * std::f32::consts::FRAC_PI_2;
            // Upright digits read top-to-bottom going DOWN toward the glass:
            // y (down the digit) maps to +y on screen when flat, to -z when upright.
            let v = [0.0, h * a.cos(), -h * a.sin()];
            let origin = [left, ay - h * a.cos(), h * a.sin()];
            let depth_dir = [0.0, -a.sin(), -a.cos()]; // slab thickness, toward the back
            let thick = 0.16 * h;
            for r in &rects {
                let c = box_on(origin, x_axis, v, scale(depth_dir, thick), *r, 0.0, 1.0);
                push_box(&mut out, eye, c, 1.0, k > 0.05);
            }
        }
        Style::Extruded => {
            let origin = [left, ay - h / 2.0, 0.0];
            // Shallow: seen from a low eye, the walls stretch a long way up
            // the glass; much deeper and they bury the digits.
            let z = [0.0, 0.0, 0.12 * h * k.max(0.001)];
            for r in &rects {
                push_box(&mut out, eye, box_on(origin, x_axis, [0.0, h, 0.0], z, *r, 0.0, 1.0), 1.0, false);
            }
        }
        Style::Hologram => {
            // Slices hovering above the glass, the stack bobbing a few points.
            let bob = (scene.t * 1.3).sin() * 0.04 * h;
            let slices = 5;
            for i in 0..slices {
                let z0 = (0.2 * h + i as f32 * 0.035 * h + bob) * k;
                let origin = [left, ay - h / 2.0, z0];
                let alpha = 0.18 + 0.5 * (i as f32 / (slices - 1) as f32);
                for r in &rects {
                    let c = box_on(origin, x_axis, [0.0, h, 0.0], [0.0, 0.0, 0.01 * h], *r, 0.0, 1.0);
                    // Caps only: a slice's walls read as stripes, not glass.
                    push_box_with(&mut out, eye, c, alpha, false, false);
                }
            }
        }
        Style::Carousel => {
            // Each glyph stands on a ring of radius R, facing outward; the ring
            // turns slowly about the vertical (z) axis through the anchor.
            let n = text.chars().count().max(1) as f32;
            let radius = width * h / 1.2; // the text spans 1.2 rad of the ring
            // A turntable SWAY (±17°), not a spin: a full turn shows the digits
            // edge-on and backwards half the time, and a clock must stay readable.
            let spin = (scene.t * 0.35).sin() * 0.3;
            let mut x = 0.0;
            let upright = k;
            for (i, c) in text.chars().enumerate() {
                if i > 0 {
                    x += GAP;
                }
                let w = if c == ':' { COLON_W } else { CELL_W };
                let (sub_rects, _) = glyph_rects(&c.to_string());
                // Angle of this glyph's centre on the ring (the text spans ~200°).
                let frac = (x + w / 2.0) / width.max(1e-3) - 0.5;
                let theta = frac * 1.2 + spin;
                let _ = n;
                let centre = [ax + radius * theta.sin(), ay + radius * theta.cos() * 0.6, 0.0];
                let tangent = [theta.cos(), -theta.sin() * 0.6, 0.0];
                let tangent = norm(tangent);
                let u = scale(tangent, h);
                let v = [0.0, h * (1.0 - upright) * 0.0 + 0.0, -h * upright.max(0.02)];
                let origin = add(centre, add(scale(u, -w / 2.0), scale(v, -1.0)));
                let normal = norm(cross(tangent, [0.0, 0.0, 1.0]));
                for r in &sub_rects {
                    let c3 = box_on(origin, u, v, scale(normal, 0.12 * h), *r, 0.0, 1.0);
                    push_box(&mut out, eye, c3, 1.0, upright > 0.05);
                }
                x += w;
            }
        }
    }
    out.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f32, f32) = (432.0, 936.0);

    fn eye() -> Eye {
        Eye::seated(SCREEN, 40.0, 900.0)
    }

    #[test]
    fn tokens_round_trip() {
        for t in Trigger::ALL {
            assert_eq!(Trigger::from_token(t.token()), Some(t));
        }
        for s in Style::ALL {
            assert_eq!(Style::from_token(s.token()), Some(s));
        }
        assert_eq!(Style::from_token("nope"), None);
    }

    #[test]
    fn a_point_on_the_glass_projects_to_itself_and_raised_points_move_away_from_the_eye() {
        let e = eye();
        let (x, y) = e.project([100.0, 300.0, 0.0]).unwrap();
        assert!((x - 100.0).abs() < 1e-3 && (y - 300.0).abs() < 1e-3);
        // The eye is past the BOTTOM edge, so a point raised off the glass is
        // drawn further UP the screen (smaller y): that is what makes it look tall.
        let (_, y_up) = e.project([100.0, 300.0, 50.0]).unwrap();
        assert!(y_up < 300.0, "{y_up}");
        assert!(e.project([100.0, 300.0, e.pos[2] + 1.0]).is_none(), "above the eye: not drawable");
    }

    #[test]
    fn seven_segments_draw_the_expected_counts() {
        let count = |s: &str| glyph_rects(s).0.len();
        assert_eq!(count("8"), 7);
        assert_eq!(count("1"), 2);
        assert_eq!(count("0"), 6);
        assert_eq!(count(":"), 2);
        assert_eq!(count("12:08"), 2 + 5 + 2 + 6 + 7);
        let (_, w) = glyph_rects("12:34");
        assert!((w - (4.0 * CELL_W + COLON_W + 4.0 * GAP)).abs() < 1e-5);
    }

    fn scene(style: Style, amount: f32) -> Scene {
        Scene { style, amount, t: 0.0, screen: SCREEN, anchor: (SCREEN.0 / 2.0, SCREEN.1 * 0.3), height: 90.0 }
    }

    #[test]
    fn every_style_builds_finite_polygons_sorted_back_to_front() {
        for style in Style::ALL {
            for amount in [0.0, 0.5, 1.0] {
                let p = build("12:34", &scene(style, amount), &eye());
                assert!(!p.is_empty(), "{style:?} at {amount}");
                assert!(p.iter().all(|q| q.pts.iter().all(|(x, y)| x.is_finite() && y.is_finite())), "{style:?}");
                assert!(p.windows(2).all(|w| w[0].depth >= w[1].depth), "{style:?} not back-to-front");
                assert!(p.iter().all(|q| (0.0..=1.0).contains(&q.alpha) && (0.0..=1.0).contains(&q.shade)));
            }
        }
    }

    #[test]
    fn standing_digits_reach_further_up_the_screen_than_flat_ones() {
        let top = |amount| {
            build("88:88", &scene(Style::Standing, amount), &eye())
                .iter()
                .filter(|p| !p.shadow)
                .flat_map(|p| p.pts)
                .map(|(_, y)| y)
                .fold(f32::INFINITY, f32::min)
        };
        let (flat, upright) = (top(0.0), top(1.0));
        assert!(upright < flat - 20.0, "upright top {upright} vs flat top {flat}");
    }

    #[test]
    fn standing_casts_a_shadow_only_once_it_has_stood_up() {
        let shadows = |a| build("12:34", &scene(Style::Standing, a), &eye()).iter().filter(|p| p.shadow).count();
        assert_eq!(shadows(0.0), 0);
        assert!(shadows(1.0) > 0);
        // Shadows are drawn first (behind everything).
        let p = build("12:34", &scene(Style::Standing, 1.0), &eye());
        let first_solid = p.iter().position(|q| !q.shadow).unwrap();
        assert!(p[first_solid..].iter().all(|q| !q.shadow));
    }

    #[test]
    fn a_raised_cap_is_painted_after_its_own_walls() {
        // Faces of one box share a depth; the cap (the brightest-lit face
        // here, facing the light above) comes last in its run.
        let p = build("1", &scene(Style::Extruded, 1.0), &eye());
        for run in p.chunk_by(|a, b| a.depth == b.depth) {
            if run.len() > 1 {
                let cap_area = quad_area(run.last().unwrap().pts);
                assert!(cap_area > 50.0, "the last face of a box is its cap: {cap_area}");
            }
        }
    }

    #[test]
    fn extruded_shows_side_walls_only_when_raised() {
        let n = |a| build("8", &scene(Style::Extruded, a), &eye()).len();
        // Flat: only the top faces are seen (one per segment); raised: walls too.
        assert_eq!(n(0.0), 7);
        assert!(n(1.0) > 7);
    }

    #[test]
    fn the_hologram_moves_and_the_still_styles_do_not() {
        let at = |style, t| build("12:34", &Scene { t, ..scene(style, 1.0) }, &eye());
        assert_ne!(at(Style::Hologram, 0.0), at(Style::Hologram, 1.0));
        assert_ne!(at(Style::Carousel, 0.0), at(Style::Carousel, 1.0));
        assert_eq!(at(Style::Standing, 0.0), at(Style::Standing, 5.0));
        assert!(Style::Hologram.animates() && !Style::Standing.animates() && Style::Standing.frame_ms().is_none());
    }

    #[test]
    fn tilt_reads_face_up_as_zero_and_face_down_as_180() {
        assert!(tilt_from_up_deg([0.0, 0.0, 9.8]).unwrap() < 0.1);
        assert!((tilt_from_up_deg([0.0, 9.8, 0.0]).unwrap() - 90.0).abs() < 0.1, "upright in the hand");
        assert!(tilt_from_up_deg([0.0, 0.0, -9.8]).unwrap() > 179.0);
        assert_eq!(tilt_from_up_deg([0.0, 0.0, 0.5]), None, "too weak to trust");
    }

    #[test]
    fn the_accelerometer_enters_flat_after_a_hold_and_leaves_quickly_with_hysteresis() {
        let mut d = FlatDetector::default();
        let tilted = |deg: f32| {
            let r = deg.to_radians();
            [0.0, 9.8 * r.sin(), 9.8 * r.cos()]
        };
        d.on_accel(tilted(5.0), 0);
        assert!(!d.is_flat(), "not before the hold");
        d.on_accel(tilted(5.0), FLAT_HOLD_MS);
        assert!(d.is_flat());
        d.on_accel(tilted(16.0), FLAT_HOLD_MS + 100);
        d.on_accel(tilted(16.0), FLAT_HOLD_MS + 400);
        assert!(d.is_flat(), "16° is inside the hysteresis band");
        d.on_accel(tilted(35.0), FLAT_HOLD_MS + 500);
        d.on_accel(tilted(35.0), FLAT_HOLD_MS + 700);
        assert!(!d.is_flat(), "picked up");
        // Face down is not flat (the clock is not visible).
        let mut d = FlatDetector::default();
        d.on_accel([0.0, 0.0, -9.8], 0);
        d.on_accel([0.0, 0.0, -9.8], 2000);
        assert!(!d.is_flat());
        assert_eq!(d.source(2000), FlatSource::Accelerometer);
    }

    #[test]
    fn the_orientation_string_falls_back_and_yields_to_a_live_accelerometer() {
        let mut d = FlatDetector::default();
        assert_eq!(d.source(0), FlatSource::None);
        d.on_orientation("undefined", 0);
        d.tick(16.0, 1000);
        assert!(!d.is_flat(), "mid-rotation reads undefined briefly");
        d.tick(16.0, UNDEFINED_HOLD_MS);
        assert!(d.is_flat(), "held: lying flat");
        assert_eq!(d.source(UNDEFINED_HOLD_MS), FlatSource::Orientation);
        d.on_orientation("normal", UNDEFINED_HOLD_MS + 10);
        assert!(!d.is_flat());
        // A fresh accelerometer outranks the string.
        let mut d = FlatDetector::default();
        d.on_accel([0.0, 9.8, 0.0], 0);
        d.on_orientation("undefined", 10);
        d.tick(16.0, 1600);
        assert!(!d.is_flat());
    }

    #[test]
    fn the_tip_in_eases_over_the_transition_and_off_means_none() {
        let mut d = FlatDetector::default();
        d.on_orientation("undefined", 0);
        d.tick(16.0, UNDEFINED_HOLD_MS);
        assert!(d.is_transitioning());
        let mut t = UNDEFINED_HOLD_MS;
        let mut n = 0;
        while d.is_transitioning() {
            t += 16;
            d.tick(16.0, t);
            n += 1;
            assert!(n < 100);
        }
        assert!((n as f32 * 16.0 - TRANSITION_MS).abs() < 40.0);
        assert_eq!(amount(Trigger::Auto, &d), 1.0);
        assert_eq!(amount(Trigger::Off, &d), 0.0);
        assert_eq!(amount(Trigger::Always, &FlatDetector::default()), 1.0);
    }
}
