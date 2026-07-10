//! # group_runner — execute a merge group end to end
//!
//! Drives a `MergeGroup` through the full pipeline on a background
//! thread:
//!   1. For each enabled clip: probe → encode (chunks → concat) → 
//!      per-clip MKV. Resolution policy adds a scale filter so all 
//!      clips end up the same size.
//!   2. Stitch the per-clip MKVs into one output with chapters.
//!   3. Optional post-encode rename if the output template used 
//!      {savings} (only knowable once sizes are final).
//!
//! Progress streams through a channel as `GroupEvent`s. The UI polls 
//! and renders; the controller owns the sequencing so the app's 
//! single-job state machine stays untouched.
//!
//! Clips are encoded ONE AT A TIME (not in parallel) because each clip
//! already parallelizes internally across chunks — running several
//! clips at once would oversubscribe the CPU.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crossbeam_channel::{Receiver, Sender};

use crate::encoder::{EncoderSettings, EncoderKind};
use crate::group::{MergeGroup, ResolutionPolicy, TemplateContext, resolve_template,
                   template_uses_savings, DEFAULT_TEMPLATE};
use crate::merge::{self, MergeClip, scale_filter};
use crate::probe::{self, VideoInfo};

/// Progress events emitted while a group runs.
#[derive(Clone, Debug)]
pub enum GroupEvent {
    /// Starting clip `index` of `total` (named).
    ClipStarted { index: usize, total: usize, name: String },
    /// A line of progress for the current clip.
    ClipProgress { index: usize, message: String },
    /// Clip finished encoding; carries the encoded size.
    ClipFinished { index: usize, bytes: u64 },
    /// A clip failed; the group continues with the rest unless fatal.
    ClipFailed { index: usize, error: String },
    /// All clips encoded; stitching now.
    Stitching,
    /// Group complete. Final output path + size + savings vs source.
    Done { output: PathBuf, output_bytes: u64, source_bytes: u64 },
    /// Whole group failed (e.g. stitch failed, nothing to encode).
    Failed(String),
    /// Cancelled by user.
    Cancelled,
}

/// Handle to a running group: the event stream + a cancel flag.
pub struct GroupRun {
    pub events: Receiver<GroupEvent>,
    pub cancel: Arc<AtomicBool>,
}

/// Configuration the runner needs beyond the group itself.
pub struct GroupRunConfig {
    /// Global encoder settings — used for any clip/group that doesn't 
    /// override.
    pub global_settings: EncoderSettings,
    /// Global output template — used when the group's template is empty.
    pub global_template: String,
    /// Chunk length in seconds for the per-clip encodes.
    pub chunk_seconds: f64,
    /// Concurrency (parallel chunks) for each clip's encode.
    pub concurrency: usize,
    /// Working directory root for intermediate per-clip outputs + 
    /// chunk state. A per-run subdir is created under here.
    pub work_root: PathBuf,
}

/// Start running a group on a background thread. Returns immediately 
/// with a handle to poll.
pub fn run_group(group: MergeGroup, config: GroupRunConfig) -> GroupRun {
    let (tx, rx) = crossbeam_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();

    std::thread::Builder::new()
        .name("av1tuner-group".into())
        .spawn(move || {
            run_group_inner(group, config, tx, cancel_for_thread);
        })
        .expect("spawn group thread");

    GroupRun { events: rx, cancel }
}

fn run_group_inner(
    group: MergeGroup,
    config: GroupRunConfig,
    tx: Sender<GroupEvent>,
    cancel: Arc<AtomicBool>,
) {
    // Settings: group override or global.
    let settings = group.settings_override.clone()
        .unwrap_or_else(|| config.global_settings.clone());

    let active: Vec<_> = group.clips.iter().filter(|c| c.enabled).collect();
    if active.is_empty() {
        let _ = tx.send(GroupEvent::Failed("group has no enabled clips".into()));
        return;
    }
    let total = active.len();

    // Per-run working dir.
    let run_dir = config.work_root.join(format!(
        "group_{}_{}",
        sanitize(&group.name),
        core_kit::time::now_secs()
    ));
    if let Err(e) = std::fs::create_dir_all(&run_dir) {
        let _ = tx.send(GroupEvent::Failed(format!("create work dir: {}", e)));
        return;
    }

    // Resolution policy → optional scale filter applied to every clip.
    // Determine the target resolution from probed clip dims. We probe 
    // each clip up front so the policy has the data it needs.
    let mut probed: Vec<(PathBuf, VideoInfo, u64)> = Vec::new();
    for (clip_idx, clip) in active.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            let _ = tx.send(GroupEvent::Cancelled);
            return;
        }
        match probe::probe(&clip.path) {
            Ok(info) => {
                let bytes = std::fs::metadata(&clip.path).map(|m| m.len()).unwrap_or(0);
                probed.push((clip.path.clone(), info, bytes));
            }
            Err(e) => {
                // Probe failure on one clip: report (with the clip's real
                // index in the active list) + skip it.
                let _ = tx.send(GroupEvent::ClipFailed {
                    index: clip_idx,
                    error: format!("probe failed: {}", e),
                });
            }
        }
    }
    if probed.is_empty() {
        let _ = tx.send(GroupEvent::Failed("no clips could be probed".into()));
        return;
    }

    let target_res = compute_target_resolution(&probed, group.resolution_policy);

    // Encode each clip sequentially.
    let mut merge_clips: Vec<MergeClip> = Vec::new();
    let mut source_bytes_total: u64 = 0;
    for (i, (path, info, src_bytes)) in probed.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            let _ = tx.send(GroupEvent::Cancelled);
            return;
        }
        let name = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip")
            .to_string();
        let _ = tx.send(GroupEvent::ClipStarted { index: i, total, name: name.clone() });
        source_bytes_total += src_bytes;

        // Per-clip settings: add the scale filter if the policy forces a
        // target resolution and this clip differs from it.
        let mut clip_settings = settings.clone();
        if let Some((tw, th)) = target_res {
            let needs_scale = info.width != tw || info.height != th;
            if needs_scale {
                clip_settings.video_filter = scale_filter((tw, th));
            }
        }

        let clip_out = run_dir.join(format!("clip_{:04}.mkv", i));
        match encode_one_clip(path, info, &clip_settings, &config, &run_dir, i, &tx, &cancel) {
            Ok(()) => {
                let bytes = std::fs::metadata(&clip_out).map(|m| m.len()).unwrap_or(0);
                let _ = tx.send(GroupEvent::ClipFinished { index: i, bytes });
                // Chapter timing must come from the ENCODED clip's actual 
                // duration, not the source duration. Frame-count rounding, 
                // VFR sources, and fps reinterpretation can make the 
                // encoded duration differ slightly; using source duration 
                // would let chapter marks drift from real stream positions, 
                // and the drift accumulates across clips. Probe the output; 
                // fall back to source duration only if that probe fails.
                let encoded_duration = probe::probe(&clip_out)
                    .map(|enc| enc.duration_s)
                    .unwrap_or(info.duration_s);
                merge_clips.push(MergeClip {
                    encoded_path: clip_out,
                    chapter_name: name,
                    duration_s: encoded_duration,
                });
            }
            Err(EncodeOneError::Cancelled) => {
                let _ = tx.send(GroupEvent::Cancelled);
                return;
            }
            Err(EncodeOneError::Failed(msg)) => {
                let _ = tx.send(GroupEvent::ClipFailed { index: i, error: msg });
                // Continue with remaining clips — partial merge is better
                // than nothing, and the UI shows which clip failed.
            }
        }
    }

    if merge_clips.is_empty() {
        let _ = tx.send(GroupEvent::Failed("all clips failed to encode".into()));
        return;
    }

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(GroupEvent::Cancelled);
        return;
    }

    // Stitch.
    let _ = tx.send(GroupEvent::Stitching);
    let output = resolve_output_path(&group, &settings, &config, &probed);
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match merge::stitch(&merge_clips, &output, &run_dir) {
        Ok(result) => {
            // Post-encode {savings} rename if the template used it.
            let final_output = maybe_rename_for_savings(
                &group, &settings, &config, &probed,
                &output, result.output_bytes, source_bytes_total,
            );
            let _ = tx.send(GroupEvent::Done {
                output: final_output,
                output_bytes: result.output_bytes,
                source_bytes: source_bytes_total,
            });
        }
        Err(e) => {
            let _ = tx.send(GroupEvent::Failed(format!("stitch failed: {}", e)));
        }
    }
}

enum EncodeOneError {
    Cancelled,
    Failed(String),
}

/// Encode a single clip through the chunk pipeline into 
/// `run_dir/clip_{index}.mkv`. Blocking; reports progress via tx.
fn encode_one_clip(
    source: &Path,
    info: &VideoInfo,
    settings: &EncoderSettings,
    config: &GroupRunConfig,
    run_dir: &Path,
    index: usize,
    tx: &Sender<GroupEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), EncodeOneError> {
    use crate::job::JobState;
    use crate::scene::{SceneStrategy, boundaries};
    use crate::scheduler::{Scheduler, SchedulerEvent};
    use crate::chunk::ChunkStatus;
    use crate::audio::AudioSettings;
    use crate::concat;

    let output = run_dir.join(format!("clip_{:04}.mkv", index));
    let state_dir = run_dir.join(format!("clip_{:04}_state", index));

    let mut job = JobState::new(source.to_path_buf(), output.clone(), settings.clone());
    job.info = Some(info.clone());
    job.chunk_boundaries = boundaries(info, &SceneStrategy::FixedSeconds(config.chunk_seconds));
    job.concurrency = config.concurrency as u32;
    // Merge clips keep their own audio (passthrough) so the stitched 
    // file has each clip's sound. The stitch step copies audio through.
    job.audio = AudioSettings::default();
    job.state_dir = state_dir.clone();

    if let Err(e) = job.save() {
        return Err(EncodeOneError::Failed(format!("save job: {}", e)));
    }

    let scheduler = Scheduler::spawn(job.clone());

    // Drain scheduler events until done/error, checking cancel.
    loop {
        if cancel.load(Ordering::SeqCst) {
            scheduler.cancel();
            return Err(EncodeOneError::Cancelled);
        }
        match scheduler.events().recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(SchedulerEvent::ChunkFinished { index: ci, .. }) => {
                let _ = tx.send(GroupEvent::ClipProgress {
                    index,
                    message: format!("chunk {} done", ci),
                });
            }
            Ok(SchedulerEvent::ChunkSkipped { index: ci, error }) => {
                let _ = tx.send(GroupEvent::ClipProgress {
                    index,
                    message: format!("chunk {} skipped: {}", ci, error),
                });
            }
            Ok(SchedulerEvent::AllChunksDone { .. }) => break,
            Ok(SchedulerEvent::Cancelled) => {
                return Err(EncodeOneError::Cancelled);
            }
            Ok(SchedulerEvent::FatalError(msg)) => {
                return Err(EncodeOneError::Failed(msg));
            }
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Concat chunks into the per-clip MKV.
    let chunks = job.load_chunks()
        .map_err(|e| EncodeOneError::Failed(format!("load chunks: {}", e)))?;
    let done = chunks.iter().filter(|c| c.status == ChunkStatus::Done).count();
    if done == 0 {
        return Err(EncodeOneError::Failed("no chunks completed".into()));
    }
    concat::concat(&state_dir, &chunks, source, &output,
                   &AudioSettings::default(), settings.kind)
        .map_err(|e| EncodeOneError::Failed(format!("concat: {}", e)))?;

    Ok(())
}

/// Determine the target resolution implied by the policy over the 
/// probed clips. None = keep native (no scaling).
fn compute_target_resolution(
    probed: &[(PathBuf, VideoInfo, u64)],
    policy: ResolutionPolicy,
) -> Option<(u32, u32)> {
    let mut res: Vec<(u32, u32)> = Vec::new();
    for (_, info, _) in probed {
        let wh = (info.width, info.height);
        if !res.contains(&wh) {
            res.push(wh);
        }
    }
    if res.len() <= 1 {
        return None;  // all same (or one clip) — no scaling needed
    }
    match policy {
        ResolutionPolicy::KeepNative => None,
        ResolutionPolicy::ScaleToSmallest =>
            res.into_iter().min_by_key(|(w, h)| (*w as u64) * (*h as u64)),
        ResolutionPolicy::ScaleToLargest =>
            res.into_iter().max_by_key(|(w, h)| (*w as u64) * (*h as u64)),
    }
}

fn codec_tag(kind: EncoderKind) -> &'static str {
    match kind {
        EncoderKind::SvtAv1 => "av1",
        EncoderKind::Vvenc => "vvc",
    }
}

/// Build the output path from the group/global template.
fn resolve_output_path(
    group: &MergeGroup,
    settings: &EncoderSettings,
    config: &GroupRunConfig,
    probed: &[(PathBuf, VideoInfo, u64)],
) -> PathBuf {
    let template = pick_template(group, config);
    let localdir = probed.first()
        .and_then(|(p, _, _)| p.parent())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let first_name = probed.first()
        .and_then(|(p, _, _)| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ctx = TemplateContext {
        groupname: &group.name,
        sourcename: first_name,
        codec: codec_tag(settings.kind),
        crf: settings.quantizer(),
        localdir: &localdir,
        savings_pct: None,
    };
    PathBuf::from(resolve_template(&template, &ctx))
}

fn pick_template(group: &MergeGroup, config: &GroupRunConfig) -> String {
    if !group.output_template.trim().is_empty() {
        group.output_template.clone()
    } else if !config.global_template.trim().is_empty() {
        config.global_template.clone()
    } else {
        DEFAULT_TEMPLATE.to_string()
    }
}

/// If the template used {savings}, rename the output now that we know 
/// the final size. Returns the (possibly new) path.
fn maybe_rename_for_savings(
    group: &MergeGroup,
    settings: &EncoderSettings,
    config: &GroupRunConfig,
    probed: &[(PathBuf, VideoInfo, u64)],
    current: &Path,
    output_bytes: u64,
    source_bytes: u64,
) -> PathBuf {
    let template = pick_template(group, config);
    if !template_uses_savings(&template) {
        return current.to_path_buf();
    }
    let savings_pct = if source_bytes > 0 {
        (1.0 - (output_bytes as f64 / source_bytes as f64)) * 100.0
    } else { 0.0 };
    let localdir = probed.first()
        .and_then(|(p, _, _)| p.parent())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let first_name = probed.first()
        .and_then(|(p, _, _)| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ctx = TemplateContext {
        groupname: &group.name,
        sourcename: first_name,
        codec: codec_tag(settings.kind),
        crf: settings.quantizer(),
        localdir: &localdir,
        savings_pct: Some(savings_pct),
    };
    let new_path = PathBuf::from(resolve_template(&template, &ctx));
    if new_path != current {
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::rename(current, &new_path).is_ok() {
            return new_path;
        }
    }
    current.to_path_buf()
}

/// Make a string safe as a path component.
fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_wh(w: u32, h: u32) -> VideoInfo {
        VideoInfo {
            width: w, height: h, frame_count: 100, fps: 30.0,
            duration_s: 3.33, pixel_format: "yuv420p".into(),
            codec: "h264".into(), audio_streams: Vec::new(),
        }
    }

    #[test]
    fn target_resolution_policy() {
        let probed = vec![
            (PathBuf::from("a"), info_wh(1920, 1080), 0),
            (PathBuf::from("b"), info_wh(1280, 720), 0),
        ];
        assert_eq!(
            compute_target_resolution(&probed, ResolutionPolicy::ScaleToSmallest),
            Some((1280, 720))
        );
        assert_eq!(
            compute_target_resolution(&probed, ResolutionPolicy::ScaleToLargest),
            Some((1920, 1080))
        );
        assert_eq!(
            compute_target_resolution(&probed, ResolutionPolicy::KeepNative),
            None
        );
    }

    #[test]
    fn no_scale_when_uniform() {
        let probed = vec![
            (PathBuf::from("a"), info_wh(1920, 1080), 0),
            (PathBuf::from("b"), info_wh(1920, 1080), 0),
        ];
        // All same resolution → no target (no scaling needed).
        assert_eq!(
            compute_target_resolution(&probed, ResolutionPolicy::ScaleToSmallest),
            None
        );
    }

    #[test]
    fn sanitize_path_component() {
        assert_eq!(sanitize("My Group/2"), "My_Group_2");
        assert_eq!(sanitize("clean-name_1"), "clean-name_1");
    }
}
