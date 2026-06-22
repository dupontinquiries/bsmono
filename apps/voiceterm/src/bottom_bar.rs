//! bottom_bar: the dynamic model behind the unified expandable bottom
//! bar. The bar is a Row of buttons, each supporting the SAME
//! tap / tap-hold-drag-to-fan-out gesture as the terminal toolbar's
//! pipe (`|⋯`) and brace (`{}`) buttons — by REUSING the existing
//! GestureHost + SlidePopup QML primitives (see
//! docs/design/unified_bottom_bar.md). This module is only the model:
//! what buttons exist, what each expands to. QML renders it; no new
//! gesture code, no JS logic.

/// What a bar button represents — drives icon/behaviour + how its
/// fan-out options are sourced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButtonKind {
    /// Open tabs (terminal/browser/cards…). Fan-out = the tab list.
    Tabs,
    /// Named workspaces. Fan-out = the workspace list.
    Workspaces,
    /// Quick-launch commands. Fan-out = recent/saved commands.
    QuickLaunch,
    /// Recent note cards (from note_cards / session).
    RecentCards,
    /// Anything else assembled ad hoc.
    Custom,
}

/// One fan-out option (a SlidePopup model entry). `id` is opaque and
/// routed back to the backend on selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarOption {
    pub id: String,
    pub label: String,
}

impl BarOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

/// One button in the bar. Tap fires `primary`; tap-hold-drag fans out
/// `options` (the SlidePopup model). `enabled` lets the model gate a
/// button without removing it (the QML keeps layout stable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarButton {
    pub id: String,
    pub label: String,
    pub kind: BarButtonKind,
    /// Action id a tap invokes (may equal the first option, or be a
    /// distinct "primary" like "switch to last tab").
    pub primary: String,
    pub options: Vec<BarOption>,
    pub enabled: bool,
    /// Optional small badge text (e.g. tab count). Empty = none.
    pub badge: String,
}

impl BarButton {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: BarButtonKind) -> Self {
        let id = id.into();
        Self {
            primary: id.clone(),
            id,
            label: label.into(),
            kind,
            options: Vec::new(),
            enabled: true,
            badge: String::new(),
        }
    }
    pub fn with_primary(mut self, primary: impl Into<String>) -> Self {
        self.primary = primary.into(); self
    }
    pub fn with_options(mut self, options: Vec<BarOption>) -> Self {
        self.options = options; self
    }
    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into(); self
    }
}

/// The ordered set of bar buttons. The QML Row mirrors this; reorder /
/// relabel / badge / enable are model edits.
#[derive(Default, Clone, Debug)]
pub struct BottomBar {
    buttons: Vec<BarButton>,
}

impl BottomBar {
    pub fn new() -> Self { Self { buttons: Vec::new() } }

    pub fn len(&self) -> usize { self.buttons.len() }
    pub fn is_empty(&self) -> bool { self.buttons.is_empty() }
    pub fn buttons(&self) -> &[BarButton] { &self.buttons }

    /// Add a button (or replace one with the same id). Returns the
    /// index. Replace-by-id keeps the bar idempotent when state
    /// refreshes rebuild a button.
    pub fn upsert(&mut self, button: BarButton) -> usize {
        if let Some(i) = self.buttons.iter().position(|b| b.id == button.id) {
            self.buttons[i] = button;
            i
        } else {
            self.buttons.push(button);
            self.buttons.len() - 1
        }
    }

    pub fn find(&self, id: &str) -> Option<&BarButton> {
        self.buttons.iter().find(|b| b.id == id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.buttons.len();
        self.buttons.retain(|b| b.id != id);
        self.buttons.len() != before
    }

    /// Move a button to a new index (clamped). Used for user reorder /
    /// most-recently-used ordering. Returns true if it moved.
    pub fn reorder(&mut self, id: &str, to: usize) -> bool {
        let from = match self.buttons.iter().position(|b| b.id == id) {
            Some(i) => i,
            None => return false,
        };
        let to = to.min(self.buttons.len().saturating_sub(1));
        if from == to { return false; }
        let b = self.buttons.remove(from);
        self.buttons.insert(to, b);
        true
    }

    /// The fan-out options for a button (the SlidePopup model). Empty
    /// if the button is unknown or has no options.
    pub fn options_for(&self, button_id: &str) -> &[BarOption] {
        self.find(button_id).map(|b| b.options.as_slice()).unwrap_or(&[])
    }

    /// Route a selection: given (button_id, option_id), return the
    /// resolved (button_kind, option) if valid. The backend uses this
    /// to dispatch — one router for the whole bar (SlidePopup echoes
    /// the button id as its gestureContext).
    pub fn resolve(&self, button_id: &str, option_id: &str)
        -> Option<(BarButtonKind, &BarOption)>
    {
        let b = self.find(button_id)?;
        let opt = b.options.iter().find(|o| o.id == option_id)?;
        Some((b.kind, opt))
    }
}

/// Build a default bar from live state. The backend supplies the
/// current tab labels, workspace names, and recent-command labels;
/// this assembles the buttons + their fan-out options. Pure: the
/// caller owns the state, this just shapes it into the model.
pub fn default_bar(
    tabs: &[(String, String)],        // (id, label) of open tabs
    workspaces: &[(String, String)],  // (id, label) of workspaces
    quick: &[(String, String)],       // (id, label) of quick-launch cmds
) -> BottomBar {
    let mut bar = BottomBar::new();

    // Tabs button: tap = switch to last tab; fan-out = all tabs.
    let tab_opts: Vec<BarOption> =
        tabs.iter().map(|(id, l)| BarOption::new(id.clone(), l.clone())).collect();
    let mut tabs_btn = BarButton::new("tabs", "Tabs", BarButtonKind::Tabs)
        .with_primary("tab:last")
        .with_options(tab_opts);
    if !tabs.is_empty() { tabs_btn = tabs_btn.with_badge(tabs.len().to_string()); }
    tabs_btn.enabled = !tabs.is_empty();
    bar.upsert(tabs_btn);

    // Workspaces.
    let ws_opts: Vec<BarOption> =
        workspaces.iter().map(|(id, l)| BarOption::new(id.clone(), l.clone())).collect();
    let mut ws_btn = BarButton::new("workspaces", "Spaces", BarButtonKind::Workspaces)
        .with_options(ws_opts);
    ws_btn.enabled = !workspaces.is_empty();
    bar.upsert(ws_btn);

    // Quick-launch (recent commands / cards).
    let q_opts: Vec<BarOption> =
        quick.iter().map(|(id, l)| BarOption::new(id.clone(), l.clone())).collect();
    let mut q_btn = BarButton::new("quick", "Launch", BarButtonKind::QuickLaunch)
        .with_options(q_opts);
    q_btn.enabled = !quick.is_empty();
    bar.upsert(q_btn);

    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(id: &str, l: &str) -> BarOption { BarOption::new(id, l) }

    #[test]
    fn upsert_adds_then_replaces_by_id() {
        let mut bar = BottomBar::new();
        let i = bar.upsert(BarButton::new("tabs", "Tabs", BarButtonKind::Tabs));
        assert_eq!(i, 0);
        assert_eq!(bar.len(), 1);
        // same id replaces, doesn't duplicate
        let i2 = bar.upsert(BarButton::new("tabs", "Tabs!", BarButtonKind::Tabs));
        assert_eq!(i2, 0);
        assert_eq!(bar.len(), 1);
        assert_eq!(bar.find("tabs").unwrap().label, "Tabs!");
    }

    #[test]
    fn options_for_and_resolve() {
        let mut bar = BottomBar::new();
        bar.upsert(
            BarButton::new("tabs", "Tabs", BarButtonKind::Tabs)
                .with_options(vec![opt("t1", "shell"), opt("t2", "browser")]),
        );
        assert_eq!(bar.options_for("tabs").len(), 2);
        let (kind, o) = bar.resolve("tabs", "t2").unwrap();
        assert_eq!(kind, BarButtonKind::Tabs);
        assert_eq!(o.label, "browser");
        assert!(bar.resolve("tabs", "nope").is_none());
        assert!(bar.resolve("nope", "t1").is_none());
    }

    #[test]
    fn reorder_moves_button() {
        let mut bar = BottomBar::new();
        bar.upsert(BarButton::new("a", "A", BarButtonKind::Custom));
        bar.upsert(BarButton::new("b", "B", BarButtonKind::Custom));
        bar.upsert(BarButton::new("c", "C", BarButtonKind::Custom));
        assert!(bar.reorder("c", 0));
        let order: Vec<&str> = bar.buttons().iter().map(|b| b.id.as_str()).collect();
        assert_eq!(order, vec!["c", "a", "b"]);
        // no-op when already there
        assert!(!bar.reorder("c", 0));
        // clamps out-of-range
        assert!(bar.reorder("c", 99));
        assert_eq!(bar.buttons().last().unwrap().id, "c");
    }

    #[test]
    fn remove_button() {
        let mut bar = BottomBar::new();
        bar.upsert(BarButton::new("a", "A", BarButtonKind::Custom));
        assert!(bar.remove("a"));
        assert!(!bar.remove("a"));
        assert!(bar.is_empty());
    }

    #[test]
    fn default_bar_assembles_from_state() {
        let tabs = vec![("t1".into(), "shell".into()), ("t2".into(), "browser".into())];
        let ws = vec![("w1".into(), "work".into())];
        let quick = vec![("q1".into(), "rebuild".into())];
        let bar = default_bar(&tabs, &ws, &quick);
        assert_eq!(bar.len(), 3);
        // tabs button has badge "2" and both tab options
        let t = bar.find("tabs").unwrap();
        assert_eq!(t.badge, "2");
        assert_eq!(t.primary, "tab:last");
        assert_eq!(t.options.len(), 2);
        assert!(t.enabled);
        // workspaces + quick present and enabled
        assert!(bar.find("workspaces").unwrap().enabled);
        assert_eq!(bar.find("quick").unwrap().options.len(), 1);
    }

    #[test]
    fn default_bar_disables_empty_sections() {
        let bar = default_bar(&[], &[], &[]);
        assert_eq!(bar.len(), 3, "buttons still present for stable layout");
        assert!(!bar.find("tabs").unwrap().enabled, "empty tabs → disabled");
        assert_eq!(bar.find("tabs").unwrap().badge, "", "no badge when empty");
        assert!(!bar.find("workspaces").unwrap().enabled);
        assert!(!bar.find("quick").unwrap().enabled);
    }

    #[test]
    fn builder_chaining() {
        let b = BarButton::new("x", "X", BarButtonKind::QuickLaunch)
            .with_primary("do-x")
            .with_options(vec![opt("o1", "one")])
            .with_badge("3");
        assert_eq!(b.primary, "do-x");
        assert_eq!(b.badge, "3");
        assert_eq!(b.options.len(), 1);
        assert!(b.enabled, "enabled by default");
    }
}
