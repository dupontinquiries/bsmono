//! tideshell — the shell itself, on layer-shell surfaces (no eframe).
//!
//!   tideshell [--trace] [--layout right-bar] [--rotate DEG]
//! `--rotate 0|90|180|270` applies that output transform once at start (to
//! check the panel's mapping on a phone); autorotation continues after.
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

// musl returns modest allocations straight to the kernel rather than
// retaining them, so a loop that allocates a few 8-32 kB buffers per frame
// pays an mmap and an munmap for each — measured on the FP5 at about 1400 of
// each per second, the largest syscall count in the process by a wide
// margin, and every pair is a page-table walk and a TLB shootdown. A caching
// allocator keeps those spans in userspace.
//
// Global rather than feature-gated: glibc would not have produced the
// symptom, but it does not suffer from the fix either, and a phone shell
// that behaves differently on the developer's laptop than on the phone is
// worse than a megabyte of allocator.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::collections::HashMap;
use std::time::Instant;

use egui::{Pos2, Rect, Vec2};
use tideshell_core::{Action, Shell, TouchEvent};
use tideshell_egui::effects::{self, Executor, Outcome, System};
use tideshell_egui::layer::{shape_for, CursorWant, Gl, Input, Io, Role, POINTER_SLOT};
use tideshell_egui::{camera, key_events, AppTile, FrameInfo, ShellView};

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
    pending_rotate: Option<u32>,
    keys: Option<tideshell_egui::keys::Keys>,
    orientation: tideshell_egui::sensors::OrientationFeed,
    orientation_status: String,
    foreground_written: String,
    proximity: tideshell_egui::sensors::ProximityFeed,
    proximity_status: String,
    imu: tideshell_egui::sensors::ImuFeed,
    compass: tideshell_egui::sensors::CompassFeed,
    mic: Option<tideshell_egui::sensors::MicFeed>,
    camera: Option<tideshell_egui::camera::ChildSource>,
    cam_duty: tideshell_egui::camera::DutyCycle,
    /// The speakers follow the screen: the same transform that rotates the
    /// display decides which driver is on the listener's left.
    pose: speaker_pose::Mix,
    volkeys: tideshell_core::volkeys::VolKeys,
    /// The matrix last pushed to PipeWire, so nothing is sent per frame.
    last_pose: Option<speaker_pose::Matrix>,
    /// The rear camera's libcamera id, found once at startup.
    rear_camera: Option<String>,
    /// The last colour grid, for deciding whether the room changed.
    last_glass: Option<Vec<[u8; 3]>>,
    /// Tracker state: the previous frame, its corners, and the world's
    /// yaw/pitch when it was taken.
    prev_frame: Option<(tideshell_core::track::Frame, Vec<tideshell_core::track::Point>, f32, f32)>,
    feed_status: (String, String, String, String),
    notify_gain_written: f32,
    last_temp_poll: u64,
    strip_base_px: u32,
    last_place_poll: u64,
    last_color_note: u64,
}

impl Shellbox {
    fn execute(&mut self, actions: Vec<Action>) {
        for a in actions {
            let o = match &a {
                Action::SavePrefs if self.persist => {
                    self.backlight.set_rate(self.shell.prefs.brightness_rate);
                    self.backlight.set_mode(&self.shell.prefs.brightness_mode);
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
                Action::Rotate(deg) => {
                    self.pending_rotate = Some(*deg);
                    continue;
                }
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

    /// What the pad should say when it cannot work.
    fn trackpad_note(&self) -> Option<&'static str> {
        match &self.ei_status {
            Some(tideshell_egui::ei::Status::Ready) => None,
            Some(tideshell_egui::ei::Status::Failed(_)) => Some("needs cosmic-comp patch 0002 (EI access denied)"),
            _ => Some("connecting to the compositor..."),
        }
    }

    /// The places log on disk.
    fn places_path() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))).map(|d| d.join("tideshell/places.log"))
    }

    /// comfortgov reads this as the app in front (its scope is spared reclaim).
    fn write_foreground_hint(&mut self) {
        let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else { return };
        let active = self.shell.windows().iter().find(|w| w.activated).map(|w| w.app_id.clone()).unwrap_or_default();
        if active != self.foreground_written {
            let _ = std::fs::write(std::path::PathBuf::from(dir).join("comfortgov.foreground"), &active);
            self.foreground_written = active;
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
                self.write_foreground_hint();
            }
        }
        if self.filter.stats != self.filter_stats {
            let s = self.filter.stats;
            self.shell.note(format!("touch filter: wet episodes {} dropped {} jumps {} excess {}", s.wet_episodes, s.dropped_wet, s.cancelled_jump, s.dropped_excess));
            self.filter_stats = s;
        }
        let fs = (
            self.imu.status(),
            self.compass.status(),
            self.mic.as_ref().map(|m| m.status.clone()).unwrap_or_else(|| "mic: off (shell.ambient.mic)".into()),
            self.camera.as_ref().map(|c| c.status.clone()).unwrap_or_else(|| "camera: off".into()),
        );
        for (new, old) in [(&fs.0, &self.feed_status.0), (&fs.1, &self.feed_status.1), (&fs.2, &self.feed_status.2), (&fs.3, &self.feed_status.3)] {
            if new != old {
                self.shell.note(new.clone());
            }
        }
        self.feed_status = fs;
        let g = self.shell.notification_gain();
        if (g - self.notify_gain_written).abs() >= 0.05 {
            self.notify_gain_written = g;
            if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
                let _ = std::fs::write(std::path::PathBuf::from(dir).join("tideshell.notify-gain"), format!("{g:.2}"));
            }
        }
        let ps = self.proximity.status();
        if ps != self.proximity_status {
            self.shell.note(ps.clone());
            self.proximity_status = ps;
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

/// comfortgov's state line: `state=Sustained ceiling=0.93 on_battery=true battery=63`.
fn parse_headroom(s: &str) -> tideshell_core::voice::Headroom {
    let mut h = tideshell_core::voice::Headroom::default();
    for kv in s.split_whitespace() {
        let Some((k, v)) = kv.split_once('=') else { continue };
        match k {
            "ceiling" => h.ceiling = v.parse().unwrap_or(1.0),
            "on_battery" => h.on_battery = v == "true",
            "battery" => h.battery_pct = v.parse().unwrap_or(100),
            "state" => h.sustained_load = v == "Sustained",
            _ => {}
        }
    }
    h
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

/// A display transform in degrees, as the shell emits it, to the screen
/// orientation the speakers care about.
fn transform_of(deg: u32) -> speaker_pose::Transform {
    match deg % 360 {
        90 => speaker_pose::Transform::Rotate90,
        180 => speaker_pose::Transform::Rotate180,
        270 => speaker_pose::Transform::Rotate270,
        _ => speaker_pose::Transform::Normal,
    }
}

/// Is this matrix audibly different from the last one sent? A crossfade
/// produces a new matrix every frame; only steps worth a round trip to
/// PipeWire are pushed.
fn pose_differs(a: &speaker_pose::Matrix, b: &speaker_pose::Matrix) -> bool {
    const EPS: f32 = 0.01;
    (a.top.from_left - b.top.from_left).abs() > EPS
        || (a.top.from_right - b.top.from_right).abs() > EPS
        || (a.bottom.from_left - b.bottom.from_left).abs() > EPS
        || (a.bottom.from_right - b.bottom.from_right).abs() > EPS
        || a.mono != b.mono
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `tideshell ctl <command>`: a client, for cosmic-comp's key bindings.
    // `tideshell install-keys`: write those bindings into cosmic's config.
    if args.get(1).map(String::as_str) == Some("ctl") {
        let cmd = args.get(2).map(String::as_str).unwrap_or("");
        match tideshell_egui::ctl::socket_path().ok_or_else(|| "XDG_RUNTIME_DIR not set".to_string()).and_then(|p| tideshell_egui::ctl::send(&p, cmd)) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("tideshell ctl: {e}");
                std::process::exit(1);
            }
        }
    }
    // `tideshell doctor power-key`: who consumes the power key here (run
    // with sudo to see system daemons' file descriptors).
    if args.get(1).map(String::as_str) == Some("doctor") {
        if args.get(2).map(String::as_str) != Some("power-key") {
            eprintln!("usage: tideshell doctor power-key");
            std::process::exit(2);
        }
        let bus = zbus::blocking::Connection::system().ok();
        // Under sudo, look at the invoking user's cosmic config, not root's.
        let home = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty() && u != "root").map(|u| std::path::PathBuf::from(format!("/home/{u}/.config"))).or_else(effects::cosmic_config_home);
        print!("{}", tideshell_egui::powerdoctor::report(&tideshell_egui::powerdoctor::gather(bus.as_ref(), home.as_deref())));
        std::process::exit(0);
    }
    // `tideshell sensors`: what each input actually resolves to on THIS
    // device, and a few live readings. Written because a probe that only
    // looks for `/sys/class/light` or an `in_illuminance_*` file concludes
    // "no ambient light sensor" on a phone whose ALS works fine behind
    // iio-sensor-proxy (the sensor hub exposes it over D-Bus, and an IIO
    // sensor in buffered mode has no `_input`/`_raw` attribute to find).
    // `tideshell speaker-config`: the filter-chain config on stdout, so the
    // installer can place it without the shell having to be running.
    if args.get(1).map(String::as_str) == Some("speaker-config") {
        print!("{}", tideshell_core::audio::speaker_chain_config());
        std::process::exit(0);
    }
    if args.get(1).map(String::as_str) == Some("sensors") {
        let sys = std::path::Path::new("/sys");
        let bus = zbus::blocking::Connection::system().ok();
        println!("iio devices:");
        let mut any = false;
        let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(sys.join("bus/iio/devices")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in &dirs {
            any = true;
            let name = std::fs::read_to_string(d.join("name")).unwrap_or_default().trim().to_string();
            let chans: Vec<String> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| f.starts_with("in_")).collect();
            let buffered = d.join("scan_elements").is_dir();
            println!("  {} {name}{}{}", d.display(), if buffered { " [buffered]" } else { "" }, if chans.is_empty() { String::new() } else { format!(" {}", chans.join(" ")) });
        }
        if !any {
            println!("  (none)");
        }
        match &bus {
            Some(b) => match tideshell_egui::sensors::sensor_proxy_claim(b) {
                Ok(unit) => {
                    println!("iio-sensor-proxy: ambient light, unit {unit}");
                    for _ in 0..4 {
                        match tideshell_egui::sensors::sensor_proxy_read(b) {
                            Ok(v) => println!("  light = {v}"),
                            Err(e) => println!("  read failed: {e}"),
                        }
                        std::thread::sleep(std::time::Duration::from_millis(400));
                    }
                }
                Err(e) => println!("iio-sensor-proxy: {e}"),
            },
            None => println!("iio-sensor-proxy: no system bus"),
        }
        match tideshell_egui::sensors::find_iio_light(sys) {
            Some(l) => println!("sysfs light: {} -> {:?} lux", l.dir.display(), l.read()),
            None => println!("sysfs light: no in_illuminance_input / in_illuminance_raw (a buffered-mode sensor has neither; the D-Bus line above is the one that matters)"),
        }
        match tideshell_egui::sensors::find_backlight(sys) {
            Some(b) => println!("backlight: {} level {:?}", b.name, b.read_level()),
            None => println!("backlight: none under /sys/class/backlight"),
        }
        let amb = tideshell_egui::sensors::Ambient::spawn(sys.to_path_buf(), bus);
        std::thread::sleep(std::time::Duration::from_millis(1200));
        println!("what tideshell would use -> {}", amb.status());
        std::process::exit(0);
    }
    // `tideshell install-keys` / `remove-keys`: the same lever the shell
    // pulls at startup, by hand.
    if matches!(args.get(1).map(String::as_str), Some("install-keys" | "remove-keys")) {
        let Some(home) = effects::cosmic_config_home() else {
            eprintln!("tideshell: no config home");
            std::process::exit(1);
        };
        let file = tideshell_egui::ctl::cosmic_custom_shortcuts(&home);
        let r = if args[1] == "install-keys" {
            let bin = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "tideshell".into());
            tideshell_egui::ctl::ensure_keys(&home, &bin).map(|o| format!("{}: {o:?} (power, volume up/down -> `{bin} ctl ...`; cosmic-comp reloads it live)", file.display()))
        } else {
            tideshell_egui::ctl::remove_keys(&home).map(|removed| format!("{}: {}", file.display(), if removed { "tideshell's bindings removed; cosmic's defaults apply again (the power key's shutdown dialog included)" } else { "no tideshell bindings to remove" }))
        };
        match r {
            Ok(m) => {
                println!("{m}");
                std::process::exit(0)
            }
            Err(e) => {
                eprintln!("tideshell {}: {e}", args[1]);
                std::process::exit(1)
            }
        }
    }
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

        // Which camera is the rear one. Asked once: `cam -l` starts a libcamera
    // manager, which is not something to do every window.
    // How the sensor sits relative to the screen: looked up per device,
    // because nothing in the stream says. libcamera reports a Rotation only
    // where the kernel driver declares one, and these drivers do not.
    // TIDESHELL_CAMERA_ROTATE / _MIRROR override the table, so a new handset
    // can be worked out live and written down afterwards.
    let model = std::fs::read("/proc/device-tree/model").map(|b| tideshell_egui::devices::parse_model(&b)).unwrap_or_default();
    let cam_mount = tideshell_egui::devices::mount_for(
        &model,
        false,
        std::env::var("TIDESHELL_CAMERA_ROTATE").ok().as_deref(),
        std::env::var("TIDESHELL_CAMERA_MIRROR").ok().as_deref(),
    );
    let cam_rot = cam_mount.rotate;
    let cam_list = std::process::Command::new("cam").arg("-l").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
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
        pending_rotate: None,
        keys: None,
        orientation: tideshell_egui::sensors::OrientationFeed::spawn(zbus::blocking::Connection::system().ok()),
        orientation_status: String::new(),
        foreground_written: String::new(),
        proximity: tideshell_egui::sensors::ProximityFeed::spawn(zbus::blocking::Connection::system().ok()),
        proximity_status: String::new(),
        imu: tideshell_egui::sensors::ImuFeed::spawn("/sys".into()),
        compass: tideshell_egui::sensors::CompassFeed::spawn(zbus::blocking::Connection::system().ok()),
        mic: None,
        camera: None,
        cam_duty: tideshell_egui::camera::DutyCycle::new(tideshell_egui::camera::Duty::default()),
        pose: speaker_pose::Mix::new(speaker_pose::Transform::Normal, speaker_pose::Tuning::default()),
        volkeys: tideshell_core::volkeys::VolKeys::default(),
        last_pose: None,
        rear_camera: tideshell_egui::camera::pick_rear(&cam_list),
        last_glass: None,
        prev_frame: None,
        feed_status: Default::default(),
        notify_gain_written: -1.0,
        last_temp_poll: 0,
        strip_base_px: 0,
        last_place_poll: 0,
        last_color_note: 0,
    };
    sb.view.camera_rot = cam_rot;
    // Default to "accelerated" until the first paint tells us otherwise: the
    // cost of guessing software on real hardware is a shell that feels
    // sluggish for no reason.
    sb.view.accelerated = true;
    // The speaker chain is tideshell's own sink, so tideshell puts it in
    // place rather than leaving it to a script the user has to find.
    {
        let out = tideshell_egui::effects::ensure_speaker_chain();
        sb.shell.note(format!("{out:?}"));
    }
    sb.view.camera_mirror = cam_mount.mirror;
    sb.shell.note(tideshell_egui::devices::describe(&model, false, cam_mount));
    sb.shell.set_date(tideshell_egui::sensors::local_yday());
    if let Some(p) = Shellbox::places_path() {
        if let Ok(text) = std::fs::read_to_string(&p) {
            sb.shell.ambience = tideshell_core::ambience::Ambience::load(&text);
            sb.shell.note(format!("places: {} known ({})", sb.shell.ambience.places.len(), p.display()));
        }
    }
    if sb.shell.prefs.ambient_mic {
        sb.mic = Some(tideshell_egui::sensors::MicFeed::spawn());
    }
    {
        let found = tideshell_egui::keys::discover(std::path::Path::new("/sys"), std::path::Path::new("/dev/input"));
        let bus = zbus::blocking::Connection::system().ok();
        // The first lever: route the keys through cosmic-comp by writing
        // its custom shortcuts (reloaded live). TIDESHELL_MANAGE_KEYS=0
        // leaves the file alone; the shell then only reports what it finds.
        let manage = std::env::var("TIDESHELL_MANAGE_KEYS").map(|v| v != "0").unwrap_or(true);
        if let Some(h) = effects::cosmic_config_home() {
            let bin = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "tideshell".into());
            if manage {
                match tideshell_egui::ctl::ensure_keys(&h, &bin) {
                    Ok(o) => sb.shell.note(format!("keys: cosmic custom shortcuts {o:?} ({})", tideshell_egui::ctl::cosmic_custom_shortcuts(&h).display())),
                    Err(e) => sb.shell.note(format!("keys: could not write cosmic custom shortcuts: {e}")),
                }
            } else {
                let ok = std::fs::read_to_string(tideshell_egui::ctl::cosmic_custom_shortcuts(&h)).map(|t| tideshell_egui::ctl::routes_to(&t, &bin)).unwrap_or(false);
                sb.shell.note(format!("keys: TIDESHELL_MANAGE_KEYS=0; cosmic routes the keys to this binary: {ok}"));
            }
        }
        let via = effects::cosmic_config_home().is_some_and(|h| tideshell_egui::ctl::compositor_route_installed(&h));
        let k = tideshell_egui::keys::Keys::spawn(found, bus.as_ref(), via);
        sb.shell.note(k.status.clone());
        sb.keys = Some(k);
    }
    sb.backlight.set_rate(sb.shell.prefs.brightness_rate);
    let wm = sb.backlight.set_mode(&sb.shell.prefs.brightness_mode);
    sb.shell.note(format!("backlight write mode: {wm:?} (shell.brightness.mode = {}; safe = one write per settled value, for the FP5 DPU bug pmaports#4274)", sb.shell.prefs.brightness_mode));
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
    /// When each side surface next wants to run, from egui's own
    /// `repaint_delay`.
    let mut side_due: HashMap<Role, u64> = HashMap::new();
    let mut side_events: HashMap<Role, Vec<egui::Event>> = HashMap::new();
    let mut side_pos: HashMap<(Role, i32), Pos2> = HashMap::new();

    let t0 = Instant::now();

    let mut overlay_events: Vec<egui::Event> = Vec::new();
    let mut contact_pos: HashMap<i32, Pos2> = HashMap::new();
    let mut last_region: Option<Option<Vec<Rect>>> = None;
    let mut renderer_noted = false;
    let mut last_frame_hash: HashMap<Role, tideshell_egui::damage::Painted> = HashMap::new();
    let mut last_poll = 0u64;
    // comfortgov reads the mtime of this file as "last user input".
    let hint = std::env::var_os("XDG_RUNTIME_DIR").map(|r| std::path::PathBuf::from(r).join("comfortgov.input"));
    let mut last_hint = 0u64;
    // Key repeat: the held key's events and when to repeat them next.
    let mut held: Option<(Vec<egui::Event>, u64)> = None;
    let mut last_power_status = String::new();
    let ctl_server = match tideshell_egui::ctl::socket_path().map(|p| tideshell_egui::ctl::Server::bind(&p)) {
        Some(Ok(s)) => {
            sb.shell.note("ctl: listening (tideshell ctl power-key | volume-up | volume-down | lock | palette | switcher)".to_string());
            Some(s)
        }
        Some(Err(e)) => {
            sb.shell.note(format!("ctl: cannot bind the control socket: {e}"));
            None
        }
        None => None,
    };
    let mut last_config_status = String::new();
    if let Some(deg) = args.iter().position(|a| a == "--rotate").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()) {
        sb.pending_rotate = Some(deg);
    }
    let trace_stderr = std::env::var("TIDESHELL_TRACE").is_ok_and(|v| v == "stderr");
    let mut printed: Vec<String> = Vec::new();
    loop {
        if trace_stderr {
            print_new_trace(&sb.shell, &mut printed);
        }
        // `busy` is about a finger or an animation, NOT about the overlay
        // being mapped. A mapped overlay is the normal state on this layout
        // — a locked screen, or the right-hand bar — so treating it as busy
        // held the loop at 16 ms forever, and with nothing to draw that is
        // sixty wakeups a second to decide to do nothing.
        let animating = sb.shell.drawer.is_animating()
            || sb.shell.switcher.is_animating()
            || sb.shell.sleep.is_animating()
            || sb.shell.ambient.is_animating()
            || (sb.shell.lock.is_locked() && !sb.shell.lock.reveal.is_settled());
        let busy = !sb.shell.recognizer.is_idle() || sb.shell.switcher.is_streaking() || animating;
        // 90 Hz while brightness is moving: one step per panel frame.
        let timeout = if sb.shell.ambient.is_animating() {
            11
        } else if busy {
            16
        } else {
            250
        };
        // Sleep on everything that can want the loop, not only on Wayland:
        // the power key arrives on the control socket and the volume keys
        // come from the evdev reader, and at the idle timeout either would
        // otherwise wait a quarter of a second before anything happened.
        let mut wake_fds: Vec<std::os::fd::BorrowedFd> = Vec::new();
        if let Some(c) = ctl_server.as_ref() {
            wake_fds.push(c.as_fd());
        }
        if let Some(k) = sb.keys.as_ref().and_then(|k| k.wake_fd()) {
            wake_fds.push(k);
        }
        let input = io.dispatch_with(timeout, &wake_fds).unwrap_or_else(|e| {
            eprintln!("tideshell: wayland: {e}");
            std::process::exit(1)
        });
        let now_ms = t0.elapsed().as_millis() as u64;
        sb.shell.set_clock(tideshell_egui::sensors::local_minute());
        while let Ok(lux) = sb.ambient.rx.try_recv() {
            sb.shell.on_lux(lux, now_ms);
        }
        for (g, a) in sb.imu.rx.try_iter().collect::<Vec<_>>() {
            sb.shell.on_imu(g, a, now_ms);
        }
        for h in sb.compass.rx.try_iter().collect::<Vec<_>>() {
            sb.shell.on_heading(h);
        }
        if let Some(m) = &sb.mic {
            for l in m.rx.try_iter().collect::<Vec<_>>() {
                sb.shell.on_mic_level(l, now_ms);
            }
        }
        // Every 60 s: which WiFi we are on (a place), the landmark hint file
        // from snapclone, the calendar, and the places log if it changed.
        if now_ms.saturating_sub(sb.last_place_poll) >= 60_000 {
            sb.last_place_poll = now_ms;
            sb.shell.set_date(tideshell_egui::sensors::local_yday());
            let hint = std::env::var_os("XDG_RUNTIME_DIR").map(|r| std::path::PathBuf::from(r).join("tideshell.place")).and_then(|p| std::fs::read_to_string(p).ok());
            if let Some((key, name)) = hint.as_deref().and_then(tideshell_core::ambience::PlaceKey::parse) {
                sb.shell.on_place(key, &name);
            } else if let Some((bssid, ssid)) = tideshell_egui::sensors::wifi_place() {
                sb.shell.on_place(tideshell_core::ambience::PlaceKey::Wifi(bssid), &ssid);
            }
            if sb.shell.take_ambience_dirty() {
                if let Some(p) = Shellbox::places_path() {
                    if let Some(d) = p.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(&p, sb.shell.ambience.save());
                }
            }
        }
        // Every 2 s: battery temperature (cold mode) and the governor's
        // state file (voice headroom); publish the voice route on change.
        if now_ms.saturating_sub(sb.last_temp_poll) >= 2000 {
            sb.last_temp_poll = now_ms;
            if let Some(c) = tideshell_egui::sensors::battery_temp_c(std::path::Path::new("/sys")) {
                sb.shell.on_battery_temp(c);
            }
            let t = sb.shell.cold_tuning();
            sb.filter.slop_scale = t.slop_scale;
            if sb.strip_base_px == 0 {
                sb.strip_base_px = io.strip_px;
            }
            io.set_strip_px((sb.strip_base_px as f32 * t.target_scale).round() as u32);
            for p in ["/run/comfortgov.state", "/tmp/comfortgov.state"] {
                if let Ok(s) = std::fs::read_to_string(p) {
                    sb.shell.headroom = parse_headroom(&s);
                    break;
                }
            }
            if let Some(line) = sb.shell.voice_route_line() {
                if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
                    let _ = std::fs::write(std::path::PathBuf::from(dir).join("tideshell.voice-route"), line);
                }
            }
        }
        // The camera: on only while the cover shows on a lit screen, and
        // then only in short windows (camera::DutyCycle). Continuous capture
        // for a surface that is blurred to a colour wash costs a software
        // debayer per frame — libcamera has no hardware ISP on this phone —
        // for detail nobody sees.
        let want_cam = sb.shell.camera_wanted();
        match sb.cam_duty.tick(want_cam, now_ms) {
            camera::DutyAction::Start => {
                let choice = camera::CameraChoice {
                    // Auto-pick the REAR camera when nothing names one:
                    // libcamerasrc's own default is whichever libcamera lists
                    // first, which here is the selfie camera.
                    name: std::env::var("TIDESHELL_CAMERA").ok().filter(|v| !v.trim().is_empty()).or_else(|| sb.rear_camera.clone()),
                    capture: camera::CameraChoice::from_vars(None, std::env::var("TIDESHELL_CAMERA_CAPTURE").ok().as_deref()).capture,
                    fps: Some(2),
                    // Frozen, so a dark room looks dark. Auto exposure
                    // exists to make every scene look the same, which is
                    // exactly wrong for a tint that is supposed to mean
                    // something about the room.
                    exposure: Some(camera::Exposure::default()),
                };
                sb.camera = Some(camera::ChildSource::start(&camera::gst_command(&choice, camera::FRAME_W, camera::FRAME_H), camera::FRAME_W, camera::FRAME_H));
            }
            camera::DutyAction::Stop => {
                sb.camera = None;
                sb.prev_frame = None;
                if !want_cam {
                    sb.view.clear_camera_frame();
                }
            }
            camera::DutyAction::Wait => {}
        }
        if let Some(f) = sb.camera.as_mut().and_then(|c| c.poll()) {
            use tideshell_core::track;
            let gray = track::Frame::from_rgba(f.w, f.h, &f.rgba);
            let (yaw, pitch) = (sb.shell.world.yaw_deg().to_radians(), sb.shell.world.pitch_deg().to_radians());
            if let Some((prev, pts, y0, p0)) = &sb.prev_frame {
                let m = track::estimate_motion(prev, pts, &gray);
                if m.tracked >= 8 {
                    let dyaw = ((yaw - y0 + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI)) - std::f32::consts::PI;
                    let predicted = sb.shell.world.predicted_shift(dyaw, pitch - p0);
                    // The pane's focal is in screen px; the tracker works on the small frame.
                    let scale = sb.shell.screen.0 / f.w as f32;
                    sb.shell.on_image_motion((m.dx * scale, m.dy * scale), predicted);
                }
            }
            let pts = track::find_corners(&gray);
            sb.prev_frame = Some((gray, pts, yaw, pitch));
            // Has the room changed? The only idle signal on a phone whose
            // accelerometer is unreadable, and the right question anyway:
            // what matters for a tint is the room, not the handset.
            let grid = camera::glass(&f, 4, 5);
            let changed = sb.last_glass.as_ref().map(|prev| camera::scene_changed(prev, &grid, 4.0)).unwrap_or(true);
            // The pane draws the wash from these twenty cells, not from the
            // frame: 60 bytes instead of a texture upload, and the GPU does
            // the interpolation.
            sb.view.set_glass(grid.clone(), 4, 5);
            // With the exposure fixed and known, the mean level is a light
            // reading: the only one this phone has, since the ambient light
            // sensor sits behind the low-power island and cannot be read.
            let mean = grid.iter().map(|c| (c[0] as u32 + c[1] as u32 + c[2] as u32) / 3).sum::<u32>() as f32 / grid.len().max(1) as f32;
            let ev = camera::scene_ev(mean, &camera::Exposure::default());
            if now_ms.saturating_sub(sb.last_color_note) >= 5000 {
                sb.shell.note(format!("scene: mean {mean:.0}, {ev:+.1} EV"));
            }
            sb.last_glass = Some(grid);
            sb.cam_duty.frame(changed);
            // What this place looks like: its dominant colours, every 5 s.
            if now_ms.saturating_sub(sb.last_color_note) >= 5000 {
                sb.last_color_note = now_ms;
                let colors = tideshell_core::ambience::dominant_colors(&f.rgba, 3);
                sb.shell.on_photo_colors(&colors);
            }
            // The wash no longer reads the texture, so it is not uploaded.
            // The motion tracker above works on the CPU-side grayscale copy
            // and never needed the GPU one.
        }
        let nears: Vec<bool> = sb.proximity.rx.try_iter().collect();
        for n in nears {
            sb.shell.on_proximity(n, now_ms);
        }
        let readings: Vec<String> = sb.orientation.rx.try_iter().collect();
        for o in readings {
            let out = sb.shell.on_orientation(&o, now_ms);
            // The speakers turn with the screen. Told from the same rotation
            // rather than from a sensor of its own, so the audio can never
            // disagree with what the user is looking at.
            for a in &out {
                if let Action::Rotate(deg) = a {
                    sb.pose.observe(transform_of(*deg), now_ms);
                }
            }
            sb.execute(out);
        }
        // Crossfade the speaker matrix. Only pushed when it actually moves:
        // a Props message per frame would be absurd, and the fade is over in
        // a few hundred ms.
        {
            let m = sb.pose.tick(now_ms);
            if sb.last_pose.map(|p| pose_differs(&p, &m)).unwrap_or(true) {
                sb.last_pose = Some(m);
                let out = tideshell_egui::effects::apply_speaker_pose(&m, speaker_pose::Tuning::default().crossover_hz);
                sb.shell.note(format!("{out:?}"));
            }
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
        // Hardware keys. The power key arrives from cosmic-comp through the
        // control socket; the volume keys come from evdev, because a
        // compositor shortcut fires on press and reports no release, and a
        // hold cannot be told from a tap without one.
        use tideshell_core::volkeys::{Intent, VolKey};
        use tideshell_egui::keys::{KeyEvent, KeyPress};
        let mut events: Vec<KeyEvent> = sb.keys.as_ref().map(|k| k.rx.try_iter().collect()).unwrap_or_default();
        if let Some(k) = sb.keys.as_ref() {
            k.drain_wake();
        }
        if let Some(srv) = &ctl_server {
            use tideshell_egui::ctl::Command;
            for c in srv.poll() {
                match c {
                    // A socket command is a complete press: down and up at
                    // once, so a bound hold can never fire from one.
                    Ok(Command::PowerKey) => events.push(KeyEvent { key: KeyPress::Power, down: true }),
                    Ok(Command::VolumeUp) => {
                        events.push(KeyEvent { key: KeyPress::VolumeUp, down: true });
                        events.push(KeyEvent { key: KeyPress::VolumeUp, down: false });
                    }
                    Ok(Command::VolumeDown) => {
                        events.push(KeyEvent { key: KeyPress::VolumeDown, down: true });
                        events.push(KeyEvent { key: KeyPress::VolumeDown, down: false });
                    }
                    Ok(Command::Lock) => sb.shell.lock_now(),
                    Ok(Command::Palette) => sb.shell.open_palette(),
                    Ok(Command::Switcher) => sb.shell.open_switcher(),
                    // The settings app decided what the chain should do; we
                    // are the process with the PipeWire connection.
                    Ok(Command::ReloadPrefs) => {
                        let (fresh, errs) = effects::load_prefs();
                        for e in &errs {
                            sb.shell.note(format!("prefs: {e}"));
                        }
                        // set_prefs is the shell's own path for a settings
                        // change, so everything that follows from one —
                        // cold tuning, layout, the ambient band — happens
                        // exactly as it would from the shell's own sheet.
                        let out = sb.shell.set_prefs(fresh);
                        sb.execute(out);
                        sb.shell.note("prefs reloaded".to_string());
                    }
                    Ok(Command::SpeakerProps(pairs)) => {
                        let out = tideshell_egui::effects::apply_speaker_props(&pairs);
                        sb.shell.note(format!("{out:?}"));
                    }
                    Err(t) => sb.shell.note(format!("ctl: unknown command {t:?}")),
                }
            }
        }
        // Steering follows cold/glove mode: with the touchscreen unreliable,
        // the keys move a selection instead of the volume.
        sb.volkeys.steering = sb.shell.cold.on;
        let mut intents: Vec<Intent> = Vec::new();
        for e in events {
            match (e.key, e.down) {
                (KeyPress::Power, true) => actions.extend(sb.shell.power_key()),
                (KeyPress::Power, false) => {}
                (KeyPress::VolumeUp, d) => intents.extend(if d { sb.volkeys.press(VolKey::Up, now_ms) } else { sb.volkeys.release(VolKey::Up, now_ms) }),
                (KeyPress::VolumeDown, d) => intents.extend(if d { sb.volkeys.press(VolKey::Down, now_ms) } else { sb.volkeys.release(VolKey::Down, now_ms) }),
            }
        }
        intents.extend(sb.volkeys.tick(now_ms));
        for i in intents {
            use tideshell_core::control_center::SliderKind;
            let v = sb.shell.drawer.volume;
            match i {
                Intent::Volume(VolKey::Up) => actions.extend(sb.shell.set_slider(SliderKind::Volume, (v + 0.05).min(1.0))),
                Intent::Volume(VolKey::Down) => actions.extend(sb.shell.set_slider(SliderKind::Volume, (v - 0.05).max(0.0))),
                // Steering: the palette is the shell's list of everything,
                // so it is what the keys walk when a finger will not do.
                Intent::Step(k) => {
                    if !sb.shell.palette.open {
                        sb.shell.open_palette();
                    }
                    sb.shell.palette_step(matches!(k, VolKey::Down));
                }
                Intent::Activate => actions.extend(sb.shell.palette_activate_selected()),
                // A hold with no binding opens the palette rather than doing
                // nothing: it is the one action that leads to every other.
                Intent::Bound(_) => sb.shell.open_palette(),
                Intent::Chord => sb.shell.lock_now(),
            }
        }
        actions.extend(sb.shell.tick(now_ms));
        sb.execute(actions);
        if let Some(on) = sb.pending_display_power.take() {
            let msg = io.set_display_power(on);
            sb.shell.note(msg);
        }
        if let Some(deg) = sb.pending_rotate {
            // Needs the compositor's first `done`; keep it pending until then.
            match io.set_transform(deg) {
                Ok(msg) => {
                    sb.pending_rotate = None;
                    sb.shell.note(msg);
                }
                Err(e) if e.starts_with("no output state") => {}
                Err(e) => {
                    sb.pending_rotate = None;
                    sb.shell.note(format!("rotate: {e}"));
                }
            }
        }
        // Compared by reference: the owning accessors allocated twice each
        // per iteration, on a path that runs whether anything changed or
        // not, and a status string changes perhaps once a session.
        if io.config_status_ref() != last_config_status {
            last_config_status = io.config_status();
            sb.shell.note(last_config_status.clone());
        }
        if io.power_status_ref() != last_power_status {
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
                side_due.remove(&role);
                continue;
            };
            let events = side_events.remove(&role).unwrap_or_default();
            // A configure is not an egui event, so a resize arriving while
            // egui wants no frame would be skipped and never repainted at
            // the new size. Compare against what was last actually painted.
            let resized = last_frame_hash.get(&role).is_some_and(|(_, was)| *was != t.size_px());
            // Before prepare and bind, not after: binding is an
            // eglMakeCurrent, and doing one per surface per iteration to
            // then decide there was nothing to draw is the cost this check
            // exists to avoid.
            if events.is_empty() && !resized && side_due.get(&role).is_some_and(|due| now_ms < *due) {
                continue;
            }
            t.prepare(&mut io);
            let size_px = t.size_px();
            let (w, h) = t.screen_points();
            if gl.bind(role, &t.surface, size_px).is_err() {
                continue;
            }
            // egui says when it wants nothing more: `repaint_delay` from
            // the previous run. Honouring it skips the run and the
            // tessellation entirely — a step earlier than the frame-hash
            // skip, which only saves the fill, and the tessellation is
            // where the per-frame allocations are.
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))),
                time: Some(t0.elapsed().as_secs_f64()),
                events,
                focused: true,
                ..Default::default()
            };
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(t.scale);
            let info = FrameInfo { apps: &apps, time: tideshell_egui::sensors::local_time(), battery: None, trackpad_note: sb.trackpad_note(), t_secs: t0.elapsed().as_secs_f32() };
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
            // When the next frame is due, from egui's own scheduling. A
            // delay it reports as very large means "never until something
            // happens"; cap it so a surface cannot wedge itself if a
            // wake-up is ever missed.
            let delay_ms = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay.as_millis()).unwrap_or(0);
            side_due.insert(role, now_ms + delay_ms.min(1_000) as u64);
            let prims = ctx_side.tessellate(out.shapes, out.pixels_per_point);
            // Skip a surface whose frame is identical to the one already on
            // screen. Tessellation has happened either way; what is saved is
            // the fill and the commit, which is where the pixels are.
            let h = (tideshell_egui::damage::frame_hash(&prims), size_px);
            if !tideshell_egui::damage::changed(last_frame_hash.get(&role).copied(), h, !out.textures_delta.is_empty()) {
                sb.execute(actions);
                continue;
            }
            last_frame_hash.insert(role, h);
            if !gl.renderer.is_empty() && !renderer_noted {
                renderer_noted = true;
                sb.view.accelerated = tideshell_core::paintrate::accelerated_renderer(&gl.renderer);
                sb.shell.note(format!(
                    "renderer: {} ({})",
                    gl.renderer,
                    if sb.view.accelerated { "accelerated" } else { "SOFTWARE — animations are rate-limited" }
                ));
            }
            if let Err(e) = gl.paint(role, size_px, out.pixels_per_point, &prims, &out.textures_delta) {
                eprintln!("tideshell: {role:?}: {e}");
            }
            sb.execute(actions);
        }

        // Overlay: map/unmap, input region, frame.
        let want = sb.view.needs_overlay(&sb.shell);
        if !want {
            gl.unbind(Role::Overlay);
            io.ensure_overlay(false);
            // Forget the hash: the surface's contents are gone, so the next
            // frame is not "identical to what is on screen" — there is
            // nothing on screen.
            last_frame_hash.remove(&Role::Overlay);
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
        let (w, h) = t.screen_points();
        if let Err(e) = gl.bind(Role::Overlay, &t.surface, size_px) {
            eprintln!("tideshell: {e}");
            continue;
        }
        // Edge/pad-owned sequences go to the engine and never reach egui.
        let mut events = std::mem::take(&mut overlay_events);
        let routed = sb.view.route_raw(&mut sb.shell, &mut events, Pos2::ZERO, t0.elapsed().as_millis() as u64);
        sb.execute(routed);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))),
            time: Some(t0.elapsed().as_secs_f64()),
            events,
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(t.scale);
        let info = FrameInfo { apps: &apps, time: tideshell_egui::sensors::local_time(), battery: None, trackpad_note: sb.trackpad_note(), t_secs: t0.elapsed().as_secs_f32() };
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
        // The overlay is the big one: full screen, and painted every
        // iteration whatever it contained. With swap_interval(0) each swap
        // also produces Wayland events, so the next poll found the fd ready
        // and never slept — the loop free-ran at over a thousand iterations
        // a second while nothing moved. Skipping the identical frame is what
        // lets it block.
        let h = (tideshell_egui::damage::frame_hash(&prims), size_px);
        if !tideshell_egui::damage::changed(last_frame_hash.get(&Role::Overlay).copied(), h, !out.textures_delta.is_empty()) {
            continue;
        }
        last_frame_hash.insert(Role::Overlay, h);
        if let Err(e) = gl.paint(Role::Overlay, size_px, out.pixels_per_point, &prims, &out.textures_delta) {
            eprintln!("tideshell: {e}");
        }
    }
}
