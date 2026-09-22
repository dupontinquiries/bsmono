//! tideshell — the shell itself, on layer-shell surfaces (no eframe).
//!
//!   tideshell [--trace] [--layout right-bar]
//!
//! Env: `TIDESHELL_SHOW_STRIPS=1` tints the edge strips so they can be seen.
//! `TIDESHELL_PERSIST=0` stops it writing ~/.config/tideshell/prefs.
//! `TIDESHELL_TRACE=stderr` streams the engine trace (gestures, actions,
//! transport outcomes, EI status) to stderr, for watching it over ssh.
//!
//! Loop: wait for Wayland input (16 ms while anything is moving, 250 ms
//! idle) -> strips feed the engine's recognizer -> the overlay, if needed,
//! gets an egui frame through ShellView -> Actions go to the verified
//! transports (zcosmic toplevel requests, EI, hatch, rusty-board).

use std::collections::HashMap;
use std::time::Instant;

use egui::{Pos2, Rect, Vec2};
use tideshell_core::{Action, Shell, TouchEvent};
use tideshell_egui::effects::{self, Executor, Outcome, System};
use tideshell_egui::layer::{shape_for, CursorWant, Gl, Input, Io, Role, POINTER_SLOT};
use tideshell_egui::{AppTile, FrameInfo, ShellView};

const STRIP_PX: u32 = 28;

fn clock() -> String {
    let (h, m) = tideshell_egui::sensors::local_hm();
    format!("{h:02}:{m:02}")
}

struct Shellbox {
    shell: Shell,
    view: ShellView,
    system: System,
    windows: Option<tideshell_egui::wayland::Windows>,
    ei: Option<tideshell_egui::ei::EiPointer>,
    ei_status: Option<tideshell_egui::ei::Status>,
    persist: bool,
    ambient: tideshell_egui::sensors::Ambient,
    backlight: tideshell_egui::sensors::BacklightOut,
    sensor_status: (String, String),
}

impl Shellbox {
    fn execute(&mut self, actions: Vec<Action>) {
        for a in actions {
            let o = match &a {
                Action::SavePrefs if self.persist => effects::save_prefs(&self.shell.prefs),
                Action::SavePrefs => Outcome::Done("prefs not persisted (TIDESHELL_PERSIST=0)".into()),
                Action::Pointer(p) => match &self.ei {
                    Some(ei) => {
                        ei.send(*p);
                        continue;
                    }
                    None => Outcome::Unwired("no EI connection"),
                },
                // Auto-brightness ramps send many of these: silent. Which
                // backlight path works (or none) is in the status lines.
                Action::SetSlider(tideshell_core::control_center::SliderKind::Brightness, v) => {
                    self.backlight.set(*v);
                    continue;
                }
                // The overlay IS the trackpad surface here; nothing to execute.
                Action::ShowTrackpad(on) => Outcome::Done(format!("trackpad {}", if *on { "on" } else { "off" })),
                _ => match self.windows.as_mut() {
                    Some(w) => tideshell_egui::wayland::Live { windows: w, fallback: &mut self.system }.run(&a),
                    None => self.system.run(&a),
                },
            };
            self.shell.note(o.describe());
        }
    }

    fn poll_world(&mut self) {
        for (o, undo) in self.system.reap() {
            self.shell.note(o.describe());
            if undo == Some(effects::Undo::KeyboardShown) {
                self.shell.set_keyboard_visible(false);
                self.shell.note("keyboard mode off: it never appeared");
            }
        }
        if let Some(w) = self.windows.as_mut() {
            if let Some(cards) = w.poll() {
                let more = self.shell.set_windows(cards);
                self.execute(more);
            }
        }
        let st = (self.ambient.status(), self.backlight.status());
        if st != self.sensor_status {
            if st.0 != self.sensor_status.0 {
                self.shell.note(st.0.clone());
            }
            if st.1 != self.sensor_status.1 {
                self.shell.note(st.1.clone());
            }
            self.sensor_status = st;
        }
        if let Some(ei) = &self.ei {
            let st = ei.status();
            if self.ei_status.as_ref() != Some(&st) {
                self.shell.note(format!("ei: {st:?}"));
                self.ei_status = Some(st);
            }
        }
    }
}

/// A contact on the overlay, as egui wants it: a Touch event (ShellView's
/// input routing, multi-touch) plus the pointer events egui widgets react
/// to — the same pair egui-winit emits for a touchscreen. The pointer slot is
/// a mouse and gets pointer events only.
pub fn contact_events(slot: i32, phase: egui::TouchPhase, pos: Pos2) -> Vec<egui::Event> {
    let mut v = Vec::new();
    if slot != POINTER_SLOT {
        v.push(egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(slot as u64), phase, pos, force: None });
    }
    let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    match phase {
        egui::TouchPhase::Start => {
            v.push(egui::Event::PointerMoved(pos));
            v.push(button(true));
        }
        egui::TouchPhase::Move => v.push(egui::Event::PointerMoved(pos)),
        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
            v.push(button(false));
            if slot != POINTER_SLOT {
                v.push(egui::Event::PointerGone);
            }
        }
    }
    v
}

/// Print trace lines not printed yet. The trace is a bounded ring, so match
/// on content: find the last printed line and print what follows it.
fn print_new_trace(shell: &Shell, printed: &mut Vec<String>) {
    let lines: Vec<&String> = shell.trace().collect();
    let start = printed.last().and_then(|last| lines.iter().rposition(|l| *l == last)).map(|i| i + 1).unwrap_or(0);
    for l in &lines[start..] {
        eprintln!("trace {l}");
        printed.push((*l).clone());
    }
    if printed.len() > 200 {
        printed.drain(..100);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let persist = std::env::var("TIDESHELL_PERSIST").map(|v| v != "0").unwrap_or(true);
    let (mut prefs, errs) = effects::load_prefs();
    if let Some(v) = args.iter().position(|a| a == "--layout").and_then(|i| args.get(i + 1)) {
        if let Err(e) = prefs.apply("shell.layout", v) {
            eprintln!("{e}");
        }
    }

    let mut io = Io::connect(STRIP_PX, std::env::var("TIDESHELL_SHOW_STRIPS").is_ok_and(|v| v == "1")).unwrap_or_else(|e| {
        eprintln!("tideshell: {e}");
        std::process::exit(1)
    });
    let mut gl = Gl::new(io.display_ptr()).unwrap_or_else(|e| {
        eprintln!("tideshell: EGL: {e}");
        std::process::exit(1)
    });

    let mut sb = Shellbox {
        shell: Shell::new(prefs, (0.0, 0.0)),
        view: ShellView::default(),
        system: System::new(),
        windows: None,
        ei: Some(tideshell_egui::ei::EiPointer::spawn()),
        ei_status: None,
        persist,
        ambient: tideshell_egui::sensors::Ambient::spawn("/sys".into(), zbus::blocking::Connection::system().ok()),
        backlight: tideshell_egui::sensors::BacklightOut::spawn(std::path::Path::new("/sys"), zbus::blocking::Connection::system().ok()),
        sensor_status: (String::new(), String::new()),
    };
    // Auto-brightness ramps from the real current level, or not at all.
    if let Some(l) = sb.backlight.initial {
        sb.shell.set_current_brightness(l);
    }
    sb.view.show_trace = has("--trace");
    sb.view.bar_surface = true;
    // A previous run that died with the fat cursor on left cosmic-comp's
    // cursor hidden; put the user's setting back before anything else.
    if let Some(o) = effects::cosmic_config_home().and_then(|h| effects::recover_system_cursor(&h)) {
        sb.shell.note(o.describe());
    }
    for e in errs {
        sb.shell.note(format!("prefs: {e}"));
    }
    match tideshell_egui::wayland::Windows::connect() {
        Ok(w) => sb.windows = Some(w),
        Err(e) => sb.shell.note(format!("wayland toplevels: {e:?}")),
    }
    let apps: Vec<AppTile> = sb.system.launcher.as_ref().map(|l| effects::hatch_tiles(l, 8)).unwrap_or_default();

    let ctx = egui::Context::default();
    let ui_prefs = settings_kit::Prefs::default();
    uikit_egui::apply_prefs(&ctx, &ui_prefs);
    let palette = uikit_egui::palette_colors_ctx(&ctx, &ui_prefs.palette);
    // Transparent panels: the overlay composites over the apps.
    ctx.global_style_mut(|s| s.visuals.panel_fill = egui::Color32::TRANSPARENT);
    // The bar's surface has its own egui context (own textures, own input).
    let bar_ctx = egui::Context::default();
    uikit_egui::apply_prefs(&bar_ctx, &ui_prefs);
    let _ = uikit_egui::palette_colors_ctx(&bar_ctx, &ui_prefs.palette);
    let mut bar_events: Vec<egui::Event> = Vec::new();
    let mut bar_pos: HashMap<i32, Pos2> = HashMap::new();

    let t0 = Instant::now();
    let mut overlay_events: Vec<egui::Event> = Vec::new();
    let mut contact_pos: HashMap<i32, Pos2> = HashMap::new();
    let mut last_region: Option<Option<Vec<Rect>>> = None;
    let mut last_poll = 0u64;
    let trace_stderr = std::env::var("TIDESHELL_TRACE").is_ok_and(|v| v == "stderr");
    let mut printed: Vec<String> = Vec::new();
    loop {
        if trace_stderr {
            print_new_trace(&sb.shell, &mut printed);
        }
        let busy = !sb.shell.recognizer.is_idle() || sb.view.needs_overlay(&sb.shell) || sb.shell.switcher.is_streaking();
        let input = io.dispatch(if busy { 16 } else { 250 }).unwrap_or_else(|e| {
            eprintln!("tideshell: wayland: {e}");
            std::process::exit(1)
        });
        let now_ms = t0.elapsed().as_millis() as u64;
        sb.shell.set_clock(tideshell_egui::sensors::local_minute());
        while let Ok(lux) = sb.ambient.rx.try_recv() {
            sb.shell.on_lux(lux, now_ms);
        }
        if let Some(screen) = io.screen() {
            if screen != sb.shell.screen {
                sb.shell.set_screen(screen);
            }
        }
        io.set_right_enabled(sb.shell.recognizer.cfg.right_enabled);

        // Strips -> recognizer (screen coords). Overlay -> egui events.
        let mut actions = Vec::new();
        for ev in input {
            match ev {
                // The bar is a widget surface, not an edge: its input goes to
                // its own egui context (it owns the right edge; see prefs).
                Input::Down { role: Role::Bar, slot, x, y, .. } => {
                    bar_pos.insert(slot, Pos2::new(x, y));
                    bar_events.extend(contact_events(slot, egui::TouchPhase::Start, Pos2::new(x, y)));
                }
                Input::Move { role: Role::Bar, slot, x, y, .. } => {
                    bar_pos.insert(slot, Pos2::new(x, y));
                    bar_events.extend(contact_events(slot, egui::TouchPhase::Move, Pos2::new(x, y)));
                }
                Input::Up { role: Role::Bar, slot, .. } => {
                    let pos = bar_pos.remove(&slot).unwrap_or(Pos2::ZERO);
                    bar_events.extend(contact_events(slot, egui::TouchPhase::End, pos));
                }
                Input::Hover { role: Role::Bar, x, y } => bar_events.push(egui::Event::PointerMoved(Pos2::new(x, y))),
                Input::Down { role: Role::Overlay, slot, x, y, .. } => {
                    contact_pos.insert(slot, Pos2::new(x, y));
                    overlay_events.extend(contact_events(slot, egui::TouchPhase::Start, Pos2::new(x, y)));
                }
                Input::Move { role: Role::Overlay, slot, x, y, .. } => {
                    contact_pos.insert(slot, Pos2::new(x, y));
                    overlay_events.extend(contact_events(slot, egui::TouchPhase::Move, Pos2::new(x, y)));
                }
                Input::Up { role: Role::Overlay, slot, .. } => {
                    let pos = contact_pos.remove(&slot).unwrap_or(Pos2::ZERO);
                    overlay_events.extend(contact_events(slot, egui::TouchPhase::End, pos));
                }
                Input::Hover { role: Role::Overlay, x, y } => overlay_events.push(egui::Event::PointerMoved(Pos2::new(x, y))),
                Input::Hover { .. } => {}
                Input::Down { role, slot, x, y, .. } => {
                    if let Some((sx, sy)) = io.to_screen(role, x, y) {
                        actions.extend(sb.shell.on_edge_touch(TouchEvent::Down { slot, x: sx, y: sy, t: now_ms }));
                    }
                }
                Input::Move { role, slot, x, y, .. } => {
                    if let Some((sx, sy)) = io.to_screen(role, x, y) {
                        actions.extend(sb.shell.on_edge_touch(TouchEvent::Move { slot, x: sx, y: sy, t: now_ms }));
                    }
                }
                Input::Up { slot, .. } => actions.extend(sb.shell.on_edge_touch(TouchEvent::Up { slot, t: now_ms })),
                Input::Cancel => {
                    actions.extend(sb.shell.on_edge_touch(TouchEvent::Cancel));
                    contact_pos.clear();
                    overlay_events.push(egui::Event::PointerGone);
                }
            }
        }
        actions.extend(sb.shell.tick(now_ms));
        sb.execute(actions);

        if now_ms.saturating_sub(last_poll) >= 100 {
            last_poll = now_ms;
            sb.poll_world();
        }

        // Bar: its own surface with an exclusive zone (right-bar layout only).
        let geom = ShellView::bar_geometry(&sb.shell);
        io.ensure_bar(geom.is_some(), geom.map_or(0, |g| g.0), geom.map_or(0, |g| g.1));
        let bar_now = io.bar();
        // Maximized windows still laid out under the bar get re-maximized
        // (see Shell::reflow_for_zone for why cosmic-comp needs this).
        if let (Some(_), Some((_, reserve))) = (&bar_now, geom) {
            let out = sb.shell.reflow_for_zone(reserve as f32);
            sb.execute(out);
        }
        match bar_now {
            Some(t) => {
                t.prepare(&mut io);
                let size_px = t.size_px();
                let (w, h) = t.logical;
                if gl.bind(Role::Bar, &t.surface, size_px).is_ok() {
                    let mut raw = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32))),
                        time: Some(t0.elapsed().as_secs_f64()),
                        events: std::mem::take(&mut bar_events),
                        focused: true,
                        ..Default::default()
                    };
                    raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(t.scale);
                    let info = FrameInfo { apps: &apps, clock: clock(), battery: None };
                    let mut bar_actions = Vec::new();
                    let out = bar_ctx.run_ui(raw, |ui| {
                        egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| {
                            bar_actions = sb.view.bar_ui(ui, &mut sb.shell, &palette, &info);
                        });
                    });
                    sb.execute(bar_actions);
                    let prims = bar_ctx.tessellate(out.shapes, out.pixels_per_point);
                    if let Err(e) = gl.paint(Role::Bar, size_px, out.pixels_per_point, &prims, &out.textures_delta) {
                        eprintln!("tideshell: bar: {e}");
                    }
                }
            }
            None => {
                gl.unbind(Role::Bar);
                bar_events.clear();
            }
        }

        // Overlay: map/unmap, input region, frame.
        let want = sb.view.needs_overlay(&sb.shell);
        if !want {
            gl.unbind(Role::Overlay);
            io.ensure_overlay(false);
            last_region = None;
            overlay_events.clear();
            continue;
        }
        io.ensure_overlay(true);
        let region = sb.view.input_rects(&sb.shell);
        if last_region.as_ref() != Some(&region) {
            let rects: Option<Vec<(i32, i32, i32, i32)>> =
                region.as_ref().map(|v| v.iter().map(|r| (r.left() as i32, r.top() as i32, r.width() as i32, r.height() as i32)).collect());
            io.set_input_region(Role::Overlay, rects.as_deref());
            last_region = Some(region);
        }
        let Some(t) = io.overlay() else { continue };
        t.prepare(&mut io);
        let size_px = t.size_px();
        let (w, h) = t.logical;
        if let Err(e) = gl.bind(Role::Overlay, &t.surface, size_px) {
            eprintln!("tideshell: {e}");
            continue;
        }
        // Edge/pad-owned sequences go to the engine and never reach egui.
        let mut events = std::mem::take(&mut overlay_events);
        let routed = sb.view.route_raw(&mut sb.shell, &mut events, Pos2::ZERO, t0.elapsed().as_millis() as u64);
        sb.execute(routed);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32))),
            time: Some(t0.elapsed().as_secs_f64()),
            events,
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(t.scale);
        let info = FrameInfo { apps: &apps, clock: clock(), battery: None };
        let mut frame_actions = Vec::new();
        let out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| {
                frame_actions = sb.view.ui(ui, &mut sb.shell, &palette, &info);
            });
        });
        sb.execute(frame_actions);
        // Over the overlay: what egui asked for, or nothing in fat mode (the
        // disc is the cursor; this also hides the system arrow over our
        // surfaces during a held drag, which cosmic-comp's timeout cannot).
        io.set_overlay_cursor(if sb.shell.fat.is_some() {
            CursorWant::Hidden
        } else {
            CursorWant::Shape(shape_for(out.platform_output.cursor_icon))
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        if let Err(e) = gl.paint(Role::Overlay, size_px, out.pixels_per_point, &prims, &out.textures_delta) {
            eprintln!("tideshell: {e}");
        }
    }
}
