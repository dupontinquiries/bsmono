//! pupil_frames <dir>: run the pupil detector over every `*.pgm` in a directory (sorted) and print one line per frame:
//! `<file> <cx> <cy> <major> <minor> <angle>` (or `<file> none`). `tools/compare_pupil.py` checks it against the
//! original Python.
use orlosky_kit::{detect_any, Gray};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: pupil_frames <dir of .pgm>");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pgm")).collect();
    files.sort();
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let g = Gray::from_pgm(&std::fs::read(&f).unwrap()).expect("a P5 pgm");
        match detect_any(&g) {
            Some(p) => println!("{name} {:.2} {:.2} {:.2} {:.2} {:.1}", p.cx, p.cy, p.major, p.minor, p.angle_deg),
            None => println!("{name} none"),
        }
    }
}
