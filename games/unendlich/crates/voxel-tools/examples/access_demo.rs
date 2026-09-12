// A cliff with no way down, and the same cliff with one.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl W { fn h(x: i32) -> i32 { if x < 0 { 70 } else { 96 } } }
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= W::h(x) { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn section(w: &W, label: &str) {
    println!("   {label}");
    for y in (66..=99).rev().step_by(2) {
        let row: String = (-34..14).map(|x| match w.get(x, y, 0) { 0 => ' ', 4 => '=', 50 => '*', _ => '#' }).collect();
        println!("   {row}");
    }
}
fn main() {
    let mut before = W(BTreeMap::new());
    section(&before, "the cliff: 26 blocks, no way down");
    let _ = &mut before;
    println!();
    for (label, ir) in [("cliff_stair", "cliff_stair 10,0 dir=180 to=71 w=3 rail=cobblestone\n"),
                        ("switchback",  "switchback 10,0 dir=180 to=71 w=3 leg=12\n")] {
        let mut w = W(BTreeMap::new());
        let (plan, issues) = parse_plan_at(ir, None);
        for i in &issues { if i.severity == voxel_tools::plan::Severity::Error { println!("ERR {}", i.message); } }
        let r = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 96, 32, false) };
        println!("== {label}: {} blocks", r.blocks_changed);
        for l in &r.log { println!("   {}", l.split(" -> ").last().unwrap_or(l).trim()); }
        section(&w, "");
        println!();
    }
}
