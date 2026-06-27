//! GTK4 + libadwaita UI for the keyboard.

pub mod animations;
pub mod key_widget;
pub mod keyboard_view;
pub mod microterm;
pub mod minimized_tray;
pub mod settings_panel;
pub mod swipe_trail;
pub mod tap_hold;
pub mod theme;
pub mod window;

pub use animations::Animator;
pub use keyboard_view::KeyboardView;
pub use microterm::{MicrotermContent, MicrotermView};
pub use minimized_tray::{DominantSide, MinimizedTray};
pub use settings_panel::SettingsPanel;
pub use swipe_trail::SwipeTrail;
pub use tap_hold::{TapHoldButton, TapHoldConfig};
