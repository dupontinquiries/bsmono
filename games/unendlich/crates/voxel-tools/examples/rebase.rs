// Two players. One raises the ground on main; the other built a tower
// on a branch from before the raise. Rebase the tower: the IR replays
// on the NEW ground, and the tower stands on the hill. A byte-level
// rebase would have buried it.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::vcs::{Policy, Repo};
use voxel_tools::weather::Change;
use voxel_tools::{VoxelTools, VoxelWorld};

#[derive(Clone)]
struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 2 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn height(w: &W, x: i32, z: i32) -> i32 { (0..200).rev().find(|&y| w.get(x, y, z) != 0).unwrap() }

/// Run IR against a world, return the changes it made. The anchor's Y
/// is RE-DERIVED from the ground: that is the whole point of a rebase.
fn run(w: &mut W, ir: &str) -> Vec<Change> {
    let before = w.clone();
    let ay = height(w, 0, 0);
    let (plan, _) = parse_plan_at(ir, Some((0, ay, 0)));
    { let mut t = VoxelTools::new(w); run_plan(&mut t, &plan, 200, 32, false); }
    w.0.iter().filter(|(k, v)| before.0.get(k) != Some(v) && before.get(k.0, k.1, k.2) != **v)
        .map(|(k, v)| Change { at: *k, to: *v }).collect()
}
fn apply(w: &mut W, changes: &[Change]) { for c in changes { w.set(c.at.0, c.at.1, c.at.2, c.to); } }

fn main() {
    let mut repo = Repo::new(32);
    let mut world = W(BTreeMap::new());

    // kess builds a tower on a branch
    repo.branch("tower").unwrap(); repo.switch("tower").unwrap();
    let tower_ir = "# tower\ncylinder @0,0 r=3 h=9 block=stone_bricks\nsphere @3,0 r=1 block=air\n";
    let ch = run(&mut world, tower_ir);
    repo.commit_changes_at("kess", "a tower", tower_ir, (0, 0), &ch);
    println!("tower on the branch: base at y={}, top at y={}", 65, height(&world, 0, 0));

    // meanwhile on main, the ground is raised
    repo.switch("main").unwrap();
    let mut main_world = W(BTreeMap::new());
    let raise_ir = "brush raise @0,0 by=12 r=30 hill\n";
    let ch = run(&mut main_world, raise_ir);
    repo.commit_changes("friend", "raise a hill", raise_ir, &ch);
    println!("main: ground raised to y={} at the tower's spot", height(&main_world, 0, 0));

    // a plain MERGE would put the tower's old cells inside the hill
    let mut merged = main_world.clone();
    repo.switch("main").unwrap();
    let head_before = repo.head_commit().tree;
    let (_, rep) = repo.merge("tower", Policy::Theirs, "kess").unwrap();
    let d = repo.diff(head_before, repo.head_commit().tree);
    apply(&mut merged, &d.iter().map(|(c, _)| *c).collect::<Vec<_>>());
    println!("\nmerge: {}", rep.summary());
    println!("  tower top after merge: y={} -- ground is {} -> the tower is BURIED, {} blocks above ground",
             73, height(&merged, 0, 0).min(76), 73 - height(&main_world, 10, 0).min(73));

    // REBASE: replay the tower's IR on the raised ground
    let mut repo2 = Repo::new(32);
    repo2.branch("tower").unwrap();
    let ch = run(&mut W(BTreeMap::new()), raise_ir);
    repo2.commit_changes("friend", "raise a hill", raise_ir, &ch);
    repo2.switch("tower").unwrap();
    let ch = run(&mut W(BTreeMap::new()), tower_ir);
    repo2.commit_changes("kess", "a tower", tower_ir, &ch);
    let plan = repo2.rebase_plan("tower", "main").unwrap();
    println!("\nrebase plan: {} commit(s) to replay", plan.len());
    let mut rebased = main_world.clone();
    repo2.switch("main").unwrap();
    for c in &plan {
        println!("  replaying {:?}: {}", c.message, c.ir.lines().nth(1).unwrap_or(""));
        let ch = run(&mut rebased, &c.ir);
        repo2.commit_changes(&c.author, &c.message, &c.ir, &ch);
    }
    let h = repo2.head_commit().hash;
    repo2.reset_branch("tower", h).unwrap();
    println!("  tower top after rebase: y={} on ground at y={} -> stands {} blocks tall on the hill",
             height(&rebased, 0, 0), height(&main_world, 0, 0), height(&rebased, 0, 0) - height(&main_world, 0, 0));
    println!("\n{} objects in the store; log:", repo2.object_count());
    for c in repo2.log("main", 5) { println!("  {} {} -- {}", voxel_tools::vcs::short(c.hash), c.author, c.message); }
}
