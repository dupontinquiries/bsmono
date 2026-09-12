// A landscape from four lines: raise a range, roughen it, forest the
// slopes, run a river off it.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};

struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 2 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn height(w: &W, x: i32, z: i32) -> i32 { (0..200).rev().find(|&y| w.get(x, y, z) != 0).unwrap() }

fn main() {
    let ir = "\
# a valley
brush raise 0,0 by=28 r=70 hill
brush raise 120,0 by=22 r=60 mesa
brush roughen 60,0 by=3 r=110 flat
layer boreal -60,-60 60,60 density=0.7 gentle
layer meadow 60,-40 120,40 density=0.5
river -30,10 dir=180 len=300
";
    let mut w = W(BTreeMap::new());
    let (plan, issues) = parse_plan_at(ir, Some((0, 65, 0)));
    for i in &issues { println!("{:?}: {}", i.severity, i.message); }
    let t0 = std::time::Instant::now();
    let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
    println!("{} blocks in {:.2}s\n", run.blocks_changed, t0.elapsed().as_secs_f32());
    for l in &run.log { println!("  {l}"); }

    println!("\nheight profile along z=0 (x -80..170):");
    let hs: Vec<i32> = (-80..=170).step_by(5).map(|x| height(&w, x, 0)).collect();
    let (lo, hi) = (*hs.iter().min().unwrap(), *hs.iter().max().unwrap());
    for y in (lo..=hi).rev().step_by(3) {
        let row: String = hs.iter().map(|h| if *h >= y { '#' } else { ' ' }).collect();
        println!("  {y:3} {row}");
    }
    let water = w.0.values().filter(|b| **b == 9).count();
    let trees = w.0.values().filter(|b| **b == 17 || **b == 18 || **b == 19).count();
    println!("\n{water} water cells, {trees} log/leaf cells");
}
