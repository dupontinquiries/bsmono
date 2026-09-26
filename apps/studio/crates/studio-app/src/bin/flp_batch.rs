//! flp-batch -- a library of FL Studio zipped loop packages into Studio
//! projects, each verified (studio_app::flp_batch).
//!
//!     flp-batch LIBRARY_DIR OUT_DIR [--opus] [--render-check]
//!
//! Every .zip directly in LIBRARY_DIR becomes OUT_DIR/<name>/ with its media,
//! <name>.studio and <name>.import.txt. OUT_DIR/BATCH.md is the summary, worst
//! first. Exit status: 0 if nothing FAILed, 1 otherwise. Needs ffmpeg on PATH
//! for --opus and for the copies some imports ask for (stretched, varispeed,
//! automation).
use studio_app::flp_batch::{convert_package, packages, summary, Options, Status};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pos: Vec<&String> = a.iter().skip(1).filter(|x| !x.starts_with("--")).collect();
    if pos.len() != 2 {
        eprintln!("usage: flp-batch LIBRARY_DIR OUT_DIR [--opus] [--render-check]");
        std::process::exit(2);
    }
    let (lib, out) = (std::path::PathBuf::from(pos[0]), std::path::PathBuf::from(pos[1]));
    let o = Options { opus: a.iter().any(|x| x == "--opus"), render_check: a.iter().any(|x| x == "--render-check") };
    let pkgs = packages(&lib);
    if pkgs.is_empty() { eprintln!("no .zip packages in {}", lib.display()); std::process::exit(2); }
    std::fs::create_dir_all(&out).expect("create OUT_DIR");
    let mut done = Vec::new();
    for (k, p) in pkgs.iter().enumerate() {
        eprint!("[{}/{}] {} ... ", k + 1, pkgs.len(), p.file_name().unwrap_or_default().to_string_lossy());
        let r = convert_package(p, &out, &o);
        eprintln!("{} ({:.1} s)", match &r.status { Status::Pass => "PASS".to_string(), Status::Warn(w) => format!("WARN: {}", w.len()), Status::Fail(e) => format!("FAIL: {e}") }, r.seconds);
        done.push(r);
        // the summary so far, so a long batch can be read while it runs
        std::fs::write(out.join("BATCH.md"), summary(&done)).expect("write BATCH.md");
    }
    let s = summary(&done);
    println!("{s}");
    std::process::exit(if done.iter().any(|o| matches!(o.status, Status::Fail(_))) { 1 } else { 0 });
}
