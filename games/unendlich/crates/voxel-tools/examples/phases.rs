// A castle built over four hundred years.
use std::collections::{BTreeMap, BTreeSet};
use voxel_tools::{history, program, transform};
fn main() {
    let (mut h, errs) = history::parse(
        "phase keep age=400 material=stone\n  A square tower put up in a hurry.\nend\n\
         phase hall age=250 material=sandstone against=keep\n  A great hall against its south wall.\nend\n\
         phase chapel age=120 material=quartz against=hall\n  Added by a widow, in better stone.\nend\n\
         phase repairs age=20 material=cobblestone against=keep,hall\n  Patching, done cheaply.\nend\n");
    for e in &errs { println!("ERR {e}"); }
    h.weather_by_age();
    println!("== a castle over four hundred years:\n{}", h.summary());
    // build each phase where it belongs, and weather it by its age
    let plans = [("keep", "part k box w=14 h=22 d=14 material=stone\nrel k on ground\n", 0),
                 ("hall", "part a box w=22 h=11 d=13 material=sandstone\nrel a on ground\n", 17),
                 ("chapel", "part c box w=9 h=13 d=11 material=quartz\nrel c on ground\n", 38),
                 ("repairs", "part r box w=6 h=8 d=6 material=cobblestone\nrel r on ground\n", -8)];
    let mut all: BTreeSet<(i32,i32,i32)> = BTreeSet::new();
    for (name, src, dx) in plans {
        let ph = h.phases.iter().find(|p| p.name == name).unwrap();
        let (prog, _) = program::parse(&format!("pass p add\n{src}end\n"));
        let r = program::run(&prog, (dx, 64, 0));
        let mut cells: BTreeSet<(i32,i32,i32)> =
            r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
        let mut note = String::new();
        if let Some((v, s)) = ph.weathering {
            let blocks: BTreeMap<(i32,i32,i32), u16> = cells.iter().map(|c| (*c, 1u16)).collect();
            let a = transform::apply(v, &cells, &blocks, s, 5);
            for c in &a.changes { if c.to == 0 { cells.remove(&c.at); } else { cells.insert(c.at); } }
            note = format!("  <- {}", a.note);
        }
        println!("   {name}: {} cells{note}", cells.len());
        all.extend(cells);
    }
    let (lo, hi) = all.iter().fold(((i32::MAX,i32::MAX),(i32::MIN,i32::MIN)),
        |(l,h),c| ((l.0.min(c.0),l.1.min(c.1)),(h.0.max(c.0),h.1.max(c.1))));
    println!("\n   the whole place, {} cells:", all.len());
    for y in (lo.1..=hi.1).rev().step_by(2) {
        let row: String = (lo.0..=hi.0).map(|x|
            if (-14..=14).any(|z| all.contains(&(x,y,z))) { '#' } else { ' ' }).collect();
        println!("   |{row}|");
    }
}
