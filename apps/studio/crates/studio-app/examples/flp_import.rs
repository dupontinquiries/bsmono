//! Import an FL Studio project into a Studio project written beside its media.
//!
//!     cargo run -p studio-app --example flp_import -- song.flp MEDIA_DIR "Project name" out.studio
//!
//! The first argument is the `.flp` itself (read natively, `flp_read`) or a
//! `tools/flp_dump.py` inventory (`.json`). Media is PROBED by decoding it
//! with Studio's own decoder, so a file Studio cannot read is reported
//! missing rather than placed silent. Where FL names `X.wav` and `X.opus` is
//! beside it, the Opus file is used and the project points at it. The import
//! report is printed and written as `<out>.import.txt`.

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 { eprintln!("usage: flp_import SONG.flp|INVENTORY.json MEDIA_DIR NAME OUT.studio"); std::process::exit(2); }
    let bytes = std::fs::read(&a[1]).expect("read the project");
    let inv: serde_json::Value = if a[1].to_lowercase().ends_with(".flp") {
        studio_app::flp_read::inventory(&bytes).unwrap_or_else(|e| panic!("{}: {}", a[1], e.0))
    } else { serde_json::from_slice(&bytes).expect("json") };
    // EXPERIMENT: `--drop-flag N` leaves out playlist items whose flags word has
    // bit(s) N (FL writes 0x40 on every item, 0x2000 on some -- meaning not
    // known yet; this is how to test it against a reference render).
    let mut inv = inv;
    if let Some(n) = a.iter().position(|x| x == "--drop-flag").and_then(|i| a.get(i + 1)) {
        let bit = u64::from_str_radix(n.trim_start_matches("0x"), 16).expect("hex flag");
        let mut dropped = 0;
        for arr in inv["arrangements"].as_array_mut().into_iter().flatten() {
            for t in arr["tracks"].as_array_mut().into_iter().flatten() {
                if let Some(items) = t["items"].as_array_mut() {
                    let before = items.len();
                    items.retain(|it| it["flags"].as_u64().is_none_or(|f| f & bit == 0));
                    dropped += before - items.len();
                }
            }
        }
        eprintln!("dropped {dropped} items with flag {n}");
    }
    let media = std::path::PathBuf::from(&a[2]);
    let opus_for = |file: &str| -> Option<String> {
        let stem = std::path::Path::new(file).file_stem()?.to_string_lossy().into_owned();
        let o = format!("{stem}.opus");
        media.join(&o).is_file().then_some(o)
    };
    let cache = std::cell::RefCell::new(std::collections::BTreeMap::new());
    let probe = |file: &str| -> Option<studio_app::flp_import::Media> {
        if let Some(m) = cache.borrow().get(file) { return *m; }
        let real = opus_for(file).unwrap_or_else(|| file.to_string());
        let m = studio_media::decode_file(&media.join(&real)).ok().map(|s| studio_app::flp_import::Media {
            len: studio_core::Ticks::from_samples(s.frames() as i64, s.sample_rate), rate: s.sample_rate,
            channels: s.channels.len() as u16,
            peak: s.channels.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs())) });
        cache.borrow_mut().insert(file.to_string(), m);
        m
    };
    let mut im = studio_app::flp_import::import(&inv, &a[3], &probe).expect("import");
    // FL's time-stretched audio: make each stretched copy once (atempo keeps
    // pitch; FL's modes here are the pitch-keeping ones -- the report says).
    for (file, r, variant) in &im.stretches {
        let out = media.join(variant);
        if out.is_file() { continue; }
        let input = media.join(opus_for(file).unwrap_or_else(|| file.clone()));
        let ok = std::process::Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-y", "-i"]).arg(&input)
            .args(["-filter:a", &format!("atempo={r}"), "-c:a", "libopus", "-b:a", "48k"]).arg(&out)
            .status().map(|s| s.success()).unwrap_or(false);
        if !ok { im.report.push(format!("could not make the stretched copy {variant}")); }
    }
    let mut swapped = 0;
    for src in im.project.sources.values_mut() {
        if let Some(o) = opus_for(&src.path) { src.path = o; src.sample_rate = 48_000; swapped += 1; }
    }
    if swapped > 0 { im.report.push(format!("media: {swapped} files as Opus (the .wav names FL used map to the .opus beside them)")); }
    let f = studio_collab::file::SaveFile::from_project(&im.project, Vec::new(), Some(im.rack));
    std::fs::write(&a[4], f.encode()).expect("write project");
    let report = im.report.join("\n");
    std::fs::write(format!("{}.import.txt", a[4]), &report).expect("write report");
    println!("{report}");
}
