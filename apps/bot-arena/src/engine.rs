//! engine.rs — the orchestration loop, independent of any GUI.
//!
//! v2 adds the "missing piece": workers are grounded in a latent space derived
//! from a text corpus, so each worker carries a *purpose* coordinate `z`, not just
//! a fitness. On top of that:
//!
//!   - Corpus -> niche labels: the corpus is tokenized and split into `niches`
//!     bins, each tagged with a representative word. A worker's `z in [0,1]` maps
//!     to a niche, so every worker is *associated with text and purpose*. (The
//!     principled version drops in vectors.bin / transitions.bin from the v1.2/
//!     v1.3 pipeline as the real latent space; this is the same mechanism with a
//!     lightweight grounding so the loop is self-contained.)
//!   - MAP-Elites-style selection: keep an elite fraction per niche instead of
//!     collapsing to one global best, so the swarm spreads across purposes
//!     (Quality-Diversity).
//!   - A latent-axis knob (`axis`, a shared atomic the GUI scrollbar drives) sets
//!     the coordinator's current focus — which purpose-region to deploy.
//!   - A coordinator that, when enabled, *learns the axis*: each generation it
//!     moves its focus toward the niche with the best reward, i.e. it learns to
//!     use the swarm.
//!
//! No GTK types appear here, so it stays `Send` and testable in isolation.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use rand::rngs::SmallRng;
use rand::SeedableRng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BackendKind {
    Sim,
    Docker,
    Conda,
    Local,
}

impl BackendKind {
    pub fn from_index(i: u32) -> Self {
        match i {
            1 => BackendKind::Docker,
            2 => BackendKind::Conda,
            3 => BackendKind::Local,
            _ => BackendKind::Sim,
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub population: usize,
    pub workers_per_container: usize,
    pub keep_fraction: f64,
    pub generations: usize,
    pub tools: Vec<String>,
    pub image: String,
    pub workspace: Option<PathBuf>,   // single repo/crate (back-compat); see `workspaces`
    pub workspaces: Vec<PathBuf>,     // repos / crates / superdirs; superdirs auto-expand
    pub backend: BackendKind,
    // latent / purpose
    pub corpus: Option<PathBuf>,
    pub niches: usize,
    pub coordinator: bool,
    // real evaluation
    pub test_cmd: String,
    pub cache: bool,
    pub fast_clone: bool,
    pub timeout_secs: u64, // per-command wall-clock kill (0 = no timeout)
    pub max_file_mb: u64,  // per-command single-file size cap (ulimit -f)
    pub memoize: bool,     // reuse scores for repo states we've already evaluated
    pub crossover: bool,   // recombine two survivors into hybrid offspring
    pub sandbox: bool,     // confine each command with bubblewrap (no net, writes limited to worktree)
    pub threads: usize,    // parallel real-backend workers, each with its own target dir (1 = sequential)
    pub ops: Vec<String>,  // enabled edit operators (see EDIT_OPS)
    pub aggression: f64,   // 0 = timid (tiny edits), 1 = aggressive (large edits / more random chars)
    pub seed: Option<u64>, // fixed seed → reproducible run; None → random
    pub batch_start: usize,// best-of-k attempts per bot at gen 0, annealing to 1 (learning rate)
    pub turbo: bool,       // skip the per-generation animation sleeps (fast/headless iteration)
    pub allow_test_edits: bool, // off → bots that touch test files score 0 (reward-hack tripwire)
    pub cargo_features: String, // e.g. "foo,bar" → cargo test --features foo,bar (blank = none)
    pub build_flags: String,    // extra args appended to the auto test command (cargo/pytest/npm/go)
    pub extra_tools: Vec<String>, // extra binaries to allow inside shelter (e.g. qmake, moc for Qt)
    pub recurrent_steps: usize, // 0 = one-shot/batched; >0 = RNN bot: hill-climb N steps per eval
    pub novelty_lambda: f64,    // selection weight: fitness + λ·distance(genome, recent survivors)
    pub resume: bool,           // at run start, load latest checkpoint if one exists
    pub differential: bool,     // score by fixed−λ·broke vs a pristine baseline (denser signal)
    pub drift: f64,             // Sim only: 0 = static landscape; >0 = optimum drifts (a chase)
    pub proposer_cmd: String,   // external "context→unified-diff" command; empty = random mutation
    pub proposer_fallbacks: Vec<String>, // escalation chain tried (in order) when the primary doesn't fully solve
    pub context_cmds: Vec<String>, // ordered context→context enrichment stages (RAG retrieval, planning, …) run once before the proposer
}

impl Default for Config {
    /// Sensible headless defaults: a small Sim population that finishes instantly.
    /// Callers (GUI, CLI, tests) override only the fields they care about.
    fn default() -> Self {
        Config {
            population: 24, workers_per_container: 1, keep_fraction: 0.5, generations: 10,
            tools: Vec::new(), image: String::new(), workspace: None, workspaces: Vec::new(),
            backend: BackendKind::Sim, corpus: None, niches: 1, coordinator: false,
            test_cmd: "auto".into(), cache: false, fast_clone: false, timeout_secs: 0,
            max_file_mb: 100, memoize: true, crossover: false, sandbox: false, threads: 1,
            ops: all_edit_ops(), aggression: 0.3, seed: None, batch_start: 1, turbo: true,
            allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(),
            extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BotStatus {
    Idle,
    Provisioning,
    Running,
    Passed,
    Failed,
    Culled,
    Spawned,
}

impl BotStatus {
    pub fn css_class(self) -> &'static str {
        match self {
            BotStatus::Idle => "bot-idle",
            BotStatus::Provisioning => "bot-prov",
            BotStatus::Running => "bot-run",
            BotStatus::Passed => "bot-pass",
            BotStatus::Failed => "bot-fail",
            BotStatus::Culled => "bot-cull",
            BotStatus::Spawned => "bot-spawn",
        }
    }
}

pub enum EngineMsg {
    Log(String),
    Bot {
        id: usize,
        status: BotStatus,
        fitness: f64,
        z: f64,
        purpose: String,
    },
    /// A short human-readable note of what a worker is currently doing (the edit
    /// kind and the command it will run). Surfaced in the live activity panel.
    Activity {
        id: usize,
        text: String,
    },
    Generation {
        n: usize,
        best: f64,
        mean: f64,
        alive: usize,
        focus: f64,
    },
    /// Per-generation snapshot for the Replays tab: the generation's best/mean and
    /// its top-k bots (id, z, fitness). Lets the GUI buffer the whole run and let
    /// the user scrub back through what the leaders looked like at each generation.
    Replay {
        gen: usize,
        best: f64,
        mean: f64,
        top: Vec<(usize, f64, f64)>,
    },
    /// Coordinator state: which purpose-region it is currently deploying.
    Coordinator {
        focus: f64,
        label: String,
        niche_reward: f64,
    },
    Done,
}

#[derive(Clone)]
struct Bot {
    id: usize,
    genome: f64, // latent "quality" the GA optimizes
    z: f64,      // latent "purpose" coordinate in [0,1]
    fitness: f64,
    alive: bool,
}

fn log(tx: &async_channel::Sender<EngineMsg>, s: impl Into<String>) {
    let _ = tx.send_blocking(EngineMsg::Log(s.into()));
}

/// Decode the shared atomic latent-axis knob (stored as f32 bits) into [0,1].
fn read_axis(axis: &AtomicU32) -> f64 {
    f32::from_bits(axis.load(Ordering::Relaxed)) as f64
}

pub fn container_count(population: usize, workers_per_container: usize) -> usize {
    let wpc = workers_per_container.max(1);
    (population + wpc - 1) / wpc
}

fn niche_of(z: f64, niches: usize) -> usize {
    ((z * niches as f64) as usize).min(niches.saturating_sub(1))
}

/// A directory is a "project root" if it carries a recognized build manifest.
fn is_project_root(dir: &Path) -> bool {
    const MANIFESTS: &[&str] = &[
        "Cargo.toml", "package.json", "CMakeLists.txt", "meson.build",
        "go.mod", "pyproject.toml", "setup.py",
    ];
    MANIFESTS.iter().any(|m| dir.join(m).exists())
}

fn is_noise_dir(name: &str) -> bool {
    matches!(name, ".git" | "target" | "build" | "node_modules" | "__pycache__"
        | ".pytest_cache" | ".venv" | "dist" | ".idea" | ".vscode")
}

/// Discover every sub-project under `root` (each dir with a build manifest), without
/// descending *into* a project once found — so a superdir of many sibling crates
/// yields each crate. If `root` itself is a project, returns just `[root]`. This is
/// the autodetection that turns "point me at a folder of repos" into a target list.
fn discover_projects(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, d)) = stack.pop() {
        if is_project_root(&dir) {
            found.push(dir);
            continue; // a project's internals aren't separate targets
        }
        if d >= max_depth {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && !is_noise_dir(&e.file_name().to_string_lossy()) {
                    stack.push((p, d + 1));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The flat list of project targets to train against: every entry in `workspaces`
/// (or the single `workspace`) expanded via `discover_projects`, deduped. Multiple
/// repos / superdirs / individual crates can be mixed; training is diversified by
/// rotating bots across this list.
pub fn effective_targets(cfg: &Config) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = cfg.workspaces.clone();
    if roots.is_empty() {
        if let Some(w) = &cfg.workspace { roots.push(w.clone()); }
    }
    let mut targets = Vec::new();
    for r in &roots {
        targets.extend(discover_projects(r, 4));
    }
    targets.sort();
    targets.dedup();
    targets
}

/// The run's master seed: the configured seed, or one drawn from entropy. Logged so
/// a "lucky" random run can be reproduced by pasting the seed back in.
fn run_seed(cfg: &Config) -> u64 {
    cfg.seed.unwrap_or_else(|| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut h);
        std::process::id().hash(&mut h);
        h.finish()
    })
}

/// A fast, non-crypto, reproducible PRNG. `thread_rng()` is a reseeding CSPRNG —
/// far more than a GA needs, and not reproducible; `SmallRng` (xoshiro) is ~an order
/// of magnitude cheaper per draw and seedable, which matters when there are millions
/// of draws per run at scale.
fn seeded_rng(seed: u64) -> SmallRng {
    SmallRng::seed_from_u64(seed)
}

/// Best-of-k batch size for a generation: starts at `start` and anneals linearly to
/// 1 by the final generation. Evaluating each bot over several attempts and keeping
/// the best raises the effective learning rate early (broad sampling), then narrows
/// to a single decisive attempt late (exploitation).
fn batch_size(gen: usize, generations: usize, start: usize) -> usize {
    let start = start.max(1);
    if start == 1 || generations <= 1 {
        return start;
    }
    let frac = gen as f64 / (generations - 1) as f64; // 0 at first gen, 1 at last
    (start as f64 * (1.0 - frac) + frac).round().max(1.0) as usize
}

/// The Sim's hidden per-purpose optimum: the genome value each point on the latent
/// axis actually wants. It varies across the axis, so different purposes prefer
/// different genomes — there's a target to *find*, not just a ceiling to ram.
fn sim_target(z: f64) -> f64 {
    0.5 + 0.35 * (z * std::f64::consts::TAU).sin()
}

/// The per-purpose optimum, optionally *drifting* over generations. With `drift = 0`
/// this is exactly `sim_target(z)` (the static landscape). With `drift > 0` each
/// niche's optimum oscillates (out of phase across the axis), so a converged
/// population can't just sit on the peak — it has to keep tracking a moving target.
/// This is the static-Sim fix: it turns the post-convergence plateau into a chase,
/// a far better proxy for "the codebase keeps changing under you."
fn sim_target_at(z: f64, gen: usize, drift: f64) -> f64 {
    let base = sim_target(z);
    if drift <= 0.0 { return base; }
    let amp = 0.30 * drift.clamp(0.0, 1.0);
    let phase = gen as f64 * 0.04 + z * std::f64::consts::TAU; // niches drift out of phase
    (base + amp * phase.sin()).clamp(0.05, 0.95)
}

fn sim_fitness_at(genome: f64, z: f64, focus: f64, noise: f64, gen: usize, drift: f64) -> f64 {
    let quality = (1.0 - (genome - sim_target_at(z, gen, drift)).abs()).clamp(0.0, 1.0);
    let focus_match = (1.0 - (z - focus).abs()).clamp(0.0, 1.0);
    (0.70 * quality + 0.18 * focus_match + 0.04 + noise).clamp(0.0, 1.0)
}

fn sim_fitness(genome: f64, z: f64, focus: f64, noise: f64) -> f64 {
    sim_fitness_at(genome, z, focus, noise, 0, 0.0)
}

/// Read a directory's text files and concatenate (best-effort), for corpus grounding.
fn read_dir_concat(dir: &PathBuf) -> std::io::Result<String> {
    let mut out = String::new();
    let mut stack = vec![dir.clone()];
    let mut budget = 4_000_000usize; // cap to keep it quick
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(s) = std::fs::read_to_string(&p) {
                let take = s.len().min(budget);
                out.push_str(&s[..take]);
                out.push(' ');
                budget = budget.saturating_sub(take);
                if budget == 0 {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

/// Build `niches` human-readable purpose labels from the corpus: tokenize, keep
/// distinct words >= 4 chars (a cheap proxy for content words), sample one per bin.
/// This is the literal "associate workers with text and purpose".
fn niche_labels(cfg: &Config, tx: &async_channel::Sender<EngineMsg>) -> Vec<String> {
    let n = cfg.niches.max(1);
    let mut words: Vec<String> = Vec::new();
    if let Some(path) = &cfg.corpus {
        let text = std::fs::read_to_string(path).or_else(|_| read_dir_concat(path)).unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            let w = raw.trim().to_lowercase();
            if w.len() >= 4 && seen.insert(w.clone()) {
                words.push(w);
            }
        }
    }
    if words.is_empty() {
        let defaults = [
            "refactor", "bugfix", "feature", "test", "docs", "perf", "cleanup", "rename",
            "guard", "io", "parse", "error", "config", "build", "api", "type",
        ];
        return (0..n).map(|i| defaults[i % defaults.len()].to_string()).collect();
    }
    let labels: Vec<String> = (0..n)
        .map(|i| {
            let idx = (i * words.len()) / n;
            words[idx.min(words.len() - 1)].clone()
        })
        .collect();
    log(tx, format!("latent space grounded in corpus: {} distinct words, {} niches", words.len(), n));
    log(tx, format!("niche purposes: [{}]", labels.join(", ")));
    labels
}
// ===== Real evaluation =====================================================
//
// The honest version of the agent loop: give each worker an isolated copy of the
// workspace, let it attempt a change, run the test suite, and score it by the
// fraction of tests passing. The "agent" here is a purpose-typed *mutation* — a
// baseline policy that the latent coordinate `z` selects the *kind* of edit for.
// Replace `mutate_source` with an LLM-driven editor to make this intelligent;
// everything around it (isolation, scoring, selection) already works.

use std::path::Path;

/// Root for this process's worktrees: temp/bot-arena-work-<pid>. Each run gets a
/// `run-<nanos>` subdir under it so concurrent runs never clobber each other.
fn work_root() -> PathBuf {
    std::env::temp_dir().join(format!("bot-arena-work-{}", std::process::id()))
}

/// Remove every worktree this process created. Call on app shutdown so nothing is
/// left in /tmp even if the window is closed mid-run.
pub fn cleanup_workspace_temp() {
    let _ = std::fs::remove_dir_all(work_root());
}

/// Recursively copy a directory, skipping VCS/build/venv noise.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let ns = name.to_string_lossy();
        if matches!(ns.as_ref(), ".git" | "__pycache__" | ".pytest_cache" | "node_modules" | "target" | ".venv") {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// A provisioned worktree, remembering how to tear itself down.
enum Provisioned {
    /// A plain recursive copy (or CoW copy) — remove the directory.
    Dir(PathBuf),
    /// A `git worktree` linked to `repo` — remove via git so the registration is cleaned too.
    Git { repo: PathBuf, path: PathBuf },
}

impl Provisioned {
    fn path(&self) -> &Path {
        match self { Provisioned::Dir(p) => p, Provisioned::Git { path, .. } => path }
    }
    fn remove(self) {
        match self {
            Provisioned::Dir(p) => { let _ = std::fs::remove_dir_all(&p); }
            Provisioned::Git { repo, path } => {
                let ok = std::process::Command::new("git").arg("-C").arg(&repo)
                    .args(["worktree", "remove", "--force"]).arg(&path)
                    .status().map(|s| s.success()).unwrap_or(false);
                if !ok {
                    let _ = std::fs::remove_dir_all(&path);
                    let _ = std::process::Command::new("git").arg("-C").arg(&repo).args(["worktree", "prune"]).status();
                }
            }
        }
    }
}

/// Provision a per-bot worktree, cheapest viable strategy first:
///   1. git worktree  — committed HEAD, shares the object store (git repos);
///   2. rsync --link-dest — hardlinks unchanged files (near-zero IO when the
///      worktree is on the same filesystem; safe because edits are atomic);
///   3. cp --reflink=auto — copy-on-write on btrfs/xfs (the portable "btrfs
///      snapshot" win, no subvolume setup needed);
///   4. recursive copy — universal fallback.
/// Strategies 2–4 skip VCS/build noise.
fn provision_worktree(cfg: &Config, ws: &Path, work: &Path) -> std::io::Result<Provisioned> {
    let _ = std::fs::remove_dir_all(work);

    if cfg.fast_clone && ws.join(".git").exists() && which("git") {
        let out = std::process::Command::new("git").arg("-C").arg(ws)
            .args(["worktree", "add", "--detach", "--force"]).arg(work).arg("HEAD")
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Ok(Provisioned::Git { repo: ws.to_path_buf(), path: work.to_path_buf() });
            }
        }
    }

    if cfg.fast_clone {
        let excludes = [".git", "target", "build", "node_modules", "__pycache__", ".pytest_cache", ".venv"];
        // rsync with hardlinks against the original (cheap when same-FS).
        if which("rsync") {
            let mut c = std::process::Command::new("rsync");
            c.arg("-a").arg("--link-dest").arg(ws);
            for e in &excludes { c.arg(format!("--exclude={e}")); }
            c.arg(format!("{}/", ws.display())).arg(work);
            if c.output().map(|o| o.status.success()).unwrap_or(false) && work.exists() {
                return Ok(Provisioned::Dir(work.to_path_buf()));
            }
        }
        // CoW copy (btrfs/xfs): copy whole tree, then strip noise.
        let cp = std::process::Command::new("cp")
            .args(["-a", "--reflink=auto"]).arg(format!("{}/.", ws.display())).arg(work).output();
        if cp.map(|o| o.status.success()).unwrap_or(false) && work.exists() {
            for e in &excludes { let _ = std::fs::remove_dir_all(work.join(e)); }
            return Ok(Provisioned::Dir(work.to_path_buf()));
        }
    }

    copy_dir_recursive(ws, work)?;
    Ok(Provisioned::Dir(work.to_path_buf()))
}

/// Collect candidate source files to edit (skipping test files so a mutation
/// changes the code under test, not the tests themselves).
fn collect_source_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) { Ok(r) => r, Err(_) => continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let ns = e.file_name().to_string_lossy().to_string();
                if !matches!(ns.as_str(), ".git" | "__pycache__" | ".pytest_cache" | "node_modules" | "target") {
                    stack.push(p);
                }
            } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                let stem = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let is_test = stem.starts_with("test_") || stem.ends_with("_test.py") || stem == "conftest.py";
                if matches!(ext, "py" | "c" | "rs" | "js" | "ts") && !is_test {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Swap a comparison/assignment operator, if present. Returns the edited line.
fn flip_ops(line: &str) -> String {
    // Bidirectional so the search can flip either way (e.g. fix `<`→`>` AND undo a
    // bad `>`→`<`). Spaced arithmetic forms (` + `, ` * `) avoid matching `+=`,
    // `->`, etc. Comparison/compound forms are checked before single chars.
    for (a, b) in [("==", "!="), (" <= ", " >= "), (" < ", " > "),
                   ("+=", "-="), ("*=", "/="), (" + ", " - "), (" * ", " / ")] {
        if line.contains(a) { return line.replacen(a, b, 1); }
        if line.contains(b) { return line.replacen(b, a, 1); }
    }
    line.to_string()
}

/// Apply one purpose-typed mutation: `z` selects which kind of edit to attempt.
/// Returns true if the file content actually changed.
/// Write atomically: write to a temp file then rename over the target. This keeps
/// hardlink-based provisioning (rsync --link-dest) safe — the shared inode is never
/// modified in place; rename swaps the directory entry to a fresh file.
fn atomic_write(path: &Path, bytes: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The available edit operators (the "edit commands"). NoOp is last so a high
/// purpose-coordinate maps to "leave the repo intact".
pub const EDIT_OPS: &[&str] = &[
    "flip-op", "flip-bool", "tweak-num", "math-op", "string-op",
    "comment-out", "uncomment", "dup-line", "delete-line", "swap-lines",
    "reorder", "wrap-loop", "splice-line", "insert-rand", "no-op",
];

pub fn all_edit_ops() -> Vec<String> {
    EDIT_OPS.iter().map(|s| s.to_string()).collect()
}

// ===== Auditor =============================================================
//
// A *non-coding* role: it makes no edits and competes for no fitness. Instead it
// watches what the coding bots are doing each generation and synthesizes it into
// (a) a structured record and (b) a plain-language overview, so a swarm of thousands
// is legible — auditable (what changed, what got flagged) and predictable (how
// concentrated vs. scattered the behavior is). Pure + testable; no GTK, no I/O.

/// One generation's audit. Fields are public so a GUI/report can render them.
#[derive(Clone, Debug)]
pub struct GenAudit {
    pub gen: usize,
    pub focus_label: String,
    pub alive: usize,
    pub best: f64,
    pub mean: f64,
    pub trend: &'static str,        // "rising" | "steady" | "falling"
    pub op_hist: Vec<(String, usize)>, // descending by count
    pub op_entropy: f64,            // 0 = one op dominates (predictable), 1 = uniform (exploratory)
    pub genome_sigma: f64,
    pub code_spread: Option<f64>,
    pub new_risk_flags: usize,      // tripwire/danger events this generation
}

/// Shannon entropy of a count distribution, normalized to [0,1] by log2(k).
fn norm_entropy(counts: &[usize]) -> f64 {
    let total: usize = counts.iter().sum();
    let k = counts.iter().filter(|&&c| c > 0).count();
    if total == 0 || k <= 1 { return 0.0; }
    let mut h = 0.0;
    for &c in counts {
        if c == 0 { continue; }
        let p = c as f64 / total as f64;
        h -= p * p.log2();
    }
    h / (k as f64).log2()
}

/// Build the op-usage histogram for a set of alive bots by mapping each bot's purpose
/// `z` through the same `op_for` the workers use, so it reflects what the population
/// is actually doing (or, in Sim, would do).
fn audit_op_hist(alive_zs: &[f64], ops: &[String]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &z in alive_zs {
        *counts.entry(op_for(z, ops)).or_insert(0) += 1;
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().map(|(k, c)| (k.to_string(), c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[allow(clippy::too_many_arguments)]
fn audit_generation(gen: usize, focus_label: &str, alive_zs: &[f64], ops: &[String],
    best: f64, mean: f64, prev_best: Option<f64>, genome_sigma: f64,
    code_spread: Option<f64>, new_risk_flags: usize) -> GenAudit {
    let op_hist = audit_op_hist(alive_zs, ops);
    let counts: Vec<usize> = op_hist.iter().map(|(_, c)| *c).collect();
    let op_entropy = norm_entropy(&counts);
    let trend = match prev_best {
        Some(p) if best > p + 0.003 => "rising",
        Some(p) if best < p - 0.003 => "falling",
        _ => "steady",
    };
    GenAudit { gen, focus_label: focus_label.to_string(), alive: alive_zs.len(),
        best, mean, trend, op_hist, op_entropy, genome_sigma, code_spread, new_risk_flags }
}

impl GenAudit {
    /// Compact machine-readable line (key=value), for logs/reports a tool can grep.
    pub fn structured(&self) -> String {
        let top_ops: Vec<String> = self.op_hist.iter().take(3)
            .map(|(n, c)| format!("{n}:{c}")).collect();
        format!("gen={} focus={} alive={} best={:.3} mean={:.3} trend={} entropy={:.2} sigma={:.3}{} risk={} ops=[{}]",
            self.gen, self.focus_label, self.alive, self.best, self.mean, self.trend,
            self.op_entropy, self.genome_sigma,
            self.code_spread.map(|s| format!(" spread={s:.3}")).unwrap_or_default(),
            self.new_risk_flags, top_ops.join(","))
    }

    /// Plain-language overview — the "conversational" audit. Synthesizes predictability
    /// (entropy), dominant behaviors, convergence, trend, and risk into one sentence.
    pub fn overview(&self) -> String {
        let total: usize = self.op_hist.iter().map(|(_, c)| c).sum::<usize>().max(1);
        let dominant: Vec<&str> = self.op_hist.iter().take(3)
            .filter(|(_, c)| *c > 0).map(|(n, _)| n.as_str()).collect();
        let share = self.op_hist.iter().take(3).map(|(_, c)| c).sum::<usize>() * 100 / total;
        let predictability = if self.op_entropy < 0.45 { "predictable" }
            else if self.op_entropy < 0.75 { "mixed" } else { "exploratory" };
        let convergence = if self.genome_sigma < 0.06 { "converged" }
            else if self.genome_sigma < 0.15 { "tightening" } else { "spread out" };
        let risk = if self.new_risk_flags == 0 { "no new risk flags".to_string() }
            else { format!("⚠ {} risk flag(s)", self.new_risk_flags) };
        format!("audit · gen {} ({}): best {:.3} {}, mean {:.3} · behaviour {}% in {{{}}} (entropy {:.2} → {}) · {} (σ{:.3}) · {}",
            self.gen, self.focus_label, self.best, self.trend, self.mean, share,
            dominant.join(", "), self.op_entropy, predictability, convergence,
            self.genome_sigma, risk)
    }
}

/// Pick the enabled operator a purpose-coordinate `z` maps to.
fn op_for<'a>(z: f64, ops: &'a [String]) -> &'a str {
    if ops.is_empty() { return "no-op"; }
    let idx = ((z * ops.len() as f64) as usize).min(ops.len() - 1);
    &ops[idx]
}

fn is_ident_byte(b: u8) -> bool { b.is_ascii_alphanumeric() || b == b'_' }

/// Split a source line into (code, comment) at the first `//` that is not inside a
/// string literal. Logic operators edit only the code half so they never "succeed"
/// by rewriting a comment while leaving the behaviour unchanged.
fn split_code_comment(line: &str) -> (String, String) {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'/' if !in_str && bytes[i + 1] == b'/' => {
                return (line[..i].to_string(), line[i..].to_string());
            }
            _ => {}
        }
        i += 1;
    }
    (line.to_string(), String::new())
}

/// Replace the first WHOLE-WORD occurrence of `from` with `to` (so `true` inside
/// `always_true` is not matched). Returns the new string, or None if no standalone
/// occurrence exists.
fn replace_word_once(s: &str, from: &str, to: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(rel) = s[i..].find(from) {
        let start = i + rel;
        let end = start + from.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return Some(format!("{}{}{}", &s[..start], to, &s[end..]));
        }
        i = start + 1;
        if i >= s.len() { break; }
    }
    None
}

/// Apply a named edit operator to `lines` at index `li`. Returns whether it
/// changed anything.
fn apply_op(op: &str, lines: &mut Vec<String>, li: usize, aggression: f64, rng: &mut impl Rng) -> bool {
    if lines.is_empty() { return false; }
    let aggression = aggression.clamp(0.0, 1.0);
    let li = li.min(lines.len() - 1);
    match op {
        "flip-op" => {
            let (code, comment) = split_code_comment(&lines[li]);
            let f = flip_ops(&code);
            if f != code { lines[li] = format!("{f}{comment}"); true } else { false }
        }
        "flip-bool" => {
            // Flip a boolean *literal* in the CODE — as a whole word, and never in a
            // comment. Without the code/comment split, a comment like `// returns
            // true` gets flipped, the code is untouched, and the test never passes.
            let (code, comment) = split_code_comment(&lines[li]);
            if let Some(new) = replace_word_once(&code, "true", "false")
                .or_else(|| replace_word_once(&code, "false", "true")) {
                lines[li] = format!("{new}{comment}"); true
            } else { false }
        }
        "tweak-num" => {
            // Nudge the first numeric *literal* by ±1, in the code only, skipping
            // digit runs that are part of an identifier or type (`i64`, `u32`).
            let (code, comment) = split_code_comment(&lines[li]);
            let bytes = code.as_bytes();
            let mut s = 0;
            while s < bytes.len() {
                if bytes[s].is_ascii_digit() {
                    let prev_is_ident = s > 0 && is_ident_byte(bytes[s - 1]);
                    let mut e = s;
                    while e < bytes.len() && bytes[e].is_ascii_digit() { e += 1; }
                    if !prev_is_ident {
                        if let Ok(n) = code[s..e].parse::<i64>() {
                            let delta = if rng.gen::<bool>() { 1 } else { -1 };
                            lines[li] = format!("{}{}{}{}", &code[..s], n + delta, &code[e..], comment);
                            return true;
                        }
                    }
                    s = e;
                } else {
                    s += 1;
                }
            }
            false
        }
        "comment-out" => {
            let indent: String = lines[li].chars().take_while(|c| c.is_whitespace()).collect();
            let body = lines[li].trim_start();
            if body.starts_with("//") { return false; }
            lines[li] = format!("{indent}// {body}");
            true
        }
        "uncomment" => {
            let indent: String = lines[li].chars().take_while(|c| c.is_whitespace()).collect();
            let body = lines[li].trim_start();
            if let Some(rest) = body.strip_prefix("//") {
                lines[li] = format!("{indent}{}", rest.trim_start());
                true
            } else { false }
        }
        "dup-line" => { let dup = lines[li].clone(); lines.insert(li, dup); true }
        "delete-line" => { lines.remove(li); true }
        "swap-lines" => {
            if li + 1 < lines.len() { lines.swap(li, li + 1); true } else { false }
        }
        "splice-line" => {
            // copy a random *other* line into position li (intra-file recombination)
            if lines.len() < 2 { return false; }
            let mut src = rng.gen_range(0..lines.len());
            while src == li { src = rng.gen_range(0..lines.len()); }
            let donor = lines[src].clone();
            lines.insert(li, donor);
            true
        }
        "math-op" => {
            // wrap the first integer literal in an arithmetic expression; magnitude
            // grows with aggression (timid: n+1, aggressive: n*K)
            let cs: Vec<char> = lines[li].chars().collect();
            let mut s = 0;
            while s < cs.len() {
                if cs[s].is_ascii_digit() && (s == 0 || !cs[s - 1].is_ascii_alphanumeric()) {
                    let mut e = s;
                    while e < cs.len() && cs[e].is_ascii_digit() { e += 1; }
                    let num: String = cs[s..e].iter().collect();
                    let pre: String = cs[..s].iter().collect();
                    let post: String = cs[e..].iter().collect();
                    let k = 2 + (aggression * 8.0).round() as i64; // 2..10
                    lines[li] = format!("{pre}({num} * {k}){post}");
                    return true;
                }
                s += 1;
            }
            false
        }
        "string-op" => {
            // mutate the contents of the first "..." literal: reverse it, and on high
            // aggression also repeat it
            let line = lines[li].clone();
            if let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) {
                if b > a + 1 {
                    let inner: String = line[a + 1..b].chars().rev().collect();
                    let inner = if aggression > 0.6 { format!("{inner}{inner}") } else { inner };
                    lines[li] = format!("{}\"{}\"{}", &line[..a], inner, &line[b + 1..]);
                    return true;
                }
            }
            false
        }
        "reorder" => {
            // reverse a block of lines starting at li; block size scales with aggression
            let max = (2.0 + aggression * 8.0).round() as usize; // 2..10
            let end = (li + max).min(lines.len());
            if end > li + 1 { lines[li..end].reverse(); true } else { false }
        }
        "wrap-loop" => {
            // wrap the line in a (trivially-bounded) loop — "loop building"
            let indent: String = lines[li].chars().take_while(|c| c.is_whitespace()).collect();
            let body = lines[li].clone();
            let reps = 1 + (aggression * 4.0).round() as i64; // 1..5
            lines[li] = format!("{indent}for _i in 0..{reps} {{");
            lines.insert(li + 1, format!("    {body}"));
            lines.insert(li + 2, format!("{indent}}}"));
            true
        }
        "insert-rand" => {
            // insert a line of file-flavored random text via an order-1 Markov chain
            // over the file's own characters (a degenerate "RNN"); length scales with
            // aggression
            let n = (4.0 + aggression * 44.0).round() as usize; // 4..48 chars
            let source: String = lines.join("\n");
            let gen = markov_chars(&source, n, rng);
            let indent: String = lines[li].chars().take_while(|c| c.is_whitespace()).collect();
            lines.insert(li, format!("{indent}// {gen}"));
            true
        }
        _ => false, // no-op
    }
}

/// Order-1 Markov chain over the characters of `source`: pick a start char, then
/// repeatedly sample the next char from those that followed the current one in the
/// source. A degenerate one-step "RNN" — output is gibberish but flavored like the
/// file (its identifiers, operators, spacing), not uniform noise.
fn markov_chars(source: &str, n: usize, rng: &mut impl Rng) -> String {
    use std::collections::HashMap;
    let chars: Vec<char> = source.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if chars.len() < 2 { return "x".repeat(n.min(8)); }
    let mut next: HashMap<char, Vec<char>> = HashMap::new();
    for w in chars.windows(2) { next.entry(w[0]).or_default().push(w[1]); }
    let mut out = String::new();
    let mut cur = chars[rng.gen_range(0..chars.len())];
    for _ in 0..n {
        out.push(cur);
        cur = match next.get(&cur) {
            Some(v) if !v.is_empty() => v[rng.gen_range(0..v.len())],
            _ => chars[rng.gen_range(0..chars.len())],
        };
    }
    out
}

fn mutate_source(dir: &Path, z: f64, ops: &[String], aggression: f64, rng: &mut impl Rng) -> bool {
    let op = op_for(z, ops).to_string();
    mutate_with_op(dir, &op, aggression, rng)
}

// ---- Fault localization: a learned-ish proposal distribution over edit sites ----
//
// Uniform-random line selection is fine on a 4-line toy but hopeless on an 87k-line
// repo. A failing test names the behaviour it expects; the code that implements
// that behaviour usually shares vocabulary with the test (the test `t_answer`
// calls `answer()`). So we build a token "query" from the FAILING tests and bias
// mutation toward source lines whose tokens overlap the query. This is the lexical
// (association) baseline of the test→code latent map; swapping token-overlap for
// word2vec cosine similarity upgrades it to the learned-embedding version without
// touching the search. ε-greedy keeps a fraction of steps uniform, so the prior
// can only change *efficiency*, never correctness — a bad guess just wastes a step.

const LOCALIZE_EPSILON: f64 = 0.30; // fraction of steps that ignore the prior and explore

/// Names of tests that FAILED, from cargo/pytest output (e.g. `tests::t_answer`).
fn failing_test_names(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        // cargo: `test tests::t_answer ... FAILED`
        if let Some(rest) = t.strip_prefix("test ") {
            if rest.ends_with("FAILED") || rest.contains(" ... FAILED") {
                if let Some(name) = rest.split(" ... ").next() {
                    out.push(name.trim().to_string());
                }
            }
        }
        // pytest: `path::test_foo FAILED` or `FAILED path::test_foo`
        if t.contains("FAILED") && (t.contains("::") || t.contains("test")) {
            for tok in t.split_whitespace() {
                if tok.contains("::") && !tok.contains("FAILED") { out.push(tok.to_string()); }
            }
        }
    }
    out.sort(); out.dedup();
    out
}

fn is_stop_token(s: &str) -> bool {
    matches!(s, "t" | "fn" | "let" | "mut" | "self" | "super" | "use" | "mod" | "pub"
        | "test" | "tests" | "assert" | "eq" | "ne" | "it" | "should" | "the" | "of" | "to"
        | "is" | "in" | "as" | "ok" | "err" | "new" | "f00" | "foo" | "bar" | "baz")
}

/// Split an identifier/path into lowercased word tokens on snake_case, camelCase,
/// `::`, and non-alphanumerics; drop noise/stopwords and very short tokens.
fn ident_tokens(s: &str, out: &mut std::collections::HashSet<String>) {
    let mut cur = String::new();
    let mut prev_lower = false;
    let flush = |cur: &mut String, out: &mut std::collections::HashSet<String>| {
        if cur.len() >= 3 && !is_stop_token(cur) { out.insert(std::mem::take(cur)); }
        else { cur.clear(); }
    };
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower { flush(&mut cur, out); } // camelCase boundary
            cur.push(ch.to_ascii_lowercase());
            prev_lower = ch.is_lowercase() || ch.is_numeric();
        } else {
            flush(&mut cur, out); // _, ::, punctuation
            prev_lower = false;
        }
    }
    flush(&mut cur, out);
}

/// Build the localization query: tokens from the failing test names PLUS the
/// identifiers referenced inside each failing test's body (the symbols under test).
fn localization_query(work: &Path, failing: &[String]) -> std::collections::HashSet<String> {
    let mut q = std::collections::HashSet::new();
    for name in failing {
        ident_tokens(name, &mut q);
    }
    if failing.is_empty() { return q; }

    // Index every source line once (small repos) so we can look up function bodies
    // by name during call-graph expansion.
    let files = collect_source_files(work);
    let mut docs: Vec<Vec<String>> = Vec::new();
    for f in &files {
        if let Ok(text) = std::fs::read_to_string(f) {
            docs.push(text.lines().map(|s| s.to_string()).collect());
        }
    }

    // Bounded breadth-first walk of the call graph: start at the failing tests,
    // and at each hop add the bodies (and the names of the functions they CALL) of
    // everything reached. A test's bug often lives in a callee, not the test itself
    // or the top-level function — this is what makes coupled/indirect defects
    // localizable instead of invisible.
    const MAX_HOPS: usize = 3;
    let mut frontier: std::collections::HashSet<String> = failing.iter()
        .map(|n| n.rsplit("::").next().unwrap_or(n).to_string()).collect();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 0..MAX_HOPS {
        if frontier.is_empty() { break; }
        let mut next: std::collections::HashSet<String> = std::collections::HashSet::new();
        for name in frontier.drain() {
            if !visited.insert(name.clone()) { continue; }
            for lines in &docs {
                for (i, line) in lines.iter().enumerate() {
                    if line.contains(&format!("fn {name}")) || line.contains(&format!("fn {name}<"))
                        || line.contains(&format!("fn {name}(")) {
                        let mut depth = 0i32; let mut started = false; let mut j = i;
                        while j < lines.len() {
                            ident_tokens(&lines[j], &mut q);
                            for callee in called_idents(&lines[j]) { next.insert(callee); }
                            for ch in lines[j].chars() {
                                if ch == '{' { depth += 1; started = true; }
                                else if ch == '}' { depth -= 1; }
                            }
                            if started && depth <= 0 { break; }
                            j += 1;
                            if j - i > 120 { break; }
                        }
                    }
                }
            }
        }
        frontier = next;
    }
    q
}

/// Identifiers that look like function CALLS on a line: `name(` (excluding obvious
/// keywords). Used to walk the call graph for fault localization.
fn called_idents(line: &str) -> Vec<String> {
    let code = strip_comment(line);
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' && i > 0 {
            // read the identifier ending just before '('
            let mut s = i;
            while s > 0 && is_ident_byte(bytes[s - 1]) { s -= 1; }
            if s < i {
                let name = &code[s..i];
                if name.len() >= 3 && !matches!(name, "if" | "for" | "while" | "match" | "fn" | "let" | "return" | "assert") {
                    out.push(name.to_string());
                }
            }
        }
        i += 1;
    }
    out
}

/// Combined edit-site weight for a line: direct lexical overlap with the failing
/// tests (weight 1 each) PLUS learned affinity (a line matching a code token the
/// model has *learned* is associated with the failing tests' vocabulary gets extra
/// weight, even with no direct overlap). This is where the static prior and the
/// learned model fuse.
fn line_weight(line: &str, query: &std::collections::HashSet<String>,
               learned: &HashMap<String, f32>) -> f32 {
    let toks = line_tokens(line);
    let lex = toks.intersection(query).count() as f32;
    let lrn: f32 = toks.iter().filter_map(|t| learned.get(t)).sum();
    lex + lrn
}

/// Mutate a line chosen by the localization prior (lexical + learned), returning
/// the edited (file, line_index) so the caller can reinforce the model when the
/// edit turns out to repair a test. ε-greedy: a fraction of steps ignore the prior.
/// Which enabled ops can plausibly make a *meaningful* change to this line, with a
/// preference weight. Logic-fixing ops are matched to the line's content — flip-op
/// where there's a comparison/arithmetic operator, tweak-num where there's a
/// number, flip-bool where there's a boolean literal, string-op where there's a
/// string. Structural ops (dup/comment/reorder/…) stay available but heavily
/// down-weighted, since they rarely repair behaviour. Matching the tool to the site
/// is what lets a localized climb fix MULTIPLE defects instead of stalling after the
/// one its random op happened to suit.
fn op_weights_for_line(line: &str, ops: &[String]) -> Vec<(String, f64)> {
    let l = strip_comment(line);
    // Blank out `->` first: the return arrow's `-`/`>` are not arithmetic/comparison
    // operators, and letting them pull flip-op/math-op onto a line (e.g. a boolean
    // getter `fn f() -> bool { true }`) wastes the edit and crowds out flip-bool.
    let l = l.replace("->", "  ");
    let toks = line_tokens(&l);
    let has_digit = l.chars().any(|c| c.is_ascii_digit());
    let has_bool = toks.contains("true") || toks.contains("false");
    let has_cmp = ["==", "!=", " <= ", " >= ", " < ", " > "].iter().any(|s| l.contains(s));
    let has_arith = [" + ", " - ", " * ", " / ", " % "].iter().any(|s| l.contains(s))
        || ["+=", "-=", "*=", "/="].iter().any(|s| l.contains(s));
    let has_str = l.contains('"');
    let mut out: Vec<(String, f64)> = Vec::new();
    for op in ops {
        let w = match op.as_str() {
            "flip-op" => if has_cmp || has_arith { 3.0 } else { 0.0 },
            "flip-bool" => if has_bool { 3.0 } else { 0.0 },
            "tweak-num" => if has_digit { 3.0 } else { 0.0 },
            "math-op" => if has_arith || has_digit { 2.0 } else { 0.0 },
            "string-op" => if has_str { 2.0 } else { 0.0 },
            "no-op" => 0.0,
            // structural ops: low baseline so a targeted site still occasionally
            // restructures (escaping local optima) without wasting most attempts.
            _ => 0.25,
        };
        if w > 0.0 { out.push((op.clone(), w)); }
    }
    if out.is_empty() {
        out = ops.iter().filter(|o| o.as_str() != "no-op").map(|o| (o.clone(), 1.0)).collect();
    }
    out
}

/// Sample an operator suited to `line`'s content.
fn pick_op_for_line(line: &str, ops: &[String], rng: &mut impl Rng) -> String {
    let weights = op_weights_for_line(line, ops);
    let total: f64 = weights.iter().map(|(_, w)| w).sum();
    if total <= 0.0 { return ops[rng.gen_range(0..ops.len())].clone(); }
    let mut pick = rng.gen::<f64>() * total;
    for (op, w) in &weights {
        if pick < *w { return op.clone(); }
        pick -= *w;
    }
    weights.last().map(|(o, _)| o.clone()).unwrap_or_else(|| "no-op".into())
}

/// Apply `op` to a specific file at a specific line index.
fn apply_op_at(f: &Path, li: usize, op: &str, aggression: f64, rng: &mut impl Rng) -> bool {
    let content = match std::fs::read_to_string(f) { Ok(c) => c, Err(_) => return false };
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    if lines.is_empty() { return false; }
    let li = li.min(lines.len() - 1);
    let changed = apply_op(op, &mut lines, li, aggression, rng);
    if changed { let _ = atomic_write(f, &lines.join("\n")); }
    changed
}

/// Apply a bundle of `k` edits at `k` DISTINCT hot lines (weighted by the lexical +
/// learned localization score), each with its content-matched operator. The directed
/// version of a multi-edit kick: instead of k independent random picks that can
/// collide on one line or scatter into cold code, the bundle spreads across the
/// highest-affinity DISTINCT sites — the shape a coordinated repair needs (flip the
/// encoder AND the matching decoder). Edits apply highest-line-first within a file so
/// a line-count change can't invalidate a sibling index. Returns the edited sites.
/// Line indices that belong to TEST code: inside a `#[cfg(test)]` block, or part of
/// a test-attributed (`#[test]`, …) item anywhere. Used to keep localization out of
/// the tests — the defect being repaired is never in the test itself, and editing a
/// test only gets the whole attempt reverted by the tripwire.
fn test_region_lines(text: &str) -> std::collections::HashSet<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let mut marked = std::collections::HashSet::new();
    let is_test_attr = |t: &str| -> bool {
        t.starts_with("#[test]") || t.starts_with("#[bench]") || t.starts_with("#[tokio::test")
            || t.starts_with("#[async_std::test") || t.starts_with("#[test_case")
            || t.starts_with("#[rstest") || (t.starts_with("#[") && t.contains("::test"))
    };
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let starts_block = t.starts_with("#[cfg(test)]") || is_test_attr(t);
        if starts_block {
            // mark from here through the end of the following brace-matched item
            let mut depth = 0i32; let mut started = false; let mut j = i;
            loop {
                marked.insert(j);
                for ch in lines[j].chars() {
                    if ch == '{' { depth += 1; started = true; }
                    else if ch == '}' { depth -= 1; }
                }
                if started && depth <= 0 { break; }
                if !started && lines[j].trim_end().ends_with(';') { break; }
                j += 1;
                if j >= lines.len() || j - i > 4000 { break; }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    marked
}

fn mutate_bundle(dir: &Path, ops: &[String], aggression: f64, rng: &mut impl Rng,
                 query: &std::collections::HashSet<String>, learned: &HashMap<String, f32>,
                 k: usize, skip_tests: bool) -> Vec<(PathBuf, usize)> {
    if ops.is_empty() { return vec![]; }
    let files = collect_source_files(dir);
    if files.is_empty() { return vec![]; }

    let have_prior = !query.is_empty() || !learned.is_empty();
    let mut hot: Vec<(usize, usize, f64)> = Vec::new();
    if have_prior {
        for (fi, f) in files.iter().enumerate() {
            if let Ok(text) = std::fs::read_to_string(f) {
                let tlines = if skip_tests { test_region_lines(&text) } else { Default::default() };
                for (li, line) in text.lines().enumerate() {
                    if tlines.contains(&li) { continue; } // never localize into test code
                    let w = line_weight(line, query, learned);
                    if w > 0.0 { hot.push((fi, li, w as f64)); }
                }
            }
        }
    }

    let mut used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut sites: Vec<(usize, usize)> = Vec::new();
    let mut attempts = 0;
    while sites.len() < k && attempts < k * 8 {
        attempts += 1;
        let explore = !have_prior || hot.is_empty() || rng.gen::<f64>() < LOCALIZE_EPSILON;
        let pick = if !explore {
            let total: f64 = hot.iter().filter(|(fi, li, _)| !used.contains(&(*fi, *li)))
                .map(|(_, _, w)| *w).sum();
            if total <= 0.0 { None } else {
                let mut r = rng.gen::<f64>() * total;
                hot.iter().filter(|(fi, li, _)| !used.contains(&(*fi, *li)))
                    .find_map(|(fi, li, w)| { if r < *w { Some((*fi, *li)) } else { r -= *w; None } })
            }
        } else {
            let fi = rng.gen_range(0..files.len());
            std::fs::read_to_string(&files[fi]).ok().and_then(|t| {
                let lines: Vec<&str> = t.lines().collect();
                if lines.is_empty() { return None; }
                let tlines = if skip_tests { test_region_lines(&t) } else { Default::default() };
                // try a few random lines to land on non-test code
                for _ in 0..6 {
                    let li = rng.gen_range(0..lines.len());
                    if !tlines.contains(&li) { return Some((fi, li)); }
                }
                None
            })
        };
        match pick {
            Some(s) if used.insert(s) => sites.push(s),
            _ => {}
        }
    }

    sites.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1))); // highest line first per file
    let mut edited = Vec::new();
    for (fi, li) in sites {
        let f = &files[fi];
        let line = std::fs::read_to_string(f).ok()
            .and_then(|t| t.lines().nth(li).map(|s| s.to_string())).unwrap_or_default();
        let op = pick_op_for_line(&line, ops, rng);
        if apply_op_at(f, li, &op, aggression, rng) { edited.push((f.clone(), li)); }
    }
    edited
}

// ---- Online, persistent fault-localization memory (the "learning" layer) --------
//
// The static lexical query (above) is rebuilt every run and only knows DIRECT word
// overlap. This model is the mature version: it LEARNS, from repair outcomes across
// every run and every workspace, which CODE vocabulary tends to fix tests carrying
// a given TEST vocabulary — e.g. "tests saying `parse` got repaired near code
// saying `tokenize`/`lex`". It's keyed on vocabulary, never on file paths, so the
// knowledge transfers between codebases. The swarm builds it automatically while
// running; there is no per-app pipeline to configure. Persisted as a flat TSV in
// the registry data dir so it accumulates across sessions and apps.

#[derive(Default)]
struct AffinityModel {
    pairs: HashMap<String, HashMap<String, f32>>, // test_token -> (code_token -> weight)
    updates: u64,
}

impl AffinityModel {
    /// A repair: tests carrying `test_tokens` were fixed by an edit on a line whose
    /// vocabulary is `code_tokens`. Reinforce every (test, code) association.
    fn reinforce(&mut self, test_tokens: &std::collections::HashSet<String>,
                 code_tokens: &std::collections::HashSet<String>, w: f32) {
        if test_tokens.is_empty() || code_tokens.is_empty() { return; }
        for tt in test_tokens {
            let row = self.pairs.entry(tt.clone()).or_default();
            for ct in code_tokens {
                if ct == tt { continue; } // self-association carries no localization signal
                *row.entry(ct.clone()).or_insert(0.0) += w;
            }
        }
        self.updates += 1;
    }

    /// Given failing-test query tokens, return learned code tokens with normalized
    /// weights (a few strong associations dominate; unknown tokens contribute none).
    fn expand(&self, query: &std::collections::HashSet<String>) -> HashMap<String, f32> {
        let mut acc: HashMap<String, f32> = HashMap::new();
        for tt in query {
            if let Some(row) = self.pairs.get(tt) {
                let sum: f32 = row.values().sum();
                if sum <= 0.0 { continue; }
                for (ct, w) in row {
                    *acc.entry(ct.clone()).or_insert(0.0) += w / sum;
                }
            }
        }
        acc
    }

    fn to_tsv(&self) -> String {
        let mut s = String::new();
        for (tt, row) in &self.pairs {
            for (ct, w) in row {
                s.push_str(&format!("{tt}\t{ct}\t{w}\n"));
            }
        }
        s
    }

    fn from_tsv(text: &str) -> Self {
        let mut m = AffinityModel::default();
        for line in text.lines() {
            let mut it = line.split('\t');
            if let (Some(tt), Some(ct), Some(w)) = (it.next(), it.next(), it.next()) {
                if let Ok(w) = w.trim().parse::<f32>() {
                    m.pairs.entry(tt.to_string()).or_default().insert(ct.to_string(), w);
                }
            }
        }
        m
    }
}

fn affinity_path() -> PathBuf { crate::registry::data_root().join("localization.tsv") }

/// Process-global model: loaded from disk on first use, shared (Mutex) across the
/// parallel bot threads, saved at run end. A global keeps the swarm's learning
/// automatic — no extra argument threaded through every evaluation function.
fn localizer() -> &'static Mutex<AffinityModel> {
    static LOCALIZER: std::sync::OnceLock<Mutex<AffinityModel>> = std::sync::OnceLock::new();
    LOCALIZER.get_or_init(|| {
        let m = std::fs::read_to_string(affinity_path())
            .map(|s| AffinityModel::from_tsv(&s))
            .unwrap_or_default();
        Mutex::new(m)
    })
}

fn save_localizer() {
    if let Ok(m) = localizer().lock() {
        if m.updates == 0 && m.pairs.is_empty() { return; }
        let _ = std::fs::create_dir_all(crate::registry::data_root());
        let _ = atomic_write(&affinity_path(), &m.to_tsv());
    }
}

/// Tokens of a single line (used to learn what code vocabulary fixed a test).
fn line_tokens(line: &str) -> std::collections::HashSet<String> {
    let mut s = std::collections::HashSet::new();
    ident_tokens(line, &mut s);
    s
}

/// Drop trailing line comments so the learned vocabulary reflects code, not prose.
fn strip_comment(line: &str) -> String {
    let mut out = line;
    for marker in ["//", "/*", "#"] {
        if let Some(p) = out.find(marker) { out = &out[..p]; }
    }
    out.to_string()
}


fn mutate_with_op(dir: &Path, op: &str, aggression: f64, rng: &mut impl Rng) -> bool {
    let files = collect_source_files(dir);
    if files.is_empty() {
        return false;
    }
    let f = files[rng.gen_range(0..files.len())].clone();
    let content = match std::fs::read_to_string(&f) { Ok(c) => c, Err(_) => return false };
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    if lines.is_empty() {
        return false;
    }
    let li = rng.gen_range(0..lines.len());
    let changed = apply_op(op, &mut lines, li, aggression, rng);
    if changed {
        let _ = atomic_write(&f, &lines.join("\n"));
    }
    changed
}

/// Parse pytest-style output for pass/total counts; fall back to exit code.
fn parse_test_output(stdout: &str, stderr: &str, code: Option<i32>) -> (u32, u32) {
    let text = format!("{stdout}\n{stderr}");
    let toks: Vec<&str> = text.split_whitespace().collect();
    let (mut passed, mut failed, mut errors, mut found) = (0u32, 0u32, 0u32, false);
    for w in toks.windows(2) {
        if let Ok(n) = w[0].parse::<u32>() {
            match w[1].trim_end_matches(|c| c == ',' || c == '.' || c == ';') {
                // accumulate, not assign: cargo workspaces print one summary per crate
                "passed" => { passed += n; found = true; }
                "failed" => { failed += n; found = true; }
                "error" | "errors" => { errors += n; found = true; }
                _ => {}
            }
        }
    }
    if found {
        (passed, (passed + failed + errors).max(1))
    } else {
        match code { Some(0) => (1, 1), _ => (0, 1) }
    }
}

/// Test-only thin wrapper used by the unit tests (the real path uses run_capture).
#[cfg(test)]
fn run_tests_local(dir: &Path, test_cmd: &str) -> (u32, u32) {
    let (out, code) = run_capture(BackendKind::Local, "", dir, test_cmd,
        RunOpts { cache: false, timeout_secs: 0, max_file_mb: 100, sandbox: false, slot: 0, allowed: vec![] });
    parse_test_output(&out, "", code)
}

/// The worker's attempt at an edit. With the `llamacpp` feature it asks the active
/// local model to rewrite a file to pass the tests, falling back to the baseline
/// mutation if there's no model or the edit fails. Without the feature it is the
/// purpose-typed mutation baseline.
#[cfg(feature = "llamacpp")]
fn worker_attempt(dir: &Path, z: f64, test_cmd: &str, ops: &[String], aggression: f64, rng: &mut impl Rng) -> bool {
    match llm_edit(dir, test_cmd) {
        Ok(true) => true,
        _ => mutate_source(dir, z, ops, aggression, rng),
    }
}

#[cfg(not(feature = "llamacpp"))]
fn worker_attempt(dir: &Path, z: f64, _test_cmd: &str, ops: &[String], aggression: f64, rng: &mut impl Rng) -> bool {
    mutate_source(dir, z, ops, aggression, rng)
}

/// LLM editor: capture the current test output, hand the model one source file
/// plus the failures, and write back its rewrite. Whole-file rewrite is the v1
/// strategy; a search/replace or unified-diff protocol is the natural follow-up.
#[cfg(feature = "llamacpp")]
fn llm_edit(dir: &Path, test_cmd: &str) -> Result<bool, String> {
    let model = crate::llama_backend::active_model().ok_or("no active local model")?;
    let output = {
        let o = std::process::Command::new("sh").arg("-c").arg(test_cmd).current_dir(dir).output();
        o.map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
            .unwrap_or_default()
    };
    let files = collect_source_files(dir);
    let target = files.into_iter().next().ok_or("no source files to edit")?;
    let src = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
    let prompt = format!(
        "You are editing a single code file so its test suite passes.\n\
         === TEST OUTPUT ===\n{output}\n\
         === CURRENT FILE: {} ===\n{src}\n\
         === INSTRUCTIONS ===\n\
         Rewrite the ENTIRE file so the tests pass. Output only the new file \
         contents — no markdown fences, no explanation.\n",
        target.display()
    );
    let params = crate::llama_backend::GenParams::default();
    let reply = crate::llama_backend::generate(&model, &prompt, &params)?;
    let new_src = strip_code_fences(&reply);
    if new_src.trim().is_empty() {
        return Ok(false);
    }
    atomic_write(&target, &new_src).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Strip a leading/trailing ``` fence (with optional language tag) if the model
/// wrapped its answer in one.
#[cfg(feature = "llamacpp")]
fn strip_code_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let body = rest.splitn(2, '\n').nth(1).unwrap_or("");
        return body.strip_suffix("```").unwrap_or(body).trim_end().to_string();
    }
    t.to_string()
}

// ===== Multi build-system detection & scoring ==============================
//
// "auto" mode (the default test command) detects the build/test systems present
// in the worktree and runs each, aggregating reward = total_passed / total_tests.
// This handles a mixed repo — e.g. a CMakeLists at the root next to a Cargo
// workspace — by running both and summing. Build failures count as failing tests
// (they contribute 0 / >=1), so they drag the score down rather than being free.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum BuildSystem { Cargo, Pytest, Ctest, Meson, Npm, GoTest, Maven }

impl BuildSystem {
    fn label(self) -> &'static str {
        match self {
            BuildSystem::Cargo => "cargo", BuildSystem::Pytest => "pytest",
            BuildSystem::Ctest => "ctest", BuildSystem::Meson => "meson",
            BuildSystem::Npm => "npm", BuildSystem::GoTest => "go",
            BuildSystem::Maven => "maven",
        }
    }
    /// Shell command (run from the system's root dir) that builds+tests.
    fn command(self, cache: bool, features: &str, flags: &str) -> String {
        let launcher = if cache { " -DCMAKE_C_COMPILER_LAUNCHER=ccache -DCMAKE_CXX_COMPILER_LAUNCHER=ccache" } else { "" };
        let feat = if features.trim().is_empty() { String::new() } else { format!(" --features {}", features.trim()) };
        let extra = if flags.trim().is_empty() { String::new() } else { format!(" {}", flags.trim()) };
        match self {
            BuildSystem::Cargo => format!("cargo test --workspace{feat}{extra} 2>&1"),
            BuildSystem::Pytest => format!("python3 -m pytest -q{extra} 2>&1"),
            BuildSystem::Ctest => format!(
                "cmake -S . -B build{launcher} >/dev/null 2>&1 && cmake --build build >/dev/null 2>&1 && ctest --test-dir build 2>&1"),
            BuildSystem::Meson => "(meson setup build || meson setup --reconfigure build) >/dev/null 2>&1; meson test -C build 2>&1".into(),
            BuildSystem::Npm => format!("npm test --silent{extra} 2>&1"),
            BuildSystem::GoTest => format!("go test ./...{extra} 2>&1"),
            // Maven Surefire prints a parseable "Tests run: N, Failures: F, …" summary
            // to the console. -B batch mode, -q quiet, offline to avoid network stalls.
            BuildSystem::Maven => format!("mvn -B -q -o test{extra} 2>&1"),
        }
    }
    /// Parse this system's output into (passed, total).
    fn parse(self, out: &str, code: Option<i32>) -> (u32, u32) {
        match self {
            BuildSystem::Ctest => parse_ctest(out, code),
            BuildSystem::Meson => parse_meson(out, code),
            BuildSystem::Npm => parse_jest(out, code),
            BuildSystem::Maven => parse_surefire(out, code),
            // cargo + pytest share the "N passed / N failed" shape (parser sums)
            _ => parse_test_output(out, "", code),
        }
    }
}

/// Parse Maven Surefire / Failsafe summary lines: `Tests run: 12, Failures: 1,
/// Errors: 0, Skipped: 2`. A multi-module build prints one per module; sum them.
/// passed = run − failures − errors − skipped; total = run − skipped (skipped tests
/// neither pass nor fail, so they shouldn't dilute the score in either direction).
fn parse_surefire(out: &str, code: Option<i32>) -> (u32, u32) {
    let (mut run, mut fail, mut err, mut skip) = (0u32, 0u32, 0u32, 0u32);
    let mut found = false;
    let field = |line: &str, key: &str| -> u32 {
        line.find(key).and_then(|p| {
            line[p + key.len()..].trim_start().split(|c: char| !c.is_ascii_digit())
                .next().and_then(|n| n.parse::<u32>().ok())
        }).unwrap_or(0)
    };
    for line in out.lines() {
        if line.contains("Tests run:") && (line.contains("Failures:") || line.contains("Error")) {
            // skip the per-test failure echo lines; the summary has Failures+Errors+Skipped
            run += field(line, "Tests run:");
            fail += field(line, "Failures:");
            err += field(line, "Errors:");
            skip += field(line, "Skipped:");
            found = true;
        }
    }
    if !found {
        return match code { Some(0) => (1, 1), _ => (0, 1) };
    }
    let total = run.saturating_sub(skip).max(1);
    let passed = run.saturating_sub(fail).saturating_sub(err).saturating_sub(skip);
    (passed, total)
}

/// Extract the identifiers of *failing* tests from a runner's output. Failing lines
/// are the most distinctive and stable across runners, and they're all we need for
/// differential scoring (fixed = baseline-failing − now-failing, broke = the
/// reverse). Patterns don't collide across runners, so we scan for all of them.
/// Best-effort: unrecognised formats yield nothing and the caller falls back to
/// plain pass-fraction scoring.
fn parse_failing_names(out: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |n: String| { if !n.is_empty() && !names.contains(&n) { names.push(n); } };
    for line in out.lines() {
        let l = line.trim();
        // cargo / pytest-verbose: `test path::name ... FAILED`
        if let Some(rest) = l.strip_prefix("test ") {
            if !rest.starts_with("result:") && (rest.ends_with("FAILED") || rest.contains("... FAILED")) {
                if let Some(name) = rest.split_whitespace().next() { push(name.to_string()); }
            }
        }
        // go: `--- FAIL: TestName (0.00s)`
        if let Some(rest) = l.strip_prefix("--- FAIL:") {
            if let Some(name) = rest.split_whitespace().next() { push(format!("go::{name}")); }
        }
        // ctest: `1/3 Test #2: name ........***Failed` (or `Failed`)
        if l.contains("Test #") && (l.contains("Failed") || l.contains("***Failed") || l.contains("***Exception")) {
            if let Some(p) = l.find("Test #") {
                let after = &l[p..];
                if let Some(colon) = after.find(':') {
                    let name = after[colon + 1..].trim().split_whitespace().next().unwrap_or("").trim_end_matches('.');
                    push(format!("ctest::{name}"));
                }
            }
        }
        // pytest non-verbose summary: `FAILED path::test_name - reason`
        if let Some(rest) = l.strip_prefix("FAILED ") {
            if let Some(name) = rest.split_whitespace().next() { push(name.to_string()); }
        }
        // jest: `✕ test name` or `● test name › ...`
        if let Some(rest) = l.strip_prefix("✕ ").or_else(|| l.strip_prefix("× ")) {
            push(format!("jest::{}", rest.trim()));
        }
    }
    names
}

/// Differential reward: centre at 0.5 (no net change vs. the pristine baseline),
/// reward fixing previously-failing tests, and punish *breaking* previously-passing
/// ones harder (asymmetric λ > 1) so regressions are strongly selected against.
/// This is denser signal than pass-fraction: it distinguishes "fixed 1, broke 0"
/// from "fixed 1, broke 1", which have the same pass count.
fn differential_reward(baseline_failing: &[String], now_failing: &[String], lambda: f64) -> f64 {
    let now: std::collections::BTreeSet<&String> = now_failing.iter().collect();
    let base: std::collections::BTreeSet<&String> = baseline_failing.iter().collect();
    let fixed = base.iter().filter(|n| !now.contains(*n)).count() as f64; // were failing, now pass
    let broke = now.iter().filter(|n| !base.contains(*n)).count() as f64; // were passing, now fail
    let denom = (baseline_failing.len() as f64).max(1.0);
    (0.5 + 0.5 * (fixed - lambda * broke) / denom).clamp(0.0, 1.0)
}

fn parse_ctest(out: &str, code: Option<i32>) -> (u32, u32) {
    // e.g. "67% tests passed, 1 tests failed out of 3"
    let toks: Vec<&str> = out.split_whitespace().collect();
    let (mut total, mut failed, mut found) = (0u32, 0u32, false);
    for i in 0..toks.len() {
        if toks[i] == "of" && i + 1 < toks.len() {
            if let Ok(n) = toks[i + 1].trim_end_matches(|c: char| !c.is_ascii_digit()).parse::<u32>() {
                total = n; found = true;
            }
        }
        if toks[i] == "failed" && i >= 2 {
            if let Ok(n) = toks[i - 2].parse::<u32>() { failed = n; }
        }
    }
    if found { (total.saturating_sub(failed), total.max(1)) }
    else { match code { Some(0) => (1, 1), _ => (0, 1) } }
}

fn parse_meson(out: &str, code: Option<i32>) -> (u32, u32) {
    // lines: "Ok: N", "Fail: M", "Timeout: K", "Unexpected Pass: U"
    let (mut ok, mut fail, mut timeout, mut upass, mut found) = (0u32, 0u32, 0u32, 0u32, false);
    let toks: Vec<&str> = out.split_whitespace().collect();
    for w in toks.windows(2) {
        if let Ok(n) = w[1].parse::<u32>() {
            match w[0] {
                "Ok:" => { ok = n; found = true; }
                "Fail:" => { fail = n; found = true; }
                "Timeout:" => { timeout = n; }
                "Pass:" => { upass = n; } // "Unexpected Pass:"
                _ => {}
            }
        }
    }
    if found { (ok, (ok + fail + timeout + upass).max(1)) }
    else { match code { Some(0) => (1, 1), _ => (0, 1) } }
}

fn parse_jest(out: &str, code: Option<i32>) -> (u32, u32) {
    // Only the "Tests:" line (ignore "Test Suites:"). "Tests: 2 failed, 5 passed, 7 total"
    for line in out.lines() {
        let l = line.trim();
        if l.starts_with("Tests:") {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let (mut passed, mut total) = (0u32, 0u32);
            for w in toks.windows(2) {
                if let Ok(n) = w[0].parse::<u32>() {
                    match w[1].trim_end_matches(',') {
                        "passed" => passed = n,
                        "total" => total = n,
                        _ => {}
                    }
                }
            }
            if total > 0 { return (passed, total); }
        }
    }
    match code { Some(0) => (1, 1), _ => (0, 1) }
}

fn has_python_project(dir: &Path) -> bool {
    for m in ["pyproject.toml", "setup.py", "setup.cfg", "pytest.ini", "conftest.py", "tox.ini"] {
        if dir.join(m).exists() { return true; }
    }
    let is_py_test = |n: &str| (n.starts_with("test_") || n.ends_with("_test.py")) && n.ends_with(".py");
    // a test_*.py directly in this dir
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some(n) = e.file_name().to_str() {
                if is_py_test(n) { return true; }
            }
        }
    }
    // a tests/ dir that actually contains .py files (not e.g. a C project's tests/)
    let tdir = dir.join("tests");
    if tdir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&tdir) {
            for e in rd.flatten() {
                if e.path().extension().and_then(|x| x.to_str()) == Some("py") { return true; }
            }
        }
    }
    false
}

/// Detect (system, root-dir) pairs, keeping only the shallowest dir per system
/// along any branch (so a cargo workspace root subsumes its members, while two
/// independent sibling subrepos are both kept).
fn detect_build_systems(root: &Path) -> Vec<(BuildSystem, PathBuf)> {
    let mut found: Vec<(BuildSystem, PathBuf)> = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    const MAX_DEPTH: usize = 4;
    while let Some((d, depth)) = stack.pop() {
        if d.join("Cargo.toml").exists() { found.push((BuildSystem::Cargo, d.clone())); }
        if d.join("CMakeLists.txt").exists() { found.push((BuildSystem::Ctest, d.clone())); }
        if d.join("meson.build").exists() { found.push((BuildSystem::Meson, d.clone())); }
        if d.join("package.json").exists() { found.push((BuildSystem::Npm, d.clone())); }
        if d.join("go.mod").exists() { found.push((BuildSystem::GoTest, d.clone())); }
        if d.join("pom.xml").exists() { found.push((BuildSystem::Maven, d.clone())); }
        if has_python_project(&d) { found.push((BuildSystem::Pytest, d.clone())); }
        if depth < MAX_DEPTH {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    if e.path().is_dir() {
                        if let Some(n) = e.file_name().to_str() {
                            if !is_noise_dir(n) { stack.push((e.path(), depth + 1)); }
                        }
                    }
                }
            }
        }
    }
    // Cargo workspace roots (manifests declaring [workspace]).
    let ws_roots: Vec<PathBuf> = found.iter()
        .filter(|(s, _)| *s == BuildSystem::Cargo)
        .filter(|(_, d)| std::fs::read_to_string(d.join("Cargo.toml")).map(|t| t.contains("[workspace]")).unwrap_or(false))
        .map(|(_, d)| d.clone())
        .collect();

    let mut kept: Vec<(BuildSystem, PathBuf)> = Vec::new();
    for (sys, dir) in &found {
        let drop = match sys {
            BuildSystem::Cargo => {
                // A crate is covered by a workspace iff an *ancestor* declares
                // [workspace] and the crate itself doesn't (members are dropped;
                // a nested crate with its own [workspace] stays independent).
                let self_ws = ws_roots.iter().any(|w| w == dir);
                let under_ws = ws_roots.iter().any(|w| w != dir && dir.starts_with(w));
                !self_ws && under_ws
            }
            // cmake/meson/etc.: nested manifests are subsumed by the shallowest.
            _ => found.iter().any(|(s2, d2)| s2 == sys && d2 != dir && dir.starts_with(d2)),
        };
        if !drop && !kept.iter().any(|(s2, d2)| s2 == sys && d2 == dir) {
            kept.push((*sys, dir.clone()));
        }
    }
    kept.sort_by(|a, b| a.1.cmp(&b.1));
    kept.truncate(12);
    kept
}

/// Build-cache env. `slot` selects a per-worker `CARGO_TARGET_DIR` so parallel
/// workers don't contend on cargo's target lock (ccache is concurrency-safe, so
/// its dir stays shared).
fn cache_env(cache: bool, slot: usize) -> Vec<(String, String)> {
    if !cache { return Vec::new(); }
    let base = crate::registry::data_root().join("cache");
    let _ = std::fs::create_dir_all(base.join("ccache"));
    let target = base.join(format!("cargo-target-{slot}"));
    let _ = std::fs::create_dir_all(&target);
    // sccache and CARGO_INCREMENTAL are mutually exclusive — sccache exits with
    // "incremental compilation is prohibited" if both are set. When sccache is
    // available we prefer it (better cross-bot sharing) and disable incremental.
    let have_sccache = which("sccache");
    let incremental = if have_sccache { "0" } else { "1" };
    let mut v = vec![
        ("CCACHE_DIR".into(), base.join("ccache").display().to_string()),
        // Bound the shared compile cache so a long multi-repo swarm can't fill the
        // disk; ccache self-evicts LRU past this. Override with ARENA_CCACHE_GB.
        ("CCACHE_MAXSIZE".into(), format!("{}G",
            std::env::var("ARENA_CCACHE_GB").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(10))),
        ("CARGO_TARGET_DIR".into(), target.display().to_string()),
        ("CARGO_INCREMENTAL".into(), incremental.into()),
        ("CC".into(), "ccache cc".into()),
        ("CXX".into(), "ccache c++".into()),
    ];
    if have_sccache {
        v.push(("RUSTC_WRAPPER".into(), "sccache".into()));
        // sccache infers rustc's sysroot from `dirname(RUSTC)/../lib`. If rustc
        // is invoked via a symlinked path (e.g. inside our shelter allow-dir),
        // the inferred sysroot points at allow_dir/../lib — which doesn't exist
        // — and sccache fails with "Failed to list rustc sysroot". Resolve to
        // the canonical real binary so the inferred sysroot is the actual
        // rustup toolchain lib dir.
        if let Some(rustc_path) = which_path("rustc") {
            if let Ok(real) = std::fs::canonicalize(&rustc_path) {
                v.push(("RUSTC".into(), real.display().to_string()));
            }
        }
    }
    v
}

/// Recursive byte size of a directory (best-effort; symlinks not followed).
fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(e.path()),
                Ok(ft) if ft.is_file() => { total += e.metadata().map(|m| m.len()).unwrap_or(0); }
                _ => {}
            }
        }
    }
    total
}

/// Footprint control for a long multi-repo swarm: if the per-slot cargo-target
/// caches exceed the cap (ARENA_CACHE_CAP_GB, default 20), evict whole slot dirs
/// oldest-first until under it. The shared ccache is left alone (it self-evicts via
/// CCACHE_MAXSIZE). Returns bytes freed. Called at run end.
fn prune_cache() -> u64 {
    let base = crate::registry::data_root().join("cache");
    let cap_gb = std::env::var("ARENA_CACHE_CAP_GB").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(20);
    let cap = cap_gb * 1024 * 1024 * 1024;
    // collect cargo-target-* dirs with size + mtime
    let Ok(rd) = std::fs::read_dir(&base) else { return 0 };
    let mut slots: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("cargo-target-") {
            let sz = dir_size(&e.path());
            let mt = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total += sz;
            slots.push((e.path(), sz, mt));
        }
    }
    if total <= cap { return 0; }
    slots.sort_by_key(|(_, _, mt)| *mt); // oldest first
    let mut freed = 0u64;
    for (path, sz, _) in slots {
        if total - freed <= cap { break; }
        if std::fs::remove_dir_all(&path).is_ok() { freed += sz; }
    }
    freed
}

/// Options for running one command.
#[derive(Clone)]
struct RunOpts {
    cache: bool,
    timeout_secs: u64,
    max_file_mb: u64,
    sandbox: bool,
    slot: usize,
    allowed: Vec<String>, // include list (shelter allowlist); empty = no extra allow
}

/// bubblewrap wrapper: network off, whole FS read-only, writes limited to the
/// worktree + the build cache, and /tmp replaced by an empty tmpfs so a command
/// can't `cd` into sibling worktrees or destroy anything outside its own dir.
fn sandbox_argv(work: &Path, cache_dir: &Path, inner: &str) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--unshare-net".into(), "--die-with-parent".into(),
        "--ro-bind".into(), "/".into(), "/".into(),
        "--dev".into(), "/dev".into(),
        "--proc".into(), "/proc".into(),
        "--tmpfs".into(), "/tmp".into(),
    ];
    // re-expose the cache (writable) and the worktree (writable, cwd)
    a.extend(["--bind".into(), cache_dir.display().to_string(), cache_dir.display().to_string()]);
    a.extend(["--bind".into(), work.display().to_string(), work.display().to_string()]);
    a.extend(["--chdir".into(), work.display().to_string()]);
    a.extend(["sh".into(), "-c".into(), inner.to_string()]);
    a
}

fn which(prog: &str) -> bool {
    std::process::Command::new("sh").arg("-c").arg(format!("command -v {prog}")).output()
        .map(|o| o.status.success()).unwrap_or(false)
}

/// Single-quote a string for safe embedding in `sh -c '...'`.
fn sh_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Wrap a command so a single output file can't exceed `max_file_mb` (ulimit -f,
/// in 1024-byte blocks under bash) and the whole thing is killed after
/// `timeout_secs` (0 = no timeout). SIGTERM, then SIGKILL 5s later.
fn guard_cmd(cmd: &str, timeout_secs: u64, max_file_mb: u64) -> String {
    let body = if timeout_secs > 0 {
        format!("timeout -k 5 {timeout_secs} sh -c {}", sh_squote(cmd))
    } else {
        format!("sh -c {}", sh_squote(cmd))
    };
    let blocks = max_file_mb.max(1) * 1024;
    format!("ulimit -f {blocks} 2>/dev/null; {body}")
}

/// Keep captured output bounded so a runaway command can't blow up memory.
fn cap_output(mut s: String) -> String {
    const MAX: usize = 256 * 1024;
    if s.len() > MAX {
        s.truncate(MAX);
        s.push_str("\n…[output truncated]…\n");
    }
    s
}

/// Run one command in `dir` via the chosen backend, with cache env, resource
/// limits, and optional sandbox, capturing (bounded) output + exit code.
fn run_capture(backend: BackendKind, container: &str, dir: &Path, cmd: &str, opts: RunOpts) -> (String, Option<i32>) {
    let env = cache_env(opts.cache, opts.slot);
    let guarded = guard_cmd(cmd, opts.timeout_secs, opts.max_file_mb);
    let out = match backend {
        BackendKind::Conda => {
            let mut c = std::process::Command::new("conda");
            c.args(["run", "-p", container, "sh", "-c", &guarded]).current_dir(dir);
            for (k, v) in &env { c.env(k, v); }
            c.output()
        }
        BackendKind::Docker => {
            let dest = format!("{container}:/work-bot");
            let _ = std::process::Command::new("docker").args(["exec", container, "rm", "-rf", "/work-bot"]).output();
            let cp = std::process::Command::new("docker").arg("cp").arg(format!("{}/.", dir.display())).arg(&dest).output();
            if cp.map(|o| !o.status.success()).unwrap_or(true) { return (String::new(), Some(1)); }
            std::process::Command::new("docker").args(["exec", container, "sh", "-c", &format!("cd /work-bot && {guarded}")]).output()
        }
        _ => {
            // Local: optionally confine with bubblewrap ("shelter mode").
            if opts.sandbox && which("bwrap") {
                let cache_dir = crate::registry::data_root().join("cache");
                let _ = std::fs::create_dir_all(&cache_dir);
                let mut argv = sandbox_argv(dir, &cache_dir, &guarded);
                // Inject PATH *inside* the sandbox via --setenv so the OUTER bwrap
                // process keeps the host's PATH (it has to, or the exec lookup for
                // `bwrap` itself fails with ENOENT — which is exactly the bug we just
                // hit). The allow-dir is what the bot's sh -c sees.
                if let Some(allow) = shelter_allow_dir(&opts.allowed) {
                    // --setenv goes BEFORE the `sh -c …` tail of argv
                    let setenv = vec!["--setenv".to_string(), "PATH".to_string(), allow.display().to_string()];
                    let split = argv.iter().position(|a| a == "sh").unwrap_or(argv.len());
                    argv.splice(split..split, setenv);
                }
                // Resolve bwrap to an absolute path so execvp doesn't depend on PATH.
                let bwrap_bin = which_path("bwrap").unwrap_or_else(|| "bwrap".to_string());
                let mut c = std::process::Command::new(&bwrap_bin);
                c.args(&argv);
                for (k, v) in &env { c.env(k, v); }
                c.output()
            } else {
                let mut c = std::process::Command::new("sh");
                c.arg("-c").arg(&guarded).current_dir(dir);
                for (k, v) in &env { c.env(k, v); }
                // Even unconfined, shadow destructive binaries so a bare `rm`/`curl`
                // hits a refusing stub rather than the real tool.
                if let (Some(shadow), Ok(orig)) = (deny_shadow_dir(), std::env::var("PATH")) {
                    c.env("PATH", format!("{}:{}", shadow.display(), orig));
                }
                c.output()
            }
        }
    };
    match out {
        Ok(o) => (cap_output(format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))), o.status.code()),
        Err(e) => (format!("[harness] could not spawn the command process: {e}"), Some(127)),
    }
}

/// Destructive / exfiltration binaries that must never be callable by a worker,
/// regardless of the allow list. In shelter mode they're simply absent from PATH;
/// outside shelter they're shadowed by stub scripts that refuse to run.
/// Reasons a generated command can be rejected before it ever reaches a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsafeReason {
    DeniedBinary(String),
    PathEscape,        // .. or absolute paths writing outside the workspace
    ShellMeta(String), // ;  &&  ||  `…`  $(…)  redirection to absolute paths
    DangerousBuiltin,  // exec / eval / source / alias
    TopLevelGlob,      // bare *, ?, or [...] at argv[0]
    Empty,
}
impl std::fmt::Display for UnsafeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnsafeReason::DeniedBinary(b) => write!(f, "uses denied binary `{b}`"),
            UnsafeReason::PathEscape => write!(f, "path escapes the workspace (.. or absolute write)"),
            UnsafeReason::ShellMeta(s) => write!(f, "contains dangerous shell meta `{s}`"),
            UnsafeReason::DangerousBuiltin => write!(f, "contains exec/eval/source/alias"),
            UnsafeReason::TopLevelGlob => write!(f, "begins with a top-level glob"),
            UnsafeReason::Empty => write!(f, "empty command"),
        }
    }
}

/// Validate a command string about to be run in a worktree. Use for any
/// FSM-generated or otherwise untrusted shell input. Defense-in-depth: bubblewrap
/// shelter prevents *damage* (writes outside the worktree fail); this validator
/// catches things earlier and is enforceable even when shelter is off.
pub fn safe_command(cmd: &str) -> Result<(), UnsafeReason> {
    let cmd = cmd.trim();
    if cmd.is_empty() { return Err(UnsafeReason::Empty); }

    // 1. Forbidden shell metacharacters that enable chaining, substitution,
    //    or redirection-out: `;` `&&` `||` `` ` `` `$(` `>` `>>` `<(`
    for bad in [";", "&&", "||", "`", "$(", ">/", ">>/", "< /", "<(", "|&"].iter() {
        if cmd.contains(bad) {
            return Err(UnsafeReason::ShellMeta((*bad).into()));
        }
    }

    // 2. Path escape: any token containing `..` or any redirection target that
    //    isn't a bare filename (we approximate by rejecting all `>` followed by
    //    a path that contains `/` — see ShellMeta above for absolute paths).
    if cmd.split_whitespace().any(|t| t.split('=').any(|p| p == ".." || p.starts_with("../") || p.contains("/.."))) {
        return Err(UnsafeReason::PathEscape);
    }

    // 3. First token (binary name) checks: denied binary, dangerous builtin,
    //    or a top-level glob (`* foo`, `?bar`, etc).
    let first = cmd.split_whitespace().next().unwrap_or("");
    if matches!(first, "exec" | "eval" | "source" | "." | "alias" | "trap" | "unalias") {
        return Err(UnsafeReason::DangerousBuiltin);
    }
    if first.starts_with('*') || first.starts_with('?') || first.starts_with('[') {
        return Err(UnsafeReason::TopLevelGlob);
    }
    // Strip path prefix so /bin/rm or ./rm or sub/rm all match "rm"
    let bin = first.rsplit('/').next().unwrap_or(first);
    if DENY_BINS.contains(&bin) {
        return Err(UnsafeReason::DeniedBinary(bin.into()));
    }

    Ok(())
}

const DENY_BINS: &[&str] = &[
    // destructive
    "rm", "rmdir", "dd", "mkfs", "shred", "wipe", "fsck", "fdisk", "parted", "mkswap",
    // system control
    "shutdown", "reboot", "halt", "poweroff", "init", "telinit", "systemctl", "service",
    "mount", "umount", "swapoff", "swapon", "kexec",
    // network exfiltration / fetch
    "wget", "curl", "scp", "rsync", "ssh", "sftp", "ftp", "nc", "ncat", "socat",
    "telnet", "tftp",
    // privilege escalation / identity
    "sudo", "su", "doas", "pkexec", "passwd", "chown", "chgrp", "chmod", "setcap",
    // kernel / device interaction
    "modprobe", "insmod", "rmmod", "sysctl",
    // package managers (network + privileged installs)
    "apt", "apt-get", "dpkg", "dnf", "yum", "rpm", "pacman", "snap", "flatpak",
    // task scheduling
    "crontab", "at", "batch",
];

/// Toolchain + safe coreutils auto-added to the shelter allow list so builds/tests
/// still run when PATH is restricted to the allow list.
const ESSENTIAL_BINS: &[&str] = &[
    "sh", "bash", "env", "pwd", "ls", "cat", "echo", "printf", "test", "true", "false",
    "timeout", "stat", "readlink", "realpath", "id",
    "mkdir", "cp", "mv", "touch", "ln", "basename", "dirname", "sed", "awk", "grep",
    "sort", "uniq", "head", "tail", "wc", "tr", "cut", "find", "xargs", "tar", "gzip",
    "rustc", "cargo", "rustup", "rust-lld", "cc", "c++", "gcc", "g++", "ld", "ar", "as", "make", "ninja",
    "ranlib", "nm", "strip", "cpp", "objcopy", "objdump", "pkg-config", "perl", "clang", "clang++", "llvm-ar",
    "cmake", "ctest", "meson", "python3", "node", "pytest", "git",
    "mvn", "java", "javac",
    // build accelerators — cache_env sets RUSTC_WRAPPER=sccache and CC="ccache cc"
    // when cache is on; if these aren't on PATH inside shelter, cargo silently
    // dies with exit 127 and empty output (the bug your run hit).
    "sccache", "ccache",
];

fn which_path(prog: &str) -> Option<String> {
    let o = std::process::Command::new("sh").arg("-c").arg(format!("command -v {prog}")).output().ok()?;
    if !o.status.success() { return None; }
    let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if p.starts_with('/') { Some(p) } else { None }
}

/// A PATH dir of stub scripts (one per denied binary) that print "blocked" and
/// exit non-zero. Prepended to PATH outside shelter mode so a bare `rm`/`curl`
/// resolves to the stub instead of the real binary, without disturbing anything
/// else on PATH.
fn deny_shadow_dir() -> Option<PathBuf> {
    let dir = crate::registry::data_root().join("cache").join("deny-shadow");
    std::fs::create_dir_all(&dir).ok()?;
    for b in DENY_BINS {
        let p = dir.join(b);
        if !p.exists() {
            std::fs::write(&p, format!("#!/bin/sh\necho \"blocked by shelter: {b}\" >&2\nexit 126\n")).ok()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    Some(dir)
}

/// A PATH dir of symlinks to exactly the allowed binaries (the GUI include list ∪
/// essential toolchain, minus the denylist). In shelter mode PATH is set to *only*
/// this dir, so any binary not on the list is unresolved — i.e. rejected.
fn shelter_allow_dir(allowed: &[String]) -> Option<PathBuf> {
    let dir = crate::registry::data_root().join("cache").join("shelter-bin");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let deny: std::collections::HashSet<&str> = DENY_BINS.iter().copied().collect();
    let mut names: Vec<String> = Vec::new();
    for n in allowed.iter().map(|s| s.as_str()).chain(ESSENTIAL_BINS.iter().copied()) {
        if !deny.contains(n) && !names.iter().any(|x| x == n) { names.push(n.to_string()); }
    }
    #[cfg(unix)]
    for n in &names {
        if let Some(target) = which_path(n) {
            let _ = std::os::unix::fs::symlink(&target, dir.join(n));
        }
    }
    Some(dir)
}

/// The executable each system needs on PATH, for preflight checks.
fn primary_tool(sys: BuildSystem) -> &'static str {
    match sys {
        BuildSystem::Cargo => "cargo", BuildSystem::Pytest => "python3",
        BuildSystem::Ctest => "cmake", BuildSystem::Meson => "meson",
        BuildSystem::Npm => "npm", BuildSystem::GoTest => "go",
        BuildSystem::Maven => "mvn",
    }
}

/// Build-system markers the arena recognizes but cannot run (no reliable test
/// command / output parser). Used to *classify* a workspace so the multi-workspace
/// swarm can auto-skip codebases it can't actually score — instead of burning the
/// whole run on a repo whose tests it can never read. (filename/dirname → name)
const UNSUPPORTED_MARKERS: &[(&str, &str)] = &[
    ("build.gradle", "Gradle"), ("build.gradle.kts", "Gradle"), ("settings.gradle", "Gradle"),
    ("build.sbt", "sbt (Scala)"), ("mix.exs", "Mix (Elixir)"), ("rebar.config", "Rebar (Erlang)"),
    ("Gemfile", "Bundler (Ruby)"), ("Rakefile", "Rake (Ruby)"), ("composer.json", "Composer (PHP)"),
    ("dune-project", "Dune (OCaml)"), ("stack.yaml", "Stack (Haskell)"), ("Package.swift", "SwiftPM"),
    ("pubspec.yaml", "Dart/Flutter"), ("Project.toml", "Julia Pkg"), ("DESCRIPTION", "R"),
    ("*.csproj", "dotnet"), ("*.sln", "dotnet"), ("*.cabal", "Cabal (Haskell)"),
];

/// What the arena can do with a workspace's build tooling.
#[derive(Debug, PartialEq)]
struct Supportability {
    supported: Vec<String>,   // labels of systems we can run
    unsupported: Vec<String>, // names of systems we recognize but can't run
}

impl Supportability {
    fn runnable(&self) -> bool { !self.supported.is_empty() }
}

/// Inventory a workspace: which build systems we can run, and which recognized-but-
/// unsupported systems are also present. Scans shallowly (the markers live near the
/// root for any real project).
fn classify_workspace(root: &Path) -> Supportability {
    let mut supported: Vec<String> = detect_build_systems(root).into_iter()
        .map(|(s, _)| s.label().to_string()).collect();
    supported.sort(); supported.dedup();

    let mut unsupported = std::collections::BTreeSet::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                for (marker, sys) in UNSUPPORTED_MARKERS {
                    let hit = if let Some(ext) = marker.strip_prefix("*") {
                        name.ends_with(ext)
                    } else { name == *marker };
                    if hit { unsupported.insert(sys.to_string()); }
                }
                if e.path().is_dir() && depth < 3 && !is_noise_dir(&name) {
                    stack.push((e.path(), depth + 1));
                }
            }
        }
    }
    Supportability { supported, unsupported: unsupported.into_iter().collect() }
}

/// Tools the configured test command needs but that aren't on PATH. A non-empty
/// result means the *harness* is broken (not the bot) — surface it, don't grind
/// out generations of zeros that look like the algorithm failing.
fn missing_tools(cfg: &Config) -> Vec<String> {
    let cmd = cfg.test_cmd.trim();
    if cmd.is_empty() || cmd.eq_ignore_ascii_case("auto") {
        let targets = effective_targets(cfg);
        if targets.is_empty() { return vec![]; }
        let mut seen: Vec<String> = Vec::new();
        for ws in &targets {
            for (sys, _) in detect_build_systems(ws) {
                let t = primary_tool(sys).to_string();
                if !which(&t) && !seen.contains(&t) { seen.push(t); }
            }
        }
        seen
    } else {
        let first = cmd.split_whitespace().next().unwrap_or("");
        if !first.is_empty() && !which(first) { vec![first.to_string()] } else { vec![] }
    }
}

/// Score a worktree: "auto"/empty test command → detect & aggregate all systems;
/// otherwise run the literal command. Returns (passed, total).
// ===== Checkpoints ==========================================================
//
// A run's population is the only thing that's hard to reconstruct (seed, ops,
// targets are all in the Config). We dump it to a small text file under the
// registry at the end of a run so a follow-up run can pick up where this one
// left off, instead of restarting from scratch every time.

#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub seed: u64,
    pub gen: usize,
    pub focus: f64,
    pub bots: Vec<(usize, f64, f64, f64, bool)>, // id, z, genome, fitness, alive
}

fn checkpoint_path() -> PathBuf {
    let dir = crate::registry::data_root().join("checkpoints");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("latest.ckpt")
}

pub fn save_checkpoint(ckpt: &Checkpoint) -> std::io::Result<PathBuf> {
    let path = checkpoint_path();
    let mut s = String::new();
    s.push_str(&format!("# bot-arena checkpoint v1\nseed {}\ngen {}\nfocus {}\nbots {}\n",
        ckpt.seed, ckpt.gen, ckpt.focus, ckpt.bots.len()));
    for (id, z, g, f, a) in &ckpt.bots {
        s.push_str(&format!("{id}\t{z}\t{g}\t{f}\t{}\n", if *a { 1 } else { 0 }));
    }
    std::fs::write(&path, s)?;
    Ok(path)
}

pub fn load_checkpoint() -> Option<Checkpoint> {
    let path = checkpoint_path();
    let s = std::fs::read_to_string(&path).ok()?;
    let mut seed = 0u64; let mut gen = 0usize; let mut focus = 0.5f64;
    let mut bots: Vec<(usize, f64, f64, f64, bool)> = Vec::new();
    for line in s.lines() {
        if line.starts_with('#') || line.is_empty() { continue; }
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("seed"), Some(v)) => { seed = v.parse().ok()?; }
            (Some("gen"), Some(v)) => { gen = v.parse().ok()?; }
            (Some("focus"), Some(v)) => { focus = v.parse().ok()?; }
            (Some("bots"), _) => {}
            (Some(id), Some(z)) => {
                let g = parts.next()?; let f = parts.next()?; let a = parts.next()?;
                bots.push((id.parse().ok()?, z.parse().ok()?, g.parse().ok()?, f.parse().ok()?, a == "1"));
            }
            _ => {}
        }
    }
    Some(Checkpoint { seed, gen, focus, bots })
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    #[test]
    fn save_then_load_round_trip() {
        let _g = super::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("ckpt-{}", std::process::id()));
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        let original = Checkpoint {
            seed: 12345, gen: 7, focus: 0.42,
            bots: vec![(0, 0.1, 0.2, 0.3, true), (1, 0.9, 0.8, 0.7, false)],
        };
        save_checkpoint(&original).unwrap();
        let loaded = load_checkpoint().unwrap();
        assert_eq!(loaded.seed, original.seed);
        assert_eq!(loaded.gen, original.gen);
        assert!((loaded.focus - original.focus).abs() < 1e-9);
        assert_eq!(loaded.bots.len(), original.bots.len());
        assert_eq!(loaded.bots[0].0, 0);
        assert_eq!(loaded.bots[1].4, false);
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// In-memory cache of average wall-clock seconds per build system, used to run
/// fastest first so a broken candidate fails early with partial scoring instead of
/// burning time on the slow systems.
fn duration_cache() -> &'static std::sync::Mutex<std::collections::HashMap<BuildSystem, f64>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<BuildSystem, f64>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
fn record_duration(sys: BuildSystem, secs: f64) {
    let mut c = duration_cache().lock().unwrap();
    let blended = c.get(&sys).copied().map(|prev| 0.7 * prev + 0.3 * secs).unwrap_or(secs);
    c.insert(sys, blended);
}
fn sort_by_duration(systems: &mut Vec<(BuildSystem, PathBuf)>) {
    let c = duration_cache().lock().unwrap();
    systems.sort_by(|a, b| {
        let da = c.get(&a.0).copied().unwrap_or(f64::INFINITY);
        let db = c.get(&b.0).copied().unwrap_or(f64::INFINITY);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// "Anti-tests" tripwire: heuristically, did the diff touch a test file? In
/// random-mutation mode, edits to tests almost always mean the bot weakened an
/// assertion (the classic reward hack — "pass the tests by deleting them"). We
/// score those zero. Real agents that legitimately *add* tests will be allowed
/// later via a config switch; for now the swarm can't earn points by cheating.
fn touched_test_file(orig: &Path, work: &Path) -> bool {
    let mut changed = Vec::new();
    diff_walk(orig, work, Path::new(""), &mut changed);
    for rel in &changed {
        if is_test_path(rel) { return true; }
        // Also catch edits to inline `#[cfg(test)] mod { … }` blocks inside an
        // otherwise non-test source file (extremely common in Rust). Without this,
        // the bot can weaken assertions in src/lib.rs and the path-only tripwire
        // never fires.
        let o = std::fs::read_to_string(orig.join(rel)).unwrap_or_default();
        let w = std::fs::read_to_string(work.join(rel)).unwrap_or_default();
        if extract_test_surface(&o) != extract_test_surface(&w) { return true; }
    }
    false
}

/// The full "test surface" of a Rust source: every `#[cfg(test)]` block PLUS every
/// individually test-attributed item (`#[test]`, `#[bench]`, `#[tokio::test]`, …)
/// anywhere in the file — including ones placed at crate root, outside any
/// `#[cfg(test)] mod`. Comparing this between two versions catches a bot that adds,
/// removes, renames, or weakens a test *wherever* it sits. The earlier
/// block-only check missed a `#[test] fn` duplicated to crate scope, which let a
/// bot inflate its pass count by copying an already-passing test.
fn extract_test_surface(src: &str) -> String {
    let mut out = extract_test_blocks(src);
    let lines: Vec<&str> = src.lines().collect();
    let is_test_attr = |t: &str| -> bool {
        t.starts_with("#[test]") || t.starts_with("#[bench]")
            || t.starts_with("#[tokio::test") || t.starts_with("#[async_std::test")
            || t.starts_with("#[test_case") || t.starts_with("#[rstest")
            || t.starts_with("#[googletest::test")
            || (t.starts_with("#[") && t.contains("::test"))
    };
    let mut i = 0;
    while i < lines.len() {
        if is_test_attr(lines[i].trim_start()) {
            // Capture from the attribute through the end of the following item,
            // brace-matched (the fn may be on the same line, as one-liner tests are).
            let mut depth = 0i32;
            let mut started = false;
            let mut j = i;
            loop {
                out.push_str(lines[j]);
                out.push('\n');
                for ch in lines[j].chars() {
                    if ch == '{' { depth += 1; started = true; }
                    else if ch == '}' { depth -= 1; }
                }
                if started && depth <= 0 { break; }
                if !started && lines[j].trim_end().ends_with(';') { break; }
                j += 1;
                if j >= lines.len() || j - i > 300 { break; }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Concatenated content of every `#[cfg(test)] mod … { … }` block in a Rust
/// source file. Comparing this for two versions of a file tells us whether the
/// bot's edit landed inside a test block, even when the file is otherwise the
/// kind of file bots are *allowed* to edit. Brace matching is depth-counted
/// (false-positives on braces-inside-strings are acceptable for a tripwire —
/// we'd rather over-flag than miss a cheat).
fn extract_test_blocks(src: &str) -> String {
    let bytes = src.as_bytes();
    let needle = b"#[cfg(test)]";
    let mut out = String::new();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // Find the opening `{` of the mod (or fn) that follows.
            let mut p = i + needle.len();
            while p < bytes.len() && bytes[p] != b'{' { p += 1; }
            if p >= bytes.len() { break; }
            let start = p;
            let mut depth = 1i32;
            p += 1;
            while p < bytes.len() && depth > 0 {
                match bytes[p] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                p += 1;
            }
            if let Ok(s) = std::str::from_utf8(&bytes[start..p]) {
                out.push_str(s);
                out.push('\n');
            }
            i = p;
        } else {
            i += 1;
        }
    }
    out
}

fn is_test_path(rel: &Path) -> bool {
    let s = rel.to_string_lossy();
    // common test layouts across languages
    s.contains("/tests/") || s.starts_with("tests/")
        || s.contains("/test/") || s.starts_with("test/")
        || s.contains("__tests__")
        || rel.file_name().map(|n| {
            let n = n.to_string_lossy();
            n.starts_with("test_") || n.ends_with("_test.go") || n.ends_with("_test.py")
                || n.ends_with(".test.js") || n.ends_with(".test.ts") || n.ends_with(".spec.js")
        }).unwrap_or(false)
}

fn diff_walk(a_root: &Path, b_root: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let a = a_root.join(rel);
    let b = b_root.join(rel);
    let Ok(rd) = std::fs::read_dir(&b) else { return; };
    for e in rd.flatten() {
        let name = e.file_name();
        let n = name.to_string_lossy();
        if is_noise_dir(&n) { continue; }
        let sub = rel.join(&name);
        let bp = e.path();
        if bp.is_dir() { diff_walk(a_root, b_root, &sub, out); continue; }
        let ap = a.join(&name);
        // file differs (size first as a cheap filter, then bytes if needed)
        let differs = match (std::fs::metadata(&ap), std::fs::metadata(&bp)) {
            (Ok(am), Ok(bm)) => am.len() != bm.len()
                || std::fs::read(&ap).unwrap_or_default() != std::fs::read(&bp).unwrap_or_default(),
            _ => true, // file exists on one side only → changed
        };
        if differs { out.push(sub); }
    }
}

fn score_worktree(cfg: &Config, container: &str, work: &Path, slot: usize) -> (u32, u32) {
    let (p, t, _) = score_worktree_out(cfg, container, work, slot);
    (p, t)
}

/// Like `score_worktree` but also returns the concatenated test output, so callers
/// can extract *which* tests failed (used to build the fault-localization prior).
fn score_worktree_out(cfg: &Config, container: &str, work: &Path, slot: usize) -> (u32, u32, String) {
    let opts = RunOpts {
        cache: cfg.cache, timeout_secs: cfg.timeout_secs, max_file_mb: cfg.max_file_mb,
        sandbox: cfg.sandbox, slot, allowed: cfg.tools.iter().cloned().chain(cfg.extra_tools.iter().cloned()).collect(),
    };
    let cmd = cfg.test_cmd.trim();
    if cmd.is_empty() || cmd.eq_ignore_ascii_case("auto") {
        let mut systems: Vec<(BuildSystem, PathBuf)> = detect_build_systems(work)
            .into_iter().filter(|(s, _)| which(primary_tool(*s))).collect();
        if systems.is_empty() { return (0, 1, String::new()); }
        sort_by_duration(&mut systems);
        let (mut p, mut t) = (0u32, 0u32);
        let mut all_out = String::new();
        for (sys, dir) in systems {
            let t0 = std::time::Instant::now();
            let (out, code) = run_capture(cfg.backend, container, &dir, &sys.command(cfg.cache, &cfg.cargo_features, &cfg.build_flags), opts.clone());
            record_duration(sys, t0.elapsed().as_secs_f64());
            let (pp, tt) = sys.parse(&out, code);
            all_out.push_str(&out); all_out.push('\n');
            p += pp; t += tt;
            if tt > 0 && pp == 0 { break; }
        }
        (p, t.max(1), all_out)
    } else {
        let (out, code) = run_capture(cfg.backend, container, work, cmd, opts);
        let (p, t) = parse_test_output(&out, "", code);
        (p, t, out)
    }
}

/// Per-target set of tests that fail on the *pristine* (unedited) source. Computed
/// once per target at run start (when differential scoring is on) and compared
/// against each bot's post-edit failing set.
fn baseline_fail_cache() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<String>>> {
    static B: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<String>>>> = std::sync::OnceLock::new();
    B.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Provision a clean worktree of `ws`, run its tests, and remember which ones fail.
/// Idempotent per target (cached). Returns the baseline failing-set.
fn compute_baseline(cfg: &Config, ws: &Path, container: &str, run_root: &Path) -> Vec<String> {
    if let Some(v) = baseline_fail_cache().lock().unwrap().get(ws) { return v.clone(); }
    let target = run_root.join(format!("baseline-{}", sanitize_name(&ws.to_string_lossy())));
    let failing = match provision_worktree(cfg, ws, &target) {
        Ok(prov) => {
            let (_p, _t, out) = score_worktree_out(cfg, container, prov.path(), 0);
            let f = parse_failing_names(&out);
            prov.remove();
            f
        }
        Err(_) => Vec::new(),
    };
    baseline_fail_cache().lock().unwrap().insert(ws.to_path_buf(), failing.clone());
    failing
}

fn sanitize_name(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>()
        .trim_matches('_').chars().rev().take(48).collect::<String>().chars().rev().collect()
}

/// A one-line preview of the command(s) `auto` will run, for the activity feed.
fn command_preview(cfg: &Config) -> String {
    let cmd = cfg.test_cmd.trim();
    if !(cmd.is_empty() || cmd.eq_ignore_ascii_case("auto")) {
        return cmd.to_string();
    }
    let targets = effective_targets(cfg);
    let mut labels: Vec<String> = Vec::new();
    for ws in targets.iter().take(3) {
        for (s, _) in detect_build_systems(ws) {
            let l = s.label().to_string();
            if !labels.contains(&l) { labels.push(l); }
        }
    }
    if labels.is_empty() { "auto".to_string() } else { labels.join(" + ") }
}

// ===== Score memoization ====================================================
//
// Many bots converge on identical repo states (no-op edits, the same fix found
// twice). Hashing the edited worktree lets us reuse a score we already computed
// instead of re-running the (expensive) test command. The cache persists under
// the registry so it survives across runs.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Deterministic content hash of a worktree's source (path + bytes), skipping
/// noise and large files. DefaultHasher uses fixed keys, so it's stable.
fn worktree_hash(dir: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut files: Vec<PathBuf> = Vec::new();
    collect_hashable(dir, dir, &mut files);
    files.sort();
    for f in files {
        if let Ok(rel) = f.strip_prefix(dir) {
            rel.to_string_lossy().hash(&mut hasher);
        }
        if let Ok(meta) = std::fs::metadata(&f) {
            if meta.len() <= 2 * 1024 * 1024 {
                if let Ok(bytes) = std::fs::read(&f) { bytes.hash(&mut hasher); }
            } else {
                meta.len().hash(&mut hasher); // huge file: hash its size only
            }
        }
    }
    hasher.finish()
}

fn collect_hashable(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(dir) { Ok(r) => r, Err(_) => return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let ns = name.to_string_lossy();
        if matches!(ns.as_ref(), ".git" | "__pycache__" | ".pytest_cache" | "node_modules" | "target" | "build" | ".venv") {
            continue;
        }
        if p.is_dir() { collect_hashable(root, &p, out); }
        else if p.is_file() { out.push(p); }
    }
}

fn scores_file() -> PathBuf {
    crate::registry::data_root().join("cache").join("scores.tsv")
}

fn load_scores() -> HashMap<u64, (u32, u32)> {
    let mut m = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(scores_file()) {
        for line in text.lines() {
            let mut it = line.split('\t');
            if let (Some(h), Some(p), Some(t)) = (it.next(), it.next(), it.next()) {
                if let (Ok(h), Ok(p), Ok(t)) = (h.parse(), p.parse(), t.parse()) {
                    m.insert(h, (p, t));
                }
            }
        }
    }
    m
}

fn save_scores(m: &HashMap<u64, (u32, u32)>) {
    let path = scores_file();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let mut s = String::new();
    for (h, (p, t)) in m { s.push_str(&format!("{h}\t{p}\t{t}\n")); }
    let _ = std::fs::write(path, s);
}

// ===== Foundations for a command-genome (future) ===========================
//
// Not yet wired into the evolutionary loop. These build a *weighted token
// vocabulary* so that searching over command strings can be grounded in plausible
// tokens (flags from --help/man, real filepaths) instead of pure random noise.

/// Pull path-like tokens out of a command string, so the vocabulary is biased
/// toward files/dirs the command actually references.
fn extract_filepaths(cmd: &str) -> Vec<String> {
    cmd.split_whitespace()
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .filter(|t| !t.starts_with('-') && (t.contains('/') || (t.contains('.') && t.len() > 2)))
        .map(|t| t.to_string())
        .collect()
}

/// Frequency-weighted vocabulary from `--help`/man text: flags (`-x`, `--long`)
/// and bare words, counted by occurrence (higher count → more likely to be drawn
/// when composing a candidate command).
fn weighted_vocab(text: &str) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for raw in text.split(|c: char| c.is_whitespace() || "|[],()<>=".contains(c)) {
        let tok = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric() && !"-_/.".contains(c));
        if tok.len() < 2 { continue; }
        let starts_ok = tok.starts_with('-') || tok.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false);
        if starts_ok { *counts.entry(tok.to_string()).or_default() += 1; }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

// ===== Diff metrics + git history (scoring foundations) =====================
//
// Not yet wired into reward. These are the building blocks for the git-derived and
// diff-derived signals: "concise vs creative" diff scoring, and sampling the repo's
// own history (progression / bug-injection curriculum).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffMetrics {
    pub lines_changed: usize,
    pub novel_tokens: usize,
    /// "creative but concise": novel identifiers per line changed. Higher = a small
    /// change that still introduces new ideas. (Caution: gameable as a *reward* —
    /// gibberish tokens inflate it — so use it as a tie-breaker/diversity signal,
    /// not as the primary metric, which stays test pass-fraction.)
    pub score: f64,
}

fn tokens(s: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            cur.push(c);
        } else if !cur.is_empty() {
            if cur.len() >= 2 { set.insert(std::mem::take(&mut cur)); } else { cur.clear(); }
        }
    }
    if cur.len() >= 2 { set.insert(cur); }
    set
}

/// Compare two file versions: how big is the change (multiset line difference) and
/// how many genuinely new identifiers it introduces.
fn diff_metrics(before: &str, after: &str) -> DiffMetrics {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for l in before.lines() { *counts.entry(l).or_default() += 1; }
    for l in after.lines() { *counts.entry(l).or_default() -= 1; }
    let lines_changed = counts.values().map(|v| v.unsigned_abs() as usize).sum();

    let before_toks = tokens(before);
    let novel_tokens = tokens(after).into_iter().filter(|t| !before_toks.contains(t)).count();

    let score = novel_tokens as f64 / (1.0 + lines_changed as f64);
    DiffMetrics { lines_changed, novel_tokens, score }
}

/// Recent commit SHAs (newest first), for progression/curriculum batches.
fn git_recent_commits(repo: &Path, n: usize) -> Vec<String> {
    let out = std::process::Command::new("git").arg("-C").arg(repo)
        .args(["log", "--format=%H", "-n", &n.to_string()]).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// The unified diff a commit introduced (its change vs. its parent) — a
/// self-labeled demonstration of "what a good change looks like".
fn git_commit_diff(repo: &Path, sha: &str) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(repo)
        .args(["show", "--format=", "--no-color", sha]).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// Coherence signal: does this patch apply cleanly to the repo? (`git apply --check`)
fn git_patch_applies(repo: &Path, diff: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = match Command::new("git").arg("-C").arg(repo)
        .args(["apply", "--check"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(c) => c, Err(_) => return false,
    };
    if let Some(stdin) = child.stdin.as_mut() { let _ = stdin.write_all(diff.as_bytes()); }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

// ===== Patch proposer (the model seam) =====================================
//
// A bot's *action* can come from an external command instead of a random mutation.
// The protocol is model-agnostic so tinygpt, a local llama.cpp GGUF, or a cloud API
// are all just "a command that turns context into a patch":
//
//   stdin  ← JSON {"test_cmd","failing":[...],"files":[{"path","content"}]}
//   stdout → a unified diff (git-apply compatible), or empty for "no change"
//
// The diff is gated by `git apply --check` (incoherent patches are rejected and the
// bot keeps its baseline), then applied and scored like any other action. Every
// winning (context → patch) pair is the natural training set for distillation.

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn collect_text_files(root: &Path, rel: &Path, out: &mut Vec<(String, String)>, total: &mut usize) {
    if *total > 256 * 1024 || out.len() >= 40 { return; }
    let dir = root.join(rel);
    let Ok(rd) = std::fs::read_dir(&dir) else { return; };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let n = name.to_string_lossy();
        if is_noise_dir(&n) { continue; }
        let p = e.path();
        let sub = rel.join(&name);
        if p.is_dir() { collect_text_files(root, &sub, out, total); continue; }
        if let Ok(meta) = p.metadata() { if meta.len() > 32 * 1024 { continue; } }
        if let Ok(content) = std::fs::read_to_string(&p) {
            *total += content.len();
            if *total > 256 * 1024 { return; }
            out.push((sub.to_string_lossy().replace('\\', "/"), content));
            if out.len() >= 40 { return; }
        }
    }
}

/// Assemble the JSON context a proposer sees: test command, current failing-test
/// names, and the repo's (text, size-capped) source files.
fn assemble_context(dir: &Path, cfg: &Config, container: &str, slot: usize) -> (String, Vec<String>) {
    let failing = {
        let (_p, _t, out) = score_worktree_out(cfg, container, dir, slot);
        parse_failing_names(&out)
    };
    let mut files = Vec::new();
    let mut total = 0usize;
    collect_text_files(dir, Path::new(""), &mut files, &mut total);
    let mut json = String::from("{");
    json.push_str(&format!("\"test_cmd\":{},", json_str(&cfg.test_cmd)));
    json.push_str("\"failing\":[");
    for (i, f) in failing.iter().enumerate() { if i > 0 { json.push(','); } json.push_str(&json_str(f)); }
    json.push_str("],\"files\":[");
    for (i, (path, content)) in files.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push_str(&format!("{{\"path\":{},\"content\":{}}}", json_str(path), json_str(content)));
    }
    json.push_str("]}");
    (json, failing)
}

/// Run the proposer command in `dir`, feeding `context` on stdin; return its stdout
/// (a unified diff) or None on failure/empty.
fn run_proposer(dir: &Path, cmd: &str, context: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh").arg("-c").arg(cmd)
        .current_dir(dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().ok()?;
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(context.as_bytes()); }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() { return None; }
    let diff = String::from_utf8_lossy(&out.stdout).to_string();
    if diff.trim().is_empty() { None } else { Some(diff) }
}

/// Apply a unified diff to the worktree, gated by `git apply --check`. Returns true
/// iff it applied.
fn apply_patch_gated(dir: &Path, diff: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if !git_patch_applies(dir, diff) { return false; }
    let mut child = match Command::new("git").arg("-C").arg(dir)
        .arg("apply").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(c) => c, Err(_) => return false,
    };
    if let Some(si) = child.stdin.as_mut() { let _ = si.write_all(diff.as_bytes()); }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// The proposer action (single-shot): assemble context, ask the model-command for a
/// patch, gate + apply it. The cascade inlines this; retained for the unit test.
#[cfg(test)]
fn proposer_attempt(dir: &Path, cfg: &Config, container: &str, slot: usize) -> (bool, Option<(Vec<String>, String)>) {
    let (ctx, failing) = assemble_context(dir, cfg, container, slot);
    match run_proposer(dir, &cfg.proposer_cmd, &ctx) {
        Some(diff) => {
            let applied = apply_patch_gated(dir, &diff);
            (applied, if applied { Some((failing, diff)) } else { None })
        }
        None => (false, None),
    }
}

/// Append a winning (context → patch) record to the experience log: the distillation
/// training set. JSONL, one record per patch that scored well, capped so it can't grow
/// without bound. This is what a small local proposer (Tier 2) is later trained on.
fn log_experience(target: &Path, failing: &[String], diff: &str, score: f64) {
    // Dedup on (target, diff): when many bots win with the same patch in one generation
    // the harvest would otherwise fill with identical records and skew the distillation
    // corpus. A process-global seen-set keeps one canonical record per distinct fix.
    {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        static SEEN: std::sync::OnceLock<Mutex<std::collections::HashSet<u64>>> = std::sync::OnceLock::new();
        let seen = SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
        let mut h = DefaultHasher::new();
        target.hash(&mut h); diff.hash(&mut h);
        let key = h.finish();
        let mut set = seen.lock().unwrap();
        if !set.insert(key) { return; } // already harvested this exact fix
    }
    let dir = crate::registry::data_root();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("experience.jsonl");
    if let Ok(m) = std::fs::metadata(&path) { if m.len() > 16 * 1024 * 1024 { return; } } // cap at 16MB
    let mut rec = String::from("{");
    rec.push_str(&format!("\"target\":{},", json_str(&target.display().to_string())));
    rec.push_str(&format!("\"score\":{score:.4},"));
    rec.push_str("\"failing_before\":[");
    for (i, f) in failing.iter().enumerate() { if i > 0 { rec.push(','); } rec.push_str(&json_str(f)); }
    rec.push_str("],");
    rec.push_str(&format!("\"diff\":{}}}\n", json_str(diff)));
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(rec.as_bytes());
    }
}

/// Cascade telemetry: which tier of the proposer chain produced each winning patch, so
/// a run reports how often the fast/cheap model sufficed vs. escalated — the headline
/// "race for speed" metric. Index = tier (0 = primary/fastest).
fn tier_counter() -> &'static Mutex<Vec<usize>> {
    static C: std::sync::OnceLock<Mutex<Vec<usize>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}
fn record_tier(tier: usize) {
    let mut v = tier_counter().lock().unwrap();
    if v.len() <= tier { v.resize(tier + 1, 0); }
    v[tier] += 1;
}

/// The ordered proposer chain: primary first, then escalation fallbacks (empties dropped).
fn proposer_chain(cfg: &Config) -> Vec<String> {
    let mut v = Vec::new();
    if !cfg.proposer_cmd.is_empty() { v.push(cfg.proposer_cmd.clone()); }
    for f in &cfg.proposer_fallbacks { if !f.is_empty() { v.push(f.clone()); } }
    v
}

/// Score the *current* worktree the way evaluate_real would: differential reward vs a
/// pristine baseline when enabled and there were failures to fix, else pass-fraction.
/// Reusable so the cascade can score each tier's attempt.
fn score_state(cfg: &Config, ws: &Path, container: &str, work: &Path, run_root: &Path, slot: usize) -> f64 {
    if cfg.differential {
        let baseline = compute_baseline(cfg, ws, container, run_root);
        if !baseline.is_empty() {
            let (_p, _t, out) = score_worktree_out(cfg, container, work, slot);
            let now = parse_failing_names(&out);
            return differential_reward(&baseline, &now, 2.0);
        }
    }
    let (passed, total) = score_worktree(cfg, container, work, slot);
    if total == 0 { 0.0 } else { passed as f64 / total as f64 }
}

/// The set of content lines a unified diff adds or removes (ignoring +++/--- headers
/// and hunk @@ markers) — a diff's "signature".
fn changed_lines(diff: &str) -> std::collections::HashSet<String> {
    diff.lines()
        .filter(|l| (l.starts_with('+') || l.starts_with('-'))
            && !l.starts_with("+++") && !l.starts_with("---"))
        .map(|l| l[1..].trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Jaccard similarity of two diffs by their changed-line sets (0 = disjoint, 1 =
/// identical). The basis for *git scoring*: how close is a candidate's change to a
/// real beneficial commit's change.
fn diff_similarity(d1: &str, d2: &str) -> f64 {
    let (a, b) = (changed_lines(d1), changed_lines(d2));
    if a.is_empty() && b.is_empty() { return 1.0; }
    let inter = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    if union == 0.0 { 0.0 } else { inter / union }
}

/// *git diversification*: pick `k` commits whose diffs are as mutually dissimilar as
/// possible (greedy max-min over diff signatures). Used to assemble a varied
/// curriculum / target set so the population doesn't overfit one kind of change.
fn diverse_commits(repo: &Path, k: usize) -> Vec<String> {
    let all = git_recent_commits(repo, 40);
    if all.len() <= k { return all; }
    let diffs: Vec<String> = all.iter().map(|s| git_commit_diff(repo, s)).collect();
    let mut chosen = vec![0usize]; // seed with the newest commit
    while chosen.len() < k {
        let mut best: Option<usize> = None;
        let mut best_score = -1.0f64;
        for i in 0..all.len() {
            if chosen.contains(&i) { continue; }
            // distance to the chosen set = nearest (most similar) chosen commit
            let nearest = chosen.iter()
                .map(|&c| 1.0 - diff_similarity(&diffs[i], &diffs[c]))
                .fold(f64::INFINITY, f64::min);
            if nearest > best_score { best_score = nearest; best = Some(i); }
        }
        match best { Some(i) => chosen.push(i), None => break }
    }
    chosen.into_iter().map(|i| all[i].clone()).collect()
}

/// Highest similarity between a candidate diff and any reference diff — "how much
/// does this change resemble the closest known-good commit?" A git scoring signal
/// (use to rank/diversify within the hard test gate, not as the sole reward).
fn best_match(candidate: &str, refs: &[String]) -> f64 {
    refs.iter().map(|r| diff_similarity(candidate, r)).fold(0.0, f64::max)
}

/// Build a reference set of (sha, diff) pairs from the most mutually-dissimilar
/// recent commits — a varied bank of "known-good changes" to score candidates
/// against, and the seed material for a progression curriculum.
fn progression_refs(repo: &Path, k: usize) -> Vec<(String, String)> {
    diverse_commits(repo, k)
        .into_iter()
        .map(|sha| { let d = git_commit_diff(repo, &sha); (sha, d) })
        .filter(|(_, d)| !d.is_empty())
        .collect()
}

/// Longest common subsequence of two line-sequences, as matched index pairs in
/// order. Used to align two versions so recombination keeps shared scaffolding.
fn lcs_lines(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut res = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] { res.push((i, j)); i += 1; j += 1; }
        else if dp[i + 1][j] >= dp[i][j + 1] { i += 1; } else { j += 1; }
    }
    res
}

/// Recombine two versions of a file (e.g. the same file at commit N-1 and N+1):
/// shared lines (the LCS) are kept as anchors so the result stays structurally
/// sensible — "rebased on lines" rather than a random character interleave — and
/// each divergent hunk between anchors is taken from one parent at ~50/50. The
/// child is a candidate to be *scored* by the harness (better/worse than either
/// parent). This is crossover at the change level.
fn merge_versions(a: &str, b: &str, rng: &mut impl Rng) -> String {
    let al: Vec<&str> = a.lines().collect();
    let bl: Vec<&str> = b.lines().collect();
    let anchors = lcs_lines(&al, &bl);
    let mut out: Vec<String> = Vec::new();
    let (mut pi, mut pj) = (0usize, 0usize);
    let mut emit_hunk = |out: &mut Vec<String>, sa: &[&str], sb: &[&str], rng: &mut dyn FnMut() -> bool| {
        if sa.is_empty() && sb.is_empty() { return; }
        if rng() { out.extend(sa.iter().map(|s| s.to_string())); }
        else { out.extend(sb.iter().map(|s| s.to_string())); }
    };
    for (ai, bj) in anchors.iter().copied() {
        let mut coin = || rng.gen_bool(0.5);
        emit_hunk(&mut out, &al[pi..ai], &bl[pj..bj], &mut coin);
        out.push(al[ai].to_string()); // shared anchor line
        pi = ai + 1; pj = bj + 1;
    }
    let mut coin = || rng.gen_bool(0.5);
    emit_hunk(&mut out, &al[pi..], &bl[pj..], &mut coin);
    out.join("\n")
}

/// Full real evaluation: isolate a copy of the workspace, let the worker attempt
/// a purpose-typed edit, run the tests, return pass-fraction in [0,1].
/// Tests that mutate the process-global CODEBOT_ARENA_HOME env var must not run
/// concurrently or they stomp each other's paths. Serialize them on this lock.
#[cfg(test)]
fn env_test_lock() -> &'static std::sync::Mutex<()> {
    static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(()))
}

/// Append-only security audit log. Every danger-pattern hit and test-edit
/// tripwire writes a timestamped line to `~/.local/share/codebot-arena/security.log`
/// and bumps a process counter so a run can summarize "N security events" without
/// threading a channel through the hot eval path. Built for overnight runs: wake
/// up, read the log, see exactly what got flagged and culled.
fn security_log_path() -> PathBuf {
    crate::registry::data_root().join("security.log")
}
fn security_counter() -> &'static std::sync::atomic::AtomicUsize {
    static C: std::sync::OnceLock<std::sync::atomic::AtomicUsize> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::atomic::AtomicUsize::new(0))
}
fn record_security_event(bot_id: usize, kind: &str, detail: &str) {
    security_counter().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let detail = detail.replace(['\n', '\t'], " ").chars().take(200).collect::<String>();
    let line = format!("{ts}\tbot={bot_id}\tkind={kind}\tdetail={detail}\n");
    let path = security_log_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Scan newly-introduced source lines for blatantly destructive constructs. This
/// is belt-and-suspenders, NOT the primary defense — bubblewrap already contains
/// damage (the host fs is read-only). It exists to (a) catch obvious cases even
/// when shelter is off, and (b) provide telemetry for the future LLM-agent regime,
/// where generated code is far more capable of authoring such calls than the
/// current mutation operators are. Returns the matched pattern if any.
fn diff_introduces_danger(orig: &Path, work: &Path) -> Option<String> {
    const DANGER: &[&str] = &[
        "rm -rf", "rm -fr", "rmdir /", "mkfs", "dd if=", "dd of=/dev",
        ":(){", // fork bomb
        "remove_dir_all(\"/\")", "remove_dir_all(\"~", "/etc/passwd", "/dev/sda",
        "chmod -R 000", "> /dev/sd", "curl http", "wget http", "nc -e", "/bin/sh -i",
        "shutil.rmtree('/'", "os.system(\"rm", "subprocess", "fork()",
    ];
    let mut changed = Vec::new();
    diff_walk(orig, work, Path::new(""), &mut changed);
    for rel in &changed {
        let before = std::fs::read_to_string(orig.join(rel)).unwrap_or_default();
        let after = std::fs::read_to_string(work.join(rel)).unwrap_or_default();
        // only lines that are NEW in `after` (not present in `before`)
        let before_lines: std::collections::HashSet<&str> = before.lines().collect();
        for line in after.lines() {
            if before_lines.contains(line) { continue; }
            let low = line.to_ascii_lowercase();
            for pat in DANGER {
                if low.contains(&pat.to_ascii_lowercase()) {
                    return Some((*pat).to_string());
                }
            }
        }
    }
    None
}

/// Pull the *useful* lines out of a failed build/test output. Compiler errors
/// (`error[E0599]: …`, `error: could not compile …`) are what diagnose a failure,
/// but they appear in the MIDDLE of cargo's output — the tail is just "build
/// failed, waiting for other jobs…". Prefer error lines; fall back to the tail.
fn error_summary(out: &str) -> String {
    let errors: Vec<&str> = out.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("error[") || t.starts_with("error:")
                || t.starts_with("error -") || t.contains("panicked at")
                || t.starts_with("--> ") // the file:line location after an error
        })
        .take(8)
        .collect();
    if !errors.is_empty() {
        return errors.join(" | ");
    }
    // no recognizable error lines → last few non-empty lines
    out.lines().rev().filter(|l| !l.trim().is_empty()).take(4)
        .collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// Pre-flight shelter check: run a trivial command under bubblewrap. If this
/// fails, every bot will fail too. Returns Ok if sandboxing works on this host,
/// or Err(reason) with the actual stderr to surface in the log. This catches
/// "cargo exits 127 silently inside shelter" before thousands of bots see it.
fn shelter_smoke_test(cfg: &Config, run_root: &Path) -> Result<(), String> {
    if !cfg.sandbox { return Ok(()); }
    if !which("bwrap") {
        return Err("Shelter mode is on but `bwrap` (bubblewrap) isn't installed. \
            Install bubblewrap or turn Shelter mode off.".into());
    }
    let smoke_dir = run_root.join("__shelter_smoke__");
    let _ = std::fs::create_dir_all(&smoke_dir);
    let opts = RunOpts {
        cache: cfg.cache, timeout_secs: 10, max_file_mb: 10,
        sandbox: true, slot: 0, allowed: cfg.tools.iter().cloned().chain(cfg.extra_tools.iter().cloned()).collect(),
    };
    // Try the actual primary tool the run will use, not just `true`.
    let probe = if cfg.test_cmd.trim().eq_ignore_ascii_case("auto") || cfg.test_cmd.trim().is_empty() {
        // pick the first tool that exists on the host
        ["cargo --version", "pytest --version", "ctest --version", "go version", "npm --version"]
            .iter().find(|c| which(c.split_whitespace().next().unwrap_or(""))).copied()
            .unwrap_or("echo shelter-smoke-ok")
    } else {
        "echo shelter-smoke-ok"
    };
    let (out, code) = run_capture(cfg.backend, "smoke", &smoke_dir, probe, opts);
    let _ = std::fs::remove_dir_all(&smoke_dir);
    if code == Some(0) {
        Ok(())
    } else {
        let tail = error_summary(&out);
        Err(format!("shelter smoke test `{probe}` failed (exit {:?}). Tail: {}\n\
            Common causes: bubblewrap can't create user namespaces on this kernel \
            (try `sysctl kernel.unprivileged_userns_clone=1`), or a tool the test \
            command needs is missing from the allow-list. You can turn Shelter \
            mode off as a workaround, but only with a trusted workspace.",
            code, if tail.is_empty() { "(empty)".to_string() } else { tail }))
    }
}

/// Score the unmodified workspace and return per-system diagnostics. If baseline
/// is 0, *why* is the single most useful thing to know — exit code 124 means a
/// timeout kill; non-zero with no test output means compile or harness error.
fn baseline_score(cfg: &Config, ws: &Path, run_root: &Path) -> (u32, u32, Vec<String>) {
    let target = run_root.join("__baseline__");
    let _ = std::fs::remove_dir_all(&target);
    let prov = match provision_worktree(cfg, ws, &target) {
        Ok(p) => p,
        Err(e) => return (0, 0, vec![format!("provision failed: {e}")]),
    };
    let work = prov.path().to_path_buf();
    let opts = RunOpts {
        cache: cfg.cache, timeout_secs: cfg.timeout_secs, max_file_mb: cfg.max_file_mb,
        sandbox: cfg.sandbox, slot: 0, allowed: cfg.tools.iter().cloned().chain(cfg.extra_tools.iter().cloned()).collect(),
    };
    let mut diag = Vec::new();
    let cmd = cfg.test_cmd.trim();
    let (p, t) = if cmd.is_empty() || cmd.eq_ignore_ascii_case("auto") {
        let systems: Vec<(BuildSystem, PathBuf)> = detect_build_systems(&work)
            .into_iter().filter(|(s, _)| which(primary_tool(*s))).collect();
        if systems.is_empty() {
            diag.push("auto detection found NO runnable build system in this workspace \
                (no Cargo.toml / pyproject.toml / package.json / CMakeLists / meson.build / go.mod, \
                or the corresponding tool isn't on PATH).".to_string());
            (0u32, 0u32)
        } else {
            let (mut pp, mut tt) = (0u32, 0u32);
            for (sys, dir) in &systems {
                let (out, code) = run_capture(cfg.backend, "baseline", dir, &sys.command(cfg.cache, &cfg.cargo_features, &cfg.build_flags), opts.clone());
                let (a, b) = sys.parse(&out, code);
                let timeout_killed = code == Some(124);
                let tail = error_summary(&out);
                if timeout_killed {
                    diag.push(format!("{} timed out after {}s (exit 124) — TESTS WERE KILLED MID-RUN. \
                        Raise 'Per-command timeout' (currently {}s; cargo needs 15-60s on most projects). \
                        Last output: {}",
                        sys.label(), cfg.timeout_secs, cfg.timeout_secs, tail));
                } else if a == 0 && b <= 1 {
                    diag.push(format!("{} ran (exit {:?}) but no test counts were parsed. \
                        Output tail: {}", sys.label(), code, tail));
                } else {
                    diag.push(format!("{}: {}/{} ({:.3})", sys.label(), a, b,
                        if b == 0 { 0.0 } else { a as f64 / b as f64 }));
                }
                pp += a; tt += b;
            }
            (pp, tt.max(1))
        }
    } else {
        let (out, code) = run_capture(cfg.backend, "baseline", &work, cmd, opts);
        let (a, b) = parse_test_output(&out, "", code);
        let tail = error_summary(&out);
        if code == Some(124) {
            diag.push(format!("custom test command timed out after {}s (exit 124). Tail: {}", cfg.timeout_secs, tail));
        } else if a == 0 && b <= 1 {
            diag.push(format!("custom test command ran (exit {:?}) but parsed no test counts. Tail: {}", code, tail));
        } else {
            diag.push(format!("custom: {a}/{b}"));
        }
        (a, b)
    };
    prov.remove();
    (p, t, diag)
}

/// Stats returned by `evaluate_batched` so the caller can log dedup hit rate.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatchStats { pub attempts: usize, pub unique: usize, pub tests_run: usize }

/// Provision a worktree ONCE, then run `k` in-place mutate / hash / dedup / score
/// cycles against it, restoring touched files between attempts. This is the
/// "collect repeated workspaces" idea: at high k, most random mutations produce
/// workspaces we've already evaluated (no-ops, identical small fixes, etc.) — we
/// only test each unique workspace once per batch, and only once across the run
/// if memoize is on. Returns the best fitness over all attempts plus stats.
/// A tiny strided 1-D convolution over a source file's byte stream, producing a
/// fixed-size fingerprint vector. Fixed pseudo-random kernels make this a
/// locality-sensitive projection: similar code maps to a similar vector, very
/// different code maps far apart — without running any tests. The stride
/// downsamples (the "strided" part), so the cost is sub-linear and the output is
/// length-independent. Intended as the cheap signal behind a quality-diversity
/// bonus (reward novelty vs. recent survivors) or a surrogate pre-filter that
/// skips obviously-redundant candidates before paying for a test run.
fn code_fingerprint(text: &str, dim: usize) -> Vec<f32> {
    let bytes = text.as_bytes();
    let dim = dim.max(1);
    const K: usize = 8;       // kernel width
    const STRIDE: usize = 4;  // > 1 → downsample
    let mut acc = vec![0f32; dim];
    let mut windows = 0u32;
    let mut pos = 0usize;
    while pos + K <= bytes.len() {
        for (d, a) in acc.iter_mut().enumerate() {
            let mut s = 0f32;
            for j in 0..K {
                s += kernel_weight(d, j) * (bytes[pos + j] as f32 / 255.0);
            }
            *a += s.tanh(); // pool windows with a squashing nonlinearity
        }
        windows += 1;
        pos += STRIDE;
    }
    // Tiny inputs (< one window) still get a signal from a single padded pass.
    if windows == 0 && !bytes.is_empty() {
        for (d, a) in acc.iter_mut().enumerate() {
            let mut s = 0f32;
            for j in 0..K { s += kernel_weight(d, j) * (*bytes.get(j).unwrap_or(&0) as f32 / 255.0); }
            *a += s.tanh();
        }
    }
    let norm = acc.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    for a in acc.iter_mut() { *a /= norm; }
    acc
}

/// Deterministic per-(dim, tap) kernel weight in [-1, 1] — a cheap fixed random
/// projection, no training required.
fn kernel_weight(d: usize, j: usize) -> f32 {
    let h = (d.wrapping_mul(2654435761).wrapping_add(j.wrapping_mul(40503)).wrapping_add(0x9E37)) as u32;
    ((h % 2001) as f32 / 1000.0) - 1.0
}

/// L2 distance between two fingerprints (both already unit-normalized).
fn fingerprint_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

/// Per-bot fingerprint of the *code change* a real-backend bot made this generation,
/// keyed by bot id. Populated by `evaluate_real`/`evaluate_recurrent`, read during
/// selection so the novelty bonus rewards "different code," not just a different
/// genome scalar. Process-global (same pattern as the duration cache) to avoid
/// threading another parameter through the hot eval path.
fn fingerprint_store() -> &'static std::sync::Mutex<std::collections::HashMap<usize, Vec<f32>>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<usize, Vec<f32>>>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Fingerprint the concatenated *new* contents of files the bot changed (vs the
/// source workspace). Returns an empty vec when nothing changed, so a no-op edit
/// earns no novelty bonus (we don't want to reward doing nothing for being "novel").
const FP_DIM: usize = 32;
fn worktree_change_fingerprint(orig: &Path, work: &Path) -> Vec<f32> {
    let mut changed = Vec::new();
    diff_walk(orig, work, Path::new(""), &mut changed);
    if changed.is_empty() { return Vec::new(); }
    let mut blob = String::new();
    for rel in changed.iter().take(8) {
        if let Ok(s) = std::fs::read_to_string(work.join(rel)) {
            blob.push_str(&s);
            if blob.len() > 64 * 1024 { break; }
        }
    }
    if blob.is_empty() { Vec::new() } else { code_fingerprint(&blob, FP_DIM) }
}

/// Snapshot the current contents of every source file (in-memory). Used by the
/// recurrent bot to revert a step that didn't improve. Source files are small text,
/// so a full in-memory snapshot is cheap; the mutator never creates new files (it
/// edits lines within an existing one), so restoring the snapshotted set is exact.
fn snapshot_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    collect_source_files(dir).into_iter()
        .filter_map(|f| std::fs::read_to_string(&f).ok().map(|c| (f, c)))
        .collect()
}
fn restore_sources(snap: &[(PathBuf, String)]) {
    for (f, c) in snap { let _ = atomic_write(f, c); }
}

/// Recurrent ("RNN") bot: a bounded iterative hill-climb inside ONE evaluation.
/// Where the batched sampler takes k independent shots from the ORIGINAL baseline
/// (no accumulation), the recurrent bot carries its best-so-far state across steps
/// and only reverts a step that fails to improve — so accepted edits stack. This is
/// the "loop ≤N steps, keep-if-improves" worker. It costs up to steps+1 scorings
/// per bot (mitigated by the cross-run memo), trading evaluation budget for the
/// ability to actually accumulate improvements, which one-shot mutation cannot do.
fn evaluate_recurrent(cfg: &Config, ws: &Path, bot_id: usize, _z: f64, container: &str,
                      run_root: &Path, rng: &mut impl Rng,
                      memo: Option<&Mutex<HashMap<u64, (u32, u32)>>>, slot: usize, steps: usize)
    -> (f64, BatchStats)
{
    let target = run_root.join(format!("bot-{bot_id}"));
    let prov = match provision_worktree(cfg, ws, &target) {
        Ok(p) => p, Err(_) => return (0.0, BatchStats::default()),
    };
    let work = prov.path().to_path_buf();
    let mut stats = BatchStats::default();

    // Score the current worktree state, going through the memo so repeated states
    // (including the unmutated baseline shared across bots) don't re-run tests.
    // Returns raw (passed, total); the caller pins the denominator (see below).
    let score_state = |work: &Path, stats: &mut BatchStats| -> (u32, u32) {
        let h = worktree_hash(work);
        if let Some(memo) = memo {
            if let Some(&(p, t)) = memo.lock().unwrap().get(&h) {
                return (p, t);
            }
        }
        let (p, t) = score_worktree(cfg, container, work, slot);
        stats.tests_run += 1;
        if let Some(memo) = memo { memo.lock().unwrap().insert(h, (p, t)); }
        (p, t)
    };

    // Baseline scoring (verbose, so we can see which tests failed and localize).
    let (bp, bt, base_out) = score_worktree_out(cfg, container, &work, slot);
    stats.tests_run += 1;
    if let Some(memo) = memo { memo.lock().unwrap().insert(worktree_hash(&work), (bp, bt)); }
    let base_total = bt.max(1);
    let frac = |p: u32, t: u32| -> f64 { p as f64 / base_total.max(t) as f64 };

    // Fault-localization prior: bias mutations toward code that shares vocabulary
    // with the FAILING tests (static lexical query) AND toward code the persistent
    // model has LEARNED is associated with that vocabulary (transfers across apps).
    let failing_names = failing_test_names(&base_out);
    let mut failing_prev: std::collections::HashSet<String> = failing_names.iter().cloned().collect();
    let query = localization_query(&work, &failing_names);
    // test-vocabulary tokens (keys into the learned model) — names only
    let mut test_tokens = std::collections::HashSet::new();
    for n in &failing_names { ident_tokens(n, &mut test_tokens); }
    // snapshot the learned associations once (no lock contention in the hot loop)
    let learned = localizer().lock().map(|m| m.expand(&test_tokens)).unwrap_or_default();

    let mut best = frac(bp, bt);
    // Adaptive multi-edit (basin hopping). A greedy single-edit climb can't cross a
    // FITNESS VALLEY — a defect needing two coordinated edits where neither alone
    // improves the score. So when the climb STALLS (no improvement for a while), we
    // escalate the "kick": apply a bundle of N edits before scoring, accepting only
    // if the whole bundle improves. Easy single-edit defects are still solved cheaply
    // at N=1; the cost only rises when stuck.
    const STALL_WINDOW: usize = 12;
    let max_bundle = std::env::var("ARENA_MAX_BUNDLE").ok()
        .and_then(|s| s.parse::<usize>().ok()).unwrap_or(4).max(1);
    let mut stall = 0usize;
    for _ in 0..steps {
        if best >= 1.0 { break; } // already perfect — nothing left to climb toward
        // Escalate the *potential* kick size as we stall, but keep single edits
        // DOMINANT even when stalled: most steps stay k=1 (so independent defects,
        // which need exactly one edit, aren't forced into a bundle that must also
        // touch — and break — an already-correct site), and only ~1/3 of stalled
        // steps fire the larger coordinated kick that crosses valleys.
        let escalated = 1 + (stall / STALL_WINDOW).min(max_bundle - 1);
        let bundle = if escalated > 1 && rng.gen::<f64>() < 0.35 { escalated } else { 1 };
        let snap = snapshot_sources(&work);
        // a directed bundle of `bundle` DISTINCT hot sites (k=1 is an ordinary edit)
        let edited = mutate_bundle(&work, &cfg.ops, cfg.aggression, rng, &query, &learned, bundle, !cfg.allow_test_edits);
        stats.attempts += 1;
        if edited.is_empty() { stall += 1; continue; }

        if !cfg.allow_test_edits && touched_test_file(ws, &work) {
            record_security_event(bot_id, "test-edit", "recurrent step touched a test; reverted");
            restore_sources(&snap);
            stall += 1;
            continue;
        }
        if let Some(pat) = diff_introduces_danger(ws, &work) {
            record_security_event(bot_id, "danger-pattern", &format!("recurrent step introduced `{pat}`; reverted"));
            restore_sources(&snap);
            stall += 1;
            continue;
        }

        let (cp, ct) = score_state(&work, &mut stats);
        let cand = frac(cp, ct);
        // Accept strict improvements always. Also accept SCORE-NEUTRAL moves with
        // some probability ("neutral drift"): fixing one half of a coupled defect
        // often doesn't change the score yet, but lets the OTHER half become a strict
        // improvement next step — this is how the climb walks across the flat floor
        // of a fitness valley instead of stalling at the edge.
        const NEUTRAL_ACCEPT_PROB: f64 = 0.30;
        let neutral_prob = std::env::var("ARENA_NEUTRAL_PROB").ok()
            .and_then(|s| s.parse::<f64>().ok()).unwrap_or(NEUTRAL_ACCEPT_PROB);
        let improved = cand > best + 1e-9;
        let neutral = !improved && cand >= best - 1e-9 && rng.gen::<f64>() < neutral_prob;
        if improved {
            best = cand;        // accept: the whole bundle persists into the next step
            stats.unique += 1;  // (here `unique` = accepted improvements)
            stall = 0;          // back to cheap single edits
            // LEARN: which tests did this bundle resolve? Reinforce the model with
            // the vocabulary of every edited line (we don't know which edit mattered,
            // so credit them all) so this localization transfers to future repairs.
            let (_, _, out2) = score_worktree_out(cfg, container, &work, slot);
            let failing_now: std::collections::HashSet<String> =
                failing_test_names(&out2).into_iter().collect();
            let resolved: Vec<String> = failing_prev.difference(&failing_now).cloned().collect();
            if !resolved.is_empty() {
                let mut resolved_toks = std::collections::HashSet::new();
                for r in &resolved { ident_tokens(r, &mut resolved_toks); }
                let mut code_toks = std::collections::HashSet::new();
                for (f, li) in &edited {
                    if let Ok(t) = std::fs::read_to_string(f) {
                        if let Some(line) = t.lines().nth(*li) {
                            code_toks.extend(line_tokens(&strip_comment(line)));
                        }
                    }
                }
                if let Ok(mut m) = localizer().lock() {
                    m.reinforce(&resolved_toks, &code_toks, 1.0);
                }
            }
            failing_prev = failing_now;
        } else if neutral {
            // persist the score-neutral state (enables crossing) but keep counting
            // toward stall so the multi-edit kick still escalates if drift isn't enough.
            stall += 1;
        } else {
            restore_sources(&snap); // reject: revert the whole bundle
            stall += 1;
        }
    }
    prov.remove();
    (best, stats)
}

fn evaluate_batched(cfg: &Config, ws: &Path, bot_id: usize, z: f64, container: &str,
                    run_root: &Path, rng: &mut impl Rng,
                    memo: Option<&Mutex<HashMap<u64, (u32, u32)>>>, slot: usize, k: usize)
    -> (f64, BatchStats)
{
    // RNN bot mode: hill-climb instead of independent sampling. A configured patch
    // proposer overrides it — a model's patch is a single coherent action, not a
    // sequence of random hill-climb steps.
    if cfg.recurrent_steps > 0 && cfg.proposer_cmd.is_empty() {
        return evaluate_recurrent(cfg, ws, bot_id, z, container, run_root, rng, memo, slot, cfg.recurrent_steps);
    }
    let k = k.max(1);
    if k == 1 {
        // Preserve the cheap path for the common k=1 case (no extra work).
        return (evaluate_real(cfg, ws, bot_id, z, container, run_root, rng, memo, slot),
                BatchStats { attempts: 1, unique: 1, tests_run: 1 });
    }
    let target = run_root.join(format!("bot-{bot_id}"));
    let prov = match provision_worktree(cfg, ws, &target) {
        Ok(p) => p,
        Err(_) => return (0.0, BatchStats::default()),
    };
    let work = prov.path().to_path_buf();
    let mut best = 0.0f64;
    let mut seen_this_batch: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut stats = BatchStats::default();

    for _ in 0..k {
        let _changed = worker_attempt(&work, z, &cfg.test_cmd, &cfg.ops, cfg.aggression, rng);
        stats.attempts += 1;

        // Tripwire short-circuits the unique counter — a test-file edit is never
        // a candidate, so it shouldn't burn our "unique workspaces" budget.
        if !cfg.allow_test_edits && touched_test_file(ws, &work) {
            record_security_event(bot_id, "test-edit", "mutation altered a test file/inline test block; scored 0");
            restore_workspace(ws, &work);
            continue;
        }
        if let Some(pat) = diff_introduces_danger(ws, &work) {
            record_security_event(bot_id, "danger-pattern", &format!("introduced `{pat}`; scored 0"));
            restore_workspace(ws, &work);
            continue;
        }

        let h = worktree_hash(&work);
        if !seen_this_batch.insert(h) {
            restore_workspace(ws, &work);
            continue; // already evaluated this exact workspace earlier in the batch
        }
        stats.unique += 1;

        // Cross-run memo: a workspace seen in a previous bot/generation is already
        // scored — reuse, no test run.
        if let Some(memo) = memo {
            if let Some(&(p, t)) = memo.lock().unwrap().get(&h) {
                let f = if t == 0 { 0.0 } else { p as f64 / t as f64 };
                if f > best { best = f; }
                restore_workspace(ws, &work);
                continue;
            }
        }

        // Truly novel workspace → run the tests.
        let (passed, total) = score_worktree(cfg, container, &work, slot);
        stats.tests_run += 1;
        if let Some(memo) = memo {
            memo.lock().unwrap().insert(h, (passed, total));
        }
        let f = if total == 0 { 0.0 } else { passed as f64 / total as f64 };
        if f > best { best = f; }
        restore_workspace(ws, &work);
    }
    prov.remove();
    (best, stats)
}

/// Restore a worktree to match `source_ws` by copying back any file that differs.
/// Cheap in the common case (only one file changed); the diff walker already
/// exists for the tripwire.
fn restore_workspace(source_ws: &Path, work: &Path) {
    let mut changed = Vec::new();
    diff_walk(source_ws, work, Path::new(""), &mut changed);
    for rel in changed {
        let src = source_ws.join(&rel);
        let dst = work.join(&rel);
        if src.exists() {
            if let Ok(bytes) = std::fs::read(&src) {
                let _ = atomic_write(&dst, &String::from_utf8_lossy(&bytes));
            }
        } else {
            // File only exists in work (mutator created one) → remove it.
            let _ = std::fs::remove_file(&dst);
        }
    }
}

fn evaluate_real(cfg: &Config, ws: &Path, bot_id: usize, z: f64, container: &str, run_root: &Path, rng: &mut impl Rng, memo: Option<&Mutex<HashMap<u64, (u32, u32)>>>, slot: usize) -> f64 {
    let ws = ws.to_path_buf();
    let target = run_root.join(format!("bot-{bot_id}"));
    let prov = match provision_worktree(cfg, &ws, &target) {
        Ok(p) => p,
        Err(_) => return 0.0,
    };
    let work = prov.path().to_path_buf();

    // Proposer cascade (the coding-ecosystem path): try the chain in order — fast/cheap
    // model first — and escalate to the next tier only when the current one didn't (near-)
    // fully solve it. The engine's own tests are the router: a tier "wins" if its patch
    // scores best, and we stop as soon as one essentially solves it, so the expensive
    // model is only ever invoked on the cases the cheap one couldn't handle.
    let chain = proposer_chain(cfg);
    if !chain.is_empty() {
        const SOLVED: f64 = 0.999; // "good enough to not escalate"
        // Assemble context once on the pristine worktree (identical input for every tier).
        let (mut ctx, failing) = assemble_context(&work, cfg, container, slot);
        // Enrichment chain: each stage is a context→context transform (RAG retrieval of
        // prior fixes / relevant symbols, a planner that injects a `plan`, etc.). Run once
        // here so every cascade tier sees the same enriched context. A stage that fails or
        // returns nothing leaves the context unchanged.
        for stage in &cfg.context_cmds {
            if stage.is_empty() { continue; }
            if let Some(enriched) = run_proposer(&work, stage, &ctx) { ctx = enriched; }
        }
        let mut best_score = 0.0f64;
        let mut best_patch: Option<String> = None;
        let mut best_tier = 0usize;
        for (tier, cmd) in chain.iter().enumerate() {
            if tier > 0 { restore_workspace(&ws, &work); } // revert before the next tier tries
            let applied = match run_proposer(&work, cmd, &ctx) {
                Some(diff) if apply_patch_gated(&work, &diff) => Some(diff),
                _ => None,
            };
            // Reuse the tripwires: a proposer that edits tests or introduces danger scores 0.
            let score = if !cfg.allow_test_edits && touched_test_file(&ws, &work) {
                record_security_event(bot_id, "test-edit", "proposer patch altered a test; scored 0");
                0.0
            } else if let Some(pat) = diff_introduces_danger(&ws, &work) {
                record_security_event(bot_id, "danger-pattern", &format!("proposer introduced `{pat}`; scored 0"));
                0.0
            } else {
                score_state(cfg, &ws, container, &work, run_root, slot)
            };
            if score > best_score { best_score = score; best_patch = applied; best_tier = tier; }
            if score >= SOLVED { break; } // cheap tier nailed it — don't pay for escalation
        }
        // Only credit a tier (and harvest the patch) when a real win occurred — a patch
        // that applied and scored clearly positive. A no-op/rejected patch isn't a win.
        let won = best_patch.is_some() && best_score >= 0.6;
        if won { record_tier(best_tier); }
        if cfg.novelty_lambda > 0.0 {
            let fp = worktree_change_fingerprint(&ws, &work);
            fingerprint_store().lock().unwrap().insert(bot_id, fp);
        }
        if won {
            if let Some(diff) = &best_patch { log_experience(&ws, &failing, diff, best_score); }
        }
        prov.remove();
        return best_score;
    }

    // Random-mutation / local-LLM baseline (no proposer configured).
    let _changed = worker_attempt(&work, z, &cfg.test_cmd, &cfg.ops, cfg.aggression, rng);

    // Reward-hack tripwire: if the bot's edit landed in a test file (or it deleted
    // assertions), refuse to score. Skipped when the user has legit test-editing on.
    if !cfg.allow_test_edits && touched_test_file(&ws, &work) {
        record_security_event(bot_id, "test-edit", "mutation altered a test file/inline test block; scored 0 and culled");
        prov.remove();
        return 0.0;
    }
    // Defense-in-depth: a mutation that introduces an obviously destructive
    // construct is never a legitimate fix — score 0 (bubblewrap would contain the
    // damage anyway, but this stops it earlier and keeps it out of the gene pool).
    if let Some(pat) = diff_introduces_danger(&ws, &work) {
        record_security_event(bot_id, "danger-pattern", &format!("introduced `{pat}`; scored 0 and culled"));
        prov.remove();
        return 0.0;
    }

    // Record a fingerprint of the change for the quality-diversity bonus (real
    // backends compare *code shape*, not the abstract genome). Cheap; reads only
    // the changed files.
    if cfg.novelty_lambda > 0.0 {
        let fp = worktree_change_fingerprint(&ws, &work);
        fingerprint_store().lock().unwrap().insert(bot_id, fp);
    }

    // Reuse the score if we've already evaluated this exact repo state.
    let key = if cfg.memoize { Some(worktree_hash(&work)) } else { None };
    if let (Some(k), Some(memo)) = (key, memo) {
        if let Some(&(p, t)) = memo.lock().unwrap().get(&k) {
            prov.remove();
            return if t == 0 { 0.0 } else { p as f64 / t as f64 };
        }
    }

    let (passed, total) = score_worktree(cfg, container, &work, slot);
    if let (Some(k), Some(memo)) = (key, memo) {
        memo.lock().unwrap().insert(k, (passed, total));
    }
    // Differential scoring: reward fixing baseline-failing tests, punish regressions
    // (asymmetrically). Falls back silently to pass-fraction if we couldn't parse
    // names (e.g. an unrecognised runner) — detectable as an empty baseline + a
    // clean run, which yields exactly 0.5; to avoid that ambiguity we only switch to
    // differential when the baseline actually had failing tests to fix.
    if cfg.differential {
        let baseline = compute_baseline(cfg, &ws, container, run_root);
        if !baseline.is_empty() {
            let (_p2, _t2, out) = score_worktree_out(cfg, container, &work, slot);
            let now = parse_failing_names(&out);
            let reward = differential_reward(&baseline, &now, 2.0);
            prov.remove();
            return reward;
        }
    }
    prov.remove(); // worktree cleaned immediately after scoring
    if total == 0 { 0.0 } else { passed as f64 / total as f64 }
}

fn provision(cfg: &Config, tx: &async_channel::Sender<EngineMsg>) -> Vec<String> {
    let n_containers = container_count(cfg.population, cfg.workers_per_container);
    let mut handles = Vec::with_capacity(n_containers);
    match cfg.backend {
        BackendKind::Sim => {
            log(tx, format!(
                "[sim] provisioning {} virtual containers for {} bots ({} workers each)",
                n_containers, cfg.population, cfg.workers_per_container
            ));
            for c in 0..n_containers {
                handles.push(format!("sim-container-{c}"));
            }
        }
        BackendKind::Docker => {
            log(tx, format!("[docker] image='{}', spawning {} containers", cfg.image, n_containers));
            let ws = cfg.workspace.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| ".".into());
            for c in 0..n_containers {
                let name = format!("bot-arena-{c}");
                let out = std::process::Command::new("docker")
                    .args(["create", "--name", &name, "-v", &format!("{ws}:/work"), "-w", "/work", &cfg.image, "sleep", "infinity"])
                    .output();
                match out {
                    Ok(o) if o.status.success() => {
                        let _ = std::process::Command::new("docker").args(["start", &name]).output();
                        log(tx, format!("[docker] started {name}"));
                        handles.push(name);
                    }
                    Ok(o) => log(tx, format!("[docker] FAILED {name}: {}", String::from_utf8_lossy(&o.stderr).trim())),
                    Err(e) => log(tx, format!("[docker] cannot exec docker: {e}")),
                }
            }
        }
        BackendKind::Conda => {
            log(tx, format!("[conda] creating {} environments", n_containers));
            for c in 0..n_containers {
                let prefix = format!("/tmp/bot-arena/env-{c}");
                let out = std::process::Command::new("conda").args(["create", "-y", "-p", &prefix, "python=3.12"]).output();
                match out {
                    Ok(o) if o.status.success() => { log(tx, format!("[conda] created {prefix}")); handles.push(prefix); }
                    Ok(o) => log(tx, format!("[conda] FAILED {prefix}: {}", String::from_utf8_lossy(&o.stderr).trim())),
                    Err(e) => log(tx, format!("[conda] cannot exec conda: {e}")),
                }
            }
        }
        BackendKind::Local => {
            log(tx, format!(
                "[local] host evaluation for {} bots (each gets an isolated worktree copy)",
                cfg.population
            ));
            handles.push("local-host".to_string());
        }
    }
    handles
}

fn teardown(cfg: &Config, handles: &[String], run_root: &Path, tx: &async_channel::Sender<EngineMsg>) {
    match cfg.backend {
        BackendKind::Docker => {
            for name in handles {
                let _ = std::process::Command::new("docker").args(["rm", "-f", name]).output();
            }
            log(tx, format!("[docker] removed {} containers", handles.len()));
        }
        BackendKind::Conda => {
            for prefix in handles {
                let _ = std::process::Command::new("conda").args(["env", "remove", "-y", "-p", prefix]).output();
                let _ = std::fs::remove_dir_all(prefix); // ensure nothing is left in /tmp
            }
            log(tx, format!("[conda] removed {} environments", handles.len()));
        }
        _ => {}
    }
    // Remove this run's worktree directory (per-bot copies live under it).
    if run_root.exists() {
        let _ = std::fs::remove_dir_all(run_root);
    }
    // If this was the last run for the process, drop the now-empty parent too.
    let parent = work_root();
    if let Ok(mut entries) = std::fs::read_dir(&parent) {
        if entries.next().is_none() {
            let _ = std::fs::remove_dir_all(&parent);
        }
    }
}

/// Population loop with latent purpose, niche-diverse selection, and a coordinator.
pub fn run_engine(
    cfg: Config,
    tx: async_channel::Sender<EngineMsg>,
    stop: Arc<AtomicBool>,
    axis: Arc<AtomicU32>,
) {
    let seed = run_seed(&cfg);
    let mut rng = seeded_rng(seed);
    log(&tx, format!("run seed: {seed}  (set this seed to reproduce the run)"));
    // Animation pacing: skip the per-generation sleeps in turbo mode or for large
    // populations (where the visual wave is meaningless and the delay dominates).
    let pace_ms: u64 = if cfg.turbo || cfg.population > 500 { 0 } else { 110 };
    let niches = cfg.niches.max(1);
    let labels = niche_labels(&cfg, &tx);
    let purpose = |z: f64| labels[niche_of(z, niches)].clone();

    // Per-run worktree dir, unique even across concurrent runs in one process.
    let run_stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let run_root = work_root().join(format!("run-{run_stamp}"));

    // Persisted score cache: reuse results for repo states we've already scored.
    let memo: Mutex<HashMap<u64, (u32, u32)>> = Mutex::new(if cfg.memoize { load_scores() } else { HashMap::new() });

    // Initialize: genomes low (to evolve up), purposes spread across the latent axis.
    let mut bots: Vec<Bot> = (0..cfg.population)
        .map(|id| {
            let z = (id as f64 + 0.5) / cfg.population as f64;
            Bot { id, genome: rng.gen_range(0.05..0.65), z, fitness: 0.0, alive: true }
        })
        .collect();

    // Resume from checkpoint: overwrite the matching prefix of the freshly-seeded
    // population with the saved bots. Extra fresh bots stay (for population growth);
    // saved bots beyond cfg.population are ignored (for shrinkage).
    if cfg.resume {
        match load_checkpoint() {
            Some(ckpt) => {
                let mut applied = 0usize;
                for (sid, sz, sg, sf, sa) in &ckpt.bots {
                    if let Some(b) = bots.get_mut(*sid) {
                        b.z = *sz; b.genome = *sg; b.fitness = *sf; b.alive = *sa;
                        applied += 1;
                    }
                }
                log(&tx, format!("resumed from checkpoint: {applied}/{} bots restored (saved seed was {})",
                    ckpt.bots.len(), ckpt.seed));
            }
            None => log(&tx, "resume requested but no checkpoint found; starting fresh".to_string()),
        }
    }

    log(&tx, format!(
        "starting: backend={}, population={}, niches={}, keep={:.0}%, generations={}, coordinator={}",
        match cfg.backend { BackendKind::Sim => "simulation", BackendKind::Docker => "docker", BackendKind::Conda => "conda", BackendKind::Local => "local" },
        cfg.population, niches, cfg.keep_fraction * 100.0, cfg.generations, cfg.coordinator
    ));
    // Full config block — paste this anywhere to reproduce a run exactly.
    log(&tx, "─── config (paste to reproduce) ───────────────────────────────");
    log(&tx, format!("  seed={}  backend={:?}  workspace={}",
        seed, cfg.backend, cfg.workspace.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "-".into())));
    if !cfg.workspaces.is_empty() {
        log(&tx, format!("  workspaces=[{}]",
            cfg.workspaces.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
    }
    log(&tx, format!("  population={}  generations={}  niches={}  keep_fraction={}  coordinator={}",
        cfg.population, cfg.generations, cfg.niches, cfg.keep_fraction, cfg.coordinator));
    log(&tx, format!("  test_cmd={:?}  timeout_secs={}  max_file_mb={}  cache={}  fast_clone={}",
        cfg.test_cmd, cfg.timeout_secs, cfg.max_file_mb, cfg.cache, cfg.fast_clone));
    log(&tx, format!("  threads={}  workers_per_container={}  memoize={}  crossover={}  shelter={}",
        cfg.threads, cfg.workers_per_container, cfg.memoize, cfg.crossover, cfg.sandbox));
    log(&tx, format!("  batch_start={}  turbo={}  allow_test_edits={}  aggression={}",
        cfg.batch_start, cfg.turbo, cfg.allow_test_edits, cfg.aggression));
    if !cfg.cargo_features.trim().is_empty() || !cfg.build_flags.trim().is_empty() {
        log(&tx, format!("  cargo_features={:?}  build_flags={:?}", cfg.cargo_features, cfg.build_flags));
    }
    if cfg.recurrent_steps > 0 {
        log(&tx, format!("  recurrent_steps={} (RNN bots: hill-climb per eval)", cfg.recurrent_steps));
    }
    log(&tx, format!("  ops=[{}]", cfg.ops.join(",")));
    log(&tx, format!("  tools=[{}]", cfg.tools.join(",")));
    log(&tx, "───────────────────────────────────────────────────────────────");

    for b in &bots {
        let _ = tx.send_blocking(EngineMsg::Bot { id: b.id, status: BotStatus::Provisioning, fitness: 0.0, z: b.z, purpose: purpose(b.z) });
    }
    let handles = provision(&cfg, &tx);
    if cfg.backend != BackendKind::Sim && handles.is_empty() {
        log(&tx, "no containers/envs provisioned; aborting.");
        let _ = tx.send_blocking(EngineMsg::Done);
        return;
    }
    for b in &bots {
        let _ = tx.send_blocking(EngineMsg::Bot { id: b.id, status: BotStatus::Idle, fitness: 0.0, z: b.z, purpose: purpose(b.z) });
    }

    if cfg.backend == BackendKind::Sim {
        log(&tx, "[sim] fitness is a closed-form model — NO builds or tests run. A full run is");
        log(&tx, "[sim] just arithmetic, so it completes in well under a second by design;");
        log(&tx, "[sim] an instant finish is expected, not a hang or a crash.");
        if cfg.recurrent_steps > 0 {
            log(&tx, format!("[sim] note: recurrent_steps={} is IGNORED in Sim — RNN hill-climb needs a", cfg.recurrent_steps));
            log(&tx, "[sim] real workspace to score against. Switch backend to Local on a project");
            log(&tx, "[sim] that compiles to actually exercise the recurrent bots.");
        }
    }

    // Report what 'auto' will test, so the reward signal is transparent — and
    // catch a broken harness (missing tools / no build system) before grinding
    // out generations of all-zeros that look like the search failing.
    if cfg.backend != BackendKind::Sim {
        let auto = { let c = cfg.test_cmd.trim(); c.is_empty() || c.eq_ignore_ascii_case("auto") };
        if auto {
            if let Some(ws) = &cfg.workspace {
                let sys = detect_build_systems(ws);
                if sys.is_empty() {
                    log(&tx, "harness error: no known build system found in the workspace. Set a Test command, or check the path. Aborting.");
                    let _ = tx.send_blocking(EngineMsg::Done);
                    return;
                }
                let names: Vec<String> = sys.iter().map(|(s, d)| {
                    let rel = d.strip_prefix(ws).ok().map(|p| p.display().to_string()).unwrap_or_default();
                    if rel.is_empty() { s.label().to_string() } else { format!("{}@{}", s.label(), rel) }
                }).collect();
                log(&tx, format!("auto-detect: found [{}]; reward = total passed / total tests", names.join(", ")));
            }
            if cfg.cache {
                log(&tx, "build cache on: ccache + shared CARGO_TARGET_DIR (incremental) under the registry");
            }
        }
        // Tool preflight (auto or literal).
        let missing = missing_tools(&cfg);
        if !missing.is_empty() {
            let runnable_remaining = if auto {
                cfg.workspace.as_ref().map(|ws|
                    detect_build_systems(ws).iter().any(|(s, _)| which(primary_tool(*s)))
                ).unwrap_or(false)
            } else { false };
            if runnable_remaining {
                log(&tx, format!("harness warning: not on PATH: {} — those systems will be skipped.", missing.join(", ")));
            } else {
                log(&tx, format!("harness error: required command(s) not on PATH: {} — nothing to run. Aborting.", missing.join(", ")));
                let _ = tx.send_blocking(EngineMsg::Done);
                return;
            }
        }
        if cfg.sandbox {
            if which("bwrap") {
                log(&tx, "sandbox on: commands confined (no network; writes limited to the worktree + cache; cd can't escape)");
            } else {
                log(&tx, "shelter mode requested but `bwrap` not found — running UNCONFINED. Install bubblewrap to enable.");
            }
        }
        if cfg.threads > 1 {
            log(&tx, format!("parallel eval: {} workers, each with its own CARGO_TARGET_DIR (more disk, no target-lock contention)", cfg.threads.clamp(1, 100)));
        }
        if cfg.fast_clone {
            if let Some(ws) = &cfg.workspace {
                if ws.join(".git").exists() && which("git") {
                    log(&tx, "fast clone on: per-bot git worktrees (committed HEAD state, shared object store)");
                }
            }
        }
    }

    // Coordinator focus: starts at the knob; if coordinator is on it then learns.
    let mut targets = effective_targets(&cfg);
    // Auto-reject: for real backends, drop workspaces with NO build system the arena
    // can run (we could never score them), and note any recognized-but-unsupported
    // systems. This keeps a big multi-repo swarm from wasting its budget on code it
    // can't test, and tells you *why* a repo was skipped.
    if cfg.backend != BackendKind::Sim && targets.len() > 1 {
        let mut kept = Vec::new();
        let mut rejected = 0usize;
        for t in targets {
            let s = classify_workspace(&t);
            if s.runnable() {
                if !s.unsupported.is_empty() {
                    log(&tx, format!("note: {} also has unsupported build system(s) [{}] — testing the supported part only",
                        t.display(), s.unsupported.join(", ")));
                }
                kept.push(t);
            } else {
                rejected += 1;
                let why = if s.unsupported.is_empty() { "no recognized build system".to_string() }
                          else { format!("only unsupported: {}", s.unsupported.join(", ")) };
                if rejected <= 8 { log(&tx, format!("skip: {} — {why}", t.display())); }
            }
        }
        if rejected > 0 {
            log(&tx, format!("auto-rejected {rejected} workspace(s) the arena can't test; {} kept.", kept.len()));
        }
        targets = kept;
    }
    if !targets.is_empty() { log(&tx, format!("training targets: {} project(s) — {}", targets.len(),
        targets.iter().take(5).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))); }

    // Baseline pre-flight (only relevant for real backends). This is the answer to
    // "best/mean stuck at 0 forever, what's wrong?": score the unmodified project
    // so the user can see the starting point and tell the difference between
    // "bots broke things" and "the harness sees nothing passing to begin with".
    if cfg.backend != BackendKind::Sim && !cfg.sandbox {
        log(&tx, "⚠  Shelter mode is OFF — bots run unconfined on the host. Their");
        log(&tx, "   test commands and any tools they invoke can write anywhere the");
        log(&tx, "   running user can. A denied-binary shadow PATH still blocks the");
        log(&tx, "   worst offenders (rm/dd/curl/sudo/etc.), but a clever command in");
        log(&tx, "   an allowed binary (cargo build.rs, pytest conftest.py) can still");
        log(&tx, "   reach the host. Turn Shelter mode ON for filesystem confinement.");
    }
    if cfg.backend != BackendKind::Sim && cfg.sandbox {
        match shelter_smoke_test(&cfg, &run_root) {
            Ok(()) => log(&tx, "shelter smoke test passed: sandboxed commands work on this host."),
            Err(reason) => {
                log(&tx, "═══════════════════════════════════════════════════════════════");
                log(&tx, "SHELTER SMOKE TEST FAILED — every bot will score 0 until fixed.");
                for line in reason.lines() { log(&tx, format!("  {line}")); }
                log(&tx, "═══════════════════════════════════════════════════════════════");
            }
        }
    }
    if cfg.backend != BackendKind::Sim && !targets.is_empty() {
        let mut sum_frac = 0.0_f64;
        let mut zero_count = 0;
        let mut any_timeout = false;
        for ws in &targets {
            let (p, t, diag) = baseline_score(&cfg, ws, &run_root);
            let frac = if t == 0 { 0.0 } else { p as f64 / t as f64 };
            log(&tx, format!("baseline {}: {p}/{t} tests pass ({:.3})", ws.display(), frac));
            for d in &diag {
                log(&tx, format!("  ↳ {d}"));
                if d.contains("timed out") || d.contains("exit 124") { any_timeout = true; }
            }
            sum_frac += frac;
            if frac == 0.0 { zero_count += 1; }
        }
        let mean_baseline = sum_frac / targets.len() as f64;
        log(&tx, format!("baseline mean across targets: {mean_baseline:.3}"));
        // Differential scoring: remember which tests fail on each pristine target so
        // bots are scored on fixed−λ·broke. Pre-warmed here (serially) so the first
        // generation's parallel workers don't each recompute the same baseline.
        if cfg.differential {
            let container = handles.first().cloned().unwrap_or_default();
            let mut total_failing = 0usize;
            for ws in &targets {
                let f = compute_baseline(&cfg, ws, &container, &run_root);
                total_failing += f.len();
            }
            if total_failing == 0 {
                log(&tx, "differential scoring ON, but no tests fail on any pristine target →");
                log(&tx, "  nothing to 'fix', so scoring falls back to pass-fraction. Point the");
                log(&tx, "  swarm at a project with failing tests to use differential rewards.");
            } else {
                log(&tx, format!("differential scoring ON: {total_failing} baseline-failing test(s) to fix (regressions penalized 2×)"));
            }
        }
        if zero_count == targets.len() {
            log(&tx, "═══════════════════════════════════════════════════════════════");
            log(&tx, "WARNING: every target scored 0 BEFORE any bot touched it.");
            log(&tx, "Every bot will therefore score 0 too. See the ↳ lines above");
            log(&tx, "for the specific reason from each system. Most common causes:");
            if any_timeout {
                log(&tx, format!("  → TIMEOUT (your case): tests killed at {}s before finishing.", cfg.timeout_secs));
                log(&tx, "     Raise 'Per-command timeout' — cargo test typically needs");
                log(&tx, "     15-60s even on a small project, more on a cold build cache.");
            }
            log(&tx, "  • `auto` detection found a build system but 0 tests in it");
            log(&tx, "  • a required tool (cargo / pytest / cmake / go) isn't on PATH");
            log(&tx, "  • the project's own tests fail on a clean checkout");
            log(&tx, "  • provisioning produced a broken copy (try fast_clone off)");
            log(&tx, "Try: run your test command manually in the workspace first.");
            log(&tx, "═══════════════════════════════════════════════════════════════");
        } else if mean_baseline > 0.0 {
            log(&tx, format!("bots that *don't break* the project will score around {mean_baseline:.2}; \
                only mutations that increase pass-fraction will exceed that."));
        }
    }

    let mut focus = read_axis(&axis);
    let mut sec_seen = security_counter().load(std::sync::atomic::Ordering::Relaxed);
    // Quality-diversity reference set: each generation's survivors become the next
    // generation's "stuff to be different from." Empty on gen 0 (no novelty bonus
    // until there's something to compare against).
    let mut prev_survivor_genomes: Vec<f64> = Vec::new();
    let mut prev_survivor_fps: Vec<Vec<f32>> = Vec::new(); // real-backend code fingerprints
    // Auditor state: prior best (trend), prior trend label, security baseline, and the
    // accumulating structured run report.
    let mut prev_audit_best: Option<f64> = None;
    let mut _last_trend: &'static str = "steady";
    let mut audit_sec_seen = security_counter().load(std::sync::atomic::Ordering::Relaxed);
    let mut audit_run: Vec<String> = Vec::new();

    for gen in 0..cfg.generations {
        if stop.load(Ordering::Relaxed) {
            log(&tx, "stop requested; halting.");
            break;
        }
        if !cfg.coordinator {
            focus = read_axis(&axis); // manual knob drives the focus
        }
        log(&tx, format!("--- generation {} (focus z={:.2} = '{}') ---", gen + 1, focus, purpose(focus)));

        // Sender side of the looking-glass contract: in turbo the engine outruns any
        // display and per-bot animation is meaningless, so we emit only per-generation
        // summaries and skip every per-bot send (the flood). Non-turbo keeps full per-bot
        // animation, which the receiver coalesces to 30 Hz.
        let animate = !cfg.turbo;

        // Evaluate. Workers near the focus get a deployment bonus: the coordinator
        // is "deploying" that purpose-region of the swarm.
        if cfg.backend == BackendKind::Sim {
            // Sim is embarrassingly parallel (no filesystem, no build lock), so
            // fan the population out across threads.
            let snapshot: Vec<(usize, usize, f64, f64)> = (0..bots.len())
                .filter(|&i| bots[i].alive)
                .map(|i| (i, bots[i].id, bots[i].z, bots[i].genome))
                .collect();
            let nthreads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
            let chunk = (snapshot.len() / nthreads).max(1);
            let results: Mutex<Vec<(usize, f64)>> = Mutex::new(Vec::with_capacity(snapshot.len()));
            let batch = batch_size(gen, cfg.generations, cfg.batch_start);
            let drift = cfg.drift;
            let show_activity = animate && cfg.population <= 200;
            std::thread::scope(|s| {
                for (ci, part) in snapshot.chunks(chunk).enumerate() {
                    let tx = tx.clone();
                    let results = &results;
                    let stop = &stop;
                    s.spawn(move || {
                        // deterministic per-(run, generation, chunk) seed: fast + reproducible
                        let mut trng = seeded_rng(seed ^ ((gen as u64) << 20) ^ (ci as u64 + 1));
                        let mut local = Vec::with_capacity(part.len());
                        for &(i, id, z, genome) in part {
                            if stop.load(Ordering::Relaxed) { break; }
                            if show_activity {
                                let _ = tx.send_blocking(EngineMsg::Activity { id, text: format!("evaluating · {}", purpose(z)) });
                            }
                            // best-of-k: more attempts early (batch decays to 1) raises
                            // the effective learning rate per generation
                            let mut fitness = f64::MIN;
                            for _ in 0..batch {
                                let noise: f64 = trng.gen_range(-0.04..0.04);
                                fitness = fitness.max(sim_fitness_at(genome, z, focus, noise, gen, drift));
                            }
                            if animate {
                                let _ = tx.send_blocking(EngineMsg::Bot { id, status: BotStatus::Running, fitness, z, purpose: purpose(z) });
                            }
                            local.push((i, fitness));
                        }
                        results.lock().unwrap().extend(local);
                    });
                }
            });
            for (i, f) in results.into_inner().unwrap() { bots[i].fitness = f; }
        } else {
            let preview = command_preview(&cfg);
            let nthreads = cfg.threads.clamp(1, 100);
            if nthreads <= 1 {
                for i in 0..bots.len() {
                    if stop.load(Ordering::Relaxed) { break; }
                    if !bots[i].alive { continue; }
                    let id = bots[i].id;
                    let z = bots[i].z;
                    let _ = tx.send_blocking(EngineMsg::Activity { id, text: format!("edit:{} → {}", op_for(z, &cfg.ops), preview) });
                    let _ = tx.send_blocking(EngineMsg::Bot { id, status: BotStatus::Running, fitness: bots[i].fitness, z, purpose: purpose(z) });
                    let container = handles.get(id / cfg.workers_per_container.max(1)).cloned().unwrap_or_default();
                    let k = batch_size(gen, cfg.generations, cfg.batch_start);
                    let ws_pb = if !targets.is_empty() { targets[id % targets.len()].clone() } else { cfg.workspace.clone().unwrap_or_default() };
                    let (fitness, bs) = evaluate_batched(&cfg, &ws_pb, id, z, &container, &run_root, &mut rng, Some(&memo), 0, k);
                    if cfg.recurrent_steps > 0 && bs.attempts > 0 {
                        let _ = tx.send_blocking(EngineMsg::Activity { id, text:
                            format!("hill-climb {} steps → {} kept, {} scored", bs.attempts, bs.unique, bs.tests_run) });
                    } else if k > 1 && bs.attempts > 0 {
                        let saved = bs.attempts.saturating_sub(bs.tests_run);
                        let _ = tx.send_blocking(EngineMsg::Activity { id, text:
                            format!("batch k={} → {} unique, {} tests run ({} dedup hits)",
                                bs.attempts, bs.unique, bs.tests_run, saved) });
                    }
                    bots[i].fitness = fitness;
                    let _ = tx.send_blocking(EngineMsg::Bot { id, status: BotStatus::Running, fitness, z, purpose: purpose(z) });
                }
            } else {
                // Parallel work queue: workers share a single AtomicUsize cursor and
                // pull the next bot when they're done with the previous one. This
                // load-balances naturally — a slot that gets a fast batch (lots of
                // dedup hits, quick fail) immediately grabs the next bot instead of
                // waiting for the slowest static chunk to finish.
                let alive: Vec<(usize, usize, f64)> = (0..bots.len())
                    .filter(|&i| bots[i].alive)
                    .map(|i| (i, bots[i].id, bots[i].z))
                    .collect();
                let results: Mutex<Vec<(usize, f64)>> = Mutex::new(Vec::with_capacity(alive.len()));
                let cursor = std::sync::atomic::AtomicUsize::new(0);
                std::thread::scope(|s| {
                    for slot in 0..nthreads {
                        let tx = tx.clone();
                        let results = &results;
                        let stop = &stop;
                        let cfg = &cfg;
                        let memo = &memo;
                        let run_root = &run_root;
                        let handles = &handles;
                        let preview = preview.clone();
                        let targets = targets.clone();
                        let cursor = &cursor;
                        let alive = &alive;
                        s.spawn(move || {
                            let mut trng = seeded_rng(seed ^ ((gen as u64) << 20) ^ ((slot as u64 + 1) << 8));
                            let mut local = Vec::new();
                            loop {
                                if stop.load(Ordering::Relaxed) { break; }
                                let idx = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if idx >= alive.len() { break; } // queue drained
                                let (i, id, z) = alive[idx];
                                let _ = tx.send_blocking(EngineMsg::Activity { id, text: format!("[t{slot}] edit:{} → {}", op_for(z, &cfg.ops), preview) });
                                let _ = tx.send_blocking(EngineMsg::Bot { id, status: BotStatus::Running, fitness: 0.0, z, purpose: purpose(z) });
                                let container = handles.get(id / cfg.workers_per_container.max(1)).cloned().unwrap_or_default();
                                let k = batch_size(gen, cfg.generations, cfg.batch_start);
                                let ws_pb = if !targets.is_empty() { targets[id % targets.len()].clone() } else { cfg.workspace.clone().unwrap_or_default() };
                                let (fitness, bs) = evaluate_batched(cfg, &ws_pb, id, z, &container, run_root, &mut trng, Some(memo), slot, k);
                                if cfg.recurrent_steps > 0 && bs.attempts > 0 {
                                    let _ = tx.send_blocking(EngineMsg::Activity { id, text:
                                        format!("[t{slot}] hill-climb {} steps → {} kept, {} scored", bs.attempts, bs.unique, bs.tests_run) });
                                } else if k > 1 && bs.attempts > 0 {
                                    let saved = bs.attempts.saturating_sub(bs.tests_run);
                                    let _ = tx.send_blocking(EngineMsg::Activity { id, text:
                                        format!("[t{slot}] batch k={} → {} unique, {} tests ({} dedup)",
                                            bs.attempts, bs.unique, bs.tests_run, saved) });
                                }
                                let _ = tx.send_blocking(EngineMsg::Bot { id, status: BotStatus::Running, fitness, z, purpose: purpose(z) });
                                local.push((i, fitness));
                            }
                            results.lock().unwrap().extend(local);
                        });
                    }
                });
                for (i, f) in results.into_inner().unwrap() { bots[i].fitness = f; }
            }
        }

        // Per-niche mean reward (for the coordinator).
        let mut niche_reward = vec![0.0f64; niches];
        let mut niche_count = vec![0usize; niches];
        for b in bots.iter().filter(|b| b.alive) {
            let nb = niche_of(b.z, niches);
            niche_reward[nb] += b.fitness;
            niche_count[nb] += 1;
        }
        for k in 0..niches {
            if niche_count[k] > 0 { niche_reward[k] /= niche_count[k] as f64; }
        }

        // MAP-Elites-style selection: keep the top fraction WITHIN each niche so
        // every purpose survives (diversity), rather than one global best winning.
        let alive_ids: Vec<usize> = (0..bots.len()).filter(|&i| bots[i].alive).collect();
        // Per-bot novelty. Real backends compare the *code* each bot produced
        // (fingerprint distance vs recent survivors); the Sim, which has no real
        // diff, falls back to genome-scalar distance. Either way: reward occupying
        // under-explored regions so crossover gets genuinely distinct parents.
        let novelty: Vec<f64> = if cfg.novelty_lambda <= 0.0 {
            vec![0.0; bots.len()]
        } else {
            let fps = fingerprint_store().lock().unwrap();
            let use_fp = !prev_survivor_fps.is_empty();
            bots.iter().map(|b| {
                if use_fp {
                    // mean fingerprint distance to survivors; no-op edits (empty fp) → 0
                    match fps.get(&b.id) {
                        Some(fp) if !fp.is_empty() => {
                            let s: f64 = prev_survivor_fps.iter()
                                .filter(|r| !r.is_empty())
                                .map(|r| fingerprint_distance(fp, r) as f64).sum();
                            let n = prev_survivor_fps.iter().filter(|r| !r.is_empty()).count();
                            if n > 0 { s / n as f64 } else { 0.0 }
                        }
                        _ => 0.0,
                    }
                } else if !prev_survivor_genomes.is_empty() {
                    let s: f64 = prev_survivor_genomes.iter().map(|g| (b.genome - g).abs()).sum();
                    s / prev_survivor_genomes.len() as f64
                } else { 0.0 }
            }).collect()
        };
        let sel_score = |i: usize| bots[i].fitness + cfg.novelty_lambda * novelty[i];

        let mut survivors: Vec<usize> = Vec::new();
        for k in 0..niches {
            let mut members: Vec<usize> = alive_ids.iter().copied().filter(|&i| niche_of(bots[i].z, niches) == k).collect();
            members.sort_by(|&a, &b| sel_score(b).partial_cmp(&sel_score(a)).unwrap());
            let keep_k = ((members.len() as f64) * cfg.keep_fraction).round().max(if members.is_empty() { 0.0 } else { 1.0 }) as usize;
            survivors.extend(members.iter().take(keep_k).copied());
        }
        if survivors.is_empty() {
            let mut a = alive_ids.clone();
            a.sort_by(|&x, &y| sel_score(y).partial_cmp(&sel_score(x)).unwrap());
            survivors = a.iter().take(1).copied().collect();
        }
        let survivor_set: std::collections::BTreeSet<usize> = survivors.iter().copied().collect();
        // Capture this generation's survivors as the next generation's reference set
        prev_survivor_genomes = survivors.iter().map(|&i| bots[i].genome).collect();
        if cfg.novelty_lambda > 0.0 {
            let fps = fingerprint_store().lock().unwrap();
            prev_survivor_fps = survivors.iter()
                .map(|&i| fps.get(&bots[i].id).cloned().unwrap_or_default())
                .collect();
        }

        let alive_n = alive_ids.len();
        let best = alive_ids.iter().map(|&i| bots[i].fitness).fold(0.0, f64::max);
        let mean = if alive_n > 0 { alive_ids.iter().map(|&i| bots[i].fitness).sum::<f64>() / alive_n as f64 } else { 0.0 };

        for &i in &alive_ids {
            let status = if survivor_set.contains(&i) { BotStatus::Passed } else { BotStatus::Failed };
            if animate {
                let _ = tx.send_blocking(EngineMsg::Bot { id: bots[i].id, status, fitness: bots[i].fitness, z: bots[i].z, purpose: purpose(bots[i].z) });
            }
        }
        if pace_ms > 0 { std::thread::sleep(Duration::from_millis(pace_ms)); }

        for &i in &alive_ids {
            if !survivor_set.contains(&i) {
                bots[i].alive = false;
                if animate {
                    let _ = tx.send_blocking(EngineMsg::Bot { id: bots[i].id, status: BotStatus::Culled, fitness: bots[i].fitness, z: bots[i].z, purpose: purpose(bots[i].z) });
                }
            }
        }
        if pace_ms > 0 { std::thread::sleep(Duration::from_millis(pace_ms)); }

        // Repopulate culled slots. Usually inherit one survivor's purpose with a
        // little drift; with crossover on, sometimes blend two survivors into a
        // hybrid (genome averaged, purpose taken from one parent) — recombination
        // explores the space faster than mutation alone.
        let surv_vec: Vec<usize> = survivor_set.iter().copied().collect();
        for i in 0..bots.len() {
            if !bots[i].alive {
                let hybrid = cfg.crossover && surv_vec.len() >= 2 && rng.gen_bool(0.5);
                if hybrid {
                    let pa = surv_vec[rng.gen_range(0..surv_vec.len())];
                    let mut pb = surv_vec[rng.gen_range(0..surv_vec.len())];
                    while pb == pa { pb = surv_vec[rng.gen_range(0..surv_vec.len())]; }
                    let gmut: f64 = rng.gen_range(-0.05..0.05);
                    bots[i].genome = ((bots[pa].genome + bots[pb].genome) / 2.0 + gmut).clamp(0.0, 1.0);
                    // purpose from one parent (a discrete trait), with slight drift
                    let zsrc = if rng.gen_bool(0.5) { bots[pa].z } else { bots[pb].z };
                    bots[i].z = (zsrc + rng.gen_range(-0.03..0.03)).clamp(0.0, 1.0);
                } else {
                    let parent = surv_vec[rng.gen_range(0..surv_vec.len())];
                    let gmut: f64 = rng.gen_range(-0.07..0.07);
                    let zmut: f64 = rng.gen_range(-0.04..0.04);
                    bots[i].genome = (bots[parent].genome + gmut).clamp(0.0, 1.0);
                    bots[i].z = (bots[parent].z + zmut).clamp(0.0, 1.0);
                }
                bots[i].fitness = 0.0;
                bots[i].alive = true;
                if animate {
                    let _ = tx.send_blocking(EngineMsg::Bot { id: bots[i].id, status: BotStatus::Spawned, fitness: 0.0, z: bots[i].z, purpose: purpose(bots[i].z) });
                }
            }
        }
        if pace_ms > 0 { std::thread::sleep(Duration::from_millis(pace_ms)); }
        if animate {
            for b in &bots {
                let _ = tx.send_blocking(EngineMsg::Bot { id: b.id, status: BotStatus::Idle, fitness: b.fitness, z: b.z, purpose: purpose(b.z) });
            }
        }

        // Coordinator learns the axis: move focus toward the best-rewarding niche.
        if cfg.coordinator {
            let best_k = (0..niches).max_by(|&a, &b| niche_reward[a].partial_cmp(&niche_reward[b]).unwrap()).unwrap_or(0);
            let target = (best_k as f64 + 0.5) / niches as f64;
            focus += 0.5 * (target - focus);
            let _ = tx.send_blocking(EngineMsg::Coordinator { focus, label: purpose(focus), niche_reward: niche_reward[best_k] });
            log(&tx, format!("coordinator -> focus z={:.2} ('{}'), niche reward {:.3}", focus, purpose(focus), niche_reward[best_k]));
        }

        let _ = tx.send_blocking(EngineMsg::Generation { n: gen + 1, best, mean, alive: alive_n, focus });
        // Top-k snapshot for the replay buffer (by fitness, descending).
        {
            let mut ranked: Vec<usize> = alive_ids.clone();
            ranked.sort_by(|&a, &b| bots[b].fitness.partial_cmp(&bots[a].fitness).unwrap_or(std::cmp::Ordering::Equal));
            let top: Vec<(usize, f64, f64)> = ranked.iter().take(8)
                .map(|&i| (bots[i].id, bots[i].z, bots[i].fitness)).collect();
            let _ = tx.send_blocking(EngineMsg::Replay { gen: gen + 1, best, mean, top });
        }
        log(&tx, format!("generation {} done: {} survivors, best={:.3}, mean={:.3}", gen + 1, survivors.len(), best, mean));
        // Diversity readout (so the novelty bonus is tunable): genome spread, plus
        // mean fingerprint spread for real backends.
        if cfg.novelty_lambda > 0.0 && survivors.len() > 1 {
            let gs: Vec<f64> = survivors.iter().map(|&i| bots[i].genome).collect();
            let gm = gs.iter().sum::<f64>() / gs.len() as f64;
            let gstd = (gs.iter().map(|g| (g - gm).powi(2)).sum::<f64>() / gs.len() as f64).sqrt();
            let fp_spread = if !prev_survivor_fps.is_empty() {
                let nonempty: Vec<&Vec<f32>> = prev_survivor_fps.iter().filter(|f| !f.is_empty()).collect();
                if nonempty.len() > 1 {
                    let mut sum = 0.0; let mut cnt = 0u32;
                    for a in 0..nonempty.len() { for b in (a+1)..nonempty.len() {
                        sum += fingerprint_distance(nonempty[a], nonempty[b]) as f64; cnt += 1;
                    }}
                    Some(sum / cnt.max(1) as f64)
                } else { None }
            } else { None };
            match fp_spread {
                Some(fs) => log(&tx, format!("  diversity: genome σ={gstd:.3}, code spread={fs:.3} (raise novelty λ to widen, lower to focus)")),
                None => log(&tx, format!("  diversity: genome σ={gstd:.3} (raise novelty λ to widen, lower to focus)")),
            }
        }
        let sec_now = security_counter().load(std::sync::atomic::Ordering::Relaxed);
        if sec_now > sec_seen {
            log(&tx, format!("[security] {} event(s) this generation flagged and culled — see {}",
                sec_now - sec_seen, security_log_path().display()));
            sec_seen = sec_now;
        }

        // Auditor: synthesize this generation's behaviour into a structured record +
        // a plain-language overview. Always recorded; the conversational line is
        // throttled (and forced on a trend change or any risk flag) so long runs
        // don't flood the log.
        {
            let alive_zs: Vec<f64> = alive_ids.iter().map(|&i| bots[i].z).collect();
            let gen_sigma = {
                let gs: Vec<f64> = survivors.iter().map(|&i| bots[i].genome).collect();
                if gs.len() > 1 {
                    let m = gs.iter().sum::<f64>() / gs.len() as f64;
                    (gs.iter().map(|g| (g - m).powi(2)).sum::<f64>() / gs.len() as f64).sqrt()
                } else { 0.0 }
            };
            let new_risk = sec_now.saturating_sub(audit_sec_seen);
            audit_sec_seen = sec_now;
            let a = audit_generation(gen + 1, purpose(focus).as_str(), &alive_zs, &cfg.ops,
                best, mean, prev_audit_best, gen_sigma, None, new_risk);
            // Throttle the conversational line: every 20 generations, the final one,
            // or any generation that raised a risk flag (trend near a plateau flips
            // too often to use as a trigger, so it's shown but doesn't force a line).
            if gen % 20 == 0 || gen + 1 == cfg.generations || new_risk > 0 {
                log(&tx, format!("  {}", a.overview()));
            }
            _last_trend = a.trend;
            prev_audit_best = Some(best);
            audit_run.push(a.structured());
            // keep the run report bounded on very long runs
            if audit_run.len() > 2000 { audit_run.remove(0); }
        }
    }

    if cfg.memoize {
        save_scores(&memo.into_inner().unwrap());
    }
    // Auto-save the run's final population so a follow-up run can resume from it
    // (the seed + last generation + each bot's z/genome/fitness are all that's
    // needed; the Config is reconstructed from the GUI).
    let ckpt = Checkpoint {
        seed, gen: cfg.generations, focus,
        bots: bots.iter().map(|b| (b.id, b.z, b.genome, b.fitness, b.alive)).collect(),
    };
    if let Ok(path) = save_checkpoint(&ckpt) {
        log(&tx, format!("checkpoint saved → {} ({} bots)", path.display(), ckpt.bots.len()));
    }
    // Write the auditor's run report next to the checkpoint: a grep-able structured
    // line per generation (bounded), for after-the-fact analysis of how the swarm
    // behaved — the durable "audit trail."
    {
        let dir = crate::registry::data_root().join("checkpoints");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("audit.log");
        let mut body = String::from("# bot-arena run audit (structured, one line per recorded generation)\n");
        for line in &audit_run { body.push_str(line); body.push('\n'); }
        if std::fs::write(&path, body).is_ok() {
            log(&tx, format!("audit trail saved → {} ({} generations recorded)", path.display(), audit_run.len()));
        }
    }
    teardown(&cfg, &handles, &run_root, &tx);
    // Persist what the swarm learned about test→code localization. This file
    // accumulates across runs and across workspaces — the automatic, app-agnostic
    // replacement for a hand-built per-app embedding pipeline.
    save_localizer();
    if let Ok(m) = localizer().lock() {
        if m.updates > 0 {
            log(&tx, format!("[localize] learned {} test→code association(s) this process; model saved to {}",
                m.updates, affinity_path().display()));
        }
    }
    if cfg.cache {
        let freed = prune_cache();
        if freed > 0 {
            log(&tx, format!("[cache] pruned {:.1} GB of build artifacts (over ARENA_CACHE_CAP_GB)",
                freed as f64 / 1e9));
        }
    }
    let total_sec = security_counter().load(std::sync::atomic::Ordering::Relaxed);
    if total_sec > 0 {
        log(&tx, format!("[security] {total_sec} flagged event(s) total this process — audit trail at {}",
            security_log_path().display()));
    }
    // Cascade report: how the "race for speed" actually played out — how many winning
    // patches each tier produced. Tier 0 is the fast/primary; higher tiers are escalations.
    {
        let v = tier_counter().lock().unwrap().clone();
        let total: usize = v.iter().sum();
        if total > 0 && (v.len() > 1 || !proposer_chain(&cfg).is_empty()) {
            let parts: Vec<String> = v.iter().enumerate()
                .map(|(t, &c)| format!("tier {t}: {c} ({}%)", if total > 0 { c * 100 / total } else { 0 }))
                .collect();
            log(&tx, format!("[cascade] winning patches by tier — {} (tier 0 = fastest; higher = escalated)",
                parts.join(", ")));
        }
    }
    log(&tx, "run complete.");
    let _ = tx.send_blocking(EngineMsg::Done);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let t = std::env::temp_dir().join(format!("ba-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        t
    }
    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn atomic_write_preserves_hardlinked_original() {
        // rsync --link-dest hardlinks the worktree to the source. An in-place write
        // would corrupt the source; atomic_write (temp + rename) must not.
        let dir = std::env::temp_dir().join(format!("atom-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("orig.txt");
        let link = dir.join("worktree.txt");
        std::fs::write(&src, "ORIGINAL").unwrap();
        std::fs::hard_link(&src, &link).unwrap();
        atomic_write(&link, "EDITED").unwrap();
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "ORIGINAL", "source untouched");
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "EDITED", "worktree updated");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provision_non_git_yields_worktree_with_files() {
        let ws = std::env::temp_dir().join(format!("prov-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::create_dir_all(ws.join("target")).unwrap();
        std::fs::write(ws.join("target/junk"), "x").unwrap(); // noise, must be skipped

        let cfg = base_cfg(); // fast_clone = true
        let work = std::env::temp_dir().join(format!("prov-work-{}", std::process::id()));
        let prov = provision_worktree(&cfg, &ws, &work).unwrap();
        assert!(matches!(prov, Provisioned::Dir(_)), "non-git → Dir strategy");
        assert!(prov.path().join("src/main.rs").exists(), "source copied");
        assert!(!prov.path().join("target").exists(), "build noise excluded");
        // editing the worktree must not touch the source (hardlink-safe)
        atomic_write(&prov.path().join("src/main.rs"), "fn main(){/*edited*/}").unwrap();
        assert_eq!(std::fs::read_to_string(ws.join("src/main.rs")).unwrap(), "fn main(){}", "source intact");
        prov.remove();
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn tripwire_flags_test_file_edits() {
        assert!(is_test_path(Path::new("tests/foo.rs")));
        assert!(is_test_path(Path::new("src/tests/inner.rs")));
        assert!(is_test_path(Path::new("test_utils.py")));
        assert!(is_test_path(Path::new("pkg/foo_test.go")));
        assert!(is_test_path(Path::new("__tests__/x.js")));
        assert!(is_test_path(Path::new("a/b/c.spec.js")));
        assert!(!is_test_path(Path::new("src/lib.rs")));
        assert!(!is_test_path(Path::new("README.md")));

        // end-to-end: a copy that adds a file under tests/ is flagged
        let orig = std::env::temp_dir().join(format!("tw-orig-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("tw-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(orig.join("src")).unwrap();
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::create_dir_all(work.join("tests")).unwrap();
        std::fs::write(orig.join("src/lib.rs"), "fn f(){}").unwrap();
        std::fs::write(work.join("src/lib.rs"), "fn f(){}").unwrap();
        std::fs::write(work.join("tests/sneaky.rs"), "// no asserts").unwrap();
        assert!(touched_test_file(&orig, &work), "edit under tests/ triggers tripwire");

        // baseline copy with no diff → not flagged
        let work2 = std::env::temp_dir().join(format!("tw-work2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work2);
        std::fs::create_dir_all(work2.join("src")).unwrap();
        std::fs::write(work2.join("src/lib.rs"), "fn f(){}").unwrap();
        assert!(!touched_test_file(&orig, &work2), "unchanged copy is clean");

        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&work2);
    }

    #[test]
    fn last_workspace_round_trips() {
        let _g = super::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("lastws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        // a real, existing dir to remember
        let ws = tmp.join("myproject");
        std::fs::create_dir_all(&ws).unwrap();
        assert!(crate::registry::load_last_workspace().is_none(), "nothing remembered yet");
        crate::registry::save_last_workspace(&ws);
        assert_eq!(crate::registry::load_last_workspace().as_deref(), Some(ws.as_path()));
        // a remembered path that no longer exists is ignored
        crate::registry::save_last_workspace(std::path::Path::new("/nonexistent/gone"));
        assert!(crate::registry::load_last_workspace().is_none(), "stale path filtered out");
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn security_events_are_logged_and_counted() {
        let _g = super::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("sec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        let before = security_counter().load(std::sync::atomic::Ordering::Relaxed);
        record_security_event(7, "danger-pattern", "introduced `rm -rf`; scored 0");
        record_security_event(9, "test-edit", "altered tests/foo.rs");
        let after = security_counter().load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(after - before, 2, "counter advanced by two events");
        let logged = std::fs::read_to_string(security_log_path()).unwrap_or_default();
        assert!(logged.contains("bot=7") && logged.contains("danger-pattern"), "log: {logged:?}");
        assert!(logged.contains("bot=9") && logged.contains("test-edit"), "log: {logged:?}");
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn danger_scan_flags_new_destructive_lines() {
        let orig = std::env::temp_dir().join(format!("dz-o-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("dz-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(orig.join("src")).unwrap();
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(orig.join("src/main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        // bot "introduces" a destructive call
        std::fs::write(work.join("src/main.rs"),
            "fn main() { std::process::Command::new(\"sh\").arg(\"-c\").arg(\"rm -rf /\").status(); }\n").unwrap();
        assert!(diff_introduces_danger(&orig, &work).is_some(), "rm -rf in a new line must flag");

        // unchanged copy → clean
        std::fs::write(work.join("src/main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        assert!(diff_introduces_danger(&orig, &work).is_none(), "no new danger → clean");
        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn error_summary_surfaces_real_compiler_errors() {
        let cargo_out = "   Compiling web2qml v0.1.0\n\
            error[E0432]: unresolved import `foo::bar`\n\
              --> src/lib.rs:3:5\n\
            warning: unused variable: x\n\
            error: could not compile `web2qml` due to previous error\n\
            warning: build failed, waiting for other jobs to finish...\n";
        let s = error_summary(cargo_out);
        assert!(s.contains("E0432"), "real error surfaced, not the trailing noise: {s}");
        assert!(!s.contains("waiting for other jobs"), "noise excluded when errors exist");

        // no error lines → falls back to tail
        let plain = "line one\nline two\nline three\n";
        assert!(error_summary(plain).contains("line three"));
    }

    #[test]
    fn tripwire_catches_inline_cfg_test_edits() {
        // A file the bot is *allowed* to edit (src/lib.rs), but its #[cfg(test)]
        // block is altered. Path-only tripwire would miss this; the inline-block
        // comparator catches it.
        let orig = std::env::temp_dir().join(format!("inline-o-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("inline-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(orig.join("src")).unwrap();
        std::fs::create_dir_all(work.join("src")).unwrap();
        // identical non-test code, different inline test bodies
        let pre = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
                   #[cfg(test)]\n\
                   mod t {\n\
                       use super::*;\n\
                       #[test] fn a() { assert_eq!(add(1, 2), 3); }\n\
                   }\n";
        let post = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
                    #[cfg(test)]\n\
                    mod t {\n\
                        use super::*;\n\
                        #[test] fn a() { assert_eq!(add(1, 2), 999); }\n\
                    }\n";
        std::fs::write(orig.join("src/lib.rs"), pre).unwrap();
        std::fs::write(work.join("src/lib.rs"), post).unwrap();
        assert!(touched_test_file(&orig, &work),
            "edit to assertion inside an inline #[cfg(test)] block must trip the wire");

        // Sanity: edits to non-test code in the same file should NOT trip.
        let post2 = "pub fn add(a: i32, b: i32) -> i32 { a + b + 0 }\n\
                     #[cfg(test)]\n\
                     mod t {\n\
                         use super::*;\n\
                         #[test] fn a() { assert_eq!(add(1, 2), 3); }\n\
                     }\n";
        std::fs::write(work.join("src/lib.rs"), post2).unwrap();
        assert!(!touched_test_file(&orig, &work),
            "edit to non-test code (same file) must not trip the wire");

        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn tripwire_catches_test_injected_at_crate_root() {
        // The exact hack the warmup challenge exposed: a bot duplicates an
        // already-passing `#[test] fn` to crate scope (OUTSIDE the #[cfg(test)]
        // mod), inflating its pass count. The block-only comparator missed it
        // because the injected test isn't inside a #[cfg(test)] block.
        let orig = std::env::temp_dir().join(format!("croot-o-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("croot-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(orig.join("src")).unwrap();
        std::fs::create_dir_all(work.join("src")).unwrap();
        let pre = "pub fn answer() -> i64 { 41 }\n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                       use super::*;\n\
                       #[test] fn t_answer() { assert_eq!(answer(), 42); }\n\
                   }\n";
        // a duplicate #[test] injected at crate root, before the cfg(test) mod
        let post = "pub fn answer() -> i64 { 41 }\n\
                    #[test] fn t_double_pos() { assert_eq!(2 + 2, 4); }\n\
                    #[cfg(test)]\n\
                    mod tests {\n\
                        use super::*;\n\
                        #[test] fn t_answer() { assert_eq!(answer(), 42); }\n\
                    }\n";
        std::fs::write(orig.join("src/lib.rs"), pre).unwrap();
        std::fs::write(work.join("src/lib.rs"), post).unwrap();
        assert!(touched_test_file(&orig, &work),
            "a #[test] fn injected at crate root must trip the wire");
        let _ = std::fs::remove_dir_all(&orig);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    #[ignore] // benchmark: cargo test --release bench_coordinated -- --ignored --nocapture
    fn bench_coordinated_valley_needs_multi_edit() {
        if !which("cargo") { return; }
        let challenge = Path::new(env!("CARGO_MANIFEST_DIR")).join("challenges/coordinated");
        if !challenge.join("Cargo.toml").exists() { return; }
        let run = |neutral: &str, bundle: &str, label: &str| {
            std::env::set_var("ARENA_NEUTRAL_PROB", neutral);
            std::env::set_var("ARENA_MAX_BUNDLE", bundle);
            let ws = std::env::temp_dir().join(format!("coord-{label}-{}", std::process::id()));
            let run_root = std::env::temp_dir().join(format!("coord-rr-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&run_root);
            copy_dir_recursive(&challenge, &ws).unwrap();
            std::fs::create_dir_all(&run_root).unwrap();
            let mut cfg = base_cfg();
            cfg.backend = BackendKind::Local;
            cfg.fast_clone = false; cfg.cache = true; cfg.memoize = true;
            cfg.recurrent_steps = 150;
            let memo = std::sync::Mutex::new(std::collections::HashMap::new());
            let mut best = 0.0f64;
            for trial in 0..2 {
                let mut rng = seeded_rng(0x5A1AD + trial);
                let (score, _) = evaluate_batched(&cfg, &ws, trial as usize, 0.5, "", &run_root, &mut rng, Some(&memo), 0, 1);
                best = best.max(score);
            }
            println!("{label}: best={best:.3}");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&run_root);
            best
        };
        let greedy = run("0.0", "1", "pure-greedy (no drift, single-edit)");
        let drift = run("0.3", "1", "neutral-drift only");
        let bundle = run("0.0", "4", "multi-edit only");
        let both = run("0.3", "4", "drift + multi-edit");
        std::env::remove_var("ARENA_NEUTRAL_PROB");
        std::env::remove_var("ARENA_MAX_BUNDLE");
        println!("VALLEY: pure-greedy {greedy:.3} | drift {drift:.3} | bundle {bundle:.3} | both {both:.3} (baseline 0.333, single-fix ceiling 0.667)");
    }

    #[test]
    #[ignore] // cargo test --release bench_deceptive -- --ignored --nocapture
    fn bench_deceptive_valley_needs_multi_edit() {
        if !which("cargo") { return; }
        let challenge = Path::new(env!("CARGO_MANIFEST_DIR")).join("challenges/deceptive");
        if !challenge.join("Cargo.toml").exists() { return; }
        let run = |bundle: &str, label: &str| {
            // neutral drift ON for both arms; vary ONLY the multi-edit cap, since
            // this valley's intermediate is strictly worse — drift cannot cross it,
            // so any crossing is attributable to bundling.
            std::env::set_var("ARENA_NEUTRAL_PROB", "0.3");
            std::env::set_var("ARENA_MAX_BUNDLE", bundle);
            let ws = std::env::temp_dir().join(format!("dec-{label}-{}", std::process::id()));
            let run_root = std::env::temp_dir().join(format!("dec-rr-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&run_root);
            copy_dir_recursive(&challenge, &ws).unwrap();
            std::fs::create_dir_all(&run_root).unwrap();
            let mut cfg = base_cfg();
            cfg.backend = BackendKind::Local;
            cfg.fast_clone = false; cfg.cache = true; cfg.memoize = true;
            cfg.recurrent_steps = 200;
            let memo = std::sync::Mutex::new(std::collections::HashMap::new());
            let mut best = 0.0f64;
            for trial in 0..3 {
                let mut rng = seeded_rng(0xDECEB + trial);
                let (score, _) = evaluate_batched(&cfg, &ws, trial as usize, 0.5, "", &run_root, &mut rng, Some(&memo), 0, 1);
                best = best.max(score);
            }
            println!("{label}: best={best:.3}");
            let _ = std::fs::remove_dir_all(&ws);
            let _ = std::fs::remove_dir_all(&run_root);
            best
        };
        let no_bundle = run("1", "drift-only (single-edit)");
        let bundled = run("4", "drift + multi-edit");
        std::env::remove_var("ARENA_NEUTRAL_PROB");
        std::env::remove_var("ARENA_MAX_BUNDLE");
        println!("DECEPTIVE VALLEY: single-edit {no_bundle:.3} (drift can't cross strictly-worse), bundling {bundled:.3} (baseline 0.667)");
    }

    #[test]
    fn flip_ops_is_bidirectional_and_arithmetic() {
        assert_eq!(flip_ops("if a < b {"), "if a > b {");
        assert_eq!(flip_ops("if a > b {"), "if a < b {"); // reverse direction
        assert_eq!(flip_ops("x == y"), "x != y");
        assert_eq!(flip_ops("{ 6 + 2 }"), "{ 6 - 2 }");
        assert_eq!(flip_ops("{ a() - 10 }"), "{ a() + 10 }");
        assert_eq!(flip_ops("n * 2"), "n / 2");
        // must NOT mangle the return arrow
        assert_eq!(flip_ops("fn f() -> i64 { 0 }"), "fn f() -> i64 { 0 }");
    }

    #[test]
    fn surefire_parser_sums_modules() {
        // single module: 12 run, 1 fail, 0 err, 2 skipped → passed=9, total=10
        let one = "Tests run: 12, Failures: 1, Errors: 0, Skipped: 2";
        assert_eq!(parse_surefire(one, Some(1)), (9, 10));
        // multi-module sums; a clean module + a failing module
        let multi = "Tests run: 4, Failures: 0, Errors: 0, Skipped: 0\n\
                     Tests run: 6, Failures: 2, Errors: 1, Skipped: 0";
        assert_eq!(parse_surefire(multi, Some(1)), (7, 10)); // 10 run, 3 bad → 7 pass
        // no summary, exit 0 → treat as a single trivially-passing unit
        assert_eq!(parse_surefire("BUILD SUCCESS", Some(0)), (1, 1));
    }

    #[test]
    fn classify_workspace_flags_unsupported() {
        let dir = std::env::temp_dir().join(format!("classify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a Rust crate (supported) sitting next to a Gradle build (unsupported)
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.1.0\"").unwrap();
        std::fs::write(dir.join("build.gradle"), "plugins { id 'java' }").unwrap();
        let s = classify_workspace(&dir);
        assert!(s.runnable(), "has cargo → runnable");
        assert!(s.supported.contains(&"cargo".to_string()));
        assert!(s.unsupported.iter().any(|u| u.contains("Gradle")));

        // a pure Gradle repo → not runnable, flagged
        let g = std::env::temp_dir().join(format!("classify-g-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&g);
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("build.gradle.kts"), "plugins { kotlin(\"jvm\") }").unwrap();
        let s2 = classify_workspace(&g);
        assert!(!s2.runnable(), "gradle-only → not runnable");
        assert!(s2.unsupported.iter().any(|u| u.contains("Gradle")));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&g);
    }

    #[test]
    fn ops_respect_identifier_boundaries() {
        let mut r = seeded_rng(1);
        // flip-bool must toggle the `false` literal, NOT the `true` in always_true
        let mut l = vec!["pub fn always_true() -> bool { false }".to_string()];
        assert!(apply_op("flip-bool", &mut l, 0, 0.3, &mut r));
        assert_eq!(l[0], "pub fn always_true() -> bool { true }",
            "flip-bool must hit the literal, not the identifier");
        // tweak-num must change the `41` literal, not the `64` in the type i64
        let mut l2 = vec!["fn answer() -> i64 { 41 }".to_string()];
        assert!(apply_op("tweak-num", &mut l2, 0, 0.3, &mut r));
        assert!(l2[0].contains("i64"), "type i64 must be left intact");
        assert!(l2[0].contains("{ 42 }") || l2[0].contains("{ 40 }"),
            "the 41 literal must be nudged by ±1, got: {}", l2[0]);
    }

    #[test]
    fn op_matches_the_line_content() {
        let ops: Vec<String> = all_edit_ops();
        let weight = |line: &str, op: &str| -> f64 {
            op_weights_for_line(line, &ops).into_iter()
                .find(|(o, _)| o == op).map(|(_, w)| w).unwrap_or(0.0)
        };
        // a comparison line should favour flip-op over a structural op
        assert!(weight("if a < b { a } else { b }", "flip-op") > weight("if a < b { a } else { b }", "dup-line"));
        // a boolean line should favour flip-bool
        assert!(weight("pub fn always_true() -> bool { false }", "flip-bool") > 0.0);
        // a numeric line should favour tweak-num; flip-bool should be inapplicable
        assert!(weight("fn answer() -> i64 { 41 }", "tweak-num") > 0.0);
        assert_eq!(weight("fn answer() -> i64 { 41 }", "flip-bool"), 0.0);
        // a line with none of the above still yields *some* op (never empty)
        assert!(!op_weights_for_line("struct Foo;", &ops).is_empty());
    }

    #[test]
    fn affinity_model_learns_and_round_trips() {
        let mut m = AffinityModel::default();
        let test_toks: std::collections::HashSet<String> =
            ["parse", "input"].iter().map(|s| s.to_string()).collect();
        let code_toks: std::collections::HashSet<String> =
            ["tokenize", "lexer"].iter().map(|s| s.to_string()).collect();
        // observe the same repair a few times
        for _ in 0..3 { m.reinforce(&test_toks, &code_toks, 1.0); }

        // a query containing "parse" should now surface the learned code vocabulary
        let q: std::collections::HashSet<String> = ["parse"].iter().map(|s| s.to_string()).collect();
        let expanded = m.expand(&q);
        assert!(expanded.contains_key("tokenize"), "model must learn parse→tokenize");
        assert!(expanded.contains_key("lexer"));
        assert!(!expanded.contains_key("parse"), "self-association is not stored");

        // a code line with no DIRECT overlap with the test query still gets weight
        // via the learned association — the whole point of the mature version.
        let empty_query = std::collections::HashSet::new();
        let w_learned = line_weight("fn tokenize(s: &str) {}", &empty_query, &expanded);
        let w_plain = line_weight("fn unrelated() {}", &empty_query, &expanded);
        assert!(w_learned > w_plain, "learned affinity must lift the associated line ({w_learned} vs {w_plain})");

        // TSV persistence round-trips
        let tsv = m.to_tsv();
        let m2 = AffinityModel::from_tsv(&tsv);
        let e2 = m2.expand(&q);
        assert!(e2.contains_key("tokenize") && e2.contains_key("lexer"),
            "model must survive a save/load round-trip");
    }

    #[test]
    fn failing_tests_and_query_tokens() {
        let out = "running 5 tests\n\
                   test tests::t_double_pos ... ok\n\
                   test tests::t_answer ... FAILED\n\
                   test tests::t_greater ... FAILED\n\
                   test result: FAILED. 1 passed; 2 failed;\n";
        let fails = failing_test_names(out);
        assert!(fails.contains(&"tests::t_answer".to_string()));
        assert!(fails.contains(&"tests::t_greater".to_string()));
        assert!(!fails.iter().any(|f| f.contains("t_double_pos")));

        let mut q = std::collections::HashSet::new();
        ident_tokens("tests::t_answer", &mut q);
        assert!(q.contains("answer"));
        assert!(!q.contains("t"), "single-letter test prefix should be dropped");
        ident_tokens("alwaysTrueValue", &mut q);
        assert!(q.contains("always") && q.contains("true") && q.contains("value"),
            "camelCase must split into words");
    }

    #[test]
    fn query_locates_the_function_under_test() {
        // A failing test that calls answer() should pull "answer" into the query,
        // and the line defining answer() should outweigh an unrelated line.
        let dir = std::env::temp_dir().join(format!("loc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"),
            "pub fn answer() -> i64 { 41 }\n\
             pub fn unrelated() -> i64 { 7 }\n\
             #[cfg(test)]\n\
             mod tests {\n\
                 use super::*;\n\
                 #[test] fn t_answer() { assert_eq!(answer(), 42); }\n\
             }\n").unwrap();
        let q = localization_query(&dir, &["tests::t_answer".to_string()]);
        assert!(q.contains("answer"), "query must include the symbol under test");
        let none = HashMap::new();
        let w_def = line_weight("pub fn answer() -> i64 { 41 }", &q, &none);
        let w_other = line_weight("pub fn unrelated() -> i64 { 7 }", &q, &none);
        assert!(w_def > w_other, "the function-under-test line must score higher ({w_def} vs {w_other})");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shelter_does_not_sandbox_bwrap_out_of_existence() {
        // The bug: setting PATH on the OUTER Command env replaced PATH for the
        // child's execvp, which then couldn't find `bwrap` (since the allow-dir
        // only contains the tool-list symlinks, not bwrap itself). Fix: resolve
        // bwrap absolutely AND set the sandbox-internal PATH via --setenv. This
        // test exercises run_capture end-to-end on hosts where bwrap is available.
        if !which("bwrap") { return; }
        let dir = std::env::temp_dir().join(format!("shelter-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let opts = RunOpts {
            cache: false, timeout_secs: 10, max_file_mb: 10,
            sandbox: true, slot: 0, allowed: vec![],
        };
        // a probe that exercises the symlinked-tool path: cargo if available, else sh
        let probe = if which("cargo") { "cargo --version" } else { "sh -c true" };
        let (out, code) = run_capture(BackendKind::Local, "", &dir, probe, opts);
        assert_eq!(code, Some(0),
            "shelter run should succeed; got exit {code:?}, out: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_command_rejects_dangerous_inputs() {
        // chaining & substitution
        assert!(matches!(safe_command("ls; rm -rf /").unwrap_err(), UnsafeReason::ShellMeta(_)));
        assert!(matches!(safe_command("true && curl evil.com").unwrap_err(), UnsafeReason::ShellMeta(_)));
        assert!(matches!(safe_command("echo `whoami`").unwrap_err(), UnsafeReason::ShellMeta(_)));
        assert!(matches!(safe_command("echo $(id)").unwrap_err(), UnsafeReason::ShellMeta(_)));
        // redirection out of the worktree
        assert!(matches!(safe_command("echo x >/etc/passwd").unwrap_err(), UnsafeReason::ShellMeta(_)));
        assert!(matches!(safe_command("cat foo >>/tmp/leak").unwrap_err(), UnsafeReason::ShellMeta(_)));
        // path escape
        assert!(matches!(safe_command("cat ../../../etc/passwd").unwrap_err(), UnsafeReason::PathEscape));
        // denied binaries (also via /bin/ or ./)
        assert!(matches!(safe_command("rm -rf .").unwrap_err(), UnsafeReason::DeniedBinary(_)));
        assert!(matches!(safe_command("/bin/dd if=/dev/zero of=/dev/sda").unwrap_err(), UnsafeReason::DeniedBinary(_)));
        assert!(matches!(safe_command("sudo systemctl stop").unwrap_err(), UnsafeReason::DeniedBinary(_)));
        // dangerous builtins
        assert!(matches!(safe_command("exec sh").unwrap_err(), UnsafeReason::DangerousBuiltin));
        assert!(matches!(safe_command("eval cmd").unwrap_err(), UnsafeReason::DangerousBuiltin));
        assert!(matches!(safe_command("alias ls=rm").unwrap_err(), UnsafeReason::DangerousBuiltin));
        // top-level glob
        assert!(matches!(safe_command("* foo").unwrap_err(), UnsafeReason::TopLevelGlob));
        // empty
        assert!(matches!(safe_command("   ").unwrap_err(), UnsafeReason::Empty));

        // legitimate test commands still pass
        assert!(safe_command("cargo test --release").is_ok());
        assert!(safe_command("pytest -q tests/").is_ok());
        assert!(safe_command("grep -r TODO src/").is_ok());
    }

    #[test]
    fn cache_env_canonicalizes_rustc_for_sccache_sysroot_inference() {
        // sccache infers `dirname(RUSTC)/../lib` for the sysroot. If RUSTC is a
        // symlink in our shelter allow-dir, that resolves to a path that doesn't
        // exist. The fix: when sccache is in use, RUSTC must be the canonical
        // (symlink-resolved) absolute path of the real binary.
        let env = cache_env(true, 0);
        let m: std::collections::HashMap<_, _> = env.into_iter().collect();
        if !m.contains_key("RUSTC_WRAPPER") {
            // sccache not on this host → RUSTC isn't needed; fix is irrelevant.
            return;
        }
        let rustc = m.get("RUSTC").expect("with sccache, RUSTC must be set");
        let p = Path::new(rustc);
        assert!(p.is_absolute(), "RUSTC must be absolute: {rustc}");
        let meta = std::fs::symlink_metadata(p).unwrap();
        assert!(!meta.file_type().is_symlink(),
            "RUSTC must be canonical (sccache derives sysroot from dirname): {rustc}");
    }

    #[test]
    fn cache_env_avoids_sccache_incremental_conflict() {
        // The actual fix: when both CCACHE_DIR and RUSTC_WRAPPER=sccache are
        // set, CARGO_INCREMENTAL must be 0 — otherwise sccache exits 101 with
        // "incremental compilation is prohibited" and every bot scores 0.
        let env = cache_env(true, 0);
        let m: std::collections::HashMap<_, _> = env.into_iter().collect();
        if m.get("RUSTC_WRAPPER").map(|s| s.contains("sccache")).unwrap_or(false) {
            assert_eq!(m.get("CARGO_INCREMENTAL").map(String::as_str), Some("0"),
                "sccache + incremental → exit 101; must be off");
        }
    }

    #[test]
    fn baseline_surfaces_timeout_kill() {
        // A workspace whose 'test' is a hung sleep; with a 1s timeout it must
        // get killed (exit 124) and the baseline diagnostic must mention it.
        let ws = std::env::temp_dir().join(format!("to-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let mut cfg = base_cfg();
        cfg.workspace = Some(ws.clone());
        cfg.test_cmd = "sleep 30".into(); // never finishes within timeout
        cfg.timeout_secs = 1;
        let run_root = std::env::temp_dir().join(format!("to-run-{}", std::process::id()));
        let (p, t, diag) = baseline_score(&cfg, &ws, &run_root);
        assert_eq!(p, 0, "no tests passed (process was killed)");
        assert!(diag.iter().any(|d| d.contains("timed out") || d.contains("exit 124")),
            "diagnostic must mention the timeout: {diag:?}");
        let _ = (t, std::fs::remove_dir_all(&ws), std::fs::remove_dir_all(&run_root));
    }

    #[test]
    fn baseline_distinguishes_working_from_broken_project() {
        // working project: tests pass on a clean checkout
        let ok = std::env::temp_dir().join(format!("bl-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ok);
        std::fs::create_dir_all(ok.join("src")).unwrap();
        std::fs::write(ok.join("Cargo.toml"),
            "[package]\nname=\"ok\"\nversion=\"0.1.0\"\nedition=\"2021\"").unwrap();
        std::fs::write(ok.join("src/lib.rs"),
            "pub fn f()->i32{1}\n#[cfg(test)] mod t{#[test] fn a(){assert_eq!(super::f(),1);}}\n").unwrap();
        let cfg_ok = { let mut c = base_cfg(); c.workspace = Some(ok.clone()); c };
        let run_root = std::env::temp_dir().join(format!("bl-run-{}", std::process::id()));
        let (p, t, _) = baseline_score(&cfg_ok, &ok, &run_root);
        assert!(t > 0 && p > 0, "working project's baseline > 0 ({p}/{t})");

        // broken project: tests exist but fail
        let bad = std::env::temp_dir().join(format!("bl-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bad);
        std::fs::create_dir_all(bad.join("src")).unwrap();
        std::fs::write(bad.join("Cargo.toml"),
            "[package]\nname=\"bad\"\nversion=\"0.1.0\"\nedition=\"2021\"").unwrap();
        std::fs::write(bad.join("src/lib.rs"),
            "pub fn f()->i32{1}\n#[cfg(test)] mod t{#[test] fn a(){assert_eq!(super::f(),999);}}\n").unwrap();
        let cfg_bad = { let mut c = base_cfg(); c.workspace = Some(bad.clone()); c };
        let (p2, t2, _) = baseline_score(&cfg_bad, &bad, &run_root);
        assert_eq!(p2, 0, "broken project: 0 passing");
        assert!(t2 > 0, "but we *can* tell tests exist ({t2}) — the harness isn't the problem");

        let _ = std::fs::remove_dir_all(&ok);
        let _ = std::fs::remove_dir_all(&bad);
        let _ = std::fs::remove_dir_all(&run_root);
    }

    #[test]
    fn batched_collects_repeated_workspaces() {
        // z=0.95 → no-op edit operator → every attempt produces the SAME workspace
        // hash. With k=12, exactly one test should run; the other 11 attempts are
        // either same-as-batch (dedup) or memo hits.
        let ws = std::env::temp_dir().join(format!("batch-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("Cargo.toml"), "[package]\nname=\"b\"\nversion=\"0.1.0\"\nedition=\"2021\"").unwrap();
        std::fs::write(ws.join("src/lib.rs"), "pub fn f()->i32{1}\n#[cfg(test)] mod t{#[test] fn a(){assert_eq!(super::f(),1);}}\n").unwrap();
        let mut cfg = base_cfg();
        cfg.workspace = Some(ws.clone());
        cfg.memoize = true;
        let memo: Mutex<HashMap<u64, (u32, u32)>> = Mutex::new(HashMap::new());
        let run_root = std::env::temp_dir().join(format!("batch-run-{}", std::process::id()));
        let mut rng = rand::thread_rng();
        let (fit, stats) = evaluate_batched(&cfg, cfg.workspace.as_deref().unwrap(),
            42, 0.95, "local-host", &run_root, &mut rng, Some(&memo), 0, 12);
        assert_eq!(stats.attempts, 12);
        assert_eq!(stats.unique, 1, "no-op edits produce one unique workspace");
        assert_eq!(stats.tests_run, 1, "tested exactly once; the other 11 were dedup hits");
        assert!(fit > 0.0, "the unique workspace passed (no-op leaves the project intact)");
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&run_root);
    }

    #[test]
    fn duration_cache_orders_systems() {
        record_duration(BuildSystem::Cargo, 5.0);
        record_duration(BuildSystem::Pytest, 0.5);
        record_duration(BuildSystem::Ctest, 30.0);
        let mut systems = vec![
            (BuildSystem::Cargo, PathBuf::from("/c")),
            (BuildSystem::Pytest, PathBuf::from("/p")),
            (BuildSystem::Ctest, PathBuf::from("/t")),
        ];
        sort_by_duration(&mut systems);
        let order: Vec<_> = systems.iter().map(|s| s.0).collect();
        assert_eq!(order, vec![BuildSystem::Pytest, BuildSystem::Cargo, BuildSystem::Ctest],
            "fastest-known runs first");
    }

    #[test]
    fn proposer_applies_a_real_patch() {
        if !which("git") { return; }
        let root = std::env::temp_dir().join(format!("prop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"),
            "pub fn f() -> i32 {\n    1\n}\n").unwrap();
        // a stub proposer: emits a diff that changes 1 -> 2
        let script = root.join("fix.sh");
        std::fs::write(&script,
            "#!/bin/sh\ncat >/dev/null\ncat <<'DIFF'\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n pub fn f() -> i32 {\n-    1\n+    2\n }\nDIFF\n").unwrap();
        let mut cfg = Config::default();
        cfg.backend = BackendKind::Local;
        cfg.proposer_cmd = format!("sh {}", script.display());
        // assemble_context runs the test cmd; give it something harmless
        cfg.test_cmd = "true".into();
        let (changed, diff) = proposer_attempt(&root, &cfg, "local-host", 0);
        let after = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        println!("PROP changed={changed} diff_some={} after={after:?}", diff.is_some());
        assert!(changed, "proposer patch should apply");
        assert!(after.contains("2"), "file should now contain the patched value");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn turbo_suppresses_per_bot_flood_but_keeps_summaries() {
        // The looking-glass sender contract: in turbo the engine must NOT emit a Bot
        // message per bot (the flood), but must still emit per-generation summaries;
        // non-turbo keeps full per-bot animation.
        fn count_msgs(turbo: bool) -> (usize, usize) {
            let (tx, rx) = async_channel::unbounded();
            let stop = Arc::new(AtomicBool::new(false));
            let axis = Arc::new(AtomicU32::new(0));
            let mut cfg = Config::default();
            cfg.backend = BackendKind::Sim;
            cfg.population = 80; cfg.generations = 5; cfg.niches = 1;
            cfg.seed = Some(1); cfg.turbo = turbo;
            let h = std::thread::spawn(move || run_engine(cfg, tx, stop, axis));
            let (mut bots, mut gens) = (0usize, 0usize);
            while let Ok(m) = rx.recv_blocking() {
                match m { EngineMsg::Bot { .. } => bots += 1,
                          EngineMsg::Generation { .. } => gens += 1,
                          EngineMsg::Done => break, _ => {} }
            }
            let _ = h.join();
            (bots, gens)
        }
        let (turbo_bots, turbo_gens) = count_msgs(true);
        let (anim_bots, anim_gens) = count_msgs(false);
        // turbo: only bounded one-time setup Bot messages (~2/bot), NOT the per-generation
        // flood (which would be >= population*generations).
        assert!(turbo_bots <= 3 * 80, "turbo emits only setup state, no per-gen flood (got {turbo_bots})");
        assert!(turbo_gens >= 5, "turbo still emits per-generation summaries");
        // non-turbo: per-generation animation present (far more than setup alone).
        assert!(anim_bots >= 80 * 5, "non-turbo animates per generation (got {anim_bots})");
        assert!(anim_bots > turbo_bots * 2, "animation emits much more than turbo setup");
        assert!(anim_gens >= 5, "non-turbo emits summaries too");
    }

    #[test]
    fn proposer_chain_orders_primary_then_fallbacks() {
        let mut cfg = Config::default();
        cfg.proposer_cmd = "fast".into();
        cfg.proposer_fallbacks = vec!["mid".into(), "".into(), "strong".into()];
        // primary first, then non-empty fallbacks in order (empties dropped)
        assert_eq!(proposer_chain(&cfg), vec!["fast", "mid", "strong"]);
        // no primary, only fallbacks
        cfg.proposer_cmd = String::new();
        assert_eq!(proposer_chain(&cfg), vec!["mid", "strong"]);
        // nothing configured → empty (engine uses the mutation path)
        cfg.proposer_fallbacks.clear();
        assert!(proposer_chain(&cfg).is_empty());
    }

    #[test]
    fn experience_log_dedups_identical_fixes() {
        let tmp = std::env::temp_dir().join(format!("explog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("CODEBOT_ARENA_HOME", &tmp);
        let path = tmp.join("experience.jsonl");
        // unique target/diff so the process-global seen-set can't collide with other tests
        let tgt = std::path::Path::new("/dedup-test-unique-xyz");
        let diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b-uniqueXYZ\n";
        log_experience(tgt, &["t::one".into()], diff, 1.0);
        log_experience(tgt, &["t::one".into()], diff, 1.0); // identical → skipped
        log_experience(tgt, &["t::one".into()], diff, 1.0); // identical → skipped
        log_experience(tgt, &["t::two".into()], "--- a/y\n+++ b/y\n@@ -1 +1 @@\n-c\n+d-uniqueXYZ\n", 1.0); // distinct
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let lines = body.lines().filter(|l| l.contains("uniqueXYZ")).count();
        std::env::remove_var("CODEBOT_ARENA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(lines, 2, "three identical fixes collapse to one record; distinct one kept (got {lines})");
    }

    #[test]
    fn auditor_entropy_and_histogram() {
        // all bots on the same op → entropy 0 (fully predictable)
        let ops = all_edit_ops();
        let same = vec![0.0; 50]; // z=0 → first op for everyone
        let h = audit_op_hist(&same, &ops);
        assert_eq!(h.len(), 1, "one op used");
        assert!(norm_entropy(&h.iter().map(|(_, c)| *c).collect::<Vec<_>>()) < 1e-9, "single op → entropy 0");
        // spread across all ops → entropy near 1 (maximally exploratory)
        let spread: Vec<f64> = (0..ops.len()).map(|i| (i as f64 + 0.5) / ops.len() as f64).collect();
        let h2 = audit_op_hist(&spread, &ops);
        assert_eq!(h2.len(), ops.len(), "every op used once");
        assert!(norm_entropy(&h2.iter().map(|(_, c)| *c).collect::<Vec<_>>()) > 0.99, "uniform → entropy ~1");
    }

    #[test]
    fn auditor_overview_reflects_state() {
        let ops = all_edit_ops();
        // converged, predictable, rising, clean
        let a = audit_generation(10, "cleanup", &vec![0.0; 40], &ops, 0.95, 0.88, Some(0.90), 0.03, None, 0);
        let ov = a.overview();
        assert!(ov.contains("predictable"), "low entropy → predictable: {ov}");
        assert!(ov.contains("converged"), "low sigma → converged");
        assert!(ov.contains("rising"), "best up vs prev → rising");
        assert!(ov.contains("no new risk"), "0 flags → clean");
        // risk flag surfaces and trend falls
        let b = audit_generation(11, "fix", &vec![0.0; 40], &ops, 0.80, 0.7, Some(0.95), 0.2, None, 3);
        let ov2 = b.overview();
        assert!(ov2.contains("risk flag(s)") && ov2.contains("falling") && ov2.contains("spread out"), "{ov2}");
        // structured line is grep-able
        assert!(b.structured().contains("risk=3") && b.structured().contains("trend=falling"));
    }

    #[test]
    fn drift_zero_matches_static_and_nonzero_moves() {
        for &z in &[0.0, 0.25, 0.5, 0.75, 0.99] {
            for g in [0usize, 1, 50, 1000] {
                assert!((sim_target_at(z, g, 0.0) - sim_target(z)).abs() < 1e-12,
                    "drift=0 is exactly the static target");
            }
            // with drift the target at two distant generations differs
            let a = sim_target_at(z, 0, 0.5);
            let b = sim_target_at(z, 40, 0.5);
            assert!((a - b).abs() > 1e-3, "drift>0 moves the optimum over generations (z={z})");
            // stays in bounds
            for g in [0usize, 7, 99, 250] {
                let t = sim_target_at(z, g, 1.0);
                assert!((0.05..=0.95).contains(&t), "drifting target stays in [0.05,0.95]");
            }
        }
    }

    #[test]
    fn population_tracks_moving_target() {
        // Run the Sim with a moving optimum; the surviving population should keep its
        // genome near the (drifting) target rather than freezing — i.e. it adapts.
        fn run(drift: f64) -> (f64, f64) {
            let (tx, rx) = async_channel::unbounded();
            let stop = Arc::new(AtomicBool::new(false));
            let axis = Arc::new(AtomicU32::new(0));
            let cfg = Config {
                population: 200, workers_per_container: 1, keep_fraction: 0.4, generations: 80,
                tools: vec![], image: String::new(), workspace: None, workspaces: vec![],
                backend: BackendKind::Sim, corpus: None, niches: 5, coordinator: true,
                test_cmd: "auto".into(), cache: false, fast_clone: false, timeout_secs: 0,
                max_file_mb: 100, memoize: false, crossover: true, sandbox: false, threads: 1,
                ops: all_edit_ops(), aggression: 0.3, seed: Some(11), batch_start: 1, turbo: true,
                allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(),
                extra_tools: vec![], recurrent_steps: 0, novelty_lambda: 0.0, resume: false,
                differential: false, drift, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
            };
            let h = std::thread::spawn(move || run_engine(cfg, tx, stop, axis));
            let (mut last_best, mut last_mean) = (0.0, 0.0);
            while let Ok(m) = rx.recv_blocking() {
                if let EngineMsg::Generation { best, mean, .. } = m { last_best = best; last_mean = mean; }
                if matches!(m, EngineMsg::Done) { break; }
            }
            let _ = h.join();
            (last_best, last_mean)
        }
        let (sb, sm) = run(0.0);
        let (db, dm) = run(0.6);
        println!("PROBE drift=0.0 -> best {sb:.3} mean {sm:.3}");
        println!("PROBE drift=0.6 -> best {db:.3} mean {dm:.3}");
        // A working GA keeps fitness respectable even while chasing; the moving target
        // makes the run *harder* (mean should be lower than the static plateau) but not
        // collapse — that gap is the point: static plateaus, drift stays a live problem.
        assert!(db > 0.4, "population still tracks a moving target (best={db})");
        assert!(dm <= sm + 1e-6, "moving target is at least as hard as static (mean {dm} <= {sm})");
    }

    #[test]
    fn parse_failing_names_across_runners() {
        let cargo = "test tests::ok_one ... ok\ntest tests::bad_two ... FAILED\ntest result: FAILED";
        let f = parse_failing_names(cargo);
        assert_eq!(f, vec!["tests::bad_two".to_string()], "cargo failing name extracted, passing ignored");

        let go = "--- PASS: TestA (0.00s)\n--- FAIL: TestB (0.01s)\nFAIL";
        assert_eq!(parse_failing_names(go), vec!["go::TestB".to_string()]);

        let ctest = "    1/2 Test #1: alpha .......   Passed\n    2/2 Test #2: beta .......***Failed";
        assert_eq!(parse_failing_names(ctest), vec!["ctest::beta".to_string()]);

        let clean = "test tests::a ... ok\ntest tests::b ... ok\ntest result: ok";
        assert!(parse_failing_names(clean).is_empty(), "all-pass output → no failing names");
    }

    #[test]
    fn differential_reward_is_asymmetric() {
        let base = vec!["t::a".to_string(), "t::b".to_string(), "t::c".to_string()]; // 3 failing
        // no change → exactly 0.5
        assert!((differential_reward(&base, &base, 2.0) - 0.5).abs() < 1e-9);
        // fixed 1, broke 0 → above 0.5
        let fix1 = vec!["t::b".to_string(), "t::c".to_string()];
        assert!(differential_reward(&base, &fix1, 2.0) > 0.5);
        // fixed all 3 → max
        assert!((differential_reward(&base, &[], 2.0) - 1.0).abs() < 1e-9);
        // fixed 1 but broke 1 (a previously-passing test now fails) scores LOWER than
        // fixing 1 cleanly — the asymmetric penalty is the whole point
        let fix1_break1 = vec!["t::b".to_string(), "t::c".to_string(), "t::NEW".to_string()];
        assert!(differential_reward(&base, &fix1_break1, 2.0) < differential_reward(&base, &fix1, 2.0),
            "breaking a test costs more than fixing one gains");
        // pure regression → below 0.5
        let break1 = vec!["t::a".to_string(), "t::b".to_string(), "t::c".to_string(), "t::NEW".to_string()];
        assert!(differential_reward(&base, &break1, 2.0) < 0.5);
    }

    #[test]
    fn fingerprint_novelty_rewards_distinct_code() {
        // Two survivors with similar code; a candidate whose code is wholly different
        // should be more "novel" than one that mirrors a survivor — independent of
        // any genome scalar.
        let surv_a = code_fingerprint(&"fn add(a:i32,b:i32)->i32{a+b}\n".repeat(6), FP_DIM);
        let surv_b = code_fingerprint(&"fn sub(a:i32,b:i32)->i32{a-b}\n".repeat(6), FP_DIM);
        let refs = vec![surv_a.clone(), surv_b.clone()];
        let mirror = code_fingerprint(&"fn add(a:i32,b:i32)->i32{a+b}\n".repeat(6), FP_DIM);
        let distinct = code_fingerprint(&"struct Tree{left:Box<Tree>,right:Box<Tree>}\n".repeat(6), FP_DIM);
        let nov = |fp: &Vec<f32>| {
            let s: f64 = refs.iter().map(|r| fingerprint_distance(fp, r) as f64).sum();
            s / refs.len() as f64
        };
        assert!(nov(&distinct) > nov(&mirror),
            "code unlike the survivors scores more novel than near-duplicate code");
    }

    #[test]
    fn empty_fingerprint_earns_no_novelty() {
        // a no-op edit produces an empty fingerprint → contributes 0 novelty,
        // so doing nothing can\'t be rewarded as "diverse".
        let fp = worktree_change_fingerprint(Path::new("/nonexistent-a"), Path::new("/nonexistent-b"));
        assert!(fp.is_empty(), "no detectable change → empty fingerprint");
    }

    #[test]
    fn novelty_changes_selection_order() {
        // Two bots: A has higher fitness but is right next to the lone survivor;
        // B has slightly lower fitness but is far from everyone. With λ=0, A wins.
        // With λ large enough, B's novelty bonus overtakes A's fitness lead.
        let prev_survivors = vec![0.10_f64];
        let bots = [
            (0usize, 0.12_f64, 0.80_f64), // (id, genome, fitness)  — near survivor, slightly better
            (1usize, 0.90_f64, 0.75_f64), // (id, genome, fitness)  — far from survivor, slightly worse
        ];
        let novelty: Vec<f64> = bots.iter().map(|&(_, g, _)| {
            let s: f64 = prev_survivors.iter().map(|sg| (g - sg).abs()).sum();
            s / prev_survivors.len() as f64
        }).collect();
        let sel = |lambda: f64, i: usize| bots[i].2 + lambda * novelty[i];
        // λ = 0 → fitness wins → A first
        let mut order: Vec<usize> = (0..bots.len()).collect();
        order.sort_by(|&a, &b| sel(0.0, b).partial_cmp(&sel(0.0, a)).unwrap());
        assert_eq!(order[0], 0, "λ=0: highest fitness selected");
        // λ = 0.2 → novelty enough to flip → B first
        order.sort_by(|&a, &b| sel(0.2, b).partial_cmp(&sel(0.2, a)).unwrap());
        assert_eq!(order[0], 1, "λ=0.2: novel low-fitness bot now outranks clustered high-fitness one");
    }

    #[test]
    fn batch_size_anneals_to_one() {
        // start=1 means always 1 (feature off)
        assert_eq!(batch_size(0, 10, 1), 1);
        // decays from start at gen 0 to 1 at the last generation
        assert_eq!(batch_size(0, 10, 50), 50);
        assert_eq!(batch_size(9, 10, 50), 1);
        let mid = batch_size(5, 10, 50);
        assert!(mid > 1 && mid < 50, "monotone-ish decay, mid={mid}");
        // never zero
        for g in 0..20 { assert!(batch_size(g, 20, 30) >= 1); }
    }

    #[test]
    fn best_match_finds_closest_reference() {
        let refs = vec![
            "--- a\n+++ b\n@@\n+totally_unrelated();\n".to_string(),
            "--- a\n+++ b\n@@\n+let x = 1;\n+let y = 2;\n".to_string(),
        ];
        let candidate = "--- a\n+++ b\n@@\n+let x = 1;\n+let z = 3;\n"; // shares a line with ref #2
        let m = best_match(candidate, &refs);
        assert!(m > 0.0 && m <= 1.0);
        // an exact copy of a reference matches it fully
        assert_eq!(best_match(&refs[1], &refs), 1.0);
        assert_eq!(best_match("anything", &[]), 0.0);
    }

    #[test]
    fn progression_refs_are_diverse_pairs() {
        if !which("git") { return; }
        let repo = std::env::temp_dir().join(format!("pr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| { std::process::Command::new("git").arg("-C").arg(&repo)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().unwrap(); };
        g(&["init", "-q"]);
        for i in 0..4 {
            std::fs::write(repo.join("f.txt"), format!("line{i}\n")).unwrap();
            g(&["add", "."]); g(&["commit", "-q", "-m", &format!("c{i}")]);
        }
        let refs = progression_refs(&repo, 3);
        assert!(refs.len() >= 2, "got reference pairs");
        for (sha, diff) in &refs {
            assert_eq!(sha.len(), 40, "full sha");
            assert!(!diff.is_empty(), "non-empty diff");
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn sim_fitness_does_not_saturate() {
        // Even a perfectly-matched bot (genome == target, focus == z) with max
        // positive noise stays strictly below 1.0 — so best/mean can't ride 1.0.
        for &z in &[0.05, 0.2, 0.4, 0.5, 0.6, 0.8, 0.95] {
            let t = sim_target(z);
            let best_case = sim_fitness(t, z, z, 0.04);
            assert!(best_case < 1.0, "fitness {best_case} must stay < 1.0 at z={z}");
            assert!(best_case > 0.85, "a matched bot still scores well: {best_case}");
        }
        // Matching the niche's target beats missing it.
        let z = 0.5;
        let t = sim_target(z);
        assert!(sim_fitness(t, z, 0.5, 0.0) > sim_fitness((t + 0.5).min(1.0), z, 0.5, 0.0),
            "closer to the hidden optimum scores higher");
    }

    #[test]
    fn diff_similarity_and_diversification() {
        let d1 = "--- a\n+++ b\n@@\n+let x = 1;\n+let y = 2;\n";
        let d2 = "--- a\n+++ b\n@@\n+let x = 1;\n+let z = 9;\n"; // shares one line
        let d3 = "--- a\n+++ b\n@@\n+totally_different();\n";
        let s12 = diff_similarity(d1, d2);
        let s13 = diff_similarity(d1, d3);
        assert!(s12 > s13, "d1 closer to d2 than d3: {s12} vs {s13}");
        assert_eq!(diff_similarity(d1, d1), 1.0, "identical diffs");
        assert_eq!(diff_similarity(d1, d3), 0.0, "disjoint diffs");

        if !which("git") { return; }
        let repo = std::env::temp_dir().join(format!("dv-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| { std::process::Command::new("git").arg("-C").arg(&repo)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().unwrap(); };
        g(&["init", "-q"]);
        for (i, content) in ["alpha\n", "alpha\nbeta\n", "alpha\nbeta\ngamma\n", "alpha\nbeta\ngamma\ndelta\n"].iter().enumerate() {
            std::fs::write(repo.join("f.txt"), content).unwrap();
            g(&["add", "."]); g(&["commit", "-q", "-m", &format!("c{i}")]);
        }
        let picked = diverse_commits(&repo, 2);
        assert_eq!(picked.len(), 2, "diversified down to k commits");
        assert_ne!(picked[0], picked[1], "distinct commits");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn aggressive_operators_apply() {
        let mut r = rand::thread_rng();
        // math-op wraps a number in an arithmetic expression
        let mut l = vec!["let n = 7;".to_string()];
        assert!(apply_op("math-op", &mut l, 0, 1.0, &mut r));
        assert!(l[0].contains('*') && l[0].contains('('), "math-op rewrote: {}", l[0]);

        // string-op reverses a literal's contents
        let mut l = vec!["let s = \"abc\";".to_string()];
        assert!(apply_op("string-op", &mut l, 0, 0.0, &mut r));
        assert!(l[0].contains("cba"), "string reversed: {}", l[0]);

        // reorder reverses a block (size grows with aggression)
        let mut l = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        assert!(apply_op("reorder", &mut l, 0, 1.0, &mut r));
        assert_ne!(l[0], "a", "block reordered");

        // wrap-loop turns 1 line into 3 (loop header/body/close)
        let mut l = vec!["    work();".to_string()];
        assert!(apply_op("wrap-loop", &mut l, 0, 0.5, &mut r));
        assert_eq!(l.len(), 3);
        assert!(l[0].contains("for _i in 0.."));

        // insert-rand adds a line; aggression scales its length
        let mut timid = vec!["fn f(){ let x = 1; }".to_string()];
        let mut bold = timid.clone();
        apply_op("insert-rand", &mut timid, 0, 0.0, &mut r);
        apply_op("insert-rand", &mut bold, 0, 1.0, &mut r);
        assert!(timid.len() == 2 && bold.len() == 2);
        assert!(bold[0].len() > timid[0].len(), "aggressive insert is longer");
    }

    #[test]
    fn markov_chars_uses_source_alphabet() {
        let mut r = rand::thread_rng();
        let out = markov_chars("abcabcabc", 20, &mut r);
        assert_eq!(out.chars().count(), 20);
        assert!(out.chars().all(|c| "abc".contains(c)), "only source chars: {out}");
    }

    #[test]
    fn deny_and_allow_dirs_enforce_lists() {
        // deny-shadow contains a refusing stub for rm but not for cargo
        if let Some(shadow) = deny_shadow_dir() {
            assert!(shadow.join("rm").exists(), "rm is shadowed");
            assert!(!shadow.join("cargo").exists(), "cargo is not shadowed");
        }
        // allow dir symlinks the toolchain but never a denied binary
        if which("sh") {
            if let Some(allow) = shelter_allow_dir(&["grep".to_string()]) {
                assert!(allow.join("sh").exists(), "essential sh present");
                assert!(allow.join("grep").exists(), "allowed grep present");
                assert!(!allow.join("rm").exists(), "denied rm absent from allow dir");
            }
        }
    }

    #[test]
    fn edit_operators_apply_as_expected() {
        let mut r = rand::thread_rng();
        let mk = |s: &str| vec![s.to_string()];

        let mut l = mk("let ok = true;");
        assert!(apply_op("flip-bool", &mut l, 0, 0.5, &mut r));
        assert_eq!(l[0], "let ok = false;");

        let mut l = mk("let n = 41;");
        assert!(apply_op("tweak-num", &mut l, 0, 0.5, &mut r));
        assert!(l[0] == "let n = 42;" || l[0] == "let n = 40;",
            "tweak-num nudges the literal by ±1, got: {}", l[0]);

        let mut l = mk("    do_thing();");
        assert!(apply_op("comment-out", &mut l, 0, 0.5, &mut r));
        assert_eq!(l[0], "    // do_thing();");
        assert!(apply_op("uncomment", &mut l, 0, 0.5, &mut r));
        assert_eq!(l[0], "    do_thing();");

        let mut l = vec!["a".into(), "b".into()];
        assert!(apply_op("swap-lines", &mut l, 0, 0.5, &mut r));
        assert_eq!(l, vec!["b".to_string(), "a".to_string()]);

        let mut l = vec!["keep".into()];
        assert!(apply_op("dup-line", &mut l, 0, 0.5, &mut r));
        assert_eq!(l.len(), 2);

        let mut l = vec!["x".into()];
        assert!(!apply_op("no-op", &mut l, 0, 0.5, &mut r));
    }

    #[test]
    fn op_for_respects_enabled_set() {
        let ops = vec!["flip-op".to_string(), "no-op".to_string()];
        assert_eq!(op_for(0.0, &ops), "flip-op");
        assert_eq!(op_for(0.99, &ops), "no-op");
        // all ops: high z still lands on the last (no-op)
        let all = all_edit_ops();
        assert_eq!(op_for(0.99, &all), "no-op");
    }

    #[test]
    fn merge_versions_keeps_anchors_picks_hunks() {
        let a = "ctx_top\nA_VARIANT\nctx_bottom";
        let b = "ctx_top\nB_VARIANT\nctx_bottom";
        let mut r = rand::thread_rng();
        let (mut saw_a, mut saw_b) = (false, false);
        for _ in 0..80 {
            let child = merge_versions(a, b, &mut r);
            assert!(child.contains("ctx_top") && child.contains("ctx_bottom"), "anchors preserved");
            let has_a = child.contains("A_VARIANT");
            let has_b = child.contains("B_VARIANT");
            assert!(has_a ^ has_b, "exactly one variant per hunk");
            saw_a |= has_a; saw_b |= has_b;
        }
        assert!(saw_a && saw_b, "both parents contribute across runs (~50/50)");
    }

    #[test]
    fn diff_metrics_reward_concise_and_novel() {
        // Same new identifier introduced two ways: a tiny change scores higher than
        // a sprawling one (concise + creative beats verbose + creative).
        let before = "fn f() { let a = 1; }";
        let small = "fn f() { let widget = 1; }";
        let big = "fn f() {\n let widget = 1;\n let x=2;\n let y=3;\n let z=4;\n let w=5;\n}";
        let m_small = diff_metrics(before, small);
        let m_big = diff_metrics(before, big);
        assert!(m_small.novel_tokens >= 1 && m_big.novel_tokens >= 1);
        assert!(m_small.score > m_big.score, "concise change scores higher: {} vs {}", m_small.score, m_big.score);
        assert!(m_small.lines_changed < m_big.lines_changed);
    }

    #[test]
    fn git_history_helpers_work() {
        if !which("git") { return; }
        let repo = std::env::temp_dir().join(format!("gh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| {
            std::process::Command::new("git").arg("-C").arg(&repo)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).output().unwrap();
        };
        g(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        g(&["add", "."]); g(&["commit", "-q", "-m", "first"]);
        std::fs::write(repo.join("a.txt"), "one\nTWO_NEW_LINE\n").unwrap();
        g(&["add", "."]); g(&["commit", "-q", "-m", "second"]);

        let commits = git_recent_commits(&repo, 10);
        assert_eq!(commits.len(), 2, "two commits");
        let diff = git_commit_diff(&repo, &commits[0]);
        assert!(diff.contains("TWO_NEW_LINE"), "latest commit diff shows the added line");
        // its own diff should apply cleanly after reverting the change
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        assert!(git_patch_applies(&repo, &diff), "the commit's diff re-applies to the reverted tree");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn memoize_reuses_score_for_identical_state() {
        // Two evaluations of a no-op edit (z high → no-op) hit the same worktree
        // hash, so the second is served from the cache without re-scoring.
        let ws = std::env::temp_dir().join(format!("memo-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("Cargo.toml"), "[package]\nname=\"m\"\nversion=\"0.1.0\"\nedition=\"2021\"").unwrap();
        std::fs::write(ws.join("src/lib.rs"), "pub fn f()->i32{1}\n#[cfg(test)] mod t{#[test] fn a(){assert_eq!(super::f(),1);}}\n").unwrap();
        let mut cfg = base_cfg();
        cfg.workspace = Some(ws.clone());
        cfg.memoize = true;
        let memo: Mutex<HashMap<u64, (u32, u32)>> = Mutex::new(HashMap::new());
        let run_root = std::env::temp_dir().join(format!("memo-run-{}", std::process::id()));
        let mut rng = rand::thread_rng();
        let r1 = evaluate_real(&cfg, cfg.workspace.as_deref().unwrap(), 0, 0.95, "local-host", &run_root, &mut rng, Some(&memo), 0);
        let inserted = memo.lock().unwrap().len();
        let r2 = evaluate_real(&cfg, cfg.workspace.as_deref().unwrap(), 1, 0.95, "local-host", &run_root, &mut rng, Some(&memo), 0);
        assert!((r1 - r2).abs() < 1e-9, "cached score matches");
        assert_eq!(inserted, 1, "one state cached after first eval");
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&run_root);
    }

    #[test]
    fn guard_cmd_enforces_timeout() {
        // `sleep 30` under a 1s guard must be killed quickly with non-zero status.
        let dir = std::env::temp_dir();
        let start = std::time::Instant::now();
        let (_out, code) = run_capture(BackendKind::Local, "", &dir, "sleep 30", RunOpts { cache: false, timeout_secs: 1, max_file_mb: 100, sandbox: false, slot: 0, allowed: vec![] });
        assert!(start.elapsed().as_secs() < 8, "timed out promptly");
        assert_ne!(code, Some(0), "killed command is non-zero");
    }

    #[test]
    fn command_genome_vocab_foundations() {
        let paths = extract_filepaths("cargo test --workspace src/lib.rs ./build/out -j 4");
        assert!(paths.contains(&"src/lib.rs".to_string()));
        assert!(paths.contains(&"./build/out".to_string()));
        assert!(!paths.iter().any(|p| p == "--workspace"));

        let help = "Usage: tool [--jobs N] [--verbose] FILE\n  --jobs    number of jobs\n  --jobs    (repeated)\n";
        let vocab = weighted_vocab(help);
        assert!(vocab.iter().any(|(t, c)| t == "--jobs" && *c >= 2), "flags counted by frequency");
        // most frequent token sorts first
        assert_eq!(vocab[0].0, "--jobs");
    }

    #[test]
    fn crossover_child_blends_two_parents() {
        // genome blend lands between the two parents (± small drift).
        let mut r = rand::thread_rng();
        let (ga, gb) = (0.2_f64, 0.8_f64);
        for _ in 0..50 {
            let gmut: f64 = r.gen_range(-0.03..0.06);
            let child = ((ga + gb) / 2.0 + gmut).clamp(0.0, 1.0);
            assert!(child > 0.45 && child < 0.62, "blend near midpoint, got {child}");
        }
    }

    fn base_cfg() -> Config {
        Config {
            population: 1, workers_per_container: 1, keep_fraction: 0.5, generations: 1,
            tools: vec![], image: String::new(), workspace: None, workspaces: vec![], backend: BackendKind::Local,
            corpus: None, niches: 2, coordinator: false, test_cmd: "auto".into(),
            cache: false, fast_clone: true,
            timeout_secs: 0, max_file_mb: 100, memoize: false, crossover: false, sandbox: false, threads: 1, ops: all_edit_ops(), aggression: 0.3, seed: None, batch_start: 1, turbo: false, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        }
    }

    #[test]
    fn git_worktree_provision_and_teardown() {
        if !which("git") { return; }
        let repo = std::env::temp_dir().join(format!("wt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git").arg("-C").arg(&repo)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args)
                .output().unwrap();
        };
        run(&["init", "-q"]);
        std::fs::write(repo.join("hello.txt"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);

        let cfg = base_cfg();
        let work = std::env::temp_dir().join(format!("wt-work-{}", std::process::id()));
        let prov = provision_worktree(&cfg, &repo, &work).unwrap();
        assert!(matches!(prov, Provisioned::Git { .. }), "should use git worktree for a git repo");
        assert!(prov.path().join("hello.txt").exists(), "committed file present in worktree");
        prov.remove();
        assert!(!work.exists(), "worktree removed on teardown");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn missing_tools_flags_unknown_command_but_not_present_ones() {
        // literal command whose program does not exist
        let mut cfg = base_cfg();
        cfg.test_cmd = "totally-not-a-real-binary-xyz --run".into();
        assert_eq!(missing_tools(&cfg), vec!["totally-not-a-real-binary-xyz".to_string()]);

        // auto over a cargo project: cargo is present here, so nothing is missing
        let proj = std::env::temp_dir().join(format!("mt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proj);
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.1.0\"").unwrap();
        cfg.test_cmd = "auto".into();
        cfg.workspace = Some(proj.clone());
        assert!(missing_tools(&cfg).is_empty(), "cargo is on PATH");
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn per_system_parsers_match_real_formats() {
        // cargo workspace prints one summary per crate -> must SUM
        let cargo = "test result: ok. 2 passed; 0 failed; 0 ignored\n\
                     test result: ok. 1 passed; 0 failed; 0 ignored\n\
                     test result: ok. 0 passed; 0 failed; 0 ignored";
        assert_eq!(parse_test_output(cargo, "", Some(0)), (3, 3));

        // ctest summary line
        assert_eq!(parse_ctest("67% tests passed, 1 tests failed out of 3", Some(8)), (2, 3));
        assert_eq!(parse_ctest("100% tests passed, 0 tests failed out of 12", Some(0)), (12, 12));

        // meson summary block
        let meson = "Ok:                 1\nExpected Fail:      0\nFail:               1\nUnexpected Pass:    0\nSkipped:            0\nTimeout:            0";
        assert_eq!(parse_meson(meson, Some(1)), (1, 2));

        // jest: ignore the Test Suites line, read only Tests:
        let jest = "Test Suites: 1 failed, 2 passed, 3 total\nTests:       2 failed, 5 passed, 7 total";
        assert_eq!(parse_jest(jest, Some(1)), (5, 7));
    }

    #[test]
    fn detects_mixed_repo_and_dedups_workspace_members() {
        let root = std::env::temp_dir().join(format!("det-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // cmake root + a cargo workspace (members a,b) + an independent crate `ext`
        // that declares its own [workspace] (so it is NOT part of root's workspace).
        std::fs::create_dir_all(root.join("a/src")).unwrap();
        std::fs::create_dir_all(root.join("b/src")).unwrap();
        std::fs::create_dir_all(root.join("ext/src")).unwrap();
        std::fs::write(root.join("CMakeLists.txt"), "project(x)").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=[\"a\",\"b\"]").unwrap();
        std::fs::write(root.join("a/Cargo.toml"), "[package]\nname=\"a\"").unwrap();
        std::fs::write(root.join("b/Cargo.toml"), "[package]\nname=\"b\"").unwrap();
        std::fs::write(root.join("ext/Cargo.toml"), "[workspace]\n[package]\nname=\"ext\"").unwrap();

        let det = detect_build_systems(&root);
        assert!(det.iter().any(|(s, d)| *s == BuildSystem::Ctest && d == &root));
        assert!(det.iter().any(|(s, d)| *s == BuildSystem::Cargo && d == &root)); // workspace root
        assert!(!det.iter().any(|(s, d)| *s == BuildSystem::Cargo && d == &root.join("a"))); // member dropped
        assert!(!det.iter().any(|(s, d)| *s == BuildSystem::Cargo && d == &root.join("b"))); // member dropped
        assert!(det.iter().any(|(s, d)| *s == BuildSystem::Cargo && d == &root.join("ext"))); // own workspace kept
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scores_real_mixed_repo_end_to_end() {
        // Requires cargo + cmake + a C compiler (present in this environment).
        if !which("cargo") || !which("cmake") { return; }
        let root = std::env::temp_dir().join(format!("mix-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // (1) a standalone cargo crate: 2 passing + 1 failing test
        std::fs::create_dir_all(root.join("rs/src")).unwrap();
        std::fs::write(root.join("rs/Cargo.toml"), "[package]\nname=\"m\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::write(root.join("rs/src/lib.rs"),
            "pub fn f()->i32{1}\n#[cfg(test)] mod t{\n#[test] fn p1(){assert_eq!(super::f(),1);}\n#[test] fn p2(){assert_eq!(super::f(),1);}\n#[test] fn fail(){assert_eq!(super::f(),2);}\n}\n").unwrap();

        // (2) a cmake project: 2 passing + 1 failing ctest
        std::fs::create_dir_all(root.join("c/tests")).unwrap();
        std::fs::write(root.join("c/CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.16)\nproject(d C)\nenable_testing()\nadd_executable(t tests/t.c)\nadd_test(NAME p1 COMMAND t 0)\nadd_test(NAME p2 COMMAND t 0)\nadd_test(NAME fail COMMAND t 1)\n").unwrap();
        std::fs::write(root.join("c/tests/t.c"), "#include <stdlib.h>\nint main(int c,char**v){return c>1?atoi(v[1]):0;}\n").unwrap();

        let cfg = Config {
            population: 1, workers_per_container: 1, keep_fraction: 0.5, generations: 1,
            tools: vec![], image: String::new(), workspace: Some(root.clone()), workspaces: vec![],
            backend: BackendKind::Local, corpus: None, niches: 2, coordinator: false,
            test_cmd: "auto".into(), cache: false, fast_clone: false,
            timeout_secs: 0, max_file_mb: 100, memoize: false, crossover: false, sandbox: false, threads: 1, ops: all_edit_ops(), aggression: 0.3, seed: None, batch_start: 1, turbo: false, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        };
        let det = detect_build_systems(&root);
        assert!(det.iter().any(|(s, _)| *s == BuildSystem::Cargo));
        assert!(det.iter().any(|(s, _)| *s == BuildSystem::Ctest));

        // cargo 2/3 + ctest 2/3 = 4/6 aggregated
        let (passed, total) = score_worktree(&cfg, "", &root, 0);
        assert_eq!((passed, total), (4, 6), "expected 4/6 across cargo+ctest, got {passed}/{total}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cargo_command_injects_features_and_flags() {
        let c = BuildSystem::Cargo.command(false, "gui,extra", "--release -- --nocapture");
        assert!(c.contains("--features gui,extra"), "features injected: {c}");
        assert!(c.contains("--release -- --nocapture"), "extra flags appended: {c}");
        // blank features/flags → clean default command
        let d = BuildSystem::Cargo.command(false, "", "");
        assert_eq!(d, "cargo test --workspace 2>&1");
        // flags also reach pytest/npm/go
        assert!(BuildSystem::Pytest.command(false, "", "-x").contains("-x"));
        assert!(BuildSystem::GoTest.command(false, "", "-run Foo").contains("-run Foo"));
    }

    #[test]
    fn strided_conv_fingerprint_is_locality_sensitive() {
        let base = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n".repeat(6);
        let small_edit = base.replacen("a + b", "a + b + 0", 1);
        let very_diff = "use std::collections::HashMap;\nstruct Tree { left: Box<Tree> }\n".repeat(6);

        let fa = code_fingerprint(&base, 32);
        let fa2 = code_fingerprint(&base, 32);
        let fb = code_fingerprint(&small_edit, 32);
        let fc = code_fingerprint(&very_diff, 32);

        assert_eq!(fa, fa2, "deterministic: same input → same fingerprint");
        let d_same = fingerprint_distance(&fa, &fa2);
        let d_small = fingerprint_distance(&fa, &fb);
        let d_big = fingerprint_distance(&fa, &fc);
        assert!(d_same < 1e-6, "identical code → ~0 distance");
        assert!(d_small < d_big, "a tiny edit must be closer than wholly different code ({d_small} !< {d_big})");
        assert!((fa.iter().map(|x| x * x).sum::<f32>().sqrt() - 1.0).abs() < 1e-3, "unit-normalized");
    }

    #[test]
    fn snapshot_restore_round_trips_sources() {
        let dir = std::env::temp_dir().join(format!("snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        let snap = snapshot_sources(&dir);
        // clobber both files
        std::fs::write(dir.join("src/lib.rs"), "GARBAGE\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "MORE GARBAGE\n").unwrap();
        restore_sources(&snap);
        assert_eq!(std::fs::read_to_string(dir.join("src/lib.rs")).unwrap(), "fn a() {}\nfn b() {}\n");
        assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recurrent_never_regresses_a_passing_baseline() {
        // End-to-end: a crate whose tests already pass. A hill-climb can't beat 1.0,
        // and every breaking mutation must be reverted — so the result stays 1.0.
        // This proves keep-if-improves never lets a regression through and that the
        // early-stop at a perfect score works. (Skipped where cargo is unavailable.)
        if !which("cargo") { return; }
        let root = std::env::temp_dir().join(format!("recur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ws = root.join("crate");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.0.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::write(ws.join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
             #[cfg(test)] mod t { #[test] fn ok() { assert_eq!(super::add(2,2), 4); } }\n").unwrap();

        let mut cfg = base_cfg();
        cfg.fast_clone = false;           // not a git repo → use full copy
        cfg.recurrent_steps = 4;
        cfg.memoize = true;
        let run_root = root.join("run");
        std::fs::create_dir_all(&run_root).unwrap();
        let memo = std::sync::Mutex::new(std::collections::HashMap::new());
        let mut rng = seeded_rng(123);
        let (score, stats) = evaluate_batched(&cfg, &ws, 0, 0.5, "", &run_root, &mut rng, Some(&memo), 0, 1);
        assert_eq!(score, 1.0, "passing baseline must remain 1.0 after hill-climb; stats={stats:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn warmup_challenge_scores_as_fraction_and_does_not_regress() {
        // The keystone end-to-end test: run the REAL Local backend against a
        // bundled challenge crate (deliberately-broken-but-fixable real Rust) and
        // prove (a) the harness parses cargo's mixed pass/fail output into the
        // correct FRACTION — not 0, not 1 — and (b) the recurrent hill-climb runs
        // end-to-end on real code and never scores below the baseline. This is the
        // thing that's been gated on a compiling target for many iterations.
        if !which("cargo") { return; }
        let challenge = Path::new(env!("CARGO_MANIFEST_DIR")).join("challenges/warmup");
        if !challenge.join("Cargo.toml").exists() { return; } // challenges not bundled in this checkout

        let ws = std::env::temp_dir().join(format!("warmup-ws-{}", std::process::id()));
        let run_root = std::env::temp_dir().join(format!("warmup-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&run_root);
        copy_dir_recursive(&challenge, &ws).unwrap();
        std::fs::create_dir_all(&run_root).unwrap();

        let mut cfg = base_cfg();
        cfg.backend = BackendKind::Local;
        cfg.fast_clone = false;   // not a git repo → full copy
        cfg.cache = true;         // shared CARGO_TARGET_DIR → fast incremental recompiles
        cfg.memoize = true;
        cfg.recurrent_steps = 8;

        // (a) baseline must be exactly 2 passed / 5 total
        let (bp, bt, _diag) = baseline_score(&cfg, &ws, &run_root);
        assert_eq!((bp, bt), (2, 5),
            "warmup baseline must parse to 2/5 from real cargo output; got {bp}/{bt}");

        // (b) recurrent hill-climb runs end-to-end and never regresses below 0.40
        let memo = std::sync::Mutex::new(std::collections::HashMap::new());
        let mut rng = seeded_rng(0xC0FFEE);
        let (score, _stats) = evaluate_batched(&cfg, &ws, 0, 0.5, "", &run_root, &mut rng, Some(&memo), 0, 1);
        assert!(score >= 0.4 - 1e-9,
            "hill-climb must not score below the 0.40 baseline; got {score}");

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&run_root);
    }

    #[test]
    #[ignore] // benchmark: run with `cargo test --release -- --ignored --nocapture`
    fn bench_warmup_hillclimb_improves() {
        if !which("cargo") { return; }
        let challenge = Path::new(env!("CARGO_MANIFEST_DIR")).join("challenges/warmup");
        if !challenge.join("Cargo.toml").exists() { return; }
        let ws = std::env::temp_dir().join(format!("warmup-bench-ws-{}", std::process::id()));
        let run_root = std::env::temp_dir().join(format!("warmup-bench-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&run_root);
        copy_dir_recursive(&challenge, &ws).unwrap();
        std::fs::create_dir_all(&run_root).unwrap();
        let mut cfg = base_cfg();
        cfg.backend = BackendKind::Local;
        cfg.fast_clone = false; cfg.cache = true; cfg.memoize = true;
        cfg.recurrent_steps = 200; // generous budget for a stochastic repair search
        let (bp, bt, _) = baseline_score(&cfg, &ws, &run_root);
        println!("BASELINE: {bp}/{bt} = {:.3}", bp as f64 / bt as f64);
        let memo = std::sync::Mutex::new(std::collections::HashMap::new());
        let mut best = 0.0f64;
        for trial in 0..6 {
            let mut rng = seeded_rng(0xBEEF + trial);
            let (score, stats) = evaluate_batched(&cfg, &ws, trial as usize, 0.5, "", &run_root, &mut rng, Some(&memo), 0, 1);
            println!("trial {trial}: score={score:.3}  (steps={}, accepted={}, scored={})",
                stats.attempts, stats.unique, stats.tests_run);
            best = best.max(score);
        }
        println!("BEST AFTER HILLCLIMB: {best:.3}  (baseline was {:.3})", bp as f64 / bt as f64);
        // Show what the swarm LEARNED automatically (no manual pipeline): the
        // test→code associations built from the repairs it just made.
        if let Ok(m) = localizer().lock() {
            println!("LEARNED {} test-token rows from repairs this process:", m.pairs.len());
            let mut rows: Vec<(&String, &HashMap<String, f32>)> = m.pairs.iter().collect();
            rows.sort_by_key(|(k, _)| k.clone());
            for (tt, row) in rows.iter().take(8) {
                let mut cs: Vec<(&String, &f32)> = row.iter().collect();
                cs.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
                let top: Vec<String> = cs.iter().take(4).map(|(c, w)| format!("{c}:{w:.0}")).collect();
                println!("  {tt} → {}", top.join(", "));
            }
        }
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&run_root);
    }

    #[test]
    fn parse_counts() {
        assert_eq!(parse_test_output("2 passed in 0.01s", "", Some(0)), (2, 2));
        assert_eq!(parse_test_output("1 failed, 1 passed in 0.02s", "", Some(1)), (1, 2));
        assert_eq!(parse_test_output("1 error in 0.0s", "", Some(2)), (0, 1));
        // cargo test format (semicolons): "1 passed; 0 failed; 0 ignored; ..."
        assert_eq!(parse_test_output("test result: ok. 12 passed; 0 failed; 0 ignored", "", Some(0)), (12, 12));
        assert_eq!(parse_test_output("test result: FAILED. 3 passed; 2 failed; 0 ignored", "", Some(101)), (3, 5));
        assert_eq!(parse_test_output("custom runner ok", "", Some(0)), (1, 1)); // exit-code fallback
        assert_eq!(parse_test_output("boom", "", Some(1)), (0, 1));
    }

    #[test]
    fn clean_repo_passes_and_broken_fails() {
        let repo = tmp("repo_clean");
        write(&repo, "calc.py", "def add(a, b):\n    return a + b\n");
        write(&repo, "test_calc.py",
            "from calc import add\n\ndef test_a():\n    assert add(1, 2) == 3\n\ndef test_b():\n    assert add(2, 2) == 4\n");
        let (p, t) = run_tests_local(&repo, "python3 -m pytest -q");
        assert_eq!((p, t), (2, 2), "clean repo should pass all tests");

        let broken = tmp("repo_broken");
        write(&broken, "calc.py", "def add(a, b):\n    return a * b\n"); // multiply: clearly wrong
        write(&broken, "test_calc.py",
            "from calc import add\n\ndef test_a():\n    assert add(1, 2) == 3\n\ndef test_b():\n    assert add(2, 2) == 4\n");
        let (p2, t2) = run_tests_local(&broken, "python3 -m pytest -q");
        assert!(p2 < t2, "broken source should fail some tests ({p2}/{t2})");
    }

    #[test]
    fn evaluate_real_noop_is_full_reward() {
        let repo = tmp("repo2");
        write(&repo, "calc.py", "def add(a, b):\n    return a + b\n");
        write(&repo, "test_calc.py",
            "from calc import add\n\ndef test_a():\n    assert add(1, 2) == 3\n");
        let cfg = Config {
            population: 1, workers_per_container: 1, keep_fraction: 0.5, generations: 1,
            tools: vec![], image: String::new(), workspace: Some(repo.clone()), workspaces: vec![],
            backend: BackendKind::Local, corpus: None, niches: 5, coordinator: false,
            test_cmd: "python3 -m pytest -q".into(),
            cache: false,
            fast_clone: false,
            timeout_secs: 0, max_file_mb: 100, memoize: false, crossover: false, sandbox: false, threads: 1, ops: all_edit_ops(), aggression: 0.3, seed: None, batch_start: 1, turbo: false, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        };
        let mut rng = rand::thread_rng();
        let run_root = tmp("run_noop");
        let r = evaluate_real(&cfg, cfg.workspace.as_deref().unwrap(), 0, 0.95, "local-host", &run_root, &mut rng, None, 0); // z high -> no-op edit
        assert!((r - 1.0).abs() < 1e-9, "no-op worker on a passing repo should score 1.0, got {r}");
    }

    #[test]
    fn worktree_is_removed_after_eval() {
        let repo = tmp("repo_clean2");
        write(&repo, "calc.py", "def add(a, b):\n    return a + b\n");
        write(&repo, "test_calc.py",
            "from calc import add\n\ndef test_a():\n    assert add(1, 2) == 3\n");
        let cfg = Config {
            population: 1, workers_per_container: 1, keep_fraction: 0.5, generations: 1,
            tools: vec![], image: String::new(), workspace: Some(repo.clone()), workspaces: vec![],
            backend: BackendKind::Local, corpus: None, niches: 5, coordinator: false,
            test_cmd: "python3 -m pytest -q".into(),
            cache: false,
            fast_clone: false,
            timeout_secs: 0, max_file_mb: 100, memoize: false, crossover: false, sandbox: false, threads: 1, ops: all_edit_ops(), aggression: 0.3, seed: None, batch_start: 1, turbo: false, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        };
        let run_root = tmp("run_cleanup");
        let mut rng = rand::thread_rng();
        let _ = evaluate_real(&cfg, cfg.workspace.as_deref().unwrap(), 7, 0.5, "local-host", &run_root, &mut rng, None, 0);
        assert!(!run_root.join("bot-7").exists(), "per-bot worktree must be removed after scoring");
    }

    #[test]
    fn full_loop_local_runs_real_evaluations() {
        let repo = tmp("loop_repo");
        write(&repo, "calc.py", "def add(a, b):\n    return a + b\n");
        write(&repo, "test_calc.py",
            "from calc import add\n\ndef test_a():\n    assert add(1, 2) == 3\n");
        let cfg = Config {
            population: 6, workers_per_container: 3, keep_fraction: 0.5, generations: 2,
            tools: vec![], image: String::new(), workspace: Some(repo.clone()), workspaces: vec![],
            backend: BackendKind::Local, corpus: None, niches: 5, coordinator: true,
            test_cmd: "python3 -m pytest -q".into(),
            cache: false,
            fast_clone: false,
            timeout_secs: 0, max_file_mb: 100, memoize: false, crossover: false, sandbox: false, threads: 1, ops: all_edit_ops(), aggression: 0.3, seed: None, batch_start: 1, turbo: false, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        };
        let (tx, rx) = async_channel::unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let axis = Arc::new(AtomicU32::new(0.5f32.to_bits()));
        let h = std::thread::spawn(move || run_engine(cfg, tx, stop, axis));

        let (mut gens, mut last_mean, mut done) = (0, -1.0f64, false);
        while let Ok(msg) = rx.recv_blocking() {
            match msg {
                EngineMsg::Generation { mean, .. } => { gens += 1; last_mean = mean; }
                EngineMsg::Done => done = true,
                _ => {}
            }
        }
        h.join().unwrap();
        assert!(done, "engine should signal Done");
        assert_eq!(gens, 2, "should report 2 generations");
        assert!((0.0..=1.0).contains(&last_mean), "mean reward must be a test fraction in [0,1], got {last_mean}");
    }
}

#[cfg(test)]
mod sim_dynamics_probe {
    use super::*;
    #[test]
    fn print_best_mean_curve() {
        fn run(turbo: bool, batch_start: usize, generations: usize, population: usize) -> (f64, f64, std::time::Duration) {
            let (tx, rx) = async_channel::unbounded();
            let stop = Arc::new(AtomicBool::new(false));
            let axis = Arc::new(AtomicU32::new(0));
            let cfg = Config {
                population, workers_per_container: 1, keep_fraction: 0.4, generations,
                tools: vec![], image: String::new(), workspace: None, workspaces: vec![], backend: BackendKind::Sim,
                corpus: None, niches: 5, coordinator: true, test_cmd: "auto".into(), cache: false,
                fast_clone: false, timeout_secs: 0, max_file_mb: 100, memoize: false, crossover: true,
                sandbox: false, threads: 1, ops: all_edit_ops(), aggression: 0.3,
                seed: Some(7), batch_start, turbo, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
            };
            let t0 = std::time::Instant::now();
            let h = std::thread::spawn(move || run_engine(cfg, tx, stop, axis));
            let (mut last_best, mut last_mean) = (0.0, 0.0);
            while let Ok(msg) = rx.recv_blocking() {
                if let EngineMsg::Generation { best, mean, .. } = msg { last_best = best; last_mean = mean; }
                if matches!(msg, EngineMsg::Done) { break; }
            }
            let _ = h.join();
            (last_best, last_mean, t0.elapsed())
        }

        // Speed: turbo runs many generations well under the old 0.33s/gen pacing floor.
        let (best, mean, dur) = run(true, 1, 40, 300);
        println!("PROBE turbo 40 gens x 300 pop: {:.0}ms ({:.1} gens/s), best={best:.3} mean={mean:.3}",
            dur.as_secs_f64() * 1000.0, 40.0 / dur.as_secs_f64());
        assert!(dur.as_secs_f64() < 40.0 * 0.33, "turbo beats the old 0.33s/gen floor");
        assert!(best <= 0.97 && mean <= best);

        // Batch: best-of-k reaches at least as high fitness in the same few generations.
        let (b1, _, _) = run(true, 1, 3, 120);
        let (bk, _, _) = run(true, 20, 3, 120);
        println!("PROBE 3-gen best: batch=1 -> {b1:.3}, batch=20 -> {bk:.3}");
        assert!(bk >= b1, "batch processing reaches at least as high early ({bk} vs {b1})");
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn discover_finds_all_sibling_crates() {
        // a fake superdir of three crates
        let root = std::env::temp_dir().join(format!("disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in ["a", "b", "c"] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.0.0\"\n").unwrap();
        }
        // a noise dir we should skip
        std::fs::create_dir_all(root.join("target").join("debug")).unwrap();
        let found = discover_projects(&root, 3);
        assert_eq!(found.len(), 3, "found three sibling crates, not the noise dir");
        let names: Vec<String> = found.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        for want in ["a", "b", "c"] { assert!(names.contains(&want.to_string())); }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn root_with_manifest_is_one_target() {
        let root = std::env::temp_dir().join(format!("disc2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.0.0\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let found = discover_projects(&root, 4);
        assert_eq!(found, vec![root.clone()], "a project root is itself the only target");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn effective_targets_merges_and_dedups() {
        let root = std::env::temp_dir().join(format!("disc3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in ["one", "two"] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.0.0\"\n").unwrap();
        }
        let mut cfg = Config {
            population:1, workers_per_container:1, keep_fraction:0.5, generations:1,
            tools: vec![], image: String::new(), workspace: None, workspaces: vec![],
            backend: BackendKind::Sim, corpus: None, niches:1, coordinator:false,
            test_cmd:"auto".into(), cache:false, fast_clone:false, timeout_secs:0,
            max_file_mb:100, memoize:false, crossover:false, sandbox:false, threads:1,
            ops: all_edit_ops(), aggression:0.3, seed: None, batch_start:1, turbo:true, allow_test_edits: false, cargo_features: String::new(), build_flags: String::new(), extra_tools: Vec::new(), recurrent_steps: 0, novelty_lambda: 0.0, resume: false, differential: false, drift: 0.0, proposer_cmd: String::new(), proposer_fallbacks: Vec::new(), context_cmds: Vec::new(),
        };
        cfg.workspaces = vec![root.clone(), root.join("one")]; // overlap on purpose
        let t = effective_targets(&cfg);
        assert_eq!(t.len(), 2, "dedup keeps two distinct projects: {t:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
