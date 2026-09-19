//! dep_audit — which runtime tools are here, which are not, and what breaks.
//!
//! `SettingsPage.qml`'s RUNTIME DEPENDENCIES card, ported as a model. The QML
//! reads `qtobj.dep_audit_json` and renders `bin`, `purpose` and a status
//! badge; this is that, minus the JSON.
//!
//! ## Why the frontend cannot just call the real thing
//!
//! `bo-controller` already has the machinery — `dep_wrangler::detect`,
//! `verify_installed`, `plan_for_tool` — and it is a BINARY crate, so nothing
//! can depend on it. That is why the card was never ported: the obvious
//! implementation is unavailable and the workaround (shelling out from a draw
//! path) is worse than none.
//!
//! So the audit arrives as data, exactly as the QML receives it, and this
//! module owns the part with decisions in it: what the states mean, what an
//! unrecognised one does, and what the card says when nothing has answered
//! yet.
//!
//! ## Unknown is MISSING, not a fourth state
//!
//! The QML's ladder ends `: "MISSING"` — anything that is not `present` or
//! `broken` reads as missing. Carried deliberately rather than adding an
//! `Unknown`: a dependency whose status nobody can establish is one you should
//! go and check, and showing it as its own greyed-out state invites reading it
//! as fine.
//!
//! Not-yet-audited is different and IS its own state, because "we have not
//! looked" and "we looked and it is gone" are different sentences and only one
//! of them is a problem.

/// What the audit found for one tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// On the system and usable.
    Present,
    /// On the system and not usable — wrong version, missing codec, refuses
    /// to run. Distinct from missing because the fix is different: installing
    /// it again will not help.
    Broken,
    /// Not found, or reported as something this build does not recognise.
    Missing,
}

impl Status {
    /// Parse the daemon's string. The QML's ladder, including its fallback.
    pub fn parse(s: &str) -> Status {
        match s {
            "present" => Status::Present,
            "broken" => Status::Broken,
            _ => Status::Missing,
        }
    }

    /// The badge text, verbatim from the QML.
    pub fn badge(self) -> &'static str {
        match self {
            Status::Present => "INSTALLED",
            Status::Broken => "BROKEN",
            Status::Missing => "MISSING",
        }
    }

    /// Does this need the operator to do something?
    pub fn needs_attention(self) -> bool {
        !matches!(self, Status::Present)
    }
}

/// One audited tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// The binary name, shown monospaced.
    pub bin: String,
    /// What it is for, in the operator's terms — "album art", not "libjpeg".
    pub purpose: String,
    pub status: Status,
}

/// The card's whole state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audit {
    /// `None` until something answers. See [`Audit::summary`].
    deps: Option<Vec<Dep>>,
    /// The package manager, as detected. Empty means not established.
    pub manager: String,
}

impl Audit {
    pub fn new() -> Audit {
        Audit::default()
    }

    /// Replace the audit. Ordered worst-first — see [`Audit::rows`].
    pub fn set(&mut self, deps: Vec<Dep>) {
        self.deps = Some(deps);
    }

    pub fn is_answered(&self) -> bool {
        self.deps.is_some()
    }

    /// Rows to draw, WORST FIRST.
    ///
    /// Broken above missing above present, and alphabetical within each. A
    /// card sorted by name puts the one thing that is wrong in the middle of
    /// twenty that are fine, which is the layout most likely to be scrolled
    /// past — and this card exists to be scanned when something is not
    /// working.
    ///
    /// Broken outranks missing because it is the more surprising failure: a
    /// missing tool explains itself, while a present-but-broken one is what
    /// someone stares at a working install wondering about.
    pub fn rows(&self) -> Vec<&Dep> {
        let Some(deps) = &self.deps else { return Vec::new() };
        let mut v: Vec<&Dep> = deps.iter().collect();
        v.sort_by_key(|d| {
            let rank = match d.status {
                Status::Broken => 0,
                Status::Missing => 1,
                Status::Present => 2,
            };
            (rank, d.bin.clone())
        });
        v
    }

    /// The one-line summary above the list.
    ///
    /// The three cases are genuinely different and a UI that collapses them
    /// lies in at least one:
    ///
    /// * not audited yet — say so, do not imply health
    /// * audited, nothing wrong — say so, so the card is worth having looked at
    /// * audited, something wrong — say how much, so it can be counted at a
    ///   glance
    pub fn summary(&self) -> String {
        let Some(deps) = &self.deps else {
            return "Not audited yet".to_string();
        };
        if deps.is_empty() {
            return "No runtime dependencies declared".to_string();
        }
        let bad = deps.iter().filter(|d| d.status.needs_attention()).count();
        if bad == 0 {
            format!("All {} present", deps.len())
        } else {
            format!("{bad} of {} need attention", deps.len())
        }
    }

    /// "Detected: <manager>", with the QML's own fallback word.
    pub fn manager_label(&self) -> String {
        let m = self.manager.trim();
        format!("Detected: {}", if m.is_empty() { "unknown" } else { m })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(bin: &str, status: Status) -> Dep {
        Dep { bin: bin.into(), purpose: "x".into(), status }
    }

    /// The QML's ladder ends in MISSING, so an unrecognised status is missing
    /// rather than a fourth state: a dependency nobody can establish is one to
    /// go and check, and a greyed-out "unknown" invites reading it as fine.
    #[test]
    fn an_unrecognised_status_reads_as_missing() {
        assert_eq!(Status::parse("present"), Status::Present);
        assert_eq!(Status::parse("broken"), Status::Broken);
        assert_eq!(Status::parse("missing"), Status::Missing);
        assert_eq!(Status::parse("degraded"), Status::Missing);
        assert_eq!(Status::parse(""), Status::Missing);
        assert_eq!(Status::parse("PRESENT"), Status::Missing, "the daemon's casing is exact");
    }

    /// Badges are verbatim from the QML.
    #[test]
    fn the_badges_match_the_qml() {
        assert_eq!(Status::Present.badge(), "INSTALLED");
        assert_eq!(Status::Broken.badge(), "BROKEN");
        assert_eq!(Status::Missing.badge(), "MISSING");
    }

    /// WORST FIRST. A card sorted by name buries the one broken tool among
    /// twenty working ones, which is the layout most likely to be scrolled
    /// past — and this card is read when something is not working.
    #[test]
    fn the_broken_one_is_at_the_top() {
        let mut a = Audit::new();
        a.set(vec![
            dep("aaa", Status::Present),
            dep("zzz", Status::Broken),
            dep("mmm", Status::Missing),
        ]);
        let order: Vec<&str> = a.rows().iter().map(|d| d.bin.as_str()).collect();
        assert_eq!(order, vec!["zzz", "mmm", "aaa"]);
    }

    /// Broken outranks missing: a missing tool explains itself, a
    /// present-but-broken one is what someone stares at a working install
    /// wondering about.
    #[test]
    fn broken_outranks_missing() {
        let mut a = Audit::new();
        a.set(vec![dep("b", Status::Missing), dep("a", Status::Broken)]);
        assert_eq!(a.rows()[0].bin, "a");
    }

    /// Alphabetical within a rank, so the list does not reshuffle between
    /// audits that found the same thing.
    #[test]
    fn rows_are_stable_within_a_status() {
        let mut a = Audit::new();
        a.set(vec![
            dep("ffmpeg", Status::Missing),
            dep("alsa", Status::Missing),
            dep("pipewire", Status::Missing),
        ]);
        let order: Vec<&str> = a.rows().iter().map(|d| d.bin.as_str()).collect();
        assert_eq!(order, vec!["alsa", "ffmpeg", "pipewire"]);
    }

    /// NOT AUDITED IS NOT HEALTHY. "We have not looked" and "we looked and it
    /// is fine" are different sentences, and a card that shows the second when
    /// it means the first is lying at the exact moment someone is debugging.
    #[test]
    fn an_unanswered_audit_does_not_claim_health() {
        let a = Audit::new();
        assert!(!a.is_answered());
        assert_eq!(a.summary(), "Not audited yet");
        assert!(a.rows().is_empty());
    }

    #[test]
    fn a_clean_audit_says_so_rather_than_saying_nothing() {
        let mut a = Audit::new();
        a.set(vec![dep("ffmpeg", Status::Present), dep("alsa", Status::Present)]);
        assert_eq!(a.summary(), "All 2 present");
    }

    #[test]
    fn a_dirty_audit_counts_what_needs_attention() {
        let mut a = Audit::new();
        a.set(vec![
            dep("ffmpeg", Status::Present),
            dep("alsa", Status::Broken),
            dep("pw", Status::Missing),
        ]);
        assert_eq!(a.summary(), "2 of 3 need attention");
    }

    /// An audit that ran and found nothing declared is distinct from one that
    /// has not run.
    #[test]
    fn an_empty_audit_is_not_an_unanswered_one() {
        let mut a = Audit::new();
        a.set(Vec::new());
        assert!(a.is_answered());
        assert_eq!(a.summary(), "No runtime dependencies declared");
    }

    /// The QML's own fallback word for an undetected package manager.
    #[test]
    fn an_undetected_manager_reads_as_unknown() {
        let mut a = Audit::new();
        assert_eq!(a.manager_label(), "Detected: unknown");
        a.manager = "  ".into();
        assert_eq!(a.manager_label(), "Detected: unknown", "whitespace is not a manager");
        a.manager = "pacman".into();
        assert_eq!(a.manager_label(), "Detected: pacman");
    }
}
