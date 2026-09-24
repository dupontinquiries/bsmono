//! Pair a folder of camera takes with a folder of recorder files.
//!
//!     cargo run --release -p studio-video --example dualsync_pool -- CAM_DIR REC_DIR
//!
//! Both folders hold audio the decoder reads (WAV is the tested path; pull a
//! camera file's audio with `ffmpeg -i C0001.mkv -vn -ac 1 C0001.wav`). Prints
//! one tab-separated line per camera file: file, verdict, recorder file,
//! offset (add to a recorder position to get the camera position), z, runner-up.
use std::path::{Path, PathBuf};
use studio_video::dualsync::{match_pool, Onsets, Verdict};

fn load(dir: &Path) -> Vec<(String, Option<Onsets>)> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).expect("readable dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "wav").unwrap_or(false))
        .collect();
    v.sort();
    v.into_iter().map(|p| {
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let on = studio_media::decode_file(&p).ok().and_then(|a| Onsets::of(&a));
        (name, on)
    }).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 3 { eprintln!("usage: dualsync_pool CAM_DIR REC_DIR"); std::process::exit(2); }
    let cams = load(Path::new(&a[1]));
    let recs: Vec<(String, Onsets)> = load(Path::new(&a[2])).into_iter()
        .filter_map(|(n, o)| o.map(|o| (n, o))).collect();
    let (cam_names, cam_on): (Vec<_>, Vec<_>) = cams.into_iter().filter_map(|(n, o)| o.map(|o| (n, o))).unzip();
    let rec_on: Vec<Onsets> = recs.iter().map(|r| r.1.clone()).collect();
    let t = std::time::Instant::now();
    let res = match_pool(&cam_on, &rec_on);
    let mut sure = 0;
    println!("camera\tverdict\trecorder\toffset_s\tz\trunner_up\trunner_z");
    for (name, m) in cam_names.iter().zip(res) {
        match m {
            Some(m) => {
                if m.verdict == Verdict::Sure { sure += 1; }
                let (ru, rz) = m.runner_up.map(|(i, z)| (recs[i].0.as_str(), z)).unwrap_or(("-", 0.0));
                println!("{name}\t{:?}\t{}\t{:+.2}\t{:.1}\t{ru}\t{rz:.1}",
                         m.verdict, recs[m.sound].0, m.offset_seconds(), m.z);
            }
            None => println!("{name}\tnone"),
        }
    }
    eprintln!("{sure} sure of {} camera takes against {} recorder files in {:.1?}",
              cam_names.len(), recs.len(), t.elapsed());
}
