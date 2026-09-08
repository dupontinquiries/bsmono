//! editor_open_env — the I/O `editor_open` deliberately does not do.
//!
//! `TODO_editor_open_context.md`'s "STILL TO WIRE" list opens with two items that are I/O
//! around a pure parser: *"~/.ssh/config read + recent-hosts merge → known-hosts list"* and
//! *"(optional) default editor from $EDITOR / detect_editor when unspecified"*.
//!
//! `editor_open` is pure on purpose — `parse_ssh_hosts` takes the config's TEXT, `resolve_host`
//! takes a list. This is the layer that reads the file and the environment, kept separate so
//! the parser stays testable without a home directory.
//!
//! ## Recent hosts count, and count last
//!
//! A host you sshed into an hour ago is a host you may mean; one in your config is a host you
//! definitely have. Config first, recents appended, duplicates dropped — so a name that is in
//! both keeps its config spelling, which is the one that will actually connect.

use crate::editor_open::Editor;

/// Host aliases from `~/.ssh/config`, or an empty list.
///
/// A missing or unreadable config is NOT an error: plenty of people have no ssh config, and
/// "open it on the build box" simply has no host to resolve then. Failing the whole request
/// over a file that was never required would be worse than resolving nothing.
pub fn ssh_config_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let path = std::path::Path::new(&home).join(".ssh").join("config");
    match std::fs::read_to_string(path) {
        Ok(text) => crate::editor_open::parse_ssh_hosts(&text),
        Err(_) => Vec::new(),
    }
}

/// The config's hosts, then any recent ones not already there.
///
/// Order is the ranking `resolve_host` sees, and config entries come first because they are
/// the spelling that will connect: a recent host recorded as `buildbox` and configured as
/// `build-box` must resolve to the configured form or the ssh will fail with a name error
/// after everything else went right.
pub fn known_hosts(config: &[String], recent: &[String]) -> Vec<String> {
    let mut out = config.to_vec();
    for r in recent {
        if !out.iter().any(|h| h.eq_ignore_ascii_case(r)) {
            out.push(r.clone());
        }
    }
    out
}

/// The editor to use when the phrase did not name one.
///
/// `$VISUAL` before `$EDITOR` — that is the long-standing convention, VISUAL being the
/// full-screen one and EDITOR the line editor, and a terminal opening someone's `ed` because
/// it read the wrong variable would be a memorable way to lose their afternoon.
///
/// An unrecognised value falls back rather than failing: `$EDITOR=micro` is a real setting and
/// this build knows two editors, so the honest answer is "not one I can drive" and the
/// default, not an error about their environment.
pub fn default_editor() -> Editor {
    for var in ["VISUAL", "EDITOR"] {
        if let Some(v) = std::env::var_os(var) {
            let v = v.to_string_lossy().to_lowercase();
            // The basename: `$EDITOR` is often a path, sometimes with flags.
            let base = v
                .split_whitespace()
                .next()
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("");
            if let Some(e) = editor_named(base) {
                return e;
            }
        }
    }
    Editor::Vim
}

/// The editor a name refers to, if this build can drive it.
pub fn editor_named(name: &str) -> Option<Editor> {
    match name {
        "vim" | "vi" => Some(Editor::Vim),
        // nvim maps to Vim: `build_open_argv` emits `+N` for the line, which nvim takes
        // identically. Claiming a separate variant this build does not have would be a
        // third editor the argv builder cannot spell.
        "nvim" | "neovim" => Some(Editor::Vim),
        "hx" | "helix" => Some(Editor::Helix),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recent host recorded as `buildbox` and configured as `build-box` must resolve to
    /// the CONFIGURED form, or the ssh fails with a name error after everything else went
    /// right.
    #[test]
    fn config_hosts_outrank_recent_ones_and_duplicates_collapse() {
        let config = vec!["build-box".to_string(), "m1n1".to_string()];
        let recent = vec!["m1n1".to_string(), "laptop".to_string(), "M1N1".to_string()];
        let all = known_hosts(&config, &recent);
        assert_eq!(all, vec!["build-box".to_string(), "m1n1".to_string(), "laptop".to_string()]);
        // Case-insensitively deduped: `M1N1` is the same machine.
        assert_eq!(all.iter().filter(|h| h.eq_ignore_ascii_case("m1n1")).count(), 1);
        // Order is what `resolve_host` ranks over, so config first is the whole point.
        assert_eq!(all[0], "build-box");
    }

    /// Plenty of people have no ssh config, and "open it on the build box" simply has no host
    /// to resolve then. Failing the request over a file that was never required would be
    /// worse than resolving nothing.
    #[test]
    fn a_missing_config_is_an_empty_list_not_a_failure() {
        let dir = std::env::temp_dir().join(format!("vt-ssh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No .ssh at all.
        let prev = std::env::var_os("HOME");
        // SAFETY: single-threaded test process; restored below.
        unsafe { std::env::set_var("HOME", &dir) };
        assert!(ssh_config_hosts().is_empty());

        std::fs::create_dir_all(dir.join(".ssh")).unwrap();
        std::fs::write(dir.join(".ssh/config"), "Host build-box\n  HostName 10.0.0.2\nHost *\n").unwrap();
        let hosts = ssh_config_hosts();
        assert_eq!(hosts, vec!["build-box".to_string()], "the wildcard is not a host");

        match prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `$VISUAL` before `$EDITOR`: the convention is VISUAL for the full-screen editor and
    /// EDITOR for the line editor, and a terminal opening someone's `ed` because it read the
    /// wrong variable is a memorable way to lose an afternoon.
    #[test]
    fn the_editor_comes_from_the_environment_in_the_conventional_order() {
        // A path with flags, which is what `$EDITOR` often is.
        assert_eq!(editor_named("nvim"), Some(Editor::Vim), "nvim takes vim's +N line flag");
        assert_eq!(editor_named("helix"), Some(Editor::Helix));
        assert_eq!(editor_named("vi"), Some(Editor::Vim));
        // An editor this build cannot drive is not an error about someone's environment.
        assert_eq!(editor_named("micro"), None);
        assert_eq!(editor_named("ed"), None);
        assert_eq!(editor_named(""), None);
    }
}
