// Honest test: what does the current pipeline make when asked for a statue?
use voxel_tools::program;
fn render(r: &program::Run, label: &str, z: i32) {
    let solid: Vec<_> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    if solid.is_empty() { println!("   {label}: nothing"); return; }
    let (lo, hi) = solid.iter().fold(((i32::MAX,i32::MAX,i32::MAX),(i32::MIN,i32::MIN,i32::MIN)),
        |(l,h), c| ((l.0.min(c.0),l.1.min(c.1),l.2.min(c.2)),(h.0.max(c.0),h.1.max(c.1),h.2.max(c.2))));
    let set: std::collections::BTreeSet<_> = solid.iter().copied().collect();
    println!("   {label}: {} cells, {}w x {}h x {}d", solid.len(), hi.0-lo.0+1, hi.1-lo.1+1, hi.2-lo.2+1);
    for y in (lo.1..=hi.1).rev() {
        let row: String = (lo.0..=hi.0).map(|x| if set.contains(&(x,y,z)) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
fn main() {
    // what a model would plausibly write with today's vocabulary
    let src = "pass body add\n\
                 part torso capsule r=3.2 r2=2.4 len=11 pitch=90 material=stone\n\
                 part head sphere r=2.6 material=stone\n\
                 part arm capsule r=1.2 r2=0.8 len=9 dir=200 pitch=-55 material=stone\n\
                 part leg capsule r=1.5 r2=1.1 len=10 dir=190 pitch=-80 material=stone\n\
                 rel torso above ground by=10\n\
                 rel head on torso\n\
                 rel arm joint torso at=0.85 out=0.9 pair\n\
                 rel leg joint torso at=0.05 out=0.5 pair\n\
               end\n";
    let (p, errs) = program::parse(src);
    if !errs.is_empty() { println!("ERRORS: {errs:?}"); }
    let r = program::run(&p, (0, 64, 0));
    println!("== a statue of a person, with today's vocabulary");
    render(&r, "front (z=0)", 0);
    for n in &r.notes { println!("   NOTE {n}"); }
}
