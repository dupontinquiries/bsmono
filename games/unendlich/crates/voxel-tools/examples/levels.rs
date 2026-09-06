// One wall, one phrase, three levels: detail on detail, and what each level adds.
use std::collections::BTreeMap;
use voxel_tools::evolve::{Archive, Budget};
use voxel_tools::gene::{evolve_levels, Fitness};
use voxel_tools::VoxelWorld;
struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b) = self.0.get(&(x,y,z)) { return b; } if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    let mut w = W(BTreeMap::new());
    for x in 0..30 { for y in 65..76 { for z in 0..2 { w.set(x, y, z, 1); } } }
    let mut arch = Archive::default();
    let t0 = std::time::Instant::now();
    let levels = evolve_levels(&mut w, (0, 65, 0), (29, 75, 1), &Fitness::from_phrase("gothic ornate"), 11, 3,
                               Budget::evals(400, 24), &mut arch, "demo");
    println!("{} levels in {:.0} ms; archive holds {} elites\n", levels.len(), t0.elapsed().as_secs_f32() * 1000.0, arch.len());
    for (l, e) in &levels {
        let m = e.metrics;
        println!("L{l}: {:3} changes  coverage {:2.0}%  edges {:2.0}%  rhythm {:.2}  contrast {:.2}  noise {:.2}",
                 e.changes.len(), m.coverage * 100.0, m.edges * 100.0, m.rhythm, m.contrast, m.noise);
    }
    println!("\nfront elevation (z toward viewer): '#' wall, '+' protrusion, letters = surface blocks");
    for y in (64..78).rev() {
        let row: String = (-1..31).map(|x| {
            if w.get(x, y, 2) != 0 { '+' } else if w.get(x, y, 3) != 0 { '*' }
            else { match w.get(x, y, 1) { 0 => ' ', 1 => '#', 58 => 'B', 86 => 'D', 4 => 'c', 17 => 'L', 61 => 'g', 37 => 'M', _ => '?' } }
        }).collect();
        println!("  {row}");
    }
}
