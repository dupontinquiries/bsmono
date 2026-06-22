//! components.rs — shared model dropdown widget

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label, ListItem, Orientation, SignalListItemFactory};
use std::rc::Rc;
use std::cell::RefCell;

use crate::providers::ModelEntry;

// ── ProviderStore ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ProviderStore {
    pub string_list: gtk4::StringList,
    pub entries:     Rc<RefCell<Vec<ModelEntry>>>,
    /// True while apply_entries is mutating the list.
    pub updating:    Rc<std::cell::Cell<bool>>,
    /// Entries deferred while the popup is open.
    pub pending:     Rc<RefCell<Option<Vec<ModelEntry>>>>,
    /// True while the DropDown popup is open.
    /// Driven by the ToggleButton child's "active" property — reliable because
    /// the ToggleButton is created at widget construction time, not lazily.
    pub popup_open:  Rc<std::cell::Cell<bool>>,
}

impl ProviderStore {
    pub fn new() -> Self {
        Self {
            string_list: gtk4::StringList::new(&[]),
            entries:     Rc::new(RefCell::new(Vec::new())),
            updating:    Rc::new(std::cell::Cell::new(false)),
            pending:     Rc::new(RefCell::new(None)),
            popup_open:  Rc::new(std::cell::Cell::new(false)),
        }
    }

    /// Apply entries immediately. Only call when popup is closed.
    pub fn apply_entries(&self, drop: &gtk4::DropDown, new_entries: Vec<ModelEntry>) {
        self.updating.set(true);
        let saved = self.string_list.string(drop.selected())
            .map(|s| s.to_string()).unwrap_or_default();
        while self.string_list.n_items() > 0 {
            self.string_list.remove(self.string_list.n_items() - 1);
        }
        for e in &new_entries { self.string_list.append(&e.store_key()); }
        let mut restored = false;
        if !saved.is_empty() {
            for i in 0..self.string_list.n_items() {
                if self.string_list.string(i).as_deref() == Some(&saved) {
                    drop.set_selected(i); restored = true; break;
                }
            }
        }
        if !restored && !new_entries.is_empty() { drop.set_selected(0); }
        *self.entries.borrow_mut() = new_entries;
        self.updating.set(false);
    }

    /// Set entries, deferring if the popup is open.
    pub fn set_entries(&self, drop: &gtk4::DropDown, new_entries: Vec<ModelEntry>) {
        if self.popup_open.get() {
            *self.pending.borrow_mut() = Some(new_entries);
            return;
        }
        self.apply_entries(drop, new_entries);
    }

    pub fn selected_entry(&self, drop: &gtk4::DropDown) -> Option<ModelEntry> {
        let key = self.string_list.string(drop.selected())?.to_string();
        self.entries.borrow().iter().find(|e| e.store_key() == key).cloned()
    }
}

// ── Shared factory helpers ────────────────────────────────────────────────────

fn make_list_factory<F>(bind: F) -> SignalListItemFactory
where F: Fn(&ListItem) + 'static
{
    let lf = SignalListItemFactory::new();
    lf.connect_setup(|_, obj| {
        let item = obj.downcast_ref::<ListItem>().unwrap();
        let vbox = GtkBox::new(Orientation::Vertical, 1);
        vbox.set_margin_start(6); vbox.set_margin_end(6);
        vbox.set_margin_top(3);   vbox.set_margin_bottom(3);
        let row1 = GtkBox::new(Orientation::Horizontal, 6);
        let name_lbl = Label::builder().halign(gtk4::Align::Start).hexpand(true).build();
        name_lbl.add_css_class("model-drop-name");
        let kind_lbl = Label::builder().halign(gtk4::Align::End).build();
        kind_lbl.add_css_class("model-drop-kind");
        row1.append(&name_lbl);
        row1.append(&kind_lbl);
        let sub_lbl = Label::builder()
            .halign(gtk4::Align::Start).hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End).build();
        sub_lbl.add_css_class("model-drop-host");
        vbox.append(&row1);
        vbox.append(&sub_lbl);
        item.set_child(Some(&vbox));
    });
    lf.connect_bind(move |_, obj| {
        let item = obj.downcast_ref::<ListItem>().unwrap();
        bind(item);
    });
    lf
}

fn make_button_factory<F>(bind: F) -> SignalListItemFactory
where F: Fn(&ListItem) + 'static
{
    let bf = SignalListItemFactory::new();
    bf.connect_setup(|_, obj| {
        let item = obj.downcast_ref::<ListItem>().unwrap();
        let lbl = Label::builder()
            .halign(gtk4::Align::Start).hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End).build();
        lbl.add_css_class("model-drop-name");
        item.set_child(Some(&lbl));
    });
    bf.connect_bind(move |_, obj| {
        let item = obj.downcast_ref::<ListItem>().unwrap();
        bind(item);
    });
    bf
}

fn set_row(item: &ListItem, name: &str, kind: &str, sub: &str) {
    let Some(vbox) = item.child().and_then(|w| w.downcast::<GtkBox>().ok()) else { return };
    let Some(row1) = vbox.first_child().and_then(|w| w.downcast::<GtkBox>().ok()) else { return };
    if let Some(n) = row1.first_child().and_then(|w| w.downcast::<Label>().ok()) { n.set_text(name); }
    if let Some(k) = row1.last_child().and_then(|w| w.downcast::<Label>().ok()) {
        k.set_text(kind); k.set_visible(!kind.is_empty());
    }
    if let Some(s) = vbox.last_child().and_then(|w| w.downcast::<Label>().ok()) {
        s.set_text(sub); s.set_visible(!sub.is_empty());
    }
}

fn item_key(item: &ListItem) -> String {
    item.item()
        .and_then(|o| o.downcast::<gtk4::StringObject>().ok())
        .map(|s| s.string().to_string())
        .unwrap_or_default()
}

// ── Wire popup tracking via ToggleButton::active ──────────────────────────────
//
// GTK4 DropDown structure (created at widget construction, not lazy):
//   GtkDropDown
//     GtkToggleButton  ← first_child(); active=true while popup is open
//     GtkPopover       ← created lazily on first open
//
// We connect to the ToggleButton's "active" property because it exists immediately,
// unlike the Popover which is created lazily and may not exist at realize time.

/// Find the first ToggleButton anywhere in the widget subtree (up to depth 3).
fn find_toggle_button(widget: &gtk4::Widget, depth: u32) -> Option<gtk4::ToggleButton> {
    if depth == 0 { return None; }
    if let Ok(tb) = widget.clone().downcast::<gtk4::ToggleButton>() { return Some(tb); }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_toggle_button(&c, depth - 1) { return Some(found); }
        child = c.next_sibling();
    }
    None
}

fn wire_toggle(store: &ProviderStore, toggle: &gtk4::ToggleButton, drop: &gtk4::DropDown) {
    let s = store.clone();
    let d = drop.clone();
    toggle.connect_notify_local(Some("active"), move |btn, _| {
        let open = btn.is_active();
        s.popup_open.set(open);
        if !open {
            if let Some(entries) = s.pending.borrow_mut().take() {
                s.apply_entries(&d, entries);
            }
        }
    });
}

fn wire_popup_tracking(store: &ProviderStore, drop: &gtk4::DropDown) {
    // Try at construction time (ToggleButton usually exists immediately)
    if let Some(toggle) = find_toggle_button(drop.upcast_ref::<gtk4::Widget>(), 3) {
        wire_toggle(store, &toggle, drop);
        return;
    }
    // Fallback: defer to realize if widget tree not ready yet
    let s = store.clone();
    let d = drop.clone();
    drop.connect_realize(move |widget| {
        if let Some(toggle) = find_toggle_button(widget.upcast_ref::<gtk4::Widget>(), 3) {
            wire_toggle(&s, &toggle, &d);
        }
    });
}

// ── build_model_dropdown ──────────────────────────────────────────────────────

pub fn build_model_dropdown(store: &ProviderStore) -> gtk4::DropDown {
    let entries = Rc::clone(&store.entries);

    let lf = {
        let e = Rc::clone(&entries);
        make_list_factory(move |item| {
            let key = item_key(item);
            let ents = e.borrow();
            let (name, sub, kind) = ents.iter()
                .find(|e| e.store_key() == key)
                .map(|e| (e.model.as_str(), e.subtitle.as_str(), e.kind.as_str()))
                .unwrap_or(("…", "", ""));
            set_row(item, name, kind, sub);
        })
    };

    let bf = {
        let e = Rc::clone(&entries);
        make_button_factory(move |item| {
            let key = item_key(item);
            let name = e.borrow().iter()
                .find(|en| en.store_key() == key)
                .map(|en| en.model.clone())
                .unwrap_or_else(|| {
                    // Fallback for placeholders like "scanning…"
                    key.rfind('@').map(|i| key[..i].to_string()).unwrap_or(key)
                });
            if let Some(lbl) = item.child().and_then(|w| w.downcast::<Label>().ok()) {
                lbl.set_text(&name);
            }
        })
    };

    let drop = gtk4::DropDown::builder()
        .model(&store.string_list)
        .factory(&bf)
        .list_factory(&lf)
        .build();

    wire_popup_tracking(store, &drop);

    drop
}

// ── build_model_dropdown_simple ───────────────────────────────────────────────

pub fn build_model_dropdown_simple(store: gtk4::StringList) -> gtk4::DropDown {
    let lf = make_list_factory(|item| {
        let key = item_key(item);
        let (model_name, host_label) = display_split_entry(&key);
        let kind = if host_label == "⚡ auto" { "auto" }
                   else if host_label == "local" { "" }
                   else { "LAN" };
        let sub = if host_label == "local" || host_label == "⚡ auto" || host_label.is_empty() {
            String::new()
        } else {
            host_label.clone()
        };
        set_row(item, &model_name, kind, &sub);
    });

    let bf = make_button_factory(|item| {
        let key = item_key(item);
        let (name, _) = display_split_entry(&key);
        if let Some(lbl) = item.child().and_then(|w| w.downcast::<Label>().ok()) {
            lbl.set_text(&name);
        }
    });

    let drop = gtk4::DropDown::builder()
        .model(&store)
        .factory(&bf)
        .list_factory(&lf)
        .build();

    // Defer StringList updates when popup is open — connect once the
    // internal ToggleButton is available (may need to wait for realize).
    {
        let d = drop.clone();
        drop.connect_realize(move |widget| {
            if let Some(toggle) = find_toggle_button(widget.upcast_ref::<gtk4::Widget>(), 3) {
                // Nothing to flush here (StringList, not ProviderStore) but
                // the toggle signal is wired so populate_model_store can use it.
                let _ = (toggle, d.clone()); // keep alive
            }
        });
    }

    drop
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn resolve_key_to_host(key: &str, default_host: &str) -> (String, String) {
    if let Some(at) = key.rfind('@') {
        let model  = key[..at].to_string();
        let suffix = &key[at + 1..];
        if suffix == "auto" { return (model, default_host.to_string()); }
        (model, suffix.to_string())
    } else {
        (key.to_string(), default_host.to_string())
    }
}

pub fn display_split_entry(key: &str) -> (String, String) {
    let (model, host_url) = resolve_key_to_host(key, "local");
    let label = if host_url == "local" || host_url.is_empty() { "local".to_string() }
                else if host_url == "⚡ auto" { "⚡ auto".to_string() }
                else {
                    host_url.trim_start_matches("http://")
                        .trim_start_matches("https://")
                        .split(':').next().unwrap_or(&host_url).to_string()
                };
    (model, label)
}

pub fn populate_model_store(
    store: &gtk4::StringList, drop: &gtk4::DropDown,
    models: &[crate::model_selector::ModelInfo],
) {
    let popup_open = find_toggle_button(drop.upcast_ref::<gtk4::Widget>(), 3)
        .map(|t| t.is_active())
        .unwrap_or(false);

    if popup_open {
        // Defer: apply once the popup closes via a one-shot signal
        let store2  = store.clone();
        let drop2   = drop.clone();
        let models2: Vec<crate::model_selector::ModelInfo> = models.to_vec();
        if let Some(toggle) = find_toggle_button(drop.upcast_ref::<gtk4::Widget>(), 3) {
            // fired flag avoids the id_cell race: if the signal fires synchronously
            // during connect (toggle already inactive), id_cell would still be None.
            let fired = Rc::new(std::cell::Cell::new(false));
            let fired2 = Rc::clone(&fired);
            toggle.connect_notify_local(Some("active"), move |btn, _| {
                if btn.is_active() { return; }       // wait for popup to close
                if fired2.replace(true) { return; }  // already applied this batch
                apply_to_stringlist(&store2, &drop2, &models2);
            });
        }
        return;
    }
    apply_to_stringlist(store, drop, models);
}

fn apply_to_stringlist(
    store: &gtk4::StringList, drop: &gtk4::DropDown,
    models: &[crate::model_selector::ModelInfo],
) {
    let new_keys: Vec<String> = models.iter().map(|m| m.store_key()).collect();

    // Compare current store contents against new keys — skip rebuild if identical.
    // This prevents DropDown from re-creating its item list and closing any open popup.
    let current_len = store.n_items() as usize;
    let already_same = current_len == new_keys.len()
        && new_keys.iter().enumerate().all(|(i, k)| {
            store.string(i as u32).as_deref() == Some(k.as_str())
        });
    if already_same { return; }

    let saved = store.string(drop.selected()).map(|s| s.to_string()).unwrap_or_default();

    // Mutate in-place: splice rather than clear+append to minimise layout invalidation
    // Trim excess rows from the end
    while store.n_items() as usize > new_keys.len() {
        store.remove(store.n_items() - 1);
    }
    // Update existing rows that differ, append new ones
    for (i, key) in new_keys.iter().enumerate() {
        if (i as u32) < store.n_items() {
            if store.string(i as u32).as_deref() != Some(key.as_str()) {
                store.splice(i as u32, 1, &[key.as_str()]);
            }
        } else {
            store.append(key);
        }
    }

    // Restore selection — only call set_selected when the value actually changes
    // to avoid triggering notify::selected which can close the DropDown popup.
    let mut restored = false;
    if !saved.is_empty() {
        for i in 0..store.n_items() {
            if store.string(i).as_deref() == Some(saved.as_str()) {
                if drop.selected() != i { drop.set_selected(i); }
                restored = true;
                break;
            }
        }
    }
    if !restored && store.n_items() > 0 && drop.selected() != 0 {
        drop.set_selected(0);
    }
}

pub fn restore_selection(drop: &gtk4::DropDown, store: &gtk4::StringList, saved: &str) {
    if saved.is_empty() { return; }
    for i in 0..store.n_items() {
        if store.string(i).as_deref() == Some(saved) { drop.set_selected(i); return; }
    }
}

/// Populate a simple StringList dropdown with models from all known hosts.
/// Shows "scanning…" immediately, updates as models are found.
/// Covers: local Ollama, LAN registry hosts, and a background /24 scan.
pub fn fetch_models_into_store(
    store: &gtk4::StringList,
    drop:  &gtk4::DropDown,
) {
    use crossbeam_channel::unbounded;

    // Show placeholder immediately
    while store.n_items() > 0 { store.remove(store.n_items() - 1); }
    store.append("scanning…");

    let store2 = store.clone();
    let drop2  = drop.clone();
    let (tx, rx) = unbounded::<Vec<crate::model_selector::ModelInfo>>();

    std::thread::spawn(move || {
        // 1. Localhost + known LAN registry
        if let Ok(reg) = crate::providers::ProviderRegistry::open() {
            let local = crate::providers::probe_host(&reg, crate::ui::ollama::DEFAULT_HOST);
            if !local.is_empty() { let _ = tx.send(local); }
            for kh in reg.known_hosts() {
                if kh.url == crate::ui::ollama::DEFAULT_HOST { continue; }
                if !kh.is_recent() { continue; }
                let models = crate::providers::probe_host(&reg, &kh.url);
                if !models.is_empty() { let _ = tx.send(models); }
            }
        }
        // 2. Background LAN scan for new hosts
        let (ptx, prx) = unbounded::<(String, Vec<crate::model_selector::ModelInfo>)>();
        std::thread::spawn(move || { crate::providers::scan_lan_parallel(&ptx); });
        for (_, batch) in prx {
            if !batch.is_empty() { let _ = tx.send(batch); }
        }
        // tx drops here → Disconnected signals the drain timer
    });

    // Drain batches, merge, repopulate store only when new models arrive
    let acc: std::rc::Rc<std::cell::RefCell<Vec<crate::model_selector::ModelInfo>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        let mut got = false;
        loop {
            match rx.try_recv() {
                Ok(batch) => {
                    let mut a = acc.borrow_mut();
                    for m in batch {
                        if !a.iter().any(|e| e.store_key() == m.store_key()) { a.push(m); }
                    }
                    got = true;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let a = acc.borrow();
                    if a.is_empty() {
                        // Only set placeholder if not already showing it
                        if store2.n_items() != 1
                            || store2.string(0).as_deref() != Some("no models found") {
                            while store2.n_items() > 0 { store2.remove(store2.n_items()-1); }
                            store2.append("no models found");
                        }
                    } else {
                        populate_model_store(&store2, &drop2, &a);
                    }
                    return glib::ControlFlow::Break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
            }
        }
        // Only touch the StringList when new data arrived — not on every tick
        if got { populate_model_store(&store2, &drop2, &acc.borrow()); }
        glib::ControlFlow::Continue
    });
}

// ── UiScheduler — batch UI commits into single idle frame ────────────────────
//
// Usage:
//   let scheduler = UiScheduler::new();
//   // ... later, instead of refresh_ui() directly:
//   scheduler.schedule(refresh.clone());
//
// Multiple calls before the idle fires coalesce into one refresh — eliminates
// the "20 updates → 20 redraws" pattern from timer loops.

pub struct UiScheduler {
    pending: Rc<std::cell::Cell<bool>>,
}

impl UiScheduler {
    pub fn new() -> Self {
        Self { pending: Rc::new(std::cell::Cell::new(false)) }
    }

    /// Schedule `f` to run on the next idle tick. If already scheduled, no-op.
    pub fn schedule(&self, f: Rc<dyn Fn()>) {
        if self.pending.replace(true) {
            return; // already queued — next idle will call it
        }
        let pending = Rc::clone(&self.pending);
        glib::idle_add_local_once(move || {
            pending.set(false);
            f();
        });
    }
}

