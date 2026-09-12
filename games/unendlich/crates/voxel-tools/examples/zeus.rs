// The sentence from the screenshot, the 1B's reply, and what the player gets now.
use std::collections::BTreeMap;
use voxel_tools::chat::{ChatSession, Reply};
use voxel_tools::focus::{Gaze, GazeHit};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b) = self.0.get(&(x,y,z)) { return b; } if y <= 64 { 2 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    let gaze = Gaze { eye: (0.0, 80.0, -30.0), dir: (0.0, -0.3, 1.0),
                      hit: Some(GazeHit { block: (0, 64, 0), face_normal: (0, 1, 0), distance: 30.0 }) };
    let mut w = W(BTreeMap::new());
    let mut t = VoxelTools::new(&mut w);
    let mut s = ChatSession::new(32, 400_000);
    let prose = "a giant stone statue of Zeus holding a lightning bolt in a lunge combat position";
    let reply = "cylinder @2,-1 r=2 h=10 block=stone\ntree @6,4 oak h=8\n";   // the 1B, verbatim
    match s.receive_plan_gated(&mut t, &gaze, reply, None, prose) {
        Reply::Preview { echo, blocks, .. } => {
            println!("{blocks} blocks");
            for l in echo.lines().filter(|l| l.contains("RECOGNIZED") || l.contains("FINISHING")) { println!("  {}", l.trim()); }
        }
        o => println!("{o:?}"),
    }
    s.commit(&mut t, &gaze);
    let top = (64..200).rev().find(|&y| (-20..20).any(|x| (-20..20).any(|z| w.get(x, y, z) != 0))).unwrap();
    println!("\nside elevation (looking west; z across, y down), {} tall:", top - 64);
    for y in (64..=top).rev().step_by(2) {
        let row: String = (-14..=16).map(|z| {
            (-12..12).map(|x| w.get(x, y, z)).find(|b| *b != 0).map(|b| match b { 37 => '*', 58 => 'B', 2 => 'g', _ => '#' }).unwrap_or(' ')
        }).collect();
        println!("  {row}");
    }
}
