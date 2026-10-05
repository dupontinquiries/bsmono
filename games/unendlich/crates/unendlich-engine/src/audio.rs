//! Spatial music playback — OPUS-NATIVE + DECODE-ONCE (behind the `audio` feature).
//!
//! rodio's convenience decoders re-decode on every loop, which turns looping menu
//! music into a continuous decode pipeline. We avoid that entirely: each track is
//! decoded from Ogg-Opus to an in-memory PCM buffer EXACTLY ONCE (`decode_opus_file`,
//! via `ogg` demux + `audiopus`/libopus), then played through [`PcmLoop`] — a custom
//! rodio `Source` that loops by wrapping a read index over the shared `Arc<[i16]>`.
//! No re-decode, no per-loop realloc; looping is a pointer wrap.
//!
//! Stays opus end-to-end (no transcode). The pure parts ([`PcmLoop`], the decode
//! buffer) are unit-tested headless; the rodio sink playback needs an audio device
//! (verified to compile; runtime needs hardware). The spatial gains come from
//! `sim::spatial_audio` (already unit-tested).

use crate::sim::spatial_audio::{menu_stem_layout, mix_gains, StemSource};
use std::sync::Arc;

/// Interleaved PCM decoded once and held in memory.
#[derive(Clone)]
pub struct DecodedPcm {
    pub samples: Arc<[i16]>,
    pub channels: u16,
    pub rate: u32,
}

/// Decode an Ogg-Opus file to interleaved i16 PCM, ONCE. Pure CPU (no audio device).
#[cfg(feature = "audio")]
pub fn decode_opus_file(path: &std::path::Path) -> Result<DecodedPcm, String> {
    use std::io::Cursor;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut rdr = ogg::PacketReader::new(Cursor::new(bytes));
    let mut dec: Option<audiopus::coder::Decoder> = None;
    let mut channels = 2u16;
    let mut out: Vec<i16> = Vec::new();
    let mut hdr_seen = 0u8;
    while let Ok(Some(p)) = rdr.read_packet() {
        let d = p.data;
        if hdr_seen < 2 {
            if d.starts_with(b"OpusHead") && d.len() > 9 {
                channels = d[9] as u16;
                let ch = if channels >= 2 { audiopus::Channels::Stereo } else { audiopus::Channels::Mono };
                dec = Some(
                    audiopus::coder::Decoder::new(audiopus::SampleRate::Hz48000, ch)
                        .map_err(|e| format!("opus decoder init: {e:?}"))?,
                );
            }
            hdr_seen += 1;
            continue;
        }
        if let Some(ref mut dc) = dec {
            let mut frame = vec![0i16; 5760 * channels as usize];
            if let Ok(frames) = dc.decode(Some(&d), &mut frame[..], false) {
                out.extend_from_slice(&frame[..frames * channels as usize]);
            }
        }
    }
    if out.is_empty() {
        return Err("no audio decoded".into());
    }
    Ok(DecodedPcm { samples: out.into(), channels, rate: 48000 })
}

/// Encode interleaved f32 PCM as an Ogg-Opus file in memory — the format the
/// game already ships and decodes (`decode_opus_file`), so effects dumped by
/// UNENDLICH_SFX_DUMP and the `roar-render` tool come out as .opus.
///
/// `rate` must be one Opus accepts natively (8/12/16/24/48 kHz); `channels`
/// 1 or 2. 20 ms frames, 96 kb/s stereo / 64 kb/s mono, complexity 10 (this is
/// offline work, never on the frame thread). Pre-skip is the encoder's
/// look-ahead, in 48 kHz units as the Ogg-Opus spec (RFC 7845) requires; the
/// final page's granule trims the zero padding of the last frame.
pub fn encode_ogg_opus(interleaved: &[f32], channels: u16, rate: u32) -> Result<Vec<u8>, String> {
    use audiopus::{coder::Encoder, Application, Bitrate, Channels, SampleRate};
    let sr = match rate {
        8000 => SampleRate::Hz8000,
        12000 => SampleRate::Hz12000,
        16000 => SampleRate::Hz16000,
        24000 => SampleRate::Hz24000,
        48000 => SampleRate::Hz48000,
        r => return Err(format!("opus: unsupported rate {r} (use 8/12/16/24/48 kHz)")),
    };
    let ch = match channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        c => return Err(format!("opus: unsupported channel count {c}")),
    };
    let mut enc = Encoder::new(sr, ch, Application::Audio).map_err(|e| format!("opus encoder: {e:?}"))?;
    let _ = enc.set_bitrate(Bitrate::BitsPerSecond(if channels == 2 { 96_000 } else { 64_000 }));
    let _ = enc.set_complexity(10);
    let to48 = 48_000 / rate;
    let pre_skip = (enc.lookahead().unwrap_or(0) * to48) as u16;

    let mut w = ogg::PacketWriter::new(Vec::new());
    let serial = 0x0D2A_6011u32;
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(channels as u8);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    w.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0).map_err(|e| e.to_string())?;
    let vendor = b"unendlich";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    w.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0).map_err(|e| e.to_string())?;

    let frame = (rate / 50) as usize; // 20 ms per channel
    let per = frame * channels as usize;
    let total_frames = interleaved.len() / channels as usize;
    let n_packets = total_frames.div_ceil(frame).max(1);
    let mut buf = vec![0.0f32; per];
    let mut out = vec![0u8; 4000];
    for i in 0..n_packets {
        let start = i * per;
        let end = (start + per).min(interleaved.len());
        buf.iter_mut().for_each(|s| *s = 0.0);
        if start < end {
            buf[..end - start].copy_from_slice(&interleaved[start..end]);
        }
        let n = enc.encode_float(&buf, &mut out).map_err(|e| format!("opus encode: {e:?}"))?;
        let last = i + 1 == n_packets;
        let granule = if last {
            pre_skip as u64 + total_frames as u64 * to48 as u64
        } else {
            pre_skip as u64 + ((i + 1) * frame) as u64 * to48 as u64
        };
        let info = if last { ogg::PacketWriteEndInfo::EndStream } else { ogg::PacketWriteEndInfo::NormalPacket };
        w.write_packet(out[..n].to_vec(), serial, info, granule).map_err(|e| e.to_string())?;
    }
    Ok(w.into_inner())
}

/// Decode-once looping cursor over an in-memory PCM buffer. Loops by wrapping the
/// read index — NEVER re-decodes and never reallocates. Cloning is O(1) (Arc).
#[derive(Clone)]
pub struct PcmLoop {
    pcm: Arc<[i16]>,
    pos: usize,
    channels: u16,
    rate: u32,
}

impl PcmLoop {
    pub fn new(pcm: Arc<[i16]>, channels: u16, rate: u32) -> Self {
        Self { pcm, pos: 0, channels, rate }
    }
    #[inline]
    pub fn next_sample(&mut self) -> i16 {
        if self.pcm.is_empty() {
            return 0;
        }
        let s = self.pcm[self.pos];
        self.pos += 1;
        if self.pos >= self.pcm.len() {
            self.pos = 0;
        }
        s
    }
}

impl Iterator for PcmLoop {
    type Item = i16;
    #[inline]
    fn next(&mut self) -> Option<i16> {
        Some(self.next_sample())
    }
}

#[cfg(feature = "audio")]
impl rodio::Source for PcmLoop {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels.max(1) }
    fn sample_rate(&self) -> u32 { self.rate }
    fn total_duration(&self) -> Option<std::time::Duration> { None }
}

/// Output stream + one decoded, looping sink per stem.
#[cfg(feature = "audio")]
/// r185: sample-synchronous stem mixer. All stems advance in ONE next()
/// stream — they cannot drift. Gains are read per CHUNK of samples (every
/// 512) to keep the hot loop cheap while staying click-free.
pub struct StemGroup {
    loops: Vec<PcmLoop>,
    gains: Arc<Vec<std::sync::atomic::AtomicU32>>,
    cached: Vec<f32>,
    countdown: u32,
    channels: u16,
    rate: u32,
    /// r186: per-stem mild saturation drive (randomized 1.05..1.25 once).
    drives: Vec<f32>,
    /// r186: unified group reverb — two feedback combs + one allpass
    /// (Schroeder-lite), parameters randomized once per session.
    comb1: Vec<f32>, comb2: Vec<f32>, ap: Vec<f32>,
    c1i: usize, c2i: usize, api: usize,
    fb1: f32, fb2: f32, apg: f32, wet: f32,
}

impl StemGroup {
    pub fn new(
        loops: Vec<PcmLoop>,
        gains: Arc<Vec<std::sync::atomic::AtomicU32>>,
        channels: u16,
        rate: u32,
    ) -> Self {
        let n = loops.len();
        // session-random but deterministic-per-launch DSP flavor
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(7);
        let rnd = |k: u64| {
            let x = seed.wrapping_mul(6364136223846793005).wrapping_add(k * 0x9e3779b97f4a7c15);
            ((x >> 33) as f32 / u32::MAX as f32).fract()
        };
        let drives: Vec<f32> = (0..n).map(|i| 1.05 + 0.20 * rnd(i as u64 + 1)).collect();
        let c1 = (rate as f32 * (0.029 + 0.012 * rnd(11)) ) as usize * channels as usize;
        let c2 = (rate as f32 * (0.041 + 0.014 * rnd(12)) ) as usize * channels as usize;
        let al = (rate as f32 * 0.005) as usize * channels as usize;
        Self {
            loops, gains, cached: vec![0.0; n], countdown: 0, channels, rate,
            drives,
            comb1: vec![0.0; c1.max(64)], comb2: vec![0.0; c2.max(64)], ap: vec![0.0; al.max(32)],
            c1i: 0, c2i: 0, api: 0,
            fb1: 0.72 + 0.08 * rnd(21), fb2: 0.66 + 0.08 * rnd(22),
            apg: 0.5, wet: 0.16 + 0.06 * rnd(23),
        }
    }
}

impl Iterator for StemGroup {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        if self.countdown == 0 {
            for (i, g) in self.gains.iter().enumerate() {
                self.cached[i] = f32::from_bits(g.load(std::sync::atomic::Ordering::Relaxed));
            }
            self.countdown = 512;
        }
        self.countdown -= 1;
        let mut acc = 0.0f32;
        for (i, l) in self.loops.iter_mut().enumerate() {
            // r186: mild per-stem saturation (soft knee; drive ~1.1x).
            let x = l.next_sample() as f32 * self.cached[i] / 32768.0;
            let d = self.drives[i];
            let sat = (x * d) / (1.0 + 0.28 * (x * d).abs());
            acc += sat;
        }
        // r186: UNIFIED group reverb (the whole group shares one time/
        // space — stems stay phase-coherent inside it).
        let dry = acc;
        let c1 = self.comb1[self.c1i];
        self.comb1[self.c1i] = dry + c1 * self.fb1;
        self.c1i = (self.c1i + 1) % self.comb1.len();
        let c2 = self.comb2[self.c2i];
        self.comb2[self.c2i] = dry + c2 * self.fb2;
        self.c2i = (self.c2i + 1) % self.comb2.len();
        let rin = (c1 + c2) * 0.5;
        let a = self.ap[self.api];
        let aout = -self.apg * rin + a;
        self.ap[self.api] = rin + self.apg * aout;
        self.api = (self.api + 1) % self.ap.len();
        let mixed = dry * (1.0 - self.wet) + aout * self.wet;
        // r185: brick limiter on the group bus — the sum never clips.
        Some((mixed * 30000.0).clamp(-32000.0, 32000.0) as i16)
    }
}

impl rodio::Source for StemGroup {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.rate }
    fn total_duration(&self) -> Option<std::time::Duration> { None }
}

pub struct SpatialMusic {
    _stream: rodio::OutputStream,
    sinks: Vec<rodio::Sink>,
    stems: Vec<StemSource>,
    /// r185: per-stem gain atomics shared with the StemGroup source.
    stem_gains: Option<Arc<Vec<std::sync::atomic::AtomicU32>>>,
    /// c21: per-stem scale from the director (`music_director::stem_mix`), parallel to
    /// `stems`, multiplied into the positional mix in `update`. 1.0 each until set.
    stem_mix: Vec<std::sync::atomic::AtomicU32>,
    falloff: f32,
    spatial: bool,
    /// r181: master volume factor (f32 bits) — applied INSIDE update()
    /// for spatial mode so positional gains and master never fight.
    master_bits: std::sync::atomic::AtomicU32,
}

#[cfg(feature = "audio")]
impl SpatialMusic {
    /// Spatial menu music: one decoded-once looping sink per stem; volumes driven by
    /// listener position via the mixer. Skips stems that fail to decode.
    pub fn start_spatial(stem_paths: &[(&'static str, std::path::PathBuf)]) -> Option<Self> {
        let (stream, handle) = rodio::OutputStream::try_default().ok()?;
        // r185: decode every stem, REQUIRE matching rate+channels (split
        // exports share format), then mix in ONE StemGroup on ONE sink —
        // sample-locked by construction (the out-of-sync fix).
        let mut loops = Vec::new();
        let mut stems = Vec::new();
        let mut fmt: Option<(u16, u32)> = Option::None;
        for s in menu_stem_layout() {
            if let Some((_, p)) = stem_paths.iter().find(|(n, _)| *n == s.name) {
                if let Ok(pcm) = decode_opus_file(p) {
                    match fmt {
                        Option::None => fmt = Some((pcm.channels, pcm.rate)),
                        Some(f) if f != (pcm.channels, pcm.rate) => {
                            log::warn!("audio: stem '{}' format mismatch — skipped", s.name);
                            continue;
                        }
                        _ => {}
                    }
                    loops.push(PcmLoop::new(pcm.samples.clone(), pcm.channels, pcm.rate));
                    stems.push(s);
                }
            }
        }
        let (channels, rate) = fmt?;
        if loops.is_empty() {
            return Option::None;
        }
        let gains: Arc<Vec<std::sync::atomic::AtomicU32>> = Arc::new(
            (0..loops.len())
                .map(|_| std::sync::atomic::AtomicU32::new(0.0f32.to_bits()))
                .collect(),
        );
        let sink = rodio::Sink::try_new(&handle).ok()?;
        sink.append(StemGroup::new(loops, gains.clone(), channels, rate));
        sink.set_volume(1.0);
        let stem_mix = (0..stems.len()).map(|_| std::sync::atomic::AtomicU32::new(1.0f32.to_bits())).collect();
        Some(Self {
            _stream: stream,
            sinks: vec![sink],
            stems,
            stem_gains: Some(gains),
            stem_mix,
            falloff: 6.0,
            spatial: true,
            master_bits: std::sync::atomic::AtomicU32::new(1.0f32.to_bits()),
        })
    }

    /// Single mixed track (no stems yet): one decoded-once looping sink at fixed gain.
    pub fn start_single(path: &std::path::Path) -> Option<Self> {
        let (stream, handle) = rodio::OutputStream::try_default().ok()?;
        let pcm = decode_opus_file(path).ok()?;
        let sink = rodio::Sink::try_new(&handle).ok()?;
        sink.append(PcmLoop::new(pcm.samples.clone(), pcm.channels, pcm.rate));
        sink.set_volume(0.8);
        Some(Self { _stream: stream, sinks: vec![sink], stems: vec![menu_stem_layout()[0]], stem_gains: Option::None, stem_mix: vec![std::sync::atomic::AtomicU32::new(1.0f32.to_bits())], falloff: 6.0, spatial: false, master_bits: std::sync::atomic::AtomicU32::new(1.0f32.to_bits()) })
    }

    /// Per-frame: crossfade stems by listener position (no-op for single track).
    /// r175: master volume — scales every sink. For single-track playback
    /// this IS the volume; for spatial stems update() re-drives per-stem
    /// volumes, so callers in spatial mode should scale there instead.
    pub fn set_master_volume(&self, v01: f32) {
        let v = v01.clamp(0.0, 1.0);
        self.master_bits
            .store(v.to_bits(), std::sync::atomic::Ordering::Relaxed);
        if !self.spatial {
            for s in &self.sinks {
                s.set_volume(v);
            }
        }
    }

    fn master(&self) -> f32 {
        f32::from_bits(self.master_bits.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Scale each named stem (the director's `stem_mix`); a name not in the layout is
    /// ignored, a stem not named keeps its scale. Applied by the next `update`. A single
    /// mixed track has no stems to scale and plays as it is.
    pub fn set_stem_mix(&self, mix: &[(&str, f32)]) {
        for (i, s) in self.stems.iter().enumerate() {
            if let Some((_, g)) = mix.iter().find(|(n, _)| *n == s.name) {
                self.stem_mix[i].store(g.clamp(0.0, 1.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    pub fn update(&self, listener: [f32; 3]) {
        if !self.spatial {
            return;
        }
        let m = self.master();
        let mut gains = mix_gains(listener, &self.stems, self.falloff);
        for (g, scale) in gains.iter_mut().zip(self.stem_mix.iter()) {
            *g *= f32::from_bits(scale.load(std::sync::atomic::Ordering::Relaxed));
        }
        if let Some(ga) = &self.stem_gains {
            // r185: lockstep group — write per-stem gain atomics; the
            // single sink's volume stays at master separately below.
            for (a, g) in ga.iter().zip(gains.iter()) {
                a.store(g.to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(s) = self.sinks.first() {
                s.set_volume(m);
            }
        } else {
            for (sink, g) in self.sinks.iter().zip(gains) {
                sink.set_volume(g * m);
            }
        }
    }
}


// ─────────────────────────────────────────────────────────────────────────────
// One-shot sound effects (the dragon roar is the first user)
// ─────────────────────────────────────────────────────────────────────────────
//
// Split for the two costs that differ by platform:
// * RENDER (synthesis + terrain echo) is CPU work of tens to hundreds of ms on
//   a phone, so it never runs on the frame thread: callers submit a closure,
//   one worker thread renders it.
// * PLAY is a hand-off of the finished buffer to rodio's mixer, from the
//   worker, through a cloned `OutputStreamHandle` (Send + Sync).
//
// `rodio::OutputStream` itself is !Send (it owns the cpal stream), so it stays
// in `Sfx`, held by the game loop. This is a SECOND output stream beside the
// music's own; on PipeWire/PulseAudio and ALSA dmix two streams on one device
// mix fine. If the device can't open a second stream, SFX run in dump-only /
// silent mode and say so once.
//
// With no device at all (xvfb soaks) the worker still renders, and
// UNENDLICH_SFX_DUMP=<dir> writes every rendered effect as Ogg-Opus there -- the
// headless way to hear what the game produced.

/// A render job: returns interleaved stereo f32 and its sample rate, or None
/// to play nothing. `label` names the dump file and the log line.
pub struct SfxJob {
    pub label: String,
    pub render: Box<dyn FnOnce() -> Option<(Vec<f32>, u32)> + Send>,
}

pub struct Sfx {
    _stream: Option<rodio::OutputStream>,
    jobs: std::sync::mpsc::SyncSender<SfxJob>,
    master_bits: Arc<std::sync::atomic::AtomicU32>,
    pub has_device: bool,
}

/// At most this many effects waiting to render; more are dropped (a flood of
/// roars at once is noise, not information).
pub const SFX_QUEUE: usize = 4;

impl Sfx {
    /// Open an output stream if there is one and start the render worker.
    /// Never fails: without a device it renders for UNENDLICH_SFX_DUMP only.
    pub fn start() -> Sfx {
        let (stream, handle) = match rodio::OutputStream::try_default() {
            Ok((s, h)) => (Some(s), Some(h)),
            Err(e) => {
                log::warn!("sfx: no output stream ({e}); effects render but are not played");
                (None, None)
            }
        };
        let has_device = handle.is_some();
        let dump = std::env::var("UNENDLICH_SFX_DUMP").ok().map(std::path::PathBuf::from);
        if let Some(d) = &dump {
            let _ = std::fs::create_dir_all(d);
            log::info!("sfx: UNENDLICH_SFX_DUMP -- writing every effect to {}", d.display());
        }
        let master_bits = Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits()));
        let master = master_bits.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<SfxJob>(SFX_QUEUE);
        let spawned = std::thread::Builder::new().name("sfx-render".into()).spawn(move || {
            let mut n = 0u32;
            while let Ok(job) = rx.recv() {
                let t0 = std::time::Instant::now();
                let Some((samples, rate)) = (job.render)() else { continue };
                let secs = samples.len() as f32 / 2.0 / rate.max(1) as f32;
                log::info!(
                    "sfx: {} rendered in {:.0} ms ({:.2} s of audio)",
                    job.label, t0.elapsed().as_secs_f32() * 1000.0, secs
                );
                if let Some(d) = &dump {
                    n += 1;
                    let safe: String = job.label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                    let path = d.join(format!("{n:03}_{safe}.opus"));
                    match encode_ogg_opus(&samples, 2, rate) {
                        Ok(bytes) => {
                            if let Err(e) = std::fs::write(&path, bytes) {
                                log::warn!("sfx: dump {} failed: {e}", path.display());
                            }
                        }
                        Err(e) => log::warn!("sfx: dump {} not encoded: {e}", path.display()),
                    }
                }
                if let Some(h) = &handle {
                    use rodio::Source;
                    let m = f32::from_bits(master.load(std::sync::atomic::Ordering::Relaxed));
                    let src = rodio::buffer::SamplesBuffer::new(2, rate, samples).amplify(m);
                    if let Err(e) = h.play_raw(src) {
                        log::warn!("sfx: play failed: {e}");
                    }
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("sfx: could not start the render thread ({e}); effects are off");
        }
        Sfx { _stream: stream, jobs: tx, master_bits, has_device }
    }

    /// Queue an effect. Returns false if the queue is full (it was dropped).
    pub fn submit(&self, job: SfxJob) -> bool {
        match self.jobs.try_send(job) {
            Ok(()) => true,
            Err(std::sync::mpsc::TrySendError::Full(j)) => {
                log::info!("sfx: queue full, dropped {}", j.label);
                false
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    /// Master volume for effects (applied when each effect starts).
    pub fn set_master_volume(&self, v01: f32) {
        self.master_bits
            .store(v01.clamp(0.0, 1.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_loop_wraps_seamlessly_and_shares_buffer() {
        let buf: Arc<[i16]> = vec![10, 20, 30, 40].into();
        let mut l = PcmLoop::new(buf.clone(), 2, 48000);
        let got: Vec<i16> = (0..10).map(|_| l.next_sample()).collect();
        assert_eq!(got, vec![10, 20, 30, 40, 10, 20, 30, 40, 10, 20], "seamless index-wrap loop");
        assert_eq!(Arc::strong_count(&buf), 2, "loop shares the buffer (decode-once, no copy)");
    }

    #[test]
    fn empty_buffer_is_silent_not_panicking() {
        let mut l = PcmLoop::new(Vec::<i16>::new().into(), 1, 48000);
        assert_eq!(l.next_sample(), 0);
        assert_eq!(l.next_sample(), 0);
    }

    #[test]
    fn iterator_matches_next_sample() {
        let buf: Arc<[i16]> = vec![5, 6, 7].into();
        let l = PcmLoop::new(buf, 1, 48000);
        let got: Vec<i16> = l.take(7).collect();
        assert_eq!(got, vec![5, 6, 7, 5, 6, 7, 5]);
    }

    #[test]
    fn opus_encode_roundtrips_through_our_decoder() {
        // 0.5 s of a 440 Hz tone, stereo, 24 kHz.
        let rate = 24_000u32;
        let n = rate as usize / 2;
        let pcm: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin();
                [s, s * 0.5]
            })
            .collect();
        let bytes = encode_ogg_opus(&pcm, 2, rate).unwrap();
        assert_eq!(&bytes[0..4], b"OggS");
        let dir = std::env::temp_dir().join("unendlich_opus_rt.opus");
        std::fs::write(&dir, &bytes).unwrap();
        let back = decode_opus_file(&dir).unwrap();
        assert_eq!(back.channels, 2);
        assert_eq!(back.rate, 48_000); // opus always decodes at 48 kHz here
        // Length: 0.5 s at 48 kHz, plus the codec's padding/look-ahead.
        let frames = back.samples.len() / 2;
        assert!((24_000..24_000 + 2_000).contains(&frames), "{frames} frames");
        // The tone survives: RMS of each channel near the input's (0.35 / 0.18).
        let rms = |ch: usize| {
            let v: Vec<f32> = back.samples.chunks(2).skip(2400).take(19_000).map(|f| f[ch] as f32 / 32768.0).collect();
            (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
        };
        assert!((rms(0) - 0.354).abs() < 0.05, "L rms {}", rms(0));
        assert!((rms(1) - 0.177).abs() < 0.05, "R rms {}", rms(1));
        assert!(encode_ogg_opus(&pcm, 2, 44_100).is_err());
    }
}
