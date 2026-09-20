//! touchup — turning a note about a shot into a fixed version of it.
//!
//! The workflow the film's guidance describes (`docs/film/TOUCHUP_VFX.md`):
//! a scene's cut stops changing, and the shots in it that need work get
//! fixed one at a time. What is fixed is never a new CLIP — the timeline
//! clip keeps its id, position and trim, and points at a new VERSION of the
//! same picture. That is what keeps a touch-up pass from re-opening the edit.
//!
//! A [`Ticket`] says which clip, which fixes, and how many frames of HANDLE
//! to render beyond the clip's trim. Handles are the whole reason this is
//! not just "render the visible range": an editor who later extends the clip
//! by ten frames must not hit unfixed picture. [`Version::covers`] answers
//! whether a trim still fits inside what was rendered, so a ticket re-opens
//! itself when the edit moves past its handles instead of silently going
//! stale.
//!
//! [`render`] decodes once for measurement, then once more through
//! [`crate::framepipe`] applying the fixes in order, and returns a [`Report`]
//! of what each one actually did — the zoom a stabilizer needed, the
//! corrections it could not afford, the pixels a clean plate refused to
//! fill. Nothing here decides anything is "good enough": the report is for a
//! person to read.

use std::path::{Path, PathBuf};

use studio_core::{Clip, ClipId, Op, Source, SourceId, Ticks};

use crate::cleanplate::{self, Settings};
use crate::framepipe::{Frame, Pipe, Size};
use crate::stabilize::{self, Budget, Flavor};

/// One fix to apply to a shot. Order matters and is the ticket's order: a
/// clean plate on a stabilized picture is not the same as the reverse
/// (stabilizing after a fill moves the filled pixels around, which is
/// usually what you want — fill first, then stabilize both together).
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    Stabilize { flavor: Flavor, budget: Budget },
    /// Remove something crossing the frame, using the frames around each one.
    CleanPlate { settings: Settings, radius: usize, align: bool },
}

/// What to fix, on which range of which source.
#[derive(Debug, Clone, PartialEq)]
pub struct Ticket {
    pub clip: ClipId,
    pub source: SourceId,
    pub path: PathBuf,
    /// The clip's trim at the time the ticket was raised.
    pub src_in: Ticks,
    pub len: Ticks,
    /// Extra seconds rendered each side. Half a second (12 frames at 24) is
    /// the usual minimum; a second is safer.
    pub handle: f64,
    pub fixes: Vec<Fix>,
    pub note: String,
}

impl Ticket {
    /// The range actually rendered: the trim plus handles, clamped at zero.
    pub fn range(&self) -> (f64, f64) {
        let start = (self.src_in.to_seconds() - self.handle).max(0.0);
        let end = self.src_in.to_seconds() + self.len.to_seconds() + self.handle;
        (start, end - start)
    }
}

/// What one fix did, in the operator's terms.
#[derive(Debug, Clone, PartialEq)]
pub enum Did {
    Stabilized { zoom: f32, clamped_px: f32, frames: usize },
    Cleaned { frames_touched: usize, pixels_filled: u64, pixels_unresolved: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub did: Vec<Did>,
    pub frames: usize,
    /// Anything the operator must decide about. Empty means the fixes did
    /// what they were asked; non-empty is not failure, it is a question.
    pub warnings: Vec<String>,
}

/// A rendered fixed version of a shot.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub path: PathBuf,
    pub clip: ClipId,
    /// Where this file's frame 0 sits in the ORIGINAL source's time, so a
    /// clip pointing at it can keep its trim: `src_in - covers_from`.
    pub covers_from: f64,
    pub covers_len: f64,
    pub size: Size,
    pub fps: f64,
    pub report: Report,
}

impl Version {
    /// Whether a (possibly re-trimmed) clip range still lies inside what was
    /// rendered. `false` means the ticket must be re-rendered — the editor
    /// pulled the clip past its handles.
    pub fn covers(&self, src_in: Ticks, len: Ticks) -> bool {
        let (a, b) = (src_in.to_seconds(), src_in.to_seconds() + len.to_seconds());
        // a frame of tolerance: the rendered file's length is rounded to whole frames
        let slack = 1.0 / self.fps;
        a >= self.covers_from - slack && b <= self.covers_from + self.covers_len + slack
    }

    /// The clip's `src_in` translated into this version's own time.
    pub fn src_in_for(&self, src_in: Ticks) -> Ticks {
        Ticks::from_seconds((src_in.to_seconds() - self.covers_from).max(0.0))
    }

    /// Point `clip` at this version, as one undoable step: register the new
    /// source, then replace the clip with an identical one reading from it.
    /// The clip keeps its id, track, position, length, gain and effects —
    /// only what it reads changes.
    ///
    /// There is no `SetClipSource` op; remove-then-add inside `Op::Group` is
    /// the existing way to change something a clip's identity is built on,
    /// and it undoes as a single step.
    pub fn relink(&self, clip: &Clip, new_source: SourceId, source_len: Ticks) -> Op {
        let mut fixed = clip.clone();
        fixed.source = new_source;
        fixed.src_in = self.src_in_for(clip.src_in);
        Op::Group { ops: vec![
            Op::AddSource { source: Source {
                id: new_source,
                path: self.path.to_string_lossy().into_owned(),
                len: source_len,
                sample_rate: 48_000,
                channels: 2,
                has_video: true,
                kind: studio_core::SourceKind::File,
            } },
            Op::RemoveClip { id: clip.id },
            Op::AddClip { clip: fixed },
        ] }
    }
}

/// Render the ticket's fixes into `out`, at `size` and `fps`.
///
/// Measurement decodes grey frames once; the fixes then run in one pass over
/// the picture. `fps` should be the source's own rate — a fixed version at a
/// different rate would no longer line up with the clip's trim.
pub fn render(t: &Ticket, out: &Path, size: Size, fps: f64) -> Result<Version, String> {
    if t.fixes.is_empty() {
        return Err("ticket has no fixes".into());
    }
    let (from, len) = t.range();
    let pipe = Pipe { input: t.path.clone(), range: Some((from, len)), size, fps };
    let mut warnings = Vec::new();
    let mut did = Vec::new();

    // Measurement pass: one grey decode shared by every fix that needs motion.
    let needs_motion = t.fixes.iter().any(|f| matches!(f,
        Fix::Stabilize { .. } | Fix::CleanPlate { align: true, .. }));
    let (path, steps) = if needs_motion {
        let grey = pipe.gray()?;
        if grey.len() < 2 { return Err(format!("only {} frame(s) decoded", grey.len())); }
        let steps = stabilize::steps(&grey, (size.w / 20).max(8), 1);
        (stabilize::measure(&grey, (size.w / 20).max(8)), steps)
    } else {
        (Vec::new(), Vec::new())
    };
    if let Some(cut) = steps.iter().position(|m| m.residual > 40.0) {
        warnings.push(format!(
            "frame {cut}: the picture changes completely here (a cut, a flash, or something \
             filling frame) — the measured camera path past it is not meaningful"));
    }

    // Resolve each fix into a per-frame operation before touching pixels.
    let mut solution = None;
    let mut plate = None;
    for f in &t.fixes {
        match f {
            Fix::Stabilize { flavor, budget } => {
                let s = stabilize::solve(&path, flavor, *budget, size.w, size.h);
                if s.clamped_px > 0.5 {
                    warnings.push(format!(
                        "stabilizer: {:.0} px of correction did not fit in the {:.0}% crop budget — \
                         the shot is steadier, not locked", s.clamped_px, (budget.max_zoom - 1.0) * 100.0));
                }
                did.push(Did::Stabilized { zoom: s.zoom, clamped_px: s.clamped_px, frames: path.len() });
                solution = Some(s);
            }
            Fix::CleanPlate { settings, radius, align } => {
                plate = Some((settings.clone(), *radius, *align));
            }
        }
    }

    let radius = plate.as_ref().map(|p| p.1).unwrap_or(0);
    let (mut filled, mut unresolved, mut touched) = (0u64, 0u64, 0usize);
    let frames = pipe.map_window(out, 18, radius, |i, window| {
        // The centre of the window is the frame being written, except at the
        // ends where the window is clipped: framepipe hands back at most
        // radius frames each side, so the current frame is the one at
        // min(i, radius) from the start.
        let here = i.min(radius).min(window.len() - 1);
        let mut frame: Frame = window[here].clone();
        if let Some((settings, _, align)) = &plate {
            let offsets: Vec<(f32, f32)> = if *align && !path.is_empty() {
                (0..window.len()).map(|k| {
                    let src = i + k - here;
                    let (a, b) = (path.get(src).copied().unwrap_or((0.0, 0.0)),
                                  path.get(i).copied().unwrap_or((0.0, 0.0)));
                    (b.0 - a.0, b.1 - a.1)
                }).collect()
            } else {
                Vec::new()
            };
            let c = cleanplate::clean(window, here, &offsets, settings);
            filled += c.filled as u64;
            unresolved += c.unresolved as u64;
            if c.filled > 0 { touched += 1; }
            frame = c.frame;
        }
        if let Some(s) = &solution {
            let c = s.corrections[i.min(s.corrections.len() - 1)];
            frame = frame.warp(c.dx, c.dy, s.zoom);
        }
        frame
    })?;

    if plate.is_some() {
        did.push(Did::Cleaned { frames_touched: touched, pixels_filled: filled, pixels_unresolved: unresolved });
        if unresolved > 0 {
            warnings.push(format!(
                "clean plate: {unresolved} pixel(s) were covered for too much of the window to fill — \
                 widen the window, or this shot needs paint or an inpainting pass"));
        }
    }
    Ok(Version {
        path: out.to_path_buf(),
        clip: t.clip,
        covers_from: from,
        covers_len: frames as f64 / fps,
        size, fps,
        report: Report { did, frames, warnings },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ffmpeg_or_skip, shoot};
    use studio_core::{Project, Track, TrackId, TrackKind};

    fn ticket(path: &Path, fixes: Vec<Fix>) -> Ticket {
        Ticket {
            clip: ClipId(1), source: SourceId(1), path: path.into(),
            src_in: Ticks::from_seconds(1.0), len: Ticks::from_seconds(2.0),
            handle: 0.5, fixes, note: "car crosses behind Max".into(),
        }
    }

    #[test]
    fn a_ticket_renders_its_handles_and_knows_when_a_retrim_outgrows_them() {
        let t = ticket(Path::new("x.mkv"), vec![]);
        assert_eq!(t.range(), (0.5, 3.0), "2 s clip + half a second each side");
        let v = Version {
            path: "v2.mp4".into(), clip: ClipId(1), covers_from: 0.5, covers_len: 3.0,
            size: Size { w: 64, h: 36 }, fps: 12.0,
            report: Report { did: vec![], frames: 36, warnings: vec![] },
        };
        assert!(v.covers(Ticks::from_seconds(1.0), Ticks::from_seconds(2.0)), "the original trim");
        assert!(v.covers(Ticks::from_seconds(0.6), Ticks::from_seconds(2.8)), "extended into the handles");
        assert!(!v.covers(Ticks::from_seconds(0.2), Ticks::from_seconds(2.0)), "past the head handle");
        assert!(!v.covers(Ticks::from_seconds(1.0), Ticks::from_seconds(3.0)), "past the tail handle");
        // and the trim translates into the version's own time
        assert_eq!(v.src_in_for(Ticks::from_seconds(1.0)), Ticks::from_seconds(0.5));
    }

    #[test]
    fn relinking_keeps_the_clip_and_only_changes_what_it_reads() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "V1".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        let src = |id: u64, path: &str| Op::AddSource { source: Source { id: SourceId(id), path: path.into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true,
            kind: studio_core::SourceKind::File } };
        p.apply(&src(1, "plate.mkv")).unwrap();
        let clip = Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(10.0), src_in: Ticks::from_seconds(1.0), len: Ticks::from_seconds(2.0),
            gain: 1.0, effects: vec![] };
        p.apply(&Op::AddClip { clip: clip.clone() }).unwrap();

        let v = Version { path: "v2.mp4".into(), clip: ClipId(1), covers_from: 0.5, covers_len: 3.0,
            size: Size { w: 64, h: 36 }, fps: 12.0,
            report: Report { did: vec![], frames: 36, warnings: vec![] } };
        let op = v.relink(&clip, SourceId(2), Ticks::from_seconds(3.0));
        let inv = p.apply(&op).expect("relinks");
        let now = &p.clips[&ClipId(1)];
        assert_eq!(now.source, SourceId(2));
        assert_eq!(now.src_in, Ticks::from_seconds(0.5), "the trim moved into the version's time");
        assert_eq!((now.pos, now.len, now.track), (clip.pos, clip.len, clip.track), "the edit did not move");
        p.apply(&inv).expect("undoes");
        assert_eq!(p.clips[&ClipId(1)], clip, "one undo puts the original clip back");
    }

    #[test]
    fn a_stabilize_ticket_renders_a_steadier_version_of_the_handled_range() {
        if !ffmpeg_or_skip("a_stabilize_ticket_renders_a_steadier_version_of_the_handled_range") { return; }
        let s = shoot(&[1.0], 6.0);
        let size = Size { w: 128, h: 72 };
        // a shaky plate with trackable texture
        let shaky = s.dir.join(format!("tu-shaky-{}.mp4", std::process::id()));
        Pipe { input: s.takes[0].video.clone(), range: None, size, fps: 12.0 }
            .map(&shaky, 12, |i, mut f| {
                for y in 0..f.h { for x in 0..f.w {
                    let (fx, fy) = (x as f32, y as f32);
                    let a = 128.0 + 90.0 * (fx / 9.0).sin() * (fy / 7.0).cos();
                    let b = 128.0 + 90.0 * (fx / 13.0 + fy / 11.0).sin();
                    f.set(x, y, [a as u8, b as u8, ((a + b) / 2.0) as u8]);
                }}
                let t = i as f32;
                f.warp((t * 0.9).sin() * 5.0, (t * 1.3).cos() * 4.0, 1.0)
            }).expect("shaky plate");

        let t = ticket(&shaky, vec![Fix::Stabilize {
            flavor: Flavor::Smooth { sigma: 8.0 }, budget: Budget { max_zoom: 1.3 } }]);
        let out = s.dir.join(format!("tu-v2-{}.mp4", std::process::id()));
        let v = render(&t, &out, size, 12.0).expect("renders");

        assert!(out.exists());
        assert_eq!(v.covers_from, 0.5);
        assert!((v.covers_len - 3.0).abs() < 0.2, "the handled range: {}", v.covers_len);
        assert!(v.covers(t.src_in, t.len), "the clip's own trim is inside it");
        match v.report.did[0] {
            Did::Stabilized { zoom, frames, .. } => {
                assert!(zoom > 1.0 && zoom <= 1.3, "zoom {zoom}");
                assert!(frames >= 34, "measured {frames} frames of a 3 s range at 12 fps");
            }
            ref other => panic!("{other:?}"),
        }
        let jitter = |p: &Pipe| -> f32 {
            let path = stabilize::measure(&p.gray().unwrap(), 12);
            path.windows(3).map(|w| (w[2].0 - 2.0 * w[1].0 + w[0].0).abs()
                + (w[2].1 - 2.0 * w[1].1 + w[0].1).abs()).sum::<f32>() / (path.len() - 2) as f32
        };
        let before = jitter(&Pipe { input: shaky.clone(), range: Some(t.range()), size, fps: 12.0 });
        let after = jitter(&Pipe { input: out.clone(), range: None, size, fps: 12.0 });
        assert!(after < before * 0.5, "{after} vs {before}");
        for f in [&shaky, &out] { let _ = std::fs::remove_file(f); }
    }

    #[test]
    fn a_clean_plate_ticket_removes_the_thing_that_crosses_and_reports_what_it_could_not() {
        if !ffmpeg_or_skip("a_clean_plate_ticket_removes_the_thing_that_crosses_and_reports_what_it_could_not") { return; }
        let s = shoot(&[1.0], 6.0);
        let size = Size { w: 96, h: 54 };
        let plate = s.dir.join(format!("tu-plate-{}.mp4", std::process::id()));
        // static background, one fast block crossing, and one parked block
        // that never moves — the second cannot be recovered and must be
        // reported rather than smeared
        Pipe { input: s.takes[0].video.clone(), range: None, size, fps: 12.0 }
            .map(&plate, 10, |i, mut f| {
                for y in 0..f.h { for x in 0..f.w {
                    let (fx, fy) = (x as f32, y as f32);
                    let v = 128.0 + 90.0 * (fx / 9.0).sin() * (fy / 7.0).cos();
                    f.set(x, y, [v as u8, (255.0 - v) as u8, v as u8]);
                }}
                let x0 = (i as i32 * 9) % 120 - 12;
                for y in 10..44 { for x in x0.max(0)..(x0 + 10).min(f.w as i32) { f.set(x as u32, y, [255, 0, 0]); } }
                f
            }).expect("plate");

        let t = ticket(&plate, vec![Fix::CleanPlate {
            settings: Settings { threshold: 25.0, ..Default::default() }, radius: 3, align: false }]);
        let out = s.dir.join(format!("tu-clean-{}.mp4", std::process::id()));
        let v = render(&t, &out, size, 12.0).expect("renders");
        match v.report.did[0] {
            Did::Cleaned { frames_touched, pixels_filled, .. } => {
                assert!(frames_touched > 20, "the block crosses most frames: {frames_touched}");
                assert!(pixels_filled > 5_000, "filled {pixels_filled}");
            }
            ref other => panic!("{other:?}"),
        }
        // the red block is gone from the rendered version
        let red = |p: &Path| -> usize {
            let px = crate::frame_rgb(p, Ticks::from_seconds(1.5), size.w, size.h).unwrap();
            px.chunks(3).filter(|p| p[0] > 170 && p[1] < 90 && p[2] < 90).count()
        };
        assert!(red(&out) * 4 < red(&plate).max(1), "{} red pixels left of {}", red(&out), red(&plate));
        for f in [&plate, &out] { let _ = std::fs::remove_file(f); }
    }
}
