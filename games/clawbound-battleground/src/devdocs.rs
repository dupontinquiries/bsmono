//! devdocs -- the generated preparation documents in `dev/`: the voice
//! script's cue cards and recording sheet, and the intro's cue sheet.
//!
//! Generated from the game's own data (`voicescript`, `lore`, `intro`), so
//! they cannot say something the game does not do. `examples/script_cards`
//! and `examples/intro_cue_sheet` write them; [`write_all`] writes them all
//! into `dev/`, and a test fails if the committed copies are stale.

use crate::gods::ROSTER;
use crate::intro::{self, LANE, NARRATION, PAN, PROLOGUE, SETTLE, SPAWN, VERSUS};
use crate::voicescript::{file_for, Part, PARTS};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Greedy word wrap to `width` characters.
fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for w in s.split_whitespace() {
        let cur = lines.last_mut().unwrap();
        if !cur.is_empty() && cur.len() + 1 + w.len() > width {
            lines.push(w.to_string());
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        }
    }
    lines
}

fn hex(c: [f32; 3], k: f32) -> String {
    let f = |v: f32| ((v * k).clamp(0.0, 1.0) * 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", f(c[0]), f(c[1]), f(c[2]))
}

/// An accent that reads on the dark card: lifted toward white until its
/// luminance clears 0.5 (Stillsmoke's storm-purple all but vanished).
fn legible(c: [f32; 3]) -> [f32; 3] {
    let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let mut c = c;
    while lum(c) < 0.5 {
        c = [c[0] + (1.0 - c[0]) * 0.15, c[1] + (1.0 - c[1]) * 0.15, c[2] + (1.0 - c[2]) * 0.15];
    }
    c
}

/// One voice's cue card, as SVG.
pub fn card(p: &Part) -> String {
    let def = ROSTER.iter().find(|d| d.name.to_ascii_lowercase().replace([' ', '\''], "") == p.who);
    let (title, name, motto, pelt, accent) = match def {
        Some(d) => (d.title.to_string(), d.name.to_string(), format!("\u{201c}{}\u{201d}", d.philosophy), d.palette[0], d.palette[3]),
        None if p.who == "narrator" => ("the storyteller of the clans".into(), "The Narrator".into(), "The prologue of the first match: the world, the clans, the Gathering.".into(), [0.28, 0.22, 0.16], [0.86, 0.72, 0.48]),
        None => ("the voice of the battleground".into(), "The Announcer".into(), "Frostfang and Emberclaw, and everything between them.".into(), [0.30, 0.34, 0.52], [0.95, 0.78, 0.36]),
    };
    // who they are, from the lore: kind, age, land, pronouns
    let about = def.and_then(|d| crate::lore::of(d.name)).map(|a| format!("{} \u{b7} {} \u{b7} {} \u{b7} {}", a.species, a.age, a.land, a.gender.pronouns()));
    let accent = legible(accent);
    let w = 1000;
    let voice = wrap(p.voice, 78);
    // a long title or lore line wraps too (Sproutkit's and Oremantle's
    // titles ran off the card's edge)
    let titles = wrap(&title, 70);
    let abouts = about.as_deref().map(|a| wrap(a, 90)).unwrap_or_default();
    let extra = (titles.len() as i32 - 1).max(0) * 30;
    let head = 250 + extra + voice.len() as i32 * 26 + abouts.len() as i32 * 24 + if abouts.is_empty() { 0 } else { 6 };
    // the narrator's sentences wrap onto two lines: taller rows
    let wraps = p.lines.iter().any(|l| l.text.len() > 46);
    let row = if wraps { 142 } else { 112 };
    let h = head + p.lines.len() as i32 * row + 110;
    let mut s = String::new();
    s += &format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="DejaVu Sans, Helvetica, Arial, sans-serif">"#);
    s += &format!(
        r#"<defs><linearGradient id="hd" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs>"#,
        hex(pelt, 0.55),
        hex(pelt, 0.18)
    );
    s += &format!(r##"<rect width="{w}" height="{h}" fill="#141319"/>"##);
    s += &format!(r#"<rect width="{w}" height="{head}" fill="url(#hd)"/>"#);
    s += &format!(r#"<rect y="{}" width="{w}" height="6" fill="{}"/>"#, head - 6, hex(accent, 1.0));
    s += &format!(r##"<text x="48" y="92" font-size="64" font-weight="bold" fill="#fbf6ea">{}</text>"##, esc(&name));
    for (i, t) in titles.iter().enumerate() {
        s += &format!(r##"<text x="50" y="{}" font-size="24" fill="#e8dcc0">{}</text>"##, 132 + i as i32 * 30, esc(t));
    }
    s += &format!(r##"<text x="50" y="{}" font-size="22" font-style="italic" fill="{}">{}</text>"##, 176 + extra, hex(accent, 1.0), esc(&motto));
    let mut vy = 218 + extra;
    for a in &abouts {
        s += &format!(r##"<text x="50" y="{vy}" font-size="18" font-weight="bold" fill="#f3ead4">{}</text>"##, esc(a));
        vy += 24;
    }
    if !abouts.is_empty() {
        vy += 6;
    }
    for (i, l) in voice.iter().enumerate() {
        s += &format!(r##"<text x="50" y="{}" font-size="19" fill="#d8d2c4">{}</text>"##, vy + i as i32 * 26, esc(l));
    }
    for (i, l) in p.lines.iter().enumerate() {
        let y = head + 24 + i as i32 * row;
        if i % 2 == 0 {
            s += &format!(r##"<rect x="0" y="{}" width="{w}" height="{row}" fill="#1b1a22"/>"##, y - 18);
        }
        // energy meter: five rising bars
        for b in 0..5u8 {
            let bh = 10 + b as i32 * 8;
            let fill = if b < l.energy { hex(accent, 1.0) } else { "#34323d".to_string() };
            s += &format!(r#"<rect x="{}" y="{}" width="11" height="{bh}" rx="2" fill="{fill}"/>"#, 50 + b as i32 * 15, y + 44 - bh);
        }
        // long lines step down so they stay on the card; a sentence too
        // long for one line wraps onto two (the narrator's were cut off at
        // the card's edge)
        let mut y = y;
        if l.text.len() > 46 {
            let parts = wrap(l.text, 50);
            // never drop words from a script: the first version kept two
            // lines of three and lost "Gathering."
            assert!(parts.len() <= 2, "{}/{}: needs {} lines: {}", p.who, l.key, parts.len(), l.text);
            for (k, part) in parts.iter().enumerate() {
                s += &format!(r##"<text x="138" y="{}" font-size="28" font-weight="bold" fill="#fbf6ea">{}</text>"##, y + 28 + k as i32 * 33, esc(part));
            }
            y += 30;
        } else {
            let size = if l.text.len() > 34 { 31 } else if l.text.len() > 28 { 34 } else { 38 };
            s += &format!(r##"<text x="138" y="{}" font-size="{size}" font-weight="bold" fill="#fbf6ea">{}</text>"##, y + 30, esc(l.text));
        }
        s += &format!(
            r##"<text x="140" y="{}" font-size="17" fill="#9e98a8"><tspan font-family="DejaVu Sans Mono, monospace" fill="{}">{}</tspan>  ·  <tspan font-style="italic" fill="#cfc7b6">{}</tspan></text>"##,
            y + 60,
            hex(accent, 0.9),
            esc(&file_for(p.who, l.key)),
            esc(l.direction)
        );
        s += &format!(r##"<text x="140" y="{}" font-size="14" fill="#77727f">plays when: {}</text>"##, y + 82, esc(l.trigger));
    }
    let fy = h - 64;
    s += &format!(r##"<rect y="{}" width="{w}" height="64" fill="#0f0e13"/>"##, fy);
    // two lines: one ran off the card's right edge
    s += &format!(r##"<text x="50" y="{}" font-size="16" fill="#8f8a98">48 kHz mono · export Opus (~96 kbps) · 0.2 s of silence before and after</text>"##, fy + 28);
    s += &format!(r##"<text x="50" y="{}" font-size="16" fill="#8f8a98">kill.1 / kill.2 (and death.1 / death.2) are two different lines: record both</text>"##, fy + 50);
    s += "</svg>";
    s
}


/// The recording sheet: every file to record, tab-separated.
pub fn recording_sheet() -> String {
    let mut sheet = String::from("file\tvoice\tline\tdirection\tenergy\tplays when\n");
    for p in PARTS.iter() {
        for l in p.lines {
            sheet += &format!("{}\t{}\t{}\t{}\t{}\t{}\n", file_for(p.who, l.key), p.who, l.text, l.direction, l.energy, l.trigger);
        }
    }
    sheet
}

/// Every generated document: (path under `dev/`, contents).
pub fn all() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = PARTS.iter().enumerate().map(|(k, p)| (format!("voice_script/{:02}_{}.svg", k + 1, p.who), card(p))).collect();
    v.push(("voice_script/recording_sheet.tsv".into(), recording_sheet()));
    v.push(("music/intro_cue_sheet.svg".into(), intro_cue_sheet()));
    v
}

/// Write every generated document under `dir` (the crate's `dev/`).
pub fn write_all(dir: &std::path::Path) -> std::io::Result<usize> {
    let docs = all();
    for (path, body) in &docs {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(p, body)?;
    }
    Ok(docs.len())
}

/// The intro's cue sheet for scoring it, as SVG: every time from `intro`.
pub fn intro_cue_sheet() -> String {
    let short = intro::total();
    let full = short + PROLOGUE;
    let (w, x0, x1) = (1800.0f32, 60.0f32, 1740.0f32);
    let px = |t: f32| x0 + (x1 - x0) * t / full;
    let bpm = 90.0f32;
    let bar = 4.0 * 60.0 / bpm;
    let mut s = String::new();
    let h = 1380.0;
    s += &format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="DejaVu Sans, Helvetica, Arial, sans-serif">"#);
    s += r##"<rect width="100%" height="100%" fill="#141319"/>"##;
    s += &format!(r##"<text x="{x0}" y="56" font-size="34" font-weight="bold" fill="#fbf6ea">Intro score -- cue sheet</text>"##);
    s += &format!(
        r##"<text x="{x0}" y="90" font-size="18" fill="#cfc7b6">Full intro (first match of a session): {full:.1} s -- file music/intro_full.opus.   Short intro (every later match): {short:.1} s, the part after the prologue -- file music/intro.opus.</text>"##
    );
    s += &format!(r##"<text x="{x0}" y="116" font-size="16" fill="#9e98a8">Times in seconds from the intro's start. Bar grid at {bpm:.0} BPM, 4/4 ({bar:.2} s a bar). The camera moves between sections over ~1 s (0.9-1.6 s), not cuts.</text>"##);

    // section lane
    let sections: Vec<(f32, f32, &str, &str)> = {
        let l = PROLOGUE;
        let mut v = vec![(0.0, l, "PROLOGUE", "#5a4a36"), (l, PAN, "PAN", "#3c4a5e")];
        let mut t = l + PAN;
        for n in ["TOP", "MID", "BOT"] {
            v.push((t, LANE, n, "#3f5a4a"));
            t += LANE;
        }
        v.push((t, SPAWN, "FROSTFANG", "#35607f"));
        t += SPAWN;
        v.push((t, SPAWN, "EMBERCLAW", "#8a4028"));
        t += SPAWN;
        v.push((t, VERSUS, "VERSUS", "#6e5a24"));
        t += VERSUS;
        v.push((t, SETTLE, "SETTLE", "#3a3844"));
        v
    };
    let ly = 160.0;
    // bar grid
    let mut b = 0.0;
    let mut k = 0;
    while b <= full + 1e-3 {
        let x = px(b);
        s += &format!(r##"<line x1="{x}" y1="{}" x2="{x}" y2="{}" stroke="{}" stroke-width="1"/>"##, ly - 10.0, ly + 210.0, if k % 4 == 0 { "#3a3843" } else { "#24232b" });
        if k % 4 == 0 {
            s += &format!(r##"<text x="{}" y="{}" font-size="11" fill="#5e5a68">bar {}</text>"##, x + 3.0, ly + 222.0, k + 1);
        }
        b += bar;
        k += 1;
    }
    for (t0, d, name, col) in &sections {
        let (a, bx) = (px(*t0), px(t0 + d));
        s += &format!(r##"<rect x="{a}" y="{ly}" width="{}" height="56" fill="{col}" stroke="#141319" stroke-width="2"/>"##, bx - a);
        s += &format!(r##"<text x="{}" y="{}" font-size="14" font-weight="bold" fill="#fbf6ea">{name}</text>"##, a + 5.0, ly + 22.0);
        s += &format!(r##"<text x="{}" y="{}" font-size="12" fill="#e0d8c8">{:.1}s</text>"##, a + 5.0, ly + 42.0, t0);
    }
    s += &format!(r##"<text x="{x0}" y="{}" font-size="13" fill="#8f8a98">camera</text>"##, ly - 16.0);

    // narration lane
    let ny = ly + 90.0;
    s += &format!(r##"<text x="{x0}" y="{}" font-size="13" fill="#8f8a98">narrator (narrator/prologue1..8.opus) -- the score ducks under it</text>"##, ny - 12.0);
    for (i, (t, key)) in NARRATION.iter().enumerate() {
        let x = px(*t);
        let y = ny + (i % 2) as f32 * 62.0;
        s += &format!(r##"<rect x="{x}" y="{}" width="3" height="54" fill="#d8b878"/>"##, y);
        let text = intro::narration_text(key);
        let words: Vec<&str> = text.split(' ').collect();
        let half = words.len().div_ceil(2);
        s += &format!(r##"<text x="{}" y="{}" font-size="12" fill="#f0e6cc">{}. {}</text>"##, x + 6.0, y + 16.0, i + 1, esc(&words[..half].join(" ")));
        s += &format!(r##"<text x="{}" y="{}" font-size="12" fill="#f0e6cc">{}</text>"##, x + 6.0, y + 32.0, esc(&words[half..].join(" ")));
        s += &format!(r##"<text x="{}" y="{}" font-size="11" fill="#8f8a98">{:.1}s</text>"##, x + 6.0, y + 48.0, t);
    }

    // sync points: numbered markers on both timelines, described in a table
    // (labels on the timeline piled up: six of them fall in the last 17 s)
    let l = PROLOGUE;
    let hits: Vec<(f32, &str)> = vec![
        (0.0, "score begins (music/intro_full)"),
        (l, "the world comes into view: the pan -- the short intro (music/intro) starts here"),
        (l + PAN + 3.0 * LANE + 0.35, "FIRST CATS APPEAR in starry mist (Frostfang)"),
        (l + PAN + 3.0 * LANE + SPAWN + 0.35, "Emberclaw appear"),
        (l + PAN + 3.0 * LANE + 2.0 * SPAWN, "the VS card slams in"),
        (full - SETTLE + 0.1, "'BEGIN' -- the camera descends to the player"),
        (full, "PLAY: the announcer calls 'The clans are loosed!'; the map's own music loop takes over"),
    ];
    let marker = |s: &mut String, x: f32, y: f32, n: usize| {
        *s += &format!(r##"<polygon points="{},{} {},{} {},{}" fill="#e8a040"/>"##, x - 7.0, y, x + 7.0, y, x, y + 12.0);
        *s += &format!(r##"<text x="{x}" y="{}" font-size="11" font-weight="bold" fill="#e8a040" text-anchor="middle">{n}</text>"##, y - 3.0);
    };
    for (i, (t, _)) in hits.iter().enumerate() {
        marker(&mut s, px(*t), ly + 58.0, i + 1);
    }

    // the match intro, zoomed: the last `short` seconds at full width
    let zy = ny + 160.0;
    let zx = |t: f32| x0 + (x1 - x0) * (t - l) / short;
    s += &format!(r##"<text x="{x0}" y="{}" font-size="13" fill="#8f8a98">the match intro, zoomed -- every match plays this (music/intro.opus): times as (in the full intro / in the short one)</text>"##, zy - 12.0);
    for (t0, d, name, col) in sections.iter().filter(|x| x.0 >= l) {
        let (a, bx) = (zx(*t0), zx(t0 + d));
        s += &format!(r##"<rect x="{a}" y="{zy}" width="{}" height="56" fill="{col}" stroke="#141319" stroke-width="2"/>"##, bx - a);
        s += &format!(r##"<text x="{}" y="{}" font-size="14" font-weight="bold" fill="#fbf6ea">{name}</text>"##, a + 6.0, zy + 22.0);
        s += &format!(r##"<text x="{}" y="{}" font-size="12" fill="#e0d8c8">{:.1}s / {:.1}s</text>"##, a + 6.0, zy + 42.0, t0, t0 - l);
    }
    for (i, (t, _)) in hits.iter().enumerate().filter(|x| x.1 .0 >= l) {
        marker(&mut s, zx(*t), zy + 58.0, i + 1);
    }
    // the table
    let ty = zy + 100.0;
    s += &format!(r##"<text x="{x0}" y="{ty}" font-size="22" font-weight="bold" fill="#fbf6ea">Sync points -- land a hit here</text>"##);
    for (i, (t, what)) in hits.iter().enumerate() {
        let y = ty + 30.0 + i as f32 * 24.0;
        let rel = if *t >= l { format!("{:5.1} s", t - l) } else { "   --  ".to_string() };
        s += &format!(r##"<text x="{x0}" y="{y}" font-size="15" fill="#e8a040" font-weight="bold">{}</text>"##, i + 1);
        s += &format!(r##"<text x="{}" y="{y}" font-size="15" fill="#f3d9a8" font-family="DejaVu Sans Mono, monospace">{:5.1} s  {}</text>"##, x0 + 30.0, t, rel);
        s += &format!(r##"<text x="{}" y="{y}" font-size="15" fill="#d8d2c4">{}</text>"##, x0 + 250.0, esc(what));
    }
    s += &format!(r##"<text x="{}" y="{}" font-size="12" fill="#8f8a98">columns: time in music/intro_full, time in music/intro</text>"##, x0 + 30.0, ty + 30.0 + hits.len() as f32 * 24.0 + 6.0);

    // direction per section, from the lore
    let dy = ty + 30.0 + hits.len() as f32 * 24.0 + 50.0;
    let notes = [
        ("PROLOGUE", "Nature first: wind over ice, grass, rain, far thunder. Wood and bone enter under the narrator -- bone flute (Oldroar), kora (Goldenmane), bansuri (Duskstripe), guqin (Stillsmoke) -- a world of clans, one at a time. Build slowly; hold the brass back until line 7 (Frostfang, Emberclaw) and let it rise under line 8."),
        ("PAN + LANES", "The brass arrives in full as the world comes into view. The three lane runs are a driving pulse: frame drums and log drums, a bar and a half each."),
        ("FROSTFANG", "The cold half: hardanger fiddle and nyckelharpa, kulning, long mountain horns, singing bowls. The first hit lands with the first starry burst."),
        ("EMBERCLAW", "The warm half answers: kora and balafon, ney and darbuka, talking drum, udu. Hot wind, cicadas."),
        ("VERSUS", "Both halves at once under full brass: the Gathering. The peak of the piece."),
        ("SETTLE", "Resolve and hand over: a last chord ringing into the match, where the map's own loop begins."),
    ];
    s += &format!(r##"<text x="{x0}" y="{}" font-size="22" font-weight="bold" fill="#fbf6ea">Direction</text>"##, dy);
    let mut y = dy + 34.0;
    for (sec, note) in notes {
        s += &format!(r##"<text x="{x0}" y="{y}" font-size="15" font-weight="bold" fill="#e8c888">{sec}</text>"##);
        let mut line = String::new();
        let mut yy = y;
        for word in note.split(' ') {
            if line.len() + word.len() > 150 {
                s += &format!(r##"<text x="{}" y="{yy}" font-size="15" fill="#d8d2c4">{}</text>"##, x0 + 150.0, esc(&line));
                line.clear();
                yy += 21.0;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        s += &format!(r##"<text x="{}" y="{yy}" font-size="15" fill="#d8d2c4">{}</text>"##, x0 + 150.0, esc(&line));
        y = yy + 34.0;
    }
    s += &format!(r##"<text x="{x0}" y="{}" font-size="14" fill="#8f8a98">Export: 48 kHz, stereo or mono, Opus. Loops for each map: music/jungle.opus, music/pass.opus (they start when play begins). See dev/LORE.md for each clan's instruments.</text>"##, h - 26.0);
    s += "</svg>";
    s
}


#[cfg(test)]
mod tests {
    /// The committed documents in `dev/` are what the game's data makes
    /// now. If this fails, regenerate: `cargo run --example devdocs`.
    #[test]
    fn the_committed_dev_docs_are_current() {
        let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dev");
        for (path, body) in super::all() {
            let on_disk = std::fs::read_to_string(dev.join(&path)).unwrap_or_default();
            assert!(on_disk == body, "dev/{path} is stale: run `cargo run --example devdocs`");
        }
    }

    #[test]
    fn every_document_is_well_formed() {
        for (path, body) in super::all() {
            if path.ends_with(".svg") {
                assert!(body.starts_with("<svg") && body.ends_with("</svg>"), "{path}");
                assert_eq!(body.matches("<text").count(), body.matches("</text>").count(), "{path}: unbalanced text elements");
            }
        }
    }
}
