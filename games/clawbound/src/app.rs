//! app.rs — the playable window. A winit + wgpu **surface** render loop that
//! reuses the exact scene geometry (`crate::build_scene`) and shaders
//! (`crate::SCENE_WGSL` / `crate::POST_WGSL`) as the headless renderer. Compiles
//! for native (winit window) and for the web (wasm32 → WebGPU/WebGL canvas).
//!
//! Native:  cargo run --features app -- --play
//! Web:     rustup target add wasm32-unknown-unknown && trunk serve --features web

use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::{event::*, event_loop::EventLoop, window::Window, window::WindowBuilder};
use winit::keyboard::{KeyCode, PhysicalKey};

use clawbound_rs::math::{look_at, perspective, v3};
use clawbound_rs::sim::Game;

/// A scripted first-pass tutorial for Prowl: a short non-interactive intro pan,
/// then a sequence of steps that each show a prompt and unlock the next once the
/// player actually does the thing — a real (if modest) interactive walkthrough
/// rather than a wall of text.
struct TutorialRun {
    step: usize,
    intro_t: f32,      // > 0 during the opening camera pan, before the player gets control
    advance_flash: f32,
}
const TUTORIAL_STEPS: &[(&str, &str)] = &[
    ("Find your paws", "Move with your bound movement keys (WASD by default — rebind anytime in Settings > Keybinds)."),
    ("Slip out of sight", "Hold your Hide/Climb key near a bush to vanish from view."),
    ("Claim a tree", "Walk up to an ancestor tree and stay close until it lights up for your clan."),
    ("Strike", "Flick the mouse and left-click to land an autoattack."),
    ("Your ultimate", "Press your Ultimate key (Q by default) to unleash your class's signature move."),
    ("Raid the jungle", "Sneak past — or fight through — a jungle camp's guard to take its herb."),
    ("Hold the ground", "Stand at a territory marker until it flips to your clan — this is what the Rock of Nine Lives ritual is built from."),
    ("You're ready", "That's the core of Prowl. Press Enter to head back to the menu."),
];
fn tutorial_step_done(step: usize, p: &clawbound_rs::prowl::Prowl) -> bool {
    match step {
        0 => p.moving,
        1 => p.hidden,
        2 => p.ancestors.iter().any(|a| a.owned_by(0)),
        3 => p.combo_last.is_some() || p.times_caught > 0,
        4 => p.ability_cd > 0.0 || p.ability_t > 0.0,
        5 => p.jungle.iter().any(|c| c.cleared() || c.cooldown > 0.0),
        6 => p.markers.iter().any(|m| m.held_by == 0),
        _ => false,   // the final step waits for Enter, handled separately
    }
}

/// Small vector icons for the buff badges — a paw print, a shield, three claw
/// marks — drawn with egui shapes so the HUD reads at a glance instead of just
/// text, without needing an image-asset pipeline.
fn icon_paw(ui: &mut egui::Ui, color: egui::Color32) {
    use egui::{epaint::CircleShape, pos2, Shape};
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    p.add(Shape::Circle(CircleShape::filled(pos2(c.x, c.y + 3.0), 5.0, color)));
    for (dx, dy, r) in [(-5.5, -3.0, 2.6), (-1.8, -5.5, 2.6), (1.8, -5.5, 2.6), (5.5, -3.0, 2.6)] {
        p.add(Shape::Circle(CircleShape::filled(pos2(c.x + dx, c.y + dy), r * 0.62, color)));
    }
}
fn icon_pelt(ui: &mut egui::Ui, color: egui::Color32) {
    use egui::{epaint::PathShape, pos2, Shape};
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    let pts = vec![
        pos2(c.x, c.y - 7.0), pos2(c.x + 6.0, c.y - 4.5), pos2(c.x + 6.0, c.y + 1.5),
        pos2(c.x, c.y + 7.0), pos2(c.x - 6.0, c.y + 1.5), pos2(c.x - 6.0, c.y - 4.5),
    ];
    ui.painter().add(Shape::Path(PathShape::convex_polygon(pts, color, egui::Stroke::NONE)));
}
fn icon_claw(ui: &mut egui::Ui, color: egui::Color32) {
    use egui::{pos2, Shape, Stroke};
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    for dx in [-4.0f32, 0.0, 4.0] {
        ui.painter().add(Shape::line_segment([pos2(c.x + dx - 2.0, c.y - 6.0), pos2(c.x + dx + 2.0, c.y + 6.0)], Stroke::new(2.2, color)));
    }
}
/// Draw the right icon for a `Power` buff badge.
fn icon_power(ui: &mut egui::Ui, power: clawbound_rs::powerup::Power, color: egui::Color32) {
    use clawbound_rs::powerup::Power;
    match power { Power::Speed => icon_paw(ui, color), Power::Shield => icon_pelt(ui, color), Power::Triple => icon_claw(ui, color) }
}

/// Small vector icons for the 6 classes' passives and ultimates. Same idea as the buff
/// badges — simple egui shapes standing in for icon art.
fn icon_passive(ui: &mut egui::Ui, class: clawbound_rs::class::Class, color: egui::Color32) {
    use clawbound_rs::class::Class;
    use egui::{epaint::CircleShape, pos2, Shape, Stroke};
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    match class {
        Class::Cat => {
            // Nine Lives — a small sparkle/star
            for a in 0..4 {
                let ang = a as f32 * std::f32::consts::FRAC_PI_2 + 0.2;
                let (dx, dy) = (ang.cos() * 6.0, ang.sin() * 6.0);
                p.add(Shape::line_segment([c, pos2(c.x + dx, c.y + dy)], Stroke::new(1.8, color)));
            }
        }
        Class::Squirrel => {
            // Cache — an acorn: a round cap over a pointed body
            p.add(Shape::Circle(CircleShape::filled(pos2(c.x, c.y - 2.0), 4.5, color)));
            p.add(Shape::convex_polygon(vec![pos2(c.x - 4.0, c.y + 1.0), pos2(c.x + 4.0, c.y + 1.0), pos2(c.x, c.y + 7.0)], color, Stroke::NONE));
        }
        Class::Otter => {
            // Slipstream — water ripples
            for (dy, r) in [(-3.0, 6.5), (2.0, 5.0), (6.0, 3.2)] {
                p.add(Shape::Path(egui::epaint::PathShape::line(
                    (0..=8).map(|i| { let t = i as f32 / 8.0; pos2(c.x - r + t * 2.0 * r, c.y + dy + (t * std::f32::consts::PI).sin() * 1.6) }).collect(),
                    Stroke::new(1.6, color))));
            }
        }
        Class::Frog => {
            // Camouflage — a leaf outline
            let pts: Vec<_> = (0..=12).map(|i| {
                let t = i as f32 / 12.0 * std::f32::consts::PI;
                pos2(c.x + (t.cos()) * 6.0, c.y - 6.0 + t.sin() * 7.0)
            }).collect();
            p.add(Shape::Path(egui::epaint::PathShape::closed_line(pts, Stroke::new(1.6, color))));
        }
        Class::Goat => {
            // Sure-Footed — a small mountain peak
            p.add(Shape::convex_polygon(vec![pos2(c.x - 7.0, c.y + 6.0), pos2(c.x, c.y - 6.0), pos2(c.x + 7.0, c.y + 6.0)], color, Stroke::NONE));
        }
        Class::Hawk => {
            // Keen Eyes — an eye
            p.add(Shape::Path(egui::epaint::PathShape::closed_line(
                vec![pos2(c.x - 7.0, c.y), pos2(c.x, c.y - 4.5), pos2(c.x + 7.0, c.y), pos2(c.x, c.y + 4.5)], Stroke::new(1.6, color))));
            p.add(Shape::Circle(CircleShape::filled(c, 2.0, color)));
        }
    }
}
fn icon_ultimate(ui: &mut egui::Ui, class: clawbound_rs::class::Class, color: egui::Color32) {
    use clawbound_rs::class::Class;
    use egui::{pos2, Shape, Stroke};
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    match class {
        Class::Cat => {
            // Second Wind — a swirling breath
            let pts: Vec<_> = (0..=16).map(|i| {
                let t = i as f32 / 16.0 * std::f32::consts::PI * 1.6;
                pos2(c.x + t.cos() * (2.0 + t * 1.3), c.y + t.sin() * (2.0 + t * 1.3))
            }).collect();
            p.add(Shape::Path(egui::epaint::PathShape::line(pts, Stroke::new(1.6, color))));
        }
        Class::Squirrel => {
            // Scamper — motion streaks
            for dx in [-4.0f32, 0.0, 4.0] {
                p.add(Shape::line_segment([pos2(c.x + dx - 5.0, c.y - 2.0), pos2(c.x + dx + 3.0, c.y - 2.0)], Stroke::new(1.8, color)));
            }
        }
        Class::Otter => {
            // Dive — a droplet
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y - 7.0), pos2(c.x + 5.0, c.y + 3.0), pos2(c.x - 5.0, c.y + 3.0)], color, Stroke::NONE));
            p.add(Shape::Circle(egui::epaint::CircleShape::filled(pos2(c.x, c.y + 4.0), 3.5, color)));
        }
        Class::Frog => {
            // Bound — an arcing leap trajectory
            let pts: Vec<_> = (0..=10).map(|i| { let t = i as f32 / 10.0; pos2(c.x - 6.0 + t * 12.0, c.y + 5.0 - (t * std::f32::consts::PI).sin() * 9.0) }).collect();
            p.add(Shape::Path(egui::epaint::PathShape::line(pts, Stroke::new(1.8, color))));
        }
        Class::Goat => {
            // Charge — a forward chevron burst
            for off in [0.0f32, 5.0] {
                p.add(Shape::line_segment([pos2(c.x - 5.0 + off, c.y - 5.0), pos2(c.x + 2.0 + off, c.y)], Stroke::new(1.8, color)));
                p.add(Shape::line_segment([pos2(c.x - 5.0 + off, c.y + 5.0), pos2(c.x + 2.0 + off, c.y)], Stroke::new(1.8, color)));
            }
        }
        Class::Hawk => {
            // Stoop — a sharp diving arrow
            p.add(Shape::line_segment([pos2(c.x - 5.0, c.y - 5.0), pos2(c.x + 5.0, c.y + 5.0)], Stroke::new(2.0, color)));
            p.add(Shape::line_segment([pos2(c.x + 5.0, c.y + 5.0), pos2(c.x - 1.0, c.y + 5.0)], Stroke::new(2.0, color)));
            p.add(Shape::line_segment([pos2(c.x + 5.0, c.y + 5.0), pos2(c.x + 5.0, c.y - 1.0)], Stroke::new(2.0, color)));
        }
    }
}

#[derive(Default)]
struct Keys { w: bool, a: bool, s: bool, d: bool, crouch: bool, restart: bool, dodge: bool, leap: bool, ability: bool, mleft: bool, mmiddle: bool, mright: bool }
impl Keys {
    /// The canonical bind-name for a physical key — matches what `Menu::feed_rebind`
    /// stores when the player rebinds an action, so a stored name here always
    /// resolves back to the same physical key.
    fn keycode_name(code: KeyCode) -> String { format!("{:?}", code).replace("Key", "").replace("Digit", "") }

    /// Set the right field(s) for a physical key press/release, per the *current*
    /// keybinds — not hardcoded, so remapping (including to non-QWERTY-adjacent keys,
    /// PageUp/PageDown, etc.) actually takes effect. Arrow keys always work for
    /// movement too, regardless of binding, as a baseline that's never taken away.
    fn set(&mut self, code: KeyCode, down: bool, kb: &clawbound_rs::menu::Keybinds) {
        use clawbound_rs::menu::Action;
        match code {
            KeyCode::ArrowUp => self.w = down,
            KeyCode::ArrowDown => self.s = down,
            KeyCode::ArrowLeft => self.a = down,
            KeyCode::ArrowRight => self.d = down,
            _ => {}
        }
        let name = Self::keycode_name(code);
        if name == kb.key_for(Action::Up) { self.w = down; }
        if name == kb.key_for(Action::Down) { self.s = down; }
        if name == kb.key_for(Action::Left) { self.a = down; }
        if name == kb.key_for(Action::Right) { self.d = down; }
        if name == kb.key_for(Action::Crouch) { self.crouch = down; }
        if name == kb.key_for(Action::Dodge) { self.dodge = down; }
        if name == kb.key_for(Action::Leap) { self.leap = down; }
        if name == kb.key_for(Action::Ability) { self.ability = down; }
        if name == kb.key_for(Action::Restart) { self.restart = down; }
        // Shift/Ctrl and C are always accepted for Crouch too, so a rebind to
        // something else doesn't strand a player who instinctively reaches for Shift.
        if matches!(code, KeyCode::ShiftLeft | KeyCode::ControlLeft) { self.crouch = down; }
    }
    fn move_vec(&self) -> (f32, f32) {
        ((self.d as i32 - self.a as i32) as f32, (self.w as i32 - self.s as i32) as f32)
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct U { mvp: [f32; 16], light: [f32; 4], key: [f32; 4], amb: [f32; 4] }

/// Ground-plane camera basis from yaw: (forward, right). `forward` is where the camera
/// looks along the ground; `right` is screen-right. Derived from the right-handed
/// look_at (screen-right = forward × up), so at yaw 0 forward=+z and right=−x.
fn cam_forward_right(yaw: f32) -> ((f32, f32), (f32, f32)) {
    let (s, c) = (yaw.sin(), yaw.cos());
    ((s, c), (-c, s))
}
/// Rotate a raw WASD vector (ix=right, iz=forward) into world space for the camera yaw.
fn move_world(yaw: f32, ix: f32, iz: f32) -> (f32, f32) {
    let (f, r) = cam_forward_right(yaw);
    (ix * r.0 + iz * f.0, ix * r.1 + iz * f.1)
}

/// Draw the pause/title menu overlay (shared by the sim and prowl render paths).
fn draw_menu(ctx: &egui::Context, items: &[String], cursor: usize, screen: clawbound_rs::menu::Screen) {
    use clawbound_rs::menu::Screen;
    use egui::{Align2, Color32, RichText};
    egui::Area::new("clawbound_menu".into()).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(Color32::from_rgba_unmultiplied(18, 14, 26, 235)).inner_margin(egui::Margin::same(24.0)).show(ui, |ui| {
            ui.set_min_width(320.0);
            ui.label(RichText::new("C L A W B O U N D").size(30.0).strong().color(Color32::from_rgb(230, 210, 170)));
            let sub = match screen { Screen::Title => "", Screen::Play => "Single Player", Screen::Settings => "Settings", Screen::Keybinds => "Keybinds" };
            if !sub.is_empty() { ui.label(RichText::new(sub).size(14.0).color(Color32::from_rgb(150, 130, 180))); }
            ui.add_space(10.0);
            for (i, it) in items.iter().enumerate() {
                let sel = i == cursor;
                ui.label(RichText::new(format!("{} {}", if sel { "▶" } else { "  " }, it)).size(20.0)
                    .color(if sel { Color32::from_rgb(255, 220, 120) } else { Color32::from_rgb(205, 205, 210) }));
            }
            ui.add_space(10.0);
            ui.label(RichText::new("↑↓ move   ←→ change   Enter select   Esc back").size(12.0).weak());
        });
    });
}

struct State {
    _window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    scene_pipe: wgpu::RenderPipeline,
    post_pipe: wgpu::RenderPipeline,
    ubuf: wgpu::Buffer,
    ubg: wgpu::BindGroup,
    world_vbuf: wgpu::Buffer,
    world_vcount: u32,
    ent_vbuf: wgpu::Buffer,
    ent_vcount: u32,
    ent_cap: u64,
    game: Game,
    prowl: Option<clawbound_rs::prowl::Prowl>,
    capture: Option<clawbound_rs::capture::Capture>,
    tutorial: Option<TutorialRun>,
    kind: clawbound_rs::hunter::Kind,
    mode: clawbound_rs::sim::Mode,
    over_timer: f32,
    debug_cone: bool,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_render: crate::egui_render::EguiRender,
    menu: clawbound_rs::menu::Menu,
    keys: Keys,
    pbgl: wgpu::BindGroupLayout,
    samp: wgpu::Sampler,
    fxbuf: wgpu::Buffer,
    scene_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    ss_on: bool,
    scene_w: f32, scene_h: f32,   // actual scene texture size (post shader needs this, not the window size, to avoid a moiré at High graphics)
    flick: (f32, f32),
    mleft_prev: bool,
    pbg: wgpu::BindGroup,
    focus: clawbound_rs::math::Vec3,
    frame: f32,
    last: web_time::Instant,
    anim_t: f32,
    yaw: f32,
    pitch: f32,
    captured: bool,
    death_t: f32,
    prev_player_alive: bool,
}

fn make_targets(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::TextureView, wgpu::TextureView) {
    let mk = |fmt, usage| device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: fmt, usage, view_formats: &[],
    });
    let scene = mk(wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let depth = mk(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT);
    (scene.create_view(&Default::default()), depth.create_view(&Default::default()))
}

impl State {
    async fn new(window: Arc<Window>) -> State {
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface), force_fallback_adapter: false,
        }).await.expect("no adapter");
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            // use the real hardware limits on native (Retina windows exceed the 2048
            // downlevel cap); keep WebGL2 limits only for the wasm/WebGL path.
            required_limits: if cfg!(target_arch = "wasm32") { wgpu::Limits::downlevel_webgl2_defaults() } else { adapter.limits() },
            memory_hints: wgpu::MemoryHints::default(),
        }, None).await.expect("no device");

        let max = device.limits().max_texture_dimension_2d;
        let (w, h) = (w.min(max), h.min(max));   // never configure beyond the device max

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format, width: w, height: h,
            present_mode: wgpu::PresentMode::Fifo, alpha_mode: caps.alpha_modes[0],
            view_formats: vec![], desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // static world geometry (built once) + the live 6-vs-1 sim
        let world = crate::build_world_env();
        let world_vcount = world.count();
        let world_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world"), contents: bytemuck::cast_slice(&world.verts), usage: wgpu::BufferUsages::VERTEX,
        });
        let kind = clawbound_rs::hunter::Kind::Fox;
        let mode = clawbound_rs::sim::Mode::Rendezvous;
        let game = Game::new_match(kind, mode);
        let focus = v3(game.cats[0].x, 0.0, game.cats[0].z);
        // dynamic entity buffer (cats + hunter + cone + beacon), rewritten each frame
        let ent_cap: u64 = 400_000 * 40;
        let ent_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("entities"), size: ent_cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });

        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("u"), size: std::mem::size_of::<U>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });

        // scene pipeline
        let scene_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(crate::SCENE_WGSL.into()) });
        let ubgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None,
            }],
        });
        let ubg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &ubgl, entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }] });
        let scene_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&ubgl], push_constant_ranges: &[] });
        let vlayout = wgpu::VertexBufferLayout {
            array_stride: 40, step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 24, shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 36, shader_location: 3, format: wgpu::VertexFormat::Float32 },
            ],
        };
        let scene_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: Some(&scene_pl),
            vertex: wgpu::VertexState { module: &scene_sh, entry_point: "vs", buffers: &[vlayout], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &scene_sh, entry_point: "fs", targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview: None, cache: None,
        });

        // post pipeline (targets the surface)
        let post_src = crate::POST_WGSL;
        let post_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(post_src.into()) });
        let samp = device.create_sampler(&wgpu::SamplerDescriptor { label: None, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let fxbuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("fx"), size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let pbgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let post_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&pbgl], push_constant_ranges: &[] });
        let post_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: Some(&post_pl),
            vertex: wgpu::VertexState { module: &post_sh, entry_point: "vs", buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &post_sh, entry_point: "fs", targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview: None, cache: None,
        });

        let (scene_view, depth_view) = make_targets(&device, w, h);
        let pbg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pbgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&samp) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&scene_view) },
            wgpu::BindGroupEntry { binding: 2, resource: fxbuf.as_entire_binding() },
        ] });

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(egui_ctx.clone(), egui::ViewportId::ROOT, &*window, None, None);
        let egui_render = crate::egui_render::EguiRender::new(&device, config.format);
        let menu = clawbound_rs::menu::Menu::new();

        State { _window: window, surface, device, queue, config, scene_pipe, post_pipe, ubuf, ubg, world_vbuf, world_vcount, ent_vbuf, ent_vcount: 0, ent_cap, game, prowl: None, capture: None, tutorial: None, kind, mode, over_timer: 0.0, debug_cone: false, egui_ctx, egui_state, egui_render, menu, keys: Keys::default(), pbgl, samp, fxbuf, scene_view, depth_view, ss_on: false, scene_w: w as f32, scene_h: h as f32, flick: (0.0, 0.0), mleft_prev: false, pbg, focus, frame: 0.0, last: web_time::Instant::now(), anim_t: 0.0, yaw: 0.55, pitch: 0.62, captured: false, death_t: 0.0, prev_player_alive: true }
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 { return; }
        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (size.width.min(max), size.height.min(max));
        self.config.width = w; self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.apply_targets();
    }

    /// (Re)create the offscreen scene/depth targets and post bind group. In High/AAA
    /// graphics the scene renders to a 1.85x-supersampled target that the post pass
    /// samples down, for crisper edges — the surface itself stays at window size.
    fn apply_targets(&mut self) {
        let max = self.device.limits().max_texture_dimension_2d;
        let s = if self.menu.settings.hires { 1.85 } else { 1.0 };
        let tw = ((self.config.width as f32 * s) as u32).min(max).max(1);
        let th = ((self.config.height as f32 * s) as u32).min(max).max(1);
        let (sv, dv) = make_targets(&self.device, tw, th);
        self.scene_view = sv; self.depth_view = dv;
        self.scene_w = tw as f32; self.scene_h = th as f32;
        self.ss_on = self.menu.settings.hires;
        self.pbg = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.pbgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.samp) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.scene_view) },
            wgpu::BindGroupEntry { binding: 2, resource: self.fxbuf.as_entire_binding() },
        ] });
    }

    /// Lock+hide the cursor for mouse-look during play; release it in menus / on game-over.
    fn set_capture(&mut self, want: bool) {
        if want == self.captured { return; }
        use winit::window::CursorGrabMode;
        if want {
            let ok = self._window.set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self._window.set_cursor_grab(CursorGrabMode::Confined));
            if ok.is_ok() { self._window.set_cursor_visible(false); self.captured = true; }
        } else {
            let _ = self._window.set_cursor_grab(CursorGrabMode::None);
            self._window.set_cursor_visible(true);
            self.captured = false;
        }
    }

    /// Raw mouse motion → orbit the camera. Yaw wraps; pitch is clamped off the poles.
    fn on_mouse(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.0026;
        self.pitch = (self.pitch + dy * 0.0022).clamp(0.12, 1.32);
        self.flick.0 += dx; self.flick.1 += dy;   // accumulate for flick-move detection
    }

    /// Grow (if needed) and upload the moving-entity vertex buffer.
    fn upload_entities(&mut self, ents: &clawbound_rs::mesh::Mesh) {
        self.ent_vcount = ents.count();
        let bytes: &[u8] = bytemuck::cast_slice(&ents.verts);
        if (bytes.len() as u64) > self.ent_cap {
            self.ent_cap = (bytes.len() as u64).next_power_of_two();
            self.ent_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("entities"), size: self.ent_cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() { self.queue.write_buffer(&self.ent_vbuf, 0, bytes); }
    }

    /// Write the third-person orbit camera uniform from the current focus/yaw/pitch.
    fn write_camera(&mut self, dist: f32) {
        let (sy, cy) = (self.yaw.sin(), self.yaw.cos());
        let horiz = self.pitch.cos() * dist;
        let height = self.pitch.sin() * dist;
        let eye = self.focus.add(v3(-sy * horiz, height, -cy * horiz));
        let target = self.focus.add(v3(0.0, 1.1, 0.0));
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let mvp = perspective(56f32.to_radians(), aspect, 0.1, 900.0).mul(&look_at(eye, target, v3(0.0, 1.0, 0.0)));
        let u = U { mvp: mvp.0, light: [0.4, 1.0, 0.35, 0.0], key: [1.05, 0.97, 0.85, 0.0], amb: [0.52, 0.53, 0.50, 0.0] };
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));
    }

    /// The two render passes (scene → post) + the egui overlay, then present.
    fn present(&mut self, bg: wgpu::Color, full: egui::FullOutput, ppp: f32) {
        let frame = match self.surface.get_current_texture() { Ok(f) => f, Err(_) => { self.surface.configure(&self.device, &self.config); return; } };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.scene_view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(bg), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth_view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None,
            });
            rp.set_pipeline(&self.scene_pipe);
            rp.set_bind_group(0, &self.ubg, &[]);
            rp.set_vertex_buffer(0, self.world_vbuf.slice(..));
            rp.draw(0..self.world_vcount, 0..1);
            if self.ent_vcount > 0 { rp.set_vertex_buffer(0, self.ent_vbuf.slice(..)); rp.draw(0..self.ent_vcount, 0..1); }
        }
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
            });
            rp.set_pipeline(&self.post_pipe); rp.set_bind_group(0, &self.pbg, &[]); rp.draw(0..3, 0..1);
        }
        self.egui_state.handle_platform_output(&self._window, full.platform_output);
        let prims = self.egui_ctx.tessellate(full.shapes, ppp);
        self.egui_render.render(&self.device, &self.queue, &mut enc, &view, [self.config.width, self.config.height], ppp, &prims, &full.textures_delta);
        self.queue.submit(Some(enc.finish()));
        frame.present();
    }

    /// Dynamic entities for the Prowl mode (the static world is already in world_vbuf).
    fn build_prowl_entities(&self) -> clawbound_rs::mesh::Mesh {
        use clawbound_rs::mesh::{rgb, Mesh};
        use clawbound_rs::prowl::ProwlObj;
        use clawbound_rs::{cat, world};
        let g = self.prowl.as_ref().unwrap();
        let mut m: Mesh = crate::build_critters(&g.critters);
        crate::build_lanes(&mut m, &g.lanes);   // worn paths between the staging grounds
        for &(px, pz, _n) in &g.pois {
            let gy = world::height_at(px, pz);
            m.set_op(0.5); m.segment(v3(px, gy, pz), v3(px, gy + 10.0, pz), 0.4, 0.1, 5, rgb(0x8fd0ff)); m.set_op(1.0);
        }
        if matches!(g.obj.kind, ProwlObj::Reach | ProwlObj::Rendezvous) {
            let (ox, oz) = (g.obj.tx, g.obj.tz); let gy = world::height_at(ox, oz);
            m.set_op(0.85); m.segment(v3(ox, gy, oz), v3(ox, gy + 22.0, oz), 0.9, 0.25, 8, rgb(0x8fd0ff)); m.set_op(1.0);
        }
        // the shrinking hunt-ring wall
        let rr = g.ring.radius();
        m.set_op(0.3);
        for k in 0..72 {
            let a = k as f32 / 72.0 * std::f32::consts::TAU;
            let (rx, rz) = (g.ring.cx + a.cos() * rr, g.ring.cz + a.sin() * rr);
            let gy = world::height_at(rx, rz);
            m.segment(v3(rx, gy - 1.0, rz), v3(rx, gy + 7.0, rz), 0.35, 0.35, 4, rgb(0x9a6cff));
        }
        m.set_op(1.0);
        // ancestor trees — grand swaying staging grounds
        for a in &g.ancestors {
            let gy = world::height_at(a.x, a.z);
            crate::build_ancestor(&mut m, a.x, a.z, gy, self.frame * 0.03 + a.x * 0.05, a.ready(), a.claimed_by);
            // a claimed tree is a beacon — a faint aura ring, colored by whoever claimed it,
            // that brightens right as it's about to pulse
            if let Some(bc) = a.beacon_class {
                let pulse_phase = 1.0 - (a.pulse_t / clawbound_rs::prowl::BEACON_PULSE).clamp(0.0, 1.0);
                m.set_op(0.15 + pulse_phase * 0.35);
                for k in 0..28 {
                    let ang = k as f32 / 28.0 * std::f32::consts::TAU;
                    let (rx, rz) = (a.x + ang.cos() * clawbound_rs::prowl::BEACON_RADIUS, a.z + ang.sin() * clawbound_rs::prowl::BEACON_RADIUS);
                    m.segment(v3(rx, gy, rz), v3(rx, gy + 0.4, rz), 0.15, 0.15, 4, rgb(bc.beacon_buff().color()));
                }
                m.set_op(1.0);
            }
        }
        // territory markers — small clan-tinted stakes, with a pulsing ring while contested
        for mk in &g.markers {
            let gy = world::height_at(mk.x, mk.z);
            m.segment(v3(mk.x, gy, mk.z), v3(mk.x, gy + 1.6, mk.z), 0.14, 0.06, 6, rgb(clawbound_rs::prowl::clan_color(mk.held_by)));
            if mk.dwell > 0.0 {
                let frac = (mk.dwell / clawbound_rs::prowl::MARKER_CLAIM_TIME).clamp(0.0, 1.0);
                m.set_op(0.5); m.boxx(v3(mk.x, gy + 1.9, mk.z), v3(0.5 * frac, 0.08, 0.5 * frac), rgb(0xffe040)); m.set_op(1.0);
            }
        }
        // the Rock of Nine Lives — the ritual's site
        {
            let (rx, rz) = g.rock;
            let gy = world::height_at(rx, rz);
            m.boxx(v3(rx, gy + 0.8, rz), v3(1.6, 1.6, 1.6), rgb(0x8a7a9a));
            if g.convergence_warning() { m.set_op(0.35 + 0.25 * (self.frame * 0.15).sin()); m.segment(v3(rx, gy, rz), v3(rx, gy + 3.0, rz), 2.2, 0.4, 10, rgb(0xc9a6ff)); m.set_op(1.0); }
        }
        // power-up pickups — floating coloured markers that bob
        for (px, pz, kind) in &g.pickups {
            let gy = world::height_at(*px, *pz) + 1.4 + (self.frame * 0.06 + *px).sin() * 0.2;
            m.boxx(v3(*px, gy, *pz), v3(0.5, 0.5, 0.5), rgb(kind.color()));
        }
        // clan minions — small cats on border patrol, tinted by clan; ranged ones (yowlers)
        // get a paler marker and a brief flash of white when they cry out
        for mm in &g.minions {
            let gy = world::height_at(mm.x, mm.z);
            let pose = cat::walk((self.anim_t * 1.3 + mm.x * 0.2).rem_euclid(1.0));
            cat::build(&mut m, v3(mm.x, gy, mm.z), mm.heading, &pose, clawbound_rs::class::Class::Cat);
            let base = clawbound_rs::prowl::clan_color(mm.clan);
            let col = if mm.yowl_flash > 0.0 { 0xffffff } else if mm.ranged { (base & 0xfefefe) | 0x404040 } else { base };
            let sz = if mm.ranged { v3(0.24, 0.10, 0.24) } else { v3(0.28, 0.12, 0.28) };
            m.boxx(v3(mm.x, gy + 1.3, mm.z), sz, rgb(col));
            if mm.ranged && mm.yowl_flash > 0.0 {
                m.set_op(mm.yowl_flash);
                m.segment(v3(mm.x, gy + 1.0, mm.z), v3(mm.x, gy + 1.0 + mm.yowl_flash * 3.0, mm.z), 0.15, 0.02, 6, rgb(0xffffff));
                m.set_op(1.0);
            }
        }
        // rival clan prowlers — full-size clan-tinted cats
        for rv in &g.rivals {
            let gy = world::height_at(rv.x, rv.z);
            let pose = if rv.stagger > 0.0 { cat::flinch(1.0 - (rv.stagger / 0.9).min(1.0)) }
                else if rv.class == clawbound_rs::class::Class::Hawk { cat::hover((self.anim_t * 1.3 + rv.x * 0.2).rem_euclid(1.0)) }
                else if gy < world::WATER_Y { cat::swim((self.anim_t * 1.3 + rv.x * 0.2).rem_euclid(1.0)) }
                else { cat::walk((self.anim_t * 1.3 + rv.x * 0.2).rem_euclid(1.0)) };
            cat::build(&mut m, v3(rv.x, gy, rv.z), rv.heading, &pose, rv.class);
            m.boxx(v3(rv.x, gy + 2.1, rv.z), v3(0.46, 0.2, 0.46), rgb(clawbound_rs::prowl::clan_color(rv.clan)));
        }
        // jungle camps — a guard cat (tinted by species) circling its herb marker
        for c in &g.jungle {
            let col = match c.kind { clawbound_rs::prowl::CampKind::Raccoon => 0x808080, clawbound_rs::prowl::CampKind::Fox => 0xd8623c, clawbound_rs::prowl::CampKind::Snake => 0x5aa054 };
            if c.guard_alive {
                let ggy = world::height_at(c.gx, c.gz);
                let pose = if c.alert { cat::walk((self.anim_t * 1.6 + c.gx * 0.2).rem_euclid(1.0)) } else { cat::idle(self.anim_t * 0.5 + c.gx) };
                cat::build(&mut m, v3(c.gx, ggy, c.gz), 0.0, &pose, clawbound_rs::class::Class::Cat);
                m.boxx(v3(c.gx, ggy + 1.3, c.gz), v3(0.3, 0.14, 0.3), rgb(col));
            }
            if c.cooldown <= 0.0 && (c.cleared() || !c.alert) {
                let hgy = world::height_at(c.x, c.z) + 0.9 + (self.frame * 0.05 + c.x).sin() * 0.15;
                m.boxx(v3(c.x, hgy, c.z), v3(0.4, 0.4, 0.4), rgb(c.kind.reward().color()));
            }
        }
        // the prowler + a class-tint marker
        let pgy = world::height_at(g.x, g.z);
        let arc = if g.leap_t > 0.0 { let ph = 1.0 - (g.leap_t / 0.5).clamp(0.0, 1.0); (ph * std::f32::consts::PI).sin() * 1.7 } else { 0.0 };
        let in_water = world::height_at(g.x, g.z) < world::WATER_Y;
        let pose = if g.respawn_flash > 0.0 { cat::caught(1.0 - (g.respawn_flash / 1.2).clamp(0.0, 1.0)) }
        else if g.class == clawbound_rs::class::Class::Hawk {
            if g.leap_t > 0.0 { cat::leap() } else if g.moving { cat::hover((self.anim_t * 1.5).rem_euclid(1.0)) } else { cat::hover_idle(self.anim_t * 0.6) }
        }
        else if g.leap_t > 0.0 { cat::leap() } else if g.dodge_t > 0.0 || g.crouched { cat::coil() }
        else if g.moving && in_water { cat::swim((self.anim_t * 1.5).rem_euclid(1.0)) }
        else if g.moving { cat::walk((self.anim_t * 1.5).rem_euclid(1.0)) } else { cat::idle(self.anim_t * 0.6) };
        cat::build(&mut m, v3(g.x, pgy + arc, g.z), g.heading, &pose, g.class);
        m.boxx(v3(g.x, pgy + 2.2, g.z), v3(0.4, 0.18, 0.4), rgb(g.class.def().color));
        if g.splash_flash > 0.0 {
            m.set_op(g.splash_flash * 0.6);
            for k in 0..8 {
                let a = k as f32 / 8.0 * std::f32::consts::TAU;
                let rr = (1.0 - g.splash_flash) * 2.2 + 0.3;
                m.segment(v3(g.x + a.cos() * rr, pgy + 0.05, g.z + a.sin() * rr), v3(g.x + a.cos() * (rr + 0.3), pgy + 0.05, g.z + a.sin() * (rr + 0.3)), 0.08, 0.02, 3, rgb(0xdff2ff));
            }
            m.set_op(1.0);
        }
        // the stalking hunter
        let hgy = world::height_at(g.hx, g.hz);
        if g.hunter_kind.is_shadow() { crate::build_shadow(&mut m, v3(g.hx, hgy, g.hz)); } else { crate::build_fox(&mut m, v3(g.hx, hgy, g.hz)); }
        m
    }

    /// The full per-frame path for the Prowl mode (parallel to the sim `render`).
    fn render_prowl(&mut self, dt: f32) {
        self.set_capture(!self.menu.open);   // prowl is endless — capture while playing

        if !self.menu.open {
            let intro = self.tutorial.as_ref().map(|t| t.intro_t).unwrap_or(0.0);
            if intro > 0.0 {
                // a short, non-interactive opening beat before handing control to the player.
                // (Camera yaw is deliberately left untouched here — panning it and not
                // resetting exactly left the camera rotated away from where the player
                // expected it when they gained control.)
                self.prowl.as_mut().unwrap().set_input(0.0, 0.0, false, false, false);
                self.prowl.as_mut().unwrap().step(dt);
                if let Some(t) = self.tutorial.as_mut() { t.intro_t -= dt; }
            } else {
            let (ix, iz) = self.keys.move_vec();
            let (mx, mz) = move_world(self.yaw, ix, iz);
            {
                let p = self.prowl.as_mut().unwrap();
                p.set_input(mx, mz, self.keys.leap, self.keys.dodge || self.keys.mmiddle, self.keys.crouch);
                p.set_ability(self.keys.ability);   // Q / bound key — the ultimate
            }
            // flick controls: a fast mouse-flick + left-click performs the matching
            // autoattack move (forward=pounce, back=hind-kick, sideways=roll, sharp=swat);
            // right-click held is the low/underbelly modifier. Same feel as the 6v1 mode.
            self.flick.0 *= 0.7; self.flick.1 *= 0.7;
            let fmag = (self.flick.0 * self.flick.0 + self.flick.1 * self.flick.1).sqrt();
            if self.keys.mleft && !self.mleft_prev {
                let fast = fmag > 14.0;
                let local = if fast { (self.flick.0 / fmag, -self.flick.1 / fmag) } else { (0.0, 0.0) };
                let mag = if fast { 1.0 } else { clawbound_rs::combat::FLICK_MIN + 0.1 };
                self.prowl.as_mut().unwrap().player_flick(local, mag, self.keys.mright);
            }
            self.mleft_prev = self.keys.mleft;
            self.prowl.as_mut().unwrap().step(dt);
            // tutorial: advance to the next step once the current one's condition is met
            if let Some(t) = self.tutorial.as_mut() {
                t.advance_flash = (t.advance_flash - dt * 1.5).max(0.0);
                if t.step < TUTORIAL_STEPS.len() - 1 && tutorial_step_done(t.step, self.prowl.as_ref().unwrap()) {
                    t.step += 1; t.advance_flash = 1.0;
                }
            }
            if self.keys.restart {
                let (c, k, diff, fw) = { let p = self.prowl.as_ref().unwrap(); (p.class, p.hunter_kind, p.difficulty, p.fixed_wind) };
                let mut p = clawbound_rs::prowl::Prowl::new(c, k);
                p.difficulty = diff;
                p.set_fixed_wind(fw);
                if let Some(t) = self.tutorial.as_mut() { p.set_tutorial_mode(true); *t = TutorialRun { step: 0, intro_t: 0.0, advance_flash: 0.0 }; }
                self.prowl = Some(p);
                self.keys.restart = false;
            }
            }
        }

        let (px, pz) = { let p = self.prowl.as_ref().unwrap(); (p.x, p.z) };
        let gy = clawbound_rs::world::height_at(px, pz);
        self.focus = v3(px, gy, pz);

        let ents = self.build_prowl_entities();
        self.upload_entities(&ents);

        let (flash, exposed, shield_flash) = { let p = self.prowl.as_ref().unwrap(); (p.respawn_flash, p.exposed, p.shielded_flash) };
        let grim = ((flash / 1.2).clamp(0.0, 1.0) * 0.8 + if exposed { 0.25 } else { 0.0 }).min(1.0);
        // a shield-soak spares you, so it shows no damage tint
        let fx0 = if shield_flash > 0.0 { 0.0 } else { grim };
        self.queue.write_buffer(&self.fxbuf, 0, bytemuck::cast_slice(&[fx0, 0.2f32, self.frame * 0.016, 0.0f32, 1.0 / self.scene_w, 1.0 / self.scene_h, self.scene_w, self.scene_h]));
        self.write_camera(20.0);

        let items = self.menu.items();
        let (cursor, open, screen) = (self.menu.cursor, self.menu.open, self.menu.screen);
        let (obj_text, prey, caught, escapes, stamina, cls, cls_name, clans, b_speed, b_shield, staging, outcome, ab_name, ab_ready, ab_cd, ab_blurb, pv_name, pv_blurb, pv_used) = {
            let p = self.prowl.as_ref().unwrap();
            let ab = p.class.ability();
            let pv = p.class.passive();
            (p.objective_text(), p.prey_caught, p.times_caught, p.escapes, p.stamina, p.class, p.class.name(),
             p.clan_scores, p.buffs.speed_boosted(), p.buffs.shield, p.on_staging, p.outcome,
             ab.name(), p.ability_ready(), p.ability_cd, ab.blurb(),
             pv.name(), pv.blurb(), p.nine_lives_used)
        };
        let (prowl_move_name, prowl_combo_flash, prowl_combo_name) = {
            let p = self.prowl.as_ref().unwrap();
            (p.player_move.map(|(m, _)| m.name()), p.combo_flash, p.combo_name)
        };
        let ((wind_dx, wind_dz), wind_strength) = self.prowl.as_ref().unwrap().wind();
        let yowled = self.prowl.as_ref().unwrap().yowled_t > 0.0;
        let (sustenance, sick, ritual_p, markers_held, convergence_soon, convergence_result, convergence_t) = {
            let p = self.prowl.as_ref().unwrap();
            let held = p.markers.iter().filter(|mk| mk.held_by == 0).count();
            (p.sustenance, p.sick, p.ritual_progress[0], held, p.convergence_warning(), p.convergence_result, p.convergence_t)
        };
        let jungle_hint = {
            let p = self.prowl.as_ref().unwrap();
            p.jungle.iter().filter(|c| {
                let d = ((p.x - c.x).powi(2) + (p.z - c.z).powi(2)).sqrt();
                d < 16.0
            }).map(|c| {
                if c.cleared() { format!("{} camp cleared — herb free", c.kind.name()) }
                else if c.alert { format!("{} camp — guard alerted!", c.kind.name()) }
                else { format!("{} camp — sneak in, or clear it out", c.kind.name()) }
            }).next()
        };
        let tutorial_ui: Option<(&'static str, &'static str, bool, f32)> = self.tutorial.as_ref().map(|t| {
            let (title, prompt) = TUTORIAL_STEPS[t.step];
            (title, prompt, t.step == TUTORIAL_STEPS.len() - 1, t.advance_flash)
        });
        let tutorial_intro = self.tutorial.as_ref().map(|t| t.intro_t > 0.0).unwrap_or(false);
        let ppp = self.egui_ctx.pixels_per_point();
        let raw = self.egui_state.take_egui_input(&self._window);
        let full = self.egui_ctx.run(raw, |ctx| {
            use egui::{Align2, Color32, RichText};
            if open { draw_menu(ctx, &items, cursor, screen); return; }
            if let Some((title, prompt, is_last, flash)) = tutorial_ui {
                egui::Area::new("tutorial".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_BOTTOM, [0.0, -70.0]).show(ctx, |ui| {
                    egui::Frame::none().fill(Color32::from_rgba_unmultiplied(20, 16, 28, 210))
                        .inner_margin(egui::Margin::symmetric(18.0, 12.0)).rounding(8.0).show(ui, |ui| {
                        ui.set_max_width(460.0);
                        ui.vertical_centered(|ui| {
                            let glow = (255.0 * (0.6 + 0.4 * flash)) as u8;
                            ui.label(RichText::new(title).size(17.0).strong().color(Color32::from_rgb(glow, 214, 120)));
                            ui.label(RichText::new(prompt).size(13.5).color(Color32::from_rgb(220, 218, 228)));
                            if is_last { ui.label(RichText::new("Press Enter to finish").size(11.5).italics().color(Color32::from_rgb(160, 200, 170))); }
                        });
                    });
                });
                if tutorial_intro {
                    egui::Area::new("tutorial_intro".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, -40.0]).show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("PROWL").size(38.0).strong().color(Color32::from_rgb(220, 200, 255)));
                            ui.label(RichText::new("a training walk through the clan grounds").size(14.0).italics().color(Color32::from_rgb(200, 200, 210)));
                        });
                    });
                }
            }
            egui::Area::new("phud".into()).anchor(Align2::LEFT_TOP, [16.0, 14.0]).show(ctx, |ui| {
                egui::Frame::none().fill(Color32::from_rgba_unmultiplied(14, 12, 20, 150))
                    .inner_margin(egui::Margin::symmetric(12.0, 9.0)).rounding(6.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(RichText::new(format!("PROWL · {cls_name}   ·   your score {}", clans[0])).size(15.0).strong().color(Color32::from_rgb(200, 180, 235)));
                    ui.horizontal(|ui| {
                        let names = ["Ember", "Tide", "Moss", "Amber"];
                        let cols = [Color32::from_rgb(216, 98, 60), Color32::from_rgb(61, 127, 216), Color32::from_rgb(90, 160, 84), Color32::from_rgb(216, 165, 60)];
                        for c in 0..4 { ui.label(RichText::new(format!("{} {}", names[c], clans[c])).size(12.0).color(cols[c])); }
                    });
                    ui.label(RichText::new(obj_text).size(14.0).color(Color32::from_rgb(200, 220, 205)));
                    ui.label(RichText::new(format!("prey {prey}   ·   caught {caught}   ·   escapes {escapes}")).size(13.0).color(Color32::from_rgb(200, 210, 225)));
                    ui.add(egui::ProgressBar::new(stamina).desired_width(168.0)
                        .fill(if stamina > 0.25 { Color32::from_rgb(120, 200, 150) } else { Color32::from_rgb(215, 120, 90) })
                        .text(RichText::new("stamina").size(11.0).color(Color32::WHITE)));
                    let mut buffs = String::new();
                    if b_speed { buffs.push_str("PAW "); }
                    if b_shield { buffs.push_str("PELT "); }
                    if !buffs.is_empty() {
                        ui.horizontal(|ui| {
                            if b_speed { icon_power(ui, clawbound_rs::powerup::Power::Speed, Color32::from_rgb(255, 214, 120)); }
                            if b_shield { icon_power(ui, clawbound_rs::powerup::Power::Shield, Color32::from_rgb(120, 200, 255)); }
                            ui.label(RichText::new(buffs).size(13.0).strong().color(Color32::from_rgb(120, 200, 255)));
                        });
                    }
                    let pv_state = if pv_name == "Nine Lives" { if pv_used { " (spent)" } else { " (ready)" } } else { "" };
                    ui.horizontal(|ui| {
                        icon_passive(ui, cls, Color32::from_rgb(150, 220, 235));
                        ui.label(RichText::new(format!("Passive · {pv_name}{pv_state}")).size(12.0).strong().color(Color32::from_rgb(150, 220, 235)));
                    });
                    ui.label(RichText::new(pv_blurb).size(10.5).italics().color(Color32::from_rgb(150, 150, 165)));
                    let (astate, acol) = if ab_ready { ("READY".to_string(), Color32::from_rgb(150, 230, 160)) } else { (format!("{:.0}s", ab_cd.ceil()), Color32::from_rgb(180, 180, 190)) };
                    ui.horizontal(|ui| {
                        icon_ultimate(ui, cls, if ab_ready { Color32::from_rgb(255, 214, 120) } else { acol });
                        ui.label(RichText::new(format!("ULT (Q) · {ab_name}  [{astate}]")).size(13.0).strong().color(if ab_ready { Color32::from_rgb(255, 214, 120) } else { acol }));
                    });
                    ui.label(RichText::new(ab_blurb).size(10.5).italics().color(Color32::from_rgb(150, 150, 165)));
                    if staging { ui.label(RichText::new("* STAGING GROUND - sheltered by the ancestors").size(13.0).color(Color32::from_rgb(140, 220, 160))); }
                    if let Some(jh) = &jungle_hint { ui.label(RichText::new(jh).size(13.0).color(Color32::from_rgb(200, 210, 140))); }
                    ui.add_space(3.0);
                    ui.add(egui::ProgressBar::new(sustenance).desired_width(168.0)
                        .fill(if sick { Color32::from_rgb(200, 110, 70) } else { Color32::from_rgb(150, 190, 110) })
                        .text(RichText::new(if sick { "sustenance - SICK" } else { "sustenance" }).size(11.0).color(Color32::WHITE)));
                    ui.label(RichText::new(format!("Territory {markers_held}/{} · Ritual {:.0}/{:.0}", clawbound_rs::prowl::N_MARKERS, ritual_p, clawbound_rs::prowl::RITUAL_WIN))
                        .size(12.0).color(Color32::from_rgb(200, 180, 220)));
                    if convergence_soon { ui.label(RichText::new(format!("* the Rock stirs... convergence in {:.0}s", convergence_t.max(0.0).ceil())).size(12.0).color(Color32::from_rgb(200, 160, 255))); }
                    if let Some(w) = convergence_result { ui.label(RichText::new(format!("Convergence resolved — {} led the territory", clawbound_rs::prowl::clan_name(w))).size(11.0).italics().color(Color32::from_rgb(180, 200, 220))); }
                    let wind_deg = wind_dx.atan2(wind_dz).to_degrees();
                    ui.label(RichText::new(format!("Wind {:.0}° · {}", wind_deg.rem_euclid(360.0),
                        if wind_strength > 0.9 { "strong" } else if wind_strength > 0.35 { "breezy" } else { "calm" }))
                        .size(11.0).color(Color32::from_rgb(160, 200, 210)));
                    if exposed { ui.label(RichText::new("! EXPOSED - get back inside the ring").size(13.0).color(Color32::from_rgb(230, 150, 120))); }
                    if yowled { ui.label(RichText::new("! OUTED - a yowl gave you away").size(13.0).strong().color(Color32::from_rgb(255, 200, 90))); }
                });
            });
            // autoattack: the current move, and a combo banner when a chain lands
            if let Some(mn) = prowl_move_name {
                egui::Area::new("pmove".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, 46.0]).show(ctx, |ui| {
                    ui.label(RichText::new(mn).size(15.0).strong().color(Color32::from_rgb(255, 240, 200)));
                });
            }
            if prowl_combo_flash > 0.05 {
                egui::Area::new("pcombo".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
                    ui.label(RichText::new(prowl_combo_name).size(20.0 + prowl_combo_flash * 12.0).strong()
                        .color(Color32::from_rgba_unmultiplied(255, 210, 90, (255.0 * prowl_combo_flash.min(1.0)) as u8)));
                });
            }
            if convergence_t <= 5.0 && convergence_t > 0.0 {
                let pulse = (self.frame as f32 * 0.4).sin() * 0.5 + 0.5;
                egui::Area::new("convergence".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, -140.0]).show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("THE RITUAL STIRS").size(22.0 + pulse * 4.0).strong().color(Color32::from_rgb(200, 160, 255)));
                        ui.label(RichText::new(format!("{:.0}", convergence_t.ceil())).size(30.0).strong().color(Color32::from_rgb(230, 200, 255)));
                    });
                });
            }
            egui::Area::new("phint".into()).anchor(Align2::CENTER_BOTTOM, [0.0, -12.0]).show(ctx, |ui| {
                ui.label(RichText::new("WASD move · mouse look · Shift hide · Space/M-click roll · F leap · flick+L-click autoattack (R-click low) · Q ultimate · R restart · Esc menu")
                    .size(12.0).color(Color32::from_rgba_unmultiplied(235, 230, 220, 190)));
            });
            if exposed {
                egui::Area::new("pexp".into()).order(egui::Order::Background).show(ctx, |ui| {
                    ui.painter().rect_filled(ctx.screen_rect(), 0.0, Color32::from_rgba_unmultiplied(70, 24, 80, 46));
                });
            }
            if let Some(w) = outcome {
                egui::Area::new("pdim".into()).order(egui::Order::Background).show(ctx, |ui| { ui.painter().rect_filled(ctx.screen_rect(), 0.0, Color32::from_black_alpha(150)); });
                let names = ["EMBER", "TIDE", "MOSS", "AMBER"];
                let cols = [Color32::from_rgb(216, 98, 60), Color32::from_rgb(61, 127, 216), Color32::from_rgb(90, 160, 84), Color32::from_rgb(216, 165, 60)];
                let (txt, col) = if (w as usize) < 4 {
                    (if w == 0 { "YOUR CLAN WINS".to_string() } else { format!("{} CLAN WINS", names[w as usize]) }, cols[w as usize])
                } else { ("STALEMATE".to_string(), Color32::from_rgb(210, 205, 200)) };
                egui::Area::new("pres".into()).anchor(Align2::CENTER_CENTER, [0.0, -10.0]).show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(txt).size(44.0).strong().color(col));
                        ui.add_space(8.0);
                        ui.label(RichText::new("press  R  for a new hunt").size(16.0).color(Color32::from_rgb(255, 220, 120)));
                    });
                });
            }
        });
        let bg = wgpu::Color { r: 0.80, g: 0.82, b: 0.86, a: 1.0 };
        self.present(bg, full, ppp);
    }

    /// Dynamic entities for Pop Island (bases, yarn, power-ups, firecrackers, creatures).
    fn build_capture_entities(&self) -> clawbound_rs::mesh::Mesh {
        use clawbound_rs::capture::{BASE_R, TAWNY};
        use clawbound_rs::mesh::{rgb, Mesh};
        use clawbound_rs::{cat, world};
        let g = self.capture.as_ref().unwrap();
        let mut m = Mesh::new();
        for team in [0u8, 1u8] {
            let (bx, bz) = g.base(team);
            let col = if team == TAWNY { rgb(0xff7030) } else { rgb(0x3070ff) };
            let gy = world::height_at(bx, bz);
            m.set_op(0.5); m.cone(v3(bx, gy + 0.15, bz), BASE_R, 0.3, 22, col); m.set_op(1.0);
            m.segment(v3(bx, gy, bz), v3(bx, gy + 16.0, bz), 0.6, 0.15, 6, col);
        }
        for &(yx, yz) in &g.yarn_free { let gy = world::height_at(yx, yz); m.cone(v3(yx, gy + 0.8, yz), 0.8, 1.4, 6, rgb(0xff6acf)); }
        for p in &g.powerups { let gy = world::height_at(p.x, p.z); m.boxx(v3(p.x, gy + 1.6, p.z), v3(0.5, 0.5, 0.5), rgb(p.kind.color())); }
        for c in &g.crackers { let gy = world::height_at(c.x, c.z); m.boxx(v3(c.x, gy + 0.9, c.z), v3(0.28, 0.28, 0.28), rgb(0xff8030)); }
        for mob in &g.mobs {
            let gy = world::height_at(mob.x, mob.z);
            let moving = mob.stun_t <= 0.0;
            let pose = if mob.stunned() { cat::flinch(1.0 - (mob.stun_t / mob.class.def().stun).clamp(0.0, 1.0)) }
                else if mob.class == clawbound_rs::class::Class::Hawk { cat::hover((self.anim_t * 1.5 + mob.x * 0.1).rem_euclid(1.0)) }
                else if moving { cat::walk((self.anim_t * 1.5 + mob.x * 0.1).rem_euclid(1.0)) }
                else { cat::idle(self.anim_t * 0.6 + mob.x * 0.05) };
            cat::build(&mut m, v3(mob.x, gy, mob.z), mob.heading, &pose, mob.class);
            let team_col = if mob.team == TAWNY { rgb(0xff7030) } else { rgb(0x3070ff) };
            m.boxx(v3(mob.x, gy + 2.1, mob.z), v3(0.5, 0.18, 0.5), team_col);
            m.boxx(v3(mob.x, gy + 1.2, mob.z), v3(0.32, 0.32, 0.32), rgb(mob.class.def().color));
            if mob.carrying { m.cone(v3(mob.x, gy + 1.9, mob.z), 0.45, 0.7, 6, rgb(0xff6acf)); }
        }
        m
    }

    /// The full per-frame path for Pop Island (parallel to the sim/prowl renders).
    fn render_capture(&mut self, dt: f32) {
        self.set_capture(!self.menu.open);
        if !self.menu.open {
            let (ix, iz) = self.keys.move_vec();
            let (mx, mz) = move_world(self.yaw, ix, iz);
            {
                let g = self.capture.as_mut().unwrap();
                g.set_input(mx, mz, self.keys.dodge || self.keys.mleft, self.keys.crouch || self.keys.mright);   // Space/L-click throw, Shift/R-click jump
                g.set_ability(self.keys.ability);   // Q — the ultimate
            }
            // flick controls: a fast mouse-flick + middle-click performs the matching
            // melee autoattack (forward=pounce, back=hind-kick, sideways=roll) — a
            // close-range option alongside the firecracker throw. Same feel as Prowl/6v1.
            self.flick.0 *= 0.7; self.flick.1 *= 0.7;
            let fmag = (self.flick.0 * self.flick.0 + self.flick.1 * self.flick.1).sqrt();
            if self.keys.mmiddle && !self.mleft_prev {
                let fast = fmag > 14.0;
                let local = if fast { (self.flick.0 / fmag, -self.flick.1 / fmag) } else { (0.0, 0.0) };
                let mag = if fast { 1.0 } else { clawbound_rs::combat::FLICK_MIN + 0.1 };
                self.capture.as_mut().unwrap().player_flick(local, mag, false);
            }
            self.mleft_prev = self.keys.mmiddle;
            self.capture.as_mut().unwrap().step(dt);
            if self.keys.restart {
                let c = self.capture.as_ref().unwrap().mobs[0].class;
                self.capture = Some(clawbound_rs::capture::Capture::new(c));
                self.keys.restart = false;
            }
        }
        let (px, pz) = { let g = self.capture.as_ref().unwrap(); (g.mobs[0].x, g.mobs[0].z) };
        let gy = clawbound_rs::world::height_at(px, pz);
        self.focus = v3(px, gy, pz);

        let ents = self.build_capture_entities();
        self.upload_entities(&ents);

        let stunned = { self.capture.as_ref().unwrap().mobs[0].stunned() };
        let grim = if stunned { 0.4 } else { 0.0 };
        self.queue.write_buffer(&self.fxbuf, 0, bytemuck::cast_slice(&[grim, 0.2f32, self.frame * 0.016, 0.0f32, 1.0 / self.scene_w, 1.0 / self.scene_h, self.scene_w, self.scene_h]));
        self.write_camera(34.0);

        let items = self.menu.items();
        let (cursor, open, screen) = (self.menu.cursor, self.menu.open, self.menu.screen);
        let (s0, s1, period, ptime, outcome, carrying, caps, stuns, cls_raw, cls, b_speed, b_shield, b_triple, cd, overtime) = {
            let g = self.capture.as_ref().unwrap();
            let p = &g.mobs[0];
            (g.score[0], g.score[1], g.period, g.period_t.max(0.0), g.outcome, p.carrying, p.captures, p.stuns, p.class, p.class.name(),
             p.buffs.speed_boosted(), p.buffs.shield, p.buffs.triple_active(), p.cd, g.overtime)
        };
        let (cap_move_name, cap_combo_flash, cap_combo_name, ult_name, ult_ready, ult_cd, ult_blurb) = {
            let g = self.capture.as_ref().unwrap();
            let ab = g.mobs[0].class.ability();
            (g.player_move.map(|(m, _)| m.name()), g.combo_flash, g.combo_name, ab.name(), g.ability_ready(), g.ability_cd, ab.blurb())
        };
        let ppp = self.egui_ctx.pixels_per_point();
        let raw = self.egui_state.take_egui_input(&self._window);
        let full = self.egui_ctx.run(raw, |ctx| {
            use egui::{Align2, Color32, RichText};
            if open { draw_menu(ctx, &items, cursor, screen); return; }
            egui::Area::new("cscore".into()).anchor(Align2::CENTER_TOP, [0.0, 12.0]).show(ctx, |ui| {
                egui::Frame::none().fill(Color32::from_rgba_unmultiplied(14, 12, 20, 160)).inner_margin(egui::Margin::symmetric(16.0, 8.0)).rounding(8.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("TAWNY {s0}")).size(20.0).strong().color(Color32::from_rgb(255, 138, 90)));
                        ui.label(RichText::new(format!("   {}:{:02}   ", (ptime as u32) / 60, (ptime as u32) % 60)).size(18.0).color(Color32::WHITE));
                        ui.label(RichText::new(format!("{s1} SILVER")).size(20.0).strong().color(Color32::from_rgb(90, 154, 255)));
                    });
                    ui.vertical_centered(|ui| ui.label(RichText::new(if overtime { "SUDDEN DEATH".to_string() } else { format!("PERIOD {period}") }).size(12.0).strong().color(if overtime { Color32::from_rgb(255, 180, 90) } else { Color32::from_gray(180) })));
                });
            });
            egui::Area::new("cinfo".into()).anchor(Align2::LEFT_BOTTOM, [16.0, -16.0]).show(ctx, |ui| {
                egui::Frame::none().fill(Color32::from_rgba_unmultiplied(14, 12, 20, 150)).inner_margin(egui::Margin::symmetric(12.0, 9.0)).rounding(6.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new(format!("{cls} - TEAM TAWNY")).size(14.0).strong().color(Color32::from_rgb(255, 160, 110)));
                    ui.label(RichText::new(format!("captures {caps}  -  stuns {stuns}")).size(13.0).color(Color32::from_rgb(210, 215, 225)));
                    let mut pu = String::new();
                    if b_speed { pu.push_str("PAW "); }
                    if b_shield { pu.push_str("PELT "); }
                    if b_triple { pu.push_str("CLAW "); }
                    if !pu.is_empty() {
                        ui.horizontal(|ui| {
                            if b_speed { icon_power(ui, clawbound_rs::powerup::Power::Speed, Color32::from_rgb(255, 214, 120)); }
                            if b_shield { icon_power(ui, clawbound_rs::powerup::Power::Shield, Color32::from_rgb(120, 200, 255)); }
                            if b_triple { icon_power(ui, clawbound_rs::powerup::Power::Triple, Color32::from_rgb(255, 106, 207)); }
                            ui.label(RichText::new(pu).size(13.0).strong().color(Color32::from_rgb(255, 224, 120)));
                        });
                    }
                    if cd > 0.0 { ui.label(RichText::new(format!("firecracker {:.1}s", cd)).size(12.0).color(Color32::from_rgb(230, 150, 120))); }
                    else { ui.label(RichText::new("firecracker READY").size(12.0).color(Color32::from_rgb(150, 220, 150))); }
                    let (ustate, ucol) = if ult_ready { ("READY".to_string(), Color32::from_rgb(150, 230, 160)) } else { (format!("{:.0}s", ult_cd.ceil()), Color32::from_rgb(180, 180, 190)) };
                    ui.horizontal(|ui| {
                        icon_ultimate(ui, cls_raw, if ult_ready { Color32::from_rgb(255, 214, 120) } else { ucol });
                        ui.label(RichText::new(format!("ULT (Q) · {ult_name}  [{ustate}]")).size(12.0).strong().color(if ult_ready { Color32::from_rgb(255, 214, 120) } else { ucol }));
                    });
                    ui.label(RichText::new(ult_blurb).size(10.0).italics().color(Color32::from_rgb(150, 150, 165)));
                });
            });
            if carrying {
                egui::Area::new("ccarry".into()).anchor(Align2::CENTER_TOP, [0.0, 86.0]).show(ctx, |ui| {
                    ui.label(RichText::new("* CARRYING YARN - GET TO YOUR BASE! *").size(16.0).strong().color(Color32::from_rgb(255, 106, 207)));
                });
            }
            if let Some(mn) = cap_move_name {
                egui::Area::new("cmove".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, 46.0]).show(ctx, |ui| {
                    ui.label(RichText::new(mn).size(15.0).strong().color(Color32::from_rgb(255, 240, 200)));
                });
            }
            if cap_combo_flash > 0.05 {
                egui::Area::new("ccombo".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
                    ui.label(RichText::new(cap_combo_name).size(20.0 + cap_combo_flash * 12.0).strong()
                        .color(Color32::from_rgba_unmultiplied(255, 210, 90, (255.0 * cap_combo_flash.min(1.0)) as u8)));
                });
            }
            egui::Area::new("chint".into()).anchor(Align2::CENTER_BOTTOM, [0.0, -12.0]).show(ctx, |ui| {
                ui.label(RichText::new("WASD move · mouse look · Space/L-click throw · Shift/R-click jump · flick+M-click melee · Q ultimate · R restart · Esc menu")
                    .size(12.0).color(Color32::from_rgba_unmultiplied(235, 230, 220, 190)));
            });
            if let Some(w) = outcome {
                egui::Area::new("cdim".into()).order(egui::Order::Background).show(ctx, |ui| { ui.painter().rect_filled(ctx.screen_rect(), 0.0, Color32::from_black_alpha(150)); });
                let (txt, col) = match w {
                    0 => ("TAWNY WINS", Color32::from_rgb(255, 138, 90)),
                    1 => ("SILVER WINS", Color32::from_rgb(90, 154, 255)),
                    _ => ("DRAW", Color32::from_rgb(220, 214, 205)),
                };
                egui::Area::new("cresult".into()).anchor(Align2::CENTER_CENTER, [0.0, -10.0]).show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(txt).size(46.0).strong().color(col));
                        ui.add_space(8.0);
                        ui.label(RichText::new("press  R  for another match").size(16.0).color(Color32::from_rgb(255, 220, 120)));
                    });
                });
            }
        });
        let bg = wgpu::Color { r: 0.62, g: 0.80, b: 0.92, a: 1.0 };
        self.present(bg, full, ppp);
    }

    fn render(&mut self) {
        self.frame += 1.0;
        let now = web_time::Instant::now();
        // real elapsed time, clamped so a stall can't fling everything across the map;
        // this keeps the game running at the same pace on 60Hz and 120Hz displays alike.
        let dt = now.duration_since(self.last).as_secs_f32().clamp(0.0, 0.1);
        self.last = now;
        self.anim_t += dt;
        self.debug_cone = self.menu.settings.show_cone;
        if self.ss_on != self.menu.settings.hires { self.apply_targets(); }   // Graphics toggle → (de)supersample
        if self.capture.is_some() { self.render_capture(dt); return; }
        if self.prowl.is_some() { self.render_prowl(dt); return; }

        // cursor capture follows play: locked+hidden while playing, freed in menus / after a round
        let want_capture = !self.menu.open && self.game.outcome.is_none();
        self.set_capture(want_capture);

        if !self.menu.open {
            // movement is CAMERA-RELATIVE: rotate the raw WASD vector by the camera yaw
            // so "W" always drives the cat the way the camera is facing (was world-space,
            // which felt like the controls were rotated ~30° off).
            let (ix, iz) = self.keys.move_vec();
            let (mx, mz) = move_world(self.yaw, ix, iz);
            self.game.set_input(0, mx, mz, self.keys.crouch, self.keys.dodge || self.keys.mmiddle, self.keys.leap);
            self.game.set_ultimate(self.keys.ability);   // Q — the clan ultimate (only fires if the settings toggle is on)

            // flick controls: a fast mouse-flick + left-click performs the matching move
            // (up = pounce, down = hind-kick, sideways = roll); a bare click is a swat.
            // Right-click held is the low modifier (underbelly swipe). Screen-up maps to
            // the cat's forward since the follow-cam sits behind it.
            self.flick.0 *= 0.7; self.flick.1 *= 0.7;
            let fmag = (self.flick.0 * self.flick.0 + self.flick.1 * self.flick.1).sqrt();
            if self.keys.mleft && !self.mleft_prev {
                let fast = fmag > 14.0;
                let local = if fast { (self.flick.0 / fmag, -self.flick.1 / fmag) } else { (0.0, 0.0) };
                let mag = if fast { 1.0 } else { clawbound_rs::combat::FLICK_MIN + 0.1 };
                self.game.player_flick(local, mag, self.keys.mright);
            }
            self.mleft_prev = self.keys.mleft;

            // when the player's own cat is caught: a short slow-motion beat + a fade flash
            let alive = self.game.cats[0].alive;
            if self.prev_player_alive && !alive { self.death_t = 1.6; }
            self.prev_player_alive = alive;
            let step_dt = if self.death_t > 0.0 { dt * 0.35 } else { dt };
            self.death_t = (self.death_t - dt).max(0.0);
            self.game.step(step_dt);

            // round flow: hold the result a beat, then restart (or on 'R')
            if self.game.outcome.is_some() { self.over_timer += dt; } else { self.over_timer = 0.0; }
            if self.keys.restart || self.over_timer > 6.0 {
                let diff = self.game.difficulty;
                let clan = self.game.clan;
                let seed = (self.frame as u32).wrapping_mul(2_654_435_761) | 1;
                self.game = Game::new_match_seeded(self.kind, self.mode, seed);
                self.game.difficulty = diff;
                self.game.clan = clan;
                self.game.set_abilities_on(self.menu.settings.clan_abilities);
                self.over_timer = 0.0; self.keys.restart = false;
                self.death_t = 0.0; self.prev_player_alive = true;
            }
        }

        // follow the player cat (or the pack centroid if it's been caught)
        let follow = if self.game.cats[0].alive { (self.game.cats[0].x, self.game.cats[0].z) }
            else {
                let a: Vec<_> = self.game.cats.iter().filter(|c| c.alive).collect();
                if a.is_empty() { (self.game.hunter.x, self.game.hunter.z) }
                else { (a.iter().map(|c| c.x).sum::<f32>() / a.len() as f32, a.iter().map(|c| c.z).sum::<f32>() / a.len() as f32) }
            };
        let gy = clawbound_rs::world::height_at(follow.0, follow.1);
        self.focus = v3(follow.0, gy, follow.1);

        // rebuild + upload the moving entities (+ otherworld dressing during a shift)
        let nightmare = if self.game.hunter.kind.is_shadow() && self.game.shifted { 1.0f32 } else { 0.0 };
        let mut ents = crate::build_entities(&self.game, self.anim_t, self.debug_cone);
        if nightmare > 0.5 { ents.extend(&crate::build_nightmare_extras(self.focus)); }
        self.ent_vcount = ents.count();
        let bytes: &[u8] = bytemuck::cast_slice(&ents.verts);
        if (bytes.len() as u64) > self.ent_cap {
            // grow the buffer rather than blink every entity out of existence this frame
            self.ent_cap = (bytes.len() as u64).next_power_of_two();
            self.ent_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("entities"), size: self.ent_cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() { self.queue.write_buffer(&self.ent_vbuf, 0, bytes); }
        // death fade reuses the nightmare desaturate/vignette in the post shader
        let death = (self.death_t / 1.6).clamp(0.0, 1.0);
        let grim = nightmare.max(death * 0.9);
        self.queue.write_buffer(&self.fxbuf, 0, bytemuck::cast_slice(&[grim, 0.2f32, self.frame * 0.016, self.game.danger, 1.0 / self.scene_w, 1.0 / self.scene_h, self.scene_w, self.scene_h]));

        // third-person orbit positioned from mouse-driven yaw/pitch
        let (sy, cy) = (self.yaw.sin(), self.yaw.cos());
        let dist = 20.0;
        let horiz = self.pitch.cos() * dist;
        let height = self.pitch.sin() * dist;
        let eye = self.focus.add(v3(-sy * horiz, height, -cy * horiz));
        let target = self.focus.add(v3(0.0, 1.1, 0.0));
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let mvp = perspective(56f32.to_radians(), aspect, 0.1, 900.0).mul(&look_at(eye, target, v3(0.0, 1.0, 0.0)));
        let u = U { mvp: mvp.0, light: [0.4, 1.0, 0.35, 0.0], key: [1.05, 0.97, 0.85, 0.0], amb: [0.52, 0.53, 0.50, 0.0] };
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));

        let frame = match self.surface.get_current_texture() { Ok(f) => f, Err(_) => { self.surface.configure(&self.device, &self.config); return; } };
        let view = frame.texture.create_view(&Default::default());
        let bg = match self.game.outcome {
            Some(clawbound_rs::sim::Outcome::CatsWin) => wgpu::Color { r: 0.40, g: 0.66, b: 0.44, a: 1.0 },
            Some(clawbound_rs::sim::Outcome::HunterWins) => wgpu::Color { r: 0.62, g: 0.36, b: 0.32, a: 1.0 },
            None if self.game.shifted => wgpu::Color { r: 0.18, g: 0.10, b: 0.24, a: 1.0 },
            None => wgpu::Color { r: 0.86, g: 0.81, b: 0.70, a: 1.0 },
        };
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.scene_view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(bg), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth_view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None,
            });
            rp.set_pipeline(&self.scene_pipe);
            rp.set_bind_group(0, &self.ubg, &[]);
            rp.set_vertex_buffer(0, self.world_vbuf.slice(..));
            rp.draw(0..self.world_vcount, 0..1);
            if self.ent_vcount > 0 {
                rp.set_vertex_buffer(0, self.ent_vbuf.slice(..));
                rp.draw(0..self.ent_vcount, 0..1);
            }
        }
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
            });
            rp.set_pipeline(&self.post_pipe); rp.set_bind_group(0, &self.pbg, &[]); rp.draw(0..3, 0..1);
        }

        // ── egui overlay (menu) ──
        let items = self.menu.items();
        let (cursor, open, screen) = (self.menu.cursor, self.menu.open, self.menu.screen);
        let over = self.game.outcome;
        // HUD snapshot (copied out so the egui closure doesn't borrow self)
        let hunter_name = self.kind.params().name;
        let mode_name = match self.mode { clawbound_rs::sim::Mode::Survive => "Survive", _ => "Rendezvous" };
        let weather_line = {
            let w = &self.game.weather;
            let wind = w.total_wind();
            let windword = if wind > 0.85 { "gale" } else if wind > 0.5 { "brisk wind" } else if wind > 0.25 { "light wind" } else { "still air" };
            format!("{} · {} · {}", w.season.name(), w.sky.name(), windword)
        };
        let ct = clawbound_rs::clan::trait_of(self.game.clan);
        let clan_line = format!("{} — {}", ct.name, clawbound_rs::clan::specialty(self.game.clan));
        let clan_traits = [("climb", ct.climb, egui::Color32::from_rgb(90, 160, 84)), ("sprint", ct.sprint, egui::Color32::from_rgb(216, 165, 60)),
                           ("swim", ct.swim, egui::Color32::from_rgb(61, 127, 216)), ("stealth", ct.stealth, egui::Color32::from_rgb(154, 108, 255)),
                           ("scent", ct.scent, egui::Color32::from_rgb(224, 128, 64))];
        let clan_color = ct.color;
        // directional threat: bearing to the hunter relative to the camera, for an
        // edge marker that tells you which way the danger is closing from.
        let (thx, thz) = (self.game.hunter.x, self.game.hunter.z);
        let (tpx, tpz) = (self.game.cats[0].x, self.game.cats[0].z);
        let (dxh, dzh) = (thx - tpx, thz - tpz);
        let (fwx, fwz) = (self.yaw.sin(), self.yaw.cos());
        let sxh = dxh * fwz - dzh * fwx;                 // screen-right component
        let syh = dxh * fwx + dzh * fwz;                 // depth (ahead) component
        let threat_bearing = sxh.atan2(syh);
        let threat = self.game.danger;
        let noise = self.game.noise_flash;
        let combo_flash = self.game.combo_flash;
        let combo_name = self.game.combo_name;
        let move_name = self.game.player_move.map(|(m, _)| m.name());
        let abilities_on = self.game.abilities_on;
        let (ult_name, ult_blurb, _ult_cd_max, _ult_dur) = self.game.clan_ultimate();
        let (pv_name, pv_blurb) = self.game.clan_passive();
        let (ult_ready, ult_cd_left) = (self.game.ultimate_ready(), self.game.ult_cd);
        // world lore for the HUD: the pack roster (name · rank · fate), the region and a
        // line about it, the time of day, and a tenet of the code.
        let region = self.game.region();
        let region_lore = self.game.region_lore();
        let scent_line = {
            let ws = self.game.read_wind();
            if ws.predator_reek > 0.22 { format!("the reek of the {} is on the wind", hunter_name.to_lowercase()) }
            else if let Some((_, _, _, s)) = ws.prey { if s > 0.15 { format!("{}-scent on the breeze — upwind", ws.prey_name) } else { String::new() } }
            else { String::new() }
        };
        let day_phase = clawbound_rs::lore::day_phase(0.25 + self.game.time / 480.0);
        let code_line = clawbound_rs::lore::code_line((self.game.tick / 600).wrapping_add(self.game.clan as u32));
        let roster: Vec<(String, &'static str, bool, bool)> = self.game.cats.iter().enumerate()
            .map(|(i, c)| (self.game.cat_name(i).to_string(), self.game.roster.get(i).map(|(_, r)| r.title()).unwrap_or("warrior"), c.alive, c.safe))
            .collect();
        let player_name = self.game.cat_name(0).to_string();
        let death_show = self.death_t;
        let blessing = if death_show > 0.0 { clawbound_rs::lore::death_blessing(&player_name) } else { String::new() };
        let p = &self.game.cats[0];
        let stamina = p.stamina;
        let player_alive = p.alive;
        let pstate = if !p.alive { "caught" } else if p.is_carried() { "carried off!" } else if p.crippled_t > 0.0 { "limping" }
            else if p.is_leaping() { "leaping" } else if p.safe { "safe" } else if p.climbing { "climbing" }
            else if p.crouched { "hidden" } else if p.moving { "running" } else { "still" };
        let alive_n = self.game.cats.iter().filter(|c| c.alive).count();
        let total = self.game.cats.len();
        let escaped = self.game.escaped;
        let over_timer = self.over_timer;
        let death_t = self.death_t;
        let ppp = self.egui_ctx.pixels_per_point();
        let raw = self.egui_state.take_egui_input(&self._window);
        let full = self.egui_ctx.run(raw, |ctx| {
            use clawbound_rs::menu::Screen;
            use egui::{Align2, Color32, RichText};
            if open {
                egui::Area::new("clawbound_menu".into()).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).fill(Color32::from_rgba_unmultiplied(18, 14, 26, 235)).inner_margin(egui::Margin::same(24.0)).show(ui, |ui| {
                        ui.set_min_width(320.0);
                        ui.label(RichText::new("C L A W B O U N D").size(30.0).strong().color(Color32::from_rgb(230, 210, 170)));
                        let sub = match screen { Screen::Title => "", Screen::Play => "Single Player", Screen::Settings => "Settings", Screen::Keybinds => "Keybinds" };
                        if !sub.is_empty() { ui.label(RichText::new(sub).size(14.0).color(Color32::from_rgb(150, 130, 180))); }
                        ui.add_space(10.0);
                        for (i, it) in items.iter().enumerate() {
                            let sel = i == cursor;
                            let t = RichText::new(format!("{} {}", if sel { "▶" } else { "  " }, it)).size(20.0)
                                .color(if sel { Color32::from_rgb(255, 220, 120) } else { Color32::from_rgb(205, 205, 210) });
                            ui.label(t);
                        }
                        ui.add_space(10.0);
                        ui.label(RichText::new("↑↓ move   ←→ change   Enter select   Esc back").size(12.0).weak());
                    });
                });
            } else if let Some(o) = over {
                // ── game-over screen: dim the world, then the verdict + a restart prompt ──
                let a = (over_timer / 0.6).clamp(0.0, 1.0);
                egui::Area::new("dim".into()).order(egui::Order::Background).show(ctx, |ui| {
                    ui.painter().rect_filled(ctx.screen_rect(), 0.0, Color32::from_black_alpha((170.0 * a) as u8));
                });
                let (txt, col) = match o {
                    clawbound_rs::sim::Outcome::CatsWin => ("CATS ESCAPE", Color32::from_rgb(150, 230, 160)),
                    clawbound_rs::sim::Outcome::HunterWins => ("THE HUNTER WINS", Color32::from_rgb(235, 130, 120)),
                };
                egui::Area::new("result".into()).anchor(Align2::CENTER_CENTER, [0.0, -10.0]).show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(txt).size(46.0).strong().color(col));
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("{escaped} of {total} cats made it out"))
                            .size(18.0).color(Color32::from_rgb(220, 214, 205)));
                        ui.add_space(14.0);
                        ui.label(RichText::new("press  R  for another round").size(16.0)
                            .color(Color32::from_rgb(255, 220, 120)));
                    });
                });
            } else {
                // ── in-game HUD ──
                egui::Area::new("hud".into()).anchor(Align2::LEFT_TOP, [16.0, 14.0]).show(ctx, |ui| {
                    egui::Frame::none().fill(Color32::from_rgba_unmultiplied(14, 12, 20, 150))
                        .inner_margin(egui::Margin::symmetric(12.0, 9.0)).rounding(6.0).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.label(RichText::new(format!("Hunter: {hunter_name}   ·   {mode_name}"))
                            .size(15.0).strong().color(Color32::from_rgb(235, 200, 170)));
                        ui.label(RichText::new(format!("Cats remaining  {alive_n}/{total}"))
                            .size(14.0).color(Color32::from_rgb(200, 220, 205)));
                        ui.add(egui::ProgressBar::new(stamina).desired_width(168.0)
                            .fill(if stamina > 0.25 { Color32::from_rgb(120, 200, 150) } else { Color32::from_rgb(215, 120, 90) })
                            .text(RichText::new("stamina").size(11.0).color(Color32::WHITE)));
                        let sc = if player_alive { Color32::from_rgb(180, 200, 235) } else { Color32::from_rgb(235, 130, 120) };
                        ui.label(RichText::new(format!("{player_name} — {pstate}")).size(13.0).color(sc));
                        ui.label(RichText::new(&weather_line).size(12.0).color(Color32::from_rgb(160, 178, 196)));
                        ui.add_space(3.0);
                        ui.label(RichText::new(&clan_line).size(13.0).strong().color(Color32::from_rgb(((clan_color >> 16) & 255) as u8, ((clan_color >> 8) & 255) as u8, (clan_color & 255) as u8)));
                        for (label, v, col) in clan_traits {
                            ui.add(egui::ProgressBar::new(((v - 0.6) / 1.05).clamp(0.05, 1.0)).desired_width(150.0).fill(col)
                                .text(RichText::new(label).size(10.0).color(Color32::WHITE)));
                        }
                        if abilities_on {
                            ui.add_space(3.0);
                            ui.label(RichText::new(format!("Passive · {pv_name}")).size(12.0).strong().color(Color32::from_rgb(150, 220, 235)));
                            ui.label(RichText::new(pv_blurb).size(10.0).italics().color(Color32::from_rgb(150, 150, 165)));
                            let (ustate, ucol) = if ult_ready { ("READY".to_string(), Color32::from_rgb(150, 230, 160)) } else { (format!("{:.0}s", ult_cd_left.ceil()), Color32::from_rgb(180, 180, 190)) };
                            ui.label(RichText::new(format!("ULT (Q) · {ult_name}  [{ustate}]")).size(12.0).strong().color(if ult_ready { Color32::from_rgb(255, 214, 120) } else { ucol }));
                            ui.label(RichText::new(ult_blurb).size(10.0).italics().color(Color32::from_rgb(150, 150, 165)));
                        }
                    });
                });
                egui::Area::new("hint".into()).anchor(Align2::CENTER_BOTTOM, [0.0, -12.0]).show(ctx, |ui| {
                    let hint = if abilities_on {
                        "WASD move · mouse look · Shift hide · Space/M-click roll · F leap · flick+L-click move (R-click low) · Q ultimate · Esc menu"
                    } else {
                        "WASD move · mouse look · Shift hide · Space/M-click roll · F leap · flick+L-click move (R-click low) · Esc menu"
                    };
                    ui.label(RichText::new(hint).size(12.0).color(Color32::from_rgba_unmultiplied(235, 230, 220, 190)));
                });
                // world panel (top-right): the region, a line about it, the hour, and the
                // named pack with each cat's rank and fate — the clan you're fighting for.
                egui::Area::new("world".into()).anchor(Align2::RIGHT_TOP, [-14.0, 14.0]).show(ctx, |ui| {
                    egui::Frame::none().fill(Color32::from_rgba_unmultiplied(14, 13, 20, 155)).inner_margin(egui::Margin::symmetric(12.0, 9.0)).rounding(7.0).show(ui, |ui| {
                        ui.set_max_width(230.0);
                        ui.label(RichText::new(format!("{region} · {day_phase}")).size(13.0).strong().color(Color32::from_rgb(210, 205, 180)));
                        ui.label(RichText::new(region_lore).size(11.0).italics().color(Color32::from_rgb(160, 160, 172)));
                        if !scent_line.is_empty() {
                            ui.label(RichText::new(format!("\u{2022} {scent_line}")).size(11.0).color(Color32::from_rgb(200, 180, 130)));
                        }
                        ui.add_space(4.0);
                        for (nm, rank, alive, safe) in &roster {
                            let (col, tag) = if !*alive { (Color32::from_rgb(150, 90, 86), "lost") }
                                else if *safe { (Color32::from_rgb(120, 200, 130), "safe") }
                                else { (Color32::from_rgb(205, 210, 220), *rank) };
                            ui.label(RichText::new(format!("{nm} — {tag}")).size(12.0).color(col));
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("\u{201c}{code_line}\u{201d}")).size(10.0).italics().color(Color32::from_rgb(140, 140, 155)));
                    });
                });
                // a farewell over the fallen — the clan's blessing as you go down
                if death_show > 0.0 && !blessing.is_empty() {
                    egui::Area::new("blessing".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, 40.0]).show(ctx, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(RichText::new(&blessing).size(15.0).italics().color(Color32::from_rgba_unmultiplied(230, 224, 210, (200.0 * (death_show / 1.6).min(1.0)) as u8)));
                    });
                }
                // combat: the current move, and a combo banner when a chain lands
                if let Some(mn) = move_name {
                    egui::Area::new("move".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, 46.0]).show(ctx, |ui| {
                        ui.label(RichText::new(mn).size(15.0).strong().color(Color32::from_rgb(255, 240, 200)));
                    });
                }
                if combo_flash > 0.05 {
                    egui::Area::new("combo".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
                        ui.label(RichText::new(combo_name).size(20.0 + combo_flash * 12.0).strong()
                            .color(Color32::from_rgba_unmultiplied(255, 210, 90, (255.0 * combo_flash.min(1.0)) as u8)));
                    });
                }
                // twig-snap cue: a sharp flash when you crack a twig underfoot
                if noise > 0.05 && player_alive {
                    egui::Area::new("snap".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, 70.0]).show(ctx, |ui| {
                        ui.label(RichText::new("*SNAP!*").size(15.0 + noise * 7.0).strong()
                            .color(Color32::from_rgba_unmultiplied(255, 220, 120, (255.0 * noise) as u8)));
                    });
                }
                // directional threat: a red marker at the screen edge in the hunter's
                // direction, brighter and larger the closer the danger.
                if threat > 0.28 && player_alive {
                    let a = ((threat - 0.28) / 0.72).clamp(0.0, 1.0);
                    let r = 235.0;
                    let off = [threat_bearing.sin() * r, -threat_bearing.cos() * r];
                    egui::Area::new("threat".into()).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, off).show(ctx, |ui| {
                        egui::Frame::none().fill(Color32::from_rgba_unmultiplied(210, 48, 40, (110.0 + 120.0 * a) as u8))
                            .rounding(10.0).inner_margin(egui::Margin::symmetric(9.0, 4.0)).show(ui, |ui| {
                            ui.label(RichText::new("THREAT").size(11.0 + a * 5.0).strong().color(Color32::WHITE));
                        });
                    });
                }
                // caught: a red flash while the slow-motion death beat plays out
                if !player_alive && death_t > 0.0 {
                    let a = (death_t / 1.6).clamp(0.0, 1.0);
                    egui::Area::new("caught".into()).order(egui::Order::Background).show(ctx, |ui| {
                        ui.painter().rect_filled(ctx.screen_rect(), 0.0, Color32::from_rgba_unmultiplied(60, 10, 12, (120.0 * a) as u8));
                    });
                    egui::Area::new("caughttxt".into()).anchor(Align2::CENTER_CENTER, [0.0, -40.0]).show(ctx, |ui| {
                        ui.label(RichText::new("CAUGHT").size(40.0).strong()
                            .color(Color32::from_rgba_unmultiplied(240, 150, 140, (255.0 * a) as u8)));
                    });
                }
            }
        });
        self.egui_state.handle_platform_output(&self._window, full.platform_output);
        let prims = self.egui_ctx.tessellate(full.shapes, ppp);
        self.egui_render.render(&self.device, &self.queue, &mut enc, &view, [self.config.width, self.config.height], ppp, &prims, &full.textures_delta);

        self.queue.submit(Some(enc.finish()));
        frame.present();
    }
}

fn run_loop(event_loop: EventLoop<()>, window: Arc<Window>, mut state: State) {
    event_loop.run(move |event, elwt| match event {
        Event::WindowEvent { window_id, event } if window_id == window.id() => {
            let _ = state.egui_state.on_window_event(&window, &event);
            match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(size) => state.resize(size),
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        let down = key_event.state == ElementState::Pressed;
                        if state.menu.open {
                            if down {
                                use clawbound_rs::menu::MenuCmd;
                                if state.menu.rebinding.is_some() {
                                    state.menu.feed_rebind(&format!("{:?}", code).replace("Key", "").replace("Digit", ""));
                                } else {
                                    match code {
                                        KeyCode::ArrowUp | KeyCode::KeyW => state.menu.up(),
                                        KeyCode::ArrowDown | KeyCode::KeyS => state.menu.down(),
                                        KeyCode::ArrowLeft | KeyCode::KeyA => state.menu.adjust(-1),
                                        KeyCode::ArrowRight | KeyCode::KeyD => state.menu.adjust(1),
                                        KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => match state.menu.activate() {
                                            MenuCmd::StartSingle(k, m) => { state.prowl = None; state.capture = None; state.tutorial = None; state.kind = k; state.mode = m; let seed = (state.frame as u32).wrapping_mul(2_654_435_761) | 1; state.game = Game::new_match_seeded(k, m, seed); state.game.difficulty = clawbound_rs::menu::difficulty_mult(state.menu.settings.difficulty); state.game.clan = state.menu.sel_clan; state.game.set_abilities_on(state.menu.settings.clan_abilities); state.over_timer = 0.0; }
                                            MenuCmd::StartMode(g, k, c) => {
                                                use clawbound_rs::menu::GameMode;
                                                state.prowl = None; state.capture = None; state.tutorial = None;
                                                let diff = clawbound_rs::menu::difficulty_mult(state.menu.settings.difficulty);
                                                match g {
                                                    GameMode::Prowl => { let mut p = clawbound_rs::prowl::Prowl::new(c, k); p.difficulty = diff; p.set_fixed_wind(state.menu.settings.prowl_fixed_wind); state.prowl = Some(p); }
                                                    GameMode::Capture => { state.capture = Some(clawbound_rs::capture::Capture::new(c)); }
                                                    _ => { state.menu.open = true; }
                                                }
                                            }
                                            MenuCmd::StartTutorial => {
                                                state.capture = None;
                                                let mut p = clawbound_rs::prowl::Prowl::new(clawbound_rs::class::Class::Cat, clawbound_rs::hunter::Kind::Fox);
                                                p.set_tutorial_mode(true);
                                                state.prowl = Some(p);
                                                state.tutorial = Some(TutorialRun { step: 0, intro_t: 4.0, advance_flash: 0.0 });
                                            }
                                            MenuCmd::Quit => elwt.exit(),
                                            MenuCmd::Close => state.menu.open = false,
                                            MenuCmd::Multiplayer | MenuCmd::None => {}
                                        },
                                        KeyCode::Escape => { if state.menu.back() == MenuCmd::Close { state.menu.open = false; } }
                                        _ => {}
                                    }
                                }
                            }
                        } else if code == KeyCode::Escape && down {
                            state.menu.open = true; state.menu.screen = clawbound_rs::menu::Screen::Title; state.menu.cursor = 0;
                        } else if down && (code == KeyCode::Enter || code == KeyCode::NumpadEnter)
                            && state.tutorial.as_ref().map(|t| t.step == TUTORIAL_STEPS.len() - 1).unwrap_or(false) {
                            state.tutorial = None; state.prowl = None;
                            state.menu.open = true; state.menu.screen = clawbound_rs::menu::Screen::Title; state.menu.cursor = 0;
                        } else {
                            if down {
                                use clawbound_rs::hunter::Kind as HK;
                                let pick = match code {
                                    KeyCode::Digit1 => Some(HK::Fox), KeyCode::Digit2 => Some(HK::Dog),
                                    KeyCode::Digit3 => Some(HK::Badger), KeyCode::Digit4 => Some(HK::Hawk),
                                    KeyCode::Digit5 => Some(HK::Snake), KeyCode::Digit6 => Some(HK::Shadow), _ => None,
                                };
                                if let Some(k) = pick { state.kind = k; state.game = Game::new_match(k, state.mode); state.over_timer = 0.0; }
                                if code == KeyCode::KeyV { state.menu.settings.show_cone = !state.menu.settings.show_cone; }
                                if code == KeyCode::KeyM {
                                    use clawbound_rs::sim::Mode;
                                    state.mode = if state.mode == Mode::Rendezvous { Mode::Survive } else { Mode::Rendezvous };
                                    state.game = Game::new_match(state.kind, state.mode); state.over_timer = 0.0;
                                }
                            }
                            state.keys.set(code, down, &state.menu.settings.keybinds);
                        }
                    }
                }
                WindowEvent::MouseInput { state: btn_state, button, .. } => {
                    let down = btn_state == ElementState::Pressed;
                    match button {
                        MouseButton::Left => state.keys.mleft = down,
                        MouseButton::Middle => state.keys.mmiddle = down,
                        MouseButton::Right => state.keys.mright = down,
                        _ => {}
                    }
                }
                WindowEvent::RedrawRequested => { state.render(); }
                _ => {}
            }
        }
        Event::DeviceEvent { event: DeviceEvent::MouseMotion { delta }, .. } => {
            if state.captured { state.on_mouse(delta.0 as f32, delta.1 as f32); }
        }
        Event::AboutToWait => window.request_redraw(),
        _ => {}
    }).unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run() {
    let _ = env_logger::try_init();
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new().with_title("CLAWBOUND").build(&event_loop).unwrap());
    let state = pollster::block_on(State::new(window.clone()));
    run_loop(event_loop, window, state);
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web_start() {
    use wasm_bindgen::JsCast;
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    let event_loop = EventLoop::new().unwrap();
    // reuse the <canvas id="clawbound"> from index.html
    let canvas = web_sys::window().unwrap().document().unwrap()
        .get_element_by_id("clawbound").unwrap()
        .dyn_into::<web_sys::HtmlCanvasElement>().unwrap();
    use winit::platform::web::WindowBuilderExtWebSys;
    let window = Arc::new(WindowBuilder::new().with_canvas(Some(canvas)).build(&event_loop).unwrap());
    wasm_bindgen_futures::spawn_local(async move {
        let mut state = State::new(window.clone()).await;
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn(move |event, elwt| match event {
            Event::WindowEvent { window_id, ref event } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(size) => state.resize(*size),
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        state.keys.set(code, key_event.state == ElementState::Pressed);
                    }
                }
                WindowEvent::RedrawRequested => { state.render(); }
                _ => {}
            },
            Event::AboutToWait => window.request_redraw(),
            _ => {}
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{cam_forward_right, move_world};
    fn approx(a: (f32, f32), b: (f32, f32)) {
        assert!((a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5, "got {:?}, want {:?}", a, b);
    }

    // W must drive the cat the way the camera faces; A/D must not be swapped.
    // Basis is checked against the right-handed look_at (screen-right = forward x up),
    // so at yaw 0 the camera sits on -z looking +z: W->+z (into screen), D->-x (screen right).
    #[test]
    fn wasd_is_camera_relative() {
        approx(move_world(0.0, 0.0, 1.0), (0.0, 1.0));    // W -> forward / into screen
        approx(move_world(0.0, 0.0, -1.0), (0.0, -1.0));  // S -> back
        approx(move_world(0.0, 1.0, 0.0), (-1.0, 0.0));   // D -> screen-right (-x in RH)
        approx(move_world(0.0, -1.0, 0.0), (1.0, 0.0));   // A -> screen-left

        // rotate the camera a quarter turn: forward becomes +x, so W now pushes +x.
        let q = std::f32::consts::FRAC_PI_2;
        approx(move_world(q, 0.0, 1.0), (1.0, 0.0));      // W -> +x
        approx(move_world(q, 1.0, 0.0), (0.0, 1.0));      // D -> +z

        // forward and right stay orthonormal at any yaw
        let (f, r) = cam_forward_right(0.9);
        assert!((f.0 * f.0 + f.1 * f.1 - 1.0).abs() < 1e-5);
        assert!((r.0 * r.0 + r.1 * r.1 - 1.0).abs() < 1e-5);
        assert!((f.0 * r.0 + f.1 * r.1).abs() < 1e-5, "forward and right must be perpendicular");
    }
}
