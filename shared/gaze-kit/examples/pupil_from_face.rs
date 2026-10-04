//! pupil_from_face <model.onnx> <dir> <out dir>: for every `*.pgm` (a Fairphone front frame, turned upright): find the face
//! with YuNet, crop a close-up around each eye landmark, run orlosky-kit's pupil detector on the crop. Writes the crops as
//! `<name>_<a|b>.pgm` into the out dir and prints `<file> <eye> <found|none> <cx cy major minor in the crop> <ms>`.
use gaze_kit::facefind::{detect_scaled, YuNet};
use orlosky_kit::{detect, Gray};
use std::path::Path;

const CROP_EYE_DISTANCES: f64 = 0.62;

fn main() {
    let mut a = std::env::args().skip(1);
    let (model, dir, out) = (a.next().expect("model"), a.next().expect("dir"), a.next().expect("out dir"));
    std::fs::create_dir_all(&out).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pgm")).collect();
    files.sort();
    let tw = 252usize;
    let first = Gray::from_pgm(&std::fs::read(&files[0]).unwrap()).unwrap().rotate_cw();
    let net = YuNet::load(Path::new(&model), tw, (first.h as f64 * tw as f64 / first.w as f64).round() as usize).expect("model");
    for f in files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        let g = Gray::from_pgm(&std::fs::read(&f).unwrap()).unwrap().rotate_cw();
        let Some(face) = detect_scaled(&net, &g, tw).unwrap().into_iter().next() else { println!("{name} - no face"); continue };
        let w = CROP_EYE_DISTANCES * face.eye_distance() as f64;
        for (tag, e) in [("a", face.eye_a), ("b", face.eye_b)] {
            let t = std::time::Instant::now();
            let crop = g.window(e.0 as f64, e.1 as f64, w, w * 0.75, 640, 480);
            let p = detect(&crop);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::fs::write(format!("{out}/{name}_{tag}.pgm"), crop.to_pgm()).unwrap();
            match p {
                Some(p) => println!("{name} {tag} found {:.1} {:.1} {:.1} {:.1} {:.1}ms", p.cx, p.cy, p.major, p.minor, ms),
                None => println!("{name} {tag} none {ms:.1}ms"),
            }
        }
    }
}
