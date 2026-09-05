// Build one guard post, teach it, then use it as vocabulary.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan, run_plan_with};
use voxel_tools::template::{capture, Library};
use voxel_tools::{VoxelTools, VoxelWorld};

struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn plan_at(w: &W, y: i32, x: std::ops::RangeInclusive<i32>, z: std::ops::RangeInclusive<i32>) {
    for zz in z {
        let row: String = x.clone().map(|xx| match w.get(xx, y, zz) {
            0 => '.', 1 => '#', 58 => '=', 86 => '%', 48 => 's', 24 => 'r', _ => 'o' }).collect();
        println!("  {row}");
    }
}

fn main() {
    let mut w = W(BTreeMap::new());
    // 1. build something worth keeping: an L-shaped post with a marker
    let (p, _) = parse_plan_at(
        "enclose @0,0 @4,4 h=4 block=stone_bricks\ncylinder @0,0 r=1 h=7 block=stone\n",
        Some((0, 65, 0)));
    { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &p, 200, 32, false); }
    println!("the original, at y=66:");
    plan_at(&w, 66, -1..=5, -1..=5);

    // 2. teach it
    let mut lib = Library::new();
    lib.add(capture(&w, "guardpost", (0, 65, 0), (4, 72, 4), "stone")).unwrap();
    println!("\n{}", lib.get("guardpost").unwrap().summary());
    println!("{}", lib.vocabulary());

    // 3. use it: four around a courtyard, each facing out, each different
    let mut w2 = W(BTreeMap::new());
    let ir = "\
place guardpost @20,0 rot=0 vary=0.05 seed=1
place guardpost @20,20 rot=90 vary=0.05 seed=2
place guardpost @0,20 rot=180 vary=0.05 seed=3
place guardpost @0,0 rot=270 vary=0.05 seed=4
";
    let (p2, issues) = parse_plan_at(ir, Some((0, 65, 0)));
    for i in &issues { println!("{:?}: {}", i.severity, i.message); }
    let run = { let mut t = VoxelTools::new(&mut w2);
                run_plan_with(&mut t, &p2, 200, 32, false, Some(&lib)) };
    println!("\nfour posts from one description: {} blocks", run.blocks_changed);
    plan_at(&w2, 66, -2..=27, -2..=27);

    // 4. and the same post in a desert palette, no recapture
    let mut w3 = W(BTreeMap::new());
    let (p3, _) = parse_plan_at("place guardpost @0,0 palette=desert\n", Some((0, 65, 0)));
    let r3 = { let mut t = VoxelTools::new(&mut w3);
               run_plan_with(&mut t, &p3, 200, 32, false, Some(&lib)) };
    println!("\nthe same post, re-materialized in desert ({} blocks):", r3.blocks_changed);
    plan_at(&w3, 66, -1..=5, -1..=5);
}
