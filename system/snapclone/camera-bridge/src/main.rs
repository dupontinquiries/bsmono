//! camera-bridge — virtual PipeWire camera nodes, demand-driven.
//!
//! Node layout (FP5 / sc7280):
//!   virtual-cam-rear            ← IMX858  ac4a000/camera_29  back-facing (main wide)
//!   virtual-cam-rear-ultrawide  ← S5KJN1  second i2c bus     back-facing (ultrawide, when present)
//!   virtual-cam-front           ← S5KJN1  ac4b000/camera_10  front-facing
//!   virtual-cam-tracking        ← S5KJN1  front + YOLO tracking
//!
//! virtual-cam-rear-ultrawide only spawns when libcamera's `cam --list`
//! reports a SECOND `Location: Back` sensor (or PW_NODE_REAR_ULTRAWIDE
//! is set explicitly). Single-rear devices skip that thread.
//!
//! Both sensors report rotation=270° in the DTS.
//! videoflip=counterclockwise corrects this in every capture pipeline.
//! Post-flip dimensions are portrait: 480×640 (default).

mod capture;
mod detection;
mod output;
mod processing;
mod tracking;

use anyhow::Result;
use log::{error, info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};

use capture::{FrameCapture, probe_camera_nodes, pick_camera_nodes_multi, parse_cam_list};
use detection::FaceDetector;
use output::VirtualCamera;
use processing::{crop_and_resize, FrameConfig};
use tracking::{Tracker, CropRect};

const DEFAULT_PW_NODE_REAR:  &str = "libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29";
const DEFAULT_PW_NODE_FRONT: &str = "libcamera_input._base_soc_0_cci_ac4b000_i2c-bus_1_camera_10";

/// Sensor rotation correction. FP5 sensors are mounted physically rotated:
///   - REAR sensor:  needs counterclockwise (90° anti-clockwise) — produces portrait
///   - FRONT sensor: needs clockwise (90° clockwise) — opposite mount orientation
/// If the displayed video looks sideways, the sensor needs the opposite flip.
const SENSOR_FLIP_REAR:  &str = "counterclockwise";
const SENSOR_FLIP_FRONT: &str = "clockwise";
const DEFAULT_WIDTH:  u32     = 480;
const DEFAULT_HEIGHT: u32     = 640;
const IDLE_GRACE:    Duration = Duration::from_secs(3);
const FRAME_TIMEOUT: Duration = Duration::from_millis(200);
const TARGET_FPS:    u64      = 30;
const FRAME_BUDGET:  Duration = Duration::from_millis(1000 / TARGET_FPS);
const LAP_THRESHOLD: f32      = 50.0;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub pw_node_rear:           String,
    pub pw_node_front:          String,
    /// Optional second back-facing camera (ultrawide on FP5). Empty
    /// string means "not detected" — the bridge will skip spawning
    /// the virtual-cam-rear-ultrawide node when this is empty.
    pub pw_node_rear_ultrawide: String,
    pub model_path:             String,
    pub detection_interval:     f64,
    pub zoom_factor:            f32,
    pub output_width:           u32,
    pub output_height:          u32,
    /// V32.32: rotation method for the rear sensor, passed directly to
    /// `videoflip method=...` in the capture pipeline. Default matches
    /// the FP5 reference DTS (counterclockwise). Override per-device
    /// via SC_CAM_FLIP_REAR env var. Valid values per GStreamer:
    ///   none, clockwise, rotate-180, counterclockwise,
    ///   horizontal-flip, vertical-flip,
    ///   upper-left-diagonal, upper-right-diagonal, automatic
    ///
    /// Different pmOS kernel builds report sensor orientation
    /// differently, and the videoflip method needed to produce a
    /// right-side-up image varies. The default is what worked on the
    /// V32.31 reference build; users on edge kernels may need to flip
    /// to clockwise / rotate-180.
    pub flip_rear:              String,
    pub flip_front:             String,
    pub flip_rear_ultrawide:    String,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        Self {
            pw_node_rear: std::env::var("PW_NODE_REAR")
                .unwrap_or_else(|_| DEFAULT_PW_NODE_REAR.into()),
            pw_node_front: std::env::var("PW_NODE_FRONT")
                .unwrap_or_else(|_| DEFAULT_PW_NODE_FRONT.into()),
            // PW_NODE_REAR_ULTRAWIDE is opt-in; if unset we leave empty
            // and let runtime discovery (cam --list) fill it in.
            pw_node_rear_ultrawide: std::env::var("PW_NODE_REAR_ULTRAWIDE")
                .unwrap_or_default(),
            model_path: std::env::var("MODEL_PATH")
                .unwrap_or_else(|_| "models/face.onnx".into()),
            detection_interval: std::env::var("DETECTION_INTERVAL")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1.0),
            zoom_factor: std::env::var("ZOOM_FACTOR")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1.4),
            output_width: std::env::var("OUTPUT_WIDTH")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_WIDTH),
            output_height: std::env::var("OUTPUT_HEIGHT")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_HEIGHT),
            // V32.32: rotation overrides. Default values match the FP5
            // reference build; users on different kernels can override
            // without rebuilding. Ultrawide defaults to rear's flip
            // (same sensor mount orientation typically) but can be
            // overridden independently if needed.
            flip_rear: std::env::var("SC_CAM_FLIP_REAR")
                .unwrap_or_else(|_| SENSOR_FLIP_REAR.into()),
            flip_front: std::env::var("SC_CAM_FLIP_FRONT")
                .unwrap_or_else(|_| SENSOR_FLIP_FRONT.into()),
            flip_rear_ultrawide: std::env::var("SC_CAM_FLIP_REAR_ULTRAWIDE")
                .ok()
                .or_else(|| std::env::var("SC_CAM_FLIP_REAR").ok())
                .unwrap_or_else(|| SENSOR_FLIP_REAR.into()),
        }
    }
}

// ── Capture pipeline string ───────────────────────────────────────────────────
//
// RULES for postmarketOS / Alpine gst-pipewire:
//
// 1. target-object must NOT be quoted — use bare node name
//    WRONG: pipewiresrc target-object="foo"
//    RIGHT: pipewiresrc target-object=foo
//
// 2. No format constraint before videoconvert — pipewiresrc negotiates natively
//    (NV12, YUY2, YUYV depending on the sensor/driver).
//    videoconvert then converts to RGB for our processing pipeline.
//
// 3. do-timestamp=true — some postmarketOS setups don't attach PTS to buffers,
//    causing downstream "no timestamp" warnings and possible pipeline stalls.
//
// 4. videoflip corrects the 270° rotation from FP5 DTS.
//
// 5. appsink with emit-signals=false and drop=true — we pull via try_pull_sample,
//    not signals. Prevents buffer buildup when processing is slow.
//
fn capture_pipeline(pw_node: &str, flip_method: &str) -> String {
    // V32.32: optional GPU-accelerated YUV→RGB conversion.
    //
    // The pipewiresrc → videoconvert → videoflip chain is the hottest
    // CPU loop in the bridge: videoconvert does YUV→RGB on the CPU,
    // which is ~30% of bridge CPU usage at 30fps. Replacing the
    // conversion with `glupload ! glcolorconvert ! gldownload` moves
    // that work to the GPU (OpenGL ES textures).
    //
    // The flip stays on CPU because it's cheap (axis-aligned memcpy
    // with stride flip) and `gltransformation` doesn't have a clean
    // discrete-rotation API equivalent to videoflip's method= string.
    //
    // Off by default — opt-in via SC_USE_GPU=1. Reasons:
    //   - GL plugin availability varies: gstreamer1.0-plugins-base
    //     must be built with --enable-gl, which is the case on most
    //     pmOS / Alpine / Debian builds but NOT guaranteed.
    //   - On systems without GPU drivers (servers, broken mesa), the
    //     pipeline would fail to start. The CPU path is bulletproof.
    //   - Quality is identical — this is purely a CPU/battery
    //     optimization, not a feature change. Test before defaulting.
    //
    // Probe with: `gst-inspect-1.0 glupload` before enabling.
    let use_gpu = std::env::var("SC_USE_GPU")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);

    // V32.34+: explicit upstream source caps. Without these,
    // pipewiresrc + libcamera_input negotiate to whatever default the
    // sensor's libcamera driver advertises. Asking for a SPECIFIC
    // resolution upstream worked in theory (drive libcamera to pick
    // a better-matching sensor mode) but device testing on FP5
    // showed libcamera doesn't publish 1280×720@30 to pipewiresrc —
    // any non-zero requested caps caused "not-negotiated (-4)" and
    // the whole camera pipeline failed to preroll.
    //
    // DEFAULTS NOW 0 (disabled). To force a specific upstream mode:
    //   CAPTURE_WIDTH=W CAPTURE_HEIGHT=H CAPTURE_FPS=F ./camera-bridge
    // Empty / unset / 0 preserves the original "let libcamera pick"
    // behavior which is the only one known to negotiate cleanly
    // through pipewiresrc on FP5.
    let cap_w: u32 = std::env::var("CAPTURE_WIDTH")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let cap_h: u32 = std::env::var("CAPTURE_HEIGHT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let cap_fps: u32 = std::env::var("CAPTURE_FPS")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let source_caps = if cap_w > 0 && cap_h > 0 {
        if cap_fps > 0 {
            format!("video/x-raw,width={cap_w},height={cap_h},framerate={cap_fps}/1 ! ")
        } else {
            format!("video/x-raw,width={cap_w},height={cap_h} ! ")
        }
    } else {
        String::new()
    };

    if use_gpu {
        // GPU path: videoconvert replaced by glupload → glcolorconvert
        // → gldownload. The final video/x-raw,format=RGB caps tell the
        // downloader what format to produce. The flip stays on CPU.
        format!(
            "pipewiresrc target-object={pw_node} do-timestamp=true \
             ! {source_caps}glupload \
             ! glcolorconvert \
             ! gldownload \
             ! videoflip method={flip_method} \
             ! video/x-raw,format=RGB \
             ! appsink name=sink max-buffers=2 drop=true sync=false emit-signals=false"
        )
    } else {
        format!(
            "pipewiresrc target-object={pw_node} do-timestamp=true \
             ! {source_caps}videoconvert \
             ! videoflip method={flip_method} \
             ! video/x-raw,format=RGB \
             ! appsink name=sink max-buffers=2 drop=true sync=false emit-signals=false"
        )
    }
}

/// V32.32: log which GPU acceleration plugins are available at startup.
///
/// Helps users decide whether to set SC_USE_GPU=1 by surfacing concrete
/// capability info instead of making them guess. Runs once during bridge
/// startup; results are written to the standard log stream so they show
/// up in journalctl alongside the rest of the bridge's INFO output.
///
/// We probe each plugin via `gst-inspect-1.0 <name>` exit code rather
/// than walking the plugin registry directly — the gst-inspect path is
/// cheaper to maintain and matches what users would run by hand.
fn probe_gpu_acceleration() {
    use std::process::Command;
    let have = |name: &str| -> bool {
        Command::new("gst-inspect-1.0")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    let glupload          = have("glupload");
    let glcolorconvert    = have("glcolorconvert");
    let gldownload        = have("gldownload");
    let v4l2h264enc       = have("v4l2h264enc");
    let vulkanupload      = have("vulkanupload");
    let vulkancolorconvert = have("vulkancolorconvert");

    let gl_capture_ok = glupload && glcolorconvert && gldownload;

    info!("GPU acceleration probe:");
    info!("  OpenGL capture chain (glupload/glcolorconvert/gldownload): {} {} {} → {}",
          if glupload          { "✓" } else { "✗" },
          if glcolorconvert    { "✓" } else { "✗" },
          if gldownload        { "✓" } else { "✗" },
          if gl_capture_ok     { "available — set SC_USE_GPU=1 to enable" }
                          else { "NOT available, CPU fallback active" });
    info!("  Vulkan capture chain (vulkanupload/vulkancolorconvert): {} {}",
          if vulkanupload       { "✓" } else { "✗" },
          if vulkancolorconvert { "✓" } else { "✗" });
    info!("  V4L2 H.264 hw encoder (v4l2h264enc): {}",
          if v4l2h264enc { "✓ available" } else { "✗ not available" });
}

// ── Laplacian variance ────────────────────────────────────────────────────────

thread_local! {
    static GREY_BUF: std::cell::RefCell<Vec<f32>> = std::cell::RefCell::new(Vec::new());
}

fn laplacian_variance(frame: &capture::Frame) -> f32 {
    let w = frame.width  as usize;
    let h = frame.height as usize;
    if w < 24 || h < 24 { return 0.0; }

    let step = 8usize;
    let sw = w / step;
    let sh = h / step;
    let needed = sw * sh;

    GREY_BUF.with(|buf| {
        let mut grey = buf.borrow_mut();
        grey.resize(needed, 0.0);

        for gy in 0..sh {
            for gx in 0..sw {
                let idx = ((gy * step) * w + (gx * step)) * 3;
                grey[gy * sw + gx] = if idx + 2 < frame.data.len() {
                    0.299 * frame.data[idx]     as f32
                  + 0.587 * frame.data[idx + 1] as f32
                  + 0.114 * frame.data[idx + 2] as f32
                } else { 0.0 };
            }
        }

        let mut sum = 0f32;
        let mut sum_sq = 0f32;
        let mut n = 0u32;
        for gy in 1..(sh - 1) {
            for gx in 1..(sw - 1) {
                let c = grey[gy * sw + gx];
                let lap = 4.0 * c
                    - grey[(gy-1)*sw + gx] - grey[(gy+1)*sw + gx]
                    - grey[gy*sw + gx - 1] - grey[gy*sw + gx + 1];
                sum    += lap;
                sum_sq += lap * lap;
                n      += 1;
            }
        }
        if n == 0 { return 0.0; }
        let mean = sum / n as f32;
        sum_sq / n as f32 - mean * mean
    })
}

// ── Frame-loop helpers ────────────────────────────────────────────────────────

fn pull_frame(cap: &mut FrameCapture, frame_time: &mut Instant) -> Result<Option<capture::Frame>> {
    let elapsed = frame_time.elapsed();
    let timeout = if elapsed < FRAME_BUDGET {
        (FRAME_BUDGET - elapsed).as_millis().max(5) as u64
    } else {
        FRAME_TIMEOUT.as_millis() as u64
    };
    let result = cap.next_frame_timeout(timeout)?;
    if result.is_some() { *frame_time = Instant::now(); }
    Ok(result)
}

fn push_frame(vcam: &mut VirtualCamera, frame: &capture::Frame, idle_since: &mut Option<Instant>) -> bool {
    match vcam.write_frame(frame) {
        Ok(()) => { *idle_since = None; true }
        Err(e) => {
            let since = idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() > IDLE_GRACE {
                info!("[{}] No consumers — releasing camera", vcam.node_name());
                return false;
            }
            warn!("[{}] write: {e}", vcam.node_name());
            true
        }
    }
}

// ── Passthrough loop ──────────────────────────────────────────────────────────

fn run_passthrough(cap: &mut FrameCapture, vcam: &mut VirtualCamera, frame_cfg: &FrameConfig) -> Result<()> {
    let mut idle_since = None;
    let mut frame_time = Instant::now();
    loop {
        let frame = match pull_frame(cap, &mut frame_time)? {
            Some(f) => f,
            None    => continue,
        };
        let out = if frame.width == vcam.width() && frame.height == vcam.height() {
            frame
        } else {
            let (fw, fh) = (frame.width, frame.height);
            let full = CropRect { x: 0, y: 0, width: fw, height: fh };
            crop_and_resize(frame, full, frame_cfg)?
        };
        if !push_frame(vcam, &out, &mut idle_since) { return Ok(()); }
    }
}

// ── Tracking loop ─────────────────────────────────────────────────────────────

fn run_tracking_loop(
    cap: &mut FrameCapture, vcam: &mut VirtualCamera, frame_cfg: &FrameConfig,
    detector: Option<&FaceDetector>, interval: Duration, zoom: f32,
) -> Result<()> {
    let mut tracker    = Tracker::new();
    let mut last_det   = Instant::now().checked_sub(interval).unwrap_or_else(Instant::now);
    let mut idle_since = None;
    let mut last_lap   = 0f32;
    let mut frame_time = Instant::now();

    loop {
        let frame = match pull_frame(cap, &mut frame_time)? {
            Some(f) => f,
            None    => continue,
        };

        if let Some(det) = detector {
            let lap = laplacian_variance(&frame);
            let interval_done = last_det.elapsed() >= interval;
            let scene_changed = last_lap > 10.0 && (last_lap - lap).abs() > LAP_THRESHOLD;

            if interval_done || scene_changed {
                match det.detect(&frame) {
                    Ok(dets) => {
                        let bbox = dets.into_iter()
                            .max_by(|a, b| {
                                let area = |d: &detection::Detection|
                                    (d.x_max - d.x_min) * (d.y_max - d.y_min);
                                area(a).partial_cmp(&area(b)).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .map(|d| (d.x_min, d.y_min, d.x_max, d.y_max));
                        tracker.update(bbox);
                        last_det = Instant::now();
                    }
                    Err(e) => error!("[{}] YOLO: {e}", vcam.node_name()),
                }
            }
            last_lap = lap;
        }

        let (fw, fh) = (frame.width, frame.height);
        let crop = tracker.smoothed_crop(fw, fh, zoom);
        let out  = crop_and_resize(frame, crop, frame_cfg)?;
        if !push_frame(vcam, &out, &mut idle_since) { return Ok(()); }
    }
}

// ── Pipeline threads ──────────────────────────────────────────────────────────

fn run_rear(config: Arc<Config>) {
    let pipeline_str = capture_pipeline(&config.pw_node_rear, &config.flip_rear);
    let frame_cfg = FrameConfig { output_width: config.output_width, output_height: config.output_height };

    let mut vcam = loop {
        match VirtualCamera::new("virtual-cam-rear", "Rear Camera",
                                 config.output_width, config.output_height) {
            Ok(v)  => break v,
            Err(e) => { error!("[rear] create vcam: {e}"); sleep(5); }
        }
    };

    // V32.32: track consecutive failures so we can escalate logging
    // from `warn!` (easy to miss) to `error!` with the full pipeline
    // string after the user has likely noticed the cam isn't working.
    // The 3-second loop sleep means 3 failures = ~10s of dead camera,
    // which is the user-visible threshold.
    let mut consecutive_failures: u32 = 0;
    loop {
        info!("[rear] Opening physical camera: {}", config.pw_node_rear);
        match FrameCapture::new(&pipeline_str) {
            Ok(mut cap) => {
                consecutive_failures = 0;
                if let Err(e) = run_passthrough(&mut cap, &mut vcam, &frame_cfg) {
                    warn!("[rear] passthrough error after open succeeded: {e}");
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                if consecutive_failures >= 3 {
                    // After 3 failures (~10s of "Starting camera…" from
                    // the user's perspective), dump the FULL pipeline
                    // string and the underlying error at error level.
                    // Common causes:
                    //   - libcamera node never appeared (`apk add
                    //     gst-plugin-libcamera`)
                    //   - rotation method not valid for installed
                    //     videoflip version (try a different
                    //     SC_CAM_FLIP_REAR value)
                    //   - sensor permission denied (libcamera-tools
                    //     setuid not configured)
                    error!("[rear] PERSISTENT FAILURE (#{consecutive_failures}): {e}");
                    error!("[rear] pipeline string was: {pipeline_str}");
                    error!("[rear] hints:");
                    error!("[rear]   • check `pw-dump | grep {}`",
                           config.pw_node_rear);
                    error!("[rear]   • check `apk info gst-plugin-libcamera`");
                    error!("[rear]   • try SC_CAM_FLIP_REAR=rotate-180 (or =none)");
                } else {
                    warn!("[rear] open failed (#{consecutive_failures}): {e}");
                }
            }
        }
        sleep(3);
    }
}

/// Spawn a virtual camera for the rear ULTRAWIDE sensor (FP5: S5KJN1 on
/// a second i2c bus). Same passthrough behavior as `run_rear`, just a
/// different physical source node + virtual node name.
///
/// Only invoked when libcamera reports a second `Location: Back` sensor
/// (see pick_camera_nodes_multi). On phones with only one rear sensor
/// this function never runs.
fn run_rear_ultrawide(config: Arc<Config>) {
    let pipeline_str = capture_pipeline(&config.pw_node_rear_ultrawide, &config.flip_rear_ultrawide);
    let frame_cfg = FrameConfig { output_width: config.output_width, output_height: config.output_height };

    let mut vcam = loop {
        match VirtualCamera::new("virtual-cam-rear-ultrawide", "Rear Camera (Ultrawide)",
                                 config.output_width, config.output_height) {
            Ok(v)  => break v,
            Err(e) => { error!("[rear-uw] create vcam: {e}"); sleep(5); }
        }
    };

    loop {
        info!("[rear-uw] Opening physical camera: {}", config.pw_node_rear_ultrawide);
        match FrameCapture::new(&pipeline_str) {
            Ok(mut cap) => {
                if let Err(e) = run_passthrough(&mut cap, &mut vcam, &frame_cfg) {
                    warn!("[rear-uw] passthrough: {e}");
                }
            }
            Err(e) => warn!("[rear-uw] open failed: {e}"),
        }
        sleep(3);
    }
}

fn run_front(config: Arc<Config>) {
    let pipeline_str = capture_pipeline(&config.pw_node_front, &config.flip_front);
    let frame_cfg = FrameConfig { output_width: config.output_width, output_height: config.output_height };

    let mut vcam = loop {
        match VirtualCamera::new("virtual-cam-front", "Front Camera",
                                 config.output_width, config.output_height) {
            Ok(v)  => break v,
            Err(e) => { error!("[front] create vcam: {e}"); sleep(5); }
        }
    };

    loop {
        info!("[front] Opening physical camera: {}", config.pw_node_front);
        match FrameCapture::new(&pipeline_str) {
            Ok(mut cap) => {
                if let Err(e) = run_passthrough(&mut cap, &mut vcam, &frame_cfg) {
                    warn!("[front] passthrough: {e}");
                }
            }
            Err(e) => warn!("[front] open failed: {e}"),
        }
        sleep(3);
    }
}

fn run_tracking(config: Arc<Config>) {
    let detector = match FaceDetector::new(&config.model_path) {
        Ok(d)  => { info!("[tracking] YOLO model loaded"); Some(d) }
        Err(e) => { warn!("[tracking] no YOLO model ({e}) — passthrough mode"); None }
    };
    let pipeline_str = capture_pipeline(&config.pw_node_front, &config.flip_front);
    let frame_cfg = FrameConfig { output_width: config.output_width, output_height: config.output_height };
    let interval  = Duration::from_secs_f64(config.detection_interval);

    let mut vcam = loop {
        match VirtualCamera::new("virtual-cam-tracking", "Face Tracking Camera",
                                 config.output_width, config.output_height) {
            Ok(v)  => break v,
            Err(e) => { error!("[tracking] create vcam: {e}"); sleep(5); }
        }
    };

    loop {
        info!("[tracking] Opening camera");
        match FrameCapture::new(&pipeline_str) {
            Ok(mut cap) => {
                if let Err(e) = run_tracking_loop(&mut cap, &mut vcam, &frame_cfg,
                                                  detector.as_ref(), interval, config.zoom_factor) {
                    warn!("[tracking] {e}");
                }
            }
            Err(e) => warn!("[tracking] open failed: {e}"),
        }
        sleep(3);
    }
}

fn sleep(s: u64) { std::thread::sleep(Duration::from_secs(s)); }

pub fn is_cgnat_address(ip: &str) -> bool {
    ip.parse::<std::net::Ipv4Addr>().map(|addr| {
        let octets = addr.octets();
        octets[0] == 100 && (octets[1] & 0xC0) == 64
    }).unwrap_or(false)
}

// ── --check ───────────────────────────────────────────────────────────────────

fn check_pipewire_status() -> Result<Vec<String>> {
    use std::process::Command;
    let output = Command::new("pw-cli")
        .arg("list-objects")
        .output()
        .map_err(|e| anyhow::anyhow!("pw-cli not found: {}", e))?;
    if !output.status.success() {
        anyhow::bail!("PipeWire not running (pw-cli list-objects failed)");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut nodes = Vec::new();
    for name in &["virtual-cam-front", "virtual-cam-rear",
                  "virtual-cam-rear-ultrawide", "virtual-cam-tracking"] {
        if text.contains(name) { nodes.push(name.to_string()); }
    }
    Ok(nodes)
}

// ── main() ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    ).init();

    // V32.34+: parent-death watchdog. On Linux, request a SIGTERM
    // when our parent (the snapclone app) dies for any reason —
    // including unexpected SIGKILL where our parent can't run its
    // cleanup destructor. Without this, an OOM-killed app would
    // orphan the bridge, which would keep holding the camera device
    // and block subsequent app starts.
    //
    // Two modes:
    //   - `prctl(PR_SET_PDEATHSIG, SIGTERM)` — sets the signal we
    //     receive when our parent dies. Works on Linux only; safe
    //     to call early in main().
    //   - Periodic getppid() check fallback for other Unixes. Not
    //     implemented here since FP5/pmOS/Alpine is our target;
    //     filed for future portability.
    //
    // Note: this fires when the IMMEDIATE parent dies. If snapclone
    // forks before spawning us we'd watch the wrong parent. Current
    // design has snapclone spawn the bridge directly via QProcess,
    // so the immediate parent IS the right one.
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl is a thread-safe syscall and the constant
        // values are well-known. PR_SET_PDEATHSIG = 1, SIGTERM = 15.
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0);
        }
        info!("Parent-death watchdog: PR_SET_PDEATHSIG=SIGTERM");

        // Race window: if the parent dies BETWEEN its fork and our
        // prctl call, the kernel reparents us to init (pid 1) and
        // we'd never get the signal. Check now: if our parent is
        // already pid 1, the app died during/before our startup
        // and we should exit immediately rather than holding the
        // camera.
        let ppid = unsafe { libc::getppid() };
        if ppid == 1 {
            warn!("Already orphaned (parent is init) — exiting to release camera");
            std::process::exit(0);
        }
    }

    let config = Arc::new(Config::from_env());

    // Handle --check before gstreamer::init() so it can be called without a display
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a == "--check").unwrap_or(false) {
        match check_pipewire_status() {
            Ok(nodes) => {
                println!("ok");
                for node in &nodes { println!("node: {}", node); }
            }
            Err(e) => { eprintln!("error: {}", e); }
        }

        gstreamer::init().ok();
        let hw_nodes = probe_camera_nodes();
        if hw_nodes.is_empty() {
            eprintln!("warning: no libcamera nodes found in pw-dump");
            eprintln!("hint: apk add gst-plugin-libcamera pipewire-gstreamer");
            eprintln!("hint: check: pw-dump | grep media.class");
        } else {
            for (name, desc) in &hw_nodes {
                println!("hw: {} ({})", name, desc);
            }
        }
        std::process::exit(0);
    }

    // V32.32: --probe-cameras emits a full diagnostic dump of camera
    // discovery state. Helps debug "rear shows as front" on FP5 where
    // we need to see what libcamera and PipeWire actually report
    // before the picker makes its decision. Output sections:
    //   1. cam --list raw entries (sensor model + path + Location)
    //   2. pw-dump libcamera_input nodes (name + description)
    //   3. What pick_camera_nodes_multi would choose given the above
    //   4. Whether keyword fallback was used vs cam-list parsing
    //
    // Usage on device: `snapclone-camera-bridge --probe-cameras`
    // Paste output into a bug report; we can write a targeted fix
    // from that without round-tripping through rebuilds.
    if args.get(1).map(|a| a == "--probe-cameras").unwrap_or(false) {
        gstreamer::init().ok();
        println!("=== camera-bridge --probe-cameras ===");
        println!();
        println!("[1] libcamera `cam --list` entries:");
        match parse_cam_list() {
            Some(entries) => {
                if entries.is_empty() {
                    println!("   (cam --list produced no parseable entries)");
                } else {
                    for (i, e) in entries.iter().enumerate() {
                        println!("   {}: model='{}' path={} location={}",
                                 i + 1, e.model, e.path_id, e.location);
                    }
                }
            }
            None => {
                println!("   (cam --list unavailable or returned no output)");
                println!("   hint: 'cam' is the libcamera CLI — apk add libcamera-tools");
            }
        }
        println!();
        println!("[2] pw-dump libcamera_input nodes:");
        let probed = probe_camera_nodes();
        if probed.is_empty() {
            println!("   (no libcamera_input nodes visible in PipeWire)");
            println!("   hint: apk add gst-plugin-libcamera pipewire-gstreamer");
        } else {
            for (i, (name, desc)) in probed.iter().enumerate() {
                println!("   {}: name='{}' desc='{}'", i + 1, name, desc);
            }
        }
        println!();
        println!("[3] pick_camera_nodes_multi decision (using compiled defaults as fallback):");
        let picked = pick_camera_nodes_multi(
            &probed,
            DEFAULT_PW_NODE_REAR,
            DEFAULT_PW_NODE_FRONT,
        );
        println!("   rear        = {}", picked.rear);
        println!("   front       = {}", picked.front);
        if picked.extra_rear.is_empty() {
            println!("   extra_rear  = (none)");
        } else {
            for (i, n) in picked.extra_rear.iter().enumerate() {
                println!("   extra_rear[{}] = {}", i, n);
            }
        }
        println!();
        println!("[4] Diagnostic notes:");
        // If cam --list returned entries, the cam-list branch was used.
        // If not, the keyword fallback was used. That's the single
        // most important piece of context for debugging "wrong sensor".
        let used_cam_list = parse_cam_list().is_some();
        println!("   path used   = {}",
                 if used_cam_list { "cam --list (libcamera Location field)" }
                 else { "keyword fallback (substring match on pw-dump desc)" });
        // Compare picked rear/front against the compiled defaults.
        // If they match exactly, no actual discovery happened — both
        // branches fell through to defaults, which on a multi-sensor
        // device is suspicious.
        if picked.rear == DEFAULT_PW_NODE_REAR && picked.front == DEFAULT_PW_NODE_FRONT {
            println!("   warning     = picked = compiled defaults; discovery may have failed");
        }
        std::process::exit(0);
    }

    gstreamer::init()?;

    // Probe actual hw node names — postmarketOS names differ from compiled defaults.
    // pick_camera_nodes_multi also discovers any ADDITIONAL back-facing
    // sensors beyond the primary rear (e.g. FP5's ultrawide). We honor a
    // user-set PW_NODE_REAR_ULTRAWIDE env var if present, otherwise take
    // the first extra back camera the system reports.
    let config = {
        let probed = probe_camera_nodes();
        if probed.is_empty() {
            warn!("No hw camera nodes found via pw-dump — using compiled defaults");
            warn!("Install: apk add gst-plugin-libcamera");
        }
        // V32.34+: log the raw `cam --list` output up front. Phone B
        // testing showed PIP "fails and falls back to rear but shows
        // front" — which means the bridge had front/rear node labels
        // swapped relative to physical sensors. The hardcoded
        // DEFAULT_PW_NODE_REAR/FRONT match Phone A's enumeration; on
        // Phone B with different kernel/libcamera versions, the
        // cci_ac4a000 / cci_ac4b000 might be enumerated reverse, OR
        // the path normalization in parse_cam_list might not match
        // the pw-dump node names exactly.
        //
        // Logging the cam --list entries AND the probed pw-dump nodes
        // lets the user (or me) match by eye when something looks
        // wrong on a new device.
        if let Some(entries) = parse_cam_list() {
            info!("cam --list returned {} entries:", entries.len());
            for (i, e) in entries.iter().enumerate() {
                info!("  [{i}] model='{}' path={} location={}",
                      e.model, e.path_id, e.location);
            }
        } else {
            warn!("cam --list unavailable or empty — falling back to keyword matching");
            warn!("Install: apk add libcamera-tools  (provides 'cam' CLI)");
        }
        info!("pw-dump probed {} camera nodes:", probed.len());
        for (i, (name, desc)) in probed.iter().enumerate() {
            info!("  [{i}] name='{name}' desc='{desc}'");
        }
        let picked = pick_camera_nodes_multi(
            &probed,
            &config.pw_node_rear,
            &config.pw_node_front,
        );
        // Resolve the ultrawide source node — env override wins, then
        // first extra-rear from discovery, else empty (no spawn).
        let ultrawide = if !config.pw_node_rear_ultrawide.is_empty() {
            // User-supplied via PW_NODE_REAR_ULTRAWIDE env var.
            info!("Using user-supplied rear ultrawide node: {}",
                  config.pw_node_rear_ultrawide);
            config.pw_node_rear_ultrawide.clone()
        } else if let Some(uw) = picked.extra_rear.first() {
            info!("Using discovered rear ultrawide node: {uw}");
            uw.clone()
        } else {
            info!("No second back-facing camera detected — ultrawide disabled");
            String::new()
        };
        Arc::new(Config {
            pw_node_rear:           picked.rear,
            pw_node_front:          picked.front,
            pw_node_rear_ultrawide: ultrawide,
            ..(*config).clone()
        })
    };

    info!("camera-bridge ready");
    info!("  rear hw node:           {}", config.pw_node_rear);
    info!("  front hw node:          {}", config.pw_node_front);
    if !config.pw_node_rear_ultrawide.is_empty() {
        info!("  rear-ultrawide hw node: {}", config.pw_node_rear_ultrawide);
    } else {
        info!("  rear-ultrawide hw node: (none)");
    }
    info!("  output: {}×{} @ {}fps", config.output_width, config.output_height, TARGET_FPS);
    // V32.32: announce active flip methods so a user comparing output
    // to expectation can confirm the override took effect. The defaults
    // are correct for the FP5 reference build; edge-kernel users may
    // need SC_CAM_FLIP_REAR=clockwise or =rotate-180 etc.
    info!("  flip rear:              {} (override: SC_CAM_FLIP_REAR)",  config.flip_rear);
    info!("  flip front:             {} (override: SC_CAM_FLIP_FRONT)", config.flip_front);
    if !config.pw_node_rear_ultrawide.is_empty() {
        info!("  flip rear-ultrawide:    {} (override: SC_CAM_FLIP_REAR_ULTRAWIDE)",
              config.flip_rear_ultrawide);
    }

    // V32.32: GPU acceleration capability probe + active state.
    // Prints once at startup so the user knows what's available on
    // this system and whether their SC_USE_GPU env var actually
    // took effect.
    probe_gpu_acceleration();
    let gpu_active = std::env::var("SC_USE_GPU")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if gpu_active {
        info!("  capture path:           GPU (SC_USE_GPU=1)");
    } else {
        info!("  capture path:           CPU (set SC_USE_GPU=1 to use GPU if available)");
    }

    // Validate that a test pipeline can be constructed before spawning threads.
    // This catches "element not found" errors (missing gst plugins) early.
    {
        let test_str = capture_pipeline(&config.pw_node_rear, &config.flip_rear);
        info!("Capture pipeline: {}", test_str);
        // Don't actually run it — just parse to catch syntax errors
        match gstreamer::parse::launch(&test_str) {
            Ok(_)  => info!("Pipeline parse: OK"),
            Err(e) => {
                error!("Pipeline parse FAILED: {e}");
                error!("Check: gst-inspect-1.0 pipewiresrc videoconvert videoflip appsink");
                std::process::exit(1);
            }
        }
    }

    let cfg_r = Arc::clone(&config);
    let cfg_f = Arc::clone(&config);
    let cfg_t = Arc::clone(&config);

    // Stagger thread starts — libcamera queues exclusive access per sensor
    let tr = std::thread::Builder::new().name("cam-rear".into())
        .spawn(move || run_rear(cfg_r)).expect("spawn rear");

    std::thread::sleep(Duration::from_millis(800));
    let tf = std::thread::Builder::new().name("cam-front".into())
        .spawn(move || run_front(cfg_f)).expect("spawn front");

    std::thread::sleep(Duration::from_millis(800));
    let tt = std::thread::Builder::new().name("cam-tracking".into())
        .spawn(move || run_tracking(cfg_t)).expect("spawn tracking");

    // Optional fourth thread for the rear ultrawide sensor when detected
    // (FP5: S5KJN1 on the second i2c bus). We staggered AFTER the
    // tracking thread so the libcamera initialization order stays
    // [rear-main, front, tracking-on-front, rear-ultrawide] — each
    // bus gets time to settle before the next consumer opens it.
    let tu = if !config.pw_node_rear_ultrawide.is_empty() {
        std::thread::sleep(Duration::from_millis(800));
        let cfg_u = Arc::clone(&config);
        Some(std::thread::Builder::new().name("cam-rear-uw".into())
            .spawn(move || run_rear_ultrawide(cfg_u)).expect("spawn rear-ultrawide"))
    } else {
        None
    };

    tr.join().ok(); tf.join().ok(); tt.join().ok();
    if let Some(t) = tu { t.join().ok(); }
    Ok(())
}
