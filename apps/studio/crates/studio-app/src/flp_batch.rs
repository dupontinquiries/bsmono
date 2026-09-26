//! flp_batch — a LIBRARY of FL Studio packages into Studio projects, each
//! verified.
//!
//! An FL "zipped loop package" (File > Export > Zipped loop package) is a
//! .zip holding the project's .flp and the media it uses. `convert_package`
//! turns one into a folder: its media (optionally re-encoded as 48 kbit/s
//! Opus), a `.studio` project, and the import report -- then VERIFIES the
//! basics and says PASS, WARN (with reasons) or FAIL. `bin/flp-batch` runs a
//! whole directory and writes a summary; one bad package never stops a batch.
//!
//! What "the basics" are:
//!   * the .studio reopens, and has the tracks / clips / sources / busses the
//!     import built (a save that loses something shows here -- it has, twice);
//!   * every file source decodes, at the length the project says;
//!   * the model refused no edit, and no media was missing;
//!   * optionally (`render_check`), a few seconds from the first clip mix to
//!     finite samples that are not all silence.
//! It does not judge the MIX -- that needs the artist's render
//! (examples/compare_reference).
//!
//! The copies an import asks for (FL's time-stretched audio, varispeed,
//! automation rendered into clips) are made HERE, shared with
//! examples/flp_import: ffmpeg for encoding, Studio's own decoder for reading.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::flp_import::{Imported, Media};

/// `X.opus` beside `X.<anything>` in `media`, if there is one.
pub fn opus_for(media: &Path, file: &str) -> Option<String> {
    let stem = Path::new(file).file_stem()?.to_string_lossy().into_owned();
    let o = format!("{stem}.opus");
    media.join(&o).is_file().then_some(o)
}

/// A probe for `flp_import::import`: decodes each file once (preferring the
/// .opus beside a name FL used) and caches what it learned.
pub fn media_probe(media: PathBuf) -> impl Fn(&str) -> Option<Media> {
    let cache = std::cell::RefCell::new(BTreeMap::<String, Option<Media>>::new());
    move |file: &str| {
        if let Some(m) = cache.borrow().get(file) { return *m; }
        let real = opus_for(&media, file).unwrap_or_else(|| file.to_string());
        let m = studio_media::decode_file(&media.join(&real)).ok().map(|s| Media {
            len: studio_core::Ticks::from_samples(s.frames() as i64, s.sample_rate), rate: s.sample_rate,
            channels: s.channels.len() as u16,
            peak: s.channels.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs())) });
        cache.borrow_mut().insert(file.to_string(), m);
        m
    }
}

/// Make every copy the import asked for, and point sources at the .opus
/// beside any name FL used. Problems go into the report.
pub fn materialize(im: &mut Imported, media: &Path, probe: &dyn Fn(&str) -> Option<Media>) {
    // FL's time-stretched audio: make each stretched copy once (atempo keeps
    // pitch; FL's modes here are the pitch-keeping ones -- the report says).
    for (file, r, variant) in &im.stretches {
        let out = media.join(variant);
        if out.is_file() { continue; }
        let input = media.join(opus_for(media, file).unwrap_or_else(|| file.clone()));
        let ok = std::process::Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-y", "-i"]).arg(&input)
            .args(["-filter:a", &format!("atempo={r}"), "-c:a", "libopus", "-b:a", "48k"]).arg(&out)
            .status().map(|s| s.success()).unwrap_or(false);
        if !ok { im.report.push(format!("could not make the stretched copy {variant}")); }
    }
    // VARISPEED copies: pitch and speed together (asetrate), as FL plays a
    // pitch-shifted sampler
    for (file, r, variant) in &im.varispeeds {
        let out = media.join(variant);
        if out.is_file() { continue; }
        let input = media.join(opus_for(media, file).unwrap_or_else(|| file.clone()));
        let rate = probe(file).map(|m| m.rate).unwrap_or(48_000) as f64;
        let ok = std::process::Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-y", "-i"]).arg(&input)
            .args(["-af", &format!("asetrate={},aresample=48000", (rate * r).round()), "-c:a", "libopus", "-b:a", "48k"]).arg(&out)
            .status().map(|s| s.success()).unwrap_or(false);
        if !ok { im.report.push(format!("could not make the varispeed copy {variant}")); }
    }
    // AUTOMATED clips: render each through its curves -- varispeed (the file
    // advances 2^(cents/1200) per output second) and gain -- into its own copy.
    // Always regenerated: the copy depends on the curve, not only the name.
    for b in &im.bakes {
        let input = media.join(opus_for(media, &b.file).unwrap_or_else(|| b.file.clone()));
        let Ok(src) = studio_media::decode_file(&input) else { im.report.push(format!("could not decode {} to render {}", b.file, b.variant)); continue };
        let (sr, out_sr) = (src.sample_rate as f64, 48_000f64);
        let frames = src.frames();
        let ch = |k: usize| &src.channels[k.min(src.channels.len() - 1)];
        let read = |c: &Vec<f32>, p: f64| -> f32 {
            let i = p.floor();
            if i < 0.0 || i as usize + 1 >= frames { return 0.0; }
            let (a, t) = (i as usize, (p - i) as f32);
            c[a] * (1.0 - t) + c[a + 1] * t
        };
        let curve = |v: &Vec<(f64, f64)>, t: f64| -> f64 {
            let k = t / crate::flp_import::BAKE_STEP;
            let i = (k.floor() as usize).min(v.len().saturating_sub(1));
            let j = (i + 1).min(v.len().saturating_sub(1));
            let u = (k - k.floor()).clamp(0.0, 1.0);
            v.get(i).map(|a| a.1).unwrap_or(0.0) * (1.0 - u) + v.get(j).map(|b| b.1).unwrap_or(0.0) * u
        };
        let gains: Vec<(f64, f64)> = b.gain.iter().map(|(t, g)| (*t, *g as f64)).collect();
        let n = (b.len * out_sr).round() as usize;
        let mut pcm: Vec<u8> = Vec::with_capacity(n * 8);
        let mut pos = b.start * sr;
        for k in 0..n {
            let t = k as f64 / out_sr;
            let g = curve(&gains, t) as f32;
            let (l, r) = (read(ch(0), pos) * g, read(ch(1), pos) * g);
            pcm.extend_from_slice(&l.to_le_bytes());
            pcm.extend_from_slice(&r.to_le_bytes());
            pos += 2f64.powf(curve(&b.cents, t) / 1200.0) * sr / out_sr;
        }
        let out = media.join(&b.variant);
        let mut child = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "-", "-c:a", "libopus", "-b:a", "96k"]).arg(&out)
            .stdin(std::process::Stdio::piped()).spawn().expect("ffmpeg");
        {
            use std::io::Write;
            let _ = child.stdin.take().expect("stdin").write_all(&pcm);
        }
        if !child.wait().map(|s| s.success()).unwrap_or(false) { im.report.push(format!("could not write the automated copy {}", b.variant)); }
    }
    let mut swapped = 0;
    for src in im.project.sources.values_mut() {
        if let Some(o) = opus_for(media, &src.path) { if o != src.path { src.path = o; src.sample_rate = 48_000; swapped += 1; } }
    }
    if swapped > 0 { im.report.push(format!("media: {swapped} files as Opus (the names FL used map to the .opus beside them)")); }
}

/// How a package came out.
#[derive(Debug, Clone, PartialEq)]
pub enum Status { Pass, Warn(Vec<String>), Fail(String) }

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub package: String,
    pub status: Status,
    pub seconds: f64,
    pub tracks: usize,
    pub clips: usize,
    pub sources: usize,
    pub busses: usize,
    pub song_seconds: f64,
    /// The folder the project, media and report went into.
    pub folder: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Re-encode WAV / FLAC / MP3 / AIFF media as 48 kbit/s Opus.
    pub opus: bool,
    /// Mix a few seconds from the first clip and check the samples.
    pub render_check: bool,
}

/// The .zip packages directly in `dir`, by name.
pub fn packages(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))).collect();
    v.sort();
    v
}

fn is_audio(name: &str) -> bool {
    let l = name.to_lowercase();
    [".wav", ".flac", ".mp3", ".aif", ".aiff", ".ogg"].iter().any(|e| l.ends_with(e))
}

/// A package into `out_root/<package name>/`, verified. Never panics on a bad
/// package: it FAILs with the reason.
pub fn convert_package(zip_path: &Path, out_root: &Path, o: &Options) -> Outcome {
    let t0 = std::time::Instant::now();
    let name = zip_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = out_root.join(&name);
    let mut out = Outcome { package: name.clone(), status: Status::Pass, seconds: 0.0, tracks: 0, clips: 0, sources: 0, busses: 0,
                            song_seconds: 0.0, folder: folder.clone() };
    let r = (|| -> Result<Vec<String>, String> {
        let f = std::fs::File::open(zip_path).map_err(|e| format!("cannot open: {e}"))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a readable zip: {e}"))?;
        std::fs::create_dir_all(&folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        let mut flps: Vec<(String, Vec<u8>)> = Vec::new();
        for k in 0..z.len() {
            let mut e = z.by_index(k).map_err(|e| format!("zip entry {k}: {e}"))?;
            let full = e.name().to_string();
            if e.is_dir() || full.contains("__MACOSX") { continue; }
            let base = Path::new(&full).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            if base.is_empty() || base.starts_with('.') { continue; }
            if base.to_lowercase().ends_with(".flp") {
                let mut b = Vec::with_capacity(e.size() as usize);
                e.read_to_end(&mut b).map_err(|x| format!("{base}: {x}"))?;
                flps.push((base, b));
                continue;
            }
            let dst = folder.join(&base);
            let opus = o.opus && is_audio(&base) && !base.to_lowercase().ends_with(".opus");
            let target = if opus { folder.join(format!(".{base}.tmp")) } else { dst.clone() };
            let mut w = std::fs::File::create(&target).map_err(|x| format!("{base}: {x}"))?;
            std::io::copy(&mut e, &mut w).map_err(|x| format!("{base}: {x}"))?;
            if opus {
                let stem = Path::new(&base).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                let ok = std::process::Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-y", "-i"]).arg(&target)
                    .args(["-c:a", "libopus", "-b:a", "48k"]).arg(folder.join(format!("{stem}.opus")))
                    .status().map(|s| s.success()).unwrap_or(false);
                let _ = std::fs::remove_file(&target);
                if !ok { return Err(format!("{base}: ffmpeg could not convert it (is ffmpeg installed?)")); }
            }
        }
        let mut warns = Vec::new();
        if flps.is_empty() { return Err("no .flp in the package".into()); }
        if flps.len() > 1 { warns.push(format!("{} .flp files; converted the largest", flps.len())); }
        flps.sort_by_key(|(_, b)| std::cmp::Reverse(b.len()));
        let (flp_name, bytes) = flps.swap_remove(0);
        let inv = crate::flp_read::inventory(&bytes).map_err(|e| format!("{flp_name}: {}", e.0))?;
        drop(bytes);
        let probe = media_probe(folder.clone());
        let mut im = crate::flp_import::import(&inv, &name, &probe).map_err(|e| format!("import: {e}"))?;
        materialize(&mut im, &folder, &probe);
        let studio = folder.join(format!("{name}.studio"));
        let save = studio_collab::file::SaveFile::from_project(&im.project, Vec::new(), Some(im.rack.clone()));
        std::fs::write(&studio, save.encode()).map_err(|e| format!("cannot write {}: {e}", studio.display()))?;
        std::fs::write(folder.join(format!("{name}.import.txt")), im.report.join("\n")).map_err(|e| format!("report: {e}"))?;
        out.tracks = im.project.tracks.len();
        out.clips = im.project.clips.len();
        out.sources = im.project.sources.len();
        out.busses = im.project.busses.len();
        out.song_seconds = im.project.duration().to_seconds();
        warns.extend(verify(&studio, &im, o)?);
        Ok(warns)
    })();
    out.status = match r {
        Ok(w) if w.is_empty() => Status::Pass,
        Ok(w) => Status::Warn(w),
        Err(e) => Status::Fail(e),
    };
    out.seconds = t0.elapsed().as_secs_f64();
    out
}

/// The basics, against what the import built. Err = FAIL, Ok(warnings).
pub fn verify(studio: &Path, im: &Imported, o: &Options) -> Result<Vec<String>, String> {
    let mut w = Vec::new();
    let bytes = std::fs::read(studio).map_err(|e| format!("cannot reread the .studio: {e}"))?;
    let save = studio_collab::file::SaveFile::decode(&bytes).ok_or("the .studio does not reopen (decode refused it)")?;
    let back = save.to_project();
    let p = &im.project;
    for (what, a, b) in [("tracks", p.tracks.len(), back.tracks.len()), ("clips", p.clips.len(), back.clips.len()),
                         ("sources", p.sources.len(), back.sources.len()), ("busses", p.busses.len(), back.busses.len()),
                         ("clip fades", p.fades.len(), back.fades.len()), ("bus pans", p.bus_pans.len(), back.bus_pans.len())] {
        if a != b { return Err(format!("the .studio lost {what}: {a} built, {b} reopened")); }
    }
    let base = studio.parent().unwrap_or(Path::new("."));
    let mut undecodable = Vec::new();
    for s in back.sources.values().filter(|s| !s.is_generator()) {
        match studio_media::decode_file(&base.join(&s.path)) {
            Ok(a) => {
                let got = studio_core::Ticks::from_samples(a.frames() as i64, a.sample_rate).to_seconds();
                let want = s.len.to_seconds();
                if want > 0.5 && (got - want).abs() > 0.02 * want + 0.05 { w.push(format!("{}: {got:.2} s decoded, the project says {want:.2} s", s.path)); }
            }
            Err(_) => undecodable.push(s.path.clone()),
        }
    }
    if !undecodable.is_empty() { w.push(format!("{} source(s) missing or undecodable: {}", undecodable.len(), undecodable.join(", "))); }
    for line in im.report.iter().filter(|l| l.starts_with("refused by the model") || l.starts_with("media not found") || l.starts_with("could not")) {
        w.push(line.clone());
    }
    if o.render_check {
        let first = back.clips.values().map(|c| c.pos).min().unwrap_or(studio_core::Ticks::ZERO);
        let mut audio = BTreeMap::new();
        for s in back.sources.values().filter(|s| !s.is_generator()) {
            if let Ok(a) = studio_media::decode_file(&base.join(&s.path)) { audio.insert(s.id, a); }
        }
        let rack = save.rack.clone().unwrap_or_else(studio_live::Rack::trap_kit);
        let mut st = BTreeMap::new();
        crate::pattern_clips::refresh(&back, &rack, &mut audio, &mut st);
        let mix = studio_audio::mixdown(&back, &audio, &BTreeMap::new(), first, first.saturating_add(studio_core::Ticks::from_seconds(4.0)), 48_000, &[], &[]);
        if mix.iter().any(|x| !x.is_finite()) { return Err("the render check produced NaN / inf samples".into()); }
        if mix.iter().all(|x| x.abs() < 1e-6) { w.push("the render check (4 s from the first clip) is silent".into()); }
    }
    Ok(w)
}

/// The batch as a Markdown table, worst first.
pub fn summary(outcomes: &[Outcome]) -> String {
    let rank = |s: &Status| match s { Status::Fail(_) => 0, Status::Warn(_) => 1, Status::Pass => 2 };
    let mut v: Vec<&Outcome> = outcomes.iter().collect();
    v.sort_by_key(|o| (rank(&o.status), o.package.clone()));
    let (p, w, f) = (outcomes.iter().filter(|o| o.status == Status::Pass).count(),
                     outcomes.iter().filter(|o| matches!(o.status, Status::Warn(_))).count(),
                     outcomes.iter().filter(|o| matches!(o.status, Status::Fail(_))).count());
    let mut t = format!("# FL batch: {} packages -- {p} pass, {w} warn, {f} fail\n\n| package | status | tracks | clips | sources | busses | song | took | notes |\n|---|---|---|---|---|---|---|---|---|\n", outcomes.len());
    for o in v {
        let (st, notes) = match &o.status { Status::Pass => ("PASS", String::new()), Status::Warn(w) => ("WARN", w.join("; ")), Status::Fail(e) => ("FAIL", e.clone()) };
        t.push_str(&format!("| {} | {st} | {} | {} | {} | {} | {:.0} s | {:.1} s | {} |\n", o.package, o.tracks, o.clips, o.sources, o.busses,
                            o.song_seconds, o.seconds, notes.replace('|', "/")));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn ev_num(id: u8, v: u32) -> Vec<u8> {
        let mut b = vec![id];
        match id { 0..=63 => b.push(v as u8), 64..=127 => b.extend((v as u16).to_le_bytes()), _ => b.extend(v.to_le_bytes()) }
        b
    }
    fn ev_data(id: u8, d: &[u8]) -> Vec<u8> {
        let mut b = vec![id];
        let mut n = d.len();
        loop { let mut x = (n & 0x7F) as u8; n >>= 7; if n > 0 { x |= 0x80; } b.push(x); if n == 0 { break; } }
        b.extend_from_slice(d);
        b
    }
    fn utf16(s: &str) -> Vec<u8> { s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect() }

    /// A minimal FL project: 120 BPM, one sampler on "tone.wav", one audio
    /// clip of it (0-2 s) on track 1, a master insert.
    fn tiny_flp() -> Vec<u8> {
        let mut item = Vec::new();
        item.extend(0u32.to_le_bytes()); item.extend(20480u16.to_le_bytes()); item.extend(0u16.to_le_bytes());
        item.extend(384u32.to_le_bytes()); item.extend((500u16 - 1).to_le_bytes()); item.extend(0u16.to_le_bytes());
        item.extend([120, 0]); item.extend(0x40u16.to_le_bytes()); item.extend([64, 100, 128, 128]);
        item.extend((-1f32).to_le_bytes()); item.extend((-1f32).to_le_bytes());
        let mut track = vec![0u8; 13]; track[0] = 1; track[12] = 1;
        let body = [ev_num(156, 120_000), ev_num(64, 0), ev_num(21, 0), ev_data(203, &utf16("Tone")),
            ev_data(196, &utf16("C:\\samples\\tone.wav")), ev_num(22, 0),
            ev_data(219, &[6400i32.to_le_bytes(), 10000u32.to_le_bytes(), 0i32.to_le_bytes()].concat()),
            ev_num(99, 0), ev_data(233, &item), ev_data(238, &track),
            ev_data(236, &[0u8; 12]), ev_data(235, &[0u8; 2]), ev_num(147, 0)].concat();
        let mut f = b"FLhd".to_vec();
        f.extend(6u32.to_le_bytes()); f.extend(0u16.to_le_bytes()); f.extend(1u16.to_le_bytes()); f.extend(96u16.to_le_bytes());
        f.extend(b"FLdt"); f.extend((body.len() as u32).to_le_bytes()); f.extend(body);
        f
    }

    /// 3 s of a 220 Hz tone, 16-bit mono WAV.
    fn tone_wav() -> Vec<u8> {
        let (rate, n) = (48_000u32, 144_000usize);
        let pcm: Vec<u8> = (0..n).flat_map(|i| (((i as f32 * 220.0 / rate as f32 * std::f32::consts::TAU).sin() * 12000.0) as i16).to_le_bytes()).collect();
        let mut w = b"RIFF".to_vec();
        w.extend((36 + pcm.len() as u32).to_le_bytes()); w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes()); w.extend(1u16.to_le_bytes()); w.extend(1u16.to_le_bytes());
        w.extend(rate.to_le_bytes()); w.extend((rate * 2).to_le_bytes()); w.extend(2u16.to_le_bytes()); w.extend(16u16.to_le_bytes());
        w.extend(b"data"); w.extend((pcm.len() as u32).to_le_bytes()); w.extend(pcm);
        w
    }

    fn zip_of(path: &Path, files: &[(&str, Vec<u8>)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn a_library_converts_verifies_and_a_bad_package_does_not_stop_it() {
        let root = std::env::temp_dir().join(format!("flp-batch-{}", std::process::id()));
        let (lib, out) = (root.join("lib"), root.join("out"));
        std::fs::create_dir_all(&lib).unwrap();
        zip_of(&lib.join("good.zip"), &[("good/untitled.flp", tiny_flp()), ("good/tone.wav", tone_wav()), ("__MACOSX/good/._tone.wav", vec![0; 8])]);
        std::fs::write(lib.join("broken.zip"), b"this is not a zip").unwrap();
        zip_of(&lib.join("noflp.zip"), &[("x/tone.wav", tone_wav())]);
        let pk = packages(&lib);
        assert_eq!(pk.len(), 3);
        let o = Options { opus: false, render_check: true };
        let outs: Vec<Outcome> = pk.iter().map(|p| convert_package(p, &out, &o)).collect();
        let get = |n: &str| outs.iter().find(|o| o.package == n).unwrap();
        let good = get("good");
        assert_eq!(good.status, Status::Pass, "{:?}", good.status);
        assert_eq!((good.tracks, good.clips), (1, 1));
        assert!(good.folder.join("good.studio").is_file() && good.folder.join("tone.wav").is_file());
        assert!(!good.folder.join("._tone.wav").exists(), "macOS resource forks are skipped");
        assert!(matches!(&get("broken").status, Status::Fail(e) if e.contains("not a readable zip")), "{:?}", get("broken").status);
        assert!(matches!(&get("noflp").status, Status::Fail(e) if e.contains("no .flp")), "{:?}", get("noflp").status);
        let md = summary(&outs);
        assert!(md.starts_with("# FL batch: 3 packages -- 1 pass, 0 warn, 2 fail"), "{md}");
        let first_row = md.lines().nth(4).unwrap();
        assert!(first_row.contains("FAIL"), "worst first: {first_row}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A package whose media is missing WARNs -- converted, but said.
    #[test]
    fn missing_media_is_a_warning_not_a_failure() {
        let root = std::env::temp_dir().join(format!("flp-batch-m-{}", std::process::id()));
        std::fs::create_dir_all(root.join("lib")).unwrap();
        zip_of(&root.join("lib").join("bare.zip"), &[("bare/untitled.flp", tiny_flp())]);
        let r = convert_package(&root.join("lib").join("bare.zip"), &root.join("out"), &Options::default());
        assert!(matches!(&r.status, Status::Warn(w) if w.iter().any(|x| x.contains("media not found"))), "{:?}", r.status);
        let _ = std::fs::remove_dir_all(&root);
    }
}
