//! video_viewer — `VideoSnapViewerDialog`.
//!
//! Plays a received video snap. The important thing about it is what it is NOT:
//!
//! ## A video snap is persistent, not view-once
//!
//! `snaps.rs` wipes an image snap's bytes when the viewer closes and fires an
//! expiry the inbox uses to mark the row. A video snap does neither — the file
//! stays in snap storage until it expires on its own schedule, and the viewer
//! offers to export it. That is a deliberate product split, per the Qt header,
//! and it is worth stating loudly because the two dialogs look alike and the
//! wrong one would either destroy a video the user was allowed to keep or keep
//! an image they were promised would vanish.
//!
//! ## Two backends, and the failover rule is the subtle part
//!
//! Qt's media player is tried first: native path, hardware-friendly. But Qt 6
//! against FFmpeg 7/8 has a specific known failure — `sws_getContext` rejects
//! the negative-height flip idiom, and some H.264 streams from this app's own
//! recorder carry rotation metadata that takes exactly that path. The player
//! reports an error and never paints.
//!
//! So the failover condition is not "did it error" but **"did it error before
//! the first frame painted"**. An error after playback has started is a real
//! failure of a working pipeline — a corrupt tail, a disconnected sink — and
//! restarting into a second backend there would drop the user back to the
//! beginning of a video they were halfway through. Before the first frame, the
//! backend never worked at all and there is nothing to lose.
//!
//! ## Capabilities differ, so the controls do
//!
//! The GStreamer fallback runs `gst-launch` as a child process writing JPEG
//! frames to tmpfs, polled at 20Hz. There is no in-process pause and no seek:
//! play restarts the pipeline. Rather than offering a scrubber that silently
//! does nothing, the controls are asked what the backend can do — a dead
//! control is worse than an absent one, because the user concludes the file is
//! broken rather than the player limited.

/// The GStreamer preview poll rate.
pub const POLL_HZ: f32 = 20.0;
/// One poll tick — the worst-case A/V lag on the fallback path. Imperceptible
/// for the 5-30s snaps this app sends, which is why the fallback is acceptable
/// at all.
pub const POLL_TICK_MS: f32 = 1000.0 / POLL_HZ;

/// Which playback path is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// QMediaPlayer + QVideoWidget. Pause and seek available.
    Qt,
    /// gst-launch child process, JPEG frames polled from tmpfs.
    GStreamer,
}

impl Backend {
    /// The fallback has no in-process pause: play restarts the pipeline.
    pub fn can_pause(self) -> bool { matches!(self, Backend::Qt) }
    /// Nor any seek.
    pub fn can_seek(self) -> bool { matches!(self, Backend::Qt) }
    pub fn name(self) -> &'static str {
        match self { Backend::Qt => "Qt media", Backend::GStreamer => "GStreamer" }
    }
}

/// What the viewer is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback { Idle, Playing, Paused, Failed }

/// What the viewer asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEvent {
    /// Export to a path the user picked. The file is COPIED — the original
    /// stays in snap storage.
    Saved(String),
    /// The viewer closed. Deliberately carries no wipe: the snap outlives it.
    Closed,
    /// Both backends failed; there is nothing more to try.
    Unplayable,
}

/// The viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoViewer {
    pub backend: Backend,
    pub state: Playback,
    /// True once any frame has reached the screen. The failover hinges on this.
    first_frame: bool,
    /// Whether the fallback has already been tried, so a second failure is
    /// terminal rather than a loop between two broken backends.
    failed_over: bool,
    /// Playback position and length, in milliseconds.
    pub position_ms: f32,
    pub duration_ms: f32,
}

impl VideoViewer {
    /// Open on the preferred backend.
    pub fn new(duration_ms: f32) -> Self {
        VideoViewer {
            backend: Backend::Qt, state: Playback::Idle, first_frame: false,
            failed_over: false, position_ms: 0.0, duration_ms,
        }
    }

    pub fn play(&mut self) {
        // Restarting the pipeline is what "play" MEANS on the fallback, so a
        // resume there necessarily starts over. Saying so in one place keeps
        // the surprise out of the call sites.
        if self.backend == Backend::GStreamer && self.state == Playback::Paused {
            self.position_ms = 0.0;
        }
        if self.state != Playback::Failed { self.state = Playback::Playing; }
    }

    /// Tap the viewfinder. Only meaningful where the backend can pause.
    pub fn toggle(&mut self) -> bool {
        if !self.backend.can_pause() { return false; }
        match self.state {
            Playback::Playing => { self.state = Playback::Paused; true }
            Playback::Paused | Playback::Idle => { self.play(); true }
            Playback::Failed => false,
        }
    }

    /// Scrub. Refused where the backend cannot seek — a scrubber that silently
    /// does nothing makes the user think the FILE is broken.
    pub fn seek(&mut self, ms: f32) -> bool {
        if !self.backend.can_seek() { return false; }
        self.position_ms = ms.clamp(0.0, self.duration_ms);
        true
    }

    /// A frame reached the screen.
    pub fn note_frame(&mut self) { self.first_frame = true; }
    pub fn has_painted(&self) -> bool { self.first_frame }

    /// The backend reported an error.
    ///
    /// Returns `true` if it failed over. The condition is "before the first
    /// frame", not "at all": an error after playback started is a real failure
    /// of a working pipeline, and restarting into another backend would drop
    /// the user back to the beginning of a video they were halfway through.
    pub fn note_error(&mut self) -> bool {
        if self.first_frame || self.failed_over || self.backend == Backend::GStreamer {
            self.state = Playback::Failed;
            return false;
        }
        self.backend = Backend::GStreamer;
        self.failed_over = true;
        self.state = Playback::Idle;
        // A fresh backend starts from the top; the previous one painted nothing
        // so there is no position to preserve.
        self.position_ms = 0.0;
        true
    }

    pub fn failed(&self) -> bool { self.state == Playback::Failed }

    /// Advance playback.
    pub fn tick(&mut self, dt_ms: f32) {
        if self.state != Playback::Playing { return; }
        self.position_ms = (self.position_ms + dt_ms).min(self.duration_ms);
        if self.position_ms >= self.duration_ms { self.state = Playback::Paused; }
    }

    /// Save a copy. The original stays put — this is an export, not a move.
    pub fn save(&self, dest: &str) -> VideoEvent { VideoEvent::Saved(dest.to_string()) }

    /// Close. Emits no wipe and no expiry, which is the whole difference from
    /// the image viewer.
    pub fn close(&self) -> VideoEvent {
        if self.failed() { VideoEvent::Unplayable } else { VideoEvent::Closed }
    }

    /// The status line, which names the backend when it is the fallback —
    /// otherwise a user reporting "no pause button" has no way to say why.
    pub fn status(&self) -> String {
        match self.state {
            Playback::Failed => "Cannot play this video".into(),
            _ if self.backend == Backend::GStreamer =>
                format!("{} (fallback — no pause or scrub)", self.backend.name()),
            _ => String::new(),
        }
    }

    /// Progress 0..=1.
    pub fn progress(&self) -> f32 {
        if self.duration_ms <= 0.0 { return 0.0; }
        (self.position_ms / self.duration_ms).clamp(0.0, 1.0)
    }
}

/// Draw the viewer. Returns an event when one is produced.
pub fn video_viewer_ui(ui: &mut egui::Ui, v: &mut VideoViewer, dest: &str,
                       c: &uikit_egui::PaletteColors) -> Option<VideoEvent> {
    v.tick(ui.input(|i| i.stable_dt) * 1000.0);
    let mut out = None;
    ui.vertical_centered(|ui| {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().min(320.0), 220.0), egui::Sense::click());
        ui.painter().rect_filled(rect, 6.0, c.bg_raised);
        if v.failed() {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "⚠",
                              egui::FontId::proportional(32.0), c.danger);
        }
        // Tap-to-toggle only where it does something.
        if resp.clicked() { v.toggle(); }

        let s = v.status();
        if !s.is_empty() { ui.colored_label(c.text_weak, s); }

        // The scrubber is ABSENT on the fallback, not present and dead: a dead
        // control makes the user think the file is broken rather than the
        // player limited.
        if v.backend.can_seek() {
            let mut p = v.position_ms;
            if ui.add(egui::Slider::new(&mut p, 0.0..=v.duration_ms.max(1.0))
                .show_value(false)).changed() {
                v.seek(p);
            }
        } else {
            let bar = ui.available_width().min(240.0);
            let (r, _) = ui.allocate_exact_size(egui::vec2(bar, 3.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 1.0, c.bg_raised);
            let mut fill = r;
            fill.set_width(r.width() * v.progress());
            ui.painter().rect_filled(fill, 1.0, c.accent);
        }

        ui.horizontal(|ui| {
            if v.backend.can_pause() {
                let label = if v.state == Playback::Playing { "Pause" } else { "Play" };
                if ui.button(label).clicked() { v.toggle(); }
            } else if ui.button("Play from start").clicked() {
                v.position_ms = 0.0;
                v.play();
            }
            // Save is offered even on the fallback: exporting is a file copy
            // and does not care which decoder failed to show it.
            if ui.button("Save").clicked() { out = Some(v.save(dest)); }
            if ui.button("Close").clicked() { out = Some(v.close()); }
        });
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer() -> VideoViewer { VideoViewer::new(10_000.0) }

    #[test]
    fn a_video_snap_outlives_its_viewer() {
        // THE distinction from snaps.rs, where closing wipes the bytes and
        // fires an expiry. Getting these two dialogs the wrong way round either
        // destroys a video the user was allowed to keep or keeps an image they
        // were promised would vanish.
        let v = viewer();
        assert_eq!(v.close(), VideoEvent::Closed);
        let dbg = format!("{:?}", v.close());
        assert!(!dbg.contains("Expired") && !dbg.contains("Wipe"));
    }

    #[test]
    fn saving_is_an_export_not_a_move() {
        let v = viewer();
        assert_eq!(v.save("/home/ada/Videos/snap.mp4"),
                   VideoEvent::Saved("/home/ada/Videos/snap.mp4".into()));
    }

    #[test]
    fn playback_starts_on_the_qt_backend() {
        let v = viewer();
        assert_eq!(v.backend, Backend::Qt);
        assert!(v.backend.can_pause() && v.backend.can_seek());
    }

    #[test]
    fn an_error_before_the_first_frame_fails_over() {
        // Qt 6 against FFmpeg 7/8 rejects the negative-height flip idiom, and
        // this app's own recorder emits rotation metadata that takes exactly
        // that path — so the player errors and never paints.
        let mut v = viewer();
        assert!(v.note_error());
        assert_eq!(v.backend, Backend::GStreamer);
        assert!(!v.failed(), "there is still a backend to try");
    }

    #[test]
    fn an_error_after_playback_started_does_not_restart_into_another_backend() {
        // A working pipeline that fails partway is a real failure. Failing over
        // there drops the user back to the beginning of a video they were
        // halfway through.
        let mut v = viewer();
        v.play();
        v.note_frame();
        v.tick(4_000.0);
        assert!(!v.note_error());
        assert_eq!(v.backend, Backend::Qt, "stayed put");
        assert!(v.failed());
    }

    #[test]
    fn a_second_failure_is_terminal_rather_than_a_loop() {
        let mut v = viewer();
        assert!(v.note_error());
        assert!(!v.note_error(), "no third backend to try");
        assert!(v.failed());
        assert_eq!(v.close(), VideoEvent::Unplayable);
    }

    #[test]
    fn the_fallback_starts_from_the_top() {
        // The first backend painted nothing, so there is no position to keep.
        let mut v = viewer();
        v.play();
        v.tick(3_000.0);
        assert!(v.position_ms > 0.0);
        v.note_error();
        assert_eq!(v.position_ms, 0.0);
    }

    #[test]
    fn the_fallback_can_neither_pause_nor_seek() {
        assert!(!Backend::GStreamer.can_pause());
        assert!(!Backend::GStreamer.can_seek());
    }

    #[test]
    fn a_tap_does_nothing_on_a_backend_that_cannot_pause() {
        // Reported honestly rather than silently ignored, so the UI can decline
        // to draw a control instead of drawing a dead one.
        let mut v = viewer();
        v.note_error();
        v.play();
        assert!(!v.toggle());
        assert_eq!(v.state, Playback::Playing, "still going");
    }

    #[test]
    fn a_seek_is_refused_on_the_fallback_rather_than_silently_ignored() {
        // A scrubber that does nothing makes the user think the FILE is broken
        // rather than the player limited.
        let mut v = viewer();
        v.note_error();
        assert!(!v.seek(5_000.0));
        assert_eq!(v.position_ms, 0.0);
    }

    #[test]
    fn resuming_on_the_fallback_restarts_because_that_is_what_play_means_there() {
        // gst-launch has no in-process pause; play relaunches the pipeline.
        // Saying so in one place keeps the surprise out of the call sites.
        let mut v = viewer();
        v.note_error();
        v.play();
        v.tick(3_000.0);
        v.state = Playback::Paused;
        v.play();
        assert_eq!(v.position_ms, 0.0);
    }

    #[test]
    fn pausing_on_the_qt_backend_keeps_the_position() {
        let mut v = viewer();
        v.play();
        v.tick(3_000.0);
        assert!(v.toggle());
        assert_eq!(v.state, Playback::Paused);
        assert!((v.position_ms - 3_000.0).abs() < 1.0);
        assert!(v.toggle());
        assert_eq!(v.state, Playback::Playing);
        assert!((v.position_ms - 3_000.0).abs() < 1.0, "resumed where it stopped");
    }

    #[test]
    fn a_seek_is_clamped_to_the_clip() {
        let mut v = viewer();
        assert!(v.seek(99_000.0));
        assert_eq!(v.position_ms, 10_000.0);
        v.seek(-5.0);
        assert_eq!(v.position_ms, 0.0);
    }

    #[test]
    fn playback_stops_at_the_end_rather_than_running_past_it() {
        let mut v = viewer();
        v.play();
        v.tick(99_000.0);
        assert_eq!(v.position_ms, 10_000.0);
        assert_eq!(v.state, Playback::Paused);
        assert_eq!(v.progress(), 1.0);
    }

    #[test]
    fn a_zero_length_clip_does_not_divide_by_zero() {
        let v = VideoViewer::new(0.0);
        assert_eq!(v.progress(), 0.0);
    }

    #[test]
    fn the_status_names_the_fallback_so_a_missing_control_is_explainable() {
        // Otherwise a user reporting "there is no pause button" has no way to
        // say why, and neither does anyone reading the report.
        let mut v = viewer();
        assert_eq!(v.status(), "", "nothing to say on the normal path");
        v.note_error();
        assert!(v.status().contains("fallback"));
        assert!(v.status().contains("no pause"));
    }

    #[test]
    fn a_failed_viewer_says_so_plainly() {
        let mut v = viewer();
        v.note_error();
        v.note_error();
        assert_eq!(v.status(), "Cannot play this video");
    }

    #[test]
    fn the_fallbacks_worst_case_lag_is_one_poll_tick() {
        // 50ms, imperceptible for the 5-30s snaps this app sends — which is
        // the reason a JPEG-polling fallback is acceptable at all.
        assert_eq!(POLL_TICK_MS, 50.0);
        assert!(POLL_TICK_MS < 100.0);
    }
}
