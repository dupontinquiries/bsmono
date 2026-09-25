//! The transcription loop: the shell around `vidkit-transcribe`.
//!
//! One thread, at low priority. Per chunk it asks `Coverage` what to do next,
//! decodes exactly that stretch of audio at 16 kHz mono f32 with one ffmpeg
//! (`-ss`/`-t` before `-i`, so it seeks rather than decoding from the start),
//! hands it to the `CueSource`, and merges the result. Between chunks it
//! re-reads the mode and the playhead, so a seek redirects it within one chunk
//! and "Pause" stops it within one chunk.
//!
//! It sleeps on a condvar when there is nothing to do and is woken only by
//! something that could change the answer: a mode change, the playhead
//! crossing into another chunk, or shutdown. No polling.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use vidkit_transcribe::coverage::{Coverage, Mode};
use vidkit_transcribe::transcript::Transcript;
use vidkit_transcribe::{CueSource, SAMPLE_RATE};

/// Where the audio comes from: a path, or a signed URL with its headers.
#[derive(Clone, Debug)]
pub struct AudioIn {
    pub input: String,
    pub headers: Vec<(String, String)>,
}

/// ffmpeg arguments for one chunk of whisper-ready audio. Pure.
pub fn chunk_audio_args(src: &AudioIn, start: Duration, len: Duration) -> Vec<String> {
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into()];
    if src.input.starts_with("http://") || src.input.starts_with("https://") {
        if !src.headers.is_empty() {
            a.push("-headers".into());
            a.push(src.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect());
        }
        for f in ["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"] {
            a.push(f.into());
        }
    }
    // Both before -i: seek the container, then read only this long. After
    // -i, ffmpeg would decode everything before the chunk and discard it --
    // for the last chunk of a lecture, the whole lecture.
    a.push("-ss".into());
    a.push(format!("{:.3}", start.as_secs_f64()));
    a.push("-t".into());
    a.push(format!("{:.3}", len.as_secs_f64()));
    a.push("-i".into());
    a.push(src.input.clone());
    for f in ["-vn", "-sn", "-dn", "-ac", "1", "-ar"] {
        a.push(f.into());
    }
    a.push(SAMPLE_RATE.to_string());
    for f in ["-f", "f32le", "-acodec", "pcm_f32le", "-"] {
        a.push(f.into());
    }
    a
}

/// Decode one chunk. Empty means the media ended before `start`.
pub fn decode_chunk(src: &AudioIn, start: Duration, len: Duration) -> Result<Vec<f32>, String> {
    let mut child = Command::new("ffmpeg")
        .args(chunk_audio_args(src, start, len))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg (transcription audio): {e}"))?;
    let mut bytes = Vec::with_capacity((len.as_secs_f64() * SAMPLE_RATE as f64 * 4.0) as usize + 64);
    child.stdout.take().map(|mut o| o.read_to_end(&mut bytes)).transpose().map_err(|e| e.to_string())?;
    let _ = child.wait();
    Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// What the UI reads.
pub struct Shared {
    pub transcript: Transcript,
    pub coverage: Coverage,
    /// What the loop is doing, in words.
    pub status: String,
    /// Bumped on every change, so the UI recomputes chapters only then.
    pub version: u64,
    /// Seconds of compute per second of audio for the last chunk. Below 1 the
    /// transcript outruns playback; above it, it falls behind.
    pub realtime_factor: Option<f32>,
    playhead: Duration,
    stop: bool,
}

pub type Factory = Box<dyn FnOnce() -> Result<Box<dyn CueSource>, String> + Send>;

pub struct Worker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// Start. `make` runs ON the worker thread: loading a whisper model takes
    /// seconds and must not freeze the window.
    pub fn start(make: Factory, audio: AudioIn, total: Option<Duration>, cache: Option<PathBuf>) -> Self {
        let mut coverage = Coverage::new(total);
        let mut transcript = Transcript::new("");
        if let Some(text) = cache.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            if let Ok(c) = Transcript::from_cache(&text) {
                coverage.restore_done(c.done_for(coverage.chunk_len()));
                transcript = c.transcript;
            }
        }
        let shared = Arc::new((
            Mutex::new(Shared {
                transcript,
                coverage,
                status: "starting".into(),
                version: 1,
                realtime_factor: None,
                playhead: Duration::ZERO,
                stop: false,
            }),
            Condvar::new(),
        ));
        let s2 = shared.clone();
        let handle = std::thread::Builder::new()
            .name("vidkit-transcribe".into())
            .spawn(move || run(s2, make, audio, cache))
            .ok();
        Self { shared, handle }
    }

    /// Read the shared state.
    pub fn with<R>(&self, f: impl FnOnce(&Shared) -> R) -> R {
        let g = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&g)
    }

    /// Tell the loop where playback is. Wakes it only when the playhead has
    /// moved into a different chunk -- every frame would be sixty wakeups a
    /// second for nothing.
    pub fn set_playhead(&self, t: Duration) {
        let (m, cv) = &*self.shared;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        let len = g.coverage.chunk_len().as_secs_f64();
        let moved = (g.playhead.as_secs_f64() / len).floor() != (t.as_secs_f64() / len).floor();
        g.playhead = t;
        if moved {
            cv.notify_all();
        }
    }

    pub fn set_mode(&self, mode: Mode) {
        let (m, cv) = &*self.shared;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.coverage.mode = mode;
        g.version += 1;
        cv.notify_all();
    }

    pub fn mode(&self) -> Mode {
        self.with(|s| s.coverage.mode)
    }
}

impl Drop for Worker {
    /// Stop after the current chunk and do not wait for it: a chunk of whisper
    /// can take seconds and closing a video must not.
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        if let Ok(mut g) = m.lock() {
            g.stop = true;
        }
        cv.notify_all();
        drop(self.handle.take());
    }
}

/// Lower this thread's priority, and so ffmpeg's and whisper's threads, which
/// inherit it. Transcription must lose every contest with the playback
/// decoder: a dropped frame is visible, a late chapter is not.
fn be_nice() {
    // SAFETY: setpriority on the calling thread (who = 0) with a constant.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

fn run(shared: Arc<(Mutex<Shared>, Condvar)>, make: Factory, audio: AudioIn, cache: Option<PathBuf>) {
    be_nice();
    let (m, cv) = &*shared;
    let set_status = |s: String| {
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.status = s;
        g.version += 1;
    };
    let mut source = match make() {
        Ok(s) => s,
        Err(e) => return set_status(format!("no transcript: {e}")),
    };
    let name = source.name();

    if let Some(all) = source.complete() {
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.transcript = Transcript::new(name.clone());
        g.transcript.replace_all(all);
        let n = g.coverage.total().map(|t| (t.as_secs_f64() / g.coverage.chunk_len().as_secs_f64()).ceil() as u32).unwrap_or(0);
        g.coverage.restore_done(0..n);
        g.status = format!("{name}: {} cues", g.transcript.cues().len());
        g.version += 1;
        return;
    }

    loop {
        let chunk = {
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if g.stop {
                    return;
                }
                if let Some(c) = g.coverage.next(g.playhead) {
                    g.coverage.begin(&c);
                    g.status = format!("{name}: transcribing {}", crate::fmt_span(c.start, c.end));
                    g.version += 1;
                    break c;
                }
                g.status = match (g.coverage.mode, g.coverage.complete()) {
                    (_, true) => format!("{name}: transcript complete"),
                    (Mode::Paused, _) => format!("{name}: paused"),
                    _ => format!("{name}: caught up with the playhead"),
                };
                g.version += 1;
                g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };

        let (a, b) = chunk.decode_span();
        let began = Instant::now();
        let result = decode_chunk(&audio, a, b.saturating_sub(a));
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(pcm) if pcm.is_empty() => g.coverage.mark_end(chunk.index),
            Ok(pcm) => {
                drop(g);
                let heard = pcm.len() as f32 / SAMPLE_RATE as f32;
                let cues = source.transcribe(&pcm, a);
                g = m.lock().unwrap_or_else(|e| e.into_inner());
                match cues {
                    Ok(c) => {
                        g.transcript.source = name.clone();
                        g.transcript.accept(&chunk, c);
                    }
                    Err(e) => g.status = format!("{name}: chunk {} failed: {e}", chunk.index),
                }
                g.realtime_factor = Some(began.elapsed().as_secs_f32() / heard.max(0.001));
                g.coverage.finish(chunk.index);
            }
            Err(e) => {
                g.status = format!("{name}: {e}");
                g.coverage.finish(chunk.index);
            }
        }
        g.version += 1;
        if let Some(p) = &cache {
            let text = g.transcript.to_cache(g.coverage.chunk_len(), g.coverage.done_indices());
            drop(g);
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // Write-then-rename: a crash mid-write must not leave a cache that
            // parses as a shorter transcript and marks chunks done.
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
    }
}

/// FNV-1a, 64-bit. Not `DefaultHasher`: its output is allowed to change
/// between Rust releases, and a toolchain update would silently orphan every
/// cached transcript -- hours of whisper -- without an error anywhere.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `~/.cache/vidkit/transcripts/<hash>.tsv`, keyed by path, size and mtime,
/// so an edited file with the same name is transcribed again.
pub fn cache_path(video: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(video).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let abs = std::fs::canonicalize(video).ok()?;
    let key = format!("{}\0{}\0{}", abs.display(), meta.len(), mtime);
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("vidkit/transcripts").join(format!("{:016x}.tsv", fnv1a(key.as_bytes()))))
}

/// The source for a video: a sidecar subtitle file if one exists (free),
/// else whisper when built in. Runs on the worker thread.
pub fn source_factory(video: Option<PathBuf>) -> Factory {
    Box::new(move || {
        if let Some(v) = &video {
            for p in vidkit_transcribe::subtitles::sidecar_candidates(v) {
                if p.is_file() {
                    if let Ok(s) = vidkit_transcribe::subtitles::SubtitleSource::open(&p) {
                        return Ok(Box::new(s) as Box<dyn CueSource>);
                    }
                }
            }
        }
        whisper_source()
    })
}

/// The source for a stream whose page offered captions: fetch the VTT
/// (tens of KB for an hour) instead of running whisper for tens of minutes.
/// A failed fetch falls through to whisper rather than to no transcript.
pub fn caption_factory(caption: vidkit_ytdlp::Caption, headers: Vec<(String, String)>) -> Factory {
    Box::new(move || match fetch_text(&caption.url, &headers) {
        Ok(text) => {
            let kind = if caption.automatic { "auto-captions" } else { "subtitles" };
            let src = vidkit_transcribe::subtitles::SubtitleSource::from_text(format!("{kind}:{}", caption.lang), &text);
            if src.all().is_empty() {
                whisper_source().map_err(|e| format!("the {kind} had no cues, and {e}"))
            } else {
                Ok(Box::new(src) as Box<dyn CueSource>)
            }
        }
        Err(e) => whisper_source().map_err(|w| format!("captions: {e}; {w}")),
    })
}

/// GET a small text resource over the app's own transports. A range from
/// zero to 16 MB: a server clamps it to the real length (RFC 9110 14.1.2),
/// and a caption file bigger than that is not a caption file.
pub fn fetch_text(url: &str, headers: &[(String, String)]) -> Result<String, String> {
    let mut t = crate::io::transport_for(url)?;
    let req = vidkit_ytdlp::RangeRequest {
        url: url.to_string(),
        headers: headers.iter().cloned().collect(),
        start: 0,
        end: 16 * 1024 * 1024 - 1,
    };
    let body = vidkit_ytdlp::HttpTransport::fetch(&mut t, &req).map_err(|e| format!("{e:?}"))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The loaded whisper model, shared by workers (model_pool): the stream's
/// worker hands it to the file's worker when a download finishes, instead of
/// the second one loading its own.
#[cfg(feature = "whisper")]
static WHISPER: crate::model_pool::Pool = crate::model_pool::Pool::new();

#[cfg(feature = "whisper")]
fn whisper_source() -> Result<Box<dyn CueSource>, String> {
    // All but one core: the playback decoder keeps one, and whisper past
    // the big cores gets slower rather than faster on a phone.
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let threads = (n.saturating_sub(1)).clamp(1, 4) as i32;
    // Up to 20 s for a worker still mid-chunk to hand it over: a CPU whisper
    // chunk takes seconds, a fresh load about as long and twice the memory.
    WHISPER.take_or(std::time::Duration::from_secs(20), || {
        vidkit_transcribe::whisper::WhisperSource::from_store(threads).map(|s| Box::new(s) as Box<dyn CueSource>)
    })
}

/// Let go of a whisper model nobody has used for two minutes (the UI loop).
pub fn trim_idle_model() {
    #[cfg(feature = "whisper")]
    WHISPER.trim(std::time::Duration::from_secs(120));
}

#[cfg(not(feature = "whisper"))]
fn whisper_source() -> Result<Box<dyn CueSource>, String> {
    Err("no subtitles beside the file, and this build has no whisper (feature `whisper`)".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidkit_transcribe::Cue;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }

    #[test]
    fn chunk_seek_and_length_precede_the_input() {
        let a = chunk_audio_args(&AudioIn { input: "x.mp4".into(), headers: vec![] }, Duration::from_secs(90), Duration::from_secs(32));
        let at = |s: &str| a.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s}: {a:?}"));
        assert!(at("-ss") < at("-i") && at("-t") < at("-i"), "{a:?}");
        assert_eq!(a[at("-ss") + 1], "90.000");
        assert_eq!(a[at("-t") + 1], "32.000");
        assert_eq!(a[at("-ar") + 1], "16000");
        assert_eq!(a[at("-ac") + 1], "1");
        assert_eq!(a[at("-f") + 1], "f32le");
        assert!(!a.iter().any(|x| x == "-reconnect"), "http options on a path kill ffmpeg");
        let h = chunk_audio_args(&AudioIn { input: "https://x/a".into(), headers: vec![("A".into(), "b".into())] }, Duration::ZERO, Duration::from_secs(1));
        assert!(h.iter().position(|x| x == "-headers").unwrap() < h.iter().position(|x| x == "-i").unwrap());
    }

    /// A real chunk from the middle of a file: the right length, at 16 kHz,
    /// and not silence -- all three break silently otherwise.
    #[test]
    fn a_chunk_decodes_to_the_right_amount_of_audible_16k_mono() {
        if !have_ffmpeg() { return; }
        let p = std::env::temp_dir().join(format!("vk_tr_{}.mkv", std::process::id()));
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=300:sample_rate=48000:duration=10"]).arg(&p).status().unwrap().success());
        let src = AudioIn { input: p.to_string_lossy().into(), headers: vec![] };
        let pcm = decode_chunk(&src, Duration::from_secs(4), Duration::from_secs(3)).unwrap();
        let secs = pcm.len() as f64 / 16_000.0;
        assert!((secs - 3.0).abs() < 0.05, "{secs} s");
        let peak = pcm.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05 && peak <= 1.0, "peak {peak}: silent or not float");
        // Past the end: empty, which Coverage reads as "the media ends here".
        assert!(decode_chunk(&src, Duration::from_secs(30), Duration::from_secs(3)).unwrap().is_empty());
        let _ = std::fs::remove_file(&p);
    }

    /// A scripted source standing in for whisper: one cue per chunk, at the
    /// chunk's start, so the test can see which chunks ran in which order.
    struct Script(Arc<Mutex<Vec<Duration>>>);
    impl CueSource for Script {
        fn transcribe(&mut self, pcm: &[f32], offset: Duration) -> Result<Vec<Cue>, String> {
            self.0.lock().unwrap().push(offset);
            let len = Duration::from_secs_f64(pcm.len() as f64 / 16_000.0);
            Ok(vec![Cue { start: offset, end: offset + len.min(Duration::from_secs(1)), text: format!("chunk at {}", offset.as_secs()) }])
        }
        fn name(&self) -> String { "script".into() }
    }

    fn wait_until(w: &Worker, f: impl Fn(&Shared) -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if w.with(&f) { return true; }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The whole loop against real ffmpeg: playhead first, then forward, then
    /// the start; the transcript complete and sorted; the cache written and
    /// read back by a second worker that then has nothing to do.
    #[test]
    fn the_loop_runs_playhead_first_and_resumes_from_its_cache() {
        if !have_ffmpeg() { return; }
        let dir = std::env::temp_dir().join(format!("vk_loop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.mkv");
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=100"]).arg(&p).status().unwrap().success());
        let cache = dir.join("t.tsv");
        let order = Arc::new(Mutex::new(Vec::new()));
        let o2 = order.clone();
        let src = AudioIn { input: p.to_string_lossy().into(), headers: vec![] };
        let w = Worker::start(Box::new(move || Ok(Box::new(Script(o2)) as Box<dyn CueSource>)), src.clone(), Some(Duration::from_secs(100)), Some(cache.clone()));
        w.set_playhead(Duration::from_secs(65)); // chunk 2 of 0..3
        assert!(wait_until(&w, |s| s.coverage.complete()), "never completed: {}", w.with(|s| s.status.clone()));
        let got: Vec<u64> = order.lock().unwrap().iter().map(|d| d.as_secs()).collect();
        assert_eq!(got.len(), 4, "{got:?}");
        // The playhead was set after start, so chunk 0 may have begun first;
        // everything after the first pick follows the playhead.
        assert!(got.ends_with(&[60, 90, 30]) || got == vec![60, 90, 0, 30], "{got:?}");
        assert_eq!(w.with(|s| s.transcript.cues().len()), 4);
        drop(w);
        assert!(cache.is_file(), "cache written");

        let again = Arc::new(Mutex::new(Vec::new()));
        let a2 = again.clone();
        let w2 = Worker::start(Box::new(move || Ok(Box::new(Script(a2)) as Box<dyn CueSource>)), src, Some(Duration::from_secs(100)), Some(cache));
        assert!(wait_until(&w2, |s| s.status.contains("complete")));
        assert!(again.lock().unwrap().is_empty(), "re-transcribed despite the cache");
        assert_eq!(w2.with(|s| s.transcript.cues().len()), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pause is obeyed between chunks and resuming continues.
    #[test]
    fn pausing_stops_new_chunks() {
        if !have_ffmpeg() { return; }
        let p = std::env::temp_dir().join(format!("vk_pause_{}.mkv", std::process::id()));
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=150"]).arg(&p).status().unwrap().success());
        let order = Arc::new(Mutex::new(Vec::new()));
        let o2 = order.clone();
        let w = Worker::start(Box::new(move || Ok(Box::new(Script(o2)) as Box<dyn CueSource>)), AudioIn { input: p.to_string_lossy().into(), headers: vec![] }, Some(Duration::from_secs(150)), None);
        w.set_mode(Mode::Paused);
        std::thread::sleep(Duration::from_millis(600));
        let n = order.lock().unwrap().len();
        assert!(n <= 1, "paused, yet {n} chunks ran");
        assert!(w.with(|s| s.status.contains("paused")), "{}", w.with(|s| s.status.clone()));
        w.set_mode(Mode::All);
        assert!(wait_until(&w, |s| s.coverage.complete()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fnv_is_the_published_function() {
        // Reference vectors for 64-bit FNV-1a.
        assert_eq!(fnv1a(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    /// A caption track served over real HTTP by a local socket: fetched,
    /// parsed, and handed to the loop whole, with no audio decoded.
    #[test]
    fn page_captions_become_the_transcript_without_decoding_audio() {
        use std::io::{Read as _, Write as _};
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nhello fourier\n\n00:01:05.000 --> 00:01:07.000\nnow laplace\n";
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = vtt.to_string();
        std::thread::spawn(move || {
            if let Ok((mut c, _)) = listener.accept() {
                let mut req = [0u8; 2048];
                let _ = c.read(&mut req);
                let head = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n", body.len(), body.len() - 1, body.len());
                let _ = c.write_all(head.as_bytes());
                let _ = c.write_all(body.as_bytes());
            }
        });
        let cap = vidkit_ytdlp::Caption { url: format!("http://127.0.0.1:{port}/cc.vtt"), lang: "en-orig".into(), automatic: true };
        // The audio input does not exist: if the loop tried to decode it, the
        // transcript would stay empty.
        let w = Worker::start(caption_factory(cap, vec![]), AudioIn { input: "/nonexistent.m4a".into(), headers: vec![] }, Some(Duration::from_secs(120)), None);
        assert!(wait_until(&w, |s| s.transcript.cues().len() == 2), "{}", w.with(|s| s.status.clone()));
        assert!(w.with(|s| s.status.starts_with("auto-captions:en-orig")), "{}", w.with(|s| s.status.clone()));
        assert!(w.with(|s| s.coverage.complete()), "all chunks marked done");
    }
}
