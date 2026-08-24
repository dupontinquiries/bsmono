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
}

impl Default for WorkspaceState {
    fn default() -> Self {
        WorkspaceState { combined: false, panel: "Timeline".into(),
                         side_slot: "viewport".into(),
                         px_per_sec: 100.0, scroll_ticks: 0,
                         closed_groups: Vec::new(),
                         bounce_format: "opus-192".into(),
                         pinned: Vec::new(), demoted: Vec::new(),
                         voice_domains: "common,instruments".into() }
    }
}

impl WorkspaceState {
    pub fn encode(&self) -> String {
        format!("# per-device layout; safe to delete, never load-bearing\n\
                 combined={}\npanel={}\nside_slot={}\npx_per_sec={}\nscroll_ticks={}\nclosed_groups={}\nbounce_format={}\npinned={}\ndemoted={}\nvoice_domains={}\n",
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
                self.voice_domains)
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
                _ => {} // a newer build's key, not our business
            }
        }
        w
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
            voice_domains: "trap,film".into() };
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
}
