// A castle, and the places that follow from it.
use voxel_tools::{ensemble, VoxelWorld};
struct Land;
impl VoxelWorld for Land {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        let ridge = (34.0 - (x as f32).abs() * 0.35).max(0.0) as i32;
        if x < -150 && x > -190 {
            if y <= 60 { return 1; }
            if y <= 63 { return voxel_tools::palette::resolve_or_suggest("water").0; }
            return 0;
        }
        let _ = z;
        if y <= 64 + ridge { 1 } else { 0 }
    }
    fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
}
fn main() {
    let (e, errs) = ensemble::parse(
        "place village kind=village serves=castle reach=within sight link=road\n  \
           People settle under a wall they can run behind.\nend\n\
         place mill kind=watermill serves=village reach=a short walk link=track\n  \
           The village grows grain and someone has to grind it.\nend\n\
         place chapel kind=chapel serves=village reach=adjoining link=none\n  \
           Big enough now to bury its own dead.\nend\n\
         place quarry kind=quarry serves=castle reach=half a day link=road\n  \
           The castle's stone came from somewhere, and it is still coming.\nend\n",
        "castle", "castle");
    for x in &errs { println!("ERR {x}"); }
    println!("== {}\n", e.summary());
    println!("== build order: {:?}\n", e.in_order().unwrap().iter().map(|s| &s.name).collect::<Vec<_>>());
    let (placed, notes) = ensemble::place(&Land, &e, (0, 0), 40, 64);
    for n in &notes { println!("   {n}"); }
    println!("\n   plan (C = castle):");
    let all: Vec<(&str, (i32,i32))> = std::iter::once(("castle", (0,0)))
        .chain(placed.iter().map(|p| (p.name.as_str(), p.at))).collect();
    let (xl, xh) = (all.iter().map(|p| p.1.0).min().unwrap(), all.iter().map(|p| p.1.0).max().unwrap());
    let (zl, zh) = (all.iter().map(|p| p.1.1).min().unwrap(), all.iter().map(|p| p.1.1).max().unwrap());
    let sx = ((xh - xl) / 60).max(1); let sz = ((zh - zl) / 14).max(1);
    for z in (zl..=zh).step_by(sz as usize) {
        let row: String = (xl..=xh).step_by(sx as usize).map(|x| {
            all.iter().find(|(_, a)| (a.0 - x).abs() < sx && (a.1 - z).abs() < sz)
                .map(|(n, _)| n.chars().next().unwrap().to_ascii_uppercase()).unwrap_or(' ')
        }).collect();
        println!("   |{row}|");
    }
    for p in &placed { if let Some((_, l)) = p.link_from {
        println!("   {} reached by {}", p.name, l.name()); } }
}
