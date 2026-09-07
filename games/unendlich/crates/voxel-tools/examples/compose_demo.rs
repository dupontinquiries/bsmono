// The four passes: what the model says, and what comes out.
use std::collections::BTreeMap;
use voxel_tools::chat::{ChatSession, Reply};
use voxel_tools::compose::{self, parse_reading};
use voxel_tools::focus::{Gaze, GazeHit};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b)=self.0.get(&(x,y,z)) { return b } if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z),b); true }
}
fn ch(b: u16) -> char { match b { 0 => ' ', 9 => '~', 61 => 'o', 31 => '*', 51 => ':', 1 => '#', _ => '?' } }
fn main() {
    let g = Gaze { eye: (0.0, 72.0, -22.0), dir: (0.0, -0.2, 1.0), hit: Some(GazeHit { block: (0,64,0), face_normal: (0,1,0), distance: 22.0 }) };
    let cases: [(&str, &str, &str); 2] = [
      ("a water sphere",
       "IT IS: a ball of water held aloft, needing a skin to hold it\nPARTS: a glass shell; the water within; an ice core\nMATERIALS: glass, water, ice\nSCALE: 16\nAVOID: a solid blue blob; anything shaped like a building",
       "part shell sphere r=8 material=glass hollow\npart water sphere r=7 material=water\npart core sphere r=2 material=ice\nrel shell above ground by=10\nrel water inside shell\nrel core inside water\n"),
      ("the sound of a bell, as a building",
       "IT IS: not a bell, but the shape a bell's sound makes: rings spreading outward\nPARTS: a tall shaft; rings around it, rising and widening\nMATERIALS: stone, quartz\nSCALE: 40\nAVOID: building an actual bell",
       "part shaft cylinder r=2 h=34 material=stone\npart ring torus r=10 thickness=1 material=quartz\nrel shaft on ground\nrel ring stack shaft count=5 gap=5\n"),
    ];
    for (prose, reading_txt, sketch) in cases {
        println!("== {prose}");
        println!("   PATH: {:?}  (the dictionary has no word for it)", compose::choose_path(prose));
        let r = parse_reading(reading_txt);
        println!("   PASS 1 says: {}", r.essence);
        println!("   PASS 2 wrote {} lines of parts and relations", sketch.lines().count());
        let mut w = W(BTreeMap::new());
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        match s.receive_sketch(&mut t, &g, sketch, None, prose) {
            Reply::Preview { blocks, echo, .. } => {
                println!("   PASS 3+4: {blocks} blocks");
                for l in echo.lines().filter(|l| l.starts_with("SKETCH")) { println!("   {}", l.trim()); }
            }
            o => { println!("   {o:?}"); continue; }
        }
        s.commit(&mut t, &g);
        drop(t);
        let top = (64..130).rev().find(|&y| (-20..20).any(|x| w.get(x,y,0) != 0)).unwrap_or(70);
        for y in ((top-40).max(64)..=top).rev().step_by(2) {
            let row: String = (-20..20).map(|x| ch(w.get(x,y,0))).collect();
            println!("   |{row}|");
        }
        println!();
    }
}
