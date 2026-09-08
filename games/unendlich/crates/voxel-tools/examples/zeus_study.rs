// The study ladder's answers, as a body.
use voxel_tools::{sketch, study};
fn main() {
    let mut s = study::Study::default();
    study::absorb(&mut s, study::Rung::Subject,
        "SUBJECT: Zeus, king of the gods, striding\nCARRIES: a thunderbolt in the raised right hand\n\
         WITH: an eagle at the left foot\nKNOWN BY: the beard and the raised bolt\n");
    study::absorb(&mut s, study::Rung::Anatomy,
        "PARTS: head 12%; beard 7%; torso 32%; arm 36%; leg 47%; bolt 20%\nWIDTH: shoulders 26%\n");
    study::absorb(&mut s, study::Rung::Pose,
        "POSE: right arm raised high; left leg bearing\nSILHOUETTE: a broad triangle broken by the bolt\n");
    study::absorb(&mut s, study::Rung::Voxelise,
        "KEEP: the beard as one mass; the bolt at twice scale\nDROP: fingers -- the hand is one block\n");
    println!("== {}", s.summary());
    let h: i32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);
    let src = study::to_sketch(&s, h, "stone");
    let (sk, errs) = sketch::parse(&src);
    for e in &errs { println!("ERR {e}"); }
    println!("   {} parts, {} relations", sk.parts.len(), sk.rels.len());
    let (placed, notes) = sketch::resolve(&sk, (0, 64, 0));
    for n in &notes { println!("   {n}"); }
    let cells: Vec<_> = placed.iter().flat_map(|p| sketch::cells(p, 1)).collect();
    let set: std::collections::BTreeSet<(i32,i32,i32)> = cells.iter().map(|c| (c.0,c.1,c.2)).collect();
    let (lo, hi) = (set.iter().map(|c| c.1).min().unwrap(), set.iter().map(|c| c.1).max().unwrap());
    let (xl, xh) = (set.iter().map(|c| c.0).min().unwrap(), set.iter().map(|c| c.0).max().unwrap());
    println!("   {} cells, {}w x {}h", set.len(), xh-xl+1, hi-lo+1);
    let step = ((hi - lo) / 34).max(1);
    for y in (lo..=hi).rev().step_by(step as usize) {
        let row: String = (xl..=xh).map(|x| if (-3..=3).any(|z| set.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
