//! Stroke data. World-space f64 points so strokes survive camera precision;
//! pressure is normalized 0..1 (0.5 = "no pressure info", so finger/mouse
//! strokes render mid-weight instead of hairline).

use crate::{RectD, Vec2d};

/// One sample on a stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkPoint {
    pub pos: Vec2d,
    /// Normalized 0..1. Sources without pressure report [`InkPoint::NO_PRESSURE`].
    pub pressure: f32,
    /// Milliseconds on the capture clock, relative to the stroke's first point.
    pub t_ms: f64,
}
impl InkPoint {
    pub const NO_PRESSURE: f32 = 0.5;
    pub fn new(pos: Vec2d, pressure: f32, t_ms: f64) -> Self {
        InkPoint { pos, pressure, t_ms }
    }
    pub fn xy(x: f64, y: f64) -> Self {
        InkPoint::new(Vec2d::new(x, y), Self::NO_PRESSURE, 0.0)
    }
}

/// Which physical tool made the stroke — kept on the stroke because it matters
/// downstream (dataset filtering: pen strokes are far better handwriting data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Pen,
    Finger,
    Mouse,
}
impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Pen => "pen",
            ToolKind::Finger => "finger",
            ToolKind::Mouse => "mouse",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pen" => Some(ToolKind::Pen),
            "finger" => Some(ToolKind::Finger),
            "mouse" => Some(ToolKind::Mouse),
            _ => None,
        }
    }
}

/// Visual style. Width is in *world units* so strokes zoom with the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    pub width: f64,
    /// RGBA, straight (unpremultiplied).
    pub color: [u8; 4],
    pub tool: ToolKind,
}
impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle {
            width: 3.0,
            color: [20, 20, 24, 255],
            tool: ToolKind::Finger,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StrokeId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub id: StrokeId,
    pub style: StrokeStyle,
    pub points: Vec<InkPoint>,
}

impl Stroke {
    pub fn new(id: StrokeId, style: StrokeStyle) -> Self {
        Stroke {
            id,
            style,
            points: Vec::new(),
        }
    }

    /// Geometry bounds, *not* inflated by width — callers expand by
    /// `style.width * 0.5` where visual bounds matter (index, culling).
    pub fn bbox(&self) -> RectD {
        let mut r = RectD::EMPTY;
        for p in &self.points {
            r.include(p.pos);
        }
        r
    }

    /// Visual bounds: geometry expanded by half the stroke width.
    pub fn visual_bbox(&self) -> RectD {
        let b = self.bbox();
        if b.is_empty() {
            b
        } else {
            b.expand(self.style.width * 0.5)
        }
    }

    /// Polyline arc length.
    pub fn path_len(&self) -> f64 {
        self.points
            .windows(2)
            .map(|w| w[0].pos.dist(w[1].pos))
            .sum()
    }

    /// Min distance from `p` to the polyline (segment-accurate).
    pub fn dist_to(&self, p: Vec2d) -> f64 {
        if self.points.is_empty() {
            return f64::INFINITY;
        }
        if self.points.len() == 1 {
            return self.points[0].pos.dist(p);
        }
        let mut best = f64::INFINITY;
        for w in self.points.windows(2) {
            best = best.min(seg_dist(w[0].pos, w[1].pos, p));
        }
        best
    }
}

/// Distance from point `p` to segment `ab`.
pub(crate) fn seg_dist(a: Vec2d, b: Vec2d, p: Vec2d) -> f64 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 <= f64::EPSILON {
        return a.dist(p);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
    Vec2d::new(a.x + ab.x * t, a.y + ab.y * t).dist(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_and_len() {
        let mut s = Stroke::new(StrokeId(1), StrokeStyle::default());
        s.points.push(InkPoint::xy(0.0, 0.0));
        s.points.push(InkPoint::xy(3.0, 4.0));
        let b = s.bbox();
        assert_eq!(b.min, Vec2d::new(0.0, 0.0));
        assert_eq!(b.max, Vec2d::new(3.0, 4.0));
        assert!((s.path_len() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn seg_distance() {
        let d = seg_dist(Vec2d::new(0.0, 0.0), Vec2d::new(10.0, 0.0), Vec2d::new(5.0, 3.0));
        assert!((d - 3.0).abs() < 1e-9);
        // beyond the endpoint clamps to endpoint distance
        let d2 = seg_dist(Vec2d::new(0.0, 0.0), Vec2d::new(10.0, 0.0), Vec2d::new(14.0, 3.0));
        assert!((d2 - 5.0).abs() < 1e-9);
    }
}
