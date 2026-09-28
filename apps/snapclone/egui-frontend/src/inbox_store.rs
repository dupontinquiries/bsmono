//! inbox_store — received snaps, on disk, until they are viewed or expire.
//!
//! Received snaps lived only in `Snapclone::received`, a capped `Vec` in
//! memory. Close the app and every unopened snap was gone — with the feed
//! still listing it, since `snap_state` only records the FINISHED ones.
//! Tapping such a tile did nothing, because `open_received` found no bytes.
//!
//! One file per snap, named by its content id, in the app's data
//! directory. The format is a header line and then the raw media bytes:
//!
//! ```text
//! sc1\t<sender_id>\t<sender_name>\t<sent_at>\t<media>\t<duration_secs>\n
//! <bytes…>
//! ```
//!
//! Tabs as the separator because a sender name can hold anything but a
//! tab or a newline, and those two are flattened on the way in. No
//! serde: the whole record is six fields, and a torn file (the process
//! killed mid-write) is refused by the header check rather than half
//! parsed.
//!
//! The promise this keeps is the ephemeral one: `remove` runs on Viewed
//! and Expired, so a snap that has been seen is not on the disk any more
//! than it is on the screen. Saving a snap is a different, deliberate act
//! (`SaveSnap`) into a different directory.

use std::path::{Path, PathBuf};

use crate::snaps::{Media, Snap};

const MAGIC: &str = "sc1";

/// Where the inbox lives: `<data_dir>/snapclone/inbox`.
pub fn default_dir() -> PathBuf {
    config_kit::data_dir("snapclone").join("inbox")
}

fn media_tag(m: Media) -> &'static str {
    match m { Media::Image => "image", Media::Video => "video", Media::Audio => "audio" }
}

fn media_of(tag: &str) -> Option<Media> {
    Some(match tag { "image" => Media::Image, "video" => Media::Video, "audio" => Media::Audio, _ => return None })
}

fn flat(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// A content id is a hex digest; anything else must not become a file
/// name. Refused rather than sanitised, so a bad id is a bug that shows.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Serialise one snap.
pub fn encode(s: &Snap) -> Vec<u8> {
    let mut out = format!("{MAGIC}\t{}\t{}\t{}\t{}\t{}\n",
        flat(&s.sender_id), flat(&s.sender_name), s.sent_at, media_tag(s.media), s.duration_secs)
        .into_bytes();
    out.extend_from_slice(&s.bytes);
    out
}

/// The inverse of `encode`. `id` comes from the file name; the header
/// does not repeat it.
pub fn decode(id: &str, data: &[u8]) -> Option<Snap> {
    let nl = data.iter().position(|b| *b == b'\n')?;
    let header = std::str::from_utf8(&data[..nl]).ok()?;
    let mut f = header.split('\t');
    if f.next()? != MAGIC { return None; }
    let sender_id = f.next()?.to_string();
    let sender_name = f.next()?.to_string();
    let sent_at: i64 = f.next()?.parse().ok()?;
    let media = media_of(f.next()?)?;
    let duration_secs: u32 = f.next()?.parse().ok()?;
    Some(Snap { id: id.to_string(), sender_id, sender_name, sent_at, media, duration_secs,
                bytes: data[nl + 1..].to_vec() })
}

fn path_for(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.snap"))
}

/// Write a snap. Atomic through a sibling temp file and a rename, the
/// same reason config_kit does it: a kill between truncate and write
/// leaves a torn record, and this one is somebody's photo.
pub fn save(dir: &Path, s: &Snap) -> std::io::Result<PathBuf> {
    if !safe_id(&s.id) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad snap id"));
    }
    std::fs::create_dir_all(dir)?;
    let path = path_for(dir, &s.id);
    let tmp = dir.join(format!(".{}.tmp.{}", s.id, std::process::id()));
    std::fs::write(&tmp, encode(s))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Forget a snap. Missing is fine: Viewed and Expired can both fire for
/// one snap, and the second removal has nothing to do.
pub fn remove(dir: &Path, id: &str) -> std::io::Result<()> {
    if !safe_id(id) { return Ok(()); }
    match std::fs::remove_file(path_for(dir, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Every snap in the inbox, newest first by `sent_at`. A file that does
/// not decode is skipped, not fatal — one torn record must not hide the
/// rest of the inbox.
pub fn load_all(dir: &Path) -> Vec<Snap> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(id) = name.strip_suffix(".snap") else { continue };
        if let Ok(data) = std::fs::read(&path) {
            if let Some(s) = decode(id, &data) { out.push(s); }
        }
    }
    out.sort_by(|a, b| b.sent_at.cmp(&a.sent_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, name: &str, at: i64) -> Snap {
        Snap { id: id.into(), sender_id: "sc-aabbccdd".into(), sender_name: name.into(),
               sent_at: at, media: Media::Image, duration_secs: 7, bytes: vec![0xff, 0xd8, 0, 1, 2, b'\n', 3] }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snapclone-inbox-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_snap_round_trips_bytes_and_all_including_a_newline_in_the_payload() {
        let s = snap("abc123", "kit\tcarson", 1_700_000_000);
        let back = decode("abc123", &encode(&s)).expect("decodes");
        assert_eq!(back.bytes, s.bytes, "a newline in the media split the record");
        assert_eq!(back.sender_name, "kit carson", "a tab in a name is flattened, not a field");
        assert_eq!(back.sent_at, s.sent_at);
        assert_eq!(back.media, Media::Image);
        assert_eq!(back.duration_secs, 7);
        assert_eq!(back.id, "abc123");
    }

    #[test]
    fn the_inbox_survives_a_restart_and_forgets_what_was_viewed() {
        let d = dir("restart");
        save(&d, &snap("older", "a", 10)).unwrap();
        save(&d, &snap("newer", "b", 20)).unwrap();
        let all = load_all(&d);
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["newer", "older"],
                   "newest first");
        remove(&d, "newer").unwrap();
        remove(&d, "newer").unwrap();
        assert_eq!(load_all(&d).len(), 1, "removed once, and removing again is fine");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_torn_file_hides_only_itself() {
        let d = dir("torn");
        save(&d, &snap("good", "a", 10)).unwrap();
        std::fs::write(d.join("bad.snap"), b"sc1\tonly\tthree").unwrap();
        std::fs::write(d.join("other.txt"), b"not a snap").unwrap();
        let all = load_all(&d);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "good");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_id_that_is_not_a_digest_cannot_become_a_path() {
        let d = dir("ids");
        let mut s = snap("x", "a", 1);
        s.id = "../../etc/passwd".into();
        assert!(save(&d, &s).is_err());
        assert!(!d.join("../../etc/passwd.snap").exists());
        assert!(remove(&d, "../x").is_ok(), "a bad id is ignored, not touched");
    }
}
