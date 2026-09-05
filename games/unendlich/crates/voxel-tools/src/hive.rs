//! The hive: the frame is the queen, and everything else waits on her.
//!
//! WHAT STUTTERS
//!
//! A commit of 200,000 blocks is one flush, one `set_blocks`, and a
//! remesh of every touched chunk inside a single frame. A layer bake
//! places four hundred trees in one call. Even the DRY RUN of a plan
//! happens on the frame thread, inside `poll_turn`. The model call is
//! off-thread; everything after it is not. Every one of those is a
//! visible hitch, and the bigger the command the longer it is.
//!
//! THE MODEL
//!
//! * The **queen** is the frame. She is sovereign. Each frame she has a
//!   budget of milliseconds for generation work, and the budget ADAPTS:
//!   slow frame, it shrinks; fast frame, it grows; a frame already over
//!   target gets nothing at all. She never runs past her budget, and a
//!   cell she cannot afford waits.
//! * A **cell** is the unit of work: the changes for ONE chunk. Small
//!   enough that no single cell can blow the budget, and aligned with
//!   the one cost that actually matters, which is a chunk remesh.
//! * The **comb** is the priority queue of cells. Nearest the player
//!   first, so what you can see fills first. Cells under the player's
//!   gaze get **royal jelly**: a priority boost. When the player moves,
//!   the comb re-sorts, and the **swarm** follows.
//! * **Drones** are pure-compute workers off the frame thread: brush
//!   planning, layer baking, the river FSM, the dry run. They read a
//!   snapshot and produce cells. They never touch the world.
//! * The **waggle dance** is progress: what is being built, how far
//!   along, and how long it will take at the current rate.
//!
//! NOTHING IN THIS FILE TOUCHES THE WORLD
//!
//! The hive schedules; the game applies. `Queen::tick` takes a closure
//! that applies one cell and returns how long it took. That closure is
//! the only place blocks are written, and it is the game's.

use crate::weather::Change;
use std::collections::BinaryHeap;
use std::time::Duration;

/// One chunk's worth of changes, tagged with where it belongs to a job.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub chunk: (i32, i32, i32),
    pub changes: Vec<Change>,
    /// Which job produced it, for the waggle dance and for cancel.
    pub job: u32,
    /// Lower runs first. Set by the comb from distance and gaze.
    pub priority: f32,
}

impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}
impl Ord for Cell {
    // BinaryHeap is a max-heap; invert so the LOWEST priority value is
    // at the top.
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.priority.partial_cmp(&self.priority).unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| o.chunk.cmp(&self.chunk))
    }
}

/// Shard a change list into cells, one per chunk.
pub fn shard(changes: Vec<Change>, chunk_size: i32, job: u32) -> Vec<Cell> {
    let cs = chunk_size.max(1);
    let mut by_chunk: std::collections::BTreeMap<(i32, i32, i32), Vec<Change>> = Default::default();
    for c in changes {
        let key = (c.at.0.div_euclid(cs), c.at.1.div_euclid(cs), c.at.2.div_euclid(cs));
        by_chunk.entry(key).or_default().push(c);
    }
    by_chunk.into_iter()
        .map(|(chunk, changes)| Cell { chunk, changes, job, priority: 0.0 })
        .collect()
}

/// Where the player is and where they are looking, for prioritising.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observer {
    pub pos: [f32; 3],
    /// Unit vector.
    pub look: [f32; 3],
}

/// The priority queue, re-sortable as the observer moves.
#[derive(Debug, Default)]
pub struct Comb {
    heap: BinaryHeap<Cell>,
    chunk_size: i32,
    /// Total cells accepted per job, for progress.
    accepted: std::collections::BTreeMap<u32, usize>,
    /// Cells finished per job.
    finished: std::collections::BTreeMap<u32, usize>,
    blocks_done: std::collections::BTreeMap<u32, usize>,
    names: std::collections::BTreeMap<u32, String>,
    cancelled: std::collections::BTreeSet<u32>,
}

impl Comb {
    pub fn new(chunk_size: i32) -> Self { Self { chunk_size, ..Default::default() } }

    pub fn len(&self) -> usize { self.heap.len() }
    pub fn is_empty(&self) -> bool { self.heap.is_empty() }

    /// Accept a job's cells, prioritised for the observer.
    pub fn offer(&mut self, name: &str, cells: Vec<Cell>, obs: &Observer) {
        for mut c in cells {
            c.priority = self.priority_of(&c, obs);
            *self.accepted.entry(c.job).or_default() += 1;
            self.names.entry(c.job).or_insert_with(|| name.to_string());
            self.heap.push(c);
        }
    }

    /// Distance from the observer to the chunk's centre, with ROYAL
    /// JELLY for chunks in the gaze cone: a chunk you are looking at
    /// runs before one behind you at the same distance. The look
    /// weighting is a multiplier on distance, not an override, so a
    /// chunk behind you and touching you still beats one in view 200
    /// blocks away.
    fn priority_of(&self, c: &Cell, obs: &Observer) -> f32 {
        let cs = self.chunk_size as f32;
        let centre = [c.chunk.0 as f32 * cs + cs / 2.0,
                      c.chunk.1 as f32 * cs + cs / 2.0,
                      c.chunk.2 as f32 * cs + cs / 2.0];
        let d = [centre[0] - obs.pos[0], centre[1] - obs.pos[1], centre[2] - obs.pos[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if dist < 1.0 { return 0.0; }
        let cos = (d[0] * obs.look[0] + d[1] * obs.look[1] + d[2] * obs.look[2]) / dist;
        // in front: 0.5x distance; behind: 1.5x. Smooth in between.
        let jelly = 1.0 - 0.5 * cos.clamp(-1.0, 1.0);
        dist * jelly
    }

    /// The SWARM: re-prioritise everything for a new observer. Called
    /// when the player has moved far enough to matter; cheap enough to
    /// call every second.
    pub fn swarm(&mut self, obs: &Observer) {
        let cells: Vec<Cell> = self.heap.drain().collect();
        for mut c in cells {
            c.priority = self.priority_of(&c, obs);
            self.heap.push(c);
        }
    }

    pub fn cancel(&mut self, job: u32) {
        self.cancelled.insert(job);
        let keep: Vec<Cell> = self.heap.drain().filter(|c| c.job != job).collect();
        for c in keep { self.heap.push(c); }
    }

    fn pop(&mut self) -> Option<Cell> {
        while let Some(c) = self.heap.pop() {
            if !self.cancelled.contains(&c.job) { return Some(c); }
        }
        None
    }

    fn done(&mut self, c: &Cell) {
        *self.finished.entry(c.job).or_default() += 1;
        *self.blocks_done.entry(c.job).or_default() += c.changes.len();
    }

    /// The WAGGLE DANCE: what is in progress and how far along.
    pub fn dance(&self, cells_per_second: f32) -> Vec<String> {
        let mut out = Vec::new();
        for (job, total) in &self.accepted {
            let done = self.finished.get(job).copied().unwrap_or(0);
            if done >= *total || self.cancelled.contains(job) { continue; }
            let left = total - done;
            let eta = if cells_per_second > 0.01 { left as f32 / cells_per_second } else { f32::NAN };
            out.push(format!("{}: {}% ({}/{} chunks, {} blocks placed{})",
                             self.names.get(job).map(|s| s.as_str()).unwrap_or("job"),
                             done * 100 / (*total).max(1), done, total,
                             self.blocks_done.get(job).copied().unwrap_or(0),
                             if eta.is_finite() { format!(", ~{eta:.0}s") } else { String::new() }));
        }
        out
    }

    pub fn is_finished(&self, job: u32) -> bool {
        self.accepted.get(&job).map(|t| self.finished.get(&job).copied().unwrap_or(0) >= *t)
            .unwrap_or(true)
    }
}

/// The frame's budget for hive work, adapting to headroom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// Milliseconds the hive may use this frame.
    pub ms: f32,
    pub min_ms: f32,
    pub max_ms: f32,
    /// Target frame time; headroom is measured against it.
    pub target_frame_ms: f32,
    /// Exponential average of one cell's cost, for planning how many
    /// fit and for the dance's ETA.
    pub cell_cost_ms: f32,
}

impl Default for Budget {
    fn default() -> Self {
        Self { ms: 2.0, min_ms: 0.25, max_ms: 6.0, target_frame_ms: 16.6, cell_cost_ms: 1.0 }
    }
}

impl Budget {
    /// Adapt from the last frame's total time. Over target: shrink
    /// hard, because a stutter is worse than a slow fill. Under target:
    /// grow gently toward the headroom.
    pub fn adapt(&mut self, last_frame_ms: f32) {
        let headroom = self.target_frame_ms - last_frame_ms;
        if headroom <= 0.0 {
            self.ms = (self.ms * 0.5).max(self.min_ms);
        } else {
            // never take more than half the headroom, and approach it
            let want = (headroom * 0.5).clamp(self.min_ms, self.max_ms);
            self.ms += (want - self.ms) * 0.25;
        }
        self.ms = self.ms.clamp(self.min_ms, self.max_ms);
    }

    /// A frame already past target gets nothing: HIBERNATE.
    pub fn hibernating(&self, last_frame_ms: f32) -> bool {
        last_frame_ms > self.target_frame_ms * 1.5
    }
}

/// The queen: runs cells within the budget and adapts.
#[derive(Debug, Default)]
pub struct Queen {
    pub budget: Budget,
    /// Cells applied in the last second, for the dance.
    rate_window: Vec<(f64, usize)>,
    pub cells_this_frame: usize,
    pub chunks_this_frame: usize,
}

/// What the queen did this frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    pub cells: usize,
    pub blocks: usize,
    pub spent_ms: f32,
    pub hibernated: bool,
    /// Cells left in the comb afterward.
    pub remaining: usize,
}

impl Queen {
    pub fn new(budget: Budget) -> Self { Self { budget, ..Default::default() } }

    /// One frame of hive work.
    ///
    /// `last_frame_ms` is how long the previous frame took in total.
    /// `now` is wall-clock seconds. `apply` places one cell and returns
    /// how long that took; it is the only place blocks are written.
    ///
    /// Stops when the budget is spent, when the comb is empty, or when
    /// the next cell is predicted not to fit. Always applies at least
    /// one cell when not hibernating, so a slow machine still makes
    /// progress rather than starving forever on a budget smaller than
    /// one cell.
    pub fn tick<F>(&mut self, comb: &mut Comb, last_frame_ms: f32, now: f64, mut apply: F) -> Tick
    where F: FnMut(&Cell) -> Duration {
        self.budget.adapt(last_frame_ms);
        self.cells_this_frame = 0;
        self.chunks_this_frame = 0;
        let mut t = Tick { remaining: comb.len(), ..Default::default() };
        if self.budget.hibernating(last_frame_ms) || comb.is_empty() {
            t.hibernated = self.budget.hibernating(last_frame_ms);
            return t;
        }
        let mut spent = 0.0f32;
        let mut seen_chunks: std::collections::BTreeSet<(i32, i32, i32)> = Default::default();
        loop {
            // Predict: would the next cell fit? After the first, stop if
            // the running average says no. The first always runs.
            if t.cells > 0 && spent + self.budget.cell_cost_ms > self.budget.ms { break; }
            let Some(cell) = comb.pop() else { break };
            let took = apply(&cell).as_secs_f32() * 1000.0;
            spent += took;
            // EMA of cell cost, so the prediction tracks this machine
            // and this world, not a constant.
            self.budget.cell_cost_ms += (took - self.budget.cell_cost_ms) * 0.2;
            t.cells += 1;
            t.blocks += cell.changes.len();
            seen_chunks.insert(cell.chunk);
            comb.done(&cell);
            if spent >= self.budget.ms { break; }
        }
        t.spent_ms = spent;
        t.remaining = comb.len();
        self.cells_this_frame = t.cells;
        self.chunks_this_frame = seen_chunks.len();
        self.rate_window.push((now, t.cells));
        self.rate_window.retain(|(ts, _)| now - ts <= 1.0);
        t
    }

    /// Cells per second over the last second, for the dance.
    pub fn rate(&self) -> f32 {
        self.rate_window.iter().map(|(_, n)| *n).sum::<usize>() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(n: usize, x0: i32) -> Vec<Change> {
        (0..n).map(|i| Change { at: (x0 + (i % 32) as i32, 64, (i / 32) as i32), to: 1 }).collect()
    }
    fn obs(x: f32, look_x: f32) -> Observer { Observer { pos: [x, 64.0, 0.0], look: [look_x, 0.0, 0.0] } }

    #[test]
    fn shard_makes_one_cell_per_chunk() {
        let cells = shard(changes(200, 0), 32, 1);
        assert_eq!(cells.len(), 1, "200 blocks in one chunk");
        let cells = shard(changes(200, 20), 32, 1);
        assert_eq!(cells.len(), 2, "straddling a chunk boundary: two cells");
        let total: usize = cells.iter().map(|c| c.changes.len()).sum();
        assert_eq!(total, 200, "nothing lost");
    }

    #[test]
    fn nearest_first_and_the_gaze_gets_royal_jelly() {
        let mut comb = Comb::new(32);
        let mut cells = Vec::new();
        for cx in [-6, -1, 0, 1, 6] {
            cells.push(Cell { chunk: (cx, 2, 0), changes: changes(10, cx * 32), job: 1, priority: 0.0 });
        }
        // standing at x=16 (chunk 0), looking +x
        comb.offer("t", cells, &obs(16.0, 1.0));
        let order: Vec<i32> = std::iter::from_fn(|| comb.pop()).map(|c| c.chunk.0).collect();
        assert_eq!(order[0], 0, "the chunk you stand in first");
        assert!(order[1] == 1, "then the one in front, before the one behind at equal distance: {order:?}");
        assert_eq!(order[2], -1);
        assert!(order.ends_with(&[6, -6]) || order.ends_with(&[-6, 6]));
        let pos6 = order.iter().position(|c| *c == 6).unwrap();
        let posm6 = order.iter().position(|c| *c == -6).unwrap();
        assert!(pos6 < posm6, "far ahead beats far behind");
    }

    #[test]
    fn the_swarm_follows_the_player() {
        let mut comb = Comb::new(32);
        let cells: Vec<Cell> = [0, 10].iter().map(|cx| Cell {
            chunk: (*cx, 2, 0), changes: changes(5, cx * 32), job: 1, priority: 0.0 }).collect();
        comb.offer("t", cells, &obs(16.0, 1.0));
        // player walks to chunk 10
        comb.swarm(&obs(10.0 * 32.0 + 16.0, 1.0));
        assert_eq!(comb.pop().unwrap().chunk.0, 10, "the comb re-sorted for where you are now");
    }

    #[test]
    fn the_queen_stops_at_her_budget() {
        let mut comb = Comb::new(32);
        let cells: Vec<Cell> = (0..50).map(|i| Cell {
            chunk: (i, 2, 0), changes: changes(10, i * 32), job: 1, priority: 0.0 }).collect();
        comb.offer("t", cells, &obs(0.0, 1.0));
        let mut q = Queen::new(Budget { ms: 2.0, ..Default::default() });
        // every cell costs 0.5ms
        let t = q.tick(&mut comb, 10.0, 0.0, |_| Duration::from_micros(500));
        assert!(t.cells >= 3 && t.cells <= 6, "about 2ms / 0.5ms = 4 cells: {}", t.cells);
        assert!(t.remaining > 40, "the rest wait");
        assert!(!t.hibernated);
    }

    #[test]
    fn a_slow_frame_shrinks_the_budget_and_a_dead_one_hibernates() {
        let mut q = Queen::new(Budget::default());
        let before = q.budget.ms;
        q.budget.adapt(20.0);   // over the 16.6 target
        assert!(q.budget.ms < before, "over target: shrink");
        let mut comb = Comb::new(32);
        comb.offer("t", vec![Cell { chunk: (0, 2, 0), changes: changes(5, 0), job: 1, priority: 0.0 }],
                   &obs(0.0, 1.0));
        let t = q.tick(&mut comb, 40.0, 0.0, |_| Duration::from_micros(100));
        assert!(t.hibernated, "a frame at 2.4x target gets nothing");
        assert_eq!(t.cells, 0);
        assert_eq!(comb.len(), 1, "and the cell is still there for a better frame");
    }

    #[test]
    fn a_fast_frame_grows_the_budget_toward_the_headroom() {
        let mut b = Budget { ms: 0.5, ..Default::default() };
        for _ in 0..20 { b.adapt(6.0); }   // lots of headroom
        assert!(b.ms > 2.0, "grew: {}", b.ms);
        assert!(b.ms <= b.max_ms);
    }

    #[test]
    fn always_at_least_one_cell_so_a_slow_machine_still_progresses() {
        // A budget smaller than one cell must not starve forever.
        let mut comb = Comb::new(32);
        comb.offer("t", shard(changes(3000, 0), 32, 1), &obs(0.0, 1.0));
        let mut q = Queen::new(Budget { ms: 0.25, min_ms: 0.25, ..Default::default() });
        let t = q.tick(&mut comb, 10.0, 0.0, |_| Duration::from_millis(3));   // each cell 3ms
        assert_eq!(t.cells, 1, "one cell, even though it cost 12x the budget");
    }

    #[test]
    fn the_cost_estimate_tracks_the_machine() {
        let mut comb = Comb::new(32);
        let cells: Vec<Cell> = (0..40).map(|i| Cell {
            chunk: (i, 2, 0), changes: changes(10, i * 32), job: 1, priority: 0.0 }).collect();
        comb.offer("t", cells, &obs(0.0, 1.0));
        let mut q = Queen::new(Budget { ms: 3.0, cell_cost_ms: 0.1, ..Default::default() });
        q.tick(&mut comb, 10.0, 0.0, |_| Duration::from_millis(2));
        assert!(q.budget.cell_cost_ms > 0.5, "learned that cells are expensive here: {}", q.budget.cell_cost_ms);
    }

    #[test]
    fn the_waggle_dance_reports_progress_and_eta() {
        let mut comb = Comb::new(32);
        // 3200 blocks in a 32-wide strip is only FOUR chunks, and a 10 ms
        // budget finishes all four in one tick -- so the first version of
        // this test asked for progress on a job that was already done.
        // Spread the work over many chunks and give the queen a budget
        // that leaves most of it waiting.
        let cells: Vec<Cell> = (0..40).map(|i| Cell {
            chunk: (i, 2, 0), changes: changes(80, i * 32), job: 7, priority: 0.0 }).collect();
        comb.offer("mountain", cells, &obs(0.0, 1.0));
        let mut q = Queen::new(Budget { ms: 1.0, cell_cost_ms: 0.2, ..Default::default() });
        q.tick(&mut comb, 5.0, 0.0, |_| Duration::from_micros(200));
        let d = comb.dance(q.rate());
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].starts_with("mountain:"), "{}", d[0]);
        assert!(d[0].contains("chunks") && d[0].contains("blocks placed"), "{}", d[0]);
        // finish it
        while !comb.is_empty() { q.tick(&mut comb, 5.0, 1.0, |_| Duration::from_micros(200)); }
        assert!(comb.dance(q.rate()).is_empty(), "a finished job leaves the dance");
        assert!(comb.is_finished(7));
    }

    #[test]
    fn cancel_drops_a_jobs_cells_and_leaves_the_rest() {
        let mut comb = Comb::new(32);
        comb.offer("a", shard(changes(1000, 0), 32, 1), &obs(0.0, 1.0));
        comb.offer("b", shard(changes(1000, 5000), 32, 2), &obs(0.0, 1.0));
        let before = comb.len();
        comb.cancel(1);
        assert!(comb.len() < before);
        let mut q = Queen::new(Budget { ms: 100.0, ..Default::default() });
        let t = q.tick(&mut comb, 5.0, 0.0, |c| { assert_eq!(c.job, 2); Duration::from_micros(10) });
        assert!(t.cells > 0);
    }
}
