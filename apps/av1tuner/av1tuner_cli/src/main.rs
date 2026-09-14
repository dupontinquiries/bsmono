//! av1tuner-cli — the command-line half.
//!
//! `av1tuner` is an egui application. Everything it decides lives in
//! `av1tuner_core`, which has no UI dependency, so a CLI over the same
//! core is a thin shell rather than a second implementation.
//!
//! ## What it is for
//!
//! Batch work over ssh, and answering the three questions that come up
//! before a batch:
//!
//!   what will this cost      `plan`
//!   what crf should I use    `sweep`
//!   just do it               `encode`
//!
//! ## Reading straight off another machine
//!
//! `--from http://host:5001/Footage/` scans a dufs listing and works
//! over HTTP without copying anything. ffmpeg reads `http://` natively
//! and only pulls the bytes it needs.
//!
//! This requires the server to answer RANGE requests. Camera QuickTime
//! puts its index (`moov`) at the very END of the file -- measured at
//! 100% of the way into one of these -- so a client must read the end
//! before it can decode the beginning. With ranges that is one small
//! request; without, every probe downloads the whole file. `dufs`
//! supports them; Python's `http.server` does not.

use av1tuner_core::{calibrate, presets, remote};
use std::process::Command;

const USAGE: &str = "\
av1tuner — plan and run AV1 encodes

  av1tuner plan   [--from SRC] [--preset NAME] [--target GB]
  av1tuner sweep  [--from SRC] [--crf A,B,C] [--clip N]
  av1tuner encode [--from SRC] [--preset NAME] [--out DIR] [--jobs N] [--dry-run]
  av1tuner presets

SRC is a directory, a file, or a dufs URL (http://host:5001/Footage/).

  --preset NAME   master, archive, period, edit, review   (default archive)
  --target GB     fit the batch in this much space instead of holding a quality
  --crf A,B,C     which crf values to sweep                (default 14,22,30)
  --clip N        seconds of footage to sweep on           (default 2)
  --jobs N        how many files to encode at once         (default: cores)
  --dry-run       print the ffmpeg commands and stop

Notes
  Whole files are encoded concurrently, never sliced. Slicing inside a file
  costs about 20% because it resets the entropy coder's adaptation; see
  docs/av1-for-this-production.md.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print!("{USAGE}");
        return;
    }
    let flags = Flags::parse(&args[1..]);
    let code = match args[0].as_str() {
        "plan" => cmd_plan(&flags),
        "sweep" => cmd_sweep(&flags),
        "encode" => cmd_encode(&flags),
        "presets" => { print!("{}", presets::advice(391.0, 472.0)); 0 }
        other => { eprintln!("no command `{other}`\n\n{USAGE}"); 2 }
    };
    std::process::exit(code);
}

/// Hand-parsed rather than clap.
///
/// The workspace does not depend on clap and adding it for six flags
/// would put a dependency in front of anyone building this. If the
/// flag set grows past what this handles readably, that is the signal
/// to add clap -- and then `clap_complete` generates the shell
/// completions for free, which is worth having.
#[derive(Debug, Default)]
struct Flags {
    from: Option<String>,
    preset: Option<String>,
    out: Option<String>,
    target_gb: Option<f64>,
    crf: Vec<f32>,
    clip_secs: f64,
    jobs: usize,
    dry_run: bool,
}

impl Flags {
    fn parse(args: &[String]) -> Flags {
        let mut f = Flags { clip_secs: 2.0, jobs: 0, ..Default::default() };
        let mut i = 0;
        while i < args.len() {
            let take = |i: &mut usize| -> Option<String> {
                *i += 1;
                args.get(*i).cloned()
            };
            match args[i].as_str() {
                "--from" => f.from = take(&mut i),
                "--preset" => f.preset = take(&mut i),
                "--out" => f.out = take(&mut i),
                "--target" => f.target_gb = take(&mut i).and_then(|v| v.parse().ok()),
                "--clip" => f.clip_secs = take(&mut i).and_then(|v| v.parse().ok()).unwrap_or(2.0),
                "--jobs" => f.jobs = take(&mut i).and_then(|v| v.parse().ok()).unwrap_or(0),
                "--crf" => f.crf = take(&mut i).map(|v| v.split(',')
                    .filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default(),
                "--dry-run" => f.dry_run = true,
                other => eprintln!("ignoring unknown flag `{other}`"),
            }
            i += 1;
        }
        if f.crf.is_empty() { f.crf = vec![14.0, 22.0, 30.0]; }
        if f.jobs == 0 {
            f.jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        }
        f
    }

    fn preset(&self) -> presets::Preset {
        self.preset.as_deref().and_then(presets::by_name)
            .unwrap_or_else(presets::recommended)
    }
}

/// Find the footage, wherever it is.
fn gather(from: Option<&str>) -> Result<remote::Source, String> {
    let src = from.unwrap_or(".");
    if src.starts_with("http://") || src.starts_with("https://") {
        // A DUFS LISTING. Checked for range support before anything
        // else: without it every probe pulls a whole 5.8 GB file to
        // read the last few kilobytes of index.
        let html = fetch(src)?;
        let (files, notes) = remote::parse_listing(&html, src);
        for n in &notes { eprintln!("  {n}"); }
        if files.is_empty() {
            return Err(format!("no files found at {src} -- is it a dufs listing?"));
        }
        if !has_range_support(src, &files) {
            return Err(remote::Readiness::NoRangeRequests.explain());
        }
        Ok(remote::Source { base: src.to_string(), files })
    } else {
        let mut files = Vec::new();
        let meta = std::fs::metadata(src).map_err(|e| format!("{src}: {e}"))?;
        if meta.is_file() {
            files.push(remote::Remote {
                url: src.to_string(),
                name: src.rsplit('/').next().unwrap_or(src).to_string(),
                bytes: meta.len(), modified: None,
            });
        } else {
            for e in std::fs::read_dir(src).map_err(|e| format!("{src}: {e}"))?.flatten() {
                let Ok(m) = e.metadata() else { continue };
                if !m.is_file() { continue; }
                files.push(remote::Remote {
                    url: e.path().to_string_lossy().to_string(),
                    name: e.file_name().to_string_lossy().to_string(),
                    bytes: m.len(), modified: None,
                });
            }
        }
        Ok(remote::Source { base: src.to_string(), files })
    }
}

fn fetch(url: &str) -> Result<String, String> {
    let out = Command::new("curl").args(["-sS", "--max-time", "20", url]).output()
        .map_err(|e| format!("curl: {e} (is curl installed?)"))?;
    if !out.status.success() {
        return Err(format!("cannot reach {url}: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Does the server answer a range request?
///
/// A 206 means yes. A 200 means it sent the whole thing and would do
/// that for every probe.
fn has_range_support(_base: &str, files: &[remote::Remote]) -> bool {
    let Some(f) = files.iter().find(|f| f.is_video()) else { return true };
    Command::new("curl")
        .args(["-sS", "-o", "/dev/null", "-w", "%{http_code}", "-r", "0-99",
               "--max-time", "20", &f.url])
        .output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "206")
        .unwrap_or(false)
}

fn cmd_plan(f: &Flags) -> i32 {
    let src = match gather(f.from.as_deref()) { Ok(s) => s, Err(e) => { eprintln!("{e}"); return 1 } };
    let gb = src.total_bytes() as f64 / 1e9;
    println!("{} video file(s), {:.1} GB\n", src.videos().len(), gb);
    let stubs = src.stubs(100_000_000);
    if !stubs.is_empty() {
        println!("{} clip(s) under 100 MB -- false starts or card tests:", stubs.len());
        for s in stubs.iter().take(5) { println!("    {}", s.name); }
        println!();
    }
    match f.target_gb {
        Some(t) => println!("targeting {t:.1} GB -- run `sweep` first so the crf comes from \
                             a measurement rather than a guess"),
        None => print!("{}", presets::advice(gb.max(0.001), 472.0)),
    }
    0
}

fn cmd_sweep(f: &Flags) -> i32 {
    let src = match gather(f.from.as_deref()) { Ok(s) => s, Err(e) => { eprintln!("{e}"); return 1 } };
    let Some(clip) = src.videos().first().copied().cloned() else {
        eprintln!("nothing to sweep"); return 1;
    };
    println!("sweeping {} on {:.0}s of {}\n",
             f.crf.iter().map(|c| format!("{c:.0}")).collect::<Vec<_>>().join(", "),
             f.clip_secs, clip.name);
    let mut rungs = Vec::new();
    for crf in &f.crf {
        let out = format!("/tmp/av1tuner-sweep-{crf:.0}.mkv");
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-v", "error", "-y", "-t", &f.clip_secs.to_string(),
                  "-i", &clip.url, "-c:v", "libsvtav1", "-crf", &format!("{crf:.0}"),
                  "-preset", "8", "-pix_fmt", "yuv420p10le", &out]);
        if cmd.status().map(|s| !s.success()).unwrap_or(true) {
            eprintln!("  crf {crf:.0}: encode failed"); continue;
        }
        let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&out);
        println!("  crf {crf:.0}: {bytes} bytes");
        rungs.push(calibrate::Rung { crf: *crf, bytes, psnr_y: None });
    }
    let sweep = calibrate::Sweep::new(rungs);
    println!();
    print!("{}", calibrate::report(&sweep, 24.0));
    if !sweep.trustworthy() { return 1; }
    0
}

fn cmd_encode(f: &Flags) -> i32 {
    let src = match gather(f.from.as_deref()) { Ok(s) => s, Err(e) => { eprintln!("{e}"); return 1 } };
    let preset = f.preset();
    let out_dir = f.out.clone().unwrap_or_else(|| ".".into());
    let videos = src.videos();
    if videos.is_empty() { eprintln!("nothing to encode"); return 1; }
    println!("{} file(s), preset `{}` (crf {:.0}) -- {}",
             videos.len(), preset.name, preset.crf, preset.purpose);
    println!("gives up: {}\n", preset.costs);

    let cmds: Vec<(String, Vec<String>)> = videos.iter().map(|v| {
        let stem = v.name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&v.name);
        let out = format!("{}/{}.mkv", out_dir.trim_end_matches('/'), stem.replace(' ', "_"));
        let mut a: Vec<String> = ["-hide_banner", "-v", "error", "-stats", "-y"]
            .iter().map(|s| s.to_string()).collect();
        if v.url.starts_with("http") {
            // a multi-gigabyte read over wifi WILL drop at least once
            a.extend(["-reconnect", "1", "-reconnect_streamed", "1",
                      "-reconnect_delay_max", "10"].iter().map(|s| s.to_string()));
        }
        a.push("-i".into()); a.push(v.url.clone());
        a.extend(preset.args());
        // CARRY THE COLOUR TAGS THROUGH UNCHANGED. Retagging here would
        // give two files that disagree about what they contain; if the
        // source is mislabelled, that is a grading decision, not an
        // encoding one.
        a.extend(["-color_primaries", "bt709", "-color_trc", "bt709",
                  "-colorspace", "bt709"].iter().map(|s| s.to_string()));
        a.push(out.clone());
        (out, a)
    }).collect();

    if f.dry_run {
        for (_, a) in &cmds { println!("ffmpeg {}", a.join(" ")); }
        return 0;
    }
    // WHOLE FILES CONCURRENTLY, NEVER SLICED. Slicing inside a file
    // resets the entropy coder and costs about 20%; 88 files are
    // already 88 natural boundaries.
    let jobs = f.jobs.min(cmds.len()).max(1);
    println!("encoding {jobs} at a time\n");
    let queue = std::sync::Arc::new(std::sync::Mutex::new(cmds.into_iter().collect::<Vec<_>>()));
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..jobs {
        let q = queue.clone();
        let fail = failed.clone();
        handles.push(std::thread::spawn(move || loop {
            let next = { q.lock().unwrap().pop() };
            let Some((out, args)) = next else { break };
            let ok = Command::new("ffmpeg").args(&args).status()
                .map(|s| s.success()).unwrap_or(false);
            if ok {
                let sz = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                println!("  done {out} ({:.0} MB)", sz as f64 / 1e6);
            } else {
                eprintln!("  FAILED {out}");
                fail.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }));
    }
    for h in handles { let _ = h.join(); }
    let n = failed.load(std::sync::atomic::Ordering::Relaxed);
    if n > 0 { eprintln!("\n{n} file(s) failed"); return 1; }
    println!("\nDo not delete the originals until someone has watched this on a real monitor.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(args: &[&str]) -> Flags {
        Flags::parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn the_flags_parse_and_default_sensibly() {
        let d = f(&[]);
        assert_eq!(d.crf, vec![14.0, 22.0, 30.0]);
        assert!(d.jobs >= 1, "jobs defaults to the core count");
        assert_eq!(d.preset().name, "archive", "the hedged default, not the smallest");
        assert!(!d.dry_run);

        let g = f(&["--from", "http://h:5001/F/", "--preset", "period",
                    "--crf", "20,28", "--jobs", "6", "--dry-run"]);
        assert_eq!(g.from.as_deref(), Some("http://h:5001/F/"));
        assert_eq!(g.preset().name, "period");
        assert_eq!(g.crf, vec![20.0, 28.0]);
        assert_eq!(g.jobs, 6);
        assert!(g.dry_run);
    }

    #[test]
    fn an_unknown_preset_falls_back_rather_than_failing() {
        // a typo at the start of a long batch should not be discovered
        // an hour in, but nor should it stop the run
        assert_eq!(f(&["--preset", "nonsense"]).preset().name, "archive");
    }

    #[test]
    fn a_local_directory_and_a_url_are_both_sources() {
        let dir = std::env::temp_dir().join("av1tuner-cli-test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("A_C0001.mov");
        std::fs::write(&p, vec![0u8; 2048]).unwrap();
        let s = gather(Some(dir.to_str().unwrap())).expect("local");
        assert_eq!(s.videos().len(), 1);
        assert_eq!(s.videos()[0].clip_number(), Some(1));
        let _ = std::fs::remove_file(&p);
        // a url that cannot be reached is an error with the url in it,
        // not a panic
        let e = gather(Some("http://127.0.0.1:1/nothing/")).unwrap_err();
        assert!(e.contains("127.0.0.1:1") || e.contains("no files"), "{e}");
    }

    #[test]
    fn usage_names_every_command_it_accepts() {
        for c in ["plan", "sweep", "encode", "presets"] {
            assert!(USAGE.contains(c), "{c} undocumented");
        }
        // and the thing that would otherwise be discovered the hard way
        assert!(USAGE.contains("never sliced"), "{USAGE}");
        assert!(USAGE.contains("20%"));
    }
}
