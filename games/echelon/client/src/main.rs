//! Echelon M1 client — Bevy front-end over the deterministic sim.
//!
//! One human (player 0, blue) against the scripted macro bot (player 1,
//! coral). The Bevy side never mutates game state directly: every
//! interaction becomes a `Cmd` fed into `World::step` on the fixed 22 Hz
//! clock — the exact pipeline the lockstep netcode uses, so swapping the
//! local bot for a remote player later only changes where commands come
//! from.
//!
//! Controls:
//!   WASD / arrows ........ pan          wheel ......... zoom
//!   left drag / click .... select own units (shift = add)
//!   right click .......... move / harvest (workers on a resource)
//!   A then left click .... attack-move        S ...... stop
//!   1-9 .................. train (selected structure's roster, see HUD)
//!   B .................... cycle buildable structure (selected worker),
//!                          ghost follows cursor, left click places,
//!                          Esc cancels
//!
//! Built against bevy = "0.16". If your Bevy differs slightly, the drift
//! points are listed in client/README.md (they're all one-liners).

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use std::collections::HashMap;

use echelon_bots::{bot_step, plan, Bot, Plan, CADENCE};
use echelon_data::{DataPack, Kind, MapDef, Mover, Rgb, TilesetDef};
use echelon_sim::map::{GameMap, ResKind, H_VOID};
use echelon_sim::world::Order;
use echelon_sim::{Cmd, Fx, PlayerCmd, World as SimWorld};

const TS: f32 = 16.0; // pixels per tile
const HUMAN: u8 = 0;
const ENEMY: u8 = 1;

// ---------- resources ----------

#[derive(Resource)]
struct Game {
    sim: SimWorld,
    bot: Bot,
    bot_plan: Plan,
    pending: Vec<PlayerCmd>,
    over: bool,
}

#[derive(Resource, Default)]
struct Ui {
    selection: Vec<u32>,
    drag_from: Option<Vec2>, // world coords
    attack_armed: bool,
    /// worker build placement: index into the worker's `builds` list
    build_ix: Option<usize>,
}

#[derive(Resource, Default)]
struct SpriteIndex(HashMap<u32, Entity>);

// ---------- components ----------

#[derive(Component)]
struct SimSprite(u32);
#[derive(Component)]
struct SelRing;
#[derive(Component)]
struct DragRect;
#[derive(Component)]
struct Ghost;
#[derive(Component)]
struct Hud;
#[derive(Component)]
struct Banner;

// ---------- coordinate helpers ----------

fn sim_to_px(p: echelon_sim::V2) -> Vec2 {
    Vec2::new(
        p.x.to_milli() as f32 * TS / 1000.0,
        -(p.y.to_milli() as f32 * TS / 1000.0),
    )
}
fn px_to_milli(v: Vec2) -> (i32, i32) {
    (
        (v.x * 1000.0 / TS) as i32,
        (-v.y * 1000.0 / TS) as i32,
    )
}
fn px_to_tile(v: Vec2) -> (i32, i32) {
    let (mx, my) = px_to_milli(v);
    (mx.div_euclid(1000), my.div_euclid(1000))
}
fn pcol(p: u8) -> Color {
    if p == HUMAN {
        Color::srgb(0.30, 0.64, 1.0)
    } else {
        Color::srgb(1.0, 0.42, 0.32)
    }
}

fn load_pack() -> DataPack {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/data/races");
    DataPack::load_dir(&dir, &["genetron.ron", "xayid.ron", "keiron.ron"]).expect("data pack")
}

fn load_map(name: &str) -> (GameMap, String) {
    if name == "standard" {
        return (GameMap::standard(), "relic_wastes".into());
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/data/maps");
    let def = MapDef::load(&dir, name).expect("map def");
    let ts = def.tileset.clone();
    (GameMap::from_def(&def).expect("map builds"), ts)
}

fn rgb(c: Rgb) -> Color {
    Color::srgb_u8(c.0, c.1, c.2)
}

/// tint an Rgb by k (0..~1.4) without relying on color-space helpers
fn shade(c: Rgb, k: f32) -> Color {
    Color::srgb(
        (c.0 as f32 / 255.0 * k).min(1.0),
        (c.1 as f32 / 255.0 * k).min(1.0),
        (c.2 as f32 / 255.0 * k).min(1.0),
    )
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

#[derive(Resource)]
struct Palette(TilesetDef);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r1 = args.get(1).cloned().unwrap_or_else(|| "Genetron".into());
    let r2 = args.get(2).cloned().unwrap_or_else(|| "Keiron".into());
    let seed: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(7);
    let map_name = args.get(4).cloned().unwrap_or_else(|| "confluence".into());

    let (gmap, ts_name) = load_map(&map_name);
    let tdir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/data/tilesets");
    let tileset = TilesetDef::load(&tdir, &ts_name).expect("tileset");
    let pack = load_pack();
    let ri = [
        pack.race_index(&r1).expect("race1"),
        pack.race_index(&r2).expect("race2"),
    ];
    let sim = SimWorld::new(pack, gmap, ri, seed);
    let game = Game {
        bot_plan: plan(&r2),
        bot: Bot::default(),
        sim,
        pending: Vec::new(),
        over: false,
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("Echelon — {} vs {} on {} (bot)", r1, r2, map_name),
                resolution: (1280.0, 800.0).into(),
                ..Default::default()
            }),
            ..Default::default()
        }))
        .insert_resource(ClearColor(shade(tileset.low, 0.45)))
        .insert_resource(Palette(tileset))
        .insert_resource(Time::<Fixed>::from_hz(echelon_data::TICKS_PER_SEC as f64))
        .insert_resource(game)
        .init_resource::<Ui>()
        .init_resource::<SpriteIndex>()
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, sim_tick)
        .add_systems(
            Update,
            (
                camera_controls,
                pointer_input,
                hotkeys,
                sync_sprites,
                draw_selection,
                draw_ghost,
                hud_text,
            )
                .chain(),
        )
        .run();
}

// ---------- startup ----------

fn setup(mut commands: Commands, game: Res<Game>, pal: Res<Palette>) {
    let start = game.sim.map.starts[HUMAN as usize];
    let home = sim_to_px(echelon_sim::V2::from_tile(start.0, start.1));
    commands.spawn((
        Camera2d,
        Transform::from_xyz(home.x, home.y, 999.0),
    ));

    // terrain: tileset-tinted vertical levels, decks, ramps, chasms
    let ts = &pal.0;
    for y in 0..game.sim.map.h {
        for x in 0..game.sim.map.w {
            let color = if game.sim.map.bridge(x, y) > 0 {
                shade(ts.bridge, 1.15)
            } else if game.sim.map.ramp(x, y).is_some() {
                shade(ts.ramp, 0.85)
            } else {
                match game.sim.map.hgt(x, y) {
                    H_VOID => shade(ts.rock, 0.35),
                    lvl => {
                        let t = lvl as f32 / 5.0;
                        shade(mix(ts.low, ts.high, t), 0.55 + 0.55 * t)
                    }
                }
            };
            commands.spawn((
                Sprite {
                    color,
                    custom_size: Some(Vec2::splat(TS)),
                    ..Default::default()
                },
                Transform::from_xyz(
                    x as f32 * TS + TS / 2.0,
                    -(y as f32 * TS + TS / 2.0),
                    0.0,
                ),
            ));
        }
    }
    // resource nodes
    for n in &game.sim.map.nodes {
        let (color, size, rot) = match n.kind {
            ResKind::Alloy => (rgb(pal.0.alloy_glow), TS * 0.55, 0.0),
            ResKind::Flux => (
                rgb(pal.0.flux_glow),
                TS * 0.6,
                std::f32::consts::FRAC_PI_4,
            ),
        };
        commands.spawn((
            Sprite {
                color,
                custom_size: Some(Vec2::splat(size)),
                ..Default::default()
            },
            Transform {
                translation: Vec3::new(
                    n.tx as f32 * TS + TS / 2.0,
                    -(n.ty as f32 * TS + TS / 2.0),
                    1.0,
                ),
                rotation: Quat::from_rotation_z(rot),
                ..Default::default()
            },
        ));
    }
    // drag rect + ghost (hidden until used)
    commands.spawn((
        Sprite {
            color: Color::srgba(0.6, 0.9, 1.0, 0.15),
            custom_size: Some(Vec2::ZERO),
            ..Default::default()
        },
        Transform::from_xyz(0.0, 0.0, 50.0),
        Visibility::Hidden,
        DragRect,
    ));
    commands.spawn((
        Sprite {
            color: Color::srgba(0.4, 1.0, 0.5, 0.4),
            custom_size: Some(Vec2::splat(TS * 3.0)),
            ..Default::default()
        },
        Transform::from_xyz(0.0, 0.0, 60.0),
        Visibility::Hidden,
        Ghost,
    ));
    // HUD
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: 15.0,
            ..Default::default()
        },
        TextColor(Color::srgb(0.83, 0.88, 0.93)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(8.0),
            top: Val::Px(6.0),
            ..Default::default()
        },
        Hud,
    ));
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: 42.0,
            ..Default::default()
        },
        TextColor(Color::srgb(1.0, 0.9, 0.4)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(38.0),
            top: Val::Percent(45.0),
            ..Default::default()
        },
        Banner,
    ));
}

// ---------- fixed-rate sim ----------

fn sim_tick(mut game: ResMut<Game>, mut ui: ResMut<Ui>) {
    if game.over {
        return;
    }
    let mut cmds = std::mem::take(&mut game.pending);
    if game.sim.tick % CADENCE == 0 {
        // the opponent thinks once a second, exactly like the headless runner
        let Game {
            ref sim,
            ref mut bot,
            ref bot_plan,
            ..
        } = *game;
        bot_step(sim, ENEMY, bot_plan, bot, &mut cmds);
    }
    game.sim.step(&cmds);
    if game.sim.defeated.iter().any(|d| *d) {
        game.over = true;
    }
    // drop dead ids from the selection
    let sim = &game.sim;
    ui.selection.retain(|&i| {
        (i as usize) < sim.ents.len() && sim.ents[i as usize].alive
    });
}

// ---------- camera ----------

fn camera_controls(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: EventReader<MouseWheel>,
    mut cam: Query<&mut Transform, With<Camera2d>>,
) {
    let Ok(mut t) = cam.single_mut() else {
        return;
    };
    let mut d = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        d.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) && keys.pressed(KeyCode::ShiftLeft) {
        // shift+S reserved: nothing (S alone is Stop)
    }
    if keys.pressed(KeyCode::ArrowDown) {
        d.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) && false {
        // A is attack-arm; use arrows/D+W for pan symmetry
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        d.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        d.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyQ) {
        d.x -= 1.0; // extra left-pan since A is taken
    }
    let speed = 600.0 * t.scale.x;
    t.translation += (d.normalize_or_zero() * speed * time.delta_secs()).extend(0.0);
    for ev in wheel.read() {
        let z = 1.0 - ev.y.clamp(-3.0, 3.0) * 0.1;
        let s = (t.scale.x * z).clamp(0.35, 3.0);
        t.scale = Vec3::new(s, s, 1.0);
    }
}

fn cursor_world(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cam: &Query<(&Camera, &GlobalTransform), With<Camera2d>>,
) -> Option<Vec2> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, gt) = cam.single().ok()?;
    camera.viewport_to_world_2d(gt, cursor).ok()
}

// ---------- pointer: select / orders / placement ----------

#[allow(clippy::too_many_arguments)]
fn pointer_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut ui: ResMut<Ui>,
    mut game: ResMut<Game>,
    mut drag_q: Query<(&mut Transform, &mut Sprite, &mut Visibility), With<DragRect>>,
) {
    let Some(cursor) = cursor_world(&windows, &cam) else {
        return;
    };

    // --- build placement takes priority over selection ---
    if let Some(bix) = ui.build_ix {
        if buttons.just_pressed(MouseButton::Left) {
            if let Some(&worker) = ui.selection.first() {
                let wdef = game.sim.ents[worker as usize].def;
                let builds = game.sim.pack.def(wdef).builds.clone();
                if let Some(name) = builds.get(bix) {
                    let unit = game.sim.pack.id(name);
                    let fp = game.sim.pack.def(unit).fp().max(1);
                    let (cx, cy) = px_to_tile(cursor);
                    let (tx, ty) = (cx - fp / 2, cy - fp / 2);
                    game.pending.push(PlayerCmd {
                        player: HUMAN,
                        cmd: Cmd::Build {
                            worker,
                            unit,
                            tx,
                            ty,
                        },
                    });
                }
            }
            ui.build_ix = None;
            return;
        }
        if buttons.just_pressed(MouseButton::Right) {
            ui.build_ix = None;
            return;
        }
        return; // while placing, ignore selection input
    }

    // --- attack-armed left click ---
    if ui.attack_armed && buttons.just_pressed(MouseButton::Left) {
        if !ui.selection.is_empty() {
            let (mx, my) = px_to_milli(cursor);
            let ids = ui.selection.clone();
            game.pending.push(PlayerCmd {
                player: HUMAN,
                cmd: Cmd::Attack { ids, mx, my },
            });
        }
        ui.attack_armed = false;
        return;
    }

    // --- drag select ---
    if buttons.just_pressed(MouseButton::Left) {
        ui.drag_from = Some(cursor);
    }
    if let Some(from) = ui.drag_from {
        if let Ok((mut t, mut s, mut vis)) = drag_q.single_mut() {
            let min = from.min(cursor);
            let max = from.max(cursor);
            t.translation = ((min + max) / 2.0).extend(50.0);
            s.custom_size = Some(max - min);
            *vis = if (max - min).length() > 4.0 {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
    if buttons.just_released(MouseButton::Left) {
        if let Some(from) = ui.drag_from.take() {
            if let Ok((_, _, mut vis)) = drag_q.single_mut() {
                *vis = Visibility::Hidden;
            }
            let additive = keys.pressed(KeyCode::ShiftLeft);
            if !additive {
                ui.selection.clear();
            }
            let min = from.min(cursor);
            let max = from.max(cursor);
            let box_select = (max - min).length() > 8.0;
            let sim = &game.sim;
            if box_select {
                for (i, e) in sim.ents.iter().enumerate() {
                    if e.alive && e.player == HUMAN && e.def != u16::MAX {
                        let d = sim.pack.def(e.def);
                        if d.kind == Kind::Structure {
                            continue; // box grabs units only
                        }
                        let p = sim_to_px(e.pos);
                        if p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y {
                            let id = i as u32;
                            if !ui.selection.contains(&id) {
                                ui.selection.push(id);
                            }
                        }
                    }
                }
            } else {
                // click: nearest own thing within ~1.2 tiles (structures ok)
                let mut best = (TS * 1.2, None);
                for (i, e) in sim.ents.iter().enumerate() {
                    if e.alive && e.player == HUMAN && e.def != u16::MAX {
                        let d = sim_to_px(e.pos).distance(cursor);
                        let fp = sim.pack.def(e.def).fp() as f32;
                        let reach = (TS * 1.2).max(fp * TS * 0.6);
                        if d < reach && d < best.0 {
                            best = (d, Some(i as u32));
                        }
                    }
                }
                if let Some(id) = best.1 {
                    if !ui.selection.contains(&id) {
                        ui.selection.push(id);
                    }
                }
            }
        }
    }

    // --- right click: move / harvest ---
    if buttons.just_pressed(MouseButton::Right) && !ui.selection.is_empty() {
        let sim = &game.sim;
        // resource under cursor?
        let mut node = None;
        for (ni, n) in sim.map.nodes.iter().enumerate() {
            let np = Vec2::new(
                n.tx as f32 * TS + TS / 2.0,
                -(n.ty as f32 * TS + TS / 2.0),
            );
            if np.distance(cursor) < TS * 1.4 && n.amount > 0 {
                node = Some(ni as u32);
            }
        }
        let workers: Vec<u32> = ui
            .selection
            .iter()
            .copied()
            .filter(|&i| sim.pack.def(sim.ents[i as usize].def).kind == Kind::Worker)
            .collect();
        if let (Some(n), false) = (node, workers.is_empty()) {
            for w in workers {
                game.pending.push(PlayerCmd {
                    player: HUMAN,
                    cmd: Cmd::Harvest { id: w, node: n },
                });
            }
        } else {
            let (mx, my) = px_to_milli(cursor);
            let ids = ui.selection.clone();
            game.pending.push(PlayerCmd {
                player: HUMAN,
                cmd: Cmd::Move { ids, mx, my },
            });
        }
    }
}

// ---------- keyboard ----------

fn hotkeys(keys: Res<ButtonInput<KeyCode>>, mut ui: ResMut<Ui>, mut game: ResMut<Game>) {
    if keys.just_pressed(KeyCode::KeyA) {
        ui.attack_armed = !ui.attack_armed;
    }
    if keys.just_pressed(KeyCode::Escape) {
        ui.attack_armed = false;
        ui.build_ix = None;
    }
    if keys.just_pressed(KeyCode::KeyS) && !ui.selection.is_empty() {
        let ids = ui.selection.clone();
        game.pending.push(PlayerCmd {
            player: HUMAN,
            cmd: Cmd::Stop { ids },
        });
    }
    // B cycles the selected worker's build list
    if keys.just_pressed(KeyCode::KeyB) {
        if let Some(&sel) = ui.selection.first() {
            let sim = &game.sim;
            let d = sim.pack.def(sim.ents[sel as usize].def);
            if d.kind == Kind::Worker && !d.builds.is_empty() {
                ui.build_ix = Some(match ui.build_ix {
                    Some(i) => (i + 1) % d.builds.len(),
                    None => 0,
                });
            }
        }
    }
    // 1-9 train from a selected production structure
    const DIGITS: [KeyCode; 9] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    for (k, key) in DIGITS.iter().enumerate() {
        if keys.just_pressed(*key) {
            if let Some(&sel) = ui.selection.first() {
                let sim = &game.sim;
                let e = &sim.ents[sel as usize];
                if e.player == HUMAN {
                    let d = sim.pack.def(e.def);
                    if let Some(name) = d.trains.get(k) {
                        let unit = sim.pack.id(name);
                        game.pending.push(PlayerCmd {
                            player: HUMAN,
                            cmd: Cmd::Train {
                                structure: sel,
                                unit,
                            },
                        });
                    }
                }
            }
        }
    }
}

// ---------- rendering sync ----------

fn sync_sprites(
    mut commands: Commands,
    game: Res<Game>,
    mut index: ResMut<SpriteIndex>,
    mut q: Query<(&mut Transform, &mut Sprite), With<SimSprite>>,
) {
    let sim = &game.sim;
    for (i, e) in sim.ents.iter().enumerate() {
        let id = i as u32;
        let live = e.alive && e.def != u16::MAX;
        match (live, index.0.get(&id).copied()) {
            (false, Some(ent)) => {
                commands.entity(ent).despawn();
                index.0.remove(&id);
            }
            (false, None) => {}
            (true, existing) => {
                let d = sim.pack.def(e.def);
                let p = sim_to_px(e.pos);
                let (size, z, rot) = match d.kind {
                    Kind::Structure => (Vec2::splat(d.fp() as f32 * TS * 0.92), 2.0, 0.0),
                    Kind::Worker => (Vec2::splat(TS * 0.4), 3.0, 0.0),
                    Kind::Unit => {
                        let big = d.supply2 >= 8;
                        let s = if big { TS * 0.7 } else { TS * 0.5 };
                        let rot = if d.mover() == Mover::Air {
                            std::f32::consts::FRAC_PI_4
                        } else {
                            0.0
                        };
                        (Vec2::splat(s), 3.0, rot)
                    }
                };
                let mut color = pcol(e.player);
                if d.kind == Kind::Structure && e.progress > 0 {
                    color = color.with_alpha(0.45);
                }
                match existing {
                    Some(ent) => {
                        if let Ok((mut t, mut s)) = q.get_mut(ent) {
                            t.translation = p.extend(z);
                            t.rotation = Quat::from_rotation_z(rot);
                            s.color = color;
                            s.custom_size = Some(size);
                        }
                    }
                    None => {
                        let ent = commands
                            .spawn((
                                Sprite {
                                    color,
                                    custom_size: Some(size),
                                    ..Default::default()
                                },
                                Transform {
                                    translation: p.extend(z),
                                    rotation: Quat::from_rotation_z(rot),
                                    ..Default::default()
                                },
                                SimSprite(id),
                            ))
                            .id();
                        index.0.insert(id, ent);
                    }
                }
            }
        }
    }
}

fn draw_selection(
    mut commands: Commands,
    ui: Res<Ui>,
    game: Res<Game>,
    rings: Query<Entity, With<SelRing>>,
) {
    for r in rings.iter() {
        commands.entity(r).despawn();
    }
    let sim = &game.sim;
    for &i in &ui.selection {
        let e = &sim.ents[i as usize];
        let d = sim.pack.def(e.def);
        let s = match d.kind {
            Kind::Structure => d.fp() as f32 * TS,
            _ => TS * 0.75,
        };
        commands.spawn((
            Sprite {
                color: Color::srgba(1.0, 1.0, 1.0, 0.28),
                custom_size: Some(Vec2::splat(s + 4.0)),
                ..Default::default()
            },
            Transform::from_translation(sim_to_px(e.pos).extend(2.5)),
            SelRing,
        ));
    }
}

fn draw_ghost(
    ui: Res<Ui>,
    game: Res<Game>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut q: Query<(&mut Transform, &mut Sprite, &mut Visibility), With<Ghost>>,
) {
    let Ok((mut t, mut s, mut vis)) = q.single_mut() else {
        return;
    };
    let Some(bix) = ui.build_ix else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(&worker) = ui.selection.first() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(cursor) = cursor_world(&windows, &cam) else {
        return;
    };
    let sim = &game.sim;
    let wd = sim.pack.def(sim.ents[worker as usize].def);
    let Some(name) = wd.builds.get(bix) else {
        *vis = Visibility::Hidden;
        return;
    };
    let def = sim.pack.def(sim.pack.id(name));
    let fp = def.fp().max(1);
    let (cx, cy) = px_to_tile(cursor);
    let (tx, ty) = (cx - fp / 2, cy - fp / 2);
    let ok = if def.gas {
        sim.map.nodes.iter().any(|n| {
            n.kind == ResKind::Flux && n.tx == tx + fp / 2 && n.ty == ty + fp / 2 && n.taken_by == 0
        })
    } else {
        sim.map.area_buildable(tx, ty, fp)
    };
    s.color = if ok {
        Color::srgba(0.4, 1.0, 0.5, 0.4)
    } else {
        Color::srgba(1.0, 0.35, 0.3, 0.4)
    };
    s.custom_size = Some(Vec2::splat(fp as f32 * TS));
    t.translation = Vec3::new(
        (tx as f32 + fp as f32 / 2.0) * TS,
        -((ty as f32 + fp as f32 / 2.0) * TS),
        60.0,
    );
    *vis = Visibility::Visible;
}

// ---------- HUD ----------

fn hud_text(
    game: Res<Game>,
    ui: Res<Ui>,
    mut hud: Query<&mut Text, (With<Hud>, Without<Banner>)>,
    mut banner: Query<&mut Text, (With<Banner>, Without<Hud>)>,
) {
    let sim = &game.sim;
    let (a, f) = sim.res[HUMAN as usize];
    let mut line = format!(
        "alloy {}   flux {}   supply {}/{}   t{} ({}:{:02})",
        a,
        f,
        sim.used2[HUMAN as usize] / 2,
        sim.cap2[HUMAN as usize] / 2,
        sim.tick,
        sim.tick / 22 / 60,
        sim.tick / 22 % 60
    );
    if ui.attack_armed {
        line.push_str("   [ATTACK — left click a target point]");
    }
    if let Some(&sel) = ui.selection.first() {
        let e = &sim.ents[sel as usize];
        let d = sim.pack.def(e.def);
        line.push_str(&format!(
            "\nselected: {} x{}   hp {}",
            d.name,
            ui.selection.len(),
            e.hp.to_int_floor().max(0)
        ));
        if !d.trains.is_empty() && d.kind == Kind::Structure {
            line.push_str("\ntrain: ");
            for (i, n) in d.trains.iter().take(9).enumerate() {
                line.push_str(&format!("[{}]{} ", i + 1, n));
            }
            if !e.queue.is_empty() {
                line.push_str(&format!("  (queue {})", e.queue.len()));
            }
        }
        if d.kind == Kind::Worker {
            match ui.build_ix {
                Some(bix) => line.push_str(&format!(
                    "\nplacing: {}  (B next, click place, Esc cancel)",
                    d.builds.get(bix).map(|s| s.as_str()).unwrap_or("?")
                )),
                None => line.push_str("\n[B] build structures"),
            }
        }
    }
    let _ = Fx::from_int(0); // keep the sim math linked in release builds
    if let Ok(mut t) = hud.single_mut() {
        *t = Text::new(line);
    }
    if let Ok(mut b) = banner.single_mut() {
        let msg = if game.over {
            if game.sim.defeated[ENEMY as usize] {
                "VICTORY"
            } else {
                "DEFEAT"
            }
        } else {
            ""
        };
        *b = Text::new(msg);
    }
}

// keep Order in the public surface the client understands (future: rally UI)
#[allow(dead_code)]
fn order_label(o: &Order) -> &'static str {
    match o {
        Order::Idle => "idle",
        Order::Move(_) => "moving",
        Order::AttackMove(_) => "attacking",
        Order::HarvestGo(_) | Order::HarvestReturn(_) => "harvesting",
        Order::BuildGo(_) | Order::Constructing(_) => "building",
    }
}
