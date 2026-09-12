// Does the geometry match the plan? Compare the layout to the cells.
use voxel_tools::{brief, typology};
fn main() {
    let req = "a castle";
    let t = typology::lookup(req).unwrap();
    let m = typology::apply_for(req, &t, &typology::triage(req, &t));
    let (b, _) = brief::parse(&typology::to_brief(&m, m.implied_total));
    let side = ((m.implied_total as f32 * 1.6).sqrt() / 2.0) as i32;
    let l = brief::lay_out(&b, ((-side, -side), (side, side)), 4, 40);
    println!("== {} on a {}x{} site", l.summary(), side*2, side*2);
    let src = brief::to_program(&l, &b, "stone", 0);
    let (p, e) = voxel_tools::program::parse(&src);
    for x in &e { println!("ERR {x}"); }
    let r = voxel_tools::program::run(&p, (0, 64, 0));
    println!("   {}", r.summary());
    let air: std::collections::BTreeSet<_> = r.changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
    // for each room, is there air where the layout said?
    let mut hit = 0;
    for room in &l.rooms {
        let (cx, cz) = (room.at.0 + room.size.0/2, room.at.1 + room.size.1/2);
        if air.contains(&(cx, 66, cz)) { hit += 1; }
        else { println!("   MISS {} at {cx},{cz}", room.name); }
    }
    println!("   {hit}/{} rooms are hollow where the plan says", l.rooms.len());
    // plan of the voids at y=66
    println!("\n   voids at y=66 (. solid shell, space = air):");
    for z in (-side..side).step_by(3) {
        let row: String = (-side..side).step_by(2)
            .map(|x| if air.contains(&(x, 66, z)) { ' ' } else { '.' }).collect();
        println!("   |{row}|");
    }
}
