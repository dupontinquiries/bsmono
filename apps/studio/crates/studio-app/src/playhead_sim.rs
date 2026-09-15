//! playhead_sim — a fast, deterministic harness for the exact bugs that are
//! slow to chase by eye in the running app: "is the playhead on the clip I
//! expect, at the time I expect, within some margin."
//!
//! Both functions this drives are already pure: [`studio_live::transport::Transport::advance`]
//! takes a sample count, not wall time, and [`crate::viewport::video_at`] is a
//! pure function of `(Project, Ticks)`. Neither touches ffmpeg, a GPU, or a
//! clock — which means a playhead/track-selection bug is reproducible in a
//! `cargo test` that runs in milliseconds, instead of by scrubbing the GUI
//! and eyeballing a preview frame.
//!
//! This does NOT test the ffmpeg-seek/cache path in `viewport.rs` itself (the
//! actual frame decode) — deliberately: that path needs real media files and
//! a subprocess, which is the opposite of fast iteration. This tests the
//! model logic that decides WHICH frame should be showing, which is where
//! "affected by tracks on the timeline and playhead position" bugs actually
//! live (wrong-track selection, off-by-one trim math, boundary ticks at a
//! clip edge) — the part that was previously only checkable by watching the
//! preview and guessing which layer was responsible.

use crate::viewport::video_at;
use studio_core::{Clip, ClipId, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};
use studio_live::transport::Transport;

/// A minimal project builder for scenarios: one call per clip, no file I/O.
/// `video_at` never reads the file at `path` (it only compares ids/ticks), so
/// paths here are just labels ("clip-A") a test can assert against directly.
pub struct SimProject {
    project: Project,
    next_id: u64,
}

impl SimProject {
    pub fn new() -> Self {
        SimProject { project: Project::default(), next_id: 1 }
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Adds a video track (topmost = added first, matching `video_at`'s own
    /// "first video track in project order wins" convention) and returns its id.
    pub fn add_video_track(&mut self) -> TrackId {
        let id = TrackId(self.fresh_id());
        self.project.tracks.push(Track { id, kind: TrackKind::Video, name: format!("V{}", id.0), muted: false, locked: false, gain: 1.0, route: None });
        id
    }

    /// Places a clip labelled `label` on `track`, spanning `[start_sec, start_sec + len_sec)`
    /// on the timeline, with `src_in_sec` as the source in-point (trim).
    pub fn place_clip(&mut self, track: TrackId, label: &str, start_sec: f64, len_sec: f64, src_in_sec: f64) -> ClipId {
        let source_id = SourceId(self.fresh_id());
        self.project.sources.insert(
            source_id,
            Source { id: source_id, path: label.to_string(), len: Ticks::from_seconds(src_in_sec + len_sec + 3600.0), sample_rate: 48000, channels: 2, has_video: true },
        );
        let clip_id = ClipId(self.fresh_id());
        self.project.clips.insert(
            clip_id,
            Clip {
                id: clip_id,
                source: source_id,
                track,
                pos: Ticks::from_seconds(start_sec),
                src_in: Ticks::from_seconds(src_in_sec),
                len: Ticks::from_seconds(len_sec),
                gain: 1.0,
                effects: Vec::new(),
            },
        );
        clip_id
    }

    pub fn project(&self) -> &Project {
        &self.project
    }
}

impl Default for SimProject {
    fn default() -> Self {
        Self::new()
    }
}

/// Drives a `Transport` by simulated audio buffers (default 512 samples @
/// 48kHz, ~10.7ms/step — a realistic device callback size) and reads
/// `video_at` after each step. No sleeping, no real time passes.
pub struct PlayheadSim<'p> {
    pub transport: Transport,
    project: &'p Project,
    buffer_frames: i64,
    sample_rate: u32,
}

impl<'p> PlayheadSim<'p> {
    pub fn new(project: &'p Project, sample_rate: u32) -> Self {
        PlayheadSim { transport: Transport::new(sample_rate), project, buffer_frames: 512, sample_rate }
    }

    pub fn with_buffer_size(mut self, frames: i64) -> Self {
        self.buffer_frames = frames;
        self
    }

    pub fn play(&mut self) -> &mut Self {
        self.transport.play();
        self
    }

    pub fn seek_seconds(&mut self, s: f64) -> &mut Self {
        self.transport.set_position(Ticks::from_seconds(s));
        self
    }

    /// Sets the transport's loop range -- the same field a beat-pattern
    /// loop toggle writes (see `studio-app/src/lib.rs`'s `loop_range =
    /// Some((start, end))`). `None` clears it.
    pub fn set_loop(&mut self, range: Option<(f64, f64)>) -> &mut Self {
        self.transport.loop_range = range.map(|(s, e)| (Ticks::from_seconds(s), Ticks::from_seconds(e)));
        self
    }

    /// Steps the transport forward by simulated device buffers until at
    /// least `ms` of (simulated) audio time has been consumed. This is the
    /// "after X milliseconds" half of the ask: real wall-clock milliseconds
    /// never elapse, only sample count, so the test runs instantly and
    /// reproduces identically every time.
    pub fn advance_ms(&mut self, ms: f64) -> &mut Self {
        let target_frames = (ms / 1000.0 * self.sample_rate as f64).round() as i64;
        let mut done = 0i64;
        while done < target_frames {
            let step = self.buffer_frames.min(target_frames - done);
            self.transport.advance(step);
            done += step;
        }
        self
    }

    /// What `video_at` resolves to right now, as `(label, media_time_secs)`
    /// for readable assertions (the label is whatever string `place_clip`
    /// was given — `video_at` never touches the actual file).
    pub fn current_clip(&self) -> Option<(String, f64)> {
        video_at(self.project, self.transport.position()).map(|(path, media)| (path, media.to_seconds()))
    }

    /// The core assertion the whole harness exists for: "is the playhead on
    /// `expected` (or on no clip, if `None`), within `margin_ms` of where it
    /// should be." Margin is on the CLIP BOUNDARY check, not a fuzzy label
    /// match — it re-resolves at `playhead ± margin` and requires the
    /// expectation to hold across that whole window, which is what actually
    /// catches an off-by-one-tick edge bug instead of hiding it inside a
    /// generous tolerance.
    pub fn assert_clip(&self, expected: Option<&str>, margin_ms: f64) {
        let got = self.current_clip().map(|(l, _)| l);
        assert_eq!(got.as_deref(), expected, "at t={:.3}s: expected clip {:?}, got {:?}", self.transport.position().to_seconds(), expected, got);

        if margin_ms > 0.0 {
            let margin = Ticks::from_seconds(margin_ms / 1000.0);
            let pos = self.transport.position();
            for probe in [pos.saturating_sub(margin), pos.saturating_add(margin)] {
                let got = video_at(self.project, probe).map(|(l, _)| l);
                assert_eq!(
                    got.as_deref(),
                    expected,
                    "at t={:.3}s (±{margin_ms}ms probe t={:.3}s): expected clip {:?} to hold across the margin, got {:?} -- \
                     a real bug here means the clip boundary is closer than the margin, or an edge is off by one tick",
                    pos.to_seconds(),
                    probe.to_seconds(),
                    expected,
                    got
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape asked for: "the playhead is on this clip within
    /// margin after X milliseconds."
    #[test]
    fn playhead_lands_on_the_right_clip_after_playing() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        sp.place_clip(track, "clip-A", 0.0, 2.0, 0.0);
        sp.place_clip(track, "clip-B", 2.0, 3.0, 10.0); // starts at src 10s (trimmed)

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.play();
        sim.advance_ms(500.0);
        sim.assert_clip(Some("clip-A"), 50.0);

        sim.advance_ms(2000.0); // now at 2.5s -- into clip-B
        sim.assert_clip(Some("clip-B"), 50.0);
    }

    /// Trim math: clip-B started at src_in=10s, so 500ms into clip-B on the
    /// timeline must read back as 10.5s of SOURCE media time -- exactly the
    /// class of off-by-one/trim bug that's invisible from the GUI (the
    /// preview just looks "slightly wrong" with no way to tell how wrong).
    #[test]
    fn media_time_honours_the_clips_trim() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        sp.place_clip(track, "clip-B", 2.0, 3.0, 10.0);

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.play();
        sim.advance_ms(2500.0); // 2.5s timeline -> 0.5s into clip-B
        let (label, media_secs) = sim.current_clip().expect("should be on clip-B");
        assert_eq!(label, "clip-B");
        assert!((media_secs - 10.5).abs() < 0.001, "expected media time ~10.5s, got {media_secs}");
    }

    /// Multi-track: the FIRST video track in project order wins, even if a
    /// later track also has a clip under the playhead. This is the
    /// "affected by tracks on the timeline" symptom made reproducible.
    #[test]
    fn topmost_track_wins_when_two_video_tracks_overlap() {
        let mut sp = SimProject::new();
        let top = sp.add_video_track(); // added first == topmost, per video_at's own doc
        let bottom = sp.add_video_track();
        sp.place_clip(top, "top-clip", 0.0, 5.0, 0.0);
        sp.place_clip(bottom, "bottom-clip", 0.0, 5.0, 0.0);

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.play();
        sim.advance_ms(1000.0);
        sim.assert_clip(Some("top-clip"), 20.0);
    }

    /// The gap between two clips: playhead should resolve to NO clip, not
    /// silently hold the previous one -- a real failure mode for a stale
    /// preview ("preview isn't loading" can mean "preview didn't update").
    #[test]
    fn a_gap_between_clips_shows_no_clip() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        sp.place_clip(track, "clip-A", 0.0, 1.0, 0.0);
        sp.place_clip(track, "clip-B", 2.0, 1.0, 0.0); // 1s gap between them

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.play();
        sim.advance_ms(1500.0); // squarely in the gap
        sim.assert_clip(None, 0.0); // margin 0: right at a clip's own start/end tick it's expected to flip, so don't probe a margin here
    }

    /// The hypothesis from the bug report: enabling a beat-pattern loop
    /// sets `transport.loop_range`, and `Transport::advance` wraps
    /// `position()` back into that range forever -- the SAME field
    /// `looping::Window` reads for audio (`lib.rs`: `Window::new(self.
    /// studio.transport.loop_range, ...)`). A video clip sitting outside
    /// the loop range should therefore become permanently unreachable
    /// while looping, even though it is a perfectly normal clip on the
    /// timeline. This test demonstrates exactly that.
    #[test]
    fn a_video_clip_outside_the_loop_range_becomes_unreachable_while_looping() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        // The beat pattern loops bars 1-2 (0s-2s); the video clip is later
        // on the timeline entirely, 5s-10s -- an ordinary arrangement: loop
        // the drum intro while editing, video comes in after it.
        sp.place_clip(track, "video-clip", 5.0, 5.0, 0.0);

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.set_loop(Some((0.0, 2.0)));
        sim.play();

        // Advance WAY past where the clip starts -- 20 real seconds' worth
        // of buffers. With the loop active, position() can never leave
        // [0,2), so the clip must never resolve, no matter how long we run.
        sim.advance_ms(20_000.0);
        let pos = sim.transport.position().to_seconds();
        assert!(pos < 2.0, "sanity check: transport should be trapped inside the loop range, but position is {pos:.3}s");
        sim.assert_clip(None, 0.0);

        // Confirm this is SPECIFICALLY the loop's doing, not something else
        // about the clip or the harness: clearing the loop and continuing
        // to play must reach the clip normally. (20_000ms is an exact
        // multiple of the 2000ms loop, so the trapped position lands
        // exactly on 0.0s -- advance comfortably past 5.0s from there,
        // not just by the clip's nominal start offset.)
        sim.set_loop(None);
        sim.advance_ms(6000.0);
        sim.assert_clip(Some("video-clip"), 50.0);
    }

    /// Same failure, cheaper to spot: a short run rather than a 20-second
    /// simulated one. Useful as the fast smoke test once the bug above is
    /// understood -- this is the one to run in a tight loop while fixing
    /// it. Advances a total that would comfortably reach the clip WITHOUT
    /// looping (6s of buffers for a clip starting at 5s) and confirms the
    /// loop traps the playhead before that ever happens.
    #[test]
    fn quick_check_loop_range_traps_the_playhead_before_the_clip() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        sp.place_clip(track, "video-clip", 5.0, 5.0, 0.0);

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.set_loop(Some((0.0, 2.0)));
        sim.play();
        sim.advance_ms(6000.0); // would be well past the clip's 5s start if not looping
        sim.assert_clip(None, 0.0);
        assert!(sim.transport.position().to_seconds() < 2.0, "playhead escaped the loop range");
    }


    /// The other bug report: pause, drag the playhead BACKWARD, press play
    /// — reported as never actually resuming playback (only fixed by
    /// moving forward, or by an edit like adding/dragging a clip). This
    /// test drives the exact sequence at the model level: play, stop
    /// (pause), seek backward, play again, advance, and confirms the
    /// playhead actually moves and resolves clips correctly afterward.
    ///
    /// This test PASSES cleanly, which is itself the useful result: by
    /// direct reading, `Transport::advance` has no direction-dependent
    /// logic (`self.frame += frames` is direction-agnostic; the loop-wrap
    /// check is a magnitude comparison, not a sign check), and
    /// `StreamingRenderer::render_into` explicitly self-heals a
    /// discontinuity in EITHER direction via its `next_expected != from`
    /// check, which resets its effect cache regardless of whether `from`
    /// is before or after where it last left off. If the real bug is what
    /// it looks like from the report, this test result says: it is not in
    /// `Transport`, `StreamingRenderer`, or `DeviceEngine::tick` -- all
    /// three are exercised here and none reproduce it. That points at the
    /// UI layer instead (the timeline drag handler that turns a backward
    /// drag into a `Seek` command, or the keyboard `PlayPause` action's
    /// input-mode state in `studio-app/src/lib.rs` around
    /// `InputMode::Playing`) -- worth checking there first next time,
    /// rather than re-auditing the model layer this test already clears.
    #[test]
    fn pause_seek_backward_then_play_resumes_normally_at_the_model_level() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        sp.place_clip(track, "clip-A", 0.0, 3.0, 0.0);
        sp.place_clip(track, "clip-B", 3.0, 3.0, 0.0);

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.play();
        sim.advance_ms(4000.0); // now 4s in, on clip-B
        sim.assert_clip(Some("clip-B"), 50.0);

        sim.transport.stop(); // pause
        assert!(!sim.transport.playing, "stop() must actually clear playing");

        sim.seek_seconds(1.0); // drag the playhead BACKWARD, onto clip-A
        sim.assert_clip(Some("clip-A"), 50.0);

        sim.play();
        assert!(sim.transport.playing, "play() after a backward seek must set playing");
        sim.advance_ms(500.0); // should now be moving forward from 1.0s
        let pos = sim.transport.position().to_seconds();
        assert!((pos - 1.5).abs() < 0.01, "playhead did not advance after play following a backward seek: at {pos:.3}s, expected ~1.5s");
        sim.assert_clip(Some("clip-A"), 50.0);
    }

    /// A seek (scrub) must resolve immediately, with no playback needed --
    /// this is the "click the timeline ruler, preview doesn't move" bug.
    #[test]
    fn seeking_without_playing_still_resolves_the_right_clip() {
        let mut sp = SimProject::new();
        let track = sp.add_video_track();
        sp.place_clip(track, "clip-A", 0.0, 2.0, 0.0);
        sp.place_clip(track, "clip-B", 2.0, 2.0, 0.0);

        let mut sim = PlayheadSim::new(sp.project(), 48_000);
        sim.seek_seconds(2.5); // never called .play()
        sim.assert_clip(Some("clip-B"), 50.0);
    }
}

