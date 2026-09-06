// A canyon dressed from strata: the bands it cut, lining up across it.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 96 + ((x as f32/40.0).sin()*3.0) as i32 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn ch(b: u16) -> char { match b { 0 => ' ', 12 => '.', 24 => ':', 172 => '=', 179 => '%', 9 => '~', 1 => '#', 3 => ',', 2 => '"', _ => '?' } }
fn main() {
    for (label, ir) in [("one flat wall= colour", "canyon 0,-60 0,60 w=16 depth=30 wall=sandstone seed=3\n"),
                        ("strata=sedimentary",    "canyon 0,-60 0,60 w=16 depth=30 strata=sedimentary seed=3\n")] {
        let mut w = W(BTreeMap::new());
        let (plan, _) = parse_plan_at(ir, None);
        let r = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        println!("== {label}: {} blocks", r.blocks_changed);
        // cross-section across the canyon at z = 0
        for y in (64..=100).rev().step_by(2) {
            let row: String = (-22..22).map(|x| ch(w.get(x, y, 0))).collect();
            println!("   {row}");
        }
        println!();
    }
    // and the profile as text
    let p = voxel_tools::strata::lookup("sedimentary").unwrap();
    println!("{}", p.describe(0, 0, 30, 3));
}
