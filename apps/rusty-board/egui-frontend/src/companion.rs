//! companion — the `rusty-board-settings` app's screens, egui only.
//!
//! Two pages: every keyboard setting (the whole `settings_model`, not the
//! handful the in-keyboard panel has room for), and custom layouts (list,
//! copy a built-in, edit, import/export JSON, choose which one the keyboard
//! shows). The page logic is here and has no eframe in it; the binary is a
//! window around [`Companion::ui`].
//!
//! All disk and socket access goes through [`Backend`], so the screens run in
//! tests against memory ([`MemBackend`]) and on the device against the real
//! files ([`FsBackend`]): `settings.conf`, the user layouts directory, and a
//! `reload-*` datagram to the running keyboard after each write.
//!
//! The models are `settings_store` / `settings_model` and `layout_edit`; this
//! file only turns taps into calls on them.

use crate::layout_edit::{self, ActionChoice, Editor};
use crate::settings_model::{self, Group, Kind, Setting};
use crate::settings_store::{self, USER_LAYOUT_KEY};
use board_layouts::io::UserLayoutEntry;
use board_layouts::Layout;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Everything the companion reads and writes.
pub trait Backend {
    fn load_settings(&self) -> BTreeMap<String, String>;
    fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String>;
    fn list_layouts(&self) -> Vec<UserLayoutEntry>;
    fn load_layout(&self, path: &Path) -> Result<Layout, String>;
    fn save_layout(&mut self, layout: &Layout) -> Result<PathBuf, String>;
    fn delete_layout(&mut self, path: &Path) -> Result<(), String>;
    /// Tell the running keyboard. `Err` means no keyboard is listening.
    fn notify(&mut self, cmd: &str) -> Result<(), String>;
    /// Installed font families, for the font picker.
    fn fonts(&self) -> Vec<crate::user_font::Family>;
}

/// The device: real files, real socket.
pub struct FsBackend;

impl Backend for FsBackend {
    fn load_settings(&self) -> BTreeMap<String, String> {
        settings_store::load()
    }
    fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        settings_store::set(key, value).map(|_| ())
    }
    fn list_layouts(&self) -> Vec<UserLayoutEntry> {
        board_layouts::io::list_user_layouts()
    }
    fn load_layout(&self, path: &Path) -> Result<Layout, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        board_layouts::io::import_from_string(&t).map_err(|e| e.to_string())
    }
    fn save_layout(&mut self, layout: &Layout) -> Result<PathBuf, String> {
        board_layouts::io::save_user_layout(layout).map_err(|e| format!("{e:#}"))
    }
    fn delete_layout(&mut self, path: &Path) -> Result<(), String> {
        board_layouts::io::delete_user_layout(path).map_err(|e| format!("{e:#}"))
    }
    fn notify(&mut self, cmd: &str) -> Result<(), String> {
        crate::ctl::send(cmd)
    }
    fn fonts(&self) -> Vec<crate::user_font::Family> {
        crate::user_font::installed_families()
    }
}

/// Tests: everything in memory, with a record of what was sent.
#[derive(Default)]
pub struct MemBackend {
    pub settings: BTreeMap<String, String>,
    pub layouts: BTreeMap<PathBuf, Layout>,
    pub sent: Vec<String>,
    pub keyboard_running: bool,
}

impl Backend for MemBackend {
    fn load_settings(&self) -> BTreeMap<String, String> {
        self.settings.clone()
    }
    fn set_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.settings.insert(key.into(), value.into());
        Ok(())
    }
    fn list_layouts(&self) -> Vec<UserLayoutEntry> {
        self.layouts
            .iter()
            .map(|(p, l)| UserLayoutEntry { path: p.clone(), name: l.name.clone(), display_name: l.display_name.clone(), error: None })
            .collect()
    }
    fn load_layout(&self, path: &Path) -> Result<Layout, String> {
        self.layouts.get(path).cloned().ok_or_else(|| "gone".into())
    }
    fn save_layout(&mut self, layout: &Layout) -> Result<PathBuf, String> {
        board_layouts::io::validate(layout).map_err(|e| e.to_string())?;
        let p = PathBuf::from(format!("/mem/{}.json", board_layouts::io::slugify(&layout.name)));
        self.layouts.insert(p.clone(), layout.clone());
        Ok(p)
    }
    fn delete_layout(&mut self, path: &Path) -> Result<(), String> {
        self.layouts.remove(path).map(|_| ()).ok_or_else(|| "gone".into())
    }
    fn notify(&mut self, cmd: &str) -> Result<(), String> {
        self.sent.push(cmd.into());
        if self.keyboard_running { Ok(()) } else { Err("not running".into()) }
    }
    fn fonts(&self) -> Vec<crate::user_font::Family> {
        vec![]
    }
}

/// The compiled-in boards a user layout can start from.
pub fn builtins() -> Vec<Layout> {
    [crate::real_layout::QWERTY_JSON, crate::real_layout::DVORAK_ORTHO_JSON, crate::real_layout::SYMBOLS_JSON]
        .iter()
        .filter_map(|j| board_layouts::io::import_from_string(j).ok())
        .collect()
}

/// The slug `layout.user` stores for a layout: its file stem.
pub fn slug_of(l: &Layout) -> String {
    board_layouts::io::slugify(&l.name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Settings,
    Layouts,
}

/// What the layouts page shows.
pub enum LayoutView {
    List,
    Edit(Box<EditState>),
    Import(String),
}

/// An open editor plus the text boxes' buffers (egui edits a `String`; the
/// model gets the value when it changes).
pub struct EditState {
    pub ed: Editor,
    name: String,
    label: String,
    shifted: String,
    extended: String,
    value: String,
    confirm_discard: bool,
    /// The file this was opened from, when it was one (a rename saves to a
    /// new file; the old one is removed so there are not two copies).
    opened_from: Option<PathBuf>,
}

impl EditState {
    pub fn new(ed: Editor, opened_from: Option<PathBuf>) -> EditState {
        let name = ed.layout.display_name.clone();
        let mut s = EditState {
            ed,
            name,
            label: String::new(),
            shifted: String::new(),
            extended: String::new(),
            value: String::new(),
            confirm_discard: false,
            opened_from,
        };
        s.sync_buffers();
        s
    }

    /// Copy the selected key into the text boxes (after a selection change).
    fn sync_buffers(&mut self) {
        let k = self.ed.selected().cloned();
        self.label = k.as_ref().map(|k| k.label.clone()).unwrap_or_default();
        self.shifted = k.as_ref().and_then(|k| k.shifted.clone()).unwrap_or_default();
        self.extended = self.ed.extended_text();
        self.value = self.ed.action_value();
    }
}

pub struct Companion<B: Backend> {
    pub backend: B,
    pub page: Page,
    pub view: LayoutView,
    values: BTreeMap<String, String>,
    /// Text/secret/colour boxes, committed on Enter or focus loss.
    text_buf: BTreeMap<&'static str, String>,
    layouts: Vec<UserLayoutEntry>,
    confirm_delete: Option<PathBuf>,
    confirm_reset: bool,
    /// The last thing that happened, and whether it was a failure.
    pub status: Option<(String, bool)>,
    /// The font picker: open, its filter, and the families (read once, on open).
    font_picker: Option<(String, Vec<crate::user_font::Family>)>,
    /// The `look.font` installed into THIS window, so the app previews it.
    font_spec: Option<String>,
}

impl<B: Backend> Companion<B> {
    pub fn new(backend: B) -> Self {
        let mut c = Companion {
            backend,
            page: Page::Settings,
            view: LayoutView::List,
            values: BTreeMap::new(),
            text_buf: BTreeMap::new(),
            layouts: Vec::new(),
            confirm_delete: None,
            confirm_reset: false,
            font_picker: None,
            font_spec: None,
            status: None,
        };
        c.reload();
        c
    }

    /// Re-read disk. On start and after every write, so the screen never
    /// shows a value the file does not hold.
    pub fn reload(&mut self) {
        self.values = self.backend.load_settings();
        self.layouts = self.backend.list_layouts();
        self.text_buf.clear();
        for s in settings_model::SETTINGS.iter().filter(|s| s.kind != Kind::Choice) {
            self.text_buf.insert(s.key, self.current(s));
        }
    }

    /// The stored value, or the first option (the default).
    pub fn current(&self, s: &Setting) -> String {
        settings_store::valid_values(&self.values).get(s.key).cloned().unwrap_or_else(|| s.values[0].to_string())
    }

    pub fn user_layout(&self) -> String {
        self.values.get(USER_LAYOUT_KEY).cloned().unwrap_or_default()
    }

    /// Write one setting and tell the keyboard. The status line says which of
    /// the two happened.
    pub fn set(&mut self, key: &str, value: &str) {
        match self.backend.set_setting(key, value) {
            Ok(()) => {
                let told = self.backend.notify("reload-settings").is_ok();
                self.say_saved(told);
            }
            Err(e) => self.status = Some((format!("Not saved: {e}"), true)),
        }
        self.reload();
    }

    fn say_saved(&mut self, told: bool) {
        self.status = Some((
            if told { "Saved; the keyboard has it.".into() } else { "Saved. The keyboard is not running; it loads this when it starts.".into() },
            false,
        ));
    }

    /// Save the open editor's layout. `and_use` also makes it the keyboard's.
    pub fn save_editor(&mut self, and_use: bool) {
        let LayoutView::Edit(st) = &mut self.view else { return };
        // A row wider than `grid_columns` fails validation; widen the grid
        // to the widest row rather than refusing (never narrows it).
        let widest = st.ed.layout.rows.iter().map(|r| r.keys.iter().map(|k| k.width).sum::<f32>() + r.offset).fold(0.0, f32::max);
        if widest > st.ed.layout.grid_columns {
            st.ed.fit_columns();
        }
        if let Err(e) = st.ed.validate() {
            self.status = Some((format!("Not saved: {e}"), true));
            return;
        }
        let saved = match self.backend.save_layout(&st.ed.layout) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some((format!("Not saved: {e}"), true));
                return;
            }
        };
        st.ed.dirty = false;
        let slug = slug_of(&st.ed.layout);
        // Renamed: the old file would be a second, stale copy.
        let mut renamed_from = None;
        if let Some(old) = st.opened_from.replace(saved.clone()) {
            if old != saved {
                let _ = self.backend.delete_layout(&old);
                renamed_from = old.file_stem().and_then(|s| s.to_str()).map(str::to_string);
            }
        }
        let in_use = self.user_layout();
        if and_use || renamed_from.as_deref() == Some(in_use.as_str()) {
            let _ = self.backend.set_setting(USER_LAYOUT_KEY, &slug);
        }
        let told = self.backend.notify("reload-layouts").is_ok();
        self.say_saved(told);
        self.reload();
    }

    /// Make a saved layout the keyboard's (empty slug: the built-in board).
    pub fn use_layout(&mut self, slug: &str) {
        self.set(USER_LAYOUT_KEY, slug);
    }

    pub fn delete_layout(&mut self, path: &Path) {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        match self.backend.delete_layout(path) {
            Ok(()) => {
                if stem == self.user_layout() {
                    let _ = self.backend.set_setting(USER_LAYOUT_KEY, "");
                }
                let told = self.backend.notify("reload-layouts").is_ok();
                self.status = Some((
                    if told { "Deleted.".into() } else { "Deleted (keyboard not running).".into() },
                    false,
                ));
            }
            Err(e) => self.status = Some((format!("Not deleted: {e}"), true)),
        }
        self.reload();
    }

    /// Open an editor on a copy of `l` (a built-in, an import, or "copy of").
    pub fn edit_copy(&mut self, l: &Layout, name: &str) {
        self.view = LayoutView::Edit(Box::new(EditState::new(Editor::from_template(l, name), None)));
    }

    pub fn edit_file(&mut self, path: &Path) {
        match self.backend.load_layout(path) {
            Ok(l) => self.view = LayoutView::Edit(Box::new(EditState::new(Editor::new(l), Some(path.to_path_buf())))),
            Err(e) => self.status = Some((format!("Cannot open: {e}"), true)),
        }
    }

    // ---------------------------------------------------------------- UI

    pub fn ui(&mut self, ctx: &egui::Context) {
        // The companion draws in the chosen key font too: the preview.
        let want = self.values.get(crate::user_font::KEY).cloned().unwrap_or_default();
        if self.font_spec.as_deref() != Some(want.as_str()) {
            if let Err(e) = crate::user_font::apply(ctx, &want) {
                self.status = Some((format!("Font not loaded: {e}"), true));
            }
            self.font_spec = Some(want);
        }
        egui::Panel::top("tabs").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                for (p, name) in [(Page::Settings, "Settings"), (Page::Layouts, "Layouts")] {
                    let b = sel_button(self.page == p, egui::RichText::new(name).size(18.0))
                        .min_size(egui::vec2(w, 44.0));
                    if ui.add(b).clicked() {
                        self.page = p;
                        self.confirm_delete = None;
                        self.confirm_reset = false;
                    }
                }
            });
            ui.add_space(4.0);
        });
        if let Some((msg, err)) = self.status.clone() {
            egui::Panel::bottom("status").show(ctx, |ui| {
                ui.add_space(4.0);
                let t = egui::RichText::new(msg);
                ui.label(if err { t.color(ui.visuals().error_fg_color) } else { t.weak() });
                ui.add_space(4.0);
            });
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Settings => self.settings_page(ui),
            Page::Layouts => self.layouts_page(ui),
        });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for g in Group::ALL {
                let items = settings_model::group(g);
                if items.is_empty() {
                    continue;
                }
                ui.add_space(10.0);
                ui.heading(g.title());
                ui.separator();
                for s in items {
                    self.setting_row(ui, s);
                    ui.add_space(8.0);
                }
            }
            ui.add_space(16.0);
            let reset = if self.confirm_reset { "Tap again to reset every setting" } else { "Reset all settings" };
            if ui.add(egui::Button::new(reset).min_size(egui::vec2(0.0, 44.0))).clicked() {
                if self.confirm_reset {
                    self.reset_all();
                } else {
                    self.confirm_reset = true;
                }
            }
            ui.add_space(24.0);
        });
    }

    /// Every setting back to its default; the chosen user layout is kept
    /// (it is a layout choice, not a setting, and losing it is surprising).
    fn reset_all(&mut self) {
        self.confirm_reset = false;
        for s in settings_model::SETTINGS {
            if self.values.contains_key(s.key) {
                let _ = self.backend.set_setting(s.key, s.values[0]);
            }
        }
        let told = self.backend.notify("reload-settings").is_ok();
        self.say_saved(told);
        self.reload();
    }

    fn setting_row(&mut self, ui: &mut egui::Ui, s: &'static Setting) {
        ui.label(egui::RichText::new(s.label).strong().size(16.0));
        ui.label(egui::RichText::new(s.help).weak());
        let cur = self.current(s);
        match s.kind {
            Kind::Font => self.font_row(ui, s, &cur),
            Kind::Choice => {
                let mut pick = None;
                ui.horizontal_wrapped(|ui| {
                    for (opt, val) in s.options.iter().zip(s.values) {
                        let b = sel_button(cur == *val, *opt).min_size(egui::vec2(56.0, 40.0));
                        if ui.add(b).clicked() && cur != *val {
                            pick = Some(*val);
                        }
                    }
                });
                if let Some(v) = pick {
                    self.set(s.key, v);
                }
            }
            Kind::Text | Kind::Secret | Kind::Color => {
                let buf = self.text_buf.entry(s.key).or_insert_with(|| cur.clone());
                let mut commit = false;
                ui.horizontal(|ui| {
                    if s.kind == Kind::Color {
                        if let Some(c) = parse_hex(buf) {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 6.0, c);
                        }
                    }
                    let te = egui::TextEdit::singleline(buf)
                        .password(s.kind == Kind::Secret)
                        .hint_text(match s.kind {
                            Kind::Color => "#rrggbb, or empty to follow the theme",
                            Kind::Secret => "unset",
                            _ => "",
                        })
                        .desired_width(f32::INFINITY);
                    let r = ui.add(te);
                    commit = r.lost_focus() && *buf != cur;
                });
                if commit {
                    let v = buf.trim().to_string();
                    if s.kind == Kind::Color && !v.is_empty() && parse_hex(&v).is_none() {
                        self.status = Some(("A colour is #rrggbb (or empty).".into(), true));
                    } else {
                        self.set(s.key, &v);
                    }
                }
            }
        }
    }

    /// The font picker: current face, Built-in, Choose… (a filterable list of
    /// installed families), or a typed family name / file path.
    fn font_row(&mut self, ui: &mut egui::Ui, s: &'static Setting, cur: &str) {
        let big = |t: &str| egui::Button::new(t.to_string()).min_size(egui::vec2(0.0, 40.0));
        ui.label(egui::RichText::new(if cur.is_empty() { "Built-in" } else { cur }).size(18.0));
        ui.label(egui::RichText::new("The quick brown fox jumps over the lazy dog 0123456789").size(18.0));
        let mut pick: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            if !cur.is_empty() && ui.add(big("Built-in")).clicked() {
                pick = Some(String::new());
            }
            let open = self.font_picker.is_some();
            if ui.add(big(if open { "Close list" } else { "Choose…" })).clicked() {
                self.font_picker = if open { None } else { Some((String::new(), self.backend.fonts())) };
            }
        });
        if let Some((filter, fams)) = &mut self.font_picker {
            ui.add(egui::TextEdit::singleline(filter).hint_text("filter, or a family name / file path").desired_width(f32::INFINITY));
            let f = filter.to_lowercase();
            if fams.is_empty() {
                ui.label(egui::RichText::new("No installed fonts found (is fontconfig installed?). Type a file path above and press Use.").weak());
            }
            egui::ScrollArea::vertical().id_salt("font-list").max_height(280.0).min_scrolled_height(280.0).show(ui, |ui| {
                for fam in fams.iter().filter(|x| f.is_empty() || x.name.to_lowercase().contains(&f)) {
                    let b = sel_button(fam.name == cur, fam.name.as_str()).min_size(egui::vec2(ui.available_width(), 36.0));
                    if ui.add(b).on_hover_text(fam.file.display().to_string()).clicked() {
                        pick = Some(fam.name.clone());
                    }
                }
            });
            if !filter.trim().is_empty() && ui.add(big(&format!("Use \"{}\"", filter.trim()))).clicked() {
                pick = Some(filter.trim().to_string());
            }
        }
        if let Some(v) = pick {
            // Checked here first, so a bad name is refused with the reason
            // instead of being saved and failing in the keyboard.
            match crate::user_font::load(&v) {
                Ok(_) => {
                    self.font_picker = None;
                    self.set(s.key, &v);
                }
                Err(e) => self.status = Some((e, true)),
            }
        }
    }

    fn layouts_page(&mut self, ui: &mut egui::Ui) {
        // Take the view so its arms can call &mut self methods.
        let view = std::mem::replace(&mut self.view, LayoutView::List);
        match view {
            LayoutView::List => self.layout_list(ui),
            LayoutView::Import(mut text) => {
                if !self.import_page(ui, &mut text) && matches!(self.view, LayoutView::List) {
                    self.view = LayoutView::Import(text);
                }
            }
            LayoutView::Edit(mut st) => {
                if !self.editor_page(ui, &mut st) && matches!(self.view, LayoutView::List) {
                    self.view = LayoutView::Edit(st);
                }
            }
        }
    }

    fn layout_list(&mut self, ui: &mut egui::Ui) {
        let big = |t: &str| egui::Button::new(t.to_string()).min_size(egui::vec2(0.0, 40.0));
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let in_use = self.user_layout();
            ui.add_space(8.0);
            ui.heading("Your layouts");
            ui.separator();
            if self.layouts.is_empty() {
                ui.label(egui::RichText::new("None yet. Copy a built-in board below, start a blank one, or import JSON.").weak());
            }
            let entries = self.layouts.clone();
            for e in &entries {
                let slug = e.path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&e.display_name).strong().size(16.0));
                    if slug == in_use {
                        ui.label(egui::RichText::new("in use").color(ui.visuals().selection.bg_fill));
                    }
                });
                if let Some(err) = &e.error {
                    ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
                }
                ui.horizontal_wrapped(|ui| {
                    if e.error.is_none() {
                        if slug != in_use && ui.add(big("Use")).clicked() {
                            self.use_layout(&slug);
                        }
                        if ui.add(big("Edit")).clicked() {
                            self.edit_file(&e.path);
                        }
                        if ui.add(big("Copy")).clicked() {
                            if let Ok(l) = self.backend.load_layout(&e.path) {
                                self.edit_copy(&l, &format!("{} copy", l.display_name));
                            }
                        }
                        if ui.add(big("Copy JSON")).clicked() {
                            match self.backend.load_layout(&e.path).and_then(|l| board_layouts::io::export_to_string(&l).map_err(|e| e.to_string())) {
                                Ok(j) => {
                                    ui.ctx().copy_text(j);
                                    self.status = Some(("Layout JSON copied to the clipboard.".into(), false));
                                }
                                Err(err) => self.status = Some((err, true)),
                            }
                        }
                    }
                    let armed = self.confirm_delete.as_deref() == Some(e.path.as_path());
                    if ui.add(big(if armed { "Tap to delete" } else { "Delete" })).clicked() {
                        if armed {
                            self.confirm_delete = None;
                            self.delete_layout(&e.path);
                        } else {
                            self.confirm_delete = Some(e.path.clone());
                        }
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add(big("New blank layout")).clicked() {
                    self.view = LayoutView::Edit(Box::new(EditState::new(Editor::new(layout_edit::blank("My layout")), None)));
                    if let LayoutView::Edit(st) = &mut self.view {
                        st.ed.dirty = true;
                    }
                }
                if ui.add(big("Import JSON")).clicked() {
                    self.view = LayoutView::Import(String::new());
                }
            });

            ui.add_space(16.0);
            ui.heading("Built-in boards");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Letters board (Settings page)").strong().size(16.0));
                if in_use.is_empty() {
                    ui.label(egui::RichText::new("in use").color(ui.visuals().selection.bg_fill));
                }
            });
            if !in_use.is_empty() && ui.add(big("Use the built-in board")).clicked() {
                self.use_layout("");
            }
            for b in builtins() {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&b.display_name).size(16.0));
                    if ui.add(big("Copy and edit")).clicked() {
                        self.edit_copy(&b, &format!("My {}", b.display_name));
                    }
                });
            }
            ui.add_space(24.0);
        });
    }

    /// Returns true when it changed `self.view` itself.
    fn import_page(&mut self, ui: &mut egui::Ui, text: &mut String) -> bool {
        ui.add_space(8.0);
        ui.heading("Import a layout");
        ui.label(egui::RichText::new("Paste layout JSON (the format \"Copy JSON\" gives). It opens in the editor as a copy; nothing is saved until you save it.").weak());
        let mut next = None;
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 40.0))).clicked() {
                next = Some(LayoutView::List);
            }
            if ui.add(egui::Button::new("Paste from clipboard").min_size(egui::vec2(0.0, 40.0))).clicked() {
                // egui delivers a paste as an event on the focused text box;
                // this only focuses it so the system paste lands there.
                ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("import-json")));
            }
            if ui.add(egui::Button::new("Open in editor").min_size(egui::vec2(0.0, 40.0))).clicked() {
                match board_layouts::io::import_from_string(text) {
                    Ok(l) => {
                        let name = l.display_name.clone();
                        next = Some(LayoutView::Edit(Box::new(EditState::new(Editor::from_template(&l, &name), None))));
                    }
                    Err(e) => self.status = Some((format!("Not a layout: {e:#}"), true)),
                }
            }
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(text).id(egui::Id::new("import-json")).code_editor().desired_rows(16).desired_width(f32::INFINITY));
        });
        match next {
            Some(v) => {
                self.view = v;
                true
            }
            None => false,
        }
    }

    /// Returns true when it changed `self.view` itself.
    fn editor_page(&mut self, ui: &mut egui::Ui, st: &mut EditState) -> bool {
        let mut leave = false;
        let mut save = None;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let back = if st.confirm_discard { "Discard changes" } else { "Back" };
            if ui.add(egui::Button::new(back).min_size(egui::vec2(0.0, 40.0))).clicked() {
                if st.ed.dirty && !st.confirm_discard {
                    st.confirm_discard = true;
                } else {
                    leave = true;
                }
            }
            if ui.add_enabled(st.ed.dirty, egui::Button::new("Save").min_size(egui::vec2(0.0, 40.0))).clicked() {
                save = Some(false);
            }
            if ui.add(egui::Button::new("Save and use").min_size(egui::vec2(0.0, 40.0))).clicked() {
                save = Some(true);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Name");
            let r = ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(f32::INFINITY));
            if r.changed() {
                st.ed.rename(&st.name);
                st.confirm_discard = false;
            }
        });
        ui.add_space(6.0);

        if preview(ui, st) {
            st.sync_buffers();
            st.confirm_discard = false;
        }
        ui.add_space(6.0);

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if inspector(ui, st) {
                st.confirm_discard = false;
            }
            ui.add_space(24.0);
        });

        if leave {
            self.view = LayoutView::List;
            self.reload();
            return true;
        }
        if let Some(and_use) = save {
            // save_editor reads self.view; put the state there for the call.
            let taken = std::mem::replace(st, EditState::new(Editor::new(layout_edit::blank("x")), None));
            self.view = LayoutView::Edit(Box::new(taken));
            self.save_editor(and_use);
            return true;
        }
        false
    }
}

/// The layout, drawn to scale; a tap selects a key. Returns whether the
/// selection changed.
fn preview(ui: &mut egui::Ui, st: &mut EditState) -> bool {
    let l = &st.ed.layout;
    let row_h = 46.0;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, row_h * l.rows.len().max(1) as f32), egui::Sense::hover());
    let span = l
        .rows
        .iter()
        .map(|r| r.keys.iter().map(|k| k.width).sum::<f32>() + if l.ortholinear { 0.0 } else { r.offset })
        .fold(l.grid_columns.max(1.0), f32::max);
    let unit = width / span;
    let vis = ui.visuals().clone();
    let mut clicked = None;
    for (ri, row) in l.rows.iter().enumerate() {
        let mut x = rect.left() + if l.ortholinear { 0.0 } else { row.offset * unit };
        let y = rect.top() + ri as f32 * row_h;
        for (ki, k) in row.keys.iter().enumerate() {
            let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(k.width * unit, row_h)).shrink(2.0);
            x += k.width * unit;
            let resp = ui.interact(r, ui.id().with(("key", ri, ki)), egui::Sense::click());
            let selected = st.ed.sel == Some((ri, ki));
            let fill = if selected {
                vis.selection.bg_fill
            } else if matches!(k.action, board_layouts::key::KeyAction::NoOp) {
                vis.extreme_bg_color
            } else {
                vis.widgets.inactive.weak_bg_fill
            };
            ui.painter().rect_filled(r, 6.0, fill);
            // The keyboard paints these as icons; egui's bundled fonts have no
            // ⇧ ⌫ ⏎, so the preview names them instead of drawing tofu.
            use board_layouts::key::KeyAction as A;
            let shown = match k.action {
                A::Shift => "shift",
                A::Backspace => "bksp",
                A::Return => "enter",
                A::Space => "space",
                _ => k.label.as_str(),
            };
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, shown, egui::FontId::proportional(if shown.chars().count() > 3 { 13.0 } else { 16.0 }), vis.text_color());
            if resp.clicked() {
                clicked = Some((ri, ki));
            }
        }
    }
    match clicked {
        Some((r, k)) if st.ed.sel != Some((r, k)) => {
            st.ed.select(r, k);
            true
        }
        _ => false,
    }
}

/// The selected key's and row's controls. Returns whether anything changed.
fn inspector(ui: &mut egui::Ui, st: &mut EditState) -> bool {
    let mut changed = false;
    let btn = |t: &str| egui::Button::new(t.to_string()).min_size(egui::vec2(44.0, 40.0));

    ui.horizontal_wrapped(|ui| {
        if ui.add(btn("+ Key")).clicked() {
            st.ed.insert_key_after();
            changed = true;
        }
        if ui.add(btn("+ Row")).clicked() {
            st.ed.insert_row_below();
            changed = true;
        }
        let ortho = st.ed.layout.ortholinear;
        if ui.add(sel_button(ortho, "Grid (no stagger)").min_size(egui::vec2(0.0, 40.0))).clicked() {
            st.ed.set_ortholinear(!ortho);
            changed = true;
        }
    });

    if st.ed.sel.is_none() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Tap a key above to change it.").weak());
        if changed {
            st.sync_buffers();
        }
        return changed;
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui.add(btn("◀")).on_hover_text("Move left").clicked() {
            st.ed.move_key(-1);
            changed = true;
        }
        if ui.add(btn("▶")).on_hover_text("Move right").clicked() {
            st.ed.move_key(1);
            changed = true;
        }
        if ui.add(btn("Delete key")).clicked() {
            st.ed.delete_key();
            changed = true;
        }
        if ui.add(btn("Delete row")).clicked() {
            st.ed.delete_row();
            changed = true;
        }
    });
    if changed {
        st.sync_buffers();
        return true;
    }
    let Some(key) = st.ed.selected().cloned() else { return changed };

    let grid = |ui: &mut egui::Ui, name: &str, buf: &mut String, hint: &str| -> bool {
        ui.label(name);
        ui.add(egui::TextEdit::singleline(buf).hint_text(hint).desired_width(f32::INFINITY)).changed()
    };
    ui.add_space(6.0);
    if grid(ui, "Label", &mut st.label, "what the key shows") {
        st.ed.set_label(&st.label);
        changed = true;
    }
    if grid(ui, "Shifted", &mut st.shifted, "empty: the label in capitals") {
        st.ed.set_shifted(&st.shifted);
        changed = true;
    }
    if grid(ui, "Long-press", &mut st.extended, "variants, space-separated: é è ê") {
        st.ed.set_extended(&st.extended);
        changed = true;
    }

    ui.add_space(6.0);
    ui.label("Width");
    ui.horizontal(|ui| {
        if ui.add(btn("−")).clicked() {
            st.ed.set_width(key.width - layout_edit::WIDTH_STEP);
            changed = true;
        }
        ui.label(egui::RichText::new(format!("{:.2}", key.width)).size(18.0).monospace());
        if ui.add(btn("+")).clicked() {
            st.ed.set_width(key.width + layout_edit::WIDTH_STEP);
            changed = true;
        }
        ui.label(egui::RichText::new("keys wide").weak());
    });
    if !st.ed.layout.ortholinear {
        let off = st.ed.sel.and_then(|(r, _)| st.ed.layout.rows.get(r)).map_or(0.0, |r| r.offset);
        ui.label("Row indent");
        ui.horizontal(|ui| {
            if ui.add(btn("−")).clicked() {
                st.ed.set_row_offset(off - layout_edit::WIDTH_STEP);
                changed = true;
            }
            ui.label(egui::RichText::new(format!("{off:.2}")).size(18.0).monospace());
            if ui.add(btn("+")).clicked() {
                st.ed.set_row_offset(off + layout_edit::WIDTH_STEP);
                changed = true;
            }
        });
    }

    ui.add_space(6.0);
    ui.label("Does");
    let now = ActionChoice::of(&key.action);
    ui.horizontal_wrapped(|ui| {
        for c in ActionChoice::ALL {
            if ui.add(sel_button(now == c, c.label()).min_size(egui::vec2(0.0, 36.0))).clicked() && now != c {
                st.ed.set_action(c, &st.value);
                changed = true;
            }
        }
    });
    if now.needs_value() {
        let hint = if now == ActionChoice::Keysym { "keysym name, e.g. Escape, Left, F5" } else { "the text to type" };
        if grid(ui, "Value", &mut st.value, hint) {
            st.ed.set_action(now, &st.value);
            changed = true;
        }
    }
    changed
}

/// A selectable button that still looks like a button when not selected (a
/// bare label is not obviously tappable on a phone).
fn sel_button<'a>(selected: bool, text: impl egui::IntoAtoms<'a>) -> egui::Button<'a> {
    egui::Button::selectable(selected, text).frame_when_inactive(true)
}

fn parse_hex(s: &str) -> Option<egui::Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Touch-sized egui style: bigger text and hit targets than egui's desktop
/// defaults, which are 18pt-tall buttons.
pub fn touch_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.interact_size.y = 40.0;
        s.spacing.button_padding = egui::vec2(12.0, 8.0);
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.scroll.bar_width = 6.0;
        // A drag over a label must SCROLL the page. egui's labels are
        // selectable by default, so a thumb-scroll started on help text
        // highlighted it and the page did not move.
        s.interaction.selectable_labels = false;
        for (t, f) in s.text_styles.iter_mut() {
            f.size = match t {
                egui::TextStyle::Heading => 22.0,
                egui::TextStyle::Small => 12.0,
                _ => 15.0,
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(running: bool) -> Companion<MemBackend> {
        Companion::new(MemBackend { keyboard_running: running, ..Default::default() })
    }

    #[test]
    fn a_setting_is_written_and_the_keyboard_told() {
        let mut c = app(true);
        c.set("layout.letters", "dvorak");
        assert_eq!(c.backend.settings.get("layout.letters").map(String::as_str), Some("dvorak"));
        assert_eq!(c.backend.sent, ["reload-settings"]);
        assert_eq!(c.status.as_ref().map(|s| s.1), Some(false));
        let s = settings_model::SETTINGS.iter().find(|s| s.key == "layout.letters").unwrap();
        assert_eq!(c.current(s), "dvorak");
    }

    #[test]
    fn with_no_keyboard_running_the_save_still_happens_and_says_so() {
        let mut c = app(false);
        c.set("layout.letters", "dvorak");
        assert_eq!(c.backend.settings.get("layout.letters").map(String::as_str), Some("dvorak"));
        assert!(c.status.as_ref().unwrap().0.contains("not running"));
    }

    #[test]
    fn copy_a_builtin_save_and_use_it() {
        let mut c = app(true);
        let q = builtins().into_iter().next().unwrap();
        c.edit_copy(&q, "Thumbs");
        if let LayoutView::Edit(st) = &mut c.view {
            st.ed.select(0, 0);
            st.ed.set_label("Q!");
        }
        c.save_editor(true);
        assert_eq!(c.backend.layouts.len(), 1);
        let (p, l) = c.backend.layouts.iter().next().unwrap();
        assert_eq!(l.rows[0].keys[0].label, "Q!");
        assert_eq!(c.user_layout(), p.file_stem().unwrap().to_str().unwrap());
        assert_eq!(c.backend.sent.last().map(String::as_str), Some("reload-layouts"));
        assert!(q.name != l.name, "the copy never takes the built-in's name");
    }

    #[test]
    fn renaming_a_layout_in_use_moves_the_file_and_the_choice() {
        let mut c = app(true);
        c.edit_copy(&layout_edit::blank("One"), "One");
        c.save_editor(true);
        let old = c.backend.layouts.keys().next().unwrap().clone();
        c.edit_file(&old);
        if let LayoutView::Edit(st) = &mut c.view {
            st.ed.rename("Two");
        }
        c.save_editor(false);
        assert_eq!(c.backend.layouts.len(), 1, "no stale copy under the old name");
        assert_eq!(c.user_layout(), "user-two");
    }

    #[test]
    fn deleting_the_layout_in_use_falls_back_to_the_builtin() {
        let mut c = app(true);
        c.edit_copy(&layout_edit::blank("Gone"), "Gone");
        c.save_editor(true);
        let p = c.backend.layouts.keys().next().unwrap().clone();
        c.delete_layout(&p);
        assert!(c.backend.layouts.is_empty());
        assert_eq!(c.user_layout(), "");
    }

    #[test]
    fn an_invalid_layout_is_not_saved() {
        let mut c = app(true);
        c.edit_copy(&layout_edit::blank("Bad"), "Bad");
        if let LayoutView::Edit(st) = &mut c.view {
            st.ed.layout.rows.clear();
        }
        c.save_editor(false);
        assert!(c.backend.layouts.is_empty());
        assert_eq!(c.status.as_ref().map(|s| s.1), Some(true));
    }

    /// Every page lays out without panicking at phone width.
    #[test]
    fn every_page_renders_headless() {
        let ctx = egui::Context::default();
        touch_style(&ctx);
        let mut c = app(true);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(432.0, 936.0))),
            ..Default::default()
        };
        for step in 0..4 {
            match step {
                1 => c.page = Page::Layouts,
                2 => c.edit_copy(&builtins()[0], "X"),
                3 => c.view = LayoutView::Import("{".into()),
                _ => {}
            }
            let _ = ctx.run(input(), |ctx| c.ui(ctx));
        }
    }
}
