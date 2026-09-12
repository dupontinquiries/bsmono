//! The arrangement: tracks of clips on a beat grid.
//!
//! Time is in BEATS (`f64`), not samples or seconds, because the grid is
//! the thing the user arranges against and the generator's whole beat-phase
//! machinery is in beats. A clip's SOURCE has a fixed duration in seconds
//! (a captured window, a decoded file); its length on the grid follows
//! from the tempo. Change the tempo and clips keep their start beat and
//! change their extent — the audio is not stretched. That is the honest
//! behaviour for a tool with no time-stretch, and it is stated here rather
//! than discovered.
//!
//! This module holds only what the arrangement IS. What can be done to it
//! is `cmd`; how the pointer does it is `tools`; how it sounds is `render`.
//! Nothing here touches egui, audio or files, so every rule is a unit test.

use std::collections::BTreeSet;

/// Where a clip's audio comes from. Ids index the `render::Library`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRef {
    /// A window captured from the generator (a "beat").
    Beat(usize),
    /// A decoded audio file.
    File(usize),
}

pub type ClipId = u64;
pub type TrackId = u64;

#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub id: ClipId,
    pub source: SourceRef,
    /// Start on the grid, in beats.
    pub start: f64,
    /// Length on the grid, in beats. At most the source's length at the
    /// current tempo minus `offset`; `Timeline::clamp_clip` enforces it.
    pub len: f64,
    /// How far into the source the clip begins, in beats (a trimmed head).
    pub offset: f64,
    /// Linear gain.
    pub gain: f32,
    pub muted: bool,
}

impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.len
    }
    pub fn contains(&self, beat: f64) -> bool {
        beat >= self.start && beat < self.end()
    }
    pub fn overlaps(&self, from: f64, to: f64) -> bool {
        self.start < to && self.end() > from
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub clips: Vec<Clip>,
    pub muted: bool,
    pub solo: bool,
    pub gain: f32,
}

impl Track {
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }
    /// The clip under `beat`, if any. Clips on one track do not overlap
    /// (placement resolves that), so there is at most one.
    pub fn clip_at(&self, beat: f64) -> Option<&Clip> {
        self.clips.iter().find(|c| c.contains(beat))
    }
    pub fn end(&self) -> f64 {
        self.clips.iter().map(Clip::end).fold(0.0, f64::max)
    }
}

/// Snap resolution, in beats. `None` is free placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid(pub Option<f64>);

impl Grid {
    pub const BAR: Grid = Grid(Some(4.0));
    pub const BEAT: Grid = Grid(Some(1.0));
    pub const HALF: Grid = Grid(Some(0.5));
    pub const QUARTER: Grid = Grid(Some(0.25));
    pub const OFF: Grid = Grid(None);

    pub fn snap(self, beat: f64) -> f64 {
        match self.0 {
            Some(step) if step > 0.0 => (beat / step).round() * step,
            _ => beat,
        }
    }
    pub fn label(self) -> &'static str {
        match self.0 {
            Some(x) if x == 4.0 => "bar",
            Some(x) if x == 1.0 => "beat",
            Some(x) if x == 0.5 => "1/2",
            Some(x) if x == 0.25 => "1/4",
            Some(_) => "grid",
            None => "free",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub tracks: Vec<Track>,
    pub bpm: f64,
    pub beats_per_bar: u32,
    /// Loop region in beats, when looping.
    pub loop_region: Option<(f64, f64)>,
    pub grid: Grid,
    next_id: u64,
}

impl Default for Timeline {
    fn default() -> Self {
        let mut t = Self {
            tracks: Vec::new(),
            bpm: 120.0,
            beats_per_bar: 4,
            loop_region: None,
            grid: Grid::BEAT,
            next_id: 1,
        };
        t.add_track("beats");
        t.add_track("audio");
        t
    }
}

impl Timeline {
    pub fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn secs_to_beats(&self, secs: f64) -> f64 {
        secs * self.bpm / 60.0
    }
    pub fn beats_to_secs(&self, beats: f64) -> f64 {
        beats * 60.0 / self.bpm
    }

    pub fn add_track(&mut self, name: &str) -> TrackId {
        let id = self.fresh_id();
        self.tracks.push(Track {
            id,
            name: name.to_string(),
            clips: Vec::new(),
            muted: false,
            solo: false,
            gain: 1.0,
        });
        id
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// The track holding `clip`, and the clip.
    pub fn find(&self, clip: ClipId) -> Option<(&Track, &Clip)> {
        self.tracks.iter().find_map(|t| t.clip(clip).map(|c| (t, c)))
    }
    pub fn find_track_of(&self, clip: ClipId) -> Option<TrackId> {
        self.find(clip).map(|(t, _)| t.id)
    }

    /// End of the last clip on any track.
    pub fn end(&self) -> f64 {
        self.tracks.iter().map(Track::end).fold(0.0, f64::max)
    }

    /// Place a clip of `source` with `source_beats` of material, starting
    /// at `start` (already snapped), on `track`. Returns the new id.
    ///
    /// OVERLAPS ARE RESOLVED BY TRIMMING THE NEWCOMER, not the neighbours:
    /// the clip that was there is the one the user placed earlier and
    /// looked at since. A newcomer that would land entirely inside an
    /// existing clip is refused rather than shortened to nothing.
    pub fn place(&mut self, track: TrackId, source: SourceRef, start: f64, source_beats: f64) -> Option<ClipId> {
        let id = self.fresh_id();
        let t = self.track_mut(track)?;
        let start = start.max(0.0);
        let mut len = source_beats.max(0.0);
        // Trim against the next clip to the right.
        if let Some(next_start) = t
            .clips
            .iter()
            .map(|c| c.start)
            .filter(|&s| s >= start)
            .fold(None, |m: Option<f64>, s| Some(m.map_or(s, |m| m.min(s))))
        {
            len = len.min(next_start - start);
        }
        // Refuse a start that is inside an existing clip.
        if t.clip_at(start).is_some() || len <= 1e-9 {
            return None;
        }
        t.clips.push(Clip { id, source, start, len, offset: 0.0, gain: 1.0, muted: false });
        t.clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        Some(id)
    }

    /// Can `clip` sit at `start` with `len` on `track` without touching
    /// another clip? Used by move/trim previews.
    pub fn fits(&self, track: TrackId, clip: ClipId, start: f64, len: f64) -> bool {
        let Some(t) = self.track(track) else { return false };
        if start < 0.0 || len <= 1e-9 {
            return false;
        }
        !t.clips.iter().any(|c| c.id != clip && c.overlaps(start, start + len))
    }

    /// The clip ids that overlap `[from, to)` on the given tracks (all
    /// tracks when `tracks` is empty). For marquee selection.
    pub fn clips_in(&self, from: f64, to: f64, tracks: &[TrackId]) -> BTreeSet<ClipId> {
        let (from, to) = (from.min(to), from.max(to));
        self.tracks
            .iter()
            .filter(|t| tracks.is_empty() || tracks.contains(&t.id))
            .flat_map(|t| t.clips.iter().filter(|c| c.overlaps(from, to)).map(|c| c.id))
            .collect()
    }

    /// Whether a track is audible given the solo state of the others.
    pub fn audible(&self, track: &Track) -> bool {
        if track.muted {
            return false;
        }
        let any_solo = self.tracks.iter().any(|t| t.solo);
        !any_solo || track.solo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl() -> Timeline {
        Timeline::default()
    }

    #[test]
    fn a_default_timeline_has_two_tracks_and_a_beat_grid() {
        let t = tl();
        assert_eq!(t.tracks.len(), 2);
        assert_eq!(t.grid, Grid::BEAT);
        assert_eq!(t.secs_to_beats(0.5), 1.0, "half a second is one beat at 120");
        assert_eq!(t.beats_to_secs(4.0), 2.0);
    }

    #[test]
    fn grids_snap_and_free_does_not() {
        assert_eq!(Grid::BEAT.snap(2.4), 2.0);
        assert_eq!(Grid::BEAT.snap(2.6), 3.0);
        assert_eq!(Grid::HALF.snap(2.6), 2.5);
        assert_eq!(Grid::BAR.snap(5.0), 4.0);
        assert_eq!(Grid::OFF.snap(2.6), 2.6);
    }

    #[test]
    fn placing_trims_the_newcomer_against_its_right_neighbour_and_refuses_an_inside_start() {
        let mut t = tl();
        let tr = t.tracks[0].id;
        let a = t.place(tr, SourceRef::Beat(0), 4.0, 2.0).unwrap();
        // A clip ending past `a`'s start is cut to reach it.
        let b = t.place(tr, SourceRef::Beat(1), 1.0, 8.0).unwrap();
        let track = t.track(tr).unwrap();
        assert_eq!(track.clip(b).unwrap().len, 3.0, "1 + 3 = 4, where a begins");
        assert_eq!(track.clip(a).unwrap().len, 2.0, "the older clip is untouched");
        // Sorted by start after placement.
        assert_eq!(track.clips.iter().map(|c| c.id).collect::<Vec<_>>(), vec![b, a]);
        // A start inside an existing clip is refused.
        assert!(t.place(tr, SourceRef::Beat(2), 4.5, 1.0).is_none());
        // Zero-length material is refused.
        assert!(t.place(tr, SourceRef::Beat(2), 10.0, 0.0).is_none());
        // Negative starts clamp to zero.
        let c = t.place(tr, SourceRef::File(0), -3.0, 1.0).unwrap();
        assert_eq!(t.track(tr).unwrap().clip(c).unwrap().start, 0.0);
    }

    #[test]
    fn fits_ignores_the_clip_itself_and_sees_the_others() {
        let mut t = tl();
        let tr = t.tracks[0].id;
        let a = t.place(tr, SourceRef::Beat(0), 0.0, 2.0).unwrap();
        let b = t.place(tr, SourceRef::Beat(0), 4.0, 2.0).unwrap();
        assert!(t.fits(tr, a, 1.0, 2.0), "a may slide right up to b");
        assert!(t.fits(tr, a, 2.0, 2.0), "touching is fine");
        assert!(!t.fits(tr, a, 3.0, 2.0), "into b is not");
        assert!(!t.fits(tr, b, -1.0, 2.0), "before the origin is not");
        assert!(t.fits(tr, b, 2.0, 2.0));
    }

    #[test]
    fn marquee_selection_collects_overlapping_clips_on_the_asked_tracks() {
        let mut t = tl();
        let (t0, t1) = (t.tracks[0].id, t.tracks[1].id);
        let a = t.place(t0, SourceRef::Beat(0), 0.0, 2.0).unwrap();
        let b = t.place(t0, SourceRef::Beat(0), 4.0, 2.0).unwrap();
        let c = t.place(t1, SourceRef::File(0), 1.0, 8.0).unwrap();
        assert_eq!(t.clips_in(1.5, 0.5, &[]), [a, c].into_iter().collect());
        assert_eq!(t.clips_in(3.0, 5.0, &[t0]), [b].into_iter().collect());
        assert!(t.clips_in(2.0, 4.0, &[t0]).is_empty(), "a gap is a gap");
    }

    #[test]
    fn solo_silences_the_others_and_mute_beats_solo() {
        let mut t = tl();
        assert!(t.audible(&t.tracks[0]) && t.audible(&t.tracks[1]));
        t.tracks[0].solo = true;
        assert!(t.audible(&t.tracks[0]));
        assert!(!t.audible(&t.tracks[1]));
        t.tracks[0].muted = true;
        assert!(!t.audible(&t.tracks[0]));
    }

    #[test]
    fn ids_are_never_reused() {
        let mut t = tl();
        let tr = t.tracks[0].id;
        let a = t.place(tr, SourceRef::Beat(0), 0.0, 1.0).unwrap();
        t.track_mut(tr).unwrap().clips.clear();
        let b = t.place(tr, SourceRef::Beat(0), 0.0, 1.0).unwrap();
        assert_ne!(a, b);
    }
}
