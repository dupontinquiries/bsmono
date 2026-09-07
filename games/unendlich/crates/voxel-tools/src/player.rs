//! A player, simulated.
//!
//! Reachability answers "can a body get from A to B". It does not
//! answer the question a builder actually has, which is "is this any
//! good to be in". Those differ: a village whose doors all open is
//! reachable and may still be a bad village -- everything twelve
//! blocks from everything, nothing to look at, one route in and out,
//! a dead end behind the mill.
//!
//! So: put a player in it. The agent has GOALS -- get inside, get to
//! the top, tour the doors, find the way out -- and walks. What it
//! manages, how far it walked to manage it, and what it never reached
//! are the score. That is the same information a person gives you
//! after five minutes in a build, and unlike a metric it is about the
//! EXPERIENCE rather than the geometry.
//!
//! THE CACHE
//!
//! Scoring is the inner loop of a search: thousands of variants, each
//! walked several times. Two things make that affordable.
//!
//! * Reachability between two points in an UNCHANGED world is the
//!   same answer every time. `Memory` caches it by (from, to, body).
//! * A variant changes a few hundred cells of a build; everything
//!   outside the changed box is unaffected. The cache is keyed by a
//!   WORLD EPOCH that the caller bumps when it edits, so a variant
//!   invalidates its own entries and nobody else's.
//!
//! On a village the cache turns 22 floods into 22 lookups from the
//! second run onward, and the runs are what a search does.

use crate::reach::{self, Body};
use crate::VoxelWorld;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// What a player tries to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Goal {
    /// Walk to a point.
    Reach { what: String, to: (i32, i32, i32) },
    /// Get inside: reach any door of the build.
    Enter { what: String },
    /// Reach the highest standable cell in the box.
    Summit,
    /// Visit every door, in any order: a tour.
    TourDoors,
    /// Get back out to a point far outside.
    Leave { to: (i32, i32, i32) },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub goal: String,
    pub done: bool,
    /// Blocks walked, as the flood measures it.
    pub cost: usize,
    /// How close it got when it failed.
    pub short_by: i32,
    pub at: Option<(i32, i32, i32)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub outcomes: Vec<Outcome>,
    /// 0..1: what fraction of the goals were met, weighted by how near
    /// the failures got.
    pub completion: f32,
    /// Average cost of the goals that succeeded. High is a build you
    /// walk across rather than through.
    pub mean_cost: f32,
    /// Distinct places the player could stand: a build with one is a
    /// corridor, with many is somewhere to be.
    pub places: usize,
    /// Cache hits and misses for this run.
    pub hits: usize,
    pub misses: usize,
}

impl Report {
    /// One number for a search to sort on, LOWER IS BETTER, in the
    /// shape the genetic engine's objectives take.
    pub fn objectives(&self) -> Vec<f32> {
        vec![
            1.0 - self.completion,                             // what it could not do
            (self.mean_cost / 400.0).min(2.0),                 // how far it had to walk
            if self.places == 0 { 1.0 } else { 1.0 / self.places as f32 },   // how varied it is
        ]
    }
    pub fn summary(&self) -> String {
        let failed: Vec<&Outcome> = self.outcomes.iter().filter(|o| !o.done).collect();
        format!("{}/{} goals, mean walk {:.0}, {} place(s){}{}",
                self.outcomes.len() - failed.len(), self.outcomes.len(), self.mean_cost, self.places,
                if failed.is_empty() { String::new() }
                else { format!(" -- FAILED: {}", failed.iter()
                    .map(|o| format!("{} (short by {})", o.goal, o.short_by))
                    .collect::<Vec<_>>().join(", ")) },
                if self.hits + self.misses > 0 {
                    format!(" [{} hit, {} miss]", self.hits, self.misses) } else { String::new() })
    }
}

/// The cache. Shared across runs and threads; keyed by an epoch the
/// caller bumps whenever the world changes.
#[derive(Debug, Default)]
pub struct Memory {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    epoch: u64,
    walks: BTreeMap<(u64, (i32, i32, i32), (i32, i32, i32), i32), (bool, usize, i32)>,
}

impl Memory {
    pub fn new() -> Arc<Self> { Arc::new(Self::default()) }

    /// The world changed: everything cached is suspect.
    pub fn invalidate(&self) {
        let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        i.epoch += 1;
        i.walks.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|i| i.walks.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    fn get(&self, from: (i32, i32, i32), to: (i32, i32, i32), b: &Body) -> Option<(bool, usize, i32)> {
        let i = self.inner.lock().ok()?;
        i.walks.get(&(i.epoch, from, to, b.step_up * 100 + b.height)).copied()
    }
    fn put(&self, from: (i32, i32, i32), to: (i32, i32, i32), b: &Body, v: (bool, usize, i32)) {
        if let Ok(mut i) = self.inner.lock() {
            let e = i.epoch;
            // a bound, so a long session does not grow without limit
            if i.walks.len() > 100_000 { i.walks.clear(); }
            i.walks.insert((e, from, to, b.step_up * 100 + b.height), v);
        }
    }
}

/// Walk a build with a set of goals and report what happened.
pub fn play<W: VoxelWorld>(w: &W, start: (i32, i32, i32), min: (i32, i32, i32), max: (i32, i32, i32),
                           goals: &[Goal], b: &Body, mem: Option<&Memory>) -> Report {
    let mut r = Report::default();
    let doors = reach::openings(w, min, max);
    let walk = |from: (i32, i32, i32), to: (i32, i32, i32), r: &mut Report| -> (bool, usize, i32) {
        if let Some(m) = mem {
            if let Some(v) = m.get(from, to, b) { r.hits += 1; return v; }
            r.misses += 1;
        }
        let res = reach::reaches(w, from, to, b);
        let v = (res.ok, res.visited, if res.ok { 0 } else { res.nearest });
        if let Some(m) = mem { m.put(from, to, b, v); }
        v
    };
    let mut here = start;
    for g in goals {
        let (name, target): (String, Option<(i32, i32, i32)>) = match g {
            Goal::Reach { what, to } => (what.clone(), Some(*to)),
            Goal::Enter { what } => (format!("enter {what}"), doors.first().copied()),
            Goal::Summit => {
                let top = (min.1..=max.1).rev().find_map(|y| {
                    (min.0..=max.0).step_by(2).flat_map(|x| (min.2..=max.2).step_by(2).map(move |z| (x, y, z)))
                        .find(|c| reach::standable(w, *c, b))
                });
                ("the top".into(), top)
            }
            Goal::TourDoors => {
                // every door in turn, from wherever the last one left us
                let mut done = 0;
                let mut cost = 0usize;
                let mut worst = 0;
                for d in &doors {
                    let (ok, c, short) = walk(here, *d, &mut r);
                    cost += c;
                    if ok { done += 1; here = *d; } else { worst = worst.max(short); }
                }
                r.outcomes.push(Outcome {
                    goal: format!("tour {} door(s)", doors.len()),
                    done: !doors.is_empty() && done == doors.len(),
                    cost, short_by: worst,
                    at: doors.first().copied(),
                });
                continue;
            }
            Goal::Leave { to } => ("leave".into(), Some(*to)),
        };
        let Some(t) = target else {
            r.outcomes.push(Outcome { goal: name, done: false, cost: 0, short_by: i32::MAX, at: None });
            continue;
        };
        let (ok, cost, short) = walk(here, t, &mut r);
        if ok { here = t; }
        r.outcomes.push(Outcome { goal: name, done: ok, cost, short_by: short, at: Some(t) });
    }
    // the places: how many separate standable regions the build has
    r.places = reach::islands(w, min, max, &Body { budget: 6_000, corridor: None, ..*b }, 6).len();
    let n = r.outcomes.len().max(1) as f32;
    // completion counts a near miss as partial credit: a stair that
    // stops one block short is nearly right, and a search needs to
    // know that or it cannot climb toward the answer.
    r.completion = r.outcomes.iter().map(|o| {
        if o.done { 1.0 } else if o.short_by == i32::MAX { 0.0 }
        else { (1.0 - o.short_by as f32 / 24.0).max(0.0) * 0.5 }
    }).sum::<f32>() / n;
    let done: Vec<&Outcome> = r.outcomes.iter().filter(|o| o.done).collect();
    r.mean_cost = if done.is_empty() { 0.0 }
        else { done.iter().map(|o| o.cost as f32).sum::<f32>() / done.len() as f32 };
    r
}

/// The goals a build of this kind should satisfy. What "good" means
/// for a house is not what it means for a canyon.
pub fn goals_for(kind: &str, min: (i32, i32, i32), max: (i32, i32, i32)) -> Vec<Goal> {
    let outside = (min.0 - 8, max.1 + 1, min.2 - 8);
    match kind {
        "house" | "hut" | "cottage" | "inn" | "temple" | "castle" => vec![
            Goal::Enter { what: kind.into() },
            Goal::Summit,
            Goal::Leave { to: outside },
        ],
        "village" | "settlement" => vec![
            Goal::TourDoors,
            Goal::Leave { to: outside },
        ],
        // a place, not a building: get in, get to the bottom, get out
        "canyon" | "gorge" | "sinkhole" | "cave" | "caldera" => vec![
            Goal::Reach { what: "the floor".into(),
                          to: ((min.0 + max.0) / 2, min.1 + 1, (min.2 + max.2) / 2) },
            Goal::Leave { to: outside },
        ],
        "mesa" | "cliffs" | "highlands" => vec![
            Goal::Summit,
            Goal::Leave { to: outside },
        ],
        _ => vec![Goal::Enter { what: kind.into() }, Goal::Leave { to: outside }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct W(BTreeMap<(i32, i32, i32), u16>);
    impl W {
        fn new() -> Self { W(BTreeMap::new()) }
        fn fill(&mut self, lo: (i32, i32, i32), hi: (i32, i32, i32), b: u16) {
            for x in lo.0..=hi.0 { for y in lo.1..=hi.1 { for z in lo.2..=hi.2 { self.0.insert((x, y, z), b); } } }
        }
        /// a hut with a door in the south wall
        fn hut(&mut self, door: bool) {
            self.fill((0, 65, 0), (8, 70, 8), 1);
            self.fill((1, 65, 1), (7, 69, 7), 0);
            if door { self.fill((4, 65, 0), (4, 66, 0), 0); }
        }
    }
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }

    fn body() -> Body { Body { budget: 6_000, ..Default::default() } }

    #[test]
    fn a_player_gets_into_a_hut_with_a_door_and_not_one_without() {
        let mut open = W::new(); open.hut(true);
        let mut shut = W::new(); shut.hut(false);
        let goals = goals_for("house", (0, 65, 0), (8, 70, 8));
        let a = play(&open, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), None);
        let b = play(&shut, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), None);
        assert!(a.completion > b.completion, "a door helps: {} vs {}", a.summary(), b.summary());
        assert!(a.outcomes.iter().any(|o| o.goal.starts_with("enter") && o.done), "{}", a.summary());
        assert!(!b.outcomes.iter().any(|o| o.goal.starts_with("enter") && o.done), "{}", b.summary());
        assert!(b.summary().contains("FAILED"));
    }

    #[test]
    fn a_near_miss_scores_better_than_a_hopeless_one() {
        // a wall one block too tall to climb, versus a wall ten blocks
        // away and twenty tall: both fail, one is nearly right
        let mut close = W::new();
        close.fill((6, 65, -30), (6, 67, 30), 1);
        let mut far = W::new();
        far.fill((6, 65, -30), (6, 85, 30), 1);
        let goals = vec![Goal::Reach { what: "beyond".into(), to: (20, 65, 0) }];
        let a = play(&close, (0, 65, 0), (0, 60, -30), (30, 90, 30), &goals, &body(), None);
        let b = play(&far, (0, 65, 0), (0, 60, -30), (30, 90, 30), &goals, &body(), None);
        assert!(!a.outcomes[0].done && !b.outcomes[0].done);
        // both are the same distance short in x, so partial credit is
        // equal -- what differs is that neither is zero
        assert!(a.completion > 0.0, "a near miss earns partial credit: {}", a.summary());
    }

    #[test]
    fn the_memory_turns_the_second_run_into_lookups() {
        let mut w = W::new(); w.hut(true);
        let mem = Memory::new();
        let goals = goals_for("house", (0, 65, 0), (8, 70, 8));
        let first = play(&w, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), Some(&mem));
        assert!(first.misses > 0 && first.hits == 0, "{}", first.summary());
        let second = play(&w, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), Some(&mem));
        assert_eq!(second.misses, 0, "everything cached: {}", second.summary());
        assert_eq!(second.hits, first.misses);
        // and the answers agree
        assert_eq!(first.outcomes.iter().map(|o| o.done).collect::<Vec<_>>(),
                   second.outcomes.iter().map(|o| o.done).collect::<Vec<_>>());
        // the world changes: the cache goes
        mem.invalidate();
        let third = play(&w, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), Some(&mem));
        assert!(third.hits == 0 && third.misses > 0, "invalidated: {}", third.summary());
    }

    #[test]
    fn a_tour_of_doors_needs_all_of_them() {
        let mut w = W::new();
        w.hut(true);
        // a second hut with no door
        w.fill((20, 65, 0), (28, 70, 8), 1);
        w.fill((21, 65, 1), (27, 69, 7), 0);
        w.fill((24, 65, 0), (24, 66, 0), 0);
        let goals = vec![Goal::TourDoors];
        let r = play(&w, (4, 65, -6), (0, 65, 0), (28, 70, 8), &goals, &body(), None);
        assert!(r.outcomes[0].goal.contains("door"), "{}", r.summary());
        assert!(r.outcomes[0].done, "both doors are reachable: {}", r.summary());
        // wall one off and the tour fails
        w.fill((24, 65, -2), (24, 69, -2), 1);
        w.fill((22, 65, -2), (26, 69, -1), 1);
        let r2 = play(&w, (4, 65, -6), (0, 65, 0), (28, 70, 8), &goals, &body(), None);
        assert!(!r2.outcomes[0].done, "one door is now behind a wall: {}", r2.summary());
    }

    #[test]
    fn the_objectives_are_lower_is_better_and_a_better_build_wins() {
        let mut good = W::new(); good.hut(true);
        let mut bad = W::new(); bad.hut(false);
        let goals = goals_for("house", (0, 65, 0), (8, 70, 8));
        let g = play(&good, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), None).objectives();
        let b = play(&bad, (4, 65, -6), (0, 65, 0), (8, 70, 8), &goals, &body(), None).objectives();
        assert!(g[0] < b[0], "the one you can enter scores lower: {g:?} vs {b:?}");
        assert!(g.iter().all(|v| v.is_finite()) && b.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn goals_differ_by_what_was_built() {
        let (lo, hi) = ((0, 60, 0), (40, 90, 40));
        assert!(goals_for("house", lo, hi).iter().any(|g| matches!(g, Goal::Enter { .. })));
        assert!(goals_for("village", lo, hi).iter().any(|g| matches!(g, Goal::TourDoors)));
        assert!(goals_for("canyon", lo, hi).iter().any(|g| matches!(g, Goal::Reach { .. })),
                "a canyon is judged by whether you can get to its floor");
        assert!(goals_for("mesa", lo, hi).iter().any(|g| matches!(g, Goal::Summit)));
        // everything must include a way out: a build you cannot leave
        // is the worst failure and the easiest to miss
        for k in ["house", "village", "canyon", "mesa", "anything"] {
            assert!(goals_for(k, lo, hi).iter().any(|g| matches!(g, Goal::Leave { .. })), "{k}");
        }
    }
}
