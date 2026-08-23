//! miclisten — the microphone shell around [`crate::micsample`] and
//! [`crate::micsession`].
//!
//! Deliberately thin, and thin is the design rather than a shortcut. The two
//! modules it joins are pure and tested against synthetic input at thousands of
//! times real speed; everything here needs a microphone and a person, so
//! anything decided here is untestable. What lives in this file is only what
//! cannot live anywhere else: opening a stream, blocking on it, and the ring
//! that turns a callback's arbitrary buffer sizes into fixed hop-sized frames.
//!
//! Available under the `app` feature, with cpal.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc;

use crate::config::Config;
use crate::micsample::{Decision, Gate, Sampler};
use crate::micsession::{Session, StopPolicy, Stopped};

/// Capture until the policy says stop. Returns the session directory.
pub fn listen(
    cfg: &Config,
    root: &std::path::Path,
    policy: StopPolicy,
    gate: Gate,
    dry_run: bool,
) -> Result<std::path::PathBuf> {
    // Written against cpal 0.18, matching player.rs in this crate rather than
    // from memory: `default_input_device` returns Option not Result,
    // `sample_rate()` is a plain u32 (not a tuple struct), and a
    // `SupportedStreamConfig` converts into a `StreamConfig` by `into()`. I got
    // all three wrong first and the compiler caught all three.
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default input device — is a microphone connected and permitted?")?;
    let supported = device.default_input_config().context("no default input config")?;
    let in_sr = supported.sample_rate();
    let sample_format = supported.sample_format();
    // cpal 0.18 exposes `description()`, not `name()` — checked in the
    // installed traits.rs rather than recalled.
    let name = device.description().map(|d| format!("{d:?}")).unwrap_or_else(|_| "<unnamed>".into());
    let stream_cfg: cpal::StreamConfig = supported.into();
    let channels = stream_cfg.channels as usize;

    // SAY WHAT IS BEING RECORDED, WHERE, AND FOR HOW LONG, before any of it
    // happens. An always-on microphone that starts quietly is the thing this
    // whole design is trying not to be.
    let stamp = timestamp();
    let mut session = Session::create(root, &stamp, cfg.n_mels, cfg.time_frames)?;
    println!("[listen] device: {name} @ {in_sr} Hz, {channels} ch");
    println!("[listen] writing mel crops to {}", session.dir().display());
    match policy.max_seconds {
        Some(s) => println!("[listen] will stop after {:.1} h, or {} crops, or a STOP file",
                            s / 3600.0, policy.budget),
        None => println!("[listen] NO TIME LIMIT — stops at {} crops or a STOP file",
                         policy.budget),
    }
    println!("[listen] crops are mel spectrograms, and mel is NOT anonymised: \
this crate's Griffin-Lim can turn them back into audible sound.");
    if dry_run {
        println!("[listen] DRY RUN — nothing will be written. Tune with \
--onset-mult / --refractory / --novelty, then run again without --dry-run.");
    }

    // The audio thread must not allocate, block, or do FFTs, so it does the
    // least possible: interleaved input -> mono f32 -> channel. Everything else
    // happens on this thread.
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let err_fn = |e| eprintln!("[listen] stream error: {e}");
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(to_mono(data, channels));
            },
            err_fn, None)?,
        f => anyhow::bail!(
            "unsupported input sample format {f:?} — only f32 is wired; \
convert with a virtual device, or add the arm here"),
    };
    stream.play()?;

    let mel = crate::dsp::Mel::new(cfg);
    let mut sampler = Sampler::new(gate, cfg.n_mels);
    let start = std::time::Instant::now();
    // Resampling is linear and per-block, matching audio_io::resample_linear —
    // the same path the file dataset takes, so a captured crop and a decoded
    // one are comparable.
    let mut pending: Vec<f32> = Vec::new();

    let reason = loop {
        let elapsed = start.elapsed().as_secs_f64();
        let asked = crate::train::stop_requested(cfg);
        if let Some(r) = policy.should_stop(elapsed, sampler.keeps, asked) {
            break r;
        }
        let Ok(block) = rx.recv_timeout(std::time::Duration::from_millis(250)) else { continue };

        let block = if in_sr == cfg.sr { block } else {
            crate::audio_io::resample_linear(&block, in_sr, cfg.sr)
        };
        pending.extend_from_slice(&block);

        // Whole hops only; the remainder stays for the next block.
        let need = cfg.n_fft;
        while pending.len() >= need {
            let (frames, _) = mel.wav_to_mel(&pending[..need]);
            // wav_to_mel returns (n_mels, T) row-major; feed each column.
            let t = frames.len() / cfg.n_mels;
            for f in 0..t {
                let col: Vec<f32> = (0..cfg.n_mels).map(|m| frames[m * t + f]).collect();
                if sampler.push(&col) == Decision::Keep {
                    if dry_run {
                        // Counted by the sampler either way; the session stays
                        // empty so the stop policy's budget still applies to
                        // what WOULD have been written.
                        println!("[listen] would keep crop {} ({:.0} s in)",
                                 sampler.keeps, start.elapsed().as_secs_f64());
                    } else {
                        match session.write_crop(&sampler.crop()) {
                            Ok(_) => println!("[listen] kept crop {} ({:.0} s in)",
                                              session.written(), start.elapsed().as_secs_f64()),
                            Err(e) => eprintln!("[listen] could not write crop: {e}"),
                        }
                    }
                }
            }
            pending.drain(..cfg.hop.min(pending.len()));
        }
    };

    drop(stream);
    let why = match reason {
        Stopped::Deadline => "time limit reached",
        Stopped::Budget => "crop budget reached",
        Stopped::Asked => "STOP file",
    };
    println!("[listen] stopped: {why}.");
    // The tally, always — a session that kept plenty is as worth explaining as
    // one that kept nothing, because the reject mix is how you tell a
    // well-tuned gate from a lucky one.
    println!("[listen] {}", sampler.rejected.advice(sampler.onsets, sampler.keeps));
    if dry_run {
        let _ = std::fs::remove_dir(session.dir()); // empty; leave no litter
        println!("[listen] dry run — nothing written.");
    } else {
        println!("[listen] {} crop(s) in {}", session.written(), session.dir().display());
        println!("[listen] train on it with: beatspace train --data-dir {}",
                 session.dir().display());
    }
    Ok(session.dir().to_path_buf())
}

fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// A sortable local-ish stamp for the session directory. Seconds since the
/// epoch rather than a formatted date: no chrono dependency, and it sorts.
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // SECONDS AND MILLIS. `Session::create` now refuses an existing directory
    // rather than overwriting a previous run's crops, which turns a
    // second-granularity stamp into a real failure: start `listen`, stop it,
    // start it again within the same second and the second run dies at
    // creation. That is a plausible thing to do while tuning with --dry-run.
    //
    // The millisecond keeps the name sortable and makes the collision as
    // unlikely as the refusal assumes. It is not a guarantee — two processes
    // could still land in the same millisecond — and the refusal is what covers
    // that, which is the right division: the stamp makes collisions rare, the
    // refusal makes them harmless.
    format!("session-{}-{:03}", now.as_secs(), now.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_downmix_averages_channels() {
        assert_eq!(to_mono(&[1.0, 3.0, 2.0, 4.0], 2), vec![2.0, 3.0]);
        assert_eq!(to_mono(&[1.0, 2.0], 1), vec![1.0, 2.0]);
    }

    #[test]
    fn a_stamp_is_sortable_and_directory_safe() {
        let s = timestamp();
        assert!(s.starts_with("session-"));
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "a session name becomes a directory: {s}");
    }
}
