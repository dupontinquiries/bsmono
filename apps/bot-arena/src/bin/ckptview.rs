//! ckptview — inspect a bot-arena checkpoint (.ckpt) without the GUI.
//!
//!   ckptview <file.ckpt> [--top N] [--svg out.svg]
//!
//! Prints a text summary (fitness distribution, niche breakdown, top survivors) and
//! optionally renders the population as an SVG: each bot a dot in (purpose z ×
//! quality genome) space, coloured by fitness. The high-fitness cluster is the
//! "heroes" — where in purpose/quality space the run actually converged.
//!
//! Dependency-free: parses the v1 text format directly, no engine link required, so
//! it builds and runs even on an older checkout.

use std::fmt::Write as _;

#[derive(Clone)]
struct BotRow { id: usize, z: f64, genome: f64, fitness: f64, alive: bool }

struct Ckpt { seed: u64, gen: usize, focus: f64, bots: Vec<BotRow> }

fn parse(path: &str) -> Result<Ckpt, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let (mut seed, mut gen, mut focus) = (0u64, 0usize, 0.5f64);
    let mut bots = Vec::new();
    for line in s.lines() {
        if line.starts_with('#') || line.trim().is_empty() { continue; }
        let mut p = line.split_whitespace();
        match (p.next(), p.next()) {
            (Some("seed"), Some(v)) => seed = v.parse().unwrap_or(0),
            (Some("gen"), Some(v)) => gen = v.parse().unwrap_or(0),
            (Some("focus"), Some(v)) => focus = v.parse().unwrap_or(0.5),
            (Some("bots"), _) => {}
            (Some(id), Some(z)) => {
                let (genome, fit, alive) = (p.next(), p.next(), p.next());
                if let (Ok(id), Ok(z), Some(g), Some(f), Some(a)) = (id.parse(), z.parse(), genome, fit, alive) {
                    bots.push(BotRow {
                        id, z, genome: g.parse().unwrap_or(0.0),
                        fitness: f.parse().unwrap_or(0.0), alive: a == "1",
                    });
                }
            }
            _ => {}
        }
    }
    if bots.is_empty() { return Err("no bot rows parsed (is this a v1 checkpoint?)".into()); }
    Ok(Ckpt { seed, gen, focus, bots })
}

const NICHES: &[(&str, f64)] = &[
    ("refactor", 0.1), ("optimize", 0.3), ("cleanup", 0.5), ("feature", 0.7), ("fix", 0.9),
];
fn niche_label(z: f64) -> &'static str {
    let i = ((z * NICHES.len() as f64) as usize).min(NICHES.len() - 1);
    NICHES[i].0
}

fn summary(c: &Ckpt, top: usize) -> String {
    let mut out = String::new();
    let n = c.bots.len();
    let alive = c.bots.iter().filter(|b| b.alive).count();
    let scored: Vec<&BotRow> = c.bots.iter().filter(|b| b.fitness > 0.0).collect();
    let ns = scored.len();
    let mean = if ns > 0 { scored.iter().map(|b| b.fitness).sum::<f64>() / ns as f64 } else { 0.0 };
    let best = c.bots.iter().map(|b| b.fitness).fold(0.0, f64::max);
    let worst_scored = scored.iter().map(|b| b.fitness).fold(1.0, f64::min);

    let _ = writeln!(out, "checkpoint: seed {}  generation {}  focus z={:.2} ({})",
        c.seed, c.gen, c.focus, niche_label(c.focus));
    let _ = writeln!(out, "population: {n} bots ({alive} alive, {ns} with nonzero fitness)");
    let _ = writeln!(out, "fitness: best {best:.3}, mean(scored) {mean:.3}, min(scored) {:.3}",
        if ns > 0 { worst_scored } else { 0.0 });

    // fitness histogram over [0,1] in 10 bins (scored bots only)
    let _ = writeln!(out, "\nfitness distribution (scored bots):");
    let mut bins = [0usize; 10];
    for b in &scored { bins[((b.fitness * 10.0) as usize).min(9)] += 1; }
    let maxbin = bins.iter().copied().max().unwrap_or(1).max(1);
    for (i, &cnt) in bins.iter().enumerate() {
        let bar = "█".repeat((cnt * 40 / maxbin).min(40));
        let _ = writeln!(out, "  {:.1}-{:.1} | {:<40} {cnt}", i as f64 / 10.0, (i + 1) as f64 / 10.0, bar);
    }

    // niche breakdown: mean fitness of scored bots per purpose bucket
    let _ = writeln!(out, "\nby purpose (niche), scored bots:");
    for (name, _) in NICHES {
        let members: Vec<&&BotRow> = scored.iter().filter(|b| niche_label(b.z) == *name).collect();
        if members.is_empty() { let _ = writeln!(out, "  {name:<9} | (none)"); continue; }
        let m = members.iter().map(|b| b.fitness).sum::<f64>() / members.len() as f64;
        let bn = members.iter().map(|b| b.fitness).fold(0.0, f64::max);
        let _ = writeln!(out, "  {name:<9} | {:>3} bots  mean {m:.3}  best {bn:.3}", members.len());
    }

    // top survivors
    let mut ranked = scored.clone();
    ranked.sort_by(|a, b| b.fitness.partial_cmp(&a.fitness).unwrap());
    let _ = writeln!(out, "\ntop {} bots:", top.min(ranked.len()));
    let _ = writeln!(out, "  {:>4}  {:>6}  {:>7}  {:>7}  purpose", "id", "z", "genome", "fitness");
    for b in ranked.iter().take(top) {
        let _ = writeln!(out, "  {:>4}  {:>6.3}  {:>7.3}  {:>7.3}  {}", b.id, b.z, b.genome, b.fitness, niche_label(b.z));
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Population as a scatter in (z × genome) space, coloured by fitness. Greener and
/// larger = fitter. Greyed dots are unscored (fitness 0). A fitness histogram runs
/// along the bottom.
fn svg(c: &Ckpt) -> String {
    let (w, h) = (820.0_f64, 640.0_f64);
    let (ml, mr, mt, mb) = (70.0, 30.0, 70.0, 160.0); // plot margins
    let pw = w - ml - mr;
    let ph = h - mt - mb;
    let best = c.bots.iter().map(|b| b.fitness).fold(0.0, f64::max).max(0.001);

    let px = |z: f64| ml + z * pw;
    let py = |g: f64| mt + (1.0 - g) * ph; // genome up

    let mut s = String::new();
    let _ = write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="ui-monospace,monospace">"#);
    let _ = write!(s, r##"<rect width="{w}" height="{h}" fill="#0f1116"/>"##);
    // title
    let _ = write!(s, r##"<text x="{ml}" y="32" fill="#e6e9ef" font-size="18">population @ gen {} · seed {}</text>"##, c.gen, c.seed);
    let _ = write!(s, r##"<text x="{ml}" y="52" fill="#7d8799" font-size="12">{} bots · purpose (z) × quality (genome) · colour = fitness</text>"##, c.bots.len());

    // plot frame + grid
    let _ = write!(s, r##"<rect x="{ml}" y="{mt}" width="{pw}" height="{ph}" fill="#151821" stroke="#2d3340"/>"##);
    for k in 1..5 {
        let gx = ml + pw * k as f64 / 5.0;
        let _ = write!(s, r##"<line x1="{gx:.1}" y1="{mt}" x2="{gx:.1}" y2="{:.1}" stroke="#21262f"/>"##, mt + ph);
        // niche label along x
        let lbl = NICHES[(k - 1).min(NICHES.len() - 1)].0;
        let _ = write!(s, r##"<text x="{:.1}" y="{:.1}" fill="#566" font-size="10" text-anchor="middle">{}</text>"##,
            ml + pw * (k as f64 - 0.5) / 5.0, mt + ph + 16.0, lbl);
    }
    for k in 1..5 {
        let gy = mt + ph * k as f64 / 5.0;
        let _ = write!(s, r##"<line x1="{ml}" y1="{gy:.1}" x2="{:.1}" y2="{gy:.1}" stroke="#21262f"/>"##, ml + pw);
    }
    let _ = write!(s, r##"<text x="20" y="{:.1}" fill="#7d8799" font-size="11" transform="rotate(-90 20 {:.1})">genome (quality) →</text>"##, mt + ph / 2.0, mt + ph / 2.0);

    // dots
    for b in &c.bots {
        let (x, y) = (px(b.z), py(b.genome));
        if b.fitness <= 0.0 {
            let _ = write!(s, r##"<circle cx="{x:.1}" cy="{y:.1}" r="2.5" fill="#3a4150" opacity="0.6"/>"##);
        } else {
            let t = (b.fitness / best).clamp(0.0, 1.0);
            // grey→green gradient by fitness
            let r = (60.0 + 20.0 * (1.0 - t)) as u8;
            let g = (90.0 + 150.0 * t) as u8;
            let bl = (90.0 * (1.0 - t)) as u8;
            let rad = 3.0 + 5.0 * t;
            let _ = write!(s, r##"<circle cx="{x:.1}" cy="{y:.1}" r="{rad:.1}" fill="#{r:02x}{g:02x}{bl:02x}" opacity="0.85"/>"##);
        }
    }

    // fitness histogram along the bottom
    let hist_top = mt + ph + 40.0;
    let hist_h = 70.0;
    let mut bins = [0usize; 20];
    let scored: Vec<&BotRow> = c.bots.iter().filter(|b| b.fitness > 0.0).collect();
    for b in &scored { bins[((b.fitness * 20.0) as usize).min(19)] += 1; }
    let maxbin = bins.iter().copied().max().unwrap_or(1).max(1) as f64;
    let bw = pw / 20.0;
    let _ = write!(s, r##"<text x="{ml}" y="{:.1}" fill="#7d8799" font-size="11">fitness distribution ({} scored)</text>"##, hist_top - 6.0, scored.len());
    for (i, &cnt) in bins.iter().enumerate() {
        let bh = hist_h * (cnt as f64 / maxbin);
        let x = ml + i as f64 * bw;
        let t = (i as f64 + 0.5) / 20.0;
        let g = (90.0 + 150.0 * t) as u8;
        let _ = write!(s, r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="#3a{g:02x}50" opacity="0.85"/>"##,
            x + 0.5, hist_top + (hist_h - bh), bw - 1.0, bh);
    }
    for k in 0..=4 {
        let x = ml + pw * k as f64 / 4.0;
        let _ = write!(s, r##"<text x="{x:.1}" y="{:.1}" fill="#566" font-size="10" text-anchor="middle">{:.2}</text>"##,
            hist_top + hist_h + 14.0, k as f64 / 4.0);
    }

    let _ = write!(s, "{}", xml_escape("")); // no-op to keep escape used
    s.push_str("</svg>");
    s
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // --audit <audit.log> [--svg out.svg]: chart the auditor's run trail instead.
    if let Some(i) = args.iter().position(|a| a == "--audit") {
        let path = args.get(i + 1).cloned().unwrap_or_default();
        let svg_out = args.iter().position(|a| a == "--svg").and_then(|j| args.get(j + 1)).cloned();
        match audit_chart(&path) {
            Ok((summary_text, svg)) => {
                print!("{summary_text}");
                match svg_out {
                    Some(p) => match std::fs::write(&p, svg) {
                        Ok(_) => println!("\nwrote audit chart → {p}"),
                        Err(e) => { eprintln!("could not write {p}: {e}"); std::process::exit(1); }
                    },
                    None => println!("\n(pass --svg out.svg to render the audit chart)"),
                }
            }
            Err(e) => { eprintln!("ckptview --audit: {e}"); std::process::exit(1); }
        }
        return;
    }

    let file = args.iter().skip(1).find(|a| !a.starts_with("--"));
    let file = match file {
        Some(f) => f.clone(),
        None => {
            // default to the registry's latest checkpoint
            let home = std::env::var("XDG_DATA_HOME").ok()
                .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/share")))
                .unwrap_or_else(|| ".".into());
            format!("{home}/codebot-arena/checkpoints/latest.ckpt")
        }
    };
    let top = args.iter().position(|a| a == "--top")
        .and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(12usize);
    let svg_path = args.iter().position(|a| a == "--svg").and_then(|i| args.get(i + 1)).cloned();

    let c = match parse(&file) {
        Ok(c) => c,
        Err(e) => { eprintln!("ckptview: {e}"); std::process::exit(1); }
    };
    print!("{}", summary(&c, top));
    if let Some(p) = svg_path {
        match std::fs::write(&p, svg(&c)) {
            Ok(_) => println!("\nwrote SVG → {p}"),
            Err(e) => { eprintln!("could not write {p}: {e}"); std::process::exit(1); }
        }
    } else {
        println!("\n(pass --svg out.svg to render the population landscape)");
    }
}

// ===== Audit trail charting ================================================

#[derive(Default, Clone)]
struct ARec { gen: f64, best: f64, mean: f64, entropy: f64, sigma: f64, risk: f64 }

fn afield(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace().find_map(|t| {
        t.strip_prefix(key).and_then(|r| r.strip_prefix('=')).and_then(|v| v.parse().ok())
    })
}

fn audit_chart(path: &str) -> Result<(String, String), String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let recs: Vec<ARec> = s.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| Some(ARec {
            gen: afield(l, "gen")?, best: afield(l, "best").unwrap_or(0.0),
            mean: afield(l, "mean").unwrap_or(0.0), entropy: afield(l, "entropy").unwrap_or(0.0),
            sigma: afield(l, "sigma").unwrap_or(0.0), risk: afield(l, "risk").unwrap_or(0.0),
        })).collect();
    if recs.len() < 2 { return Err("need at least 2 recorded generations".into()); }

    let last = recs.last().unwrap();
    let risks = recs.iter().filter(|r| r.risk > 0.0).count();
    let mean_entropy = recs.iter().map(|r| r.entropy).sum::<f64>() / recs.len() as f64;
    let mut text = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(text, "audit trail: {} generations recorded", recs.len());
    let _ = writeln!(text, "final: best {:.3}, mean {:.3}, entropy {:.2} ({}), σ {:.3}",
        last.best, last.mean, last.entropy,
        if last.entropy < 0.45 { "predictable" } else if last.entropy < 0.75 { "mixed" } else { "exploratory" },
        last.sigma);
    let _ = writeln!(text, "mean behavioural entropy over run: {mean_entropy:.2}");
    let _ = writeln!(text, "generations with risk flags: {risks}");

    // SVG line chart (best/mean/entropy/sigma in [0,1]; red ticks for risk)
    let (w, h) = (900.0_f64, 460.0_f64);
    let (ml, mr, mt, mb) = (48.0, 16.0, 40.0, 28.0);
    let pw = w - ml - mr; let ph = h - mt - mb;
    let g0 = recs.first().unwrap().gen;
    let g1 = recs.last().unwrap().gen.max(g0 + 1.0);
    let px = |g: f64| ml + (g - g0) / (g1 - g0) * pw;
    let py = |v: f64| mt + (1.0 - v.clamp(0.0, 1.0)) * ph;
    let mut svg = String::new();
    let _ = write!(svg, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="ui-monospace,monospace">"#);
    let _ = write!(svg, r##"<rect width="{w}" height="{h}" fill="#0f1116"/>"##);
    let _ = write!(svg, r##"<text x="{ml}" y="26" fill="#e6e9ef" font-size="15">run audit · {} generations</text>"##, recs.len());
    let _ = write!(svg, r##"<rect x="{ml}" y="{mt}" width="{pw}" height="{ph}" fill="#151821" stroke="#2d3340"/>"##);
    for k in 0..=4 {
        let y = mt + ph * k as f64 / 4.0;
        let _ = write!(svg, r##"<line x1="{ml}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="#21262f"/>"##, ml + pw);
        let _ = write!(svg, r##"<text x="8" y="{:.1}" fill="#566" font-size="9">{:.2}</text>"##, y + 3.0, 1.0 - k as f64 / 4.0);
    }
    for r in recs.iter().filter(|r| r.risk > 0.0) {
        let _ = write!(svg, r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#d84050" stroke-width="1.5" opacity="0.8"/>"##,
            px(r.gen), mt + ph, px(r.gen), mt + ph - 14.0);
    }
    let poly = |sel: &dyn Fn(&ARec) -> f64, color: &str| {
        let pts: Vec<String> = recs.iter().map(|r| format!("{:.1},{:.1}", px(r.gen), py(sel(r)))).collect();
        format!(r##"<polyline points="{}" fill="none" stroke="{color}" stroke-width="1.6"/>"##, pts.join(" "))
    };
    svg.push_str(&poly(&|r| r.best, "#4dd68c"));
    svg.push_str(&poly(&|r| r.mean, "#4d9de0"));
    svg.push_str(&poly(&|r| r.entropy, "#e69e3d"));
    svg.push_str(&poly(&|r| r.sigma, "#a980db"));
    let legend = [("best", "#4dd68c"), ("mean", "#4d9de0"), ("entropy (predictability)", "#e69e3d"), ("σ (convergence)", "#a980db"), ("risk", "#d84050")];
    let mut lx = ml + 4.0;
    for (name, col) in legend {
        let _ = write!(svg, r##"<rect x="{lx:.1}" y="{:.1}" width="10" height="10" fill="{col}"/>"##, mt + 6.0);
        let _ = write!(svg, r##"<text x="{:.1}" y="{:.1}" fill="#9aa3b2" font-size="11">{name}</text>"##, lx + 14.0, mt + 15.0);
        lx += 14.0 + name.len() as f64 * 6.6 + 16.0;
    }
    svg.push_str("</svg>");
    Ok((text, svg))
}
