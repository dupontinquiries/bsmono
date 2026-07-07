//! bsgames — the fleet's game centre. Pure backend here: the game **registry** (what's installed +
//! how to launch it), the **launch argv** builder (spawning is the caller's I/O), and a **friends
//! view** over the netcode profile service (presence → joinable rows). The egui GUI (behind `gui`)
//! renders these; `main` (behind `app`) is the eframe shell.
pub mod centre;
pub mod wiring;
#[cfg(feature = "gui")]
pub mod ui;

pub use centre::{friend_rows, launch_argv, registry, FriendRow, GameEntry, LaunchKind};
