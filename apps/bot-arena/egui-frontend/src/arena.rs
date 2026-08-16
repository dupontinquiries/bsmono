//! arena — the real engine behind the dashboard.
//!
//! The GTK app runs `bot_arena::engine::run_engine` on a worker thread and
//! drains `EngineMsg` on the main loop; this does the identical thing with the
//! identical engine — the seam is the lib split, not a view-model copy. The
//! egui shell calls `EngineHost::drain` each frame (async-channel's
//! `try_recv` is synchronous, so no runtime is needed).
//!
//! Colors are the GTK CSS classes, hex for hex, because a bot that is blue in
//! one shell and green in the other reads as a different STATE, not a
//! different theme:
//!
//! ```text
//! .bot-idle #4b4b4b   .bot-prov #5a4b00   .bot-run #1c5fa8   .bot-pass #1f7a3d
//! .bot-fail #7a2f2f   .bot-cull #262626 (55% opacity)        .bot-spawn #8a7a1f
//! ```

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use bot_arena::engine::{self, BotStatus, Config, EngineMsg};
use egui::Color32;

/// One live cell in the arena grid, keyed by bot id.
#[derive(Debug, Clone, PartialEq)]
pub struct BotCell {
    pub id: usize,
    pub status: BotStatus,
    pub fitness: f64,
    pub z: f64,
    pub purpose: String,
}

/// One generation's replay snapshot (the engine's `Replay` message).
#[derive(Debug, Clone, PartialEq)]
pub struct ReplaySnap {
    pub gen: usize,
    pub best: f64,
    pub mean: f64,
    /// Top-k bots: (id, z, fitness).
    pub top: Vec<(usize, f64, f64)>,
}

/// The GTK CSS status color, hex for hex. `Culled` carries its 55% opacity in
/// the alpha channel so the grid dims the dead the way the GTK grid does.
pub fn status_color(s: BotStatus) -> Color32 {
    match s {
        BotStatus::Idle => Color32::from_rgb(0x4b, 0x4b, 0x4b),
        BotStatus::Provisioning => Color32::from_rgb(0x5a, 0x4b, 0x00),
        BotStatus::Running => Color32::from_rgb(0x1c, 0x5f, 0xa8),
        BotStatus::Passed => Color32::from_rgb(0x1f, 0x7a, 0x3d),
        BotStatus::Failed => Color32::from_rgb(0x7a, 0x2f, 0x2f),
        BotStatus::Culled => Color32::from_rgba_unmultiplied(0x26, 0x26, 0x26, 140),
        BotStatus::Spawned => Color32::from_rgb(0x8a, 0x7a, 0x1f),
    }
}

/// Live model the views render. Everything in here arrived as an `EngineMsg`.
#[derive(Default)]
pub struct ArenaModel {
    pub bots: Vec<BotCell>,
    pub logs: Vec<String>,
    pub activity: Vec<(usize, String)>,
    pub replays: Vec<ReplaySnap>,
    /// Latest generation line: (n, best, mean, alive, focus).
    pub generation: Option<(usize, f64, f64, usize, f64)>,
    /// Coordinator line: (focus, label, niche_reward).
    pub coordinator: Option<(f64, String, f64)>,
    pub done: bool,
}

/// Cap the log so an overnight run cannot grow a Vec without bound. 5000 lines
/// ≈ what anyone scrolls; the GTK TextView had the same practical ceiling.
const LOG_CAP: usize = 5000;
/// The activity feed only ever shows the tail.
const ACTIVITY_CAP: usize = 64;

impl ArenaModel {
    /// Apply one message. Split from the channel drain so tests can feed
    /// messages directly — the mapping is the behavior worth pinning.
    pub fn apply(&mut self, msg: EngineMsg) {
        match msg {
            EngineMsg::Log(s) => {
                self.logs.push(s);
                if self.logs.len() > LOG_CAP {
                    let cut = self.logs.len() - LOG_CAP;
                    self.logs.drain(..cut);
                }
            }
            EngineMsg::Bot { id, status, fitness, z, purpose } => {
                match self.bots.iter_mut().find(|b| b.id == id) {
                    Some(b) => {
                        b.status = status;
                        b.fitness = fitness;
                        b.z = z;
                        b.purpose = purpose;
                    }
                    None => {
                        self.bots.push(BotCell { id, status, fitness, z, purpose });
                        // Ids arrive in spawn order but a checkpoint resume can
                        // interleave; the grid sorts so cell positions are stable.
                        self.bots.sort_by_key(|b| b.id);
                    }
                }
            }
            EngineMsg::Activity { id, text } => {
                self.activity.push((id, text));
                if self.activity.len() > ACTIVITY_CAP {
                    let cut = self.activity.len() - ACTIVITY_CAP;
                    self.activity.drain(..cut);
                }
            }
            EngineMsg::Generation { n, best, mean, alive, focus } => {
                self.generation = Some((n, best, mean, alive, focus));
            }
            EngineMsg::Replay { gen, best, mean, top } => {
                self.replays.push(ReplaySnap { gen, best, mean, top });
            }
            EngineMsg::Coordinator { focus, label, niche_reward } => {
                self.coordinator = Some((focus, label, niche_reward));
            }
            EngineMsg::Done => self.done = true,
        }
    }

    /// Standings: live bots ranked by fitness, best first.
    pub fn ranked(&self) -> Vec<&BotCell> {
        let mut v: Vec<&BotCell> = self.bots.iter().collect();
        v.sort_by(|a, b| b.fitness.partial_cmp(&a.fitness).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
}

/// Owns the engine thread's channel, stop flag, and the shared latent-axis
/// knob (the same `Arc<AtomicU32>` the GTK slider writes).
pub struct EngineHost {
    rx: Option<async_channel::Receiver<EngineMsg>>,
    stop: Arc<AtomicBool>,
    pub axis: Arc<AtomicU32>,
}

impl Default for EngineHost {
    fn default() -> Self {
        EngineHost {
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
            // 0.5 in f32 bits — the GTK slider's centered default.
            axis: Arc::new(AtomicU32::new(0.5f32.to_bits())),
        }
    }
}

impl EngineHost {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Launch a run. The model resets — a new run's grid starting with the old
    /// run's corpses is how the GTK app behaves too (it rebuilds the FlowBox).
    pub fn start(&mut self, cfg: Config, model: &mut ArenaModel) {
        if self.running() {
            return;
        }
        *model = ArenaModel::default();
        self.stop.store(false, Ordering::Relaxed);
        let (tx, rx) = async_channel::unbounded();
        self.rx = Some(rx);
        let stop = self.stop.clone();
        let axis = self.axis.clone();
        std::thread::spawn(move || engine::run_engine(cfg, tx, stop, axis));
    }

    /// Ask the run to stop. The engine checks the flag at its own boundaries;
    /// `Done` still arrives through the channel.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Drain everything pending into the model. Returns true if anything
    /// changed so the shell can request a repaint (and keep animating while a
    /// run is live).
    pub fn drain(&mut self, model: &mut ArenaModel) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(msg) => {
                    let was_done = model.done;
                    model.apply(msg);
                    changed = true;
                    if model.done && !was_done {
                        self.rx = None;
                        return true;
                    }
                }
                Err(async_channel::TryRecvError::Empty) => break,
                Err(async_channel::TryRecvError::Closed) => {
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }
}

// ── setup form ───────────────────────────────────────────────────────────────

/// The setup page's editable state. Mirrors the GTK Setup page's controls and
/// maps onto `engine::Config` — the engine's own `Default` supplies everything
/// the page doesn't surface, exactly as the GTK page does.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupForm {
    pub population: usize,
    pub workers_per_container: usize,
    pub keep_fraction: f64,
    pub generations: usize,
    pub backend_index: u32,
    pub image: String,
    pub workspace: String,
    pub corpus: String,
    pub niches: usize,
    pub test_cmd: String,
    pub turbo: bool,
    pub seed: String,
}

impl Default for SetupForm {
    fn default() -> Self {
        let d = Config::default();
        SetupForm {
            population: d.population,
            workers_per_container: d.workers_per_container,
            keep_fraction: d.keep_fraction,
            generations: d.generations,
            backend_index: 0,
            image: d.image,
            workspace: String::new(),
            corpus: String::new(),
            niches: d.niches,
            test_cmd: d.test_cmd,
            turbo: d.turbo,
            seed: String::new(),
        }
    }
}

impl SetupForm {
    /// The `Config` this form describes. Blank text fields mean "engine
    /// default", the same contract the GTK rows have.
    pub fn to_config(&self) -> Config {
        let path = |s: &str| {
            let t = s.trim();
            if t.is_empty() { None } else { Some(std::path::PathBuf::from(t)) }
        };
        Config {
            population: self.population,
            workers_per_container: self.workers_per_container,
            keep_fraction: self.keep_fraction,
            generations: self.generations,
            backend: engine::BackendKind::from_index(self.backend_index),
            image: self.image.trim().to_string(),
            workspace: path(&self.workspace),
            workspaces: path(&self.workspace).into_iter().collect(),
            corpus: path(&self.corpus),
            niches: self.niches.max(1),
            test_cmd: if self.test_cmd.trim().is_empty() {
                "auto".into()
            } else {
                self.test_cmd.trim().to_string()
            },
            turbo: self.turbo,
            seed: self.seed.trim().parse::<u64>().ok(),
            ..Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_the_gtk_color() {
        // Hex-for-hex with main.rs's CSS. If a class changes there, this is
        // the test that says the shells disagree.
        assert_eq!(status_color(BotStatus::Idle), Color32::from_rgb(0x4b, 0x4b, 0x4b));
        assert_eq!(status_color(BotStatus::Provisioning), Color32::from_rgb(0x5a, 0x4b, 0x00));
        assert_eq!(status_color(BotStatus::Running), Color32::from_rgb(0x1c, 0x5f, 0xa8));
        assert_eq!(status_color(BotStatus::Passed), Color32::from_rgb(0x1f, 0x7a, 0x3d));
        assert_eq!(status_color(BotStatus::Failed), Color32::from_rgb(0x7a, 0x2f, 0x2f));
        assert_eq!(status_color(BotStatus::Spawned), Color32::from_rgb(0x8a, 0x7a, 0x1f));
        assert_eq!(
            status_color(BotStatus::Culled).a(),
            140,
            "culled dims — the GTK grid's 55% opacity"
        );
    }

    #[test]
    fn bot_messages_update_in_place_and_keep_the_grid_stable() {
        let mut m = ArenaModel::default();
        m.apply(EngineMsg::Bot { id: 2, status: BotStatus::Idle, fitness: 0.0, z: 0.5, purpose: "b".into() });
        m.apply(EngineMsg::Bot { id: 0, status: BotStatus::Idle, fitness: 0.0, z: 0.1, purpose: "a".into() });
        assert_eq!(m.bots.len(), 2);
        assert_eq!(m.bots[0].id, 0, "cells sort by id — positions don't jump");
        m.apply(EngineMsg::Bot { id: 2, status: BotStatus::Passed, fitness: 0.9, z: 0.5, purpose: "b".into() });
        assert_eq!(m.bots.len(), 2, "an update is not a new cell");
        assert_eq!(m.bots[1].status, BotStatus::Passed);
        assert_eq!(m.ranked()[0].id, 2, "standings rank by fitness");
    }

    #[test]
    fn logs_are_capped_so_an_overnight_run_cannot_eat_the_heap() {
        let mut m = ArenaModel::default();
        for i in 0..(LOG_CAP + 100) {
            m.apply(EngineMsg::Log(format!("line {i}")));
        }
        assert_eq!(m.logs.len(), LOG_CAP);
        assert!(m.logs[0].contains("line 100"), "oldest lines drop first");
    }

    #[test]
    fn a_real_sim_run_streams_to_done() {
        // The engine's own headless default: a small Sim population, turbo.
        // This is the whole point of the lib seam — the identical engine the
        // GTK app runs, driven from this shell, in a test.
        let mut host = EngineHost::default();
        let mut model = ArenaModel::default();
        let cfg = Config { population: 8, generations: 3, ..Config::default() };
        host.start(cfg, &mut model);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while host.running() {
            host.drain(&mut model);
            assert!(std::time::Instant::now() < deadline, "engine hung");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(model.done);
        assert_eq!(model.bots.len(), 8, "every bot reported in");
        assert!(model.generation.is_some());
        assert!(!model.replays.is_empty(), "replay snapshots buffered for scrubbing");
        assert!(model.logs.iter().any(|l| l.contains("run seed")), "the seed line arrived");
    }

    #[test]
    fn the_form_maps_blank_fields_to_engine_defaults() {
        let f = SetupForm::default();
        let c = f.to_config();
        assert_eq!(c.test_cmd, "auto");
        assert!(c.workspace.is_none());
        assert!(c.seed.is_none());
        let f = SetupForm { seed: "42".into(), workspace: " /repo ".into(), ..SetupForm::default() };
        let c = f.to_config();
        assert_eq!(c.seed, Some(42));
        assert_eq!(c.workspace, Some(std::path::PathBuf::from("/repo")));
        assert_eq!(c.workspaces, vec![std::path::PathBuf::from("/repo")]);
    }
}
