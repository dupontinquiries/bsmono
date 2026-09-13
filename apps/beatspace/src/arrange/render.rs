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

use std::collections::HashMap;

use super::model::{ClipId, SourceRef, Timeline};
use super::stretch::Stretcher;

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

/// The floor on every clip edge: 5 ms at 22050, short enough not to
/// soften a transient, long enough not to click. A clip's own fades are
/// on top of this.
const FADE_SECS: f64 = 0.005;

/// Per-clip stretcher state, kept between blocks so a tempo-locked clip
/// streams instead of restarting every block. Owned by whoever renders
/// (the transport, or an export); an entry is created on first use and
/// dropped when the clip's rate changes.
#[derive(Default)]
pub struct StretchStates {
    map: HashMap<ClipId, Stretcher>,
}

impl StretchStates {
    fn get(&mut self, clip: ClipId, rate: f64) -> &mut Stretcher {
        let stale = self.map.get(&clip).map_or(false, |s| (s.rate() - rate).abs() > 1e-9);
        if stale {
            self.map.remove(&clip);
        }
        self.map.entry(clip).or_insert_with(|| Stretcher::new(rate))
    }
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// Mix `[from_beat, to_beat)` of the arrangement into `out` (which is
/// sized by the caller; `sr` says how many samples a second is). Adds into
/// `out`, so a caller rendering a wrap can call twice.
pub fn render_block(t: &Timeline, lib: &Library, sr: u32, from_beat: f64, out: &mut [f32], states: &mut StretchStates) {
    let n = out.len();
    if n == 0 {
        return;
    }
    let spb = 60.0 / t.bpm * sr as f64; // samples per beat
    let block_start = from_beat * spb; // absolute sample of out[0]
    let to_beat = from_beat + n as f64 / spb;
    let floor = (FADE_SECS * sr as f64) as usize;
    let mut scratch: Vec<f32> = Vec::new();

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
            let fade_in = ((clip.fade_in * spb) as usize).max(floor).min(clip_len);
            let fade_out = ((clip.fade_out * spb) as usize).max(floor).min(clip_len);
            // Which samples of `out` this clip covers.
            let a = (clip_start - block_start).max(0.0) as usize;
            let b = ((clip_start + clip_len as f64 - block_start).min(n as f64)).max(0.0) as usize;
            if b <= a {
                continue;
            }
            let first_in_clip = (block_start + a as f64 - clip_start).round() as usize;
            let rate = clip.rate_at(t.bpm);
            // The material for out[a..b], at the clip's rate.
            let material: &[f32] = if (rate - 1.0).abs() < 1e-6 {
                let lo = (offset + first_in_clip).min(src.audio.len());
                let hi = (lo + (b - a)).min(src.audio.len());
                &src.audio[lo..hi]
            } else {
                scratch.resize(b - a, 0.0);
                let st = states.get(clip.id, rate);
                st.render(&src.audio, offset + first_in_clip, &mut scratch);
                &scratch[..]
            };
            for (k, &x) in material.iter().enumerate() {
                let i = a + k;
                let in_clip = first_in_clip + k;
                let mut g = gain;
                if in_clip < fade_in {
                    g *= in_clip as f32 / fade_in as f32;
                }
                let to_end = clip_len.saturating_sub(in_clip);
                if to_end < fade_out {
                    g *= to_end as f32 / fade_out as f32;
                }
                out[i] += x * g;
            }
        }
    }
}

/// The arrangement transport: where playback is and how it wraps.
#[derive(Default)]
pub struct Transport {
    pub playing: bool,
    /// The next beat to render.
    pub pos: f64,
    /// Stretcher state per tempo-locked clip, continued across blocks.
    pub stretch: StretchStates,
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
                        render_block(t, lib, sr, self.pos, &mut out[done..done + take], &mut self.stretch);
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
                    render_block(t, lib, sr, self.pos, &mut out[done..], &mut self.stretch);
                    self.pos += beats_left;
                    done = n;
                }
            }
        }
        start
    }

    pub fn seek(&mut self, beat: f64) {
        self.pos = beat.max(0.0);
        // The stretchers restart on their own at a discontinuity; dropping
        // them just makes that explicit and frees any for clips gone.
        self.stretch.clear();
    }
}

/// Render the whole arrangement, for an export. Peak-limited to 0.98.
pub fn render_all(t: &Timeline, lib: &Library, sr: u32) -> Vec<f32> {
    let end = t.end();
    if end <= 0.0 {
        return Vec::new();
    }
    let n = (t.beats_to_secs(end) * sr as f64).ceil() as usize;
    let mut out = vec![0.0f32; n];
    let mut states = StretchStates::default();
    render_block(t, lib, sr, 0.0, &mut out, &mut states);
    let peak = out.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    if peak > 0.98 {
        for x in &mut out {
            *x *= 0.98 / peak;
        }
    }
    out
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
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 1.0, source_beats: 2.0, exact: false, lock: None }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out, &mut StretchStates::default());
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
        render_block(&e.timeline, &l, 100, 0.0, &mut out, &mut StretchStates::default());
        assert!((out[100] - 0.25).abs() < 1e-6);
        e.apply(Command::ToggleMute { track: t0 }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out, &mut StretchStates::default());
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn a_trimmed_clip_plays_its_offset_material() {
        let l = lib();
        let mut e = Editor::default();
        let t1 = e.timeline.tracks[1].id;
        let id = e.apply(Command::Place { track: t1, source: SourceRef::File(0), start: 0.0, source_beats: 4.0, exact: false, lock: None }).unwrap().created[0];
        // Head-trim one beat: the clip now starts at beat 1 and plays the
        // ramp from its 50th sample.
        e.apply(Command::Trim { clip: id, start: 1.0, len: 3.0, offset: 1.0 }).unwrap();
        let mut out = vec![0.0f32; 200];
        render_block(&e.timeline, &l, 100, 0.0, &mut out, &mut StretchStates::default());
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
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap();
        let mut out = vec![0.0f32; 22050];
        render_block(&e.timeline, &l, 22050, 0.0, &mut out, &mut StretchStates::default());
        let fade = (0.005 * 22050.0) as usize; // 110 samples
        assert_eq!(out[0], 0.0);
        assert!(out[fade / 2] > 0.4 && out[fade / 2] < 0.6);
        assert_eq!(out[fade + 10], 1.0);
        assert!(out[22049] < 0.05, "faded at the clip's end");
    }

    #[test]
    fn clip_fades_ramp_over_their_length_and_never_below_the_floor() {
        let mut l = Library::default();
        l.add_beat(Sample { name: "dc".into(), audio: vec![1.0; 44100], sr: 22050, planes: None, bpm: None });
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let id = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        // One beat of fade in (0.5 s = 11025 samples), a quarter beat out.
        e.apply(Command::SetClipFades { clip: id, fade_in: 1.0, fade_out: 0.25 }).unwrap();
        let mut out = vec![0.0f32; 22050];
        render_block(&e.timeline, &l, 22050, 0.0, &mut out, &mut StretchStates::default());
        assert!((out[5512] - 0.5).abs() < 0.01, "halfway through the fade in: {}", out[5512]);
        assert!((out[11025 + 100] - 1.0).abs() < 1e-3);
        let fo = 22050 / 4 / 2; // 0.25 beat = 2756 samples; check its middle
        assert!((out[22050 - 1378 - 1] - 0.5).abs() < 0.02, "halfway through the fade out: {}", out[22050 - 1378 - 1]);
        // Fades are clamped to the clip.
        e.apply(Command::SetClipFades { clip: id, fade_in: 50.0, fade_out: 50.0 }).unwrap();
        let c = e.timeline.find(id).unwrap().1;
        assert!(c.fade_in <= 2.0 && c.fade_out <= 2.0);
        let _ = fo;
    }

    #[test]
    fn a_tempo_locked_clip_is_stretched_to_keep_its_beats() {
        // A 1 s beat (440 Hz sine) locked to 120: two beats. At 60 BPM it
        // still spans two beats — now 2 s — and the pitch stays 440.
        let sr = 22050;
        let mut l = Library::default();
        let sine: Vec<f32> = (0..sr).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin()).collect();
        l.add_beat(Sample { name: "tone".into(), audio: sine, sr, planes: None, bpm: Some(120.0) });
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        let id = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap().created[0];
        e.apply(Command::SetTempoLock { clip: id, bpm: Some(120.0) }).unwrap();
        e.apply(Command::SetTempo { bpm: 60.0 }).unwrap();
        assert_eq!(e.timeline.find(id).unwrap().1.len, 2.0);
        // Two beats at 60 BPM is 2 s: the clip should sound for all of it.
        let mut tr = Transport::default();
        let mut out = vec![0.0f32; 2 * sr as usize];
        tr.render_next(&e.timeline, &l, sr, &mut out);
        let late = &out[3 * sr as usize / 2..2 * sr as usize - 2048];
        let peak = late.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak > 0.8, "the second second is silent: peak {peak}");
        let zc = late.windows(2).filter(|p| (p[0] < 0.0) != (p[1] < 0.0)).count() as f32 / (late.len() as f32 / sr as f32);
        assert!((zc - 880.0).abs() < 25.0, "pitch drifted: {zc} crossings/s");
        // Unlocked, the same clip at 60 plays its 1 s of material and then
        // nothing.
        e.apply(Command::SetTempoLock { clip: id, bpm: None }).unwrap();
        let c = e.timeline.find(id).unwrap().1;
        assert_eq!(c.len, 1.0, "1 s of material at 60 BPM is one beat");
        let mut tr = Transport::default();
        let mut out = vec![0.0f32; 2 * sr as usize];
        tr.render_next(&e.timeline, &l, sr, &mut out);
        assert!(out[sr as usize + 1000].abs() < 1e-6);
    }

    #[test]
    fn a_block_from_the_middle_of_the_arrangement_is_aligned() {
        let l = lib();
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 2.0, source_beats: 2.0, exact: false, lock: None }).unwrap();
        // Ask for beats 1..3 (100 samples): the clip begins halfway.
        let mut out = vec![0.0f32; 100];
        render_block(&e.timeline, &l, 100, 1.0, &mut out, &mut StretchStates::default());
        assert_eq!(out[49], 0.0);
        assert_eq!(out[50], 1.0);
    }

    #[test]
    fn the_transport_wraps_at_the_loop_end_within_a_block() {
        let l = lib();
        let mut e = Editor::default();
        let t0 = e.timeline.tracks[0].id;
        e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 0.0, source_beats: 2.0, exact: false, lock: None }).unwrap();
        e.apply(Command::SetLoop { region: Some((0.0, 1.0)) }).unwrap();
        let mut tr = Transport { playing: true, pos: 0.5, ..Default::default() };
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
