//! The two EXIF facts a photo library needs, read from a JPEG's header with no
//! dependency: which way up the picture is (`orientation`) and when it was
//! taken (`taken_at`).
//!
//! Both the daemon (to date a photo when it is registered) and the gallery (to
//! turn a thumbnail upright) need these, which is why they live in the core
//! rather than in either binary. Anything missing or malformed reads as "no
//! answer" — upright, or `None` — never as an error: a photo with a broken
//! header is still a photo.
//!
//! What is read: APP1 `Exif\0\0`, the TIFF header (`II` little-endian or `MM`
//! big-endian), IFD0, and the Exif sub-IFD it points to (tag 0x8769).

/// The parsed subset. `orientation` is 1..=8 (1 when absent); `taken` is the
/// raw `YYYY:MM:DD HH:MM:SS` string's value as Unix seconds (see `taken_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exif {
    pub orientation: u16,
    pub taken: Option<i64>,
}

impl Default for Exif {
    fn default() -> Self {
        Exif { orientation: 1, taken: None }
    }
}

/// EXIF orientation (1..=8), or 1 when there is none or it cannot be read.
pub fn orientation(bytes: &[u8]) -> u16 {
    read(bytes).orientation
}

/// When the photo was taken, as Unix seconds, from DateTimeOriginal (0x9003 in
/// the Exif sub-IFD), else IFD0's DateTime (0x0132, when the file was last
/// written by the camera or an editor — later than the shot, but still far
/// better than the day it was copied in).
///
/// EXIF times carry NO time zone: they are the camera's wall clock. They are
/// returned as if that wall clock were UTC, and that is deliberate — the
/// library groups by UTC day (`album::Item::day`), so the day a photo files
/// under is the calendar day on the camera's clock where it was taken, which is
/// the day the person remembers. Converting with a guessed zone would move a
/// late-evening shot to the next day.
pub fn taken_at(bytes: &[u8]) -> Option<i64> {
    read(bytes).taken
}

/// Parse both facts in one pass over the header.
pub fn read(bytes: &[u8]) -> Exif {
    tiff(bytes).map(parse_tiff).unwrap_or_default()
}

/// The TIFF block inside a JPEG's APP1 Exif segment.
fn tiff(b: &[u8]) -> Option<&[u8]> {
    if b.get(0..2)? != [0xFF, 0xD8] {
        return None; // not a JPEG
    }
    let mut i = 2;
    while i + 4 <= b.len() && b[i] == 0xFF {
        let marker = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if marker == 0xDA || len < 2 {
            return None; // image data begins: no EXIF before it
        }
        if marker == 0xE1 && b.get(i + 4..i + 10)? == b"Exif\0\0" {
            let end = (i + 2 + len).min(b.len());
            return b.get(i + 10..end);
        }
        i += 2 + len;
    }
    None
}

struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, i: usize) -> Option<u16> {
        let v = self.b.get(i..i + 2)?;
        Some(if self.le { u16::from_le_bytes([v[0], v[1]]) } else { u16::from_be_bytes([v[0], v[1]]) })
    }
    fn u32(&self, i: usize) -> Option<u32> {
        let v = self.b.get(i..i + 4)?;
        Some(if self.le {
            u32::from_le_bytes([v[0], v[1], v[2], v[3]])
        } else {
            u32::from_be_bytes([v[0], v[1], v[2], v[3]])
        })
    }
    /// The entries of the IFD at `off`: (tag, type, count, entry offset).
    fn entries(&self, off: usize) -> Vec<(u16, u16, u32, usize)> {
        let Some(n) = self.u16(off) else { return Vec::new() };
        (0..(n as usize).min(512))
            .map_while(|k| {
                let e = off + 2 + k * 12;
                Some((self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?, e))
            })
            .collect()
    }
    /// An ASCII value (type 2): inline when it fits in four bytes, else at the
    /// offset the entry holds.
    fn ascii(&self, ty: u16, count: u32, e: usize) -> Option<&str> {
        if ty != 2 {
            return None;
        }
        let count = count as usize;
        let at = if count <= 4 { e + 8 } else { self.u32(e + 8)? as usize };
        let raw = self.b.get(at..at + count)?;
        let raw = raw.split(|c| *c == 0).next()?;
        std::str::from_utf8(raw).ok()
    }
}

fn parse_tiff(b: &[u8]) -> Exif {
    let le = match b.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Exif::default(),
    };
    let t = Tiff { b, le };
    let Some(ifd0) = t.u32(4) else { return Exif::default() };
    let mut out = Exif::default();
    let mut modified = None;
    let mut sub = None;
    for (tag, ty, count, e) in t.entries(ifd0 as usize) {
        match tag {
            0x0112 => {
                if let Some(v) = t.u16(e + 8) {
                    if (1..=8).contains(&v) {
                        out.orientation = v;
                    }
                }
            }
            0x0132 => modified = t.ascii(ty, count, e).and_then(parse_datetime),
            0x8769 => sub = t.u32(e + 8),
            _ => {}
        }
    }
    let original = sub.and_then(|off| {
        t.entries(off as usize)
            .into_iter()
            .find(|(tag, ..)| *tag == 0x9003)
            .and_then(|(_, ty, count, e)| t.ascii(ty, count, e))
            .and_then(parse_datetime)
    });
    out.taken = original.or(modified);
    out
}

/// `YYYY:MM:DD HH:MM:SS` as Unix seconds, the wall clock read as UTC. Cameras
/// with an unset clock write `0000:00:00 00:00:00` or blanks; those are `None`.
pub fn parse_datetime(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b':' || b[7] != b':' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if y < 1800 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + se)
}

/// (year, month, day) → days since the epoch, proleptic Gregorian. Howard
/// Hinnant's `days_from_civil`, the inverse of `album::civil_from_unix`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Test and tool helper: a JPEG APP1 segment carrying the given orientation
/// and, optionally, DateTimeOriginal (in the Exif sub-IFD) and DateTime (in
/// IFD0). Public so the gallery's and the daemon's tests build real headers
/// through one implementation rather than three hand-rolled ones.
pub fn app1_segment(orientation: u16, original: Option<&str>, modified: Option<&str>, le: bool) -> Vec<u8> {
    let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let ascii = |s: &str| {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    };
    // Layout: header(8) | IFD0 | sub-IFD | string data.
    let n0 = 1 + modified.is_some() as u16 + original.is_some() as u16;
    let ifd0_len = 2 + 12 * n0 as usize + 4;
    let sub_at = 8 + ifd0_len;
    let sub_len = if original.is_some() { 2 + 12 + 4 } else { 0 };
    let mut data_at = sub_at + sub_len;
    let mut data = Vec::new();
    let mut tiff = Vec::new();
    tiff.extend_from_slice(if le { b"II" } else { b"MM" });
    tiff.extend_from_slice(&w16(42));
    tiff.extend_from_slice(&w32(8));
    tiff.extend_from_slice(&w16(n0));
    tiff.extend_from_slice(&w16(0x0112));
    tiff.extend_from_slice(&w16(3));
    tiff.extend_from_slice(&w32(1));
    tiff.extend_from_slice(&w16(orientation));
    tiff.extend_from_slice(&[0, 0]);
    if let Some(m) = modified {
        let v = ascii(m);
        tiff.extend_from_slice(&w16(0x0132));
        tiff.extend_from_slice(&w16(2));
        tiff.extend_from_slice(&w32(v.len() as u32));
        tiff.extend_from_slice(&w32(data_at as u32));
        data_at += v.len();
        data.extend(v);
    }
    if original.is_some() {
        tiff.extend_from_slice(&w16(0x8769));
        tiff.extend_from_slice(&w16(4));
        tiff.extend_from_slice(&w32(1));
        tiff.extend_from_slice(&w32(sub_at as u32));
    }
    tiff.extend_from_slice(&w32(0));
    if let Some(o) = original {
        let v = ascii(o);
        tiff.extend_from_slice(&w16(1));
        tiff.extend_from_slice(&w16(0x9003));
        tiff.extend_from_slice(&w16(2));
        tiff.extend_from_slice(&w32(v.len() as u32));
        tiff.extend_from_slice(&w32(data_at as u32));
        tiff.extend_from_slice(&w32(0));
        data.extend(v);
    }
    tiff.extend(data);
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(&tiff);
    seg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_with(seg: Vec<u8>) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg);
        j.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        j
    }

    #[test]
    fn date_taken_prefers_the_original_over_the_modified_time() {
        for le in [true, false] {
            let j = jpeg_with(app1_segment(6, Some("2024:07:14 21:30:05"), Some("2024:08:01 09:00:00"), le));
            let e = read(&j);
            assert_eq!(e.orientation, 6, "le={le}");
            // 2024-07-14 is day 19918 since the epoch.
            assert_eq!(e.taken, Some(19_918 * 86_400 + 21 * 3600 + 30 * 60 + 5), "le={le}");
        }
        // Only IFD0's DateTime: it is used.
        let j = jpeg_with(app1_segment(1, None, Some("2024:08:01 09:00:00"), true));
        assert_eq!(taken_at(&j), Some(19_936 * 86_400 + 9 * 3600));
        // Neither: no answer.
        assert_eq!(taken_at(&jpeg_with(app1_segment(1, None, None, true))), None);
    }

    #[test]
    fn the_day_a_photo_files_under_is_the_cameras_calendar_day() {
        // A 23:50 shot must not roll into the next day: the wall clock is read
        // as UTC, and the library groups by UTC day.
        let j = jpeg_with(app1_segment(1, Some("2023:12:31 23:50:00"), None, true));
        let item = crate::album::Item { path: "x".into(), taken_at: taken_at(&j).unwrap(),
            thumbnail: None, bytes: 0, metadata_json: None };
        assert_eq!(item.day(), "2023-12-31");
    }

    #[test]
    fn unset_and_malformed_clocks_are_no_answer() {
        for s in ["0000:00:00 00:00:00", "    :  :     :  :  ", "2024-07-14 21:30:05", "2024:13:01 00:00:00", "2024:07:14", ""] {
            assert_eq!(parse_datetime(s), None, "{s:?}");
        }
        assert_eq!(parse_datetime("1970:01:01 00:00:00"), Some(0));
        assert_eq!(parse_datetime("1969:12:31 23:59:59"), Some(-1), "before the epoch floors correctly");
        // Not a JPEG, truncated, out-of-range orientation.
        assert_eq!(read(b"\x89PNG\r\n"), Exif::default());
        let full = jpeg_with(app1_segment(6, Some("2024:07:14 21:30:05"), None, true));
        assert_eq!(read(&full[..20]), Exif::default(), "truncated");
        assert_eq!(orientation(&jpeg_with(app1_segment(9, None, None, true))), 1);
    }

    #[test]
    fn days_from_civil_inverts_the_librarys_calendar() {
        for secs in [-86_400i64 * 400, -1, 0, 951_782_400 /* 2000-02-29 */, 1_788_393_600, 4_102_444_800] {
            let day = secs.div_euclid(86_400);
            let item = crate::album::Item { path: String::new(), taken_at: secs, thumbnail: None, bytes: 0, metadata_json: None };
            let d = item.day();
            let (y, m, dd) = (d[0..4].parse().unwrap(), d[5..7].parse().unwrap(), d[8..10].parse().unwrap());
            assert_eq!(days_from_civil(y, m, dd), day, "{d}");
        }
    }
}
