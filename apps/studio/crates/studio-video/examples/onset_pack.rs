//! Pack (or check) a real shoot as onset curves: no media, ~100 bytes/second.
//!
//!     onset_pack pack  CAM_DIR REC_DIR OUT.onsets     # WAVs in, one file out
//!     onset_pack match OUT.onsets                     # run match_pool on it
//!
//! Format: repeated records of [kind u8: 0=camera 1=recorder][name_len u8][name]
//! [len u32 LE][len bytes]. Deliberately trivial so a fixture is inspectable.
use std::io::{Read, Write};
use std::path::Path;
use studio_video::dualsync::{match_pool, Onsets, Verdict};

fn wavs(dir: &Path) -> Vec<(String, Onsets)> {
    let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "wav").unwrap_or(false)).collect();
    v.sort();
    v.into_iter().filter_map(|p| {
        let o = Onsets::of(&studio_media::decode_file(&p).ok()?)?;
        Some((p.file_stem()?.to_string_lossy().into_owned(), o))
    }).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("pack") => {
            let mut f = std::fs::File::create(&a[4]).unwrap();
            for (kind, dir) in [(0u8, &a[2]), (1u8, &a[3])] {
                for (name, o) in wavs(Path::new(dir)) {
                    let b = o.to_bytes();
                    f.write_all(&[kind, name.len() as u8]).unwrap();
                    f.write_all(name.as_bytes()).unwrap();
                    f.write_all(&(b.len() as u32).to_le_bytes()).unwrap();
                    f.write_all(&b).unwrap();
                }
            }
        }
        Some("match") => {
            let mut buf = Vec::new();
            std::fs::File::open(&a[2]).unwrap().read_to_end(&mut buf).unwrap();
            let (mut cams, mut recs) = (Vec::new(), Vec::new());
            let mut i = 0;
            while i < buf.len() {
                let (kind, nl) = (buf[i], buf[i + 1] as usize);
                let name = String::from_utf8_lossy(&buf[i + 2..i + 2 + nl]).into_owned();
                let at = i + 2 + nl;
                let len = u32::from_le_bytes(buf[at..at + 4].try_into().unwrap()) as usize;
                let o = Onsets::from_bytes(&buf[at + 4..at + 4 + len]);
                if kind == 0 { cams.push((name, o)) } else { recs.push((name, o)) }
                i = at + 4 + len;
            }
            let pics: Vec<Onsets> = cams.iter().map(|c| c.1.clone()).collect();
            let snds: Vec<Onsets> = recs.iter().map(|r| r.1.clone()).collect();
            let mut sure = 0;
            for (c, m) in cams.iter().zip(match_pool(&pics, &snds)) {
                if let Some(m) = m {
                    if m.verdict == Verdict::Sure { sure += 1; }
                    println!("{}\t{:?}\t{}\t{:+.2}\t{:.1}", c.0, m.verdict, recs[m.sound].0, m.offset_seconds(), m.z);
                }
            }
            eprintln!("{sure} sure of {}", cams.len());
        }
        _ => eprintln!("usage: onset_pack pack CAM REC OUT | match OUT"),
    }
}
