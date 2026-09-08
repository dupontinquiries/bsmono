// Is the castle this crate builds actually usable?
use voxel_tools::{brief, program, typology, unshown};
fn main() {
    let req = "a castle";
    let t = typology::lookup(req).unwrap();
    let m = typology::apply_for(req, &t, &typology::triage(req, &t));
    let (b, _) = brief::parse(&typology::to_brief(&m, m.implied_total));
    let side = ((m.implied_total as f32 * 1.6).sqrt() / 2.0) as i32;
    let l = brief::lay_out(&b, ((-side, -side), (side, side)), 4, 40);
    let (p, _) = program::parse(&brief::to_program(&l, &b, "stone", 0));
    let r = program::run(&p, (0, 64, 0));
    let solid: std::collections::BTreeSet<(i32,i32,i32)> =
        r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    println!("== the castle as built: {} cells, {} rooms", solid.len(), l.rooms.len());
    let dark = unshown::plan_lights(&solid, 7, 2);
    println!("   {}", dark.summary());
    let ch = unshown::light_changes(&dark, "glowstone");
    println!("   {} light block(s) would fix it -- {:.2}% of the build",
             ch.len(), ch.len() as f32 / solid.len() as f32 * 100.0);
    println!("\n== what the video leaves out, for this build:");
    for o in unshown::omissions(false, false, false) { println!("   - {o}"); }
}
