// The same request, three sites. Nothing in the plan names a material.
use std::collections::BTreeMap;
use voxel_tools::ambient::Ambient;
use voxel_tools::{VoxelTools, VoxelWorld};

struct Ground { block: u16, cells: BTreeMap<(i32,i32,i32), u16> }
impl VoxelWorld for Ground {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.cells.get(&(x,y,z)) { return b; }
        if y <= 64 { self.block } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x,y,z), b); true }
}

fn main() {
    let sites: &[(&str, u16, f32, f32, i32)] = &[
        ("a glacier",        32, 0.05, 0.60, 70),
        ("open desert",      12, 0.95, 0.05, 66),
        ("jungle floor",     23, 0.85, 0.90, 68),
        ("temperate meadow",  2, 0.55, 0.45, 66),
        ("a mountain shelf",  2, 0.35, 0.40, 150),
    ];
    for (label, ground, t, h, y) in sites {
        let mut w = Ground { block: *ground, cells: BTreeMap::new() };
        // put the surface at the requested height
        for x in -2..=2 { for z in -2..=2 { for yy in 65..=*y { w.set(x, yy, z, *ground); } } }
        let tools = VoxelTools::new(&mut w);
        let amb: Ambient = tools.read_ambient(0, 0, 250, *t, *h, 64);
        let s = amb.suggest();
        println!("{label:20} -> palette {:9} species {:7} structure={:3} path={:3}",
                 s.palette, s.species, s.structure, s.path);
        println!("{:22} {}", "", amb.prompt_line());
    }
}
