//! store — where the photos are, and the roll that remembers what became
//! of them.
//!
//! ## The capture directory is app-private on purpose
//!
//! `$XDG_DATA_HOME/snapshot-cam/roll/`, not `~/Pictures`. This app's rule
//! is that a photo is on this device and nowhere else until the person
//! says otherwise — and "in the photo library every other app scans" is
//! somewhere else. Backing up to photosync is the act that puts a copy in
//! the library; before it, the file is here and only here.
//!
//! ## The roll file
//!
//! One line per shot, tab-separated, newest first, beside the photos:
//! `file  w  h  taken_ms  camera  disposition  handoff`. The disposition
//! is what this app must never lose: a photo shared with someone is
//! shared whether or not the app restarted, and a roll that forgot would
//! show "local" over a picture somebody else has — the one lie this app
//! is built to avoid.

use crate::{Disposition, Roll, ShareTarget, Shot};
use std::path::{Path, PathBuf};

pub const APP: &str = "snapshot-cam";
pub const ROLL_FILE: &str = "roll.tsv";
/// JPEG quality for captures. 90 keeps a phone frame under ~300 kB without
/// visible blocking; the roll is not an archive of raw sensor data.
pub const JPEG_QUALITY: u8 = 90;

/// The capture directory.
pub fn capture_dir() -> PathBuf { config_kit::data_dir(APP).join("roll") }

/// A disposition as one token in the roll file.
pub fn encode_disposition(d: &Disposition) -> String {
    match d {
        Disposition::Local => "local".into(),
        Disposition::Pending(t) => format!("pending:{}", encode_target(t)),
        Disposition::Shared(t) => format!("shared:{}", encode_target(t)),
        Disposition::Failed(t, why) => format!("failed:{}:{}", encode_target(t), why.replace(['\t', '\n'], " ")),
    }
}

fn encode_target(t: &ShareTarget) -> String {
    match t {
        ShareTarget::PhotoSync => "photosync".into(),
        ShareTarget::Contact(id) => format!("contact={id}"),
    }
}

fn decode_target(s: &str) -> Option<ShareTarget> {
    if s == "photosync" { return Some(ShareTarget::PhotoSync); }
    s.strip_prefix("contact=").filter(|id| !id.is_empty()).map(|id| ShareTarget::Contact(id.to_string()))
}

/// The token back. `None` for a token this version does not know — the
/// caller keeps the shot as local rather than dropping the line, because
/// losing a photo from the roll is worse than mislabelling its state.
pub fn decode_disposition(s: &str) -> Option<Disposition> {
    if s == "local" { return Some(Disposition::Local); }
    if let Some(t) = s.strip_prefix("pending:") { return decode_target(t).map(Disposition::Pending); }
    if let Some(t) = s.strip_prefix("shared:") { return decode_target(t).map(Disposition::Shared); }
    if let Some(rest) = s.strip_prefix("failed:") {
        // The target has no colon in it (photosync, contact=sc-…), so the
        // first colon after it starts the reason.
        let (t, why) = rest.split_once(':').unwrap_or((rest, ""));
        return decode_target(t).map(|t| Disposition::Failed(t, why.to_string()));
    }
    None
}

/// The roll as text.
pub fn render(roll: &Roll) -> String {
    let mut out = String::from("# snapshot-cam roll: file\tw\th\ttaken_ms\tcamera\tdisposition\thandoff\tsent_to\n");
    for s in roll.shots() {
        out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", s.file, s.width, s.height, s.taken_ms,
            s.camera, encode_disposition(s.disposition()), s.handoff.as_deref().unwrap_or(""), s.sent_to.join(",")));
    }
    out
}

/// Text back to a roll. Malformed lines are skipped; an unknown
/// disposition keeps the shot as local.
pub fn parse(text: &str) -> Roll {
    let mut shots: Vec<Shot> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() { continue; }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 6 || f[0].is_empty() { continue; }
        let (Ok(w), Ok(h), Ok(t)) = (f[1].parse::<u32>(), f[2].parse::<u32>(), f[3].parse::<u64>()) else { continue };
        let mut s = Shot::new(f[0], w, h, t, f[4]);
        if let Some(d) = decode_disposition(f[5]) { s = s.with_disposition(d); }
        s.handoff = f.get(6).filter(|v| !v.is_empty()).map(|v| v.to_string());
        s.sent_to = f.get(7).map(|v| v.split(',').filter(|x| !x.is_empty()).map(String::from).collect()).unwrap_or_default();
        // A roll written before `sent_to` existed: the disposition is all
        // there is, and a contact in it is a contact who has the photo.
        if s.sent_to.is_empty() {
            if let Disposition::Pending(ShareTarget::Contact(id)) | Disposition::Shared(ShareTarget::Contact(id))
                | Disposition::Failed(ShareTarget::Contact(id), _) = s.disposition() {
                s.sent_to.push(id.clone());
            }
        }
        shots.push(s);
    }
    // The file is newest-first; `push` inserts at the front, so feed it
    // oldest-first to land in the same order.
    let mut roll = Roll::default();
    for s in shots.into_iter().rev() { roll.push(s); }
    roll
}

pub fn load(dir: &Path) -> Roll {
    std::fs::read_to_string(dir.join(ROLL_FILE)).map(|t| parse(&t)).unwrap_or_default()
}

/// Write the roll atomically: a temp file, then a rename, so a crash
/// mid-write leaves the previous roll rather than half of a new one.
pub fn save(dir: &Path, roll: &Roll) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{ROLL_FILE}.tmp"));
    std::fs::write(&tmp, render(roll))?;
    std::fs::rename(tmp, dir.join(ROLL_FILE))
}

/// Encode an RGBA frame as JPEG into `path`.
/// `name`, or `name` with `-2`, `-3`… before the extension — the first that
/// is not already in `dir` (or its trash). Filenames have one-second
/// resolution, and two shutter presses in one second used to write the same
/// file: the second photo silently replaced the first.
pub fn free_name(dir: &Path, name: &str) -> String {
    let taken = |n: &str| dir.join(n).exists() || dir.join(".trash").join(n).exists();
    if !taken(name) { return name.to_string(); }
    let (stem, ext) = name.rsplit_once('.').map(|(s, e)| (s, format!(".{e}"))).unwrap_or((name, String::new()));
    (2..).map(|i| format!("{stem}-{i}{ext}")).find(|n| !taken(n)).expect("an unbounded range finds one")
}

pub fn save_jpeg(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if rgba.len() != (width as usize) * (height as usize) * 4 { return Err("frame size does not match its geometry".into()); }
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    // JPEG has no alpha; the encoder refuses RGBA, so drop it.
    let rgb: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let img = image::RgbImage::from_raw(width, height, rgb).ok_or("bad frame")?;
    // Encoded in memory, written to a hidden temporary beside it, renamed
    // into place: a crash, a kill or a full disk mid-write leaves no
    // truncated JPEG on the roll (found by killing the app the instant a
    // burst merge's file appeared).
    let mut bytes = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY);
    img.write_with_encoder(enc).map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or("no file name")?.to_string_lossy().to_string();
    let tmp = path.with_file_name(format!(".{name}.part"));
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(&bytes)?;
        f.sync_all()
    }).and_then(|_| std::fs::rename(&tmp, path));
    if written.is_err() { let _ = std::fs::remove_file(&tmp); }
    written.map_err(|e| e.to_string())
}

/// A small RGBA thumbnail of a frame, nearest-neighbour, for the roll
/// list. `side` is the long edge.
pub fn thumbnail(width: u32, height: u32, rgba: &[u8], side: u32) -> (u32, u32, Vec<u8>) {
    if width == 0 || height == 0 || rgba.len() < (width as usize) * (height as usize) * 4 { return (0, 0, Vec::new()); }
    let scale = side as f32 / width.max(height) as f32;
    let (tw, th) = (((width as f32 * scale) as u32).max(1), ((height as f32 * scale) as u32).max(1));
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        let sy = (y as f32 / scale) as u32;
        for x in 0..tw {
            let sx = (x as f32 / scale) as u32;
            let i = ((sy.min(height - 1) * width + sx.min(width - 1)) * 4) as usize;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    (tw, th, out)
}

/// A thumbnail from a saved JPEG, for shots taken before this launch.
pub fn thumbnail_from_file(path: &Path, side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::open(path).ok()?.thumbnail(side, side).to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_photos_in_one_second_get_two_files() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(free_name(d.path(), "cam-1.jpg"), "cam-1.jpg");
        std::fs::write(d.path().join("cam-1.jpg"), b"x").unwrap();
        assert_eq!(free_name(d.path(), "cam-1.jpg"), "cam-1-2.jpg");
        std::fs::write(d.path().join("cam-1-2.jpg"), b"x").unwrap();
        std::fs::create_dir_all(d.path().join(".trash")).unwrap();
        std::fs::write(d.path().join(".trash").join("cam-1-3.jpg"), b"x").unwrap();
        assert_eq!(free_name(d.path(), "cam-1.jpg"), "cam-1-4.jpg", "a trashed name is not reused either");
    }

    #[test]
    fn every_disposition_survives_the_roll_file() {
        let mut r = Roll::default();
        r.push(Shot::new("a.jpg", 640, 480, 1, "front"));
        r.push(Shot::new("b.jpg", 640, 480, 2, "front"));
        r.push(Shot::new("c.jpg", 640, 480, 3, "rear"));
        r.push(Shot::new("d.jpg", 640, 480, 4, "rear"));
        let shots = r.shots_mut();
        shots[0].share_to(ShareTarget::Contact("sc-ada".into())).unwrap();
        shots[0].handoff = Some("1-2-3".into());
        shots[1].share_to(ShareTarget::PhotoSync).unwrap();
        shots[1].confirm_sent();
        shots[2].share_to(ShareTarget::Contact("sc-bee".into())).unwrap();
        shots[2].fail("peer unreachable: no route");
        shots[3].share_to(ShareTarget::Contact("sc-ada".into())).unwrap();
        shots[3].confirm_sent();
        shots[3].share_to(ShareTarget::PhotoSync).unwrap();
        let back = parse(&render(&r));
        assert_eq!(back.shots(), r.shots(), "\n{}", render(&r));
        assert_eq!(back.shots()[3].sent_to, vec!["sc-ada"], "the sticky record survives");
        assert!(back.shots()[3].disclosed());
        assert_eq!(back.shots()[0].file, "d.jpg", "newest first, as pushed");
        assert_eq!(back.disclosed().len(), 3, "the pending, the failed and the backed-up-after-sending all count");
    }

    #[test]
    fn a_line_with_an_unknown_disposition_keeps_the_photo_as_local() {
        // A newer version's token must not drop the photo from the roll.
        let r = parse("x.jpg\t1\t1\t9\tcam\tteleported:mars\t\n");
        assert_eq!(r.len(), 1);
        assert!(r.shots()[0].is_local());
        assert!(parse("garbage line\n\t\t\n").is_empty());
    }

    #[test]
    fn a_roll_from_before_sent_to_existed_still_knows_who_has_the_photo() {
        let r = parse("x.jpg\t1\t1\t9\tcam\tshared:contact=sc-bee\t\n");
        assert_eq!(r.shots()[0].sent_to, vec!["sc-bee"]);
    }

    #[test]
    fn a_reason_with_a_colon_in_it_comes_back_whole() {
        let d = Disposition::Failed(ShareTarget::Contact("sc-x".into()), "peer unreachable: no route".into());
        assert_eq!(decode_disposition(&encode_disposition(&d)), Some(d));
    }

    #[test]
    fn a_frame_becomes_a_jpeg_on_disk_and_a_thumbnail() {
        let d = tempfile::tempdir().unwrap();
        let (w, h) = (64u32, 48u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (i, px) in rgba.chunks_mut(4).enumerate() {
            px[0] = (i % 64) as u8 * 4; px[1] = 90; px[2] = 200; px[3] = 255;
        }
        let path = d.path().join("cam-1.jpg");
        save_jpeg(&path, w, h, &rgba).unwrap();
        let back = image::open(&path).unwrap();
        assert_eq!((back.width(), back.height()), (w, h));
        assert!(save_jpeg(&d.path().join("bad.jpg"), w, h, &rgba[..10]).is_err(), "a torn frame is refused");
        let (tw, th, px) = thumbnail(w, h, &rgba, 16);
        assert_eq!((tw, th), (16, 12));
        assert_eq!(px.len(), 16 * 12 * 4);
        let (fw, fh, _) = thumbnail_from_file(&path, 16).unwrap();
        assert_eq!((fw, fh), (16, 12));
    }

    #[test]
    fn save_and_load_round_trip_through_the_directory() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Roll::default();
        r.push(Shot::new("a.jpg", 1, 1, 1, "c"));
        save(d.path(), &r).unwrap();
        assert_eq!(load(d.path()).shots(), r.shots());
        assert!(load(Path::new("/nonexistent-roll-dir")).is_empty());
    }
}
