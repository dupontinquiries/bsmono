//! editor -- the scribe window: type scenes and lines on one side, the centred page on the other.
//!
//! Wide windows show both side by side; a narrow one (a phone) shows one at a time with a switch. The
//! preview uses the built-in faces (egui carries no serif or typewriter face), tinted with the style's
//! paper and ink so a vibe reads at a glance; the exported HTML uses the real font stacks.

use crate::html;
use crate::script::{self, Item, Script};
use crate::store::{self, Settings};
use crate::style::{Paper, STYLES};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Font stacks offered besides the style's own.
pub const FONTS: [(&str, &str); 8] = [
    ("Georgia", "Georgia, serif"),
    ("Times", "\"Times New Roman\", Times, serif"),
    ("Garamond", "Garamond, \"EB Garamond\", serif"),
    ("Palatino", "\"Palatino Linotype\", Palatino, serif"),
    ("Helvetica", "Helvetica, Arial, sans-serif"),
    ("Gill Sans", "\"Gill Sans\", sans-serif"),
    ("Courier", "\"Courier New\", Courier, monospace"),
    ("Typewriter", "\"American Typewriter\", \"Special Elite\", monospace"),
];

const SAMPLE: &str = "Title: UNTITLED\nAuthor: you\n\n# kitchen - night\nthe kettle starts to scream\nmara: I told you not to.\n(beat)\njo: You never said.\n===\n# roof\nwind. a long way down.\n";

/// Autosave this long after the last keystroke.
pub const AUTOSAVE: Duration = Duration::from_millis(1500);

pub struct Editor {
    pub path: Option<PathBuf>,
    pub text: String,
    saved: String,
    pub settings: Settings,
    edited_at: Option<Instant>,
    pub report: String,
    parsed: Option<(u64, Script)>,
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn color(hex: &str) -> egui::Color32 {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Editor {
    /// Open `path` (created empty-with-a-sample in memory if it does not exist yet), or a new unsaved script.
    pub fn open(path: Option<PathBuf>) -> Editor {
        // an existing file is clean as opened; a new one shows the sample and is saved when you type
        let (text, saved, report) = match &path {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => (t.clone(), t, format!("opened {}", p.display())),
                Err(_) => (SAMPLE.to_string(), String::new(), format!("new file {} (saved when you type)", p.display())),
            },
            None => (SAMPLE.to_string(), SAMPLE.to_string(), "new script: give it a name with `scribe-egui file.fountain` to keep it".to_string()),
        };
        Editor { path, saved, text, settings: Settings::load(), edited_at: None, report, parsed: None }
    }

    pub fn dirty(&self) -> bool { self.text != self.saved }

    pub fn script(&mut self) -> &Script {
        let h = hash(&self.text);
        if self.parsed.as_ref().is_none_or(|(k, _)| *k != h) {
            let name = self.path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str());
            self.parsed = Some((h, script::parse_any(name, &self.text)));
        }
        &self.parsed.as_ref().unwrap().1
    }

    pub fn save(&mut self) {
        let Some(p) = self.path.clone() else { self.report = "no file name: start with `scribe-egui file.fountain`".into(); return };
        match store::write_atomic(&p, &self.text) {
            Ok(()) => { self.saved = self.text.clone(); self.edited_at = None; self.report = format!("saved {}", p.display()); }
            Err(e) => self.report = format!("could not save {}: {e}", p.display()),
        }
    }

    /// Write the printable page beside the script and say where.
    pub fn export(&mut self) -> Option<PathBuf> {
        let out = match &self.path {
            Some(p) => p.with_extension("html"),
            None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Documents").join("scribe").join("untitled.html"),
        };
        let opts = self.settings.options.clone();
        let page = html::render(self.script(), &opts);
        match store::write_atomic(&out, &page) {
            Ok(()) => { self.report = format!("exported {} ({}); open it in a browser and print", out.display(), opts.style.name); Some(out) }
            Err(e) => { self.report = format!("could not export: {e}"); None }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            let name = self.path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("untitled");
            ui.strong(format!("{name}{}", if self.dirty() { " •" } else { "" }));
            if ui.button("save").clicked() { self.save(); }
            if ui.button("export html").clicked() { self.export(); }
            if ui.button("open page").on_hover_text("open the exported page in the browser").clicked() {
                if let Some(p) = self.export() { let _ = std::process::Command::new("xdg-open").arg(p).spawn(); }
            }
        });
        ui.horizontal_wrapped(|ui| {
            let o = &mut self.settings.options;
            egui::ComboBox::from_id_salt("scribe-style").selected_text(o.style.name).show_ui(ui, |ui| {
                for s in STYLES { if ui.selectable_label(o.style.name == s.name, format!("{} — {}", s.name, s.blurb)).clicked() { o.style = s; changed = true; } }
            });
            let cur = o.font.as_deref().and_then(|f| FONTS.iter().find(|(_, v)| *v == f)).map(|(n, _)| *n).unwrap_or("style's font");
            egui::ComboBox::from_id_salt("scribe-font").selected_text(cur).show_ui(ui, |ui| {
                if ui.selectable_label(o.font.is_none(), "style's font").clicked() { o.font = None; changed = true; }
                for (n, v) in FONTS { if ui.selectable_label(o.font.as_deref() == Some(v), n).clicked() { o.font = Some(v.to_string()); changed = true; } }
            });
            let mut size = o.size();
            if ui.add(egui::DragValue::new(&mut size).range(8.0..=24.0).speed(0.1).suffix(" pt")).changed() { o.size_pt = Some(size); changed = true; }
            let mut a4 = o.paper == Paper::A4;
            if ui.checkbox(&mut a4, "A4").changed() { o.paper = if a4 { Paper::A4 } else { Paper::Letter }; changed = true; }
            changed |= ui.checkbox(&mut o.scene_numbers, "scene numbers").changed();
        });
        if changed { self.settings.save(); }
    }

    /// The centred page.
    fn page(&mut self, ui: &mut egui::Ui) {
        let o = self.settings.options.clone();
        let (paper, ink, accent) = (color(o.style.paper), color(o.style.ink), color(o.style.accent));
        let mono = o.style.font.contains("monospace");
        let family = if mono { egui::FontFamily::Monospace } else { egui::FontFamily::Proportional };
        let size = (o.size() * 1.25).clamp(11.0, 30.0);
        let script = self.script().clone();
        egui::Frame::new().fill(paper).inner_margin(egui::Margin::symmetric(14, 18)).show(ui, |ui| {
            let width = ui.available_width();
            // the text column, centred in the page (never an indent)
            let col = (o.style.column_in * 96.0).min(width);
            ui.set_width(width);
            let mut centred = |ui: &mut egui::Ui, text: &str, sz: f32, c: egui::Color32, gap: f32| {
                let mut job = egui::text::LayoutJob::simple(text.to_string(), egui::FontId::new(sz, family.clone()), c, col);
                job.halign = egui::Align::Center;
                let galley = ui.painter().layout_job(job);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, galley.size().y + gap), egui::Sense::hover());
                ui.painter().galley(egui::pos2(rect.center().x, rect.top()), galley, c);
            };
            let m = &script.meta;
            if o.title_page && !m.is_empty() {
                ui.add_space(10.0);
                if let Some(t) = &m.title { centred(ui, &html::plain(t).to_uppercase(), size * 1.7, ink, 8.0); }
                if let Some(c) = &m.credit { centred(ui, &html::plain(c), size * 0.95, accent, 2.0); }
                if let Some(a) = &m.author { centred(ui, &html::plain(a), size, ink, 2.0); }
                if let Some(d) = &m.draft_date { centred(ui, d, size * 0.8, accent, 24.0); }
            }
            let mut scene_no = 0;
            for item in &script.items {
                match item {
                    Item::Scene(t) => {
                        scene_no += 1;
                        let t = html::plain(t);
                        let t = if o.style.scene.uppercase { t.to_uppercase() } else { t };
                        let t = if o.scene_numbers { format!("{scene_no}.  {t}") } else { t };
                        ui.add_space(size * 1.2);
                        centred(ui, &t, size, ink, 6.0);
                        let r = ui.cursor();
                        if o.style.scene.rule { ui.painter().hline(egui::Rangef::new(r.center().x - col * 0.3, r.center().x + col * 0.3), r.top() - 3.0, egui::Stroke::new(1.0_f32, accent)); }
                    }
                    Item::Line { speaker, text } => {
                        if let Some(n) = speaker {
                            ui.add_space(size * 0.6);
                            let n = html::plain(n);
                            centred(ui, &if o.style.speaker.uppercase { n.to_uppercase() } else { n }, size * 0.9, accent, 1.0);
                            centred(ui, &html::plain(text), size, ink, 4.0);
                        } else {
                            centred(ui, &html::plain(text), size, ink, size * 0.5);
                        }
                    }
                    Item::Direction(t) => centred(ui, &html::plain(t), size * 0.95, accent, size * 0.5),
                    Item::Section { text, .. } => centred(ui, &html::plain(text).to_uppercase(), size * 0.8, accent, 4.0),
                    Item::PageBreak => { ui.add_space(8.0); let r = ui.cursor(); ui.painter().hline(egui::Rangef::new(r.left() + 20.0, r.right() - 20.0), r.top(), egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.5))); ui.add_space(12.0); }
                }
            }
        });
    }

    fn typing(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("scribe-type").auto_shrink([false, false]).show(ui, |ui| {
            let r = ui.add(uikit_egui::text_field::edit_multi("scribe-text", &mut self.text)
                .font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(24).lock_focus(true)
                .hint_text("# a scene\na line\nname: a spoken line"));
            if r.changed() { self.edited_at = Some(Instant::now()); }
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let compact = uikit_egui::adaptive::Adaptive::detect(&ctx).class.is_compact();
        self.controls(ui);
        let mut view = self.settings.view.clone();
        if compact {
            // a phone: one at a time
            if view == "both" { view = "type".into(); }
            ui.horizontal(|ui| {
                for (k, label) in [("type", "type"), ("page", "page")] {
                    if ui.selectable_label(view == k, label).clicked() { view = k.into(); }
                }
            });
        } else {
            ui.horizontal(|ui| {
                for (k, label) in [("type", "type"), ("both", "both"), ("page", "page")] {
                    if ui.selectable_label(view == k, label).clicked() { view = k.into(); }
                }
            });
        }
        if view != self.settings.view { self.settings.view = view.clone(); self.settings.save(); }
        ui.separator();
        match view.as_str() {
            "type" => self.typing(ui),
            "page" => { egui::ScrollArea::vertical().id_salt("scribe-page").auto_shrink([false, false]).show(ui, |ui| self.page(ui)); }
            _ => {
                ui.columns(2, |cols| {
                    self.typing(&mut cols[0]);
                    egui::ScrollArea::vertical().id_salt("scribe-page").auto_shrink([false, false]).show(&mut cols[1], |ui| self.page(ui));
                });
            }
        }
        let script_line = { let s = self.script(); format!("{} scenes · {} lines · {} words", s.scenes(), s.lines(), s.words()) };
        ui.separator();
        ui.add(egui::Label::new(egui::RichText::new(format!("{script_line} — {}", self.report)).small().weak()).truncate());
        // autosave a moment after the last keystroke
        if let Some(t) = self.edited_at {
            if t.elapsed() >= AUTOSAVE && self.path.is_some() { self.save(); } else { ctx.request_repaint_after(AUTOSAVE); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ed: &mut Editor, size: egui::Vec2) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for frame in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(frame as f64 * 0.016),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| { egui::CentralPanel::default().show_inside(ui, |ui| ed.ui(ui)); });
            if frame == 2 { texts = uikit_egui::layout_probe::texts(&out.shapes).into_iter().map(|g| g.text).collect(); }
        }
        texts
    }

    fn editor(d: &std::path::Path, body: &str) -> Editor {
        let p = d.join("film.fountain");
        std::fs::write(&p, body).unwrap();
        let mut e = Editor::open(Some(p));
        e.settings = Settings::default();
        e
    }

    #[test]
    fn it_opens_a_file_and_shows_the_page_wide_and_narrow() {
        let d = tempfile::tempdir().unwrap();
        let mut e = editor(d.path(), "Title: HEAT\nAuthor: A\n\nINT. ROOM - DAY\n\nMARA\nWait here.\n\nShe waits.\n");
        assert!(!e.dirty() && e.report.starts_with("opened"));
        e.settings.view = "page".into();
        for size in [egui::vec2(900.0, 700.0), egui::vec2(360.0, 720.0)] {
            let texts = run(&mut e, size);
            let all = texts.join("|");
            for want in ["HEAT", "INT. ROOM - DAY", "MARA", "Wait here.", "She waits."] {
                assert!(all.contains(want), "{size:?}: `{want}` not drawn in {all}");
            }
        }
        assert_eq!((e.script().scenes(), e.script().lines()), (1, 2));
    }

    #[test]
    fn typing_autosaves_and_a_phone_gets_one_pane_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let mut e = editor(d.path(), "# a\nx\n");
        e.text.push_str("y\n");
        assert!(e.dirty());
        e.edited_at = Some(Instant::now() - AUTOSAVE - Duration::from_millis(10));
        e.settings.view = "both".into();
        let narrow = run(&mut e, egui::vec2(360.0, 720.0)).join("|");
        assert!(!e.dirty(), "autosaved after the pause");
        assert_eq!(std::fs::read_to_string(d.path().join("film.fountain")).unwrap(), "# a\nx\ny\n");
        assert!(narrow.contains("type") && narrow.contains("page"), "the switch is there: {narrow}");
    }

    #[test]
    fn export_writes_a_centred_page_beside_the_script() {
        let d = tempfile::tempdir().unwrap();
        let mut e = editor(d.path(), "# a scene\nmara: hello\n");
        let out = e.export().expect("exported");
        assert_eq!(out, d.path().join("film.html"));
        let h = std::fs::read_to_string(out).unwrap();
        assert!(h.contains("text-align:center") && h.contains("a scene") && h.contains("hello"));
        e.settings.options.style = crate::style::find("marquee").unwrap();
        e.export();
        assert!(std::fs::read_to_string(d.path().join("film.html")).unwrap().contains("Playfair"), "the chosen style is what is exported");
    }
}
