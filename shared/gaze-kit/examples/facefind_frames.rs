//! facefind_frames <model.onnx> <dir> [target_w=252]: YuNet over every `*.pgm` in a directory, each turned a quarter
//! clockwise first (a Fairphone front frame); prints `<file> <score> <face x y w h> <eye a x y> <eye b x y> ... ms`
//! in the TURNED full-size frame's pixels, or `<file> none`.
use gaze_kit::facefind::{detect_scaled, YuNet};
use orlosky_kit::Gray;
use std::path::Path;

fn main() {
    let mut a = std::env::args().skip(1);
    let (model, dir) = (a.next().expect("usage: facefind_frames <model.onnx> <dir> [target_w]"), a.next().expect("dir"));
    let tw: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(252);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pgm")).collect();
    files.sort();
    let first = Gray::from_pgm(&std::fs::read(&files[0]).unwrap()).unwrap().rotate_cw();
    let th = (first.h as f64 * tw as f64 / first.w as f64).round() as usize;
    let net = YuNet::load(Path::new(&model), tw, th).expect("load model");
    for f in files {
        let g = Gray::from_pgm(&std::fs::read(&f).unwrap()).unwrap().rotate_cw();
        let t = std::time::Instant::now();
        let faces = detect_scaled(&net, &g, tw).expect("detect");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        match faces.first() {
            Some(c) => println!("{name} {:.3} {:.1} {:.1} {:.1} {:.1} {:.1} {:.1} {:.1} {:.1} {:.0}ms", c.score, c.x, c.y, c.w, c.h, c.eye_a.0, c.eye_a.1, c.eye_b.0, c.eye_b.1, ms),
            None => println!("{name} none {ms:.0}ms"),
        }
    }
}
