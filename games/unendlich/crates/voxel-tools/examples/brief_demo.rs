// Architectural programming: schedule -> adjacency -> layout -> geometry.
use voxel_tools::brief::{self, Adjacency};
fn main() {
    let src = "budget 1200\ncirculation 0.35\n\
        space great hall area=240 height=2 needs=daylight,tall\n\
        space kitchen area=90 needs=wet,noisy\n\
        space buttery area=40\n\
        space solar area=70 needs=daylight,private\n\
        space chapel area=60 needs=daylight,tall\n\
        space lodging area=45 count=4 needs=daylight\n\
        space stores area=60\n\
        space guardroom area=50 needs=outside_door\n\
        adj kitchen buttery must\nadj buttery great hall must\n\
        adj solar great hall should\nadj chapel solar should\n\
        adj lodging kitchen avoid\nadj stores kitchen should\n\
        adj guardroom great hall should\nadj lodging stores avoid\n";
    let (b, errs) = brief::parse(src);
    for e in &errs { println!("ERR {e}"); }
    let r = b.reconcile();
    println!("== the schedule");
    for l in &r.lines { println!("{l}"); }
    println!("   {}", r.summary());
    println!("\n== the matrix");
    for (a, c, h) in &b.adjacencies {
        println!("   {a:<12} {:<8} {c}", match h { Adjacency::Must => "MUST", Adjacency::Should => "should",
                                                   Adjacency::Avoid => "AVOID", _ => "-" });
    }
    let l = brief::lay_out(&b, ((-26, -26), (26, 26)), 9, 60);
    println!("\n== the layout: {}", l.summary());
    for n in &l.notes { println!("   NOTE {n}"); }
    for u in &l.unmet { println!("   UNMET {u}"); }
    // plan view, one letter per room
    let key: Vec<char> = "HKBSCLLLLTG".chars().collect();
    for z in -26..26 {
        let row: String = (-26..26).map(|x| {
            match l.rooms.iter().position(|r| x >= r.at.0 && x < r.at.0 + r.size.0
                                           && z >= r.at.1 && z < r.at.1 + r.size.1) {
                Some(i) => key.get(i).copied().unwrap_or('?'),
                None => '.',
            }
        }).collect();
        println!("   {row}");
    }
    for (i, r) in l.rooms.iter().enumerate() {
        println!("   {} = {} {}x{}", key.get(i).copied().unwrap_or('?'), r.name, r.size.0, r.size.1);
    }
    let prog = brief::to_program(&l, &b, "stone", 0);
    let (p, e) = voxel_tools::program::parse(&prog);
    let run = voxel_tools::program::run(&p, (0, 64, 0));
    println!("\n== the geometry: {} (errs {})", run.summary(), e.len());
}
