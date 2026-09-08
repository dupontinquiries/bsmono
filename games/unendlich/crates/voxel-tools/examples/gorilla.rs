use std::collections::BTreeSet;
use voxel_tools::{compose, program, study::{self, Rung, Study}};
fn main() {
    for req in ["a gorilla statue", "a giant tree with vines"] {
        println!("== {req}  -> {:?}", compose::choose_path(req));
    }
    // what the study path actually builds for a gorilla
    let mut s = Study::default();
    study::absorb(&mut s, Rung::Subject, "SUBJECT: a gorilla, crouching on its knuckles\n");
    study::absorb(&mut s, Rung::Anatomy, "PARTS: head 14%; torso 38%; arm 45%; leg 30%\nWIDTH: 40%\n");
    println!("   plan from subject: {:?}", s.plan);
    let src = study::to_sketch(&s, 30, "stone");
    let (prog, errs) = program::parse(&format!("pass a add\n{src}end\n"));
    for e in errs.iter().take(2) { println!("   ERR {e}"); }
    let r = program::run(&prog, (0, 64, 0));
    let solid: BTreeSet<(i32,i32,i32)> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    let lo = solid.iter().map(|c| c.1).min().unwrap_or(0);
    println!("   {} cells, {} parts, lowest y={} (ground is 64)", solid.len(), prog.passes[0].name.len(), lo);
    if lo > 65 { println!("   *** FLOATING by {} blocks", lo - 64); }
    for n in r.notes.iter().take(4) { println!("   note {n}"); }
}
