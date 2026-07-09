//! Unified model picker: logscope's real Ollama `ModelEntry` rows AND local
//! GGUF files (`llm_backend::LocalModel`) in one list. Chat routes by the
//! selected item's kind.
use crate::theme;
use egui::{FontId, RichText};

pub use logscope::llm_backend::LocalModel;
pub use logscope::providers::ModelEntry;

pub enum PickerItem {
    Ollama(ModelEntry),
    Local(LocalModel),
}
impl PickerItem {
    pub fn name(&self) -> &str {
        match self { PickerItem::Ollama(e) => &e.model, PickerItem::Local(m) => &m.name }
    }
    pub fn is_local(&self) -> bool { matches!(self, PickerItem::Local(_)) }
    /// (name, subtitle, kind-badge)
    fn row(&self) -> (String, String, String) {
        match self {
            PickerItem::Ollama(e) => { let (n, s, k) = e.display(); (n.into(), s.into(), k.into()) }
            PickerItem::Local(m) => {
                let gb = m.size_bytes as f64 / 1e9;
                let p = if m.params_billions > 0.0 { format!("{:.0}B · ", m.params_billions) } else { String::new() };
                (m.name.clone(), format!("{p}{gb:.1} GB · local file"), "gguf".into())
            }
        }
    }
}

pub struct ModelState {
    pub items: Vec<PickerItem>,
    pub selected: usize,
}
impl Default for ModelState {
    fn default() -> Self { Self { items: Vec::new(), selected: 0 } }
}
impl ModelState {
    pub fn current(&self) -> String {
        self.items.get(self.selected).map(|i| i.name().to_string()).unwrap_or_else(|| "—".into())
    }
    pub fn selected_item(&self) -> Option<&PickerItem> { self.items.get(self.selected) }
    pub fn selected_is_local(&self) -> bool {
        self.selected_item().map(|i| i.is_local()).unwrap_or(false)
    }
    pub fn selected_local_path(&self) -> Option<String> {
        match self.selected_item() {
            Some(PickerItem::Local(m)) => Some(m.path.to_string_lossy().into_owned()),
            _ => None,
        }
    }
}

/// Index of the smallest LOCAL model in the pool — fewest parameters, with file size breaking ties. `None`
/// if there are no local models. This is the "spawn a tinier model" pick: the cheap third run that diagnoses
/// a bad-jump compaction. Ollama rows are skipped (the diagnosis is meant to be a small local file).
pub fn smallest_local(items: &[PickerItem]) -> Option<usize> {
    items.iter().enumerate()
        .filter_map(|(i, it)| match it {
            PickerItem::Local(m) => Some((i, m.params_billions, m.size_bytes)),
            _ => None,
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(a.2.cmp(&b.2)))
        .map(|(i, _, _)| i)
}

fn item_text(i: &PickerItem) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let (name, sub, kind) = i.row();
    let kind_col = if kind == "gguf" { theme::AMBER() } else { theme::DIM() };
    let mut job = LayoutJob::default();
    job.append(&name, 0.0, TextFormat { color: theme::INK(), font_id: FontId::monospace(12.0), ..Default::default() });
    job.append(&format!("\n{kind}", ), 0.0, TextFormat { color: kind_col, font_id: FontId::monospace(9.0), ..Default::default() });
    job.append(&format!(" · {sub}"), 0.0, TextFormat { color: theme::DIM(), font_id: FontId::monospace(9.0), ..Default::default() });
    job
}

pub fn model_combo(ui: &mut egui::Ui, st: &mut ModelState, id: &str, scanning: bool) {
    let sel = if st.items.is_empty() {
        (if scanning { "scanning…" } else { "no models" }).to_string()
    } else { st.current() };
    egui::ComboBox::from_id_salt(id).selected_text(sel).width(250.0).show_ui(ui, |ui| {
        for (i, e) in st.items.iter().enumerate() {
            ui.selectable_value(&mut st.selected, i, item_text(e));
        }
        if st.items.is_empty() {
            ui.label(RichText::new("(scanning…)").color(theme::DIM()).small());
        }
    });
}

/// What the folder popup reports back so the caller can act (browse / rescan) without this owning that state.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct FolderPopupResult { pub browse: bool, pub scan: bool }

/// The model-folder configuration, tucked into a popup button (📁) beside the picker instead of eating a
/// whole header row. Edits `models_dir` in place; returns which button (if any) was pressed. `native` is the
/// system-file-picker toggle, mutated in place.
pub fn folder_popup(ui: &mut egui::Ui, models_dir: &mut String, native: &mut bool, native_label: &str) -> FolderPopupResult {
    let mut out = FolderPopupResult::default();
    let btn = ui.button("📁").on_hover_text("Models folder settings");
    let pid = ui.make_persistent_id("folder_popup");
    if btn.clicked() { ui.memory_mut(|m| m.toggle_popup(pid)); }
    // CloseOnClickOutside so the text field / checkbox / buttons INSIDE work without dismissing the popup
    // (menu_button closed on every click — that was the "can't interact" bug).
    egui::popup_below_widget(ui, pid, &btn, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(340.0);
        ui.label(RichText::new("models folder").color(theme::DIM()).small());
        ui.add(egui::TextEdit::singleline(models_dir)
            .font(egui::TextStyle::Monospace).desired_width(320.0)
            .hint_text("dir to walk for .gguf (recurses, incl. HF cache)"));
        ui.horizontal(|ui| {
            if ui.button("📂 Browse…").clicked() { out.browse = true; }
            if ui.button("Scan").clicked() { out.scan = true; }
        });
        ui.separator();
        ui.checkbox(native, RichText::new("system file picker").small())
            .on_hover_text(format!("Use the desktop's own file dialog — detected: {native_label}.\nUncheck for the built-in picker on headless sessions."));
    });
    out
}

/// Replaces the single-select dropdown with a multi-model DEPLOYMENT picker: every available model is a TILE
/// you check to include, then assign a role (Target/Draft/Delegate/Embed) + device, ordered by priority. This
/// is the backing UI for tree/spec work (a Target verifies a smaller Draft) across devices. Mutates `dep` in
/// place; returns true if the plan changed this frame. Renders inline (host it in a popup/panel).
/// The **active-pipeline** selectors: one dropdown per primary role — Large (text target), Small
/// (text draft), Voice (STT/TTS), Image (diffusion). Each is a `ComboBox`, so this MUST be shown in
/// a Window or a normal panel — never a `popup_below_widget` — because a ComboBox's dropdown is a
/// separate layer that a click-outside popup treats as an outside click and closes (the
/// "double-dropdown close each other" bug). Returns true if the plan changed.
pub fn pipeline_selectors(
    ui: &mut egui::Ui,
    dep: &mut logscope::deploy::Deployment,
    available: &[String],
    est_bytes: &dyn Fn(&str) -> u64,
) -> bool {
    use logscope::deploy::{DeviceId, Role};
    let mut changed = false;
    let device = dep.devices.first().map(|d| d.id).unwrap_or(DeviceId(0));
    let rows = [
        (Role::Target, "Large model", "the main text model that commits tokens"),
        (Role::Draft, "Small model", "a smaller model that drafts tokens for the large one to verify (speculative)"),
        (Role::Voice, "Voice model", "speech — Whisper (STT) or a TTS"),
        (Role::Image, "Image model", "image diffusion — Z-Image / FLUX / SD"),
    ];
    egui::Grid::new("pipeline_roles").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        for (role, label, hint) in rows {
            ui.label(RichText::new(label).color(theme::INK())).on_hover_text(hint);
            let cur = dep.role_model(role).map(|s| s.to_string());
            let sel_text = cur.clone().unwrap_or_else(|| "(none)".into());
            egui::ComboBox::from_id_salt(("pipeline", label))
                .selected_text(sel_text)
                .width(250.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(cur.is_none(), "(none)").clicked() {
                        dep.clear_role(role);
                        changed = true;
                    }
                    for name in available {
                        let is_sel = cur.as_deref() == Some(name.as_str());
                        let gb = est_bytes(name) as f64 / 1e9;
                        let txt = format!("{name}  ·  {gb:.1} GB");
                        if ui.selectable_label(is_sel, RichText::new(txt).monospace().small()).clicked() {
                            dep.set_role(role, name, device, est_bytes(name));
                            changed = true;
                        }
                    }
                    if available.is_empty() {
                        ui.label(RichText::new("(no models — scan a folder)").color(theme::DIM()).small());
                    }
                });
            ui.end_row();
        }
    });
    changed
}

pub fn deployment_tiles(ui: &mut egui::Ui, dep: &mut logscope::deploy::Deployment, available: &[String], est_bytes: &dyn Fn(&str) -> u64) -> bool {
    use logscope::deploy::{Role, Slot, DeviceId};
    let mut changed = false;
    let roles = [Role::Target, Role::Draft, Role::Delegate, Role::Embed, Role::Image, Role::Voice];
    let role_name = |r: Role| r.label();
    let devices: Vec<(DeviceId, String)> = dep.devices.iter().map(|d| (d.id, d.name.clone())).collect();

    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for name in available.iter() {
            let slot_idx = dep.slots.iter().position(|s| &s.model == name);
            let mut enabled = slot_idx.map(|j| dep.slots[j].enabled).unwrap_or(false);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    // the check tile: include this model in the deployment
                    if ui.checkbox(&mut enabled, "").changed() {
                        changed = true;
                        match slot_idx {
                            Some(j) => dep.slots[j].enabled = enabled,
                            None if enabled => dep.add(Slot {
                                model: name.clone(),
                                role: if dep.slots.iter().all(|s| s.role != Role::Target) { Role::Target } else { Role::Draft },
                                device: devices.first().map(|d| d.0).unwrap_or(DeviceId(0)),
                                priority: dep.slots.len() as u32,
                                enabled: true,
                                est_bytes: est_bytes(name),
                            }),
                            None => {}
                        }
                    }
                    ui.label(RichText::new(name).color(theme::INK()).monospace());
                    // role + device selectors only matter once the tile is on. Inline selectable CHIPS, not
                    // ComboBoxes — a nested combo opens its own popup layer, which the parent popup reads as a
                    // click-outside and closes (the "role dropdown cancels the menu" bug).
                    if let Some(j) = dep.slots.iter().position(|s| &s.model == name) {
                        if dep.slots[j].enabled {
                            let mut role = dep.slots[j].role;
                            ui.separator();
                            for r in roles {
                                if ui.selectable_label(role == r, RichText::new(role_name(r)).small()).clicked() { role = r; changed = true; }
                            }
                            dep.slots[j].role = role;
                            if devices.len() > 1 {
                                let mut dev = dep.slots[j].device;
                                ui.separator();
                                for (id, nm) in &devices {
                                    if ui.selectable_label(dev == *id, RichText::new(nm).small()).clicked() { dev = *id; changed = true; }
                                }
                                dep.slots[j].device = dev;
                            }
                        }
                    }
                });
            });
        }
    });
    // validation strip — surfaces the plan's problems inline (what gates the Load button)
    let issues = dep.validate();
    if issues.is_empty() {
        ui.label(RichText::new(if dep.tree_ready() { "✓ ready · speculation on" } else { "✓ ready" }).color(theme::GREEN()).small());
    } else {
        for iss in &issues {
            ui.label(RichText::new(format!("• {}", issue_text(iss))).color(theme::RED()).small());
        }
    }
    changed
}

fn issue_text(i: &logscope::deploy::Issue) -> String {
    use logscope::deploy::Issue::*;
    match i {
        Empty => "check at least one model".into(),
        NoTarget => "needs a Target model".into(),
        MultipleTargets(n) => format!("{n} Targets — pick one"),
        DraftNotSmallerThanTarget => "the Draft must be smaller than the Target".into(),
        UnknownDevice(m) => format!("{m}: unknown device"),
        DeviceOverBudget { used, cap, .. } => format!("device over budget ({:.1} > {:.1} GB)", *used as f64 / 1e9, *cap as f64 / 1e9),
    }
}

#[cfg(test)]
mod walk_tests {
    use logscope::llm_backend::list_local_gguf;
    use std::fs;

    #[test]
    fn recursive_walk_finds_nested_and_names_hf_cache() {
        let root = std::env::temp_dir().join(format!("lsq_walk_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Llama-3.2-1B-Instruct-Q4_K_M.gguf"), b"x").unwrap();
        let sub = root.join("mistral-7b");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("mistral-7b-instruct-q4.gguf"), b"x").unwrap();
        let hf = root.join("models--meta-llama--Llama-3.2-3B").join("snapshots").join("a1b2c3d4e5f6a1b2");
        fs::create_dir_all(&hf).unwrap();
        fs::write(hf.join("model.gguf"), b"x").unwrap();
        fs::write(root.join("notes.txt"), b"x").unwrap();

        let found = list_local_gguf(&root);
        let names: Vec<&str> = found.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(found.len(), 3, "all three nested gguf, got {names:?}");
        assert!(names.iter().any(|n| n.contains("Llama-3.2-1B-Instruct")), "descriptive stem kept: {names:?}");
        assert!(names.iter().any(|n| *n == "mistral-7b-instruct-q4"), "nested found: {names:?}");
        assert!(names.iter().any(|n| n.contains("meta-llama/Llama-3.2-3B")), "HF name unwrapped: {names:?}");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;
    use logscope::llm_backend::LocalModel;
    use std::path::PathBuf;

    fn local(name: &str, params: f32, bytes: u64) -> PickerItem {
        PickerItem::Local(LocalModel { name: name.into(), path: PathBuf::from(name), size_bytes: bytes, params_billions: params })
    }

    #[test]
    fn smallest_local_picks_fewest_params_then_size() {
        let items = vec![
            local("deepseek-8b", 8.0, 5_000_000_000),
            local("smollm2-360m", 0.36, 380_000_000),
            local("llama-1b", 1.0, 800_000_000),
        ];
        assert_eq!(smallest_local(&items), Some(1), "the 360M model is the tiniest");

        // tie on params → smaller file wins
        let tie = vec![local("a-1b", 1.0, 900_000_000), local("b-1b", 1.0, 700_000_000)];
        assert_eq!(smallest_local(&tie), Some(1));

        // no local models → None
        assert_eq!(smallest_local(&[]), None);
    }
}
