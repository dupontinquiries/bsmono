//! dictate-cli — drive the dictation engine from a transcript on stdin.
//!
//! The engine is pure and fully tested, but every transcript it has ever
//! seen was written by hand. This exists so real recognizer output can
//! be fed through it without a mic, a Qt build, or the app running:
//! capture what whisper actually produces, pipe it in, look at what
//! comes out.
//!
//! ## Build
//!
//! Plain rustc, no cargo and no Qt — the modules it uses are the
//! std-only ones exposed from lib.rs:
//!
//! ```sh
//! cd apps/voiceterm/src
//! rustc --edition 2021 -O ../tools/dictate-cli.rs -o /tmp/dictate-cli
//! ```
//!
//! (Run it from `src/` — the `#[path]` attributes below are relative to
//! this file, which lives one directory up. Building here rather than
//! adding a `[[bin]]` is deliberate: a second cargo target would still
//! drag the whole qmetaobject dependency tree in, which defeats the
//! point of being able to run this anywhere.)
//!
//! ## Use
//!
//! One utterance per line, as whisper produced it:
//!
//! ```sh
//! /tmp/dictate-cli --mode code --buffer src/main.rs < transcript.txt
//! ```
//!
//! Line prefixes:
//!   `fast: text`   this utterance came from the acoustic fast path
//!   `prev: text`   a phonetic preview (display only, never applied)
//!   `#  anything`  comment, ignored
//!   anything else  a whisper result
//!
//! Flags:
//!   --mode prose|code   starting mode           (default prose)
//!   --buffer PATH       seed the buffer and symbol table from a file
//!   --fast-enabled      let the fast path act on control verbs
//!   --quiet             final buffer only, no per-line trace
//!   --prompt LABEL      print the whisper initial_prompt for a grammar
//!                       label and exit

#[path = "../src/bar_action.rs"]
mod bar_action;
#[path = "../src/bar_pages.rs"]
mod bar_pages;
#[path = "../src/bottom_bar.rs"]
mod bottom_bar;
#[path = "../src/code_symbols.rs"]
mod code_symbols;
#[path = "../src/dictate.rs"]
mod dictate;
#[path = "../src/dictation_engine.rs"]
mod dictation_engine;
#[path = "../src/dictation_session.rs"]
mod dictation_session;
#[path = "../src/editor_voice.rs"]
mod editor_voice;
#[path = "../src/repair.rs"]
mod repair;
#[path = "../src/voice_status.rs"]
mod voice_status;

use dictate::Mode;
use dictation_engine::{Engine, Outcome};
use dictation_session::{Policy, Route, Tier};
use std::io::{BufRead, Write};

struct Args {
    mode: Mode,
    buffer: Option<String>,
    fast_enabled: bool,
    quiet: bool,
    prompt: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        mode: Mode::Prose,
        buffer: None,
        fast_enabled: false,
        quiet: false,
        prompt: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(f) = it.next() {
        match f.as_str() {
            "--mode" => {
                a.mode = match it.next().as_deref() {
                    Some("code") => Mode::Code,
                    Some("prose") => Mode::Prose,
                    other => return Err(format!("--mode wants prose|code, got {other:?}")),
                }
            }
            "--buffer" => a.buffer = Some(it.next().ok_or("--buffer wants a path")?),
            "--prompt" => a.prompt = Some(it.next().ok_or("--prompt wants a grammar label")?),
            "--fast-enabled" => a.fast_enabled = true,
            "--quiet" => a.quiet = true,
            "-h" | "--help" => return Err(usage()),
            other => return Err(format!("unknown flag {other:?}\n\n{}", usage())),
        }
    }
    Ok(a)
}

fn usage() -> String {
    "dictate-cli [--mode prose|code] [--buffer PATH] [--fast-enabled] [--quiet]\n\
     \x20           [--prompt GRAMMAR_LABEL]\n\n\
     Reads one utterance per line from stdin. Prefix a line with 'fast: '\n\
     to route it through the acoustic fast path, 'prev: ' for a phonetic\n\
     preview, or '#' for a comment."
        .into()
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    let seed = match &args.buffer {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("cannot read {p}: {e}");
                std::process::exit(2);
            }
        },
        None => String::new(),
    };

    let mut engine = Engine::new(
        &seed,
        seed.len(),
        args.mode,
        Policy { fast_enabled: args.fast_enabled, ..Default::default() },
        80,
        3,
    );

    if let Some(label) = &args.prompt {
        // Answers "what would whisper be primed with here?" without
        // running whisper — the piece of voice.rs's wiring that is
        // easiest to get wrong and hardest to see.
        match engine.whisper_prompt(label, "en") {
            Some(k) => println!("{k:?}"),
            None => println!("(none)"),
        }
        return;
    }

    let stdin = std::io::stdin();
    let mut now = 0u64;
    let mut counts = Counts::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        };
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }

        if let Some(p) = t.strip_prefix("prev:") {
            engine.preview(p.trim(), now);
            if !args.quiet {
                println!("{:<7} {}", "prev", p.trim());
            }
            continue;
        }

        let (tier, text) = match t.strip_prefix("fast:") {
            Some(rest) => (Tier::Fast, rest.trim()),
            None => (Tier::Whisper, t),
        };

        now += 1000;
        engine.capture_start(now);
        let out = engine.recognize(tier, text, 1.0, now + 300);
        counts.tally(&out);

        if !args.quiet {
            println!("{:<7} {:<50} {}", tier_label(tier), truncate(text, 50), verdict(&out));
        }
        if let Some((heard, meant)) = &out.teach {
            if !args.quiet {
                // "would offer", not "would teach": learn_from_confirmation
                // applies its own similarity filter and drops pairs that
                // look like a change of mind rather than a mishearing.
                println!("        \u{21b3} would offer confusion_table: {heard:?} -> {meant:?}");
            }
        }
        if out.ended {
            if !args.quiet {
                println!("        \u{21b3} session ended; remaining input ignored");
            }
            break;
        }
    }

    let produced = &engine.text()[seed.len().min(engine.text().len())..];
    let mut out = std::io::stdout();
    if !args.quiet {
        let _ = writeln!(out, "\n--- produced ---");
    }
    let _ = writeln!(out, "{produced}");
    if !args.quiet {
        let _ = writeln!(
            out,
            "\n{} inserted, {} controls, {} repairs ({} no-op), {} dropped",
            counts.content, counts.control, counts.repair, counts.repair_noop, counts.dropped
        );
        let _ = writeln!(out, "mode {:?}, cursor {}", engine.mode(), engine.cursor());
    }
}

#[derive(Default)]
struct Counts {
    content: usize,
    control: usize,
    repair: usize,
    repair_noop: usize,
    dropped: usize,
}

impl Counts {
    fn tally(&mut self, o: &Outcome) {
        match &o.route {
            Route::Content(_) => self.content += 1,
            Route::Control(_) => self.control += 1,
            Route::Repair(_) => {
                self.repair += 1;
                if !o.applied {
                    self.repair_noop += 1;
                }
            }
            Route::Dropped(_) => self.dropped += 1,
        }
    }
}

fn tier_label(t: Tier) -> &'static str {
    match t {
        Tier::Fast => "fast",
        Tier::Whisper => "whisper",
    }
}

fn verdict(o: &Outcome) -> String {
    match &o.route {
        Route::Content(_) => "insert".into(),
        Route::Control(c) => format!("control {:?}", c),
        Route::Repair(r) => {
            format!("repair {}{:?}", if o.applied { "" } else { "NO-OP " }, r)
        }
        Route::Dropped(d) => format!("drop {}", voice_status::drop_label(*d)),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let keep: String = s.chars().take(n.saturating_sub(1)).collect();
    format!("{keep}\u{2026}")
}
