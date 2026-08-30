//! studio_host — the process half of `studio` (CorpusStudio).
//!
//! `studio` owns the SEQUENCE: which stage is next, what a failure means, what
//! the user may press. This module owns everything the sequence asks for:
//! building each stage's argv, spawning it, streaming its output, validating
//! its artifact, and persisting the sticky settings. All of it is ported
//! behavior-for-behavior from the GTK `src/bin/corpus_studio.rs`, and the
//! places where that code made a judgement call keep the call:
//!
//! * **Exit 0 with an empty artifact is a failure** — a corpus whose extension
//!   matches zero files makes stage 1 emit an empty tokenizer that only blows
//!   up at stage 3, so the artifact is checked HERE and the failure surfaces at
//!   the stage that caused it.
//! * **The pre-flight extension check** — if a corpus-reading stage is queued
//!   and `CORPUS_EXT` matches nothing, fail before spawning anything, with the
//!   extensions that ARE present in the message.
//! * **Stop is honored between stages** (and mid-stderr-stream, where the GTK
//!   worker also kills the child); a running stage otherwise finishes.
//! * **Settings share the GTK file.** `glib::user_config_dir()` is the XDG
//!   config dir, so reading `$XDG_CONFIG_HOME/corpus-studio/settings.conf`
//!   (fallback `~/.config`) means the egui and GTK studios see one another's
//!   sticky settings — same keys, same key=value format.
//!
//! The worker communicates over `std::sync::mpsc` (the shell drains with
//! `try_recv` each frame) with the exact message shapes the GTK worker used
//! over async-channel, so the streaming behavior — stdout on a helper thread,
//! stderr on the worker — carries over unchanged.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use crate::studio::{StageState, Studio, StudioAction, STAGE_NAMES};

/// Everything a run needs. Field-for-field the GTK `StudioConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct StudioConfig {
    pub corpus: PathBuf,
    pub preprocess: bool,
    /// Use the bundled Rust corpus_md converter instead of an external tool.
    pub builtin_pp: bool,
    pub tool: Option<PathBuf>,
    /// Template with `{in}` and `{out}`.
    pub tool_args: String,
    pub bin_dir: PathBuf,
    pub out_dir: PathBuf,
    pub corpus_ext: String,
    pub merges: u32,
    pub max_n: u32,
    pub w2v_dim: u32,
    pub w2v_epochs: u32,
    pub gpt_steps: u32,
    pub gpt_dim: u32,
    pub gpt_layers: u32,
    pub niches: u32,
}

impl Default for StudioConfig {
    /// The GTK setup page's initial values: registry-backed bin/out paths and
    /// the slider defaults, so a fresh egui studio and a fresh GTK studio
    /// propose the identical run.
    fn default() -> Self {
        StudioConfig {
            corpus: PathBuf::new(),
            preprocess: false,
            builtin_pp: true,
            tool: None,
            tool_args: "{in} {out}/corpus.md -j 4".into(),
            bin_dir: bot_arena::registry::bin_dir(),
            out_dir: bot_arena::registry::data_root().join("out"),
            corpus_ext: String::new(),
            merges: 8000,
            max_n: 4,
            w2v_dim: 64,
            w2v_epochs: 8,
            gpt_steps: 3000,
            gpt_dim: 128,
            gpt_layers: 2,
            niches: 6,
        }
    }
}

/// Worker → shell messages. The GTK `Msg`, verbatim.
#[derive(Debug, Clone, PartialEq)]
pub enum StudioMsg {
    Log(String),
    StepStart(usize),
    StepLog(usize, String),
    StepDone(usize, bool),
    Finished,
}

// ── paths & settings ─────────────────────────────────────────────────────────

/// One staging dir per process: temp/corpus-studio/staged-<pid>. Cleared on
/// each "Stage to temp" and removable on shutdown, so /tmp never accumulates.
pub fn staging_dir() -> PathBuf {
    std::env::temp_dir()
        .join("corpus-studio")
        .join(format!("staged-{}", std::process::id()))
}

/// The XDG user-config dir — what `glib::user_config_dir()` resolves to, so
/// the settings file is SHARED with the GTK studio.
fn user_config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config")
}

pub fn settings_path() -> PathBuf {
    user_config_dir().join("corpus-studio").join("settings.conf")
}

/// Read the settings map, or an empty one.
///
/// The missing error here is DELIBERATE, unlike the one that used to be
/// in `save_settings_to`. A settings file that does not exist is the
/// normal first run, and every key has a default — so "could not read"
/// and "nothing configured yet" want the same behaviour, and returning a
/// Result would force every caller to write the same `unwrap_or_default`.
///
/// Saving is the opposite case: nothing downstream can recover from a
/// failed write, and the person needs to know their choices were not
/// kept.
pub fn load_settings_from(path: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                m.insert(k.trim().to_string(), v.to_string());
            }
        }
    }
    m
}

/// Persist the settings map.
///
/// RETURNS THE ERROR rather than discarding it. This was
/// `let _ = std::fs::write(...)` inside a `()` function, so a failed
/// save — a read-only home, a full disk, a permissions change — was
/// indistinguishable from a successful one at every call site. The next
/// run silently used the old settings, which presents as "it forgot what
/// I chose" rather than as a write error.
pub fn save_settings_to(path: &Path, m: &HashMap<String, String>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        // Still discarded, and correctly: a create_dir_all failure will
        // surface as the write error a few lines down, with a message
        // naming the actual problem. Reporting it twice would be noise.
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = String::new();
    for (k, v) in m {
        out.push_str(k);
        out.push('=');
        out.push_str(v);
        out.push('\n');
    }
    std::fs::write(path, out)
}

impl StudioConfig {
    /// Read the sticky settings (the same file, keys and formats the GTK studio
    /// writes) over the defaults. Slider values are stored as f64 strings by
    /// GTK (`Scale::value()`), so they parse through f64.
    pub fn load_sticky(mut self, m: &HashMap<String, String>) -> Self {
        let path = |k: &str| m.get(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(p) = path("corpus") {
            self.corpus = p;
        }
        if let Some(p) = path("tool") {
            self.tool = Some(p);
        }
        if let Some(p) = path("bin") {
            self.bin_dir = p;
        }
        if let Some(p) = path("out") {
            self.out_dir = p;
        }
        if let Some(v) = m.get("ext") {
            self.corpus_ext = v.clone();
        }
        if let Some(v) = m.get("args") {
            self.tool_args = v.clone();
        }
        if let Some(v) = m.get("preprocess") {
            self.preprocess = v == "true";
        }
        let num = |k: &str, cur: u32| -> u32 {
            m.get(k)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|f| f as u32)
                .unwrap_or(cur)
        };
        self.merges = num("merges", self.merges);
        self.max_n = num("max_n", self.max_n);
        self.w2v_dim = num("w2v_dim", self.w2v_dim);
        self.w2v_epochs = num("w2v_epochs", self.w2v_epochs);
        self.niches = num("niches", self.niches);
        self.gpt_steps = num("gpt_steps", self.gpt_steps);
        self.gpt_dim = num("gpt_dim", self.gpt_dim);
        self.gpt_layers = num("gpt_layers", self.gpt_layers);
        self
    }

    /// The map `save_settings_to` persists — the GTK key set, verbatim.
    pub fn sticky_map(&self) -> HashMap<String, String> {
        let ps = |p: &PathBuf| p.display().to_string();
        let mut m = HashMap::new();
        m.insert("corpus".into(), ps(&self.corpus));
        m.insert(
            "tool".into(),
            self.tool.as_ref().map(ps).unwrap_or_default(),
        );
        m.insert("bin".into(), ps(&self.bin_dir));
        m.insert("out".into(), ps(&self.out_dir));
        m.insert("ext".into(), self.corpus_ext.clone());
        m.insert("args".into(), self.tool_args.clone());
        m.insert(
            "preprocess".into(),
            if self.preprocess { "true" } else { "false" }.into(),
        );
        m.insert("merges".into(), self.merges.to_string());
        m.insert("max_n".into(), self.max_n.to_string());
        m.insert("w2v_dim".into(), self.w2v_dim.to_string());
        m.insert("w2v_epochs".into(), self.w2v_epochs.to_string());
        m.insert("niches".into(), self.niches.to_string());
        m.insert("gpt_steps".into(), self.gpt_steps.to_string());
        m.insert("gpt_dim".into(), self.gpt_dim.to_string());
        m.insert("gpt_layers".into(), self.gpt_layers.to_string());
        m
    }
}

// ── corpus inspection ────────────────────────────────────────────────────────

/// The bundled Rust converter sits next to this binary.
pub fn builtin_converter() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("corpus_md")))
        .unwrap_or_else(|| PathBuf::from("corpus_md"))
}

/// Effective corpus the pipeline stages read: the preprocessed dir if enabled.
pub fn effective_corpus(cfg: &StudioConfig) -> PathBuf {
    if cfg.preprocess {
        cfg.out_dir.join("preprocessed")
    } else {
        cfg.corpus.clone()
    }
}

/// Scan a corpus directory and return the file extensions present, most-common
/// first. The C tools filter by CORPUS_EXT (default ".md"), so a corpus of
/// .txt files silently matches ZERO files and the first stage produces an
/// empty tokenizer — which only blows up downstream.
pub fn detect_corpus_exts(dir: &Path) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    fn walk(dir: &Path, counts: &mut HashMap<String, usize>, depth: usize) {
        if depth > 6 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.starts_with('.'))
                    .unwrap_or(false)
                {
                    continue;
                }
                walk(&p, counts, depth + 1);
            } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                *counts.entry(format!(".{ext}")).or_insert(0) += 1;
            }
        }
    }
    walk(dir, &mut counts, 0);
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// How many files in `dir` match the comma-separated `exts` list (suffix match).
pub fn count_matching_files(dir: &Path, exts: &str) -> usize {
    let exts: Vec<&str> = exts
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if exts.is_empty() {
        return 0;
    }
    let mut n = 0;
    fn walk(dir: &Path, exts: &[&str], n: &mut usize, depth: usize) {
        if depth > 6 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.starts_with('.'))
                    .unwrap_or(false)
                {
                    continue;
                }
                walk(&p, exts, n, depth + 1);
            } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if exts.iter().any(|x| name.ends_with(x)) {
                    *n += 1;
                }
            }
        }
    }
    walk(dir, &exts, &mut n, 0);
    n
}

/// The output file a stage is expected to produce (for post-run validation).
/// None for stages with no single canonical output (preprocess).
pub fn stage_output(cfg: &StudioConfig, idx: usize) -> Option<PathBuf> {
    let o = |n: &str| cfg.out_dir.join(n);
    match idx {
        1 => Some(o("tokenizer.bin")),
        2 => Some(o("model.bin")),
        3 => Some(o("vectors.bin")),
        4 => Some(o("paragraph_vectors.bin")),
        5 => Some(o("transitions.bin")),
        6 => Some(o("gpt.bin")),
        _ => None,
    }
}

/// Build the (program, args, set_corpus_ext) for a stage. Returns None to skip.
/// Ported verbatim: the argv these tools receive must not drift between shells.
pub fn stage_command(cfg: &StudioConfig, idx: usize) -> Option<(PathBuf, Vec<String>, bool)> {
    let bin = |name: &str| cfg.bin_dir.join(name);
    let out = |name: &str| cfg.out_dir.join(name).display().to_string();
    let corpus = effective_corpus(cfg).display().to_string();
    match idx {
        0 => {
            if !cfg.preprocess {
                return None;
            }
            let pre = cfg.out_dir.join("preprocessed").display().to_string();
            let subst = cfg
                .tool_args
                .replace("{in}", &cfg.corpus.display().to_string())
                .replace("{out}", &pre);
            let parts: Vec<String> = subst.split_whitespace().map(|s| s.to_string()).collect();
            if cfg.builtin_pp {
                // Bundled Rust converter (sits next to this binary) — no python/html2text.
                Some((builtin_converter(), parts, false))
            } else {
                let tool = cfg.tool.clone()?;
                // Python tools (e.g. create-md-from-web.py) run via the interpreter.
                if tool.extension().and_then(|s| s.to_str()) == Some("py") {
                    let mut args = vec![tool.display().to_string()];
                    args.extend(parts);
                    Some((PathBuf::from("python3"), args, false))
                } else {
                    Some((tool, parts, false))
                }
            }
        }
        1 => Some((
            bin("bpe_train"),
            vec![corpus, out("tokenizer.bin"), cfg.merges.to_string()],
            true,
        )),
        2 => Some((
            bin("ngram_train"),
            vec![corpus, out("tokenizer.bin"), out("model.bin"), cfg.max_n.to_string()],
            true,
        )),
        3 => Some((
            bin("word2vec_train"),
            vec![
                corpus,
                out("tokenizer.bin"),
                out("vectors.bin"),
                cfg.w2v_dim.to_string(),
                cfg.w2v_epochs.to_string(),
            ],
            true,
        )),
        4 => Some((
            bin("sentence_pool"),
            vec![corpus, out("tokenizer.bin"), out("vectors.bin"), out("paragraph_vectors.bin")],
            true,
        )),
        5 => Some((
            bin("transitions"),
            vec![out("paragraph_vectors.bin"), out("transitions.bin"), cfg.niches.to_string()],
            false,
        )),
        6 => Some((
            bin("tiny_gpt"),
            vec![
                "train".into(),
                corpus,
                out("tokenizer.bin"),
                out("gpt.bin"),
                cfg.gpt_steps.to_string(),
                cfg.gpt_dim.to_string(),
                cfg.gpt_layers.to_string(),
            ],
            true,
        )),
        _ => None,
    }
}

/// Pre-flight: if a corpus-reading stage is queued and the configured extension
/// matches zero files, fail NOW with an actionable message rather than letting
/// stage 1 emit an empty tokenizer that blows up at step 3. (Skipped when
/// preprocessing, which generates files at runtime.)
pub fn preflight(cfg: &StudioConfig, stages: &[usize]) -> Result<(), String> {
    let reads_corpus = stages.iter().any(|s| matches!(s, 1 | 2 | 3 | 4 | 6));
    if reads_corpus && !cfg.preprocess {
        let n = count_matching_files(&effective_corpus(cfg), &cfg.corpus_ext);
        if n == 0 {
            let present = detect_corpus_exts(&effective_corpus(cfg));
            let hint = if present.is_empty() {
                "the corpus folder appears to contain no files".to_string()
            } else {
                let list: Vec<String> = present
                    .iter()
                    .take(4)
                    .map(|(e, c)| format!("{e}×{c}"))
                    .collect();
                format!(
                    "corpus contains {} but CORPUS_EXT='{}' matches none — set the extension field to match",
                    list.join(", "),
                    cfg.corpus_ext
                )
            };
            return Err(format!("⚠ 0 corpus files match — {hint}"));
        }
    }
    Ok(())
}

/// Copy the corpus into the per-process staging dir ("Stage to temp"). Returns
/// the new corpus path. Uses `cp -r` like the GTK studio (the corpus can be
/// large; a Rust file-walk copy would be slower and no safer here).
pub fn stage_to_temp(corpus: &Path) -> Result<PathBuf, String> {
    let dir = staging_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ok = Command::new("cp")
        .arg("-r")
        .arg(corpus)
        .arg(&dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err("staging copy failed".into());
    }
    let name = corpus.file_name().ok_or("corpus has no name")?;
    Ok(dir.join(name))
}

// ── the worker ───────────────────────────────────────────────────────────────

/// Run a sequence of stages, streaming output back. The GTK `run_stages`,
/// with `std::sync::mpsc` in place of async-channel.
pub fn run_stages(
    cfg: StudioConfig,
    stages: Vec<usize>,
    tx: Sender<StudioMsg>,
    stop: Arc<AtomicBool>,
) {
    let _ = std::fs::create_dir_all(&cfg.out_dir);
    if cfg.preprocess {
        let _ = std::fs::create_dir_all(cfg.out_dir.join("preprocessed"));
    }
    for idx in stages {
        if stop.load(Ordering::Relaxed) {
            let _ = tx.send(StudioMsg::Log("stopped.".into()));
            break;
        }
        let cmd = match stage_command(&cfg, idx) {
            Some(c) => c,
            None => {
                let _ = tx.send(StudioMsg::StepDone(idx, true)); // skipped == ok
                let _ = tx.send(StudioMsg::Log(format!("[{}] skipped", STAGE_NAMES[idx])));
                continue;
            }
        };
        let (prog, args, set_ext) = cmd;
        let _ = tx.send(StudioMsg::StepStart(idx));
        let _ = tx.send(StudioMsg::StepLog(
            idx,
            format!("$ {} {}", prog.display(), args.join(" ")),
        ));

        let mut command = Command::new(&prog);
        command.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if set_ext && !cfg.corpus_ext.trim().is_empty() {
            command.env("CORPUS_EXT", &cfg.corpus_ext);
        }
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(StudioMsg::StepLog(idx, format!("failed to launch: {e}")));
                let _ = tx.send(StudioMsg::StepDone(idx, false));
                break;
            }
        };
        // Forward stdout on a helper thread; read stderr (where the C tools log) here.
        if let Some(out) = child.stdout.take() {
            let txo = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = txo.send(StudioMsg::StepLog(idx, line));
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send(StudioMsg::StepLog(idx, line));
                if stop.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break;
                }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        // Output validation: a stage can exit 0 but produce nothing (e.g. a
        // corpus that matched no files, or a partial write). Verify the expected
        // artifact exists and is non-empty so the failure surfaces HERE, at the
        // real stage, instead of cascading and looking like a later step's fault.
        let ok = if ok {
            match stage_output(&cfg, idx) {
                Some(out) => match std::fs::metadata(&out) {
                    Ok(m) if m.len() > 0 => true,
                    _ => {
                        let _ = tx.send(StudioMsg::StepLog(
                            idx,
                            format!(
                                "exit 0 but expected output is missing or empty: {}",
                                out.display()
                            ),
                        ));
                        let _ = tx.send(StudioMsg::StepLog(
                            idx,
                            format!(
                                "→ {} produced no usable output — check the corpus extension and earlier stages.",
                                STAGE_NAMES[idx]
                            ),
                        ));
                        false
                    }
                },
                None => true,
            }
        } else {
            false
        };
        let _ = tx.send(StudioMsg::StepDone(idx, ok));
        if !ok {
            let _ = tx.send(StudioMsg::Log(format!(
                "[{}] FAILED — halting",
                STAGE_NAMES[idx]
            )));
            break;
        }
    }
    let _ = tx.send(StudioMsg::Finished);
}

// ── the host: worker ↔ state machine ────────────────────────────────────────

/// Owns the worker's channel and the stop flag, and translates `StudioMsg`
/// into `Studio` transitions. The shell calls `action` on button presses and
/// `drain` every frame.
pub struct StudioHost {
    pub cfg: StudioConfig,
    /// One-line status, mirroring the GTK statusbar.
    pub status: String,
    rx: Option<Receiver<StudioMsg>>,
    stop: Arc<AtomicBool>,
}

impl Default for StudioHost {
    fn default() -> Self {
        let sticky = load_settings_from(&settings_path());
        StudioHost {
            cfg: StudioConfig::default().load_sticky(&sticky),
            status: "Ready".into(),
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl StudioHost {
    /// A worker is alive until its `Finished` arrives.
    pub fn worker_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Handle a `studio_ui` action. Start runs every currently-Pending stage
    /// (after a Reset that is all of them — the GTK "Run all").
    pub fn action(&mut self, act: StudioAction, studio: &mut Studio) {
        match act {
            StudioAction::Start => {
                if self.worker_running() {
                    return;
                }
                let stages: Vec<usize> = (0..STAGE_NAMES.len())
                    .filter(|&i| studio.state(i) == StageState::Pending)
                    .collect();
                if stages.is_empty() {
                    self.status = "pipeline already complete — Reset to run again".into();
                    return;
                }
                if let Err(msg) = preflight(&self.cfg, &stages) {
                    self.status = msg;
                    return;
                }
                if let Err(e) = save_settings_to(&settings_path(), &self.cfg.sticky_map()) {
                    // Said, not swallowed — but the run STARTS anyway.
                    // Losing the settings is annoying; refusing to run
                    // because they could not be written would be worse,
                    // and the person can see the warning and decide.
                    self.status = format!("settings not saved ({e}) — running anyway");
                }
                self.stop.store(false, Ordering::Relaxed);
                self.status = "running…".into();
                let (tx, rx) = std::sync::mpsc::channel();
                self.rx = Some(rx);
                let cfg = self.cfg.clone();
                let stop = self.stop.clone();
                std::thread::spawn(move || run_stages(cfg, stages, tx, stop));
            }
            StudioAction::Stop => {
                self.stop.store(true, Ordering::Relaxed);
                studio.request_stop();
                self.status = "stopping after this stage…".into();
            }
            StudioAction::Reset => {
                if self.worker_running() {
                    return;
                }
                studio.reset();
                self.status = "Ready".into();
            }
        }
    }

    /// Apply everything the worker has sent. Returns true if anything changed
    /// (the shell requests a repaint on true).
    pub fn drain(&mut self, studio: &mut Studio) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(msg) => {
                    changed = true;
                    match msg {
                        StudioMsg::Log(s) => self.status = s,
                        StudioMsg::StepStart(i) => {
                            studio.begin(i);
                            self.status = format!("running {}", STAGE_NAMES[i]);
                        }
                        StudioMsg::StepLog(i, line) => studio.log_at(i, &line),
                        StudioMsg::StepDone(i, ok) => {
                            if studio.running() == Some(i) {
                                studio.finish(
                                    i,
                                    if ok { StageState::Done } else { StageState::Failed },
                                );
                                if !ok {
                                    self.status =
                                        format!("Stopped at {}", STAGE_NAMES[i]);
                                }
                            } else if studio.state(i) == StageState::Pending && ok {
                                // The skip path: the worker never sent StepStart.
                                studio.begin(i);
                                studio.finish(i, StageState::Skipped);
                            }
                        }
                        StudioMsg::Finished => finished = true,
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.rx = None;
            if !studio.halted() && studio.next_stage().is_none() && !studio.stop_requested() {
                self.status = "Complete".into();
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join("studio-host-tests").join(format!(
            "{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn base_cfg(out: &Path) -> StudioConfig {
        StudioConfig {
            out_dir: out.to_path_buf(),
            bin_dir: PathBuf::from("/opt/v15/bin"),
            corpus: PathBuf::from("/data/corpus"),
            ..StudioConfig::default()
        }
    }

    // ── argv parity: the exact commands the GTK studio would run ──

    #[test]
    fn stage_argv_matches_the_gtk_studio() {
        let cfg = StudioConfig {
            merges: 8192,
            max_n: 5,
            w2v_dim: 96,
            w2v_epochs: 12,
            gpt_steps: 4000,
            gpt_dim: 192,
            gpt_layers: 3,
            niches: 8,
            ..base_cfg(Path::new("/out"))
        };
        let (p, a, ext) = stage_command(&cfg, 1).unwrap();
        assert_eq!(p, PathBuf::from("/opt/v15/bin/bpe_train"));
        assert_eq!(a, vec!["/data/corpus", "/out/tokenizer.bin", "8192"]);
        assert!(ext, "corpus-reading stages take CORPUS_EXT");

        let (p, a, _) = stage_command(&cfg, 2).unwrap();
        assert_eq!(p, PathBuf::from("/opt/v15/bin/ngram_train"));
        assert_eq!(a, vec!["/data/corpus", "/out/tokenizer.bin", "/out/model.bin", "5"]);

        let (p, a, _) = stage_command(&cfg, 3).unwrap();
        assert_eq!(p, PathBuf::from("/opt/v15/bin/word2vec_train"));
        assert_eq!(a, vec!["/data/corpus", "/out/tokenizer.bin", "/out/vectors.bin", "96", "12"]);

        let (_, a, ext) = stage_command(&cfg, 5).unwrap();
        assert_eq!(a, vec!["/out/paragraph_vectors.bin", "/out/transitions.bin", "8"]);
        assert!(!ext, "transitions reads vectors, not the corpus");

        let (_, a, _) = stage_command(&cfg, 6).unwrap();
        assert_eq!(
            a,
            vec!["train", "/data/corpus", "/out/tokenizer.bin", "/out/gpt.bin", "4000", "192", "3"]
        );
    }

    #[test]
    fn preprocess_substitutes_in_and_out_and_routes_python_via_interpreter() {
        let mut cfg = base_cfg(Path::new("/out"));
        cfg.preprocess = true;
        cfg.builtin_pp = false;
        cfg.tool = Some(PathBuf::from("/tools/create-md-from-web.py"));
        cfg.tool_args = "{in} {out}/corpus.md -j 4".into();
        let (p, a, _) = stage_command(&cfg, 0).unwrap();
        assert_eq!(p, PathBuf::from("python3"), ".py tools run via the interpreter");
        assert_eq!(
            a,
            vec![
                "/tools/create-md-from-web.py",
                "/data/corpus",
                "/out/preprocessed/corpus.md",
                "-j",
                "4"
            ]
        );
        // Preprocess off → stage 0 skips.
        cfg.preprocess = false;
        assert!(stage_command(&cfg, 0).is_none());
        // And the effective corpus flips with the toggle.
        assert_eq!(effective_corpus(&cfg), PathBuf::from("/data/corpus"));
        cfg.preprocess = true;
        assert_eq!(effective_corpus(&cfg), PathBuf::from("/out/preprocessed"));
    }

    // ── settings: shared file, same keys, GTK float formats ──

    #[test]
    fn sticky_settings_roundtrip_and_parse_gtk_float_values() {
        let dir = tmp("settings");
        let path = dir.join("settings.conf");
        let mut cfg = StudioConfig::default();
        cfg.corpus = PathBuf::from("/data/my corpus/html"); // path with spaces
        cfg.merges = 8192;
        cfg.preprocess = true;
        // Unwrapped rather than ignored: this test asserts a round-trip,
        // and a failed write would otherwise make it pass against the
        // PREVIOUS file — or against nothing, if the load also silently
        // returns an empty map.
        save_settings_to(&path, &cfg.sticky_map()).expect("settings written");
        let back = StudioConfig::default().load_sticky(&load_settings_from(&path));
        assert_eq!(back.corpus, PathBuf::from("/data/my corpus/html"));
        assert_eq!(back.merges, 8192);
        assert!(back.preprocess);

        // GTK writes Scale::value() — f64 strings like "8192.0". They must parse.
        let mut m = HashMap::new();
        m.insert("merges".to_string(), "8192.0".to_string());
        m.insert("gpt_layers".to_string(), "3.0".to_string());
        let cfg = StudioConfig::default().load_sticky(&m);
        assert_eq!(cfg.merges, 8192);
        assert_eq!(cfg.gpt_layers, 3);
    }

    // ── preflight: the zero-match corpus fails before anything spawns ──

    #[test]
    fn preflight_names_the_extensions_that_are_present() {
        let dir = tmp("preflight");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("b.txt"), "y").unwrap();
        let mut cfg = base_cfg(&tmp("preflight-out"));
        cfg.corpus = dir;
        cfg.corpus_ext = ".md".into();
        let err = preflight(&cfg, &[1, 2, 3]).unwrap_err();
        assert!(err.contains(".txt×2"), "hint lists what IS there: {err}");
        assert!(err.contains(".md"), "and what was asked for: {err}");
        // Preprocess generates files at runtime — the check is skipped.
        cfg.preprocess = true;
        assert!(preflight(&cfg, &[1, 2, 3]).is_ok());
        // A stage list that never reads the corpus doesn't care.
        cfg.preprocess = false;
        assert!(preflight(&cfg, &[5]).is_ok());
    }

    #[test]
    fn counting_respects_the_comma_list_and_skips_dot_dirs() {
        let dir = tmp("count");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/x.md"), "hidden").unwrap();
        std::fs::write(dir.join("a.md"), "x").unwrap();
        std::fs::write(dir.join("b.markdown"), "y").unwrap();
        std::fs::write(dir.join("c.txt"), "z").unwrap();
        assert_eq!(count_matching_files(&dir, ".md"), 1, ".git is not corpus");
        assert_eq!(count_matching_files(&dir, ".md, .txt"), 2);
        assert_eq!(count_matching_files(&dir, ""), 0, "no extensions, no matches");
    }

    // ── the worker, end to end, with real processes ──

    /// A fake stage binary: logs a line and produces its artifact — or fails.
    fn fake_bin(dir: &Path, name: &str, body: &str) {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Run the host+machine loop to completion, draining like a frame loop.
    fn run_to_finish(host: &mut StudioHost, studio: &mut Studio) {
        host.action(StudioAction::Start, studio);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while host.worker_running() {
            host.drain(studio);
            assert!(std::time::Instant::now() < deadline, "worker hung");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        host.drain(studio);
    }

    #[test]
    fn a_full_run_completes_and_the_artifacts_gate_success() {
        let bins = tmp("bins-ok");
        let out = tmp("out-ok");
        let corpus = tmp("corpus-ok");
        std::fs::write(corpus.join("a.md"), "hello world").unwrap();
        // Every stage: echo something, write a non-empty artifact.
        for (name, artifact) in [
            ("bpe_train", "tokenizer.bin"),
            ("ngram_train", "model.bin"),
            ("word2vec_train", "vectors.bin"),
            ("sentence_pool", "paragraph_vectors.bin"),
            ("transitions", "transitions.bin"),
            ("tiny_gpt", "gpt.bin"),
        ] {
            fake_bin(
                &bins,
                name,
                &format!("echo training >&2; printf data > \"{}\"", out.join(artifact).display()),
            );
        }
        let mut host = StudioHost {
            cfg: StudioConfig {
                corpus,
                bin_dir: bins,
                out_dir: out,
                corpus_ext: ".md".into(),
                ..StudioConfig::default()
            },
            status: String::new(),
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
        };
        let mut studio = Studio::default();
        run_to_finish(&mut host, &mut studio);
        assert_eq!(studio.state(0), StageState::Skipped, "preprocess off = skipped, run continues");
        for i in 1..STAGE_NAMES.len() {
            assert_eq!(studio.state(i), StageState::Done, "stage {i}");
        }
        assert_eq!(studio.summary(), "Complete");
        assert_eq!(host.status, "Complete");
        assert!(
            studio.stages()[1].log.iter().any(|l| l.contains("training")),
            "stderr streamed into the stage log"
        );
    }

    #[test]
    fn an_empty_artifact_fails_the_stage_that_produced_it_and_halts() {
        let bins = tmp("bins-empty");
        let out = tmp("out-empty");
        let corpus = tmp("corpus-empty");
        std::fs::write(corpus.join("a.md"), "hello").unwrap();
        // Stage 1 exits 0 but writes an EMPTY tokenizer — the classic silent failure.
        fake_bin(&bins, "bpe_train", &format!(": > \"{}\"", out.join("tokenizer.bin").display()));
        fake_bin(&bins, "ngram_train", "echo should-never-run; exit 0");
        let mut host = StudioHost {
            cfg: StudioConfig {
                corpus,
                bin_dir: bins,
                out_dir: out,
                corpus_ext: ".md".into(),
                ..StudioConfig::default()
            },
            status: String::new(),
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
        };
        let mut studio = Studio::default();
        run_to_finish(&mut host, &mut studio);
        assert_eq!(studio.state(1), StageState::Failed, "the failure lands at ITS stage");
        assert_eq!(studio.state(2), StageState::Pending, "nothing after it ran");
        assert!(studio.halted());
        assert!(
            studio.stages()[1].log.iter().any(|l| l.contains("missing or empty")),
            "the log says why"
        );
        assert!(studio.stages()[1].expanded, "the failing pane opened itself");
    }

    #[test]
    fn preflight_blocks_start_and_leaves_the_machine_untouched() {
        let corpus = tmp("corpus-none");
        std::fs::write(corpus.join("a.txt"), "not md").unwrap();
        let mut host = StudioHost {
            cfg: StudioConfig {
                corpus,
                bin_dir: tmp("bins-none"),
                out_dir: tmp("out-none"),
                corpus_ext: ".md".into(),
                ..StudioConfig::default()
            },
            status: String::new(),
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
        };
        let mut studio = Studio::default();
        host.action(StudioAction::Start, &mut studio);
        assert!(!host.worker_running(), "nothing spawned");
        assert!(host.status.contains("0 corpus files match"));
        assert_eq!(studio.state(1), StageState::Pending);
        assert!(studio.can_start(), "the machine is untouched — fix the ext and Run again");
    }
}
