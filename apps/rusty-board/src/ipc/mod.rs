//! IPC: D-Bus service for external integration.
//!
//! Exposes `io.rustyboard.Keyboard` on the user session bus, with methods
//! for showing/hiding the keyboard, starting voice input, etc.

#[cfg(feature = "dbus")]
pub mod dbus;

#[cfg(not(feature = "dbus"))]
pub mod dbus {
    use anyhow::Result;
    pub async fn run_service() -> Result<()> {
        Ok(())
    }
}
