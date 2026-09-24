//! export — the dialog's decision layer, and the run that does the work.
//!
//! The one-click exporter (`Studio::export_arrangement`) stays what it is: a
//! synchronous whole-arrangement render to `./exports`, no questions asked.
//! This module is the flow around the questions — stems or one file, the
//! whole arrangement or the selection or the rack loop, which rate, which
//! bitrate — split the way everything here is split:
//!
//! * [`Plan`] + [`plan_jobs`]/[`plan_rack_jobs`] are PURE: a plan and a
//!   project in, a list of named render jobs out, refusals as `Err` strings
//!   the dialog shows instead of closing. Every decision a UI test cannot
//!   reach lives here, headless.
//! * [`start_arrangement`]/[`start_rack`] run the jobs on a worker thread —
//!   the TCP carrier's shape: a thread that owns the work, an mpsc facing
//!   the shell, drained once per frame — reporting [`Event`]s chunk by
//!   chunk so a long render is a moving bar, not a frozen window, and a
//!   cancel flag checked between chunks so "stop" means soon, not at the
//!   end. A cancelled job writes NO file; files from jobs already finished
//!   stay, and the `File` events already said which those are.
//! * [`pick_save`]/[`pick_folder`] are the native picker behind the
//!   `picker` cargo feature (rfd, XDG portal on Linux) — the `midi`
//!   feature's precedent: a build without it still compiles, and a runtime
//!   without a portal service degrades to the default `./exports` path with
//!   the status line saying so, never a silent no-op.
//!
//! Rendering reuses `mixdown_with_cache` with one cache per job: contiguous
//! chunked calls match a one-shot offline render sample for sample — the
//! property the streaming renderer already pinned — so a tracked export is
//! bit-identical to the untracked one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use studio_audio::SourceAudio;
use studio_core::{ClipId, Project, Ticks, TrackId, TrackKind};

pub use crate::ExportFormat;

/// What kind of span the export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Tick zero through the last clip's end.
    Full,
    /// The hull of the selected clips — first selected start to last
    /// selected end. Refused out loud when nothing is selected.
    Selection,
    /// One pass of the current rack pattern, rendered by the same
    /// `render_pattern` the audition loop plays.
    RackLoop,
    /// One file per song (group), named after it, spanning its clips'
    /// hull, in TIMELINE order — the one ordering the model actually
    /// expresses. This is C12-14e's answer, decided: **the album is an
    /// organizational container; songs export independently.** A
    /// concatenated single-file album would have to invent an ordering
    /// and a gaplessness the model never stated, and distribution wants
    /// per-track files anyway; a "whole timeline as one file" already
    /// exists as [`Scope::Full`], untouched.
    EachSong,
}

/// Everything the dialog decides, as a value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub format: ExportFormat,
    pub scope: Scope,
    /// One file per stem instead of one mixdown. For the arrangement a stem
    /// is a track (soloed through its own busses); for the rack loop it is
    /// a channel.
    pub stems: bool,
    /// WAV render rate. Opus pins 48k — the codec's contract, enforced in
    /// the run rather than trusted to the dialog.
    pub rate: u32,
    pub layout: studio_audio::format::ChannelFormat,
    /// Opus bitrate in bits/sec.
    pub opus_bitrate: u32,
}

impl Default for Plan {
    fn default() -> Plan {
        // Opus when the build has it; WAV otherwise (never a default that fails).
        Plan { format: if crate::OPUS_BUILT { ExportFormat::Opus } else { ExportFormat::Wav },
               scope: Scope::Full, stems: false,
               rate: 48_000, layout: studio_audio::format::ChannelFormat::Stereo,
               opus_bitrate: 128_000 }
    }
}

/// One arrangement render: a name (empty for the single mixdown), a span,
/// and the solo set that isolates the stem.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub name: String,
    pub from: Ticks,
    pub to: Ticks,
    pub solo: Vec<TrackId>,
}

/// The arrangement jobs a plan means. Pure — every refusal is a string the
/// dialog can show without closing.
pub fn plan_jobs(plan: &Plan, project: &Project, selection: &[ClipId])
                 -> Result<Vec<Job>, String> {
    let (from, to) = match plan.scope {
        Scope::Full => {
            let end = studio_audio::audible_end(project); // tails ring past the last clip
            if end <= Ticks::ZERO { return Err("the arrangement is empty".into()); }
            (Ticks::ZERO, end)
        }
        Scope::Selection => {
            let clips: Vec<_> = selection.iter()
                .filter_map(|id| project.clips.get(id)).collect();
            if clips.is_empty() { return Err("nothing is selected".into()); }
            let from = clips.iter().map(|c| c.pos).min().unwrap();
            let to = clips.iter().map(|c| c.end()).max().unwrap();
            (from, to)
        }
        Scope::RackLoop => return Err("rack jobs are planned by plan_rack_jobs".into()),
        Scope::EachSong => {
            // A song's export is its SPAN of the arrangement — exactly what
            // playback of that span plays, ungrouped material included. The
            // principle cuts both ways: two songs sharing a time range
            // would bleed into each other's files, so that is refused out
            // loud instead of shipped as a surprise.
            if plan.stems {
                return Err("stems per song multiply fast — pick the song with \
                            'switch song', select its clips, and export selection \
                            stems instead".into());
            }
            let mut spans: Vec<(String, Ticks, Ticks)> = Vec::new();
            for (gid, g) in &project.groups {
                let clips: Vec<_> = g.clips.iter()
                    .filter_map(|c| project.clips.get(c)).collect();
                if clips.is_empty() { continue; } // every member deleted
                let from = clips.iter().map(|c| c.pos).min().unwrap();
                let to = clips.iter().map(|c| c.end()).max().unwrap();
                // Name collisions (two songs both called "interlude")
                // disambiguate by id rather than overwriting a file.
                let name = if project.groups.values()
                    .filter(|o| o.name == g.name).count() > 1 {
                    format!("{} ({})", g.name, gid.0)
                } else { g.name.clone() };
                spans.push((name, from, to));
            }
            if spans.is_empty() {
                return Err("no songs — group clips into songs first ('switch song' \
                            lists them)".into());
            }
            spans.sort_by_key(|(_, from, _)| *from);
            for w in spans.windows(2) {
                if w[1].1 < w[0].2 {
                    return Err(format!(
                        "'{}' and '{}' overlap on the timeline — their exports \
                         would bleed into each other. Separate them in time, or \
                         export each via selection.", w[0].0, w[1].0));
                }
            }
            return Ok(spans.into_iter()
                .map(|(name, from, to)| Job { name, from, to, solo: Vec::new() })
                .collect());
        }
    };
    if !plan.stems {
        return Ok(vec![Job { name: String::new(), from, to, solo: Vec::new() }]);
    }
    // A stem per audio track that actually carries material in the span —
    // a silent stem is a file nobody asked for. For a selection export the
    // material that counts is the SELECTED clips: soloing a track whose
    // only clip in range wasn't selected would smuggle it into its stem.
    let counts = |t: TrackId| -> bool {
        project.clips.values().any(|c| {
            c.track == t && c.pos < to && c.end() > from
                && (plan.scope != Scope::Selection || selection.contains(&c.id))
        })
    };
    let jobs: Vec<Job> = project.tracks.iter()
        .filter(|t| t.kind == TrackKind::Audio && counts(t.id))
        .map(|t| Job { name: t.name.clone(), from, to, solo: vec![t.id] })
        .collect();
    if jobs.is_empty() {
        return Err("no audio track carries material in that span".into());
    }
    Ok(jobs)
}

/// The rack-loop jobs a plan means: one render of the current pattern, or a
/// stem per unmuted channel (rendered with every other channel muted — the
/// mute flag `render_pattern` already honours).
pub fn plan_rack_jobs(stems: bool, rack: &studio_live::Rack, pattern: usize)
                      -> Result<Vec<(String, studio_live::Rack)>, String> {
    if rack.patterns.get(pattern).is_none() {
        return Err("no pattern to render".into());
    }
    if !stems {
        return Ok(vec![(String::new(), rack.clone())]);
    }
    let live: Vec<usize> = rack.channels.iter().enumerate()
        .filter(|(_, c)| !c.muted).map(|(i, _)| i).collect();
    if live.is_empty() { return Err("every channel is muted".into()); }
    Ok(live.into_iter().map(|keep| {
        let mut r = rack.clone();
        for (i, ch) in r.channels.iter_mut().enumerate() {
            if i != keep { ch.muted = true; }
        }
        (rack.channels[keep].name.clone(), r)
    }).collect())
}

/// Where the deliverables land: an exact file (single) or a folder (stems),
/// which is also which native dialog the shell opens.
#[derive(Debug, Clone, PartialEq)]
pub enum Dest {
    File(PathBuf),
    Folder(PathBuf),
}

/// What the worker tells the shell, drained once per frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `done` whole jobs finished; `frac` is progress within the current one.
    Progress { done: usize, jobs: usize, frac: f32 },
    /// A deliverable landed on disk.
    File(PathBuf),
    /// A line for the status bar (what stage a long job is in).
    Note(String),
    Done,
    Cancelled,
    Failed(String),
}

/// A run in flight. Dropping it does NOT cancel — an export someone started
/// should finish even if the tracker window is closed; the cancel button
/// sets the flag on purpose.
pub struct Run {
    pub rx: mpsc::Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
    pub jobs: usize,
}

/// Whether a native picker is compiled in — how the shell tells "the user
/// cancelled the dialog" (do nothing) from "there is no dialog to open"
/// (fall back to `./exports` and say so).
#[cfg(feature = "picker")]
pub const PICKER_AVAILABLE: bool = true;
#[cfg(not(feature = "picker"))]
pub const PICKER_AVAILABLE: bool = false;

pub fn sanitize(s: &str) -> String {
    let out: String = s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    if out.is_empty() { "untitled".into() } else { out }
}

/// A path that does not exist yet — the exporter's overwrite guard,
/// extracted so stems get it too.
fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 1u32;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    path
}

/// Interleaved stereo → the deliverable on disk.
fn write_deliverable(plan: &Plan, stereo: &[f32], rate: u32, seed: u64, path: &Path)
                     -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
    }
    match plan.format {
        ExportFormat::Wav => {
            let samples = studio_audio::format::expand_from_stereo(stereo, plan.layout, rate);
            studio_audio::export::export_wav_dithered(
                &samples, plan.layout.channels() as u16, rate, seed, path)
                .map_err(|e| format!("wav write: {e}"))
        }
        ExportFormat::Opus => {
            let mut planes = vec![Vec::with_capacity(stereo.len() / 2); 2];
            for fr in stereo.chunks(2) {
                planes[0].push(fr[0]);
                planes[1].push(*fr.get(1).unwrap_or(&fr[0]));
            }
            let audio = SourceAudio { sample_rate: rate, channels: planes };
            let bytes = studio_media::encode_opus(&audio, plan.opus_bitrate as i32)
                .map_err(|e| e.to_string())?;
            std::fs::write(path, bytes).map_err(|e| format!("write {path:?}: {e}"))
        }
        ExportFormat::Mp3 | ExportFormat::M4a => {
            let wav = path.with_extension("tmp.wav");
            studio_audio::export::export_wav_dithered(stereo, 2, rate, seed, &wav)
                .map_err(|e| format!("wav write: {e}"))?;
            let r = studio_video::transcode_audio(&wav, path, plan.format.ffmpeg_codec().expect("lossy"),
                                                  plan.opus_bitrate.max(96_000))
                .map_err(|e| format!("{}: {e} — ffmpeg with that encoder is needed", plan.format.ext()));
            let _ = std::fs::remove_file(&wav);
            r
        }
    }
}

fn job_path(dest: &Dest, plan: &Plan, project_name: &str, job_name: &str) -> PathBuf {
    let ext = plan.format.ext();
    match dest {
        Dest::File(p) => p.clone(),
        Dest::Folder(dir) => {
            let base = if job_name.is_empty() { sanitize(project_name) }
                       else { format!("{}-{}", sanitize(project_name), sanitize(job_name)) };
            unique_path(dir, &base, ext)
        }
    }
}

/// Render the plan's arrangement jobs on a worker thread. Everything the
/// worker needs is CLONED IN — the shell keeps editing while the export
/// renders the project as it stood when the button was pressed, which is
/// the only export anyone means.
pub fn start_arrangement(
    plan: Plan,
    jobs: Vec<Job>,
    dest: Dest,
    project: Project,
    audio: std::collections::BTreeMap<studio_core::SourceId, SourceAudio>,
    wake: crate::wake::Wake,
) -> Run {
    // Opus's contract, enforced where it cannot be forgotten; the phone's
    // formats are stereo too.
    let mut plan = plan;
    if plan.format == ExportFormat::Opus { plan.rate = 48_000; }
    if plan.format.stereo_only() { plan.layout = studio_audio::format::ChannelFormat::Stereo; }
    let (tx, rx) = crate::wake::channel(wake);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let n_jobs = jobs.len();
    std::thread::spawn(move || {
        let rate = plan.rate;
        let chunk = Ticks::from_seconds(1.0);
        for (ji, job) in jobs.iter().enumerate() {
            // One cache per job: contiguous chunks match the one-shot
            // render exactly; a NEW job is a seek, and a fresh cache is
            // precisely the reset a seek means.
            let mut cache = studio_audio::EffectCache::new();
            let total = (job.to.0 - job.from.0).max(1);
            let mut stereo: Vec<f32> = Vec::new();
            let mut at = job.from;
            while at < job.to {
                if flag.load(Ordering::Relaxed) {
                    let _ = tx.send(Event::Cancelled);
                    return;
                }
                let to = Ticks((at.0 + chunk.0).min(job.to.0));
                stereo.extend(studio_audio::mixdown_with_cache(
                    &project, &audio, &std::collections::BTreeMap::new(),
                    at, to, rate, &job.solo, &[], &mut cache));
                at = to;
                let _ = tx.send(Event::Progress {
                    done: ji, jobs: n_jobs,
                    frac: ((at.0 - job.from.0) as f64 / total as f64) as f32,
                });
            }
            let path = job_path(&dest, &plan, &project.name, &job.name);
            match write_deliverable(&plan, &stereo, rate, path_seed(&path), &path) {
                Ok(()) => { let _ = tx.send(Event::File(path)); }
                Err(e) => { let _ = tx.send(Event::Failed(e)); return; }
            }
        }
        let _ = tx.send(Event::Done);
    });
    Run { rx, cancel, jobs: n_jobs }
}

/// The music video: every video clip on the timeline cut into one mp4
/// (top video track on top, lower tracks composited through blends,
/// opacity and reframes as the viewport shows them, black where none) over the arrangement's
/// mixdown. Video sources whose files are not on disk are gaps; a
/// timeline with no video at all is refused, since black over audio is
/// not a video anyone asked for. Synchronous — the shell runs it on a
/// worker. Returns the file written.
pub fn render_music_video(
    project: &Project,
    audio: &std::collections::BTreeMap<studio_core::SourceId, SourceAudio>,
    out: &Path,
    rate: u32,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<PathBuf, String> {
    let paths: std::collections::BTreeMap<studio_core::SourceId, PathBuf> = project.sources.values()
        .filter(|s| s.has_video)
        .map(|s| (s.id, PathBuf::from(&s.path)))
        .filter(|(_, p)| p.is_file())
        .collect();
    let end = studio_audio::audible_end(project); // tails ring past the last clip
    let list = studio_video::edit::edit_list(project, &paths, Some(end));
    if !list.iter().any(studio_video::edit::Segment::has_picture) {
        return Err(if paths.is_empty() {
            "no video on the timeline (or its files are not where the project says)".into()
        } else { "no video clip on a video track".into() });
    }
    // The mixdown under it, as a WAV beside the output; removed after.
    let stereo = studio_audio::mixdown(project, audio, &std::collections::BTreeMap::new(),
                                       Ticks::ZERO, end, rate, &[], &[]);
    let wav = out.with_extension("mix.wav");
    let bytes = studio_audio::export::export_wav_dithered_bytes(&stereo, 2, rate, path_seed(out)).map_err(|e| e.to_string())?;
    std::fs::write(&wav, bytes).map_err(|e| format!("{}: {e}", wav.display()))?;
    let result = studio_video::edit::render(&list, Some(&wav), out, width, height, fps)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&wav);
    result.map(|()| out.to_path_buf())
}

/// What the video export will not reproduce from the viewport (rotation,
/// retiming — see `studio_video::edit::Picture`), one line per
/// affected cut. The export still runs; these go to the tracker as notes so
/// a file that differs from the preview says so instead of surprising.
pub fn video_warnings(project: &Project) -> Vec<String> {
    let paths: std::collections::BTreeMap<studio_core::SourceId, PathBuf> = project.sources.values()
        .filter(|s| s.has_video)
        .map(|s| (s.id, PathBuf::from(&s.path)))
        .collect();
    studio_video::edit::unsupported(&studio_video::edit::edit_list(project, &paths, None))
}

/// [`render_music_video`] on a worker, reported through the same `Run`
/// the audio exports use so the tracker window shows it. Cancel is not
/// honoured mid-render: ffmpeg is one child process and this crate does
/// not reach into it; the flag is checked only before it starts.
pub fn start_video(
    project: Project,
    audio: std::collections::BTreeMap<studio_core::SourceId, SourceAudio>,
    out: PathBuf,
    rate: u32,
    width: u32,
    height: u32,
    fps: u32,
    wake: crate::wake::Wake,
) -> Run {
    let (tx, rx) = crate::wake::channel(wake);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        if flag.load(Ordering::Relaxed) { let _ = tx.send(Event::Cancelled); return; }
        let _ = tx.send(Event::Progress { done: 0, jobs: 1, frac: 0.0 });
        let _ = tx.send(Event::Note("rendering the video through ffmpeg — this takes a while".into()));
        for w in video_warnings(&project) {
            let _ = tx.send(Event::Note(format!("differs from the preview: {w}")));
        }
        match render_music_video(&project, &audio, &out, rate, width, height, fps) {
            Ok(p) => { let _ = tx.send(Event::File(p)); let _ = tx.send(Event::Done); }
            Err(e) => { let _ = tx.send(Event::Failed(e)); }
        }
    });
    Run { rx, cancel, jobs: 1 }
}

/// Render the rack-loop jobs — one `render_pattern` pass each, duplicated
/// to both channels (the loop is mono by construction, and a stereo file
/// is what every player expects).
pub fn start_rack(
    plan: Plan,
    jobs: Vec<(String, studio_live::Rack)>,
    dest: Dest,
    pattern: usize,
    audio: std::collections::BTreeMap<studio_core::SourceId, SourceAudio>,
    project_name: String,
    wake: crate::wake::Wake,
) -> Run {
    let mut plan = plan;
    if plan.format == ExportFormat::Opus {
        plan.rate = 48_000;
        plan.layout = studio_audio::format::ChannelFormat::Stereo;
    }
    let (tx, rx) = crate::wake::channel(wake);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let n_jobs = jobs.len();
    std::thread::spawn(move || {
        for (ji, (name, rack)) in jobs.iter().enumerate() {
            if flag.load(Ordering::Relaxed) {
                let _ = tx.send(Event::Cancelled);
                return;
            }
            let Some(pat) = rack.patterns.get(pattern) else {
                let _ = tx.send(Event::Failed("the pattern vanished mid-export".into()));
                return;
            };
            let mut synth = studio_live::BuiltinSynth { sources: &audio };
            let mono = studio_live::render_pattern(
                pat, rack, &mut synth, &rack.tempo(), plan.rate, 0);
            let mut stereo = Vec::with_capacity(mono.len() * 2);
            for s in &mono { stereo.push(*s); stereo.push(*s); }
            let path = job_path(&dest, &plan, &project_name, name);
            match write_deliverable(&plan, &stereo, plan.rate, path_seed(&path), &path) {
                Ok(()) => { let _ = tx.send(Event::File(path)); }
                Err(e) => { let _ = tx.send(Event::Failed(e)); return; }
            }
            let _ = tx.send(Event::Progress { done: ji + 1, jobs: n_jobs, frac: 0.0 });
        }
        let _ = tx.send(Event::Done);
    });
    Run { rx, cancel, jobs: n_jobs }
}

/// The dither seed: derived from the path so re-exporting the same name is
/// still deterministic per deliverable, the property the one-click
/// exporter's timestamp seed already had by accident.
fn path_seed(p: &Path) -> u64 {
    p.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64,
        |h, b| (h ^ b as u64).wrapping_mul(0x1000_0000_01b3))
}

// ── the native picker, behind its feature ───────────────────────────────

/// The native save dialog, or `None` when the feature is off or the
/// desktop has no portal to ask — the caller falls back to `./exports`
/// and says so, never silently.
#[cfg(feature = "picker")]
pub fn pick_save(suggested: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export to…")
        .set_file_name(suggested)
        .save_file()
}
#[cfg(not(feature = "picker"))]
pub fn pick_save(_suggested: &str) -> Option<PathBuf> { None }

/// The native open-file dialog (a word list, say), same fallback contract.
#[cfg(feature = "picker")]
pub fn pick_file() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Choose a file…").pick_file()
}
#[cfg(not(feature = "picker"))]
pub fn pick_file() -> Option<PathBuf> { None }

/// The native folder dialog for stems, same fallback contract.
#[cfg(feature = "picker")]
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Export stems into…").pick_folder()
}
#[cfg(not(feature = "picker"))]
pub fn pick_folder() -> Option<PathBuf> { None }

#[cfg(test)]
mod video_tests {
    use super::*;
    use studio_core::{Clip, Op, Source, SourceId, Track};

    fn has_x264() -> bool {
        std::process::Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(" libx264 "))
            .unwrap_or(false)
    }

    #[test]
    fn the_default_export_is_one_this_build_can_encode() {
        assert!(Plan::default().format.available(), "{:?}", Plan::default().format);
    }

    #[test]
    fn the_music_video_is_the_timelines_cuts_over_the_mixdown_and_a_missing_file_is_a_gap() {
        let tmp = std::env::temp_dir().join(format!("studio-mv-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "cam".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        // A video source whose file is NOT there: the render must refuse,
        // saying so, rather than shell out to nothing.
        let ghost = tmp.join("ghost.mp4");
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: ghost.to_string_lossy().into_owned(),
            len: Ticks::from_seconds(1.0), sample_rate: 48_000, channels: 1, has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: Ticks::ZERO,
            src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        let audio = std::collections::BTreeMap::new();
        let out = tmp.join("mv.mp4");
        let err = render_music_video(&p, &audio, &out, 48_000, 64, 48, 10).unwrap_err();
        assert!(err.contains("not where the project says"), "{err}");
        if !has_x264() { return; }
        // Now a real fixture behind the same source.
        let ok = std::process::Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", "color=c=green:s=64x48:r=10:d=1",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-an", ghost.to_str().unwrap()]).output().unwrap();
        assert!(ok.status.success());
        let got = render_music_video(&p, &audio, &out, 48_000, 64, 48, 10).unwrap();
        assert_eq!(got, out);
        assert!(out.is_file());
        assert!(!out.with_extension("mix.wav").exists(), "the mixdown WAV is cleaned up");
        let probe = studio_video::probe(&out).unwrap();
        assert!((probe.duration.to_seconds() - 1.0).abs() < 0.15, "{:?}", probe.duration);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, Op, Source, SourceId, Track};

    fn project() -> Project {
        let mut p = Project { name: "cut".into(), ..Default::default() };
        for (i, (kind, nm)) in [(TrackKind::Video, "V1"), (TrackKind::Audio, "A1"),
                                 (TrackKind::Audio, "A2"), (TrackKind::Audio, "empty")]
            .iter().enumerate()
        {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: *kind,
                name: (*nm).into(), muted: false, locked: false, gain: 1.0, route: None },
                at: i }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1,
            has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        for (id, track, at) in [(1u64, 2u64, 0.0), (2, 3, 1.0), (3, 2, 3.0)] {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(1),
                track: TrackId(track), pos: Ticks::from_seconds(at),
                src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0),
                gain: 1.0, effects: vec![] } }).unwrap();
        }
        p
    }


    /// C12-14e, decided and pinned: the album is a container, songs
    /// export independently, in timeline order, named after themselves.
    #[test]
    fn each_song_is_one_job_in_timeline_order() {
        let mut p = project();
        // Two songs, deliberately DEFINED out of timeline order: "closer"
        // (clip 3, at 3s) gets the lower GroupId, "opener" (clips 1+2,
        // 0..2s) the higher. Timeline order must win over id order.
        p.apply(&Op::SetGroup { id: studio_core::GroupId(1), name: "closer".into(),
            clips: vec![ClipId(3)] }).unwrap();
        p.apply(&Op::SetGroup { id: studio_core::GroupId(2), name: "opener".into(),
            clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        let plan = Plan { scope: Scope::EachSong, ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[]).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "opener");
        assert_eq!((jobs[0].from, jobs[0].to),
                   (Ticks::ZERO, Ticks::from_seconds(2.0)),
                   "the hull of clips 1 and 2");
        assert_eq!(jobs[1].name, "closer");
        assert_eq!((jobs[1].from, jobs[1].to),
                   (Ticks::from_seconds(3.0), Ticks::from_seconds(4.0)));
        assert!(jobs.iter().all(|j| j.solo.is_empty()),
                "a song is a mixdown of its span, not a stem");
    }

    /// The refusals, each a sentence the dialog shows instead of closing:
    /// overlap would bleed, stems would multiply, and no songs means the
    /// scope has nothing to mean.
    #[test]
    fn each_song_refuses_overlap_stems_and_emptiness() {
        let mut p = project();
        // No groups at all: nothing to export per-song.
        let plan = Plan { scope: Scope::EachSong, ..Plan::default() };
        assert!(plan_jobs(&plan, &p, &[]).unwrap_err().contains("no songs"));
        // A group whose every clip was deleted is skipped, and if it was
        // the ONLY group that is still "no songs", not a zero-length job.
        p.apply(&Op::SetGroup { id: studio_core::GroupId(1), name: "ghost".into(),
            clips: vec![ClipId(99)] }).unwrap();
        assert!(plan_jobs(&plan, &p, &[]).unwrap_err().contains("no songs"));
        // Overlapping songs: clip 1 (0..1s) and clip 2 (1..2s) vs a song
        // holding clip 3 (3..4s) is fine — but a song holding clips 2 and 3
        // (1..4s) against one holding clip 1 and clip 3... build a real
        // overlap: song A = clips 1+3 (0..4s hull), song B = clip 2 (1..2s).
        p.apply(&Op::SetGroup { id: studio_core::GroupId(1), name: "a".into(),
            clips: vec![ClipId(1), ClipId(3)] }).unwrap();
        p.apply(&Op::SetGroup { id: studio_core::GroupId(2), name: "b".into(),
            clips: vec![ClipId(2)] }).unwrap();
        let err = plan_jobs(&plan, &p, &[]).unwrap_err();
        assert!(err.contains("overlap"), "got: {err}");
        // Stems per song: refused with the workaround named.
        let stems = Plan { scope: Scope::EachSong, stems: true, ..Plan::default() };
        assert!(plan_jobs(&stems, &p, &[]).unwrap_err().contains("switch song"));
    }

    /// Two songs with one name must not overwrite one file — the id
    /// disambiguates, and only when needed.
    #[test]
    fn duplicate_song_names_disambiguate_by_id() {
        let mut p = project();
        p.apply(&Op::SetGroup { id: studio_core::GroupId(1), name: "interlude".into(),
            clips: vec![ClipId(1)] }).unwrap();
        p.apply(&Op::SetGroup { id: studio_core::GroupId(2), name: "interlude".into(),
            clips: vec![ClipId(3)] }).unwrap();
        let plan = Plan { scope: Scope::EachSong, ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[]).unwrap();
        assert_eq!(jobs[0].name, "interlude (1)");
        assert_eq!(jobs[1].name, "interlude (2)");
    }

    #[test]
    fn a_full_single_export_is_one_job_spanning_the_arrangement() {
        let p = project();
        let jobs = plan_jobs(&Plan::default(), &p, &[]).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].from, Ticks::ZERO);
        assert_eq!(jobs[0].to, p.duration());
        assert!(jobs[0].solo.is_empty(), "a mixdown solos nothing");
    }

    #[test]
    fn stems_are_one_per_audio_track_that_carries_material() {
        // The video track is not audio; the "empty" track has no clips —
        // neither earns a file. A silent stem is a file nobody asked for.
        let p = project();
        let plan = Plan { stems: true, ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[]).unwrap();
        assert_eq!(jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(),
                   vec!["A1", "A2"]);
        assert_eq!(jobs[0].solo, vec![TrackId(2)], "each stem solos its own track");
    }

    #[test]
    fn a_selection_export_spans_the_hull_and_refuses_when_empty() {
        let p = project();
        let plan = Plan { scope: Scope::Selection, ..Plan::default() };
        assert!(plan_jobs(&plan, &p, &[]).is_err(), "no selection, said out loud");
        // Clips 1 (0..1s) and 3 (3..4s): the hull is 0..4s.
        let jobs = plan_jobs(&plan, &p, &[ClipId(1), ClipId(3)]).unwrap();
        assert_eq!(jobs[0].from, Ticks::ZERO);
        assert_eq!(jobs[0].to, Ticks::from_seconds(4.0));
    }

    #[test]
    fn selection_stems_cover_only_tracks_owning_selected_clips() {
        // Clip 2 lives on A2 inside the hull but is NOT selected: soloing
        // A2 would smuggle it into a stem, so A2 earns no job.
        let p = project();
        let plan = Plan { scope: Scope::Selection, stems: true, ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[ClipId(1), ClipId(3)]).unwrap();
        assert_eq!(jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), vec!["A1"]);
    }

    #[test]
    fn rack_stems_are_one_per_unmuted_channel_with_the_rest_muted() {
        let mut rack = studio_live::Rack::trap_kit();
        rack.channels[1].muted = true;
        let jobs = plan_rack_jobs(true, &rack, 0).unwrap();
        assert_eq!(jobs.len(), rack.channels.len() - 1, "the muted channel earns no stem");
        for (name, r) in &jobs {
            let live: Vec<_> = r.channels.iter().enumerate()
                .filter(|(_, c)| !c.muted).collect();
            assert_eq!(live.len(), 1, "exactly one channel sounds in '{name}'");
            assert_eq!(&live[0].1.name, name);
        }
        let single = plan_rack_jobs(false, &rack, 0).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].1, rack, "the single render is the rack as it stands");
        assert!(plan_rack_jobs(true, &rack, 99).is_err(), "a missing pattern refuses");
    }

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-export-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn drain(run: &Run) -> Vec<Event> {
        let mut evs = Vec::new();
        while let Ok(e) = run.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            let last = matches!(e, Event::Done | Event::Cancelled | Event::Failed(_));
            evs.push(e);
            if last { break; }
        }
        evs
    }

    #[test]
    fn a_tracked_wav_export_lands_the_file_and_reports_monotonic_progress() {
        let p = project();
        let mut audio = std::collections::BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000,
            channels: vec![vec![0.25; 48_000 * 10]] });
        let plan = Plan { format: ExportFormat::Wav, ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[]).unwrap();
        let dir = tempdir("wav");
        let run = start_arrangement(plan, jobs, Dest::File(dir.join("mix.wav")), p, audio, crate::wake::none());
        let evs = drain(&run);
        assert!(matches!(evs.last(), Some(Event::Done)), "{evs:?}");
        let mut last = -1.0f32;
        for e in &evs {
            if let Event::Progress { frac, .. } = e {
                assert!(*frac >= last, "progress went backwards: {evs:?}");
                last = *frac;
            }
        }
        assert!((last - 1.0).abs() < 1e-6, "the bar reaches the end");
        let f = dir.join("mix.wav");
        assert!(evs.contains(&Event::File(f.clone())));
        assert!(std::fs::metadata(&f).map(|m| m.len() > 44).unwrap_or(false),
                "a real WAV landed");
    }

    #[cfg(feature = "audio")] // plans an Opus export explicitly
    #[test]
    fn stems_land_one_named_file_each_and_opus_survives_its_own_decoder() {
        let p = project();
        let mut audio = std::collections::BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000,
            channels: vec![vec![0.25; 48_000 * 10]] });
        let plan = Plan { format: ExportFormat::Opus, stems: true, rate: 44_100,
                          ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[]).unwrap();
        let dir = tempdir("stems");
        let run = start_arrangement(plan, jobs, Dest::Folder(dir.clone()), p, audio, crate::wake::none());
        let evs = drain(&run);
        assert!(matches!(evs.last(), Some(Event::Done)), "{evs:?}");
        let files: Vec<_> = evs.iter().filter_map(|e| match e {
            Event::File(p) => Some(p.clone()), _ => None }).collect();
        assert_eq!(files.len(), 2, "one file per stem: {files:?}");
        assert!(files[0].file_name().unwrap().to_string_lossy().contains("A1"));
        // The deliverable is a real Ogg Opus: this crate's own decoder —
        // the roundtrip yardstick the encoder tests already use — reads it
        // back at the codec's contractual 48k despite the plan asking 44.1.
        let bytes = std::fs::read(&files[0]).unwrap();
        let back = studio_media::decode_bytes(bytes, Some("opus")).expect("decodes");
        assert_eq!(back.sample_rate, 48_000);
        // The ARRANGEMENT is four seconds (three one-second clips, the last
        // at 3s) — this assertion's first draft measured against the ten-
        // second SOURCE and failed, which is exactly the confusion the
        // span-vs-source distinction exists to prevent.
        let secs = back.frames() as f64 / 48_000.0;
        assert!((secs - 4.0).abs() < 0.1, "the stem is the span, not the source: {secs}s");
    }

    #[test]
    fn cancel_stops_the_run_and_writes_no_file() {
        let p = project();
        let mut audio = std::collections::BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000,
            channels: vec![vec![0.25; 48_000 * 10]] });
        let plan = Plan { format: ExportFormat::Wav, ..Plan::default() };
        let jobs = plan_jobs(&plan, &p, &[]).unwrap();
        let dir = tempdir("cancel");
        let dest = dir.join("mix.wav");
        let run = start_arrangement(plan, jobs, Dest::File(dest.clone()), p, audio, crate::wake::none());
        // Cancel before the worker gets anywhere: the flag is checked
        // before every chunk, so the first check already sees it.
        run.cancel.store(true, Ordering::Relaxed);
        let evs = drain(&run);
        assert!(evs.contains(&Event::Cancelled), "{evs:?}");
        assert!(!evs.iter().any(|e| matches!(e, Event::File(_))));
        assert!(!dest.exists(), "a cancelled job writes nothing");
    }

    #[cfg(feature = "audio")] // plans an Opus export explicitly
    #[test]
    fn a_rack_loop_export_is_audible_and_stereo() {
        let rack = studio_live::Rack::trap_kit();
        let jobs = plan_rack_jobs(false, &rack, 0).unwrap();
        let dir = tempdir("rack");
        let plan = Plan { format: ExportFormat::Opus, scope: Scope::RackLoop,
                          ..Plan::default() };
        let run = start_rack(plan, jobs, Dest::File(dir.join("loop.opus")), 0,
                             Default::default(), "cut".into(), crate::wake::none());
        let evs = drain(&run);
        assert!(matches!(evs.last(), Some(Event::Done)), "{evs:?}");
        let bytes = std::fs::read(dir.join("loop.opus")).unwrap();
        let back = studio_media::decode_bytes(bytes, Some("opus")).expect("decodes");
        assert_eq!(back.channels.len(), 2);
        assert!(back.channels[0].iter().any(|s| s.abs() > 0.01), "the kit is in the file");
    }
}

/// Run ffmpeg for a test, panicking with its error text on failure.
#[cfg(test)]
pub(crate) fn tests_support_run(args: &[&str]) {
    let o = std::process::Command::new("ffmpeg").args(args).output().expect("ffmpeg runs");
    assert!(o.status.success(), "ffmpeg {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
}
