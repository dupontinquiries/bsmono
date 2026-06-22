// GTK4 UI module
// The actual GTK UI is in main.rs - this is a placeholder for future refactoring

use crate::world_state::WorldState;

#[allow(dead_code)]
pub fn run(_world: WorldState) -> Result<(), String> {
    Err("GTK UI runs from main.rs".to_string())
}
