//! Import an FL Studio project (via its `tools/flp_dump.py` JSON inventory)
//! into a Studio project written beside the project's media.
//!
//!     cargo run -p studio-app --example flp_import -- song.flp.json MEDIA_DIR "Project name" out.studio
//!
//! Media is PROBED by decoding it with Studio's own decoder, so a file Studio
//! cannot read is reported as missing rather than placed as a silent clip.
//! The import report is printed and written beside the project as
//! `<out>.import.txt`.

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 { eprintln!("usage: flp_import INVENTORY.json MEDIA_DIR NAME OUT.studio"); std::process::exit(2); }
    let inv: serde_json::Value = serde_json::from_slice(&std::fs::read(&a[1]).expect("inventory")).expect("json");
    let media = std::path::PathBuf::from(&a[2]);
    let cache = std::cell::RefCell::new(std::collections::BTreeMap::new());
    let probe = |file: &str| -> Option<studio_app::flp_import::Media> {
        if let Some(m) = cache.borrow().get(file) { return *m; }
        let m = studio_media::decode_file(&media.join(file)).ok().map(|s| studio_app::flp_import::Media {
            len: studio_core::Ticks::from_samples(s.frames() as i64, s.sample_rate), rate: s.sample_rate,
            channels: s.channels.len() as u16 });
        cache.borrow_mut().insert(file.to_string(), m);
        m
    };
    let im = studio_app::flp_import::import(&inv, &a[3], &probe).expect("import");
    let f = studio_collab::file::SaveFile::from_project(&im.project, Vec::new(), Some(im.rack));
    std::fs::write(&a[4], f.encode()).expect("write project");
    let report = im.report.join("\n");
    std::fs::write(format!("{}.import.txt", a[4]), &report).expect("write report");
    println!("{report}");
}
