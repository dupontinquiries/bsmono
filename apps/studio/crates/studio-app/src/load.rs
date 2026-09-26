//! load — a project's media, decoded (and converted to the render rate)
//! on every core.
//!
//! Decoding and resampling are per file and independent, and they were the
//! slow half of a short render: A Top A Tower's 142 files took 25.7 s to
//! decode one after another, and converting Goat's 21 to 16 kHz another
//! 7.1 s. Workers take the next file from a shared counter, so a long file
//! does not hold up the rest.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use studio_audio::SourceAudio;
use studio_core::{Project, SourceId};

/// Every file source of `project` (paths relative to `base`), decoded and --
/// when `rate` is given and differs -- converted to it with the band-limited
/// resampler. Returns the audio and the paths that could not be decoded.
/// `threads` 0 = the machine's parallelism.
pub fn load_sources(project: &Project, base: &Path, rate: Option<u32>, threads: usize)
    -> (BTreeMap<SourceId, SourceAudio>, Vec<String>)
{
    let jobs: Vec<(SourceId, String)> = project.sources.values().filter(|s| !s.is_generator())
        .map(|s| (s.id, s.path.clone())).collect();
    let n = if threads == 0 { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) } else { threads };
    let next = AtomicUsize::new(0);
    let results: Vec<Vec<(SourceId, Result<SourceAudio, String>)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..n.min(jobs.len()).max(1)).map(|_| sc.spawn(|| {
            let mut mine = Vec::new();
            loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some((id, path)) = jobs.get(k) else { break };
                let r = studio_media::decode_file(&base.join(path)).map_err(|_| path.clone()).map(|a| match rate {
                    Some(r) if a.sample_rate != r => studio_audio::resample::resample_audio(&a, r),
                    _ => a,
                });
                mine.push((*id, r));
            }
            mine
        })).collect();
        handles.into_iter().map(|h| h.join().expect("a loader thread")).collect()
    });
    let (mut audio, mut failed) = (BTreeMap::new(), Vec::new());
    for (id, r) in results.into_iter().flatten() {
        match r { Ok(a) => { audio.insert(id, a); } Err(p) => failed.push(p) }
    }
    failed.sort();
    (audio, failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Op, Source, Ticks};

    /// Many threads or one, the same audio; a missing file is reported by
    /// path; with a rate, every source comes back at it.
    #[test]
    fn loading_in_parallel_gives_what_loading_serially_does() {
        let dir = std::env::temp_dir().join(format!("studio-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Project::default();
        for k in 0..6u64 {
            let rate = if k % 2 == 0 { 48_000 } else { 44_100 };
            let tone: Vec<f32> = (0..rate / 4).map(|i| ((i as f32) * 0.01 * (k + 1) as f32).sin() * 0.3).collect();
            let name = format!("t{k}.wav");
            studio_audio::export::export_wav_dithered(&tone, 1, rate, k, &dir.join(&name)).unwrap();
            p.apply(&Op::AddSource { source: Source::file(SourceId(k + 1), &name, Ticks::from_seconds(0.25), rate, 1, false) }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source::file(SourceId(99), "missing.wav", Ticks::from_seconds(1.0), 48_000, 1, false) }).unwrap();
        let (one, f1) = load_sources(&p, &dir, None, 1);
        let (many, f4) = load_sources(&p, &dir, None, 4);
        assert_eq!(one.len(), 6);
        assert_eq!(f1, vec!["missing.wav".to_string()]);
        assert_eq!(f4, f1);
        for (id, a) in &one { assert_eq!(a.channels, many[id].channels, "source {id:?}"); }
        let (at16, _) = load_sources(&p, &dir, Some(16_000), 3);
        assert!(at16.values().all(|a| a.sample_rate == 16_000 && (a.frames() as i64 - 4_000).abs() <= 1), "all at 16k, a quarter second each");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
