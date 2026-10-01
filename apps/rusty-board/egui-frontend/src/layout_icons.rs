//! The layout switcher's icons: one SVG per kind of keyboard, drawn as vector strokes.
//!
//! The rail used to print each layout's name, which on a strip narrow enough to leave the
//! keys their width means "English (Dvorak — Ortholinear) ⌨" running off both edges. An
//! icon says which board it is at 20pt.
//!
//! ## Why a parser for a subset, not an SVG library
//!
//! The icons are ours, 24x24, stroke-drawn, and use five elements. `resvg` (what hatch
//! uses for arbitrary Breeze icons) is a rasteriser stack -- usvg, tiny-skia, roxmltree --
//! which a keyboard compiled on a phone would pay for to draw nine outlines, and whose
//! output is a texture that blurs when the scale changes. The subset here turns each
//! element into egui strokes at paint time, so an icon is crisp at any scale for free.
//!
//! Supported: `<svg viewBox stroke-width fill>`, `<rect x y width height rx>`,
//! `<circle cx cy r>`, `<line x1 y1 x2 y2>`, `<polyline points>`, `<path d>` with
//! `M L H V Z` (and their relative forms). `fill="currentColor"` on an element fills it;
//! everything else is stroked. Anything outside the subset is an ERROR, not skipped: an
//! icon that silently loses half its shapes is the failure this module must not have, and
//! every shipped icon is parsed by a test.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// One drawable element, in viewBox units.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Rect { x: f32, y: f32, w: f32, h: f32, rx: f32, fill: bool },
    Circle { cx: f32, cy: f32, r: f32, fill: bool },
    /// A polyline; `closed` joins the last point to the first.
    Poly { pts: Vec<(f32, f32)>, closed: bool },
}

/// A parsed icon.
#[derive(Debug, Clone, PartialEq)]
pub struct Icon {
    /// `min-x min-y width height`.
    pub view: [f32; 4],
    pub stroke_w: f32,
    pub prims: Vec<Prim>,
}

/// Which icon a keyboard gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Qwerty,
    Ortho,
    Symbols,
    Cardinal,
    Cardinal8,
    Steno,
    Morse,
    Slide,
    /// Anything unrecognised: an outline with the layout's initial painted inside.
    Generic,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Qwerty, Kind::Ortho, Kind::Symbols, Kind::Cardinal, Kind::Cardinal8, Kind::Steno, Kind::Morse, Kind::Slide, Kind::Generic,
    ];

    /// The icon's SVG source, compiled in -- a keyboard must not fail to draw its switcher
    /// because a data directory was not installed.
    pub fn svg(self) -> &'static str {
        match self {
            Kind::Qwerty => include_str!("../../resources/icons/layouts/qwerty.svg"),
            Kind::Ortho => include_str!("../../resources/icons/layouts/ortho.svg"),
            Kind::Symbols => include_str!("../../resources/icons/layouts/symbols.svg"),
            Kind::Cardinal => include_str!("../../resources/icons/layouts/cardinal.svg"),
            Kind::Cardinal8 => include_str!("../../resources/icons/layouts/cardinal8.svg"),
            Kind::Steno => include_str!("../../resources/icons/layouts/steno.svg"),
            Kind::Morse => include_str!("../../resources/icons/layouts/morse.svg"),
            Kind::Slide => include_str!("../../resources/icons/layouts/slide.svg"),
            Kind::Generic => include_str!("../../resources/icons/layouts/generic.svg"),
        }
    }

    /// The kind for a layout, from its name. Order matters: "Cardinal 8" before
    /// "Cardinal", "Dvorak — Ortholinear" before anything that could also say "qwerty".
    pub fn for_layout(name: &str) -> Kind {
        let n = name.to_lowercase();
        if n.contains("cardinal8") || n.contains("cardinal 8") || n.contains("cardinal-8") {
            Kind::Cardinal8
        } else if n.contains("cardinal") || n.contains("flick") {
            Kind::Cardinal
        } else if n.contains("steno") {
            Kind::Steno
        } else if n.contains("morse") {
            Kind::Morse
        } else if n.contains("slide") {
            Kind::Slide
        } else if n.contains("symbol") {
            Kind::Symbols
        } else if n.contains("ortho") || n.contains("dvorak") || n.contains("colemak") {
            Kind::Ortho
        } else if n.contains("qwerty") || n.contains("azerty") || n.contains("qwertz") {
            Kind::Qwerty
        } else {
            Kind::Generic
        }
    }
}

/// The rail's own chevrons.
pub const COLLAPSE_SVG: &str = include_str!("../../resources/icons/layouts/collapse.svg");
pub const EXPAND_SVG: &str = include_str!("../../resources/icons/layouts/expand.svg");

/// Parse an icon in the supported subset (see the module doc).
pub fn parse(svg: &str) -> Result<Icon, String> {
    let mut icon = Icon { view: [0.0, 0.0, 24.0, 24.0], stroke_w: 1.5, prims: Vec::new() };
    let mut root_fill = false;
    let mut seen_root = false;
    let mut rest = svg;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        // Comments and declarations carry no drawing.
        if after.starts_with("!--") {
            let end = after.find("-->").ok_or("unterminated comment")?;
            rest = &after[end + 3..];
            continue;
        }
        let close = after.find('>').ok_or("unterminated tag")?;
        let tag = after[..close].trim_end_matches('/').trim();
        rest = &after[close + 1..];
        if tag.starts_with('?') || tag.starts_with('!') || tag.starts_with('/') {
            continue;
        }
        let (name, attrs_src) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let attrs = attrs(attrs_src)?;
        let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        let num = |k: &str| -> Result<f32, String> {
            get(k).ok_or(format!("<{name}> needs {k}"))?.trim().parse::<f32>().map_err(|e| format!("{k}: {e}"))
        };
        let fill = match get("fill") {
            Some(v) => v == "currentColor",
            None => root_fill,
        };
        match name {
            "svg" => {
                seen_root = true;
                if let Some(vb) = get("viewBox") {
                    let v = numbers(vb)?;
                    if v.len() != 4 || v[2] <= 0.0 || v[3] <= 0.0 {
                        return Err(format!("bad viewBox {vb:?}"));
                    }
                    icon.view = [v[0], v[1], v[2], v[3]];
                }
                if let Some(w) = get("stroke-width") {
                    icon.stroke_w = w.trim().parse().map_err(|e| format!("stroke-width: {e}"))?;
                }
                root_fill = get("fill") == Some("currentColor");
            }
            "rect" => icon.prims.push(Prim::Rect {
                x: get("x").map_or(Ok(0.0), |_| num("x"))?,
                y: get("y").map_or(Ok(0.0), |_| num("y"))?,
                w: num("width")?,
                h: num("height")?,
                rx: get("rx").map_or(Ok(0.0), |_| num("rx"))?,
                fill,
            }),
            "circle" => icon.prims.push(Prim::Circle { cx: num("cx")?, cy: num("cy")?, r: num("r")?, fill }),
            "line" => icon.prims.push(Prim::Poly {
                pts: vec![(num("x1")?, num("y1")?), (num("x2")?, num("y2")?)],
                closed: false,
            }),
            "polyline" | "polygon" => {
                let v = numbers(get("points").ok_or("polyline needs points")?)?;
                if v.len() < 4 || v.len() % 2 != 0 {
                    return Err("polyline points must be pairs".into());
                }
                icon.prims.push(Prim::Poly {
                    pts: v.chunks(2).map(|c| (c[0], c[1])).collect(),
                    closed: name == "polygon",
                });
            }
            "path" => icon.prims.extend(path(get("d").ok_or("path needs d")?)?),
            other => return Err(format!("<{other}> is outside the supported subset")),
        }
    }
    if !seen_root {
        return Err("no <svg> root".into());
    }
    Ok(icon)
}

/// `name="value"` pairs.
fn attrs(src: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut s = src.trim();
    while !s.is_empty() {
        let eq = s.find('=').ok_or(format!("attribute without a value near {s:?}"))?;
        let key = s[..eq].trim().to_string();
        let v = s[eq + 1..].trim_start();
        let q = v.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("unquoted attribute")?;
        let end = v[1..].find(q).ok_or("unterminated attribute")?;
        out.push((key, v[1..1 + end].to_string()));
        s = v[end + 2..].trim_start();
    }
    Ok(out)
}

/// A list of numbers separated by spaces and/or commas.
fn numbers(s: &str) -> Result<Vec<f32>, String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f32>().map_err(|e| format!("{t:?}: {e}")))
        .collect()
}

/// `M L H V Z` and relative forms, as polylines -- one per subpath.
fn path(d: &str) -> Result<Vec<Prim>, String> {
    // Tokenise: a command letter, or a number (a '-' starts a new number, as in "M3-4").
    let mut toks: Vec<Tok> = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, toks: &mut Vec<Tok>| -> Result<(), String> {
        if !num.is_empty() {
            toks.push(Tok::Num(num.parse().map_err(|e| format!("{num:?}: {e}"))?));
            num.clear();
        }
        Ok(())
    };
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            flush(&mut num, &mut toks)?;
            toks.push(Tok::Cmd(c));
        } else if c == '-' && !num.is_empty() && !num.ends_with('e') {
            flush(&mut num, &mut toks)?;
            num.push(c);
        } else if c == ',' || c.is_whitespace() {
            flush(&mut num, &mut toks)?;
        } else {
            num.push(c);
        }
    }
    flush(&mut num, &mut toks)?;

    let mut out = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut cmd = ' ';
    let mut i = 0;
    let take = |i: &mut usize| -> Result<f32, String> {
        match toks.get(*i) {
            Some(Tok::Num(v)) => {
                *i += 1;
                Ok(*v)
            }
            _ => Err("path: expected a number".into()),
        }
    };
    let end_sub = |cur: &mut Vec<(f32, f32)>, out: &mut Vec<Prim>, closed: bool| {
        if cur.len() >= 2 {
            out.push(Prim::Poly { pts: std::mem::take(cur), closed });
        } else {
            cur.clear();
        }
    };
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                end_sub(&mut cur, &mut out, true);
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            'M' => {
                end_sub(&mut cur, &mut out, false);
                let (dx, dy) = (take(&mut i)?, take(&mut i)?);
                (x, y) = if rel { (x + dx, y + dy) } else { (dx, dy) };
                cur.push((x, y));
                // Further pairs after a moveto are linetos (SVG 1.1 §8.3.2).
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let (dx, dy) = (take(&mut i)?, take(&mut i)?);
                (x, y) = if rel { (x + dx, y + dy) } else { (dx, dy) };
                cur.push((x, y));
            }
            'H' => {
                let v = take(&mut i)?;
                x = if rel { x + v } else { v };
                cur.push((x, y));
            }
            'V' => {
                let v = take(&mut i)?;
                y = if rel { y + v } else { v };
                cur.push((x, y));
            }
            other => return Err(format!("path command {other:?} is outside the supported subset")),
        }
    }
    end_sub(&mut cur, &mut out, false);
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
enum Tok {
    Cmd(char),
    Num(f32),
}

/// Paint `icon` scaled uniformly to fit `rect` (centred), in `color`.
pub fn paint(painter: &Painter, icon: &Icon, rect: Rect, color: Color32) {
    let [vx, vy, vw, vh] = icon.view;
    let k = (rect.width() / vw).min(rect.height() / vh);
    let origin = rect.center() - Vec2::new(vw, vh) * k * 0.5;
    let at = |x: f32, y: f32| Pos2::new(origin.x + (x - vx) * k, origin.y + (y - vy) * k);
    let stroke = Stroke::new((icon.stroke_w * k).max(1.0), color);
    for p in &icon.prims {
        match p {
            Prim::Rect { x, y, w, h, rx, fill } => {
                let r = Rect::from_min_max(at(*x, *y), at(x + w, y + h));
                let cr = CornerRadius::same((rx * k).round().clamp(0.0, 255.0) as u8);
                if *fill {
                    painter.rect_filled(r, cr, color);
                } else {
                    painter.rect_stroke(r, cr, stroke, egui::StrokeKind::Middle);
                }
            }
            Prim::Circle { cx, cy, r, fill } => {
                if *fill {
                    painter.circle_filled(at(*cx, *cy), r * k, color);
                } else {
                    painter.circle_stroke(at(*cx, *cy), r * k, stroke);
                }
            }
            Prim::Poly { pts, closed } => {
                let pts: Vec<Pos2> = pts.iter().map(|(x, y)| at(*x, *y)).collect();
                painter.add(if *closed { Shape::closed_line(pts, stroke) } else { Shape::line(pts, stroke) });
            }
        }
    }
}

/// Parsed icons, once per process. Parsing is cheap but the rail draws every frame.
pub fn icon(kind: Kind) -> &'static Icon {
    use std::sync::OnceLock;
    static ICONS: OnceLock<Vec<Icon>> = OnceLock::new();
    let all = ICONS.get_or_init(|| {
        Kind::ALL.iter().map(|k| parse(k.svg()).unwrap_or_else(|e| panic!("{k:?}.svg: {e}"))).collect()
    });
    &all[Kind::ALL.iter().position(|k| *k == kind).expect("in ALL")]
}

/// The chevrons, parsed once.
pub fn chevron(expand: bool) -> &'static Icon {
    use std::sync::OnceLock;
    static C: OnceLock<(Icon, Icon)> = OnceLock::new();
    let (c, e) = C.get_or_init(|| {
        (parse(COLLAPSE_SVG).expect("collapse.svg"), parse(EXPAND_SVG).expect("expand.svg"))
    });
    if expand { e } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(icon: &Icon) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut add = |x: f32, y: f32| {
            b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        };
        for p in &icon.prims {
            match p {
                Prim::Rect { x, y, w, h, .. } => {
                    add(*x, *y);
                    add(x + w, y + h);
                }
                Prim::Circle { cx, cy, r, .. } => {
                    add(cx - r, cy - r);
                    add(cx + r, cy + r);
                }
                Prim::Poly { pts, .. } => pts.iter().for_each(|(x, y)| add(*x, *y)),
            }
        }
        b
    }

    /// Every shipped icon parses, draws something, and stays inside its viewBox. A shape
    /// outside the viewBox is clipped away on a real SVG renderer and drawn here, so the two
    /// would disagree about what the icon is.
    #[test]
    fn every_shipped_icon_parses_and_fits_its_viewbox() {
        let mut n = 0;
        for k in Kind::ALL {
            let icon = parse(k.svg()).unwrap_or_else(|e| panic!("{k:?}: {e}"));
            assert!(!icon.prims.is_empty(), "{k:?} draws nothing");
            let (x0, y0, x1, y1) = bounds(&icon);
            let [vx, vy, vw, vh] = icon.view;
            assert!(x0 >= vx && y0 >= vy && x1 <= vx + vw && y1 <= vy + vh, "{k:?} leaves its viewBox: {:?}", (x0, y0, x1, y1));
            n += 1;
        }
        for s in [COLLAPSE_SVG, EXPAND_SVG] {
            assert!(!parse(s).unwrap().prims.is_empty());
        }
        assert_eq!(n, Kind::ALL.len());
    }

    /// Unique per keyboard: no two kinds share a drawing.
    #[test]
    fn no_two_kinds_share_an_icon() {
        let icons: Vec<Icon> = Kind::ALL.iter().map(|k| parse(k.svg()).unwrap()).collect();
        for i in 0..icons.len() {
            for j in i + 1..icons.len() {
                assert_ne!(icons[i], icons[j], "{:?} and {:?} draw the same icon", Kind::ALL[i], Kind::ALL[j]);
            }
        }
    }

    /// The boards this app actually ships get the icon that describes them.
    #[test]
    fn the_shipped_boards_get_their_own_kinds() {
        assert_eq!(Kind::for_layout("en_us_qwerty"), Kind::Qwerty);
        assert_eq!(Kind::for_layout("English (QWERTY)"), Kind::Qwerty);
        assert_eq!(Kind::for_layout("English (Dvorak — Ortholinear)"), Kind::Ortho);
        assert_eq!(Kind::for_layout("symbols"), Kind::Symbols);
        assert_eq!(Kind::for_layout("cardinal"), Kind::Cardinal);
        assert_eq!(Kind::for_layout("cardinal8"), Kind::Cardinal8);
        assert_eq!(Kind::for_layout("English (Cardinal Grid)"), Kind::Cardinal);
        assert_eq!(Kind::for_layout("Stenotype"), Kind::Steno);
        assert_eq!(Kind::for_layout("morse"), Kind::Morse);
        assert_eq!(Kind::for_layout("slide"), Kind::Slide);
        assert_eq!(Kind::for_layout("Klingon"), Kind::Generic);
        // And the demo boards' own names, from the code, not from memory.
        let b = crate::Board::new();
        let kinds: Vec<Kind> = b.layout_names().iter().map(|n| Kind::for_layout(n)).collect();
        assert!(!kinds.contains(&Kind::Generic), "a built-in board fell through to Generic: {:?}", b.layout_names());
        let mut uniq = kinds.clone();
        uniq.sort_by_key(|k| *k as u8);
        uniq.dedup();
        assert_eq!(uniq.len(), kinds.len(), "two built-in boards share an icon: {:?}", b.layout_names());
    }

    /// The subset, and its refusals.
    #[test]
    fn the_path_subset_handles_relative_and_implicit_linetos() {
        let icon = parse(r#"<svg viewBox="0 0 10 10"><path d="M1 1 l2 0 2,2 h1 v-1 Z M5-0.5L6 6"/></svg>"#).unwrap();
        assert_eq!(icon.prims.len(), 2);
        assert_eq!(
            icon.prims[0],
            Prim::Poly { pts: vec![(1.0, 1.0), (3.0, 1.0), (5.0, 3.0), (6.0, 3.0), (6.0, 2.0)], closed: true }
        );
        assert_eq!(icon.prims[1], Prim::Poly { pts: vec![(5.0, -0.5), (6.0, 6.0)], closed: false });
        // A fill on the element wins over the root's stroke-only default.
        let icon = parse(r#"<svg fill="none"><circle cx="1" cy="2" r="3" fill="currentColor"/></svg>"#).unwrap();
        assert_eq!(icon.prims[0], Prim::Circle { cx: 1.0, cy: 2.0, r: 3.0, fill: true });
        // Outside the subset is an error, never a silently missing shape.
        assert!(parse(r#"<svg><path d="M0 0 C1 1 2 2 3 3"/></svg>"#).is_err());
        assert!(parse(r#"<svg><ellipse cx="1" cy="1" rx="1" ry="1"/></svg>"#).is_err());
        assert!(parse(r#"<svg><rect width="x" height="1"/></svg>"#).is_err());
        assert!(parse("<rect width='1' height='1'/>").is_err(), "no root");
    }
}
