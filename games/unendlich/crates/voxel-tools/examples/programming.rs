// The full chain: request -> triage -> massing -> brief -> layout -> geometry.
use voxel_tools::{brief, typology};
fn main() {
    let request = std::env::args().nth(1).unwrap_or_else(|| "a huge fortress with a chapel, on a cliff".into());
    let request = request.as_str();
    println!("REQUEST: {request}\n");
    let t = typology::lookup(request).expect("a known type");
    println!("== TYPOLOGY: a {} is {}", t.name, t.summary);
    let wishes = typology::triage(request, &t);
    println!("\n== TRIAGE");
    for w in &wishes { println!("   {:<10} {:?}", w.kind(), w); }
    let m = typology::apply_for(request, &t, &wishes);
    let total = m.implied_total;
    println!("\n== MASSING");
    print!("{}", m.summary(total));
    for l in &m.log { println!("   -> {l}"); }
    for u in &m.unplaced { println!("   ?? {u}"); }
    let src = typology::to_brief(&m, total);
    let (b, errs) = brief::parse(&src);
    for e in &errs { println!("   ERR {e}"); }
    println!("\n== BRIEF: {}", b.reconcile().summary());
    println!("   {} spaces, {} adjacencies", b.spaces.len(), b.adjacencies.len());
    let side = ((total as f32 * 1.6).sqrt() / 2.0) as i32;
    let l = brief::lay_out(&b, ((-side, -side), (side, side)), 4, 40);
    println!("   site {}x{}", side*2, side*2);
    println!("\n== LAYOUT: {}", l.summary());
    for n in l.notes.iter().take(3) { println!("   NOTE {n}"); }
    // the massing as sections, each its own body
    let mprog = typology::to_program(&m, total, side * 2, "stone");
    let (mp, me) = voxel_tools::program::parse(&mprog);
    for e in &me { println!("   ERR {e}"); }
    let mrun = voxel_tools::program::run(&mp, (0, 64, 0));
    println!("\n== SECTIONS: {}", mrun.summary());
    for (n, cells) in &mrun.bodies {
        let top = cells.iter().map(|c| c.1).max().unwrap() - 64;
        let w = cells.iter().map(|c| c.0).max().unwrap() - cells.iter().map(|c| c.0).min().unwrap();
        println!("   {n:<16} {:>7} cells  {w:>3} wide  {top:>3} tall", cells.len());
    }
    let prog = brief::to_program(&l, &b, "stone", 0);
    let (p, _) = voxel_tools::program::parse(&prog);
    let run = voxel_tools::program::run(&p, (0, 64, 0));
    println!("\n== GEOMETRY: {}", run.summary());
}
