//! multiplayer — soldier's online-play control plane. This file held the ORIGINAL implementation;
//! it moved to `netcode::multiplayer` (generalized by one parameter, the game id, tests included)
//! once every fleet game needed the same host/announce/join shape — see that module's docs. The
//! wrapper below pins soldier's two-argument constructor so call sites read as before and the
//! game id can't drift from the registry's.

pub use netcode::multiplayer::Multiplayer as SharedMultiplayer;

pub struct Multiplayer;

impl Multiplayer {
    /// Soldier's constructor: the shared control plane, pinned to game id `"soldier"` (the same
    /// id `bsgames::centre::registry` launches and `netcode::session` records carry).
    pub fn new(player_id: &str, display_name: &str) -> SharedMultiplayer {
        SharedMultiplayer::new("soldier", player_id, display_name)
    }

    /// Test/table variant with an explicit store path — see the shared module's doc.
    pub fn at_store(player_id: &str, display_name: &str, store_path: std::path::PathBuf) -> SharedMultiplayer {
        SharedMultiplayer::at_store("soldier", player_id, display_name, store_path)
    }
}
