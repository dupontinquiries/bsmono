// The dictionary's statue against one built from a study.
use std::collections::BTreeSet;
use voxel_tools::study::{self, Rung, Study};
fn draw(cells: &BTreeSet<(i32,i32,i32)>, label: &str) {
    if cells.is_empty() { println!("   {label}: nothing"); return; }
    let (lo, hi) = cells.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h),c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    println!("   {label}: {} cells, {}w x {}h", cells.len(), hi.0-lo.0+1, hi.1-lo.1+1);
    let sy = (((hi.1-lo.1)/30).max(1)) as usize;
    for y in (lo.1..=hi.1).rev().step_by(sy) {
        let row: String = (lo.0..=hi.0).map(|x|
            if (lo.2..=hi.2).any(|z| cells.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
fn main() {
    let mut st = Study::default();
    study::absorb(&mut st, Rung::Subject, "SUBJECT: Zeus, striding\nCARRIES: a thunderbolt in the raised right hand\nWITH: an eagle at the left foot\n");
    study::absorb(&mut st, Rung::Anatomy, "PARTS: head 12%; beard 7%; torso 32%; arm 36%; leg 47%; bolt 20%; eagle 12%\nWIDTH: shoulders 26%\n");
    study::absorb(&mut st, Rung::Pose, "POSE: right arm raised; striding, left leg bearing\nSILHOUETTE: a broad triangle broken by the bolt\n");
    study::absorb(&mut st, Rung::Voxelise, "KEEP: the beard as one mass; the bolt at twice scale\nDROP: fingers\n");
    let src = study::to_sketch(&st, 90, "stone");
    println!("== the sketch the study wrote\n{}", src.lines().map(|l| format!("   {l}")).collect::<Vec<_>>().join("\n"));
    let (sk, errs) = voxel_tools::sketch::parse(&src);
    for e in &errs { println!("ERR {e}"); }
    let (placed, notes) = voxel_tools::sketch::resolve(&sk, (0, 64, 0));
    let mut cells = BTreeSet::new();
    for p in &placed { for c in voxel_tools::sketch::cells(p, 1) { cells.insert((c.0,c.1,c.2)); } }
    println!("\n== built from the study");
    for n in notes.iter().take(3) { println!("   note {n}"); }
    draw(&cells, "front");
}
