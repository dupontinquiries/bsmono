//! bsweb-egui — the parts web on a phone.
//!
//! ## The target is a small desktop, not a touch device
//!
//! The Fairphone runs a full Linux desktop with a mouse and keyboard. That
//! is a *small screen*, not a *finger*, and the two want opposite things:
//!
//! * **Pointer sizing, not touch.** `Adaptive::detect` infers a 44pt minimum
//!   target from a phone-width surface. On 44pt rows a 1080p-class panel
//!   shows a dozen parts out of 1,790. Forced to `with_touch(false)` and a
//!   28pt target, it shows three times as many, and a mouse hits them fine.
//! * **Two panes, always.** The compact single-pane pattern exists because a
//!   thumb cannot work a 320px side panel. A cursor can. So list and detail
//!   sit side by side at every width; only the split ratio moves.
//! * **Keyboard first.** `/` filters, `j`/`k` and arrows move, `Enter` opens,
//!   `x` removes, `u` undoes. A nomad machine is used one-handed on a train
//!   far more than it is used with a trackpad.
//!
//! ## It edits, and the edits are undoable
//!
//! The earlier version of this file staged removals and exported a shell
//! command for a desktop to run, on the theory that a phone save was unsafe.
//! That was the wrong premise: this *is* a desktop, `Web::write` is already
//! temp-then-rename, and a nomad is precisely the person who cannot go
//! re-run the pipeline somewhere else.
//!
//! So it mutates, through `bsweb::Journal` — every edit an invertible value,
//! the same bargain `aperture-core::op` makes. Unsaved depth is shown, undo
//! is unbounded within a session, and `gc` stays off this surface because it
//! renumbers geometry and is the one operation an index-level undo cannot
//! reverse.

use bsweb::{Journal, Web};
use egui::{RichText, Ui};
use uikit_egui::adaptive::Adaptive;
use uikit_egui::theme_bridge::PaletteColors;
use uikit_egui::{carved, fuzzy, skin::Skin};

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Which column the list is sorted by. Triage is mostly "show me the biggest
/// thing I do not need", so triangle count leads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Tris,
    Instances,
    Residual,
    Name,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Tris, Sort::Instances, Sort::Residual, Sort::Name];
    pub fn label(self) -> &'static str {
        match self {
            Sort::Tris => "tris",
            Sort::Instances => "reuse",
            Sort::Residual => "fit",
            Sort::Name => "kind",
        }
    }
}

pub struct App {
    pub path: PathBuf,
    pub web: Option<Web>,
    pub error: Option<String>,

    pub query: String,
    pub kind: Option<String>,
    pub sort: Sort,
    pub shared_only: bool,

    /// Rows that survived the current filter, as indices into `web.index.parts`.
    filtered: Vec<usize>,
    dirty: bool,

    pub selected: Option<usize>,
    /// Keyboard cursor into `filtered` — distinct from `selected`, which is
    /// what the detail pane shows. Moving is not opening.
    pub cursor: usize,
    /// Selected for the next action, held as `catalog/id` keys — never bare
    /// ids, because 195 ids in the real catalog name two different meshes.
    pub marked: BTreeSet<String>,
    pub journal: Journal,
    /// Edits since the last write. Shown, because an unsaved web on a
    /// travelling machine is the thing you want to know about.
    pub unsaved: usize,
    pub status: String,

    pub colors: PaletteColors,
    pub skin: Skin,
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let mut app = App {
            path,
            web: None,
            error: None,
            query: String::new(),
            kind: None,
            sort: Sort::Tris,
            shared_only: false,
            filtered: Vec::new(),
            dirty: true,
            selected: None,
            cursor: 0,
            marked: BTreeSet::new(),
            journal: Journal::new(),
            unsaved: 0,
            status: String::new(),
            colors: uikit_egui::default_palette(),
            skin: Skin::default(),
        };
        app.load();
        app
    }

    pub fn load(&mut self) {
        match Web::open(&self.path) {
            Ok(w) => {
                self.web = Some(w);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.web = None;
            }
        }
        self.dirty = true;
    }

    /// Distinct part kinds, most common first — the filter chips.
    pub fn kinds(&self) -> Vec<(String, usize)> {
        let Some(w) = &self.web else { return Vec::new() };
        let mut m = std::collections::BTreeMap::<&str, usize>::new();
        for p in &w.index.parts {
            *m.entry(p.name.as_deref().unwrap_or("(none)")).or_default() += 1;
        }
        let mut v: Vec<(String, usize)> =
            m.into_iter().map(|(k, c)| (k.to_string(), c)).collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        v
    }

    /// Recompute the visible rows. Cheap enough on 1,790 parts to run on any
    /// change, but gated on `dirty` so it does not run every frame — at 60 fps
    /// a fuzzy score over every row is the one thing here that could actually
    /// cost a phone battery.
    fn refilter(&mut self) {
        let Some(w) = &self.web else { return };
        let rc = w.refcounts();
        let q = self.query.trim().to_lowercase();
        let mut rows: Vec<(i32, usize)> = Vec::new();
        for (i, p) in w.index.parts.iter().enumerate() {
            if let Some(k) = &self.kind {
                if p.name.as_deref() != Some(k.as_str()) {
                    continue;
                }
            }
            if self.shared_only && rc.get(p.geom).copied().unwrap_or(0) < 2 {
                continue;
            }
            let score = if q.is_empty() {
                0
            } else {
                let hay = format!(
                    "{} {} {}",
                    p.id,
                    p.name.as_deref().unwrap_or(""),
                    p.from.as_deref().unwrap_or("")
                )
                .to_lowercase();
                match fuzzy::score(&q, &hay) {
                    Some(s) => s,
                    None => continue,
                }
            };
            rows.push((score, i));
        }
        let parts = &w.index.parts;
        match self.sort {
            Sort::Tris => rows.sort_by_key(|(_, i)| std::cmp::Reverse(parts[*i].nt)),
            Sort::Instances => {
                rows.sort_by_key(|(_, i)| std::cmp::Reverse(parts[*i].instances.unwrap_or(0)))
            }
            Sort::Residual => rows.sort_by(|a, b| {
                parts[a.1]
                    .residual_mean
                    .unwrap_or(f64::MAX)
                    .partial_cmp(&parts[b.1].residual_mean.unwrap_or(f64::MAX))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            Sort::Name => rows.sort_by(|a, b| parts[a.1].name.cmp(&parts[b.1].name)),
        }
        // a live query outranks the column sort, or typing feels broken
        if !q.is_empty() {
            rows.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        }
        self.filtered = rows.into_iter().map(|(_, i)| i).collect();
        self.dirty = false;
    }

    pub fn visible(&self) -> &[usize] {
        &self.filtered
    }

    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// The command to run on the desktop. This is the whole output of a
    /// triage session.
    pub fn remove_marked(&mut self) {
        let Some(w) = self.web.as_mut() else { return };
        let n = w.remove_journaled(&self.marked.iter().cloned().collect(), &mut self.journal);
        self.marked.clear();
        self.unsaved += usize::from(n > 0);
        self.selected = None;
        self.dirty = true;
        self.status = format!("removed {n}");
    }

    pub fn undo(&mut self) {
        let Some(w) = self.web.as_mut() else { return };
        match w.undo(&mut self.journal) {
            Some(msg) => { self.unsaved += 1; self.dirty = true; self.status = msg; }
            None => self.status = "nothing to undo".into(),
        }
    }

    pub fn save(&mut self) {
        let Some(w) = self.web.as_ref() else { return };
        let codec = w.codec;
        match w.write(&self.path, codec) {
            Ok(n) => { self.unsaved = 0; self.status = format!("saved, {} KB", n / 1024); }
            Err(e) => self.status = format!("SAVE FAILED: {e}"),
        }
    }

    // ------------------------------------------------------------ drawing

    /// Draw the whole app inside a `Ui`.
    ///
    /// Takes a `Ui` rather than a `Context` because egui 0.34.3 deprecated
    /// top-level `Panel::show(ctx, …)` in favour of `show_inside(ui, …)` —
    /// the shell bridges once via `Context::run_ui`. It also makes this
    /// embeddable: a launcher can drop the whole view into one tab.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // Pointer, not finger — see the module note. Adaptive still classifies
        // the SIZE, which is what drives the split ratio below.
        let a = Adaptive::detect(&ctx).with_touch(false).with_min_touch_target(28.0);
        uikit_egui::adaptive::apply_text_floor(&ctx, &a);
        self.keys(&ctx);

        if self.dirty {
            self.refilter();
        }

        egui::Panel::top("filters").show_inside(ui, |ui| self.filter_row(ui, &a));
        egui::Panel::bottom("status").show_inside(ui, |ui| self.status_bar(ui));

        // Two panes at every width. Only the split moves: a cursor can work a
        // 280px column, so there is no reason to hide one on a small screen.
        if self.selected.is_some() {
            let w = a.pick(280.0f32, 340.0, 420.0);
            egui::Panel::right("detail").min_size(w).show_inside(ui, |ui| self.detail(ui, false));
        }
        self.list(ui, &a);
    }

    /// Keyboard is the primary input on this machine.
    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;                       // the filter box owns the keys
        }
        let (dn, up, open, mark, undo, save, del);
        {
            use egui::Key;
            let i = ctx.input(|i| (
                i.key_pressed(Key::J) || i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::K) || i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::X) || i.key_pressed(Key::Space),
                i.key_pressed(Key::U),
                i.key_pressed(Key::S) && i.modifiers.ctrl,
                i.key_pressed(Key::Delete),
            ));
            dn = i.0; up = i.1; open = i.2; mark = i.3; undo = i.4; save = i.5; del = i.6;
        }
        let n = self.filtered.len();
        if n > 0 && (dn || up) {
            let cur = self.cursor.min(n - 1);
            self.cursor = if dn { (cur + 1).min(n - 1) } else { cur.saturating_sub(1) };
        }
        if open && self.cursor < n { self.selected = Some(self.filtered[self.cursor]); }
        if mark && self.cursor < n {
            if let Some(w) = &self.web {
                let k = Web::key(&w.index.parts[self.filtered[self.cursor]]);
                if !self.marked.remove(&k) { self.marked.insert(k); }
            }
        }
        if undo { self.undo(); }
        if save { self.save(); }
        if del && !self.marked.is_empty() { self.remove_marked(); }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let c = self.colors;
        ui.horizontal(|ui| {
            if self.unsaved > 0 {
                ui.label(RichText::new(format!("● {} unsaved", self.unsaved))
                    .color(c.warning).strong());
                if carved::button(ui, "save  ^S", c.bg_card, c.accent, c.text, &self.skin).clicked() {
                    self.save();
                }
            } else {
                ui.label(RichText::new("saved").color(c.text_weak).small());
            }
            if !self.marked.is_empty() {
                ui.label(RichText::new(format!("{} marked", self.marked.len())).color(c.accent));
                if carved::button(ui, "remove  Del", c.bg_card, c.danger, c.text, &self.skin).clicked() {
                    self.remove_marked();
                }
            }
            if self.journal.can_undo() {
                let lbl = self.journal.describe().unwrap_or_default();
                if carved::button(ui, &format!("undo {lbl}  u"), c.bg_card, c.accent, c.text, &self.skin)
                    .clicked() { self.undo(); }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(&self.status).color(c.text_weak).small());
                ui.label(RichText::new("j/k move · Enter open · x mark · u undo")
                    .color(c.text_weak).small());
            });
        });
    }

    fn filter_row(&mut self, ui: &mut Ui, a: &Adaptive) {
        let c = self.colors;
        if let Some(e) = &self.error {
            ui.colored_label(c.danger, format!("cannot read web: {e}"));
            return;
        }
        ui.add_space(a.gutter * 0.5);
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("filter parts…")
                    .desired_width(ui.available_width() - 90.0),
            );
            if r.changed() {
                self.dirty = true;
            }
            if carved::button(ui, "clear", c.bg_card, c.accent, c.text, &self.skin).clicked() {
                self.query.clear();
                self.kind = None;
                self.shared_only = false;
                self.dirty = true;
            }
        });

        ui.horizontal_wrapped(|ui| {
            for s in Sort::ALL {
                let on = self.sort == s;
                if carved::pill(ui, s.label(), c.bg_card, c.accent, c.text, &self.skin, on)
                    .clicked()
                {
                    self.sort = s;
                    self.dirty = true;
                }
            }
            let on = self.shared_only;
            if carved::pill(ui, "shared", c.bg_card, c.accent, c.text, &self.skin, on).clicked() {
                self.shared_only = !self.shared_only;
                self.dirty = true;
            }
        });

        // kind chips: fewer on a phone, because a wrapped row of 40 chips is
        // the whole screen
        let max = a.pick(6usize, 12, 20);
        let kinds = self.kinds();
        ui.horizontal_wrapped(|ui| {
            for (k, n) in kinds.iter().take(max) {
                let on = self.kind.as_deref() == Some(k.as_str());
                if carved::pill(
                    ui,
                    &format!("{k} {n}"),
                    c.bg_card,
                    c.accent,
                    c.text,
                    &self.skin,
                    on,
                )
                .clicked()
                {
                    self.kind = if on { None } else { Some(k.clone()) };
                    self.dirty = true;
                }
            }
        });
        ui.add_space(a.gutter * 0.5);
        ui.label(
            RichText::new(format!("{} parts", self.filtered.len()))
                .color(c.text_weak)
                .small(),
        );
    }

    fn list(&mut self, ui: &mut Ui, a: &Adaptive) {
        let Some(w) = &self.web else { return };
        let c = self.colors;
        let row_h = a.min_touch_target.max(44.0);
        let rc = w.refcounts();
        let rows: Vec<usize> = self.filtered.clone();
        let mut clicked = None;
        let mut toggled = None;

        egui::ScrollArea::vertical().auto_shrink([false; 2]).show_rows(
            ui,
            row_h,
            rows.len(),
            |ui, range| {
                for r in range {
                    let i = rows[r];
                    let p = &w.index.parts[i];
                    let key = Web::key(p);
                    let is_staged = self.marked.contains(&key);
                    let shared = rc.get(p.geom).copied().unwrap_or(0) > 1;

                    ui.horizontal(|ui| {
                        ui.set_min_height(row_h);
                        let mark = if is_staged { "✕" } else { "·" };
                        if carved::button(
                            ui,
                            mark,
                            if is_staged { c.danger } else { c.bg_card },
                            c.accent,
                            c.text,
                            &self.skin,
                        )
                        .clicked()
                        {
                            toggled = Some(key.clone());
                        }
                        let tint = if is_staged { c.text_weak } else { c.text };
                        let label = format!(
                            "{}  {}",
                            p.name.as_deref().unwrap_or("?"),
                            short(&p.id)
                        );
                        if ui
                            .add(egui::Label::new(RichText::new(label).color(tint)).sense(
                                egui::Sense::click(),
                            ))
                            .clicked()
                        {
                            clicked = Some(i);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if shared {
                                carved::chip(ui, "shared", c.bg_base, c.accent_soft, c.border);
                            }
                            ui.label(
                                RichText::new(format!("{} tris", p.nt))
                                    .color(c.text_weak)
                                    .small(),
                            );
                        });
                    });
                }
            },
        );
        if let Some(k) = toggled {
            if !self.marked.remove(&k) {
                self.marked.insert(k);
            }
        }
        if let Some(i) = clicked {
            self.selected = Some(i);
        }
    }

    fn detail(&mut self, ui: &mut Ui, compact: bool) {
        let Some(w) = &self.web else { return };
        let Some(i) = self.selected else { return };
        let Some(p) = w.index.parts.get(i) else { return };
        let c = self.colors;

        if compact
            && carved::button(ui, "‹ back", c.bg_card, c.accent, c.text, &self.skin).clicked()
        {
            self.selected = None;
            return;
        }
        ui.heading(RichText::new(Web::key(p)).color(c.text));
        ui.add_space(6.0);

        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            row(ui, c, "kind", p.name.as_deref().unwrap_or("-"));
            row(ui, c, "from", p.from.as_deref().unwrap_or("-"));
            row(ui, c, "catalog", p.catalog.as_deref().unwrap_or("-"));
            row(ui, c, "triangles", &p.nt.to_string());
            row(ui, c, "vertices", &p.nv.to_string());
            row(ui, c, "reuse", &opt(p.instances));
            row(ui, c, "residual mean", &optf(p.residual_mean, 4));
            row(ui, c, "residual max", &optf(p.residual_max, 4));
            row(ui, c, "size", &optf(p.size, 3));
            if p.span.len() == 3 {
                row(
                    ui,
                    c,
                    "extent",
                    &format!("{:.3} × {:.3} × {:.3}", p.span[0], p.span[1], p.span[2]),
                );
            }

            // Sharers are the thing a triage decision hinges on: removing a
            // part whose geometry three others use frees nothing.
            let sharers: Vec<String> = w
                .index
                .parts
                .iter()
                .filter(|q| q.geom == p.geom && Web::key(q) != Web::key(p))
                .map(Web::key)
                .collect();
            ui.add_space(8.0);
            if sharers.is_empty() {
                ui.label(
                    RichText::new("geometry is unique to this part — removing it frees payload")
                        .color(c.success)
                        .small(),
                );
            } else {
                ui.label(
                    RichText::new(format!(
                        "geometry shared with {} other part(s) — removing this frees nothing \
                         until they go too",
                        sharers.len()
                    ))
                    .color(c.warning)
                    .small(),
                );
                for s in sharers.iter().take(8) {
                    ui.label(RichText::new(format!("   {s}")).color(c.text_weak).small());
                }
            }

            ui.add_space(10.0);
            let key = Web::key(p);
            let on = self.marked.contains(&key);
            let text = if on { "unmark  x" } else { "mark for removal  x" };
            if carved::button_accent(ui, text, c.bg_card, c.danger, c.text, &self.skin).clicked() {
                if !self.marked.remove(&key) {
                    self.marked.insert(key);
                }
            }
        });
    }
}

fn short(id: &str) -> String {
    if id.len() <= 26 {
        id.to_string()
    } else {
        format!("{}…{}", &id[..14], &id[id.len() - 8..])
    }
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "-".into())
}

fn optf(v: Option<f64>, p: usize) -> String {
    v.map(|x| format!("{x:.p$}", p = p)).unwrap_or_else(|| "-".into())
}

fn row(ui: &mut Ui, c: PaletteColors, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{k:<15}")).color(c.text_weak).monospace());
        ui.label(RichText::new(v).color(c.text));
    });
}
