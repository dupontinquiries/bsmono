//! playback_diag — when tapping a track does nothing, which layer is at fault.
//!
//! `SettingsPage.qml`'s PLAYBACK DIAGNOSTIC card, whose own help line is the
//! whole brief: *"When tapping a track does nothing, run this"*.
//!
//! The controller's `diagnose_playback` walks a ladder and reports where it
//! stopped. This is that ladder as a model — the steps, what each one proves,
//! and what a stop at each one means.
//!
//! ## The point is the LAYER, not the pass/fail
//!
//! "Playback is broken" is not actionable; "the server bound but its port is
//! blocked" is. A diagnostic that answered yes or no would be worth less than
//! the error the user already has, which is why every outcome here names a
//! layer and a next move.
//!
//! ## It deliberately does not play anything
//!
//! From the controller's own doc: *"This doesn't actually play anything on the
//! speaker — it just verifies the controller-side half of the pipeline is
//! healthy."*
//!
//! That boundary is worth keeping visible rather than quietly improving. A
//! green result means the controller's half works, and the fault is in the
//! speaker, the network between them, or the device's own player. Reporting
//! that as "playback OK" would send someone to look in the place that is
//! already known to be fine — so [`Outcome::ControllerHealthy`] says which
//! half passed, not that playback works.

/// One rung of the ladder, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// Bind the track server and get a port.
    BindServer,
    /// Expose a small in-memory test WAV — a 220 Hz tone for 200 ms.
    ExposeTestClip,
    /// Build the device-facing URL from the active device's IP.
    BuildUrl,
    /// GET that URL from the controller, proving the server is reachable.
    Reach,
}

impl Step {
    pub const ALL: [Step; 4] =
        [Step::BindServer, Step::ExposeTestClip, Step::BuildUrl, Step::Reach];

    /// What a person sees while it runs.
    pub fn label(self) -> &'static str {
        match self {
            Step::BindServer => "Binding track server",
            Step::ExposeTestClip => "Exposing test clip",
            Step::BuildUrl => "Building device URL",
            Step::Reach => "Checking the server responds",
        }
    }

    /// What failing HERE means, and what to do. The reason the ladder exists.
    pub fn on_failure(self) -> &'static str {
        match self {
            Step::BindServer =>
                "The controller could not open a port to serve audio from. \
                 Another process may hold it, or the system may not allow binding.",
            Step::ExposeTestClip =>
                "The server is up but would not accept a clip to serve. \
                 This is a fault in the controller itself, not your network.",
            Step::BuildUrl =>
                "No usable address for the selected device. Pick a device, \
                 or check that it reported an IP.",
            // The QML's own wording for this one: "BIND_OK but /health failed;
            // port may be blocked". It is the most common real failure and the
            // least guessable from the symptom, which is why it is last.
            Step::Reach =>
                "The server bound but does not answer on its port — a firewall \
                 or another process is likely blocking it.",
        }
    }
}

/// How far it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not run yet. Distinct from a pass, for the same reason `dep_audit`
    /// separates them: a card that looks green before anyone pressed the
    /// button is lying while someone debugs.
    NotRun,
    /// Stopped at this step.
    Failed { at: Step, detail: String },
    /// Every step passed.
    ///
    /// Named for what it proves rather than "OK". The controller's half is
    /// healthy; the speaker, the network, and the device's player are all
    /// still untested, and saying "playback works" would send someone to look
    /// where it is already known to be fine.
    ControllerHealthy { port: u16, url: String },
}

impl Outcome {
    /// The line the settings card shows.
    pub fn summary(&self) -> String {
        match self {
            Outcome::NotRun => "Not run".to_string(),
            Outcome::Failed { at, .. } => format!("Failed: {}", at.label().to_lowercase()),
            Outcome::ControllerHealthy { port, .. } =>
                format!("Controller side OK (port {port})"),
        }
    }

    /// The paragraph under it. Empty before a run — an explanation of nothing
    /// is noise.
    pub fn detail(&self) -> String {
        match self {
            Outcome::NotRun => String::new(),
            Outcome::Failed { at, detail } if detail.is_empty() => at.on_failure().to_string(),
            // The concrete error goes FIRST: "connection refused" tells a
            // person more than the category does, and the guidance is context
            // for it rather than a replacement.
            Outcome::Failed { at, detail } => format!("{detail}\n\n{}", at.on_failure()),
            Outcome::ControllerHealthy { url, .. } => format!(
                "The controller can serve audio at {url}. If a track still will not \
                 play, the fault is past this point — the speaker, the network \
                 between them, or the device's own player."
            ),
        }
    }

    /// Steps that passed, for a checklist.
    ///
    /// A failure shows every rung BELOW it as passed, because they did — and
    /// seeing three greens and a red is what tells someone the problem is
    /// narrow rather than total.
    pub fn passed(&self) -> Vec<Step> {
        match self {
            Outcome::NotRun => Vec::new(),
            Outcome::Failed { at, .. } => {
                Step::ALL.iter().copied().take_while(|s| s < at).collect()
            }
            Outcome::ControllerHealthy { .. } => Step::ALL.to_vec(),
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, Outcome::Failed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NOT RUN IS NOT A PASS. A card that looks green before anyone pressed
    /// the button is lying at the moment someone is debugging.
    #[test]
    fn an_unrun_diagnostic_claims_nothing() {
        let o = Outcome::NotRun;
        assert_eq!(o.summary(), "Not run");
        assert!(o.detail().is_empty(), "an explanation of nothing is noise");
        assert!(o.passed().is_empty());
        assert!(!o.is_failure());
    }

    /// A FAILURE SHOWS THE RUNGS BELOW IT AS PASSED, because they did. Three
    /// greens and a red is what tells someone the problem is narrow.
    #[test]
    fn a_failure_reports_everything_that_worked_first() {
        let o = Outcome::Failed { at: Step::Reach, detail: String::new() };
        assert_eq!(o.passed(), vec![Step::BindServer, Step::ExposeTestClip, Step::BuildUrl]);
        let early = Outcome::Failed { at: Step::BindServer, detail: String::new() };
        assert!(early.passed().is_empty(), "nothing passed before the first rung");
    }

    /// Every failure names a layer and a next move -- a yes/no answer would be
    /// worth less than the error the user already has.
    #[test]
    fn every_step_says_what_to_do_about_it() {
        for s in Step::ALL {
            let t = s.on_failure();
            assert!(t.len() > 40, "{s:?} has no guidance");
            assert!(!t.contains("error"), "{s:?} restates the symptom: {t}");
        }
        assert!(Step::Reach.on_failure().contains("firewall"),
                "the most common real failure should name its most common cause");
    }

    /// THE CONCRETE ERROR COMES FIRST. "connection refused" tells a person
    /// more than the category, and the guidance is context for it.
    #[test]
    fn a_real_error_is_shown_above_the_guidance() {
        let o = Outcome::Failed {
            at: Step::Reach,
            detail: "connection refused (os error 111)".into(),
        };
        let d = o.detail();
        assert!(d.starts_with("connection refused"), "{d}");
        assert!(d.contains("firewall"), "and keeps the guidance: {d}");
    }

    /// With no concrete error there is still guidance rather than a blank.
    #[test]
    fn a_bare_failure_still_explains_itself() {
        let o = Outcome::Failed { at: Step::BuildUrl, detail: String::new() };
        assert_eq!(o.detail(), Step::BuildUrl.on_failure());
    }

    /// SUCCESS IS NAMED FOR WHAT IT PROVES. The controller's half is healthy;
    /// saying "playback works" would send someone to look where it is already
    /// known to be fine.
    #[test]
    fn success_does_not_claim_the_speaker_works() {
        let o = Outcome::ControllerHealthy {
            port: 8080,
            url: "http://10.0.0.5:8080/t.wav".into(),
        };
        assert!(o.summary().contains("Controller side OK"), "{}", o.summary());
        assert!(!o.summary().to_lowercase().contains("playback ok"));
        let d = o.detail();
        assert!(d.contains("the speaker") && d.contains("network"),
                "it must point past itself: {d}");
        assert_eq!(o.passed().len(), Step::ALL.len());
    }

    /// The ladder's order is the diagnostic. Reordering it would make a
    /// failure report the wrong layer as passed.
    #[test]
    fn the_steps_are_ordered_cheapest_and_most_local_first() {
        assert!(Step::BindServer < Step::ExposeTestClip);
        assert!(Step::ExposeTestClip < Step::BuildUrl);
        assert!(Step::BuildUrl < Step::Reach);
    }
}
