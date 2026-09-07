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

/// Shell script run ON THE TARGET to install a binary that was pushed to /tmp: move it
/// into the target user's ~/.local/bin, mark it executable, and write a .desktop
/// launcher. `$HOME` expands on the target (unquoted heredoc). If `has_icon`, the icon
/// was pushed to /tmp too and is moved into the per-user cache. Pure — unit-tested.
pub fn push_install_script(app: &str, has_icon: bool) -> String {
    // THE ICON GOES IN THE ICON THEME, not the cache.
    //
    // It used to land in `~/.cache/gitmind/shots/` with an absolute `Icon=` path. That
    // works until something clears the cache — which is what a cache is for — and then
    // every pushed app loses its icon with no error anywhere. `hicolor` is the fallback
    // theme every desktop reads (Plasma, Plasma Mobile, COSMIC, GNOME/GTK), and an icon
    // NAME rather than a path is what lets each of them pick the right size and apply its
    // own theming.
    let (icon_mv, icon_line) = if has_icon {
        (
            format!(
                "mkdir -p \"$HOME/.local/share/icons/hicolor/256x256/apps\"\n\
                 mv /tmp/{app}.icon \"$HOME/.local/share/icons/hicolor/256x256/apps/{app}.png\"\n"
            ),
            format!("Icon={app}\n"),
        )
    } else {
        (String::new(), String::new())
    };
    format!(
        "set -e\n\
         mkdir -p \"$HOME/.local/bin\" \"$HOME/.local/share/applications\"\n\
         mv /tmp/{app} \"$HOME/.local/bin/{app}\"\n\
         chmod +x \"$HOME/.local/bin/{app}\"\n\
         {icon_mv}\
         cat > \"$HOME/.local/share/applications/{app}.desktop\" <<DESK\n\
         [Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={app}\n\
         Comment=Pushed by gitmind fleet\n\
         Exec=$HOME/.local/bin/{app}\n\
         {icon_line}\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupNotify=true\n\
         StartupWMClass={app}\n\
         DESK\n\
         {refresh}\
         echo gitmind-pushed",
        refresh = refresh_caches(),
    )
}

/// Tell the desktop a new launcher exists.
///
/// Writing the .desktop file is not enough on every desktop this fleet targets:
///
/// * **Plasma and Plasma Mobile** read a compiled cache (`kbuildsycoca`). Until it is
///   rebuilt the app is installed and absent from the launcher, which reads as the push
///   having silently failed — and is the single most likely thing to be reported as "it
///   didn't install".
/// * **GNOME/GTK and COSMIC** rescan the directory, but `update-desktop-database` is what
///   refreshes the MIME/desktop index that `gio launch` and file associations use.
///
/// Every command is best-effort: `command -v` first, output discarded, `|| true` after. The
/// script runs under `set -e`, and a missing `kbuildsycoca6` on a GNOME box must not fail an
/// install that has already succeeded. Both kbuildsycoca names are tried because Plasma 5
/// and 6 differ and a phone may be on either.
fn refresh_caches() -> String {
    let cmds = [
        "kbuildsycoca6 --noincremental",
        "kbuildsycoca5 --noincremental",
        "update-desktop-database \"$HOME/.local/share/applications\"",
        "gtk-update-icon-cache -f -t \"$HOME/.local/share/icons/hicolor\"",
    ];
    let mut s = String::new();
    for c in cmds {
        let bin = c.split_whitespace().next().unwrap_or("");
        s.push_str(&format!(
            "command -v {bin} >/dev/null 2>&1 && {c} >/dev/null 2>&1 || true\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {

    /// THE SCRIPT IS RUN, in a throwaway HOME, and the result inspected. Reading the text
    /// only proves it says the right words; running it proves the heredoc closes, the paths
    /// exist, and `set -e` does not trip over a desktop tool this machine lacks — which is
    /// the case every non-Plasma target hits.
    #[test]
    fn the_script_installs_a_working_launcher_in_a_scratch_home() {
        let root = std::env::temp_dir().join(format!("gm-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The pushed binary and icon, as `deploy` would have left them.
        let tmp = root.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("peek"), b"#!/bin/sh\ntrue\n").unwrap();
        std::fs::write(tmp.join("peek.icon"), b"\x89PNG\r\n").unwrap();

        // /tmp is baked into the script, so run it with a HOME override and a TMPDIR shim:
        // rewrite the two /tmp paths to the scratch ones rather than touching the real /tmp.
        // Through a token, in that order: the scratch dir is itself under /tmp, so a plain
        // second replace of "/tmp/peek" rewrites the TAIL of the path the first pass just
        // produced. It did, and the script failed with a doubled path.
        let script = push_install_script("peek", true)
            .replace("/tmp/peek.icon", "@@ICON@@")
            .replace("/tmp/peek", tmp.join("peek").to_str().unwrap())
            .replace("@@ICON@@", tmp.join("peek.icon").to_str().unwrap());

        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", &root)
            .output()
            .expect("sh");
        assert!(
            out.status.success(),
            "the install script failed: {}\n--- stderr ---\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("gitmind-pushed"),
            "no completion marker: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );

        // The binary landed, executable.
        let bin = root.join(".local/bin/peek");
        assert!(bin.is_file(), "no binary at {bin:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&bin).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "not executable: {mode:o}");
        }

        // The icon is in the ICON THEME, not a cache that something will clear.
        let icon = root.join(".local/share/icons/hicolor/256x256/apps/peek.png");
        assert!(icon.is_file(), "no themed icon at {icon:?}");

        // The launcher parses, and carries what each desktop needs.
        let desk = std::fs::read_to_string(root.join(".local/share/applications/peek.desktop")).unwrap();
        assert!(desk.starts_with("[Desktop Entry]"), "{desk}");
        for want in ["Type=Application", "Terminal=false", "StartupWMClass=peek", "Icon=peek"] {
            assert!(desk.contains(want), "missing {want} in:\n{desk}");
        }
        // `Exec` is an ABSOLUTE path with $HOME already expanded — a launcher does not run a
        // shell, so an unexpanded $HOME would be a literal directory name.
        let exec = desk.lines().find(|l| l.starts_with("Exec=")).expect("an Exec line");
        assert!(!exec.contains('$'), "unexpanded variable in {exec}");
        assert!(exec.contains(root.to_str().unwrap()), "{exec}");
        // No leading whitespace on any line: the format! continuations are the easiest thing
        // to get wrong here, and a leading space makes the whole entry unparseable.
        for l in desk.lines() {
            assert_eq!(l, l.trim_start(), "indented line in the desktop entry: {l:?}");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The refresh must not fail an install on a machine without the tool. This container
    /// has none of them, which is exactly the GNOME/COSMIC case.
    #[test]
    fn a_missing_desktop_tool_does_not_fail_the_install() {
        let s = refresh_caches();
        for bin in ["kbuildsycoca6", "kbuildsycoca5", "update-desktop-database", "gtk-update-icon-cache"] {
            assert!(s.contains(bin), "{bin} is not attempted");
        }
        // Guarded and swallowed: `command -v` first, `|| true` after, under `set -e`.
        for line in s.lines().filter(|l| !l.trim().is_empty()) {
            assert!(line.starts_with("command -v "), "unguarded: {line}");
            assert!(line.ends_with("|| true"), "not swallowed: {line}");
        }
        // And it really runs clean under `set -e` here, where none of them exist.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("set -e\n{s}echo ok"))
            .output()
            .expect("sh");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    use super::*;

    #[test]
    fn paths_under_local() {
        assert_eq!(bin_dir("/home/user/"), "/home/user/.local/bin");
        assert_eq!(desktop_dir("/home/user"), "/home/user/.local/share/applications");
    }

    #[test]
    fn push_install_script_installs_and_launches() {
        let s = push_install_script("peek", false);
        assert!(s.contains("mv /tmp/peek \"$HOME/.local/bin/peek\""), "moves into ~/.local/bin");
        assert!(s.contains("chmod +x"), "executable");
        assert!(s.contains("[Desktop Entry]") && s.contains("Exec=$HOME/.local/bin/peek"), "launcher");
        assert!(!s.contains("Icon="), "no icon when none pushed");
        assert!(s.contains("echo gitmind-pushed"), "completion marker");
        // with icon → installed into the icon theme and referenced BY NAME
        let si = push_install_script("peek", true);
        assert!(
            si.contains("mv /tmp/peek.icon")
                && si.contains("hicolor/256x256/apps/peek.png")
                && si.contains("Icon=peek\n"),
            // The icon moved OUT of `~/.cache`: a cache is a thing that gets cleared, and
            // when it was, every pushed app lost its icon with no error anywhere. An icon
            // NAME in the hicolor theme is what every target desktop resolves and themes.
            "the icon should be installed into the icon theme by name:\n{si}"
        );
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
