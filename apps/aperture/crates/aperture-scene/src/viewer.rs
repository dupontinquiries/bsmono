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
        };
        out.git = out.path.as_deref().and_then(gitspace::discover);
        if let Some((r, rel)) = &out.git {
            out.revs = gitspace::history(r, rel, 48);
            // The live file wears what's changed since the last commit.
            if let Some(base) = out.revs.iter().find(|v| v.kind == gitspace::RevKind::Commit) {
                out.changed = gitspace::worktree_changed(r, &base.oid, rel);
            }
        }
        out.build_rooms();
        out
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
                    self.rev_note = text.trim().lines().next().unwrap_or("").chars().take(160).collect();
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
        format!("{name} · lines {top}–{bot} of {}{room}{rev} · {:?}", self.buffer.len_lines(), self.lang)
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

}
