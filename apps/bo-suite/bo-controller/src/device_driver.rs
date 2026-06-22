// src/device_driver.rs
//
// Driver abstraction for "the thing we're sending audio to."
// Up until this commit the controller assumed every device was a
// Mozart-firmware speaker and tried REST/DLNA paths against it. The
// reality (per the user's logs): some devices are non-Mozart and
// reject every REST endpoint, leaving Bluetooth as the only viable
// audio path.
//
// This module defines a uniform interface for talking to any
// audio target — Mozart speaker over REST/DLNA, generic speaker
// over Bluetooth + local PulseAudio sink, or a no-op stub when the
// device is unreachable or unrecognized. The QtState layer then
// constructs the right driver per active device and dispatches
// volume/play/stop operations through the trait, never special-
// casing firmware family at the call site.
//
// ── Why enum dispatch instead of trait objects ────────────────────
//
// Trait objects with async fn need `async_trait` (extra dep, boxed
// futures everywhere) or `Pin<Box<dyn Future>>` boilerplate. Enum
// dispatch trades the open-set property (any new driver can be
// plugged in without modifying this enum) for static dispatch +
// zero runtime overhead. We have a known finite set of driver
// kinds, so the enum approach wins on simplicity + perf.
//
// ── Bluetooth playback model ──────────────────────────────────────
//
// When the user picks a non-Mozart device and clicks play, the BT
// driver:
//
//   1. Connects via `bluetoothctl connect <mac>` (idempotent — if
//      already connected, bluetoothctl returns immediately).
//   2. Derives the PulseAudio sink name from the MAC. PA's naming
//      convention for BT audio sinks is bluez_sink.<MAC_WITH_UNDER-
//      SCORES>.a2dp_sink (older PA) or bluez_output.<MAC>.1 (PA 15+
//      and PipeWire). The driver tries both.
//   3. Spawns `paplay --device=<sink> <file>` to play the file.
//      paplay handles WAV/AU natively; for Opus/MP3/FLAC etc the
//      caller is expected to have already decoded to a paplay-
//      compatible format (the Mix & Play path produces WAV-style
//      Opus that paplay won't play directly — see TODO).
//   4. Tracks the child PID so subsequent `stop()` calls can SIGTERM
//      the right process.
//
// Volume on BT goes through `pactl set-sink-volume`. AVRCP would
// also work but is more variable across devices; pactl is uniform.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;

use crate::firmware_family::FirmwareFamily;

/// Configure a child process to receive SIGTERM the moment its parent
/// (this controller process) dies, by ANY means: clean exit, SIGKILL,
/// window-manager force-close, crash, OOM-killer, anything. Without
/// this, when the controller exits the kernel reparents paplay/ffmpeg
/// to init/systemd and they keep playing audio long after we're gone
/// — exactly the user-visible bug ("music kept playing after I closed
/// the app").
///
/// Implementation: use Linux's prctl(PR_SET_PDEATHSIG, SIGTERM) via a
/// pre_exec hook on tokio's Command. The hook runs in the freshly-
/// forked child between fork() and exec(), so we set the parent-death
/// signal on the child itself BEFORE it becomes paplay/ffmpeg.
///
/// Why SIGTERM and not SIGKILL: SIGTERM lets paplay flush its PA
/// stream cleanly so PA doesn't log "stream destroyed under us"
/// warnings. paplay handles SIGTERM by exiting within ~100ms which is
/// fast enough for the user-visible "playback stopped" outcome.
///
/// Safety: pre_exec runs in the child between fork and exec. It MUST
/// be async-signal-safe — only call functions explicitly marked as
/// such (see signal-safety(7)). prctl is on that list.
#[cfg(unix)]
fn add_pdeath_sig(cmd: &mut Command) {
    // tokio::process::Command exposes pre_exec as an inherent method
    // on unix targets, so we don't need to import std's CommandExt.
    // SAFETY: pre_exec's closure runs in the child after fork() and
    // before exec(). prctl is async-signal-safe. We don't touch any
    // shared state, allocate, or call non-async-signal-safe functions.
    unsafe {
        cmd.pre_exec(|| {
            // SIGTERM, not SIGKILL: gives paplay a chance to flush
            // its PA stream cleanly. SIGTERM still terminates within
            // ~100ms which is more than fast enough.
            let r = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
            if r != 0 {
                // prctl failed (rare — would mean the kernel doesn't
                // support PR_SET_PDEATHSIG, which is essentially
                // every Linux since 2.1.57). Continue anyway; the
                // child still spawns, just without die-with-parent.
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn add_pdeath_sig(_cmd: &mut Command) {
    // No-op on non-Unix. The user is Linux-only per project scope,
    // so this never actually runs, but the cfg gate keeps tests
    // and (theoretical) cross-compilation honest.
}

/// Errors any driver can produce. Coarse-grained on purpose — most
/// of the callers just want to surface a string in the UI.
#[derive(Debug, Clone)]
#[allow(dead_code)]  // Network/State variants reserved for future
                     // driver expansions; Subprocess + Unsupported
                     // cover today's usage.
pub enum DriverError {
    /// The driver doesn't support this operation. E.g. URI playback
    /// requested from the BT driver, or any operation on Noop.
    Unsupported(String),
    /// An OS subprocess failed (paplay, pactl, bluetoothctl). The
    /// message includes stderr or the exit code.
    Subprocess(String),
    /// HTTP / REST call failed (Mozart driver).
    Network(String),
    /// Driver state is inconsistent (e.g. asked to stop with no
    /// active playback).
    State(String),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::Unsupported(s) => write!(f, "unsupported: {}", s),
            DriverError::Subprocess(s)  => write!(f, "subprocess: {}", s),
            DriverError::Network(s)     => write!(f, "network: {}", s),
            DriverError::State(s)       => write!(f, "state: {}", s),
        }
    }
}

/// Mozart driver — thin wrapper over the existing reqwest::Client +
/// base_url. Volume/play operations defer to the same modules the
/// controller already uses (play_uri, mozart_source_switch, etc).
/// This driver doesn't reimplement that logic; it just provides a
/// uniform shape so the call site can stay generic.
#[derive(Debug, Clone)]
#[allow(dead_code)]  // `client` is held for forthcoming driver-direct
                     // probes (volume read/write through the driver
                     // trait rather than the legacy Command channel).
pub struct MozartDriver {
    pub base_url: String,
    pub client:   reqwest::Client,
}

/// Bluetooth driver — talks to the device via system audio plumbing
/// rather than the device's own REST API. Audio goes paplay → PulseAudio
/// → bluez → speaker. Volume goes through pactl on the BT sink.
#[derive(Debug, Clone)]
pub struct BluetoothDriver {
    /// Bluetooth MAC address of the speaker. Required to derive the
    /// PulseAudio sink name + to issue `bluetoothctl connect`.
    pub mac: String,
    /// Optional human-readable name for logging.
    pub name: String,
    /// PID of the currently-spawned paplay (or 0 if nothing's playing).
    /// Tracked here rather than in QtState so the same driver instance
    /// can stop its own playback without coordinating with another
    /// thread.
    pub playing_pid: Arc<Mutex<Option<u32>>>,
    /// The sink name that the most recent `play_local_file` actually
    /// routed to. This can DIFFER from sink_name()/sink_name_pipewire()
    /// when the preferred sink wasn't present and play fell back to an
    /// existing bluez sink. Volume commands must target the sink that's
    /// actually playing — otherwise audio comes out of speaker A while
    /// `pactl set-sink-volume` targets non-existent speaker B and fails
    /// on both preferred names (the reported "set-sink-volume failed on
    /// both PA and PipeWire" symptom). None until the first play.
    pub active_sink: Arc<Mutex<Option<String>>>,
}

/// Noop driver — for devices we can't reach or don't recognize.
/// Every operation returns Unsupported with a clear message so the
/// UI can show something useful.
#[derive(Debug, Clone)]
pub struct NoopDriver {
    pub reason: String,
}

/// Multi-room driver — fans audio across 2+ Bluetooth speakers via
/// a PulseAudio combine-sink. Clock sync is handled by PA itself
/// (resamples per-slave to match the master clock), giving us
/// reasonably-tight phase alignment without proprietary protocols.
///
/// Lifecycle:
///   * `create(members)` — issues `pactl load-module
///     module-combine-sink sink_name=bo_multi_<hash> slaves=<sinks>`,
///     captures the module ID for teardown
///   * `play_local_file` — paplay --device=bo_multi_<hash>
///   * `teardown()` — `pactl unload-module <id>`, frees the virtual
///     sink. Called explicitly by QtState when the user clears the
///     binding; we don't rely on Drop because cloned driver instances
///     across the worker boundary would each try to unload
///
/// `combine_module_id` is shared via Arc so cloned drivers see the
/// same teardown state. Once teardown runs, the Option is None and
/// subsequent teardowns are no-ops.
#[derive(Debug, Clone)]
pub struct MultiRoomDriver {
    /// PulseAudio sink name we created. Format: `bo_multi_<hash>`
    /// where the hash is computed from the member MACs (so the same
    /// set of speakers produces the same sink name, avoiding leaks
    /// across reconnects).
    pub sink_name: String,
    /// Constituent speakers — `(mac, friendly_name)` tuples. The
    /// UI uses the names for display; the MACs map to PA sink names
    /// via the BluetoothDriver naming convention.
    pub members: Vec<(String, String)>,
    /// PulseAudio module ID — needed for `pactl unload-module` on
    /// teardown. None after teardown runs, so subsequent calls are
    /// idempotent. Shared via Arc across driver clones.
    pub combine_module_id: Arc<Mutex<Option<u32>>>,
    /// PID of the currently-spawned paplay (or None if nothing's
    /// playing). Same reaper machinery as BluetoothDriver.
    pub playing_pid: Arc<Mutex<Option<u32>>>,
}

/// Local driver — plays through the controller machine's default
/// audio sink rather than the device. Same plumbing as
/// BluetoothDriver (ffmpeg→paplay) but with no `--device` argument
/// so paplay routes to the system default. This is the right
/// answer for several real situations:
///
///   * Active device is non-Mozart and no BT speaker is paired —
///     better to play audio HERE than to do nothing
///   * User wants to preview a track without disturbing the room
///     it'd normally play in
///   * Soundscape output preview before deploying to the speaker
///
/// The driver shares the same paplay-via-pipe logic with
/// BluetoothDriver to avoid divergence. Volume is controlled via
/// `pactl set-sink-volume @DEFAULT_SINK@` which means the
/// controller's system volume slider is affected, which is what the
/// user expects.
#[derive(Debug, Clone)]
pub struct LocalDriver {
    /// PID of the currently-spawned paplay (or None if nothing's
    /// playing). Same reaper machinery as BluetoothDriver.
    pub playing_pid: Arc<Mutex<Option<u32>>>,
}

/// Enum dispatch over the concrete driver types. The QtState layer
/// holds one of these per active device; method calls dispatch
/// statically to the appropriate impl.
#[derive(Debug, Clone)]
pub enum AnyDriver {
    Mozart(MozartDriver),
    Bluetooth(BluetoothDriver),
    /// Plays through the controller's default audio sink. The
    /// answer for "device can't accept REST URI playback and no
    /// BT speaker is bound — just play it on the local machine".
    Local(LocalDriver),
    /// Multi-room: fans audio across 2+ BT speakers via a PA
    /// combine-sink. Driver lifecycle includes explicit teardown
    /// to free the virtual sink.
    MultiRoom(MultiRoomDriver),
    Noop(NoopDriver),
}

impl AnyDriver {
    /// Construct the right driver from a detected firmware family
    /// plus the connection identifier (URL for REST, MAC for BT).
    /// Falls back to `Local` (audio plays on the controller's own
    /// speakers) when the device can't accept REST playback AND no
    /// Bluetooth MAC is bound. Only the device-truly-unreachable case
    /// produces a Noop.
    pub fn for_family(family: FirmwareFamily,
                      url: &str,
                      bt_mac: Option<&str>,
                      bt_name: Option<&str>,
                      client: reqwest::Client) -> AnyDriver
    {
        match family {
            FirmwareFamily::Mozart => AnyDriver::Mozart(MozartDriver {
                base_url: url.to_string(),
                client,
            }),
            FirmwareFamily::BeolinkConverse
            | FirmwareFamily::LegacyMozart
            | FirmwareFamily::Unknown => {
                if let Some(mac) = bt_mac {
                    AnyDriver::Bluetooth(BluetoothDriver {
                        mac: mac.to_string(),
                        name: bt_name.unwrap_or("").to_string(),
                        playing_pid: Arc::new(Mutex::new(None)),
                        active_sink: Arc::new(Mutex::new(None)),
                    })
                } else {
                    // No BT speaker bound. Fall back to Local rather
                    // than Noop — the user can still hear audio on
                    // the controller's own speakers, which beats
                    // silent failure. The UI surfaces a hint that
                    // they can pick a BT speaker for room playback.
                    AnyDriver::Local(LocalDriver {
                        playing_pid: Arc::new(Mutex::new(None)),
                    })
                }
            }
            FirmwareFamily::Unreachable => AnyDriver::Noop(NoopDriver {
                reason: "device unreachable — check network".to_string(),
            }),
        }
    }

    /// Family tag of this driver. Useful for UI logic that needs to
    /// branch on Mozart-vs-not without inspecting AnyDriver variants
    /// directly.
    #[allow(dead_code)]  // Reserved for upcoming driver-aware UI logic.
    pub fn family(&self) -> FirmwareFamily {
        match self {
            AnyDriver::Mozart(_)    => FirmwareFamily::Mozart,
            AnyDriver::Bluetooth(_) => FirmwareFamily::Unknown,
            AnyDriver::Local(_)     => FirmwareFamily::Unknown,
            AnyDriver::MultiRoom(_) => FirmwareFamily::Unknown,
            AnyDriver::Noop(_)      => FirmwareFamily::Unknown,
        }
    }

    /// Short human label for the UI ("Mozart REST", "Bluetooth (12:34:56:…)").
    pub fn label(&self) -> String {
        match self {
            AnyDriver::Mozart(d)    => format!("Mozart REST @ {}", d.base_url),
            AnyDriver::Bluetooth(d) => {
                if d.name.is_empty() {
                    format!("Bluetooth ({})", d.mac)
                } else {
                    format!("Bluetooth → {} ({})", d.name, d.mac)
                }
            }
            AnyDriver::Local(_) => "Local (controller speakers)".to_string(),
            AnyDriver::MultiRoom(d) => {
                let names: Vec<&str> = d.members.iter()
                    .map(|(_, n)| n.as_str()).collect();
                format!("Multi-room → {} ({} speakers)",
                        names.join(" + "), d.members.len())
            }
            AnyDriver::Noop(d) => format!("Noop ({})", d.reason),
        }
    }

    pub async fn play_local_file(
        &self,
        path: &Path,
        title: &str,
        start_offset_sec: u32,
        on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
    ) -> Result<(), DriverError>
    {
        match self {
            AnyDriver::Bluetooth(d) => d.play_local_file(path, title, start_offset_sec, on_end).await,
            AnyDriver::Local(d)     => d.play_local_file(path, title, start_offset_sec, on_end).await,
            AnyDriver::MultiRoom(d) => d.play_local_file(path, title, start_offset_sec, on_end).await,
            AnyDriver::Mozart(_) => Err(DriverError::Unsupported(
                "Mozart driver requires a URI; wrap the file via \
                 track_server first and call play_uri".to_string()
            )),
            AnyDriver::Noop(d) => Err(DriverError::Unsupported(d.reason.clone())),
        }
    }

    pub async fn stop(&self) -> Result<(), DriverError> {
        match self {
            AnyDriver::Bluetooth(d) => d.stop().await,
            AnyDriver::Local(d)     => d.stop().await,
            AnyDriver::MultiRoom(d) => d.stop().await,
            AnyDriver::Mozart(_)    => Ok(()),
            AnyDriver::Noop(_)      => Ok(()),
        }
    }

    #[allow(dead_code)]
    pub async fn get_volume_percent(&self) -> Result<u8, DriverError> {
        match self {
            AnyDriver::Bluetooth(d) => d.get_volume_percent().await,
            AnyDriver::Local(d)     => d.get_volume_percent().await,
            AnyDriver::MultiRoom(d) => d.get_volume_percent().await,
            AnyDriver::Mozart(_) => Err(DriverError::Unsupported(
                "Mozart volume is read via existing Command::GetVolume".to_string()
            )),
            AnyDriver::Noop(d) => Err(DriverError::Unsupported(d.reason.clone())),
        }
    }

    pub async fn set_volume_percent(&self, pct: u8) -> Result<(), DriverError> {
        match self {
            AnyDriver::Bluetooth(d) => d.set_volume_percent(pct).await,
            AnyDriver::Local(d)     => d.set_volume_percent(pct).await,
            AnyDriver::MultiRoom(d) => d.set_volume_percent(pct).await,
            AnyDriver::Mozart(_) => Err(DriverError::Unsupported(
                "Mozart volume is set via existing Command::SetVolume".to_string()
            )),
            AnyDriver::Noop(d) => Err(DriverError::Unsupported(d.reason.clone())),
        }
    }

    /// Teardown any driver-specific resources. Currently only the
    /// MultiRoom driver has non-trivial teardown (unloading the PA
    /// combine-sink module); other drivers no-op. Called when a
    /// driver is being replaced or the application is shutting down.
    pub async fn teardown(&self) -> Result<(), DriverError> {
        match self {
            AnyDriver::MultiRoom(d) => d.teardown().await,
            _ => Ok(()),
        }
    }

    /// Is this driver currently piping audio to a child process?
    /// Used by the sink-disappeared watchdog: if `is_playing() &&
    /// !sink_reachable()` for two consecutive polls, the sink is
    /// probably gone (BT speaker powered off, walked out of range,
    /// USB DAC unplugged) and we should stop before PulseAudio
    /// migrates the stream to a fallback sink the user didn't pick.
    /// Mozart and Noop drivers don't track local child PIDs and
    /// return false — they can't be "stuck" on a disappeared sink.
    pub fn is_playing(&self) -> bool {
        match self {
            AnyDriver::Bluetooth(d) => d.playing_pid.lock()
                .map(|g| g.is_some()).unwrap_or(false),
            AnyDriver::Local(d)     => d.playing_pid.lock()
                .map(|g| g.is_some()).unwrap_or(false),
            AnyDriver::MultiRoom(d) => d.playing_pid.lock()
                .map(|g| g.is_some()).unwrap_or(false),
            _ => false,
        }
    }
}

// ── BluetoothDriver impl ─────────────────────────────────────────

impl BluetoothDriver {
    /// Derive the canonical PulseAudio / PipeWire sink name for a BT
    /// MAC. PA's naming convention is `bluez_sink.AA_BB_CC_DD_EE_FF.
    /// a2dp_sink` (colons converted to underscores). PipeWire uses
    /// `bluez_output.AA_BB_CC_DD_EE_FF.1`. We return the PA-style name
    /// since pactl handles both, falling back to a probe if needed.
    pub fn sink_name(&self) -> String {
        let mac_underscored = self.mac.replace(':', "_").to_uppercase();
        format!("bluez_sink.{}.a2dp_sink", mac_underscored)
    }

    /// Alternative sink name used by newer PA + PipeWire. We try
    /// both during volume operations.
    pub fn sink_name_pipewire(&self) -> String {
        let mac_underscored = self.mac.replace(':', "_").to_uppercase();
        format!("bluez_output.{}.1", mac_underscored)
    }

    /// Ensure the BT device is connected. Idempotent — bluetoothctl
    /// returns immediately if already paired+connected.
    ///
    /// Fast-path optimization: `bluetoothctl info <mac>` is essentially
    /// instant (~50 ms) and tells us whether the device is already
    /// connected. If yes, we skip `connect` entirely — saving 100-200
    /// ms per play even on the happy path, and avoiding the rare cases
    /// where `connect` against an already-connected device hangs (BT
    /// stack quirk on some adapters under PipeWire).
    ///
    /// Slow path: if info says "Connected: no" or info itself fails, we
    /// issue `connect` with a 4-second timeout (was 10 s). 4 s covers
    /// initial-session negotiation on the common adapters; for BT
    /// stacks that genuinely need longer the music falls through to
    /// paplay's own retry which has historically worked even when
    /// bluetoothctl connect timed out (per user's real-device logs).
    /// The previous 10 s timeout added 6 s of pure wait to every
    /// failed play.
    pub async fn ensure_connected(&self) -> Result<(), DriverError> {
        // Fast-path: are we already connected?
        if self.is_connected_fast().await {
            return Ok(());
        }
        let out = Command::new("bluetoothctl")
            .args(["connect", &self.mac])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| DriverError::Subprocess(
                format!("spawn bluetoothctl: {}", e)
            ))?;
        let out = tokio::time::timeout(
            Duration::from_secs(4),
            out.wait_with_output(),
        ).await
            .map_err(|_| DriverError::Subprocess(
                "bluetoothctl connect timed out after 4s".to_string()
            ))?
            .map_err(|e| DriverError::Subprocess(
                format!("bluetoothctl wait: {}", e)
            ))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(DriverError::Subprocess(format!(
                "bluetoothctl connect {} failed: {}",
                self.mac,
                stderr.lines().next().unwrap_or("")
            )));
        }
        Ok(())
    }

    /// Cheap (~50 ms) "is this device currently connected" probe via
    /// `bluetoothctl info <mac>`. Returns false on any error —
    /// non-zero exit, parse failure, missing field. The caller
    /// follows up with a real connect attempt, so a false negative
    /// just costs a single retry; a false positive would skip the
    /// connect and the paplay-fallback path picks up the slack.
    async fn is_connected_fast(&self) -> bool {
        let res = tokio::time::timeout(
            Duration::from_millis(800),
            Command::new("bluetoothctl")
                .args(["info", &self.mac])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        ).await;
        let out = match res {
            Ok(Ok(o)) if o.status.success() => o,
            _ => return false,
        };
        let s = String::from_utf8_lossy(&out.stdout);
        // bluetoothctl output: `\tConnected: yes` or `\tConnected: no`.
        s.lines().any(|line| {
            let t = line.trim();
            t.eq_ignore_ascii_case("Connected: yes")
        })
    }

    /// Play a local file via paplay routed to the BT sink. Spawns
    /// paplay as a child process; stores the PID for later stop().
    ///
    /// Codec handling: paplay supports WAV/AU/SND natively, but not
    /// Opus/MP3/FLAC etc. For those, we pipe `ffmpeg -i <file> -f wav -`
    /// into paplay's stdin. The file extension drives the decision —
    /// .wav/.au/.snd go direct, everything else routes through ffmpeg.
    /// This makes the Mix & Play OggOpus output playable on a BT
    /// driver without re-encoding to WAV first.
    ///
    /// ffmpeg failures fall back to direct paplay so the caller at
    /// least gets a clear error if the file is malformed (rather than
    /// a confusing pipe-broke error).
    pub async fn play_local_file(
        &self,
        path: &Path,
        _title: &str,
        start_offset_sec: u32,
        on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
    ) -> Result<(), DriverError>
    {
        // Stop any prior playback first — paplay doesn't replace,
        // it stacks. Without this, repeated plays of different
        // files would all play simultaneously.
        let _ = self.stop().await;

        // Best-effort: ensure the BT link is up. Failure here isn't
        // fatal — paplay may still succeed if the system audio
        // graph is already routed correctly via a default-sink
        // setting. We log + continue.
        if let Err(e) = self.ensure_connected().await {
            eprintln!("bluetooth: connect to {} returned: {} \
                       — continuing, will try paplay anyway",
                       self.mac, e);
        }

        // ── Pre-flight: verify the bluez sink actually exists ──
        //
        // The user's symptom (issue: paplay returns clean, ffmpeg
        // exits 224, no sound): the bluez sink we point paplay at
        // doesn't exist because the BT device isn't actually
        // connected this session — even though we have a remembered
        // last_success from a prior session.
        //
        // Before: paplay would spawn, fail silently to "create sink
        // input: No such entity", exit successfully (it didn't
        // crash), ffmpeg would get SIGPIPE on its next write and
        // exit with 224 (broken pipe). No sound, no obvious error
        // in the log.
        //
        // Now: query `pactl list short sinks` and check whether our
        // target sink is in the list. If not, look for ANY bluez
        // sink that IS present and use it. If no bluez sink exists
        // at all, surface a clear error in last_play_status instead
        // of pretending to play.
        let preferred_sinks = vec![
            self.sink_name(),
            self.sink_name_pipewire(),
        ];
        let available = list_pactl_sinks();
        let sink = if available.is_empty() {
            // pactl failed entirely (PA/PipeWire not running?).
            // Try anyway — paplay will produce its own error.
            eprintln!(
                "bluetooth: WARNING — pactl returned no sinks (audio \
                 server may be down). Falling back to '{}'; if you \
                 hear no sound, restart pipewire-pulse or pulseaudio.",
                preferred_sinks[0]);
            preferred_sinks[0].clone()
        } else if let Some(s) = preferred_sinks.iter().find(|s| available.contains(s)) {
            s.clone()
        } else {
            // Our preferred sink isn't there. Look for any bluez sink.
            let bluez_alt: Option<&String> = available.iter()
                .find(|s| s.starts_with("bluez_sink.")
                       || s.starts_with("bluez_output."));
            match bluez_alt {
                Some(alt) => {
                    eprintln!(
                        "bluetooth: PRE-FLIGHT — wanted '{}' but it's \
                         not in pactl. Falling back to existing bluez \
                         sink '{}'. (To target a specific speaker, \
                         pair it on the Connect page and verify it \
                         reaches Connected: yes in bluetoothctl.)",
                        preferred_sinks[0], alt);
                    alt.clone()
                }
                None => {
                    let summary: String = available.iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(DriverError::Subprocess(format!(
                        "no Bluetooth sink available: '{}' not found \
                         in pactl and no other bluez_sink/bluez_output \
                         exists. Currently available sinks: [{}]. \
                         Connect a BT speaker (or pair via the system \
                         BT settings) before tapping play, or pick a \
                         different active device on the Connect page.",
                        preferred_sinks[0], summary)));
                }
            }
        };

        let path_str = path.to_string_lossy().to_string();
        let needs_decode = !is_paplay_native_format(path);
        eprintln!("bluetooth: play {} → sink={} (needs_decode={})",
                  path_str, sink, needs_decode);
        // Remember the sink we actually routed to so volume commands hit
        // the same speaker (it may be a fallback that differs from the
        // preferred sink_name()).
        if let Ok(mut g) = self.active_sink.lock() {
            *g = Some(sink.clone());
        }

        let pid_slot = self.playing_pid.clone();
        let pid = if needs_decode || start_offset_sec > 0 {
            // paplay can't seek; always go through ffmpeg when offset needed.
            spawn_ffmpeg_into_paplay(&path_str, &sink, start_offset_sec, pid_slot.clone(), on_end).await?
        } else {
            spawn_paplay_direct(&path_str, &sink, pid_slot.clone(), on_end).await?
        };

        {
            let mut g = self.playing_pid.lock().unwrap();
            *g = Some(pid);
        }
        Ok(())
    }

    /// Stop the currently-spawned paplay. SIGTERM the child by PID.
    /// If no PID is recorded, no-op (idempotent).
    pub async fn stop(&self) -> Result<(), DriverError> {
        let pid_opt = {
            let g = self.playing_pid.lock().unwrap();
            *g
        };
        if let Some(pid) = pid_opt {
            // kill is one syscall, won't block — but use the
            // shell `kill` binary instead of the libc raw call to
            // keep the dep surface flat.
            let r = Command::new("kill")
                .arg(pid.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            match r {
                Ok(s) if s.success() => {
                    let mut g = self.playing_pid.lock().unwrap();
                    *g = None;
                    Ok(())
                }
                Ok(s) => Err(DriverError::Subprocess(
                    format!("kill {} exit code {:?}", pid, s.code())
                )),
                Err(e) => Err(DriverError::Subprocess(
                    format!("kill {} spawn failed: {}", pid, e)
                )),
            }
        } else {
            Ok(())  // nothing playing → success
        }
    }

    /// Read sink volume via pactl. Tries the older PA name first,
    /// then the PipeWire name.
    #[allow(dead_code)]  // Wired via AnyDriver::get_volume_percent;
                         // currently invoked only from tests + future
                         // UI polling. The qt_method path goes through
                         // the legacy Command::GetVolume for Mozart.
    pub async fn get_volume_percent(&self) -> Result<u8, DriverError> {
        for sink in [self.sink_name(), self.sink_name_pipewire()] {
            let out = Command::new("pactl")
                .args(["get-sink-volume", &sink])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| DriverError::Subprocess(
                    format!("pactl spawn: {}", e)
                ))?;
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(p) = parse_pactl_volume(&text) {
                    return Ok(p);
                }
            }
        }
        Err(DriverError::Subprocess(
            "pactl get-sink-volume failed on both PA and PipeWire sink names"
                .to_string()
        ))
    }

    /// Set sink volume via pactl.
    /// Build the ordered list of sinks to try for a volume command:
    /// the sink we're actually playing on first (it may be a fallback
    /// that differs from the canonical names), then the canonical PA and
    /// PipeWire names, de-duplicated. Pure so it can be unit-tested.
    fn volume_sink_candidates(&self) -> Vec<String> {
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(g) = self.active_sink.lock() {
            if let Some(s) = g.as_ref() { candidates.push(s.clone()); }
        }
        for s in [self.sink_name(), self.sink_name_pipewire()] {
            if !candidates.contains(&s) { candidates.push(s); }
        }
        candidates
    }

    pub async fn set_volume_percent(&self, pct: u8) -> Result<(), DriverError> {
        let pct = pct.min(100);
        // Try the sink we're actually playing on FIRST (it may be a
        // fallback that differs from the preferred names), then the
        // canonical PA + PipeWire names as fallbacks. Without the active
        // sink first, volume would target a speaker that isn't playing
        // (or doesn't exist) and fail on both preferred names.
        for sink in &self.volume_sink_candidates() {
            let out = Command::new("pactl")
                .args(["set-sink-volume", sink, &format!("{}%", pct)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| DriverError::Subprocess(
                    format!("pactl spawn: {}", e)
                ))?;
            if out.status.success() {
                return Ok(());
            }
        }
        Err(DriverError::Subprocess(
            "pactl set-sink-volume failed on both PA and PipeWire sink names"
                .to_string()
        ))
    }
}

/// Decide whether paplay can read this file directly. paplay handles
/// libsndfile-supported formats (WAV/AU/SND/FLAC since libsndfile 1.0
/// supports it but paplay's older builds may not — we're conservative).
/// Everything else routes through `ffmpeg -i <file> -f wav -` which
/// transcodes to PCM and pipes to paplay's stdin.
///
/// We classify by extension rather than by content sniffing because:
///   * Sniffing requires an extra file open + read
///   * Misclassification is recoverable — paplay errors quickly and
///     the next play attempt can be told the right thing
///   * 99% of files have correct extensions in practice
pub fn is_paplay_native_format(path: &Path) -> bool {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    matches!(ext.as_deref(),
        Some("wav") | Some("au") | Some("snd"))
}

/// Spawn paplay reading from a file path directly. For WAV/AU/SND.
/// Returns the PID so the caller can record it for stop().
async fn spawn_paplay_direct(
    path: &str,
    sink: &str,
    pid_slot: Arc<Mutex<Option<u32>>>,
    on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
) -> Result<u32, DriverError> {
    eprintln!("bluetooth: paplay --device={} {}", sink, path);
    // Build the command with die-with-parent enforcement so paplay
    // doesn't survive the controller. kill_on_drop also fires when
    // the reaper task gets dropped (e.g. tokio runtime shutdown),
    // belt-and-braces alongside PR_SET_PDEATHSIG.
    let mut cmd = Command::new("paplay");
    cmd.args(["--device", sink])
       .arg(path)
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::piped())
       .kill_on_drop(true);
    add_pdeath_sig(&mut cmd);
    let child = cmd.spawn()
        .map_err(|e| DriverError::Subprocess(
            format!("spawn paplay: {}", e)
        ))?;
    let pid = child.id().unwrap_or(0);
    spawn_child_reaper(child, pid, sink.to_string(), pid_slot, on_end);
    Ok(pid)
}

/// Spawn `ffmpeg -i <path> -f wav -` and pipe its stdout into paplay's
/// stdin. ffmpeg decodes Opus/MP3/FLAC/etc to PCM; paplay streams
/// the WAV to the BT sink.
///
/// **Lifetime of the ffmpeg Child is critical.** The Child handle
/// must outlive paplay, or `kill_on_drop(true)` SIGKILLs ffmpeg the
/// moment this function returns. Without ffmpeg producing audio,
/// paplay sees a closed stdin and exits with code 1 (the symptom:
/// "music doesn't play"). We hold the Child in a detached tokio
/// task that awaits ffmpeg's natural exit — which happens when
/// paplay finishes and closes the read end of the pipe, causing
/// ffmpeg's next write to fail and the process to terminate.
///
/// PR_SET_PDEATHSIG remains as the death-with-parent safety net for
/// app shutdown; the reaper handles the in-session lifetime.
async fn spawn_ffmpeg_into_paplay(
    path: &str,
    sink: &str,
    start_offset_sec: u32,
    pid_slot: Arc<Mutex<Option<u32>>>,
    on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
) -> Result<u32, DriverError> {
    eprintln!("bluetooth: ffmpeg -ss {} -i {} -f wav - | paplay --device={}",
              start_offset_sec, path, sink);
    let mut ffmpeg_cmd = Command::new("ffmpeg");
    // -ss before -i = fast input-seek (keyframe accuracy, ~±100ms).
    // Build args dynamically so we only add -ss when seeking.
    let offset_str = format!("{}", start_offset_sec);
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error"];
    if start_offset_sec > 0 { args.extend(["-ss", &offset_str]); }
    args.extend(["-i", path, "-f", "wav", "-"]);
    ffmpeg_cmd.args(&args)
              .stdin(Stdio::null())
              .stdout(Stdio::piped())
              .stderr(Stdio::piped())
              .kill_on_drop(true);
    add_pdeath_sig(&mut ffmpeg_cmd);
    let mut ffmpeg = ffmpeg_cmd.spawn()
        .map_err(|e| DriverError::Subprocess(
            format!("spawn ffmpeg: {} (install with: apt install ffmpeg)", e)
        ))?;
    let ffmpeg_pid = ffmpeg.id().unwrap_or(0);
    let ffmpeg_stdout = ffmpeg.stdout.take().ok_or_else(|| DriverError::Subprocess(
        "ffmpeg stdout missing — couldn't pipe".to_string()
    ))?;
    let ffmpeg_stdout: Stdio = ffmpeg_stdout.try_into()
        .map_err(|e: std::io::Error| DriverError::Subprocess(
            format!("convert ffmpeg stdout to Stdio: {}", e)
        ))?;

    let mut paplay_cmd = Command::new("paplay");
    paplay_cmd.args(["--device", sink])
              .stdin(ffmpeg_stdout)
              .stdout(Stdio::null())
              .stderr(Stdio::piped())
              .kill_on_drop(true);
    add_pdeath_sig(&mut paplay_cmd);
    let paplay = paplay_cmd.spawn()
        .map_err(|e| DriverError::Subprocess(
            format!("spawn paplay (piped): {}", e)
        ))?;

    // Hold the ffmpeg Child in a detached task that awaits its
    // natural exit. Without this, the Child gets dropped at function
    // return and kill_on_drop(true) SIGKILLs ffmpeg immediately —
    // breaking paplay's stdin pipe and causing paplay to exit with
    // code 1 (no audio). The task ends when ffmpeg exits, which
    // happens organically after paplay closes its read end (paplay
    // finishes or is killed). PR_SET_PDEATHSIG is the app-exit
    // backstop if ffmpeg is still running.
    tokio::spawn(async move {
        match ffmpeg.wait().await {
            Ok(status) => {
                eprintln!("driver: ffmpeg (pid {}) exited with {:?}",
                          ffmpeg_pid, status.code());
            }
            Err(e) => {
                eprintln!("driver: ffmpeg (pid {}) wait failed: {}",
                          ffmpeg_pid, e);
            }
        }
    });

    let pid = paplay.id().unwrap_or(0);
    spawn_child_reaper(paplay, pid, sink.to_string(), pid_slot, on_end);
    Ok(pid)
}

/// Background task that waits on a paplay child, clears the PID
/// slot when it exits (so subsequent stop() is a no-op), and invokes
/// an optional callback signaling end-of-playback.
///
/// The callback is how the QueueManager learns to play the next
/// track: when paplay finishes naturally, the worker handler calls
/// us with a "send TrackEnded" closure, and that fires the auto-
/// advance path in QtState.
///
/// Note that stop() also clears playing_pid (after killing the
/// process), so the reaper's clear is for the natural-end path.
/// We guard against double-clearing by only resetting the slot if
/// it still matches OUR pid.
fn spawn_child_reaper(
    mut child: tokio::process::Child,
    pid: u32,
    sink: String,
    pid_slot: Arc<Mutex<Option<u32>>>,
    on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
) {
    tokio::spawn(async move {
        let status = child.wait().await;
        let reason = match &status {
            Ok(s) if s.success() => {
                eprintln!("driver: paplay on {} (pid {}) finished cleanly", sink, pid);
                EndReason::CompletedNaturally
            }
            Ok(s) => {
                eprintln!("driver: paplay on {} (pid {}) exited with {:?}",
                          sink, pid, s.code());
                // Exit-code nonzero is typically SIGTERM from our own
                // stop() or a paplay-internal error. Either way the
                // queue shouldn't auto-advance — the user (or an error)
                // intentionally stopped playback.
                EndReason::StoppedOrErrored
            }
            Err(e) => {
                eprintln!("driver: paplay wait error: {}", e);
                EndReason::StoppedOrErrored
            }
        };
        // Clear the PID slot — but only if it's still ours. A
        // racing stop() could have already cleared it AND assigned
        // a new PID for a different play.
        {
            let mut g = pid_slot.lock().unwrap();
            if *g == Some(pid) {
                *g = None;
            }
        }
        if let Some(cb) = on_end {
            cb(reason);
        }
    });
}

/// Why a paplay child stopped. Drives queue auto-advance logic:
/// CompletedNaturally fires the next track, StoppedOrErrored doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    CompletedNaturally,
    StoppedOrErrored,
}

// ── LocalDriver impl ─────────────────────────────────────────────

impl LocalDriver {
    /// Play a local file through the controller's default audio sink.
    /// Same ffmpeg→paplay machinery as the BT driver but with no
    /// --device argument so paplay routes to PA's default-sink.
    pub async fn play_local_file(
        &self,
        path: &Path,
        _title: &str,
        start_offset_sec: u32,
        on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
    ) -> Result<(), DriverError>
    {
        let _ = self.stop().await;

        let path_str = path.to_string_lossy().to_string();
        let needs_decode = !is_paplay_native_format(path);
        eprintln!("local: play {} (needs_decode={})", path_str, needs_decode);

        let pid_slot = self.playing_pid.clone();
        let pid = if needs_decode || start_offset_sec > 0 {
            spawn_ffmpeg_into_paplay_default(&path_str, start_offset_sec, pid_slot.clone(), on_end).await?
        } else {
            spawn_paplay_default(&path_str, pid_slot.clone(), on_end).await?
        };
        let mut g = self.playing_pid.lock().unwrap();
        *g = Some(pid);
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), DriverError> {
        let pid_opt = { *self.playing_pid.lock().unwrap() };
        if let Some(pid) = pid_opt {
            let r = Command::new("kill")
                .arg(pid.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            match r {
                Ok(s) if s.success() => {
                    *self.playing_pid.lock().unwrap() = None;
                    Ok(())
                }
                Ok(s) => Err(DriverError::Subprocess(
                    format!("kill {} exit code {:?}", pid, s.code())
                )),
                Err(e) => Err(DriverError::Subprocess(
                    format!("kill {} spawn failed: {}", pid, e)
                )),
            }
        } else {
            Ok(())
        }
    }

    /// Volume is read from the PA default sink. pactl supports
    /// "@DEFAULT_SINK@" as a magic identifier; we use it to avoid
    /// having to query the active sink name separately.
    pub async fn get_volume_percent(&self) -> Result<u8, DriverError> {
        let out = Command::new("pactl")
            .args(["get-sink-volume", "@DEFAULT_SINK@"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| DriverError::Subprocess(
                format!("pactl spawn: {}", e)
            ))?;
        if !out.status.success() {
            return Err(DriverError::Subprocess(
                "pactl get-sink-volume @DEFAULT_SINK@ failed".to_string()
            ));
        }
        parse_pactl_volume(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| DriverError::Subprocess(
                "couldn't parse pactl output".to_string()
            ))
    }

    pub async fn set_volume_percent(&self, pct: u8) -> Result<(), DriverError> {
        let pct = pct.min(100);
        let out = Command::new("pactl")
            .args(["set-sink-volume", "@DEFAULT_SINK@",
                   &format!("{}%", pct)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| DriverError::Subprocess(
                format!("pactl spawn: {}", e)
            ))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(DriverError::Subprocess(
                "pactl set-sink-volume failed".to_string()
            ))
        }
    }
}

/// Spawn paplay reading from a file with no --device argument
/// (paplay routes to the system default sink). Returns the PID.
async fn spawn_paplay_default(
    path: &str,
    pid_slot: Arc<Mutex<Option<u32>>>,
    on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
) -> Result<u32, DriverError> {
    eprintln!("local: paplay {}", path);
    let mut cmd = Command::new("paplay");
    cmd.arg(path)
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::piped())
       .kill_on_drop(true);
    add_pdeath_sig(&mut cmd);
    let child = cmd.spawn()
        .map_err(|e| DriverError::Subprocess(
            format!("spawn paplay: {}", e)
        ))?;
    let pid = child.id().unwrap_or(0);
    spawn_child_reaper(child, pid, "default".to_string(), pid_slot, on_end);
    Ok(pid)
}

/// Pipe ffmpeg's WAV output into paplay's stdin, no --device (default
/// sink). Used for compressed formats that paplay can't decode.
async fn spawn_ffmpeg_into_paplay_default(
    path: &str,
    start_offset_sec: u32,
    pid_slot: Arc<Mutex<Option<u32>>>,
    on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
) -> Result<u32, DriverError> {
    eprintln!("local: ffmpeg -ss {} -i {} -f wav - | paplay (default sink)", start_offset_sec, path);
    let mut ffmpeg_cmd = Command::new("ffmpeg");
    let offset_str = format!("{}", start_offset_sec);
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error"];
    if start_offset_sec > 0 { args.extend(["-ss", &offset_str]); }
    args.extend(["-i", path, "-f", "wav", "-"]);
    ffmpeg_cmd.args(&args)
              .stdin(Stdio::null())
              .stdout(Stdio::piped())
              .stderr(Stdio::piped())
              .kill_on_drop(true);
    add_pdeath_sig(&mut ffmpeg_cmd);
    let mut ffmpeg = ffmpeg_cmd.spawn()
        .map_err(|e| DriverError::Subprocess(
            format!("spawn ffmpeg: {} (install with: apt install ffmpeg)", e)
        ))?;
    let ffmpeg_pid = ffmpeg.id().unwrap_or(0);
    let ffmpeg_stdout = ffmpeg.stdout.take().ok_or_else(|| DriverError::Subprocess(
        "ffmpeg stdout missing".to_string()
    ))?;
    let ffmpeg_stdout: Stdio = ffmpeg_stdout.try_into()
        .map_err(|e: std::io::Error| DriverError::Subprocess(
            format!("convert ffmpeg stdout: {}", e)
        ))?;
    let mut paplay_cmd = Command::new("paplay");
    paplay_cmd.stdin(ffmpeg_stdout)
              .stdout(Stdio::null())
              .stderr(Stdio::piped())
              .kill_on_drop(true);
    add_pdeath_sig(&mut paplay_cmd);
    let paplay = paplay_cmd.spawn()
        .map_err(|e| DriverError::Subprocess(
            format!("spawn paplay (piped): {}", e)
        ))?;
    // Hold ffmpeg's Child so kill_on_drop(true) doesn't SIGKILL it
    // the moment this function returns — same pattern as the BT
    // helper. See that one's comment block for the rationale.
    tokio::spawn(async move {
        match ffmpeg.wait().await {
            Ok(status) => {
                eprintln!("driver: ffmpeg (pid {}) exited with {:?}",
                          ffmpeg_pid, status.code());
            }
            Err(e) => {
                eprintln!("driver: ffmpeg (pid {}) wait failed: {}",
                          ffmpeg_pid, e);
            }
        }
    });
    let pid = paplay.id().unwrap_or(0);
    spawn_child_reaper(paplay, pid, "default".to_string(), pid_slot, on_end);
    Ok(pid)
}

// ── MultiRoomDriver impl ─────────────────────────────────────────

impl MultiRoomDriver {
    /// Compute a stable sink name from member MACs. Same set of speakers
    /// → same sink name across runs, which lets us detect a pre-existing
    /// module on the PA daemon and reuse it (avoiding `cannot find slave
    /// sink` after a reconnect).
    fn sink_name_for(members: &[(String, String)]) -> String {
        let mut macs: Vec<String> = members.iter()
            .map(|(m, _)| m.replace(':', "_").to_uppercase())
            .collect();
        macs.sort();
        // FNV-1a on the joined MACs — 64-bit fits comfortably in a hex
        // suffix and gives plenty of collision room for a small set
        // of speakers.
        let joined = macs.join(",");
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in joined.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("bo_multi_{:016x}", hash)
    }

    /// Create a PulseAudio combine-sink with the given members. Returns
    /// the driver with module_id populated, or an error if pactl
    /// load-module fails (typically because one of the slave sinks
    /// doesn't exist — the BT speakers must already be paired+connected).
    ///
    /// We use module-combine-sink rather than module-loopback for
    /// each speaker because combine-sink handles clock sync via
    /// resampling per-slave; loopback would let speakers drift.
    pub async fn create(members: Vec<(String, String)>)
        -> Result<MultiRoomDriver, DriverError>
    {
        if members.len() < 2 {
            return Err(DriverError::State(
                format!("multi-room needs ≥2 speakers, got {}", members.len())
            ));
        }
        let sink_name = Self::sink_name_for(&members);
        // Build slave list — PA sink names for each BT MAC.
        let slaves: Vec<String> = members.iter()
            .map(|(m, _)| {
                let mac_under = m.replace(':', "_").to_uppercase();
                format!("bluez_sink.{}.a2dp_sink", mac_under)
            })
            .collect();
        let slaves_csv = slaves.join(",");
        eprintln!("multiroom: load-module sink_name={} slaves={}",
                  sink_name, slaves_csv);

        let out = Command::new("pactl")
            .args(["load-module", "module-combine-sink",
                   &format!("sink_name={}", sink_name),
                   &format!("slaves={}", slaves_csv),
                   // Sync mode: resample to master clock so the slaves
                   // stay phase-aligned. The "remix" flag re-channels
                   // the stream to match each slave's channel map.
                   "channels=2",
                   "remix=yes"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| DriverError::Subprocess(
                format!("pactl spawn: {}", e)
            ))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(DriverError::Subprocess(format!(
                "pactl load-module failed: {} \
                 (are all the BT speakers paired AND connected? \
                 Check via `pactl list short sinks`)",
                stderr.trim()
            )));
        }
        // pactl load-module prints the module ID to stdout on success.
        let id_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let module_id: u32 = id_str.parse()
            .map_err(|_| DriverError::Subprocess(format!(
                "pactl returned non-numeric module ID: {:?}", id_str
            )))?;
        eprintln!("multiroom: combine-sink created module_id={} sink={}",
                  module_id, sink_name);

        Ok(MultiRoomDriver {
            sink_name,
            members,
            combine_module_id: Arc::new(Mutex::new(Some(module_id))),
            playing_pid: Arc::new(Mutex::new(None)),
        })
    }

    /// Unload the combine-sink module. Idempotent: subsequent calls
    /// no-op since combine_module_id is cleared on first run. Always
    /// stops any active playback first so we don't leak paplay PIDs
    /// pointing at a sink that no longer exists.
    pub async fn teardown(&self) -> Result<(), DriverError> {
        let _ = self.stop().await;
        let id_opt = {
            let mut g = self.combine_module_id.lock().unwrap();
            g.take()
        };
        if let Some(id) = id_opt {
            eprintln!("multiroom: unload-module {}", id);
            let out = Command::new("pactl")
                .args(["unload-module", &id.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| DriverError::Subprocess(
                    format!("pactl spawn: {}", e)
                ))?;
            if !out.status.success() {
                // Already unloaded? PA-restarted? Both are harmless;
                // log + continue.
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("multiroom: unload returned {}: {}",
                          out.status, stderr.trim());
            }
        }
        Ok(())
    }

    pub async fn play_local_file(
        &self,
        path: &Path,
        _title: &str,
        start_offset_sec: u32,
        on_end: Option<Box<dyn FnOnce(EndReason) + Send + 'static>>,
    ) -> Result<(), DriverError>
    {
        let _ = self.stop().await;
        let path_str = path.to_string_lossy().to_string();
        let needs_decode = !is_paplay_native_format(path);
        eprintln!("multiroom: play {} → sink={} ({} speakers, needs_decode={})",
                  path_str, self.sink_name, self.members.len(), needs_decode);
        // The existing BT spawn helpers already accept an arbitrary
        // sink name — we just pass our combine-sink name.
        let pid_slot = self.playing_pid.clone();
        let pid = if needs_decode {
            spawn_ffmpeg_into_paplay(&path_str, &self.sink_name, start_offset_sec, pid_slot.clone(), on_end).await?
        } else {
            spawn_paplay_direct(&path_str, &self.sink_name, pid_slot.clone(), on_end).await?
        };
        *self.playing_pid.lock().unwrap() = Some(pid);
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), DriverError> {
        let pid_opt = { *self.playing_pid.lock().unwrap() };
        if let Some(pid) = pid_opt {
            let r = Command::new("kill")
                .arg(pid.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            match r {
                Ok(s) if s.success() => {
                    *self.playing_pid.lock().unwrap() = None;
                    Ok(())
                }
                Ok(s) => Err(DriverError::Subprocess(
                    format!("kill {} exit code {:?}", pid, s.code())
                )),
                Err(e) => Err(DriverError::Subprocess(
                    format!("kill {} spawn failed: {}", pid, e)
                )),
            }
        } else {
            Ok(())
        }
    }

    pub async fn get_volume_percent(&self) -> Result<u8, DriverError> {
        let out = Command::new("pactl")
            .args(["get-sink-volume", &self.sink_name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| DriverError::Subprocess(
                format!("pactl spawn: {}", e)
            ))?;
        if !out.status.success() {
            return Err(DriverError::Subprocess(format!(
                "pactl get-sink-volume {} failed", self.sink_name
            )));
        }
        parse_pactl_volume(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| DriverError::Subprocess(
                "couldn't parse pactl output".to_string()
            ))
    }

    pub async fn set_volume_percent(&self, pct: u8) -> Result<(), DriverError> {
        let pct = pct.min(100);
        let out = Command::new("pactl")
            .args(["set-sink-volume", &self.sink_name,
                   &format!("{}%", pct)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| DriverError::Subprocess(
                format!("pactl spawn: {}", e)
            ))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(DriverError::Subprocess(
                "pactl set-sink-volume failed".to_string()
            ))
        }
    }
}

/// Parse pactl's `get-sink-volume` output to extract a 0-100 percent.
/// pactl format examples:
///   "Volume: front-left: 65536 / 100% / 0.00 dB, front-right: 65536 / 100% / 0.00 dB"
///   "Volume: mono: 32768 / 50% / -6.00 dB"
///
/// We take the first %-suffixed token. Returns None if the format
/// doesn't match (alien locale, future pactl version, etc).
#[allow(dead_code)]  // Used by tests + the get-volume path which is
                     // not yet on the hot wire (callers still go through
                     // Command::GetVolume for Mozart).
pub fn parse_pactl_volume(text: &str) -> Option<u8> {
    for line in text.lines() {
        if !line.trim_start().starts_with("Volume:") { continue; }
        // Find first integer followed by % sign.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            if i < bytes.len() && bytes[i] == b'%' {
                if let Ok(p) = std::str::from_utf8(&bytes[start..i]) {
                    if let Ok(n) = p.parse::<u32>() {
                        return Some(n.min(100) as u8);
                    }
                }
            }
        }
    }
    None
}

/// Query the audio server for the list of sink names available right
/// now. Returns sink NAMES (first whitespace-separated token per line
/// of `pactl list short sinks`).
///
/// This is the live truth about what the audio graph can play to —
/// used as a pre-flight check in BluetoothDriver::play_local_file so
/// we don't fire ffmpeg + paplay at a sink that doesn't exist (the
/// most common cause of "paplay completes successfully but no sound
/// reaches the speaker": the bluez sink isn't there because the BT
/// device isn't actually connected this session, only remembered).
///
/// Empty Vec on any error — caller should treat that as "audio
/// server not running" rather than "no sinks." pactl is usually
/// available; absence is unusual.
fn list_pactl_sinks() -> Vec<String> {
    // Use blocking std::process::Command here because we're called
    // from an async fn but only briefly (~10 ms typical); reaching
    // for tokio::process for this would force the caller into more
    // complexity than warranted. The pre-flight runs once per play,
    // not per audio frame.
    let out = match std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter_map(|line| {
            // `pactl list short sinks` rows are TAB-separated:
            //   <id>\t<name>\t<driver>\t<format>\t<state>
            // We want column 1 (name).
            let mut cols = line.split('\t');
            let _id = cols.next()?;
            let name = cols.next()?.trim();
            if name.is_empty() { None } else { Some(name.to_string()) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pactl_volume_standard() {
        let s = "Volume: front-left: 65536 /  100% / 0.00 dB,\n\
                 front-right: 65536 / 100% / 0.00 dB";
        assert_eq!(parse_pactl_volume(s), Some(100));
    }

    #[test]
    fn parse_pactl_volume_mono_half() {
        let s = "Volume: mono: 32768 / 50% / -6.00 dB";
        assert_eq!(parse_pactl_volume(s), Some(50));
    }

    #[test]
    fn parse_pactl_volume_clamps_over_100() {
        // Some PA versions go to 150% (boosted). We clamp to 100
        // since that's what the slider model expects.
        let s = "Volume: mono: 98304 / 150% / 3.52 dB";
        assert_eq!(parse_pactl_volume(s), Some(100));
    }

    #[test]
    fn parse_pactl_volume_returns_none_for_garbage() {
        assert_eq!(parse_pactl_volume(""), None);
        assert_eq!(parse_pactl_volume("not pactl output"), None);
        assert_eq!(parse_pactl_volume("Some other line\n"), None);
    }

    #[test]
    fn bluetooth_sink_name_format() {
        let d = BluetoothDriver {
            mac: "00:09:A7:77:C2:88".to_string(),
            name: "Beosound Emerge".to_string(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        };
        assert_eq!(d.sink_name(), "bluez_sink.00_09_A7_77_C2_88.a2dp_sink");
        assert_eq!(d.sink_name_pipewire(), "bluez_output.00_09_A7_77_C2_88.1");
    }

    #[test]
    fn volume_targets_active_sink_first() {
        // When play fell back to a sink that differs from the canonical
        // names, volume must try that active sink FIRST — otherwise it
        // targets a non-playing/absent speaker and fails on both
        // preferred names (the reported set-sink-volume bug).
        let d = BluetoothDriver {
            mac: "50:1E:2D:5B:E6:EF".to_string(),
            name: "Smokey".to_string(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(Some(
                "bluez_sink.00_09_A7_77_CD_55.a2dp_sink".to_string()))),
        };
        let c = d.volume_sink_candidates();
        // active sink leads
        assert_eq!(c[0], "bluez_sink.00_09_A7_77_CD_55.a2dp_sink");
        // canonical names follow as fallbacks
        assert!(c.contains(&d.sink_name()));
        assert!(c.contains(&d.sink_name_pipewire()));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn volume_candidates_dedup_when_active_equals_canonical() {
        // If the active sink IS the canonical PA name, it must not be
        // listed twice.
        let d = BluetoothDriver {
            mac: "50:1E:2D:5B:E6:EF".to_string(),
            name: "Smokey".to_string(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        };
        let canonical = d.sink_name();
        *d.active_sink.lock().unwrap() = Some(canonical.clone());
        let c = d.volume_sink_candidates();
        assert_eq!(c[0], canonical);
        assert_eq!(c.iter().filter(|s| **s == canonical).count(), 1,
            "canonical sink should appear exactly once");
        assert_eq!(c.len(), 2); // canonical (==active) + pipewire
    }

    #[test]
    fn volume_candidates_no_active_sink_uses_canonical_only() {
        // Before any play, active_sink is None — fall back to the two
        // canonical names, in order.
        let d = BluetoothDriver {
            mac: "50:1E:2D:5B:E6:EF".to_string(),
            name: "Smokey".to_string(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        };
        let c = d.volume_sink_candidates();
        assert_eq!(c, vec![d.sink_name(), d.sink_name_pipewire()]);
    }

    #[test]
    fn bluetooth_sink_name_lowercase_mac_normalized_to_upper() {
        let d = BluetoothDriver {
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            name: String::new(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        };
        // PA's BT sink names use uppercase hex.
        assert_eq!(d.sink_name(), "bluez_sink.AA_BB_CC_DD_EE_FF.a2dp_sink");
    }

    #[test]
    fn any_driver_picks_mozart_for_mozart_family() {
        let client = reqwest::Client::new();
        let d = AnyDriver::for_family(
            FirmwareFamily::Mozart,
            "http://192.168.1.10",
            None, None,
            client,
        );
        match d {
            AnyDriver::Mozart(m) => assert_eq!(m.base_url, "http://192.168.1.10"),
            _ => panic!("expected Mozart driver"),
        }
    }

    #[test]
    fn any_driver_picks_bluetooth_when_non_mozart_and_mac_available() {
        let client = reqwest::Client::new();
        let d = AnyDriver::for_family(
            FirmwareFamily::BeolinkConverse,
            "http://10.0.0.1",
            Some("AA:BB:CC:DD:EE:FF"),
            Some("My Speaker"),
            client,
        );
        match d {
            AnyDriver::Bluetooth(b) => {
                assert_eq!(b.mac, "AA:BB:CC:DD:EE:FF");
                assert_eq!(b.name, "My Speaker");
            }
            _ => panic!("expected Bluetooth driver"),
        }
    }

    #[test]
    fn any_driver_picks_local_when_non_mozart_and_no_mac() {
        // Updated behavior: instead of falling to Noop, we now route
        // through the controller's own speakers via LocalDriver. This
        // means library/playlist playback works out-of-the-box even
        // before the user has paired a BT speaker.
        let client = reqwest::Client::new();
        let d = AnyDriver::for_family(
            FirmwareFamily::Unknown,
            "http://10.0.0.1",
            None, None,
            client,
        );
        match d {
            AnyDriver::Local(_) => {}
            _ => panic!("expected Local driver as fallback when no BT MAC, got {:?}", d),
        }
    }

    #[test]
    fn any_driver_picks_noop_for_unreachable() {
        // Unreachable still produces Noop — we can't even verify the
        // device is there, so playing on local speakers wouldn't help.
        let client = reqwest::Client::new();
        let d = AnyDriver::for_family(
            FirmwareFamily::Unreachable,
            "http://10.0.0.1",
            Some("AA:BB:CC:DD:EE:FF"),
            None,
            client,
        );
        match d {
            AnyDriver::Noop(_) => {},
            _ => panic!("Unreachable should be Noop"),
        }
    }

    #[test]
    fn driver_label_mentions_concrete_details() {
        let mozart = AnyDriver::Mozart(MozartDriver {
            base_url: "http://192.168.1.10".to_string(),
            client: reqwest::Client::new(),
        });
        assert!(mozart.label().contains("192.168.1.10"));

        let bt = AnyDriver::Bluetooth(BluetoothDriver {
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            name: "Speaker".to_string(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        });
        let lbl = bt.label();
        assert!(lbl.contains("AA:BB:CC:DD:EE:FF"));
        assert!(lbl.contains("Speaker"));

        let noop = AnyDriver::Noop(NoopDriver { reason: "test".into() });
        assert!(noop.label().contains("test"));
    }

    #[tokio::test]
    async fn bluetooth_play_uri_via_any_returns_unsupported_message() {
        // play_local_file on Mozart driver is the inverse case (it
        // returns Unsupported because Mozart expects a URI not a
        // path). Make sure the error message is informative.
        let m = AnyDriver::Mozart(MozartDriver {
            base_url: "http://example".to_string(),
            client: reqwest::Client::new(),
        });
        let r = m.play_local_file(Path::new("/tmp/x.opus"), "test", 0, None).await;
        match r {
            Err(DriverError::Unsupported(s)) => {
                assert!(s.contains("Mozart"), "msg: {}", s);
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn noop_driver_returns_reason_on_play() {
        let n = AnyDriver::Noop(NoopDriver {
            reason: "device gone".into()
        });
        match n.play_local_file(Path::new("/x"), "y", 0, None).await {
            Err(DriverError::Unsupported(s)) => assert_eq!(s, "device gone"),
            other => panic!("expected Unsupported(device gone), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn stop_on_idle_bluetooth_is_idempotent() {
        // No PID stored → stop() should return Ok without spawning
        // anything.
        let d = BluetoothDriver {
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            name: String::new(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        };
        let r = d.stop().await;
        assert!(r.is_ok(), "idle stop should succeed, got {:?}", r);
    }

    #[tokio::test]
    async fn stop_on_idle_local_is_idempotent() {
        let d = LocalDriver {
            playing_pid: Arc::new(Mutex::new(None)),
        };
        let r = d.stop().await;
        assert!(r.is_ok(), "idle local stop should succeed, got {:?}", r);
    }

    #[test]
    fn local_driver_label_is_descriptive() {
        let d = AnyDriver::Local(LocalDriver {
            playing_pid: Arc::new(Mutex::new(None)),
        });
        let lbl = d.label();
        assert!(lbl.to_lowercase().contains("local") ||
                lbl.to_lowercase().contains("controller"),
                "label should mention local/controller, got: {}", lbl);
    }

    #[test]
    fn multiroom_sink_name_stable_across_member_order() {
        // Hashing should be order-independent so reconnecting with the
        // same speakers in a different order reuses the same sink name.
        let a = MultiRoomDriver::sink_name_for(&[
            ("AA:BB:CC:DD:EE:FF".into(), "First".into()),
            ("11:22:33:44:55:66".into(), "Second".into()),
        ]);
        let b = MultiRoomDriver::sink_name_for(&[
            ("11:22:33:44:55:66".into(), "Second".into()),
            ("AA:BB:CC:DD:EE:FF".into(), "First".into()),
        ]);
        assert_eq!(a, b, "sink name should be order-independent");
    }

    #[test]
    fn multiroom_sink_name_differs_for_different_speakers() {
        let a = MultiRoomDriver::sink_name_for(&[
            ("AA:BB:CC:DD:EE:FF".into(), "x".into()),
            ("11:22:33:44:55:66".into(), "y".into()),
        ]);
        let b = MultiRoomDriver::sink_name_for(&[
            ("AA:BB:CC:DD:EE:FF".into(), "x".into()),
            ("77:88:99:AA:BB:CC".into(), "z".into()),
        ]);
        assert_ne!(a, b);
    }

    #[test]
    fn multiroom_sink_name_format() {
        let n = MultiRoomDriver::sink_name_for(&[
            ("AA:BB:CC:DD:EE:FF".into(), "x".into()),
            ("11:22:33:44:55:66".into(), "y".into()),
        ]);
        assert!(n.starts_with("bo_multi_"));
        // Hex suffix should be 16 chars (u64 in hex).
        assert_eq!(n.len(), "bo_multi_".len() + 16);
        // Each char of the suffix is a hex digit.
        for c in n["bo_multi_".len()..].chars() {
            assert!(c.is_ascii_hexdigit(), "non-hex char in sink name: {}", c);
        }
    }

    #[tokio::test]
    async fn multiroom_create_rejects_lone_speaker() {
        // Need at least 2 speakers; a single speaker doesn't need a
        // combine sink, should use BluetoothDriver directly.
        let r = MultiRoomDriver::create(vec![
            ("AA:BB:CC:DD:EE:FF".into(), "Solo".into())
        ]).await;
        match r {
            Err(DriverError::State(msg)) => {
                assert!(msg.contains("2"), "msg should mention ≥2, got: {}", msg);
            }
            other => panic!("expected State error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn multiroom_teardown_is_idempotent() {
        // Build a driver with a None module_id (as if teardown
        // already ran) and confirm a second teardown doesn't error.
        let d = MultiRoomDriver {
            sink_name: "bo_multi_test".into(),
            members: vec![
                ("AA:BB:CC:DD:EE:FF".into(), "x".into()),
                ("11:22:33:44:55:66".into(), "y".into()),
            ],
            combine_module_id: Arc::new(Mutex::new(None)),
            playing_pid: Arc::new(Mutex::new(None)),
        };
        let r = d.teardown().await;
        assert!(r.is_ok(), "teardown on cleared driver should succeed, got {:?}", r);
    }

    #[test]
    fn paplay_native_formats_recognized() {
        // The trio paplay can read directly.
        assert!(is_paplay_native_format(Path::new("file.wav")));
        assert!(is_paplay_native_format(Path::new("file.WAV")));
        assert!(is_paplay_native_format(Path::new("/tmp/song.au")));
        assert!(is_paplay_native_format(Path::new("snd.SND")));
    }

    #[test]
    fn is_playing_false_for_drivers_without_pid() {
        // Fresh drivers have no child PID — is_playing should be false.
        let local = AnyDriver::Local(LocalDriver {
            playing_pid: Arc::new(Mutex::new(None)),
        });
        assert!(!local.is_playing());

        let bt = AnyDriver::Bluetooth(BluetoothDriver {
            mac: "AA:BB:CC:DD:EE:FF".into(),
            name: "X".into(),
            playing_pid: Arc::new(Mutex::new(None)),
            active_sink: Arc::new(Mutex::new(None)),
        });
        assert!(!bt.is_playing());

        let mr = AnyDriver::MultiRoom(MultiRoomDriver {
            sink_name: "bo_multi_test".into(),
            members: vec![
                ("AA:BB:CC:DD:EE:FF".into(), "a".into()),
                ("11:22:33:44:55:66".into(), "b".into()),
            ],
            combine_module_id: Arc::new(Mutex::new(None)),
            playing_pid: Arc::new(Mutex::new(None)),
        });
        assert!(!mr.is_playing());

        // Mozart + Noop always report false (don't track local PIDs).
        let noop = AnyDriver::Noop(NoopDriver { reason: "test".into() });
        assert!(!noop.is_playing());
    }

    #[test]
    fn is_playing_true_when_pid_present() {
        // Simulate an in-flight paplay by populating the pid slot.
        let pid_slot = Arc::new(Mutex::new(Some(12345u32)));
        let bt = AnyDriver::Bluetooth(BluetoothDriver {
            mac: "AA:BB:CC:DD:EE:FF".into(),
            name: "X".into(),
            playing_pid: pid_slot,
        });
        assert!(bt.is_playing());
    }

    #[test]
    fn paplay_needs_decode_for_compressed_formats() {
        // Everything else routes through ffmpeg.
        assert!(!is_paplay_native_format(Path::new("song.opus")));
        assert!(!is_paplay_native_format(Path::new("song.mp3")));
        assert!(!is_paplay_native_format(Path::new("song.flac")));
        assert!(!is_paplay_native_format(Path::new("song.m4a")));
        assert!(!is_paplay_native_format(Path::new("song.ogg")));
        assert!(!is_paplay_native_format(Path::new("/tmp/bo-mix-1234.opus")));
    }

    #[test]
    fn paplay_needs_decode_for_no_extension() {
        // No extension → safest to decode; ffmpeg will figure out
        // the format from content.
        assert!(!is_paplay_native_format(Path::new("song")));
        assert!(!is_paplay_native_format(Path::new("/tmp/noext")));
    }

    // Marker: a separate file path is needed for tests that spawn
    // paplay; we don't include those here to avoid requiring paplay
    // + a working BT stack in CI. The non-spawning unit tests above
    // cover construction, name derivation, parsing, and error
    // surface — the parts that matter for confidence.
    #[allow(dead_code)]
    fn _untested_paths_noted() {}
}
