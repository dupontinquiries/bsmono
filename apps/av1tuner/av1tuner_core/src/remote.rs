//! remote — footage that lives on the other laptop.
//!
//! The card is on one machine and the cores are on another. Copying
//! 391 GB across to transcode it, then copying it back, is most of a
//! day and needs the space free on both ends.
//!
//! It does not have to be copied. ffmpeg reads `http://` natively, and
//! `dufs .` in the footage directory is a one-line server. Verified:
//! `ffprobe` over plain HTTP returns codec, dimensions and duration
//! without downloading the file.
//!
//! ## But only if the server does range requests
//!
//! Camera-original QuickTime puts `mdat` first and `moov` LAST.
//! Measured on one of these files: `mdat` at byte 0, `moov` at
//! 37,044,224 -- 100% of the way in, 5,701 bytes long. The index is the
//! last thing in the file.
//!
//! So a client must read the END before it can decode the BEGINNING.
//! With HTTP range support that is one small request. Without it,
//! reading the index means pulling the entire file -- 5.8 GB to learn
//! the frame rate. Python's `http.server` fails here ("partial file",
//! no frame extracted); `dufs` implements ranges properly.
//!
//! That is why [`Source::check`] tests ranges before anything else and
//! refuses rather than proceeding: the failure mode is not an error
//! message, it is an afternoon of accidental downloading.

use std::collections::BTreeMap;

/// One file offered by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Full URL, already percent-encoded as the listing gave it.
    pub url: String,
    /// The name a human would recognise.
    pub name: String,
    pub bytes: u64,
    /// As the listing reported it, unparsed: formats vary and a wrong
    /// date is worse than no date.
    pub modified: Option<String>,
}

impl Remote {
    /// The camera's clip number, for ordering an import.
    ///
    /// `..._0929_C0062.mov` -> 62. Falls back to None rather than a
    /// guess: a batch sorted by a misread number is worse than one
    /// sorted by name.
    pub fn clip_number(&self) -> Option<u32> {
        let stem = self.name.rsplit('.').nth(1).unwrap_or(&self.name);
        let tail = stem.rsplit(['_', '-', ' ']).next()?;
        let digits: String = tail.chars().skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 6 { return None; }
        digits.parse().ok()
    }

    pub fn is_video(&self) -> bool {
        let l = self.name.to_lowercase();
        [".mov", ".mp4", ".mxf", ".mkv", ".braw", ".avi", ".m4v"]
            .iter().any(|e| l.ends_with(e))
    }
}

/// Pull the file rows out of a dufs listing.
///
/// Deliberately a narrow scrape of the one shape dufs emits, not a
/// general HTML parser. If dufs changes its markup this returns
/// nothing, which is visible; a lenient parser would return SOME rows
/// and silently skip a third of the shoot.
pub fn parse_listing(html: &str, base: &str) -> (Vec<Remote>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    // rows look like: <td class="path cell-name"><a href="URL" ...>NAME</a>
    for chunk in html.split("cell-name").skip(1) {
        let Some(href_at) = chunk.find("href=\"") else { continue };
        let rest = &chunk[href_at + 6..];
        let Some(end) = rest.find('"') else { continue };
        let url = rest[..end].to_string();
        // the visible name is between the next > and </a>
        let name = rest[end..].find('>').and_then(|gt| {
            let after = &rest[end + gt + 1..];
            after.find("</a>").map(|e| after[..e].trim().to_string())
        }).unwrap_or_default();
        if name.is_empty() || url.ends_with('/') { continue; }
        // the size cell follows in the same row
        let bytes = chunk.find("cell-size").and_then(|s| {
            let after = &chunk[s..];
            let gt = after.find('>')?;
            let e = after[gt + 1..].find('<')?;
            parse_size(after[gt + 1..gt + 1 + e].trim())
        }).unwrap_or(0);
        let modified = chunk.find("cell-mtime").and_then(|s| {
            let after = &chunk[s..];
            let gt = after.find('>')?;
            let e = after[gt + 1..].find('<')?;
            Some(after[gt + 1..gt + 1 + e].trim().to_string())
        }).filter(|s| !s.is_empty());
        let url = if url.starts_with("http") { url }
                  else { format!("{}/{}", base.trim_end_matches('/'), url.trim_start_matches('/')) };
        out.push(Remote { url, name, bytes, modified });
    }
    if out.is_empty() && html.contains("<a ") {
        notes.push("the page has links but no rows this recognises -- dufs may have changed \
                    its markup, or this is not a dufs listing".into());
    }
    (out, notes)
}

/// "5.8 GB", "642.7 MB", "58.9 MB" -> bytes.
///
/// Approximate by nature: the listing has already rounded, so this
/// recovers roughly what was there. Good enough for ordering and for
/// projecting a batch total; not for anything that must be exact.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, unit): (String, String) = s.chars().partition(|c| c.is_ascii_digit() || *c == '.');
    let v: f64 = num.parse().ok()?;
    let mult = match unit.trim().to_uppercase().as_str() {
        "B" | "" => 1.0,
        "KB" | "K" | "KIB" => 1024.0,
        "MB" | "M" | "MIB" => 1024.0 * 1024.0,
        "GB" | "G" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "TB" | "T" | "TIB" => 1024.0f64.powi(4),
        _ => return None,
    };
    Some((v * mult) as u64)
}

/// A server holding footage.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub base: String,
    pub files: Vec<Remote>,
}

/// What a source must support before a batch is worth starting.
#[derive(Debug, Clone, PartialEq)]
pub enum Readiness {
    Ready,
    /// The one that matters: without ranges, reading the index at the
    /// end of a QuickTime means downloading the whole file.
    NoRangeRequests,
    Unreachable(String),
}

impl Readiness {
    pub fn explain(&self) -> String {
        match self {
            Readiness::Ready => "ready".into(),
            Readiness::NoRangeRequests => "the server does not answer range requests. \
                Camera QuickTime files put their index (moov) at the very END -- measured at \
                100% of the way into one of these -- so every probe would download the whole \
                file to read the last few kilobytes. Use `dufs`, which supports ranges; \
                Python's http.server does not.".into(),
            Readiness::Unreachable(w) => format!("cannot reach it: {w}"),
        }
    }
    pub fn ok(&self) -> bool { matches!(self, Readiness::Ready) }
}

impl Source {
    pub fn videos(&self) -> Vec<&Remote> {
        let mut v: Vec<&Remote> = self.files.iter().filter(|f| f.is_video()).collect();
        v.sort_by_key(|f| (f.clip_number().unwrap_or(u32::MAX), f.name.clone()));
        v
    }

    pub fn total_bytes(&self) -> u64 { self.videos().iter().map(|f| f.bytes).sum() }

    /// Clips too small to be takes: false starts, a card test, a
    /// finger on the button. Worth naming rather than transcoding.
    pub fn stubs(&self, under: u64) -> Vec<&Remote> {
        self.videos().into_iter().filter(|f| f.bytes < under).collect()
    }
}

// ── deciding the batch ───────────────────────────────────────────────

/// What the batch is aiming at. One or the other -- a batch cannot
/// both hold a quality and hit a size, and pretending otherwise is how
/// people end up with neither.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Aim {
    /// Hold this quality; the size lands where it lands.
    Quality { crf: f32 },
    /// Fit in this many bytes; the quality lands where it lands.
    Size { total_bytes: u64 },
}

/// A plan for one file.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub url: String,
    pub name: String,
    pub source_bytes: u64,
    pub crf: f32,
    /// What this is expected to come out at.
    pub projected_bytes: u64,
}

/// Turn an aim into per-file settings.
///
/// For a size target this uses the measured curve rather than a guess:
/// the caller supplies a sweep from `calibrate`, and the CRF is the one
/// whose projected total fits. Refuses if the target is outside what
/// was measured, because extrapolating that curve is how a batch comes
/// out unusable.
pub fn plan(source: &Source, aim: Aim, sweep: &crate::calibrate::Sweep,
            swept_source_bytes: u64) -> (Vec<Task>, Vec<String>) {
    let mut notes = Vec::new();
    let videos = source.videos();
    if videos.is_empty() { return (Vec::new(), vec!["no video files here".into()]); }
    if !sweep.trustworthy() {
        notes.push("the quality scale on the sweep looked wrong; these numbers inherit that"
                   .into());
    }
    let crf = match aim {
        Aim::Quality { crf } => crf,
        Aim::Size { total_bytes } => {
            // scale the target down to what one swept sample represents
            let total_src = source.total_bytes().max(1);
            let share = swept_source_bytes as f64 / total_src as f64;
            let per_sample = (total_bytes as f64 * share) as u64;
            match sweep.crf_for_bytes(per_sample) {
                Some(c) => c,
                None => {
                    notes.push(format!(
                        "a {:.1} GB target needs a crf outside the swept range -- widen the \
                         sweep rather than extrapolating, the curve is not linear out there",
                        total_bytes as f64 / 1e9));
                    return (Vec::new(), notes);
                }
            }
        }
    };
    let ratio = ratio_at(sweep, crf, swept_source_bytes);
    let tasks: Vec<Task> = videos.iter().map(|f| Task {
        url: f.url.clone(), name: f.name.clone(), source_bytes: f.bytes, crf,
        projected_bytes: (f.bytes as f64 * ratio) as u64,
    }).collect();
    let projected: u64 = tasks.iter().map(|t| t.projected_bytes).sum();
    notes.push(format!("{} file(s), {:.1} GB -> about {:.1} GB at crf {crf:.1} ({:.0}:1)",
                       tasks.len(), source.total_bytes() as f64 / 1e9,
                       projected as f64 / 1e9,
                       source.total_bytes() as f64 / projected.max(1) as f64));
    (tasks, notes)
}

fn ratio_at(sweep: &crate::calibrate::Sweep, crf: f32, source_bytes: u64) -> f64 {
    if source_bytes == 0 || sweep.rungs.is_empty() { return 0.05; }
    // nearest measured rung, which beats interpolating on a curve this
    // steep
    let nearest = sweep.rungs.iter()
        .min_by(|a, b| (a.crf - crf).abs().partial_cmp(&(b.crf - crf).abs())
                .unwrap_or(std::cmp::Ordering::Equal));
    nearest.map(|r| r.bytes as f64 / source_bytes as f64).unwrap_or(0.05)
}

/// The ffmpeg invocation for one task, reading straight off the server.
///
/// No download step. The file is opened by URL and the encoder reads
/// through it; only the bytes actually needed cross the network.
pub fn command(t: &Task, preset: u32, extra: &[String]) -> Vec<String> {
    let mut v: Vec<String> = ["-hide_banner", "-v", "error", "-stats"]
        .iter().map(|s| s.to_string()).collect();
    // reconnect: a 5.8 GB read over wifi WILL drop at least once, and
    // losing an hour of encoding to a blip is avoidable
    v.extend(["-reconnect", "1", "-reconnect_streamed", "1",
              "-reconnect_delay_max", "10"].iter().map(|s| s.to_string()));
    v.push("-i".into());
    v.push(t.url.clone());
    v.extend(["-c:v", "libsvtav1"].iter().map(|s| s.to_string()));
    v.push("-crf".into());
    v.push(format!("{:.1}", t.crf));
    v.push("-preset".into());
    v.push(preset.to_string());
    v.extend(["-pix_fmt", "yuv420p10le"].iter().map(|s| s.to_string()));
    v.extend(extra.iter().cloned());
    v.push(out_name(&t.name));
    v
}

fn out_name(name: &str) -> String {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    format!("{}.mkv", stem.replace(' ', "_"))
}

/// A line per task, for the operator to read before it starts.
pub fn preview(tasks: &[Task], notes: &[String]) -> String {
    let mut s = String::new();
    for n in notes { s.push_str(&format!("{n}\n")); }
    for t in tasks.iter().take(6) {
        s.push_str(&format!("  {:<52} {:>7.1} GB -> {:>6.0} MB\n",
                            t.name.chars().take(52).collect::<String>(),
                            t.source_bytes as f64 / 1e9,
                            t.projected_bytes as f64 / 1e6));
    }
    if tasks.len() > 6 { s.push_str(&format!("  ... and {} more\n", tasks.len() - 6)); }
    s
}

/// Group by shooting session, from the timestamp the camera put in the
/// filename. Useful for doing one morning at a time rather than 88
/// files in one go.
pub fn by_session(source: &Source) -> BTreeMap<String, Vec<&Remote>> {
    let mut m: BTreeMap<String, Vec<&Remote>> = BTreeMap::new();
    for f in source.videos() {
        // ..._2026-09-10_0929_C0062.mov -> 2026-09-10
        let day = f.name.split('_').find(|p| p.len() == 10 && p.matches('-').count() == 2)
            .unwrap_or("unknown").to_string();
        m.entry(day).or_default().push(f);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibrate::{Rung, Sweep};

    /// The markup dufs actually emits, from a real page.
    const LISTING: &str = r#"
<tr id="addPath85">
  <td class="path cell-icon"><svg></svg></td>
  <td class="path cell-name">
    <a href="http://192.168.2.221:5001/Fragile%20short%20film/Blackmagic%20Production%20Camera%204K_2026-09-10_0929_C0062.mov" target="_blank">Blackmagic Production Camera 4K_2026-09-10_0929_C0062.mov</a>
  </td>
  <td class="cell-mtime">2026-09-10 10:29</td>
  <td class="cell-size">5.8 GB</td>
</tr>
<tr id="addPath86">
  <td class="path cell-name">
    <a href="http://192.168.2.221:5001/Fragile%20short%20film/Blackmagic%20Production%20Camera%204K_2026-09-10_0934_C0063.mov" target="_blank">Blackmagic Production Camera 4K_2026-09-10_0934_C0063.mov</a>
  </td>
  <td class="cell-mtime">2026-09-10 10:34</td>
  <td class="cell-size">61.2 MB</td>
</tr>
<tr id="addPath87">
  <td class="path cell-name">
    <a href="http://192.168.2.221:5001/Fragile%20short%20film/notes.txt" target="_blank">notes.txt</a>
  </td>
  <td class="cell-mtime">2026-09-10 11:00</td>
  <td class="cell-size">2.1 KB</td>
</tr>"#;

    fn source() -> Source {
        let (files, notes) = parse_listing(LISTING, "http://192.168.2.221:5001");
        assert!(notes.is_empty(), "{notes:?}");
        Source { base: "http://192.168.2.221:5001".into(), files }
    }

    fn sweep() -> Sweep {
        // measured: 26,828,306 bytes of prores in, these out
        Sweep::new(vec![
            Rung { crf: 14.0, bytes: 245_137, psnr_y: Some(46.0) },
            Rung { crf: 22.0, bytes: 134_669, psnr_y: Some(44.2) },
            Rung { crf: 30.0, bytes: 86_341, psnr_y: Some(42.6) },
        ])
    }

    #[test]
    fn it_reads_the_listing_the_server_actually_serves() {
        let s = source();
        assert_eq!(s.files.len(), 3, "{:?}", s.files);
        let v = s.videos();
        assert_eq!(v.len(), 2, "notes.txt is not footage");
        // sorted by the camera's clip number, not by name
        assert_eq!(v[0].clip_number(), Some(62));
        assert_eq!(v[1].clip_number(), Some(63));
        assert_eq!(v[0].bytes, (5.8 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(v[0].modified.as_deref(), Some("2026-09-10 10:29"));
        // the URL is kept percent-encoded exactly as given -- re-encoding
        // a path with a space in it is how these stop resolving
        assert!(v[0].url.contains("Fragile%20short%20film"), "{}", v[0].url);
    }

    #[test]
    fn a_listing_it_cannot_read_says_so_rather_than_returning_half() {
        // a lenient parser would return SOME rows and silently skip a
        // third of the shoot
        let (files, notes) = parse_listing("<html><a href='x'>a file</a></html>", "http://h");
        assert!(files.is_empty());
        assert!(notes.iter().any(|n| n.contains("may have changed its markup")), "{notes:?}");
        // and an empty page is not an error
        assert!(parse_listing("<html></html>", "http://h").1.is_empty());
    }

    #[test]
    fn the_range_requirement_is_explained_in_terms_of_what_goes_wrong() {
        let r = Readiness::NoRangeRequests;
        assert!(!r.ok());
        let e = r.explain();
        assert!(e.contains("moov") && e.contains("END"), "{e}");
        assert!(e.contains("download the whole file"), "the consequence, not the fact: {e}");
        assert!(e.contains("dufs") && e.contains("http.server"), "what to use: {e}");
        assert!(Readiness::Ready.ok());
    }

    #[test]
    fn sizes_come_back_from_the_shapes_a_listing_uses() {
        assert_eq!(parse_size("2.1 KB"), Some(2150));
        assert_eq!(parse_size("61.2 MB"), Some(64_172_851));
        assert_eq!(parse_size("5.8 GB"), Some(6_227_702_579));
        assert_eq!(parse_size("512 B"), Some(512));
        assert!(parse_size("some day").is_none());
        assert!(parse_size("").is_none());
    }

    #[test]
    fn a_quality_aim_and_a_size_aim_are_different_questions() {
        let s = source();
        let (tasks, notes) = plan(&s, Aim::Quality { crf: 22.0 }, &sweep(), 26_828_306);
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| (t.crf - 22.0).abs() < 0.01));
        assert!(notes.iter().any(|n| n.contains("->") && n.contains(":1")), "{notes:?}");
        // every task projects smaller than its source
        assert!(tasks.iter().all(|t| t.projected_bytes < t.source_bytes));
        // A SIZE AIM PICKS THE CRF INSTEAD. The target must lie inside
        // what was swept: this sweep measured 109:1 to 310:1, so on a
        // 6.3 GB batch it can reach roughly 20 to 58 MB. 200 MB is only
        // 31:1 -- more generous than anything measured -- and asking
        // for it is correctly refused rather than extrapolated.
        let target = 40_000_000u64;
        let (sized, _) = plan(&s, Aim::Size { total_bytes: target }, &sweep(), 26_828_306);
        assert!(!sized.is_empty());
        let total: u64 = sized.iter().map(|t| t.projected_bytes).sum();
        assert!(total < target * 3 && total > target / 3, "landed near the target: {total}");
    }

    #[test]
    fn a_size_target_outside_the_measured_curve_is_refused() {
        // extrapolating that curve is how a batch comes out unusable
        let s = source();
        let (tasks, notes) = plan(&s, Aim::Size { total_bytes: 1_000 }, &sweep(), 26_828_306);
        assert!(tasks.is_empty());
        assert!(notes.iter().any(|n| n.contains("widen the sweep")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("not linear out there")));
    }

    #[test]
    fn the_command_reads_from_the_server_and_survives_a_blip() {
        let s = source();
        let (tasks, _) = plan(&s, Aim::Quality { crf: 22.0 }, &sweep(), 26_828_306);
        let c = command(&tasks[0], 6, &[]);
        // NO DOWNLOAD STEP: the url is the input
        let i = c.iter().position(|x| x == "-i").unwrap();
        assert!(c[i + 1].starts_with("http://"), "{:?}", c[i + 1]);
        // a 5.8 GB read over wifi will drop at least once
        assert!(c.contains(&"-reconnect".to_string()), "{c:?}");
        assert!(c.contains(&"-reconnect_delay_max".to_string()));
        assert!(c.contains(&"yuv420p10le".to_string()), "10-bit is kept");
        // the output name loses the spaces that make shell quoting a
        // nuisance, and keeps the clip number
        assert!(c.last().unwrap().ends_with(".mkv"));
        assert!(c.last().unwrap().contains("C0062"), "{:?}", c.last());
        assert!(!c.last().unwrap().contains(' '));
    }

    #[test]
    fn tiny_clips_are_named_rather_than_silently_transcoded() {
        // a 61 MB clip next to 5.8 GB ones is a false start or a card
        // test, and spending an encode on it is waste
        let s = source();
        let stubs = s.stubs(100_000_000);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].clip_number(), Some(63));
    }

    #[test]
    fn clips_group_by_the_day_the_camera_recorded_them() {
        let s = source();
        let m = by_session(&s);
        assert_eq!(m.len(), 1);
        assert_eq!(m["2026-09-10"].len(), 2);
    }
}
