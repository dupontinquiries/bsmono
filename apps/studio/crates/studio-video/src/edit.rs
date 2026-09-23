//! edit — the timeline's picture, rendered through ffmpeg's edit list.
//!
//! The arrangement already renders to audio; this is the other half of
//! a music video: every clip on a video track, in timeline order, cut
//! from its source file and concatenated into one picture, with the
//! mixdown underneath. No frame is decoded here — the cut list is pure
//! timeline arithmetic ([`edit_list`]), the command is a string
//! ([`render_args`]), and ffmpeg does the rest, which is the crate's
//! whole stance (see its module docs).
//!
//! ## The rule for two clips at once
//!
//! Video tracks stack: the FIRST video track in the project is on top,
//! and where two clips overlap in time the top one is the picture. The
//! same as every editor's track order, and the same as this app's
//! arrangement paints them. A gap where no video track has a clip is
//! black for that long — the timing of the audio must hold, so a gap is
//! a segment, not a skip.
//!
//! ## What ffmpeg is asked
//!
//! One `-i` per distinct source file, one `-i` for the mixdown WAV, and
//! a `filter_complex` that trims each cut out of its input (`trim` +
//! `setpts`), scales and pads every cut to one frame size (sources of
//! different sizes concatenate only if they agree), generates the black
//! gaps (`color`), and concatenates them all in order. `-shortest` so
//! the picture ends with the audio and never a frame past it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use studio_core::{Project, SourceId, Ticks, TrackKind};

/// One piece of the picture: `len` of `path` from `src_in`, placed at
/// `at` on the output.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub path: PathBuf,
    pub src_in: Ticks,
    pub len: Ticks,
    pub at: Ticks,
    /// The clip's picture effects, as the preview reads them.
    pub picture: Picture,
}

/// What the export does to a cut's picture: the clip's `color` and
/// `transform` effects, parsed by `studio_fx::instantiate` — the same parser
/// the viewport uses — so the file shows what the preview showed.
///
/// Until this existed the export ignored every clip effect: a reframe or a
/// grade seen in the viewport was simply absent from the rendered file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picture {
    pub transform: Option<studio_fx::Transform>,
    pub color: Option<studio_fx::Color>,
    /// A kaleidoscope fold, run after the grade and before the reframe --
    /// the same order the viewport uses.
    pub kaleido: Option<crate::kaleido::Kaleido>,
    /// A clip style transfer, run as a 3D LUT after the grade and before the
    /// kaleidoscope -- the viewport's order.
    pub style: Option<crate::style::Transfer>,
    /// Effects that change the picture in the preview but that this export
    /// cannot yet reproduce, named for the person exporting. Never silently
    /// dropped: a file that differs from the preview must say so.
    pub unsupported: Vec<String>,
}

impl Picture {
    /// From a clip's effect list, in order (a later effect of the same kind
    /// wins, as in the viewport).
    pub fn of(effects: &[studio_core::Effect]) -> Picture {
        let mut p = Picture { kaleido: crate::kaleido::Kaleido::of(effects),
                              style: crate::style::Transfer::of(effects), ..Picture::default() };
        for fx in effects.iter().filter(|f| f.enabled) {
            match studio_fx::instantiate(fx, 48_000.0) {
                Some(studio_fx::Instance::Transform(t)) => {
                    p.transform = (!t.is_identity()).then_some(t);
                    p.unsupported.retain(|u| !u.starts_with("transform"));
                    if t.rotation.abs() > 1e-4 { p.unsupported.push("transform rotation".into()); }
                    if (t.opacity - 1.0).abs() > 1e-4 { p.unsupported.push("transform opacity".into()); }
                }
                Some(studio_fx::Instance::Color(c)) => p.color = (!c.is_identity()).then_some(c),
                Some(studio_fx::Instance::Blend(..)) => p.unsupported.push("blend (layer compositing)".into()),
                Some(studio_fx::Instance::Retime(..)) => p.unsupported.push("retime".into()),
                _ => {}
            }
        }
        p
    }

    pub fn is_identity(&self) -> bool {
        self.transform.is_none() && self.color.is_none() && self.kaleido.is_none() && self.style.is_none()
    }

    /// The filter chain from a fitted `w`×`h` frame to the finished picture,
    /// as `(foreground chain, overlay position)`. The foreground chain runs on
    /// the fitted frame; when there is a transform it ends smaller than the
    /// canvas and is overlaid on black at the position given.
    fn filters(&self, w: u32, h: u32) -> (String, Option<(i64, i64)>) {
        let mut f = String::new();
        if let Some(c) = &self.color {
            // Lift, then gamma, then gain, per channel — studio_fx::Color::apply's order.
            let ch = |i: usize| format!("pow(max(val/255+({:.6}),0),1/{:.6})*{:.6}*255",
                                        c.lift[i], c.gamma[i].max(1e-3), c.gain[i]);
            f.push_str(&format!(",lutrgb=r='{}':g='{}':b='{}'", ch(0), ch(1), ch(2)));
            if (c.saturation - 1.0).abs() > 1e-6 {
                // y + (c - y)·s with Rec.709 luma, as a 3×3 matrix.
                let s = c.saturation;
                let (kr, kg, kb) = (0.2126 * (1.0 - s), 0.7152 * (1.0 - s), 0.0722 * (1.0 - s));
                f.push_str(&format!(
                    ",colorchannelmixer=rr={:.6}:rg={kg:.6}:rb={kb:.6}:gr={kr:.6}:gg={:.6}:gb={kb:.6}:br={kr:.6}:bg={kg:.6}:bb={:.6}",
                    kr + s, kg + s, kb + s));
            }
        }
        if let Some(st) = &self.style {
            // the LUT as a .cube in the temp dir, named by its content so the
            // same look is written once however many cuts share it
            let cube = st.cube(crate::style::LUT_N);
            let mut hsh = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&cube, &mut hsh);
            let path = std::env::temp_dir().join(format!("studio-style-{:016x}.cube", std::hash::Hasher::finish(&hsh)));
            if !path.exists() { let _ = std::fs::write(&path, &cube); }
            f.push_str(&format!(",lut3d=file='{}':interp=trilinear", path.display()));
        }
        if let Some(k) = &self.kaleido {
            f.push(',');
            f.push_str(&crate::kaleido::geq_filter(k, w, h));
        }
        let Some(t) = &self.transform else { return (f, None) };
        // Forward map of the compositor's inverse: a layer point u (0..1)
        // lands at canvas px = ax + (u·W − ax)·sx + ox. Rotation is not
        // reproduced (reported in `unsupported`).
        let (wf, hf) = (w as f64, h as f64);
        let (cl, ct, cr, cb) = (t.crop.0 as f64, t.crop.1 as f64, t.crop.2 as f64, t.crop.3 as f64);
        let (sx, sy) = ((t.scale_x as f64).max(1e-4), (t.scale_y as f64).max(1e-4));
        let (ax, ay) = (t.anchor_x as f64 * wf, t.anchor_y as f64 * hf);
        let cx = (cl * wf).round().max(0.0);
        let cy = (ct * hf).round().max(0.0);
        let cw = (((1.0 - cl - cr) * wf).round()).clamp(1.0, wf - cx);
        let chh = (((1.0 - ct - cb) * hf).round()).clamp(1.0, hf - cy);
        // Place by the continuous edges, then cover exactly the canvas pixels
        // whose CENTRES fall inside — the compositor samples at pixel centres,
        // and rounding the cropped size first and scaling second was off by a
        // row or column at each edge (measured: 19 rows where it drew 20).
        let edge_x = |u: f64| ax + (u * wf - ax) * sx + t.x as f64 * wf;
        let edge_y = |v: f64| ay + (v * hf - ay) * sy + t.y as f64 * hf;
        let first = |e: f64| (e - 0.5).ceil();
        let (px0, px1) = (first(edge_x(cl)), first(edge_x(1.0 - cr)));
        let (py0, py1) = (first(edge_y(ct)), first(edge_y(1.0 - cb)));
        let (ow, oh) = ((px1 - px0).max(1.0), (py1 - py0).max(1.0));
        f.push_str(&format!(",crop={cw}:{chh}:{cx}:{cy},scale={ow}:{oh}:flags=bicubic"));
        let (x0, y0) = (px0, py0);
        (f, Some((x0.round() as i64, y0.round() as i64)))
    }
}

/// A segment of the output: a cut, or black.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Cut(Cut),
    Black { at: Ticks, len: Ticks },
}

impl Segment {
    pub fn at(&self) -> Ticks { match self { Segment::Cut(c) => c.at, Segment::Black { at, .. } => *at } }
    pub fn len(&self) -> Ticks { match self { Segment::Cut(c) => c.len, Segment::Black { len, .. } => *len } }
    pub fn end(&self) -> Ticks { self.at().saturating_add(self.len()) }
}

/// The picture as segments covering `[0, end)` — `end` is the project's
/// duration unless given — with no gaps: black where no video clip is.
/// Only clips whose source has a path in `paths` and sits on a video
/// track count; a video source whose file has not arrived is a gap.
pub fn edit_list(project: &Project, paths: &BTreeMap<SourceId, PathBuf>, end: Option<Ticks>) -> Vec<Segment> {
    let end = end.unwrap_or_else(|| project.duration());
    // Video clips in priority order: track index ascending (first is top).
    let mut clips: Vec<(usize, &studio_core::Clip)> = project.clips.values()
        .filter_map(|c| {
            let ti = project.tracks.iter().position(|t| t.id == c.track)?;
            if project.tracks[ti].kind != TrackKind::Video { return None; }
            if !paths.contains_key(&c.source) { return None; }
            Some((ti, c))
        })
        .collect();
    clips.sort_by_key(|(ti, c)| (*ti, c.pos));
    // Sweep: each clip claims whatever of its span is not yet covered.
    let mut covered: Vec<(Ticks, Ticks)> = Vec::new();
    let mut cuts: Vec<Cut> = Vec::new();
    for (_, c) in clips {
        let (mut from, to) = (c.pos, c.end().min(end));
        if from >= to { continue; }
        // Walk the uncovered pieces of [from, to).
        let mut pieces = Vec::new();
        let mut occupied: Vec<(Ticks, Ticks)> = covered.iter().copied()
            .filter(|(s, e)| *s < to && *e > from).collect();
        occupied.sort();
        for (s, e) in occupied {
            if s > from { pieces.push((from, s)); }
            from = from.max(e);
        }
        if from < to { pieces.push((from, to)); }
        for (s, e) in pieces {
            cuts.push(Cut {
                path: paths[&c.source].clone(),
                src_in: c.src_in.saturating_add(s.saturating_sub(c.pos)),
                len: e.saturating_sub(s),
                at: s,
                picture: Picture::of(&c.effects),
            });
            covered.push((s, e));
        }
    }
    cuts.sort_by_key(|c| c.at);
    // Fill the gaps with black.
    let mut out = Vec::new();
    let mut t = Ticks::ZERO;
    for c in cuts {
        if c.at > t { out.push(Segment::Black { at: t, len: c.at.saturating_sub(t) }); }
        t = c.at.saturating_add(c.len);
        out.push(Segment::Cut(c));
    }
    if end > t { out.push(Segment::Black { at: t, len: end.saturating_sub(t) }); }
    out
}

/// The ffmpeg arguments that render `segments` at `w`×`h`, `fps`, with
/// `audio` (a WAV of the mixdown) underneath, to `out`. Pure, so the
/// command is testable without running it.
/// How a source that is not the output's shape is made to fit.
///
/// This was one hardcoded filter string, applied to every cut with no way to
/// choose otherwise, so letterboxing was not a missing feature but an
/// unexposed policy.
///
/// ## The bars stay COMPUTED
///
/// None of these bakes black pixels into the source. The padding is produced
/// at render time from the fit and the frame size, which is what lets a
/// project change its aspect later — bake the bars and that change cannot
/// undo them, and a source that was already letterboxed gets bars inside bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// Fit the whole frame inside, pad the rest. Nothing is cropped.
    ///
    /// The default because it is the only one that never destroys picture, and
    /// because it is what this pipeline has always done — changing the default
    /// would silently re-render every existing project differently.
    #[default]
    Contain,
    /// Fill the frame, cropping whatever overflows. No bars, edges lost.
    Cover,
    /// Stretch to the frame, ignoring aspect. Rarely right, occasionally
    /// exactly what is wanted for a source with bad metadata.
    Stretch,
}

impl Fit {
    /// The scale/crop/pad chain for this policy, at `w`x`h`.
    ///
    /// `force_original_aspect_ratio` does the arithmetic; `crop` and `pad`
    /// then take the frame to exactly the output size, because `scale` alone
    /// lands on one axis and leaves the other short or over.
    pub fn chain(self, w: u32, h: u32) -> String {
        match self {
            // Written on one line each. A `\` continuation would strip the
            // newline and the indent, but a filter string is a place where
            // "probably strips exactly what I meant" is not good enough --
            // one stray space and ffmpeg rejects the whole graph.
            Fit::Contain =>
                format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"),
            Fit::Cover =>
                format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}"),
            Fit::Stretch => format!("scale={w}:{h}"),
        }
    }
}

pub fn render_args(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32) -> Vec<String> {
    render_args_with(segments, audio, out, w, h, fps, Fit::default())
}

/// `render_args` with an explicit fit policy.
pub fn render_args_with(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32, fit: Fit) -> Vec<String> {
    render_args_enc(segments, audio, out, w, h, fps, fit, VideoEncoder::X264)
}

/// The video encoder a render uses.
///
/// `libx264` was HARDCODED, and Fedora's ffmpeg (`ffmpeg-free`) does not have
/// it -- licensing -- so on those machines every video export failed. The
/// fleet already knew: fao-codec's test fixtures moved off libx264 for the same
/// Fedora boxes. Fedora does ship `libopenh264` (still H.264, plays anywhere),
/// and `mpeg4` exists in every ffmpeg build as the last resort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    X264,
    OpenH264,
    Mpeg4,
}

impl VideoEncoder {
    /// Best first.
    pub const PREFERENCE: [VideoEncoder; 3] = [VideoEncoder::X264, VideoEncoder::OpenH264, VideoEncoder::Mpeg4];

    /// ffmpeg's name for it.
    pub fn name(self) -> &'static str {
        match self {
            VideoEncoder::X264 => "libx264",
            VideoEncoder::OpenH264 => "libopenh264",
            VideoEncoder::Mpeg4 => "mpeg4",
        }
    }

    /// Codec and quality arguments. Each encoder takes quality differently:
    /// x264 a constant-quality factor, openh264 only a bitrate, mpeg4 a
    /// quantiser -- the same flag on all three would be ignored by two.
    pub fn codec_args(self) -> Vec<String> {
        let a: &[&str] = match self {
            VideoEncoder::X264 => &["-c:v", "libx264", "-crf", "20", "-preset", "medium"],
            VideoEncoder::OpenH264 => &["-c:v", "libopenh264", "-b:v", "8M"],
            VideoEncoder::Mpeg4 => &["-c:v", "mpeg4", "-q:v", "3"],
        };
        a.iter().map(|s| s.to_string()).collect()
    }

    /// The best encoder in an `ffmpeg -encoders` listing. Pure, so a Fedora
    /// listing can be tested on a machine that has libx264. Matches the NAME
    /// column exactly: "libx264" must not match "libx264rgb".
    pub fn pick(listing: &str) -> Option<VideoEncoder> {
        let names: Vec<&str> = listing.lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let flags = it.next()?;
                (flags.starts_with('V') && flags.len() == 6).then(|| it.next()).flatten()
            })
            .collect();
        VideoEncoder::PREFERENCE.into_iter().find(|e| names.contains(&e.name()))
    }

    /// Ask this machine's ffmpeg, once.
    pub fn detect() -> Option<VideoEncoder> {
        static FOUND: std::sync::OnceLock<Option<VideoEncoder>> = std::sync::OnceLock::new();
        *FOUND.get_or_init(|| {
            let out = std::process::Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().ok()?;
            VideoEncoder::pick(&String::from_utf8_lossy(&out.stdout))
        })
    }
}

/// [`render_args_with`] with the encoder explicit.
#[allow(clippy::too_many_arguments)]
pub fn render_args_enc(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32, fit: Fit,
                       encoder: VideoEncoder) -> Vec<String> {
    let mut args: Vec<String> = vec!["-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into()];
    // One input per distinct file, in first-use order.
    let mut inputs: Vec<PathBuf> = Vec::new();
    for s in segments {
        if let Segment::Cut(c) = s {
            if !inputs.contains(&c.path) { inputs.push(c.path.clone()); }
        }
    }
    for p in &inputs { args.push("-i".into()); args.push(p.to_string_lossy().into_owned()); }
    let audio_index = inputs.len();
    if let Some(a) = audio { args.push("-i".into()); args.push(a.to_string_lossy().into_owned()); }
    // The fit chain, then the invariants every cut needs regardless of it:
    // square pixels, one frame rate, one pixel format -- concat refuses inputs
    // that disagree on any of them.
    //
    // MERGED: the film pipeline added `fit_rgb` for cuts with picture effects
    // (processed in RGB -- the colour maths is per RGB channel, and cropping
    // 4:2:0 to odd sizes would shift chroma). It hardcoded the old Contain
    // chain; built from `fit.chain` here, so Cover and Stretch reach
    // effect-bearing cuts too instead of being silently ignored. Built BEFORE
    // `fit` is shadowed by the String below, which would hide `.chain()`.
    let chain = fit.chain(w, h);
    let fit_rgb = format!("{chain},setsar=1,fps={fps},format=rgb24");
    let fit = format!("{chain},setsar=1,fps={fps},format=yuv420p");
    let mut filter = String::new();
    let mut labels = Vec::new();
    for (i, s) in segments.iter().enumerate() {
        let label = format!("[s{i}]");
        match s {
            Segment::Cut(c) if !c.picture.is_identity() => {
                let input = inputs.iter().position(|p| *p == c.path).expect("collected above");
                let (chain, at) = c.picture.filters(w, h);
                let base = format!("[{input}:v]trim=start={:.6}:duration={:.6},setpts=PTS-STARTPTS,{fit_rgb}{chain}",
                                   c.src_in.to_seconds(), c.len.to_seconds());
                match at {
                    None => filter.push_str(&format!("{base},format=yuv420p{label};")),
                    Some((x, y)) => filter.push_str(&format!(
                        "{base}[fg{i}];color=c=black:s={w}x{h}:r={fps}:d={:.6},format=rgb24[bg{i}];\
[bg{i}][fg{i}]overlay=x={x}:y={y}:shortest=1:format=rgb,format=yuv420p{label};", c.len.to_seconds())),
                }
            }
            Segment::Cut(c) => {
                let input = inputs.iter().position(|p| *p == c.path).expect("collected above");
                filter.push_str(&format!(
                    "[{input}:v]trim=start={:.6}:duration={:.6},setpts=PTS-STARTPTS,{fit}{label};",
                    c.src_in.to_seconds(), c.len.to_seconds()));
            }
            Segment::Black { len, .. } => {
                filter.push_str(&format!(
                    "color=c=black:s={w}x{h}:r={fps}:d={:.6},format=yuv420p{label};", len.to_seconds()));
            }
        }
        labels.push(label);
    }
    filter.push_str(&format!("{}concat=n={}:v=1:a=0[vout]", labels.concat(), segments.len()));
    args.push("-filter_complex".into());
    args.push(filter);
    args.push("-map".into()); args.push("[vout]".into());
    if audio.is_some() {
        args.push("-map".into()); args.push(format!("{audio_index}:a"));
        args.push("-c:a".into()); args.push("aac".into());
        args.push("-shortest".into());
    }
    args.extend(encoder.codec_args());
    args.extend(["-pix_fmt".into(), "yuv420p".into(), "-movflags".into(), "+faststart".into()]);
    args.push(out.to_string_lossy().into_owned());
    args
}

/// Everything the export will NOT reproduce from the preview, one line per
/// cut that has any, for the status line. Empty means the file will match.
pub fn unsupported(segments: &[Segment]) -> Vec<String> {
    segments.iter().filter_map(|s| match s {
        Segment::Cut(c) if !c.picture.unsupported.is_empty() => Some(format!(
            "{} at {:.2}s: {} not rendered", c.path.file_name().map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(), c.at.to_seconds(), c.picture.unsupported.join(", "))),
        _ => None,
    }).collect()
}

/// Render `segments` with `audio` under it to `out`. `segments` must be
/// non-empty (a project with no duration has no picture to render).
pub fn render(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32)
    -> Result<(), super::VideoError>
{
    if segments.is_empty() {
        return Err(super::VideoError::UnexpectedOutput("edit list is empty: nothing on the timeline".into()));
    }
    let encoder = VideoEncoder::detect().ok_or_else(|| super::VideoError::UnexpectedOutput(
        "this ffmpeg has none of libx264, libopenh264 or mpeg4 -- cannot write video".into()))?;
    render_with_encoder(segments, audio, out, w, h, fps, encoder)
}

/// [`render`] with the encoder chosen by the caller.
pub fn render_with_encoder(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32,
                           encoder: VideoEncoder) -> Result<(), super::VideoError> {
    if segments.is_empty() {
        return Err(super::VideoError::UnexpectedOutput("edit list is empty: nothing on the timeline".into()));
    }
    let args = render_args_enc(segments, audio, out, w, h, fps, Fit::default(), encoder);
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    super::run("ffmpeg", &refs).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, Track, TrackId};

    fn project_with(paths: &BTreeMap<SourceId, PathBuf>) -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "cam A".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Video, name: "cam B".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(3), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 2 }).unwrap();
        for id in paths.keys() {
            p.apply(&Op::AddSource { source: Source { id: *id, path: paths[id].to_string_lossy().into_owned(),
                len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1, has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        }
        p
    }
    fn clip(p: &mut Project, id: u64, source: u64, track: u64, pos: f64, len: f64, src_in: f64) {
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(source), track: TrackId(track),
            pos: Ticks::from_seconds(pos), src_in: Ticks::from_seconds(src_in), len: Ticks::from_seconds(len),
            gain: 1.0, effects: vec![] } }).unwrap();
    }
    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }

    #[test]
    fn the_top_track_wins_an_overlap_and_gaps_are_black() {
        let paths: BTreeMap<SourceId, PathBuf> =
            [(SourceId(1), "a.mp4".into()), (SourceId(2), "b.mp4".into()), (SourceId(3), "c.wav".into())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 2.0, 0.0);   // cam A: 0–2 from a.mp4
        clip(&mut p, 2, 2, 2, 1.0, 3.0, 0.5);   // cam B: 1–4 from b.mp4 (from 0.5 in)
        clip(&mut p, 3, 2, 2, 5.0, 1.0, 0.0);   // cam B: 5–6
        clip(&mut p, 4, 3, 3, 0.0, 7.0, 0.0);   // audio: not picture
        let list = edit_list(&p, &paths, None);
        let shape: Vec<(f64, f64, Option<(&str, f64)>)> = list.iter().map(|seg| match seg {
            Segment::Cut(c) => (c.at.to_seconds(), c.len.to_seconds(),
                                Some((c.path.to_str().unwrap(), c.src_in.to_seconds()))),
            Segment::Black { at, len } => (at.to_seconds(), len.to_seconds(), None),
        }).collect();
        // A wins 0–2 outright (top track), B shows 2–4 from 1.5 s in
        // (its own 0.5 plus the second it lost), black 4–5, B 5–6, black
        // to the project's end at 7 (the audio clip's).
        let want: Vec<(f64, f64, Option<(&str, f64)>)> = vec![
            (0.0, 2.0, Some(("a.mp4", 0.0))),
            (2.0, 2.0, Some(("b.mp4", 1.5))),
            (4.0, 1.0, None),
            (5.0, 1.0, Some(("b.mp4", 0.0))),
            (6.0, 1.0, None),
        ];
        for (g, w) in shape.iter().zip(&want) {
            assert!((g.0 - w.0).abs() < 1e-6 && (g.1 - w.1).abs() < 1e-6, "{shape:?}");
            match (&g.2, &w.2) {
                (Some((gp, gi)), Some((wp, wi))) => { assert_eq!(gp, wp); assert!((gi - wi).abs() < 1e-6, "{shape:?}"); }
                (None, None) => {}
                _ => panic!("{shape:?}"),
            }
        }
        assert_eq!(shape.len(), want.len(), "{shape:?}");
        // Contiguous: every segment starts where the last ended.
        for w in list.windows(2) { assert_eq!(w[0].end(), w[1].at()); }
        // A source with no file yet is a gap, not a crash.
        let fewer: BTreeMap<SourceId, PathBuf> = [(SourceId(2), "b.mp4".into())].into();
        let list = edit_list(&p, &fewer, Some(s(4.0)));
        assert!(matches!(list[0], Segment::Black { .. }), "cam A's file is not here: black where it was");
        assert!(matches!(&list[1], Segment::Cut(c) if c.at == s(1.0) && c.len == s(3.0)), "{list:?}");
        assert!(edit_list(&Project::default(), &paths, None).is_empty());
    }

    #[test]
    fn the_command_names_each_file_once_and_concatenates_in_order() {
        let a: PathBuf = "a.mp4".into();
        let segs = vec![
            Segment::Cut(Cut { path: a.clone(), src_in: s(0.0), len: s(1.0), at: s(0.0), picture: Picture::default() }),
            Segment::Black { at: s(1.0), len: s(0.5) },
            Segment::Cut(Cut { path: a.clone(), src_in: s(3.0), len: s(1.0), at: s(1.5), picture: Picture::default() }),
        ];
        let args = render_args(&segs, Some(Path::new("mix.wav")), Path::new("out.mp4"), 640, 360, 25);
        let inputs: Vec<&String> = args.windows(2).filter(|w| w[0] == "-i").map(|w| &w[1]).collect();
        assert_eq!(inputs, vec!["a.mp4", "mix.wav"], "a.mp4 once, though it is cut twice; the audio last");
        let fc = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(fc.contains("[0:v]trim=start=0.000000:duration=1.000000"), "{fc}");
        assert!(fc.contains("[0:v]trim=start=3.000000:duration=1.000000"), "{fc}");
        assert!(fc.contains("color=c=black:s=640x360:r=25:d=0.500000"), "{fc}");
        assert!(fc.ends_with("[s0][s1][s2]concat=n=3:v=1:a=0[vout]"), "{fc}");
        assert!(args.windows(2).any(|w| w[0] == "-map" && w[1] == "1:a"), "the audio is input 1");
        assert!(args.contains(&"-shortest".to_string()));
        assert_eq!(args.last().unwrap(), "out.mp4");
        let silent = render_args(&segs, None, Path::new("out.mp4"), 640, 360, 25);
        assert!(!silent.contains(&"-shortest".to_string()) && !silent.iter().any(|a| a == "mix.wav"));
    }

    #[test]
    fn a_wav_transcodes_to_mp3_and_m4a_that_probe_as_audio_of_the_same_length() {
        if !super::super::tests::has_ffmpeg() { return; }
        let enc = super::super::run("ffmpeg", &["-hide_banner", "-encoders"]).unwrap();
        let enc = String::from_utf8_lossy(&enc);
        let tmp = std::env::temp_dir();
        let wav = tmp.join(format!("studio-tc-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..96_000).map(|i| (i as f32 * 0.03).sin() * 0.3).collect();
        std::fs::write(&wav, studio_audio::export::export_wav_dithered_bytes(&tone, 2, 48_000, 1).unwrap()).unwrap();
        for (codec, ext) in [("libmp3lame", "mp3"), ("aac", "m4a")] {
            if !enc.contains(&format!(" {codec} ")) { continue; }
            let out = tmp.join(format!("studio-tc-{}.{ext}", std::process::id()));
            super::super::transcode_audio(&wav, &out, codec, 160_000).expect(codec);
            // `probe` is a video probe (an audio-only file is NoVideoStream
            // to it, correctly); the app's decoder is the reader here.
            let back = studio_media::decode_file(&out).expect("studio-media reads it back");
            assert_eq!(back.channels.len(), 2, "{ext}: stereo");
            let secs = back.frames() as f64 / back.sample_rate as f64;
            assert!((secs - 1.0).abs() < 0.15, "{ext}: a second, got {secs}");
            let size = std::fs::metadata(&out).unwrap().len();
            assert!(size > 5_000 && size < 60_000, "{ext}: 160 kbps of a second is ~20 KB, got {size}");
            let _ = std::fs::remove_file(&out);
        }
        assert!(super::super::transcode_audio(&wav, &tmp.join("x.bin"), "no-such-codec", 1000).is_err(),
                "a codec ffmpeg lacks fails, with its stderr");
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn a_two_camera_cut_renders_and_the_frames_are_the_right_cameras() {
        if !super::super::tests::has_ffmpeg() { return; }
        // Two one-second fixtures of solid colour, no audio.
        let fixture = |colour: &str| -> PathBuf {
            let path = std::env::temp_dir().join(format!("studio-edit-{colour}-{}-{}.mp4", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
            let src = format!("color=c={colour}:s=64x48:r=10:d=1");
            // mpeg4, not libx264: the fixture's own codec is irrelevant here, and
            // libx264 made this test fail before testing anything on Fedora.
            super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &src, "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p",
                "-an", path.to_str().unwrap()]).expect("fixture");
            path
        };
        let red = fixture("red");
        let blue = fixture("blue");
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), red.clone()), (SourceId(2), blue.clone())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 0.5, 0.0);   // red 0–0.5
        clip(&mut p, 2, 2, 2, 0.5, 0.5, 0.2);   // blue 0.5–1.0
        clip(&mut p, 3, 1, 1, 1.5, 0.5, 0.3);   // red 1.5–2.0, black 1.0–1.5 between
        let list = edit_list(&p, &paths, None);
        assert_eq!(list.len(), 4);
        let out = std::env::temp_dir().join(format!("studio-edit-out-{}.mp4", std::process::id()));
        // A mixdown to go under it: 2 s of a quiet tone, as WAV.
        let wav = std::env::temp_dir().join(format!("studio-edit-mix-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..96_000).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        std::fs::write(&wav, studio_audio::export::export_wav_dithered_bytes(&tone, 1, 48_000, 1).unwrap()).unwrap();
        render(&list, Some(&wav), &out, 64, 48, 10).expect("render");
        let probe = super::super::probe(&out).expect("probe the render");
        assert!((probe.duration.to_seconds() - 2.0).abs() < 0.15, "two seconds of picture: {:?}", probe.duration);
        assert!(probe.has_audio, "the mixdown is under it");
        let colour_at = |t: f64| -> (u8, u8, u8) {
            let px = super::super::frame_rgb(&out, s(t), 8, 6).unwrap();
            // The centre pixel of an 8×6 frame.
            let i = (3 * 8 + 4) * 3;
            (px[i], px[i + 1], px[i + 2])
        };
        let is = |c: (u8, u8, u8), r: bool, g: bool, b: bool| {
            (c.0 > 128) == r && (c.1 > 128) == g && (c.2 > 128) == b
        };
        assert!(is(colour_at(0.25), true, false, false), "0.25 s: red, got {:?}", colour_at(0.25));
        assert!(is(colour_at(0.75), false, false, true), "0.75 s: blue, got {:?}", colour_at(0.75));
        assert!(is(colour_at(1.25), false, false, false), "1.25 s: black, got {:?}", colour_at(1.25));
        assert!(is(colour_at(1.75), true, false, false), "1.75 s: red again, got {:?}", colour_at(1.75));
        for f in [red, blue, out, wav] { let _ = std::fs::remove_file(f); }
    }

    /// A real `ffmpeg -encoders` line has a six-character flags column; the
    /// name is the next field.
    fn listing(names: &[&str]) -> String {
        let mut l = String::from("Encoders:\n V..... = Video\n ------\n");
        for n in names { l.push_str(&format!(" V....D {n:<20} some description\n")); }
        l.push_str(" A....D aac                  AAC (Advanced Audio Coding)\n");
        l
    }

    #[test]
    fn the_best_available_video_encoder_is_picked() {
        assert_eq!(VideoEncoder::pick(&listing(&["libx264", "libopenh264", "mpeg4"])), Some(VideoEncoder::X264));
        // FEDORA's ffmpeg-free: no libx264 -- H.264 still, through openh264.
        assert_eq!(VideoEncoder::pick(&listing(&["libopenh264", "libsvtav1", "mpeg4"])), Some(VideoEncoder::OpenH264));
        // libx264rgb is a DIFFERENT encoder; a substring match would take it.
        assert_eq!(VideoEncoder::pick(&listing(&["libx264rgb", "mpeg4"])), Some(VideoEncoder::Mpeg4));
        assert_eq!(VideoEncoder::pick(&listing(&["libsvtav1"])), None);
        // The legend line " V..... = Video" must not register an encoder "=".
        assert_eq!(VideoEncoder::pick(" V..... = Video\n"), None);
    }

    /// And against THIS machine's real listing, so the parser is checked on
    /// what ffmpeg actually prints, not only on a hand-made imitation.
    #[test]
    fn this_machines_ffmpeg_listing_parses() {
        if !crate::testkit::ffmpeg_or_skip("this_machines_ffmpeg_listing_parses") { return; }
        assert!(VideoEncoder::detect().is_some(), "some encoder found in a real ffmpeg -encoders listing");
    }

    /// THE FALLBACK REALLY RENDERS: the same three-cut edit through mpeg4, as
    /// a machine with neither libx264 nor openh264 would -- the file is mpeg4,
    /// the right length, and shows the right picture at the right times.
    #[test]
    fn an_export_without_libx264_still_renders_the_edit() {
        if !crate::testkit::ffmpeg_or_skip("an_export_without_libx264_still_renders_the_edit") { return; }
        let dir = std::env::temp_dir().join(format!("studio-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = |colour: &str| {
            let path = dir.join(format!("{colour}.mp4"));
            super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &format!("color=c={colour}:s=64x48:r=10:d=1"),
                "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-an", path.to_str().unwrap()]).expect("fixture");
            path
        };
        let (red, blue) = (fixture("red"), fixture("blue"));
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), red), (SourceId(2), blue)].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 0.5, 0.0);
        clip(&mut p, 2, 2, 2, 0.5, 0.5, 0.2);
        clip(&mut p, 3, 1, 1, 1.5, 0.5, 0.3);
        let list = edit_list(&p, &paths, None);
        let out = dir.join("out.mp4");
        render_with_encoder(&list, None, &out, 64, 48, 10, VideoEncoder::Mpeg4).expect("render through mpeg4");
        let codec = std::process::Command::new("ffprobe").args(["-v", "error", "-select_streams", "v:0",
            "-show_entries", "stream=codec_name", "-of", "csv=p=0"]).arg(&out).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&codec.stdout).trim(), "mpeg4");
        let probe = super::super::probe(&out).expect("probe");
        assert!((probe.duration.to_seconds() - 2.0).abs() < 0.15, "{:?}", probe.duration);
        let centre = |t: f64| { let px = super::super::frame_rgb(&out, s(t), 8, 6).unwrap(); let i = (3 * 8 + 4) * 3; (px[i], px[i + 1], px[i + 2]) };
        let (r, b, k) = (centre(0.25), centre(0.75), centre(1.25));
        assert!(r.0 > 128 && r.2 < 128, "red at 0.25 s: {r:?}");
        assert!(b.2 > 128 && b.0 < 128, "blue at 0.75 s: {b:?}");
        assert!(k.0 < 60 && k.1 < 60 && k.2 < 60, "black gap at 1.25 s: {k:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE DEFAULT MUST NOT CHANGE. This pipeline has always fit-inside-and-
    /// padded; a different default would silently re-render every existing
    /// project.
    #[test]
    fn the_default_fit_is_what_the_pipeline_always_did() {
        assert_eq!(Fit::default(), Fit::Contain);
        let c = Fit::Contain.chain(1920, 1080);
        assert!(c.contains("force_original_aspect_ratio=decrease"), "{c}");
        assert!(c.contains("pad=1920:1080:(ow-iw)/2:(oh-ih)/2"), "centred: {c}");
        assert!(!c.contains("crop"), "Contain never crops: {c}");
    }

    /// Cover fills and crops; the distinction from Contain is `increase`
    /// versus `decrease`, which is one word and the entire difference.
    #[test]
    fn cover_fills_the_frame_and_crops_rather_than_padding() {
        let c = Fit::Cover.chain(1920, 1080);
        assert!(c.contains("force_original_aspect_ratio=increase"), "{c}");
        assert!(c.contains("crop=1920:1080"), "{c}");
        assert!(!c.contains("pad="), "Cover leaves no bars: {c}");
    }

    /// Stretch ignores aspect entirely -- no ratio flag at all, which is what
    /// makes it the one that distorts.
    #[test]
    fn stretch_does_not_preserve_aspect() {
        let c = Fit::Stretch.chain(1920, 1080);
        assert_eq!(c, "scale=1920:1080");
        assert!(!c.contains("force_original_aspect_ratio"), "{c}");
    }

    /// THE BARS STAY COMPUTED. No policy writes black pixels into a source --
    /// bake them and a later aspect change cannot undo them, and an already-
    /// letterboxed source gets bars inside bars.
    #[test]
    fn no_fit_bakes_anything_into_the_source() {
        for f in [Fit::Contain, Fit::Cover, Fit::Stretch] {
            let c = f.chain(640, 360);
            assert!(!c.contains("drawbox"), "{f:?} draws: {c}");
            assert!(!c.contains("overlay"), "{f:?} composites: {c}");
        }
    }

    /// The invariants concat needs ride on every policy, or cuts of different
    /// shapes refuse to join.
    #[test]
    fn every_policy_still_gets_square_pixels_and_one_rate() {
        for f in [Fit::Contain, Fit::Cover, Fit::Stretch] {
            let segs = vec![Segment::Cut(Cut {
                path: PathBuf::from("a.mp4"), src_in: Ticks(0),
                len: Ticks::from_seconds(1.0), at: Ticks(0),
                // `picture` arrived with the film pipeline (merged); these
                // fit tests predate it and render no effects.
                picture: Picture::default(),
            })];
            let args = render_args_with(&segs, None, Path::new("o.mp4"), 640, 360, 25, f);
            let filter = args.join(" ");
            assert!(filter.contains("setsar=1"), "{f:?}");
            assert!(filter.contains("fps=25"), "{f:?}");
            assert!(filter.contains("format=yuv420p"), "{f:?}");
        }
    }

    /// `render_args` keeps its old behaviour exactly, so existing callers and
    /// their tests mean the same thing.
    #[test]
    fn the_old_entry_point_is_unchanged() {
        let segs = vec![Segment::Cut(Cut {
            path: PathBuf::from("a.mp4"), src_in: Ticks(0),
            len: Ticks::from_seconds(1.0), at: Ticks(0),
            picture: Picture::default(),
        })];
        let old = render_args(&segs, None, Path::new("o.mp4"), 640, 360, 25);
        let new = render_args_with(&segs, None, Path::new("o.mp4"), 640, 360, 25, Fit::Contain);
        assert_eq!(old, new);
    }

    fn fx(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
        studio_core::Effect { kind: kind.into(), enabled: true,
            params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    #[test]
    fn picture_effects_reach_the_edit_list_and_unrenderable_ones_are_named() {
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), "a.mp4".into())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 2.0, 0.0);
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![
            fx("color", &[("gain_r", 0.5)]),
            fx("transform", &[("scale_x", 0.5), ("scale_y", 0.5), ("rotation", 10.0)]),
            fx("blend", &[]),
            fx("eq", &[]),                                     // audio: not the picture's business
            studio_core::Effect { enabled: false, ..fx("transform", &[("x", 0.3)]) },  // disabled: ignored
        ] }).unwrap();
        let list = edit_list(&p, &paths, None);
        let Segment::Cut(c) = &list[0] else { panic!("{list:?}") };
        assert_eq!(c.picture.color.unwrap().gain[0], 0.5);
        assert_eq!(c.picture.transform.unwrap().scale_x, 0.5);
        assert_eq!(c.picture.transform.unwrap().x, 0.0, "the disabled transform did not win");
        let warn = unsupported(&list);
        assert_eq!(warn.len(), 1, "{warn:?}");
        assert!(warn[0].contains("rotation") && warn[0].contains("blend"), "{warn:?}");
        // identity effects cost nothing: the cut takes the plain path
        let plain = Picture::of(&[fx("transform", &[]), fx("color", &[])]);
        assert!(plain.is_identity());
        let args = render_args(&[Segment::Cut(Cut { path: "a.mp4".into(), src_in: s(0.0), len: s(1.0),
            at: s(0.0), picture: plain })], None, Path::new("o.mp4"), 64, 36, 12);
        assert!(!args.iter().any(|a| a.contains("overlay") || a.contains("lutrgb")), "{args:?}");
    }

    /// The export must show what the viewport shows. Render a transformed,
    /// graded cut through ffmpeg and compare every pixel against the
    /// viewport's own compositor fed the same source frame.
    #[test]
    fn the_export_matches_the_viewport_for_transform_and_colour() {
        if !crate::testkit::ffmpeg_or_skip("the_export_matches_the_viewport_for_transform_and_colour") { return; }
        let shoot = crate::testkit::shoot(&[1.0], 4.0);
        let take = &shoot.takes[0];
        let (w, h) = (64u32, 36u32);
        let t = studio_fx::Transform { x: 0.1, y: -0.05, scale_x: 0.5, scale_y: 0.6,
            crop: (0.2, 0.0, 0.0, 0.1), ..Default::default() };
        let c = studio_fx::Color { lift: [0.0, 0.0, 0.1], gamma: [1.0, 1.6, 1.0],
            gain: [0.6, 1.0, 1.0], saturation: 0.7 };
        let picture = Picture { transform: Some(t), color: Some(c), kaleido: None, style: None, unsupported: vec![] };
        let seg = Segment::Cut(Cut { path: take.video.clone(), src_in: s(1.0), len: s(1.0), at: s(0.0), picture });
        let out = shoot.dir.join(format!("fx-{}.mp4", std::process::id()));
        render(&[seg], None, &out, w, h, 12).expect("render");
        let got = super::super::frame_rgb(&out, s(0.5), w, h).unwrap();
        // The viewport's path: the source frame, graded, through the compositor.
        let src = super::super::frame_rgb(&take.video, s(1.5), w, h).unwrap();
        let graded: Vec<u8> = src.chunks(3).flat_map(|p| {
            let o = c.apply([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]);
            o.map(|v| (v * 255.0 + 0.5) as u8)
        }).collect();
        let want = studio_fx::compositor::composite(w, h, [0, 0, 0], &[studio_fx::compositor::Layer {
            rgb: &graded, w, h, transform: t, mode: studio_fx::compositor::BlendMode::Normal, opacity: 1.0 }]);
        // Coverage: which pixels are picture and which are black must agree
        // away from edges (bicubic vs nearest disagree on a 1-px border).
        let lit = |px: &[u8], i: usize| px[i] as u32 + px[i + 1] as u32 + px[i + 2] as u32 > 60;
        let mut inside = 0;
        let mut disagree = 0;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 3;
                if lit(&want, i) { inside += 1; }
                if lit(&want, i) != lit(&got, i) { disagree += 1; }
            }
        }
        assert!(inside > 200 && inside < (w * h) as usize - 200, "the transform must leave both picture and black: {inside}");
        let bbox = |px: &[u8]| {
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for y in 0..h as usize { for x in 0..w as usize {
                if lit(px, (y * w as usize + x) * 3) { x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y); }
            }}
            (x0, y0, x1, y1)
        };
        // Placement must be exact: same box, not merely similar. (overlay's
        // default yuv420 format rounds x/y to even — it put this box one
        // pixel up and left, which a pixel-count tolerance let through.)
        assert_eq!(bbox(&got), bbox(&want), "placed box");
        assert!(disagree <= 40, "{disagree} pixels disagree on coverage");
        // Colour, at the centre of the placed picture: within codec error.
        let (cx, cy) = (((0.5 + 0.1) * w as f64) as usize, ((0.5 - 0.05) * h as f64) as usize);
        let i = (cy * w as usize + cx) * 3;
        for k in 0..3 {
            assert!((got[i + k] as i32 - want[i + k] as i32).abs() <= 8,
                    "channel {k}: export {:?} vs viewport {:?}", &got[i..i + 3], &want[i..i + 3]);
        }
        // and the grade actually did something measurable
        assert!((want[i] as i32 - take.rgb[0] as i32).abs() > 40, "{:?} vs source {:?}", &want[i..i + 3], take.rgb);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn a_kaleidoscope_cut_exports_what_the_preview_folds() {
        if !crate::testkit::ffmpeg_or_skip("a_kaleidoscope_cut_exports_what_the_preview_folds") { return; }
        use crate::framepipe::{Pipe, Size};
        let shoot = crate::testkit::shoot(&[1.0], 3.0);
        let (w, h) = (96u32, 54u32);
        // a textured source: a flat colour folds to itself and would prove nothing
        let tex = shoot.dir.join(format!("kal-src-{}.mp4", std::process::id()));
        Pipe { input: shoot.takes[0].video.clone(), range: None, size: Size { w, h }, fps: 12.0 }
            .map(&tex, 12, |_, mut f| {
                for y in 0..f.h { for x in 0..f.w {
                    // big cells: both files are lossy 4:2:0 encodes, which smear
                    // edges, and "fold then smear" differs from "smear then fold"
                    // at every edge. An 8x6 checker at this size is ALL edges and
                    // measured 20.6% "different" with nothing wrong in the export.
                    let v = if (x / 24 + y / 18) % 2 == 0 { 230 } else { 30 };
                    f.set(x, y, [v, (x * 255 / f.w) as u8, (y * 255 / f.h) as u8]);
                }}
                f
            }).expect("textured source");
        let k = crate::kaleido::Kaleido { segments: 6, angle: 15.0, cx: 0.5, cy: 0.5 };
        let cut = |picture: Picture| Segment::Cut(Cut { path: tex.clone(), src_in: s(0.5), len: s(1.0), at: s(0.0), picture });
        let plain = shoot.dir.join(format!("kal-plain-{}.mp4", std::process::id()));
        let folded = shoot.dir.join(format!("kal-fold-{}.mp4", std::process::id()));
        render(&[cut(Picture::default())], None, &plain, w, h, 12).expect("plain");
        render(&[cut(Picture { kaleido: Some(k), ..Picture::default() })], None, &folded, w, h, 12).expect("folded");
        let a = super::super::frame_rgb(&plain, s(0.5), w, h).unwrap();
        let b = super::super::frame_rgb(&folded, s(0.5), w, h).unwrap();
        let want = crate::kaleido::apply(&a, w, h, &k);
        let off = b.chunks(3).zip(want.chunks(3))
            .filter(|(x, y)| x.iter().zip(y.iter()).any(|(p, q)| (*p as i32 - *q as i32).abs() > 40)).count();
        let share = off as f32 / (w * h) as f32;
        // a looser bar than the raw geq parity: two lossy encodes sit between them
        assert!(share < 0.08, "{:.1}% of the exported frame differs from the preview's fold", share * 100.0);
        // and it really is folded: not the plain frame
        let same_as_plain = a.chunks(3).zip(b.chunks(3))
            .filter(|(x, y)| x.iter().zip(y.iter()).all(|(p, q)| (*p as i32 - *q as i32).abs() <= 20)).count();
        assert!((same_as_plain as f32) < 0.8 * (w * h) as f32, "the export is not simply the plain picture");
        for f in [&tex, &plain, &folded] { let _ = std::fs::remove_file(f); }
    }

    #[test]
    fn a_styled_cut_exports_what_the_preview_shows() {
        if !crate::testkit::ffmpeg_or_skip("a_styled_cut_exports_what_the_preview_shows") { return; }
        use crate::framepipe::{Pipe, Size};
        let shoot = crate::testkit::shoot(&[1.0], 3.0);
        let (w, h) = (96u32, 54u32);
        let tex = shoot.dir.join(format!("sty-src-{}.mp4", std::process::id()));
        Pipe { input: shoot.takes[0].video.clone(), range: None, size: Size { w, h }, fps: 12.0 }
            .map(&tex, 12, |_, mut f| {
                for y in 0..f.h { for x in 0..f.w {
                    let sky = y < f.h / 2;
                    let v = if sky { 150 + (y * 4) as u8 } else { 40 + (x * 2) as u8 };
                    f.set(x, y, if sky { [v - 30, v - 10, v] } else { [v / 2, v, v / 3] });
                }}
                f
            }).expect("source");
        let a0 = super::super::frame_rgb(&tex, s(1.0), w, h).unwrap();
        // a look to transfer: the same picture pushed warm and bright
        let warm: Vec<u8> = a0.chunks(3).flat_map(|p| [p[0].saturating_add(50), p[1].saturating_add(20), p[2]]).collect();
        let t = crate::style::Transfer::fit(&crate::style::stats(&a0), &crate::style::stats(&warm), 1.0);
        let cut = |picture: Picture| Segment::Cut(Cut { path: tex.clone(), src_in: s(0.5), len: s(1.0), at: s(0.0), picture });
        let (plain, styled) = (shoot.dir.join(format!("sty-p-{}.mp4", std::process::id())), shoot.dir.join(format!("sty-s-{}.mp4", std::process::id())));
        render(&[cut(Picture::default())], None, &plain, w, h, 12).expect("plain");
        render(&[cut(Picture { style: Some(t), ..Picture::default() })], None, &styled, w, h, 12).expect("styled");
        let a = super::super::frame_rgb(&plain, s(0.5), w, h).unwrap();
        let b = super::super::frame_rgb(&styled, s(0.5), w, h).unwrap();
        let mut want = a.clone();
        crate::style::apply_lut(&mut want, &t.lut(crate::style::LUT_N), crate::style::LUT_N);
        let mean = |v: &[u8]| v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64;
        let err = b.iter().zip(&want).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / b.len() as f64;
        assert!(err < 6.0, "export differs from the preview's look by {err:.1} levels on average");
        assert!((mean(&b) - mean(&a)).abs() > 8.0, "and it really changed: {} vs {}", mean(&b), mean(&a));
        for f in [&tex, &plain, &styled] { let _ = std::fs::remove_file(f); }
    }
}
