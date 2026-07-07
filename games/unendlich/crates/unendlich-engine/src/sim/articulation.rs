//! Articulation — a branching skeleton of stick-slip [`Joint`]s. This is the
//! tree-dragon's body: a spine of segments with branches jutting off at various
//! points, each segment a joint with its own [`JointMaterial`]. Because the joints
//! are angle-dependent stick-slip, the whole creature behaves like something
//! made of wood or rusted iron — under a drive it doesn't smoothly pose, it
//! *unblocks*, joint by joint, freeing the easy ones first and straining against
//! the seized ones. Curling and mobilising emerge from the material, not from
//! authored animation.
//!
//! Topology and joint dynamics only (forward kinematics to world positions is the
//! render layer's job, and needs trig the fixed-point sim avoids). Deterministic.

use crate::sim::fixed::Fixed;
use crate::sim::joint::{Joint, JointMaterial, Phase};

/// One bone: a joint, a rest length, and its parent in the tree (`None` = root).
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub parent: Option<usize>,
    pub length: Fixed,
    pub joint: Joint,
}

/// What a drive step did to the whole skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DriveReport {
    /// Joints that moved this step.
    pub moving: usize,
    /// Joints held by friction this step.
    pub stuck: usize,
    /// Joints that broke free from rest this step.
    pub freed: usize,
}

/// A branching skeleton.
#[derive(Debug, Clone, Default)]
pub struct Articulation {
    pub segments: Vec<Segment>,
}

impl Articulation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a segment under `parent` (`None` for the root). Returns its index.
    pub fn add(&mut self, parent: Option<usize>, length: Fixed, material: JointMaterial) -> usize {
        let idx = self.segments.len();
        self.segments.push(Segment { parent, length, joint: Joint::new(material, Fixed::ZERO) });
        idx
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn children(&self, i: usize) -> Vec<usize> {
        self.segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.parent == Some(i))
            .map(|(j, _)| j)
            .collect()
    }

    pub fn is_leaf(&self, i: usize) -> bool {
        !self.segments.iter().any(|s| s.parent == Some(i))
    }

    /// Chain distance from the root (root = 0).
    pub fn depth(&self, mut i: usize) -> usize {
        let mut d = 0;
        while let Some(p) = self.segments[i].parent {
            d += 1;
            i = p;
        }
        d
    }

    /// Total absolute joint angle — a trig-free proxy for how curled (vs extended)
    /// the body is. 0 = laid out straight.
    pub fn total_curl(&self) -> Fixed {
        let mut sum = Fixed::ZERO;
        for s in &self.segments {
            sum = sum + s.joint.angle.abs();
        }
        sum
    }

    /// Drive every joint one step with the same torque, tallying the result.
    pub fn drive_all(&mut self, drive: Fixed, dt: Fixed) -> DriveReport {
        let mut r = DriveReport::default();
        for s in self.segments.iter_mut() {
            let out = s.joint.step(drive, dt);
            match out.phase {
                Phase::Stuck => r.stuck += 1,
                Phase::Slipping | Phase::Caught => r.moving += 1,
            }
            if out.broke_free {
                r.freed += 1;
            }
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt() -> Fixed {
        Fixed::from_frac(1, 10)
    }

    /// A spine of `n` segments with a couple of branches off it.
    fn dragon_body() -> Articulation {
        let mut a = Articulation::new();
        let m = JointMaterial::rusted();
        let len = Fixed::from_int(2);
        let root = a.add(None, len, m); // 0
        let s1 = a.add(Some(root), len, m); // 1
        let s2 = a.add(Some(s1), len, m); // 2 (tail tip)
        let _b1 = a.add(Some(s1), len, m); // 3 branch off mid-spine
        let _b2 = a.add(Some(root), len, m); // 4 branch off root
        let _ = s2;
        a
    }

    #[test]
    fn topology_reflects_the_branching_body() {
        let a = dragon_body();
        assert_eq!(a.len(), 5);
        assert_eq!(a.depth(0), 0, "root");
        assert_eq!(a.depth(2), 2, "tail tip is two joints out");
        // Segment 1 has two children: the continuing spine (2) and a branch (3).
        let mut kids = a.children(1);
        kids.sort();
        assert_eq!(kids, vec![2, 3]);
        assert!(a.is_leaf(2) && a.is_leaf(3) && a.is_leaf(4), "tips are leaves");
        assert!(!a.is_leaf(0) && !a.is_leaf(1));
    }

    #[test]
    fn a_rusted_body_stays_seized_under_a_weak_drive() {
        let mut a = dragon_body();
        let weak = Fixed::ONE; // below rusted base friction (2)
        for _ in 0..50 {
            let r = a.drive_all(weak, dt());
            assert_eq!(r.stuck, a.len(), "every joint locked");
            assert_eq!(r.moving, 0);
        }
        assert_eq!(a.total_curl(), Fixed::ZERO, "it never moved");
    }

    #[test]
    fn a_strong_drive_unblocks_the_body_and_it_starts_to_move() {
        let mut a = dragon_body();
        let strong = Fixed::from_int(20); // above the detent peaks
        let mut ever_freed = 0;
        for _ in 0..30 {
            ever_freed += a.drive_all(strong, dt()).freed;
        }
        assert!(ever_freed > 0, "joints broke free");
        assert!(a.total_curl() > Fixed::ZERO, "the body is moving / curling");
    }

    #[test]
    fn a_smooth_body_mobilises_immediately() {
        let mut a = Articulation::new();
        let m = JointMaterial::smooth();
        let len = Fixed::from_int(2);
        let r0 = a.add(None, len, m);
        a.add(Some(r0), len, m);
        let report = a.drive_all(Fixed::from_int(5), dt());
        assert!(report.moving >= 1, "smooth joints free at once");
    }

    #[test]
    fn stronger_drive_mobilises_more_than_a_weaker_one() {
        let run = |drive: Fixed| {
            let mut a = dragon_body();
            let mut moving_total = 0;
            for _ in 0..20 {
                moving_total += a.drive_all(drive, dt()).moving;
            }
            moving_total
        };
        // Drive just above the valley breakaway vs well above the peaks.
        assert!(run(Fixed::from_int(30)) > run(Fixed::from_int(3)), "more torque, more motion");
    }

    #[test]
    fn motion_is_deterministic() {
        let go = || {
            let mut a = dragon_body();
            for _ in 0..25 {
                a.drive_all(Fixed::from_int(9), dt());
            }
            a.total_curl()
        };
        assert_eq!(go(), go());
    }
}
