//! af_remote — run the REAL autofocus search against a phone's REAL lens, from a laptop.
//!
//! Cross-building for the phone (aarch64-musl) is the heavy route. This is the cheap one: the
//! laptop runs `capture_kit::focus::{Search, sharpness}` — the code the apps will run — and
//! drives the phone over ssh: one command moves the lens (`v4l2-ctl focus_absolute`), one
//! fetches a few frames through the same `libcamerasrc` source snapshot-cam uses and returns
//! the last as raw RGBA. So what it exercises is the algorithm on real frames and a real
//! actuator; what it does NOT exercise is the on-device loop's timing (each probe here pays an
//! ssh round trip and a gst start, ~1-2 s).
//!
//!   AF_SSH='ssh -i ~/.ssh/id_phone -o IdentitiesOnly=yes -o BatchMode=yes user@192.168.2.231' \
//!   AF_CAMERA='/base/soc@0/cci@ac4a000/i2c-bus@1/camera@29' \
//!   AF_SUBDEV=/dev/v4l-subdev30 cargo run --release --example af_remote -- [outdir]
//!
//! Writes `<outdir>/frame-<position>.rgba` (640x480) for every probe and the verdict.
use capture_kit::focus::{parse_focus_range, roi_at, sharpness, Search, Step};
use capture_kit::RawFrame;
use std::process::Command;
use std::sync::Arc;

const W: usize = 640;
const H: usize = 480;

fn ssh(ssh_cmd: &str, remote: &str) -> Result<Vec<u8>, String> {
    let mut parts = ssh_cmd.split_whitespace();
    let prog = parts.next().ok_or("AF_SSH is empty")?;
    let out = Command::new(prog).args(parts).arg(remote).output().map_err(|e| format!("{prog}: {e}"))?;
    if !out.status.success() {
        return Err(format!("remote `{remote}` exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
    }
    Ok(out.stdout)
}

fn main() -> Result<(), String> {
    let ssh_cmd = std::env::var("AF_SSH").map_err(|_| "set AF_SSH (the ssh command prefix, ending in user@host)")?;
    let cam = std::env::var("AF_CAMERA").map_err(|_| "set AF_CAMERA (a libcamera id from `cam -l`)")?;
    let dev = std::env::var("AF_SUBDEV").map_err(|_| "set AF_SUBDEV (the lens motor subdev)")?;
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let range_txt = String::from_utf8_lossy(&ssh(&ssh_cmd, &format!("v4l2-ctl -d {dev} -l"))?).into_owned();
    let (min, max, step) = parse_focus_range(&range_txt).ok_or("the subdev has no focus_absolute")?;
    println!("lens range {min}..{max} step {step}");

    // The last of N frames: the first ones are captured while auto-exposure is still settling.
    let grab = format!(
        "gst-launch-1.0 -q libcamerasrc camera-name=\"{cam}\" ! video/x-raw,width={W},height={H} ! identity eos-after=8 ! \
         videoconvert ! video/x-raw,format=RGBA,width={W},height={H} ! fdsink fd=1 2>/dev/null"
    );
    let frame_bytes = W * H * 4;
    let roi = roi_at(0.5, 0.5, 0.5); // the centre half, as a tap at the middle would ask

    let mut search = Search::new(min, max, step);
    let mut log: Vec<(i64, f64)> = vec![];
    let t0 = std::time::Instant::now();
    let outcome = loop {
        match search.step() {
            Step::Probe(p) => {
                ssh(&ssh_cmd, &format!("v4l2-ctl -d {dev} --set-ctrl focus_absolute={p}"))?;
                std::thread::sleep(std::time::Duration::from_millis(300)); // the lens settle
                let raw = ssh(&ssh_cmd, &grab)?;
                if raw.len() < frame_bytes { return Err(format!("short frame read: {} < {frame_bytes}", raw.len())); }
                let last = &raw[raw.len() - frame_bytes..];
                std::fs::write(format!("{out_dir}/frame-{p:04}.rgba"), last).map_err(|e| e.to_string())?;
                let f = RawFrame { width: W as u32, height: H as u32, captured_ms: 0, seq: 0, rgba: Arc::from(last.to_vec()) };
                let s = sharpness(&f, roi).ok_or("sharpness refused the frame")?;
                println!("  probe focus={p:4}  sharpness={s:.5}");
                log.push((p, s));
                search.feed(s);
            }
            Step::Done(o) => break o,
        }
    };
    println!("\n{} probes in {:.1}s -> {:?}", outcome.probes, t0.elapsed().as_secs_f64(), outcome);
    if let Some(p) = outcome.position {
        ssh(&ssh_cmd, &format!("v4l2-ctl -d {dev} --set-ctrl focus_absolute={p}"))?;
        println!("lens left at {p}");
    } else {
        ssh(&ssh_cmd, &format!("v4l2-ctl -d {dev} --set-ctrl focus_absolute={min}"))?;
        println!("no confident peak (contrast {:.2} < threshold): lens returned to {min}", outcome.contrast);
    }
    let (lo, hi) = log.iter().fold((f64::MAX, f64::MIN), |a, &(_, s)| (a.0.min(s), a.1.max(s)));
    println!("sharpness range over the sweep: {lo:.5} .. {hi:.5}  (x{:.2})", if lo > 0.0 { hi / lo } else { 0.0 });
    Ok(())
}
