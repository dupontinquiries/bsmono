// gitmind-core/src/launch.rs
//
// One binary, right frontend. On a phone (postmarketOS, a touchscreen, an
// on-screen keyboard, launched from an app grid with no terminal) you want
// the GUI; in a terminal or a pipe you want the CLI. This module decides,
// from environment signals, with explicit flags always winning.
//
// The decision logic is pure (testable with a constructed Env); Env::detect
// fills it from the real system.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frontend {
    Cli,
    Gui,
    Tui,
}

#[derive(Clone, Debug, Default)]
pub struct Env {
    /// stdout is a real terminal (false when piped or launched from a GUI).
    pub stdout_is_tty: bool,
    /// $DISPLAY or $WAYLAND_DISPLAY is set.
    pub has_display: bool,
    /// Running over SSH (force CLI even if a display leaks through).
    pub is_ssh: bool,
    /// A touchscreen input device is present.
    pub touch_device: bool,
    /// An on-screen keyboard (squeekboard/maliit) is around → mobile shell.
    pub on_screen_keyboard: bool,
    /// The OS looks like a mobile Linux (postmarketOS).
    pub mobile_os: bool,
    /// Explicit override from --gui/--cli/--tui (or GITMIND_FRONTEND).
    pub forced: Option<Frontend>,
}

/// Decide the frontend. Order of authority: explicit flag > SSH (always
/// CLI) > clear mobile/touch GUI context > terminal CLI > fallback CLI.
pub fn decide(env: &Env) -> Frontend {
    if let Some(f) = env.forced {
        return f;
    }
    // A remote shell is a terminal session no matter what display exists.
    if env.is_ssh {
        return Frontend::Cli;
    }
    // Launched from a graphical/app context with no controlling terminal,
    // or clearly on a mobile/touch device with a display → GUI.
    let mobile_context = env.mobile_os || env.touch_device || env.on_screen_keyboard;
    if env.has_display && (!env.stdout_is_tty || mobile_context) {
        return Frontend::Gui;
    }
    // Anything else with a terminal is the CLI; pipes/scripts too.
    Frontend::Cli
}

impl Env {
    pub fn detect(forced: Option<Frontend>) -> Self {
        use std::io::IsTerminal;
        Env {
            stdout_is_tty: std::io::stdout().is_terminal(),
            has_display: nonempty("WAYLAND_DISPLAY") || nonempty("DISPLAY"),
            is_ssh: nonempty("SSH_TTY") || nonempty("SSH_CONNECTION"),
            touch_device: detect_touch(),
            on_screen_keyboard: nonempty("MALIIT_SERVER") || nonempty("SQUEEKBOARD")
                || process_exists("squeekboard") || process_exists("maliit-server"),
            mobile_os: detect_mobile_os(),
            forced: forced.or_else(env_forced),
        }
    }
}

fn nonempty(key: &str) -> bool {
    std::env::var(key).map(|v| !v.is_empty()).unwrap_or(false)
}

fn env_forced() -> Option<Frontend> {
    match std::env::var("GITMIND_FRONTEND").ok()?.to_ascii_lowercase().as_str() {
        "gui" => Some(Frontend::Gui),
        "cli" => Some(Frontend::Cli),
        "tui" => Some(Frontend::Tui),
        _ => None,
    }
}

fn detect_mobile_os() -> bool {
    std::fs::read_to_string("/etc/os-release")
        .map(|s| {
            let l = s.to_ascii_lowercase();
            l.contains("postmarketos") || l.contains("mobian") || l.contains("droidian")
        })
        .unwrap_or(false)
}

fn detect_touch() -> bool {
    // The kernel lists input devices here; a touchscreen shows up by name.
    std::fs::read_to_string("/proc/bus/input/devices")
        .map(|s| {
            let l = s.to_ascii_lowercase();
            l.contains("touchscreen") || l.contains("touch screen")
        })
        .unwrap_or(false)
}

fn process_exists(name: &str) -> bool {
    // Cheap, no deps: scan /proc/*/comm. Best-effort.
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let comm = e.path().join("comm");
            if let Ok(c) = std::fs::read_to_string(&comm) {
                if c.trim() == name {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Env {
        Env { stdout_is_tty: true, ..Default::default() }
    }

    #[test]
    fn terminal_is_cli() {
        assert_eq!(decide(&base()), Frontend::Cli);
    }

    #[test]
    fn launched_from_gui_no_tty_with_display() {
        let mut e = base();
        e.stdout_is_tty = false;
        e.has_display = true;
        assert_eq!(decide(&e), Frontend::Gui);
    }

    #[test]
    fn phone_with_terminal_open_still_prefers_gui() {
        let mut e = base(); // tty true (a terminal app on the phone)
        e.has_display = true;
        e.mobile_os = true;
        e.touch_device = true;
        assert_eq!(decide(&e), Frontend::Gui);
    }

    #[test]
    fn ssh_always_cli_even_with_display() {
        let mut e = base();
        e.has_display = true;
        e.is_ssh = true;
        e.mobile_os = true;
        assert_eq!(decide(&e), Frontend::Cli);
    }

    #[test]
    fn explicit_flag_wins() {
        let mut e = base();
        e.forced = Some(Frontend::Tui);
        assert_eq!(decide(&e), Frontend::Tui);
    }

    #[test]
    fn pipe_with_no_display_is_cli() {
        let mut e = base();
        e.stdout_is_tty = false; // piped
        e.has_display = false;
        assert_eq!(decide(&e), Frontend::Cli);
    }
}
