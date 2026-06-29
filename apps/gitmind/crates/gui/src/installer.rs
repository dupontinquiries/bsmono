//! Turning a fetched binary into an installed app on this device: where it lands and
//! the `.desktop` launcher entry so it shows up in Plasma (Mobile). The cp + file
//! writes run on-device; the paths and the entry text are pure and unit-tested.

/// `~/.local/bin` — where the active user's binaries go (on PATH, no root).
pub fn bin_dir(home: &str) -> String {
    format!("{}/.local/bin", home.trim_end_matches('/'))
}

/// `~/.local/share/applications` — where per-user `.desktop` entries live.
pub fn desktop_dir(home: &str) -> String {
    format!("{}/.local/share/applications", home.trim_end_matches('/'))
}

/// Where an installed binary goes: native binaries to ~/.local/bin (on PATH, runnable);
/// non-native (cross) binaries to a per-target dir under ~/.local/lib/gitmind, since
/// they can't run on this device directly (they're for another device or an emulator).
pub fn install_dir(home: &str, triple: &str, native: bool) -> String {
    if native {
        bin_dir(home)
    } else {
        format!("{}/.local/lib/gitmind/{}", home.trim_end_matches('/'), triple)
    }
}

/// The `.desktop` launcher entry. `icon` may be an absolute image path (e.g. the
/// cached screenshot) or empty to omit it. Pure — unit-tested.
pub fn desktop_entry(name: &str, exec: &str, icon: &str) -> String {
    let mut s = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment=Installed by gitmind fleet\n\
         Exec={exec}\n\
         Terminal=false\n\
         Categories=Utility;\n"
    );
    if !icon.trim().is_empty() {
        s.push_str(&format!("Icon={}\n", icon.trim()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_under_local() {
        assert_eq!(bin_dir("/home/user/"), "/home/user/.local/bin");
        assert_eq!(desktop_dir("/home/user"), "/home/user/.local/share/applications");
    }

    #[test]
    fn install_dir_splits_native_and_emulator() {
        assert_eq!(install_dir("/home/u", "aarch64-unknown-linux-musl", true), "/home/u/.local/bin");
        assert_eq!(install_dir("/home/u", "aarch64-unknown-linux-gnu", false), "/home/u/.local/lib/gitmind/aarch64-unknown-linux-gnu");
    }

    #[test]
    fn desktop_entry_is_well_formed() {
        let e = desktop_entry("voiceterm", "/home/u/.local/bin/voiceterm", "/c/shots/voiceterm.png");
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("Type=Application"));
        assert!(e.contains("Name=voiceterm"));
        assert!(e.contains("Exec=/home/u/.local/bin/voiceterm"));
        assert!(e.contains("Icon=/c/shots/voiceterm.png"));
        // no icon → no Icon line
        assert!(!desktop_entry("x", "/b/x", "").contains("Icon="));
    }
}
