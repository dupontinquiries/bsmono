//! backend_check — say why the window will not open, before the panic does.
//!
//! A windowed build that cannot reach a display server fails deep inside a dependency:
//! `wayland-sys`'s `wayland_client_handle()` tries `libwayland-client.so.0`, then
//! `libwayland-client.so`, and `.expect()`s the result. What reaches the user is a panic
//! naming a line in a crate they have never heard of, on a phone, in a terminal they cannot
//! copy out of.
//!
//! Worse, the message points the wrong way. It says the library could not be LOADED, which
//! reads as "not installed" — and the most likely cause is that it is installed and this
//! binary cannot dlopen anything at all, because it was linked statically. Someone reading
//! that message installs a package they already have.
//!
//! This runs first and says which it is.
//!
//! ## Why static linking is the likely answer
//!
//! winit does not link the display libraries; it dlopens them, so one binary can serve
//! Wayland, X11, or neither. rustc links musl targets statically by default. A static binary
//! has no dynamic loader, so every dlopen fails and both backends look absent — on a machine
//! with both installed. The cross-build for a phone is exactly where this bites, and a
//! native desktop build of the same source is dynamic and works, which makes it look like a
//! device problem rather than a linkage one.
//!
//! ## Pure, then a thin reader
//!
//! [`diagnose`] takes facts and returns a sentence. [`preflight`] gathers the facts from
//! this process. Everything that decides anything is in the first one.

/// What the environment says about a display server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session<'a> {
    /// `$WAYLAND_DISPLAY`.
    pub wayland: Option<&'a str>,
    /// `$DISPLAY`.
    pub x11: Option<&'a str>,
    /// This binary has no dynamic loader, so `dlopen` cannot succeed for anything.
    pub statically_linked: bool,
}

/// The explanation to print, or `None` when nothing looks wrong.
///
/// Deliberately returns `None` for the ordinary case rather than a reassuring line: a
/// message printed on every successful start is one nobody reads by the time it matters.
pub fn diagnose(s: Session<'_>) -> Option<String> {
    let has_session = s.wayland.is_some_and(|v| !v.is_empty()) || s.x11.is_some_and(|v| !v.is_empty());

    if s.statically_linked {
        // The order matters: a static binary fails even with a perfectly good session, and
        // saying "no compositor" there would send someone to check their session.
        return Some(format!(
            "This build is STATICALLY LINKED, so it cannot open a display backend on any \
             machine.\n\
             \x20 winit does not link libwayland-client or libX11 — it dlopens them at run \
             time — and a static binary has no dynamic loader, so every dlopen fails and \
             both backends look absent even where they are installed.\n\
             \x20 Installing a wayland package will not help. Rebuild with \
             RUSTFLAGS=\"-C target-feature=-crt-static\" (gitmind sets this for windowed \
             apps on musl targets).\n\
             \x20 Session seen: {}.",
            describe_session(s, has_session)
        ));
    }

    if !has_session {
        return Some(
            "No display server in this environment: neither WAYLAND_DISPLAY nor DISPLAY is \
             set.\n\
             \x20 If this is a TTY or an ssh session, there is nothing to open a window on. \
             If you expected a session, check that this process inherited the environment — \
             a service started outside the user session usually has not."
                .to_string(),
        );
    }

    None
}

fn describe_session(s: Session<'_>, has: bool) -> String {
    if !has {
        return "none (no WAYLAND_DISPLAY, no DISPLAY)".to_string();
    }
    let mut parts = Vec::new();
    if let Some(w) = s.wayland.filter(|v| !v.is_empty()) {
        parts.push(format!("WAYLAND_DISPLAY={w}"));
    }
    if let Some(x) = s.x11.filter(|v| !v.is_empty()) {
        parts.push(format!("DISPLAY={x}"));
    }
    parts.join(", ")
}

/// Does this process have a dynamic loader?
///
/// Read from `/proc/self/maps`: a dynamically linked process has the loader and its shared
/// objects mapped, a static one has neither. Cheaper and more direct than re-reading our own
/// ELF headers, and it reflects what actually happened rather than what the file claims.
///
/// Returns `None` when `/proc` is not readable — on which nothing should be asserted, so the
/// caller treats it as "not known to be static" rather than guessing.
#[cfg(target_os = "linux")]
pub fn statically_linked() -> Option<bool> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    Some(!maps_show_dynamic_loader(&maps))
}

#[cfg(not(target_os = "linux"))]
pub fn statically_linked() -> Option<bool> {
    None
}

/// Do these maps show a dynamic loader or any shared object?
pub fn maps_show_dynamic_loader(maps: &str) -> bool {
    maps.lines().any(|l| {
        let f = l.rsplit(' ').next().unwrap_or("");
        f.contains("/ld-") || f.contains("/ld.so") || f.ends_with(".so") || f.contains(".so.")
    })
}

/// Gather the facts and diagnose. Print the result before opening a window.
pub fn preflight() -> Option<String> {
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    let x11 = std::env::var("DISPLAY").ok();
    diagnose(Session {
        wayland: wayland.as_deref(),
        x11: x11.as_deref(),
        // Unknown counts as NOT static: a false accusation would send someone rebuilding a
        // binary that is fine.
        statically_linked: statically_linked().unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(w: Option<&'static str>, x: Option<&'static str>, st: bool) -> Session<'static> {
        Session { wayland: w, x11: x, statically_linked: st }
    }

    /// The message that was misleading people: the library is INSTALLED and this binary
    /// cannot dlopen it. Someone reading wayland-sys's own panic installs a package they
    /// already have.
    #[test]
    fn a_static_binary_is_told_it_is_static_not_that_wayland_is_missing() {
        let m = diagnose(session(Some("wayland-0"), None, true)).expect("a diagnosis");
        assert!(m.contains("STATICALLY LINKED"), "{m}");
        assert!(m.contains("crt-static"), "the fix is not named: {m}");
        assert!(m.contains("will not help"), "it should say installing the package is futile: {m}");
        // Even with a perfectly good session — which is the point. Saying "no compositor"
        // here would send someone to check a session that is fine.
        assert!(m.contains("WAYLAND_DISPLAY=wayland-0"), "{m}");
    }

    #[test]
    fn no_session_is_reported_as_no_session() {
        let m = diagnose(session(None, None, false)).expect("a diagnosis");
        assert!(m.contains("No display server"), "{m}");
        assert!(!m.contains("STATICALLY"), "a dynamic binary must not be accused: {m}");
        // Empty strings are as good as unset — a cleared variable is not a session.
        assert!(diagnose(session(Some(""), Some(""), false)).is_some());
    }

    /// A message printed on every successful start is one nobody reads by the time it
    /// matters.
    #[test]
    fn a_working_setup_says_nothing() {
        assert_eq!(diagnose(session(Some("wayland-0"), None, false)), None);
        assert_eq!(diagnose(session(None, Some(":0"), false)), None);
        assert_eq!(diagnose(session(Some("wayland-1"), Some(":0"), false)), None);
    }

    /// Read from the maps rather than our own ELF headers: it reflects what actually
    /// happened rather than what the file claims.
    #[test]
    fn the_loader_is_recognised_in_a_maps_dump() {
        let dynamic = "\
7f0e0a000000-7f0e0a028000 r--p 00000000 fe:01 1 /usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2
7f0e0a200000-7f0e0a300000 r-xp 00000000 fe:01 2 /usr/lib/x86_64-linux-gnu/libc.so.6
";
        assert!(maps_show_dynamic_loader(dynamic));
        let musl = "7f0e0a000000-7f0e0a028000 r--p 00000000 fe:01 1 /lib/ld-musl-aarch64.so.1\n";
        assert!(maps_show_dynamic_loader(musl));

        // A static binary: its own image, the heap, the stack, and nothing else.
        let static_maps = "\
00400000-004a0000 r-xp 00000000 fe:01 10 /home/k/.local/bin/voiceterm
00e00000-00e21000 rw-p 00000000 00:00 0 [heap]
7ffd0a000000-7ffd0a021000 rw-p 00000000 00:00 0 [stack]
";
        assert!(!maps_show_dynamic_loader(static_maps), "a static process looked dynamic");
        assert!(!maps_show_dynamic_loader(""), "an empty dump is not evidence of a loader");
    }

    /// An unknown answer must not accuse: a false "you are static" sends someone rebuilding
    /// a binary that is fine.
    #[test]
    fn an_unknown_linkage_is_treated_as_dynamic() {
        assert_eq!(diagnose(session(Some("wayland-0"), None, false)), None);
    }
}
