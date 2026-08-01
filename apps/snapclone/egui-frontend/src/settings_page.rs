//! settings_page — the five tab bodies, ported from `SettingsPage.cpp`.
//!
//! The shell already ports the sub-tab TABLE verbatim (General/Camera/Network/
//! Storage/About) and pins it with a test. What was behind the tabs was
//! nothing: the port had the names and no page bodies, which the parity audit
//! would happily have counted as "settings present".
//!
//! Tab contents follow the Qt comments exactly, because a settings page people
//! know is one where things are where they were:
//!
//! * **0 General** — identity, appearance, privacy
//! * **1 Camera** — camera tuning, accessibility (audio), face library
//! * **2 Network** — discovery/ports, STUN, crypto
//! * **3 Storage** — storage, export, security, ambient
//! * **4 About** — about, diagnostics, debug flags
//!
//! ## Dirty tracking, which is the part with teeth
//!
//! The Qt page marks itself dirty on any edit (`markD` wired to every widget)
//! and that drives "unsaved changes". Getting it wrong in either direction is
//! bad: miss an edit and the user loses work silently; report dirty when
//! nothing changed and the prompt becomes noise people click through — which
//! then hides the real one. So dirtiness is computed by COMPARING to the last
//! saved state rather than set by a flag on every keystroke.

/// Which tab, matching the Qt indices so `setCurrentSubTab(index)` from the nav
/// bar routes correctly. The numbering is the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    General,
    Camera,
    Network,
    Storage,
    About,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::General, Tab::Camera, Tab::Network, Tab::Storage, Tab::About];

    pub fn label(self) -> &'static str {
        match self {
            Tab::General => "General", Tab::Camera => "Camera", Tab::Network => "Network",
            Tab::Storage => "Storage", Tab::About => "About",
        }
    }

    /// `setCurrentSubTab(index)`. An out-of-range index falls back to General
    /// rather than showing a blank page, because a blank settings page looks
    /// like the app has lost its configuration.
    pub fn from_index(i: usize) -> Tab { *Tab::ALL.get(i).unwrap_or(&Tab::General) }
    pub fn index(self) -> usize { Tab::ALL.iter().position(|t| *t == self).unwrap_or(0) }
}

/// Everything the page edits. One struct so dirtiness is a comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    // General
    pub display_name: String,
    pub palette: String,
    pub read_receipts: bool,
    // Camera
    pub camera_device: String,
    pub mirror_front: bool,
    pub shutter_sound: bool,
    pub face_library: bool,
    // Network
    pub listen_port: u16,
    pub upnp: bool,
    pub stun_host: String,
    pub dht_host: String,
    pub require_encryption: bool,
    // Storage
    pub keep_days: u32,
    pub export_dir: String,
    pub ambient_enabled: bool,
    // About / debug
    pub debug_trace: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            display_name: String::new(), palette: "midnight".into(), read_receipts: true,
            camera_device: String::new(), mirror_front: true, shutter_sound: true,
            face_library: false,
            listen_port: 41234, upnp: true, stun_host: String::new(), dht_host: String::new(),
            require_encryption: true,
            keep_days: 30, export_dir: String::new(), ambient_enabled: false,
            debug_trace: false,
        }
    }
}

/// The page: current values, the last saved values, and which tab is showing.
#[derive(Debug, Clone, Default)]
pub struct SettingsPage {
    pub tab: Tab,
    pub current: Settings,
    saved: Settings,
}

impl SettingsPage {
    pub fn new(initial: Settings) -> SettingsPage {
        SettingsPage { tab: Tab::General, current: initial.clone(), saved: initial }
    }

    /// Are there unsaved changes?
    ///
    /// Computed rather than flagged: a flag set on every keystroke reports
    /// dirty when a user types a character and deletes it, and a prompt that
    /// cries wolf is one people click through — which then hides the real one.
    pub fn is_dirty(&self) -> bool { self.current != self.saved }

    /// Which tabs hold an unsaved change, so the UI can mark them. Without
    /// this, "unsaved changes" on a five-tab page means hunting.
    pub fn dirty_tabs(&self) -> Vec<Tab> {
        let c = &self.current;
        let s = &self.saved;
        let mut out = Vec::new();
        if c.display_name != s.display_name || c.palette != s.palette
            || c.read_receipts != s.read_receipts { out.push(Tab::General); }
        if c.camera_device != s.camera_device || c.mirror_front != s.mirror_front
            || c.shutter_sound != s.shutter_sound || c.face_library != s.face_library {
            out.push(Tab::Camera);
        }
        if c.listen_port != s.listen_port || c.upnp != s.upnp || c.stun_host != s.stun_host
            || c.dht_host != s.dht_host || c.require_encryption != s.require_encryption {
            out.push(Tab::Network);
        }
        if c.keep_days != s.keep_days || c.export_dir != s.export_dir
            || c.ambient_enabled != s.ambient_enabled { out.push(Tab::Storage); }
        if c.debug_trace != s.debug_trace { out.push(Tab::About); }
        out
    }

    /// Commit. Returns false when nothing changed, so a caller does not write a
    /// file and emit a "saved" toast for a no-op.
    pub fn save(&mut self) -> bool {
        if !self.is_dirty() { return false; }
        self.saved = self.current.clone();
        true
    }

    pub fn revert(&mut self) { self.current = self.saved.clone(); }

    /// Validate before saving. Returns the problems, empty when fine.
    ///
    /// Reported as a list rather than refusing at the first fault, because
    /// fixing settings one error at a time is miserable.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        let c = &self.current;
        // Ports below 1024 need privileges most desktop apps do not have, so
        // accepting one produces a listener that fails at runtime with a
        // confusing error rather than here with a clear one.
        if c.listen_port < 1024 {
            v.push(format!("port {} is privileged; use 1024 or above", c.listen_port));
        }
        if c.keep_days == 0 {
            v.push("keeping snaps for 0 days deletes them on arrival".into());
        }
        if !c.stun_host.is_empty() && !c.stun_host.contains(':') {
            v.push("STUN host needs a port, like stun.example.org:3478".into());
        }
        if c.display_name.chars().count() > 64 {
            v.push("display name is too long".into());
        }
        v
    }
}

/// Draw the page body for the current tab.
pub fn settings_ui(ui: &mut egui::Ui, page: &mut SettingsPage,
                   colors: &uikit_egui::PaletteColors) {
    let dirty = page.dirty_tabs();
    ui.horizontal(|ui| {
        for t in Tab::ALL {
            let mark = if dirty.contains(&t) { " •" } else { "" };
            if ui.selectable_label(page.tab == t, format!("{}{mark}", t.label())).clicked() {
                page.tab = t;
            }
        }
    });
    ui.separator();
    let c = &mut page.current;
    match page.tab {
        Tab::General => {
            ui.label("Identity");
            ui.text_edit_singleline(&mut c.display_name);
            ui.label("Appearance");
            egui::ComboBox::from_label("palette").selected_text(&c.palette).show_ui(ui, |ui| {
                for p in ["midnight", "daylight", "ember"] {
                    ui.selectable_value(&mut c.palette, p.into(), p);
                }
            });
            ui.label("Privacy");
            ui.checkbox(&mut c.read_receipts, "send read receipts");
        }
        Tab::Camera => {
            ui.label("Camera");
            ui.text_edit_singleline(&mut c.camera_device);
            ui.checkbox(&mut c.mirror_front, "mirror the front camera");
            ui.checkbox(&mut c.shutter_sound, "shutter sound");
            ui.label("Face library");
            ui.checkbox(&mut c.face_library, "recognise faces on this device");
        }
        Tab::Network => {
            ui.label("Discovery");
            ui.add(egui::DragValue::new(&mut c.listen_port).range(1..=65535).prefix("port "));
            ui.checkbox(&mut c.upnp, "UPnP port mapping");
            ui.label("STUN");
            ui.text_edit_singleline(&mut c.stun_host);
            ui.label("DHT");
            ui.text_edit_singleline(&mut c.dht_host);
            ui.label("Crypto");
            ui.checkbox(&mut c.require_encryption, "require encryption");
        }
        Tab::Storage => {
            ui.label("Retention");
            ui.add(egui::DragValue::new(&mut c.keep_days).range(0..=3650).suffix(" days"));
            ui.label("Export");
            ui.text_edit_singleline(&mut c.export_dir);
            ui.label("Ambient");
            ui.checkbox(&mut c.ambient_enabled, "ambient background effects");
        }
        Tab::About => {
            ui.label(format!("snapclone {}", env!("CARGO_PKG_VERSION")));
            ui.label("Diagnostics");
            ui.checkbox(&mut c.debug_trace, "verbose tracing");
        }
    }
    let problems = page.problems();
    if !problems.is_empty() {
        ui.separator();
        for p in &problems {
            ui.label(egui::RichText::new(format!("⚠ {p}")).color(colors.warning).small());
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        // Saving with problems is refused, but the button explains itself
        // rather than being mysteriously grey.
        let can = page.is_dirty() && problems.is_empty();
        if ui.add_enabled(can, egui::Button::new("Save")).clicked() { page.save(); }
        if ui.add_enabled(page.is_dirty(), egui::Button::new("Revert")).clicked() { page.revert(); }
        if page.is_dirty() {
            ui.label(egui::RichText::new("unsaved changes").color(colors.warning).small());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tab_indices_match_the_qt_contract() {
        // setCurrentSubTab(index) comes from the nav bar, so the numbering IS
        // the contract between the shell and this page.
        assert_eq!(Tab::from_index(0), Tab::General);
        assert_eq!(Tab::from_index(1), Tab::Camera);
        assert_eq!(Tab::from_index(2), Tab::Network);
        assert_eq!(Tab::from_index(3), Tab::Storage);
        assert_eq!(Tab::from_index(4), Tab::About);
        for (i, t) in Tab::ALL.iter().enumerate() { assert_eq!(t.index(), i); }
        // out of range falls back rather than showing a blank page, which looks
        // like the app lost its configuration
        assert_eq!(Tab::from_index(99), Tab::General);
    }

    #[test]
    fn dirtiness_is_computed_not_flagged() {
        // A flag set on every keystroke reports dirty when a user types a
        // character and deletes it, and a prompt that cries wolf gets clicked
        // through — which hides the real one.
        let mut p = SettingsPage::new(Settings::default());
        assert!(!p.is_dirty());
        p.current.display_name = "ada".into();
        assert!(p.is_dirty());
        p.current.display_name = String::new();
        assert!(!p.is_dirty(), "typing and deleting is not a change");
    }

    #[test]
    fn the_dirty_tab_is_named_so_the_user_is_not_hunting() {
        let mut p = SettingsPage::new(Settings::default());
        assert!(p.dirty_tabs().is_empty());
        p.current.listen_port = 5000;
        assert_eq!(p.dirty_tabs(), vec![Tab::Network]);
        p.current.mirror_front = false;
        let d = p.dirty_tabs();
        assert!(d.contains(&Tab::Camera) && d.contains(&Tab::Network));
        assert!(!d.contains(&Tab::General));
    }

    #[test]
    fn saving_a_no_op_does_nothing_and_says_so() {
        // Otherwise the app writes a file and shows "saved" for a click that
        // changed nothing, training the user to distrust the message.
        let mut p = SettingsPage::new(Settings::default());
        assert!(!p.save(), "nothing to save");
        p.current.upnp = false;
        assert!(p.save());
        assert!(!p.is_dirty(), "and it is clean afterwards");
        assert!(!p.save(), "saving twice changes nothing");
    }

    #[test]
    fn revert_restores_the_last_saved_state_not_the_defaults() {
        let mut p = SettingsPage::new(Settings { listen_port: 5000, ..Default::default() });
        p.current.listen_port = 6000;
        p.save();
        p.current.listen_port = 7000;
        p.revert();
        assert_eq!(p.current.listen_port, 6000, "back to SAVED, not to the default");
        assert!(!p.is_dirty());
    }

    #[test]
    fn validation_lists_every_problem_rather_than_the_first() {
        // Fixing settings one error at a time is miserable.
        let mut p = SettingsPage::new(Settings::default());
        p.current.listen_port = 80;
        p.current.keep_days = 0;
        p.current.stun_host = "stun.example.org".into();
        let v = p.problems();
        assert_eq!(v.len(), 3, "all three at once: {v:?}");
        assert!(v.iter().any(|s| s.contains("privileged")));
        assert!(v.iter().any(|s| s.contains("0 days")));
        assert!(v.iter().any(|s| s.contains("port")));
        // and a valid config has none
        p.current = Settings::default();
        assert!(p.problems().is_empty());
        // a STUN host WITH a port is fine
        p.current.stun_host = "stun.example.org:3478".into();
        assert!(p.problems().is_empty());
    }
}
