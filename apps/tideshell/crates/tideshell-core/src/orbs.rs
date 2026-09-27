//! Depth orbs: the lock screen's "objects tracked in 3D", without a picture.
//!
//! The cover shows the room as a colour wash (camera.rs: no recognisable
//! image ever reaches a lock screen). On top of it, the corners the tracker
//! follows become soft orbs. Each orb's DEPTH comes from motion parallax,
//! the oldest trick in structure-from-motion: when the phone moves, near
//! things slide across the frame more than far ones. So an orb's apparent
//! depth is how far its own flow departs from the median (the far
//! background's) flow. Near orbs are drawn larger, brighter, and keep
//! drifting with the phone after the camera stops; far ones sit small and dim.
//!
//! Cost: the tracker already runs on a 240x320 grayscale frame during the
//! camera's short live window. This model is a few dozen floats. When the
//! live window ends the orbs settle and stop changing, so the cover needs no
//! frames for them.

/// Orbs kept at most (the tracker yields up to 48 corners; half is plenty).
pub const MAX_ORBS: usize = 24;
/// How quickly an orb's position and depth follow new readings (per update).
pub const FOLLOW: f32 = 0.45;
/// Updates an orb survives without being matched before it is dropped.
pub const MISS_LIMIT: u8 = 3;
/// Resting opacity once the live window is over.
pub const REST_ALPHA: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orb {
    /// Position in the frame, normalised 0..1.
    pub x: f32,
    pub y: f32,
    /// 0 = far (moves with the background), 1 = near.
    pub depth: f32,
    /// 0..1, fading in on birth and out when lost.
    pub alpha: f32,
    pub color: [u8; 3],
    missed: u8,
}

impl Orb {
    /// Screen radius for a pane whose short side is `short` points.
    pub fn radius(&self, short: f32) -> f32 {
        short * (0.018 + 0.05 * self.depth)
    }
}

/// One tracked corner between two live frames: where it was, where it went
/// (frame pixels), and the room's colour there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub color: [u8; 3],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Orbs {
    pub orbs: Vec<Orb>,
    /// Whether the camera is live (orbs animate) or resting.
    pub live: bool,
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

impl Orbs {
    /// A live frame's tracks, in a `w`x`h` frame.
    pub fn update(&mut self, tracks: &[Track], w: f32, h: f32) {
        self.live = true;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (mx, my) = (median(tracks.iter().map(|t| t.to.0 - t.from.0).collect()), median(tracks.iter().map(|t| t.to.1 - t.from.1).collect()));
        // Parallax scale: how much the background itself moved, with a floor
        // so a still phone does not turn sensor noise into depth.
        let base = (mx * mx + my * my).sqrt().max(1.5);
        for o in &mut self.orbs {
            o.missed = o.missed.saturating_add(1);
        }
        for t in tracks {
            let (fx, fy) = (t.to.0 - t.from.0 - mx, t.to.1 - t.from.1 - my);
            let depth = ((fx * fx + fy * fy).sqrt() / (base * 2.0)).clamp(0.0, 1.0);
            let (nx, ny) = (t.to.0 / w, t.to.1 / h);
            let (px, py) = (t.from.0 / w, t.from.1 / h);
            // Match to the unmatched orb nearest this track's previous position.
            let mut near: Option<(usize, f32)> = None;
            for (i, o) in self.orbs.iter().enumerate() {
                let d = (o.x - px).hypot(o.y - py);
                if o.missed > 0 && d < 0.06 && near.is_none_or(|(_, best)| d < best) {
                    near = Some((i, d));
                }
            }
            let near = near.map(|(i, _)| i);
            match near {
                Some(i) => {
                    let o = &mut self.orbs[i];
                    // Positions follow the tracker closely; depth is smoothed.
                    o.x += (nx - o.x) * 0.9;
                    o.y += (ny - o.y) * 0.9;
                    o.depth += (depth - o.depth) * FOLLOW;
                    o.alpha = (o.alpha + 0.34).min(1.0);
                    o.color = t.color;
                    o.missed = 0;
                }
                None if self.orbs.len() < MAX_ORBS => self.orbs.push(Orb { x: nx, y: ny, depth, alpha: 0.34, color: t.color, missed: 0 }),
                None => {}
            }
        }
        for o in &mut self.orbs {
            if o.missed > 0 {
                o.alpha = (o.alpha - 0.34).max(0.0);
            }
        }
        self.orbs.retain(|o| o.missed <= MISS_LIMIT && o.alpha > 0.0);
    }

    /// The live window ended: orbs settle where they are, dimmed. Nothing
    /// changes after this until the camera is live again.
    pub fn rest(&mut self) {
        self.live = false;
        for o in &mut self.orbs {
            o.alpha = o.alpha.min(REST_ALPHA);
        }
    }

    /// Unlocked or asleep: forget them (a new room next time).
    pub fn clear(&mut self) {
        self.orbs.clear();
        self.live = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(from: (f32, f32), d: (f32, f32)) -> Track {
        Track { from, to: (from.0 + d.0, from.1 + d.1), color: [200, 180, 150] }
    }

    #[test]
    fn near_things_move_more_than_the_background_and_read_as_nearer() {
        let mut o = Orbs::default();
        // Background: most corners shift by (4, 0); one shifts by (14, 0).
        let mut t: Vec<Track> = (0..8).map(|i| track((20.0 + i as f32 * 25.0, 100.0), (4.0, 0.0))).collect();
        t.push(track((120.0, 250.0), (14.0, 0.0)));
        o.update(&t, 240.0, 320.0);
        let near = o.orbs.iter().max_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap()).unwrap();
        assert!((near.y - 250.0 / 320.0).abs() < 1e-3, "the fast one is the near one");
        assert!(near.depth > 0.9);
        assert!(o.orbs.iter().filter(|x| x.depth < 0.05).count() >= 7, "the background reads as far");
        assert!(near.radius(400.0) > o.orbs[0].radius(400.0));
    }

    #[test]
    fn a_still_phone_does_not_invent_depth_from_noise() {
        let mut o = Orbs::default();
        let t: Vec<Track> = (0..6).map(|i| track((30.0 * i as f32 + 10.0, 50.0), (if i % 2 == 0 { 0.5 } else { -0.5 }, 0.0))).collect();
        o.update(&t, 240.0, 320.0);
        assert!(o.orbs.iter().all(|x| x.depth < 0.4), "{:?}", o.orbs.iter().map(|x| x.depth).collect::<Vec<_>>());
    }

    #[test]
    fn orbs_follow_their_corner_fade_in_and_die_when_lost() {
        let mut o = Orbs::default();
        o.update(&[track((100.0, 100.0), (2.0, 0.0))], 240.0, 320.0);
        assert_eq!(o.orbs.len(), 1);
        let a0 = o.orbs[0].alpha;
        o.update(&[track((102.0, 100.0), (2.0, 0.0))], 240.0, 320.0);
        assert_eq!(o.orbs.len(), 1, "matched, not duplicated");
        assert!(o.orbs[0].alpha > a0, "fading in");
        assert!((o.orbs[0].x - 104.0 / 240.0).abs() < 0.01);
        for _ in 0..=MISS_LIMIT {
            o.update(&[], 240.0, 320.0);
        }
        assert!(o.orbs.is_empty(), "lost for too long");
    }

    #[test]
    fn capped_and_resting_is_dim_and_still() {
        let mut o = Orbs::default();
        let t: Vec<Track> = (0..60).map(|i| track(((i % 10) as f32 * 24.0, (i / 10) as f32 * 50.0), (3.0, 1.0))).collect();
        o.update(&t, 240.0, 320.0);
        assert_eq!(o.orbs.len(), MAX_ORBS);
        o.update(&t.iter().map(|t| track(t.to, (3.0, 1.0))).collect::<Vec<_>>(), 240.0, 320.0);
        o.rest();
        assert!(!o.live && o.orbs.iter().all(|x| x.alpha <= REST_ALPHA));
        let before = o.clone();
        o.rest();
        assert_eq!(o, before, "resting is a fixed point: no frames needed");
        o.clear();
        assert!(o.orbs.is_empty());
    }
}
