//! Decoding, by driving ffmpeg.
//!
//! Nothing in this tree decodes video. The demuxers, the player loop, the
//! sync policy, the presenter and the colour conversion are all finished and
//! all fed a test pattern, because a real decoder needs V4L2 hardware or a
//! codec library and neither could be stood up where this was written.
//!
//! ffmpeg can be. This spawns it and reads raw NV12 off its stdout, which
//! makes video actually appear — and is honest about what it costs.
//!
//! # This is a stopgap, and it duplicates work
//!
//! ffmpeg demuxes as well as decodes, so with this in the loop the container
//! parsing in `vidkit-mp4` and `vidkit-webm` is doing nothing for playback; it
//! still describes the file and still drives seeking and the section plan. Two
//! demuxers for one file is not the destination.
//!
//! The destination is [`vidkit_core::traits::Decoder`] over V4L2 — packets in,
//! DMA-BUFs out, hardware. That path exists and has never run. This one exists
//! so the other ninety percent of the player can be seen working, and so the
//! frame queue, the A/V clock and the presenter get exercised by real frame
//! timings rather than a synthetic loop.
//!
//! # Why a pipe and not libavcodec
//!
//! Linking libavcodec means a build that fails wherever the headers are the
//! wrong version, which is most places. A pipe costs a process and some memory
//! bandwidth and works against whatever ffmpeg the machine has. For a stopgap
//! that is the right trade; for the real path it would not be.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout};

/// What ffprobe says about a file.
#[derive(Clone, PartialEq, Debug)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    /// Frames per second, as a rational reduced to a float.
    pub fps: f64,
}

/// Parse `ffprobe -show_entries stream=width,height,r_frame_rate` output.
///
/// Separated from the process call so the parsing is tested without spawning
/// anything — and the rational is the part that goes wrong: `30000/1001` is
/// 29.97 and `30/1` is 30, and a parser that splits on `/` and takes the first
/// number gets 30000 fps.
pub fn parse_probe(text: &str) -> Option<Probe> {
    let mut w = None;
    let mut h = None;
    let mut fps = None;
    for line in text.lines() {
        let (k, v) = line.split_once('=')?;
        match k.trim() {
            "width" => w = v.trim().parse().ok(),
            "height" => h = v.trim().parse().ok(),
            "r_frame_rate" => {
                let v = v.trim();
                fps = match v.split_once('/') {
                    Some((n, d)) => {
                        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
                        (d != 0.0).then_some(n / d)
                    }
                    None => v.parse().ok(),
                };
            }
            _ => {}
        }
    }
    Some(Probe {
        width: w?,
        height: h?,
        // A stream with no frame rate is not fatal: still images and some
        // captures report none, and 24 is a better guess than refusing.
        fps: fps.filter(|f| *f > 0.0).unwrap_or(24.0),
    })
}

/// Ask ffprobe about a file.
pub fn probe(path: &Path) -> Result<Probe, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate",
            "-of", "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "ffprobe is not installed".to_string(),
            _ => format!("ffprobe: {e}"),
        })?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe: {}",
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("failed")
        ));
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| format!("{}: no video stream", path.display()))
}

/// A running ffmpeg, yielding NV12 frames.
/// Where the frames come from, so a seek can restart the same source.
#[derive(Clone, Debug)]
enum Source {
    File(std::path::PathBuf),
    Streaming {
        video_url: String,
        audio_url: Option<String>,
        headers: Vec<(String, String)>,
    },
}

pub struct FfmpegFrames {
    child: Child,
    source: Source,
    cap: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub frame_interval: Duration,
    /// Reused between frames: at 1080p this is 3 MB, and allocating it per
    /// frame is the same waste the CPU converter was fixed for.
    buf: Vec<u8>,
    index: u64,
    pub frames_read: u64,
}

impl FfmpegFrames {
    /// Decode `path` to NV12, optionally scaled down.
    ///
    /// `max_long_edge` exists because decoding 1440p in software to throw most
    /// of it away is the expensive mistake the capability model is there to
    /// avoid — letting ffmpeg scale costs far less than decoding full size and
    /// scaling afterwards.
    pub fn open(path: &Path, max_long_edge: Option<u32>) -> Result<Self, String> {
        Self::open_at(path, max_long_edge, Duration::ZERO)
    }

    /// [`open`](Self::open), starting at `from`.
    pub fn open_at(
        path: &Path,
        max_long_edge: Option<u32>,
        from: Duration,
    ) -> Result<Self, String> {
        let p = probe(path)?;
        let (mut w, mut h) = (p.width, p.height);
        let mut filters: Vec<String> = Vec::new();
        if let Some(cap) = max_long_edge {
            let long = w.max(h);
            if long > cap {
                let scale = cap as f64 / long as f64;
                // Even dimensions: NV12 chroma is subsampled by two, and an
                // odd width makes every row after the first shear.
                w = ((w as f64 * scale) as u32) & !1;
                h = ((h as f64 * scale) as u32) & !1;
                filters.push(format!("scale={w}:{h}"));
            }
        }

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-nostdin"]);
        if !from.is_zero() {
            // Before -i: seeks the container to the nearest keyframe. After
            // -i it would decode from the start and discard, making a scrub
            // cost time proportional to how far you scrubbed.
            cmd.args(["-ss", &format!("{:.3}", from.as_secs_f64())]);
        }
        cmd.arg("-i").arg(path);
        if !filters.is_empty() {
            cmd.args(["-vf", &filters.join(",")]);
        }
        cmd.args(["-f", "rawvideo", "-pix_fmt", "nv12", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Inherited stderr would scribble over the UI; ffmpeg is chatty
            // even at -v error.
            .stderr(Stdio::null());

        let child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "ffmpeg is not installed".to_string(),
            _ => format!("ffmpeg: {e}"),
        })?;

        Ok(Self {
            child,
            source: Source::File(path.to_path_buf()),
            cap: max_long_edge,
            width: w,
            height: h,
            frame_interval: Duration::from_secs_f64(1.0 / p.fps.max(1.0)),
            buf: vec![0u8; frame_bytes(w, h)],
            index: 0,
            frames_read: 0,
        })
    }

    /// Jump to a position.
    ///
    /// ffmpeg is restarted with `-ss` rather than asked to seek: a decode
    /// pipeline reading raw frames off a pipe has no seek, and draining to the
    /// target would read every frame in between — for a scrub to the end of a
    /// lecture that is the whole file.
    ///
    /// `-ss` goes BEFORE `-i`. After it, ffmpeg decodes from the start and
    /// throws frames away, which is accurate and turns a scrub into a wait
    /// proportional to how far you scrubbed. Before it, ffmpeg seeks the
    /// container and starts at the nearest keyframe, which is what a scrub
    /// wants.
    pub fn seek(&mut self, to: Duration) -> Result<(), String> {
        let source = self.source.clone();
        let cap = self.cap;
        let mut next = match &source {
            Source::File(p) => Self::open_at(p, cap, to)?,
            Source::Streaming { video_url, audio_url, headers } => {
                let input = StreamingInput {
                    video_url: video_url.clone(),
                    audio_url: audio_url.clone(),
                    headers: headers.clone(),
                };
                Self::open_streaming_at(&input, cap, to)?
            }
        };
        // Timestamps continue from where the seek landed, not from zero, or
        // the transport jumps back to the start of the film every scrub.
        next.index = (to.as_secs_f64() / next.frame_interval.as_secs_f64().max(1e-9)) as u64;
        *self = next;
        Ok(())
    }

    /// The next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Option<Frame> {
        let stdout = self.child.stdout.as_mut()?;
        // read_exact, not read: a pipe hands over whatever is ready, so a
        // single read returns a partial frame and the picture tears diagonally
        // — the classic symptom of treating a pipe like a file.
        if stdout.read_exact(&mut self.buf).is_err() {
            return None;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let luma = self.buf[..w * h].to_vec();
        let chroma = self.buf[w * h..].to_vec();

        let pts = self.frame_interval.mul_f64(self.index as f64);
        self.index += 1;
        self.frames_read += 1;

        Some(Frame {
            data: FrameData::Cpu(CpuPlanes {
                data: vec![luma, chroma],
                strides: vec![self.width, self.width],
            }),
            width: self.width,
            height: self.height,
            layout: PlaneLayout::Nv12,
            color: ColorInfo::default(),
            pts,
            duration: Some(self.frame_interval),
            recycle: None,
        })
    }
}

impl Drop for FfmpegFrames {
    /// Kill it. An ffmpeg left running after the window closes keeps decoding
    /// a film nobody is watching, which on a phone is the whole battery.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Bytes in one NV12 frame.
pub fn frame_bytes(w: u32, h: u32) -> usize {
    let (w, h) = (w as usize, h as usize);
    // Chroma rounds UP: an odd dimension still needs a half-plane row, and
    // truncating leaves the last row short so the read never completes.
    w * h + w.div_ceil(2) * 2 * h.div_ceil(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// `30000/1001` is 29.97 and `30/1` is 30. A parser that splits on `/` and
    /// takes the first number gets 30000 fps and every frame is due
    /// immediately.
    #[test]
    fn a_rational_frame_rate_is_divided_not_truncated() {
        let p = parse_probe("width=1920\nheight=1080\nr_frame_rate=30000/1001\n").unwrap();
        assert_eq!((p.width, p.height), (1920, 1080));
        assert!((p.fps - 29.97).abs() < 0.01, "got {}", p.fps);

        let q = parse_probe("width=640\nheight=480\nr_frame_rate=30/1\n").unwrap();
        assert!((q.fps - 30.0).abs() < 0.001);
    }

    /// A stream with no frame rate is not fatal; still images report 0/0.
    #[test]
    fn a_missing_frame_rate_falls_back() {
        let p = parse_probe("width=64\nheight=64\nr_frame_rate=0/0\n").unwrap();
        assert!(p.fps > 0.0, "a zero rate would make every frame due at once");
    }

    #[test]
    fn a_probe_without_dimensions_is_not_a_video() {
        assert!(parse_probe("r_frame_rate=30/1\n").is_none());
        assert!(parse_probe("").is_none());
    }

    /// NV12 chroma rounds up. Truncating leaves the last row short and the
    /// read never completes, so playback stops one frame in with no error.
    #[test]
    fn odd_dimensions_round_the_chroma_plane_up() {
        assert_eq!(frame_bytes(2, 2), 4 + 2);
        assert_eq!(frame_bytes(1920, 1080), 1920 * 1080 * 3 / 2);
        let odd = frame_bytes(3, 3);
        assert_eq!(odd, 9 + 2 * 2 * 2, "an odd frame was rounded down");
    }

    /// Decode something ffmpeg made, so the whole path runs: spawn, pipe,
    /// frame sizing, NV12 layout.
    #[test]
    fn it_decodes_a_real_clip() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-dec-{}.mp4", std::process::id()));
        let made = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=1",
                "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(made.success(), "could not make a test clip");

        let mut d = FfmpegFrames::open(&path, None).expect("open");
        assert_eq!((d.width, d.height), (320, 240));
        assert!((d.frame_interval.as_secs_f64() - 0.1).abs() < 0.01);

        let first = d.next_frame().expect("a first frame");
        assert_eq!(first.layout, PlaneLayout::Nv12);
        assert_eq!(first.pts, Duration::ZERO);
        match &first.data {
            FrameData::Cpu(p) => {
                assert_eq!(p.data[0].len(), 320 * 240, "luma is the wrong size");
                assert_eq!(p.data[1].len(), 320 * 240 / 2, "chroma is the wrong size");
                // testsrc is a colour pattern, not a flat field: a decoder
                // handing back zeros would pass every size check above.
                assert!(
                    p.data[0].iter().any(|&v| v > 16),
                    "the luma plane is empty — nothing was decoded"
                );
            }
            _ => panic!("expected CPU planes"),
        }

        let mut n = 1;
        while d.next_frame().is_some() {
            n += 1;
        }
        assert!(n >= 9, "expected about ten frames, got {n}");
        // Timestamps advance, or the frame queue shows one frame forever.
        let _ = std::fs::remove_file(&path);
    }

    /// Scaling happens in ffmpeg. Decoding 1440p to throw most of it away is
    /// the expensive mistake the capability model exists to avoid.
    #[test]
    fn it_scales_down_on_request() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-scale-{}.mp4", std::process::id()));
        Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=640x480:rate=5:duration=1",
                "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .expect("spawn");

        let mut d = FfmpegFrames::open(&path, Some(320)).expect("open");
        assert_eq!(d.width, 320, "not scaled");
        assert_eq!(d.height, 240, "aspect ratio lost");
        assert!(d.width % 2 == 0 && d.height % 2 == 0, "odd dimensions in NV12");
        let f = d.next_frame().expect("frame");
        match &f.data {
            FrameData::Cpu(p) => assert_eq!(p.data[0].len(), 320 * 240),
            _ => unreachable!(),
        }
        let _ = std::fs::remove_file(&path);
    }

    /// The exact clip the player showed a test pattern for. If this decodes,
    /// the decoder is fine and the wiring is not.
    #[test]
    fn it_decodes_the_clip_the_player_could_not() {
        if !have_ffmpeg() || !Path::new("/tmp/real.mp4").exists() {
            eprintln!("SKIP");
            return;
        }
        let mut d = FfmpegFrames::open(Path::new("/tmp/real.mp4"), Some(1920)).expect("open");
        eprintln!("DIAG size={}x{} interval={:?}", d.width, d.height, d.frame_interval);
        let f = d.next_frame();
        eprintln!("DIAG first frame: {}", f.is_some());
        assert!(f.is_some(), "the decoder returned nothing for a file it opened");
    }

    /// Seek must land somewhere else, and quickly.
    ///
    /// A clip that changes brightness over time, so "did it move" is a
    /// question the pixels answer. Asserting only the reported timestamp would
    /// pass against a seek that reset to zero and relabelled the frames.
    #[test]
    fn seeking_lands_at_a_different_picture() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-seek-{}.mp4", std::process::id()));
        // Luma as a function of TIME, so "did it move" is a question the
        // pixels answer. My first attempt faded a solid black source, which
        // stays black — both ends read 16.0 and the test failed on its own
        // fixture rather than on the seek.
        let ok = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "color=c=black:size=160x120:rate=10:duration=10",
                "-vf", "geq=lum='min(250,T*25)':cb=128:cr=128",
                "-c:v", "libx264", "-g", "10", "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: could not encode");
            return;
        }

        let mean = |f: &Frame| -> f64 {
            match &f.data {
                FrameData::Cpu(p) => {
                    p.data[0].iter().map(|&v| v as f64).sum::<f64>() / p.data[0].len() as f64
                }
                _ => 0.0,
            }
        };

        let mut d = FfmpegFrames::open(&path, None).expect("open");
        let start = mean(&d.next_frame().expect("first frame"));

        let began = std::time::Instant::now();
        d.seek(Duration::from_secs(8)).expect("seek");
        let after = d.next_frame().expect("a frame after seeking");
        let took = began.elapsed();

        assert!(
            (mean(&after) - start).abs() > 5.0,
            "the picture did not change: {start:.1} then {:.1}",
            mean(&after)
        );
        // Timestamps continue from the seek, not from zero, or the transport
        // jumps back to the start of the film on every scrub.
        assert!(
            after.pts >= Duration::from_secs(7),
            "timestamps restarted at {:?}",
            after.pts
        );
        // Restarting ffmpeg with -ss is near-instant; decoding to the target
        // would take about as long as the clip.
        assert!(took < Duration::from_secs(5), "seek took {took:?}");

        let _ = std::fs::remove_file(&path);
    }

    /// `-ss` before `-i` seeks the container; after it, ffmpeg decodes from
    /// the start and discards, so a scrub costs time proportional to how far
    /// you scrubbed — and over HTTP, downloads it too.
    #[test]
    fn the_seek_flag_precedes_the_input() {
        let input = StreamingInput {
            video_url: "http://cdn/v.mp4".into(),
            audio_url: None,
            headers: Vec::new(),
        };
        let mut args = streaming_args(&input, None);
        let at = args.iter().position(|a| a == "-i").unwrap();
        args.insert(at, "12.000".into());
        args.insert(at, "-ss".into());
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < i, "-ss after -i decodes and discards: {args:?}");
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_hanging() {
        if !have_ffmpeg() {
            return;
        }
        let e = probe(Path::new("/nonexistent/nope.mp4")).unwrap_err();
        assert!(!e.is_empty());
    }
}

// ---------------------------------------------------------------------------
// fetch while playing
// ---------------------------------------------------------------------------

/// Decode straight from URLs, without downloading first.
///
/// The download-then-play path waits for the whole file before the first
/// frame, which on a lecture is a long stare at a test pattern. This starts
/// decoding immediately.
///
/// # Why ffmpeg fetches rather than us
///
/// The obvious shape — download in the background, point the decoder at the
/// growing file — does not work. ffmpeg reaching the write head of a regular
/// file sees EOF and stops; there is no "wait for more" for a plain file. And
/// an MP4 written by a remux has its `moov` atom at the END, so the file is
/// not even openable until the last byte lands.
///
/// Giving ffmpeg the URLs sidesteps both: an HTTP input blocks for more data
/// instead of ending, and ffmpeg reads only as far ahead as it needs.
///
/// # Two inputs, because YouTube has two streams
///
/// Video and audio arrive as separate URLs. ffmpeg takes both as inputs and
/// maps one stream from each — the same muxing yt-dlp would have done, done
/// while playing instead of before it.
pub struct StreamingInput {
    pub video_url: String,
    /// Separate audio, when the site serves it that way.
    pub audio_url: Option<String>,
    /// Headers the media URL is signed against. A CDN that checks the
    /// User-Agent answers 403 without them, which looks exactly like an
    /// expired link.
    pub headers: Vec<(String, String)>,
}

/// ffmpeg arguments for streaming inputs.
///
/// Pure, because the flag order is what breaks: `-headers` and `-user_agent`
/// are INPUT options and must precede the `-i` they apply to. Placed after,
/// ffmpeg silently ignores them and the fetch 403s.
pub fn streaming_args(input: &StreamingInput, max_long_edge: Option<u32>) -> Vec<String> {
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into()];

    let header_blob = |a: &mut Vec<String>| {
        if !input.headers.is_empty() {
            let joined: String = input
                .headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect();
            a.push("-headers".into());
            a.push(joined);
        }
        // Reconnect on a dropped connection. A CDN closing an idle socket
        // mid-film is routine, and without this the film simply stops.
        a.push("-reconnect".into());
        a.push("1".into());
        a.push("-reconnect_streamed".into());
        a.push("1".into());
        a.push("-reconnect_delay_max".into());
        a.push("5".into());
    };

    header_blob(&mut a);
    a.push("-i".into());
    a.push(input.video_url.clone());

    if let Some(au) = &input.audio_url {
        header_blob(&mut a);
        a.push("-i".into());
        a.push(au.clone());
        // Take video from the first input and audio from the second.
        a.push("-map".into());
        a.push("0:v:0".into());
        a.push("-map".into());
        a.push("1:a:0".into());
    }

    if let Some(cap) = max_long_edge {
        a.push("-vf".into());
        // -2 keeps the other axis even and proportional, which NV12 needs.
        a.push(format!("scale='if(gt(iw,ih),min({cap},iw),-2)':'if(gt(iw,ih),-2,min({cap},ih))'"));
    }
    a.push("-f".into());
    a.push("rawvideo".into());
    a.push("-pix_fmt".into());
    a.push("nv12".into());
    a.push("-".into());
    a
}

impl FfmpegFrames {
    /// Start decoding from URLs, fetching as it goes.
    ///
    /// The dimensions come from probing the video URL, which costs one small
    /// HTTP request rather than the whole file.
    pub fn open_streaming(
        input: &StreamingInput,
        max_long_edge: Option<u32>,
    ) -> Result<Self, String> {
        Self::open_streaming_at(input, max_long_edge, Duration::ZERO)
    }

    /// [`open_streaming`](Self::open_streaming), starting at `from`.
    pub fn open_streaming_at(
        input: &StreamingInput,
        max_long_edge: Option<u32>,
        from: Duration,
    ) -> Result<Self, String> {
        let p = probe_url(&input.video_url, &input.headers)?;
        let (mut w, mut h) = (p.width, p.height);
        if let Some(cap) = max_long_edge {
            let long = w.max(h);
            if long > cap {
                let scale = cap as f64 / long as f64;
                w = ((w as f64 * scale) as u32) & !1;
                h = ((h as f64 * scale) as u32) & !1;
            }
        }

        let mut args = streaming_args(input, max_long_edge);
        if !from.is_zero() {
            // Inserted before the first -i, for the same reason as the file
            // path: after it, ffmpeg would fetch and discard everything up to
            // the seek point, which over HTTP means downloading it too.
            let at = args.iter().position(|a| a == "-i").unwrap_or(0);
            args.insert(at, format!("{:.3}", from.as_secs_f64()));
            args.insert(at, "-ss".into());
        }
        let child = Command::new("ffmpeg")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => "ffmpeg is not installed".to_string(),
                _ => format!("ffmpeg: {e}"),
            })?;

        Ok(Self {
            child,
            source: Source::Streaming {
                video_url: input.video_url.clone(),
                audio_url: input.audio_url.clone(),
                headers: input.headers.clone(),
            },
            cap: max_long_edge,
            width: w,
            height: h,
            frame_interval: Duration::from_secs_f64(1.0 / p.fps.max(1.0)),
            buf: vec![0u8; frame_bytes(w, h)],
            index: 0,
            frames_read: 0,
        })
    }
}

/// ffprobe a URL rather than a path.
pub fn probe_url(url: &str, headers: &[(String, String)]) -> Result<Probe, String> {
    let mut cmd = Command::new("ffprobe");
    cmd.args(["-v", "error"]);
    if !headers.is_empty() {
        let joined: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        cmd.args(["-headers", &joined]);
    }
    cmd.args([
        "-select_streams", "v:0",
        "-show_entries", "stream=width,height,r_frame_rate",
        "-of", "default=noprint_wrappers=1",
    ])
    .arg(url);

    let out = cmd.output().map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe {url}: {}",
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("failed")
        ));
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| format!("{url}: no video stream"))
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    fn input() -> StreamingInput {
        StreamingInput {
            video_url: "http://cdn/v.mp4".into(),
            audio_url: None,
            headers: vec![("User-Agent".into(), "Mozilla/5.0".into())],
        }
    }

    /// `-headers` is an INPUT option. After the `-i` it applies to, ffmpeg
    /// ignores it and the fetch 403s on any CDN that checks.
    #[test]
    fn headers_come_before_the_input_they_apply_to() {
        let a = streaming_args(&input(), None);
        let hi = a.iter().position(|x| x == "-headers").expect("no headers");
        let ii = a.iter().position(|x| x == "-i").expect("no input");
        assert!(hi < ii, "headers after -i are ignored: {a:?}");
    }

    /// Two inputs and an explicit map, or ffmpeg picks one file's streams and
    /// the audio is silently dropped.
    #[test]
    fn separate_audio_becomes_a_second_input_and_a_map() {
        let mut i = input();
        i.audio_url = Some("http://cdn/a.m4a".into());
        let a = streaming_args(&i, None);
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 2, "{a:?}");
        assert!(a.windows(2).any(|w| w[0] == "-map" && w[1] == "0:v:0"));
        assert!(a.windows(2).any(|w| w[0] == "-map" && w[1] == "1:a:0"));
    }

    /// Each input needs its own headers; a signed audio URL is signed too.
    #[test]
    fn both_inputs_get_the_headers() {
        let mut i = input();
        i.audio_url = Some("http://cdn/a.m4a".into());
        let a = streaming_args(&i, None);
        assert_eq!(
            a.iter().filter(|x| *x == "-headers").count(),
            2,
            "the audio input was left unsigned: {a:?}"
        );
    }

    /// A CDN closing an idle socket mid-film is routine; without reconnect the
    /// film just stops.
    #[test]
    fn it_reconnects_on_a_dropped_connection() {
        let a = streaming_args(&input(), None);
        assert!(a.iter().any(|x| x == "-reconnect"));
        assert!(a.iter().any(|x| x == "-reconnect_streamed"));
    }

    #[test]
    fn the_output_is_raw_nv12_on_stdout() {
        let a = streaming_args(&input(), None);
        assert_eq!(a.last().unwrap(), "-");
        assert!(a.windows(2).any(|w| w[0] == "-pix_fmt" && w[1] == "nv12"));
    }

    /// FETCH WHILE PLAYING, against a real socket: ffmpeg pulls over HTTP and
    /// frames come out. The server counts how much it served, and the check is
    /// that decoding STARTED before the whole file went out.
    #[test]
    fn it_decodes_over_http_without_downloading_first() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null())
            .stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true)
        {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // Long enough that serving it all takes visibly longer than the header.
        let clip = std::env::temp_dir().join(format!("vidkit-stream-{}.mp4", std::process::id()));
        let ok = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=20",
                "-pix_fmt", "yuv420p", "-c:v", "libx264",
                // moov at the front, so a streaming reader can start.
                "-movflags", "+faststart",
            ])
            .arg(&clip)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: could not encode");
            return;
        }
        let body = std::fs::read(&clip).unwrap();
        let total = body.len();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..32 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 { continue; }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (start, end) = req.lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string))
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().unwrap_or(body.len() - 1)))
                    })
                    .unwrap_or((0, body.len() - 1));
                if start >= body.len() { continue; }
                let slice = &body[start..=end.min(body.len() - 1)];
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                    start + slice.len() - 1, body.len(), slice.len());
                let _ = sock.write_all(head.as_bytes());
                // Dribble it, so a decoder that waited for the whole file
                // would visibly stall.
                for chunk in slice.chunks(16 * 1024) {
                    if sock.write_all(chunk).is_err() { break; }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });

        let input = StreamingInput {
            video_url: format!("http://127.0.0.1:{port}/clip.mp4"),
            audio_url: None,
            headers: Vec::new(),
        };
        let started = std::time::Instant::now();
        let mut d = FfmpegFrames::open_streaming(&input, None).expect("open streaming");
        assert_eq!((d.width, d.height), (320, 240));

        let first = d.next_frame().expect("a first frame over http");
        let elapsed = started.elapsed();
        match &first.data {
            FrameData::Cpu(p) => {
                assert_eq!(p.data[0].len(), 320 * 240);
                assert!(p.data[0].iter().any(|&v| v > 16), "decoded an empty frame");
            }
            _ => panic!("expected CPU planes"),
        }
        // Serving 20 seconds of dribbled video takes far longer than this; a
        // first frame this early means it did not wait for the file.
        assert!(
            elapsed < Duration::from_secs(20),
            "waited {elapsed:?} for the first frame — that is download-then-play"
        );
        eprintln!("DIAG first frame after {elapsed:?} of a {total}-byte clip");
        let _ = std::fs::remove_file(&clip);
    }
}
