//! The library (what clips point at) and the renderer (what they sound like).
//!
//! Sources are audio at one sample rate — the model's — decoded or captured
//! once and held by index, so a clip is a few numbers and a copy of the
//! timeline is cheap. Rendering is by BLOCK: the player asks for
//! `[from_beat, to_beat)` and gets samples, so an edit made while playing
//! is heard at the next block rather than after a re-render of the whole
//! arrangement. A block is the sum over audible tracks of the clips that
//! overlap it, each at its own offset, gain, and with a short fade at both
//! edges so a cut never clicks.
//!
//! Looping is the caller's: `Transport::advance` wraps the position and the
//! renderer is asked for two blocks around the wrap.

use super::model::{SourceRef, Timeline};

/// One source in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub name: String,
    /// Mono, at `sr`.
    pub audio: Vec<f32>,
    pub sr: u32,
    /// For a captured beat: the mel planes it was rendered from and the
    /// tempo the conductor had, so it can be re-rendered or re-armed.
    pub planes: Option<Vec<f32>>,
    pub bpm: Option<f32>,
}

impl Sample {
    pub fn secs(&self) -> f64 {
        self.audio.len() as f64 / self.sr.max(1) as f64
    }
    /// A coarse envelope for drawing: `n` peak values across the sample.
    pub fn peaks(&self, n: usize) -> Vec<f32> {
        let n = n.max(1);
        let len = self.audio.len();
        if len == 0 {
            return vec![0.0; n];
        }
        (0..n)
            .map(|i| {
                let a = i * len / n;
                let b = ((i + 1) * len / n).max(a + 1).min(len);
                self.audio[a..b].iter().fold(0.0f32, |m, &x| m.max(x.abs()))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    pub beats: Vec<Sample>,
    pub files: Vec<Sample>,
}

impl Library {
    pub fn get(&self, r: SourceRef) -> Option<&Sample> {
        match r {
            SourceRef::Beat(i) => self.beats.get(i),
            SourceRef::File(i) => self.files.get(i),
        }
    }
    pub fn add_beat(&mut self, s: Sample) -> SourceRef {
        self.beats.push(s);
        SourceRef::Beat(self.beats.len() - 1)
    }
    pub fn add_file(&mut self, s: Sample) -> SourceRef {
        self.files.push(s);
        SourceRef::File(self.files.len() - 1)
    }
    /// A source's length in beats at the timeline's tempo.
    pub fn beats_of(&self, r: SourceRef, t: &Timeline) -> Option<f64> {
        self.get(r).map(|s| t.secs_to_beats(s.secs()))
    }
    pub fn name(&self, r: SourceRef) -> &str {
        self.get(r).map(|s| s.name.as_str()).unwrap_or("?")
    }
}

/// Samples of fade at each clip edge. 5 ms at 22050; short enough not to
/// soften a transient, long enough not to click.
const FADE_SECS: f64 = 0.005;

/// Mix `[from_beat, to_beat)` of the arrangement into `out` (which is
/// sized by the caller; `sr` says how many samples a second is). Adds into
/// `out`, so a caller rendering a wrap can call twice.
pub fn render_block(t: &Timeline, lib: &Library, sr: u32, from_beat: f64, out: &mut [f32]) {
    let n = out.len();
    if n == 0 {
        return;
    }
    let spb = 60.0 / t.bpm * sr as f64; // samples per beat
    let block_start = from_beat * spb; // absolute sample of out[0]
    let to_beat = from_beat + n as f64 / spb;
    let fade = (FADE_SECS * sr as f64) as usize;

    for track in &t.tracks {
        if !t.audible(track) {
            continue;
        }
        for clip in &track.clips {
            if clip.muted || !clip.overlaps(from_beat, to_beat) {
                continue;
            }
            let Some(src) = lib.get(clip.source) else { continue };
            if src.audio.is_empty() {
                continue;
            }
            let gain = clip.gain * track.gain;
            let clip_start = clip.start * spb;
            let clip_len = (clip.len * spb).round() as usize;
            let offset = (clip.offset * spb).round() as usize;
            // Which samples of `out` this clip covers.
            let a = (clip_start - block_start).max(0.0) as usize;
            let b = ((clip_start + clip_len as f64 - block_start).min(n as f64)).max(0.0) as usize;
            for i in a..b {
                // Position within the clip, then within the source.
                let in_clip = (block_start + i as f64 - clip_start) as usize;
                let src_ix = offset + in_clip;
                if src_ix >= src.audio.len() {
                    break;
                }
                let mut g = gain;
                if fade > 0 {
                    if in_clip < fade {
                        g *= in_clip as f32 / fade as f32;
                    }
                    let to_end = clip_len.saturating_sub(in_clip);
                    if to_end < fade {
                        g *= to_end as f32 / fade as f32;
                    }
                }
                out[i] += src.audio[src_ix] * g;
            }
        }
    }
}

/// The arrangement transport: where playback is and how it wraps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    pub playing: bool,
    /// The next beat to render.
    pub pos: f64,
}

impl Default for Transport {
    fn default() -> Self {
        Self { playing: false, pos: 0.0 }
    }
}

impl Transport {
    /// Render the next `out.len()` samples from `pos`, honouring the loop
    /// region: a block that crosses the loop end is rendered in two parts
    /// and the position wraps. Returns the beat the block started at.
    pub fn render_next(&mut self, t: &Timeline, lib: &Library, sr: u32, out: &mut [f32]) -> f64 {
        for o in out.iter_mut() {
            *o = 0.0;
        }
        let spb = 60.0 / t.bpm * sr as f64;
        let start = self.pos;
        let mut done = 0usize;
        let n = out.len();
        while done < n {
            let remaining = n - done;
            let beats_left = remaining as f64 / spb;
            match t.loop_region {
                Some((a, b)) if self.pos < b && self.pos + beats_left > b => {
                    // Up to the loop end, then wrap to its start.
                    let take = ((b - self.pos) * spb).round() as usize;
                    let take = take.min(remaining);
                    if take > 0 {
                        render_block(t, lib, sr, self.pos, &mut out[done..done + take]);
                    }
                    done += take;
                    self.pos = a;
                    if take == 0 {
                        // Guard against a loop shorter than a sample.
                        self.pos = a;
                        break;
                    }
                }
                Some((a, b)) if self.pos >= b => {
                    let _ = a;
                    self.pos = a;
                }
                _ => {
                    render_block(t, lib, sr, self.pos, &mut out[done..]);
                    self.pos += beats_left;
                    done = n;
                }
            }
        }
        start
    }

    pub fn seek(&mut self, beat: f64) {
        self.pos = beat.max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::cmd::{Command, Editor};

    fn lib() -> Library {
        let mut l = Library::default();
        // Beat 0: one second of 1.0; file 0: two seconds of ramp 0..1.
        l.add_beat(Sample { name: "one".into(), audio: vec![1.0; 100], sr: 100, planes: None, bpm: None });
        l.add_file(Sample { name: "ramp".into(), audio: (0..200).map(|i| i as f32 / 200.0).collect(), sr: 100, planes: None, bpm: None });
        l
    }

    #[test]
    fn a_sample_knows_its_length_in_beats_at_the_timeline_tempo() {
        let l = lib();
        let t = Timeline::default(); // 120 BPM
        assert_eq!(l.beats_of(SourceRef::Beat(0), &t), Some(2.0));
        assert_eq!(l.beats_of(SourceRef::File(0), &t), Some(4.0));
        assert_eq!(l.get(SourceRef::File(7)), None);
        assert_eq!(l.name(SourceRef::Beat(0)), "one");
        assert_eq!(l.get(SourceRef::File(0)).unwrap().peaks(4).len(), 4);
    }

    #[test]
    fn a_block_sums_the_clips_that_overlap_it_with_gains_and_edge_fades() {
        let l = lib();
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        // At 120 BPM and 100 Hz, a beat is 50 samples. Beat 0 (1 s = 2
        // beats) at beat 1: samples 50..150.
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 1.0, source_beats: 2.0 }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[49], 0.0);
        // Fade is 5 ms = 0.5 samples at 100 Hz -> 0 samples: no fade here.
        assert_eq!(out[50], 1.0);
        assert_eq!(out[149], 1.0);
        assert_eq!(out[150], 0.0);
        // Gain and track gain multiply; a muted track is silent.
        e.apply(Command::SetTrackGain { track: t0, gain: 0.5 }).unwrap();
        let id = e.timeline.tracks[0].clips[0].id;
        e.apply(Command::SetClipGain { clip: id, gain: 0.5 }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out);
        assert!((out[100] - 0.25).abs() < 1e-6);
        e.apply(Command::ToggleMute { track: t0 }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out);
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn a_trimmed_clip_plays_its_offset_material() {
        let l = lib();
        let mut e = Editor::default();
        let t1 = e.timeline.tracks[1].id;
        let id = e.apply(Command::Place { track: t1, source: SourceRef::File(0), start: 0.0, source_beats: 4.0 }).unwrap().created[0];
        // Head-trim one beat: the clip now starts at beat 1 and plays the
        // ramp from its 50th sample.
        e.apply(Command::Trim { clip: id, start: 1.0, len: 3.0, offset: 1.0 }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out);
        assert_eq!(out[49], 0.0);
        assert!((out[50] - 50.0 / 200.0).abs() < 1e-6, "sample 50 of the ramp, not 0");
        assert!((out[199] - 199.0 / 200.0).abs() < 1e-6);
    }

    #[test]
    fn edge_fades_apply_at_a_realistic_rate() {
        let mut l = Library::default();
        l.add_beat(Sample { name: "dc".into(), audio: vec![1.0; 44100], sr: 22050, planes: None, bpm: None });
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0 }).unwrap();
        let mut out = vec![0.0f32; 22050];
        render_block(&e.timeline, &l, 22050, 0.0, &mut out);
        let fade = (0.005 * 22050.0) as usize; // 110 samples
        assert_eq!(out[0], 0.0);
        assert!(out[fade / 2] > 0.4 && out[fade / 2] < 0.6);
        assert_eq!(out[fade + 10], 1.0);
        assert!(out[22049] < 0.05, "faded at the clip's end");
    }

    #[test]
    fn a_block_from_the_middle_of_the_arrangement_is_aligned() {
        let l = lib();
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 2.0, source_beats: 2.0 }).unwrap();
        // Ask for beats 1..3 (100 samples): the clip begins halfway.
        let mut out = vec![0.0f32; 100];
        render_block(&e.timeline, &l, 100, 1.0, &mut out);
        assert_eq!(out[49], 0.0);
        assert_eq!(out[50], 1.0);
    }

    #[test]
    fn the_transport_wraps_at_the_loop_end_within_a_block() {
        let l = lib();
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0 }).unwrap();
        e.apply(Command::SetLoop { region: Some((0.0, 1.0)) }).unwrap();
        let mut tr = Transport { playing: true, pos: 0.5 };
        // 100 samples = 2 beats: 0.5..1.0 (25 samples), wrap, 0..1 (50),
        // wrap, 0..0.5 (25).
        let mut out = vec![0.0f32; 100];
        let started = tr.render_next(&e.timeline, &l, 100, &mut out);
        assert_eq!(started, 0.5);
        assert!((tr.pos - 0.5).abs() < 1e-9, "ended half a beat into the third pass");
        assert!(out.iter().all(|&x| x == 1.0), "the clip covers the whole loop");
        // A position past the loop end snaps back to its start before
        // rendering; a block may END exactly on the loop end, and that is
        // wrapped at the next call rather than mid-block.
        tr.pos = 5.0;
        tr.render_next(&e.timeline, &l, 100, &mut out);
        assert!(tr.pos <= 1.0, "{}", tr.pos);
        tr.render_next(&e.timeline, &l, 100, &mut out);
        assert!(tr.pos <= 1.0);
        // Without a loop the position runs on.
        e.apply(Command::SetLoop { region: None }).unwrap();
        tr.pos = 0.0;
        tr.render_next(&e.timeline, &l, 100, &mut out);
        assert_eq!(tr.pos, 2.0);
        tr.seek(-3.0);
        assert_eq!(tr.pos, 0.0);
    }
}
