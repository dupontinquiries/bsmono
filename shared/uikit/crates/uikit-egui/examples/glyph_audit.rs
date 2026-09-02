//! Which characters this tree draws, and whether the fonts we ship can draw
//! them. Run it against the monorepo root:
//!
//! ```sh
//! cargo run -p uikit-egui --example glyph_audit -- /path/to/repo
//! ```
//!
//! # Why this exists
//!
//! The Qt frontends got their icons from `QIcon::fromTheme` — Breeze or
//! Adwaita, SVG files on disk, resolved by the desktop at runtime. None of the
//! egui ports have that. What they have instead is Unicode characters typed
//! into string literals and drawn as TEXT, which means an icon is present if
//! and only if some installed font has that glyph, and absent as a tofu box if
//! not.
//!
//! Nothing in this tree calls `FontDefinitions` or installs a font, so every
//! app is running on exactly what egui bundles: Ubuntu-Light for Latin, plus
//! NotoEmoji and a small emoji-icon font. That set was never chosen against
//! the characters the apps actually use — it is just the default — so the
//! squares are not a mystery, they are the gap between two lists nobody had
//! put side by side.
//!
//! This tool puts them side by side. It reports, per character: how often it
//! appears, which crates use it, and whether `Fonts::has_glyph` can find it at
//! the sizes the UI draws at. A character that no font has is a box on every
//! machine, today, with no way for a user to fix it.
//!
//! # What it does NOT tell you
//!
//! Coverage is not legibility. A glyph that exists may still be the wrong
//! weight, the wrong metrics, or a colour emoji rendered monochrome. This
//! answers "is it a box", which is the failure the field report describes, and
//! nothing beyond that.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Characters that are text, not iconography, and would only add noise.
///
/// Box-drawing is the big one: `─` alone appears tens of thousands of times as
/// separator rules, and it is not an icon anyone is looking for.
fn is_prose(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        // quotes, dashes, ellipsis
        0x2010..=0x2027
        // box drawing and block elements — rules and meters, not icons
        | 0x2500..=0x259F
        // combining marks and the variation selectors that follow emoji
        | 0x0300..=0x036F | 0xFE00..=0xFE0F
    )
}

fn scan(root: &Path) -> BTreeMap<char, (usize, BTreeSet<String>)> {
    let mut found: BTreeMap<char, (usize, BTreeSet<String>)> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    let mut files = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(name.as_str(), "target" | ".git" | "node_modules" | "vendor") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            files += 1;
            let owner = crate_of(&p, root);
            // String literals only. A comment full of arrows is not drawn, and
            // counting it would inflate every number here.
            for lit in string_literals(&text) {
                for ch in lit.chars() {
                    if (ch as u32) < 0x80 || is_prose(ch) {
                        continue;
                    }
                    let slot = found.entry(ch).or_insert_with(|| (0, BTreeSet::new()));
                    slot.0 += 1;
                    slot.1.insert(owner.clone());
                }
            }
        }
    }
    // A scan that matched nothing passes every check below it, so say what it
    // saw and let the caller judge.
    eprintln!("scanned {files} .rs files under {}", root.display());
    assert!(files > 100, "only {files} files scanned — is the root right?");
    found
}

/// The crate a file belongs to, for the "who uses this" column.
fn crate_of(p: &Path, root: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    let s = rel.to_string_lossy();
    match s.split("/src/").next() {
        Some(head) => head.to_string(),
        None => s.to_string(),
    }
}

/// Contents of every `"..."` literal, escapes left alone.
///
/// Deliberately crude: it does not understand raw strings or nested escapes
/// perfectly, and it does not need to. A missed literal under-reports one
/// character that some other literal almost certainly also uses, and a
/// spurious one shows up as a character with a low count that is easy to
/// dismiss. Precision here would cost a Rust parser to answer a question a
/// grep answers.
fn string_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut lit = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => break,
                _ => lit.push(c),
            }
        }
        out.push(lit);
    }
    out
}

fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let found = scan(&root);

    // A bare context with the DEFAULT fonts — the state every app in this tree
    // is actually in, since none of them install any.
    let ctx = egui::Context::default();
    ctx.run(Default::default(), |_| {}); // one pass so the font atlas exists

    // Ask at a real UI size. Coverage does not vary by size, but asking
    // through a FontId is the API, and Body is what most of these are drawn
    // at.
    let font = egui::FontId::proportional(14.0);

    // Controls, asserted before any result is printed. `has_glyph` returning
    // a constant — because the atlas was never built, or the FontId names a
    // family that does not exist — would report either "nothing is missing"
    // or "everything is missing", and both read as a finding.
    //
    // The specific pairs matter. `▶` is covered and `▲` is not; `⬅` is
    // covered and `→` is not. That is not a range, it is the emoji-icon font's
    // hand-picked subset, and pinning two neighbours on opposite sides of it
    // is what proves this is reading real per-character coverage rather than
    // guessing from a codepoint block.
    let control = |c: char| ctx.fonts_mut(|f| f.has_glyph(&font, c));
    assert!(control('a') && control('é'), "Latin is missing — the atlas did not build");
    assert!(control('▶') && control('⬅'), "the icon font is not loaded at all");
    assert!(!control('→') && !control('▲'),
            "the controls now resolve — either a font was added (good: update these) \
             or has_glyph has stopped discriminating (bad)");

    let mut missing: Vec<(char, usize, BTreeSet<String>)> = Vec::new();
    let mut present = 0usize;
    for (ch, (n, crates)) in &found {
        let has = ctx.fonts_mut(|f| f.has_glyph(&font, *ch));
        if has {
            present += 1;
        } else {
            missing.push((*ch, *n, crates.clone()));
        }
    }
    missing.sort_by(|a, b| b.1.cmp(&a.1));

    println!(
        "\n{} distinct non-ASCII characters drawn; {present} covered, {} MISSING",
        found.len(),
        missing.len()
    );
    if missing.is_empty() {
        println!("nothing renders as a box with the bundled fonts.");
        return;
    }
    println!("\nthese render as a box on every machine, today:\n");
    println!("{:<10} {:<4} {:>5}  {}", "codepoint", "char", "uses", "crates");
    for (ch, n, crates) in &missing {
        let list: Vec<&str> = crates.iter().map(|s| s.as_str()).take(4).collect();
        let more = if crates.len() > 4 {
            format!(" +{}", crates.len() - 4)
        } else {
            String::new()
        };
        println!(
            "U+{:04X}     {:<4} {:>5}  {}{}",
            *ch as u32,
            ch,
            n,
            list.join(", "),
            more
        );
    }
    let total: usize = missing.iter().map(|(_, n, _)| n).sum();
    println!("\n{total} call sites draw a character no bundled font has.");
}
