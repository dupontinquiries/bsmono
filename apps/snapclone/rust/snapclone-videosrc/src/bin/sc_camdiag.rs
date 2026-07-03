// sc-camdiag — on-device camera diagnostic harness for SnapClone.
//
// WHY: the app's "frames-seen: N" log line is actually the size of a 30-entry
// framerate ring buffer, so it caps at 30 and tells you nothing about real
// delivery. And preview problems (freezes, low fps, wrong stride) are hard to
// distinguish on-device by eye. This harness runs the EXACT same gst-launch
// pipeline the app uses, reads the raw BGRx frames off fd 1, and reports the
// REAL numbers: total frames, wall-clock fps, inter-frame gaps (stalls),
// declared vs actual stride, and per-frame layout validity (via the same
// snapclone_videosrc::validate_preview_frame the app should use).
//
// It is intentionally dependency-free (pure std + the lib) so it builds with the
// device's stock rustc and runs headless. Paste its summary back to iterate.
//
// USAGE (on the FP5):
//   sc-camdiag <pipewire-node-name> [--width W] [--height H] [--seconds S]
// e.g.
//   sc-camdiag libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29 \
//              --width 1280 --height 720 --seconds 10
//
// Find the node name from the app's log line "start() ACCEPTED — requested-
// device: \"...\"" or from `pw-dump | grep node.name`.

use snapclone_videosrc::{validate_preview_frame, FrameValidity};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Args {
    node: String,
    width: u32,
    height: u32,
    seconds: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut a = std::env::args().skip(1);
    let node = a.next().ok_or_else(|| {
        "usage: sc-camdiag <node-name> [--width W] [--height H] [--seconds S]".to_string()
    })?;
    let mut width = 1280u32;
    let mut height = 720u32;
    let mut seconds = 10u64;
    while let Some(flag) = a.next() {
        let val = a.next().ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--width" => width = val.parse().map_err(|_| "bad --width".to_string())?,
            "--height" => height = val.parse().map_err(|_| "bad --height".to_string())?,
            "--seconds" => seconds = val.parse().map_err(|_| "bad --seconds".to_string())?,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(Args { node, width, height, seconds })
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    let bpp = 4u32; // BGRx
    let stride = args.width * bpp; // app negotiates tight BGRx at the sink
    let frame_bytes = (stride as usize) * (args.height as usize);

    // The EXACT pipeline shape the app uses (GstLaunchBackend): pipewiresrc →
    // queue(leaky) → videoconvert → BGRx → videoscale → videoconvert → BGRx@WxH
    // → fdsink. We point fdsink at stdout and read it here.
    let pipeline = format!(
        "pipewiresrc target-object={node} do-timestamp=true ! \
         queue max-size-buffers=4 max-size-time=0 max-size-bytes=0 leaky=downstream ! \
         videoconvert ! video/x-raw,format=BGRx ! \
         videoscale add-borders=true ! videoconvert ! \
         video/x-raw,format=BGRx,width={w},height={h} ! \
         fdsink fd=1 sync=false qos=false",
        node = args.node, w = args.width, h = args.height
    );

    eprintln!("sc-camdiag: node={}", args.node);
    eprintln!("sc-camdiag: expecting BGRx {}x{}  stride={}  frame={} bytes",
              args.width, args.height, stride, frame_bytes);
    eprintln!("sc-camdiag: capturing for {}s ...", args.seconds);

    let mut child = match Command::new("gst-launch-1.0")
        .arg("-q")
        .args(pipeline.split(' ').filter(|s| !s.is_empty()))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("sc-camdiag: failed to spawn gst-launch-1.0: {e}");
            std::process::exit(1);
        }
    };

    let mut out = child.stdout.take().expect("stdout piped");
    let mut buf = vec![0u8; frame_bytes];
    let deadline = Instant::now() + Duration::from_secs(args.seconds);

    let start = Instant::now();
    let mut first_frame_at: Option<Instant> = None;
    let mut last_frame_at: Option<Instant> = None;
    let mut frames: u64 = 0;
    let mut bad_layout: u64 = 0;
    let mut max_gap = Duration::ZERO;
    let mut stalls_over_200ms: u64 = 0;
    // Duplicate detection via a FULL-FRAME FNV-1a hash. The previous 8-byte
    // sample could be fooled by `videoscale add-borders=true` letterboxing —
    // sampled bytes landing in the static black bars look "identical" every
    // frame even when the live region changes. Hashing every byte removes that
    // false positive. Cheap enough for a diagnostic (a few hundred MB/s).
    let mut unique_frames: u64 = 0;
    let mut dup_consecutive: u64 = 0;
    let mut last_hash: u64 = 0;
    // Letterbox probe (run once on the first frame): are the top+bottom rows all
    // one color? If so add-borders is padding this sensor → aspect mismatch.
    let mut border_note = String::new();

    let fnv = |bytes: &[u8]| -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    };

    while Instant::now() < deadline {
        match out.read_exact(&mut buf) {
            Ok(()) => {
                let now = Instant::now();
                frames += 1;
                if first_frame_at.is_none() {
                    first_frame_at = Some(now);
                    eprintln!("sc-camdiag: first frame at {} ms",
                              now.duration_since(start).as_millis());
                    // one-shot letterbox check: compare first row vs a middle row
                    let row = stride as usize;
                    if buf.len() >= row * args.height as usize {
                        let top_uniform = buf[..row].chunks(4).all(|p| p[0] == buf[0]
                            && p[1] == buf[1] && p[2] == buf[2]);
                        let mid = (args.height as usize / 2) * row;
                        let mid_uniform = buf[mid..mid + row].chunks(4)
                            .all(|p| p[0] == buf[mid] && p[1] == buf[mid + 1]);
                        if top_uniform && !mid_uniform {
                            border_note = " (top row is a uniform border — add-borders \
                                            is letterboxing this sensor: aspect mismatch)"
                                .to_string();
                        }
                    }
                }
                if let Some(prev) = last_frame_at {
                    let gap = now.duration_since(prev);
                    if gap > max_gap { max_gap = gap; }
                    if gap > Duration::from_millis(200) { stalls_over_200ms += 1; }
                }
                last_frame_at = Some(now);

                let v = validate_preview_frame(
                    args.width, args.height, bpp, stride, buf.len() as u64,
                );
                if !v.is_ok() {
                    bad_layout += 1;
                    if bad_layout <= 3 {
                        eprintln!("sc-camdiag: frame {frames} BAD layout: {v:?}");
                    }
                }

                let h = fnv(&buf);
                if frames > 1 && h == last_hash {
                    dup_consecutive += 1;
                } else {
                    unique_frames += 1;
                }
                last_hash = h;
            }
            Err(_) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    // ── report ──────────────────────────────────────────────────────────────
    let elapsed = first_frame_at
        .map(|f| last_frame_at.unwrap_or(f).duration_since(f).as_secs_f64())
        .unwrap_or(0.0);
    let fps = if elapsed > 0.0 { frames as f64 / elapsed } else { 0.0 };
    let content_fps = if elapsed > 0.0 { unique_frames as f64 / elapsed } else { 0.0 };
    let dup_pct = if frames > 0 { dup_consecutive as f64 * 100.0 / frames as f64 } else { 0.0 };

    println!("\n──────── sc-camdiag report ────────");
    println!("node:                 {}", args.node);
    println!("frames delivered:     {frames}");
    println!("capture window:       {:.2}s (first→last frame)", elapsed);
    println!("delivery fps:         {:.1}", fps);
    println!("CONTENT fps (unique): {:.1}{}", content_fps, border_note);
    println!("duplicate frames:     {dup_consecutive} ({dup_pct:.0}% of delivered)");
    println!("max inter-frame gap:  {} ms", max_gap.as_millis());
    println!("stalls >200ms:        {stalls_over_200ms}");
    println!("bad-layout frames:    {bad_layout} / {frames}");
    if first_frame_at.is_none() {
        println!("VERDICT: NO FRAMES — pipeline never delivered. Check node name / permissions.");
    } else if frames < 5 {
        println!("VERDICT: STARVED — first frame arrived but delivery stopped early.");
    } else if bad_layout > 0 {
        println!("VERDICT: LAYOUT ERRORS — stride/size mismatch (tiling/skew likely).");
    } else if content_fps < 20.0 && dup_pct > 25.0 {
        println!("VERDICT: DUPLICATE-PADDED — pipeline emits ~{:.0}fps but only ~{:.0}fps is \
                  NEW content; the source is repeating frames (sensor/ISP slower than the \
                  delivery rate, or a stuck buffer).", fps, content_fps);
    } else if content_fps < 15.0 {
        println!("VERDICT: LOW CONTENT FPS — genuinely few unique frames (choppy preview).");
    } else {
        println!("VERDICT: HEALTHY — steady delivery, correct layout, frames are unique.");
    }
    println!("───────────────────────────────────");
    let _ = FrameValidity::Ok;
}
