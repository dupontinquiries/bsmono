//! touchup_job — running a shot's fix in the background, and landing it.
//!
//! [`studio_video::touchup::render`] takes seconds to minutes (it decodes,
//! measures and re-encodes a whole shot), so it cannot run on the UI thread.
//! This is the same shape the exporter uses: a worker, a channel of
//! [`Event`]s, and the shell polling them while it repaints.
//!
//! Landing is deliberately NOT the same as the upscaler's. An upscale adds a
//! new clip beside the old one, because it is a different rendition the
//! editor chooses between. A touch-up REPLACES what the clip reads — same
//! id, same position, same trim — because the edit was already approved and
//! a second clip on another track would silently double the shot.

use std::path::PathBuf;
use std::sync::mpsc;

use studio_video::framepipe::Size;
use studio_video::touchup::{self, Ticket, Version};

use crate::Studio;

#[derive(Debug)]
pub enum Event {
    /// A stage, for the status line.
    Note(String),
    Done(Box<Version>),
    Failed(String),
}

/// A fix in flight. Dropping it does not stop the worker — the file it is
/// writing is still worth having, the same stance the exporter takes.
pub struct Job {
    pub rx: mpsc::Receiver<Event>,
    pub clip: studio_core::ClipId,
}

/// Render `ticket` into `out` on a worker thread.
pub fn start(ticket: Ticket, out: PathBuf, size: Size, fps: f64, wake: crate::wake::Wake) -> Job {
    // wakes the window on every event: a touch-up that had landed still read
    // "measuring" four minutes later until the mouse moved, without it
    let (tx, rx) = crate::wake::channel(wake);
    let clip = ticket.clip;
    std::thread::spawn(move || {
        let _ = tx.send(Event::Note(format!("touch-up: measuring {}",
            ticket.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
        let _ = match touchup::render(&ticket, &out, size, fps) {
            Ok(v) => tx.send(Event::Done(Box::new(v))),
            Err(e) => tx.send(Event::Failed(e)),
        };
    });
    Job { rx, clip }
}

/// The timeline stretch a touch-up render covers, for lining a drive signal
/// up with it: the clip plus `handle` seconds each side, where the head handle
/// is cut short if the clip starts less than `handle` into its source (the
/// render cannot read before the source's start). Returns (from, to) in
/// timeline seconds, and how many frames of silence to pad at the front when
/// `from` falls before timeline zero. Frame i of the render then reads the
/// drive at frame i -- a drive computed for the clip alone runs half a second
/// early against the handled render.
pub fn drive_span(clip: &studio_core::Clip, handle: f64, fps: f64) -> (f64, f64, usize) {
    let head = (clip.src_in.to_seconds() - handle).max(0.0);
    let lead = clip.src_in.to_seconds() - head;
    let from = clip.pos.to_seconds() - lead;
    let to = clip.end().to_seconds() + handle;
    let pad = if from < 0.0 { (-from * fps).round() as usize } else { 0 };
    (from.max(0.0), to, pad)
}

impl Studio {
    /// Point the clip at its fixed version, as one undoable step, and report
    /// what the fix could not do.
    pub fn land_touchup(&mut self, v: &Version) {
        let Some(clip) = self.session.doc.project.clips.get(&v.clip).cloned() else {
            self.status = "the clip this fix belongs to is gone".into();
            return;
        };
        if !v.covers(clip.src_in, clip.len) {
            // The editor re-trimmed past the handles while it rendered.
            // Landing it anyway would show unfixed picture at one end.
            self.status = format!(
                "the clip was re-trimmed past the handles this fix rendered — raise it again \
                 (rendered {:.2}s from {:.2}s)", v.covers_len, v.covers_from);
            return;
        }
        let id = studio_core::SourceId(
            self.session.doc.project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
        let op = v.relink(&clip, id, studio_core::Ticks::from_seconds(v.covers_len));
        self.edit(op, "touch-up landed");
        let mut said = format!("touch-up landed on '{}'", v.path.file_name()
            .map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        for w in &v.report.warnings {
            said.push_str(" — ");
            said.push_str(w);
        }
        self.status = said;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;
    use studio_video::touchup::Report;

    fn studio_with_clip() -> (Studio, ClipId) {
        let mut p = Project { name: "film".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "V1".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "/card/C0031.mkv".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true,
            kind: SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(5.0), src_in: Ticks::from_seconds(2.0),
            len: Ticks::from_seconds(3.0), gain: 0.8, effects: vec![] } }).unwrap();
        let mut s = Studio::new(p, crate::SiteId(1), crate::Team::from_contacts("solo", &["sc-local".into()]));
        s.selection = vec![ClipId(1)];
        (s, ClipId(1))
    }

    fn version(covers_from: f64, covers_len: f64, warnings: Vec<String>) -> Version {
        Version {
            path: "/renders/C0031-v002.mp4".into(), clip: ClipId(1), covers_from, covers_len,
            size: Size { w: 1920, h: 1080 }, fps: 24.0,
            report: Report { did: vec![], frames: (covers_len * 24.0) as usize, warnings },
        }
    }

    #[test]
    fn a_drive_lines_up_with_the_handled_render() {
        let clip = |pos: f64, src_in: f64| Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(pos), src_in: Ticks::from_seconds(src_in), len: Ticks::from_seconds(4.0),
            gain: 1.0, effects: vec![] };
        // a full half-second of handle both sides
        assert_eq!(drive_span(&clip(10.0, 3.0), 0.5, 24.0), (9.5, 14.5, 0));
        // the clip starts 0.2 s into its source: only 0.2 s of head handle exists
        let (from, _, pad) = drive_span(&clip(10.0, 0.2), 0.5, 24.0);
        assert!((from - 9.8).abs() < 1e-9 && pad == 0, "{from}");
        // the handled render starts before timeline zero: pad the front with silence
        let (from, _, pad) = drive_span(&clip(0.2, 3.0), 0.5, 24.0);
        assert_eq!((from, pad), (0.0, 7), "0.3 s before zero is 7 frames at 24 fps");
        // and it agrees with the ticket's own range, which is what the render reads
        let c = clip(10.0, 3.0);
        let t = studio_video::touchup::Ticket { clip: c.id, source: c.source, path: "x".into(), src_in: c.src_in, len: c.len,
            handle: 0.5, fixes: vec![], note: String::new() };
        let (ts, tl) = t.range();
        let (from, to, _) = drive_span(&c, 0.5, 24.0);
        assert!(((to - from) - tl).abs() < 1e-9 && (ts - 2.5).abs() < 1e-9, "same span as the render: {from}..{to} vs {ts}+{tl}");
    }

    #[test]
    fn landing_replaces_what_the_clip_reads_and_nothing_else() {
        let (mut s, id) = studio_with_clip();
        let before = s.session.doc.project.clips[&id].clone();
        s.land_touchup(&version(1.5, 4.0, vec![]));
        let after = &s.session.doc.project.clips[&id];
        assert_ne!(after.source, before.source, "it reads the fixed file now");
        assert_eq!(after.src_in, Ticks::from_seconds(0.5), "the trim moved into the version's time");
        assert_eq!((after.pos, after.len, after.track, after.gain),
                   (before.pos, before.len, before.track, before.gain), "the edit did not move");
        assert_eq!(s.session.doc.project.clips.len(), 1, "no second clip: a touch-up is not an upscale");
        s.key(studio_egui::keys::Key::c('u'));
        assert_eq!(s.session.doc.project.clips[&id], before, "one undo puts the plate back");
    }

    #[test]
    fn a_retrim_past_the_handles_refuses_to_land_and_says_why() {
        let (mut s, id) = studio_with_clip();
        // rendered 1.5..5.5; then the editor pulled the head back to 0.5
        s.edit(Op::TrimClip { id, src_in: Ticks::from_seconds(0.5), len: Ticks::from_seconds(3.0) }, "retrim");
        let before = s.session.doc.project.clips[&id].clone();
        s.land_touchup(&version(1.5, 4.0, vec![]));
        assert_eq!(s.session.doc.project.clips[&id], before, "nothing was landed");
        assert!(s.status.contains("re-trimmed"), "{}", s.status);
    }

    #[test]
    fn warnings_from_the_fix_reach_the_status_line() {
        let (mut s, _) = studio_with_clip();
        s.land_touchup(&version(1.5, 4.0, vec!["stabilizer: 30 px did not fit the crop budget".into()]));
        assert!(s.status.contains("crop budget"), "{}", s.status);
    }

    #[test]
    fn a_missing_clip_is_said_out_loud_rather_than_ignored() {
        let (mut s, id) = studio_with_clip();
        s.edit(Op::RemoveClip { id }, "removed");
        s.land_touchup(&version(1.5, 4.0, vec![]));
        assert!(s.status.contains("gone"), "{}", s.status);
    }
}
