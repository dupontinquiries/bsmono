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
                if !is_debug && !self.models.state.is_ready() {
                    self.say("no model loaded -- /model to list, /model <name> to load, \
                              or /build debug <anything> to test without one");
                    return Pending::None;
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
