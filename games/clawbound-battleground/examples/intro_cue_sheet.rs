// The intro's cue sheet, for scoring it: every section, narration line and
// sync point on a timeline, with a bar grid and instrumentation notes from
// the lore. Every time on it comes from src/intro.rs, so it cannot drift from
// the game.
//   cargo run --example intro_cue_sheet [out.svg]
use clawbound::intro::{self, LANE, NARRATION, PAN, PROLOGUE, SETTLE, SPAWN, VERSUS};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "intro_cue_sheet.svg".into());
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
    s += &format!(r##"<text x="{x0}" y="{}" font-size="14" fill="#8f8a98">Export: 48 kHz, stereo or mono, Opus. Loops for each map: music/jungle.opus, music/pass.opus (they start when play begins). See LORE.md for each clan's instruments.</text>"##, h - 26.0);
    s += "</svg>";
    std::fs::write(&out, s).expect("write");
    println!("cue sheet -> {out} (full {full:.1} s, short {short:.1} s)");
}
