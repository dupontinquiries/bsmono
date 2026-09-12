// A flair pass inside a program: the prompt's own example, built.
use voxel_tools::program;
fn main() {
    let src = "\
asset merlon
  part m box w=1 h=2 d=1 material=stone
end
pass curtain wall add
  part w box w=40 h=10 d=40 material=stone hollow
  rel w on ground
end
pass gatehouse add
  part g box w=8 h=13 d=8 material=stone
  rel g beside w dir=south by=-4
end
pass battlements add
  flair merlon on curtain_wall top_edge except gatehouse all 6
    every 2
  end
end
";
    let (p, errs) = program::parse(src);
    for e in &errs { println!("ERR {e}"); }
    let r = program::run(&p, (0, 64, 0));
    println!("== {}", r.summary());
    for n in &r.notes { println!("   {n}"); }
    println!("   bodies: {:?}", r.bodies.keys().collect::<Vec<_>>());
    let solid: std::collections::BTreeSet<_> = r.changes.iter().filter(|c| c.to != 0).map(|c| c.at).collect();
    let top = solid.iter().map(|c| c.1).max().unwrap();
    println!("\n   plan at the parapet (y={}, M merlon, # wall/gate):", top - 1);
    for z in -26..=22 {
        let row: String = (-24..=24).map(|x| {
            if solid.contains(&(x, top, z)) { 'M' }
            else if solid.contains(&(x, top - 2, z)) { '#' } else { ' ' }
        }).collect();
        println!("   {row}");
    }
}
