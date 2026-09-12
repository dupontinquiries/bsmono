// Every staged edit must be filed under the chunk the engine would
// compute for it: ChunkCoord::from_block_pos = div_euclid(32).
use std::collections::BTreeMap;
use voxel_tools::{VoxelTools, VoxelWorld};
use voxel_tools::plan::{parse_plan_at, run_plan};
use voxel_tools::staged::StagedWorld;

struct Flat(BTreeMap<(i32,i32,i32), u16>);
impl VoxelWorld for Flat {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
}
fn engine_chunk(x: i32, y: i32, z: i32) -> (i32,i32,i32) {
    (x.div_euclid(32), y.div_euclid(32), z.div_euclid(32))
}
fn main() {
    for anchor in [(0,65,0), (-1,65,-1), (-33,65,-33), (31,65,31), (32,65,32), (5,-39,5)] {
        let base = Flat(BTreeMap::new());
        let mut staged = StagedWorld::new(&base);
        let (plan, _) = parse_plan_at(
            "cylinder @0,0 r=4 h=40 block=stone\npath @-6,-6 @6,6 w=3 block=cobble\n",
            Some(anchor));
        let n = { let mut t = VoxelTools::new(&mut staged);
                  run_plan(&mut t, &plan, 200, 32, false).blocks_changed };
        let groups = staged.drain_by_chunk(32);
        let mut bad = 0;
        for (c, blocks) in &groups {
            for &(x,y,z,_) in blocks { if engine_chunk(x,y,z) != *c { bad += 1; } }
        }
        println!("anchor {anchor:?}: {n} blocks -> {} chunks, misfiled {bad}", groups.len());
        assert_eq!(bad, 0);
    }
    println!("every edit filed under the chunk the engine would compute");
}
