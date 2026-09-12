// Windowed application: winit 0.29 event loop + egui wiring + player controls.

use std::collections::HashSet;
use std::time::Instant;

use glam::Vec3;
use winit::event::{ElementState, Event, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{EventLoop, EventLoopWindowTarget};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowBuilder};

use crate::render::{Camera, Renderer};
use crate::scene::Scene;
use crate::sim::Game;
use crate::ui::{self, UiState};
use crate::vfx::Vfx;

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Menu,
    Select,
    Playing,
}

struct AppState {
    window: &'static Window,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    scene: Option<Scene>,
    egui_ctx: egui::Context,
    egui_winit: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    ui: UiState,
    screen: Screen,
    game: Option<Game>,
    vfx: Vfx,
    keys: HashSet<KeyCode>,
    mouse_down: bool,
    locked: bool,
    scoreboard: bool,
    cam_yaw: f32,
    cam_pitch: f32,
    cam_dist: f32,
    last_frame: Instant,
    accumulator: f32,
    exiting: bool,
    #[cfg(feature = "audio")]
    ambience: Option<crate::audio::output::Ambience>,
    thunder_count: u32,
    thunder_dist: f32,
}

pub fn run() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut state: Option<AppState> = None;

    event_loop
        .run(move |event, elwt| {
            match event {
                Event::Resumed => {
                    if state.is_none() {
                        eprintln!("RESUMED: initializing window + GPU");
                        match init_window(elwt) {
                            Ok(s) => {
                                eprintln!("RESUMED: init ok — surface configured");
                                state = Some(s);
                            }
                            Err(msg) => {
                                eprintln!(
                                    "\n[clawbound] graphics initialization failed:\n  {msg}\n\nHints:\n  - force a backend with WGPU_BACKEND=metal|vulkan|dx12|gl|all\n  - update your GPU drivers\n"
                                );
                                elwt.exit();
                            }
                        }
                    }
                }
                Event::AboutToWait => {
                    if let Some(s) = state.as_ref() {
                        s.window.request_redraw();
                    }
                }
                Event::WindowEvent { event, .. } => {
                    if let Some(s) = state.as_mut() {
                        if event == WindowEvent::RedrawRequested {
                            render(s);
                            if s.exiting {
                                elwt.exit();
                            }
                        } else {
                            handle_window_event(s, event);
                        }
                    }
                }
                _ => {}
            }
        })
        .expect("event loop run");
}

struct GpuInit {
    surface: wgpu::Surface<'static>,
    #[allow(dead_code)]
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
}

fn try_gpu_backend(window: &'static Window, backends: wgpu::Backends) -> Result<GpuInit, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
    // wgpu 0.19 PANICS inside create_surface (instance.rs `hal_surface.unwrap()`)
    // when none of the requested backends managed to initialize its HAL instance
    // (no driver/ICD on this machine). Probe adapters first — enumeration never
    // touches the window surface, so it is panic-free.
    if instance.enumerate_adapters(backends).is_empty() {
        return Err("no adapters for this backend (driver/ICD missing or incompatible)".to_string());
    }
    let surface = instance
        .create_surface(window)
        .map_err(|e| format!("create_surface: {e}"))?;
    // SAFETY: `window` is a leaked &'static Window, so the returned
    // Surface<'static> stays valid for the life of the (single-window) process.
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| "no compatible adapter".to_string())?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("main device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
        },
        None,
    ))
    .map_err(|e| format!("request_device: {e}"))?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .or_else(|| caps.formats.first().copied())
        .ok_or_else(|| "surface exposes no formats".to_string())?;
    Ok(GpuInit { surface, adapter, device, queue, format })
}

/// Backend priority per platform. `WGPU_BACKEND` env overrides.
/// IMPORTANT: never request a backend mask whose instance can't exist on this
/// OS — wgpu 0.19 panics inside create_surface when NO enabled backend can
/// build the surface (`instance.rs: hal_surface.unwrap()`).
/// - stock macOS: only Metal (Vulkan needs MoltenVK, GL needs ANGLE)
/// - Windows: Vulkan → DX12 → GL
/// - Linux/other: Vulkan → GL
fn platform_backends() -> Vec<(&'static str, wgpu::Backends)> {
    if let Ok(env_backend) = std::env::var("WGPU_BACKEND") {
        let b = env_backend.trim().to_ascii_lowercase();
        let forced = match b.as_str() {
            "metal" => Some(wgpu::Backends::METAL),
            "vulkan" => Some(wgpu::Backends::VULKAN),
            "dx12" => Some(wgpu::Backends::DX12),
            "gl" | "gles" => Some(wgpu::Backends::GL),
            "primary" => Some(wgpu::Backends::PRIMARY),
            "all" => Some(wgpu::Backends::all()),
            other => {
                eprintln!("[clawbound] ignoring unknown WGPU_BACKEND='{other}' (metal|vulkan|dx12|gl|primary|all)");
                None
            }
        };
        let usable = forced.filter(|mask| {
            if cfg!(target_os = "macos") {
                // a mask without METAL can never create a surface on stock macOS
                mask.intersects(wgpu::Backends::METAL)
            } else {
                mask.intersects(wgpu::Backends::VULKAN | wgpu::Backends::GL | wgpu::Backends::DX12)
            }
        });
        match (forced, usable) {
            (Some(_), Some(mask)) => {
                eprintln!("[clawbound] WGPU_BACKEND override: forcing {mask:?}");
                return vec![("forced", mask)];
            }
            (Some(mask), None) => {
                eprintln!("[clawbound] WGPU_BACKEND={mask:?} cannot work on this OS (no MoltenVK/ANGLE?) — using platform defaults");
            }
            _ => {}
        }
    }
    let mut list = Vec::new();
    if cfg!(target_os = "macos") {
        list.push(("metal", wgpu::Backends::METAL));
    } else {
        if cfg!(windows) {
            list.push(("dx12", wgpu::Backends::DX12));
        }
        list.push(("vulkan", wgpu::Backends::VULKAN));
        list.push(("gl", wgpu::Backends::GL));
    }
    list
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn init_window(target: &EventLoopWindowTarget<()>) -> Result<AppState, String> {
    let window = WindowBuilder::new()
        .with_title("Clawbound — Battleground · Rust + wgpu Microvoxel Edition")
        .with_inner_size(winit::dpi::LogicalSize::new(1366.0f64, 768.0f64))
        .build(target)
        .map_err(|e| format!("window creation failed: {e}"))?;
    // leak the window so the surface can be 'static (one window per process)
    let window: &'static Window = Box::leak(Box::new(window));
    let size = window.inner_size();

    let mut errors = Vec::new();
    let mut gpu = None;
    for (name, mask) in platform_backends() {
        // wgpu 0.19 has several internal `unwrap()`s along its instance/surface
        // paths (missing driver/ICD, missing ANGLE, headless EGL…). Contain
        // them: a backend that panics is simply "unavailable" on this machine.
        let attempt =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| try_gpu_backend(window, mask)));
        match attempt {
            Ok(Ok(g)) => {
                let info = g.adapter.get_info();
                eprintln!("[clawbound] GPU ok — backend '{name}': '{}' ({:?})", info.name, info.backend);
                gpu = Some(g);
                break;
            }
            Ok(Err(e)) => {
                eprintln!("[clawbound] backend '{name}' unavailable: {e}");
                errors.push(format!("{name}: {e}"));
            }
            Err(p) => {
                let msg = panic_message(&p);
                eprintln!("[clawbound] backend '{name}' crashed internally: {msg}");
                errors.push(format!("{name}: wgpu internal panic ({msg})"));
            }
        }
    }
    let GpuInit { surface, device, queue, format, .. } = gpu.ok_or_else(|| {
        format!(
            "no usable GPU backend (tried {}). Set WGPU_BACKEND=metal|vulkan|dx12|gl|all to override, and update your GPU drivers.",
            errors.join(" | ")
        )
    })?;

    surface.configure(
        &device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        },
    );

    let egui_ctx = egui::Context::default();
    let egui_winit = egui_winit::State::new(egui_ctx.clone(), egui::ViewportId::ROOT, window, None, None);
    let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1);
    let renderer = Renderer::new(device, queue, size.width.max(1), size.height.max(1), format);

    Ok(AppState {
        window,
        surface,
        surface_format: format,
        renderer,
        scene: None,
        egui_ctx,
        egui_winit,
        egui_renderer,
        ui: UiState::new(),
        screen: Screen::Menu,
        game: None,
        vfx: Vfx::default(),
        keys: HashSet::new(),
        mouse_down: false,
        locked: false,
        scoreboard: false,
        cam_yaw: std::f32::consts::PI,
        cam_pitch: 0.38,
        cam_dist: 8.5,
        last_frame: Instant::now(),
        accumulator: 0.0,
        exiting: false,
        #[cfg(feature = "audio")]
        ambience: None,
        thunder_count: 0,
        thunder_dist: 1.0,
    })
}

fn handle_window_event(s: &mut AppState, event: WindowEvent) {
    let egui_response = s.egui_winit.on_window_event(&s.window, &event);

    match event {
        WindowEvent::CloseRequested => s.exiting = true,
        WindowEvent::Resized(size) => {
            s.renderer.resize(size.width.max(1), size.height.max(1));
            s.surface.configure(
                &s.renderer.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: s.surface_format,
                    width: size.width.max(1),
                    height: size.height.max(1),
                    present_mode: wgpu::PresentMode::Fifo,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                },
            );
        }
        WindowEvent::CursorMoved { position, .. } => {
            if s.locked {
                let center = winit::dpi::PhysicalPosition::new(
                    s.window.inner_size().width as f64 / 2.0,
                    s.window.inner_size().height as f64 / 2.0,
                );
                let dx = position.x - center.x;
                let dy = position.y - center.y;
                s.cam_yaw -= dx as f32 * 0.0045;
                s.cam_pitch = (s.cam_pitch + dy as f32 * 0.0035).clamp(0.06, 1.15);
                let _ = s.window.set_cursor_position(center);
            }
        }
        WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
            if !egui_response.consumed {
                if s.screen == Screen::Playing && !s.locked && !s.ui.shop_open {
                    s.locked = true;
                    let _ = s.window.set_cursor_grab(winit::window::CursorGrabMode::Locked)
                        .or_else(|_| s.window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
                    s.window.set_cursor_visible(false);
                }
                s.mouse_down = true;
            }
        }
        WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
            s.mouse_down = false;
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let d = match delta {
                winit::event::MouseScrollDelta::LineDelta(_, y) => y * 0.8,
                winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.02,
            };
            s.cam_dist = (s.cam_dist - d).clamp(4.5, 16.0);
        }
        WindowEvent::KeyboardInput {
            event:
                KeyEvent {
                    state: ElementState::Pressed,
                    physical_key: PhysicalKey::Code(code),
                    ..
                },
            ..
        } => {
            if !egui_response.consumed {
                s.keys.insert(code);
                handle_key(s, code);
            }
        }
        WindowEvent::KeyboardInput {
            event:
                KeyEvent {
                    state: ElementState::Released,
                    physical_key: PhysicalKey::Code(code),
                    ..
                },
            ..
        } => {
            s.keys.remove(&code);
            if code == KeyCode::Tab || code == KeyCode::KeyC {
                s.scoreboard = false;
            }
        }
        _ => {}
    }
}

fn handle_key(s: &mut AppState, code: KeyCode) {
    match code {
        KeyCode::Escape => {
            if s.screen == Screen::Playing {
                s.locked = false;
                s.ui.shop_open = false;
                let _ = s.window.set_cursor_grab(winit::window::CursorGrabMode::None);
                s.window.set_cursor_visible(true);
            }
        }
        KeyCode::Tab | KeyCode::KeyC => {
            if s.screen == Screen::Playing {
                s.scoreboard = true;
            }
        }
        KeyCode::KeyB => {
            if s.screen == Screen::Playing {
                s.ui.shop_open = !s.ui.shop_open;
                if s.ui.shop_open && s.locked {
                    s.locked = false;
                    let _ = s.window.set_cursor_grab(winit::window::CursorGrabMode::None);
                    s.window.set_cursor_visible(true);
                }
            }
        }
        KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 => {
            if s.screen == Screen::Playing && s.game.is_some() {
                let slot = match code {
                    KeyCode::Digit1 => 0,
                    KeyCode::Digit2 => 1,
                    KeyCode::Digit3 => 2,
                    _ => 3,
                };
                let aim = compute_aim(s);
                if let Some(g) = s.game.as_mut() {
                    g.cast_queue.push((0, slot, aim));
                }
            }
        }
        _ => {}
    }
}

fn cam_dir(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos())
}

fn compute_aim(s: &AppState) -> Vec3 {
    let player = &s.game.as_ref().unwrap().gods[0];
    let dir = cam_dir(s.cam_yaw, s.cam_pitch);
    let eye = player.pos + Vec3::Y * 1.5 - dir * s.cam_dist;
    if dir.y < -0.02 {
        let t = -eye.y / dir.y;
        let hit = eye + dir * t;
        let d = Vec3::new(hit.x, 0.0, hit.z) - Vec3::new(player.pos.x, 0.0, player.pos.z);
        if d.length() > 30.0 {
            player.pos + d.normalize() * 30.0
        } else {
            Vec3::new(hit.x, 0.0, hit.z)
        }
    } else {
        let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
        player.pos + f * 14.0
    }
}

fn start_game(s: &mut AppState) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(42);
    let mut rng = crate::sim::Rng::new(0xBEEF ^ seed);
    let mut pool: Vec<u32> = (0..12u32).filter(|&i| i != s.ui.selected as u32).collect();
    let mut allies = [0u32; 4];
    let mut enemies = [0u32; 5];
    for a in allies.iter_mut() {
        let k = (rng.next_u() % pool.len() as u64) as usize;
        *a = pool.remove(k);
    }
    for e in enemies.iter_mut() {
        let k = (rng.next_u() % pool.len() as u64) as usize;
        *e = pool.remove(k.min(pool.len() - 1));
    }
    s.game = Some(Game::new(s.ui.selected as u32, allies, enemies));
    s.vfx = Vfx::default();
    s.screen = Screen::Playing;
    #[cfg(feature = "audio")]
    if s.ambience.is_none() {
        s.ambience = crate::audio::output::Ambience::start();
    }
    s.ui.floaters.clear();
    s.ui.announce = None;
    s.ui.shop_open = false;
    s.cam_yaw = std::f32::consts::PI;
    s.cam_pitch = 0.38;
}

fn render(s: &mut AppState) {
    let dt_frame = s.last_frame.elapsed().as_secs_f32().min(0.05);
    s.last_frame = Instant::now();

    if s.scene.is_none() {
        s.scene = Some(Scene::new(&mut s.renderer));
        render_portraits(s);
    }
    let scene = s.scene.as_ref().unwrap();

    // fixed-step simulation + player input
    if s.screen == Screen::Playing {
        if let Some(g) = s.game.as_mut() {
            s.accumulator = (s.accumulator + dt_frame).min(0.25);
            while s.accumulator >= 1.0 / 60.0 {
                s.accumulator -= 1.0 / 60.0;
                let mut mv = Vec3::ZERO;
                let yaw = s.cam_yaw;
                let fwd = Vec3::new(yaw.sin(), 0.0, yaw.cos()).normalize_or_zero();
                let right = Vec3::new(fwd.z, 0.0, -fwd.x);
                if s.keys.contains(&KeyCode::KeyW) { mv += fwd; }
                if s.keys.contains(&KeyCode::KeyS) { mv -= fwd; }
                if s.keys.contains(&KeyCode::KeyD) { mv += right; }
                if s.keys.contains(&KeyCode::KeyA) { mv -= right; }
                let gi = &mut g.gods[0];
                gi.move_dir = if !s.ui.shop_open { mv } else { Vec3::ZERO };
                gi.attack_held = s.mouse_down && !s.ui.shop_open;
                g.tick(1.0 / 60.0);
                let events: Vec<crate::sim::Event> = g.events.drain(..).collect();
                for e in &events {
                    match e {
                        crate::sim::Event::Damage { pos, amount, big } => {
                            if s.ui.floaters.len() < 50 && *amount > 0.0 {
                                s.ui.push_events(&[ui::Event2::Damage { pos: *pos, amount: *amount, big: *big }]);
                            }
                        }
                        crate::sim::Event::Announce { text } => {
                            s.ui.push_events(&[ui::Event2::Announce { text: text.clone(), dur: 2.5 }]);
                        }
                        // sky-tall bolts are lightning (weather or Thunder Front)
                        crate::sim::Event::BoltFx { from, to, .. } if from.y - to.y > 30.0 => {
                            s.thunder_count = s.thunder_count.wrapping_add(1);
                            s.thunder_dist = (to.distance(g.gods[0].pos) / 160.0).clamp(0.0, 1.0);
                        }
                        _ => {}
                    }
                }
                s.vfx.update(1.0 / 60.0, &events, g);
            }
        }
        s.ui.tick(dt_frame);
    }

    // camera
    let cam = match s.screen {
        Screen::Menu | Screen::Select => {
            let t = s.last_frame.elapsed().as_secs_f32() * 0.05;
            let r = 55.0;
            Camera {
                eye: Vec3::new(t.cos() * r, 32.0, t.sin() * r),
                target: Vec3::ZERO,
                fov_y: 0.9,
            }
        }
        Screen::Playing => {
            let player = &s.game.as_ref().unwrap().gods[0];
            let dir = cam_dir(s.cam_yaw, s.cam_pitch);
            let target = player.pos + Vec3::Y * 1.5;
            Camera {
                eye: target - dir * s.cam_dist,
                target,
                fov_y: 0.95,
            }
        }
    };
    let vp = cam.view_proj(s.renderer.width as f32 / s.renderer.height as f32);

    // ambience: the listener is the player; the wind pans with the camera
    #[cfg(feature = "audio")]
    if let (Some(g), Some(amb)) = (s.game.as_ref(), s.ambience.as_ref()) {
        let p = crate::audio::params_for(g, g.gods[0].pos, s.cam_yaw, s.thunder_count, s.thunder_dist);
        amb.set(p);
    }

    // world draws (+ this frame's sky state)
    if let Some(g) = s.game.as_ref() {
        s.renderer.atmosphere = g.weather.atmosphere();
        let mut vfx = std::mem::take(&mut s.vfx);
        scene.draw(&mut s.renderer, g, &mut vfx, cam);
        s.vfx = vfx;
    } else {
        s.renderer.atmosphere = crate::weather::Weather::new().atmosphere();
        s.renderer.clear_queues();
    }

    // surface frame
    let frame = match s.surface.get_current_texture() {
        Ok(f) => Some(f),
        Err(_) => {
            s.surface.configure(
                &s.renderer.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: s.surface_format,
                    width: s.renderer.width,
                    height: s.renderer.height,
                    present_mode: wgpu::PresentMode::Fifo,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                },
            );
            // Minimized/occluded windows can briefly fail to produce a frame;
            // skip this frame instead of panicking.
            s.surface
                .get_current_texture()
                .map_err(|e| eprintln!("[clawbound] skipping frame: {e}"))
                .ok()
        }
    };
    let Some(frame) = frame else { return; };
    let view = frame.texture.create_view(&Default::default());
    s.renderer.render_frame(&view, cam, s.last_frame.elapsed().as_secs_f32());

    // egui frame
    let raw_input = s.egui_winit.take_egui_input(s.window);
    let mut play = false;
    let mut quit = false;
    let mut locked_in = false;
    let mut again = false;
    let mut menu = false;
    let output = s.egui_ctx.run(raw_input, |ctx| {
        match s.screen {
            Screen::Menu => ui::draw_menu(ctx, &mut play, &mut quit),
            Screen::Select => ui::draw_select(ctx, &mut s.ui, &mut locked_in),
            Screen::Playing => {
                if let Some(g) = s.game.as_mut() {
                    if s.ui.shop_open {
                        ui::draw_shop(ctx, g, 0, &mut s.ui);
                    } else {
                        ui::draw_hud(ctx, g, 0, vp, s.renderer.width as f32, s.renderer.height as f32, &mut s.ui);
                    }
                    if s.scoreboard {
                        ui::draw_scoreboard(ctx, g);
                    }
                    if g.over.is_some() {
                        ui::draw_end(ctx, g, &mut again, &mut menu);
                    }
                }
            }
        }
    });
    s.egui_winit.handle_platform_output(s.window, output.platform_output);

    for td in output.textures_delta.set.iter() {
        s.egui_renderer.update_texture(&s.renderer.device, &s.renderer.queue, td.0, &td.1);
    }
    let screen_desc = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [s.renderer.width, s.renderer.height],
        pixels_per_point: s.egui_ctx.pixels_per_point(),
    };
    let clipped = s.egui_ctx.tessellate(output.shapes, screen_desc.pixels_per_point);
    let mut encoder = s.renderer.device.create_command_encoder(&Default::default());
    let mut cbuffers = s.egui_renderer.update_buffers(&s.renderer.device, &s.renderer.queue, &mut encoder, &clipped, &screen_desc);
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        s.egui_renderer.render(&mut rpass, &clipped, &screen_desc);
    }
    cbuffers.push(encoder.finish());
    s.renderer.queue.submit(cbuffers);
    for td in output.textures_delta.free.iter() {
        s.egui_renderer.free_texture(td);
    }
    frame.present();

    // transitions
    if play {
        s.screen = Screen::Select;
    }
    if quit {
        s.exiting = true;
    }
    if locked_in || again {
        start_game(s);
    }
    if menu {
        s.screen = Screen::Menu;
        s.game = None;
    }
}

fn render_portraits(s: &mut AppState) {
    let scene = s.scene.as_ref().unwrap();
    for i in 0..crate::gods::ROSTER.len() {
        let body = scene.god_body[i][0];
        let weapon = scene.god_weapon[i][0];
        let pixels = s.renderer.render_portrait(body, weapon);
        let image = egui::ColorImage::from_rgba_unmultiplied([128, 128], &pixels);
        let tid = s.egui_ctx.load_texture(format!("ancestor{}", i), image, egui::TextureOptions::LINEAR);
        s.ui.portraits.insert(i as u32, tid.id());
    }
}
