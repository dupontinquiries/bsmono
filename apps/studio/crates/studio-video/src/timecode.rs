//! timecode — SMPTE timecode, including the drop-frame arithmetic that makes 29.97 and
//! 59.94 work.
//!
//! Studio could already print a wall clock (`studio_egui::format_timecode`, `H:MM:SS.mmm`).
//! That is a duration, not a timecode, and on a film set they are different objects: a
//! timecode names a FRAME, it is what a camera stamps, what a slate displays, what a sound
//! recordist jams, and what every note on a shot list refers to. `01:00:00:12` is not a
//! length of time — it is a frame you can cut on.
//!
//! ## Drop-frame is the whole difficulty, and it is not what its name suggests
//!
//! NTSC video runs at 30/1.001 ≈ 29.97 frames a second, so an hour of video contains 107,892
//! frames rather than 108,000. Counting them at 30 a second drifts 3.6 seconds slow per
//! hour — which for a broadcaster filling an exact slot is a real problem.
//!
//! Drop-frame fixes the LABELS, not the frames. **No picture is ever discarded.** Two frame
//! NUMBERS are skipped at the start of every minute except every tenth minute: after
//! `00:00:59;29` comes `00:01:00;02`. Nine minutes in ten drop two labels; 9 × 2 × 6 = 108
//! labels an hour, exactly the drift. So `01:00:00;00` is frame 107,892 and lands on the
//! real one-hour mark.
//!
//! Two consequences this module makes explicit rather than leaving to a caller:
//!
//! * **Some timecodes do not exist.** `00:01:00;00` and `00:01:00;01` are not frames. A
//!   parser that accepts them silently produces an off-by-two cut. [`Timecode::valid`] says
//!   so, and [`parse`] refuses them.
//! * **The separator carries meaning.** By convention a semicolon before the frames means
//!   drop-frame, a colon means non-drop. `01:00:00:00` and `01:00:00;00` are two different
//!   frames, 108 apart, and printing both with a colon is how that difference gets lost.
//!
//! ## 23.976 does not drop
//!
//! Only 29.97 and 59.94 have drop-frame variants. 23.976 is also a 1.001 pulldown and also
//! drifts, and there is no drop-frame standard for it — cameras stamp it non-drop and the
//! drift is accepted. Asking for drop-frame at 24 is refused rather than approximated.
//!
//! Pure: no ffmpeg, no clock. [`Rate::nearest`] maps what `ffprobe` reports onto a rate.

use studio_core::Ticks;

/// A frame rate, as film and video actually use them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rate {
    /// 24/1.001 — film transferred for NTSC. Non-drop only.
    Fps23_976,
    Fps24,
    Fps25,
    /// 30/1.001 — NTSC. Drop-frame capable.
    Fps29_97,
    Fps30,
    Fps50,
    /// 60/1.001. Drop-frame capable.
    Fps59_94,
    Fps60,
}

impl Rate {
    /// The frames-per-second COUNT used for labelling: 30 for 29.97, 24 for 23.976. This is
    /// what the frames field counts up to, and it is an integer for every rate in use —
    /// which is why timecode works at all.
    pub fn nominal(self) -> u32 {
        match self {
            Rate::Fps23_976 | Rate::Fps24 => 24,
            Rate::Fps25 => 25,
            Rate::Fps29_97 | Rate::Fps30 => 30,
            Rate::Fps50 => 50,
            Rate::Fps59_94 | Rate::Fps60 => 60,
        }
    }

    /// The real rate, in frames per second.
    pub fn exact(self) -> f64 {
        match self {
            Rate::Fps23_976 => 24_000.0 / 1001.0,
            Rate::Fps24 => 24.0,
            Rate::Fps25 => 25.0,
            Rate::Fps29_97 => 30_000.0 / 1001.0,
            Rate::Fps30 => 30.0,
            Rate::Fps50 => 50.0,
            Rate::Fps59_94 => 60_000.0 / 1001.0,
            Rate::Fps60 => 60.0,
        }
    }

    /// Is this one of the two rates with a drop-frame standard?
    pub fn can_drop(self) -> bool {
        matches!(self, Rate::Fps29_97 | Rate::Fps59_94)
    }

    /// How many frame numbers are skipped per dropping minute: 2 at 29.97, 4 at 59.94.
    pub fn dropped_per_minute(self) -> u32 {
        match self {
            Rate::Fps29_97 => 2,
            Rate::Fps59_94 => 4,
            _ => 0,
        }
    }

    /// The rate closest to a measured frame rate, which is what `ffprobe` gives us.
    ///
    /// A tolerance rather than equality: `30000/1001` comes back through a float as
    /// `29.969999…`, and a container muxed by hand can carry a rounded `29.97`.
    ///
    /// CLOSEST within the window, not the first inside it — and the window is 0.02, not
    /// something roomier. The pairs this has to separate are 0.024 and 0.03 apart
    /// (23.976 vs 24, 29.97 vs 30), so a wide window plus a first-match scan labels every
    /// 24 fps file as 23.976 and, worse, marks a true 30 fps file drop-frame capable —
    /// which would put a `;` in its timecode and shift every label. Caught by the test
    /// that probes a 24.0 file.
    pub fn nearest(fps: f64) -> Option<Rate> {
        const ALL: [Rate; 8] = [
            Rate::Fps23_976,
            Rate::Fps24,
            Rate::Fps25,
            Rate::Fps29_97,
            Rate::Fps30,
            Rate::Fps50,
            Rate::Fps59_94,
            Rate::Fps60,
        ];
        ALL.into_iter()
            .map(|r| (r, (r.exact() - fps).abs()))
            .filter(|(_, d)| *d < 0.02)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(r, _)| r)
    }

    /// `23.976`, `29.97 DF`, and so on — for a UI that has to say which it is.
    pub fn label(self, drop: bool) -> String {
        let n = match self {
            Rate::Fps23_976 => "23.976",
            Rate::Fps24 => "24",
            Rate::Fps25 => "25",
            Rate::Fps29_97 => "29.97",
            Rate::Fps30 => "30",
            Rate::Fps50 => "50",
            Rate::Fps59_94 => "59.94",
            Rate::Fps60 => "60",
        };
        if drop { format!("{n} DF") } else { n.to_string() }
    }
}

/// A frame, named the way a slate names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    pub rate: Rate,
    /// Drop-frame labelling. Only meaningful at 29.97 and 59.94.
    pub drop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimecodeError {
    /// Not `HH:MM:SS:FF` / `HH:MM:SS;FF`.
    Malformed(String),
    /// A field out of range for the rate — frames ≥ nominal, minutes ≥ 60, and so on.
    OutOfRange(String),
    /// A drop-frame label that names no frame: `00:01:00;00` and `;01` do not exist.
    DroppedLabel(String),
    /// Drop-frame asked for at a rate that has no drop-frame standard.
    NoDropAtThisRate(Rate),
}

impl std::fmt::Display for TimecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimecodeError::Malformed(s) => write!(f, "not a timecode: {s:?}"),
            TimecodeError::OutOfRange(s) => write!(f, "out of range: {s}"),
            TimecodeError::DroppedLabel(s) => {
                write!(f, "{s} is a dropped label — no such frame exists in drop-frame")
            }
            TimecodeError::NoDropAtThisRate(r) => {
                write!(f, "{} has no drop-frame standard", r.label(false))
            }
        }
    }
}

impl std::error::Error for TimecodeError {}

impl Timecode {
    pub fn new(hours: u32, minutes: u32, seconds: u32, frames: u32, rate: Rate, drop: bool) -> Timecode {
        Timecode { hours, minutes, seconds, frames, rate, drop }
    }

    /// Is this label a frame that exists?
    ///
    /// The drop-frame half is the one that matters: at the start of every minute that is not
    /// a multiple of ten, the first two (or four) frame numbers are skipped.
    pub fn valid(&self) -> bool {
        if self.frames >= self.rate.nominal() || self.seconds > 59 || self.minutes > 59 {
            return false;
        }
        if self.drop && !self.rate.can_drop() {
            return false;
        }
        if self.drop && self.seconds == 0 && self.minutes % 10 != 0 {
            return self.frames >= self.rate.dropped_per_minute();
        }
        true
    }

    /// The frame number counted from `00:00:00:00`.
    pub fn to_frames(&self) -> i64 {
        let n = self.rate.nominal() as i64;
        let total_minutes = self.hours as i64 * 60 + self.minutes as i64;
        let plain = ((total_minutes * 60) + self.seconds as i64) * n + self.frames as i64;
        if !self.drop {
            return plain;
        }
        // Subtract the labels that were skipped: `dropped` per minute, for every minute so
        // far except the tenths.
        let d = self.rate.dropped_per_minute() as i64;
        plain - d * (total_minutes - total_minutes / 10)
    }

    /// The label for a frame number.
    pub fn from_frames(mut frame: i64, rate: Rate, drop: bool) -> Timecode {
        let n = rate.nominal() as i64;
        if drop && rate.can_drop() {
            // Add back the skipped labels, ten-minute block by ten-minute block, so the
            // remainder can be converted as if nothing had been dropped. The standard
            // algorithm; the `m > d` guard is what keeps the first frames of a ten-minute
            // block — which are NOT dropped — from being pushed forward.
            let d = rate.dropped_per_minute();
            let per_10min = (rate.exact() * 600.0).round() as i64;
            let per_min = n * 60 - d as i64;
            let blocks = frame / per_10min;
            let rem = frame % per_10min;
            frame += d as i64 * 9 * blocks;
            if rem > d as i64 {
                frame += d as i64 * ((rem - d as i64) / per_min);
            }
        }
        let frames = (frame % n) as u32;
        let total_seconds = frame / n;
        Timecode {
            hours: (total_seconds / 3600) as u32,
            minutes: ((total_seconds / 60) % 60) as u32,
            seconds: (total_seconds % 60) as u32,
            frames,
            rate,
            drop: drop && rate.can_drop(),
        }
    }

    /// The timecode at a position on Studio's own timeline.
    ///
    /// Floors rather than rounds: a position 0.9 of a frame past `:12` is still showing
    /// frame 12 on screen, and a timecode that rounds up names a frame the viewer cannot
    /// see yet.
    pub fn from_ticks(t: Ticks, rate: Rate, drop: bool) -> Timecode {
        let secs = t.0 as f64 / studio_core::TICKS_PER_SECOND as f64;
        let frame = (secs * rate.exact()).floor() as i64;
        Timecode::from_frames(frame.max(0), rate, drop)
    }

    /// Where this frame starts on the timeline.
    pub fn to_ticks(&self) -> Ticks {
        let secs = self.to_frames() as f64 / self.rate.exact();
        Ticks((secs * studio_core::TICKS_PER_SECOND as f64).round() as i64)
    }

    /// `01:00:00:12`, or `01:00:00;12` for drop-frame. The separator is the only thing
    /// distinguishing two timecodes 108 frames apart, so it is not cosmetic.
    pub fn format(&self) -> String {
        let sep = if self.drop { ';' } else { ':' };
        format!(
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, sep, self.frames
        )
    }
}

impl std::fmt::Display for Timecode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.format())
    }
}

/// Parse `HH:MM:SS:FF` or `HH:MM:SS;FF`.
///
/// The separator before the frames decides drop-frame, which is the convention every NLE
/// and every camera uses. A `.` is accepted there too, because some tools write it that way
/// for non-drop.
pub fn parse(s: &str, rate: Rate) -> Result<Timecode, TimecodeError> {
    let t = s.trim();
    let bytes = t.as_bytes();
    // Find the last separator: everything before it is H:M:S, after it is frames.
    let cut = t
        .rfind([':', ';', '.'])
        .ok_or_else(|| TimecodeError::Malformed(t.to_string()))?;
    let drop = bytes[cut] == b';';
    if drop && !rate.can_drop() {
        return Err(TimecodeError::NoDropAtThisRate(rate));
    }
    let head: Vec<&str> = t[..cut].split(':').collect();
    if head.len() != 3 {
        return Err(TimecodeError::Malformed(t.to_string()));
    }
    let num = |x: &str| -> Result<u32, TimecodeError> {
        x.trim()
            .parse::<u32>()
            .map_err(|_| TimecodeError::Malformed(t.to_string()))
    };
    let tc = Timecode {
        hours: num(head[0])?,
        minutes: num(head[1])?,
        seconds: num(head[2])?,
        frames: num(&t[cut + 1..])?,
        rate,
        drop,
    };
    if tc.frames >= rate.nominal() || tc.seconds > 59 || tc.minutes > 59 {
        return Err(TimecodeError::OutOfRange(format!(
            "{} at {}",
            tc.format(),
            rate.label(drop)
        )));
    }
    if !tc.valid() {
        return Err(TimecodeError::DroppedLabel(tc.format()));
    }
    Ok(tc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_hour_of_non_drop_is_a_round_number_of_frames() {
        for (rate, per_hour) in [
            (Rate::Fps24, 86_400),
            (Rate::Fps25, 90_000),
            (Rate::Fps30, 108_000),
            (Rate::Fps60, 216_000),
        ] {
            let tc = Timecode::new(1, 0, 0, 0, rate, false);
            assert_eq!(tc.to_frames(), per_hour, "{}", rate.label(false));
            assert_eq!(Timecode::from_frames(per_hour, rate, false), tc);
        }
    }

    /// The number the whole standard exists for: an hour of 29.97 is 107,892 frames, not
    /// 108,000, and drop-frame labelling is what makes `01:00:00;00` land on it.
    #[test]
    fn an_hour_of_drop_frame_is_107892_frames() {
        let tc = Timecode::new(1, 0, 0, 0, Rate::Fps29_97, true);
        assert_eq!(tc.to_frames(), 107_892);
        assert_eq!(Timecode::from_frames(107_892, Rate::Fps29_97, true), tc);
        // and it really is an hour of wall clock — ALMOST. 107,892 frames is 3599.9964 s:
        // drop-frame is accurate to 3.6 MILLISECONDS an hour, not exactly. Dropping 108
        // labels an hour corrects 3.6 s of drift and leaves a thousandth of that behind,
        // because 30/1.001 is not quite 29.97. Worth pinning as the real number rather
        // than asserting an exactness the standard does not have — over a day it is 86 ms,
        // which is why long-running broadcast plant jams to house sync and does not
        // accumulate timecode.
        let secs = 107_892.0 / Rate::Fps29_97.exact();
        assert!((secs - 3600.0).abs() < 0.004, "{secs}");
        assert!((secs - 3600.0).abs() > 0.003, "if this got exact, the arithmetic changed: {secs}");
        // non-drop labelling of the same frame is 3.6 seconds behind
        let nd = Timecode::from_frames(107_892, Rate::Fps29_97, false);
        assert_eq!(nd.format(), "00:59:56:12");
        // 59.94 drops four a minute; an hour is twice 107,892
        assert_eq!(Timecode::new(1, 0, 0, 0, Rate::Fps59_94, true).to_frames(), 215_784);
    }

    /// After `00:00:59;29` comes `00:01:00;02` — the two labels in between name no frame.
    #[test]
    fn the_minute_boundary_skips_two_labels_but_no_pictures() {
        let last = Timecode::new(0, 0, 59, 29, Rate::Fps29_97, true);
        let n = last.to_frames();
        assert_eq!(Timecode::from_frames(n + 1, Rate::Fps29_97, true).format(), "00:01:00;02");
        // consecutive frames: no picture was skipped, only labels
        assert_eq!(Timecode::from_frames(n, Rate::Fps29_97, true).format(), "00:00:59;29");
        // the tenth minute drops nothing
        let before_ten = Timecode::new(0, 9, 59, 29, Rate::Fps29_97, true);
        assert_eq!(
            Timecode::from_frames(before_ten.to_frames() + 1, Rate::Fps29_97, true).format(),
            "00:10:00;00"
        );
        // ten minutes is 17,982 frames
        assert_eq!(Timecode::new(0, 10, 0, 0, Rate::Fps29_97, true).to_frames(), 17_982);
    }

    /// A parser that accepts a dropped label produces a cut two frames off, silently.
    #[test]
    fn a_label_that_names_no_frame_is_refused() {
        assert!(matches!(
            parse("00:01:00;00", Rate::Fps29_97),
            Err(TimecodeError::DroppedLabel(_))
        ));
        assert!(matches!(
            parse("00:01:00;01", Rate::Fps29_97),
            Err(TimecodeError::DroppedLabel(_))
        ));
        assert!(parse("00:01:00;02", Rate::Fps29_97).is_ok(), "the first frame that exists");
        assert!(parse("00:10:00;00", Rate::Fps29_97).is_ok(), "the tenth minute drops nothing");
        // the same labels are fine non-drop, because nothing is skipped there
        assert!(parse("00:01:00:00", Rate::Fps29_97).is_ok());
        assert!(!Timecode::new(0, 1, 0, 0, Rate::Fps29_97, true).valid());
        assert!(Timecode::new(0, 1, 0, 0, Rate::Fps29_97, false).valid());
    }

    #[test]
    fn the_separator_is_the_difference_between_two_frames_108_apart() {
        let df = parse("01:00:00;00", Rate::Fps29_97).unwrap();
        let nd = parse("01:00:00:00", Rate::Fps29_97).unwrap();
        assert!(df.drop && !nd.drop);
        assert_eq!(nd.to_frames() - df.to_frames(), 108);
        assert_eq!(df.format(), "01:00:00;00");
        assert_eq!(nd.format(), "01:00:00:00");
        // and a drop-frame request at a rate with no such standard is refused, not rounded
        assert_eq!(
            parse("01:00:00;00", Rate::Fps24),
            Err(TimecodeError::NoDropAtThisRate(Rate::Fps24))
        );
        assert!(!Timecode::new(0, 0, 0, 0, Rate::Fps24, true).valid());
    }

    #[test]
    fn junk_is_refused_by_shape_and_by_range() {
        assert!(matches!(parse("", Rate::Fps25), Err(TimecodeError::Malformed(_))));
        assert!(matches!(parse("01:00:00", Rate::Fps25), Err(TimecodeError::Malformed(_))));
        assert!(matches!(parse("1:2:3:4:5", Rate::Fps25), Err(TimecodeError::Malformed(_))));
        assert!(matches!(parse("aa:bb:cc:dd", Rate::Fps25), Err(TimecodeError::Malformed(_))));
        // frame 25 does not exist at 25 fps — it is the 26th
        assert!(matches!(parse("00:00:00:25", Rate::Fps25), Err(TimecodeError::OutOfRange(_))));
        assert!(parse("00:00:00:24", Rate::Fps25).is_ok());
        assert!(matches!(parse("00:60:00:00", Rate::Fps25), Err(TimecodeError::OutOfRange(_))));
    }

    /// Every frame of the first twelve minutes at 29.97 DF round-trips, which is what
    /// catches an off-by-one at a block boundary rather than at a hand-picked sample.
    #[test]
    fn every_frame_round_trips_across_the_boundaries() {
        for rate in [Rate::Fps29_97, Rate::Fps59_94] {
            let n = (rate.exact() * 720.0) as i64; // twelve minutes
            for f in 0..n {
                let tc = Timecode::from_frames(f, rate, true);
                assert!(tc.valid(), "{f} produced an invalid label {}", tc.format());
                assert_eq!(tc.to_frames(), f, "{} did not round-trip", tc.format());
                assert_eq!(parse(&tc.format(), rate).unwrap(), tc);
            }
        }
        for rate in [Rate::Fps24, Rate::Fps25, Rate::Fps30] {
            for f in (0..rate.nominal() as i64 * 400).step_by(7) {
                let tc = Timecode::from_frames(f, rate, false);
                assert_eq!(tc.to_frames(), f);
            }
        }
    }

    #[test]
    fn ticks_land_on_the_frame_being_shown_not_the_next_one() {
        let rate = Rate::Fps25;
        // exactly frame 12
        let t = Ticks((12.0 / 25.0 * studio_core::TICKS_PER_SECOND as f64) as i64);
        assert_eq!(Timecode::from_ticks(t, rate, false).frames, 12);
        // nine tenths of the way through frame 12: still frame 12 on screen
        let late = Ticks(t.0 + (0.9 / 25.0 * studio_core::TICKS_PER_SECOND as f64) as i64);
        assert_eq!(Timecode::from_ticks(late, rate, false).frames, 12);
        // a negative position clamps to the start rather than wrapping
        assert_eq!(Timecode::from_ticks(Ticks(-5000), rate, false).to_frames(), 0);
        // and back again
        let tc = Timecode::new(0, 1, 2, 3, rate, false);
        assert_eq!(Timecode::from_ticks(tc.to_ticks(), rate, false), tc);
    }

    #[test]
    fn a_measured_frame_rate_maps_onto_a_named_one() {
        assert_eq!(Rate::nearest(30000.0 / 1001.0), Some(Rate::Fps29_97));
        assert_eq!(Rate::nearest(29.97), Some(Rate::Fps29_97));
        assert_eq!(Rate::nearest(23.976), Some(Rate::Fps23_976));
        // the pairs 0.024 and 0.03 apart must not collapse into each other: a 24 read as
        // 23.976 mislabels every frame, and a 30 read as 29.97 makes it drop-frame capable
        assert_eq!(Rate::nearest(24.0), Some(Rate::Fps24));
        assert_eq!(Rate::nearest(30.0), Some(Rate::Fps30));
        assert_eq!(Rate::nearest(60.0), Some(Rate::Fps60));
        assert!(!Rate::nearest(30.0).unwrap().can_drop());
        assert_eq!(Rate::nearest(25.0), Some(Rate::Fps25));
        assert_eq!(Rate::nearest(59.94), Some(Rate::Fps59_94));
        // a rate nobody shoots is not silently rounded to one that is
        assert_eq!(Rate::nearest(48.0), None);
        assert_eq!(Rate::nearest(0.0), None);
        assert_eq!(Rate::Fps29_97.label(true), "29.97 DF");
        assert_eq!(Rate::Fps23_976.label(false), "23.976");
        assert!(!Rate::Fps23_976.can_drop(), "23.976 drifts too, and has no drop standard");
    }
}
