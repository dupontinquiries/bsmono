// Composite terrain features: a place from one word.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl W { fn base(x: i32, z: i32) -> i32 { (74 + ((x as f32/50.0).sin()*5.0) as i32 + ((z as f32/60.0).cos()*4.0) as i32).clamp(64, 100) } }
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= W::base(x, z) { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    for name in ["badlands", "gorge", "caldera", "cliffs", "highlands", "sinkhole"] {
        let mut w = W(BTreeMap::new());
        let ir = format!("feature {name} @0,0 r=120 seed=4\n");
        let (plan, issues) = parse_plan_at(&ir, Some((0, 78, 0)));
        for i in &issues { if i.severity == voxel_tools::plan::Severity::Error { println!("  ERR {}", i.message); } }
        let t0 = std::time::Instant::now();
        let r = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        println!("== {name}: {} blocks in {:.1}s", r.blocks_changed, t0.elapsed().as_secs_f32());
        for l in &r.log { for seg in l.split(" -- ").skip(1) { println!("   {}", seg.trim()); } }
        let top = (60..130).rev().find(|&y| (-60..60).any(|x| w.get(x, y, 0) != 0)).unwrap_or(100);
        for y in ((top-34).max(50)..=top).rev().step_by(3) {
            let row: String = (-60..60).map(|x| match w.get(x, y, 0) { 0 => ' ', 9 => '~', 12 => '.', 24 => ':', 18|20 => '*', _ => '#' }).collect();
            println!("   {row}");
        }
        println!();
    }
}
