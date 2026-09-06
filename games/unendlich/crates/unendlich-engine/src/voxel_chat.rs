//! The paintbrush, wired.
//!
//! Owns everything that persists between turns — the chat box, the
//! build session, the model registry, the voice link — and routes
//! every `Command` variant the parser can produce. The game calls
//! `key()` from its input handler, `dispatch()` with a parsed command,
//! and `drain_log()` to draw the chat.
//!
//! Two threading facts shape the API:
//!
//! * A model round-trip takes a second or more. `dispatch` returns
//!   `Pending::Model(prose)` instead of blocking, and the game calls
//!   `finish_turn` when its worker thread has the reply. A blocking
//!   call here would stall the frame.
//! * Writes are staged, not applied. `commit` fills a `Vec` of block
//!   edits; the caller flushes them through `ChunkManager::set_blocks`
//!   (one clone per chunk) and marks the chunks dirty.

// `ai-gpu` must imply `ai-engine`. It did not once, and the failure was
// silent in the worst way: the describe() line said "in-instance" while
// every code path was compiled out, so /build returned None instantly
// and the message told the operator to enable a feature they already
// had. A build with the GPU linked and the seam missing cannot exist.
#[cfg(all(feature = "ai-gpu", not(feature = "ai-engine")))]
compile_error!("feature `ai-gpu` requires `ai-engine`; ai-gpu should list it");


use crate::sim::command_parse::{AnimProfile, Command, FocusAdjust};
use crate::voxel_bind::{flush, WorldView};
use crate::world::chunk_manager::ChunkManager;
use std::time::Instant;
use voxel_tools::anim::{pacing, Frame, Profile};
use voxel_tools::chat::{ChatSession, Reply, VoiceRoute};
use voxel_tools::chatbox::{Action, ChatBox, Key};
use voxel_tools::focus::{Gaze, GazeHit};
use voxel_tools::model::{default_models_dir, ModelRegistry};
use voxel_tools::staged::StagedWorld;
use voxel_tools::voice::{self, VoiceIntent};
use voxel_tools::VoxelTools;

/// Work the game must do that this module cannot do itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    /// Nothing to do.
    None,
    /// Run a model round-trip with this prose, then call `finish_turn`.
    Model(String),
    /// Load this model file, then call `finish_load` / `fail_load`.
    LoadModel(std::path::PathBuf),
    /// Register this grammar with the voiced socket.
    VoiceGrammar(Vec<Vec<String>>),
    /// Stop listening.
    VoiceOff,
    /// Play these camera poses, easing between them with
    /// `camera::lerp`, then return control to the player.
    Camera(Vec<voxel_tools::camera::Pose>),
}

/// Locate a provider script without depending on the process working
/// directory.
///
/// The first version hardcoded `games/unendlich/crates/...`, which is
/// only correct when the game is launched from the monorepo root. Run
/// from `games/unendlich` -- which is where you actually run it -- and
/// it fails with `No such file or directory (os error 2)`, reported
/// once per attempt and impossible to act on without reading the
/// source.
///
/// Resolution order:
///   1. `$UNENDLICH_BUILD_PROVIDER` (may be a command with arguments)
///   2. next to the crate, via `CARGO_MANIFEST_DIR` -- correct for
///      `cargo run` from anywhere in the workspace
///   3. a few paths relative to the CWD, for a launched binary
///   4. next to the executable, for an installed build
pub fn find_provider(name: &str, allow_env: bool) -> Result<Vec<String>, String> {
    // `/build debug` passes allow_env=false: its whole purpose is to
    // exercise the pipeline with a known-good canned plan, so an env
    // override pointing at a real model would defeat it.
    if allow_env {
    if let Ok(v) = std::env::var("UNENDLICH_BUILD_PROVIDER") {
        let argv: Vec<String> = v.split_whitespace().map(String::from).collect();
        if !argv.is_empty() {
            return Ok(argv);
        }
    }
    }
    let mut tried: Vec<String> = Vec::new();
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();

    // CARGO_MANIFEST_DIR is unendlich-engine/; providers live in the
    // sibling voxel-tools crate. Baked at compile time, so it does not
    // care where the binary is started from.
    candidates.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../voxel-tools/providers").join(name));
    for rel in [
        "crates/voxel-tools/providers",
        "games/unendlich/crates/voxel-tools/providers",
        "../crates/voxel-tools/providers",
        "providers",
    ] {
        candidates.push(std::path::Path::new(rel).join(name));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("providers").join(name));
        }
    }
    for c in candidates {
        if c.is_file() {
            let abs = c.canonicalize().unwrap_or(c);
            log::info!("[paintbrush] provider: {}", abs.display());
            return Ok(vec![abs.display().to_string()]);
        }
        tried.push(c.display().to_string());
    }
    Err(format!(
        "provider '{name}' not found. Set UNENDLICH_BUILD_PROVIDER to an \
         executable, or check one of these paths exists:\n  {}",
        tried.join("\n  ")))
}

/// Outcome of a keystroke: a command to dispatch, text for the
/// clipboard, or nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyOut {
    Command(Command),
    Copy(String),
    None,
}

/// Which backend answers a `/build`.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderChoice {
    /// The canned fixture script. No model, no server.
    Fixture,
    /// A subprocess reading `<system>\0<user>` (aperture's CommandProvider).
    Script(Vec<String>),
    /// In-process HTTP to a resident local server. The default worth
    /// using when the engine is not compiled in: a script pays model
    /// LOAD on every request, a server loads once.
    Http(voxel_tools::llm::HttpConfig),
    /// IN-INSTANCE: logscope's wgpu engine (gpu-gemm) linked into this
    /// process through aperture-engine. The model is resident, on the
    /// GPU the renderer already owns. No server, no socket, no second
    /// copy of the weights.
    ///
    /// This is the one to want. HTTP still pays a socket round-trip and
    /// keeps a second process alive holding its own copy of the model;
    /// a game that already has a wgpu device and a model file has no
    /// reason to do either.
    Engine { model_path: String },
}

impl ProviderChoice {
    /// Parse a `/provider` spec.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let s = spec.trim();
        if s.eq_ignore_ascii_case("fixture") { return Ok(ProviderChoice::Fixture); }
        // `engine` with no path uses whatever /model selected.
        if s.eq_ignore_ascii_case("engine") || s.eq_ignore_ascii_case("wgpu") {
            return Ok(ProviderChoice::Engine { model_path: String::new() });
        }
        if let Some(p) = s.strip_prefix("engine:").or_else(|| s.strip_prefix("wgpu:")) {
            return Ok(ProviderChoice::Engine { model_path: p.trim().to_string() });
        }
        if let Some(rest) = s.strip_prefix("script:") {
            let argv: Vec<String> = rest.split_whitespace().map(String::from).collect();
            if argv.is_empty() { return Err("script: needs a command".into()); }
            return Ok(ProviderChoice::Script(argv));
        }
        if s.eq_ignore_ascii_case("script") {
            return Ok(ProviderChoice::Script(Vec::new())); // resolved later
        }
        // bare `ollama` / `openai` get sensible defaults
        let spec = match s.to_lowercase().as_str() {
            "ollama" => "ollama:127.0.0.1:11434",
            "openai" | "llama" | "llamacpp" => "openai:127.0.0.1:8080",
            _ => s,
        };
        voxel_tools::llm::HttpConfig::parse(spec)
            .map(ProviderChoice::Http)
            .ok_or_else(|| format!(
                "unknown provider '{s}'. Try: fixture | script:<cmd> | \
                 ollama[:host:port[:model]] | openai[:host:port]"))
    }

    pub fn describe(&self) -> String {
        match self {
            ProviderChoice::Fixture => "fixture (canned plan, no model)".into(),
            ProviderChoice::Script(a) if a.is_empty() => "script (auto-located)".into(),
            ProviderChoice::Script(a) => format!("script: {}", a.join(" ")),
            ProviderChoice::Engine { model_path } => {
                let which = if model_path.is_empty() { "the /model selection" } else { model_path };
                // Gate on the SAME feature the code is gated on. Checking
                // a different one is how this reported "in-instance" while
                // the engine was compiled out.
                if cfg!(feature = "ai-gpu") {
                    format!("wgpu engine, in-instance ({which})")
                } else if cfg!(feature = "ai-engine") {
                    format!("wgpu engine seam present but GPU not linked ({which}) -- \
                             rebuild with --features ai-gpu")
                } else {
                    format!("wgpu engine ({which}) -- NOT COMPILED IN: \
                             rebuild with --features ai-gpu")
                }
            }
            ProviderChoice::Http(c) => format!(
                "{} at {} (model {}) -- in-process, no subprocess",
                match c.dialect {
                    voxel_tools::llm::Dialect::Ollama => "ollama",
                    voxel_tools::llm::Dialect::OpenAi => "openai",
                },
                c.host, if c.model.is_empty() { "server default" } else { &c.model }),
        }
    }
}

/// A model request running off the frame thread.
///
/// The provider call blocks for as long as generation takes -- 22.5
/// seconds for a 1B in the field log, during which the window is dead:
/// no camera, no chunk streaming, no way to cancel. That is not a
/// pause, it is an unresponsive application.
///
/// So the turn is split. `start` hands the prompt to a worker and
/// returns immediately; `poll` is called each frame and returns the
/// reply when it arrives. The engine handle is `Send + Sync` (aperture
/// requires it of every `Backend`), so the worker can hold a clone
/// while the frame thread keeps drawing.
/// A DRONE in flight: the dry run of a reply, off the frame thread.
///
/// The model call was already off-thread. The dry run that followed it
/// -- parse, validate, and simulate every op against the world -- ran
/// on the frame, and for a landscape plan that is the whole brush and
/// layer computation: hundreds of thousands of cell writes into a
/// staging map, inside one frame. The drone does that against an owned
/// read view of the chunk store and sends back the session state it
/// produced; the frame thread swaps it in, which is a move.
pub struct Drone {
    rx: std::sync::mpsc::Receiver<(voxel_tools::chat::ChatSession, Vec<Reply>)>,
    pub started: Instant,
}

impl Drone {
    pub fn poll(&self) -> Option<(voxel_tools::chat::ChatSession, Vec<Reply>)> {
        self.rx.try_recv().ok()
    }
}

pub struct PendingTurn {
    rx: std::sync::mpsc::Receiver<Result<String, String>>,
    pub started: Instant,
    pub provider: String,
    /// The gaze the turn was started with. A reply must be applied
    /// against the focus it was PLANNED for -- the player will have
    /// moved during those seconds, and @0,0 means where they were
    /// looking when they asked, not where they drifted to.
    pub gaze: Gaze,
    pub player_feet: Option<(i32, i32, i32)>,
    /// What the player asked for, verbatim. The intent gate checks a
    /// model's world ops against THIS; without it every world op is
    /// refused, including one the player explicitly asked for.
    pub prose: String,
    /// What to do with the reply when it lands.
    pub kind: TurnKind,
    /// Cancel flag and live progress, shared with the worker.
    pub ctl: std::sync::Arc<voxel_tools::llm::TurnControl>,
}

/// The kinds of model turn, so the reply is routed without the bin
/// having to remember which it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind { Build, Repair }

impl PendingTurn {
    /// Has the reply arrived? None means still working.
    pub fn poll(&self) -> Option<Result<String, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) =>
                Some(Err("the worker died without answering".into())),
        }
    }
    pub fn elapsed(&self) -> std::time::Duration { self.started.elapsed() }
}

/// Bridges aperture-engine's `Backend` to voxel-tools' `Completer`.
///
/// Two shapes to reconcile. Aperture's is `generate(prompt, max_tokens)`
/// on `&self` (the engine is Send+Sync and shared by handle); ours is
/// `complete(system, user)` on `&mut self`. The system prompt and the
/// user turn are concatenated the way a base-ish model expects — one
/// text, no chat template — because that is what `assemble_prompt` does
/// for the same engine in the editor, and two callers of one engine
/// disagreeing about prompt shape is how you get output that is subtly
/// worse for no visible reason.
#[cfg(feature = "ai-engine")]
pub struct EngineCompleter {
    /// The shared resident engine. Loading happens HERE, in `complete`,
    /// on the worker thread -- not in `completer_for` on the frame.
    /// The GGUF load into the GPU is seconds; done on the frame it was
    /// the block on the first /build that every off-thread change
    /// before this one failed to remove.
    engine: std::sync::Arc<std::sync::Mutex<ResidentEngine>>,
    path: String,
    max_tokens: usize,
}

#[cfg(feature = "ai-engine")]
impl voxel_tools::llm::Completer for EngineCompleter {
    fn name(&self) -> &str { "wgpu engine (in-instance)" }
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        // Load if needed. The lock is held only for the load; generation
        // runs on the handle with the lock released, so a second turn
        // is not serialised behind this one's decode.
        let inner = {
            let mut eng = self.engine.lock().map_err(|_| "engine lock poisoned".to_string())?;
            eng.ensure(&self.path, self.max_tokens)?
        };
        // A CHAT turn, in the model's own template. Sent as raw text an
        // instruct model continued the grammar instead of answering it,
        // and an 8B came back with no usable line. The backend wraps
        // the system half as a stable, cacheable prefix and stops at the
        // model's end-of-turn token.
        inner.generate_chat(system, user, self.max_tokens)
    }

    fn complete_with(&mut self, system: &str, user: &str,
                     ctl: &voxel_tools::llm::TurnControl) -> Result<String, String> {
        let inner = {
            let mut eng = self.engine.lock().map_err(|_| "engine lock poisoned".to_string())?;
            eng.ensure(&self.path, self.max_tokens)?
        };
        // Cancel reaches the decode loop between steps; progress leaves
        // it every few tokens. This is what makes /cancel stop the GPU
        // rather than let it finish a reply nobody will read.
        inner.generate_chat_streaming(system, user, self.max_tokens,
                                      &|| ctl.cancelled(),
                                      &mut |n, text| ctl.report(n, text))
    }
}

/// The resident engine. Loading a GGUF takes seconds and gigabytes, so it
/// happens ONCE and the handle is cloned per request. Held here rather
/// than in the bin so `/provider engine` and `/model` can cooperate.
#[cfg(feature = "ai-engine")]
pub struct ResidentEngine {
    handle: Option<std::sync::Arc<dyn aperture_engine::Backend>>,
    loaded_path: String,
    /// The process's GPU governor. Every engine in this process paces
    /// on it; the frame loop refills it. The game's build model is
    /// Tier::Background: it takes only the slots an interactive engine
    /// and the renderer leave.
    pub governor: aperture_engine::governor::Handle,
}

#[cfg(feature = "ai-engine")]
impl Default for ResidentEngine {
    fn default() -> Self {
        Self { handle: None, loaded_path: String::new(),
               governor: aperture_engine::governor::Handle(std::sync::Arc::new(
                   aperture_engine::governor::Governor::default())) }
    }
}

#[cfg(feature = "ai-engine")]
impl ResidentEngine {
    /// Load `path` unless it is already the loaded one. Returns a handle.
    ///
    /// NOT EXERCISED where this was written: no GPU adapter and no
    /// checkpoint in that sandbox, exactly as aperture-engine's own
    /// `load` says of itself. Compile-checked only.
    pub fn ensure(&mut self, path: &str, max_tokens: usize)
        -> Result<std::sync::Arc<dyn aperture_engine::Backend>, String> {
        if let Some(h) = &self.handle {
            if self.loaded_path == path { return Ok(h.clone()); }
        }
        #[cfg(feature = "ai-gpu")]
        {
            let cfg = aperture_engine::EngineConfig {
                model_path: path.to_string(),
                max_tokens,
                // A plan is structured output: greedy is right, and it is
                // what logscope-serve uses for completions.
                temperature: 0.0,
                top_k: 1,
                top_p: 1.0,
                // The failure mode a plan hits is repeating a line
                // forever; the editor tunes for the same thing.
                repeat_penalty: 1.15,
                repeat_last_n: 64,
                instruction: String::new(),
                // Paced by the game's governor, as a BACKGROUND engine:
                // the renderer and any interactive engine come first.
                governor: Some(self.governor.clone()),
                tier: aperture_engine::governor::Tier::Background,
            };
            let engine = aperture_engine::LocalEngine::load(cfg)?;
            let h = engine.handle();
            self.handle = Some(h.clone());
            self.loaded_path = path.to_string();
            Ok(h)
        }
        #[cfg(not(feature = "ai-gpu"))]
        {
            let _ = (path, max_tokens);
            Err("wgpu engine not compiled in -- rebuild with --features ai-gpu".into())
        }
    }

    pub fn loaded(&self) -> Option<&str> {
        self.handle.as_ref().map(|_| self.loaded_path.as_str())
    }
}

pub struct Paintbrush {
    pub chatbox: ChatBox,
    pub session: ChatSession,
    pub models: ModelRegistry,
    log: Vec<String>,
    /// Frames from the last commit, drained by the animator.
    reveal: Vec<Frame>,
    reveal_fps: f32,
    load_started: Option<Instant>,
    chunk_size: i32,
    /// Current backend. Defaults from $UNENDLICH_BUILD_HTTP if set, so a
    /// running ollama is picked up without typing anything.
    pub provider: ProviderChoice,
    /// Claims, ownership and build history; saved with the world.
    pub land: voxel_tools::land::Registry,
    /// Templates the player captured; saved with the world.
    pub library: voxel_tools::template::Library,
    /// The world tree: every commit, branches, merges; saved with the world.
    pub repo: voxel_tools::vcs::Repo,
    /// Set by anything that changes land, library or repo; the bin
    /// saves when it sees it.
    pub state_dirty: bool,
    /// Who is building, for claims and commits. The bin sets it from
    /// the profile; "player" until then.
    pub player: String,
    /// Living plants a committed plan asked the engine to register.
    /// The bin drains these into the flora store each frame; from then
    /// on the lifecycle owns them and the builder's bred genome is
    /// what they grow with.
    pub pending_plants: Vec<voxel_tools::flora::PlantRequest>,
    /// THE HIVE. Every committed change is sharded into chunk-sized
    /// cells and applied by the queen within the frame's budget, nearest
    /// the player first. A 200,000-block commit used to be one flush and
    /// one remesh of every touched chunk inside a single frame.
    pub comb: voxel_tools::hive::Comb,
    pub queen: voxel_tools::hive::Queen,
    next_job: u32,
    /// Last frame's total time, fed to the queen so she adapts.
    pub last_frame_ms: f32,
    /// Where the player was last frame, for the comb's priorities and
    /// for noticing when they have moved enough to re-sort.
    obs_cache: Option<voxel_tools::hive::Observer>,
    /// A dry run in flight on a drone. At most one: a second reply
    /// while one is being simulated waits its turn.
    pub drone: Option<Drone>,
    /// The resident in-instance engine, if one has been loaded.
    #[cfg(feature = "ai-engine")]
    pub engine: std::sync::Arc<std::sync::Mutex<ResidentEngine>>,
    /// Context window and reply budget, used to size the system prompt.
    /// 8192/512 unless the engine reports otherwise.
    pub ctx_tokens: usize,
    pub max_reply: usize,
    /// A model request in flight, if any. One at a time: a second
    /// /build while one is running replaces nothing and is refused,
    /// because two plans racing to become `pending` is a coin flip.
    pub in_flight: Option<PendingTurn>,
}

impl Paintbrush {
    pub fn new(chunk_size: i32) -> Self {
        let models = ModelRegistry::scan(default_models_dir());
        let mut chatbox = ChatBox::new();
        // Tab-complete model names from the first scan.
        chatbox.set_dynamic_args("/model", models.models.iter().map(|m| m.name.clone()).collect());
        log::info!("[paintbrush] scanned {} .gguf in {}",
                   models.models.len(), models.dir.display());
        let mut log = Vec::new();
        log.push(format!("{} models in {}", models.models.len(), models.dir.display()));
        log.push("Enter to chat. Try: /build debug a tower".into());
        Self {
            chatbox,
            session: ChatSession::new(chunk_size, 200),
            models,
            log,
            reveal: Vec::new(),
            reveal_fps: 12.0,
            load_started: None,
            chunk_size,
            provider: std::env::var("UNENDLICH_BUILD_HTTP").ok()
                .and_then(|s| ProviderChoice::parse(&s).ok())
                // With the engine compiled in, in-instance is the right
                // default: the weights are already on disk and the GPU is
                // already open.
                .unwrap_or(if cfg!(feature = "ai-gpu") {
                    ProviderChoice::Engine { model_path: String::new() }
                } else {
                    ProviderChoice::Fixture
                }),
            #[cfg(feature = "ai-engine")]
            engine: std::sync::Arc::new(std::sync::Mutex::new(ResidentEngine::default())),
            in_flight: None,
            drone: None,
            land: voxel_tools::land::Registry::new(),
            library: voxel_tools::template::Library::new(),
            repo: voxel_tools::vcs::Repo::new(chunk_size),
            state_dirty: false,
            player: "player".into(),
            pending_plants: Vec::new(),
            comb: voxel_tools::hive::Comb::new(chunk_size),
            queen: voxel_tools::hive::Queen::new(voxel_tools::hive::Budget::default()),
            next_job: 1,
            last_frame_ms: 16.0,
            obs_cache: None,
            ctx_tokens: 8192,
            max_reply: 512,
        }
    }

    pub fn say(&mut self, s: impl Into<String>) { self.log.push(s.into()); }
    pub fn drain_log(&mut self) -> Vec<String> { std::mem::take(&mut self.log) }
    pub fn log(&self) -> &[String] { &self.log }

    /// Build a `Gaze` from a raycast result. Call each frame.
    pub fn gaze_from(eye: [f32; 3], dir: [f32; 3],
                     hit: Option<([i32; 3], [i32; 3], f32)>) -> Gaze {
        Gaze {
            eye: (eye[0], eye[1], eye[2]),
            dir: (dir[0], dir[1], dir[2]),
            hit: hit.map(|(b, n, d)| GazeHit {
                block: (b[0], b[1], b[2]),
                face_normal: (n[0], n[1], n[2]),
                distance: d,
            }),
        }
    }

    /// What a keystroke produced.
    ///
    /// `key()` could only return a Command, which cannot express "put
    /// this on the clipboard" -- the box knows the text, but only the
    /// window can set the clipboard. `key_full` is the complete form;
    /// `key` stays for callers that only care about commands.
    pub fn key_full(&mut self, k: Key) -> KeyOut {
        match self.chatbox.key(k) {
            Action::Copy(text) => KeyOut::Copy(text),
            other => match Self::action_to_command(other) {
                Some(c) => KeyOut::Command(c),
                None => KeyOut::None,
            },
        }
    }

    fn action_to_command(a: Action) -> Option<Command> {
        match a {
            Action::Submit(line) => Some(crate::sim::command_parse::parse(&line)),
            Action::Commit => Some(Command::CommitPlan),
            Action::Undo => Some(Command::UndoBuild),
            Action::FocusBigger => Some(Command::Focus(Some(FocusAdjust::Bigger))),
            Action::FocusSmaller => Some(Command::Focus(Some(FocusAdjust::Smaller))),
            _ => None,
        }
    }

    /// Feed a key, keeping only a resulting command. Kept because most
    /// callers do not care about the clipboard; `key_full` is the
    /// complete form. One implementation, so the two cannot drift.
    pub fn key(&mut self, k: Key) -> Option<Command> {
        match self.key_full(k) {
            KeyOut::Command(c) => Some(c),
            _ => None,
        }
    }

    /// True while the box has the keyboard: the game must not move the
    /// player. Forgetting this is why you walk off a cliff mid-sentence.
    pub fn captures_input(&self) -> bool { self.chatbox.open }

    /// Route a parsed command. Returns work for the caller.
    pub fn dispatch(&mut self, cm: &ChunkManager, gaze: &Gaze, cmd: Command) -> Pending {
        // Trace every command. The first field session produced 14k lines
        // of log with not one mention of the paintbrush, because every
        // message went to the in-game chat panel and nothing to `log`.
        // A feature you cannot diagnose from a log is a feature you
        // cannot support.
        log::info!("[paintbrush] dispatch {:?} (chat_open={})", cmd, self.chatbox.open);
        match cmd {
            Command::Build(prose) => {
                // `/build debug ...` uses the fixture provider, which needs
                // no weights at all -- gating it on a loaded model defeated
                // the one command whose job is to prove the pipeline works
                // before you have a model. It reported "no model loaded"
                // and stopped, which is exactly what "nothing happened"
                // looks like from the outside.
                // Clear any pending plan as the turn STARTS. If this turn
                // fails, /commit must find nothing -- not the previous
                // turn's plan, which it would rebuild without a word.
                if self.session.clear_pending() {
                    self.say("(replacing the pending plan)");
                }
                let is_debug = prose.trim_start().starts_with("debug");
                log::info!("[paintbrush] /build debug={} ready={} prose={:?}",
                           is_debug, self.models.state.is_ready(), prose);
                // Ask the PROVIDER whether it can answer, not the model
                // registry. `/model` is only one of the ways weights get
                // chosen: `/provider engine:/path.gguf` carries its own,
                // and an ollama or openai server has its own already
                // loaded. Gating those on a /model that is irrelevant to
                // them refused turns that would have worked.
                if !is_debug {
                    if let Some(why) = self.provider_not_ready() {
                        self.say(why);
                        return Pending::None;
                    }
                }
                Pending::Model(prose)
            }
            Command::BuildVoice(on) => {
                let (reply, grammar) = self.session.set_voice(on);
                self.push(reply);
                if on { Pending::VoiceGrammar(grammar) } else { Pending::VoiceOff }
            }
            Command::ShowPlan => { let r = self.session.show_plan(); self.push(r); Pending::None }
            Command::CommitPlan => { self.commit(cm, gaze); Pending::None }
            Command::UndoBuild => {
                let view = WorldView(cm);
                let mut staged = StagedWorld::new(&view);
                let mut tools = VoxelTools::new(&mut staged);
                let r = self.session.undo(&mut tools);
                self.push(r);
                self.flush_staged(cm, &mut staged);
                Pending::None
            }
            Command::Focus(adj) => {
                let r = self.session.adjust_focus(gaze, adj.map(|a| a == FocusAdjust::Bigger));
                self.push(r);
                Pending::None
            }
            Command::Anim(p) => {
                let prof = match p {
                    AnimProfile::GroundUp => Profile::GroundUp,
                    AnimProfile::Physics => Profile::Physics,
                    AnimProfile::Random => Profile::Random,
                    AnimProfile::Instant => Profile::Instant,
                };
                let r = self.session.set_profile(prof);
                self.push(r);
                Pending::None
            }
            Command::Provider(None) => {
                self.say(format!("provider: {}", self.provider.describe()));
                self.say("switch with: /provider engine (in-instance wgpu, no server) \
                          | /provider ollama | /provider openai:127.0.0.1:8080 \
                          | /provider script:./my.sh | /provider fixture");
                Pending::None
            }
            Command::Provider(Some(spec)) => {
                match ProviderChoice::parse(&spec) {
                    Ok(p) => {
                        log::info!("[paintbrush] provider -> {}", p.describe());
                        self.say(format!("provider: {}", p.describe()));
                        self.provider = p;
                    }
                    Err(e) => self.say(e),
                }
                Pending::None
            }
            Command::Cancel => {
                if !self.cancel_turn() { self.say("nothing in flight"); }
                Pending::None
            }
            Command::Shot { kind, id } => {
                // Subject: a named workset, else the last build.
                let subject = match id.as_deref() {
                    Some(i) => self.land.get(i).map(|w| (w.name.clone(), w.min, w.max)),
                    None => self.session.last_build.as_ref()
                        .map(|lb| (lb.name.clone(), lb.min, lb.max)),
                };
                let Some((name, min, max)) = subject else {
                    self.say("nothing to frame -- build something, or /shot <kind> <id>");
                    return Pending::None;
                };
                use voxel_tools::camera::{self, Subject};
                let subj = Subject { min, max };
                let eye = [gaze.eye.0, gaze.eye.1, gaze.eye.2];
                let fov = 1.2;
                let poses = match kind.as_str() {
                    "frame" | "hero" => vec![camera::frame(&subj, fov, eye)],
                    "orbit" => camera::orbit(&subj, fov, 24, 8.0),
                    "reveal" => camera::reveal(&subj, fov, eye, 4.0),
                    "fly" => {
                        // Along the build's own footprint at head height:
                        // in through the near face, across, and out.
                        let y = min.1;
                        let cx = (min.0 + max.0) / 2;
                        camera::flythrough(&[
                            (cx, y, min.2 - 6), (cx, y, min.2), (cx, y, (min.2 + max.2) / 2),
                            (cx, y, max.2), (cx, y, max.2 + 6)], 1.7, 6.0)
                    }
                    other => {
                        self.say(format!("unknown shot '{other}': frame, orbit, reveal, fly"));
                        return Pending::None;
                    }
                };
                let total: f32 = poses.iter().map(|p| p.seconds).sum();
                self.say(format!("{kind} of \"{name}\": {} pose(s), {total:.1}s -- \
                                  press any key to stop", poses.len()));
                log::info!("[paintbrush] shot {kind} of {name}: {} poses", poses.len());
                Pending::Camera(poses)
            }
            Command::Refine(which) => {
                let view = WorldView(cm);
                let mut staged = StagedWorld::new(&view);
                let tools = VoxelTools::new(&mut staged);
                let Some(rungs) = self.session.refine_ladder(&tools, gaze) else {
                    self.say("nothing to refine -- build something first");
                    return Pending::None;
                };
                let ir = match which.as_deref() {
                    None => { self.say(voxel_tools::refine::menu(&rungs)); return Pending::None; }
                    Some("all") => voxel_tools::refine::all(&rungs),
                    Some(name) => match voxel_tools::refine::rung(&rungs, name) {
                        Some(r) => { self.say(format!("{}: {}", r.name, r.what)); r.ir.clone() }
                        None => {
                            self.say(format!("no pass '{name}'; try: {}",
                                rungs.iter().map(|r| r.name).collect::<Vec<_>>().join(", ")));
                            return Pending::None;
                        }
                    },
                };
                drop(tools);
                // A refinement is a plan like any other: previewed,
                // committed and undone on its own, which is the whole
                // reason it is a ladder rather than one "finish" button.
                // And simulated on a drone like any other.
                self.launch_drone(cm, gaze, ir, None, "");
                Pending::None
            }
            Command::Repair => {
                // On the worker, like every model call. The reply is
                // routed to `finish_repair` by its TurnKind.
                self.start_repair(gaze, None);
                Pending::None
            }
            Command::Hive => {
                if self.comb.is_empty() {
                    self.say(format!("hive idle; budget {:.1} ms/frame, a cell costs ~{:.2} ms here",
                                     self.queen.budget.ms, self.queen.budget.cell_cost_ms));
                } else {
                    for line in self.comb.dance(self.queen.rate()) { self.say(line); }
                    self.say(format!("{} chunks waiting; {:.1} ms/frame budget, {:.0} chunks/s",
                                     self.comb.len(), self.queen.budget.ms, self.queen.rate()));
                }
                Pending::None
            }
            Command::Deferred { drop } => {
                if drop {
                    let n = self.session.deferred.len();
                    self.session.deferred.cells.clear();
                    self.say(format!("dropped {n} deferred block(s)"));
                } else {
                    self.say(self.session.deferred.message());
                }
                Pending::None
            }
            Command::Land(op) => {
                use crate::sim::command_parse::LandOp;
                match op {
                    LandOp::Here => {
                        let (x, z) = (gaze.eye.0 as i32, gaze.eye.2 as i32);
                        // Collect the text first: `at` borrows the registry
                        // and `say` needs the whole paintbrush.
                        let lines: Vec<String> = self.land.at(x, z, 8).iter().map(|w| w.summary()).collect();
                        if lines.is_empty() {
                            self.say(format!("no claims here ({} in the world); /land claim <name> \
                                              claims your last build", self.land.len()));
                        } else {
                            for l in lines { self.say(l); }
                        }
                    }
                    LandOp::Info(id) => {
                        let found = self.land.get(&id).map(|w| (w.summary(), w.history_for_prompt(6)));
                        match found {
                            Some((a, b)) => { self.say(a); self.say(b); }
                            None => self.say(format!("no workset '{id}'")),
                        }
                    }
                    LandOp::Add { id, player } => {
                        let actor = self.player.clone();
                        match self.land.add_member(&id, &actor, &player) {
                            Ok(()) => { self.say(format!("{player} may now build on {id}")); self.state_dirty = true; }
                            Err(e) => self.say(e),
                        }
                    }
                    LandOp::Claim(name) => match self.session.last_build.as_ref() {
                        Some(lb) => {
                            let id = self.land.claim(&name, &self.player, lb.min, lb.max);
                            let ir = self.session.last_ir.clone();
                            self.land.record(&id, voxel_tools::land::Turn {
                                player: self.player.clone(), prose: self.session.last_prose.clone(),
                                ir, blocks: 0 }, lb.min, lb.max);
                            self.state_dirty = true;
                            self.say(format!("claimed {id} \"{name}\" at {:?}..{:?}", lb.min, lb.max));
                        }
                        None => self.say("nothing to claim -- build something first"),
                    },
                }
                Pending::None
            }
            Command::Keys => {
                for c in voxel_tools::chatbox::CARDS {
                    if c.command == "/keys" { self.say(c.summary.replace(" · ", "\n  ")); }
                }
                Pending::None
            }
            Command::Model(None) => {
                log::info!("[paintbrush] /model list: {} models in {}",
                           self.models.models.len(), self.models.dir.display());
                let l = self.models.listing();
                self.say(l);
                // State line: the single question worth answering after a
                // load is "will the next /build call a model or the
                // fixture", and until now that was only inferable.
                self.say(format!(
                    "state: {} -> next /build uses {}",
                    self.models.state.message(),
                    if self.models.state.is_ready() { "llama-cli.sh" } else { "the FIXTURE" }));
                Pending::None
            }
            Command::Model(Some(name)) => match self.models.begin_load(&name) {
                Ok(m) => {
                    let path = m.path.clone();
                    log::info!("[paintbrush] loading model {}", path.display());
                    let msg = self.models.state.message();
                    self.say(msg);
                    self.load_started = Some(Instant::now());
                    Pending::LoadModel(path)
                }
                Err(e) => {
                    log::warn!("[paintbrush] /model {name:?} failed: {e}");
                    self.say(e);
                    Pending::None
                }
            },
            _ => Pending::None,     // not ours; the game's existing arms handle it
        }
    }

    /// The model replied. `raw` is the completion text.
    pub fn finish_turn(&mut self, cm: &ChunkManager, gaze: &Gaze, raw: &str,
                       player_feet: Option<(i32, i32, i32)>) {
        self.finish_turn_with(cm, gaze, raw, player_feet, "")
    }

    /// As `finish_turn`, with the player's prose so the world-op gate
    /// can check the model's proposals against what was actually asked.
    pub fn finish_turn_with(&mut self, cm: &ChunkManager, gaze: &Gaze, raw: &str,
                            player_feet: Option<(i32, i32, i32)>, prose: &str) {
        let ir = voxel_tools::llm::clean_reply(raw);
        log::info!("[paintbrush] provider returned {} chars -> {} plan lines",
                   raw.len(), ir.lines().count());
        if ir.trim().is_empty() {
            self.say(format!("the model replied ({} chars) but produced no plan lines", raw.len()));
            return;
        }
        self.launch_drone(cm, gaze, ir, player_feet, prose);
    }

    /// Hand a dry run to a DRONE. Every path that previews IR goes
    /// through here -- a model reply, a macro, a refinement rung, a
    /// repair -- so none of them simulates on the frame thread.
    ///
    /// A clone of the session carries everything the simulation reads
    /// (focus, climate, last build, budget); it comes back with the
    /// pending plan set, and the frame thread swaps that state in. The
    /// session is small; the clone is cheap. The world is read through
    /// an owned view of the chunk store, so nothing borrows the frame.
    ///
    /// Refuses while one is already flying: two dry runs racing to set
    /// `pending` would leave whichever landed last as the plan, which
    /// might not be the one the player asked for last.
    pub fn launch_drone(&mut self, cm: &ChunkManager, gaze: &Gaze, ir: String,
                        player_feet: Option<(i32, i32, i32)>, prose: &str) {
        if self.drone.is_some() {
            self.say("still simulating the previous plan -- a moment");
            return;
        }
        let session = self.session.clone();
        let drone_world = crate::voxel_bind::DroneWorld(cm.drone_view());
        let gaze = *gaze;
        let prose = prose.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("paintbrush-drone".into()).spawn(move || {
            crate::thread_priority::configure_current_thread(
                crate::thread_priority::ThreadRole::ChunkLoader);
            let mut session = session;
            let mut staged = StagedWorld::new(&drone_world);
            let mut tools = VoxelTools::new(&mut staged);
            let r = session.receive_plan_gated(&mut tools, &gaze, &ir, player_feet, &prose);
            debug_assert_eq!(staged.pending(), 0, "preview must not stage writes");
            let _ = tx.send((session, vec![r]));
        }).expect("spawn drone");
        self.drone = Some(Drone { rx, started: Instant::now() });
        self.say("simulating ... (the world keeps running)");
    }

    /// Collect a finished drone: swap in the session state it produced
    /// and show its reply. Call every frame; cheap when nothing is
    /// flying.
    pub fn poll_drone(&mut self) {
        let Some(d) = &self.drone else { return };
        let Some((session, replies)) = d.poll() else { return };
        let took = self.drone.take().map(|d| d.started.elapsed()).unwrap_or_default();
        log::info!("[paintbrush] drone finished in {:.0} ms", took.as_secs_f32() * 1000.0);
        // Only the fields the dry run produces move over. Anything the
        // frame changed meanwhile -- climate, deferred cells, the land
        // registry -- stays as it is now.
        self.session.pending = session.pending;
        self.session.last_salvage = session.last_salvage;
        for r in replies { self.push(r); }
    }

    /// Start a turn on a worker thread. Returns immediately.
    ///
    /// `completer_for` runs HERE, on the frame thread, because it may
    /// load a model and the resident engine is not shareable while
    /// loading. Generation -- the part that takes twenty seconds --
    /// runs on the worker.
    pub fn start_turn(&mut self, cm: &ChunkManager, gaze: &Gaze,
                      surface: Option<&str>, prose: &str,
                      player_feet: Option<(i32, i32, i32)>) {
        if let Some(t) = &self.in_flight {
            self.say(format!("already asking {} ({:.0}s) -- wait or /cancel",
                             t.provider, t.elapsed().as_secs_f32()));
            return;
        }
        let debug = prose.trim_start().starts_with("debug");
        // A recognized macro against the build in focus needs no model:
        // "fortify my base" is geometry once the base's box is known.
        if !debug {
            if let Some(ir) = self.session.macro_ir(prose, gaze) {
                log::info!("[paintbrush] macro expanded, no model call: {prose:?}");
                self.say("(expanded without the model)");
                self.launch_drone(cm, gaze, ir, player_feet, prose);
                return;
            }
            // "Connect the mill to the barn": names resolved against the
            // land registry, then a ROAD that deals with whatever is in
            // the way. No model.
            if matches!(voxel_tools::verbs::recognize(prose), Some(voxel_tools::verbs::Verb::Connect)) {
                match voxel_tools::chat::ChatSession::connect_ir(prose, &self.land) {
                    Ok(ir) => {
                        self.say("(road planned without the model)");
                        self.launch_drone(cm, gaze, ir, player_feet, prose);
                        return;
                    }
                    Err(e) => { self.say(e); return; }
                }
            }
        }
        // Same borrow shape as `prompt_for`: the view lives for the
        // length of this call, which is all a prompt needs -- it reads
        // the world and never writes.
        // The context -- terrain read, ambient, region digest -- is a
        // few hundred thousand block reads on a fresh focus. It used to
        // be built HERE, on the frame, at the moment of Enter: a visible
        // hitch before "asking ..." appeared. It is built on the worker
        // now, from a clone of the session over an owned view of the
        // chunk store, immediately before the model call it feeds.
        let session = self.session.clone();
        let drone_world = crate::voxel_bind::DroneWorld(cm.drone_view());
        let (gaze_c, surface_c, prose_c) = (*gaze, surface.map(|s| s.to_string()), prose.to_string());
        let user: Box<dyn FnOnce() -> String + Send> = Box::new(move || {
            let mut staged = StagedWorld::new(&drone_world);
            let tools = VoxelTools::new(&mut staged);
            let (u, _pass) = session.user_turn(&tools, &gaze_c, surface_c.as_deref(), &prose_c, None);
            u
        });
        // Context-sized system prompt: a small model given the long one
        // echoes it back instead of answering.
        let system = self.system_prompt();
        let Some(completer) = self.completer_for(debug) else {
            self.say("no provider available -- /provider to check");
            return;
        };
        self.spawn_turn(completer, system, user, gaze, player_feet, prose, TurnKind::Build);
    }

    /// The system prompt sized for the LOADED model: a 1B gets the
    /// compact one, an 8B the medium, a 70B the full. The log showed
    /// 10,338 chars going to a 1B because the choice was by context
    /// alone and `ctx_tokens` was a constant that never crossed the
    /// threshold. Size comes from the registry, ctx from the load.
    fn prompt_tier(&self) -> voxel_tools::prompt::Tier {
        let (params, ctx) = match &self.models.state {
            voxel_tools::model::LoadState::Loaded { name, ctx, .. } => (
                self.models.resolve(name).ok().map(|m| m.params_billions).filter(|b| *b > 0.0),
                *ctx as usize),
            _ => (None, self.ctx_tokens),
        };
        voxel_tools::prompt::tier_for(params, ctx)
    }

    fn system_prompt(&self) -> String {
        let (params, ctx) = match &self.models.state {
            voxel_tools::model::LoadState::Loaded { name, ctx, .. } => (
                self.models.resolve(name).ok().map(|m| m.params_billions).filter(|b| *b > 0.0),
                *ctx as usize),
            _ => (None, self.ctx_tokens),
        };
        let tier = voxel_tools::prompt::tier_for(params, ctx);
        log::info!("[paintbrush] prompt tier {tier:?} (params {params:?}, ctx {ctx})");
        voxel_tools::prompt::system_prompt_for_model(params, ctx, self.max_reply)
    }

    /// Put a model call on the worker thread. The ONE place a completer
    /// is invoked from the game: every kind of turn -- build, outline,
    /// repair -- goes through here, so none of them can run the model
    /// on the frame. The `/repair` handler once called `complete`
    /// inline in the bin: a twenty-second stall, the exact thing the
    /// rest of this file exists to prevent.
    fn spawn_turn(&mut self, mut completer: Box<dyn voxel_tools::llm::Completer>,
                  system: String, user: Box<dyn FnOnce() -> String + Send>, gaze: &Gaze,
                  player_feet: Option<(i32, i32, i32)>, prose: &str, kind: TurnKind) {
        let name = completer.name().to_string();
        log::info!("[paintbrush] asking {name} ({} chars system, ctx {}, {kind:?})",
                   system.len(), self.ctx_tokens);
        self.say(format!("asking {name} ... (the world keeps running)"));
        let (tx, rx) = std::sync::mpsc::channel();
        let ctl = std::sync::Arc::new(voxel_tools::llm::TurnControl::default());
        let ctl_w = ctl.clone();
        std::thread::spawn(move || {
            // Same treatment as the chunk loaders: deprioritised and, on
            // a heterogeneous SoC, pinned to the efficiency cores. The
            // frame thread keeps the performance cores; on a four-core
            // phone an AI thread at equal priority was a fifth of the
            // machine taken from the renderer for a minute.
            crate::thread_priority::configure_current_thread(
                crate::thread_priority::ThreadRole::ChunkLoader);
            // The user prompt is built here, on the worker.
            let user = user();
            log::info!("[paintbrush] user prompt {} chars", user.len());
            let r = completer.complete_with(&system, &user, &ctl_w);
            // A closed receiver means the turn was cancelled; nothing to
            // report and nobody to report to.
            let _ = tx.send(r);
        });
        self.in_flight = Some(PendingTurn {
            rx, started: Instant::now(), provider: name, gaze: *gaze, player_feet,
            prose: prose.to_string(), kind, ctl,
        });
    }

    /// Test hook: start a build turn with a given completer and a fixed
    /// prompt, so a harness can drive cancel and progress without a
    /// provider script.
    pub fn spawn_turn_for_test(&mut self, completer: Box<dyn voxel_tools::llm::Completer>, gaze: &Gaze) {
        self.spawn_turn(completer, "system".into(), Box::new(|| "user".into()), gaze, None, "test",
                        TurnKind::Build);
    }

    /// Start a repair turn: the model is asked to fix only the lines it
    /// got wrong, on the worker like any other turn.
    pub fn start_repair(&mut self, gaze: &Gaze, player_feet: Option<(i32, i32, i32)>) {
        if self.in_flight.is_some() {
            self.say("a turn is already in flight -- wait or /cancel");
            return;
        }
        let Some(user) = self.session.repair_prompt() else {
            self.say("nothing to repair -- the last plan had no bad lines");
            return;
        };
        let system = self.system_prompt();
        let Some(completer) = self.completer_for(false) else {
            self.say("no provider available -- /provider to check");
            return;
        };
        self.spawn_turn(completer, system, Box::new(move || user), gaze, player_feet, "", TurnKind::Repair);
    }

    /// The model answered a repair turn with fixed lines. Merge them
    /// with the salvaged good lines and preview the whole thing.
    pub fn finish_repair(&mut self, cm: &ChunkManager, gaze: &Gaze, raw: &str,
                         player_feet: Option<(i32, i32, i32)>) {
        let fixed = voxel_tools::llm::clean_reply(raw);
        let Some(merged) = self.session.merge_repair(&fixed) else {
            self.say("no repair was pending");
            return;
        };
        log::info!("[paintbrush] repair merged: {} fixed line(s)", fixed.lines().count());
        self.launch_drone(cm, gaze, merged, player_feet, "");
    }

    /// Call every frame. Applies a reply once the worker has one.
    pub fn poll_turn(&mut self, cm: &ChunkManager) {
        let Some(turn) = &self.in_flight else { return };
        let Some(result) = turn.poll() else { return };
        let turn = self.in_flight.take().expect("just checked");
        match result {
            Ok(raw) => {
                log::info!("[paintbrush] reply in {:.1}s, {} chars",
                           turn.elapsed().as_secs_f32(), raw.len());
                match turn.kind {
                    TurnKind::Build =>
                        self.finish_turn_with(cm, &turn.gaze, &raw, turn.player_feet, &turn.prose),
                    TurnKind::Repair =>
                        self.finish_repair(cm, &turn.gaze, &raw, turn.player_feet),
                }
            }
            Err(e) => {
                log::warn!("[paintbrush] provider failed: {e}");
                self.say(format!("provider failed: {e}"));
            }
        }
    }

    /// Drop a request in flight. The worker finishes and its answer is
    /// discarded -- there is no way to interrupt a generate() mid-token
    /// through aperture's Backend, and pretending otherwise would leave
    /// a thread writing into a channel nobody reads.
    pub fn cancel_turn(&mut self) -> bool {
        match self.in_flight.take() {
            Some(t) => {
                // The flag reaches the decode loop between steps: the GPU
                // stops within one token. Before this the worker ran the
                // whole reply out, minutes of paced GPU for nothing.
                t.ctl.cancel();
                let (n, _) = t.ctl.snapshot();
                self.say(format!("cancelled after {:.0}s and {n} tokens; generation stops",
                                 t.elapsed().as_secs_f32()));
                true
            }
            None => false,
        }
    }

    /// Why the current provider cannot answer, or None if it can.
    /// Cheap and side-effect free: it must not load anything.
    pub fn provider_not_ready(&self) -> Option<String> {
        match &self.provider {
            // These bring their own weights or talk to something that has.
            ProviderChoice::Fixture => None,
            ProviderChoice::Script(_) => None,
            ProviderChoice::Http(_) => None,
            ProviderChoice::Engine { model_path } => {
                if !model_path.is_empty() || self.models.state.is_ready() {
                    None
                } else {
                    Some("the engine has no model -- /model <name>, \
                          or /provider engine:/path/to/model.gguf".into())
                }
            }
        }
    }

    /// Build the completer for a turn. Everything the bin needs to know
    /// about provider selection lives here.
    ///
    /// Returns None with a message already in the log when the choice
    /// cannot be honoured -- a missing script, an engine that is not
    /// compiled in, a `/provider engine` with no model chosen.
    pub fn completer_for(&mut self, debug: bool)
        -> Option<Box<dyn voxel_tools::llm::Completer>> {
        use voxel_tools::llm::{CommandProvider, HttpProvider};
        let choice = if debug { ProviderChoice::Fixture } else { self.provider.clone() };
        log::info!("[paintbrush] provider choice: {}", choice.describe());
        match choice {
            ProviderChoice::Http(cfg) => Some(Box::new(HttpProvider::new(cfg))),
            ProviderChoice::Fixture | ProviderChoice::Script(_) => {
                let argv = match &choice {
                    ProviderChoice::Script(a) if !a.is_empty() => Ok(a.clone()),
                    ProviderChoice::Script(_) => find_provider("llama-cli.sh", true),
                    _ => find_provider("fixture.sh", !debug),
                };
                match argv {
                    Ok(a) => Some(Box::new(CommandProvider::new(a))),
                    Err(e) => { self.say(e); None }
                }
            }
            ProviderChoice::Engine { model_path } => self.engine_completer(&model_path),
        }
    }

    #[cfg(feature = "ai-engine")]
    fn engine_completer(&mut self, model_path: &str)
        -> Option<Box<dyn voxel_tools::llm::Completer>> {
        // An empty path means "whatever /model selected", so the two
        // commands cooperate instead of each holding their own idea of
        // which weights are in play.
        let path = if model_path.is_empty() {
            match &self.models.state {
                voxel_tools::model::LoadState::Loaded { name, .. } => {
                    match self.models.resolve(name) {
                        Ok(m) => m.path.display().to_string(),
                        Err(e) => {
                            log::warn!("[paintbrush] engine: cannot resolve model: {e}");
                            self.say(e);
                            return None;
                        }
                    }
                }
                _ => {
                    log::warn!("[paintbrush] engine: no model chosen");
                    self.say("no model chosen -- /model <name> first, \
                              or /provider engine:/path/to/model.gguf");
                    return None;
                }
            }
        } else { model_path.to_string() };

        let loaded = self.engine.lock().map(|e| e.loaded() == Some(path.as_str())).unwrap_or(false);
        if !loaded {
            self.say(format!("loading {path} into the GPU on the worker (once) ..."));
        }
        // No load here. The completer loads on the worker when it runs.
        // The reply cap follows the prompt tier: a 1B that runs long is
        // looping, a 70B village needs the room. 512 flat cut an 8B's
        // hut off mid-plan.
        let max_tokens = match self.prompt_tier() {
            voxel_tools::prompt::Tier::Compact => 400,
            voxel_tools::prompt::Tier::Medium => 900,
            voxel_tools::prompt::Tier::Full => 1400,
        };
        Some(Box::new(EngineCompleter { engine: self.engine.clone(), path, max_tokens }))
    }

    #[cfg(not(feature = "ai-engine"))]
    fn engine_completer(&mut self, _model_path: &str)
        -> Option<Box<dyn voxel_tools::llm::Completer>> {
        log::warn!("[paintbrush] engine requested but ai-engine is not compiled in");
        self.say("the in-instance wgpu engine is not compiled in (no ai-engine \
                  feature) -- rebuild with --features ai-gpu, or /provider ollama");
        None
    }

    /// The context and prompt for the pending turn. Call on the worker
    /// thread; both halves are pure.
    pub fn prompt_for(&self, cm: &ChunkManager, gaze: &Gaze, surface: Option<&str>, prose: &str)
        -> (String, String) {
        let view = WorldView(cm);
        let mut staged = StagedWorld::new(&view);
        let tools = VoxelTools::new(&mut staged);
        let (user, _pass) = self.session.user_turn(&tools, gaze, surface, prose, None);
        (voxel_tools::prompt::system_prompt(), user)
    }

    fn commit(&mut self, cm: &ChunkManager, gaze: &Gaze) {
        let view = WorldView(cm);
        let mut staged = StagedWorld::new(&view);
        {
            let mut tools = VoxelTools::new(&mut staged);
            let r = self.session.commit(&mut tools, gaze);
        // Living plants planted by this commit go to the engine.
        if let Some(plants) = self.session.take_plants() {
            let n = plants.len();
            self.pending_plants.extend(plants);
            if n > 0 { self.say(format!("{n} living plant(s) registered with the engine")); }
        }
            self.push(r);
        }
        let touched = staged.chunks_touched(self.chunk_size);
        let n = staged.pending();
        log::info!("[paintbrush] commit: {n} blocks across {touched} chunk(s)");
        if n > 0 {
            // THE WORLD TREE: every commit is a commit, with its IR and
            // anchor, so it can be branched, merged and rebased later.
            // And the workset journal, when the build is inside a claim,
            // so "make this larger" has the original request to read.
            let changes = staged.snapshot();
            if let Some(lb) = self.session.last_build.as_ref() {
                let ir = self.session.last_ir.clone();
                let anchor = (lb.min.0, lb.min.2);
                self.repo.commit_changes_at(&self.player, &lb.name, &ir, anchor, &changes);
                let cx = (lb.min.0 + lb.max.0) / 2;
                let cz = (lb.min.2 + lb.max.2) / 2;
                if let Some(ws) = self.land.at(cx, cz, 0).first().map(|w| w.id.clone()) {
                    self.land.record(&ws, voxel_tools::land::Turn {
                        player: self.player.clone(), prose: self.session.last_prose.clone(),
                        ir, blocks: n }, lb.min, lb.max);
                }
                self.state_dirty = true;
            }
        }
        self.flush_staged(cm, &mut staged);
        if n > 0 {
            self.reveal = self.session.reveal(25);
            let total: usize = self.reveal.iter().map(|f| f.block_count()).sum();
            self.reveal_fps = pacing(total, 25, 3.0);
            self.say(format!("{n} blocks, {touched} chunk(s) remeshed, {} frames",
                             self.reveal.len()));
        }
    }

    fn flush_staged(&mut self, cm: &ChunkManager, staged: &mut StagedWorld<'_, WorldView<'_>>) {
        if staged.pending() == 0 { return; }
        // Small edits go straight in: sharding forty blocks is overhead
        // for nothing, and the player expects a door to appear NOW.
        // Anything larger is offered to the comb and the queen applies it
        // over the coming frames within her budget. Was 4,000, which is
        // still four chunks remeshed in one frame; a tower at 300 is
        // fine immediately, a hut at 800 is a hitch.
        const IMMEDIATE: usize = 512;
        if staged.pending() > IMMEDIATE {
            let changes = staged.snapshot();
            let name = self.session.last_build.as_ref().map(|b| b.name.clone())
                .unwrap_or_else(|| "build".into());
            let job = self.next_job; self.next_job += 1;
            let cells = voxel_tools::hive::shard(changes, self.chunk_size, job);
            let n = cells.len();
            let obs = self.observer();
            self.comb.offer(&name, cells, &obs);
            staged.clear();
            self.say(format!("{n} chunks queued; filling nearest-first without dropping frames \
                              (/hive for progress)"));
            log::info!("[hive] job {job} '{name}': {n} cells offered");
            return;
        }
        // Snapshot the cells BEFORE draining: a skipped cell has to be
        // remembered with its target block, or it is gone.
        let snapshot: Vec<voxel_tools::weather::Change> = staged.snapshot();
        let (applied, skipped, _chunks) = flush(cm, staged, self.chunk_size);
        log::info!("[paintbrush] flush: applied={applied} skipped={skipped}");
        if skipped > 0 {
            // Which ones? The ones whose chunk is not loaded. Keep those
            // and retry when the chunk appears -- previously they were
            // mentioned once and the build kept its hole forever.
            let missing: Vec<_> = snapshot.into_iter()
                .filter(|c| !cm.is_loaded_at(c.at.0, c.at.1, c.at.2))
                .collect();
            let n = missing.len();
            self.session.deferred.cells.extend(missing);
            self.say(format!("{n} block(s) deferred until their chunk loads \
                              (they place themselves when you go there; /deferred to see)"));
        }
        let _ = applied;
    }

    fn observer(&self) -> voxel_tools::hive::Observer {
        self.obs_cache.unwrap_or(voxel_tools::hive::Observer {
            pos: [0.0, 64.0, 0.0], look: [1.0, 0.0, 0.0] })
    }

    /// The queen's frame. Call once per frame AFTER the renderer has
    /// been given its frame time, so she adapts to headroom and never
    /// runs past it. Applies as many cells as fit, nearest first.
    pub fn tick_hive(&mut self, cm: &ChunkManager, eye: [f32; 3], look: [f32; 3],
                     last_frame_ms: f32, now: f64) -> voxel_tools::hive::Tick {
        let obs = voxel_tools::hive::Observer { pos: eye, look };
        // The SWARM: re-sort when the player has moved a chunk or so.
        let moved = self.obs_cache.map(|o| {
            let d = [o.pos[0] - eye[0], o.pos[1] - eye[1], o.pos[2] - eye[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 24.0 * 24.0
        }).unwrap_or(true);
        if moved && !self.comb.is_empty() { self.comb.swarm(&obs); }
        self.obs_cache = Some(obs);
        self.last_frame_ms = last_frame_ms;
        if self.comb.is_empty() { return voxel_tools::hive::Tick::default(); }
        let cs = self.chunk_size;
        let mut deferred: Vec<voxel_tools::weather::Change> = Vec::new();
        let t = self.queen.tick(&mut self.comb, last_frame_ms, now, |cell| {
            let t0 = std::time::Instant::now();
            let edits: Vec<(i32, i32, i32, crate::world::blocks::BlockId)> = cell.changes.iter()
                .map(|c| (c.at.0, c.at.1, c.at.2, crate::voxel_bind::block_from_u16(c.to)))
                .collect();
            let (_applied, skipped) = cm.set_blocks(&edits);
            if skipped > 0 {
                deferred.extend(cell.changes.iter()
                    .filter(|c| !cm.is_loaded_at(c.at.0, c.at.1, c.at.2)).cloned());
            }
            let _ = cs;
            t0.elapsed()
        });
        if !deferred.is_empty() { self.session.deferred.cells.extend(deferred); }
        if t.cells > 0 { self.session.invalidate_context(); }
        t
    }

    /// Load claims, templates and the world tree from the world folder.
    /// Missing files are an empty state, not an error: a world made
    /// before this existed has nothing to load.
    pub fn load_state(&mut self, world_root: &std::path::Path) {
        let dir = world_root.join("paintbrush");
        if let Ok(t) = std::fs::read_to_string(dir.join("land.txt")) {
            self.land = voxel_tools::land::Registry::load(&t);
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("templates.txt")) {
            self.library = voxel_tools::template::Library::load(&t);
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("worldtree.txt")) {
            self.repo = voxel_tools::vcs::Repo::load(&t);
        }
        log::info!("[paintbrush] loaded {} claim(s), {} template(s), {} branch(es) from {}",
                   self.land.len(), self.library.len(), self.repo.branches().count(), dir.display());
        self.state_dirty = false;
    }

    /// Save them. Called by the bin when `state_dirty` is set -- after a
    /// commit, a claim, a capture -- so a crash loses at most the last
    /// action. Small files; a write is cheap.
    pub fn save_state(&mut self, world_root: &std::path::Path) -> std::io::Result<()> {
        if !self.state_dirty { return Ok(()); }
        let dir = world_root.join("paintbrush");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("land.txt"), self.land.save())?;
        std::fs::write(dir.join("templates.txt"), self.library.save())?;
        std::fs::write(dir.join("worldtree.txt"), self.repo.save())?;
        self.state_dirty = false;
        Ok(())
    }

    /// One line for the HUD while a turn is in flight: tokens, rate,
    /// and lines of plan so far. None when idle. A 200-second wait with
    /// no feedback reads as a hang; this is the difference.
    pub fn status_line(&self) -> Option<String> {
        let t = self.in_flight.as_ref()?;
        let (n, text) = t.ctl.snapshot();
        let secs = t.elapsed().as_secs_f32().max(0.1);
        let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
        Some(if n == 0 {
            format!("{}: thinking ... {secs:.0}s", t.provider)
        } else {
            format!("{}: {n} tokens, {:.1} tok/s, {lines} line(s) so far -- /cancel to stop",
                    t.provider, n as f32 / secs)
        })
    }

    /// The pending plan's landing spot for the renderer's marker:
    /// (name, world position at the anchor's surface, blocks).
    pub fn waypoint(&self) -> Option<(String, [f32; 3], usize)> {
        self.session.pending_waypoint().map(|w| {
            // The marker sits at the centre of the dry run's box when
            // there is one -- a village's anchor may be a corner of it
            // -- else at the anchor, a block above the ground.
            let p = match w.bounds {
                Some((lo, hi)) => [(lo.0 + hi.0) as f32 / 2.0 + 0.5, hi.1 as f32 + 1.5,
                                   (lo.2 + hi.2) as f32 / 2.0 + 0.5],
                None => [w.anchor.0 as f32 + 0.5, w.anchor.1 as f32 + 1.5, w.anchor.2 as f32 + 0.5],
            };
            (w.name, p, w.blocks)
        })
    }

    /// The frame loop's call for GPU pacing: refill the governor's
    /// slots from the previous frame's time. Cheap; call every frame
    /// beside `tick_hive`. A no-op without the in-instance engine.
    pub fn frame_gpu(&self, last_frame_ms: f32) {
        #[cfg(feature = "ai-engine")]
        if let Ok(e) = self.engine.lock() { e.governor.frame(last_frame_ms); }
        #[cfg(not(feature = "ai-engine"))]
        let _ = last_frame_ms;
    }

    /// Hand committed plant requests to the flora store. Called by the
    /// bin each frame with the current frame count for `planted_tick`.
    pub fn drain_plants(&mut self, flora: &mut crate::world::flora_store::FloraStore, tick: u64) -> usize {
        let n = self.pending_plants.len();
        for p in self.pending_plants.drain(..) {
            let mut fc = crate::world::flora_store::FloraCell::sown(p.species_block as u8, tick);
            fc.growth01 = p.growth;
            // a mature placement is already at its final stage
            fc.stage = if p.growth >= 0.99 { 4 } else { 0 };
            fc.vigor = 0.5 + p.genome[voxel_tools::flora::GROWTH_RATE] * 0.4;
            fc.genome = p.genome;
            flora.plant([p.at.0, p.at.1, p.at.2], fc);
        }
        n
    }

    /// Retry deferred cells whose chunk has since loaded. Called every
    /// frame; cheap when nothing is waiting.
    pub fn retry_deferred(&mut self, cm: &ChunkManager) {
        if self.session.deferred.is_empty() { return; }
        let cs = self.chunk_size;
        let ready = self.session.deferred.take_ready(cs,
            &|c| cm.is_loaded_at(c.0 * cs, c.1 * cs, c.2 * cs));
        if ready.is_empty() { return; }
        let edits: Vec<(i32, i32, i32, crate::world::blocks::BlockId)> = ready.iter()
            .map(|c| (c.at.0, c.at.1, c.at.2, crate::voxel_bind::block_from_u16(c.to)))
            .collect();
        let (applied, skipped) = cm.set_blocks(&edits);
        log::info!("[paintbrush] deferred retry: applied={applied} skipped={skipped}");
        if applied > 0 {
            self.session.invalidate_context();
            self.say(format!("{applied} deferred block(s) placed now that their chunk loaded"));
        }
    }

    /// A voiced `event`. Returns work, exactly like `dispatch`.
    pub fn voice_event(&mut self, cm: &ChunkManager, gaze: &Gaze,
                       verb: &str, args: &[String]) -> Pending {
        let Some(intent) = voice::parse_event(verb, args) else { return Pending::None };
        let needs_model = matches!(intent, VoiceIntent::Build(_));
        let view = WorldView(cm);
        let mut staged = StagedWorld::new(&view);
        let route = {
            let mut tools = VoxelTools::new(&mut staged);
            self.session.voice(&mut tools, gaze, &intent)
        };
        match route {
            VoiceRoute::NeedsModel(prose) => {
                if !self.models.state.is_ready() {
                    self.say("heard you, but no model is loaded");
                    return Pending::None;
                }
                Pending::Model(prose)
            }
            VoiceRoute::Done(r) => {
                self.push(r);
                // a spoken commit stages blocks like a typed one
                self.flush_staged(cm, &mut staged);
                if needs_model { Pending::None } else { Pending::None }
            }
            VoiceRoute::Ignored => Pending::None,
        }
    }

    pub fn finish_load(&mut self, ctx: u32) {
        let took = self.load_started.take().map(|t| t.elapsed()).unwrap_or_default();
        let msg = self.models.finish_load(took, ctx);
        self.say(msg);
    }

    pub fn fail_load(&mut self, why: impl Into<String>) {
        self.load_started = None;
        let msg = self.models.fail_load(why);
        self.say(msg);
    }

    /// Frames left to reveal, and the rate. The animator pops one per
    /// tick and marks each frame's chunks dirty.
    pub fn take_reveal(&mut self) -> (Vec<Frame>, f32) {
        (std::mem::take(&mut self.reveal), self.reveal_fps)
    }

    /// Route a session reply into the log and pending state. Public so
    /// the harness can drive the paintbrush with hand-written IR.
    pub fn push(&mut self, r: Reply) {
        match r {
            Reply::Say(m) => self.log.push(m),
            Reply::Preview { echo, blocks, .. } => {
                self.log.push(echo);
                self.log.push(format!("{blocks} blocks planned -- /commit to build"));
            }
            Reply::Committed { blocks, batches, .. } =>
                self.log.push(format!("built {blocks} blocks in {batches} batches")),
            Reply::Nothing => {}
        }
    }
}
