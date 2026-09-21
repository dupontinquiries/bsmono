//! transcripts — putting what was said under the clips that say it.
//!
//! Whisper leaves a `.srt` per media file (the film's card has one per camera
//! file). This matches them to sources by file stem — the same convention
//! relinking uses, for the same reason: the transcript is named after the
//! media — and parses them with `studio_voice::parse_srt`. A source with no
//! `.srt` gets none rather than a guess; a `.txt` with no timings is used
//! only as one segment spanning the whole source, because an untimed line
//! drawn at the right clip is still better than nothing, and it is marked as
//! low confidence so nothing downstream treats its placement as a fact.

use std::collections::BTreeMap;
use std::path::Path;

use studio_core::{Project, SourceId};
use studio_voice::Segment;

/// What loading found.
#[derive(Debug, Default, PartialEq)]
pub struct Loaded {
    pub words: BTreeMap<SourceId, Vec<Segment>>,
    /// Sources with timed `.srt` transcripts.
    pub timed: usize,
    /// Sources that only had an untimed `.txt`.
    pub untimed: usize,
    /// Sources with no transcript in the folder.
    pub missing: usize,
}

impl Loaded {
    pub fn summary(&self) -> String {
        let mut s = format!("transcripts: {} timed", self.timed);
        if self.untimed > 0 { s.push_str(&format!(", {} untimed", self.untimed)); }
        if self.missing > 0 { s.push_str(&format!(", {} without one", self.missing)); }
        s
    }
}

fn stem(p: &Path) -> Option<String> { p.file_stem().map(|s| s.to_string_lossy().to_lowercase()) }

/// Match every file source in `project` to a transcript in `dir`.
pub fn load(project: &Project, dir: &Path) -> Loaded {
    let mut srt: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    let mut txt: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let (Some(s), Some(ext)) = (stem(&p), p.extension().map(|x| x.to_string_lossy().to_lowercase())) else { continue };
            match ext.as_str() { "srt" => { srt.insert(s, p); } "txt" => { txt.insert(s, p); } _ => {} }
        }
    }
    let mut out = Loaded::default();
    for (id, src) in &project.sources {
        if src.kind != studio_core::SourceKind::File { continue; }
        let Some(s) = stem(Path::new(&src.path)) else { continue };
        if let Some(p) = srt.get(&s) {
            let segs = studio_voice::parse_srt(&std::fs::read_to_string(p).unwrap_or_default());
            if !segs.is_empty() { out.words.insert(*id, segs); out.timed += 1; continue; }
        }
        if let Some(p) = txt.get(&s) {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !text.is_empty() {
                out.words.insert(*id, vec![Segment { start: studio_core::Ticks::ZERO, end: src.len,
                    text, speaker: None, confidence: 0.3 }]);
                out.untimed += 1;
                continue;
            }
        }
        out.missing += 1;
    }
    out
}

/// Transcripts that sit NEXT TO their media (`C0057.mkv` beside
/// `C0057.srt`) — found without asking, because that is where whisper
/// writes them by default and a person who put them there should not have
/// to point at the folder they are already in.
pub fn beside_media(project: &Project) -> Loaded {
    let dirs: std::collections::BTreeSet<std::path::PathBuf> = project.sources.values()
        .filter(|s| s.kind == studio_core::SourceKind::File)
        .filter_map(|s| Path::new(&s.path).parent().map(|p| p.to_path_buf()))
        .collect();
    let mut all = Loaded::default();
    for d in dirs {
        let l = load(project, &d);
        all.timed += l.timed;
        all.untimed += l.untimed;
        for (k, v) in l.words { all.words.entry(k).or_insert(v); }
    }
    // every source is judged once per folder above; count the ones still bare
    all.missing = project.sources.values()
        .filter(|s| s.kind == studio_core::SourceKind::File && !all.words.contains_key(&s.id)).count();
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Op, Source, Ticks};

    struct Dir(std::path::PathBuf);
    impl Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn dir(files: &[(&str, &str)]) -> Dir {
        let p = std::env::temp_dir().join(format!("tx-{}-{:?}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&p).unwrap();
        for (n, body) in files { std::fs::write(p.join(n), body).unwrap(); }
        Dir(p)
    }

    #[test]
    fn transcripts_beside_the_media_are_found_without_asking() {
        let d = dir(&[("C0057.mkv", "x"), ("C0057.srt", "1\n00:00:00,000 --> 00:00:02,000\nhello\n")]);
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1),
            d.0.join("C0057.mkv").display().to_string(), Ticks::from_seconds(5.0), 48_000, 2, true) }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(2), "/elsewhere/C0099.mkv",
            Ticks::from_seconds(5.0), 48_000, 2, true) }).unwrap();
        let l = beside_media(&p);
        assert_eq!(l.words[&SourceId(1)][0].text, "hello");
        assert_eq!((l.timed, l.missing), (1, 1), "{}", l.summary());
    }

    #[test]
    fn transcripts_find_their_media_by_name_and_say_what_is_missing() {
        let mut p = Project::default();
        for (i, path) in ["/proxies/C0057.mkv", "/proxies/C0058.mkv", "/proxies/C0059.mkv"].iter().enumerate() {
            p.apply(&Op::AddSource { source: Source::file(SourceId(i as u64 + 1), *path,
                Ticks::from_seconds(20.0), 48_000, 2, true) }).unwrap();
        }
        let d = dir(&[
            ("C0057.srt", "1\n00:00:01,000 --> 00:00:03,000\nGood morning, I'm Gloria.\n"),
            ("c0058.txt", "Oh look who is here.\nI'm Sister Frances."),
        ]);
        let l = load(&p, &d.0);
        assert_eq!((l.timed, l.untimed, l.missing), (1, 1, 1), "{}", l.summary());
        assert_eq!(l.words[&SourceId(1)][0].start, Ticks::from_seconds(1.0));
        let untimed = &l.words[&SourceId(2)][0];
        assert_eq!(untimed.text, "Oh look who is here. I'm Sister Frances.");
        assert!(untimed.confidence < 0.5, "an untimed placement is not presented as a fact");
        assert_eq!(untimed.end, Ticks::from_seconds(20.0), "it spans its whole source");
        assert!(!l.words.contains_key(&SourceId(3)));
        assert_eq!(l.summary(), "transcripts: 1 timed, 1 untimed, 1 without one");
    }
}
