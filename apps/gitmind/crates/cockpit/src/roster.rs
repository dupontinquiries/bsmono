//! roster — the fleet, remembered between runs.
//!
//! `deploy::scan_ssh` finds hosts; it does not know which of them is "my
//! pinephone". That identity lives in the operator's head and has to outlive
//! both the IP and the OS on the device, because the whole workflow is wiping
//! these things repeatedly and expecting the fleet to still make sense
//! afterwards.
//!
//! ## What is stable, and what is not
//!
//! * The **id** is assigned once by the operator and never changes. It is the
//!   thing a label, a build target and a history hang off.
//! * The **hostname** is the join key against a scan. postmarketOS keeps it
//!   across a reflash of the same image, which makes it the most stable thing
//!   the network actually reports.
//! * The **IP** churns constantly — DHCP, a different network, a reboot — and
//!   is therefore a field to update rather than an identity. A roster keyed on
//!   address grows a duplicate every time a phone gets a new lease, which is
//!   how a fleet of four becomes a list of nine.
//!
//! ## A wipe has to be noticed, not discovered
//!
//! After a reflash the device answers on the same hostname with a new host key
//! and no fleet user. If the roster keeps saying `Keyed`, the planner skips
//! enrolment and the run fails at `scp` with a permission error that looks
//! like a bug. So a changed host key resets enrolment: the reroll then
//! re-enrols as its first step, which is exactly the behaviour the operator
//! wants and never has to ask for.

use crate::reroll::{Device, Enrollment, Reach};
use std::collections::BTreeMap;

/// What a scan saw: a reachable host and, when we could read it, its ssh host
/// key fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub ip: String,
    pub hostname: String,
    /// `None` when the key could not be read — treated as "unchanged" rather
    /// than as a wipe, because a failed read is not evidence of a reflash and
    /// forcing re-enrolment on a network hiccup would be worse than missing
    /// one.
    pub host_key: Option<String>,
}

/// One remembered device.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Assigned by the operator, never derived. Survives everything.
    pub id: String,
    pub label: String,
    /// Join key against a scan.
    pub hostname: String,
    /// Last known address. Updated by every scan.
    pub ip: String,
    pub triple: String,
    pub enrollment: Enrollment,
    /// The host key we last trusted, if we have ever read one.
    pub host_key: Option<String>,
    /// Was it in the most recent scan?
    pub online: bool,
    /// Package index known-stale. Set on a reflash, cleared by a refresh —
    /// a just-imaged phone always needs `apk update` before anything installs
    /// against it.
    pub needs_refresh: bool,
}

impl Entry {
    /// The planner's view of this device.
    pub fn to_device(&self) -> Device {
        Device {
            id: self.id.clone(),
            label: self.label.clone(),
            host: self.ip.clone(),
            triple: self.triple.clone(),
            enrollment: self.enrollment,
            reach: if self.online { Reach::Online } else { Reach::Offline },
            // The roster's name is the intended one; a device answering to
            // something else gets renamed rather than the roster relabelling
            // itself to match whatever the phone happens to say.
            desired_hostname: Some(self.hostname.clone()),
            needs_refresh: self.needs_refresh,
        }
    }
}

/// What changed about a device when a scan came in. Returned so the GUI can
/// say "pine moved to .47" or "fp5 looks reflashed" instead of silently
/// mutating the list under the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Appeared { id: String },
    Vanished { id: String },
    MovedTo { id: String, ip: String },
    /// Host key changed: the device was reflashed, and enrolment was reset.
    Reflashed { id: String },
    /// Seen on the network and not on the roster.
    Unknown { hostname: String, ip: String },
}

/// The fleet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roster {
    entries: BTreeMap<String, Entry>,
}

impl Roster {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a device the operator has named.
    ///
    /// Refuses a duplicate id rather than overwriting: an id is the one thing
    /// here that is supposed to be permanent, and silently replacing one would
    /// detach a label from its history.
    pub fn add(&mut self, e: Entry) -> Result<(), String> {
        if self.entries.contains_key(&e.id) {
            return Err(format!("{} is already on the roster", e.id));
        }
        self.entries.insert(e.id.clone(), e);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.entries.remove(id).is_some()
    }

    /// Every device, in id order, as the planner wants them.
    pub fn devices(&self) -> Vec<Device> {
        self.entries.values().map(|e| e.to_device()).collect()
    }

    /// Fold a scan into the roster.
    ///
    /// Devices absent from the scan go offline; they are NOT removed. A phone
    /// that is switched off is still one of your phones, and deleting it would
    /// throw away the label and the target triple the operator set — which
    /// they would then have to type again after every reroll.
    pub fn observe(&mut self, seen: &[Seen]) -> Vec<Change> {
        let mut changes = Vec::new();
        let by_host: BTreeMap<&str, &Seen> =
            seen.iter().map(|s| (s.hostname.as_str(), s)).collect();

        for e in self.entries.values_mut() {
            match by_host.get(e.hostname.as_str()) {
                Some(s) => {
                    if !e.online {
                        changes.push(Change::Appeared { id: e.id.clone() });
                    }
                    e.online = true;
                    if e.ip != s.ip {
                        e.ip = s.ip.clone();
                        changes.push(Change::MovedTo { id: e.id.clone(), ip: s.ip.clone() });
                    }
                    // A changed key means a new OS on the same device. Reset
                    // the trust so the next plan re-enrols instead of failing
                    // at scp with something that reads like a bug.
                    match (&e.host_key, &s.host_key) {
                        (Some(old), Some(new)) if old != new => {
                            e.host_key = Some(new.clone());
                            e.enrollment = Enrollment::None;
                            // A new OS means a stale index too.
                            e.needs_refresh = true;
                            changes.push(Change::Reflashed { id: e.id.clone() });
                        }
                        // First time we have managed to read one: record it,
                        // and do NOT call that a reflash.
                        (None, Some(new)) => e.host_key = Some(new.clone()),
                        _ => {}
                    }
                }
                None => {
                    if e.online {
                        changes.push(Change::Vanished { id: e.id.clone() });
                    }
                    e.online = false;
                }
            }
        }

        // Hosts on the network that nobody has named. Surfaced rather than
        // auto-added: the operator's roster should contain devices they chose,
        // or a reroll starts pushing builds at the neighbour's printer.
        let known: Vec<String> =
            self.entries.values().map(|e| e.hostname.clone()).collect();
        for s in seen {
            if !known.iter().any(|h| h == &s.hostname) {
                changes.push(Change::Unknown {
                    hostname: s.hostname.clone(),
                    ip: s.ip.clone(),
                });
            }
        }
        changes
    }

    /// Record that enrolment succeeded, so the next plan does not repeat it.
    pub fn mark_enrolled(&mut self, id: &str, host_key: Option<String>) -> bool {
        match self.entries.get_mut(id) {
            Some(e) => {
                e.enrollment = Enrollment::Keyed;
                if host_key.is_some() {
                    e.host_key = host_key;
                }
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, host: &str, ip: &str) -> Entry {
        Entry {
            id: id.into(),
            label: id.into(),
            hostname: host.into(),
            ip: ip.into(),
            triple: "aarch64-unknown-linux-gnu".into(),
            enrollment: Enrollment::Keyed,
            host_key: Some("KEY-A".into()),
            online: true,
            needs_refresh: false,
        }
    }

    fn seen(host: &str, ip: &str, key: &str) -> Seen {
        Seen { ip: ip.into(), hostname: host.into(), host_key: Some(key.into()) }
    }

    #[test]
    fn a_new_lease_moves_a_device_rather_than_cloning_it() {
        let mut r = Roster::default();
        r.add(entry("pine", "pine64", "192.168.1.20")).unwrap();
        let changes = r.observe(&[seen("pine64", "192.168.1.47", "KEY-A")]);
        assert_eq!(r.len(), 1, "a roster keyed on address would now hold two");
        assert_eq!(r.get("pine").unwrap().ip, "192.168.1.47");
        assert_eq!(changes, vec![Change::MovedTo {
            id: "pine".into(), ip: "192.168.1.47".into() }]);
    }

    #[test]
    fn a_reflash_resets_trust_so_the_next_run_re_enrols() {
        // The whole workflow is wiping these repeatedly. If the roster keeps
        // saying Keyed, the planner skips enrolment and the run dies at scp
        // with a permission error that looks like a bug in gitmind.
        let mut r = Roster::default();
        r.add(entry("fp5", "fairphone", "10.0.0.5")).unwrap();
        let changes = r.observe(&[seen("fairphone", "10.0.0.5", "KEY-B")]);
        assert_eq!(r.get("fp5").unwrap().enrollment, Enrollment::None);
        assert!(changes.contains(&Change::Reflashed { id: "fp5".into() }));
        // And the planner now schedules the enrolment on its own.
        let plan = crate::reroll::plan(&r.devices(), &["voiceterm".to_string()]);
        assert_eq!(plan.enroll_count(), 1);
    }

    #[test]
    fn an_unreadable_host_key_is_not_evidence_of_a_wipe() {
        let mut r = Roster::default();
        r.add(entry("pine", "pine64", "10.0.0.9")).unwrap();
        let changes = r.observe(&[Seen {
            ip: "10.0.0.9".into(), hostname: "pine64".into(), host_key: None }]);
        assert_eq!(r.get("pine").unwrap().enrollment, Enrollment::Keyed,
                   "a failed read must not force a re-enrol on a network hiccup");
        assert!(!changes.iter().any(|c| matches!(c, Change::Reflashed { .. })));
    }

    #[test]
    fn first_sight_of_a_key_is_recorded_and_is_not_a_reflash() {
        let mut r = Roster::default();
        let mut e = entry("pine", "pine64", "10.0.0.9");
        e.host_key = None;
        r.add(e).unwrap();
        let changes = r.observe(&[seen("pine64", "10.0.0.9", "KEY-A")]);
        assert_eq!(r.get("pine").unwrap().host_key.as_deref(), Some("KEY-A"));
        assert_eq!(r.get("pine").unwrap().enrollment, Enrollment::Keyed);
        assert!(changes.is_empty());
    }

    #[test]
    fn a_switched_off_phone_goes_offline_and_keeps_its_settings() {
        let mut r = Roster::default();
        r.add(entry("fp5", "fairphone", "10.0.0.5")).unwrap();
        let changes = r.observe(&[]);
        assert_eq!(changes, vec![Change::Vanished { id: "fp5".into() }]);
        let e = r.get("fp5").unwrap();
        assert!(!e.online);
        // Deleting it would throw away the label and the triple, which the
        // operator would then retype after every reroll.
        assert_eq!(e.triple, "aarch64-unknown-linux-gnu");
        assert_eq!(e.enrollment, Enrollment::Keyed);
        // It comes back as Appeared, not as a new device.
        let changes = r.observe(&[seen("fairphone", "10.0.0.5", "KEY-A")]);
        assert_eq!(changes, vec![Change::Appeared { id: "fp5".into() }]);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn strangers_on_the_network_are_reported_not_adopted() {
        let mut r = Roster::default();
        r.add(entry("pine", "pine64", "10.0.0.9")).unwrap();
        let changes = r.observe(&[
            seen("pine64", "10.0.0.9", "KEY-A"),
            seen("brother-printer", "10.0.0.31", "KEY-P"),
        ]);
        assert_eq!(r.len(), 1, "auto-adopting would push builds at the printer");
        assert!(changes.contains(&Change::Unknown {
            hostname: "brother-printer".into(), ip: "10.0.0.31".into() }));
    }

    #[test]
    fn an_id_is_permanent_so_adding_it_twice_is_refused() {
        let mut r = Roster::default();
        r.add(entry("pine", "pine64", "10.0.0.9")).unwrap();
        let err = r.add(entry("pine", "other-host", "10.0.0.10")).unwrap_err();
        assert!(err.contains("already on the roster"));
        assert_eq!(r.get("pine").unwrap().hostname, "pine64");
    }

    #[test]
    fn enrolling_is_remembered_so_the_next_plan_skips_it() {
        let mut r = Roster::default();
        let mut e = entry("fp5", "fairphone", "10.0.0.5");
        e.enrollment = Enrollment::None;
        r.add(e).unwrap();
        assert_eq!(crate::reroll::plan(&r.devices(), &["v".to_string()]).enroll_count(), 1);
        assert!(r.mark_enrolled("fp5", Some("KEY-B".into())));
        assert_eq!(crate::reroll::plan(&r.devices(), &["v".to_string()]).enroll_count(), 0);
        assert!(!r.mark_enrolled("nobody", None));
    }
}
