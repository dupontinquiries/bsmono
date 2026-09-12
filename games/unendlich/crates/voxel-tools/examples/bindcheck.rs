//! Type-checks voxel_bind.rs against stubs whose signatures were read
//! from the real engine source, so the binding is verified as far as
//! it can be without the engine's dependency tree.
use std::collections::HashMap;
use std::sync::Mutex;

// --- stubs mirroring the real signatures ---
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum BlockId { Air = 0, Stone = 1, Cobblestone = 4, StoneBricks = 58 }
impl TryFrom<u8> for BlockId {
    type Error = u8;
    fn try_from(v: u8) -> Result<Self, u8> {
        match v { 0 => Ok(BlockId::Air), 1 => Ok(BlockId::Stone),
                  4 => Ok(BlockId::Cobblestone), 58 => Ok(BlockId::StoneBricks), o => Err(o) }
    }
}
/// `set_block`/`set_blocks` take &self: interior mutability via DashMap.
pub struct ChunkManager { cells: Mutex<HashMap<(i32,i32,i32), BlockId>> }
impl ChunkManager {
    fn new() -> Self { Self { cells: Mutex::new(HashMap::new()) } }
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.cells.lock().unwrap().get(&(x,y,z)).copied()
            .unwrap_or(if y <= 64 { BlockId::Stone } else { BlockId::Air })
    }
    pub fn set_blocks(&self, edits: &[(i32,i32,i32,BlockId)]) -> (usize, usize) {
        let mut c = self.cells.lock().unwrap();
        for &(x,y,z,b) in edits { c.insert((x,y,z), b); }
        (edits.len(), 0)
    }
}

// --- the binding, verbatim in shape from voxel_bind.rs ---
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

fn main() {
    use voxel_tools::chat::{ChatSession, Reply};
    use voxel_tools::focus::{Gaze, GazeHit};
    use voxel_tools::VoxelTools;

    let cm = ChunkManager::new();
    let gaze = Gaze { eye: (0.0, 70.0, -10.0), dir: (0.0, -0.3, 1.0),
        hit: Some(GazeHit { block: (0, 64, 0), face_normal: (0, 1, 0), distance: 12.0 }) };
    let mut session = ChatSession::new(32, 200);

    // read through the live world, stage every write
    let view = WorldView(&cm);
    let mut staged = StagedWorld::new(&view);
    let ir = "terraform @-4,-4 @4,4 smooth block=stone\ncylinder @0,0 r=3 h=10 block=stone_bricks hollow\npath @3,0 @14,8 w=2 block=cobblestone\n";
    {
        let mut tools = VoxelTools::new(&mut staged);
        match session.receive_plan_for(&mut tools, &gaze, ir, Some((0, 65, -10))) {
            Reply::Preview { blocks, .. } => println!("preview: {blocks} blocks"),
            r => println!("unexpected: {r:?}"),
        }
        match session.commit(&mut tools, &gaze) {
            Reply::Committed { blocks, batches, .. } => println!("commit: {blocks} blocks, {batches} batches"),
            r => println!("unexpected: {r:?}"),
        }
    }
    println!("staged pending: {}", staged.pending());
    println!("chunks that will be cloned: {}", staged.chunks_touched(32));
    let (applied, skipped, chunks) = flush(&cm, &mut staged, 32);
    println!("flushed: applied={applied} skipped={skipped} clone_for_edit_calls={chunks}");
    println!("world now: (0,65,0)={:?}  (3,66,0)={:?}",
             cm.block_at(0, 65, 0), cm.block_at(3, 66, 0));
    let frames = session.reveal(25);
    println!("reveal: {} frames", frames.len());
}
