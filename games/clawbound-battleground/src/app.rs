// Windowed application: winit 0.29 event loop + egui wiring + player controls.
//
// Input flows one way: winit events → `input::InputEvent` (logical keys, raw
// mouse motion, touch) → `input::resolve()` once per frame → `Controls` → sim.
// Nothing below reads a KeyCode; the bindings decide what a key means.

use std::time::Instant;

use glam::{Vec2, Vec3};
use winit::event::{DeviceEvent, ElementState, Event, KeyEvent, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{EventLoop, EventLoopWindowTarget};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowBuilder};

use crate::gamepad::{Pad, PadState};
use crate::input::{self, Action, Bind, Config, Controls, InputEvent, InputState, MouseBtn, Named};
use crate::touch::TouchPad;

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
    /// Browsing or hosting on the local network.
    Lan,
}

struct AppState {
    window: &'static Window,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    /// The present modes the surface offers, and the one in use (vsync on:
    /// Fifo; off: `present_mode_for`).
    present_modes: Vec<wgpu::PresentMode>,
    present: wgpu::PresentMode,
    renderer: Renderer,
    scene: Option<Scene>,
    egui_ctx: egui::Context,
    egui_winit: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    ui: UiState,
    screen: Screen,
    game: Option<Game>,
    vfx: Vfx,
    // --- input stack
    cfg: Config,
    input: InputState,
    pad: Pad,
    pad_state: PadState,
    touch: TouchPad,
    controls: Controls,
    /// Controls panel open (pause / rebind). `capturing` = waiting for a press.
    controls_open: bool,
    capturing: Option<Action>,
    /// Walk-to order from a tap (world ground point).
    walk_to: Option<Vec3>,
    /// Open only while the LAN screen is: it holds a socket.
    lan: Option<crate::lanlobby::LanLobby>,
    /// The stalk toggle. App state, not sim state: the sim decides whether
    /// a cat that WANTS to stalk currently can.
    stalk_on: bool,
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
    /// The match opening, while it runs (`intro`). The sim holds until it
    /// is done; any key, click or touch skips it.
    intro: Option<crate::intro::Intro>,
    skip_intro: bool,
    /// Counter the audio synth watches: a change starts the fanfare.
    fanfare_count: u32,
    /// The ambience's last params, faded to silence out of a match.
    #[cfg(feature = "audio")]
    audio_last: crate::audio::AudioParams,
    /// Loaded audio (`soundbank`), decoding in the background.
    bank: Option<crate::soundbank::Bank>,
    /// This tick's events, for the audio cues.
    audio_events: Vec<crate::sim::Event>,
    voice_gate: crate::cues::VoiceGate,
    /// Whether this match's music has been started.
    music_on: bool,
    /// The game-state announcer (`announcer`).
    announcer: crate::announcer::Announcer,
    /// Frames since the listener's space was last sent to the mixer.
    space_tick: u32,
    /// The volumes last sent to the mixer (master, music, sfx, voice).
    vol_sent: [f32; 4],
    /// Smoothed frame time, seconds (the frame-rate readout).
    frame_s: f32,
    /// Intros played this session: the first is the full, narrated one.
    intros_played: u32,
    /// A scored intro track is playing (it replaces the synthesized fanfare).
    intro_music: bool,
    /// Watching an all-bot match (`spectator`); None when playing.
    spectator: Option<crate::spectator::Spectator>,
    /// Spectator switches pressed since last frame (Tab, Z, X).
    spec_cycle: i32,
    spec_director: bool,
    spec_free: bool,
    /// A prepared ability awaiting the attack button (`targeting`).
    targeting: crate::targeting::Targeting,
    /// The attack button as last frame saw it (for its press edge), and
    /// whether the current press belongs to the targeting (fired an
    /// ability) so it must not also start auto-attacks until released.
    attack_was: bool,
    attack_eaten: bool,
    thunder_dist: f32,
    launch: crate::launch::LaunchOptions,
}

pub fn run(opts: crate::launch::LaunchOptions) {
    // Before anything builds geometry: the renderer meshes the terrain.
    crate::map::set_active(opts.map);
    if let Some(role) = &opts.handoff {
        // The centre handed us a friend session: it becomes a den (see the
        // RESUMED block). This used to log "not wired yet" and start a solo
        // match. The MATCH is still not networked; the den is.
        eprintln!(
            "[clawbound] bsgames handoff ({role:?}, me={:?}, peer={:?}): opening a den",
            opts.player_name, opts.peer_id
        );
    }
    let event_loop = EventLoop::new().expect("event loop");
    let mut state: Option<AppState> = None;

    event_loop
        .run(move |event, elwt| {
            match event {
                Event::Resumed => {
                    if state.is_none() {
                        eprintln!("RESUMED: initializing window + GPU");
                        match init_window(elwt) {
                            Ok(mut s) => {
                                eprintln!("RESUMED: init ok — surface configured");
                                s.launch = opts.clone();
                                if let Some(role) = &opts.handoff {
                                    let id = opts.player_id.clone().unwrap_or_else(|| format!("cat-{:x}", std::process::id()));
                                    let name = opts.player_name.clone().unwrap_or_else(|| "Cat".into());
                                    let mp = netcode::multiplayer::Multiplayer::new(crate::lanlobby::GAME, &id, &name);
                                    let hosting = matches!(role, crate::launch::HandoffRole::Host);
                                    s.lan = Some(crate::lanlobby::LanLobby::open_for_handoff(hosting, opts.peer_id.as_deref(), mp, netcode::lan::LAN_PORT));
                                    s.screen = Screen::Lan; // before the quick match, which only starts from Menu
                                } else if opts.lan {
                                    let me = opts.player_name.clone().or_else(|| std::env::var("USER").ok()).unwrap_or_else(|| "Cat".into());
                                    s.lan = Some(crate::lanlobby::LanLobby::open(&me));
                                    s.screen = Screen::Lan;
                                }
                                if let Some(n) = &opts.player_name {
                                    s.ui.player_label = Some(n.clone());
                                }
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
                Event::DeviceEvent { event, .. } => {
                    if let Some(s) = state.as_mut() {
                        handle_device_event(s, event);
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
    let GpuInit { surface, device, queue, format, adapter, .. } = gpu.ok_or_else(|| {
        format!(
            "no usable GPU backend (tried {}). Set WGPU_BACKEND=metal|vulkan|dx12|gl|all to override, and update your GPU drivers.",
            errors.join(" | ")
        )
    })?;
    // the present modes this surface offers (vsync off picks from these)
    let present_modes = surface.get_capabilities(&adapter).present_modes;
    let cfg0 = Config::load();
    let present = present_mode_for(cfg0.settings.vsync, &present_modes);
    eprintln!("[clawbound] present modes {present_modes:?}; using {present:?}");

    surface.configure(
        &device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: present,
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
        present_modes,
        present,
        renderer,
        scene: None,
        egui_ctx,
        egui_winit,
        egui_renderer,
        ui: UiState::new(),
        screen: Screen::Menu,
        game: None,
        vfx: Vfx::default(),
        cfg: {
            let c = Config::load();
            eprintln!("[clawbound] controls: {} scheme{}", c.bindings.scheme.name(), Config::path().map(|p| format!(" ({})", p.display())).unwrap_or_default());
            c
        },
        input: InputState::default(),
        pad: Pad::new(),
        pad_state: PadState::default(),
        touch: TouchPad::new(),
        controls: Controls::default(),
        controls_open: false,
        capturing: None,
        walk_to: None,
        lan: None,
        stalk_on: false,
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
        intro: None,
        skip_intro: false,
        fanfare_count: 0,
        #[cfg(feature = "audio")]
        // silent until a match: the menu has no wind to fade from
        audio_last: crate::audio::AudioParams { master: 0.0, ..crate::audio::AudioParams::default() },
        bank: None,
        audio_events: Vec::new(),
        voice_gate: Default::default(),
        music_on: false,
        announcer: Default::default(),
        space_tick: 0,
        vol_sent: [-1.0; 4],
        frame_s: 1.0 / 60.0,
        intros_played: 0,
        intro_music: false,
        spectator: None,
        spec_cycle: 0,
        spec_director: false,
        spec_free: false,
        targeting: Default::default(),
        attack_was: false,
        attack_eaten: false,
        thunder_dist: 1.0,
        launch: crate::launch::LaunchOptions::default(),
    })
}

/// winit logical key → our backend-neutral bind (None for keys we don't map).
fn bind_of_key(k: &Key) -> Option<Bind> {
    Some(match k {
        Key::Character(s) => input::ch(s.as_str()),
        Key::Named(n) => Bind::Named(match n {
            NamedKey::Space => Named::Space,
            NamedKey::Shift => Named::Shift,
            NamedKey::Control => Named::Control,
            NamedKey::Alt => Named::Alt,
            NamedKey::Tab => Named::Tab,
            NamedKey::Escape => Named::Escape,
            NamedKey::Enter => Named::Enter,
            NamedKey::Backspace => Named::Backspace,
            NamedKey::ArrowUp => Named::ArrowUp,
            NamedKey::ArrowDown => Named::ArrowDown,
            NamedKey::ArrowLeft => Named::ArrowLeft,
            NamedKey::ArrowRight => Named::ArrowRight,
            NamedKey::PageUp => Named::PageUp,
            NamedKey::PageDown => Named::PageDown,
            NamedKey::Home => Named::Home,
            NamedKey::End => Named::End,
            NamedKey::CapsLock => Named::CapsLock,
            NamedKey::F1 => Named::F1,
            NamedKey::F2 => Named::F2,
            NamedKey::F3 => Named::F3,
            NamedKey::F4 => Named::F4,
            NamedKey::F5 => Named::F5,
            _ => return None,
        }),
        _ => return None,
    })
}

fn bind_of_mouse(b: MouseButton) -> Bind {
    Bind::Mouse(match b {
        MouseButton::Left => MouseBtn::Left,
        MouseButton::Right => MouseBtn::Right,
        MouseButton::Middle => MouseBtn::Middle,
        MouseButton::Back => MouseBtn::Back,
        MouseButton::Forward => MouseBtn::Forward,
        MouseButton::Other(n) => MouseBtn::Other(n),
    })
}

fn set_locked(s: &mut AppState, lock: bool) {
    if s.locked == lock {
        return;
    }
    s.locked = lock;
    if lock {
        // Locked is the right mode on X11/Windows/macOS; Wayland compositors
        // that refuse it fall back to Confined + hidden cursor, and the raw
        // DeviceEvent::MouseMotion path still delivers deltas either way.
        let _ = s.window.set_cursor_grab(winit::window::CursorGrabMode::Locked)
            .or_else(|_| s.window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
        s.window.set_cursor_visible(false);
    } else {
        let _ = s.window.set_cursor_grab(winit::window::CursorGrabMode::None);
        s.window.set_cursor_visible(true);
    }
}

fn handle_device_event(s: &mut AppState, event: DeviceEvent) {
    if let DeviceEvent::MouseMotion { delta } = event {
        if s.locked {
            s.input.handle(InputEvent::MouseDelta(delta.0 as f32, delta.1 as f32));
        }
    }
}

fn handle_window_event(s: &mut AppState, event: WindowEvent) {
    let egui_response = s.egui_winit.on_window_event(&s.window, &event);
    let size = s.window.inner_size();
    let short = size.width.min(size.height).max(1) as f32;
    // touch positions in normalized units of the shorter side, y down
    let norm = |p: winit::dpi::PhysicalPosition<f64>| Vec2::new(p.x as f32 / short, p.y as f32 / short);

    match event {
        WindowEvent::CloseRequested => s.exiting = true,
        WindowEvent::Focused(false) => {
            s.input.handle(InputEvent::FocusLost);
            // release the OS grab and show the cursor while the window is
            // in the background — otherwise `s.locked` can say "captured"
            // while the compositor has already let go of it behind our
            // back, and a later click silently no-ops instead of re-locking.
            set_locked(s, false);
        }
        WindowEvent::Resized(size) => {
            s.renderer.resize(size.width.max(1), size.height.max(1));
            let sh = size.width.min(size.height).max(1) as f32;
            s.touch.extent = Vec2::new(size.width.max(1) as f32 / sh, size.height.max(1) as f32 / sh);
            s.surface.configure(
                &s.renderer.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: s.surface_format,
                    width: size.width.max(1),
                    height: size.height.max(1),
                    present_mode: s.present,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                },
            );
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let b = bind_of_mouse(button);
            if state == ElementState::Pressed && s.intro.is_some() {
                s.skip_intro = true;
                return;
            }
            match state {
                ElementState::Pressed => {
                    if let Some(a) = s.capturing.take() {
                        s.cfg.bindings.capture(a, b);
                        s.cfg.save();
                        return;
                    }
                    if !egui_response.consumed {
                        // a click on the world while the cursor is free re-captures it
                        if s.screen == Screen::Playing && !s.locked && !s.ui.shop_open && !s.controls_open && !s.cfg.settings.touch_enabled {
                            set_locked(s, true);
                        }
                        s.input.handle(InputEvent::Press(b));
                    }
                }
                ElementState::Released => s.input.handle(InputEvent::Release(b)),
            }
        }
        WindowEvent::MouseWheel { delta, .. } => {
            if !egui_response.consumed {
                let d = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y * 0.8,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.02,
                };
                s.input.handle(InputEvent::Scroll(d));
            }
        }
        WindowEvent::KeyboardInput { event: KeyEvent { state, logical_key, repeat, .. }, .. } => {
            if state == ElementState::Pressed && !repeat && s.intro.is_some() {
                s.skip_intro = true;
                return;
            }
            if state == ElementState::Pressed && !repeat && s.spectator.is_some() {
                use winit::keyboard::{Key, NamedKey};
                match &logical_key {
                    Key::Named(NamedKey::Tab) => {
                        s.spec_cycle += 1;
                        return;
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("z") => {
                        s.spec_director = true;
                        return;
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("x") => {
                        s.spec_free = true;
                        return;
                    }
                    _ => {}
                }
            }
            let Some(b) = bind_of_key(&logical_key) else { return };
            match state {
                ElementState::Pressed if !repeat => {
                    if let Some(a) = s.capturing.take() {
                        // Esc cancels a capture instead of binding it
                        if b != Bind::Named(Named::Escape) {
                            s.cfg.bindings.capture(a, b);
                            s.cfg.save();
                        }
                        return;
                    }
                    // typing into egui (shop search etc.) must not move the ancestor
                    if !egui_response.consumed || b == Bind::Named(Named::Escape) {
                        s.input.handle(InputEvent::Press(b));
                    }
                }
                ElementState::Pressed => {}
                ElementState::Released => s.input.handle(InputEvent::Release(b)),
            }
        }
        WindowEvent::Touch(t) if s.intro.is_some() => {
            if t.phase == winit::event::TouchPhase::Started {
                s.skip_intro = true;
            }
        }
        WindowEvent::Touch(t) => {
            // a touchscreen announces itself: enable the on-screen pad on first touch
            if !s.cfg.settings.touch_enabled {
                s.cfg.settings.touch_enabled = true;
                s.cfg.save();
                set_locked(s, false);
            }
            s.touch.enabled = true;
            s.touch.scale = s.cfg.settings.touch_scale;
            let p = norm(t.location);
            match t.phase {
                TouchPhase::Started => {
                    if !egui_response.consumed {
                        s.touch.down(t.id, p);
                    }
                }
                TouchPhase::Moved => s.touch.moved(t.id, p),
                TouchPhase::Ended => s.touch.up(t.id),
                TouchPhase::Cancelled => s.touch.cancel(t.id),
            }
        }
        _ => {}
    }
}

/// Once per frame: poll the pad, resolve everything to `Controls`, and act on
/// the meta actions (menu, shop, scoreboard, capture) here; gameplay intent
/// is applied in the fixed-step loop.
fn gather_controls(s: &mut AppState, dt: f32) {
    s.pad_state = s.pad.poll();
    // BEFORE tick, which reads it. `enabled` and `scale` are copied after
    // tick below and get away with it because tick does not use them; glide
    // it does, so copying it after would apply a changed setting a frame late.
    s.touch.glide = s.cfg.settings.touch_glide;
    s.touch.tick(dt);
    s.touch.enabled = s.cfg.settings.touch_enabled;
    s.touch.scale = s.cfg.settings.touch_scale;

    // bind capture from a controller button
    if let Some(a) = s.capturing {
        if let Some(code) = s.pad_state.any_edge() {
            s.cfg.bindings.capture(a, Bind::Pad(code));
            s.cfg.save();
            s.capturing = None;
        }
    }

    let gameplay_active = s.screen == Screen::Playing
        && !s.ui.shop_open
        && !s.controls_open
        && s.capturing.is_none()
        && (s.locked || s.cfg.settings.touch_enabled || s.pad_state.connected);
    let c = input::resolve(&s.input, &s.cfg, &s.pad_state, Some(&s.touch), gameplay_active);
    s.controls = c;
    s.input.end_frame();
    s.touch.end_frame();

    if s.screen != Screen::Playing {
        return;
    }
    if c.menu && s.capturing.is_none() {
        if s.spectator.is_some() {
            // Esc leaves the match being watched
            s.spectator = None;
            s.game = None;
            s.intro = None;
            s.screen = Screen::Menu;
            return;
        } else if s.targeting.slot.is_some() {
            // Esc puts a prepared ability away before it opens anything
            s.targeting.slot = None;
        } else if s.ui.shop_open {
            s.ui.shop_open = false;
            s.ui.sell_armed = None;
        } else if s.controls_open {
            s.controls_open = false;
            if !s.cfg.settings.touch_enabled {
                set_locked(s, true);
            }
        } else {
            // one press: free the cursor and open the panel together,
            // instead of a first press just releasing and a second one
            // actually opening it
            set_locked(s, false);
            s.controls_open = true;
        }
    }
    if c.shop {
        s.ui.shop_open = !s.ui.shop_open;
        if s.ui.shop_open {
            set_locked(s, false);
        } else {
            s.ui.sell_armed = None;
            if !s.cfg.settings.touch_enabled {
                set_locked(s, true);
            }
        }
    }
    s.scoreboard = c.scoreboard;
    if gameplay_active {
        s.cam_yaw += c.look.x;
        // a touch more headroom at both ends than before: near-level
        // when you push all the way up, a steeper overhead angle at the
        // other end, so the range doesn't feel like it stops short.
        s.cam_pitch = (s.cam_pitch + c.look.y).clamp(-0.12, 1.30);
        s.cam_dist = (s.cam_dist - c.zoom).clamp(4.5, 16.0);
        if c.recenter {
            if let Some(g) = s.game.as_ref() {
                s.cam_yaw = g.gods[0].yaw + std::f32::consts::FRAC_PI_2;
            }
        }
        if let Some(tap) = c.tap {
            s.walk_to = ground_point_from_screen(s, tap);
        }
        if c.mv.length_squared() > 0.01 {
            s.walk_to = None; // the stick takes over from a walk-to order
        }
    }
}

/// Screen point (normalized, y down, x in shorter-side units) → ground hit.
fn ground_point_from_screen(s: &AppState, p: Vec2) -> Option<Vec3> {
    let g = s.game.as_ref()?;
    let player = &g.gods[0];
    let dir = cam_dir(s.cam_yaw, s.cam_pitch);
    let target = player.pos + Vec3::Y * 1.5;
    let cam = Camera { eye: target - dir * s.cam_dist, target, fov_y: 0.95 };
    let aspect = s.renderer.width as f32 / s.renderer.height.max(1) as f32;
    let vp = cam.view_proj(aspect);
    let inv = vp.inverse();
    // p is in shorter-side units: x spans 0..extent.x, y spans 0..extent.y
    let ext = s.touch.extent;
    let ndc = Vec2::new(p.x / ext.x * 2.0 - 1.0, 1.0 - p.y / ext.y * 2.0);
    let near = inv * glam::Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
    let far = inv * glam::Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
    let a = near.truncate() / near.w;
    let b = far.truncate() / far.w;
    let d = b - a;
    if d.y.abs() < 1e-4 {
        return None;
    }
    let t = -a.y / d.y;
    if t < 0.0 {
        return None;
    }
    let hit = a + d * t;
    Some(Vec3::new(hit.x, 0.0, hit.z))
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

/// Put the world on map `m`: the active map (thread-local), the renderer's
/// terrain/water/flood meshes, and the scene -- the old scene's GPU meshes
/// freed first. A no-op when everything is already on `m`.
fn switch_map(s: &mut AppState, m: crate::map::MapId) {
    let same = crate::map::active() == m && s.renderer.world_map == m && s.scene.as_ref().map_or(true, |sc| sc.map == m);
    if same {
        return;
    }
    let t = Instant::now();
    let old = s.scene.take();
    s.scene = Some(crate::scene::switch_world(&mut s.renderer, old, m));
    eprintln!("[clawbound] switched to {} in {:.1} s ({} meshes resident)", m.name(), t.elapsed().as_secs_f32(), s.renderer.mesh_count());
}

/// Watch a match: every cat a bot, on the map picked on the select screen,
/// the intro first, then the director. Esc leaves.
fn start_spectate(s: &mut AppState) {
    s.spectator = None;
    start_game(s);
    if let Some(g) = s.game.as_mut() {
        let lane = g.gods[0].ai.as_ref().map(|b| b.lane).unwrap_or(crate::consts::Lane::Mid);
        g.gods[0].ai = Some(crate::sim::AiBrain { lane, aggression: 0.8, state: crate::sim::AiState::Push, target: None, strafe_dir: 1.0, strafe_t: 0.0, buy_t: 1.0, wp: 0 });
        g.gods[0].is_player = false;
    }
    s.spectator = Some(crate::spectator::Spectator::new());
    s.screen = Screen::Playing;
}

/// The scene fits the graphics setting: Ultra needs smooth parts built (and
/// built for the chosen fur -- the face's fur differs); switching Ultra off
/// only turns it off.
fn fit_scene_to_quality(s: &mut AppState) {
    let want = crate::quality::Quality::effective().smooth_characters();
    let fur = crate::render::FurStyle::from_env();
    s.renderer.set_fur_style(fur);
    let rebuild = s.scene.as_ref().is_some_and(|sc| want && (sc.god_smooth.is_empty() || sc.fur_built != fur));
    if rebuild {
        let old = s.scene.take().expect("scene");
        s.renderer.free_meshes_from(old.first_mesh);
        s.scene = Some(Scene::new(&mut s.renderer));
    }
    if let Some(sc) = s.scene.as_mut() {
        sc.ultra = want && !sc.god_smooth.is_empty();
    }
}

fn start_game(s: &mut AppState) {
    // The map the select screen chose -- except with a LAN lobby open,
    // where both peers must be on the same map and the picker is not synced
    // between them: that match stays on the launch map.
    let want = if s.lan.is_some() { s.launch.map } else { s.ui.map_choice };
    switch_map(s, want);
    fit_scene_to_quality(s);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(42);
    let mut rng = crate::sim::Rng::new(0xBEEF ^ seed);
    if crate::map::active() != crate::map::MapId::Jungle {
        let (blue, red) = crate::map::team_picks(s.ui.selected as u32, &mut rng);
        s.game = Some(Game::new_teams(&blue, &red));
        finish_start(s);
        return;
    }
    let mut pool: Vec<u32> = (0..crate::gods::ROSTER_LEN as u32).filter(|&i| i != s.ui.selected as u32).collect();
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
    finish_start(s);
}

/// Everything after the rosters are drawn: shared by every map.
fn finish_start(s: &mut AppState) {
    s.vfx = Vfx::default();
    s.screen = Screen::Playing;
    // static for the whole match (an ancestor's kit never changes): which
    // ability slots accept a touch drag-to-aim. Computed once here rather
    // than every frame in gather_controls so it can never race a same-frame
    // touch-down against a stale value.
    {
        let def = s.game.as_ref().unwrap().def(0);
        let caps = [def.abilities[0].quick_aimable(), def.abilities[1].quick_aimable(), def.abilities[2].quick_aimable(), def.abilities[3].quick_aimable()];
        s.touch.set_drag_capable(caps);
    }
    #[cfg(feature = "audio")]
    if s.ambience.is_none() {
        s.ambience = crate::audio::output::Ambience::start();
        #[cfg(feature = "audio")]
        if let Some(amb) = s.ambience.as_ref() {
            let root = crate::soundbank::default_root();
            eprintln!("[clawbound] audio: {} Hz, buffer {:?} frames; loading {}", amb.sample_rate, amb.buffer_frames, root.display());
            s.bank = Some(crate::soundbank::Bank::loading(crate::soundbank::Loader::start(root, amb.sample_rate)));
        }
    }
    s.ui.floaters.clear();
    s.ui.announce = None;
    s.ui.shop_open = false;
    s.cam_yaw = std::f32::consts::PI;
    s.cam_pitch = 0.38;
    s.walk_to = None;
    // The opening: every cat held out of the world until the intro brings
    // it in, and the intro ends on exactly the camera play starts with.
    if let Some(g) = s.game.as_mut() {
        for god in g.gods.iter_mut() {
            god.respawn_t = 999.0;
        }
        let teams: Vec<crate::consts::Team> = g.gods.iter().map(|x| x.team).collect();
        let p0 = g.gods[0].pos;
        let dir = cam_dir(s.cam_yaw, s.cam_pitch);
        let target = p0 + Vec3::Y * 1.5;
        let play = crate::intro::Shot { eye: target - dir * s.cam_dist, target, fov_y: 0.95 };
        // the session's first match opens with the narrated prologue
        s.intro = Some(if s.intros_played == 0 { crate::intro::Intro::full(&teams, p0, play) } else { crate::intro::Intro::new(&teams, p0, play) });
        s.intros_played += 1;
        s.intro_music = false;
        s.skip_intro = false;
    }
    s.controls_open = false;
    if !s.cfg.settings.touch_enabled {
        set_locked(s, true);
    }
}

fn render(s: &mut AppState) {
    // The sim's step is capped (a stall must not become a teleport); the
    // intro is a film, timed in wall-clock seconds. Under lavapipe at ~2 fps
    // the 0.05 cap ran it at a twentieth of speed -- seen in an Xvfb run of
    // the real app, where the pan never moved.
    let dt_wall = s.last_frame.elapsed().as_secs_f32().min(0.5);
    let dt_frame = dt_wall.min(0.05);
    s.last_frame = Instant::now();

    // Hand the world's static collision to the sim.
    //
    // Guarded rather than done at one construction site because neither order
    // is guaranteed: `Scene` is built lazily on the first frame that has a
    // renderer, `Game` is built when a match starts, and either can come
    // first. The `is_empty` check makes this idempotent, so it costs one
    // pointer comparison per frame after the handover.
    if let (Some(sc), Some(g)) = (s.scene.as_ref(), s.game.as_mut()) {
        if g.blockers.is_empty() && !sc.blockers.is_empty() {
            g.blockers = sc.blockers.clone();
            eprintln!("[clawbound] sim took {} prop blockers", g.blockers.len());
        }
    }

    if s.scene.is_none() {
        s.scene = Some(Scene::new(&mut s.renderer));
        render_portraits(s);
        if s.launch.quick_match && s.screen == Screen::Menu {
            // --quick-match (bsgames): straight into a 5v5 with the named or a random ancestor
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1);
            s.ui.selected = s.launch.ancestor_id().map(|i| i as usize).unwrap_or((seed % crate::gods::ROSTER_LEN as u64) as usize);
            eprintln!("[clawbound] quick match as {}", crate::gods::god(s.ui.selected as u32).name);
            start_game(s);
        }
    }
    // input: one resolve per frame, then the fixed-step loop applies it
    gather_controls(s, dt_frame);
    let scene = s.scene.as_ref().unwrap();

    // fixed-step simulation + player input
    if s.screen == Screen::Playing {
        let c = s.controls;
        let aim = if s.game.is_some() { compute_aim(s) } else { Vec3::ZERO };
        let yaw = s.cam_yaw;
        let walk_to = s.walk_to;
        if c.sneak_toggle {
            s.stalk_on = !s.stalk_on;
        }
        let stalk_on = s.stalk_on;
        let attack_eaten = s.attack_eaten || s.targeting.eats_attack();
        if let Some(g) = s.game.as_mut() {
            let spectating = s.spectator.is_some();
            if !spectating {
            // A leap goes where the cat is being steered; standing still,
            // where the camera looks. `leap::plan` turns that direction into
            // a place -- the rock ahead, if there is one.
            if c.leap {
                let (fwd, right) = input::camera_basis(yaw);
                let dir = if c.mv.length_squared() > 0.04 { fwd * c.mv.y + right * c.mv.x } else { fwd };
                g.leap_queue.push((0, dir));
            }
            // Casts: an ability key PREPARES the ability (its guide shows);
            // the attack button fires it at the current aim (`targeting`).
            // A touch drag-to-aim is its own two-step gesture and fires on
            // release, as before.
            // the facing cone couples the cat to where the camera looks
            g.gods[0].face_ref = input::camera_basis(yaw).0;
            let attack_edge = c.attack && !s.attack_was;
            let keys: [bool; 4] = std::array::from_fn(|k| c.cast[k] && c.cast_dir[k].is_none());
            let out = s.targeting.step(crate::targeting::Press { cast: keys, attack: attack_edge, cancel: false });
            if let crate::targeting::Outcome::Fire(slot) = out {
                g.cast_queue.push((0, slot, aim));
                s.attack_eaten = true;
            }
            if s.targeting.eats_attack() && c.attack {
                s.attack_eaten = true;
            }
            if !c.attack {
                s.attack_eaten = false;
            }
            s.attack_was = c.attack;
            for slot in 0..4u32 {
                if c.cast[slot as usize] && c.cast_dir[slot as usize].is_some() {
                    let cast_aim = if let Some(dir) = c.cast_dir[slot as usize] {
                        // a committed touch drag-to-aim: rotate its screen-
                        // space direction (x right, y up) onto the camera's
                        // own basis, same convention movement uses, so a
                        // drag toward the top of the screen aims the way
                        // the camera is actually looking, not a fixed axis
                        let (fwd, right) = input::camera_basis(yaw);
                        let world_dir = fwd * dir.y + right * dir.x;
                        g.gods[0].pos + world_dir * 20.0
                    } else {
                        aim
                    };
                    g.cast_queue.push((0, slot, cast_aim));
                }
            }
            } // !spectating
            // The intro runs instead of the sim: cats appear on its beats,
            // the fanfare starts on the first spawn, and play begins when
            // it ends or is skipped.
            if let Some(it) = s.intro.as_mut() {
                let before = it.t;
                let arrived = if s.skip_intro { it.skip() } else { it.advance(dt_wall) };
                let bursts = !s.skip_intro;
                for gi in arrived {
                    g.gods[gi].respawn_t = 0.0;
                    if bursts {
                        let c = g.gods[gi].team.rgb();
                        s.vfx.star_burst(g.gods[gi].pos, c, gi as f32);
                    }
                }
                // The intro's score and narration (studio files, if loaded):
                // `music/intro_full` for the narrated intro, `music/intro` for
                // the short one, from its first frame; the narrator's lines
                // on their schedule. A skip silences both.
                #[cfg(feature = "audio")]
                if let (Some(amb), Some(bank)) = (s.ambience.as_mut(), s.bank.as_mut()) {
                    use crate::mixer::{Bus, Cmd, Play};
                    if before == 0.0 {
                        let key = if it.is_full() { "music/intro_full" } else { "music/intro" };
                        if let Some(id) = bank.pick(key) {
                            s.intro_music = amb.mixer.play(Play { clip: id, bus: Bus::Music, gain: 1.0, pan: 0.0, priority: 10, looped: false, speaker: None });
                        }
                    }
                    for key in it.narration_due(before, it.t) {
                        if let Some(id) = bank.pick(&format!("narrator/{key}")) {
                            amb.mixer.play(Play { clip: id, bus: Bus::Voice, gain: 1.0, pan: 0.0, priority: 10, looped: false, speaker: Some(u16::MAX - 1) });
                        }
                    }
                    if s.skip_intro {
                        amb.mixer.send(Cmd::StopBus(Bus::Music));
                        amb.mixer.send(Cmd::StopBus(Bus::Voice));
                        s.intro_music = false;
                    }
                }
                if before < it.fanfare_at() && it.t >= it.fanfare_at() && !s.skip_intro && !s.intro_music {
                    // a studio fanfare, if one is loaded; else the synthesized one
                    let mut played = false;
                    #[cfg(feature = "audio")]
                    if let (Some(amb), Some(bank)) = (s.ambience.as_mut(), s.bank.as_mut()) {
                        if let Some(id) = bank.pick("music/fanfare") {
                            played = amb.mixer.play(crate::mixer::Play { clip: id, bus: crate::mixer::Bus::Music, gain: 1.0, pan: 0.0, priority: 10, looped: false, speaker: None });
                        }
                    }
                    if !played {
                        s.fanfare_count = s.fanfare_count.wrapping_add(1);
                    }
                }
                s.vfx.update(dt_frame, &[], g);
                if it.done {
                    s.announcer = Default::default();
                    s.announcer.start(g.t);
                    s.intro = None;
                    s.skip_intro = false;
                    s.accumulator = 0.0;
                }
            }
            s.accumulator = (s.accumulator + dt_frame).min(0.25);
            while s.intro.is_none() && s.accumulator >= 1.0 / 60.0 {
                s.accumulator -= 1.0 / 60.0;
                // camera_basis (input.rs) is verified against the real view
                // matrix; hand-rolling this vector here is exactly how it
                // was backwards before (D/right and A/left were swapped).
                let (fwd, right) = crate::input::camera_basis(yaw);
                let mut mv = fwd * c.mv.y + right * c.mv.x;
                if let Some(t) = walk_to {
                    let d = Vec3::new(t.x - g.gods[0].pos.x, 0.0, t.z - g.gods[0].pos.z);
                    if d.length() > 0.8 {
                        mv = d.normalize();
                    } else {
                        s.walk_to = None;
                    }
                }
                // A held stalk, or a stick or touch pad pushed gently: on an
                // analogue input the magnitude IS the intent, and the sim is
                // about to throw it away by normalising.
                let gentle = walk_to.is_none() && c.mv.length() > 0.08 && c.mv.length() < 0.5;
                // a spectated match's god 0 is a bot like the rest
                if s.spectator.is_none() {
                    let gi = &mut g.gods[0];
                    gi.sneak = stalk_on || c.sneak_held || gentle;
                    gi.move_dir = mv;
                    // the click that fired a prepared ability is not an attack
                    gi.attack_held = c.attack && !attack_eaten;
                }
                g.tick(1.0 / 60.0);
                let events: Vec<crate::sim::Event> = g.events.drain(..).collect();
                s.audio_events.extend(events.iter().cloned());
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
        Screen::Menu | Screen::Select | Screen::Lan => {
            let t = s.last_frame.elapsed().as_secs_f32() * 0.05;
            let r = 55.0;
            Camera {
                eye: Vec3::new(t.cos() * r, 32.0, t.sin() * r),
                target: Vec3::ZERO,
                fov_y: 0.9,
            }
        }
        Screen::Playing => {
            let g = s.game.as_ref().unwrap();
            let player = &g.gods[0];
            let dir = cam_dir(s.cam_yaw, s.cam_pitch);
            // the ground moves: an earthquake (or a nearby boar slam) jolts the camera
            let shake = g.disasters.shake;
            let tt = s.last_frame.elapsed().as_secs_f32() * 60.0 + g.t * 47.0;
            let jolt = Vec3::new((tt * 1.7).sin(), (tt * 2.3).cos() * 0.6, (tt * 1.1).sin()) * shake * 0.35;
            let target = player.pos + Vec3::Y * 1.5 + jolt;
            match s.intro.as_ref() {
                Some(it) => {
                    let f = it.frame();
                    Camera { eye: f.shot.eye, target: f.shot.target, fov_y: f.shot.fov_y }
                }
                None if s.spectator.is_some() => {
                    let inp = crate::spectator::Input {
                        cycle: std::mem::take(&mut s.spec_cycle),
                        director: std::mem::take(&mut s.spec_director),
                        free: std::mem::take(&mut s.spec_free),
                        mv: s.controls.mv,
                        rise: -s.controls.zoom,
                        look: glam::Vec2::new(s.controls.look.x, -s.controls.look.y),
                    };
                    let g = s.game.as_ref().unwrap();
                    let shot = s.spectator.as_mut().unwrap().update(g, inp, dt_frame);
                    Camera { eye: shot.eye, target: shot.target, fov_y: shot.fov_y }
                }
                None => Camera {
                    eye: target - dir * s.cam_dist + jolt * 0.5,
                    target,
                    fov_y: 0.95,
                },
            }
        }
    };
    let vp = cam.view_proj(s.renderer.width as f32 / s.renderer.height as f32);

    // the player's settings, applied: quality and fur (a command-line or
    // environment override still wins), volumes when they change
    {
        let st = &s.cfg.settings;
        crate::quality::Quality::set_chosen(st.quality);
        crate::render::FurStyle::set_chosen(st.fur);
        s.renderer.quality = crate::quality::Quality::effective();
        // vsync follows the setting: the surface is reconfigured on change
        let want = present_mode_for(s.cfg.settings.vsync, &s.present_modes);
        if want != s.present {
            s.present = want;
            s.surface.configure(
                &s.renderer.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: s.surface_format,
                    width: s.renderer.width.max(1),
                    height: s.renderer.height.max(1),
                    present_mode: want,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                },
            );
            eprintln!("[clawbound] present mode -> {want:?}");
        }
        // the shadow map follows the quality (rebuilt only when it changes)
        let size = s.renderer.quality.shadow_size();
        s.renderer.set_shadow_size(size);
        let vols = [st.vol_master, st.vol_music, st.vol_sfx, st.vol_voice];
        #[cfg(feature = "audio")]
        if vols != s.vol_sent {
            if let Some(amb) = s.ambience.as_mut() {
                use crate::mixer::{Bus, Cmd};
                // the mixer's own balance, times the player's volumes
                amb.mixer.send(Cmd::BusGain(Bus::Music, 0.55 * vols[1] * vols[0]));
                amb.mixer.send(Cmd::BusGain(Bus::Sfx, 0.9 * vols[2] * vols[0]));
                amb.mixer.send(Cmd::BusGain(Bus::Voice, 1.0 * vols[3] * vols[0]));
                s.vol_sent = vols;
            }
        }
        let _ = vols;
        s.frame_s += (dt_frame.clamp(0.0, 0.5) - s.frame_s) * 0.08;
    }

    // ambience: the listener is the player; the wind pans with the camera
    // Out of a match -- the menu, the select screen, after a match -- the
    // ambience fades to silence (`audio::fade_out`) instead of holding the
    // last match's wind.
    #[cfg(feature = "audio")]
    if let Some(amb) = s.ambience.as_ref() {
        let p = match s.game.as_ref() {
            Some(g) if s.screen == Screen::Playing => {
                let mut p = crate::audio::params_for(g, g.gods[0].pos, s.cam_yaw, s.thunder_count, s.thunder_dist);
                p.fanfare_count = s.fanfare_count;
                p
            }
            _ => crate::audio::fade_out(s.audio_last, dt_frame),
        };
        // the master volume scales the synthesized ambience too
        let mut p = p;
        s.audio_last = p;
        p.master *= s.cfg.settings.vol_master;
        amb.set(p);
    }

    // Loaded audio: take what the loader has finished, and play this frame's
    // cues -- placed round the listener (the player's cat, or where the
    // spectator looks), voice lines gated per cat. Music loops per map.
    #[cfg(feature = "audio")]
    if let (Some(amb), Some(bank)) = (s.ambience.as_mut(), s.bank.as_mut()) {
        use crate::mixer::{Bus, Cmd, Play};
        bank.poll(&mut amb.mixer);
        match s.game.as_ref() {
            Some(g) if s.screen == Screen::Playing => {
                // the map's music waits for the intro (and its score) to end
                if !s.music_on && s.intro.is_none() {
                    if let Some(id) = bank.pick(&format!("music/{}", crate::map::active().key())) {
                        amb.mixer.play(Play { clip: id, bus: Bus::Music, gain: 0.8, pan: 0.0, priority: 9, looped: true, speaker: None });
                    }
                    s.music_on = true;
                }
                let f = cam.target - cam.eye;
                let right = Vec3::new(f.x, 0.0, f.z).normalize_or_zero().cross(Vec3::Y).normalize_or_zero();
                let ear = if s.spectator.is_some() { cam.target } else { g.gods[0].pos };
                // the space the listener is in: reverb and echo, a few times
                // a second (it glides between places on its own)
                s.space_tick = s.space_tick.wrapping_add(1);
                if s.space_tick % 6 == 0 {
                    amb.mixer.send(Cmd::Space(crate::reverb::space_at(ear)));
                }
                if let Some(w) = g.over {
                    let me = if s.spectator.is_some() { None } else { Some(g.gods[0].team) };
                    s.announcer.end(w, me, g.t);
                }
                for ev in s.audio_events.iter() {
                    s.announcer.hear(ev, g);
                }
                if let Some(call) = s.announcer.next(g.t) {
                    let key = format!("announcer/{}", call.key);
                    let secs = match bank.pick(&key) {
                        Some(id) => {
                            amb.mixer.play(Play { clip: id, bus: Bus::Voice, gain: 1.0, pan: 0.0, priority: 10, looped: false, speaker: Some(u16::MAX) });
                            bank.secs(id, amb.sample_rate)
                        }
                        None => 1.2, // unrecorded: keep the pacing anyway
                    };
                    s.announcer.playing(g.t, secs);
                }
                for ev in s.audio_events.drain(..) {
                    for cue in crate::cues::cues_for(&ev, g) {
                        let Some((gain, pan)) = crate::cues::place(&cue, ear, right) else { continue };
                        if !s.voice_gate.allow(&cue, g.t) {
                            continue;
                        }
                        if let Some(id) = bank.first(&cue.keys) {
                            amb.mixer.play(Play { clip: id, bus: cue.bus, gain, pan, priority: cue.priority, looped: false, speaker: cue.speaker.map(|x| x as u16) });
                        }
                    }
                }
            }
            _ => {
                s.audio_events.clear();
                if s.music_on {
                    amb.mixer.send(Cmd::StopBus(Bus::Music));
                    amb.mixer.send(Cmd::StopBus(Bus::Voice));
                    s.music_on = false;
                }
            }
        }
    }

    // world draws (+ this frame's sky state)
    if let Some(g) = s.game.as_ref() {
        s.renderer.atmosphere = g.weather.atmosphere();
        // catastrophe uniforms: flood rise, shake, fire glow near the player
        let near_fire = g.disasters.fire.iter().map(|f| f.pos.distance(g.gods[0].pos)).fold(f32::MAX, f32::min);
        let glow = if near_fire < 40.0 { (1.0 - near_fire / 40.0).clamp(0.0, 1.0) } else { 0.0 };
        s.renderer.event = (g.disasters.flood_rise, g.disasters.shake, glow);
        let mut vfx = std::mem::take(&mut s.vfx);
        scene.draw(&mut s.renderer, g, &mut vfx, cam);
        s.vfx = vfx;
        // the prepared ability's guide, where the camera aims
        if let Some(slot) = s.targeting.slot {
            if s.screen == Screen::Playing && g.gods[0].respawn_t <= 0.0 {
                let ab = g.def(0).abilities[slot as usize];
                let guide = crate::targeting::guide_for(&ab, g.gods[0].pos, compute_aim(s));
                scene.draw_guide(&mut s.renderer, &crate::targeting::guide_points(&guide, 1.1), ab.color);
            }
        }
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
                    present_mode: s.present,
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
    let mut open_lan = false;
    let mut spectate = false;
    let mut lan_action = crate::lanlobby::Action::None;
    let mut locked_in = false;
    let mut again = false;
    let mut menu = false;
    let mut open_controls = false;
    let mut panel = ui::ControlsPanel { close: false, capture: None, clear: None, scheme: None, reset: false };
    let (show_fps, frame_s) = (s.cfg.settings.show_fps, s.frame_s);
    let output = s.egui_ctx.run(raw_input, |ctx| {
        if show_fps {
            ui::draw_fps(ctx, 1.0 / frame_s.max(1e-4), frame_s * 1000.0);
        }
        match s.screen {
            Screen::Menu => {
                ui::draw_menu(ctx, &mut play, &mut quit, &mut open_lan, &mut spectate);
                ui::draw_controls_button(ctx, &mut open_controls);
            }
            Screen::Select => ui::draw_select(ctx, &mut s.ui, &mut locked_in),
            Screen::Lan => {
                if let Some(l) = s.lan.as_mut() {
                    l.tick(Instant::now());
                    lan_action = ui::draw_lan(ctx, l);
                }
            }
            Screen::Playing => {
                if let Some(g) = s.game.as_mut() {
                    if let Some(it) = s.intro.as_ref() {
                        ui::draw_intro_overlay(ctx, &it.frame().overlay);
                    } else if let Some(sp) = s.spectator.as_ref() {
                        ui::draw_spectator_hud(ctx, g, vp, &sp.label(g));
                    } else if s.ui.shop_open {
                        ui::draw_shop(ctx, g, 0, &mut s.ui);
                    } else {
                        ui::draw_hud(ctx, g, 0, vp, s.renderer.width as f32, s.renderer.height as f32, &mut s.ui);
                        if s.cfg.settings.touch_enabled {
                            ui::draw_touch_pad(ctx, &s.touch, g, s.renderer.width as f32, s.renderer.height as f32);
                        } else {
                            ui::draw_crosshair(ctx, s.locked, s.renderer.width as f32, s.renderer.height as f32);
                            if !s.locked && !s.controls_open {
                                ui::draw_click_to_play(ctx);
                            }
                        }
                    }
                    if s.scoreboard {
                        ui::draw_scoreboard_labelled(ctx, g, s.ui.player_label.as_deref());
                    }
                    if g.over.is_some() {
                        ui::draw_end(ctx, g, &mut again, &mut menu);
                    }
                }
            }
        }
        if s.controls_open {
            ui::draw_controls(ctx, &mut s.cfg, s.capturing, s.pad_state.connected, &mut panel);
        }
    });
    if open_controls {
        s.controls_open = true;
    }
    if panel.close {
        s.controls_open = false;
        s.capturing = None;
        if s.screen == Screen::Playing && !s.cfg.settings.touch_enabled {
            set_locked(s, true);
        }
    }
    if let Some(a) = panel.capture {
        s.capturing = Some(a);
    }
    if let Some(a) = panel.clear {
        s.cfg.bindings.clear(a);
        s.cfg.save();
    }
    if let Some(sc) = panel.scheme {
        s.cfg.bindings = input::Bindings::for_scheme(sc);
        s.cfg.save();
    }
    if panel.reset {
        s.cfg = Config::default();
        s.cfg.save();
    }
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
    if open_lan {
        s.lan = Some(crate::lanlobby::LanLobby::open(&std::env::var("USER").unwrap_or_else(|_| "Cat".into())));
        s.screen = Screen::Lan;
    }
    match lan_action {
        crate::lanlobby::Action::Back => {
            s.lan = None; // closes the socket and stops announcing
            s.screen = Screen::Menu;
        }
        crate::lanlobby::Action::Join(addr) => {
            // Discovery ends here. There is no synchronised match to enter
            // yet, and the screen's footer says so; do not fake one.
            // Joins the den's roster for real. It still does not start a
            // synchronised match -- there is none -- and the footer says so.
            if let Some(l) = s.lan.as_mut() {
                l.join(addr);
            }
        }
        crate::lanlobby::Action::None => {}
    }
    if spectate {
        start_spectate(s);
    }
    if play {
        s.screen = Screen::Select;
    }
    if quit {
        s.exiting = true;
    }
    if locked_in || again {
        // the chosen ancestor answers the lock-in (voicescript: "select")
        #[cfg(feature = "audio")]
        if locked_in {
            if let (Some(amb), Some(bank)) = (s.ambience.as_mut(), s.bank.as_mut()) {
                let def = &crate::gods::ROSTER[(s.ui.selected as usize).min(crate::gods::ROSTER.len() - 1)];
                let who = def.name.to_ascii_lowercase().replace([' ', '\''], "");
                if let Some(id) = bank.pick(&format!("voice/{who}/select")) {
                    amb.mixer.play(crate::mixer::Play { clip: id, bus: crate::mixer::Bus::Voice, gain: 1.0, pan: 0.0, priority: 9, looped: false, speaker: None });
                }
            }
        }
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

/// The present mode for the vsync setting: on, Fifo (always offered); off,
/// uncapped -- Immediate if the surface offers it (Metal does), else
/// Mailbox, else Fifo after all.
pub fn present_mode_for(vsync: bool, offered: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    use wgpu::PresentMode::*;
    if vsync {
        return Fifo;
    }
    [Immediate, Mailbox].into_iter().find(|m| offered.contains(m)).unwrap_or(Fifo)
}

#[cfg(test)]
mod present_tests {
    use super::present_mode_for;
    use wgpu::PresentMode::*;

    #[test]
    fn vsync_off_uncaps_where_the_surface_allows() {
        assert_eq!(present_mode_for(true, &[Fifo, Immediate]), Fifo);
        assert_eq!(present_mode_for(false, &[Fifo, Immediate, Mailbox]), Immediate);
        assert_eq!(present_mode_for(false, &[Fifo, Mailbox]), Mailbox);
        assert_eq!(present_mode_for(false, &[Fifo]), Fifo, "nothing better offered");
    }
}
