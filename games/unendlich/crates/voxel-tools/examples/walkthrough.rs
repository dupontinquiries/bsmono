// Can you actually walk the building the programming chain just made?
use std::collections::BTreeMap;
use voxel_tools::{brief, reach, typology, VoxelWorld};
struct W(BTreeMap<(i32,i32,i32),u16>);
impl VoxelWorld for W {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { if let Some(&b)=self.0.get(&(x,y,z)) { return b } if y <= 64 { 1 } else { 0 } }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z),b); true }
}
fn main() {
    let req = "a castle";
    let t = typology::lookup(req).unwrap();
    let m = typology::apply_for(req, &t, &typology::triage(req, &t));
    let (b, _) = brief::parse(&typology::to_brief(&m, m.implied_total));
    let side = ((m.implied_total as f32 * 1.6).sqrt() / 2.0) as i32;
    let l = brief::lay_out(&b, ((-side, -side), (side, side)), 4, 40);
    let (p, _) = voxel_tools::program::parse(&brief::to_program(&l, &b, "stone", 0));
    let r = voxel_tools::program::run(&p, (0, 64, 0));
    let mut w = W(BTreeMap::new());
    for c in &r.changes { w.set(c.at.0, c.at.1, c.at.2, c.to); }
    println!("== {} rooms, {}", l.rooms.len(), r.summary());
    for room in &l.rooms { println!("   room {:<16} {:?}", room.name, room.qualities); }
    println!("   with outside_door: {}", l.rooms.iter().filter(|r| r.qualities.contains("outside_door")).count());
    // stand outside and try to reach the centre of each room
    let outside = (-side - 6, 65, -side - 6);
    let body = reach::Body { budget: 900_000, corridor: Some(200), ..Default::default() };
    let mut ok = 0;
    for room in &l.rooms {
        let (cx, cz) = (room.at.0 + room.size.0/2, room.at.1 + room.size.1/2);
        // A BODY STANDS ON THE FLOOR, not at the room's centre. Asking
        // for the centre of a 4-tall void puts the target two blocks
        // above anywhere anyone can stand, and every room reported
        // "within 2 blocks" -- of the answer.
        let floor = 64;
        let res = reach::reaches(&w, outside, (cx, floor, cz), &body);
        if res.ok { ok += 1; } else { println!("   UNREACHABLE {:<14} {}", room.name, res.summary((cx,floor,cz))); }
    }
    println!("   {ok}/{} rooms can be walked to from outside", l.rooms.len());
    let doors = reach::openings(&w, (-side, 64, -side), (side, 80, side));
    println!("   {} doorway(s) found by the critique", doors.len());
    // where IS the air, in a section through the middle?
    println!("\n   section at z=0, y 62..72 (# solid, space air, ? untouched):");
    for y in (62..72).rev() {
        let row: String = (-side..side).step_by(2).map(|x| match w.0.get(&(x,y,0)) {
            Some(0) => ' ', Some(_) => '#', None => if y <= 64 { 'g' } else { '?' } }).collect();
        println!("   {y:3}|{row}|");
    }
}
