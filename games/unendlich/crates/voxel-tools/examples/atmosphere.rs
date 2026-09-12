// Three prompts that are not (only) about blocks, decomposed.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{mood, VoxelTools, VoxelWorld};

struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn main() {
    for prose in ["build an eerie forest with horrifying creatures",
                  "dig an underground city with NPCs named Alex",
                  "build a fire dragon nest"] {
        println!("== {prose}");
        let moods = mood::recognize(prose);
        let names: Vec<&str> = moods.iter().map(|m| m.name).collect();
        println!("   moods: {names:?}");
        if !moods.is_empty() {
            let a = mood::blend(&moods);
            println!("   atmosphere for the engine: fog {:.1}, light {:.2}, warmth {:+.2}, time {:?}",
                     a.fog, a.light, a.warmth, a.time);
        }
        for (kind, disp, count, name) in mood::creatures(prose) {
            println!("   creatures: {count} x '{kind}' ({}){}", disp.word(),
                     name.map(|n| format!(", named {n}")).unwrap_or_default());
        }
    }

    // and one of them as the IR a model would emit, run for real
    println!("\n== the dragon nest, as IR:");
    let ir = "\
# dragon nest
form nest @-12,-12 @12,12 depth=6 rim=3 block=basalt
mood fiery @-16,-16 @16,16
populate fire dragon @-6,-6 @6,6 count=1 name=Vex
";
    let mut w = W(BTreeMap::new());
    let (plan, issues) = parse_plan_at(ir, Some((0, 65, 0)));
    for i in &issues { println!("   {:?}: {}", i.severity, i.message); }
    let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
    for l in &run.log { println!("   {l}"); }
    println!("   cross-section through the nest (x=0):");
    for y in (58..70).rev() {
        let row: String = (-14..=14).map(|z| match w.get(0, y, z) {
            0 => '.', 38 => 'B', 96 => 'M', 49 => 'O', _ => '#' }).collect();
        println!("   {row}");
    }
}
