// A settlement, and then time passing over it.
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};
use voxel_tools::plan::{parse_plan_at, run_plan};

struct Flat(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for Flat {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn plan_view(w: &Flat, y: i32, r: i32) {
    for z in -r..=r {
        let row: String = (-r..=r).map(|x| match w.get(x, y, z) {
            0 => '.', 1 => '#', 23 => 'm', 4 => 'o', 29 => 'v', 86 => '=', _ => '+' }).collect();
        println!("  {row}");
    }
}

fn main() {
    let mut w = Flat(BTreeMap::new());
    let (plan, issues) = parse_plan_at(
        "form village @-24,-24 @24,24 count=6 seed=3\n", Some((0, 65, 0)));
    for i in &issues { println!("{:?}: {}", i.severity, i.message); }
    let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
    println!("village: {} blocks\n", run.blocks_changed);
    println!("plan at y=67 (houses ring a centre, paths run inward):");
    plan_view(&w, 67, 24);

    let (p2, _) = parse_plan_at(
        // ABSOLUTE coords here, deliberately: `@-24,60,-24` would mean 60
    // blocks ABOVE the anchor, not y=60, and the weathering would land
    // in the sky. The `@` prefix is relative on every axis including Y.
    "weather -24,60,-24 24,80,24 ancient seed=5\n", Some((0, 65, 0)));
    let r2 = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &p2, 200, 32, false) };
    println!("\nafter `weather ancient`: {} blocks changed", r2.blocks_changed);
    println!("m = moss, o = rubble, v = vines\n");
    plan_view(&w, 66, 14);
}
