//! camera-view — fullscreen camera display window.
//!
//! Reads from a virtual PipeWire camera node written by camera-bridge and
//! displays it fullscreen.  Never opens physical camera hardware directly.
//!
//! Rotation defaults to R0; camera-bridge corrects the 270° sensor rotation
//! in the capture pipeline. Use --rotate to override if needed.
//!
//! The `CaptureHandle` wraps the GStreamer thread with an AtomicBool stop flag
//!
//! Usage:
//!   camera-view [OPTIONS]
//!
//!   -c, --camera <NODE>   rear | ultrawide | front | tracking | <full node>
//!                         [default: virtual-cam-rear]
//!   -r, --rotate <DEG>    0 | 90 | 180 | 270 | auto  [default: auto]
//!       --no-cursor       hide cursor (wallpaper mode)
//!       --title <STRING>  window title
//!   -h, --help

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use eframe::egui;
use log::{error, info, warn};

use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video::VideoInfo;

// ── Available virtual nodes (created by camera-bridge) ───────────────────────
// camera-view reads from one of these.  The physical cameras are never touched.
//
//   virtual-cam-rear            — rear main wide (IMX858)         ← default
//   virtual-cam-rear-ultrawide  — rear ultrawide (S5KJN1, FP5)    (when present)
//   virtual-cam-front           — front (S5KJN1)
//   virtual-cam-tracking        — front + YOLO face tracking
//
// Selectable via:
//   --camera <node>            full node name OR shorthand "rear" / "ultrawide" /
//                              "front" / "tracking"
//   CAMERA_NODE=<node>         env var, same accepted values
//
// Shorthand resolution is just a convenience for the .desktop file —
// "--camera ultrawide" expands to virtual-cam-rear-ultrawide.
const DEFAULT_VIRTUAL_NODE: &str = "virtual-cam-rear";

/// Resolve a user-facing shorthand to the full virtual-cam-* node name.
/// Returns the input unchanged when it's already a full node name (i.e.
/// contains a hyphen — heuristic but reliable since shorthands are all
/// single words).
fn resolve_camera_shorthand(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "rear" | "main" | "wide"   => "virtual-cam-rear".into(),
        "ultrawide" | "uw" | "ultra" => "virtual-cam-rear-ultrawide".into(),
        "front" | "selfie"          => "virtual-cam-front".into(),
        "tracking" | "track"        => "virtual-cam-tracking".into(),
        _ => s.to_string(),  // assume already a node name
    }
}

// ── Rotation ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Rotation {
    Auto,
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "auto" | "Auto" => Ok(Self::Auto),
            "0"             => Ok(Self::R0),
            "90"            => Ok(Self::R90),
            "180"           => Ok(Self::R180),
            "270"           => Ok(Self::R270),
            other => Err(anyhow!(
                "Unknown rotation '{}'. Valid: auto, 0, 90, 180, 270", other
            )),
        }
    }

    /// Returns the GStreamer `videoflip method=` string, or None for no flip.
    fn videoflip_method(self) -> Option<&'static str> {
        match self {
            Self::Auto | Self::R0 => None,
            Self::R90             => Some("clockwise"),
            Self::R180            => Some("rotate-180"),
            Self::R270            => Some("counterclockwise"),
        }
    }

    /// Resolve Auto rotation.
    /// camera-bridge applies videoflip in the capture pipeline, so frames
    /// from virtual nodes are already portrait-up.  Auto therefore resolves
    /// to R0 (no additional flip in camera-view).
    /// The screen/frame dimensions are kept as parameters for future use
    /// when reading raw (uncorrected) PipeWire sources directly.
    fn resolve(self, _frame_w: usize, _frame_h: usize, _screen_w: f32, _screen_h: f32) -> Self {
        if self != Self::Auto {
            return self;
        }
        Self::R0  // camera-bridge already corrected sensor rotation
    }
}

// ── Config ────────────────────────────────────────────────────────────────────

struct Config {
    pw_node:   String,
    rotation:  Rotation,
    no_cursor: bool,
    title:     String,
}

impl Config {
    fn from_args() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let args: Vec<String> = std::env::args().collect();
        let mut rotation_str: Option<String> = None;
        let mut no_cursor = false;
        let mut title = "camera-view".to_string();
        let mut camera_arg: Option<String> = None;
        let mut i = 1;

        while i < args.len() {
            match args[i].as_str() {
                "-h" | "--help" => { print_help(&args[0]); std::process::exit(0); }
                "-r" | "--rotate" => {
                    i += 1;
                    rotation_str = Some(
                        args.get(i)
                            .ok_or_else(|| anyhow!("--rotate requires an argument"))?
                            .clone(),
                    );
                }
                other if other.starts_with("--rotate=") => {
                    rotation_str = Some(other.trim_start_matches("--rotate=").to_string());
                }
                "--no-cursor" => no_cursor = true,
                "--title" => {
                    i += 1;
                    title = args.get(i)
                        .ok_or_else(|| anyhow!("--title requires an argument"))?
                        .clone();
                }
                other if other.starts_with("--title=") => {
                    title = other.trim_start_matches("--title=").to_string();
                }
                // --camera <node>: pick which virtual-cam-* PipeWire node to
                // display. Accepts full node names or shorthand:
                //   --camera rear       → virtual-cam-rear
                //   --camera ultrawide  → virtual-cam-rear-ultrawide
                //   --camera front      → virtual-cam-front
                //   --camera tracking   → virtual-cam-tracking
                // Or any literal node name (anything containing a hyphen is
                // assumed to already be a full name).
                "-c" | "--camera" => {
                    i += 1;
                    camera_arg = Some(
                        args.get(i)
                            .ok_or_else(|| anyhow!("--camera requires an argument"))?
                            .clone(),
                    );
                }
                other if other.starts_with("--camera=") => {
                    camera_arg = Some(other.trim_start_matches("--camera=").to_string());
                }
                other => return Err(anyhow!("Unknown argument: {}", other)),
            }
            i += 1;
        }

        // camera-bridge applies videoflip in the capture pipeline to correct the
        // 270° sensor rotation.  Frames from virtual nodes are already portrait-up,
        // so camera-view defaults to no rotation.  Use --rotate or CAMERA_ROTATION
        // to override if needed.
        let rot_str = rotation_str
            .or_else(|| std::env::var("CAMERA_ROTATION").ok())
            .unwrap_or_else(|| "0".to_string());

        // Camera node selection priority (highest wins):
        //   1. --camera CLI arg
        //   2. CAMERA_NODE env var (preferred new name; concise)
        //   3. PW_SOURCE_NODE env var (legacy name, kept for back-compat)
        //   4. compiled default (virtual-cam-rear)
        //
        // The CLI arg supports shorthand names ("ultrawide", "front", etc.)
        // via resolve_camera_shorthand; env vars expect either shorthand or
        // a full node name.
        let raw_node = camera_arg
            .or_else(|| std::env::var("CAMERA_NODE").ok())
            .or_else(|| std::env::var("PW_SOURCE_NODE").ok())
            .unwrap_or_else(|| DEFAULT_VIRTUAL_NODE.to_string());
        let pw_node = resolve_camera_shorthand(&raw_node);
        if pw_node != raw_node {
            info!("Resolved camera shorthand '{raw_node}' → '{pw_node}'");
        }

        Ok(Self {
            pw_node,
            rotation: Rotation::from_str(&rot_str)?,
            no_cursor,
            title,
        })
    }
}

fn print_help(bin: &str) {
    println!(
"camera-view — fullscreen camera display (transparent phone / wallpaper)

Reads from the virtual camera nodes written by camera-bridge.
Start camera-bridge before camera-view.

USAGE:
    {bin} [OPTIONS]

OPTIONS:
    -c, --camera <NODE>   Which virtual camera to display.
                          Shorthand: rear | ultrawide | front | tracking
                          Or a full node name (anything with a hyphen).
                          [default: virtual-cam-rear]
    -r, --rotate <DEG>    Rotation: auto | 0 | 90 | 180 | 270 [default: auto]
        --no-cursor       Hide cursor (wallpaper mode)
        --title <STRING>  Window title [default: camera-view]
    -h, --help

ROTATION:
    auto   Auto-detect from frame vs screen aspect ratio (recommended).
           Resolves to R0 — camera-bridge corrects rotation already.
    0/90/180/270  Force a specific rotation.

ENVIRONMENT:
    CAMERA_NODE       Virtual node to read (preferred new name).
                      Same shorthand resolution as --camera.
    PW_SOURCE_NODE    Legacy alias for CAMERA_NODE (kept for back-compat).
    CAMERA_ROTATION   Default rotation [auto]

EXAMPLES:
    {bin}                                           # rear cam, auto-rotate
    {bin} --camera ultrawide                        # rear ultrawide (FP5)
    {bin} --camera front --no-cursor                # front cam wallpaper
    {bin} --rotate 270                              # force 270°
    CAMERA_NODE=tracking {bin}                      # face tracking via env
"
    );
}

// ── Shared frame ──────────────────────────────────────────────────────────────

struct CameraFrame {
    pixels: Vec<u8>,   // RGBA8 row-major
    width:  usize,
    height: usize,
}

type SharedFrame = Arc<ArcSwap<Option<CameraFrame>>>;

// ── GStreamer capture pipeline ────────────────────────────────────────────────

fn build_pipeline(pw_node: &str, flip_method: Option<&str>) -> String {
    let flip = match flip_method {
        Some(m) => format!("videoflip method={m} ! "),
        None    => String::new(),
    };
    format!(
        "pipewiresrc target-object=\"{pw_node}\" ! \
         videoconvert ! \
         {flip}\
         video/x-raw,format=RGBA ! \
         appsink name=sink max-buffers=2 drop=true sync=false"
    )
}

/// Run a capture pipeline until `stop` is set or an error occurs.
fn run_pipeline(
    pipeline_str: &str,
    shared:       SharedFrame,
    stop:         Arc<AtomicBool>,
) -> Result<()> {
    gstreamer::init()?;

    let pipeline = gstreamer::parse::launch(pipeline_str)?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow!("Not a pipeline"))?;

    let appsink = pipeline
        .by_name("sink")
        .ok_or_else(|| anyhow!("No appsink"))?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("Not an appsink"))?;

    appsink.set_caps(Some(
        &gstreamer::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .build(),
    ));
    appsink.set_sync(false);

    pipeline.set_state(gstreamer::State::Playing)?;
    pipeline
        .state(gstreamer::ClockTime::from_seconds(5))
        .0
        .map_err(|_| anyhow!("Pipeline did not reach PLAYING"))?;

    info!("Capture pipeline playing: {pipeline_str}");

    // Cache frame dimensions — caps never change during a pipeline run.
    // Parsing VideoInfo on every frame allocates several heap strings at 30fps.
    let mut cached_dims: Option<(usize, usize)> = None;

    loop {
        // Check stop flag before blocking on pull_sample.
        if stop.load(Ordering::Relaxed) {
            info!("Capture thread received stop signal");
            break;
        }

        let sample = match appsink.try_pull_sample(gstreamer::ClockTime::from_mseconds(100)) {
            Some(s) => s,
            None    => continue, // timeout — loop and check stop flag again
        };

        // Parse caps only on the first frame; reuse thereafter.
        let (width, height) = match cached_dims {
            Some(d) => d,
            None => {
                let caps = sample.caps().ok_or_else(|| anyhow!("No caps"))?;
                let info = VideoInfo::from_caps(caps)?;
                let d = (info.width() as usize, info.height() as usize);
                cached_dims = Some(d);
                info!("View pipeline: {}×{} RGBA", d.0, d.1);
                d
            }
        };

        let buffer = sample.buffer().ok_or_else(|| anyhow!("No buffer"))?;
        let map    = buffer.map_readable()
            .map_err(|_| anyhow!("Cannot map buffer"))?;
        let pixels = map.as_slice().to_vec();

        // Release GStreamer buffer refs before storing the new frame.
        // This keeps the appsink queue from filling while we hold references.
        drop(map);
        drop(sample);

        shared.store(Arc::new(Some(CameraFrame { pixels, width, height })));
    }

    let _ = pipeline.set_state(gstreamer::State::Null);
    Ok(())
}

// ── Capture thread management ─────────────────────────────────────────────────

struct CaptureHandle {
    stop:   Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle {
    /// Spawn a new capture thread with the given pipeline.
    fn spawn(pipeline_str: String, shared: SharedFrame) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("gst-capture".into())
            .spawn(move || {
                if let Err(e) = run_pipeline(&pipeline_str, shared, stop_clone) {
                    error!("Capture thread error: {e}");
                }
            })
            .expect("Failed to spawn capture thread");
        Self { stop, thread: Some(thread) }
    }

    /// Signal the capture thread to stop and wait for it to exit.
    /// Uses a separate thread to enforce a 2s timeout so the UI never
    /// hangs permanently if the GStreamer pipeline stalls.
    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            // Spawn a watcher that joins with a deadline.
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let done2 = std::sync::Arc::clone(&done);
            std::thread::spawn(move || {
                let _ = t.join();
                done2.store(true, Ordering::Relaxed);
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !done.load(Ordering::Relaxed) {
                if std::time::Instant::now() > deadline {
                    warn!("Capture thread did not stop within 2s — continuing anyway");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

// ── egui application ──────────────────────────────────────────────────────────

struct CameraApp {
    pw_node:           String,
    rotation:          Rotation,
    resolved_rotation: Option<Rotation>,
    shared:            SharedFrame,
    capture:           Option<CaptureHandle>,
    texture:           Option<egui::TextureHandle>,
    texture_size:      (usize, usize),
    /// Last uploaded Arc — compare identity to skip redundant GPU uploads.
    last_frame_arc:    Option<Arc<Option<CameraFrame>>>,
    last_frame_time:   Instant,
    frame_count:       u64,
    fps:               f32,
    no_cursor:         bool,
}

impl CameraApp {
    fn new(config: Config, shared: SharedFrame) -> Self {
        let rotation = config.rotation;
        // If rotation is explicit (not Auto) we can start the pipeline immediately
        // with the correct flip baked in.  If Auto we start without flip and
        // restart once we know the screen size.
        let flip = if rotation == Rotation::Auto {
            None
        } else {
            rotation.videoflip_method()
        };
        let pipeline_str = build_pipeline(&config.pw_node, flip);
        let capture = CaptureHandle::spawn(pipeline_str, Arc::clone(&shared));

        Self {
            pw_node:           config.pw_node,
            rotation,
            resolved_rotation: if rotation == Rotation::Auto { None } else { Some(rotation) },
            shared,
            capture:           Some(capture),
            texture:           None,
            texture_size:      (0, 0),
            last_frame_arc:    None,
            last_frame_time:   Instant::now(),
            frame_count:       0,
            fps:               0.0,
            no_cursor:         config.no_cursor,
        }
    }

    /// Restart the capture pipeline with a specific flip method.
    /// Stops the old pipeline first to prevent the rotation stutter.
    fn restart_with_rotation(&mut self, resolved: Rotation) {
        info!("Restarting capture pipeline with rotation {:?}", resolved);

        // 1. Stop the old pipeline — blocks until the thread exits.
        if let Some(mut h) = self.capture.take() {
            h.stop_and_join();
        }

        // 2. Clear stale frames so we don't display a rotated frame
        //    while the new pipeline warms up.
        self.shared.store(Arc::new(None));
        self.texture      = None;
        self.texture_size = (0, 0);

        // 3. Start fresh pipeline with correct flip.
        let pipeline_str = build_pipeline(&self.pw_node, resolved.videoflip_method());
        self.capture = Some(CaptureHandle::spawn(pipeline_str, Arc::clone(&self.shared)));
    }

    fn upload_texture(&mut self, ctx: &egui::Context, frame: &CameraFrame) {
        let needs_new = self.texture.is_none()
            || self.texture_size != (frame.width, frame.height);

        let opts = egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification:  egui::TextureFilter::Linear,
            ..Default::default()
        };
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width, frame.height],
            &frame.pixels,
        );

        if needs_new {
            self.texture = Some(ctx.load_texture("camera_frame", image, opts));
            self.texture_size = (frame.width, frame.height);
            info!("Texture allocated {}×{}", frame.width, frame.height);
        } else if let Some(tex) = self.texture.as_mut() {
            tex.set(image, opts);
        }
    }

    /// Cover-mode render: scale to fill the panel completely, crop overflow.
    fn draw_cover(ui: &mut egui::Ui, tex: &egui::TextureHandle, panel_rect: egui::Rect) {
        let panel  = panel_rect.size();
        let tex_sz = tex.size_vec2();
        let scale  = (panel.x / tex_sz.x).max(panel.y / tex_sz.y);
        let scaled = tex_sz * scale;
        let offset = (panel - scaled) * 0.5;
        let draw_rect = egui::Rect::from_min_size(panel_rect.min + offset, scaled);

        let uv_min = egui::pos2(
            (panel_rect.min.x - draw_rect.min.x) / scaled.x,
            (panel_rect.min.y - draw_rect.min.y) / scaled.y,
        );
        let uv_max = egui::pos2(
            (panel_rect.max.x - draw_rect.min.x) / scaled.x,
            (panel_rect.max.y - draw_rect.min.y) / scaled.y,
        );

        ui.painter_at(panel_rect).image(
            tex.id(),
            panel_rect,
            egui::Rect::from_min_max(uv_min, uv_max),
            egui::Color32::WHITE,
        );
    }
}

impl eframe::App for CameraApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.no_cursor {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }

        // ── resolve Auto rotation on first frame ──────────────────────────────
        // Only runs once.  Stops the unrotated pipeline first, then starts a
        // new one with the correct videoflip — no stutter.
        if self.resolved_rotation.is_none() {
            let screen = ctx.screen_rect();
            let guard  = self.shared.load();
            if let Some(cam) = guard.as_ref() {
                let resolved = self.rotation.resolve(
                    cam.width, cam.height,
                    screen.width(), screen.height(),
                );
                info!(
                    "Auto rotation resolved to {:?} \
                     (frame {}×{}, screen {}×{})",
                    resolved,
                    cam.width, cam.height,
                    screen.width() as u32, screen.height() as u32,
                );
                self.resolved_rotation = Some(resolved);

                if resolved.videoflip_method().is_some() {
                    // Drop the guard before restarting (avoids holding the
                    // ArcSwap read guard across a blocking stop_and_join).
                    drop(guard);
                    self.restart_with_rotation(resolved);
                }
            }
        }

        // ── upload new frame (only when Arc identity changes) ─────────────────
        {
            let guard = self.shared.load();
            // Clone the Arc cheaply and compare pointer identity.
            // ArcSwap::store() always creates a new Arc, so ptr_eq correctly
            // distinguishes "same frame" from "new frame".
            let current: Arc<Option<CameraFrame>> = Arc::clone(&guard);
            let is_new = self.last_frame_arc.as_ref()
                .map_or(true, |prev| !Arc::ptr_eq(prev, &current));
            if is_new {
                if let Some(cam) = current.as_ref() {
                    self.upload_texture(ctx, cam);
                    self.frame_count += 1;
                    let elapsed = self.last_frame_time.elapsed().as_secs_f32();
                    if elapsed >= 1.0 {
                        self.fps             = self.frame_count as f32 / elapsed;
                        self.frame_count     = 0;
                        self.last_frame_time = Instant::now();
                    }
                }
                self.last_frame_arc = Some(current);
            }
        }

        // ── render ────────────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let panel_rect = ui.max_rect();
                if let Some(tex) = &self.texture {
                    Self::draw_cover(ui, tex, panel_rect);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(
                            egui::Color32::from_gray(50),
                            "Waiting for camera-bridge…",
                        );
                    });
                }

                #[cfg(debug_assertions)]
                {
                    let text = format!(
                        "{:.1} fps  rot={:?}  node={}",
                        self.fps,
                        self.resolved_rotation,
                        &self.pw_node,
                    );
                    ui.painter().text(
                        panel_rect.min + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::monospace(13.0),
                        egui::Color32::from_rgba_premultiplied(255, 220, 0, 200),
                    );
                }
            });

        // Limit repaint rate to ~33fps to avoid spinning all 8 cores.
        // The capture thread runs at 30fps so 33ms here means we never miss
        // a frame but also never busy-loop the GPU thread.
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    let config = Config::from_args()?;

    info!(
        "camera-view  node={}  rotation={:?}",
        config.pw_node, config.rotation
    );

    gstreamer::init()?;

    let shared: SharedFrame = Arc::new(ArcSwap::new(Arc::new(None)));

    // Clone title before the viewport builder borrows config and before the
    // closure moves config — otherwise Rust sees a borrow + move conflict.
    let window_title = config.title.clone();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_fullscreen(true)
            .with_decorations(false)
            .with_transparent(true)
            .with_fullsize_content_view(true)
            .with_title(&window_title)
            .with_min_inner_size([320.0, 240.0]),
        vsync: true,
        ..Default::default()
    };

    let shared_clone = Arc::clone(&shared);
    eframe::run_native(
        &window_title,
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(CameraApp::new(config, shared_clone)))
        }),
    )
    .map_err(|e| anyhow!("eframe error: {e}"))?;

    Ok(())
}
