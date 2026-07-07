//! app.rs — the playable shell (only compiled with `--features app`). It owns an eframe window, grabs
//! the mouse for FPS look, runs the simulation on a fixed timestep, software-renders the world into an
//! `egui` mesh, and stamps a deliberately chunky early-2000s HUD on top: crosshair, big health/ammo
//! readouts, the weapon name, a kill/wave counter, a low-tech radar minimap, a red hit-flash, and
//! screen-shake.
//!
//! Esc opens an in-game menu with a full settings page: mouse sensitivity, invert-look, field of view,
//! minimap toggle, and fully rebindable movement keys. Defaults are laid out for **Dvorak** — the
//! `. o e u` cluster sits in the same inverted-T shape WASD does on QWERTY — with a one-click QWERTY
//! preset for everyone else.

use eframe::egui;
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, Stroke, Vec2};

use crate::camera::{scale_for_fov, FpsCam};
use crate::math::Vec3;
use crate::game::Game;
use crate::level::{Cell, MapTheme};
use crate::player::Input;
use crate::creatures::{Creature, HuntRole};
use crate::builder::{AssetKind, CustomMap, Placed};
use crate::db::Db;
use crate::cosmetics::{Accent, ArmorFrame, GunSkin, Reticle};
use crate::enemy::Difficulty;
use crate::loadout::{Loadout, Perk};
use crate::weapons::WeaponKind;

const FIXED_DT: f32 = 1.0 / 120.0;

/// Which on-screen state we're in. The simulation only advances in `Play`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Start,
    Play,
    Pause,
    Settings,
}

/// Which page of the full-screen front-end menu is showing (only meaningful in `Mode::Start`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuScreen {
    Main,
    Multiplayer,
    MapSelect,
    Loadout,
    Customize,
    CustomMatch,
    Hunt,
    Profile,
    Builder,
}

/// Live state for the map-editor session: the map being authored plus the current placement brush.
struct BuildState {
    map: CustomMap,
    sel: usize, // index into AssetKind::ALL
    rot: f32,
    scale: f32,
    param: f32, // tunable (man-cannon strength)
    snap: bool,
    scatter: bool,
    fly: bool, // free-fly camera vs. walk
    status: String,
    status_t: f32,
    /// Countdown to the next allowed placement while the mouse button is held — paint-mode throttle so
    /// dragging across the ground places a steady trail instead of one asset per rendered frame.
    paint_cd: f32,
}

impl BuildState {
    fn brush(&self) -> AssetKind {
        AssetKind::ALL[self.sel % AssetKind::ALL.len()]
    }
    /// Switch the active brush and reset the tunable to that asset's default.
    fn set_brush(&mut self, sel: usize) {
        self.sel = sel % AssetKind::ALL.len();
        self.param = self.brush().default_param();
    }
    fn note(&mut self, s: &str) {
        self.status = s.to_string();
        self.status_t = 2.5;
    }
}

/// A rebindable movement action.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BindAction {
    Forward,
    Left,
    Back,
    Right,
    Jump,
    Sprint,
    Crouch,
    Dive,
    Reload,
    // ── Menu / forge navigation (arrow-key + vim-style defaults) ──
    NavUp,
    NavDown,
    NavLeft,
    NavRight,
    NavSelect,
    NavBack,
    // ── Forge editor actions ──
    ForgeNextBrush,
    ForgePrevBrush,
    ForgeRotate,
    ForgeScaleUp,
    ForgeScaleDown,
    ForgeUndo,
    ForgeMenu,
}

/// Movement + action keys.
#[derive(Clone, Copy)]
struct Bindings {
    forward: Key,
    left: Key,
    back: Key,
    right: Key,
    jump: Key,
    sprint: Key,
    crouch: Key,
    crawl: Key,
    dive: Key,
    reload: Key,
    // menu / forge navigation
    nav_up: Key,
    nav_down: Key,
    nav_left: Key,
    nav_right: Key,
    nav_select: Key,
    nav_back: Key,
    // forge editor
    forge_next: Key,
    forge_prev: Key,
    forge_rotate: Key,
    forge_scale_up: Key,
    forge_scale_down: Key,
    forge_undo: Key,
    forge_menu: Key,
}

impl Bindings {
    /// Dvorak default: `.` above `o e u`, mirroring the WASD inverted-T by physical position.
    fn dvorak() -> Self {
        Self {
            forward: Key::Period,
            left: Key::O,
            back: Key::E,
            right: Key::U,
            jump: Key::Space,
            sprint: Key::Semicolon,
            crouch: Key::Q,
            crawl: Key::X,
            dive: Key::J,
            reload: Key::P,
            ..Self::nav_forge_defaults()
        }
    }
    fn qwerty() -> Self {
        Self {
            forward: Key::W,
            left: Key::A,
            back: Key::S,
            right: Key::D,
            jump: Key::Space,
            sprint: Key::ArrowUp,
            crouch: Key::C,
            crawl: Key::X,
            dive: Key::V,
            reload: Key::R,
            ..Self::nav_forge_defaults()
        }
    }
    /// Shared defaults for menu/forge navigation, independent of the movement layout: arrow keys plus a
    /// vim-style `h j k l` for up/down/left/right, Enter to select, Backspace to go back, and the forge
    /// editor keys. These are the same regardless of Dvorak/QWERTY so navigation is always available.
    fn nav_forge_defaults() -> Self {
        Self {
            // movement placeholders (overwritten by dvorak()/qwerty() via struct update)
            forward: Key::W,
            left: Key::A,
            back: Key::S,
            right: Key::D,
            jump: Key::Space,
            sprint: Key::ArrowUp,
            crouch: Key::C,
            crawl: Key::X,
            dive: Key::V,
            reload: Key::R,
            // navigation: arrows are primary; hjkl handled as always-on aliases in the nav helper
            nav_up: Key::ArrowUp,
            nav_down: Key::ArrowDown,
            nav_left: Key::ArrowLeft,
            nav_right: Key::ArrowRight,
            nav_select: Key::Enter,
            nav_back: Key::Backspace,
            // forge editor
            forge_next: Key::Tab,
            forge_prev: Key::Backtick,
            forge_rotate: Key::R,
            forge_scale_up: Key::Equals,
            forge_scale_down: Key::Minus,
            forge_undo: Key::Z,
            forge_menu: Key::Escape,
        }
    }
    fn get(&self, a: BindAction) -> Key {
        match a {
            BindAction::Forward => self.forward,
            BindAction::Left => self.left,
            BindAction::Back => self.back,
            BindAction::Right => self.right,
            BindAction::Jump => self.jump,
            BindAction::Sprint => self.sprint,
            BindAction::Crouch => self.crouch,
            BindAction::Dive => self.dive,
            BindAction::Reload => self.reload,
            BindAction::NavUp => self.nav_up,
            BindAction::NavDown => self.nav_down,
            BindAction::NavLeft => self.nav_left,
            BindAction::NavRight => self.nav_right,
            BindAction::NavSelect => self.nav_select,
            BindAction::NavBack => self.nav_back,
            BindAction::ForgeNextBrush => self.forge_next,
            BindAction::ForgePrevBrush => self.forge_prev,
            BindAction::ForgeRotate => self.forge_rotate,
            BindAction::ForgeScaleUp => self.forge_scale_up,
            BindAction::ForgeScaleDown => self.forge_scale_down,
            BindAction::ForgeUndo => self.forge_undo,
            BindAction::ForgeMenu => self.forge_menu,
        }
    }
    fn set(&mut self, a: BindAction, k: Key) {
        match a {
            BindAction::Forward => self.forward = k,
            BindAction::Left => self.left = k,
            BindAction::Back => self.back = k,
            BindAction::Right => self.right = k,
            BindAction::Jump => self.jump = k,
            BindAction::Sprint => self.sprint = k,
            BindAction::Crouch => self.crouch = k,
            BindAction::Dive => self.dive = k,
            BindAction::Reload => self.reload = k,
            BindAction::NavUp => self.nav_up = k,
            BindAction::NavDown => self.nav_down = k,
            BindAction::NavLeft => self.nav_left = k,
            BindAction::NavRight => self.nav_right = k,
            BindAction::NavSelect => self.nav_select = k,
            BindAction::NavBack => self.nav_back = k,
            BindAction::ForgeNextBrush => self.forge_next = k,
            BindAction::ForgePrevBrush => self.forge_prev = k,
            BindAction::ForgeRotate => self.forge_rotate = k,
            BindAction::ForgeScaleUp => self.forge_scale_up = k,
            BindAction::ForgeScaleDown => self.forge_scale_down = k,
            BindAction::ForgeUndo => self.forge_undo = k,
            BindAction::ForgeMenu => self.forge_menu = k,
        }
    }
}

/// Player-tunable options, edited live on the settings page.
struct Settings {
    sensitivity: f32,
    invert_y: bool,
    fov_deg: f32,
    third_person: bool,
    show_minimap: bool,
    bindings: Bindings,
    /// Geometry detail / "render resolution" multiplier (tessellation of curved meshes).
    render_detail: f32,
    /// Procedural-texture resolution: how finely floors/walls are subdivided for vertex tinting.
    texture_detail: f32,
    /// Selected graphics quality preset (Low/Medium/High/Ultra) — bundles detail, fog, lights, shadows.
    graphics_preset: crate::graphics::GraphicsPreset,
    /// Team colours: the hue hostile enemies are tinted toward, and the friendly hue for allied markers.
    /// Defaults orange (hostile) / green (friendly).
    hostile_color: egui::Color32,
    friendly_color: egui::Color32,
    /// Preferred renderer: false = software painter, true = GPU (wgpu). Mirrors `Db::gpu_renderer`; applied on
    /// the next launch (the two backends use different windowing, so it can't hot-swap live).
    gpu_renderer: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sensitivity: 0.0022,
            invert_y: false,
            fov_deg: 74.0,
            third_person: false,
            show_minimap: true,
            bindings: Bindings::dvorak(),
            render_detail: 1.4,
            texture_detail: 3.0,
            graphics_preset: crate::graphics::GraphicsPreset::default(),
            hostile_color: egui::Color32::from_rgb(230, 120, 40), // orange
            friendly_color: egui::Color32::from_rgb(70, 190, 90), // green
            gpu_renderer: false,
        }
    }
}

pub struct FpsApp {
    game: Game,
    settings: Settings,
    mode: Mode,
    /// Material texture atlas (built once on the first frame) + its bound id for the renderer.
    atlas: Option<egui::TextureHandle>,
    atlas_id: egui::TextureId,
    /// The atlas tile resolution the current atlas was baked at — when the graphics preset changes this, the
    /// atlas is rebuilt at the new resolution.
    atlas_tile: usize,
    /// When `Some`, the next key press is captured as the binding for this action.
    rebinding: Option<BindAction>,
    grabbed: bool,
    accum: f32,
    /// True the first frame so we can grab the mouse once the window exists.
    first: bool,
    /// The base seed, so the menu can start a fresh run.
    seed: u64,
    /// Bumped each new game so successive runs differ.
    runs: u64,
    /// Which menu opened Settings, so its Back button returns there.
    settings_from: Mode,
    /// Which front-end page is showing.
    screen: MenuScreen,
    /// Persistent player record: K/D metaprofile + saved loadout.
    db: Db,
    /// Map selector state: chosen biome (None = random / quick play) and whether we arrived via "online".
    sel_preset: Option<usize>,
    mp_online: bool,
    /// Per-life kill bookkeeping so deaths fold cleanly into the profile.
    recorded_kills: u32,
    life_best_streak: u32,
    was_alive: bool,
    /// Custom-match settings: bot count and bot skill tier.
    custom_bots: usize,
    custom_diff: Difficulty,
    /// The map-selection "cinematic tuning" slider (0..1): how hard the recursive enhancer pushes a base map
    /// toward a livelier, more varied, more cinematic layout before play begins.
    cinematic_tuning: f32,
    /// `Some(creature)` queues The Hunt mode for the next deploy; `None` is the standard shooter.
    hunt_creature: Option<Creature>,
    hunt_role: HuntRole,
    /// Map-builder state: the active editing session (when `Some`), plus the New-Map screen's choices and a
    /// cached list of saved custom maps for the menus.
    build: Option<BuildState>,
    /// When true, the forge's Esc-menu overlay is showing (Resume / Keybinds / Save & Exit). Esc toggles it
    /// instead of immediately dumping the player out of the editor, so the mouse can reach forge tooling.
    forge_menu_open: bool,
    build_theme: usize,
    build_size: i32,
    build_maps: Vec<String>,
    /// Armory preview: orbit angles + which body slot is being edited.
    armory_yaw: f32,
    armory_pitch: f32,
    armory_slot: usize,
    /// Cached crossfading map preview shown in the map-select menu.
    map_preview: Option<MapPreview>,
    /// Procedural sound engine (only with the `sound` feature; `None` if no audio device).
    #[cfg(feature = "sound")]
    audio: Option<crate::audio::play::Audio>,
}

impl FpsApp {
    pub fn new(seed: u64) -> Self {
        let mut app = Self {
            game: Game::new(seed),
            settings: Settings::default(),
            mode: Mode::Start,
            atlas: None,
            atlas_id: egui::TextureId::Managed(0),
            atlas_tile: 0,
            rebinding: None,
            grabbed: false,
            accum: 0.0,
            first: true,
            seed,
            runs: 0,
            settings_from: Mode::Pause,
            screen: MenuScreen::Main,
            db: Db::load(),
            sel_preset: None,
            mp_online: false,
            recorded_kills: 0,
            life_best_streak: 0,
            was_alive: true,
            custom_bots: 12,
            custom_diff: Difficulty::Regular,
            cinematic_tuning: 0.5,
            hunt_creature: None,
            hunt_role: HuntRole::Predator,
            build: None,
            forge_menu_open: false,
            build_theme: 0,
            build_size: 160,
            build_maps: Vec::new(),
            armory_yaw: 0.6,
            armory_pitch: 0.1,
            armory_slot: 0,
            map_preview: None,
            #[cfg(feature = "sound")]
            audio: crate::audio::play::Audio::new(),
        };
        // Reflect the persisted renderer choice into the live settings so the toggle shows the saved state.
        app.settings.gpu_renderer = app.db.gpu_renderer;
        app
    }

    /// Apply the resolution/detail settings to the global mesh tessellation knobs.
    fn apply_detail(&self) {
        // The graphics preset sets a baseline geometry detail; the manual slider multiplies on top of it, so
        // a player can pick a preset and still fine-tune. Low preset floors the tessellation for framerate.
        let preset = self.settings.graphics_preset.settings();
        crate::mesh::set_detail(self.settings.render_detail * preset.geom_detail);
        crate::level::set_texture_detail(self.settings.texture_detail);
    }

    /// Begin a brand-new run (used by the start menu's Play button).
    fn start_new_game(&mut self, ctx: &egui::Context) {
        self.runs = self.runs.wrapping_add(1);
        self.apply_detail();
        let seed = self.seed.wrapping_add(self.runs.wrapping_mul(0x9E37_79B9));
        self.game = Game::new_match_tuned(
            seed,
            self.sel_preset,
            self.db.loadout,
            self.db.cosmetics,
            self.custom_bots,
            self.custom_diff,
            self.hunt_creature,
            self.hunt_role,
            crate::enhance::CinematicTuning(self.cinematic_tuning),
        );
        self.db.profile.record_match();
        self.db.save();
        self.recorded_kills = 0;
        self.life_best_streak = 0;
        self.was_alive = true;
        self.goto(ctx, Mode::Play);
    }

    /// The Armory: an orbitable character preview plus per-body-part style/colour pickers. Renders the
    /// mannequin live with the equipped `ArmorSet` so you can mix sets across head/shoulders/arms/legs/etc.
    fn armory_panel(&mut self, ui: &mut egui::Ui, gold: Color32, cyan: Color32) {
        use crate::character::{ArmorSlot, PieceStyle};
        ui.label(egui::RichText::new("ARMORY").monospace().size(16.0).color(gold));
        ui.label(egui::RichText::new("Drag to orbit · mix a piece on every body part").monospace().size(11.0).color(dim()));
        ui.add_space(4.0);

        // ── 3D preview ───────────────────────────────────────────────────────────────────────────────
        let (resp, painter) = ui.allocate_painter(egui::vec2(360.0, 380.0), egui::Sense::drag());
        let rect = resp.rect;
        painter.rect_filled(rect, 6.0, Color32::from_rgb(18, 21, 27));
        // floor disc backdrop
        painter.circle_filled(Pos2::new(rect.center().x, rect.bottom() - 60.0), 120.0, Color32::from_rgb(24, 28, 36));
        if resp.dragged() {
            let d = resp.drag_delta();
            self.armory_yaw -= d.x * 0.012;
            self.armory_pitch = (self.armory_pitch + d.y * 0.010).clamp(-0.7, 1.0);
        } else {
            self.armory_yaw += 0.004; // gentle turntable when idle
        }
        // Orbit camera looking at the chest height.
        let center = Vec3::new(0.0, 0.95, 0.0);
        let r = 2.7;
        let (cy, sy) = (self.armory_yaw.cos(), self.armory_yaw.sin());
        let (cp, sp) = (self.armory_pitch.cos(), self.armory_pitch.sin());
        let pos = center + Vec3::new(sy * cp, sp, cy * cp) * r;
        let f = (center - pos).normalized();
        let cam = FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() };
        let mut light = crate::mesh::Light::sun(Vec3::new(-0.4, 0.9, 0.45), Color32::from_rgb(255, 246, 224));
        light.sky = Color32::from_rgb(150, 170, 200);
        light.ground = Color32::from_rgb(40, 38, 44);
        light.ambient = 0.95;
        let fog = crate::render::Fog::new(Color32::from_rgb(18, 21, 27), 30.0, 80.0);
        // Living idle animation: the figure breathes, shifts its weight and glances around while you browse,
        // rather than standing as a dead T-pose. Driven by the UI clock so it's smooth regardless of framerate.
        let t = ui.input(|i| i.time) as f32;
        let meshes = crate::character::build_mannequin_posed(&self.db.cosmetics.set, crate::character::Pose::idle(t));
        let scale = scale_for_fov(rect.height(), 46f32.to_radians());
        let mesh = crate::render::render(&meshes, &cam, rect.center().x, rect.center().y + 12.0, scale, &light, &fog, rect.width(), rect.height(), 0.0, crate::math::Vec2::new(0.0, 0.0), self.atlas_id, 5, &[]);
        // request continuous repaint so the idle animation actually plays
        ui.ctx().request_repaint();
        // Paint the mannequin directly on the panel painter (see MapPreview::paint for why with_clip_rect was
        // dropped — it silently swallowed the mesh, leaving the armory preview blank).
        painter.add(egui::Shape::mesh(mesh));

        ui.add_space(6.0);
        // ── Slot selector ─────────────────────────────────────────────────────────────────────────────
        let slot = ArmorSlot::ALL[self.armory_slot % ArmorSlot::ALL.len()];
        ui.horizontal(|ui| {
            if ui.add_sized([30.0, 26.0], egui::Button::new(egui::RichText::new("◀").monospace())).clicked() {
                self.armory_slot = (self.armory_slot + ArmorSlot::ALL.len() - 1) % ArmorSlot::ALL.len();
            }
            ui.label(egui::RichText::new(format!("{:^16}", slot.name())).monospace().size(15.0).color(gold));
            if ui.add_sized([30.0, 26.0], egui::Button::new(egui::RichText::new("▶").monospace())).clicked() {
                self.armory_slot = (self.armory_slot + 1) % ArmorSlot::ALL.len();
            }
        });
        // style row
        let cur_style = self.db.cosmetics.set.style(slot);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("style").monospace().size(11.0).color(dim()));
            for s in PieceStyle::ALL {
                let sel = cur_style == s;
                let txt = egui::RichText::new(s.name()).monospace().size(12.0).color(if sel { gold } else { Color32::from_rgb(196, 202, 212) });
                if ui.add_sized([66.0, 24.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                    self.db.cosmetics.set.set_style(slot, s);
                }
            }
        });
        // accent row
        let cur_acc = self.db.cosmetics.set.accent(slot);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("colour").monospace().size(11.0).color(dim()));
            for a in Accent::ALL {
                let sel = cur_acc == a;
                if ui.add_sized([20.0, 20.0], egui::Button::new("").fill(a.color()).stroke(egui::Stroke::new(if sel { 2.0 } else { 0.0 }, gold))).clicked() {
                    self.db.cosmetics.set.set_accent(slot, a);
                }
            }
        });
        let _ = cyan;
    }

    /// A fresh per-deploy seed (for playing custom maps / generated runs).
    fn next_seed(&mut self) -> u64 {
        self.runs = self.runs.wrapping_add(1);
        self.seed.wrapping_add(self.runs.wrapping_mul(0x9E37_79B9))
    }

    /// Common play-start bookkeeping when a game has already been assigned to `self.game`.
    fn begin_play(&mut self, ctx: &egui::Context) {
        self.apply_detail();
        self.recorded_kills = 0;
        self.life_best_streak = 0;
        self.was_alive = true;
        #[cfg(feature = "sound")]
        self.start_ambient_bed();
        self.goto(ctx, Mode::Play);
    }

    /// Build and loop the biome's procedural ambient sound bed (wind + drips + room reverb).
    #[cfg(feature = "sound")]
    fn play_sfx(&mut self, events: &[crate::game::Sfx]) {
        use crate::game::Sfx;
        let Some(a) = self.audio.as_ref() else { return };
        for (i, ev) in events.iter().enumerate() {
            let s = i as u32;
            let samples = match *ev {
                Sfx::Shot(bore) => crate::audio::gunshot(bore, self.seed as u32 ^ s),
                Sfx::Impact(h) => crate::audio::impact(h, self.seed as u32 ^ s ^ 0x11),
                Sfx::Explosion(sz) => crate::audio::explosion(sz, self.seed as u32 ^ s ^ 0x22),
                Sfx::Pickup => crate::audio::blip(520.0, true, s),
                Sfx::Reload => crate::audio::reload_click(s),
            };
            a.play(samples);
        }
    }

    #[cfg(feature = "sound")]
    fn start_ambient_bed(&mut self) {
        use crate::audio::{AmbientBed, Material};
        use crate::level::MapTheme;
        let theme = self.game.level.cfg.theme;
        let (mat, exposure) = match theme {
            MapTheme::Mistfall | MapTheme::Toxic => (Material::stone(), 0.55), // wet, semi-open
            MapTheme::Ice => (Material::ice(), 0.7),
            MapTheme::Volcanic => (Material::stone(), 0.6),
            MapTheme::Meadow | MapTheme::Redwood | MapTheme::Canopy | MapTheme::Alien => (Material::foliage(), 0.95),
            MapTheme::Industrial | MapTheme::Bunker | MapTheme::Warehouse | MapTheme::Military => (Material::metal(), 0.1),
            _ => (Material::stone(), 0.4),
        };
        let bed = AmbientBed::from(mat, exposure);
        let samples = crate::audio::render_bed(bed, 4.0, self.seed as u32);
        if let Some(a) = self.audio.as_ref() {
            a.set_bed(samples);
        }
    }

    /// Open the map editor on `map`: build a peaceful sandbox level and enter Play with the brush active.
    fn enter_builder(&mut self, ctx: &egui::Context, map: CustomMap) {
        self.apply_detail();
        self.game = Game::from_custom(&map, self.db.loadout, self.db.cosmetics);
        self.build = Some(BuildState { map, sel: 0, rot: 0.0, scale: 1.0, param: 0.0, snap: false, scatter: false, fly: true, status: String::new(), status_t: 0.0, paint_cd: 0.0 });
        self.goto(ctx, Mode::Play);
    }

    /// Save the active custom map and return to the builder menu.
    fn exit_builder(&mut self, ctx: &egui::Context) {
        if let Some(b) = &self.build {
            b.map.save();
        }
        self.build = None;
        self.build_maps = CustomMap::list();
        self.screen = MenuScreen::Builder;
        self.goto(ctx, Mode::Start);
    }

    /// Per-frame editor tick: ground-aim placement, deletion, brush cycling, rotate/scale, quick-save.
    /// Matchmaking: pick a map from the preferred pool (or random) and scale the lobby — bot count and
    /// difficulty — to the player's rank, so opponents track the player's skill.
    fn matchmake_deploy(&mut self) {
        let rating = crate::rank::rating(&self.db.profile);
        let lobby = crate::rank::matchmake(rating);
        self.custom_bots = lobby.bots;
        self.custom_diff = lobby.difficulty;
        // choose a map from the preferred pool if any, else fully random
        if self.db.map_prefs.is_empty() {
            self.sel_preset = None;
        } else {
            let pick = (rating as usize)
                .wrapping_add(self.db.profile.matches as usize)
                .wrapping_mul(2654435761)
                % self.db.map_prefs.len();
            self.sel_preset = Some(self.db.map_prefs[pick].min(crate::level::PRESETS.len() - 1));
        }
    }

    /// The interactive Forge menu: brush picker (by category), tunable sliders, and toggles. Floats over the
    /// fly-around view; clicking it never places assets (builder_tick ignores clicks egui consumed).
    fn build_panel(&mut self, ctx: &egui::Context) {
        let cam = self.game.player.cam.pos;
        let gold = Color32::from_rgb(220, 180, 90);
        let Some(b) = self.build.as_mut() else { return };
        egui::Window::new("MAP FORGE")
            .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
            .resizable(false)
            .collapsible(true)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for cat in ["Flora", "Terrain", "Props", "Gadgets", "Pickups"] {
                    ui.label(egui::RichText::new(cat).monospace().size(11.0).color(gold));
                    ui.horizontal_wrapped(|ui| {
                        for (i, k) in AssetKind::ALL.iter().enumerate() {
                            if k.category() == cat {
                                let sel = b.sel == i;
                                let resp = ui.selectable_label(sel, egui::RichText::new(k.name()).monospace().size(12.0));
                                if resp.clicked() {
                                    b.set_brush(i);
                                }
                                resp.on_hover_text(format!("Tab/click to select {} as the active brush.\nClick the ground to place one; hold LMB to paint a trail.", k.name()));
                            }
                        }
                    });
                }
                ui.separator();
                ui.add(egui::Slider::new(&mut b.rot, 0.0..=std::f32::consts::TAU).text("rotation"))
                    .on_hover_text("Facing angle for the next placement (`,` / `.` also nudge this).");
                ui.add(egui::Slider::new(&mut b.scale, 0.3..=4.0).text("scale"))
                    .on_hover_text("Uniform size for the next placement.\nMouse wheel over the 3D view also adjusts this without opening the panel.");
                if b.brush().is_cannon() {
                    ui.add(egui::Slider::new(&mut b.param, 4.0..=18.0).text("launch power")).on_hover_text("How hard the man-cannon flings the player on contact.");
                }
                ui.separator();
                ui.checkbox(&mut b.snap, "grid snap (G)").on_hover_text("Snap placement to the 2-unit tile grid — lines assets up cleanly with the level's floor tiles.");
                ui.checkbox(&mut b.scatter, "scatter brush (B)").on_hover_text("Each placement drops a small natural cluster of the brush instead of one asset — faster for filling in foliage/rubble fields.");
                ui.checkbox(&mut b.fly, "fly camera").on_hover_text("Free-fly noclip camera for building vs. the normal walking/jumping player camera.");
                ui.separator();
                ui.label(egui::RichText::new(format!("brush: {}   placed: {}", b.brush().name(), b.map.placed.len())).monospace().size(11.0));
                ui.horizontal(|ui| {
                    if ui
                        .button("Set spawn here")
                        .on_hover_text("Move this map's spawn point to your current position.")
                        .clicked()
                    {
                        b.map.spawn = (cam.x, cam.z);
                        b.note("Spawn set");
                    }
                    if ui
                        .button("Save")
                        .on_hover_text("Write this map to disk now (Esc also saves on exit).\nMaps can also be authored/edited externally as JSON — see buildscript.rs.")
                        .clicked()
                    {
                        let ok = b.map.save();
                        b.note(if ok { "Saved" } else { "save failed" });
                    }
                });
                ui.label(egui::RichText::new("LMB place (hold to paint) · RMB delete · scroll = scale · Z undo · Esc = forge menu").monospace().size(10.0).color(Color32::from_gray(150)));
            });
    }

    /// Translate the navigation keybinds (arrow keys + vim h/j/k/l by default, all rebindable) into egui's
    /// built-in directional focus events, so every front-end menu, the settings/pause page, and the forge
    /// Esc-menu is fully keyboard-navigable. egui already moves widget focus on arrow keys / Tab and activates
    /// the focused widget on Enter/Space; we inject the matching synthetic key events whenever the player's
    /// bound nav keys are pressed. Only runs in menu-like contexts so it never eats gameplay input.
    fn handle_menu_nav(&mut self, ctx: &egui::Context) {
        // Skip while capturing a rebind (the settings page is listening for the raw key to assign it).
        if self.rebinding.is_some() {
            return;
        }
        let menu_active = matches!(self.mode, Mode::Start | Mode::Settings | Mode::Pause) || self.forge_menu_open;
        if !menu_active {
            return;
        }
        let b = self.settings.bindings;
        // Map each bound nav action to the egui Key egui itself interprets for focus movement / activation.
        // vim h/j/k/l are always-on aliases alongside whatever the player bound.
        let pressed = |ctx: &egui::Context, k: Key| ctx.input(|i| i.key_pressed(k));
        let mut inject: Vec<Key> = Vec::new();
        if pressed(ctx, b.get(BindAction::NavUp)) || pressed(ctx, Key::K) {
            inject.push(Key::ArrowUp);
        }
        if pressed(ctx, b.get(BindAction::NavDown)) || pressed(ctx, Key::J) {
            inject.push(Key::ArrowDown);
        }
        if pressed(ctx, b.get(BindAction::NavLeft)) || pressed(ctx, Key::H) {
            inject.push(Key::ArrowLeft);
        }
        if pressed(ctx, b.get(BindAction::NavRight)) || pressed(ctx, Key::L) {
            inject.push(Key::ArrowRight);
        }
        if pressed(ctx, b.get(BindAction::NavSelect)) {
            inject.push(Key::Enter);
        }
        if inject.is_empty() {
            return;
        }
        // If nothing is focused yet, the first nav press should focus the first focusable widget: egui does
        // this when it receives a Tab, so seed focus with Tab when no widget currently holds it.
        let has_focus = ctx.memory(|m| m.focused().is_some());
        ctx.input_mut(|i| {
            if !has_focus {
                i.events.push(egui::Event::Key {
                    key: Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                });
            }
            for k in inject {
                // Enter also needs a Space to reliably click egui buttons that only respond to Space; push both.
                i.events.push(egui::Event::Key {
                    key: k,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                });
                if k == Key::Enter {
                    i.events.push(egui::Event::Key {
                        key: Key::Space,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::default(),
                    });
                }
            }
        });
    }

    /// The forge's Esc-menu overlay: a centred panel giving mouse access to Resume / Keybinds / Save & Exit
    /// while editing. Opened by the Forge Menu key (Esc by default). Placement is suppressed while it's up.
    fn forge_menu_panel(&mut self, ctx: &egui::Context) {
        let gold = Color32::from_rgb(220, 180, 90);
        let mut want_resume = false;
        let mut want_keybinds = false;
        let mut want_save_exit = false;
        egui::Area::new(egui::Id::new("forge_menu"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(20, 22, 26, 240))
                    .inner_margin(egui::Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_width(260.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("MAP FORGE").monospace().size(20.0).color(gold).strong());
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("editing paused").monospace().size(11.0).color(dim()));
                            ui.add_space(14.0);
                            let btn = |ui: &mut egui::Ui, txt: &str| ui.add_sized([220.0, 34.0], egui::Button::new(egui::RichText::new(txt).monospace().size(15.0))).clicked();
                            if btn(ui, "Resume editing") {
                                want_resume = true;
                            }
                            ui.add_space(4.0);
                            if btn(ui, "Controls / keybinds") {
                                want_keybinds = true;
                            }
                            ui.add_space(4.0);
                            if btn(ui, "Save & exit to menu") {
                                want_save_exit = true;
                            }
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("Esc resumes · arrows/hjkl navigate").monospace().size(10.0).color(dim()));
                        });
                    });
            });
        if want_resume {
            self.forge_menu_open = false;
            let fly = self.build.as_ref().map_or(false, |b| b.fly);
            self.set_grab(ctx, fly);
        } else if want_keybinds {
            self.forge_menu_open = false;
            self.settings_from = Mode::Play;
            self.goto(ctx, Mode::Settings);
        } else if want_save_exit {
            self.forge_menu_open = false;
            self.exit_builder(ctx);
        }
    }

    fn builder_tick(&mut self, ctx: &egui::Context) {
        // Read discrete editor inputs. Forge actions come from the rebindable bindings (defaults: Tab next,
        // Backtick prev, R rotate, +/- scale, Z undo) with the legacy comma/period/quote keys kept as always-
        // on aliases so existing muscle memory still works.
        let bnd = self.settings.bindings;
        let (place, delete, next_kind, prev_kind, rot_l, rot_r, scale_dn, scale_up, save, undo, snap_t, scatter_t) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.secondary_pressed(),
                i.key_pressed(bnd.get(BindAction::ForgeNextBrush)) || i.key_pressed(Key::Tab),
                i.key_pressed(bnd.get(BindAction::ForgePrevBrush)) || i.key_pressed(Key::Backtick),
                i.key_pressed(Key::Comma),
                i.key_pressed(bnd.get(BindAction::ForgeRotate)) || i.key_pressed(Key::Period),
                i.key_pressed(bnd.get(BindAction::ForgeScaleDown)) || i.key_pressed(Key::Semicolon),
                i.key_pressed(bnd.get(BindAction::ForgeScaleUp)) || i.key_pressed(Key::Quote),
                i.key_pressed(Key::Enter),
                i.key_pressed(bnd.get(BindAction::ForgeUndo)) || i.key_pressed(Key::Z),
                i.key_pressed(Key::G),
                i.key_pressed(Key::B),
            )
        });
        // Rapid-design mouse extras: hold the primary button to paint a steady trail instead of clicking once
        // per asset, and scroll to resize on the fly without leaving the keyboard's home row.
        let (held, scroll_y) = ctx.input(|i| (i.pointer.primary_down(), i.smooth_scroll_delta.y));
        let dt = ctx.input(|i| i.stable_dt);
        let ui_busy = ctx.is_pointer_over_egui();
        let (held, delete) = (held && !ui_busy, delete && !ui_busy);
        let Some(b) = self.build.as_mut() else { return };
        b.status_t = (b.status_t - dt).max(0.0);
        // Scroll wheel resizes the brush — the fast path for "make this one a little bigger/smaller" without
        // reaching for the keyboard, which matters when you're mid-drag with the other hand on the mouse.
        if scroll_y.abs() > 0.01 && !ui_busy {
            b.scale = (b.scale + scroll_y * 0.02).clamp(0.4, 3.5);
        }
        // Paint mode: a single click always places one; holding the button places a steady trail throttled
        // to a fixed rate (not once per rendered frame, which at 60+ fps would carpet the ground solid).
        b.paint_cd = (b.paint_cd - dt).max(0.0);
        let place = place || (held && b.paint_cd <= 0.0);
        if held && b.paint_cd <= 0.0 {
            b.paint_cd = 0.09;
        }
        let nkinds = AssetKind::ALL.len();
        if next_kind {
            b.set_brush(b.sel + 1);
        }
        if prev_kind {
            b.set_brush(b.sel + nkinds - 1);
        }
        if snap_t {
            b.snap = !b.snap;
            b.note(if b.snap { "Grid snap ON" } else { "Grid snap OFF" });
        }
        if scatter_t {
            b.scatter = !b.scatter;
            b.note(if b.scatter { "Scatter brush ON" } else { "Scatter brush OFF" });
        }
        if rot_l {
            b.rot -= std::f32::consts::FRAC_PI_8;
        }
        if rot_r {
            b.rot += std::f32::consts::FRAC_PI_8;
        }
        if scale_dn {
            b.scale = (b.scale - 0.1).max(0.4);
        }
        if scale_up {
            b.scale = (b.scale + 0.1).min(3.5);
        }
        // Aim point on the ground (ray-march the look vector down to the heightfield).
        let aim = aim_ground(&self.game);
        if let Some((mut ax, mut az)) = aim {
            if b.snap {
                ax = (ax / 2.0).round() * 2.0;
                az = (az / 2.0).round() * 2.0;
            }
            if place {
                let brush = b.brush();
                if b.scatter && !brush.solid() {
                    // scatter brush: a small natural cluster of the current asset (foliage/decals only)
                    let n = 5;
                    for k in 0..n {
                        let a = k as f32 * 1.7 + (ax + az);
                        let rr = 1.4 + (k as f32) * 0.4;
                        let p = Placed { kind: brush, x: ax + a.cos() * rr, z: az + a.sin() * rr, rot: a, scale: b.scale * (0.8 + 0.4 * (k as f32 % 2.0)), param: brush.default_param() };
                        b.map.placed.push(p);
                        self.game.level.add_placed(&p);
                    }
                    b.note(&format!("Scattered {}", brush.name()));
                } else {
                    let p = Placed { kind: brush, x: ax, z: az, rot: b.rot, scale: b.scale, param: b.param };
                    b.map.placed.push(p);
                    self.game.level.add_placed(&p);
                    b.note(&format!("Placed {}", p.kind.name()));
                }
            }
            if delete {
                if let Some((idx, d)) = b
                    .map
                    .placed
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (i, (p.x - ax).powi(2) + (p.z - az).powi(2)))
                    .min_by(|a, c| a.1.partial_cmp(&c.1).unwrap_or(std::cmp::Ordering::Equal))
                {
                    if d < 36.0 {
                        b.map.placed.remove(idx);
                        let keep_cam = self.game.player.cam;
                        self.game = Game::from_custom(&b.map, self.db.loadout, self.db.cosmetics);
                        self.game.player.cam = keep_cam;
                        b.note("Deleted");
                    }
                }
            }
        }
        if undo && !b.map.placed.is_empty() {
            b.map.placed.pop();
            let keep_cam = self.game.player.cam;
            self.game = Game::from_custom(&b.map, self.db.loadout, self.db.cosmetics);
            self.game.player.cam = keep_cam;
            b.note("Undo");
        }
        if save {
            let ok = b.map.save();
            b.note(if ok { "SAVED" } else { "save failed" });
        }
    }
    /// Fold the kills/deaths of the current life into the persistent profile when the player dies.
    fn track_profile(&mut self) {
        self.life_best_streak = self.life_best_streak.max(self.game.streak);
        if self.was_alive && !self.game.player.alive {
            let life_kills = self.game.kills.saturating_sub(self.recorded_kills);
            self.db.profile.record_life(life_kills as u64, self.game.wave, self.life_best_streak);
            self.recorded_kills = self.game.kills;
            self.life_best_streak = 0;
            self.db.save();
        }
        self.was_alive = self.game.player.alive;
    }

    /// The full-screen front-end menu (shown in `Mode::Start`). Returns (start_game, quit, open_settings).
    /// Navigates between Main / Multiplayer / Map Select / Loadout / Profile in place via `self.screen`.
    fn front_end(&mut self, ctx: &egui::Context, _rect: Rect) -> (bool, bool, bool) {
        let mut start_game = false;
        let mut quit = false;
        let mut open_settings = false;
        let gold = Color32::from_rgb(235, 224, 180);
        let cyan = Color32::from_rgb(130, 200, 230);

        egui::Area::new("front_end".into()).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("EARLY-FPS").monospace().size(46.0).color(gold));
                let p = &self.db.profile;
                ui.label(
                    egui::RichText::new(format!("{}   ·   {}   ·   K/D {:.2}   ·   wave {}", p.name, self.db.profile.tier(), p.kd(), p.best_wave))
                        .monospace()
                        .size(13.0)
                        .color(cyan),
                );
                ui.add_space(20.0);

                match self.screen {
                    MenuScreen::Main => {
                        if menu_button(ui, "Single Player").clicked() {
                            self.mp_online = false;
                            self.hunt_creature = None;
                            self.screen = MenuScreen::MapSelect;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "The Hunt").clicked() {
                            self.mp_online = false;
                            if self.hunt_creature.is_none() {
                                self.hunt_creature = Some(Creature::Revenant);
                            }
                            self.screen = MenuScreen::Hunt;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Multiplayer").clicked() {
                            self.hunt_creature = None;
                            self.screen = MenuScreen::Multiplayer;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Loadout").clicked() {
                            self.screen = MenuScreen::Loadout;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Customize").clicked() {
                            self.screen = MenuScreen::Customize;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Map Forge").clicked() {
                            self.build_maps = CustomMap::list();
                            self.screen = MenuScreen::Builder;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Profile").clicked() {
                            self.screen = MenuScreen::Profile;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Settings").clicked() {
                            open_settings = true;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Quit").clicked() {
                            quit = true;
                        }
                    }
                    MenuScreen::Multiplayer => {
                        ui.label(egui::RichText::new("MULTIPLAYER").monospace().size(22.0).color(gold));
                        ui.add_space(10.0);
                        if menu_button(ui, "Online  —  Find Match").clicked() {
                            // No netcode yet: matchmaking is simulated locally vs bots, seeded by skill.
                            self.mp_online = true;
                            self.sel_preset = None;
                            start_game = true;
                        }
                        ui.label(
                            egui::RichText::new(format!("matchmaking by skill {} ({})", self.db.profile.skill(), self.db.profile.tier()))
                                .monospace()
                                .size(11.0)
                                .color(dim()),
                        );
                        ui.add_space(8.0);
                        if menu_button(ui, "Custom Match").clicked() {
                            self.mp_online = false;
                            self.screen = MenuScreen::CustomMatch;
                        }
                        ui.add_space(14.0);
                        if menu_button(ui, "Back").clicked() {
                            self.screen = MenuScreen::Main;
                        }
                    }
                    MenuScreen::MapSelect => {
                        ui.label(egui::RichText::new("SELECT MAP").monospace().size(22.0).color(gold));
                        // ── Crossfading preview of the selected map's areas ──────────────────────────────
                        let key = self.sel_preset.map(|i| i as u64).unwrap_or(u64::MAX);
                        if self.map_preview.as_ref().map(|p| p.key) != Some(key) {
                            let cfg = match self.sel_preset {
                                Some(i) => crate::level::PRESETS[i].config(1234 + i as u64),
                                None => crate::level::MapConfig::varied(777),
                            };
                            self.map_preview = Some(MapPreview::build(key, cfg));
                        }
                        let pw = ui.available_width().min(380.0);
                        let (prect, _) = ui.allocate_exact_size(egui::vec2(pw, 150.0), egui::Sense::hover());
                        if let Some(p) = &self.map_preview {
                            paint_sky_gradient(ui.painter(), prect, p.light.sky, p.fog.color);
                        } else {
                            ui.painter().rect_filled(prect, 4.0, Color32::from_rgb(8, 9, 13));
                        }
                        let t = ui.input(|i| i.time) as f32;
                        if let Some(p) = &self.map_preview {
                            p.paint(&ui.painter_at(prect), prect, t, self.atlas_id);
                        }
                        ui.painter().rect_stroke(prect, 4.0, Stroke::new(1.0, Color32::from_rgb(60, 70, 84)), egui::StrokeKind::Inside);
                        ui.ctx().request_repaint(); // animate the crossfade
                        ui.add_space(4.0);
                        if let Some(i) = self.sel_preset {
                            let pr = &crate::level::PRESETS[i];
                            ui.label(
                                egui::RichText::new(format!("{} · {}×{} · {}", pr.name, pr.size, pr.size, pr.theme.name()))
                                    .monospace()
                                    .size(11.0)
                                    .color(dim()),
                            );
                        }
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                            egui::Grid::new("maps").spacing([8.0, 8.0]).show(ui, |ui| {
                                for (i, pr) in crate::level::PRESETS.iter().enumerate() {
                                    let sel = self.sel_preset == Some(i);
                                    let starred = self.db.map_prefs.contains(&i);
                                    let txt = if starred { format!("\u{2605} {}", pr.name) } else { pr.name.to_string() };
                                    let label = egui::RichText::new(txt).monospace().size(12.0).color(if sel { gold } else if starred { Color32::from_rgb(238, 196, 86) } else { Color32::from_rgb(200, 205, 215) });
                                    if ui
                                        .add_sized([140.0, 28.0], egui::Button::new(label).fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) }))
                                        .clicked()
                                    {
                                        self.sel_preset = Some(i);
                                    }
                                    if i % 4 == 3 {
                                        ui.end_row();
                                    }
                                }
                            });
                        });
                        ui.add_space(6.0);
                        // preference toggle for the selected map
                        if let Some(i) = self.sel_preset {
                            let starred = self.db.map_prefs.contains(&i);
                            let txt = if starred { "\u{2605} Remove from pool" } else { "\u{2606} Add to map pool" };
                            if ui.add_sized([200.0, 26.0], egui::Button::new(egui::RichText::new(txt).monospace().size(13.0))).clicked() {
                                if starred {
                                    self.db.map_prefs.retain(|&x| x != i);
                                } else {
                                    self.db.map_prefs.push(i);
                                }
                                self.db.save();
                            }
                        }
                        ui.label(egui::RichText::new(format!("map pool: {} preferred", self.db.map_prefs.len())).monospace().size(11.0).color(dim()));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let rnd = self.sel_preset.is_none();
                            if ui.add_sized([120.0, 30.0], egui::Button::new(egui::RichText::new("Random").monospace().color(if rnd { gold } else { dim() }))).clicked() {
                                self.sel_preset = None;
                            }
                            if ui.add_sized([120.0, 30.0], egui::Button::new(egui::RichText::new("Deploy").monospace().size(16.0))).clicked() {
                                start_game = true;
                            }
                            // Matchmake: pick from the preferred pool and scale the lobby to the player's rank.
                            if ui.add_sized([150.0, 30.0], egui::Button::new(egui::RichText::new("\u{2605} Matchmake").monospace().size(16.0).color(gold))).clicked() {
                                self.matchmake_deploy();
                                start_game = true;
                            }
                        });
                        ui.add_space(12.0);
                        if menu_button(ui, "Back").clicked() {
                            self.screen = MenuScreen::Main;
                        }
                    }
                    MenuScreen::Loadout => {
                        loadout_page(ui, &mut self.db.loadout, gold, cyan);
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("ARSENAL — LEGENDARY PRIMARY").monospace().size(13.0).color(cyan));
                        ui.label(egui::RichText::new("Rolls two traits + a unique look; overrides your primary.").monospace().size(11.0).color(dim()));
                        ui.add_space(4.0);
                        let cur = self.db.loadout.legendary;
                        if ui.add_sized([150.0, 24.0], egui::Button::new(egui::RichText::new("Standard (none)").monospace().size(12.0).color(if cur.is_none() { gold } else { Color32::from_rgb(196, 202, 212) })).fill(if cur.is_none() { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                            self.db.loadout.legendary = None;
                        }
                        ui.add_space(2.0);
                        egui::Grid::new("arsenal").spacing([6.0, 4.0]).show(ui, |ui| {
                            for (i, lg) in crate::legendary::POOL.iter().enumerate() {
                                let sel = cur == Some(i);
                                let txt = egui::RichText::new(lg.name).monospace().size(13.0).color(if sel { lg.accent.color() } else { Color32::from_rgb(200, 206, 214) });
                                if ui.add_sized([108.0, 26.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(46, 40, 56) } else { Color32::from_rgb(24, 28, 34) }).stroke(egui::Stroke::new(if sel { 1.5 } else { 0.0 }, lg.accent.color()))).clicked() {
                                    self.db.loadout.legendary = Some(i);
                                }
                                if i % 3 == 2 {
                                    ui.end_row();
                                }
                            }
                        });
                        if let Some(lg) = self.db.loadout.legend() {
                            ui.add_space(3.0);
                            ui.label(egui::RichText::new(format!("{} · {}", lg.base.def().name, "legendary")).monospace().size(11.0).color(lg.accent.color()));
                            ui.label(egui::RichText::new(format!("• {} — {}", lg.perk1.name(), lg.perk1.desc())).monospace().size(11.0).color(dim()));
                            ui.label(egui::RichText::new(format!("• {} — {}", lg.perk2.name(), lg.perk2.desc())).monospace().size(11.0).color(dim()));
                            ui.label(egui::RichText::new(format!("dmg ×{:.2}   reload ×{:.2}", lg.damage_mul(), lg.reload_mul())).monospace().size(11.0).color(cyan));
                        }
                        ui.add_space(10.0);
                        if menu_button(ui, "Save & Back").clicked() {
                            self.db.save();
                            self.screen = MenuScreen::Main;
                        }
                    }
                    MenuScreen::Customize => {
                        self.armory_panel(ui, gold, cyan);
                        ui.add_space(10.0);
                        customize_page(ui, &mut self.db.cosmetics, gold, cyan);
                        ui.add_space(8.0);
                        if menu_button(ui, "Save & Back").clicked() {
                            self.db.save();
                            self.screen = MenuScreen::Main;
                        }
                    }
                    MenuScreen::CustomMatch => {
                        ui.label(egui::RichText::new("CUSTOM MATCH").monospace().size(22.0).color(gold));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(format!("Bots: {}", self.custom_bots)).monospace().size(15.0).color(cyan));
                        ui.add(egui::Slider::new(&mut self.custom_bots, 1..=40).text("count"));
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Bot Skill").monospace().size(13.0).color(cyan));
                        ui.horizontal(|ui| {
                            for diff in Difficulty::ALL {
                                let sel = self.custom_diff == diff;
                                let txt = egui::RichText::new(diff.name()).monospace().size(13.0).color(if sel { gold } else { dim() });
                                if ui.add_sized([100.0, 28.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                                    self.custom_diff = diff;
                                }
                            }
                        });
                        ui.add_space(12.0);
                        // Cinematic tuning: how hard the recursive enhancer pushes the chosen map toward a
                        // livelier, more varied, more cinematic layout before play.
                        let tuning_label = crate::enhance::CinematicTuning(self.cinematic_tuning).label();
                        ui.label(egui::RichText::new(format!("Cinematic Tuning: {}", tuning_label)).monospace().size(15.0).color(cyan));
                        ui.add(egui::Slider::new(&mut self.cinematic_tuning, 0.0..=1.0).text("intensity").show_value(false))
                            .on_hover_text("Iteratively enhances the map for a livelier, more cinematic feel.\nOff = the map exactly as authored. Higher = denser cover, more motion, a bolder focal landmark.");
                        ui.add_space(12.0);
                        if menu_button(ui, "Choose Map →").clicked() {
                            self.screen = MenuScreen::MapSelect;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Back").clicked() {
                            self.screen = MenuScreen::Multiplayer;
                        }
                    }
                    MenuScreen::Hunt => {
                        ui.label(egui::RichText::new("THE HUNT").monospace().size(24.0).color(gold));
                        ui.add_space(6.0);
                        // Role: play the creature, or one of the hunted.
                        ui.horizontal(|ui| {
                            for (r, label) in [(HuntRole::Predator, "Predator"), (HuntRole::Survivor, "Survivor")] {
                                let sel = self.hunt_role == r;
                                let txt = egui::RichText::new(label).monospace().size(15.0).color(if sel { gold } else { dim() });
                                if ui.add_sized([130.0, 30.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(60, 40, 48) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                                    self.hunt_role = r;
                                }
                            }
                        });
                        ui.add_space(8.0);
                        let predator = self.hunt_role == HuntRole::Predator;
                        if predator {
                            ui.label(egui::RichText::new("You are the predator. The others run and hide. Hunt them all.").monospace().size(12.0).color(dim()));
                        } else {
                            ui.label(egui::RichText::new("You are one of the hunted. Survive the creature with three others until dawn.").monospace().size(12.0).color(dim()));
                        }
                        ui.add_space(10.0);
                        let cur = self.hunt_creature.unwrap_or(Creature::Revenant);
                        ui.label(egui::RichText::new(if predator { "CHOOSE YOUR CREATURE" } else { "THE CREATURE YOU FACE" }).monospace().size(13.0).color(cyan));
                        ui.add_space(4.0);
                        for c in Creature::ALL {
                            let sel = cur == c;
                            let txt = egui::RichText::new(c.name()).monospace().size(16.0).color(if sel { gold } else { Color32::from_rgb(200, 206, 214) });
                            if ui.add_sized([260.0, 32.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(60, 40, 48) } else { Color32::from_rgb(26, 28, 34) })).clicked() {
                                self.hunt_creature = Some(c);
                            }
                            if sel {
                                ui.label(egui::RichText::new(c.desc()).monospace().size(11.0).color(dim()));
                            }
                            ui.add_space(3.0);
                        }
                        ui.add_space(8.0);
                        if predator {
                            ui.label(egui::RichText::new(format!("Prey: {}", self.custom_bots)).monospace().size(13.0).color(cyan));
                            ui.add(egui::Slider::new(&mut self.custom_bots, 4..=40).text("count"));
                        } else {
                            ui.label(egui::RichText::new("Survivors: you + 3 AI").monospace().size(12.0).color(dim()));
                        }
                        ui.add_space(10.0);
                        if menu_button(ui, "Choose Hunting Ground →").clicked() {
                            self.screen = MenuScreen::MapSelect;
                        }
                        ui.add_space(6.0);
                        if menu_button(ui, "Back").clicked() {
                            self.hunt_creature = None;
                            self.screen = MenuScreen::Main;
                        }
                    }
                    MenuScreen::Builder => {
                        ui.label(egui::RichText::new("MAP FORGE").monospace().size(22.0).color(gold));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Author an open arena, place assets, save and replay it.").monospace().size(12.0).color(dim()));
                        ui.add_space(12.0);

                        // New map: pick a biome base + size.
                        ui.label(egui::RichText::new("NEW MAP").monospace().size(14.0).color(cyan));
                        ui.add_space(4.0);
                        let themes = MapTheme::ALL;
                        let cur = themes[self.build_theme % themes.len()];
                        ui.horizontal(|ui| {
                            if ui.add_sized([34.0, 26.0], egui::Button::new(egui::RichText::new("◀").monospace())).clicked() {
                                self.build_theme = (self.build_theme + themes.len() - 1) % themes.len();
                            }
                            ui.label(egui::RichText::new(format!("{:^12}", cur.name())).monospace().size(16.0).color(gold));
                            if ui.add_sized([34.0, 26.0], egui::Button::new(egui::RichText::new("▶").monospace())).clicked() {
                                self.build_theme = (self.build_theme + 1) % themes.len();
                            }
                        });
                        ui.label(egui::RichText::new(format!("Size: {}", self.build_size)).monospace().size(12.0).color(cyan));
                        ui.add(egui::Slider::new(&mut self.build_size, 96..=224).text("tiles"));
                        ui.add_space(6.0);
                        if menu_button(ui, "Create & Edit  →").clicked() {
                            let n = self.build_maps.len() + 1;
                            let map = CustomMap::new(&format!("My Map {}", n), cur, self.build_size);
                            self.enter_builder(ctx, map);
                        }

                        ui.add_space(14.0);
                        ui.label(egui::RichText::new("SAVED MAPS").monospace().size(14.0).color(cyan));
                        ui.add_space(4.0);
                        if self.build_maps.is_empty() {
                            ui.label(egui::RichText::new("(none yet — create one above)").monospace().size(11.0).color(dim()));
                        }
                        let names = self.build_maps.clone();
                        for name in names {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("{:<18}", name)).monospace().size(14.0).color(Color32::from_rgb(206, 212, 220)));
                                if ui.add_sized([70.0, 26.0], egui::Button::new(egui::RichText::new("Edit").monospace())).clicked() {
                                    if let Some(m) = CustomMap::load(&name) {
                                        self.enter_builder(ctx, m);
                                    }
                                }
                                if ui.add_sized([70.0, 26.0], egui::Button::new(egui::RichText::new("Play").monospace())).clicked() {
                                    if let Some(m) = CustomMap::load(&name) {
                                        self.game = Game::play_custom(&m, self.next_seed(), self.db.loadout, self.db.cosmetics, self.custom_bots, self.custom_diff);
                                        self.build = None;
                                        self.begin_play(ctx);
                                    }
                                }
                            });
                            ui.add_space(2.0);
                        }
                        ui.add_space(12.0);
                        if menu_button(ui, "Back").clicked() {
                            self.screen = MenuScreen::Main;
                        }
                    }
                    MenuScreen::Profile => {
                        let p = &self.db.profile;
                        ui.label(egui::RichText::new("PROFILE").monospace().size(22.0).color(gold));
                        ui.add_space(8.0);
                        // ── Rank emblem ──────────────────────────────────────────────────────────────
                        let rk = crate::rank::rank_for(p);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(rk.tier.symbol()).size(40.0).color(rk.tier.color()));
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(rk.label()).monospace().size(20.0).color(rk.tier.color()));
                                ui.label(egui::RichText::new(format!("MMR {}", rk.rating)).monospace().size(12.0).color(Color32::from_rgb(180, 186, 196)));
                            });
                        });
                        // progress to next division/tier
                        ui.add(egui::ProgressBar::new(rk.progress).desired_width(240.0).fill(rk.tier.color()));
                        let lobby = crate::rank::matchmake(rk.rating);
                        ui.label(egui::RichText::new(format!("matchmaking: {} bots · {} · band {}–{}", lobby.bots, lobby.difficulty.name(), lobby.band.0, lobby.band.1)).monospace().size(11.0).color(Color32::from_rgb(150, 156, 166)));
                        ui.add_space(10.0);
                        for (k, v) in [
                            ("K/D", format!("{:.2}", p.kd())),
                            ("Kills", p.kills.to_string()),
                            ("Deaths", p.deaths.to_string()),
                            ("Matches", p.matches.to_string()),
                            ("Best streak", p.best_streak.to_string()),
                            ("Best wave", p.best_wave.to_string()),
                        ] {
                            ui.label(egui::RichText::new(format!("{:<14}{}", k, v)).monospace().size(15.0).color(Color32::from_rgb(205, 210, 220)));
                        }
                        ui.add_space(14.0);
                        if menu_button(ui, "Back").clicked() {
                            self.screen = MenuScreen::Main;
                        }
                    }
                }
            });
        });

        (start_game, quit, open_settings)
    }

    fn set_grab(&mut self, ctx: &egui::Context, on: bool) {
        self.grabbed = on;
        let grab = if on { egui::CursorGrab::Locked } else { egui::CursorGrab::None };
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(grab));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(!on));
    }

    /// Switch mode and reconcile the mouse grab (grabbed only while playing).
    fn goto(&mut self, ctx: &egui::Context, mode: Mode) {
        if mode != Mode::Settings {
            self.rebinding = None;
        }
        self.mode = mode;
        self.set_grab(ctx, mode == Mode::Play);
    }

    /// Read movement/look/fire/weapon input for a play frame.
    fn gather_play_input(&self, ctx: &egui::Context) -> (Input, Vec2) {
        ctx.input(|i| {
            let b = &self.settings.bindings;
            let mut inp = Input::default();
            if i.key_down(b.forward) {
                inp.fwd += 1.0;
            }
            if i.key_down(b.back) {
                inp.fwd -= 1.0;
            }
            if i.key_down(b.right) {
                inp.strafe += 1.0;
            }
            if i.key_down(b.left) {
                inp.strafe -= 1.0;
            }
            inp.jump = i.key_down(b.jump);
            inp.fire = i.pointer.primary_down();
            inp.ads = i.pointer.secondary_down();
            // Sprint no longer lives on shift — shift is now crawl/prone by default (per the movement rework).
            inp.sprint = i.key_down(b.sprint);
            inp.crouch = i.key_down(b.crouch);
            // Crawl/prone: shift by default, or the bound crawl key.
            inp.crawl = i.modifiers.shift || i.key_down(b.crawl);
            inp.dive = i.key_pressed(b.dive);
            inp.reload = i.key_pressed(b.reload);
            inp.deploy_uav = i.key_pressed(Key::G);
            inp.deploy_sentry = i.key_pressed(Key::H);
            inp.deploy_strike = i.key_pressed(Key::F);
            inp.throw_frag = i.key_pressed(Key::T);
            inp.activate_super = i.key_pressed(Key::Z);

            // Weapon select 1..4 (number row is identical across layouts).
            if i.key_pressed(Key::Num1) {
                inp.want_slot = Some(0);
            }
            if i.key_pressed(Key::Num2) {
                inp.want_slot = Some(1);
            }
            if i.key_pressed(Key::Num3) {
                inp.want_slot = Some(2);
            }
            if i.key_pressed(Key::Num4) {
                inp.want_slot = Some(3);
            }
            // Mouse-wheel weapon cycling (discrete notches).
            let mut wheel = 0.0;
            for ev in &i.events {
                if let egui::Event::MouseWheel { delta, .. } = ev {
                    wheel += delta.y;
                }
            }
            if wheel.abs() > 0.1 {
                let cur = self.game.player.current as i32;
                let dir = if wheel < 0.0 { 1 } else { -1 };
                inp.want_slot = Some((cur + dir).rem_euclid(4) as usize);
            }

            // Raw mouse motion (works while the cursor is locked); fall back to pointer delta.
            let mut look = Vec2::ZERO;
            for ev in &i.events {
                if let egui::Event::MouseMoved(d) = ev {
                    look += *d;
                }
            }
            if look == Vec2::ZERO {
                look = i.pointer.delta();
            }
            inp.fly = self.build.as_ref().map(|b| b.fly).unwrap_or(false);
            (inp, look)
        })
    }

    /// First non-Escape key pressed this frame, used to capture a new binding.
    fn captured_key(ctx: &egui::Context) -> Option<Key> {
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Key { key, pressed: true, .. } = ev {
                    if *key != Key::Escape {
                        return Some(*key);
                    }
                }
            }
            None
        })
    }
}

impl eframe::App for FpsApp {
    /// The skybox: with no more flat ceiling geometry, whatever egui hasn't painted over is what the player
    /// sees above the world, so this has to be the level's actual sky colour (time-of-day, per-biome) rather
    /// than a fixed black — otherwise every map would show a black void overhead instead of open sky.
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        let s = self.game.light.sky;
        [s.r() as f32 / 255.0, s.g() as f32 / 255.0, s.b() as f32 / 255.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Persist the renderer preference the moment it's toggled on the settings page (it's read at launch).
        if self.settings.gpu_renderer != self.db.gpu_renderer {
            self.db.gpu_renderer = self.settings.gpu_renderer;
            self.db.save();
        }

        // Menu keyboard navigation: in the front-end menus, the forge Esc-menu, and the settings/pause pages,
        // arrow keys and vim h/j/k/l drive widget focus (and Enter activates) so the UI is usable without a
        // mouse. Injected as egui focus events before any panel is laid out this frame.
        self.handle_menu_nav(&ctx);

        // Build (or rebuild) the material texture atlas at the graphics preset's resolution. Rebuilds only
        // when the preset's tile size actually changes, so switching Low↔Ultra re-bakes crisper/coarser
        // textures. Remember its id for the renderer.
        let want_tile = self.settings.graphics_preset.settings().atlas_tile;
        if self.atlas.is_none() || self.atlas_tile != want_tile {
            let (rgba, w, h) = crate::mesh::build_material_atlas(want_tile, 5);
            let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            let opts = egui::TextureOptions {
                magnification: egui::TextureFilter::Linear,
                minification: egui::TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::ClampToEdge,
                // Mipmaps: without them, the atlas's fine grain/bevel detail aliases into shimmer on distant
                // walls and floors (classic minification moire) — the mip chain lets far surfaces sample a
                // pre-blurred version instead of undersampling the full-res texture.
                mipmap_mode: Some(egui::TextureFilter::Linear),
            };
            let tex = ctx.load_texture("material_atlas", img, opts);
            self.atlas_id = tex.id();
            self.atlas = Some(tex);
            self.atlas_tile = want_tile;
        }

        if self.first {
            self.first = false;
            // Stay on the start menu with the cursor free until the player hits Play.
            self.set_grab(&ctx, false);
        }

        let esc = ctx.input(|i| i.key_pressed(Key::Escape));

        // ── Rebinding capture takes priority over everything else ────────────────
        if let Some(action) = self.rebinding {
            if esc {
                self.rebinding = None; // Escape cancels the capture.
            } else if let Some(k) = Self::captured_key(&ctx) {
                self.settings.bindings.set(action, k);
                self.rebinding = None;
            }
        } else if esc {
            // Esc cycles Play -> Pause, Settings -> Pause, Pause -> Play; the start menu ignores it.
            match self.mode {
                Mode::Play => {
                    if self.build.is_some() {
                        // In the forge, Esc toggles the forge menu overlay (giving the mouse access to
                        // Resume / Keybinds / Save & Exit) rather than dumping you straight out of the editor.
                        self.forge_menu_open = !self.forge_menu_open;
                        self.set_grab(&ctx, !self.forge_menu_open && !self.build.as_ref().map_or(false, |b| b.fly));
                        // when the menu opens, always free the cursor so it can click the overlay
                        if self.forge_menu_open {
                            self.set_grab(&ctx, false);
                        }
                    } else {
                        self.goto(&ctx, Mode::Pause);
                    }
                }
                Mode::Pause => self.goto(&ctx, Mode::Play),
                Mode::Settings => self.goto(&ctx, Mode::Pause),
                Mode::Start => {}
            }
        }

        let focused = ctx.input(|i| i.focused);

        // ── Simulate (Play only) ─────────────────────────────────────────────────
        if self.mode == Mode::Play && focused {
            if ctx.input(|i| i.key_pressed(Key::V)) {
                self.settings.third_person = !self.settings.third_person;
            }
            let (mut input, look) = self.gather_play_input(&ctx);
            // Zoom-scaled look: the further the view is zoomed in (ADS), the slower the aim moves, so fine
            // adjustments at high magnification stay controllable. Factor = how much the FOV has narrowed.
            let base = self.settings.fov_deg.max(1.0);
            let ads = self.game.player.ads_t;
            let wfov = self.game.player.weapon().kind.def().ads_fov;
            let cur = base * (1.0 - ads) + wfov * ads;
            let zoom_look = ((cur * 0.5).to_radians().tan() / (base * 0.5).to_radians().tan()).clamp(0.18, 1.0);
            input.dyaw = look.x * self.settings.sensitivity * zoom_look;
            let sign = if self.settings.invert_y { 1.0 } else { -1.0 };
            input.dpitch = sign * look.y * self.settings.sensitivity * zoom_look;

            let dt = ctx.input(|i| i.stable_dt).min(0.1);
            self.accum += dt;
            let mut steps = 0;
            while self.accum >= FIXED_DT && steps < 8 {
                self.game.step(&input, FIXED_DT);
                input.dyaw = 0.0; // look is applied once per frame
                input.dpitch = 0.0;
                input.want_slot = None;
                self.accum -= FIXED_DT;
                steps += 1;
            }
            self.track_profile();
            // Drain queued sound events every frame (keeps the queue bounded); play them if sound is on.
            let events = self.game.take_sfx();
            #[cfg(feature = "sound")]
            self.play_sfx(&events);
            #[cfg(not(feature = "sound"))]
            let _ = events;
            if self.build.is_some() {
                if self.forge_menu_open {
                    self.forge_menu_panel(&ctx);
                } else {
                    self.builder_tick(&ctx);
                }
                self.build_panel(&ctx);
            }
        }

        // ── Draw the world ───────────────────────────────────────────────────────
        let rect = ui.max_rect();
        let (w, h) = (rect.width(), rect.height());
        let painter = ui.painter_at(rect);

        paint_backdrop(&painter, rect, &self.game);

        let cam = self.game.player.view_cam();
        let (sx, sy) = self.game.shake_offset();
        let cx = rect.center().x + sx;
        let cy = rect.center().y + sy;
        // Aim-down-sights zooms the FOV toward the weapon's scoped value.
        let ads = self.game.player.ads_t;
        // Third person (toggle with V) when not aiming: pull the camera back so the player sees their
        // customised character. ADS snaps back to first person for a clean aim.
        let tp_active = self.settings.third_person && ads < 0.4;
        let cam = if tp_active {
            let mut tc = cam;
            let fwd = cam.forward();
            tc.pos = cam.pos - fwd * 3.4 + crate::math::Vec3::new(0.0, 0.45, 0.0);
            tc
        } else {
            cam
        };
        let wfov = self.game.player.weapon().kind.def().ads_fov;
        let mut fov = self.settings.fov_deg * (1.0 - ads) + wfov * ads;
        // Camera feel: sprinting widens the lens for a sense of speed; firing punches it a touch; a dive
        // flares it out. All eased by how aimed you are.
        let pl = &self.game.player;
        let sprint_kick = if pl.sprinting { 9.0 } else { 0.0 };
        let dive_kick = (pl.dive_t * 2.0).min(1.0) * 7.0;
        let fire_kick = pl.recoil * 26.0;
        fov += (sprint_kick + dive_kick + fire_kick) * (1.0 - ads);
        let scale = scale_for_fov(h, fov.to_radians());

        if self.game.has_lod() {
            self.game.lod_update(&cam, 24);
        }
        let meshes = self.game.frame_meshes(&cam, tp_active);
        let gset = self.settings.graphics_preset.settings();
        self.game.shadow_blobs = gset.shadows != crate::graphics::ShadowQuality::Off;
        self.game.particles.set_cap((2200.0 * gset.particle_scale) as usize);
        self.game.hostile_color = self.settings.hostile_color;
        self.game.friendly_color = self.settings.friendly_color;
        let mut dyn_lights = self.game.dynamic_lights();
        if dyn_lights.len() > gset.max_dynamic_lights {
            // keep the strongest N per the quality preset (Low = 0 → no dynamic lights at all)
            dyn_lights.sort_by(|a, b| b.intensity.partial_cmp(&a.intensity).unwrap_or(std::cmp::Ordering::Equal));
            dyn_lights.truncate(gset.max_dynamic_lights);
        }
        // Apply the preset's fog draw-distance scale (Ultra sees further, Low pulls the fog in for speed).
        let mut render_fog = self.game.fog;
        render_fog.start *= gset.fog_scale;
        render_fog.end *= gset.fog_scale;
        let mesh = crate::render::render(&meshes, &cam, cx, cy, scale, &self.game.light, &render_fog, w, h, self.game.time, self.game.wind(), self.atlas_id, 5, &dyn_lights);
        painter.add(egui::Shape::mesh(mesh));

        // HUD (always shown; dimmed by the menu overlay when paused).
        draw_damage_vignette(&painter, rect, &self.game);
        draw_hit_flash(&painter, rect, &self.game);
        draw_underwater_tint(&painter, rect, &self.game);
        if self.mode == Mode::Play {
            // UAV enemy markers project into the world view.
            if self.game.uav_timer > 0.0 {
                draw_uav_markers(&painter, rect, &self.game, &cam, cx, cy, scale);
            }
            // Sniper scope overlay when aimed; otherwise a dynamic reticle + hitmarker.
            let scoped = self.game.player.weapon().kind.def().scoped;
            if scoped && ads > 0.6 {
                draw_scope(&painter, rect, ads);
            } else {
                draw_reticle(&painter, rect, &self.game, ads);
            }
            draw_hitmarker(&painter, rect, &self.game);
            draw_damage_arrows(&painter, rect, &self.game);
        }
        draw_hud(&painter, rect, &self.game, &self.settings);
        if let Some(b) = &self.build {
            draw_builder_hud(&painter, rect, b);
        }
        draw_scorestreaks(&painter, rect, &self.game);
        draw_compass(&painter, rect, &self.game);
        draw_killfeed(&painter, rect, &self.game);
        if self.settings.show_minimap {
            draw_minimap(&painter, rect, &self.game);
        }
        if !self.game.player.alive {
            draw_center_banner(&painter, rect, "YOU WERE KILLED", Color32::from_rgb(220, 50, 50));
        }

        // ── Menus ────────────────────────────────────────────────────────────────
        if self.mode != Mode::Play {
            painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(6, 8, 12, 170));
        }

        let mut next_mode: Option<Mode> = None;
        let mut start_game = false;
        let mut quit = false;
        match self.mode {
            Mode::Start => {
                let (sg, q, open_settings) = self.front_end(&ctx, rect);
                start_game = sg;
                quit = q;
                if open_settings {
                    next_mode = Some(Mode::Settings);
                }
            }
            Mode::Pause => {
                egui::Window::new("pause_menu")
                    .title_bar(false)
                    .resizable(false)
                    .collapsible(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .frame(menu_frame())
                    .show(&ctx, |ui| {
                        ui.set_width(240.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("PAUSED").monospace().size(30.0).color(Color32::from_rgb(235, 220, 120)));
                            ui.add_space(14.0);
                            if menu_button(ui, "Resume").clicked() {
                                next_mode = Some(Mode::Play);
                            }
                            ui.add_space(6.0);
                            if menu_button(ui, "Settings").clicked() {
                                next_mode = Some(Mode::Settings);
                            }
                            ui.add_space(6.0);
                            if menu_button(ui, "Quit to Menu").clicked() {
                                next_mode = Some(Mode::Start);
                            }
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("Esc to resume").monospace().size(12.0).color(dim()));
                            ui.add_space(4.0);
                        });
                    });
            }
            Mode::Settings => {
                let settings = &mut self.settings;
                let rebinding = &mut self.rebinding;
                let from = self.settings_from;
                egui::Window::new("settings_menu")
                    .title_bar(false)
                    .resizable(false)
                    .collapsible(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .frame(menu_frame())
                    .show(&ctx, |ui| {
                        settings_page(ui, settings, rebinding, &mut next_mode, from);
                    });
            }
            Mode::Play => {}
        }
        // Remember where Settings was opened from, so its Back button returns there.
        if next_mode == Some(Mode::Settings) {
            self.settings_from = self.mode;
        }
        if start_game {
            self.start_new_game(&ctx);
        } else if let Some(m) = next_mode {
            self.goto(&ctx, m);
        }
        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        ctx.request_repaint();
    }
}

// ── Menu widgets ─────────────────────────────────────────────────────────────────

fn menu_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(18, 22, 28, 245))
        .stroke(Stroke::new(1.0, Color32::from_rgb(70, 84, 98)))
        .inner_margin(egui::Margin::same(18))
        .corner_radius(6)
}

/// The builder's on-screen overlay: the active brush, its rotation/scale, the placed count, the controls,
/// a transient status line, and a small crosshair marker.
fn draw_builder_hud(p: &egui::Painter, rect: Rect, b: &BuildState) {
    let gold = Color32::from_rgb(235, 224, 180);
    let cyan = Color32::from_rgb(150, 210, 235);
    let dim = Color32::from_rgb(170, 178, 188);
    let x = rect.left() + 16.0;
    let mut y = rect.top() + 16.0;
    p.text(
        Pos2::new(x, y),
        Align2::LEFT_TOP,
        format!("FORGE — {}", b.map.name),
        FontId::monospace(18.0),
        gold,
    );
    y += 24.0;
    p.text(
        Pos2::new(x, y),
        Align2::LEFT_TOP,
        format!("Brush: {}   rot {:>3}°   scale {:.1}x", b.brush().name(), (b.rot.to_degrees().round() as i32) % 360, b.scale),
        FontId::monospace(15.0),
        cyan,
    );
    y += 20.0;
    p.text(
        Pos2::new(x, y),
        Align2::LEFT_TOP,
        format!("Placed: {}   snap {}   scatter {}", b.map.placed.len(), if b.snap { "ON" } else { "off" }, if b.scatter { "ON" } else { "off" }),
        FontId::monospace(13.0),
        dim,
    );
    y += 22.0;
    for line in [
        "LMB place   RMB delete   Z undo",
        "Tab/`  next/prev asset",
        ",/.  rotate    ;/'  scale",
        "G grid-snap   B scatter brush",
        "Enter save    Esc save & exit",
    ] {
        p.text(Pos2::new(x, y), Align2::LEFT_TOP, line, FontId::monospace(12.0), dim);
        y += 16.0;
    }
    // transient status (e.g. SAVED)
    if b.status_t > 0.0 {
        p.text(
            rect.center_top() + egui::vec2(0.0, 40.0),
            Align2::CENTER_TOP,
            &b.status,
            FontId::monospace(20.0),
            gold,
        );
    }
    // ground-aim marker at the crosshair
    let c = rect.center();
    p.circle_stroke(c, 7.0, Stroke::new(1.5, Color32::from_rgb(120, 230, 160)));
    p.line_segment([c - egui::vec2(11.0, 0.0), c + egui::vec2(11.0, 0.0)], Stroke::new(1.0, Color32::from_rgb(120, 230, 160)));
    p.line_segment([c - egui::vec2(0.0, 11.0), c + egui::vec2(0.0, 11.0)], Stroke::new(1.0, Color32::from_rgb(120, 230, 160)));
}

/// Ray-march the player's look vector down onto the heightfield and return the (x,z) ground point under
/// the crosshair, or `None` if aiming above the horizon. Used by the map editor to place/delete assets.
fn aim_ground(g: &Game) -> Option<(f32, f32)> {
    let eye = g.player.eye();
    let dir = g.player.cam.forward();
    if dir.y > -0.02 {
        return None; // looking flat or up — nowhere on the ground to point at
    }
    let mut t = 0.4_f32;
    while t < 140.0 {
        let p = eye + dir * t;
        if p.y <= g.level.floor_h(p.x, p.z) {
            return Some((p.x, p.z));
        }
        t += 0.4;
    }
    None
}

fn menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized([200.0, 30.0], egui::Button::new(egui::RichText::new(text).monospace().size(16.0)))
}

/// Reticle / gun-skin / armor customization page.
fn customize_page(ui: &mut egui::Ui, c: &mut crate::cosmetics::Cosmetics, gold: Color32, cyan: Color32) {
    ui.label(egui::RichText::new("CUSTOMIZE").monospace().size(22.0).color(gold));
    ui.add_space(6.0);

    let pick = |ui: &mut egui::Ui, label: &str, sel: bool, gold: Color32| -> bool {
        let txt = egui::RichText::new(label).monospace().size(13.0).color(if sel { gold } else { Color32::from_rgb(196, 202, 212) });
        ui.add_sized([108.0, 26.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked()
    };

    ui.label(egui::RichText::new("RETICLE").monospace().size(13.0).color(cyan));
    egui::Grid::new("ret").spacing([6.0, 6.0]).show(ui, |ui| {
        for (i, r) in Reticle::ALL.iter().enumerate() {
            if pick(ui, r.name(), c.reticle == *r, gold) {
                c.reticle = *r;
            }
            if i % 4 == 3 {
                ui.end_row();
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("colour").monospace().size(12.0).color(dim()));
        for a in Accent::ALL {
            let sel = c.reticle_color == a;
            if ui.add_sized([18.0, 18.0], egui::Button::new("").fill(a.color()).stroke(egui::Stroke::new(if sel { 2.0 } else { 0.0 }, gold))).clicked() {
                c.reticle_color = a;
            }
        }
    });

    ui.add_space(8.0);
    ui.label(egui::RichText::new("WEAPON SKIN").monospace().size(13.0).color(cyan));
    egui::Grid::new("skin").spacing([6.0, 6.0]).show(ui, |ui| {
        for (i, s) in GunSkin::ALL.iter().enumerate() {
            if pick(ui, s.name(), c.skin == *s, gold) {
                c.skin = *s;
            }
            if i % 4 == 3 {
                ui.end_row();
            }
        }
    });

    ui.add_space(8.0);
    ui.label(egui::RichText::new("ARMOR FRAME").monospace().size(13.0).color(cyan));
    egui::Grid::new("armor").spacing([6.0, 6.0]).show(ui, |ui| {
        for (i, f) in ArmorFrame::ALL.iter().enumerate() {
            let resp_clicked = pick(ui, f.name(), c.armor == *f, gold);
            if resp_clicked {
                c.armor = *f;
            }
            if i % 3 == 2 {
                ui.end_row();
            }
        }
    });
    ui.label(egui::RichText::new(c.armor.desc()).monospace().size(11.0).color(dim()));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("colour").monospace().size(12.0).color(dim()));
        for a in Accent::ALL {
            let sel = c.armor_color == a;
            if ui.add_sized([18.0, 18.0], egui::Button::new("").fill(a.color()).stroke(egui::Stroke::new(if sel { 2.0 } else { 0.0 }, gold))).clicked() {
                c.armor_color = a;
            }
        }
    });
}

/// The loadout builder: pick a primary weapon and up to three perks (the sidearm is a fixed pistol).
fn loadout_page(ui: &mut egui::Ui, l: &mut Loadout, gold: Color32, cyan: Color32) {
    ui.label(egui::RichText::new("LOADOUT").monospace().size(22.0).color(gold));
    ui.add_space(8.0);

    // ── Class pick (Star Jumper): health pool + grenade + super ─────────────────────────────────────────
    ui.label(egui::RichText::new("CLASS").monospace().size(13.0).color(cyan));
    egui::Grid::new("classes").spacing([6.0, 6.0]).show(ui, |ui| {
        for c in crate::class::ClassKind::ALL {
            let sel = l.class == c;
            let txt = egui::RichText::new(c.name()).monospace().size(14.0).color(if sel { gold } else { Color32::from_rgb(200, 205, 215) });
            if ui.add_sized([120.0, 28.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                l.class = c;
            }
        }
    });
    {
        let cd = l.class.def();
        let gren = match cd.grenade {
            crate::class::GrenadeKind::Frag => "frag",
            crate::class::GrenadeKind::Gravity => "gravity well",
            crate::class::GrenadeKind::Acid => "acid pool",
        };
        let sup = match cd.super_kind {
            crate::class::SuperKind::None => "no super",
            crate::class::SuperKind::ShoulderCannons => "shoulder cannons",
            crate::class::SuperKind::GravitySlam => "gravity slam",
        };
        ui.label(egui::RichText::new(format!("{} HP · {} · {} (X)", cd.max_health as i32, gren, sup)).monospace().size(11.0).color(dim()));
    }
    ui.add_space(10.0);

    ui.label(egui::RichText::new("PRIMARY").monospace().size(13.0).color(cyan));
    egui::Grid::new("primaries").spacing([6.0, 6.0]).show(ui, |ui| {
        for (i, w) in WeaponKind::PRIMARIES.iter().enumerate() {
            let sel = l.primary == *w;
            let txt = egui::RichText::new(w.display()).monospace().size(14.0).color(if sel { gold } else { Color32::from_rgb(200, 205, 215) });
            if ui.add_sized([120.0, 28.0], egui::Button::new(txt).fill(if sel { Color32::from_rgb(44, 54, 64) } else { Color32::from_rgb(24, 28, 34) })).clicked() {
                l.primary = *w;
            }
            if i % 3 == 2 {
                ui.end_row();
            }
        }
    });
    let d = l.primary.def();
    ui.label(
        egui::RichText::new(format!("{} · {} dmg · {}-rnd mag", l.primary.class(), d.damage as i32, d.mag_size))
            .monospace()
            .size(11.0)
            .color(dim()),
    );

    ui.add_space(10.0);
    let count = l.perks.iter().filter(|p| p.is_some()).count();
    ui.label(egui::RichText::new(format!("PERKS  ({}/3)", count)).monospace().size(13.0).color(cyan));
    egui::Grid::new("perks").spacing([6.0, 6.0]).show(ui, |ui| {
        for (i, perk) in Perk::ALL.iter().enumerate() {
            let on = l.has(*perk);
            let txt = egui::RichText::new(format!("{}  {}", perk.icon(), perk.name())).monospace().size(13.0).color(if on { gold } else { Color32::from_rgb(190, 195, 205) });
            let resp = ui.add_sized([170.0, 28.0], egui::Button::new(txt).fill(if on { Color32::from_rgb(46, 58, 50) } else { Color32::from_rgb(24, 28, 34) }));
            if resp.clicked() {
                l.toggle_perk(*perk);
            }
            resp.on_hover_text(perk.desc());
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
}

fn settings_page(ui: &mut egui::Ui, s: &mut Settings, rebinding: &mut Option<BindAction>, next_mode: &mut Option<Mode>, return_to: Mode) {
    ui.set_width(340.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("SETTINGS").monospace().size(26.0).color(Color32::from_rgb(235, 220, 120)));
    });
    ui.add_space(12.0);

    section(ui, "MOUSE");
    ui.add(egui::Slider::new(&mut s.sensitivity, 0.0006..=0.006).logarithmic(true).text("Sensitivity"));
    ui.checkbox(&mut s.invert_y, "Invert vertical look");
    ui.checkbox(&mut s.third_person, "Third-person camera (V)");

    ui.add_space(10.0);
    section(ui, "VIEW");
    ui.add(egui::Slider::new(&mut s.fov_deg, 60.0..=100.0).suffix("°").text("Field of view"));
    ui.checkbox(&mut s.show_minimap, "Show minimap");

    ui.add_space(10.0);
    section(ui, "GRAPHICS");
    // Quality preset selector: one click bundles detail, fog, dynamic lights, shadows, particle density.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Quality preset").size(13.0));
        for p in crate::graphics::GraphicsPreset::ALL {
            let selected = s.graphics_preset == p;
            if ui.selectable_label(selected, p.name()).clicked() {
                s.graphics_preset = p;
            }
        }
    });
    ui.label(egui::RichText::new(s.graphics_preset.desc()).monospace().size(11.0).color(dim()));
    ui.add(egui::Slider::new(&mut s.render_detail, 0.6..=3.0).text("Render resolution (geometry detail)"));
    ui.add(egui::Slider::new(&mut s.texture_detail, 1.0..=8.0).text("Procgen texture resolution"));
    ui.label(egui::RichText::new("preset & detail apply to the next map you load").monospace().size(11.0).color(dim()));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Renderer").size(13.0));
        if ui.selectable_label(!s.gpu_renderer, "Software").clicked() {
            s.gpu_renderer = false;
        }
        if ui.selectable_label(s.gpu_renderer, "GPU (wgpu)").clicked() {
            s.gpu_renderer = true;
        }
    });
    ui.label(egui::RichText::new("GPU is experimental — takes effect on restart (needs a build with the `gpu` feature).").monospace().size(11.0).color(dim()));

    ui.add_space(10.0);
    section(ui, "TEAM COLORS");
    ui.label(egui::RichText::new("tint hostiles and friendlies to tell factions apart at a glance").monospace().size(11.0).color(dim()));
    ui.horizontal(|ui| {
        ui.label("Hostile");
        ui.color_edit_button_srgba(&mut s.hostile_color);
        ui.add_space(16.0);
        ui.label("Friendly");
        ui.color_edit_button_srgba(&mut s.friendly_color);
    });
    if ui.button("Reset to default (orange / green)").clicked() {
        s.hostile_color = egui::Color32::from_rgb(230, 120, 40);
        s.friendly_color = egui::Color32::from_rgb(70, 190, 90);
    }

    ui.add_space(10.0);
    section(ui, "MOVEMENT KEYS");
    bind_row(ui, "Forward", BindAction::Forward, s, rebinding);
    bind_row(ui, "Left", BindAction::Left, s, rebinding);
    bind_row(ui, "Back", BindAction::Back, s, rebinding);
    bind_row(ui, "Right", BindAction::Right, s, rebinding);
    bind_row(ui, "Jump", BindAction::Jump, s, rebinding);
    bind_row(ui, "Sprint", BindAction::Sprint, s, rebinding);
    bind_row(ui, "Crouch", BindAction::Crouch, s, rebinding);
    bind_row(ui, "Dive", BindAction::Dive, s, rebinding);
    bind_row(ui, "Reload", BindAction::Reload, s, rebinding);

    ui.add_space(10.0);
    section(ui, "MENU / FORGE NAVIGATION");
    ui.label(egui::RichText::new("arrow keys and vim h j k l both work in menus & the forge").monospace().size(11.0).color(dim()));
    bind_row(ui, "Nav Up", BindAction::NavUp, s, rebinding);
    bind_row(ui, "Nav Down", BindAction::NavDown, s, rebinding);
    bind_row(ui, "Nav Left", BindAction::NavLeft, s, rebinding);
    bind_row(ui, "Nav Right", BindAction::NavRight, s, rebinding);
    bind_row(ui, "Select", BindAction::NavSelect, s, rebinding);
    bind_row(ui, "Back", BindAction::NavBack, s, rebinding);

    ui.add_space(10.0);
    section(ui, "MAP FORGE");
    ui.label(egui::RichText::new("Esc opens the forge menu · these act while editing a map").monospace().size(11.0).color(dim()));
    bind_row(ui, "Forge Menu", BindAction::ForgeMenu, s, rebinding);
    bind_row(ui, "Next Brush", BindAction::ForgeNextBrush, s, rebinding);
    bind_row(ui, "Prev Brush", BindAction::ForgePrevBrush, s, rebinding);
    bind_row(ui, "Rotate", BindAction::ForgeRotate, s, rebinding);
    bind_row(ui, "Scale Up", BindAction::ForgeScaleUp, s, rebinding);
    bind_row(ui, "Scale Down", BindAction::ForgeScaleDown, s, rebinding);
    bind_row(ui, "Undo", BindAction::ForgeUndo, s, rebinding);

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(egui::RichText::new("Dvorak  .oeu").monospace()).clicked() {
            s.bindings = Bindings::dvorak();
            *rebinding = None;
        }
        if ui.button(egui::RichText::new("QWERTY  wasd").monospace()).clicked() {
            s.bindings = Bindings::qwerty();
            *rebinding = None;
        }
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);
    ui.vertical_centered(|ui| {
        if menu_button(ui, "Back").clicked() {
            *next_mode = Some(return_to);
        }
    });
}

fn section(ui: &mut egui::Ui, label: &str) {
    ui.label(egui::RichText::new(label).monospace().size(13.0).color(Color32::from_rgb(130, 200, 230)));
    ui.add_space(2.0);
}

fn bind_row(ui: &mut egui::Ui, label: &str, action: BindAction, s: &Settings, rebinding: &mut Option<BindAction>) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 22.0], egui::Label::new(egui::RichText::new(label).monospace().size(15.0)).selectable(false));
        let listening = *rebinding == Some(action);
        let txt = if listening {
            "press a key…".to_string()
        } else {
            format!("[ {} ]", s.bindings.get(action).symbol_or_name())
        };
        let col = if listening { Color32::from_rgb(235, 220, 120) } else { Color32::from_rgb(210, 214, 222) };
        let btn = egui::Button::new(egui::RichText::new(txt).monospace().size(15.0).color(col));
        if ui.add_sized([170.0, 24.0], btn).clicked() {
            *rebinding = Some(action);
        }
    });
}

// ── HUD pieces ─────────────────────────────────────────────────────────────────

/// The vertical sky gradient alone — deep sky at the top easing into horizon fog at the middle of the
/// viewport. Shared by the live gameplay backdrop (which adds stars/sun on top) and the lightweight map
/// thumbnail preview (which doesn't need those extras).
fn paint_sky_gradient(p: &egui::Painter, rect: Rect, sky: Color32, fog: Color32) {
    let mid = rect.center().y;
    let lerp = |a: Color32, b: Color32, s: f32| {
        let s = s.clamp(0.0, 1.0);
        Color32::from_rgb(
            (a.r() as f32 * (1.0 - s) + b.r() as f32 * s) as u8,
            (a.g() as f32 * (1.0 - s) + b.g() as f32 * s) as u8,
            (a.b() as f32 * (1.0 - s) + b.b() as f32 * s) as u8,
        )
    };
    let sky_top = darker(sky, 0.55);
    let bands = 16;
    let bh = (mid - rect.min.y) / bands as f32;
    for i in 0..bands {
        let y0 = rect.min.y + i as f32 * bh;
        let s = i as f32 / (bands - 1) as f32;
        let strip = Rect::from_min_max(Pos2::new(rect.min.x, y0), Pos2::new(rect.max.x, y0 + bh + 1.0));
        p.rect_filled(strip, 0.0, lerp(sky_top, fog, s * s));
    }
    // below the horizon line: the fog tone, flat (the ground/floor mesh normally covers this, but on maps
    // with no floor directly underfoot at the edges — e.g. looking down past a cliff — this reads as haze
    // rather than a hard black cutoff).
    let ground_strip = Rect::from_min_max(Pos2::new(rect.min.x, mid), Pos2::new(rect.max.x, rect.max.y));
    p.rect_filled(ground_strip, 0.0, darker(fog, 0.8));
}

fn paint_backdrop(p: &egui::Painter, rect: Rect, g: &Game) {
    let sky = g.light.sky;
    let fog = g.fog.color;
    let mid = rect.center().y;
    let yaw = g.player.cam.yaw;
    let t = g.time;
    let theme = g.level.cfg.theme;
    let brightness = sky.r() as u32 + sky.g() as u32 + sky.b() as u32;
    let night = brightness < 200;

    // 1) Vertical sky gradient: deep sky up top easing into horizon fog.
    paint_sky_gradient(p, rect, sky, fog);
    let lerp = |a: Color32, b: Color32, s: f32| {
        let s = s.clamp(0.0, 1.0);
        Color32::from_rgb(
            (a.r() as f32 * (1.0 - s) + b.r() as f32 * s) as u8,
            (a.g() as f32 * (1.0 - s) + b.g() as f32 * s) as u8,
            (a.b() as f32 * (1.0 - s) + b.b() as f32 * s) as u8,
        )
    };
    let sky_top = darker(sky, 0.55);

    // Map a world bearing to a screen x (≈74° fov), wrapping.
    let ang_to_x = |world_ang: f32| {
        let mut d = world_ang - yaw;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        rect.center().x + d * (rect.width() / 1.3)
    };

    // 2) Stars + moon on dark skies; otherwise a parallaxing sun glow.
    if night {
        for i in 0..70 {
            let sa = (i as f32) * 0.91; // spread of bearings
            let x = ang_to_x(sa);
            if x < rect.min.x || x > rect.max.x {
                continue;
            }
            let y = rect.min.y + ((i as f32 * 12.9898).sin().abs() * (mid - rect.min.y) * 0.85);
            let tw = 0.5 + 0.5 * ((t * 2.0 + i as f32).sin());
            let a = (120.0 * tw) as u8;
            p.circle_filled(Pos2::new(x, y), 1.1, Color32::from_rgba_unmultiplied(230, 235, 255, a));
        }
        let mx = ang_to_x(2.3);
        if mx > rect.min.x && mx < rect.max.x {
            let c = Pos2::new(mx, rect.min.y + (mid - rect.min.y) * 0.3);
            for k in 0..5 {
                let r = 16.0 + k as f32 * 10.0;
                let a = (30.0 - k as f32 * 5.0).max(3.0) as u8;
                p.circle_filled(c, r, Color32::from_rgba_unmultiplied(210, 220, 240, a));
            }
            p.circle_filled(c, 14.0, Color32::from_rgb(225, 230, 245));
        }
    } else {
        sun_glow(p, rect, mid, g.light.color, ang_to_x(0.7));
        // 3) Drifting clouds across the upper sky.
        let cloud = lerp(sky, Color32::WHITE, 0.5);
        for i in 0..7 {
            let ca = (i as f32) * 0.97 + t * 0.012; // slow drift
            let x = ang_to_x(ca);
            if x < rect.min.x - 80.0 || x > rect.max.x + 80.0 {
                continue;
            }
            let y = rect.min.y + (mid - rect.min.y) * (0.18 + 0.12 * ((i as f32 * 7.1).sin()));
            for (dx, r) in [(-28.0, 16.0), (-8.0, 22.0), (14.0, 18.0), (34.0, 13.0)] {
                p.circle_filled(Pos2::new(x + dx, y), r, Color32::from_rgba_unmultiplied(cloud.r(), cloud.g(), cloud.b(), 46));
            }
        }
    }

    // 4) Distant horizon silhouettes — two parallax ridge layers, shaped by theme: a city skyline for urban,
    // a flat ocean line for the carrier, jagged volcanic peaks, low hull ridges in space, hills otherwise.
    let cols = 90;
    let cw = rect.width() / cols as f32;
    let urban = matches!(theme, MapTheme::Urban);
    let ocean = matches!(theme, MapTheme::Carrier);
    let volcanic = matches!(theme, MapTheme::Volcanic);
    let space = matches!(theme, MapTheme::Station);
    if ocean {
        // a clean sea horizon: a slightly hazy band where sky meets water, no ridgeline
        let band = lerp(fog, sky_top, 0.2);
        p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, mid - 3.0), Pos2::new(rect.max.x, mid)), 0.0, band);
    } else {
        for layer in 0..2 {
            let (amp, base_h, col) = if layer == 0 {
                (rect.height() * 0.10, 0.04, lerp(fog, sky_top, 0.35))
            } else {
                (rect.height() * 0.16, 0.0, darker(fog, 0.7))
            };
            for c in 0..cols {
                let x = rect.min.x + c as f32 * cw;
                let world_ang = yaw + (x - rect.center().x) / (rect.width() / 1.3);
                let h = if urban {
                    // blocky skyline: quantized towers
                    let q = (world_ang * 6.0 + layer as f32 * 3.0).sin() * 0.5 + 0.5;
                    let step = ((world_ang * 9.0 + layer as f32).sin() * 4.0).floor().abs() % 4.0 / 4.0;
                    base_h * rect.height() + amp * (0.3 + 0.7 * (q * 0.5 + step * 0.5))
                } else if volcanic {
                    // sharp jagged peaks — a higher, spikier ridge
                    base_h * rect.height() + amp * 1.4 * ridge_noise(world_ang * 2.6 + layer as f32 * 7.0).powf(0.6)
                } else if space {
                    // low distant hull/station structures on the near layer only, flat void beyond
                    if layer == 0 {
                        base_h * rect.height() + amp * 0.5 * ridge_noise(world_ang * 4.0).powf(2.0)
                    } else {
                        0.0
                    }
                } else {
                    base_h * rect.height() + amp * ridge_noise(world_ang * 1.7 + layer as f32 * 10.0)
                };
                let top = mid - h;
                p.rect_filled(Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + cw + 1.0, mid)), 0.0, col);
                // volcanic peaks catch a faint ember-glow along their crest
                if volcanic && layer == 1 && h > amp * 0.8 {
                    p.rect_filled(Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + cw + 1.0, top + 2.0)), 0.0, Color32::from_rgba_unmultiplied(255, 120, 40, 60));
                }
            }
        }
    }

    // Ground half is the fog colour so the floor melts into the horizon.
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, mid), rect.max), 0.0, fog);
}

fn sun_glow(p: &egui::Painter, rect: Rect, mid: f32, glow: Color32, sun_x: f32) {
    if sun_x < rect.min.x - 60.0 || sun_x > rect.max.x + 60.0 {
        return;
    }
    let c = Pos2::new(sun_x, mid - rect.height() * 0.14);
    for k in 0..7 {
        let r = 24.0 + k as f32 * 24.0;
        let a = (50.0 - k as f32 * 6.5).max(4.0) as u8;
        p.circle_filled(c, r, Color32::from_rgba_unmultiplied(glow.r(), glow.g(), glow.b(), a));
    }
    p.circle_filled(c, 18.0, Color32::from_rgba_unmultiplied(255, 255, 250, 110));
}

/// Layered-sine ridgeline in 0..1 for the horizon silhouette.
fn ridge_noise(a: f32) -> f32 {
    let v = (a).sin() * 0.5 + (a * 2.3 + 1.0).sin() * 0.28 + (a * 5.1 + 2.0).sin() * 0.14 + (a * 9.7).sin() * 0.07;
    (v * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// A modern dynamic reticle: four ticks that bloom apart as your spread grows (moving, hip-fire), shrink
/// when you aim down sights, with a small centre dot. Turns faintly red over an enemy.
fn draw_reticle(p: &egui::Painter, rect: Rect, g: &Game, ads: f32) {
    let c = rect.center();
    let pl = &g.player;
    let spd = (pl.vel.x * pl.vel.x + pl.vel.z * pl.vel.z).sqrt();
    let bloom = (6.0 + spd * 0.7) * (1.0 - 0.6 * ads) + if pl.sprinting { 8.0 } else { 0.0 };
    let on_target = aim_on_enemy(g);
    let base = g.cosmetics.reticle_color.color();
    let col = if on_target { Color32::from_rgb(245, 90, 78) } else { base };
    let stroke = Stroke::new(1.6, col);
    let len = 7.0;
    let gap = bloom;
    use crate::cosmetics::Reticle;
    match g.cosmetics.reticle {
        Reticle::Cross => {
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], stroke);
            p.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], stroke);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], stroke);
        }
        Reticle::Dot => {
            p.circle_filled(c, 2.2, col);
        }
        Reticle::CrossDot => {
            p.circle_filled(c, 1.6, col);
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], stroke);
            p.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], stroke);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], stroke);
        }
        Reticle::Circle => {
            p.circle_stroke(c, gap + 4.0, stroke);
            p.circle_filled(c, 1.2, col);
        }
        Reticle::Chevron => {
            let y = c.y + gap + 2.0;
            p.line_segment([Pos2::new(c.x - 8.0, y + 6.0), Pos2::new(c.x, y)], stroke);
            p.line_segment([Pos2::new(c.x + 8.0, y + 6.0), Pos2::new(c.x, y)], stroke);
            p.circle_filled(c, 1.2, col);
        }
        Reticle::Brackets => {
            let g2 = gap + 4.0;
            for sx in [-1.0, 1.0] {
                let x = c.x + sx * g2;
                p.line_segment([Pos2::new(x, c.y - 6.0), Pos2::new(x, c.y + 6.0)], stroke);
                p.line_segment([Pos2::new(x, c.y - 6.0), Pos2::new(x - sx * 4.0, c.y - 6.0)], stroke);
                p.line_segment([Pos2::new(x, c.y + 6.0), Pos2::new(x - sx * 4.0, c.y + 6.0)], stroke);
            }
            p.circle_filled(c, 1.2, col);
        }
        Reticle::TBar => {
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x + gap + len, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], stroke);
        }
    }
}

/// Rough check: is the camera pointing at a live enemy? Used to tint the reticle.
fn aim_on_enemy(g: &Game) -> bool {
    let cam = &g.player.cam;
    let fwd = cam.forward();
    let eye = g.player.eye();
    for e in &g.enemies {
        if !e.alive() {
            continue;
        }
        let to = e.center() - eye;
        let dist = to.len();
        if dist < 0.5 || dist > 120.0 {
            continue;
        }
        let dir = to * (1.0 / dist);
        if fwd.dot(dir) > 1.0 - (e.kind.def().radius * 1.6 / dist).min(0.05) {
            return true;
        }
    }
    false
}

/// The sniper scope: a black surround with a thin circular cutout and crosshair lines.
fn draw_scope(p: &egui::Painter, rect: Rect, ads: f32) {
    let c = rect.center();
    let r = (rect.height() * 0.42) * (0.6 + 0.4 * ads);
    // black surround: four big rects leaving a square hole, then a ring to fake the circle edge
    let dark = Color32::from_rgba_unmultiplied(4, 5, 7, 255);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, c.y - r)), 0.0, dark);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, c.y + r), rect.max), 0.0, dark);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, c.y - r), Pos2::new(c.x - r, c.y + r)), 0.0, dark);
    p.rect_filled(Rect::from_min_max(Pos2::new(c.x + r, c.y - r), Pos2::new(rect.max.x, c.y + r)), 0.0, dark);
    p.circle_stroke(c, r, Stroke::new(2.0, Color32::from_rgb(10, 10, 12)));
    p.circle_stroke(c, r - 2.0, Stroke::new(1.0, Color32::from_rgb(40, 44, 50)));
    // crosshair
    let line = Stroke::new(1.0, Color32::from_rgb(20, 22, 26));
    p.line_segment([Pos2::new(c.x - r, c.y), Pos2::new(c.x + r, c.y)], line);
    p.line_segment([Pos2::new(c.x, c.y - r), Pos2::new(c.x, c.y + r)], line);
    p.line_segment([Pos2::new(c.x - 16.0, c.y), Pos2::new(c.x + 16.0, c.y)], Stroke::new(1.6, Color32::from_rgb(180, 30, 30)));
    p.line_segment([Pos2::new(c.x, c.y - 16.0), Pos2::new(c.x, c.y + 16.0)], Stroke::new(1.6, Color32::from_rgb(180, 30, 30)));
    p.circle_filled(c, 1.4, Color32::from_rgb(200, 40, 40));
}

/// The hitmarker: four short diagonal ticks around the centre when a shot connects (red on a kill).
fn draw_hitmarker(p: &egui::Painter, rect: Rect, g: &Game) {
    if g.hitmarker <= 0.0 {
        return;
    }
    let c = rect.center();
    let k = (g.hitmarker / 0.18).clamp(0.0, 1.0);
    let col = if g.hitmarker_kill {
        Color32::from_rgba_unmultiplied(235, 60, 50, (k * 255.0) as u8)
    } else {
        Color32::from_rgba_unmultiplied(235, 238, 242, (k * 230.0) as u8)
    };
    let stroke = Stroke::new(2.0, col);
    let r0 = 7.0;
    let r1 = 12.0;
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        p.line_segment([Pos2::new(c.x + dx * r0, c.y + dy * r0), Pos2::new(c.x + dx * r1, c.y + dy * r1)], stroke);
    }
}

fn draw_hit_flash(p: &egui::Painter, rect: Rect, g: &Game) {
    let f = g.player.hurt_flash;
    if f > 0.01 {
        let a = (f * 120.0) as u8;
        p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(150, 12, 12, a));
    }
}

/// Underwater screen treatment: a blue-green wash that deepens with submersion, going strongest and adding a
/// caustic shimmer band once the head goes under. Reads immediately as "I'm in the water" without any
/// renderer changes — a flat overlay keyed off the player's submersion state.
fn draw_underwater_tint(p: &egui::Painter, rect: Rect, g: &Game) {
    let sub = g.player.submersion;
    if sub <= 0.05 {
        return;
    }
    // wading (feet wet) is a faint tint; head-under is a strong immersive wash.
    let depth = (sub / 1.6).clamp(0.0, 1.0);
    let base_a = if g.player.underwater { 90.0 } else { 26.0 * depth };
    let a = base_a.clamp(0.0, 110.0) as u8;
    p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(28, 92, 116, a));
    if g.player.underwater {
        // a slow caustic shimmer — two offset sine bands of brightness drifting across the screen
        let t = g.time;
        let bands = 5;
        for i in 0..bands {
            let ph = t * 0.6 + i as f32 * 1.3;
            let y = rect.top() + rect.height() * ((ph.sin() * 0.5 + 0.5) * 0.9 + 0.05);
            let h = rect.height() * 0.06;
            let sa = (18.0 + 10.0 * (ph * 1.7).cos()) as u8;
            p.rect_filled(
                Rect::from_min_max(egui::pos2(rect.left(), y), egui::pos2(rect.right(), y + h)),
                0.0,
                Color32::from_rgba_unmultiplied(120, 190, 210, sa),
            );
        }
    }
}

/// Low-health blood vignette around the screen edges — fades out as health regenerates.
fn draw_damage_vignette(p: &egui::Painter, rect: Rect, g: &Game) {
    if !g.player.alive {
        return;
    }
    let h = g.player.health;
    if h >= 60.0 {
        return;
    }
    let t = ((60.0 - h) / 60.0).clamp(0.0, 1.0);
    // a pulse so it reads as "critical"
    let pulse = 0.7 + 0.3 * (g.time * 4.0).sin();
    let band = (rect.height() * 0.16) * (0.5 + 0.5 * t);
    let alpha = (t * pulse * 150.0) as u8;
    let col = Color32::from_rgba_unmultiplied(120, 0, 0, alpha);
    // top, bottom, left, right soft bands
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + band)), 0.0, col);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - band), rect.max), 0.0, col);
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.min.x + band, rect.max.y)), 0.0, col);
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.max.x - band, rect.min.y), rect.max), 0.0, col);
}

fn draw_hud(p: &egui::Painter, rect: Rect, g: &Game, s: &Settings) {
    let pl = &g.player;
    let pad = 26.0;
    let hy = rect.max.y - pad;

    // ── Bottom-right readout. In The Hunt there's no ammo — show the creature + prey remaining. ──────────
    if g.is_hunt() {
        if g.is_survivor_hunt() {
            // Survivor: escape timer + survivors remaining.
            let t = g.hunt_timer().max(0.0);
            let mm = (t as u32) / 60;
            let ss = (t as u32) % 60;
            let tcol = if g.hunt_escaped() { Color32::from_rgb(150, 230, 150) } else if t < 20.0 { Color32::from_rgb(235, 120, 90) } else { Color32::from_rgb(228, 232, 238) };
            let label = if g.hunt_escaped() { "DAWN".to_string() } else { format!("{}:{:02}", mm, ss) };
            p.text(Pos2::new(rect.max.x - pad, hy), Align2::RIGHT_BOTTOM, label, FontId::monospace(40.0), tcol);
            p.text(Pos2::new(rect.max.x - pad, hy - 46.0), Align2::RIGHT_BOTTOM, "UNTIL DAWN", FontId::monospace(14.0), dim());
            p.text(Pos2::new(rect.max.x - pad, hy - 68.0), Align2::RIGHT_BOTTOM, format!("SURVIVORS {}", g.survivors_left()), FontId::monospace(15.0), Color32::from_rgb(180, 200, 210));
            p.text(Pos2::new(rect.max.x - pad, hy - 90.0), Align2::RIGHT_BOTTOM, format!("HUNTED BY {}", g.creature().name().to_uppercase()), FontId::monospace(12.0), g.creature().color());
        } else {
            let alive = g.enemies_alive();
            let cr = g.creature();
            p.text(Pos2::new(rect.max.x - pad, hy), Align2::RIGHT_BOTTOM, format!("{}", alive), FontId::monospace(48.0), Color32::from_rgb(228, 120, 110));
            p.text(Pos2::new(rect.max.x - pad - 70.0, hy - 6.0), Align2::RIGHT_BOTTOM, "PREY", FontId::monospace(18.0), Color32::from_rgb(150, 156, 164));
            p.text(Pos2::new(rect.max.x - pad, hy - 52.0), Align2::RIGHT_BOTTOM, cr.name(), FontId::monospace(18.0), cr.color());
            p.text(Pos2::new(rect.max.x - pad, hy - 76.0), Align2::RIGHT_BOTTOM, "LMB  STRIKE", FontId::monospace(13.0), dim());
        }
    } else {
    let wpn = pl.weapon();
    let def = wpn.kind.def();
    let reloading = wpn.reloading > 0.0;
    let magcol = if reloading {
        Color32::from_rgb(230, 200, 90)
    } else if wpn.mag == 0 {
        Color32::from_rgb(225, 70, 60)
    } else {
        Color32::from_rgb(238, 240, 244)
    };
    p.text(Pos2::new(rect.max.x - pad, hy), Align2::RIGHT_BOTTOM, format!("{}", wpn.mag), FontId::monospace(48.0), magcol);
    let reserve_x = rect.max.x - pad - 64.0;
    p.text(
        Pos2::new(reserve_x, hy - 4.0),
        Align2::RIGHT_BOTTOM,
        format!("/ {}", wpn.reserve),
        FontId::monospace(22.0),
        Color32::from_rgb(150, 156, 164),
    );
    // Show the legendary's name (in its accent) when the legendary primary is up; else the archetype name.
    let (wlabel, wcol) = match g.player.loadout.legend() {
        Some(lg) if g.player.current == 0 || g.player.current == 2 => (lg.name, lg.accent.color()),
        _ => (def.name, weapon_color(def.kind)),
    };
    p.text(Pos2::new(rect.max.x - pad, hy - 52.0), Align2::RIGHT_BOTTOM, wlabel, FontId::monospace(18.0), wcol);
    // Class grenade count (T to throw): label reflects the equipped grenade kind.
    let frags = g.frags();
    let fcol = if frags > 0 { Color32::from_rgb(210, 150, 90) } else { dim() };
    let glabel = match pl.class.def().grenade {
        crate::class::GrenadeKind::Frag => "T FRAG",
        crate::class::GrenadeKind::Gravity => "T GRAV",
        crate::class::GrenadeKind::Acid => "T ACID",
    };
    p.text(Pos2::new(reserve_x, hy + 18.0), Align2::RIGHT_BOTTOM, format!("{} x{}", glabel, frags), FontId::monospace(15.0), fcol);
    if reloading {
        p.text(Pos2::new(rect.max.x - pad, hy - 76.0), Align2::RIGHT_BOTTOM, "RELOADING", FontId::monospace(13.0), Color32::from_rgb(230, 200, 90));
    } else if wpn.mag == 0 {
        p.text(Pos2::new(rect.max.x - pad, hy - 76.0), Align2::RIGHT_BOTTOM, "PRESS R", FontId::monospace(13.0), dim());
    }
    }

    // weapon slot pips (primary / sidearm) — not shown in The Hunt (claws only)
    if !g.is_hunt() {
        let mut x = rect.max.x - pad;
        for slot in (0..2).rev() {
            let k = pl.weapons[slot].kind;
            let sel = slot == pl.current;
            let col = if sel { weapon_color(k) } else { dim() };
            p.text(Pos2::new(x, hy - 100.0), Align2::RIGHT_BOTTOM, format!("{}", slot + 1), FontId::monospace(if sel { 18.0 } else { 14.0 }), col);
            x -= 24.0;
        }
    }

    // ── Health bar, bottom-left ───────────────────────────────────────────────────
    let bar_w = 188.0;
    let bar_h = 12.0;
    let bx = rect.min.x + pad;
    let bbottom = hy;
    let bar = Rect::from_min_max(Pos2::new(bx, bbottom - bar_h), Pos2::new(bx + bar_w, bbottom));
    p.rect_filled(bar, 2.0, Color32::from_rgba_unmultiplied(10, 12, 16, 180));
    let frac = (pl.health / pl.max_health).clamp(0.0, 1.0);
    let fill = Rect::from_min_max(bar.min, Pos2::new(bar.min.x + bar_w * frac, bar.max.y));
    p.rect_filled(fill, 2.0, health_color(pl.health));
    p.rect_stroke(bar, 2.0, Stroke::new(1.0, Color32::from_rgb(60, 68, 78)), egui::StrokeKind::Inside);
    p.text(Pos2::new(bx, bbottom - bar_h - 6.0), Align2::LEFT_BOTTOM, format!("HEALTH  {}", pl.class.name().to_uppercase()), FontId::monospace(12.0), dim());

    // ── Super-ability charge bar, just above the health bar (only for classes with a super) ──────────────
    if pl.class.def().super_kind != crate::class::SuperKind::None {
        let sc = g.super_charge().clamp(0.0, 1.0);
        let sb_bottom = bbottom - bar_h - 28.0;
        let sbar = Rect::from_min_max(Pos2::new(bx, sb_bottom - 8.0), Pos2::new(bx + bar_w, sb_bottom));
        p.rect_filled(sbar, 2.0, Color32::from_rgba_unmultiplied(10, 12, 16, 180));
        let scol = if sc >= 1.0 { Color32::from_rgb(120, 220, 255) } else { Color32::from_rgb(70, 120, 160) };
        let sfill = Rect::from_min_max(sbar.min, Pos2::new(sbar.min.x + bar_w * sc, sbar.max.y));
        p.rect_filled(sfill, 2.0, scol);
        p.rect_stroke(sbar, 2.0, Stroke::new(1.0, Color32::from_rgb(50, 70, 84)), egui::StrokeKind::Inside);
        let slabel = if sc >= 1.0 { "SUPER READY [X]".to_string() } else { "SUPER".to_string() };
        let slcol = if sc >= 1.0 { Color32::from_rgb(140, 230, 255) } else { dim() };
        p.text(Pos2::new(bx, sb_bottom - 10.0), Align2::LEFT_BOTTOM, slabel, FontId::monospace(11.0), slcol);
    }
    // ── Jetpack charge bar (only when the loadout has a jetpack) — the ion-drive fuel gauge ───────────────
    if pl.jetpack {
        let jc = pl.jet_fuel.clamp(0.0, 1.0);
        // sit it just above the super bar (or the health bar if no super)
        let super_offset = if pl.class.def().super_kind != crate::class::SuperKind::None { 44.0 } else { 0.0 };
        let jb_bottom = bbottom - bar_h - 28.0 - super_offset;
        let jbar = Rect::from_min_max(Pos2::new(bx, jb_bottom - 8.0), Pos2::new(bx + bar_w, jb_bottom));
        p.rect_filled(jbar, 2.0, Color32::from_rgba_unmultiplied(8, 14, 22, 180));
        // colour shifts from dim blue (empty) to bright ion-cyan (full); pulse while actively thrusting
        let base = if jc >= 0.999 { Color32::from_rgb(120, 210, 255) } else { Color32::from_rgb(70, 130, 190) };
        let jcol = if pl.jet_active {
            let f = 0.7 + 0.3 * ((g.time * 30.0).sin() * 0.5 + 0.5);
            Color32::from_rgb((90.0 + 130.0 * f) as u8, (170.0 + 60.0 * f) as u8, 255)
        } else {
            base
        };
        let jfill = Rect::from_min_max(jbar.min, Pos2::new(jbar.min.x + bar_w * jc, jbar.max.y));
        p.rect_filled(jfill, 2.0, jcol);
        p.rect_stroke(jbar, 2.0, Stroke::new(1.0, Color32::from_rgb(48, 78, 104)), egui::StrokeKind::Inside);
        let jlabel = if jc < 0.15 { "ION CHARGING" } else { "ION THRUST" };
        p.text(Pos2::new(bx, jb_bottom - 10.0), Align2::LEFT_BOTTOM, jlabel, FontId::monospace(11.0), Color32::from_rgb(120, 200, 255));
    }
    if g.streak >= 2 {
        let sy = if pl.class.def().super_kind != crate::class::SuperKind::None { bbottom - bar_h - 48.0 } else { bbottom - bar_h - 24.0 };
        p.text(
            Pos2::new(bx, sy),
            Align2::LEFT_BOTTOM,
            format!("STREAK x{}", g.streak),
            FontId::monospace(13.0),
            Color32::from_rgb(255, 190, 80),
        );
    }

    // ── Score / wave, top-left ────────────────────────────────────────────────────
    p.text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad),
        Align2::LEFT_TOP,
        format!("{:06}", g.score),
        FontId::monospace(26.0),
        Color32::from_rgb(235, 238, 244),
    );
    p.text(Pos2::new(rect.min.x + pad, rect.min.y + pad + 30.0), Align2::LEFT_TOP, "SCORE", FontId::monospace(12.0), dim());
    p.text(
        Pos2::new(rect.min.x + pad, rect.min.y + pad + 54.0),
        Align2::LEFT_TOP,
        format!("WAVE {}   ENEMIES {}", g.wave, g.enemies_alive()),
        FontId::monospace(15.0),
        Color32::from_rgb(210, 150, 120),
    );

    // ── Controls hint, faint, bottom-centre ───────────────────────────────────────
    let b = &s.bindings;
    let hint = format!(
        "{}{}{}{} move · MOUSE look · LMB fire · RMB aim · SHIFT sprint · R reload · 1-4 weapons · ESC menu",
        b.forward.symbol_or_name(),
        b.left.symbol_or_name(),
        b.back.symbol_or_name(),
        b.right.symbol_or_name(),
    );
    p.text(
        Pos2::new(rect.center().x, rect.max.y - 6.0),
        Align2::CENTER_BOTTOM,
        hint,
        FontId::monospace(11.0),
        Color32::from_rgba_unmultiplied(200, 200, 200, 110),
    );
}

/// UAV: a red diamond marker over each live enemy (on-screen), or an edge chevron pointing toward
/// off-screen ones — the recon sweep that lets you push with information.
fn draw_uav_markers(p: &egui::Painter, rect: Rect, g: &Game, cam: &FpsCam, cx: f32, cy: f32, scale: f32) {
    let c = rect.center();
    let col = Color32::from_rgb(235, 80, 70);
    for e in &g.enemies {
        if !e.alive() {
            continue;
        }
        let head = e.center() + crate::math::Vec3::new(0.0, e.kind.def().height * 0.5 + 0.4, 0.0);
        match cam.project(head, cx, cy, scale) {
            Some((sx, sy, depth)) if depth > 0.0 => {
                let at = Pos2::new(sx, sy);
                // small downward diamond
                let s = 6.0;
                p.add(egui::Shape::convex_polygon(
                    vec![Pos2::new(at.x, at.y - s), Pos2::new(at.x + s, at.y), Pos2::new(at.x, at.y + s), Pos2::new(at.x - s, at.y)],
                    Color32::from_rgba_unmultiplied(235, 80, 70, 180),
                    Stroke::new(1.0, col),
                ));
            }
            _ => {
                // off-screen: a chevron at the edge in the enemy's screen direction
                let to = e.center() - cam.pos;
                let rx = to.dot(cam.right());
                let fz = to.dot(cam.forward());
                let ang = rx.atan2(fz.max(0.001)) * if fz < 0.0 { 3.0 } else { 1.0 };
                let dir = Vec2::new(ang.sin(), -0.2).normalized();
                let edge = c + dir * (rect.width().min(rect.height()) * 0.4);
                p.circle_filled(edge, 3.0, Color32::from_rgba_unmultiplied(235, 80, 70, 150));
            }
        }
    }
}

/// Directional damage indicators: red arcs at the screen edge pointing toward where fire is coming from.
fn draw_damage_arrows(p: &egui::Painter, rect: Rect, g: &Game) {
    let c = rect.center();
    let yaw = g.player.cam.yaw;
    for (dir, life) in &g.damage_dirs {
        // angle of the shooter relative to where we're facing (0 = dead ahead)
        let world_ang = dir.x.atan2(-dir.z); // matches yaw convention (yaw 0 → -Z)
        let rel = world_ang - yaw;
        let a = (life.clamp(0.0, 1.0) * 200.0) as u8;
        let col = Color32::from_rgba_unmultiplied(220, 40, 40, a);
        // place an arc ~120px from centre at the relative bearing (screen: +x right, +y down)
        let rr = rect.height() * 0.18;
        let centre = c + Vec2::new(rel.sin() * rr, -rel.cos() * rr);
        // a short arc drawn as a thick chevron facing outward
        let tangent = Vec2::new(rel.cos(), rel.sin());
        let out = Vec2::new(rel.sin(), -rel.cos());
        let p0 = centre - tangent * 18.0;
        let p1 = centre + out * 12.0;
        let p2 = centre + tangent * 18.0;
        p.line_segment([p0, p1], Stroke::new(4.0, col));
        p.line_segment([p1, p2], Stroke::new(4.0, col));
    }
}

/// Scorestreak readout: Q-UAV and F-STRIKE with a fill bar that completes as you score, and a READY tag.
fn draw_scorestreaks(p: &egui::Painter, rect: Rect, g: &Game) {
    let x = rect.min.x + 26.0;
    let mut y = rect.min.y + 26.0 + 84.0;
    let rows = [
        ("G  UAV", g.uav_progress(), g.uav_ready(), g.uav_timer > 0.0),
        ("H  SENTRY", g.sentry_progress(), g.sentry_ready(), false),
        ("F  STRIKE", g.strike_progress(), g.strike_ready(), false),
    ];
    for (label, prog, ready, active) in rows {
        let key_col = if active {
            Color32::from_rgb(120, 230, 140)
        } else if ready {
            Color32::from_rgb(255, 210, 90)
        } else {
            Color32::from_rgb(150, 156, 164)
        };
        p.text(Pos2::new(x, y), Align2::LEFT_TOP, label, FontId::monospace(13.0), key_col);
        // progress bar
        let bw = 96.0;
        let bh = 5.0;
        let bar = Rect::from_min_size(Pos2::new(x + 86.0, y + 3.0), Vec2::new(bw, bh));
        p.rect_filled(bar, 1.0, Color32::from_rgba_unmultiplied(10, 12, 16, 160));
        let fillw = bw * prog.clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(fillw, bh)), 1.0, key_col);
        if active {
            p.text(Pos2::new(x + 86.0 + bw + 8.0, y, ), Align2::LEFT_TOP, "ONLINE", FontId::monospace(11.0), key_col);
        } else if ready {
            p.text(Pos2::new(x + 86.0 + bw + 8.0, y), Align2::LEFT_TOP, "READY", FontId::monospace(11.0), key_col);
        }
        y += 18.0;
    }
}

/// A thin top-centre compass strip with cardinal letters that scrolls as you turn (a BO2 HUD staple).
fn draw_compass(p: &egui::Painter, rect: Rect, g: &Game) {
    let yaw = g.player.cam.yaw;
    let cx = rect.center().x;
    let top = rect.min.y + 16.0;
    let halfw = 150.0;
    let strip = Rect::from_min_max(Pos2::new(cx - halfw, top), Pos2::new(cx + halfw, top + 20.0));
    p.rect_filled(strip, 3.0, Color32::from_rgba_unmultiplied(10, 12, 16, 130));
    // yaw=0 looks -Z which we call North. Map heading to pixels: 90° across ≈ halfw.
    let px_per_rad = halfw / (std::f32::consts::PI * 0.5);
    let marks = [("N", 0.0), ("E", std::f32::consts::FRAC_PI_2), ("S", std::f32::consts::PI), ("W", -std::f32::consts::FRAC_PI_2)];
    for (lbl, ang) in marks {
        // signed smallest difference between facing yaw and this mark
        let mut d = ang - yaw;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        let xx = cx + d * px_per_rad;
        if xx < strip.min.x + 6.0 || xx > strip.max.x - 6.0 {
            continue;
        }
        p.text(Pos2::new(xx, top + 10.0), Align2::CENTER_CENTER, lbl, FontId::monospace(13.0), Color32::from_rgb(220, 226, 232));
    }
    // centre tick
    p.line_segment([Pos2::new(cx, top), Pos2::new(cx, top + 20.0)], Stroke::new(1.5, Color32::from_rgb(255, 200, 90)));
}

/// Kill feed: recent elimination lines fading out in the top-right, under the minimap.
fn draw_killfeed(p: &egui::Painter, rect: Rect, g: &Game) {
    let x = rect.max.x - 16.0;
    let mut y = rect.min.y + 170.0;
    for (text, life) in g.feed.iter().take(5) {
        let a = (life.clamp(0.0, 1.0) * 230.0) as u8;
        let col = if text.starts_with("WAVE") {
            Color32::from_rgba_unmultiplied(255, 200, 90, a)
        } else {
            Color32::from_rgba_unmultiplied(225, 235, 245, a)
        };
        p.text(Pos2::new(x, y), Align2::RIGHT_TOP, text, FontId::monospace(14.0), col);
        y += 19.0;
    }
}

fn draw_minimap(p: &egui::Painter, rect: Rect, g: &Game) {
    let size = 132.0;
    let map_rect = Rect::from_min_size(Pos2::new(rect.max.x - size - 16.0, rect.min.y + 16.0), Vec2::splat(size));
    p.rect_filled(map_rect, 4.0, Color32::from_rgba_unmultiplied(10, 12, 16, 180));
    p.rect_stroke(map_rect, 4.0, Stroke::new(1.0, Color32::from_rgb(60, 70, 80)), egui::StrokeKind::Inside);

    let lvl = &g.level;
    let ts = lvl.ts();
    let radius_tiles = 13.0;
    let ppt = (size * 0.5) / radius_tiles;
    let center = map_rect.center();
    let px = g.player.pos.x / ts;
    let pz = g.player.pos.z / ts;

    let to_screen = |wx_tiles: f32, wz_tiles: f32| -> Pos2 { Pos2::new(center.x + (wx_tiles - px) * ppt, center.y + (wz_tiles - pz) * ppt) };

    let r = radius_tiles.ceil() as i32;
    let (ptx, ptz) = (px.floor() as i32, pz.floor() as i32);
    for tz in (ptz - r)..=(ptz + r) {
        for tx in (ptx - r)..=(ptx + r) {
            let cell = lvl.cell(tx, tz);
            let col = match cell {
                Cell::Wall | Cell::Solid => Color32::from_rgb(90, 110, 130),
                Cell::Floor => Color32::from_rgba_unmultiplied(40, 50, 60, 130),
                _ => continue,
            };
            let sc = to_screen(tx as f32 + 0.5, tz as f32 + 0.5);
            if !map_rect.contains(sc) {
                continue;
            }
            let half = ppt * 0.5;
            p.rect_filled(Rect::from_center_size(sc, Vec2::splat(half * 2.0 - 1.0)), 0.0, col);
        }
    }

    for e in &g.enemies {
        if !e.alive() {
            continue;
        }
        let sc = to_screen(e.pos.x / ts, e.pos.z / ts);
        if map_rect.contains(sc) {
            p.circle_filled(sc, 2.6, Color32::from_rgb(235, 70, 60));
        }
    }

    p.circle_filled(center, 3.0, Color32::from_rgb(120, 255, 140));
    let yaw = g.player.cam.yaw;
    let dir = Vec2::new(yaw.sin(), -yaw.cos());
    p.line_segment([center, center + dir * 11.0], Stroke::new(1.5, Color32::from_rgb(120, 255, 140)));
}

fn draw_center_banner(p: &egui::Painter, rect: Rect, text: &str, col: Color32) {
    let c = rect.center();
    p.text(c + Vec2::new(2.0, 2.0), Align2::CENTER_CENTER, text, FontId::monospace(40.0), Color32::from_black_alpha(180));
    p.text(c, Align2::CENTER_CENTER, text, FontId::monospace(40.0), col);
}

// ── small colour helpers ────────────────────────────────────────────────────────

fn dim() -> Color32 {
    Color32::from_rgb(150, 156, 162)
}
fn health_color(h: f32) -> Color32 {
    if h > 60.0 {
        Color32::from_rgb(120, 230, 130)
    } else if h > 30.0 {
        Color32::from_rgb(235, 200, 90)
    } else {
        Color32::from_rgb(230, 70, 60)
    }
}
fn weapon_color(k: WeaponKind) -> Color32 {
    match k {
        WeaponKind::Ar => Color32::from_rgb(210, 214, 222),
        WeaponKind::Smg => Color32::from_rgb(200, 210, 220),
        WeaponKind::Shotgun => Color32::from_rgb(230, 170, 110),
        WeaponKind::Sniper => Color32::from_rgb(150, 200, 255),
        WeaponKind::Lmg => Color32::from_rgb(220, 200, 150),
        WeaponKind::Dmr => Color32::from_rgb(210, 220, 170),
        WeaponKind::Pistol => Color32::from_rgb(200, 205, 215),
        WeaponKind::EmSword => Color32::from_rgb(120, 200, 255),
        WeaponKind::Launcher => Color32::from_rgb(220, 190, 130),
        WeaponKind::ArcCoil => Color32::from_rgb(150, 220, 255),
        WeaponKind::CryoLance => Color32::from_rgb(190, 240, 255),
        WeaponKind::VoidMaul => Color32::from_rgb(180, 130, 220),
    }
}
fn darker(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8)
}

/// A cached, crossfading preview of a map shown in the map-select menu. Generates the level once, picks a few
/// vantage cameras over different areas (spawn, centre, a mid room), and dissolves between them on a timer so
/// the menu shows the place from several angles without the player entering it.
struct MapPreview {
    key: u64,
    level: crate::level::Level,
    vantages: Vec<FpsCam>,
    light: crate::mesh::Light,
    fog: crate::render::Fog,
}

impl MapPreview {
    fn build(key: u64, cfg: crate::level::MapConfig) -> Self {
        let level = crate::level::Level::generate(cfg);
        let ts = level.ts();
        let center = Vec3::new(level.w as f32 * 0.5 * ts, level.floor_y, level.h as f32 * 0.5 * ts);
        // Aim at the centerpiece: the most-central room (where the landmark sits), lifted a little so the
        // structure and the ground around it are both in frame.
        let (mcx, mcz) = (center.x, center.z);
        let mut focus = center;
        let mut bd = f32::MAX;
        for r in &level.rooms {
            let d = (r.center.x - mcx).powi(2) + (r.center.z - mcz).powi(2);
            if d < bd {
                bd = d;
                focus = r.center;
            }
        }
        let target = Vec3::new(focus.x, focus.y + 3.0, focus.z);
        // Distance and height scale with the map so big maps pull back; always high enough to clear terrain
        // and look *down* on the layout at a cinematic angle.
        let reach = level.w as f32 * ts;
        let dist = (reach * 0.16).clamp(24.0, 60.0);
        let height = (reach * 0.12).clamp(16.0, 42.0);
        // Three establishing angles sweeping around the centerpiece (plus one lower approach from spawn).
        let mut vantages: Vec<FpsCam> = (0..3)
            .map(|i| {
                let ang = 0.6 + i as f32 * 2.25;
                let pos = target + Vec3::new(ang.cos() * dist, height, ang.sin() * dist);
                let f = (target - pos).normalized();
                FpsCam { pos, yaw: f.x.atan2(-f.z), pitch: f.y.clamp(-1.4, 1.4).asin() }
            })
            .collect();
        // A grounded approach shot from the spawn looking toward the centerpiece — sells the player's-eye scale.
        let sp = level.spawn(0);
        let spos = sp + Vec3::new(0.0, 6.0, 0.0);
        let sf = (Vec3::new(target.x, target.y, target.z) - spos).normalized();
        vantages.push(FpsCam { pos: spos, yaw: sf.x.atan2(-sf.z), pitch: sf.y.clamp(-1.4, 1.4).asin() });

        let pal = level.pal;
        let mut light = crate::mesh::Light::sun(Vec3::new(-0.4, 0.92, 0.42), Color32::from_rgb(255, 248, 230));
        light.sky = pal.sky;
        light.ground = Color32::from_rgb(52, 50, 56);
        light.ambient = 1.05; // brighter than in-game so the thumbnail reads clearly
        // Push the fog well back so the establishing distance doesn't grey the scene out.
        let fog = crate::render::Fog::new(pal.fog, 55.0, 280.0);
        Self { key, level, vantages, light, fog }
    }

    fn paint(&self, painter: &egui::Painter, rect: Rect, time: f32, atlas: egui::TextureId) {
        let n = self.vantages.len();
        if n == 0 {
            return;
        }
        let cycle = 4.5_f32; // seconds per vantage — a slow, readable sweep
        let phase = time / cycle;
        let i = (phase as usize) % n;
        let j = (i + 1) % n;
        let frac = phase.fract();
        // crossfade over the last 40%, eased (smoothstep) so the transition glides rather than snaps
        let raw = ((frac - 0.6) / 0.4).clamp(0.0, 1.0);
        let fade = raw * raw * (3.0 - 2.0 * raw);
        let scale = scale_for_fov(rect.height(), 52f32.to_radians());
        let wind = crate::math::Vec2::new(0.0, 0.0);
        // Collect the full renderable mesh set once: static geometry (from the LOD field if the map has one,
        // else the flat list) plus water surfaces and destructibles, so authored maps that live in the LOD
        // field or use water/props render in the thumbnail exactly as they do in-game — and textured, via the
        // real material atlas rather than the flat untextured fallback the preview used before.
        let mut meshes: Vec<crate::mesh::Mesh3> = Vec::new();
        if let Some(field) = &self.level.lod_field {
            for m in field.render_meshes() {
                meshes.push(m.clone());
            }
        } else {
            meshes.extend(self.level.meshes.iter().cloned());
        }
        for d in &self.level.destructibles {
            meshes.extend(d.mesh());
        }
        let paint_one = |cam: &FpsCam, a: f32| {
            if a <= 0.002 {
                return;
            }
            let mut mesh = crate::render::render(
                &meshes, cam, rect.center().x, rect.center().y, scale, &self.light, &self.fog, rect.width(), rect.height(), time, wind, atlas, 5, &[],
            );
            if a < 0.998 {
                for v in &mut mesh.vertices {
                    let c = v.color;
                    v.color = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * a) as u8);
                }
            }
            // Paint the rendered mesh directly. (Previously used `with_clip_rect(rect).add(...)`, which in this
            // egui version returned a detached painter whose shapes never reached the layer — so the preview
            // showed only the sky gradient. The mesh verts are already inside `rect`, so a direct add on the
            // same painter is correct and visible.)
            painter.add(egui::Shape::mesh(mesh));
        };
        paint_one(&self.vantages[i], 1.0);
        paint_one(&self.vantages[j], fade);
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn nav_and_forge_bindings_have_sensible_defaults() {
        // Both keyboard layouts must supply the navigation + forge keys, and the nav defaults must be the
        // arrow keys (with hjkl handled as always-on aliases in handle_menu_nav) so menus are navigable and
        // the forge is fully controllable out of the box.
        for b in [Bindings::dvorak(), Bindings::qwerty()] {
            assert_eq!(b.get(BindAction::NavUp), Key::ArrowUp);
            assert_eq!(b.get(BindAction::NavDown), Key::ArrowDown);
            assert_eq!(b.get(BindAction::NavLeft), Key::ArrowLeft);
            assert_eq!(b.get(BindAction::NavRight), Key::ArrowRight);
            assert_eq!(b.get(BindAction::NavSelect), Key::Enter);
            assert_eq!(b.get(BindAction::ForgeMenu), Key::Escape);
            assert_eq!(b.get(BindAction::ForgeNextBrush), Key::Tab);
            assert_eq!(b.get(BindAction::ForgeUndo), Key::Z);
        }
    }

    #[test]
    fn rebinding_a_nav_key_round_trips() {
        let mut b = Bindings::qwerty();
        b.set(BindAction::NavSelect, Key::Space);
        assert_eq!(b.get(BindAction::NavSelect), Key::Space);
        b.set(BindAction::ForgeRotate, Key::T);
        assert_eq!(b.get(BindAction::ForgeRotate), Key::T);
        // movement keys are untouched by nav/forge rebinds
        assert_eq!(b.get(BindAction::Forward), Key::W);
    }

    #[test]
    fn preview_vantages_are_elevated_and_look_down() {
        let cfg = crate::level::PRESETS[0].config(99);
        let p = MapPreview::build(0, cfg);
        assert!(p.vantages.len() >= 3, "expected several establishing vantages");
        // the orbit vantages (all but the last spawn-approach shot) must sit well above the ground and pitch
        // downward, so the thumbnail looks onto the layout rather than into a wall.
        for v in &p.vantages[..3] {
            assert!(v.pos.y > p.level.floor_y + 10.0, "vantage should be elevated");
            assert!(v.pitch < 0.0, "vantage should look downward");
        }
    }
}
