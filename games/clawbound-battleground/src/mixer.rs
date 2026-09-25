//! mixer -- plays loaded clips (music, sound effects, character voices) in
//! the audio callback, on top of the synthesized ambience.
//!
//! ## Realtime rules
//!
//! [`Mixer::render_add`] runs on the audio thread, where a stall is an
//! audible click. It never blocks, never allocates, never frees:
//! - commands arrive through an `rtrb` single-producer/single-consumer ring
//!   (wait-free); a full ring drops the command on the GAME side;
//! - clips are `Arc<[f32]>` decoded before they arrive (`soundbank`); the
//!   bank keeps its own reference to every clip, so a voice finishing drops
//!   a reference but never the last one -- no free on this thread;
//! - voices and the clip table are fixed-size, allocated in `new`.
//!
//! ## What it does
//!
//! Three buses (music, sfx, voice) with their own gains; equal-power pan;
//! voice lines duck the music and sfx while they play; one line per
//! speaker (a new bark replaces the cat's previous one, they never pile up);
//! when every voice is busy, the lowest priority -- then the oldest -- is
//! stolen.

use std::sync::Arc;

/// Voices playing at once.
pub const MAX_VOICES: usize = 32;
/// Clips the table can hold.
pub const MAX_CLIPS: usize = 4096;
/// Commands the ring holds between two audio callbacks.
pub const RING: usize = 256;
/// How far music and sfx duck under a voice line (linear gain).
pub const DUCK: f32 = 0.45;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Music = 0,
    Sfx = 1,
    Voice = 2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Play {
    pub clip: u32,
    pub bus: Bus,
    pub gain: f32,
    /// -1 left .. 1 right.
    pub pan: f32,
    /// Higher survives voice stealing.
    pub priority: u8,
    pub looped: bool,
    /// A voice line's speaker (a god index): one line per speaker at a time.
    pub speaker: Option<u16>,
}

pub enum Cmd {
    /// A decoded clip: interleaved f32 at the device rate, 1 or 2 channels.
    Load { id: u32, pcm: Arc<[f32]>, channels: u8 },
    Play(Play),
    StopBus(Bus),
    BusGain(Bus, f32),
}

#[derive(Clone)]
struct Voice {
    clip: Option<Arc<[f32]>>,
    channels: u8,
    pos: usize,
    p: Play,
    age: u64,
}

impl Voice {
    fn idle() -> Voice {
        Voice { clip: None, channels: 1, pos: 0, p: Play { clip: 0, bus: Bus::Sfx, gain: 0.0, pan: 0.0, priority: 0, looped: false, speaker: None }, age: 0 }
    }
}

/// The game's side: push commands, never waits.
pub struct MixerHandle {
    tx: rtrb::Producer<Cmd>,
    /// Commands dropped because the ring was full (a burst larger than the
    /// ring between two callbacks). Visible, not silent.
    pub dropped: u64,
}

impl MixerHandle {
    pub fn send(&mut self, c: Cmd) -> bool {
        match self.tx.push(c) {
            Ok(()) => true,
            Err(_) => {
                self.dropped += 1;
                false
            }
        }
    }
    pub fn play(&mut self, p: Play) -> bool {
        self.send(Cmd::Play(p))
    }
}

/// The audio thread's side.
pub struct Mixer {
    rx: rtrb::Consumer<Cmd>,
    clips: Vec<Option<(Arc<[f32]>, u8)>>,
    voices: Vec<Voice>,
    bus_gain: [f32; 3],
    duck: f32,
    clock: u64,
}

pub fn new() -> (MixerHandle, Mixer) {
    let (tx, rx) = rtrb::RingBuffer::new(RING);
    (
        MixerHandle { tx, dropped: 0 },
        Mixer { rx, clips: vec![None; MAX_CLIPS], voices: vec![Voice::idle(); MAX_VOICES], bus_gain: [0.55, 0.9, 1.0], duck: 1.0, clock: 0 },
    )
}

impl Mixer {
    fn apply(&mut self, c: Cmd) {
        match c {
            Cmd::Load { id, pcm, channels } => {
                if let Some(slot) = self.clips.get_mut(id as usize) {
                    *slot = Some((pcm, channels.clamp(1, 2)));
                }
            }
            Cmd::Play(p) => {
                let Some(Some((pcm, ch))) = self.clips.get(p.clip as usize) else { return };
                let (pcm, ch) = (pcm.clone(), *ch);
                self.clock += 1;
                // one line per speaker: the new one replaces the old
                if let (Bus::Voice, Some(sp)) = (p.bus, p.speaker) {
                    for v in self.voices.iter_mut() {
                        if v.clip.is_some() && v.p.bus == Bus::Voice && v.p.speaker == Some(sp) {
                            v.clip = None;
                        }
                    }
                }
                let slot = match self.voices.iter().position(|v| v.clip.is_none()) {
                    Some(i) => i,
                    None => {
                        // steal: lowest priority, then the oldest
                        let (i, v) = self.voices.iter().enumerate().min_by_key(|(_, v)| (v.p.priority, v.age)).expect("voices");
                        if v.p.priority > p.priority {
                            return; // everything playing matters more
                        }
                        i
                    }
                };
                self.voices[slot] = Voice { clip: Some(pcm), channels: ch, pos: 0, p, age: self.clock };
            }
            Cmd::StopBus(b) => {
                for v in self.voices.iter_mut() {
                    if v.p.bus == b {
                        v.clip = None;
                    }
                }
            }
            Cmd::BusGain(b, g) => self.bus_gain[b as usize] = g.clamp(0.0, 2.0),
        }
    }

    /// Voices playing now.
    pub fn active(&self) -> usize {
        self.voices.iter().filter(|v| v.clip.is_some()).count()
    }

    /// Mix every playing voice INTO `out` (interleaved, `channels` wide),
    /// on top of whatever is there. Allocation- and lock-free.
    pub fn render_add(&mut self, out: &mut [f32], channels: usize) {
        while let Ok(c) = self.rx.pop() {
            self.apply(c);
        }
        let channels = channels.max(1);
        let frames = out.len() / channels;
        // duck toward its target once per block (smooth enough at ~5 ms)
        let talking = self.voices.iter().any(|v| v.clip.is_some() && v.p.bus == Bus::Voice);
        let want = if talking { DUCK } else { 1.0 };
        let k = 1.0 - (-(frames as f32) / 2400.0).exp(); // ~50 ms at 48 kHz
        self.duck += (want - self.duck) * k;
        for v in self.voices.iter_mut() {
            let Some(pcm) = v.clip.as_ref() else { continue };
            let ch = v.channels as usize;
            let len = pcm.len() / ch;
            let duck = if v.p.bus == Bus::Voice { 1.0 } else { self.duck };
            let g = v.p.gain * self.bus_gain[v.p.bus as usize] * duck;
            let a = (v.p.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            let (gl, gr) = (a.cos() * g, a.sin() * g);
            let mut ended = false;
            for f in 0..frames {
                if v.pos >= len {
                    if v.p.looped && len > 0 {
                        v.pos = 0;
                    } else {
                        ended = true;
                        break;
                    }
                }
                let (l, r) = if ch == 2 { (pcm[v.pos * 2], pcm[v.pos * 2 + 1]) } else { (pcm[v.pos], pcm[v.pos]) };
                v.pos += 1;
                let o = &mut out[f * channels..f * channels + channels];
                if channels == 1 {
                    o[0] += (l * gl + r * gr) * std::f32::consts::FRAC_1_SQRT_2;
                } else {
                    o[0] += l * gl;
                    o[1] += r * gr;
                }
            }
            if ended {
                // drops a reference; the bank holds another, so nothing is
                // freed on this thread
                v.clip = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, v: f32) -> Arc<[f32]> {
        (0..n).map(|_| v).collect::<Vec<f32>>().into()
    }
    fn play(clip: u32, bus: Bus) -> Play {
        Play { clip, bus, gain: 1.0, pan: 0.0, priority: 1, looped: false, speaker: None }
    }

    /// A clip plays from the first sample of the next block, at its gain,
    /// panned, and stops at its end.
    #[test]
    fn a_clip_plays_exactly_once_centred() {
        let (mut h, mut m) = new();
        let clip = tone(1000, 0.5);
        h.send(Cmd::Load { id: 3, pcm: clip.clone(), channels: 1 });
        h.send(Cmd::BusGain(Bus::Sfx, 1.0));
        h.play(play(3, Bus::Sfx));
        let mut out = vec![0.0f32; 2 * 1500];
        m.render_add(&mut out, 2);
        let centre = 0.5 * std::f32::consts::FRAC_PI_4.cos();
        assert!((out[0] - centre).abs() < 1e-5 && (out[1] - centre).abs() < 1e-5, "first frame {} {}", out[0], out[1]);
        assert!((out[2 * 999] - centre).abs() < 1e-5, "last frame of the clip");
        assert_eq!(out[2 * 1000], 0.0, "and then silence");
        assert_eq!(m.active(), 0);
    }

    /// It ADDS to what is there (the ambience), it does not overwrite.
    #[test]
    fn it_mixes_on_top_of_the_ambience() {
        let (mut h, mut m) = new();
        h.send(Cmd::Load { id: 0, pcm: tone(10, 0.2), channels: 1 });
        h.send(Cmd::BusGain(Bus::Sfx, 1.0));
        h.play(Play { pan: -1.0, ..play(0, Bus::Sfx) });
        let mut out = vec![0.1f32; 2 * 4];
        m.render_add(&mut out, 2);
        assert!((out[0] - 0.3).abs() < 1e-5 && (out[1] - 0.1).abs() < 1e-5, "hard left: {out:?}");
    }

    /// A voice line ducks the music under it, and the music comes back.
    #[test]
    fn a_voice_line_ducks_the_music() {
        let (mut h, mut m) = new();
        h.send(Cmd::Load { id: 0, pcm: tone(48_000, 0.5), channels: 1 });
        h.send(Cmd::Load { id: 1, pcm: tone(9_600, 0.5), channels: 1 });
        h.play(Play { looped: true, ..play(0, Bus::Music) });
        let mut out = vec![0.0f32; 256];
        m.render_add(&mut out, 1);
        let before = out[255];
        h.play(Play { speaker: Some(4), ..play(1, Bus::Voice) });
        for _ in 0..20 {
            m.render_add(&mut out, 1);
        }
        // the voice alone is 0.5 * 1.0 * cos(pi/4) * sqrt(1/2) scaled; measure
        // the MUSIC's share by stopping the voice bus and rendering one block
        h.send(Cmd::StopBus(Bus::Voice));
        out.iter_mut().for_each(|x| *x = 0.0);
        m.render_add(&mut out, 1);
        let ducked = out[0];
        assert!(ducked < before * 0.8, "not ducked: {ducked} vs {before}");
        for _ in 0..200 {
            out.iter_mut().for_each(|x| *x = 0.0);
            m.render_add(&mut out, 1);
        }
        assert!((out[255] - before).abs() < before * 0.02, "did not come back: {} vs {before}", out[255]);
    }

    /// One line per speaker: a new bark replaces the cat's previous one.
    #[test]
    fn a_speaker_says_one_thing_at_a_time() {
        let (mut h, mut m) = new();
        h.send(Cmd::Load { id: 0, pcm: tone(10_000, 0.3), channels: 1 });
        h.play(Play { speaker: Some(2), ..play(0, Bus::Voice) });
        h.play(Play { speaker: Some(2), ..play(0, Bus::Voice) });
        h.play(Play { speaker: Some(5), ..play(0, Bus::Voice) });
        let mut out = vec![0.0f32; 64];
        m.render_add(&mut out, 1);
        assert_eq!(m.active(), 2, "cat 2 once, cat 5 once");
    }

    /// Full: the lowest priority is stolen; nothing more important is.
    #[test]
    fn voices_are_stolen_by_priority() {
        let (mut h, mut m) = new();
        h.send(Cmd::Load { id: 0, pcm: tone(100_000, 0.01), channels: 1 });
        let mut out = vec![0.0f32; 64];
        for _ in 0..MAX_VOICES {
            h.play(Play { priority: 5, ..play(0, Bus::Sfx) });
        }
        m.render_add(&mut out, 1);
        assert_eq!(m.active(), MAX_VOICES);
        h.play(Play { priority: 1, ..play(0, Bus::Sfx) });
        m.render_add(&mut out, 1);
        assert!(m.voices.iter().all(|v| v.p.priority == 5), "a low one displaced a high one");
        h.play(Play { priority: 9, ..play(0, Bus::Sfx) });
        m.render_add(&mut out, 1);
        assert!(m.voices.iter().any(|v| v.p.priority == 9), "an important one could not get in");
    }

    /// The game never waits: past the ring's size, commands are dropped and
    /// counted, not blocked on.
    #[test]
    fn a_full_ring_drops_and_counts_never_blocks() {
        let (mut h, _m) = new();
        for _ in 0..RING + 10 {
            h.play(play(0, Bus::Sfx));
        }
        assert_eq!(h.dropped, 10);
    }

    /// A finished voice drops ITS reference only; the bank's keeps the
    /// clip alive, so the audio thread never frees memory.
    #[test]
    fn the_audio_thread_never_holds_the_last_reference() {
        let (mut h, mut m) = new();
        let clip = tone(100, 0.1);
        let bank_ref = clip.clone();
        h.send(Cmd::Load { id: 0, pcm: clip, channels: 1 });
        h.play(play(0, Bus::Sfx));
        let mut out = vec![0.0f32; 512];
        m.render_add(&mut out, 1);
        assert_eq!(m.active(), 0);
        assert_eq!(Arc::strong_count(&bank_ref), 2, "bank + clip table");
    }
}
