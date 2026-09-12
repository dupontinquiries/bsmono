//! Realtime audio output with seamless crossfades.
//!
//! Two modes, chosen by which push the generator thread calls:
//!
//! * LOOP (`push`): one short "beat" buffer at a time. The callback loops the
//!   current beat until a new one arrives and equal-power crossfades (cos/sin)
//!   into it from its first sample, so morphs through latent space are
//!   click-free. The v6 behaviour.
//! * STREAM (`push_window`): each buffer is a continuation that starts
//!   `advance` samples after the previous one; the overlap is crossfaded in
//!   place and playback never restarts. `queued_src_samples` tells the
//!   generator thread how far ahead it is, so it paces to the audio clock.
//!   The sample-level work is `stream::OverlapAdd`, which has no cpal in it
//!   and is tested on its own.
//!
//! Generated audio is at the model sample rate; we resample to the output
//! device rate on push so the callback stays trivial. The callback takes the
//! lock ONCE per buffer and fills a block.

use anyhow::{anyhow, Result};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio_io::resample_linear;
use crate::stream::OverlapAdd;

pub struct Player {
    /// Stream mode, once `push_window` has been called.
    stream: Option<OverlapAdd>,
    cur: Vec<f32>,
    pos: usize,
    next: Option<Vec<f32>>,
    fade_pos: usize,
    fade_len: usize,
    src_sr: u32,
    out_sr: u32,
    /// last few output samples, for the GUI waveform strip
    scope: Vec<f32>,
    scope_cap: usize,
    /// Output gain, 0.0 to 1.0. Muting happens HERE rather than in the UI so
    /// nothing can push past it: the generator thread keeps producing beats and
    /// the callback keeps consuming them, but nothing reaches the device.
    gain: f32,
    /// Playback stopped: the callback outputs silence and the position does not
    /// advance. Distinct from gain 0 — a muted stream is still running and
    /// resumes where it was, a stopped one restarts the current beat. Both
    /// exist because "I want quiet" and "I want it to stop" are different
    /// requests and conflating them loses your place.
    stopped: bool,
}

impl Player {
    fn new(src_sr: u32, out_sr: u32) -> Self {
        Self {
            stream: None,
            cur: Vec::new(),
            pos: 0,
            next: None,
            fade_pos: 0,
            fade_len: (out_sr as usize / 20).max(1), // ~50 ms
            src_sr,
            out_sr,
            scope: vec![0.0; 512],
            scope_cap: 512,
            gain: 1.0,
            stopped: false,
        }
    }

    /// Hand the player a freshly generated beat (mono, at the model sample rate).
    pub fn push(&mut self, buf: &[f32]) {
        let res = resample_linear(buf, self.src_sr, self.out_sr);
        if res.is_empty() {
            return;
        }
        if self.cur.is_empty() {
            self.cur = res;
            self.pos = 0;
            return;
        }
        // If a fade is already running, snap it to its target first so we always
        // crossfade from a single coherent source.
        if let Some(next) = self.next.take() {
            self.pos = self.fade_pos.min(next.len().saturating_sub(1));
            self.cur = next;
        }
        self.fade_pos = 0;
        self.fade_len = (self.out_sr as usize / 20).max(1);
        self.next = Some(res);
    }

    /// Hand the player the next window of a STREAM (mono, at the model sample
    /// rate). `advance` is how many source samples after the previous
    /// window's start this one starts; the overlap is crossfaded.
    pub fn push_window(&mut self, buf: &[f32], advance: usize) {
        let res = resample_linear(buf, self.src_sr, self.out_sr);
        if res.is_empty() {
            return;
        }
        let adv = (advance as u64 * self.out_sr as u64 / self.src_sr.max(1) as u64) as usize;
        self.stream.get_or_insert_with(OverlapAdd::new).push(&res, adv);
    }

    /// Stream mode: drop everything queued. The next `push_window` starts
    /// the track over at the playhead.
    pub fn clear_stream(&mut self) {
        if let Some(oa) = self.stream.as_mut() {
            oa.clear();
        }
    }

    /// Stream mode: samples queued ahead of the playhead, in SOURCE-rate
    /// samples so the generator thread can compare against window lengths.
    pub fn queued_src_samples(&self) -> usize {
        match &self.stream {
            Some(oa) => (oa.queued() as u64 * self.src_sr as u64 / self.out_sr.max(1) as u64) as usize,
            None => 0,
        }
    }

    /// Fill a mono block. Loop mode goes sample by sample through
    /// `next_sample`; stream mode pulls a block from the overlap-add queue,
    /// which is why the callback asks for blocks — draining a Vec one sample
    /// at a time would move the whole queue per sample.
    pub fn fill(&mut self, out: &mut [f32]) {
        let Some(oa) = self.stream.as_mut() else {
            for o in out.iter_mut() {
                *o = self.next_sample();
            }
            return;
        };
        if self.stopped {
            for o in out.iter_mut() {
                *o = 0.0;
            }
        } else {
            oa.pull(out);
            for o in out.iter_mut() {
                *o *= self.gain;
            }
        }
        // The scope shows what LEFT the player, gain included, and shows
        // silence when stopped rather than freezing.
        let n = out.len().min(self.scope_cap);
        let drop = (self.scope.len() + n).saturating_sub(self.scope_cap);
        self.scope.drain(..drop);
        self.scope.extend_from_slice(&out[out.len() - n..]);
    }

    /// Set the output gain. Clamped, because a gain above 1 clips on a signal
    /// that is already normalised and the distortion sounds like a broken model.
    pub fn set_gain(&mut self, g: f32) {
        self.gain = g.clamp(0.0, 1.0);
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Stop or resume. Stopping rewinds to the start of the current beat, so
    /// resuming begins on a beat boundary rather than mid-phrase.
    pub fn set_stopped(&mut self, s: bool) {
        if s && !self.stopped {
            self.pos = 0;
            self.fade_pos = 0;
        }
        self.stopped = s;
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn next_sample(&mut self) -> f32 {
        if self.stopped || self.cur.is_empty() {
            // The scope is fed zeros too, so the waveform strip shows silence
            // rather than freezing on the last frame — a frozen scope reads as
            // a hung app.
            if self.scope.len() >= self.scope_cap { self.scope.remove(0); }
            self.scope.push(0.0);
            return 0.0;
        }
        let clen = self.cur.len();
        let a = self.cur[self.pos % clen];

        let out = if self.next.is_none() {
            self.pos = (self.pos + 1) % clen;
            a
        } else {
            let (b, nlen) = {
                let next = self.next.as_ref().unwrap();
                let idx = self.fade_pos.min(next.len() - 1);
                (next[idx], next.len())
            };
            let t = (self.fade_pos as f32 / self.fade_len as f32).min(1.0);
            let theta = t * std::f32::consts::FRAC_PI_2;
            let s = a * theta.cos() + b * theta.sin();

            self.fade_pos += 1;
            self.pos = (self.pos + 1) % clen;

            if self.fade_pos >= self.fade_len || self.fade_pos >= nlen {
                let next = self.next.take().unwrap();
                self.pos = self.fade_pos.min(next.len().saturating_sub(1));
                self.cur = next;
                self.fade_pos = 0;
            }
            s
        };

        // Gain applied LAST, after the crossfade, so a fade in progress is not
        // distorted by a volume change landing mid-blend.
        let out = out * self.gain;

        // feed the scope ring (cheap; just overwrite oldest)
        if self.scope.len() >= self.scope_cap {
            self.scope.remove(0);
        }
        // The scope shows what LEFT the player, gain included — a waveform at
        // full height while the output is muted is a display that disagrees
        // with the speakers.
        self.scope.push(out);
        out
    }

    /// Snapshot of recent output for the GUI waveform.
    pub fn scope_snapshot(&self) -> Vec<f32> {
        self.scope.clone()
    }
}

/// Open the default output device, start the stream, and return it together
/// with the shared Player. The caller MUST keep the returned `Stream` alive
/// (cpal stops playback when it is dropped) and is the one that calls
/// `player.lock().push(..)`. The stream is created on the calling thread because
/// `cpal::Stream` is `!Send`.
pub fn start(src_sr: u32) -> Result<(cpal::Stream, Arc<Mutex<Player>>)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no output audio device"))?;
    let supported = device.default_output_config()?;
    let out_sr = supported.sample_rate();
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let player = Arc::new(Mutex::new(Player::new(src_sr, out_sr)));
    let stream = build_stream(&device, &config, sample_format, player.clone())?;
    stream.play()?;
    println!(
        "[audio] output @ {} Hz, {} ch, {:?}",
        out_sr, config.channels, sample_format
    );
    Ok((stream, player))
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: cpal::SampleFormat,
    player: Arc<Mutex<Player>>,
) -> Result<cpal::Stream> {
    use cpal::SampleFormat as SF;
    let s = match fmt {
        SF::F32 => run::<f32>(device, config, player)?,
        SF::I16 => run::<i16>(device, config, player)?,
        SF::U16 => run::<u16>(device, config, player)?,
        other => return Err(anyhow!("unsupported output sample format: {other:?}")),
    };
    Ok(s)
}

fn run<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    player: Arc<Mutex<Player>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let err_fn = |e| eprintln!("[audio] stream error: {e}");
    // One mono scratch block, grown on the first callback and reused: no
    // allocation on the audio thread after that.
    let mut mono: Vec<f32> = Vec::new();
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels.max(1);
            mono.resize(frames, 0.0);
            match player.lock() {
                Ok(mut p) => p.fill(&mut mono),
                Err(_) => mono.iter_mut().for_each(|m| *m = 0.0),
            }
            for (frame, &v) in data.chunks_mut(channels).zip(mono.iter()) {
                let s: T = T::from_sample(v);
                for d in frame.iter_mut() {
                    *d = s;
                }
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}
