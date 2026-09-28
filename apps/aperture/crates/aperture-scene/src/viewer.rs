//! viewer — the headless brain of aperture-view, same discipline as the
//! editor's state.rs: no egui, no GL, time and pixels as parameters, so every
//! behavior here is unit-tested and the app layer only translates and draws.
//!
//! The page is the contract with the renderer: whenever the visible window of
//! lines changes (scroll settled on a new top line, file loaded, theme
//! flipped), [`Viewer::ensure_page`] re-rasterizes through `scene_kit::inscribe`
//! and bumps `version` — the GL side re-uploads only on that bump. Scroll
//! *animation* is free: the page holds one extra row above and below and the
//! quad's UV window slides between integer lines, so the camera's smoothness
//! never forces a raster.

use aperture_core::syntax::{Language, Syntax, TokKind};
use aperture_core::gitspace::{self, GitRunner, Rev, Runner as _};
use aperture_complete::{engine::Engine, providers::CommandProvider, CompletionRequest};
use crate::rooms::{self, RoomEnv, Station};
use aperture_core::Buffer;
use aperture_egui::camera::Smooth;
use scene_kit::inscribe::{self, StyledLine};
use std::path::PathBuf;
use std::sync::Arc;

pub const COLS: usize = 92;
pub const ROWS: usize = 40;
pub const SCALE: usize = 2;
/// Rasterized rows = visible ROWS + one spare above and below for the UV slide.
pub const RASTER_ROWS: usize = ROWS + 2;

/// Ink on parchment: the sheet is tinted by the mesh color; these are glyph
/// colors chosen to read on it. Keyword amber, string moss, comment faded,
/// number gold, attr rust — the engraved cousin of the editor's palette.
pub fn span_color(kind: TokKind) -> [u8; 3] {
    match kind {
        TokKind::Keyword => [156, 84, 18],
        TokKind::Str => [58, 104, 44],
        TokKind::Comment => [128, 116, 96],
        TokKind::Number => [146, 108, 22],
        TokKind::Attr => [122, 70, 34],
        TokKind::Fn => [104, 56, 92],
        TokKind::Type => [60, 88, 116],
        TokKind::Plain => [44, 36, 26],
    }
}
pub const INK: [u8; 3] = [44, 36, 26];

pub struct PageOut {
    pub size: [i32; 2],
    pub rgba: Arc<Vec<u8>>,
    pub version: u64,
}

pub struct Viewer {
    pub buffer: Buffer,
    pub syntax: Syntax,
    pub lang: Language,
    pub path: Option<PathBuf>,

    /// Top visible line, animated. The *raster* window follows its rounded
    /// target; the fractional part becomes a UV offset on the quad.
    pub top: Smooth,
    /// Orbit camera around the lectern.
    pub yaw: Smooth,
    pub pitch: Smooth,
    pub dist: Smooth,

    raster_top: Option<usize>,
    page: Option<PageOut>,
    next_version: u64,

    /// World-space point + age (seconds) of the last touch; the shader's
    /// proximity glow. Age advances by frame delta (frame-rate independent,
    /// per physical-ui-findings), and expires after the response has faded.
    pub touch: Option<[f32; 4]>,

    // ── rooms ───────────────────────────────────────────────────────────────
    pub stations: Vec<Station>,
    pub focus: usize,
    /// Camera target flies between stations (the "jump").
    pub tx: Smooth,
    pub ty: Smooth,
    pub tz: Smooth,
    /// Room-mood crossfade: env_from → focused station's env as mix 0→1.
    env_from: RoomEnv,
    env_mix: Smooth,
    /// The station-plaque atlas (rasterized once per file).
    pub atlas: Option<PageOut>,
    /// One plaque row's height in atlas px (2 glyph cells).
    pub atlas_cell_px: f32,
    /// Set on jump; the app rebuilds the world and clears it.
    pub world_dirty: bool,

    // ── gitspace ────────────────────────────────────────────────────────────
    /// The repo behind the file, when there is one.
    git: Option<(GitRunner, String)>,
    /// The file's timeline: commits + gitmind checkpoints, newest first.
    pub revs: Vec<Rev>,
    /// None = the live worktree; Some(i) = viewing revs[i].
    pub rev_ix: Option<usize>,
    /// The worktree text, kept to come home to.
    worktree_text: String,
    /// Line ranges the shown revision changed (tinted in the raster).
    changed: Vec<(usize, usize)>,

    // ── the AI assistant's margin note ──────────────────────────────────────
    /// Same worker + same env contract as the editor (`APERTURE_CMD`): the
    /// newest-request-wins engine is EXACTLY right for scrubbing a timeline —
    /// flick through ten revisions and only the one you settle on gets
    /// described; the rest coalesce away unsent.
    describe: Option<Engine>,
    /// The note for the settled-on revision, shown in the chrome.
    pub rev_note: String,
    note_generation: u64,
    /// A transient result line (branch created/failed) with a countdown —
    /// the viewer has no now_ms clock like the editor's status chip, so this
    /// decays by `step`'s dt instead. Takes priority over `rev_note` in the
    /// chrome while it's alive; they're never both about the same thing.
    pub branch_msg: Option<(String, f32)>,

    // ── homecoming: personal re-entry, not project history ─────────────────
    /// Where you left off, if you ever marked it (`refs/aperture/visited/*`).
    pub visited_oid: Option<String>,
    /// Lines changed between `visited_oid` and NOW (worktree included) —
    /// the touched-signal a homecoming uses instead of "what the newest
    /// commit changed": a returning person cares about the whole gap, not
    /// the single most recent step of it.
    since_visited_changed: Vec<(usize, usize)>,
    /// `visited_oid`'s position in `self.revs` (newest-first), if it's
    /// within the fetched window — the doorway into winding forward.
    pub homecoming_start_ix: Option<usize>,

    // ── the way back (aperture_core::nav) ────────────────────────────────────
    /// Station clicks and graph-node jumps, as edges — the same `NavPath` the
    /// flat editor keeps, with the same entity naming (outline kind + name),
    /// so a path means the same thing in both shells. ←/→ walk it.
    pub nav: aperture_core::nav::NavPath,
}

/// Plaque text width — chosen WITH the plaque's world aspect: 14 columns over
/// two glyph rows ≈ the 9.5×1.5→ taller plaque's aspect, so glyphs stay
/// square-ish instead of engraving squeezed noodles.
pub const ATLAS_COLS: usize = 14;

impl Viewer {
    pub fn from_text(text: &str, lang: Language, path: Option<PathBuf>) -> Self {
        let mut out = Viewer {
            buffer: Buffer::from_str(text),
            syntax: Syntax::new(lang),
            lang,
            path,
            top: Smooth::new(0.0, 10.0),
            yaw: Smooth::new(0.12, 6.0),
            pitch: Smooth::new(-0.34, 6.0),
            dist: Smooth::new(30.0, 8.0),
            raster_top: None,
            page: None,
            next_version: 1,
            touch: None,
            stations: Vec::new(),
            focus: 0,
            tx: Smooth::new(0.0, 5.0),
            ty: Smooth::new(8.6, 5.0),
            tz: Smooth::new(0.0, 5.0),
            env_from: rooms::BASE_ENV,
            env_mix: Smooth::new(1.0, 2.2),
            atlas: None,
            atlas_cell_px: (scene_kit::inscribe::GLYPH * SCALE * 2) as f32,
            world_dirty: false,
            git: None,
            revs: Vec::new(),
            rev_ix: None,
            worktree_text: text.to_string(),
            changed: Vec::new(),
            describe: std::env::var("APERTURE_CMD").ok().map(|cmd| {
                let argv = cmd.split_whitespace().map(str::to_string).collect();
                Engine::new(Box::new(CommandProvider::new(argv)))
            }),
            rev_note: String::new(),
            note_generation: 0,
            branch_msg: None,
            visited_oid: None,
            since_visited_changed: Vec::new(),
            homecoming_start_ix: None,
            nav: Default::default(),
        };
        out.git = out.path.as_deref().and_then(gitspace::discover);
        if let Some((r, rel)) = &out.git {
            out.revs = gitspace::history(r, rel, 48);
            // The live file wears what's changed since the last commit.
            if let Some(base) = out.revs.iter().find(|v| v.kind == gitspace::RevKind::Commit) {
                out.changed = gitspace::worktree_changed(r, &base.oid, rel);
            }
            out.visited_oid = gitspace::last_visited(r, rel);
            if let Some(visited) = &out.visited_oid {
                out.since_visited_changed = gitspace::worktree_changed(r, visited, rel);
                out.homecoming_start_ix = out.revs.iter().position(|v| &v.oid == visited);
                // A marker exists and points at something DIFFERENT from
                // where the file stands now: touched should mean "changed
                // since I was last here," a strictly more useful signal on
                // return than "what the single newest commit did."
                if !out.since_visited_changed.is_empty() {
                    out.changed = out.since_visited_changed.clone();
                }
            }
        }
        out.build_rooms();
        out
    }

    /// "I'm caught up" — mark the current HEAD as where you're leaving off.
    /// Overwrites any previous mark; there's only ever one "last visited"
    /// point for a file, the same way there's only one bookmark position in
    /// a book, not a history of every page you've ever opened it to.
    pub fn mark_visited(&mut self) {
        let msg = match &self.git {
            Some((r, rel)) => match gitspace::head_oid(r) {
                Some(head) => match gitspace::mark_visited(r, rel, &head) {
                    Ok(()) => {
                        self.visited_oid = Some(head);
                        self.since_visited_changed.clear();
                        "marked — see you next time".to_string()
                    }
                    Err(e) => format!("mark failed: {}", e.trim().lines().next().unwrap_or(&e)),
                },
                None => "mark: no commits yet".to_string(),
            },
            None => "mark: not a git repo".to_string(),
        };
        self.branch_msg = Some((msg, 3.0));
    }

    /// Step into the gap: jump the revision preview to right where you left
    /// off. From there, `rev_step(-1)` (the existing "newer" direction) IS
    /// winding forward — every room that changed while you were away lights
    /// up as you walk it, one step of history at a time, until stepping
    /// forward from the newest fetched revision comes home to the live
    /// worktree, the same "arrived at now" convention `rev_step` already has.
    pub fn enter_homecoming(&mut self) {
        let Some(ix) = self.homecoming_start_ix else { return };
        self.jump_to_rev(ix);
    }

    /// Jump the revision preview directly to `self.revs[ix]` — the piece
    /// `rev_step` doesn't expose on its own (it only steps by ±1 from
    /// wherever it currently is).
    /// Fly the camera to frame an arbitrary point cluster — `centroid` +
    /// `bounding_radius` + `fit_distance`, logscope's own vocabulary for
    /// "look at this," reused here for the commit graph specifically
    /// (`G` in the app layer). `tx/ty/tz` become the orbit target (the
    /// SAME fields `jump` already retargets for stations, so this camera
    /// move glides exactly like every other one in the corridor); `dist`
    /// is the orbit distance, pulled back far enough that
    /// `bounding_radius` says the whole cluster fits.
    pub fn frame_points(&mut self, points: &[scene_kit::spatial::Vec3]) {
        if points.is_empty() {
            return;
        }
        let c = scene_kit::spatial::centroid(points);
        let r = scene_kit::spatial::bounding_radius(points, c);
        self.tx.target = c.x;
        self.ty.target = c.y;
        self.tz.target = c.z;
        self.dist.target = scene_kit::spatial::fit_distance(r);
    }

    /// Jump the revision preview directly to `revs[ix]` — the click-a-
    /// graph-node entry point. Public because the app layer needs to reach
    /// it from a screen click; `jump_to_rev` stays the shared internal
    /// engine both this and `enter_homecoming` walk through.
    pub fn jump_to_graph_node(&mut self, ix: usize) {
        let ix = ix.min(self.revs.len().saturating_sub(1));
        let (from, fsite) = self.nav_rev_here();
        self.jump_to_rev(ix);
        let (to, tsite) = self.nav_rev_here();
        self.nav.push(aperture_core::nav::NavStep::new(from, aperture_core::entity::Relation::TouchedBy, to).at(fsite).landed(tsite));
    }

    /// A station jump someone ASKED for (a plaque click), recorded so ←
    /// comes back. `[`/`]` walk the corridor in order and are not recorded:
    /// the order itself is the way back.
    pub fn jump_recorded(&mut self, to: usize) {
        let from = self.nav_station_here();
        self.jump(to);
        let landed = self.nav_station_here();
        if let (Some((f, fs)), Some((t, ts))) = (from, landed) {
            self.nav.push(aperture_core::nav::NavStep::new(f, aperture_core::entity::Relation::Opened, t).at(fs).landed(ts));
        }
    }

    /// The trail to light: stations the path has visited, in order, as
    /// indices into THIS corridor (by kind + name — a revision rebuilds the
    /// stations, and an item this version lacks simply drops out of the
    /// trail). Only the part behind the cursor: going back dims the steps
    /// you have undone.
    pub fn trail_stations(&self) -> Vec<usize> {
        use aperture_core::entity::EntityId;
        let find = |id: &EntityId| match id {
            EntityId::Item { kind, name, .. } => self
                .stations
                .iter()
                .position(|s| &s.item.name == name && &format!("{:?}", s.item.kind).to_lowercase() == kind),
            _ => None,
        };
        let mut out: Vec<usize> = Vec::new();
        for step in self.nav.trail() {
            for id in [&step.from, &step.to] {
                if let Some(i) = find(id) {
                    if out.last() != Some(&i) {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    fn nav_rel_path(&self) -> String {
        match (&self.git, &self.path) {
            (Some((_, rel)), _) => rel.clone(),
            (None, Some(p)) => p.to_string_lossy().into_owned(),
            (None, None) => "[scratch]".into(),
        }
    }

    /// The focused station as an entity — named exactly as the flat
    /// editor's `nav_here` names an item, so the two agree on identity.
    fn nav_station_here(&self) -> Option<(aperture_core::entity::EntityId, aperture_core::entity::Site)> {
        let st = self.stations.get(self.focus)?;
        let rel = self.nav_rel_path();
        let id = aperture_core::entity::EntityId::item(rel.clone(), format!("{:?}", st.item.kind).to_lowercase(), st.item.name.clone());
        Some((id, aperture_core::entity::Site::new(rel, st.item.line as u32, 0)))
    }

    /// Which version of the file is showing: a commit, or the live file.
    fn nav_rev_here(&self) -> (aperture_core::entity::EntityId, aperture_core::entity::Site) {
        let rel = self.nav_rel_path();
        let site = aperture_core::entity::Site::new(rel.clone(), self.top.target.max(0.0) as u32, 0);
        match self.rev_ix.and_then(|i| self.revs.get(i)) {
            Some(r) => (aperture_core::entity::EntityId::Commit { oid: r.oid.clone() }, site),
            None => (aperture_core::entity::EntityId::file(rel), site),
        }
    }

    /// ← (`back`) / →: go to where a recorded jump started, or where it
    /// landed. A station is found by NAME in whatever revision is showing
    /// (the corridor is rebuilt per revision, so indices do not survive);
    /// one that does not exist in this version is said, not guessed at.
    pub fn nav_go(&mut self, back: bool) {
        let step = if back { self.nav.back().cloned() } else { self.nav.forward().cloned() };
        let Some(step) = step else {
            self.branch_msg = Some((if back { "nothing to go back to" } else { "nothing ahead" }.into(), 2.0));
            return;
        };
        let (target, site) = if back { (step.from.clone(), step.site.clone()) } else { (step.to.clone(), step.landing.clone()) };
        use aperture_core::entity::EntityId;
        let ok = match &target {
            EntityId::Item { kind, name, .. } => {
                match self.stations.iter().position(|s| &s.item.name == name && &format!("{:?}", s.item.kind).to_lowercase() == kind) {
                    Some(i) => {
                        self.jump(i);
                        true
                    }
                    None => {
                        self.branch_msg = Some((format!("{name} is not in this version"), 2.5));
                        false
                    }
                }
            }
            EntityId::Commit { oid } => match self.revs.iter().position(|r| &r.oid == oid) {
                Some(i) => {
                    self.jump_to_rev(i);
                    true
                }
                None => false,
            },
            EntityId::File { .. } => {
                // The live file.
                while self.rev_ix.is_some() {
                    let before = self.rev_ix;
                    self.rev_step(-1);
                    if self.rev_ix == before {
                        break;
                    }
                }
                true
            }
            _ => false,
        };
        if !ok {
            // Did not move: put the cursor back so the step is still there.
            if back { self.nav.forward(); } else { self.nav.back(); }
            return;
        }
        if matches!(target, EntityId::Commit { .. } | EntityId::File { .. }) {
            if let Some(s) = site {
                self.top.target = (s.line as f32).clamp(0.0, self.max_top());
            }
        }
    }

    fn jump_to_rev(&mut self, ix: usize) {
        let cur = self.rev_ix.map(|i| i as isize).unwrap_or(-1);
        let target = ix as isize;
        let mut guard = 0;
        while self.rev_ix.map(|i| i as isize).unwrap_or(-1) != target && guard < self.revs.len() + 2 {
            // Index 0 is newest; a HIGHER index is OLDER. rev_step's d=+1
            // means older (walks to a higher index), d=-1 means newer.
            let d = if target > cur { 1 } else { -1 };
            self.rev_step(d);
            guard += 1;
        }
    }

    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(&path)?;
        let lang = Language::from_path(&path);
        Ok(Self::from_text(&text, lang, Some(path)))
    }

    /// Outline → stations + the plaque atlas. Called once at load (and again
    /// if a future edit path wants live rooms — the pieces are all re-runnable).
    fn build_rooms(&mut self) {
        let outline = self.syntax.outline(&self.buffer);
        // Blame heat only for the LIVE worktree: `git blame` answers "who
        // touched what, as of now" — a historical revision would need
        // blame-at-rev (possible, deferred; stations there stay cold).
        let ages = match (&self.git, self.rev_ix) {
            (Some((r, rel)), None) => gitspace::line_ages(r, rel),
            _ => Vec::new(),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.stations = rooms::layout_full(outline, &ages, now, &self.changed);
        if self.stations.is_empty() {
            self.atlas = None;
            return;
        }
        let mut lines = Vec::with_capacity(self.stations.len() * 2);
        for st in &self.stations {
            let glyph = match st.item.kind {
                aperture_core::syntax::OutlineKind::Fn => "fn",
                aperture_core::syntax::OutlineKind::Struct => "st",
                aperture_core::syntax::OutlineKind::Enum => "en",
                aperture_core::syntax::OutlineKind::Trait => "tr",
                aperture_core::syntax::OutlineKind::Impl => "im",
                aperture_core::syntax::OutlineKind::Mod => "md",
                aperture_core::syntax::OutlineKind::Const => "cn",
            };
            let mut name = st.item.name.clone();
            if name.chars().count() > ATLAS_COLS {
                name = name.chars().take(ATLAS_COLS - 1).collect::<String>() + "…";
            }
            let mut top = StyledLine { text: name, spans: vec![] };
            top.spans.push((0..top.text.len(), [40, 30, 20]));
            let bottom = StyledLine {
                text: format!("{glyph} L{}", st.item.line + 1),
                spans: vec![(0..2, [110, 84, 46])],
            };
            lines.push(top);
            lines.push(bottom);
        }
        let page = inscribe::render_page(&lines, ATLAS_COLS, SCALE, [70, 58, 40]);
        self.atlas = Some(PageOut { size: [page.w, page.h], rgba: Arc::new(page.rgba), version: 1 });
        self.jump(0);
        self.env_mix.snap(1.0);
        // First room: arrive already there, no opening flight.
        self.tx.snap(self.tx.target);
        self.ty.snap(self.ty.target);
        self.tz.snap(self.tz.target);
        self.top.snap(self.top.target);
    }

    /// Fly to station `to`: lectern + camera target move, the page scrolls to
    /// the item (snapping past the same 120-line horizon the editor's follow
    /// camera uses — a thousand-line glide is a blur, not a journey), and the
    /// room's mood crossfades from wherever the air currently is.
    pub fn jump(&mut self, to: usize) {
        if self.stations.is_empty() {
            return;
        }
        let to = to.min(self.stations.len() - 1);
        self.env_from = self.current_env();
        self.env_mix.snap(0.0);
        self.env_mix.target = 1.0;
        self.focus = to;
        let st = &self.stations[to];
        // Same anchor formula as scene::build_rooms.
        self.tx.target = st.pos.x * 0.35;
        self.ty.target = 8.6;
        self.tz.target = st.pos.z + 4.5;
        let line = st.item.line as f32;
        if (line - self.top.target).abs() > 120.0 {
            self.top.snap(line.clamp(0.0, self.max_top()));
        } else {
            self.top.target = line.clamp(0.0, self.max_top());
        }
        self.world_dirty = true;
    }

    /// Step through the timeline: `d = +1` older, `-1` newer; stepping newer
    /// past the newest revision comes home to the live worktree. Swapping
    /// content REBUILDS the rooms — the corridor IS that version's file, so
    /// stations appear, move, and vanish across time. That's the feature,
    /// not a glitch: you watch the architecture grow.
    pub fn rev_step(&mut self, d: isize) {
        if self.revs.is_empty() {
            return;
        }
        let cur = self.rev_ix.map(|i| i as isize).unwrap_or(-1);
        let next = (cur + d).clamp(-1, self.revs.len() as isize - 1);
        if next == cur {
            return;
        }
        let (text, changed, ix) = if next < 0 {
            let ch = self
                .git
                .as_ref()
                .and_then(|(r, rel)| {
                    self.revs
                        .iter()
                        .find(|v| v.kind == gitspace::RevKind::Commit)
                        .map(|base| gitspace::worktree_changed(r, &base.oid, rel))
                })
                .unwrap_or_default();
            (self.worktree_text.clone(), ch, None)
        } else {
            let i = next as usize;
            let Some((r, rel)) = &self.git else { return };
            let Some(text) = gitspace::file_at(r, &self.revs[i].oid, rel) else {
                // The file didn't exist at this revision (or a checkpoint
                // predates it): step over it in the same direction.
                self.rev_ix = Some(i);
                return self.rev_step(d.signum());
            };
            let ch = gitspace::changed_ranges(r, &self.revs[i].oid, rel);
            (text, ch, Some(i))
        };
        self.rev_ix = ix;
        self.changed = changed;
        self.set_text(&text);
        self.request_describe();
    }

    /// Ask the assistant for a margin note on the shown revision: what it
    /// changed and why it might be the one you're looking for. Providers are
    /// the user's own (`APERTURE_CMD`); no provider, no note, zero cost.
    fn request_describe(&mut self) {
        let (Some(engine), Some(i)) = (&self.describe, self.rev_ix) else {
            self.rev_note.clear();
            return;
        };
        let rev = &self.revs[i];
        self.note_generation += 1;
        self.rev_note.clear();
        let stat = self
            .git
            .as_ref()
            .and_then(|(r, rel)| r.run(&["show", "--stat", "--format=%s", &rev.oid, "--", rel]).ok())
            .unwrap_or_default();
        let prompt = format!(
            "One sentence, plain words: what did this change do to this file, and when would someone want this version?\nCommit: {}\n{}\nAnswer:",
            rev.subject,
            stat.chars().take(600).collect::<String>()
        );
        engine.request(CompletionRequest {
            generation: self.note_generation,
            prefix: prompt,
            suffix: String::new(),
            lang: "text".into(),
            path: None,
        });
    }

    /// Replace the shown content (revision paging): new buffer, fresh parse,
    /// rebuilt rooms, raster invalidated, focus re-clamped.
    fn set_text(&mut self, text: &str) {
        self.buffer = Buffer::from_str(text);
        self.syntax = Syntax::new(self.lang);
        self.raster_top = None;
        self.page = None;
        self.top.snap(self.top.pos.min(self.max_top()));
        self.focus = 0;
        self.build_rooms();
        self.world_dirty = true;
    }

    /// Promote the revision currently on screen to a real branch — same
    /// operation, same guard, same message shape as the flat editor's
    /// `branch_from_here`: a no-op with a status line while live (never
    /// branch HEAD by accident), a real `git branch` while previewing.
    pub fn branch_from_here(&mut self) {
        let msg = match (&self.git, self.rev_ix) {
            (Some((r, _)), Some(i)) => {
                let rev = &self.revs[i];
                let name = gitspace::suggest_branch_name(rev);
                match gitspace::branch_from(r, &rev.oid, &name) {
                    Ok(()) => format!("branched '{name}' from {}", &rev.oid[..8]),
                    Err(e) => format!("branch failed: {}", e.trim().lines().next().unwrap_or(&e)),
                }
            }
            _ => "branch: view a revision first".to_string(),
        };
        self.branch_msg = Some((msg, 3.0));
    }

    /// gitmind's `materialize`, same guard shape as `branch_from_here` and
    /// the flat editor's `materialize_here`: check the previewed revision
    /// out into a fresh side-by-side worktree, a no-op message while live.
    pub fn materialize_here(&mut self) {
        let msg = match (&self.git, self.rev_ix) {
            (Some((r, _)), Some(i)) => {
                let rev = &self.revs[i];
                let dest = gitspace::suggest_worktree_path(&r.root, rev);
                match gitspace::materialize(r, &rev.oid, &dest.to_string_lossy()) {
                    Ok(()) => format!("materialized at {}", dest.display()),
                    Err(e) => format!("materialize failed: {}", e.trim().lines().next().unwrap_or(&e)),
                }
            }
            _ => "materialize: view a revision first".to_string(),
        };
        self.branch_msg = Some((msg, 3.0));
    }

    pub fn jump_rel(&mut self, d: isize) {
        if self.stations.is_empty() {
            return;
        }
        let n = self.stations.len() as isize;
        let to = (self.focus as isize + d).rem_euclid(n);
        self.jump(to as usize);
    }

    /// The air right now: the crossfade between where we were and the
    /// focused room.
    pub fn current_env(&self) -> RoomEnv {
        let to = self.stations.get(self.focus).map(|s| s.env).unwrap_or(rooms::BASE_ENV);
        rooms::lerp_env(&self.env_from, &to, self.env_mix.pos)
    }

    pub fn max_top(&self) -> f32 {
        (self.buffer.len_lines().saturating_sub(ROWS)) as f32
    }

    pub fn scroll_lines(&mut self, dy: f32) {
        self.top.target = (self.top.target + dy).clamp(0.0, self.max_top());
    }

    pub fn page_step(&mut self, dir: f32) {
        self.scroll_lines(dir * (ROWS as f32 - 2.0));
    }

    pub fn goto_top(&mut self) {
        self.top.target = 0.0;
    }
    pub fn goto_bottom(&mut self) {
        self.top.target = self.max_top();
    }

    /// The raster window starts one line above the (rounded) top target so the
    /// fractional scroll can slide within the texture.
    fn wanted_raster_top(&self) -> usize {
        (self.top.target.round() as usize).saturating_sub(1)
    }

    /// Fraction of one row that the *animated* top sits below the raster
    /// window's first row — the quad's V offset, in rows. In [0, RASTER_ROWS).
    pub fn uv_row_offset(&self) -> f32 {
        (self.top.pos - self.raster_top.unwrap_or(0) as f32).clamp(0.0, (RASTER_ROWS - ROWS) as f32 + 1.0)
    }

    /// Re-rasterize if the wanted window moved. Returns the current page.
    pub fn ensure_page(&mut self) -> &PageOut {
        let want = self.wanted_raster_top();
        if self.page.is_none() || self.raster_top != Some(want) {
            let mut lines = Vec::with_capacity(RASTER_ROWS);
            let n = self.buffer.len_lines();
            let (buffer, syntax, changed) = (&self.buffer, &mut self.syntax, &self.changed);
            for row in 0..RASTER_ROWS {
                let li = want + row;
                if li >= n {
                    lines.push(StyledLine::default());
                    continue;
                }
                let raw = buffer.line(li).into_owned();
                let text = raw.replace('\t', "    ");
                // Tab expansion shifts byte offsets; map spans through it.
                let spans: Vec<(std::ops::Range<usize>, [u8; 3])> = syntax
                    .spans_for(li, buffer)
                    .into_iter()
                    .map(|s| {
                        let shift = |b: usize| {
                            let tabs = raw[..b.min(raw.len())].matches('\t').count();
                            b + tabs * 3
                        };
                        (shift(s.range.start)..shift(s.range.end), span_color(s.kind))
                    })
                    .collect();
                let gutter = format!("{:>4} ", li + 1);
                let mut styled = StyledLine { text: format!("{gutter}{text}"), spans: vec![] };
                // Changed lines wear a green-inked gutter — diff as marginalia,
                // the way a proofreader marks a manuscript. (`changed` is
                // borrowed directly: `self` is split by the syntax borrow.)
                let is_changed = changed.iter().any(|(a, b)| li >= *a && li < *b);
                let gutter_ink = if is_changed { [64, 118, 52] } else { [150, 134, 108] };
                styled.spans.push((0..gutter.len(), gutter_ink));
                styled
                    .spans
                    .extend(spans.into_iter().map(|(r, c)| (r.start + gutter.len()..r.end + gutter.len(), c)));
                lines.push(styled);
            }
            let page = inscribe::render_page(&lines, COLS, SCALE, INK);
            self.page = Some(PageOut {
                size: [page.w, page.h],
                rgba: Arc::new(page.rgba),
                version: self.next_version,
            });
            self.next_version += 1;
            self.raster_top = Some(want);
        }
        self.page.as_ref().unwrap()
    }

    /// Advance all animation; true while anything still moves (repaint cue).
    /// Drain the describe engine (call once per frame, before/after step).
    pub fn poll_note(&mut self) {
        let Some(engine) = &self.describe else { return };
        while let Some(out) = engine.poll() {
            if out.generation == self.note_generation {
                if let Ok(text) = out.result {
                    // The margin note reads as a small legible sign in the
                    // chrome, not a full page — `textsurface`'s vocabulary
                    // for exactly that shape of thing is `StreetSign`
                    // ("flat, vertical, and already a text object in the
                    // world"), budgeted at 60 characters. That number isn't
                    // arbitrary the way a bare `.take(160)` was: it's the
                    // same legibility reasoning logscope already worked out
                    // for a small in-world sign, applied to a small
                    // in-chrome one — the SHAPE of the constraint (how much
                    // a person can read at a glance) doesn't care whether
                    // the text sits on a rendered object or drawn chrome.
                    let budget = scene_kit::textsurface::Surface::StreetSign.char_budget();
                    self.rev_note = text.trim().lines().next().unwrap_or("").chars().take(budget).collect();
                }
            }
        }
    }

    pub fn step(&mut self, dt: f32) -> bool {
        let moving = self.top.step(dt)
            | self.yaw.step(dt)
            | self.pitch.step(dt)
            | self.dist.step(dt)
            | self.tx.step(dt)
            | self.ty.step(dt)
            | self.tz.step(dt)
            | self.env_mix.step(dt);
        if let Some((_, t)) = &mut self.branch_msg {
            *t -= dt;
            if *t <= 0.0 {
                self.branch_msg = None;
            }
        }
        if let Some(t) = &mut self.touch {
            t[3] += dt;
            if t[3] > 4.0 {
                self.touch = None;
            }
        }
        moving || self.touch.is_some()
    }

    pub fn touch_at(&mut self, world: [f32; 3]) {
        self.touch = Some([world[0], world[1], world[2], 0.0]);
    }

    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw.target = (self.yaw.target + dyaw).clamp(-0.9, 0.9);
        self.pitch.target = (self.pitch.target + dpitch).clamp(-0.62, -0.08);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.dist.target = (self.dist.target * factor).clamp(16.0, 60.0);
    }

    /// The hour (0..24) this scene's sun should reflect — logscope's
    /// day-cycle theme, pulled in a second time but for a different axis
    /// than `RoomEnv`: kind-of-room (fn/struct/mod...) is orthogonal to
    /// time-of-day, so this drives ONLY the sun's angle, never fighting
    /// RoomEnv's already-tuned mood colors. Previewing a revision: the
    /// REAL hour that commit happened, so winding forward through history
    /// means walking through actual days and nights, dawn to dusk, in the
    /// order they really occurred — not a metaphor, the literal timestamp.
    /// Live: the wall clock (`scene_kit`'s own override contract, so
    /// `SCENE_KIT_HOUR`/`LOGSCOPE_HOUR` still works for anyone testing or
    /// deliberately pinning it).
    pub fn current_hour(&self) -> f32 {
        match self.rev_ix.and_then(|i| self.revs.get(i)) {
            Some(rev) => (rev.time.rem_euclid(86_400) as f32) / 3600.0,
            None => scene_kit::mesh3d::local_hour(),
        }
    }

    pub fn status_line(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[scratch]".into());
        let top = self.top.target as usize + 1;
        let bot = (top + ROWS - 1).min(self.buffer.len_lines());
        let rev = match self.rev_ix {
            None => String::new(),
            Some(i) => {
                let r = &self.revs[i];
                let mark = if matches!(r.kind, gitspace::RevKind::Checkpoint(_)) { "◈ " } else { "" };
                format!(
                    " · rev {}/{} {}{} {}",
                    i + 1,
                    self.revs.len(),
                    mark,
                    &r.oid[..r.oid.len().min(8)],
                    r.subject.chars().take(36).collect::<String>()
                )
            }
        };
        let room = self
            .stations
            .get(self.focus)
            .map(|s| format!(" · room {}/{} {}", self.focus + 1, self.stations.len(), s.item.name))
            .unwrap_or_default();
        // Welcome back: only while live (rev_ix is None) and a gap exists —
        // once H has been pressed and winding forward has begun, the rev
        // segment above already says where you are; this would be noise.
        let welcome = if self.rev_ix.is_none() {
            match self.homecoming_start_ix {
                Some(gap) if gap > 0 => {
                    let n = self.stations.iter().filter(|s| s.touched).count();
                    format!(
                        " · ⌂ {gap} rev{} since you were last here, {n} room{} changed — press H",
                        if gap == 1 { "" } else { "s" },
                        if n == 1 { "" } else { "s" }
                    )
                }
                _ => String::new(),
            }
        } else {
            String::new()
        };
        format!("{name} · lines {top}–{bot} of {}{room}{rev}{welcome} · {:?}", self.buffer.len_lines(), self.lang)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(lines: usize) -> Viewer {
        // No trailing newline: ropey counts one, and these tests reason in exact line counts.
        let text = (0..lines).map(|i| format!("let x{i} = {i}; // c")).collect::<Vec<_>>().join("\n");
        Viewer::from_text(&text, Language::Rust, None)
    }

    #[test]
    fn page_regenerates_only_when_the_window_moves() {
        let mut v = viewer(200);
        let v1 = v.ensure_page().version;
        assert_eq!(v.ensure_page().version, v1, "same window → same page");
        v.scroll_lines(0.4); // sub-line scroll: UV slides, raster stays
        assert_eq!(v.ensure_page().version, v1);
        v.scroll_lines(5.0);
        assert!(v.ensure_page().version > v1, "window moved → re-raster");
    }

    #[test]
    fn scroll_clamps_and_page_step_overlaps() {
        let mut v = viewer(50);
        v.scroll_lines(-10.0);
        assert_eq!(v.top.target, 0.0);
        v.goto_bottom();
        assert_eq!(v.top.target, (50 - ROWS) as f32);
        let mut long = viewer(200);
        long.page_step(1.0);
        assert_eq!(long.top.target, ROWS as f32 - 2.0, "one page keeps two lines of context");
        let mut short = viewer(5);
        short.goto_bottom();
        assert_eq!(short.top.target, 0.0, "short files never scroll");
    }

    #[test]
    fn page_carries_gutter_ink_and_syntax_color() {
        let mut v = viewer(60);
        let p = {
            let p = v.ensure_page();
            (p.size, p.rgba.clone())
        };
        assert_eq!(p.0[0] as usize, COLS * 8 * SCALE + 8 * SCALE * 2);
        let keyword = span_color(TokKind::Keyword);
        let has_kw = p.1.chunks(4).any(|px| px[3] == 255 && px[..3] == keyword);
        let has_gutter = p.1.chunks(4).any(|px| px[3] == 255 && px[..3] == [150, 134, 108]);
        assert!(has_kw, "`let` inked in keyword amber");
        assert!(has_gutter, "line numbers inked in gutter tone");
    }

    #[test]
    fn touch_ages_by_frame_delta_and_expires() {
        let mut v = viewer(10);
        v.touch_at([1.0, 2.0, 3.0]);
        for _ in 0..45 {
            v.step(0.1);
        }
        assert!(v.touch.is_none(), "4.5s of aging is past the 4s expiry");
        v.touch_at([0.0, 0.0, 0.0]);
        v.step(0.016);
        assert!((v.touch.unwrap()[3] - 0.016).abs() < 1e-6, "age is the frame delta, not wall clock");
    }

    #[test]
    fn uv_offset_tracks_the_animated_top() {
        let mut v = viewer(200);
        v.scroll_lines(10.0);
        v.ensure_page();
        for _ in 0..240 {
            v.step(1.0 / 60.0);
        }
        // settled: top.pos == 10, raster_top == 9 → offset == 1 row
        assert!((v.uv_row_offset() - 1.0).abs() < 0.05, "offset {}", v.uv_row_offset());
    }
    #[test]
    fn rooms_jump_moves_focus_scroll_env_and_flags_world() {
        let src = "fn alpha() {}\n".to_string()
            + &"// pad\n".repeat(200)
            + "mod gate;\nstruct Beta { x: u32 }\nfn omega() {}\n";
        let mut v = Viewer::from_text(&src, Language::Rust, None);
        assert!(v.stations.len() >= 4, "outline found: {}", v.stations.len());
        assert!(v.atlas.is_some(), "plaque atlas rasterized");
        assert_eq!(v.focus, 0);
        v.world_dirty = false;
        let env0 = v.current_env();
        let beta_ix = v.stations.iter().position(|s| s.item.name == "Beta").unwrap();
        v.jump(beta_ix);
        assert_eq!(v.focus, beta_ix);
        assert!(v.world_dirty, "the app must rebuild the corridor");
        // 200 lines away > the 120-line horizon: the page SNAPS — to the item
        // line where possible, to max_top when the item sits inside the last
        // page (the clamp is load-bearing: never scroll past the file's end).
        let line = v.stations[beta_ix].item.line as f32;
        assert_eq!(v.top.pos, line.min(v.max_top()), "snapped, clamped");
        assert!(line >= v.top.pos && line < v.top.pos + ROWS as f32, "the item is on the page");
        // The camera target is flying, not teleporting.
        assert!(v.tz.pos != v.tz.target, "flight in progress");
        // The mood is crossfading toward a DIFFERENT room.
        for _ in 0..120 { v.step(1.0 / 60.0); }
        let env1 = v.current_env();
        assert!(env0 != env1, "different room, different air");
        assert_eq!(env1, v.stations[beta_ix].env, "crossfade settles on the room");
    }

    #[test]
    fn rooms_atlas_rows_are_inked() {
        let v = Viewer::from_text("fn alpha() {}\nstruct Beta;\n", Language::Rust, None);
        let atlas = v.atlas.as_ref().unwrap();
        assert_eq!(atlas.size[0] as usize, (ATLAS_COLS * 8 * 2) + (8 * 2 * 2), "14 cols + margins");
        // Both stations' rows contain ink (non-transparent pixels).
        let cell = v.atlas_cell_px as usize;
        for row in 0..2 {
            let y0 = cell / 2 + row * cell;
            let stride = atlas.size[0] as usize * 4;
            let band = &atlas.rgba[y0 * stride..(y0 + cell).min(atlas.size[1] as usize) * stride];
            assert!(band.chunks(4).any(|p| p[3] > 0), "row {row} has ink");
        }
    }

    #[test]
    fn plain_text_has_no_rooms_and_jump_is_a_noop() {
        let mut v = Viewer::from_text("just words\n", Language::Plain, None);
        assert!(v.stations.is_empty());
        assert!(v.atlas.is_none());
        v.jump(3);
        v.jump_rel(1);
        assert_eq!(v.focus, 0);
        assert!(!v.world_dirty);
    }

    #[test]
    fn revisions_page_through_history_and_rebuild_the_rooms() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-viewer-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("m.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        std::fs::write(&f, "fn one() {}\nfn two() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "two"]);
        // Live worktree adds a third, uncommitted fn.
        let live = "fn one() {}\nfn two() {}\nfn three() {}\n";
        std::fs::write(&f, live).unwrap();

        let mut v = Viewer::from_text(live, Language::Rust, Some(f.clone()));
        assert_eq!(v.revs.len(), 2, "two commits found");
        assert_eq!(v.rev_ix, None, "starts at the worktree");
        assert_eq!(v.stations.len(), 3);
        assert!(v.changed.iter().any(|(a, b)| *a <= 2 && 2 < *b), "uncommitted fn three is marked");

        v.rev_step(1); // older: newest commit ("two")
        assert_eq!(v.rev_ix, Some(0));
        assert_eq!(v.stations.len(), 2, "the corridor IS that version's file");
        assert!(v.world_dirty);
        assert_eq!(v.changed, vec![(1, 2)], "what THIS revision changed");
        assert!(v.status_line().contains("rev 1/2"), "{}", v.status_line());

        v.rev_step(1); // older still: "one"
        assert_eq!(v.stations.len(), 1);
        v.rev_step(-1);
        v.rev_step(-1); // newer past newest: home
        assert_eq!(v.rev_ix, None);
        assert_eq!(v.buffer.to_string(), live, "the worktree text comes back exactly");
        assert_eq!(v.stations.len(), 3);
        assert!(v.rev_note.is_empty(), "no APERTURE_CMD, no note, zero cost");

        // Blame heat reached the stations (all fresh: committed seconds ago).
        assert!(v.stations.iter().take(2).all(|s| s.heat > 0.9), "fresh masonry glows");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn viewer_branch_from_here_guards_live_and_creates_while_previewing() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("v.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "worth keeping"]);

        let mut v = Viewer::from_text("fn one() {}\n", Language::Rust, Some(f));
        v.branch_from_here();
        assert!(v.branch_msg.as_ref().unwrap().0.contains("view a revision first"), "guarded while live");
        let listed = || {
            String::from_utf8(Command::new("git").arg("-C").arg(&dir).args(["branch", "--list"]).output().unwrap().stdout).unwrap()
        };
        assert!(!listed().contains("worth-keeping"));

        v.rev_step(1);
        v.branch_from_here();
        let (msg, remaining) = v.branch_msg.as_ref().unwrap();
        assert!(msg.starts_with("branched"), "{msg}");
        assert!(listed().contains("from/worth-keeping-"));
        assert!(*remaining > 0.0);

        // The message decays with step()'s dt and clears.
        for _ in 0..40 {
            v.step(0.1);
        }
        assert!(v.branch_msg.is_none(), "decayed after ~4s of stepping");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn viewer_materialize_here_guards_live_and_checks_out_while_previewing() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-mat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("mv.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "for the trip"]);

        let mut v = Viewer::from_text("fn one() {}\n", Language::Rust, Some(f));
        v.materialize_here();
        assert!(v.branch_msg.as_ref().unwrap().0.contains("view a revision first"), "guarded while live");

        v.rev_step(1);
        v.materialize_here();
        let (msg, _) = v.branch_msg.clone().unwrap();
        assert!(msg.starts_with("materialized at"), "{msg}");
        let path = msg.strip_prefix("materialized at ").unwrap();
        assert_eq!(std::fs::read_to_string(format!("{path}/mv.rs")).unwrap(), "fn one() {}\n");

        let _ = Command::new("git").arg("-C").arg(&dir).args(["worktree", "remove", "--force", path]).output();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn touched_stations_track_whatever_diff_is_on_screen() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-touched-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("t.rs");
        std::fs::write(&f, "fn alpha() {}\nfn beta() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        // Uncommitted: only beta's body changes.
        let live = "fn alpha() {}\nfn beta() { 1 }\n";
        std::fs::write(&f, live).unwrap();

        let v = Viewer::from_text(live, Language::Rust, Some(f));
        let alpha = v.stations.iter().find(|s| s.item.name == "alpha").unwrap();
        let beta = v.stations.iter().find(|s| s.item.name == "beta").unwrap();
        assert!(!alpha.touched, "alpha wasn't touched by the uncommitted edit");
        assert!(beta.touched, "beta was");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn homecoming_marks_recalls_and_winds_forward_through_the_gap() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-homecoming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("h.rs");
        std::fs::write(&f, "fn alpha() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "alpha only"]);

        // Mark visited HERE, at the very first commit.
        let mut v = Viewer::from_text("fn alpha() {}\n", Language::Rust, Some(f.clone()));
        v.mark_visited();
        assert!(v.branch_msg.as_ref().unwrap().0.starts_with("marked"));
        assert!(v.visited_oid.is_some());

        // Time passes: two more commits land while "away."
        std::fs::write(&f, "fn alpha() {}\nfn beta() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "beta arrives"]);
        std::fs::write(&f, "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "gamma arrives"]);

        // Reopen fresh, as if returning to the file after time away.
        let live = std::fs::read_to_string(&f).unwrap();
        let mut v2 = Viewer::from_text(&live, Language::Rust, Some(f.clone()));
        assert!(v2.visited_oid.is_some(), "the mark persisted (it's a git ref)");
        assert_eq!(v2.stations.len(), 3, "alpha, beta, gamma");
        let beta = v2.stations.iter().find(|s| s.item.name == "beta").unwrap();
        let gamma = v2.stations.iter().find(|s| s.item.name == "gamma").unwrap();
        let alpha = v2.stations.iter().find(|s| s.item.name == "alpha").unwrap();
        assert!(beta.touched, "beta arrived while away");
        assert!(gamma.touched, "so did gamma");
        assert!(!alpha.touched, "alpha existed before the mark — untouched");

        // Wind forward from the mark: first step lands on "beta arrives".
        assert!(v2.homecoming_start_ix.is_some());
        v2.enter_homecoming();
        assert!(v2.rev_ix.is_some(), "landed inside the gap, not live");
        let landed_subject = v2.revs[v2.rev_ix.unwrap()].subject.clone();
        assert_eq!(landed_subject, "alpha only", "starts AT the mark itself");

        v2.rev_step(-1); // wind forward one step: newer
        assert_eq!(v2.revs[v2.rev_ix.unwrap()].subject, "beta arrives");
        v2.rev_step(-1);
        assert_eq!(v2.revs[v2.rev_ix.unwrap()].subject, "gamma arrives");
        v2.rev_step(-1); // one more step forward: arrived at now
        assert!(v2.rev_ix.is_none(), "wound all the way forward to live");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn homecoming_is_inert_with_no_mark_and_no_repo() {
        let mut v = Viewer::from_text("fn a() {}\n", Language::Rust, None);
        assert!(v.visited_oid.is_none());
        assert!(v.homecoming_start_ix.is_none());
        v.enter_homecoming(); // must not panic
        assert!(v.rev_ix.is_none());
        v.mark_visited();
        assert!(v.branch_msg.as_ref().unwrap().0.contains("not a git repo"));
    }

    #[test]
    fn welcome_back_status_shows_the_gap_and_clears_once_homecoming_starts() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-welcome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("w.rs");
        std::fs::write(&f, "fn a() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        let mut v = Viewer::from_text("fn a() {}\n", Language::Rust, Some(f.clone()));
        v.mark_visited();
        std::fs::write(&f, "fn a() {}\nfn b() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "grew"]);

        let live = std::fs::read_to_string(&f).unwrap();
        let mut v2 = Viewer::from_text(&live, Language::Rust, Some(f));
        assert!(v2.status_line().contains("1 rev since"), "{}", v2.status_line());
        assert!(v2.status_line().contains("1 room changed"), "{}", v2.status_line());
        v2.enter_homecoming();
        assert!(!v2.status_line().contains("since you were last here"), "no longer 'live', no welcome banner: {}", v2.status_line());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_hour_follows_the_real_commit_timestamp_while_previewing() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-hour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str], date: Option<&str>| {
            let mut cmd = Command::new("git");
            cmd.arg("-C").arg(&dir).args(args);
            if let Some(d) = date {
                cmd.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
            }
            let out = cmd.output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"], None);
        git(&["config", "user.email", "t@t"], None);
        git(&["config", "user.name", "t"], None);
        let f = dir.join("t.rs");
        std::fs::write(&f, "fn a() {}\n").unwrap();
        git(&["add", "."], None);
        // A fixed, known timestamp: 1970-01-02 03:15:00 UTC = 27 hours in =
        // hour-of-day 3.25 (3:15am), chosen off any DST/local-time ambiguity
        // since gitspace parses %ct (unix seconds) directly.
        git(&["commit", "-q", "-m", "night owl commit"], Some("1970-01-02T03:15:00+00:00"));

        let mut v = Viewer::from_text("fn a() {}\n", Language::Rust, Some(f));
        assert_eq!(v.rev_ix, None);
        // Live: current_hour should be a plausible wall-clock value, NOT
        // locked to the commit — sanity range check only (real time varies).
        assert!((0.0..24.0).contains(&v.current_hour()));

        v.rev_step(1); // preview the only commit
        assert!(v.rev_ix.is_some());
        let h = v.current_hour();
        assert!((h - 3.25).abs() < 0.02, "expected ~3.25 (03:15 UTC), got {h}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jump_to_graph_node_moves_the_preview_to_that_exact_revision() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-graphclick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("g.rs");
        for (i, msg) in ["one", "two", "three"].iter().enumerate() {
            std::fs::write(&f, format!("fn n{i}() {{}}\n")).unwrap();
            git(&["add", "."]);
            git(&["commit", "-q", "-m", msg]);
        }
        let live = std::fs::read_to_string(&f).unwrap();
        let mut v = Viewer::from_text(&live, Language::Rust, Some(f));
        assert_eq!(v.revs.len(), 3);
        assert!(v.rev_ix.is_none(), "starts live");

        let target = v.revs.iter().position(|r| r.subject == "two").unwrap();
        v.jump_to_graph_node(target);
        assert_eq!(v.rev_ix, Some(target));
        assert_eq!(v.revs[v.rev_ix.unwrap()].subject, "two");

        // Clicking an out-of-range index clamps rather than panicking.
        v.jump_to_graph_node(9999);
        assert_eq!(v.rev_ix, Some(v.revs.len() - 1));

        // The way back walks the revisions in reverse: to "two", then live.
        v.nav_go(true);
        assert_eq!(v.rev_ix, Some(target), "back to the revision the last jump left");
        v.nav_go(true);
        assert!(v.rev_ix.is_none(), "and back again to the live file");
        v.nav_go(false);
        assert_eq!(v.rev_ix, Some(target), "forward retakes it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn margin_note_truncates_to_the_real_textsurface_streetsign_budget() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-scene-margin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("m.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);

        let mut v = Viewer::from_text("fn one() {}\n", Language::Rust, Some(f));
        let long = "y".repeat(200);
        v.describe = Some(Engine::new(Box::new(CommandProvider::new(vec!["echo".into(), "-n".into(), long]))));

        v.rev_step(1);
        let mut got = false;
        for _ in 0..200 {
            v.poll_note();
            if !v.rev_note.is_empty() {
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "note arrived");
        // Not a hardcoded expectation of 60 — reads the SAME real function
        // the production code calls, so this fails if that budget ever
        // changes without the intent behind it changing too.
        let expected = scene_kit::textsurface::Surface::StreetSign.char_budget();
        assert_eq!(v.rev_note.chars().count(), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frame_points_centers_the_target_and_pulls_back_for_a_wider_cluster() {
        use scene_kit::spatial::Vec3;
        let mut v = Viewer::from_text("fn a() {}\n", Language::Rust, None);
        let tight = vec![Vec3::new(0.0, 5.0, 0.0), Vec3::new(1.0, 5.0, 0.0)];
        v.frame_points(&tight);
        // Centroid of the two points.
        assert!((v.tx.target - 0.5).abs() < 1e-4);
        assert!((v.ty.target - 5.0).abs() < 1e-4);
        assert!((v.tz.target - 0.0).abs() < 1e-4);
        let dist_tight = v.dist.target;

        let wide = vec![Vec3::new(-20.0, 5.0, 0.0), Vec3::new(20.0, 5.0, 0.0)];
        v.frame_points(&wide);
        assert!((v.tx.target - 0.0).abs() < 1e-4, "still centered, now on a wider spread");
        assert!(v.dist.target > dist_tight, "a wider cluster needs a farther camera to fit it");
    }

    #[test]
    fn frame_points_is_a_noop_on_empty_input() {
        let mut v = Viewer::from_text("fn a() {}\n", Language::Rust, None);
        let before = (v.tx.target, v.ty.target, v.tz.target, v.dist.target);
        v.frame_points(&[]);
        assert_eq!((v.tx.target, v.ty.target, v.tz.target, v.dist.target), before);
    }


    #[test]
    fn a_plaque_click_is_recorded_and_arrow_left_flies_back() {
        let src = "fn alpha() {}\n".to_string() + &"// pad\n".repeat(50) + "struct Beta { x: u32 }\nfn omega() {}\n";
        let mut v = Viewer::from_text(&src, Language::Rust, None);
        let beta = v.stations.iter().position(|s| s.item.name == "Beta").unwrap();
        let omega = v.stations.iter().position(|s| s.item.name == "omega").unwrap();
        v.jump_rel(1); // walking the corridor in order: not recorded
        assert!(v.nav.is_empty());
        v.jump(0);
        v.jump_recorded(beta);
        v.jump_recorded(omega);
        assert_eq!(v.nav.len(), 2);
        v.nav_go(true);
        assert_eq!(v.focus, beta);
        v.nav_go(true);
        assert_eq!(v.focus, 0, "back to alpha, where the first click started");
        v.nav_go(true);
        assert_eq!(v.focus, 0, "nothing further back: stays put");
        assert!(v.branch_msg.as_ref().unwrap().0.contains("nothing to go back"));
        v.nav_go(false);
        assert_eq!(v.focus, beta);
    }

    #[test]
    fn going_back_to_an_item_this_version_lacks_says_so_and_keeps_the_step() {
        let src = "fn alpha() {}\nfn beta() {}\n";
        let mut v = Viewer::from_text(src, Language::Rust, None);
        v.jump_recorded(1);
        // A different version of the file, without alpha.
        v.set_text("fn beta() {}\nfn gamma() {}\n");
        v.nav_go(true);
        assert!(v.branch_msg.as_ref().unwrap().0.contains("alpha is not in this version"));
        assert!(v.nav.can_go_back(), "the step is still there for when alpha is");
    }


    #[test]
    fn the_trail_is_the_visited_stations_in_order_and_shrinks_when_you_go_back() {
        let src = "fn alpha() {}\n".to_string() + &"// pad\n".repeat(30) + "struct Beta { x: u32 }\nfn omega() {}\n";
        let mut v = Viewer::from_text(&src, Language::Rust, None);
        let beta = v.stations.iter().position(|s| s.item.name == "Beta").unwrap();
        let omega = v.stations.iter().position(|s| s.item.name == "omega").unwrap();
        v.jump_recorded(omega);
        v.jump_recorded(beta);
        assert_eq!(v.trail_stations(), vec![0, omega, beta]);
        v.nav_go(true);
        assert_eq!(v.trail_stations(), vec![0, omega], "the undone step is no longer lit");
    }

}
