// Flair: greebles, locations with negatives, modifiers over the set.
use std::collections::{BTreeMap, BTreeSet};
use voxel_tools::flair::{self, Bodies};
fn main() {
    // a castle: ring wall, gatehouse in the south, keep in the middle
    let mut wall = BTreeSet::new();
    for x in -18i32..=18 { for z in -18i32..=18 { for y in 0..10 {
        if x.abs() == 18 || z.abs() == 18 { wall.insert((x, y, z)); } }}}
    let mut gate = BTreeSet::new();
    for x in -4i32..=4 { for z in -21i32..=-15 { for y in 0..13 { gate.insert((x, y, z)); } } }
    let mut keep = BTreeSet::new();
    for x in -6i32..=6 { for z in -6i32..=6 { for y in 0..20 { keep.insert((x, y, z)); } } }
    let mut b = Bodies::new();
    b.insert("curtain_wall".into(), wall);
    b.insert("gatehouse".into(), gate);
    b.insert("keep".into(), keep);
    let mut g: BTreeMap<String, Vec<((i32,i32,i32), u16)>> = BTreeMap::new();
    g.insert("merlon".into(), (0..2).map(|y| ((0,y,0), 1u16)).collect());
    g.insert("bracket".into(), vec![((0,0,0),58u16)]);
    g.insert("finial".into(), (0..3).map(|y| ((0,y,0), 51u16)).collect());

    let src = "\
flair merlon on curtain_wall top_edge except gatehouse all 5
  every 2
end
flair bracket on keep faces
  every 4
  clamp 14 17
end
flair finial on keep corners
  thin 5
end
";
    let (flairs, errs) = flair::parse(src);
    for e in &errs { println!("ERR {e}"); }
    let mut all: BTreeMap<(i32,i32,i32), u16> = BTreeMap::new();
    for f in &flairs {
        let a = flair::apply(f, &b, &g);
        for l in &a.log { println!("   {l}"); }
        for n in a.notes.iter().filter(|n| !n.contains("excluded")) { println!("   note {n}"); }
        for n in a.notes.iter().filter(|n| n.contains("excluded")) { println!("   {n}"); }
        for c in a.changes { all.insert(c.at, c.to); }
    }
    println!("\n   {} flair cells total", all.len());
    // plan view of the parapet, showing the gate gap
    println!("\n   the parapet at y=10 (M merlon, # wall, G gatehouse):");
    for z in -22..=20 {
        let row: String = (-22..=20).map(|x| {
            if all.contains_key(&(x, 10, z)) { 'M' }
            else if b["gatehouse"].contains(&(x, 10, z)) { 'G' }
            else if b["curtain_wall"].contains(&(x, 9, z)) { '#' } else { ' ' }
        }).collect();
        println!("   {row}");
    }
}
