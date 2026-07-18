//! origami.rs — slice 10: the folded-paper CARD MODELS, ported from
//! origami.py's draw_part. Units stop being generic diamonds: they render as
//! their actual card models — the same recipes cards.json ships and the pack
//! editor edits — drawn with the papercraft facet language.
//!
//! Geometry is a faithful port of the Python part vocabulary:
//!   builtin  slab · panel · shell · wedge · wing · spine · leg · crest ·
//!            tusk · tail · orb
//!   JSON     every part in parts.json (poly / prism / lines / disc, with
//!            raised / pair / thick / tone_mult), so the outfit rack — robe,
//!            wizardhat, staff, sleeve, hood — folds here too.
//!
//! Prisms extrude screen-space polygons upward with lambert-lit walls and a
//! lit top (game.py draw_prism), back-facing walls culled by the same
//! ny < -0.35 rule. Static pose (flap 0, breathe fixed) — frames, not film.

use crate::render::{mix, rgb, shade, Frame};
use std::collections::BTreeMap;
use std::path::Path;

// ── model data ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ModelPart {
    pub part: String,
    pub at: (f64, f64),
    pub size: f64,
    pub h: f64,
    pub ang: f64,
    pub tone: f32,
}

#[derive(Debug, Clone)]
pub struct CardModel {
    pub paper: [f32; 3],
    pub parts: Vec<ModelPart>,
}

#[derive(Debug, Clone)]
pub struct JsonPart {
    pub kind: String, // poly | prism | lines | disc
    pub points: Vec<(f64, f64)>,
    pub radius: f64,
    pub raised: bool,
    pub pair: bool,
    pub thick: f64,
    pub tone_mult: f32,
}

pub struct ModelBook {
    pub models: BTreeMap<String, CardModel>,
    pub json_parts: BTreeMap<String, JsonPart>,
}

pub const BUILTIN_PARTS: &[&str] = &[
    "slab", "panel", "shell", "wedge", "wing", "spine", "leg", "crest", "tusk", "tail", "orb",
];

impl ModelBook {
    /// Load cards.json models + parts.json parts from the game directory.
    pub fn load(game_dir: &Path) -> Result<Self, String> {
        let cards_raw = std::fs::read_to_string(game_dir.join("cards.json"))
            .map_err(|e| format!("cards.json: {e}"))?;
        let cards: serde_json::Value =
            serde_json::from_str(&cards_raw).map_err(|e| e.to_string())?;
        let parts_raw = std::fs::read_to_string(game_dir.join("parts.json"))
            .map_err(|e| format!("parts.json: {e}"))?;
        let parts: serde_json::Value =
            serde_json::from_str(&parts_raw).map_err(|e| e.to_string())?;

        let mut json_parts = BTreeMap::new();
        for (k, p) in parts["parts"].as_object().into_iter().flatten() {
            let pts = p["points"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|q| {
                            Some((q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))
                        })
                        .collect()
                })
                .unwrap_or_default();
            json_parts.insert(
                k.clone(),
                JsonPart {
                    kind: p["kind"].as_str().unwrap_or("poly").into(),
                    points: pts,
                    radius: p["radius"].as_f64().unwrap_or(0.3),
                    raised: p["raised"].as_bool().unwrap_or(false),
                    pair: p["pair"].as_bool().unwrap_or(false),
                    thick: p["thick"].as_f64().unwrap_or(2.0),
                    tone_mult: p["tone_mult"].as_f64().unwrap_or(1.0) as f32,
                },
            );
        }

        let mut models = BTreeMap::new();
        for (k, c) in cards["cards"].as_object().into_iter().flatten() {
            let Some(model) = c["model"].as_array() else {
                continue;
            };
            let paper = c["paper"]
                .as_array()
                .and_then(|a| {
                    Some([
                        a.first()?.as_f64()? as f32 / 255.0,
                        a.get(1)?.as_f64()? as f32 / 255.0,
                        a.get(2)?.as_f64()? as f32 / 255.0,
                    ])
                })
                .unwrap_or([0.8, 0.75, 0.66]);
            let parts: Vec<ModelPart> = model
                .iter()
                .filter_map(|p| {
                    let name = p["part"].as_str()?.to_string();
                    let known = BUILTIN_PARTS.contains(&name.as_str())
                        || json_parts.contains_key(&name);
                    if !known {
                        return None; // fold_sanitize's defensiveness
                    }
                    let at = p["at"]
                        .as_array()
                        .and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)))
                        .unwrap_or((0.0, 0.0));
                    Some(ModelPart {
                        part: name,
                        at,
                        size: p["size"].as_f64().unwrap_or(1.0),
                        h: p["h"].as_f64().unwrap_or(0.45),
                        ang: p["ang"].as_f64().unwrap_or(0.0),
                        tone: p["tone"].as_f64().unwrap_or(1.0) as f32,
                    })
                })
                .collect();
            models.insert(k.clone(), CardModel { paper, parts });
        }
        Ok(ModelBook { models, json_parts })
    }
}

// ── screen-space drawing helpers on the Frame ───────────────────────────────

fn fill_poly(f: &mut Frame, pts: &[(f64, f64)], col: [f32; 3]) {
    if pts.len() < 3 {
        return;
    }
    for i in 1..pts.len() - 1 {
        f.fill_tri([pts[0], pts[i], pts[i + 1]], col);
    }
}

fn outline(f: &mut Frame, pts: &[(f64, f64)], col: [f32; 3]) {
    for i in 0..pts.len() {
        line(f, pts[i], pts[(i + 1) % pts.len()], col, 1.0);
    }
}

fn line(f: &mut Frame, a: (f64, f64), b: (f64, f64), col: [f32; 3], w: f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (nx, ny) = (-dy / len * w / 2.0, dx / len * w / 2.0);
    fill_poly(
        f,
        &[
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny),
        ],
        col,
    );
}

fn circle(f: &mut Frame, c: (f64, f64), r: f64, col: [f32; 3]) {
    let seg = 10;
    let pts: Vec<(f64, f64)> = (0..seg)
        .map(|i| {
            let a = i as f64 / seg as f64 * std::f64::consts::TAU;
            (c.0 + a.cos() * r, c.1 + a.sin() * r)
        })
        .collect();
    fill_poly(f, &pts, col);
}

/// game.py draw_prism: extrude a screen polygon up by h; lambert walls, lit
/// top, back-facing walls (ny < -0.35) culled.
fn prism(f: &mut Frame, pts: &[(f64, f64)], h: f64, col: [f32; 3]) -> Vec<(f64, f64)> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let cx: f64 = pts.iter().map(|p| p.0).sum::<f64>() / n as f64;
    let cy: f64 = pts.iter().map(|p| p.1).sum::<f64>() / n as f64;
    let mut edges: Vec<((f64, f64), (f64, f64))> =
        (0..n).map(|i| (pts[i], pts[(i + 1) % n])).collect();
    edges.sort_by(|a, b| (a.0 .1 + a.1 .1).partial_cmp(&(b.0 .1 + b.1 .1)).unwrap());
    for (a, b) in edges {
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let (mut nx, mut ny) = (ey, -ex);
        let (mx, my) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        if (mx - cx) * nx + (my - cy) * ny < 0.0 {
            nx = -nx;
            ny = -ny;
        }
        let ln = (nx * nx + ny * ny).sqrt().max(1e-6);
        let (nx, ny) = (nx / ln, ny / ln);
        if ny < -0.35 {
            continue; // hidden behind the lit top
        }
        // lambert vs a sun from the upper-left (game.py's light feel)
        let lit = (0.72 + 0.28 * ((-nx * 0.6 + ny * 0.8).max(0.0))) as f32;
        fill_poly(
            f,
            &[a, b, (b.0, b.1 - h), (a.0, a.1 - h)],
            shade(col, lit * 0.9),
        );
    }
    let top: Vec<(f64, f64)> = pts.iter().map(|p| (p.0, p.1 - h)).collect();
    fill_poly(f, &top, shade(col, 1.06));
    outline(f, &top, shade(col, 0.62));
    top
}

// ── the part renderer ───────────────────────────────────────────────────────

/// Draw one model part at screen (x, y) with body scale `s`, facing `fdir`
/// (+1 right / −1 left), on paper colour `base`. `hgt` is the model's ground
/// height budget (origami.py's hgt).
#[allow(clippy::too_many_arguments)]
pub fn draw_part(
    frame: &mut Frame,
    book: &ModelBook,
    part: &ModelPart,
    x: f64,
    y: f64,
    s: f64,
    fdir: f64,
    base: [f32; 3],
    hgt: f64,
) {
    let ps = part.size.max(0.05) * s;
    let ph = part.h * hgt * 1.6;
    let col = shade(base, part.tone);
    let cx = x + part.at.0 * s * fdir;
    let cy = y + part.at.1 * s;
    let (ca, sa) = (part.ang.cos(), part.ang.sin());
    match part.part.as_str() {
        "slab" => {
            let body = vec![
                (cx - ps * fdir, cy - ps * 0.28),
                (cx + ps * 0.9 * fdir, cy - ps * 0.32),
                (cx + ps * fdir, cy + ps * 0.34),
                (cx - ps * 1.05 * fdir, cy + ps * 0.36),
            ];
            let top = prism(frame, &body, ph, col);
            // the fold crease down the lit top
            if top.len() == 4 {
                let a = ((top[0].0 + top[3].0) / 2.0, (top[0].1 + top[3].1) / 2.0);
                let b = ((top[1].0 + top[2].0) / 2.0, (top[1].1 + top[2].1) / 2.0);
                line(frame, a, b, shade(col, 0.7), 1.0);
            }
        }
        "panel" => {
            let quad = vec![
                (cx - ps * fdir, cy - ps * 0.3),
                (cx + ps * fdir, cy - ps * 0.32),
                (cx + ps * fdir, cy + ps * 0.3),
                (cx - ps * fdir, cy + ps * 0.3),
            ];
            fill_poly(frame, &quad, col);
            outline(frame, &quad, shade(col, 0.62));
        }
        "shell" => {
            let dome = vec![
                (cx - ps * fdir, cy + ps * 0.3),
                (cx - ps * 0.55 * fdir, cy - ps * 0.5),
                (cx + ps * 0.55 * fdir, cy - ps * 0.5),
                (cx + ps * fdir, cy + ps * 0.3),
            ];
            prism(frame, &dome, ph, col);
        }
        "wedge" => {
            let tip = (cx + ps * 0.9 * ca * fdir, cy - ph - ps * 0.5 + ps * 0.9 * sa);
            let tri = vec![
                (cx - ps * 0.4 * fdir, cy - ph + ps * 0.1),
                tip,
                (cx + ps * 0.5 * fdir, cy - ph + ps * 0.2),
            ];
            fill_poly(frame, &tri, col);
            outline(frame, &tri, shade(col, 0.62));
        }
        "wing" => {
            let dir = if part.ang >= 0.0 { 0.6 } else { -0.6 };
            let tip = (cx + ps * dir * fdir, cy - ps - ph);
            let w = vec![
                (cx, cy - ph * 0.4),
                tip,
                (cx + ps * 0.5 * fdir, cy - ps * 0.1 - ph * 0.4),
            ];
            fill_poly(frame, &w, shade(base, part.tone * 0.96));
            outline(frame, &w, shade(base, part.tone * 0.6));
        }
        "spine" => {
            let n = 3;
            for i in 0..n {
                let px = cx + (i as f64 - (n as f64 - 1.0) / 2.0) * ps * 0.5 * fdir;
                let tri = vec![
                    (px, cy - ph),
                    (px + ps * 0.18 * fdir, cy - ph - ps * 0.5),
                    (px + ps * 0.36 * fdir, cy - ph),
                ];
                fill_poly(frame, &tri, col);
            }
        }
        "leg" => {
            let leg = vec![
                (cx - ps * 0.18 * fdir, cy),
                (cx + ps * 0.18 * fdir, cy),
                (cx + ps * 0.12 * fdir, cy + ps * 0.5),
                (cx - ps * 0.12 * fdir, cy + ps * 0.5),
            ];
            fill_poly(frame, &leg, shade(base, part.tone * 0.6));
        }
        "crest" => {
            let crest = vec![
                (cx - ps * fdir, cy + ps * 0.1 - ph),
                (cx, cy - ps * 0.7 - ph),
                (cx + ps * fdir, cy + ps * 0.1 - ph),
            ];
            prism(frame, &crest, ph * 0.7, col);
        }
        "tusk" => {
            for dxo in [-0.12, 0.12] {
                let a = (cx + dxo * ps * fdir, cy - ph);
                let b = (
                    a.0 + ps * 0.7 * ca * fdir,
                    a.1 - ps * 0.5 * (1.0 - sa.abs()) - ps * 0.3 * sa,
                );
                line(frame, a, b, rgb(0xEAE4D2), (ps * 0.12).max(2.0));
            }
        }
        "tail" => {
            let tail = vec![
                (cx, cy - ph * 0.4),
                (cx - ps * ca * fdir, cy - ph * 0.4 - ps * 0.5 * sa),
                (cx - ps * 0.7 * ca * fdir, cy + ps * 0.1),
            ];
            fill_poly(frame, &tail, col);
            outline(frame, &tail, shade(col, 0.62));
        }
        "orb" => circle(frame, (cx, cy - ph), ps.max(2.0), col),
        name => {
            let Some(spec) = book.json_parts.get(name) else {
                return;
            };
            let col2 = shade(base, part.tone * spec.tone_mult);
            let base_y = if spec.raised { cy - ph } else { cy };
            let mirrors: &[f64] = if spec.pair { &[1.0, -1.0] } else { &[1.0] };
            for &mir in mirrors {
                let tx = |p: (f64, f64)| {
                    let (lx, ly) = (p.0 * ps * mir, p.1 * ps);
                    (
                        cx + (lx * ca - ly * sa) * fdir,
                        base_y + lx * sa + ly * ca,
                    )
                };
                match spec.kind.as_str() {
                    "disc" => circle(frame, tx((0.0, 0.0)), (spec.radius * ps).max(2.0), col2),
                    "lines" => {
                        for wnd in spec.points.windows(2) {
                            line(frame, tx(wnd[0]), tx(wnd[1]), col2, spec.thick.max(1.0));
                        }
                    }
                    "prism" => {
                        let pts: Vec<(f64, f64)> = spec.points.iter().map(|&p| tx(p)).collect();
                        prism(frame, &pts, ph, col2);
                    }
                    _ => {
                        let pts: Vec<(f64, f64)> = spec.points.iter().map(|&p| tx(p)).collect();
                        fill_poly(frame, &pts, col2);
                        outline(frame, &pts, shade(col2, 0.62));
                    }
                }
            }
        }
    }
}

/// Draw a whole card model at screen (x, y), body scale `s`, team-tinted.
pub fn draw_model(
    frame: &mut Frame,
    book: &ModelBook,
    card_key: &str,
    x: f64,
    y: f64,
    s: f64,
    fdir: f64,
    team_tint: [f32; 3],
) -> bool {
    let Some(model) = book.models.get(card_key) else {
        return false;
    };
    let base = mix(model.paper, team_tint, 0.28);
    for part in &model.parts {
        draw_part(frame, book, part, x, y, s, fdir, base, s * 0.55);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_whole_vocabulary_loads() {
        let mb = ModelBook::load(&game_dir()).unwrap();
        // the outfit rack and the classics are all present
        for p in ["robe", "wizardhat", "staff", "sleeve", "hood", "sail", "halo", "claw"] {
            assert!(mb.json_parts.contains_key(p), "missing json part {p}");
        }
        // humanoids + upgraded models carry full recipes
        for (k, min_parts) in [("mothmage", 8), ("foldknight", 6), ("inkwitch", 5), ("phoenix", 6)] {
            let m = mb.models.get(k).unwrap_or_else(|| panic!("no model for {k}"));
            assert!(m.parts.len() >= min_parts, "{k}: {} parts", m.parts.len());
        }
    }

    #[test]
    fn models_render_ink_onto_the_page() {
        let mb = ModelBook::load(&game_dir()).unwrap();
        let mut f = Frame::new(400, 300);
        f.clear(rgb(0xC4B294));
        for (i, key) in ["mothmage", "foldknight", "inkwitch"].iter().enumerate() {
            let ok = draw_model(&mut f, &mb, key, 70.0 + i as f64 * 130.0, 200.0, 42.0, 1.0,
                                rgb(0x8C7CC8));
            assert!(ok, "{key} must draw");
        }
        // coverage: the canvas is no longer flat paper
        let mut set = std::collections::HashSet::new();
        for p in f.px.chunks_exact(4).step_by(13) {
            set.insert((p[0], p[1], p[2]));
        }
        assert!(set.len() > 25, "only {} colours — models missing", set.len());
        // an unknown card declines politely
        assert!(!draw_model(&mut f, &mb, "no_such_card", 0.0, 0.0, 10.0, 1.0, rgb(0xFFFFFF)));
    }
}
