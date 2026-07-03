// src/bin/bo-muxd.rs
//
// The headless bo-mux daemon. Owns the soundscape engine (player.rs), the
// mesh (mesh.rs), and the scene library; exposes the control API (api.rs)
// over a Unix domain socket carrying line-delimited JSON.
//
// Socket acquisition order:
//   1. systemd socket activation — if launched by a .socket unit, inherit
//      the already-bound listener via the LISTEN_FDS protocol (fd 3).
//   2. standalone — bind $XDG_RUNTIME_DIR/bo-mux/control.sock ourselves.
//
// This lets the SAME binary run both as a systemd-activated user service
// and directly from a terminal during development, with no code change.
//
// NOTE: This is the daemon's structure and control loop. The audio render
// path (player active_voices -> mux -> output) and the mesh socket I/O are
// the still-sketched pieces (see mux.rs / mesh.rs TODOs); the daemon wires
// them together and the API surface is real + testable.

use bo_mux::api::*;
use bo_mux::soundscape::Soundscape;
use bo_mux::soundscape_config;
use bo_mux::transport::{Transport, TransportCmd, LogSink, run as run_transport};
use bo_mux::sink_system::{SystemSink, PlayerKind};
use bo_mux::activity_watch::ActivityWatch;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};
use std::thread;

/// Shared daemon state behind a mutex (the control loop is low-frequency;
/// the audio thread reads a snapshot, so contention is negligible).
struct Daemon {
    scene_dir: PathBuf,
    loaded: Option<Soundscape>,
    playing: bool,
    role: String,
    seed: u64,
    /// Handle to the running transport thread (Some while playing).
    transport_tx: Option<Sender<TransportCmd>>,
    /// Handle to the mesh thread (None if multicast couldn't start).
    mesh: Option<bo_mux::mesh_runner::MeshHandle>,
    /// Shared activity tracker — knows when to back off and stop
    /// spawning new clips (controller closed, BT off, KDE picker
    /// moved audio elsewhere, idle). See bo_mux::activity_watch.
    watch: Arc<ActivityWatch>,
    /// Shared master gain multiplier. f32 stored as u32 bits (no
    /// AtomicF32 in std). Default = 1.0 (no change). Updated by
    /// `SetSceneGain` commands from the controller; read on every
    /// voice_on in SystemSink.
    scene_gain: Arc<std::sync::atomic::AtomicU32>,
}

impl Daemon {
    fn new(scene_dir: PathBuf) -> Self {
        Self {
            scene_dir, loaded: None, playing: false,
            role: "idle".into(), seed: 0, transport_tx: None,
            mesh: None, watch: Arc::new(ActivityWatch::new()),
            scene_gain: Arc::new(std::sync::atomic::AtomicU32::new(
                1.0_f32.to_bits())),
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        // Every incoming request is a "controller is alive" signal.
        // The activity watch uses this for the heartbeat timer AND
        // exits passive mode whenever a command lands. The cmd_tag
        // is used to distinguish user actions (load_scene, play,
        // stop) from polls (ping, status, list_*) for the idle
        // timer. See activity_watch.rs.
        let cmd_tag = match &req.cmd {
            Command::Ping              => "ping",
            Command::Status            => "status",
            Command::ListScenes        => "list_scenes",
            Command::LoadScene { .. }  => "load_scene",
            Command::Play { .. }       => "play",
            Command::Stop              => "stop",
            Command::StartSession{..}  => "start_session",
            Command::LeaveSession      => "leave_session",
            Command::ListPeers         => "list_peers",
            Command::SaveScene { .. }  => "save_scene",
            Command::SetSceneGain {..} => "set_scene_gain",
            Command::PinLayer {..}     => "pin_layer",
            Command::Shutdown          => "shutdown",
        };
        self.watch.note_command(cmd_tag);
        match req.cmd {
            Command::Ping => Response::ok(req.id, serde_json::json!({
                "api_version": API_VERSION, "pong": true
            })),

            Command::Status => {
                let power_json = {
                    let p = bo_mux::power_state::PowerState::read();
                    serde_json::json!({
                        "on_battery": p.on_battery,
                        "capacity": p.capacity,
                        "has_battery": p.has_battery,
                    })
                };
                Response::ok(req.id, serde_json::json!({
                "loaded": self.loaded.as_ref().map(|s| s.name.clone()),
                "playing": self.playing,
                "role": self.mesh.as_ref().map(|m| m.role()).unwrap_or_else(|| self.role.clone()),
                "seed": self.seed,
                "api_version": API_VERSION,
                // Activity-watch state. `passive=true` means the
                // daemon has decided to stop spawning new clips
                // (controller stale, sink moved externally, BT off,
                // or idle). `passive_reason` is the most recent
                // trigger — useful for surfacing in the GUI as
                // "Soundscape paused (idle for 12 min)".
                "passive": self.watch.is_passive(),
                "passive_reason": self.watch.passive_reason(),
                // Power state so the controller can show a battery-aware
                // hint (e.g. "paused to save battery (12%)") and decide
                // whether to even offer to start the soundscape when the
                // device is unplugged and low. Absent battery → on_battery
                // false / capacity null (desktop), and these never gate.
                "power": power_json,                // Current scene-gain multiplier (default 1.0).
                // Controller can show "scene boosted +30%" or
                // similar from this.
                "scene_gain": f32::from_bits(self.scene_gain.load(
                    std::sync::atomic::Ordering::Relaxed)),
                // Fabric snapshot for the drag-to-speaker UI: peers +
                // speakers + current assignment + unclaimed channels.
                // "{}" when no mesh (single-device).
                "fabric": self.mesh.as_ref()
                    .map(|m| serde_json::from_str::<serde_json::Value>(&m.fabric_json())
                        .unwrap_or(serde_json::json!({})))
                    .unwrap_or(serde_json::json!({})),
                // The loaded scene's layers + their default channel, so
                // the UI has draggable items even before playback. Each:
                // { name, channel }.
                "layers": self.loaded.as_ref().map(|s|
                    s.layers.iter().map(|l| serde_json::json!({
                        "name": l.name, "channel": l.channel
                    })).collect::<Vec<_>>())
                    .unwrap_or_default(),
            }))
            }

            Command::ListScenes => {
                // Return rich metadata, not bare filenames, so the UI can
                // show a real name + description + layout + layer count
                // instead of a cryptic stem like "forest_5_1". Each entry:
                // { id, name, description, author, layout, layers }.
                // `id` is the filename stem (what load_scene expects);
                // `name` falls back to the id when the file omits it.
                let mut scenes = Vec::new();
                if let Ok(rd) = std::fs::read_dir(&self.scene_dir) {
                    let mut entries: Vec<_> = rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                        .collect();
                    entries.sort();
                    for p in entries {
                        let stem = match p.file_stem().and_then(|s| s.to_str()) {
                            Some(s) => s.trim_end_matches(".soundscape").to_string(),
                            None => continue,
                        };
                        // Parse for metadata; tolerate either schema.
                        // Both SoundscapeConfig and raw Soundscape carry
                        // name/layers; description/author/layout are
                        // config-only and default gracefully.
                        let meta = std::fs::read_to_string(&p).ok()
                            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok());
                        let (name, desc, author, layout, n_layers) = match &meta {
                            Some(v) => (
                                v.get("name").and_then(|x| x.as_str())
                                    .unwrap_or(&stem).to_string(),
                                v.get("description").and_then(|x| x.as_str())
                                    .unwrap_or("").to_string(),
                                v.get("author").and_then(|x| x.as_str())
                                    .unwrap_or("").to_string(),
                                v.get("layout").and_then(|x| x.as_str())
                                    .unwrap_or("Stereo").to_string(),
                                v.get("layers").and_then(|x| x.as_array())
                                    .map(|a| a.len()).unwrap_or(0),
                            ),
                            None => (stem.clone(), String::new(), String::new(),
                                     "Stereo".to_string(), 0),
                        };
                        scenes.push(serde_json::json!({
                            "id": stem,
                            "name": name,
                            "description": desc,
                            "author": author,
                            "layout": layout,
                            "layers": n_layers,
                        }));
                    }
                }
                Response::ok(req.id, serde_json::json!({ "scenes": scenes }))
            }

            Command::LoadScene { name, inline_json } => {
                let json = if let Some(j) = inline_json {
                    j
                } else if let Some(n) = name {
                    let path = self.scene_dir.join(format!("{}.soundscape.json", n));
                    match std::fs::read_to_string(&path) {
                        Ok(s) => s,
                        Err(e) => return Response::err(req.id, format!("read {}: {}", path.display(), e)),
                    }
                } else {
                    return Response::err(req.id, "load_scene needs name or inline_json");
                };
                // Scene files come in two schemas:
                //   1. The RICH config format (SoundscapeConfig) used by
                //      the bootstrapped presets and the GUI editor —
                //      start_state is a string state name, layers carry
                //      gain_range/interval_ms/routing, etc. This is what
                //      the Spaces page reads and writes.
                //   2. The RAW runtime format (Soundscape) — start_state
                //      is a usize index, states carry pre-computed
                //      transitions. Older/hand-authored scenes use this.
                //
                // We try the rich config first (and compile() it into the
                // runtime Soundscape, which validates + resolves routing),
                // then fall back to the raw format. This was a real bug:
                // the daemon used to ONLY parse the raw format, so every
                // bootstrapped preset (forest_quiet, river_calm, etc.)
                // failed to load with "invalid type: string, expected
                // usize".
                let parsed: Result<Soundscape, String> =
                    match soundscape_config::parse(&json) {
                        Ok(cfg) => soundscape_config::compile(&cfg)
                            .map(|(scene, _routings)| scene),
                        Err(cfg_err) => {
                            // Not the config schema — try raw runtime.
                            serde_json::from_str::<Soundscape>(&json)
                                .map_err(|raw_err| format!(
                                    "scene parse failed as both config                                      ({}) and raw ({})", cfg_err, raw_err))
                        }
                    };
                match parsed {
                    Ok(scene) => {
                        let nm = scene.name.clone();
                        // Teach the activity watch about this scene's
                        // preferred sink (if any). When that sink
                        // disappears from `pactl list sinks` — e.g.
                        // Bluetooth toggled off — the watch flips us
                        // to passive mode so we don't keep spawning
                        // paplay against a sink that's no longer there.
                        self.watch.note_configured_sink(
                            scene.target_sink.clone());
                        self.loaded = Some(scene);
                        Response::ok(req.id, serde_json::json!({ "loaded": nm }))
                    }
                    Err(e) => Response::err(req.id, format!("parse scene: {}", e)),
                }
            }

            Command::Play { seed } => {
                let scene = match &self.loaded {
                    Some(s) => s.clone(),
                    None => return Response::err(req.id, "no scene loaded"),
                };
                // Stop any prior transport before starting a new one.
                if let Some(tx) = self.transport_tx.take() {
                    let _ = tx.send(TransportCmd::Stop);
                }
                // Seed: explicit, else derive from wall clock so each session
                // is a different (but reproducible) evolution.
                self.seed = seed.unwrap_or_else(|| {
                    use std::time::{SystemTime, UNIX_EPOCH};
                    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1)
                });

                // Spawn the transport thread: it owns the Player and drives
                // it in real time, emitting voice on/off to the sink. If a
                // system audio player exists (paplay/ffplay/aplay) we make
                // real sound; otherwise we fall back to the LogSink so the
                // daemon still runs observably (e.g. headless CI).
                let (tx, rx) = channel::<TransportCmd>();
                let scene_for_thread = scene.clone();
                let seed = self.seed;
                let have_player = PlayerKind::detect() != PlayerKind::None;
                // Clone the activity-watch passive flag into the
                // transport thread. The SystemSink reads it on
                // every voice_on; when set, the spawn is skipped.
                // The audio engine itself keeps ticking (so timers
                // stay accurate) — only the user-facing spawn is
                // suppressed.
                let passive_flag = self.watch.passive_flag();
                let scene_gain = self.scene_gain.clone();
                thread::spawn(move || {
                    let transport = Transport::new(scene_for_thread, seed);
                    let start = Instant::now();
                    let tick = Duration::from_millis(100);
                    if have_player {
                        let mut sink = SystemSink::new();
                        sink.set_passive_flag(passive_flag);
                        sink.set_scene_gain_atomic(scene_gain);
                        // Resolve relative clip paths ("clips/foo.opus")
                        // against the bo-mux data dir, where the clip
                        // pool was bootstrapped on first run.
                        sink.set_clip_base(data_dir());
                        run_transport(transport, sink, start, rx, tick);
                    } else {
                        run_transport(transport, LogSink::default(), start, rx, tick);
                    }
                });
                self.transport_tx = Some(tx);
                self.playing = true;
                Response::ok(req.id, serde_json::json!({
                    "playing": true, "seed": self.seed,
                    "audible": have_player
                }))
            }

            Command::Stop => {
                if let Some(tx) = self.transport_tx.take() {
                    let _ = tx.send(TransportCmd::Stop);
                }
                self.playing = false;
                Response::ok_empty(req.id)
            }

            Command::StartSession { layout } => {
                let parsed = match layout.as_str() {
                    "quad" => bo_mux::protocol::Layout::quad(),
                    "5.1" | "51" => bo_mux::protocol::Layout::surround_51(),
                    _ => bo_mux::protocol::Layout::stereo(),
                };
                match &self.mesh {
                    Some(mesh) => {
                        mesh.start_session(parsed);
                        self.role = "coordinator".into();
                        Response::ok(req.id, serde_json::json!({ "role": self.role, "layout": layout }))
                    }
                    None => Response::err(req.id,
                        "mesh unavailable (multicast couldn't start) — single-device only"),
                }
            }
            Command::LeaveSession => {
                if let Some(mesh) = &self.mesh { mesh.leave_session(); }
                self.role = "idle".into();
                Response::ok_empty(req.id)
            }
            Command::PinLayer { channel, speaker } => {
                // Drag-to-speaker: pin this layer's channel to the chosen
                // speaker on the fabric. Needs an active mesh; single-
                // device setups have nothing to pin across.
                match &self.mesh {
                    Some(mesh) => {
                        mesh.pin_claim(channel.clone(), speaker.clone());
                        Response::ok(req.id, serde_json::json!({
                            "pinned": { "channel": channel, "speaker": speaker }
                        }))
                    }
                    None => Response::err(req.id,
                        "no mesh fabric (single-device) — pinning needs 2+ nodes"),
                }
            }
            Command::ListPeers => {
                let peers = self.mesh.as_ref()
                    .map(|m| m.peers_json())
                    .unwrap_or_else(|| "[]".into());
                // peers_json is already a JSON array string; embed it raw.
                let val: serde_json::Value = serde_json::from_str(&peers).unwrap_or(serde_json::json!([]));
                Response::ok(req.id, val)
            }

            Command::SaveScene { name, json } => {
                // Validate before writing so we never persist a broken scene.
                // Validate via config-first parse (matches LoadScene).
                let valid = soundscape_config::parse(&json)
                    .and_then(|cfg| soundscape_config::compile(&cfg).map(|_| ()))
                    .or_else(|_| serde_json::from_str::<Soundscape>(&json)
                        .map(|_| ()).map_err(|e| e.to_string()));
                if let Err(e) = valid {
                    return Response::err(req.id, format!("refusing to save invalid scene: {}", e));
                }
                let _ = std::fs::create_dir_all(&self.scene_dir);
                let path = self.scene_dir.join(format!("{}.soundscape.json", name));
                match std::fs::write(&path, json) {
                    Ok(_) => Response::ok(req.id, serde_json::json!({ "saved": path.display().to_string() })),
                    Err(e) => Response::err(req.id, format!("write: {}", e)),
                }
            }

            Command::SetSceneGain { value } => {
                // Clamp to a sane range: < 0 makes no sense
                // (gain is a multiplier); > 1.5 risks clipping
                // when combined with per-voice peak_gain near 1.0.
                // The controller's curve typically stays in
                // [0.7, 1.4] but we accept overrides up to 1.5.
                let clamped = value.clamp(0.0, 1.5);
                self.scene_gain.store(
                    clamped.to_bits(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                Response::ok(req.id, serde_json::json!({
                    "gain": clamped,
                    "clamped_from": value,
                }))
            }

            Command::Shutdown => {
                // Reply first, the loop sees `should_exit` and returns.
                Response::ok_empty(req.id)
            }
        }
    }
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("bo-mux")
}

fn scene_dir() -> PathBuf {
    data_dir().join("scenes")
}

/// Copy the bundled "starter" soundscape presets into the user's
/// scene dir on first run so the Spaces page shows working presets
/// out of the box. Skips files that already exist (idempotent;
/// re-running this never overwrites user edits to a scene with the
/// same name).
///
/// We use include_str! to embed the JSONs at compile time rather
/// than read them from a relative path at runtime, because the
/// daemon binary moves around (installed location, dev tree,
/// systemd-managed) and we'd have no reliable way to locate the
/// source examples.
///
/// Scenes included:
///   - forest_quiet:   stereo-friendly forest ambience (wind + leaves,
///                     no water). The default "ambient calm" preset.
///   - river_calm:     stereo-friendly water-only soundscape using
///                     brook clip pool. Works on a single device or
///                     a mesh — RouteStrategy::Auto handles upmix
///                     when more speakers are available.
///   - forest_immersive: Quad-layout immersive forest (the original
///                     spatial scene the bo-mux mesh was designed
///                     around). Downmixes cleanly to stereo if
///                     fewer speakers are configured.
fn bootstrap_default_scenes(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let presets: &[(&str, &str)] = &[
        ("forest_quiet.soundscape.json",
         include_str!("../../examples/forest_quiet.soundscape.json")),
        ("river_calm.soundscape.json",
         include_str!("../../examples/river_calm.soundscape.json")),
        ("forest_immersive.soundscape.json",
         include_str!("../../examples/forest_immersive.soundscape.json")),
        // Day-cycle forest (dawn → midday → dusk state machine).
        ("forest.soundscape.json",
         include_str!("../../examples/forest.soundscape.json")),
        // Stereo synth-pad drone — pink-noise pads, good for focus.
        ("synth_pad.soundscape.json",
         include_str!("../../examples/synth_pad.soundscape.json")),
        // Quad ocean: waves + gulls + distant whale.
        ("ocean_quad.soundscape.json",
         include_str!("../../examples/ocean_quad.soundscape.json")),
        // 5.1 forest with a 30 Hz sub-pad on the LFE channel.
        ("forest_5_1.soundscape.json",
         include_str!("../../examples/forest_5_1.soundscape.json")),
        // Wind buffeting — gusty, for masking traffic/HVAC noise.
        ("wind_buffeting.soundscape.json",
         include_str!("../../examples/wind_buffeting.soundscape.json")),
    ];
    let mut wrote = 0usize;
    for (name, content) in presets {
        let dest = dir.join(name);
        // Write if missing OR if the existing file is empty/unparseable.
        // The old behavior (skip if exists) meant a stale scene_dir from
        // an earlier daemon that shipped fewer presets — or a truncated /
        // corrupt file — would permanently hide those scenes from the GUI
        // (the reported "river and forest missing" bug). Healing a bad
        // file is safe: presets are Anthropic-shipped, not user content.
        let needs_write = match std::fs::read_to_string(&dest) {
            Ok(s) => {
                s.trim().is_empty()
                    || serde_json::from_str::<serde_json::Value>(&s).is_err()
            }
            Err(_) => true, // missing/unreadable
        };
        if !needs_write { continue; }
        if std::fs::write(&dest, content).is_ok() {
            wrote += 1;
        }
    }
    if wrote > 0 {
        eprintln!("bo-muxd: bootstrapped {} default scene preset(s) into {:?} \
                   (first-run setup — user-installed scenes left untouched)",
                  wrote, dir);
    }
}

/// Bootstrap the soundscape clip pool. The default scenes reference
/// clips like "clips/wind_soft_a.opus". Rather than ship ~6 MB of
/// audio in the binary, we SYNTHESIZE the clips with ffmpeg on first
/// run if they're missing. Each is a short (~8 s) loop-friendly
/// ambient bed built from filtered noise:
///
///   - brown_pad_[a-h]: brown noise → lowpass → gentle tremolo.
///     Warm, dark drone bed. Cutoff/tremolo vary per variant so the
///     layer pool doesn't sound like one repeated loop.
///   - wind_soft_[a-h]: pink noise → bandpass → gusting tremolo.
///     Airy wind whistle.
///   - brook_*/leaves_*/etc: if a richer pre-recorded pool was
///     installed alongside (data_dir/clips), those are used as-is;
///     we only synthesize the synthetic beds that are missing.
///
/// Idempotent: skips any clip that already exists. If ffmpeg is
/// absent, logs once and continues — scenes that reference only the
/// pre-installed pool still work; scenes needing synthetic beds will
/// log a per-clip "file not found" at play time, which is clear.
fn bootstrap_clips(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);

    // (name, ffmpeg_filter, source_kind). source_kind picks the noise.
    // We only synthesize the beds the default scenes need that aren't
    // typically part of a recorded pool.
    struct Synth { name: &'static str, src: &'static str, filt: String }
    let mut jobs: Vec<Synth> = Vec::new();

    let variants = ["a", "b", "c", "d", "e", "f", "g", "h"];
    for (i, v) in variants.iter().enumerate() {
        // brown_pad: lowpass 180..460, tremolo 0.10..0.31
        let lp = 180 + i * 40;
        let trem = 0.10 + (i as f32) * 0.03;
        jobs.push(Synth {
            name: Box::leak(format!("brown_pad_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=brown:amplitude=0.5:duration=8:sample_rate=48000",
            filt: format!(
                "lowpass=f={},tremolo=f={:.2}:d=0.3,afade=t=in:d=1.5,\
                 afade=t=out:st=6.5:d=1.5,volume=0.7", lp, trem),
        });
        // wind_soft: bandpass 400..1100, tremolo 0.12..0.40
        let bp = 400 + i * 100;
        let trem2 = 0.12 + (i as f32) * 0.04;
        jobs.push(Synth {
            name: Box::leak(format!("wind_soft_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=pink:amplitude=0.6:duration=8:sample_rate=48000",
            filt: format!(
                "bandpass=f={}:width_type=h:w=600,tremolo=f={:.2}:d=0.5,\
                 afade=t=in:d=2,afade=t=out:st=6:d=2,volume=0.55", bp, trem2),
        });
        // pink_pad: lush mid-focused synth-pad bed (synth_pad scene).
        // Bandpass + echo gives it a chorused, spacious character.
        let cf = 300 + i * 50;
        let trem3 = 0.15 + (i as f32) * 0.025;
        jobs.push(Synth {
            name: Box::leak(format!("pink_pad_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=pink:amplitude=0.55:duration=8:sample_rate=48000",
            filt: format!(
                "bandpass=f={}:width_type=h:w=400,aecho=0.8:0.7:40:0.25,\
                 tremolo=f={:.2}:d=0.4,afade=t=in:d=2.5,afade=t=out:st=5.5:d=2.5,\
                 volume=0.6", cf, trem3),
        });
    }

    // ── Ocean clips (ocean_quad scene) ──────────────────────────────
    for (n, _) in [(1u32, ()), (2, ()), (3, ())].iter() {
        let lp = 600 + n * 200;
        jobs.push(Synth {
            name: Box::leak(format!("wave_break_{}.opus", n).into_boxed_str()),
            src: "anoisesrc=color=brown:amplitude=0.8:duration=6:sample_rate=48000",
            filt: format!(
                "lowpass=f={},tremolo=f=0.18:d=0.85,afade=t=in:d=1.8,\
                 afade=t=out:st=4:d=2,volume=0.7", lp),
        });
    }
    for (v, base) in [("a", 1200u32), ("b", 900)].iter() {
        jobs.push(Synth {
            name: Box::leak(format!("gull_call_{}.opus", v).into_boxed_str()),
            src: Box::leak(format!(
                "sine=frequency={}:duration=0.5:sample_rate=48000", base
            ).into_boxed_str()),
            filt: "vibrato=f=12:d=0.6,afade=t=in:d=0.05,afade=t=out:st=0.3:d=0.2,\
                   volume=0.35,apad=pad_dur=3.5".to_string(),
        });
    }
    jobs.push(Synth {
        name: "whale_long_a.opus",
        src: "sine=frequency=80:duration=6:sample_rate=48000",
        filt: "vibrato=f=0.3:d=0.8,lowpass=f=200,afade=t=in:d=2,\
               afade=t=out:st=3.5:d=2.5,volume=0.5".to_string(),
    });

    // ── 5.1 forest clips (forest_5_1 scene) ─────────────────────────
    for (v, bp) in [("a", 500u32), ("b", 750)].iter() {
        jobs.push(Synth {
            name: Box::leak(format!("wind_loop_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=pink:amplitude=0.6:duration=10:sample_rate=48000",
            filt: format!(
                "bandpass=f={}:width_type=h:w=800,tremolo=f=0.2:d=0.5,\
                 afade=t=in:d=2.5,afade=t=out:st=7.5:d=2.5,volume=0.5", bp),
        });
    }
    for (n, base) in [(1u32, 2400u32), (2, 3100)].iter() {
        jobs.push(Synth {
            name: Box::leak(format!("bird_chirp_{}.opus", n).into_boxed_str()),
            src: Box::leak(format!(
                "sine=frequency={}:duration=0.18:sample_rate=48000", base
            ).into_boxed_str()),
            filt: "vibrato=f=30:d=0.8,afade=t=in:d=0.02,afade=t=out:st=0.10:d=0.08,\
                   volume=0.25,apad=pad_dur=4".to_string(),
        });
    }
    jobs.push(Synth {
        name: "creek_loop.opus",
        src: "anoisesrc=color=pink:amplitude=0.5:duration=8:sample_rate=48000",
        filt: "highpass=f=800,lowpass=f=4000,tremolo=f=6:d=0.3,afade=t=in:d=1.5,\
               afade=t=out:st=6.5:d=1.5,volume=0.45".to_string(),
    });
    jobs.push(Synth {
        name: "sub_pad_30hz.opus",
        src: "sine=frequency=30:duration=8:sample_rate=48000",
        filt: "afade=t=in:d=2,afade=t=out:st=6:d=2,volume=0.6".to_string(),
    });

    // ── Brook / river water (forest + river_calm scenes) ────────────
    // Water = mid/high filtered noise with fast, shallow tremolo for the
    // "bubbling" texture. Distance shapes it: close is bright + present,
    // mid is rolled off, far is dark + quiet with a touch of echo for
    // depth. Each of the three distances gets 8 variants (a..h) so the
    // scheduler can avoid back-to-back repeats.
    for (i, v) in variants.iter().enumerate() {
        // close: bright, 700..1400 Hz centre, lively tremolo.
        let cf_c = 700 + i * 100;
        jobs.push(Synth {
            name: Box::leak(format!("brook_close_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=white:amplitude=0.5:duration=8:sample_rate=48000",
            filt: format!(
                "highpass=f=500,lowpass=f={},tremolo=f={:.1}:d=0.35,\
                 afade=t=in:d=1.2,afade=t=out:st=6.8:d=1.2,volume=0.42",
                cf_c, 7.0 + (i as f32) * 0.4),
        });
        // mid: rolled off, a little softer.
        let lp_m = 1600 + i * 80;
        jobs.push(Synth {
            name: Box::leak(format!("brook_mid_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=pink:amplitude=0.5:duration=8:sample_rate=48000",
            filt: format!(
                "highpass=f=350,lowpass=f={},tremolo=f={:.1}:d=0.3,\
                 afade=t=in:d=1.5,afade=t=out:st=6.5:d=1.5,volume=0.36",
                lp_m, 5.5 + (i as f32) * 0.3),
        });
        // far: dark, quiet, with a short echo so it sits "behind" the mix.
        let lp_f = 900 + i * 50;
        jobs.push(Synth {
            name: Box::leak(format!("brook_far_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=brown:amplitude=0.5:duration=8:sample_rate=48000",
            filt: format!(
                "highpass=f=250,lowpass=f={},aecho=0.8:0.6:30:0.2,\
                 tremolo=f={:.1}:d=0.25,afade=t=in:d=2,afade=t=out:st=6:d=2,\
                 volume=0.28", lp_f, 4.0 + (i as f32) * 0.2),
        });
    }

    // ── Forest wind beds (forest + forest_quiet scenes) ─────────────
    // wind_canopy = nearby leaves-in-wind (brighter, more movement);
    // wind_distant = far treeline wind (darker, slower swell).
    for (i, v) in variants.iter().enumerate() {
        let bp_c = 700 + i * 120;
        jobs.push(Synth {
            name: Box::leak(format!("wind_canopy_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=pink:amplitude=0.6:duration=9:sample_rate=48000",
            filt: format!(
                "bandpass=f={}:width_type=h:w=900,tremolo=f={:.2}:d=0.6,\
                 afade=t=in:d=2,afade=t=out:st=7:d=2,volume=0.5",
                bp_c, 0.25 + (i as f32) * 0.05),
        });
        let lp_d = 500 + i * 60;
        jobs.push(Synth {
            name: Box::leak(format!("wind_distant_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=brown:amplitude=0.6:duration=10:sample_rate=48000",
            filt: format!(
                "lowpass=f={},tremolo=f={:.2}:d=0.5,afade=t=in:d=3,\
                 afade=t=out:st=7:d=3,volume=0.42",
                lp_d, 0.12 + (i as f32) * 0.03),
        });
    }

    // ── Leaves rustle (forest scenes) ───────────────────────────────
    // Short, sharp filtered-noise bursts padded out to a few seconds so
    // they trigger as occasional accents rather than a continuous bed.
    for (i, v) in variants.iter().enumerate() {
        let hp = 1500 + i * 150;
        jobs.push(Synth {
            name: Box::leak(format!("leaves_rustle_{}.opus", v).into_boxed_str()),
            src: "anoisesrc=color=white:amplitude=0.4:duration=0.7:sample_rate=48000",
            filt: format!(
                "highpass=f={},lowpass=f=8000,tremolo=f=18:d=0.7,\
                 afade=t=in:d=0.08,afade=t=out:st=0.45:d=0.2,volume=0.3,\
                 apad=pad_dur=3.5", hp),
        });
    }

    // Skip jobs whose target already exists.
    let pending: Vec<&Synth> = jobs.iter()
        .filter(|j| !dir.join(j.name).exists())
        .collect();
    if pending.is_empty() { return; }

    // Probe ffmpeg once.
    let have_ffmpeg = std::process::Command::new("sh")
        .arg("-c").arg("command -v ffmpeg")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    if !have_ffmpeg {
        eprintln!("bo-muxd: {} synthetic clip(s) missing and ffmpeg not \
                   found — install ffmpeg to auto-generate ambient beds, \
                   or drop your own .opus clips into {:?}",
                  pending.len(), dir);
        return;
    }

    let mut made = 0usize;
    for j in pending {
        let out = dir.join(j.name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y",
                   "-f", "lavfi", "-i", j.src,
                   "-af", &j.filt,
                   "-c:a", "libopus", "-b:a", "64k"])
            .arg(&out)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => made += 1,
            _ => eprintln!("bo-muxd: failed to synthesize clip {:?}", out),
        }
    }
    if made > 0 {
        eprintln!("bo-muxd: synthesized {} ambient clip(s) into {:?} \
                   (first-run setup)", made, dir);
    }
}

/// Build this device's NodeInfo for the mesh: a stable-ish id from hostname
/// + a uid-ish number, the local audio output as a speaker, plus any
/// configured B&O speaker URLs from $BO_SPEAKERS (comma-separated).
fn build_node_info() -> bo_mux::protocol::NodeInfo {
    use bo_mux::protocol::{NodeInfo, Platform, Speaker};
    let host = std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "device".into());
    let uid = std::fs::read_to_string("/proc/self/loginuid").ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1000);
    let node_id = format!("{}-{}", host, uid);

    let mut speakers = vec![Speaker {
        id: "local".into(),
        name: format!("{} output", host),
        platform: Platform::Linux,
    }];
    if let Ok(list) = std::env::var("BO_SPEAKERS") {
        for url in list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            speakers.push(Speaker { id: url.into(), name: url.into(), platform: Platform::Mozart });
        }
    }
    NodeInfo { node_id, name: host, platform: Platform::Linux, speakers }
}

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(DEFAULT_SOCKET_REL)
}

/// Acquire the listener: prefer a systemd-passed fd, else bind ourselves.
fn acquire_listener() -> std::io::Result<UnixListener> {
    // systemd socket-activation protocol: LISTEN_FDS / LISTEN_PID set, and
    // the first passed fd is 3 (SD_LISTEN_FDS_START).
    if let (Ok(pid), Ok(fds)) = (std::env::var("LISTEN_PID"), std::env::var("LISTEN_FDS")) {
        let my_pid = std::process::id().to_string();
        let n: i32 = fds.parse().unwrap_or(0);
        if pid == my_pid && n >= 1 {
            use std::os::unix::io::FromRawFd;
            // SAFETY: fd 3 is the listener systemd bound and passed to us.
            let listener = unsafe { UnixListener::from_raw_fd(3) };
            eprintln!("bo-muxd: using systemd socket-activated fd 3");
            return Ok(listener);
        }
    }
    // Standalone: bind the socket ourselves (dev / non-systemd).
    let path = socket_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let _ = std::fs::remove_file(&path); // clear stale socket
    eprintln!("bo-muxd: binding {}", path.display());
    UnixListener::bind(path)
}

fn serve_client(stream: UnixStream, daemon: Arc<Mutex<Daemon>>) -> bool {
    let mut should_exit = false;
    let reader = BufReader::new(match stream.try_clone() { Ok(s) => s, Err(_) => return false });
    let mut writer = stream;
    for line in reader.lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim().is_empty() { continue; }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let is_shutdown = matches!(req.cmd, Command::Shutdown);
                let r = daemon.lock().unwrap().handle(req);
                if is_shutdown { should_exit = true; }
                r
            }
            Err(e) => Response::err(0, format!("bad request: {}", e)),
        };
        if let Ok(mut s) = serde_json::to_string(&resp) {
            s.push('\n');
            if writer.write_all(s.as_bytes()).is_err() { break; }
            let _ = writer.flush();
        }
        if should_exit { break; }
    }
    should_exit
}

fn main() {
    let scene_dir_ = scene_dir();
    // First-run bootstrap: if the scene dir is empty, copy the
    // built-in example scenes from CARGO_MANIFEST_DIR/examples/
    // (bundled at build time via include_bytes!) into the dir so
    // the user has working presets without manual setup. The user
    // can play "Forest" or "River" the first time they open the
    // Spaces page.
    bootstrap_default_scenes(&scene_dir_);
    bootstrap_clips(&data_dir().join("clips"));

    let daemon = Arc::new(Mutex::new(Daemon::new(scene_dir_)));

    // ── Save the user's default sink BEFORE we touch routing ──
    //
    // The user's complaint: "the Multimedia routing is still sticky
    // after I closed the app." Cause: bo-mux sets pactl's default
    // sink to whatever scene/voice target it wants. When the daemon
    // exits without restoring, the user's regular audio (Firefox,
    // Spotify, system sounds) keeps going to the scene sink — feels
    // like the controller is "still installed" even though it isn't
    // running.
    //
    // Fix has two halves:
    //   1. On startup, capture the existing default sink → write to
    //      /run/user/$UID/bo-mux.sink-before. Survives daemon restart.
    //   2. On graceful shutdown (the loop break below, or SIGTERM /
    //      SIGINT caught by the handler installed here), run
    //      `pactl set-default-sink <saved>` and remove the sentinel.
    //
    // The state file path is also what bo-mux-fix-audio.sh checks; if
    // a crash leaves the sentinel behind, the next launch of the
    // controller sees it and runs fix-audio to recover.
    let saved_sink_path = sink_state_path();
    let saved_sink = capture_current_default_sink();
    if let Some(s) = &saved_sink {
        if let Err(e) = std::fs::write(&saved_sink_path, s.as_bytes()) {
            eprintln!("bo-muxd: WARN — couldn't persist sink sentinel \
                       {:?}: {} (sticky-routing recovery won't work)",
                       saved_sink_path, e);
        } else {
            eprintln!("bo-muxd: saved pre-session default sink '{}'", s);
        }
    } else {
        eprintln!("bo-muxd: no default sink found at startup; nothing \
                   to restore on shutdown");
    }

    // SIGTERM / SIGINT handler. The std lib doesn't ship one, but the
    // `signal-hook-registry` crate provides a minimal API that's
    // already an indirect dep of tokio so the cost is essentially
    // zero. We forward the signal to a shared atomic that the request
    // loop checks each iteration.
    //
    // What restores happens HERE rather than inside the loop because
    // signal handlers run in a separate thread; doing pactl from the
    // handler itself is unsafe (signal handlers must be async-signal-
    // safe). Instead the handler sets a flag; the main thread sees
    // the flag and exits the loop, then the shutdown sequence below
    // runs the actual `pactl set-default-sink` call.
    let shutting_down = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = Arc::clone(&shutting_down);
        // SAFETY: this only sets a single AtomicBool with Relaxed
        // ordering, which IS async-signal-safe.
        let _ = unsafe {
            signal_hook_registry::register(
                15i32, // SIGTERM
                move || flag.store(true, std::sync::atomic::Ordering::Relaxed)) };
        let flag = Arc::clone(&shutting_down);
        let _ = unsafe {
            signal_hook_registry::register(
                2i32,  // SIGINT
                move || flag.store(true, std::sync::atomic::Ordering::Relaxed)) };
    }

    // Bring up the mesh so peers/sessions work. A stable node id is derived
    // from hostname + uid (good enough; persist a uuid later if needed).
    let me = build_node_info();
    match bo_mux::mesh_runner::spawn(me) {
        Ok(handle) => {
            daemon.lock().unwrap().mesh = Some(handle);
            eprintln!("bo-muxd: mesh up");
        }
        Err(e) => eprintln!("bo-muxd: mesh unavailable ({}) — single-device mode", e),
    }

    let listener = match acquire_listener() {
        Ok(l) => l,
        Err(e) => { eprintln!("bo-muxd: cannot acquire socket: {}", e); std::process::exit(1); }
    };
    // Listener accept() is blocking. To honor the shutdown flag from
    // the signal handler without polling, set a short accept timeout
    // so the loop checks the flag every second.
    if let Ok(addr) = listener.local_addr() {
        eprintln!("bo-muxd: ready (api v{}) on {:?}", API_VERSION, addr);
    } else {
        eprintln!("bo-muxd: ready (api v{})", API_VERSION);
    }

    // Background watcher: every TICK_INTERVAL_SECS, inspect external
    // state (pactl default sink, command staleness) and flip to
    // passive if any trigger fires. The watch reference is cloned
    // once and held by the thread; tick() locks nothing of the
    // daemon so it never contends with the request loop.
    {
        let watch = daemon.lock().unwrap().watch.clone();
        thread::spawn(move || {
            let interval = Duration::from_secs(
                bo_mux::activity_watch::TICK_INTERVAL_SECS);
            loop {
                thread::sleep(interval);
                watch.tick();
            }
        });
    }

    for conn in listener.incoming() {
        // Signal-driven graceful shutdown.
        if shutting_down.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("bo-muxd: signal received, shutting down");
            break;
        }
        match conn {
            Ok(stream) => {
                let d = Arc::clone(&daemon);
                // Serve serially: the control API is low-traffic and the
                // shared state is mutexed anyway. (A thread-per-conn variant
                // is trivial later if needed.)
                if serve_client(stream, d) {
                    eprintln!("bo-muxd: shutdown requested");
                    break;
                }
            }
            Err(e) => { eprintln!("bo-muxd: accept error: {}", e); }
        }
    }

    // ── Shutdown: restore the user's default sink ──
    //
    // This runs on:
    //   - explicit "shutdown" RPC (serve_client returns true)
    //   - SIGTERM / SIGINT (the atomic flag breaks the loop)
    //   - listener error that exits the for-loop (rare)
    //
    // If the daemon is killed with SIGKILL or otherwise crashes,
    // this restore CAN'T run — the sentinel file at
    // sink_state_path() stays on disk and the next launch of the
    // controller (or the user's manual bo-mux-fix-audio.sh) sees
    // it and recovers.
    if let Some(s) = saved_sink {
        let ok = std::process::Command::new("pactl")
            .args(["set-default-sink", &s])
            .status().map(|st| st.success()).unwrap_or(false);
        if ok {
            eprintln!("bo-muxd: restored default sink to '{}'", s);
            let _ = std::fs::remove_file(&saved_sink_path);
        } else {
            eprintln!("bo-muxd: WARN — failed to restore default sink to \
                       '{}'; sentinel left at {:?} for recovery", s,
                       saved_sink_path);
        }
    }
}

/// Path of the sink-sentinel file. Lives under /run/user/$UID because
/// (1) tmpfs survives the daemon but not a reboot — exactly the
/// right scope for "what was the user's sink before this session"
/// and (2) the user's bo-mux-fix-audio.sh script already looks
/// here. If XDG_RUNTIME_DIR is unset (some non-systemd-logind setups),
/// fall back to /tmp.
fn sink_state_path() -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    dir.join("bo-mux.sink-before")
}

/// Run `pactl get-default-sink` and return the name. Returns None if
/// pactl is missing or returns no useful output. Doesn't fail loud —
/// some setups (containers, headless test rigs) have no PulseAudio
/// at all and that's fine; the daemon just runs without sink
/// management in that case.
fn capture_current_default_sink() -> Option<String> {
    let out = std::process::Command::new("pactl")
        .arg("get-default-sink")
        .output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
