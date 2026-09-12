// "a bunker": the model writes a spec and a plan; the pool tunes the plan.
use std::collections::BTreeMap;
use voxel_tools::chat::{ChatSession, Reply};
use voxel_tools::focus::{Gaze, GazeHit};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b) = self.0.get(&(x,y,z)) { return b; } if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    let gaze = Gaze { eye: (0.0, 72.0, -14.0), dir: (0.0, -0.4, 1.0),
                      hit: Some(GazeHit { block: (0, 64, 0), face_normal: (0, 1, 0), distance: 16.0 }) };
    let mut w = W(BTreeMap::new());
    let mut t = VoxelTools::new(&mut w);
    let mut s = ChatSession::new(32, 400_000);
    // what a model writes: a thin open box, and the numbers it MEANT
    let ir = "# bunker\nspec height=5-7 durability=high sound=muffled shelter=high openness=0.2\nenclose @-6,-6 @6,6 h=5 block=stone\nsphere @6,0 r=1 block=air\n";
    match s.receive_plan_gated(&mut t, &gaze, ir, None, "a bunker") {
        Reply::Preview { echo, blocks, .. } => {
            println!("{blocks} blocks after tuning\n");
            for l in echo.lines().filter(|l| l.contains("target:") || l.contains("FINISHING") || l.contains("spec")) { println!("  {}", l.trim()); }
        }
        o => println!("{o:?}"),
    }
    println!("\ntraining record:\n  {}", s.training.first().map(|l| l.replace('\t', "  |  ")).unwrap_or_default());
    s.commit(&mut t, &gaze);
    println!("\nsection through the bunker (z=0):");
    for y in (63..74).rev() {
        let row: String = (-9..=9).map(|x| match w.get(x, y, 0) { 0 => '.', _ => '#' }).collect();
        println!("  {row}");
    }
}
