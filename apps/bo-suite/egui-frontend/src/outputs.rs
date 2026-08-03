//! outputs — `DeviceSelector`, the top-area product switcher.
//!
//! `BtSinkPicker` is NOT here. I wrote one and then found `settings_page.rs`
//! already had `SinkPicker`, with a rule mine did not: `active` (what the
//! backend reports is really in the multiroom group) kept separate from
//! `checked` (what the user ticked), so a failed join cannot look like a
//! success. Shipping a second picker would have been the "two lists of the same
//! people" failure this port keeps finding in Qt — in our own code this time.
//! The missing Qt rules went into that one instead; this module keeps only the
//! surface that did not already exist.

// ── device selector ─────────────────────────────────────────────────────────

/// One known device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The url is the identity: two devices can share a name.
    pub url: String,
    pub name: String,
    pub kind: String,
}

impl Device {
    pub fn label(&self) -> String {
        if self.name.trim().is_empty() { self.url.clone() } else { self.name.clone() }
    }
}

/// The top-area product selector.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceSelector {
    devices: Vec<Device>,
    active_url: String,
    pub expanded: bool,
    /// 0..=1 unfold progress.
    t: f32,
}

/// The panel's unfold, in milliseconds.
pub const UNFOLD_MS: f32 = 220.0;

impl DeviceSelector {
    pub fn set_devices(&mut self, devices: Vec<Device>) {
        // If the active device vanished, fall back to the first rather than
        // showing a name for something that is no longer there.
        if !devices.iter().any(|d| d.url == self.active_url) {
            self.active_url = devices.first().map(|d| d.url.clone()).unwrap_or_default();
        }
        self.devices = devices;
    }

    pub fn devices(&self) -> &[Device] { &self.devices }
    pub fn active_url(&self) -> &str { &self.active_url }
    pub fn active(&self) -> Option<&Device> {
        self.devices.iter().find(|d| d.url == self.active_url)
    }

    /// The collapsed row's text.
    pub fn title(&self) -> String {
        match self.active() {
            Some(d) => d.label(),
            None => "No device".into(),
        }
    }

    pub fn toggle(&mut self) { self.expanded = !self.expanded; }

    /// Select a device. Collapses the panel — leaving it open after a choice
    /// makes the user close it by hand to see what they chose.
    pub fn select(&mut self, url: &str) -> bool {
        if !self.devices.iter().any(|d| d.url == url) { return false; }
        self.active_url = url.to_string();
        self.expanded = false;
        true
    }

    pub fn tick(&mut self, dt_ms: f32) {
        let target = if self.expanded { 1.0 } else { 0.0 };
        let step = dt_ms / UNFOLD_MS;
        if self.t < target { self.t = (self.t + step).min(target); }
        else if self.t > target { self.t = (self.t - step).max(target); }
    }

    /// Eased unfold.
    pub fn unfold(&self) -> f32 { uikit_egui::motion::ease::out_cubic(self.t.clamp(0.0, 1.0)) }

    /// Still drawn while folding away, or the panel vanishes instead of closing.
    pub fn panel_visible(&self) -> bool { self.expanded || self.t > 0.0 }

    /// The chevron's rotation in degrees — it points down closed and up open,
    /// and rotates WITH the unfold so the two read as one movement.
    pub fn chevron_deg(&self) -> f32 { 180.0 * self.unfold() }
}

/// Draw the device selector. Returns the url when one is chosen.
pub fn device_selector_ui(ui: &mut egui::Ui, d: &mut DeviceSelector,
                          c: &uikit_egui::PaletteColors) -> Option<String> {
    d.tick(ui.input(|i| i.stable_dt) * 1000.0);
    let mut picked = None;
    ui.horizontal(|ui| {
        if ui.selectable_label(d.expanded, d.title()).clicked() { d.toggle(); }
        // The chevron rotates with the unfold so the two read as one movement.
        let glyph = if d.chevron_deg() > 90.0 { "▴" } else { "▾" };
        ui.colored_label(c.text_weak, glyph);
    });
    if d.panel_visible() {
        let reveal = d.unfold();
        for dev in d.devices().to_vec() {
            let h = 26.0 * reveal;
            if h < 1.0 { continue; }
            let active = dev.url == d.active_url();
            let text = egui::RichText::new(dev.label())
                .color(if active { c.accent } else { c.text });
            if ui.add_sized([ui.available_width(), h],
                egui::SelectableLabel::new(active, text)).clicked() {
                if d.select(&dev.url) { picked = Some(dev.url.clone()); }
            }
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── device selector ─────────────────────────────────────────────────────

    fn devices() -> Vec<Device> {
        vec![Device { url: "u1".into(), name: "Stage".into(), kind: "stage".into() },
             Device { url: "u2".into(), name: "Balance".into(), kind: "balance".into() }]
    }

    #[test]
    fn the_first_device_becomes_active_when_there_was_none() {
        let mut d = DeviceSelector::default();
        d.set_devices(devices());
        assert_eq!(d.active_url(), "u1");
        assert_eq!(d.title(), "Stage");
    }

    #[test]
    fn an_active_device_that_disappears_is_replaced_rather_than_shown_as_a_ghost() {
        let mut d = DeviceSelector::default();
        d.set_devices(devices());
        d.select("u2");
        d.set_devices(vec![devices()[0].clone()]);
        assert_eq!(d.active_url(), "u1", "fell back rather than naming something gone");
    }

    #[test]
    fn no_devices_at_all_says_so() {
        let d = DeviceSelector::default();
        assert_eq!(d.title(), "No device");
        assert!(d.active().is_none());
    }

    #[test]
    fn selecting_a_device_collapses_the_panel() {
        // Leaving it open makes the user close it by hand to see what they
        // chose.
        let mut d = DeviceSelector::default();
        d.set_devices(devices());
        d.toggle();
        assert!(d.expanded);
        assert!(d.select("u2"));
        assert!(!d.expanded);
        assert_eq!(d.title(), "Balance");
    }

    #[test]
    fn selecting_a_device_that_is_not_there_changes_nothing() {
        let mut d = DeviceSelector::default();
        d.set_devices(devices());
        assert!(!d.select("nope"));
        assert_eq!(d.active_url(), "u1");
    }

    #[test]
    fn the_panel_stays_drawn_while_it_folds_away() {
        // Dropping it at expanded=false makes the panel vanish rather than
        // close.
        let mut d = DeviceSelector::default();
        d.set_devices(devices());
        d.toggle();
        d.tick(UNFOLD_MS);
        assert_eq!(d.unfold(), 1.0);
        d.select("u2");
        d.tick(UNFOLD_MS / 4.0);
        assert!(d.panel_visible(), "still folding");
        d.tick(UNFOLD_MS);
        assert!(!d.panel_visible());
    }

    #[test]
    fn the_chevron_turns_with_the_unfold_so_they_read_as_one_movement() {
        let mut d = DeviceSelector::default();
        d.set_devices(devices());
        assert_eq!(d.chevron_deg(), 0.0);
        d.toggle();
        d.tick(UNFOLD_MS / 2.0);
        let mid = d.chevron_deg();
        assert!(mid > 0.0 && mid < 180.0, "part way round: {mid}");
        d.tick(UNFOLD_MS);
        assert_eq!(d.chevron_deg(), 180.0);
    }

    #[test]
    fn a_nameless_device_falls_back_to_its_url() {
        let d = Device { url: "http://1.2.3.4".into(), name: "".into(), kind: "".into() };
        assert_eq!(d.label(), "http://1.2.3.4");
    }
}
