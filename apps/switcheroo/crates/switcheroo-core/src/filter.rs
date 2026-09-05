//! The menu's device filter: what to show from everything the scan found.
//!
//! Pure over `&Device`, so "favourites only, name contains 'porch', hide
//! anything weaker than −80 dBm" is a test. The UI holds one of these and
//! renders it as chips; the filter itself never sees egui.

use crate::{Device, Kind};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Case-insensitive substring of the name (or id, so an unnamed plug is
    /// still findable by address).
    pub query: String,
    /// Only kinds in this set. Empty = all kinds.
    pub kinds: Vec<Kind>,
    /// Only devices switcheroo can actually switch (GATT, or paired HomeKit).
    pub controllable_only: bool,
    pub favourites_only: bool,
    /// Drop anything weaker than this many dBm. `None` = no floor.
    pub min_rssi: Option<i16>,
    /// Drop devices not heard from within this long.
    pub hide_stale_after: Option<Duration>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { query: String::new(), kinds: Vec::new(), controllable_only: false, favourites_only: false, min_rssi: None, hide_stale_after: None }
    }
}

impl Filter {
    pub fn is_default(&self) -> bool { *self == Filter::default() }

    pub fn accepts(&self, d: &Device, now: Instant) -> bool {
        if !self.query.is_empty() {
            let q = self.query.to_ascii_lowercase();
            if !d.name.to_ascii_lowercase().contains(&q) && !d.id.as_str().to_ascii_lowercase().contains(&q) {
                return false;
            }
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&d.kind) { return false; }
        if self.controllable_only && !d.is_controllable() { return false; }
        if self.favourites_only && !d.favourite { return false; }
        if let Some(floor) = self.min_rssi {
            match d.rssi { Some(r) if r >= floor => {}, _ => return false }
        }
        if let Some(after) = self.hide_stale_after {
            if d.is_stale(now, after) { return false; }
        }
        true
    }

    pub fn apply<'a>(&self, devices: impl IntoIterator<Item = &'a Device>, now: Instant) -> Vec<&'a Device> {
        devices.into_iter().filter(|d| self.accepts(d, now)).collect()
    }

    /// Chip labels for the active parts, e.g. `["porch", "favourites", "≥ -80 dBm"]`.
    pub fn active_labels(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.query.is_empty() { v.push(format!("\"{}\"", self.query)); }
        for k in &self.kinds { v.push(k.label().to_string()); }
        if self.controllable_only { v.push("switchable".into()); }
        if self.favourites_only { v.push("favourites".into()); }
        if let Some(r) = self.min_rssi { v.push(format!("≥ {r} dBm")); }
        if let Some(d) = self.hide_stale_after { v.push(format!("seen < {}s", d.as_secs())); }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceId;

    fn dev(name: &str, kind: Kind, rssi: Option<i16>, now: Instant) -> Device {
        let mut d = Device::new(DeviceId::new(name.to_ascii_lowercase()), name, kind, now);
        d.rssi = rssi;
        d
    }

    #[test]
    fn each_axis_filters_and_the_default_passes_everything() {
        let now = Instant::now();
        let mut porch = dev("Porch Plug", Kind::GenericGatt, Some(-60), now);
        porch.favourite = true;
        let wemo = dev("Wemo", Kind::HomeKit, Some(-85), now);
        let mut old = dev("Old Thing", Kind::Unknown, None, now);
        old.last_seen = now - Duration::from_secs(120);
        let all = [porch.clone(), wemo.clone(), old.clone()];

        assert_eq!(Filter::default().apply(&all, now).len(), 3);
        assert_eq!(Filter { query: "por".into(), ..Default::default() }.apply(&all, now).len(), 1);
        assert_eq!(Filter { query: "WEMO".into(), ..Default::default() }.apply(&all, now)[0].name, "Wemo");
        assert_eq!(Filter { kinds: vec![Kind::HomeKit], ..Default::default() }.apply(&all, now).len(), 1);
        assert_eq!(Filter { controllable_only: true, ..Default::default() }.apply(&all, now).len(), 1, "unpaired HomeKit is not switchable");
        assert_eq!(Filter { favourites_only: true, ..Default::default() }.apply(&all, now).len(), 1);
        assert_eq!(Filter { min_rssi: Some(-80), ..Default::default() }.apply(&all, now).len(), 1, "no rssi fails a floor");
        assert_eq!(Filter { hide_stale_after: Some(Duration::from_secs(60)), ..Default::default() }.apply(&all, now).len(), 2);
    }

    #[test]
    fn a_paired_homekit_device_counts_as_switchable() {
        let now = Instant::now();
        let mut wemo = dev("Wemo", Kind::HomeKit, Some(-50), now);
        assert!(!Filter { controllable_only: true, ..Default::default() }.accepts(&wemo, now));
        wemo.paired = true;
        assert!(Filter { controllable_only: true, ..Default::default() }.accepts(&wemo, now));
    }

    #[test]
    fn labels_name_the_active_parts_only() {
        let f = Filter { query: "porch".into(), favourites_only: true, min_rssi: Some(-80), ..Default::default() };
        assert_eq!(f.active_labels(), vec!["\"porch\"", "favourites", "≥ -80 dBm"]);
        assert!(Filter::default().active_labels().is_empty());
        assert!(Filter::default().is_default());
    }
}
