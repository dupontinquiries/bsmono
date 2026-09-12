// What can today's DSL NOT say? The honest test for whether code generation
// would help.
use voxel_tools::program;
struct Ground;
impl voxel_tools::VoxelWorld for Ground {
    fn get(&self, x: i32, y: i32, _z: i32) -> u16 { if y <= 64 + (x / 8).clamp(0, 12) { 1 } else { 0 } }
    fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
}
fn try_it(label: &str, src: &str) {
    let (p, errs) = program::parse(src);
    let r = program::run_in(&Ground, &p, (0, 64, 0));
    let cells = r.changes.len();
    println!("{label:52} {}", if !errs.is_empty() { format!("PARSE FAIL: {}", errs[0]) }
             else if cells == 0 { "BUILT NOTHING".into() } else { format!("ok, {cells} cells") });
}
fn main() {
    try_it("a ring of 8 towers", "asset t\n part s cylinder r=2 h=10 material=stone\nend\npass a add\n repeat t ring radius=20 count=8\nend\n");
    try_it("...each one bigger than the last (scale=1+t*1.4)", "asset t\n part s cylinder r=2 h=10 material=stone\nend\npass a add\n repeat t ring radius=20 count=8 scale=1+t*1.4\nend\n");
    try_it("a spiral of towers rising", "pass a add\n part s helix r=20 h=40 turns=2 material=stone\nend\n");
    try_it("towers only where the ground is flat", "asset t\n part s cylinder r=2 h=10 material=stone\nend\npass a add\n repeat t grid min=-40,0,-40 max=40,0,40 step=10 on_ground max_slope=2\nend\n");
    try_it("a wall following an arbitrary path", "frame path\n a -30,0,-20\n b 0,0,10\n c 30,0,-15\nend\nasset s\n part x box w=2 h=5 d=2 material=stone\nend\npass a add\n repeat s along path step=2\nend\n");
    try_it("30 windows spaced by the wall's own length", "asset w\n part x box w=1 h=2 d=1 material=stone\nend\npass a add\n repeat w line from=-30,4,0 to=30,4,0 step=len/30\nend\n");
}
