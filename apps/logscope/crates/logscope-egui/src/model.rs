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
