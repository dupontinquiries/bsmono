// Hostile input to the parser and the ops: a model can emit anything.
use std::collections::BTreeMap;
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::{VoxelTools, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b) = self.0.get(&(x,y,z)) { return b; } if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn main() {
    let cases: &[&str] = &[
        "", " ", "\n\n\n", "   \n  \n", "@", "@,", "@,,", "cylinder @", "cylinder @,", "cylinder r=", "cylinder r=x h=",
        "cylinder @99999999999,0", "cylinder @0,0 r=2147483647 h=2147483647", "cylinder @0,0 r=-2147483648",
        "fill @0,0,0 @2147483647,2147483647,2147483647", "sphere @0,0 r=99999", "terraform @-99999,-99999 @99999,99999",
        "brush raise @0,0 by=2147483647 r=400", "layer boreal @-1000,-1000 @1000,1000 density=9", "river @0,0 len=2147483647",
        "road @0,0 @2147483647,0", "lake @0,0 r=80 level=-2147483648", "def a\nuse a @0,0\n", "def\nend\n", "use\n", "def a\ndef b\nend\n",
        "def x\nuse x @0,0\nend\nuse x @0,0\n", "form", "form statue", "form statue @0,0 @0,0", "form village @0,0 @1,1 count=99999",
        "greeble", "greeble @0,0,0 @0,0,0", "greeble @0,0,0 @200,200,200 gens=999 genome=s0:", "genome=;;;", "views\nend\n",
        "views @0,0\nfront:\nend\n", "plant", "plant @0,0 ", "populate", "populate @0,0", "mood", "mood @0,0 @1,1", "weather",
        "world", "world flood", "place", "place nothing @0,0 rot=999999999", "detail", "detail speckle", "capture", "\u{0}\u{0}",
        "cylinder @0,0 r=3 h=9 # comment", "CYLINDER @0,0", "cylinder\t@0,0", "cylinder @0,0 r=3 h=9 block=", "cylinder @0,0 block=\u{1F600}",
        "roof @0,0 @0,0", "roof @-100,-100 @100,100 overhang=999", "stairs @0,0 @0,0", "bridge @0,0 @0,0", "mirror @0,0,0 @0,0,0 x",
        "scatter @0,0 @0,0 clusters=0 per=0", "tree @0,0 oak h=0", "tree @0,0 oak h=-5", "path @0,0 @0,0 w=0", "enclose @0,0 @0,0 h=0",
        "restyle", "restyle @0,0,0 @1,1,1 nope nope", "views @0,0 depth=0\nfront:\n#\nend\n",
    ];
    let mut panics = 0;
    let mut slow = 0;
    for (i, c) in cases.iter().enumerate() {
        let c = c.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut w = W(BTreeMap::new());
                let (plan, _issues) = parse_plan_at(&c, Some((0, 65, 0)));
                let mut t = VoxelTools::new(&mut w);
                let _ = run_plan(&mut t, &plan, 200, 32, true);
            }));
            let _ = tx.send(r.is_err());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(true) => { panics += 1; println!("PANIC on case {i}: {:?}", cases[i]); }
            Ok(false) => {}
            Err(_) => { slow += 1; println!("SLOW  (>2s) case {i}: {:?}", cases[i]); }
        }
    }
    println!("{} slow", slow);
    let (plan, _) = parse_plan_at("cylinder @0,0 r=3 h=9\n", None);
    let _ = plan;
    println!("{} cases, {panics} panics", cases.len());
}
