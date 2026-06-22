//! # App — egui frontend for M1
//!
//! Single video flow:
//! 1. Pick input file (native file picker via rfd, or paste path)
//! 2. Output path auto-derives from input (override with picker)
//! 3. Adjust settings (CRF, preset, pixel format, chunk size)
//! 4. Click "Preflight" — runs validation, shows checks
//! 5. Click "Start" — kicks off probe → plan → encode
//! 6. Progress + ETA shown live; chunks visualized as a colored strip
//! 7. On completion, the summary panel shows size reduction + total time
//!
//! Resume: when input + output already have an `.av1tuner` dir alongside
//! the output, app loads that state on click. Completed chunks are 
//! reused; only pending ones encode.

use std::path::PathBuf;
use eframe::egui::{self, Color32, ProgressBar, RichText};
use log::info;
use crossbeam_channel::bounded;

use av1tuner_core::{
    audio::{AudioSettings, AudioMode, OpusSettings, OpusApplication},
    chunk::{ChunkState, ChunkStatus},
    concat,
    config::UserConfig,
    encoder::{EncoderKind, EncoderSettings, VvencPreset},
    eta::{EtaTracker, format_duration, from_chunks},
    job::{derive_state_dir, JobState, JobStatus, JobProgress},
    preflight::{self, PreflightReport, CheckStatus},
    probe,
    scene::{boundaries, SceneStrategy},
    scheduler::{Scheduler, SchedulerEvent},
};

pub struct App {
    input_path: String,
    output_path: String,
    settings: EncoderSettings,
    audio: AudioSettings,
    chunk_seconds: f64,
    /// Override for parallel-worker count. 0 = "use default (cpu_count/2)".
    concurrency: u32,
    /// Suggested default for the concurrency slider, derived from CPU count.
    default_concurrency: u32,
    
    preflight: Option<PreflightReport>,
    job: Option<JobState>,
    chunks: Vec<ChunkState>,
    scheduler: Option<Scheduler>,
    log_lines: Vec<String>,
    
    /// Rolling ETA tracker — fed by ChunkFinished events.
    eta: EtaTracker,
    /// When the current encode started (wall clock). None when no job running.
    job_started: Option<std::time::Instant>,
    
    /// Source video size (cached after probe + file stat).
    source_bytes: Option<u64>,
    
    /// Set when concat completes.
    final_output: Option<PathBuf>,
    final_output_bytes: Option<u64>,
    /// Wall-clock encode duration in seconds (set on completion).
    final_duration_s: Option<u64>,
    
    /// Whether we detected a resumable job and showed the resume offer.
    resume_offered: bool,
    
    /// Tracks the last-saved settings so we only write config when 
    /// they actually change. Initialized at startup from loaded config.
    last_saved_config: UserConfig,
    /// Auto-remove intermediates after success (from config; tidy mode).
    auto_cleanup: bool,
    /// Use the encode cache to skip unchanged clips (from config).
    use_cache: bool,
    /// Signature thoroughness for the cache (from config).
    cache_mode: av1tuner_core::SignatureMode,
    /// Debounce: write config to disk at most every 1s even if values 
    /// keep changing. Prevents disk thrashing as user types in paths.
    last_save_at: std::time::Instant,
    
    /// Set when a probe is running in the background. Polled each 
    /// frame; when the result arrives, downstream logic runs (start_job 
    /// or just storing for later compare).
    pending_probe: Option<PendingProbe>,
    
    /// True if the running probe should kick off encoding when done. 
    /// False if it's a standalone probe-only run (e.g. before compare).
    probe_will_start: bool,
    
    /// Cached probe info from the most recent probe. Cleared when 
    /// input_path or output_path changes. Used by compare-codecs.
    cached_info: Option<probe::VideoInfo>,
    
    /// Sample-comparison run in background. Polled like pending_probe.
    pending_compare: Option<PendingCompare>,
    
    /// Last completed comparison result, shown in the UI.
    compare_result: Option<av1tuner_core::ComparisonResult>,
    
    /// Cancel flag for the running comparison.
    compare_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    
    /// Sample length in seconds for the compare button. UI-tunable.
    compare_sample_seconds: f64,
    
    /// Pending VVenC install, if one's running. Holds the event 
    /// receiver from vvenc_install::install_async + the cancel handle.
    pending_install: Option<PendingInstall>,
    
    /// Most recent install status — drives the UI text below the 
    /// Install button.
    install_status: InstallStatus,
    
    /// Pending prediction-analysis run (Phase 0 measurement).
    pending_prediction: Option<PendingPrediction>,
    
    /// Cancel flag for the running prediction analysis.
    prediction_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    
    /// Last completed prediction analysis stats.
    prediction_result: Option<av1tuner_core::PredictionStats>,
    
    /// Sample length in seconds for prediction analysis (separate from 
    /// compare-codecs sample length — prediction needs longer to get 
    /// stable I/P frame statistics).
    prediction_sample_seconds: f64,
    
    // --- Groups (merge tabs) ---
    /// Which top-level mode the UI is in.
    ui_mode: UiMode,
    /// The merge groups (tabs). Each is one merged output.
    groups: Vec<av1tuner_core::MergeGroup>,
    /// Index of the currently-selected tab.
    active_group: usize,
    /// Global output-name template used by groups that don't override.
    global_template: String,
    /// A running group, if any.
    pending_group: Option<PendingGroup>,
    /// Last group run's final status line for display.
    group_status: String,
}

#[derive(Clone, Copy, PartialEq)]
enum UiMode {
    SingleFile,
    Groups,
}

struct PendingGroup {
    run: av1tuner_core::GroupRun,
    started_at: std::time::Instant,
    /// Which group index is running (so we can show its name).
    group_index: usize,
    /// Progress: (current clip index, total clips).
    progress: (usize, usize),
    /// Latest progress message.
    last_message: String,
}

struct PendingPrediction {
    rx: crossbeam_channel::Receiver<
        Result<av1tuner_core::PredictionStats, av1tuner_core::PredictionError>
    >,
    started_at: std::time::Instant,
}

struct PendingInstall {
    rx: crossbeam_channel::Receiver<av1tuner_core::vvenc_install::InstallEvent>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    started_at: std::time::Instant,
    /// Current step label for the progress banner.
    current_step: String,
}

#[derive(Clone, Debug, PartialEq)]
enum InstallStatus {
    NotStarted,
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
}

/// Background comparison-encode state. The thread sends one value 
/// through the receiver and exits.
struct PendingCompare {
    rx: crossbeam_channel::Receiver<
        Result<av1tuner_core::ComparisonResult, av1tuner_core::SampleError>
    >,
    started_at: std::time::Instant,
}

/// Background-probe state. The thread sends one value through the 
/// receiver and exits. App polls the receiver in update().
struct PendingProbe {
    rx: crossbeam_channel::Receiver<Result<probe::VideoInfo, probe::ProbeError>>,
    /// Captured inputs so we can complete start_job once probe finishes.
    input: PathBuf,
    output: PathBuf,
    started_at: std::time::Instant,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = UserConfig::load();
        let default_concurrency = std::thread::available_parallelism()
            .map(|n| (n.get() as u32 / 2).max(1))
            .unwrap_or(2);
        let concurrency = cfg.concurrency_override.unwrap_or(default_concurrency);
        Self {
            input_path: String::new(),
            output_path: String::new(),
            settings: cfg.last_settings.clone(),
            audio: cfg.last_audio.clone(),
            chunk_seconds: cfg.last_chunk_seconds,
            concurrency,
            default_concurrency,
            preflight: None,
            job: None,
            chunks: Vec::new(),
            scheduler: None,
            log_lines: Vec::new(),
            eta: EtaTracker::new(),
            job_started: None,
            source_bytes: None,
            final_output: None,
            final_output_bytes: None,
            final_duration_s: None,
            resume_offered: false,
            auto_cleanup: cfg.auto_cleanup,
            use_cache: cfg.use_cache,
            cache_mode: cfg.cache_mode,
            last_saved_config: cfg,
            last_save_at: std::time::Instant::now(),
            pending_probe: None,
            probe_will_start: false,
            cached_info: None,
            pending_compare: None,
            compare_result: None,
            compare_cancel: std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false)),
            compare_sample_seconds: 8.0,
            pending_install: None,
            install_status: InstallStatus::NotStarted,
            pending_prediction: None,
            prediction_cancel: std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false)),
            prediction_result: None,
            prediction_sample_seconds: 15.0,
            ui_mode: UiMode::SingleFile,
            groups: vec![av1tuner_core::MergeGroup::new("Group 1")],
            active_group: 0,
            global_template: av1tuner_core::group::DEFAULT_TEMPLATE.to_string(),
            pending_group: None,
            group_status: String::new(),
        }
    }
    
    /// Build a UserConfig from the current UI state. Used to compare 
    /// against last_saved_config and decide whether to write to disk.
    fn current_config(&self) -> UserConfig {
        UserConfig {
            last_settings: self.settings.clone(),
            last_audio: self.audio.clone(),
            last_chunk_seconds: self.chunk_seconds,
            concurrency_override: if self.concurrency == self.default_concurrency {
                None
            } else {
                Some(self.concurrency)
            },
            last_input_dir: PathBuf::from(&self.input_path).parent()
                .map(|p| p.to_path_buf()),
            last_output_dir: PathBuf::from(&self.output_path).parent()
                .map(|p| p.to_path_buf()),
            auto_cleanup: self.auto_cleanup,
            use_cache: self.use_cache,
            cache_mode: self.cache_mode,
        }
    }
    
    /// If settings changed since last save, persist them. Cheap to call 
    /// frequently — checks for change first, and debounces writes to 
    /// at most once per second so typing in path fields doesn't thrash 
    /// the disk.
    fn maybe_save_config(&mut self) {
        // Debounce: don't even check if we saved recently.
        if self.last_save_at.elapsed() < std::time::Duration::from_millis(1000) {
            return;
        }
        let now = self.current_config();
        // Quick check: serialize-compare. Cheap because the struct is tiny.
        let now_json = serde_json::to_string(&now).unwrap_or_default();
        let last_json = serde_json::to_string(&self.last_saved_config).unwrap_or_default();
        if now_json != last_json {
            now.save();
            self.last_saved_config = now;
            self.last_save_at = std::time::Instant::now();
        }
    }
    
    fn append_log(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        info!("{}", s);
        self.log_lines.push(s);
        if self.log_lines.len() > 500 {
            let drop_count = self.log_lines.len() - 500;
            self.log_lines.drain(..drop_count);
        }
    }
    
    fn derived_output(&self) -> String {
        if self.input_path.is_empty() {
            return String::new();
        }
        let p = PathBuf::from(&self.input_path);
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "output".to_string());
        let parent = p.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        // Codec-specific tag so the user can tell at a glance which 
        // encode produced which file. Container always Matroska.
        let tag = match self.settings.kind {
            EncoderKind::SvtAv1 => "av1",
            EncoderKind::Vvenc => "vvc",
        };
        parent.join(format!("{}.{}.mkv", stem, tag)).to_string_lossy().to_string()
    }
    
    fn pick_input_file(&mut self) {
        // rfd's blocking API is fine here — file picker is modal and 
        // we're not encoding. For mid-encode picks we'd use AsyncFileDialog.
        let dialog = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mkv", "webm", "mov", "avi", "y4m", "ivf"])
            .add_filter("All files", &["*"])
            .set_title("Pick input video");
        if let Some(path) = dialog.pick_file() {
            self.input_path = path.to_string_lossy().to_string();
            if self.output_path.is_empty() {
                self.output_path = self.derived_output();
            }
            self.check_for_resume();
        }
    }
    
    fn pick_output_file(&mut self) {
        let dialog = rfd::FileDialog::new()
            .add_filter("Matroska", &["mkv"])
            .add_filter("All files", &["*"])
            .set_title("Pick output path")
            .set_file_name(
                PathBuf::from(&self.derived_output())
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "output.av1.mkv".to_string())
            );
        if let Some(path) = dialog.save_file() {
            self.output_path = path.to_string_lossy().to_string();
            self.check_for_resume();
        }
    }
    
    /// Look for an existing `<output>.av1tuner/job.json`. If present, 
    /// offer to resume by loading the existing chunks + job state.
    fn check_for_resume(&mut self) {
        // Path changed → cached probe info is stale.
        self.cached_info = None;
        self.compare_result = None;
        if self.output_path.is_empty() { return; }
        let output = PathBuf::from(&self.output_path);
        let state_dir = derive_state_dir(&output);
        let job_json = state_dir.join("job.json");
        if !job_json.exists() {
            self.resume_offered = false;
            return;
        }
        // Try to load it.
        match JobState::load(&state_dir) {
            Ok(job) => {
                let chunks = job.load_chunks().unwrap_or_default();
                let done = chunks.iter().filter(|c| c.status == ChunkStatus::Done).count();
                let total = chunks.len();
                self.append_log(format!(
                    "found existing job state: {} of {} chunks already done (codec={})",
                    done, total, job.settings.kind.display_name()
                ));
                // If the user has the UI set to a different codec than 
                // what was used previously, resuming would mix codecs.
                // Don't offer resume in that case — surface a clear warning.
                if job.settings.kind != self.settings.kind {
                    self.append_log(format!(
                        "current UI codec ({}) differs from saved job ({}). \
                         To resume, switch back; to start fresh, discard previous.",
                        self.settings.kind.display_name(),
                        job.settings.kind.display_name(),
                    ));
                }
                if total > 0 && done > 0 && job.status != JobStatus::Done
                    && job.settings.kind == self.settings.kind
                {
                    self.resume_offered = true;
                    self.chunks = chunks;
                    self.job = Some(job);
                }
            }
            Err(e) => {
                self.append_log(format!("found state dir but couldn't load: {}", e));
            }
        }
    }
    
    fn run_preflight(&mut self) {
        let input = PathBuf::from(&self.input_path);
        let output = PathBuf::from(&self.output_path);
        let state_dir = derive_state_dir(&output);
        let report = preflight::run(
            &input, &output, &state_dir, &self.settings, &self.audio,
        );
        self.append_log(format!(
            "preflight: {} checks, {} warnings, fatal={}",
            report.checks.len(), report.warnings(), report.fatal()
        ));
        self.preflight = Some(report);
    }
    
    /// Kicks off a probe in the background. Returns immediately. The 
    /// rest of the start flow runs in `finish_start_job` once the 
    /// probe result arrives, polled from `update`.
    fn start_job(&mut self) {
        let input = PathBuf::from(&self.input_path);
        let output = PathBuf::from(&self.output_path);
        
        // Source size for the completion summary — cheap, do it now.
        self.source_bytes = std::fs::metadata(&input).ok().map(|m| m.len());
        
        self.append_log(format!("probing {}...", input.display()));
        
        let (tx, rx) = bounded::<Result<probe::VideoInfo, probe::ProbeError>>(1);
        let probe_input = input.clone();
        std::thread::Builder::new()
            .name("av1tuner-probe".into())
            .spawn(move || {
                let result = probe::probe(&probe_input);
                let _ = tx.send(result);
            })
            .expect("spawn probe thread");
        
        self.pending_probe = Some(PendingProbe {
            rx, input, output,
            started_at: std::time::Instant::now(),
        });
        self.probe_will_start = true;
    }
    
    /// Kick off a sample-encode comparison between SVT-AV1 and VVenC, 
    /// using the user's current per-codec settings (so the test reflects 
    /// what they'd actually use). Requires a cached probe.
    fn start_compare(&mut self) {
        let Some(info) = self.cached_info.clone() else {
            self.append_log(
                "compare: need probe info first (click Probe)".to_string()
            );
            return;
        };
        if self.pending_compare.is_some() {
            self.append_log("compare: already running".to_string());
            return;
        }
        
        // Quick pre-check that both encoder binaries exist. Saves the 
        // user from waiting for the AV1 encode to complete just to learn 
        // VVenC isn't installed.
        if which::which("SvtAv1EncApp").is_err() {
            self.append_log(
                "compare: SvtAv1EncApp not on PATH — install svt-av1".to_string()
            );
            return;
        }
        let vvenc_available = which::which("vvencapp").is_ok()
            || av1tuner_core::vvenc_install::InstallPaths::default_paths()
                .map(|p| p.is_installed()).unwrap_or(false);
        if !vvenc_available {
            self.append_log(
                "compare: vvencapp not found — use the Install VVenC button first".to_string()
            );
            return;
        }
        
        let source = PathBuf::from(&self.input_path);
        if !source.exists() {
            self.append_log(format!("compare: input not found: {}", source.display()));
            return;
        }
        
        // Build the two settings — one per codec — using the user's 
        // current state for each codec's specific fields. We do NOT use 
        // self.settings.kind directly; we encode with BOTH codecs.
        let settings_av1 = self.settings.with_kind(EncoderKind::SvtAv1);
        let settings_vvc = self.settings.with_kind(EncoderKind::Vvenc);
        let settings_list = vec![settings_av1, settings_vvc];
        
        // Temp dir for sample outputs. Use the system temp dir under a 
        // per-run name so concurrent compares don't collide.
        let temp_dir = std::env::temp_dir()
            .join(format!("av1tuner_compare_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs()).unwrap_or(0),
            ));
        
        // Clean up any previous comparison results.
        if let Some(prev) = self.compare_result.take() {
            av1tuner_core::sample::cleanup_samples(&prev);
        }
        
        // Reset cancel flag.
        self.compare_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        let cancel_for_thread = self.compare_cancel.clone();
        
        let sample_seconds = self.compare_sample_seconds;
        self.append_log(format!(
            "compare: encoding {:.0}s sample with both codecs at current settings...",
            sample_seconds
        ));
        
        let (tx, rx) = bounded::<Result<av1tuner_core::ComparisonResult, av1tuner_core::SampleError>>(1);
        std::thread::Builder::new()
            .name("av1tuner-compare".into())
            .spawn(move || {
                let result = av1tuner_core::sample::run_comparison(
                    &source, &info, &settings_list, sample_seconds,
                    &temp_dir, cancel_for_thread,
                );
                let _ = tx.send(result);
            })
            .expect("spawn compare thread");
        
        self.pending_compare = Some(PendingCompare {
            rx,
            started_at: std::time::Instant::now(),
        });
    }
    
    /// Poll for finished comparison result.
    fn poll_pending_compare(&mut self) {
        let try_result = match self.pending_compare.as_ref() {
            Some(p) => p.rx.try_recv(),
            None => return,
        };
        let result = match try_result {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.append_log("compare thread disconnected".to_string());
                self.pending_compare = None;
                return;
            }
        };
        let pending = self.pending_compare.take().expect("just checked above");
        let elapsed = pending.started_at.elapsed();
        
        match result {
            Ok(r) => {
                self.append_log(format!(
                    "compare complete in {:.1}s ({} frames sampled)",
                    elapsed.as_secs_f64(), r.sample_frames
                ));
                for s in &r.samples {
                    self.append_log(format!(
                        "  {}: {} bytes in {:.1}s",
                        s.kind.display_name(),
                        format_bytes(s.bytes),
                        s.elapsed_ms as f64 / 1000.0
                    ));
                }
                self.compare_result = Some(r);
            }
            Err(e) => {
                self.append_log(format!("compare failed: {}", e));
            }
        }
    }
    
    fn cancel_compare(&mut self) {
        if self.pending_compare.is_some() {
            self.compare_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            self.append_log("compare cancel requested".to_string());
        }
    }
    
    /// Start a VVenC install in the background.
    fn start_install_vvenc(&mut self) {
        use av1tuner_core::vvenc_install::{check_build_tools, install_async, InstallPaths};
        
        if self.pending_install.is_some() {
            self.append_log("install already in progress".to_string());
            return;
        }
        
        // Pre-flight the toolchain so we don't start a doomed build.
        if let Err(missing) = check_build_tools() {
            self.append_log(format!(
                "install: missing build tool: {} — install with your package manager \
                 (e.g. `sudo dnf install {}` on Fedora)",
                missing,
                // Hint the package name when possible.
                match missing.as_str() {
                    "git" => "git",
                    "cmake" => "cmake",
                    "make" => "make",
                    _ => "gcc-c++ cmake make git",
                }
            ));
            self.install_status = InstallStatus::Failed(format!("missing {}", missing));
            return;
        }
        
        let Some(paths) = InstallPaths::default_paths() else {
            self.append_log("install: couldn't determine a data directory".to_string());
            return;
        };
        
        self.append_log(format!(
            "starting VVenC install — clone + cmake + build → {}",
            paths.vvencapp_path.display()
        ));
        self.append_log(
            "this builds from source and takes ~5-15 minutes depending on your CPU".to_string()
        );
        
        let (rx, cancel) = install_async(paths);
        self.pending_install = Some(PendingInstall {
            rx, cancel,
            started_at: std::time::Instant::now(),
            current_step: "starting".to_string(),
        });
        self.install_status = InstallStatus::Running;
    }
    
    fn poll_pending_install(&mut self) {
        use av1tuner_core::vvenc_install::InstallEvent;
        
        let events: Vec<InstallEvent> = match self.pending_install.as_ref() {
            Some(p) => p.rx.try_iter().collect(),
            None => return,
        };
        
        // The current_step needs to be mutated inside the loop; we'll 
        // collect the new step text and apply at the end.
        let mut new_step: Option<String> = None;
        let mut finished = false;
        for ev in events {
            match ev {
                InstallEvent::StepStarted(step) => {
                    self.append_log(format!("install: {}...", step));
                    new_step = Some(step);
                }
                InstallEvent::Output(line) => {
                    // Most lines are cmake/make chatter; log only the 
                    // ones that look useful so the log doesn't fill up.
                    if is_useful_output(&line) {
                        self.append_log(format!("  {}", line));
                    }
                }
                InstallEvent::StepFinished(step) => {
                    self.append_log(format!("install: {} done", step));
                }
                InstallEvent::Done(path) => {
                    self.append_log(format!(
                        "✓ VVenC installed at {} (took {:.0}s)",
                        path.display(),
                        self.pending_install.as_ref()
                            .map(|p| p.started_at.elapsed().as_secs_f64())
                            .unwrap_or(0.0)
                    ));
                    self.install_status = InstallStatus::Succeeded;
                    finished = true;
                }
                InstallEvent::Failed(msg) => {
                    self.append_log(format!("install failed: {}", msg));
                    self.install_status = InstallStatus::Failed(msg);
                    finished = true;
                }
                InstallEvent::Cancelled => {
                    self.append_log("install cancelled".to_string());
                    self.install_status = InstallStatus::Cancelled;
                    finished = true;
                }
            }
        }
        
        if let Some(step) = new_step {
            if let Some(p) = self.pending_install.as_mut() {
                p.current_step = step;
            }
        }
        if finished {
            self.pending_install = None;
        }
    }
    
    fn cancel_install(&mut self) {
        if let Some(p) = &self.pending_install {
            p.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            self.append_log("install cancel requested".to_string());
        }
    }
    
    /// Kick off Phase 0 prediction analysis on a sample. Requires 
    /// cached probe info (we need to know frame count + fps).
    fn start_prediction(&mut self) {
        let Some(info) = self.cached_info.clone() else {
            self.append_log(
                "prediction: need probe info first (click Probe)".to_string()
            );
            return;
        };
        if self.pending_prediction.is_some() {
            self.append_log("prediction: already running".to_string());
            return;
        }
        if self.settings.kind != EncoderKind::SvtAv1 {
            self.append_log(
                "prediction: only supported on SVT-AV1 (switch codec to run)".to_string()
            );
            return;
        }
        if which::which("SvtAv1EncApp").is_err() {
            self.append_log(
                "prediction: SvtAv1EncApp not on PATH".to_string()
            );
            return;
        }
        
        let source = PathBuf::from(&self.input_path);
        if !source.exists() {
            self.append_log(format!(
                "prediction: input not found: {}", source.display()
            ));
            return;
        }
        
        // Clone what the background thread needs.
        let settings = self.settings.clone();
        let sample_seconds = self.prediction_sample_seconds;
        self.prediction_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        let cancel_for_thread = self.prediction_cancel.clone();
        
        self.append_log(format!(
            "prediction: analyzing {:.0}s sample for inter-frame structure...",
            sample_seconds
        ));
        
        let (tx, rx) = bounded::<Result<av1tuner_core::PredictionStats, av1tuner_core::PredictionError>>(1);
        std::thread::Builder::new()
            .name("av1tuner-prediction".into())
            .spawn(move || {
                let cancel_fn = move || cancel_for_thread.load(
                    std::sync::atomic::Ordering::SeqCst);
                let result = av1tuner_core::prediction_stats::run_analysis(
                    &source, &info, &settings, sample_seconds, &cancel_fn,
                );
                let _ = tx.send(result);
            })
            .expect("spawn prediction thread");
        
        self.pending_prediction = Some(PendingPrediction {
            rx,
            started_at: std::time::Instant::now(),
        });
    }
    
    fn poll_pending_prediction(&mut self) {
        let try_result = match self.pending_prediction.as_ref() {
            Some(p) => p.rx.try_recv(),
            None => return,
        };
        let result = match try_result {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.append_log("prediction thread disconnected".to_string());
                self.pending_prediction = None;
                return;
            }
        };
        let pending = self.pending_prediction.take().expect("just checked above");
        let elapsed = pending.started_at.elapsed();
        match result {
            Ok(stats) => {
                let verdict = av1tuner_core::prediction_stats::verdict_for(&stats);
                self.append_log(format!(
                    "prediction analysis complete in {:.1}s — verdict: {}",
                    elapsed.as_secs_f64(),
                    verdict.label()
                ));
                self.append_log(format!(
                    "  {} frames, {} keyframes, {} inter — P/I ratio = {:.3}",
                    stats.total_frames, stats.keyframe_count, 
                    stats.inter_count, stats.p_to_i_ratio
                ));
                if !stats.stat_report_parsed {
                    self.append_log(
                        "  (stat-report parsing fell back to estimation —                          numbers are coarse)".to_string()
                    );
                }
                self.prediction_result = Some(stats);
            }
            Err(e) => {
                self.append_log(format!("prediction failed: {}", e));
            }
        }
    }
    
    fn cancel_prediction(&mut self) {
        if self.pending_prediction.is_some() {
            self.prediction_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            self.append_log("prediction cancel requested".to_string());
        }
    }
    
    // ---- Groups (merge tabs) ----
    
    fn active_group_mut(&mut self) -> Option<&mut av1tuner_core::MergeGroup> {
        self.groups.get_mut(self.active_group)
    }
    
    fn add_group(&mut self) {
        let n = self.groups.len() + 1;
        self.groups.push(av1tuner_core::MergeGroup::new(format!("Group {}", n)));
        self.active_group = self.groups.len() - 1;
    }
    
    fn remove_group(&mut self, index: usize) {
        if self.groups.len() <= 1 { return; }  // keep at least one
        if index < self.groups.len() {
            self.groups.remove(index);
            if self.active_group >= self.groups.len() {
                self.active_group = self.groups.len() - 1;
            }
        }
    }
    
    /// Fill width/height for any clips in the active group that don\'t 
    /// have them yet, using the fast (instant) probe. Lets the mismatch 
    /// flag + per-clip dimensions show immediately after import.
    fn probe_active_group_dims(&mut self) {
        let idx = self.active_group;
        if idx >= self.groups.len() { return; }
        let count = self.groups[idx].clips.len();
        for c in 0..count {
            if self.groups[idx].clips[c].width.is_some() {
                continue;
            }
            let path = self.groups[idx].clips[c].path.clone();
            if let Ok(info) = av1tuner_core::probe::probe_fast(&path) {
                self.groups[idx].clips[c].width = Some(info.width);
                self.groups[idx].clips[c].height = Some(info.height);
            }
        }
    }
    
    /// Pick a folder, scan for videos, add them to the active group.
    fn import_folder_to_active(&mut self, recursive: bool) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else { return; };
        match av1tuner_core::discovery::scan_folder(&folder, recursive) {
            Ok(found) => {
                let count = found.len();
                if let Some(group) = self.active_group_mut() {
                    for v in found {
                        group.clips.push(av1tuner_core::GroupClip::new(v.path, v.bytes));
                    }
                }
                self.append_log(format!(
                    "imported {} videos from {} ({})",
                    count, folder.display(),
                    if recursive { "recursive" } else { "top-level" }
                ));
                self.probe_active_group_dims();
            }
            Err(e) => self.append_log(format!("folder scan failed: {}", e)),
        }
    }
    
    /// Pick a zip, inspect (warn if pointless), extract videos, add to 
    /// the active group.
    fn import_zip_to_active(&mut self) {
        let Some(zip_path) = rfd::FileDialog::new()
            .add_filter("Zip archive", &["zip"])
            .pick_file() else { return; };
        
        // Inspect first to warn.
        match av1tuner_core::discovery::inspect_zip(&zip_path) {
            Ok(inspection) => {
                if inspection.video_names.is_empty() {
                    self.append_log("zip contains no recognized video files".to_string());
                    return;
                }
                if inspection.zip_was_pointless() {
                    self.append_log(format!(
                        "⚠ this zip barely compressed its video ({:.0}% of original) \
                         — zipping already-compressed video doesn't help. Extracting anyway.",
                        inspection.video_compression_ratio() * 100.0
                    ));
                }
            }
            Err(e) => {
                self.append_log(format!("zip inspect failed: {}", e));
                return;
            }
        }
        
        // Extract to a temp dir next to the zip.
        let dest = std::env::temp_dir().join(format!(
            "av1tuner_unzip_{}", std::process::id()
        ));
        match av1tuner_core::discovery::extract_videos_from_zip(&zip_path, &dest) {
            Ok(paths) => {
                let count = paths.len();
                if let Some(group) = self.active_group_mut() {
                    for p in paths {
                        let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                        group.clips.push(av1tuner_core::GroupClip::new(p, bytes));
                    }
                }
                self.append_log(format!("extracted {} videos from zip", count));
                self.probe_active_group_dims();
            }
            Err(e) => self.append_log(format!("zip extract failed: {}", e)),
        }
    }
    
    /// Start running the active group.
    fn start_active_group(&mut self) {
        if self.pending_group.is_some() {
            self.append_log("a group is already running".to_string());
            return;
        }
        let idx = self.active_group;
        let Some(group) = self.groups.get(idx).cloned() else { return; };
        if group.active_count() == 0 {
            self.append_log("group has no enabled clips".to_string());
            return;
        }
        
        let config = av1tuner_core::GroupRunConfig {
            global_settings: self.settings.clone(),
            global_template: self.global_template.clone(),
            chunk_seconds: self.chunk_seconds,
            concurrency: self.concurrency.max(1) as usize,
            work_root: std::env::temp_dir().join("av1tuner_groups"),
        };
        
        self.append_log(format!(
            "starting group '{}' — {} clips → merged output",
            group.name, group.active_count()
        ));
        let total = group.active_count();
        let run = av1tuner_core::run_group(group, config);
        self.pending_group = Some(PendingGroup {
            run,
            started_at: std::time::Instant::now(),
            group_index: idx,
            progress: (0, total),
            last_message: "starting...".to_string(),
        });
        self.group_status = "running".to_string();
    }
    
    fn cancel_active_group(&mut self) {
        if let Some(p) = &self.pending_group {
            p.run.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            self.append_log("group cancel requested".to_string());
        }
    }
    
    fn poll_pending_group(&mut self) {
        use av1tuner_core::GroupEvent;
        let events: Vec<GroupEvent> = match self.pending_group.as_ref() {
            Some(p) => p.run.events.try_iter().collect(),
            None => return,
        };
        let mut finished = false;
        for ev in events {
            match ev {
                GroupEvent::ClipStarted { index, total, name } => {
                    if let Some(p) = self.pending_group.as_mut() {
                        p.progress = (index, total);
                        p.last_message = format!("encoding clip {}/{}: {}", index + 1, total, name);
                    }
                    self.append_log(format!("group: clip {}/{} — {}", index + 1, total, name));
                }
                GroupEvent::ClipProgress { index, message } => {
                    if let Some(p) = self.pending_group.as_mut() {
                        p.last_message = format!("clip {}: {}", index + 1, message);
                    }
                }
                GroupEvent::ClipFinished { index, bytes } => {
                    self.append_log(format!(
                        "group: clip {} done ({})", index + 1, format_bytes(bytes)));
                }
                GroupEvent::ClipFailed { index, error } => {
                    self.append_log(format!("group: clip {} FAILED: {}", index + 1, error));
                }
                GroupEvent::Stitching => {
                    if let Some(p) = self.pending_group.as_mut() {
                        p.last_message = "stitching clips into final output...".to_string();
                    }
                    self.append_log("group: stitching merged output with chapters".to_string());
                }
                GroupEvent::Done { output, output_bytes, source_bytes } => {
                    let savings = if source_bytes > 0 {
                        (1.0 - output_bytes as f64 / source_bytes as f64) * 100.0
                    } else { 0.0 };
                    self.append_log(format!(
                        "✓ group done: {} ({}, {:.1}% smaller than sources)",
                        output.display(), format_bytes(output_bytes), savings));
                    self.group_status = format!(
                        "done: {} ({:.0}% smaller)", output.display(), savings);
                    finished = true;
                }
                GroupEvent::Failed(msg) => {
                    self.append_log(format!("group failed: {}", msg));
                    self.group_status = format!("failed: {}", msg);
                    finished = true;
                }
                GroupEvent::Cancelled => {
                    self.append_log("group cancelled".to_string());
                    self.group_status = "cancelled".to_string();
                    finished = true;
                }
            }
        }
        if finished {
            self.pending_group = None;
        }
    }
    
    
    /// Probe only — no encode follows. Used by Compare Codecs to get 
    /// VideoInfo before sample encoding.
    fn start_probe_only(&mut self) {
        let input = PathBuf::from(&self.input_path);
        let output = PathBuf::from(&self.output_path);
        self.source_bytes = std::fs::metadata(&input).ok().map(|m| m.len());
        self.append_log(format!("probing {}...", input.display()));
        let (tx, rx) = bounded::<Result<probe::VideoInfo, probe::ProbeError>>(1);
        let probe_input = input.clone();
        std::thread::Builder::new()
            .name("av1tuner-probe-only".into())
            .spawn(move || {
                let _ = tx.send(probe::probe(&probe_input));
            })
            .expect("spawn probe thread");
        self.pending_probe = Some(PendingProbe {
            rx, input, output,
            started_at: std::time::Instant::now(),
        });
        self.probe_will_start = false;
    }
    
    /// Poll for a finished background probe. If one is pending and the 
    /// result has arrived, run the rest of the start sequence (chunk 
    /// planning + scheduler spawn).
    fn poll_pending_probe(&mut self) {
        // Peek at the receiver to see if a result is available. Avoid 
        // borrowing self.pending_probe across a &mut self assignment.
        let try_result = match self.pending_probe.as_ref() {
            Some(p) => p.rx.try_recv(),
            None => return,
        };
        
        let result = match try_result {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.append_log("probe thread disconnected without sending a result");
                self.pending_probe = None;
                return;
            }
        };
        
        let pending = self.pending_probe.take().expect("just checked above");
        let elapsed = pending.started_at.elapsed();
        
        let info = match result {
            Ok(i) => i,
            Err(e) => {
                self.append_log(format!("probe failed after {:.1}s: {}",
                    elapsed.as_secs_f64(), e));
                return;
            }
        };
        self.append_log(format!(
            "probed in {:.1}s: {}x{} {:.2}fps {} frames ({:.1}s) codec={}",
            elapsed.as_secs_f64(),
            info.width, info.height, info.fps, info.frame_count,
            info.duration_s, info.codec
        ));
        if info.audio_streams.is_empty() {
            self.append_log("no audio tracks in source".to_string());
        } else {
            for (i, a) in info.audio_streams.iter().enumerate() {
                self.append_log(format!(
                    "audio[{}]: {} {}ch @ {}Hz{}",
                    i, a.codec, a.channels, a.sample_rate,
                    if a.language.is_empty() { String::new() }
                    else { format!(" ({})", a.language) }
                ));
            }
        }
        
        // Cache the probe result so the compare-codecs flow can reuse it.
        self.cached_info = Some(info.clone());
        
        if self.probe_will_start {
            self.finish_start_job(pending.input, pending.output, info);
        } else {
            self.append_log("probe complete (no encode queued)".to_string());
        }
    }
    
    /// Completes the job-start sequence after probe finishes.
    fn finish_start_job(&mut self, input: PathBuf, output: PathBuf, info: probe::VideoInfo) {
        
        // Cache short-circuit: if this exact source+settings was already 
        // encoded and the cached output is intact, reuse it instead of 
        // re-encoding. Skipped when resuming (the user explicitly wants 
        // to continue a specific job) or when the cached output IS the 
        // requested output (nothing to do).
        if !self.resume_offered {
            if let Some(cached) = self.check_cache(&input) {
                if cached != output {
                    match std::fs::copy(&cached, &output) {
                        Ok(bytes) => {
                            self.append_log(format!(
                                "cache HIT: reused prior encode ({}), skipped re-encoding",
                                format_bytes(bytes)
                            ));
                            self.final_output = Some(output);
                            self.final_output_bytes = Some(bytes);
                            self.final_duration_s = Some(0);
                            return;
                        }
                        Err(e) => {
                            // Copy failed (permissions, disk full) — fall 
                            // through to a normal encode rather than error.
                            self.append_log(format!(
                                "cache hit but copy failed ({}); encoding normally", e
                            ));
                        }
                    }
                } else {
                    self.append_log(
                        "cache HIT: output already up to date, nothing to do".to_string());
                    self.final_output = Some(output);
                    self.final_output_bytes = std::fs::metadata(&input).ok().map(|m| m.len());
                    return;
                }
            }
        }
        
        // Either start fresh or resume an existing job at the same output.
        let mut job = if self.resume_offered {
            // Use the loaded job state but refresh settings + boundaries 
            // from current UI in case the user changed something.
            // Conservative: warn if settings differ; for M1 we just trust 
            // the on-disk state and replay it.
            let mut j = self.job.take().unwrap_or_else(|| {
                let mut j = JobState::new(input.clone(), output.clone(), self.settings.clone());
                j.info = Some(info.clone());
                j.chunk_boundaries = boundaries(&info, &SceneStrategy::FixedSeconds(self.chunk_seconds));
                j
            });
            j.concurrency = self.concurrency;
            j.audio = self.audio.clone();
            j
        } else {
            let mut j = JobState::new(input.clone(), output.clone(), self.settings.clone());
            j.info = Some(info.clone());
            j.chunk_boundaries = boundaries(&info, &SceneStrategy::FixedSeconds(self.chunk_seconds));
            j.concurrency = self.concurrency;
            j.audio = self.audio.clone();
            j
        };
        
        job.status = JobStatus::Probed;
        let chunk_count = job.chunk_count();
        self.append_log(format!(
            "planned {} chunks of ~{:.1}s each (concurrency={})",
            chunk_count, self.chunk_seconds, job.concurrency
        ));
        
        if let Err(e) = job.save() {
            self.append_log(format!("save job state: {}", e));
            return;
        }
        
        match job.load_chunks() {
            Ok(c) => {
                let done = c.iter().filter(|c| c.status == ChunkStatus::Done).count();
                if done > 0 {
                    self.append_log(format!("reusing {} already-done chunks from previous run", done));
                }
                // Pre-seed the ETA tracker with prior encode_ms data.
                self.eta = from_chunks(&c, 10);
                self.chunks = c;
            }
            Err(e) => {
                self.append_log(format!("load chunks: {}", e));
                return;
            }
        }
        
        let scheduler = Scheduler::spawn(job.clone());
        self.job = Some(job);
        self.scheduler = Some(scheduler);
        self.final_output = None;
        self.final_output_bytes = None;
        self.final_duration_s = None;
        self.resume_offered = false;
        self.job_started = Some(std::time::Instant::now());
    }
    
    fn poll_scheduler_events(&mut self) {
        let Some(scheduler) = &self.scheduler else { return };
        let events: Vec<SchedulerEvent> = scheduler.events().try_iter().collect();
        let mut all_done = false;
        let mut cancelled = false;
        for event in events {
            match event {
                SchedulerEvent::Started { total, already_done } => {
                    self.append_log(format!(
                        "scheduler started: {} chunks total, {} already done",
                        total, already_done
                    ));
                }
                SchedulerEvent::ChunkStarted { index, attempt } => {
                    if let Some(c) = self.chunks.iter_mut().find(|c| c.index == index) {
                        c.status = ChunkStatus::Encoding;
                        c.attempts = attempt;
                    }
                }
                SchedulerEvent::ChunkFinished { index, elapsed_ms, output_bytes } => {
                    let frames = if let Some(c) = self.chunks.iter_mut().find(|c| c.index == index) {
                        c.status = ChunkStatus::Done;
                        c.encode_ms = Some(elapsed_ms);
                        c.output_bytes = Some(output_bytes);
                        c.frame_count()
                    } else { 0 };
                    self.eta.record_chunk(frames, elapsed_ms);
                    self.append_log(format!(
                        "chunk {} done ({}ms, {} bytes, {:.1} fps)",
                        index, elapsed_ms, output_bytes,
                        if elapsed_ms > 0 { frames as f64 * 1000.0 / elapsed_ms as f64 } else { 0.0 }
                    ));
                }
                SchedulerEvent::ChunkRetrying { index, attempt, error } => {
                    if let Some(c) = self.chunks.iter_mut().find(|c| c.index == index) {
                        c.status = ChunkStatus::Failed;
                        c.attempts = attempt;
                        c.last_error = Some(error.clone());
                    }
                    self.append_log(format!(
                        "chunk {} attempt {} failed; retrying: {}",
                        index, attempt, error
                    ));
                }
                SchedulerEvent::ChunkSkipped { index, error } => {
                    if let Some(c) = self.chunks.iter_mut().find(|c| c.index == index) {
                        c.status = ChunkStatus::Skipped;
                        c.last_error = Some(error.clone());
                    }
                    self.append_log(format!("chunk {} SKIPPED: {}", index, error));
                }
                SchedulerEvent::Progress { .. } => {}
                SchedulerEvent::AllChunksDone { done, skipped, total } => {
                    self.append_log(format!(
                        "all chunks done: {}/{} ok, {} skipped",
                        done, total, skipped
                    ));
                    all_done = true;
                }
                SchedulerEvent::Cancelled => {
                    self.append_log("scheduler cancelled".to_string());
                    cancelled = true;
                }
                SchedulerEvent::FatalError(msg) => {
                    self.append_log(format!("FATAL: {}", msg));
                    cancelled = true;
                }
            }
        }
        if all_done {
            self.run_concat();
        }
        if cancelled {
            self.scheduler = None;
            self.job_started = None;
        }
    }
    
    /// Hash the encode settings that affect output, for the cache 
    /// signature. Includes everything that changes the bytes: codec, 
    /// quantizer, preset, tune, pixel format, chunk size, audio mode.
    fn settings_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // Codec + quantizer + preset.
        format!("{:?}", self.settings.kind).hash(&mut h);
        self.settings.quantizer().hash(&mut h);
        self.settings.preset.hash(&mut h);
        self.settings.tune.hash(&mut h);
        format!("{:?}", self.settings.vvenc_preset).hash(&mut h);
        self.settings.vvenc_qpa.hash(&mut h);
        self.settings.pixel_format.hash(&mut h);
        self.settings.video_filter.hash(&mut h);
        // Chunk size (affects keyframe placement → output bytes).
        ((self.chunk_seconds * 1000.0) as u64).hash(&mut h);
        // Audio mode affects the final mux.
        format!("{:?}", self.audio.mode).hash(&mut h);
        h.finish()
    }
    
    /// Check the cache before encoding: if this exact source + settings 
    /// was already encoded and the output is intact, returns the cached 
    /// output path. The caller can copy/point at it instead of encoding.
    fn check_cache(&self, source: &std::path::Path) -> Option<std::path::PathBuf> {
        if !self.use_cache {
            return None;
        }
        let sig = av1tuner_core::compute_signature(
            source, self.settings_hash(), self.cache_mode).ok()?;
        let manifest = av1tuner_core::CacheManifest::load();
        manifest.lookup(&sig).map(|e| e.output_path.clone())
    }
    
    fn run_concat(&mut self) {
        // Extract what we need from the job by value, ending the immutable 
        // borrow of self.job before we call any &mut self methods 
        // (append_log, etc) below.
        let (state_dir, source, output, audio_settings, encoder_kind, chunks_result) = 
            match self.job.as_ref() {
                Some(job) => (
                    job.state_dir.clone(),
                    job.input.clone(),
                    job.output.clone(),
                    job.audio.clone(),
                    job.settings.kind,
                    job.load_chunks(),
                ),
                None => return,
            };
        
        let chunks = match chunks_result {
            Ok(c) => c,
            Err(e) => {
                self.append_log(format!("concat: load chunks failed: {}", e));
                return;
            }
        };
        
        self.append_log("concatenating chunks...".to_string());
        let result = concat::concat(
            &state_dir, &chunks, &source, &output, &audio_settings, encoder_kind);
        
        match result {
            Ok(r) => {
                self.append_log(format!(
                    "concat done: {} chunks, {} skipped, {} bytes output",
                    r.chunks_concatenated, r.chunks_skipped, r.output_bytes
                ));
                self.final_output = Some(output.clone());
                self.final_output_bytes = Some(r.output_bytes);
                self.final_duration_s = self.job_started.map(|t| t.elapsed().as_secs());
                
                if let Some(job) = self.job.as_mut() {
                    job.status = JobStatus::Done;
                    job.touch();
                    let _ = job.save();
                }
                
                // Record this successful encode in the cache so a future 
                // run with the same source + settings can skip it.
                if self.use_cache {
                    let settings_hash = self.settings_hash();
                    match av1tuner_core::compute_signature(
                        &source, settings_hash, self.cache_mode)
                    {
                        Ok(sig) => {
                            let mut manifest = av1tuner_core::CacheManifest::load();
                            if manifest.record(sig, output.clone()).is_ok() {
                                let _ = manifest.save();
                                self.append_log("cache: recorded this encode".to_string());
                            }
                        }
                        Err(e) => {
                            self.append_log(format!("cache: couldn't sign source: {}", e));
                        }
                    }
                }
                
                // Auto-cleanup intermediates if enabled. The cache 
                // manifest (in the config dir) is untouched, so cache 
                // hits still work after cleanup.
                if self.auto_cleanup {
                    match av1tuner_core::clean_state_dir(&state_dir) {
                        Ok(report) if report.files_removed > 0 => {
                            self.append_log(format!(
                                "cleanup: removed {} intermediate files ({})",
                                report.files_removed,
                                format_bytes(report.bytes_freed)
                            ));
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                self.append_log(format!("concat failed: {}", e));
                if let Some(job) = self.job.as_mut() {
                    job.status = JobStatus::Failed;
                    job.touch();
                    let _ = job.save();
                }
            }
        }
        self.scheduler = None;
        self.job_started = None;
    }
    
    fn cancel_job(&mut self) {
        if let Some(s) = &self.scheduler {
            s.cancel();
            self.append_log("cancel requested (will stop within ~100ms)".to_string());
        }
    }
    
    fn progress(&self) -> JobProgress {
        let total = self.chunks.len();
        let done = self.chunks.iter().filter(|c| c.status == ChunkStatus::Done).count();
        let failed = self.chunks.iter().filter(|c| {
            matches!(c.status, ChunkStatus::Failed | ChunkStatus::Skipped)
        }).count();
        let encoding = self.chunks.iter().filter(|c| c.status == ChunkStatus::Encoding).count();
        JobProgress { total, done, failed, encoding }
    }
    
    /// Remaining frames across all pending + encoding chunks.
    fn remaining_frames(&self) -> u64 {
        self.chunks.iter()
            .filter(|c| matches!(c.status, 
                ChunkStatus::Pending | ChunkStatus::Encoding | ChunkStatus::Failed))
            .map(|c| c.frame_count())
            .sum()
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_pending_probe();
        self.poll_pending_compare();
        self.poll_pending_install();
        self.poll_pending_prediction();
        self.poll_pending_group();
        self.poll_scheduler_events();
        
        if self.scheduler.is_some() 
            || self.pending_probe.is_some() 
            || self.pending_compare.is_some()
            || self.pending_install.is_some()
            || self.pending_prediction.is_some()
            || self.pending_group.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("av1tuner");
                ui.label(RichText::new("M1 — single video, per-segment isolation, resume")
                    .weak().size(11.0));
            });
        });
        
        egui::SidePanel::left("log_panel")
            .resizable(true)
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.heading("Log");
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for line in &self.log_lines {
                            ui.label(RichText::new(line).monospace().size(11.0));
                        }
                    });
            });
        
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.render_main(ui);
            });
        });
        
        // Persist settings if anything changed. Cheap because we compare 
        // against the cached last_saved_config first.
        self.maybe_save_config();
    }
}

impl App {
    fn render_groups(&mut self, ui: &mut egui::Ui) {
        let running = self.pending_group.is_some();
        
        // --- Tab bar ---
        ui.horizontal_wrapped(|ui| {
            let group_count = self.groups.len();
            for i in 0..group_count {
                let name = self.groups[i].name.clone();
                let selected = i == self.active_group;
                let label = if self.groups[i].active_count() > 0 {
                    format!("{} ({})", name, self.groups[i].active_count())
                } else {
                    name
                };
                if ui.selectable_label(selected, label).clicked() {
                    self.active_group = i;
                }
            }
            if ui.button("+ Tab").on_hover_text("Add a new merge group").clicked() {
                self.add_group();
            }
        });
        ui.separator();
        
        // --- Active group editor ---
        let idx = self.active_group;
        if idx >= self.groups.len() { return; }
        
        // Group name + delete.
        let mut delete_requested = false;
        ui.horizontal(|ui| {
            ui.label("Group name:");
            ui.text_edit_singleline(&mut self.groups[idx].name);
            if self.groups.len() > 1 
                && ui.button("🗑 Delete tab").clicked() 
            {
                delete_requested = true;
            }
        });
        if delete_requested {
            self.remove_group(idx);
            return;
        }
        
        // Import buttons.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Add clips:").strong());
            if ui.add_enabled(!running, egui::Button::new("Add folder…"))
                .on_hover_text("Pick a folder; recursively finds videos in subfolders")
                .clicked() 
            {
                self.import_folder_to_active(true);
            }
            if ui.add_enabled(!running, egui::Button::new("Add folder (top-level only)"))
                .clicked() 
            {
                self.import_folder_to_active(false);
            }
            if ui.add_enabled(!running, egui::Button::new("Add files…")).clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Video", av1tuner_core::discovery::VIDEO_EXTENSIONS)
                    .pick_files() 
                {
                    for f in files {
                        let bytes = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                        self.groups[idx].clips.push(
                            av1tuner_core::GroupClip::new(f, bytes));
                    }
                    self.probe_active_group_dims();
                }
            }
            if ui.add_enabled(!running, egui::Button::new("Add from zip…"))
                .on_hover_text("Extract videos from a zip (warns if the zip didn't compress them)")
                .clicked() 
            {
                self.import_zip_to_active();
            }
        });
        
        // Resolution mismatch flag.
        if self.groups[idx].has_resolution_mismatch() {
            let res = self.groups[idx].distinct_resolutions();
            let res_str: Vec<String> = res.iter()
                .map(|(w, h)| format!("{}×{}", w, h)).collect();
            ui.colored_label(
                Color32::from_rgb(230, 180, 30),
                format!("⚑ Mixed resolutions: {}. Policy below will normalize them.",
                    res_str.join(", "))
            );
        }
        
        // Clip list with checkboxes + reorder + remove.
        ui.add_space(4.0);
        ui.group(|ui| {
            ui.label(RichText::new(format!("Clips ({})", self.groups[idx].clips.len())).strong());
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let clip_count = self.groups[idx].clips.len();
                let mut to_remove: Option<usize> = None;
                let mut move_up: Option<usize> = None;
                let mut move_down: Option<usize> = None;
                for c in 0..clip_count {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.groups[idx].clips[c].enabled, "");
                        // Reorder controls.
                        if ui.small_button("↑").clicked() && c > 0 {
                            move_up = Some(c);
                        }
                        if ui.small_button("↓").clicked() && c + 1 < clip_count {
                            move_down = Some(c);
                        }
                        let clip = &self.groups[idx].clips[c];
                        let fname = clip.path.file_name()
                            .and_then(|s| s.to_str()).unwrap_or("?");
                        let dim = match (clip.width, clip.height) {
                            (Some(w), Some(h)) => format!("  {}×{}", w, h),
                            _ => String::new(),
                        };
                        ui.label(format!("{}{}  ({})", 
                            fname, dim, format_bytes(clip.bytes)));
                        if ui.small_button("✕").clicked() {
                            to_remove = Some(c);
                        }
                    });
                }
                if let Some(c) = move_up {
                    self.groups[idx].clips.swap(c, c - 1);
                }
                if let Some(c) = move_down {
                    self.groups[idx].clips.swap(c, c + 1);
                }
                if let Some(c) = to_remove {
                    self.groups[idx].clips.remove(c);
                }
            });
        });
        
        // Resolution policy.
        ui.horizontal(|ui| {
            ui.label("Resolution policy:");
            let current = self.groups[idx].resolution_policy;
            egui::ComboBox::from_id_source("respolicy")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    use av1tuner_core::ResolutionPolicy as RP;
                    for p in [RP::ScaleToSmallest, RP::ScaleToLargest, RP::KeepNative] {
                        ui.selectable_value(
                            &mut self.groups[idx].resolution_policy, p, p.label());
                    }
                });
        });
        
        // Output template (per-group override).
        ui.horizontal(|ui| {
            ui.label("Output template:");
            let hint = self.global_template.clone();
            let resp = ui.text_edit_singleline(&mut self.groups[idx].output_template);
            resp.on_hover_text(format!(
                "Tokens: {{groupname}} {{sourcename}} {{codec}} {{crf}} \
                 {{date}} {{time}} {{localdir}} {{savings}}. \
                 Empty = use global template: {}", hint
            ));
        });
        ui.label(RichText::new(
            "Tokens: {groupname} {sourcename} {codec} {crf} {date} {time} \
             {localdir} {savings}. Leave empty to use the global template."
        ).weak().size(10.0));
        
        ui.add_space(6.0);
        
        // Run controls.
        ui.horizontal(|ui| {
            let can_run = !running && self.groups[idx].active_count() > 0;
            if ui.add_enabled(can_run, egui::Button::new("▶ Run this group")).clicked() {
                self.start_active_group();
            }
            if ui.add_enabled(running, egui::Button::new("Cancel")).clicked() {
                self.cancel_active_group();
            }
        });
        
        // Running banner.
        if let Some(p) = &self.pending_group {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let gname = self.groups.get(p.group_index)
                        .map(|g| g.name.clone()).unwrap_or_default();
                    ui.label(format!("Running '{}' — clip {}/{} ({:.0}s)",
                        gname, p.progress.0 + 1, p.progress.1,
                        p.started_at.elapsed().as_secs_f64()));
                });
                ui.label(RichText::new(&p.last_message).weak().size(11.0));
            });
        }
        
        // Last status.
        if !self.group_status.is_empty() && self.pending_group.is_none() {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Last run: {}", self.group_status)).weak());
        }
        
        // Global template editor (applies to groups without an override).
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Global output template:");
            ui.text_edit_singleline(&mut self.global_template);
        });
    }
    
    fn render_main(&mut self, ui: &mut egui::Ui) {
        // Mode switcher: single-file vs groups (merge tabs).
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ui_mode, UiMode::SingleFile, "Single file");
            ui.selectable_value(&mut self.ui_mode, UiMode::Groups, "Groups (merge)");
        });
        ui.separator();
        
        if self.ui_mode == UiMode::Groups {
            self.render_groups(ui);
            return;
        }
        
        // Input/output paths with file pickers.
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Input").strong());
                if ui.button("Browse…").clicked() {
                    self.pick_input_file();
                }
            });
            ui.text_edit_singleline(&mut self.input_path);
            
            ui.add_space(6.0);
            
            ui.horizontal(|ui| {
                ui.label(RichText::new("Output").strong());
                if ui.button("Browse…").clicked() {
                    self.pick_output_file();
                }
                if ui.button("Derive from input").clicked() {
                    self.output_path = self.derived_output();
                    self.check_for_resume();
                }
            });
            ui.text_edit_singleline(&mut self.output_path);
        });
        
        // Resume offer banner.
        if self.resume_offered && self.scheduler.is_none() {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(230, 180, 30), "↺");
                    let done = self.chunks.iter().filter(|c| c.status == ChunkStatus::Done).count();
                    ui.label(format!(
                        "Found existing job: {} of {} chunks already encoded. \
                         Click Start to resume.",
                        done, self.chunks.len()
                    ));
                    if ui.button("Discard previous").clicked() {
                        if let Some(job) = &self.job {
                            let _ = std::fs::remove_dir_all(&job.state_dir);
                        }
                        self.resume_offered = false;
                        self.chunks.clear();
                        self.job = None;
                        self.append_log("previous job state discarded".to_string());
                    }
                });
            });
        }
        
        ui.add_space(8.0);
        
        // Encoder settings — controls swap based on selected codec.
        ui.group(|ui| {
            ui.label(RichText::new("Encoder settings").strong());
            
            // Codec picker. Same width for both options.
            egui::ComboBox::from_label("Codec")
                .selected_text(self.settings.kind.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.settings.kind, EncoderKind::SvtAv1,
                        EncoderKind::SvtAv1.display_name());
                    ui.selectable_value(
                        &mut self.settings.kind, EncoderKind::Vvenc,
                        EncoderKind::Vvenc.display_name());
                });
            
            // VVenC-selected info + install controls.
            if self.settings.kind == EncoderKind::Vvenc {
                ui.colored_label(
                    Color32::from_rgb(230, 180, 30),
                    "⚠ VVC/H.266 has patent royalties for commercial use. \
                     Personal experimentation only. Browsers + most apps \
                     can't play H.266 output (use mpv or VLC).",
                );
                self.render_vvenc_install_controls(ui);
            }
            
            // Codec-specific controls.
            match self.settings.kind {
                EncoderKind::SvtAv1 => {
                    ui.add(egui::Slider::new(&mut self.settings.svt_crf, 0..=63)
                        .text("CRF"))
                        .on_hover_text("Lower = better quality, bigger file. Typical 22-35.");
                    ui.add(egui::Slider::new(&mut self.settings.preset, 0..=13)
                        .text("Preset"))
                        .on_hover_text("Lower = slower + better quality. Typical 4-8.");
                    ui.horizontal(|ui| {
                        ui.label("Tune");
                        ui.radio_value(&mut self.settings.tune, 0, "Visual")
                            .on_hover_text("Optimize for perceptual quality.");
                        ui.radio_value(&mut self.settings.tune, 1, "PSNR")
                            .on_hover_text("Optimize for objective PSNR.");
                    });
                }
                EncoderKind::Vvenc => {
                    ui.add(egui::Slider::new(&mut self.settings.vvenc_qp, 0..=63)
                        .text("QP"))
                        .on_hover_text("Quantizer 0-63. Lower = better quality. Typical 28-40 for VVC.");
                    
                    egui::ComboBox::from_label("Preset")
                        .selected_text(self.settings.vvenc_preset.display_name())
                        .show_ui(ui, |ui| {
                            for p in VvencPreset::all() {
                                ui.selectable_value(
                                    &mut self.settings.vvenc_preset,
                                    *p,
                                    p.display_name(),
                                );
                            }
                        });
                    
                    ui.checkbox(&mut self.settings.vvenc_qpa,
                        "Perceptual quality (QPA)")
                        .on_hover_text(
                            "Quality-perceptual-adaptive rate control. \
                             Modest encode-time cost, noticeably better \
                             perceptual quality. Recommended."
                        );
                }
            }
            
            // Shared across codecs.
            ui.horizontal(|ui| {
                ui.label("Pixel format");
                ui.text_edit_singleline(&mut self.settings.pixel_format);
            });
            
            ui.add(egui::Slider::new(&mut self.chunk_seconds, 2.0..=30.0)
                .text("Chunk size (seconds)"))
                .on_hover_text("Shorter = faster recovery from crashes. Default 10s.");
            
            let max_concurrency = (self.default_concurrency * 2).max(8);
            ui.add(egui::Slider::new(&mut self.concurrency, 1..=max_concurrency)
                .text("Parallel workers"))
                .on_hover_text(format!(
                    "Number of chunks encoding at once. \n\
                     Default ({}) = half your CPU cores, leaving room \
                     for OS + UI. Higher = faster but more thermal load.",
                    self.default_concurrency
                ));
            
            ui.separator();
            
            // Cache + cleanup toggles.
            ui.checkbox(&mut self.use_cache, "Skip unchanged re-encodes (cache)")
                .on_hover_text(
                    "Before encoding, check whether this exact source + \
                     settings was already encoded and the output is still \
                     intact. If so, reuse it instead of re-encoding. The \
                     check verifies the source hasn't changed (size + \
                     timestamp + sampled content) and the output isn't \
                     corrupted."
                );
            
            ui.checkbox(&mut self.auto_cleanup, "Auto-clean intermediates after success")
                .on_hover_text(
                    "Remove per-chunk temporary files after a successful \
                     encode to keep the workspace tidy. Turn OFF for large \
                     batches: keeping intermediates lets a crash resume in \
                     seconds instead of re-encoding completed chunks. The \
                     cache is unaffected either way."
                );
        });
        
        ui.add_space(8.0);
        
        // Audio settings.
        self.render_audio_settings(ui);
        
        ui.add_space(8.0);
        
        // Action buttons.
        let running = self.scheduler.is_some();
        let probing = self.pending_probe.is_some();
        let comparing = self.pending_compare.is_some();
        let can_act = !self.input_path.is_empty() 
            && !self.output_path.is_empty()
            && !running && !probing && !comparing;
        
        ui.horizontal(|ui| {
            if ui.add_enabled(can_act, egui::Button::new("Preflight")).clicked() {
                self.run_preflight();
            }
            let preflight_ok = self.preflight.as_ref()
                .map(|r| !r.fatal()).unwrap_or(false);
            let start_label = if probing {
                "Probing..."
            } else if self.resume_offered {
                "Resume"
            } else {
                "Start"
            };
            if ui.add_enabled(preflight_ok && can_act, 
                egui::Button::new(start_label)).clicked() 
            {
                self.start_job();
            }
            if ui.add_enabled(running, egui::Button::new("Cancel")).clicked() {
                self.cancel_job();
            }
        });
        
        // Compare codecs row — separate from Preflight/Start since it's 
        // a different workflow (no commit, just preview).
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Compare codecs:").strong());
            
            // Probe button — needed if we don't have cached info yet.
            if self.cached_info.is_none() {
                let probe_label = if probing { "Probing..." } else { "Probe source" };
                if ui.add_enabled(can_act, egui::Button::new(probe_label)).clicked() {
                    self.start_probe_only();
                }
            }
            
            // Compare button — enabled when we have cached info and 
            // aren't already running something.
            let can_compare = self.cached_info.is_some() && can_act;
            let compare_label = if comparing { "Encoding samples..." } else { "Encode samples" };
            if ui.add_enabled(can_compare, egui::Button::new(compare_label)).clicked() {
                self.start_compare();
            }
            
            if ui.add_enabled(comparing, egui::Button::new("Stop compare")).clicked() {
                self.cancel_compare();
            }
            
            // Sample length slider — kept compact since it's a sub-control.
            ui.add(egui::Slider::new(&mut self.compare_sample_seconds, 2.0..=30.0)
                .text("sample length (s)")
                .clamp_to_range(true));
        });
        
        // Prediction analysis row (Phase 0 measurement).
        let predicting = self.pending_prediction.is_some();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Prediction analysis:").strong());
            let can_predict = self.cached_info.is_some() 
                && self.settings.kind == EncoderKind::SvtAv1
                && can_act;
            let predict_label = if predicting { 
                "Analyzing..." 
            } else { 
                "Run analysis" 
            };
            if ui.add_enabled(can_predict, egui::Button::new(predict_label))
                .on_hover_text(
                    "Encodes a sample with SVT-AV1's stat reporter on, then                      measures inter-frame vs keyframe size ratio. Surfaces                      a verdict (Green/Yellow/Red) about whether a learned                      predictor would have headroom over AV1 on this content.                      Phase 0 measurement only — does not train a real predictor."
                )
                .clicked() 
            {
                self.start_prediction();
            }
            if ui.add_enabled(predicting, egui::Button::new("Stop analysis")).clicked() {
                self.cancel_prediction();
            }
            ui.add(egui::Slider::new(&mut self.prediction_sample_seconds, 5.0..=60.0)
                .text("sample length (s)")
                .clamp_to_range(true));
        });
        
        // Probing banner.
        if let Some(pending) = &self.pending_probe {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "Probing {} (running for {:.1}s)...",
                        pending.input.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| pending.input.display().to_string()),
                        pending.started_at.elapsed().as_secs_f64()
                    ));
                });
                ui.label(RichText::new(
                    "ffprobe reads container metadata first (instant). For \
                     videos without metadata it counts frames exactly, which \
                     takes a few seconds per minute of source."
                ).weak().size(11.0));
            });
        }
        
        // Comparing banner.
        if let Some(pending) = &self.pending_compare {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "Encoding samples with both codecs ({:.1}s elapsed)...",
                        pending.started_at.elapsed().as_secs_f64()
                    ));
                });
                ui.label(RichText::new(
                    "Encoding the same source slice with SVT-AV1 and VVenC \
                     at your current per-codec settings. This runs each \
                     encoder sequentially so total time is roughly the sum \
                     of both."
                ).weak().size(11.0));
            });
        }
        
        // Comparison result panel.
        if let Some(result) = &self.compare_result {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.label(RichText::new("Codec comparison").strong());
                ui.label(format!(
                    "Sample: {} frames ({:.1}s of source)",
                    result.sample_frames, result.sample_seconds
                ));
                ui.add_space(4.0);
                
                let total_frames = self.cached_info.as_ref()
                    .map(|i| i.frame_count).unwrap_or(0);
                
                // Header row.
                egui::Grid::new("compare_grid")
                    .striped(true)
                    .num_columns(4)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Codec").strong());
                        ui.label(RichText::new("Sample size").strong());
                        ui.label(RichText::new("Encode time").strong());
                        ui.label(RichText::new("Projected total").strong());
                        ui.end_row();
                        
                        // Establish a baseline for ratio display. Use 
                        // the AV1 result if present, else the first.
                        let baseline = result.samples.iter()
                            .find(|s| s.kind == EncoderKind::SvtAv1)
                            .or_else(|| result.samples.first())
                            .map(|s| s.bytes);
                        
                        for s in &result.samples {
                            ui.label(s.kind.display_name());
                            let ratio_text = match baseline {
                                Some(b) if b > 0 && s.kind != EncoderKind::SvtAv1 => {
                                    let pct = (s.bytes as f64 / b as f64 - 1.0) * 100.0;
                                    let sign = if pct < 0.0 { "" } else { "+" };
                                    format!(" ({}{:.1}%)", sign, pct)
                                }
                                _ => String::new(),
                            };
                            ui.label(format!(
                                "{}{}", format_bytes(s.bytes), ratio_text
                            ));
                            let fps = if s.elapsed_ms > 0 {
                                result.sample_frames as f64 * 1000.0 / s.elapsed_ms as f64
                            } else { 0.0 };
                            ui.label(format!(
                                "{:.1}s ({:.1} fps)",
                                s.elapsed_ms as f64 / 1000.0,
                                fps
                            ));
                            let projected = if total_frames > 0 {
                                format_bytes(result.project_total_bytes(s, total_frames))
                            } else {
                                "—".to_string()
                            };
                            ui.label(projected);
                            ui.end_row();
                        }
                    });
                
                ui.add_space(4.0);
                ui.label(RichText::new(
                    "Projection is linear scaling from sample to full length \
                     — real encodes vary with content complexity."
                ).weak().size(10.0));
                
                // Surface the most useful one-line takeaway.
                let av1 = result.samples.iter().find(|s| s.kind == EncoderKind::SvtAv1);
                let vvc = result.samples.iter().find(|s| s.kind == EncoderKind::Vvenc);
                if let (Some(a), Some(v)) = (av1, vvc) {
                    let pct = (v.bytes as f64 / a.bytes as f64 - 1.0) * 100.0;
                    let summary = if pct < -5.0 {
                        format!("VVC is {:.1}% smaller than AV1 on this content.", -pct)
                    } else if pct > 5.0 {
                        format!("VVC is {:.1}% larger than AV1 on this content.", pct)
                    } else {
                        format!("VVC and AV1 are within {:.1}% of each other.", pct.abs())
                    };
                    let speed_pct = if a.elapsed_ms > 0 {
                        (v.elapsed_ms as f64 / a.elapsed_ms as f64 - 1.0) * 100.0
                    } else { 0.0 };
                    let speed_note = if speed_pct > 10.0 {
                        format!(" VVC took {:.0}% longer to encode.", speed_pct)
                    } else if speed_pct < -10.0 {
                        format!(" VVC was {:.0}% faster to encode.", -speed_pct)
                    } else {
                        " Encode times were comparable.".to_string()
                    };
                    ui.add_space(2.0);
                    ui.label(RichText::new(format!("{}{}", summary, speed_note))
                        .strong());
                }
            });
        }
        
        // Prediction analysis: running banner.
        if let Some(pending) = &self.pending_prediction {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "Encoding sample with stat reporter ({:.1}s elapsed)...",
                        pending.started_at.elapsed().as_secs_f64()
                    ));
                });
                ui.label(RichText::new(
                    "Measuring how much inter-frame structure AV1's prediction \
                     captures on your content. Result will indicate whether a \
                     learned predictor has theoretical headroom over AV1."
                ).weak().size(11.0));
            });
        }
        
        // Prediction analysis: result panel.
        if let Some(stats) = &self.prediction_result {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.label(RichText::new("Prediction analysis").strong());
                
                let verdict = av1tuner_core::prediction_stats::verdict_for(stats);
                let (badge_color, badge_text) = match verdict {
                    av1tuner_core::Verdict::Green => 
                        (Color32::from_rgb(60, 180, 75), "GREEN"),
                    av1tuner_core::Verdict::Yellow => 
                        (Color32::from_rgb(230, 180, 30), "YELLOW"),
                    av1tuner_core::Verdict::Red => 
                        (Color32::from_rgb(220, 80, 80), "RED"),
                };
                
                ui.horizontal(|ui| {
                    ui.label(RichText::new(badge_text)
                        .strong().color(badge_color).size(18.0));
                    ui.label(RichText::new(verdict.label()).strong());
                });
                
                ui.add_space(2.0);
                egui::Grid::new("prediction_grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Sample frames:");
                        ui.label(format!("{}", stats.total_frames));
                        ui.end_row();
                        ui.label("Keyframes / inter:");
                        ui.label(format!("{} / {}", 
                            stats.keyframe_count, stats.inter_count));
                        ui.end_row();
                        ui.label("Avg keyframe size:");
                        ui.label(format_bytes(stats.avg_keyframe_bytes as u64));
                        ui.end_row();
                        ui.label("Avg inter-frame size:");
                        ui.label(format_bytes(stats.avg_inter_bytes as u64));
                        ui.end_row();
                        ui.label("P/I size ratio:");
                        ui.label(format!("{:.3} ({:.1}%)", 
                            stats.p_to_i_ratio, stats.p_to_i_ratio * 100.0));
                        ui.end_row();
                        ui.label("Inter-frame size CV:");
                        ui.label(format!("{:.3}", stats.inter_size_cv));
                        ui.end_row();
                    });
                
                ui.add_space(4.0);
                ui.label(RichText::new(verdict.explanation(stats))
                    .weak().size(11.0));
                
                if !stats.stat_report_parsed {
                    ui.add_space(2.0);
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 30),
                        "Note: SVT-AV1's stat output couldn't be parsed; \
                         numbers are estimated from total bytes. Verdict is \
                         less reliable."
                    );
                }
            });
        }
        
        // Preflight results.
        if let Some(report) = &self.preflight {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.label(RichText::new("Preflight").strong());
                for c in &report.checks {
                    let (icon, color) = match c.status {
                        CheckStatus::Pass => ("✓", Color32::from_rgb(60, 180, 75)),
                        CheckStatus::Warn => ("!", Color32::from_rgb(230, 180, 30)),
                        CheckStatus::Fail => ("✗", Color32::from_rgb(220, 60, 60)),
                    };
                    ui.horizontal(|ui| {
                        ui.colored_label(color, icon);
                        ui.label(RichText::new(&c.name).strong());
                        ui.label("—");
                        ui.label(&c.message);
                    });
                }
            });
        }
        
        // Progress.
        if !self.chunks.is_empty() {
            let p = self.progress();
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.label(RichText::new("Progress").strong());
                ui.add(ProgressBar::new(p.fraction())
                    .text(format!("{}/{} chunks", p.done, p.total))
                    .desired_width(ui.available_width()));
                ui.label(format!(
                    "done: {}   encoding: {}   pending: {}   failed/skipped: {}",
                    p.done, p.encoding, p.pending(), p.failed
                ));
                
                // Throughput + ETA, only while actively running.
                if self.scheduler.is_some() {
                    let fps = self.eta.frames_per_second();
                    let remaining = self.remaining_frames();
                    let eta_str = self.eta.eta_string(remaining);
                    let elapsed = self.job_started
                        .map(|t| format_duration(t.elapsed().as_secs()))
                        .unwrap_or_else(|| "—".to_string());
                    ui.label(format!(
                        "elapsed: {}   throughput: {:.1} fps   ETA: {}",
                        elapsed, fps, eta_str
                    ));
                    
                    // Encoded-so-far + projected total.
                    let encoded_bytes: u64 = self.chunks.iter()
                        .filter_map(|c| c.output_bytes)
                        .sum();
                    let done_frames: u64 = self.chunks.iter()
                        .filter(|c| c.status == ChunkStatus::Done)
                        .map(|c| c.frame_count())
                        .sum();
                    let total_frames: u64 = self.chunks.iter()
                        .map(|c| c.frame_count())
                        .sum();
                    let projected = if done_frames > 0 {
                        (encoded_bytes as f64 / done_frames as f64
                            * total_frames as f64) as u64
                    } else { 0 };
                    ui.label(format!(
                        "encoded so far: {}   projected total: {}",
                        format_bytes(encoded_bytes),
                        if projected > 0 { format_bytes(projected) } 
                        else { "—".to_string() }
                    ));
                }
                
                chunk_strip(ui, &self.chunks);
            });
        }
        
        // Completion summary.
        if let (Some(path), Some(out_bytes)) = (&self.final_output, self.final_output_bytes) {
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("✓ Encode complete")
                        .strong()
                        .color(Color32::from_rgb(60, 180, 75))
                        .size(15.0));
                    if ui.button("Show in file manager").clicked() {
                        reveal_in_file_manager(path);
                    }
                });
                ui.add_space(4.0);
                ui.label(format!("Output: {}", path.display()));
                ui.label(format!("Size: {}", format_bytes(out_bytes)));
                if let Some(src) = self.source_bytes {
                    let ratio = out_bytes as f64 / src as f64;
                    let pct = (1.0 - ratio) * 100.0;
                    let color = if pct > 0.0 {
                        Color32::from_rgb(60, 180, 75)
                    } else {
                        Color32::from_rgb(230, 180, 30)
                    };
                    ui.horizontal(|ui| {
                        ui.label(format!("Source: {}", format_bytes(src)));
                        ui.colored_label(color, format!("  ({:+.1}%)", -pct));
                    });
                }
                if let Some(dur) = self.final_duration_s {
                    ui.label(format!("Total time: {}", format_duration(dur)));
                }
                
                // Per-chunk skip warning.
                let skipped = self.chunks.iter()
                    .filter(|c| c.status == ChunkStatus::Skipped).count();
                if skipped > 0 {
                    ui.add_space(4.0);
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 30),
                        format!(
                            "⚠ {} chunks were skipped due to encoder failures. \
                             Output has gaps. Check the log for details.",
                            skipped
                        ),
                    );
                }
            });
        }
    }
}

impl App {
    /// Show the install state + Install/Cancel button for VVenC. 
    /// Called from the encoder settings panel when VVenC is selected.
    fn render_vvenc_install_controls(&mut self, ui: &mut egui::Ui) {
        use av1tuner_core::vvenc_install::InstallPaths;
        
        let on_path = which::which("vvencapp").is_ok();
        let installed_locally = InstallPaths::default_paths()
            .map(|p| p.is_installed()).unwrap_or(false);
        let is_running = self.pending_install.is_some();
        
        ui.add_space(4.0);
        
        // Status line.
        if on_path {
            ui.colored_label(Color32::from_rgb(60, 180, 75),
                "✓ vvencapp on PATH — ready to encode");
        } else if installed_locally {
            ui.colored_label(Color32::from_rgb(60, 180, 75),
                "✓ vvencapp installed locally (av1tuner-managed) — ready to encode");
        } else if is_running {
            // Suppress "not installed" message while install is running.
        } else {
            ui.label(RichText::new(
                "vvencapp is not on PATH and no av1tuner-managed install exists."
            ).weak());
            match &self.install_status {
                InstallStatus::Failed(msg) => {
                    ui.colored_label(Color32::from_rgb(220, 60, 60),
                        format!("Last install attempt failed: {}", msg));
                }
                InstallStatus::Cancelled => {
                    ui.colored_label(Color32::from_rgb(230, 180, 30),
                        "Last install attempt was cancelled");
                }
                _ => {}
            }
        }
        
        // Buttons.
        ui.horizontal(|ui| {
            let label = if is_running { 
                "Installing..." 
            } else if installed_locally || on_path { 
                "Reinstall VVenC" 
            } else { 
                "Install VVenC from source" 
            };
            if ui.add_enabled(!is_running, egui::Button::new(label))
                .on_hover_text(
                    "Clones github.com/fraunhoferhhi/vvenc, runs cmake + make, \
                     installs the vvencapp binary to ~/.local/share/av1tuner/bin. \
                     Requires git, cmake, make, and a C++ compiler (gcc-c++ or clang). \
                     Takes ~5-15 minutes."
                )
                .clicked() 
            {
                self.start_install_vvenc();
            }
            if ui.add_enabled(is_running, egui::Button::new("Cancel install")).clicked() {
                self.cancel_install();
            }
        });
        
        // Progress banner while running.
        if let Some(p) = &self.pending_install {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "{} ({:.0}s elapsed)",
                    p.current_step,
                    p.started_at.elapsed().as_secs_f64()
                ));
            });
        }
    }
    
    fn render_audio_settings(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(RichText::new("Audio").strong());
            
            // Mode picker: three radio-style buttons.
            ui.horizontal(|ui| {
                let is_pass = matches!(self.audio.mode, AudioMode::Passthrough);
                let is_opus = matches!(self.audio.mode, AudioMode::EncodeOpus(_));
                let is_drop = matches!(self.audio.mode, AudioMode::Drop);
                
                if ui.selectable_label(is_pass, "Passthrough")
                    .on_hover_text("Copy audio + subtitles unchanged. Fastest, no quality loss, largest file.")
                    .clicked() 
                {
                    self.audio.mode = AudioMode::Passthrough;
                }
                if ui.selectable_label(is_opus, "Encode → Opus")
                    .on_hover_text("Re-encode first audio track to Opus. Smaller file, modest quality cost.")
                    .clicked() 
                {
                    if !is_opus {
                        self.audio.mode = AudioMode::EncodeOpus(OpusSettings::default());
                    }
                }
                if ui.selectable_label(is_drop, "Drop")
                    .on_hover_text("Strip audio entirely. Video-only output. Subtitles still pass through.")
                    .clicked() 
                {
                    self.audio.mode = AudioMode::Drop;
                }
            });
            
            // Mode-specific controls.
            if let AudioMode::EncodeOpus(opus) = &mut self.audio.mode {
                ui.add_space(4.0);
                ui.add(egui::Slider::new(&mut opus.bitrate_kbps, 32..=320)
                    .text("Opus bitrate (kbps)"))
                    .on_hover_text(
                        "Opus is excellent at low bitrates:\n\
                         • 32-64: voice/podcast\n\
                         • 96-128: transparent for music\n\
                         • 160-256: archive quality"
                    );
                
                egui::ComboBox::from_label("Application")
                    .selected_text(format!("{:?}", opus.application))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut opus.application, OpusApplication::Audio, "Audio (music/mixed)");
                        ui.selectable_value(&mut opus.application, OpusApplication::Voip, "VoIP (speech)");
                        ui.selectable_value(&mut opus.application, OpusApplication::LowDelay, "Low Delay");
                    });
                
                ui.horizontal(|ui| {
                    ui.checkbox(&mut opus.vbr, "VBR")
                        .on_hover_text("Variable bitrate. On by default — better quality per byte.");
                    ui.add(egui::Slider::new(&mut opus.compression_level, 0..=10)
                        .text("Compression"))
                        .on_hover_text("Higher = slower encode, better quality. Opus is fast even at 10.");
                });
            }
        });
    }
}

/// Visualize chunks as a colored strip.
fn chunk_strip(ui: &mut egui::Ui, chunks: &[ChunkState]) {
    if chunks.is_empty() { return; }
    let available = ui.available_width();
    let chunk_width = (available / chunks.len() as f32).max(2.0);
    let height = 20.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(available, height),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    for (i, c) in chunks.iter().enumerate() {
        let x0 = rect.left() + i as f32 * chunk_width;
        let x1 = (x0 + chunk_width - 1.0).min(rect.right());
        let color = match c.status {
            ChunkStatus::Pending => Color32::from_gray(80),
            ChunkStatus::Encoding => Color32::from_rgb(230, 180, 30),
            ChunkStatus::Done => Color32::from_rgb(60, 180, 75),
            ChunkStatus::Failed => Color32::from_rgb(180, 80, 30),
            ChunkStatus::Skipped => Color32::from_rgb(220, 60, 60),
        };
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, rect.top()),
                egui::pos2(x1, rect.bottom()),
            ),
            0.0,
            color,
        );
    }
    
    // Tooltip on hover showing the chunk under the cursor.
    // Only formats the hovered chunk's data — we don't allocate per 
    // chunk per frame even with thousands of chunks.
    if let Some(pos) = response.hover_pos() {
        let rel_x = pos.x - rect.left();
        let idx = (rel_x / chunk_width).floor() as usize;
        if idx < chunks.len() {
            let c = &chunks[idx];
            let mut parts = vec![
                format!("chunk {}", c.index),
                format!("frames {}..{} ({} total)", 
                    c.start_frame, c.end_frame, c.frame_count()),
                format!("status: {:?}", c.status),
                format!("attempts: {}", c.attempts),
            ];
            if let Some(ms) = c.encode_ms {
                let fps = if ms > 0 { 
                    c.frame_count() as f64 * 1000.0 / ms as f64 
                } else { 0.0 };
                parts.push(format!("encode time: {} ms ({:.1} fps)", ms, fps));
            }
            if let Some(b) = c.output_bytes {
                parts.push(format!("output: {} bytes ({:.1} KB)", b, b as f64 / 1024.0));
            }
            if let Some(e) = &c.last_error {
                parts.push(format!("error: {}", e));
            }
            response.on_hover_text(parts.join("\n"));
        }
    }
}

/// Open the system file manager at the given file's parent directory.
/// On Linux, prefers xdg-open. On macOS, `open -R` (reveals the file). 
/// On Windows, `explorer /select,`. Best-effort; errors are logged.
fn reveal_in_file_manager(path: &std::path::Path) {
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R").arg(path).spawn();
    
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display())).spawn();
    
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open")
        .arg(dir).spawn();
    
    if let Err(e) = result {
        log::warn!("couldn't reveal {} in file manager: {}", path.display(), e);
    }
}

/// Filter for which install-stage output lines to surface in the log.
/// Build output is mostly cmake compile lines that fill the log with 
/// noise. Surface progress markers + warnings + errors.
fn is_useful_output(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // Always show our own $-prefixed command lines and CMake summaries.
    if line.starts_with("$ ") { return true; }
    if line.starts_with("--") { return true; }  // CMake's "-- Found foo" lines
    // Show percentage progress from make.
    if line.contains('%') && line.contains('[') { return true; }
    // Show warnings + errors regardless of source.
    if lower.contains("error") || lower.contains("warning") { return true; }
    // Show the final "Built target" lines.
    if line.contains("Built target") { return true; }
    false
}

fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if b >= GB { format!("{:.2} GB", b as f64 / GB as f64) }
    else if b >= MB { format!("{:.2} MB", b as f64 / MB as f64) }
    else if b >= KB { format!("{:.1} KB", b as f64 / KB as f64) }
    else { format!("{} B", b) }
}
