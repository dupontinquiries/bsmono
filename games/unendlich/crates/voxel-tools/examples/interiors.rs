// The castle this crate builds, furnished and stocked.
use std::collections::BTreeSet;
use voxel_tools::{brief, contents, typology};
fn main() {
    let req = "a castle";
    let t = typology::lookup(req).unwrap();
    let m = typology::apply_for(req, &t, &typology::triage(req, &t));
    let (b, _) = brief::parse(&typology::to_brief(&m, m.implied_total));
    let side = ((m.implied_total as f32 * 1.6).sqrt() / 2.0) as i32;
    let l = brief::lay_out(&b, ((-side, -side), (side, side)), 4, 40);
    println!("== {} rooms", l.rooms.len());
    let (mut cells, mut chests) = (0usize, 0usize);
    for r in &l.rooms {
        let mut floor = BTreeSet::new();
        for x in r.at.0..r.at.0 + r.size.0 { for z in r.at.1..r.at.1 + r.size.1 {
            floor.insert((x, 64, z)); }}
        let outer: BTreeSet<(i32,i32,i32)> = floor.iter().copied()
            .filter(|c| c.0 == r.at.0 || c.0 == r.at.0 + r.size.0 - 1).collect();
        let room = contents::Room { name: r.name.clone(), floor, outer };
        let f = contents::furnish(&room, 7);
        let loot = contents::stock(&r.name, &f.containers, 7);
        cells += f.changes.len();
        chests += loot.len();
        println!("\n   {}", contents::describe(&f, &loot));
        for x in loot.iter().take(2) { println!("     {} at {:?}: {}", x.container, x.at, x.items.join(", ")); }
    }
    println!("\n== {cells} cells of furnishing, {chests} containers");
}
