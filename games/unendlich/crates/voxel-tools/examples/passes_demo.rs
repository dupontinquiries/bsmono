// Assets first, then passes that add or cut.
use std::collections::BTreeMap;
use voxel_tools::chat::{ChatSession, Reply};
use voxel_tools::focus::{Gaze, GazeHit};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b)=self.0.get(&(x,y,z)) { return b } if y <= 80 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z),b); true }
}
fn main() {
    let g = Gaze { eye: (0.0, 90.0, -30.0), dir: (0.0, -0.2, 1.0), hit: Some(GazeHit { block: (0,80,0), face_normal: (0,1,0), distance: 30.0 }) };
    let cases: [(&str, &str); 2] = [
      ("carve a great schism into the ground",
       "pass rift cut\n  part gash box w=90 h=26 d=16 material=stone\n  rel gash below ground by=0\nend\n"),
      ("a keep with a gate and merlons",
       "asset merlon\n  part tooth box w=2 h=3 d=2 material=stone\nend\n\
        pass mass add\n  part keep box w=24 h=16 d=24 material=stone hollow\n  rel keep on ground\nend\n\
        pass gate cut\n  part door box w=5 h=8 d=26 material=stone\n  rel door on ground\nend\n\
        pass teeth add\n  repeat merlon line from=-11,9,-12 to=11,9,-12 step=4\nend\n"),
    ];
    for (prose, prog) in cases {
        let mut w = W(BTreeMap::new());
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        println!("== {prose}");
        match s.receive_program(&mut t, &g, prog, None, prose) {
            Reply::Preview { echo, .. } => for l in echo.lines() { println!("   {}", l.trim()); },
            o => { println!("   {o:?}"); continue; }
        }
        s.commit(&mut t, &g);
        drop(t);
        for y in (68..=100).rev().step_by(2) {
            let zs = if prose.starts_with("carve") { 0 } else { -12 };
            let row: String = (-26..26).map(|x| if w.get(x,y,zs) != 0 { '#' } else if y <= 80 { '.' } else { ' ' }).collect();
            println!("   |{row}|");
        }
        println!();
    }
}
