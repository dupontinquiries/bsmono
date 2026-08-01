//! transport — the bar that is always on screen, ported from
//! `BottomTransportBar.qml`.
//!
//! bo-suite's port was `lib.rs` and nothing else: a device list with volumes and
//! no way to play anything. The transport bar is the surface a music controller
//! cannot work without, and it carries the state everything else reads.
//!
//! What the QML contracts, and what is mirrored:
//!
//! * **A device strip that only appears with more than one device**
//!   (`showDeviceStrip: queueDevices.length > 1`). A strip listing one speaker
//!   is a row of chrome telling you what you already know.
//! * **Repeat modes cycle off → all → one** (`repeat_mode_tag`), which is the
//!   order every player uses, so getting it wrong is immediately wrong to a user.
//! * **Position and duration** with a seek. The subtlety is that a seek must not
//!   fight the clock: a position that keeps advancing from the network while the
//!   user drags makes the handle jump out from under the finger.
//! * **`estimatedSec`** — the QML extrapolates position between updates, because
//!   a bar that only moves when a packet arrives stutters.

/// Repeat state, in the order every player cycles them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    /// off → all → one → off. The order is the contract; a player that cycles
    /// differently is wrong in a way a user notices in one press.
    pub fn next(self) -> Repeat {
        match self { Repeat::Off => Repeat::All, Repeat::All => Repeat::One, Repeat::One => Repeat::Off }
    }
    pub fn tag(self) -> &'static str {
        match self { Repeat::Off => "off", Repeat::All => "all", Repeat::One => "one" }
    }
}

/// What is playing, and where.
#[derive(Debug, Clone, Default)]
pub struct Transport {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Seconds into the track, as last reported.
    reported: f64,
    /// Seconds of local extrapolation since that report.
    since: f64,
    pub duration: f64,
    /// Set while the user is dragging: the clock must not move the handle out
    /// from under their finger.
    scrubbing: Option<f64>,
    /// Devices in the queue. The strip only shows when there is a choice.
    pub devices: Vec<String>,
    pub active: Option<usize>,
    /// Where playback was when paused.
    ///
    /// For BT and local sinks the QML notes that "pause" is really STOP — the
    /// stream ends — so resuming has to `seek_to` the remembered position.
    /// `Some(0.0)` is meaningful and distinct from `None`: pausing at the very
    /// start must resume at the start, not from wherever the stream happens to
    /// begin. That is why this is an Option rather than a plain f64.
    paused_at: Option<f64>,
}

impl Transport {
    /// `showDeviceStrip: queueDevices.length > 1`. A strip listing one speaker
    /// is chrome telling you what you already know.
    pub fn show_device_strip(&self) -> bool { self.devices.len() > 1 }

    /// A position report arrived from the network.
    pub fn report(&mut self, seconds: f64) {
        self.reported = seconds.max(0.0);
        self.since = 0.0;
    }

    /// Advance the local clock between reports, so the bar moves smoothly
    /// rather than stuttering one packet at a time.
    pub fn tick(&mut self, dt: f64) {
        if self.playing && self.scrubbing.is_none() { self.since += dt.max(0.0); }
    }

    /// Where the handle sits: the scrub position while dragging, the
    /// extrapolated clock otherwise.
    pub fn position(&self) -> f64 {
        match self.scrubbing {
            Some(p) => p.clamp(0.0, self.duration.max(0.0)),
            None => (self.reported + self.since).clamp(0.0, self.duration.max(0.0)),
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 { 0.0 } else { (self.position() / self.duration) as f32 }
    }

    pub fn begin_scrub(&mut self) { self.scrubbing = Some(self.position()); }
    pub fn scrub_to(&mut self, seconds: f64) {
        if self.scrubbing.is_some() { self.scrubbing = Some(seconds.clamp(0.0, self.duration.max(0.0))); }
    }

    /// Release. Returns the seek target, or `None` if we were not scrubbing —
    /// so a stray release cannot seek to zero.
    pub fn end_scrub(&mut self) -> Option<f64> {
        let p = self.scrubbing.take()?;
        self.reported = p;
        self.since = 0.0;
        Some(p)
    }

    pub fn is_scrubbing(&self) -> bool { self.scrubbing.is_some() }

    /// Pause. Remembers the position, because on BT and local sinks the stream
    /// stops rather than pausing.
    pub fn pause(&mut self) {
        if !self.playing { return; }
        self.playing = false;
        self.paused_at = Some(self.position());
    }

    /// Resume. Returns a seek target when one is remembered — including 0.0,
    /// which is a real position and not "nothing to seek to".
    pub fn resume(&mut self) -> Option<f64> {
        self.playing = true;
        let at = self.paused_at.take()?;
        self.reported = at;
        self.since = 0.0;
        Some(at)
    }

    /// Play/pause in one call, returning any seek the resume needs.
    pub fn toggle(&mut self) -> Option<f64> {
        if self.playing { self.pause(); None } else { self.resume() }
    }

    /// Select a device. An out-of-range index is ignored rather than clearing
    /// the selection, since silently switching to nothing looks like a dropout.
    pub fn select_device(&mut self, i: usize) {
        if i < self.devices.len() { self.active = Some(i); }
    }
}

/// `m:ss`, the only format a transport bar should use. Hours appear only when
/// there are hours, because `0:03:12` on a three-minute track is noise.
pub fn timecode(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

/// Draw the bar. Returns a seek target when the user finished scrubbing.
pub fn transport_ui(ui: &mut egui::Ui, t: &mut Transport,
                    colors: &uikit_egui::PaletteColors) -> Option<f64> {
    let mut seek = None;
    if t.show_device_strip() {
        ui.horizontal(|ui| {
            for (i, d) in t.devices.clone().iter().enumerate() {
                if ui.selectable_label(t.active == Some(i), d).clicked() { t.select_device(i); }
            }
        });
        ui.separator();
    }
    ui.horizontal(|ui| {
        if ui.button(if t.playing { "❚❚" } else { "▶" }).clicked() {
            // toggle rather than flipping the flag: on BT and local sinks a
            // resume must seek back to where pause stopped the stream
            if let Some(at) = t.toggle() { seek = Some(at); }
        }
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(if t.title.is_empty() { "—" } else { &t.title }).strong());
            ui.label(egui::RichText::new(&t.artist).weak().small());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(t.repeat != Repeat::Off, t.repeat.tag()).clicked() {
                t.repeat = t.repeat.next();
            }
            if ui.selectable_label(t.shuffle, "shuffle").clicked() { t.shuffle = !t.shuffle; }
        });
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(timecode(t.position())).small());
        let w = ui.available_width() - 46.0;
        let (rect, resp) = ui.allocate_exact_size(egui::Vec2::new(w.max(40.0), 14.0),
                                                 egui::Sense::click_and_drag());
        let p = ui.painter_at(rect);
        let mid = rect.center().y;
        p.line_segment([egui::Pos2::new(rect.min.x, mid), egui::Pos2::new(rect.max.x, mid)],
                       egui::Stroke::new(3.0, colors.bg_card));
        let x = rect.min.x + rect.width() * t.fraction();
        p.line_segment([egui::Pos2::new(rect.min.x, mid), egui::Pos2::new(x, mid)],
                       egui::Stroke::new(3.0, colors.accent));
        p.circle_filled(egui::Pos2::new(x, mid), if t.is_scrubbing() { 7.0 } else { 5.0 }, colors.accent);
        if resp.drag_started() { t.begin_scrub(); }
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.dragged() {
                let f = ((pos.x - rect.min.x) / rect.width().max(1.0)) as f64;
                t.scrub_to(f * t.duration);
            }
        }
        if resp.drag_stopped() { seek = t.end_scrub(); }
        ui.label(egui::RichText::new(timecode(t.duration)).small());
    });
    seek
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> Transport {
        Transport { title: "Track".into(), playing: true, duration: 200.0,
                    devices: vec!["Beosound".into(), "Kitchen".into()], ..Default::default() }
    }

    #[test]
    fn repeat_cycles_off_all_one() {
        // The order every player uses; getting it wrong is wrong to a user in
        // one press.
        assert_eq!(Repeat::default(), Repeat::Off);
        assert_eq!(Repeat::Off.next(), Repeat::All);
        assert_eq!(Repeat::All.next(), Repeat::One);
        assert_eq!(Repeat::One.next(), Repeat::Off);
        assert_eq!(Repeat::All.tag(), "all");
    }

    #[test]
    fn the_device_strip_hides_when_there_is_no_choice() {
        // A strip listing one speaker is chrome telling you what you know.
        let mut t = playing();
        assert!(t.show_device_strip());
        t.devices.truncate(1);
        assert!(!t.show_device_strip());
        t.devices.clear();
        assert!(!t.show_device_strip());
    }

    #[test]
    fn the_position_extrapolates_between_network_reports() {
        // A bar that only moves when a packet arrives stutters.
        let mut t = playing();
        t.report(10.0);
        assert_eq!(t.position(), 10.0);
        t.tick(0.5);
        t.tick(0.5);
        assert!((t.position() - 11.0).abs() < 1e-9, "{}", t.position());
        // and a new report resets the extrapolation rather than adding to it
        t.report(30.0);
        assert!((t.position() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn a_paused_transport_does_not_advance() {
        let mut t = playing();
        t.playing = false;
        t.report(10.0);
        t.tick(5.0);
        assert_eq!(t.position(), 10.0);
    }

    #[test]
    fn the_clock_does_not_fight_the_finger_while_scrubbing() {
        // THE subtlety: a position still advancing from the network makes the
        // handle jump out from under the user's finger mid-drag.
        let mut t = playing();
        t.report(10.0);
        t.begin_scrub();
        t.scrub_to(120.0);
        t.tick(3.0);                       // reports keep arriving
        t.report(13.0);
        assert_eq!(t.position(), 120.0, "the handle stays where it was dragged");
        let seek = t.end_scrub().expect("a seek target");
        assert_eq!(seek, 120.0);
        assert_eq!(t.position(), 120.0, "and the clock resumes from there");
    }

    #[test]
    fn a_release_without_a_drag_does_not_seek_to_zero() {
        // A stray release must not restart the track.
        let mut t = playing();
        t.report(40.0);
        assert_eq!(t.end_scrub(), None);
        assert_eq!(t.position(), 40.0);
    }

    #[test]
    fn the_position_never_leaves_the_track() {
        let mut t = playing();
        t.report(190.0);
        t.tick(60.0);
        assert_eq!(t.position(), 200.0, "clamped at the end");
        assert!((t.fraction() - 1.0).abs() < 1e-6);
        t.begin_scrub();
        t.scrub_to(-50.0);
        assert_eq!(t.position(), 0.0);
        // and a zero-length track does not divide by zero
        let mut z = Transport { duration: 0.0, ..playing() };
        z.report(5.0);
        assert_eq!(z.fraction(), 0.0);
    }

    #[test]
    fn timecode_shows_hours_only_when_there_are_hours() {
        // 0:03:12 on a three-minute track is noise.
        assert_eq!(timecode(0.0), "0:00");
        assert_eq!(timecode(9.0), "0:09");
        assert_eq!(timecode(192.0), "3:12");
        assert_eq!(timecode(3612.0), "1:00:12");
        assert_eq!(timecode(-5.0), "0:00", "a negative clock reads zero, not garbage");
    }

    #[test]
    fn pausing_remembers_the_position_and_resuming_seeks_back_to_it() {
        // On BT and local sinks "pause" is really STOP - the stream ends - so
        // resume has to seek. Without this the track restarts from wherever the
        // sink happens to begin.
        let mut t = playing();
        t.report(42.0);
        t.pause();
        assert!(!t.playing);
        t.tick(10.0);
        assert_eq!(t.position(), 42.0, "a paused clock does not run");
        assert_eq!(t.resume(), Some(42.0));
        assert!(t.playing);
        assert_eq!(t.position(), 42.0);
    }

    #[test]
    fn pausing_at_zero_resumes_at_zero_rather_than_nowhere() {
        // Some(0.0) is a real position. Storing the pause point as a plain
        // number would make "paused at the very start" indistinguishable from
        // "never paused", and the track would resume from the sink's idea of
        // the beginning instead of ours.
        let mut t = playing();
        t.report(0.0);
        t.pause();
        assert_eq!(t.resume(), Some(0.0));
    }

    #[test]
    fn resuming_without_a_pause_does_not_seek() {
        let mut t = playing();
        t.playing = false;
        assert_eq!(t.resume(), None, "nothing was remembered");
        assert!(t.playing);
        // and pausing twice does not overwrite the first position
        let mut u = playing();
        u.report(30.0);
        u.pause();
        u.report(90.0);
        u.pause();
        assert_eq!(u.resume(), Some(30.0), "the first pause wins");
    }

    #[test]
    fn selecting_a_device_that_does_not_exist_is_ignored() {
        // Silently switching to nothing looks like a dropout.
        let mut t = playing();
        t.select_device(1);
        assert_eq!(t.active, Some(1));
        t.select_device(99);
        assert_eq!(t.active, Some(1), "the selection is kept");
    }
}
