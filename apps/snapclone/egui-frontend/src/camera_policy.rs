//! camera_policy — `CameraPolicy` and `CameraHealthService`, ported.
//!
//! The viewfinder was already ported; this is the layer underneath it that
//! decides **which camera to open and how long to wait before giving up**. It
//! is the part that makes a camera "work" on hardware that misbehaves, which is
//! most phone hardware.
//!
//! ## Why the timeouts are a named struct and not constants
//!
//! The Qt header records what they were before: 2000ms `waitForStarted` buried
//! in `GstLaunchBackend::start`, a 5000ms first-frame watchdog, a 2000ms
//! early-exit window, and 480×640 hard-coded into the caps. Three problems came
//! with that layout, and the third is the one that matters here — **test code
//! could not override them.** A probe wants to fail fast; a real capture wants
//! to wait. With the numbers inline, one of those two has to be wrong.
//!
//! So `probe()` returns the same policy with the short timeouts substituted.
//! That is not a convenience: a probe that waits five seconds per device turns
//! "which camera works" into a twenty-second question on a four-camera phone.
//!
//! ## Health is cached with a TTL, and Untested is not Broken
//!
//! Probing a camera costs a session teardown, and the kernel libcamera module
//! typically cannot serve two clients per sensor at once — so probes are serial
//! and results are cached for a minute. Past that they are stale and reprobed.
//!
//! The three-state result is the important part. **`Untested` devices are never
//! returned as the best device**, because "we have not looked" is not evidence
//! of working, and opening an unprobed camera in front of a user who pressed
//! the shutter is how you get a black rectangle. But `Untested` is also not
//! `Broken` — it must not be reported as a failure or hidden from the settings
//! list, because the user may know something the probe does not.
//!
//! ## A successful real capture outranks any probe
//!
//! The Qt comment is explicit: a device that actually delivered frames to a
//! user session is stronger evidence than a probe, and is recorded as `Working`
//! with a synthetic zero time-to-first-frame. A probe is a guess about the
//! future; a capture is a fact about the past.

/// Which way a camera faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing { Rear, Front, Unknown }

/// Timeouts and limits for the capture path.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraPolicy {
    /// How long to wait for the backend to report started.
    pub startup_timeout_ms: u32,
    /// Watchdog on the first frame. A backend can start and never deliver.
    pub first_frame_timeout_ms: u32,
    /// Window in which a failure is treated as "try the alternate node"
    /// rather than as a real failure.
    pub early_exit_window_ms: u32,
    /// Whether to try the alternate node at all.
    pub auto_retry_alternate: bool,

    pub capture_width: u32,
    pub capture_height: u32,
    /// Preview is downscaled: a 1280×720 preview costs battery to draw at a
    /// size no phone screen shows.
    pub preview_max_width: u32,
    pub preview_max_height: u32,
    /// 0 means unthrottled.
    pub preview_max_fps: u32,

    /// Output stride alignment in pixels. 16 rather than 4 because many
    /// GStreamer elements assume more than the format requires, and a wrong
    /// stride shows as a colour tint rather than an error.
    pub stride_align_px: u32,
    /// Quality ratio applied when the source cannot keep up.
    pub low_quality_ratio: f32,
    /// Frame interval ratio past which the stream counts as degraded.
    pub frame_rate_degraded_ratio: f32,
    /// Rotation override, empty means "as the sensor reports".
    pub rotation: String,

    /// Offload colour conversion and scaling to the GPU
    /// (`glupload`/`glcolorconvert`/`glcolorscale`/`gldownload`).
    ///
    /// OFF by default, and the reason is specific rather than cautious: that
    /// pipeline failed to negotiate on the FP5/postmarketOS GL stack during
    /// testing, while the CPU dual-videoconvert path is proven. Enabling it
    /// cannot permanently break the camera — the backend falls back to CPU on
    /// the same device when the GL path will not link — so it is exposed as an
    /// opt-in for hardware where it does work.
    pub prefer_gpu_convert: bool,

    /// Whether two camera streams may run at once (rear viewfinder plus a
    /// front selfie overlay: picture-in-picture, multi-camera).
    ///
    /// OFF by default because single-ISP phones cannot do it, and the failure
    /// is not an error — the second camera simply never starts, which reads as
    /// the feature being broken. Desktops with USB webcams, and phones with
    /// independent sensors, can turn it on.
    pub simultaneous_capture: bool,
}

impl Default for CameraPolicy {
    fn default() -> Self {
        CameraPolicy {
            startup_timeout_ms: 2_000,
            first_frame_timeout_ms: 5_000,
            early_exit_window_ms: 2_000,
            auto_retry_alternate: true,
            capture_width: 1280,
            capture_height: 720,
            preview_max_width: 1024,
            preview_max_height: 576,
            preview_max_fps: 0,
            stride_align_px: 16,
            low_quality_ratio: 0.75,
            frame_rate_degraded_ratio: 1.5,
            rotation: String::new(),
            prefer_gpu_convert: false,
            simultaneous_capture: false,
        }
    }
}

impl CameraPolicy {
    /// The same policy with probe timeouts substituted.
    ///
    /// A probe that waits five seconds per device turns "which camera works"
    /// into a twenty-second question on a four-camera phone. Everything else —
    /// resolutions, stride, rotation — is unchanged, because a probe that
    /// configured the pipeline differently would not be testing the thing the
    /// user is about to open.
    pub fn probe(&self) -> CameraPolicy {
        CameraPolicy {
            startup_timeout_ms: 1_000,
            first_frame_timeout_ms: 1_500,
            early_exit_window_ms: 800,
            ..self.clone()
        }
    }

    /// Preview size for a given sensor size, preserving aspect.
    ///
    /// Never upscales: a 640×480 sensor previewed at 1024 wide is a blurrier
    /// image costing more to draw.
    pub fn preview_size(&self, src_w: u32, src_h: u32) -> (u32, u32) {
        if src_w == 0 || src_h == 0 { return (0, 0); }
        let sx = self.preview_max_width as f32 / src_w as f32;
        let sy = self.preview_max_height as f32 / src_h as f32;
        let s = sx.min(sy).min(1.0);
        (((src_w as f32 * s).round() as u32).max(1),
         ((src_h as f32 * s).round() as u32).max(1))
    }

    /// Round a width up to the stride alignment.
    pub fn aligned_width(&self, w: u32) -> u32 {
        let a = self.stride_align_px.max(1);
        w.div_ceil(a) * a
    }

    /// Is a measured frame interval degraded relative to the target?
    pub fn is_degraded(&self, measured_ms: f32, target_ms: f32) -> bool {
        if target_ms <= 0.0 { return false; }
        measured_ms > target_ms * self.frame_rate_degraded_ratio
    }
}

/// What a probe or a capture concluded about a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Never probed. NOT a failure — see the module docs.
    Untested,
    Working,
    Broken,
}

/// One cached result.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub health: Health,
    /// Milliseconds to the first frame. Zero for a result promoted from a real
    /// capture, which had no probe to time.
    pub time_to_first_frame_ms: u32,
    pub facing: Facing,
    /// When the result was recorded, in the caller's clock.
    pub at_ms: i64,
}

/// How long a result stays fresh.
pub const STALE_AFTER_MS: i64 = 60_000;

/// The cache of device health.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraHealth {
    entries: Vec<(String, Probe)>,
}

impl CameraHealth {
    /// Record a probe result.
    pub fn record(&mut self, id: &str, p: Probe) {
        if let Some(e) = self.entries.iter_mut().find(|(k, _)| k == id) { e.1 = p; }
        else { self.entries.push((id.to_string(), p)); }
    }

    /// Record that a REAL capture succeeded.
    ///
    /// Outranks any probe: a probe is a guess about the future, a capture is a
    /// fact about the past. Recorded with a synthetic zero time-to-first-frame
    /// because there was no probe to time.
    pub fn record_capture_success(&mut self, id: &str, facing: Facing, now_ms: i64) {
        self.record(id, Probe { health: Health::Working, time_to_first_frame_ms: 0,
                                facing, at_ms: now_ms });
    }

    /// The cached result, or `Untested` when there is none.
    pub fn status(&self, id: &str) -> Health {
        self.entries.iter().find(|(k, _)| k == id)
            .map(|(_, p)| p.health).unwrap_or(Health::Untested)
    }

    pub fn get(&self, id: &str) -> Option<&Probe> {
        self.entries.iter().find(|(k, _)| k == id).map(|(_, p)| p)
    }

    /// Is a result old enough to reprobe?
    ///
    /// An absent result is stale by definition: never looked is not fresh.
    pub fn is_stale(&self, id: &str, now_ms: i64) -> bool {
        match self.get(id) {
            None => true,
            Some(p) => now_ms - p.at_ms > STALE_AFTER_MS,
        }
    }

    /// Have we any result at all?
    ///
    /// The caller uses this to choose between asking for a best device and
    /// falling back to plain enumeration — asking an empty cache would always
    /// answer "nothing works".
    pub fn has_any_result(&self) -> bool {
        self.entries.iter().any(|(_, p)| p.health != Health::Untested)
    }

    /// The best working device, preferring a facing.
    ///
    /// `Untested` is NEVER returned: "we have not looked" is not evidence of
    /// working, and opening an unprobed camera for someone who just pressed the
    /// shutter is how you get a black rectangle. Among working devices the
    /// fastest to first frame wins, because that is the one that will feel like
    /// it opened instantly.
    pub fn best_working(&self, preferred: Facing) -> Option<&str> {
        let pick = |f: Option<Facing>| {
            self.entries.iter()
                .filter(|(_, p)| p.health == Health::Working)
                .filter(|(_, p)| f.map_or(true, |f| p.facing == f))
                .min_by_key(|(_, p)| p.time_to_first_frame_ms)
                .map(|(k, _)| k.as_str())
        };
        // Preferred facing first, then anything working. A user who asked for
        // the rear camera and has only a front one should still get a picture.
        pick(Some(preferred)).or_else(|| pick(None))
    }

    /// Devices needing a reprobe.
    pub fn stale_ids(&self, known: &[String], now_ms: i64) -> Vec<String> {
        known.iter().filter(|id| self.is_stale(id, now_ms)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(h: Health, ttff: u32, f: Facing, at: i64) -> Probe {
        Probe { health: h, time_to_first_frame_ms: ttff, facing: f, at_ms: at }
    }

    #[test]
    fn a_probe_uses_short_timeouts_and_the_same_pipeline() {
        // A probe that waits five seconds per device turns "which camera works"
        // into a twenty-second question on a four-camera phone. But it must
        // configure the pipeline identically, or it is not testing what the
        // user is about to open.
        let p = CameraPolicy::default();
        let q = p.probe();
        assert!(q.first_frame_timeout_ms < p.first_frame_timeout_ms);
        assert!(q.startup_timeout_ms < p.startup_timeout_ms);
        assert_eq!(q.capture_width, p.capture_width);
        assert_eq!(q.stride_align_px, p.stride_align_px);
        assert_eq!(q.rotation, p.rotation);
    }

    #[test]
    fn the_preview_is_downscaled_but_never_upscaled() {
        // A 640x480 sensor previewed at 1024 wide is a blurrier image that
        // costs more to draw.
        let p = CameraPolicy::default();
        assert_eq!(p.preview_size(1280, 720), (1024, 576));
        assert_eq!(p.preview_size(640, 480), (640, 480), "left alone");
    }

    #[test]
    fn the_preview_keeps_the_sensor_aspect() {
        // Stretching a 4:3 sensor into a 16:9 preview shows the user a lie
        // about what the photo will look like.
        let p = CameraPolicy::default();
        let (w, h) = p.preview_size(4000, 3000);
        assert!(((w as f32 / h as f32) - 4.0 / 3.0).abs() < 0.01, "got {w}x{h}");
    }

    #[test]
    fn a_zero_sized_sensor_does_not_divide_by_zero() {
        assert_eq!(CameraPolicy::default().preview_size(0, 0), (0, 0));
    }

    #[test]
    fn stride_alignment_rounds_up_to_sixteen_pixels() {
        // 16 rather than the format's 4, because many GStreamer elements assume
        // more than the format requires — and a wrong stride shows as a colour
        // TINT rather than an error, which is the hardest kind to attribute.
        let p = CameraPolicy::default();
        assert_eq!(p.aligned_width(1280), 1280);
        assert_eq!(p.aligned_width(1281), 1296);
        assert_eq!(p.aligned_width(1), 16);
    }

    #[test]
    fn a_slow_stream_is_degraded_only_past_the_ratio() {
        // Some jitter is normal; declaring degradation on every late frame
        // would have the quality dropping constantly.
        let p = CameraPolicy::default();
        assert!(!p.is_degraded(33.0, 33.0));
        assert!(!p.is_degraded(45.0, 33.0), "within 1.5x");
        assert!(p.is_degraded(60.0, 33.0));
        assert!(!p.is_degraded(60.0, 0.0), "no target means no judgement");
    }

    #[test]
    fn an_unknown_device_is_untested_not_broken() {
        // "We have not looked" must not be reported as a failure or hide the
        // device from settings — the user may know something the probe does not.
        let h = CameraHealth::default();
        assert_eq!(h.status("cam0"), Health::Untested);
        assert_ne!(h.status("cam0"), Health::Broken);
    }

    #[test]
    fn an_untested_device_is_never_offered_as_the_best() {
        // Opening an unprobed camera for someone who just pressed the shutter
        // is how you get a black rectangle.
        let mut h = CameraHealth::default();
        h.record("cam0", probe(Health::Untested, 0, Facing::Rear, 0));
        assert_eq!(h.best_working(Facing::Rear), None);
        assert!(!h.has_any_result());
    }

    #[test]
    fn the_fastest_working_device_wins() {
        // It is the one that will feel like it opened instantly.
        let mut h = CameraHealth::default();
        h.record("slow", probe(Health::Working, 900, Facing::Rear, 0));
        h.record("fast", probe(Health::Working, 120, Facing::Rear, 0));
        assert_eq!(h.best_working(Facing::Rear), Some("fast"));
    }

    #[test]
    fn a_broken_device_is_never_offered() {
        let mut h = CameraHealth::default();
        h.record("broken", probe(Health::Broken, 10, Facing::Rear, 0));
        assert_eq!(h.best_working(Facing::Rear), None);
        assert!(h.has_any_result(), "but we DID look, which is different");
    }

    #[test]
    fn the_preferred_facing_wins_but_does_not_veto() {
        // Someone who asked for the rear camera and has only a working front
        // one should still get a picture.
        let mut h = CameraHealth::default();
        h.record("front", probe(Health::Working, 200, Facing::Front, 0));
        assert_eq!(h.best_working(Facing::Rear), Some("front"));

        h.record("rear", probe(Health::Working, 800, Facing::Rear, 0));
        assert_eq!(h.best_working(Facing::Rear), Some("rear"),
                   "even though the front camera is faster");
    }

    #[test]
    fn a_real_capture_outranks_a_probe() {
        // A probe is a guess about the future; a capture is a fact about the
        // past.
        let mut h = CameraHealth::default();
        h.record("cam0", probe(Health::Broken, 0, Facing::Rear, 0));
        h.record_capture_success("cam0", Facing::Rear, 1_000);
        assert_eq!(h.status("cam0"), Health::Working);
        assert_eq!(h.get("cam0").unwrap().time_to_first_frame_ms, 0,
                   "synthetic: there was no probe to time");
    }

    #[test]
    fn results_go_stale_so_a_camera_fixed_by_a_reboot_is_reconsidered() {
        let mut h = CameraHealth::default();
        h.record("cam0", probe(Health::Broken, 0, Facing::Rear, 0));
        assert!(!h.is_stale("cam0", STALE_AFTER_MS));
        assert!(h.is_stale("cam0", STALE_AFTER_MS + 1));
    }

    #[test]
    fn a_device_never_probed_is_stale_by_definition() {
        // Never looked is not fresh, so it is queued for probing rather than
        // silently skipped.
        let h = CameraHealth::default();
        assert!(h.is_stale("cam0", 0));
        assert_eq!(h.stale_ids(&["cam0".into(), "cam1".into()], 0).len(), 2);
    }

    #[test]
    fn recording_the_same_device_twice_replaces_rather_than_appends() {
        // Otherwise the cache grows without bound and the oldest answer might
        // be the one found first.
        let mut h = CameraHealth::default();
        h.record("cam0", probe(Health::Broken, 0, Facing::Rear, 0));
        h.record("cam0", probe(Health::Working, 50, Facing::Rear, 10));
        assert_eq!(h.status("cam0"), Health::Working);
        assert_eq!(h.stale_ids(&["cam0".into()], 0).len(), 0);
    }
}
