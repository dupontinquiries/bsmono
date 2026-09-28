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
        let raw = match self.scrubbing {
            Some(p) => p,
            None => self.reported + self.since,
        };
        // An unknown length (0) does not pin the clock at zero: a player
        // that reports no duration still has a position worth showing.
        if self.duration > 0.0 { raw.clamp(0.0, self.duration) } else { raw.max(0.0) }
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

    /// Forget the remembered pause point — a new track, or a stop.
    pub fn clear_pause(&mut self) { self.paused_at = None; }

    /// A seek while paused: the next resume starts here, and the handle
    /// shows it now.
    pub fn set_paused_at(&mut self, at: f64) {
        let at = if self.duration > 0.0 { at.clamp(0.0, self.duration) } else { at.max(0.0) };
        self.paused_at = Some(at);
        self.reported = at;
        self.since = 0.0;
    }

    /// Where a resume would start, if paused.
    pub fn paused_at(&self) -> Option<f64> { self.paused_at }

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

/// What the bar asked for. Returned rather than applied: the bar owns
/// no queue and no player, and a button that flipped `playing` itself
/// was how the transport became a picture of one — see `Controller::
/// toggle_play` for the pairing of the flag with the request.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportAction {
    Toggle,
    Previous,
    Next,
    Stop,
    /// The user released the scrub handle here, seconds.
    Seek(f64),
    CycleRepeat,
    ToggleShuffle,
}

/// Paint a 5x5 identicon into `rect`, the same cells `Identicon.qml` draws.
///
/// Cover art without files: every scanned library has tracks with no
/// embedded picture, and a blank square beside the title reads as a
/// failed image load. The pattern is a stable function of the title, so
/// a track looks like itself every time it comes around.
pub fn identicon_paint(p: &egui::Painter, rect: egui::Rect, title: &str,
                       colors: &uikit_egui::PaletteColors) {
    let icon = crate::chrome::Identicon::of(title);
    let bg = colors.bg_card;
    p.rect_filled(rect, 4.0, bg);
    let fill = egui::ecolor::Hsva::new(icon.hue / 360.0, 0.55, 0.85, 1.0);
    let cell = rect.width() / 5.0;
    for row in 0..5 {
        for col in 0..5 {
            if icon.cells[row * 5 + col] {
                let r = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + col as f32 * cell, rect.min.y + row as f32 * cell),
                    egui::vec2(cell, cell));
                p.rect_filled(r.shrink(0.5), 0.0, fill);
            }
        }
    }
    // Nothing to key on: an empty title still gets a tile rather than a
    // hole, but a plain one — a face for "nothing" would be a lie.
    if title.trim().is_empty() {
        p.rect_filled(rect, 4.0, bg);
    }
}

/// Draw the bar. Returns what the user asked for, if anything.
///
/// One row: art, title over artist, then the transport buttons and the
/// mode toggles pushed to the right. Below it the scrub bar with the two
/// timecodes. The title appears ONCE — three copies of it stacked in
/// the old strip was the first thing a screenshot showed.
pub fn transport_ui(ui: &mut egui::Ui, t: &mut Transport,
                    colors: &uikit_egui::PaletteColors) -> Option<TransportAction> {
    let mut act = None;
    if t.show_device_strip() {
        ui.horizontal(|ui| {
            for (i, d) in t.devices.clone().iter().enumerate() {
                if ui.selectable_label(t.active == Some(i), d).clicked() { t.select_device(i); }
            }
        });
        ui.separator();
    }
    // Below this width the buttons no longer fit beside the title and
    // take a row of their own — the phone case, which is the deploy
    // target. One threshold, measured on the bar's own width rather than
    // the window's, so a split pane gets the same treatment.
    let compact = ui.available_width() < 560.0;
    let buttons = |ui: &mut egui::Ui, act: &mut Option<TransportAction>, t: &Transport| {
        let big = |s: &str| egui::RichText::new(s).size(18.0);
        if ui.add(egui::Button::new(big("⏹")).min_size(egui::vec2(30.0, 30.0)))
            .on_hover_text("Stop").clicked() { *act = Some(TransportAction::Stop); }
        if ui.add(egui::Button::new(big("⏮")).min_size(egui::vec2(34.0, 30.0)))
            .on_hover_text("Previous").clicked() { *act = Some(TransportAction::Previous); }
        if ui.add(egui::Button::new(big(if t.playing { "⏸" } else { "▶" }))
            .min_size(egui::vec2(44.0, 30.0)))
            .on_hover_text(if t.playing { "Pause" } else { "Play" }).clicked() {
            *act = Some(TransportAction::Toggle);
        }
        if ui.add(egui::Button::new(big("⏭")).min_size(egui::vec2(34.0, 30.0)))
            .on_hover_text("Next").clicked() { *act = Some(TransportAction::Next); }
    };
    let modes = |ui: &mut egui::Ui, act: &mut Option<TransportAction>, t: &Transport, compact: bool| {
        let rep = if compact { match t.repeat { Repeat::Off => "↻", Repeat::All => "↻ all", Repeat::One => "↻ 1" }.to_string() }
                  else { format!("repeat {}", t.repeat.tag()) };
        if ui.selectable_label(t.repeat != Repeat::Off, rep)
            .on_hover_text("repeat: off → all → one").clicked() {
            *act = Some(TransportAction::CycleRepeat);
        }
        if ui.selectable_label(t.shuffle, if compact { "⇄" } else { "shuffle" })
            .on_hover_text("shuffle").clicked() {
            *act = Some(TransportAction::ToggleShuffle);
        }
    };
    ui.horizontal(|ui| {
        let (art, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
        identicon_paint(ui.painter(), art, &t.title, colors);
        ui.add_space(4.0);
        // The title gets what the buttons leave, and is truncated rather
        // than wrapped: a two-line title pushes the scrub bar down and a
        // wrapped artist under it pushes it further.
        let reserve = if compact { 0.0 } else { 330.0 };
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - reserve).max(80.0));
            ui.add(egui::Label::new(egui::RichText::new(
                if t.title.is_empty() { "Nothing playing" } else { &t.title }).strong()).truncate());
            ui.add(egui::Label::new(egui::RichText::new(
                if t.artist.is_empty() { "—" } else { &t.artist }).weak().small()).truncate());
        });
        if !compact {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right-to-left, so the order is spelled backwards here.
                modes(ui, &mut act, t, false);
                ui.add_space(8.0);
                let big = |s: &str| egui::RichText::new(s).size(18.0);
                if ui.add(egui::Button::new(big("⏭")).min_size(egui::vec2(34.0, 30.0)))
                    .on_hover_text("Next").clicked() { act = Some(TransportAction::Next); }
                if ui.add(egui::Button::new(big(if t.playing { "⏸" } else { "▶" }))
                    .min_size(egui::vec2(44.0, 30.0)))
                    .on_hover_text(if t.playing { "Pause" } else { "Play" }).clicked() {
                    act = Some(TransportAction::Toggle);
                }
                if ui.add(egui::Button::new(big("⏮")).min_size(egui::vec2(34.0, 30.0)))
                    .on_hover_text("Previous").clicked() { act = Some(TransportAction::Previous); }
                if ui.add(egui::Button::new(big("⏹")).min_size(egui::vec2(30.0, 30.0)))
                    .on_hover_text("Stop").clicked() { act = Some(TransportAction::Stop); }
            });
        }
    });
    if compact {
        ui.horizontal(|ui| {
            buttons(ui, &mut act, t);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                modes(ui, &mut act, t, true);
            });
        });
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(timecode(t.position())).small().monospace());
        let w = ui.available_width() - 52.0;
        let (rect, resp) = ui.allocate_exact_size(egui::Vec2::new(w.max(40.0), 16.0),
                                                 egui::Sense::click_and_drag());
        let p = ui.painter_at(rect);
        let mid = rect.center().y;
        p.line_segment([egui::Pos2::new(rect.min.x, mid), egui::Pos2::new(rect.max.x, mid)],
                       egui::Stroke::new(3.0_f32, colors.bg_card));
        if t.duration > 0.0 {
            let x = rect.min.x + rect.width() * t.fraction();
            p.line_segment([egui::Pos2::new(rect.min.x, mid), egui::Pos2::new(x, mid)],
                           egui::Stroke::new(3.0_f32, colors.accent));
            p.circle_filled(egui::Pos2::new(x, mid), if t.is_scrubbing() { 7.0 } else { 5.0 }, colors.accent);
        }
        if resp.drag_started() { t.begin_scrub(); }
        if let Some(pos) = resp.interact_pointer_pos() {
            let f = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
            if resp.dragged() { t.scrub_to(f * t.duration); }
            // A tap on the bar is a seek too — the thing every player
            // does, and the only way to seek precisely on a phone.
            if resp.clicked() && t.duration > 0.0 {
                act = Some(TransportAction::Seek(f * t.duration));
            }
        }
        if resp.drag_stopped() {
            if let Some(at) = t.end_scrub() { act = Some(TransportAction::Seek(at)); }
        }
        ui.label(egui::RichText::new(if t.duration > 0.0 { timecode(t.duration) } else { "–:––".into() })
            .small().monospace());
    });
    act
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
    fn an_unknown_length_does_not_pin_the_clock_at_zero() {
        // ffplay reports a position and no duration; the bar has no end
        // to draw but the timecode still counts.
        let mut t = Transport { duration: 0.0, ..playing() };
        t.report(12.0);
        t.tick(3.0);
        assert!((t.position() - 15.0).abs() < 1e-9, "{}", t.position());
        assert_eq!(t.fraction(), 0.0, "no bar without a length");
    }

    #[test]
    fn a_seek_while_paused_becomes_the_resume_point() {
        let mut t = playing();
        t.report(20.0);
        t.pause();
        t.set_paused_at(90.0);
        assert_eq!(t.position(), 90.0, "the handle shows it now");
        assert_eq!(t.resume(), Some(90.0));
        // and clearing forgets it
        let mut u = playing();
        u.report(5.0); u.pause(); u.clear_pause();
        assert_eq!(u.paused_at(), None);
        assert_eq!(u.resume(), None);
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
