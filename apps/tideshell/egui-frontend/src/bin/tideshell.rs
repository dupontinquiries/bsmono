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
use tideshell_egui::{key_events, AppTile, FrameInfo, ShellView};

const STRIP_PX: u32 = 28;

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
    /// A dropped-pointer-output notice was already written for the current EI state.
    ei_drop_noted: bool,
    /// Wet / palm / pocket rejection in front of every consumer.
    filter: tideshell_core::touchfilter::TouchFilter,
    filter_stats: tideshell_core::touchfilter::Stats,
    pending_display_power: Option<bool>,
    keys: Option<tideshell_egui::keys::Keys>,
    orientation: tideshell_egui::sensors::OrientationFeed,
    orientation_status: String,
}

impl Shellbox {
    fn execute(&mut self, actions: Vec<Action>) {
        for a in actions {
            let o = match &a {
                Action::SavePrefs if self.persist => {
                    self.backlight.set_rate(self.shell.prefs.brightness_rate);
                    effects::save_prefs(&self.shell.prefs)
                }
                Action::SavePrefs => Outcome::Done("prefs not persisted (TIDESHELL_PERSIST=0)".into()),
                // Only a Ready EI connection moves the pointer. Output sent to a
                // failed one used to vanish silently (seen on the FP5: the
                // trace showed clicks while ei was Failed(AccessDenied)); now
                // the first dropped output per failure says so.
                Action::Pointer(p) => match self.ei.as_ref().map(|e| e.status()) {
                    Some(tideshell_egui::ei::Status::Ready) => {
                        self.ei.as_ref().unwrap().send(*p);
                        continue;
                    }
                    other => {
                        if self.ei_drop_noted {
                            continue;
                        }
                        self.ei_drop_noted = true;
                        Outcome::Unwired(match other {
                            Some(tideshell_egui::ei::Status::Failed(_)) => "trackpad output dropped: EI failed (see the ei: line; cosmic-comp needs patch 0002)",
                            Some(_) => "trackpad output dropped: EI not ready yet",
                            None => "trackpad output dropped: no EI connection",
                        })
                    }
                },
                // Auto-brightness ramps send many of these: silent. Which
                // backlight path works (or none) is in the status lines.
                Action::SetSlider(tideshell_core::control_center::SliderKind::Brightness, v) => {
                    self.backlight.set(*v);
                    continue;
                }
                // Executed by the layer runtime (it owns the Wayland connection).
                Action::DisplayPower(on) => {
                    self.pending_display_power = Some(*on);
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
        if self.filter.stats != self.filter_stats {
            let s = self.filter.stats;
            self.shell.note(format!("touch filter: wet episodes {} dropped {} jumps {} excess {}", s.wet_episodes, s.dropped_wet, s.cancelled_jump, s.dropped_excess));
            self.filter_stats = s;
        }
        let os = self.orientation.status();
        if os != self.orientation_status {
            self.shell.note(os.clone());
            self.orientation_status = os;
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
                self.ei_drop_noted = false;
            }
        }
    }
}

/// Touch events through the rejection filter, in screen coordinates. The
/// pointer (slot -1) is never filtered. A rejected contact becomes a Cancel.
fn filter_input(f: &mut tideshell_core::touchfilter::TouchFilter, io: &Io, ev: Input, now: u64) -> Option<Input> {
    let to_screen = |role: Role, x: f32, y: f32| io.to_screen(role, x, y).unwrap_or((x, y));
    let verdict = match ev {
        Input::Down { role, slot, x, y, .. } if slot != POINTER_SLOT => {
            let (sx, sy) = to_screen(role, x, y);
            f.filter(TouchEvent::Down { slot, x: sx, y: sy, t: now })
        }
        Input::Move { role, slot, x, y, .. } if slot != POINTER_SLOT => {
            let (sx, sy) = to_screen(role, x, y);
            f.filter(TouchEvent::Move { slot, x: sx, y: sy, t: now })
        }
        Input::Up { slot, .. } if slot != POINTER_SLOT => f.filter(TouchEvent::Up { slot, t: now }),
        Input::Cancel => f.filter(TouchEvent::Cancel),
        _ => return Some(ev),
    };
    match verdict {
        Some(TouchEvent::Cancel) => Some(Input::Cancel),
        Some(_) => Some(ev),
        None => None,
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
        ei_drop_noted: false,
        filter: tideshell_core::touchfilter::TouchFilter::default(),
        filter_stats: tideshell_core::touchfilter::Stats::default(),
        pending_display_power: None,
        keys: None,
        orientation: tideshell_egui::sensors::OrientationFeed::spawn(zbus::blocking::Connection::system().ok()),
        orientation_status: String::new(),
    };
    {
        let found = tideshell_egui::keys::discover(std::path::Path::new("/sys"), std::path::Path::new("/dev/input"));
        let bus = zbus::blocking::Connection::system().ok();
        let k = tideshell_egui::keys::Keys::spawn(found, bus.as_ref());
        sb.shell.note(k.status.clone());
        sb.keys = Some(k);
    }
    sb.backlight.set_rate(sb.shell.prefs.brightness_rate);
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
    // The palette lists every hatch entry, not just the bar's favourites.
    let all_apps: Vec<(String, String)> = sb.system.launcher.as_ref().map(|l| l.results().map(|e| (e.id.clone(), e.name.clone())).collect()).unwrap_or_default();
    sb.shell.set_apps(all_apps);

    let ctx = egui::Context::default();
    let mut ui_prefs = settings_kit::Prefs::default();
    ui_prefs.palette = sb.shell.prefs.theme.clone();
    uikit_egui::apply_prefs(&ctx, &ui_prefs);
    let palette = uikit_egui::palette_colors_ctx(&ctx, &ui_prefs.palette);
    // Transparent panels: the overlay composites over the apps.
    ctx.global_style_mut(|s| s.visuals.panel_fill = egui::Color32::TRANSPARENT);
    // Side surfaces (bar, peeks) each get their own egui context (own
    // textures, own input), created on first use.
    let mut side_ctx: HashMap<Role, egui::Context> = HashMap::new();
    let mut side_events: HashMap<Role, Vec<egui::Event>> = HashMap::new();
    let mut side_pos: HashMap<(Role, i32), Pos2> = HashMap::new();

    let t0 = Instant::now();
    let mut overlay_events: Vec<egui::Event> = Vec::new();
    let mut contact_pos: HashMap<i32, Pos2> = HashMap::new();
    let mut last_region: Option<Option<Vec<Rect>>> = None;
    let mut last_poll = 0u64;
    // comfortgov reads the mtime of this file as "last user input".
    let hint = std::env::var_os("XDG_RUNTIME_DIR").map(|r| std::path::PathBuf::from(r).join("comfortgov.input"));
    let mut last_hint = 0u64;
    // Key repeat: the held key's events and when to repeat them next.
    let mut held: Option<(Vec<egui::Event>, u64)> = None;
    let mut last_power_status = String::new();
    let trace_stderr = std::env::var("TIDESHELL_TRACE").is_ok_and(|v| v == "stderr");
    let mut printed: Vec<String> = Vec::new();
    loop {
        if trace_stderr {
            print_new_trace(&sb.shell, &mut printed);
        }
        let busy = !sb.shell.recognizer.is_idle() || sb.view.needs_overlay(&sb.shell) || sb.shell.switcher.is_streaking();
        // 90 Hz while brightness is moving: one step per panel frame.
        let timeout = if sb.shell.ambient.is_animating() { 11 } else if busy { 16 } else { 250 };
        let input = io.dispatch(timeout).unwrap_or_else(|e| {
            eprintln!("tideshell: wayland: {e}");
            std::process::exit(1)
        });
        let now_ms = t0.elapsed().as_millis() as u64;
        sb.shell.set_clock(tideshell_egui::sensors::local_minute());
        while let Ok(lux) = sb.ambient.rx.try_recv() {
            sb.shell.on_lux(lux, now_ms);
        }
        let readings: Vec<String> = sb.orientation.rx.try_iter().collect();
        for o in readings {
            let out = sb.shell.on_orientation(&o, now_ms);
            sb.execute(out);
        }
        if let Some(screen) = io.screen() {
            if screen != sb.shell.screen {
                sb.shell.set_screen(screen);
            }
        }
        io.set_right_enabled(sb.shell.recognizer.cfg.right_enabled);

        // Strips -> recognizer (screen coords). Overlay -> egui events.
        // Real touches (not the pointer) pass the rejection filter first.
        let mut actions = Vec::new();
        for ev in input {
            let ev = match filter_input(&mut sb.filter, &io, ev, now_ms) {
                Some(e) => e,
                None => continue,
            };
            if matches!(ev, Input::Down { .. }) && now_ms.saturating_sub(last_hint) >= 500 {
                last_hint = now_ms;
                if let Some(h) = &hint {
                    let _ = std::fs::write(h, b"");
                }
            }
            match ev {
                // Side surfaces (bar, peeks) are widget surfaces, not edges:
                // their input goes to their own egui contexts.
                Input::Down { role, slot, x, y, .. } if role.is_side() => {
                    side_pos.insert((role, slot), Pos2::new(x, y));
                    side_events.entry(role).or_default().extend(contact_events(slot, egui::TouchPhase::Start, Pos2::new(x, y)));
                }
                Input::Move { role, slot, x, y, .. } if role.is_side() => {
                    side_pos.insert((role, slot), Pos2::new(x, y));
                    side_events.entry(role).or_default().extend(contact_events(slot, egui::TouchPhase::Move, Pos2::new(x, y)));
                }
                Input::Up { role, slot, .. } if role.is_side() => {
                    let pos = side_pos.remove(&(role, slot)).unwrap_or(Pos2::ZERO);
                    side_events.entry(role).or_default().extend(contact_events(slot, egui::TouchPhase::End, pos));
                }
                Input::Hover { role, x, y } if role.is_side() => side_events.entry(role).or_default().push(egui::Event::PointerMoved(Pos2::new(x, y))),
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
                Input::Key { pressed, keysym, utf8, ctrl, alt, shift } => {
                    let ev = key_events(pressed, keysym, &utf8, ctrl, alt, shift, false);
                    if pressed {
                        let (rate, delay) = io.st_repeat();
                        held = (rate > 0).then(|| (key_events(true, keysym, &utf8, ctrl, alt, shift, true), now_ms + delay.max(0) as u64));
                    } else {
                        held = None;
                    }
                    overlay_events.extend(ev);
                }
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
        if let Some((ev, next)) = &mut held {
            if now_ms >= *next {
                overlay_events.extend(ev.iter().cloned());
                let (rate, _) = io.st_repeat();
                *next = now_ms + (1000 / rate.max(1)) as u64;
            }
        }
        // Hardware keys: power = sleep/wake, volume = a 5 % fade.
        let presses: Vec<tideshell_egui::keys::KeyPress> = sb.keys.as_ref().map(|k| k.rx.try_iter().collect()).unwrap_or_default();
        for k in presses {
            use tideshell_core::control_center::SliderKind;
            use tideshell_egui::keys::KeyPress;
            let v = sb.shell.drawer.volume;
            actions.extend(match k {
                KeyPress::Power => sb.shell.power_key(),
                KeyPress::VolumeUp => sb.shell.set_slider(SliderKind::Volume, (v + 0.05).min(1.0)),
                KeyPress::VolumeDown => sb.shell.set_slider(SliderKind::Volume, (v - 0.05).max(0.0)),
            });
        }
        actions.extend(sb.shell.tick(now_ms));
        sb.execute(actions);
        if let Some(on) = sb.pending_display_power.take() {
            let msg = io.set_display_power(on);
            sb.shell.note(msg);
        }
        if io.power_status() != last_power_status {
            last_power_status = io.power_status();
            sb.shell.note(last_power_status.clone());
        }
        // Keys go to the overlay only while it shows something typed into.
        io.set_overlay_keyboard(sb.shell.palette.open || sb.view.settings_open());

        if now_ms.saturating_sub(last_poll) >= 100 {
            last_poll = now_ms;
            sb.poll_world();
        }

        // Side surfaces, each with an exclusive zone: the bar (right-bar
        // layout) and the two carousel peeks (carousel mode). Each has its
        // own egui context, input queue and EGL window.
        let bar_geom = ShellView::bar_geometry(&sb.shell);
        let peek = sb.shell.peek_reserve().round() as u32;
        let sides: [(Role, Option<(u32, i32)>); 3] = [
            (Role::Bar, bar_geom),
            (Role::PeekLeft, (peek > 0).then_some((peek, peek as i32))),
            (Role::PeekRight, (peek > 0).then_some((peek, peek as i32))),
        ];
        let mut reserved = 0.0;
        for (role, geom) in sides {
            io.ensure_side(role, geom.is_some(), geom.map_or(0, |g| g.0), geom.map_or(0, |g| g.1));
            if let (Some(_), Some((_, r))) = (io.side(role), geom) {
                reserved += r as f32;
            }
        }
        // Maximized windows still laid out under a zone get re-maximized
        // (see Shell::reflow_for_zone for why cosmic-comp needs this).
        if reserved > 0.0 {
            let out = sb.shell.reflow_for_zone(reserved);
            sb.execute(out);
        }
        for (role, _) in sides {
            let Some(t) = io.side(role) else {
                gl.unbind(role);
                side_events.remove(&role);
                continue;
            };
            t.prepare(&mut io);
            let size_px = t.size_px();
            let (w, h) = t.logical;
            if gl.bind(role, &t.surface, size_px).is_err() {
                continue;
            }
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32))),
                time: Some(t0.elapsed().as_secs_f64()),
                events: side_events.remove(&role).unwrap_or_default(),
                focused: true,
                ..Default::default()
            };
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(t.scale);
            let info = FrameInfo { apps: &apps, time: tideshell_egui::sensors::local_time(), battery: None };
            let mut actions = Vec::new();
            let ctx_side = side_ctx.entry(role).or_insert_with(|| {
                let c = egui::Context::default();
                uikit_egui::apply_prefs(&c, &ui_prefs);
                let _ = uikit_egui::palette_colors_ctx(&c, &ui_prefs.palette);
                c
            });
            let out = ctx_side.run_ui(raw, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| {
                    actions = match role {
                        Role::Bar => sb.view.bar_ui(ui, &mut sb.shell, &palette, &info),
                        Role::PeekLeft => sb.view.peek_ui(ui, &mut sb.shell, tideshell_core::carousel::Side::Left, &palette),
                        _ => sb.view.peek_ui(ui, &mut sb.shell, tideshell_core::carousel::Side::Right, &palette),
                    };
                });
            });
            sb.execute(actions);
            let prims = ctx_side.tessellate(out.shapes, out.pixels_per_point);
            if let Err(e) = gl.paint(role, size_px, out.pixels_per_point, &prims, &out.textures_delta) {
                eprintln!("tideshell: {role:?}: {e}");
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
        let info = FrameInfo { apps: &apps, time: tideshell_egui::sensors::local_time(), battery: None };
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
