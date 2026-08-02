//! settings_page — the settings surface and the Bluetooth sink picker.
//!
//! `SettingsPage.qml` is 1,854 lines and the largest single surface in the app.
//! Three things in it carry real behaviour; the rest is rows.
//!
//! ## The dependency audit
//!
//! `dep_audit_json` lists external tools the app needs and whether they are
//! present. It exists because this is a router: without `bluetoothctl` there is
//! no Bluetooth, and finding that out when a speaker fails to connect is far
//! worse than being told on the settings page. A missing **optional** dependency
//! is not a failure and must not look like one — a page showing five red rows
//! for features nobody uses teaches people to ignore it.
//!
//! ## Scan progress
//!
//! The same `scan_progress_json` the scan picker uses. A scan of a large library
//! runs for minutes, so the page has to distinguish *running*, *finished* and
//! *never started* — and a progress bar stuck at 100% is the classic way to make
//! a finished job look hung.
//!
//! ## The sink picker's multiroom mode
//!
//! `multiMode` with `checkedMacs`, against `activeMultiroomMacs` from the
//! backend. The distinction that matters: what you have **ticked** is not what
//! is **playing**, and showing them as one control means a failed join silently
//! looks like a success.

/// One external tool the app depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub present: bool,
    /// Optional dependencies gate a feature; required ones gate the app.
    pub required: bool,
    /// What breaks without it — shown instead of a bare "missing", because
    /// "install bluez-utils" means nothing until you know what stops working.
    pub enables: String,
}

impl Dependency {
    /// Does this warrant an alarm? A missing optional tool is a note, not a
    /// fault: a page showing five red rows for features nobody uses teaches
    /// people to ignore the page.
    pub fn is_problem(&self) -> bool { self.required && !self.present }
}

/// A running or finished library scan.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Scan {
    /// Nothing has been started this session — NOT the same as a scan that
    /// finished with nothing, which is why this is not just `done: false`.
    #[default]
    Idle,
    Running { seen: usize, total: Option<usize>, path: String },
    Done { added: usize },
    Failed { reason: String },
}

impl Scan {
    /// 0..1, or `None` when the total is unknown — an indeterminate scan must
    /// show a spinner rather than a bar frozen at some arbitrary fraction.
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Scan::Running { seen, total: Some(t), .. } if *t > 0 =>
                Some((*seen as f32 / *t as f32).clamp(0.0, 1.0)),
            Scan::Done { .. } => Some(1.0),
            _ => None,
        }
    }
    pub fn running(&self) -> bool { matches!(self, Scan::Running { .. }) }
    /// One line for the row.
    pub fn label(&self) -> String {
        match self {
            Scan::Idle => "not scanned this session".into(),
            Scan::Running { seen, total: Some(t), .. } => format!("scanning {seen}/{t}"),
            Scan::Running { seen, .. } => format!("scanning… {seen} found"),
            Scan::Done { added } if *added == 0 => "scan finished, nothing new".into(),
            Scan::Done { added } => format!("scan finished, {added} added"),
            Scan::Failed { reason } => format!("scan failed: {reason}"),
        }
    }
}

/// The settings page.
#[derive(Debug, Clone, Default)]
pub struct SettingsPage {
    pub deps: Vec<Dependency>,
    pub scan: Scan,
    pub expanded: std::collections::BTreeSet<String>,
}

impl SettingsPage {
    /// Dependencies that actually need attention.
    pub fn problems(&self) -> Vec<&Dependency> {
        self.deps.iter().filter(|d| d.is_problem()).collect()
    }

    /// Optional tools that are absent — worth listing separately so a user can
    /// see what they are missing out on without it reading as breakage.
    pub fn optional_missing(&self) -> Vec<&Dependency> {
        self.deps.iter().filter(|d| !d.required && !d.present).collect()
    }

    pub fn toggle(&mut self, key: &str) {
        if !self.expanded.remove(key) { self.expanded.insert(key.to_string()); }
    }
    pub fn is_expanded(&self, key: &str) -> bool { self.expanded.contains(key) }

    /// A scan may only be started when one is not already running. Starting a
    /// second scan over the same library doubles the work and can report
    /// duplicate additions.
    pub fn can_scan(&self) -> bool { !self.scan.running() }
}

// ── Bluetooth sink picker ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Sink {
    pub mac: String,
    pub name: String,
    pub connected: bool,
}

impl Sink {
    /// A sink with no name shows its MAC — an unnamed row is unusable, and a
    /// MAC at least identifies the device on its own label.
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() { &self.mac } else { &self.name }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SinkPicker {
    pub open: bool,
    pub multi: bool,
    pub devices: Vec<Sink>,
    /// What the user has ticked.
    checked: Vec<String>,
    /// What the backend reports is actually in the multiroom group. Separate
    /// from `checked` on purpose: a failed join must not look like a success.
    pub active: Vec<String>,
}

impl SinkPicker {
    pub fn open(&mut self, devices: Vec<Sink>, active: Vec<String>) {
        self.open = true;
        self.devices = devices;
        self.active = active.clone();
        // Start from what is actually playing, so the dialog opens showing the
        // truth rather than an empty set the user has to rebuild.
        self.checked = active;
    }

    pub fn close(&mut self) { self.open = false; }

    pub fn toggle(&mut self, mac: &str) {
        if !self.devices.iter().any(|d| d.mac == mac) { return; }
        match self.checked.iter().position(|m| m == mac) {
            Some(i) => { self.checked.remove(i); }
            None => self.checked.push(mac.to_string()),
        }
    }

    pub fn is_checked(&self, mac: &str) -> bool { self.checked.iter().any(|m| m == mac) }
    pub fn is_active(&self, mac: &str) -> bool { self.active.iter().any(|m| m == mac) }

    /// Ticked but not yet joined — the row shows a spinner rather than a tick.
    pub fn is_pending(&self, mac: &str) -> bool { self.is_checked(mac) && !self.is_active(mac) }

    /// Has the user changed anything? Compared as SETS, because the order a
    /// user ticks speakers in is not a change to the group.
    pub fn is_dirty(&self) -> bool {
        let mut a: Vec<&String> = self.checked.iter().collect();
        let mut b: Vec<&String> = self.active.iter().collect();
        a.sort(); b.sort();
        a != b
    }

    /// The group to apply. `None` in single mode or when nothing changed.
    pub fn apply(&self) -> Option<Vec<String>> {
        if !self.multi || !self.is_dirty() { return None; }
        Some(self.checked.clone())
    }

    /// Single-mode selection: one sink, immediately.
    pub fn select(&mut self, mac: &str) -> Option<String> {
        if self.multi || !self.devices.iter().any(|d| d.mac == mac) { return None; }
        self.checked = vec![mac.to_string()];
        Some(mac.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(n: &str, present: bool, required: bool) -> Dependency {
        Dependency { name: n.into(), present, required, enables: "something".into() }
    }

    fn sink(mac: &str, name: &str) -> Sink {
        Sink { mac: mac.into(), name: name.into(), connected: true }
    }

    #[test]
    fn a_missing_optional_tool_is_not_an_alarm() {
        // Five red rows for features nobody uses teaches people to ignore the
        // page, which is how a REAL missing dependency gets missed.
        let p = SettingsPage {
            deps: vec![dep("bluetoothctl", false, true), dep("ffmpeg", false, false),
                       dep("pactl", true, true)],
            ..Default::default()
        };
        assert_eq!(p.problems().len(), 1, "only the required one");
        assert_eq!(p.problems()[0].name, "bluetoothctl");
        assert_eq!(p.optional_missing().len(), 1, "but the optional one is still listed");
    }

    #[test]
    fn a_scan_that_never_started_is_not_a_scan_that_found_nothing() {
        // Collapsing these into `done: false` makes a fresh session look like a
        // library with no music in it.
        let idle = Scan::Idle;
        let empty = Scan::Done { added: 0 };
        assert_ne!(idle, empty);
        assert!(idle.label().contains("not scanned"));
        assert!(empty.label().contains("nothing new"));
    }

    #[test]
    fn an_indeterminate_scan_has_no_fraction() {
        // A bar frozen at an arbitrary fraction is worse than a spinner: it
        // implies progress that is not being measured.
        let unknown = Scan::Running { seen: 40, total: None, path: "/m".into() };
        assert_eq!(unknown.fraction(), None);
        assert!(unknown.label().contains("40 found"));
        let known = Scan::Running { seen: 40, total: Some(200), path: "/m".into() };
        assert_eq!(known.fraction(), Some(0.2));
        // and a finished scan reads full rather than stuck
        assert_eq!(Scan::Done { added: 3 }.fraction(), Some(1.0));
        assert!(!Scan::Done { added: 3 }.running());
    }

    #[test]
    fn a_second_scan_cannot_start_while_one_runs() {
        // It doubles the work and can report duplicate additions.
        let mut p = SettingsPage::default();
        assert!(p.can_scan());
        p.scan = Scan::Running { seen: 1, total: None, path: "/m".into() };
        assert!(!p.can_scan());
        p.scan = Scan::Failed { reason: "no such path".into() };
        assert!(p.can_scan(), "a failed scan can be retried");
    }

    #[test]
    fn the_picker_opens_showing_what_is_actually_playing() {
        // Opening empty makes the user rebuild a group that already exists.
        let mut p = SinkPicker { multi: true, ..Default::default() };
        p.open(vec![sink("aa", "Kitchen"), sink("bb", "Study")], vec!["aa".into()]);
        assert!(p.is_checked("aa"));
        assert!(!p.is_dirty(), "nothing has been changed yet");
        assert_eq!(p.apply(), None);
    }

    #[test]
    fn ticked_is_not_playing() {
        // Showing them as one control means a failed join silently looks like a
        // success.
        let mut p = SinkPicker { multi: true, ..Default::default() };
        p.open(vec![sink("aa", "Kitchen"), sink("bb", "Study")], vec!["aa".into()]);
        p.toggle("bb");
        assert!(p.is_checked("bb"));
        assert!(!p.is_active("bb"));
        assert!(p.is_pending("bb"), "shown as joining, not joined");
        assert!(!p.is_pending("aa"), "already in the group");
        assert_eq!(p.apply().unwrap().len(), 2);
    }

    #[test]
    fn reordering_ticks_is_not_a_change() {
        // The order a user ticks speakers in is not a change to the group, and
        // treating it as one enables an Apply button that does nothing.
        let mut p = SinkPicker { multi: true, ..Default::default() };
        p.open(vec![sink("aa", "A"), sink("bb", "B")], vec!["aa".into(), "bb".into()]);
        assert!(!p.is_dirty());
        p.toggle("aa");
        p.toggle("aa");
        assert!(!p.is_dirty(), "off and on again is where it started");
    }

    #[test]
    fn single_mode_selects_one_and_never_applies_a_group() {
        let mut p = SinkPicker::default();
        p.open(vec![sink("aa", "A"), sink("bb", "B")], vec![]);
        assert_eq!(p.select("bb").as_deref(), Some("bb"));
        assert!(p.is_checked("bb"));
        assert!(!p.is_checked("aa"), "single mode replaces");
        assert_eq!(p.apply(), None, "never a group in single mode");
    }

    #[test]
    fn an_unknown_mac_is_ignored() {
        // A stale device list would otherwise let the user tick a speaker that
        // is no longer there and apply a group containing it.
        let mut p = SinkPicker { multi: true, ..Default::default() };
        p.open(vec![sink("aa", "A")], vec![]);
        p.toggle("zz");
        assert!(!p.is_checked("zz"));
        assert_eq!(p.select("zz"), None);
    }

    #[test]
    fn a_sink_with_no_name_shows_its_mac() {
        assert_eq!(sink("aa:bb", "").label(), "aa:bb");
        assert_eq!(sink("aa:bb", "Kitchen").label(), "Kitchen");
    }

    #[test]
    fn expanding_a_section_toggles() {
        let mut p = SettingsPage::default();
        assert!(!p.is_expanded("audio"));
        p.toggle("audio");
        assert!(p.is_expanded("audio"));
        p.toggle("audio");
        assert!(!p.is_expanded("audio"));
    }
}
