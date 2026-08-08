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

use aperture_core::syntax::{Language, Lexer, SyntaxCache, TokKind};
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
    pub syntax: SyntaxCache,
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
}

impl Viewer {
    pub fn from_text(text: &str, lang: Language, path: Option<PathBuf>) -> Self {
        Viewer {
            buffer: Buffer::from_str(text),
            syntax: SyntaxCache::new(Lexer::new(lang)),
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
        }
    }

    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(&path)?;
        let lang = Language::from_path(&path);
        Ok(Self::from_text(&text, lang, Some(path)))
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
            let (buffer, syntax) = (&self.buffer, &mut self.syntax);
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
                    .spans_for(li, |j| buffer.line(j).into_owned())
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
                styled.spans.push((0..gutter.len(), [150, 134, 108]));
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
    pub fn step(&mut self, dt: f32) -> bool {
        let moving = self.top.step(dt) | self.yaw.step(dt) | self.pitch.step(dt) | self.dist.step(dt);
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
        format!("{name} · lines {top}–{bot} of {} · {:?}", self.buffer.len_lines(), self.lang)
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
}
