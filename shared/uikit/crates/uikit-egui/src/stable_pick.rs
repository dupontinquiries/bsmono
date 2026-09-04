//! A selection that survives the list being rebuilt.
//!
//! # Third occurrence, so it moves here
//!
//! `logscope-egui/src/model.rs` built this for chat models. `d2x-core`'s
//! `registry.rs` was then "shaped after logscope's picker" for upscaler
//! models, and its header records the reason in the same words:
//! **selection is keyed by path, not by index**. CADscope now wants a picker
//! for its reference/ask panel.
//!
//! What transferred each time was not code — a chat model, a `.safetensors`
//! and a CAD reference share no fields — but the state machine and one rule.
//! That is what lives here.
//!
//! # Not the same thing as `model_picker`
//!
//! `model_picker` in this crate is a llama.cpp GGUF browser: rows, a starred
//! recommendation, download progress, a `ModelAction` for the host to
//! dispatch. It is a whole screen. This is the two-field state behind a combo,
//! and it has no opinion about what an item is or where it came from. An app
//! can use either or both.
//!
//! # The rule the module exists for
//!
//! A rescan rebuilds the list. Adding one file shifts every index after it, so
//! a stored index silently names a DIFFERENT item — and a bounds check does
//! not help, because the stale index is still in range. Nothing looks wrong
//! and the wrong thing loads.

/// What the picker needs from an item: an identity and something to show.
pub trait PickerEntry {
    /// Stable across a rescan — a path, a tag, an id. Anything the caller can
    /// persist. NOT an index, which is the bug this module is about.
    fn key(&self) -> String;

    /// What the user reads in the list.
    fn label(&self) -> String;

    /// Whether this entry can be chosen. Defaults to true.
    ///
    /// d2x's registry has entries that scan and fail verification — a model
    /// file that loads and produces grey. Showing them greyed beats hiding
    /// them: "my model is missing" sends the user looking in the wrong place.
    fn selectable(&self) -> bool {
        true
    }
}

/// A list and a selection into it.
#[derive(Debug, Clone)]
pub struct PickerState<T> {
    items: Vec<T>,
    /// `usize::MAX` is "nothing selected", deliberately distinct from index 0.
    ///
    /// A sentinel rather than `Option<usize>` because logscope's revert path
    /// needs a state that is not "the first item": defaulting to 0 after a
    /// failed load asserts something is active that is not, and a name in the
    /// active slot is the one piece of state nobody re-checks.
    selected: usize,
}

impl<T> Default for PickerState<T> {
    fn default() -> Self {
        PickerState { items: Vec::new(), selected: usize::MAX }
    }
}

impl<T: PickerEntry> PickerState<T> {
    pub fn new(items: Vec<T>) -> Self {
        let selected = if items.is_empty() { usize::MAX } else { 0 };
        PickerState { items, selected }
    }

    pub fn items(&self) -> &[T] { &self.items }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    pub fn selected(&self) -> Option<&T> { self.items.get(self.selected) }
    pub fn selected_index(&self) -> Option<usize> {
        self.items.get(self.selected).map(|_| self.selected)
    }
    /// The selected item's key, for persisting across a restart.
    pub fn selected_key(&self) -> Option<String> { self.selected().map(|i| i.key()) }

    /// What a collapsed combo shows. An em dash when nothing is chosen, rather
    /// than an empty box that reads as a rendering fault.
    pub fn current_label(&self) -> String {
        self.selected().map(|i| i.label()).unwrap_or_else(|| "—".into())
    }

    /// Select by index, refusing an out-of-range or unselectable choice.
    pub fn select(&mut self, index: usize) -> bool {
        match self.items.get(index) {
            Some(i) if i.selectable() => { self.selected = index; true }
            _ => false,
        }
    }

    pub fn select_key(&mut self, key: &str) -> bool {
        match self.items.iter().position(|i| i.key() == key) {
            Some(p) => self.select(p),
            None => false,
        }
    }

    /// Point back at `key` after an operation failed, or clear the selection.
    ///
    /// A key that is gone LEAVES the selection alone rather than guessing:
    /// moving it somewhere arbitrary is the same failure in a different place.
    pub fn revert_to(&mut self, key: Option<&str>) {
        match key {
            Some(k) => {
                if let Some(p) = self.items.iter().position(|i| i.key() == k) {
                    self.selected = p;
                }
            }
            None => self.selected = usize::MAX,
        }
    }

    /// Replace the list, restoring the selection BY IDENTITY.
    ///
    /// Returns whether the previously-selected item survived. A caller that
    /// ignores the answer gets the safe fallback; one that cares can say "the
    /// model you had is gone" instead of quietly running a different one.
    pub fn replace_items(&mut self, items: Vec<T>) -> bool {
        let want = self.selected_key();
        self.items = items;
        match want.and_then(|k| self.items.iter().position(|i| i.key() == k)) {
            Some(p) => { self.selected = p; true }
            None => {
                self.selected = if self.items.is_empty() { usize::MAX } else { 0 };
                false
            }
        }
    }
}

/// A combo over a [`PickerState`]. True when the selection changed.
///
/// `id` is required rather than derived from the label: two pickers on one
/// screen showing the same text would share widget state and fight, which only
/// shows up once a second picker is added.
pub fn picker_combo<T: PickerEntry>(
    ui: &mut egui::Ui,
    st: &mut PickerState<T>,
    id: &str,
    busy: bool,
) -> bool {
    let mut changed = false;
    let current = st.current_label();
    egui::ComboBox::from_id_salt(id)
        .selected_text(if busy { format!("{current} …") } else { current })
        .show_ui(ui, |ui| {
            for i in 0..st.items().len() {
                let (label, ok) = {
                    let it = &st.items()[i];
                    (it.label(), it.selectable())
                };
                let is_sel = st.selected_index() == Some(i);
                ui.add_enabled_ui(ok, |ui| {
                    if ui.selectable_label(is_sel, label).clicked() && st.select(i) {
                        changed = true;
                    }
                });
            }
        });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Model { path: String, ok: bool }
    impl PickerEntry for Model {
        fn key(&self) -> String { self.path.clone() }
        fn label(&self) -> String {
            self.path.rsplit('/').next().unwrap_or(&self.path).to_string()
        }
        fn selectable(&self) -> bool { self.ok }
    }
    fn m(p: &str) -> Model { Model { path: p.into(), ok: true } }

    #[test]
    fn a_rescan_that_inserts_an_item_does_not_move_the_selection() {
        // THE bug. An index survives a rescan while naming something else, and
        // a bounds check cannot see it because the stale index is in range.
        let mut st = PickerState::new(vec![m("/m/b.st"), m("/m/c.st")]);
        assert!(st.select_key("/m/c.st"));
        assert_eq!(st.selected_index(), Some(1));

        let kept = st.replace_items(vec![m("/m/a.st"), m("/m/b.st"), m("/m/c.st")]);
        assert!(kept, "the selected model was reported missing");
        assert_eq!(st.selected_key().as_deref(), Some("/m/c.st"),
                   "the selection followed the index instead of the identity");
        assert_eq!(st.selected_index(), Some(2));
    }

    #[test]
    fn a_vanished_selection_is_reported_rather_than_silently_replaced() {
        let mut st = PickerState::new(vec![m("/m/a"), m("/m/b")]);
        st.select_key("/m/b");
        assert!(!st.replace_items(vec![m("/m/a")]), "a missing item was reported present");
        assert_eq!(st.selected_index(), Some(0), "the fallback is out of range");
    }

    #[test]
    fn an_empty_list_selects_nothing_rather_than_index_zero() {
        let mut st = PickerState::new(vec![m("/m/a")]);
        assert!(!st.replace_items(Vec::<Model>::new()));
        assert_eq!(st.selected_index(), None);
        assert_eq!(st.current_label(), "—", "an empty picker showed a blank box");
        assert_eq!(PickerState::<Model>::default().selected_index(), None);
    }

    #[test]
    fn reverting_to_a_name_that_is_gone_leaves_the_selection_alone() {
        let mut st = PickerState::new(vec![m("/m/a"), m("/m/b")]);
        st.select_key("/m/b");
        st.revert_to(Some("/m/nonexistent"));
        assert_eq!(st.selected_key().as_deref(), Some("/m/b"));
        // Clearing is distinct from selecting the first item.
        st.revert_to(None);
        assert_eq!(st.selected_index(), None);
    }

    #[test]
    fn an_unselectable_entry_is_listed_but_cannot_be_chosen() {
        let bad = Model { path: "/m/broken".into(), ok: false };
        let mut st = PickerState::new(vec![m("/m/good"), bad]);
        assert_eq!(st.items().len(), 2, "the unusable entry was hidden");
        assert!(!st.select(1));
        assert!(!st.select_key("/m/broken"));
        assert_eq!(st.selected_index(), Some(0));
    }

    #[test]
    fn the_label_is_display_and_the_key_is_identity() {
        // Two files with the same basename are different models. Keying on the
        // label would treat them as one and restore the wrong one.
        let (a, b) = (m("/models/v1/net.st"), m("/models/v2/net.st"));
        assert_eq!(a.label(), b.label());
        assert_ne!(a.key(), b.key());
        let mut st = PickerState::new(vec![a, b]);
        st.select(1);
        assert!(st.replace_items(vec![m("/models/v1/net.st"), m("/models/v2/net.st")]));
        assert_eq!(st.selected_key().as_deref(), Some("/models/v2/net.st"));
    }
}
