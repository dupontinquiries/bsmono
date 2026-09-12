// Does @0,0 land where you are looking? Three worlds where the answer
// is currently no.
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::focus::{Focus, Gaze, GazeHit};

/// Terrain at `ground`, plus a cave ceiling far above at `ceiling`.
struct Layered { ground: i32, ceiling: Option<i32>, cells: BTreeMap<(i32,i32,i32), u16> }
impl VoxelWorld for Layered {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.cells.get(&(x,y,z)) { return b; }
        if y <= self.ground { return 1; }
        if let Some(c) = self.ceiling { if y >= c && y <= c + 3 { return 1; } }
        0
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x,y,z), b); true }
}

fn place(world: &mut Layered, gaze: &Gaze, from_y: i32, label: &str) {
    let f = Focus::from_gaze(gaze);
    let (plan, _) = parse_plan_at("cylinder @0,0 r=1 h=3 block=stone_bricks\n", Some(f.anchor));
    let mut t = VoxelTools::new(world);
    let run = run_plan(&mut t, &plan, from_y, 32, false);
    let built: Vec<i32> = (-70..340).filter(|&y| t.world.get(f.anchor.0, y, f.anchor.2) == 58).collect();
    println!("{label}\n   anchor {:?}  built at y={:?}  ({} blocks)",
             f.anchor, built, run.blocks_changed);
    let ok = built.first().map(|&y| (y - f.anchor.1).abs() <= 1).unwrap_or(false);
    println!("   lands at the anchor? {}", if ok { "YES" } else { "NO  <-- wrong place" });
}

fn main() {
    let g = |bx: i32, by: i32, bz: i32| Gaze {
        eye: (bx as f32, by as f32 + 6.0, bz as f32 - 8.0), dir: (0.0, -0.4, 1.0),
        hit: Some(GazeHit { block: (bx, by, bz), face_normal: (0, 1, 0), distance: 9.0 }) };

    let mut w = Layered { ground: 64, ceiling: None, cells: BTreeMap::new() };
    place(&mut w, &g(0, 64, 0), 200, "surface at y=64, looking at the ground");

    let mut w = Layered { ground: 250, ceiling: None, cells: BTreeMap::new() };
    place(&mut w, &g(0, 250, 0), 200, "mountain at y=250, ABOVE the search ceiling");

    let mut w = Layered { ground: 20, ceiling: Some(120), cells: BTreeMap::new() };
    place(&mut w, &g(0, 20, 0), 200, "cave floor y=20 under a ceiling at y=120");

    // Negative coordinates: div_euclid, not integer division, or the
    // chunk for x=-1 is 0 instead of -1 and the edit lands in the wrong
    // chunk entirely.
    let mut w = Layered { ground: 64, ceiling: None, cells: BTreeMap::new() };
    place(&mut w, &g(-33, 64, -33), 200, "negative coordinates across a chunk boundary");

    // Deep underground, below sea level and below y=0.
    let mut w = Layered { ground: -40, ceiling: Some(30), cells: BTreeMap::new() };
    place(&mut w, &g(5, -40, 5), 200, "deep cave at y=-40");
}
