// Headless harness: renders the game (no window) via Vulkan/GL — used to
// capture screenshots for verification on machines without a display.

use glam::{Mat4, Vec3};

use crate::render::{Camera, Renderer};
use crate::scene::Scene;
use crate::sim::{Game, Rng, Team};
use crate::ui::{self, UiState};
use crate::vfx::Vfx;

pub(crate) fn make_device() -> Option<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    // Metal on macOS, then Vulkan → GL elsewhere; never fail with a panic.
    let mut masks = Vec::new();
    if cfg!(target_os = "macos") {
        masks.push(wgpu::Backends::METAL);
    } else {
        masks.push(wgpu::Backends::VULKAN);
        masks.push(wgpu::Backends::GL);
    }
    for mask in masks {
        // Contain wgpu-internal panics (missing EGL/driver) just like app.rs.
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<
            (wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue),
            String,
        > {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: mask,
                ..Default::default()
            });
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
            .ok_or_else(|| "no adapter".to_string())?;
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("headless"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            ))
            .map_err(|e| e.to_string())?;
            Ok((instance, adapter, device, queue))
        }));
        match attempt {
            Ok(Ok((instance, adapter, device, queue))) => {
                eprintln!("[clawbound] headless GPU ok — {:?}", adapter.get_info().backend);
                return Some((instance, adapter, device, queue));
            }
            Ok(Err(e)) => eprintln!("[clawbound] headless: {mask:?} unavailable: {e}"),
            Err(_) => eprintln!("[clawbound] headless: {mask:?} crashed internally, skipping"),
        }
    }
    None
}

const W: u32 = 1280;
const H: u32 = 720;

pub fn run() -> Result<(), String> {
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("shop") {
        return run_shop_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("pass") {
        return run_pass_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("intro") {
        return run_intro_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("perf") {
        return run_perf();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("switch") {
        return run_switch_check();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("select") {
        return run_select_shot();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("aim") {
        return run_aim_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("blast") {
        return run_blast_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("ultra") {
        return run_ultra_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("thunder") {
        return run_thunder_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("wet") {
        return run_wet_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("surface") {
        return run_surface_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("settings") {
        return run_settings_shot();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("abilityfx") {
        return run_abilityfx_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("quality") {
        return run_quality_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("spectate") {
        return run_spectate();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("sprout") {
        return run_sprout_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("atlantis") {
        return crate::atlantisshots::run();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("flare") {
        return run_flare_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("glades") {
        return run_glade_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("hud") {
        return run_hud_shots();
    }
    if std::env::var("CLAWBOUND_SHOTS").as_deref() == Ok("sky") {
        return crate::skyshots::run();
    }
    let (_instance, _adapter, device, queue) =
        make_device().ok_or_else(|| "No suitable GPU backend found (install mesa-vulkan-drivers for lavapipe)".to_string())?;
    let adapter_info = {
        // fetch a rough name via adapter? not critical
        "headless".to_string()
    };
    let _ = adapter_info;

    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shot target"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());

    println!("building voxel world…");
    let scene = Scene::new(&mut renderer);
    println!("total voxel instances uploaded: {}", scene.total_voxels);

    // demo match: all AI driven
    let mut rng = Rng::new(0xD30);
    let mut pool: Vec<u32> = (0..crate::gods::ROSTER_LEN as u32).collect();
    let player = pool.remove((rng.next_u() % crate::gods::ROSTER_LEN as u64) as usize);
    let mut allies = [0u32; 4];
    let mut enemies = [0u32; 5];
    for a in allies.iter_mut() {
        *a = pool.remove((rng.next_u() % pool.len() as u64) as usize);
    }
    for e in enemies.iter_mut() {
        *e = pool.remove((rng.next_u() % pool.len() as u64) as usize);
    }
    let mut game = Game::new(player, allies, enemies);
    // As app.rs does. Without this the harness simulated a world with no
    // props in it, which is not the game anyone plays.
    game.blockers = scene.blockers.clone();
    // drive the player slot with AI for the demo
    let lane = crate::consts::Lane::Mid;
    game.gods[0].ai = Some({ let mut b = crate::sim::AiBrain::new(lane, 0.9); b.buy_t = 1.0; b });
    game.open_playbook(&mut Rng::new(0x9A6F));

    // egui for HUD in screenshots
    let egui_ctx = egui::Context::default();
    let mut ui_state = UiState::new();

    // portraits
    for i in 0..crate::gods::ROSTER.len() {
        let body = scene.god_body[i][0];
        let weapon = scene.god_weapon[i][0];
        let pixels = renderer.render_portrait(body, weapon);
        let image = egui::ColorImage::from_rgba_unmultiplied([128, 128], &pixels);
        let tid = egui_ctx.load_texture(format!("ancestor{}", i), image, egui::TextureOptions::LINEAR);
        ui_state.portraits.insert(i as u32, tid.id());
    }
    println!("portraits rendered ({})", ui_state.portraits.len());

    let mut vfx = Vfx::default();
    let mut sim_time = 0.0f32;
    let captures: &[(f32, &str)] = &[
        (8.0, "clawbound_select.png"),
        (13.0, "clawbound_horizon.png"),
        (55.0, "clawbound_battle_mid.png"),
        (56.0, "clawbound_orbit.png"),
        (110.0, "clawbound_battle_late.png"),
    ];
    let mut next_capture = 0usize;

    // capture god select screen first
    {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
            time: Some(0.0),
            ..Default::default()
        };
        let mut locked = true;
        let output = egui_ctx.run(raw, |ctx| {
            ui::draw_select(ctx, &mut ui_state, &mut locked);
        });
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &egui_ctx, output, W, H);
        save_shot(&renderer.device, &renderer.queue, &target, captures[0].1)?;
        next_capture = 1;
        println!("saved {}", captures[0].1);
    }

    let steps_per_frame = 2; // sim at 120Hz
    let mut frame: u64 = 0;
    let total = 120.0f32;

    while next_capture < captures.len() {
        let cap_t = captures[next_capture].0;
        // fast-forward the simulation to the next capture point (no render)
        while sim_time < cap_t.min(total) {
            for _ in 0..steps_per_frame {
                let _n0 = game.events.len();
                game.tick(1.0 / 120.0);
                let events: Vec<crate::sim::Event> = game.events.drain(..).collect();
                vfx.update(1.0 / 120.0, &events, &game);
                for e in &events {
                    match e {
                        crate::sim::Event::Damage { pos, amount, big } => {
                            if ui_state.floaters.len() < 50 && *amount > 0.0 {
                                ui_state.push_events(&[ui::Event2::Damage { pos: *pos, amount: *amount, big: *big }]);
                            }
                        }
                        crate::sim::Event::Announce { text } => {
                            ui_state.push_events(&[ui::Event2::Announce { text: text.clone(), dur: 2.5 }]);
                        }
                        _ => {}
                    }
                }
                sim_time += 1.0 / 120.0;
            }
            ui_state.tick(1.0 / 60.0);
            if game.over.is_some() {
                println!("match over at t={:.1}", sim_time);
                break;
            }
        }
        if game.over.is_some() && sim_time < cap_t {
            // capture what we have anyway
        }

        // script an ultimate barrage right before battle captures
        if !captures[next_capture].1.contains("orbit") {
            for i in 0..game.gods.len() {
                if game.gods[i].respawn_t <= 0.0 {
                    let mut aim = game.gods[i].pos + Vec3::X * 5.0;
                    for e in &game.gods {
                        if e.team != game.gods[i].team && e.respawn_t <= 0.0 {
                            aim = e.pos;
                            break;
                        }
                    }
                    game.gods[i].cds = [0.0; 4];
                    game.gods[i].mana = game.gods[i].max_mana;
                    game.cast_queue.push((i, 3, aim));
                    game.cast_queue.push((i, 1, aim));
                }
            }
            for _ in 0..40 {
                game.tick(1.0 / 120.0);
                let events: Vec<crate::sim::Event> = game.events.drain(..).collect();
                vfx.update(1.0 / 120.0, &events, &game);
                sim_time += 1.0 / 120.0;
            }
        }

        // action camera: frame the player + nearest enemy fight
        let p = &game.gods[0];
        let mut cam_target = p.pos + Vec3::Y * 1.4;
        let mut nearest: Option<(f32, Vec3)> = None;
        for g2 in &game.gods {
            if g2.team != p.team && g2.respawn_t <= 0.0 {
                let d = g2.pos.distance(p.pos);
                if d < 26.0 && nearest.map_or(true, |(bd, _)| d < bd) {
                    nearest = Some((d, g2.pos));
                }
            }
        }
        if let Some((_, ep)) = nearest {
            cam_target = (p.pos + ep) * 0.5 + Vec3::Y * 1.6;
        }
        let yaw = sim_time * 0.11 + 2.4;
        let cam = if captures[next_capture].1.contains("orbit") {
            let a = sim_time * 0.02;
            Camera {
                eye: Vec3::new(a.cos() * 70.0, 55.0, a.sin() * 70.0),
                target: Vec3::ZERO,
                fov_y: 0.9,
            }
        } else if captures[next_capture].1.contains("horizon") {
            // low angle from the blue jungle toward the red base + north ziggurat
            Camera {
                eye: Vec3::new(34.0, 8.0, -84.0),
                target: Vec3::new(-8.0, 20.0, 130.0),
                fov_y: 1.0,
            }
        } else {
            Camera {
                eye: cam_target + Vec3::new(yaw.cos() * 7.5, 6.0, yaw.sin() * 7.5),
                target: cam_target,
                fov_y: 0.95,
            }
        };

        renderer.atmosphere = game.weather.atmosphere();
        renderer.event = (game.disasters.flood_rise, game.disasters.shake, 0.0);
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.render_frame(&view, cam, sim_time);

        // HUD overlay
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
            time: Some(sim_time as f64),
            ..Default::default()
        };
        let vp = cam.view_proj(W as f32 / H as f32);
        let output = egui_ctx.run(raw, |ctx| {
            ui::draw_hud(ctx, &game, 0, vp, W as f32, H as f32, &mut ui_state, renderer.camera.eye);
            if captures[next_capture].1.contains("orbit") {
                ui::draw_scoreboard(ctx, &game);
            }
        });
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &egui_ctx, output, W, H);

        save_shot(&renderer.device, &renderer.queue, &target, captures[next_capture].1)?;
        println!(
            "saved {} (t={:.0})  ancestors alive: {}  minions: {}  towers: {}/{}",
            captures[next_capture].1,
            sim_time,
            game.gods.iter().filter(|g| g.respawn_t <= 0.0).count(),
            game.minions.len(),
            game.tower_alive[0],
            game.tower_alive[1]
        );
        next_capture += 1;
        frame += 1;
        if game.over.is_some() {
            break;
        }
    }

    println!("headless render complete. {} frames simulated.", frame);
    Ok(())
}

pub(crate) fn draw_egui(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    egui_renderer: &mut egui_wgpu::Renderer,
    view: &wgpu::TextureView,
    ctx: &egui::Context,
    output: egui::FullOutput,
    w: u32,
    h: u32,
) {
    for td in output.textures_delta.set.iter() {
        egui_renderer.update_texture(device, queue, td.0, &td.1);
    }
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: ctx.pixels_per_point(),
    };
    let clipped = ctx.tessellate(output.shapes, screen.pixels_per_point);
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut cbuffers = egui_renderer.update_buffers(device, queue, &mut encoder, &clipped, &screen);
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui headless"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        egui_renderer.render(&mut rpass, &clipped, &screen);
    }
    cbuffers.push(encoder.finish());
    queue.submit(cbuffers);
    for td in output.textures_delta.free.iter() {
        egui_renderer.free_texture(td);
    }
}

pub(crate) fn save_shot(device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::Texture, path: &str) -> Result<(), String> {
    let (w, h) = (target.width(), target.height());
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot read"),
        size: (w * h * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &read_buf,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = read_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range();
    let mut img = Vec::with_capacity((w * h * 4) as usize);
    // remove row padding (none here since bytes_per_row == w*4)
    img.extend_from_slice(&data);
    drop(data);
    read_buf.unmap();
    let out = image::RgbaImage::from_raw(w, h, img).ok_or("image layout")?;
    out.save(path).map_err(|e| e.to_string())?;
    let _ = Mat4::IDENTITY;
    let _ = Team::Blue;
    Ok(())
}


/// `--rig-sheet`: close-ups of ancestors in staged states.
///
/// Mirrors `app.rs`'s frame: `vfx.update` then `Scene::draw` then
/// `render_frame`, with the scene's own blockers handed to the game. The sim
/// is NOT ticked — states are staged by writing `GodUnit` fields the way the
/// sim does (`atk_anim = 1.0` on a swing, a `DashState` for a dash), so what
/// is drawn is what the draw path makes of real sim state, not a pose the
/// game never produces.
pub fn run_rig_sheet() -> Result<(), String> {
    use crate::puppet::CastStyle;
    use crate::sim::DashState;
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sheet target"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);

    // A real fern and a real root, from the scene's own blockers.
    let fern = scene.blockers.all().iter().find_map(|b| {
        let c = b.cover;
        (c.amount > 0.5 && c.amount < 0.6 && c.x.abs() < 60.0 && c.z.abs() < 60.0).then_some(Vec3::new(c.x, 0.0, c.z))
    });
    let root = scene.blockers.steps().first().map(|s| match s.shape {
        crate::blockers::Shape::Segment { x0, z0, x1, z1, .. } => (Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1)),
        crate::blockers::Shape::Circle { x, z, .. } => (Vec3::new(x, 0.0, z), Vec3::new(x + 1.0, 0.0, z)),
    });
    println!("fern at {fern:?}; root at {root:?}; {} root steps", scene.blockers.steps().len());

    struct Shot { name: &'static str, eye: Vec3, look: Vec3 }
    let row = Vec3::new(0.0, 0.0, -40.0);
    let mut shots = vec![Shot { name: "rig_lineup.png", eye: row + Vec3::new(0.0, 6.5, -12.0), look: row + Vec3::Y * 0.6 }];
    if let Some((a, b)) = root {
        let mid = (a + b) * 0.5;
        let along = (b - a).normalize_or_zero();
        let side = Vec3::new(-along.z, 0.0, along.x);
        shots.push(Shot { name: "rig_on_root.png", eye: mid + side * 1.2 + along * 6.5 + Vec3::Y * 2.4, look: mid + Vec3::Y * 1.6 });
    }
    if let Some(f) = fern {
        shots.push(Shot { name: "rig_in_fern.png", eye: f + Vec3::new(-2.2, 6.5, 3.4), look: f + Vec3::Y * 0.4 });
    }

    // Signature frames: every ancestor casting its ultimate, in a file down
    // the lane, seen from the side. 0..=5 then 6..=12.
    let file = |k: usize| Vec3::new(0.0, 0.0, -56.0 + k as f32 * 5.2);
    for (name, count) in [("sig_0_to_5.png", 6usize), ("sig_6_to_12.png", 7)] {
        let mid = (file(0) + file(count - 1)) * 0.5;
        shots.push(Shot { name, eye: mid + Vec3::new(-17.0, 13.0, 0.0), look: mid + Vec3::Y * 1.0 });
    }
    let sig_first = shots.len() - 2;

    // A real wall from the scene: one cat trotting its top, one knocked off
    // it a tenth of a second ago and halfway through righting itself.
    let wall = scene.blockers.with_perch().find_map(|(b, p)| match (b.shape, p) {
        (crate::blockers::Shape::Segment { x0, z0, x1, z1, .. }, crate::blockers::Perch::Narrow)
            if (x1 - x0).hypot(z1 - z0) > 6.0 && x0.hypot(z0) < 120.0 => Some((Vec3::new(x0, b.top, z0), Vec3::new(x1, b.top, z1))),
        _ => None,
    });
    let wall_shot = shots.len();
    if let Some((a, b)) = wall {
        let mid = (a + b) * 0.5;
        let along = (b - a).normalize_or_zero();
        let side = Vec3::new(-along.z, 0.0, along.x);
        shots.push(Shot { name: "rig_wall_and_fall.png", eye: mid + side * 8.0 + Vec3::Y * 3.5, look: mid - Vec3::Y * 0.6 });
    }
    // The authored centre from above: the Hollow, and a run either side of the lane.
    shots.push(Shot { name: "map_centre.png", eye: Vec3::new(0.0, 62.0, -46.0), look: Vec3::new(0.0, 0.0, 8.0) });
    // A grove (scene::groves): the first bramble bed in the going layer.
    let grove = scene.blockers.going().all().iter().find_map(|r| match r.shape {
        // r > 2.5: a bamboo stand is also a 0.55 thicket, 1.5 across.
        crate::blockers::Shape::Circle { x, z, r: rad } if r.mul == crate::scene::BRAMBLE_MUL && rad > 2.5 => Some(Vec3::new(x, 0.0, z)),
        _ => None,
    });
    if let Some(g) = grove {
        shots.push(Shot { name: "map_grove.png", eye: g + Vec3::new(-11.0, 5.0, -11.0), look: g + Vec3::Y * 0.5 });
        shots.push(Shot { name: "map_grove_above.png", eye: g + Vec3::new(-4.0, 42.0, -6.0), look: g });
    }
    // The deep jungle ring from above: west of the west river, the ground
    // JUNGLE_MARGIN added.
    // The whole valley from straight above (near enough): where the props are.
    shots.push(Shot { name: "map_overview.png", eye: Vec3::new(0.0, 330.0, -60.0), look: Vec3::ZERO });
    shots.push(Shot { name: "map_deep_jungle.png", eye: Vec3::new(-150.0, 70.0, -60.0), look: Vec3::new(-135.0, 0.0, 0.0) });
    // Paws in motion: one cat trotting across open ground, seen side-on at
    // paw height, captured on several frames (`rig_trot_strip_<step>.png`).
    let trot_at = Vec3::new(0.0, 0.0, -48.0) + Vec3::X * 20.0;
    shots.push(Shot { name: "rig_trot_strip.png", eye: trot_at + Vec3::new(0.0, 1.1, -4.2), look: trot_at + Vec3::Y * 0.45 });
    // A strike in the Hollow, a third of a second on: the front is among the
    // trees, the ground is freshly broken, and an older crack beside it is
    // half healed. Gameplay camera distance, so the fitted shadow box is tight.
    shots.push(Shot { name: "fx_blast.png", eye: Vec3::new(-21.0, 8.0, -1.0), look: Vec3::new(-9.0, 0.6, 10.0) });
    // A crossing: the log in its seat with the river running under it.
    if let Some(c) = crate::logs::crossings().first() {
        let down = Vec3::new(-c.along.z, 0.0, c.along.x);
        shots.push(Shot { name: "log_crossing.png", eye: c.at - down * 15.0 + c.along * 6.0 + Vec3::Y * 9.0, look: c.at });
    }
    // An enemy tower with the player just inside its line: the ring solid red,
    // the tether on the player.
    {
        let t = consts_tower();
        shots.push(Shot { name: "tower_line.png", eye: t + Vec3::new(30.0, 26.0, -22.0), look: t + Vec3::new(10.0, 0.0, 0.0) });
    }
    println!("wall at {wall:?}");
    // CLAWBOUND_SHEET=wall,sig_0 renders only the shots whose names match.
    let only: Vec<String> = std::env::var("CLAWBOUND_SHEET").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();

    for (si, shot) in shots.iter().enumerate() {
        if !only.is_empty() && !only.iter().any(|o| shot.name.contains(o.as_str())) {
            continue;
        }
        let mut game = if si == sig_first + 1 {
            Game::new(6, [7, 8, 9, 10], [11, 12, 0, 1, 2])
        } else {
            Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9])
        };
        game.blockers = scene.blockers.clone();
        let mut vfx = Vfx::default();
        let cam = Camera { eye: shot.eye, target: shot.look, fov_y: if shot.name == "map_overview.png" { 1.15 } else if si == 0 || si >= sig_first { 1.0 } else { 0.6 } };
        const STEPS: usize = 48;
        for step in 0..STEPS {
            let left = STEPS - 1 - step;
            match si {
                0 => {
                    // Six cats in a row facing +X (yaw 0), seen from -Z.
                    for k in 0..6 {
                        let g = &mut game.gods[k];
                        g.yaw = 0.0;
                        g.pos = row + Vec3::X * (k as f32 * 4.6 - 11.5);
                    }
                    // The movers ARRIVE at their slot on the capture frame.
                    game.gods[0].pos.x -= left as f32 * (5.0 / 60.0); // trotting +X
                    game.gods[1].pos.x -= left as f32 * (11.0 / 60.0); // bounding
                    if left == 10 {
                        game.gods[2].atk_anim = 1.0; // the sim's swing pulse
                    }
                    game.gods[2].atk_anim = (game.gods[2].atk_anim - 4.0 / 60.0).max(0.0);
                    if left == 18 {
                        vfx.puppets.on_cast(3, CastStyle::Rear);
                    }
                    if left == 12 {
                        vfx.puppets.on_cast(4, CastStyle::Arch);
                    }
                    // A dash a third of the way through, as systems.rs poses it.
                    let from = row + Vec3::X * 9.7;
                    let to = from + Vec3::X * 6.0;
                    let a = 0.3;
                    game.gods[5].dash = Some(DashState { from, to, t: a * 0.4, dur: 0.4, dmg: 0.0, arc: 0.35, blast: None });
                    game.gods[5].pos = from.lerp(to, a) + Vec3::Y * (a * (1.0 - a) * 4.0 * 0.35);
                }
                1 => {
                    // Walk one cat along a real root, toward the camera.
                    let (a, b) = root.unwrap();
                    let dir = (b - a).normalize_or_zero();
                    let g = &mut game.gods[0];
                    g.yaw = dir.z.atan2(dir.x);
                    g.pos = a - dir * 1.2 + dir * (step as f32 * (2.0 / 60.0));
                }
                k if shots[k].name == "fx_blast.png" => {
                    for gi in 1..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                    game.gods[0].pos = Vec3::new(-15.5, 0.0, 8.0);
                    game.gods[0].yaw = 1.9;
                    if left == 40 {
                        // an old break, then a downpour's worth of healing
                        vfx.ground.strike(Vec3::new(-13.0, 0.0, 3.5), 4.0, 1.0);
                        for _ in 0..36 {
                            vfx.ground.tick(0.25, 1.0, 1.0);
                        }
                    }
                    if left == 20 {
                        vfx.quake(Vec3::new(-10.5, 0.0, 12.0), 6.0, 1.0);
                    }
                }
                k if shots[k].name == "log_crossing.png" => {
                    let c = crate::logs::crossings()[0];
                    for gi in 1..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                    // one cat walking the spine
                    game.gods[0].yaw = c.along.z.atan2(c.along.x);
                    game.gods[0].pos = c.at - c.along * (left as f32 * (3.0 / 60.0));
                }
                k if shots[k].name == "tower_line.png" => {
                    let t = consts_tower();
                    for gi in 1..game.gods.len() {
                        game.gods[gi].respawn_t = 999.0;
                    }
                    game.gods[0].pos = t + Vec3::X * (crate::consts::TOWER_RANGE - 2.0);
                    game.tick(1.0 / 60.0); // the real sim decides who the tower aims at
                }
                k if shots[k].name == "rig_trot_strip.png" => {
                    for gi in 1..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                    let g = &mut game.gods[0];
                    g.yaw = 0.0;
                    g.pos = trot_at - Vec3::X * (left as f32 * (4.5 / 60.0));
                }
                k if shots[k].name.starts_with("map_") => {
                    for gi in 0..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                }
                k if k == wall_shot => {
                    let (a, b) = wall.unwrap();
                    let along = (b - a).normalize_or_zero();
                    let side = Vec3::new(-along.z, 0.0, along.x);
                    // On the ridge, as the sim holds a perched cat.
                    let g = &mut game.gods[0];
                    g.yaw = along.z.atan2(along.x);
                    g.pos = a + along * (1.0 + step as f32 * (4.0 / 60.0));
                    g.perch = Some(a.y);
                    g.perch_narrow = true;
                    // Knocked off 0.12 s ago: most of the way over, front leading.
                    let mut fall = crate::leap::Fall::new(a.y, 2.4);
                    fall.t = 0.12;
                    fall.vy = -3.6;
                    let g = &mut game.gods[1];
                    g.yaw = along.z.atan2(along.x);
                    g.pos = a + along * 7.5 + side * 2.2 + Vec3::Y * (-a.y * 0.0 - 0.25);
                    g.fall = Some(fall);
                    for gi in 2..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                }
                k if k >= sig_first => {
                    let count = if k == sig_first { 6 } else { 7 };
                    let mut casts = Vec::new();
                    for gi in 0..count {
                        let g = &mut game.gods[gi];
                        g.yaw = 0.0;
                        g.pos = file(gi);
                        // 0.2 s before the capture: mid-effect, not the first frame of it.
                        if left == 12 {
                            casts.push(crate::sim::Event::Cast { god: gi, slot: 3, aim: g.pos + Vec3::X * 7.0 });
                        }
                    }
                    for gi in count..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                    vfx.update(1.0 / 60.0, &casts, &game);
                    renderer.clear_queues();
                    scene.draw(&mut renderer, &game, &mut vfx, cam);
                    continue;
                }
                _ => {
                    // One real fern, two cats passing it on the capture frame.
                    // Cat 0 stalks by, a hand's width from the stem: low, and
                    // bent round it. Cat 1 runs through a unit the other side.
                    // Both are driven the way the sim drives them: `sneak`
                    // intent on one, speed on the other.
                    let f = fern.unwrap();
                    let yaw = 0.8f32;
                    let dir = Vec3::new(yaw.cos(), 0.0, yaw.sin());
                    let side = Vec3::new(-dir.z, 0.0, dir.x);
                    let g = &mut game.gods[0];
                    g.yaw = yaw;
                    g.sneak = true;
                    g.pos = f - side * 0.3 - dir * (left as f32 * (3.2 / 60.0) + 0.35);
                    let g = &mut game.gods[1];
                    g.yaw = yaw;
                    g.pos = f + side * 1.1 - dir * (left as f32 * (12.4 / 60.0) - 0.6);
                    for gi in 2..game.gods.len() {
                        game.gods[gi].pos = Vec3::new(0.0, 0.0, 150.0);
                    }
                }
            }
            vfx.update(1.0 / 60.0, &[], &game);
            renderer.clear_queues();
            scene.draw(&mut renderer, &game, &mut vfx, cam);
            if shot.name == "rig_trot_strip.png" && step >= STEPS - 16 && step % 3 == 0 {
                // follow the cat so every frame frames it
                let c = Camera { eye: game.gods[0].pos + Vec3::new(0.0, 1.1, -4.2), target: game.gods[0].pos + Vec3::Y * 0.45, fov_y: 0.6 };
                renderer.clear_queues();
                scene.draw(&mut renderer, &game, &mut vfx, c);
                renderer.atmosphere = game.weather.atmosphere();
                renderer.render_frame(&view, c, 1.0);
                let name = format!("rig_trot_strip_{step}.png");
                save_shot(&renderer.device, &renderer.queue, &target, &name)?;
                println!("saved {name}");
            }
        }
        // CLAWBOUND_NO_BRUSH=1 renders the same frame with the leaf-bend off,
        // so the two can be diffed: everything else in the frame is identical,
        // so any pixel that differs is a leaf that moved.
        if std::env::var("CLAWBOUND_NO_BRUSH").is_ok() {
            renderer.brushes.clear();
        }
        // CLAWBOUND_NO_FX=1: the same frame with no blast, cracks or shafts.
        if std::env::var("CLAWBOUND_NO_FX").is_ok() {
            renderer.blast_a = Default::default();
            renderer.blast_b = Default::default();
            renderer.cracks = Default::default();
            renderer.shaft_steps = 0;
        }
        println!("blasts: {:?}  cracks open: {:?}", renderer.blast_b.iter().map(|b| (b[0] * 100.0).round() / 100.0).collect::<Vec<_>>(), renderer.cracks.iter().filter(|c| c[3] > 0.0).map(|c| (c[3] * 100.0).round() / 100.0).collect::<Vec<_>>());
        println!("brushes this frame: {:?}", renderer.brushes.iter().map(|b| (b[3] * 100.0).round() / 100.0).collect::<Vec<_>>());
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, shot.name)?;
        println!("saved {}", shot.name);
    }
    Ok(())
}

/// The enemy (Red) mid outer tower, for the staged tower shot.
fn consts_tower() -> Vec3 {
    crate::consts::tower_pos(crate::consts::Lane::Mid, crate::consts::Team::Red, false)
}

/// The shop, three ways: desktop over the world, a landscape phone (844 x
/// 474 pt) over the world, and a portrait phone (390 x 845 pt) on its own.
/// `CLAWBOUND_SHOTS=shop clawbound-battleground --screenshot`.
pub fn run_shop_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mk = |r: &Renderer, w: u32, h: u32| {
        r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shop shot"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let target = mk(&renderer, W, H);
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    // A mid-game purse: some bought, some affordable, some not, one armed
    // for sale -- every state a card and a slot can be in.
    let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    game.blockers = scene.blockers.clone();
    game.gods[0].gold = 5000.0;
    let rec = crate::shopview::recommended(&game, 0);
    let first = crate::shopview::offers(&game, 0, rec[0]);
    let _ = game.buy_item(0, first[0].item.id);
    let _ = game.buy_item(0, first[3].item.id);
    game.gods[0].gold = 1650.0;
    let mut ui_state = UiState::new();
    ui_state.shop_open = true;
    ui_state.shop_tab = rec[0];
    let mut vfx = Vfx::default();
    for _ in 0..30 {
        game.tick(1.0 / 60.0);
    }
    let p = game.gods[0].pos;
    let cam = Camera { eye: p + Vec3::new(6.0, 7.0, -8.0), target: p + Vec3::Y * 1.2, fov_y: 0.95 };
    let egui_ctx = egui::Context::default();
    for (name, ppp) in [("clawbound_shop.png", 1.0f32), ("clawbound_shop_landscape_phone.png", W as f32 / 844.0)] {
        renderer.atmosphere = game.weather.atmosphere();
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.render_frame(&view, cam, 1.0);
        let out = shop_frame(&egui_ctx, &mut game, &mut ui_state, W, H, ppp);
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &egui_ctx, out, W, H);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    // The Curio shelf, one of them in the study pane.
    ui_state.shop_tab = crate::items::Cat::Curio;
    ui_state.shop_study = crate::items::all().iter().find(|i| i.name == "Thrice-Pounce Claws").map(|i| i.id);
    {
        renderer.atmosphere = game.weather.atmosphere();
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.render_frame(&view, cam, 1.0);
        let out = shop_frame(&egui_ctx, &mut game, &mut ui_state, W, H, 1.0);
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &egui_ctx, out, W, H);
        save_shot(&renderer.device, &renderer.queue, &target, "clawbound_shop_curios.png")?;
        println!("saved clawbound_shop_curios.png");
    }
    ui_state.shop_tab = rec[0];
    ui_state.shop_study = None;
    // Portrait phone: the shop alone on a cleared frame, on its Shelves
    // tab and then its Hoard tab.
    let (pw, ph) = (576u32, 1248u32); // width*4 must be a multiple of 256 for the readback
    let ptarget = mk(&renderer, pw, ph);
    let pview = ptarget.create_view(&Default::default());
    for (name, tab) in [("clawbound_shop_phone.png", crate::shopui::PhoneTab::Shelves), ("clawbound_shop_phone_hoard.png", crate::shopui::PhoneTab::Hoard)] {
        ui_state.shop_phone_tab = tab;
        ui_state.sell_armed = if tab == crate::shopui::PhoneTab::Hoard { Some(1) } else { None };
        clear(&renderer.device, &renderer.queue, &pview);
        let out = shop_frame(&egui_ctx, &mut game, &mut ui_state, pw, ph, pw as f32 / 390.0);
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &pview, &egui_ctx, out, pw, ph);
        save_shot(&renderer.device, &renderer.queue, &ptarget, name)?;
        println!("saved {name}");
    }
    Ok(())
}

/// Two egui passes (the first lays out, the second paints with hover state
/// settled), returning the second's output.
fn shop_frame(ctx: &egui::Context, game: &mut Game, st: &mut UiState, w: u32, h: u32, ppp: f32) -> egui::FullOutput {
    ctx.set_pixels_per_point(ppp);
    let mut textures = egui::TexturesDelta::default();
    let mut out = None;
    for k in 0..2 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / ppp, h as f32 / ppp))),
            time: Some(k as f64 * 0.016),
            ..Default::default()
        };
        let o = ctx.run(raw, |c| ui::draw_shop(c, game, 0, st));
        // Every pass's texture updates must reach the renderer, or the font
        // atlas the first pass created is patched before it exists.
        textures.append(o.textures_delta.clone());
        out = Some(o);
    }
    let mut out = out.expect("ran");
    out.textures_delta = textures;
    out
}

fn clear(device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let _p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.08, b: 0.05, a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
    queue.submit([enc.finish()]);
}

/// Twinfang Pass, looked at: the whole map from above, the gorge between
/// the peaks at cat height, a waterfall up close, a clearing, and the
/// 15v15 fight in fog. `CLAWBOUND_SHOTS=pass clawbound-battleground
/// --screenshot --map pass`. The match is real: bots on both sides, the
/// scene's own blockers, `steps` of sim before the fight shot.
pub fn run_pass_shots() -> Result<(), String> {
    if crate::map::active() != crate::map::MapId::Pass {
        return Err("run with --map pass (or CLAWBOUND_MAP=pass)".into());
    }
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pass shot"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    println!("total voxel instances uploaded: {}", scene.total_voxels);
    let mut rng = Rng::new(0xFA11);
    let (blue, red) = crate::map::team_picks(3, &mut rng);
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    game.gods[0].ai = Some({ let mut b = crate::sim::AiBrain::new(crate::consts::Lane::Mid, 0.9); b.buy_t = 1.0; b });
    let mut vfx = Vfx::default();
    let only: Vec<String> = std::env::var("CLAWBOUND_SHEET").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let want = |n: &str| only.is_empty() || only.iter().any(|o| n.contains(o.as_str()));
    let mut shoot = |renderer: &mut Renderer, game: &Game, vfx: &mut Vfx, name: &str, cam: Camera| -> Result<(), String> {
        if !want(name) {
            return Ok(());
        }
        renderer.clear_queues();
        vfx.update(1.0 / 60.0, &[], game);
        scene.draw(renderer, game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
        Ok(())
    };
    let fall = crate::pass::falls()[0];
    let clearing = crate::pass::clearings()[4];
    let cz = Vec3::new(clearing.0, 0.0, clearing.1);
    shoot(&mut renderer, &game, &mut vfx, "pass_overview.png", Camera { eye: Vec3::new(0.0, 470.0, -100.0), target: Vec3::ZERO, fov_y: 1.1 })?;
    shoot(&mut renderer, &game, &mut vfx, "pass_gorge.png", Camera { eye: Vec3::new(2.0, 3.0, -76.0), target: Vec3::new(-4.0, 22.0, 0.0), fov_y: 1.0 })?;
    // Cameras stand ON the floor (clamped in from the rock), or the first
    // try looked at the inside of a boulder and a cliff.
    let on_floor = |p: Vec3, h: f32| crate::map::onto_floor(p, 1.5) + Vec3::Y * h;
    // from down the stream, looking back up it at the fall
    shoot(&mut renderer, &game, &mut vfx, "pass_waterfall.png", Camera { eye: on_floor(crate::pass::streams()[0][3], 2.5), target: fall.foot + Vec3::Y * 5.0, fov_y: 0.75 })?;
    shoot(&mut renderer, &game, &mut vfx, "pass_clearing.png", Camera { eye: on_floor(cz + Vec3::new(-5.0, 0.0, -5.0), 7.0), target: cz, fov_y: 1.05 })?;
    // the new scenery, at real placements
    {
        use crate::passworld::Item;
        let items = crate::passworld::placements();
        let first = |f: &dyn Fn(&Item) -> Option<(Vec3, Vec3)>| items.iter().find_map(|i| f(i));
        if let Some((at, _)) = first(&|i| if let Item::Statue { at, owl: false, .. } = i { Some((*at, Vec3::Z)) } else { None }) {
            shoot(&mut renderer, &game, &mut vfx, "pass_statue.png", Camera { eye: at + Vec3::new(9.0, 5.0, 24.0), target: at + Vec3::Y * 8.0, fov_y: 0.9 })?;
        }
        if let Some((at, fwd)) = first(&|i| if let Item::Arch { at, yaw, .. } = i { Some((*at, Vec3::new(yaw.sin(), 0.0, yaw.cos()))) } else { None }) {
            shoot(&mut renderer, &game, &mut vfx, "pass_arch.png", Camera { eye: at - fwd * 22.0 + Vec3::Y * 4.0, target: at + Vec3::Y * 6.0, fov_y: 0.95 })?;
        }
        if let Some((at, _)) = first(&|i| if let Item::Gem { at, .. } = i { Some((*at, Vec3::Z)) } else { None }) {
            // from out on the floor, a few metres off (the first try stood
            // the camera inside a crystal)
            let f = crate::pass::floor();
            let e = 0.5;
            let away = Vec3::new(f.sdf(at.x - e, at.z) - f.sdf(at.x + e, at.z), 0.0, f.sdf(at.x, at.z - e) - f.sdf(at.x, at.z + e)).normalize_or_zero();
            let out = crate::map::onto_floor(at + away * 7.0, 1.0);
            shoot(&mut renderer, &game, &mut vfx, "pass_gems.png", Camera { eye: out + Vec3::Y * 2.2, target: at + Vec3::Y * 1.0, fov_y: 0.9 })?;
        }
        if let Some((at, _)) = first(&|i| if let Item::Statue { at, owl: true, .. } = i { Some((*at, Vec3::Z)) } else { None }) {
            let look_from = Vec3::new(at.x * 0.82, 4.0, 0.0);
            shoot(&mut renderer, &game, &mut vfx, "pass_owl.png", Camera { eye: look_from, target: at + Vec3::Y * 7.0, fov_y: 0.95 })?;
        }
    }
    // a cave: from outside its mouth, then from the gameplay camera with the
    // player's cat inside it (the roof ghosts)
    if let Some(cave) = crate::pass::caves().into_iter().next() {
        let d = (cave.b - cave.a).normalize_or_zero();
        // on the ledge itself, a few units short of the mouth (the first try
        // stood in the clearing, inside a pine)
        // from the far end, looking back in (the near end has a spring
        // tree by it, and the camera kept standing in it)
        shoot(&mut renderer, &game, &mut vfx, "pass_cave.png", Camera { eye: cave.b + d * 5.0 + Vec3::Y * 2.2, target: cave.b - d * 10.0 + Vec3::Y * 2.5, fov_y: 1.0 })?;
        let keep = game.gods[0].pos;
        game.gods[0].pos = cave.mid();
        let t = cave.mid() + Vec3::Y * 1.5;
        let back = Vec3::new(-d.x, 0.0, -d.z);
        shoot(&mut renderer, &game, &mut vfx, "pass_cave_inside.png", Camera { eye: t + back * 12.0 + Vec3::Y * 6.0, target: t, fov_y: 0.95 })?;
        game.gods[0].pos = keep;
    }
    // the climb: from the foot of Top's up-ramp, looking up onto the shelf;
    // and from the shelf, looking back down
    {
        let top = crate::consts::Lane::Top.polyline();
        let len: f32 = top.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let at = |f: f32| {
            let p = crate::pass::along(&top, f * len);
            Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z)
        };
        shoot(&mut renderer, &game, &mut vfx, "pass_climb.png", Camera { eye: at(0.33) + Vec3::Y * 4.0, target: at(0.47) + Vec3::Y * 2.0, fov_y: 0.95 })?;
        shoot(&mut renderer, &game, &mut vfx, "pass_shelf.png", Camera { eye: at(0.5) + Vec3::Y * 5.0, target: at(0.64) + Vec3::Y * 0.0, fov_y: 0.95 })?;
    }
    // camps, gaps and niches
    if let Some(c) = crate::pass::camps().into_iter().next() {
        let mouth = c.at - c.back * (c.r + 3.0);
        shoot(&mut renderer, &game, &mut vfx, "pass_camp.png", Camera { eye: crate::map::onto_floor(mouth, 1.0) + Vec3::Y * 4.5, target: c.at + c.back * 3.0, fov_y: 1.0 })?;
    }
    if let Some(g) = crate::pass::caps().iter().find(|c| !c.niche) {
        let d = (g.b - g.a).normalize_or_zero();
        shoot(&mut renderer, &game, &mut vfx, "pass_gap.png", Camera { eye: g.a - d * 5.0 + Vec3::Y * 2.0, target: g.b + d * 8.0 + Vec3::Y * 1.5, fov_y: 0.95 })?;
    }
    if let Some(n) = crate::pass::caps().iter().find(|c| c.niche) {
        let out = (n.a - n.b).normalize_or_zero();
        shoot(&mut renderer, &game, &mut vfx, "pass_niche.png", Camera { eye: n.a + out * 3.2 + Vec3::Y * 1.9, target: n.b + Vec3::Y * 1.3, fov_y: 0.9 })?;
    }
    // a choke, from the wide lane looking into the narrows
    let (cp, cd) = crate::pass::choke_points()[2];
    shoot(&mut renderer, &game, &mut vfx, "pass_choke.png", Camera { eye: cp - cd * 22.0 + Vec3::Y * 6.0, target: cp + cd * 6.0 + Vec3::Y * 2.0, fov_y: 0.95 })?;
    shoot(&mut renderer, &game, &mut vfx, "pass_peaks.png", Camera { eye: Vec3::new(0.0, 4.0, -118.0), target: Vec3::new(0.0, 40.0, 0.0), fov_y: 1.0 })?;
    // The fight: run the match, then frame the densest knot of cats.
    let steps: usize = std::env::var("CLAWBOUND_PASS_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(75) * 60;
    let t0 = std::time::Instant::now();
    for _ in 0..steps {
        game.tick(1.0 / 60.0);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(1.0 / 60.0, &ev, &game);
    }
    println!("sim: {} s of 15v15 in {:.2} s wall ({:.2} ms/tick)", steps / 60, t0.elapsed().as_secs_f32(), t0.elapsed().as_secs_f32() * 1000.0 / steps as f32);
    let alive: Vec<Vec3> = game.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| g.pos).collect();
    let knot = alive.iter().copied().max_by_key(|p| alive.iter().filter(|q| (**q - *p).length() < 14.0).count()).unwrap_or(Vec3::ZERO);
    game.weather.force(crate::weather::WeatherKind::Fog, game.t + 999.0);
    for _ in 0..(10 * 60) {
        game.tick(1.0 / 60.0);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(1.0 / 60.0, &ev, &game);
    }
    shoot(&mut renderer, &game, &mut vfx, "pass_fight_fog.png", Camera { eye: knot + Vec3::new(-10.0, 9.0, -12.0), target: knot + Vec3::Y, fov_y: 1.0 })?;
    // The same fight by day, under plain overcast: a map event (a Long
    // Night) had darkened the first one, which hid the cats.
    game.weather.night = 0.0;
    game.weather.night_target = 0.0;
    game.weather.force(crate::weather::WeatherKind::Overcast, game.t + 999.0);
    for _ in 0..(3 * 60) {
        game.tick(1.0 / 60.0);
        game.weather.night = 0.0;
        game.weather.night_target = 0.0;
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(1.0 / 60.0, &ev, &game);
    }
    let alive: Vec<Vec3> = game.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| g.pos).collect();
    let knot = alive.iter().copied().max_by_key(|p| alive.iter().filter(|q| (**q - *p).length() < 14.0).count()).unwrap_or(Vec3::ZERO);
    let cam = Camera { eye: knot + Vec3::new(-11.0, 8.0, -13.0), target: knot + Vec3::Y, fov_y: 1.0 };
    shoot(&mut renderer, &game, &mut vfx, "pass_fight_day.png", cam)?;
    // HUD and scoreboard with 30 cats, over that frame.
    if want("pass_hud") {
        let mut egui_renderer = egui_wgpu::Renderer::new(&renderer.device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
        let ctx = egui::Context::default();
        let mut st = UiState::new();
        renderer.clear_queues();
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        let vp = cam.view_proj(W as f32 / H as f32);
        let mut textures = egui::TexturesDelta::default();
        let mut out = None;
        for k in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
                time: Some(k as f64 * 0.016),
                ..Default::default()
            };
            let o = ctx.run(raw, |c| {
                ui::draw_hud(c, &game, 0, vp, W as f32, H as f32, &mut st, renderer.camera.eye);
                ui::draw_scoreboard(c, &game);
            });
            textures.append(o.textures_delta.clone());
            out = Some(o);
        }
        let mut out = out.expect("ran");
        out.textures_delta = textures;
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &ctx, out, W, H);
        save_shot(&renderer.device, &renderer.queue, &target, "pass_hud_scoreboard.png")?;
        println!("saved pass_hud_scoreboard.png");
    }
    Ok(())
}

/// The match intro, played through the real draw path: the timeline
/// (`intro`) drives the camera, fires the starry bursts as cats arrive and
/// feeds the overlay. Captures a frame from each beat.
/// `CLAWBOUND_SHOTS=intro clawbound-battleground --screenshot [--map pass]`.
pub fn run_intro_shots() -> Result<(), String> {
    use crate::intro::{self, Intro};
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("intro shot"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let mut rng = Rng::new(0x1A7);
    let (blue, red) = crate::map::team_picks(3, &mut rng);
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    for g in game.gods.iter_mut() {
        g.respawn_t = 999.0; // not arrived
    }
    let teams: Vec<Team> = game.gods.iter().map(|g| g.team).collect();
    let p0 = game.gods[0].pos;
    let play = intro::Shot { eye: p0 + Vec3::new(0.0, 6.5, -9.0), target: p0 + Vec3::Y * 1.5, fov_y: 0.95 };
    // CLAWBOUND_INTRO_FULL=1: the narrated first-match intro
    let full = std::env::var("CLAWBOUND_INTRO_FULL").is_ok();
    let mut it = if full { Intro::full(&teams, p0, play) } else { Intro::new(&teams, p0, play) };
    let lead = if full { intro::PROLOGUE } else { 0.0 };
    let mut vfx = Vfx::default();
    let ctx = egui::Context::default();
    let dt = 1.0 / 60.0;
    let mut caps: Vec<(f32, &str)> = Vec::new();
    if full {
        caps.extend([(3.0, "intro_0a_prologue.png"), (17.0, "intro_0b_prologue.png"), (30.0, "intro_0c_prologue.png"), (42.5, "intro_0d_prologue.png")]);
    }
    let short = [
        (2.2, "intro_1_pan.png"),
        (intro::PAN + 0.9, "intro_2_lane_top.png"),
        (intro::PAN + intro::LANE + 0.9, "intro_3_lane_mid.png"),
        (intro::FANFARE_AT + 0.75, "intro_4_spawn_blue.png"),
        (intro::FANFARE_AT + intro::SPAWN + 0.75, "intro_5_spawn_red.png"),
        (intro::FANFARE_AT + 2.0 * intro::SPAWN + intro::VERSUS * 0.5, "intro_6_versus.png"),
        (intro::total() - 0.35, "intro_7_settle.png"),
    ];
    caps.extend(short.iter().map(|(t, n)| (t + lead, *n)));
    let mut next = 0;
    while !it.done && next < caps.len() {
        for gi in it.advance(dt) {
            game.gods[gi].respawn_t = 0.0;
            let c = game.gods[gi].team.rgb();
            vfx.star_burst(game.gods[gi].pos, c, gi as f32);
        }
        vfx.update(dt, &[], &game);
        if it.t >= caps[next].0 {
            let f = it.frame();
            let cam = Camera { eye: f.shot.eye, target: f.shot.target, fov_y: f.shot.fov_y };
            renderer.clear_queues();
            scene.draw(&mut renderer, &game, &mut vfx, cam);
            renderer.atmosphere = game.weather.atmosphere();
            renderer.render_frame(&view, cam, it.t);
            let o = f.overlay;
            let mut textures = egui::TexturesDelta::default();
            let mut out = None;
            for k in 0..2 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
                    time: Some(k as f64 * 0.016),
                    ..Default::default()
                };
                let r = ctx.run(raw, |c| ui::draw_intro_overlay(c, &o));
                textures.append(r.textures_delta.clone());
                out = Some(r);
            }
            let mut out = out.expect("ran");
            out.textures_delta = textures;
            draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &ctx, out, W, H);
            save_shot(&renderer.device, &renderer.queue, &target, caps[next].1)?;
            println!("saved {} (t={:.2}, {:?})", caps[next].1, it.t, f.beat);
            next += 1;
        }
    }
    Ok(())
}

/// Profiling spot-check: the active map, a gameplay camera (the app's
/// distance and pitch) at representative places, and per place the CPU
/// time to build the frame (`Scene::draw`), the time to render it to GPU
/// completion, the draws and voxel instances drawn, and how many static
/// chunks survived culling. Plus two high views. lavapipe renders on the
/// CPU, so the absolute times are not a phone's; the ratios between maps
/// and places, and the counts, are the point.
/// `CLAWBOUND_SHOTS=perf clawbound-battleground --screenshot [--map pass]`
pub fn run_perf() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("perf"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let t0 = std::time::Instant::now();
    let scene = Scene::new(&mut renderer);
    let build = t0.elapsed().as_secs_f32();
    let mut rng = Rng::new(0x9E7F);
    let (blue, red) = crate::map::team_picks(3, &mut rng);
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    // a minute of play so cats, minions and effects are out on the map
    for _ in 0..(60 * 60) {
        game.tick(1.0 / 60.0);
        game.events.clear();
    }
    let mut vfx = Vfx::default();
    let total_chunks = scene.chunks.len();
    let total_inst: u64 = scene.total_voxels as u64;
    println!("map {:?}: {} cats, world build {:.1} s, {} static chunks, {} voxel instances uploaded", crate::map::active(), game.gods.len(), build, total_chunks, total_inst);
    // the app's gameplay camera: yaw pi, pitch 0.38, its default distance
    let play = |at: Vec3, yaw: f32| {
        let pitch = 0.38f32;
        let dir = Vec3::new(yaw.cos() * pitch.cos(), -pitch.sin(), yaw.sin() * pitch.cos());
        let target = at + Vec3::Y * 1.5;
        Camera { eye: target - dir * 14.0, target, fov_y: 0.95 }
    };
    let lane_mid = |l: crate::consts::Lane, f: f32| {
        let p = l.polyline();
        let len: f32 = p.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        crate::pass::along(&p, len * f)
    };
    let base = Team::Blue.base_pos();
    let spots: Vec<(&str, Camera)> = vec![
        ("blue base", play(base + Vec3::new(0.0, 0.0, 12.0), std::f32::consts::FRAC_PI_2)),
        ("mid lane 1/3", play(lane_mid(crate::consts::Lane::Mid, 0.33), std::f32::consts::FRAC_PI_2)),
        ("top lane 1/2", play(lane_mid(crate::consts::Lane::Top, 0.5), std::f32::consts::FRAC_PI_2)),
        ("bot lane 1/4", play(lane_mid(crate::consts::Lane::Bot, 0.25), std::f32::consts::FRAC_PI_2)),
        ("centre", play(Vec3::ZERO, std::f32::consts::FRAC_PI_2)),
        ("high view", Camera { eye: Vec3::new(0.0, 120.0, -crate::consts::arena_radius()), target: Vec3::ZERO, fov_y: 1.0 }),
        ("overhead", Camera { eye: Vec3::new(0.0, 380.0, -60.0), target: Vec3::ZERO, fov_y: 1.1 }),
    ];
    println!("{:<14} {:>9} {:>9} {:>6} {:>10} {:>10} {:>8}", "place", "build ms", "frame ms", "draws", "instances", "shadow", "chunks");
    for (name, cam) in spots {
        let mut best = (f32::MAX, f32::MAX);
        let mut chunks_drawn = 0;
        for k in 0..6 {
            vfx.update(1.0 / 60.0, &[], &game);
            let a = std::time::Instant::now();
            scene.draw(&mut renderer, &game, &mut vfx, cam);
            let b = a.elapsed().as_secs_f32() * 1000.0;
            renderer.atmosphere = game.weather.atmosphere();
            let c = std::time::Instant::now();
            renderer.render_frame(&view, cam, game.t);
            renderer.device.poll(wgpu::Maintain::Wait);
            let d = c.elapsed().as_secs_f32() * 1000.0;
            if k >= 2 {
                best = (best.0.min(b), best.1.min(d));
            }
            let vp = cam.view_proj(W as f32 / H as f32);
            chunks_drawn = scene.chunks.iter().filter(|ch| crate::render::sphere_in_frustum(&vp, ch.center, ch.radius)).count();
        }
        let st = renderer.stats;
        println!("{:<14} {:>9.2} {:>9.1} {:>6} {:>10} {:>10} {:>4}/{:<4}", name, best.0, best.1, st.draws, st.instances, st.shadow_instances, chunks_drawn, total_chunks);
        if std::env::var("CLAWBOUND_PERF_SHOTS").is_ok() {
            save_shot(&renderer.device, &renderer.queue, &target, &format!("perf_{:?}_{}.png", crate::map::active(), name.replace(' ', "_").replace('/', "-")))?;
        }
    }
    Ok(())
}

/// The map switch, end to end through the real renderer: jungle, switch to
/// the pass, switch back, the same camera each time. Checks the two maps
/// render differently, the resident mesh count returns exactly to the
/// jungle's (no leak), and the jungle renders the same before and after
/// (a deterministic rebuild). Prints a verdict; Err if any check fails.
/// `CLAWBOUND_SHOTS=switch clawbound-battleground --screenshot`
pub fn run_switch_check() -> Result<(), String> {
    use crate::map::MapId;
    crate::map::set_active(MapId::Jungle);
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("switch"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let cam = Camera { eye: Vec3::new(0.0, 60.0, -150.0), target: Vec3::new(0.0, 0.0, -60.0), fov_y: 1.0 };
    let mut scene: Option<Scene> = None;
    let mut shots: Vec<(MapId, usize, Vec<u8>)> = Vec::new();
    for (k, m) in [MapId::Jungle, MapId::Pass, MapId::Jungle].into_iter().enumerate() {
        let t = std::time::Instant::now();
        let sc = crate::scene::switch_world(&mut renderer, scene.take(), m);
        let built = t.elapsed().as_secs_f32();
        let game = Game::new_teams(&[0, 1, 2, 3, 4], &[5, 6, 7, 8, 9]);
        let mut vfx = Vfx::default();
        vfx.update(1.0 / 60.0, &[], &game);
        renderer.clear_queues();
        sc.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, 1.0);
        let name = format!("switch_{k}_{}.png", m.key());
        save_shot(&renderer.device, &renderer.queue, &target, &name)?;
        let px = std::fs::read(&name).map_err(|e| e.to_string())?;
        if k == 0 {
            // control: the same map, same camera, rendered again with NO
            // switch -- what frame-to-frame variation looks like on its own
            let game2 = Game::new_teams(&[0, 1, 2, 3, 4], &[5, 6, 7, 8, 9]);
            let mut vfx2 = Vfx::default();
            vfx2.update(1.0 / 60.0, &[], &game2);
            renderer.clear_queues();
            sc.draw(&mut renderer, &game2, &mut vfx2, cam);
            renderer.atmosphere = game2.weather.atmosphere();
            renderer.render_frame(&view, cam, 1.0);
            save_shot(&renderer.device, &renderer.queue, &target, "switch_0_control.png")?;
        }
        println!("{name}: {} built in {built:.1} s, {} meshes resident, world geometry for {:?}", m.name(), renderer.mesh_count(), renderer.world_map);
        shots.push((m, renderer.mesh_count(), px));
        scene = Some(sc);
    }
    let differ = shots[0].2 != shots[1].2;
    let no_leak = shots[0].1 == shots[2].1;
    // Pixel-identical is too strict: the control (the same frame again, no
    // switch) is not identical either. The round trip must differ from the
    // first frame by no more than the control does, give or take.
    let decode = |path: &str| -> Result<Vec<u8>, String> {
        let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
        Ok(img.into_raw())
    };
    let first = decode("switch_0_jungle.png")?;
    let control = decode("switch_0_control.png")?;
    let back = decode("switch_2_jungle.png")?;
    let differing = |a: &[u8], b: &[u8]| a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 8)).count();
    let (noise, trip) = (differing(&first, &control), differing(&first, &back));
    println!("pixels differing by > 8: control (no switch) {noise}, round trip {trip}");
    let same_back = trip <= noise * 2 + 50;
    println!("maps render differently: {differ}; mesh count back to the jungle's ({} -> {} -> {}): {no_leak}; jungle identical after the round trip: {same_back}", shots[0].1, shots[1].1, shots[2].1);
    if differ && no_leak && same_back {
        Ok(())
    } else {
        Err("map switch check failed".into())
    }
}

/// The select screen, with the map picker, on a cleared frame.
pub fn run_select_shot() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("select"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let ctx = egui::Context::default();
    let mut st = UiState::new();
    st.map_choice = crate::map::MapId::Sky;
    st.selected = crate::gods::ROSTER_LEN - 1; // the newest ancestor
    clear(&renderer.device, &renderer.queue, &view);
    let mut textures = egui::TexturesDelta::default();
    let mut out = None;
    let mut locked = false;
    for k in 0..2 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
            time: Some(k as f64 * 0.016),
            ..Default::default()
        };
        let o = ctx.run(raw, |c| ui::draw_select(c, &mut st, &mut locked));
        textures.append(o.textures_delta.clone());
        out = Some(o);
    }
    let mut out = out.expect("ran");
    out.textures_delta = textures;
    draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &ctx, out, W, H);
    save_shot(&renderer.device, &renderer.queue, &target, "select_map_picker.png")?;
    println!("saved select_map_picker.png");
    Ok(())
}

/// Targeting and the cat's new body language, through the real draw path:
/// each of a cat's four abilities prepared with its guide showing, then the
/// shoulder shuffle (the attack button with nothing in range) frame by
/// frame, and a standing cat for the bent-knee stance.
/// `CLAWBOUND_SHOTS=aim clawbound-battleground --screenshot`
pub fn run_aim_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("aim"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let god: u32 = std::env::var("CLAWBOUND_AIM_GOD").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut game = Game::new_teams(&[god], &[(god + 1) % 13]);
    game.blockers = scene.blockers.clone();
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    let at = Vec3::new(0.0, 0.0, -36.0);
    game.gods[0].pos = at;
    game.gods[0].yaw = std::f32::consts::FRAC_PI_2; // facing +z
    let mut vfx = Vfx::default();
    for _ in 0..30 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    let cam = Camera { eye: at + Vec3::new(0.0, 9.0, -11.0), target: at + Vec3::new(0.0, 0.0, 6.0), fov_y: 1.0 };
    let aim = at + Vec3::new(4.0, 0.0, 12.0);
    let mut shoot = |renderer: &mut Renderer, vfx: &mut Vfx, game: &Game, name: &str, guide: Option<u32>, cam: Camera| -> Result<(), String> {
        renderer.clear_queues();
        scene.draw(renderer, game, vfx, cam);
        if let Some(slot) = guide {
            let ab = game.def(0).abilities[slot as usize];
            let g = crate::targeting::guide_for(&ab, game.gods[0].pos, aim);
            scene.draw_guide(renderer, &crate::targeting::guide_points(&g, 1.1), ab.color);
        }
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
        Ok(())
    };
    for slot in 0..4u32 {
        let name = format!("aim_slot{slot}_{}.png", game.def(0).abilities[slot as usize].name.replace(' ', "_"));
        shoot(&mut renderer, &mut vfx, &game, &name, Some(slot), cam)?;
    }
    // the shoulder shuffle, close
    let close = Camera { eye: at + Vec3::new(3.2, 1.6, 3.4), target: at + Vec3::Y * 0.9, fov_y: 0.8 };
    shoot(&mut renderer, &mut vfx, &game, "whiff_0_stand.png", None, close)?;
    vfx.puppets.on_whiff(0);
    for (k, steps) in [(1, 8), (2, 8), (3, 8)] {
        for _ in 0..steps {
            game.tick(1.0 / 60.0);
            game.events.clear();
            vfx.update(1.0 / 60.0, &[], &game);
        }
        shoot(&mut renderer, &mut vfx, &game, &format!("whiff_{k}.png"), None, close)?;
    }
    Ok(())
}

/// The spectator on an all-bot match through the real draw path: 20 s of
/// the director (reporting how smooth the camera was and how often it
/// changed fights), then frames of the director, a followed cat and the
/// free camera, with the spectator overlay.
/// `CLAWBOUND_SHOTS=spectate clawbound-battleground --screenshot [--map pass]`
pub fn run_spectate() -> Result<(), String> {
    use crate::spectator::{Input, Spectator};
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("spectate"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let (blue, red) = crate::map::team_picks(4, &mut Rng::new(0x5EC7));
    let mut game = Game::new_teams(&blue, &red);
    game.blockers = scene.blockers.clone();
    game.gods[0].ai = Some({ let mut b = crate::sim::AiBrain::new(crate::consts::Lane::Mid, 0.8); b.buy_t = 1.0; b });
    game.open_playbook(&mut Rng::new(0x9A6E));
    let mut vfx = Vfx::default();
    for _ in 0..(70 * 60) {
        game.tick(1.0 / 60.0);
        game.events.clear();
    }
    let mut sp = Spectator::new();
    let dt = 1.0 / 60.0;
    let (mut worst_turn, mut worst_step, mut switches) = (0.0f32, 0.0f32, 0);
    let mut prev: Option<crate::intro::Shot> = None;
    let mut focus = usize::MAX;
    for k in 0..(20 * 60) {
        game.tick(dt);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(dt, &ev, &game);
        let cuts_before = sp.cuts;
        let shot = sp.update(&game, if k == 0 { Input { director: true, ..Default::default() } } else { Input::default() }, dt);
        // a deliberate cut is not a jerk: measure the moves between cuts
        if sp.cuts != cuts_before {
            prev = None;
        }
        if let Some(p) = prev {
            worst_turn = worst_turn.max((p.target - p.eye).normalize().dot((shot.target - shot.eye).normalize()).clamp(-1.0, 1.0).acos().to_degrees());
            worst_step = worst_step.max((shot.eye - p.eye).length());
        }
        if sp.focus != focus {
            switches += 1;
            focus = sp.focus;
        }
        prev = Some(shot);
    }
    println!("director, 20 s: {} focus changes, {} cuts; between cuts: worst view turn {:.2} deg/frame, worst eye travel {:.2} u/frame", switches - 1, sp.cuts, worst_turn, worst_step);
    let ctx = egui::Context::default();
    let mut shoot = |renderer: &mut Renderer, vfx: &mut Vfx, game: &Game, sp: &Spectator, shot: crate::intro::Shot, name: &str| -> Result<(), String> {
        let cam = Camera { eye: shot.eye, target: shot.target, fov_y: shot.fov_y };
        renderer.clear_queues();
        scene.draw(renderer, game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        let vp = cam.view_proj(W as f32 / H as f32);
        let label = sp.label(game);
        let mut textures = egui::TexturesDelta::default();
        let mut out = None;
        for k in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
                time: Some(k as f64 * 0.016),
                ..Default::default()
            };
            let o = ctx.run(raw, |c| ui::draw_spectator_hud(c, game, vp, &label, cam.eye));
            textures.append(o.textures_delta.clone());
            out = Some(o);
        }
        let mut out = out.expect("ran");
        out.textures_delta = textures;
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &ctx, out, W, H);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name} ({})", label);
        Ok(())
    };
    let shot = sp.update(&game, Input::default(), dt);
    shoot(&mut renderer, &mut vfx, &game, &sp, shot, "spectate_director.png")?;
    sp.update(&game, Input { cycle: 3, ..Default::default() }, dt);
    let mut shot = shot;
    for _ in 0..150 {
        game.tick(dt);
        game.events.clear();
        vfx.update(dt, &[], &game);
        shot = sp.update(&game, Input::default(), dt);
    }
    shoot(&mut renderer, &mut vfx, &game, &sp, shot, "spectate_follow.png")?;
    sp.update(&game, Input { free: true, ..Default::default() }, dt);
    for _ in 0..90 {
        shot = sp.update(&game, Input { rise: 1.0, look: glam::Vec2::new(0.0, -0.004), ..Default::default() }, dt);
    }
    shoot(&mut renderer, &mut vfx, &game, &sp, shot, "spectate_free.png")?;
    Ok(())
}

/// A blast front behind a cat, rendered with and without it, so the
/// difference shows exactly which pixels the shell bends: a dome of air
/// round the blast, and the cat standing in front of it untouched.
/// `CLAWBOUND_SHOTS=blast clawbound-battleground --screenshot`
pub fn run_blast_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blast"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let mut game = Game::new_teams(&[0], &[1]);
    game.blockers = scene.blockers.clone();
    game.gods[1].respawn_t = 999.0;
    let cat = Vec3::new(0.0, 0.0, -40.0);
    game.gods[0].pos = cat;
    game.gods[0].yaw = -std::f32::consts::FRAC_PI_2;
    let mut vfx = Vfx::default();
    for _ in 0..20 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    let cam = Camera { eye: cat + Vec3::new(0.0, 3.2, -9.0), target: cat + Vec3::new(0.0, 2.0, 12.0), fov_y: 0.95 };
    for (name, strength) in [("blast_off.png", 0.0f32), ("blast_on.png", 1.0)] {
        renderer.clear_queues();
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        // a nova 11 u behind the cat, its front 7 u out
        renderer.blast_a = Default::default();
        renderer.blast_b = Default::default();
        renderer.blast_a[0] = [cat.x, 0.0, cat.z + 11.0, 7.0];
        renderer.blast_b[0] = [strength, 0.0, 0.0, 0.0];
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    Ok(())
}

/// Sproutkit and his gifts: a close look at him, the litter running, three
/// enemies giggling, and the dance party with its glitter-ball.
/// `CLAWBOUND_SHOTS=sprout clawbound-battleground --screenshot`
pub fn run_sprout_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sprout"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let kit = crate::gods::ROSTER.iter().position(|d| matches!(d.pelt, crate::gods::PeltKind::Kit)).unwrap() as u32;
    let mut game = Game::new_teams(&[kit], &[0, 6, 7]);
    game.blockers = scene.blockers.clone();
    game.next_wave_t = 1e9;
    let at = Vec3::new(0.0, 0.0, -40.0);
    game.gods[0].pos = at;
    game.gods[0].yaw = std::f32::consts::FRAC_PI_2;
    game.gods[0].mana = 9999.0;
    let foes = [Vec3::new(-2.5, 0.0, 7.0), Vec3::new(2.5, 0.0, 8.0), Vec3::new(0.0, 0.0, 10.0)];
    let place = |g: &mut Game| {
        for (k, f) in foes.iter().enumerate() {
            g.gods[k + 1].pos = at + *f;
            g.gods[k + 1].yaw = -std::f32::consts::FRAC_PI_2;
            g.gods[k + 1].ai = None;
            g.gods[k + 1].hp = g.gods[k + 1].max_hp;
        }
        g.gods[0].ai = None;
    };
    place(&mut game);
    let mut vfx = Vfx::default();
    let step = |game: &mut Game, vfx: &mut Vfx, secs: f32| {
        for _ in 0..(secs * 60.0) as usize {
            game.tick(1.0 / 60.0);
            let ev = std::mem::take(&mut game.events);
            vfx.update(1.0 / 60.0, &ev, game);
        }
    };
    let mut shoot = |renderer: &mut Renderer, game: &Game, vfx: &mut Vfx, name: &str, cam: Camera| -> Result<(), String> {
        renderer.clear_queues();
        scene.draw(renderer, game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
        Ok(())
    };
    step(&mut game, &mut vfx, 0.3);
    shoot(&mut renderer, &game, &mut vfx, "sprout_close.png", Camera { eye: at + Vec3::new(2.4, 1.6, 3.0), target: at + Vec3::Y * 1.2, fov_y: 0.8 })?;
    let wide = Camera { eye: at + Vec3::new(-9.0, 7.0, -6.0), target: at + Vec3::new(0.0, 1.0, 7.0), fov_y: 1.0 };
    // the litter, running
    for k in 1..4 {
        game.gods[k].pos = at + foes[k - 1] * 1.4;
    }
    game.cast_queue.push((0, 0, at + Vec3::new(0.0, 0.0, 8.0)));
    step(&mut game, &mut vfx, 0.4);
    shoot(&mut renderer, &game, &mut vfx, "sprout_litter.png", wide)?;
    step(&mut game, &mut vfx, 1.5);
    // the giggles
    place(&mut game);
    for k in 1..4 {
        game.gods[k].pos = at + foes[k - 1] * 0.7;
    }
    game.gods[0].cds = [0.0; 4];
    game.cast_queue.push((0, 1, at + Vec3::Z * 4.0));
    step(&mut game, &mut vfx, 0.55);
    shoot(&mut renderer, &game, &mut vfx, "sprout_giggle.png", wide)?;
    step(&mut game, &mut vfx, 2.0);
    // the dance party
    place(&mut game);
    game.gods[0].cds = [0.0; 4];
    game.cast_queue.push((0, 3, at + Vec3::new(0.0, 0.0, 8.5)));
    step(&mut game, &mut vfx, 1.6);
    shoot(&mut renderer, &game, &mut vfx, "sprout_party.png", Camera { eye: at + Vec3::new(-11.0, 9.0, -4.0), target: at + Vec3::new(0.0, 3.5, 8.5), fov_y: 1.0 })?;
    Ok(())
}

/// The ultra preview beside today's look: four ancestors (two maned) on
/// one frame drawn as voxels, then smooth and furred, then under a coloured
/// ability light with a cat between it and another (the traced shadow), and
/// a close-up of the manes.
/// `CLAWBOUND_SHOTS=ultra clawbound-battleground --screenshot`
pub fn run_ultra_shots() -> Result<(), String> {
    std::env::set_var("CLAWBOUND_QUALITY", "ultra");
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ultra"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut scene = Scene::new(&mut renderer);
    let id = |n: &str| crate::gods::ROSTER.iter().position(|d| d.name == n).unwrap() as u32;
    let mut game = Game::new_teams(&[id("Goldenmane"), id("Duskstripe")], &[id("Oldroar"), id("Faryowl")]);
    game.blockers = scene.blockers.clone();
    let spot = Vec3::new(0.0, 0.0, -40.0);
    for (k, g) in game.gods.iter_mut().enumerate() {
        g.ai = None;
        g.pos = spot + Vec3::new(k as f32 * 2.6 - 3.9, 0.0, 0.0);
        g.yaw = -std::f32::consts::FRAC_PI_2 + 0.5; // three-quarters to camera
    }
    let mut vfx = Vfx::default();
    for _ in 0..30 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    let wide = Camera { eye: spot + Vec3::new(0.5, 2.6, -7.5), target: spot + Vec3::Y * 0.8, fov_y: 0.8 };
    let close = Camera { eye: game.gods[0].pos + Vec3::new(1.2, 1.5, -2.6), target: game.gods[0].pos + Vec3::new(0.3, 1.1, 0.0), fov_y: 0.7 };
    let mut shoot = |renderer: &mut Renderer, scene: &Scene, vfx: &mut Vfx, name: &str, cam: Camera| -> Result<(), String> {
        scene.draw(renderer, &game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name} ({} coloured lights, {} occluders)", renderer.lights.len(), renderer.occluders.len());
        Ok(())
    };
    scene.ultra = false;
    shoot(&mut renderer, &scene, &mut vfx, "ultra_0_voxel.png", wide)?;
    scene.ultra = true;
    shoot(&mut renderer, &scene, &mut vfx, "ultra_1_smooth.png", wide)?;
    // a violet ability light beyond the right-hand cat, a little low: the
    // cat between it and the next one casts a traced shadow onto it
    let right = game.gods[3].pos;
    vfx.glows.push(crate::vfx::Glow { pos: right + Vec3::new(1.6, 0.9, 0.4), color: [0.7, 0.35, 1.0], radius: 9.0, life: 1.0, max: 1.0, power: 9.0 });
    vfx.glows.push(crate::vfx::Glow { pos: game.gods[0].pos + Vec3::new(-1.5, 1.4, -1.0), color: [1.0, 0.55, 0.15], radius: 7.0, life: 1.0, max: 1.0, power: 5.0 });
    shoot(&mut renderer, &scene, &mut vfx, "ultra_2_lit.png", wide)?;
    // the traced shadow, measured: the same lit frame with the cats'
    // spheres taken away (no occluders: the light passes through)
    scene.draw(&mut renderer, &game, &mut vfx, wide);
    renderer.occluders.clear();
    renderer.atmosphere = game.weather.atmosphere();
    renderer.render_frame(&view, wide, 1.0);
    save_shot(&renderer.device, &renderer.queue, &target, "ultra_2b_lit_no_occluders.png")?;
    println!("saved ultra_2b_lit_no_occluders.png");
    vfx.glows.clear();
    shoot(&mut renderer, &scene, &mut vfx, "ultra_3_mane.png", close)?;
    scene.ultra = false;
    shoot(&mut renderer, &scene, &mut vfx, "ultra_3b_mane_voxel.png", close)?;
    scene.ultra = true;
    // the other fur style, for comparison
    let mine = renderer.fur_style;
    let other = if mine == crate::render::FurStyle::Tape { crate::render::FurStyle::Locks } else { crate::render::FurStyle::Tape };
    renderer.set_fur_style(other);
    shoot(&mut renderer, &scene, &mut vfx, "ultra_3c_mane_other_style.png", close)?;
    renderer.set_fur_style(mine);
    // what it costs: the wide frame, voxel against ultra, best of 4
    for ultra in [false, true] {
        scene.ultra = ultra;
        let mut best = f32::MAX;
        for _ in 0..4 {
            scene.draw(&mut renderer, &game, &mut vfx, wide);
            let t = std::time::Instant::now();
            renderer.render_frame(&view, wide, 1.0);
            renderer.device.poll(wgpu::Maintain::Wait);
            best = best.min(t.elapsed().as_secs_f32() * 1000.0);
        }
        println!("frame, 4 cats, {}: {best:.0} ms (lavapipe)", if ultra { "ultra" } else { "voxel" });
    }
    Ok(())
}

/// Stillsmoke's Ninth Thunder among three other cats, in the ultra preview:
/// the channel mid-gather (motes, sigil, column), then a lightning strike
/// landing (the bolt, and its light throwing the cats' traced shadows),
/// and the same strike as voxels.
/// `CLAWBOUND_SHOTS=thunder clawbound-battleground --screenshot`
pub fn run_thunder_shots() -> Result<(), String> {
    std::env::set_var("CLAWBOUND_QUALITY", "ultra");
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thunder"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut scene = Scene::new(&mut renderer);
    let id = |n: &str| crate::gods::ROSTER.iter().position(|d| d.name == n).unwrap() as u32;
    let mut game = Game::new_teams(&[id("Stillsmoke"), id("Goldenmane")], &[id("Duskstripe"), id("Faryowl")]);
    game.blockers = scene.blockers.clone();
    let spot = Vec3::new(0.0, 0.0, -40.0);
    let aim = spot + Vec3::new(0.0, 0.0, 6.0);
    let places = [spot, spot + Vec3::new(-2.5, 0.0, 1.0), aim + Vec3::new(1.4, 0.0, 0.3), aim + Vec3::new(-1.6, 0.0, 1.2)];
    for (g, p) in game.gods.iter_mut().zip(places) {
        g.ai = None;
        g.pos = p;
        g.yaw = std::f32::consts::FRAC_PI_2; // facing +z, toward the storm
        g.mana = 10_000.0;
    }
    let mut vfx = Vfx::default();
    let step = |game: &mut Game, vfx: &mut Vfx| -> Vec<crate::sim::Event> {
        game.tick(1.0 / 60.0);
        let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
        vfx.update(1.0 / 60.0, &ev, game);
        ev
    };
    for _ in 0..20 {
        step(&mut game, &mut vfx);
    }
    let def = &crate::gods::ROSTER[game.gods[0].def_id as usize];
    let slot = (0..4).find(|&s| def.abilities[s].ult).unwrap() as u32;
    game.cast_queue.push((0, slot, aim));
    let cam = Camera { eye: spot + Vec3::new(5.5, 4.2, -6.5), target: spot + Vec3::new(-0.3, 1.2, 3.5), fov_y: 0.95 };
    let mut shoot = |renderer: &mut Renderer, scene: &Scene, vfx: &mut Vfx, game: &Game, name: &str| -> Result<(), String> {
        scene.draw(renderer, game, vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name} ({} lights)", renderer.lights.len());
        Ok(())
    };
    // mid-channel: 60% of the wind-up
    let wind = crate::abilities::cast_timing(&def.abilities[slot as usize]).0;
    for _ in 0..((wind * 0.6) * 60.0) as usize {
        step(&mut game, &mut vfx);
    }
    shoot(&mut renderer, &scene, &mut vfx, &game, "thunder_1_channel.png")?;
    // then on to the first strike that lands, a beat into the storm
    let mut seen = 0;
    for _ in 0..600 {
        let ev = step(&mut game, &mut vfx);
        if ev.iter().any(|e| matches!(e, crate::sim::Event::StrikeFx { .. })) {
            seen += 1;
            if seen == 3 {
                break;
            }
        }
    }
    shoot(&mut renderer, &scene, &mut vfx, &game, "thunder_2_strike.png")?;
    // the strike's shadows, measured: the same frame without the cats'
    // spheres (nothing to trace against)
    scene.draw(&mut renderer, &game, &mut vfx, cam);
    renderer.occluders.clear();
    renderer.atmosphere = game.weather.atmosphere();
    renderer.render_frame(&view, cam, game.t);
    save_shot(&renderer.device, &renderer.queue, &target, "thunder_2b_no_occluders.png")?;
    scene.ultra = false;
    shoot(&mut renderer, &scene, &mut vfx, &game, "thunder_3_strike_voxel.png")?;
    Ok(())
}

/// After rain, a low sun ahead: the path and flagstones wet, High beside
/// Ultra (ground as a mirror; god rays through the canopy's gaps).
/// `CLAWBOUND_SHOTS=wet clawbound-battleground --screenshot`
pub fn run_wet_shots() -> Result<(), String> {
    std::env::set_var("CLAWBOUND_QUALITY", "ultra");
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wet"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut scene = Scene::new(&mut renderer);
    let mut game = Game::new_teams(&[0], &[1]);
    game.blockers = scene.blockers.clone();
    let spot = Vec3::new(0.0, 0.0, -40.0);
    game.gods[0].pos = spot + Vec3::new(0.8, 0.0, 3.0);
    game.gods[0].yaw = std::f32::consts::FRAC_PI_2;
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    let mut vfx = Vfx::default();
    for _ in 0..20 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    let mut atm = game.weather.atmosphere();
    atm.wetness = 1.0;
    atm.rain = 0.0;
    atm.cloud_cover = 0.25;
    atm.sun_dir = Vec3::new(0.15, 0.2, 1.0).normalize();
    let cam = Camera { eye: spot + Vec3::new(0.0, 1.5, -6.0), target: spot + Vec3::new(0.0, 3.2, 12.0), fov_y: 1.0 };
    for (ultra, name) in [(false, "wet_high.png"), (true, "wet_ultra.png")] {
        scene.ultra = ultra;
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = atm;
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    Ok(())
}

/// The ground's surface: a cat at a lane's edge, a low sun. Dry (material
/// borders, contact-hardening shadows), light rain (water in the cracks and
/// hollows), heavy rain (more floods; in ultra, puddles reflect), and a
/// close-up of the paws.
/// `CLAWBOUND_SHOTS=surface clawbound-battleground --screenshot`
pub fn run_surface_shots() -> Result<(), String> {
    std::env::set_var("CLAWBOUND_QUALITY", "ultra");
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let mut game = Game::new_teams(&[0], &[1]);
    game.blockers = scene.blockers.clone();
    // the mid lane's edge: the path at x ~ 0, grass beyond ~ +7
    let spot = Vec3::new(5.5, 0.0, -30.0);
    game.gods[0].pos = spot;
    game.gods[0].yaw = 0.3;
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    let mut vfx = Vfx::default();
    for _ in 0..20 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    // diagnostics: CLAWBOUND_SURFACE_NOCAT renders the scene without the cat
    if std::env::var("CLAWBOUND_SURFACE_NOCAT").is_ok() {
        game.gods[0].respawn_t = 999.0;
    }
    let mut atm = game.weather.atmosphere();
    atm.rain = 0.0;
    atm.cloud_cover = 0.1;
    atm.sun_dir = Vec3::new(-0.7, 0.45, 0.35).normalize();
    let wide = Camera { eye: spot + Vec3::new(-3.0, 4.0, -7.0), target: spot + Vec3::new(0.5, 0.3, 2.0), fov_y: 1.0 };
    let close = Camera { eye: spot + Vec3::new(-1.2, 1.1, -2.2), target: spot + Vec3::new(0.3, 0.2, 0.3), fov_y: 0.8 };
    for (wet, cam, name) in [(0.0f32, wide, "surface_dry.png"), (0.35, wide, "surface_rain_light.png"), (0.9, wide, "surface_rain_heavy.png"), (0.0, close, "surface_paws.png")] {
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        atm.wetness = wet;
        renderer.atmosphere = atm;
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    Ok(())
}

/// The controls and settings panel, Ultra selected (so the fur choice and
/// its note show). `CLAWBOUND_SHOTS=settings clawbound-battleground --screenshot`
pub fn run_settings_shot() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("settings"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let ctx = egui::Context::default();
    let mut cfg = crate::input::Config::default();
    cfg.settings.quality = crate::quality::Quality::Ultra;
    cfg.settings.show_fps = true;
    clear(&renderer.device, &renderer.queue, &view);
    let mut textures = egui::TexturesDelta::default();
    let mut out = None;
    for k in 0..3 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
            time: Some(k as f64 * 0.016),
            ..Default::default()
        };
        let mut panel = ui::ControlsPanel { in_match: true, leave: false, close: false, capture: None, clear: None, scheme: None, reset: false };
        let o = ctx.run(raw, |c| {
            ui::draw_fps(c, 60.0, 16.7);
            ui::draw_controls(c, &mut cfg, None, false, &mut panel);
        });
        textures.append(o.textures_delta.clone());
        out = Some(o);
    }
    let mut out = out.expect("ran");
    out.textures_delta = textures;
    draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &ctx, out, W, H);
    save_shot(&renderer.device, &renderer.queue, &target, "settings_panel.png")?;
    println!("saved settings_panel.png");
    Ok(())
}

/// The upgraded ability effects: a dash's afterimages, a nova's dust wave,
/// a bolt's trail. `CLAWBOUND_SHOTS=abilityfx clawbound-battleground --screenshot`
pub fn run_abilityfx_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("abilityfx"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let spot = Vec3::new(0.0, 0.0, -40.0);
    let run = |renderer: &mut Renderer, who: &str, arch: fn(&crate::gods::Archetype) -> bool, frames: usize, name: &str, cam: Camera| -> Result<(), String> {
        let id = crate::gods::ROSTER.iter().position(|d| d.name == who).unwrap() as u32;
        let mut game = Game::new_teams(&[id], &[(id + 1) % 13]);
        game.blockers = scene.blockers.clone();
        game.gods[1].respawn_t = 999.0;
        game.gods[0].ai = None;
        game.gods[0].pos = spot;
        game.gods[0].yaw = 0.0;
        game.gods[0].mana = 10_000.0;
        let mut vfx = Vfx::default();
        for _ in 0..10 {
            game.tick(1.0 / 60.0);
            game.events.clear();
            vfx.update(1.0 / 60.0, &[], &game);
        }
        let def = &crate::gods::ROSTER[id as usize];
        let slot = (0..4).find(|&s| arch(&def.abilities[s].arch)).expect("archetype") as u32;
        game.cast_queue.push((0, slot, spot + Vec3::X * 9.0));
        for _ in 0..frames {
            game.tick(1.0 / 60.0);
            let ev: Vec<crate::sim::Event> = game.events.drain(..).collect();
            vfx.update(1.0 / 60.0, &ev, &game);
        }
        scene.draw(renderer, &game, &mut vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name} ({} afterimages, {} particles; caster shield {:.0})", vfx.afterimages.len(), vfx.particles.len(), game.gods[0].shield);
        // and the same frame without the dust, to find it by difference
        if !vfx.dust.is_empty() {
            let mut nodust = std::mem::take(&mut vfx.dust);
            scene.draw(renderer, &game, &mut vfx, cam);
            renderer.render_frame(&view, cam, game.t);
            save_shot(&renderer.device, &renderer.queue, &target, &name.replace(".png", "_nodust.png"))?;
            println!("  ({} dust)", nodust.len());
            std::mem::swap(&mut vfx.dust, &mut nodust);
        }
        // and without the rings, to identify what is left
        if name.contains("nova") {
            let rings = std::mem::take(&mut vfx.rings);
            let n_rings = rings.len();
            scene.draw(renderer, &game, &mut vfx, cam);
            renderer.render_frame(&view, cam, game.t);
            save_shot(&renderer.device, &renderer.queue, &target, &name.replace(".png", "_norings.png"))?;
            println!("  ({n_rings} rings)");
            vfx.rings = rings;
        }
        Ok(())
    };
    let side = Camera { eye: spot + Vec3::new(4.5, 3.0, -7.5), target: spot + Vec3::new(4.5, 0.8, 0.0), fov_y: 0.9 };
    run(&mut renderer, "Goldenmane", |a| matches!(a, crate::gods::Archetype::Dash { .. }), 12, "abilityfx_dash.png", side)?;
    run(&mut renderer, "Oldroar", |a| matches!(a, crate::gods::Archetype::Nova { .. }), 42, "abilityfx_nova.png", side)?;
    run(&mut renderer, "Suneye", |a| matches!(a, crate::gods::Archetype::Bolt { .. }), 22, "abilityfx_bolt.png", side)?;
    // whoever has each of these (the first ancestor with one)
    let who = |f: fn(&crate::gods::Archetype) -> bool| crate::gods::ROSTER.iter().find(|d| d.abilities.iter().any(|a| f(&a.arch))).map(|d| d.name).unwrap();
    let close = Camera { eye: spot + Vec3::new(2.5, 2.4, -5.0), target: spot + Vec3::new(0.3, 0.9, 0.0), fov_y: 0.9 };
    run(&mut renderer, who(|a| matches!(a, crate::gods::Archetype::Shield { .. })), |a| matches!(a, crate::gods::Archetype::Shield { .. }), 40, "abilityfx_shield.png", close)?;
    run(&mut renderer, who(|a| matches!(a, crate::gods::Archetype::Heal { .. })), |a| matches!(a, crate::gods::Archetype::Heal { .. }), 34, "abilityfx_heal.png", close)?;
    Ok(())
}

/// One scene at each quality: the shadow map rebuilt as the quality
/// changes (in one run -- the rebuild is what could go wrong), bloom and
/// particles scaled. `CLAWBOUND_SHOTS=quality clawbound-battleground --screenshot`
pub fn run_quality_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("quality"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let mut game = Game::new_teams(&[0, 1], &[2, 3]);
    game.blockers = scene.blockers.clone();
    let spot = Vec3::new(0.0, 0.0, -40.0);
    for (k, g) in game.gods.iter_mut().enumerate() {
        g.ai = None;
        g.pos = spot + Vec3::new(k as f32 * 2.2 - 3.3, 0.0, 0.0);
        g.yaw = -1.1;
    }
    let mut vfx = Vfx::default();
    for _ in 0..20 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    vfx.update(0.0, &[crate::sim::Event::NovaFx { pos: spot + Vec3::Z * 3.0, radius: 5.0, color: [1.0, 0.6, 0.2] }], &game);
    let cam = Camera { eye: spot + Vec3::new(0.5, 3.0, -7.5), target: spot + Vec3::Y * 0.8, fov_y: 0.9 };
    for q in [crate::quality::Quality::High, crate::quality::Quality::Medium, crate::quality::Quality::Low, crate::quality::Quality::High] {
        renderer.quality = q;
        renderer.set_shadow_size(q.shadow_size());
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, 1.0);
        let name = format!("quality_{}.png", q.key());
        save_shot(&renderer.device, &renderer.queue, &target, &name)?;
        println!("saved {name}: shadow map {} px", renderer.shadow_size);
    }
    Ok(())
}


/// The HUD's level track and ability ranks (`ranks`): a cat at level 4 with
/// the ultimate still locked, then the same cat just reaching 5 (the toast),
/// then at 15 with every ability at its top.
fn run_hud_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let scene = Scene::new(&mut renderer);
    let mut game = Game::new(14, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
    game.blockers = scene.blockers.clone();
    let mut vfx = Vfx::default();
    for _ in 0..60 {
        game.tick(1.0 / 60.0);
    }
    let p = game.gods[0].pos;
    let cam = Camera { eye: p + Vec3::new(6.0, 9.0, -10.0), target: p + Vec3::Y * 1.2, fov_y: 0.95 };
    let mut egui_renderer = egui_wgpu::Renderer::new(&renderer.device, wgpu::TextureFormat::Rgba8UnormSrgb, None, 1);
    let ctx = egui::Context::default();
    let mut st = UiState::new();
    for (name, level, xp_frac, toast) in [("hud_level4.png", 4u32, 0.6f32, false), ("hud_level5_toast.png", 5, 0.05, true), ("hud_level15.png", 15, 0.0, false)] {
        game.set_level(0, level);
        game.gods[0].xp = crate::consts::xp_for_level(level) * xp_frac;
        game.gods[0].cds = [0.0, 3.2, 0.0, 0.0];
        if !toast {
            st.level_seen = level;
            st.level_toast_t = 0.0;
        } else {
            st.level_seen = level - 1;
        }
        renderer.clear_queues();
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = game.weather.atmosphere();
        renderer.render_frame(&view, cam, game.t);
        let vp = cam.view_proj(W as f32 / H as f32);
        let mut textures = egui::TexturesDelta::default();
        let mut out = None;
        for k in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
                time: Some(1.0 + k as f64 * 0.016),
                ..Default::default()
            };
            let o = ctx.run(raw, |c| ui::draw_hud(c, &game, 0, vp, W as f32, H as f32, &mut st, renderer.camera.eye));
            textures.append(o.textures_delta.clone());
            out = Some(o);
        }
        let mut out = out.expect("ran");
        out.textures_delta = textures;
        draw_egui(&renderer.device, &renderer.queue, &mut egui_renderer, &view, &ctx, out, W, H);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    Ok(())
}


/// The lens flare (`lensflare`, post.wgsl): a low sun in frame on a clear
/// day, looked at from the lane at High and Ultra, and with the sun behind
/// a trunk (the flare must go out), and the same view on Low (none).
/// `CLAWBOUND_SHOTS=flare clawbound-battleground --screenshot`
pub fn run_flare_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("flare"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut scene = Scene::new(&mut renderer);
    let mut game = Game::new_teams(&[0], &[1]);
    game.blockers = scene.blockers.clone();
    let spot = Vec3::new(0.0, 0.0, -40.0);
    game.gods[0].pos = spot + Vec3::new(0.8, 0.0, 3.0);
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    let mut vfx = Vfx::default();
    for _ in 0..20 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    let mut atm = game.weather.atmosphere();
    atm.wetness = 0.0;
    atm.rain = 0.0;
    atm.cloud_cover = 0.05;
    atm.night = 0.0;
    atm.sun_dir = Vec3::new(0.35, 0.42, 1.0).normalize();
    let eye = spot + Vec3::new(0.0, 2.2, -6.0);
    let sun_x = atm.sun_dir.x;
    let sun_z = atm.sun_dir.z;
    let look = |yaw_off: f32, pitch: f32| {
        let d = Vec3::new(sun_x + yaw_off, pitch, sun_z).normalize();
        Camera { eye, target: eye + d * 20.0, fov_y: 1.0 }
    };
    let shots: [(&str, crate::quality::Quality, Camera); 4] = [
        ("flare_high.png", crate::quality::Quality::High, look(-0.3, 0.3)),
        ("flare_ultra_offaxis.png", crate::quality::Quality::Ultra, look(0.3, 0.2)),
        ("flare_low.png", crate::quality::Quality::Low, look(-0.3, 0.3)),
        // the sun low, behind the ruined pillar on the lane: no flare
        ("flare_occluded.png", crate::quality::Quality::High, look(-0.35, 0.02)),
    ];
    for (name, q, cam) in shots {
        let mut atm = atm;
        if name == "flare_occluded.png" {
            atm.sun_dir = Vec3::new(0.35, 0.16, 1.0).normalize();
        }
        renderer.quality = q;
        renderer.set_shadow_size(q.shadow_size());
        scene.ultra = q == crate::quality::Quality::Ultra && scene.ultra;
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = atm;
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    // Caustics on a river bed (common_sea.wgsl): a high sun over the
    // western river, looked down at, on High and on Low (none).
    let rx = crate::consts::river_x(-1.0, 10.0);
    let bed = Vec3::new(rx, 0.0, 10.0);
    let mut atm2 = atm;
    // the sun behind the camera: its glitter off the surface out of view
    atm2.sun_dir = Vec3::new(-0.35, 0.85, -0.45).normalize();
    for (name, q) in [("river_caustics_high.png", crate::quality::Quality::High), ("river_caustics_low.png", crate::quality::Quality::Low)] {
        renderer.quality = q;
        renderer.set_shadow_size(q.shadow_size());
        let cam = Camera { eye: bed + Vec3::new(-5.0, 6.5, -7.0), target: bed, fov_y: 0.9 };
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = atm2;
        renderer.render_frame(&view, cam, 3.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    Ok(())
}

/// The jungle's glades (`glades`), from a cat's height and from above: a
/// Ring, and the Steps up to a pit. `CLAWBOUND_SHOTS=glades`.
pub fn run_glade_shots() -> Result<(), String> {
    let (_i, _a, device, queue) = make_device().ok_or_else(|| "No suitable GPU backend".to_string())?;
    let mut renderer = Renderer::new(device, queue, W, H, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glades"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut scene = Scene::new(&mut renderer);
    let mut game = Game::new_teams(&[0], &[1]);
    game.blockers = scene.blockers.clone();
    let (rx, rz) = crate::glades::RING;
    game.gods[0].pos = Vec3::new(rx + 0.8, 1.4, rz + 1.2);
    game.gods[1].respawn_t = 999.0;
    game.gods[0].ai = None;
    let mut vfx = Vfx::default();
    for _ in 0..20 {
        game.tick(1.0 / 60.0);
        game.events.clear();
        vfx.update(1.0 / 60.0, &[], &game);
    }
    let mut atm = game.weather.atmosphere();
    atm.wetness = 0.0;
    atm.rain = 0.0;
    atm.cloud_cover = 0.2;
    atm.night = 0.0;
    atm.sun_dir = Vec3::new(0.3, 0.8, 0.4).normalize();
    renderer.atmosphere = atm;
    let pit = crate::bosses::pit_positions()[1];
    let steps_mid = Vec3::new(pit.x * 0.62, 0.0, pit.z * 0.62);
    let shots: [(&str, Camera); 3] = [
        ("glade_ring_above.png", Camera { eye: Vec3::new(rx - 14.0, 26.0, rz - 16.0), target: Vec3::new(rx, 0.0, rz), fov_y: 0.9 }),
        ("glade_ring_ground.png", Camera { eye: Vec3::new(rx - 13.0, 3.0, rz - 9.0), target: Vec3::new(rx + 2.0, 1.5, rz + 2.0), fov_y: 1.0 }),
        ("glade_steps.png", Camera { eye: steps_mid + Vec3::new(-16.0, 16.0, -14.0), target: steps_mid + Vec3::new(6.0, 0.0, 0.0), fov_y: 1.0 }),
    ];
    renderer.quality = crate::quality::Quality::High;
    renderer.set_shadow_size(crate::quality::Quality::High.shadow_size());
    for (name, cam) in shots {
        scene.draw(&mut renderer, &game, &mut vfx, cam);
        renderer.atmosphere = atm;
        renderer.render_frame(&view, cam, 1.0);
        save_shot(&renderer.device, &renderer.queue, &target, name)?;
        println!("saved {name}");
    }
    Ok(())
}
