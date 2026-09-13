//! SVG backend: a `GraphDoc` → one `<svg>` string. std-only, no UI, so a
//! server (depot's tutorials), a CLI (`depot niftyplot FILE`) or an
//! "export" button in the egui front-end can all use it.
//!
//! Layout policy — the same input renders the same bytes every time:
//!
//! * a document with planes is laid out in **columns**, one plane per
//!   column in declaration order, members stacked top to bottom, nodes
//!   outside every plane in a last column. This keeps a plane's members
//!   together, which the force layout does not know how to do, and is
//!   what a diagram of "online side | air gap | offline side" wants;
//! * a document without planes goes through `graph_layout`'s force
//!   layout (deterministic: seeded grid, no RNG) with a left→right bias.
//!
//! Colours are concrete presentation attributes (so librsvg, resvg,
//! ImageMagick and image viewers render it as-is) plus a `<style>` of
//! `var(--…)` overrides that only browsers apply, so the same SVG follows a
//! page's light/dark theme when inlined. Verified both ways: rsvg-convert
//! rasterises it correctly, and the depot UI inlines it.

use crate::graph_layout::{bounding_box, layout_force_directed, seed_grid, CardPos, FlowDirection, LayoutConfig, LayoutEdge};
use crate::niftyplot::{EdgeStyle, GraphDoc, Shape};
use std::collections::HashMap;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct SvgOptions {
    /// Approximate glyph width used to size node boxes (px per char).
    pub char_w: f32,
    pub font_px: f32,
    pub node_h: f32,
    pub node_pad: f32,
    /// Gap between columns / between stacked nodes.
    pub gap_x: f32,
    pub gap_y: f32,
    pub margin: f32,
}

impl Default for SvgOptions {
    fn default() -> Self {
        SvgOptions { char_w: 7.2, font_px: 13.0, node_h: 34.0, node_pad: 14.0, gap_x: 120.0, gap_y: 26.0, margin: 24.0 }
    }
}

struct Placed {
    id: String,
    label: String,
    shape: Shape,
    x: f32,
    y: f32,
    hw: f32,
    hh: f32,
}

pub fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

fn node_label(doc: &GraphDoc, i: usize) -> String {
    let n = &doc.nodes[i];
    n.label.clone().unwrap_or_else(|| n.id.clone())
}

fn half_w(label: &str, o: &SvgOptions) -> f32 {
    (label.chars().count() as f32 * o.char_w / 2.0 + o.node_pad).max(28.0)
}

/// Place nodes. Returns positions (centres) plus, for planed docs, each
/// plane's bounding column (name, x0, y0, x1, y1).
fn place(doc: &GraphDoc, o: &SvgOptions) -> (Vec<Placed>, Vec<(String, f32, f32, f32, f32)>) {
    let n = doc.nodes.len();
    let mut placed: Vec<Placed> = (0..n)
        .map(|i| {
            let label = node_label(doc, i);
            let hw = half_w(&label, o);
            Placed { id: doc.nodes[i].id.clone(), label, shape: doc.nodes[i].shape, x: 0.0, y: 0.0, hw, hh: o.node_h / 2.0 }
        })
        .collect();
    let mut planes_out = Vec::new();
    if doc.planes.iter().any(|p| !p.members.is_empty()) {
        // Column layout. Membership by first plane that lists the node.
        let mut col_of: HashMap<&str, usize> = HashMap::new();
        for (pi, p) in doc.planes.iter().enumerate() {
            for m in &p.members {
                col_of.entry(m.as_str()).or_insert(pi);
            }
        }
        let ncols = doc.planes.len() + 1; // + the "no plane" column
        let mut columns: Vec<Vec<usize>> = vec![Vec::new(); ncols];
        for (i, nd) in doc.nodes.iter().enumerate() {
            let c = col_of.get(nd.id.as_str()).copied().unwrap_or(ncols - 1);
            columns[c].push(i);
        }
        let plane_pad = 16.0;
        let title_h = 22.0;
        let mut x = o.margin;
        let tallest = columns.iter().map(|c| c.len()).max().unwrap_or(1).max(1) as f32;
        let col_h = tallest * o.node_h + (tallest - 1.0) * o.gap_y;
        for (c, members) in columns.iter().enumerate() {
            if members.is_empty() {
                continue;
            }
            let col_w = members.iter().map(|&i| placed[i].hw * 2.0).fold(0.0, f32::max);
            let cx = x + plane_pad + col_w / 2.0;
            // Centre this column's stack vertically against the tallest column.
            let stack_h = members.len() as f32 * o.node_h + (members.len() as f32 - 1.0) * o.gap_y;
            let mut y = o.margin + title_h + plane_pad + (col_h - stack_h) / 2.0 + o.node_h / 2.0;
            for &i in members {
                placed[i].x = cx;
                placed[i].y = y;
                y += o.node_h + o.gap_y;
            }
            if c < doc.planes.len() {
                planes_out.push((doc.planes[c].name.clone(), x, o.margin, x + col_w + 2.0 * plane_pad, o.margin + title_h + col_h + 2.0 * plane_pad));
            }
            x += col_w + 2.0 * plane_pad + o.gap_x;
        }
    } else if n > 0 {
        let mut cards: Vec<CardPos> = placed.iter().map(|p| CardPos { id: p.id.clone(), x: 0.0, y: 0.0, pinned: false, half_w: p.hw, half_h: p.hh }).collect();
        let edges: Vec<LayoutEdge> = doc.edges.iter().map(|e| LayoutEdge::new(e.from.clone(), e.to.clone())).collect();
        let cfg = LayoutConfig { width: 640.0, height: 420.0, flow_direction: FlowDirection::LeftRight, iterations: 300, ..LayoutConfig::default() };
        seed_grid(&mut cards, &cfg);
        layout_force_directed(&mut cards, &edges, &cfg);
        let (x0, y0, _, _) = bounding_box(&cards);
        for (p, c) in placed.iter_mut().zip(cards.iter()) {
            p.x = c.x - x0 + p.hw + o.margin;
            p.y = c.y - y0 + p.hh + o.margin;
        }
    }
    (placed, planes_out)
}

/// Point on the boundary of node `p`'s box along the ray from its centre
/// toward (tx, ty) — where an edge should start or end.
fn border_point(p: &Placed, tx: f32, ty: f32) -> (f32, f32) {
    let dx = tx - p.x;
    let dy = ty - p.y;
    if dx.abs() < 0.001 && dy.abs() < 0.001 {
        return (p.x, p.y);
    }
    let (hw, hh) = (p.hw + 2.0, p.hh + 2.0);
    let t = (hw / dx.abs()).min(hh / dy.abs());
    (p.x + dx * t, p.y + dy * t)
}

fn shape_path(p: &Placed, out: &mut String) {
    let (x, y, hw, hh) = (p.x, p.y, p.hw, p.hh);
    match p.shape {
        Shape::Circle => {
            let _ = write!(out, r##"<ellipse class="node" fill="#fff" stroke="#222" stroke-width="1.4" cx="{x:.1}" cy="{y:.1}" rx="{hw:.1}" ry="{hh:.1}"/>"##);
        }
        Shape::Diamond => {
            let (hw, hh) = (hw + 10.0, hh + 8.0);
            let _ = write!(out, r##"<polygon class="node" fill="#fff" stroke="#222" stroke-width="1.4" points="{:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1}"/>"##, x, y - hh, x + hw, y, x, y + hh, x - hw, y);
        }
        Shape::Hexagon => {
            let k = 10.0;
            let _ = write!(out, r##"<polygon class="node" fill="#fff" stroke="#222" stroke-width="1.4" points="{:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1}"/>"##, x - hw + k, y - hh, x + hw - k, y - hh, x + hw, y, x + hw - k, y + hh, x - hw + k, y + hh, x - hw, y);
        }
        Shape::Parallelogram => {
            let k = 10.0;
            let _ = write!(out, r##"<polygon class="node" fill="#fff" stroke="#222" stroke-width="1.4" points="{:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1}"/>"##, x - hw + k, y - hh, x + hw, y - hh, x + hw - k, y + hh, x - hw, y + hh);
        }
        Shape::Cylinder => {
            let _ = write!(out, r##"<rect class="node" fill="#fff" stroke="#222" stroke-width="1.4" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}"/><ellipse class="node-top" fill="#fff" stroke="#222" stroke-width="1.4" cx="{x:.1}" cy="{:.1}" rx="{hw:.1}" ry="5"/>"##, x - hw, y - hh, 2.0 * hw, 2.0 * hh, 5.0, y - hh);
        }
        Shape::Cloud => {
            let _ = write!(out, r##"<rect class="node" fill="#fff" stroke="#222" stroke-width="1.4" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}"/>"##, x - hw, y - hh, 2.0 * hw, 2.0 * hh, hh);
        }
        Shape::Note => {
            let f = 9.0;
            let _ = write!(out, r##"<path class="node" fill="#fff" stroke="#222" stroke-width="1.4" d="M{:.1},{:.1} H{:.1} l{f},{f} V{:.1} H{:.1} Z"/><path class="node-fold" fill="none" stroke="#222" stroke-width="1" d="M{:.1},{:.1} v{f} h{f}"/>"##, x - hw, y - hh, x + hw - f, y + hh, x - hw, x + hw - f, y - hh);
        }
        Shape::Rect => {
            let _ = write!(out, r##"<rect class="node" fill="#fff" stroke="#222" stroke-width="1.4" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="6"/>"##, x - hw, y - hh, 2.0 * hw, 2.0 * hh);
        }
    }
}

/// Render `doc` to an SVG document string.
pub fn render(doc: &GraphDoc, o: &SvgOptions) -> String {
    let (placed, planes) = place(doc, o);
    let idx: HashMap<&str, usize> = placed.iter().enumerate().map(|(i, p)| (p.id.as_str(), i)).collect();

    // Extent.
    let mut x1 = 0.0f32;
    let mut y1 = 0.0f32;
    for p in &placed {
        x1 = x1.max(p.x + p.hw + 12.0);
        y1 = y1.max(p.y + p.hh + 12.0);
    }
    for pl in &planes {
        x1 = x1.max(pl.3);
        y1 = y1.max(pl.4);
    }
    let title_h = if doc.title.is_some() { 30.0 } else { 0.0 };
    let warn_h = doc.warnings.len() as f32 * 16.0 + if doc.warnings.is_empty() { 0.0 } else { 8.0 };
    let w = (x1 + o.margin).max(120.0);
    let h = (y1 + o.margin + title_h + warn_h).max(60.0);

    let mut s = String::with_capacity(4096);
    let _ = write!(s, r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" class="niftyplot" role="img">"##);
    // Theme hooks: a browser applies these over the attributes below (a
    // stylesheet beats a presentation attribute); librsvg/resvg/ImageMagick
    // drop the var() declarations and keep the attributes. Both render.
    s.push_str(concat!(
        "<style>",
        ".niftyplot{font-family:system-ui,sans-serif}",
        ".node,.node-top{fill:var(--nifty-node,var(--panel));stroke:var(--nifty-line,var(--fg))}",
        ".node-fold{stroke:var(--nifty-line,var(--fg))}",
        ".label,.elabel,.title{fill:var(--nifty-text,var(--fg))}",
        ".edge{stroke:var(--nifty-edge,var(--muted))}",
        ".arrow{fill:var(--nifty-edge,var(--muted))}",
        ".elabel-bg{fill:var(--nifty-bg,var(--bg))}",
        ".plane{fill:var(--nifty-plane,var(--panel));stroke:var(--nifty-planeline,var(--line))}",
        ".plane-title{fill:var(--nifty-muted,var(--muted));text-transform:uppercase}",
        "</style>"
    ));
    let _ = write!(s, r##"<defs><marker id="np-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="#666" class="arrow"/></marker></defs>"##);
    let _ = write!(s, r##"<g font-size="{:.0}" transform="translate(0,{title_h:.0})">"##, o.font_px);
    if let Some(t) = &doc.title {
        let _ = write!(s, r##"<text class="title" fill="#222" font-weight="600" x="{:.0}" y="-10">{}</text>"##, o.margin, xml_escape(t));
    }
    for (name, x0, y0, x1, y1) in &planes {
        let _ = write!(s, r##"<rect class="plane" fill="#f4f4f4" stroke="#ccc" x="{x0:.1}" y="{y0:.1}" width="{:.1}" height="{:.1}" rx="10"/><text class="plane-title" fill="#666" font-size="10" letter-spacing=".06em" x="{:.1}" y="{:.1}">{}</text>"##, x1 - x0, y1 - y0, x0 + 12.0, y0 + 16.0, xml_escape(name));
    }
    for e in &doc.edges {
        let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) else { continue };
        let (pa, pb) = (&placed[a], &placed[b]);
        let (sx, sy) = border_point(pa, pb.x, pb.y);
        let (ex, ey) = border_point(pb, pa.x, pa.y);
        let class = match e.style {
            EdgeStyle::Solid => "edge",
            EdgeStyle::Dashed => "edge dashed",
            EdgeStyle::Heavy => "edge heavy",
            EdgeStyle::Bidirectional => "edge",
        };
        let start = if e.style == EdgeStyle::Bidirectional { r##" marker-start="url(#np-arrow)""## } else { "" };
        let sw = if e.style == EdgeStyle::Heavy { "3" } else { "1.6" };
        let dash = if e.style == EdgeStyle::Dashed { r##" stroke-dasharray="6 5""## } else { "" };
        let _ = write!(s, r##"<line class="{class}" fill="none" stroke="#666" stroke-width="{sw}"{dash} x1="{sx:.1}" y1="{sy:.1}" x2="{ex:.1}" y2="{ey:.1}" marker-end="url(#np-arrow)"{start}/>"##);
        if let Some(l) = &e.label {
            let (mx, my) = ((sx + ex) / 2.0, (sy + ey) / 2.0);
            let lw = l.chars().count() as f32 * o.char_w * 0.85 + 10.0;
            let _ = write!(s, r##"<rect class="elabel-bg" fill="#fff" opacity=".9" x="{:.1}" y="{:.1}" width="{lw:.1}" height="16" rx="3"/><text class="elabel" fill="#222" text-anchor="middle" dominant-baseline="middle" font-size="11" x="{mx:.1}" y="{my:.1}">{}</text>"##, mx - lw / 2.0, my - 8.0, xml_escape(l));
        }
    }
    for p in &placed {
        shape_path(p, &mut s);
        let _ = write!(s, r##"<text class="label" fill="#222" text-anchor="middle" dominant-baseline="middle" x="{:.1}" y="{:.1}">{}</text>"##, p.x, p.y, xml_escape(&p.label));
    }
    if !doc.warnings.is_empty() {
        let mut y = y1 + 14.0;
        for wn in &doc.warnings {
            let _ = write!(s, r##"<text class="warn" fill="#b8860b" font-size="10" x="{:.0}" y="{y:.0}">{} line {}: {}</text>"##, o.margin, wn.severity.glyph(), wn.line, xml_escape(&wn.message));
            y += 16.0;
        }
    }
    s.push_str("</g></svg>");
    s
}

/// Parse and render in one call.
pub fn render_source(source: &str) -> String {
    render(&crate::niftyplot::parse(source), &SvgOptions::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLANED: &str = "%kind: graph\n%title: \"One pack, two depots\"\nplane online {\n    Online: \"online depot\"\n    Pack: \"sync.depot\"\n}\nplane offline {\n    Have: \"have.txt\"\n    Offline: \"offline depot\"\n}\nOffline -- \"1. have\" → Have\nHave → Online\nOnline => Pack\nPack -.-> Offline\n";

    #[test]
    fn planed_doc_renders_columns_edges_labels_and_is_deterministic() {
        let a = render_source(PLANED);
        let b = render_source(PLANED);
        assert_eq!(a, b, "same input, same bytes");
        assert!(a.starts_with("<svg ") && a.ends_with("</svg>"));
        assert!(a.contains(">One pack, two depots<"));
        assert_eq!(a.matches("class=\"plane\"").count(), 2, "one column per plane");
        assert!(a.contains(">ONLINE<") || a.contains(">online<"), "plane title (css uppercases): {a}");
        assert_eq!(a.matches("<line class=\"edge").count(), 4);
        assert!(a.contains("class=\"edge heavy\"") && a.contains("class=\"edge dashed\""));
        assert!(a.contains(">1. have<"), "edge label");
        for l in ["online depot", "sync.depot", "have.txt", "offline depot"] {
            assert!(a.contains(&format!(">{l}<")), "node label {l}");
        }
        // Column order = declaration order: online's members left of offline's.
        let x_of = |label: &str| -> f32 {
            let i = a.find(&format!(">{label}<")).unwrap();
            let head = &a[..i];
            let xs = head.rfind("x=\"").unwrap();
            head[xs + 3..].split('"').next().unwrap().parse().unwrap()
        };
        assert!(x_of("online depot") < x_of("offline depot"));
        assert!((x_of("online depot") - x_of("sync.depot")).abs() < 0.01, "same column, same x");
    }

    #[test]
    fn plain_doc_uses_force_layout_and_shapes_escape_and_warn() {
        let src = "%kind: graph\nA as circle: \"Start & <go>\"\nB as diamond: \"Decision\"\nC as cylinder: \"DB\"\nD as note: \"Note\"\nE as hexagon: \"Hex\"\nF as parallelogram: \"Para\"\nG as cloud: \"Cloud\"\nA → B → C\nB ↔ D\nA → {E, F, G}\n";
        let s = render_source(src);
        assert!(s.contains("Start &amp; &lt;go&gt;"), "labels are escaped");
        assert!(s.contains("<ellipse class=\"node\"") && s.contains("<polygon class=\"node\"") && s.contains("class=\"node-fold\"") && s.contains("class=\"node-top\""));
        assert!(s.contains("marker-start="), "bidirectional edge has both heads");
        assert_eq!(s.matches("class=\"plane\"").count(), 0);
        assert_eq!(render_source(src), s, "force layout is deterministic");
        // Every node centre is inside the viewBox.
        let vb: Vec<f32> = s.split("viewBox=\"").nth(1).unwrap().split('"').next().unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
        for seg in s.split("<text class=\"label\" x=\"").skip(1) {
            let x: f32 = seg.split('"').next().unwrap().parse().unwrap();
            let y: f32 = seg.split("y=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
            assert!(x > 0.0 && x < vb[2] && y > 0.0 && y < vb[3], "node at {x},{y} outside {vb:?}");
        }
        let w = render_source("%kind: graph\n%bogus: x\nA → B\n");
        assert!(w.contains("class=\"warn\""), "unknown directive surfaces as a warning line: {w}");
        assert!(render_source("").contains("<svg "), "empty doc still renders");
    }
}
