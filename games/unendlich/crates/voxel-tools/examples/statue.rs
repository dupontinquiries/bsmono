// "a creepy stone statue": what a 1B says, and what the words add.
use std::collections::BTreeMap;
use voxel_tools::chat::{ChatSession, Reply};
use voxel_tools::focus::{Gaze, GazeHit};
use voxel_tools::{VoxelTools, VoxelWorld};

struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}

fn section(w: &W, z: i32) {
    for y in (63..92).rev() {
        let row: String = (-10..=16).map(|x| match w.get(x, y, z) {
            0 => ' ', 1 => '#', 58 => '=', 86 => 'D', 4 => 'c', 23 => 'm', 37 => 'M', _ => '+' }).collect();
        println!("  {row}");
    }
}

fn main() {
    let gaze = Gaze { eye: (3.0, 70.0, -12.0), dir: (0.0, -0.4, 1.0),
                      hit: Some(GazeHit { block: (3, 64, 3), face_normal: (0, 1, 0), distance: 14.0 }) };
    for (label, ir, prose) in [
        ("what the 1B did: a pillar and a ball",
         "cylinder @0,0 r=2 h=8 block=stone\nsphere @0,0 r=2 y=73 block=stone_bricks\n", "a creepy stone statue"),
        ("with the statue form, plain",
         "form statue @-3,-3 @3,3 gargoyle pose=standing h=18\n", "a stone statue"),
        ("with the statue form, and the word creepy doing its work",
         "form statue @-3,-3 @3,3 gargoyle pose=standing h=18\n", "a creepy stone statue"),
    ] {
        let mut w = W(BTreeMap::new());
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        match s.receive_plan_gated(&mut t, &gaze, ir, None, prose) {
            Reply::Preview { echo, blocks, .. } => {
                println!("== {label}: {blocks} blocks");
                for l in echo.lines().filter(|l| l.contains("FINISHING") || l.contains("mood") && l.contains("engine")) {
                    println!("   {}", l.trim());
                }
            }
            o => println!("{o:?}"),
        }
        s.commit(&mut t, &gaze);
        section(&w, 3);
        println!();
    }
}
