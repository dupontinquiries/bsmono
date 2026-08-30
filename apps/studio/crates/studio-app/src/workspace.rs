//! workspace — the layout, saved simply, per device.
//!
//! Which panels are open, whether the workbench is on, what the side slot
//! shows, where the timeline is scrolled: none of that is a project fact
//! (a phone and a desktop editing ONE project want different layouts, the
//! way `solo` is monitoring state and not an op), so none of it belongs in
//! the `.studio` file or the archive. It lives in a sidecar the device
//! keeps beside the project — `<name>.workspace` — written on save, read
//! on open, and TOLERANT: an unknown key is a newer build's business, a
//! garbled file means defaults, and view state must never be the reason a
//! project fails to open. This is also the pattern for "both apps open on
//! one project": each frontend keeps its own sidecar over the shared
//! project file, and their states save simply because they never entangle.

use std::path::Path;

/// The state worth keeping across sessions. Everything has a default; a
/// missing key means "as before".
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceState {
    pub combined: bool,
    /// Panel and side-slot as their LABELS, not indices — a reordered enum
    /// in a future build must not silently retarget everyone's layout.
    pub panel: String,
    pub side_slot: String,
    pub px_per_sec: f32,
    pub scroll_ticks: i64,
    /// Compositions folded closed, as ids — per device, like everything
    /// else here: your fold is not your collaborator's fold.
    pub closed_groups: Vec<u64>,
    /// Library-bound render format token (see `morph_pad::BounceFormat`)
    /// — per device, because storage constraints are.
    pub bounce_format: String,
    /// Sample triage for THIS project: pinned floats to the top of its
    /// listing, demoted sinks. Stored here rather than in the op log
    /// because a pin is a person's working memory, not a fact about the
    /// music — a collaborator's triage must not rearrange yours mid-take.
    pub pinned: Vec<String>,
    pub demoted: Vec<String>,
    /// Genre vocabularies the voice fastpath listens with — per device,
    /// because the ears follow the room, not the song.
    pub voice_domains: String,
    /// Whether the library sidebar is showing — layout, so it lives here
    /// with `combined` and friends rather than being forgotten every
    /// launch (it was: the flag existed only in the shell).
    pub library_open: bool,
    /// Pressed→effective key translations, in `rebinds::Rebinds`' own
    /// codepoint encoding. A string here rather than a parsed type so the
    /// sidecar stays a dumb tolerant format; the shell parses on load.
    pub rebinds: String,
    /// What presence packets call this device's person. Per DEVICE on
    /// purpose — the same person on a phone and a desktop may want
    /// "kes (phone)" vs "kes", and the sidecar is exactly the file that
    /// distinction lives in. Empty means "site N", never an error.
    pub display_name: String,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        WorkspaceState { combined: false, panel: "Timeline".into(),
                         side_slot: "viewport".into(),
                         px_per_sec: 100.0, scroll_ticks: 0,
                         closed_groups: Vec::new(),
                         bounce_format: "opus-192".into(),
                         pinned: Vec::new(), demoted: Vec::new(),
                         voice_domains: "common,instruments".into(),
                         library_open: true, display_name: String::new(),
                         rebinds: String::new() }
    }
}

impl WorkspaceState {
    pub fn encode(&self) -> String {
        format!("# per-device layout; safe to delete, never load-bearing\n\
                 combined={}\npanel={}\nside_slot={}\npx_per_sec={}\nscroll_ticks={}\nclosed_groups={}\nbounce_format={}\npinned={}\ndemoted={}\nvoice_domains={}\nlibrary_open={}\ndisplay_name={}\nrebinds={}\n",
                self.combined, self.panel, self.side_slot,
                self.px_per_sec, self.scroll_ticks,
                self.closed_groups.iter().map(|g| g.to_string())
                    .collect::<Vec<_>>().join(","),
                self.bounce_format,
                // Tab-joined: a sample path may contain a comma (real
                // packs do), and losing a pin to punctuation would be a
                // silent, confusing failure.
                self.pinned.join("\t"),
                self.demoted.join("\t"),
                self.voice_domains, self.library_open, self.display_name,
                self.rebinds)
    }
    /// Parse leniently: defaults underneath, known keys on top, everything
    /// else ignored. There is no error case ON PURPOSE.
    pub fn decode(s: &str) -> WorkspaceState {
        let mut w = WorkspaceState::default();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "combined" => if let Ok(b) = v.trim().parse() { w.combined = b },
                "panel" => w.panel = v.trim().to_string(),
                "side_slot" => w.side_slot = v.trim().to_string(),
                "px_per_sec" => if let Ok(f) = v.trim().parse::<f32>() {
                    if f.is_finite() && f > 0.0 { w.px_per_sec = f; }
                },
                "scroll_ticks" => if let Ok(t) = v.trim().parse::<i64>() {
                    if t >= 0 { w.scroll_ticks = t; }
                },
                "closed_groups" => w.closed_groups = v.trim().split(',')
                    .filter_map(|x| x.trim().parse().ok()).collect(),
                "bounce_format" => w.bounce_format = v.trim().to_string(),
                "pinned" => w.pinned = v.split('\t')
                    .filter(|x| !x.trim().is_empty()).map(str::to_string).collect(),
                "demoted" => w.demoted = v.split('\t')
                    .filter(|x| !x.trim().is_empty()).map(str::to_string).collect(),
                "voice_domains" => w.voice_domains = v.trim().to_string(),
                "library_open" => if let Ok(b) = v.trim().parse() { w.library_open = b },
                "display_name" => w.display_name = v.trim().to_string(),
                "rebinds" => w.rebinds = v.trim().to_string(),
                _ => {} // a newer build's key, not our business
            }
        }
        w
    }
}

/// A named arrangement of the layout primitives that already exist —
/// which panel is central, whether the workbench strip is on, what the
/// side slot shows, whether the library rides along. Expressed in the
/// sidecar's OWN vocabulary (panel labels, slot tokens), so a preset is
/// literally a partial `WorkspaceState` and can never say something the
/// sidecar cannot.
///
/// ## Why presets, not a dock or a dashboard
///
/// Three shapes were on the table for "the layout system":
///
/// * **Dashboarding** — every panel visible at once. The workbench
///   (`combined`) already IS the desktop's version of this, scoped to
///   what fits; a full four-panel dashboard dies on a phone, and
///   `uikit_egui::responsive_nav` exists precisely because even a TAB
///   ROW doesn't reliably fit there.
/// * **Free workflowing** — user-arranged docking. A new dependency, a
///   giant persisted state space, and a seam for a future that may not
///   come; `ambient-learner`'s shell doc is the cautionary tale about
///   layouts whose states outnumber their routes in ("nine of its ten
///   pages had no route in").
/// * **Workspacing** — this: a handful of NAMED arrangements over the
///   primitives that already exist and already persist. kdenlive ships
///   exactly this (Logging / Editing / Audio / Color); it is one
///   keypress to switch and zero new state to store, because a preset
///   only ever writes fields the sidecar already owns.
///
/// A preset deliberately does NOT touch the camera (`px_per_sec`,
/// `scroll_ticks`) or triage (`pinned`/`demoted`): switching how the
/// room is arranged must not move where you were looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub name: &'static str,
    /// `Panel` label, the same string `WorkspaceState.panel` stores.
    pub panel: &'static str,
    pub combined: bool,
    /// Side-slot token, the same vocabulary the sidecar stores.
    pub side_slot: &'static str,
    pub library_open: bool,
}

/// The four rooms. Order is the keybinding (`C-1`..`C-4`).
pub const LAYOUTS: [Layout; 4] = [
    // Arrange & trim: the kdenlive posture. Timeline central, media
    // library at hand, viewport beside it when there is video.
    Layout { name: "edit", panel: "Timeline", combined: false,
             side_slot: "viewport", library_open: true },
    // Beatmaking: the FL posture. The rack full-screen, the sample
    // library open — the two things a beat is made of.
    Layout { name: "beat", panel: "Rack", combined: false,
             side_slot: "off", library_open: true },
    // Mixing: faders full-screen, nothing else pulling the eye.
    Layout { name: "mix", panel: "Mixer", combined: false,
             side_slot: "off", library_open: false },
    // Performing/producing: the workbench — timeline on top, rack in
    // the strip below, mixer one slot away. The "shown together" room.
    Layout { name: "perform", panel: "Timeline", combined: true,
             side_slot: "mixer", library_open: false },
];

impl Layout {
    /// Write this preset's fields over a state, leaving everything a
    /// preset has no opinion on (camera, triage, formats, name) alone.
    pub fn apply(&self, w: &mut WorkspaceState) {
        w.panel = self.panel.to_string();
        w.combined = self.combined;
        w.side_slot = self.side_slot.to_string();
        w.library_open = self.library_open;
    }
}

/// The sidecar path for a project file: `project.studio` → `project.workspace`.
pub fn sidecar(project: &Path) -> std::path::PathBuf {
    project.with_extension("workspace")
}

pub fn save(project: &Path, w: &WorkspaceState) {
    // Best-effort by design: a read-only directory must not fail a save.
    let _ = std::fs::write(sidecar(project), w.encode());
}

pub fn load(project: &Path) -> WorkspaceState {
    std::fs::read_to_string(sidecar(project))
        .map(|s| WorkspaceState::decode(&s))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_round_trips_and_garbage_means_defaults_never_failure() {
        let w = WorkspaceState { combined: true, panel: "Rack".into(),
            side_slot: "mixer".into(), px_per_sec: 250.0, scroll_ticks: 12345,
            closed_groups: vec![3, 9], bounce_format: "flac".into(),
            pinned: vec!["kicks/808, big.wav".into()], demoted: vec!["hats/tick.opus".into()],
            voice_domains: "trap,film".into(),
            library_open: false, display_name: "kes".into(),
            rebinds: "44>46,107>100".into() };
        assert_eq!(WorkspaceState::decode(&w.encode()), w);
        // A newer build's key rides along harmlessly; garbage lines too.
        let with_future = format!("{}\nfuture_key=whatever\n<<<garbage", w.encode());
        assert_eq!(WorkspaceState::decode(&with_future), w);
        assert_eq!(WorkspaceState::decode("total nonsense"), WorkspaceState::default(),
                   "view state is never the reason a project fails to open");
        // Hostile numbers clamp to sanity rather than propagating.
        let bad = "px_per_sec=-4\nscroll_ticks=-9\npx_per_sec=NaN";
        assert_eq!(WorkspaceState::decode(bad), WorkspaceState::default());
    }

    #[test]
    fn the_sidecar_sits_beside_the_project_it_describes() {
        assert_eq!(sidecar(Path::new("beats/cut.studio")),
                   Path::new("beats/cut.workspace"));
        assert_eq!(sidecar(Path::new("cut.studioarch")),
                   Path::new("cut.workspace"),
                   "an archive's layout sidecar shares the stem too");
    }

    /// A preset is a partial state in the sidecar's own vocabulary — so
    /// every panel label and slot token it names must be one the decode
    /// path and the shell's lookup actually recognise, and applying one
    /// must leave the camera exactly where it was.
    #[test]
    fn presets_speak_the_sidecar_vocabulary_and_leave_the_camera_alone() {
        let panels: Vec<&str> = studio_egui::touch::Panel::ALL.iter()
            .map(|p| p.label()).collect();
        let slots = ["off", "viewport", "rack", "program", "mixer"];
        for l in LAYOUTS {
            assert!(panels.contains(&l.panel),
                    "{}: panel {:?} is not a real Panel label", l.name, l.panel);
            assert!(slots.contains(&l.side_slot),
                    "{}: slot {:?} is not a token the shell maps", l.name, l.side_slot);
        }
        let mut w = WorkspaceState::default();
        w.px_per_sec = 37.5;
        w.scroll_ticks = 9_000;
        w.display_name = "kes".into();
        LAYOUTS[3].apply(&mut w);
        assert_eq!(w.panel, "Timeline");
        assert!(w.combined, "perform is the workbench room");
        assert_eq!((w.px_per_sec, w.scroll_ticks), (37.5, 9_000),
                   "switching rooms must not move the camera");
        assert_eq!(w.display_name, "kes", "or rename anyone");
    }

    /// The two new keys ride the same tolerant format: encoded, decoded,
    /// and absent-means-default (an old sidecar written before they
    /// existed must load with the library open and no name).
    #[test]
    fn library_and_name_round_trip_and_default() {
        let mut w = WorkspaceState::default();
        w.library_open = false;
        w.display_name = "kes (phone)".into();
        let back = WorkspaceState::decode(&w.encode());
        assert_eq!(back, w);
        let old = WorkspaceState::decode("combined=true\npanel=Mixer\n");
        assert!(old.library_open, "missing key means the old default");
        assert!(old.display_name.is_empty());
    }
}
