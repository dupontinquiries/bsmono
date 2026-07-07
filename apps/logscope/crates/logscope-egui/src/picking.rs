//! picking.rs — screen-space hit-testing for the PHYSICAL UI.
//!
//! "Physical UI adoption" means in-scene 3D objects ARE the controls: you hover a node to reveal its image,
//! press the pearl to talk, click a shot to scrub it — no 2D chrome layer floating on top. The one thing all
//! of that needs is: given where each object landed on screen this frame (from the very same projection the
//! renderer already computes into `proj[]`) and the cursor, decide what's under the pointer. That's this
//! module. It's pure — projected screen coords in, a picked handle out — so it unit-tests without egui, a
//! window, or the GPU, and the same call resolves hover, click, and drag-start.

/// A pickable in-scene object as it landed on screen this frame.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    /// Caller's handle — a node id, a control id, whatever it wants back.
    pub id: usize,
    /// Projected screen position in pixels (the object's centre).
    pub screen: [f32; 2],
    /// Pick radius in pixels — how big the object reads on screen (grows for near objects, shrinks for far).
    pub radius: f32,
    /// Camera-space depth; smaller = nearer the camera. Breaks ties so the object visually ON TOP wins.
    pub depth: f32,
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    dx * dx + dy * dy
}

/// The object under `cursor`: among every target whose pixel disc contains the cursor, the FRONT-MOST one
/// (smallest `depth`), ties broken by whichever centre is closest to the cursor. Returns that target's `id`,
/// or `None` if the cursor is over empty space. Front-most-wins matches occlusion — you pick what's drawn on
/// top, exactly like clicking the visible object.
pub fn pick(targets: &[Target], cursor: [f32; 2]) -> Option<usize> {
    let mut best: Option<(&Target, f32)> = None; // (target, dist²)
    for t in targets {
        if t.radius <= 0.0 || !t.screen[0].is_finite() || !t.screen[1].is_finite() {
            continue;
        }
        let d2 = dist2(t.screen, cursor);
        if d2 > t.radius * t.radius {
            continue; // cursor outside this object's disc
        }
        match best {
            None => best = Some((t, d2)),
            Some((b, bd2)) => {
                // nearer to camera wins; on a depth tie, the closer centre wins
                if t.depth < b.depth - 1e-4 || ((t.depth - b.depth).abs() <= 1e-4 && d2 < bd2) {
                    best = Some((t, d2));
                }
            }
        }
    }
    best.map(|(t, _)| t.id)
}

/// Like [`pick`] but grows every disc by `slop` px first — a gentler hover target (fat-finger / trackpad
/// tolerance) without changing the drawn size. `slop <= 0` is identical to [`pick`].
pub fn pick_slop(targets: &[Target], cursor: [f32; 2], slop: f32) -> Option<usize> {
    if slop <= 0.0 {
        return pick(targets, cursor);
    }
    let grown: Vec<Target> = targets.iter().map(|t| Target { radius: t.radius + slop, ..*t }).collect();
    pick(&grown, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: usize, x: f32, y: f32, r: f32, depth: f32) -> Target {
        Target { id, screen: [x, y], radius: r, depth }
    }

    #[test]
    fn picks_the_object_under_the_cursor() {
        let ts = [t(10, 100.0, 100.0, 20.0, 5.0), t(11, 300.0, 100.0, 20.0, 5.0)];
        assert_eq!(pick(&ts, [105.0, 98.0]), Some(10));
        assert_eq!(pick(&ts, [298.0, 103.0]), Some(11));
    }

    #[test]
    fn empty_space_picks_nothing() {
        let ts = [t(10, 100.0, 100.0, 20.0, 5.0)];
        assert_eq!(pick(&ts, [200.0, 200.0]), None);
        assert_eq!(pick(&[], [0.0, 0.0]), None);
    }

    #[test]
    fn edge_is_inside_but_just_outside_is_not() {
        let ts = [t(10, 0.0, 0.0, 10.0, 1.0)];
        assert_eq!(pick(&ts, [10.0, 0.0]), Some(10)); // exactly on the radius counts
        assert_eq!(pick(&ts, [10.1, 0.0]), None);
    }

    #[test]
    fn overlapping_targets_front_most_wins() {
        // a big far object and a small near object both under the cursor → the near one is picked
        let far = t(1, 100.0, 100.0, 60.0, 40.0);
        let near = t(2, 100.0, 100.0, 12.0, 3.0);
        assert_eq!(pick(&[far, near], [100.0, 100.0]), Some(2));
        assert_eq!(pick(&[near, far], [100.0, 100.0]), Some(2), "order-independent");
    }

    #[test]
    fn depth_tie_breaks_on_closest_centre() {
        let a = t(1, 90.0, 100.0, 30.0, 5.0);
        let b = t(2, 110.0, 100.0, 30.0, 5.0); // same depth, both cover cursor at 105
        assert_eq!(pick(&[a, b], [105.0, 100.0]), Some(2), "cursor is nearer b's centre");
    }

    #[test]
    fn non_finite_projection_is_ignored() {
        // a node projected behind the camera (non-finite screen) must never be picked
        let bad = Target { id: 9, screen: [f32::NAN, f32::NAN], radius: 50.0, depth: -1.0 };
        let good = t(3, 100.0, 100.0, 20.0, 5.0);
        assert_eq!(pick(&[bad, good], [100.0, 100.0]), Some(3));
    }

    #[test]
    fn slop_widens_the_hover_target() {
        let ts = [t(7, 0.0, 0.0, 10.0, 1.0)];
        assert_eq!(pick(&ts, [14.0, 0.0]), None);           // outside the 10px disc
        assert_eq!(pick_slop(&ts, [14.0, 0.0], 6.0), Some(7)); // inside once grown to 16px
    }
}
