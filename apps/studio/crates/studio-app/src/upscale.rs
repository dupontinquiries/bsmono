//! Upscale a clip through dandere2x — print-side, like autotune.
//!
//! dandere2x is sequential: every output frame depends on the previous
//! *output* (blocks that passed the quality gate are copied from it), so it
//! cannot be a chain effect any more than autotune can; a scrub-able
//! per-frame effect that silently needed the whole clip in order would be a
//! lie about what chains are. The right shape is the one autotune already
//! uses: bounce the selected clip's range through it in the background and
//! land the result as a NEW source and clip beside the original on a video
//! track with room. The original stays on disk, stays undoable, and A/B is
//! a mute toggle away.
//!
//! The job runs on its own thread and reports through the export tracker's
//! `Run`/`Event`, so progress, cancel and "where did it land" are the same
//! window every other render uses. Cancel keeps a playable partial file.

use crate::export::{Event, Run};
use crate::Studio;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

#[derive(Debug, Clone)]
pub struct Settings {
    /// `.safetensors` (Real-ESRGAN via tools/pth2st.py) or waifu2x `.json`.
    /// `None` = the vendored realesr-animevideov3 (fast; anime-trained).
    pub model: Option<PathBuf>,
    pub scale: u32,
    /// JPEG quality the block-reuse gate is measured against.
    pub quality: u8,
    pub block_size: u32,
    pub crf: u32,
    pub preset: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            model: std::env::var_os("D2X_MODEL").map(PathBuf::from),
            scale: 2,
            quality: 97,
            block_size: 30,
            crf: 24,
            preset: 6,
        }
    }
}

/// Start the bounce. `[start_s, end_s)` is the clip's source range.
pub fn start(input: PathBuf, start_s: f64, end_s: f64, out: PathBuf, settings: &Settings) -> Run {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut job = d2x_job::Job::default();
    job.input = input;
    job.output = out.clone();
    job.model = settings.model.clone();
    job.scale = settings.scale;
    job.pipeline.quality = settings.quality;
    job.pipeline.analyze.block_size = settings.block_size;
    job.encode.crf = settings.crf;
    job.encode.preset = settings.preset;
    job.start = start_s;
    job.end = Some(end_s);
    job.cancel = Some(cancel.clone());
    let flag = cancel.clone();
    std::thread::Builder::new().name("studio-upscale".into()).spawn(move || {
        if let Some(dir) = out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut total: Option<u64> = None;
        let r = d2x_job::job::run(&job, |e| match e {
            d2x_job::Event::Stage(m) => {
                let _ = tx.send(Event::Note(format!("upscale: {m}")));
            }
            d2x_job::Event::Pipeline(p) | d2x_job::Event::Segment(_, p) => match p {
                d2x_job::PipelineEvent::Started { info, upscaler, .. } => {
                    total = info.frame_count;
                    let _ = tx.send(Event::Note(format!("upscale: {upscaler}")));
                }
                d2x_job::PipelineEvent::FrameDone { index, .. } => {
                    let frac = total.map_or(0.0, |n| (index + 1) as f32 / n.max(1) as f32).min(0.999);
                    let _ = tx.send(Event::Progress { done: 0, jobs: 1, frac });
                }
                d2x_job::PipelineEvent::Finished { frames, elapsed, upscaled_pixel_fraction } => {
                    let _ = tx.send(Event::Note(format!("upscale: {frames} frames in {:.0}s, {:.0}% of pixels through the network", elapsed.as_secs_f64(), upscaled_pixel_fraction * 100.0)));
                }
            },
        });
        match r {
            Ok(_) => {
                let _ = tx.send(Event::File(out));
                let _ = tx.send(if flag.load(Ordering::Relaxed) { Event::Cancelled } else { Event::Done });
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(format!("{e:#}")));
            }
        }
    })
    .expect("spawn upscale thread");
    Run { rx, cancel, jobs: 1 }
}

impl Studio {
    /// Kick off a bounce of the selected clip. Returns the run and the
    /// clip it belongs to; [`Studio::land_upscaled`] is called with the
    /// file when the run reports it.
    pub fn upscale_selected(&mut self, settings: &Settings) -> Option<(studio_core::ClipId, Run)> {
        let Some(clip_id) = self.selection.first().copied() else {
            self.status = "upscale: select a clip first".into();
            return None;
        };
        let clip = self.session.doc.project.clips.get(&clip_id)?.clone();
        let Some(src) = self.session.doc.project.sources.get(&clip.source) else {
            self.status = "upscale: that clip has no source".into();
            return None;
        };
        if !src.has_video {
            self.status = "upscale: that clip has no picture".into();
            return None;
        }
        let input = PathBuf::from(&src.path);
        let start_s = clip.src_in.to_seconds();
        let end_s = clip.src_in.saturating_add(clip.len).to_seconds();
        let next_id = self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1;
        let out = input
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("upscaled")
            .join(format!("{}-{}-x{}.mkv", input.file_stem().and_then(|s| s.to_str()).unwrap_or("clip"), next_id, settings.scale));
        self.status = format!("upscaling {:.1}s of {} → {}", end_s - start_s, input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), out.display());
        Some((clip_id, start(input, start_s, end_s, out, settings)))
    }

    /// Land a finished bounce: a new source for the file and a new clip at
    /// the original's position on the first unlocked video track with room.
    pub fn land_upscaled(&mut self, clip_id: studio_core::ClipId, path: &std::path::Path) {
        let Some(clip) = self.session.doc.project.clips.get(&clip_id).cloned() else { return };
        let Some(old) = self.session.doc.project.sources.get(&clip.source).cloned() else { return };
        let len = match d2x_job::ffmpeg::probe(path) {
            Ok(p) => p.duration_s.map(studio_core::Ticks::from_seconds).unwrap_or(clip.len),
            Err(_) => clip.len,
        };
        let id = studio_core::SourceId(self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let new_clip = studio_core::ClipId(self.session.doc.project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        self.edit(
            studio_core::Op::AddSource {
                source: studio_core::Source { id, path: path.display().to_string(), len, sample_rate: old.sample_rate, channels: old.channels, has_video: true },
            },
            "upscaled",
        );
        let end = clip.end();
        let project = &self.session.doc.project;
        let track = project
            .tracks
            .iter()
            .filter(|t| t.kind == studio_core::TrackKind::Video && !t.locked)
            .find(|t| !project.clips.values().any(|c| c.track == t.id && c.pos < end && c.end() > clip.pos))
            .map(|t| t.id);
        let Some(track) = track else {
            self.status = format!("upscaled to {} — no video track has room beside the clip; import it by hand", path.display());
            return;
        };
        self.edit(
            studio_core::Op::AddClip {
                clip: studio_core::Clip { id: new_clip, source: id, track, pos: clip.pos, src_in: studio_core::Ticks(0), len: clip.len.min(len), gain: clip.gain, effects: clip.effects.clone() },
            },
            "upscaled",
        );
        self.selection = vec![new_clip];
        self.status = format!("upscaled clip landed on '{}'", self.session.doc.project.tracks.iter().find(|t| t.id == track).map(|t| t.name.clone()).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn studio_with_video_clip() -> (Studio, ClipId) {
        let mut p = Project { name: "vid".into(), ..Default::default() };
        let mk = |id: u64, kind: TrackKind, name: &str| Track { id: TrackId(id), kind, name: name.into(), muted: false, locked: false, gain: 1.0, route: None };
        p.tracks.push(mk(1, TrackKind::Video, "V1"));
        p.tracks.push(mk(2, TrackKind::Video, "V2"));
        p.sources.insert(SourceId(1), Source { id: SourceId(1), path: "/nowhere/take.mkv".into(), len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: true });
        p.clips.insert(ClipId(1), Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: Ticks::from_seconds(1.0), src_in: Ticks::from_seconds(2.0), len: Ticks::from_seconds(3.0), gain: 1.0, effects: Vec::new() });
        let mut s = Studio::new(p, crate::SiteId(1), crate::Team::from_contacts("solo", &["sc-local".into()]));
        s.selection = vec![ClipId(1)];
        (s, ClipId(1))
    }

    /// Landing a finished bounce is two undoable ops: a new video source and
    /// a new clip at the original's position on the first video track with
    /// room (V2 here — V1 is occupied by the original), with the original
    /// untouched. The file need not exist for the model side to be right.
    #[test]
    fn a_landed_upscale_is_a_new_source_and_clip_beside_the_original() {
        let (mut s, clip) = studio_with_video_clip();
        let before = s.session.doc.project.clips.len();
        s.land_upscaled(clip, std::path::Path::new("/nowhere/upscaled/take-2-x2.mkv"));
        let p = &s.session.doc.project;
        assert_eq!(p.clips.len(), before + 1);
        assert!(p.clips.contains_key(&clip), "original clip stays");
        let new = p.clips.values().find(|c| c.id != clip).expect("new clip");
        assert_eq!(new.track, TrackId(2), "lands on the free video track");
        assert_eq!(new.pos, Ticks::from_seconds(1.0));
        assert_eq!(new.src_in, Ticks(0), "the rendered file starts at the clip's in-point");
        assert_eq!(new.len, Ticks::from_seconds(3.0));
        let src = p.sources.get(&new.source).expect("new source");
        assert!(src.has_video);
        assert!(src.path.ends_with("take-2-x2.mkv"));
        assert_eq!(s.selection, vec![new.id]);
    }

    #[test]
    fn upscaling_needs_a_video_clip() {
        let (mut s, _) = studio_with_video_clip();
        s.selection.clear();
        assert!(s.upscale_selected(&Settings::default()).is_none());
        assert!(s.status.contains("select a clip"), "{}", s.status);
        // An audio-only source refuses out loud too.
        let (mut s, _) = studio_with_video_clip();
        s.session.doc.project.sources.get_mut(&SourceId(1)).unwrap().has_video = false;
        assert!(s.upscale_selected(&Settings::default()).is_none());
        assert!(s.status.contains("no picture"), "{}", s.status);
    }
}
