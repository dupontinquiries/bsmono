use std::collections::BTreeSet;
use voxel_tools::{graft, program};
fn main() {
    for req in ["a lighthouse made of bones", "a tower that branches like a tree",
                "a cathedral in the manner of a nautilus", "a hut built like timber framing",
                "a stone lighthouse"] {
        match graft::read(req) {
            Some((m, s)) => println!("{req:42} -> {}", graft::Graft { subject: s, manner: m }.summary()),
            None => println!("{req:42} -> not a graft"),
        }
    }
    // and what the manner does to real cells
    let (p, _) = program::parse(
        "pass t add\n  part t profile r=7,6,5,4,3 h=22 material=stone\n  rel t on ground\nend\n");
    let r = program::run(&p, (0, 64, 0));
    let cells: BTreeSet<(i32,i32,i32)> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    println!("\n== a tower of {} cells, in each manner:", cells.len());
    for m in graft::known() {
        let ch = graft::apply_surface(&m, &cells, 5);
        let hollowed = ch.iter().filter(|c| c.to == 0).count();
        let mats: BTreeSet<u16> = ch.iter().filter(|c| c.to != 0).map(|c| c.to).collect();
        println!("   {:<9} {} hollowed, {} left, {} material(s)", m.name, hollowed,
                 ch.len() - hollowed, mats.len());
    }
    println!("\n{}", graft::known()[0].prompt());
}
