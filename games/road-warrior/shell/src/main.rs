//! Confirmed working: built, run, and screenshotted repeatedly under Xvfb
//! with lavapipe (software Vulkan) across a full session. Physics, terrain,
//! all three levels, boss combat, ramming, jump/flip, nitro, camera, save/
//! load, unlocks, and full campaign completion have all been visually
//! verified against real rendered frames, not just logic. What's genuinely
//! untested: real gyro input (needs actual hardware — see gyro.rs), and
//! whether any of it is fun.

mod audio;
mod flow;
mod gyro;
mod render;
mod save;

use eframe::egui_wgpu;
use road_warrior_core::{campaign, cues, encounter, BakedLighting, Boss, CarKind, Campaign, SaveState, Sun, Track, Vehicle};
use gyro::GyroTilt;
use render::{SceneCallback, SceneResources, SceneUniforms, TerrainUniforms};

/// A short-lived on-screen message for events the player should actually see
/// — "unlocked!", "new best time!", etc. These previously only went to
/// `eprintln!`, meaning a player running the real game (not from a terminal)
/// never saw them at all.
struct Toast {
    message: String,
    shown_at: std::time::Instant,
}

const TOAST_DURATION_SECS: f32 = 3.0;

/// One physics step. 60Hz: the vehicle integrator is a simple Euler step and
/// gets visibly springier on ramps at 30.
const FIXED_DT: f32 = 1.0 / 60.0;
/// Most steps to run in one frame before giving up on catching all the way
/// up. Four means a frame as slow as 66ms still simulates in real time.
const MAX_STEPS_PER_FRAME: u32 = 4;

/// Samples in the baked lighting field, over the whole track.
///
/// 256 across an 88-unit level is a sample every ~0.34 units, finer than
/// the 64 columns the viewport shows, so the shader's interpolation is
/// never the limiting factor. The bake is a few milliseconds at level load.
const LIGHT_SAMPLES: usize = 256;

struct RoadWarriorApp {
    vehicle: Vehicle,
    track: Track,
    boss: Option<Boss>,
    campaign: Campaign,
    save: SaveState,
    tilt: GyroTilt,
    throttle: f32,
    /// Rays traced once when the level loaded. The camera is fixed and the
    /// sun does not move, so per-column sun visibility and sky openness are
    /// properties of the terrain alone — tracing them every frame would
    /// recompute identical numbers sixty times a second.
    light: BakedLighting,
    /// Unspent real time, carried between frames so slow frames do not lose
    /// it. See the accumulator in `update`.
    accumulator: f32,
    start_time: std::time::Instant,
    last_frame: std::time::Instant,
    level_start_time: std::time::Instant,
    toasts: Vec<Toast>,
    /// Title / play / pause / campaign-complete (`flow`).
    flow: flow::Flow,
    /// The sound (`audio`); `None` on a machine with no output, and the game runs silent.
    audio: Option<audio::Audio>,
    /// Names the car's moments for the sound (`cues::Listener`).
    listener: cues::Listener,
    /// `M` toggles it; the master goes to zero, the synth keeps running.
    muted: bool,
}

/// Matches the index order the shader assumes (0=Behemoth, 1=SixWheeler,
/// 2=RedSedan, 3=Pillager) — same order as `CarKind::ALL`, kept in sync
/// manually since WGSL has no way to share an enum with Rust.
fn car_kind_index(kind: CarKind) -> f32 {
    CarKind::ALL.iter().position(|&k| k == kind).unwrap_or(2) as f32
}

impl RoadWarriorApp {
    fn new(cc: &eframe::CreationContext<'_>, tilt: GyroTilt) -> Self {
        // Register wgpu scene resources once, keyed by type in CallbackResources.
        if let Some(render_state) = &cc.wgpu_render_state {
            let resources = SceneResources::new(&render_state.device, render_state.target_format);
            render_state.renderer.write().callback_resources.insert(resources);
        }

        // The whole campaign is authored data (assets/campaign.json) — same
        // LevelData/Track/Boss path core's tests run, just sequenced.
        let campaign_json = include_str!("../assets/campaign.json");
        let mut campaign = Campaign::from_json(campaign_json).expect("assets/campaign.json should be valid");

        let save = save::load_or_create();
        campaign.seek(save.campaign_level_index);
        // A finished save (e.g. campaign completed, then levels were added
        // in an update) shouldn't strand the player past the end — restart
        // from the last valid level rather than showing nothing.
        if campaign.is_finished() {
            campaign.seek(0);
        }

        let level = campaign.current_level().expect("campaign should have at least one level");
        let (track, mut boss) = level.build();
        // The car is chosen on the title screen (`flow`), among the save's unlocked cars.
        // Until c21 a heuristic picked it ("Pillager if unlocked, else Behemoth, ..."), so an
        // unlock was a toast and never a choice. The title opens with the newest unlock
        // selected, which is what the heuristic preferred; this vehicle is replaced on Start.
        let flow = flow::Flow::new(save.unlocked_cars());
        let starting_car = flow.selected_car(save.unlocked_cars()).unwrap_or(CarKind::RedSedan);

        // Debug-only, opt-in via env var: shrinks the boss to a couple of ram
        // hits' worth of health. Grinding a real 1000-1800 HP boss down takes
        // a dozen-plus drive-by passes (ramming only deals damage on contact
        // at speed, gated by a cooldown) — impractical to exercise the
        // level-complete -> unlock -> save -> next-level chain without this.
        // cfg!(debug_assertions) means this is structurally impossible in a
        // release build, not just "undocumented" — the env var check itself
        // compiles away in --release, so there's no cheat code hiding in a
        // shipped binary regardless of what anyone sets in their environment.
        if cfg!(debug_assertions) && std::env::var("RW_DEBUG_WEAK_BOSS").is_ok() {
            if let Some(b) = &mut boss {
                b.health = 10.0;
                b.max_health = 10.0;
            }
        }

        let mut vehicle = Vehicle::new(starting_car);
        // Debug-only, opt-in via env var, compiled out of release builds:
        // force a specific car by index (0=Behemoth, 1=SixWheeler,
        // 2=RedSedan, 3=Pillager) for testing/QA without needing to grind
        // through unlocks first.
        if cfg!(debug_assertions) {
            if let Ok(idx) = std::env::var("RW_DEBUG_CAR") {
                if let Ok(idx) = idx.parse::<usize>() {
                    if let Some(&kind) = CarKind::ALL.get(idx) {
                        vehicle = Vehicle::new(kind);
                    }
                }
            }
        }
        // Debug-only, opt-in via env var: forces a near-instant death so the
        // respawn/retry-level flow is actually testable without waiting for
        // organic damage to accumulate. Compiled out in --release (see the
        // cfg!(debug_assertions) comment on RW_DEBUG_WEAK_BOSS above).
        if cfg!(debug_assertions) && std::env::var("RW_DEBUG_LOW_HP").is_ok() {
            vehicle.health = 1.0;
        }

        let audio = audio::Audio::start();
        if audio.is_none() {
            eprintln!("road-warrior: no audio output; running silent");
        }
        // Honour a saved mute before the first engine frame is mixed.
        if save.muted {
            if let Some(a) = &audio {
                a.send(audio::Msg::Master(0.0));
            }
        }
        let listener = cues::Listener::new(&vehicle);
        Self {
            audio,
            listener,
            muted: save.muted,
            vehicle,
            light: BakedLighting::bake(&track, Sun::default(), LIGHT_SAMPLES),
            track,
            boss,
            campaign,
            save,
            tilt,
            throttle: 0.0,
            accumulator: 0.0,
            start_time: std::time::Instant::now(),
            last_frame: std::time::Instant::now(),
            level_start_time: std::time::Instant::now(),
            toasts: Vec::new(),
            flow,
        }
    }

    /// The title's Start: drive `kind` from the saved level with full stats.
    fn start_run(&mut self, kind: CarKind) {
        self.vehicle = Vehicle::new(kind);
        if cfg!(debug_assertions) && std::env::var("RW_DEBUG_LOW_HP").is_ok() {
            self.vehicle.health = 1.0;
        }
        if self.campaign.is_finished() {
            self.campaign.seek(0);
        }
        self.load_current_level();
        self.listener.reset(&self.vehicle);
        self.level_start_time = std::time::Instant::now();
        self.accumulator = 0.0;
        self.toasts.clear();
    }

    fn sound(&self, m: audio::Msg) {
        if let Some(a) = &self.audio {
            a.send(m);
        }
    }

    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.sound(audio::Msg::Master(if self.muted { 0.0 } else { 1.0 }));
        // remembered across launches, like the campaign position
        self.save.muted = self.muted;
        save::save(&self.save);
        self.toasts.push(Toast { message: if self.muted { "Sound off (M)".into() } else { "Sound on".into() }, shown_at: std::time::Instant::now() });
    }

    /// Resets the current level (fresh boss, vehicle back to full stats at
    /// the start) after a crash — the standard "retry the level" convention
    /// for this genre, chosen since there was no direction on what death
    /// should do; the alternative (lose a life / restart the whole campaign)
    /// is a bigger design decision I didn't want to guess at silently.
    fn respawn_out_of_fuel(&mut self) {
        let kind = self.vehicle.kind;
        self.vehicle = Vehicle::new(kind);
        self.load_current_level();
        self.sound(audio::Msg::Sting(audio::Sting::Lost));
        self.listener.reset(&self.vehicle);
        self.level_start_time = std::time::Instant::now();
        self.toasts.push(Toast {
            message: "Out of fuel! Retrying level...".to_string(),
            shown_at: std::time::Instant::now(),
        });
    }

    fn respawn(&mut self) {
        let kind = self.vehicle.kind;
        self.vehicle = Vehicle::new(kind);
        self.load_current_level();
        self.sound(audio::Msg::Sting(audio::Sting::Lost));
        self.listener.reset(&self.vehicle);
        self.level_start_time = std::time::Instant::now();
        self.toasts.push(Toast { message: "Crashed! Retrying level...".to_string(), shown_at: std::time::Instant::now() });
    }

    /// Loads whatever level the campaign is currently pointing at, resetting
    /// vehicle position (but not stats/health/gas — carries over between levels).
    fn load_current_level(&mut self) {
        if let Some(level) = self.campaign.current_level() {
            let (track, mut boss) = level.build();
            if cfg!(debug_assertions) && std::env::var("RW_DEBUG_WEAK_BOSS").is_ok() {
                if let Some(b) = &mut boss {
                    b.health = 10.0;
                    b.max_health = 10.0;
                }
            }
            // Trace the level's lighting once, here, where the track
            // changes — not per frame.
            self.light = BakedLighting::bake(&track, Sun::default(), LIGHT_SAMPLES);
            self.track = track;
            self.boss = boss;
            self.vehicle.pos_x = 0.0;
        }
    }
}

/// How many fixed steps this frame gets, and what time is carried over.
///
/// Pulled out of `update` because it is the piece that was wrong: the old
/// code took `elapsed.min(FIXED_DT)` and dropped the remainder, so a device
/// that could not hold 60fps ran the whole simulation slow. `update` needs
/// an egui context and cannot be unit-tested; this can.
fn steps_for(accumulator: &mut f32, elapsed: f32) -> u32 {
    *accumulator = (*accumulator + elapsed.max(0.0)).min(FIXED_DT * MAX_STEPS_PER_FRAME as f32);
    let steps = (*accumulator / FIXED_DT) as u32;
    *accumulator -= steps as f32 * FIXED_DT;
    steps
}

#[cfg(test)]
mod timestep_tests {
    use super::*;

    #[test]
    fn a_normal_frame_takes_exactly_one_step() {
        let mut acc = 0.0;
        assert_eq!(steps_for(&mut acc, FIXED_DT), 1);
        assert!(acc < 1e-6, "and leaves nothing behind");
    }

    #[test]
    fn a_slow_frame_catches_up_instead_of_running_slow() {
        // THE BUG. A 100ms frame is three steps' worth of time. The old
        // clamp ran one and threw the other two away, which is why 175
        // seconds of wall clock integrated 13 seconds of physics.
        let mut acc = 0.0;
        assert_eq!(steps_for(&mut acc, 0.1), 4);
    }

    #[test]
    fn no_time_is_lost_across_frames_that_do_not_divide_evenly() {
        // Half-steps must accumulate, or a 90fps display loses a third of
        // every frame forever.
        let mut acc = 0.0;
        let frame = FIXED_DT * 0.5;
        let mut total = 0;
        for _ in 0..100 {
            total += steps_for(&mut acc, frame);
        }
        assert!((49..=51).contains(&total), "100 half-frames is ~50 steps, got {total}");
    }

    #[test]
    fn a_stall_is_capped_rather_than_simulated_in_full() {
        // The reason a cap exists at all: simulating a five-second stall as
        // 300 steps takes longer than five seconds, and the backlog then
        // grows every frame until the app is wedged. Past the cap we drop
        // time on purpose.
        let mut acc = 0.0;
        assert_eq!(steps_for(&mut acc, 5.0), MAX_STEPS_PER_FRAME);
        assert!(acc < FIXED_DT, "no backlog is carried out of a stall: {acc}");
    }

    #[test]
    fn a_backwards_clock_does_not_underflow() {
        let mut acc = 0.0;
        assert_eq!(steps_for(&mut acc, -1.0), 0);
        assert!(acc >= 0.0);
    }
}

impl eframe::App for RoadWarriorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // FIXED-STEP ACCUMULATOR, not a clamped frame delta.
        //
        // This used to be `dt = elapsed.min(1/30)` with `last_frame = now`,
        // which DISCARDS every millisecond past 33 — so on any device that
        // cannot hold 30fps the simulation silently runs in slow motion
        // while the clock keeps real time. Measured under software Vulkan:
        // 175 seconds of wall clock had integrated about 13 seconds of
        // physics, so a tank of fuel lasted three minutes and the car
        // crawled. The clamp's intent (don't take one giant step after a
        // stall) was right; throwing the time away was not.
        //
        // Now the leftover is carried in `self.accumulator` and spent as
        // whole `FIXED_DT` steps. Slow frames take several steps instead of
        // losing time.
        let now = std::time::Instant::now();
        let elapsed = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        // A cap on steps per frame, because the alternative to slow motion
        // is a death spiral: if simulating N steps takes longer than N*dt,
        // the backlog grows every frame and the app locks up. Past the cap
        // we DO drop time, deliberately and visibly, rather than freeze.
        let steps = steps_for(&mut self.accumulator, elapsed);
        let dt = FIXED_DT;

        // Keyboard fallback for desktop testing; gyro drives `tilt` on-device.
        // Left/Right does double duty: throttle while grounded (where tilt
        // has no effect on physics), flip control while airborne (where
        // throttle has no effect on physics) — Left/Right was previously
        // hardwired to throttle only, meaning flips were only controllable
        // via a real gyro, making the desktop build untestable for the
        // core jump/flip loop. `keyboard_tilt` and `throttle` are naturally
        // exclusive since a vehicle can't be both grounded and airborne.
        // The screens' keys first (`flow`): Esc pauses, Enter/Space confirm, Left/Right select
        // a car on the title or quit from the pause screen. In play, Left/Right/Space are the
        // car's and the flow ignores them.
        let unlocked: Vec<CarKind> = self.save.unlocked_cars().to_vec();
        let mut flow_keys: Vec<flow::Key> = Vec::new();
        let mut mute_pressed = false;
        ctx.input(|i| {
            mute_pressed = i.key_pressed(egui::Key::M);
            if i.key_pressed(egui::Key::Escape) {
                flow_keys.push(flow::Key::Back);
            }
            if i.key_pressed(egui::Key::Enter) || (i.key_pressed(egui::Key::Space) && !self.flow.simulates()) {
                flow_keys.push(flow::Key::Confirm);
            }
            if !self.flow.simulates() {
                if i.key_pressed(egui::Key::ArrowLeft) {
                    flow_keys.push(flow::Key::Left);
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    flow_keys.push(flow::Key::Right);
                }
            }
        });
        if mute_pressed {
            self.toggle_mute();
        }
        for k in flow_keys {
            match self.flow.on_key(k, &unlocked) {
                Some(flow::Action::Start(kind)) => self.start_run(kind),
                Some(flow::Action::Resume) => self.accumulator = 0.0,
                Some(flow::Action::QuitToTitle) => {
                    // the level is abandoned; the save already holds the campaign index
                    self.accumulator = 0.0;
                }
                None => {}
            }
        }

        let mut keyboard_tilt = 0.0f32;
        let mut nitro_active = false;
        if self.flow.simulates() {
            ctx.input(|i| {
                self.throttle = 0.0;
                if i.key_down(egui::Key::ArrowRight) {
                    self.throttle += 1.0;
                    keyboard_tilt += 1.0;
                }
                if i.key_down(egui::Key::ArrowLeft) {
                    self.throttle -= 1.0;
                    keyboard_tilt -= 1.0;
                }
                if i.key_pressed(egui::Key::Space) && !self.vehicle.airborne {
                    self.vehicle.launch(6.0);
                }
                nitro_active = i.key_down(egui::Key::N);
            });
        } else {
            self.throttle = 0.0;
        }

        let tilt = (self.tilt.get() + keyboard_tilt).clamp(-1.0, 1.0);
        // Off the play screen the world stands still: the accumulator is drained so no time
        // banks up behind a menu (`steps_for` capped it anyway) and no step runs.
        let steps = if self.flow.simulates() { steps } else { self.accumulator = 0.0; 0 };

        // Every step in the frame's budget. `steps` is 1 at 60fps and more
        // when a frame ran long; input is sampled once per frame and held
        // across the sub-steps, which is what a player expects from a held
        // key.
        let mut boss_defeated_this_frame = false;
        for _ in 0..steps {
            let outcome = if let Some(boss) = &mut self.boss {
                let outcome = encounter::advance(&mut self.vehicle, &self.track, boss, dt, self.throttle, tilt, nitro_active, -9.8);
                boss_defeated_this_frame |= outcome.boss_defeated_this_tick;
                Some(outcome)
            } else {
                road_warrior_core::sim::advance(&mut self.vehicle, &self.track, dt, self.throttle, tilt, nitro_active, -9.8);
                None
            };
            // what the step did, to hear (`cues`)
            for c in self.listener.step(&self.vehicle, nitro_active, outcome) {
                self.sound(audio::Msg::Cue(c));
            }
            // Stop sub-stepping the moment the car dies, so a death is not
            // simulated three more steps past the point it happened.
            if self.vehicle.health <= 0.0 {
                break;
            }
        }
        // the engine's note, every frame: idling on the menus, silent-throttle off the play screen
        self.sound(audio::Msg::Engine(if self.flow.simulates() {
            cues::engine(&self.vehicle, self.throttle)
        } else {
            cues::Engine { rpm: 0.0, load: 0.0 }
        }));
        if boss_defeated_this_frame {
            self.toasts.push(Toast { message: "Boss defeated!".to_string(), shown_at: std::time::Instant::now() });
        }

        // Checked before level-completion below: no point evaluating whether
        // the level is complete if the vehicle just died. Respawn resets
        // pos_x back to 0, so a death exactly at the finish line correctly
        // does NOT also count as completing it.
        if !self.flow.simulates() {
            // no deaths, no completions and no toasts from a world that did not move
        } else if self.vehicle.health <= 0.0 {
            self.respawn();
        } else if self.vehicle.is_stranded() {
            // An empty tank used to be a SOFTLOCK: the car coasts to a stop,
            // health is still positive so nothing respawns, the level cannot
            // complete, and there is no refuel anywhere in the game. The
            // player sits at a standstill until they quit.
            //
            // Retrying the level is the existing recovery path and it
            // already restores fuel, so this is the same failure as a crash
            // with an honest message.
            self.respawn_out_of_fuel();
        }

        // Guard against re-triggering every frame forever: once the campaign
        // is finished, `self.track`/`self.boss` still hold the last level's
        // (already-defeated) state, and the vehicle is still sitting at/past
        // the end — without this check, `is_level_complete` stays true
        // permanently, which was confirmed by actually letting the game run
        // past campaign completion: "campaign complete!" printed on every
        // single frame forever, and `save::save()` (a disk write) fired just
        // as often. Real bug, not just a log-spam cosmetic issue.
        if self.flow.simulates() && !self.campaign.is_finished() && campaign::is_level_complete(&self.vehicle, &self.track, self.boss.as_ref()) {
            let mut new_best_msg = None;
            let mut unlocked_msg = None;
            if let Some(level) = self.campaign.current_level() {
                let elapsed = self.level_start_time.elapsed().as_secs_f32();
                if self.save.record_time(&level.name, elapsed) {
                    new_best_msg = Some(format!("New best time: {elapsed:.1}s!"));
                }
                if let Some(unlocked) = self.save.apply_level_reward(level) {
                    unlocked_msg = Some(format!("Unlocked {unlocked:?}!"));
                }
            }
            // Pushed after the `level` borrow (from self.campaign) ends, since
            // self.toasts.push still needs a moment where self.campaign isn't
            // borrowed — direct field pushes here, not a self-taking method,
            // specifically to keep this borrow-checker-friendly.
            if let Some(msg) = new_best_msg {
                self.toasts.push(Toast { message: msg, shown_at: std::time::Instant::now() });
            }
            if let Some(msg) = unlocked_msg {
                self.toasts.push(Toast { message: msg, shown_at: std::time::Instant::now() });
            }

            self.sound(audio::Msg::Sting(audio::Sting::LevelComplete));
            if self.campaign.advance() {
                self.load_current_level();
                self.listener.reset(&self.vehicle);
                self.level_start_time = std::time::Instant::now();
            } else {
                // the campaign-complete screen (`flow`); it was a three-second toast
                self.flow.campaign_finished();
            }
            self.save.campaign_level_index = self.campaign.current_index();
            save::save(&self.save);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.max_rect();
            let boss_relative_x = self.boss.as_ref().map_or(0.0, |b| b.pos_x - self.vehicle.pos_x);
            let boss_present = self.boss.as_ref().map_or(0.0, |b| if b.phase() != road_warrior_core::BossPhase::Defeated { 1.0 } else { 0.0 });
            let boss_telegraphing = self.boss.as_ref().map_or(0.0, |b| if b.is_telegraphing() { 1.0 } else { 0.0 });

            let material = road_warrior_core::material::for_vehicle(&self.vehicle.stats);
            let uniforms = SceneUniforms {
                car_x: self.vehicle.pos_x * 0.05, // scale world units into shader's [-1,1]-ish space
                car_y: self.vehicle.pos_y,
                car_rotation: self.vehicle.rotation,
                time: self.start_time.elapsed().as_secs_f32(),
                resolution: [rect.width(), rect.height()],
                boss_relative_x,
                boss_present,
                boss_telegraphing,
                car_kind: car_kind_index(self.vehicle.kind),
                reflectivity: material.reflectivity,
                specular_power: material.specular_power,
            };

            // Sampled per frame from the live track, so the ground the
            // shader draws is the ground the physics is using.
            let terrain = TerrainUniforms::sample(&self.track, &self.light, self.vehicle.pos_x);

            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                SceneCallback { uniforms, terrain },
            ));

            egui::Area::new("hud".into())
                .fixed_pos(egui::pos2(12.0, 12.0))
                .show(ctx, |ui| {
                    if let Some(level) = self.campaign.current_level() {
                        ui.colored_label(
                            egui::Color32::WHITE,
                            format!("level {}/{}: {}", self.campaign.current_index() + 1, self.campaign.levels.len(), level.name),
                        );
                        let elapsed = self.level_start_time.elapsed().as_secs_f32();
                        match self.save.best_time(&level.name) {
                            Some(best) => ui.colored_label(egui::Color32::WHITE, format!("time  {elapsed:>5.1}s (best {best:.1}s)")),
                            None => ui.colored_label(egui::Color32::WHITE, format!("time  {elapsed:>5.1}s (no best yet)")),
                        };
                    }
                    ui.colored_label(egui::Color32::WHITE, format!("gas   {:>5.1}", self.vehicle.gas));
                    ui.colored_label(egui::Color32::WHITE, format!("nitro {:>5.1}", self.vehicle.nitro));
                    ui.colored_label(egui::Color32::WHITE, format!("hp    {:>5.1}", self.vehicle.health));
                    ui.colored_label(egui::Color32::WHITE, format!("tilt  {:>5.2}", tilt));
                    if let Some(boss) = &self.boss {
                        ui.colored_label(egui::Color32::WHITE, format!("boss  {:>5.1}/{:.0}", boss.health, boss.max_health));
                    }
                });

            // Expire old toasts first so the count used for layout below is accurate.
            self.toasts.retain(|t| t.shown_at.elapsed().as_secs_f32() < TOAST_DURATION_SECS);
            if !self.toasts.is_empty() {
                egui::Area::new("toasts".into())
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 24.0))
                    .show(ctx, |ui| {
                        for toast in &self.toasts {
                            // Fade out over the last second so it doesn't just vanish.
                            let age = toast.shown_at.elapsed().as_secs_f32();
                            let alpha = (1.0 - (age - (TOAST_DURATION_SECS - 1.0)).max(0.0)).clamp(0.0, 1.0);
                            let color = egui::Color32::from_rgba_unmultiplied(255, 220, 120, (alpha * 255.0) as u8);
                            ui.label(egui::RichText::new(&toast.message).color(color).size(20.0).strong());
                        }
                    });
            }
            // The screens over the world (`flow`): the title with its car select, the pause,
            // the campaign-complete. Drawn as an egui area over the scene, dimmed.
            if !self.flow.simulates() {
                ui.painter().rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(6, 8, 12, 150));
                let gold = egui::Color32::from_rgb(255, 220, 120);
                let dim = egui::Color32::from_rgb(190, 196, 205);
                let n_levels = self.campaign.levels.len().max(1);
                let level_no = (self.campaign.current_index() + 1).min(n_levels);
                let level_name = self.campaign.current_level().map(|l| l.name.clone()).unwrap_or_default();
                egui::Area::new("flow_screen".into())
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| match self.flow.screen {
                            flow::Screen::Title { .. } => {
                                ui.label(egui::RichText::new("ROAD WARRIOR").color(gold).size(44.0).strong());
                                ui.add_space(10.0);
                                ui.label(egui::RichText::new(format!("level {level_no}/{n_levels} — {level_name}")).color(dim).size(18.0));
                                if let Some(best) = self.save.best_time(&level_name) {
                                    ui.label(egui::RichText::new(format!("best {best:.1}s")).color(dim).size(14.0));
                                }
                                ui.add_space(18.0);
                                let car = self.flow.selected_car(&unlocked).unwrap_or(CarKind::RedSedan);
                                let st = car.stats();
                                ui.label(egui::RichText::new(format!("◀  {car:?}  ▶")).color(gold).size(26.0).strong());
                                ui.label(
                                    egui::RichText::new(format!(
                                        "top {:.0}  accel {:.0}  mass {:.0}  armor {:.0}  tank {:.0}",
                                        st.top_speed, st.accel, st.mass, st.armor, st.gas_capacity
                                    ))
                                    .color(dim)
                                    .size(14.0),
                                );
                                ui.label(egui::RichText::new(format!("{} of {} cars unlocked", unlocked.len(), CarKind::ALL.len())).color(dim).size(13.0));
                                ui.add_space(18.0);
                                ui.label(egui::RichText::new("← → choose a car   ·   Enter to drive").color(dim).size(16.0));
                            }
                            flow::Screen::Paused => {
                                ui.label(egui::RichText::new("PAUSED").color(gold).size(40.0).strong());
                                ui.add_space(12.0);
                                ui.label(egui::RichText::new("Esc or Enter to resume   ·   ← → quit to the title").color(dim).size(16.0));
                            }
                            flow::Screen::CampaignComplete => {
                                ui.label(egui::RichText::new("CAMPAIGN COMPLETE").color(gold).size(40.0).strong());
                                ui.add_space(10.0);
                                for l in &self.campaign.levels {
                                    let best = self.save.best_time(&l.name).map(|t| format!("{t:.1}s")).unwrap_or_else(|| "—".into());
                                    ui.label(egui::RichText::new(format!("{}  {best}", l.name)).color(dim).size(16.0));
                                }
                                ui.add_space(14.0);
                                ui.label(egui::RichText::new("Enter for the title — the campaign starts over from level 1").color(dim).size(16.0));
                            }
                            flow::Screen::Playing => {}
                        });
                    });
            }
        });

        ctx.request_repaint(); // game loop, not event-driven
    }
}

fn main() -> eframe::Result<()> {
    let tilt = GyroTilt::new();

    // Gyro polling runs on its own tokio runtime/thread so a D-Bus hiccup
    // never stalls the render loop.
    let tilt_for_gyro = tilt.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            if let Err(e) = gyro::run(tilt_for_gyro).await {
                eprintln!("gyro task exited: {e}");
            }
        });
    });

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Road Warrior",
        options,
        Box::new(move |cc| Box::new(RoadWarriorApp::new(cc, tilt))),
    )
}
