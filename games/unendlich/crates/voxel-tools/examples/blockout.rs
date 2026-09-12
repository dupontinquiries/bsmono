// Block out the whole thing, judge it, fix the massing, and only then detail.
use voxel_tools::{dossier, program};
fn draw(src: &str, label: &str) {
    let (p, _) = program::parse(src);
    let r = program::run(&p, (0, 64, 0));
    let solid: std::collections::BTreeSet<(i32,i32,i32)> =
        r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    if solid.is_empty() { println!("   {label}: nothing"); return; }
    let (lo, hi) = solid.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h),c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    println!("\n   {label}: {} cells, {}w x {}h", solid.len(), hi.0-lo.0+1, hi.1-lo.1+1);
    let sy = (((hi.1-lo.1)/18).max(1)) as usize;
    for y in (lo.1..=hi.1).rev().step_by(sy) {
        let row: String = (lo.0..=hi.0).map(|x|
            if (lo.2..=hi.2).any(|z| solid.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
fn main() {
    let (mut d, _) = dossier::parse(
        "is a tower carrying sails, with the machinery below\n\
         part base size=400\n  A round stone base, wide and low.\nend\n\
         part tower size=900 after=base\n  A tapering stone tower.\nend\n\
         part cap size=200 after=tower\n  A domed timber cap.\nend\n\
         part sails size=300 after=cap\n  Four long latticed sails.\nend\n");
    draw(&dossier::blockout(&d, (0, 64, 0)).1, "blockout as planned");
    // the judgement a builder makes standing back
    let v = dossier::parse_verdict("reads no\nsize base 1200\nsize sails 1600\nsize tower 2400\n");
    println!("\n   verdict: {}", v.summary());
    for l in d.revise(&v) { println!("     {l}"); }
    draw(&dossier::blockout(&d, (0, 64, 0)).1, "blockout revised");
    println!("\n   the whole revision cost 3 lines and no rebuild.");
}
