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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusAdjust { Bigger, Smaller }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnimProfile { GroundUp, Physics, Random, Instant }
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    Build(String), BuildVoice(bool), ShowPlan, CommitPlan, UndoBuild,
    Focus(Option<FocusAdjust>), Anim(AnimProfile), Model(Option<String>), Keys, Cancel, Repair, Hive, Refine(Option<String>), Deferred { drop: bool }, Provider(Option<String>), Shot { kind: String, id: Option<String> }, Unknown,
}
pub fn parse(line: &str) -> Command {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("/build ") {
        return match rest.trim().to_ascii_lowercase().as_str() {
            "enablevoice" => Command::BuildVoice(true),
            "disablevoice" => Command::BuildVoice(false),
            _ => Command::Build(rest.trim().to_string()),
        };
    }
    let mut it = t.split_whitespace();
    match (it.next(), it.next()) {
        (Some("/plan"), None) => Command::ShowPlan,
        (Some("/commit"), None) => Command::CommitPlan,
        (Some("/undo"), None) => Command::UndoBuild,
        (Some("/focus"), None) => Command::Focus(None),
        (Some("/focus"), Some("bigger")) => Command::Focus(Some(FocusAdjust::Bigger)),
        (Some("/focus"), Some("smaller")) => Command::Focus(Some(FocusAdjust::Smaller)),
        (Some("/anim"), Some("physics")) => Command::Anim(AnimProfile::Physics),
        (Some("/anim"), Some("instant")) => Command::Anim(AnimProfile::Instant),
        (Some("/anim"), Some(_)) => Command::Anim(AnimProfile::GroundUp),
        (Some("/model"), None) => Command::Model(None),
        (Some("/model"), Some(n)) => Command::Model(Some(n.to_string())),
        (Some("/keys"), None) => Command::Keys,
        (Some("/hive"), None) => Command::Hive,
        (Some("/refine"), None) => Command::Refine(None),
        (Some("/refine"), Some(r)) => Command::Refine(Some(r.to_string())),
        (Some("/shot"), Some(k)) => Command::Shot { kind: k.to_string(), id: None },
        (Some("/cancel"), None) => Command::Cancel,
        (Some("/provider"), None) => Command::Provider(None),
        (Some("/provider"), Some(p)) => Command::Provider(Some(p.to_string())),
        _ => Command::Unknown,
    }
}
