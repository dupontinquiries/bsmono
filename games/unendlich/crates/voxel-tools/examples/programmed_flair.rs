use voxel_tools::{compose, program};
fn main() {
    for req in ["a huge fortress with a chapel", "a ruined citadel"] {
        let p = compose::programme(req, ((0,0),(0,0)), 5, None);
        let (prog, errs) = program::parse(&p.src);
        for e in errs.iter().take(3) { println!("ERR {e}"); }
        let r = program::run(&prog, (0, 64, 0));
        println!("== {req}");
        println!("   {}", p.summary());
        println!("   {} assets, {}", prog.assets.len(), r.summary());
        for n in r.notes.iter().filter(|n| n.contains("placement")) { println!("   {n}"); }
        println!("   bodies: {:?}", r.bodies.keys().take(8).collect::<Vec<_>>());
    }
}
