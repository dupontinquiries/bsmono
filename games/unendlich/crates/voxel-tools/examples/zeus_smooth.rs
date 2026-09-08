// A figure with and without the joint fillet.
use voxel_tools::{program, study::{self, Rung, Study}};
fn main() {
    let mut st = Study::default();
    study::absorb(&mut st, Rung::Subject, "SUBJECT: Zeus, striding\n");
    study::absorb(&mut st, Rung::Anatomy, "PARTS: head 12%; torso 32%; arm 36%; leg 47%\nWIDTH: 26%\n");
    study::absorb(&mut st, Rung::Pose, "POSE: right arm raised\n");
    let src = study::to_sketch(&st, 60, "stone");
    let (prog, _) = program::parse(&format!("pass a add\n{src}end\n"));
    let r = program::run(&prog, (0, 64, 0));
    println!("== {}", r.summary());
    for n in r.notes.iter().filter(|n| n.contains("smoothed")) { println!("   {n}"); }
    let solid: std::collections::BTreeSet<(i32,i32,i32)> =
        r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    let (lo, hi) = solid.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h),c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    for y in (lo.1..=hi.1).rev().step_by(2) {
        let row: String = (lo.0..=hi.0).map(|x|
            if (lo.2..=hi.2).any(|z| solid.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
