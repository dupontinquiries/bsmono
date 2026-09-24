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
//! Video tracks stack: the FIRST video track in the project is on top.
//! Where clips overlap in time, the picture is the STACK the viewport's
//! compositor paints (`studio_fx::compositor`): from the top down to the
//! first layer that hides everything under it -- Normal blend, full
//! opacity, no reframe -- painted bottom to top over black. A lone opaque
//! layer is a plain [`Cut`], the old "top one wins"; anything else (a blend
//! mode, a translucent layer, a picture-in-picture showing the track under
//! it) is a [`Segment::Stack`]. A gap where no video track has a clip is
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

use studio_core::{ClipId, Project, SourceId, Ticks, TrackKind};
use studio_fx::compositor::BlendMode;

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
    /// The clip's fade in / out, from the model's fades: an OPACITY ramp. Over
    /// nothing that is a fade to black (the canvas is black); over a clip on
    /// a lower track it is a dissolve into it. Runs after the grade.
    pub fade: Option<Fade>,
    /// The clip's `blend` effect, `(mode, opacity)`; `None` is Normal at full
    /// opacity. Honoured where the layer is composited over the ones under it
    /// ([`Segment::Stack`]); a transform's own `opacity` multiplies in, as in
    /// the compositor.
    pub blend: Option<(BlendMode, f32)>,
    /// Effects that change the picture in the preview but that this export
    /// cannot yet reproduce, named for the person exporting. Never silently
    /// dropped: a file that differs from the preview must say so.
    pub unsupported: Vec<String>,
}

/// A clip's fades, placed on its SOURCE's timeline, in seconds.
///
/// On the source timeline, not the cut's, because `edit_list` can split one
/// clip into several pieces -- a higher track covering part of it, a
/// dissolve starting -- and a piece that starts halfway through a fade must
/// start half-faded. The filters run after `trim` but BEFORE `setpts` resets
/// the clock, so they still see source timestamps and every piece inherits
/// exactly the right level; a fade computed per piece would restart from 0.
///
/// c20 moved them from before the trim to after the picture chain: a fade is
/// an opacity now, and opacity is applied to the finished (graded) layer --
/// so a grade that lifts blacks no longer lifts a faded-out picture.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fade {
    /// Fade in from black over `[in_at, in_at + in_len)`; `in_len` 0 = none.
    pub in_at: f64,
    pub in_len: f64,
    /// Fade out to black over `[out_at, out_at + out_len)`; 0 = none.
    pub out_at: f64,
    pub out_len: f64,
}

impl Fade {
    /// For a clip starting at `src_in` in its source, `len` long, with the
    /// model's `(fade_in, fade_out)`. `None` when both are zero.
    pub fn of(src_in: Ticks, len: Ticks, fade_in: Ticks, fade_out: Ticks) -> Option<Fade> {
        if fade_in.0 <= 0 && fade_out.0 <= 0 {
            return None;
        }
        let (start, l) = (src_in.to_seconds(), len.to_seconds());
        let (fi, fo) = (fade_in.to_seconds().max(0.0), fade_out.to_seconds().max(0.0));
        Some(Fade { in_at: start, in_len: fi, out_at: start + l - fo, out_len: fo })
    }

    /// ffmpeg `fade` filters, each PRECEDED by a comma, for after the picture
    /// chain and before `setpts` (see the type's docs). `alpha` fades the
    /// alpha channel -- a layer in a stack, dissolving into what is under it
    /// -- instead of the picture to black, which is the same thing over black.
    pub fn filters(&self, alpha: bool) -> String {
        let a = if alpha { ":alpha=1" } else { "" };
        let mut f = String::new();
        if self.in_len > 0.0 {
            f.push_str(&format!(",fade=t=in:st={:.6}:d={:.6}{a}", self.in_at, self.in_len));
        }
        if self.out_len > 0.0 {
            f.push_str(&format!(",fade=t=out:st={:.6}:d={:.6}{a}", self.out_at, self.out_len));
        }
        f
    }

    /// Whether any of source `[from, to)` is partly faded.
    pub fn touches(&self, from: f64, to: f64) -> bool {
        let hit = |at: f64, len: f64| len > 0.0 && from < at + len - 1e-9 && at < to - 1e-9;
        hit(self.in_at, self.in_len) || hit(self.out_at, self.out_len)
    }
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
                }
                Some(studio_fx::Instance::Color(c)) => p.color = (!c.is_identity()).then_some(c),
                Some(studio_fx::Instance::Blend(m, o)) =>
                    p.blend = (m != BlendMode::Normal || o < 1.0 - 1e-4).then_some((m, o)),
                Some(studio_fx::Instance::Retime(..)) => p.unsupported.push("retime".into()),
                _ => {}
            }
        }
        p
    }

    pub fn is_identity(&self) -> bool {
        self.transform.is_none() && self.color.is_none() && self.kaleido.is_none() && self.style.is_none()
    }

    /// The layer's blend mode (Normal without a `blend` effect).
    pub fn mode(&self) -> BlendMode { self.blend.map_or(BlendMode::Normal, |(m, _)| m) }

    /// The layer's opacity as the compositor computes it: the blend effect's
    /// dial times the transform's own.
    pub fn alpha(&self) -> f32 {
        let o = self.blend.map_or(1.0, |(_, o)| o) * self.transform.map_or(1.0, |t| t.opacity);
        o.clamp(0.0, 1.0)
    }

    /// Painted over black this is just its picture: Normal, fully opaque.
    pub fn is_opaque(&self) -> bool { self.mode() == BlendMode::Normal && self.alpha() >= 1.0 - 1e-4 }

    /// Hides every layer under it: opaque AND filling the frame (no reframe
    /// or crop). Where this is false the track below shows, as in the preview.
    pub fn covers(&self) -> bool {
        self.is_opaque() && self.transform.map_or(true, |t| studio_fx::Transform { opacity: 1.0, ..t }.is_identity())
    }

    /// [`covers`](Self::covers) over source `[from, to)` seconds: not while
    /// it is fading, when what is under it shows through -- the dissolve.
    pub fn covers_during(&self, from: f64, to: f64) -> bool {
        self.covers() && self.fade.map_or(true, |f| !f.touches(from, to))
    }

    /// The filter chain from a fitted `w`×`h` frame to the finished picture,
    /// as `(foreground chain, placement)`. The foreground chain runs on the
    /// fitted frame; when there is a transform it ends smaller than the
    /// canvas, `(width, height)` of the placement, and is overlaid at its
    /// `(x, y)` — which may lie partly off the canvas.
    fn filters(&self, w: u32, h: u32) -> (String, Option<Placement>) {
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
        // lands at canvas px = ax + (u·W − ax)·sx + ox, then turns by the
        // rotation about the anchor (below).
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
        if t.rotation.abs() <= 1e-4 {
            return (f, Some(Placement { x: px0.round() as i64, y: py0.round() as i64, w: ow as i64, h: oh as i64 }));
        }
        // ROTATION, clockwise (`Transform::rotation`; ffmpeg's `rotate` also
        // turns clockwise for a positive angle), about the ANCHOR. `rotate`
        // turns about the frame's centre into a frame sized to hold the
        // result, corners transparent -- so turn the placed box, then put
        // that frame's centre where turning the box's centre about the
        // anchor sends it.
        let th = (t.rotation as f64).to_radians();
        let (c, s) = (th.cos(), th.sin());
        let (rw, rh) = ((ow * c.abs() + oh * s.abs()).ceil().max(1.0), (ow * s.abs() + oh * c.abs()).ceil().max(1.0));
        f.push_str(&format!(",format=rgba,rotate=a={th:.9}:ow={rw}:oh={rh}:c=none:bilinear=1"));
        let (pvx, pvy) = (ax + t.x as f64 * wf, ay + t.y as f64 * hf);
        let (dx, dy) = (px0 + ow * 0.5 - pvx, py0 + oh * 0.5 - pvy);
        let (ncx, ncy) = (pvx + dx * c - dy * s, pvy + dx * s + dy * c);
        (f, Some(Placement { x: (ncx - rw * 0.5).round() as i64, y: (ncy - rh * 0.5).round() as i64,
                             w: rw as i64, h: rh as i64 }))
    }
}

/// Where a reframed layer lands on the canvas, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement { x: i64, y: i64, w: i64, h: i64 }

impl Placement {
    /// The part of the layer on a `w`×`h` canvas, as
    /// `(crop x, crop y, width, height, canvas x, canvas y)`, or `None` when
    /// it is entirely off. `pad` (which puts it on a canvas-sized frame)
    /// cannot take a negative offset, so what hangs off is cropped first.
    fn visible(self, w: u32, h: u32) -> Option<(i64, i64, i64, i64, i64, i64)> {
        let (x0, y0) = (self.x.max(0), self.y.max(0));
        let (x1, y1) = ((self.x + self.w).min(w as i64), (self.y + self.h).min(h as i64));
        (x1 > x0 && y1 > y0).then_some((x0 - self.x, y0 - self.y, x1 - x0, y1 - y0, x0, y0))
    }
}

/// A segment of the output: a cut, black, or a stack of layers composited.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Cut(Cut),
    Black { at: Ticks, len: Ticks },
    /// Several layers seen at once, BOTTOM FIRST (the compositor's order),
    /// painted over black, each with its blend mode and opacity. Every layer
    /// spans exactly `[at, at + len)`.
    Stack { at: Ticks, len: Ticks, layers: Vec<Cut> },
}

impl Segment {
    pub fn at(&self) -> Ticks {
        match self { Segment::Cut(c) => c.at, Segment::Black { at, .. } | Segment::Stack { at, .. } => *at }
    }
    pub fn len(&self) -> Ticks {
        match self { Segment::Cut(c) => c.len, Segment::Black { len, .. } | Segment::Stack { len, .. } => *len }
    }
    pub fn end(&self) -> Ticks { self.at().saturating_add(self.len()) }
    /// Any source picture in it (not only black).
    pub fn has_picture(&self) -> bool { !matches!(self, Segment::Black { .. }) }
    /// Every cut in it, bottom first.
    pub fn cuts(&self) -> &[Cut] {
        match self {
            Segment::Cut(c) => std::slice::from_ref(c),
            Segment::Black { .. } => &[],
            Segment::Stack { layers, .. } => layers,
        }
    }
    fn lengthen(&mut self, by: Ticks) {
        match self {
            Segment::Cut(c) => c.len = c.len.saturating_add(by),
            Segment::Black { len, .. } => *len = len.saturating_add(by),
            Segment::Stack { len, layers, .. } => {
                *len = len.saturating_add(by);
                for l in layers { l.len = l.len.saturating_add(by); }
            }
        }
    }
}

/// The picture as segments covering `[0, end)` — `end` is the project's
/// duration unless given — with no gaps: black where no video clip is.
/// Only clips whose source has a path in `paths` and sits on a video
/// track count; a video source whose file has not arrived is a gap.
///
/// The timeline is cut at every clip edge; each piece between two edges
/// holds the stack visible there (see the module docs), and neighbouring
/// pieces showing the same clips are one segment -- so a timeline with no
/// overlaps gives exactly one cut per clip, as it always has.
pub fn edit_list(project: &Project, paths: &BTreeMap<SourceId, PathBuf>, end: Option<Ticks>) -> Vec<Segment> {
    let end = end.unwrap_or_else(|| project.duration());
    // Video clips in priority order: track index ascending (first is top);
    // within a track the earlier clip claims an overlap.
    let mut clips: Vec<(usize, &studio_core::Clip, Picture)> = project.clips.values()
        .filter_map(|c| {
            let ti = project.tracks.iter().position(|t| t.id == c.track)?;
            if project.tracks[ti].kind != TrackKind::Video { return None; }
            if !paths.contains_key(&c.source) { return None; }
            let picture = Picture {
                fade: project.fades.get(&c.id).and_then(|(fi, fo)| Fade::of(c.src_in, c.len, *fi, *fo)),
                ..Picture::of(&c.effects)
            };
            Some((ti, c, picture))
        })
        .collect();
    clips.sort_by_key(|(ti, c, _)| (*ti, c.pos));
    let mut edges: Vec<Ticks> = vec![Ticks::ZERO, end];
    for (_, c, _) in &clips {
        // a fade's edges too: a dissolve stacks only while it is fading
        let (fi, fo) = project.fades.get(&c.id).copied().unwrap_or((Ticks::ZERO, Ticks::ZERO));
        let (fin, fout) = (c.pos.saturating_add(fi), c.end().saturating_sub(fo));
        edges.extend([c.pos, c.end(), fin, fout].into_iter().filter(|t| *t >= c.pos && *t <= c.end() && *t < end));
    }
    edges.sort();
    edges.dedup();
    let mut out: Vec<Segment> = Vec::new();
    let mut showing: Option<Vec<ClipId>> = None;
    for pair in edges.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b { continue; }
        // Top down: one clip per track, stopping at the first that hides the rest.
        let mut stack: Vec<&(usize, &studio_core::Clip, Picture)> = Vec::new();
        let mut track = None;
        for entry in &clips {
            let (ti, c, pic) = entry;
            if track == Some(*ti) || !(c.pos <= a && a < c.end()) { continue; }
            track = Some(*ti);
            stack.push(entry);
            let from = c.src_in.saturating_add(a.saturating_sub(c.pos)).to_seconds();
            if pic.covers_during(from, from + b.saturating_sub(a).to_seconds()) { break; }
        }
        let ids: Vec<ClipId> = stack.iter().map(|(_, c, _)| c.id).collect();
        if showing.as_ref() == Some(&ids) {
            out.last_mut().expect("a previous piece set `showing`").lengthen(b.saturating_sub(a));
            continue;
        }
        let cut = |(_, c, pic): &&(usize, &studio_core::Clip, Picture)| Cut {
            path: paths[&c.source].clone(),
            src_in: c.src_in.saturating_add(a.saturating_sub(c.pos)),
            len: b.saturating_sub(a),
            at: a,
            picture: pic.clone(),
        };
        out.push(match stack.as_slice() {
            [] => Segment::Black { at: a, len: b.saturating_sub(a) },
            [one] if one.2.is_opaque() => Segment::Cut(cut(one)),
            _ => Segment::Stack { at: a, len: b.saturating_sub(a), layers: stack.iter().rev().map(cut).collect() },
        });
        showing = Some(ids);
    }
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
    for c in segments.iter().flat_map(|s| s.cuts()) {
        if !inputs.contains(&c.path) { inputs.push(c.path.clone()); }
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
                let fades = c.picture.fade.map(|f| f.filters(false)).unwrap_or_default();
                let base = format!("[{input}:v]trim=start={:.6}:duration={:.6},{fit_rgb}{chain}{fades},setpts=PTS-STARTPTS",
                                   c.src_in.to_seconds(), c.len.to_seconds());
                match at {
                    None => filter.push_str(&format!("{base},format=yuv420p{label};")),
                    Some(Placement { x, y, .. }) => filter.push_str(&format!(
                        "{base}[fg{i}];color=c=black:s={w}x{h}:r={fps}:d={:.6},format=rgb24[bg{i}];\
[bg{i}][fg{i}]overlay=x={x}:y={y}:shortest=1:format=rgb,format=yuv420p{label};", c.len.to_seconds())),
                }
            }
            Segment::Cut(c) => {
                let input = inputs.iter().position(|p| *p == c.path).expect("collected above");
                let fades = c.picture.fade.map(|f| f.filters(false)).unwrap_or_default();
                filter.push_str(&format!(
                    "[{input}:v]trim=start={:.6}:duration={:.6},{fit}{fades},setpts=PTS-STARTPTS{label};",
                    c.src_in.to_seconds(), c.len.to_seconds()));
            }
            Segment::Black { len, .. } => {
                filter.push_str(&format!(
                    "color=c=black:s={w}x{h}:r={fps}:d={:.6},format=yuv420p{label};", len.to_seconds()));
            }
            Segment::Stack { len, layers, .. } => {
                filter.push_str(&stack_filter(i, layers, &inputs, len.to_seconds(), w, h, fps, &fit_rgb));
                filter.push_str(&format!("format=yuv420p,setsar=1{label};"));
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

/// ffmpeg's `blend` name for a mode.
fn blend_name(m: BlendMode) -> &'static str {
    match m {
        BlendMode::Normal => "normal",
        BlendMode::Add => "addition",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
    }
}

/// The graph for one [`Segment::Stack`], `i`th in the list: a black canvas,
/// then each layer painted over it bottom first, ending in an RGB stream
/// with the label open (the caller appends the chain that closes it).
///
/// Every layer is first made a CANVAS-SIZED RGBA frame: its picture chain,
/// its opacity in the alpha channel, and -- when reframed -- padded out with
/// transparency to the canvas, so what it does not cover is alpha 0 and
/// leaves the canvas alone. Then, per layer, the compositor's formula
/// `canvas + (mode(canvas, layer) - canvas) * alpha`:
///
///  * Normal is `overlay` with that alpha -- the formula exactly.
///  * Any other mode is `blend` over the canvas, with the canvas as the
///    FIRST input: ffmpeg's `overlay` blend mode tests its first input
///    ("top") for the dark/light switch, and the compositor tests the canvas.
///    Its alpha plane is `multiply` with the canvas's opaque 255, which hands
///    the layer's alpha through untouched; `overlay` then lays the blended
///    picture over the canvas by it. Multiply, screen and addition are
///    symmetric, so only overlay cared about the order -- tested anyway.
#[allow(clippy::too_many_arguments)]
fn stack_filter(i: usize, layers: &[Cut], inputs: &[PathBuf], len: f64, w: u32, h: u32, fps: u32, fit_rgb: &str) -> String {
    let mut f = format!("color=c=black:s={w}x{h}:r={fps}:d={len:.6},format=rgb24[k{i}_0];");
    let mut canvas = 0;
    for (j, c) in layers.iter().enumerate() {
        let (chain, at) = c.picture.filters(w, h);
        let mut layer = String::new();
        if let Some(p) = at {
            // entirely off the canvas: nothing to paint (and pad would refuse it)
            let Some((cx, cy, vw, vh, x, y)) = p.visible(w, h) else { continue };
            if (cx, cy, vw, vh) != (0, 0, p.w, p.h) { layer.push_str(&format!(",crop={vw}:{vh}:{cx}:{cy}")); }
            layer.push_str(&format!(",pad={w}:{h}:{x}:{y}:color=black@0"));
        }
        let input = inputs.iter().position(|p| *p == c.path).expect("collected by the caller");
        let fades = c.picture.fade.map(|f| f.filters(true)).unwrap_or_default();
        let alpha = c.picture.alpha();
        let dim = if alpha < 1.0 - 1e-4 { format!(",colorchannelmixer=aa={alpha:.6}") } else { String::new() };
        // alpha BEFORE pad, so the padding stays fully transparent
        f.push_str(&format!("[{input}:v]trim=start={:.6}:duration={:.6},{fit_rgb}{chain},format=rgba{fades},setpts=PTS-STARTPTS{dim}{layer}[l{i}_{j}];",
                            c.src_in.to_seconds(), c.len.to_seconds()));
        let (from, to) = (format!("[k{i}_{canvas}]"), format!("[k{i}_{}]", canvas + 1));
        match c.picture.mode() {
            BlendMode::Normal => f.push_str(&format!("{from}[l{i}_{j}]overlay=format=rgb:shortest=1,format=rgb24{to};")),
            m => f.push_str(&format!(
                "{from}format=gbrap,split[ka{i}_{j}][kb{i}_{j}];[l{i}_{j}]format=gbrap[lg{i}_{j}];\
[ka{i}_{j}][lg{i}_{j}]blend=c0_mode={n}:c1_mode={n}:c2_mode={n}:c3_mode=multiply:shortest=1[x{i}_{j}];\
[kb{i}_{j}][x{i}_{j}]overlay=format=rgb:shortest=1,format=rgb24{to};", n = blend_name(m))),
        }
        canvas += 1;
    }
    format!("{f}[k{i}_{canvas}]")
}

/// Everything the export will NOT reproduce from the preview, one line per
/// cut that has any, for the status line. Empty means the file will match.
pub fn unsupported(segments: &[Segment]) -> Vec<String> {
    segments.iter().flat_map(|s| s.cuts()).filter(|c| !c.picture.unsupported.is_empty()).map(|c| format!(
        "{} at {:.2}s: {} not rendered", c.path.file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(), c.at.to_seconds(), c.picture.unsupported.join(", "))).collect()
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
            Segment::Stack { .. } => panic!("opaque full-frame clips never stack: {seg:?}"),
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

    /// A fade reaches the cut, on the SOURCE timeline: a clip at 2 s in its
    /// source, 3 s long, fading in 1 s and out 0.5 s, fades in over source
    /// 2-3 s and out over 4.5-5 s -- after the trim, before the clock resets.
    #[test]
    fn a_clip_fade_reaches_the_export_on_the_source_timeline() {
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), "a.mp4".into())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 3.0, 2.0);
        p.apply(&Op::SetClipFade { id: ClipId(1), fade_in: s(1.0), fade_out: s(0.5) }).unwrap();
        let list = edit_list(&p, &paths, None);
        let Segment::Cut(c) = &list[0] else { panic!("{list:?}") };
        let f = c.picture.fade.expect("the model's fade");
        assert!((f.in_at - 2.0).abs() < 1e-6 && (f.in_len - 1.0).abs() < 1e-6, "{f:?}");
        assert!((f.out_at - 4.5).abs() < 1e-6 && (f.out_len - 0.5).abs() < 1e-6, "{f:?}");
        let fc = render_args(&list, None, Path::new("o.mp4"), 64, 48, 10).into_iter()
            .skip_while(|a| a != "-filter_complex").nth(1).unwrap();
        let (fi, fo, tr, sp) = (fc.find("fade=t=in:st=2.000000").unwrap(), fc.find("fade=t=out:st=4.500000").unwrap(),
                                fc.find("trim=").unwrap(), fc.find("setpts=").unwrap());
        assert!(tr < fi && fo < sp, "fades run after the trim, still on source time (before setpts): {fc}");
        assert!(Fade::of(s(0.0), s(1.0), Ticks::ZERO, Ticks::ZERO).is_none(), "no fade, no filter");
    }

    /// Mean brightness of the centre of a rendered frame at `t`.
    fn luma_at(out: &Path, t: f64) -> f64 {
        let px = super::super::frame_rgb(out, s(t), 8, 6).unwrap();
        px.iter().map(|&v| v as f64).sum::<f64>() / px.len() as f64
    }

    /// RENDERED: a white clip fading in over its first second and out over its
    /// last is dark at its start and end and bright in its middle. And a clip
    /// whose START is covered by a higher track starts HALF-faded where it
    /// first shows -- the case the source-timeline design exists for; a fade
    /// restarted per cut would start that piece from black.
    #[test]
    fn a_rendered_clip_fades_and_a_covered_start_shows_mid_fade() {
        if !crate::testkit::ffmpeg_or_skip("a_rendered_clip_fades_and_a_covered_start_shows_mid_fade") { return; }
        let dir = std::env::temp_dir().join(format!("studio-fade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = |colour: &str| {
            let path = dir.join(format!("{colour}.mp4"));
            super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &format!("color=c={colour}:s=64x48:r=20:d=4"),
                "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-an", path.to_str().unwrap()]).expect("fixture");
            path
        };
        let (white, red) = (fixture("white"), fixture("red"));
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), white), (SourceId(2), red)].into();
        let mut p = project_with(&paths);
        // cam B (lower): white 0-3 s, fading in 2 s and out 1 s.
        clip(&mut p, 1, 1, 2, 0.0, 3.0, 0.0);
        p.apply(&Op::SetClipFade { id: ClipId(1), fade_in: s(2.0), fade_out: s(1.0) }).unwrap();
        // cam A (top): red over 0-1 s, covering the first half of the fade-in.
        clip(&mut p, 2, 2, 1, 0.0, 1.0, 0.0);
        let out = dir.join("out.mp4");
        render_with_encoder(&edit_list(&p, &paths, None), None, &out, 64, 48, 20, VideoEncoder::Mpeg4).expect("render");
        let (at_1_1, mid, near_end) = (luma_at(&out, 1.05), luma_at(&out, 2.0), luma_at(&out, 2.95));
        // White is ~235-255. Half-way through a 2 s fade-in: ~half bright.
        assert!(at_1_1 > 80.0 && at_1_1 < 180.0, "the covered clip appears HALF-faded at 1.05 s, not black: {at_1_1:.0}");
        assert!(mid > 200.0, "fully up by 2 s: {mid:.0}");
        assert!(near_end < 40.0, "faded out by the clip's end: {near_end:.0}");
        let _ = std::fs::remove_dir_all(&dir);
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
            fx("blend", &[("mode", 2.0), ("opacity", 0.5)]),
            fx("eq", &[]),                                     // audio: not the picture's business
            studio_core::Effect { enabled: false, ..fx("transform", &[("x", 0.3)]) },  // disabled: ignored
        ] }).unwrap();
        let list = edit_list(&p, &paths, None);
        // translucent: composited over black, so a stack of one, not a plain cut
        let Segment::Stack { layers, .. } = &list[0] else { panic!("{list:?}") };
        let c = &layers[0];
        assert_eq!(c.picture.color.unwrap().gain[0], 0.5);
        assert_eq!(c.picture.transform.unwrap().scale_x, 0.5);
        assert_eq!(c.picture.transform.unwrap().x, 0.0, "the disabled transform did not win");
        assert_eq!(c.picture.blend, Some((BlendMode::Multiply, 0.5)));
        // blends and rotation are rendered now: nothing to warn about
        let warn = unsupported(&list);
        assert!(warn.is_empty(), "{warn:?}");
        // retiming still is not, and says so
        assert_eq!(Picture::of(&[fx("retime", &[("to_speed", 2.0)])]).unsupported, vec!["retime".to_string()]);
        // a blend effect at its defaults (Normal, full) is no blend at all
        assert_eq!(Picture::of(&[fx("blend", &[])]).blend, None);
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
        let picture = Picture { transform: Some(t), color: Some(c), kaleido: None, style: None, fade: None, blend: None, unsupported: vec![] };
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

    /// `(at, len, clip paths bottom first)` for each segment -- the shape of an
    /// edit list without its pictures. Black is an empty path list.
    fn stacks(list: &[Segment]) -> Vec<(f64, f64, Vec<String>)> {
        list.iter().map(|s| (s.at().to_seconds(), s.len().to_seconds(),
            s.cuts().iter().map(|c| c.path.to_string_lossy().into_owned()).collect())).collect()
    }
    fn names(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn a_reframed_or_translucent_top_layer_lets_the_track_below_show() {
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), "top.mp4".into()), (SourceId(2), "low.mp4".into())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 1.0, 2.0, 0.0);   // top track 1-3
        clip(&mut p, 2, 2, 2, 0.0, 4.0, 0.0);   // lower track 0-4
        // picture-in-picture: half size -- the lower track shows around it
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![fx("transform", &[("scale_x", 0.5), ("scale_y", 0.5)])] }).unwrap();
        let list = edit_list(&p, &paths, None);
        assert_eq!(stacks(&list), vec![
            (0.0, 1.0, names(&["low.mp4"])),
            (1.0, 2.0, names(&["low.mp4", "top.mp4"])),   // bottom first
            (3.0, 1.0, names(&["low.mp4"])),
        ]);
        let Segment::Stack { layers, .. } = &list[1] else { panic!("{list:?}") };
        assert_eq!(layers[0].src_in, s(1.0), "the lower clip's piece starts where it was reached");
        assert_eq!(layers[1].src_in, s(0.0));
        // translucent, full frame: the same stack
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![fx("blend", &[("opacity", 0.5)])] }).unwrap();
        assert_eq!(stacks(&edit_list(&p, &paths, None))[1], (1.0, 2.0, names(&["low.mp4", "top.mp4"])));
        // opaque transform opacity alone counts too (the compositor multiplies it in)
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![fx("transform", &[("opacity", 0.25)])] }).unwrap();
        assert_eq!(stacks(&edit_list(&p, &paths, None))[1].2, names(&["low.mp4", "top.mp4"]));
        // and with no effects the old rule holds exactly: top wins, one cut
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![] }).unwrap();
        assert!(matches!(edit_list(&p, &paths, None)[1], Segment::Cut(_)));
    }

    #[test]
    fn an_opaque_full_frame_layer_hides_everything_under_it_and_does_not_split() {
        let paths: BTreeMap<SourceId, PathBuf> =
            [(SourceId(1), "a.mp4".into()), (SourceId(2), "b.mp4".into()), (SourceId(4), "c.mp4".into())].into();
        let mut p = project_with(&paths);
        p.apply(&Op::AddTrack { track: studio_core::Track { id: TrackId(4), kind: TrackKind::Video, name: "cam C".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 2 }).unwrap();
        clip(&mut p, 1, 1, 1, 0.0, 4.0, 0.0);   // top: translucent 0-4
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![fx("blend", &[("mode", 3.0)])] }).unwrap();
        clip(&mut p, 2, 2, 2, 0.0, 4.0, 0.0);   // middle: opaque, full frame, 0-4
        clip(&mut p, 3, 4, 4, 1.0, 1.0, 0.0);   // bottom: 1-2, hidden by the middle
        let list = edit_list(&p, &paths, None);
        // the bottom clip's edges at 1 and 2 must not cut the stack: nothing visible changes there
        assert_eq!(stacks(&list), vec![(0.0, 4.0, names(&["b.mp4", "a.mp4"]))], "{list:?}");
        let Segment::Stack { layers, len, .. } = &list[0] else { panic!() };
        assert!(layers.iter().all(|l| l.len == *len), "merged pieces lengthen every layer");
    }

    /// Every blend mode, a translucent layer and a picture-in-picture, rendered
    /// through ffmpeg and compared with the viewport's own compositor fed the
    /// same decoded source frames.
    #[test]
    fn every_blend_mode_exports_what_the_compositor_paints() {
        if !crate::testkit::ffmpeg_or_skip("every_blend_mode_exports_what_the_compositor_paints") { return; }
        let dir = std::env::temp_dir().join(format!("studio-blend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (64u32, 48u32);
        // Channels on both sides of 0.5 in each colour, so the overlay mode's
        // dark/light switch goes different ways per channel -- testing it on
        // the WRONG input changes the answer by ~30 levels.
        let fixture = |name: &str, hex: &str| {
            let path = dir.join(format!("{name}.mp4"));
            super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &format!("color=c={hex}:s={w}x{h}:r=20:d=8"),
                "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-an", path.to_str().unwrap()]).expect("fixture");
            path
        };
        let (top, low) = (fixture("top", "0x64B4DC"), fixture("low", "0xC86432"));
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), top.clone()), (SourceId(2), low.clone())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 100, 2, 2, 0.0, 8.0, 0.0);   // the lower track, all the way
        let cases: [(f64, Vec<studio_core::Effect>, BlendMode, f32, studio_fx::Transform); 8] = [
            (0.0, vec![fx("blend", &[("opacity", 0.5)])], BlendMode::Normal, 0.5, Default::default()),
            (1.0, vec![fx("blend", &[("mode", 1.0)])], BlendMode::Add, 1.0, Default::default()),
            (2.0, vec![fx("blend", &[("mode", 2.0)])], BlendMode::Multiply, 1.0, Default::default()),
            (3.0, vec![fx("blend", &[("mode", 3.0)])], BlendMode::Screen, 1.0, Default::default()),
            (4.0, vec![fx("blend", &[("mode", 4.0), ("opacity", 0.75)])], BlendMode::Overlay, 0.75, Default::default()),
            (5.0, vec![fx("transform", &[("scale_x", 0.5), ("scale_y", 0.5)])], BlendMode::Normal, 1.0,
             studio_fx::Transform { scale_x: 0.5, scale_y: 0.5, ..Default::default() }),
            // hanging off the left edge: cropped before pad, and multiplied
            (6.0, vec![fx("transform", &[("scale_x", 0.5), ("scale_y", 0.5), ("x", -0.5)]), fx("blend", &[("mode", 2.0)])],
             BlendMode::Multiply, 1.0, studio_fx::Transform { scale_x: 0.5, scale_y: 0.5, x: -0.5, ..Default::default() }),
            // turned, in a stack: the rotated frame's transparent corners must
            // survive the alpha dial and the pad, and show the track below.
            // Scale 0.8 puts every sample point well inside or outside the
            // turned edge: at 0.6 one sat 3.6 px inside a DIAGONAL edge and
            // read 12 levels off in blue -- 4:2:0 chroma and the codec smear a
            // diagonal colour edge further than the axis-aligned ones above.
            // Multiply, not Screen: screen with black is the canvas unchanged,
            // so opaque black corners would pass unseen (they did, once).
            (7.0, vec![fx("transform", &[("scale_x", 0.8), ("scale_y", 0.8), ("rotation", 40.0)]), fx("blend", &[("mode", 2.0)])],
             BlendMode::Multiply, 1.0, studio_fx::Transform { scale_x: 0.8, scale_y: 0.8, rotation: 40.0, ..Default::default() }),
        ];
        for (k, (at, effects, ..)) in cases.iter().enumerate() {
            clip(&mut p, k as u64 + 1, 1, 1, *at, 1.0, *at);
            p.apply(&Op::SetEffects { id: ClipId(k as u64 + 1), effects: effects.clone() }).unwrap();
        }
        let list = edit_list(&p, &paths, None);
        assert_eq!(list.len(), cases.len(), "one stack per case: {:?}", stacks(&list));
        assert!(list.iter().all(|s| matches!(s, Segment::Stack { layers, .. } if layers.len() == 2)), "{:?}", stacks(&list));
        let out = dir.join("out.mp4");
        render_with_encoder(&list, None, &out, w, h, 20, VideoEncoder::Mpeg4).expect("render");
        let (a, b) = (super::super::frame_rgb(&low, s(0.5), w, h).unwrap(), super::super::frame_rgb(&top, s(0.5), w, h).unwrap());
        let mut checked = 0;
        for (at, _, mode, opacity, transform) in &cases {
            let got = super::super::frame_rgb(&out, s(at + 0.5), w, h).unwrap();
            let want = studio_fx::compositor::composite(w, h, [0, 0, 0], &[
                studio_fx::compositor::Layer { rgb: &a, w, h, transform: Default::default(), mode: BlendMode::Normal, opacity: 1.0 },
                studio_fx::compositor::Layer { rgb: &b, w, h, transform: *transform, mode: *mode, opacity: *opacity },
            ]);
            // interior of the layer AND outside it; one pixel from each placed edge
            // is left out (bicubic vs nearest, and 4:2:0 chroma, disagree there)
            // (3w/8, h/2) is where an off-edge layer lands if it is padded
            // without being cropped first: shifted right by what hung off
            for (x, y) in [(w / 2, h / 2), (3, 3), (w - 4, h - 4), (w / 4 - 3, h / 2), (3 * w / 8, h / 2)] {
                let i = ((y * w + x) * 3) as usize;
                for c in 0..3 {
                    assert!((got[i + c] as i32 - want[i + c] as i32).abs() <= 8,
                        "{mode:?} @{opacity} {transform:?} at ({x},{y}): export {:?} vs compositor {:?}", &got[i..i + 3], &want[i..i + 3]);
                }
                checked += 1;
            }
        }
        assert_eq!(checked, cases.len() * 5);
        // the picture-in-picture really shows the lower track around it (the
        // old export painted black there) and the top inside it
        let pip = super::super::frame_rgb(&out, s(5.5), w, h).unwrap();
        assert!(pip[0] > 150 && pip[2] < 100, "corner is the lower track: {:?}", &pip[..3]);
        let i = (((h / 2) * w + w / 2) * 3) as usize;
        assert!(pip[i + 2] > 180, "centre is the top clip: {:?}", &pip[i..i + 3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fade_over_a_lower_clip_is_a_dissolve_and_stacks_only_while_fading() {
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), "top.mp4".into()), (SourceId(2), "low.mp4".into())].into();
        let mut p = project_with(&paths);
        // incoming on top: the lower clip 0-4, the top clip 2-5 fading in 1 s.
        // The lower clip runs a second PAST the fade, so only the fade's own
        // edge ends the stack there -- without it 2-4 would all be composited
        // (the same picture, since a faded-in layer covers, but the slow path).
        clip(&mut p, 1, 1, 1, 2.0, 3.0, 0.0);
        clip(&mut p, 2, 2, 2, 0.0, 4.0, 0.0);
        p.apply(&Op::SetClipFade { id: ClipId(1), fade_in: s(1.0), fade_out: Ticks::ZERO }).unwrap();
        assert_eq!(stacks(&edit_list(&p, &paths, None)), vec![
            (0.0, 2.0, names(&["low.mp4"])),
            (2.0, 1.0, names(&["low.mp4", "top.mp4"])),   // the dissolve
            (3.0, 2.0, names(&["top.mp4"])),              // faded in: covers again, one cut
        ]);
        // outgoing on top: the top clip 0-3 fading out 1 s over the lower 2-5
        let mut q = project_with(&paths);
        clip(&mut q, 1, 1, 1, 0.0, 3.0, 0.0);
        clip(&mut q, 2, 2, 2, 2.0, 3.0, 0.0);
        q.apply(&Op::SetClipFade { id: ClipId(1), fade_in: Ticks::ZERO, fade_out: s(1.0) }).unwrap();
        assert_eq!(stacks(&edit_list(&q, &paths, None)), vec![
            (0.0, 2.0, names(&["top.mp4"])),
            (2.0, 1.0, names(&["low.mp4", "top.mp4"])),
            (3.0, 2.0, names(&["low.mp4"])),
        ]);
        // a fade over nothing adds edges but no segments: still one cut
        let mut r = project_with(&paths);
        clip(&mut r, 1, 1, 1, 0.0, 4.0, 0.0);
        r.apply(&Op::SetClipFade { id: ClipId(1), fade_in: s(1.0), fade_out: s(1.0) }).unwrap();
        let one = edit_list(&r, &paths, None);
        assert_eq!(stacks(&one), vec![(0.0, 4.0, names(&["top.mp4"]))]);
        assert!(matches!(one[0], Segment::Cut(_)));
    }

    /// RENDERED: red on the lower track, blue on top fading in over it. Half
    /// way through, the picture is half of each -- not a dip through black,
    /// which is what two fades used to give -- and it matches the compositor.
    #[test]
    fn a_rendered_dissolve_mixes_the_two_clips_instead_of_dipping_to_black() {
        if !crate::testkit::ffmpeg_or_skip("a_rendered_dissolve_mixes_the_two_clips_instead_of_dipping_to_black") { return; }
        let dir = std::env::temp_dir().join(format!("studio-dissolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (64u32, 48u32);
        let fixture = |colour: &str| {
            let path = dir.join(format!("{colour}.mp4"));
            super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &format!("color=c={colour}:s={w}x{h}:r=20:d=5"),
                "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-an", path.to_str().unwrap()]).expect("fixture");
            path
        };
        let (blue, red) = (fixture("blue"), fixture("red"));
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), blue.clone()), (SourceId(2), red.clone())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 1.0, 3.0, 0.0);   // blue on top, 1-4, fading in over 1-3
        clip(&mut p, 2, 2, 2, 0.0, 4.0, 0.0);   // red below, 0-4
        p.apply(&Op::SetClipFade { id: ClipId(1), fade_in: s(2.0), fade_out: Ticks::ZERO }).unwrap();
        let out = dir.join("out.mp4");
        render_with_encoder(&edit_list(&p, &paths, None), None, &out, w, h, 20, VideoEncoder::Mpeg4).expect("render");
        let (r, b) = (super::super::frame_rgb(&red, s(0.5), w, h).unwrap(), super::super::frame_rgb(&blue, s(0.5), w, h).unwrap());
        let i = (((h / 2) * w + w / 2) * 3) as usize;
        for (t, g) in [(1.5, 0.25f32), (2.0, 0.5), (2.5, 0.75), (3.5, 1.0)] {
            let got = super::super::frame_rgb(&out, s(t), w, h).unwrap();
            let want = studio_fx::compositor::composite(w, h, [0, 0, 0], &[
                studio_fx::compositor::Layer { rgb: &r, w, h, transform: Default::default(), mode: BlendMode::Normal, opacity: 1.0 },
                studio_fx::compositor::Layer { rgb: &b, w, h, transform: Default::default(), mode: BlendMode::Normal, opacity: g },
            ]);
            for c in 0..3 {
                assert!((got[i + c] as i32 - want[i + c] as i32).abs() <= 10,
                    "at {t} s (fade {g}): export {:?} vs compositor {:?}", &got[i..i + 3], &want[i..i + 3]);
            }
        }
        // the point of a dissolve: at its middle both are there, not black
        let mid = super::super::frame_rgb(&out, s(2.0), w, h).unwrap();
        assert!(mid[i] > 90 && mid[i + 2] > 90, "half red AND half blue at the middle: {:?}", &mid[i..i + 3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The fade is an opacity on the FINISHED layer, so it comes after the
    /// grade: a grade that lifts blacks does not lift a faded-out clip. (Before
    /// c20 the fade ran first and the lift showed through it: ~77 of 255 here.)
    #[test]
    fn a_faded_out_clip_is_black_even_under_a_grade_that_lifts_blacks() {
        if !crate::testkit::ffmpeg_or_skip("a_faded_out_clip_is_black_even_under_a_grade_that_lifts_blacks") { return; }
        let dir = std::env::temp_dir().join(format!("studio-liftfade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("grey.mp4");
        super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", "color=c=gray:s=64x48:r=20:d=3",
            "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-an", src.to_str().unwrap()]).expect("fixture");
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), src)].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 3.0, 0.0);
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![fx("color", &[("lift_r", 0.3), ("lift_g", 0.3), ("lift_b", 0.3)])] }).unwrap();
        p.apply(&Op::SetClipFade { id: ClipId(1), fade_in: Ticks::ZERO, fade_out: s(2.0) }).unwrap();
        let list = edit_list(&p, &paths, None);
        assert!(list[0].cuts()[0].picture.color.is_some(), "the lift reached the cut");
        let out = dir.join("out.mp4");
        render_with_encoder(&list, None, &out, 64, 48, 20, VideoEncoder::Mpeg4).expect("render");
        let (start, end) = (luma_at(&out, 0.5), luma_at(&out, 2.95));
        assert!(start > 150.0, "lifted grey before the fade: {start:.0}");
        assert!(end < 25.0, "faded out to black, the lift faded with it: {end:.0}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rotated, non-square picture-in-picture, rendered and compared with
    /// the compositor pixel by pixel. A rectangle turned the WRONG way covers
    /// a visibly different set of pixels (its bounding box alone would not
    /// tell: +30 and -30 have the same one), so the coverage mask is the check.
    #[test]
    fn a_rotated_layer_exports_where_the_compositor_turns_it() {
        if !crate::testkit::ffmpeg_or_skip("a_rotated_layer_exports_where_the_compositor_turns_it") { return; }
        let dir = std::env::temp_dir().join(format!("studio-rot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (96u32, 54u32);
        let src = dir.join("white.mp4");
        super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &format!("color=c=white:s={w}x{h}:r=12:d=2"),
            "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-an", src.to_str().unwrap()]).expect("fixture");
        let t = studio_fx::Transform { scale_x: 0.6, scale_y: 0.3, rotation: 30.0, x: 0.08, y: -0.04,
                                       anchor_x: 0.4, anchor_y: 0.5, ..Default::default() };
        let picture = Picture { transform: Some(t), ..Picture::default() };
        let seg = Segment::Cut(Cut { path: src.clone(), src_in: s(0.0), len: s(1.0), at: s(0.0), picture });
        let out = dir.join("out.mp4");
        render_with_encoder(&[seg], None, &out, w, h, 12, VideoEncoder::Mpeg4).expect("render");
        let got = super::super::frame_rgb(&out, s(0.5), w, h).unwrap();
        let white = vec![255u8; (w * h * 3) as usize];
        let mask = |t: studio_fx::Transform| studio_fx::compositor::composite(w, h, [0, 0, 0],
            &[studio_fx::compositor::Layer { rgb: &white, w, h, transform: t, mode: BlendMode::Normal, opacity: 1.0 }]);
        let lit = |px: &[u8], i: usize| px[i] as u32 + px[i + 1] as u32 + px[i + 2] as u32 > 380;
        let disagree = |want: &[u8]| (0..(w * h) as usize).filter(|p| lit(want, p * 3) != lit(&got, p * 3)).count();
        let want = mask(t);
        let inside = (0..(w * h) as usize).filter(|p| lit(&want, p * 3)).count();
        assert!(inside > 400 && inside < (w * h) as usize / 2, "a real rectangle, not the frame: {inside}");
        let wrong_way = disagree(&mask(studio_fx::Transform { rotation: -30.0, ..t }));
        let right = disagree(&want);
        // edges: bilinear vs nearest, and 4:2:0 -- a pixel's worth round the rim
        assert!(right * 6 < inside, "{right} of {inside} pixels disagree with the compositor");
        assert!(wrong_way > right * 3, "and turned the other way it would be clearly wrong: {wrong_way} vs {right}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
