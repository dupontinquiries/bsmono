//! Animation: how a finished plan is revealed.
//!
//! The naive reveal -- blocks appear in the order the ops ran -- reads
//! as "materialising", not "building". Real construction has an order
//! people recognise: ground first, then foundations, walls rising,
//! roof last, decoration after the structure exists. Reproducing that
//! order costs nothing (the blocks are already placed; this is only
//! which frame each becomes visible in) and is the difference between
//! a build that feels authored and one that feels pasted.
//!
//! The schedule orders blocks by, in priority:
//!
//!   1. PHASE   terraform < structure < decoration
//!              (ground before things on it, things before trimmings)
//!   2. HEIGHT  low to high within a phase -- walls rise
//!   3. ORIGIN  near the plan's anchor first -- a build grows outward
//!
//! and then cuts the ordered list into frames of `blocks_per_frame`.
//! Frames are further grouped by chunk so the mesher can rebuild each
//! chunk once per frame, which keeps the animation from stalling on
//! large builds.

use crate::plan::{Batch, Op};

/// How a build reveals. Selectable with `/anim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Ground, then walls rising, then decoration, growing outward
    /// from the anchor. The default; reads as construction.
    #[default]
    GroundUp,
    /// Blocks drop in from above and settle. Bottom-up within each
    /// column, but columns start at staggered times so the build
    /// "rains in" rather than rising as one flat sheet. Each frame
    /// carries a `drop_from` height the animator can lerp from.
    Physics,
    /// Seeded shuffle. Reads as "materialising"; useful for magical
    /// or ruined styles, and as a control to compare against.
    Random,
    /// One frame, everything. For when you just want it there.
    Instant,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "ground-up" | "groundup" | "ground" | "build" => Profile::GroundUp,
            "physics" | "fall" | "drop" => Profile::Physics,
            "random" | "shuffle" | "scatter" => Profile::Random,
            "instant" | "now" | "none" => Profile::Instant,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self { Profile::GroundUp => "ground-up", Profile::Physics => "physics",
                     Profile::Random => "random", Profile::Instant => "instant" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase { Ground = 0, Structure = 1, Decoration = 2 }

impl Phase {
    pub fn of(op: &Op) -> Self {
        match op {
            Op::Terraform { .. } => Phase::Ground,
            Op::Scatter { .. } | Op::Tree { .. } => Phase::Decoration,
            _ => Phase::Structure,
        }
    }
}

/// One frame of the reveal: blocks to make visible, grouped by chunk.
#[derive(Debug, Clone)]
pub struct Frame {
    pub index: u32,
    pub phase: Phase,
    /// (chunk, blocks) -- each chunk appears once per frame.
    pub chunks: Vec<((i32, i32, i32), Vec<(i32, i32, i32)>)>,
    /// For the physics profile: blocks in this frame should be drawn
    /// falling from this many blocks above their final position.
    pub drop_from: Option<i32>,
}

impl Frame {
    pub fn block_count(&self) -> usize {
        self.chunks.iter().map(|(_, b)| b.len()).sum()
    }
}

/// Build the reveal schedule. `ops` are the plan's ops in order (the
/// batch tick indexes into them); `anchor` is the focus anchor the
/// plan was built around; `blocks_per_frame` sets the pace.
pub fn schedule(ops: &[Op], batches: &[Batch], anchor: (i32, i32, i32),
                blocks_per_frame: usize, chunk_size: i32) -> Vec<Frame> {
    schedule_with(Profile::GroundUp, ops, batches, anchor, blocks_per_frame, chunk_size, 0)
}

fn column_jitter(x: i32, z: i32, seed: u64) -> i64 {
    // small deterministic per-column offset so physics columns do not
    // all start together; 0..7 frames' worth of delay
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ (z as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
    h ^= h >> 29; h = h.wrapping_mul(0xBF58476D1CE4E5B9); h ^= h >> 32;
    (h % 8) as i64
}

pub fn schedule_with(profile: Profile, ops: &[Op], batches: &[Batch], anchor: (i32, i32, i32),
                     blocks_per_frame: usize, chunk_size: i32, seed: u64) -> Vec<Frame> {
    let bpf = blocks_per_frame.max(1);

    // Flatten to (phase, y, dist2, xyz)
    let mut items: Vec<(Phase, i32, i64, (i32, i32, i32))> = Vec::new();
    for b in batches {
        let phase = ops.get(b.tick as usize).map(Phase::of).unwrap_or(Phase::Structure);
        for &(x, y, z, _) in &b.blocks {
            let (dx, dz) = ((x - anchor.0) as i64, (z - anchor.2) as i64);
            items.push((phase, y, dx * dx + dz * dz, (x, y, z)));
        }
    }
    let mut drop_from: Option<i32> = None;
    let bpf = match profile {
        Profile::GroundUp => {
            // The ordering that reads as construction. Decoration is
            // sorted by distance only, so trees and scatter fill outward.
            items.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| if a.0 == Phase::Decoration { std::cmp::Ordering::Equal } else { a.1.cmp(&b.1) })
                    .then_with(|| a.2.cmp(&b.2))
            });
            bpf
        }
        Profile::Physics => {
            // bottom-up within a column (a block cannot land before the
            // one under it), columns staggered by a per-column jitter,
            // phases still respected so ground settles before walls
            items.sort_by(|a, b| {
                let ja = column_jitter(a.3 .0, a.3 .2, seed);
                let jb = column_jitter(b.3 .0, b.3 .2, seed);
                a.0.cmp(&b.0)
                    .then_with(|| (a.1 as i64 + ja).cmp(&(b.1 as i64 + jb)))
                    .then_with(|| a.2.cmp(&b.2))
            });
            drop_from = Some(12);
            bpf
        }
        Profile::Random => {
            // seeded Fisher-Yates so "random" is reproducible for a seed
            let mut s = seed | 1;
            for i in (1..items.len()).rev() {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let j = ((s >> 33) as usize) % (i + 1);
                items.swap(i, j);
            }
            bpf
        }
        Profile::Instant => items.len().max(1),
    };

    let mut frames = Vec::new();
    for (i, chunk_of_items) in items.chunks(bpf).enumerate() {
        let phase = chunk_of_items[0].0;
        let mut by_chunk: std::collections::BTreeMap<(i32, i32, i32), Vec<(i32, i32, i32)>> =
            Default::default();
        for &(_, _, _, (x, y, z)) in chunk_of_items {
            let c = (x.div_euclid(chunk_size), y.div_euclid(chunk_size), z.div_euclid(chunk_size));
            by_chunk.entry(c).or_default().push((x, y, z));
        }
        frames.push(Frame { index: i as u32, phase, chunks: by_chunk.into_iter().collect(), drop_from });
    }
    frames
}

/// Suggested pacing: frames per second such that a build of this
/// size completes in roughly `target_seconds`, clamped to a range
/// that still reads as animation. Small builds should not crawl;
/// large ones should not flash.
pub fn pacing(total_blocks: usize, blocks_per_frame: usize, target_seconds: f32) -> f32 {
    let frames = (total_blocks as f32 / blocks_per_frame.max(1) as f32).ceil().max(1.0);
    (frames / target_seconds.max(0.5)).clamp(4.0, 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan, run_plan};
    use crate::{VoxelTools, VoxelWorld};
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }

    fn build(src: &str) -> (Vec<Op>, Vec<Batch>) {
        let mut w = Flat::new();
        let (plan, _) = parse_plan(src);
        let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        (plan.ops, run.batches)
    }

    #[test]
    fn ground_then_structure_then_decoration() {
        let (ops, batches) = build(
            "tree 10,0 oak h=6\nterraform 0,0 6,6 delta=2 block=dirt\ncylinder 0,0 r=2 h=5 block=stone\n");
        let frames = schedule(&ops, &batches, (0, 65, 0), 25, 32);
        let phases: Vec<Phase> = frames.iter().map(|f| f.phase).collect();
        // phases must be non-decreasing regardless of op order in the plan
        assert!(phases.windows(2).all(|w| w[0] <= w[1]), "{phases:?}");
        assert_eq!(phases.first(), Some(&Phase::Ground));
        assert_eq!(phases.last(), Some(&Phase::Decoration));
    }

    #[test]
    fn walls_rise_from_the_bottom() {
        let (ops, batches) = build("cylinder 0,0 r=3 h=12 block=stone\n");
        let frames = schedule(&ops, &batches, (0, 65, 0), 20, 32);
        let ys: Vec<i32> = frames.iter()
            .map(|f| f.chunks.iter().flat_map(|(_, b)| b.iter().map(|p| p.1)).min().unwrap())
            .collect();
        assert!(ys.windows(2).all(|w| w[0] <= w[1]), "min y per frame must not decrease: {ys:?}");
        assert!(ys.last().unwrap() > ys.first().unwrap());
    }

    #[test]
    fn each_chunk_appears_once_per_frame() {
        let (ops, batches) = build("cylinder 0,0 r=4 h=40 block=stone\n");
        let frames = schedule(&ops, &batches, (0, 65, 0), 50, 32);
        for f in &frames {
            let mut seen = std::collections::BTreeSet::new();
            for (c, _) in &f.chunks { assert!(seen.insert(*c), "chunk {c:?} twice in frame {}", f.index); }
        }
        let total: usize = frames.iter().map(Frame::block_count).sum();
        let placed: usize = batches.iter().map(|b| b.blocks.len()).sum();
        assert_eq!(total, placed, "every placed block must be revealed exactly once");
    }

    #[test]
    fn pacing_keeps_small_builds_brisk_and_large_ones_bounded() {
        assert!(pacing(30, 10, 3.0) >= 4.0);          // tiny build: floor
        assert_eq!(pacing(60_000, 50, 3.0), 60.0);     // huge build: ceiling
        let mid = pacing(600, 20, 3.0);                // 30 frames over 3 s
        assert!((mid - 10.0).abs() < 0.01, "{mid}");
    }

    #[test]
    fn profiles_parse_with_aliases() {
        assert_eq!(Profile::parse("ground-up"), Some(Profile::GroundUp));
        assert_eq!(Profile::parse("PHYSICS"), Some(Profile::Physics));
        assert_eq!(Profile::parse("fall"), Some(Profile::Physics));
        assert_eq!(Profile::parse("instant"), Some(Profile::Instant));
        assert_eq!(Profile::parse("sideways"), None);
    }

    #[test]
    fn instant_is_one_frame_with_everything() {
        let (ops, batches) = build("cylinder 0,0 r=4 h=20 block=stone\n");
        let f = schedule_with(Profile::Instant, &ops, &batches, (0, 65, 0), 10, 32, 0);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].block_count(), batches.iter().map(|b| b.blocks.len()).sum::<usize>());
    }

    #[test]
    fn physics_never_lands_a_block_before_the_one_under_it() {
        let (ops, batches) = build("cylinder 0,0 r=3 h=15 block=stone\n");
        let frames = schedule_with(Profile::Physics, &ops, &batches, (0, 65, 0), 7, 32, 42);
        assert!(frames.iter().all(|f| f.drop_from == Some(12)));
        // within any column, y must be non-decreasing across frames
        let mut seen: std::collections::HashMap<(i32, i32), i32> = Default::default();
        for f in &frames {
            for (_, blocks) in &f.chunks {
                for &(x, y, z) in blocks {
                    if let Some(prev) = seen.get(&(x, z)) {
                        assert!(y > *prev, "column ({x},{z}) landed y={y} after y={prev}");
                    }
                    seen.insert((x, z), y);
                }
            }
        }
    }

    #[test]
    fn physics_columns_are_staggered_not_lockstep() {
        let (ops, batches) = build("fill 0,65,0 9,66,9 block=stone\n");
        let g = schedule_with(Profile::GroundUp, &ops, &batches, (0, 65, 0), 20, 32, 0);
        let p = schedule_with(Profile::Physics, &ops, &batches, (0, 65, 0), 20, 32, 42);
        // ground-up finishes y=65 entirely before y=66; physics mixes them
        let mixed = |frames: &[Frame]| frames.iter().any(|f| {
            let ys: std::collections::BTreeSet<i32> =
                f.chunks.iter().flat_map(|(_, b)| b.iter().map(|q| q.1)).collect();
            ys.len() > 1
        });
        assert!(!mixed(&g), "ground-up frames are single-layer for a flat slab");
        assert!(mixed(&p), "physics frames mix layers because columns are staggered");
    }

    #[test]
    fn random_is_reproducible_for_a_seed_and_differs_across_seeds() {
        let (ops, batches) = build("cylinder 0,0 r=3 h=8 block=stone\n");
        let first = |seed| schedule_with(Profile::Random, &ops, &batches, (0, 65, 0), 5, 32, seed)
            .first().unwrap().chunks.iter().flat_map(|(_, b)| b.clone()).collect::<Vec<_>>();
        assert_eq!(first(7), first(7));
        assert_ne!(first(7), first(8));
    }
}
