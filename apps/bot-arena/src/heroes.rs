//! Heroes gallery — evolve small FSM "specialists" and render each natively with
//! cairo for visual inspection. Small machines are the point: few states means
//! narrow, legible, specialized behaviour that a higher-level controller could
//! call like a subroutine. No SVG loader dependency — we draw the graph directly.

use gtk::prelude::*;
use gtk::cairo;
use std::cell::RefCell;
use std::f64::consts::{TAU, FRAC_PI_2};
use std::rc::Rc;

use crate::fsm::{Fsm, StateKind};

fn rgb(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

fn node_colors(k: &StateKind) -> ((f64, f64, f64), (f64, f64, f64)) {
    match k {
        StateKind::Char(_)  => (rgb(0x1e, 0x3a, 0x5f), rgb(0x4d, 0x9d, 0xe0)),
        StateKind::Pipe     => (rgb(0x5a, 0x3d, 0x1e), rgb(0xe0, 0x90, 0x3d)),
        StateKind::Space    => (rgb(0x2d, 0x33, 0x40), rgb(0x7d, 0x87, 0x99)),
        StateKind::Token(_) => (rgb(0x1e, 0x40, 0x30), rgb(0x3d, 0xd6, 0x8c)),
        StateKind::Dup | StateKind::Mark | StateKind::Recall => (rgb(0x3a, 0x2a, 0x4d), rgb(0xa8, 0x7f, 0xd6)),
    }
}

/// Render an FSM into a cairo context sized `w`×`h`: dark card, weighted edges
/// (with arrowheads / self-loops), nodes coloured by emission kind, start node
/// dashed. Mirrors the SVG export's circular layout so heroes look consistent
/// whether drawn live or exported.
pub fn draw_fsm(cr: &cairo::Context, fsm: &Fsm, w: f64, h: f64) {
    let bg = rgb(0x0f, 0x11, 0x16);
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    let n = fsm.kinds.len().max(1);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let node_r = if n > 9 { 11.0 } else { 15.0 };
    let r = (w.min(h) / 2.0) - node_r - 10.0;
    let pos: Vec<(f64, f64)> = (0..n).map(|i| {
        let a = (i as f64) / (n as f64) * TAU - FRAC_PI_2;
        (cx + r * a.cos(), cy + r * a.sin())
    }).collect();

    // edges (under nodes)
    let edge = rgb(0x4a, 0x55, 0x68);
    for (i, outs) in fsm.edges.iter().enumerate() {
        for (t, wt) in outs {
            cr.set_source_rgb(edge.0, edge.1, edge.2);
            cr.set_line_width(0.6 + (*wt as f64) * 2.6);
            let (x1, y1) = pos[i];
            if i == *t {
                cr.arc(x1, y1 - node_r - 6.0, 6.0, 0.0, TAU);
                let _ = cr.stroke();
                continue;
            }
            let (x2, y2) = pos[*t];
            let (dx, dy) = (x2 - x1, y2 - y1);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let (ux, uy) = (dx / len, dy / len);
            let (sx, sy) = (x1 + ux * node_r, y1 + uy * node_r);
            let (ex, ey) = (x2 - ux * (node_r + 4.0), y2 - uy * (node_r + 4.0));
            cr.move_to(sx, sy);
            cr.line_to(ex, ey);
            let _ = cr.stroke();
            // arrowhead
            let ah = 5.0;
            let (px, py) = (-uy, ux); // perpendicular
            cr.move_to(ex, ey);
            cr.line_to(ex - ux * ah + px * ah * 0.6, ey - uy * ah + py * ah * 0.6);
            cr.line_to(ex - ux * ah - px * ah * 0.6, ey - uy * ah - py * ah * 0.6);
            cr.close_path();
            let _ = cr.fill();
        }
    }

    // nodes
    cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(if n > 9 { 8.0 } else { 10.0 });
    for (i, (x, y)) in pos.iter().enumerate() {
        let (fill, stroke) = node_colors(&fsm.kinds[i]);
        cr.arc(*x, *y, node_r, 0.0, TAU);
        cr.set_source_rgb(fill.0, fill.1, fill.2);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(stroke.0, stroke.1, stroke.2);
        cr.set_line_width(if i == fsm.start { 2.6 } else { 1.6 });
        if i == fsm.start { cr.set_dash(&[3.0, 2.0], 0.0); } else { cr.set_dash(&[], 0.0); }
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);

        let lbl = fsm.kinds[i].label();
        cr.set_source_rgb(0.90, 0.91, 0.94);
        if let Ok(ext) = cr.text_extents(&lbl) {
            cr.move_to(*x - ext.width() / 2.0 - ext.x_bearing(), *y + ext.height() / 2.0);
            let _ = cr.show_text(&lbl);
        }
    }
}

fn edge_count(fsm: &Fsm) -> usize { fsm.edges.iter().map(|e| e.len()).sum() }

/// One hero card: the drawn machine plus a stats caption.
fn hero_card(fsm: Fsm, caption: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.set_width_request(190);
    let area = gtk::DrawingArea::new();
    area.set_content_width(180);
    area.set_content_height(180);
    {
        let fsm = fsm.clone();
        area.set_draw_func(move |_, cr, w, h| draw_fsm(cr, &fsm, w as f64, h as f64));
    }
    card.append(&area);
    let label = gtk::Label::new(Some(caption));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    label.add_css_class("dim-label");
    label.set_width_chars(24);
    card.append(&label);
    card
}

fn regenerate(flow: &gtk::FlowBox, n_states: usize, max_conn: usize, count: usize, target: &str, evo: crate::fsm::EvoOps) {
    // clear
    while let Some(child) = flow.first_child() { flow.remove(&child); }
    let target = target.trim();
    let ops = crate::engine::all_edit_ops();
    let active: Vec<&str> = [("concat", evo.concat), ("reverse", evo.reverse),
        ("dedup", evo.dedup), ("prune", evo.prune)]
        .iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect();
    for i in 0..count {
        let seed = 0x9E3779B97F4A7C15u64 ^ (i as u64).wrapping_mul(0x100000001B3);
        let (fsm, caption) = if target.is_empty() {
            // RANDOM, untrained machines: by definition terrible at any task — the
            // gallery shows their *structure*, not competence. To see machines that
            // do well at something, give a target string and they'll be evolved.
            let fsm = Fsm::random_seeded(n_states, max_conn, seed);
            let prog = fsm.run_ops(&ops, 6, &mut rng_from(seed ^ 0xF00D));
            let cap = format!("RANDOM · {} states · {} edges\nas policy: {}",
                fsm.n_states(), edge_count(&fsm), prog.join(" → "));
            (fsm, cap)
        } else {
            let (fsm, _out, fit) = crate::fsm::evolve_toward_with(target, n_states, max_conn, 40, 30, seed, evo);
            let prog = fsm.run_ops(&ops, 6, &mut rng_from(seed ^ 0xF00D));
            let ops_line = if active.is_empty() { String::new() } else { format!(" · ops: {}", active.join("+")) };
            let cap = format!("evolved → '{}' · fit {:.2}{}\n{} states · {} edges\npolicy: {}",
                short(target), fit, ops_line, fsm.n_states(), edge_count(&fsm), prog.join(" → "));
            (fsm, cap)
        };
        flow.insert(&hero_card(fsm, &caption), -1);
    }
}

fn short(s: &str) -> String {
    if s.chars().count() > 16 { format!("{}…", s.chars().take(15).collect::<String>()) } else { s.to_string() }
}

fn rng_from(seed: u64) -> rand::rngs::SmallRng {
    use rand::SeedableRng;
    rand::rngs::SmallRng::seed_from_u64(seed)
}

fn spin(min: f64, max: f64, val: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, 1.0);
    s.set_value(val);
    s.set_valign(gtk::Align::Center);
    s
}

/// Build the Heroes page: controls bar + scrollable grid of evolved machines.
pub fn page() -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(8);
    root.set_margin_end(8);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let states = spin(2.0, 16.0, 6.0);   // small by default — specialists, not generalists
    let conn = spin(1.0, 6.0, 3.0);
    let count = spin(1.0, 48.0, 8.0);
    let target = gtk::Entry::new();
    target.set_placeholder_text(Some("target string (optional) — e.g. cargo test"));
    target.set_hexpand(true);
    let go = gtk::Button::with_label("Evolve heroes");
    go.add_css_class("suggested-action");

    let lbl = |t: &str| { let l = gtk::Label::new(Some(t)); l.add_css_class("dim-label"); l };
    bar.append(&lbl("states")); bar.append(&states);
    bar.append(&lbl("max edges")); bar.append(&conn);
    bar.append(&lbl("count")); bar.append(&count);
    bar.append(&target);
    bar.append(&go);
    root.append(&bar);

    // structural genetic-operator toggles
    let toggle = |label: &str| -> (gtk::Box, gtk::Switch) {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let sw = gtk::Switch::new();
        sw.set_valign(gtk::Align::Center);
        let l = gtk::Label::new(Some(label));
        l.add_css_class("dim-label");
        b.append(&sw); b.append(&l);
        (b, sw)
    };
    let ops_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let ops_lbl = gtk::Label::new(Some("operators:"));
    ops_lbl.add_css_class("dim-label");
    ops_bar.append(&ops_lbl);
    let (concat_box, concat_sw) = toggle("concat (sequence parents)");
    let (reverse_box, reverse_sw) = toggle("reverse (transpose edges)");
    let (dedup_box, dedup_sw) = toggle("dedup (merge twins)");
    let (prune_box, prune_sw) = toggle("prune (drop unreachable)");
    ops_bar.append(&concat_box); ops_bar.append(&reverse_box);
    ops_bar.append(&dedup_box); ops_bar.append(&prune_box);
    root.append(&ops_bar);

    let hint = gtk::Label::new(Some(
        "Blank target → RANDOM untrained machines (expected to be useless — this view is for inspecting structure). \
         Type a target string to evolve machines toward it and see a fitness score."));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");
    root.append(&hint);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(8)
        .row_spacing(8)
        .column_spacing(8)
        .build();
    let scroller = gtk::ScrolledWindow::builder().vexpand(true).hexpand(true).child(&flow).build();
    root.append(&scroller);

    let flow_rc = Rc::new(flow);
    let regen = {
        let flow = flow_rc.clone();
        let states = states.clone();
        let conn = conn.clone();
        let count = count.clone();
        let target = target.clone();
        let concat_sw = concat_sw.clone();
        let reverse_sw = reverse_sw.clone();
        let dedup_sw = dedup_sw.clone();
        let prune_sw = prune_sw.clone();
        Rc::new(move || {
            let evo = crate::fsm::EvoOps {
                concat: concat_sw.is_active(),
                reverse: reverse_sw.is_active(),
                dedup: dedup_sw.is_active(),
                prune: prune_sw.is_active(),
            };
            regenerate(&flow, states.value() as usize, conn.value() as usize,
                       count.value() as usize, target.text().as_str(), evo);
        })
    };
    {
        let regen = regen.clone();
        go.connect_clicked(move |_| (regen)());
    }
    {
        let regen = regen.clone();
        target.connect_activate(move |_| (regen)());
    }
    // re-evolve live when an operator is toggled
    for sw in [&concat_sw, &reverse_sw, &dedup_sw, &prune_sw] {
        let regen = regen.clone();
        sw.connect_active_notify(move |_| (regen)());
    }
    // seed the gallery once on construction
    (regen)();
    root
}
