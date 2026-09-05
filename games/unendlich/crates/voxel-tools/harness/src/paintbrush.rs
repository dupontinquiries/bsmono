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

mod log {
    macro_rules! info { ($($t:tt)*) => { eprintln!($($t)*) } }
    macro_rules! warn_ { ($($t:tt)*) => { eprintln!($($t)*) } }
    pub(crate) use info;
    pub(crate) use warn_ as warn;
}

// `ai-gpu` must imply `ai-engine`. It did not once, and the failure was
// silent in the worst way: the describe() line said "in-instance" while
// every code path was compiled out, so /build returned None instantly
// and the message told the operator to enable a feature they already
// had. A build with the GPU linked and the seam missing cannot exist.
#[cfg(all(feature = "ai-gpu", not(feature = "ai-engine")))]
compile_error!("feature `ai-gpu` requires `ai-engine`; ai-gpu should list it");


use crate::stubs::{AnimProfile, Command, FocusAdjust};
use crate::stubs::parse as parse_command;
use crate::bind::{flush, WorldView};
use crate::stubs::ChunkManager;
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
}

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
    inner: std::sync::Arc<dyn aperture_engine::Backend>,
    max_tokens: usize,
}

#[cfg(feature = "ai-engine")]
impl voxel_tools::llm::Completer for EngineCompleter {
    fn name(&self) -> &str { "wgpu engine (in-instance)" }
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        self.inner.generate(&format!("{system}\n\n{user}"), self.max_tokens)
    }
}

/// The resident engine. Loading a GGUF takes seconds and gigabytes, so it
/// happens ONCE and the handle is cloned per request. Held here rather
/// than in the bin so `/provider engine` and `/model` can cooperate.
#[cfg(feature = "ai-engine")]
#[derive(Default)]
pub struct ResidentEngine {
    handle: Option<std::sync::Arc<dyn aperture_engine::Backend>>,
    loaded_path: String,
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
    /// The resident in-instance engine, if one has been loaded.
    #[cfg(feature = "ai-engine")]
    pub engine: ResidentEngine,
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
            engine: ResidentEngine::default(),
            in_flight: None,
            land: voxel_tools::land::Registry::new(),
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
            Action::Submit(line) => Some(parse_command(&line)),
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
        let ir = voxel_tools::llm::clean_reply(raw);
        log::info!("[paintbrush] provider returned {} chars -> {} plan lines",
                   raw.len(), ir.lines().count());
        if ir.trim().is_empty() {
            self.say(format!("the model replied ({} chars) but produced no plan lines", raw.len()));
            return;
        }
        let view = WorldView(cm);
        let mut staged = StagedWorld::new(&view);
        let mut tools = VoxelTools::new(&mut staged);
        let r = self.session.receive_plan_for(&mut tools, gaze, &ir, player_feet);
        self.push(r);
        // A preview must leave nothing staged: receive_plan_for reverses
        // its own dry run, so this is an assertion, not a cleanup.
        debug_assert_eq!(staged.pending(), 0, "preview must not stage writes");
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
                let view = WorldView(cm);
                let mut staged = StagedWorld::new(&view);
                let mut tools = VoxelTools::new(&mut staged);
                let r = self.session.receive_plan_for(&mut tools, gaze, &ir, player_feet);
                self.push(r);
                return;
            }
        }
        // Same borrow shape as `prompt_for`: the view lives for the
        // length of this call, which is all a prompt needs -- it reads
        // the world and never writes.
        let user = {
            let view = WorldView(cm);
            let mut staged = StagedWorld::new(&view);
            let tools = VoxelTools::new(&mut staged);
            let (u, _pass) = self.session.user_turn(&tools, gaze, surface, prose, None);
            u
        };
        // Context-sized system prompt: a small model given the long one
        // echoes it back instead of answering.
        let system = voxel_tools::prompt::system_prompt_for(self.ctx_tokens, self.max_reply);
        let Some(mut completer) = self.completer_for(debug) else {
            self.say("no provider available -- /provider to check");
            return;
        };
        let name = completer.name().to_string();
        log::info!("[paintbrush] asking {name} ({} + {} chars, ctx {})",
                   system.len(), user.len(), self.ctx_tokens);
        self.say(format!("asking {name} ... (the world keeps running)"));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = completer.complete(&system, &user);
            // A closed receiver means the turn was cancelled; nothing to
            // report and nobody to report to.
            let _ = tx.send(r);
        });
        self.in_flight = Some(PendingTurn {
            rx, started: Instant::now(), provider: name, gaze: *gaze, player_feet,
        });
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
                self.finish_turn(cm, &turn.gaze, &raw, turn.player_feet);
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
                self.say(format!("cancelled after {:.0}s (the worker finishes in the \
                                  background and its answer is dropped)",
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

        if self.engine.loaded() != Some(path.as_str()) {
            self.say(format!("loading {path} into the GPU (once) ..."));
        }
        log::info!("[paintbrush] engine: ensuring {path}");
        match self.engine.ensure(&path, 512) {
            Ok(inner) => {
                log::info!("[paintbrush] engine ready");
                Some(Box::new(EngineCompleter { inner, max_tokens: 512 }))
            }
            Err(e) => {
                log::warn!("[paintbrush] engine load failed: {e}");
                self.say(format!("engine: {e}"));
                None
            }
        }
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
            self.push(r);
        }
        let touched = staged.chunks_touched(self.chunk_size);
        let n = staged.pending();
        log::info!("[paintbrush] commit: {n} blocks across {touched} chunk(s)");
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
        let (applied, skipped, _chunks) = flush(cm, staged, self.chunk_size);
        log::info!("[paintbrush] flush: applied={applied} skipped={skipped}");
        if skipped > 0 {
            self.say(format!("{skipped} block(s) skipped (chunk not loaded)"));
        }
        let _ = applied;
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

    fn push(&mut self, r: Reply) {
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
