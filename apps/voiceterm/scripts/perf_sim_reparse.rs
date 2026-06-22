// Perf simulation: model the publish→re-parse decision and measure how many
// EXPENSIVE RichText re-parses fire over a realistic frame sequence, under
// (A) the current main.rs logic (HTML-equality dedup, cursor tied to html
// signal) vs (B) using the existing-but-unwired row_cache::classify_change
// (cursor-only → cheap signal, no re-parse).
//
// Frame model from the profile logs: a static editor where the CURSOR moves
// (blink + caret) but text is unchanged, interspersed with occasional real
// content changes. The logs showed html ~248-289B early and 2540↔2536↔2632
// oscillation later (a statusline with a changing field) plus cursor-align
// logs every frame.

#[derive(Clone)]
struct Frame { html: String, cur_row: i32, cur_col: i32 }

// (A) current logic: re-parse fires whenever html String differs from last.
// cursor props are tied to term_output_changed, but the *re-parse* cost is
// the html-signal fire. We count html-signal fires.
fn count_reparse_current(frames: &[Frame]) -> usize {
    let mut last = String::new();
    let mut reparse = 0;
    for f in frames {
        if f.html != last { reparse += 1; last = f.html.clone(); }
    }
    reparse
}

// (B) classify_change: Nothing | CursorOnly | Html. Only Html fires a
// re-parse; CursorOnly fires the cheap cursor signal (no re-parse).
#[derive(PartialEq)] enum Change { Nothing, CursorOnly, Html }
fn classify(prev: &Frame, cur: &Frame) -> Change {
    if prev.html != cur.html { Change::Html }
    else if prev.cur_row != cur.cur_row || prev.cur_col != cur.cur_col { Change::CursorOnly }
    else { Change::Nothing }
}
fn count_reparse_classified(frames: &[Frame]) -> (usize, usize) {
    let mut reparse = 0; let mut cheap = 0;
    for w in frames.windows(2) {
        match classify(&w[0], &w[1]) {
            Change::Html => reparse += 1,
            Change::CursorOnly => cheap += 1,
            Change::Nothing => {}
        }
    }
    (reparse, cheap)
}

fn main(){
    // Scenario 1: static editor, cursor blink only (caret on/off every frame),
    // text identical. 600 frames (~30s at 20fps).
    let mut s1 = Vec::new();
    let body = "<span>fn main() {</span>".repeat(40); // ~960B stable text
    for i in 0..600 {
        // blink toggles cursor col between a value and itself (caret redraw):
        // html stays identical; only cursor moves slightly on caret blink.
        s1.push(Frame{ html: body.clone(), cur_row: 5, cur_col: 12 + (i%2) });
    }
    let a1 = count_reparse_current(&s1);
    let (b1, c1) = count_reparse_classified(&s1);
    println!("Scenario 1 — static editor, cursor blink (600 frames):");
    println!("  (A) current: {} re-parses", a1);
    println!("  (B) classify_change: {} re-parses + {} cheap cursor fires", b1, c1);

    // Scenario 2: statusline oscillation (helix: a field like position/anim
    // that flips the html by a few bytes every frame even when 'idle').
    let mut s2 = Vec::new();
    for i in 0..600 {
        let pos = if i%2==0 { "2540" } else { "2536" };
        s2.push(Frame{ html: format!("{}<span class=status>{}</span>", body, pos),
                       cur_row: 31, cur_col: 5 });
    }
    let a2 = count_reparse_current(&s2);
    let (b2, _c2) = count_reparse_classified(&s2);
    println!("Scenario 2 — statusline oscillation (600 frames):");
    println!("  (A) current: {} re-parses", a2);
    println!("  (B) classify_change: {} re-parses (html genuinely differs — NOT helped)", b2);

    // Scenario 3: real typing — each frame appends a char (genuine content).
    let mut s3 = Vec::new();
    let mut t = String::from("<span>");
    for i in 0..120 { t.push((b'a'+(i%26) as u8) as char);
        s3.push(Frame{ html: t.clone(), cur_row: 0, cur_col: i as i32 }); }
    let a3 = count_reparse_current(&s3);
    let (b3,_) = count_reparse_classified(&s3);
    println!("Scenario 3 — real typing (120 frames):");
    println!("  (A) current: {} re-parses   (B) classify: {} re-parses (both correct — real changes)", a3, b3);
}
