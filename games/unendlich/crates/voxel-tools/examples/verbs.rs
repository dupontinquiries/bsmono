// One build, nine verbs.
use std::collections::{BTreeMap, BTreeSet};
use voxel_tools::{program, transform::{self, Verb}};
fn main() {
    let (p, _) = program::parse(
        "pass tower add\n  part t profile r=7,6,5,5,4,3 h=26 material=stone\n  rel t on ground\nend\n");
    let r = program::run(&p, (0, 64, 0));
    let cells: BTreeSet<(i32,i32,i32)> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    let blocks: BTreeMap<(i32,i32,i32), u16> = r.changes.iter().map(|c| (c.at, c.to)).collect();
    println!("== a tower: {} cells", cells.len());
    for v in Verb::all() {
        let a = transform::apply(v, &cells, &blocks, 0.7, 5);
        let mut after = cells.clone();
        for c in &a.changes { if c.to == 0 { after.remove(&c.at); } else { after.insert(c.at); } }
        let (lo, hi) = after.iter().fold(((i32::MAX,i32::MAX),(i32::MIN,i32::MIN)),
            |(l,h),c| ((l.0.min(c.0),l.1.min(c.1)),(h.0.max(c.0),h.1.max(c.1))));
        println!("\n   {:<12} {}", v.name(), a.note);
        for y in (lo.1..=hi.1).rev().step_by(3) {
            let row: String = (lo.0..=hi.0).map(|x|
                if after.contains(&(x,y,0)) { '#' } else { ' ' }).collect();
            println!("   |{row}|");
        }
    }
}
