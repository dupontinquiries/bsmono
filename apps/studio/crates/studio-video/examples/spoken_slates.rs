//! Read the spoken slate out of every transcript in a folder, in file order,
//! and flag lone scene numbers and hallucinated transcripts.
//!
//!     cargo run -p studio-video --example spoken_slates -- TRANSCRIPT_DIR
//!
//! Transcripts are whisper `.txt` files named after their media file.
use studio_video::slate_ocr::{self, Board};
use studio_video::spoken_slate::{hallucination, read, suspicious};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: spoken_slates TRANSCRIPT_DIR");
    let mut files: Vec<_> = std::fs::read_dir(&dir).expect("dir").filter_map(|e| e.ok())
        .map(|e| e.path()).filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false)).collect();
    files.sort();
    let mut names = Vec::new();
    let mut boards: Vec<Option<Board>> = Vec::new();
    for p in &files {
        let text = std::fs::read_to_string(p).unwrap_or_default();
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let b = read(&text);
        let h = hallucination(&text);
        println!("{name}\t{}\t{}\t{}", b.as_ref().map(|b| b.label()).unwrap_or("-".into()),
                 b.as_ref().map(|b| format!("{:.2}", b.sure)).unwrap_or("-".into()),
                 h.map(|h| format!("{h:?}")).unwrap_or_default());
        names.push(name);
        boards.push(b);
    }
    for s in suspicious(&boards) {
        println!("SUSPECT {}: heard scene {}, neighbours say {}", names[s.index], s.heard, s.likely);
    }
    let clips: Vec<slate_ocr::Clip> = names.iter().zip(&boards)
        .map(|(n, b)| slate_ocr::Clip { file: n.clone(), board: b.clone().unwrap_or_default() }).collect();
    let (read_ok, unread) = slate_ocr::arrange(clips);
    eprintln!("{} of {} read completely; {} left in camera order", read_ok.len(), names.len(), unread.len());
}
