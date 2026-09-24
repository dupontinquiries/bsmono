//! How much each transcript-gutter filter keeps, over a folder of whisper .srt files.
//!
//!     cargo run -p studio-voice --example gutter_filter -- SRT_DIR [WORDLIST]
use studio_voice::{corpus_counts, filter_words, parse_dictionary, parse_srt, WordFilter};

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("usage: gutter_filter SRT_DIR [WORDLIST]");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "srt").unwrap_or(false)).collect();
    files.sort();
    let all: Vec<Vec<_>> = files.iter().map(|p| parse_srt(&std::fs::read_to_string(p).unwrap_or_default())).collect();
    let corpus = corpus_counts(all.iter());
    let mut modes = vec![WordFilter::All, WordFilter::Filler, WordFilter::Unique];
    if let Some(w) = a.next() { modes.push(WordFilter::Dictionary(parse_dictionary(&std::fs::read_to_string(w).unwrap()))); }
    for m in &modes {
        let per: Vec<Vec<_>> = all.iter().map(|s| filter_words(s, m, &corpus)).collect();
        let hits: usize = per.iter().map(|v| v.len()).sum();
        let clips = per.iter().filter(|v| !v.is_empty()).count();
        let sample: Vec<String> = per.iter().flatten().take(8).map(|s| s.text.clone()).collect();
        println!("{:18} {hits:5} across {clips:2} clips   e.g. {}", m.label(), sample.join(" | "));
    }
}
