//! Population inspector — load a run's checkpoint (the saved population) and view it
//! in-app: a scatter in (purpose z × quality genome) space coloured by fitness, plus
//! a summary. Answers "the run finished — what did it actually produce?" without
//! re-running. Drawn natively with cairo (no SVG-loader dependency), mirroring the
//! `ckptview` CLI tool so the picture is identical in-app and exported.

use gtk::prelude::*;
use gtk::cairo;
use std::cell::RefCell;
use std::rc::Rc;

use crate::engine::{load_checkpoint, Checkpoint};

const NICHES: &[&str] = &["refactor", "optimize", "cleanup", "feature", "fix"];
fn niche_label(z: f64) -> &'static str {
    NICHES[((z * NICHES.len() as f64) as usize).min(NICHES.len() - 1)]
}

fn draw_population(cr: &cairo::Context, c: &Checkpoint, w: f64, h: f64) {
    let (ml, mr, mt, mb) = (54.0, 16.0, 16.0, 26.0);
    let pw = (w - ml - mr).max(10.0);
    let ph = (h - mt - mb).max(10.0);
    let best = c.bots.iter().map(|b| b.3).fold(0.0_f64, f64::max).max(0.001);

    // background + frame
    cr.set_source_rgb(0.059, 0.067, 0.086);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    cr.set_source_rgb(0.082, 0.094, 0.13);
    cr.rectangle(ml, mt, pw, ph);
    let _ = cr.fill();

    // grid + niche labels along x
    cr.set_line_width(1.0);
    for k in 1..5 {
        let gx = ml + pw * k as f64 / 5.0;
        cr.set_source_rgb(0.13, 0.15, 0.18);
        cr.move_to(gx, mt); cr.line_to(gx, mt + ph); let _ = cr.stroke();
        let gy = mt + ph * k as f64 / 5.0;
        cr.move_to(ml, gy); cr.line_to(ml + pw, gy); let _ = cr.stroke();
    }
    cr.set_source_rgb(0.34, 0.4, 0.47);
    cr.set_font_size(10.0);
    for (i, name) in NICHES.iter().enumerate() {
        cr.move_to(ml + pw * (i as f64 + 0.5) / 5.0 - 16.0, mt + ph + 16.0);
        let _ = cr.show_text(name);
    }
    // y axis label
    cr.save().ok();
    cr.translate(14.0, mt + ph / 2.0 + 28.0);
    cr.rotate(-std::f64::consts::FRAC_PI_2);
    cr.move_to(0.0, 0.0);
    let _ = cr.show_text("genome (quality) →");
    cr.restore().ok();

    // dots
    for b in &c.bots {
        let (_id, z, genome, fit, _alive) = *b;
        let x = ml + z * pw;
        let y = mt + (1.0 - genome) * ph;
        if fit <= 0.0 {
            cr.set_source_rgba(0.23, 0.255, 0.31, 0.6);
            cr.arc(x, y, 2.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        } else {
            let t = (fit / best).clamp(0.0, 1.0);
            let r = (60.0 + 20.0 * (1.0 - t)) / 255.0;
            let g = (90.0 + 150.0 * t) / 255.0;
            let bl = (90.0 * (1.0 - t)) / 255.0;
            cr.set_source_rgba(r, g, bl, 0.85);
            cr.arc(x, y, 3.0 + 5.0 * t, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }
}

fn summarize(c: &Checkpoint) -> String {
    let n = c.bots.len();
    let scored: Vec<&(usize, f64, f64, f64, bool)> = c.bots.iter().filter(|b| b.3 > 0.0).collect();
    let ns = scored.len();
    let mean = if ns > 0 { scored.iter().map(|b| b.3).sum::<f64>() / ns as f64 } else { 0.0 };
    let best = c.bots.iter().map(|b| b.3).fold(0.0, f64::max);
    let mut s = format!(
        "seed {} · generation {} · focus z={:.2} ({})\n{n} bots, {ns} scored · best {best:.3} · mean(scored) {mean:.3}\n\nby purpose:  ",
        c.seed, c.gen, c.focus, niche_label(c.focus));
    for name in NICHES {
        let m: Vec<&&(usize, f64, f64, f64, bool)> = scored.iter().filter(|b| niche_label(b.1) == *name).collect();
        if m.is_empty() { continue; }
        let mn = m.iter().map(|b| b.3).sum::<f64>() / m.len() as f64;
        s.push_str(&format!("{name} {} (μ{mn:.2})   ", m.len()));
    }
    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
    s.push_str("\ntop bots:  ");
    for b in ranked.iter().take(6) {
        s.push_str(&format!("#{} ({:.2}@z{:.2})  ", b.0, b.3, b.1));
    }
    s
}

pub fn page() -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(8);
    root.set_margin_end(8);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let reload = gtk::Button::with_label("Reload latest checkpoint");
    reload.add_css_class("suggested-action");
    let export = gtk::Button::with_label("Export SVG…");
    bar.append(&reload);
    bar.append(&export);
    root.append(&bar);

    let info = gtk::Label::new(Some("No checkpoint loaded yet. A checkpoint is written at the end of every run; click Reload."));
    info.set_xalign(0.0);
    info.set_wrap(true);
    info.add_css_class("dim-label");
    root.append(&info);

    let area = gtk::DrawingArea::new();
    area.set_vexpand(true);
    area.set_hexpand(true);
    area.set_content_height(420);
    root.append(&area);

    let current: Rc<RefCell<Option<Checkpoint>>> = Rc::new(RefCell::new(None));

    {
        let current = current.clone();
        area.set_draw_func(move |_, cr, w, h| {
            if let Some(c) = current.borrow().as_ref() {
                draw_population(cr, c, w as f64, h as f64);
            } else {
                cr.set_source_rgb(0.059, 0.067, 0.086);
                cr.rectangle(0.0, 0.0, w as f64, h as f64);
                let _ = cr.fill();
            }
        });
    }

    let do_reload = {
        let current = current.clone();
        let area = area.clone();
        let info = info.clone();
        Rc::new(move || {
            match load_checkpoint() {
                Some(c) => {
                    info.set_text(&summarize(&c));
                    *current.borrow_mut() = Some(c);
                }
                None => info.set_text("No checkpoint found at ~/.local/share/codebot-arena/checkpoints/latest.ckpt — run something first."),
            }
            area.queue_draw();
        })
    };
    {
        let do_reload = do_reload.clone();
        reload.connect_clicked(move |_| (do_reload)());
    }
    {
        let current = current.clone();
        export.connect_clicked(move |btn| {
            if current.borrow().is_none() { return; }
            // write next to the checkpoint
            let home = std::env::var("XDG_DATA_HOME").ok()
                .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/share")))
                .unwrap_or_else(|| ".".into());
            let path = format!("{home}/codebot-arena/checkpoints/population.svg");
            if let Some(c) = current.borrow().as_ref() {
                let _ = std::fs::write(&path, crate::inspect::population_svg(c));
                btn.set_label(&format!("Saved → {path}"));
            }
        });
    }
    // load once on construction so the page isn't blank if a checkpoint exists
    (do_reload)();
    root
}

/// Standalone SVG of the population, mirroring the cairo view (for export / sharing).
pub fn population_svg(c: &Checkpoint) -> String {
    use std::fmt::Write as _;
    let (w, h) = (820.0_f64, 560.0_f64);
    let (ml, mr, mt, mb) = (70.0, 30.0, 60.0, 60.0);
    let pw = w - ml - mr; let ph = h - mt - mb;
    let best = c.bots.iter().map(|b| b.3).fold(0.0, f64::max).max(0.001);
    let mut s = String::new();
    let _ = write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="ui-monospace,monospace">"#);
    let _ = write!(s, r##"<rect width="{w}" height="{h}" fill="#0f1116"/>"##);
    let _ = write!(s, r##"<text x="{ml}" y="34" fill="#e6e9ef" font-size="17">population @ gen {} · seed {}</text>"##, c.gen, c.seed);
    let _ = write!(s, r##"<rect x="{ml}" y="{mt}" width="{pw}" height="{ph}" fill="#151821" stroke="#2d3340"/>"##);
    for (i, name) in NICHES.iter().enumerate() {
        let _ = write!(s, r##"<text x="{:.1}" y="{:.1}" fill="#566" font-size="10" text-anchor="middle">{}</text>"##,
            ml + pw * (i as f64 + 0.5) / 5.0, mt + ph + 16.0, name);
    }
    for b in &c.bots {
        let (x, y) = (ml + b.1 * pw, mt + (1.0 - b.2) * ph);
        if b.3 <= 0.0 {
            let _ = write!(s, r##"<circle cx="{x:.1}" cy="{y:.1}" r="2.5" fill="#3a4150" opacity="0.6"/>"##);
        } else {
            let t = (b.3 / best).clamp(0.0, 1.0);
            let (r, g, bl) = ((60.0 + 20.0 * (1.0 - t)) as u8, (90.0 + 150.0 * t) as u8, (90.0 * (1.0 - t)) as u8);
            let _ = write!(s, r##"<circle cx="{x:.1}" cy="{y:.1}" r="{:.1}" fill="#{r:02x}{g:02x}{bl:02x}" opacity="0.85"/>"##, 3.0 + 5.0 * t);
        }
    }
    s.push_str("</svg>");
    s
}

// ===== Audit view ==========================================================
//
// Renders the auditor's structured run trail (checkpoints/audit.log) as line charts:
// best/mean fitness, behavioural entropy (predictability), and survivor σ over the
// run, with red ticks where risk flags fired. The visual complement to the
// conversational audit stream in the Logs tab.

#[derive(Clone, Default)]
struct AuditRec { gen: f64, best: f64, mean: f64, entropy: f64, sigma: f64, risk: f64 }

fn field(line: &str, key: &str) -> Option<f64> {
    // matches `key=<number>` token-wise (avoids ops=[...] etc.)
    line.split_whitespace().find_map(|tok| {
        tok.strip_prefix(key).and_then(|r| r.strip_prefix('='))
            .and_then(|v| v.parse::<f64>().ok())
    })
}

fn load_audit() -> Vec<AuditRec> {
    let home = std::env::var("XDG_DATA_HOME").ok()
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/share")))
        .unwrap_or_else(|| ".".into());
    let path = format!("{home}/codebot-arena/checkpoints/audit.log");
    let s = match std::fs::read_to_string(&path) { Ok(s) => s, Err(_) => return Vec::new() };
    s.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).filter_map(|l| {
        Some(AuditRec {
            gen: field(l, "gen")?, best: field(l, "best").unwrap_or(0.0),
            mean: field(l, "mean").unwrap_or(0.0), entropy: field(l, "entropy").unwrap_or(0.0),
            sigma: field(l, "sigma").unwrap_or(0.0), risk: field(l, "risk").unwrap_or(0.0),
        })
    }).collect()
}

fn draw_audit(cr: &cairo::Context, recs: &[AuditRec], w: f64, h: f64) {
    cr.set_source_rgb(0.059, 0.067, 0.086);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    if recs.len() < 2 { return; }
    let (ml, mr, mt, mb) = (46.0, 16.0, 16.0, 24.0);
    let pw = (w - ml - mr).max(10.0);
    let ph = (h - mt - mb).max(10.0);
    cr.set_source_rgb(0.082, 0.094, 0.13);
    cr.rectangle(ml, mt, pw, ph); let _ = cr.fill();

    let g0 = recs.first().unwrap().gen;
    let g1 = recs.last().unwrap().gen.max(g0 + 1.0);
    let px = |g: f64| ml + (g - g0) / (g1 - g0) * pw;
    let py = |v: f64| mt + (1.0 - v.clamp(0.0, 1.0)) * ph; // all series in [0,1]

    // grid
    cr.set_source_rgb(0.13, 0.15, 0.18); cr.set_line_width(1.0);
    for k in 0..=4 {
        let y = mt + ph * k as f64 / 4.0;
        cr.move_to(ml, y); cr.line_to(ml + pw, y); let _ = cr.stroke();
        cr.set_source_rgb(0.34, 0.4, 0.47); cr.set_font_size(9.0);
        cr.move_to(6.0, y + 3.0); let _ = cr.show_text(&format!("{:.2}", 1.0 - k as f64 / 4.0));
        cr.set_source_rgb(0.13, 0.15, 0.18);
    }

    // risk ticks (red) along the bottom
    for r in recs.iter().filter(|r| r.risk > 0.0) {
        cr.set_source_rgba(0.85, 0.25, 0.3, 0.8);
        cr.move_to(px(r.gen), mt + ph); cr.line_to(px(r.gen), mt + ph - 12.0); let _ = cr.stroke();
    }

    let line = |cr: &cairo::Context, sel: &dyn Fn(&AuditRec) -> f64, col: (f64, f64, f64)| {
        cr.set_source_rgb(col.0, col.1, col.2);
        cr.set_line_width(1.6);
        for (i, r) in recs.iter().enumerate() {
            let (x, y) = (px(r.gen), py(sel(r)));
            if i == 0 { cr.move_to(x, y); } else { cr.line_to(x, y); }
        }
        let _ = cr.stroke();
    };
    line(cr, &|r| r.best, (0.30, 0.84, 0.55));     // green
    line(cr, &|r| r.mean, (0.30, 0.62, 0.88));     // blue
    line(cr, &|r| r.entropy, (0.90, 0.62, 0.24));  // orange
    line(cr, &|r| r.sigma, (0.66, 0.50, 0.86));    // purple

    // legend
    cr.set_font_size(11.0);
    let items = [("best", (0.30, 0.84, 0.55)), ("mean", (0.30, 0.62, 0.88)),
                 ("entropy (predictability)", (0.90, 0.62, 0.24)), ("σ (convergence)", (0.66, 0.50, 0.86)),
                 ("risk", (0.85, 0.25, 0.3))];
    let mut lx = ml + 6.0;
    for (name, col) in items {
        cr.set_source_rgb(col.0, col.1, col.2);
        cr.rectangle(lx, mt + 6.0, 10.0, 10.0); let _ = cr.fill();
        cr.move_to(lx + 14.0, mt + 15.0); let _ = cr.show_text(name);
        lx += 14.0 + name.len() as f64 * 6.5 + 14.0;
    }
}

pub fn audit_page() -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(8); root.set_margin_bottom(8); root.set_margin_start(8); root.set_margin_end(8);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let reload = gtk::Button::with_label("Reload audit trail");
    reload.add_css_class("suggested-action");
    bar.append(&reload);
    root.append(&bar);

    let info = gtk::Label::new(Some("The auditor writes a structured trail each run. Click Reload to chart it."));
    info.set_xalign(0.0); info.set_wrap(true); info.add_css_class("dim-label");
    root.append(&info);

    let area = gtk::DrawingArea::new();
    area.set_vexpand(true); area.set_hexpand(true); area.set_content_height(420);
    root.append(&area);

    let recs: Rc<RefCell<Vec<AuditRec>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let recs = recs.clone();
        area.set_draw_func(move |_, cr, w, h| draw_audit(cr, &recs.borrow(), w as f64, h as f64));
    }
    let do_reload = {
        let recs = recs.clone(); let area = area.clone(); let info = info.clone();
        Rc::new(move || {
            let r = load_audit();
            if r.is_empty() {
                info.set_text("No audit trail found — run something first (the trail is written at run end).");
            } else {
                let last = r.last().unwrap();
                let risks: usize = r.iter().filter(|x| x.risk > 0.0).count();
                info.set_text(&format!("{} generations recorded · final best {:.3}, entropy {:.2} · {} generation(s) with risk flags",
                    r.len(), last.best, last.entropy, risks));
            }
            *recs.borrow_mut() = r;
            area.queue_draw();
        })
    };
    { let do_reload = do_reload.clone(); reload.connect_clicked(move |_| (do_reload)()); }
    (do_reload)();
    root
}
