// Carved terrain at scale: what a heightfield cannot say.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl W {
    fn base(x: i32, z: i32) -> i32 {
        // a plateau in the east, a valley through the middle
        let ridge = 78 + (x as f32 / 30.0).sin() as i32 * 4;
        let v = ((z as f32 / 40.0).cos() * 6.0) as i32;
        (ridge + v + x / 12).clamp(64, 104)
    }
}
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= W::base(x, z) { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    for (label, ir) in [
        ("canyon", "canyon @-80,0 @80,0 w=18 depth=26 floor=sand wall=sandstone seed=3\n"),
        ("cave",   "cave @60,0 dir=180 len=110 r=5 branches=4 seed=7\n"),
        ("mesa",   "mesa @0,0 r=30 drop=22 seed=5\n"),
        ("waterfall", "waterfall @0,0 dir=180 w=9 h=22 seed=2\n"),
        ("overhang",  "overhang @0,0 dir=180 w=24 depth=12 h=7 seed=4\n"),
    ] {
        let mut w = W(BTreeMap::new());
        let (plan, issues) = parse_plan_at(ir, Some((0, 80, 0)));
        for i in &issues { if i.severity == voxel_tools::plan::Severity::Error { println!("  ERR {}", i.message); } }
        let t0 = std::time::Instant::now();
        let r = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        println!("== {label}: {} blocks in {:.0} ms", r.blocks_changed, t0.elapsed().as_secs_f32() * 1000.0);
        for l in &r.log { println!("   {}", l.split(" -> ").last().unwrap_or(l).trim()); }
        // a section: z = 0, looking along x
        let (x0, x1) = (-40, 40);
        let top = (60..120).rev().find(|&y| (x0..x1).any(|x| w.get(x, y, 0) != 0)).unwrap_or(100);
        for y in ((top - 30).max(56)..=top).rev().step_by(2) {
            let row: String = (x0..x1).map(|x| match w.get(x, y, 0) { 0 => ' ', 9 => '~', 12 => '.', 24 => ':', _ => '#' }).collect();
            println!("   {row}");
        }
        println!();
    }
}
