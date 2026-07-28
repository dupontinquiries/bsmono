//! game.rs — the simulation that owns the whole world and advances it one fixed step at a time. It
//! holds the level, the player, the live enemies, and the particle system, and it is where a trigger
//! pull becomes a raycast, damage, blood, sparks, a tracer and a little screen-shake. It also assembles
//! the per-frame list of `Mesh3` the renderer draws: the static level, every enemy body, all particles,
//! tracers, and the first-person weapon viewmodel held in front of the camera. The app layer only has to
//! feed it [`Input`] and paint the mesh it produces.

use crate::camera::FpsCam;
use crate::combat::{raycast, HitKind};
use crate::enemy::{populate, Difficulty, Enemy};
use crate::loadout::Loadout;
use crate::cosmetics::Cosmetics;
use crate::creatures::{Creature, GameMode, HuntRole};
use crate::level::{Level, MapConfig, MapTheme, Obstacle, Rng};
use crate::math::{clampf, Vec2, Vec3};
use crate::mesh::{billboard, disc, hash01, lerp_c, rgb, tube, uv_sphere, Light, Mesh3};
use crate::particles::Particles;
use crate::pickups::{self, Pickup};
use crate::player::{Input, Player};
use crate::render::Fog;
use crate::weapons::WeaponKind;
use egui::Color32;

/// Score needed (this life) to call in each scorestreak.
const UAV_COST: u32 = 400;
const SENTRY_COST: u32 = 700;
const STRIKE_COST: u32 = 1200;
/// How long a fog-bank event lasts (seconds), ramping in and out.
const EVENT_LEN: f32 = 14.0;

/// A deployed auto-turret: tracks the nearest visible enemy and fires until its ammo or timer runs out.
struct Sentry {
    pos: Vec3,
    yaw: f32,
    cooldown: f32,
    life: f32,
    ammo: u32,
}

/// A lingering gravity well left by an Assassin's gravity grenade — inside it, gravity is near-zero.
struct GravityZone {
    center: Vec3,
    radius: f32,
    life: f32,
}

/// An acid puddle left by a Tank's grenade — damage-over-time, doubled against heavies.
struct AcidPool {
    center: Vec3,
    radius: f32,
    life: f32,
    dps: f32,
}

/// A brief glowing line from a muzzle to an impact point; fades over a few frames.
struct Tracer {
    a: Vec3,
    b: Vec3,
    life: f32,
    max_life: f32,
    color: Color32,
    rad: f32,
}

/// A gameplay sound event. The app synthesises a one-shot for each (gunshot, pickup, etc.) when the `sound`
/// feature is on; the core just records intent so it stays device-free and testable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sfx {
    /// A weapon report; the f32 is the "bore" 0..1 (bigger = beefier).
    Shot(f32),
    /// A bullet impact; the f32 is surface hardness 0..1.
    Impact(f32),
    /// An explosion; the f32 is size 0..1.
    Explosion(f32),
    /// A collectible was picked up.
    Pickup,
    /// A reload sequence began.
    Reload,
}

/// A per-map ambient "live event" — a distinctive hazard or spectacle keyed to the map's biome that
/// periodically sweeps through, so each map feels alive in its own way rather than sharing one generic effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapEvent {
    /// No special event (the map relies on its own systems, e.g. Desert Town's quake+sandstorm).
    None,
    /// Volcanic: an ember storm — a surge of glowing embers and ash raining across the map with a heat-flash.
    EmberStorm,
    /// Ice: a blizzard whiteout — driving snow and a thick fog bank that cuts visibility, then clears.
    Blizzard,
    /// Toxic: a gas surge — the acid mist swells, a sickly green haze rolls up and the air itself bites.
    GasSurge,
    /// Station: a solar-wind flare — an aurora shimmer washes the deck and static discharge sparks off edges.
    SolarFlare,
    /// Carrier: a squall — a hard sea wind drives spray and rocks the deck, with a sweep of rain.
    Squall,
    /// Alien / Canopy / Mistfall: a spore bloom — drifting bioluminescent motes rise and pulse the fog.
    SporeBloom,
    /// Redwood / Meadow: a wind gust front — a rolling gust bends the world, kicks up leaves/pollen and hisses.
    WindGust,
}

impl MapEvent {
    /// Pick the signature event for a biome. Desert Town keeps its bespoke quake+sandstorm (None here).
    pub fn for_theme(theme: MapTheme) -> MapEvent {
        match theme {
            MapTheme::Volcanic => MapEvent::EmberStorm,
            MapTheme::Ice => MapEvent::Blizzard,
            MapTheme::Toxic => MapEvent::GasSurge,
            MapTheme::Station => MapEvent::SolarFlare,
            MapTheme::Carrier => MapEvent::Squall,
            MapTheme::Alien | MapTheme::Canopy | MapTheme::Mistfall => MapEvent::SporeBloom,
            MapTheme::Redwood | MapTheme::Meadow => MapEvent::WindGust,
            _ => MapEvent::None,
        }
    }
    /// A short HUD banner announcing the event when it strikes.
    pub fn banner(self) -> &'static str {
        match self {
            MapEvent::None => "",
            MapEvent::EmberStorm => "EMBER STORM",
            MapEvent::Blizzard => "BLIZZARD",
            MapEvent::GasSurge => "GAS SURGE",
            MapEvent::SolarFlare => "SOLAR FLARE",
            MapEvent::Squall => "SQUALL",
            MapEvent::SporeBloom => "SPORE BLOOM",
            MapEvent::WindGust => "WIND FRONT",
        }
    }
}

pub struct Game {
    pub level: Level,
    pub player: Player,
    pub enemies: Vec<Enemy>,
    pub particles: Particles,
    pub light: Light,
    pub fog: Fog,
    pub kills: u32,
    pub wave: u32,
    pub time: f32,
    pub shake: f32,
    pub pickups: Vec<Pickup>,
    /// Procedural-audio event queue: the app (with the `sound` feature) drains this each frame and plays
    /// a synthesised one-shot per event. Empty otherwise.
    pub sfx: Vec<Sfx>,
    /// Score (Black Ops scorestreak flavour): +100 per kill, shown on the HUD.
    pub score: u32,
    /// Consecutive-kill streak (resets on death).
    pub streak: u32,
    /// Hit feedback: countdown timer for the crosshair hitmarker, and whether that hit was a kill.
    pub hitmarker: f32,
    pub hitmarker_kill: bool,
    /// Recent kill-feed lines (text, time-remaining).
    pub feed: Vec<(String, f32)>,
    /// Scorestreak progress this life (resets on death) and whether each has been spent.
    pub streak_points: u32,
    pub uav_used: bool,
    pub strike_used: bool,
    /// While > 0, the UAV is online and enemies are marked on the HUD.
    pub uav_timer: f32,
    /// Deployed sentry guns and whether the streak has been spent this life.
    sentries: Vec<Sentry>,
    pub sentry_used: bool,
    /// Super-ability charge 0..1; fills over time and on kills. Spend it to trigger the class super.
    pub super_charge: f32,
    /// Active gravity wells + acid puddles (from class grenades), an armed slam, and the Tank cannon window.
    gravity_zones: Vec<GravityZone>,
    acid_pools: Vec<AcidPool>,
    slam_armed: bool,
    cannon_timer: f32,
    /// Directional damage indicators: world-space direction toward the shooter + life remaining.
    pub damage_dirs: Vec<(Vec3, f32)>,
    /// Ambient drifting motes (dust / embers / snow / spores): base position + animation phase.
    motes: Vec<(Vec3, f32)>,
    /// Trigger state last frame, for semi-auto edge detection.
    fire_was_down: bool,
    /// Muzzle-flash light timer: set on each shot, decays fast — drives the dynamic muzzle point light.
    muzzle_t: f32,
    /// Short-lived explosion point lights: (world pos, life remaining).
    blast_lights: Vec<(Vec3, f32)>,
    /// Live physics debris chunks from destroyed cover/buildings — box gibs that tumble under gravity, spin,
    /// bounce off the floor and settle. Cheap rigid-ish bodies for readable destruction ("building tumble").
    tumble: Vec<TumbleChunk>,
    /// When true, cheap contact-shadow blobs are drawn under enemies and dynamic entities (set by the app
    /// from the graphics preset's shadow quality).
    pub shadow_blobs: bool,
    /// Team colours (set by the app from settings): the hostile hue enemies are tinted toward, and the
    /// friendly hue used for allied markers / the player in third person. Defaults green (friendly) / orange
    /// (hostile) — matching the requested defaults.
    pub hostile_color: egui::Color32,
    pub friendly_color: egui::Color32,
    /// Sustained-fire counter: climbs while you hold fire, decays when you stop — drives the recoil pattern.
    recoil_shots: f32,
    /// Perk modifiers carried from the loadout: enemy detection range × this, ammo pickups × this.
    detect_mul: f32,
    ammo_mul: f32,
    /// Player cosmetics (used by the viewmodel/HUD); bot skill tier and its derived combat multipliers.
    pub cosmetics: Cosmetics,
    difficulty: Difficulty,
    aim_mul: f32,
    fire_mul: f32,
    /// Dynamic-event fog: the map's baseline fog distances, the current event intensity (0..1, a fog bank
    /// rolling in), a phase timer for the active event, and a countdown to the next one.
    fog_base: (f32, f32),
    fog_event: f32,
    event_phase: f32,
    event_next: f32,
    /// Which event is currently active, plus the blackout/surge intensities and the baseline ambient the
    /// blackout dims from. (Fog uses `fog_event`.)
    event_kind: EventKind,
    dark_event: f32,
    surge: f32,
    /// Possession-gas event intensity 0..1 (open biomes): plant gas drifts in and frenzies the wildlife.
    gas: f32,
    ambient_base: f32,
    /// Dust bookkeeping: a cooldown between foot puffs and the previous airborne state for landing bursts.
    foot_dust: f32,
    prev_air: bool,
    /// Runtime map mutation: cover barriers that rise during a fog bank and lower when it clears. `shutters`
    /// holds their footprints; `base_obstacles` is the obstacle count before they're inserted; `shutters_up`
    /// tracks whether they're currently solid.
    shutters: Vec<Obstacle>,
    base_obstacles: usize,
    shutters_up: bool,
    /// Enemy grenades in flight, and a throttle timer between throws.
    grenades: Vec<Grenade>,
    grenade_timer: f32,
    /// In-flight rocket-launcher projectiles: real ballistic-ish flight (light gravity), impact detonation
    /// against the first thing they touch (wall/obstacle/enemy) rather than a fuse timer.
    rockets: Vec<Rocket>,
    /// Player frag grenades remaining (lethal equipment, thrown along the aim line).
    frags: u32,
    /// Active objective mode (Search & Destroy / Capture the Flag) layered over the shooter, or None.
    pub objective: crate::objectives::ObjectiveState,
    /// Game mode + the creature the player is hunting as (Hunt mode only); melee strike cooldown.
    mode: GameMode,
    creature: Creature,
    strike_cd: f32,
    /// Hunt sub-role + (survivor mode) a countdown to escape and whether the player has escaped.
    hunt_role: HuntRole,
    hunt_timer: f32,
    hunt_escaped: bool,
    tracers: Vec<Tracer>,
    rng: Rng,
    seed: u64,
    /// When true the level is being authored in the map builder: combat, enemies, waves and scorestreaks
    /// are all suppressed so the world is a peaceful sandbox to place assets in.
    pub editing: bool,
    /// Current sandstorm intensity 0..1 (Desert Town); renderer/HUD can read this for dust + fog.
    pub storm: f32,
    /// Earthquake liveliness (Desert Town): a countdown to the next tremor, the current tremor's remaining
    /// duration, and its intensity. Tremors periodically shake the ground, rain dust from roof edges, and
    /// dislodge falling debris — keeping the ruined town feeling structurally alive and on the edge of another
    /// collapse. `quake_active` > 0 means a tremor is currently running.
    quake_cooldown: f32,
    quake_active: f32,
    quake_intensity: f32,
    /// Whether this map runs the earthquake system (set for the quake-struck desert town).
    quake_map: bool,
    /// Generalized per-map "live event" system: a distinctive ambient hazard/spectacle that periodically
    /// sweeps the map based on its biome (an ember storm on volcanic maps, a blizzard whiteout on ice, a gas
    /// surge on toxic, a solar-wind aurora flare in space, etc.), so every map feels alive in its own way.
    /// `event_kind` is what this map runs; the cooldown/active/intensity mirror the quake fields.
    map_event: MapEvent,
    map_event_cooldown: f32,
    map_event_active: f32,
    map_event_intensity: f32,
    /// Player submersion last frame — so a rising edge (plunging into water) can throw a splash crown.
    prev_submersion: f32,
    /// Persistent bullet-impact decals (scorch craters on walls/obstacles/destructibles). Capped and FIFO —
    /// oldest decal drops off once the cap is hit, same pattern real engines use for a decal budget.
    pub decals: Vec<crate::mesh::Mesh3>,
}

impl Game {
    pub fn new(seed: u64) -> Self {
        Self::new_with(seed, None, Loadout::default())
    }

    /// Build a game on `seed`, optionally forcing a biome `theme`, with the player's chosen `loadout`.
    pub fn new_with(seed: u64, theme: Option<MapTheme>, loadout: Loadout) -> Self {
        let mut cfg = MapConfig::varied(seed);
        if let Some(t) = theme {
            cfg.theme = t;
        }
        Self::from_config(cfg, seed, loadout, Cosmetics::default(), 12, Difficulty::Regular, None, HuntRole::Predator)
    }
    /// Build a game from a named preset (index into `level::PRESETS`); `None` falls back to a seed-derived
    /// biome at the default size.
    pub fn new_preset(seed: u64, preset: Option<usize>, loadout: Loadout) -> Self {
        Self::new_match(seed, preset, loadout, Cosmetics::default(), 12, Difficulty::Regular, None, HuntRole::Predator)
    }

    /// The full constructor used by the menu: a preset, the player's loadout + cosmetics, a bot count, a
    /// difficulty tier, and `hunt` — `Some(creature)` switches on The Hunt mode (player is that creature,
    /// enemies become unarmed prey).
    pub fn new_match(seed: u64, preset: Option<usize>, loadout: Loadout, cosmetics: Cosmetics, bots: usize, difficulty: Difficulty, hunt: Option<Creature>, role: HuntRole) -> Self {
        let cfg = match preset {
            Some(i) => crate::level::PRESETS[i.min(crate::level::PRESETS.len() - 1)].config(seed),
            None => MapConfig::varied(seed),
        };
        Self::from_config(cfg, seed, loadout, cosmetics, bots.clamp(1, 60), difficulty, hunt, role)
    }

    /// Build a match for a generated biome config.
    fn from_config(cfg: MapConfig, seed: u64, loadout: Loadout, cosmetics: Cosmetics, bots: usize, difficulty: Difficulty, hunt: Option<Creature>, role: HuntRole) -> Self {
        Self::from_level(Level::generate(cfg), seed, loadout, cosmetics, bots, difficulty, hunt, role)
    }

    /// Build a match with the **cinematic-tuning slider** applied: generate the map, then run the recursive
    /// enhancer at the given intensity before starting play, so one base map yields a family of livelier,
    /// more varied layouts. `tuning` 0 → the authored map untouched; 1 → maximum cinematic enhancement. This
    /// is the constructor the map-selection UI calls with the slider value.
    pub fn new_match_tuned(
        seed: u64,
        preset: Option<usize>,
        loadout: Loadout,
        cosmetics: Cosmetics,
        bots: usize,
        difficulty: Difficulty,
        hunt: Option<Creature>,
        role: HuntRole,
        tuning: crate::enhance::CinematicTuning,
    ) -> Self {
        let cfg = match preset {
            Some(i) => crate::level::PRESETS[i.min(crate::level::PRESETS.len() - 1)].config(seed),
            None => MapConfig::varied(seed),
        };
        let mut level = Level::generate(cfg);
        crate::enhance::enhance(&mut level, tuning, seed, 60);
        Self::from_level(level, seed, loadout, cosmetics, bots.clamp(1, 60), difficulty, hunt, role)
    }

    /// Open an authored custom map in the builder: peaceful (no bots), with the editor active.
    pub fn from_custom(map: &crate::builder::CustomMap, loadout: Loadout, cosmetics: Cosmetics) -> Self {
        let level = Level::from_custom(map);
        let mut g = Self::from_level(level, 1, loadout, cosmetics, 0, Difficulty::Regular, None, HuntRole::Predator);
        g.editing = true;
        g
    }

    /// Play an authored custom map for real: bots spawn, combat is live.
    pub fn play_custom(map: &crate::builder::CustomMap, seed: u64, loadout: Loadout, cosmetics: Cosmetics, bots: usize, difficulty: Difficulty) -> Self {
        let level = Level::from_custom(map);
        Self::from_level(level, seed, loadout, cosmetics, bots.clamp(1, 60), difficulty, None, HuntRole::Predator)
    }

    fn from_level(level: Level, seed: u64, loadout: Loadout, cosmetics: Cosmetics, bots: usize, difficulty: Difficulty, hunt: Option<Creature>, role: HuntRole) -> Self {
        let base_obstacles = level.obstacles.len();
        let level_has_sandstorm = level.sandstorm.is_some();
        let level_theme = level.cfg.theme;
        let shutters = build_shutters(&level);
        let mut player = Player::new(&level);
        player.apply_loadout(loadout);
        player.set_class(loadout.class);
        // Apply the armor frame: starting plate + a bulk-based move-speed multiplier.
        player.armor = cosmetics.armor.armor();
        player.move_mul *= cosmetics.armor.speed_mul();
        let mode = if hunt.is_some() { GameMode::Hunt } else { GameMode::Standard };
        let creature = hunt.unwrap_or(Creature::Revenant);
        let mut enemies = populate(&level, bots, seed);
        if mode == GameMode::Hunt {
            // Hunt is a deliberately different sandbox from the standard wave shooter: the predator has its own
            // creature movement kit and survivors are fragile and grounded, so loadout perks like the jetpack
            // (which unbalanced the mode) are stripped here. Movement in Hunt comes from the role, not perks.
            player.jetpack = false;
            match role {
                HuntRole::Predator => {
                    // The creature is fast and tanky; prey can't hurt it.
                    player.move_mul *= creature.speed_mul();
                    player.armor = 0.0;
                }
                HuntRole::Survivor => {
                    // The player is one of the hunted: a fragile, unarmed survivor. One enemy becomes the AI
                    // creature; the rest (the "three others") are fellow survivors.
                    enemies.truncate(4);
                    if let Some(h) = enemies.first_mut() {
                        h.hunter = true;
                        h.hunter_creature = Some(creature); // give it the chosen creature's silhouette
                    }
                    player.armor = 0.0;
                }
            }
        }
        scale_enemy_health(&mut enemies, difficulty.health_mul());
        // The AI creature shrugs off the survivors entirely — a deep health pool so a panicked unarmed player
        // can't stumble into killing it.
        if mode == GameMode::Hunt && role == HuntRole::Survivor {
            for e in enemies.iter_mut().filter(|e| e.hunter) {
                e.health = 100000.0;
            }
        }
        let pal = level.pal;
        // Sun colour + fill vary by biome: warm furnace light on lava, cold bright on ice, eerie on alien.
        let (sun_col, ambient) = match level.cfg.theme {
            MapTheme::Volcanic => (rgb(255, 158, 100), 0.72),
            MapTheme::Ice => (rgb(226, 240, 255), 0.95),
            MapTheme::Alien => (rgb(172, 198, 152), 0.82),
            MapTheme::Canopy => (rgb(236, 214, 150), 0.86),
            MapTheme::Mistfall => (rgb(176, 212, 220), 0.84),
            MapTheme::Toxic => (rgb(196, 214, 130), 0.80),
            MapTheme::Urban => (rgb(255, 248, 230), 0.88),
            MapTheme::Carrier => (rgb(212, 224, 232), 0.86),
            MapTheme::Redwood => (rgb(240, 236, 190), 0.90), // dappled forest light — warm and fairly bright
            _ => (rgb(255, 244, 214), 0.78),
        };
        // Time-of-day rolls per seed so the same biome can deploy at dawn, noon, dusk, or under overcast —
        // shifting sun colour + angle, sky/horizon tint, ambient fill, and fog density.
        let tod = (seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 31) % 4;
        let (sun_col, ambient, sun_dir, sky, fogcol, fog_mul) = if level.cfg.theme == MapTheme::Station {
            // Orbit: a hard white star, deep shadows, the black of space, long dark fog so stars stay visible.
            (rgb(255, 250, 244), 0.36, Vec3::new(-0.5, 0.55, 0.42), rgb(8, 9, 16), rgb(6, 7, 14), 0.5)
        } else { match tod {
            0 => (
                lerp_c(sun_col, rgb(255, 184, 150), 0.5),
                ambient * 0.82,
                Vec3::new(-0.78, 0.42, 0.34),
                lerp_c(pal.sky, rgb(214, 178, 190), 0.4),
                lerp_c(pal.fog, rgb(224, 176, 168), 0.4),
                1.12,
            ), // dawn
            2 => (
                lerp_c(sun_col, rgb(255, 150, 92), 0.55),
                ambient * 0.85,
                Vec3::new(0.72, 0.4, -0.32),
                lerp_c(pal.sky, rgb(236, 172, 130), 0.42),
                lerp_c(pal.fog, rgb(238, 158, 120), 0.45),
                1.2,
            ), // dusk
            3 => (
                lerp_c(sun_col, rgb(202, 208, 214), 0.5),
                ambient * 1.06,
                Vec3::new(-0.3, 0.94, 0.2),
                lerp_c(pal.sky, rgb(198, 203, 210), 0.5),
                lerp_c(pal.fog, rgb(196, 200, 206), 0.45),
                1.35,
            ), // overcast
            _ => (sun_col, ambient, Vec3::new(-0.4, 0.85, 0.3), pal.sky, pal.fog, 1.0), // midday
        } };
        let mut light = Light::sun(sun_dir, sun_col);
        light.sky = sky;
        light.ambient = ambient;
        // Fog hides the far clip and sells the era. Draw distance is generous but bounded so the far-cull
        // keeps very large maps affordable (otherwise fog.end would scale with the whole map diagonal).
        let reach = level.cfg.size as f32 * level.ts();
        let mut fog = Fog::new(fogcol, (reach * 0.34 / fog_mul).min(120.0), (reach * 1.18 / fog_mul).min(260.0));
        // Learned-site atmosphere: the fog takes the model's OWN light tone (the palette's
        // brightest entry pulled toward the skin fog) and sits a little closer — aerial
        // perspective that makes the scanned massing read at scale instead of floating in
        // clear air. Sites without a bound palette keep the skin fog untouched.
        if let Some(f) = &level.learned_facade {
            if let Some(&light) = f.palette.last() {
                let mix = |a: u8, b: u8| ((a as u16 * 11 + b as u16 * 5) / 16) as u8;
                fog.color = Color32::from_rgb(mix(fog.color.r(), light.r()),
                                              mix(fog.color.g(), light.g()),
                                              mix(fog.color.b(), light.b()));
                fog.start *= 0.78;
                fog.end *= 0.88;
            }
        }
        let fog = fog;

        let mut pickups = pickups::populate(&level, seed);
        // collectibles authored in a custom map (the floating item sits above its pad)
        for (i, &(pos, kind)) in level.custom_pickups.iter().enumerate() {
            pickups.push(Pickup::new(kind, pos + Vec3::new(0.0, 0.6, 0.0), i as f32 * 0.7));
        }

        // Ambient motes scattered through the play volume; they drift in `frame_meshes`.
        let mut mrng = Rng::new(seed ^ 0x3072_BEEF);
        let span = level.cfg.size as f32 * level.ts();
        let mut motes = Vec::with_capacity(150);
        for _ in 0..150 {
            let p = Vec3::new(
                mrng.f32() * span,
                level.floor_y + 0.3 + mrng.f32() * (level.ceil_y - level.floor_y - 0.4).max(0.5),
                mrng.f32() * span,
            );
            motes.push((p, mrng.f32() * 6.2832));
        }

        Self {
            level,
            player,
            enemies,
            particles: Particles::new(seed),
            light,
            fog,
            kills: 0,
            wave: 1,
            time: 0.0,
            shake: 0.0,
            pickups,
            sfx: Vec::new(),
            score: 0,
            streak: 0,
            hitmarker: 0.0,
            hitmarker_kill: false,
            feed: Vec::new(),
            streak_points: 0,
            uav_used: false,
            strike_used: false,
            uav_timer: 0.0,
            sentries: Vec::new(),
            sentry_used: false,
            super_charge: 0.0,
            gravity_zones: Vec::new(),
            acid_pools: Vec::new(),
            slam_armed: false,
            cannon_timer: 0.0,
            damage_dirs: Vec::new(),
            motes,
            fire_was_down: false,
            muzzle_t: 0.0,
            blast_lights: Vec::new(),
            tumble: Vec::new(),
            shadow_blobs: true,
            hostile_color: egui::Color32::from_rgb(230, 120, 40), // orange (default hostile)
            friendly_color: egui::Color32::from_rgb(70, 190, 90),  // green (default friendly)
            recoil_shots: 0.0,
            detect_mul: loadout.detect_mul() * difficulty.detect_mul(),
            ammo_mul: loadout.ammo_mul(),
            cosmetics,
            difficulty,
            aim_mul: difficulty.aim_mul(),
            fire_mul: difficulty.fire_mul(),
            fog_base: (fog.start, fog.end),
            fog_event: 0.0,
            event_phase: 0.0,
            event_next: 24.0,
            event_kind: EventKind::Fog,
            dark_event: 0.0,
            surge: 0.0,
            gas: 0.0,
            ambient_base: ambient,
            foot_dust: 0.0,
            prev_air: false,
            shutters,
            base_obstacles,
            shutters_up: false,
            grenades: Vec::new(),
            grenade_timer: 7.0,
            rockets: Vec::new(),
            frags: if mode == GameMode::Hunt { 0 } else { 3 },
            objective: crate::objectives::ObjectiveState::none(),
            mode,
            creature,
            strike_cd: 0.0,
            hunt_role: role,
            hunt_timer: 100.0,
            hunt_escaped: false,
            tracers: Vec::new(),
            rng: Rng::new(seed ^ 0x0A11),
            seed,
            editing: false,
            storm: 0.0,
            quake_cooldown: 8.0 + level_has_sandstorm as u8 as f32 * 4.0,
            quake_active: 0.0,
            quake_intensity: 0.0,
            quake_map: level_has_sandstorm,
            // pick the biome's signature live event (Desert Town keeps its bespoke quake → None here)
            map_event: if level_has_sandstorm { MapEvent::None } else { MapEvent::for_theme(level_theme) },
            map_event_cooldown: 12.0,
            map_event_active: 0.0,
            map_event_intensity: 0.0,
            prev_submersion: 0.0,
            decals: Vec::new(),
        }
    }

    /// Advance the whole world by `dt` seconds.
    /// Current water surface of flood channel `i` (Flood Zone map) — for diagnostics and tests.
    pub fn flood_surface(&self, i: usize) -> Option<f32> {
        self.level.flood.get(i).map(|c| c.surface())
    }

    /// Configure this match as **Search & Destroy**: place two bomb sites at well-separated rooms and switch
    /// the objective layer on. The player attacks (plants); enemies defend (defuse).
    pub fn set_search_destroy(&mut self) {
        use crate::objectives::ObjectiveState;
        // pick two far-apart rooms for the sites (fall back to spawn offsets on tiny maps)
        let (a, b) = self.two_far_points();
        self.objective = ObjectiveState::search_destroy(a, b);
    }

    /// Configure this match as **Capture the Flag**: a blue base near the player spawn and a red base far
    /// across the map, first to 3 captures.
    pub fn set_capture_the_flag(&mut self) {
        use crate::objectives::ObjectiveState;
        let blue = self.level.spawn(0);
        let (_, red) = self.two_far_points();
        self.objective = ObjectiveState::capture_the_flag(blue, red, 3);
    }

    /// Two far-apart walkable points on the map — used to place bomb sites / flag bases. Uses the room list
    /// when available (picks the two most distant room centres), else offsets from spawn.
    fn two_far_points(&self) -> (Vec3, Vec3) {
        let rooms = &self.level.rooms;
        if rooms.len() >= 2 {
            // brute-force the most distant pair (room counts are small)
            let mut best = (0usize, 1usize, 0.0f32);
            for i in 0..rooms.len() {
                for j in (i + 1)..rooms.len() {
                    let ci = rooms[i].center;
                    let cj = rooms[j].center;
                    let d = (ci.x - cj.x).powi(2) + (ci.z - cj.z).powi(2);
                    if d > best.2 {
                        best = (i, j, d);
                    }
                }
            }
            let a = rooms[best.0].center;
            let b = rooms[best.1].center;
            (
                Vec3::new(a.x, self.level.floor_h(a.x, a.z), a.z),
                Vec3::new(b.x, self.level.floor_h(b.x, b.z), b.z),
            )
        } else {
            let s = self.level.spawn(0);
            let far = self.level.spawn(1.min(self.level.rooms.len().saturating_sub(1)));
            (s, far)
        }
    }

    /// Collect the active dynamic point lights this frame — muzzle flashes, live explosions/rockets, the
    /// planted-bomb device, and CTF flag glows — capped to the strongest handful so shading stays cheap. The
    /// app passes these to the renderer, where each casts real coloured light on nearby geometry.
    pub fn dynamic_lights(&self) -> Vec<crate::mesh::PointLight> {
        use crate::mesh::{rgb, PointLight};
        let mut lights: Vec<PointLight> = Vec::new();
        // muzzle flash: a brief warm light at the player's gun when firing
        if self.player.alive && self.muzzle_t > 0.01 {
            let e = self.player.eye();
            let f = self.player.cam.forward_flat();
            let pos = e + f * 0.8 + Vec3::new(0.0, -0.2, 0.0);
            lights.push(PointLight::new(pos, rgb(255, 220, 150), 1.4 * self.muzzle_t, 9.0));
        }
        // jetpack ion glow: a cyan-blue light under the player while thrusting, pulsing with the plume
        if self.player.jet_active {
            let flick = 0.85 + 0.15 * (self.time * 40.0).sin();
            let pos = self.player.pos + Vec3::new(0.0, 0.4, 0.0);
            lights.push(PointLight::new(pos, rgb(90, 170, 255), 1.5 * flick, 7.0));
        }
        // live rockets glow
        for r in &self.rockets {
            lights.push(PointLight::new(r.pos, rgb(255, 150, 60), 1.2, 8.0));
        }
        // planted bomb device pulses red
        if let Some(sidx) = self.objective.planted_site {
            if let Some(site) = self.objective.sites.get(sidx) {
                let pulse = 0.6 + 0.4 * (self.time * 6.0).sin();
                lights.push(PointLight::new(Vec3::new(site.pos.x, site.pos.y + 0.3, site.pos.z), rgb(255, 60, 40), 1.0 * pulse, 7.0));
            }
        }
        // CTF flags emit a soft team-coloured glow
        if self.objective.mode == crate::objectives::Objective::CaptureTheFlag {
            for (i, flag) in self.objective.flags.iter().enumerate() {
                let col = if i == 0 { rgb(90, 140, 255) } else { rgb(255, 90, 80) };
                lights.push(PointLight::new(Vec3::new(flag.pos.x, flag.pos.y + 1.4, flag.pos.z), col, 0.7, 6.0));
            }
        }
        // explosion flashes tracked as short-lived lights
        for (pos, life) in &self.blast_lights {
            let k = (*life / 0.4).clamp(0.0, 1.0);
            lights.push(PointLight::new(*pos, rgb(255, 180, 90), 1.8 * k, 12.0));
        }
        // burning cars/wreckage (Pyre Garage): each fire source casts a flickering orange light. Use a
        // per-source phase so they don't all flicker together, and only light the nearest handful.
        if !self.level.fire_sources.is_empty() {
            let pe = self.player.eye();
            let mut srcs: Vec<&Vec3> = self.level.fire_sources.iter().collect();
            srcs.sort_by(|a, b| {
                let da = (a.x - pe.x).powi(2) + (a.z - pe.z).powi(2);
                let db = (b.x - pe.x).powi(2) + (b.z - pe.z).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
            for (i, s) in srcs.iter().take(6).enumerate() {
                let flick = 0.7 + 0.3 * (self.time * (7.0 + i as f32) + i as f32 * 2.1).sin() + 0.1 * (self.time * 19.0).sin();
                lights.push(PointLight::new(Vec3::new(s.x, s.y + 0.4, s.z), rgb(255, 130, 45), (1.3 * flick).max(0.2), 10.0));
            }
        }
        // keep only the strongest ~10 to bound per-vertex cost (fire maps have many lights)
        if lights.len() > 10 {
            lights.sort_by(|a, b| b.intensity.partial_cmp(&a.intensity).unwrap_or(std::cmp::Ordering::Equal));
            lights.truncate(10);
        }
        lights
    }

    pub fn step(&mut self, input: &Input, dt: f32) {
        self.time += dt;
        self.muzzle_t = (self.muzzle_t - dt * 12.0).max(0.0);
        for b in &mut self.blast_lights {
            b.1 -= dt;
        }
        self.blast_lights.retain(|b| b.1 > 0.0);
        self.update_events(dt);
        self.shake = (self.shake - dt * 4.0).max(0.0);
        self.hitmarker = (self.hitmarker - dt).max(0.0);
        self.uav_timer = (self.uav_timer - dt).max(0.0);
        for f in &mut self.feed {
            f.1 -= dt;
        }
        self.feed.retain(|f| f.1 > 0.0);
        for d in &mut self.damage_dirs {
            d.1 -= dt;
        }
        self.damage_dirs.retain(|d| d.1 > 0.0);
        let was_alive = self.player.alive;

        // 1) Player movement & look.
        // Gravity wells (Assassin grenade) drop the player's effective gravity to near-zero while inside.
        let mut gscale = 1.0f32;
        {
            let pe = self.player.eye();
            for z in &self.gravity_zones {
                if ((pe.x - z.center.x).powi(2) + (pe.z - z.center.z).powi(2)).sqrt() < z.radius {
                    gscale = gscale.min(0.18);
                }
            }
        }
        // A flying predator (the Storm Kraken) hovers: strongly reduced gravity so it drifts through the air,
        // holding altitude and gliding rather than being pinned to the ground.
        if self.mode == GameMode::Hunt && self.hunt_role == HuntRole::Predator && self.creature.flying() {
            gscale = gscale.min(0.28);
        }
        self.player.gravity_scale = gscale;
        self.player.update(&self.level, input, dt);
        // Water splash: a rising edge in submersion (plunging in) throws a crown of droplets scaled by how
        // fast the transition happened; sustained fast wading kicks up a few beads at the feet.
        {
            let sub = self.player.submersion;
            let prev = self.prev_submersion;
            let surf = self.level.water_surface_at(self.player.pos.x, self.player.pos.z);
            if let Some(sy) = surf {
                let entering = prev < 0.2 && sub >= 0.2;
                let hspeed = (self.player.vel.x * self.player.vel.x + self.player.vel.z * self.player.vel.z).sqrt();
                if entering {
                    // impact strength from the vertical speed at the moment of entry
                    let strength = (self.player.vel.y.abs() * 0.6 + 2.0).clamp(2.0, 12.0);
                    let at = Vec3::new(self.player.pos.x, sy, self.player.pos.z);
                    self.particles.splash(at, strength, self.level.water.first().map(|w| w.col).unwrap_or(crate::mesh::rgb(90, 150, 180)));
                } else if sub > 0.2 && sub < 1.2 && hspeed > 3.5 {
                    // wading fast through shallow water — occasional foot splashes
                    if (self.time * 12.0).fract() < dt * 12.0 {
                        let at = Vec3::new(self.player.pos.x, sy, self.player.pos.z);
                        self.particles.splash(at, (hspeed * 0.4).clamp(1.0, 4.0), self.level.water.first().map(|w| w.col).unwrap_or(crate::mesh::rgb(90, 150, 180)));
                    }
                }
            }
            self.prev_submersion = sub;
        }
        self.update_flood(dt);
        self.update_sandstorm(dt);
        self.update_quake(dt);
        self.update_map_event(dt);
        self.update_fires(dt);
        self.detonate_barrels();
        // Man cannons: stepping onto a launch pad flings you upward (works in live play and sandbox walk).
        if self.player.on_ground && !self.level.cannons.is_empty() {
            for &(pos, strength) in &self.level.cannons {
                let dx = self.player.pos.x - pos.x;
                let dz = self.player.pos.z - pos.z;
                if dx * dx + dz * dz < 1.0 && (self.player.pos.y - pos.y).abs() < 1.8 {
                    self.player.vel.y = strength;
                    self.player.on_ground = false;
                    self.particles.smoke_puff(pos + Vec3::new(0.0, 0.25, 0.0), 6);
                    break;
                }
            }
        }
        // Air currents: stepping into a ground-level jetstream sweeps you along its flow (ride to travel).
        if !self.level.air_currents.is_empty() && self.player.alive {
            let pf = self.player.pos.xz();
            let py = self.player.pos.y;
            let mut push = crate::math::Vec2::new(0.0, 0.0);
            for ac in &self.level.air_currents {
                if (py - ac.y).abs() < 3.2 {
                    if let Some(dir) = ac.flow_at(pf) {
                        push.x += dir.x * ac.strength * dt;
                        push.y += dir.y * ac.strength * dt;
                    }
                }
            }
            if push.x != 0.0 || push.y != 0.0 {
                self.player.ride(&self.level, push.x, push.y);
                if (self.time * 12.0).fract() < dt * 12.0 {
                    self.particles.smoke_puff(self.player.pos + Vec3::new(0.0, 0.6, 0.0), 2);
                }
            }
        }
        // Kicked-up dust: puffs under fast feet, a burst on touchdown.
        self.foot_dust -= dt;
        let (ppos, pspd, pair) = {
            let pl = &self.player;
            ((pl.pos), (pl.vel.x * pl.vel.x + pl.vel.z * pl.vel.z).sqrt(), !pl.on_ground)
        };
        if !pair && pspd > 6.5 && self.foot_dust <= 0.0 {
            self.particles.foot_puff(ppos + Vec3::new(0.0, 0.12, 0.0), 2);
            self.foot_dust = 0.13;
        }
        if self.prev_air && !pair {
            self.particles.foot_puff(ppos + Vec3::new(0.0, 0.08, 0.0), 5);
        }
        // Jetpack ion exhaust: while the thruster fires, blast a downward ion plume from twin nozzles behind
        // the player's hips. This is the visible "ion blast" — bright cyan-white streaking down, cooling to blue.
        if self.player.jet_active {
            let yaw = self.player.cam.yaw;
            let back = Vec3::new(-yaw.sin(), 0.0, yaw.cos()); // behind the player
            let right = Vec3::new(yaw.cos(), 0.0, yaw.sin());
            let hip = self.player.pos + Vec3::new(0.0, 0.7, 0.0) + back * 0.15;
            self.particles.ion_thrust(hip + right * 0.22, back);
            self.particles.ion_thrust(hip - right * 0.22, back);
        }
        self.prev_air = pair;
        // In the map builder there are no enemies, waves, or weapons — just fly around and place assets.
        if self.editing {
            return;
        }
        self.update_grenades(dt);
        self.update_rockets(dt);
        self.update_tumble(dt);
        // Dying breaks your streak and clears scorestreak progress.
        if was_alive && !self.player.alive {
            self.streak = 0;
            self.streak_points = 0;
            self.uav_used = false;
            self.strike_used = false;
            self.uav_timer = 0.0;
            self.sentry_used = false;
            self.sentries.clear();
        }

        // 1b) Deploy earned scorestreaks (standard mode only — the creature has no radio).
        if self.player.alive && self.mode == GameMode::Standard {
            if input.deploy_uav && self.uav_ready() {
                self.uav_timer = 14.0;
                self.uav_used = true;
                self.feed.insert(0, ("UAV ONLINE".to_string(), 3.5));
            }
            if input.deploy_strike && self.strike_ready() {
                self.missile_strike();
            }
            if input.deploy_sentry && self.sentry_ready() {
                let fwd = self.player.cam.forward_flat();
                let p = self.player.pos + fwd * 2.5;
                let pos = Vec3::new(p.x, self.level.floor_h(p.x, p.z), p.z);
                self.sentries.push(Sentry { pos, yaw: self.player.cam.yaw, cooldown: 0.6, life: 24.0, ammo: 90 });
                self.sentry_used = true;
                self.feed.insert(0, ("SENTRY GUN ONLINE".to_string(), 3.0));
            }
            if input.throw_frag {
                self.throw_frag();
            }
            if input.activate_super {
                self.activate_super();
            }
        }
        self.update_sentries(dt);
        self.update_supers(dt);

        if self.strike_cd > 0.0 {
            self.strike_cd -= dt;
        }
        if self.mode == GameMode::Hunt && self.hunt_role == HuntRole::Predator {
            // 2-Hunt) Melee claw strike on the fire button — no ammo, no recoil, short reach.
            let want = input.fire && self.player.alive && !self.player.sprinting;
            if want && !self.fire_was_down && self.strike_cd <= 0.0 {
                self.melee_strike();
            }
            self.fire_was_down = input.fire;
        } else if self.mode == GameMode::Hunt {
            // Survivor: unarmed. No attack — just run and hide.
            self.fire_was_down = input.fire;
        } else {
            // 2) Shooting — needs a fresh press for semi-auto, can't fire while sprinting or reloading.
            let want_fire = input.fire && self.player.alive && !self.player.sprinting;
            let def = self.player.weapon().kind.def();
            let edge = want_fire && (def.auto || !self.fire_was_down);
            if edge && self.player.weapon().ready() {
                self.fire();
            } else {
                // Not firing this frame — let the recoil pattern settle back toward neutral.
                self.recoil_shots = (self.recoil_shots - dt * 9.0).max(0.0);
            }
            self.fire_was_down = input.fire;
        }

        // 3) Enemies think, move, and shoot back.
        let player_eye = self.player.eye();
        let player_feet = self.player.pos;
        let survivor_hunt = self.mode == GameMode::Hunt && self.hunt_role == HuntRole::Survivor;
        let mut incoming: Vec<(Vec3, Vec3, f32, bool)> = Vec::new();
        // Squad coordination: any engaged enemy broadcasts the player's last-known position; idle teammates
        // within earshot break to Search toward it, so a single contact pulls a whole pack onto you.
        if survivor_hunt {
            self.update_survivor_enemies(dt);
        } else {
        let sources: Vec<(Vec3, Vec2)> = self.enemies.iter().filter_map(|e| e.alert_source().map(|ls| (e.center(), ls))).collect();
        if !sources.is_empty() {
            for e in &mut self.enemies {
                if !e.is_idle() {
                    continue;
                }
                let ec = e.center();
                for (sc, ls) in &sources {
                    let dx = ec.x - sc.x;
                    let dz = ec.z - sc.z;
                    if dx * dx + dz * dz < 22.0 * 22.0 {
                        e.alert(*ls);
                        break;
                    }
                }
            }
        }
        for e in &mut self.enemies {
            // Possession gas frenzies the wildlife: sharper senses and a faster trigger while it hangs.
            let detect = self.detect_mul * (1.0 + 0.6 * self.gas);
            let fire = self.fire_mul * (1.0 - 0.35 * self.gas);
            if let Some(atk) = e.update(&self.level, player_eye, player_feet, detect, self.aim_mul, fire, self.mode == GameMode::Hunt, &mut self.rng, dt) {
                incoming.push((atk.from, atk.to, atk.damage, atk.hit_player));
            }
        }
        }
        // Apply enemy fire: tracer + impact, and damage the player if the shot connected.
        for (from, to, dmg, hit) in incoming {
            self.tracers.push(Tracer { a: from, b: to, life: 0.05, max_life: 0.05, color: rgb(255, 170, 90), rad: 0.02 });
            if hit {
                self.player.take_damage(dmg);
                self.shake = (self.shake + 0.25).min(1.2);
                // Remember which way the shot came from for the on-screen damage arrows.
                let dir = (from - self.player.eye()).with_y(0.0);
                if dir.len_sq() > 1e-3 {
                    self.damage_dirs.push((dir.normalized(), 1.1));
                }
            } else {
                self.particles.impact_sparks(to, (from - to).normalized());
            }
        }

        // 4) Particles & tracers age out.
        self.particles.tick(self.level.floor_y, dt);
        for t in &mut self.tracers {
            t.life -= dt;
        }
        self.tracers.retain(|t| t.life > 0.0);

        // 4b) Pickups bob/spin and get grabbed when you walk over them (if you can use them).
        for p in &mut self.pickups {
            p.update(dt);
        }
        let mut grabbed: Vec<Vec3> = Vec::new();
        for p in &mut self.pickups {
            if p.try_collect(&mut self.player) {
                grabbed.push(p.pos);
                self.sfx.push(Sfx::Pickup);
                // Scavenger perk: top off every weapon's reserve with a bonus on each pickup.
                if self.ammo_mul > 1.0 {
                    let bonus = (24.0 * (self.ammo_mul - 1.0)) as u32;
                    for w in &mut self.player.weapons {
                        w.add_reserve(bonus);
                    }
                }
            }
        }
        for pos in grabbed {
            self.particles.impact_sparks(pos + Vec3::new(0.0, 0.6, 0.0), Vec3::UP);
        }
        self.pickups.retain(|p| p.alive);

        // 5) Reap the dead and count kills; advance the wave when the room is clear.
        let before = self.enemies.len();
        self.enemies.retain(|e| !e.should_remove());
        // (kills are tallied at the moment of death below, not on removal)
        let _ = before;

        // ── Objective modes: drive the S&D / CTF state machine ────────────────────────────────────────────
        // Feed it the player's feet, whether the player can act, whether the "use" key (reload, repurposed
        // as the plant/defuse/pick-up action here) is held, and the live enemy positions. When a round
        // resolves it posts to the kill-feed; the app reads `objective` for the HUD banner.
        if self.objective.mode != crate::objectives::Objective::None && !self.objective.is_over() {
            let enemy_pts: Vec<Vec3> = self.enemies.iter().filter(|e| e.alive()).map(|e| e.pos).collect();
            let holding = input.reload; // the action key while standing on a site / near a flag
            let before_scores = self.objective.scores;
            if let Some(outcome) = self.objective.update(self.player.pos, self.player.alive, holding, &enemy_pts, dt) {
                use crate::objectives::RoundOutcome;
                let line = match outcome {
                    RoundOutcome::AttackersWin => "OBJECTIVE COMPLETE — VICTORY",
                    RoundOutcome::DefendersWin => "OBJECTIVE FAILED — DEFEAT",
                    RoundOutcome::Draw => "ROUND DRAWN",
                };
                self.feed.insert(0, (line.to_string(), 10.0));
            } else if self.objective.scores != before_scores {
                // a capture happened this frame
                self.feed.insert(0, (format!("CAPTURE  {}–{}", self.objective.scores[0], self.objective.scores[1]), 6.0));
            }
        }

        if self.mode == GameMode::Hunt {
            // The Hunt has no waves. Survivor mode runs an escape countdown instead.
            if self.hunt_role == HuntRole::Survivor && !self.hunt_escaped && self.player.alive {
                self.hunt_timer -= dt;
                if self.hunt_timer <= 0.0 {
                    self.hunt_timer = 0.0;
                    self.hunt_escaped = true;
                    self.feed.insert(0, ("DAWN BREAKS — YOU SURVIVED".to_string(), 8.0));
                }
            }
        } else if self.enemies.iter().all(|e| !e.alive()) && self.enemies.iter().all(|e| e.should_remove() || !e.alive()) {
            // All current enemies are down (dead or finishing their death). Spawn the next wave once
            // the corpses have cleared.
            if self.enemies.is_empty() {
                self.next_wave();
            }
        }
    }

    /// Dynamic map events. On a timer a fog bank rolls in: the fog closes to a fraction of its range over a
    /// few seconds, holds, then recedes — an honest approximation of volumetric fog within a vector renderer
    /// (the painter can't do screen-space god-rays). The motes thicken into the bank while it's active.
    fn update_events(&mut self, dt: f32) {
        // Authored maps drive their own weather (e.g. the Desert Town sandstorm) — skip the random ambient
        // event roulette (fog banks, blackouts, gas) so they don't fight the authored front.
        if self.level.sandstorm.is_some() {
            return;
        }
        if self.event_phase > 0.0 {
            self.event_phase -= dt;
            let p = (self.event_phase / EVENT_LEN).clamp(0.0, 1.0);
            let inten = (1.0 - (p - 0.5).abs() * 2.0).clamp(0.0, 1.0); // tent envelope
            self.fog_event = 0.0;
            self.dark_event = 0.0;
            self.surge = 0.0;
            self.gas = 0.0;
            match self.event_kind {
                EventKind::Fog => self.fog_event = inten,
                EventKind::Blackout => self.dark_event = inten,
                EventKind::Surge => self.surge = inten,
                EventKind::PossessionGas => self.gas = inten,
            }
            // While the gas hangs, frenzy the possessed wildlife (more alert, faster) and let it drift.
            if self.gas > 0.0 {
                if (self.time * 6.0).fract() < dt * 6.0 {
                    let span = self.level.w as f32 * self.level.ts();
                    let gx = self.rng.f32() * span;
                    let gz = self.rng.f32() * span;
                    self.particles.smoke_puff(Vec3::new(gx, self.level.floor_h(gx, gz) + 0.7, gz), 4);
                }
            }
            if self.event_phase <= 0.0 {
                self.fog_event = 0.0;
                self.dark_event = 0.0;
                self.surge = 0.0;
                self.gas = 0.0;
                self.event_next = 22.0 + self.rng.f32() * 18.0;
                let msg = match self.event_kind {
                    EventKind::Fog => "FOG CLEARING",
                    EventKind::Blackout => "POWER RESTORED",
                    EventKind::Surge => "SURGE SUBSIDING",
                    EventKind::PossessionGas => "GAS DISSIPATING",
                };
                self.feed.insert(0, (msg.to_string(), 3.0));
            }
        } else {
            self.event_next -= dt;
            if self.event_next <= 0.0 {
                let roll = self.rng.f32();
                // Surge only when the map has a lava/water body to raise; possession gas only on the open,
                // plant-covered biomes where the "plantae" actually grow.
                let open = self.level.cfg.theme.open_sky() && self.mode == GameMode::Standard;
                let kind = if self.level.has_hazard && roll < 0.30 {
                    EventKind::Surge
                } else if open && roll < 0.58 {
                    EventKind::PossessionGas
                } else if roll < 0.78 {
                    EventKind::Blackout
                } else {
                    EventKind::Fog
                };
                self.trigger_event(kind);
            }
        }

        // Fog bank: pull the fog planes in — softened so even a full fog event hazes rather than blinds.
        let (bs, be) = self.fog_base;
        self.fog.start = bs * (1.0 - 0.42 * self.fog_event);
        self.fog.end = be * (1.0 - 0.34 * self.fog_event);

        // Power failure: dim the ambient with an unstable flicker.
        let flicker = 1.0 + 0.12 * (self.time * 23.0).sin() * self.dark_event;
        self.light.ambient = (self.ambient_base * (1.0 - 0.6 * self.dark_event) * flicker).max(0.05);

        // Surge: the lava/water hazard climbs over the low ground. Standing in it burns (lava) or drags you
        // (water), so the catwalks and mezzanines become the safe high ground.
        if self.level.has_hazard {
            let hy = self.level.hazard_y + self.surge * SURGE_RISE;
            let feet = self.player.pos.y;
            if self.player.alive && feet < hy - 0.15 {
                let depth = (hy - feet).min(2.0);
                let rate = if self.level.hazard_dps > 0.0 { self.level.hazard_dps } else { 9.0 };
                self.player.take_damage(rate * depth.min(1.0) * dt);
                self.shake = (self.shake + 1.4 * dt).min(0.6);
                let inward = (Vec3::new(self.level.w as f32, 0.0, self.level.h as f32) * (self.level.ts() * 0.5) - self.player.pos).with_y(0.0);
                if inward.len_sq() > 1e-3 {
                    self.damage_dirs.push((inward.normalized(), 0.4));
                }
            }
        }

        // Runtime map mutation: cover barriers rise during a fog bank and drop as it clears.
        let want_up = self.fog_event > 0.5;
        if want_up && !self.shutters_up {
            self.level.obstacles.extend(self.shutters.iter().copied());
            self.shutters_up = true;
            self.feed.insert(0, ("BARRIERS RISING".to_string(), 3.0));
        } else if !want_up && self.shutters_up {
            self.level.obstacles.truncate(self.base_obstacles);
            self.shutters_up = false;
        }
    }

    fn trigger_event(&mut self, kind: EventKind) {
        if self.event_phase <= 0.0 {
            self.event_phase = EVENT_LEN;
            self.event_kind = kind;
            let msg = match kind {
                EventKind::Fog => "FOG ROLLING IN",
                EventKind::Blackout => "POWER FAILURE",
                EventKind::Surge => {
                    if self.level.hazard_lava {
                        "LAVA SURGE — GET HIGH"
                    } else {
                        "FLOODING — GET HIGH"
                    }
                }
                EventKind::PossessionGas => "POSSESSION GAS — WILDLIFE TURNING",
            };
            self.feed.insert(0, (msg.to_string(), 3.5));
            if kind == EventKind::PossessionGas {
                let n = 3 + (self.rng.f32() * 3.0) as u32;
                self.spawn_possessed(n);
            }
        }
    }

    /// The gas drives the local wildlife feral: spawn a handful of fast, frenzied creatures (Rushers and
    /// Screechers) at open spots away from the player — the "possessed wildlife" of the hollow-forest world.
    fn spawn_possessed(&mut self, count: u32) {
        use crate::enemy::{Enemy, EnemyKind, EnemyState};
        let mut nid = self.enemies.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let rooms = self.level.rooms.len();
        if rooms <= 1 {
            return;
        }
        let pfeet = self.player.pos;
        let mut placed = 0u32;
        let mut tries = 0;
        while placed < count && tries < count * 12 {
            tries += 1;
            let ri = 1 + (self.rng.next_u64() as usize % (rooms - 1));
            let r = self.level.rooms[ri].rect;
            let rx = r.x0 + self.rng.i32_range(1, (r.w() - 1).max(1));
            let rz = r.z0 + self.rng.i32_range(1, (r.h() - 1).max(1));
            let p = self.level.tile_center(rx, rz);
            if !self.level.has_floor_at(p.xz()) {
                continue;
            }
            if ((p.x - pfeet.x).powi(2) + (p.z - pfeet.z).powi(2)).sqrt() < 14.0 {
                continue; // not right on top of the player
            }
            let kind = if self.rng.f32() < 0.5 { EnemyKind::Rusher } else { EnemyKind::Screecher };
            let mut e = Enemy::new(nid, kind, p);
            e.state = EnemyState::Chase; // possessed = already hunting
            self.particles.smoke_puff(p + Vec3::new(0.0, 0.8, 0.0), 8);
            self.enemies.push(e);
            nid += 1;
            placed += 1;
        }
    }

    fn trigger_fog_event(&mut self) {
        self.trigger_event(EventKind::Fog);
    }

    /// Current world wind as a horizontal vector (direction × strength), used to bow foliage in the renderer
    /// and drift the ambient life. Derived from the seed so each map has its own prevailing breeze.
    pub fn wind(&self) -> crate::math::Vec2 {
        let wph = (self.seed % 628) as f32 * 0.01;
        let wstr = 0.6 + ((self.seed >> 7) % 100) as f32 * 0.012;
        crate::math::Vec2::new(wph.cos() * wstr * 0.16, wph.sin() * wstr * 0.16)
    }

    /// Current fog-event intensity (0..1) — the app thickens the drifting motes by this.
    pub fn fog_event(&self) -> f32 {
        self.fog_event
    }

    /// Whether this map uses the dynamic LOD field (large maps).
    pub fn has_lod(&self) -> bool {
        self.level.lod_field.is_some()
    }

    /// Drain queued sound events (the app plays them when the `sound` feature is on). Capped so a long pause
    /// in playback can't let the queue grow without bound.
    pub fn take_sfx(&mut self) -> Vec<Sfx> {
        if self.sfx.len() > 32 {
            let n = self.sfx.len();
            self.sfx.drain(0..n - 32);
        }
        std::mem::take(&mut self.sfx)
    }

    /// Per-frame LOD step: re-select Full/Coarse/Impostor per chunk from the camera, rebuilding at most
    /// `budget` chunks this frame so the cost stays bounded.
    pub fn lod_update(&mut self, cam: &FpsCam, budget: usize) {
        if let Some(field) = &mut self.level.lod_field {
            field.update(cam.pos, budget);
        }
    }

    /// Advance the timed flood channels (Flood Zone map): raise each channel's water surface along its cycle,
    /// and wade-slow the player when they're standing in water that's risen past their feet. The surge is what
    /// turns the dry low routes into temporary no-go zones and back again.
    fn update_flood(&mut self, dt: f32) {
        if self.level.flood.is_empty() {
            return;
        }
        let ts = self.level.ts();
        let n = self.level.flood.len();
        let mut wade = false;
        for i in 0..n {
            self.level.flood[i].phase += dt;
            let (idx, surf, flooded, in_here) = {
                let ch = &self.level.flood[i];
                let p = self.player.pos;
                (ch.water_idx, ch.surface(), ch.flooded(), ch.contains_world(p.x, p.z, ts))
            };
            if idx < self.level.water.len() {
                self.level.water[idx].y = surf;
            }
            if flooded && in_here && self.player.pos.y < surf + 0.2 {
                wade = true;
            }
        }
        if wade {
            // wading drags you — bleed horizontal speed so the flood genuinely impedes movement
            self.player.vel.x *= (1.0 - 4.5 * dt).max(0.0);
            self.player.vel.z *= (1.0 - 4.5 * dt).max(0.0);
        }
    }

    /// Advance the sandstorm front (Desert Town): ramp its intensity along the cycle, shove the player along
    /// the wind while it blows, kick up dust, and announce it rolling in. `self.storm` (0..1) is read by the
    /// Emit the continuous fire plume at each of the map's fire sources (Pyre Garage): flame tongues, rising
    /// embers, and black smoke, throttled so the particle budget holds across many fires at once. The dynamic
    /// orange light for each fire is added in `dynamic_lights` (flickering), so this owns only the particles.
    fn update_fires(&mut self, dt: f32) {
        if self.level.fire_sources.is_empty() {
            return;
        }
        // throttle: emit on a cadence, and only for the nearest handful of fires to the camera so distant
        // ones don't flood the buffer. Copy the source list to avoid borrowing self across the emit calls.
        let pe = self.player.eye();
        let mut srcs: Vec<Vec3> = self.level.fire_sources.clone();
        srcs.sort_by(|a, b| {
            let da = (a.x - pe.x).powi(2) + (a.z - pe.z).powi(2);
            let db = (b.x - pe.x).powi(2) + (b.z - pe.z).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        let phase = (self.time * 40.0).fract();
        for (i, src) in srcs.iter().take(6).enumerate() {
            // stagger emission per-source so they don't pulse in lockstep
            if (phase + i as f32 * 0.17).fract() < dt * 40.0 {
                self.particles.flame(*src, 2);
                self.particles.embers(*src + Vec3::new(0.0, 0.2, 0.0), 1);
            }
            // smoke on a slower cadence
            if ((self.time * 8.0 + i as f32 * 0.4).fract()) < dt * 8.0 {
                self.particles.smoke_puff(*src + Vec3::new(0.0, 0.8, 0.0), 1);
            }
        }
    }

    /// renderer/HUD for the dust veil and visibility drop.
    fn update_sandstorm(&mut self, dt: f32) {
        let Some(mut st) = self.level.sandstorm else {
            self.storm = 0.0;
            return;
        };
        let prev = self.storm;
        st.phase += dt;
        let inten = st.intensity();
        self.level.sandstorm = Some(st);
        self.storm = inten;
        // Drive the fog bank from the storm: pull the fog planes in and tint toward dust so visibility drops
        // as the front sweeps through, and restore the baseline as it clears. update_events is skipped on
        // these maps, so this is the sole owner of the fog planes here. Kept modest — a real sandstorm is a
        // bright, hazy wall of dust, not a blackout; the fog colour is light and we lift the ambient a touch
        // so the whole scene reads as glare-through-dust rather than going dark.
        {
            let fe = inten;
            let (bs, be) = self.fog_base;
            // Tuned between the old blinding wall (0.4/0.34) and last pass's near-nothing (0.18/0.15): the storm
            // pulls the fog planes in enough that the distance clearly closes down and reads as real weather
            // (~a third off your sightline at the peak), but the *tint* stays light and the *particle* dust is
            // sparse, so it's glare-through-haze you can still fight in — not a tan blackout at arm's reach.
            self.fog.start = bs * (1.0 - 0.42 * fe);
            self.fog.end = be * (1.0 - 0.34 * fe);
            let dust = rgb(224, 202, 158);
            self.fog.color = lerp_c(self.level.pal.fog, dust, inten * 0.4);
            self.fog_event = fe * 0.45; // fewer volumetric motes so the screen doesn't fill with dust quads
        }
        self.light.ambient = self.ambient_base * (1.0 + 0.18 * inten);
        // Constant low-level sand drift so the desert floor is always subtly *alive* — a restless skitter of
        // grains along the prevailing wind even when no storm is blowing, ramping up as the front sweeps in.
        // Kept sparse and low to the ground so it pricks peripheral vision without becoming a screen of quads.
        {
            let base_rate = 0.15 + inten * 0.45; // grains-per-frame probability budget (was 0.30 + 0.9·inten)
            let wind = Vec3::new(st.wind.x, 0.0, st.wind.y);
            let wlen = wind.len().max(1e-3);
            let wdir = wind * (1.0 / wlen);
            let perp = Vec3::new(-wdir.z, 0.0, wdir.x);
            let bursts = (base_rate * 3.0) as usize + 1;
            // precompute spawn points (needs self.rng + self.level) before touching self.particles
            let mut spawns: Vec<Vec3> = Vec::new();
            for _ in 0..bursts {
                if self.rng.f32() > base_rate {
                    continue;
                }
                let ahead = (self.rng.f32() - 0.3) * 20.0;
                let side = (self.rng.f32() - 0.5) * 24.0;
                let gp = self.player.pos + wdir * ahead + perp * side;
                let surf = self.level.surface_at(gp.xz()).unwrap_or(self.level.floor_y);
                spawns.push(Vec3::new(gp.x, surf, gp.z));
            }
            let per = 1 + (inten * 2.0) as usize;
            for gp in spawns {
                self.particles.sand_drift(gp, wdir, per);
            }
        }
        if inten > 0.04 && self.player.alive {
            // wind push, strongest at the storm's peak
            self.player.vel.x += st.wind.x * inten * 2.6 * dt;
            self.player.vel.z += st.wind.z * inten * 2.6 * dt;
            // Sparse dust wisps in the middle distance (not stacked on the lens) — a light gust rate so they
            // never accumulate into a wall of quads blotting the screen.
            if self.rng.f32() < inten * 0.12 {
                let wind = Vec3::new(st.wind.x, 0.0, st.wind.y).normalized();
                let ahead = wind * (10.0 + self.rng.f32() * 14.0);
                let jitter = self.rng.f32() - 0.5;
                let side = Vec3::new(-wind.z, 0.0, wind.x) * jitter * 16.0;
                let updraft = self.rng.f32();
                let p = self.player.pos + ahead + side + Vec3::new(0.0, 1.2 + updraft * 3.0, 0.0);
                self.particles.dust_puff(p, Vec3::new(st.wind.x, 0.0, st.wind.y), 1);
            }
        }
        if prev < 0.25 && inten >= 0.25 {
            self.feed.insert(0, ("SANDSTORM INCOMING".to_string(), 3.0));
            self.feed.truncate(5);
        }
    }

    /// Earthquake liveliness (the quake-struck desert town). On a cooldown a tremor strikes: the ground
    /// shakes, dust sheets rain from roof edges and cracks, and a few chunks of masonry break loose and fall.
    /// Between tremors the town is uneasily still — a faint creak of settling now and then. This is the
    /// "twitch-inducing environment shift" that keeps players on edge: motion pricks the eye periodically,
    /// against a tense quiet baseline, rather than a constant churn that the eye tunes out.
    fn update_quake(&mut self, dt: f32) {
        if !self.quake_map {
            return;
        }
        if self.quake_active > 0.0 {
            // a tremor is running
            self.quake_active -= dt;
            let i = self.quake_intensity;
            self.shake = (self.shake + i * 2.4 * dt).min(1.4);
            // rain dust + drop debris from random roof edges around the player
            let mut spawns: Vec<(Vec3, bool)> = Vec::new();
            let bursts = 1 + (i * 4.0) as usize;
            for _ in 0..bursts {
                if self.rng.f32() > 0.5 * i + 0.1 {
                    continue;
                }
                let ang = self.rng.f32() * std::f32::consts::TAU;
                let dist = 4.0 + self.rng.f32() * 16.0;
                let px = self.player.pos.x + ang.cos() * dist;
                let pz = self.player.pos.z + ang.sin() * dist;
                // dust falls from up high (roof height ~ 6-9m); occasionally a solid chunk breaks off
                let high = 6.0 + self.rng.f32() * 3.0;
                let heavy = self.rng.f32() < 0.3;
                spawns.push((Vec3::new(px, high, pz), heavy));
            }
            for (p, heavy) in spawns {
                // falling dust sheet
                self.particles.dust_puff(p, Vec3::new(0.0, -1.0, 0.0), 2);
                if heavy {
                    // a chunk of masonry breaks loose and tumbles down
                    self.spawn_tumble(p, Vec3::new(0.4, 0.4, 0.4), rgb(150, 130, 104), 3, 2.0);
                }
            }
        } else {
            // between tremors: count down, with a faint settling creak (a rare tiny dust trickle)
            self.quake_cooldown -= dt;
            if self.rng.f32() < 0.004 {
                // occasional uneasy micro-trickle to keep the space from feeling dead
                let ang = self.rng.f32() * std::f32::consts::TAU;
                let p = self.player.pos + Vec3::new(ang.cos() * 8.0, 6.5, ang.sin() * 8.0);
                self.particles.dust_puff(p, Vec3::new(0.0, -0.8, 0.0), 1);
            }
            if self.quake_cooldown <= 0.0 {
                // strike a tremor
                self.quake_intensity = 0.5 + self.rng.f32() * 0.5;
                self.quake_active = 1.2 + self.rng.f32() * 1.6;
                self.quake_cooldown = 14.0 + self.rng.f32() * 16.0; // next one in 14-30s
                self.shake = (self.shake + self.quake_intensity).min(1.6);
                self.feed.insert(0, ("AFTERSHOCK".to_string(), 2.5));
                self.feed.truncate(5);
            }
        }
    }

    /// Generalized per-map live event. Mirrors the quake cadence (a tense quiet baseline broken by a periodic
    /// sweep) but the *kind* of event is chosen from the biome, so a volcanic map gets an ember storm, an ice
    /// map a blizzard whiteout, space a solar flare, and so on. Drives `fog_event` (for the events that thicken
    /// the air), emits themed particles around the player, and pushes a HUD banner when it strikes.
    fn update_map_event(&mut self, dt: f32) {
        if self.map_event == MapEvent::None {
            return;
        }
        if self.map_event_active > 0.0 {
            self.map_event_active -= dt;
            let i = self.map_event_intensity;
            let p = self.player.pos;
            // events that thicken the air ramp fog_event up while active
            let fogs = matches!(self.map_event, MapEvent::Blizzard | MapEvent::GasSurge | MapEvent::Squall | MapEvent::SporeBloom);
            if fogs {
                // Cap the fog these events add much lower (was 0.85) so a blizzard/gas surge hazes visibility
                // without whiting the screen out — you can still see and fight through the event.
                self.fog_event = (self.fog_event + dt * 0.6 * i).min((i * 0.5).min(0.45));
            }
            // per-event particle flavour, emitted in a ring around the player so it reads wherever you are
            let bursts = 2 + (i * 5.0) as usize;
            for _ in 0..bursts {
                if self.rng.f32() > 0.4 * i + 0.15 {
                    continue;
                }
                let ang = self.rng.f32() * std::f32::consts::TAU;
                let dist = 3.0 + self.rng.f32() * 16.0;
                let px = p.x + ang.cos() * dist;
                let pz = p.z + ang.sin() * dist;
                match self.map_event {
                    MapEvent::EmberStorm => {
                        // glowing embers rising + ash drifting down from above
                        self.particles.embers(Vec3::new(px, 1.0 + self.rng.f32() * 2.0, pz), 2);
                        if self.rng.f32() < 0.4 {
                            self.particles.dust_puff(Vec3::new(px, 7.0, pz), Vec3::new(0.0, -1.0, 0.0), 1);
                        }
                    }
                    MapEvent::Blizzard => {
                        // driving snow: dust particles blown nearly horizontal on a stiff wind
                        let wind = Vec3::new((self.time * 0.3).cos() * 4.0, -0.6, (self.time * 0.3).sin() * 4.0);
                        self.particles.dust_puff(Vec3::new(px, 2.0 + self.rng.f32() * 3.0, pz), wind, 2);
                    }
                    MapEvent::GasSurge => {
                        // sickly green acid mist welling up from the ground
                        self.particles.smoke_puff(Vec3::new(px, 0.4, pz), 2);
                    }
                    MapEvent::SolarFlare => {
                        // static discharge sparks flicking off nearby edges
                        if self.rng.f32() < 0.5 {
                            self.particles.ion_thrust(Vec3::new(px, 1.0 + self.rng.f32() * 2.0, pz), Vec3::new(0.0, 1.0, 0.0));
                        }
                    }
                    MapEvent::Squall => {
                        // sea spray + rain driven on the wind
                        let wind = Vec3::new((self.time * 0.5).cos() * 5.0, -3.0, (self.time * 0.5).sin() * 5.0);
                        self.particles.splash(Vec3::new(px, 0.2, pz), 1.0, rgb(150, 175, 190));
                        self.particles.dust_puff(Vec3::new(px, 3.0, pz), wind, 1);
                    }
                    MapEvent::SporeBloom => {
                        // drifting bioluminescent motes rising slowly
                        self.particles.embers(Vec3::new(px, 0.6 + self.rng.f32() * 2.5, pz), 1);
                    }
                    MapEvent::WindGust => {
                        // a rolling gust kicking leaves/pollen along the ground
                        let wind = Vec3::new((self.time * 0.6).cos() * 6.0, 0.4, (self.time * 0.6).sin() * 6.0);
                        self.particles.dust_puff(Vec3::new(px, 0.8 + self.rng.f32() * 1.5, pz), wind, 2);
                    }
                    MapEvent::None => {}
                }
            }
            // the squall also rocks the view a little (deck heave)
            if self.map_event == MapEvent::Squall {
                self.shake = (self.shake + i * 0.5 * dt).min(0.6);
            }
        } else {
            self.map_event_cooldown -= dt;
            if self.map_event_cooldown <= 0.0 {
                self.map_event_intensity = 0.55 + self.rng.f32() * 0.45;
                self.map_event_active = 5.0 + self.rng.f32() * 5.0; // events last a while, then clear
                self.map_event_cooldown = 22.0 + self.rng.f32() * 20.0; // next in 22-42s
                let banner = self.map_event.banner();
                if !banner.is_empty() {
                    self.feed.insert(0, (banner.to_string(), 3.0));
                    self.feed.truncate(5);
                }
            }
        }
    }

    /// Fire the blast for any explosive barrel that has been destroyed but not yet detonated — by gunfire,
    /// a grenade, or another barrel's blast. Because each blast damages nearby destructibles (barrels
    /// included) and the scan runs every frame, a row of drums chains naturally: A dies this frame, its
    /// blast kills B, and B's own detonation fires on the next scan — a rolling ripple instead of one
    /// simultaneous bang, which also reads better.
    /// Leave a persistent scorch-crater decal where a shot hit a wall, obstacle, or destructible. Capped at
    /// a fixed budget with FIFO eviction (oldest decal removed first) so a long firefight against the same
    /// wall doesn't grow the mesh list forever — same trade real engines make for decal pools.
    fn spawn_decal(&mut self, point: Vec3, normal: Vec3) {
        const MAX_DECALS: usize = 140;
        let seed = self.rng.next_u64() as u32;
        let radius = 0.11 + (seed % 100) as f32 * 0.0009; // slight size variety, ~0.11-0.20
        self.decals.push(crate::mesh::bullet_decal(point, normal, radius, seed));
        if self.decals.len() > MAX_DECALS {
            let over = self.decals.len() - MAX_DECALS;
            self.decals.drain(0..over);
        }
    }

    fn detonate_barrels(&mut self) {
        const BARREL_BLAST: f32 = 5.4;
        // collect this frame's pending detonations first (can't blast while iterating mutably)
        let mut blasts: Vec<Vec3> = Vec::new();
        for d in &mut self.level.destructibles {
            if d.explosive() && !d.blocks() && !d.detonated {
                d.detonated = true;
                blasts.push(d.center);
            }
        }
        for b in blasts {
            self.particles.smoke_puff(b + Vec3::new(0.0, 0.4, 0.0), 16);
            self.particles.impact_sparks(b, Vec3::UP);
            self.shake = (self.shake + 0.7).min(1.6);
            self.sfx.push(Sfx::Explosion(0.8));
            // shred nearby destructibles (this is the chain-reaction edge)
            for d in &mut self.level.destructibles {
                if !d.blocks() {
                    continue;
                }
                let dist = (d.center - b).len();
                if dist < BARREL_BLAST {
                    d.damage(80.0 * (1.0 - dist / BARREL_BLAST));
                }
            }
            // the player catches it
            let eye = self.player.eye();
            let pd = (eye - b).len();
            if pd < BARREL_BLAST && self.player.alive {
                let falloff = 1.0 - pd / BARREL_BLAST;
                self.player.take_damage(52.0 * falloff);
                let dir = (b - eye).with_y(0.0);
                if dir.len_sq() > 1e-3 {
                    self.damage_dirs.push((dir.normalized() * -1.0, 1.1));
                }
            }
            // enemies catch it — barrel kills credit the player (they shot the barrel)
            let mut kills = 0u32;
            for e in &mut self.enemies {
                if !e.alive() {
                    continue;
                }
                let ed = (e.center() - b).len();
                if ed < BARREL_BLAST {
                    let was = e.alive();
                    e.damage(90.0 * (1.0 - ed / BARREL_BLAST));
                    if was && !e.alive() {
                        kills += 1;
                    }
                }
            }
            if kills > 0 {
                self.kills += kills;
                self.score += 100 * kills;
                self.streak += kills;
                self.streak_points += 100 * kills;
                let label = if kills > 1 { format!("BARREL  x{}", kills) } else { "BARREL KILL".to_string() };
                self.feed.insert(0, (label, 2.5));
            } else {
                self.feed.insert(0, ("BARREL DETONATED".to_string(), 2.0));
            }
            self.feed.truncate(5);
        }
    }

    /// Fling a burst of physics debris chunks from a destroyed object at `center` (size `half`, tinted
    /// `color`). Each chunk gets an outward+upward velocity, a random spin, and tumbles under gravity. `count`
    /// scales with how big the thing was. This is the "building tumble" — call it whenever a destructible or
    /// a chunk of structure is blown apart.
    fn spawn_tumble(&mut self, center: Vec3, half: Vec3, color: egui::Color32, count: usize, force: f32) {
        // cap the live debris so a chain of explosions can't flood the sim
        const MAX_CHUNKS: usize = 90;
        for i in 0..count {
            if self.tumble.len() >= MAX_CHUNKS {
                break;
            }
            let a = self.rng.f32() * std::f32::consts::TAU;
            let up = 2.5 + self.rng.f32() * force;
            let out = 1.5 + self.rng.f32() * force * 0.7;
            let vel = Vec3::new(a.cos() * out, up, a.sin() * out);
            // chunk size a fraction of the parent, jittered
            let s = 0.18 + self.rng.f32() * 0.28;
            let ch = Vec3::new(half.x * s, half.y * s, half.z * s);
            let spawn = center + Vec3::new((self.rng.f32() - 0.5) * half.x, (self.rng.f32() - 0.5) * half.y, (self.rng.f32() - 0.5) * half.z);
            let axis = Vec3::new(self.rng.f32() - 0.5, self.rng.f32() - 0.5, self.rng.f32() - 0.5).normalized();
            self.tumble.push(TumbleChunk {
                pos: spawn,
                vel,
                half: ch,
                color: crate::mesh::dither(color, (i as i32) * 37 + center.x as i32, 0.12),
                ang: self.rng.f32() * std::f32::consts::TAU,
                ang_vel: (self.rng.f32() - 0.5) * 12.0,
                spin_axis: axis,
                life: 3.5 + self.rng.f32() * 2.0,
                rest: false,
            });
        }
    }

    /// Integrate the debris chunks: gravity, spin, floor bounce with damping, then settle + fade.
    fn update_tumble(&mut self, dt: f32) {
        let floor_y = self.level.floor_y;
        for c in &mut self.tumble {
            c.life -= dt;
            if c.rest {
                continue;
            }
            c.vel.y -= 26.0 * dt; // gravity
            c.pos = c.pos + c.vel * dt;
            c.ang += c.ang_vel * dt;
            // floor collision: bounce with energy loss, settle when slow
            let ground = self.level.surface_at(c.pos.xz()).unwrap_or(floor_y) + c.half.y;
            if c.pos.y <= ground {
                c.pos.y = ground;
                if c.vel.y.abs() < 1.6 && c.vel.xz().len() < 1.2 {
                    c.rest = true;
                    c.vel = Vec3::ZERO;
                    c.ang_vel = 0.0;
                } else {
                    c.vel.y = -c.vel.y * 0.42; // bounce
                    c.vel.x *= 0.7;
                    c.vel.z *= 0.7;
                    c.ang_vel *= 0.7;
                }
            }
        }
        // drop expired chunks
        self.tumble.retain(|c| c.life > 0.0);
    }

    /// Enemy grenades: occasionally a chasing bot lobs one toward you (more often at higher difficulty); in
    /// flight they arc under gravity and detonate on fuse or impact, dealing falloff blast damage so cover
    /// alone won't save you.
    fn update_grenades(&mut self, dt: f32) {
        // Throw throttle.
        self.grenade_timer -= dt;
        if self.grenade_timer <= 0.0 && self.player.alive {
            let peye = self.player.eye();
            let thrower = self
                .enemies
                .iter()
                .find(|e| e.is_chasing() && { let c = e.center(); let dx = c.x - peye.x; let dz = c.z - peye.z; let d2 = dx * dx + dz * dz; d2 > 64.0 && d2 < 30.0 * 30.0 });
            if let Some(e) = thrower {
                let from = e.center() + Vec3::new(0.0, 0.5, 0.0);
                let to = peye;
                let flat = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
                let dist = flat.len().max(1.0);
                let dir = flat * (1.0 / dist);
                // lob: horizontal speed proportional to distance + an upward arc
                let speed = (dist * 0.9).clamp(8.0, 20.0);
                let vel = dir * speed + Vec3::new(0.0, 6.5, 0.0);
                self.grenades.push(Grenade { pos: from, vel, fuse: 1.7, from_enemy: true, kind: crate::class::GrenadeKind::Frag });
                self.feed.insert(0, ("GRENADE OUT".to_string(), 1.6));
            }
            // Higher difficulty throws more frequently.
            self.grenade_timer = (6.5 * self.fire_mul).max(3.0) + self.rng.f32() * 3.0;
        }

        // Advance grenades; detonate on fuse expiry or when they hit the floor.
        let mut blasts: Vec<(Vec3, bool, crate::class::GrenadeKind)> = Vec::new();
        for g in &mut self.grenades {
            g.vel.y -= GRENADE_GRAV * dt;
            g.pos = g.pos + g.vel * dt;
            g.fuse -= dt;
            let floor = self.level.support_y(g.pos.x, g.pos.z, g.pos.y);
            if g.pos.y <= floor + 0.1 {
                g.pos.y = floor + 0.1;
                g.vel.y = -g.vel.y * 0.4; // small bounce
                g.vel.x *= 0.6;
                g.vel.z *= 0.6;
            }
            if g.fuse <= 0.0 {
                blasts.push((g.pos, g.from_enemy, g.kind));
            }
        }
        self.grenades.retain(|g| g.fuse > 0.0);

        for (b, from_enemy, kind) in blasts {
            use crate::class::GrenadeKind;
            self.particles.smoke_puff(b + Vec3::new(0.0, 0.3, 0.0), 12);
            self.shake = (self.shake + 0.6).min(1.6);
            // a detonation sound, scaled by kind (frag is the punchiest)
            self.sfx.push(Sfx::Explosion(match kind {
                GrenadeKind::Frag => 0.7,
                GrenadeKind::Gravity => 0.4,
                GrenadeKind::Acid => 0.35,
            }));
            match kind {
                // A gravity grenade arms a low-G well and gives a one-time outward pulse + light damage.
                GrenadeKind::Gravity => {
                    self.gravity_zones.push(GravityZone { center: b, radius: 6.5, life: 5.0 });
                    self.particles.impact_sparks(b, Vec3::UP);
                    for e in &mut self.enemies {
                        if e.alive() && (e.center() - b).len() < 6.5 {
                            e.damage(18.0);
                        }
                    }
                    self.feed.insert(0, ("GRAVITY WELL".to_string(), 2.5));
                    continue;
                }
                // An acid grenade lays a lingering damage puddle (doubled vs heavies, handled in update).
                GrenadeKind::Acid => {
                    self.acid_pools.push(AcidPool { center: b, radius: 4.2, life: 6.0, dps: 20.0 });
                    self.feed.insert(0, ("ACID POOL".to_string(), 2.5));
                    continue;
                }
                GrenadeKind::Frag => {}
            }
            self.particles.impact_sparks(b, Vec3::UP);
            self.blast_lights.push((b + Vec3::new(0.0, 0.5, 0.0), 0.4)); // dynamic explosion flash
            self.particles.embers(b + Vec3::new(0.0, 0.3, 0.0), 14); // rising glowing embers
            // Frag blasts shred nearby destructible cover with falloff — grenades open routes.
            let mut destroyed_tumble: Vec<(Vec3, Vec3, egui::Color32)> = Vec::new();
            for d in &mut self.level.destructibles {
                if !d.blocks() {
                    continue;
                }
                let dist = (d.center - b).len();
                if dist < GRENADE_BLAST {
                    // heavier charge — a grenade should reliably destroy cover, crates, barrels, walls near it
                    let just_destroyed = d.damage(180.0 * (1.0 - dist / GRENADE_BLAST));
                    if just_destroyed {
                        destroyed_tumble.push((d.center, d.half, d.base_tumble_color()));
                    }
                }
            }
            // Now that the destructible borrow is released, fling physics debris from anything that just
            // blew apart — the "building tumble".
            for (c, half, col) in destroyed_tumble {
                let count = (6.0 + half.x * half.z * 6.0) as usize;
                self.spawn_tumble(c, half, col, count.min(16), 6.0);
            }
            // Player catches the blast regardless of who threw it.
            let eye = self.player.eye();
            let d = (eye - b).len();
            if d < GRENADE_BLAST && self.player.alive {
                let falloff = 1.0 - d / GRENADE_BLAST;
                self.player.take_damage(58.0 * falloff);
                let dir = (b - eye).with_y(0.0);
                if dir.len_sq() > 1e-3 {
                    self.damage_dirs.push((dir.normalized() * -1.0, 1.1));
                }
            }
            // Enemies in the blast take falloff damage — friendly fire from an enemy frag, or your own frag.
            let mut player_kills = 0u32;
            let mut last_kill: Option<Vec3> = None;
            for e in &mut self.enemies {
                if !e.alive() {
                    continue;
                }
                let ec = e.center();
                let ed = (ec - b).len();
                if ed < GRENADE_BLAST {
                    let was_alive = e.alive();
                    e.damage(140.0 * (1.0 - ed / GRENADE_BLAST));
                    self.particles.blood(ec, Vec3::UP);
                    if was_alive && !e.alive() && !from_enemy {
                        player_kills += 1;
                        last_kill = Some(e.pos);
                    }
                }
            }
            if player_kills > 0 {
                self.kills += player_kills;
                self.score += 100 * player_kills;
                self.streak += player_kills;
                self.streak_points += 100 * player_kills;
                let label = if player_kills > 1 { format!("FRAG  x{}", player_kills) } else { "FRAG KILL".to_string() };
                self.feed.insert(0, (label, 3.0));
                self.feed.truncate(5);
                if let (Some(p), Some(kind)) = (last_kill, pickups::roll_drop(&mut self.rng)) {
                    let phase = self.time;
                    self.pickups.push(Pickup::new(kind, p.with_y(self.level.floor_y), phase));
                }
            }
        }
    }

    /// Throw the class's equipment grenade along the current aim line, if any remain. Frag detonates on the
    /// fuse; gravity and acid arm a lingering field where they land.
    pub fn throw_frag(&mut self) {
        if self.frags == 0 || !self.player.alive {
            return;
        }
        self.frags -= 1;
        let eye = self.player.eye();
        let dir = self.player.cam.forward();
        let vel = dir * 24.0 + Vec3::new(0.0, 3.6, 0.0); // harder throw → more range (was 17)
        let kind = self.player.class.def().grenade;
        self.grenades.push(Grenade { pos: eye + dir * 0.4, vel, fuse: 1.6, from_enemy: false, kind });
    }

    /// Number of live particles (for the liveliness harness / HUD debug).
    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }
    /// Number of live physics debris chunks.
    pub fn tumble_count(&self) -> usize {
        self.tumble.len()
    }
    /// World positions of all live particles (liveliness harness).
    pub fn particles_iter(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.particles.positions()
    }
    /// World positions of live tumble debris chunks.
    pub fn tumble_positions(&self) -> Vec<Vec3> {
        self.tumble.iter().map(|c| c.pos).collect()
    }
    /// World positions of active fire sources.
    pub fn fire_positions(&self) -> Vec<Vec3> {
        self.level.fire_sources.clone()
    }
    /// A cheap proxy for swaying/dangling geometry activity in view: counts meshes that carry sway weights
    /// whose bounds fall inside the view predicate. Used by the liveliness harness to credit wind-bowed
    /// foliage, cloth, and wires as low-level ambient motion.
    pub fn sway_sample(&self, _cam: Vec3, in_view: &dyn Fn(Vec3) -> bool) -> f32 {
        let mut count = 0.0f32;
        for m in &self.level.meshes {
            if m.sway.is_empty() {
                continue;
            }
            // representative point: the mesh's first vertex + offset
            if let Some(v) = m.verts.first() {
                if in_view(*v + m.offset) {
                    count += 1.0;
                }
            }
        }
        count.min(20.0) // cap so a field of grass doesn't dominate
    }

    /// Frag grenades remaining (for the HUD).
    pub fn frags(&self) -> u32 {
        self.frags
    }

    /// Super-ability charge 0..1 (for the HUD).
    pub fn super_charge(&self) -> f32 {
        self.super_charge
    }

    /// Activate the class super if it's fully charged. Tank: two auto-targeting shoulder cannons for a short
    /// window. Assassin: launch off the ground, arming a slam that detonates a gravity shockwave on landing.
    pub fn activate_super(&mut self) {
        use crate::class::SuperKind;
        if self.super_charge < 1.0 || !self.player.alive {
            return;
        }
        match self.player.class.def().super_kind {
            SuperKind::None => return,
            SuperKind::ShoulderCannons => {
                let p = self.player.pos;
                let fwd = self.player.cam.forward_flat();
                let right = Vec3::new(fwd.z, 0.0, -fwd.x);
                for side in [-0.55f32, 0.55] {
                    let pos = p + right * side + fwd * 0.4 + Vec3::new(0.0, 1.25, 0.0);
                    self.sentries.push(Sentry { pos, yaw: self.player.cam.yaw, cooldown: 0.2, life: 7.0, ammo: 240 });
                }
                self.cannon_timer = 7.0;
                self.feed.insert(0, ("SHOULDER CANNONS".to_string(), 3.0));
            }
            SuperKind::GravitySlam => {
                let dir = self.player.cam.forward();
                self.player.vel.y = 13.0;
                self.player.vel.x += dir.x * 6.0;
                self.player.vel.z += dir.z * 6.0;
                self.player.on_ground = false;
                self.slam_armed = true;
                self.feed.insert(0, ("GRAVITY SLAM".to_string(), 3.0));
            }
        }
        self.super_charge = 0.0;
    }

    /// Advance in-flight rocket-launcher rounds and detonate them on the first thing they touch — a wall,
    /// an obstacle, a destructible, or an enemy — using a per-tick raycast so a fast projectile can't tunnel
    /// through thin geometry in one frame. Unlike grenades (fuse/floor-bounce), this is impact detonation:
    /// a rocket that flies clean over everything self-detonates at `max_range` instead of flying forever.
    fn update_rockets(&mut self, dt: f32) {
        let mut blasts: Vec<Vec3> = Vec::new();
        for r in &mut self.rockets {
            r.vel.y -= ROCKET_GRAV * dt;
            let step = r.vel * dt;
            let step_len = step.len();
            if step_len < 1e-5 {
                continue;
            }
            let dir = step * (1.0 / step_len);
            let targets: Vec<_> = self.enemies.iter().filter(|e| e.alive()).map(|e| e.target()).collect();
            let hit = crate::combat::raycast(&self.level, &targets, r.pos, dir, step_len);
            if hit.kind != crate::combat::HitKind::Miss {
                blasts.push(hit.point);
                r.traveled = r.max_range; // mark for despawn below
                continue;
            }
            r.pos = r.pos + step;
            r.traveled += step_len;
            if r.traveled >= r.max_range {
                blasts.push(r.pos);
            }
        }
        self.rockets.retain(|r| r.traveled < r.max_range);

        for b in blasts {
            self.particles.smoke_puff(b + Vec3::new(0.0, 0.3, 0.0), 16);
            self.particles.impact_sparks(b, Vec3::UP);
            self.spawn_decal(b, Vec3::UP);
            self.shake = (self.shake + 0.9).min(1.8);
            self.sfx.push(Sfx::Explosion(0.9));
            let mut rocket_tumble: Vec<(Vec3, Vec3, egui::Color32)> = Vec::new();
            for d in &mut self.level.destructibles {
                if !d.blocks() {
                    continue;
                }
                let dist = (d.center - b).len();
                if dist < ROCKET_BLAST {
                    // a rocket flattens cover — big destructible damage
                    let just = d.damage(220.0 * (1.0 - dist / ROCKET_BLAST));
                    if just {
                        rocket_tumble.push((d.center, d.half, d.base_tumble_color()));
                    }
                }
            }
            for (c, half, col) in rocket_tumble {
                let count = (8.0 + half.x * half.z * 7.0) as usize;
                self.spawn_tumble(c, half, col, count.min(20), 8.0);
            }
            let eye = self.player.eye();
            let pd = (eye - b).len();
            if pd < ROCKET_BLAST && self.player.alive {
                let falloff = 1.0 - pd / ROCKET_BLAST;
                self.player.take_damage(45.0 * falloff);
                let dir = (b - eye).with_y(0.0);
                if dir.len_sq() > 1e-3 {
                    self.damage_dirs.push((dir.normalized() * -1.0, 1.1));
                }
            }
            let mut kills = 0u32;
            for e in &mut self.enemies {
                if !e.alive() {
                    continue;
                }
                let ed = (e.center() - b).len();
                if ed < ROCKET_BLAST {
                    let was = e.alive();
                    e.damage(120.0 * (1.0 - ed / ROCKET_BLAST));
                    if was && !e.alive() {
                        kills += 1;
                    }
                }
            }
            if kills > 0 {
                self.kills += kills;
                self.score += 100 * kills;
                self.streak += kills;
                self.streak_points += 100 * kills;
                let label = if kills > 1 { format!("SPLASH  x{}", kills) } else { "SPLASH KILL".to_string() };
                self.feed.insert(0, (label, 2.5));
                self.feed.truncate(5);
            }
        }
    }

    /// Charge the super, tick down gravity wells / acid pools (applying their effects), and detonate the
    /// Assassin's slam on touchdown.
    fn update_supers(&mut self, dt: f32) {
        use crate::class::SuperKind;
        if self.player.alive && self.player.class.def().super_kind != SuperKind::None && self.super_charge < 1.0 {
            self.super_charge = (self.super_charge + dt / 30.0 * self.player.class.def().charge_rate).min(1.0);
        }
        if self.cannon_timer > 0.0 {
            self.cannon_timer -= dt;
        }
        for z in &mut self.gravity_zones {
            z.life -= dt;
        }
        self.gravity_zones.retain(|z| z.life > 0.0);

        // Acid pools: damage anyone standing in them, doubled against heavies (Brutes).
        for pool in &mut self.acid_pools {
            pool.life -= dt;
            for e in &mut self.enemies {
                if !e.alive() {
                    continue;
                }
                if (e.center() - pool.center).len() < pool.radius {
                    let mult = if e.kind.def().armored { 2.0 } else { 1.0 };
                    e.damage(pool.dps * mult * dt);
                }
            }
            if self.player.alive {
                let pf = self.player.pos;
                if ((pf.x - pool.center.x).powi(2) + (pf.z - pool.center.z).powi(2)).sqrt() < pool.radius {
                    self.player.take_damage(pool.dps * 0.5 * dt); // friendly acid stings a little
                }
            }
        }
        self.acid_pools.retain(|p| p.life > 0.0);

        // Gravity slam: detonate a shockwave the moment we land.
        if self.slam_armed && self.player.on_ground {
            self.slam_armed = false;
            let b = self.player.pos;
            self.shake = (self.shake + 1.0).min(1.8);
            self.sfx.push(Sfx::Explosion(0.6));
            self.particles.impact_sparks(b, Vec3::UP);
            self.particles.smoke_puff(b + Vec3::new(0.0, 0.3, 0.0), 16);
            let mut kills = 0u32;
            for e in &mut self.enemies {
                if !e.alive() {
                    continue;
                }
                let d = (e.center() - b).len();
                if d < 7.0 {
                    let was = e.alive();
                    e.damage(120.0 * (1.0 - d / 7.0));
                    if was && !e.alive() {
                        kills += 1;
                    }
                }
            }
            if kills > 0 {
                self.kills += kills;
                self.score += 75 * kills;
            }
            self.feed.insert(0, ("SLAM".to_string(), 2.0));
        }
    }

    /// The Hunt survivor mode: an AI creature stalks every survivor (the player and three AI), who flee and
    /// hide. The creature targets whichever survivor is nearest and melee-kills on contact.
    fn update_survivor_enemies(&mut self, dt: f32) {
        let hunter_idx = match self.enemies.iter().position(|e| e.hunter && e.alive()) {
            Some(i) => i,
            None => return,
        };
        let h_eye = self.enemies[hunter_idx].eye_pos();
        let h_feet = self.enemies[hunter_idx].center();

        // Every other survivor flees the hunter and hides.
        for i in 0..self.enemies.len() {
            if i == hunter_idx || !self.enemies[i].alive() {
                continue;
            }
            let _ = self.enemies[i].update(&self.level, h_eye, h_feet, self.detect_mul, self.aim_mul, self.fire_mul, true, &mut self.rng, dt);
        }

        if self.hunt_escaped {
            return; // dawn broke; the creature breaks off
        }

        // The creature picks the nearest survivor — player or AI.
        let mut best = f32::MAX;
        let mut tgt: Option<Vec3> = None;
        let mut tgt_prey: Option<usize> = None;
        if self.player.alive {
            let d = (self.player.pos - h_feet).len();
            if d < best {
                best = d;
                tgt = Some(self.player.eye());
            }
        }
        for i in 0..self.enemies.len() {
            if i == hunter_idx || !self.enemies[i].alive() {
                continue;
            }
            let c = self.enemies[i].center();
            let d = (c - h_feet).len();
            if d < best {
                best = d;
                tgt = Some(c);
                tgt_prey = Some(i);
            }
        }

        if let Some(t) = tgt {
            let reach = self.creature.reach();
            let speed = self.creature.speed_mul() * 1.08;
            let contact = self.enemies[hunter_idx].hunt_toward(&self.level, t, speed, reach, dt);
            if contact && self.strike_cd <= 0.0 {
                self.strike_cd = self.creature.strike_cd();
                self.shake = (self.shake + 0.5).min(1.4);
                if let Some(pi) = tgt_prey {
                    self.enemies[pi].damage(self.creature.damage());
                    let c = self.enemies[pi].center();
                    self.particles.blood(c, Vec3::UP);
                    if !self.enemies[pi].alive() {
                        self.feed.insert(0, ("A SURVIVOR WAS TAKEN".to_string(), 3.0));
                        self.feed.truncate(5);
                    }
                } else if self.player.alive {
                    self.player.take_damage(self.creature.damage());
                    let dir = (h_feet - self.player.eye()).with_y(0.0);
                    if dir.len_sq() > 1e-3 {
                        self.damage_dirs.push((dir.normalized(), 1.2));
                    }
                }
            }
        }
    }

    /// True if this is The Hunt played from the survivor's side.
    pub fn is_survivor_hunt(&self) -> bool {
        self.mode == GameMode::Hunt && self.hunt_role == HuntRole::Survivor
    }

    /// Seconds left on the escape countdown (survivor mode).
    pub fn hunt_timer(&self) -> f32 {
        self.hunt_timer
    }

    /// True once the survivor outlasts the hunt.
    pub fn hunt_escaped(&self) -> bool {
        self.hunt_escaped
    }

    /// Survivors still alive (the player plus living AI survivors).
    pub fn survivors_left(&self) -> usize {
        (if self.player.alive { 1 } else { 0 }) + self.enemies.iter().filter(|e| e.alive() && !e.hunter).count()
    }

    /// True if this match is The Hunt (player is a creature, enemies are prey).
    /// This map's signature live-event kind (biome spectacle), and its current intensity 0..1 while active.
    pub fn map_event(&self) -> MapEvent {
        self.map_event
    }
    pub fn map_event_intensity(&self) -> f32 {
        if self.map_event_active > 0.0 {
            self.map_event_intensity
        } else {
            0.0
        }
    }

    pub fn is_hunt(&self) -> bool {
        self.mode == GameMode::Hunt
    }

    /// The creature the player is hunting as.
    pub fn creature(&self) -> Creature {
        self.creature
    }

    /// Creature melee claw strike (Hunt mode): hit the nearest prey in a forward cone within reach.
    fn melee_strike(&mut self) {
        self.strike_cd = self.creature.strike_cd();
        self.shake = (self.shake + 0.3).min(0.8);
        let eye = self.player.eye();
        let fwd = self.player.cam.forward();
        let reach = self.creature.reach();
        let dmg = self.creature.damage();
        let mut best: Option<usize> = None;
        let mut best_d = reach;
        for (i, e) in self.enemies.iter().enumerate() {
            if !e.alive() {
                continue;
            }
            let c = e.center();
            let to = c - eye;
            let d = to.len();
            if d > reach || d < 1e-3 {
                continue;
            }
            let dot = (to.x * fwd.x + to.y * fwd.y + to.z * fwd.z) / d;
            if dot > 0.5 && d < best_d {
                best = Some(i);
                best_d = d;
            }
        }
        if let Some(i) = best {
            let was_alive = self.enemies[i].alive();
            self.enemies[i].damage(dmg);
            let ec = self.enemies[i].center();
            self.particles.blood(ec, fwd);
            // Storm Kraken strikes arc lightning: a burst of electric ion sparks at the hit plus a bright
            // blue-white flash light, so the kill reads as a discharge rather than a claw swipe.
            if self.creature.electric() {
                for _ in 0..3 {
                    self.particles.ion_thrust(ec + Vec3::new(0.0, 0.4, 0.0), fwd);
                }
                self.blast_lights.push((ec + Vec3::new(0.0, 0.5, 0.0), 0.25));
            }
            if was_alive && !self.enemies[i].alive() {
                self.kills += 1;
                self.score += 75;
                self.streak += 1;
                self.streak_points += 75;
                let label = if self.streak >= 3 { format!("PREY DOWN  x{}", self.streak) } else { "PREY DOWN".to_string() };
                self.feed.insert(0, (label, 3.0));
                self.feed.truncate(5);
            }
        }
    }

    fn next_wave(&mut self) {
        self.wave += 1;
        let n = 10 + self.wave as usize * 3;
        self.enemies = populate(&self.level, n, self.seed.wrapping_add(self.wave as u64 * 1103));
        scale_enemy_health(&mut self.enemies, self.difficulty.health_mul());
        self.frags = (self.frags + 2).min(3);
        // Health regenerates on its own now; clearing a wave just resupplies your reserves.
        self.player.add_ammo();
        self.feed.insert(0, (format!("WAVE {} INCOMING", self.wave), 3.5));
        // Every few waves, a fog bank rolls in with the new enemies.
        if self.wave % 3 == 0 {
            self.trigger_fog_event();
        }
    }

    pub fn uav_ready(&self) -> bool {
        !self.uav_used && self.streak_points >= UAV_COST
    }
    pub fn strike_ready(&self) -> bool {
        !self.strike_used && self.streak_points >= STRIKE_COST
    }
    /// Progress (0..1) toward each scorestreak, for the HUD.
    pub fn uav_progress(&self) -> f32 {
        (self.streak_points as f32 / UAV_COST as f32).min(1.0)
    }
    pub fn strike_progress(&self) -> f32 {
        (self.streak_points as f32 / STRIKE_COST as f32).min(1.0)
    }
    pub fn sentry_ready(&self) -> bool {
        !self.sentry_used && self.streak_points >= SENTRY_COST
    }
    pub fn sentry_progress(&self) -> f32 {
        (self.streak_points as f32 / SENTRY_COST as f32).min(1.0)
    }

    /// Deployed sentries acquire the nearest live enemy in range and fire on a fast cooldown.
    fn update_sentries(&mut self, dt: f32) {
        if self.sentries.is_empty() {
            return;
        }
        let mut sentries = std::mem::take(&mut self.sentries);
        for s in &mut sentries {
            s.life -= dt;
            if s.cooldown > 0.0 {
                s.cooldown -= dt;
            }
            let muzzle = s.pos + Vec3::new(0.0, 0.95, 0.0);
            // nearest live enemy in range
            let mut best: Option<(Vec3, f32)> = None;
            for e in &self.enemies {
                if !e.alive() {
                    continue;
                }
                let c = e.center();
                let d = (c - muzzle).len();
                if d > 30.0 {
                    continue;
                }
                if best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((c, d));
                }
            }
            if let Some((c, _)) = best {
                let dir = (c - muzzle).normalized();
                s.yaw = dir.x.atan2(-dir.z);
                if s.cooldown <= 0.0 && s.ammo > 0 {
                    let targets: Vec<_> = self.enemies.iter().filter(|e| e.alive()).map(|e| e.target()).collect();
                    let hit = raycast(&self.level, &targets, muzzle, dir, 32.0);
                    self.particles.muzzle_flash(muzzle, dir, rgb(255, 220, 150));
                    self.tracers.push(Tracer { a: muzzle, b: hit.point, life: 0.05, max_life: 0.05, color: rgb(255, 220, 150), rad: 0.02 });
                    if let HitKind::Enemy(hid) | HitKind::Headshot(hid) = hit.kind {
                        let hs = matches!(hit.kind, HitKind::Headshot(_));
                        self.particles.blood(hit.point, dir);
                        if let Some(e) = self.enemies.iter_mut().find(|e| e.id == hid) {
                            let was = e.alive();
                            e.damage(if hs { 35.0 } else { 16.0 });
                            if was && !e.alive() {
                                self.kills += 1;
                                self.score += 50;
                            }
                        }
                    }
                    s.cooldown = 0.15;
                    s.ammo -= 1;
                }
            }
        }
        sentries.retain(|s| s.life > 0.0 && s.ammo > 0);
        self.sentries = sentries;
    }

    /// Call in a missile strike: heavy damage to every live enemy with explosions and a big shake.
    fn missile_strike(&mut self) {
        self.strike_used = true;
        self.feed.insert(0, ("MISSILE STRIKE INBOUND".to_string(), 3.5));
        self.shake = (self.shake + 1.0).min(1.6);
        let ids: Vec<usize> = self.enemies.iter().filter(|e| e.alive()).map(|e| e.id).collect();
        for id in ids {
            let (center, pos) = match self.enemies.iter().find(|e| e.id == id) {
                Some(e) => (e.center(), e.pos),
                None => continue,
            };
            self.particles.plasma_splash(center, Vec3::UP, rgb(255, 180, 90));
            self.particles.smoke_puff(center, 6);
            if let Some(e) = self.enemies.iter_mut().find(|e| e.id == id) {
                let was_alive = e.alive();
                e.damage(140.0);
                if was_alive && !e.alive() {
                    self.kills += 1;
                    self.score += 100;
                }
            }
            let _ = pos;
        }
    }

    /// Resolve one trigger pull: per-pellet raycast, damage, effects, recoil, ammo.
    fn fire(&mut self) {
        let cam = self.player.view_cam();
        let def = self.player.weapon().kind.def();
        let origin = self.player.muzzle();
        let fwd = cam.forward();
        let right = cam.right();
        let up = cam.up();

        // Aiming down sights tightens the cone; hip-fire is loose.
        // Base spread tightens as you aim; Gunslinger tightens hip-fire further (applied at hip, fades in ADS).
        let hip_tighten = self.player.loadout.hipfire_mul();
        let hipfire_factor = hip_tighten + (1.0 - hip_tighten) * self.player.ads_t; // →hip_tighten at hip, →1 aimed
        let spread = def.spread * (1.0 - 0.7 * self.player.ads_t) * hipfire_factor;
        let sniper = def.kind == WeaponKind::Sniper;

        // Muzzle flash + ejected brass at the barrel.
        self.particles.muzzle_flash(origin, fwd, def.muzzle);
        self.muzzle_t = 1.0; // light up the muzzle flash dynamic light
        self.particles.eject_shell(origin - up * 0.05, right, up);
        // Audio: a report whose "bore" (beefiness) tracks the weapon class.
        let bore = match def.kind {
            WeaponKind::Launcher => 1.0,
            WeaponKind::Sniper | WeaponKind::Lmg => 0.95,
            WeaponKind::Shotgun => 0.85,
            WeaponKind::Dmr | WeaponKind::Ar => 0.6,
            WeaponKind::Smg => 0.4,
            WeaponKind::Pistol => 0.5,
            WeaponKind::EmSword => 0.3,
            WeaponKind::ArcCoil => 0.7,
            WeaponKind::CryoLance => 0.75,
            WeaponKind::VoidMaul => 0.45,
        };
        self.sfx.push(Sfx::Shot(bore));

        let mut any_hit = false;
        let mut any_kill = false;
        if def.kind == WeaponKind::Launcher {
            // A real projectile, not a hitscan pellet: no immediate hit resolution here — the blast (and
            // any resulting hitmarker-equivalent feedback) happens in `update_rockets` when it lands.
            self.rockets.push(Rocket { pos: origin, vel: fwd * ROCKET_SPEED, traveled: 0.0, max_range: def.range });
        } else {
            for _ in 0..def.pellets {
            let dir = jitter(fwd, spread, &mut self.rng);
            let targets: Vec<_> = self
                .enemies
                .iter()
                .filter(|e| e.alive())
                .map(|e| e.target())
                .collect();
            let hit = raycast(&self.level, &targets, origin, dir, def.range);

            // Tracer for every pellet — snipers leave a bright, thick streak.
            let tcol = if sniper { rgb(220, 235, 255) } else { rgb(255, 230, 170) };
            let trad = if sniper { 0.04 } else { 0.025 };
            self.tracers.push(Tracer { a: origin, b: hit.point, life: 0.06, max_life: 0.06, color: tcol, rad: trad });

            match hit.kind {
                HitKind::Enemy(id) | HitKind::Headshot(id) => {
                    any_hit = true;
                    let headshot = matches!(hit.kind, HitKind::Headshot(_));
                    let dmg_mul = if headshot { 2.2 } else { 1.0 }; // headshots hit much harder
                    self.particles.blood(hit.point, dir);
                    let mut drop_at: Option<Vec3> = None;
                    if let Some(e) = self.enemies.iter_mut().find(|e| e.id == id) {
                        let was_alive = e.alive();
                        e.damage(def.damage * self.player.damage_mul * dmg_mul);
                        if was_alive && !e.alive() {
                            self.kills += 1;
                            self.score += if headshot { 150 } else { 100 };
                            self.streak += 1;
                            self.streak_points += if headshot { 150 } else { 100 };
                            any_kill = true;
                            drop_at = Some(e.pos);
                        }
                    }
                    // EXOTIC — Arc Coil chain lightning: after the primary hit, arc to the nearest living
                    // enemies within range, each jump doing reduced damage with a bright electric tracer.
                    if def.chain > 0 {
                        let mut from = hit.point;
                        let mut zapped: Vec<usize> = vec![id];
                        for jump in 0..def.chain {
                            // find the nearest living enemy not yet zapped, within a jump radius
                            let mut best: Option<(usize, Vec3, f32)> = None;
                            for e in self.enemies.iter().filter(|e| e.alive() && !zapped.contains(&e.id)) {
                                let c = e.center();
                                let d = (c - from).len();
                                if d < 9.0 && best.map(|b| d < b.2).unwrap_or(true) {
                                    best = Some((e.id, c, d));
                                }
                            }
                            let Some((nid, npos, _)) = best else { break };
                            zapped.push(nid);
                            self.tracers.push(Tracer { a: from, b: npos, life: 0.08, max_life: 0.08, color: rgb(150, 220, 255), rad: 0.03 });
                            self.particles.ion_thrust(npos, (npos - from).normalized());
                            let chain_dmg = def.damage * self.player.damage_mul * (0.7f32).powi(jump as i32 + 1);
                            let mut chain_drop: Option<Vec3> = None;
                            if let Some(e) = self.enemies.iter_mut().find(|e| e.id == nid) {
                                let was = e.alive();
                                e.damage(chain_dmg);
                                if was && !e.alive() {
                                    self.kills += 1;
                                    self.score += 100;
                                    self.streak += 1;
                                    any_kill = true;
                                    chain_drop = Some(e.pos);
                                }
                            }
                            if let Some(p) = chain_drop {
                                if let Some(kind) = pickups::roll_drop(&mut self.rng) {
                                    self.pickups.push(Pickup::new(kind, p.with_y(self.level.floor_y), self.time));
                                }
                            }
                            from = npos;
                        }
                    }
                    // EXOTIC — Cryo Lance pierce: the beam passes through and hits every enemy further along the
                    // same line (beyond the first), each taking full damage.
                    if def.pierce {
                        for e in self.enemies.iter_mut().filter(|e| e.alive() && e.id != id) {
                            let c = e.center();
                            let to = c - origin;
                            let along = to.x * dir.x + to.y * dir.y + to.z * dir.z;
                            if along <= 0.0 || along > def.range {
                                continue;
                            }
                            // perpendicular distance from the beam line
                            let closest = origin + dir * along;
                            let perp = (c - closest).len();
                            if perp < e.target().radius + 0.6 {
                                let was = e.alive();
                                e.damage(def.damage * self.player.damage_mul);
                                if was && !e.alive() {
                                    self.kills += 1;
                                    self.score += 100;
                                    self.streak += 1;
                                    any_kill = true;
                                }
                            }
                        }
                    }
                    if let Some(p) = drop_at {
                        let label = if headshot {
                            "HEADSHOT".to_string()
                        } else if self.streak >= 3 {
                            format!("ELIMINATED  x{}", self.streak)
                        } else {
                            "ELIMINATED".to_string()
                        };
                        self.feed.insert(0, (label, 3.0));
                        self.feed.truncate(5);
                        if let Some(kind) = pickups::roll_drop(&mut self.rng) {
                            let phase = self.time;
                            self.pickups.push(Pickup::new(kind, p.with_y(self.level.floor_y), phase));
                        }
                    }
                }
                HitKind::Wall | HitKind::Obstacle => {
                    self.particles.impact_sparks(hit.point, hit.normal);
                    self.spawn_decal(hit.point, hit.normal);
                }
                HitKind::Destructible(idx) => {
                    self.particles.impact_sparks(hit.point, hit.normal);
                    self.spawn_decal(hit.point, hit.normal);
                    let mut broke: Option<Vec3> = None;
                    if let Some(d) = self.level.destructibles.get_mut(idx) {
                        if d.damage(def.damage * self.player.damage_mul) {
                            broke = Some(d.center);
                        }
                    }
                    if let Some(c) = broke {
                        // it just blew apart — kick up debris and announce the opened cover
                        self.particles.smoke_puff(c, 14);
                        self.feed.insert(0, ("COVER DESTROYED".to_string(), 2.0));
                        self.feed.truncate(5);
                    }
                }
                HitKind::Miss => {}
            }
        }
        }

        // Hitmarker feedback (kill marker is brighter/red).
        if any_hit {
            self.hitmarker = 0.18;
            self.hitmarker_kill = any_kill;
            self.sfx.push(Sfx::Impact(0.4)); // flesh/armor thud
        }

        // Consume a round, start cooldown, kick the view and shake the screen.
        {
            let w = self.player.weapon_mut();
            w.consume();
            w.cooldown = def.interval;
            // Auto-reload when the magazine runs dry.
            if w.mag == 0 {
                w.start_reload();
            }
        }
        // Recoil pattern: vertical climb grows with sustained fire; horizontal wanders side to side
        // (a sinusoid plus a little randomness) so a spray drifts and must be controlled.
        let n = self.recoil_shots;
        let vert = def.kick * (1.0 + n * 0.05);
        let wander = (n * 0.7).sin() * 0.7 + (self.rng.f32() - 0.5) * 0.6;
        let horiz = def.recoil_h * wander;
        self.player.add_recoil(vert * self.player.recoil_mul, horiz * self.player.recoil_mul);
        self.recoil_shots = (self.recoil_shots + 1.0).min(20.0);
        self.shake = (self.shake + def.kick * 6.0).min(1.5);
    }

    /// Tessellate the level's water/lava surfaces fresh each frame with traveling waves. Flat-shaded, so each
    /// facet takes the wave-gradient normal at its centre — that makes the fresnel sky reflection shimmer and
    /// break up across the ripples instead of mirroring a flat plate. Crests lighten to foam (water) or glow
    /// molten (lava). Cheap: a couple of thousand triangles only on the maps that actually have water.
    fn build_water_meshes(&self) -> Vec<Mesh3> {
        let t = self.time;
        let mut out: Vec<Mesh3> = Vec::new();
        for wb in &self.level.water {
            // (dirx, dirz, wavelength, amplitude, speed) — a few crossing wave trains.
            let waves: [(f32, f32, f32, f32, f32); 3] = if wb.lava {
                [(0.82, 0.2, 9.0, 0.10, 0.45), (-0.3, 0.92, 14.0, 0.13, 0.30), (0.5, -0.62, 6.0, 0.05, 0.65)]
            } else {
                [(0.9, 0.22, 5.0, 0.07, 1.25), (-0.42, 0.86, 3.2, 0.05, 1.7), (0.2, -0.72, 8.5, 0.10, 0.85)]
            };
            let sample = |x: f32, z: f32| -> (f32, Vec3) {
                let (mut h, mut dx, mut dz) = (0.0f32, 0.0f32, 0.0f32);
                for (ux, uz, wl, amp, sp) in waves {
                    let k = std::f32::consts::TAU / wl;
                    let ph = (x * ux + z * uz) * k + t * sp;
                    h += amp * ph.sin();
                    dx += amp * k * ux * ph.cos();
                    dz += amp * k * uz * ph.cos();
                }
                (h, Vec3::new(-dx, 1.0, -dz).normalized())
            };
            // Crest tint: water lightens to foam, lava glows molten orange — plus fine surface ripple sparkle
            // and foam streaks concentrated on the crests so the sheet reads as moving water, not a flat tint.
            let lvl = &self.level;
            let crest = |x: f32, z: f32, h: f32| -> Color32 {
                let cf = (h * 4.0).clamp(0.0, 1.0);
                if wb.lava {
                    let flick = 0.5 + 0.5 * ((x * 2.3 - t * 1.1).sin() * (z * 1.9 + t * 0.7).sin());
                    lerp_c(wb.col, rgb(255, 200, 90), (cf * (0.6 + 0.5 * flick)).clamp(0.0, 1.0))
                } else {
                    // depth below the surface → darken and shift toward deep blue-green
                    let depth = (wb.y - lvl.floor_h(x, z)).clamp(0.0, 4.0);
                    let df = depth / 4.0;
                    let base = lerp_c(crate::mesh::scale_rgb(wb.col, 1.12), rgb(18, 46, 66), df);
                    // fine crossing ripples → subtle bright/dark surface shimmer
                    let rip = 0.5 + 0.5 * ((x * 3.3 + t * 1.6).sin() * (z * 2.9 - t * 1.1).sin());
                    let mut c = crate::mesh::scale_rgb(base, 0.9 + 0.2 * rip);
                    c = lerp_c(c, rgb(225, 238, 245), cf * 0.55 * (1.0 - df * 0.5));
                    // foam streaks: patchy white where a crest and a ripple line up (stronger in the shallows)
                    let foam = ((cf - 0.35).max(0.0) * 1.6 * rip - 0.25).max(0.0) * (1.0 - df * 0.6);
                    lerp_c(c, rgb(240, 248, 250), foam.min(0.7))
                }
            };
            let mut m = Mesh3::new(wb.col);
            m.reflect = wb.reflect;
            m.emissive = if wb.lava { wb.emissive * (0.8 + 0.25 * (t * 1.4).sin()) } else { wb.emissive };

            if wb.disc {
                let (rings, segs) = (10, 22);
                let (cx, cz, r) = (wb.cx, wb.cz, wb.radius);
                for ri in 0..rings {
                    let r0 = r * ri as f32 / rings as f32;
                    let r1 = r * (ri + 1) as f32 / rings as f32;
                    for si in 0..segs {
                        let a0 = si as f32 / segs as f32 * std::f32::consts::TAU;
                        let a1 = (si + 1) as f32 / segs as f32 * std::f32::consts::TAU;
                        let pt = |rr: f32, a: f32| {
                            let (x, z) = (cx + a.cos() * rr, cz + a.sin() * rr);
                            let (h, n) = sample(x, z);
                            (Vec3::new(x, wb.y + h, z), n, h)
                        };
                        let (p00, n00, h00) = pt(r0, a0);
                        let (p10, _, _) = pt(r1, a0);
                        let (p11, _, _) = pt(r1, a1);
                        let (p01, _, _) = pt(r0, a1);
                        m.push_quad(p00, p10, p11, p01, n00, crest(p00.x, p00.z, h00));
                    }
                }
            } else {
                let div = 26;
                let step = wb.radius * 2.0 / div as f32;
                let x0 = wb.cx - wb.radius;
                let z0 = wb.cz - wb.radius;
                for iz in 0..div {
                    for ix in 0..div {
                        let xa = x0 + ix as f32 * step;
                        let za = z0 + iz as f32 * step;
                        let p = |x: f32, z: f32| {
                            let (h, n) = sample(x, z);
                            (Vec3::new(x, wb.y + h, z), n, h)
                        };
                        let (p00, n00, h00) = p(xa, za);
                        let (p10, _, _) = p(xa + step, za);
                        let (p11, _, _) = p(xa + step, za + step);
                        let (p01, _, _) = p(xa, za + step);
                        m.push_quad(p00, p10, p11, p01, n00, crest(p00.x, p00.z, h00));
                    }
                }
            }
            crate::mesh::set_alpha(&mut m, wb.alpha);
            out.push(m);
        }
        out
    }

    /// Build the full world mesh list for this frame (level, enemies, particles, tracers, viewmodel).
    /// Build world markers for the active objective mode: pulsing bomb-site rings (Search & Destroy) and
    /// flag poles/banners at each base (Capture the Flag). Empty when no objective mode is active.
    fn build_objective_meshes(&self) -> Vec<Mesh3> {
        use crate::mesh::{boxx, disc, rgb, tube, FACE_ALL};
        use crate::objectives::Objective;
        let mut out: Vec<Mesh3> = Vec::new();
        match self.objective.mode {
            Objective::None => {}
            Objective::SearchDestroy => {
                let planted = self.objective.planted_site;
                for (i, site) in self.objective.sites.iter().enumerate() {
                    let is_planted = planted == Some(i);
                    // a pulsing ground ring; red once planted, amber otherwise
                    let pulse = 0.5 + 0.5 * (self.time * if is_planted { 6.0 } else { 2.2 }).sin();
                    let col = if is_planted { rgb(230, 60, 40) } else { rgb(220, 170, 60) };
                    let mut ring = disc(Vec3::new(site.pos.x, site.pos.y + 0.04, site.pos.z), site.radius, 20, col);
                    ring.emissive = 0.4 + 0.5 * pulse;
                    out.push(ring);
                    // a floating site letter marker (a small emissive box column)
                    let mut post = boxx(Vec3::new(site.pos.x, site.pos.y + 1.0, site.pos.z), Vec3::new(0.12, 1.0, 0.12), col, FACE_ALL);
                    post.emissive = 0.3 + 0.4 * pulse;
                    out.push(post);
                    if is_planted {
                        // the planted device: a small blinking box on the ground
                        let blink = if (self.time * 3.0).fract() < 0.5 { rgb(255, 40, 30) } else { rgb(120, 20, 15) };
                        let mut dev = boxx(Vec3::new(site.pos.x, site.pos.y + 0.22, site.pos.z), Vec3::new(0.22, 0.2, 0.16), blink, FACE_ALL);
                        dev.emissive = 0.9;
                        out.push(dev);
                    }
                }
            }
            Objective::CaptureTheFlag => {
                for (i, flag) in self.objective.flags.iter().enumerate() {
                    let team_col = if i == 0 { rgb(70, 120, 230) } else { rgb(230, 70, 60) };
                    // base pad ring at home
                    let mut pad = disc(Vec3::new(flag.home.x, flag.home.y + 0.03, flag.home.z), flag.radius, 18, team_col);
                    pad.emissive = 0.35;
                    out.push(pad);
                    // pole + banner at the flag's current position (home, carrier, or dropped)
                    let p = flag.pos;
                    let pole = tube(Vec3::new(p.x, p.y, p.z), Vec3::new(p.x, p.y + 2.2, p.z), 0.05, 4, rgb(180, 180, 180));
                    out.push(pole);
                    // banner: a bright box near the top, waving via a small time offset
                    let wav = (self.time * 3.0 + i as f32).sin() * 0.12;
                    let mut banner = boxx(Vec3::new(p.x + 0.45 + wav, p.y + 1.85, p.z), Vec3::new(0.42, 0.3, 0.02), team_col, FACE_ALL);
                    banner.emissive = 0.5;
                    out.push(banner);
                }
            }
        }
        out
    }

    pub fn frame_meshes(&self, cam: &FpsCam, third_person: bool) -> Vec<Mesh3> {
        let (right, up, _fwd) = cam.basis();
        let mut out: Vec<Mesh3> = Vec::with_capacity(self.level.meshes.len() + self.enemies.len() + 6);

        // Static level geometry — from the dynamic LOD field if this map has one (far chunks become cheap
        // impostors), otherwise the flat chunked list.
        if let Some(field) = &self.level.lod_field {
            for m in field.render_meshes() {
                out.push(m.clone());
            }
        } else {
            for m in &self.level.meshes {
                out.push(m.clone());
            }
        }
        // Animated water/lava surfaces: tessellated fresh each frame with traveling waves, per-facet normals
        // (so reflections shimmer) and foam/molten crests.
        out.extend(self.build_water_meshes());
        // Destructible objects render their current state (intact / cracked / rubble) live, so a barrier you
        // just blew apart turns to a debris pile in place.
        for d in &self.level.destructibles {
            out.extend(d.mesh());
        }
        // Persistent bullet-impact decals — scorch craters left on walls/obstacles from prior shots.
        for dc in &self.decals {
            out.push(dc.clone());
        }
        // Enemy bodies.
        for e in &self.enemies {
            out.push(e.build_mesh_tinted(Some(self.hostile_color)));
        }
        // ── Contact shadow blobs: a cheap soft dark disc on the ground under each enemy (and the player in
        // third person). Reads as a real contact shadow without a shadow-map pass — the affordable "dynamic
        // shadows" for a software renderer. Gated by the graphics preset via `shadow_blobs`.
        if self.shadow_blobs {
            for e in &self.enemies {
                if !e.alive() {
                    continue;
                }
                let gy = self.level.floor_h(e.pos.x, e.pos.z);
                // fade + shrink the blob as the body rises off the ground (jump/spawn)
                let lift = (e.pos.y - gy).max(0.0);
                let fade = (1.0 - lift / 3.0).clamp(0.0, 1.0);
                if fade <= 0.02 {
                    continue;
                }
                let r = 0.55 * (0.7 + 0.3 * fade);
                let mut blob = crate::mesh::disc(Vec3::new(e.pos.x, gy + 0.02, e.pos.z), r, 12, rgb(10, 10, 12));
                crate::mesh::set_alpha(&mut blob, (110.0 * fade) as u8);
                out.push(blob);
            }
            if third_person && self.player.alive {
                let p = self.player.pos;
                let gy = self.level.floor_h(p.x, p.z);
                let lift = (p.y - gy).max(0.0);
                let fade = (1.0 - lift / 3.0).clamp(0.0, 1.0);
                if fade > 0.02 {
                    let mut blob = crate::mesh::disc(Vec3::new(p.x, gy + 0.02, p.z), 0.6 * (0.7 + 0.3 * fade), 12, rgb(10, 10, 12));
                    crate::mesh::set_alpha(&mut blob, (120.0 * fade) as u8);
                    out.push(blob);
                }
            }
        }
        // Particles (two batched meshes).
        out.extend(self.particles.build(right, up));
        // Tracers as thin glowing tubes.
        for t in &self.tracers {
            let k = (t.life / t.max_life).clamp(0.0, 1.0);
            let col = lerp_c(rgb(20, 16, 12), t.color, k);
            let mut tube_m = tube(t.a, t.b, t.rad, 4, col);
            tube_m.emissive = 0.9 * k;
            out.push(tube_m);
        }
        // Pickups (glowing pad + spinning icon).
        for p in &self.pickups {
            out.extend(p.build());
        }
        // Deployed sentry guns.
        for s in &self.sentries {
            out.extend(build_sentry_mesh(s));
        }
        // ── Objective markers: bomb sites (S&D) and flags (CTF) ───────────────────────────────────────────
        out.extend(self.build_objective_meshes());
        // Ambient drifting motes (one emissive billboard batch).
        out.push(self.build_motes(right, up));
        // Living world: birds wheeling overhead, fireflies, glints on water.
        out.extend(self.build_ambient_life(right, up));
        // Rising/falling cover barriers (dynamic map event).
        out.extend(self.build_shutter_meshes());
        // Surge flood: a translucent lava/water sheet that climbs over the low ground during a surge event.
        if self.level.has_hazard && self.surge > 0.01 {
            let ts = self.level.ts();
            let spanx = self.level.w as f32 * ts;
            let spanz = self.level.h as f32 * ts;
            let hy = self.level.hazard_y + self.surge * SURGE_RISE;
            let (base, emit) = match self.level.cfg.theme {
                MapTheme::Volcanic => (rgb(232, 110, 42), 0.7),
                MapTheme::Toxic => (rgb(140, 188, 54), 0.3),
                _ => (rgb(52, 112, 134), 0.05),
            };
            let a = (150.0 + self.surge * 80.0).min(235.0) as u8;
            let col = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a);
            let wob = (self.time * 1.3).sin() * 0.06;
            let mut sheet = crate::mesh::boxx(
                Vec3::new(spanx * 0.5, hy + wob, spanz * 0.5),
                Vec3::new(spanx * 0.5, 0.05, spanz * 0.5),
                col,
                crate::mesh::FACE_TOP,
            );
            sheet.emissive = emit * self.surge;
            out.push(sheet);
        }
        // Possession gas: drifting translucent green haze layers hugging the ground while the event hangs.
        if self.gas > 0.01 {
            let ts = self.level.ts();
            let spanx = self.level.w as f32 * ts;
            let spanz = self.level.h as f32 * ts;
            for layer in 0..3 {
                let hy = self.level.floor_y + 1.0 + layer as f32 * 1.4 + (self.time * 0.7 + layer as f32).sin() * 0.3;
                let a = ((40.0 + self.gas * 70.0) * (1.0 - layer as f32 * 0.22)).min(150.0) as u8;
                let col = Color32::from_rgba_unmultiplied(120, 200, 70, a);
                let mut sheet = crate::mesh::boxx(
                    Vec3::new(spanx * 0.5, hy, spanz * 0.5),
                    Vec3::new(spanx * 0.5, 0.05, spanz * 0.5),
                    col,
                    crate::mesh::FACE_TOP,
                );
                sheet.emissive = 0.18 * self.gas;
                out.push(sheet);
            }
        }
        // Grenades in flight: the body colour reads the kind (olive frag, blue gravity, green acid).
        // Physics debris chunks: spinning box gibs from destroyed cover/buildings. Rotate each about its own
        // axis so it visibly tumbles; fade the last half-second of life via alpha.
        for c in &self.tumble {
            let mut m = crate::mesh::boxx(c.pos, c.half, c.color, crate::mesh::FACE_ALL);
            crate::mesh::rotate_about(&mut m, c.pos, c.spin_axis, c.ang);
            if c.life < 0.8 {
                crate::mesh::set_alpha(&mut m, (c.life / 0.8 * 255.0) as u8);
            }
            out.push(m);
        }
        for g in &self.grenades {
            let blink = if (self.time * (3.0 + (1.7 - g.fuse) * 5.0)).sin() > 0.0 { 1.0 } else { 0.25 };
            let (body_c, cap_c) = match g.kind {
                crate::class::GrenadeKind::Frag => (rgb(46, 50, 46), rgb(255, 120, 60)),
                crate::class::GrenadeKind::Gravity => (rgb(40, 60, 96), rgb(120, 200, 255)),
                crate::class::GrenadeKind::Acid => (rgb(46, 70, 40), rgb(150, 230, 90)),
            };
            let mut m = uv_sphere(g.pos, 0.11, 8, 6, body_c);
            m.emissive = 0.15 * blink;
            out.push(m);
            let mut cap = uv_sphere(g.pos + Vec3::new(0.0, 0.11, 0.0), 0.04, 6, 4, cap_c);
            cap.emissive = blink;
            out.push(cap);
        }
        // Rockets in flight: a short olive body, a bright warhead nose, oriented along the flight path.
        for r in &self.rockets {
            let dir = if r.vel.len_sq() > 1e-4 { r.vel.normalized() } else { Vec3::new(0.0, 0.0, 1.0) };
            let tail = r.pos - dir * 0.22;
            let mut body = tube(tail, r.pos, 0.045, 6, rgb(70, 72, 56));
            body.emissive = 0.05;
            out.push(body);
            let mut nose = uv_sphere(r.pos + dir * 0.05, 0.05, 6, 5, rgb(255, 140, 80));
            nose.emissive = 0.7;
            out.push(nose);
        }
        // Gravity wells: a softly pulsing translucent blue sphere with a bright ground ring.
        for z in &self.gravity_zones {
            let pulse = 0.6 + 0.4 * (self.time * 2.4).sin();
            let mut orb = uv_sphere(z.center, z.radius * 0.85, 12, 8, rgb(90, 160, 255));
            orb.emissive = 0.35 * pulse;
            crate::mesh::set_alpha(&mut orb, 46);
            out.push(orb);
            let mut ring = disc(Vec3::new(z.center.x, self.level.floor_h(z.center.x, z.center.z) + 0.05, z.center.z), z.radius, 20, rgb(120, 200, 255));
            ring.emissive = 0.5 * pulse;
            crate::mesh::set_alpha(&mut ring, 70);
            out.push(ring);
        }
        // Acid pools: a translucent green disc with a few bubbling blobs.
        for pool in &self.acid_pools {
            let gy = self.level.floor_h(pool.center.x, pool.center.z) + 0.04;
            let mut sheet = disc(Vec3::new(pool.center.x, gy, pool.center.z), pool.radius, 18, rgb(110, 200, 70));
            sheet.emissive = 0.3;
            crate::mesh::set_alpha(&mut sheet, 120);
            out.push(sheet);
            for k in 0..4 {
                let a = (self.time * 1.5 + k as f32 * 1.6).sin();
                let bx = pool.center.x + (k as f32 - 1.5) * pool.radius * 0.4;
                let bz = pool.center.z + a * pool.radius * 0.3;
                let mut blob = uv_sphere(Vec3::new(bx, gy + 0.06 + a.abs() * 0.1, bz), 0.12, 6, 4, rgb(150, 230, 90));
                blob.emissive = 0.4;
                out.push(blob);
            }
        }
        // Volumetric haze: stacked translucent depth sheets that thicken with the biome + fog events.
        out.extend(self.build_volumetric_fog(cam, right, up));
        // First-person weapon viewmodel — or, in third person, the player's customised character body.
        if third_person {
            // Drive the body with a locomotion pose from the player's real speed and crouch, so the
            // third-person figure walks/runs/crouches in step with what you're doing (matching the animated
            // enemy bodies) rather than standing frozen.
            let speed = self.player.vel.xz().len();
            let speed01 = (speed / 6.0).clamp(0.0, 1.0);
            let pose = if speed01 > 0.05 {
                crate::character::Pose::moving(self.time, speed01, self.player.crouch_t.max(self.player.prone_t))
            } else {
                crate::character::Pose::idle(self.time)
            };
            let mut body = crate::character::build_mannequin_posed(&self.cosmetics.set, pose);
            let pos = self.player.pos;
            let ang = self.player.cam.yaw + std::f32::consts::PI;
            for m in &mut body {
                crate::mesh::rotate_y_about(m, crate::math::Vec3::ZERO, ang);
                m.offset = pos;
            }
            out.extend(body);
        } else {
            out.extend(self.build_viewmodel(cam));
        }
        out
    }

    /// A layered approximation of volumetric fog: several large camera-facing translucent sheets parked at
    /// increasing depths ahead of the view. Stacked, depth-sorted translucency builds a genuine sense of
    /// atmospheric volume that the per-vertex distance fog alone can't — and it swells during fog events and
    /// in naturally hazy biomes. A ground bias makes it pool low like real ground fog.
    fn build_volumetric_fog(&self, cam: &FpsCam, right: Vec3, up: Vec3) -> Vec<Mesh3> {
        let base = match self.level.cfg.theme {
            MapTheme::Mistfall => 0.42,
            MapTheme::Volcanic => 0.20,
            MapTheme::Bunker => 0.16,
            MapTheme::Canopy => 0.18,
            MapTheme::Ice => 0.12,
            MapTheme::Toxic => 0.30,
            _ => 0.05,
        };
        let density = (base + self.fog_event * 0.75).min(1.0);
        if density < 0.04 {
            return Vec::new();
        }
        let (_r, _u, fwd) = cam.basis();
        let fog = self.fog.color;
        let t = self.time;
        // depth, relative weight (mid layers strongest)
        let layers = [(9.0, 0.5), (17.0, 0.85), (28.0, 1.0), (44.0, 0.8), (66.0, 0.55)];
        let mut out = Vec::new();
        for (i, (depth, w)) in layers.iter().enumerate() {
            let a = (density * w * 46.0).min(48.0) as u8;
            if a < 4 {
                continue;
            }
            // Drift the sheet laterally + sink it slightly below eye level so the haze pools low.
            let drift = (t * 0.15 + i as f32 * 1.7).sin() * 2.0;
            let centre = cam.pos + fwd * *depth + right * drift + up * (-1.0 - 0.02 * depth);
            let size = depth * 0.85;
            let col = Color32::from_rgba_unmultiplied(fog.r(), fog.g(), fog.b(), a);
            out.push(billboard(centre, size, right, up, col, 0.45));
        }
        out
    }

    /// The dynamic cover barriers, drawn emerging from / sinking into the floor by the current fog-event
    /// intensity so the rise reads clearly before they become solid.
    fn build_shutter_meshes(&self) -> Vec<Mesh3> {
        if self.fog_event < 0.03 {
            return Vec::new();
        }
        let rise = self.fog_event;
        let (rx, uy, fz) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let body = lerp_c(self.level.pal.wall, rgb(40, 44, 50), 0.3);
        let stripe = lerp_c(self.level.pal.accent, rgb(255, 210, 80), 0.4);
        let mut out = Vec::new();
        for s in &self.shutters {
            let drop = (1.0 - rise) * 2.0 * s.half.y;
            let c = Vec3::new(s.center.x, s.center.y - drop, s.center.z);
            out.push(obox(c, rx, uy, fz, s.half, body));
            // a hazard stripe along the top lip
            let top = Vec3::new(c.x, c.y + s.half.y - 0.08, c.z);
            out.push(obox(top, rx, uy, fz, Vec3::new(s.half.x, 0.06, s.half.z * 1.04), stripe));
        }
        out
    }

    /// Animated ambient life that makes the world feel inhabited: a flock of birds wheeling above the open
    /// biomes, fireflies hovering in the lush ones, and moving glints on water/lava. All rebuilt per frame
    /// from `self.time`, so everything drifts and pulses.
    fn build_ambient_life(&self, right: Vec3, up: Vec3) -> Vec<Mesh3> {
        let theme = self.level.cfg.theme;
        let span = self.level.cfg.size as f32 * self.level.ts();
        let t = self.time;
        let mut out = Vec::new();
        let (cx, cz) = (span * 0.5, span * 0.5);

        let outdoor = matches!(
            theme,
            MapTheme::Military | MapTheme::Urban | MapTheme::Volcanic | MapTheme::Ice | MapTheme::Alien | MapTheme::Canopy | MapTheme::Mistfall | MapTheme::Meadow | MapTheme::Redwood | MapTheme::Toxic
        );
        // Per-seed wind: a horizontal direction + strength that drifts the flying life downwind.
        let wph = (self.seed % 628) as f32 * 0.01;
        let (wdx, wdz) = (wph.cos(), wph.sin());
        let wstr = 0.6 + ((self.seed >> 7) % 100) as f32 * 0.012;
        if outdoor {
            // A loose flock circling high overhead, the whole flock sliding downwind — dark silhouettes that
            // "flap" by pulsing their size.
            let bird_col = rgb(34, 36, 44);
            let mut birds = Mesh3::new(bird_col);
            let bcx = (cx + wdx * t * wstr * 1.4).rem_euclid(span);
            let bcz = (cz + wdz * t * wstr * 1.4).rem_euclid(span);
            for i in 0..10i32 {
                let ph = i as f32 / 10.0 * std::f32::consts::TAU;
                let r = 14.0 + (i % 4) as f32 * 7.0;
                let spd = 0.22 + (i % 3) as f32 * 0.05;
                let a = t * spd + ph;
                let x = bcx + a.cos() * r;
                let z = bcz + a.sin() * r;
                let y = self.level.ceil_y + 4.0 + (t * 0.5 + ph).sin() * 1.6;
                let flap = 0.16 + (t * 7.0 + ph).sin().abs() * 0.08;
                birds.merge(&billboard(Vec3::new(x, y, z), flap, right, up, bird_col, 0.0));
            }
            out.push(birds);
        }

        if matches!(theme, MapTheme::Meadow) {
            // Butterflies fluttering low over the field, carried downwind — bright wings flickering as they flap.
            let wings = [rgb(245, 210, 90), rgb(235, 120, 90), rgb(150, 120, 220), rgb(240, 240, 240), rgb(240, 150, 80)];
            let mut flutter = Mesh3::new(wings[0]).emit(0.25);
            for i in 0..16i32 {
                let ph = i as f32 * 1.97;
                let drift_x = wdx * t * wstr * 2.4;
                let drift_z = wdz * t * wstr * 2.4;
                let x = (cx + drift_x + (t * 0.5 + ph).cos() * (10.0 + (i % 5) as f32 * 6.0) + (t * 1.7 + ph).sin() * 2.0).rem_euclid(span);
                let z = (cz + drift_z + (t * 0.42 + ph * 1.3).sin() * (10.0 + (i % 4) as f32 * 6.0) + (t * 1.9 + ph).cos() * 2.0).rem_euclid(span);
                let y = self.level.floor_h(x, z) + 0.8 + (t * 2.0 + ph).sin() * 0.4;
                let flap = (t * 9.0 + ph).sin().abs(); // 0..1 wing beat
                let sz = 0.07 + flap * 0.06;
                let col = wings[(i as usize) % wings.len()];
                flutter.merge(&billboard(Vec3::new(x, y, z), sz, right, up, col, 0.25));
            }
            out.push(flutter);
        }

        if matches!(theme, MapTheme::Canopy | MapTheme::Alien | MapTheme::Mistfall) {
            // Fireflies / spores: low hovering glow points that blink individually (per-billboard brightness).
            let base = match theme {
                MapTheme::Mistfall => rgb(150, 212, 232),
                MapTheme::Canopy => rgb(236, 206, 120),
                _ => rgb(180, 232, 120),
            };
            let mut flies = Mesh3::new(base).emit(0.9);
            for i in 0..28i32 {
                let hx = hash01(i, 11);
                let hz = hash01(i, 29);
                let hp = hash01(i, 71) * std::f32::consts::TAU;
                let x = (hx * span + (t * 0.4 + hp).sin() * 1.2).rem_euclid(span);
                let z = (hz * span + (t * 0.33 + hp * 1.7).cos() * 1.2).rem_euclid(span);
                let y = self.level.floor_y + 0.6 + (t * 0.7 + hp).sin() * 0.5 + hash01(i, 5) * 1.4;
                let blink = (0.4 + 0.6 * ((t * 2.4 + hp).sin() * 0.5 + 0.5)).clamp(0.0, 1.0);
                let col = lerp_c(rgb(0, 0, 0), base, blink);
                flies.merge(&billboard(Vec3::new(x, y, z), 0.05, right, up, col, 0.9));
            }
            out.push(flies);
        }

        if matches!(theme, MapTheme::Mistfall | MapTheme::Volcanic) {
            // Moving specular glints skating across the water/lava surface.
            let (gcol, emit) = if theme == MapTheme::Volcanic { (rgb(255, 180, 90), 1.0) } else { (rgb(200, 235, 245), 0.5) };
            let y = self.level.floor_y + self.level.min_h() - 0.45;
            let mut glints = Mesh3::new(gcol).emit(emit);
            for i in 0..22i32 {
                let hx = hash01(i, 17);
                let hz = hash01(i, 53);
                let drift = (t * 0.25 + i as f32).sin() * 4.0;
                let x = (hx * span + drift).rem_euclid(span);
                let z = (hz * span + drift * 0.7).rem_euclid(span);
                let tw = (t * 3.0 + i as f32 * 2.0).sin() * 0.5 + 0.5;
                glints.merge(&billboard(Vec3::new(x, y, z), 0.06 + 0.05 * tw, right, up, lerp_c(rgb(0, 0, 0), gcol, 0.5 + 0.5 * tw), emit));
            }
            out.push(glints);
        }

        out
    }

    /// One emissive billboard batch of slowly drifting ambient particles, tinted and animated per biome:
    /// rising embers on lava, falling snow on ice, hovering spores on the alien world, faint dust elsewhere.
    fn build_motes(&self, right: Vec3, up: Vec3) -> Mesh3 {
        let (col, size, emit, rise) = match self.level.cfg.theme {
            MapTheme::Volcanic => (rgb(255, 140, 60), 0.04, 1.0, 0.9),
            MapTheme::Ice => (rgb(225, 238, 255), 0.05, 0.5, -0.7),
            MapTheme::Alien => (rgb(170, 210, 120), 0.045, 0.7, 0.1),
            _ => (rgb(150, 160, 180), 0.03, 0.18, 0.1),
        };
        let span = self.level.cfg.size as f32 * self.level.ts();
        let head = (self.level.ceil_y - self.level.floor_y).max(1.0);
        let mut m = Mesh3::new(col);
        // During a fog event the motes swell into a drifting bank — a poor man's volumetric fog.
        let fe = self.fog_event;
        let size = size * (1.0 + 5.0 * fe);
        let emit = emit + 0.25 * fe;
        m.emissive = emit;
        let t = self.time;
        // Per-seed wind nudges the motes downwind as they drift.
        let wph = (self.seed % 628) as f32 * 0.01;
        let (wdx, wdz) = (wph.cos(), wph.sin());
        let wstr = 0.6 + ((self.seed >> 7) % 100) as f32 * 0.012;
        for (base, phase) in &self.motes {
            // drift: a slow lateral sway plus a downwind crawl, plus a vertical crawl that wraps in the headroom
            let sway = (
                (t * 0.5 + phase).sin() * 0.6 + wdx * t * wstr * 0.8,
                (t * 0.4 + phase * 1.7).cos() * 0.6 + wdz * t * wstr * 0.8,
            );
            let dy = (t * rise * 0.6 + phase).rem_euclid(head);
            let y = self.level.floor_y + 0.3 + (base.y - self.level.floor_y - 0.3 + dy).rem_euclid(head);
            let p = Vec3::new((base.x + sway.0).rem_euclid(span), y, (base.z + sway.1).rem_euclid(span));
            let bb = billboard(p, size, right, up, col, emit);
            m.merge(&bb);
        }
        m
    }

    /// A low-poly gun held in front of the camera, bobbing and kicking with the player. It rises to the
    /// centre when aiming down sights, drops to the hip when sprinting, and the sniper hides behind the
    /// scope overlay when fully aimed.
    fn build_viewmodel(&self, cam: &FpsCam) -> Vec<Mesh3> {
        let (right, up, fwd) = cam.basis();
        let def = self.player.weapon().kind.def();
        let ads = self.player.ads_t;

        // The Hunt: the player wields claws as the predator, or empty hands as an unarmed survivor.
        if self.mode == GameMode::Hunt {
            if self.hunt_role == HuntRole::Survivor {
                return Vec::new();
            }
            let bob = self.player.bob_y;
            let crouch = self.player.crouch_t;
            let col = self.creature.color();
            let dark = lerp_c(col, rgb(12, 12, 14), 0.5);
            let bone = lerp_c(col, rgb(228, 226, 220), 0.35);
            let swipe = if self.creature.strike_cd() > 0.0 { (self.strike_cd / self.creature.strike_cd()).clamp(0.0, 1.0) } else { 0.0 };
            let mut out = Vec::new();
            for side in [-1.0f32, 1.0] {
                let reach = 0.5 + swipe * 0.4; // thrust forward right after a strike
                let hand = cam.pos + fwd * reach + right * (0.32 * side) - up * (0.42 - bob * 0.5 + 0.05 * crouch);
                out.push(obox(hand - fwd * 0.12, right, up, fwd, Vec3::new(0.075, 0.075, 0.14), dark));
                for k in -1..=1 {
                    let spread = k as f32 * 0.055;
                    let tbase = hand + right * (spread * side) + up * 0.02;
                    let tip_dir = (fwd + right * (side * 0.18) + up * 0.06).normalized();
                    let tip = tbase + tip_dir * (0.22 + swipe * 0.08);
                    let mid = (tbase + tip) * 0.5;
                    out.push(obox(mid, right, up, fwd, Vec3::new(0.02, 0.02, 0.13 + swipe * 0.04), bone));
                }
            }
            return out;
        }

        // Scoped rifles vanish behind the scope overlay once you're most of the way aimed.
        if def.scoped && ads > 0.6 {
            return Vec::new();
        }

        let bob = self.player.bob_y * (1.0 - ads);
        // Cap the recoil kick that pulls the weapon toward the camera. Unclamped, heavy recoil drove the
        // viewmodel anchor inside (or behind) the near plane, so weapon triangles straddled it and smeared
        // into sharp spikes across the screen. Limiting the pull-back keeps the whole model safely in front.
        let recoil = self.player.recoil.min(0.12);
        // Hip vs aimed anchor, blended by ads; sprint lowers and cants the weapon.
        let hip = cam.pos + fwd * (0.95 - recoil * 2.5) + right * 0.26 - up * (0.30 - bob * 0.6);
        let aim = cam.pos + fwd * (0.66 - recoil * 1.6) + right * 0.012 - up * (0.075 - bob * 0.3);
        let mut base = hip.lerp(aim, ads);
        if self.player.sprinting {
            base = base + right * 0.10 - up * 0.12 + fwd * 0.05;
        }
        // Crouch lowers/pulls the weapon in; a dive drops and cants it hard; landing jolts it down.
        let crouch = self.player.crouch_t;
        let dive = (self.player.dive_t * 2.0).min(1.0);
        base = base - up * (0.06 * crouch + 0.26 * dive + 0.5 * self.player.land_dip)
            + right * (0.05 * crouch + 0.12 * dive)
            - fwd * (0.10 * dive);
        // Idle sway (a slow breathing drift), eased out as you aim.
        let t = self.time;
        base = base + right * ((t * 1.3).sin() * 0.006 * (1.0 - ads)) + up * ((t * 1.9 + 1.0).sin() * 0.005 * (1.0 - ads));
        // Reload animation: dip the weapon down and pull it back over the reload, then bring it up.
        let ws = self.player.weapon();
        if ws.reloading > 0.0 && def.reload > 0.0 {
            let p = (1.0 - ws.reloading / def.reload).clamp(0.0, 1.0);
            let dip = (p * std::f32::consts::PI).sin();
            base = base - up * (0.22 * dip) - fwd * (0.10 * dip) + right * (0.05 * dip);
        }

        let body_col = match def.kind {
            WeaponKind::Ar => rgb(64, 66, 72),
            WeaponKind::Smg => rgb(58, 60, 66),
            WeaponKind::Shotgun => rgb(78, 58, 40),
            WeaponKind::Sniper => rgb(52, 56, 64),
            WeaponKind::Lmg => rgb(60, 58, 52),
            WeaponKind::Dmr => rgb(60, 56, 48),
            WeaponKind::Pistol => rgb(54, 56, 62),
            WeaponKind::EmSword => rgb(40, 60, 80),
            WeaponKind::Launcher => rgb(70, 72, 56),
            WeaponKind::ArcCoil => rgb(40, 66, 90),
            WeaponKind::CryoLance => rgb(48, 74, 92),
            WeaponKind::VoidMaul => rgb(56, 40, 72),
        };
        // Weapon skin tints the body toward the chosen finish.
        let body_col = match self.cosmetics.skin.tint() {
            Some(t) => lerp_c(body_col, t, 0.7),
            None => body_col,
        };
        // A legendary primary stamps the weapon with its signature colour (its "unique appearance").
        let body_col = match self.player.loadout.legend() {
            Some(lg) if self.player.current == 0 || self.player.current == 2 => lerp_c(body_col, lg.accent.color(), 0.45),
            _ => body_col,
        };
        let metal = lerp_c(body_col, rgb(18, 18, 22), 0.45);

        let mut out = Vec::new();
        // A gloved hand at the grip, tinted by the armor frame's colour — palm block plus a thumb wrap so it
        // reads as a hand gripping, not a cube.
        let glove = self.cosmetics.armor_color.color();
        let glove_dark = lerp_c(glove, rgb(20, 20, 24), 0.35);
        out.push(obox(base - fwd * 0.13 - up * 0.06, right, up, fwd, Vec3::new(0.05, 0.06, 0.07), glove_dark));
        // thumb wrapping over the top of the grip
        out.push(obox(base - fwd * 0.11 - up * 0.02 + right * 0.02, right, up, fwd, Vec3::new(0.028, 0.028, 0.06), glove_dark));
        // support hand on the foregrip (further forward)
        out.push(obox(base + fwd * 0.12 - up * 0.055, right, up, fwd, Vec3::new(0.045, 0.05, 0.06), glove_dark));
        // Receiver/body block.
        out.push(obox(base, right, up, fwd, Vec3::new(0.07, 0.07, 0.20), body_col));
        // upper rail / handguard along the top of the receiver
        out.push(obox(base + up * 0.05, right, up, fwd, Vec3::new(0.045, 0.02, 0.22), lerp_c(body_col, rgb(0, 0, 0), 0.25)));
        // ejection port detail on the right side
        out.push(obox(base + right * 0.06 + fwd * 0.03, right, up, fwd, Vec3::new(0.015, 0.03, 0.06), metal));
        // Grip angling down-back.
        let grip_c = base - fwd * 0.13 - up * 0.12;
        out.push(obox(grip_c, right, up, fwd, Vec3::new(0.045, 0.11, 0.05), metal));
        // trigger guard loop under the receiver
        out.push(obox(base - up * 0.075 - fwd * 0.02, right, up, fwd, Vec3::new(0.03, 0.02, 0.05), metal));
        // Barrel out the front.
        let barrel_a = base + fwd * 0.18;
        let barrel_b = base + fwd * (0.18 + barrel_len(def.kind));
        out.push(tube(barrel_a, barrel_b, barrel_rad(def.kind), 6, metal));

        match def.kind {
            WeaponKind::Sniper => {
                // long body, a scope tube sitting on top
                let scope_c = base + up * 0.085 + fwd * 0.02;
                out.push(tube(scope_c - fwd * 0.07, scope_c + fwd * 0.07, 0.028, 8, rgb(24, 24, 28)));
                out.push(obox(scope_c, right, up, fwd, Vec3::new(0.02, 0.03, 0.02), metal));
            }
            WeaponKind::Ar => {
                // a low-profile optic, a curved magazine, and a collapsible stock
                out.push(obox(base + up * 0.07 + fwd * 0.02, right, up, fwd, Vec3::new(0.022, 0.03, 0.05), rgb(26, 26, 30)));
                out.push(obox(base - up * 0.11 + fwd * 0.02, right, up, fwd, Vec3::new(0.035, 0.07, 0.05), metal)); // magazine
                out.push(obox(base - fwd * 0.22, right, up, fwd, Vec3::new(0.04, 0.045, 0.09), body_col)); // stock
            }
            WeaponKind::Smg => {
                // stubby suppressor + a long magazine
                out.push(tube(barrel_b, barrel_b + fwd * 0.07, 0.035, 8, rgb(30, 30, 34)));
                out.push(obox(base - up * 0.12 - fwd * 0.02, right, up, fwd, Vec3::new(0.03, 0.08, 0.04), metal));
            }
            WeaponKind::Shotgun => {
                // under-barrel pump tube
                out.push(tube(barrel_a - up * 0.035, barrel_b - up * 0.035, 0.026, 6, metal));
            }
            WeaponKind::Lmg => {
                // box magazine under the receiver
                out.push(obox(base - up * 0.13 - fwd * 0.02, right, up, fwd, Vec3::new(0.06, 0.08, 0.07), metal));
            }
            WeaponKind::Dmr => {
                // marksman optic, magazine, and a full stock
                out.push(obox(base + up * 0.08 + fwd * 0.02, right, up, fwd, Vec3::new(0.022, 0.032, 0.06), rgb(24, 24, 28)));
                out.push(obox(base - up * 0.11 + fwd * 0.01, right, up, fwd, Vec3::new(0.032, 0.06, 0.045), metal));
                out.push(obox(base - fwd * 0.24, right, up, fwd, Vec3::new(0.04, 0.05, 0.1), body_col));
            }
            WeaponKind::Pistol => {}
            WeaponKind::EmSword => {
                // a glowing energy blade extending forward from the hilt
                out.push(obox(base + fwd * 0.34, right, up, fwd, Vec3::new(0.015, 0.05, 0.34), rgb(120, 200, 255)));
            }
            WeaponKind::Launcher => {
                // a bulbous warhead tip past the muzzle, and a rear counterweight/stock
                out.push(uv_sphere(barrel_b + fwd * 0.03, 0.055, 8, 6, rgb(200, 90, 60)));
                out.push(obox(base - fwd * 0.22, right, up, fwd, Vec3::new(0.05, 0.05, 0.1), metal));
            }
            WeaponKind::ArcCoil => {
                // twin tesla coils flanking the barrel, capped with glowing electrode spheres
                for s in [-1.0f32, 1.0] {
                    let coil = base + right * (0.05 * s) + up * 0.04 + fwd * 0.1;
                    out.push(tube(coil, coil + fwd * 0.12, 0.02, 6, rgb(60, 90, 120)));
                    let mut node = uv_sphere(coil + fwd * 0.14, 0.032, 7, 5, rgb(150, 220, 255));
                    node.emissive = 0.9;
                    out.push(node);
                }
                // a charged core between them
                let mut core = uv_sphere(base + fwd * 0.16 + up * 0.02, 0.03, 7, 5, rgb(190, 235, 255));
                core.emissive = 1.0;
                out.push(core);
            }
            WeaponKind::CryoLance => {
                // a long crystalline emitter with a frosted lens; a cold glowing tip
                out.push(obox(base + up * 0.05 + fwd * 0.05, right, up, fwd, Vec3::new(0.02, 0.03, 0.16), rgb(120, 180, 210)));
                let mut lens = uv_sphere(barrel_b + fwd * 0.02, 0.045, 8, 6, rgb(200, 245, 255));
                lens.emissive = 0.7;
                out.push(lens);
            }
            WeaponKind::VoidMaul => {
                // a heavy void-forged hammer head on a short haft, edged with violet energy
                out.push(obox(base + fwd * 0.36, right, up, fwd, Vec3::new(0.09, 0.11, 0.1), rgb(40, 30, 54)));
                let mut edge = obox(base + fwd * 0.36 + up * 0.11, right, up, fwd, Vec3::new(0.1, 0.02, 0.11), rgb(170, 90, 220));
                edge.emissive = 0.8;
                out.push(edge);
            }
        }
        out
    }

    /// Scale that maps world units to pixels for the given viewport, plus the shake offset to apply
    /// to the screen centre this frame.
    pub fn shake_offset(&self) -> (f32, f32) {
        if self.shake <= 0.0 {
            return (0.0, 0.0);
        }
        let t = self.time * 60.0;
        let s = self.shake * 6.0;
        ((t * 1.7).sin() * s, (t * 2.3).cos() * s)
    }

    pub fn enemies_alive(&self) -> usize {
        self.enemies.iter().filter(|e| e.alive()).count()
    }
}

/// Pick a handful of larger rooms and lay out a cover barrier across each — the footprints that rise from
/// the floor during a fog bank and lower when it clears, reshaping sightlines mid-match.
fn build_shutters(level: &Level) -> Vec<Obstacle> {
    let ts = level.ts();
    let mut out = Vec::new();
    for (i, room) in level.rooms.iter().enumerate() {
        if i == 0 || room.rect.w() < 7 || room.rect.h() < 7 {
            continue;
        }
        if out.len() >= 7 {
            break;
        }
        // A low wall spanning most of the room width, sitting a little off-centre.
        let half_w = (room.rect.w() as f32 * ts) * 0.32;
        let cx = room.center.x;
        let cz = room.center.z + (room.rect.h() as f32 * ts) * 0.18 * if i % 2 == 0 { 1.0 } else { -1.0 };
        out.push(Obstacle {
            center: Vec3::new(cx, room.floor_y + 1.1, cz),
            half: Vec3::new(half_w, 1.1, 0.45),
        });
    }
    out
}

/// Which dynamic map event is running. Fog rolls a bank in and raises cover; Blackout cuts the lights;
/// Surge floods the lava/water up over the low ground, forcing a scramble to high cover.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EventKind {
    Fog,
    Blackout,
    Surge,
    PossessionGas,
}

/// How far the hazard surface climbs at peak surge intensity (world units).
const SURGE_RISE: f32 = 3.2;

/// A lobbed grenade: ballistic flight then a proximity blast. `from_enemy` decides kill credit (a player
/// frag that kills an enemy scores; an enemy frag never credits the player), but the blast itself is
/// indiscriminate — enemy grenades can catch their own squad.
struct Grenade {
    pos: Vec3,
    vel: Vec3,
    fuse: f32,
    from_enemy: bool,
    kind: crate::class::GrenadeKind,
}
const GRENADE_GRAV: f32 = 17.0;
const GRENADE_BLAST: f32 = 7.2; // bigger, punchier frag radius (was 4.6) — grenades now clear a real area

/// A physics debris chunk: a spinning box gib flung from destroyed cover or a collapsing building. Integrates
/// under gravity, spins on a random axis, bounces off the floor with damping, and fades out once settled.
/// Not a true rigid body (no chunk-vs-chunk collision) but enough to sell "the building tumbled apart".
struct TumbleChunk {
    pos: Vec3,
    vel: Vec3,
    half: Vec3,
    color: egui::Color32,
    /// current rotation (radians) about `spin_axis`, and the angular speed
    ang: f32,
    ang_vel: f32,
    spin_axis: Vec3,
    life: f32,
    rest: bool, // settled on the ground → stop integrating, just linger then fade
}

/// A launcher round in flight. Light gravity (mostly flat trajectory, the way a real rocket reads at combat
/// ranges) and a max-range self-detonate so a rocket that flies over the map's edge doesn't just vanish or
/// fly forever — it always ends in a blast somewhere.
struct Rocket {
    pos: Vec3,
    vel: Vec3,
    traveled: f32,
    max_range: f32,
}
const ROCKET_GRAV: f32 = 4.0;
const ROCKET_BLAST: f32 = 5.2;
const ROCKET_SPEED: f32 = 46.0;

/// Scale a freshly populated wave's health by the difficulty multiplier.
fn scale_enemy_health(enemies: &mut [Enemy], mul: f32) {
    for e in enemies.iter_mut() {
        e.health *= mul;
    }
}

fn barrel_len(k: WeaponKind) -> f32 {
    match k {
        WeaponKind::Ar => 0.30,
        WeaponKind::Smg => 0.18,
        WeaponKind::Shotgun => 0.30,
        WeaponKind::Sniper => 0.42,
        WeaponKind::Lmg => 0.34,
        WeaponKind::Dmr => 0.36,
        WeaponKind::Pistol => 0.12,
        WeaponKind::EmSword => 0.40,
        WeaponKind::Launcher => 0.34,
        WeaponKind::ArcCoil => 0.28,
        WeaponKind::CryoLance => 0.46,
        WeaponKind::VoidMaul => 0.44,
    }
}
fn barrel_rad(k: WeaponKind) -> f32 {
    match k {
        WeaponKind::Ar => 0.028,
        WeaponKind::Smg => 0.03,
        WeaponKind::Shotgun => 0.05,
        WeaponKind::Sniper => 0.026,
        WeaponKind::Lmg => 0.032,
        WeaponKind::Dmr => 0.026,
        WeaponKind::Pistol => 0.024,
        WeaponKind::EmSword => 0.02,
        WeaponKind::Launcher => 0.06,
        WeaponKind::ArcCoil => 0.03,
        WeaponKind::CryoLance => 0.024,
        WeaponKind::VoidMaul => 0.03,
    }
}

/// An oriented box: a cuboid whose axes are arbitrary unit vectors (so it turns with the view).
/// A small tripod auto-turret: legs to a hub, a yawing body with twin barrels, and a glowing eye.
fn build_sentry_mesh(s: &Sentry) -> Vec<Mesh3> {
    let fwd = Vec3::new(s.yaw.sin(), 0.0, -s.yaw.cos());
    let right = Vec3::new(fwd.z, 0.0, -fwd.x);
    let up = Vec3::UP;
    let metal = rgb(74, 78, 86);
    let dark = rgb(40, 42, 48);
    let mut out = Vec::new();
    let hub = s.pos + up * 0.5;
    for k in 0..3 {
        let a = (k as f32) * 2.094_395 + 0.4;
        let foot = s.pos + Vec3::new(a.cos() * 0.32, 0.0, a.sin() * 0.32);
        out.push(tube(foot, hub, 0.04, 4, dark));
    }
    out.push(disc(s.pos + up * 0.02, 0.3, 8, dark));
    let body = s.pos + up * 0.62;
    out.push(obox(body, right, up, fwd, Vec3::new(0.2, 0.13, 0.24), metal));
    out.push(tube(body + fwd * 0.2, body + fwd * 0.6, 0.04, 5, dark));
    out.push(tube(body + fwd * 0.2 + up * 0.06, body + fwd * 0.55 + up * 0.06, 0.03, 5, dark));
    let mut eye = obox(body + fwd * 0.22, right, up, fwd, Vec3::new(0.05, 0.05, 0.03), rgb(255, 90, 60));
    eye.emissive = 0.9;
    out.push(eye);
    out
}

fn obox(c: Vec3, right: Vec3, up: Vec3, fwd: Vec3, half: Vec3, color: Color32) -> Mesh3 {
    let rx = right * half.x;
    let uy = up * half.y;
    let fz = fwd * half.z;
    // 8 corners.
    let p = |sx: f32, sy: f32, sz: f32| c + rx * sx + uy * sy + fz * sz;
    let v = [
        p(-1.0, -1.0, -1.0),
        p(1.0, -1.0, -1.0),
        p(1.0, 1.0, -1.0),
        p(-1.0, 1.0, -1.0),
        p(-1.0, -1.0, 1.0),
        p(1.0, -1.0, 1.0),
        p(1.0, 1.0, 1.0),
        p(-1.0, 1.0, 1.0),
    ];
    let mut m = Mesh3::new(color);
    let mut quad = |a: usize, b: usize, cc: usize, d: usize, n: Vec3| {
        m.push_quad(v[a], v[b], v[cc], v[d], n, color);
    };
    quad(0, 3, 2, 1, -fwd.normalized()); // back
    quad(4, 5, 6, 7, fwd.normalized()); // front
    quad(0, 1, 5, 4, -up.normalized()); // bottom
    quad(3, 7, 6, 2, up.normalized()); // top
    quad(0, 4, 7, 3, -right.normalized()); // left
    quad(1, 2, 6, 5, right.normalized()); // right
    m
}

/// Jitter a direction inside a cone of half-angle `spread` radians.
fn jitter(dir: Vec3, spread: f32, rng: &mut Rng) -> Vec3 {
    if spread <= 0.0 {
        return dir;
    }
    let up = if dir.y.abs() > 0.95 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::UP };
    let right = dir.cross(up).normalized();
    let realup = right.cross(dir).normalized();
    let a = (rng.f32() - 0.5) * 2.0 * spread;
    let b = (rng.f32() - 0.5) * 2.0 * spread;
    (dir + right * a + realup * b).normalized()
}

// keep clampf referenced for future tuning hooks
#[allow(dead_code)]
fn _touch(x: f32) -> f32 {
    clampf(x, 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_fire() -> Input {
        Input { fire: true, ..Default::default() }
    }

    #[test]
    fn hunt_mode_strips_the_jetpack_perk() {
        // Bringing a jetpack into The Hunt unbalanced it — hunt should disable the perk regardless of loadout.
        let mut lo = Loadout::default();
        lo.toggle_perk(crate::loadout::Perk::Jetpack);
        assert!(lo.jetpack(), "loadout should have the jetpack for this test");
        // standard match keeps it
        let g_std = Game::new_match(1, None, lo, Cosmetics::default(), 8, Difficulty::Regular, None, HuntRole::Predator);
        assert!(g_std.player.jetpack, "standard mode keeps the jetpack");
        // hunt match strips it
        let g_hunt = Game::new_match(1, None, lo, Cosmetics::default(), 8, Difficulty::Regular, Some(Creature::Revenant), HuntRole::Predator);
        assert!(!g_hunt.player.jetpack, "hunt mode should strip the jetpack");
    }

    #[test]
    fn decals_accumulate_and_respect_the_cap() {
        let mut g = Game::new(0);
        assert!(g.decals.is_empty(), "a fresh game should start with no decals");
        for i in 0..200 {
            g.spawn_decal(Vec3::new(i as f32, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        }
        assert_eq!(g.decals.len(), 140, "decal pool should cap at the fixed budget, not grow unbounded");
        // every spawned decal should be real, non-degenerate geometry
        for d in &g.decals {
            assert!(d.tri_count() > 0, "a decal should have geometry");
            for tri in &d.tris {
                let a = d.verts[tri[0] as usize];
                let b = d.verts[tri[1] as usize];
                let c = d.verts[tri[2] as usize];
                let area = (b - a).cross(c - a).len() * 0.5;
                assert!(area > 1e-7, "bullet_decal produced a degenerate triangle");
            }
        }
    }

    #[test]
    fn a_rocket_splash_damages_a_nearby_enemy_on_detonation() {
        use crate::loadout::Loadout;
        // Exercise `update_rockets` directly rather than the full aim/fire pipeline — a rocket's flight path
        // depends on the player's exact look angle intersecting an enemy's hit-sphere (itself offset from
        // `pos` by the enemy kind's height), which is a lot of incidental geometry to get exactly right in a
        // test of what's actually new here: splash damage and impact/max-range detonation.
        let mut loadout = Loadout::default();
        loadout.primary = crate::weapons::WeaponKind::Launcher;
        let mut g = Game::new_with(0, None, loadout);
        assert!(!g.enemies.is_empty());
        let center = g.enemies[0].center();
        let hp_before = g.enemies[0].health;
        // A rocket 3 units to the side of the enemy, flying straight at it with just enough max_range to
        // reach roughly that point — self-detonates almost on top of the enemy, well inside ROCKET_BLAST.
        g.rockets.push(Rocket { pos: center - Vec3::new(3.0, 0.0, 0.0), vel: Vec3::new(ROCKET_SPEED, 0.0, 0.0), traveled: 0.0, max_range: 2.9 });
        for _ in 0..10 {
            g.update_rockets(1.0 / 60.0);
        }
        assert!(g.rockets.is_empty(), "the rocket should have detonated (impact or max-range) and been removed");
        assert!(g.enemies[0].health < hp_before || !g.enemies[0].alive(), "an enemy standing where the rocket detonated should take splash damage");
    }

    #[test]
    fn arc_coil_chains_lightning_to_a_clustered_second_enemy() {
        use crate::loadout::Loadout;
        let mut loadout = Loadout::default();
        loadout.primary = crate::weapons::WeaponKind::ArcCoil;
        let mut g = Game::new_with(0, None, loadout);
        // flat spawn ground: put the primary enemy a few units ahead and aim right at it (proven pattern)
        let base = g.player.pos;
        g.enemies[0].pos = base + Vec3::new(0.0, 0.0, -6.0);
        // a second enemy clustered near the first, within a chain jump
        g.enemies[1].pos = base + Vec3::new(1.6, 0.0, -7.0);
        for e in g.enemies.iter_mut().take(2) { e.health = 500.0; e.state = crate::enemy::EnemyState::Idle; }
        let e_center = g.enemies[0].center();
        let to = e_center - g.player.eye();
        g.player.cam.yaw = to.x.atan2(-to.z);
        g.player.cam.pitch = (to.y / to.len()).asin().clamp(-1.0, 1.0);
        let h1_before = g.enemies[1].health;
        g.player.weapon_mut().cooldown = 0.0;
        g.fire();
        assert!(g.enemies[0].health < 500.0, "primary target should be hit");
        assert!(g.enemies[1].health < h1_before, "arc coil should chain to the nearby second enemy");
    }

    #[test]
    fn cryo_lance_pierces_two_enemies_in_a_line() {
        use crate::loadout::Loadout;
        let mut loadout = Loadout::default();
        loadout.primary = crate::weapons::WeaponKind::CryoLance;
        let mut g = Game::new_with(0, None, loadout);
        let base = g.player.pos;
        // two enemies directly in line ahead, one behind the other
        g.enemies[0].pos = base + Vec3::new(0.0, 0.0, -6.0);
        g.enemies[1].pos = base + Vec3::new(0.0, 0.0, -11.0);
        for e in g.enemies.iter_mut().take(2) { e.health = 500.0; e.state = crate::enemy::EnemyState::Idle; }
        let e_center = g.enemies[0].center();
        let to = e_center - g.player.eye();
        g.player.cam.yaw = to.x.atan2(-to.z);
        g.player.cam.pitch = (to.y / to.len()).asin().clamp(-1.0, 1.0);
        g.player.weapon_mut().cooldown = 0.0;
        g.fire();
        assert!(g.enemies[0].health < 500.0, "first enemy on the line should be hit");
        assert!(g.enemies[1].health < 500.0, "cryo lance should pierce through to the second");
    }

    #[test]
    fn each_biome_gets_its_own_signature_live_event() {
        // Different themes should map to different signature events, and an event should actually fire over time.
        assert_eq!(MapEvent::for_theme(MapTheme::Volcanic), MapEvent::EmberStorm);
        assert_eq!(MapEvent::for_theme(MapTheme::Ice), MapEvent::Blizzard);
        assert_eq!(MapEvent::for_theme(MapTheme::Toxic), MapEvent::GasSurge);
        assert_eq!(MapEvent::for_theme(MapTheme::Station), MapEvent::SolarFlare);
        assert_eq!(MapEvent::for_theme(MapTheme::Carrier), MapEvent::Squall);
        // banners are non-empty for real events
        assert!(!MapEvent::EmberStorm.banner().is_empty());
        assert!(MapEvent::None.banner().is_empty());
        // drive a volcanic map forward and confirm its event eventually becomes active
        let mut g = Game::new_with(3, Some(MapTheme::Volcanic), Loadout::default());
        assert_eq!(g.map_event(), MapEvent::EmberStorm);
        let mut saw_active = false;
        for _ in 0..3000 {
            g.step(&Input::default(), 1.0 / 30.0);
            if g.map_event_intensity() > 0.0 {
                saw_active = true;
                break;
            }
        }
        assert!(saw_active, "the ember storm should fire within a reasonable window");
    }

    #[test]
    fn a_fired_rocket_actually_leaves_the_muzzle_and_is_tracked() {
        // A lighter end-to-end check than the splash test above: firing the launcher should spawn a tracked
        // in-flight rocket at all (the fire() → rockets.push() wiring), without asserting on exact aim.
        use crate::loadout::Loadout;
        let mut loadout = Loadout::default();
        loadout.primary = crate::weapons::WeaponKind::Launcher;
        let mut g = Game::new_with(0, None, loadout);
        g.player.weapon_mut().cooldown = 0.0;
        let input = Input { fire: true, ..Default::default() };
        g.step(&input, 1.0 / 60.0);
        assert_eq!(g.rockets.len(), 1, "firing the launcher should spawn exactly one tracked rocket");
        assert!(g.rockets[0].vel.len() > 1.0, "the rocket should actually be moving, not spawned at rest");
    }

    #[test]
    fn a_destroyed_barrel_detonates_hurting_nearby_enemies_and_chaining_to_its_neighbour() {
        use crate::destructible::{Destructible, DestructibleKind};
        let mut g = Game::new(0);
        // plant two drums close enough to chain, with an enemy standing next to them
        let p = g.player.pos;
        let b0 = Vec3::new(p.x + 30.0, 0.5, p.z + 30.0); // far from the player so the blast can't hurt them
        let b1 = b0 + Vec3::new(1.2, 0.0, 0.3);
        g.level.destructibles.push(Destructible::new(DestructibleKind::ExplosiveBarrel, b0, Vec3::new(0.34, 0.5, 0.34), 1));
        g.level.destructibles.push(Destructible::new(DestructibleKind::ExplosiveBarrel, b1, Vec3::new(0.34, 0.5, 0.34), 2));
        let i0 = g.level.destructibles.len() - 2;
        let i1 = g.level.destructibles.len() - 1;
        // park an enemy in the blast radius and note its health
        assert!(!g.enemies.is_empty());
        g.enemies[0].pos = b0 + Vec3::new(1.5, 0.0, 0.0);
        let hp_before = g.enemies[0].health;
        // "shoot" the first barrel to destruction, then let the game tick
        g.level.destructibles[i0].damage(999.0);
        let input = Input::default();
        for _ in 0..12 {
            g.step(&input, 1.0 / 60.0);
        }
        assert!(g.level.destructibles[i0].detonated, "the destroyed drum should have detonated");
        assert!(!g.level.destructibles[i1].blocks(), "its neighbour should be destroyed by the blast");
        assert!(g.level.destructibles[i1].detonated, "…and should have chain-detonated in a later frame");
        assert!(g.enemies[0].health < hp_before || !g.enemies[0].alive(), "an enemy beside the drums should catch the blast");
    }

    #[test]
    fn desert_town_rooftops_are_walk_reachable_via_external_stairs() {
        use crate::level::PRESETS;
        // Sample every ~0.58m (finer than the 0.68m stair tread, so no step gets skipped), read the top
        // standable surface at each column, then flood-fill from ground level allowing rises up to the
        // player's 0.65 step limit. A healthy majority of buildings must end with walk-up roof access —
        // that's the difference between rooftops as decoration and rooftops as a traversal lane.
        let p = PRESETS.iter().find(|p| p.name == "Desert Town").unwrap();
        let l = crate::level::Level::generate(p.config(7));
        let ts = l.ts();
        let world = l.w as f32 * ts;
        let n = (world / 0.58) as usize;
        let step = world / n as f32;
        let mut h = vec![f32::NEG_INFINITY; n * n];
        for j in 0..n {
            for i in 0..n {
                h[j * n + i] = l.support_y(i as f32 * step + step * 0.5, j as f32 * step + step * 0.5, 50.0);
            }
        }
        let mut reach = vec![false; n * n];
        let mut q = std::collections::VecDeque::new();
        for k in 0..n * n {
            if h[k].abs() < 0.2 {
                reach[k] = true;
                q.push_back(k);
            }
        }
        while let Some(k) = q.pop_front() {
            let (i, j) = (k % n, k / n);
            for (di, dj) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (ni, nj) = (i as i32 + di, j as i32 + dj);
                if ni < 0 || nj < 0 || ni >= n as i32 || nj >= n as i32 {
                    continue;
                }
                let nk = nj as usize * n + ni as usize;
                if !reach[nk] && h[nk] - h[k] <= 0.66 {
                    reach[nk] = true;
                    q.push_back(nk);
                }
            }
        }
        // bucket roof cells into ~building-sized clusters and count how many have any reachable cell
        let mut have = std::collections::HashSet::new();
        let mut have_reach = std::collections::HashSet::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * n + i;
                if h[k] > 5.8 {
                    let key = ((i as f32 * step / 18.0) as i32, (j as f32 * step / 18.0) as i32);
                    have.insert(key);
                    if reach[k] {
                        have_reach.insert(key);
                    }
                }
            }
        }
        assert!(!have.is_empty(), "the town should have rooftops");
        let frac = have_reach.len() as f32 / have.len() as f32;
        assert!(frac >= 0.5, "at least half the roof clusters should be walk-reachable via stairs (got {:.0}%)", frac * 100.0);
    }

    #[test]
    fn a_new_game_has_a_level_player_and_enemies() {
        let g = Game::new(0);
        assert!(!g.level.meshes.is_empty());
        assert!(g.player.alive);
        assert!(!g.enemies.is_empty());
    }

    #[test]
    fn frame_meshes_include_level_enemies_and_viewmodel() {
        let g = Game::new(0);
        let cam = g.player.view_cam();
        let meshes = g.frame_meshes(&cam, false);
        // At least: level meshes + one enemy + a viewmodel piece.
        assert!(meshes.len() > g.level.meshes.len());
        let tris: usize = meshes.iter().map(|m| m.tri_count()).sum();
        assert!(tris > 50);
    }

    #[test]
    fn search_and_destroy_runs_through_the_game_loop() {
        use crate::objectives::{Objective, SndPhase};
        let mut g = Game::new(1);
        g.set_search_destroy();
        assert_eq!(g.objective.mode, Objective::SearchDestroy);
        assert_eq!(g.objective.sites.len(), 2);
        // teleport the player onto site A and hold the plant action (reload key) via the step loop
        let site_a = g.objective.sites[0].pos;
        g.player.pos = site_a;
        let plant_input = Input { reload: true, ..Default::default() };
        for _ in 0..300 {
            g.player.pos = site_a; // keep them on the site
            g.step(&plant_input, 1.0 / 60.0);
            if g.objective.snd_phase == SndPhase::Planted {
                break;
            }
        }
        assert_eq!(g.objective.snd_phase, SndPhase::Planted, "planting through the game loop should arm the bomb");
        // markers should be present in the frame
        let cam = g.player.view_cam();
        let meshes = g.frame_meshes(&cam, false);
        assert!(meshes.len() > g.level.meshes.len(), "objective markers should add meshes");
    }

    #[test]
    fn capture_the_flag_sets_up_two_bases() {
        use crate::objectives::Objective;
        let mut g = Game::new(2);
        g.set_capture_the_flag();
        assert_eq!(g.objective.mode, Objective::CaptureTheFlag);
        // the two flag homes should be meaningfully apart
        let a = g.objective.flags[0].home;
        let b = g.objective.flags[1].home;
        let d = ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt();
        assert!(d > 8.0, "the two flag bases should be well separated (got {d:.1})");
    }

    #[test]
    fn a_deployed_sentry_shoots_enemies() {
        let mut g = Game::new(0);
        let base = g.player.pos;
        g.enemies.truncate(1);
        g.enemies[0].pos = base + Vec3::new(0.0, 0.0, -6.0);
        g.sentries.push(Sentry { pos: base + Vec3::new(0.0, 0.0, -3.0), yaw: 0.0, cooldown: 0.0, life: 10.0, ammo: 40 });
        let hp0 = g.enemies[0].health;
        for _ in 0..30 {
            g.step(&Input::default(), 1.0 / 30.0);
        }
        assert!(g.enemies[0].health < hp0, "a sentry should damage a nearby enemy");
    }

    #[test]
    fn firing_consumes_ammo_and_starts_cooldown() {
        let mut g = Game::new(0);
        let mag0 = g.player.weapon().mag;
        g.step(&input_fire(), 1.0 / 60.0);
        assert_eq!(g.player.weapon().mag, mag0 - 1);
        assert!(g.player.weapon().cooldown > 0.0);
    }

    #[test]
    fn a_point_blank_shot_into_an_enemy_damages_it() {
        let mut g = Game::new(0);
        // The spawn room is flat tier-0 ground (Industrial has no terrain noise). Put an enemy a few
        // units in front of the spawned player there and aim at it.
        let base = g.player.pos;
        g.enemies[0].pos = base + Vec3::new(0.0, 0.0, -3.0);
        let e_center = g.enemies[0].center();
        let to = e_center - g.player.eye();
        g.player.cam.yaw = to.x.atan2(-to.z);
        g.player.cam.pitch = (to.y / to.len()).asin().clamp(-1.0, 1.0);
        let hp0 = g.enemies[0].health;
        g.step(&input_fire(), 1.0 / 60.0);
        assert!(g.enemies[0].health < hp0, "enemy should have taken damage");
    }

    #[test]
    fn clearing_all_enemies_advances_the_wave() {
        let mut g = Game::new(0);
        // Kill them all instantly.
        for e in &mut g.enemies {
            e.damage(10_000.0);
        }
        // Let their death animations finish so they get reaped and a new wave spawns.
        for _ in 0..200 {
            g.step(&Input::default(), 1.0 / 60.0);
            if g.wave > 1 {
                break;
            }
        }
        assert!(g.wave > 1, "wave should have advanced after a clear");
        assert!(!g.enemies.is_empty(), "next wave should have spawned");
    }
#[test]
fn firing_queues_sound_events() {
    use crate::game::{Game, Sfx};
    use crate::level::{Level, MapConfig, MapTheme};
    use crate::player::Input;
    let lvl = Level::generate(MapConfig { seed: 1, theme: MapTheme::Industrial, size: 120, ..Default::default() });
    let mut g = Game::from_level(lvl, 1, Default::default(), Default::default(), 0, crate::enemy::Difficulty::Regular, None, crate::creatures::HuntRole::Predator);
    let mut input = Input::default();
    input.fire = true;
    for _ in 0..6 { g.step(&input, 1.0 / 60.0); }
    let events = g.take_sfx();
    assert!(events.iter().any(|e| matches!(e, Sfx::Shot(_))), "firing should queue a Shot");
    assert!(g.take_sfx().is_empty(), "take_sfx drains the queue");
}

    fn game_with_class(c: crate::class::ClassKind) -> Game {
        let mut l = crate::loadout::Loadout::default();
        l.class = c;
        Game::from_level(
            crate::level::Level::generate(crate::level::MapConfig::default()),
            1, l, Default::default(), 8,
            crate::enemy::Difficulty::Regular, None, crate::creatures::HuntRole::Predator,
        )
    }

    #[test]
    fn tank_has_a_huge_health_pool_assassin_a_small_one() {
        let tank = game_with_class(crate::class::ClassKind::Tank);
        let ass = game_with_class(crate::class::ClassKind::Assassin);
        assert!(tank.player.max_health > 200.0 && (tank.player.health - tank.player.max_health).abs() < 0.1);
        assert!(ass.player.max_health < 100.0);
    }

    #[test]
    fn frag_grenade_destroys_cover_and_flings_tumble_debris() {
        // A frag detonating on a crate should now (with the boosted destructible damage) destroy it and spawn
        // physics debris chunks that tumble — the "building tumble" effect.
        let mut g = game_with_class(crate::class::ClassKind::Trooper);
        let spot = g.player.pos + Vec3::new(0.0, 0.0, -3.0);
        g.level.destructibles.clear();
        g.level.destructibles.push(crate::destructible::Destructible::new(
            crate::destructible::DestructibleKind::CrateStack,
            spot + Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(0.6, 0.6, 0.6),
            1,
        ));
        g.grenades.push(Grenade { pos: spot + Vec3::new(0.0, 0.4, 0.0), vel: Vec3::ZERO, fuse: 0.03, from_enemy: false, kind: crate::class::GrenadeKind::Frag });
        for _ in 0..4 {
            g.step(&Input::default(), 1.0 / 60.0);
        }
        assert!(!g.level.destructibles[0].blocks(), "the crate should be destroyed by the frag");
        assert!(!g.tumble.is_empty(), "destroying cover should fling physics debris chunks");
    }

    #[test]
    fn tumble_debris_settles_on_the_ground() {
        // A spawned debris chunk should fall, bounce, and eventually come to rest (not fall forever).
        let mut g = game_with_class(crate::class::ClassKind::Trooper);
        let p = g.player.pos + Vec3::new(0.0, 5.0, -2.0);
        g.spawn_tumble(p, Vec3::new(0.5, 0.5, 0.5), rgb(150, 120, 90), 6, 4.0);
        assert!(!g.tumble.is_empty());
        for _ in 0..240 {
            g.update_tumble(1.0 / 60.0);
        }
        // after 4 seconds everything has either come to rest or expired — none should be mid-air falling fast
        for c in &g.tumble {
            assert!(c.rest || c.vel.y.abs() < 5.0, "debris should settle, not free-fall forever");
        }
    }

    #[test]
    fn gravity_grenade_opens_a_low_gravity_well() {
        let mut g = game_with_class(crate::class::ClassKind::Assassin);
        let p = g.player.pos;
        g.grenades.push(Grenade { pos: p + Vec3::new(0.0, 0.4, 0.0), vel: Vec3::ZERO, fuse: 0.03, from_enemy: false, kind: crate::class::GrenadeKind::Gravity });
        for _ in 0..4 {
            g.step(&Input::default(), 1.0 / 60.0);
        }
        assert!(!g.gravity_zones.is_empty(), "a gravity well should be armed");
        // standing in it, the player's effective gravity is near zero
        assert!(g.player.gravity_scale < 0.5, "gravity should be reduced inside the well, got {}", g.player.gravity_scale);
    }

    #[test]
    fn acid_pool_eats_a_standing_enemy_and_doubles_on_heavies() {
        let mut g = game_with_class(crate::class::ClassKind::Tank);
        let spot = g.player.pos + Vec3::new(0.0, 0.0, -5.0);
        g.enemies.truncate(1);
        g.enemies[0].kind = crate::enemy::EnemyKind::Brute;
        g.enemies[0].pos = spot;
        let hp0 = g.enemies[0].health;
        g.acid_pools.push(AcidPool { center: spot, radius: 3.0, life: 3.0, dps: 20.0 });
        for _ in 0..30 {
            g.update_supers(1.0 / 30.0); // apply the pool without letting the brute walk off it
        }
        assert!(g.enemies[0].health < hp0 - 10.0, "acid should chew through a standing brute");
    }

    #[test]
    fn gravity_slam_shockwaves_enemies_on_landing() {
        let mut g = game_with_class(crate::class::ClassKind::Assassin);
        g.enemies.truncate(1);
        g.enemies[0].pos = g.player.pos + Vec3::new(1.0, 0.0, 0.0);
        let hp0 = g.enemies[0].health;
        // arm the slam and simulate the touchdown directly
        g.slam_armed = true;
        g.player.on_ground = true;
        g.update_supers(1.0 / 60.0);
        assert!(!g.slam_armed, "slam should consume on landing");
        assert!(g.enemies[0].health < hp0, "the landing shockwave should hurt nearby enemies");
    }

    #[test]
    fn tank_super_deploys_two_shoulder_cannons() {
        let mut g = game_with_class(crate::class::ClassKind::Tank);
        g.super_charge = 1.0;
        let n0 = g.sentries.len();
        g.activate_super();
        assert_eq!(g.sentries.len(), n0 + 2, "tank super should deploy two cannons");
        assert_eq!(g.super_charge, 0.0, "super should be spent");
    }

    #[test]
    fn super_charges_over_time_for_a_class_that_has_one() {
        let mut g = game_with_class(crate::class::ClassKind::Tank);
        g.super_charge = 0.0;
        for _ in 0..120 {
            g.step(&Input::default(), 1.0 / 30.0);
        }
        assert!(g.super_charge > 0.0, "super should accrue charge over time");
    }

    #[test]
    fn an_air_current_carries_a_still_player_along_its_flow() {
        // Build a Meadow map (has currents), drop the player onto a current midline, and confirm that with no
        // input they drift in the flow direction.
        let lvl = crate::level::Level::generate(crate::level::MapConfig { seed: 4, theme: crate::level::MapTheme::Meadow, size: 180, ..Default::default() });
        if lvl.air_currents.is_empty() {
            return;
        }
        let ac = lvl.air_currents[0].clone();
        let mid = ac.path[ac.path.len() / 2];
        let dir = ac.flow_at(mid).expect("midline in current");
        let mut g = Game::from_level(lvl, 1, Default::default(), Default::default(), 0, crate::enemy::Difficulty::Regular, None, crate::creatures::HuntRole::Predator);
        g.player.pos = Vec3::new(mid.x, g.level.floor_h(mid.x, mid.y), mid.y);
        g.player.vel = Vec3::ZERO;
        let p0 = g.player.pos;
        for _ in 0..30 {
            g.step(&Input::default(), 1.0 / 30.0);
        }
        let moved = Vec3::new(g.player.pos.x - p0.x, 0.0, g.player.pos.z - p0.z);
        let dist = (moved.x * moved.x + moved.z * moved.z).sqrt();
        assert!(dist > 1.0, "the current should carry the idle player, moved {:.2}", dist);
        // and roughly in the flow direction (dot product positive)
        let dot = moved.x * dir.x + moved.z * dir.y;
        assert!(dot > 0.0, "carry should be downstream (dot {:.2})", dot);
    }

    #[test]
    fn possession_gas_turns_wildlife_and_hangs_in_the_air() {
        let mut g = Game::new(0);
        let n0 = g.enemies.len();
        g.trigger_event(EventKind::PossessionGas);
        // the gas drives extra "possessed" creatures into the fight immediately
        assert!(g.enemies.len() >= n0 + 3, "gas should spawn possessed wildlife ({} -> {})", n0, g.enemies.len());
        // the freshly possessed creatures spawn already aggressive (hunting), not idle wildlife
        let aggressive = g.enemies[n0..].iter().all(|e| e.state != crate::enemy::EnemyState::Idle);
        assert!(aggressive, "possessed wildlife should spawn hunting, not idle");
        // and the cloud intensity ramps up as the event runs
        for _ in 0..40 {
            g.step(&Input::default(), 1.0 / 30.0);
        }
        assert!(g.gas > 0.0, "the gas should hang in the air during the event");
    }

    #[test]
    fn water_surface_ripples_and_reflects() {
        let lvl = crate::level::Level::generate(crate::level::MapConfig { seed: 6, theme: crate::level::MapTheme::Mistfall, size: 150, ..Default::default() });
        if lvl.water.is_empty() {
            return;
        }
        let mut g = Game::from_level(lvl, 0, Default::default(), Default::default(), 0, crate::enemy::Difficulty::Regular, None, crate::creatures::HuntRole::Predator);
        g.time = 0.0;
        let m0 = g.build_water_meshes();
        g.time = 1.5;
        let m1 = g.build_water_meshes();
        assert!(!m0.is_empty(), "mistfall should build a water mesh");
        assert!(m0[0].reflect > 0.4, "water should be reflective");
        // the same vertex moves between two times → it's animated, not a frozen sheet
        let moved = m0[0].verts.iter().zip(m1[0].verts.iter()).any(|(a, b)| (a.y - b.y).abs() > 1e-3);
        assert!(moved, "water surface should ripple over time");
    }

}
