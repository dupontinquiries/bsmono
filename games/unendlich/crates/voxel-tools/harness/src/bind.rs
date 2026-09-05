//! voxel_bind.rs, verbatim in shape.
use crate::stubs::{BlockId, ChunkManager};
use voxel_tools::staged::StagedWorld;
use voxel_tools::VoxelWorld;

pub struct WorldView<'a>(pub &'a ChunkManager);
impl<'a> VoxelWorld for WorldView<'a> {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { self.0.block_at(x, y, z) as u16 }
    fn set(&mut self, _x: i32, _y: i32, _z: i32, _b: u16) -> bool { false }
    fn y_bounds(&self) -> (i32, i32) { (-64, 320) }
}
pub fn block_from_u16(v: u16) -> BlockId {
    u8::try_from(v).ok().and_then(|b| BlockId::try_from(b).ok()).unwrap_or(BlockId::Air)
}
pub fn flush(cm: &ChunkManager, staged: &mut StagedWorld<'_, WorldView<'_>>, chunk_size: i32)
    -> (usize, usize, usize) {
    let groups = staged.drain_by_chunk(chunk_size);
    let chunks = groups.len();
    let mut edits: Vec<(i32,i32,i32,BlockId)> = Vec::new();
    for (_c, blocks) in groups {
        edits.extend(blocks.into_iter().map(|(x,y,z,b)| (x,y,z,block_from_u16(b))));
    }
    let (a, s) = cm.set_blocks(&edits);
    (a, s, chunks)
}


/// The world as a drone sees it (harness copy of voxel_bind::DroneWorld).
pub struct DroneWorld(pub crate::stubs::DroneView);
impl voxel_tools::VoxelWorld for DroneWorld {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { self.0.block_at(x, y, z) as u8 as u16 }
    fn set(&mut self, _x: i32, _y: i32, _z: i32, _b: u16) -> bool { false }
    fn y_bounds(&self) -> (i32, i32) { (-4096, 8192) }
}
