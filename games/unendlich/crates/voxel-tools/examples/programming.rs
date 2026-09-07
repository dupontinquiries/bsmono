// The full chain: request -> triage -> massing -> brief -> layout -> geometry.
use voxel_tools::{brief, typology};
fn main() {
    let request = "a forbidding castle with a huge keep and a chapel, on a cliff, without stables";
    println!("REQUEST: {request}\n");
    let t = typology::lookup(request).expect("a known type");
    println!("== TYPOLOGY: a {} is {}", t.name, t.summary);
    let wishes = typology::triage(request, &t);
    println!("\n== TRIAGE");
    for w in &wishes { println!("   {:<10} {:?}", w.kind(), w); }
    let m = typology::apply(&t, &wishes);
    println!("\n== MASSING");
    print!("{}", m.summary(5000));
    for l in &m.log { println!("   -> {l}"); }
    for u in &m.unplaced { println!("   ?? {u}"); }
    let src = typology::to_brief(&m, 5000);
    let (b, errs) = brief::parse(&src);
    for e in &errs { println!("   ERR {e}"); }
    println!("\n== BRIEF: {}", b.reconcile().summary());
    println!("   {} spaces, {} adjacencies", b.spaces.len(), b.adjacencies.len());
    let l = brief::lay_out(&b, ((-45, -45), (45, 45)), 4, 40);
    println!("\n== LAYOUT: {}", l.summary());
    for n in l.notes.iter().take(3) { println!("   NOTE {n}"); }
    let prog = brief::to_program(&l, &b, "stone", 0);
    let (p, _) = voxel_tools::program::parse(&prog);
    let run = voxel_tools::program::run(&p, (0, 64, 0));
    println!("\n== GEOMETRY: {}", run.summary());
}
