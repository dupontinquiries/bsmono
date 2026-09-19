use gtk::prelude::*;
use std::rc::Rc;
use stoat_core::vitals::{self, Shared};

pub struct VitalsPage {
    pub root: gtk::ScrolledWindow,
    summary: gtk::Label,
    areas: Vec<gtk::DrawingArea>,
}

#[derive(Clone, Copy)]
enum Series {
    Rss,
    Cpu,
    Frames,
}

impl VitalsPage {
    pub fn new(data: Shared) -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let summary = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).build();
        summary.add_css_class("caption");
        body.append(&summary);

        let data = Rc::new(data);
        let mut areas = Vec::new();
        for (title, series) in [("Memory (resident)", Series::Rss), ("CPU (% of one core)", Series::Cpu), ("Frame interval (ms, includes marker overhead)", Series::Frames)] {
            let label = gtk::Label::builder().label(title).xalign(0.0).margin_top(10).wrap(true).build();
            label.add_css_class("heading");
            let area = gtk::DrawingArea::builder().content_height(120).hexpand(true).build();
            area.add_css_class("card");
            let d = data.clone();
            area.set_draw_func(move |a, cr, w, h| draw(a, cr, w as f64, h as f64, &d, series));
            body.append(&label);
            body.append(&area);
            areas.push(area);
        }
        VitalsPage { root: super::scrolled_clamp(&body), summary, areas }
    }

    pub fn refresh(&self, data: &Shared) {
        let lines = vitals::summarize(&data.lock().unwrap());
        self.summary.set_text(&if lines.is_empty() { "No samples yet. Memory and CPU are sampled from /proc while the program runs; frame pacing needs a frame marker set on the Launch page.".to_string() } else { lines.join("\n") });
        for a in &self.areas {
            a.queue_draw();
        }
    }
}

fn draw(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64, h: f64, data: &Shared, series: Series) {
    let fg = area.color();
    let (r, g, b) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
    let pad = 8.0;
    let v = data.lock().unwrap();
    let (values, hitch_over): (Vec<f64>, Option<f64>) = match series {
        Series::Rss => (v.samples().iter().map(|s| s.rss_kb as f64 / 1024.0).collect(), None),
        Series::Cpu => (v.samples().iter().map(|s| s.cpu_pct as f64).collect(), None),
        Series::Frames => {
            let f = v.frames();
            (f.iter().map(|f| f.ms as f64).collect(), vitals::frame_stats(&f).map(|s| s.p50_ms as f64 * 1.5))
        }
    };
    drop(v);
    cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(10.0);
    cr.set_source_rgba(r, g, b, 0.7);
    if values.len() < 2 {
        cr.move_to(pad, h / 2.0);
        let _ = cr.show_text("no data");
        return;
    }
    let max = values.iter().cloned().fold(f64::MIN, f64::max).max(1e-6) * 1.1;
    let (gw, gh) = (w - 2.0 * pad, h - 2.0 * pad - 10.0);
    let x = |i: usize| pad + gw * i as f64 / (values.len() - 1) as f64;
    let y = |val: f64| pad + 10.0 + gh * (1.0 - val / max);

    match series {
        Series::Frames => {
            let bw = (gw / values.len() as f64).max(1.0);
            for (i, val) in values.iter().enumerate() {
                let hitch = hitch_over.is_some_and(|t| *val > t);
                if hitch {
                    cr.set_source_rgba(0.88, 0.11, 0.14, 0.9);
                } else {
                    cr.set_source_rgba(r, g, b, 0.45);
                }
                cr.rectangle(pad + bw * i as f64, y(*val), (bw - 1.0).max(1.0), pad + 10.0 + gh - y(*val));
                let _ = cr.fill();
            }
        }
        _ => {
            cr.set_source_rgba(0.21, 0.52, 0.89, 0.25);
            cr.move_to(x(0), y(0.0));
            for (i, val) in values.iter().enumerate() {
                cr.line_to(x(i), y(*val));
            }
            cr.line_to(x(values.len() - 1), y(0.0));
            let _ = cr.fill();
            cr.set_source_rgba(0.21, 0.52, 0.89, 1.0);
            cr.set_line_width(1.5);
            for (i, val) in values.iter().enumerate() {
                if i == 0 {
                    cr.move_to(x(i), y(*val));
                } else {
                    cr.line_to(x(i), y(*val));
                }
            }
            let _ = cr.stroke();
        }
    }
    cr.set_source_rgba(r, g, b, 0.7);
    let unit = match series {
        Series::Rss => "MiB",
        Series::Cpu => "%",
        Series::Frames => "ms",
    };
    cr.move_to(pad, pad + 8.0);
    let _ = cr.show_text(&format!("peak {:.1} {unit} · last {:.1} {unit} · stop is at the right edge", max / 1.1, values[values.len() - 1]));
}
