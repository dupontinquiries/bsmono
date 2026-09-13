//! Save and load an arrangement.
//!
//! A project is a DIRECTORY: `project.json` (the timeline and a manifest
//! of the library) beside `beats/` and `files/` holding every source's
//! audio at the model sample rate, in the container the preference names
//! (`Config::audio_format`: wav, flac or opus). Files are copied in rather
//! than referenced, so a project moved to another machine still plays; the
//! original path is kept in the manifest for the reader's sake. A captured
//! beat's mel planes are written too (`.planes`: a small header then f32
//! little-endian), so it can be re-armed or re-rendered later.
//!
//! The manifest records each source's actual file name, so a project saved
//! as flac and re-saved as wav reads back either way; loading decodes
//! whatever the name says through `audio_io::decode_to_mono`. The Opus
//! rungs are lossy — a project saved that way and loaded again is not
//! bit-for-bit the project that was saved, and a captured beat's planes
//! are its lossless record. A build without the Opus encoder writes the
//! flac rung instead and the manifest says so.
//!
//! The manifest lists sources in library order, because a clip's
//! `SourceRef` is an index into that order; loading rebuilds the library
//! in the same order. `Timeline` serialises whole — including its id
//! counter, so ids after a load are still never reused.
//!
//! The serialisation is pure and tested on its own; `save`/`load` do the
//! I/O through `audio_io` and `std::fs`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::model::Timeline;
use super::render::{Library, Sample};
use audio_kit::BounceFormat;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleMeta {
    pub name: String,
    /// Relative to the project directory.
    pub audio: String,
    pub sr: u32,
    pub bpm: Option<f32>,
    /// Relative path of the planes file, for captured beats.
    pub planes: Option<String>,
    /// Where an imported file came from. Informational.
    pub origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub timeline: Timeline,
    pub beats: Vec<SampleMeta>,
    pub files: Vec<SampleMeta>,
}

impl ProjectFile {
    /// The manifest for `timeline` and `lib`, naming where each source's
    /// audio will be written in `format` — the format as it will actually
    /// be written on this build. Pure: no file is touched.
    pub fn describe(timeline: &Timeline, lib: &Library, format: BounceFormat) -> Self {
        let ext = crate::audio_io::effective_format(format).ext();
        let meta = |dir: &str, i: usize, s: &Sample| SampleMeta {
            name: s.name.clone(),
            audio: format!("{dir}/{dir}-{i:03}.{ext}"),
            sr: s.sr,
            bpm: s.bpm,
            planes: s.planes.as_ref().map(|_| format!("{dir}/{dir}-{i:03}.planes")),
            origin: None,
        };
        Self {
            version: FORMAT_VERSION,
            timeline: timeline.clone(),
            beats: lib.beats.iter().enumerate().map(|(i, s)| meta("beats", i, s)).collect(),
            files: lib.files.iter().enumerate().map(|(i, s)| meta("files", i, s)).collect(),
        }
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let p: Self = serde_json::from_str(s).context("project.json")?;
        if p.version > FORMAT_VERSION {
            return Err(anyhow!("project format {} is newer than this build's {}", p.version, FORMAT_VERSION));
        }
        Ok(p)
    }

    /// Every `SourceRef` in the timeline must index the manifest.
    pub fn check_refs(&self) -> Result<()> {
        use super::model::SourceRef;
        for t in &self.timeline.tracks {
            for c in &t.clips {
                let ok = match c.source {
                    SourceRef::Beat(i) => i < self.beats.len(),
                    SourceRef::File(i) => i < self.files.len(),
                };
                if !ok {
                    return Err(anyhow!("clip {} on {:?} points at {:?}, which the manifest does not have", c.id, t.name, c.source));
                }
            }
        }
        Ok(())
    }
}

/// Write the project into `dir` (created if needed), its audio at the
/// device's rung (or the flac rung, on a build without Opus). Existing
/// audio files with the same names are overwritten; the directory is not
/// cleared, so a project that lost sources — or changed format — leaves
/// the old files behind rather than deleting anything. Returns the format
/// actually written.
pub fn save(dir: &Path, timeline: &Timeline, lib: &Library, format: BounceFormat) -> Result<BounceFormat> {
    let format = crate::audio_io::effective_format(format);
    let pf = ProjectFile::describe(timeline, lib, format);
    pf.check_refs()?;
    std::fs::create_dir_all(dir.join("beats"))?;
    std::fs::create_dir_all(dir.join("files"))?;
    for (meta, s) in pf.beats.iter().zip(&lib.beats).chain(pf.files.iter().zip(&lib.files)) {
        let written = crate::audio_io::save_audio(&dir.join(&meta.audio), &s.audio, s.sr, format)
            .with_context(|| format!("writing {}", meta.audio))?;
        debug_assert_eq!(written, dir.join(&meta.audio));
        if let (Some(rel), Some(planes)) = (&meta.planes, &s.planes) {
            write_planes(&dir.join(rel), planes)?;
        }
    }
    let tmp = dir.join("project.json.tmp");
    std::fs::write(&tmp, pf.to_json()?)?;
    std::fs::rename(&tmp, dir.join("project.json"))?;
    Ok(format)
}

/// Read a project. Every source is decoded to `sr` (a project saved at
/// another rate is resampled, which is lossy but plays).
pub fn load(dir: &Path, sr: u32) -> Result<(Timeline, Library)> {
    let raw = std::fs::read_to_string(dir.join("project.json"))
        .with_context(|| format!("no project.json in {}", dir.display()))?;
    let pf = ProjectFile::from_json(&raw)?;
    pf.check_refs()?;
    let read = |meta: &SampleMeta| -> Result<Sample> {
        let audio = crate::audio_io::decode_to_mono(&dir.join(&meta.audio), sr)
            .with_context(|| format!("reading {}", meta.audio))?;
        let planes = match &meta.planes {
            Some(rel) => read_planes(&dir.join(rel)).ok(),
            None => None,
        };
        Ok(Sample { name: meta.name.clone(), audio, sr, planes, bpm: meta.bpm })
    };
    let mut lib = Library::default();
    for m in &pf.beats {
        lib.beats.push(read(m)?);
    }
    for m in &pf.files {
        lib.files.push(read(m)?);
    }
    Ok((pf.timeline, lib))
}

const PLANES_MAGIC: &[u8; 4] = b"BSPL";

fn write_planes(path: &Path, planes: &[f32]) -> Result<()> {
    let mut bytes = Vec::with_capacity(8 + planes.len() * 4);
    bytes.extend_from_slice(PLANES_MAGIC);
    bytes.extend_from_slice(&(planes.len() as u32).to_le_bytes());
    for p in planes {
        bytes.extend_from_slice(&p.to_le_bytes());
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

fn read_planes(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 8 || &bytes[..4] != PLANES_MAGIC {
        return Err(anyhow!("{} is not a planes file", path.display()));
    }
    let n = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if bytes.len() < 8 + n * 4 {
        return Err(anyhow!("{} is truncated", path.display()));
    }
    Ok(bytes[8..8 + n * 4].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// A default project directory beside the checkpoints.
pub fn default_dir(ckpt_dir: &Path) -> PathBuf {
    ckpt_dir.join("arrangement")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::cmd::{Command, Editor};
    use super::super::model::SourceRef;

    fn fixture() -> (Timeline, Library) {
        let mut lib = Library::default();
        lib.add_beat(Sample { name: "kick".into(), audio: (0..400).map(|i| (i as f32 / 400.0).sin()).collect(), sr: 8000, planes: Some(vec![0.1, -0.2, 0.3]), bpm: Some(120.0) });
        lib.add_file(Sample { name: "pad.wav".into(), audio: vec![0.25; 1600], sr: 8000, planes: None, bpm: None });
        let mut e = Editor::default();
        let (t0, t1) = (e.timeline.tracks[0].id, e.timeline.tracks[1].id);
        let a = e.apply(Command::Place { track: t0, source: SourceRef::Beat(0), start: 2.0, source_beats: 0.1, exact: false, lock: None }).unwrap().created[0];
        e.apply(Command::SetTempoLock { clip: a, bpm: Some(120.0) }).unwrap();
        e.apply(Command::SetClipFades { clip: a, fade_in: 0.02, fade_out: 0.0 }).unwrap();
        e.apply(Command::Place { track: t1, source: SourceRef::File(0), start: 0.0, source_beats: 0.4, exact: false, lock: None }).unwrap();
        e.apply(Command::SetLoop { region: Some((0.0, 4.0)) }).unwrap();
        e.apply(Command::SetTempo { bpm: 96.0 }).unwrap();
        (e.timeline, lib)
    }

    #[test]
    fn the_manifest_round_trips_through_json_with_every_field() {
        let (t, lib) = fixture();
        let pf = ProjectFile::describe(&t, &lib, BounceFormat::Wav);
        assert_eq!(pf.beats[0].audio, "beats/beats-000.wav");
        assert_eq!(ProjectFile::describe(&t, &lib, BounceFormat::Flac).beats[0].audio, "beats/beats-000.flac");
        assert_eq!(pf.beats[0].planes.as_deref(), Some("beats/beats-000.planes"));
        assert_eq!(pf.files[0].planes, None);
        let json = pf.to_json().unwrap();
        let back = ProjectFile::from_json(&json).unwrap();
        assert_eq!(back, pf);
        assert_eq!(back.timeline, t, "tempo lock, fades, loop, tempo and the id counter all survive");
        back.check_refs().unwrap();
    }

    #[test]
    fn a_newer_format_or_a_dangling_source_is_refused() {
        let (t, lib) = fixture();
        let mut pf = ProjectFile::describe(&t, &lib, BounceFormat::Wav);
        pf.version = FORMAT_VERSION + 1;
        assert!(ProjectFile::from_json(&pf.to_json().unwrap()).is_err());
        pf.version = FORMAT_VERSION;
        pf.files.clear();
        let err = pf.check_refs().unwrap_err().to_string();
        assert!(err.contains("File(0)"), "{err}");
    }

    #[test]
    fn save_then_load_gives_back_the_same_arrangement_and_audio() {
        let (t, lib) = fixture();
        let dir = std::env::temp_dir().join(format!("bs-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, &t, &lib, BounceFormat::Wav).unwrap();
        assert!(dir.join("project.json").exists());
        assert!(dir.join("beats/beats-000.wav").exists());
        assert!(dir.join("beats/beats-000.planes").exists());
        assert!(dir.join("files/files-000.wav").exists());
        let (t2, lib2) = load(&dir, 8000).unwrap();
        assert_eq!(t2, t);
        assert_eq!(lib2.beats.len(), 1);
        assert_eq!(lib2.files.len(), 1);
        assert_eq!(lib2.beats[0].name, "kick");
        assert_eq!(lib2.beats[0].bpm, Some(120.0));
        assert_eq!(lib2.beats[0].planes.as_deref(), Some(&[0.1f32, -0.2, 0.3][..]));
        assert_eq!(lib2.beats[0].audio.len(), 400);
        // 16-bit WAV: within a quantisation step.
        let err = lib2.beats[0].audio.iter().zip(&lib.beats[0].audio).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-3, "audio changed by {err}");
        assert!((lib2.files[0].audio[10] - 0.25).abs() < 1e-3);
        // Ids after a load are still fresh.
        let mut e = Editor::new(t2);
        e.apply(Command::AddTrack { name: "x".into() }).unwrap();
        let new_track = e.timeline.tracks.last().unwrap().id;
        assert!(t.tracks.iter().all(|tr| tr.id != new_track));
        assert!(t.tracks.iter().flat_map(|tr| tr.clips.iter()).all(|c| c.id != new_track));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_saved_as_flac_loads_back_the_same_and_is_smaller() {
        let (t, lib) = fixture();
        let dir = std::env::temp_dir().join(format!("bs-project-flac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, &t, &lib, BounceFormat::Flac).unwrap();
        assert!(dir.join("beats/beats-000.flac").exists());
        assert!(!dir.join("beats/beats-000.wav").exists());
        let manifest = std::fs::read_to_string(dir.join("project.json")).unwrap();
        assert!(manifest.contains("beats/beats-000.flac"));
        let (t2, lib2) = load(&dir, 8000).unwrap();
        assert_eq!(t2, t);
        let err = lib2.beats[0].audio.iter().zip(&lib.beats[0].audio).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-3, "flac changed the audio by {err}");
        assert_eq!(lib2.files[0].audio.len(), 1600);
        // The constant 0.25 file: FLAC codes it in a handful of bytes.
        let flac_len = std::fs::metadata(dir.join("files/files-000.flac")).unwrap().len();
        assert!(flac_len < 1600 * 2 / 4, "flac {flac_len} bytes vs 3200 as wav");
        // Re-saving as wav beside it: the manifest follows, the old flac
        // is left alone.
        save(&dir, &t2, &lib2, BounceFormat::Wav).unwrap();
        assert!(dir.join("beats/beats-000.wav").exists() && dir.join("beats/beats-000.flac").exists());
        let (t3, _) = load(&dir, 8000).unwrap();
        assert_eq!(t3, t);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_opus_rung_on_a_build_without_the_encoder_writes_the_flac_rung_and_says_so() {
        let (t, lib) = fixture();
        let dir = std::env::temp_dir().join(format!("bs-project-opus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = save(&dir, &t, &lib, BounceFormat::Opus192).unwrap();
        let manifest = std::fs::read_to_string(dir.join("project.json")).unwrap();
        if cfg!(feature = "opus-encode") {
            assert_eq!(written, BounceFormat::Opus192);
            assert!(manifest.contains("beats/beats-000.opus"));
        } else {
            assert_eq!(written, BounceFormat::Flac, "bigger, never a hole");
            assert!(manifest.contains("beats/beats-000.flac"));
            assert!(dir.join("beats/beats-000.flac").exists());
        }
        let (t2, _) = load(&dir, 8000).unwrap();
        assert_eq!(t2, t);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_a_missing_project_names_the_directory() {
        let err = load(Path::new("/nonexistent/beatspace-project"), 8000).unwrap_err().to_string();
        assert!(err.contains("/nonexistent/beatspace-project"), "{err}");
    }
}
