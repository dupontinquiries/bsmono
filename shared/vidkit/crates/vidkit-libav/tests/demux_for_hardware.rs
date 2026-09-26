//! The bitstream a V4L2 decoder would get, decoded by the ffmpeg CLI, is
//! the same picture as the container decoded directly.

use std::process::{Command, Stdio};
use std::time::Duration;
use vidkit_libav::demux::Demuxer;

fn have(enc: &str) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains(enc)).unwrap_or(false)
}

fn decode(path: &std::path::Path, fmt: Option<&str>) -> Vec<u8> {
    let mut c = Command::new("ffmpeg");
    c.args(["-v", "error"]);
    if let Some(f) = fmt {
        c.args(["-f", f]);
    }
    c.arg("-i").arg(path).args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]).output().unwrap().stdout
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("vk-demux-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn encode(name: &str, args: &[&str]) -> std::path::PathBuf {
    let f = dir().join(name);
    assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2"]).args(args).args(["-pix_fmt", "yuv420p"]).arg(&f).stderr(Stdio::null()).status().unwrap().success());
    f
}

fn annexb_matches(name: &str, args: &[&str], raw_fmt: &str) {
    let f = encode(name, args);
    let mut d = Demuxer::open(f.to_str().unwrap(), &[], Duration::ZERO).unwrap();
    let mut es = Vec::new();
    let mut n = 0;
    while let Some(u) = d.next().unwrap() {
        if n == 0 {
            assert!(u.key && u.data.starts_with(&[0, 0, 0, 1]), "{name}: first unit is a key frame in Annex B");
        }
        es.extend(u.data);
        n += 1;
    }
    let raw = dir().join(format!("{name}.es"));
    std::fs::write(&raw, &es).unwrap();
    let (a, b) = (decode(&f, None), decode(&raw, Some(raw_fmt)));
    assert!(!a.is_empty() && a == b, "{name}: Annex B stream decodes differently ({} vs {} bytes)", a.len(), b.len());
    eprintln!("DIAG {name}: {n} units, Annex B decodes byte-identical");
}

#[test]
fn h264_and_hevc_become_annexb() {
    if have("libx264") {
        annexb_matches("a.mp4", &["-c:v", "libx264", "-bf", "2"], "h264");
    }
    if have("libx265") {
        annexb_matches("b.mp4", &["-c:v", "libx265", "-x265-params", "log-level=none"], "hevc");
    }
}

/// YouTube's VP9 packs hidden alt-refs into superframes (the 4K60 field
/// file: 42.7% of its bits are hidden frames). ffmpeg's libvpx wrapper
/// does not produce them here, so this uses the field file when it is
/// present (VIDKIT_VP9_SUPERFRAME_FILE or the uploaded sample), cut to half
/// a second: split, one frame per unit, written as IVF, the CLI decodes the
/// same pictures.
#[test]
fn vp9_superframes_split_into_frames() {
    let src = std::env::var("VIDKIT_VP9_SUPERFRAME_FILE").unwrap_or_else(|_| "/mnt/user-data/uploads/tmp_30aV79sXOW.ivf".into());
    if !std::path::Path::new(&src).exists() {
        eprintln!("no VP9 superframe sample; skipped");
        return;
    }
    let f = dir().join("c.ivf");
    assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-i", &src, "-t", "0.5", "-c", "copy"]).arg(&f).status().unwrap().success());
    let mut d = Demuxer::open(f.to_str().unwrap(), &[], Duration::ZERO).unwrap();
    let (w, h) = (d.width as u16, d.height as u16);
    let mut units = Vec::new();
    while let Some(u) = d.next().unwrap() {
        units.push(u.data);
    }
    let pkts = Command::new("ffprobe").args(["-v", "error", "-select_streams", "v", "-count_packets", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0"]).arg(&f).output().unwrap();
    let pkts: usize = String::from_utf8_lossy(&pkts.stdout).trim().parse().unwrap();
    assert!(units.len() > pkts, "superframes present and split: {} units from {pkts} packets", units.len());
    let mut ivf = Vec::new();
    ivf.extend(b"DKIF");
    ivf.extend(0u16.to_le_bytes());
    ivf.extend(32u16.to_le_bytes());
    ivf.extend(b"VP90");
    ivf.extend(w.to_le_bytes());
    ivf.extend(h.to_le_bytes());
    ivf.extend(60u32.to_le_bytes());
    ivf.extend(1u32.to_le_bytes());
    ivf.extend((units.len() as u32).to_le_bytes());
    ivf.extend(0u32.to_le_bytes());
    for (i, u) in units.iter().enumerate() {
        ivf.extend((u.len() as u32).to_le_bytes());
        ivf.extend((i as u64).to_le_bytes());
        ivf.extend(u);
    }
    let raw = dir().join("split.ivf");
    std::fs::write(&raw, &ivf).unwrap();
    let (a, b) = (decode(&f, None), decode(&raw, None));
    assert!(!a.is_empty() && a == b, "split VP9 decodes differently ({} vs {})", a.len(), b.len());
    eprintln!("DIAG vp9: {} units from {pkts} packets, split stream decodes byte-identical", units.len());
}
