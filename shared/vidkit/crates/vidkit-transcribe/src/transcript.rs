//! The accumulated transcript, and its cache file.

use std::time::Duration;

use vidkit_relevance::{Cue, Window};

use crate::coverage::Chunk;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// Sorted by start. Invariant kept by `accept`.
    cues: Vec<Cue>,
    /// Which source produced it, for the cache header and the UI.
    pub source: String,
}

impl Transcript {
    pub fn new(source: impl Into<String>) -> Self {
        Self { cues: Vec::new(), source: source.into() }
    }

    pub fn cues(&self) -> &[Cue] {
        &self.cues
    }

    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// Merge one chunk's output.
    ///
    /// Ownership is by START time: a cue belongs to the chunk its start falls
    /// in. The chunk was decoded with post-roll, so its output includes the
    /// first sentence of the NEXT chunk too; that one is dropped here and
    /// produced again, whole, by its owner. Cues this chunk already owned are
    /// replaced, so redoing a chunk (a better model, a retry) is idempotent.
    ///
    /// A cue that starts inside the chunk but runs past the owned end keeps
    /// its real end — that is the sentence the post-roll exists to finish.
    pub fn accept(&mut self, chunk: &Chunk, cues: Vec<Cue>) {
        let owns = |c: &Cue| c.start >= chunk.start && c.start < chunk.end;
        self.cues.retain(|c| !owns(c));
        let fresh: Vec<Cue> = cues.into_iter().filter(|c| owns(c) && c.end >= c.start).collect();
        let at = self.cues.partition_point(|c| c.start < chunk.start);
        // Sorted within the chunk too: a source is not obliged to emit in
        // order, and binary search downstream assumes it.
        let mut fresh = fresh;
        fresh.sort_by_key(|c| c.start);
        self.cues.splice(at..at, fresh);
    }

    /// Replace everything, for a source that answers in one go (subtitles).
    pub fn replace_all(&mut self, mut cues: Vec<Cue>) {
        cues.sort_by_key(|c| c.start);
        self.cues = cues;
    }

    /// The cue being spoken at `t`, for a subtitle line. When cues overlap,
    /// the latest-starting one wins: it is the newer sentence.
    pub fn at(&self, t: Duration) -> Option<&Cue> {
        let upto = self.cues.partition_point(|c| c.start <= t);
        self.cues[..upto].iter().rev().take(4).find(|c| t < c.end)
    }

    /// Minute windows for vidkit-relevance's chapters and search.
    pub fn windows(&self, window: Duration) -> Vec<Window> {
        vidkit_relevance::windows(&self.cues, window)
    }

    // ---- cache ---------------------------------------------------------

    /// Serialise with the done-chunk list, so a reopened lecture resumes
    /// rather than re-transcribing. Line-oriented and tab-separated: greppable,
    /// diffable, and no serde in a crate that otherwise needs none.
    pub fn to_cache(&self, chunk_len: Duration, done: impl Iterator<Item = u32>) -> String {
        let done: Vec<String> = done.map(|i| i.to_string()).collect();
        let mut s = format!(
            "#vidkit-transcript\tv1\tchunk_ms={}\tsource={}\n#done\t{}\n",
            chunk_len.as_millis(),
            one_line(&self.source),
            done.join(",")
        );
        for c in &self.cues {
            s.push_str(&format!(
                "{}\t{}\t{}\n",
                c.start.as_millis(),
                c.end.as_millis(),
                one_line(&c.text)
            ));
        }
        s
    }

    /// Parse a cache. Returns the transcript, the chunk length it was made
    /// with, and the done chunks. A cache made with a different chunk length
    /// is still valid transcript but its done list is in other units, so the
    /// caller must discard the list (see [`Cached::done_for`]).
    pub fn from_cache(text: &str) -> Result<Cached, String> {
        let mut lines = text.lines();
        let head = lines.next().ok_or("empty cache")?;
        let mut fields = head.split('\t');
        if fields.next() != Some("#vidkit-transcript") || fields.next() != Some("v1") {
            return Err(format!("not a v1 transcript cache: {head:?}"));
        }
        let mut chunk_ms = None;
        let mut source = String::new();
        for f in fields {
            if let Some(v) = f.strip_prefix("chunk_ms=") {
                chunk_ms = v.parse::<u64>().ok();
            } else if let Some(v) = f.strip_prefix("source=") {
                source = v.to_string();
            }
        }
        let chunk_len = Duration::from_millis(chunk_ms.ok_or("cache header has no chunk_ms")?);
        let done_line = lines.next().ok_or("cache has no #done line")?;
        let done_list = done_line.strip_prefix("#done\t").or_else(|| done_line.strip_prefix("#done")).ok_or("second line is not #done")?;
        let done: Vec<u32> = done_list
            .split(',')
            .filter(|x| !x.trim().is_empty())
            .map(|x| x.trim().parse().map_err(|e| format!("done index {x:?}: {e}")))
            .collect::<Result<_, _>>()?;
        let mut cues = Vec::new();
        for (n, line) in lines.enumerate() {
            if line.is_empty() {
                continue;
            }
            let mut p = line.splitn(3, '\t');
            let (a, b, t) = (p.next(), p.next(), p.next());
            let (Some(a), Some(b), Some(t)) = (a, b, t) else {
                return Err(format!("cue line {}: expected start\\tend\\ttext", n + 3));
            };
            let ms = |x: &str| x.parse::<u64>().map(Duration::from_millis).map_err(|e| format!("cue line {}: {e}", n + 3));
            cues.push(Cue { start: ms(a)?, end: ms(b)?, text: t.to_string() });
        }
        let mut transcript = Transcript::new(source);
        transcript.replace_all(cues);
        Ok(Cached { transcript, chunk_len, done })
    }
}

/// A parsed cache file.
pub struct Cached {
    pub transcript: Transcript,
    pub chunk_len: Duration,
    done: Vec<u32>,
}

impl Cached {
    /// The done list, but only if it was recorded in chunks of `chunk_len`.
    /// Mismatched units would mark the wrong stretches transcribed.
    pub fn done_for(&self, chunk_len: Duration) -> Vec<u32> {
        if self.chunk_len == chunk_len {
            self.done.clone()
        } else {
            Vec::new()
        }
    }
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::Coverage;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    fn cue(a: u64, b: u64, t: &str) -> Cue {
        Cue { start: ms(a), end: ms(b), text: t.into() }
    }

    fn cov() -> Coverage {
        Coverage::with_chunk(Some(ms(90_000)), ms(30_000), ms(2_000))
    }

    /// The overlap is decoded twice, by two chunks. The sentence in it must
    /// appear once, from the chunk its start belongs to — and in full, since
    /// the post-roll let the earlier chunk hear its end.
    #[test]
    fn a_sentence_across_a_boundary_appears_once_and_whole() {
        let c = cov();
        let mut t = Transcript::new("test");
        // Chunk 0 heard 0..32 s: a sentence at 29 s running to 31.5 s, and the
        // first words of chunk 1's first sentence at 31.8 s.
        t.accept(&c.chunk(0), vec![cue(1_000, 5_000, "intro"), cue(29_000, 31_500, "straddles"), cue(31_800, 32_000, "cut")]);
        // Chunk 1 heard 30..62 s and owns everything starting from 30 s,
        // including the sentence chunk 0 only caught the start of.
        //
        // Known limit, not covered here: if whisper, starting cold at 30 s,
        // emits a fragment of the straddler's tail stamped >= 30 s, chunk 1
        // owns that fragment and it appears once more. Ownership by start
        // cannot tell a tail from a new sentence.
        t.accept(&c.chunk(1), vec![cue(31_800, 34_000, "cut whole"), cue(40_000, 45_000, "next")]);
        let texts: Vec<&str> = t.cues().iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["intro", "straddles", "cut whole", "next"]);
        assert_eq!(t.cues()[1].end, ms(31_500), "the straddler keeps its real end");
    }

    #[test]
    fn redoing_a_chunk_replaces_rather_than_duplicates() {
        let c = cov();
        let mut t = Transcript::new("test");
        t.accept(&c.chunk(1), vec![cue(35_000, 36_000, "first try")]);
        t.accept(&c.chunk(0), vec![cue(1_000, 2_000, "zero")]);
        t.accept(&c.chunk(1), vec![cue(35_500, 37_000, "second try")]);
        let texts: Vec<&str> = t.cues().iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["zero", "second try"]);
    }

    /// Chunks arrive out of order (playhead first); the transcript must stay
    /// sorted or `at` and the windows are wrong.
    #[test]
    fn out_of_order_chunks_keep_the_transcript_sorted() {
        let c = cov();
        let mut t = Transcript::new("test");
        t.accept(&c.chunk(2), vec![cue(70_000, 71_000, "c"), cue(61_000, 62_000, "b2")]);
        t.accept(&c.chunk(0), vec![cue(2_000, 3_000, "a")]);
        t.accept(&c.chunk(1), vec![cue(45_000, 46_000, "b")]);
        let starts: Vec<u64> = t.cues().iter().map(|c| c.start.as_millis() as u64).collect();
        assert!(!starts.is_empty());
        assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
        assert_eq!(starts, vec![2_000, 45_000, 61_000, 70_000]);
    }

    #[test]
    fn at_finds_the_spoken_cue_and_nothing_in_a_gap() {
        let mut t = Transcript::new("x");
        t.replace_all(vec![cue(0, 2_000, "one"), cue(1_500, 4_000, "two"), cue(6_000, 7_000, "three")]);
        assert_eq!(t.at(ms(500)).map(|c| c.text.as_str()), Some("one"));
        assert_eq!(t.at(ms(1_800)).map(|c| c.text.as_str()), Some("two"), "latest start wins in an overlap");
        assert_eq!(t.at(ms(5_000)), None, "silence has no subtitle");
        assert_eq!(t.at(ms(7_000)), None, "end is exclusive");
    }

    #[test]
    fn the_cache_round_trips_and_survives_tabs_in_text() {
        let mut t = Transcript::new("whisper:ggml-base.en.bin");
        t.replace_all(vec![cue(0, 1_234, "hello\tthere"), cue(5_000, 6_000, "line\nbreak")]);
        let s = t.to_cache(ms(30_000), [0u32, 2].into_iter());
        let back = Transcript::from_cache(&s).unwrap();
        assert_eq!(back.transcript.source, "whisper:ggml-base.en.bin");
        assert_eq!(back.transcript.cues().len(), 2);
        assert_eq!(back.transcript.cues()[0].text, "hello there");
        assert_eq!(back.transcript.cues()[0].end, ms(1_234));
        assert_eq!(back.done_for(ms(30_000)), vec![0, 2]);
        assert!(back.done_for(ms(20_000)).is_empty(), "other chunk units are not trusted");
    }

    #[test]
    fn a_foreign_file_is_rejected_with_a_reason() {
        let e = Transcript::from_cache("WEBVTT\n\n00:00.000 --> 00:01.000\nhi").err().unwrap();
        assert!(e.contains("not a v1 transcript cache"), "{e}");
        let e = Transcript::from_cache("#vidkit-transcript\tv1\tchunk_ms=30000\n#done\t0\n12\tx\ttext").err().unwrap();
        assert!(e.contains("cue line 3"), "{e}");
    }
}
