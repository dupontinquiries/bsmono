//! viewport — the picture beside the music.
//!
//! A music video today is export → import → watch → adjust → export: the
//! iteration loop runs through the filesystem. The viewport closes it: the
//! frame under the playhead, drawn beside the rack and the mixer in the
//! workbench, so a cut lands against the picture it scores. Honest about
//! its rate: frames come from `studio_video::frame_rgb` (an ffmpeg seek per
//! frame), cached on a coarse half-second key — a scrub preview and an
//! edit reference, not a player; realtime video playback is the named
//! future and belongs with a real decoder, not a subprocess per frame.
//!
//! The clip choice is pure ([`video_at`]) and tested: the FIRST video
//! track in project order is the topmost — the same convention the
//! timeline draws by — and the media time honours the clip's trim.

use studio_core::{Project, Ticks, TrackKind};

/// Viewport pixel size: 16:9, small enough for a phone's workbench strip,
/// fixed so the texture never reallocates.
pub const W: u32 = 320;
pub const H: u32 = 180;

/// The video clip under `playhead`, as (source path, media time): the
/// first video track in project order wins (topmost, the timeline's own
/// convention), and media time is `src_in + (playhead - pos)` — the
/// trim-honouring position INSIDE the source file.
pub fn video_at(project: &Project, playhead: Ticks) -> Option<(String, Ticks)> {
    for t in project.tracks.iter().filter(|t| t.kind == TrackKind::Video) {
        let mut hit: Option<(&studio_core::Clip, Ticks)> = None;
        for c in project.clips.values().filter(|c| c.track == t.id) {
            if c.pos <= playhead && playhead < c.end() {
                let media = c.src_in.saturating_add(playhead.saturating_sub(c.pos));
                hit = Some((c, media));
                break;
            }
        }
        if let Some((c, media)) = hit {
            let path = project.sources.get(&c.source)?.path.clone();
            return Some((path, media));
        }
    }
    None
}

/// The coarse cache key: half-second steps, so scrubbing repaints at 2 fps
/// while a paused playhead costs zero subprocesses per frame.
/// EVERY video clip under the playhead, one per video track, in PAINT
/// order: bottom first, so the compositor (which paints its slice bottom to
/// top) ends on the FIRST video track -- the topmost, as `video_at`, the
/// export and the timeline all have it. This returned track order until
/// c20, which painted the top track UNDER the others: a full-frame clip on a
/// lower track hid the one the caption named. Each carries its
/// source path, media time, the clip's transform (from a `transform`
/// effect, identity otherwise) and its blend (from a `blend` effect,
/// Normal at full opacity otherwise). `video_at` is the first of these;
/// it stays because the cache key and the caption use it.
pub fn layers_at(project: &Project, playhead: Ticks) -> Vec<LayerSpec> {
    let mut out = Vec::new();
    for t in project.tracks.iter().rev().filter(|t| t.kind == TrackKind::Video) {
        for c in project.clips.values().filter(|c| c.track == t.id) {
            if c.pos <= playhead && playhead < c.end() {
                let Some(src) = project.sources.get(&c.source) else { continue };
                let media = c.src_in.saturating_add(playhead.saturating_sub(c.pos));
                let mut transform = studio_fx::Transform::default();
                let mut mode = studio_fx::compositor::BlendMode::Normal;
                let mut opacity = 1.0;
                let mut color = None;
                for fx in c.effects.iter().filter(|f| f.enabled) {
                    match studio_fx::instantiate(fx, 48_000.0) {
                        Some(studio_fx::Instance::Transform(t)) => transform = t,
                        Some(studio_fx::Instance::Color(g)) => color = (!g.is_identity()).then_some(g),
                        Some(studio_fx::Instance::Blend(m, o)) => { mode = m; opacity = o; }
                        _ => {}
                    }
                }
                let kaleido = studio_video::kaleido::Kaleido::of(&c.effects);
                let style = studio_video::style::Transfer::of(&c.effects);
                let fade = project.fades.get(&c.id)
                    .map(|(fi, fo)| fade_gain(c.pos, c.end(), *fi, *fo, playhead)).unwrap_or(1.0);
                out.push(LayerSpec { path: src.path.clone(), media, transform, mode, opacity, color, kaleido, style, fade });
                break;
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct LayerSpec {
    pub path: String,
    pub media: Ticks,
    pub transform: studio_fx::Transform,
    pub mode: studio_fx::compositor::BlendMode,
    pub opacity: f32,
    /// The clip's `color` grade — applied to the layer before compositing,
    /// the same order the export uses (`studio_video::edit::Picture`), so
    /// the preview no longer shows an ungraded picture the file won't have.
    pub color: Option<studio_fx::Color>,
    /// The clip's kaleidoscope, folded after the grade (the export's order).
    pub kaleido: Option<studio_video::kaleido::Kaleido>,
    /// The clip's style transfer, applied as its LUT after the grade.
    pub style: Option<studio_video::style::Transfer>,
    /// The clip's fade at the playhead: 1.0 fully there, 0.0 gone. An
    /// OPACITY, multiplied into the layer's when it is painted
    /// ([`LayerSpec::paint_opacity`]): over nothing that fades to black, over
    /// a lower track's clip it dissolves into it -- the export's rule.
    pub fade: f32,
}

/// A clip's opacity at `t` under its fade in / out: linear, as ffmpeg's
/// `fade` filter is, so the preview fades exactly as the file does.
pub fn fade_gain(pos: Ticks, end: Ticks, fade_in: Ticks, fade_out: Ticks, t: Ticks) -> f32 {
    let mut g = 1.0f64;
    if fade_in.0 > 0 {
        g = g.min(t.saturating_sub(pos).to_seconds() / fade_in.to_seconds());
    }
    if fade_out.0 > 0 {
        g = g.min(end.saturating_sub(t).to_seconds() / fade_out.to_seconds());
    }
    g.clamp(0.0, 1.0) as f32
}

impl LayerSpec {
    /// The opacity this layer is painted with: the blend dial times the fade.
    /// (The transform's own opacity the compositor multiplies in itself.)
    pub fn paint_opacity(&self) -> f32 { self.opacity * self.fade }

    /// Grade a fetched frame in place. The fade is NOT applied here: it is an
    /// opacity, applied when the graded layer is painted -- the export's order
    /// since c20, so a faded-out clip is black even under a grade that lifts
    /// blacks (before c20 both lifted it; now neither does).
    pub fn grade(&self, px: &mut [u8]) {
        if let Some(c) = &self.color {
            for p in px.chunks_mut(3) {
                let o = c.apply([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]);
                for k in 0..3 { p[k] = (o[k] * 255.0 + 0.5) as u8; }
            }
        }
        if let Some(t) = &self.style {
            let n = studio_video::style::LUT_N;
            studio_video::style::apply_lut(px, &t.lut(n), n);
        }
        if let Some(k) = &self.kaleido {
            let folded = studio_video::kaleido::apply(px, W, H, k);
            px.copy_from_slice(&folded);
        }
    }
}

pub fn coarse(t: Ticks) -> i64 {
    (t.to_seconds() * 2.0).floor() as i64
}

/// One angle of a multiview: where its picture goes on the viewport canvas,
/// and which moment of it to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub path: String,
    pub media: Ticks,
    /// Placement as the compositor takes it: the angle's whole frame scaled
    /// into its tile's picture rectangle.
    pub transform: studio_fx::Transform,
    /// True when this tile is too small to read anything in — the caller
    /// says so rather than drawing nine unreadable thumbnails.
    pub cramped: bool,
}

/// Lay a camera group out as a grid on the viewport canvas, at `playhead`.
///
/// The angles are the group's clips, each shown at ITS OWN moment: an angle
/// whose clip is trimmed differently still shows the same instant of the
/// scene, because media time is `src_in + (playhead - pos)` per clip, the
/// same rule [`video_at`] uses. Angles that do not cover the playhead are
/// left out, so a multiview at the head of a shot does not show black tiles
/// for angles that start later.
///
/// `None` when fewer than two angles are under the playhead — one angle is
/// not a multiview, it is the ordinary viewport.
pub fn multiview_at(project: &Project, playhead: Ticks, group: studio_core::GroupId) -> Option<Vec<Pane>> {
    let g = project.groups.get(&group)?;
    let live: Vec<(&studio_core::Clip, &studio_core::Source)> = g.clips.iter()
        .filter_map(|id| project.clips.get(id))
        .filter(|c| c.pos <= playhead && playhead < c.pos.saturating_add(c.len))
        .filter_map(|c| project.sources.get(&c.source).map(|s| (c, s)))
        .filter(|(_, s)| s.has_video)
        .collect();
    if live.len() < 2 { return None; }
    let opts = studio_video::grid::Options::default();
    // The canvas is the viewport texture, not the delivery size: a multiview
    // is a way of LOOKING, so it is drawn at the size it is looked at.
    let layout = studio_video::grid::layout(live.len() as u32, W, H, W, H, opts);
    Some(live.iter().zip(&layout.tiles).enumerate().map(|(i, ((c, s), tile))| {
        let (px, py, pw, ph) = tile.picture;
        Pane {
            path: s.path.clone(),
            media: c.src_in.saturating_add(playhead.saturating_sub(c.pos)),
            transform: studio_fx::Transform {
                // anchor at the top-left, so the placement is a plain
                // scale-then-offset into the tile's picture rectangle
                anchor_x: 0.0, anchor_y: 0.0,
                x: px as f32 / W as f32, y: py as f32 / H as f32,
                scale_x: pw as f32 / W as f32, scale_y: ph as f32 / H as f32,
                ..Default::default()
            },
            cramped: layout.too_small.contains(&i),
        }
    }).collect())
}

/// Draw a camera group as a grid. Falls back to the ordinary viewport when
/// the group is not showing at least two angles here.
pub fn show_multiview(ui: &mut egui::Ui, project: &Project, playhead: Ticks,
                      group: studio_core::GroupId, base: &std::path::Path,
                      cache: &mut FrameCache, c: &uikit_egui::PaletteColors) {
    let Some(panes) = multiview_at(project, playhead, group) else {
        show(ui, project, playhead, base, cache, c);
        return;
    };
    let key = (panes.iter().map(|p| format!("mv:{}@{}", p.path, coarse(p.media)))
        .collect::<Vec<_>>().join("|"), coarse(playhead));
    if cache.key.as_ref() != Some(&key) && cache.failed.as_ref() != Some(&key) {
        let mut frames = Vec::new();
        let mut err = None;
        for p in &panes {
            match studio_video::frame_rgb(&base.join(&p.path), p.media, W, H) {
                Ok(px) => frames.push((px, p.transform)),
                Err(e) => { err = Some(format!("{e:?}")); break; }
            }
        }
        match err {
            None => {
                let stack: Vec<studio_fx::compositor::Layer<'_>> = frames.iter()
                    .map(|(px, t)| studio_fx::compositor::Layer { rgb: px, w: W, h: H, transform: *t,
                        mode: studio_fx::compositor::BlendMode::Normal, opacity: 1.0 })
                    .collect();
                let px = studio_fx::compositor::composite(W, H, [0, 0, 0], &stack);
                let img = egui::ColorImage::from_rgb([W as usize, H as usize], px.as_slice());
                cache.tex = Some(ui.ctx().load_texture("viewport", img, egui::TextureOptions::LINEAR));
                cache.key = Some(key);
            }
            Some(e) => {
                cache.failed = Some(key);
                ui.colored_label(c.text_weak, format!("frame unavailable: {e}"));
            }
        }
    }
    if let Some(tex) = &cache.tex {
        let avail = ui.available_width().min(W as f32 * 2.0);
        let size = egui::vec2(avail, avail * H as f32 / W as f32);
        ui.add(egui::Image::new(tex).fit_to_exact_size(size));
        let cramped = panes.iter().filter(|p| p.cramped).count();
        let mut line = format!("{} angles @ {:.1}s — preview at ~2 fps", panes.len(), playhead.to_seconds());
        if cramped > 0 {
            line.push_str(&format!(" — {cramped} tile(s) too small to read"));
        }
        ui.colored_label(c.text_weak, line);
    }
}

/// Frame cache owned by the shell: what is showing, and its texture.
#[derive(Default)]
pub struct FrameCache {
    key: Option<(String, i64)>,
    tex: Option<egui::TextureHandle>,
    /// The last fetch failed for this key — remembered so a broken file is
    /// one error message, not one ffmpeg spawn per repaint.
    failed: Option<(String, i64)>,
}

/// Draw the frame under the playhead, fetching only when the coarse key
/// moved. `base` resolves the project's relative media paths.
pub fn show(ui: &mut egui::Ui, project: &Project, playhead: Ticks,
            base: &std::path::Path, cache: &mut FrameCache,
            c: &uikit_egui::PaletteColors) {
    let Some((path, media)) = video_at(project, playhead) else {
        cache.key = None;
        ui.centered_and_justified(|ui| {
            ui.colored_label(c.text_weak, "no video under the playhead");
        });
        return;
    };
    let layers = layers_at(project, playhead);
    // The key names every layer at its coarse time, so a second track
    // appearing or a blend changing re-composites; one track is the old key.
    // Transform and grade are in the key: without them, adjusting either left
    // the old picture on screen until the playhead crossed a half-second.
    // The FADE is in the key too: the composite is graded then cached, so
    // without it a fade would freeze at whatever brightness was cached first.
    let key = (layers.iter().map(|l| format!("{}@{}:{:?}:{:.2}:{:?}:{:?}:{:.2}", l.path, coarse(l.media), l.mode, l.opacity,
                                             l.transform, (l.color, l.kaleido, l.style), l.fade)).collect::<Vec<_>>().join("|"), coarse(media));
    if cache.key.as_ref() != Some(&key) && cache.failed.as_ref() != Some(&key) {
        let mut frames: Vec<(Vec<u8>, &LayerSpec)> = Vec::new();
        let mut err = None;
        for l in &layers {
            match studio_video::frame_rgb(&base.join(&l.path), l.media, W, H) {
                Ok(mut px) => { l.grade(&mut px); frames.push((px, l)) }
                Err(e) => { err = Some(format!("{e:?}")); break; }
            }
        }
        match err {
            None => {
                let stack: Vec<studio_fx::compositor::Layer<'_>> = frames.iter().map(|(px, l)| studio_fx::compositor::Layer {
                    rgb: px, w: W, h: H, transform: l.transform, mode: l.mode, opacity: l.paint_opacity() }).collect();
                let px = studio_fx::compositor::composite(W, H, [0, 0, 0], &stack);
                let img = egui::ColorImage::from_rgb([W as usize, H as usize], px.as_slice());
                cache.tex = Some(ui.ctx().load_texture(
                    "viewport", img, egui::TextureOptions::LINEAR));
                cache.key = Some(key);
            }
            Some(e) => {
                cache.failed = Some(key);
                ui.colored_label(c.text_weak, format!("frame unavailable: {e}"));
            }
        }
    }
    if let Some(tex) = &cache.tex {
        let avail = ui.available_width().min(W as f32 * 2.0);
        let size = egui::vec2(avail, avail * H as f32 / W as f32);
        ui.add(egui::Image::new(tex).fit_to_exact_size(size));
        let n = layers_at(project, playhead).len();
        ui.colored_label(c.text_weak, if n > 1 {
            format!("{path} +{} layer{} @ {:.1}s — preview at ~2 fps", n - 1, if n > 2 { "s" } else { "" }, media.to_seconds())
        } else {
            format!("{path} @ {:.1}s — preview at ~2 fps", media.to_seconds())
        });
    }
}

#[cfg(test)]
mod tests {
    fn secs(x: f64) -> Ticks { Ticks::from_seconds(x) }

    /// Linear, as ffmpeg's fade: half way through a 2 s fade-in is half
    /// bright; the fade-out mirrors it; outside both, full.
    #[test]
    fn the_preview_fade_matches_the_export_curve() {
        let g = |t: f64| fade_gain(secs(0.0), secs(3.0), secs(2.0), secs(1.0), secs(t));
        assert!((g(0.0) - 0.0).abs() < 1e-6);
        assert!((g(1.0) - 0.5).abs() < 1e-3, "{}", g(1.0));
        assert!((g(2.0) - 1.0).abs() < 1e-6);
        assert!((g(2.5) - 0.5).abs() < 1e-3, "{}", g(2.5));
        assert!(g(2.999) < 0.01);
        assert_eq!(fade_gain(secs(0.0), secs(3.0), Ticks::ZERO, Ticks::ZERO, secs(1.0)), 1.0, "no fade");
    }

    /// The model's fade reaches the layer at the playhead, and grading a
    /// frame through it dims it -- before the colour grade, as the export.
    #[test]
    fn a_faded_clip_dims_its_layer() {
        let mut p = Project::default();
        p.apply(&studio_core::Op::AddTrack { track: studio_core::Track { id: studio_core::TrackId(1),
            kind: TrackKind::Video, name: "v".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&studio_core::Op::AddSource { source: studio_core::Source { id: studio_core::SourceId(1),
            path: "a.mp4".into(), len: secs(60.0), sample_rate: 48_000, channels: 2,
            has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&studio_core::Op::AddClip { clip: studio_core::Clip { id: studio_core::ClipId(1),
            source: studio_core::SourceId(1), track: studio_core::TrackId(1), pos: secs(0.0), src_in: secs(0.0),
            len: secs(4.0), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&studio_core::Op::SetClipFade { id: studio_core::ClipId(1), fade_in: secs(2.0), fade_out: Ticks::ZERO }).unwrap();
        let l = &layers_at(&p, secs(0.5))[0];
        assert!((l.fade - 0.25).abs() < 1e-3, "{}", l.fade);
        let mut px = vec![200u8; 12];
        l.grade(&mut px);
        assert!(px.iter().all(|&v| v == 200), "the grade leaves the fade to painting: {px:?}");
        let paint = |under: Option<&[u8]>| {
            let mut stack = Vec::new();
            if let Some(u) = under { stack.push(studio_fx::compositor::Layer { rgb: u, w: 2, h: 2,
                transform: Default::default(), mode: studio_fx::compositor::BlendMode::Normal, opacity: 1.0 }); }
            stack.push(studio_fx::compositor::Layer { rgb: &px, w: 2, h: 2, transform: l.transform, mode: l.mode,
                opacity: l.paint_opacity() });
            studio_fx::compositor::composite(2, 2, [0, 0, 0], &stack)[0]
        };
        assert_eq!(paint(None), 50, "over nothing: a quarter of 200, the old fade to black");
        assert_eq!(paint(Some(&[100u8; 12])), 125, "over a lower clip: a dissolve, 0.25*200 + 0.75*100");
        assert_eq!(layers_at(&p, secs(3.0))[0].fade, 1.0);
    }

    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId};

    fn project() -> Project {
        let mut p = Project::default();
        for (i, kind) in [TrackKind::Video, TrackKind::Video, TrackKind::Audio]
            .iter().enumerate()
        {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: *kind,
                name: format!("T{i}"), muted: false, locked: false, gain: 1.0,
                route: None }, at: i }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.mp4".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2,
            has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(2), path: "b.mp4".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2,
            has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        // Top video track: a TRIMMED clip at 2s reading from 5s into a.mp4.
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::from_seconds(2.0),
            src_in: Ticks::from_seconds(5.0), len: Ticks::from_seconds(4.0),
            gain: 1.0, effects: vec![] } }).unwrap();
        // Second video track underneath, covering everything.
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(2), source: SourceId(2),
            track: TrackId(2), pos: Ticks::ZERO,
            src_in: Ticks::ZERO, len: Ticks::from_seconds(20.0),
            gain: 1.0, effects: vec![] } }).unwrap();
        p
    }

    #[test]
    fn the_topmost_video_clip_wins_and_its_trim_is_honoured() {
        let p = project();
        // Under the top clip: its source, at src_in + offset — 5 + 1 = 6s.
        let (path, media) = video_at(&p, Ticks::from_seconds(3.0)).unwrap();
        assert_eq!(path, "a.mp4");
        assert!((media.to_seconds() - 6.0).abs() < 1e-6, "trim honoured: {media:?}");
        // Past the top clip's end: the lower track shows through.
        let (path, media) = video_at(&p, Ticks::from_seconds(10.0)).unwrap();
        assert_eq!(path, "b.mp4", "the lower video track shows through a gap");
        assert!((media.to_seconds() - 10.0).abs() < 1e-6);
        // Past everything: nothing, said as None rather than a stale frame.
        assert!(video_at(&p, Ticks::from_seconds(30.0)).is_none());
    }

    #[test]
    fn the_coarse_key_repaints_at_two_fps_and_not_more() {
        assert_eq!(coarse(Ticks::from_seconds(1.0)), coarse(Ticks::from_seconds(1.4)),
                   "within the same half second: one fetch");
        assert_ne!(coarse(Ticks::from_seconds(1.4)), coarse(Ticks::from_seconds(1.6)),
                   "crossing it: a new frame");
    }
    /// The compositor paints its slice bottom to top, so the LAST layer is
    /// the one on top -- and that must be the clip `video_at` (and the
    /// export, and the timeline) call the topmost: the FIRST video track.
    #[test]
    fn the_stack_is_painted_so_the_first_video_track_ends_on_top() {
        let mut p = Project::default();
        for (i, name) in ["top", "bottom"].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(10 + i as u64), kind: TrackKind::Video, name: (*name).into(),
                muted: false, locked: false, gain: 1.0, route: None }, at: i }).unwrap();
            p.apply(&Op::AddSource { source: Source { id: SourceId(20 + i as u64), path: format!("{name}.mp4"),
                len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(30 + i as u64), source: SourceId(20 + i as u64), track: TrackId(10 + i as u64),
                pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(5.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        let layers = layers_at(&p, Ticks::from_seconds(1.0));
        assert_eq!(layers.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["bottom.mp4", "top.mp4"]);
        assert_eq!(layers.last().unwrap().path, video_at(&p, Ticks::from_seconds(1.0)).unwrap().0);
        // and through the compositor itself: two opaque full frames, the top one is what shows
        let (red, blue) = ([255u8, 0, 0].repeat(4), [0u8, 0, 255].repeat(4));
        let frame = |l: &LayerSpec| if l.path == "top.mp4" { &red } else { &blue };
        let stack: Vec<_> = layers.iter().map(|l| studio_fx::compositor::Layer { rgb: frame(l), w: 2, h: 2,
            transform: l.transform, mode: l.mode, opacity: l.opacity }).collect();
        assert_eq!(&studio_fx::compositor::composite(2, 2, [0, 0, 0], &stack)[..3], &[255, 0, 0]);
    }

    #[test]
    fn a_second_video_track_is_a_second_layer_with_its_blend_and_transform() {
        let mut p = Project::default();
        for (i, name) in ["v1", "v2"].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(10 + i as u64), kind: TrackKind::Video, name: (*name).into(),
                muted: false, locked: false, gain: 1.0, route: None }, at: i }).unwrap();
            p.apply(&Op::AddSource { source: Source { id: SourceId(20 + i as u64), path: format!("{name}.mp4"),
                len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        }
        let mut blend = std::collections::BTreeMap::new();
        blend.insert("mode".to_string(), 2.0); // multiply
        blend.insert("opacity".to_string(), 0.5);
        let mut xf = std::collections::BTreeMap::new();
        xf.insert("x".to_string(), 0.25);
        for (i, fx) in [vec![], vec![
            studio_core::Effect { kind: "blend".into(), params: blend, enabled: true },
            studio_core::Effect { kind: "transform".into(), params: xf, enabled: true },
        ]].into_iter().enumerate() {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(30 + i as u64), source: SourceId(20 + i as u64), track: TrackId(10 + i as u64),
                pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(5.0), gain: 1.0, effects: fx } }).unwrap();
        }
        let layers = layers_at(&p, Ticks::from_seconds(1.0));
        assert_eq!(layers.len(), 2);
        // paint order: v2 (the lower track) first, v1 last
        assert_eq!(layers[1].path, "v1.mp4");
        assert_eq!(layers[1].mode, studio_fx::compositor::BlendMode::Normal);
        assert_eq!(layers[0].mode, studio_fx::compositor::BlendMode::Multiply);
        assert!((layers[0].opacity - 0.5).abs() < 1e-6);
        assert!((layers[0].transform.x - 0.25).abs() < 1e-6);
        assert!(layers_at(&p, Ticks::from_seconds(7.0)).is_empty());
    }

}

#[cfg(test)]
mod multiview_tests {
    use super::*;
    use studio_core::*;

    /// Three angles of the same scene, each trimmed differently: angle 2
    /// starts a second later on the timeline, angle 3 covers only the tail.
    fn project() -> Project {
        let mut p = Project::default();
        for id in 1..=3u64 {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video,
                name: format!("angle {id}"), muted: false, locked: false, gain: 1.0, route: None },
                at: (id - 1) as usize }).unwrap();
            p.apply(&Op::AddSource { source: Source { id: SourceId(id), path: format!("C{id}.mkv"),
                len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true,
                kind: SourceKind::File } }).unwrap();
        }
        let mut add = |id: u64, pos: f64, src_in: f64, len: f64| {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id),
                pos: Ticks::from_seconds(pos), src_in: Ticks::from_seconds(src_in),
                len: Ticks::from_seconds(len), gain: 1.0, effects: vec![] } }).unwrap();
        };
        add(1, 0.0, 10.0, 8.0);
        add(2, 0.0, 4.0, 8.0);
        add(3, 6.0, 30.0, 2.0);   // only the last two seconds
        p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(),
                                clips: vec![ClipId(1), ClipId(2), ClipId(3)] }).unwrap();
        p
    }

    #[test]
    fn every_pane_shows_the_same_instant_through_its_own_trim() {
        let p = project();
        let panes = multiview_at(&p, Ticks::from_seconds(7.0), GroupId(1)).expect("three angles");
        assert_eq!(panes.len(), 3);
        // src_in + (playhead - pos), per angle
        let at: Vec<f64> = panes.iter().map(|x| x.media.to_seconds()).collect();
        assert_eq!(at, vec![17.0, 11.0, 31.0], "{at:?}");
    }

    #[test]
    fn angles_that_are_not_under_the_playhead_are_left_out() {
        let p = project();
        let panes = multiview_at(&p, Ticks::from_seconds(2.0), GroupId(1)).expect("two angles");
        assert_eq!(panes.len(), 2, "the third angle starts at 6 s");
        // and with only one angle live there is no multiview to draw
        assert!(multiview_at(&p, Ticks::from_seconds(9.0), GroupId(1)).is_none(),
                "past every angle but none: nothing to grid");
        let mut one = p.clone();
        one.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: vec![ClipId(1)] }).unwrap();
        assert!(multiview_at(&one, Ticks::from_seconds(2.0), GroupId(1)).is_none());
    }

    #[test]
    fn the_panes_tile_the_canvas_without_overlapping() {
        let p = project();
        let panes = multiview_at(&p, Ticks::from_seconds(7.0), GroupId(1)).unwrap();
        // Each transform maps the whole frame into a rectangle; check those
        // rectangles are inside the canvas and disjoint.
        let rect = |t: &studio_fx::Transform| {
            let (x, y) = (t.x * W as f32, t.y * H as f32);
            (x, y, x + t.scale_x * W as f32, y + t.scale_y * H as f32)
        };
        let rects: Vec<_> = panes.iter().map(|p| rect(&p.transform)).collect();
        for r in &rects {
            assert!(r.0 >= -0.5 && r.1 >= -0.5 && r.2 <= W as f32 + 0.5 && r.3 <= H as f32 + 0.5, "{r:?}");
            assert!(r.2 > r.0 && r.3 > r.1, "{r:?}");
        }
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let overlap = a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3;
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
        // 16:9 angles in a 16:9 canvas: the picture keeps its shape
        for (p, r) in panes.iter().zip(&rects) {
            let aspect = (r.2 - r.0) / (r.3 - r.1);
            assert!((aspect - W as f32 / H as f32).abs() < 0.05, "{aspect} for {p:?}");
        }
    }
}
