//! Stubs mirroring the real engine signatures, read from source.
use std::collections::HashMap;
use std::sync::Mutex;
// --- stubs mirroring the real signatures ---
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum BlockId { Air = 0, Stone = 1, Cobblestone = 4, Water = 9, StoneBricks = 58 }
impl TryFrom<u8> for BlockId {
    type Error = u8;
    fn try_from(v: u8) -> Result<Self, u8> {
        match v { 0 => Ok(BlockId::Air), 1 => Ok(BlockId::Stone),
                  4 => Ok(BlockId::Cobblestone), 58 => Ok(BlockId::StoneBricks),
                  9 => Ok(BlockId::Water), _ => Ok(BlockId::Stone) }
    }
}
/// `set_block`/`set_blocks` take &self: interior mutability via DashMap.
pub struct ChunkManager { cells: std::sync::Arc<Mutex<HashMap<(i32,i32,i32), BlockId>>> }
#[derive(Clone)]
pub struct DroneView(std::sync::Arc<Mutex<HashMap<(i32,i32,i32), BlockId>>>);
impl DroneView {
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.0.lock().unwrap().get(&(x,y,z)).copied()
            .unwrap_or(if y <= 64 { BlockId::Stone } else { BlockId::Air })
    }
}
impl ChunkManager {
    pub fn new() -> Self { Self { cells: std::sync::Arc::new(Mutex::new(HashMap::new())) } }
    pub fn drone_view(&self) -> DroneView { DroneView(std::sync::Arc::clone(&self.cells)) }
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.cells.lock().unwrap().get(&(x,y,z)).copied()
            .unwrap_or(if y <= 64 { BlockId::Stone } else { BlockId::Air })
    }
    pub fn is_loaded_at(&self, _x: i32, _y: i32, _z: i32) -> bool { true }
    /// Cells explicitly written (not the implicit flat ground).
    pub fn block_count(&self) -> usize { self.cells.lock().unwrap().len() }
    pub fn set_blocks(&self, edits: &[(i32,i32,i32,BlockId)]) -> (usize, usize) {
        let mut c = self.cells.lock().unwrap();
        for &(x,y,z,b) in edits { c.insert((x,y,z), b); }
        // The real engine clones the chunk for edit and remeshes it: on
        // the order of a millisecond or two per chunk. A hash insert is
        // microseconds, and with that the queen drains everything in two
        // frames and the scenario proves nothing. Model the real cost.
        if !edits.is_empty() { std::thread::sleep(std::time::Duration::from_micros(1500)); }
        (edits.len(), 0)
    }
}


// Command enum mirroring sim/command_parse.rs
// THE REAL PARSER, NOT A COPY OF IT.
//
// This stubbed `Command` and its own `parse`, and adding two variants
// to the real enum compiled here and BROKE THE ENGINE BUILD -- the
// harness cannot catch a non-exhaustive match in a file it does not
// use. `sim/command_parse.rs` has zero crate dependencies, so there
// was never a reason to double it.
pub use crate::real_commands::{AnimProfile, Command, FocusAdjust, LandOp, parse};


/// Mirror of world::flora_store, enough for the paintbrush to register plants.
#[derive(Clone, Copy, Debug)]
pub struct FloraCell { pub block: u8, pub growth01: f32, pub stage: u8, pub element: u8,
                       pub vigor: f32, pub planted_tick: u64, pub genome: [f32; 13] }
impl FloraCell {
    pub fn sown(block: u8, tick: u64) -> Self {
        Self { block, growth01: 0.02, stage: 0, element: 0, vigor: 0.6, planted_tick: tick, genome: [0.5; 13] }
    }
}
#[derive(Default)]
pub struct FloraStore { pub cells: HashMap<[i32; 3], FloraCell> }
impl FloraStore {
    pub fn plant(&mut self, pos: [i32; 3], cell: FloraCell) { self.cells.insert(pos, cell); }
    pub fn len(&self) -> usize { self.cells.len() }
}

/// Stub of thread_priority::configure_current_thread(ChunkLoader).
pub fn deprioritise() {}

