//! The system-row glyphs (lock, log out, restart, shut down), drawn as strokes.
//!
//! They used to be looked up in the icon theme as `system-lock-screen`, `system-reboot`... and the
//! phone's theme has none of them, so the row showed a bare letter or nothing. A power glyph does not
//! depend on a theme, so these are polylines in a unit box, scaled to wherever they are drawn.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

fn arc(c: (f32, f32), r: f32, from_deg: f32, to_deg: f32) -> Vec<(f32, f32)> {
    let n = 24;
    (0..=n)
        .map(|i| {
            let a = (from_deg + (to_deg - from_deg) * i as f32 / n as f32).to_radians();
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// Polylines for an action's glyph in a [-1, 1] box (y down). Unknown ids give a dot.
pub fn paths(id: &str) -> Vec<Vec<(f32, f32)>> {
    match id {
        // Open circle with a bar through the gap at the top.
        "poweroff" => vec![arc((0.0, 0.1), 0.78, -60.0, 240.0), vec![(0.0, -0.9), (0.0, -0.05)]],
        // A circular arrow: almost a full turn and a head at its open end.
        "reboot" => {
            let mut a = arc((0.0, 0.0), 0.75, -40.0, 250.0);
            let end = *a.last().unwrap();
            // Head at the end of the arc, pointing along the travel direction.
            let ang = 250f32.to_radians();
            let tangent = (-ang.sin(), ang.cos());
            let normal = (ang.cos(), ang.sin());
            let tip = (end.0 + tangent.0 * 0.28, end.1 + tangent.1 * 0.28);
            let l = (end.0 + normal.0 * 0.28, end.1 + normal.1 * 0.28);
            let r = (end.0 - normal.0 * 0.28, end.1 - normal.1 * 0.28);
            a.shrink_to_fit();
            vec![a, vec![l, tip, r]]
        }
        // A padlock: body, shackle.
        "lock" => vec![
            vec![(-0.6, -0.05), (0.6, -0.05), (0.6, 0.85), (-0.6, 0.85), (-0.6, -0.05)],
            {
                let mut s = vec![(-0.38, -0.05), (-0.38, -0.4)];
                s.extend(arc((0.0, -0.4), 0.38, 180.0, 360.0));
                s.push((0.38, -0.05));
                s
            },
        ],
        // A door frame and an arrow leaving it.
        "logout" => vec![
            vec![(-0.2, -0.8), (-0.8, -0.8), (-0.8, 0.8), (-0.2, 0.8)],
            vec![(-0.3, 0.0), (0.8, 0.0)],
            vec![(0.45, -0.35), (0.8, 0.0), (0.45, 0.35)],
        ],
        _ => vec![vec![(0.0, 0.0), (0.01, 0.0)]],
    }
}

/// Paint `id`'s glyph centred in `rect`.
pub fn paint(p: &Painter, rect: Rect, id: &str, color: Color32) {
    let s = rect.width().min(rect.height()) / 2.0;
    let c = rect.center();
    let stroke = Stroke::new((s * 0.16).max(1.6), color);
    for path in paths(id) {
        let pts: Vec<Pos2> = path.into_iter().map(|(x, y)| c + Vec2::new(x, y) * s).collect();
        p.add(Shape::line(pts, stroke));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_system_action_has_a_glyph_inside_the_box() {
        for a in hatch_core::SYSTEM_ACTIONS {
            let ps = paths(a.id);
            let n: usize = ps.iter().map(|p| p.len()).sum();
            assert!(n >= 4, "{} has real strokes ({n} points)", a.id);
            for (x, y) in ps.iter().flatten() {
                assert!(x.abs() <= 1.01 && y.abs() <= 1.01, "{}: ({x},{y}) is outside the glyph box", a.id);
            }
        }
    }

    #[test]
    fn the_glyphs_differ_from_each_other() {
        let all: Vec<_> = hatch_core::SYSTEM_ACTIONS.iter().map(|a| format!("{:?}", paths(a.id))).collect();
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j], "two actions share a glyph");
            }
        }
    }
}
