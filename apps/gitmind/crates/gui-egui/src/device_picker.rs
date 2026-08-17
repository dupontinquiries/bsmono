//! device_picker — choose a device, trust its host key, install your key.
//!
//! Ported from `qml/DevicePicker.qml`, which the Qt app embeds wherever a host
//! is needed. Its own header: "discover on the LAN, select, trust the host
//! key, and install your key for passwordless access."
//!
//! ## Why this is the piece the fleet layer was missing
//!
//! `cockpit::roster` remembers devices and `cockpit::reroll` plans work across
//! them, but nothing in the egui port lets a person *add* one. Enrolment was
//! the step the fleet design kept deferring — the planner emits
//! `Step::Enroll`, and until now there was no surface behind it.
//!
//! Every operation it needs already exists in cockpit: `deploy::scan_ssh`,
//! `deploy::host_trusted`, `deploy::setup_key`. This is the model that
//! sequences them, kept apart from the widget so the sequence can be tested
//! without a network or a screen.
//!
//! ## Trust is re-read, never remembered
//!
//! The QML calls `host_trusted` again on every selection and after every trust
//! action rather than caching, and that is worth preserving: a host key can
//! change under you — a reflash is exactly that, and this fleet reflashes
//! constantly — so a cached "trusted" is a claim about a machine that may no
//! longer be the one answering.
//!
//! ## The password never enters the model
//!
//! `setup_key` needs one to authenticate the first time. It is passed straight
//! to that call and never stored on this struct, because the struct is `Debug`
//! and ends up in logs, and a password that lives in a UI type outlives the
//! moment it was needed.

/// Where the picker is in its sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Nothing discovered yet.
    Idle,
    /// A scan is running.
    Discovering,
    /// Devices found; none chosen. Carries how many.
    Found(usize),
    /// A device is chosen. `trusted` decides which action is offered.
    Selected { trusted: bool },
    /// A trust or key-install job is in flight.
    Working(Job),
    /// Something failed, with the reason to show.
    Failed(String),
}

/// What long-running thing is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    Discover,
    Trust,
    SetupKey,
}

/// One discovered host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub ip: String,
    /// Reverse-resolved name when there was one. The QML falls back to the ip
    /// for display, and so does [`Picker::selected_label`].
    pub hostname: String,
}

/// The picker's state.
#[derive(Debug, Clone)]
pub struct Picker {
    pub phase: Phase,
    pub devices: Vec<Found>,
    /// An index, not a copy of the device: a re-scan that drops the selected
    /// host must invalidate the selection rather than leave a stale row
    /// pointing at something no longer on the network.
    pub selected: Option<usize>,
    /// The account to connect as — `user` on a fresh postmarketOS image.
    pub user: String,
    pub note: String,
}

impl Default for Picker {
    fn default() -> Self {
        Picker {
            phase: Phase::Idle,
            devices: Vec::new(),
            selected: None,
            user: "user".into(),
            note: String::new(),
        }
    }
}

impl Picker {
    /// A scan started.
    pub fn discovering(&mut self) {
        self.phase = Phase::Discovering;
        self.note.clear();
    }

    /// A scan finished.
    ///
    /// The selection survives only if the same ip came back. A device that
    /// left the network cannot be the target of a trust or a key install, and
    /// keeping the row selected would offer both.
    pub fn discovered(&mut self, found: Vec<Found>) {
        let keep = self
            .selected_ip()
            .and_then(|ip| found.iter().position(|d| d.ip == ip));
        self.devices = found;
        self.selected = keep;
        self.phase = match keep {
            // Trust is NOT carried across a scan — see the header.
            Some(_) => Phase::Selected { trusted: false },
            None => Phase::Found(self.devices.len()),
        };
    }

    /// Choose a device. `trusted` is the freshly-read answer from
    /// `deploy::host_trusted`, not a remembered one.
    pub fn select(&mut self, idx: usize, trusted: bool) {
        if idx >= self.devices.len() {
            return;
        }
        self.selected = Some(idx);
        self.phase = Phase::Selected { trusted };
    }

    pub fn selected_ip(&self) -> Option<String> {
        self.selected.and_then(|i| self.devices.get(i)).map(|d| d.ip.clone())
    }

    /// What to show for the chosen device: the hostname, or the ip when there
    /// is none — matching the QML's `selName` fallback.
    pub fn selected_label(&self) -> Option<String> {
        self.selected.and_then(|i| self.devices.get(i)).map(|d| {
            if d.hostname.is_empty() { d.ip.clone() } else { d.hostname.clone() }
        })
    }

    /// Begin a job. Refused for the host-scoped ones with nothing selected,
    /// because both name a host and there is none to name.
    pub fn start(&mut self, job: Job) -> bool {
        if job != Job::Discover && self.selected.is_none() {
            return false;
        }
        self.phase = Phase::Working(job);
        self.note.clear();
        true
    }

    /// A job finished. `trusted` is re-read afterwards, for the same reason it
    /// is re-read on selection.
    pub fn finished(&mut self, job: Job, ok: bool, message: &str, trusted: bool) {
        self.note = message.to_string();
        self.phase = if !ok {
            Phase::Failed(message.to_string())
        } else if job == Job::Discover {
            Phase::Found(self.devices.len())
        } else {
            Phase::Selected { trusted }
        };
    }

    /// May a key be installed right now?
    ///
    /// Only onto a trusted host. `setup_key` authenticates with a password
    /// over ssh, and doing that before the host key is verified is how a
    /// credential reaches a machine that answered but is not the one meant.
    pub fn can_setup_key(&self) -> bool {
        matches!(self.phase, Phase::Selected { trusted: true })
    }

    /// May the host key be trusted right now?
    pub fn can_trust(&self) -> bool {
        matches!(self.phase, Phase::Selected { trusted: false })
    }

    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Discovering | Phase::Working(_))
    }

    /// The one line under the picker.
    pub fn status_line(&self) -> String {
        match &self.phase {
            Phase::Idle => "no devices — scan the network".into(),
            Phase::Discovering | Phase::Working(Job::Discover) => "scanning…".into(),
            Phase::Found(0) => "nothing answered on this subnet".into(),
            Phase::Found(n) => format!("{n} device(s) — choose one"),
            Phase::Selected { trusted: false } => format!(
                "{} — host key NOT trusted",
                self.selected_label().unwrap_or_default()
            ),
            Phase::Selected { trusted: true } => {
                format!("{} — trusted", self.selected_label().unwrap_or_default())
            }
            Phase::Working(Job::Trust) => "adding the host key…".into(),
            Phase::Working(Job::SetupKey) => "installing your key…".into(),
            Phase::Failed(why) => format!("failed: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(n: usize) -> Vec<Found> {
        (0..n)
            .map(|i| Found {
                ip: format!("10.0.0.{}", 10 + i),
                hostname: if i == 0 { String::new() } else { format!("phone{i}") },
            })
            .collect()
    }

    #[test]
    fn a_key_cannot_be_installed_before_the_host_key_is_trusted() {
        // setup_key authenticates with a PASSWORD over ssh. Sending one to a
        // host whose key has not been verified is how a credential reaches a
        // machine that answered but is not the one meant.
        let mut p = Picker::default();
        p.discovered(found(2));
        p.select(0, false);
        assert!(!p.can_setup_key(), "untrusted host must not be sent a password");
        assert!(p.can_trust());
        p.select(0, true);
        assert!(p.can_setup_key());
        assert!(!p.can_trust(), "already trusted");
    }

    #[test]
    fn a_rescan_that_loses_the_device_drops_the_selection() {
        // Keeping it would offer trust and key-install against a host that is
        // no longer on the network.
        let mut p = Picker::default();
        p.discovered(found(3));
        p.select(2, true);
        assert_eq!(p.selected_ip().as_deref(), Some("10.0.0.12"));
        p.discovered(found(1));
        assert_eq!(p.selected, None);
        assert!(!p.can_setup_key() && !p.can_trust());
    }

    #[test]
    fn a_rescan_that_keeps_the_device_re_asks_trust() {
        // The ip came back so the row survives, but trust resets until it is
        // read again: a host key changes under you on every reflash, and this
        // fleet reflashes constantly.
        let mut p = Picker::default();
        p.discovered(found(3));
        p.select(1, true);
        p.discovered(found(3));
        assert_eq!(p.selected, Some(1), "same ip came back");
        assert_eq!(
            p.phase,
            Phase::Selected { trusted: false },
            "trust must be re-read, never carried across a scan"
        );
    }

    #[test]
    fn the_label_falls_back_to_the_ip_like_the_qml_does() {
        let mut p = Picker::default();
        p.discovered(found(2));
        p.select(0, false);
        assert_eq!(p.selected_label().as_deref(), Some("10.0.0.10"), "no hostname");
        p.select(1, false);
        assert_eq!(p.selected_label().as_deref(), Some("phone1"));
    }

    #[test]
    fn a_host_job_is_refused_with_nothing_selected() {
        let mut p = Picker::default();
        p.discovered(found(2));
        assert!(!p.start(Job::Trust), "trust names a host and there is none");
        assert!(!p.start(Job::SetupKey));
        assert!(p.start(Job::Discover), "discovery needs no selection");
    }

    #[test]
    fn a_failure_shows_its_reason_and_offers_no_action() {
        let mut p = Picker::default();
        p.discovered(found(1));
        p.select(0, false);
        p.start(Job::Trust);
        p.finished(Job::Trust, false, "ssh-keyscan: connection refused", false);
        assert!(matches!(p.phase, Phase::Failed(_)));
        assert!(p.status_line().contains("connection refused"));
        // The operator should see what went wrong, not a live button.
        assert!(!p.can_trust() && !p.can_setup_key());
    }

    #[test]
    fn an_empty_scan_says_so_rather_than_looking_idle() {
        let mut p = Picker::default();
        p.discovering();
        assert!(p.busy());
        p.discovered(vec![]);
        assert_eq!(p.status_line(), "nothing answered on this subnet");
        assert!(!p.busy());
    }

    #[test]
    fn trusting_then_installing_walks_the_whole_enrolment() {
        // The sequence the fleet planner's Step::Enroll needs a surface for.
        let mut p = Picker::default();
        p.discovering();
        p.discovered(found(2));
        p.select(1, false);
        assert!(p.can_trust());
        assert!(p.start(Job::Trust));
        p.finished(Job::Trust, true, "host key added", true);
        assert!(p.can_setup_key(), "trusted now, so the password may be sent");
        assert!(p.start(Job::SetupKey));
        p.finished(Job::SetupKey, true, "key installed", true);
        assert_eq!(p.phase, Phase::Selected { trusted: true });
        assert!(!p.busy());
    }
}
