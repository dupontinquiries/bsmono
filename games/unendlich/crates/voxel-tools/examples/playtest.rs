// Tune a build by playing it: a parallel search scored by what a
// simulated player could and could not do.
use std::collections::BTreeMap;
use voxel_tools::evolve::{Budget, Genome as Gen};

use voxel_tools::player::{self, Goal, Memory};
use voxel_tools::reach::Body;
use voxel_tools::VoxelWorld;

#[derive(Clone, Debug, PartialEq)]
struct Doors([i32; 4]);            // where a door is cut in each wall, or -1 for none
impl Gen for Doors {
    fn mutate(&self, seed: u64) -> Self {
        let mut d = self.clone();
        let i = (seed % 4) as usize;
        d.0[i] = ((seed >> 8) % 10) as i32 - 1;
        d
    }
    fn crossover(&self, o: &Self, seed: u64) -> Self {
        let mut d = self.clone();
        for i in 0..4 { if (seed >> i) & 1 == 1 { d.0[i] = o.0[i]; } }
        d
    }
}

struct W(BTreeMap<(i32,i32,i32),u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        // a compound wall around the hut: the only way in is a door
        if y <= 64 { return 1; }
        if y <= 70 && (x == -6 || x == 14 || z == -6 || z == 14) { return 1; }
        0
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z),b); true }
}

fn build(d: &Doors) -> W {
    let mut w = W(BTreeMap::new());
    for x in 0..=8 { for y in 65..=70 { for z in 0..=8 {
        let shell = x==0||x==8||z==0||z==8||y==70;
        if shell { w.0.insert((x,y,z), 1); } else { w.0.insert((x,y,z), 0); }
    }}}
    // a gate in the compound wall, and doors in the hut per the genome
    for y in 65..=66 { w.0.insert((4, y, -6), 0); }
    for (i, at) in d.0.iter().enumerate() {
        if *at < 0 { continue; }
        let a = at.clamp(&1, &7);
        for y in 65..=66 {
            match i { 0 => { w.0.insert((*a, y, 0), 0); } 1 => { w.0.insert((*a, y, 8), 0); }
                      2 => { w.0.insert((0, y, *a), 0); } _ => { w.0.insert((8, y, *a), 0); } }
        }
    }
    w
}

fn score(d: &Doors, mem: &Memory) -> Vec<f32> {
    let w = build(d);
    let goals = player::goals_for("house", (0,65,0), (8,70,8));
    let b = Body { budget: 4000, ..Default::default() };
    let r = player::play(&w, (4, 65, -10), (0,65,0), (8,70,8), &goals, &b, Some(mem));
    let mut o = r.objectives();
    // a door in every wall is a shed, not a house: prefer few
    o.push(d.0.iter().filter(|a| **a >= 0).count() as f32 * 0.15);
    o
}

fn main() {
    // One and four: on a multicore box the second is faster, and on
    // any box the two agree exactly -- which is the property that
    // matters. A seeded search that is not reproducible is a slot
    // machine.
    for threads in [1, 4] {
        let mem = Memory::new();
        let t0 = std::time::Instant::now();
        let out = voxel_tools::evolve::evolve_par(
            7, Budget::evals(320, 16), &[],
            |s| Doors([(s % 10) as i32 - 1, ((s >> 8) % 10) as i32 - 1,
                       ((s >> 16) % 10) as i32 - 1, ((s >> 24) % 10) as i32 - 1]),
            |d| score(d, &mem), &[1.0, 0.5, 0.5, 1.0], threads);
        let best = voxel_tools::evolve::pick(&out.front, &[1.0, 0.5, 0.5, 1.0]).unwrap();
        let w = build(&best.genome);
        let goals = player::goals_for("house", (0,65,0), (8,70,8));
        let r = player::play(&w, (4, 65, -10), (0,65,0), (8,70,8), &goals,
                             &Body { budget: 4000, ..Default::default() }, None);
        println!("{threads} thread(s): {} evals in {:.1}s, front {} -- best {:?}",
                 out.evaluations, t0.elapsed().as_secs_f32(), out.front.len(), best.genome.0);
        println!("   the player says: {}", r.summary());
        println!("   memory held {} walk(s)", mem.len());
    }
}
