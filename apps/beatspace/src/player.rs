//! Realtime audio output with seamless crossfades.
//!
//! The generator thread produces one short "beat" buffer at a time and calls
//! `Player::push`. The cpal callback pulls one sample at a time via
//! `Player::next_sample`, looping the current beat until a new one arrives and
//! equal-power crossfading (cos/sin) into it so morphs through latent space are
//! click-free. Generated audio is at the model sample rate; we resample to the
//! output device rate on push so the callback stays trivial.

use anyhow::{anyhow, Result};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio_io::resample_linear;

pub struct Player {
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

    /// Produce one output sample.
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
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            if let Ok(mut p) = player.lock() {
                for frame in data.chunks_mut(channels) {
                    let v = p.next_sample();
                    let s: T = T::from_sample(v);
                    for d in frame.iter_mut() {
                        *d = s;
                    }
                }
            } else {
                for d in data.iter_mut() {
                    *d = T::from_sample(0.0f32);
                }
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}
