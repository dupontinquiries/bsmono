use std::collections::BTreeSet;
use voxel_tools::study::{self, Rung, Study};
fn main() {
    let mut s = Study::default();
    study::absorb(&mut s, Rung::Subject, "SUBJECT: a giant emerald frog, squat and wide\nKNOWN BY: the wide mouth and bulging eyes\n");
    study::absorb(&mut s, Rung::Anatomy, "PLAN: quadruped\nPARTS: head 26%; torso 42%; leg 48%; eye 8%\nWIDTH: 62%\n");
    study::absorb(&mut s, Rung::Pose, "POSE: squatting, legs splayed\nSILHOUETTE: a low wide wedge\n");
    let src = study::to_sketch(&s, 40, "emerald");
    println!("{}", src.lines().map(|l| format!("   {l}")).collect::<Vec<_>>().join("\n"));
    let (sk, errs) = voxel_tools::sketch::parse(&src);
    for e in &errs { println!("ERR {e}"); }
    let (placed, _) = voxel_tools::sketch::resolve(&sk, (0, 64, 0));
    let (p2, _) = voxel_tools::program::parse(&format!("pass a add\n{src}end\n"));
    let run = voxel_tools::program::run(&p2, (0, 64, 0));
    let cells: BTreeSet<(i32,i32,i32)> = run.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    let _ = placed;
    let (lo, hi) = cells.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h),c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    println!("\n   {} cells, {}w x {}h x {}d", cells.len(), hi.0-lo.0+1, hi.1-lo.1+1, hi.2-lo.2+1);
    println!("   side view:");
    for y in (lo.1..=hi.1).rev() {
        let row: String = (lo.2..=hi.2).map(|z| if (lo.0..=hi.0).any(|x| cells.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
