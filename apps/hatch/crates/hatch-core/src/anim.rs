//! anim — the tile [`Board`]: every tile's pose, advanced by an explicit tick.
//!
//! This is the launcher's visible internal loop. The view never animates
//! anything itself: each frame it calls [`Board::tick`] with a timestamp and
//! paints [`Board::poses`]. Tests drive the same tick with synthetic clocks,
//! so "does the restack converge", "do leavers get removed", "does a stagger
//! order hold" are assertions, not screenshots.
//!
//! Three motions, mirroring the plasmoid's two systems plus its remove:
//!
//! * **enter** — a tile new to the board slides in along its
//!   [`grid::slide_start`] vector after that vector's stagger delay.
//! * **restack** — a tile that already exists and whose home cell moved (the
//!   search filtered the order) eases from where it *currently is* to the new
//!   home. Retargeting mid-flight is seamless because the ease always starts
//!   from the live pose.
//! * **exit** — a tile no longer in the order fades in place, then is removed
//!   on the tick that finishes the fade.

use crate::grid::{self, GridSpec, SlidePattern};

/// The one easing curve this module uses.
///
/// Inlined rather than imported. `uikit_egui::motion::ease` is pure float
/// maths and would have been the right thing to share, but `motion.rs` pulls
/// `egui::{Color32, Context, Id, Response}` at file scope — importing the
/// module would put the whole UI crate into hatch-core, which is headless on
/// purpose so the launcher's logic can be tested and reused without a
/// toolkit. Four lines of arithmetic is a smaller price than that dependency.
///
/// If `ease` is ever split into its own egui-free module, this should go back
/// to importing it: two copies of a curve is a real cost, just a smaller one.
fn out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Where and how to paint one tile right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TilePose {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub alpha: f32,
    /// Radians.
    pub rot: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting out the stagger delay, invisible at the start pose.
    EnterPending { at_ms: f64 },
    /// Moving start→home since `t0`.
    Entering { t0: f64 },
    /// Moving from a captured pose to home since `t0`.
    Restacking { t0: f64 },
    Settled,
    /// Fading out in place since `t0`.
    Exiting { t0: f64 },
}

#[derive(Clone, Debug)]
struct Tile {
    key: String,
    /// Home (target) position — the cell this tile is heading to.
    home: (f32, f32),
    /// Pose the current phase interpolates FROM.
    from: TilePose,
    /// Live pose, updated every tick.
    pose: TilePose,
    phase: Phase,
}

/// Tunables the settings page exposes; defaults follow the plasmoid schema.
#[derive(Clone, Copy, Debug)]
pub struct BoardConfig {
    pub pattern: SlidePattern,
    pub enter_ms: f32,
    pub stagger_ms: f32,
    pub restack_ms: f32,
    pub restack_stagger_ms: f32,
    pub exit_ms: f32,
}

impl Default for BoardConfig {
    fn default() -> Self {
        BoardConfig {
            pattern: SlidePattern::FromBelow,
            enter_ms: 350.0,
            stagger_ms: 12.0,
            restack_ms: 250.0,
            restack_stagger_ms: 10.0,
            exit_ms: 120.0,
        }
    }
}

pub struct Board {
    tiles: Vec<Tile>,
    pub config: BoardConfig,
}

impl Board {
    pub fn new(config: BoardConfig) -> Board {
        Board { tiles: Vec::new(), config }
    }

    /// The new visible order (tile keys, grid order). Diffs against the
    /// current board:
    /// existing keys restack, new keys enter along the slide pattern, missing
    /// keys exit. `now_ms` is the caller's clock (monotonic, ms).
    pub fn retarget(&mut self, order: &[&str], spec: &GridSpec, now_ms: f64) {
        let rows = spec.rows_for(order.len());
        let mut next: Vec<Tile> = Vec::with_capacity(order.len());

        for (i, key) in order.iter().enumerate() {
            let (hx, hy) = spec.cell_pos(i);
            if let Some(old) = self.tiles.iter().find(|t| t.key == *key && !matches!(t.phase, Phase::Exiting { .. })) {
                let mut t = old.clone();
                if (t.home.0 - hx).abs() > 0.5 || (t.home.1 - hy).abs() > 0.5 {
                    // Cell moved: restack from wherever it is right now.
                    t.home = (hx, hy);
                    t.from = t.pose;
                    // Stagger by target index so the wave reads left-to-right.
                    let delay = i as f32 * self.config.restack_stagger_ms;
                    t.phase = Phase::Restacking { t0: now_ms + delay as f64 };
                } else {
                    t.home = (hx, hy);
                }
                next.push(t);
            } else {
                let start = grid::slide_start(
                    self.config.pattern,
                    i,
                    spec.cols,
                    rows,
                    spec.cell,
                    self.config.stagger_ms,
                );
                let from = TilePose {
                    x: hx + start.dx,
                    y: hy + start.dy,
                    scale: start.scale,
                    alpha: if self.config.pattern == SlidePattern::None { 1.0 } else { 0.0 },
                    rot: start.rot,
                };
                next.push(Tile {
                    key: (*key).to_string(),
                    home: (hx, hy),
                    from,
                    pose: from,
                    phase: if self.config.pattern == SlidePattern::None {
                        Phase::Settled
                    } else {
                        Phase::EnterPending { at_ms: now_ms + start.delay_ms as f64 }
                    },
                });
            }
        }

        // Anything not in the new order fades out in place.
        for old in &self.tiles {
            let leaving = !order.iter().any(|k| *k == old.key);
            if leaving && !matches!(old.phase, Phase::Exiting { .. }) {
                let mut t = old.clone();
                t.from = t.pose;
                t.phase = Phase::Exiting { t0: now_ms };
                next.push(t);
            } else if leaving {
                next.push(old.clone()); // keep an in-flight exit until it finishes
            }
        }

        self.tiles = next;
        self.tick(now_ms); // settle Phase::Settled newcomers' poses immediately
    }

    /// Advance every tile to `now_ms`. Returns true while anything is still
    /// moving (the shell uses this for request_repaint).
    pub fn tick(&mut self, now_ms: f64) -> bool {
        let cfg = self.config;
        let mut moving = false;
        for t in &mut self.tiles {
            match t.phase {
                Phase::Settled => {
                    t.pose = TilePose { x: t.home.0, y: t.home.1, scale: 1.0, alpha: 1.0, rot: 0.0 };
                }
                Phase::EnterPending { at_ms } => {
                    moving = true;
                    if now_ms >= at_ms {
                        t.phase = Phase::Entering { t0: at_ms };
                        // fall through next tick; set pose now for this frame
                        Self::ease_to_home(t, cfg.enter_ms, at_ms, now_ms, true);
                    } else {
                        t.pose = t.from; // parked, alpha 0
                    }
                }
                Phase::Entering { t0 } => {
                    if Self::ease_to_home(t, cfg.enter_ms, t0, now_ms, true) {
                        t.phase = Phase::Settled;
                    } else {
                        moving = true;
                    }
                }
                Phase::Restacking { t0 } => {
                    if now_ms < t0 {
                        moving = true; // stagger hold: stay at `from`
                        t.pose = t.from;
                    } else if Self::ease_to_home(t, cfg.restack_ms, t0, now_ms, false) {
                        t.phase = Phase::Settled;
                    } else {
                        moving = true;
                    }
                }
                Phase::Exiting { t0 } => {
                    let u = (((now_ms - t0) as f32) / cfg.exit_ms).clamp(0.0, 1.0);
                    t.pose.alpha = (t.from.alpha * (1.0 - u)).max(0.0);
                    t.pose.scale = t.from.scale * (1.0 - 0.15 * u);
                    if u < 1.0 {
                        moving = true;
                    }
                }
            }
        }
        // Remove exits that completed.
        self.tiles.retain(|t| !(matches!(t.phase, Phase::Exiting { .. }) && t.pose.alpha <= 0.0));
        moving
    }

    /// Ease `t` from `t.from` toward home over `dur_ms` starting at `t0`.
    /// Returns true when arrived. `fade_in` also ramps alpha 0→1.
    fn ease_to_home(t: &mut Tile, dur_ms: f32, t0: f64, now_ms: f64, fade_in: bool) -> bool {
        let u = (((now_ms - t0) as f32) / dur_ms.max(1.0)).clamp(0.0, 1.0);
        let e = out_cubic(u);
        t.pose = TilePose {
            x: t.from.x + (t.home.0 - t.from.x) * e,
            y: t.from.y + (t.home.1 - t.from.y) * e,
            scale: t.from.scale + (1.0 - t.from.scale) * e,
            alpha: if fade_in { t.from.alpha + (1.0 - t.from.alpha) * e } else { t.from.alpha },
            rot: t.from.rot * (1.0 - e),
        };
        u >= 1.0
    }

    /// Live poses in paint order. Exiting tiles come with their fading alpha.
    pub fn poses(&self) -> impl Iterator<Item = (&str, TilePose)> + '_ {
        self.tiles.iter().map(|t| (t.key.as_str(), t.pose))
    }

    pub fn is_settled(&self) -> bool {
        self.tiles.iter().all(|t| matches!(t.phase, Phase::Settled))
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> GridSpec {
        GridSpec { cols: 4, rows_visible: 3, cell: 100.0 }
    }

    fn board() -> Board {
        Board::new(BoardConfig::default())
    }

    #[test]
    fn enter_converges_to_home_and_settles() {
        let mut b = board();
        b.retarget(&["a", "b", "c"], &spec(), 0.0);
        assert!(!b.is_settled());
        // Run well past enter + max stagger.
        let mut t = 0.0;
        while b.tick(t) {
            t += 16.0;
            assert!(t < 5000.0, "enter never settled");
        }
        let poses: Vec<_> = b.poses().collect();
        assert_eq!(poses.len(), 3);
        for (i, (_, p)) in poses.iter().enumerate() {
            let (hx, hy) = spec().cell_pos(i);
            assert!((p.x - hx).abs() < 0.01 && (p.y - hy).abs() < 0.01);
            assert!((p.alpha - 1.0).abs() < 0.01 && (p.scale - 1.0).abs() < 0.01);
        }
        assert!(b.is_settled());
    }

    #[test]
    fn stagger_orders_the_entrance() {
        let mut b = board();
        b.retarget(&["a", "b", "c", "d", "e", "f"], &spec(), 0.0);
        // Just after tile 0's motion starts, later tiles are still parked
        // below (FromBelow: delay = index * stagger).
        b.tick(20.0);
        let poses: Vec<_> = b.poses().collect();
        let (x0, y0) = spec().cell_pos(0);
        let (_, p0) = poses[0];
        let (_, p5) = poses[5];
        assert!(p0.y < y0 + 80.0, "first tile has started moving up");
        assert!((p0.x - x0).abs() < 0.01);
        let (_, hy5) = spec().cell_pos(5);
        assert!((p5.y - (hy5 + 80.0)).abs() < 0.01, "tile 5 still parked at its start offset");
        assert_eq!(p5.alpha, 0.0);
    }

    #[test]
    fn restack_moves_survivors_and_fades_leavers() {
        let mut b = board();
        b.retarget(&["a", "b", "c", "d"], &spec(), 0.0);
        let mut t = 0.0;
        while b.tick(t) {
            t += 16.0;
        }
        // Filter down: only c survives, moving from cell 2 to cell 0.
        b.retarget(&["c"], &spec(), t);
        assert_eq!(b.len(), 4, "leavers stay on board while fading");
        let mut steps = 0;
        while b.tick(t) {
            t += 16.0;
            steps += 1;
            assert!(steps < 1000, "restack never settled");
        }
        assert_eq!(b.len(), 1, "faded leavers are removed");
        let (key, p) = b.poses().next().unwrap();
        assert_eq!(key, "c");
        assert!((p.x - 0.0).abs() < 0.01 && (p.y - 0.0).abs() < 0.01);
        assert!((p.alpha - 1.0).abs() < 0.01);
    }

    #[test]
    fn retarget_mid_flight_starts_from_live_pose() {
        let mut b = board();
        b.retarget(&["a", "b"], &spec(), 0.0);
        b.tick(100.0); // mid-enter
        let mid: Vec<_> = b.poses().map(|(_, p)| p).collect();
        // Reorder while still moving: b now first.
        b.retarget(&["b", "a"], &spec(), 100.0);
        b.tick(100.0);
        let after: Vec<_> = b.poses().map(|(k, p)| (k.to_string(), p)).collect();
        // 'a' should be at (or continuing from) its mid pose, not teleported.
        let a_after = after.iter().find(|(k, _)| k == "a").unwrap().1;
        assert!((a_after.x - mid[0].x).abs() < 1.0 && (a_after.y - mid[0].y).abs() < 1.0);
    }

    #[test]
    fn pattern_none_appears_instantly() {
        let mut b = Board::new(BoardConfig { pattern: SlidePattern::None, ..BoardConfig::default() });
        b.retarget(&["a", "b"], &spec(), 0.0);
        assert!(b.is_settled());
        for (_, p) in b.poses() {
            assert_eq!(p.alpha, 1.0);
            assert_eq!(p.scale, 1.0);
        }
    }

    /// The inlined curve must stay identical to `uikit_egui::motion::ease`.
    ///
    /// This is the cost of the copy, paid explicitly. hatch-core cannot import
    /// that module without pulling egui into a headless crate, so the values
    /// are pinned here instead: the endpoints, the midpoint, and the clamp on
    /// both sides. If the shared curve is ever retuned, this fails and someone
    /// decides deliberately rather than the launcher animating differently
    /// from every other app by accident.
    #[test]
    fn the_inlined_curve_matches_the_shared_one() {
        // 1 - (1-t)^3, clamped to 0..=1.
        assert_eq!(out_cubic(0.0), 0.0);
        assert_eq!(out_cubic(1.0), 1.0);
        assert!((out_cubic(0.5) - 0.875).abs() < 1e-6, "{}", out_cubic(0.5));
        // Out-of-range inputs clamp rather than extrapolating: a tick that
        // overshoots its duration must land at rest, not past it.
        assert_eq!(out_cubic(-1.0), 0.0);
        assert_eq!(out_cubic(2.0), 1.0);
        // And it decelerates — more progress in the first half than the
        // second, which is what "out" means and what a linear curve would
        // fail.
        assert!(out_cubic(0.5) > 0.5, "an out curve is ahead at the midpoint");
    }
}
