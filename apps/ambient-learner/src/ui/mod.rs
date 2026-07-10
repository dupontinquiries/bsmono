// src/ui/mod.rs
// UI abstraction layer - supports multiple UI frameworks

use crate::world_state::WorldState;

#[cfg(feature = "gtk")]
pub mod gtk4;

#[cfg(feature = "iced")]
pub mod iced;

#[cfg(feature = "egui")]
pub mod egui;

/// UI framework selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UIFramework {
    #[cfg(feature = "gtk")]
    Gtk,
    #[cfg(feature = "iced")]
    Iced,
}

impl UIFramework {
    pub fn as_str(&self) -> &'static str {
        match self {
            #[cfg(feature = "gtk")]
            UIFramework::Gtk => "gtk",
            #[cfg(feature = "iced")]
            UIFramework::Iced => "iced",
        }
    }
    
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            #[cfg(feature = "gtk")]
            "gtk" | "gtk4" => Some(UIFramework::Gtk),
            #[cfg(feature = "iced")]
            "iced" => Some(UIFramework::Iced),
            _ => None,
        }
    }
}

/// Run the selected UI.
///
/// **Currently unused.** The real dispatch lives in `main.rs::main`,
/// which calls into per-framework entry points directly. This
/// function exists as a placeholder for a future refactor that
/// moves UI dispatch behind a common interface. Until then, both
/// arms below return an error rather than trying to call a real
/// entry point with the wrong signature.
#[allow(unused_variables, dead_code)]
pub fn run_ui(framework: UIFramework, world: WorldState) -> Result<(), String> {
    match framework {
        #[cfg(feature = "gtk")]
        UIFramework::Gtk => {
            Err("GTK UI must be run from main.rs::run_gtk_ui for now".to_string())
        }
        #[cfg(feature = "iced")]
        UIFramework::Iced => {
            // The Iced entry point needs the CalibrationQueue too;
            // this dispatcher doesn't have it. Use main.rs::run_iced_ui.
            Err("Iced UI must be run from main.rs::run_iced_ui for now".to_string())
        }
    }
}
