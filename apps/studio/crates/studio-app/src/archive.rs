//! archive — the session as ONE file, audio included.
//!
//! The folder-of-relative-paths save is fine on the machine that made it
//! and a liability everywhere else: a moved sample, a renamed take, a
//! half-synced directory, and the session opens with holes. The archive is
//! the professional artifact: **all state** (the `SaveFile` — snapshot ops
//! with mixer busses/chains/sends/automation, the rack with patterns and
//! rolls, optional history) **and every reference recording the session
//! used**, embedded, in one file that either opens whole or refuses whole.
//!
//! ## The fidelity slider
//!
//! Embedding audio costs size, so the level is a choice, not a default:
//!
//! * proxy levels re-encode each source to Opus at the level's bitrate —
//!   the musician's proxy workflow: the archive always PLAYS, everywhere,
//!   and sounds like the session;
//! * `Master` embeds the untouched original bytes when the session holds
//!   them (a `.opus` stays `.opus`, an import's file stays itself), and a
//!   dithered 16-bit WAV of the decoded audio when it does not (a bounce,
//!   a take) — never silence, never a hole.
//!
//! Proxies come with an upgrade path: [`ArchiveMeta::search_paths`] name
//! where full-quality per-device artifacts live, and [`find_upgrade`]
//! checks them on open — lossless extensions first, because finding a
//! *worse* file than the embedded proxy would be an upgrade in name only.
//! A device that has the WAVs plays the WAVs; a phone that has nothing
//! plays the proxies; the session is the same session on both.
//!
//! ## Refusal discipline
//!
//! Same as the collab file format: magic checked, every length checked,
//! any malformation or truncation refuses the WHOLE archive — an archive
//! that half-opens is how files get lost, which is the failure this
//! format exists to end.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use studio_audio::SourceAudio;
use studio_collab::file::SaveFile;
use studio_core::{Op, SourceId};

pub const MAGIC: &[u8; 8] = b"STUARCH1";
const VERSION: u8 = 1;

/// The fidelity presets the save popup's slider walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 64 kbps proxies — a session that fits in a chat message.
    Sketch,
    /// 96 kbps proxies — demos and phone-first collaboration.
    Demo,
    /// 160 kbps proxies — transparent for most material.
    Standard,
    /// Originals byte-for-byte where held, dithered 16-bit WAV where not.
    Master,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Sketch, Level::Demo, Level::Standard, Level::Master];
    pub fn label(self) -> &'static str {
        match self {
            Level::Sketch => "sketch (64k proxies)",
            Level::Demo => "demo (96k proxies)",
            Level::Standard => "standard (160k proxies)",
            Level::Master => "master (originals)",
        }
    }
    /// The proxy bitrate, or `None` for the originals level.
    pub fn opus_bitrate(self) -> Option<u32> {
        match self {
            Level::Sketch => Some(64_000),
            Level::Demo => Some(96_000),
            Level::Standard => Some(160_000),
            Level::Master => None,
        }
    }
}

/// Who and where: the credits, the collaborators the archive is authorized
/// for, and where each device may find better bytes than the embedded ones.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveMeta {
    /// Credits / authorized collaborators — names or snapclone sc-IDs; the
    /// roster a team session would check is derived from these, the same
    /// contacts-are-the-account rule collab already follows.
    pub artists: Vec<String>,
    /// Relative (to the archive) or absolute directories to search for
    /// higher-quality per-device artifacts of each embedded proxy.
    pub search_paths: Vec<String>,
    pub level: Level,
}

/// How one embedded media entry was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Re-encoded proxy at the archive's bitrate.
    OpusProxy,
    /// The session's original bytes, untouched.
    Original,
    /// Lossless fallback for audio the session held only as PCM.
    Wav,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaEntry {
    pub source: SourceId,
    pub kind: MediaKind,
    pub ext: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unpacked {
    pub save: SaveFile,
    pub meta: ArchiveMeta,
    pub media: Vec<MediaEntry>,
}

// ── the tiny length-checked codec ───────────────────────────────────────

struct R<'a> { b: &'a [u8], i: usize }
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.i.checked_add(n)?;
        if end > self.b.len() { return None; }
        let s = &self.b[self.i..end];
        self.i = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    fn u16(&mut self) -> Option<u16> { Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?)) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn u64(&mut self) -> Option<u64> { Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?)) }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn blob(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        Some(self.take(n)?.to_vec())
    }
}
fn put_str(o: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    o.extend((b.len().min(u16::MAX as usize) as u16).to_le_bytes());
    o.extend(&b[..b.len().min(u16::MAX as usize)]);
}
fn put_blob(o: &mut Vec<u8>, b: &[u8]) {
    o.extend((b.len() as u32).to_le_bytes());
    o.extend(b);
}

// ── audio conditioning for the proxy encoder ────────────────────────────

/// Linear resample to Opus's contractual 48k, capped to stereo — the same
/// quality stance every other resample here takes, applied at ARCHIVE time
/// where a proxy is the stated goal.
fn condition_for_opus(a: &SourceAudio) -> SourceAudio {
    let chans: Vec<&Vec<f32>> = a.channels.iter().take(2).collect();
    if a.sample_rate == 48_000 {
        return SourceAudio { sample_rate: 48_000,
            channels: chans.into_iter().cloned().collect() };
    }
    let ratio = a.sample_rate as f64 / 48_000.0;
    let out_len = (a.frames() as f64 / ratio) as usize;
    let channels = chans.into_iter().map(|c| {
        (0..out_len).map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos as usize;
            if i0 + 1 >= c.len() { *c.last().unwrap_or(&0.0) }
            else {
                let f = (pos - i0 as f64) as f32;
                c[i0] * (1.0 - f) + c[i0 + 1] * f
            }
        }).collect()
    }).collect();
    SourceAudio { sample_rate: 48_000, channels }
}

fn wav_bytes(a: &SourceAudio, seed: u64) -> Result<Vec<u8>, String> {
    let ch = a.channels.len().max(1);
    let mut inter = Vec::with_capacity(a.frames() * ch);
    for f in 0..a.frames() {
        for c in &a.channels { inter.push(*c.get(f).unwrap_or(&0.0)); }
    }
    studio_audio::export::export_wav_dithered_bytes(&inter, ch as u16, a.sample_rate, seed)
        .map_err(|e| format!("wav: {e}"))
}

// ── pack / unpack ───────────────────────────────────────────────────────

/// Build the archive. Returns the bytes plus the sources the session holds
/// NO audio for — embedded as nothing, reported out loud, because a
/// missing sample is a fact the person saving should hear now, not a hole
/// a collaborator finds later.
pub fn pack(
    save: &SaveFile,
    audio: &BTreeMap<SourceId, SourceAudio>,
    originals: &BTreeMap<SourceId, (String, Vec<u8>)>,
    meta: &ArchiveMeta,
) -> Result<(Vec<u8>, Vec<SourceId>), String> {
    let mut o = Vec::new();
    o.extend(MAGIC);
    o.push(VERSION);
    o.extend((meta.artists.len().min(u16::MAX as usize) as u16).to_le_bytes());
    for a in &meta.artists { put_str(&mut o, a); }
    o.extend((meta.search_paths.len().min(u16::MAX as usize) as u16).to_le_bytes());
    for p in &meta.search_paths { put_str(&mut o, p); }
    o.push(match meta.level { Level::Sketch => 0, Level::Demo => 1,
                              Level::Standard => 2, Level::Master => 3 });
    put_blob(&mut o, &save.encode());

    let sources: Vec<&studio_core::Source> = save.snapshot.iter().filter_map(|op| {
        if let Op::AddSource { source } = op { Some(source) } else { None }
    }).collect();
    let mut entries: Vec<MediaEntry> = Vec::new();
    let mut missing = Vec::new();
    for src in sources {
        let entry = match meta.level.opus_bitrate() {
            None => {
                // Master: the untouched original where the session holds
                // one, lossless PCM where it does not.
                if let Some((ext, bytes)) = originals.get(&src.id) {
                    Some(MediaEntry { source: src.id, kind: MediaKind::Original,
                                      ext: ext.clone(), bytes: bytes.clone() })
                } else if let Some(a) = audio.get(&src.id) {
                    Some(MediaEntry { source: src.id, kind: MediaKind::Wav,
                                      ext: "wav".into(), bytes: wav_bytes(a, src.id.0)? })
                } else { None }
            }
            Some(bitrate) => {
                if let Some(a) = audio.get(&src.id) {
                    let cond = condition_for_opus(a);
                    match studio_media::encode_opus(&cond, bitrate as i32) {
                        Ok(bytes) => Some(MediaEntry { source: src.id,
                            kind: MediaKind::OpusProxy, ext: "opus".into(), bytes }),
                        // A build without the opus feature still archives —
                        // lossless is a bigger file, never a hole.
                        Err(_) => Some(MediaEntry { source: src.id, kind: MediaKind::Wav,
                            ext: "wav".into(), bytes: wav_bytes(&cond, src.id.0)? }),
                    }
                } else if let Some((ext, bytes)) = originals.get(&src.id) {
                    // PCM never decoded but the compressed original is held
                    // (media that arrived over the wire, say): embed it —
                    // it is already its own best proxy.
                    Some(MediaEntry { source: src.id, kind: MediaKind::Original,
                                      ext: ext.clone(), bytes: bytes.clone() })
                } else { None }
            }
        };
        match entry {
            Some(e) => entries.push(e),
            None => missing.push(src.id),
        }
    }
    o.extend((entries.len() as u32).to_le_bytes());
    for e in &entries {
        o.extend(e.source.0.to_le_bytes());
        o.push(match e.kind { MediaKind::OpusProxy => 0, MediaKind::Original => 1,
                              MediaKind::Wav => 2 });
        put_str(&mut o, &e.ext);
        put_blob(&mut o, &e.bytes);
    }
    Ok((o, missing))
}

/// Open the archive, or `None` for anything malformed or truncated —
/// never a partial session.
pub fn unpack(b: &[u8]) -> Option<Unpacked> {
    let mut r = R { b, i: 0 };
    if r.take(8)? != MAGIC { return None; }
    if r.u8()? != VERSION { return None; }
    let an = r.u16()? as usize;
    let mut artists = Vec::with_capacity(an.min(4096));
    for _ in 0..an { artists.push(r.str()?); }
    let pn = r.u16()? as usize;
    let mut search_paths = Vec::with_capacity(pn.min(4096));
    for _ in 0..pn { search_paths.push(r.str()?); }
    let level = match r.u8()? {
        0 => Level::Sketch, 1 => Level::Demo, 2 => Level::Standard, 3 => Level::Master,
        _ => return None,
    };
    let save = SaveFile::decode(&r.blob()?)?;
    let mn = r.u32()? as usize;
    let mut media = Vec::with_capacity(mn.min(65_536));
    for _ in 0..mn {
        let source = SourceId(r.u64()?);
        let kind = match r.u8()? {
            0 => MediaKind::OpusProxy, 1 => MediaKind::Original, 2 => MediaKind::Wav,
            _ => return None,
        };
        let ext = r.str()?;
        let bytes = r.blob()?;
        media.push(MediaEntry { source, kind, ext, bytes });
    }
    if r.i != b.len() { return None; } // trailing garbage is damage too
    Some(Unpacked { save, meta: ArchiveMeta { artists, search_paths, level }, media })
}

/// A higher-quality on-device artifact for `source_path`, searched under
/// `search_paths` (relative to the archive's own directory, or absolute).
/// Exact basename first, then the file's stem with LOSSLESS extensions
/// before lossy ones — finding a worse file than the embedded proxy would
/// be an upgrade in name only.
pub fn find_upgrade(archive_dir: &Path, search_paths: &[String], source_path: &str)
                    -> Option<PathBuf> {
    let src = Path::new(source_path);
    let basename = src.file_name()?.to_string_lossy().into_owned();
    let stem = src.file_stem()?.to_string_lossy().into_owned();
    const EXTS: [&str; 6] = ["wav", "flac", "aiff", "opus", "m4a", "mp3"];
    for sp in search_paths {
        let dir = if Path::new(sp).is_absolute() { PathBuf::from(sp) }
                  else { archive_dir.join(sp) };
        let exact = dir.join(&basename);
        if exact.is_file() { return Some(exact); }
        for ext in EXTS {
            let p = dir.join(format!("{stem}.{ext}"));
            if p.is_file() { return Some(p); }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Project, Source, Ticks, Track, TrackId, TrackKind};

    fn save_with_sources(n: u64) -> SaveFile {
        let mut p = Project { name: "cut".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0 }).unwrap();
        for id in 1..=n {
            p.apply(&Op::AddSource { source: Source { id: SourceId(id),
                path: format!("takes/take{id}.opus"), len: Ticks::from_seconds(1.0),
                sample_rate: 44_100, channels: 1, has_video: false } }).unwrap();
        }
        SaveFile::from_project(&p, Vec::new(), Some(studio_live::Rack::trap_kit()))
    }
    fn tone(rate: u32, secs: f32) -> SourceAudio {
        let n = (rate as f32 * secs) as usize;
        SourceAudio { sample_rate: rate, channels: vec![
            (0..n).map(|i| (i as f32 / rate as f32 * 220.0
                            * std::f32::consts::TAU).sin() * 0.5).collect()] }
    }
    fn meta(level: Level) -> ArchiveMeta {
        ArchiveMeta { artists: vec!["ada".into(), "sc:9f31".into()],
                      search_paths: vec!["hq".into()], level }
    }

    #[test]
    fn an_archive_round_trips_whole_and_a_missing_sample_is_said_at_pack_time() {
        let save = save_with_sources(2);
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), tone(44_100, 1.0)); // source 2 has NO audio
        let (bytes, missing) = pack(&save, &audio, &BTreeMap::new(),
                                    &meta(Level::Demo)).unwrap();
        assert_eq!(missing, vec![SourceId(2)],
                   "the hole is reported to the person saving, not found by a collaborator");
        let u = unpack(&bytes).expect("opens whole");
        assert_eq!(u.save, save, "ALL state: mixer, rack, patterns, rolls, automation");
        assert_eq!(u.meta, meta(Level::Demo));
        assert_eq!(u.media.len(), 1);
        assert_eq!(u.media[0].kind, MediaKind::OpusProxy);
        // The proxy is a real Opus this repo's own decoder reads at 48k —
        // an archive must PLAY everywhere, that is its whole point.
        let back = studio_media::decode_bytes(u.media[0].bytes.clone(), Some("opus"))
            .expect("the proxy decodes");
        assert_eq!(back.sample_rate, 48_000);
        let secs = back.frames() as f64 / 48_000.0;
        assert!((secs - 1.0).abs() < 0.05, "one second in, one second out: {secs}");
    }

    #[test]
    fn the_fidelity_slider_actually_moves_the_size() {
        let save = save_with_sources(1);
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), tone(48_000, 3.0));
        let m = |l| pack(&save, &audio, &BTreeMap::new(), &meta(l)).unwrap().0.len();
        let (sketch, standard) = (m(Level::Sketch), m(Level::Standard));
        assert!(sketch < standard,
                "a lower bitrate is a smaller archive: {sketch} vs {standard}");
    }

    #[test]
    fn master_embeds_the_original_bytes_untouched_and_pcm_falls_back_lossless() {
        let save = save_with_sources(2);
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(2), tone(48_000, 0.5)); // a bounce: PCM only
        let mut originals = BTreeMap::new();
        let fake_opus = vec![7u8; 999]; // the BYTES are what must survive
        originals.insert(SourceId(1), ("opus".to_string(), fake_opus.clone()));
        let (bytes, missing) = pack(&save, &audio, &originals,
                                    &meta(Level::Master)).unwrap();
        assert!(missing.is_empty());
        let u = unpack(&bytes).unwrap();
        let s1 = u.media.iter().find(|m| m.source == SourceId(1)).unwrap();
        assert_eq!(s1.kind, MediaKind::Original);
        assert_eq!(s1.bytes, fake_opus, "a .opus stays .opus, byte for byte");
        let s2 = u.media.iter().find(|m| m.source == SourceId(2)).unwrap();
        assert_eq!(s2.kind, MediaKind::Wav, "PCM the session recorded goes lossless");
        assert!(studio_media::decode_bytes(s2.bytes.clone(), Some("wav")).is_ok());
    }

    #[test]
    fn every_truncation_refuses_the_whole_archive() {
        // The collab file format's damaged-file discipline, applied here:
        // an archive that half-opens is how files get lost. Every prefix,
        // plus one byte of trailing garbage.
        let save = save_with_sources(1);
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), tone(48_000, 0.2));
        let (bytes, _) = pack(&save, &audio, &BTreeMap::new(), &meta(Level::Sketch)).unwrap();
        assert!(unpack(&bytes).is_some(), "the intact archive opens");
        for cut in 0..bytes.len() {
            assert!(unpack(&bytes[..cut]).is_none(), "prefix {cut} must refuse");
        }
        let mut long = bytes.clone();
        long.push(0);
        assert!(unpack(&long).is_none(), "trailing garbage is damage too");
    }

    #[test]
    fn the_upgrade_path_prefers_lossless_and_exact_names() {
        let dir = std::env::temp_dir().join(format!("studio-arch-up-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hq")).unwrap();
        // Only a lossy m4a on device: found, because better-than-proxy.
        std::fs::write(dir.join("hq/take1.m4a"), b"x").unwrap();
        let sp = vec!["hq".to_string()];
        assert_eq!(find_upgrade(&dir, &sp, "takes/take1.opus"),
                   Some(dir.join("hq/take1.m4a")));
        // A WAV appears: lossless outranks it.
        std::fs::write(dir.join("hq/take1.wav"), b"x").unwrap();
        assert_eq!(find_upgrade(&dir, &sp, "takes/take1.opus"),
                   Some(dir.join("hq/take1.wav")));
        // The exact basename outranks everything — same name IS the file.
        std::fs::write(dir.join("hq/take1.opus"), b"x").unwrap();
        assert_eq!(find_upgrade(&dir, &sp, "takes/take1.opus"),
                   Some(dir.join("hq/take1.opus")));
        assert_eq!(find_upgrade(&dir, &sp, "takes/other.opus"), None,
                   "nothing pretends to be an upgrade for a file that is not there");
    }
}
