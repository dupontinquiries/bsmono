// What the model is shown before it plans. All computed, no model call.
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};

struct T { f: fn(i32,i32)->i32, water: bool, cells: BTreeMap<(i32,i32,i32),u16> }
impl VoxelWorld for T {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.cells.get(&(x,y,z)) { return b; }
        let h = (self.f)(x, z);
        if y > h { return 0; }
        if self.water && y == h && h <= 62 { 9 } else { 1 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x,y,z), b); true }
}

fn main() {
    let sites: &[(&str, fn(i32,i32)->i32, bool)] = &[
        ("a hillside falling away west", |x, _| 64 + x / 2, false),
        ("a cliff to the north",         |_, z| if z < -4 { 84 } else { 64 }, false),
        ("a shore",                      |_, z| if z > 5 { 58 } else { 66 }, true),
    ];
    for (label, f, water) in sites {
        let mut w = T { f: *f, water: *water, cells: BTreeMap::new() };
        let tools = VoxelTools::new(&mut w);
        let s = voxel_tools::scene::read(&tools, (0, 65, 0), 20, 300, 15);
        println!("== {label}\n{}\n", s.prompt_block());
    }
}
