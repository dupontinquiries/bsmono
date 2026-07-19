//! Keyboard core: state machine, key model, and event types.

pub mod engine;
pub mod character_draw;
// key / state / code_mode moved to crates/board-layouts (gtk-free, shared with the egui
// frontend); the module re-exports keep every `crate::keyboard::key::…` (etc.) path working.
pub use board_layouts::code_mode;
pub use board_layouts::key;
pub use board_layouts::state;

pub use engine::KeyboardEngine;
pub use key::{ButtonBehavior, Key, KeyAction, KeyGestures, KeyType};
pub use state::{KeyboardState, KeypadState, ModifierState};
