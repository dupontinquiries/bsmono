//! Sketch → structure: classify strokes into boxes/lines/scribbles, connect
//! open strokes whose endpoints land on two different boxes into directed
//! edges, and emit **niftyplot** graph source that `nifty-core::niftyplot::
//! parse` accepts. This is the voiceterm→logscope import path: draw boxes and
//! arrows in the hand-drawing panel, export, and logscope renders the graph.
//!
//! Deliberately heuristic and geometric — no ML here. The ML path (handwriting
//! → *text labels* on the recognized nodes) plugs in later via the dataset /
//! finetuning pipeline; the seam is [`GraphSketch`]: recognition produces it,
//! labeling mutates it, [`GraphSketch::to_niftyplot`] serializes it.
//!
//! Heuristics (all scale-free — thresholds are relative to stroke size):
//! * closed if endpoint gap < 20% of path length;
//! * closed + RDP-corner-count 3..=6 + high bbox fill → Rect-ish box;
//!   closed + smoother outline + fill ≈ π/4 → Ellipse box; else Blob box
//!   (all three become niftyplot nodes, with the shape hint);
//! * open + both endpoints within snap radius of *different* boxes → edge
//!   (direction = draw direction);
//! * anything else → Scribble (annotation ink; ignored by export).

use crate::filter::{rdp, resample};
use crate::stroke::Stroke;
use crate::{RectD, Vec2d};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeClass {
    /// Closed, cornered: a box.
    Rect,
    /// Closed, smooth, elliptical fill ratio.
    Ellipse,
    /// Closed but neither: still a container.
    Blob,
    /// Open, essentially straight.
    Line,
    /// Open, not straight (freehand / handwriting).
    Scribble,
    /// Too small / degenerate to classify.
    Dot,
}

/// Classification of one stroke, with the geometry the graph builder needs.
#[derive(Clone, Debug)]
pub struct ClassifiedStroke {
    pub class: ShapeClass,
    pub bbox: RectD,
    pub start: Vec2d,
    pub end: Vec2d,
}

/// Classify a single stroke. Scale-free; works in world units.
pub fn classify(stroke: &Stroke) -> ClassifiedStroke {
    let bbox = stroke.bbox();
    let diag = if bbox.is_empty() {
        0.0
    } else {
        bbox.min.dist(bbox.max)
    };
    let n = stroke.points.len();
    let (start, end) = if n == 0 {
        (Vec2d::ZERO, Vec2d::ZERO)
    } else {
        (stroke.points[0].pos, stroke.points[n - 1].pos)
    };
    let path = stroke.path_len();

    if n < 3 || diag < 1e-9 || path < 1e-9 {
        return ClassifiedStroke {
            class: ShapeClass::Dot,
            bbox,
            start,
            end,
        };
    }

    // Even out sensor cadence before geometry checks.
    let even = resample(&stroke.points, (path / 128.0).max(diag / 256.0));

    // Closed = endpoints meet, judged against BOTH path length and shape
    // size. Path-relative alone false-positives on long oscillating
    // scribbles (huge path, endpoints coincidentally near); size-relative
    // alone false-positives on tiny strokes.
    let gap = start.dist(end);
    let closed = gap < path * 0.20 && gap < diag * 0.30;

    if closed {
        // Corner count via RDP with epsilon relative to size.
        let simplified = rdp(&even, diag * 0.05);
        // For a closed stroke, first and last simplified points nearly
        // coincide; count distinct vertices.
        let mut verts = simplified.len();
        if verts >= 2 && simplified[0].pos.dist(simplified[verts - 1].pos) < diag * 0.1 {
            verts -= 1;
        }
        let fill = polygon_area(&even) / (bbox.width() * bbox.height()).max(1e-12);
        let class = if (3..=6).contains(&verts) && fill > 0.72 {
            ShapeClass::Rect
        } else if fill > 0.55 && fill < 0.95 && verts > 6 {
            ShapeClass::Ellipse
        } else {
            ShapeClass::Blob
        };
        return ClassifiedStroke {
            class,
            bbox,
            start,
            end,
        };
    }

    // Open: straight if max deviation from the chord is small relative to length.
    let chord = start.dist(end).max(1e-12);
    let max_dev = even
        .iter()
        .map(|p| crate::stroke::seg_dist(start, end, p.pos))
        .fold(0.0f64, f64::max);
    let class = if chord > path * 0.80 && max_dev < chord * 0.12 {
        ShapeClass::Line
    } else {
        ShapeClass::Scribble
    };
    ClassifiedStroke {
        class,
        bbox,
        start,
        end,
    }
}

/// Shoelace area of the point loop (absolute).
fn polygon_area(pts: &[crate::stroke::InkPoint]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..pts.len() {
        let p = pts[i].pos;
        let q = pts[(i + 1) % pts.len()].pos;
        a += p.x * q.y - q.x * p.y;
    }
    (a * 0.5).abs()
}

/// A recognized node (one closed stroke).
#[derive(Clone, Debug)]
pub struct SketchNode {
    /// Deterministic id: `n1`, `n2`, … in reading order (top-left first).
    pub id: String,
    /// None until a labeling pass (manual or ML handwriting→text) fills it.
    pub label: Option<String>,
    pub class: ShapeClass,
    pub bbox: RectD,
}

/// A recognized directed edge (one open stroke between two nodes).
#[derive(Clone, Debug)]
pub struct SketchEdge {
    pub from: String,
    pub to: String,
}

/// The structured result: mutate labels, then export.
#[derive(Clone, Debug, Default)]
pub struct GraphSketch {
    pub nodes: Vec<SketchNode>,
    pub edges: Vec<SketchEdge>,
    /// Strokes that didn't become structure (annotation ink / handwriting) —
    /// indices into the input slice, for the labeling pass to consume.
    pub leftover: Vec<usize>,
}

/// Build a graph from strokes. `snap_frac` scales the endpoint-snap radius
/// relative to the mean node size (0.35 default is forgiving for finger
/// sketches; tighten for pen).
pub fn recognize_graph(strokes: &[Stroke], snap_frac: f64) -> GraphSketch {
    let classified: Vec<ClassifiedStroke> = strokes.iter().map(classify).collect();

    // Nodes: closed shapes, ordered top-left → bottom-right for stable ids.
    let mut node_idx: Vec<usize> = classified
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            matches!(
                c.class,
                ShapeClass::Rect | ShapeClass::Ellipse | ShapeClass::Blob
            )
        })
        .map(|(i, _)| i)
        .collect();
    node_idx.sort_by(|&a, &b| {
        let ca = classified[a].bbox.center();
        let cb = classified[b].bbox.center();
        (ca.y, ca.x)
            .partial_cmp(&(cb.y, cb.x))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let nodes: Vec<SketchNode> = node_idx
        .iter()
        .enumerate()
        .map(|(k, &i)| SketchNode {
            id: format!("n{}", k + 1),
            label: None,
            class: classified[i].class,
            bbox: classified[i].bbox,
        })
        .collect();

    let mean_size = if nodes.is_empty() {
        0.0
    } else {
        nodes
            .iter()
            .map(|n| n.bbox.min.dist(n.bbox.max))
            .sum::<f64>()
            / nodes.len() as f64
    };
    let snap = mean_size * snap_frac;

    // Which node does a point attach to? Inside the bbox always counts;
    // otherwise nearest bbox-expanded-by-snap.
    let attach = |p: Vec2d| -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (k, node) in nodes.iter().enumerate() {
            let d = if node.bbox.contains(p) {
                0.0
            } else if node.bbox.expand(snap).contains(p) {
                // distance to bbox edge, roughly
                let c = node.bbox.center();
                p.dist(c)
            } else {
                continue;
            };
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((k, d));
            }
        }
        best.map(|(k, _)| k)
    };

    let mut edges = Vec::new();
    let mut leftover = Vec::new();
    for (i, c) in classified.iter().enumerate() {
        if node_idx.contains(&i) {
            continue;
        }
        let is_connector = matches!(c.class, ShapeClass::Line | ShapeClass::Scribble);
        if is_connector {
            if let (Some(a), Some(b)) = (attach(c.start), attach(c.end)) {
                if a != b {
                    edges.push(SketchEdge {
                        from: nodes[a].id.clone(),
                        to: nodes[b].id.clone(),
                    });
                    continue;
                }
            }
        }
        leftover.push(i);
    }

    GraphSketch {
        nodes,
        edges,
        leftover,
    }
}

impl GraphSketch {
    /// Emit niftyplot graph source (`%kind: graph`, `id shape: "label"`,
    /// `a -> b`) that `nifty_core::niftyplot::parse` round-trips.
    pub fn to_niftyplot(&self) -> String {
        let mut out = String::from("%kind: graph\n");
        for n in &self.nodes {
            let shape = match n.class {
                ShapeClass::Ellipse => " as circle",
                _ => "", // Rect is the default shape; Blob maps to rect too
            };
            match &n.label {
                Some(l) => {
                    // niftyplot's grammar has no escape sequences: outer
                    // quotes are stripped, inner quotes pass through raw.
                    // Only newlines are impossible (line-based format).
                    let flat = l.replace('\n', " ");
                    out.push_str(&format!("{}{}: \"{}\"\n", n.id, shape, flat));
                }
                None => {
                    if !shape.is_empty() {
                        out.push_str(&format!("{}{}: \"{}\"\n", n.id, shape, n.id));
                    } else {
                        out.push_str(&format!("{}\n", n.id));
                    }
                }
            }
        }
        for e in &self.edges {
            out.push_str(&format!("{} -> {}\n", e.from, e.to));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{InkPoint, StrokeId, StrokeStyle};

    fn stroke_from(pts: &[(f64, f64)]) -> Stroke {
        let mut s = Stroke::new(StrokeId(0), StrokeStyle::default());
        s.points = pts.iter().map(|&(x, y)| InkPoint::xy(x, y)).collect();
        s
    }

    /// A hand-drawn-ish rectangle around (cx,cy) with size w×h and jitter.
    fn wobbly_rect(cx: f64, cy: f64, w: f64, h: f64) -> Stroke {
        let mut pts = Vec::new();
        let (x0, y0, x1, y1) = (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
        let jitter = |i: usize| ((i as f64 * 2.7).sin()) * (w.min(h) * 0.015);
        let n = 12;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            pts.push((x0 + (x1 - x0) * t, y0 + jitter(i)));
        }
        for i in 0..=n {
            let t = i as f64 / n as f64;
            pts.push((x1 + jitter(i + 20), y0 + (y1 - y0) * t));
        }
        for i in 0..=n {
            let t = i as f64 / n as f64;
            pts.push((x1 - (x1 - x0) * t, y1 + jitter(i + 40)));
        }
        for i in 0..=n {
            let t = i as f64 / n as f64;
            pts.push((x0 + jitter(i + 60), y1 - (y1 - y0) * t));
        }
        stroke_from(&pts)
    }

    fn wobbly_circle(cx: f64, cy: f64, r: f64) -> Stroke {
        let pts: Vec<(f64, f64)> = (0..=72)
            .map(|i| {
                let a = i as f64 / 72.0 * std::f64::consts::TAU;
                let rr = r * (1.0 + 0.03 * (a * 5.0).sin());
                (cx + rr * a.cos(), cy + rr * a.sin())
            })
            .collect();
        stroke_from(&pts)
    }

    #[test]
    fn classifies_basic_shapes() {
        assert_eq!(classify(&wobbly_rect(0.0, 0.0, 100.0, 60.0)).class, ShapeClass::Rect);
        assert_eq!(classify(&wobbly_circle(0.0, 0.0, 50.0)).class, ShapeClass::Ellipse);
        let line = stroke_from(&[(0.0, 0.0), (50.0, 1.0), (100.0, 0.5), (150.0, 0.0)]);
        assert_eq!(classify(&line).class, ShapeClass::Line);
        let squiggle: Vec<(f64, f64)> = (0..60)
            .map(|i| (i as f64 * 2.0, (i as f64 * 0.7).sin() * 30.0))
            .collect();
        assert_eq!(classify(&stroke_from(&squiggle)).class, ShapeClass::Scribble);
    }

    #[test]
    fn two_boxes_and_a_connector_make_a_graph() {
        let a = wobbly_rect(0.0, 0.0, 100.0, 60.0);
        let b = wobbly_rect(300.0, 0.0, 100.0, 60.0);
        // arrow drawn from inside A's right edge to inside B's left edge
        let conn = stroke_from(&[(45.0, 0.0), (120.0, 3.0), (200.0, -2.0), (258.0, 0.0)]);
        let g = recognize_graph(&[a, b, conn], 0.35);
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        // reading order: A is left of B at same y → n1 is A
        assert_eq!(g.edges[0].from, "n1");
        assert_eq!(g.edges[0].to, "n2");
        assert!(g.leftover.is_empty());
    }

    #[test]
    fn dangling_line_stays_leftover() {
        let a = wobbly_rect(0.0, 0.0, 100.0, 60.0);
        let dangler = stroke_from(&[(500.0, 500.0), (600.0, 500.0)]);
        let g = recognize_graph(&[a, dangler], 0.35);
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty());
        assert_eq!(g.leftover, vec![1]);
    }
}
