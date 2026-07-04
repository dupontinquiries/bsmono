//! viz — the shared "draw once, render many" layer for voiceterm's diagram/plot systems.
//!
//! voiceterm's geometry engines — mermaid, layered/graph layout, niftyplot — are already
//! Qt-free: they *compute positions*. Only the drawing layer was QML. Rather than write a
//! separate egui renderer for each, they all target one intermediate representation: a
//! [`Scene`] — a flat display list of 2D [`Prim`]itives in world coordinates. An engine lays
//! out **once** into a `Scene`; [`render_scene`] draws any `Scene` into egui each frame,
//! fitting world bounds to the available rect. Same immediate-mode inversion as the web
//! renderer: the expensive layout is cached in the `Scene`; per-frame is just a draw.
//!
//! This module provides the `Scene`/`Prim` IR, the egui renderer, and adapters that turn a
//! **graph** (nodes + edges) and a **plot** (data series, the niftyplot role) into a `Scene`.
//! mermaid/sequence follow the same path: their layout → `Scene`.

/// A palette colour role, resolved to a concrete colour at render time (so scenes are
/// theme-independent and respect the display accessibility filter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Fg,
    Weak,
    Accent,
    AccentSoft,
    Danger,
    Success,
    Warning,
    Card,
    Raised,
}

/// Stroke/fill styling for a primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub stroke: Option<Role>,
    pub width: f32,
    pub fill: Option<Role>,
}

impl Default for Style {
    fn default() -> Self {
        Style { stroke: Some(Role::Fg), width: 1.5, fill: None }
    }
}

impl Style {
    pub fn stroke(role: Role, width: f32) -> Style {
        Style { stroke: Some(role), width, fill: None }
    }
    pub fn filled(fill: Role, stroke: Role) -> Style {
        Style { stroke: Some(stroke), width: 1.5, fill: Some(fill) }
    }
}

/// Text anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Center,
    Start,
}

/// A 2D drawing primitive in world coordinates (y grows downward, screen-style).
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Line { a: (f32, f32), b: (f32, f32), style: Style },
    Polyline { points: Vec<(f32, f32)>, style: Style },
    Rect { min: (f32, f32), max: (f32, f32), rounding: f32, style: Style },
    Circle { center: (f32, f32), radius: f32, style: Style },
    Arrow { a: (f32, f32), b: (f32, f32), style: Style },
    Text { pos: (f32, f32), text: String, size: f32, role: Role, anchor: Anchor },
}

/// A flat display list plus its world bounds (min_x, min_y, max_x, max_y).
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub prims: Vec<Prim>,
    pub bounds: (f32, f32, f32, f32),
}

impl Default for Scene {
    fn default() -> Self {
        Scene { prims: Vec::new(), bounds: (0.0, 0.0, 1.0, 1.0) }
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene::default()
    }

    pub fn push(&mut self, p: Prim) {
        self.prims.push(p);
    }

    /// Recompute `bounds` to fit all primitives (with a small margin).
    pub fn fit_bounds(&mut self) {
        let mut minx = f32::INFINITY;
        let mut miny = f32::INFINITY;
        let mut maxx = f32::NEG_INFINITY;
        let mut maxy = f32::NEG_INFINITY;
        let mut acc = |x: f32, y: f32| {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        };
        for p in &self.prims {
            match p {
                Prim::Line { a, b, .. } | Prim::Arrow { a, b, .. } => {
                    acc(a.0, a.1);
                    acc(b.0, b.1);
                }
                Prim::Polyline { points, .. } => {
                    for pt in points {
                        acc(pt.0, pt.1);
                    }
                }
                Prim::Rect { min, max, .. } => {
                    acc(min.0, min.1);
                    acc(max.0, max.1);
                }
                Prim::Circle { center, radius, .. } => {
                    acc(center.0 - radius, center.1 - radius);
                    acc(center.0 + radius, center.1 + radius);
                }
                Prim::Text { pos, .. } => acc(pos.0, pos.1),
            }
        }
        if minx.is_finite() {
            let mx = ((maxx - minx) * 0.05).max(2.0);
            let my = ((maxy - miny) * 0.05).max(2.0);
            self.bounds = (minx - mx, miny - my, maxx + mx, maxy + my);
        }
    }

    pub fn width(&self) -> f32 {
        (self.bounds.2 - self.bounds.0).max(1e-3)
    }
    pub fn height(&self) -> f32 {
        (self.bounds.3 - self.bounds.1).max(1e-3)
    }
}

// ── egui renderer (thin; walks the prebuilt Scene) ───────────────────────────

fn role_color(role: Role, c: &uikit_egui::PaletteColors) -> egui::Color32 {
    match role {
        Role::Fg => c.text,
        Role::Weak => c.text_weak,
        Role::Accent => c.accent,
        Role::AccentSoft => c.accent_soft,
        Role::Danger => c.danger,
        Role::Success => c.success,
        Role::Warning => c.warning,
        Role::Card => c.bg_card,
        Role::Raised => c.bg_raised,
    }
}

/// Draw a [`Scene`] into `ui`, fitting its world bounds to `target` (keeping aspect ratio).
/// No layout here — cheap per frame.
pub fn render_scene(ui: &egui::Ui, scene: &Scene, target: egui::Rect, colors: &uikit_egui::PaletteColors) {
    let painter = ui.painter_at(target);
    let s = (target.width() / scene.width()).min(target.height() / scene.height()) * 0.92;
    let (cx, cy) = ((scene.bounds.0 + scene.bounds.2) / 2.0, (scene.bounds.1 + scene.bounds.3) / 2.0);
    let tc = target.center();
    let map = |p: (f32, f32)| egui::pos2(tc.x + (p.0 - cx) * s, tc.y + (p.1 - cy) * s);
    let stroke = |st: &Style| {
        egui::Stroke::new(st.width.max(0.5), st.stroke.map(|r| role_color(r, colors)).unwrap_or(egui::Color32::TRANSPARENT))
    };

    for prim in &scene.prims {
        match prim {
            Prim::Line { a, b, style } => {
                painter.line_segment([map(*a), map(*b)], stroke(style));
            }
            Prim::Polyline { points, style } => {
                let pts: Vec<egui::Pos2> = points.iter().map(|p| map(*p)).collect();
                painter.add(egui::Shape::line(pts, stroke(style)));
            }
            Prim::Rect { min, max, rounding, style } => {
                let r = egui::Rect::from_two_pos(map(*min), map(*max));
                let fill = style.fill.map(|f| role_color(f, colors)).unwrap_or(egui::Color32::TRANSPARENT);
                painter.rect(r, egui::Rounding::same(*rounding), fill, stroke(style));
            }
            Prim::Circle { center, radius, style } => {
                let fill = style.fill.map(|f| role_color(f, colors)).unwrap_or(egui::Color32::TRANSPARENT);
                painter.circle(map(*center), radius * s, fill, stroke(style));
            }
            Prim::Arrow { a, b, style } => {
                let (pa, pb) = (map(*a), map(*b));
                painter.line_segment([pa, pb], stroke(style));
                // arrowhead
                let dir = (pb - pa).normalized();
                let n = egui::vec2(-dir.y, dir.x);
                let tip = pb;
                let back = pb - dir * 9.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, back + n * 4.0, back - n * 4.0],
                    stroke(style).color,
                    egui::Stroke::NONE,
                ));
            }
            Prim::Text { pos, text, size, role, anchor } => {
                let align = match anchor {
                    Anchor::Center => egui::Align2::CENTER_CENTER,
                    Anchor::Start => egui::Align2::LEFT_CENTER,
                };
                painter.text(map(*pos), align, text, egui::FontId::proportional(*size), role_color(*role, colors));
            }
        }
    }
}

// ── graph adapter (nodes + edges → Scene) ────────────────────────────────────

/// A positioned graph node (the layout engine supplies `pos`; here we mirror its output).
#[derive(Debug, Clone)]
pub struct GNode {
    pub id: String,
    pub label: String,
    pub pos: (f32, f32),
    pub size: (f32, f32),
}

/// A directed edge between node ids.
#[derive(Debug, Clone)]
pub struct GEdge {
    pub from: String,
    pub to: String,
}

/// Turn a laid-out graph into a `Scene`: rounded node boxes with labels + arrowed edges
/// between box centers. (mermaid/link_graph feed the same shape after their layout.)
pub fn graph_scene(nodes: &[GNode], edges: &[GEdge]) -> Scene {
    let mut scene = Scene::new();
    let find = |id: &str| nodes.iter().find(|n| n.id == id).map(|n| n.pos);
    // edges first (under the nodes)
    for e in edges {
        if let (Some(a), Some(b)) = (find(&e.from), find(&e.to)) {
            scene.push(Prim::Arrow { a, b, style: Style::stroke(Role::Weak, 1.5) });
        }
    }
    for n in nodes {
        let (w, h) = (n.size.0 / 2.0, n.size.1 / 2.0);
        scene.push(Prim::Rect {
            min: (n.pos.0 - w, n.pos.1 - h),
            max: (n.pos.0 + w, n.pos.1 + h),
            rounding: 6.0,
            style: Style::filled(Role::Card, Role::Accent),
        });
        scene.push(Prim::Text { pos: n.pos, text: n.label.clone(), size: 13.0, role: Role::Fg, anchor: Anchor::Center });
    }
    scene.fit_bounds();
    scene
}

// ── plot adapter (data series → Scene, the niftyplot role) ───────────────────

/// A named data series.
#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub points: Vec<(f64, f64)>,
    pub role: Role,
}

/// Build a `Scene` for one or more data series: axes, and each series as a polyline. World
/// y grows downward, so data is flipped (higher value = higher on screen). This is the
/// niftyplot render target; the niftyplot engine's sampled points feed straight in.
pub fn plot_scene(series: &[Series]) -> Scene {
    let mut scene = Scene::new();
    // data bounds
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for s in series {
        for &(x, y) in &s.points {
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    if !minx.is_finite() {
        return scene;
    }
    let (w, h) = (200.0f32, 120.0f32); // world plot area
    let spanx = (maxx - minx).max(1e-9);
    let spany = (maxy - miny).max(1e-9);
    let to_world = |x: f64, y: f64| -> (f32, f32) {
        let px = ((x - minx) / spanx) as f32 * w;
        let py = h - ((y - miny) / spany) as f32 * h; // flip y
        (px, py)
    };
    // axes (left + bottom)
    scene.push(Prim::Line { a: (0.0, 0.0), b: (0.0, h), style: Style::stroke(Role::Weak, 1.0) });
    scene.push(Prim::Line { a: (0.0, h), b: (w, h), style: Style::stroke(Role::Weak, 1.0) });
    // series
    for s in series {
        let pts: Vec<(f32, f32)> = s.points.iter().map(|&(x, y)| to_world(x, y)).collect();
        if pts.len() >= 2 {
            scene.push(Prim::Polyline { points: pts, style: Style::stroke(s.role, 2.0) });
        } else {
            for p in pts {
                scene.push(Prim::Circle { center: p, radius: 2.0, style: Style::stroke(s.role, 2.0) });
            }
        }
    }
    scene.fit_bounds();
    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_fits_bounds_over_prims() {
        let mut s = Scene::new();
        s.push(Prim::Line { a: (0.0, 0.0), b: (10.0, 20.0), style: Style::default() });
        s.fit_bounds();
        // bounds cover the line (plus margin)
        assert!(s.bounds.0 <= 0.0 && s.bounds.1 <= 0.0);
        assert!(s.bounds.2 >= 10.0 && s.bounds.3 >= 20.0);
    }

    #[test]
    fn graph_scene_has_box_and_label_per_node_and_arrow_per_edge() {
        let nodes = vec![
            GNode { id: "a".into(), label: "A".into(), pos: (0.0, 0.0), size: (40.0, 24.0) },
            GNode { id: "b".into(), label: "B".into(), pos: (100.0, 0.0), size: (40.0, 24.0) },
        ];
        let edges = vec![GEdge { from: "a".into(), to: "b".into() }];
        let scene = graph_scene(&nodes, &edges);
        let rects = scene.prims.iter().filter(|p| matches!(p, Prim::Rect { .. })).count();
        let texts = scene.prims.iter().filter(|p| matches!(p, Prim::Text { .. })).count();
        let arrows = scene.prims.iter().filter(|p| matches!(p, Prim::Arrow { .. })).count();
        assert_eq!(rects, 2);
        assert_eq!(texts, 2);
        assert_eq!(arrows, 1);
    }

    #[test]
    fn plot_scene_builds_axes_and_polyline() {
        let series = vec![Series {
            name: "cpu".into(),
            points: vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)],
            role: Role::Accent,
        }];
        let scene = plot_scene(&series);
        let lines = scene.prims.iter().filter(|p| matches!(p, Prim::Line { .. })).count();
        let polys = scene.prims.iter().filter(|p| matches!(p, Prim::Polyline { .. })).count();
        assert_eq!(lines, 2, "two axes");
        assert_eq!(polys, 1, "one series polyline");
    }

    #[test]
    fn plot_flips_y_so_higher_values_sit_higher() {
        let series = vec![Series { name: "s".into(), points: vec![(0.0, 0.0), (1.0, 10.0)], role: Role::Fg }];
        let scene = plot_scene(&series);
        if let Some(Prim::Polyline { points, .. }) = scene.prims.iter().find(|p| matches!(p, Prim::Polyline { .. })) {
            // higher data y (10) → smaller world y (screen-up)
            assert!(points[1].1 < points[0].1);
        } else {
            panic!("no polyline");
        }
    }

    #[test]
    fn empty_plot_is_empty() {
        assert!(plot_scene(&[]).prims.is_empty());
    }
}
