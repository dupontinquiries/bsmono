// What can today's DSL NOT say? The honest test for whether code generation
// would help.
use voxel_tools::program;
fn try_it(label: &str, src: &str) {
    let (p, errs) = program::parse(src);
    let r = program::run(&p, (0, 64, 0));
    let cells = r.changes.len();
    println!("{label:52} {}", if !errs.is_empty() { format!("PARSE FAIL: {}", errs[0]) }
             else if cells == 0 { "BUILT NOTHING".into() } else { format!("ok, {cells} cells") });
}
fn main() {
    try_it("a ring of 8 towers", "asset t\n part s cylinder r=2 h=10 material=stone\nend\npass a add\n repeat t ring radius=20 count=8\nend\n");
    try_it("...each one bigger than the last (scale=1+t*1.4)", "asset t\n part s cylinder r=2 h=10 material=stone\nend\npass a add\n repeat t ring radius=20 count=8 scale=1+t*1.4\nend\n");
    try_it("a spiral of towers rising", "pass a add\n part s helix r=20 h=40 turns=2 material=stone\nend\n");
    try_it("towers only where the ground is flat", "pass a add\n repeat t grid min=-40,0,-40 max=40,0,40 step=10 if=flat\nend\n");
    try_it("a wall following an arbitrary path", "pass a add\n part w link thickness=2 material=stone\nend\n");
    try_it("30 windows spaced by the wall's own length", "pass a add\n repeat w line from=-30,4,0 to=30,4,0 step=len/30\nend\n");
}
