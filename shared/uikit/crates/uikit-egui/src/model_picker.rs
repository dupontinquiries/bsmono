//! model_picker — egui port of `qml/ModelPicker.qml` (core).
//!
//! A local llama.cpp GGUF browser/activator: rows for each candidate model — already-downloaded ones
//! first, the recommended quant starred — with a per-row action (Download an available file, or Use a
//! downloaded one), plus a download-progress readout. The QML drove this through backend hooks
//! (`download_model`, `download_progress_json`, `local_models_json`, `set_llama_model_path`); this port
//! renders the list + progress and returns the chosen [`ModelAction`] for the host to dispatch. Search
//! over Hugging Face and the actual download/activation stay in the backend.
//!
//! Row ordering is pure and unit-tested.

use egui::{Color32, RichText};

/// One selectable model.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModelRow {
    /// GGUF file name (the identity used for download/activate).
    pub name: String,
    /// Human size, e.g. "4.1 GB" (empty if unknown).
    pub size_label: String,
    /// Optional source repo (for downloads).
    pub repo: String,
    /// Already on disk.
    pub downloaded: bool,
    /// The suggested quant for this machine.
    pub recommended: bool,
    /// Currently the active model.
    pub active: bool,
    /// One-line description from the curated list ("fast, good at code").
    /// Empty when unknown — a row with no blurb renders without one
    /// rather than with a placeholder, since an empty line under every
    /// model is worse than none under some.
    pub blurb: String,
}

/// What the HuggingFace search pane is showing.
///
/// The QML uses ONE result list for both, swapped by a `showingFiles`
/// flag, with a `‹ back to results` affordance to return. Modelled the
/// same way rather than as two lists: they occupy the same space, only
/// one is ever visible, and two lists would let both be populated and
/// disagree about which is current.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchView {
    /// Repositories matching the query.
    #[default]
    Repos,
    /// The GGUF files inside one repo — `repo` is what to go back from.
    Files { repo: String },
}

/// The search pane's state. Pure; the host performs the queries.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub view: SearchView,
    /// Repo names, or file names once drilled in. One vector for the
    /// same reason there is one view.
    pub results: Vec<String>,
    /// True while a query is in flight, so the pane can say so rather
    /// than looking empty — an empty list and a pending search are
    /// indistinguishable otherwise, and one of them is not a result.
    pub searching: bool,
}

impl SearchState {
    /// Drill into a repo. Clears the results, because the ones on screen
    /// belong to the previous view: leaving them would show repo names
    /// under a files heading for as long as the fetch takes.
    pub fn open_repo(&mut self, repo: impl Into<String>) {
        self.view = SearchView::Files { repo: repo.into() };
        self.results.clear();
        self.searching = true;
    }

    /// Return to the repo results. Also clears, for the same reason, and
    /// the host re-runs the query — cheaper than holding two result sets
    /// to keep one of them stale.
    pub fn back(&mut self) {
        self.view = SearchView::Repos;
        self.results.clear();
        self.searching = true;
    }

    /// Is the back affordance shown? Only when drilled in, matching the
    /// QML's `visible: picker.showingFiles`.
    pub fn can_go_back(&self) -> bool {
        matches!(self.view, SearchView::Files { .. })
    }

    /// What the empty state should say. Distinguishes the three cases a
    /// blank list can mean, which the QML also does: nothing typed yet,
    /// a search running, and a search that genuinely found nothing.
    pub fn empty_hint(&self) -> Option<&'static str> {
        if !self.results.is_empty() {
            return None;
        }
        Some(if self.searching {
            "searching…"
        } else if self.query.trim().is_empty() {
            "Search HuggingFace GGUF models (Enter)…"
        } else {
            "no matches"
        })
    }
}

/// What the user asked to do with a row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ModelAction {
    /// Download `file` from `repo`.
    Download { repo: String, file: String },
    /// Activate an already-downloaded model by file name.
    Use(String),
}

/// The heading above the list, which the QML varies on detected RAM.
///
/// `ram_gb` is 0 when detection failed or has not run. The two audits
/// that found this missing (ModelPicker and SetupWizard, C12-28a/b)
/// wanted the same string, so it lives here once rather than being
/// written twice with two wordings.
///
/// This deliberately does NOT decide WHICH model is recommended. That is
/// `hf_hub::recommend_curated` in voiceterm — reserve-aware, speed-first,
/// falling back to the smallest rather than to nothing — and it needs the
/// curated list and a whisper-reserve estimate that a shared widget has
/// no business knowing. The host sets `ModelRow::recommended`; this only
/// explains what the star MEANS, which is the part that was missing.
/// Duplicating the fit arithmetic here to avoid passing a bool would be
/// two answers to one question, and they would drift.
pub fn picks_heading(ram_gb: u32) -> String {
    if ram_gb > 0 {
        format!("Quick picks (★ = recommended for your {ram_gb}GB machine):")
    } else {
        // No RAM figure: promise ordering, not suitability. Claiming a
        // recommendation "for your machine" when nothing was measured
        // would be a guess wearing a measurement's clothes.
        "Quick picks (fastest first):".to_string()
    }
}

/// Sort rows for display: downloaded first, then recommended, then by name. Pure — the tested core.
pub fn sorted_rows(rows: &[ModelRow]) -> Vec<ModelRow> {
    let mut v = rows.to_vec();
    v.sort_by(|a, b| {
        b.downloaded
            .cmp(&a.downloaded)
            .then(b.recommended.cmp(&a.recommended))
            .then(a.name.cmp(&b.name))
    });
    v
}

/// The QML shows models in TWO sections, not one list: "Downloaded:"
/// first, then the curated "Quick picks". This splits them, preserving
/// `sorted_rows`' order within each.
///
/// The distinction is not cosmetic. A downloaded model costs one click
/// to use; a curated one costs a multi-gigabyte download. Presenting
/// them in a single list — which the port did — makes those look like
/// the same decision, and the star is the only hint otherwise. Someone
/// scanning for "the one I already have" has to read every row.
///
/// Returns `(downloaded, picks)`. Either may be empty, and the caller
/// renders a section only when it has rows — an empty "Downloaded:"
/// heading on a fresh install is a promise of something that is not
/// there.
pub fn split_sections(rows: &[ModelRow]) -> (Vec<ModelRow>, Vec<ModelRow>) {
    let sorted = sorted_rows(rows);
    let (have, want): (Vec<ModelRow>, Vec<ModelRow>) =
        sorted.into_iter().partition(|r| r.downloaded);
    (have, want)
}

/// Browser state.
#[derive(Default)]
pub struct ModelPicker {
    pub rows: Vec<ModelRow>,
    /// Free-text status from the backend ("", "downloading", "done", "error: …").
    pub download_status: String,
    /// 0..=100 while a download is in flight.
    pub download_percent: u8,
    /// Detected system RAM in GB, or 0 when unknown. Set by the host —
    /// the widget reports it, the host measures it.
    pub detected_ram_gb: u32,
}

impl ModelPicker {
    /// Render the picker; returns a [`ModelAction`] on the frame a row's button is clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<ModelAction> {
        let mut action = None;

        if !self.download_status.is_empty() {
            let downloading = self.download_status == "downloading" || self.download_status.starts_with("fetch");
            if downloading {
                ui.add(egui::ProgressBar::new(self.download_percent as f32 / 100.0).text(format!("{}%", self.download_percent)));
            } else {
                let err = self.download_status.starts_with("error");
                let col = if err { Color32::from_rgb(189, 90, 68) } else { ui.visuals().weak_text_color() };
                ui.colored_label(col, &self.download_status);
            }
            ui.add_space(6.0);
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // TWO SECTIONS, as the QML has. A downloaded model costs one
            // click; a curated one costs a multi-gigabyte download, and a
            // single list makes those look like the same decision. Each
            // heading is drawn only when its section has rows — an empty
            // "Downloaded:" on a fresh install promises something that is
            // not there.
            let (have, picks) = split_sections(&self.rows);
            let mut first = true;
            for (heading, section) in [
                ("Downloaded:".to_string(), have),
                (picks_heading(self.detected_ram_gb), picks),
            ] {
                if section.is_empty() {
                    continue;
                }
                if !first {
                    ui.add_space(10.0);
                }
                first = false;
                ui.label(RichText::new(heading).weak().small());
                ui.add_space(4.0);
            for m in section {
                ui.horizontal(|ui| {
                    // star for the recommended quant
                    ui.label(if m.recommended { RichText::new("★").color(Color32::from_rgb(201, 185, 154)) } else { RichText::new("  ") });
                    // name + size + active/downloaded badge
                    let mut name = RichText::new(&m.name);
                    if m.active {
                        name = name.strong().color(ui.visuals().selection.bg_fill);
                    }
                    ui.label(name);
                    if !m.size_label.is_empty() {
                        ui.label(RichText::new(&m.size_label).weak().small());
                    }
                    // right-aligned action
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if m.active {
                            ui.label(RichText::new("active").small().color(ui.visuals().selection.bg_fill));
                        } else if m.downloaded {
                            if ui.button("Use").clicked() {
                                action = Some(ModelAction::Use(m.name.clone()));
                            }
                        } else if ui.button("Download").clicked() {
                            action = Some(ModelAction::Download { repo: m.repo.clone(), file: m.name.clone() });
                        }
                    });
                });
                // The blurb sits UNDER the row, indented, as in the QML —
                // it explains what the model is for, which is the thing
                // that makes a curated list worth having over a list of
                // file names. Skipped entirely when empty.
                if !m.blurb.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(RichText::new(&m.blurb).weak().small());
                    });
                }
                ui.separator();
            }
            }
            if self.rows.is_empty() {
                ui.weak_label_compat("no models — search a repo or drop a .gguf in the models folder");
            }
        });

        action
    }
}

// small compat shim so the "empty" hint reads clearly regardless of egui label helpers
trait WeakLabel {
    fn weak_label_compat(&mut self, text: &str);
}
impl WeakLabel for egui::Ui {
    fn weak_label_compat(&mut self, text: &str) {
        self.label(RichText::new(text).weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, downloaded: bool, recommended: bool) -> ModelRow {
        ModelRow {
            name: name.into(),
            size_label: String::new(),
            repo: String::new(),
            downloaded,
            recommended,
            active: false,
            blurb: String::new(),
        }
    }

    #[test]
    fn drilling_in_and_back_never_shows_the_previous_view_s_rows() {
        // The bug this prevents: repo names listed under a files heading
        // (or the reverse) for as long as a fetch takes. One list, one
        // view, cleared on every transition.
        let mut s = SearchState { query: "qwen".into(), ..Default::default() };
        s.results = vec!["TheBloke/Qwen".into(), "Qwen/Qwen2".into()];
        assert!(!s.can_go_back(), "no back affordance at the top level");

        s.open_repo("Qwen/Qwen2");
        assert!(s.results.is_empty(), "stale repo names survived the drill-in");
        assert!(s.searching);
        assert!(s.can_go_back());
        assert_eq!(s.view, SearchView::Files { repo: "Qwen/Qwen2".into() });

        s.results = vec!["qwen2-7b-q4.gguf".into()];
        s.searching = false;
        s.back();
        assert!(s.results.is_empty(), "stale file names survived going back");
        assert!(!s.can_go_back());
    }

    #[test]
    fn an_empty_list_says_which_kind_of_empty_it_is() {
        // Three different situations render as a blank list, and only one
        // of them means "there is nothing". Conflating them makes a
        // pending search look like a failed one.
        let mut s = SearchState::default();
        assert_eq!(s.empty_hint(), Some("Search HuggingFace GGUF models (Enter)…"));

        s.query = "llama".into();
        s.searching = true;
        assert_eq!(s.empty_hint(), Some("searching…"));

        s.searching = false;
        assert_eq!(s.empty_hint(), Some("no matches"), "a finished search that found nothing");

        s.results = vec!["a".into()];
        assert_eq!(s.empty_hint(), None, "results: no hint at all");
    }

    #[test]
    fn sections_separate_one_click_from_one_download() {
        // The split exists because those are different decisions: a
        // downloaded model costs a click, a curated one costs gigabytes.
        // A single list makes them look alike, and the star is then the
        // only hint — which is why the port read as a flat list of
        // equivalent options when it is not.
        let rows = vec![
            row("zeta.gguf", true, false),
            row("alpha.gguf", false, true),
            row("beta.gguf", false, false),
            row("aaa.gguf", true, false),
        ];
        let (have, picks) = split_sections(&rows);
        assert_eq!(have.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
                   vec!["aaa.gguf", "zeta.gguf"], "downloaded, still name-sorted");
        assert_eq!(picks.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
                   vec!["alpha.gguf", "beta.gguf"], "recommended leads the picks");
        assert!(picks[0].recommended);
    }

    #[test]
    fn a_fresh_install_has_no_downloaded_section_to_promise() {
        // Every row is a candidate; nothing is on disk. The caller skips
        // an empty section rather than drawing a heading over nothing.
        let rows = vec![row("a.gguf", false, true), row("b.gguf", false, false)];
        let (have, picks) = split_sections(&rows);
        assert!(have.is_empty());
        assert_eq!(picks.len(), 2);
    }

    #[test]
    fn the_heading_only_claims_a_recommendation_when_ram_was_measured() {
        // The star means "will actually run on this machine" — not
        // "popular" or "newest" — and the heading is the only place that
        // is said. With no RAM figure it must promise ORDERING instead,
        // because claiming a recommendation "for your machine" when
        // nothing was measured is a guess wearing a measurement's
        // clothes.
        let known = picks_heading(16);
        assert!(known.contains("16GB"), "{known}");
        assert!(known.contains("★"), "the star is explained: {known}");

        let unknown = picks_heading(0);
        assert!(!unknown.contains("your"), "claimed a fit with no measurement: {unknown}");
        assert!(unknown.contains("fastest first"), "{unknown}");
    }

    #[test]
    fn downloaded_first_then_recommended_then_name() {
        let rows = vec![
            row("zeta", false, false),
            row("alpha", false, true),   // recommended, not downloaded
            row("beta", true, false),    // downloaded
            row("gamma", true, true),    // downloaded + recommended → very top
        ];
        let sorted = sorted_rows(&rows);
        let order: Vec<&str> = sorted.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(order, ["gamma", "beta", "alpha", "zeta"]);
    }

    #[test]
    fn empty_is_empty() {
        assert!(sorted_rows(&[]).is_empty());
    }
}
