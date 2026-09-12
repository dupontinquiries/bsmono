// A model draws two elevations; the tool reconstructs, builds, and
// reports how faithful the reconstruction is.
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

fn main() {
    // Exactly what a model would emit: a picture, not coordinates.
    let ir = "\
views @0,0 depth=9
front:
....#....
...###...
..#####..
.#######.
#########
.#######.
..#####..
side:
....#....
...###...
..#####..
.#######.
#########
.#######.
..#####..
end
";
    println!("the model's reply:\n{ir}");
    let mut w = Flat(BTreeMap::new());
    let (plan, issues) = parse_plan_at(ir, Some((0, 65, 0)));
    for i in &issues { println!("  {:?}: {}", i.severity, i.message); }
    let run = { let mut t = VoxelTools::new(&mut w); run_plan(&mut t, &plan, 200, 32, false) };
    println!("built {} blocks", run.blocks_changed);
    for l in &run.log { println!("  {l}"); }

    // Read it back as elevations: what was drawn is what stands.
    for (label, fixed) in [("front (z=4)", 4), ("side (x=4)", 4)] {
        println!("\n{label}:");
        for y in (65..72).rev() {
            let row: String = (0..9).map(|i| {
                let solid = if label.starts_with("front") { w.get(i, y, fixed) }
                            else { w.get(fixed, y, i) };
                if solid != 0 { '#' } else { '.' }
            }).collect();
            println!("  {row}");
        }
    }
    // A horizontal slice through the middle shows the third dimension
    // neither drawing stated: the intersection is a square-ish disc.
    println!("\nplan at the waist (y=69), which NEITHER drawing specified:");
    for z in 0..9 {
        let row: String = (0..9).map(|x| if w.get(x, 69, z) != 0 { '#' } else { '.' }).collect();
        println!("  {row}");
    }
}
