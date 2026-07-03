// src/main.rs
//
// Patched version: both products.json and qml/ now have:
//   - multi-location runtime search (env var → cwd → exe-relative → installed paths)
//   - compile-time-embedded fallback via include_str! / include_dir!
//   - non-fatal degradation on missing/malformed files
//
// What changed from the original main.rs:
//   1. New imports for `include_dir` and `Path`
//   2. New `find_products_file`, `find_qml_dir`, `extract_embedded_qml`
//   3. `set_products_from_file` now takes no path arg and falls back to embedded
//   4. main() resolves the QML directory before calling `engine.load_file`
//   5. A `QmlDirGuard` cleans up any temp dir we extracted to
//
// Everything else (QtState struct, background worker, command/result enums,
// Mozart endpoints) is unchanged.

use qmetaobject::*;
use reqwest::Client;
use serde_json::Value;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::collections::HashSet;

/// Global SSDP discovery cache. Populated at startup by a background
/// M-SEARCH thread; read by play_uri's DLNA strategy so each play
/// attempt doesn't pay the 1.5-second discovery latency. Empty
/// initially (we don't block startup waiting for the search).
static SSDP_CACHE: once_cell::sync::Lazy<RwLock<Vec<dlna_discover::DiscoveredDevice>>>
    = once_cell::sync::Lazy::new(|| RwLock::new(Vec::new()));
use std::thread;
use std::time::Duration;
use std::process::Command as ProcessCommand;
use std::path::{Path, PathBuf};

// The media library subsystem (SQLite catalog + scanner + worker thread).
mod media;
mod mux_bridge;
mod noise_adaptive_gain;
mod playback_modes;
mod queue_policy;
mod layer_player;
mod lyrics_transcriber;
mod spectral_masker;
mod volume_safety;
mod volume_safety_ack;
mod masker;
mod cancel_tones;
mod pattern_learner;
mod mic_capture;
mod predictor;
mod pattern_runner;
mod sanitizer_driver;
mod anc_engine;
mod anc_runner;
mod audio_capture_dump;
mod audio_features;
mod backup_beeper;
mod drone_canceller;
mod frequency_shifter;
mod music_masker;
mod network_sim;
mod passby_canceller;
mod reverb;
mod settings_store;
mod v1;
mod room_geometry;
mod speaker_calibration;
mod track_server;
mod play_uri;
mod device_priority;
mod anticipator;
mod shadow_stream;
mod pre_buffer;
mod volume_gesture;
mod aggression;
mod state_epoch;
mod conn_warmer;
mod rtt_tracker;
mod circuit_breaker;
mod device_ping;
mod bt_autoconnect;
mod bt_respect;
mod wake_on_lan;
mod device_notify;
mod dns_prewarm;
mod speculative_play;
mod speculation_accuracy;
mod rt_sched;
mod dep_wrangler;
mod dep_wrangler_run;
mod dlna_discover;
mod dlna_position;
mod mozart_source;
mod mozart_source_switch;
mod firmware_family;
mod device_driver;
mod sim;
#[allow(dead_code)]   // consumed by build.rs via include!, not by main
mod qml_lint;
mod looper;
mod realtime_mixer;
mod kde_tools;
mod mpris_listener;
mod stem_split;
mod stem_stage;
mod podcast_diar;
mod playback_isolator;
mod stream_separator;
mod channel_routing;

// Embedded fallbacks — baked into the binary at compile time.
// Build fails loud-and-early if these paths don't exist.
const EMBEDDED_PRODUCTS_JSON: &str = include_str!("../assets/products.json");
const EMBEDDED_QML: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/qml");
/// Static product silhouettes + SVGs + any other read-only assets
/// QML refers to. Bundled at build time alongside EMBEDDED_QML so
/// the binary can render without an external install. Extracted as
/// a sibling of the QML temp dir so `Qt.resolvedUrl("../assets/...")`
/// from inside a QML file resolves correctly.
const EMBEDDED_ASSETS: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets");

// QML entry file inside the qml/ directory.
const QML_ENTRY_FILE: &str = "volume_knobs.qml";

/// Commands sent from GUI -> background Tokio thread.
/// GetMuteState/SetMuteState variants are vestigial: the device's
/// /BeoSound/muted endpoint isn't whitelisted, so the mute path now
/// volume→0-with-restore via SetVolume. Kept as a fallback in case
/// firmware exposes mute control in the future.
#[allow(dead_code)]
#[derive(Clone)]
enum Command {
    GetVolume { url: String },
    SetVolume { url: String, value: i32 },
    GetMuteState { url: String },
    SetMuteState { url: String, muted: bool },
    GetProductInfo { url: String },
    PlayMozartUri { url: String, stream_uri: String },
    /// Multi-endpoint best-effort play. Tries several ASE/BeoZone endpoint
    /// shapes (loadUri, PlayQueue, playPointer) until one returns 2xx.
    /// Used by the library "tap to play" flow. Reports the outcome back
    /// via ResultMessage::PlayOutcome.
    PlayResolvedUrl { url: String, track_url: String, title: String },
    /// Speculative pre-load: POST a paused loadUri so a real tap only
    /// needs a Transport=play. Best-effort; failures are not surfaced
    /// to QML, since the fallback (full play_uri on tap) still works.
    /// `track_id` is the catalog ID so the speculative stager can
    /// match it on promotion.
    SpeculativeLoadUri { url: String, track_url: String, title: String, track_id: i64 },
    /// Probe live device state: battery, network interface, BT availability.
    /// Used by the priority-group system to pick the best hub. Returns
    /// ResultMessage::DeviceStateProbed regardless of which endpoints
    /// responded (missing fields stay None).
    ProbeDeviceCapabilities { url: String },
    /// Ask the device what its current active source is. The worker
    /// runs the mozart_source probe and reports back as
    /// ResultMessage::ActiveSourceProbed.
    ProbeActiveSource { url: String },
    /// Try to programmatically activate the Network/DLNA source on
    /// the given device. Drives mozart_source_switch and reports
    /// back via ResultMessage::NetworkSourceSwitched.
    TryActivateNetwork { url: String },
    /// One-shot probe of which firmware family a device speaks.
    /// Result via ResultMessage::FirmwareProbed.
    ProbeFirmware { url: String },
    /// Run an async driver operation on the worker's tokio runtime.
    /// The variant carries the snapshot of the current AnyDriver
    /// (cheap to clone — driver owns small state) + which method
    /// to invoke. Result lands in ResultMessage::DriverResult.
    DriverOp {
        driver: device_driver::AnyDriver,
        op:     DriverOpKind,
    },
    /// Async-create a multi-room combine-sink with the given members.
    /// On success the result is the freshly-built MultiRoomDriver,
    /// delivered via ResultMessage::MultiRoomCreated for QtState to
    /// swap in as current_driver. On failure the error message is
    /// surfaced in the same result.
    CreateMultiRoom { members: Vec<(String, String)> },
    /// Async teardown for a multi-room driver — unloads the PA
    /// combine-sink module so we don't leak virtual sinks on PA.
    /// Idempotent.
    TeardownMultiRoom { driver: device_driver::AnyDriver },
    /// Read the current driver's volume via pactl. Used by the
    /// periodic refresh so the slider stays in sync with KDE-side
    /// volume changes. Result via ResultMessage::DriverVolume.
    DriverGetVolume { driver: device_driver::AnyDriver },
    /// Subscribe to push notifications from the device — long-poll a
    /// candidate endpoint and emit Volume / NowPlaying / Standby
    /// results inline as if they came from a fresh GetVolume.
    /// One-shot from the controller's perspective: the worker spawns
    /// a long-running task that survives until the channel closes.
    SubscribeNotifications { url: String },
    StopPlayback { url: String },
    CheckKdeMedia,

    // ── Added features ──────────────────────────────────────────────
    /// List the device's available input sources.
    GetSources { url: String },
    /// Switch the active source by its id.
    SetSource { url: String, source_id: String },
    /// Read now-playing metadata (title/artist/source).
    GetNowPlaying { url: String },
    /// Send a transport command: "play" | "pause" | "stop" | "forward" | "back".
    Transport { url: String, action: String },
    /// Read standby/power state.
    GetStandby { url: String },
    /// Set standby on/off (true = go to standby).
    SetStandby { url: String, standby: bool },
    /// List the device's sound modes/presets (Mozart).
    GetSoundModes { url: String },
    /// Activate a sound mode by id (Mozart).
    SetSoundMode { url: String, mode_id: String },
}

/// Cross-thread message from a spawned bluetoothctl connect attempt
/// back to the QtState scheduler. Carried on its own channel so it
/// doesn't pollute the network-result enum.
#[derive(Debug, Clone)]
pub struct BtAttemptResult {
    pub mac: String,
    pub success: bool,
}

/// Results sent from background -> GUI
enum ResultMessage {
    Volume(i32),
    MuteState(bool),
    ProductInfo { name: String, type_name: String },
    KdeMediaPlaying(bool),
    Error(String),

    // ── Added features ──────────────────────────────────────────────
    /// JSON array string of {id, name, type} sources, plus the active id.
    Sources { json: String, active_id: String },
    /// Now-playing metadata.
    NowPlaying { title: String, artist: String, source: String, playing: bool },
    /// Standby state (true = in standby).
    Standby(bool),
    /// JSON array of {id, name} sound modes + the active id.
    SoundModes { json: String, active_id: String },
    /// A device found via mDNS discovery: friendly name + base url.
    DeviceDiscovered { name: String, url: String },
    /// Outcome of a play_track attempt: which endpoint succeeded (or
    /// empty when none), plus a human-readable summary of what was tried.
    PlayOutcome { used_endpoint: String, summary: String, url: String },
    /// Sent once when a device's notify-subscription task gives up
    /// after MAX_DISCOVERY_CYCLES of fruitless probing. QtState
    /// remembers this per-URL in app_kv so subsequent app launches
    /// skip the discovery cycles entirely — saves ~7 seconds of
    /// startup time on every Mozart-legacy device.
    NotifyUnsupported { url: String },
    /// Result of a ProbeDeviceCapabilities call. All fields are Option so
    /// the priority scorer can distinguish "we know it's false" from
    /// "we didn't get to ask". Reachable is always populated.
    DeviceStateProbed {
        url: String,
        reachable: bool,
        on_battery: Option<bool>,
        battery_percent: Option<u8>,
        charging: Option<bool>,
        on_ethernet: Option<bool>,
        on_wifi: Option<bool>,
        bluetooth_active: Option<bool>,
    },
    /// Result of a ProbeActiveSource call. When no candidate path
    /// returned a parseable response, `id` and `label` are empty and
    /// `family` is "unknown" — the UI shows the chip greyed out.
    ActiveSourceProbed {
        url: String,
        id: String,
        label: String,
        family: String,             // serialized SourceFamily
        can_play: bool,
        block_reason: String,
    },
    /// Result of a TryActivateNetwork attempt. `success` reports
    /// whether at least one endpoint accepted the activation;
    /// `json` is the full SwitchOutcome (used_endpoint, attempts,
    /// network_source_id) so the UI can show what was tried.
    NetworkSourceSwitched {
        url: String,
        success: bool,
        json: String,
    },
    /// Result of a ProbeFirmware call. `family` is the serialized
    /// FirmwareFamily tag ("mozart", "beolink", "mozart_legacy",
    /// "unknown", "unreachable"); `json` is the full FirmwareProbe
    /// blob with reasoning + body snippet for the diagnostic UI.
    FirmwareProbed {
        url: String,
        family: String,
        json: String,
    },
    /// Outcome of an async driver operation (play_local_file, stop,
    /// set_volume_percent). Empty string = success; non-empty =
    /// human-readable error message. The QtState handler routes this
    /// into `driver_last_error` so the UI can show what went wrong.
    DriverResult(String),
    /// A driver-played track finished naturally (paplay exited 0).
    /// Drives queue auto-advance: when this fires, QtState looks at
    /// the active queue and plays the next track if there is one.
    /// Stopped/errored ends don't produce this — only natural ends.
    TrackEnded,
    /// Result of an async CreateMultiRoom command. On success the
    /// `driver` field carries the new MultiRoomDriver to install as
    /// current_driver; on failure `error` is the human-readable
    /// reason.
    MultiRoomCreated {
        driver: Option<device_driver::AnyDriver>,
        error:  String,
    },
    /// Result of a DriverGetVolume command. `pct` is 0-100, or None
    /// if the driver couldn't read its sink (e.g. sink was removed
    /// since the command issued). QtState updates self.volume if
    /// the value differs significantly.
    DriverVolume { pct: Option<u8> },
}

/// Kinds of driver operations the worker can dispatch. Kept separate
/// from `Command` because all variants share the same `driver` payload.
#[derive(Debug, Clone)]
enum DriverOpKind {
    /// Play a local file at the given path, with optional title for
    /// the device's display.
    PlayLocalFile { path: String, title: String, start_offset_sec: u32 },
    /// Stop whatever's currently playing (idempotent).
    Stop,
    /// Set volume 0-100 percent. Currently only meaningful for
    /// Bluetooth driver — Mozart goes through the existing volume
    /// safety pipeline instead.
    SetVolumePct(u8),
}


// QML/Qt signal-naming convention is camelCase (e.g. `productsChanged`),
// which conflicts with rust's default snake_case lint. Allow at struct
// level so qt_signal!()-generated identifiers don't trip it.
#[allow(non_snake_case)]
#[derive(QObject, Default)]
struct QtState {
    base: qt_base_class!(trait QObject),

    // exposed to QML (camelCase)
    active_url: qt_property!(String; NOTIFY url_changed),
    /// Product type of the active device (e.g. "BeosoundStage"). Drives the
    /// per-product volume-safety ceiling. Empty when unknown — in which
    /// case the conservative default ceiling is used.
    active_product_id: qt_property!(String; NOTIFY url_changed),
    /// Approximate dB SPL at the current volume for the active product.
    /// Recomputed whenever volume or product changes.
    active_db_estimate: qt_property!(f64; NOTIFY url_changed),
    /// Set true momentarily after the safety system clamps the volume on
    /// startup; QML can show a toast like "volume capped at 35 (≈75 dB)".
    volume_was_clamped: qt_property!(bool; NOTIFY volume_clamped_changed),
    volume_clamped_changed: qt_signal!(),
    /// Have we already done the startup safety check on this device?
    /// (Reset whenever active_url changes.)
    safety_check_done: bool,
    /// Rate limiter for set_volume_api. Trips on rapid-fire volume
    /// changes (legitimate user UI doesn't normally generate >12
    /// changes/sec; an attacker ramping volume aggressively does).
    /// Not a qt_property: the runtime state lives inside QtState
    /// but the QML side only sees the resulting "play_status" toast
    /// when a lockout fires.
    volume_rate_limiter: volume_safety::VolumeRateLimiter,
    /// Timestamps (ms) of recent BT MAC association changes. The
    /// `set_active_bt_mac` path uses this to detect a flood of
    /// switches (likely from spoofed mDNS announcements) and
    /// briefly lock out further changes. Legitimate user device
    /// switches are well below the budget; an automated attack
    /// trips it.
    bt_switch_history: Vec<u64>,
    /// Unix-ms instant past which BT switch lockout ends. Zero when
    /// no lockout is active.
    bt_switch_lockout_until_ms: u64,
    /// Running sanitizer driver, if active. Dropped to stop.
    sanitizer_handle: Option<Box<sanitizer_driver::SanitizerDriver>>,
    /// Running pattern runner (mic capture + learner), if adaptive mode is on.
    pattern_runner: Option<Box<pattern_runner::PatternRunner>>,
    /// ANC engine + runner. Lazily constructed on first `anc_enable`
    /// call; dropped when the user disables ANC fully.
    anc_runner: Option<Box<anc_runner::AncRunner>>,
    /// Stateful adapter turning the canceller's loudness observation
    /// into a soundscape master-gain multiplier. Updated each tick
    /// of anc_standby_refresh; when its emit-threshold fires we
    /// publish to `anc_scene_gain` and QML relays to bo-mux.
    noise_gain: noise_adaptive_gain::NoiseAdaptiveGain,
    /// Sticky enable state — what the user toggled on the QML side.
    /// The runner has its own atomic too; this is the QML-visible
    /// mirror.
    anc_enabled: qt_property!(bool; NOTIFY anc_changed),
    /// Last alignment-probe correlation in [0, 1]. 0 means never
    /// successfully probed; > PROBE_MIN_CORRELATION means trusted.
    anc_alignment_correlation: qt_property!(f64; NOTIFY anc_changed),
    /// Last alignment-probe round-trip delay in milliseconds.
    /// 0 if never measured.
    anc_alignment_ms: qt_property!(f64; NOTIFY anc_changed),
    /// Mic backend in use (`parec`, `arecord`, or empty if not started).
    anc_backend: qt_property!(String; NOTIFY anc_changed),
    /// Anti-noise gain in [0, 1] (default 0.6).
    anc_anti_gain: qt_property!(f64; NOTIFY anc_changed),
    /// Music gain in [0, 2] (default 1.0).
    anc_music_gain: qt_property!(f64; NOTIFY anc_changed),
    /// Blend anti-noise with music (true) vs play alone (false).
    anc_blend_with_music: qt_property!(bool; NOTIFY anc_changed),
    anc_changed: qt_signal!(),
    /// Track HTTP server, lazily bound on the first play_track call so
    /// the controller doesn't bind a port until needed.
    track_server: Option<Arc<track_server::TrackServer>>,
    /// User-action anticipator. Observes play/pause/skip/switch events,
    /// learns transition frequencies, exposes top-N predictions for
    /// pre-staging.
    anticipator: anticipator::Anticipator,
    /// Aggression policy: how forward-leaning the pre-staging is.
    /// Driven by a QML slider. LOW by default (safe defaults); the user
    /// can dial up to HIGH for experimental aggressive pre-staging.
    aggression: aggression::AggressionPolicy,
    /// Current aggression level, exposed for QML binding (0=OFF, 1=LOW,
    /// 2=MEDIUM, 3=HIGH).
    aggression_level: qt_property!(i32; NOTIFY aggression_changed),
    aggression_label: qt_property!(String; NOTIFY aggression_changed),
    aggression_changed: qt_signal!(),
    /// Current TimeWindow label as a string ("MORNING" / "AFTERNOON" /
    /// "EVENING" / "NIGHT"). Updated on every tick. The UI can show this
    /// alongside the anticipation diagnostic so the user understands the
    /// time-of-day context behind a prediction.
    current_time_window: qt_property!(String; NOTIFY time_window_changed),
    time_window_changed: qt_signal!(),
    /// Rolling RTT tracker for observed device round-trips. Cue
    /// scheduling uses `recommend_cue_ms()` to size cues to the
    /// observed network conditions rather than a hardcoded estimate.
    rtt: rtt_tracker::RttTracker,
    /// Per-device RTT trackers. Same tracker per device URL — useful
    /// when the user has multiple devices with very different network
    /// paths (e.g. local Wi-Fi Stage vs cross-mesh Emerge). The active
    /// device's tracker drives cue sizing.
    rtt_per_device: std::collections::HashMap<String, rtt_tracker::RttTracker>,
    /// Cache of pre-synthesized estimate-cue PCM buffers, keyed by the
    /// (bpm × 10 to dedupe similar tempos, tonic_hz × 10, duration_ms,
    /// harmonics) tuple. Cue synthesis is ~5 ms per call; caching makes
    /// the second and subsequent plays of the same parameters
    /// effectively free.
    cue_cache: std::collections::HashMap<(u32, u32, u32, u32), std::sync::Arc<Vec<u8>>>,
    /// Speculative pre-load tracker. When the anticipator's confidence
    /// on PlayTrack is high enough, we POST a paused loadUri to the
    /// device so a real tap only needs to send Transport=play.
    /// Coexists with shadow streams: speculative-pause is preferred
    /// when firmware supports it; shadow is the fallback.
    speculative: speculative_play::SpeculativeStager,
    /// Hit/miss accuracy tracker for the speculative pre-load system.
    /// When the recent window shows poor accuracy, we pause speculation
    /// for a cooldown period to stop wasting device-pipeline cycles.
    /// Hit/miss outcomes are recorded by the play_track and prestage
    /// invalidation paths.
    speculation_accuracy: speculation_accuracy::AccuracyTracker,
    /// Most recent observed RTT in ms; 0 when no samples. Exposed for
    /// the QML diagnostic strip.
    rtt_last_ms: qt_property!(i32; NOTIFY rtt_changed),
    /// p50 (median) RTT in ms over the rolling window. 0 when fewer
    /// than 3 samples seen.
    rtt_p50_ms: qt_property!(i32; NOTIFY rtt_changed),
    /// p95 RTT in ms. Diagnostic. 0 when fewer than 3 samples.
    rtt_p95_ms: qt_property!(i32; NOTIFY rtt_changed),
    rtt_changed: qt_signal!(),
    /// When the most-recent GetVolume was issued. Used to measure the
    /// round-trip latency when its Volume reply arrives.
    last_command_at: Option<std::time::Instant>,
    /// Connection warmer — rate-limited proactive GETs to keep TCP
    /// connections in the pool. Shifts ~50-150 ms of cold-connection
    /// latency off the user's critical path.
    conn_warmer: conn_warmer::ConnWarmer,
    /// Atomic epoch for the device's volume state. Bumped on every
    /// confirmed Volume result and on every SetVolume issued. Managers
    /// that depend on volume (shadow stream's saved-volume restore,
    /// gesture filter's anchor) capture the epoch when they start work
    /// and check it again before committing — if it's moved, their
    /// view is stale.
    volume_epoch: state_epoch::SharedEpoch,
    /// Atomic epoch for the active device. Bumped on every device
    /// switch + failover. Pre-stage operations capture this and can
    /// abort if the user has moved on.
    device_epoch: state_epoch::SharedEpoch,
    /// Adjust the aggression policy. QML slider calls this. Persisted
    /// to the app_state KV so it survives restarts.
    set_aggression: qt_method!(fn(&mut self, level: i32)),
    /// Friction filter between raw dial gestures and SetVolume calls.
    /// Rejects single-event jumps above WRAPAROUND_THRESHOLD (touch
    /// boundary-cross artifacts) and caps per-event step size so a
    /// rapid drag can't surprise-blast the volume. Anchored to the
    /// device's actual volume on every Volume result so it stays in
    /// sync with external changes (physical buttons on the speaker,
    /// other controllers in the cluster).
    volume_gesture: volume_gesture::GestureFilter,
    /// Gesture-driven volume setter. QML's VolumeDial drag handlers
    /// call this instead of set_volume_api so accidental wraparound
    /// jumps are guarded. Direct setters (number pad, keyboard) still
    /// use set_volume_api for full responsiveness.
    set_volume_from_gesture: qt_method!(fn(&mut self, target: i32)),
    /// JSON of current predictions for the UI ("about to ask for next").
    prediction_json: qt_property!(String; NOTIFY prediction_changed),
    prediction_changed: qt_signal!(),
    /// Tick the anticipator from QML (every ~30s) so InactivityTick ages
    /// volatile states back to Idle.
    tick_anticipator: qt_method!(fn(&mut self)),

    /// ── Audio capture dump (for offline CNN training data) ──
    ///
    /// `set_capture_dump_enabled(true)` turns on the ring-buffered
    /// mic-audio recorder; events the FSM detects trigger 40-second
    /// WAV captures. `set_capture_location_tag` attaches a free-form
    /// string ("bedroom_window_north") to subsequent captures so the
    /// training pipeline can split by acoustic environment.
    /// `trigger_manual_capture` is the "I just heard a passby and the
    /// FSM missed it" escape hatch — saves the last 30s of ring buffer
    /// plus the next 10s. `capture_dump_enabled` mirrors the current
    /// state for the UI binding.
    set_capture_dump_enabled: qt_method!(fn(&mut self, on: bool)),
    set_capture_location_tag: qt_method!(fn(&mut self, tag: String)),
    trigger_manual_capture: qt_method!(fn(&mut self)),
    capture_dump_enabled: qt_property!(bool; NOTIFY capture_dump_changed),
    capture_dump_changed: qt_signal!(),
    capture_count: qt_property!(i32; NOTIFY capture_dump_changed),
    refresh_capture_count: qt_method!(fn(&mut self)),

    /// Anti-noise saturation amounts (0.0-0.6). 0 = pure sinusoidal
    /// harmonics; higher = soft-clipped output that matches the
    /// timbre of real engines (asymmetric pressure waveform). The
    /// right value depends on the engine type — gasoline ~0.1, diesel
    /// ~0.2, small 2-stroke (leafblower) ~0.3+.
    /// Stored on the running cancellers (passed through to the
    /// standby thread's instances) and persisted to disk so the
    /// setting survives restarts.
    set_passby_saturation: qt_method!(fn(&mut self, s: f64)),
    set_drone_saturation: qt_method!(fn(&mut self, s: f64)),
    passby_saturation: qt_property!(f64; NOTIFY saturation_changed),
    drone_saturation: qt_property!(f64; NOTIFY saturation_changed),
    saturation_changed: qt_signal!(),
    /// Consecutive command failures on the active device. Trips an auto-
    /// failover when it exceeds the threshold.
    active_failures: u32,
    /// Briefly true after an auto-failover so the UI can show a toast.
    failover_active: qt_property!(bool; NOTIFY failover_changed),
    failover_from: qt_property!(String; NOTIFY failover_changed),
    failover_to: qt_property!(String; NOTIFY failover_changed),
    failover_changed: qt_signal!(),
    dismiss_failover: qt_method!(fn(&mut self)),
    /// Send a transport command (play/pause/forward/back) to every device
    /// at SecondaryHub priority or better. Coordinated pause across rooms.
    transport_all: qt_method!(fn(&mut self, action: String)),
    /// Track IDs we've already pre-resolved this session. Prevents
    /// re-pre-staging the same track over and over.
    prestaged_track_ids: std::collections::HashSet<i64>,
    /// Per-track exposed-URL cache so play_track can use the already-
    /// computed token URL instead of re-exposing on every tap. Maps
    /// `track_id → http://host:port/t/<token>`.
    prestaged_track_urls: std::collections::HashMap<i64, String>,
    /// Last time we ran the moderate-tier pre-stage. Used to throttle.
    last_prestage_at: Option<std::time::Instant>,
    /// The most-recent track_id the user requested for *actual* playback,
    /// so the TrackForPlay handler knows whether to send PlayResolvedUrl
    /// (real tap) or just cache the URL (pre-stage).
    last_user_play_request: Option<i64>,
    /// Volume-gated shadow stream manager. When the anticipator's
    /// PlayTrack confidence is very high AND we have a high-likelihood
    /// candidate track, we silently pre-play it on the device with
    /// volume=0 so a tap promotes to instant audio.
    shadow: shadow_stream::ShadowManager,
    /// Last device-reported volume — saved into the shadow before we
    /// zero the device. Updated on every Volume result.
    last_known_volume: i32,
    /// Track id of the candidate currently primed for shadow (top of
    /// library_json after the most recent library refresh). We don't
    /// shadow until we have a target with high confidence.
    shadow_candidate: Option<i64>,
    /// Briefly true after a shadow-hit so the UI can show "instant via
    /// shadow" as a delight detail in the play-status strip.
    last_play_was_shadow: qt_property!(bool; NOTIFY play_status_changed),
    /// When true, manual volume changes can exceed the unsafe threshold
    /// (≈95 dB). False by default; the user has to explicitly unlock it
    /// via `enable_high_volume()` to acknowledge the risk. Resets when
    /// switching devices.
    volume_high_unlocked: qt_property!(bool; NOTIFY volume_high_unlocked_changed),
    volume_high_unlocked_changed: qt_signal!(),
    /// Volume value the user attempted to set that exceeded the unsafe
    /// threshold (0 when nothing pending). The UI surfaces this so the
    /// user can choose: cancel, or unlock and retry.
    volume_pending_request: qt_property!(i32; NOTIFY volume_clamped_changed),
    /// True when the *current* volume is in the loud band (88-95 dB).
    /// Drives a styling change on the volume control.
    volume_is_loud: qt_property!(bool; NOTIFY volume_changed),
    /// The per-product loud threshold (vol where ≈88 dB starts).
    /// Exposed so the UI can draw a "loud zone" marker on the volume slider.
    safety_loud_volume: qt_property!(i32; NOTIFY url_changed),
    /// The per-product unsafe threshold (vol where ≈95 dB starts).
    safety_unsafe_volume: qt_property!(i32; NOTIFY url_changed),
    url_changed: qt_signal!(),

    // JSON array of known devices: [{name, url, type}]. Drives the header
    // device selector. Seeded from a config list; can grow via discovery.
    devices_json: qt_property!(String; NOTIFY devices_changed),
    devices_changed: qt_signal!(),

    /// JSON array of devices enriched with priority + live-state info.
    /// Each entry is:
    ///   {name, url, type, group, group_label, group_color, score, explain,
    ///    reachable, on_battery, battery_percent, on_ethernet, on_wifi,
    ///    bluetooth_active}
    /// Sorted by priority (PrimaryHub first). The UI renders badges +
    /// status lines from this list.
    priority_devices_json: qt_property!(String; NOTIFY priority_changed),
    /// URL of the recommended hub. May be empty when nothing is reachable.
    recommended_hub_url: qt_property!(String; NOTIFY priority_changed),
    /// product_id of the recommended hub (for the header label).
    recommended_hub_product: qt_property!(String; NOTIFY priority_changed),
    priority_changed: qt_signal!(),

    /// Cached per-device live state, keyed by URL. Updated by probes.
    device_states: std::collections::HashMap<String, device_priority::DeviceState>,
    /// Re-evaluate priorities for all known devices (probes live state
    /// via the worker channel). Called on startup + every device change.
    refresh_priorities: qt_method!(fn(&mut self)),
    /// Apply the recommended hub as the active device.
    apply_recommended_hub: qt_method!(fn(&mut self)),

    volume: qt_property!(i32; NOTIFY volume_changed),
    volume_changed: qt_signal!(),

    muted: qt_property!(bool; NOTIFY muted_changed),
    muted_changed: qt_signal!(),

    product_name: qt_property!(String; NOTIFY product_info_changed),
    product_type: qt_property!(String; NOTIFY product_info_changed),
    product_info_changed: qt_signal!(),
    products: qt_property!(String; NOTIFY productsChanged),
    productsChanged: qt_signal!(),

    // unused at the moment
    kde_media_active: qt_property!(bool; NOTIFY kde_media_changed),
    kde_media_changed: qt_signal!(),

    passthrough: qt_property!(bool; NOTIFY passthrough_changed),
    passthrough_changed: qt_signal!(),

    stream_uri: qt_property!(String; NOTIFY stream_uri_changed),
    stream_uri_changed: qt_signal!(),

    auto_route_kde: qt_property!(bool; NOTIFY auto_route_changed),
    auto_route_changed: qt_signal!(),

    // ── Sources ─────────────────────────────────────────────────────
    sources_json: qt_property!(String; NOTIFY sources_changed),
    active_source_id: qt_property!(String; NOTIFY sources_changed),
    sources_changed: qt_signal!(),

    // ── Now playing ─────────────────────────────────────────────────
    np_title: qt_property!(String; NOTIFY now_playing_changed),
    np_artist: qt_property!(String; NOTIFY now_playing_changed),
    np_source: qt_property!(String; NOTIFY now_playing_changed),
    np_playing: qt_property!(bool; NOTIFY now_playing_changed),
    /// Track position estimate in seconds since play started, derived
    /// from wall clock at play_track time. Best-effort: paplay
    /// doesn't expose position natively, so this is the elapsed
    /// time since the start of the current play. Set to 0 on every
    /// new track. QML uses this to drive the progress fill in the
    /// BottomTransportBar.
    np_position_sec: qt_property!(i32; NOTIFY now_playing_changed),
    /// Track duration in seconds — from the tracks table if known,
    /// else 0 (in which case QML hides the progress fill rather
    /// than showing a misleading "0%" forever).
    np_duration_sec: qt_property!(i32; NOTIFY now_playing_changed),
    np_seek_pos: qt_property!(i32; NOTIFY now_playing_changed),
    now_playing_changed: qt_signal!(),

    // ── Standby / power ─────────────────────────────────────────────
    standby: qt_property!(bool; NOTIFY standby_changed),
    standby_changed: qt_signal!(),

    // ── Sound modes / presets (Mozart) ──────────────────────────────
    sound_modes_json: qt_property!(String; NOTIFY sound_modes_changed),
    active_sound_mode: qt_property!(String; NOTIFY sound_modes_changed),
    sound_modes_changed: qt_signal!(),

    // ── Media library ───────────────────────────────────────────────
    // JSON arrays of TrackSummary / group counts for the Library page.
    library_json: qt_property!(String; NOTIFY library_changed),
    library_context: qt_property!(String; NOTIFY library_changed),
    library_changed: qt_signal!(),
    artists_json: qt_property!(String; NOTIFY artists_changed),
    /// Which artist context (\"top\", \"alpha\") populated artists_json.
    /// QML uses this to render with appropriate sort/heading.
    artists_context: qt_property!(String; NOTIFY artists_changed),
    artists_changed: qt_signal!(),

    /// Paginated-load result, JSON-wrapped:
    /// `{"context": String, "offset": i64, "total": i64, "items": [...]}`.
    /// Updated whenever a `load_*_page` method completes.
    ///
    /// QML side reads this once per page-fetch by binding to
    /// `onPage_changed`, parses the JSON, and either replaces or
    /// appends to its ListView model based on whether the result's
    /// `offset` is 0 or > 0. The wrapper-object shape is necessary
    /// to keep the four fields (context/offset/total/items) atomic;
    /// without it, the QML side would see intermediate states where
    /// `total` belonged to the OLD page and `items` to the new one.
    page_json: qt_property!(String; NOTIFY page_changed),
    page_changed: qt_signal!(),
    /// QML-callable: request a page of the alphabetized artists list.
    /// 0-indexed offset, positive limit. Result lands as a Page
    /// ResultMessage and is published via `page_json`.
    load_artists_page: qt_method!(fn(&mut self, offset: i32, limit: i32)),
    /// Same, for the recently-added tracks list.
    load_recent_page: qt_method!(fn(&mut self, offset: i32, limit: i32)),
    /// Same, for a specific artist's tracks.
    load_by_artist_page: qt_method!(fn(&mut self, artist_id: i32, offset: i32, limit: i32)),
    /// Every track in the library (the "All Tracks" view), paginated and
    /// title-ordered. Guarantees no track is unreachable regardless of
    /// metadata — backs the missing-tracks fix.
    load_all_tracks_page: qt_method!(fn(&mut self, offset: i32, limit: i32)),
    /// Current sort for the all-tracks browse list: "title" / "artist" /
    /// "album" / "recent" / "rating". Persisted; drives load_all_tracks_page.
    all_tracks_sort: qt_property!(QString; NOTIFY all_tracks_sort_changed),
    all_tracks_sort_changed: qt_signal!(),
    set_all_tracks_sort: qt_method!(fn(&mut self, sort: String)),

    // ── Albums (browse-by-album view) ─────────────────────────────
    //
    // Tracks already carry album + year fields; albums are an
    // aggregation on top of those. Pattern mirrors artists exactly:
    // a single JSON property holds the current list (top OR alpha),
    // with `albums_context` indicating which is loaded so the QML
    // side can label and sort appropriately.
    //
    // For the detail view we use the existing TrackList channel
    // (album-tracks share the same shape as artist-tracks / search
    // results), tagged with "album:<title>:<primary_artist>" so the
    // result handler can route to the album page properties.
    albums_json:    qt_property!(String; NOTIFY albums_changed),
    albums_context: qt_property!(String; NOTIFY albums_changed),
    albums_changed: qt_signal!(),
    /// Title of the album currently opened in the detail view. Empty
    /// before any album is opened. QML uses this for the page header.
    current_album_title: qt_property!(String; NOTIFY current_album_changed),
    /// Primary artist of the album currently opened (needed to
    /// disambiguate same-title albums by different artists).
    current_album_artist: qt_property!(String; NOTIFY current_album_changed),
    /// JSON array of tracks in the open album.
    current_album_tracks_json: qt_property!(String; NOTIFY current_album_changed),
    current_album_changed: qt_signal!(),
    tags_json: qt_property!(String; NOTIFY tags_changed),
    tags_changed: qt_signal!(),

    // ── Playlists, queue, history (music-player core) ────────────────

    /// JSON array of all playlists (PlaylistSummary[]).
    playlists_json: qt_property!(String; NOTIFY playlists_changed),
    playlists_changed: qt_signal!(),
    /// The id of the most recently created playlist. QML watches this
    /// to navigate to the new playlist's detail page.
    last_playlist_created: qt_property!(i64; NOTIFY last_playlist_changed),
    last_playlist_changed: qt_signal!(),

    /// The currently-open playlist's id (or -1 when no playlist page is open).
    current_playlist_id: qt_property!(i64; NOTIFY current_playlist_changed),
    /// JSON array of TrackSummary for the currently-open playlist.
    current_playlist_tracks_json: qt_property!(String; NOTIFY current_playlist_changed),
    current_playlist_changed: qt_signal!(),

    /// The active device's play queue as JSON. Updates whenever a track
    /// is pushed, popped, or reordered.
    queue_json: qt_property!(String; NOTIFY queue_changed),
    queue_changed: qt_signal!(),
    /// Device URL the currently-displayed queue belongs to. Mirrors
    /// active_url by default but lets QueuePage peek at other devices'
    /// queues via load_queue_for_device without re-routing volume.
    queue_view_device_url: qt_property!(String; NOTIFY queue_changed),
    /// List of all devices that have queued items, with counts.
    /// Driven by LoadQueueDevices.
    queue_devices_json: qt_property!(String; NOTIFY queue_devices_changed),
    queue_devices_changed: qt_signal!(),

    // ── Soundscape loop processor ──────────────────────────────────
    //
    // Drives SoundscapePage. The progress property is updated as the
    // random-walk search runs; result_json carries the final
    // {output_path, size_bytes, loop} object on success. running is
    // true between start and completion so the UI can show a busy
    // indicator without us having to invent a separate state machine.

    /// 0.0..1.0 during search, snaps to 1.0 on completion.
    soundscape_progress: qt_property!(f32; NOTIFY soundscape_changed),
    /// True from process_soundscape() invocation until done/failed.
    soundscape_running: qt_property!(bool; NOTIFY soundscape_changed),
    /// JSON object on success: {output_path, size_bytes, loop:{...}}.
    /// Empty string before any run completes.
    soundscape_result_json: qt_property!(String; NOTIFY soundscape_changed),
    /// Last error message (empty on success).
    soundscape_error: qt_property!(String; NOTIFY soundscape_changed),
    soundscape_changed: qt_signal!(),

    /// Run the loop search + Opus encode. Returns immediately; result
    /// arrives via SoundscapeProgress + SoundscapeDone/Failed.
    process_soundscape: qt_method!(fn(&mut self,
        input_path: String,
        output_path: String,
        target_seconds: f32,
        tolerance_seconds: f32,
        n_candidates: i32,
        bitrate_kbps: i32,
        fade_ms: i32)),

    // ── Stem split + virtual staging ──────────────────────────────
    //
    // QML calls process_stem_split(input_wav_path, output_dir). The
    // controller:
    //   1. Reads the input WAV (any sample rate, mono or stereo —
    //      stereo is downmixed to mono before splitting).
    //   2. Runs stem_split::split_stems → 4 mono streams.
    //   3. Writes drums.wav, voice.wav, piano.wav, synth.wav into
    //      `output_dir`.
    //   4. Runs stem_stage::stage with default placements →
    //      writes staged_stereo.wav.
    //   5. Surfaces success/error in stem_status_json.
    //
    // The work happens on a background std::thread so the QML thread
    // stays responsive; status updates come back via stem_changed.
    process_stem_split: qt_method!(fn(&mut self,
        input_wav: String,
        output_dir: String)),

    /// JSON status string. Shape:
    ///   {"state":"running"|"done"|"failed",
    ///    "progress":0.0..1.0,
    ///    "message":"…",
    ///    "outputs":["drums.wav","voice.wav",...]}
    stem_status_json: qt_property!(String; NOTIFY stem_changed),
    stem_running: qt_property!(bool; NOTIFY stem_changed),
    stem_changed: qt_signal!(),

    // ── Podcast speaker diarization ───────────────────────────────
    //
    // QML calls process_diarize(input_wav_path). The controller:
    //   1. Reads the input WAV (any sample rate, mono or stereo —
    //      stereo downmixed).
    //   2. Runs podcast_diar::diarize → Vec<DiarSegment>.
    //   3. Builds a JSON timeline and assigns it to diar_segments_json.
    //
    // Same threading pattern as stem split — work goes to a
    // background thread; results come back via diar_changed.
    process_diarize: qt_method!(fn(&mut self, input_wav: String)),

    /// JSON array. Shape:
    ///   [{"start":0.0,"end":3.2,"speaker":0}, ...]
    diar_segments_json: qt_property!(String; NOTIFY diar_changed),
    diar_running: qt_property!(bool; NOTIFY diar_changed),
    diar_status: qt_property!(String; NOTIFY diar_changed),
    diar_changed: qt_signal!(),

    // ── Realtime stream mixer ──────────────────────────────────────
    //
    // Two channel groups — "speaker" (operates on the Mid signal —
    // centered content, typically vocals) and "instrumental" (Side —
    // wide-panned, typically instruments). Each group has a 3-band
    // EQ with LOCKED center frequencies (in realtime_mixer.rs as
    // SPK_* / INS_* constants — pinned to the perceptually
    // meaningful zones for that musical role) plus a master volume.
    //
    // Gains are in dB; the UI maps slider position 0..1 to ±12 dB.
    // Volume is linear (0.0..2.0 in the UI; 1.0 = unity).
    //
    // Changes propagate to the audio thread via apply_mixer_params()
    // which acquires the mixer's RwLock briefly. Reads from the
    // audio thread use a read lock — uncontended in typical use.

    mix_spk_low:   qt_property!(f32; NOTIFY mixer_changed),
    mix_spk_mid:   qt_property!(f32; NOTIFY mixer_changed),
    mix_spk_high:  qt_property!(f32; NOTIFY mixer_changed),
    mix_spk_vol:   qt_property!(f32; NOTIFY mixer_changed),
    mix_ins_low:   qt_property!(f32; NOTIFY mixer_changed),
    mix_ins_mid:   qt_property!(f32; NOTIFY mixer_changed),
    mix_ins_high:  qt_property!(f32; NOTIFY mixer_changed),
    mix_ins_vol:   qt_property!(f32; NOTIFY mixer_changed),
    /// Stereo width / spatializer, 0.0 (mono) … 2.0 (double-wide), default
    /// 1.0 (off). Scales the Side component in the mixer's M/S stage.
    mix_width:     qt_property!(f32; NOTIFY mixer_changed),
    mixer_changed: qt_signal!(),

    /// Apply a single slider change. `channel` is "spk" or "ins";
    /// `band` is "low", "mid", "high", or "vol". The value is in dB
    /// for EQ bands and linear for "vol".
    set_mixer_value: qt_method!(fn(&mut self,
        channel: String, band: String, value: f32)),
    /// Reset every band to neutral (0 dB) and both volumes to 1.0.
    reset_mixer: qt_method!(fn(&mut self)),
    /// Reset just ONE channel's sliders (low/mid/high/vol) to
    /// neutral, leaving the other channel untouched. Pass "spk"
    /// or "ins". Used by the per-channel ↺ button on each
    /// ChannelColumn header.
    reset_mixer_channel: qt_method!(fn(&mut self, channel: String)),
    /// Locked band center frequencies for the QML labels. Returned
    /// as a JSON object so the UI doesn't have to hardcode them.
    mixer_band_centers_json: qt_property!(String; NOTIFY mixer_changed),
    /// Frequency-response curves for the two channels, as a JSON
    /// array of [{hz, spk_db, ins_db}, ...]. Recomputed on every
    /// mixer_changed signal — the renderer just re-reads this
    /// property and redraws. ~64 log-spaced points cover the
    /// audible range (20 Hz – 20 kHz). The "groups make sense"
    /// principle becomes literally visible: moving SPK MID lifts
    /// the speaker curve at 1.5 kHz, INS LOW lifts the instrumental
    /// curve at 200 Hz, and so on.
    mixer_curves_json: qt_property!(String; NOTIFY mixer_changed),

    // ── Mixer offline file processor ───────────────────────────────
    // Drives MixerPage's file processor section. The user picks an
    // input WAV + output Opus path, hits Run, and the worker thread
    // pipes the WAV through the SAME mixer code path the realtime
    // audio thread will use — so this section doubles as a preview
    // of what live processing will sound like.
    mixer_proc_progress: qt_property!(f32; NOTIFY mixer_proc_changed),
    mixer_proc_running:  qt_property!(bool; NOTIFY mixer_proc_changed),
    mixer_proc_result_json: qt_property!(String; NOTIFY mixer_proc_changed),
    mixer_proc_error: qt_property!(String; NOTIFY mixer_proc_changed),
    mixer_proc_changed: qt_signal!(),

    // ── Spectrum analyzer ──────────────────────────────────────────
    //
    // Source-material spectrum overlay for the mixer page. The user
    // picks any WAV; we FFT-average it; the canvas draws the result
    // behind the EQ curves on the same log-frequency / dB axis. This
    // is the third layer of the "groups make sense" verification:
    // even before they touch a knob, the user can see whether their
    // source material actually has content where the locked band
    // centers are. If a podcast has its energy mostly below 1 kHz,
    // they can pick that up at a glance and decide to favor SPEAKER
    // LOW + MID; if a piece of music has cymbals dominating, they
    // know INSTRUMENTAL HIGH will land somewhere meaningful.
    //
    // JSON shape: {"input_path": "...", "points": [{hz, db}, ...]}.
    // Empty string before any analysis runs.
    spectrum_json:    qt_property!(String; NOTIFY spectrum_changed),
    spectrum_running: qt_property!(bool;   NOTIFY spectrum_changed),
    spectrum_error:   qt_property!(String; NOTIFY spectrum_changed),
    spectrum_changed: qt_signal!(),

    /// Compute the spectrum of a WAV file in the background. Result
    /// arrives via spectrum_json + spectrum_changed.
    analyze_spectrum: qt_method!(fn(&mut self, input_path: String)),
    /// Clear the spectrum overlay (sets spectrum_json to empty).
    clear_spectrum:   qt_method!(fn(&mut self)),

    /// Process a WAV file through the current mixer settings into an
    /// Opus output. Returns immediately; result via mixer_proc_*.
    process_through_mixer_file: qt_method!(fn(&mut self,
        input_path: String,
        output_path: String,
        bitrate_kbps: i32)),
    /// Like process_through_mixer_file but, on success, immediately
    /// exposes the output file via the track server and tells the
    /// active device to play it. This is the closest thing to a
    /// realtime mixer the bo-controller offers: snapshot the slider
    /// state, render the mix, hand it to the speakers. Future:
    /// continuous-streaming version that respects live slider moves.
    mix_and_play_file: qt_method!(fn(&mut self,
        input_path: String,
        bitrate_kbps: i32)),
    /// LIVE STREAM through the mixer. Unlike mix_and_play_file, this
    /// doesn't snapshot params at click-time — the track_server
    /// reads the shared mixer state on every 20 ms frame, so slider
    /// moves during playback affect what comes out of the speaker
    /// within roughly the device's internal buffer (a few hundred
    /// ms). This is the "realtime stream mixer" workflow.
    live_stream_through_mixer: qt_method!(fn(&mut self,
        input_path: String,
        bitrate_kbps: i32)),

    // ── ANC: active noise cancellation ──
    //
    // The QML side calls anc_enable to start the engine + mic bridge,
    // then can run a probe at any time to (re)measure the round-trip
    // delay between this device's output and its mic. anc_play_with_anc
    // wires the full pipeline: ffmpeg-decode → engine.pull_output(mix)
    // → sink. Mix knobs (anti gain, music gain, blend toggle) are
    // qt_properties bound to the SettingsPage ANC card.
    anc_enable: qt_method!(fn(&mut self, on: bool)),
    /// Fire a single alignment probe. The sink string is the same
    /// PA device name we use for music playback ("" = default).
    /// Blocks for up to ~1 second; the QML side should call this from
    /// a button press and be ready for ~1s of UI lag.
    anc_run_probe: qt_method!(fn(&mut self, sink: String)),
    /// Adjust the anti-noise + music mix gains. Both clamped on the
    /// way through (anti to [0,1], music to [0,2]).
    anc_set_mix: qt_method!(fn(&mut self, anti: f64, music: f64)),
    /// Toggle whether anti-noise blends into the music stream (true)
    /// or plays standalone (false).
    anc_set_blend: qt_method!(fn(&mut self, blend: bool)),
    /// Start a blended-with-ANC playback for a single track. Returns
    /// immediately; the actual decode→mix→sink pipeline runs on a
    /// background thread. Stop the same way music normally stops.
    anc_play_with_anc: qt_method!(fn(&mut self,
        track_path: String, sink: String)),

    // ── Overnight standby pass-by cancellation ──
    //
    // Different from the always-on ANC engine: standby only emits
    // anti-noise during detected vehicle pass-by events. Idle most
    // of the night, fires for 20-30s when a car passes.
    /// Begin overnight pass-by cancellation against `sink`. Spawns
    /// a long-running paplay subprocess + a mic-watching thread.
    /// Idempotent — calling while already active is a no-op.
    anc_start_standby: qt_method!(fn(&mut self, sink: String)),
    /// Stop the overnight standby loop. Tears down the paplay
    /// subprocess and the watcher thread.
    anc_stop_standby: qt_method!(fn(&mut self)),
    /// Forget the learned per-phase transition timers. Useful when
    /// the user moves to a new location with different traffic
    /// patterns. The canceller starts fresh on the next event.
    anc_reset_standby_timers: qt_method!(fn(&mut self)),
    /// Whether standby mode is currently active.
    anc_standby_active: qt_property!(bool; NOTIFY anc_standby_changed),
    /// Current FSM phase: "Quiet" | "Approach" | "Peak" | "Recede".
    anc_standby_phase: qt_property!(String; NOTIFY anc_standby_changed),
    /// Estimated fundamental frequency of the current event, 0 if
    /// not in an event.
    anc_standby_fundamental_hz: qt_property!(f64; NOTIFY anc_standby_changed),
    /// Confidence in the learned transition timers, 0..1.
    anc_standby_timer_confidence: qt_property!(f64; NOTIFY anc_standby_changed),
    /// Doppler rate in Hz/s. Positive = approaching, negative =
    /// receding, ~0 = stationary noise source.
    anc_standby_doppler: qt_property!(f64; NOTIFY anc_standby_changed),
    /// Phase-lock quality, 0..1. The UI surfaces this so the user
    /// can see whether the canceller is genuinely producing
    /// destructive interference (quality > 0.5) vs. just emitting
    /// uncorrelated noise (quality < 0.2). After the fuzz harness
    /// + bin-interpolation fix this regularly reads 0.7-0.9 during
    /// a real pass-by; before, it sat near 0 the whole time.
    anc_standby_lock_quality: qt_property!(f64; NOTIFY anc_standby_changed),
    /// Tick the standby snapshot from the QML side. The runner
    /// updates its internal snapshot every window; this method
    /// pulls it through to the qt_properties so the UI refreshes.
    anc_standby_refresh: qt_method!(fn(&mut self)),
    anc_standby_changed: qt_signal!(),

    /// Current noise-adaptive scene-gain multiplier (0.7..1.4).
    /// The controller computes this from the canceller's ambient
    /// loudness observation and updates it as conditions change.
    /// QML reads this and dispatches `mux.set_scene_gain(g)` so
    /// bo-muxd applies it to the next round of voice spawns.
    /// 1.0 = nominal; below = quieter than nominal; above = louder.
    /// See noise_adaptive_gain.rs for the curve.
    anc_scene_gain: qt_property!(f64; NOTIFY anc_scene_gain_changed),
    /// The smoothed loudness ratio (band_energy / noise_floor) the
    /// curve was evaluated against. Surfaced for UI / diagnostics
    /// (e.g. "loudness 23× floor → boost +18%").
    anc_scene_gain_loudness: qt_property!(f64; NOTIFY anc_scene_gain_changed),
    anc_scene_gain_changed: qt_signal!(),

    /// V1 EXPERIMENTAL FEATURES TOGGLE.
    ///
    /// When OFF (default), only validated production features run:
    /// the music masker, the backup-beeper masker, the drone
    /// canceller. The passby canceller's process_window still runs
    /// for diagnostics but no anti-noise is committed to the speaker.
    ///
    /// When ON, the passby canceller emits anti-noise; the per-
    /// harmonic phase lock auto-mode activates; the JitterSmoother
    /// is available; and any other Experimental-class features
    /// become live. See docs/v1-plan.md for the full classification.
    experimental_enabled: qt_property!(bool; NOTIFY experimental_changed),
    /// Toggle experimental features on/off. Persists across restart
    /// via settings store. Emits experimental_changed.
    set_experimental: qt_method!(fn(&mut self, on: bool)),
    experimental_changed: qt_signal!(),

    /// The currently-open track's id for the tag editor (-1 = none).
    current_track_tags_id: qt_property!(i64; NOTIFY current_track_tags_changed),
    /// JSON array of TagInfo for the currently-selected track.
    current_track_tags_json: qt_property!(String; NOTIFY current_track_tags_changed),
    current_track_tags_changed: qt_signal!(),

    /// Recent play history as JSON.
    history_json: qt_property!(String; NOTIFY history_changed),
    history_changed: qt_signal!(),

    /// Plays-per-hour-of-day histogram as JSON (24 entries with
    /// fields `hour` 0–23 and `count` u32). Driven by LoadHistoryStats.
    history_stats_json: qt_property!(String; NOTIFY history_stats_changed),
    history_stats_changed: qt_signal!(),

    /// Rolling KPI summary as JSON: { total_plays, distinct_tracks,
    /// top_track: {track_id, title, primary_artist, play_count} | null,
    /// top_artist: {name, play_count} | null, lookback_days }.
    history_summary_json: qt_property!(String; NOTIFY history_summary_changed),
    history_summary_changed: qt_signal!(),

    // ── Music-player commands (QML-callable methods) ─────────────────

    /// Create a new playlist.
    create_playlist: qt_method!(fn(&mut self, name: String, description: String)),
    /// Rename + redescribe.
    update_playlist_meta: qt_method!(fn(&mut self, id: i64, name: String, description: String)),
    /// Delete (cascades to tracks).
    delete_playlist: qt_method!(fn(&mut self, id: i64)),
    /// Load all playlists.
    load_playlists: qt_method!(fn(&mut self)),
    /// Load a single playlist's track list.
    load_playlist_tracks: qt_method!(fn(&mut self, id: i64)),
    /// Add a single track to a playlist.
    add_track_to_playlist: qt_method!(fn(&mut self, playlist_id: i64, track_id: i64)),
    /// Remove a track from a playlist.
    remove_track_from_playlist: qt_method!(fn(&mut self, playlist_id: i64, track_id: i64)),
    /// Reorder: move track to new position.
    reorder_playlist: qt_method!(fn(&mut self, playlist_id: i64, track_id: i64, new_ord: i64)),
    /// Set a track's 0–5 star rating. Persists durably (path-mirrored);
    /// the confirmed value comes back via last_rated_track/last_rated_value.
    set_track_rating: qt_method!(fn(&mut self, track_id: i64, rating: i64)),
    /// Write metadata edits into the actual file's tags (portable +
    /// permanent). JSON body shape matches set_track_override
    /// (title/artist/album/year). Result lands in tag_write_status.
    write_track_tags_to_file: qt_method!(fn(&mut self, track_id: i32, json: String)),
    /// Last tag-write outcome message (success or error), for the UI.
    tag_write_status: qt_property!(QString; NOTIFY tag_write_status_changed),
    tag_write_status_changed: qt_signal!(),
    /// Last track whose rating was confirmed by the worker, and its value,
    /// so a rating widget can reconcile its optimistic state.
    last_rated_track: qt_property!(i64; NOTIFY rating_changed),
    last_rated_value: qt_property!(i64; NOTIFY rating_changed),
    rating_changed: qt_signal!(),

    /// Push current track or any track to the queue.
    queue_push: qt_method!(fn(&mut self, track_id: i64)),
    /// Insert at position N (0 = play next).
    queue_insert: qt_method!(fn(&mut self, track_id: i64, at: i64)),
    /// Pop the head — call after a track finishes playing.
    queue_pop: qt_method!(fn(&mut self)),
    /// Remove a specific queue entry.
    queue_remove: qt_method!(fn(&mut self, queue_id: i64)),
    /// Clear the active device's queue.
    queue_clear: qt_method!(fn(&mut self)),
    /// Refresh queue state from the catalog.
    load_queue: qt_method!(fn(&mut self)),
    /// Refresh the list of devices with queued items.
    load_queue_devices: qt_method!(fn(&mut self)),
    /// Load a specific device's queue without changing which device
    /// is active. Used by QueuePage's per-device picker to peek at
    /// other devices' queues without re-routing volume commands.
    load_queue_for_device: qt_method!(fn(&mut self, device_url: String)),

    /// Create or update a tag.
    upsert_tag: qt_method!(fn(&mut self, name: String, color: String)),
    /// Attach a tag to a track.
    attach_tag: qt_method!(fn(&mut self, track_id: i64, tag_id: i64)),
    /// Detach a tag from a track.
    detach_tag: qt_method!(fn(&mut self, track_id: i64, tag_id: i64)),
    /// Load tags for a single track.
    load_track_tags: qt_method!(fn(&mut self, track_id: i64)),
    /// Refresh tag list (counts).
    load_all_tags: qt_method!(fn(&mut self)),
    /// Load tracks attached to a tag (drill-down view).
    load_tracks_by_tag: qt_method!(fn(&mut self, tag_id: i64)),

    /// Top artists by play count.
    load_top_artists: qt_method!(fn(&mut self, limit: i32)),
    /// All artists alphabetical.
    load_all_artists_alpha: qt_method!(fn(&mut self)),
    /// Artists browse sort: "name" / "plays" / "tracks". Persisted.
    artist_sort: qt_property!(QString; NOTIFY artist_sort_changed),
    artist_sort_changed: qt_signal!(),
    set_artist_sort: qt_method!(fn(&mut self, sort: String)),
    /// Top albums by total play count.
    load_top_albums: qt_method!(fn(&mut self, limit: i32)),
    /// All albums alphabetical by title.
    load_all_albums_alpha: qt_method!(fn(&mut self)),
    /// Albums browse sort: "title" / "artist" / "year" / "plays". Persisted.
    album_sort: qt_property!(QString; NOTIFY album_sort_changed),
    album_sort_changed: qt_signal!(),
    set_album_sort: qt_method!(fn(&mut self, sort: String)),
    /// Open a specific album in the detail view. Populates the
    /// current_album_* properties asynchronously when the worker
    /// replies with the track list.
    open_album: qt_method!(fn(&mut self, title: String, primary_artist: String)),
    /// Clear the current album detail view (e.g. when navigating away).
    close_album: qt_method!(fn(&mut self)),

    /// Load top tracks by play count into the main library view.
    load_top_tracks: qt_method!(fn(&mut self, limit: i32)),
    /// Load recently-added tracks (by file mtime) into the library view.
    load_recently_added: qt_method!(fn(&mut self, limit: i32)),
    /// Load never-played tracks (play_count = 0) into the library view.
    load_never_played: qt_method!(fn(&mut self, limit: i32)),

    /// Recent play history.
    load_recent_history: qt_method!(fn(&mut self, limit: i32)),
    /// Plays-per-hour-of-day histogram for the History page chart.
    load_history_stats: qt_method!(fn(&mut self, lookback_days: i32)),
    /// Rolling KPI summary for the History page KPI strip.
    load_history_summary: qt_method!(fn(&mut self, lookback_days: i32)),
    /// Run an advanced multi-term search.
    search_advanced: qt_method!(fn(&mut self, query: String)),
    // Scan progress: phase text + done/total for a progress bar.
    scan_phase: qt_property!(String; NOTIFY scan_changed),
    scan_done: qt_property!(i32; NOTIFY scan_changed),
    scan_total: qt_property!(i32; NOTIFY scan_changed),
    scan_changed: qt_signal!(),
    // Catalog totals for the library header.
    lib_total: qt_property!(i32; NOTIFY catalog_stats_changed),
    lib_parsed: qt_property!(i32; NOTIFY catalog_stats_changed),
    lib_analyzed: qt_property!(i32; NOTIFY catalog_stats_changed),
    catalog_stats_changed: qt_signal!(),

    // Methods called from QML
    refresh_volume: qt_method!(fn(&mut self)),
    set_volume_api: qt_method!(fn(&mut self, v: i32)),
    /// Inline per-device volume from the playbar strip. If `url` is the
    /// active device, this is the normal safe volume path. If it's a
    /// different device, we promote it to active first (so volume routes
    /// through the right driver with the right safety ceiling), then set.
    set_device_volume: qt_method!(fn(&mut self, url: String, v: i32)),
    refresh_mute: qt_method!(fn(&mut self)),
    toggle_mute: qt_method!(fn(&mut self)),
    refresh_product_info: qt_method!(fn(&mut self)),
    toggle_passthrough: qt_method!(fn(&mut self)),
    start_stream: qt_method!(fn(&mut self, uri: String)),
    stop_stream: qt_method!(fn(&mut self)),
    toggle_auto_route: qt_method!(fn(&mut self)),
    tx_check_audio: qt_method!(fn(&mut self)),
    poll_results: qt_method!(fn(&mut self)),

    // ── Added feature methods ───────────────────────────────────────
    refresh_sources: qt_method!(fn(&mut self)),
    select_source: qt_method!(fn(&mut self, source_id: String)),
    refresh_now_playing: qt_method!(fn(&mut self)),
    transport: qt_method!(fn(&mut self, action: String)),
    seek_to: qt_method!(fn(&mut self, secs: i32)),
    refresh_standby: qt_method!(fn(&mut self)),
    toggle_standby: qt_method!(fn(&mut self)),
    refresh_sound_modes: qt_method!(fn(&mut self)),
    select_sound_mode: qt_method!(fn(&mut self, mode_id: String)),
    refresh_all: qt_method!(fn(&mut self)),
    select_device: qt_method!(fn(&mut self, url: String)),
    /// Variant of select_device that also tells the controller what product
    /// type was selected — needed so volume_safety can pick the right
    /// dB-approximation ceiling on first connect.
    select_device_with_product: qt_method!(fn(&mut self, url: String, product_id: String)),
    /// Unlock high-volume mode for the current session. Future set_volume
    /// requests above the unsafe threshold (≈95 dB) will go through.
    /// Resets to false when the user switches devices.
    enable_high_volume: qt_method!(fn(&mut self)),
    /// Re-lock the hard volume ceiling.
    disable_high_volume: qt_method!(fn(&mut self)),
    /// Clear the volume_was_clamped flag (after the user dismisses the toast).
    dismiss_clamp_warning: qt_method!(fn(&mut self)),

    // ── Frequency sanitizer ──────────────────────────────────────────
    /// Start the sanitizer with a preset id (pink/brown/engine/road/fan/
    /// hvac), an anti-tone fundamental in Hz (0 = noise only), and a
    /// gain in dB (typically -10..-30).
    start_sanitizer: qt_method!(fn(&mut self, preset_id: String, freq_hz: f64, gain_db: f64)),
    /// Stop the sanitizer cleanly.
    stop_sanitizer: qt_method!(fn(&mut self)),
    /// Update the anti-tone fundamental without restarting the player.
    retune_sanitizer: qt_method!(fn(&mut self, freq_hz: f64)),
    /// Is the sanitizer subprocess currently running?
    sanitizer_running: qt_property!(bool; NOTIFY sanitizer_changed),
    /// Current preset id (empty when not running).
    sanitizer_preset: qt_property!(String; NOTIFY sanitizer_changed),
    /// Current anti-tone fundamental in Hz (0 when noise-only or not running).
    sanitizer_freq: qt_property!(f64; NOTIFY sanitizer_changed),
    /// Last error message from start_sanitizer (e.g. "no audio player found").
    sanitizer_error: qt_property!(String; NOTIFY sanitizer_changed),
    sanitizer_changed: qt_signal!(),

    // ── Adaptive (predictive) cancellation ──────────────────────────
    /// Start adaptive cancellation: open the mic, learn the dominant
    /// periodic noise pattern, and mix the inverse-phase prediction
    /// into the sanitizer output. Requires the sanitizer to already be
    /// running (it shares the same audio path).
    start_adaptive_cancellation: qt_method!(fn(&mut self)),
    /// Stop adaptive cancellation; the predictor goes silent. The
    /// sanitizer's shaped-noise + anti-tones keep playing.
    stop_adaptive_cancellation: qt_method!(fn(&mut self)),
    /// True when the pattern runner is alive and learning.
    adaptive_active: qt_property!(bool; NOTIFY adaptive_changed),
    /// Last detected periodic-noise frequency in Hz (0 if no recent detection).
    adaptive_detected_hz: qt_property!(f64; NOTIFY adaptive_changed),
    /// Last periodicity score 0..1 (how predictable the noise is).
    adaptive_score: qt_property!(f64; NOTIFY adaptive_changed),
    /// Name of the mic capture backend in use ("parec" or "arecord"),
    /// or empty when not active.
    adaptive_backend: qt_property!(String; NOTIFY adaptive_changed),
    /// Last error from start_adaptive_cancellation (e.g. "no parec found").
    adaptive_error: qt_property!(String; NOTIFY adaptive_changed),
    /// Refresh the adaptive_* properties from the runner. QML uses a Timer
    /// to call this every ~1s so the freq/score values stay live.
    refresh_adaptive_status: qt_method!(fn(&mut self)),
    adaptive_changed: qt_signal!(),
    add_device: qt_method!(fn(&mut self, name: String, url: String)),

    // ── Media library methods ───────────────────────────────────────
    rescan_library: qt_method!(fn(&mut self)),
    quick_scan: qt_method!(fn(&mut self)),
    /// Scan a user-chosen list of folders (passed as a JSON array string).
    /// Empty / "[]" falls back to the configured defaults.
    scan_with_roots: qt_method!(fn(&mut self, roots_json: String)),
    /// Suggested scan roots: ~/Music, ~/Documents, ~/Downloads — only
    /// those that actually exist. The dialog uses this to pre-tick boxes.
    suggested_scan_roots: qt_method!(fn(&mut self) -> String),
    /// The most recently scanned root path (persisted). The scan picker
    /// defaults to it and the native Browse dialog opens there.
    last_scan_root: qt_property!(QString; NOTIFY last_scan_root_changed),
    last_scan_root_changed: qt_signal!(),
    /// Open the system file picker (kdialog on KDE, zenity elsewhere)
    /// to pick an audio file. Returns the chosen path or empty string
    /// on cancel/no-picker. Synchronous — the qt_method blocks until
    /// the picker closes, but the picker runs as a subprocess so the
    /// rest of the app stays responsive enough that this is acceptable
    /// UX for a "Browse…" button.
    pick_audio_file: qt_method!(fn(&mut self) -> String),
    /// Folder picker (touch-friendly on KDE/plasma-mobile). Used by
    /// import flows where the user wants to point at a whole tree
    /// instead of one file at a time. See pick_audio_folder fn.
    pick_audio_folder: qt_method!(fn(&mut self) -> String),

    /// Look up the discovered/added friendly name for the currently
    /// active device URL. Falls back to the URL itself if no name
    /// was captured (manually-added devices without discovery name).
    /// Used by the Sound page's DEVICE card so something useful
    /// shows for Model when REST product-info probes fail (which
    /// they do on devices that respond to mDNS but reject the
    /// product-info endpoint, like the Emerge under recent firmware).
    active_device_name: qt_method!(fn(&mut self) -> String),

    /// Suggest a default directory for soundscape outputs. Picks the
    /// first existing scan root (typically ~/Music) and appends
    /// /Soundscapes. The directory is created on first render via
    /// the worker (process_soundscape ensures parent dirs exist).
    /// Returns "" if no scan roots exist and HOME is unset.
    suggested_soundscapes_dir: qt_method!(fn(&mut self) -> String),
    /// List previously-rendered soundscapes from
    /// `suggested_soundscapes_dir`. Returns a JSON array of
    /// `{path, name, size_bytes, modified_unix}` sorted by modified
    /// time descending (newest first). Empty array if the directory
    /// doesn't exist yet or contains no .opus files. The QML
    /// SoundscapePage uses this to render a "previous renders" list.
    list_soundscapes: qt_method!(fn(&mut self) -> String),
    /// Add a freshly-rendered soundscape file to the music catalog by
    /// triggering a focused scan of its containing directory. The
    /// soundscape lands in `~/Music/Soundscapes/` by default — which
    /// is typically under the existing scan root `~/Music/`, so the
    /// file would eventually appear via the next full RescanLibrary
    /// anyway. This shortcut makes it immediate: the user can see
    /// their soundscape in Library and Search without waiting for
    /// the next scheduled scan.
    ///
    /// The async scan emits ScanProgress events; status surfaces in
    /// `last_play_status`. Returns true if the path looked valid
    /// enough to dispatch; false on obvious error (empty / non-file).
    add_soundscape_to_library: qt_method!(fn(&mut self, path: String) -> bool),
    /// Delete a soundscape file from disk. Path must resolve to a
    /// file under the user's Soundscapes directory — a defensive
    /// sandbox to prevent stray rm calls from this method being
    /// abused as a file-system blast radius. Returns true if the
    /// file existed and was successfully removed.
    delete_soundscape: qt_method!(fn(&mut self, path: String) -> bool),
    /// Rename a soundscape file. `new_name` is treated as just the
    /// new basename (no path separators allowed). The renamed file
    /// stays in the same directory. Useful for the user to tag a
    /// good render before it gets buried under "rain v23.opus".
    /// Returns true on success.
    rename_soundscape: qt_method!(
        fn(&mut self, old_path: String, new_name: String) -> bool),
    analyze_backlog: qt_method!(fn(&mut self)),
    cancel_analysis: qt_method!(fn(&mut self)),
    load_recent: qt_method!(fn(&mut self)),
    load_artists: qt_method!(fn(&mut self)),
    load_by_artist: qt_method!(fn(&mut self, artist_id: i32)),
    load_tags: qt_method!(fn(&mut self)),
    load_by_tag: qt_method!(fn(&mut self, tag_id: i32)),
    load_home: qt_method!(fn(&mut self)),
    record_play: qt_method!(fn(&mut self, track_id: i32)),
    /// Play the given track on the currently-active device. Looks up the
    /// file path, exposes it via the local track HTTP server, and POSTs
    /// the resulting URL to the device's ASE/BeoZone play endpoint(s).
    /// Multiple endpoint shapes are tried; the first 2xx wins. The track
    /// server is spun up lazily on first use.
    play_track: qt_method!(fn(&mut self, track_id: i32)),
    /// Single-tap semantics: queue gating. If something is currently
    /// playing, ADD the track to the active device's queue and
    /// surface that to the user — don't interrupt. If nothing is
    /// playing, fall through to play_track as the natural-feeling
    /// behavior. See fn enqueue_or_play for the decision.
    enqueue_or_play: qt_method!(fn(&mut self, track_id: i32)),
    /// Mode-aware enqueue. `mode` is a queue_policy::EnqueueMode tag:
    /// "stack", "replace", "sync", or "midgap". The policy turns the mode
    /// + current playback state into a concrete action plan which this
    /// executes. Stack/Replace use the linear queue; sync/midgap are
    /// layered (executed when the multi-layer player lands — for now they
    /// report back via last_play_status).
    enqueue_with_mode: qt_method!(fn(&mut self, track_id: i32, mode: String)),
    /// The user's chosen default add-behaviour ("stack" / "replace" /
    /// "sync" / "midgap"). Persisted; drives the plain enqueue gesture.
    enqueue_mode: qt_property!(QString; NOTIFY enqueue_mode_changed),
    enqueue_mode_changed: qt_signal!(),
    set_enqueue_mode: qt_method!(fn(&mut self, mode: String)),
    /// Double-tap semantics: "play now and resume". Push the
    /// currently-playing track onto the front of the queue
    /// (position 0) so it'll play next, then play the requested
    /// track immediately. If nothing is playing, just play.
    play_now: qt_method!(fn(&mut self, track_id: i32)),
    /// Stop the device-side playback (best-effort — uses the standard
    /// Mozart stop endpoint).
    stop_track_playback: qt_method!(fn(&mut self)),
    /// Clear the last_play_status banner (without affecting actual playback).
    dismiss_play_status: qt_method!(fn(&mut self)),
    /// The most recent play_track outcome (Used endpoint name or error).
    last_play_status: qt_property!(String; NOTIFY play_status_changed),
    /// The URL the device was last asked to play (so the user can see
    /// what we tried). Useful for diagnosing "device returned 404" issues.
    last_play_url: qt_property!(String; NOTIFY play_status_changed),
    play_status_changed: qt_signal!(),
    /// Full-text search across title, artist, album. Empty query clears.
    library_search: qt_method!(fn(&mut self, query: String)),
    /// Save a manual override for a track. Accepts a JSON object string with
    /// any of "title"/"artist"/"album"/"year"; missing keys mean unchanged
    /// from parsed. Empty object {} removes the override (parsed values
    /// reappear). Using a JSON blob avoids qt_method arity surprises.
    set_track_override: qt_method!(fn(&mut self, track_id: i32, override_json: String)),
    /// Ask the worker for BPM/key on a specific track (powers the now-playing chip).
    load_key_bpm: qt_method!(fn(&mut self, track_id: i32)),

    /// Identification ping: a short distinctive two-tone sound played
    /// ON the target device's speakers so the user can hear which
    /// physical hardware they're targeting. Pre-warms the connection
    /// via two GetVolume cycles then fires the play command. QML calls
    /// this from the device picker. The two arguments are the URL +
    /// product_id of the device to ping (not necessarily the active one).
    ping_device: qt_method!(fn(&mut self, url: String, product_id: String)),
    /// Cached `/t/<token>` URL segment for the synthesized ping WAV,
    /// once it's been registered with the track server. Reused across
    /// all device-identify calls in the session.
    ping_path_cache: String,

    // ── Bluetooth auto-connect ─────────────────────────────────────────
    //
    // Per-MAC "let this device connect automatically" toggle. When
    // enabled, the controller periodically runs `bluetoothctl connect
    // <MAC>` until the device joins, with exponential backoff to keep
    // load light when the device is unreachable.

    bt_autoconnect: bt_autoconnect::BtAutoConnect,
    /// System-respect layer — tracks which BT devices were connected
    /// at startup vs which we caused to connect during this session,
    /// so the on-shutdown cleanup only undoes our own work and never
    /// touches devices the user had connected for other reasons. See
    /// bt_respect.rs for the policy.
    bt_respect: bt_respect::BtRespect,
    /// JSON of `bt_respect` audit log + summary, surfaced to QML for
    /// the Settings page. The user can audit "what did the controller
    /// do to my BT?" without having to read logs.
    bt_respect_summary: qt_property!(String; NOTIFY bt_respect_changed),
    bt_respect_log_json: qt_property!(String; NOTIFY bt_respect_changed),
    /// Idle-cleanup threshold in minutes (1..60). Surfaced as a
    /// slider on the Settings page so the user can shorten/lengthen
    /// the auto-disconnect window without rebuilding. Default 15.
    /// Persisted via app_kv so the choice survives restarts.
    bt_respect_idle_minutes: qt_property!(i32; NOTIFY bt_respect_changed),
    bt_respect_changed: qt_signal!(),
    /// Update the idle threshold from QML. Clamps to 1..=60 minutes
    /// and persists.
    set_bt_respect_idle_minutes: qt_method!(fn(&mut self, m: i32)),
    /// QML calls this on app close (and after idle timeout) so the
    /// controller disconnects any MACs it caused to connect during
    /// this session.
    bt_cleanup_owned: qt_method!(fn(&mut self)),
    /// Periodic tick from QML — checks idle threshold and runs
    /// bt_cleanup_owned if exceeded.
    tick_bt_respect_idle: qt_method!(fn(&mut self)),
    /// True iff the BT adapter is currently powered ON (last we
    /// checked via `bluetoothctl show`). Polled by a periodic Timer
    /// in QML so the picker can show a "BT is off" hint and offer
    /// the power-on prompt with confirm-unlock delay.
    bt_system_powered: qt_property!(bool; NOTIFY bt_respect_changed),
    /// Power the system BT adapter on or off. Async — the result
    /// flows back via the next bluetoothctl show poll. Called from
    /// the BtSinkPicker's "Turn on Bluetooth" button after the
    /// 1-second confirm-unlock countdown.
    set_bt_system_powered: qt_method!(fn(&mut self, on: bool)),
    /// One-shot poll of `bluetoothctl show` to refresh
    /// `bt_system_powered`. QML calls this every few seconds from a
    /// Timer.
    refresh_bt_system_powered: qt_method!(fn(&mut self)),
    /// Per-MAC Wake-on-LAN throttler. Fires magic packets at predicted
    /// sleeping devices with rate limiting so we don't flood the
    /// broadcast domain.
    wol_throttler: wake_on_lan::WakeThrottler,
    /// JSON array of currently-managed devices, surfaced for QML so the
    /// settings page can render the list. Each entry:
    /// `{ "mac": "AA:BB:...", "name": "Grace", "auto_connect": bool,
    ///   "status": "connected" | "connecting" | "idle" | "backoff" }`.
    bt_devices_json: qt_property!(String; NOTIFY bt_devices_changed),
    bt_devices_changed: qt_signal!(),
    /// Cross-thread result channel for completed bluetoothctl runs.
    /// Sender is cloned into each spawned thread; receiver is polled
    /// from `apply_result_messages` to feed outcomes back to the
    /// scheduler. Initialized lazily on first tick.
    bt_result_tx: Option<Sender<BtAttemptResult>>,
    bt_result_rx: Option<Receiver<BtAttemptResult>>,
    /// Channel for results of the startup BT snapshot — each detached
    /// `bluetoothctl info <mac>` thread sends back its
    /// `(mac, name, was_connected)` triple. None outside the
    /// snapshot window.
    bt_snapshot_rx: Option<Receiver<(String, String, bool)>>,
    /// How many snapshot results we're still waiting for. Once this
    /// hits 0 we mark `bt_respect.snapshot_taken = true`.
    bt_snapshot_pending: usize,
    /// Watchdog counter for "active driver, sink unreachable". When
    /// DriverGetVolume returns None while audio is supposedly playing,
    /// this increments; on Some(_) it resets. At 2 strikes we
    /// conclude the sink genuinely disappeared and stop playback —
    /// the real fix for the "BT disconnect made audio jump to laptop
    /// speakers" bug. Sticky `i32` rather than `bool` so we tolerate
    /// transient single-poll failures (PA briefly busy, etc.).
    sink_unreachable_strikes: i32,
    /// Outlier-staging slot for driver volume sync. When pactl
    /// returns a value >50pt away from the current slider, we don't
    /// adopt immediately — we record it here and only adopt on the
    /// next poll if the value persists. Filters the typical "21 →
    /// 100" stale-read blips during BT sink renegotiation.
    /// Cleared on any in-range adoption.
    driver_volume_pending: Option<i32>,
    /// When the user last set the volume via set_volume_api. While this
    /// is recent (within the grace window), DriverVolume reports from
    /// pactl are NOT allowed to move the slider — the BT speaker often
    /// echoes back a value 1-3 points off what we just set, and without
    /// this guard the slider oscillates (the "16→18, 16→14" fight seen
    /// in the logs). User intent wins until the dust settles.
    last_user_volume_at: Option<std::time::Instant>,
    /// URLs of devices known not to support push notifications.
    /// Populated from ResultMessage::NotifyUnsupported events and
    /// persisted via app_kv (`no_notify_devices` JSON array). On
    /// subsequent app launches, devices in this set skip the
    /// 5-cycle ~7-second discovery probe entirely. Cleared if the
    /// user explicitly re-discovers (clearing the cache is on the
    /// roadmap; for now editing app_state.db is the escape hatch).
    no_notify_devices: HashSet<String>,
    /// Has the `no_notify_devices` KV load completed since launch?
    /// While false, we DEFER device_notify subscription spawns —
    /// otherwise we'd subscribe BEFORE knowing which devices to skip,
    /// re-probing known-bad firmware every session (the very thing
    /// the persistence was meant to fix). Drained from
    /// `pending_notify_subscriptions` when this flips true.
    no_notify_loaded: bool,
    /// URLs we wanted to subscribe to before the blocklist finished
    /// loading. HashSet rather than Vec so duplicate refresh_all
    /// calls don't queue the same URL twice.
    pending_notify_subscriptions: HashSet<String>,
    /// Channel for async `bluetoothctl show` results. Each entry is
    /// `(powered, is_initial_snapshot)`. The is_initial flag tells
    /// the receiver whether to record this in BtRespect's snapshot
    /// (first-time) or just update the live `bt_system_powered`
    /// property (subsequent polls).
    bt_power_rx: Option<Receiver<(bool, bool)>>,

    /// Channel for async `bluetoothctl devices Paired` results — a
    /// list of (mac, name) tuples discovered on the system. The probe
    /// runs in a background thread because bluetoothctl can take a
    /// few hundred ms (or block indefinitely if BlueZ is wedged); we
    /// don't want the GUI to stall on that. Drained from
    /// poll_results into bt_autoconnect.merge_discovered.
    bt_discovery_rx: Option<Receiver<Vec<(String, String)>>>,

    /// QML calls this to flip the toggle on a specific device by MAC.
    set_device_auto_connect: qt_method!(fn(&mut self, mac: String, name: String, enabled: bool)),
    /// Periodic tick (every 10 s from QML) — drives the scheduler.
    /// Devices whose backoff has elapsed get a connect attempt; we
    /// spawn `bluetoothctl connect <MAC>` and report back.
    tick_bt_autoconnect: qt_method!(fn(&mut self)),
    /// Scan KDE's paired BT device list (via `bluetoothctl devices
    /// Paired`) and merge any found devices into the auto-connect
    /// table as disabled-by-default. Lets the user enable the toggle
    /// without having to know the MAC up-front.
    discover_bt_devices: qt_method!(fn(&mut self)),
    /// Disconnect a Bluetooth device immediately. Doesn't unpair —
    /// the device stays in KDE's list, just drops the active session.
    disconnect_bt_device: qt_method!(fn(&mut self, mac: String)),
    /// Forget a Bluetooth device — unpairs it AND removes it from our
    /// auto-connect tracking. The KDE-side `bluetoothctl remove <mac>`
    /// is the actual unpair; the local table also drops the entry.
    forget_bt_device: qt_method!(fn(&mut self, mac: String)),

    // ── KDE/system tools audit ────────────────────────────────────────
    //
    // Detects which command-line utilities the controller depends on
    // are present + offers install/build-from-source recipes for the
    // missing ones. Surfaced in the settings UI.

    /// JSON array of {bin, purpose, used_by, apt_package, source_url,
    /// source_script, status, status_detail}. Filled by `audit_kde_tools`.
    kde_tools_json: qt_property!(String; NOTIFY kde_tools_changed),
    /// Pre-built apt install command for any missing tools. Empty when
    /// nothing's missing. The user is meant to review + paste, not
    /// have the app run sudo on their behalf.
    kde_tools_install_cmd: qt_property!(String; NOTIFY kde_tools_changed),
    kde_tools_changed: qt_signal!(),

    /// Run the tool audit and refresh the qt_properties. Called from
    /// QML when the settings page is opened.
    audit_kde_tools: qt_method!(fn(&mut self)),

    // ── System dependencies audit (apt/dnf/pacman/apk/zypper) ───────
    //
    // Surfaces what the dep_wrangler knows about the host's package
    // manager and required runtime tools. The UI shows a list with
    // status (✓ installed / ✗ missing) and, for missing tools, the
    // exact command the user can copy + paste. We don't run sudo
    // from the GUI — that's a security boundary best left to the
    // shell.

    /// Detected package manager label ("Alpine (apk)", "Debian/Ubuntu (apt)", …)
    /// or "unknown" when /etc/os-release doesn't match anything.
    dep_pm_label: qt_property!(String; NOTIFY dep_audit_changed),
    /// JSON array of {binary, friendly_name, installed, plan_text}.
    /// `plan_text` is the human-readable install plan (empty when
    /// already installed).
    dep_audit_json: qt_property!(String; NOTIFY dep_audit_changed),
    dep_audit_changed: qt_signal!(),

    /// Re-run the audit. Quick (just stats files on PATH); safe to
    /// call on every page open.
    refresh_dep_audit: qt_method!(fn(&mut self)),

    // ── Active source state (Mozart-specific) ───────────────────────
    //
    // What's the device currently playing FROM? Mozart firmware only
    // accepts arbitrary URI playback when the active source is
    // Network / DLNA / Music. Knowing the source ahead of time lets
    // us (a) show it as a chip in the now-playing bar and (b) warn
    // the user proactively when they tap play to a source that won't
    // accept it. Polled periodically via a background task.

    /// Stable source id (e.g. "spotify", "network", "linein"). Empty
    /// when unknown.
    device_source_id: qt_property!(String; NOTIFY device_source_changed),
    /// Human-readable label (e.g. "Spotify"). May equal id on minimal
    /// firmware responses.
    device_source_label: qt_property!(String; NOTIFY device_source_changed),
    /// One of: "network_dlna", "spotify", "airplay", "bluetooth",
    /// "linein", "tv", "internet_radio", "other_streaming", "unknown".
    /// QML maps this to the chip color + warning state.
    device_source_family: qt_property!(String; NOTIFY device_source_changed),
    /// True when the active source accepts DLNA URI playback.
    device_source_can_play: qt_property!(bool; NOTIFY device_source_changed),
    /// Human-readable explanation when can_play is false. Drives a
    /// toast on play attempts.
    device_source_block_reason: qt_property!(String; NOTIFY device_source_changed),
    device_source_changed: qt_signal!(),

    /// Trigger an immediate source-state re-probe. Called from QML
    /// when the user switches devices or hits the refresh icon.
    refresh_active_source: qt_method!(fn(&mut self)),

    /// Try to programmatically switch the active device's source to
    /// Network/DLNA so URI playback works. Best-effort: probes
    /// several candidate REST paths; some Mozart firmware versions
    /// accept one, others lock source switching to physical buttons.
    /// Result lands in network_switch_* properties so the UI can
    /// show what happened.
    try_activate_network: qt_method!(fn(&mut self)),
    /// Last attempt's outcome JSON (used_endpoint, attempts list,
    /// success bool). Empty before the first attempt.
    network_switch_json:    qt_property!(String; NOTIFY network_switch_changed),
    /// True while a switch attempt is in flight.
    network_switch_running: qt_property!(bool;   NOTIFY network_switch_changed),
    network_switch_changed: qt_signal!(),

    // ── Firmware family detection ─────────────────────────────────
    //
    // The driver layer this controller is going to grow needs to
    // know what kind of device it's talking to. "Beosound" is a
    // marketing name covering at least three distinct firmware
    // generations (Mozart, Beolink Converse, older ASE-based), and
    // each one accepts a different REST API. Probing on device-add
    // tells us which generation we're working with so the UI shows
    // appropriate fallback options instead of pretending every
    // device is a Mozart and silently failing.
    //
    // device_firmware_family: serialized tag — "mozart" / "beolink" /
    //   "mozart_legacy" / "unknown" / "unreachable".
    // device_firmware_label:  human label for the chip.
    // device_firmware_diagnostic_json: full FirmwareProbe blob
    //   (reasoning + body snippet) for the diagnostic UI.
    device_firmware_family: qt_property!(String; NOTIFY device_firmware_changed),
    device_firmware_label:  qt_property!(String; NOTIFY device_firmware_changed),
    device_firmware_diagnostic_json: qt_property!(String; NOTIFY device_firmware_changed),
    /// True iff the detected family supports REST URI playback
    /// (only Mozart). Derived from the family tag for cheap QML
    /// access without parsing the JSON.
    device_supports_rest_playback: qt_property!(bool; NOTIFY device_firmware_changed),
    /// True iff the UI should recommend Bluetooth as the audio path
    /// (everything except healthy Mozart).
    device_recommends_bluetooth: qt_property!(bool; NOTIFY device_firmware_changed),
    device_firmware_changed: qt_signal!(),

    /// Re-probe the active device's firmware family. Used when the
    /// user wants to refresh the classification (e.g. after a
    /// firmware update on the device).
    refresh_firmware_family: qt_method!(fn(&mut self)),

    // ── Driver layer (Mozart vs Bluetooth vs Noop) ────────────────
    //
    // Human label describing the current driver, e.g.
    //   "Mozart REST @ http://192.168.2.12"
    //   "Bluetooth → Beosound Emerge (00:09:A7:77:C2:88)"
    //   "Noop (device firmware (BeolinkConverse) doesn't speak REST...)"
    // Empty until the first firmware probe completes.
    current_driver_label: qt_property!(String; NOTIFY driver_changed),
    /// "mozart" | "bluetooth" | "noop" — short tag for QML logic.
    current_driver_kind:  qt_property!(String; NOTIFY driver_changed),
    driver_changed: qt_signal!(),

    /// Associate a Bluetooth MAC with the active device. After this,
    /// the driver layer will pick BluetoothDriver for non-Mozart
    /// firmware and route playback via paplay rather than REST.
    /// Pass an empty string to clear the association.
    set_active_bt_mac: qt_method!(fn(&mut self, mac: String, friendly_name: String)),

    /// Bind a multi-room session: combine 2+ BT speakers into a
    /// virtual PulseAudio sink, swap the current driver to that sink.
    /// `macs_json` and `names_json` are JSON arrays of strings with
    /// matched indexing (macs_json[i] pairs with names_json[i]).
    /// Calling with <2 MACs is an error — for a single speaker use
    /// set_active_bt_mac instead.
    set_multiroom_macs: qt_method!(fn(&mut self, macs_json: String, names_json: String)),
    /// Tear down any active multi-room session: unload the
    /// PulseAudio combine-sink module, rebuild the driver as if
    /// no BT MAC were bound.
    clear_multiroom: qt_method!(fn(&mut self)),
    /// JSON of `{macs: [...], names: [...]}` for the currently-active
    /// multi-room session, or `null` if none. The picker reads this
    /// to highlight which speakers are part of the active set.
    active_multiroom_json: qt_property!(String; NOTIFY driver_changed),

    /// Play a local file via the current driver. For Mozart-active
    /// devices this delegates to the existing REST/DLNA pipeline
    /// (wraps the file via track_server first). For Bluetooth-active
    /// devices this paplays straight to the BT sink. Errors surface
    /// in `driver_last_error`.
    driver_play_local_file: qt_method!(fn(&mut self, path: String, title: String)),
    /// Stop any currently-playing audio on the current driver.
    driver_stop: qt_method!(fn(&mut self)),
    /// Set volume 0-100 via the current driver. For Mozart this
    /// falls through to the existing Command::SetVolume path so the
    /// usual safety clamping still applies; for Bluetooth this calls
    /// pactl set-sink-volume on the BT sink directly.
    driver_set_volume_pct: qt_method!(fn(&mut self, pct: i32)),
    /// Last driver operation error, empty when the last call succeeded.
    driver_last_error: qt_property!(String; NOTIFY driver_changed),

    /// Currently-associated Bluetooth MAC for the active device's
    /// audio route. Empty when no BT route is bound. Read-only from
    /// QML; mutated via `set_active_bt_mac`.
    active_bt_mac:  qt_property!(String; NOTIFY driver_changed),
    /// Friendly name of the currently-associated Bluetooth device.
    /// Empty when no BT route is bound. Same notify as active_bt_mac.
    active_bt_name: qt_property!(String; NOTIFY driver_changed),

    // ── MPRIS context (what's playing in other apps on this machine) ──
    //
    // Pulled from `playerctl` once every ~30 s. Surfaces in the UI as
    // an optional "currently playing in <player>: <title>" hint; also
    // feeds the anticipator so predictions can take "user is already
    // listening to music in another app" into account.

    /// Player name from MPRIS (e.g. "spotify", "firefox"). Empty when
    /// nothing's playing.
    mpris_player: qt_property!(String; NOTIFY mpris_changed),
    /// Currently-playing track title.
    mpris_title: qt_property!(String; NOTIFY mpris_changed),
    /// Currently-playing artist.
    mpris_artist: qt_property!(String; NOTIFY mpris_changed),
    /// True when an active MPRIS-controlled player is playing.
    mpris_playing: qt_property!(bool; NOTIFY mpris_changed),
    mpris_changed: qt_signal!(),

    /// Refresh MPRIS state by shelling out to `playerctl`. Cheap
    /// (~10-30 ms when nothing's playing). Driven by the anticipator
    /// tick so it runs on the same cadence as other context refreshes.
    refresh_mpris: qt_method!(fn(&mut self)),

    /// Run a self-test on the playback pipeline. Reports which layer
    /// is healthy via the play status banner. Used when the user
    /// suspects playback isn't working — they can tap "Diagnose" and
    /// the result tells them whether the track server, the URL build,
    /// or the device API is the broken bit.
    diagnose_playback: qt_method!(fn(&mut self)),

    // Now-playing key/BPM HUD (only meaningful with --features analysis).
    np_bpm: qt_property!(f64; NOTIFY np_key_changed),
    np_camelot: qt_property!(String; NOTIFY np_key_changed),
    np_key_text: qt_property!(String; NOTIFY np_key_changed),
    np_key_changed: qt_signal!(),

    dark_mode: qt_property!(bool; NOTIFY dark_mode_changed),
    dark_mode_changed: qt_signal!(),

    // Internal channels for communicating with background thread
    tx: Option<Sender<Command>>,
    rx: Option<Receiver<ResultMessage>>,

    // Channels to the dedicated media-library worker thread (owns SQLite).
    media_tx: Option<Sender<media::MediaCmd>>,
    media_rx: Option<Receiver<media::MediaResult>>,

    // Volume to restore when un-muting. The Stage's BeoSound:/muted path
    // is not whitelisted (it 500s), so mute is implemented as volume->0
    // with restore of this remembered level.
    prev_volume: i32,

    // Realtime stream mixer (Mid/Side EQ + per-channel volume). Held
    // behind an Arc<RwLock> so an audio thread can acquire it
    // independently of the UI thread when streaming is wired up. For
    // now the UI thread is the only consumer; reads/writes are quick
    // enough that an RwLock is more than sufficient (vs the
    // lock-free triple-buffer pattern we'd want for hard-realtime).
    mixer: Arc<RwLock<realtime_mixer::RealtimeMixer>>,

    // "Mix & Play" continuation state. When the user clicks the
    // Mix & Play button, mix_and_play_pending is set to true and the
    // file processor is dispatched; on MixerProcessDone, the handler
    // checks this flag and, if true, exposes the output via
    // track_server + dispatches PlayResolvedUrl. Cleared on either
    // success or failure so a subsequent plain "Run" doesn't auto-
    // play unexpectedly.
    mix_and_play_pending: bool,

    // ── Current device driver ─────────────────────────────────────
    //
    // Per-active-device driver instance picked from the detected
    // firmware family. Updated whenever:
    //   * The user switches devices (select_device / select_device_with_product)
    //   * A firmware probe completes with a new family
    //   * A Bluetooth MAC is associated with a non-Mozart device
    //
    // QML doesn't see this directly; instead it reads
    // `current_driver_label` (a string description) and calls the
    // various `driver_*` qt_methods which dispatch through the
    // wrapped AnyDriver.
    // Note: active_bt_mac/active_bt_name are declared above as
    // qt_property! so QML can read them (BtSinkPicker shows which
    // device is currently bound). They're set by set_active_bt_mac
    // and cleared on driver reset.
    current_driver: Option<device_driver::AnyDriver>,
    /// Track ID of the track most recently dispatched to the driver
    /// for playback. Used by `advance_queue_if_any` to find the
    /// just-ended track in the queue and pick the successor.
    last_played_track_id: i64,
    /// Concurrent-playback layer model (queue_policy Synchronize/MidgapLayer
    /// modes drive this). Pure bookkeeping; the driver mirrors its actions
    /// into real decode pipes. Defaults to empty (single-player path).
    layer_set: layer_player::LayerSet,
    last_played_path: String,
    /// Counter for the child-mode skip loop. Increments on each
    /// blacklisted track in a row; resets to 0 when we successfully
    /// advance to a clean one. Stops the recursion at 100 so a
    /// queue full of blacklisted tracks doesn't blow the stack.
    /// (Reset on every successful play_track or queue change.)
    child_skip_depth: u32,

    // ── Playback modes (repeat + child filter) ───────────────────
    //
    // Repeat mode: Off/One/All cycled via repeat_mode_cycle() from
    // QML. Off is the default; the user picks One when they want
    // a single song on loop, All when they want the queue to wrap.
    //
    // Child mode: a content filter that skips tracks whose title or
    // filename matches a substantial profanity / drug / sexual /
    // violent / slur blacklist (see playback_modes.rs). When on,
    // the queue auto-advance skips matched tracks and logs the
    // reason. The filter is intentionally aggressive — false
    // positives are recoverable (user disables it), false negatives
    // leak inappropriate content to a child and aren't.
    repeat_mode_tag: qt_property!(String; NOTIFY playback_modes_changed),
    child_mode_on:   qt_property!(bool;   NOTIFY playback_modes_changed),
    playback_modes_changed: qt_signal!(),
    repeat_mode_cycle: qt_method!(fn(&mut self)),
    set_child_mode:    qt_method!(fn(&mut self, on: bool)),

    // ── Multiroom / Soundscape intent dispatch ───────────────────
    //
    // These are SCAFFOLDING for the user's "ask to create and play"
    // UX. They emit a tagged intent (JSON) into intent_payload that
    // the QML side can subscribe to and render as a confirmation
    // sheet ("Play 'Spotify' across [Stage + Smokey + Emerge01]?").
    //
    // The actual multiroom + soundscape creation are already
    // implemented elsewhere (mux_bridge + the multiroom: lines in
    // the user's recent logs). What we add here is the INTENT
    // surface: the QML side sends voice/button "create multiroom"
    // → controller validates available speakers → emits intent
    // with a concrete payload → QML renders confirmation → user
    // taps confirm → controller proceeds. Without this layer,
    // create/play happens silently with no per-step undo.
    intent_payload: qt_property!(String; NOTIFY intent_changed),
    intent_changed: qt_signal!(),
    propose_multiroom_play: qt_method!(fn(&mut self, track_id: i32)),
    propose_soundscape_play: qt_method!(fn(&mut self, scene_name: String)),

    // ── Lyrics transcription ─────────────────────────────────────
    //
    // Async transcription via whisper-cli (see lyrics_transcriber.rs).
    // Triggered from QML for the currently-playing track; result
    // arrives in lyrics_text after the worker thread finishes.
    lyrics_text:   qt_property!(String; NOTIFY lyrics_changed),
    lyrics_status: qt_property!(String; NOTIFY lyrics_changed),
    lyrics_changed: qt_signal!(),
    /// Transcribe lyrics for the currently-loaded track. Template
    /// is "lrc" / "markdown" / "plain". Result populates lyrics_text
    /// once the worker completes.
    transcribe_lyrics: qt_method!(fn(&mut self, template: String)),
    /// QML-facing view of the same value. Bottom transport bar reads
    /// this to know which track to replay when the user taps play
    /// on a non-Mozart driver (there's no "resume paused" concept on
    /// paplay; we just play the same file again).
    last_played_track_id_qml: qt_property!(i32; NOTIFY play_status_changed),
}

impl QtState {
    // helper for product visual and name
    fn set_product_data(&mut self, data: &serde_json::Value) {
        let json_str = serde_json::to_string(data).expect("Failed to serialize JSON");
        self.products = json_str;
        self.productsChanged(); // emit signal to QML
    }

    /// Alternate loader: set the products JSON from an already-serialized
    /// string. Used by tests or out-of-band loaders; not invoked by the
    /// regular `set_products_from_file` path.
    #[allow(dead_code)]
    fn set_product_data_ser(&mut self, ser_data: String) {
        // TODO validate
        self.products = ser_data;
        self.productsChanged(); // emit signal to QML
    }


    /// Called by main() right after creating the object to attach channels.
    fn attach_channels(&mut self, tx: Sender<Command>, rx: Receiver<ResultMessage>) {
        self.tx = Some(tx);
        self.rx = Some(rx);
    }

    /// Attach the media-worker channels (separate thread, owns SQLite).
    fn attach_media_channels(&mut self, tx: Sender<media::MediaCmd>, rx: Receiver<media::MediaResult>) {
        self.media_tx = Some(tx);
        self.media_rx = Some(rx);
    }

    /// Helper: get active url as Option<String>. Returns None for empty
    /// OR malformed URLs — reqwest's URL builder produces unhelpful
    /// "invalid port number" errors when given something like "http://"
    /// or a discovered device entry without a port that the builder
    /// can't normalize, so we skip those eagerly.
    fn active_url_opt(&self) -> Option<String> {
        let u = self.active_url.trim();
        if u.is_empty() { return None; }
        // Minimal validity check: must start with http:// or https:// and
        // have at least one non-empty path segment after the scheme.
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return None;
        }
        let after_scheme = u.split("://").nth(1).unwrap_or("");
        // Reject a host portion that's empty or just a colon (e.g. "http://"
        // or "http://:").
        let host_part = after_scheme.split('/').next().unwrap_or("");
        if host_part.is_empty() || host_part == ":" || host_part.starts_with(':') {
            return None;
        }
        // If the port portion (after ':') is present, it must be all digits.
        if let Some(port_str) = host_part.rsplit_once(':').map(|(_, p)| p) {
            if !port_str.is_empty() && !port_str.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
        }
        Some(u.to_string())
    }

    /// Called from QML (or Rust) to ask background thread to GET volume.
    fn refresh_volume(&mut self) {
        let url = match self.active_url_opt() {
            Some(u) => u,
            None => {
                eprintln!("refresh_volume: no active_url set");
                return;
            }
        };
        // Route the poll based on driver kind:
        //   • Non-Mozart (local / bluetooth / multiroom) → poll the
        //     actual playback sink via pactl. The Stage's REST volume
        //     is meaningless here (no audio flows through it) and
        //     polling it just produces log noise — Emerges return
        //     `errorCode/errorId/errorMessage: Not found` on every
        //     hit, which was filling the user's terminal at one line
        //     per 3 seconds.
        //   • Mozart (or unknown) → existing REST GetVolume path.
        //     The device's own volume IS what the user hears.
        let kind = self.current_driver_kind.clone();
        let non_mozart = (kind == "local" || kind == "bluetooth" || kind == "multiroom")
                         && self.current_driver.is_some();

        if non_mozart {
            if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                let _ = tx.send(Command::DriverGetVolume {
                    driver: drv.clone(),
                });
            }
            // Don't also fire REST GetVolume — the device either
            // doesn't have a useful one for us (Emerge), or has one
            // that's decoupled from what's actually playing (Stage
            // when audio routes through PA). Either way, the result
            // would just be log noise.
            return;
        }
        if let Some(tx) = &self.tx {
            self.last_command_at = Some(std::time::Instant::now());
            let _ = tx.send(Command::GetVolume { url });
        } else {
            eprintln!("refresh_volume: no tx channel attached");
        }
    }

    /// Update the aggression policy. Cascades to all pre-staging
    /// subsystems via the AggressionPolicy methods. Persisted to the
    /// app_state KV so the choice survives restarts.
    fn set_aggression(&mut self, level: i32) {
        let new_policy = aggression::AggressionPolicy::from_level(level);
        if new_policy == self.aggression { return; }
        eprintln!("aggression: {} → {}",
                  self.aggression.label(), new_policy.label());
        self.aggression = new_policy;
        self.aggression_level = new_policy.level() as i32;
        self.aggression_label = new_policy.label().to_string();
        self.aggression_changed();
        // Persist the level via the generic KV channel.
        self.send_media(media::MediaCmd::SaveAppKv {
            key: "aggression_level".to_string(),
            value: new_policy.level().to_string(),
        });
    }

    /// Gesture-driven volume setter. The dial calls this with its raw
    /// target value; we route through GestureFilter to reject wraparound
    /// artifacts and cap per-event step size, then hand off to the same
    /// safety + dispatch path as direct setters. The gesture filter
    /// guards rate-of-change; the safety system guards absolute values —
    /// both apply in sequence.
    fn set_volume_from_gesture(&mut self, target: i32) {
        let now = std::time::Instant::now();
        let filtered = self.volume_gesture.submit(target, now);
        // Route through set_volume_api for the existing safety clamp +
        // Command::SetVolume dispatch. If the filter caps the gesture,
        // set_volume_api still sees a valid integer target and the rest
        // of the pipeline proceeds normally.
        self.set_volume_api(filtered);
    }

    /// Called from QML when slider is moved — updates UI immediately and sends command.
    fn set_volume_api(&mut self, v: i32) {
        // Any explicit volume gesture counts as user activity — keeps
        // the BT idle-cleanup timer from firing during interactive use.
        self.bt_respect.note_activity();
        // Mark that the user just set the volume. DriverVolume sync
        // backs off for a grace window after this so the BT speaker's
        // slightly-off echo can't fight the slider.
        self.last_user_volume_at = Some(std::time::Instant::now());

        // ── Attack-surface defenses ──
        //
        // These run BEFORE clamp_for_safety because they're protecting
        // against patterns that the per-product safety policy can't
        // see: rapid-fire volume floods + absolute hard ceiling
        // bypasses. The rate limit drops requests entirely during a
        // lockout (so a flood from a compromised path can't ramp
        // the volume); the hard ceiling caps even the unlock-
        // override path so no software route gets above 95% no
        // matter what.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let rate_outcome = self.volume_rate_limiter.gate(now_ms);
        use volume_safety::RateLimitOutcome;
        match rate_outcome {
            RateLimitOutcome::Pass => {}
            RateLimitOutcome::Tripped { lockout_ms } => {
                eprintln!(
                    "volume_safety: RATE-LIMIT TRIPPED — {} changes in {} ms; \
                     locking out further volume changes for {} ms. \
                     Possible attack or stuck slider.",
                    volume_safety::RATE_LIMIT_MAX_CHANGES,
                    volume_safety::RATE_LIMIT_WINDOW_MS,
                    lockout_ms);
                self.last_play_status = "Volume rate limit triggered — please wait".into();
                self.play_status_changed();
                return;
            }
            RateLimitOutcome::Locked { remaining_ms } => {
                eprintln!(
                    "volume_safety: dropping volume change during lockout \
                     ({} ms remaining)", remaining_ms);
                return;
            }
        }
        // Hard absolute ceiling: applies before the per-product clamp
        // and before the unlock override. The user's physical knob on
        // the speaker remains; only the software set_volume path is
        // capped here.
        let v = volume_safety::enforce_hard_ceiling(v);

        // Apply per-product safety policy first. Manual requests above
        // the unsafe threshold get clamped down unless the user has
        // explicitly unlocked high-volume mode for this session.
        let pid = self.active_product_id.clone();
        let unlocked = self.volume_high_unlocked;
        let (actual, reason) = volume_safety::clamp_for_safety(&pid, v, unlocked);

        use volume_safety::ClampReason;
        match reason {
            ClampReason::Clamped { from, to } => {
                eprintln!(
                    "volume_safety: manual request {} exceeds unsafe threshold; \
                     clamped to {} (≈{:.0} dB) on {}. Call enable_high_volume() to override.",
                    from, to, volume_safety::approx_db_at(&pid, to), self.active_product_id,
                );
                // Stash the requested value so QML can offer "unlock and retry".
                self.volume_pending_request = from;
                self.volume_was_clamped = true;
                self.volume_clamped_changed();
            }
            ClampReason::Loud => {
                // Pass through but mark the loud band so the UI can warn.
            }
            ClampReason::NotClamped => {}
        }

        let clamped = actual;
        self.volume = clamped;
        self.volume_is_loud = clamped > volume_safety::loud_threshold(&pid);
        self.volume_changed();

        // ── Persistent safety acknowledgement ──
        //
        // If the user actively sets the volume above the startup
        // safety ceiling (the level that would have triggered the
        // "clamping (unknown product) from X -> Y" log on next
        // startup), record an ack against this device URL so the
        // next session doesn't yank them back down. This is the
        // explicit-consent moment: the user touched the slider and
        // landed in the loud band, so respecting that on subsequent
        // restarts is what they want.
        //
        // We don't ack on the `volume_high_unlocked` override path
        // alone — that's session-only and the user might have
        // unlocked just to play one loud track. The persistent ack
        // requires actually committing a loud volume value.
        let startup_ceiling = volume_safety::safe_startup_volume(&pid);
        if clamped > startup_ceiling {
            volume_safety_ack::ack(&self.active_url);
        }

        // A manual volume change clears the mute state (you're clearly
        // listening again). Remember the level for the next mute.
        if clamped > 0 && self.muted {
            self.muted = false;
            self.muted_changed();
        }
        if clamped > 0 { self.prev_volume = clamped; }

        let url = match self.active_url_opt() {
            Some(u) => u,
            None => {
                eprintln!("set_volume_api: no active_url set");
                return;
            }
        };
        // Route based on driver kind. The Stage at 192.168.2.12 is
        // Mozart-control-only (REST setData works) but cannot output
        // audio for arbitrary URIs. When the user is sending audio
        // through a BT speaker / Local sink / MultiRoom combine sink,
        // the device's REST volume isn't what they hear — the PA sink
        // volume is. So:
        //
        //   * Mozart driver (or no driver bound): set the device's
        //     REST volume. This is the legacy path; matches what
        //     "set volume on the speaker" has always meant.
        //   * Bluetooth/Local/MultiRoom driver: route to the driver's
        //     own set_volume_percent via Command::DriverOp. This
        //     uses pactl set-sink-volume on the actual playback sink,
        //     which is what the user actually hears.
        //
        // We don't double-set on non-Mozart drivers — the device's
        // REST volume is irrelevant when audio doesn't flow through
        // the device. Setting both would just create confusion.
        let kind = self.current_driver_kind.as_str();
        if kind == "mozart" || kind.is_empty() {
            if let Some(tx) = &self.tx {
                let _ = tx.send(Command::SetVolume { url, value: clamped });
            } else {
                eprintln!("set_volume_api: no tx channel attached");
            }
        } else {
            // Non-Mozart driver: dispatch to the driver's pactl path.
            if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                let pct = clamped.clamp(0, 100) as u8;
                eprintln!("set_volume_api: routing {}% to {} driver",
                          pct, kind);
                let _ = tx.send(Command::DriverOp {
                    driver: drv.clone(),
                    op: DriverOpKind::SetVolumePct(pct),
                });
            } else {
                eprintln!("set_volume_api: no driver bound for {} kind", kind);
            }
        }
    }

    /// Inline per-device volume from the playbar strip. Volume can only be
    /// safely routed through the active device's driver (with that
    /// device's product-specific safety ceiling). So:
    ///   * url == active: set directly via the normal safe path.
    ///   * url != active: this device has no bound driver here, and we
    ///     don't know its product ceiling — promote it to active first
    ///     (one tap selects + sets), then apply on the next interaction.
    ///     We DON'T blindly fire volume at an unselected device, which
    ///     could hit the wrong sink or bypass its ceiling.
    /// Either way the value still passes through set_volume_api's safety,
    /// rate-limit and clamp logic — this is a thin pre-router, not a
    /// bypass.
    fn set_device_volume(&mut self, url: String, v: i32) {
        if url.is_empty() { return; }
        if url == self.active_url {
            self.set_volume_api(v);
        } else {
            // Promote to active so the right driver + ceiling apply, then
            // the user's next nudge (or this same value, re-sent by QML
            // after the select settles) lands on the correct device. We
            // intentionally do NOT set volume in the same call: selecting
            // rebuilds the driver asynchronously, and firing volume before
            // that completes would race the driver rebuild.
            self.select_device(url);
            self.last_play_status =
                "Tap volume again — switched to this speaker first".to_string();
            self.play_status_changed();
        }
    }

    /// Unlock the unsafe-volume ceiling for the current session. Future
    /// volume requests above ≈95 dB will be sent to the device. Resets
    /// to locked when the user picks a different device.
    fn enable_high_volume(&mut self) {
        if !self.volume_high_unlocked {
            self.volume_high_unlocked = true;
            self.volume_high_unlocked_changed();
            eprintln!("volume_safety: high-volume mode UNLOCKED on {}", self.active_product_id);
        }
    }

    /// Re-lock the unsafe-volume ceiling.
    fn disable_high_volume(&mut self) {
        if self.volume_high_unlocked {
            self.volume_high_unlocked = false;
            self.volume_high_unlocked_changed();
            eprintln!("volume_safety: high-volume mode LOCKED on {}", self.active_product_id);
        }
    }

    /// Called by the UI after the user dismisses the "volume was clamped"
    /// toast. Clears the flag and the pending request.
    fn dismiss_clamp_warning(&mut self) {
        self.volume_was_clamped = false;
        self.volume_pending_request = 0;
        self.volume_clamped_changed();
    }

    // ── Frequency sanitizer ──────────────────────────────────────────

    fn start_sanitizer(&mut self, preset_id: String, freq_hz: f64, gain_db: f64) {
        // Map id → preset enum. Unknown ids fall back to pink.
        let preset = masker::MaskerPreset::from_id(&preset_id)
            .unwrap_or(masker::MaskerPreset::Pink);
        let anti = if freq_hz > 0.0 { Some(freq_hz as f32) } else { None };
        // Drop the previous driver first (clean stop) before starting a new one.
        self.sanitizer_handle = None;
        match sanitizer_driver::SanitizerDriver::start(preset, anti, gain_db as f32) {
            Ok(h) => {
                eprintln!(
                    "sanitizer: started preset={} freq={:.1} gain={:.1} dB via {}",
                    preset_id, freq_hz, gain_db, h.player_name(),
                );
                self.sanitizer_handle = Some(Box::new(h));
                self.sanitizer_running = true;
                self.sanitizer_preset  = preset_id;
                self.sanitizer_freq    = freq_hz;
                self.sanitizer_error   = String::new();
            }
            Err(e) => {
                eprintln!("sanitizer: failed to start: {}", e);
                self.sanitizer_running = false;
                self.sanitizer_error = e;
            }
        }
        self.sanitizer_changed();
    }

    fn stop_sanitizer(&mut self) {
        if self.sanitizer_handle.is_some() {
            // Stop the adaptive runner first — it needs the sanitizer's
            // predictor state and writing to a dropped one is a no-op
            // but cleaner to tear down in order.
            if self.pattern_runner.is_some() {
                self.pattern_runner = None;
                self.adaptive_active = false;
                self.adaptive_detected_hz = 0.0;
                self.adaptive_score = 0.0;
                self.adaptive_backend = String::new();
                self.adaptive_changed();
            }
            self.sanitizer_handle = None;
            self.sanitizer_running = false;
            self.sanitizer_preset  = String::new();
            self.sanitizer_freq    = 0.0;
            self.sanitizer_changed();
            eprintln!("sanitizer: stopped");
        }
    }

    fn retune_sanitizer(&mut self, freq_hz: f64) {
        if let Some(h) = self.sanitizer_handle.as_ref() {
            h.retune(freq_hz as f32);
            self.sanitizer_freq = freq_hz;
            self.sanitizer_changed();
        }
    }

    // ── Adaptive (predictive) cancellation ──────────────────────────

    fn start_adaptive_cancellation(&mut self) {
        // Adaptive cancellation needs the sanitizer's audio thread + its
        // PredictorState. Without a running sanitizer there's no output
        // path, so refuse and surface a clear message.
        let pstate = match self.sanitizer_handle.as_ref() {
            Some(h) => h.predictor_state.clone(),
            None => {
                self.adaptive_error = "start the sanitizer first".to_string();
                self.adaptive_active = false;
                self.adaptive_changed();
                return;
            }
        };
        // Drop any previous runner so we start clean.
        self.pattern_runner = None;
        match pattern_runner::PatternRunner::start(pstate) {
            Ok(r) => {
                eprintln!("adaptive: started via {} mic backend", r.mic_backend());
                self.adaptive_backend = r.mic_backend().to_string();
                self.adaptive_error = String::new();
                self.adaptive_active = true;
                self.pattern_runner = Some(Box::new(r));
            }
            Err(e) => {
                eprintln!("adaptive: failed to start: {}", e);
                self.adaptive_active = false;
                self.adaptive_error = e;
            }
        }
        self.adaptive_changed();
    }

    fn stop_adaptive_cancellation(&mut self) {
        if self.pattern_runner.is_some() {
            self.pattern_runner = None;  // Drop signals the runner to stop
            self.adaptive_active = false;
            self.adaptive_detected_hz = 0.0;
            self.adaptive_score = 0.0;
            self.adaptive_backend = String::new();
            self.adaptive_changed();
            eprintln!("adaptive: stopped");
        }
    }

    /// QML-polled refresh of adaptive_detected_hz / adaptive_score so the
    /// UI can show a live readout of what the learner is finding.
    fn refresh_adaptive_status(&mut self) {
        let (hz, score) = match self.pattern_runner.as_ref() {
            Some(r) => (r.last_freq_hz() as f64, r.last_score() as f64),
            None => return,
        };
        let changed = (hz - self.adaptive_detected_hz).abs() > 0.05
                   || (score - self.adaptive_score).abs() > 0.005;
        if changed {
            self.adaptive_detected_hz = hz;
            self.adaptive_score = score;
            self.adaptive_changed();
        }
    }

    /// Mute is derived locally (volume==0 with a remembered restore level)
    /// because the device's BeoSound:/muted path isn't whitelisted. This
    /// is kept as a no-op so existing QML callers don't break.
    fn refresh_mute(&mut self) {
        // Intentionally empty: see toggle_mute. Mute state is local.
    }

    /// Toggle mute. The device's BeoSound:/muted path is not whitelisted
    /// (returns 500), so we implement mute as "set volume 0, remember the
    /// prior level" and un-mute as "restore that level". Both use the
    /// setVolume path the device actually accepts. Optimistic UI: flip
    /// state immediately, the authoritative volume confirms via
    /// ResultMessage::Volume.
    fn toggle_mute(&mut self) {
        // Mute/unmute is an explicit user gesture — count as activity.
        self.bt_respect.note_activity();
        let url = match self.active_url_opt() {
            Some(u) => u,
            None => { eprintln!("toggle_mute: no active_url set"); return; }
        };

        let kind = self.current_driver_kind.clone();
        let route_to_driver = !(kind == "mozart" || kind.is_empty());

        if !self.muted {
            // Muting: remember current level (unless already 0), then go to 0.
            if self.volume > 0 { self.prev_volume = self.volume; }
            self.muted = true;
            self.muted_changed();
            self.volume = 0;
            self.volume_changed();
            if route_to_driver {
                if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                    let _ = tx.send(Command::DriverOp {
                        driver: drv.clone(),
                        op: DriverOpKind::SetVolumePct(0),
                    });
                }
            } else if let Some(tx) = &self.tx {
                let _ = tx.send(Command::SetVolume { url, value: 0 });
            }
        } else {
            // Un-muting: restore the remembered level (fall back to a gentle
            // default if we never captured one).
            let restore = if self.prev_volume > 0 { self.prev_volume } else { 20 };
            self.muted = false;
            self.muted_changed();
            self.volume = restore;
            self.volume_changed();
            if route_to_driver {
                if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                    let pct = restore.clamp(0, 100) as u8;
                    let _ = tx.send(Command::DriverOp {
                        driver: drv.clone(),
                        op: DriverOpKind::SetVolumePct(pct),
                    });
                }
            } else if let Some(tx) = &self.tx {
                let _ = tx.send(Command::SetVolume { url, value: restore });
            }
        }
    }

    /// Refresh product info
    fn refresh_product_info(&mut self) {
        let url = match self.active_url_opt() {
            Some(u) => u,
            None => return,
        };
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::GetProductInfo { url });
        }
    }

    // ── Sources ─────────────────────────────────────────────────────
    fn refresh_sources(&mut self) {
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::GetSources { url });
        }
    }
    fn select_source(&mut self, source_id: String) {
        // Optimistic: reflect immediately.
        self.active_source_id = source_id.clone();
        self.sources_changed();
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::SetSource { url, source_id });
        }
    }

    // ── Now playing ─────────────────────────────────────────────────
    fn refresh_now_playing(&mut self) {
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::GetNowPlaying { url });
        }
    }
    fn transport(&mut self, action: String) {
        // Optimistic play/pause reflection for snappy UI.
        match action.as_str() {
            "play"  => { self.np_playing = true;  self.now_playing_changed(); }
            "pause" | "stop" => { self.np_playing = false; self.now_playing_changed(); }
            _ => {}
        }
        // Tell the anticipator what happened — drives FSM transitions
        // and the next round of pre-staging decisions.
        match action.as_str() {
            "play"    => self.observe_action(anticipator::ActionKind::Resume),
            "pause"   => self.observe_action(anticipator::ActionKind::Pause),
            "forward" => self.observe_action(anticipator::ActionKind::SkipNext),
            "back"    => self.observe_action(anticipator::ActionKind::SkipPrev),
            _ => {}
        }
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::Transport { url, action });
        }
    }

    /// Seek to an absolute position in the current track (BT/local only).
    /// Stops current ffmpeg+paplay and restarts with `-ss <secs>` so
    /// decoding begins at the requested timestamp. Mozart devices use
    /// REST transport and don't go through this path.
    fn seek_to(&mut self, secs: i32) {
        let secs = secs.max(0) as u32;
        let path = self.last_played_path.clone();
        let title = self.np_title.clone();
        let duration = self.np_duration_sec;
        if path.is_empty()
            || self.current_driver_kind == "mozart"
            || self.current_driver_kind == "noop"
        {
            return;
        }
        let offset = if duration > 0 { secs.min((duration - 1).max(0) as u32) } else { secs };
        // Snap the estimator immediately so the scrub bar responds
        // before the new process has even spawned.
        self.np_seek_pos = offset as i32;
        self.now_playing_changed();
        eprintln!("seek_to: {} → {}s (dur={})", path, offset, duration);
        if let Some(tx) = &self.tx {
            let driver = match &self.current_driver {
                Some(d) => d.clone(),
                None => return,
            };
            let _ = tx.send(Command::DriverOp {
                driver,
                op: DriverOpKind::PlayLocalFile {
                    path,
                    title,
                    start_offset_sec: offset,
                },
            });
        }
        self.np_playing = true;
        self.now_playing_changed();
    }

    // ── Standby / power ─────────────────────────────────────────────
    fn refresh_standby(&mut self) {
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::GetStandby { url });
        }
    }
    fn toggle_standby(&mut self) {
        let target = !self.standby;
        self.standby = target;
        self.standby_changed();
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::SetStandby { url, standby: target });
        }
    }

    // ── Sound modes ─────────────────────────────────────────────────
    fn refresh_sound_modes(&mut self) {
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::GetSoundModes { url });
        }
    }
    fn select_sound_mode(&mut self, mode_id: String) {
        self.active_sound_mode = mode_id.clone();
        self.sound_modes_changed();
        if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
            let _ = tx.send(Command::SetSoundMode { url, mode_id });
        }
    }

    /// Refresh everything at once — used on launch and on reconnect.
    fn refresh_all(&mut self) {
        self.refresh_product_info();
        self.refresh_volume();
        self.refresh_sources();
        self.refresh_now_playing();
        self.refresh_standby();
        self.refresh_sound_modes();
        // Spawn a push-notification subscription for the active device.
        // This is best-effort: if the device's firmware doesn't expose
        // any of the candidate endpoints, the task quietly backs off
        // and the regular polling loop continues to do its job.
        // Subscribing eliminates GetVolume polling traffic when supported,
        // and surfaces external state changes (physical dial, another
        // controller) within ~50 ms instead of one poll interval.
        if let Some(url) = self.active_url_opt() {
            if self.no_notify_loaded {
                // Blocklist available — make the decision now.
                if self.no_notify_devices.contains(&url) {
                    eprintln!("device_notify: skipping {} — \
                               previously confirmed no push support \
                               (persisted in app_kv)", url);
                } else if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::SubscribeNotifications { url });
                }
            } else {
                // Blocklist still loading. Queue the URL — the
                // AppKvLoaded handler for `no_notify_devices` drains
                // this set and only fires Subscribe for entries that
                // AREN'T blocklisted. Without this defer, the very
                // first refresh_all of the session would subscribe
                // to known-bad devices and waste 7s per device
                // re-confirming what we already knew.
                eprintln!("device_notify: queuing subscribe for {} \
                           (blocklist load pending)", url);
                self.pending_notify_subscriptions.insert(url);
            }
        }
        // Load persisted FSM transition counts (idempotent — the worker
        // replies with empty string if nothing's stored yet, and load_json
        // re-seeds priors in that case).
        self.send_media(media::MediaCmd::LoadAnticipatorState);
        // Load persisted aggression level. Defaults to LOW when absent.
        self.send_media(media::MediaCmd::LoadAppKv {
            key: "aggression_level".to_string(),
        });
        // Load persisted BT auto-connect device list. Empty when first run.
        self.send_media(media::MediaCmd::LoadAppKv {
            key: "bt_autoconnect_devices".to_string(),
        });
        // Load persisted BT-respect idle threshold (minutes). Defaults
        // to 15 (set in BtRespect::new) when absent.
        self.send_media(media::MediaCmd::LoadAppKv {
            key: "bt_respect_idle_minutes".to_string(),
        });
        // Load persisted no-notify device list. Each entry is a URL
        // we've previously determined doesn't support push subscriptions
        // — skipping the discovery probe saves ~7s of startup time
        // per legacy device.
        self.send_media(media::MediaCmd::LoadAppKv {
            key: "no_notify_devices".to_string(),
        });
    }

    /// Restore the active device's last-known volume from the persistent
    /// store into the display state. Display-only: it updates the slider/
    /// badge but does NOT push to the device, so selecting a speaker never
    /// causes a surprise volume jump. A live volume probe overrides this
    /// if the device reports an authoritative value. Per-device memory is
    /// what makes "Stage at 30, headphones at 60" survive switching back
    /// and forth, even for BT sinks that don't report volume reliably.
    fn restore_device_volume(&mut self) {
        if self.active_url.is_empty() { return; }
        let vkey = format!("device_volume:{}", self.active_url);
        if let Some(v_str) = settings_store::get_str(&vkey) {
            if let Ok(v) = v_str.parse::<i32>() {
                if v > 0 && v <= 100 {
                    self.volume = v;
                    self.last_known_volume = v;
                    self.volume_changed();
                    eprintln!("device_memory: restored volume {} for {}",
                              v, self.active_url);
                }
            }
        }
    }

    /// Add each device's remembered volume (from settings_store) to the
    /// worker's queue-devices JSON, so the per-device strip can display
    /// volume alongside the head track + count. Devices with no stored
    /// volume get `volume: -1` (the strip then hides the volume chip).
    /// Best-effort: on any parse error the original JSON is returned
    /// unchanged, so a malformed payload never blanks the strip.
    fn enrich_queue_devices_with_volume(&self, json: &str) -> String {
        let mut val: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => return json.to_string(),
        };
        if let Some(arr) = val.as_array_mut() {
            for dev in arr.iter_mut() {
                let url = dev.get("device_url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                let vol = if url.is_empty() {
                    -1
                } else if url == self.active_url && self.volume > 0 {
                    // The active device's live volume is fresher than the
                    // last persisted value.
                    self.volume
                } else {
                    let vkey = format!("device_volume:{}", url);
                    settings_store::get_str(&vkey)
                        .and_then(|s| s.parse::<i32>().ok())
                        .filter(|v| *v > 0 && *v <= 100)
                        .unwrap_or(-1)
                };
                if let Some(obj) = dev.as_object_mut() {
                    obj.insert("volume".to_string(),
                               serde_json::Value::from(vol));
                }
            }
        }
        serde_json::to_string(&val).unwrap_or_else(|_| json.to_string())
    }

    /// Switch the active device, then refresh all of its state.
    fn select_device(&mut self, url: String) {
        if url.is_empty() || url == self.active_url { return; }
        // User explicitly picking a device counts as activity.
        self.bt_respect.note_activity();
        self.active_url = url;
        self.url_changed();
        // Auto-relock the high-volume unlock when switching devices.
        // The unlock is a per-session "I know what I'm doing on THIS
        // device" gesture — carrying it across devices means the user
        // who unlocked a 30 % ceiling on a quiet TV soundbar could
        // then accidentally drive a Beolab to dangerous levels just
        // by picking it from the device dropdown. Relock here forces
        // a fresh consent on the new device.
        //
        // Caller of enable_high_volume() is SafetyToast.qml; this is
        // the missing counterpart.
        self.disable_high_volume();
        // Clear stale now-playing/source so the UI doesn't show the old
        // device's data during the fetch.
        self.np_title = String::new();
        self.np_artist = String::new();
        self.np_source = String::new();
        self.now_playing_changed();
        // Reset the per-device startup safety check so the new device's
        // volume gets the per-product ceiling treatment.
        self.safety_check_done = false;
        // Wipe stale source-state from the previous device — show
        // "unknown" until the new probe lands.
        self.device_source_id = String::new();
        self.device_source_label = String::new();
        self.device_source_family = "unknown".to_string();
        self.device_source_can_play = false;
        self.device_source_block_reason = String::new();
        self.device_source_changed();
        self.device_firmware_family = String::new();
        self.device_firmware_label = String::new();
        self.device_firmware_diagnostic_json = String::new();
        self.device_supports_rest_playback = false;
        self.device_recommends_bluetooth = false;
        // Restore previously-observed classification (see PlayOutcome
        // handler for where the verdict gets written). Prevents the
        // first tap on a known-legacy device from spraying 7 doomed
        // REST endpoints again.
        let cls_key = format!("device_family:{}", self.active_url);
        if let Some(fam_str) = settings_store::get_str(&cls_key) {
            let fam = firmware_family::family_from_tag(&fam_str);
            self.device_firmware_family = fam_str;
            self.device_firmware_label = fam.label().to_string();
            self.device_supports_rest_playback = fam.supports_rest_uri_playback();
            self.device_recommends_bluetooth   = fam.recommends_bluetooth_fallback();
            eprintln!(
                "device_memory: restored '{}' = {} (skipping REST probe spray)",
                self.active_url, self.device_firmware_label);
        }
        self.device_firmware_changed();
        // Restore this device's last-known volume (display-only).
        self.restore_device_volume();
        // Reset the driver binding to the empty state. The next
        // restore_device_sink_binding call below will reload the
        // user's persisted choice for this device, OR the firmware
        // probe will rebuild the driver into a default.
        self.active_bt_mac = String::new();
        self.active_bt_name = String::new();
        self.active_multiroom_json = "null".to_string();
        self.refresh_all();
        // Kick the source-state probe asynchronously. Result arrives
        // via ResultMessage::ActiveSourceProbed.
        self.refresh_active_source();
        self.refresh_firmware_family();
        // Load the per-device sink choice saved last time the user
        // bound something for this device. Restored via AppKvLoaded
        // handler which calls rebuild_driver as needed.
        self.restore_device_sink_binding();
    }

    /// Variant of select_device that also captures the product type so
    /// the volume-safety system can pick the right ceiling. QML calls
    /// this when the user picks a device from the dropdown.
    fn select_device_with_product(&mut self, url: String, product_id: String) {
        let same = url == self.active_url && product_id == self.active_product_id;
        if same { return; }
        // User explicitly picking a device counts as activity.
        self.bt_respect.note_activity();
        // Cancel any shadow we were holding — it's tied to the previous
        // device. Restore that device's saved volume (best effort).
        if let Some(saved) = self.shadow.cancel() {
            if let (Some(prev_url), Some(tx)) = (self.active_url_opt(), &self.tx) {
                let _ = tx.send(Command::SetVolume { url: prev_url, value: saved });
            }
        }
        // Bump device_epoch — managers depending on "active device"
        // (pre-staged URLs, anticipator's confidence weighting) can
        // observe the change. Also wipe the prestaged URL cache: those
        // tokens were built against the *previous* device's local IP
        // and would point to wrong-network URLs on the new device.
        self.device_epoch.bump();
        self.prestaged_track_urls.clear();
        self.prestaged_track_ids.clear();
        // Any speculative pre-load on the previous device is now
        // irrelevant — the new device's pipeline doesn't have the
        // track. Clear all speculative state so a future tap doesn't
        // try to promote based on stale per-device records.
        self.speculative.clear();
        self.observe_action(anticipator::ActionKind::SwitchDevice);
        self.active_url = url;
        self.active_product_id = product_id.clone();
        self.url_changed();
        self.np_title = String::new();
        self.np_artist = String::new();
        self.np_source = String::new();
        self.now_playing_changed();
        self.safety_check_done = false;
        // Each device session starts locked — switching devices shouldn't
        // silently inherit the previous device's high-volume override.
        // Uses the disable_high_volume method so the relock is logged
        // and the changed() signal fires once, consistently with
        // select_device above.
        self.disable_high_volume();
        // Recompute per-product safety thresholds for the new device.
        self.safety_loud_volume   = volume_safety::loud_threshold(&product_id);
        self.safety_unsafe_volume = volume_safety::unsafe_threshold(&product_id);
        // Reset source-state for the new device; the probe below
        // refills it once the device responds.
        self.device_source_id = String::new();
        self.device_source_label = String::new();
        self.device_source_family = "unknown".to_string();
        self.device_source_can_play = false;
        self.device_source_block_reason = String::new();
        self.device_source_changed();
        // Wipe firmware-family cache so we don't display stale info
        // for the previous device; the probe below refills it.
        self.device_firmware_family = String::new();
        self.device_firmware_label = String::new();
        self.device_firmware_diagnostic_json = String::new();
        self.device_supports_rest_playback = false;
        self.device_recommends_bluetooth = false;
        // Restore previously-observed classification (see PlayOutcome
        // handler for where the verdict gets written). Prevents the
        // first tap on a known-legacy device from spraying 7 doomed
        // REST endpoints again.
        let cls_key = format!("device_family:{}", self.active_url);
        if let Some(fam_str) = settings_store::get_str(&cls_key) {
            let fam = firmware_family::family_from_tag(&fam_str);
            self.device_firmware_family = fam_str;
            self.device_firmware_label = fam.label().to_string();
            self.device_supports_rest_playback = fam.supports_rest_uri_playback();
            self.device_recommends_bluetooth   = fam.recommends_bluetooth_fallback();
            eprintln!(
                "device_memory: restored '{}' = {} (skipping REST probe spray)",
                self.active_url, self.device_firmware_label);
        }
        self.device_firmware_changed();
        // Restore this device's last-known volume (display-only; the live
        // probe overrides if the device reports an authoritative value).
        self.restore_device_volume();
        // Reset + restore the driver binding for the newly-selected
        // device (same as select_device's flow).
        self.active_bt_mac = String::new();
        self.active_bt_name = String::new();
        self.active_multiroom_json = "null".to_string();
        self.refresh_all();
        self.refresh_active_source();
        self.refresh_firmware_family();
        self.restore_device_sink_binding();
    }

    /// Append a device to the list (e.g. user-added or discovered).
    /// De-dupes on url. Stores a minimal {name, url} entry.
    fn add_device(&mut self, name: String, url: String) {
        if url.is_empty() { return; }
        let mut list: Vec<serde_json::Value> =
            serde_json::from_str(&self.devices_json).unwrap_or_default();
        let exists = list.iter().any(|d| d.get("url").and_then(|u| u.as_str()) == Some(url.as_str()));
        if !exists {
            list.push(serde_json::json!({ "name": name, "url": url }));
            self.devices_json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
            self.devices_changed();
        }
    }

    // ── Media library methods ───────────────────────────────────────
    fn send_media(&self, cmd: media::MediaCmd) {
        if let Some(tx) = &self.media_tx {
            let _ = tx.send(cmd);
        } else {
            eprintln!("media: no worker channel attached");
        }
    }
    fn rescan_library(&mut self)  { self.send_media(media::MediaCmd::RescanLibrary); }
    fn quick_scan(&mut self)      { self.send_media(media::MediaCmd::QuickScan); }
    fn scan_with_roots(&mut self, roots_json: String) {
        let roots: Vec<String> = serde_json::from_str(&roots_json).unwrap_or_default();
        // Remember the most recently scanned root so the picker can default
        // to it next time (and the Browse dialog can open there).
        if let Some(first) = roots.first() {
            if !first.trim().is_empty() {
                self.last_scan_root = first.clone().into();
                let _ = settings_store::set_str("last_scan_root", first);
                self.last_scan_root_changed();
            }
        }
        self.send_media(media::MediaCmd::ScanWithRoots(roots));
    }
    fn suggested_scan_roots(&mut self) -> String {
        let mut out = Vec::<serde_json::Value>::new();
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for sub in ["Music", "Documents/Music", "Downloads", "Documents"] {
                let p = home.join(sub);
                if p.is_dir() {
                    out.push(serde_json::json!({
                        "path": p.to_string_lossy(),
                        "label": sub,
                        "exists": true,
                    }));
                }
            }
        }
        // Always include the home dir itself as the "scan everywhere reasonable" option.
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            out.push(serde_json::json!({
                "path": home.to_string_lossy(),
                "label": "Home (everywhere)",
                "exists": home.is_dir(),
            }));
        }
        serde_json::to_string(&out).unwrap_or_else(|_| "[]".into())
    }

    /// Open the system file picker via kdialog (preferred on KDE) or
    /// zenity (fallback). The picker filters for common audio
    /// extensions. Synchronous: blocks until the dialog closes.
    /// Returns the chosen path or empty string if the user cancels
    /// or no picker is installed.
    ///
    /// We try kdialog first because the user is on KDE/Fairphone FP5
    /// where it integrates better with the system theme; zenity is
    /// the cross-DE fallback. Both binaries are available in standard
    /// Linux package repos.
    fn pick_audio_file(&mut self) -> String {
        use std::process::Command;
        let filter = "Audio files (*.wav *.opus *.mp3 *.flac *.m4a *.ogg)";
        // kdialog: --getopenfilename <start_dir> <filter>
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        if let Ok(out) = Command::new("kdialog")
            .args(["--getopenfilename", &home, filter])
            .output()
        {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    eprintln!("pick_audio_file: kdialog → {}", path);
                    return path;
                }
            }
        }
        // zenity: --file-selection --filename=<dir>/ --file-filter=<filter>
        if let Ok(out) = Command::new("zenity")
            .args(["--file-selection",
                   &format!("--filename={}/", home),
                   "--file-filter=Audio | *.wav *.opus *.mp3 *.flac *.m4a *.ogg",
                   "--file-filter=All files | *"])
            .output()
        {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    eprintln!("pick_audio_file: zenity → {}", path);
                    return path;
                }
            }
        }
        eprintln!("pick_audio_file: no picker available (install kdialog or zenity)");
        String::new()
    }

    /// Pick a FOLDER instead of a single file. Used by mobile-friendly
    /// flows where the user typically has a "Music" folder they want
    /// the controller to scan or treat as a root. On KDE/plasma-mobile
    /// (Fairphone FP5) kdialog --getexistingdirectory renders a
    /// touch-friendly tree picker that's the natural mobile gesture.
    ///
    /// Returns the selected folder path, or empty string on cancel /
    /// no picker available.
    fn pick_audio_folder(&mut self) -> String {
        use std::process::Command;
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        // Sensible starting point: Music subfolder if it exists, else home.
        let start_dir = {
            let music = format!("{}/Music", home);
            if std::path::Path::new(&music).exists() { music } else { home.clone() }
        };
        // kdialog --getexistingdirectory <start_dir>
        // Touch-friendly on plasma-mobile.
        if let Ok(out) = Command::new("kdialog")
            .args(["--getexistingdirectory", &start_dir])
            .output()
        {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    eprintln!("pick_audio_folder: kdialog → {}", path);
                    return path;
                }
            }
        }
        // zenity fallback: --file-selection --directory
        if let Ok(out) = Command::new("zenity")
            .args(["--file-selection", "--directory",
                   &format!("--filename={}/", start_dir)])
            .output()
        {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    eprintln!("pick_audio_folder: zenity → {}", path);
                    return path;
                }
            }
        }
        eprintln!("pick_audio_folder: no picker available (install kdialog or zenity)");
        String::new()
    }

    /// Suggested default directory for soundscape outputs. Logic:
    ///   1. If `$HOME/Music` exists, use `$HOME/Music/Soundscapes`
    ///      (matches the catalog's standard root pattern).
    ///   2. Else if $HOME is set, use `$HOME/Soundscapes`.
    ///   3. Else return empty so the QML side can fall back to
    ///      "same dir as input".
    /// The directory itself is not created here — `process_soundscape`
    /// in the worker handles `mkdir -p` on the parent before writing.
    /// Friendly name for the currently active device URL. Pulls from
    /// `devices_json` (the mDNS-discovered + user-added device list),
    /// since this captures the name even when product-info probes
    /// fail. Falls back to the bare URL when no entry matches.
    fn active_device_name(&mut self) -> String {
        if self.active_url.is_empty() { return String::new(); }
        if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&self.devices_json) {
            for d in &list {
                if d.get("url").and_then(|v| v.as_str()) == Some(self.active_url.as_str()) {
                    if let Some(n) = d.get("name").and_then(|v| v.as_str()) {
                        if !n.is_empty() {
                            return n.to_string();
                        }
                    }
                }
            }
        }
        self.active_url.clone()
    }

    fn suggested_soundscapes_dir(&mut self) -> String {
        if let Ok(home) = std::env::var("HOME") {
            let music = std::path::Path::new(&home).join("Music");
            if music.is_dir() {
                return music.join("Soundscapes").to_string_lossy().to_string();
            }
            return std::path::Path::new(&home)
                .join("Soundscapes")
                .to_string_lossy()
                .to_string();
        }
        String::new()
    }

    /// List rendered .opus soundscapes in `suggested_soundscapes_dir`.
    /// Sorted newest-first by mtime so the most recent renders are
    /// always at the top. Each entry: {path, name, size_bytes,
    /// modified_unix}. The QML side renders this as a tappable list.
    ///
    /// We scan synchronously since the directory is small (typically
    /// <100 files). For pathological cases we cap at 200 entries to
    /// keep the JSON manageable.
    fn list_soundscapes(&mut self) -> String {
        let dir = self.suggested_soundscapes_dir();
        if dir.is_empty() { return "[]".to_string(); }
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => return "[]".to_string(),
        };
        let mut rows: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                let ext = p.extension().and_then(|s| s.to_str())
                           .map(|s| s.to_ascii_lowercase());
                if ext.as_deref() != Some("opus") { return None; }
                let md = e.metadata().ok()?;
                let mtime = md.modified().ok()?;
                Some((p, md.len(), mtime))
            })
            .collect();
        rows.sort_by(|a, b| b.2.cmp(&a.2));   // newest first
        rows.truncate(200);
        let arr: Vec<serde_json::Value> = rows.into_iter().map(|(p, sz, mt)| {
            let name = p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let modified_unix = mt
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            serde_json::json!({
                "path": p.to_string_lossy().to_string(),
                "name": name,
                "size_bytes": sz,
                "modified_unix": modified_unix,
            })
        }).collect();
        serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
    }
    fn analyze_backlog(&mut self) { self.send_media(media::MediaCmd::AnalyzeBacklog); }
    fn cancel_analysis(&mut self) { self.send_media(media::MediaCmd::CancelAnalysis); }

    /// Trigger a focused scan of the containing directory so the
    /// catalog picks up the freshly-rendered soundscape. The user
    /// will see it appear in Library + Search shortly after.
    fn add_soundscape_to_library(&mut self, path: String) -> bool {
        let p = std::path::Path::new(&path);
        if path.is_empty() || !p.is_file() {
            self.last_play_status = "soundscape file not found".to_string();
            self.play_status_changed();
            eprintln!("add_soundscape_to_library: invalid path {:?}", path);
            return false;
        }
        // Scan the containing directory only — fast, focused. The
        // file itself becomes a row in the catalog with whatever
        // tags the scanner can read from its Opus headers. If the
        // user wants nicer metadata they can edit it via TagEditor.
        let parent = match p.parent() {
            Some(d) => d.to_string_lossy().to_string(),
            None => {
                self.last_play_status = "soundscape has no parent directory".to_string();
                self.play_status_changed();
                return false;
            }
        };
        eprintln!("add_soundscape_to_library: scanning {} to pick up {}",
                  parent, path);
        self.last_play_status = format!(
            "Adding to library — scanning {}…",
            std::path::Path::new(&parent)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or(parent.clone())
        );
        self.play_status_changed();
        self.bt_respect.note_activity();
        self.send_media(media::MediaCmd::ScanWithRoots(vec![parent]));
        true
    }

    /// Validate that `path` resolves to a regular file inside the
    /// user's soundscapes directory. Defensive — protects against
    /// the rename/delete methods being invoked with a path that
    /// tries to escape the sandbox via `..` or absolute paths.
    /// Returns the canonicalized PathBuf on success, None on any
    /// of the failure modes (with a side-effect log line).
    fn validate_soundscape_path(&self, path: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(path);
        let canon = match p.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("soundscape: canonicalize {:?} failed: {}", path, e);
                return None;
            }
        };
        if !canon.is_file() {
            eprintln!("soundscape: {:?} is not a regular file", canon);
            return None;
        }
        // The sandbox: same directory as suggested_soundscapes_dir().
        // We canonicalize that too so symlink chains don't fool the
        // contains-check. `~/Music/Soundscapes` typically.
        let sandbox = std::path::PathBuf::from(
            self.soundscapes_dir_string_only()
        );
        let sandbox_canon = match sandbox.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                // Sandbox doesn't exist yet — nothing to delete/rename
                // inside it. Reject conservatively.
                eprintln!("soundscape: sandbox {:?} not on disk yet",
                          sandbox);
                return None;
            }
        };
        if !canon.starts_with(&sandbox_canon) {
            eprintln!("soundscape: rejected path {:?} (outside sandbox {:?})",
                      canon, sandbox_canon);
            return None;
        }
        Some(canon)
    }

    /// Return the soundscapes directory as a plain String (without
    /// the qt_method noise of suggested_soundscapes_dir, which mutates
    /// &mut self). Used inside validate_soundscape_path.
    fn soundscapes_dir_string_only(&self) -> String {
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(home);
            return home.join("Music").join("Soundscapes")
                .to_string_lossy().to_string();
        }
        // Fallback if HOME isn't set — unlikely on Linux.
        "/tmp/Soundscapes".to_string()
    }

    fn delete_soundscape(&mut self, path: String) -> bool {
        let canon = match self.validate_soundscape_path(&path) {
            Some(c) => c,
            None => {
                self.last_play_status =
                    "soundscape delete refused — path not in Soundscapes folder".into();
                self.play_status_changed();
                return false;
            }
        };
        let name = canon.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| canon.display().to_string());
        match std::fs::remove_file(&canon) {
            Ok(_) => {
                eprintln!("soundscape: deleted {:?}", canon);
                self.last_play_status = format!("Deleted soundscape: {}", name);
                self.play_status_changed();
                self.bt_respect.note_activity();
                // Notify QML so the previous-renders list refetches.
                // soundscape_changed signal is what the page listens
                // to for refresh.
                self.soundscape_changed();
                true
            }
            Err(e) => {
                eprintln!("soundscape: delete {:?} failed: {}", canon, e);
                self.last_play_status =
                    format!("Couldn't delete '{}': {}", name, e);
                self.play_status_changed();
                false
            }
        }
    }

    fn rename_soundscape(&mut self, old_path: String, new_name: String) -> bool {
        let canon = match self.validate_soundscape_path(&old_path) {
            Some(c) => c,
            None => {
                self.last_play_status =
                    "soundscape rename refused — path not in Soundscapes folder".into();
                self.play_status_changed();
                return false;
            }
        };
        // new_name must be a plain basename — no path separators, no
        // empty string, no `..` shenanigans. We keep the file in its
        // current directory.
        let trimmed = new_name.trim();
        if trimmed.is_empty()
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed == "."
            || trimmed == ".."
        {
            eprintln!("soundscape: rename refused — invalid name {:?}", trimmed);
            self.last_play_status =
                "Rename refused — name can't be empty or contain slashes".into();
            self.play_status_changed();
            return false;
        }
        // Preserve the original extension if the user didn't include
        // one (avoids accidentally renaming "rain v3.opus" → "rainy"
        // and losing the .opus suffix). Match case of original.
        let old_ext = canon.extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{}", s));
        let target_basename = match (&old_ext, trimmed.contains('.')) {
            (Some(ext), false) => format!("{}{}", trimmed, ext),
            _ => trimmed.to_string(),
        };
        let parent = match canon.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                self.last_play_status =
                    "Rename failed — file has no parent directory".into();
                self.play_status_changed();
                return false;
            }
        };
        let target = parent.join(&target_basename);
        if target.exists() {
            self.last_play_status =
                format!("Rename refused — '{}' already exists", target_basename);
            self.play_status_changed();
            eprintln!("soundscape: rename target {:?} already exists", target);
            return false;
        }
        match std::fs::rename(&canon, &target) {
            Ok(_) => {
                eprintln!("soundscape: renamed {:?} → {:?}", canon, target);
                self.last_play_status =
                    format!("Renamed to '{}'", target_basename);
                self.play_status_changed();
                self.bt_respect.note_activity();
                self.soundscape_changed();
                true
            }
            Err(e) => {
                eprintln!("soundscape: rename {:?} → {:?} failed: {}",
                          canon, target, e);
                self.last_play_status = format!("Rename failed: {}", e);
                self.play_status_changed();
                false
            }
        }
    }
    fn load_recent(&mut self)     {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        self.send_media(media::MediaCmd::LoadRecent);
    }
    fn load_artists(&mut self)    {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        self.send_media(media::MediaCmd::LoadArtists);
    }
    fn load_tags(&mut self)       {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        self.send_media(media::MediaCmd::LoadTags);
    }
    fn load_home(&mut self)       {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        self.send_media(media::MediaCmd::LoadHome);
    }
    fn load_by_artist(&mut self, artist_id: i32) {
        self.send_media(media::MediaCmd::LoadByArtist(artist_id as i64));
    }
    fn load_by_tag(&mut self, tag_id: i32) {
        self.send_media(media::MediaCmd::LoadByTag(tag_id as i64));
    }
    /// Paginated load: request one page of the artists list. The QML
    /// side calls this on initial library open with offset=0, and
    /// then again with offset=current_len on near-bottom scroll.
    /// Result lands via `page_json` after a brief async round-trip.
    fn load_artists_page(&mut self, offset: i32, limit: i32) {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        let sort = {
            let s = self.artist_sort.to_string();
            if s.is_empty() { "name".to_string() } else { s }
        };
        self.send_media(media::MediaCmd::LoadArtistsPage {
            offset: offset as i64, limit: limit as i64, sort,
        });
    }
    fn load_recent_page(&mut self, offset: i32, limit: i32) {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        self.send_media(media::MediaCmd::LoadRecentPage {
            offset: offset as i64, limit: limit as i64,
        });
    }
    fn load_by_artist_page(&mut self, artist_id: i32, offset: i32, limit: i32) {
        self.send_media(media::MediaCmd::LoadByArtistPage {
            artist_id: artist_id as i64,
            offset: offset as i64, limit: limit as i64,
        });
    }
    fn load_all_tracks_page(&mut self, offset: i32, limit: i32) {
        self.observe_action(anticipator::ActionKind::OpenLibrary);
        let sort = {
            let s = self.all_tracks_sort.to_string();
            if s.is_empty() { "title".to_string() } else { s }
        };
        self.send_media(media::MediaCmd::LoadAllTracksPage {
            offset: offset as i64, limit: limit as i64, sort,
        });
    }
    /// Change the all-tracks sort and persist it. The caller (QML) reloads
    /// page 0 after this so the new order takes effect.
    fn set_all_tracks_sort(&mut self, sort: String) {
        // Validate through the enum so only known tags persist.
        let tag_str = media::catalog::TrackSort::from_tag(&sort).to_tag();
        self.all_tracks_sort = tag_str.into();
        let _ = settings_store::set_str("all_tracks_sort", tag_str);
        self.all_tracks_sort_changed();
    }
    fn record_play(&mut self, track_id: i32) {
        self.send_media(media::MediaCmd::RecordPlay(track_id as i64));
    }

    /// Cycle the repeat mode: Off → One → All → Off. Persisted via
    /// settings_store immediately. The repeat_mode_tag qt_property
    /// reflects the current value so QML can render the right icon
    /// (loop, loop-1, no-loop) on the toggle button.
    fn repeat_mode_cycle(&mut self) {
        let next = playback_modes::load_repeat_mode().next();
        playback_modes::save_repeat_mode(next);
        self.repeat_mode_tag = next.as_tag().to_string();
        self.playback_modes_changed();
        eprintln!("playback_modes: repeat={}", next.as_tag());
    }

    /// Toggle child mode on/off. Persisted immediately.
    fn set_child_mode(&mut self, on: bool) {
        playback_modes::save_child_mode(on);
        self.child_mode_on = on;
        self.child_skip_depth = 0;
        self.playback_modes_changed();
        eprintln!("playback_modes: child_mode={}", on);
    }

    /// Build an "intent" describing a proposed multiroom play, write
    /// it to intent_payload, and signal QML to render a confirmation
    /// sheet. The payload is JSON with a `kind` field the QML side
    /// switches on. NO playback happens here — that's the QML's
    /// confirm-handler's job (it calls into the existing multiroom
    /// create path once the user OKs).
    ///
    /// The payload lists every speaker we could include + the user's
    /// last-used grouping, so the confirmation sheet doesn't have to
    /// re-query anything. Speakers are deduped by MAC and annotated
    /// with their current connection state.
    fn propose_multiroom_play(&mut self, track_id: i32) {
        let mut candidates: Vec<serde_json::Value> = Vec::new();
        let live_macs = collect_live_bluez_macs();
        // Always include the active device (it's the user's current
        // focus) — flag whether it can play URIs or only via BT.
        if !self.active_url.is_empty() {
            candidates.push(serde_json::json!({
                "url": self.active_url,
                "name": self.product_name,
                "kind": "network",
                "family": self.device_firmware_family,
                "rest_ok": self.device_supports_rest_playback,
                "active": true,
            }));
        }
        for (mac, cfg) in self.bt_autoconnect_iter() {
            let mac_norm = mac.replace(':', "_").to_uppercase();
            candidates.push(serde_json::json!({
                "mac": mac,
                "name": cfg.name,
                "kind": "bluetooth",
                "live": live_macs.contains(&mac_norm),
                "active": mac == self.active_bt_mac,
            }));
        }
        let intent = serde_json::json!({
            "kind": "propose_multiroom_play",
            "track_id": track_id,
            "candidates": candidates,
            "default_volume": self.volume,
        });
        self.intent_payload = intent.to_string();
        self.intent_changed();
        eprintln!("intent: propose_multiroom_play track={} candidates={}",
                  track_id, candidates.len());
    }

    /// Same shape but for soundscapes. Lists the available scene
    /// JSONs (from the bo-mux scene dir) and the current speaker
    /// layout so QML can render a "Play <Forest Immersive> across
    /// [Stage + Smokey]?" confirmation.
    fn propose_soundscape_play(&mut self, scene_name: String) {
        let scene_dir = dirs::data_dir()
            .map(|d| d.join("bo-mux/scenes"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let scenes: Vec<String> = std::fs::read_dir(&scene_dir)
            .ok()
            .map(|rd| rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("json") {
                        p.file_stem().and_then(|s| s.to_str()).map(String::from)
                    } else { None }
                })
                .collect())
            .unwrap_or_default();
        let intent = serde_json::json!({
            "kind": "propose_soundscape_play",
            "requested_scene": scene_name,
            "available_scenes": scenes,
            "scene_dir": scene_dir.to_string_lossy(),
        });
        self.intent_payload = intent.to_string();
        self.intent_changed();
        eprintln!("intent: propose_soundscape_play scene='{}' available={}",
                  scene_name, scenes.len());
    }

    /// Kick off whisper transcription for the currently-loaded
    /// track. Runs synchronously on the calling thread — for a
    /// typical 3-4 min song with the base.en model this takes
    /// 5-15 s, during which the UI thread is blocked. That's
    /// not ideal for UX but is correct and ships now; a future
    /// turn can move the work to a worker thread once we add a
    /// general mpsc<LyricsResult> channel parallel to the media
    /// channel. For now the QML side should disable the button
    /// while lyrics_status starts with "transcribing".
    ///
    /// Templates: "lrc" / "markdown" / "plain". Default LRC.
    fn transcribe_lyrics(&mut self, template: String) {
        let tid = self.last_played_track_id;
        if tid == 0 {
            self.lyrics_status = "no track loaded — play something first".into();
            self.lyrics_changed();
            return;
        }
        let (path, title, artist) = match self.lookup_track_meta_for_lyrics(tid) {
            Some(t) => t,
            None => {
                self.lyrics_status = "track path not found in catalog".into();
                self.lyrics_changed();
                return;
            }
        };
        let tmpl = match template.as_str() {
            "markdown" => lyrics_transcriber::Template::Markdown,
            "plain"    => lyrics_transcriber::Template::Plain,
            _          => lyrics_transcriber::Template::Lrc,
        };
        self.lyrics_status = format!("transcribing '{}'…", title);
        self.lyrics_text = String::new();
        self.lyrics_changed();
        // SYNC call. See doc above for why.
        match lyrics_transcriber::transcribe_with_cache(
            std::path::Path::new(&path), tmpl, &title, &artist
        ) {
            Ok(text) => {
                self.lyrics_text = text;
                self.lyrics_status = format!("done ({})", template);
            }
            Err(e) => {
                self.lyrics_text = String::new();
                self.lyrics_status = format!("error: {}", e);
            }
        }
        self.lyrics_changed();
    }

    /// Path + title + artist lookup. Same idea as
    /// lookup_track_meta_for_skip but returns artist too (used by
    /// the lyrics template).
    fn lookup_track_meta_for_lyrics(&self, tid: i64)
        -> Option<(String, String, String)>
    {
        let caches = [&self.library_json, &self.artists_json];
        for cache in caches {
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(cache) {
                for entry in &arr {
                    if entry.get("id").and_then(|v| v.as_i64()) == Some(tid) {
                        let path = entry.get("path")
                            .and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let title = entry.get("title")
                            .and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let artist = entry.get("artist")
                            .and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if !path.is_empty() {
                            return Some((path, title, artist));
                        }
                    }
                }
            }
        }
        None
    }

    /// Single-tap on a track in any library list. Semantics:
    ///
    ///   • Nothing playing → play immediately.
    ///   • Something already playing → push to the back of THIS
    ///     device's queue. The current playback continues; the
    ///     newly-queued track plays when the current one ends or
    ///     the user taps next.
    ///
    /// This is the "gate music play requests to one per device"
    /// behavior the user asked for — accidental re-taps and "queue
    /// up the next song while I listen to this one" both DTRT
    /// without interrupting the current playback.
    ///
    /// The "playing" check uses np_playing as the source of truth.
    /// During the brief window between the previous track ending and
    /// the queue advancing, np_playing may be false even though the
    /// driver is between songs — in that case we fall through to
    /// play_track, which is correct behavior (we shouldn't queue
    /// when nothing's playing).
    fn enqueue_or_play(&mut self, track_id: i32) {
        // Tapping the track that's already playing is a no-op in the plain
        // gesture (don't queue a duplicate of what you're hearing). The
        // explicit mode methods can still re-add it if the user really
        // wants to.
        if self.np_playing && self.last_played_track_id == track_id as i64 {
            return;
        }
        // Delegate to the mode-aware path using the user's persisted
        // default. For "stack" (the historical default) this matches the
        // old behaviour: append when something's playing, else play.
        let mode = if self.enqueue_mode.to_string().is_empty() {
            "stack".to_string()
        } else {
            self.enqueue_mode.to_string()
        };
        self.enqueue_with_mode(track_id, mode);
    }

    /// Persist the user's default enqueue mode. Validated through
    /// EnqueueMode::from_tag so a bad value can't be stored.
    fn set_enqueue_mode(&mut self, mode: String) {
        let m = queue_policy::EnqueueMode::from_tag(&mode);
        self.enqueue_mode = m.as_tag().into();
        let _ = settings_store::set_str("enqueue_mode", m.as_tag());
        self.enqueue_mode_changed();
    }


    /// Execute one LayerAction from the layer model. The Top layer routes
    /// through the normal play path (the transport bar controls it). For
    /// Overlay / Underlay, the model + UI state are now exact; the final
    /// concrete step is spawning a SECOND concurrent decode pipe to the
    /// same sink with the layer's offset and gain. PulseAudio mixes
    /// streams to a sink, so this is mechanically possible — the driver
    /// work is a selective reap (StopAllExcept) so the layers don't kill
    /// each other. Until that driver hook lands, non-top spawns report
    /// their intent honestly rather than silently doing nothing.
    fn execute_layer_action(&mut self, action: layer_player::LayerAction) {
        use layer_player::LayerAction;
        match action {
            LayerAction::Spawn { id, track_id, offset_ms, gain, paused } => {
                // The Top layer (the first/only one) uses the normal play
                // path. is_layered() is false for a lone top.
                if !self.layer_set.is_layered() {
                    self.play_track(track_id as i32);
                    return;
                }
                eprintln!(
                    "layer: spawn id={} track={} offset={}ms gain={:.2} paused={} \
                     (concurrent pipe is the remaining driver hook)",
                    id, track_id, offset_ms, gain, paused);
                self.last_play_status = format!(
                    "Layer queued (id {}, +{}ms) — concurrent playback lands with \
                     the driver hook", id, offset_ms);
                self.play_status_changed();
            }
            LayerAction::Stop { id } => {
                eprintln!("layer: stop id={}", id);
            }
            LayerAction::StopAllExcept { id } => {
                eprintln!("layer: stop all except id={}", id);
            }
        }
    }


    /// Mode-aware enqueue. Builds the current PlaybackState, asks the
    /// queue_policy for an action plan, then executes it. The policy holds
    /// all the behavioural decisions; this method is just the executor, so
    /// adding or altering a mode never touches branching logic here.
    fn enqueue_with_mode(&mut self, track_id: i32, mode: String) {
        use queue_policy::{EnqueueMode, PlaybackState, QueueAction, plan};
        let mode = EnqueueMode::from_tag(&mode);
        let state = PlaybackState {
            is_playing: self.np_playing,
            is_paused: !self.np_playing && self.last_played_track_id != 0,
            position_ms: (self.np_position_sec as i64).max(0) * 1000,
        };
        let actions = plan(mode, track_id as i64, state);
        let dev = self.active_url.clone();
        for action in actions {
            match action {
                QueueAction::ClearQueue => {
                    self.send_media(media::MediaCmd::QueueClear { device_url: dev.clone() });
                }
                QueueAction::PushBack(tid) => {
                    self.send_media(media::MediaCmd::QueuePush {
                        track_id: tid, device_url: dev.clone() });
                    self.last_play_status = format!("added track {} to queue", tid);
                    self.play_status_changed();
                }
                QueueAction::InsertAt(tid, at) => {
                    self.send_media(media::MediaCmd::QueueInsert {
                        track_id: tid, device_url: dev.clone(), at });
                }
                QueueAction::PlayNow(tid) => {
                    self.play_track(tid as i32);
                }
                QueueAction::AddSyncLayer(tid, paused) => {
                    // Record the overlay in the layer model and execute its
                    // spawn intent. The model bookkeeping is exact; the
                    // driver-level concurrent decode pipe is the remaining
                    // step (see execute_layer_action).
                    let acts = self.layer_set.add_overlay(tid, paused);
                    for a in acts { self.execute_layer_action(a); }
                }
                QueueAction::AddUnderlayLayer(tid, off_ms) => {
                    let acts = self.layer_set.add_underlay(tid, off_ms);
                    for a in acts { self.execute_layer_action(a); }
                }
            }
        }
    }


    /// Double-tap on a track. "Play this RIGHT NOW, don't lose my
    /// place." Pushes the currently-playing track to position 0 of
    /// the queue (so it plays next when the new track ends), then
    /// plays the requested track immediately.
    ///
    /// If nothing is playing, falls through to play_track — there's
    /// nothing to displace.
    ///
    /// Limitation: position resumption is at the TRACK level, not
    /// mid-song. The displaced track restarts from 0:00 when the
    /// queue advances back to it. Proper mid-song resume would need
    /// position tracking in the player (paplay doesn't expose it,
    /// see SPEAKER_SEPARATION.md notes about backend swap to mpv).
    fn play_now(&mut self, track_id: i32) {
        if self.np_playing && self.last_played_track_id != 0
                            && self.last_played_track_id != track_id as i64 {
            eprintln!("play_now: displacing current {} to queue[0], \
                       playing {} immediately on {}",
                      self.last_played_track_id, track_id, self.active_url);
            self.send_media(media::MediaCmd::QueueInsert {
                track_id: self.last_played_track_id,
                device_url: self.active_url.clone(),
                at: 0,
            });
        }
        self.play_track(track_id);
    }

    /// Play the given track on the active device. Step 1: resolve path
    /// via the media worker (async). When TrackForPlay comes back, the
    /// result handler completes the chain (expose + POST).
    fn play_track(&mut self, track_id: i32) {
        // Keep the layer model's Top in sync with normal playback. A plain
        // play of a new track collapses any prior layered mix to a single
        // Top (the old overlays/underlays end when you start something new).
        // Pure bookkeeping — we DON'T execute the returned actions, since
        // play_track itself performs the spawn and the driver's own reap
        // stops the previous pipes.
        let _ = self.layer_set.collapse_to(track_id as i64);
        // Entry log — single line that captures EVERYTHING the
        // downstream dispatch decisions depend on, so logs from a
        // user reporting "playback not working" reveal which branch
        // was taken without having to reproduce. Add no PII; this
        // is the same data we'd ask for in a bug report.
        eprintln!(
            "play_track: entry tid={} active='{}' driver_kind='{}' \
             family='{}' rest_ok={} bt_mac='{}' source_family='{}' \
             source_can_play={}",
            track_id, self.active_url, self.current_driver_kind,
            self.device_firmware_family, self.device_supports_rest_playback,
            self.active_bt_mac, self.device_source_family,
            self.device_source_can_play,
        );

        if self.active_url.is_empty() {
            self.last_play_status = "no active device".to_string();
            self.play_status_changed();
            return;
        }

        // ── BT auto-fallback for unclassified / legacy devices ──
        //
        // Specific user failure mode we're addressing here:
        //   - Active device is a Beosound (Stage/Emerge/etc.) running
        //     legacy Mozart firmware. Returns REST 500s on every play
        //     endpoint; can't accept URI playback.
        //   - Firmware classification probe hasn't completed (or
        //     returned empty product_name). `auto_pair_bt_mac_by_name`
        //     bails on empty name; `active_bt_mac` stays empty.
        //   - `for_family` with `bt_mac=None` for LegacyMozart picks
        //     LocalDriver — so play_track silently plays on the
        //     controller's speakers, not the room speaker.
        //   - User has BT speakers connected via the auto-connect
        //     system (e.g. Smokey, Emerge01) ready to play, but the
        //     driver layer doesn't know to use them.
        //
        // Fix: when product_name was never populated AND we have at
        // least one BT speaker the auto-connect system reports as
        // connected, pick the first one and force a driver rebuild
        // BEFORE proceeding. This routes audio to a real speaker
        // instead of the laptop.
        //
        // Safety: only kicks in when active_bt_mac is empty AND the
        // family is non-Mozart AND there's no pending active_bt_mac
        // from a user action. We never override a user choice; we
        // only fill in a default when none was set.
        let needs_bt_fallback = self.active_bt_mac.is_empty()
            && (self.current_driver_kind == "local"
                || self.current_driver_kind == "noop"
                || self.device_firmware_family == "mozart_legacy");
        if needs_bt_fallback {
            // Walk the auto-connect registry for any device whose
            // BT sink is currently present in pactl. This is the
            // strongest "currently usable" signal — it means the
            // audio server has the route ready, paplay will reach
            // the speaker.
            //
            // Previously we picked on `last_success.is_some()`,
            // which is a HISTORICAL signal (a successful connect
            // happened earlier in this session). It's a great
            // proxy when connects stay alive, but the user's logs
            // showed Smokey getting last_success=true from a past
            // run and then `bluetoothctl connect Smokey → FAIL`
            // repeatedly with no live sink — and we'd still pick
            // Smokey, spawn ffmpeg+paplay against its nonexistent
            // sink, ffmpeg would get SIGPIPE and exit 224.
            //
            // pactl_macs is the set of MACs that currently have a
            // bluez sink in the audio server. We prefer those.
            // Falls back to last_success if pactl has no bluez
            // sinks at all (in which case there's nothing better
            // to choose).
            let pactl_macs = collect_live_bluez_macs();
            let mut candidate: Option<(String, String)> = None;
            // Pass 1: pick the first registry entry whose MAC has
            // a live pactl sink.
            for (mac, cfg) in self.bt_autoconnect_iter() {
                let mac_norm = mac.replace(':', "_").to_uppercase();
                if pactl_macs.contains(&mac_norm) {
                    candidate = Some((mac, cfg.name));
                    break;
                }
            }
            // Pass 2 (only if pass 1 found nothing AND pactl knows
            // about no bluez sinks at all — i.e. nothing is live):
            // fall back to historical last_success. Otherwise the
            // user has zero working speakers and the failure mode
            // is "no audio path available" which the BT driver
            // will surface clearly.
            if candidate.is_none() && pactl_macs.is_empty() {
                for (mac, cfg) in self.bt_autoconnect_iter() {
                    if cfg.last_success.is_some() {
                        candidate = Some((mac, cfg.name));
                        break;
                    }
                }
            }
            if let Some((mac, name)) = candidate {
                eprintln!(
                    "play_track: auto-routing to connected BT speaker \
                     '{}' ({}) — active device '{}' can't accept URI \
                     playback and no BT was explicitly bound",
                    name, mac, self.active_url);
                // set_active_bt_mac handles the rebuild + persistence;
                // calling it directly here saves the user from having
                // to manually configure on the Connect page.
                self.set_active_bt_mac(mac, name);
            } else if self.current_driver_kind == "local" {
                // We're about to play on the controller's own
                // speakers. That's almost never what the user wants
                // when they have an active Beosound selected. Surface
                // it before the fact rather than after a silent miss.
                eprintln!(
                    "play_track: WARNING — driver=local, audio will \
                     play on the controller's speakers. Pair a BT \
                     speaker to route to the room.");
            }
        }
        // Pre-play guard: if the device's active source can't accept
        // URI playback (Spotify / AirPlay / Bluetooth / Line-in / TV
        // / etc.), bail with a clear toast BEFORE wasting a play_uri
        // round trip. The user sees the actionable reason immediately
        // instead of debugging silent failure.
        //
        // We only guard when we KNOW the source family — `unknown`
        // (probe hasn't completed or returned nothing) lets the play
        // attempt through, because some firmware variants don't expose
        // the active-source paths at all, and we don't want to block
        // play on devices where DLNA happens to work despite an
        // unprobeable source state.
        //
        // The guard is also bypassed for non-Mozart drivers: when we
        // route through Bluetooth (or any future non-REST driver),
        // the device's active source is irrelevant — audio goes over
        // a different channel entirely. Only Mozart needs this check.
        if !self.device_source_can_play
            && !self.device_source_family.is_empty()
            && self.device_source_family != "unknown"
            && (self.current_driver_kind.is_empty()
                || self.current_driver_kind == "mozart")
        {
            let reason = self.device_source_block_reason.clone();
            eprintln!("play_track: blocked by source guard — source={}, family={}, \
                       reason={}", self.device_source_id, self.device_source_family,
                       reason);
            self.last_play_status = if reason.is_empty() {
                "Device source can't play URIs — switch to Network on the device.".into()
            } else {
                reason
            };
            self.play_status_changed();
            // Re-probe in case the user just switched sources on the
            // device but our cached state is stale. The result lands
            // async; the next tap will go through if the source has
            // genuinely changed.
            self.refresh_active_source();
            return;
        }
        self.observe_action(anticipator::ActionKind::PlayTrack);
        let tid = track_id as i64;

        // Speculative/shadow paths only apply to REST-driven (Mozart)
        // devices. The optimizations work by talking to the device's
        // own audio pipeline (paused-track promotion, volume-zero
        // shadow streaming) — neither of those concepts maps to a
        // Bluetooth audio sink. For non-Mozart drivers we skip
        // straight to track resolution.
        let is_mozart_driver = self.current_driver_kind.is_empty()
            || self.current_driver_kind == "mozart";

        // HOTTEST path: speculative-pause promotion. If this exact
        // track is already loaded in the device's pipeline (paused),
        // a single Transport=play starts it — no fetch, no buffering,
        // no volume manipulation. This is preferred over shadow stream
        // because there's zero risk of an audible artifact during the
        // transition.
        let now = std::time::Instant::now();
        if is_mozart_driver {
            if let speculative_play::PromoteResult::Hit =
                self.speculative.try_promote(&self.active_url, tid, now)
            {
            eprintln!("speculative_play: HIT for track {} — sending Transport=play", tid);
            self.last_play_status = "instant (speculative)".to_string();
            self.last_play_was_shadow = false;
            self.play_status_changed();
            self.send_media(media::MediaCmd::RecordPlay(tid));
            // Record the win for the accuracy tracker.
            self.speculation_accuracy.observe(speculation_accuracy::Outcome::Hit);
            // Invalidate the speculation now that it's been used — a
            // user re-tap shouldn't promote the same paused load twice
            // (the device is no longer paused after our unpause).
            self.speculative.invalidate(&self.active_url);
            if let Some(tx) = &self.tx {
                let _ = tx.send(Command::Transport {
                    url: self.active_url.clone(),
                    action: "play".to_string(),
                });
            }
            return;
            }
        }

        // Hot path: did we shadow this exact track? If so, the device is
        // already streaming it at volume=0. We just restore the volume.
        // Like speculative_play above, this only applies to Mozart-driven
        // devices — Bluetooth audio doesn't have a "shadow at volume 0"
        // concept.
        if is_mozart_driver {
            match self.shadow.promote(tid) {
            shadow_stream::PromoteResult::Hit { saved_volume } => {
                eprintln!("shadow: HIT for track {} — restoring vol {}", tid, saved_volume);
                self.last_play_status = "instant (shadow)".to_string();
                self.last_play_was_shadow = true;
                self.play_status_changed();
                // Restore volume via the normal SetVolume path so the
                // safety system stays in the loop. The device's audio
                // pipeline is already warm with this track playing silently,
                // so the user hears it immediately.
                self.send_media(media::MediaCmd::RecordPlay(tid));
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::SetVolume {
                        url: self.active_url.clone(),
                        value: saved_volume,
                    });
                }
                return;
            }
            shadow_stream::PromoteResult::Mismatch { previous_saved_volume, .. } => {
                // We were shadowing a different track. Restore that shadow's
                // saved volume so the user's baseline doesn't drift, then
                // fall through to the normal play path.
                eprintln!("shadow: mismatch — restoring vol then playing requested track");
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::SetVolume {
                        url: self.active_url.clone(),
                        value: previous_saved_volume,
                    });
                }
                self.last_play_was_shadow = false;
            }
            shadow_stream::PromoteResult::Miss => {
                self.last_play_was_shadow = false;
            }
        }
        }  // end if is_mozart_driver — shadow path

        // Tag this as a user-initiated request so the TrackForPlay
        // result handler knows to actually POST to the device.
        self.last_user_play_request = Some(tid);

        // Fire a low-frequency anticipation cue immediately (subject to
        // the aggression policy). This is a tempo+key-matched build-up
        // that plays on the local controller machine (paplay) — its job
        // is to give the user audible "the system heard you" feedback
        // within ~10 ms of the tap, then exponentially decay so the
        // device's real audio crossfades naturally on top. The tonic
        // frequency is sourced from the now-playing key chip if known.
        // Best-effort; failures are silent.
        if self.aggression.cue_duration_ms() > 0 {
            let cue_bpm = if self.np_bpm > 0.0 { self.np_bpm as f32 }
                          else { pre_buffer::DEFAULT_BPM };
            let cue_hz = if !self.np_camelot.is_empty() {
                pre_buffer::tonic_hz_for_camelot(&self.np_camelot)
            } else { pre_buffer::DEFAULT_TONIC_HZ };
            // Adaptive estimated-device-latency. The RTT tracker has been
            // accumulating samples from every GetVolume; recommend_cue_ms
            // returns a duration that ~1.2× the observed p95 so the
            // device's real audio arrives during the cue's decay tail.
            // Falls back to 350 ms when fewer than 3 samples seen.
            let est_latency = self.rtt.recommend_cue_ms();
            let policy_pref = self.aggression.cue_duration_ms();
            // Honor the larger of the policy's preference and the RTT-
            // derived recommendation. The cue should never undercut the
            // real-audio arrival; the policy preference is a minimum,
            // the RTT-derived value is what the network actually needs.
            let target_ms = policy_pref.max(est_latency);
            let sched = pre_buffer::schedule_for_track(
                cue_bpm,
                target_ms,
                est_latency,
            );
            let harmonics = self.aggression.cue_harmonics();
            self.fire_estimate_cue(cue_bpm, cue_hz, sched.duration_ms, harmonics);
        }

        // Fast path: if the track was pre-staged (URL already exposed),
        // skip the resolve roundtrip.
        //
        // Only valid for the Mozart REST path. Non-Mozart drivers
        // (bluetooth/local/multiroom) play the local file directly
        // via paplay — they need a filesystem path, not an HTTP URL.
        // For those drivers we fall through to ResolveForPlay so the
        // dispatch site can route to DriverOp::PlayLocalFile correctly.
        // Without this guard, prestaged tracks would fire PlayResolvedUrl
        // unconditionally — which on a `mozart_legacy` Stage means
        // every queued track hits the dead REST endpoints and floods
        // the log with "ALL ENDPOINTS FAILED" attempts.
        if is_mozart_driver {
            if let Some(track_url) = self.prestaged_track_urls.get(&tid).cloned() {
                self.last_play_status = "requesting play (pre-staged)…".to_string();
                self.last_play_url = track_url.clone();
                self.play_status_changed();
                self.send_media(media::MediaCmd::RecordPlay(tid));
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::PlayResolvedUrl {
                        url: self.active_url.clone(),
                        track_url,
                        title: format!("track {}", track_id),
                    });
                }
                return;
            }
        }
        self.last_play_status = "resolving…".to_string();
        self.play_status_changed();
        self.send_media(media::MediaCmd::ResolveForPlay(tid));
    }

    /// Stop device-side playback. Best-effort POST to the Mozart stop
    /// endpoint; logged but not retried.
    fn stop_track_playback(&mut self) {
        let url = match self.active_url_opt() {
            Some(u) => u,
            None => return,
        };
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::StopPlayback { url });
        }
    }

    /// Clear the play-attempt status banner. Doesn't touch actual playback.
    fn dismiss_play_status(&mut self) {
        self.last_play_status = String::new();
        self.last_play_url    = String::new();
        self.play_status_changed();
    }

    /// Play the identification ping on a target device. The ping is a
    /// distinctive two-tone sound (A4 + E5 — a perfect-fifth interval)
    /// the user can hear, so when they have multiple speakers in
    /// different rooms they can audibly find the one they're tapping
    /// on. The pre-warm sequence (2 × GetVolume) shifts the cold-
    /// connection latency off the play command's critical path.
    ///
    /// Doesn't change the active device. The ping fires at whatever
    /// volume the target device is currently at; the design uses
    /// moderately-loud mid-range tones so it's audible without being
    /// alarming.
    fn ping_device(&mut self, url: String, product_id: String) {
        if url.is_empty() || !is_well_formed_http_url(&url) {
            eprintln!("ping_device: bad url '{}'", url);
            return;
        }

        // Lazy-init: bind the track server (if not already), synthesize
        // the ping WAV, register it on the server. The path token is
        // cached so we only do this once per session.
        if self.track_server.is_none() {
            match track_server::TrackServer::bind(track_server::DEFAULT_PORT) {
                Ok(s) => self.track_server = Some(Arc::new(s)),
                Err(_) => match track_server::TrackServer::bind(0) {
                    Ok(s) => self.track_server = Some(Arc::new(s)),
                    Err(e) => {
                        eprintln!("ping_device: track_server bind failed: {}", e);
                        return;
                    }
                }
            }
        }
        if self.ping_path_cache.is_empty() {
            let wav = device_ping::synthesize_ping_wav();
            let server = self.track_server.as_ref().unwrap();
            let seg = server.expose_bytes("audio/wav", wav);
            self.ping_path_cache = seg;
            eprintln!("ping_device: ping WAV registered at {}", self.ping_path_cache);
        }

        // Build the device-fetchable URL using the target device's IP
        // so the track server picks the right local interface.
        let server = self.track_server.as_ref().unwrap();
        let peer_ip = url_to_ip(&url);
        let ping_url = match peer_ip {
            Some(ip) => server.url_for(ip, &self.ping_path_cache),
            None => return,
        };

        eprintln!("ping_device: pinging {} (product={})", url, product_id);

        // Pre-warm: 2 × GetVolume to wake the device's network stack.
        // These ride the circuit breaker — if the device is unreachable,
        // they fast-fail and the play below also fast-fails. Cheap.
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::GetVolume { url: url.clone() });
            let _ = tx.send(Command::GetVolume { url: url.clone() });
        }

        // Play the ping. PlayResolvedUrl POSTs to the device's network-
        // audio source; the device fetches our local URL and plays it.
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::PlayResolvedUrl {
                url,
                track_url: ping_url,
                title: "Device Ping".to_string(),
            });
        }
    }

    // ── Bluetooth auto-connect ───────────────────────────────────────

    /// Flip the auto-connect toggle for a Bluetooth device. The MAC
    /// uniquely identifies the device across reboots and pairings;
    /// the name is for UI display. When `enabled` becomes true, the
    /// next `tick_bt_autoconnect` will fire a connect attempt.
    fn set_device_auto_connect(&mut self, mac: String, name: String, enabled: bool) {
        let now = std::time::Instant::now();
        self.bt_autoconnect.set_auto_connect(&mac, &name, enabled, now);
        self.refresh_bt_devices_json();
        // Persist immediately so the choice survives a restart.
        self.send_media(media::MediaCmd::SaveAppKv {
            key: "bt_autoconnect_devices".to_string(),
            value: self.bt_autoconnect.to_json(),
        });
        eprintln!("bt: {} for {} ({})",
            if enabled { "enabled auto-connect" } else { "disabled auto-connect" },
            name, mac);
    }

    /// Periodic tick. Driven by a 10-second QML Timer. For each device
    /// whose backoff has elapsed, spawn `bluetoothctl connect <MAC>`
    /// in a background thread; when it finishes, report success/failure.
    fn tick_bt_autoconnect(&mut self) {
        let now = std::time::Instant::now();
        let due = self.bt_autoconnect.devices_due(now);
        if due.is_empty() { return; }
        for (mac, name) in due {
            self.bt_autoconnect.note_attempt_started(&mac, now);
            self.spawn_bluetoothctl_connect(mac.clone(), name);
        }
        self.refresh_bt_devices_json();
    }

    /// Spawn a thread running `bluetoothctl connect <MAC>`. The result
    /// arrives back via the result channel as a synthetic BtConnect
    /// outcome the QtState handler consumes. Defensive: the binary may
    /// be missing entirely, in which case we fail fast.
    fn spawn_bluetoothctl_connect(&mut self, mac: String, name: String) {
        // Record the attempt in the respect ledger BEFORE spawning,
        // so the audit log shows the intent even if the spawn fails.
        self.bt_respect.note_connect_attempt(&mac, &name);
        self.refresh_bt_respect();
        let result_tx = match &self.tx {
            Some(_) => self.bt_result_tx.clone(),
            None => None,
        };
        std::thread::spawn(move || {
            use std::process::Command as ProcCommand;
            let started = std::time::Instant::now();
            let out = ProcCommand::new("bluetoothctl")
                .args(["--timeout", "8", "connect", &mac])
                .output();
            let success = match &out {
                Ok(o) => {
                    let stdout = String::from_utf8_lossy(&o.stdout).to_lowercase();
                    let stderr = String::from_utf8_lossy(&o.stderr).to_lowercase();
                    let combined = format!("{} {}", stdout, stderr);
                    // bluetoothctl "Connection successful" is the canonical
                    // marker. "Failed to connect" or "Not available" is failure.
                    combined.contains("connection successful")
                }
                Err(_) => false,
            };
            let elapsed = started.elapsed();
            eprintln!("bt: bluetoothctl connect {} ({}) → {} in {:?}",
                mac, name, if success { "OK" } else { "FAIL" }, elapsed);
            if let Some(tx) = result_tx {
                let _ = tx.send(BtAttemptResult { mac, success });
            }
        });
    }

    /// Rebuild bt_devices_json from the current scheduler state so QML
    /// can render the list.
    fn refresh_bt_devices_json(&mut self) {
        use bt_autoconnect::AttemptState;
        let now = std::time::Instant::now();
        let mut entries = Vec::new();
        for (mac, cfg) in self.bt_autoconnect_iter() {
            let status = match &cfg.state {
                AttemptState::InFlight { .. } => "connecting",
                AttemptState::Idle { next_attempt_at } => {
                    if !cfg.auto_connect { "idle" }
                    else if cfg.last_success.map(|s| now.duration_since(s)
                            < std::time::Duration::from_secs(120)).unwrap_or(false) {
                        "connected"
                    }
                    else if *next_attempt_at > now { "backoff" }
                    else { "queued" }
                }
            };
            entries.push(serde_json::json!({
                "mac": mac,
                "name": cfg.name,
                "auto_connect": cfg.auto_connect,
                "status": status,
            }));
        }
        self.bt_devices_json = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into());
        self.bt_devices_changed();
    }

    /// Helper iterator clone — the BtAutoConnect doesn't expose iter()
    /// directly, so this wraps it. Used by refresh_bt_devices_json.
    fn bt_autoconnect_iter(&self) -> Vec<(String, bt_autoconnect::DeviceConfig)> {
        // Enabled + disabled both — UI lists everything you've toggled.
        let json = self.bt_autoconnect.to_json();
        let arr: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap_or_default();
        arr.into_iter().filter_map(|e| {
            let mac = e.get("mac").and_then(|v| v.as_str())?.to_string();
            let cfg = self.bt_autoconnect.get(&mac)?.clone();
            Some((mac, cfg))
        }).collect()
    }

    /// Look up the friendly name registered for a MAC in the
    /// auto-connect table. Used by the BtRespect ledger so logged
    /// entries carry human-readable device names.
    fn bt_autoconnect_name_for(&self, mac: &str) -> Option<String> {
        self.bt_autoconnect.get(mac).map(|c| c.name.clone())
    }

    /// Push current BtRespect summary + log to the QML-facing properties
    /// and emit the change signal. Called after any ledger mutation so
    /// the Settings page audit refreshes live.
    fn refresh_bt_respect(&mut self) {
        self.bt_respect_summary = self.bt_respect.summary();
        self.bt_respect_log_json = self.bt_respect.log_json();
        self.bt_respect_idle_minutes =
            self.bt_respect.idle_threshold.as_secs() as i32 / 60;
        self.bt_respect_changed();
    }

    /// QML-facing setter for the idle threshold. Clamps to 1..=60.
    /// Persists the choice in app_kv so it survives restarts.
    fn set_bt_respect_idle_minutes(&mut self, m: i32) {
        let clamped = m.clamp(1, 60) as u64;
        self.bt_respect.idle_threshold = std::time::Duration::from_secs(clamped * 60);
        eprintln!("bt_respect: idle threshold → {} min", clamped);
        self.bt_respect.note_activity();
        self.send_media(media::MediaCmd::SaveAppKv {
            key: "bt_respect_idle_minutes".to_string(),
            value: clamped.to_string(),
        });
        self.refresh_bt_respect();
    }

    /// Spawn a detached `bluetoothctl show` thread to learn whether
    /// the system BT adapter is powered. Result lands in
    /// `bt_power_rx`; drained from `poll_results`. `is_initial`
    /// piggybacks on the channel so the first poll feeds into
    /// `bt_respect.observe_initial_power` (so we know what to
    /// restore on shutdown).
    fn spawn_bt_power_probe(&mut self, is_initial: bool) {
        if self.bt_power_rx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<(bool, bool)>();
            self.bt_power_rx = Some(rx);
            // Probe spawns share the channel by re-using the existing
            // rx; tx is moved into the spawned thread.
            std::thread::spawn(move || {
                use std::process::Command as ProcCommand;
                let powered = match ProcCommand::new("bluetoothctl").arg("show").output() {
                    Ok(out) => String::from_utf8_lossy(&out.stdout)
                        .to_lowercase()
                        .contains("powered: yes"),
                    Err(_) => false,
                };
                let _ = tx.send((powered, is_initial));
            });
        } else {
            // Channel already exists — get a clone of its sender.
            // The std mpsc API doesn't expose a "clone sender from
            // receiver" directly; we use a fresh channel and drain
            // the old one into it. Simpler: build a second channel
            // alongside. For our use the load is low (one poll per
            // few seconds) so just rebuild.
            let (tx, rx) = std::sync::mpsc::channel::<(bool, bool)>();
            // Best-effort: drain anything pending on the old rx.
            if let Some(old) = self.bt_power_rx.take() {
                while let Ok(v) = old.try_recv() {
                    let _ = tx.send(v);
                }
            }
            self.bt_power_rx = Some(rx);
            std::thread::spawn(move || {
                use std::process::Command as ProcCommand;
                let powered = match ProcCommand::new("bluetoothctl").arg("show").output() {
                    Ok(out) => String::from_utf8_lossy(&out.stdout)
                        .to_lowercase()
                        .contains("powered: yes"),
                    Err(_) => false,
                };
                let _ = tx.send((powered, is_initial));
            });
        }
    }

    /// QML-facing: poll bluetoothctl show now to refresh the power
    /// indicator. Cheap; called every few seconds from a Timer.
    fn refresh_bt_system_powered(&mut self) {
        self.spawn_bt_power_probe(false);
    }

    /// Spawn `bluetoothctl devices Paired` in a background thread.
    /// Results land in `bt_discovery_rx`, drained from poll_results.
    ///
    /// We use a fresh channel each call (vs. cloning a sender) for
    /// the same reason as `spawn_bt_power_probe`: std mpsc's
    /// receiver doesn't expose sender-cloning, and the call rate is
    /// low enough that rebuilding is fine. Any previously-queued
    /// result on the old rx is drained into the new one so we don't
    /// lose data if probes overlap.
    ///
    /// The thread has no internal timeout — bluetoothctl typically
    /// returns in <500ms. If BlueZ is genuinely wedged the thread
    /// can stay alive for a while; we accept that trade-off because
    /// the QML thread stays responsive and the wedged subprocess
    /// will eventually be reaped (or stay until process exit).
    fn spawn_bt_discovery_probe(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
        if let Some(old) = self.bt_discovery_rx.take() {
            while let Ok(v) = old.try_recv() {
                let _ = tx.send(v);
            }
        }
        self.bt_discovery_rx = Some(rx);
        std::thread::spawn(move || {
            use std::process::Command as ProcCommand;
            let output = match ProcCommand::new("bluetoothctl")
                .args(["devices", "Paired"])
                .output()
            {
                Ok(o) => o,
                Err(e) => {
                    eprintln!(
                        "bt_discovery: bluetoothctl failed: {} \
                        (is the bluez tool installed?)", e);
                    let _ = tx.send(Vec::new());
                    return;
                }
            };
            let stdout = String::from_utf8_lossy(&output.stdout);
            let parsed = bt_autoconnect::parse_bluetoothctl_devices(&stdout);
            eprintln!("bt_discovery: {} paired device(s) seen", parsed.len());
            let _ = tx.send(parsed);
        });
    }

    /// Drain async BT discovery results into the auto-connect table.
    /// Called from poll_results each tick. Any newly-discovered MACs
    /// are merged in as disabled-by-default; persistence is written
    /// only when something actually changed (to avoid hot churn on
    /// every poll).
    fn drain_bt_discovery(&mut self) {
        let drained: Vec<Vec<(String, String)>> = {
            let mut acc = Vec::new();
            if let Some(rx) = &self.bt_discovery_rx {
                while let Ok(v) = rx.try_recv() {
                    acc.push(v);
                }
            }
            acc
        };
        if drained.is_empty() { return; }
        let now = std::time::Instant::now();
        let mut any_added = 0_usize;
        for parsed in drained {
            let added = self.bt_autoconnect.merge_discovered(&parsed, now);
            any_added += added;
        }
        if any_added > 0 {
            eprintln!("bt_discovery: merged {} new device(s) into auto-connect table",
                      any_added);
            self.send_media(media::MediaCmd::SaveAppKv {
                key: "bt_autoconnect_devices".to_string(),
                value: self.bt_autoconnect.to_json(),
            });
            self.refresh_bt_devices_json();
        } else {
            // No new MACs but still refresh JSON so freshly-paired
            // names update in the UI.
            self.refresh_bt_devices_json();
        }
    }

    /// QML-facing: turn the BT adapter on or off. Runs
    /// `bluetoothctl power on|off` in a detached thread. Records
    /// the intent in BtRespect so shutdown knows what to restore.
    fn set_bt_system_powered(&mut self, on: bool) {
        eprintln!("bt_power: requesting power {}", if on { "on" } else { "off" });
        if on {
            self.bt_respect.note_powered_on();
        }
        self.refresh_bt_respect();
        let arg = if on { "on" } else { "off" };
        std::thread::spawn(move || {
            use std::process::Command as ProcCommand;
            let r = ProcCommand::new("bluetoothctl")
                .args(["power", arg])
                .output();
            match r {
                Ok(o) => eprintln!(
                    "bt_power: bluetoothctl power {} → {}",
                    arg,
                    String::from_utf8_lossy(&o.stdout).trim()
                ),
                Err(e) => eprintln!("bt_power: spawn failed: {}", e),
            }
        });
        // Schedule a follow-up poll so the UI updates within ~500ms.
        // The poll happens via the QML Timer; we just nudge it here.
        // Nothing else to do — the user-visible result lands when
        // bt_system_powered changes via the channel drain.
        self.bt_respect.note_activity();
    }

    /// Drain the BT power probe channel. Called from poll_results.
    /// First result (with is_initial=true) feeds the BtRespect
    /// snapshot. Subsequent ones just update the live property.
    fn drain_bt_power(&mut self) {
        let mut drained: Vec<(bool, bool)> = Vec::new();
        if let Some(rx) = &self.bt_power_rx {
            loop {
                match rx.try_recv() {
                    Ok(t) => drained.push(t),
                    Err(_) => break,
                }
            }
        }
        if drained.is_empty() { return; }
        for (powered, is_initial) in drained {
            if is_initial {
                eprintln!("bt_respect: system BT initially powered={}", powered);
                self.bt_respect.observe_initial_power(powered);
            }
            if self.bt_system_powered != powered {
                self.bt_system_powered = powered;
                // bt_respect_changed signal triggers UI updates for
                // bt_system_powered too (they share NOTIFY).
                self.refresh_bt_respect();
            }
        }
    }

    /// Capture the initial BT connection state at startup. For each
    /// paired MAC in the auto-connect table, runs `bluetoothctl info
    /// <MAC>` (in a detached thread, so we don't block startup) and
    /// records whether the device was Connected.
    ///
    /// The result feeds into BtRespect via observe_initial. Once all
    /// MACs report back, mark_snapshot_done is called and the cleanup
    /// path becomes active.
    fn snapshot_bt_state(&mut self) {
        let macs_names: Vec<(String, String)> = self.bt_autoconnect_iter()
            .into_iter()
            .map(|(mac, cfg)| (mac, cfg.name))
            .collect();
        if macs_names.is_empty() {
            // No paired devices to snapshot — mark done immediately
            // so the cleanup path still works (it'll just have an
            // empty set to act on).
            self.bt_respect.mark_snapshot_done();
            self.refresh_bt_respect();
            eprintln!("bt_respect: no paired devices to snapshot");
            return;
        }
        eprintln!("bt_respect: snapshotting {} paired device(s)", macs_names.len());
        // Channel for snapshot results back to the main loop.
        let (snap_tx, snap_rx) = std::sync::mpsc::channel::<(String, String, bool)>();
        self.bt_snapshot_rx = Some(snap_rx);
        let count = macs_names.len();
        self.bt_snapshot_pending = count;
        for (mac, name) in macs_names {
            let tx = snap_tx.clone();
            std::thread::spawn(move || {
                use std::process::Command as ProcCommand;
                let out = ProcCommand::new("bluetoothctl")
                    .args(["info", &mac])
                    .output();
                let connected = match out {
                    Ok(o) => {
                        let body = String::from_utf8_lossy(&o.stdout).to_lowercase();
                        // bluetoothctl info prints "Connected: yes" /
                        // "Connected: no" on the paired device. Anything
                        // else (device unknown, daemon down, etc.) we
                        // treat as "not connected" for safety.
                        body.contains("connected: yes")
                    }
                    Err(_) => false,
                };
                let _ = tx.send((mac, name, connected));
            });
        }
    }

    /// Drain pending snapshot results from the channel. Called from
    /// poll_results so the main loop sees them as they arrive without
    /// blocking. Marks snapshot_done when all expected results land.
    fn drain_bt_snapshot(&mut self) {
        let mut drained: Vec<(String, String, bool)> = Vec::new();
        if let Some(rx) = &self.bt_snapshot_rx {
            loop {
                match rx.try_recv() {
                    Ok(t) => drained.push(t),
                    Err(_) => break,
                }
            }
        }
        if drained.is_empty() { return; }
        for (mac, name, was_connected) in drained {
            eprintln!("bt_respect: snapshot {} ({}) → was_connected={}",
                      mac, name, was_connected);
            self.bt_respect.observe_initial(&mac, &name, was_connected);
            self.bt_snapshot_pending = self.bt_snapshot_pending.saturating_sub(1);
        }
        if self.bt_snapshot_pending == 0 {
            self.bt_respect.mark_snapshot_done();
            self.bt_snapshot_rx = None;
            eprintln!("bt_respect: snapshot complete — {}",
                      self.bt_respect.summary());
        }
        self.refresh_bt_respect();
    }

    /// Disconnect any MACs the respect ledger tags as owned-by-us
    /// (we connected them, they weren't connected before we started).
    /// Called from QML on app close (via Window.onClosing) and on
    /// idle-timeout. Idempotent — running it twice does nothing the
    /// second time because note_disconnect_attempt clears the
    /// we_connected set.
    fn bt_cleanup_owned(&mut self) {
        let to_disconnect = self.bt_respect.macs_to_disconnect_on_shutdown();
        let restore_power_off = self.bt_respect.should_restore_power_off();
        if to_disconnect.is_empty() && !restore_power_off {
            eprintln!("bt_respect: nothing to clean up");
            return;
        }
        if !to_disconnect.is_empty() {
            eprintln!("bt_respect: cleaning up {} owned MAC(s) on app close",
                      to_disconnect.len());
        }
        for (mac, name) in &to_disconnect {
            self.bt_respect.note_disconnect_attempt(mac, "app close / cleanup");
            let mac_c = mac.clone();
            let name_c = name.clone();
            std::thread::spawn(move || {
                use std::process::Command as ProcCommand;
                let out = ProcCommand::new("bluetoothctl")
                    .args(["--timeout", "4", "disconnect", &mac_c])
                    .output();
                let ok = match &out {
                    Ok(o) => String::from_utf8_lossy(&o.stdout)
                                .to_lowercase()
                                .contains("disconnect"),
                    Err(_) => false,
                };
                eprintln!("bt_respect: disconnect {} ({}) → {}",
                          mac_c, name_c, if ok { "OK" } else { "FAIL/IGNORED" });
            });
        }
        if restore_power_off {
            // We powered the adapter on during the session AND it
            // was off at startup. Restore the user's original system
            // state by powering it back off.
            eprintln!("bt_respect: restoring BT power-off (we turned it on)");
            std::thread::spawn(move || {
                use std::process::Command as ProcCommand;
                let r = ProcCommand::new("bluetoothctl")
                    .args(["--timeout", "4", "power", "off"])
                    .output();
                match r {
                    Ok(o) => eprintln!(
                        "bt_respect: bluetoothctl power off → {}",
                        String::from_utf8_lossy(&o.stdout).trim()
                    ),
                    Err(e) => eprintln!("bt_respect: power-off spawn failed: {}", e),
                }
            });
        }
        self.refresh_bt_respect();
    }

    /// Check idle threshold. If exceeded and no audio is currently
    /// playing through our drivers, run the cleanup. Called every 30s
    /// from a QML Timer.
    fn tick_bt_respect_idle(&mut self) {
        // If audio is actively flowing through a driver we own, don't
        // disconnect — the user is using these devices NOW.
        let driving_audio = matches!(self.current_driver_kind.as_str(),
                                     "bluetooth" | "multiroom");
        if driving_audio {
            // Active use counts as activity — reset the idle clock so
            // we don't disconnect mid-playback if the playing track
            // happens to be long.
            self.bt_respect.note_activity();
            return;
        }
        let now = std::time::Instant::now();
        if self.bt_respect.idle_expired(now) {
            eprintln!("bt_respect: idle threshold reached — running cleanup");
            self.bt_cleanup_owned();
            // Reset the clock so we don't re-trigger every tick.
            self.bt_respect.note_activity();
        }
    }

    /// Non-blocking drain of the BT result channel. For each completed
    /// bluetoothctl attempt, feed the outcome into the scheduler.
    fn drain_bt_results(&mut self) {
        let now = std::time::Instant::now();
        // Drain BEFORE borrowing rx — we'll need mutable self for
        // bt_respect.note_connect_result. Pull all messages into a
        // local vec first, then process.
        let mut drained: Vec<BtAttemptResult> = Vec::new();
        if let Some(rx) = &self.bt_result_rx {
            loop {
                match rx.try_recv() {
                    Ok(res) => drained.push(res),
                    Err(_) => break,
                }
            }
        }
        if drained.is_empty() { return; }
        for res in &drained {
            self.bt_autoconnect.note_attempt_result(&res.mac, res.success, now);
            // Look up the friendly name from devices_json if we have
            // it; the respect log wants both. Falls back to "" which
            // the audit page tolerates.
            let name = self.bt_autoconnect_name_for(&res.mac)
                .unwrap_or_default();
            self.bt_respect.note_connect_result(&res.mac, &name, res.success);
        }
        self.refresh_bt_devices_json();
        self.refresh_bt_respect();
    }

    /// Audit the system for needed command-line utilities. Updates the
    /// kde_tools_json + kde_tools_install_cmd properties so the
    /// settings UI can render the result. Runs `which <bin>` plus
    /// `<bin> --version` per tool, which is cheap (~5ms on a warm
    /// system). Called on demand from QML when the user opens the
    /// settings page.
    fn audit_kde_tools(&mut self) {
        let results = kde_tools::audit_all();
        let missing_count = results.iter()
            .filter(|(_, s)| !matches!(s, kde_tools::InstallStatus::Present))
            .count();
        eprintln!("kde_tools: {}/{} tools present",
                  results.len() - missing_count, results.len());
        let install_cmd = kde_tools::apt_install_command(&results).unwrap_or_default();
        self.kde_tools_json = kde_tools::audit_to_json(&results);
        self.kde_tools_install_cmd = install_cmd;
        self.kde_tools_changed();
    }

    /// Refresh the deps audit shown on SettingsPage. Detects the
    /// package manager, walks the known runtime tools, and produces
    /// a JSON list the QML can render. Missing tools include their
    /// formatted install plan so the user can copy + paste.
    fn refresh_dep_audit(&mut self) {
        let pm = dep_wrangler::detect();
        self.dep_pm_label = pm.label().to_string();

        // The tools we care about. Each entry is (binary, friendly_name).
        // Mirrors the install-deps.sh list so the audit reflects what
        // the script would install.
        //
        // PulseAudio AND PipeWire CLIs are both listed because the
        // controller falls back PulseAudio → PipeWire → ALSA when
        // playing local audio (cue tones, BT routing). On a stock
        // KDE-on-glibc system `paplay` works either against PulseAudio
        // or PipeWire's pulse-compat layer. On a PipeWire-only Alpine
        // / postmarketOS install without the pipewire-pulse package
        // installed, `paplay` is absent and `pw-cat` becomes the only
        // working path — so we now spawn it as a middle fallback and
        // surface its missing-ness here.
        let tools: &[(&str, &str)] = &[
            ("paplay",        "PulseAudio playback"),
            ("pw-cat",        "PipeWire playback"),
            ("aplay",         "ALSA playback"),
            ("bluetoothctl",  "Bluetooth (bluez)"),
            ("ffmpeg",        "FFmpeg"),
            ("avahi-daemon",  "Avahi (mDNS)"),
            // libopus runtime — required by the soundscape creator
            // for OggOpus encoding. We detect via the shared library
            // file rather than a binary (libopus has no CLI tool).
            ("libopus.so.0",  "Opus codec (libopus)"),
        ];

        let mut entries: Vec<serde_json::Value> = Vec::with_capacity(tools.len());
        let mut missing = 0usize;
        for (bin, name) in tools {
            let installed = dep_wrangler::verify_installed(bin);
            let plan_text = if installed {
                String::new()
            } else {
                missing += 1;
                let plan = dep_wrangler::plan_for_tool(bin, name, pm);
                dep_wrangler::plan_to_text(&plan)
            };
            entries.push(serde_json::json!({
                "binary":        bin,
                "friendly_name": name,
                "installed":     installed,
                "plan_text":     plan_text,
            }));
        }
        eprintln!("dep_audit: pm={}, {}/{} runtime tools present",
                  pm.label(), tools.len() - missing, tools.len());
        self.dep_audit_json =
            serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into());
        self.dep_audit_changed();
    }

    /// Kick a one-shot probe of the active device's source. Result
    /// arrives async via ResultMessage::ActiveSourceProbed.
    fn refresh_active_source(&mut self) {
        let url = self.active_url.clone();
        if url.is_empty() {
            eprintln!("refresh_active_source: no active_url set");
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::ProbeActiveSource { url });
        }
    }

    /// Attempt to programmatically switch the active device's source
    /// to Network/DLNA. The worker tries several REST candidates;
    /// result lands in network_switch_json. If successful, the
    /// device's active source is re-probed automatically (so the
    /// warning banner clears without the user tapping Refresh).
    fn try_activate_network(&mut self) {
        let url = self.active_url.clone();
        if url.is_empty() {
            self.network_switch_json = serde_json::json!({
                "success": false,
                "error": "no active device"
            }).to_string();
            self.network_switch_changed();
            return;
        }
        eprintln!("try_activate_network: dispatching for {}", url);
        self.network_switch_running = true;
        // Don't clear the previous attempt's JSON until the new one
        // returns — keeps the UI showing the prior outcome instead
        // of flickering to empty while waiting.
        self.network_switch_changed();
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::TryActivateNetwork { url });
        }
    }

    /// Kick a one-shot firmware-family probe of the active device.
    /// Result arrives async via ResultMessage::FirmwareProbed.
    fn refresh_firmware_family(&mut self) {
        let url = self.active_url.clone();
        if url.is_empty() {
            eprintln!("refresh_firmware_family: no active_url set");
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::ProbeFirmware { url });
        }
    }

    /// Called when a track ends naturally (paplay exited 0 via the
    /// driver). Find what just played in queue_json and trigger the
    /// next track. If nothing was queued or we're already at the
    /// end, clears the playing state silently.
    ///
    /// Matching: we use `last_user_play_request` (set by play_track)
    /// or the most recently recorded RecordPlay to identify "what
    /// just ended". The queue is ordered by `ord` ascending; we play
    /// the entry immediately after the matched one.
    fn advance_queue_if_any(&mut self) {
        // ── Repeat-one short-circuit ──
        //
        // Before consulting the queue at all: if the user is in
        // RepeatMode::One, replay the just-finished track. This
        // covers the "loop one song" use case (lullaby on repeat,
        // a passage to memorize, etc) without requiring a queue.
        //
        // We deliberately re-play even if the queue is non-empty,
        // because "repeat ONE" means exactly that — the queue
        // resumes only when the user toggles repeat off or to All.
        let repeat = playback_modes::load_repeat_mode();
        if repeat == playback_modes::RepeatMode::One && self.last_played_track_id != 0 {
            eprintln!("repeat=one: replaying track {}", self.last_played_track_id);
            self.play_track(self.last_played_track_id as i32);
            return;
        }

        let q = match serde_json::from_str::<Vec<serde_json::Value>>(&self.queue_json) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("queue: nothing queued — playback ends here");
                self.last_play_status = "Playback finished".to_string();
                // Track finished and nothing's queued: flip the playbar
                // to a paused state. We KEEP np_title/np_artist so the
                // bar still shows what just played (and the ▶ button can
                // replay it), but np_playing=false so the glyph reads ▶
                // not ❚❚. Without this the bar claimed to still be
                // playing after paplay had already exited.
                self.np_playing = false;
                self.now_playing_changed();
                self.play_status_changed();
                return;
            }
        };
        // Identify the track that just played. Best signal: the most
        // recent user-requested track in this device's session. If
        // that's unavailable, take the queue's first entry (start of
        // playlist) so a fresh tap+ended cycle still flows.
        let last_played = self.last_played_track_id;
        let mut next_track_id: Option<i64> = None;
        if last_played != 0 {
            let mut found = false;
            for entry in &q {
                let tid = entry.get("track_id").and_then(|v| v.as_i64()).unwrap_or(0);
                if found {
                    next_track_id = Some(tid);
                    break;
                }
                if tid == last_played {
                    found = true;
                }
            }
        }
        // Fallback: just play the first queued track if the last-played
        // wasn't found in the queue (e.g. user played from Library
        // outside the queue, then the queue is independent of that).
        if next_track_id.is_none() {
            eprintln!("queue: last-played track {} not in queue — no advance",
                      last_played);
            self.last_play_status = "Playback finished".to_string();
            self.play_status_changed();
            return;
        }
        let next_id = next_track_id.unwrap() as i32;

        // ── Child mode skip ──
        //
        // Before actually playing, run the title+filename through
        // the blacklist. If it matches, log + recurse to look for
        // the next-next track. We track child_skip_depth to avoid
        // infinite recursion on a queue full of matching titles.
        if playback_modes::load_child_mode() {
            if let Some((title, path)) = self.lookup_track_meta_for_skip(next_id as i64) {
                if let Some(reason) = playback_modes::should_skip_in_child_mode(&title, &path) {
                    eprintln!("child_mode: skipping track {} '{}' — {}",
                              next_id, title, reason);
                    // Advance past this track by pretending it
                    // just ended (set last_played_track_id then
                    // recurse). We DON'T call play_track on the
                    // blocked track first.
                    self.last_played_track_id = next_id as i64;
                    // Guard against recursing too deep on a fully
                    // blacklisted queue.
                    self.child_skip_depth = self.child_skip_depth.saturating_add(1);
                    if self.child_skip_depth > 100 {
                        eprintln!("child_mode: skipped 100 tracks in a row \
                                   — giving up. Disable Child Mode or pick \
                                   a different playlist.");
                        self.last_play_status =
                            "Child mode: no clean tracks left in queue.".to_string();
                        self.play_status_changed();
                        self.child_skip_depth = 0;
                        return;
                    }
                    self.advance_queue_if_any();
                    return;
                }
            }
        }
        self.child_skip_depth = 0;

        eprintln!("queue: advancing to track {}", next_id);
        self.play_track(next_id);
    }

    /// Look up a track's title + path by id for the child-mode
    /// filter. Returns None if the track isn't in the in-memory
    /// library cache (which would be unusual at queue-advance time
    /// since the queue is built from the library).
    fn lookup_track_meta_for_skip(&self, tid: i64) -> Option<(String, String)> {
        // Use the prestaged URL map to find the path; otherwise
        // walk the recent JSON cache. Failing those, we ask the
        // media worker (synchronously) — but that's slow, so we
        // skip the check and let the track play rather than block.
        if let Some(s) = self.prestaged_track_urls.get(&tid) {
            // s is a URL like http://controller/t/<token>. We don't
            // have the title here; fall through to JSON cache.
            let _ = s;
        }
        // Walk the recent_json and artists_json for any entry with
        // this id, take title + path from whichever has them.
        let try_caches = [&self.library_json, &self.artists_json];
        for cache in try_caches {
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(cache) {
                for entry in &arr {
                    if entry.get("id").and_then(|v| v.as_i64()) == Some(tid) {
                        let title = entry.get("title")
                            .and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let path = entry.get("path")
                            .and_then(|v| v.as_str()).unwrap_or("").to_string();
                        return Some((title, path));
                    }
                }
            }
        }
        None
    }

    /// Rebuild the current_driver from the detected firmware family +
    /// any associated Bluetooth MAC. Called every time those inputs
    /// change: device switch, firmware probe result, BT MAC assignment.
    fn rebuild_driver(&mut self) {
        let family = firmware_family::family_from_tag(&self.device_firmware_family);
        // For non-Mozart families, opportunistically auto-associate a
        // paired BT MAC. The match is by friendly name: the active
        // device's product_name vs the paired BT device's name, both
        // lowercased + whitespace-collapsed. This means devices the
        // user has already paired (e.g. via the Connect page) get
        // picked up automatically without manual MAC entry.
        if family.recommends_bluetooth_fallback()
            && self.active_bt_mac.is_empty()
        {
            self.auto_pair_bt_mac_by_name();
        }
        // Build a fresh reqwest client (cheap) so the driver isn't
        // borrowing one we'd have to plumb through ownership.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let bt_mac = if self.active_bt_mac.is_empty() {
            None
        } else {
            Some(self.active_bt_mac.as_str())
        };
        let bt_name = if self.active_bt_name.is_empty() {
            None
        } else {
            Some(self.active_bt_name.as_str())
        };
        let drv = device_driver::AnyDriver::for_family(
            family, &self.active_url, bt_mac, bt_name, client,
        );
        let kind = match &drv {
            device_driver::AnyDriver::Mozart(_)    => "mozart",
            device_driver::AnyDriver::Bluetooth(_) => "bluetooth",
            device_driver::AnyDriver::Local(_)     => "local",
            device_driver::AnyDriver::MultiRoom(_) => "multiroom",
            device_driver::AnyDriver::Noop(_)      => "noop",
        };
        self.current_driver_kind  = kind.to_string();
        self.current_driver_label = drv.label();
        self.current_driver = Some(drv);
        self.driver_changed();
        eprintln!("driver: rebuilt → {} ({})",
                  self.current_driver_kind, self.current_driver_label);
    }

    /// Look at the user's known BT devices and pick one whose name
    /// resembles the active device's product_name. If found, set
    /// active_bt_mac + active_bt_name so the driver picks BT.
    ///
    /// Matching is permissive: lowercase, whitespace-collapsed,
    /// punctuation-removed substring check. "Beosound Emerge" matches
    /// "Beosound-Emerge-36184899" (the device's network-discovered
    /// name carries a serial suffix; BT advertises a cleaner name).
    fn auto_pair_bt_mac_by_name(&mut self) {
        let active_name = self.product_name.clone();
        if active_name.is_empty() { return; }
        let active_norm = normalize_device_name(&active_name);
        for (mac, cfg) in self.bt_autoconnect_iter() {
            let bt_norm = normalize_device_name(&cfg.name);
            if bt_norm.is_empty() { continue; }
            // Substring match works both directions — sometimes BT
            // name is shorter, sometimes the network name is.
            if active_norm.contains(&bt_norm) || bt_norm.contains(&active_norm) {
                eprintln!("driver: auto-paired BT '{}' ({}) → device '{}'",
                          cfg.name, mac, active_name);
                self.active_bt_mac = mac;
                self.active_bt_name = cfg.name;
                return;
            }
        }
    }

    /// Associate a Bluetooth MAC with the active device. The next
    /// rebuild_driver() picks BluetoothDriver instead of Noop when
    /// firmware is non-Mozart, enabling actual audio routing.
    fn set_active_bt_mac(&mut self, mac: String, friendly_name: String) {
        let trimmed = mac.trim().to_string();
        let name_trimmed = friendly_name.trim().to_string();
        if trimmed == self.active_bt_mac && name_trimmed == self.active_bt_name {
            return;
        }
        // ── Attack-surface defense: BT switch flooding ──
        //
        // Repeated mDNS announces from a malicious LAN peer pretending
        // to be different B&O devices could trigger rapid auto-pair
        // → set_active_bt_mac cycles. Each cycle restarts the audio
        // pipeline; a flood causes audio glitches at best, BT-stack
        // corruption at worst. Track recent switches; if we exceed
        // the budget within the window, lock out further switches
        // for a cooldown period. The legitimate user moving between
        // devices via the UI is well under the budget; an automated
        // attacker isn't.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now_ms < self.bt_switch_lockout_until_ms {
            let remaining = self.bt_switch_lockout_until_ms - now_ms;
            eprintln!("bt_switch: dropping switch to {} during flood-lockout \
                       ({} ms remaining)", trimmed, remaining);
            return;
        }
        const BT_SWITCH_WINDOW_MS: u64 = 30_000;     // 30s window
        const BT_SWITCH_MAX_IN_WINDOW: usize = 5;    // up to 5 switches
        const BT_SWITCH_COOLDOWN_MS: u64 = 60_000;   // 1-minute lockout
        let cutoff = now_ms.saturating_sub(BT_SWITCH_WINDOW_MS);
        self.bt_switch_history.retain(|&t| t >= cutoff);
        self.bt_switch_history.push(now_ms);
        if self.bt_switch_history.len() > BT_SWITCH_MAX_IN_WINDOW {
            eprintln!(
                "bt_switch: FLOOD DETECTED — {} switches in {} ms; \
                 locking out further BT MAC changes for {} ms",
                self.bt_switch_history.len(),
                BT_SWITCH_WINDOW_MS,
                BT_SWITCH_COOLDOWN_MS);
            self.bt_switch_lockout_until_ms = now_ms + BT_SWITCH_COOLDOWN_MS;
            self.bt_switch_history.clear();
            self.last_play_status =
                "Too many Bluetooth switches — locked out briefly".into();
            self.play_status_changed();
            return;
        }

        // BT route choice is user activity.
        self.bt_respect.note_activity();
        // Fresh sink → reset the unreachable watchdog. Otherwise
        // strikes accumulated on the old sink would carry across
        // and cause a spurious stop on the new one's first poll.
        self.sink_unreachable_strikes = 0;
        // If a multi-room session was active, tear it down BEFORE
        // swapping to single-speaker mode. Otherwise the combine-sink
        // module would leak in PA.
        if let Some(device_driver::AnyDriver::MultiRoom(_)) = &self.current_driver {
            if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                let _ = tx.send(Command::TeardownMultiRoom { driver: drv.clone() });
            }
        }
        // Whatever pipeline was active just got replaced. Invalidate
        // the stale-playing badge before rebuilding so the UI doesn't
        // flash; the next poll on the new driver will fill it back in
        // if appropriate.
        self.invalidate_local_playback_state();
        self.active_bt_mac = trimmed.clone();
        self.active_bt_name = name_trimmed.clone();
        self.active_multiroom_json = "null".to_string();
        eprintln!("driver: BT MAC associated → {} ({})",
                  self.active_bt_mac, self.active_bt_name);
        // Persist the binding so it sticks across restarts AND across
        // device switches. The key is per-device-URL: when the user
        // selects this device again later, we restore its sink.
        self.persist_device_sink_binding();
        self.rebuild_driver();
    }

    /// Persist the current driver binding (single-BT MAC or multi-room
    /// member set) under a per-device-URL key in app_kv. The next time
    /// the user switches to this device, restore_device_sink_binding
    /// reads it back and rebuilds the driver accordingly.
    ///
    /// JSON shape:
    ///   {"kind":"bt","mac":"AA:BB:..","name":"Speaker"}
    ///   {"kind":"multiroom","macs":["AA:..","BB:.."],"names":["a","b"]}
    ///   {"kind":"none"}   — explicit "play locally" choice
    fn persist_device_sink_binding(&mut self) {
        if self.active_url.is_empty() { return; }
        let val = if !self.active_multiroom_json.is_empty()
                  && self.active_multiroom_json != "null"
        {
            // Reuse the multiroom JSON we already track.
            format!(r#"{{"kind":"multiroom","payload":{}}}"#,
                    self.active_multiroom_json)
        } else if !self.active_bt_mac.is_empty() {
            serde_json::json!({
                "kind": "bt",
                "mac":  self.active_bt_mac,
                "name": self.active_bt_name,
            }).to_string()
        } else {
            r#"{"kind":"none"}"#.to_string()
        };
        let key = format!("device_sink:{}", self.active_url);
        self.send_media(media::MediaCmd::SaveAppKv { key, value: val });
    }

    /// Restore a previously-saved sink binding for the active device.
    /// Triggered on device switch. Async — the value comes back via
    /// AppKvLoaded and is processed in the result handler, which then
    /// calls rebuild_driver.
    fn restore_device_sink_binding(&mut self) {
        if self.active_url.is_empty() { return; }
        let key = format!("device_sink:{}", self.active_url);
        self.send_media(media::MediaCmd::LoadAppKv { key });
    }

    /// Bind a multi-room session: combine 2+ BT speakers into a
    /// PulseAudio combine-sink. Inputs are JSON arrays of MAC strings
    /// and matching friendly names. Dispatches an async create
    /// through the worker; result arrives via MultiRoomCreated.
    fn set_multiroom_macs(&mut self, macs_json: String, names_json: String) {
        self.bt_respect.note_activity();
        self.sink_unreachable_strikes = 0;
        let macs: Vec<String> = match serde_json::from_str(&macs_json) {
            Ok(v) => v,
            Err(e) => {
                self.driver_last_error = format!("invalid macs_json: {}", e);
                self.driver_changed();
                return;
            }
        };
        let names: Vec<String> = serde_json::from_str(&names_json)
            .unwrap_or_else(|_| vec![String::new(); macs.len()]);
        if macs.len() < 2 {
            self.driver_last_error =
                "multi-room needs ≥2 speakers — pick more, or use single-speaker mode"
                .to_string();
            self.driver_changed();
            return;
        }
        // Pair (mac, name) — pad names with "" if the JSON arrays
        // are mismatched lengths.
        let members: Vec<(String, String)> = macs.into_iter()
            .enumerate()
            .map(|(i, m)| (m, names.get(i).cloned().unwrap_or_default()))
            .collect();
        eprintln!("multiroom: requesting create with {} members", members.len());

        // Pre-flight sink check. The CreateMultiRoom worker shells out
        // to `pactl load-module module-combine-sink slaves=<csv>` —
        // when any slave isn't currently a PA sink, the call fails
        // with the user-hostile "Failure: Input/Output error". This
        // typically happens when one of the BT speakers wasn't
        // actually connected (the controller's bluetoothctl connect
        // failed silently, or the speaker is out of range).
        //
        // Surface the missing speakers BEFORE the worker call so the
        // error message is actionable instead of generic.
        let missing = self.find_missing_pa_sinks(&members);
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter()
                .map(|(_, n)| if n.is_empty() { "unknown".to_string() } else { n.clone() })
                .collect();
            let msg = format!(
                "Can't combine — {} not connected to the system. \
                 Try again or pick only connected speakers.",
                names.join(", ")
            );
            eprintln!("multiroom: pre-flight rejected — missing sinks: {:?}", missing);
            self.driver_last_error = msg;
            self.driver_changed();
            return;
        }

        // Tear down any existing multi-room first.
        if let Some(device_driver::AnyDriver::MultiRoom(_)) = &self.current_driver {
            if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                let _ = tx.send(Command::TeardownMultiRoom { driver: drv.clone() });
            }
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::CreateMultiRoom { members });
        }
    }

    /// For each (mac, name) in `members`, check whether
    /// `bluez_sink.<MAC_UNDER>.a2dp_sink` exists in `pactl list short
    /// sinks`. Returns the subset that are missing. Used as a
    /// pre-flight before CreateMultiRoom so the error message names
    /// the actual unreachable speakers instead of just "I/O error".
    fn find_missing_pa_sinks(
        &self,
        members: &[(String, String)],
    ) -> Vec<(String, String)> {
        use std::process::Command as ProcCommand;
        let out = match ProcCommand::new("pactl")
            .args(["list", "short", "sinks"])
            .output()
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(e) => {
                eprintln!("multiroom: pactl list failed: {} (skipping pre-flight)", e);
                return Vec::new();   // can't pre-check; let the worker try
            }
        };
        members.iter().filter(|(mac, _name)| {
            let expected = format!("bluez_sink.{}.a2dp_sink",
                                    mac.replace(':', "_"));
            !out.contains(&expected)
        }).cloned().collect()
    }

    /// Tear down any active multi-room session. After this the
    /// driver rebuilds based on `active_bt_mac` + firmware family —
    /// typically falling to BluetoothDriver if a MAC is set, or
    /// LocalDriver if not.
    /// After tearing down a controller-spawned paplay/ffmpeg pipeline
    /// (BT, Local, or MultiRoom drivers), invalidate the cached
    /// "currently playing" state so the UI doesn't flash a stale
    /// playing badge until the next ~5s poll catches up. Idempotent.
    ///
    /// Mozart drivers are explicitly NOT cleared: the device may still
    /// be playing its own input source (radio, line-in, network
    /// audio), and the next NowPlaying poll will report the truth.
    /// Forcing false there would lie to the user about the device.
    fn invalidate_local_playback_state(&mut self) {
        let was_local_pipeline = matches!(
            &self.current_driver,
            Some(device_driver::AnyDriver::Bluetooth(_))
                | Some(device_driver::AnyDriver::Local(_))
                | Some(device_driver::AnyDriver::MultiRoom(_)),
        );
        if was_local_pipeline && self.np_playing {
            self.np_playing = false;
            self.np_duration_sec = 0;
            self.np_seek_pos = 0;
            self.now_playing_changed();
        }
    }

    fn clear_multiroom(&mut self) {
        self.bt_respect.note_activity();
        self.sink_unreachable_strikes = 0;
        if let Some(device_driver::AnyDriver::MultiRoom(_)) = &self.current_driver {
            if let (Some(tx), Some(drv)) = (&self.tx, &self.current_driver) {
                let _ = tx.send(Command::TeardownMultiRoom { driver: drv.clone() });
            }
        }
        self.active_multiroom_json = "null".to_string();
        // Combine-sink teardown kills paplay synchronously; reflect
        // that in the cached badge so the UI doesn't lie for ~5s
        // until the next poll cycle. See invalidate_local_playback_state.
        self.invalidate_local_playback_state();
        self.persist_device_sink_binding();
        self.rebuild_driver();
    }

    /// Play a local file through the current driver. Bluetooth path
    /// spawns paplay; Mozart returns Unsupported (the caller should
    /// expose-via-track_server first and use the URI playback path).
    /// Result is swallowed asynchronously; status surfaces in
    /// driver_last_error.
    fn driver_play_local_file(&mut self, path: String, title: String) {
        self.bt_respect.note_activity();
        // Fresh playback — reset the unreachable watchdog so the
        // first DriverGetVolume poll after this doesn't act on a
        // stale strike count.
        self.sink_unreachable_strikes = 0;
        let drv = match &self.current_driver {
            Some(d) => d.clone(),
            None => {
                self.driver_last_error =
                    "no driver bound (active device not probed yet?)".into();
                self.driver_changed();
                return;
            }
        };
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::DriverOp {
                driver: drv,
                op: DriverOpKind::PlayLocalFile { path, title, start_offset_sec: 0 },
            });
        }
    }

    /// Stop playback on the current driver. Idempotent.
    /// Stop driver-routed playback. Two layers:
    ///   1. Async driver.stop() via the worker — kills the tracked
    ///      paplay PID cleanly. Normal path.
    ///   2. Synchronous pkill fallback — kills any orphan paplay or
    ///      ffmpeg processes the controller spawned but lost track
    ///      of. Handles the panic-stop case (BT disconnected, sink
    ///      moved, app reopened with stale children, etc.) where the
    ///      tracked PID may not match reality.
    ///
    /// The pkill uses `--parent <our_pid>` so we only kill children of
    /// THIS controller process — we don't disturb other paplay
    /// instances on the system (system notification sounds, other
    /// apps using paplay, etc).
    fn driver_stop(&mut self) {
        // Async path via the worker.
        if let Some(drv) = &self.current_driver {
            if let Some(tx) = &self.tx {
                let _ = tx.send(Command::DriverOp {
                    driver: drv.clone(),
                    op: DriverOpKind::Stop,
                });
            }
        }
        // ── Mozart-specific stop ──
        //
        // The AnyDriver::Mozart::stop() above returns Ok(()) without
        // actually calling the device — the original design was
        // conservative: "the device may still be playing its own
        // source" (Spotify Connect, AirPlay, etc.), so blindly
        // calling /Stream/Stop would kill external sessions the
        // user wasn't intending to interrupt.
        //
        // But the user pressing Stop in OUR app after pressing Play
        // in OUR app reasonably expects the music to stop. We track
        // "did this controller initiate the current playback" via
        // last_user_play_request. When that's set, route through the
        // existing Command::StopPlayback (POSTs to /BeoZone/Zone/
        // Stream/Stop) so the Mozart device actually halts. When it
        // isn't set, fall back to the conservative no-op — that
        // covers the "user is just adjusting volume on someone
        // else's Spotify" case.
        //
        // Without this, the Stop button silently did nothing for
        // Mozart devices that the controller was actively driving —
        // a textbook "GUI doesn't actually do stuff" bug.
        let is_mozart = matches!(
            &self.current_driver,
            Some(device_driver::AnyDriver::Mozart(_))
        ) || self.current_driver.is_none();  // empty driver = legacy Mozart path
        if is_mozart && self.last_user_play_request.is_some() {
            if let Some(url) = self.active_url_opt() {
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::StopPlayback { url });
                }
            }
            // Clear the "we own it" marker so a second Stop tap
            // doesn't re-fire the REST call.
            self.last_user_play_request = None;
        }
        // Sync fallback: pkill our own paplay + ffmpeg children. The
        // --parent flag scopes to children of this process so we
        // don't affect anything else on the system.
        let our_pid = std::process::id().to_string();
        for binary in ["paplay", "ffmpeg"] {
            let _ = std::process::Command::new("pkill")
                .args(["--parent", &our_pid, binary])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        // The processes we just killed were what produced the audio;
        // the cached "playing" badge no longer reflects reality. For
        // Mozart drivers we leave it alone — the device may still be
        // playing its own source.
        self.invalidate_local_playback_state();
        eprintln!("driver_stop: dispatched async stop + (mozart? {}) + sync pkill",
                  is_mozart);
    }

    /// Set volume via the current driver. For Mozart this falls back
    /// to the existing volume-safety pipeline (so high-volume guards
    /// still apply); for Bluetooth this is direct pactl.
    fn driver_set_volume_pct(&mut self, pct: i32) {
        let pct = pct.clamp(0, 100) as u8;
        let drv = match &self.current_driver {
            Some(d) => d.clone(),
            None => return,
        };
        // User-driven volume change invalidates any in-flight pending
        // pactl confirmation. Without clearing here, a held outlier
        // candidate from before the user's manual change could survive
        // into the next poll cycle and override the user's choice on
        // re-confirmation. Found by the sim fuzz harness (seed
        // 16012015614525151664, walk 10, step 13).
        self.driver_volume_pending = None;
        match &drv {
            device_driver::AnyDriver::Mozart(_) => {
                // Defer to the existing volume path — keeps safety
                // clamping + epoch tracking intact.
                self.set_volume_api(pct as i32);
            }
            device_driver::AnyDriver::Bluetooth(_)
            | device_driver::AnyDriver::Local(_)
            | device_driver::AnyDriver::MultiRoom(_)
            | device_driver::AnyDriver::Noop(_) => {
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::DriverOp {
                        driver: drv,
                        op: DriverOpKind::SetVolumePct(pct),
                    });
                }
            }
        }
    }

    // ── Music-player command bodies ─────────────────────────────────
    //
    // Each method is a thin forwarder: it builds a MediaCmd and hands
    // it to the media worker via send_media. Results come back as
    // MediaResult variants and land in apply_media_result, which
    // updates the corresponding qt_property. QML pages bind to those
    // properties and re-render on the property-changed signal.

    fn create_playlist(&mut self, name: String, description: String) {
        eprintln!("playlist: create '{}' ({})", name, description);
        self.send_media(media::MediaCmd::CreatePlaylist { name, description });
    }
    fn update_playlist_meta(&mut self, id: i64, name: String, description: String) {
        self.send_media(media::MediaCmd::UpdatePlaylistMeta { id, name, description });
    }
    fn delete_playlist(&mut self, id: i64) {
        self.send_media(media::MediaCmd::DeletePlaylist(id));
    }
    fn load_playlists(&mut self) {
        self.send_media(media::MediaCmd::ListPlaylists);
    }
    fn load_playlist_tracks(&mut self, id: i64) {
        self.send_media(media::MediaCmd::LoadPlaylistTracks(id));
    }
    fn add_track_to_playlist(&mut self, playlist_id: i64, track_id: i64) {
        self.send_media(media::MediaCmd::AddTrackToPlaylist { playlist_id, track_id });
    }
    fn remove_track_from_playlist(&mut self, playlist_id: i64, track_id: i64) {
        self.send_media(media::MediaCmd::RemoveTrackFromPlaylist { playlist_id, track_id });
    }
    fn reorder_playlist(&mut self, playlist_id: i64, track_id: i64, new_ord: i64) {
        self.send_media(media::MediaCmd::ReorderPlaylist { playlist_id, track_id, new_ord });
    }
    fn set_track_rating(&mut self, track_id: i64, rating: i64) {
        self.send_media(media::MediaCmd::SetTrackRating { track_id, rating });
    }
    fn write_track_tags_to_file(&mut self, track_id: i32, json: String) {
        let v: serde_json::Value = serde_json::from_str(&json)
            .unwrap_or(serde_json::json!({}));
        // Include a field only if the JSON has it; an explicit empty string
        // clears it (matching the edit dialog), so we map "" through as
        // Some(""), and a missing key as None (leave file value untouched).
        let field = |key: &str| -> Option<String> {
            v.get(key).and_then(|x| x.as_str()).map(|s| s.trim().to_string())
        };
        let year = v.get("year").and_then(|x| x.as_i64())
            .map(|y| if (0..4000).contains(&y) { y as u32 } else { 0 });
        self.send_media(media::MediaCmd::WriteTagsToFile {
            track_id: track_id as i64,
            title:  field("title"),
            artist: field("artist"),
            album:  field("album"),
            year,
        });
    }

    // Queue methods route to the active device URL. Empty active_url
    // means "global queue" — useful for organizing before a device is
    // selected. QML doesn't need to think about this.
    fn queue_push(&mut self, track_id: i64) {
        let device_url = self.active_url.clone();
        self.send_media(media::MediaCmd::QueuePush { track_id, device_url });
    }
    fn queue_insert(&mut self, track_id: i64, at: i64) {
        let device_url = self.active_url.clone();
        self.send_media(media::MediaCmd::QueueInsert { track_id, device_url, at });
    }
    fn queue_pop(&mut self) {
        let device_url = self.active_url.clone();
        self.send_media(media::MediaCmd::QueuePop { device_url });
    }
    fn queue_remove(&mut self, queue_id: i64) {
        self.send_media(media::MediaCmd::QueueRemove(queue_id));
    }
    fn queue_clear(&mut self) {
        let device_url = self.active_url.clone();
        self.send_media(media::MediaCmd::QueueClear { device_url });
    }
    fn load_queue(&mut self) {
        let device_url = self.active_url.clone();
        self.send_media(media::MediaCmd::LoadQueue { device_url });
    }
    fn load_queue_devices(&mut self) {
        self.send_media(media::MediaCmd::LoadQueueDevices);
    }
    fn load_queue_for_device(&mut self, device_url: String) {
        self.send_media(media::MediaCmd::LoadQueue { device_url });
    }

    /// Kick a soundscape loop-find + encode job. The actual work
    /// happens on the media worker thread; results stream back via
    /// the soundscape_* qt_properties.
    fn process_soundscape(&mut self,
        input_path: String,
        output_path: String,
        target_seconds: f32,
        tolerance_seconds: f32,
        n_candidates: i32,
        bitrate_kbps: i32,
        fade_ms: i32)
    {
        eprintln!("soundscape: queued processing of {} → {} (target {:.1}s, ±{:.1}s)",
                  input_path, output_path, target_seconds, tolerance_seconds);
        // Reset UI state so the user sees "starting" immediately.
        self.soundscape_progress = 0.0;
        self.soundscape_running = true;
        self.soundscape_result_json = String::new();
        self.soundscape_error = String::new();
        self.soundscape_changed();
        self.send_media(media::MediaCmd::ProcessSoundscape {
            input_path,
            output_path,
            target_seconds,
            tolerance_seconds,
            n_candidates: n_candidates.max(50) as usize,
            bitrate_kbps: bitrate_kbps.max(24) as u32,
            fade_ms: fade_ms.max(0) as u32,
        });
    }

    /// Run the four-way stem split + virtual staging on a WAV file.
    /// All four mono stems plus a staged stereo mix are written into
    /// `output_dir`. Work happens on a background thread so the QML
    /// stays responsive; check `stem_running` / `stem_status_json`
    /// from QML to poll progress.
    ///
    /// Output filenames:
    ///   drums.wav, voice.wav, piano.wav, synth.wav (mono),
    ///   staged_stereo.wav (the spatially-mixed stereo version)
    ///
    /// All written as 16-bit PCM WAV at the input's sample rate (or
    /// 44.1 kHz if the input rate was something exotic and we
    /// resampled).
    fn process_stem_split(&mut self, input_wav: String, output_dir: String) {
        self.stem_running = true;
        let empty: Vec<String> = Vec::new();
        self.stem_status_json = serde_json::json!({
            "state": "running",
            "progress": 0.0,
            "message": format!("reading {}", input_wav),
            "outputs": empty,
        }).to_string();
        self.stem_changed();
        // Spawn off the QML thread.
        std::thread::spawn(move || {
            stem_split_worker(&input_wav, &output_dir)
        });
    }

    /// Run speaker diarization on a podcast WAV. Result is a JSON
    /// timeline of (start, end, speaker_id) segments, surfaced via
    /// `diar_segments_json`. Status messages flow through `diar_status`.
    fn process_diarize(&mut self, input_wav: String) {
        self.diar_running = true;
        self.diar_status = format!("loading {}", input_wav);
        self.diar_segments_json = "[]".into();
        self.diar_changed();
        std::thread::spawn(move || {
            diarize_worker(&input_wav)
        });
    }

    // ── Mixer methods ──────────────────────────────────────────────
    //
    // Single dispatch point for slider changes: the QML slider's
    // onValueChanged calls set_mixer_value(channel, band, value),
    // which (a) updates the qt_property so the UI label tracks the
    // slider in real time, and (b) snapshots all eight properties
    // into a MixerParams struct and pushes it to the shared mixer.
    //
    // We pass the parameters through a single setter rather than 8
    // distinct methods to keep the QML side a tight loop.

    fn set_mixer_value(&mut self, channel: String, band: String, value: f32) {
        // Clamp dB bands to ±12 dB and volumes to 0..2.0; the QML
        // sliders already enforce this but defending in depth costs
        // nothing.
        let v = value.max(-24.0).min(24.0);
        match (channel.as_str(), band.as_str()) {
            ("spk", "low")  => self.mix_spk_low  = v.max(-12.0).min(12.0),
            ("spk", "mid")  => self.mix_spk_mid  = v.max(-12.0).min(12.0),
            ("spk", "high") => self.mix_spk_high = v.max(-12.0).min(12.0),
            ("spk", "vol")  => self.mix_spk_vol  = value.max(0.0).min(2.0),
            ("ins", "low")  => self.mix_ins_low  = v.max(-12.0).min(12.0),
            ("ins", "mid")  => self.mix_ins_mid  = v.max(-12.0).min(12.0),
            ("ins", "high") => self.mix_ins_high = v.max(-12.0).min(12.0),
            ("ins", "vol")  => self.mix_ins_vol  = value.max(0.0).min(2.0),
            // Spatializer width: 0 (mono) … 2.0 (double-wide).
            ("spatial", "width") => self.mix_width = value.max(0.0).min(2.0),
            _ => {
                eprintln!("set_mixer_value: unknown ({}, {})", channel, band);
                return;
            }
        }
        self.apply_mixer_params();
        self.mixer_changed();
    }

    fn reset_mixer(&mut self) {
        self.mix_spk_low = 0.0; self.mix_spk_mid = 0.0;
        self.mix_spk_high = 0.0; self.mix_spk_vol = 1.0;
        self.mix_ins_low = 0.0; self.mix_ins_mid = 0.0;
        self.mix_ins_high = 0.0; self.mix_ins_vol = 1.0;
        self.mix_width = 1.0;
        self.apply_mixer_params();
        self.mixer_changed();
    }

    /// Reset ONE channel's sliders to neutral. The other channel
    /// keeps its current values. Called from the per-channel ↺
    /// button in the ChannelColumn header.
    ///
    /// channel = "spk" → reset speaker bands + volume
    /// channel = "ins" → reset instrumental bands + volume
    /// any other value → no-op + log (defensive against typos in QML)
    fn reset_mixer_channel(&mut self, channel: String) {
        match channel.as_str() {
            "spk" => {
                self.mix_spk_low = 0.0;
                self.mix_spk_mid = 0.0;
                self.mix_spk_high = 0.0;
                self.mix_spk_vol = 1.0;
                eprintln!("mixer: reset speaker channel");
            }
            "ins" => {
                self.mix_ins_low = 0.0;
                self.mix_ins_mid = 0.0;
                self.mix_ins_high = 0.0;
                self.mix_ins_vol = 1.0;
                eprintln!("mixer: reset instrumental channel");
            }
            other => {
                eprintln!("reset_mixer_channel: unknown channel '{}' \
                           (expected 'spk' or 'ins')", other);
                return;
            }
        }
        self.apply_mixer_params();
        self.mixer_changed();
    }

    /// Snapshot the current mixer parameters from the qt_properties.
    /// Used both for live updates to the shared mixer and to package
    /// the params for the offline file processor.
    fn snapshot_mixer_params(&self) -> realtime_mixer::MixerParams {
        realtime_mixer::MixerParams {
            speaker: realtime_mixer::GroupParams {
                low_gain_db:  self.mix_spk_low,
                mid_gain_db:  self.mix_spk_mid,
                high_gain_db: self.mix_spk_high,
                volume:       self.mix_spk_vol,
            },
            instrumental: realtime_mixer::GroupParams {
                low_gain_db:  self.mix_ins_low,
                mid_gain_db:  self.mix_ins_mid,
                high_gain_db: self.mix_ins_high,
                volume:       self.mix_ins_vol,
            },
            width: self.mix_width,
        }
    }

    /// Dispatch the offline file processor with the current mixer
    /// parameters. Like process_soundscape: returns immediately,
    /// results stream back via mixer_proc_* properties.
    fn process_through_mixer_file(&mut self,
        input_path: String, output_path: String, bitrate_kbps: i32)
    {
        eprintln!("mixer_proc: queued {} → {} @ {} kbps",
                  input_path, output_path, bitrate_kbps);
        self.mixer_proc_progress = 0.0;
        self.mixer_proc_running = true;
        self.mixer_proc_result_json = String::new();
        self.mixer_proc_error = String::new();
        self.mixer_proc_changed();
        let params = self.snapshot_mixer_params();
        self.send_media(media::MediaCmd::ProcessThroughMixer {
            input_path,
            output_path,
            bitrate_kbps: bitrate_kbps.max(24) as u32,
            params,
        });
    }

    fn analyze_spectrum(&mut self, input_path: String) {
        eprintln!("spectrum: analyzing {}", input_path);
        self.spectrum_running = true;
        self.spectrum_error = String::new();
        self.spectrum_changed();
        self.send_media(media::MediaCmd::AnalyzeSpectrum { input_path });
    }

    fn clear_spectrum(&mut self) {
        if !self.spectrum_json.is_empty() {
            self.spectrum_json = String::new();
            self.spectrum_error = String::new();
            self.spectrum_changed();
        }
    }

    /// Mix the input through the current slider settings and play
    /// the result on the active device. The chain is:
    ///   1. Pick a temp path under /tmp for the Opus output.
    ///   2. Set mix_and_play_pending=true so MixerProcessDone
    ///      auto-continues to the play step.
    ///   3. Dispatch ProcessThroughMixer with snapshotted params.
    /// On completion the handler exposes the file via track_server
    /// and sends PlayResolvedUrl to the device worker.
    fn mix_and_play_file(&mut self, input_path: String, bitrate_kbps: i32) {
        if self.active_url.is_empty() {
            self.mixer_proc_error = "no active device".into();
            self.mixer_proc_changed();
            return;
        }
        if input_path.is_empty() {
            self.mixer_proc_error = "no input path".into();
            self.mixer_proc_changed();
            return;
        }
        // Use the system temp dir with a timestamp-based name so
        // repeated mixes don't collide. The file is left on disk;
        // /tmp's normal sweep policy cleans it up between sessions.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let output_path = format!("/tmp/bo-mix-{}.opus", ts);
        eprintln!("mix_and_play: queued {} → {} → play on {}",
                  input_path, output_path, self.active_url);
        self.mix_and_play_pending = true;
        self.process_through_mixer_file(input_path, output_path, bitrate_kbps);
    }

    /// Live-stream the input through the shared mixer to the active
    /// device. No file is written; the track server holds the source
    /// in RAM and pipes it through the mixer in 20 ms frames,
    /// snapshotting the shared `RealtimeMixer` params on every frame.
    /// Slider moves take effect after the device's internal buffer
    /// drains (a few hundred ms typically), which is what makes this
    /// "realtime" in a way that mix_and_play_file is not.
    fn live_stream_through_mixer(&mut self,
        input_path: String, bitrate_kbps: i32)
    {
        if self.active_url.is_empty() {
            self.mixer_proc_error = "no active device".into();
            self.mixer_proc_changed();
            return;
        }
        if input_path.is_empty() {
            self.mixer_proc_error = "no input path".into();
            self.mixer_proc_changed();
            return;
        }
        // Make sure the track server is running.
        if self.track_server.is_none() {
            let bound = track_server::TrackServer::bind(track_server::DEFAULT_PORT)
                .or_else(|_| track_server::TrackServer::bind(0));
            match bound {
                Ok(s) => self.track_server = Some(Arc::new(s)),
                Err(e) => {
                    self.mixer_proc_error =
                        format!("live_stream: track_server bind failed: {}", e);
                    self.mixer_proc_changed();
                    return;
                }
            }
        }
        let server = self.track_server.as_ref().unwrap().clone();
        let mixer = self.mixer.clone();
        let bitrate = bitrate_kbps.max(24) as u32;
        let seg = server.expose_mixer_stream(
            std::path::PathBuf::from(&input_path),
            mixer,
            bitrate,
        );
        // Resolve the URL through the device's IP so the right
        // local interface is chosen.
        let peer_ip = url_to_ip(&self.active_url);
        let stream_url = match peer_ip {
            Some(ip) => server.url_for(ip, &seg),
            None => {
                self.mixer_proc_error =
                    "live_stream: cannot determine local IP for device".into();
                self.mixer_proc_changed();
                return;
            }
        };
        eprintln!("live_stream: serving {} → telling {} to play",
                  stream_url, self.active_url);
        // Surface in the result card so the user has confirmation.
        // We use the same UI plumbing as the file processor for
        // consistency; mixer_proc_running is false here because the
        // "processing" happens connection-by-connection on the
        // track server thread, not on the media worker.
        self.mixer_proc_result_json = serde_json::json!({
            "live_stream": true,
            "url":         stream_url,
            "input_path":  input_path.clone(),
        }).to_string();
        self.mixer_proc_error = String::new();
        self.mixer_proc_changed();

        let title = std::path::Path::new(&input_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Live mix")
            .to_string();
        let active = self.active_url.clone();
        if let Some(tx) = self.tx.as_ref() {
            let _ = tx.send(Command::PlayResolvedUrl {
                url: active,
                track_url: stream_url,
                title,
            });
        }
    }

    // ──────────────────────────────────────────────────────────────
    // ANC methods
    //
    // The runner is lazily constructed: anc_enable(true) is the
    // first place that builds it + starts the mic bridge. Disabling
    // doesn't tear the runner down — the user is likely to toggle
    // on and off; keeping the mic bridge live across toggles avoids
    // the multi-hundred-ms parec spin-up every time. anc_enable(false)
    // just stops emitting anti-noise; the bridge keeps capturing
    // (so when the user re-enables, the predictor has a recent
    // window of mic samples to learn from).

    /// Ensure the engine + runner exist; called from every ANC entry
    /// point so we don't duplicate the lazy-construct pattern.
    fn ensure_anc_runner(&mut self) -> Result<(), String> {
        if self.anc_runner.is_some() { return Ok(()); }
        // Share the PredictorState with the existing sanitizer +
        // pattern_runner if they're already running. That way the
        // template that the pattern_learner extracts from the mic
        // signal is the SAME template the ANC engine inverts —
        // there's only one "ground truth" for what the periodic
        // noise looks like, and both consumers benefit from any
        // refresh. Without sharing, the sanitizer + ANC would each
        // learn their own template from the same mic, doubling the
        // CPU and producing two subtly-different anti-noise
        // signals that could phase-cancel each other.
        //
        // Fall back to a fresh PredictorState if the sanitizer isn't
        // running — the user has explicitly asked for ANC without
        // also enabling the sanitizer.
        let pstate = match self.sanitizer_handle.as_ref() {
            Some(h) => h.predictor_state.clone(),
            None => std::sync::Arc::new(
                predictor::PredictorState::new(0.5)),
        };
        let engine = std::sync::Arc::new(anc_engine::AncEngine::new(pstate));
        let mut runner = anc_runner::AncRunner::new(engine);
        runner.start_mic_bridge()?;
        // V1: push the persisted saturation values into the freshly
        // created runner so the standby loop uses the user's
        // restored settings, not the defaults. QtState already has
        // these loaded from settings_store at startup.
        runner.set_passby_saturation(self.passby_saturation as f32);
        runner.set_drone_saturation(self.drone_saturation as f32);
        self.anc_backend = runner.backend_label().to_string();
        self.anc_runner = Some(Box::new(runner));
        Ok(())
    }

    fn anc_enable(&mut self, on: bool) {
        if on {
            if let Err(e) = self.ensure_anc_runner() {
                eprintln!("anc: failed to start mic bridge: {}", e);
                self.anc_enabled = false;
                self.anc_changed();
                return;
            }
        }
        // Compute the sink BEFORE taking the mutable borrow on the
        // runner — borrow checker won't let us call self.current_anc_sink
        // while &mut self.anc_runner is alive.
        let sink = if on { self.current_anc_sink() } else { String::new() };
        if let Some(runner) = self.anc_runner.as_mut() {
            runner.engine().set_enabled(on);
            if on {
                // Periodic re-probe keeps the alignment fresh against
                // drift sources (thermal, BT re-pair, listener movement).
                // No-op if already running.
                runner.set_reprobe_target(sink, true);
                runner.start_periodic_reprobe();
            } else {
                // Disabling ANC also pauses reprobing so we don't keep
                // chirping the speaker after the user toggled off.
                runner.set_reprobe_target(String::new(), false);
            }
        }
        self.anc_enabled = on;
        self.anc_changed();
        eprintln!("anc: {}", if on { "enabled" } else { "disabled" });
    }

    /// Resolve the current speaker sink for ANC probe routing.
    /// Returns the PulseAudio sink name for a BT driver, or "" for
    /// a Mozart-REST or local-PA driver (system default sink).
    fn current_anc_sink(&self) -> String {
        // The BT driver's sink_name is the BT speaker's PA sink. For
        // Mozart drivers the controller streams via track_server so
        // the probe wouldn't reach the Mozart speaker that way anyway
        // — fall back to "" and let the user trigger a manual probe
        // if they want to characterize the local-mic / network-speaker
        // path explicitly.
        if let Some(drv) = &self.current_driver {
            if let device_driver::AnyDriver::Bluetooth(bt) = drv {
                return bt.sink_name();
            }
        }
        String::new()
    }

    fn anc_run_probe(&mut self, sink: String) {
        // Take the runner pointer briefly without lifetime tangles —
        // self.anc_runner stays put; we just borrow.
        if self.anc_runner.is_none() {
            if let Err(e) = self.ensure_anc_runner() {
                eprintln!("anc_probe: cannot start mic: {}", e);
                return;
            }
        }
        // Identify the active speaker so we can persist the
        // calibration under a stable key. BT MAC for BT drivers,
        // active_product_id for Mozart, "local" as fallback.
        let speaker_id = self.calibration_speaker_id();
        let label = if !self.active_bt_name.is_empty() {
            self.active_bt_name.clone()
        } else if !self.product_name.is_empty() {
            self.product_name.clone()
        } else { "local".to_string() };
        let runner = self.anc_runner.as_ref().unwrap();
        match runner.run_probe(&sink) {
            Ok(result) => {
                // Mirror to the QML-visible properties. The runner
                // already updated engine.alignment internally for the
                // sample-side path; here we surface to the UI.
                let ms = (result.round_trip_samples as f64
                    / anc_engine::ANC_SAMPLE_RATE as f64) * 1000.0;
                self.anc_alignment_correlation = result.correlation as f64;
                self.anc_alignment_ms = ms;
                // Persist the calibration sample. The function filters
                // out low-correlation / non-finite probes itself, so
                // junk doesn't pollute the history. Location tag is
                // empty for now — the UI hasn't surfaced the picker;
                // a future pass adds "nightstand/desk" quick-select.
                speaker_calibration::record_calibration(
                    &speaker_id, &label,
                    result.round_trip_samples as i64,
                    result.correlation,
                    "",
                );
                self.anc_changed();
            }
            Err(e) => {
                eprintln!("anc_probe: error: {}", e);
            }
        }
    }

    /// Resolve a stable speaker_id for calibration persistence.
    /// BT MAC → "bt:<mac>", Mozart → "mozart:<product_id>",
    /// otherwise "local".
    fn calibration_speaker_id(&self) -> String {
        if !self.active_bt_mac.is_empty() {
            return format!("bt:{}", self.active_bt_mac);
        }
        if !self.active_product_id.is_empty() {
            return format!("mozart:{}", self.active_product_id);
        }
        "local".to_string()
    }

    fn set_capture_dump_enabled(&mut self, on: bool) {
        audio_capture_dump::set_enabled(on);
        self.capture_dump_enabled = audio_capture_dump::is_enabled();
        self.refresh_capture_count();
        self.capture_dump_changed();
    }

    fn set_capture_location_tag(&mut self, tag: String) {
        audio_capture_dump::set_location_tag(&tag);
    }

    fn trigger_manual_capture(&mut self) {
        audio_capture_dump::trigger_capture("manual");
        self.refresh_capture_count();
    }

    fn refresh_capture_count(&mut self) {
        let n = audio_capture_dump::list_captures().len() as i32;
        if n != self.capture_count {
            self.capture_count = n;
            self.capture_dump_changed();
        }
    }

    /// Saturation setters. Both clamp + sanitize at this boundary.
    /// The actual canceller instances live inside the standby thread
    /// (`AncRunner::start_standby_mode`) so the only thing we can
    /// do here is store the desired value; the standby thread reads
    /// it via the `AncRunner::standby_saturations` channel on each
    /// process_window. (See AncRunner.)
    fn set_passby_saturation(&mut self, s: f64) {
        let safe = if s.is_finite() { s.clamp(0.0, 0.6) } else { 0.0 };
        self.passby_saturation = safe;
        if let Some(runner) = self.anc_runner.as_ref() {
            runner.set_passby_saturation(safe as f32);
        }
        // Persist so the choice survives restart. Failure to
        // persist is logged but doesn't break the runtime — the
        // value is already applied to the running canceller.
        if let Err(e) = crate::settings_store::set_float(
            "passby_saturation", safe)
        {
            eprintln!("v1: failed to persist passby_saturation: {}", e);
        }
        self.saturation_changed();
    }

    fn set_drone_saturation(&mut self, s: f64) {
        let safe = if s.is_finite() { s.clamp(0.0, 0.6) } else { 0.0 };
        self.drone_saturation = safe;
        if let Some(runner) = self.anc_runner.as_ref() {
            runner.set_drone_saturation(safe as f32);
        }
        if let Err(e) = crate::settings_store::set_float(
            "drone_saturation", safe)
        {
            eprintln!("v1: failed to persist drone_saturation: {}", e);
        }
        self.saturation_changed();
    }

    fn anc_set_mix(&mut self, anti: f64, music: f64) {
        // Clamp + sanitize at this boundary so even if QML sends
        // bizarre numbers (NaN binding from a broken slider, e.g.)
        // the engine sees safe values.
        let safe_anti = if anti.is_finite() { anti.clamp(0.0, 1.0) } else { 0.6 };
        let safe_music = if music.is_finite() { music.clamp(0.0, 2.0) } else { 1.0 };
        if let Some(runner) = self.anc_runner.as_ref() {
            runner.engine().set_mix(anc_engine::MixSettings {
                anti_noise_gain: safe_anti as f32,
                music_gain: safe_music as f32,
            });
        }
        self.anc_anti_gain = safe_anti;
        self.anc_music_gain = safe_music;
        self.anc_changed();
    }

    fn anc_set_blend(&mut self, blend: bool) {
        if let Some(runner) = self.anc_runner.as_ref() {
            runner.engine().set_blend_with_music(blend);
        }
        self.anc_blend_with_music = blend;
        self.anc_changed();
    }

    fn anc_play_with_anc(&mut self, track_path: String, sink: String) {
        if track_path.is_empty() {
            eprintln!("anc_play: empty track path");
            return;
        }
        if self.anc_runner.is_none() {
            if let Err(e) = self.ensure_anc_runner() {
                eprintln!("anc_play: cannot start mic: {}", e);
                return;
            }
        }
        // Engine must be enabled to do any mixing; otherwise it just
        // forwards music. Auto-enable to match user expectation.
        if !self.anc_enabled {
            self.anc_enable(true);
        }
        let runner = self.anc_runner.as_ref().unwrap();
        match runner.play_with_anc(&track_path, &sink) {
            Ok(()) => {
                eprintln!("anc_play: started for {} → {}", track_path,
                    if sink.is_empty() { "(default sink)" } else { sink.as_str() });
                // Children are tracked inside the runner; stop_playback
                // (called from the controller's transport stop path)
                // cleans them up properly without leaks.
            }
            Err(e) => {
                eprintln!("anc_play: failed: {}", e);
            }
        }
    }

    fn anc_start_standby(&mut self, sink: String) {
        if let Err(e) = self.ensure_anc_runner() {
            eprintln!("anc_standby: cannot start mic: {}", e);
            return;
        }
        // Default sink to whatever the ANC engine would target if the
        // QML side passed an empty string (typical: BT sink when a BT
        // speaker is active, default otherwise).
        let resolved_sink = if sink.is_empty() {
            self.current_anc_sink()
        } else { sink };
        if let Some(runner) = self.anc_runner.as_mut() {
            match runner.start_standby_mode(resolved_sink.clone()) {
                Ok(()) => {
                    self.anc_standby_active = true;
                    eprintln!("anc_standby: started against '{}'",
                        if resolved_sink.is_empty() { "(default)" } else { resolved_sink.as_str() });
                }
                Err(e) => {
                    eprintln!("anc_standby: start failed: {}", e);
                    self.anc_standby_active = false;
                }
            }
        }
        self.anc_standby_changed();
    }

    fn anc_stop_standby(&mut self) {
        if let Some(runner) = self.anc_runner.as_mut() {
            runner.stop_standby_mode();
        }
        self.anc_standby_active = false;
        self.anc_standby_phase = "Quiet".to_string();
        self.anc_standby_fundamental_hz = 0.0;
        self.anc_standby_changed();
    }

    fn anc_reset_standby_timers(&mut self) {
        // Currently the canceller's reset_timers lives inside the
        // standby thread's local PassbyCanceller, which doesn't have
        // an exposed reset interface from outside. The cheapest way
        // to forget timers is to restart the standby mode — the
        // PassbyCanceller is built fresh on each start.
        if self.anc_standby_active {
            let sink = self.current_anc_sink();
            self.anc_stop_standby();
            self.anc_start_standby(sink);
            eprintln!("anc_standby: timers reset via restart");
        }
    }

    fn anc_standby_refresh(&mut self) {
        if let Some(runner) = self.anc_runner.as_ref() {
            let s = runner.standby_snapshot();
            self.anc_standby_phase = match s.phase {
                passby_canceller::PassbyPhase::Quiet    => "Quiet",
                passby_canceller::PassbyPhase::Approach => "Approach",
                passby_canceller::PassbyPhase::Peak     => "Peak",
                passby_canceller::PassbyPhase::Recede   => "Recede",
            }.to_string();
            self.anc_standby_fundamental_hz = s.fundamental_hz as f64;
            self.anc_standby_timer_confidence = s.timer_confidence as f64;
            self.anc_standby_doppler = s.doppler_hz_per_s as f64;
            self.anc_standby_lock_quality = s.phase_lock_quality as f64;
            self.anc_standby_active = runner.standby_active();
            // ── Noise-adaptive scene gain ───────────────────────
            // Feed band_energy/noise_floor into the gain curve.
            // If the curve emits a new gain (delta > 2% from last),
            // publish via the qt_property; QML's Connections relays
            // to bo-mux via mux.set_scene_gain().
            //
            // Guard against an uninitialized noise floor (cold
            // start) so we don't divide by zero. Below 1e-6 means
            // the canceller hasn't built a floor estimate yet —
            // treat as "nominal" (ratio = 10) until it does.
            let ratio = if s.noise_floor > 1e-6 {
                s.band_energy / s.noise_floor
            } else { 10.0 };
            if let Some(new_gain) = self.noise_gain.observe(ratio) {
                self.anc_scene_gain = new_gain as f64;
                self.anc_scene_gain_loudness = self.noise_gain.smoothed_ratio() as f64;
                self.anc_scene_gain_changed();
            }
            self.anc_standby_changed();
        }
    }

    /// V1 experimental setter — toggles the runtime flag AND
    /// persists it to the settings store so it survives restart.
    /// The runtime flag lives in `crate::v1::EXPERIMENTAL_ENABLED`;
    /// the persisted side lives in the QML-visible settings JSON.
    fn set_experimental(&mut self, on: bool) {
        crate::v1::set_experimental_enabled(on);
        self.experimental_enabled = on;
        // Persist so the choice survives restart.
        match crate::settings_store::set_bool("experimental_enabled", on) {
            Ok(()) => {}
            Err(e) => eprintln!(
                "v1: failed to persist experimental_enabled={}: {}",
                on, e),
        }
        self.experimental_changed();
        eprintln!("v1: experimental features {} ({})",
                  if on { "ENABLED" } else { "disabled" },
                  if on { "research mode — canceller may make things worse" }
                  else { "production mode — masker only" });
    }
    /// Continuation after MixerProcessDone when the user clicked
    /// "Mix & Play". Parses the result JSON, ensures the track
    /// server is running, exposes the mixed Opus file via a
    /// tokenized URL, and dispatches PlayResolvedUrl. Errors land
    /// in mixer_proc_error so the UI's error card shows them.
    fn continue_mix_and_play(&mut self, result_json: &str) {
        // Parse output_path out of the result JSON.
        let parsed: serde_json::Value = match serde_json::from_str(result_json) {
            Ok(v) => v,
            Err(e) => {
                self.mixer_proc_error =
                    format!("mix-and-play: bad result JSON: {}", e);
                self.mixer_proc_changed();
                return;
            }
        };
        let output_path = match parsed.get("output_path")
            .and_then(|v| v.as_str())
        {
            Some(s) => s.to_string(),
            None => {
                self.mixer_proc_error = "mix-and-play: missing output_path".into();
                self.mixer_proc_changed();
                return;
            }
        };
        // Ensure the track server is running. Use the same
        // bind-or-fallback dance as the diagnose path.
        if self.track_server.is_none() {
            let bound = track_server::TrackServer::bind(track_server::DEFAULT_PORT)
                .or_else(|_| track_server::TrackServer::bind(0));
            match bound {
                Ok(s) => self.track_server = Some(Arc::new(s)),
                Err(e) => {
                    self.mixer_proc_error =
                        format!("mix-and-play: track_server bind failed: {}", e);
                    self.mixer_proc_changed();
                    return;
                }
            }
        }
        let server = self.track_server.as_ref().unwrap().clone();
        // Expose the mixed Opus file under a tokenized URL.
        let seg = server.expose(std::path::PathBuf::from(&output_path));
        // Build the full URL using the device's IP as the peer so
        // the track server picks the right local interface (otherwise
        // it might use loopback, which the device can't reach).
        let peer_ip = url_to_ip(&self.active_url);
        let mix_url = match peer_ip {
            Some(ip) => server.url_for(ip, &seg),
            None => {
                self.mixer_proc_error =
                    "mix-and-play: cannot determine local IP for device".into();
                self.mixer_proc_changed();
                return;
            }
        };
        eprintln!("mix_and_play: serving {} → telling {} to play", mix_url, self.active_url);

        // Dispatch the play command. Title pulls from the input
        // filename's stem so the device shows something sensible
        // in its now-playing display.
        let title = std::path::Path::new(&output_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Mix")
            .to_string();
        let active = self.active_url.clone();
        if let Some(tx) = self.tx.as_ref() {
            let _ = tx.send(Command::PlayResolvedUrl {
                url: active,
                track_url: mix_url,
                title,
            });
        }
    }

    /// Snapshot the eight properties into a MixerParams and push it
    /// into the shared mixer. Acquires a write lock briefly; safe to
    /// call from the UI thread.
    fn apply_mixer_params(&mut self) {
        let params = realtime_mixer::MixerParams {
            speaker: realtime_mixer::GroupParams {
                low_gain_db:  self.mix_spk_low,
                mid_gain_db:  self.mix_spk_mid,
                high_gain_db: self.mix_spk_high,
                volume:       self.mix_spk_vol,
            },
            instrumental: realtime_mixer::GroupParams {
                low_gain_db:  self.mix_ins_low,
                mid_gain_db:  self.mix_ins_mid,
                high_gain_db: self.mix_ins_high,
                volume:       self.mix_ins_vol,
            },
            width: self.mix_width,
        };
        if let Ok(mut m) = self.mixer.write() {
            m.set_params(params);
            // Recompute the response curves so the QML visualization
            // updates in lock-step with the sliders. 64 points across
            // 20 Hz – 20 kHz is a good balance of smoothness and
            // serialization cost (~5 KB JSON, recomputed on every
            // slider move).
            let curve = m.compute_curves(64, 20.0, 20_000.0);
            let arr: Vec<_> = curve.iter().map(|(hz, spk, ins)| {
                serde_json::json!({
                    "hz":     hz,
                    "spk_db": spk,
                    "ins_db": ins,
                })
            }).collect();
            self.mixer_curves_json = serde_json::to_string(&arr).unwrap_or_default();
        }
    }

    // Tag methods. QML passes an empty `color` string to mean "no
    // color"; convert to None for the worker.
    fn upsert_tag(&mut self, name: String, color: String) {
        let color = if color.is_empty() { None } else { Some(color) };
        self.send_media(media::MediaCmd::UpsertTag { name, color });
    }
    fn attach_tag(&mut self, track_id: i64, tag_id: i64) {
        self.send_media(media::MediaCmd::AttachTag { track_id, tag_id });
    }
    fn detach_tag(&mut self, track_id: i64, tag_id: i64) {
        self.send_media(media::MediaCmd::DetachTag { track_id, tag_id });
    }
    fn load_track_tags(&mut self, track_id: i64) {
        self.send_media(media::MediaCmd::LoadTrackTags(track_id));
    }
    fn load_all_tags(&mut self) {
        self.send_media(media::MediaCmd::ListAllTags);
    }
    fn load_tracks_by_tag(&mut self, tag_id: i64) {
        self.send_media(media::MediaCmd::LoadTracksByTag(tag_id));
    }

    // Artist + history methods.
    fn load_top_artists(&mut self, limit: i32) {
        let limit = limit.max(1) as usize;
        self.send_media(media::MediaCmd::LoadTopArtists(limit));
    }
    fn load_all_artists_alpha(&mut self) {
        self.send_media(media::MediaCmd::LoadAllArtistsAlpha);
    }
    fn set_artist_sort(&mut self, sort: String) {
        // The artists browse list is paginated, so its sort lives in SQL
        // (Name / Track count). Validate through ArtistPageSort.
        let tag = match media::catalog::ArtistPageSort::from_tag(&sort) {
            media::catalog::ArtistPageSort::TrackCount => "tracks",
            media::catalog::ArtistPageSort::Name => "name",
        };
        self.artist_sort = tag.into();
        let _ = settings_store::set_str("artist_sort", tag);
        self.artist_sort_changed();
    }
    fn load_top_albums(&mut self, limit: i32) {
        let limit = limit.max(1) as usize;
        self.send_media(media::MediaCmd::LoadTopAlbums(limit));
    }
    fn load_all_albums_alpha(&mut self) {
        let sort = {
            let s = self.album_sort.to_string();
            if s.is_empty() { "title".to_string() } else { s }
        };
        self.send_media(media::MediaCmd::LoadAllAlbumsAlpha { sort });
    }
    fn set_album_sort(&mut self, sort: String) {
        let tag = media::library::AlbumSort::from_tag(&sort).to_tag();
        self.album_sort = tag.into();
        let _ = settings_store::set_str("album_sort", tag);
        self.album_sort_changed();
    }
    /// Open an album detail view. Pre-fills the title + artist so
    /// the page header can render immediately, then asks the worker
    /// for the track list which arrives async via TrackList tagged
    /// "album:<title>:<artist>".
    fn open_album(&mut self, title: String, primary_artist: String) {
        self.current_album_title = title.clone();
        self.current_album_artist = primary_artist.clone();
        self.current_album_tracks_json = String::new();
        self.current_album_changed();
        self.send_media(media::MediaCmd::LoadTracksInAlbum {
            title, primary_artist,
        });
    }
    fn close_album(&mut self) {
        // Clear all three so QML bindings to current_album_* go
        // empty in a single signal. Saves the page from a brief
        // flash of stale content while the user navigates away.
        if !self.current_album_title.is_empty()
            || !self.current_album_artist.is_empty()
            || !self.current_album_tracks_json.is_empty()
        {
            self.current_album_title = String::new();
            self.current_album_artist = String::new();
            self.current_album_tracks_json = String::new();
            self.current_album_changed();
        }
    }

    /// Smart-playlist style track lists. All three populate the main
    /// library view via the standard TrackList channel — the QML
    /// side can switch between them like it does artist/tag/BPM-band
    /// drilldowns.
    fn load_top_tracks(&mut self, limit: i32) {
        let limit = limit.max(1) as usize;
        self.send_media(media::MediaCmd::LoadTopTracks(limit));
    }
    fn load_recently_added(&mut self, limit: i32) {
        let limit = limit.max(1) as usize;
        self.send_media(media::MediaCmd::LoadRecentlyAdded(limit));
    }
    fn load_never_played(&mut self, limit: i32) {
        let limit = limit.max(1) as usize;
        self.send_media(media::MediaCmd::LoadNeverPlayed(limit));
    }
    fn load_recent_history(&mut self, limit: i32) {
        let limit = limit.max(1) as usize;
        self.send_media(media::MediaCmd::LoadRecentHistory(limit));
    }
    fn load_history_stats(&mut self, lookback_days: i32) {
        let days = lookback_days.max(1) as i64;
        self.send_media(media::MediaCmd::LoadHistoryStats { lookback_days: days });
    }
    fn load_history_summary(&mut self, lookback_days: i32) {
        let days = lookback_days.max(1) as i64;
        self.send_media(media::MediaCmd::LoadHistorySummary { lookback_days: days });
    }
    fn search_advanced(&mut self, query: String) {
        self.send_media(media::MediaCmd::SearchAdvanced(query));
    }

    /// Scan KDE's paired Bluetooth devices and merge them into the
    /// auto-connect table as disabled-by-default. The user can then
    /// flip the toggle without needing to type the MAC by hand.
    /// Runs in a background thread via spawn_bt_discovery_probe —
    /// the QML thread doesn't block on `bluetoothctl devices Paired`
    /// (which can take a few hundred ms or longer if BlueZ is busy).
    /// Results land via the channel drain in poll_results.
    fn discover_bt_devices(&mut self) {
        eprintln!("discover_bt_devices: kicking off async scan");
        self.spawn_bt_discovery_probe();
    }

    /// Disconnect the active session for a device but leave it paired.
    /// Best-effort — bluetoothctl exit code is consulted but we don't
    /// fail loudly if the device is already disconnected.
    fn disconnect_bt_device(&mut self, mac: String) {
        if mac.is_empty() { return; }
        eprintln!("disconnect_bt_device: {}", mac);
        std::thread::spawn(move || {
            use std::process::Command as ProcCommand;
            let _ = ProcCommand::new("bluetoothctl")
                .args(["--timeout", "5", "disconnect", &mac])
                .output();
        });
    }

    /// Forget a Bluetooth device — unpairs it from the system + drops
    /// it from our auto-connect tracking. Used when the user no longer
    /// wants the controller managing this device at all.
    fn forget_bt_device(&mut self, mac: String) {
        if mac.is_empty() { return; }
        eprintln!("forget_bt_device: {}", mac);
        // Local drop first so the UI updates immediately.
        let was_present = self.bt_autoconnect.forget(&mac);
        if was_present {
            self.send_media(media::MediaCmd::SaveAppKv {
                key: "bt_autoconnect_devices".to_string(),
                value: self.bt_autoconnect.to_json(),
            });
            self.refresh_bt_devices_json();
        }
        // Then fire off the system-level unpair in the background.
        let mac_for_thread = mac.clone();
        std::thread::spawn(move || {
            use std::process::Command as ProcCommand;
            let _ = ProcCommand::new("bluetoothctl")
                .args(["--timeout", "5", "remove", &mac_for_thread])
                .output();
        });
    }

    /// Sample the desktop's MPRIS state via `playerctl`. The shell-out
    /// is cheap (~10-30 ms) so we run it inline. Cached values are
    /// only invalidated when they actually change to keep the
    /// mpris_changed signal quiet.
    fn refresh_mpris(&mut self) {
        let state = mpris_listener::poll();
        let dirty = self.mpris_playing != state.playing
            || self.mpris_player  != state.player
            || self.mpris_title   != state.title
            || self.mpris_artist  != state.artist;
        if dirty {
            self.mpris_playing = state.playing;
            self.mpris_player  = state.player.clone();
            self.mpris_title   = state.title.clone();
            self.mpris_artist  = state.artist.clone();
            self.mpris_changed();
            if state.is_active() {
                eprintln!("mpris: {} playing '{}' by {}",
                    state.player, state.title, state.artist);
            }
        }
    }

    /// Run a self-test on the playback pipeline. Useful when "playback
    /// is broken" and the user wants to know *which* layer is failing.
    /// Sequence:
    ///   1. Bind the track server if not bound; verify the port.
    ///   2. Expose a small in-memory test WAV (a 200 ms tone).
    ///   3. Build the device-facing URL using the active device's IP.
    ///   4. Verify the URL resolves from the controller (a localhost
    ///      GET) — proves the server itself is up and reachable.
    ///   5. Surface results in `last_play_status` so the settings
    ///      page or play status banner shows what's working.
    ///
    /// This doesn't actually play anything on the speaker — it just
    /// verifies the controller-side half of the pipeline is healthy.
    fn diagnose_playback(&mut self) {
        // Step 1: track server.
        if self.track_server.is_none() {
            match track_server::TrackServer::bind(track_server::DEFAULT_PORT) {
                Ok(s) => self.track_server = Some(Arc::new(s)),
                Err(_) => match track_server::TrackServer::bind(0) {
                    Ok(s) => self.track_server = Some(Arc::new(s)),
                    Err(e) => {
                        self.last_play_status =
                            format!("diagnose: cannot bind track server: {}", e);
                        self.play_status_changed();
                        return;
                    }
                }
            }
        }
        let server = self.track_server.as_ref().unwrap();
        // Step 2: register a tiny test WAV (a 220 Hz tone for 200 ms).
        let wav = device_ping::synthesize_ping_wav();
        let seg = server.expose_bytes("audio/wav", wav);
        // Step 3: build URL using the active device's IP as the peer
        // so we pick the right local interface.
        let active = self.active_url.clone();
        let peer_ip = url_to_ip(&active);
        let url = match peer_ip {
            Some(ip) => server.url_for(ip, &seg),
            None => {
                self.last_play_status =
                    "diagnose: no active device IP to test against".to_string();
                self.play_status_changed();
                return;
            }
        };
        // Step 4: do a localhost GET against the server to verify
        // it's serving content. Cheap, blocking — runs on the QtState
        // thread but only takes a few ms.
        let port = server.port();
        let probe_url = format!("http://127.0.0.1:{}{}", port, seg);
        let ok = match std::process::Command::new("curl")
            .args(["-sS", "--max-time", "2", "-o", "/dev/null", "-w", "%{http_code}",
                   &probe_url])
            .output()
        {
            Ok(o) => {
                let code = String::from_utf8_lossy(&o.stdout);
                code.trim() == "200"
            }
            Err(_) => false,
        };
        self.last_play_status = if ok {
            format!("diagnose: server OK ({}), device URL: {}", port, url)
        } else {
            format!("diagnose: server BIND_OK but /health failed; port {} may be blocked", port)
        };
        self.last_play_url = url.clone();
        self.play_status_changed();

        // Step 5 (device side): we've now verified the controller's
        // half — server binds, exposes content, localhost can fetch.
        // That's the CONTROLLER's half. The user complaint "playback
        // doesn't work" almost always means the DEVICE's half is
        // failing: the device can't reach our HTTP server (firewall,
        // wrong-segment routing), or the device REJECTS the URL
        // (mozart_legacy returns 500 on every play endpoint).
        //
        // Surface the device-side state into the same status banner
        // so the user sees the actual failure mode, not just "server
        // OK" which is misleading on a device that can't play.
        let device_hint = {
            let family = self.device_firmware_family.as_str();
            let driver = self.current_driver_kind.as_str();
            let rest_ok = self.device_supports_rest_playback;
            let active_bt = !self.active_bt_mac.is_empty();
            if active.is_empty() {
                Some("no active device — select one on the Connect page".to_string())
            } else if family == "mozart_legacy" && !rest_ok && !active_bt {
                // The exact case the user is hitting per the recent
                // log: Stage @ 192.168.2.12 is legacy mozart, no BT
                // bound for it. The "diagnose: server OK" line is
                // useless here; the issue is the device side.
                Some(format!(
                    "device '{}' is legacy Mozart firmware — can't \
                     accept URI playback. Bind a Bluetooth speaker on \
                     the Connect page to route audio to a real speaker.",
                    self.device_firmware_label))
            } else if family.is_empty() && driver.is_empty() {
                Some("device still being classified — try again in a \
                      few seconds. If this persists, the device may be \
                      unreachable.".to_string())
            } else if driver == "noop" {
                Some(format!(
                    "no playback path: {}. Bind a Bluetooth speaker \
                     on the Connect page.",
                    self.current_driver_label))
            } else if driver == "local" {
                Some("audio will play on this device's speakers (no \
                      room speaker bound). Pair a Bluetooth speaker \
                      on the Connect page to route to a room speaker."
                     .to_string())
            } else { None }
        };
        if let Some(hint) = device_hint {
            // Append the device-side conclusion to the existing
            // status. Don't replace it — the user wants to know
            // both halves: "server OK + device side: <explanation>"
            // is far more actionable than just one or the other.
            self.last_play_status = format!(
                "{}\n→ {}", self.last_play_status, hint);
            self.play_status_changed();
        }
    }

    // ── Priority groups ──────────────────────────────────────────────

    /// Fire off a capability probe for every known device. The results
    /// arrive asynchronously as ResultMessage::DeviceStateProbed, which
    /// updates the cached state map and rebuilds priority_devices_json.
    /// QML calls this on startup and after device list changes.
    fn refresh_priorities(&mut self) {
        let devices: Vec<serde_json::Value> =
            serde_json::from_str(&self.devices_json).unwrap_or_default();
        for d in &devices {
            if let Some(url) = d.get("url").and_then(|v| v.as_str()) {
                if !is_well_formed_http_url(url) {
                    eprintln!("refresh_priorities: skip malformed url '{}'", url);
                    continue;
                }
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Command::ProbeDeviceCapabilities {
                        url: url.to_string(),
                    });
                }
            }
        }
        // Rebuild the JSON immediately with whatever cached state we have,
        // so the UI shows *something* while probes are in flight.
        self.rebuild_priority_json();
    }

    /// Switch to the device flagged as the recommended hub. No-op if we
    /// don't currently have one (all offline, or no devices known).
    fn apply_recommended_hub(&mut self) {
        let url = self.recommended_hub_url.clone();
        let pid = self.recommended_hub_product.clone();
        if !url.is_empty() {
            self.select_device_with_product(url, pid);
        }
    }

    // ── Anticipator wiring ───────────────────────────────────────────

    /// Record an action with the anticipator + refresh prediction_json
    /// for the UI. Internal helper used by the qt_methods below.
    fn observe_action(&mut self, kind: anticipator::ActionKind) {
        self.anticipator.observe(kind);
        self.refresh_prediction_json();
        self.execute_prestage();
    }

    fn refresh_prediction_json(&mut self) {
        let preds = self.anticipator.predict(3);
        let json: Vec<serde_json::Value> = preds.iter().map(|p| {
            serde_json::json!({
                "action": format!("{:?}", p.action),
                "confidence": p.confidence,
                "horizon_ms": p.horizon_ms,
                "cost": format!("{:?}", p.cost),
            })
        }).collect();
        self.prediction_json = serde_json::to_string(&json)
            .unwrap_or_else(|_| "[]".into());
        self.prediction_changed();
    }

    /// Read prestage recommendations and dispatch the cheap ones. Moderate
    /// and Expensive recommendations are exposed via prediction_json for
    /// QML to act on (e.g. opening Library proactively warms the catalog).
    fn execute_prestage(&mut self) {
        // External-player context: if MPRIS reports something else
        // is actively playing (Spotify, Firefox, etc.), the user is
        // engaged with that audio and isn't imminently going to
        // PlayTrack on the B&O. We still keep the connection warm
        // (Cheap pre-stage stays on) but skip the expensive paths:
        // speculative loadUri thrashes the device pipeline, shadow
        // streams compete with the user's external audio, and
        // resolving N candidate tracks burns SQLite + HTTP server
        // cycles for nothing. The user can still tap a track at any
        // time — this just defers proactive work to when they actually
        // engage with the controller.
        let externally_playing = self.mpris_playing && !self.mpris_player.is_empty();
        let recs = self.anticipator.prestage_recommendations();
        for r in recs {
            // Skip expensive prestaging when MPRIS shows external playback.
            // Cheap operations (connection warm) still run — they're
            // genuinely lightweight and might benefit the upcoming
            // "user pauses Spotify to tap our app" transition.
            if externally_playing
                && matches!(r.cost, anticipator::PrestageCost::Moderate
                                  | anticipator::PrestageCost::Expensive)
            {
                continue;
            }
            match (r.action, r.cost) {
                // Cheap pre-stage: keep the device's connection warm by
                // firing a lightweight GetVolume. The reqwest client pools
                // the connection so the next real request reuses it. Rate-
                // limited via ConnWarmer so a prediction-loop can't flood.
                (anticipator::ActionKind::Resume, anticipator::PrestageCost::Cheap)
                | (anticipator::ActionKind::TrackEnded, anticipator::PrestageCost::Cheap) => {
                    self.fire_connection_warm();
                }
                // Picker is likely → probe all device capabilities now so
                // the priority list is fresh when they open the picker.
                (anticipator::ActionKind::OpenDevicePicker, _) => {
                    self.refresh_priorities();
                }
                // SwitchDevice prediction at high confidence → pre-wake
                // the candidate device via WoL if its MAC is known.
                // For a sleeping B&O speaker the wake cycle is ~3-5 s,
                // so even a moderate-confidence prediction wins on
                // perceived "tap → device responds" latency.
                (anticipator::ActionKind::SwitchDevice, anticipator::PrestageCost::Moderate)
                | (anticipator::ActionKind::SwitchDevice, anticipator::PrestageCost::Expensive) => {
                    if !self.aggression.pre_stage_enabled() { continue; }
                    // Adaptive floor: pre-stage more aggressively when
                    // the network is slow, because catch-up costs more.
                    let floor = self.aggression
                        .moderate_floor_adaptive(self.rtt_p95_ms.max(0) as u32);
                    if r.confidence < floor { continue; }
                    self.fire_wol_for_known_devices();
                }
                // MODERATE: PlayTrack from Browsing with high confidence.
                // Resolve top recent tracks via the media worker so when
                // the user actually taps, the path is already cached AND
                // the local HTTP server has the URL exposed. Latency drop:
                // ~10-30 ms saved per tap, which is the dominant local-side
                // cost in the resolve→expose→POST chain.
                //
                // VERY HIGH (≥0.80): also fire a shadow stream — the
                // candidate track plays silently on the device so a real
                // tap promotes to instant audio via a volume ramp only.
                // The shadow gate (ShadowManager::should_shadow) has its
                // own safety checks (device-not-playing, volume known).
                (anticipator::ActionKind::PlayTrack, anticipator::PrestageCost::Moderate) => {
                    if !self.aggression.pre_stage_enabled() { continue; }
                    // Adaptive floor scales with RTT. On a fast LAN
                    // (RTT 5-15ms) the base floor applies. On a sluggish
                    // mesh hop (RTT 200-500ms) the floor drops by up to
                    // 0.15 so we prestage more eagerly — the user
                    // benefits more when the alternative is "wait
                    // longer".
                    let floor = self.aggression
                        .moderate_floor_adaptive(self.rtt_p95_ms.max(0) as u32);
                    if r.confidence < floor { continue; }
                    self.prestage_top_tracks();
                    // Shadow stream gates on its own (higher) threshold.
                    // Disabled entirely at OFF/LOW (shadow_enabled=false).
                    if self.aggression.shadow_enabled()
                        && r.confidence >= self.aggression.shadow_floor()
                    {
                        self.try_start_shadow(r.confidence);
                    }
                }
                _ => { /* not currently auto-handled */ }
            }
        }
    }

    /// Send a small batch of ResolveForPlay for the top recent track IDs.
    /// The worker replies with TrackForPlay events; apply_media_result
    /// already handles those (exposes via track_server, builds URL,
    /// flips prestaged_track_urls). The actual play_track tap then just
    /// reissues the same resolve — but the worker's SQLite cache + our
    /// track_server token table are already primed.
    ///
    /// We cap pre-staging at ~5 tracks so we don't churn the catalog or
    /// fill the token table with stale entries. Tokens live for the app
    /// session; stale ones are harmless (the file path will still be
    /// valid the next time anyone fetches them).
    fn prestage_top_tracks(&mut self) {
        // Throttle: don't repeat within 5 seconds. The prediction will
        // keep firing on every observation, but the actual SQL+expose
        // work shouldn't run more than once per several seconds.
        let now = std::time::Instant::now();
        if let Some(last) = self.last_prestage_at {
            if now.duration_since(last).as_secs() < 5 { return; }
        }
        self.last_prestage_at = Some(now);
        // Read library_json (populated when Library opens) for current
        // visible track list, then take the top N ids as candidates.
        // N comes from the aggression policy: more aggression = wider
        // fanout. Newer platforms with parallel decode benefit from this.
        let count = self.aggression.multi_prestage_count() as usize;
        if count == 0 { return; }
        let recent: Vec<serde_json::Value> =
            serde_json::from_str(&self.library_json).unwrap_or_default();
        let ids: Vec<i64> = recent.iter()
            .filter_map(|v| v.get("id").and_then(|x| x.as_i64()))
            .take(count)
            .collect();
        if ids.is_empty() { return; }
        eprintln!("anticipator: pre-staging {} candidate tracks ({})",
                  ids.len(), self.aggression.label());
        for id in ids {
            if !self.prestaged_track_ids.contains(&id) {
                self.prestaged_track_ids.insert(id);
                self.send_media(media::MediaCmd::ResolveForPlay(id));
            }
        }
    }

    /// Try to start a shadow stream for the high-likelihood candidate.
    /// Fires only when the prediction confidence + situation all clear
    /// the bar in ShadowManager::should_shadow. Otherwise it logs the
    /// reason and returns silently — no harm, no audible artifact.
    fn try_start_shadow(&mut self, confidence: f32) {
        // Shadow is a Mozart-REST optimization: it pre-warms the
        // device's network audio source by silently dispatching the
        // predicted next track at vol=0, so when the user actually
        // plays it the buffer is already filled and playback starts
        // instantly. This only makes sense when the *device* is the
        // audio sink. With a non-Mozart driver (bluetooth/local/
        // multiroom), audio routes through PulseAudio locally — the
        // device's REST endpoint won't be touched, so a shadow
        // dispatch would just fire 9-endpoint failures against a
        // device that isn't playing anything. Skip cleanly.
        //
        // Same goes for `mozart_legacy` devices that don't accept
        // URI playback at all — shadow has nothing to pre-warm.
        let drv_kind = self.current_driver_kind.as_str();
        if matches!(drv_kind, "bluetooth" | "local" | "multiroom" | "noop") {
            // Quiet path — don't even log; happens on every
            // anticipator hint with non-Mozart routing.
            return;
        }
        if !self.device_firmware_family.is_empty()
            && !self.device_supports_rest_playback
        {
            eprintln!("shadow: skip (firmware family {} doesn't accept URI playback)",
                      self.device_firmware_family);
            return;
        }
        let candidate = match self.shadow_candidate {
            Some(id) => id,
            None => return,
        };
        // We need the candidate's URL already cached (either from a prior
        // pre-stage or from a previous play). If it isn't, kick off the
        // pre-stage first; we'll get another chance after TrackForPlay.
        let url = match self.prestaged_track_urls.get(&candidate).cloned() {
            Some(u) => u,
            None => {
                if !self.prestaged_track_ids.contains(&candidate) {
                    self.prestaged_track_ids.insert(candidate);
                    self.send_media(media::MediaCmd::ResolveForPlay(candidate));
                }
                return;
            }
        };
        let device_is_playing = self.np_playing;
        let device_has_vol = self.last_known_volume > 0;
        let decision = self.shadow.should_shadow(
            confidence, device_is_playing, device_has_vol, candidate);
        match decision {
            shadow_stream::ShadowDecision::Skip(why) => {
                eprintln!("shadow: skip ({})", why);
                return;
            }
            shadow_stream::ShadowDecision::Go => {}
        }
        // Save volume, then drive device to 0 → start stream silently.
        let saved = self.last_known_volume;
        eprintln!(
            "shadow: starting for track {} (saving vol={}, confidence={:.2})",
            candidate, saved, confidence,
        );
        self.shadow.start(candidate, url.clone(), saved);
        // Issue the volume drop + stream start through the existing
        // command channel. The device sees: vol=0 → play URL.
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::SetVolume {
                url: self.active_url.clone(),
                value: 0,
            });
            let _ = tx.send(Command::PlayResolvedUrl {
                url: self.active_url.clone(),
                track_url: url,
                title: "(shadow)".to_string(),
            });
        }
    }

    /// Fire the richer "estimate" cue (multi-harmonic build-up with
    /// crossfade-friendly exponential decay tail). Used by play_track
    /// at LOW aggression and above. Empty when harmonics=0 or duration=0.
    fn fire_estimate_cue(&mut self, bpm: f32, tonic_hz: f32, duration_ms: u32, harmonics: u32) {
        // Cache key: integer rounding of float params, so near-identical
        // calls reuse the same buffer. ×10 multipliers preserve a tenth
        // of a unit of resolution (124.5 BPM ≠ 124.6 BPM in the cache).
        let key = (
            (bpm * 10.0) as u32,
            (tonic_hz * 10.0) as u32,
            duration_ms,
            harmonics,
        );
        let pcm = if let Some(cached) = self.cue_cache.get(&key) {
            cached.clone()
        } else {
            let fresh = pre_buffer::synthesize_estimate_cue(
                bpm, tonic_hz, duration_ms, harmonics);
            if fresh.is_empty() { return; }
            let arc = std::sync::Arc::new(fresh);
            // Cap the cache so a pathological caller can't grow it
            // unbounded. 32 entries is plenty for typical use — a
            // listening session touches maybe 5-10 distinct (key, BPM)
            // pairs.
            if self.cue_cache.len() >= 32 {
                // Evict an arbitrary entry. Order doesn't matter much
                // since cache hits are extremely cheap to miss.
                let k = self.cue_cache.keys().next().cloned();
                if let Some(k) = k { self.cue_cache.remove(&k); }
            }
            self.cue_cache.insert(key, arc.clone());
            arc
        };
        // Stream a clone of the bytes — paplay consumes a Vec<u8>.
        // Arc<Vec<u8>> → clone the inner Vec for now; we could refactor
        // stream_pcm_to_player to take a &[u8] borrow to avoid this,
        // but the dup is negligible relative to the playback cost.
        Self::stream_pcm_to_player((*pcm).clone());
    }

    /// Shared paplay/aplay piping. Spawned in a detached thread so the
    /// QML event loop doesn't block on subprocess wait.
    ///
    /// Children inherit PR_SET_PDEATHSIG so they die when this process
    /// dies. Cue tones are short (<1s) so this almost never matters in
    /// practice, but the consistency with the music-playback path
    /// matters for the "no audio survives the app" invariant.
    ///
    /// Spawn fallback chain:
    ///   1. `paplay` — works on PulseAudio and on PipeWire when the
    ///      `pipewire-pulse` compatibility package is installed.
    ///   2. `pw-cat --playback` — works on PipeWire-only setups
    ///      (e.g. Alpine / postmarketOS without pipewire-pulse),
    ///      where paplay isn't installed at all.
    ///   3. `aplay` — last-ditch ALSA path; some headless / embedded
    ///      configurations only have alsa-utils.
    ///
    /// Each candidate gets the same PCM bytes piped to stdin and the
    /// same PR_SET_PDEATHSIG setup. We don't try multiple in parallel
    /// — the first one that spawns wins.
    fn stream_pcm_to_player(pcm: Vec<u8>) {
        std::thread::spawn(move || {
            use std::io::Write;
            use std::process::{Command, Stdio};
            #[cfg(unix)]
            use std::os::unix::process::CommandExt;
            // Helper: prepare a command with the standard pre-exec
            // pdeath setup and stdin piped. Caller adds the args.
            let mk = |bin: &str| -> Command {
                let mut c = Command::new(bin);
                c.stdin(Stdio::piped())
                 .stdout(Stdio::null())
                 .stderr(Stdio::null());
                #[cfg(unix)]
                unsafe {
                    c.pre_exec(|| {
                        let r = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                        if r != 0 { return Err(std::io::Error::last_os_error()); }
                        Ok(())
                    });
                }
                c
            };

            let mut paplay = mk("paplay");
            paplay.args(["--raw", "--format=s16le", "--rate=48000", "--channels=1"]);
            let mut child = match paplay.spawn() {
                Ok(c) => c,
                Err(_paplay_err) => {
                    // PipeWire fallback. pw-cat's flag set differs:
                    //   --playback selects direction
                    //   --raw     reads raw PCM rather than parsing a header
                    //   --format / --rate / --channels mirror paplay
                    //   -        the trailing "-" means stdin
                    let mut pw = mk("pw-cat");
                    pw.args(["--playback", "--raw",
                             "--format=s16", "--rate=48000",
                             "--channels=1", "-"]);
                    match pw.spawn() {
                        Ok(c) => c,
                        Err(_pw_err) => {
                            let mut aplay = mk("aplay");
                            aplay.args(["-q", "-f", "S16_LE", "-r", "48000",
                                        "-c", "1", "-t", "raw"]);
                            match aplay.spawn() {
                                Ok(c) => c,
                                Err(e) => {
                                    eprintln!("cue: no audio player found \
                                               (paplay/pw-cat/aplay all failed); \
                                               last err: {}", e);
                                    return;
                                }
                            }
                        }
                    }
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(&pcm);
                drop(stdin);
            }
            let _ = child.wait();
        });
    }

    /// Legacy single-thump cue. Kept for tests + diagnostics; the play
    /// path uses fire_estimate_cue now.
    #[allow(dead_code)]
    fn fire_cue(&self, bpm: f32, tonic_hz: f32, duration_ms: u32) {
        let pcm = pre_buffer::synthesize_cue_pcm(bpm, tonic_hz, duration_ms);
        Self::stream_pcm_to_player(pcm);
    }

    fn tick_anticipator(&mut self) {
        self.observe_action(anticipator::ActionKind::InactivityTick);
        // Drain any completed bluetoothctl attempts and feed them into
        // the scheduler so its backoff state stays accurate.
        self.drain_bt_results();
        // Sample MPRIS — what's playing in other apps on this machine.
        // Used for context-aware predictions + the "currently playing"
        // hint in the UI.
        self.refresh_mpris();
        // Refresh the user-visible time window label.
        let win_label = anticipator::TimeWindow::now_local().label();
        if self.current_time_window != win_label {
            self.current_time_window = win_label.to_string();
            self.time_window_changed();
        }
        // Reap any expired shadow with a coherence check: the user may
        // have manually adjusted volume during the shadow's lifetime. The
        // shadow set the device to vol=0; if last_known_volume is now
        // away from 0, someone touched it and we shouldn't blindly
        // restore the pre-shadow value.
        if let Some(state_ref) = self.shadow.current() {
            if state_ref.is_expired() {
                let saved = state_ref.saved_volume;
                let current = self.last_known_volume;
                let user_touched = current.abs() > 3;
                // Drop the shadow state.
                let _ = self.shadow.cancel();
                if user_touched {
                    eprintln!("shadow: expired but vol={} ≠ 0; user adjusted during \
                               shadow — leaving their value alone, just stopping stream",
                              current);
                    // Stop the playback but DON'T override the user's volume choice.
                    if let Some(tx) = &self.tx {
                        let _ = tx.send(Command::StopPlayback {
                            url: self.active_url.clone(),
                        });
                    }
                } else {
                    eprintln!("shadow: expired, restoring vol={}", saved);
                    if let (Some(url), Some(tx)) = (self.active_url_opt(), &self.tx) {
                        let _ = tx.send(Command::SetVolume { url, value: saved });
                        let _ = tx.send(Command::StopPlayback {
                            url: self.active_url.clone(),
                        });
                    }
                }
            }
        }
        // Persist learned transitions every tick (~30 s cadence). The
        // payload is small (a few hundred bytes JSON) and the SQLite upsert
        // is cheap — well worth doing routinely so learning compounds even
        // if the app crashes between sessions.
        let snapshot = self.anticipator.to_json();
        self.send_media(media::MediaCmd::SaveAnticipatorState(snapshot));
    }

    fn dismiss_failover(&mut self) {
        self.failover_active = false;
        self.failover_from = String::new();
        self.failover_to = String::new();
        self.failover_changed();
    }

    // ── Coordinated transport ────────────────────────────────────────

    /// Send a transport command to all hub-tier devices simultaneously.
    /// The action string matches the existing transport() argument:
    /// "play" | "pause" | "forward" | "back".
    fn transport_all(&mut self, action: String) {
        // Parse priority_devices_json + send the same command to every
        // PrimaryHub / SecondaryHub at once. We don't touch satellites or
        // listeners — they should mirror their hub naturally.
        let devices: Vec<serde_json::Value> =
            serde_json::from_str(&self.priority_devices_json).unwrap_or_default();
        let mut targets = 0;
        for d in &devices {
            let group = d.get("group").and_then(|v| v.as_str()).unwrap_or("");
            if group != "PrimaryHub" && group != "SecondaryHub" { continue; }
            let url = d.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if url.is_empty() { continue; }
            if let Some(tx) = &self.tx {
                let _ = tx.send(Command::Transport {
                    url,
                    action: action.clone(),
                });
            }
            targets += 1;
        }
        eprintln!("transport_all: '{}' sent to {} hubs", action, targets);
        // Update FSM: pause from anywhere → Paused
        match action.as_str() {
            "pause" => self.observe_action(anticipator::ActionKind::Pause),
            "play"  => self.observe_action(anticipator::ActionKind::Resume),
            "forward" => self.observe_action(anticipator::ActionKind::SkipNext),
            "back"    => self.observe_action(anticipator::ActionKind::SkipPrev),
            _ => {}
        }
    }

    /// Track a command failure on the active device. When N consecutive
    /// failures pile up, auto-failover to the next-best hub from the
    /// priority list. Resets to 0 on any successful response.
    fn note_active_failure(&mut self) {
        self.active_failures = self.active_failures.saturating_add(1);
        if self.active_failures < 3 { return; }   // 3 strikes
        let current_url = self.active_url.clone();
        // Display name for the failing device. product_name is only
        // populated after a successful info-fetch; for a device that
        // was never reachable (the most common failover trigger), it's
        // empty and we'd render "auto-failover:  (failing)" with the
        // gap. Fall back to the device's display name from either
        // priority_devices_json or devices_json so the toast always
        // names the failing device.
        let current_name = {
            let from_product = self.product_name.clone();
            if !from_product.is_empty() {
                from_product
            } else {
                self.lookup_device_name(&current_url)
                    .unwrap_or_else(|| current_url.clone())
            }
        };
        // Find the next-best hub that isn't the failing one.
        let devices: Vec<serde_json::Value> =
            serde_json::from_str(&self.priority_devices_json).unwrap_or_default();
        for d in &devices {
            let url = d.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let group = d.get("group").and_then(|v| v.as_str()).unwrap_or("");
            let reachable = d.get("reachable").and_then(|v| v.as_bool()).unwrap_or(false);
            if url == current_url { continue; }
            if !reachable { continue; }
            if group != "PrimaryHub" && group != "SecondaryHub" { continue; }
            let next_pid = d.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let next_name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            eprintln!("auto-failover: {} (failing) → {}", current_name, next_name);
            self.failover_active = true;
            self.failover_from = current_name.clone();
            self.failover_to = next_name;
            self.failover_changed();
            // Reset BEFORE switching: select_device_with_product calls
            // refresh_all, which kicks off new requests that could fail.
            // Without the reset, those failures pile on the old counter
            // and re-trigger this method, causing a flapping loop. The
            // circuit breaker also helps, but resetting here is the
            // correct policy: we've handed off to a new device, give it
            // a clean 3-strike window of its own.
            self.active_failures = 0;
            self.select_device_with_product(url, next_pid);
            return;
        }
        eprintln!("auto-failover: no alternative hub available (from {})", current_name);
        // Don't keep climbing the counter if there's nowhere to go —
        // the failures will keep coming. Reset so we re-evaluate cleanly
        // when a new alternative appears (e.g. a discovered device).
        self.active_failures = 0;
    }

    /// Look up a device's display name by URL across both known device
    /// sources. Used by failover messages when product_name is empty
    /// (which is normal for a device that was offline from the start).
    fn lookup_device_name(&self, url: &str) -> Option<String> {
        for src in [&self.priority_devices_json, &self.devices_json] {
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(src) {
                for d in &arr {
                    let d_url = d.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    if d_url == url {
                        if let Some(name) = d.get("name").and_then(|v| v.as_str()) {
                            if !name.is_empty() { return Some(name.to_string()); }
                        }
                    }
                }
            }
        }
        None
    }

    fn note_active_success(&mut self) {
        if self.active_failures > 0 { self.active_failures = 0; }
    }

    /// Fire a rate-limited connection-warm to the active device. The
    /// physical request is a GetVolume (cheapest endpoint that confirms
    /// the device is reachable); the response is observed by the normal
    /// Volume-result path, which also feeds the RTT tracker. No effect
    /// when the warmer's rate limit or per-URL cooldown rejects.
    fn fire_connection_warm(&mut self) {
        let url = match self.active_url_opt() {
            Some(u) => u,
            None => return,
        };
        let now = std::time::Instant::now();
        match self.conn_warmer.should_warm(&url, now) {
            conn_warmer::WarmDecision::Warm => {
                self.conn_warmer.note_warmed(&url, now);
                if let Some(tx) = &self.tx {
                    self.last_command_at = Some(now);
                    let _ = tx.send(Command::GetVolume { url });
                }
            }
            conn_warmer::WarmDecision::Cooldown => {
                // Already warm — no work needed
            }
            conn_warmer::WarmDecision::GlobalRateLimit => {
                // Hot loop guard — log once for diagnostics
                eprintln!("conn_warmer: global rate limit hit");
            }
        }
    }

    /// Send a Wake-on-LAN magic packet to every device whose MAC we
    /// know AND which isn't the currently active device. Rate-limited
    /// per-MAC via the WakeThrottler so a hot prediction loop can't
    /// flood. Best-effort: failures are logged but not surfaced to QML.
    ///
    /// MAC addresses come from the priority_devices_json data, which is
    /// populated by the device probe machinery. Devices without a
    /// reported MAC are skipped silently.
    fn fire_wol_for_known_devices(&mut self) {
        let devices: Vec<serde_json::Value> =
            serde_json::from_str(&self.priority_devices_json).unwrap_or_default();
        let active = self.active_url.clone();
        let now = std::time::Instant::now();
        for d in &devices {
            let url = d.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url == active || url.is_empty() { continue; }
            let mac = match d.get("mac").and_then(|v| v.as_str()) {
                Some(m) if !m.is_empty() => m,
                _ => continue,
            };
            match self.wol_throttler.check(mac, now) {
                wake_on_lan::WakeDecision::Send => {
                    match wake_on_lan::send_magic(mac, wake_on_lan::WOL_PORT) {
                        Ok(()) => {
                            self.wol_throttler.note_sent(mac, now);
                            eprintln!("wol: sent magic packet to {} ({})",
                                mac, d.get("name").and_then(|v| v.as_str()).unwrap_or(""));
                        }
                        Err(e) => eprintln!("wol: send to {} failed: {}", mac, e),
                    }
                }
                wake_on_lan::WakeDecision::Throttled { .. } => {
                    // Recently woken — no action
                }
            }
        }
    }

    /// Rebuild priority_devices_json from the current devices_json + the
    /// cached device_states. Sorts the result by priority (best first).
    /// Also updates recommended_hub_url / recommended_hub_product.
    fn rebuild_priority_json(&mut self) {
        let devices: Vec<serde_json::Value> =
            serde_json::from_str(&self.devices_json).unwrap_or_default();
        // Build a DeviceContext for each.
        let mut contexts: Vec<device_priority::DeviceContext> = Vec::new();
        for d in &devices {
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let url = d.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let pid = d.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if url.is_empty() { continue; }
            let state = self.device_states.get(&url).cloned().unwrap_or_default();
            contexts.push(device_priority::DeviceContext {
                product_id: pid.clone(),
                name,
                url,
                traits: device_priority::DeviceTraits::for_product(&pid),
                state,
            });
        }
        // Sort: best first.
        contexts.sort_by(|a, b| {
            let ga = device_priority::priority_group(a);
            let gb = device_priority::priority_group(b);
            ga.cmp(&gb).then_with(|| {
                let sa = device_priority::priority_score(a);
                let sb = device_priority::priority_score(b);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        // Serialize for QML.
        let arr: Vec<serde_json::Value> = contexts.iter().map(|c| {
            let group = device_priority::priority_group(c);
            let score = device_priority::priority_score(c);
            serde_json::json!({
                "name": c.name,
                "url": c.url,
                "type": c.product_id,
                "group": format!("{:?}", group),
                "group_label": group.label(),
                "group_color": group.badge_color(),
                "score": score,
                "explain": device_priority::explain(c),
                "reachable": c.state.reachable,
                "on_battery": c.state.on_battery,
                "battery_percent": c.state.battery_percent,
                "charging": c.state.charging,
                "on_ethernet": c.state.on_ethernet,
                "on_wifi": c.state.on_wifi,
                "bluetooth_active": c.state.bluetooth_active,
            })
        }).collect();
        self.priority_devices_json = serde_json::to_string(&arr)
            .unwrap_or_else(|_| "[]".into());

        // Recommended hub
        if let Some(pick) = device_priority::recommended_hub(&contexts) {
            self.recommended_hub_url = pick.url.clone();
            self.recommended_hub_product = pick.product_id.clone();
        } else {
            self.recommended_hub_url = String::new();
            self.recommended_hub_product = String::new();
        }
        self.priority_changed();
    }
    /// Full-text library search. Empty/whitespace query reverts to recent.
    fn library_search(&mut self, query: String) {
        let q = query.trim().to_string();
        if q.is_empty() {
            self.send_media(media::MediaCmd::LoadRecent);
        } else {
            self.send_media(media::MediaCmd::Search(q));
        }
    }

    /// Save (or clear) a manual override for a track. `override_json` is a
    /// JSON object with optional "title","artist","album","year". Empty
    /// object `{}` clears all override fields (parser values reappear).
    fn set_track_override(&mut self, track_id: i32, override_json: String) {
        let v: serde_json::Value = serde_json::from_str(&override_json).unwrap_or(serde_json::json!({}));
        let nz = |key: &str| -> Option<String> {
            v.get(key).and_then(|x| x.as_str())
             .map(|s| s.trim().to_string())
             .filter(|s| !s.is_empty())
        };
        let yr = v.get("year").and_then(|x| x.as_i64())
                  .and_then(|y| if y > 0 && y < 4000 { Some(y as u16) } else { None });
        self.send_media(media::MediaCmd::SetOverride {
            track_id: track_id as i64,
            title:  nz("title"),
            artist: nz("artist"),
            album:  nz("album"),
            year:   yr,
        });
    }

    fn load_key_bpm(&mut self, track_id: i32) {
        self.send_media(media::MediaCmd::LoadKeyBpmForTrack(track_id as i64));
    }

    /// Toggle auto-routing of KDE media
    fn toggle_auto_route(&mut self) {
        self.auto_route_kde = !self.auto_route_kde;
        self.auto_route_changed();

        if self.auto_route_kde {
            // Start monitoring - do initial check
            self.tx_check_audio();
        }
    }

    /// Send command to check for audio
    fn tx_check_audio(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::CheckKdeMedia);
        }
    }

    /// Toggle passthrough locally and also start/stop streaming
    fn toggle_passthrough(&mut self) {
        self.passthrough = !self.passthrough;
        self.passthrough_changed();

        if self.passthrough {
            // Start streaming - use a default stream URI if none set
            let uri = if self.stream_uri.is_empty() {
                "http://localhost:8000/stream.mp3".to_string()
            } else {
                self.stream_uri.clone()
            };
            self.start_stream(uri);
        } else {
            self.stop_stream();
        }
    }

    /// Start streaming audio from a URI using Mozart API
    fn start_stream(&mut self, uri: String) {
        self.stream_uri = uri.clone();
        self.stream_uri_changed();

        let url = match self.active_url_opt() {
            Some(u) => u,
            None => {
                eprintln!("start_stream: no active_url set");
                return;
            }
        };

        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::PlayMozartUri {
                url,
                stream_uri: uri
            });
        }
    }

    /// Stop playback
    fn stop_stream(&mut self) {
        let url = match self.active_url_opt() {
            Some(u) => u,
            None => {
                eprintln!("stop_stream: no active_url set");
                return;
            }
        };

        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::StopPlayback { url });
        }
    }

    /// Poll the results channel (call from QML Timer on main thread).
    /// This drains any pending results and applies them to Qt properties.
    fn poll_results(&mut self) {
        // Drain BT snapshot results first — they're independent of
        // the main rx channel and arrive asynchronously from detached
        // `bluetoothctl info` threads.
        self.drain_bt_snapshot();
        // Same for BT power-state probes.
        self.drain_bt_power();
        // Async paired-device discovery results.
        self.drain_bt_discovery();
        if self.rx.is_none() {
            return;
        }

        // Collect all messages first to avoid borrow issues
        let mut messages = Vec::new();
        let rx = self.rx.as_ref().unwrap();
        loop {
            match rx.try_recv() {
                Ok(msg) => messages.push(msg),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    eprintln!("poll_results: channel disconnected");
                    break;
                }
            }
        }

        // Now process all messages
        for msg in messages {
            match msg {
                ResultMessage::Volume(v) => {
                    self.note_active_success();
                    let reported = v.clamp(0, 100);
                    // Measure RTT: if we have a command-issue timestamp,
                    // the elapsed time approximates the network round-trip
                    // for this particular command. The first sample after
                    // startup may be inflated by initialization costs;
                    // the rolling window absorbs that noise quickly.
                    if let Some(issued) = self.last_command_at.take() {
                        let rtt = issued.elapsed();
                        if rtt < std::time::Duration::from_secs(5) {
                            self.rtt.observe(rtt);
                            // Update per-device tracker too. Keyed on the
                            // CURRENT active_url — the result message
                            // doesn't carry the URL itself, so we attribute
                            // the round-trip to whichever device was
                            // active at the time of issue. This is a
                            // reasonable approximation: failover may have
                            // changed active_url mid-flight, but in that
                            // case the next sample correctly attributes
                            // to the new device.
                            let key = self.active_url.clone();
                            if !key.is_empty() {
                                let entry = self.rtt_per_device
                                    .entry(key)
                                    .or_insert_with(rtt_tracker::RttTracker::new);
                                entry.observe(rtt);
                            }
                            self.rtt_last_ms = rtt.as_millis() as i32;
                            self.rtt_p50_ms = self.rtt.p50()
                                .map(|d| d.as_millis() as i32).unwrap_or(0);
                            self.rtt_p95_ms = self.rtt.p95()
                                .map(|d| d.as_millis() as i32).unwrap_or(0);
                            self.rtt_changed();
                        }
                    }
                    // Bump volume_epoch — managers like the shadow stream
                    // use this to detect "actual volume changed since I
                    // saved my pre-shadow value". Any committed change
                    // to the device-confirmed volume bumps.
                    self.volume_epoch.bump();
                    // Clone so we can call &mut self methods below without
                    // holding an immutable borrow on self.active_product_id.
                    let pid = self.active_product_id.clone();
                    let ceiling = volume_safety::safe_startup_volume(&pid);
                    // Skip the startup clamp if the user has previously
                    // acknowledged this device URL — they've already
                    // explicitly chosen a loud level on a past session
                    // and being yanked down on every restart is more
                    // annoying than helpful for a known-trusted device.
                    let already_acked = volume_safety_ack::is_acked(&self.active_url);
                    if !self.safety_check_done && reported > ceiling && !already_acked {
                        // The "{label}" slot is empty when product
                        // info hasn't loaded yet — fall back to
                        // "(unknown product)" so the log line reads
                        // cleanly instead of "clamping  from 41".
                        let label = if pid.is_empty() {
                            "(unknown product)".to_string()
                        } else { pid.clone() };
                        eprintln!(
                            "volume_safety: clamping {} from {} -> {} (≈{:.0} dB) on {}",
                            label, reported, ceiling,
                            volume_safety::approx_db_at(&pid, ceiling),
                            self.active_url,
                        );
                        self.volume = ceiling;
                        self.set_volume_api(ceiling);
                        self.volume_was_clamped = true;
                        self.volume_clamped_changed();
                    } else {
                        if already_acked && reported > ceiling {
                            // Quiet log line so a debugging user can see
                            // the safety check noticed the loud level
                            // but the persistent ack overrode the clamp.
                            eprintln!(
                                "volume_safety: device {} reported {} (>{}) but is acked; \
                                 leaving volume alone", self.active_url, reported, ceiling);
                        }
                        self.volume = reported;
                    }
                    self.safety_check_done = true;
                    self.active_db_estimate =
                        volume_safety::approx_db_at(&pid, self.volume) as f64;
                    self.volume_is_loud = self.volume > volume_safety::loud_threshold(&pid);
                    self.last_known_volume = self.volume;
                    // Persist this device's volume so switching away and
                    // back restores ITS level, not whatever the previously-
                    // active device was at. Keyed per device URL. Only
                    // persist sane values (>0) — a 0 is usually a shadow/
                    // ducking artifact, not a level the user chose.
                    if !self.active_url.is_empty() && self.volume > 0 {
                        let vkey = format!("device_volume:{}", self.active_url);
                        let _ = settings_store::set_str(&vkey, &self.volume.to_string());
                    }
                    // Re-anchor the gesture filter to the authoritative
                    // device value. This keeps a sane baseline if the
                    // volume changed via physical buttons on the speaker,
                    // another controller in the cluster, or our own
                    // SetVolume call that the filter then needs to track.
                    self.volume_gesture.anchor(self.volume);
                    self.volume_changed();
                    self.url_changed();
                }
                ResultMessage::MuteState(m) => {
                    self.muted = m;
                    self.muted_changed();
                }
                ResultMessage::ProductInfo { name, type_name } => {
                    self.product_name = name;
                    self.product_type = type_name;
                    self.product_info_changed();
                }
                ResultMessage::KdeMediaPlaying(active) => {
                    self.kde_media_active = active;
                    self.kde_media_changed();

                    // Auto-route if enabled
                    if active && self.auto_route_kde && !self.passthrough {
                        self.toggle_passthrough();
                    }
                }
                ResultMessage::Sources { json, active_id } => {
                    self.sources_json = json;
                    self.active_source_id = active_id;
                    self.sources_changed();
                }
                ResultMessage::NowPlaying { title, artist, source, playing } => {
                    self.np_title = title;
                    self.np_artist = artist;
                    self.np_source = source;
                    self.np_playing = playing;
                    self.now_playing_changed();
                }
                ResultMessage::Standby(s) => {
                    self.standby = s;
                    self.standby_changed();
                }
                ResultMessage::SoundModes { json, active_id } => {
                    self.sound_modes_json = json;
                    self.active_sound_mode = active_id;
                    self.sound_modes_changed();
                }
                ResultMessage::DeviceDiscovered { name, url } => {
                    // Reject malformed URLs at the gate. mDNS often
                    // surfaces a device at TWO URLs: a routable IPv4
                    // (good) and an IPv6 link-local (useless because
                    // reqwest can't reach it without a scope id). We
                    // keep the IPv4 and silently drop the link-local
                    // so it never ends up in the picker or the
                    // failover candidates.
                    if !is_well_formed_http_url(&url) {
                        eprintln!("discover: ignoring unreachable URL {} for {}",
                                  url, name);
                        continue;
                    }
                    // Merge into the device list, de-duping on url.
                    let mut list: Vec<serde_json::Value> =
                        serde_json::from_str(&self.devices_json).unwrap_or_default();
                    let exists = list.iter()
                        .any(|d| d.get("url").and_then(|u| u.as_str()) == Some(url.as_str()));
                    if !exists {
                        eprintln!("discovered device: {} @ {}", name, url);
                        list.push(serde_json::json!({ "name": name, "url": url, "discovered": true }));
                        self.devices_json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
                        self.devices_changed();
                        // If this is the currently-active device AND
                        // its product_id is still unknown (mozart_legacy
                        // firmware returns 500 for the product-info
                        // endpoints), seed an inference from the mDNS-
                        // discovered friendly name. This gets the
                        // volume_safety ceilings right for the real
                        // hardware — without it, the Stage at
                        // 192.168.2.12 falls into the generic "unknown"
                        // 95 dB ceiling and the startup clamp uses an
                        // overly-conservative 18% volume limit.
                        if url == self.active_url && self.active_product_id.is_empty() {
                            if let Some(pid) = volume_safety::infer_product_id(&name) {
                                eprintln!("product: inferred {} from mDNS name '{}'",
                                    pid, name);
                                self.active_product_id = pid.to_string();
                                if self.product_name.is_empty() {
                                    self.product_name = name.clone();
                                }
                                self.product_info_changed();
                            }
                        }
                        // Probe the newcomer's capabilities so it lands in
                        // the right priority bucket right away. The probe
                        // is async — we'll rebuild the JSON when it lands.
                        if let Some(tx) = &self.tx {
                            let _ = tx.send(Command::ProbeDeviceCapabilities {
                                url: url.clone(),
                            });
                        }
                        // Render a row immediately with whatever we know
                        // (likely just intrinsic traits) so the UI doesn't
                        // wait for the probe to surface the device.
                        self.rebuild_priority_json();
                    }
                }
                ResultMessage::NotifyUnsupported { url } => {
                    // First time we've seen this device come back as
                    // unsupportive of push? Persist so we skip the
                    // discovery probe on next launch.
                    if self.no_notify_devices.insert(url.clone()) {
                        eprintln!("notify_blocklist: {} marked as no-notify \
                                   (will skip discovery on next launch)", url);
                        let json = serde_json::to_string(
                            &self.no_notify_devices.iter().collect::<Vec<_>>()
                        ).unwrap_or_else(|_| "[]".into());
                        self.send_media(media::MediaCmd::SaveAppKv {
                            key: "no_notify_devices".to_string(),
                            value: json,
                        });
                    }
                }
                ResultMessage::PlayOutcome { used_endpoint, summary, url } => {
                    // Speculation success: the worker uses a SPEC_LOADED:
                    // prefix on the summary to signal "this PlayOutcome
                    // is a speculative pre-load completion, not a user-
                    // initiated play". Don't surface it to the UI; just
                    // update the speculative stager.
                    if let Some(rest) = summary.strip_prefix("SPEC_LOADED:") {
                        // Format: SPEC_LOADED:<track_id>:<device_url>
                        if let Some((tid_str, device_url)) = rest.split_once(':') {
                            if let Ok(track_id) = tid_str.parse::<i64>() {
                                self.speculative.mark_loaded(
                                    device_url, track_id, std::time::Instant::now());
                                eprintln!("speculative_play: marked track {} loaded on {}",
                                          track_id, device_url);
                            }
                        }
                        continue;
                    }
                    self.last_play_status = if used_endpoint.is_empty() {
                        // None of the play strategies got a 2xx. Show
                        // the cleanest summary; the verbose per-endpoint
                        // log is already in stderr.
                        format!("device refused all play endpoints — check console")
                    } else {
                        format!("playing via {}", used_endpoint)
                    };
                    self.last_play_url = url.clone();
                    self.play_status_changed();
                    eprintln!("play_outcome: {}", summary);

                    // ── ALL-FAILED outcome → classify + persist + BT retry ─
                    //
                    // When every endpoint failed, this is almost
                    // certainly a `mozart_legacy` device the firmware
                    // probe hasn't classified yet. We fingerprint by
                    // looking at the SUMMARY string play_uri emits
                    // (the per-endpoint stderr lines aren't visible
                    // to us here — they go straight to console).
                    //
                    // The summary format play_uri uses is:
                    //   "no endpoint accepted (404 loadUri; 404 X; …;
                    //    500 setData?path=BeoSound%3A%2Fplay&...; …)"
                    //
                    // We need a substring that's:
                    //   - present whenever the device is legacy mozart
                    //   - absent in every other failure mode
                    //
                    // "500 setData?path=BeoSound" satisfies both. The
                    // /api/setData endpoint with the BeoSound%3A%2F
                    // path is the legacy-mozart-only API surface;
                    // modern Mozart routes play through /BeoZone and
                    // returns 404 on setData. A 500 specifically on
                    // setData?path=BeoSound is the lock.
                    //
                    // (My previous version checked for the response
                    // body string "path not whitelisted" which is
                    // ONLY in the per-endpoint stderr, not in
                    // summary, so the detection never fired — see
                    // the user's continuing log of failed attempts.)
                    if used_endpoint.is_empty() && !url.is_empty() {
                        // Always log when we hit ALL_FAILED so the
                        // user can see whether the post-failure
                        // logic runs. Without this you can't tell
                        // "the branch didn't fire" from "fired but
                        // had nothing to retry with".
                        eprintln!(
                            "play_outcome: ALL_FAILED detected for url={} \
                             active_url={} active_bt_mac='{}' \
                             current_driver_kind='{}' \
                             last_user_play_request={:?} \
                             last_played_track_id={}",
                            url, self.active_url, self.active_bt_mac,
                            self.current_driver_kind,
                            self.last_user_play_request,
                            self.last_played_track_id);

                        // Classification: we ALWAYS persist + retry on
                        // ALL_FAILED. The fingerprint just chooses the
                        // label we record.
                        //
                        // - "500 setData?path=BeoSound" → mozart_legacy
                        //   (the Stage at .12: legacy whitelist refuses
                        //   the play path)
                        // - all-404 from nginx + a JSON 404 on setData →
                        //   "no_rest_playback" (the Emerge at .14 case
                        //   in the user's most recent log: nginx returns
                        //   404 on /BeoZone/* because those endpoints
                        //   aren't configured, and /api/setData routes
                        //   to a backend that says Not found)
                        // - anything else → "no_rest_playback" generic
                        //   verdict
                        //
                        // For both labels the user-facing remediation
                        // is identical: route via Bluetooth (or local)
                        // because there is no REST way to make this
                        // device play a URI. So gating the retry on
                        // the fingerprint was a mistake — broaden it.
                        let (verdict_label, verdict_reason) =
                            if summary.contains("500 setData?path=BeoSound") {
                                ("mozart_legacy",
                                 "matched '500 setData?path=BeoSound' \
                                  in summary (Stage / legacy Mozart \
                                  whitelist)")
                            } else {
                                ("no_rest_playback",
                                 "no '500 setData?path=BeoSound' match \
                                  — device's REST surface lacks any \
                                  working URI-play endpoint")
                            };
                        eprintln!(
                            "play_outcome: classifier verdict='{}' reason: {}",
                            verdict_label, verdict_reason);

                        // Persist no matter what — every ALL_FAILED is
                        // a "this device can't play URIs" verdict.
                        {
                            let key = format!("device_family:{}", url);
                            match settings_store::set_str(&key, verdict_label) {
                                Ok(()) => eprintln!(
                                    "device_memory: persisted '{}' = {}",
                                    url, verdict_label),
                                Err(e) => eprintln!(
                                    "device_memory: WARN — failed to persist \
                                     verdict for '{}': {} (will re-detect \
                                     next session)", url, e),
                            }
                        }

                        // Update live state if this is the active device.
                        // For "no_rest_playback" we don't have a perfect
                        // FirmwareFamily enum value — use LegacyMozart's
                        // playbook (REST off, BT recommended) since the
                        // operational consequences are identical.
                        if url == self.active_url {
                            eprintln!(
                                "play_outcome: updating live firmware state \
                                 for active device → {} + rebuild_driver",
                                verdict_label);
                            self.device_firmware_family = verdict_label.to_string();
                            self.device_firmware_label  = match verdict_label {
                                "mozart_legacy" => firmware_family::FirmwareFamily::LegacyMozart
                                    .label().to_string(),
                                _ => "Bluetooth-only (REST playback unavailable)".to_string(),
                            };
                            self.device_supports_rest_playback = false;
                            self.device_recommends_bluetooth   = true;
                            self.device_firmware_changed();
                            self.rebuild_driver();
                            eprintln!(
                                "play_outcome: after rebuild_driver, \
                                 current_driver_kind='{}'",
                                self.current_driver_kind);
                        }

                        // Retry path: unchanged shape, just always runs.
                        let want_retry = self.last_user_play_request
                            .or(if self.last_played_track_id > 0 {
                                Some(self.last_played_track_id)
                            } else { None });
                        match want_retry {
                            None => eprintln!(
                                "play_outcome: no track-id available for retry \
                                 (last_user_play_request={:?}, \
                                 last_played_track_id={}) — user must tap again",
                                self.last_user_play_request,
                                self.last_played_track_id),
                            Some(tid) => {
                                if !self.active_bt_mac.is_empty() {
                                    eprintln!(
                                        "play_outcome: active_bt_mac='{}' is \
                                         already set; play_track's own dispatch \
                                         will route through Bluetooth this time",
                                        self.active_bt_mac);
                                    self.play_track(tid as i32);
                                } else {
                                    // Live-sink-preferring pick (same
                                    // logic as play_track's auto-bind).
                                    let pactl_macs = collect_live_bluez_macs();
                                    let mut cand: Option<(String, String)> = None;
                                    let mut considered = 0usize;
                                    for (mac, cfg) in self.bt_autoconnect_iter() {
                                        considered += 1;
                                        let mac_norm = mac.replace(':', "_").to_uppercase();
                                        let has_live_sink = pactl_macs.contains(&mac_norm);
                                        eprintln!(
                                            "play_outcome: considering BT \
                                             candidate '{}' (mac={}) \
                                             auto_connect={} last_success={} \
                                             has_live_pactl_sink={}",
                                            cfg.name, mac, cfg.auto_connect,
                                            cfg.last_success.is_some(),
                                            has_live_sink);
                                        if has_live_sink {
                                            cand = Some((mac, cfg.name));
                                            break;
                                        }
                                    }
                                    // Fallback to last_success only if
                                    // pactl shows no live bluez sinks at
                                    // all (otherwise we'd pick a sink
                                    // that doesn't exist and ffmpeg
                                    // would exit 224 from broken pipe).
                                    if cand.is_none() && pactl_macs.is_empty() {
                                        for (mac, cfg) in self.bt_autoconnect_iter() {
                                            if cfg.last_success.is_some() {
                                                cand = Some((mac, cfg.name));
                                                break;
                                            }
                                        }
                                    }
                                    match cand {
                                        Some((mac, name)) => {
                                            eprintln!(
                                                "play_outcome: retrying track {} \
                                                 via BT speaker '{}' ({}) — \
                                                 REST refused on {}",
                                                tid, name, mac, url);
                                            self.set_active_bt_mac(mac, name);
                                            self.play_track(tid as i32);
                                        }
                                        None => eprintln!(
                                            "play_outcome: would retry via BT \
                                             but no auto-connected speaker is \
                                             available (considered {} \
                                             candidates); user must enable \
                                             auto-connect for at least one BT \
                                             device on the Connect page",
                                            considered),
                                    }
                                }
                            }
                        }
                    }
                }
                ResultMessage::DeviceStateProbed {
                    url, reachable, on_battery, battery_percent, charging,
                    on_ethernet, on_wifi, bluetooth_active,
                } => {
                    self.device_states.insert(url.clone(), device_priority::DeviceState {
                        reachable, on_battery, battery_percent, charging,
                        on_ethernet, on_wifi, bluetooth_active,
                    });
                    self.rebuild_priority_json();
                }
                ResultMessage::ActiveSourceProbed {
                    url, id, label, family, can_play, block_reason,
                } => {
                    // Only adopt the result if it's for the CURRENT
                    // active device — a stale result for a previous
                    // device shouldn't overwrite fresh state.
                    if url == self.active_url {
                        self.device_source_id           = id;
                        self.device_source_label        = label;
                        self.device_source_family       = family;
                        self.device_source_can_play     = can_play;
                        self.device_source_block_reason = block_reason;
                        self.device_source_changed();
                    }
                }
                ResultMessage::NetworkSourceSwitched { url, success, json } => {
                    // Only adopt the result if it's for the CURRENT
                    // active device (same reasoning as
                    // ActiveSourceProbed). Stale switches for a
                    // device the user already left would otherwise
                    // overwrite fresh state.
                    if url == self.active_url {
                        self.network_switch_running = false;
                        self.network_switch_json = json;
                        self.network_switch_changed();
                        eprintln!("network_switch: result for {} → success={}",
                                  url, success);
                    }
                }
                ResultMessage::FirmwareProbed { url, family, json } => {
                    // Adopt the result only for the active device.
                    // Other devices' firmware family is cached in
                    // their device records but the QML-bound state
                    // tracks the currently-selected one.
                    if url == self.active_url {
                        let fam = firmware_family::family_from_tag(&family);
                        self.device_firmware_family = family;
                        self.device_firmware_label = fam.label().to_string();
                        self.device_firmware_diagnostic_json = json;
                        self.device_supports_rest_playback =
                            fam.supports_rest_uri_playback();
                        self.device_recommends_bluetooth =
                            fam.recommends_bluetooth_fallback();
                        self.device_firmware_changed();
                        // The driver depends on firmware family —
                        // rebuild it whenever the family is updated.
                        self.rebuild_driver();
                    }
                }
                ResultMessage::DriverResult(msg) => {
                    // Empty msg = success. Non-empty = error string
                    // that should surface in the UI.
                    self.driver_last_error = msg;
                    self.driver_changed();
                }
                ResultMessage::TrackEnded => {
                    // The driver finished playing a track naturally
                    // (paplay exited 0). Drive queue auto-advance:
                    // if there's a queued track after the one that
                    // just ended, play it. Otherwise just clear the
                    // playing state.
                    eprintln!("track ended naturally — checking queue for next");
                    self.advance_queue_if_any();
                }
                ResultMessage::DriverVolume { pct } => {
                    // Two responsibilities now:
                    //   1. Sync the slider with the pactl-reported
                    //      volume (KDE-side changes etc.)
                    //   2. Watchdog: if the driver claims to be playing
                    //      but pactl can't read the sink for two
                    //      polls in a row, the sink probably went
                    //      away (BT speaker powered off, range lost,
                    //      USB DAC unplugged). Stop before PA migrates
                    //      the stream to a fallback sink the user
                    //      didn't pick (the "audio jumped to laptop
                    //      speakers" bug — real fix, not just a Stop
                    //      button).
                    //   3. Outlier rejection: pactl can briefly report
                    //      stale or default values during a BT sink
                    //      renegotiation (typical pattern: slider
                    //      21 → 100 in a single poll, then 22 → 45
                    //      on the next). A 50+ pt jump on a single
                    //      poll is almost never a real user action;
                    //      we require a second consistent reading
                    //      before adopting it, to avoid yanking the
                    //      slider around during transient blips.
                    match pct {
                        Some(p) => {
                            self.sink_unreachable_strikes = 0;
                            let p = p as i32;
                            // Grace window: if the user set the volume in
                            // the last 2.5s, ignore pactl echoes for the
                            // slider. BT speakers report back a value a
                            // couple points off what we set; adopting it
                            // makes the slider oscillate against the user.
                            // The watchdog (strike reset above) still runs.
                            let in_grace = self.last_user_volume_at
                                .map(|t| t.elapsed().as_millis() < 2500)
                                .unwrap_or(false);
                            if in_grace {
                                // Hold the user's value; clear any pending
                                // big-jump candidate so a stale blip from
                                // before the gesture doesn't get adopted
                                // the moment the grace window closes.
                                // `continue` (not return) so the rest of
                                // this poll's messages still get processed.
                                self.driver_volume_pending = None;
                                continue;
                            }
                            let delta = (p - self.volume).abs();
                            if delta >= 2 {
                                if delta > 50 {
                                    // Big jump — require a second
                                    // consistent reading before
                                    // adopting. Track the proposed
                                    // value; on the next tick if
                                    // it matches, adopt.
                                    if self.driver_volume_pending == Some(p) {
                                        eprintln!("driver_volume: confirmed big jump \
                                                   {} → {} (two consistent reads)",
                                                  self.volume, p);
                                        self.volume = p;
                                        self.driver_volume_pending = None;
                                        self.volume_changed();
                                    } else {
                                        eprintln!("driver_volume: rejecting outlier \
                                                   {} → {} (Δ={}); waiting for confirm",
                                                  self.volume, p, delta);
                                        self.driver_volume_pending = Some(p);
                                    }
                                } else {
                                    eprintln!("driver_volume: sync slider {} → {}",
                                              self.volume, p);
                                    self.volume = p;
                                    self.driver_volume_pending = None;
                                    self.volume_changed();
                                }
                            } else {
                                self.driver_volume_pending = None;
                            }
                        }
                        None => {
                            // Only count strikes when something is
                            // actually playing — otherwise a None on
                            // an idle Local driver (the controller's
                            // default sink might be unavailable at
                            // boot, etc.) shouldn't trigger anything.
                            let playing = self.current_driver.as_ref()
                                .map(|d| d.is_playing())
                                .unwrap_or(false);
                            if !playing {
                                self.sink_unreachable_strikes = 0;
                                return;
                            }
                            self.sink_unreachable_strikes += 1;
                            eprintln!(
                                "driver_volume: sink unreachable strike {}/2 \
                                 (driver kind={}, label={})",
                                self.sink_unreachable_strikes,
                                self.current_driver_kind,
                                self.current_driver.as_ref()
                                    .map(|d| d.label()).unwrap_or_default(),
                            );
                            if self.sink_unreachable_strikes >= 2 {
                                eprintln!("driver_volume: sink confirmed gone — stopping playback");
                                // Build a user-visible status before
                                // stopping so the panel reflects what
                                // happened instead of silently halting.
                                let sink_label = match &self.current_driver {
                                    Some(device_driver::AnyDriver::Bluetooth(d)) =>
                                        if d.name.is_empty() { d.mac.clone() }
                                        else { d.name.clone() },
                                    Some(device_driver::AnyDriver::MultiRoom(d)) =>
                                        format!("multi-room ({} speakers)", d.members.len()),
                                    Some(device_driver::AnyDriver::Local(_)) =>
                                        "system default sink".to_string(),
                                    _ => "audio output".to_string(),
                                };
                                self.last_play_status = format!(
                                    "{} disappeared — playback stopped \
                                     (re-bind a sink to keep listening)",
                                    sink_label
                                );
                                self.play_status_changed();
                                self.driver_stop();
                                self.sink_unreachable_strikes = 0;
                            }
                        }
                    }
                }
                ResultMessage::MultiRoomCreated { driver, error } => {
                    if let Some(drv) = driver {
                        let members = match &drv {
                            device_driver::AnyDriver::MultiRoom(d) => d.members.clone(),
                            _ => vec![],
                        };
                        let macs: Vec<String> = members.iter()
                            .map(|(m, _)| m.clone()).collect();
                        let names: Vec<String> = members.iter()
                            .map(|(_, n)| n.clone()).collect();
                        let label = drv.label();
                        self.current_driver = Some(drv);
                        self.current_driver_kind = "multiroom".to_string();
                        self.current_driver_label = label.clone();
                        self.active_multiroom_json = serde_json::to_string(
                            &serde_json::json!({"macs": macs, "names": names})
                        ).unwrap_or_else(|_| "null".into());
                        self.active_bt_mac = String::new();
                        self.active_bt_name = String::new();
                        self.driver_last_error = String::new();
                        self.driver_changed();
                        // Persist this binding for the active device.
                        self.persist_device_sink_binding();
                        eprintln!("multiroom: driver installed → {}", label);
                    } else {
                        self.driver_last_error = format!(
                            "multi-room create failed: {}", error
                        );
                        self.driver_changed();
                        eprintln!("multiroom: create failed: {}", error);
                    }
                }
                ResultMessage::Error(e) => {
                    eprintln!("background error: {}", e);
                    // Network-style errors against the active device count
                    // toward auto-failover. We don't trigger on every error
                    // (some are 4xx because the device doesn't support a
                    // specific endpoint) — only ones that look like the
                    // device is unreachable.
                    let lc = e.to_lowercase();
                    if lc.contains("failed") && (
                        lc.contains("connect") || lc.contains("timeout")
                        || lc.contains("dns") || lc.contains("os error")
                        || lc.contains("refused") || lc.contains("unreachable")
                    ) {
                        self.note_active_failure();
                    }
                }
            }
        }

        // ── Drain the media-worker channel too ──────────────────────
        if self.media_rx.is_some() {
            let mut media_msgs = Vec::new();
            {
                let mrx = self.media_rx.as_ref().unwrap();
                loop {
                    match mrx.try_recv() {
                        Ok(m) => media_msgs.push(m),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    }
                }
            }
            for m in media_msgs {
                self.apply_media_result(m);
            }
        }
    }

    /// Apply one MediaResult to the exposed Qt properties.
    fn apply_media_result(&mut self, m: media::MediaResult) {
        match m {
            media::MediaResult::TrackList { context, json } => {
                // Album track lists go to the album-detail properties,
                // not the generic library_json. The context starts
                // with "album:<title>:<primary_artist>".
                if context.starts_with("album:") {
                    // Only adopt the result if it matches the album
                    // we currently have open (stale result from a
                    // previous open_album shouldn't overwrite fresh
                    // state).
                    let expected = format!(
                        "album:{}:{}",
                        self.current_album_title,
                        self.current_album_artist,
                    );
                    if context == expected {
                        self.current_album_tracks_json = json;
                        self.current_album_changed();
                    }
                } else {
                    self.library_context = context;
                    self.library_json = json;
                    self.library_changed();
                    // Refresh the shadow candidate: the topmost track
                    // in the visible list is the most likely tap.
                    // Used by the shadow stream pre-staging path.
                    let list: Vec<serde_json::Value> =
                        serde_json::from_str(&self.library_json).unwrap_or_default();
                    self.shadow_candidate = list.first()
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_i64());
                }
            }
            media::MediaResult::GroupList { context, json } => {
                if context == "artists" || context.starts_with("artist") {
                    self.artists_json = json;
                    self.artists_changed();
                } else if context == "tags" || context.starts_with("tag") {
                    self.tags_json = json;
                    self.tags_changed();
                }
            }
            media::MediaResult::Page { context, json, offset, total } => {
                // Paginated load result. Routes by context exactly
                // like TrackList/GroupList, but also publishes the
                // total + offset so QML can render its scrollbar
                // and decide when to fetch the next page.
                //
                // Append vs replace decision is the QML side's
                // responsibility: it knows whether this page came
                // from an initial load (offset=0 → replace model)
                // or a scroll-driven fetch (offset>0 → concat).
                //
                // We expose the trio (json + offset + total) via a
                // single qt_property `page_json` that QML reads.
                // Format is:
                //   {"context": "...", "offset": N, "total": N,
                //    "items": [ ... ]}
                // Wrapping in a single JSON makes the QML side a
                // straight `JSON.parse()` instead of having to
                // synchronize three properties whose updates land
                // in slightly different orders.
                let wrapped = serde_json::json!({
                    "context": context,
                    "offset": offset,
                    "total": total,
                    "items": serde_json::from_str::<serde_json::Value>(&json)
                        .unwrap_or(serde_json::json!([])),
                });
                self.page_json = wrapped.to_string();
                self.page_changed();
            }
            media::MediaResult::BpmBandCounts(_json) => {
                // Reserved for the BPM-band browse UI (Turn 3).
            }
            media::MediaResult::TrackRating { track_id, rating } => {
                // Confirmed rating write — expose for the rating widget to
                // reconcile its optimistic state.
                self.last_rated_track = track_id;
                self.last_rated_value = rating;
                self.rating_changed();
            }
            media::MediaResult::TagWriteResult { track_id, ok, message } => {
                eprintln!("tag write for {}: ok={} ({})", track_id, ok, message);
                self.tag_write_status = message.into();
                self.tag_write_status_changed();
                // A successful file write means the DB-override overlay is
                // now redundant for those fields, but we leave it; a rescan
                // will pick up the file's new tags. Trigger a library
                // refresh so the UI reflects the change immediately.
                if ok {
                    self.send_media(media::MediaCmd::ListPlaylists);
                }
            }
            media::MediaResult::ScanProgress(p) => {
                self.scan_phase = format!("{:?}", p.phase);
                self.scan_done = p.done as i32;
                self.scan_total = p.total as i32;
                self.scan_changed();
            }
            media::MediaResult::CatalogStats(s) => {
                self.lib_total = s.total as i32;
                self.lib_parsed = s.parsed as i32;
                self.lib_analyzed = s.analyzed as i32;
                self.catalog_stats_changed();
            }
            media::MediaResult::ResumeState(_r) => {
                // Reserved for the home "resume" card (Turn 3).
            }
            media::MediaResult::NowPlayingKeyBpm { bpm, camelot, key_text } => {
                self.np_bpm      = bpm.map(|v| v as f64).unwrap_or(0.0);
                self.np_camelot  = camelot.unwrap_or_default();
                self.np_key_text = key_text.unwrap_or_default();
                self.np_key_changed();
            }
            media::MediaResult::TrackForPlay { track_id, path, title, artist, duration_secs } => {
                // Determine whether this resolution was triggered by a real
                // user tap or by the anticipator's pre-stage. We POST only
                // on real taps; pre-stages just cache the URL for future use.
                let is_user_request = self.last_user_play_request == Some(track_id);
                if is_user_request { self.last_user_play_request = None; }

                // Lazily bind the local HTTP server on the first play. If
                // binding fails (port in use), report it via last_play_status
                // and bail — there's no point asking the device to fetch a
                // URL we can't actually serve.
                if self.track_server.is_none() {
                    match track_server::TrackServer::bind(track_server::DEFAULT_PORT) {
                        Ok(s) => self.track_server = Some(Arc::new(s)),
                        Err(e) => {
                            // Try an OS-assigned port as a fallback.
                            match track_server::TrackServer::bind(0) {
                                Ok(s) => self.track_server = Some(Arc::new(s)),
                                Err(e2) => {
                                    if is_user_request {
                                        self.last_play_status =
                                            format!("track server bind failed: {} / {}", e, e2);
                                        self.play_status_changed();
                                    }
                                    return;
                                }
                            }
                        }
                    }
                    // First-time bind: do a LAN-reachability probe.
                    // On Alpine/postmarketOS and some other distros
                    // the default firewall (nftables/iptables INPUT
                    // chain) often blocks non-standard ports like
                    // 8473. The probe TCP-connects from our own LAN
                    // IP to the bound port — same path the device
                    // takes. If that fails, the device's fetch will
                    // also fail and the user gets a clear hint to
                    // open the port (or pick a standard one). Only
                    // probe if we know a peer IP; if no device is
                    // selected yet we'll probe lazily.
                    if let (Some(srv), Some(peer_ip)) = (
                        self.track_server.as_ref(),
                        url_to_ip(&self.active_url),
                    ) {
                        if let Err(msg) = srv.check_reachable_from_lan(peer_ip) {
                            eprintln!("track_server: REACHABILITY WARNING — {}", msg);
                            // Surface to the UI so the user can act
                            // on it. Doesn't block the play attempt
                            // — sometimes the probe is wrong (e.g.
                            // the firewall has stateful rules that
                            // permit established-by-device but not
                            // self-loopback connections). Better to
                            // try and let the actual play either
                            // succeed or fail with a clear message.
                            self.last_play_status = format!(
                                "warning: {} (playback may fail)", msg);
                            self.play_status_changed();
                        }
                    }
                }
                let server = self.track_server.as_ref().unwrap().clone();
                let device_url = self.active_url.clone();
                if device_url.is_empty() && is_user_request {
                    self.last_play_status = "no active device".to_string();
                    self.play_status_changed();
                    return;
                }
                let peer_ip = url_to_ip(&device_url);
                let path_buf = std::path::PathBuf::from(&path);
                if !path_buf.exists() {
                    if is_user_request {
                        self.last_play_status = format!("file not found: {}", path);
                        self.play_status_changed();
                    }
                    return;
                }
                let segment = server.expose(path_buf);
                let track_url = match peer_ip {
                    Some(ip) => server.url_for(ip, &segment),
                    None => {
                        // peer_ip = None means url_to_ip couldn't
                        // extract an IP from device_url and DNS
                        // resolution also failed. The old code fell
                        // back to `http://127.0.0.1:8473/...` here —
                        // which the REMOTE Mozart device can never
                        // reach (it's the device's own loopback,
                        // not the controller's). Playback then
                        // silently failed with no diagnostic. Now
                        // we surface the real error.
                        if is_user_request {
                            self.last_play_status = format!(
                                "can't reach device — couldn't resolve '{}'",
                                device_url);
                            self.play_status_changed();
                        }
                        eprintln!("play: aborting because peer_ip is unresolvable \
                                   for device_url '{}'", device_url);
                        return;
                    }
                };
                // Cache the URL for future taps — both pre-staged and
                // user-initiated resolutions populate this so the next
                // tap of the same track is essentially free.
                self.prestaged_track_urls.insert(track_id, track_url.clone());

                if !is_user_request {
                    // Pre-stage only. The URL is cached and the path
                    // lookup is warm in SQLite. If aggression is at
                    // MEDIUM or HIGH, ALSO fire a speculative paused-
                    // load so the device's audio pipeline is primed.
                    // A real tap on this track then needs only a
                    // single Transport=play (no fetch).
                    //
                    // GATE: skip entirely if the accuracy tracker has
                    // recommended pause. The accuracy check returns
                    // Continue when there's not enough data yet, so
                    // this only kicks in once we have evidence.
                    let now = std::time::Instant::now();
                    match self.speculation_accuracy.recommend(now) {
                        speculation_accuracy::Recommendation::Disable { hit_ratio } => {
                            if !self.speculation_accuracy.is_disabled() {
                                eprintln!("speculative_play: pausing — accuracy {:.0}% < threshold",
                                          hit_ratio * 100.0);
                                self.speculation_accuracy.note_disabled(now);
                            }
                            return;
                        }
                        speculation_accuracy::Recommendation::ReEnable => {
                            // Cooldown elapsed — clear the disable + try again.
                            eprintln!("speculative_play: resuming after cooldown");
                            self.speculation_accuracy.reset();
                        }
                        speculation_accuracy::Recommendation::Continue => {
                            if self.speculation_accuracy.is_disabled() {
                                // Still in cooldown, don't speculate.
                                return;
                            }
                        }
                    }
                    if self.aggression.shadow_enabled() && !device_url.is_empty() {
                        let decision = self.speculative
                            .check(&device_url, track_id, now);
                        match decision {
                            speculative_play::SpeculateDecision::Proceed => {
                                self.speculative.note_attempt(&device_url, now);
                                if let Some(tx) = &self.tx {
                                    let _ = tx.send(Command::SpeculativeLoadUri {
                                        url: device_url.clone(),
                                        track_url: track_url.clone(),
                                        title: title.clone(),
                                        track_id,
                                    });
                                }
                            }
                            speculative_play::SpeculateDecision::AlreadyLoaded { .. } => {
                                // No-op — already primed.
                            }
                            speculative_play::SpeculateDecision::Cooldown { .. } => {
                                // Too soon since last speculation. Skip.
                            }
                            speculative_play::SpeculateDecision::Mismatch { .. } => {
                                // A different track is currently
                                // pre-loaded. The previous speculation
                                // is being replaced without being
                                // used — that's a Miss for accuracy.
                                self.speculation_accuracy.observe(
                                    speculation_accuracy::Outcome::Mismatch);
                                // Replacing it costs a network call;
                                // only do so if we're at HIGH aggression
                                // where we tolerate more thrashing.
                                if self.aggression.level() >= 3 {
                                    self.speculative.note_attempt(&device_url, now);
                                    if let Some(tx) = &self.tx {
                                        let _ = tx.send(Command::SpeculativeLoadUri {
                                            url: device_url.clone(),
                                            track_url: track_url.clone(),
                                            title: title.clone(),
                                            track_id,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    return;
                }

                self.last_play_url = track_url.clone();
                self.last_play_status = format!("requesting play: {} — {}", artist, title);
                self.play_status_changed();

                // Now that we've committed to playing this track, record it
                // in the catalog (updates last_played_at for the home cards).
                self.send_media(media::MediaCmd::RecordPlay(track_id));

                // ── Driver routing ─────────────────────────────────────
                //
                // Mozart devices get the existing REST/DLNA dispatch
                // (Command::PlayResolvedUrl → multi-strategy POST). For
                // Bluetooth-driven devices we don't want to ask the
                // device to fetch our HTTP URL (it can't speak REST) —
                // we paplay the local file straight to the BT sink via
                // the driver layer. Noop driver: surface a clear status
                // and stop, since neither path will work.
                //
                // History recording. Every successful DISPATCH of a play
                // (regardless of driver) records the play in the
                // history table — `record_play` qt_method was never
                // called from anywhere, so play history was empty
                // forever and the Recent / Top / Never-played smart
                // cards all returned no rows. Wire the recording at
                // dispatch time rather than completion: we don't get
                // a reliable "play finished" signal from Mozart
                // devices (they may swap source mid-track), and the
                // user's intent — "I tapped this track to play it" —
                // is the right thing to log. Speculative-hit, shadow-
                // hit, and pre-staged paths already call RecordPlay;
                // this fills in the COLD path where the user taps a
                // brand-new track that wasn't pre-staged.
                let drv_kind = self.current_driver_kind.clone();
                let recorded = matches!(drv_kind.as_str(),
                    "bluetooth" | "local" | "multiroom") || (
                    !matches!(drv_kind.as_str(), "noop")
                    && (self.device_firmware_family.is_empty()
                        || self.device_supports_rest_playback)
                );
                if recorded && is_user_request {
                    self.send_media(media::MediaCmd::RecordPlay(track_id));
                }
                match drv_kind.as_str() {
                    "bluetooth" | "local" | "multiroom" => {
                        if let Some(drv) = &self.current_driver {
                            eprintln!("play_track: routing to {} driver \
                                       ({}) for track {} ({})",
                                       drv_kind, drv.label(), track_id, title);
                            // Remember which track is now playing so
                            // advance_queue_if_any can find its
                            // successor when paplay ends. Mirror to
                            // the QML-facing field so the bottom
                            // transport bar's replay button knows
                            // which track to re-spawn paplay for.
                            self.last_played_track_id = track_id;
                            self.last_played_track_id_qml = track_id as i32;
                            self.play_status_changed();
                            // Populate the now-playing bar for BT/local
                            // drivers. Mozart devices fire NowPlaying via
                            // the REST notification path; BT/local plays
                            // never do, so the playbar stayed blank.
                            self.np_title = title.clone();
                            self.np_artist = artist.clone();
                            self.np_playing = true;
                            self.np_duration_sec = duration_secs as i32;
                            self.np_seek_pos = 0;
                            self.last_played_path = path.clone();
                            self.now_playing_changed();
                            // Activity beat for the idle-cleanup timer.
                            self.bt_respect.note_activity();
                            if let Some(tx) = &self.tx {
                                let _ = tx.send(Command::DriverOp {
                                    driver: drv.clone(),
                                    op: DriverOpKind::PlayLocalFile {
                                        path: path.clone(),
                                        title: title.clone(),
                                        start_offset_sec: 0,
                                    },
                                });
                            }
                        } else {
                            self.last_play_status =
                                format!("{} driver expected but not bound",
                                        drv_kind);
                            self.play_status_changed();
                        }
                    }
                    "noop" => {
                        // The driver couldn't bind to anything that can
                        // actually play audio. Surface the reason loudly
                        // so the user understands what to do — typically
                        // "pair a Bluetooth speaker on the Connect page".
                        self.last_play_status = format!(
                            "Cannot play: {}",
                            self.current_driver_label
                                .strip_prefix("Noop (")
                                .and_then(|s| s.strip_suffix(")"))
                                .unwrap_or(&self.current_driver_label)
                        );
                        self.play_status_changed();
                        eprintln!("play_track: skipped — noop driver ({})",
                                  self.current_driver_label);
                    }
                    _ => {
                        // Default / "mozart" / unset → existing REST flow.
                        //
                        // Skip the dispatch entirely when firmware
                        // classification has already determined this
                        // device can't accept URI playback over REST
                        // (Beosound Stage/Level/Balance/Emerge running
                        // current Mozart firmware: family =
                        // `mozart_legacy`). The previous behavior fired
                        // PlayResolvedUrl anyway and got 9 simultaneous
                        // endpoint failures per track, flooding the
                        // log and wasting ~1s on every queue advance.
                        // Now we surface a one-liner status and stop.
                        if !self.device_firmware_family.is_empty()
                            && !self.device_supports_rest_playback {
                            eprintln!("play_track: skipped REST dispatch — family={} \
                                       (no URI playback support; bind a BT speaker)",
                                       self.device_firmware_family);
                            self.last_play_status = format!(
                                "{} firmware doesn't accept URI playback. \
                                 Bind a Bluetooth speaker on Connect to route audio.",
                                self.device_firmware_label,
                            );
                            self.play_status_changed();
                        } else if let Some(tx) = &self.tx {
                            // Dispatch the multi-endpoint POST to the
                            // worker thread (it owns the tokio runtime).
                            // Result comes back as PlayResult.
                            let _ = tx.send(Command::PlayResolvedUrl {
                                url: device_url,
                                track_url,
                                title: title.clone(),
                            });
                        }
                    }
                }
            }
            media::MediaResult::AnticipatorStateLoaded(json) => {
                if !json.is_empty() {
                    let loaded = self.anticipator.load_json(&json);
                    eprintln!("anticipator: restored {} learned transitions", loaded);
                    self.refresh_prediction_json();
                }
            }
            media::MediaResult::AppKvLoaded { key, value } => {
                match key.as_str() {
                    "aggression_level" => {
                        if !value.is_empty() {
                            if let Ok(lvl) = value.parse::<i32>() {
                                // Restore directly without triggering a save loop —
                                // set the fields manually rather than calling
                                // self.set_aggression which would persist again.
                                let new_policy = aggression::AggressionPolicy::from_level(lvl);
                                self.aggression = new_policy;
                                self.aggression_level = new_policy.level() as i32;
                                self.aggression_label = new_policy.label().to_string();
                                self.aggression_changed();
                                eprintln!("aggression: restored {}", new_policy.label());
                            }
                        }
                    }
                    "bt_autoconnect_devices" => {
                        if !value.is_empty() {
                            let now = std::time::Instant::now();
                            let loaded = self.bt_autoconnect.load_json(&value, now);
                            eprintln!("bt: restored {} device(s) from KV", loaded);
                            self.refresh_bt_devices_json();
                            // Kick the BT respect snapshot now that
                            // we know which MACs are paired. Without
                            // this baseline the cleanup path would
                            // refuse to act (the safer default).
                            if !self.bt_respect.snapshot_done() {
                                self.snapshot_bt_state();
                                // Same first-poll snapshot for the
                                // system BT power state — used to
                                // decide whether to power off on
                                // shutdown if we turned it on.
                                self.spawn_bt_power_probe(true);
                            }
                        }
                        // Auto-scan paired BT devices at startup, even
                        // when KV was empty (first-launch case) or when
                        // it was populated (refresh in case the user
                        // paired new devices since last launch — e.g.
                        // a Beosound Stage they want to drive over BT).
                        // The previous behavior required the user to
                        // open Settings and tap "Discover" before any
                        // freshly-paired sink showed up.
                        self.spawn_bt_discovery_probe();
                    }
                    "bt_respect_idle_minutes" => {
                        if !value.is_empty() {
                            if let Ok(m) = value.parse::<u64>() {
                                let clamped = m.clamp(1, 60);
                                self.bt_respect.idle_threshold =
                                    std::time::Duration::from_secs(clamped * 60);
                                eprintln!("bt_respect: restored idle threshold {} min",
                                          clamped);
                                self.refresh_bt_respect();
                            }
                        }
                    }
                    "no_notify_devices" => {
                        if !value.is_empty() {
                            if let Ok(arr) = serde_json::from_str::<Vec<String>>(&value) {
                                let n = arr.len();
                                for u in arr {
                                    self.no_notify_devices.insert(u);
                                }
                                eprintln!(
                                    "notify_blocklist: restored {} URL(s) — \
                                     will skip discovery for known-bad devices",
                                    n
                                );
                            }
                        }
                        // Mark as loaded EVEN IF empty — empty is a
                        // valid first-run state and the gate should
                        // be released either way.
                        self.no_notify_loaded = true;
                        // Drain any subscriptions that were queued
                        // before this load completed. For each, only
                        // fire if the URL isn't in the just-loaded
                        // blocklist. Saves the ~7s-per-device probe
                        // for known-bad firmware on every launch.
                        if !self.pending_notify_subscriptions.is_empty() {
                            let pending: Vec<String> =
                                self.pending_notify_subscriptions
                                    .drain().collect();
                            for u in pending {
                                if self.no_notify_devices.contains(&u) {
                                    eprintln!(
                                        "device_notify: skipping {} \
                                         (in blocklist, was deferred)",
                                        u
                                    );
                                } else if let Some(tx) = &self.tx {
                                    eprintln!(
                                        "device_notify: dispatching deferred \
                                         subscribe for {}", u
                                    );
                                    let _ = tx.send(
                                        Command::SubscribeNotifications {
                                            url: u,
                                        });
                                }
                            }
                        }
                    }
                    k if k.starts_with("device_sink:") => {
                        // Per-device sink binding restore. Only adopt
                        // if it's still for the currently-active device
                        // (the user may have switched away while the
                        // load was in flight).
                        let url = &k["device_sink:".len()..];
                        if url != self.active_url || value.is_empty() {
                            // Skip; either stale load or no binding
                            // saved yet.
                        } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&value) {
                            let kind = v.get("kind")
                                .and_then(|x| x.as_str())
                                .unwrap_or("");
                            match kind {
                                "bt" => {
                                    let mac = v.get("mac").and_then(|x| x.as_str())
                                        .unwrap_or("").to_string();
                                    let name = v.get("name").and_then(|x| x.as_str())
                                        .unwrap_or("").to_string();
                                    if !mac.is_empty() {
                                        eprintln!("device_sink: restoring BT {} ({}) for {}",
                                                  mac, name, url);
                                        self.active_bt_mac = mac;
                                        self.active_bt_name = name;
                                        self.active_multiroom_json = "null".to_string();
                                        self.rebuild_driver();
                                    }
                                }
                                "multiroom" => {
                                    // Re-create the combine-sink async.
                                    let payload = v.get("payload").cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    let macs: Vec<String> = payload.get("macs")
                                        .and_then(|x| x.as_array())
                                        .map(|a| a.iter()
                                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                                            .collect())
                                        .unwrap_or_default();
                                    let names: Vec<String> = payload.get("names")
                                        .and_then(|x| x.as_array())
                                        .map(|a| a.iter()
                                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                                            .collect())
                                        .unwrap_or_default();
                                    if macs.len() >= 2 {
                                        eprintln!("device_sink: restoring multiroom ({} speakers) for {}",
                                                  macs.len(), url);
                                        let members: Vec<(String, String)> = macs.into_iter()
                                            .enumerate()
                                            .map(|(i, m)| (m,
                                                names.get(i).cloned().unwrap_or_default()))
                                            .collect();
                                        if let Some(tx) = &self.tx {
                                            let _ = tx.send(Command::CreateMultiRoom { members });
                                        }
                                    }
                                }
                                "none" => {
                                    // Explicit "play locally" — already
                                    // the default state. Nothing to do.
                                    eprintln!("device_sink: restored 'play locally' for {}", url);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => { /* unknown key — silently ignore for forward compat */ }
                }
            }
            media::MediaResult::Error(e) => {
                eprintln!("media error: {}", e);
                // If a resolve failed with "track NN not found", parse
                // the track id out and surface it appropriately:
                //   * If it was a user-initiated tap, surface in the UI.
                //   * Either way, the bad id stays in prestaged_track_ids
                //     so we don't keep firing pre-stages for it. (The
                //     normal flow only adds an id to prestaged_track_ids
                //     when issuing ResolveForPlay; on failure we keep it
                //     there as a "tried, doesn't work" marker.)
                if let Some(rest) = e.strip_prefix("resolve_for_play: track ") {
                    if let Some(id_str) = rest.split_whitespace().next() {
                        if let Ok(bad_id) = id_str.parse::<i64>() {
                            // Was the user actually waiting for this one?
                            if self.last_user_play_request == Some(bad_id) {
                                self.last_user_play_request = None;
                                self.last_play_status =
                                    "track not in library — try rescanning".to_string();
                                self.play_status_changed();
                            }
                            // Marker stays. prestaged_track_ids already
                            // contains this id (set when we issued
                            // ResolveForPlay); leaving it suppresses the
                            // retry.
                        }
                    }
                }
            }

            // ── New music-player state propagation ──
            //
            // Each of these maps a MediaResult straight into a QML
            // qt_property + signal so the QML pages can simply bind
            // to the property and re-render on changes.
            media::MediaResult::PlaylistCreated { id, name } => {
                eprintln!("playlist: created id={} name={}", id, name);
                self.last_playlist_created = id;
                self.last_playlist_changed();
            }
            media::MediaResult::PlaylistsList(json) => {
                if self.playlists_json != json {
                    self.playlists_json = json;
                    self.playlists_changed();
                }
            }
            media::MediaResult::PlaylistTracks { playlist_id, json } => {
                self.current_playlist_id = playlist_id;
                self.current_playlist_tracks_json = json;
                self.current_playlist_changed();
            }
            media::MediaResult::QueueState { device_url, json } => {
                // Always update the displayed queue — the result is
                // for whichever device the caller asked about. We
                // track the source device in queue_view_device_url so
                // QML can show "viewing queue of device X" when it's
                // not the active one.
                self.queue_json = json;
                self.queue_view_device_url = device_url;
                self.queue_changed();
            }
            media::MediaResult::QueueDevices(json) => {
                // Enrich the worker's per-device list with each device's
                // remembered volume (from settings_store, only reachable on
                // this thread) so the playbar strip can show the full
                // multi-speaker picture: host · "track" · vol · count. The
                // worker can't do this — volume lives outside the media DB.
                let enriched = self.enrich_queue_devices_with_volume(&json);
                if self.queue_devices_json != enriched {
                    self.queue_devices_json = enriched;
                    self.queue_devices_changed();
                }
            }
            media::MediaResult::SoundscapeProgress(p) => {
                // Throttle: only emit signal when progress changed by
                // ≥1% to avoid flooding QML with redundant updates.
                if (p - self.soundscape_progress).abs() >= 0.01 {
                    self.soundscape_progress = p;
                    self.soundscape_changed();
                }
            }
            media::MediaResult::SoundscapeDone(json) => {
                self.soundscape_progress = 1.0;
                self.soundscape_running = false;
                self.soundscape_result_json = json;
                self.soundscape_error = String::new();
                self.soundscape_changed();
                eprintln!("soundscape: done");
            }
            media::MediaResult::SoundscapeFailed(reason) => {
                self.soundscape_progress = 0.0;
                self.soundscape_running = false;
                self.soundscape_result_json = String::new();
                self.soundscape_error = reason.clone();
                self.soundscape_changed();
                eprintln!("soundscape: FAILED — {}", reason);
            }
            media::MediaResult::MixerProcessProgress(p) => {
                if (p - self.mixer_proc_progress).abs() >= 0.01 {
                    self.mixer_proc_progress = p;
                    self.mixer_proc_changed();
                }
            }
            media::MediaResult::MixerProcessDone(json) => {
                self.mixer_proc_progress = 1.0;
                self.mixer_proc_running = false;
                self.mixer_proc_result_json = json.clone();
                self.mixer_proc_error = String::new();
                self.mixer_proc_changed();
                eprintln!("mixer_proc: done");

                // Mix & Play continuation. If the user clicked the
                // "Mix & Play" button, the done handler also exposes
                // the output via the track server and tells the
                // active device to play it. The flag is consumed
                // here so a subsequent plain Run won't auto-play.
                if self.mix_and_play_pending {
                    self.mix_and_play_pending = false;
                    self.continue_mix_and_play(&json);
                }
            }
            media::MediaResult::MixerProcessFailed(reason) => {
                self.mixer_proc_progress = 0.0;
                self.mixer_proc_running = false;
                self.mixer_proc_result_json = String::new();
                self.mixer_proc_error = reason.clone();
                self.mix_and_play_pending = false;     // consume the flag
                self.mixer_proc_changed();
                eprintln!("mixer_proc: FAILED — {}", reason);
            }
            media::MediaResult::SpectrumDone(json) => {
                self.spectrum_running = false;
                self.spectrum_json = json;
                self.spectrum_error = String::new();
                self.spectrum_changed();
                eprintln!("spectrum: done");
            }
            media::MediaResult::SpectrumFailed(reason) => {
                self.spectrum_running = false;
                self.spectrum_json = String::new();
                self.spectrum_error = reason.clone();
                self.spectrum_changed();
                eprintln!("spectrum: FAILED — {}", reason);
            }
            media::MediaResult::TagsList(json) => {
                if self.tags_json != json {
                    self.tags_json = json;
                    self.tags_changed();
                }
            }
            media::MediaResult::TrackTags { track_id, json } => {
                self.current_track_tags_id = track_id;
                self.current_track_tags_json = json;
                self.current_track_tags_changed();
            }
            media::MediaResult::HistoryList(json) => {
                if self.history_json != json {
                    self.history_json = json;
                    self.history_changed();
                }
            }
            media::MediaResult::HistoryStats(json) => {
                if self.history_stats_json != json {
                    self.history_stats_json = json;
                    self.history_stats_changed();
                }
            }
            media::MediaResult::HistorySummary(json) => {
                if self.history_summary_json != json {
                    self.history_summary_json = json;
                    self.history_summary_changed();
                }
            }
            media::MediaResult::ArtistsList { context, json } => {
                // Two contexts share one property; the QML side picks
                // which to render via a separate property.
                self.artists_context = context;
                self.artists_json = json;
                self.artists_changed();
            }
            media::MediaResult::AlbumsList { context, json } => {
                self.albums_context = context;
                self.albums_json = json;
                self.albums_changed();
            }
        }
    }
}

/// Cheap structural validity check for an http(s) URL. Catches the
/// common failure modes that produce reqwest "invalid port number"
/// builder errors: empty scheme, missing host, non-numeric port, stray
/// colons. Does NOT validate that the host resolves or that the device
/// is reachable — just that reqwest can parse it.
fn is_well_formed_http_url(url: &str) -> bool {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return false;
    }
    let after_scheme = match u.split("://").nth(1) {
        Some(s) => s,
        None => return false,
    };
    // Reject IPv6 link-local URLs (fe80:...). reqwest can't address
    // them without a scope id, and the B&O firmware doesn't expose
    // its REST API on link-local anyway. mDNS sometimes surfaces
    // these alongside the routable address; this guard ensures the
    // priority/failover system picks the IPv4 (or non-link-local)
    // version every time.
    if after_scheme.starts_with("fe80:") || after_scheme.starts_with("[fe80:") {
        return false;
    }
    let host_part = after_scheme.split('/').next().unwrap_or("");
    if host_part.is_empty() || host_part == ":" || host_part.starts_with(':') {
        return false;
    }
    // Bracketed IPv6 host: `[2001:db8::1]` or `[2001:db8::1]:8080`.
    // The naive rsplit_once(':') below would find a colon inside the
    // IPv6 address and mis-parse it as the port separator. Detect
    // the bracketed form first and only treat content AFTER the
    // closing `]` as a potential port section.
    if host_part.starts_with('[') {
        let close = match host_part.find(']') {
            Some(i) => i,
            None => return false,   // unclosed bracket
        };
        let rest = &host_part[close + 1..];
        if rest.is_empty() { return true; }     // `[ipv6]` with no port
        let port = match rest.strip_prefix(':') {
            Some(p) => p,
            None => return false,               // `[ipv6]xyz` malformed
        };
        if port.is_empty() { return false; }
        return port.chars().all(|c| c.is_ascii_digit());
    }
    if let Some((host, port)) = host_part.rsplit_once(':') {
        if host.is_empty() { return false; }
        if !port.is_empty() && !port.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// Normalize a device name for fuzzy matching across discovery
/// channels. Lowercases, replaces punctuation with spaces, strips
/// a trailing serial-like token, and collapses whitespace runs. The
/// result is a "shape" string that lets us match e.g.
/// "Beosound Emerge" (BT advertised name) against
/// "Beosound-Emerge-36184899" (network mDNS name with serial suffix).
fn normalize_device_name(name: &str) -> String {
    let mut s: String = name.chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
        .collect();
    // Strip trailing token if it looks like a serial.
    // Heuristic: token is >=4 chars AND >50% digits.
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() > 1 {
        let last = tokens.last().unwrap();
        if last.len() >= 4 {
            let digit_share = last.chars().filter(|c| c.is_ascii_digit()).count()
                              as f32 / last.len() as f32;
            if digit_share > 0.5 {
                let kept: Vec<&str> = tokens[..tokens.len()-1].to_vec();
                s = kept.join(" ");
            }
        }
    }
    // Collapse whitespace runs.
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pull the host portion of a `http://host[:port]/...` URL, parse as IP.
/// Returns None for hostnames (we don't resolve DNS here — devices are
/// expected to be on the local LAN with stable IPs).
/// Extract the IP address from a device URL. Accepts both literal
/// IPv4/IPv6 hosts (`http://192.168.2.12`) and DNS hostnames
/// (`http://beosound-stage.local`) — the latter resolved via the
/// system resolver. Returns None if the URL has no parseable host
/// or DNS resolution fails.
///
/// Why hostname resolution: mDNS-discovered devices show up as
/// `<name>.local` URLs. Without resolving them here, the track
/// server's `url_for(peer_ip, ...)` is called with None and the
/// caller's fallback sends the device a `http://127.0.0.1:8473/...`
/// URL it can't reach — silent playback failure with no clear
/// diagnostic. The DNS lookup is one syscall (cached after the
/// first hit) and well worth the cost.
/// Run `pactl list short sinks` and return the set of BT MAC
/// addresses (underscore-uppercase form, e.g. "00_09_A7_77_CD_55")
/// that have a live bluez sink right now.
///
/// We extract the MAC from the sink name. PulseAudio names them
/// `bluez_sink.<MAC>.a2dp_sink`; PipeWire names them
/// `bluez_output.<MAC>.1`. Either way the underscore-separated MAC
/// is the part after the first dot.
///
/// Why this matters: the BT auto-bind logic was picking speakers
/// whose `last_success` was Some(...) — a session-historical
/// signal. After a BT drop or a stale registry, that pointed at
/// devices with no live sink. paplay would then write to a name
/// that didn't exist, paplay would still "complete cleanly", and
/// ffmpeg would exit 224 from the broken pipe — no sound, no
/// obvious error. Querying pactl gives us live ground truth.
///
/// Empty Vec on any error (pactl missing, audio server down, etc.)
/// — caller treats that as "no usable BT routes right now".
fn collect_live_bluez_macs() -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let stdout = match std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return out,
    };
    let text = String::from_utf8_lossy(&stdout);
    for line in text.lines() {
        // Columns are tab-separated: id, name, driver, format, state
        let name = match line.split('\t').nth(1) {
            Some(n) => n.trim(),
            None => continue,
        };
        // Extract MAC: bluez_sink.00_09_A7_77_CD_55.a2dp_sink
        //                          ^^^^^^^^^^^^^^^^^
        //              bluez_output.00_09_A7_77_CD_55.1
        let mac_part = if let Some(rest) = name.strip_prefix("bluez_sink.") {
            rest.split('.').next()
        } else if let Some(rest) = name.strip_prefix("bluez_output.") {
            rest.split('.').next()
        } else {
            None
        };
        if let Some(mac) = mac_part {
            // Normalize to underscore-uppercase to match what the
            // caller produces from the registry MAC string.
            out.insert(mac.to_uppercase());
        }
    }
    out
}

fn url_to_ip(url: &str) -> Option<std::net::IpAddr> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    // IPv6 hosts are bracketed in URLs: `[::1]:8080`. The brackets
    // are required because IPv6 addresses contain `:` which would
    // otherwise collide with the port separator. Handle that case
    // first; the non-bracketed code path can split on `:` safely.
    let host = if let Some(stripped) = after_scheme.strip_prefix('[') {
        // `[host]:port/path` — host runs up to the closing `]`.
        match stripped.find(']') {
            Some(end) => &stripped[..end],
            None => return None,  // malformed: opening bracket but no close
        }
    } else {
        // Non-IPv6: host ends at the first `:` (port) or `/` (path).
        after_scheme.split(|c: char| c == ':' || c == '/').next()?
    };
    if host.is_empty() { return None; }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Some(ip);
    }
    // Fall back to DNS resolution. Use `to_socket_addrs` with a
    // dummy port — the resolver doesn't actually connect.
    use std::net::ToSocketAddrs;
    if let Ok(mut addrs) = (host, 0u16).to_socket_addrs() {
        // Prefer IPv4 (B&O devices advertise both but their REST
        // API is more reliably reached over v4 on typical home
        // networks). Fall through to IPv6 if no v4.
        let mut v6_fallback = None;
        for addr in addrs.by_ref() {
            match addr.ip() {
                ip @ std::net::IpAddr::V4(_) => return Some(ip),
                ip @ std::net::IpAddr::V6(_) => {
                    if v6_fallback.is_none() { v6_fallback = Some(ip); }
                }
            }
        }
        return v6_fallback;
    }
    None
}

/// Browse the LAN for Bang & Olufsen devices via mDNS/DNS-SD and feed any
/// found devices back through the result channel. Runs on its own thread
/// (mdns-sd uses a flume channel internally; we just block-recv on it).
///
/// B&O devices advertise under a few service types depending on generation.
/// We browse the common ones. For each resolved instance we build the base
/// URL from its address + port (the REST API lives at http://<ip>).
fn spawn_mdns_discovery(result_tx: Sender<ResultMessage>) {
    thread::spawn(move || {
        let daemon = match mdns_sd::ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => { eprintln!("mDNS: couldn't start daemon: {}", e); return; }
        };

        // Service types B&O products are known to advertise. Browsing
        // several covers ASE (older) and Mozart (newer) generations.
        let service_types = [
            "_beoremote._tcp.local.",
            "_bangolufsen._tcp.local.",
            "_beozone._tcp.local.",
            "_http._tcp.local.",        // fallback; filtered by name below
        ];

        let mut receivers = Vec::new();
        for st in service_types {
            match daemon.browse(st) {
                Ok(rx) => receivers.push(rx),
                Err(e) => eprintln!("mDNS: browse {} failed: {}", st, e),
            }
        }
        if receivers.is_empty() { return; }

        eprintln!("mDNS: browsing for Bang & Olufsen devices…");

        loop {
            for rx in &receivers {
                // Non-blocking drain of each browse channel.
                while let Ok(event) = rx.try_recv() {
                    if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                        let fullname = info.get_fullname().to_string();
                        let friendly = fullname.split('.').next().unwrap_or(&fullname).to_string();

                        // For the generic _http fallback, only accept names
                        // that look like B&O gear to avoid listing printers etc.
                        let looks_bo = {
                            let l = friendly.to_lowercase();
                            l.contains("beo") || l.contains("bang") || l.contains("b&o")
                        };
                        let from_http = fullname.contains("_http._tcp");
                        if from_http && !looks_bo { continue; }

                        if let Some(addr) = info.get_addresses().iter().next() {
                            // REST API is plain http on the device's address.
                            // Most B&O REST lives on port 8080 (ASE) but the
                            // app's existing calls use bare host, so default
                            // to no explicit port unless the SRV port looks
                            // like a REST port.
                            let url = format!("http://{}", addr);
                            let _ = result_tx.send(ResultMessage::DeviceDiscovered {
                                name: friendly.clone(),
                                url,
                            });
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}


/// Forward a parsed DeviceNotification into the regular ResultMessage
/// channel as if it had come from a polled GetVolume / GetNowPlaying.
/// Lets the rest of the codebase ignore where the state change came
/// from — push vs poll is opaque to consumers.
fn forward_notif(n: device_notify::DeviceNotification, tx: &Sender<ResultMessage>) {
    use device_notify::DeviceNotification as DN;
    match n {
        DN::Volume(v)  => { let _ = tx.send(ResultMessage::Volume(v.clamp(0, 100))); }
        DN::Mute(b)    => { let _ = tx.send(ResultMessage::MuteState(b)); }
        DN::Standby(b) => { let _ = tx.send(ResultMessage::Standby(b)); }
        DN::NowPlaying { title, artist, source } => {
            // Best-effort mapping into our existing NowPlaying shape;
            // `playing` is unknown from a single notification so we
            // default to true (the most useful interpretation when a
            // track changes).
            let _ = tx.send(ResultMessage::NowPlaying {
                title, artist, source, playing: true,
            });
        }
        DN::Source(_) | DN::Transport(_) | DN::Unknown(_) => {
            // Could route to dedicated channels in the future; for
            // now these are advisory and we just log via the caller.
        }
    }
}

/// Background worker main loop — runs on its own thread and owns a Tokio runtime.
fn spawn_background_worker(cmd_rx: Receiver<Command>, result_tx: Sender<ResultMessage>) {
    thread::spawn(move || {
        // Request real-time scheduling priority for the worker thread.
        // This is a best-effort hint: on systems where the user lacks
        // CAP_SYS_NICE (i.e. not in the `audio` group), the call returns
        // EPERM and the worker keeps running with default scheduling.
        // The functional difference is invisible to the user — what RT
        // scheduling buys is *reduced jitter* under system load. Without
        // it, a busy CPU can preempt the worker for 10-50 ms at a time,
        // and gestures feel sluggish. With it, the worker generally runs
        // within ~1 ms of when it's ready.
        let _ = rt_sched::request_rt_priority(rt_sched::WORKER_RT_PRIORITY);

        // Build a multithreaded runtime for async tasks
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime build failed");

        // Create a reqwest client tuned for low-latency LAN communication
        // with a small number of devices reused indefinitely.
        //
        //   * `pool_max_idle_per_host` keeps multiple connections warm
        //     per device — important when we issue concurrent commands
        //     (e.g. GetVolume + Transport from different code paths).
        //   * `pool_idle_timeout` long enough to outlast typical user
        //     pauses between interactions; without this, connections
        //     age out after ~30s and we pay TCP+TLS setup again.
        //   * `tcp_nodelay` disables Nagle on outgoing connections so
        //     the device sees our POST headers immediately rather than
        //     coalesced with the body bytes.
        //   * `connect_timeout` short so dead devices fail fast — the
        //     circuit breaker further protects against repeated dead
        //     attempts.
        //   * `http2_adaptive_window` enables HTTP/2 ALPN; some Mozart
        //     firmware speaks h2 over plain HTTP (h2c) and can serve
        //     multiple commands over the same connection in parallel.
        let client = Client::builder()
            .danger_accept_invalid_certs(false)
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(std::time::Duration::from_secs(120))
            .tcp_nodelay(true)
            .connect_timeout(std::time::Duration::from_secs(3))
            .http2_adaptive_window(true)
            .build()
            .expect("reqwest client build failed");

        // Per-URL circuit breaker. After N consecutive failures we
        // fast-fail subsequent commands to that URL with a short
        // diagnostic instead of waiting for the network timeout. This
        // is what stops the "queued gestures to a now-dead device"
        // spam from the auto-failover loop.
        let mut breaker = circuit_breaker::CircuitBreaker::new();

        // Helper: returns true if we should skip this command because
        // the URL's breaker is open. Updates state to HalfOpen when
        // backoff has elapsed (next call falls through to Allow).
        // Returns true when caller should NOT send (i.e. fast-fail).
        let breaker_check = |breaker: &mut circuit_breaker::CircuitBreaker, url: &str| {
            let now = std::time::Instant::now();
            match breaker.check(url, now) {
                circuit_breaker::BreakerDecision::Allow => false,
                circuit_breaker::BreakerDecision::Blocked { .. } => true,
            }
        };

        // Blocking loop: receive commands and handle each via rt.block_on.
        // SetVolume gets coalescing: when one arrives we peek the channel
        // for any more SetVolume calls targeting the same URL and keep
        // only the latest value. This is the single biggest latency win
        // for an active drag: 5-8 commands per second from a held finger
        // collapse to one or two trips per second, and the device sees
        // the user's actual destination value sooner.
        let mut buffered_cmd: Option<Command> = None;
        loop {
            let cmd = match buffered_cmd.take() {
                Some(c) => c,
                None => match cmd_rx.recv() {
                    Ok(c) => c,
                    Err(_) => break,   // channel closed
                },
            };
            // Coalesce step: if this is a SetVolume, drain the channel
            // (non-blocking) and keep only the LATEST SetVolume for the
            // same URL. Other commands that interleave get processed
            // in order — we DON'T reorder across command types, just
            // collapse same-URL SetVolumes.
            let cmd = if let Command::SetVolume { ref url, .. } = cmd {
                let mut latest = cmd.clone();
                let target_url = url.clone();
                let mut coalesced = 0u32;
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Command::SetVolume { url: u, value: v }) if u == target_url => {
                            latest = Command::SetVolume { url: u, value: v };
                            coalesced += 1;
                        }
                        Ok(other) => {
                            // Different command type or different URL — save it
                            // for the next iteration. Stop coalescing here so
                            // we don't reorder commands.
                            buffered_cmd = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if coalesced > 0 {
                    eprintln!("worker: coalesced {} SetVolume commands → final value",
                              coalesced);
                }
                latest
            } else {
                cmd
            };
            match cmd {
                Command::GetVolume { url } => {
                    if breaker_check(&mut breaker, &url) {
                        // Quietly drop — verbose-but-throttled diagnostic
                        // only emitted on the actual trip event below.
                        continue;
                    }
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    let url_for_breaker = url.clone();
                    let response = rt.block_on(async move {
                        let endpoint = format!("{}/api/getData?path=BeoSound%3A%2Fvolume&roles=value", url);
                        match client.get(&endpoint).send().await {
                            Ok(resp) => {
                                if let Ok(text) = resp.text().await {
                                    println!("GetVolume response: {}", text);
                                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                                        let maybe_val = json.as_array()
                                            .and_then(|arr| arr.first())
                                            .and_then(|obj| obj.get("i32_"))
                                            .and_then(|v| v.as_i64());
                                        if let Some(v) = maybe_val {
                                            let v_i32 = (v as i32).clamp(0, 100);
                                            println!("Parsed volume: {}", v_i32);
                                            let _ = result_tx.send(ResultMessage::Volume(v_i32));
                                            return Ok(());
                                        }
                                    }
                                }
                                Ok(())  // parse glitch — don't penalize the breaker
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("GetVolume failed: {}", e)));
                                Err(())
                            }
                        }
                    });
                    // Update breaker based on outcome.
                    let now = std::time::Instant::now();
                    match response {
                        Ok(()) => breaker.note_success(&url_for_breaker),
                        Err(()) => {
                            breaker.note_failure(&url_for_breaker, now);
                            if breaker.is_open(&url_for_breaker) {
                                eprintln!("circuit-breaker: {} OPEN (will fast-fail for a few seconds)",
                                          url_for_breaker);
                            }
                        }
                    }
                }

                Command::SetVolume { url, value } => {
                    if breaker_check(&mut breaker, &url) {
                        continue;
                    }
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    let url_for_breaker = url.clone();
                    let response = rt.block_on(async move {
                        // Use setData API with beoSoundVolumeData structure
                        let volume_data = serde_json::json!({
                            "type": "beoSoundVolumeData",
                            "beoSoundVolumeData": {
                                "volume": value,
                                "volumeSource": "qtapp"
                            }
                        });

                        let value_str = serde_json::to_string(&volume_data).unwrap();
                        let encoded_value = urlencoding::encode(&value_str);

                        let endpoint = format!(
                            "{}/api/setData?path=BeoSound%3A%2FsetVolume&roles=activate&value={}",
                            url, encoded_value
                        );

                        println!("SetVolume POST to: {}", endpoint);

                        match client.post(&endpoint)
                            .header("Accept", "application/json")
                            .header("X-Requested-With", "XMLHttpRequest")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status();
                                println!("SetVolume status: {}", status);
                                if status.is_success() {
                                    println!("SetVolume request successful");
                                    let _ = result_tx.send(ResultMessage::Volume(value.clamp(0, 100)));
                                    Ok(())
                                } else {
                                    if let Ok(text) = resp.text().await {
                                        println!("SetVolume error: {}", text);
                                    }
                                    // HTTP-level failure (4xx/5xx) but TCP worked;
                                    // don't open the breaker for app-layer errors.
                                    Ok(())
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("SetVolume failed: {}", e)));
                                Err(())
                            }
                        }
                    });
                    let now = std::time::Instant::now();
                    match response {
                        Ok(()) => breaker.note_success(&url_for_breaker),
                        Err(()) => {
                            breaker.note_failure(&url_for_breaker, now);
                            if breaker.is_open(&url_for_breaker) {
                                eprintln!("circuit-breaker: {} OPEN (will fast-fail for a few seconds)",
                                          url_for_breaker);
                            }
                        }
                    }
                }

                Command::GetMuteState { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!(
                            "{}/api/getData?path=BeoSound%3A%2Fmuted&roles=value", url
                        );
                        match client.get(&endpoint).send().await {
                            Ok(resp) => {
                                if let Ok(json) = resp.json::<Value>().await {
                                    // ASE returns [{"bool_":true,"type":"bool_"}]
                                    let muted = json.as_array()
                                        .and_then(|arr| arr.first())
                                        .and_then(|obj| obj.get("bool_"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    println!("GetMuteState: {}", muted);
                                    let _ = result_tx.send(ResultMessage::MuteState(muted));
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("GetMuteState failed: {}", e)));
                            }
                        }
                    });
                }

                Command::SetMuteState { url, muted } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        // Mirror the SetVolume encoding style: send the bool as a
                        // typed value the ASE setData endpoint accepts.
                        let mute_data = serde_json::json!({
                            "type": "bool_",
                            "bool_": muted
                        });
                        let value_str = serde_json::to_string(&mute_data).unwrap();
                        let encoded_value = urlencoding::encode(&value_str);

                        let endpoint = format!(
                            "{}/api/setData?path=BeoSound%3A%2Fmuted&roles=value&value={}",
                            url, encoded_value
                        );

                        println!("SetMuteState POST to: {}", endpoint);

                        match client.post(&endpoint)
                            .header("Accept", "application/json")
                            .header("X-Requested-With", "XMLHttpRequest")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status();
                                println!("SetMuteState status: {}", status);
                                if status.is_success() {
                                    let _ = result_tx.send(ResultMessage::MuteState(muted));
                                } else if let Ok(text) = resp.text().await {
                                    println!("SetMuteState error: {}", text);
                                    // Re-query so UI reflects reality if the PUT was rejected
                                    let _ = result_tx.send(ResultMessage::Error(
                                        format!("SetMuteState rejected: {}", text)));
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("SetMuteState failed: {}", e)));
                            }
                        }
                    });
                }

                Command::GetProductInfo { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoDevice", url);
                        if let Ok(resp) = client.get(&endpoint).send().await {
                            if let Ok(json) = resp.json::<Value>().await {
                                let name = json.get("beoDevice")
                                    .and_then(|d| d.get("productFriendlyName"))
                                    .and_then(|n| n.get("productFriendlyName"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown Device")
                                    .to_string();

                                let type_name = json.get("beoDevice")
                                    .and_then(|d| d.get("productId"))
                                    .and_then(|p| p.get("productType"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown Type")
                                    .to_string();

                                println!("Product: {} ({})", name, type_name);
                                let _ = result_tx.send(ResultMessage::ProductInfo { name, type_name });
                            }
                        }
                    });
                }

                Command::CheckKdeMedia => {
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        // Check PulseAudio/PipeWire for active sinks playing audio
                        let output = ProcessCommand::new("pactl")
                            .arg("list")
                            .arg("sink-inputs")
                            .output();

                        if let Ok(output) = output {
                            let status = String::from_utf8_lossy(&output.stdout);

                            // Check if there are any sink inputs (playing audio)
                            let has_audio = status.contains("Sink Input #") &&
                                          (status.contains("state: RUNNING") ||
                                           status.contains("State: RUNNING") ||
                                           status.contains("CORKED: no"));

                            println!("Audio detection: {}", if has_audio { "Audio playing" } else { "No audio" });
                            let _ = result_tx.send(ResultMessage::KdeMediaPlaying(has_audio));
                        } else {
                            // Fallback: Try checking DBus for MPRIS media players
                            let dbus_output = ProcessCommand::new("sh")
                                .arg("-c")
                                .arg("dbus-send --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ListNames 2>/dev/null | grep -c 'org.mpris.MediaPlayer2' || echo 0")
                                .output();

                            if let Ok(dbus_output) = dbus_output {
                                let count_str = String::from_utf8_lossy(&dbus_output.stdout);
                                let has_players = count_str.trim().parse::<i32>().unwrap_or(0) > 0;
                                let _ = result_tx.send(ResultMessage::KdeMediaPlaying(has_players));
                            }
                        }
                    });
                }

                Command::PlayMozartUri { url, stream_uri } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        // Mozart API: POST to PlayQueue
                        let endpoint = format!("{}/BeoZone/Zone/Stream/PlayQueue/", url);
                        let body = serde_json::json!({
                            "playQueueItem": {
                                "behaviour": "impulsive",
                                "startNowIfNotPlaying": true,
                                "provider": {
                                    "id": "dlna"
                                },
                                "image": [],
                                "track": {
                                    "uri": stream_uri,
                                    "mimeType": "audio/mpeg"
                                }
                            }
                        });

                        println!("PlayMozartUri to: {} with URI: {}", endpoint, stream_uri);

                        match client.post(&endpoint)
                            .header("Content-Type", "application/json")
                            .json(&body)
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status();
                                println!("PlayMozartUri status: {}", status);
                                if status.is_success() {
                                    println!("Mozart playback started");
                                } else if let Ok(text) = resp.text().await {
                                    println!("PlayMozartUri error: {}", text);
                                    let _ = result_tx.send(ResultMessage::Error(format!("PlayMozartUri failed: {}", text)));
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("PlayMozartUri failed: {}", e)));
                            }
                        }
                    });
                }

                Command::PlayResolvedUrl { url, track_url, title } => {
                    // Belt-and-suspenders link-local guard. play_uri has
                    // its own check, but this is the cheapest place to
                    // catch the case where the device picker still
                    // somehow has an fe80:: URL selected (e.g. an old
                    // install with stale code). Bail with a clear toast
                    // instead of letting reqwest spam 7+ "builder error:
                    // invalid port number" lines per attempt.
                    if url.contains("://fe80:") || url.contains("://[fe80:") {
                        let _ = result_tx.send(ResultMessage::PlayOutcome {
                            used_endpoint: String::new(),
                            summary: "device URL is IPv6 link-local (unreachable) — \
                                      re-select the device from the picker"
                                      .to_string(),
                            url: track_url,
                        });
                        continue;
                    }
                    // Don't bother POSTing to a known-dead device. The
                    // circuit breaker protects the rest of the system
                    // from spinning queued commands against a URL that
                    // hasn't responded in N seconds; play should honor
                    // it too. The fallback shows in the QML status as
                    // "device refused all play endpoints — check console"
                    // which is the right user-facing message.
                    if breaker_check(&mut breaker, &url) {
                        let _ = result_tx.send(ResultMessage::PlayOutcome {
                            used_endpoint: String::new(),
                            summary: "circuit breaker open — device unreachable".to_string(),
                            url: track_url,
                        });
                        continue;
                    }
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let outcome = play_uri::play_url_on_device(
                            &url, &track_url, &title, &client).await;
                        let summary = if let Some(ep) = &outcome.used_endpoint {
                            format!("OK via {}", ep)
                        } else {
                            // Build a short diagnostic from the attempts.
                            let parts: Vec<String> = outcome.attempts.iter()
                                .map(|a| format!(
                                    "{} {}",
                                    a.status.map(|s| s.to_string())
                                        .unwrap_or_else(|| "ERR".to_string()),
                                    a.endpoint.rsplit('/').next().unwrap_or("?")
                                ))
                                .collect();
                            format!("no endpoint accepted ({})", parts.join("; "))
                        };
                        let _ = result_tx.send(ResultMessage::PlayOutcome {
                            used_endpoint: outcome.used_endpoint.unwrap_or_default(),
                            summary,
                            url: outcome.url,
                        });
                    });
                }

                Command::SpeculativeLoadUri { url, track_url, title, track_id } => {
                    // Skip dead devices fast — no point speculating on
                    // a device we've already established isn't responding.
                    // Saves the wasted POST and keeps the device-side
                    // pipeline clean for the eventual reconnect.
                    if breaker_check(&mut breaker, &url) {
                        continue;
                    }
                    // Best-effort speculative pre-load. Failure here is
                    // expected on older firmware; we silently fall back
                    // to the regular play_uri flow on actual user tap.
                    // Successes are surfaced via ResultMessage::PlayOutcome
                    // with a `summary` prefix that tells the QtState
                    // handler to call `speculative.mark_loaded`.
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        match play_uri::speculative_load_paused(
                            &url, &track_url, &title, &client).await
                        {
                            Ok(endpoint) => {
                                eprintln!("speculative_play: OK on {} via {} (track_id={})",
                                          url, endpoint, track_id);
                                // The "SPEC_LOADED:" prefix lets the QtState
                                // handler distinguish a speculation success
                                // from a regular play. We embed track_id +
                                // device_url in the summary so the handler
                                // can update the SpeculativeStager.
                                let _ = result_tx.send(ResultMessage::PlayOutcome {
                                    used_endpoint: endpoint,
                                    summary: format!("SPEC_LOADED:{}:{}", track_id, url),
                                    url: track_url,
                                });
                            }
                            Err(diag) => {
                                eprintln!("speculative_play: {} (track_id={}, device={})",
                                          diag, track_id, url);
                                // No PlayOutcome emitted on failure — we
                                // don't want to corrupt the user-visible
                                // last_play_status with a speculation
                                // failure. The fallback path runs on tap.
                            }
                        }
                    });
                }

                Command::SubscribeNotifications { url } => {
                    if breaker_check(&mut breaker, &url) {
                        continue;
                    }
                    // Spawn a persistent long-poll task. The task tries
                    // each candidate endpoint in turn; first one that
                    // returns 200 becomes the steady-state subscription.
                    // We don't await the task — it runs in the
                    // background, emitting ResultMessage variants
                    // whenever the device pushes state.
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    let url_for_task = url.clone();
                    rt.spawn(async move {
                        let mut since: u64 = 0;
                        let mut active_endpoint: Option<String> = None;
                        let mut backoff_ms: u64 = 500;
                        // After this many full discovery cycles where
                        // NONE of the candidate endpoints responded
                        // with a 2xx, we conclude this firmware just
                        // doesn't support push notifications and give
                        // up. The regular GetVolume polling loop keeps
                        // doing its job. 5 cycles ≈ 50 seconds of probing.
                        const MAX_DISCOVERY_CYCLES: u32 = 5;
                        let mut unsupported_attempts: u32 = 0;
                        loop {
                            // Discovery step: probe candidate endpoints
                            // until one returns 200. Cache the winner.
                            if active_endpoint.is_none() {
                                for ep in device_notify::CANDIDATE_ENDPOINTS {
                                    let probe_url = device_notify::long_poll_url(
                                        &url_for_task, ep, 0);
                                    match client.get(&probe_url)
                                        .timeout(std::time::Duration::from_secs(2))
                                        .send().await
                                    {
                                        Ok(r) if r.status().is_success() => {
                                            eprintln!("device_notify: using endpoint {} for {}",
                                                ep, url_for_task);
                                            active_endpoint = Some(ep.to_string());
                                            // Process the discovery response too
                                            if let Ok(text) = r.text().await {
                                                if let Ok(j) = serde_json::from_str::<serde_json::Value>(&text) {
                                                    for n in device_notify::parse(&j) {
                                                        forward_notif(n, &result_tx);
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                                if active_endpoint.is_none() {
                                    // No endpoint worked this cycle.
                                    // Increment the cumulative counter
                                    // and exit if we've burned through
                                    // our discovery budget. The regular
                                    // polling loop keeps the UI in sync.
                                    unsupported_attempts += 1;
                                    if unsupported_attempts >= MAX_DISCOVERY_CYCLES {
                                        eprintln!("device_notify: giving up on {} after {} \
                                                  unsuccessful discovery cycles — \
                                                  firmware does not support push notifications. \
                                                  Falling back to GetVolume polling.",
                                                  url_for_task, unsupported_attempts);
                                        // Tell QtState so it can
                                        // persist the result and skip
                                        // the subscription on future
                                        // launches. Saves ~7 seconds
                                        // of startup probing per
                                        // legacy device.
                                        let _ = result_tx.send(
                                            ResultMessage::NotifyUnsupported {
                                                url: url_for_task.clone(),
                                            });
                                        return;
                                    }
                                    eprintln!("device_notify: no endpoint matched for {}, \
                                              backing off {}ms (cycle {}/{})",
                                              url_for_task, backoff_ms,
                                              unsupported_attempts, MAX_DISCOVERY_CYCLES);
                                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                                    backoff_ms = (backoff_ms * 2).min(60_000);
                                    continue;
                                }
                                backoff_ms = 500;   // reset on success
                                unsupported_attempts = 0;
                            }

                            // Steady state: long-poll the chosen endpoint.
                            let ep = active_endpoint.as_ref().unwrap();
                            let poll_url = device_notify::long_poll_url(
                                &url_for_task, ep, since);
                            match client.get(&poll_url)
                                .timeout(std::time::Duration::from_secs(35))
                                .send().await
                            {
                                Ok(r) if r.status().is_success() => {
                                    if let Ok(text) = r.text().await {
                                        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&text) {
                                            for n in device_notify::parse(&j) {
                                                forward_notif(n, &result_tx);
                                            }
                                            // Pull notification sequence id
                                            // if the firmware exposes one
                                            // (BeoNotify does).
                                            if let Some(id) = j.pointer("/notification/0/id")
                                                .and_then(|v| v.as_u64())
                                            {
                                                since = id;
                                            }
                                        }
                                    }
                                    backoff_ms = 500;
                                }
                                Ok(_) | Err(_) => {
                                    // Connection broke or server returned 5xx.
                                    // Back off and re-probe.
                                    active_endpoint = None;
                                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                                    backoff_ms = (backoff_ms * 2).min(60_000);
                                }
                            }
                        }
                    });
                    eprintln!("device_notify: subscription task spawned for {}", url);
                }
                Command::ProbeActiveSource { url } => {
                    // Tolerant probe: tries multiple candidate paths.
                    // Logged inline so the user can see which path
                    // returned data (or that none did).
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    let url_for_log = url.clone();
                    rt.spawn(async move {
                        let state = mozart_source::probe(&url, &client).await;
                        match &state {
                            Some(s) => eprintln!(
                                "mozart_source: {} → id={:?}, label={:?}, \
                                 family={:?}, via {}",
                                url_for_log, s.id, s.label, s.family, s.via_path),
                            None => eprintln!(
                                "mozart_source: {} → no candidate path returned \
                                 a parseable source", url_for_log),
                        }
                        let (id, label, family, can_play, block_reason) = match state {
                            Some(s) => {
                                let fam_str = match s.family {
                                    mozart_source::SourceFamily::NetworkDlna     => "network_dlna",
                                    mozart_source::SourceFamily::Spotify         => "spotify",
                                    mozart_source::SourceFamily::AirPlay         => "airplay",
                                    mozart_source::SourceFamily::Bluetooth       => "bluetooth",
                                    mozart_source::SourceFamily::LineIn          => "linein",
                                    mozart_source::SourceFamily::Tv              => "tv",
                                    mozart_source::SourceFamily::InternetRadio   => "internet_radio",
                                    mozart_source::SourceFamily::OtherStreaming  => "other_streaming",
                                    mozart_source::SourceFamily::Unknown         => "unknown",
                                };
                                (
                                    s.id.unwrap_or_default(),
                                    s.label.unwrap_or_default(),
                                    fam_str.to_string(),
                                    s.family.accepts_uri_playback(),
                                    s.family.cant_play_reason().to_string(),
                                )
                            }
                            None => (
                                String::new(),
                                String::new(),
                                "unknown".to_string(),
                                false,
                                "Could not detect active source. Try refreshing \
                                 or switching device.".to_string(),
                            ),
                        };
                        let _ = result_tx.send(ResultMessage::ActiveSourceProbed {
                            url: url_for_log, id, label, family,
                            can_play, block_reason,
                        });
                    });
                }
                Command::TryActivateNetwork { url } => {
                    // Attempt programmatic source switching to
                    // Network/DLNA. Runs the full strategy ladder
                    // from mozart_source_switch and reports the
                    // outcome back to the UI. After a successful
                    // switch we trigger a fresh active-source probe
                    // so the warning banner clears immediately.
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    let url_for_log = url.clone();
                    rt.spawn(async move {
                        eprintln!("try_activate_network: starting for {}", url);
                        let outcome = mozart_source_switch::try_activate_network(
                            &url, &client).await;
                        eprintln!("try_activate_network: {} → success={}, \
                                   tried {} endpoint(s), used={:?}",
                                   url_for_log, outcome.success,
                                   outcome.attempts.len(), outcome.used_endpoint);
                        let json = serde_json::to_string(&outcome).unwrap_or_default();
                        let success = outcome.success;
                        let _ = result_tx.send(ResultMessage::NetworkSourceSwitched {
                            url: url_for_log.clone(),
                            success,
                            json,
                        });
                        // On success: probe the active source again
                        // so the cached state + warning banner update
                        // without the user having to tap "Check".
                        if success {
                            let state = mozart_source::probe(
                                &url_for_log, &client).await;
                            let (id, label, family, can_play, block_reason) = match state {
                                Some(s) => {
                                    let fam_str = match s.family {
                                        mozart_source::SourceFamily::NetworkDlna     => "network_dlna",
                                        mozart_source::SourceFamily::Spotify         => "spotify",
                                        mozart_source::SourceFamily::AirPlay         => "airplay",
                                        mozart_source::SourceFamily::Bluetooth       => "bluetooth",
                                        mozart_source::SourceFamily::LineIn          => "linein",
                                        mozart_source::SourceFamily::Tv              => "tv",
                                        mozart_source::SourceFamily::InternetRadio   => "internet_radio",
                                        mozart_source::SourceFamily::OtherStreaming  => "other_streaming",
                                        mozart_source::SourceFamily::Unknown         => "unknown",
                                    };
                                    (s.id.unwrap_or_default(),
                                     s.label.unwrap_or_default(),
                                     fam_str.to_string(),
                                     s.family.accepts_uri_playback(),
                                     s.family.cant_play_reason().to_string())
                                }
                                None => (
                                    "Network".to_string(),
                                    "Network".to_string(),
                                    "network_dlna".to_string(),
                                    true,
                                    String::new(),
                                ),
                            };
                            let _ = result_tx.send(ResultMessage::ActiveSourceProbed {
                                url: url_for_log, id, label, family,
                                can_play, block_reason,
                            });
                        }
                    });
                }
                Command::ProbeFirmware { url } => {
                    // Cheap one-shot HTTP probe to classify the
                    // device's firmware family. Used both on device-
                    // add (so we know what kind of device we're
                    // talking to from the first interaction) and
                    // whenever the user manually re-probes from the
                    // UI. The result drives which fallback options
                    // the UI shows: Mozart gets "Try Network",
                    // everything else gets "Try Bluetooth".
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    let url_for_log = url.clone();
                    rt.spawn(async move {
                        let probe = firmware_family::probe(&url, &client).await;
                        eprintln!("firmware_probe: {} → family={}, status={:?}",
                                  url_for_log, probe.family, probe.volume_status);
                        for reason in &probe.reasoning {
                            eprintln!("  └─ {}", reason);
                        }
                        let json = serde_json::to_string(&probe).unwrap_or_default();
                        let family_tag = probe.family.clone();
                        let _ = result_tx.send(ResultMessage::FirmwareProbed {
                            url: url_for_log,
                            family: family_tag,
                            json,
                        });
                    });
                }
                Command::DriverOp { driver, op } => {
                    // Run the driver operation in the worker's tokio
                    // runtime. Each operation is its own task so a
                    // slow stop() doesn't queue behind a play().
                    // Results stream back as ResultMessage::DriverResult.
                    let result_tx = result_tx.clone();
                    rt.spawn(async move {
                        let r = match op {
                            DriverOpKind::PlayLocalFile { path, title, start_offset_sec } => {
                                // Build the end-of-playback callback
                                // that sends TrackEnded when paplay
                                // exits naturally. We only fire on
                                // CompletedNaturally — Stop/error
                                // ends shouldn't trigger auto-advance.
                                let tx = result_tx.clone();
                                let on_end: Option<Box<dyn FnOnce(device_driver::EndReason) + Send + 'static>> =
                                    Some(Box::new(move |reason| {
                                        if reason == device_driver::EndReason::CompletedNaturally {
                                            let _ = tx.send(ResultMessage::TrackEnded);
                                        }
                                    }));
                                driver.play_local_file(
                                    std::path::Path::new(&path), &title, start_offset_sec, on_end
                                ).await
                            }
                            DriverOpKind::Stop => driver.stop().await,
                            DriverOpKind::SetVolumePct(p) => {
                                driver.set_volume_percent(p).await
                            }
                        };
                        let msg = match r {
                            Ok(_) => String::new(),
                            Err(e) => format!("{}", e),
                        };
                        if !msg.is_empty() {
                            eprintln!("driver: op error: {}", msg);
                        }
                        let _ = result_tx.send(ResultMessage::DriverResult(msg));
                    });
                }
                Command::CreateMultiRoom { members } => {
                    let result_tx = result_tx.clone();
                    rt.spawn(async move {
                        let r = device_driver::MultiRoomDriver::create(members).await;
                        let (driver, error) = match r {
                            Ok(d) => (Some(device_driver::AnyDriver::MultiRoom(d)),
                                      String::new()),
                            Err(e) => (None, format!("{}", e)),
                        };
                        let _ = result_tx.send(ResultMessage::MultiRoomCreated {
                            driver, error,
                        });
                    });
                }
                Command::TeardownMultiRoom { driver } => {
                    let result_tx = result_tx.clone();
                    rt.spawn(async move {
                        let r = driver.teardown().await;
                        let msg = r.err().map(|e| format!("{}", e)).unwrap_or_default();
                        let _ = result_tx.send(ResultMessage::DriverResult(msg));
                    });
                }
                Command::DriverGetVolume { driver } => {
                    let result_tx = result_tx.clone();
                    rt.spawn(async move {
                        let pct = driver.get_volume_percent().await.ok();
                        let _ = result_tx.send(ResultMessage::DriverVolume { pct });
                    });
                }
                Command::ProbeDeviceCapabilities { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        // Best-effort GETs to common device-state endpoints.
                        // Each call has a short timeout so an unreachable
                        // device doesn't block the whole probe.
                        let mut reachable = false;
                        let mut on_battery = None;
                        let mut battery_pct = None;
                        let mut charging = None;
                        let mut on_eth = None;
                        let mut on_wifi = None;
                        let mut bt = None;

                        // /BeoZone/Zone/Battery — modern firmware
                        let battery_url = format!(
                            "{}/BeoZone/Zone/Battery",
                            url.trim_end_matches('/'));
                        if let Ok(resp) = client.get(&battery_url)
                            .timeout(std::time::Duration::from_secs(3))
                            .send().await
                        {
                            reachable = true;
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                // Common shapes:
                                //   {"battery":{"level":86,"charging":true,"charger":"AC"}}
                                //   {"level":86,"charging":true}
                                let root = j.get("battery").unwrap_or(&j);
                                if let Some(lvl) = root.get("level").and_then(|v| v.as_u64()) {
                                    battery_pct = Some(lvl.min(100) as u8);
                                }
                                if let Some(c) = root.get("charging").and_then(|v| v.as_bool()) {
                                    charging = Some(c);
                                }
                                if let Some(charger) = root.get("charger").and_then(|v| v.as_str()) {
                                    // "AC" / "USB" → on AC means on_battery=false
                                    on_battery = Some(!matches!(charger, "AC" | "ac" | "USB" | "usb"));
                                } else if let Some(level) = battery_pct {
                                    // We have a battery reading at all → on battery
                                    let _ = level;
                                    on_battery = Some(true);
                                }
                            }
                        }

                        // /BeoZone/Zone/NetworkSettings — network interface state
                        let net_url = format!(
                            "{}/BeoZone/Zone/NetworkSettings",
                            url.trim_end_matches('/'));
                        if let Ok(resp) = client.get(&net_url)
                            .timeout(std::time::Duration::from_secs(3))
                            .send().await
                        {
                            reachable = true;
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                // Look for connected ethernet/wifi.
                                let walk = |root: &serde_json::Value| -> (Option<bool>, Option<bool>) {
                                    let mut eth = None;
                                    let mut wifi = None;
                                    if let Some(arr) = root.as_array().or_else(|| root.get("interfaces").and_then(|v| v.as_array())) {
                                        for iface in arr {
                                            let kind = iface.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                            let connected = iface.get("connected").and_then(|v| v.as_bool())
                                                .or_else(|| iface.get("active").and_then(|v| v.as_bool()))
                                                .unwrap_or(false);
                                            match kind.to_ascii_lowercase().as_str() {
                                                "ethernet" | "wired" => eth = Some(connected),
                                                "wifi" | "wireless" => wifi = Some(connected),
                                                _ => {}
                                            }
                                        }
                                    }
                                    (eth, wifi)
                                };
                                let (e, w) = walk(&j);
                                if e.is_some() { on_eth = e; }
                                if w.is_some() { on_wifi = w; }
                            }
                        }

                        // /BeoZone/Zone/Sources — does bluetooth show up?
                        let src_url = format!(
                            "{}/BeoZone/Zone/Sources",
                            url.trim_end_matches('/'));
                        if let Ok(resp) = client.get(&src_url)
                            .timeout(std::time::Duration::from_secs(3))
                            .send().await
                        {
                            reachable = true;
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let txt = j.to_string().to_ascii_lowercase();
                                if txt.contains("bluetooth") {
                                    // Sources can list bluetooth as an
                                    // available input even when not paired —
                                    // we just confirm the radio is there.
                                    bt = Some(true);
                                }
                            }
                        }

                        // If at least one endpoint replied we consider the
                        // device reachable; otherwise it's offline for our
                        // priority purposes.
                        let _ = result_tx.send(ResultMessage::DeviceStateProbed {
                            url, reachable,
                            on_battery, battery_percent: battery_pct, charging,
                            on_ethernet: on_eth, on_wifi: on_wifi,
                            bluetooth_active: bt,
                        });
                    });
                }

                Command::StopPlayback { url } => {
                    let client = client.clone();
                    rt.block_on(async move {
                        // Try stopping via Mozart API
                        let endpoint = format!("{}/BeoZone/Zone/Stream/Stop", url);

                        println!("Stopping playback at: {}", endpoint);
                        if let Ok(resp) = client.post(&endpoint).send().await {
                            println!("Stop status: {}", resp.status());
                        }
                    });
                }

                // ── Sources ─────────────────────────────────────────
                Command::GetSources { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoZone/Zone/Sources", url);
                        match client.get(&endpoint).send().await {
                            Ok(resp) => {
                                if let Ok(json) = resp.json::<Value>().await {
                                    // The ASE/Mozart shape is roughly:
                                    // { "sources": [ [ "id", {friendlyName, sourceType{type}, ...} ], ... ] }
                                    let mut out = Vec::new();
                                    if let Some(arr) = json.get("sources").and_then(|s| s.as_array()) {
                                        for entry in arr {
                                            // entry is often a 2-tuple [id, {meta}]
                                            let (id, meta) = match entry.as_array() {
                                                Some(pair) if pair.len() >= 2 => (
                                                    pair[0].as_str().unwrap_or("").to_string(),
                                                    pair[1].clone(),
                                                ),
                                                _ => continue,
                                            };
                                            let name = meta.get("friendlyName")
                                                .and_then(|v| v.as_str())
                                                .or_else(|| meta.get("sourceType")
                                                    .and_then(|t| t.get("type"))
                                                    .and_then(|v| v.as_str()))
                                                .unwrap_or(&id)
                                                .to_string();
                                            let stype = meta.get("sourceType")
                                                .and_then(|t| t.get("type"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            out.push(serde_json::json!({
                                                "id": id, "name": name, "type": stype
                                            }));
                                        }
                                    }
                                    let json_str = serde_json::to_string(&out).unwrap_or_default();
                                    let active = json.get("active")
                                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let _ = result_tx.send(ResultMessage::Sources {
                                        json: json_str, active_id: active,
                                    });
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("GetSources failed: {}", e)));
                            }
                        }
                    });
                }
                Command::SetSource { url, source_id } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        // Activate a source. Endpoint shape varies by stack;
                        // this targets the BeoZone active-source setter.
                        let endpoint = format!("{}/BeoZone/Zone/ActiveSources", url);
                        let body = serde_json::json!({
                            "primaryExperience": {
                                "source": { "id": source_id }
                            }
                        });
                        match client.post(&endpoint).json(&body).send().await {
                            Ok(resp) => {
                                println!("SetSource status: {}", resp.status());
                                if !resp.status().is_success() {
                                    if let Ok(t) = resp.text().await {
                                        let _ = result_tx.send(ResultMessage::Error(
                                            format!("SetSource rejected: {}", t)));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("SetSource failed: {}", e)));
                            }
                        }
                    });
                }

                // ── Now playing ─────────────────────────────────────
                Command::GetNowPlaying { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoZone/Zone/Sound/Volume", url); // touch to keep-alive
                        let _ = endpoint;
                        let np = format!("{}/BeoZone/Zone/Music", url);
                        match client.get(&np).send().await {
                            Ok(resp) => {
                                if let Ok(json) = resp.json::<Value>().await {
                                    // Best-effort extraction across stack variants.
                                    let title = json.pointer("/playNowMessage/nowPlaying/name")
                                        .or_else(|| json.pointer("/nowPlaying/name"))
                                        .or_else(|| json.pointer("/trackData/name"))
                                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let artist = json.pointer("/playNowMessage/nowPlaying/artist")
                                        .or_else(|| json.pointer("/nowPlaying/artist"))
                                        .or_else(|| json.pointer("/trackData/artist"))
                                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let source = json.pointer("/playNowMessage/source/friendlyName")
                                        .or_else(|| json.pointer("/source/friendlyName"))
                                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let state = json.pointer("/playNowMessage/state")
                                        .or_else(|| json.pointer("/state"))
                                        .and_then(|v| v.as_str()).unwrap_or("");
                                    let playing = state.eq_ignore_ascii_case("play")
                                        || state.eq_ignore_ascii_case("playing");
                                    let _ = result_tx.send(ResultMessage::NowPlaying {
                                        title, artist, source, playing,
                                    });
                                }
                            }
                            Err(_) => { /* now-playing is optional; stay quiet on error */ }
                        }
                    });
                }
                Command::Transport { url, action } => {
                    let client = client.clone();
                    rt.block_on(async move {
                        // Map to the BeoZone Stream control verbs.
                        let verb = match action.as_str() {
                            "play"    => "Play",
                            "pause"   => "Pause",
                            "stop"    => "Stop",
                            "forward" => "Forward",
                            "back"    => "Back",
                            _ => "Play",
                        };
                        let endpoint = format!("{}/BeoZone/Zone/Stream/{}", url, verb);
                        if let Ok(resp) = client.post(&endpoint).send().await {
                            println!("Transport {} status: {}", verb, resp.status());
                        }
                    });
                }

                // ── Standby / power ─────────────────────────────────
                Command::GetStandby { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoDevice/powerManagement/standby", url);
                        if let Ok(resp) = client.get(&endpoint).send().await {
                            if let Ok(json) = resp.json::<Value>().await {
                                let standby = json.pointer("/standby/powerState")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.eq_ignore_ascii_case("standby"))
                                    .unwrap_or(false);
                                let _ = result_tx.send(ResultMessage::Standby(standby));
                            }
                        }
                    });
                }
                Command::SetStandby { url, standby } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoDevice/powerManagement/standby", url);
                        let body = serde_json::json!({
                            "standby": { "powerState": if standby { "standby" } else { "on" } }
                        });
                        match client.put(&endpoint).json(&body).send().await {
                            Ok(resp) => {
                                println!("SetStandby status: {}", resp.status());
                                if !resp.status().is_success() {
                                    if let Ok(t) = resp.text().await {
                                        let _ = result_tx.send(ResultMessage::Error(
                                            format!("SetStandby rejected: {}", t)));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("SetStandby failed: {}", e)));
                            }
                        }
                    });
                }

                // ── Sound modes / presets (Mozart) ──────────────────
                Command::GetSoundModes { url } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoZone/Zone/Sound/Mode", url);
                        match client.get(&endpoint).send().await {
                            Ok(resp) => {
                                if let Ok(json) = resp.json::<Value>().await {
                                    // Shape varies; commonly:
                                    // { "mode": { "active": "id", "list": [ {id, friendlyName}, ... ] } }
                                    let mut out = Vec::new();
                                    let list = json.pointer("/mode/list")
                                        .or_else(|| json.pointer("/list"))
                                        .and_then(|v| v.as_array());
                                    if let Some(arr) = list {
                                        for m in arr {
                                            let id = m.get("id").and_then(|v| v.as_str())
                                                .or_else(|| m.get("mode").and_then(|v| v.as_str()))
                                                .unwrap_or("").to_string();
                                            let name = m.get("friendlyName").and_then(|v| v.as_str())
                                                .or_else(|| m.get("name").and_then(|v| v.as_str()))
                                                .unwrap_or(&id).to_string();
                                            if !id.is_empty() {
                                                out.push(serde_json::json!({ "id": id, "name": name }));
                                            }
                                        }
                                    }
                                    let active = json.pointer("/mode/active")
                                        .or_else(|| json.pointer("/active"))
                                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let json_str = serde_json::to_string(&out).unwrap_or_default();
                                    let _ = result_tx.send(ResultMessage::SoundModes {
                                        json: json_str, active_id: active,
                                    });
                                }
                            }
                            Err(_) => { /* optional feature; stay quiet */ }
                        }
                    });
                }
                Command::SetSoundMode { url, mode_id } => {
                    let client = client.clone();
                    let result_tx = result_tx.clone();
                    rt.block_on(async move {
                        let endpoint = format!("{}/BeoZone/Zone/Sound/Mode/Active", url);
                        let body = serde_json::json!({ "active": mode_id });
                        match client.put(&endpoint).json(&body).send().await {
                            Ok(resp) => {
                                println!("SetSoundMode status: {}", resp.status());
                                if !resp.status().is_success() {
                                    if let Ok(t) = resp.text().await {
                                        let _ = result_tx.send(ResultMessage::Error(
                                            format!("SetSoundMode rejected: {}", t)));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = result_tx.send(ResultMessage::Error(format!("SetSoundMode failed: {}", e)));
                            }
                        }
                    });
                }
            }
        }

        eprintln!("background thread exiting because command channel closed");
    });
}

// ─── Product data helpers ───────────────────────────────────────────────────

use serde::Deserialize;
use std::fs;

/// Legacy product-data schema for the old per-product emoji/icon shape.
/// The current asset pipeline uses SVG silhouettes from assets/products/
/// + the products.json control-flags structure; this remains for
/// compatibility with older configs.
#[allow(dead_code)]
#[derive(Deserialize, Clone)]
struct ProductData {
    emoji: String,
    icon: String,
}

#[allow(dead_code)]
type ProductMap = std::collections::HashMap<String, ProductData>;

// ─── PRODUCTS.JSON LOADER (patched) ─────────────────────────────────────────
//
// Search order at runtime:
//   1. $BO_PRODUCTS_JSON                  (explicit override)
//   2. ./assets/products.json             (project root / dev)
//   3. <exe_dir>/assets/products.json     (binary next to assets/)
//   4. <exe_dir>/../share/bo-controller/products.json   (installed)
//   5. ~/.local/share/bo-controller/products.json       (user install)
//   6. /usr/share/bo-controller/products.json
//   7. /usr/local/share/bo-controller/products.json
//
// Falls back to the JSON embedded at compile time via `include_str!`.
// Never panics — degraded gracefully if everything fails.

fn try_load_products_from_disk() -> Option<(PathBuf, String)> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(env_path) = std::env::var("BO_PRODUCTS_JSON") {
        candidates.push(env_path.into());
    }
    candidates.push("assets/products.json".into());
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets/products.json"));
            candidates.push(dir.join("../share/bo-controller/products.json"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/share/bo-controller/products.json"));
    }
    candidates.push("/usr/share/bo-controller/products.json".into());
    candidates.push("/usr/local/share/bo-controller/products.json".into());

    for c in &candidates {
        if !c.is_file() { continue; }
        match fs::read_to_string(c) {
            Ok(data) => return Some((c.clone(), data)),
            Err(e) => eprintln!("warning: found {} but couldn't read: {}", c.display(), e),
        }
    }
    None
}

/// Resolve products.json from disk or fall back to the embedded copy,
/// then install into the QtState QObject. Never panics.
fn set_products_from_file(pinned: &mut QObjectPinned<QtState>) {
    let (source_label, json_str) = match try_load_products_from_disk() {
        Some((path, data)) => (path.display().to_string(), data),
        None => {
            eprintln!("info: using embedded products.json (no external file found)");
            ("<embedded>".to_string(), EMBEDDED_PRODUCTS_JSON.to_string())
        }
    };

    let json = match serde_json::from_str::<serde_json::Value>(&json_str) {
        Ok(j) => {
            eprintln!("info: loaded products from {}", source_label);
            // ═══════════════════════════════════════════════════════
            // Unmistakable build-version banner. If the user's next
            // log doesn't include the BUILD_TAG line below, the
            // binary they're running is from an older source tree
            // and none of the recent fixes are active. The tag
            // bumps any time PlayOutcome dispatch policy changes,
            // so we can correlate logs to code versions quickly.
            // ═══════════════════════════════════════════════════════
            eprintln!(
                "BUILD_TAG: 2026-06-19; play_outcome=v3-all-failed-always-falls-back; \
                 device_memory=on; bt_fallback_gate=last_success_only; \
                 np_bt_local=on; enqueue_or_play=wired; soundscape_ducking=on; \
                 per_device_strip=on; duration_from_lofty=on; \
                 scrub_bar=on; playbar_volume=on; fabric_drag=on; \
                 mux_api=v2; tags_track_count_fix=on; \
                 tags_survive_rescan=on; volume_grace_window=on; \
                 loop_studio_rename=on; prune_orphans=on; \
                 albums_alpha_fallback=on; playbar_title_clip=on; \
                 ss_no_repeat=on; ss_center_downmix=on; ss_fsm_continuity=on; ss_spatial_spread=on; anc_freq_taper=on; \
                 scene_metadata=on; scene_picker_rich=on; seek_log_fix=on; spaces_connections_target_fix=on; rng_high_bits=on; relink_txn=on; scene_picker_offline=on; ui_bottom_padding=on; scroll_strength=on; scene_bootstrap_heal=on; search_clear_count=on; theme_toast=on; toggle_press_feedback=on; ss_swell=on; bt_volume_active_sink=on; duration_ffprobe_fallback=on; battery_aware=on; duck_silence_skip=on; power_status=on; ss_timing_jitter=on; unknown_artist_fallback=on; all_tracks_view=on; all_tracks_tab=on; play_orphan_reap=on; per_device_head_title=on; resume_single_seek=on; per_device_volume_memory=on; per_device_volume_strip=on; inline_device_volume_stepper=on; quad_lfe_synthesis=on; workshop_opus_input=on; mixer_spatializer_width=on; stem_stereo_placement=on; playlist_drag_reorder=on; queue_policy_modes=on; track_ratings=on; edit_dialog_global=on; edit_long_press=on; rating_in_edit=on; tag_write_back=on; layer_model=on; playpause_resume_fix=on; add_to_playlist_ui=on; playlist_create_enter=on; smooth_scroll_rebound=on; all_tracks_sort=on; album_sort=on; artist_sort=on; lenient_scroll=on; native_folder_picker=on; last_scan_root=on; tag_artist_populate_fix=on; standby_card_overlap_fix=on; playlist_rename_ui=on; repeat_button=on");

            // Device-memory banner: dump every persisted firmware
            // classification so the user can verify the per-device
            // verdict layer is doing its job.
            let persisted = settings_store::keys_with_prefix("device_family:");
            if persisted.is_empty() {
                eprintln!(
                    "device_memory: 0 persisted device classifications \
                     (verdicts get written after any play_uri ALL_FAILED \
                     outcome — formerly only the '500 setData?path=BeoSound' \
                     fingerprint, now any all-endpoints-failed result)");
            } else {
                eprintln!(
                    "device_memory: {} persisted classification(s):",
                    persisted.len());
                for (key, value) in &persisted {
                    let url = key.strip_prefix("device_family:").unwrap_or(key);
                    eprintln!("device_memory:   {} = {}", url, value);
                }
            }
            j
        }
        Err(e) => {
            eprintln!(
                "warning: {} contained invalid JSON ({}), falling back to embedded",
                source_label, e
            );
            match serde_json::from_str::<serde_json::Value>(EMBEDDED_PRODUCTS_JSON) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: embedded products.json failed to parse: {}", e);
                    eprintln!("error: product card will use defaults.");
                    return;
                }
            }
        }
    };

    pinned.borrow_mut().set_product_data(&json);
}

// ─── QML DIRECTORY RESOLVER (patched) ───────────────────────────────────────
//
// Same search-then-embed pattern as products.json, but for the entire
// qml/ directory. If we have to fall back to embedded, extract to a
// per-PID temp directory so QML's relative imports still resolve.

struct QmlDirGuard {
    path: PathBuf,
    is_temp: bool,
}
impl Drop for QmlDirGuard {
    fn drop(&mut self) {
        if self.is_temp {
            // For embedded extraction we created
            // <runtime>/bo-controller-<pid>/{qml,assets}/. Clean up
            // the parent dir to remove both subtrees in one shot.
            // For source-tree disk paths (is_temp=false), we never
            // touch the filesystem here.
            let target = self.path.parent()
                .filter(|p| p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("bo-controller-"))
                    .unwrap_or(false))
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.path.clone());
            let _ = fs::remove_dir_all(&target);
        }
    }
}

/// Look for QML on disk in places that ONLY exist during active
/// development — never in installed locations. The intent is:
///
///   1. `BO_QML_DIR` env var — explicit override for any workflow.
///   2. Source-tree paths reachable from `cargo r` / running the
///      target/release/ binary directly — these enable live-editing
///      QML without rebuilding.
///
/// Installed paths like `~/.local/share/bo-controller/qml` are
/// **intentionally absent**. They used to be in this list, but
/// the moment they exist they win over the embedded copy — which
/// is the just-compiled QML in your binary. Net effect: every
/// QML fix you rebuilt and forgot to install showed up as "still
/// broken." Now: embedded wins by default; you only get disk QML
/// when actually editing the source tree.
fn find_qml_dir_on_disk() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(env_path) = std::env::var("BO_QML_DIR") {
        candidates.push(env_path.into());
    }
    // Run-from-cwd cases (developer workflows).
    candidates.push("qml".into());                  // when cwd is bo-controller/
    candidates.push("bo-controller/qml".into());    // when cwd is bo-suite/ workspace root
                                                    // (covers `cargo r --release`)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("qml"));
            // target/release/bo-controller → walk up to workspace
            // root and look for bo-controller/qml. Lets the binary
            // find its source-tree QML when run from inside target/.
            candidates.push(dir.join("../../bo-controller/qml"));
        }
    }

    for c in &candidates {
        if c.is_dir() && c.join(QML_ENTRY_FILE).is_file() {
            return Some(c.clone());
        }
    }
    None
}

fn extract_dir_recursively(dir: &include_dir::Dir<'_>, target: &Path) {
    for f in dir.files() {
        let dest = target.join(f.path().file_name().unwrap_or_default());
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&dest, f.contents()) {
            eprintln!("warning: couldn't extract {}: {}", dest.display(), e);
        }
    }
    for sub in dir.dirs() {
        let sub_target = target.join(sub.path().file_name().unwrap_or_default());
        let _ = fs::create_dir_all(&sub_target);
        extract_dir_recursively(sub, &sub_target);
    }
}

/// Pick a QML source for the engine. Source-tree disk wins for
/// active-development workflows (lets the user edit QML and re-run
/// without re-compiling). Otherwise, the binary's embedded QML +
/// assets are extracted to a temp dir — which means a freshly-built
/// binary is always self-contained: no `./scripts/install.sh` needed.
///
/// Layout when using embedded:
///
///     <runtime_dir>/bo-controller-<pid>/
///         ├── qml/      ← QML files; engine entry is here
///         └── assets/   ← SVG silhouettes etc; reachable via
///                         `../assets/...` from QML
fn resolve_qml_dir() -> QmlDirGuard {
    if let Some(disk_dir) = find_qml_dir_on_disk() {
        eprintln!("info: loading QML from source tree at {}", disk_dir.display());
        eprintln!("      (set BO_QML_DIR= to override; unset to fall through to embedded)");
        return QmlDirGuard { path: disk_dir, is_temp: false };
    }

    // Build a per-PID temp parent; populate it with both qml/ and
    // assets/. We return the path to the qml/ subdir as the QML
    // root so the engine's entry-file resolution works as it does
    // with a real source tree, while ../assets/ keeps resolving.
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let parent = base.join(format!("bo-controller-{}", std::process::id()));
    if parent.exists() {
        let _ = fs::remove_dir_all(&parent);
    }
    let qml_dir = parent.join("qml");
    let assets_dir = parent.join("assets");
    if let Err(e) = fs::create_dir_all(&qml_dir) {
        eprintln!("error: couldn't create QML temp dir at {}: {}", qml_dir.display(), e);
        return QmlDirGuard { path: qml_dir, is_temp: true };
    }
    let _ = fs::create_dir_all(&assets_dir);
    extract_dir_recursively(&EMBEDDED_QML, &qml_dir);
    extract_dir_recursively(&EMBEDDED_ASSETS, &assets_dir);
    eprintln!("info: loading QML from binary-embedded copy (extracted to {})",
              qml_dir.display());
    eprintln!("info: assets extracted to {}", assets_dir.display());
    // The guard cleans up the QML dir on Drop, but the parent +
    // assets/ dir is left behind (cheap; cleared on next launch
    // and by reboot). To clean up the whole tree, we'd need to
    // teach QmlDirGuard about the parent — easy follow-up if it
    // matters in practice.
    QmlDirGuard { path: qml_dir, is_temp: true }
}

/// Read a WAV file and convert it to a mono `Vec<f32>` in [-1.0, 1.0].
///
/// Handles all the bit-depth + channel-count + sample-format
/// permutations the workers need:
///   - Integer WAVs: 8, 16, 24, 32-bit. 32-bit scale uses u64 shift
///     to avoid the `1_i32 << 31` overflow trap.
///   - Float WAVs: 32-bit IEEE.
///   - Mono and multi-channel (downmixed by mean across channels).
///
/// Returns (mono_samples, sample_rate). On any I/O error the error
/// is logged via `eprintln!` and an empty vec is returned — callers
/// can detect the empty case if they want to bail.
fn wav_to_mono_f32(input_wav: &str) -> Option<(Vec<f32>, u32)> {
    let mut reader = match hound::WavReader::open(input_wav) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("wav_to_mono: can't open {}: {}", input_wav, e);
            return None;
        }
    };
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels as usize;
    if channels == 0 {
        eprintln!("wav_to_mono: {} reports 0 channels — refusing", input_wav);
        return None;
    }
    let bits = spec.bits_per_sample as i32;
    if bits < 1 || bits > 32 {
        eprintln!("wav_to_mono: {} has invalid bits_per_sample {}",
                  input_wav, bits);
        return None;
    }
    // Use u64 shift to avoid the `1_i32 << 31` overflow that would
    // flip the sign of every sample. For bits=32, scale = 1/2^31.
    let scale = 1.0_f32 / (1_u64 << (bits - 1)) as f32;
    let mono: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let samples: Vec<i32> = reader.samples::<i32>()
                .filter_map(|s| s.ok()).collect();
            if channels == 1 {
                samples.iter().map(|&s| s as f32 * scale).collect()
            } else {
                samples.chunks(channels)
                    .map(|frame| {
                        let sum: f32 = frame.iter()
                            .map(|&s| s as f32 * scale).sum();
                        sum / channels as f32
                    }).collect()
            }
        }
        hound::SampleFormat::Float => {
            let samples: Vec<f32> = reader.samples::<f32>()
                .filter_map(|s| s.ok()).collect();
            if channels == 1 { samples }
            else {
                samples.chunks(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                    .collect()
            }
        }
    };
    Some((mono, sample_rate))
}

/// Background worker entry point for the stem-split pipeline. Reads
/// the input WAV, downmixes to mono, runs stem_split, stages, writes
/// 5 output WAVs. All errors go to stderr; QML detects completion by
/// polling the output directory for the expected filenames.
///
/// Output names:
///   drums.wav, voice.wav, piano.wav, synth.wav (mono),
///   staged_stereo.wav (final spatially-mixed stereo).
fn stem_split_worker(input_wav: &str, output_dir: &str) {
    use std::path::Path;
    eprintln!("stem_split: reading {}", input_wav);

    if let Err(e) = std::fs::create_dir_all(output_dir) {
        eprintln!("stem_split: can't create output dir {}: {}", output_dir, e);
        return;
    }

    let (mono, sample_rate) = match wav_to_mono_f32(input_wav) {
        Some(v) => v,
        None => return,
    };
    if mono.is_empty() {
        eprintln!("stem_split: empty input — nothing to do");
        return;
    }
    eprintln!("stem_split: loaded {} mono samples at {} Hz", mono.len(), sample_rate);

    let mlp = stem_split::SmallMlp::with_priors(sample_rate);
    let stems = stem_split::split_stems(&mono, sample_rate, &mlp);
    eprintln!("stem_split: separated four stems, staging…");

    let write_mono = |name: &str, samples: &[f32]| -> Result<(), String> {
        let path = Path::new(output_dir).join(name);
        let s = hound::WavSpec {
            channels: 1, sample_rate,
            bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, s)
            .map_err(|e| format!("create {}: {}", path.display(), e))?;
        for &v in samples {
            let clamped = v.clamp(-1.0, 1.0);
            let i = (clamped * 32767.0) as i16;
            w.write_sample(i).map_err(|e| format!("write: {}", e))?;
        }
        w.finalize().map_err(|e| format!("finalize: {}", e))?;
        Ok(())
    };
    for (name, data) in [
        ("drums.wav", &stems.drums),
        ("voice.wav", &stems.voice),
        ("piano.wav", &stems.piano),
        ("synth.wav", &stems.synth),
    ] {
        if let Err(e) = write_mono(name, data) {
            eprintln!("stem_split: write {} failed: {}", name, e);
            return;
        }
    }

    let cfg = stem_stage::StageConfig::default_for(sample_rate);
    let (left, right) = stem_stage::stage(
        &stems.voice, &stems.drums, &stems.piano, &stems.synth, &cfg,
    );
    let staged_path = Path::new(output_dir).join("staged_stereo.wav");
    let stereo_spec = hound::WavSpec {
        channels: 2, sample_rate,
        bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
    };
    match hound::WavWriter::create(&staged_path, stereo_spec) {
        Ok(mut w) => {
            let n = left.len().min(right.len());
            let mut ok = true;
            for i in 0..n {
                let l = (left[i].clamp(-1.0, 1.0) * 32767.0) as i16;
                let r = (right[i].clamp(-1.0, 1.0) * 32767.0) as i16;
                if w.write_sample(l).is_err() || w.write_sample(r).is_err() {
                    ok = false; break;
                }
            }
            if !ok { eprintln!("stem_split: write staged_stereo failed mid-stream"); }
            else if let Err(e) = w.finalize() {
                eprintln!("stem_split: finalize staged: {}", e);
            } else {
                eprintln!("stem_split: done. 5 files in {}", output_dir);
            }
        }
        Err(e) => eprintln!("stem_split: create staged: {}", e),
    }
}

/// Background worker entry point for podcast diarization. Reads the
/// input WAV, downmixes to mono, runs the diarization pipeline,
/// writes the timeline as JSON to `<input_wav>.diar.json` (next to
/// the input). All errors go to stderr.
fn diarize_worker(input_wav: &str) {
    eprintln!("diarize: reading {}", input_wav);
    let (mono, sample_rate) = match wav_to_mono_f32(input_wav) {
        Some(v) => v,
        None => return,
    };
    if mono.is_empty() {
        eprintln!("diarize: empty input — nothing to diarize");
        return;
    }
    eprintln!("diarize: loaded {} samples at {} Hz", mono.len(), sample_rate);
    let segs = podcast_diar::diarize(&mono, sample_rate);
    let json = serde_json::Value::Array(segs.iter().map(|s| {
        serde_json::json!({
            "start": s.start_s,
            "end":   s.end_s,
            "speaker": s.speaker_id,
        })
    }).collect());
    let out_path = format!("{}.diar.json", input_wav);
    if let Err(e) = std::fs::write(&out_path, serde_json::to_string(&json).unwrap_or_default()) {
        eprintln!("diarize: write {} failed: {}", out_path, e);
        return;
    }
    let unique: std::collections::BTreeSet<u32> = segs.iter().map(|s| s.speaker_id).collect();
    eprintln!("diarize: done. {} segments, {} speakers, written to {}",
              segs.len(), unique.len(), out_path);
}

/// Where the media catalog SQLite DB lives: $XDG_DATA_HOME/bo-controller/
/// (or ~/.local/share/bo-controller/), created if needed.
fn media_db_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("bo-controller");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("media.sqlite")
}

/// Music roots to scan: $BO_MUSIC_DIR (colon-separated) or ~/Music.
fn media_music_roots() -> Vec<PathBuf> {
    if let Ok(dirs) = std::env::var("BO_MUSIC_DIR") {
        let roots: Vec<PathBuf> = dirs.split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !roots.is_empty() { return roots; }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return vec![PathBuf::from(home).join("Music")];
    }
    vec![PathBuf::from("./Music")]
}

fn main() {
    // Print a version banner first thing so the user can see at a
    // glance which build is running. `BO_BUILD_STAMP` is set by
    // build.rs at compile time; if it's empty the build was done
    // outside our normal flow (cargo test, etc).
    eprintln!("bo-controller starting (build: {}, profile: {})",
              env!("BO_BUILD_STAMP"),
              if cfg!(debug_assertions) { "debug" } else { "release" });

    // Register the SSDP cache reader for play_uri. play_uri can't see
    // SSDP_CACHE directly (it lives in main.rs, which isn't a
    // library), so we hand it a closure that looks into the cache.
    // When the background warm-up populates it, play_uri's DLNA
    // strategy will find devices instantly without re-running M-SEARCH.
    play_uri::set_ssdp_cache_provider(|target_host| {
        let cache = SSDP_CACHE.read().ok()?;
        cache.iter()
            .find(|d| dlna_discover::matches_host(d, target_host))
            .cloned()
    });

    // Kick off a background SSDP M-SEARCH right at startup. The
    // controller often boots before the user picks a track, so by
    // the time they tap play the discovery results are cached and
    // play_uri's DLNA strategy can hit the real control URL on the
    // first try (no 1.5 s wait at play time). The cache is owned by
    // the static SSDP_CACHE below; play_uri reads from it.
    std::thread::spawn(|| {
        // Wait briefly for mDNS to have time to surface devices first,
        // so our search hits the network in roughly the same window.
        std::thread::sleep(std::time::Duration::from_secs(2));
        match dlna_discover::m_search(
            dlna_discover::ST_MEDIA_RENDERER,
            std::time::Duration::from_millis(2000),
        ) {
            Ok(devs) => {
                eprintln!("ssdp warmup: found {} MediaRenderer(s)", devs.len());
                for d in &devs {
                    eprintln!("ssdp warmup: {} ({})", d.location, d.server);
                }
                // Stash into the cache. Future per-play SSDP lookups
                // in play_uri can short-circuit by reading this.
                let mut cache = SSDP_CACHE.write().unwrap();
                *cache = devs;
            }
            Err(e) => eprintln!("ssdp warmup: error: {}", e),
        }
    });

    // channels
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (result_tx, result_rx) = mpsc::channel::<ResultMessage>();
    let discovery_tx = result_tx.clone();
    spawn_background_worker(cmd_rx, result_tx);
    spawn_mdns_discovery(discovery_tx);

    // ── Media library worker (owns its own SQLite connection) ───────
    // DB lives under XDG data dir; music roots come from $BO_MUSIC_DIR or
    // a sensible default (~/Music). The worker publishes whatever is
    // already cataloged the instant it starts, so the Library page
    // populates immediately even before any scan completes.
    let (media_cmd_tx, media_cmd_rx) = mpsc::channel::<media::MediaCmd>();
    let (media_res_tx, media_res_rx) = mpsc::channel::<media::MediaResult>();
    let db_path = media_db_path();
    let music_roots = media_music_roots();
    eprintln!("media: db at {}, roots: {:?}", db_path.display(), music_roots);
    thread::spawn(move || {
        media::run_media_worker(media_cmd_rx, media_res_tx, db_path, music_roots, Vec::new());
    });

    // Qt object
    let mut state = QtState::default();
    state.attach_channels(cmd_tx, result_rx);
    state.attach_media_channels(media_cmd_tx, media_res_rx);

    // Set the URL before creating QObjectBox
    state.active_url = "http://192.168.2.12".to_string();
    state.prev_volume = 20;  // fallback restore level for the very first un-mute

    // Kick an initial firmware-family probe of the active device so
    // the UI knows right away whether to offer Mozart-style fallbacks
    // ("Try Network") or Bluetooth-style fallbacks ("Try Bluetooth").
    // The probe is fire-and-forget — if it takes 2 seconds, the rest
    // of startup proceeds without waiting.
    state.refresh_firmware_family();

    // ANC defaults — derive(Default) gives every other field a
    // sensible zero/empty, but the gain values want non-zero
    // out-of-the-box so toggling ANC on without first opening the
    // settings card produces audible (and music-passing) output.
    state.anc_anti_gain = 0.6;        // mirrors MixSettings::default()
    state.anc_music_gain = 1.0;
    state.anc_blend_with_music = true;
    state.anc_standby_phase = "Quiet".to_string();

    // V1: restore the experimental-features setting from disk.
    // Defaults to false (production-only behavior); user opts in
    // via the QML toggle, persisted across restart.
    if let Some(persisted) = settings_store::get_bool("experimental_enabled") {
        v1::set_experimental_enabled(persisted);
        state.experimental_enabled = persisted;
        eprintln!("v1: loaded experimental_enabled={} from settings", persisted);
    }
    // V1: restore saturation slider values. The runner's atomic
    // bit-pattern slot picks these up next time it's started; for
    // now we just stash them on QtState so the QML reads the right
    // initial values.
    if let Some(v) = settings_store::get_float("passby_saturation") {
        state.passby_saturation = v.clamp(0.0, 0.6);
        eprintln!("v1: loaded passby_saturation={} from settings",
                  state.passby_saturation);
    }
    if let Some(v) = settings_store::get_float("drone_saturation") {
        state.drone_saturation = v.clamp(0.0, 0.6);
        eprintln!("v1: loaded drone_saturation={} from settings",
                  state.drone_saturation);
    }

    // Initialize the realtime stream mixer to all-neutral. Defaults
    // (0 dB EQ, 1.0 volume) are bit-identical passthrough — verified
    // by realtime_mixer::tests::roundtrip_neutral_is_identity. Seed
    // the QML-facing properties from the same source of truth.
    state.mix_spk_low  = 0.0; state.mix_spk_mid  = 0.0;
    state.mix_spk_high = 0.0; state.mix_spk_vol  = 1.0;
    state.mix_ins_low  = 0.0; state.mix_ins_mid  = 0.0;
    state.mix_ins_high = 0.0; state.mix_ins_vol  = 1.0;
    state.mix_width    = 1.0;   // spatializer off by default (unity width)
    // Restore the persisted default enqueue mode (Stack if unset/invalid).
    state.enqueue_mode = queue_policy::EnqueueMode::from_tag(
        &settings_store::get_str("enqueue_mode").unwrap_or_default()
    ).as_tag().into();
    // Restore the all-tracks browse sort (Title if unset/invalid).
    {
        let s = settings_store::get_str("all_tracks_sort").unwrap_or_default();
        state.all_tracks_sort = media::catalog::TrackSort::from_tag(&s).to_tag().into();
    }
    {
        let s = settings_store::get_str("album_sort").unwrap_or_default();
        state.album_sort = media::library::AlbumSort::from_tag(&s).to_tag().into();
    }
    {
        let s = settings_store::get_str("artist_sort").unwrap_or_default();
        state.artist_sort = match media::catalog::ArtistPageSort::from_tag(&s) {
            media::catalog::ArtistPageSort::TrackCount => "tracks",
            media::catalog::ArtistPageSort::Name => "name",
        }.into();
    }
    // Last-scanned music location (empty until the first scan).
    state.last_scan_root = settings_store::get_str("last_scan_root")
        .unwrap_or_default().into();
    // Publish the band centers so the QML can label sliders without
    // hardcoding the principle (kept DRY with realtime_mixer.rs).
    state.mixer_band_centers_json = serde_json::json!({
        "spk_low":  realtime_mixer::SPK_LOW_HZ,
        "spk_mid":  realtime_mixer::SPK_MID_HZ,
        "spk_high": realtime_mixer::SPK_HIGH_HZ,
        "ins_low":  realtime_mixer::INS_LOW_HZ,
        "ins_mid":  realtime_mixer::INS_MID_HZ,
        "ins_high": realtime_mixer::INS_HIGH_HZ,
    }).to_string();
    // Seed the initial response curves (everything at neutral →
    // straight 0 dB line). Without this the QML Canvas would draw
    // an empty plot on first render.
    state.apply_mixer_params();
    // Seed the device list. Add more here, or call add_device() from QML.
    // (mDNS/_beoremote._tcp discovery would populate this automatically —
    // a future enhancement; for now it's a configured list.)
    state.devices_json = serde_json::json!([
        { "name": "Beosound Stage", "url": "http://192.168.2.12" }
    ]).to_string();

    // Pre-warm the DNS cache for any hostname-based device URLs (e.g.
    // mDNS .local names). IP-literal URLs (the default device above)
    // skip resolution entirely. Best-effort; results land in the OS
    // resolver cache so the FIRST request after startup pays cache-hit
    // latency rather than cache-miss.
    {
        let mut urls: Vec<String> = vec![state.active_url.clone()];
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&state.devices_json) {
            for d in arr {
                if let Some(u) = d.get("url").and_then(|v| v.as_str()) {
                    urls.push(u.to_string());
                }
            }
        }
        dns_prewarm::warm_urls(&urls);
    }

    // Aggression defaults to LOW. The user can dial up via the slider
    // in Spaces; the chosen level persists across restarts (loaded
    // from the catalog's app_state KV).
    state.aggression = aggression::AggressionPolicy::LOW;
    state.aggression_level = state.aggression.level() as i32;
    state.aggression_label = state.aggression.label().to_string();
    state.current_time_window = anticipator::TimeWindow::now_local().label().to_string();

    // BT auto-connect: dedicated channel for results from spawned
    // bluetoothctl threads. State is restored from persisted KV on
    // first refresh_all (handled via MediaResult::AppKvLoaded for
    // the "bt_autoconnect_devices" key).
    let (bt_tx, bt_rx) = mpsc::channel::<BtAttemptResult>();
    state.bt_result_tx = Some(bt_tx);
    state.bt_result_rx = Some(bt_rx);

    let qtobj = QObjectBox::new(state);

    // Register with QML engine
    qml_register_type::<QtState>(cstr::cstr!("com.bo"), 1, 0, cstr::cstr!("QtState"));

    let mut engine = QmlEngine::new();
    engine.set_object_property("qtobj".into(), qtobj.pinned());

    // The soundscape/multi-speaker daemon bridge, exposed to QML as `mux`.
    let mux = QObjectBox::new(mux_bridge::MuxBridge::default());
    engine.set_object_property("mux".into(), mux.pinned());
    eprintln!("info: registered mux bridge as 'mux' context property");

    // Load product data — disk or embedded fallback
    set_products_from_file(&mut qtobj.pinned());

    // Resolve QML directory — disk or embedded fallback. The guard must
    // stay alive until engine.exec() returns; if we extracted to temp,
    // it cleans up at end of main().
    let qml_dir = resolve_qml_dir();
    let entry_path = qml_dir.path.join(QML_ENTRY_FILE);
    let entry_str = entry_path.to_string_lossy().to_string();
    eprintln!("info: loading entry file: {}", entry_str);
    engine.load_file(entry_str.into());

    println!("QML loaded (check errors above)");

    // run
    engine.exec();

    drop(qml_dir);  // explicit; happens at end of main() anyway
}

#[cfg(test)]
mod main_tests {
    use super::*;

    #[test]
    fn well_formed_accepts_normal_urls() {
        assert!(is_well_formed_http_url("http://192.168.1.10"));
        assert!(is_well_formed_http_url("http://192.168.1.10/"));
        assert!(is_well_formed_http_url("http://192.168.1.10:8080"));
        assert!(is_well_formed_http_url("http://192.168.1.10:8080/api"));
        assert!(is_well_formed_http_url("https://device.local"));
    }

    #[test]
    fn well_formed_rejects_empty_and_partial() {
        assert!(!is_well_formed_http_url(""));
        assert!(!is_well_formed_http_url("http://"));
        assert!(!is_well_formed_http_url("http:///path"));
        assert!(!is_well_formed_http_url("192.168.1.10"));
        assert!(!is_well_formed_http_url("ftp://example.com"));
    }

    #[test]
    fn well_formed_rejects_bad_ports() {
        assert!(!is_well_formed_http_url("http://host:abc"));
        assert!(!is_well_formed_http_url("http://:8080"));
        assert!(!is_well_formed_http_url("http://:"));
    }

    #[test]
    fn well_formed_rejects_ipv6_link_local() {
        // mDNS sometimes surfaces these alongside the routable IPv4.
        // reqwest can't address them; our guard must drop them.
        assert!(!is_well_formed_http_url("http://fe80::209:a7ff:fe77:cd56"));
        assert!(!is_well_formed_http_url("http://[fe80::209:a7ff:fe77:cd56]"));
        assert!(!is_well_formed_http_url("http://[fe80::1]:8080"));
        // But proper routable IPv6 (non-link-local) should still pass.
        assert!(is_well_formed_http_url("http://[2001:db8::1]"));
    }

    #[test]
    fn well_formed_accepts_default_port_implicit() {
        // The user's default device URL — no port, port 80 implied.
        assert!(is_well_formed_http_url("http://192.168.2.12"));
    }

    #[test]
    fn well_formed_handles_whitespace() {
        // We trim and accept.
        assert!(is_well_formed_http_url("  http://192.168.1.10  "));
    }

    #[test]
    fn normalize_device_name_strips_serial_suffix() {
        // The actual case from the user's logs: BT advertises
        // "Beosound Emerge", mDNS advertises the same device as
        // "Beosound-Emerge-36184899". Both should normalize to the
        // same shape so the auto-pair matches.
        let bt   = normalize_device_name("Beosound Emerge");
        let mdns = normalize_device_name("Beosound-Emerge-36184899");
        assert_eq!(bt, mdns,
                   "expected normalized forms to match: bt={:?} mdns={:?}",
                   bt, mdns);
    }

    #[test]
    fn normalize_device_name_lowercases() {
        assert_eq!(normalize_device_name("Beosound Stage"),
                   normalize_device_name("BEOSOUND STAGE"));
        assert_eq!(normalize_device_name("Beosound Stage"), "beosound stage");
    }

    #[test]
    fn normalize_device_name_keeps_short_trailing_tokens() {
        // A short token (<4 chars) at the end shouldn't get stripped
        // since legitimate model names can end in short tokens.
        // "Beosound 1" or "Stage 2".
        assert_eq!(normalize_device_name("Stage 2"), "stage 2");
    }

    #[test]
    fn normalize_device_name_keeps_alphabetic_trailing_tokens() {
        // "Beosound Edge" — Edge is alphabetic, should not be stripped.
        assert_eq!(normalize_device_name("Beosound Edge"), "beosound edge");
    }

    #[test]
    fn normalize_device_name_handles_mixed_punctuation() {
        // Hyphens, underscores, dots → spaces; then collapsed.
        assert_eq!(normalize_device_name("Beosound_Stage.v2"),
                   "beosound stage v2");
    }

    #[test]
    fn normalize_device_name_empty_for_empty_input() {
        assert_eq!(normalize_device_name(""), "");
        assert_eq!(normalize_device_name("  "), "");
        assert_eq!(normalize_device_name("---"), "");
    }

    #[test]
    fn url_to_ip__literal_ipv4() {
        let ip = url_to_ip("http://192.168.2.12");
        assert!(matches!(ip, Some(std::net::IpAddr::V4(_))));
        assert_eq!(ip.unwrap().to_string(), "192.168.2.12");
    }

    #[test]
    fn url_to_ip__ipv4_with_port_and_path() {
        let ip = url_to_ip("http://192.168.2.12:8080/path/here");
        assert_eq!(ip.unwrap().to_string(), "192.168.2.12");
    }

    #[test]
    fn url_to_ip__ipv6_brackets_stripped() {
        let ip = url_to_ip("http://[::1]:8080");
        assert!(matches!(ip, Some(std::net::IpAddr::V6(_))));
    }

    #[test]
    fn url_to_ip__empty_returns_none() {
        // Empty URL has no host.
        assert!(url_to_ip("").is_none());
    }

    #[test]
    fn url_to_ip__no_scheme_falls_through_to_host() {
        // Bare host without scheme — still parseable as IP.
        let ip = url_to_ip("192.168.2.12");
        assert_eq!(ip.unwrap().to_string(), "192.168.2.12");
    }

    #[test]
    fn url_to_ip__localhost_hostname_resolves() {
        // localhost should resolve to 127.0.0.1 (or ::1) on every
        // sane Linux box — this validates the DNS fallback path.
        // Skipped silently if the test host can't resolve localhost
        // (unusual; CI containers sometimes lack /etc/hosts).
        if let Some(ip) = url_to_ip("http://localhost") {
            assert!(ip.is_loopback(),
                "localhost should resolve to a loopback, got {}", ip);
        }
    }
}
