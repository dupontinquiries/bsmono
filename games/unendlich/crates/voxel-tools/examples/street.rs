// One line, six houses, one style: the seed makes them a street.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b) = self.0.get(&(x,y,z)) { return b; } if y <= 64 { 2 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    for style in ["cottage", "tudor", "nordic", "desert", "gothic"] {
        let mut w = W(BTreeMap::new());
        let ir: String = (0..4).map(|i| format!("form house @{},0 @{},8 style={style} seed={}\n", i * 16, i * 16 + 12, i + 1)).collect();
        let (plan, issues) = parse_plan_at(&ir, Some((0, 65, 0)));
        for i in &issues { if i.severity == voxel_tools::plan::Severity::Error { println!("{}", i.message); } }
        let r = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
        println!("== {style}: {} blocks, four houses -- front elevation (z=0, looking north)", r.blocks_changed);
        let top = (64..100).rev().find(|&y| (-2..66).any(|x| w.get(x, y, 0) != 0 || w.get(x, y, 1) != 0 || w.get(x, y, -1) != 0)).unwrap_or(70);
        for y in (64..=top).rev() {
            let row: String = (-2..66).map(|x| {
                // the nearest solid along z for this x,y: a front elevation
                (-4..10).map(|z| w.get(x, y, z)).find(|b| *b != 0).map(|b| match b {
                    57 => '#', 86 => 'D', 17 => 'L', 48 => 'S', 58 => 'B', 4 => 'c', 24 => 'r', 1 => '#', 20 | 18 => 'l', 19 => 'b', 37 => 'M', _ => '+' }).unwrap_or(' ')
            }).collect();
            println!("  {row}");
        }
        println!();
    }
}
