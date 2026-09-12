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
    Focus(Option<FocusAdjust>), Anim(AnimProfile), Model(Option<String>), Keys, Cancel, Repair, Land(LandOp), Hive, Refine(Option<String>), Deferred { drop: bool }, Provider(Option<String>), Shot { kind: String, id: Option<String> }, Branch(Option<String>), Switch(String), Merge(String), Like(String), Review, Rounds(Option<usize>), Takes(usize, String), Pick(usize), Unknown,
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
    // /takes carries a REQUEST, which has spaces. The match below is
    // over two whitespace-split tokens, so putting it there gave the
    // count and dropped the request -- "building 3 takes of \"\"".
    if let Some(rest) = t.strip_prefix("/takes ") {
        let rest = rest.trim();
        let mut w = rest.split_whitespace();
        return match w.next().and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => Command::Takes(n.clamp(2, 4), w.collect::<Vec<_>>().join(" ")),
            None => Command::Takes(3, rest.to_string()),
        };
    }
    if let Some(rest) = t.strip_prefix("/pick ") {
        return match rest.trim().parse::<usize>() {
            Ok(v) if (1..=4).contains(&v) => Command::Pick(v),
            _ => Command::Unknown,
        };
    }
    // /land takes the whole remainder: a claim name may have spaces.
    if let Some(rest) = t.strip_prefix("/land") {
        let rest = rest.trim();
        if rest.is_empty() { return Command::Land(LandOp::Here); }
        let mut it = rest.split_whitespace();
        return match (it.next(), it.next(), it.next()) {
            (Some("info"), Some(id), _) => Command::Land(LandOp::Info(id.into())),
            (Some("add"), Some(id), Some(p)) => Command::Land(LandOp::Add { id: id.into(), player: p.into() }),
            (Some("claim"), Some(_), _) => Command::Land(LandOp::Claim(rest[5..].trim().into())),
            _ => Command::Unknown,
        };
    }
    for (pfx, mk) in [("/switch ", 0u8), ("/merge ", 1), ("/like ", 2), ("/branch ", 3)] {
        if let Some(rest) = t.strip_prefix(pfx) {
            let r = rest.trim().to_string();
            return match mk { 0 => Command::Switch(r), 1 => Command::Merge(r), 2 => Command::Like(r), _ => Command::Branch(Some(r)) };
        }
    }
    if t == "/branch" { return Command::Branch(None); }
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
        (Some("/review"), None) => Command::Review,
        (Some("/repair"), None) => Command::Repair,
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LandOp { Here, Info(String), Add { id: String, player: String }, Claim(String) }
