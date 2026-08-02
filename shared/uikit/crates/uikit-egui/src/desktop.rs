//! desktop — the Linux desktop integration every port was missing.
//!
//! A `.desktop` file matches a running window by its **application id**, and no
//! port set one: every frontend called `NativeOptions::default()`. The visible
//! consequences are the ones people report as "the app looks broken":
//!
//! * **No icon in the dock or alt-tab.** The compositor cannot find the
//!   `.desktop` entry, so it falls back to a generic placeholder.
//! * **No window grouping.** Two windows of the same app appear as unrelated
//!   entries.
//! * **Wrong name.** The window title is used instead of the entry's `Name`,
//!   so a title that changes with content changes the app's name in the dock.
//!
//! On Wayland this is the *only* mechanism — there is no WM_CLASS to fall back
//! on — so an unset id is not a cosmetic gap there, it is the whole integration.
//!
//! ## The id has to match the .desktop file exactly
//!
//! `org.snapclone.SnapClone.desktop` means the app id must be
//! `org.snapclone.SnapClone` — no extension, same case. That is the single most
//! common way this is got wrong, and it fails silently: the app runs perfectly
//! and simply has no icon.

/// Build the window options for an app, given its reverse-DNS id.
///
/// The id is also set as `StartupWMClass`'s counterpart on X11, so one value
/// covers both display servers and cannot drift between them.
pub fn viewport(app_id: &str, title: &str, size: [f32; 2]) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_app_id(app_id)
        .with_title(title)
        .with_inner_size(size)
}

/// Strip a `.desktop` suffix if someone passes the filename instead of the id.
///
/// Forgiving on purpose: passing the filename is the natural mistake, it fails
/// silently, and the cost of accepting both is one line.
pub fn app_id_from(entry: &str) -> &str {
    entry.strip_suffix(".desktop").unwrap_or(entry)
}

/// Is this a plausible reverse-DNS application id?
///
/// Checked rather than assumed because the failure is invisible: an id with a
/// trailing `.desktop`, a space, or no dot at all produces an app that runs
/// fine and has no icon, and nobody connects the two.
pub fn is_valid_app_id(id: &str) -> bool {
    !id.is_empty()
        && !id.ends_with(".desktop")
        && id.contains('.')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_filename_is_accepted_as_an_id() {
        // The natural mistake, and it fails silently.
        assert_eq!(app_id_from("org.snapclone.SnapClone.desktop"), "org.snapclone.SnapClone");
        assert_eq!(app_id_from("org.snapclone.SnapClone"), "org.snapclone.SnapClone");
    }

    #[test]
    fn the_invisible_mistakes_are_rejected() {
        // Each of these produces an app that runs perfectly and has no icon,
        // which is why they are worth checking rather than trusting.
        assert!(is_valid_app_id("org.snapclone.SnapClone"));
        assert!(!is_valid_app_id("org.snapclone.SnapClone.desktop"), "the extension");
        assert!(!is_valid_app_id("snapclone"), "no dot: not reverse-DNS");
        assert!(!is_valid_app_id("org snapclone"), "a space");
        assert!(!is_valid_app_id(""), "empty");
        assert!(!is_valid_app_id(".org.snapclone"), "leading dot");
        assert!(!is_valid_app_id("org.snapclone."), "trailing dot");
        assert!(!is_valid_app_id("org..snapclone"), "empty segment");
    }

    #[test]
    fn hyphens_and_underscores_are_allowed() {
        // Real ids use them: org.gnome.Text-Editor, com.my_org.App.
        assert!(is_valid_app_id("org.example.My-App"));
        assert!(is_valid_app_id("org.example.my_app"));
    }

    #[test]
    fn the_viewport_carries_the_id() {
        let v = viewport("org.example.App", "App", [800.0, 600.0]);
        assert_eq!(v.app_id.as_deref(), Some("org.example.App"));
        assert_eq!(v.title.as_deref(), Some("App"));
    }
}
