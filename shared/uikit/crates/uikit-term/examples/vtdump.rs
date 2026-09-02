//! Replay a raw pty capture through the parser and print the resulting grid in
//! the corpus format, so a capture can be diffed against a reference emulator
//! by eye or by `diff`.
//!
//! The formatting itself lives in `uikit_term::snapshot` because the corpus
//! test needs byte-identical output; a second copy here would make a formatting
//! difference look like a parser bug.
use std::io::Read;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: vtdump <file.bin> [cols] [rows]");
    let cols: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(80);
    let rows: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(24);
    let mut bytes = Vec::new();
    std::fs::File::open(&path).unwrap().read_to_end(&mut bytes).unwrap();
    let mut t = uikit_term::Terminal::new(cols, rows);
    t.feed_bytes(&bytes);
    print!("{}", uikit_term::snapshot::dump(&t.grid));
}
