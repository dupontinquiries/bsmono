//! Player commands. A replay is exactly: seed + data hash + this stream.

use echelon_data::DefId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Cmd {
    /// move to milli-tile point
    Move { ids: Vec<u32>, mx: i32, my: i32 },
    /// attack-move to milli-tile point
    Attack { ids: Vec<u32>, mx: i32, my: i32 },
    Stop { ids: Vec<u32> },
    /// send worker to gather from resource node index
    Harvest { id: u32, node: u32 },
    /// enqueue a unit at a production structure
    Train { structure: u32, unit: DefId },
    /// worker erects a structure with its top-left at tile (tx,ty)
    Build { worker: u32, unit: DefId, tx: i32, ty: i32 },
    /// Genetron: link an Uploader to a production facility
    Link { uploader: u32, facility: u32 },
    /// hold position: fire at whatever comes in range, never chase
    Hold { ids: Vec<u32> },
    /// set a production structure's rally point
    Rally { structure: u32, mx: i32, my: i32 },
    /// fire an Active ability: slot indexes the unit's abil list;
    /// target is entity index+0 (0 = none) with mx/my as the point
    Cast { caster: u32, slot: u8, target: u32, mx: i32, my: i32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerCmd {
    pub player: u8,
    pub cmd: Cmd,
}
