//! tideshell-core — the touch shell as a headless engine.
//!
//! Layering (each seam isolates something that actually varies):
//!
//! ```text
//!   touch samples ──► edge::Recognizer ──► GestureEvent ─┐
//!   overlay taps  ─────────────────────────────────────── ┼─► shell::Shell ──► Vec<Action>
//!   window list   ─────────────────────────────────────── ┘        │
//!                                                     state for painting
//! ```
//!
//! * **Input** arrives as plain [`TouchEvent`]s. In production they come from
//!   thin layer-shell edge strips (the Wayland implicit touch grab keeps
//!   delivering motion after the finger leaves the strip), in the simulator
//!   from egui. The recognizer does not care which.
//! * **Output** is [`Action`]: what should happen in the world. Executing it is
//!   a transport's job — zcosmic toplevel management, cosmic-comp's EI socket,
//!   rusty-board's D-Bus, hatch-core. Nothing here links any of them, so every
//!   decision is testable with plain values.
//! * **Observability**: [`shell::Shell::trace`] is a bounded log of every
//!   recognizer transition and emitted action, drawn by the frontend's debug
//!   overlay, so "why didn't my swipe open the switcher" is answerable on the
//!   phone.

pub mod ambience;
pub mod ambient;
pub mod ambient_sound;
pub mod anim;
pub mod audio;
pub mod bar;
pub mod appinfo;
pub mod carousel;
pub mod chaingraph;
pub mod clock;
pub mod clock3d;
pub mod gloveflick;
pub mod nightclock;
pub mod orbs;
pub mod cold;
pub mod control_center;
pub mod cursor;
pub mod dimming;
pub mod edge;
pub mod hold;
pub mod paintrate;
pub mod palette;
pub mod lock;
pub mod morse;
pub mod prefs;
pub mod rotate;
pub mod season;
pub mod shell;
pub mod sidebar;
pub mod sleep;
pub mod switcher;
pub mod tilecache;
pub mod tilt;
pub mod touchfilter;
pub mod track;
pub mod trackpad;
pub mod voice;
pub mod volkeys;
pub mod world;

pub use shell::Shell;

/// Monotonic milliseconds. The caller's clock; tests pass literals.
pub type Millis = u64;

/// One touch sample in screen-logical pixels, origin top-left, y down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchEvent {
    Down { slot: i32, x: f32, y: f32, t: Millis },
    Move { slot: i32, x: f32, y: f32, t: Millis },
    Up { slot: i32, t: Millis },
    /// The compositor took the sequence away (e.g. a grab started).
    Cancel,
}

/// A toplevel as the shell needs it. Built from
/// `systemcompkit::toplevel::ToplevelInfo` by the transport layer, so this
/// crate stays free of the protocol crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCard {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub activated: bool,
    pub maximized: bool,
    /// Width the compositor laid the window out at (COSMIC `geometry`), if known.
    pub width: Option<i32>,
}

/// What the shell wants done. Transports execute these; the engine never
/// assumes an action succeeded (window state comes back via `set_windows`).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// zcosmic_toplevel_manager_v1.activate
    Activate(u64),
    /// zcosmic_toplevel_manager_v1.close
    Close(u64),
    /// zcosmic_toplevel_manager_v1.set_maximized / unset_maximized
    SetMaximized(u64, bool),
    /// Launch a desktop entry by id through hatch-core (so recents stay shared).
    LaunchApp(String),
    /// Take a screenshot, save it, and put it on the clipboard.
    Screenshot,
    /// Summon hatch's full app grid (`hatch-egui --toggle`).
    SummonLauncher,
    /// rusty-board `io.rustyboard.Keyboard1.Show` / `Hide`.
    ShowKeyboard(bool),
    /// A command for rusty-board's control socket (`$XDG_RUNTIME_DIR/rusty-board.ctl`):
    /// voice-start | voice-done | voice-cancel | voice-toggle | cold on | cold off.
    KeyboardCommand(&'static str),
    /// Map or unmap the trackpad overlay surface.
    ShowTrackpad(bool),
    /// Virtual pointer output, sent over cosmic-comp's EI socket.
    Pointer(trackpad::PointerOut),
    /// A quick-settings toggle changed. Backends (NetworkManager, BlueZ, ...)
    /// are per-kind transports.
    SetQuick(control_center::QuickKind, bool),
    SetGovernor(control_center::CpuGovernor),
    SetSlider(control_center::SliderKind, f32),
    /// Prefs changed; persist `ShellPrefs::to_kv()` through settingsd.
    SavePrefs,
    /// Hide (or restore) the compositor-drawn cursor while the fat cursor is
    /// drawn instead. cosmic-comp: its live `cursor_hide_timeout` config.
    HideSystemCursor(bool),
    /// Panel on/off without suspending (zwlr_output_power_v1.set_mode).
    DisplayPower(bool),
    /// Rotate the output (degrees: 0, 90, 180, 270).
    Rotate(u32),
    /// Audio voicing (-1 soft .. +1 cut through); see `audio`.
    SetVoicing(f32),
    /// Switch the apps' theme to light (true) for sunlight, or restore.
    AppsLightTheme(bool),
}

/// Windows that exist for plumbing and must not be treated as apps.
///
/// xwaylandvideobridge (KDE) keeps a tiny, invisible X11 window mapped so the
/// screencast portal can offer X11 apps for sharing. KWin hides it with window
/// rules; COSMIC and tideshell have none, so it turned up as a blank "app" in
/// the switcher and carousel (and, under a phone window policy, would be
/// maximized over everything). Matched by app id, case-insensitively, with
/// and without the reverse-DNS prefix.
pub fn is_helper_window(app_id: &str) -> bool {
    const HELPERS: [&str; 2] = ["org.kde.xwaylandvideobridge", "xwaylandvideobridge"];
    let id = app_id.trim().to_ascii_lowercase();
    HELPERS.iter().any(|h| id == *h)
}

#[cfg(test)]
mod helper_window_tests {
    #[test]
    fn the_video_bridge_is_a_helper_and_real_apps_are_not() {
        assert!(super::is_helper_window("org.kde.xwaylandvideobridge"));
        assert!(super::is_helper_window("XWaylandVideoBridge"));
        assert!(!super::is_helper_window("org.kde.kdenlive"));
        assert!(!super::is_helper_window("com.system76.CosmicSettings"));
    }
}
