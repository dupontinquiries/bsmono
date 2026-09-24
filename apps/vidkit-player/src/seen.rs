//! What you have watched, and what was said in it.
//!
//! One pair of files per video in `$XDG_DATA_HOME/vidkit/seen/` (persistent,
//! not /tmp: a transcript is tens of KB and should outlive the media):
//!
//! ```text
//! <key>.meta   one line: url \t title \t last-watched-unix \t transcript-file
//! <key>.tsv    the transcript, in vidkit-transcribe's cache format
//! ```
//!
//! The `.tsv` IS the transcription loop's cache, so reopening a URL -- after
//! the download was evicted from /tmp, or streamed again -- resumes the
//! transcript instead of redoing it. The palette searches all of them to
//! answer "which video did I hear that in, and where".

use std::path::{Path, PathBuf};
use std::time::Duration;

use vidkit_relevance::Window;
use vidkit_transcribe::transcript::Transcript;

pub fn dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("vidkit/seen"))
}

/// What identifies a video for the seen store: the page URL, or for a local
/// file `file://<canonical path>`.
pub fn url_for_file(p: &Path) -> String {
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    format!("file://{}", abs.display())
}

/// The transcript file name for a video. A local file's includes its size
/// and mtime, so an edited file with the same name is transcribed again; a
/// URL's is the URL alone, so a re-download or a stream shares it.
pub fn transcript_name(url: &str, file: Option<&Path>) -> String {
    let mut key = url.trim().to_string();
    if let Some(m) = file.and_then(|f| std::fs::metadata(f).ok()) {
        let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        key.push_str(&format!("\0{}\0{mtime}", m.len()));
    }
    format!("{:016x}.tsv", crate::transcribe::fnv1a(key.as_bytes()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub url: String,
    pub title: String,
    pub watched: u64,
    pub transcript: String,
}

impl Meta {
    pub fn to_line(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        format!("{}\t{}\t{}\t{}", clean(&self.url), clean(&self.title), self.watched, clean(&self.transcript))
    }
    pub fn from_line(l: &str) -> Option<Self> {
        let mut p = l.trim_end().splitn(4, '\t');
        Some(Meta { url: p.next()?.to_string(), title: p.next()?.to_string(), watched: p.next()?.parse().ok()?, transcript: p.next()?.to_string() })
    }
}

fn meta_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{:016x}.meta", crate::transcribe::fnv1a(url.trim().as_bytes())))
}

/// Note that `url` is being watched; returns the transcript cache path.
/// Keeps an earlier title when this open does not know one.
pub fn record(dir: &Path, url: &str, title: Option<&str>, file: Option<&Path>) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let mp = meta_path(dir, url);
    let old = std::fs::read_to_string(&mp).ok().and_then(|t| Meta::from_line(&t));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let title = title
        .map(str::to_string)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| old.as_ref().map(|m| m.title.clone()))
        .unwrap_or_else(|| default_title(url));
    let m = Meta { url: url.to_string(), title, watched: now, transcript: transcript_name(url, file) };
    let tmp = mp.with_extension("meta.tmp");
    std::fs::write(&tmp, m.to_line()).ok()?;
    std::fs::rename(&tmp, &mp).ok()?;
    Some(dir.join(&m.transcript))
}

/// A readable stand-in when there is no title: the file name, or the URL
/// without its scheme.
pub fn default_title(url: &str) -> String {
    if let Some(p) = url.strip_prefix("file://") {
        return Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string());
    }
    url.split_once("://").map(|(_, r)| r).unwrap_or(url).chars().take(60).collect()
}

/// One moment in one watched video.
#[derive(Clone, Debug, PartialEq)]
pub struct SeenHit {
    pub url: String,
    pub title: String,
    pub at: Duration,
    pub snippet: String,
    pub score: f32,
}

/// Every watched video's minute windows, loaded once per palette opening.
#[derive(Default)]
pub struct Index {
    videos: Vec<(Meta, Vec<Window>)>,
}

impl Index {
    /// Every watched title's meta, without loading transcripts.
    pub fn metas(dir: &Path) -> Vec<Meta> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        rd.flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("meta"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok().and_then(|t| Meta::from_line(&t)))
            .collect()
    }

    pub fn load(dir: &Path) -> Self {
        let mut videos = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else { return Self::default() };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("meta") {
                continue;
            }
            let Some(m) = std::fs::read_to_string(&p).ok().and_then(|t| Meta::from_line(&t)) else { continue };
            let Some(t) = std::fs::read_to_string(dir.join(&m.transcript)).ok().and_then(|t| Transcript::from_cache(&t).ok()) else { continue };
            let w = t.transcript.windows(Duration::from_secs(60));
            if !w.is_empty() {
                videos.push((m, w));
            }
        }
        // Most recently watched first: ties in score go to what you saw last.
        videos.sort_by(|a, b| b.0.watched.cmp(&a.0.watched));
        Self { videos }
    }

    pub fn len(&self) -> usize {
        self.videos.len()
    }

    /// The best moments across every video, best first; at most `per_video`
    /// from one, so one long lecture that says the word forty times does not
    /// bury the other video you were actually thinking of.
    pub fn search(&self, query: &str, limit: usize, per_video: usize) -> Vec<SeenHit> {
        if query.trim().chars().count() < 2 {
            return Vec::new();
        }
        let mut out: Vec<SeenHit> = Vec::new();
        for (m, w) in &self.videos {
            for h in vidkit_relevance::sections::search(w, query, per_video) {
                out.push(SeenHit { url: m.url.clone(), title: m.title.clone(), at: h.at, snippet: h.text.chars().take(90).collect(), score: h.score });
            }
        }
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(limit);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidkit_transcribe::Cue;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vk-seen-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn write_transcript(path: &Path, cues: &[(u64, &str)]) {
        let mut t = Transcript::new("test");
        t.replace_all(cues.iter().map(|&(s, x)| Cue { start: Duration::from_secs(s), end: Duration::from_secs(s + 5), text: x.into() }).collect());
        std::fs::write(path, t.to_cache(Duration::from_secs(30), [0u32].into_iter())).unwrap();
    }

    #[test]
    fn meta_round_trips_and_survives_tabs_in_titles() {
        let m = Meta { url: "https://y/w?v=1".into(), title: "A\tB".into(), watched: 9, transcript: "x.tsv".into() };
        let back = Meta::from_line(&m.to_line()).unwrap();
        assert_eq!(back.title, "A B");
        assert_eq!(back.url, m.url);
    }

    /// The point of it: a phrase heard in one of several videos is found, in
    /// the right video, at the right minute, from a fresh index.
    #[test]
    fn a_phrase_is_traced_back_to_its_video_and_minute() {
        let d = scratch("find");
        let a = record(&d, "https://youtube.com/watch?v=aaa", Some("Signals 101"), None).unwrap();
        write_transcript(&a, &[(10, "welcome to signals"), (130, "the fourier transform maps time to frequency"), (200, "homework")]);
        let b = record(&d, "https://youtube.com/watch?v=bbb", Some("Cooking with Ada"), None).unwrap();
        write_transcript(&b, &[(5, "today we make bread"), (70, "let the dough rise overnight")]);
        let idx = Index::load(&d);
        assert_eq!(idx.len(), 2);
        let h = idx.search("fourier transform", 5, 2);
        assert_eq!(h.first().map(|h| (h.title.as_str(), h.at.as_secs())), Some(("Signals 101", 120)), "{h:?}");
        let h = idx.search("dough", 5, 2);
        assert_eq!(h.first().map(|h| h.url.as_str()), Some("https://youtube.com/watch?v=bbb"));
        assert!(idx.search("quantum", 5, 2).is_empty());
        assert!(idx.search("a", 5, 2).is_empty(), "one letter matches everything; not a query");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Reopening a URL keeps its title and points at the SAME transcript, so
    /// the transcription loop resumes instead of starting over.
    #[test]
    fn reopening_a_url_reuses_its_transcript_and_title() {
        let d = scratch("reopen");
        let p1 = record(&d, "https://v/1", Some("Lecture 1"), None).unwrap();
        let p2 = record(&d, "https://v/1", None, None).unwrap();
        assert_eq!(p1, p2);
        let m = Meta::from_line(&std::fs::read_to_string(meta_path(&d, "https://v/1")).unwrap()).unwrap();
        assert_eq!(m.title, "Lecture 1", "a later open without a title keeps the old one");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_edited_local_file_gets_a_new_transcript() {
        let d = scratch("edit");
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("clip.mkv");
        std::fs::write(&f, b"one").unwrap();
        let url = url_for_file(&f);
        let t1 = transcript_name(&url, Some(&f));
        std::fs::write(&f, b"longer now").unwrap();
        assert_ne!(t1, transcript_name(&url, Some(&f)));
        assert_eq!(transcript_name("https://x", None), transcript_name("https://x", None));
        assert_eq!(default_title(&url), "clip.mkv");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn one_video_cannot_crowd_out_the_rest() {
        let d = scratch("crowd");
        let a = record(&d, "https://v/long", Some("Long"), None).unwrap();
        let cues: Vec<(u64, &str)> = (0..30).map(|i| (i * 60, "entropy entropy entropy")).collect();
        write_transcript(&a, &cues);
        let b = record(&d, "https://v/short", Some("Short"), None).unwrap();
        write_transcript(&b, &[(0, "a word on entropy")]);
        let h = Index::load(&d).search("entropy", 5, 2);
        assert!(h.iter().any(|h| h.title == "Short"), "{h:?}");
        assert!(h.iter().filter(|h| h.title == "Long").count() <= 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
