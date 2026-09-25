// The voice script (src/voicescript.rs) as cue cards to read at the mic --
// one SVG per voice -- and a recording sheet (TSV) of every file to make.
//   cargo run --example script_cards [out dir]
use clawbound::gods::ROSTER;
use clawbound::voicescript::{file_for, Part, PARTS};

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

fn card(p: &Part) -> String {
    let def = ROSTER.iter().find(|d| d.name.to_ascii_lowercase().replace([' ', '\''], "") == p.who);
    let (title, name, motto, pelt, accent) = match def {
        Some(d) => (d.title.to_string(), d.name.to_string(), format!("\u{201c}{}\u{201d}", d.philosophy), d.palette[0], d.palette[3]),
        None => ("the voice of the battleground".into(), "The Announcer".into(), "Frostfang and Emberclaw, and everything between them.".into(), [0.30, 0.34, 0.52], [0.95, 0.78, 0.36]),
    };
    let accent = legible(accent);
    let w = 1000;
    let voice = wrap(p.voice, 78);
    let head = 250 + voice.len() as i32 * 26;
    let row = 112;
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
    s += &format!(r##"<text x="50" y="132" font-size="24" fill="#e8dcc0">{}</text>"##, esc(&title));
    s += &format!(r##"<text x="50" y="176" font-size="22" font-style="italic" fill="{}">{}</text>"##, hex(accent, 1.0), esc(&motto));
    for (i, l) in voice.iter().enumerate() {
        s += &format!(r##"<text x="50" y="{}" font-size="19" fill="#d8d2c4">{}</text>"##, 218 + i as i32 * 26, esc(l));
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
        // long lines step down so they stay on the card
        let size = if l.text.len() > 34 { 31 } else if l.text.len() > 28 { 34 } else { 38 };
        s += &format!(r##"<text x="138" y="{}" font-size="{size}" font-weight="bold" fill="#fbf6ea">{}</text>"##, y + 30, esc(l.text));
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

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "script_cards".into()));
    std::fs::create_dir_all(&out).expect("out dir");
    let mut sheet = String::from("file\tvoice\tline\tdirection\tenergy\tplays when\n");
    let mut n = 0;
    for (k, p) in PARTS.iter().enumerate() {
        let name = format!("{:02}_{}.svg", k + 1, p.who);
        std::fs::write(out.join(&name), card(p)).expect("card");
        for l in p.lines {
            sheet += &format!("{}\t{}\t{}\t{}\t{}\t{}\n", file_for(p.who, l.key), p.who, l.text, l.direction, l.energy, l.trigger);
            n += 1;
        }
    }
    std::fs::write(out.join("recording_sheet.tsv"), sheet).expect("sheet");
    println!("{} cards, {} lines -> {}", PARTS.len(), n, out.display());
}
