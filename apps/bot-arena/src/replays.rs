//! Replays tab — buffers a per-generation snapshot of the run (best, mean, and
//! the top-k bots) and lets you scrub back through it: a cairo trajectory chart
//! with a marker at the selected generation, plus a leaderboard for that frame.
//! "Top-k event monitoring + logging" without flooding the live grid.

use gtk::prelude::*;
use gtk::cairo;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone)]
pub struct Frame {
    pub gen: usize,
    pub best: f64,
    pub mean: f64,
    pub top: Vec<(usize, f64, f64)>, // (id, z, fitness)
}

#[derive(Clone)]
pub struct Replays {
    pub root: gtk::Box,
    frames: Rc<RefCell<Vec<Frame>>>,
    cursor: Rc<Cell<usize>>,
    follow: Rc<Cell<bool>>,
    scale: gtk::Scale,
    chart: gtk::DrawingArea,
    board: gtk::Label,
    header: gtk::Label,
}

fn rgb(r: u8, g: u8, b: u8) -> (f64, f64, f64) { (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0) }

fn draw_trajectory(cr: &cairo::Context, frames: &[Frame], cursor: usize, w: f64, h: f64) {
    let bg = rgb(0x0f, 0x11, 0x16);
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    if frames.len() < 2 { return; }

    let (pad_l, pad_r, pad_t, pad_b) = (40.0, 12.0, 12.0, 22.0);
    let (pw, ph) = (w - pad_l - pad_r, h - pad_t - pad_b);
    // y range auto-scaled to observed values, padded
    let mut ymin = f64::INFINITY;
    let mut ymax = f64::NEG_INFINITY;
    for f in frames {
        ymin = ymin.min(f.mean).min(f.best);
        ymax = ymax.max(f.best).max(f.mean);
    }
    if !ymin.is_finite() { ymin = 0.0; ymax = 1.0; }
    let span = (ymax - ymin).max(1e-3);
    ymin = (ymin - span * 0.08).max(0.0);
    ymax = (ymax + span * 0.08).min(1.0);
    let n = frames.len();
    let x_at = |i: usize| pad_l + pw * (i as f64) / ((n - 1).max(1) as f64);
    let y_at = |v: f64| pad_t + ph * (1.0 - (v - ymin) / (ymax - ymin).max(1e-6));

    // gridlines + y labels
    cr.set_source_rgb(0.18, 0.20, 0.25);
    cr.set_line_width(1.0);
    cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(9.0);
    for g in 0..=4 {
        let v = ymin + (ymax - ymin) * (g as f64 / 4.0);
        let y = y_at(v);
        cr.move_to(pad_l, y); cr.line_to(w - pad_r, y); let _ = cr.stroke();
        cr.set_source_rgb(0.5, 0.54, 0.6);
        cr.move_to(4.0, y + 3.0); let _ = cr.show_text(&format!("{v:.2}"));
        cr.set_source_rgb(0.18, 0.20, 0.25);
    }

    // cursor marker
    if cursor < n {
        let x = x_at(cursor);
        cr.set_source_rgb(0.35, 0.38, 0.45);
        cr.set_line_width(1.5);
        cr.move_to(x, pad_t); cr.line_to(x, pad_t + ph); let _ = cr.stroke();
    }

    // mean line (dim) then best line (bright)
    let plot = |cr: &cairo::Context, sel: fn(&Frame) -> f64, col: (f64, f64, f64)| {
        cr.set_source_rgb(col.0, col.1, col.2);
        cr.set_line_width(1.8);
        for (i, f) in frames.iter().enumerate() {
            let (x, y) = (x_at(i), y_at(sel(f)));
            if i == 0 { cr.move_to(x, y); } else { cr.line_to(x, y); }
        }
        let _ = cr.stroke();
    };
    plot(cr, |f| f.mean, rgb(0x7d, 0x87, 0x99));
    plot(cr, |f| f.best, rgb(0x3d, 0xd6, 0x8c));
}

impl Replays {
    pub fn new() -> Replays {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_margin_top(8); root.set_margin_bottom(8);
        root.set_margin_start(8); root.set_margin_end(8);

        let header = gtk::Label::new(Some("No run recorded yet — start a run to capture the replay."));
        header.set_xalign(0.0);
        header.add_css_class("dim-label");
        root.append(&header);

        let chart = gtk::DrawingArea::new();
        chart.set_content_height(220);
        chart.set_hexpand(true);
        root.append(&chart);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let follow_btn = gtk::ToggleButton::with_label("Follow latest");
        follow_btn.set_active(true);
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 1.0, 1.0);
        scale.set_hexpand(true);
        scale.set_draw_value(true);
        scale.set_value_pos(gtk::PositionType::Left);
        scale.set_digits(0);
        bar.append(&follow_btn);
        bar.append(&scale);
        root.append(&bar);

        let board = gtk::Label::new(Some(""));
        board.set_xalign(0.0);
        board.set_yalign(0.0);
        board.set_wrap(false);
        board.add_css_class("monospace");
        let board_scroll = gtk::ScrolledWindow::builder().vexpand(true).child(&board).build();
        root.append(&board_scroll);

        let r = Replays {
            root, frames: Rc::new(RefCell::new(Vec::new())),
            cursor: Rc::new(Cell::new(0)), follow: Rc::new(Cell::new(true)),
            scale, chart, board, header,
        };

        // chart draw func
        {
            let frames = r.frames.clone();
            let cursor = r.cursor.clone();
            r.chart.set_draw_func(move |_, cr, w, h| {
                draw_trajectory(cr, &frames.borrow(), cursor.get(), w as f64, h as f64);
            });
        }
        // scrubbing
        {
            let me = r.clone();
            r.scale.connect_value_changed(move |s| {
                let idx = (s.value() as usize).saturating_sub(1);
                me.cursor.set(idx);
                me.refresh_board();
                me.chart.queue_draw();
            });
        }
        {
            let follow = r.follow.clone();
            follow_btn.connect_toggled(move |b| follow.set(b.is_active()));
        }
        r
    }

    pub fn reset(&self) {
        self.frames.borrow_mut().clear();
        self.cursor.set(0);
        self.scale.set_range(1.0, 1.0);
        self.board.set_text("");
        self.header.set_text("Recording…");
        self.chart.queue_draw();
    }

    pub fn push(&self, frame: Frame) {
        {
            let mut f = self.frames.borrow_mut();
            f.push(frame);
            let n = f.len();
            self.scale.set_range(1.0, n.max(1) as f64);
            if self.follow.get() {
                self.cursor.set(n - 1);
                self.scale.set_value(n as f64);
            }
        }
        self.header.set_text(&format!("{} generations recorded · green = best, grey = mean", self.frames.borrow().len()));
        self.refresh_board();
        self.chart.queue_draw();
    }

    fn refresh_board(&self) {
        let frames = self.frames.borrow();
        let idx = self.cursor.get().min(frames.len().saturating_sub(1));
        let Some(f) = frames.get(idx) else { self.board.set_text(""); return; };
        let mut s = format!("generation {}    best {:.3}    mean {:.3}\n\ntop bots:\n", f.gen, f.best, f.mean);
        for (rank, (id, z, fit)) in f.top.iter().enumerate() {
            s.push_str(&format!("  #{:<2} bot {:<5} z={:.2}  fitness {:.3}\n", rank + 1, id, z, fit));
        }
        self.board.set_text(&s);
    }
}
