//! The dots: what to show, for how long, and when the frames count. A 3x3 grid first (the corners and edges are where a gaze
//! model is worst, and the centre is the one it would guess), shuffled, then random dots in between. Each dot gets a settling
//! time for the eye to arrive (no frames kept: a saccade takes a fifth of a second and the head follows after) and a hold
//! during which every frame is kept.

use crate::rng::Rng;

pub const SETTLE_SECS: f32 = 0.7;
pub const HOLD_SECS: f32 = 1.1;
/// Dots stay this far in from the screen edge (fractions).
pub const MARGIN: f32 = 0.08;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    pub id: u32,
    pub pos: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub dots: Vec<Dot>,
}

/// Where the schedule is at a moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub dot: Dot,
    /// Are frames being kept (the settling is over).
    pub capturing: bool,
    /// 0..1 through this dot's time, for a shrinking ring.
    pub progress: f32,
}

impl Schedule {
    pub fn new(seed: u64, extra: usize) -> Schedule {
        let mut rng = Rng::new(seed);
        let g = [MARGIN + 0.02, 0.5, 1.0 - MARGIN - 0.02];
        let mut grid: Vec<[f32; 2]> = g.iter().flat_map(|&y| g.iter().map(move |&x| [x, y])).collect();
        rng.shuffle(&mut grid);
        let mut pos = grid;
        for _ in 0..extra { pos.push([MARGIN + rng.unit() as f32 * (1.0 - 2.0 * MARGIN), MARGIN + rng.unit() as f32 * (1.0 - 2.0 * MARGIN)]); }
        Schedule { dots: pos.into_iter().enumerate().map(|(i, p)| Dot { id: i as u32, pos: p }).collect() }
    }

    pub fn per_dot_secs() -> f32 { SETTLE_SECS + HOLD_SECS }

    pub fn total_secs(&self) -> f32 { self.dots.len() as f32 * Self::per_dot_secs() }

    /// None once the schedule is over (or before it starts).
    pub fn at(&self, t: f32) -> Option<Step> {
        if t < 0.0 || !t.is_finite() { return None; }
        let i = (t / Self::per_dot_secs()) as usize;
        let dot = *self.dots.get(i)?;
        let into = t - i as f32 * Self::per_dot_secs();
        Some(Step { dot, capturing: into >= SETTLE_SECS, progress: into / Self::per_dot_secs() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_covers_the_screen_once_each_before_the_random_dots() {
        let s = Schedule::new(4, 6);
        assert_eq!(s.dots.len(), 15);
        let mut grid: Vec<[i32; 2]> = s.dots[..9].iter().map(|d| [(d.pos[0] * 10.0).round() as i32, (d.pos[1] * 10.0).round() as i32]).collect();
        grid.sort();
        grid.dedup();
        assert_eq!(grid.len(), 9, "nine distinct grid points");
        assert!(s.dots[..9].iter().any(|d| d.pos[0] < 0.15 && d.pos[1] < 0.15), "a corner is in it");
        assert!(s.dots[..9].iter().any(|d| d.pos == [0.5, 0.5]), "and the centre");
        assert!(s.dots.iter().enumerate().all(|(i, d)| d.id == i as u32));
    }

    #[test]
    fn every_dot_stays_inside_the_margin_and_the_order_is_repeatable_per_seed() {
        for seed in 0..20 {
            for d in &Schedule::new(seed, 30).dots {
                assert!(d.pos.iter().all(|&v| (MARGIN..=1.0 - MARGIN).contains(&v)), "{d:?}");
            }
        }
        assert_eq!(Schedule::new(7, 5), Schedule::new(7, 5));
        assert_ne!(Schedule::new(7, 5).dots[0], Schedule::new(8, 5).dots[0], "different seeds shuffle differently");
    }

    #[test]
    fn frames_only_count_after_the_settling_and_the_schedule_ends() {
        let s = Schedule::new(1, 0);
        assert!((s.total_secs() - 9.0 * 1.8).abs() < 1e-4);
        let first = s.at(0.0).unwrap();
        assert!(!first.capturing && first.progress == 0.0);
        assert!(!s.at(SETTLE_SECS - 0.01).unwrap().capturing, "still settling");
        assert!(s.at(SETTLE_SECS + 0.01).unwrap().capturing, "now it counts");
        let second = s.at(Schedule::per_dot_secs() + 0.05).unwrap();
        assert_eq!(second.dot.id, 1);
        assert!(!second.capturing, "a new dot starts settling again");
        assert!(s.at(s.total_secs() + 0.1).is_none() && s.at(-1.0).is_none() && s.at(f32::NAN).is_none());
        let last = s.at(s.total_secs() - 0.01).unwrap();
        assert_eq!(last.dot.id, 8);
    }
}
