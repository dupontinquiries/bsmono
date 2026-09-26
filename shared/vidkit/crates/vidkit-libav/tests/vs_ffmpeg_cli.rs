//! The in-process decoder against the ffmpeg CLI the player used before:
//! the same file, the same seek, the same bytes.

use std::process::{Command, Stdio};
use std::time::Duration;

use vidkit_core::frame::FrameData;
use vidkit_libav::{Decoder, Hw, Options};

fn have(enc: &str) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains(enc)).unwrap_or(false)
}

fn make(name: &str, codec_args: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vk-libav-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join(name);
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=3", "-g", "30"])
        .args(codec_args)
        .args(["-pix_fmt", "yuv420p"])
        .arg(&f)
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "encode {name}");
    f
}

fn cli(f: &std::path::Path, from: Option<&str>) -> Vec<u8> {
    let mut c = Command::new("ffmpeg");
    c.args(["-v", "error"]);
    if let Some(s) = from {
        c.args(["-ss", s]);
    }
    c.arg("-i").arg(f).args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]);
    c.output().unwrap().stdout
}

fn ours(f: &std::path::Path, from: Duration) -> (Vec<u8>, Vec<Duration>) {
    let mut d = Decoder::open(f.to_str().unwrap(), &Options { hw: Hw::Software, from, threads: 0, export_grain: false }).unwrap();
    let (mut out, mut pts) = (Vec::new(), Vec::new());
    while let Some(fr) = d.next_frame(&mut |n| Vec::with_capacity(n)).unwrap() {
        let FrameData::Cpu(p) = fr.data else { panic!("software frames are CPU planes") };
        for pl in p.data {
            out.extend_from_slice(&pl);
        }
        pts.push(fr.pts);
    }
    (out, pts)
}

fn check(name: &str, codec_args: &[&str]) {
    let f = make(name, codec_args);
    let (a, pts) = ours(&f, Duration::ZERO);
    let b = cli(&f, None);
    assert_eq!(a.len(), b.len(), "{name}: frame count ({} vs {} bytes)", a.len(), b.len());
    assert!(a == b, "{name}: decoded bytes differ from the CLI");
    assert_eq!(pts.first(), Some(&Duration::ZERO));
    assert!((pts[30].as_secs_f64() - 1.0).abs() < 0.002, "{name}: frame 30 at {:?}", pts[30]);
    // Mid-GOP seek: the frames from 1.5 s on, exactly as `-ss 1.5`.
    let (a, pts) = ours(&f, Duration::from_millis(1500));
    let b = cli(&f, Some("1.5"));
    assert_eq!(a.len(), b.len(), "{name}: seek frame count");
    assert!(a == b, "{name}: seeked bytes differ from the CLI");
    assert!((pts[0].as_secs_f64() - 1.5).abs() < 0.002, "{name}: first frame after seek at {:?}", pts[0]);
    eprintln!("DIAG {name}: whole file and seek byte-identical to the CLI");
}

#[test]
fn h264_vp9_av1_match_the_cli() {
    if have("libx264") {
        check("a.mp4", &["-c:v", "libx264"]);
    }
    if have("libvpx-vp9") {
        check("b.webm", &["-c:v", "libvpx-vp9", "-b:v", "300k", "-deadline", "realtime", "-cpu-used", "8"]);
    }
    if have("libsvtav1") {
        check("c.mkv", &["-c:v", "libsvtav1", "-preset", "12"]);
    }
}

/// No VAAPI here: asking for it fails at open with a reason, not a crash.
#[test]
fn a_missing_vaapi_device_is_an_error() {
    if !have("libx264") {
        return;
    }
    let f = make("d.mp4", &["-c:v", "libx264"]);
    let e = Decoder::open(f.to_str().unwrap(), &Options { hw: Hw::Vaapi { device: "/dev/dri/renderD199".into() }, from: Duration::ZERO, threads: 0, export_grain: false }).err().expect("no such device");
    assert!(e.contains("VAAPI device"), "{e}");
}

/// Film grain exported in process + our CPU synthesis == libdav1d's own
/// synthesis, every byte of every frame. Also the check on the hand-declared
/// AVFilmGrainParams layout.
#[test]
fn exported_grain_plus_ours_is_dav1ds_picture() {
    if !have("libsvtav1") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("vk-libav-g-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("g.mkv");
    let (w, h) = (352usize, 198usize);
    assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=352x198:rate=30:duration=2,noise=alls=14:allf=t", "-c:v", "libsvtav1", "-preset", "10", "-g", "30", "-svtav1-params", "film-grain=10", "-pix_fmt", "yuv420p"]).arg(&f).stderr(Stdio::null()).status().unwrap().success());
    let run = |export: bool| {
        let mut d = Decoder::open(f.to_str().unwrap(), &Options { hw: Hw::Software, from: Duration::from_millis(700), threads: 0, export_grain: export }).unwrap();
        let mut frames = Vec::new();
        while let Some(fr) = d.next_frame(&mut |n| Vec::with_capacity(n)).unwrap() {
            frames.push(fr);
        }
        (frames, d.grain_frames, d.codec.clone())
    };
    let (applied, g0, codec) = run(false);
    let (exported, g1, _) = run(true);
    assert_eq!(g0, 0);
    assert_eq!(g1 as usize, exported.len(), "every frame carries grain");
    assert_eq!(applied.len(), exported.len());
    for (i, (a, e)) in applied.iter().zip(&exported).enumerate() {
        let (FrameData::Cpu(pa), FrameData::Cpu(pe)) = (&a.data, &e.data) else { panic!() };
        let (mut y, mut u, mut v) = (pe.data[0].clone(), pe.data[1].clone(), pe.data[2].clone());
        vidkit_core::av1_grain::apply_i420(e.grain.as_ref().unwrap(), &mut y, &mut u, &mut v, w, h, false);
        let bad = [(&y, &pa.data[0]), (&u, &pa.data[1]), (&v, &pa.data[2])].iter().map(|(x, z)| x.iter().zip(z.iter()).filter(|(p, q)| p != q).count()).sum::<usize>();
        assert_eq!(bad, 0, "frame {i}: {bad} bytes differ from {codec}'s own grain");
    }
    eprintln!("DIAG {} frames from 0.7 s through {codec}: exported grain + ours == its synthesis", exported.len());
}
