use voxel_tools::{brief, VoxelWorld};
struct W(std::collections::BTreeMap<(i32,i32,i32),u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&v) = self.0.get(&(x,y,z)) { return v } if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, v: u16) -> bool { self.0.insert((x,y,z), v); true }
}
fn main() {
    let b = brief::parse("space keep area=220 height=4\nspace yard area=120\nadj keep yard must\n").0;
    let l = brief::lay_out(&b, ((-20,-20),(20,20)), 3, 30);
    let k = l.rooms.iter().find(|r| r.name=="keep").unwrap();
    let src = brief::to_program(&l, &b, "stone", 0);
    let (p,e) = voxel_tools::program::parse(&src);
    for x in &e { println!("ERR {x}"); }
    let r = voxel_tools::program::run(&p, (0,64,0));
    println!("== {} ({} storeys)", r.summary(), k.height);
    let mut w = W(Default::default());
    for c in &r.changes { w.set(c.at.0,c.at.1,c.at.2,c.to); }
    let sz = k.at.1 + 2;
    println!("   section through the stair (z={sz}):");
    for y in (63..=64+k.height*5+2).rev() {
        let row: String = (k.at.0..k.at.0+k.size.0).map(|x| if w.get(x,y,sz)!=0 {'#'} else {' '}).collect();
        println!("   y={y:3} |{row}|");
    }
    let body = voxel_tools::reach::Body { budget: 40_000, corridor: None, ..Default::default() };
    let (seen, _) = voxel_tools::reach::flood(&w, (k.at.0+2, 66, k.at.1+2), &body);
    println!("   a body starting on the ground floor reaches y={} ({} cells)",
             seen.iter().map(|c| c.1).max().unwrap_or(0), seen.len());
}
