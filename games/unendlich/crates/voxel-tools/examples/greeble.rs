// One wall, three phrases, three evolved decorators.
use std::collections::BTreeMap;
use voxel_tools::gene::{evolve, faces, Fitness};
use voxel_tools::VoxelWorld;

struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn main() {
    // a two-thick wall, 24 wide, 9 high, facing +z
    let mut w = W(BTreeMap::new());
    for x in 0..24 { for y in 65..74 { for z in 0..2 { w.set(x, y, z, 1); } } }
    let f = faces(&w, (0, 65, 0), (23, 73, 1));
    let front: Vec<_> = f.iter().filter(|fc| fc.normal == (0, 0, 1)).cloned().collect();
    println!("{} faces on the front\n", front.len());

    for phrase in ["industrial", "gothic and symmetric", "clean minimal"] {
        let t0 = std::time::Instant::now();
        let e = evolve(&front, &Fitness::from_phrase(phrase), 11, 24, 14);
        let m = e.metrics;
        println!("== {phrase:22} score {:.2} in {:.0} ms   coverage {:.0}% edges {:.0}% depth {:.0}% noise {:.2} sym {:.2}",
                 e.score, t0.elapsed().as_secs_f32() * 1000.0,
                 m.coverage * 100.0, m.edges * 100.0, m.depth * 100.0, m.noise, m.symmetry);
        println!("   history: {}", e.history.iter().map(|s| format!("{s:.1}")).collect::<Vec<_>>().join(" "));
        for l in e.genome.to_text().lines().skip(1) { println!("   {l}"); }
        // render the front face: P protrusion, letters for put blocks, . untouched
        let touched: BTreeMap<_, _> = e.changes.iter().map(|c| (c.at, c.to)).collect();
        for y in (65..74).rev() {
            let row: String = (0..24).map(|x| {
                if touched.contains_key(&(x, y, 2)) { 'P' }
                else if let Some(b) = touched.get(&(x, y, 1)) { match b { 58 => 'B', 86 => 'D', 4 => 'c', 17 => 'L', 61 => 'g', 37 => 'm', 0 => ' ', _ => '#' } }
                else if touched.contains_key(&(x, y + 1, 1)) && y == 73 { '^' }
                else { '.' }
            }).collect();
            println!("   {row}");
        }
        println!();
    }
}
