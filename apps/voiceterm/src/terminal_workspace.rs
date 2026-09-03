//! Native terminal workspace: tabs of split panes, voice-addressable.
//!
//! v0.1.743 grew the layout model here; it has since moved to
//! `shared/pane-kit`, because aperture needs the same tree with a
//! different payload and two copies of a tiling model is one copy that
//! drifts. What stays here is what is genuinely voiceterm's:
//!
//! * [`PaneKind`] — a pane holds a PTY session or a web page.
//! * The spoken/keyboard command vocabulary ([`WsCommand`],
//!   [`parse_command`], [`apply`]), matching zellij_voice's verbs.
//! * The QML wire format and the session snapshot, which stores tree
//!   SHAPE only and reassigns pane ids on restore because the caller
//!   respawns PTYs — deliberately not pane-kit's id-preserving
//!   `snapshot_with`.
//! * The web-pane helpers, which exist because web panes are never
//!   focused and so cannot be reached by focus-based commands.
//!
//! Those hang off [`TermPanes`], an extension trait: inherent methods
//! can only be added in the crate that defines the type, so app-specific
//! operations on a shared type have to arrive this way. That is a
//! feature — the trait is exactly the list of things voiceterm needs
//! that a generic pane model should not know about.
//!
//! Pure: this owns LAYOUT plus session metadata. PTY spawn/teardown is
//! the caller's job, reacting to the pane ids this model creates and
//! removes.

use std::collections::VecDeque;

pub use pane_kit::{Dir, Node, Orient, Rect, Tab, TabOrderMode, Workspace};

/// What a pane hosts. Terminal is the default, so a fresh pane from a
/// split is a shell.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum PaneKind {
    #[default]
    Terminal,
    Web { url: String },
}

/// voiceterm's workspace: panes hold [`PaneKind`].
pub type TermWorkspace = Workspace<PaneKind>;

/// voiceterm-specific operations over a pane workspace.
pub trait TermPanes {
    fn pane_kind(&self, id: u64) -> Option<&PaneKind>;
    fn set_pane_kind(&mut self, id: u64, k: PaneKind) -> bool;
    fn active_tab_web_urls(&self) -> Vec<String>;
    fn set_active_pane_web(&mut self, url: &str) -> u64;
    fn split_web(&mut self, dir: Dir, url: &str) -> u64;
    fn close_newest_web_pane(&mut self) -> Option<u64>;
    fn close_web_or_focused_pane(&mut self) -> Option<u64>;
    fn to_wire(&self) -> String;
    fn snapshot(&self) -> String;
}

impl TermPanes for TermWorkspace {
    fn pane_kind(&self, id: u64) -> Option<&PaneKind> {
        self.payload(id)
    }
    fn set_pane_kind(&mut self, id: u64, k: PaneKind) -> bool {
        self.set_payload(id, k)
    }

    /// URLs of every web pane in the ACTIVE tab, in pane order. Backs
    /// the voice-context piping — the assistant gets the pages open in
    /// the tab the user is looking at.
    fn active_tab_web_urls(&self) -> Vec<String> {
        self.active()
            .root
            .panes()
            .into_iter()
            .filter_map(|(_, k)| match k {
                PaneKind::Web { url } => Some(url.clone()),
                _ => None,
            })
            .collect()
    }

    fn set_active_pane_web(&mut self, url: &str) -> u64 {
        let id = self.focused_pane();
        self.set_pane_kind(id, PaneKind::Web { url: url.to_string() });
        id
    }

    /// Split and make the NEW pane a web pane. Focus stays on the
    /// ORIGINAL pane: main.rs's active-terminal must stay valid, and a
    /// web pane needs no pty — it renders a page, not a shell.
    fn split_web(&mut self, dir: Dir, url: &str) -> u64 {
        let prev_focus = self.focused_pane();
        let new_id = self.split(dir);
        self.set_pane_kind(new_id, PaneKind::Web { url: url.to_string() });
        self.active_mut().focused_pane = prev_focus;
        new_id
    }

    /// Close the newest web pane in the active tab (highest id).
    ///
    /// Web panes are never focused (see `split_web`), so `close_pane`
    /// cannot reach them. NEWEST rather than first-in-layout is what the
    /// original does and what its tests pin.
    fn close_newest_web_pane(&mut self) -> Option<u64> {
        let mut web_ids: Vec<u64> = self
            .active()
            .root
            .panes()
            .into_iter()
            .filter(|(_, k)| matches!(k, PaneKind::Web { .. }))
            .map(|(id, _)| id)
            .collect();
        web_ids.sort_unstable();
        let target = *web_ids.last()?;
        self.close_pane_id(target).then_some(target)
    }

    /// A web pane if the active tab has one, else the focused pane.
    /// Two frontends each remembering to fall back is one frontend that
    /// forgets, and the forgetting is silent: close-pane simply stops
    /// working when no web pane is open.
    fn close_web_or_focused_pane(&mut self) -> Option<u64> {
        self.close_newest_web_pane().or_else(|| self.close_pane())
    }

    /// Compact wire description for QML, one line per tab:
    /// `idx \x1F name \x1F pane_count \x1F active \x1F focused_pane
    /// \x1F pinned \x1F fence_after`.
    fn to_wire(&self) -> String {
        let mut s = String::new();
        for (i, t) in self.tabs.iter().enumerate() {
            let fence_after = if self.fences.contains(&(i + 1)) { 1 } else { 0 };
            s.push_str(&format!(
                "{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\n",
                i,
                t.name,
                t.root.count(),
                if i == self.active_tab { 1 } else { 0 },
                t.focused_pane,
                if t.pinned { 1 } else { 0 },
                fence_after
            ));
        }
        s
    }

    /// Session persistence: one line per tab as
    /// `name \x1F flattened_tree \x1F focus_index`.
    ///
    /// Pane ids are NOT preserved — the caller respawns PTYs on restore,
    /// so a stored id would name a process that no longer exists. Focus
    /// is stored as an INDEX in pane order, which is stable across the
    /// reassignment.
    fn snapshot(&self) -> String {
        let mut s = format!("active\u{1F}{}\n", self.active_tab);
        for t in &self.tabs {
            let fidx = t.root.pane_ids().iter().position(|&p| p == t.focused_pane).unwrap_or(0);
            s.push_str(&format!("{}\u{1F}{}\u{1F}{}\n", t.name, serialize_node(&t.root), fidx));
        }
        s
    }
}

/// Rebuild from [`TermPanes::snapshot`], reassigning fresh pane ids but
/// keeping tab names, split shapes, ratios and focus.
pub fn restore(text: &str) -> Option<TermWorkspace> {
    let mut next: u64 = 0;
    let mut tabs: Vec<Tab<PaneKind>> = Vec::new();
    let mut active = 0usize;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("active\u{1F}") {
            active = rest.trim().parse().unwrap_or(0);
            continue;
        }
        let mut parts = line.split('\u{1F}');
        let name = parts.next()?.to_string();
        let tree = parts.next()?;
        // optional focus-index field; older snapshots default to 0
        let focus_idx: usize = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let root = parse_node(tree, &mut next)?;
        let ids = root.pane_ids();
        let focused = ids.get(focus_idx).or_else(|| ids.first()).copied().unwrap_or(0);
        tabs.push(Tab { name, root, focused_pane: focused, pinned: false, last_used: 0 });
    }
    Workspace::from_parts(tabs, active, next)
}

fn serialize_node(n: &Node<PaneKind>) -> String {
    match n {
        Node::Pane { .. } => "P".to_string(),
        Node::Split { orient, ratio, a, b } => {
            let o = if matches!(orient, Orient::H) { 'H' } else { 'V' };
            // v0.1.763: include the split ratio (`H:0.50(...)`) so resizes
            // survive a restart. Older snapshots without `:ratio` still parse
            // (parse_node defaults to 0.5).
            format!("{}:{:.2}({},{})", o, ratio, serialize_node(a), serialize_node(b))
        }
    }
}

/// Parse the flattened form, allocating fresh pane ids from `next`.
fn parse_node(s: &str, next: &mut u64) -> Option<Node<PaneKind>> {
    let s = s.trim();
    if s == "P" {
        let id = *next; *next += 1;
        return Some(Node::Pane { id, payload: PaneKind::Terminal });
    }
    let orient = match s.chars().next()? {
        'H' => Orient::H,
        'V' => Orient::V,
        _ => return None,
    };
    // v0.1.763: optional `:<ratio>` after the orient char (older snapshots
    // omit it → default 0.5). Strip it before the parenthesized children.
    let after_orient = &s[1..];
    let (ratio, rest) = if let Some(r) = after_orient.strip_prefix(':') {
        let pidx = r.find('(')?;
        let ratio: f32 = r[..pidx].parse().ok()?;
        (ratio.clamp(0.05, 0.95), &r[pidx..])
    } else {
        (0.5, after_orient)
    };
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    // split at the top-level comma (depth 0)
    let mut depth = 0i32;
    let mut split_at = None;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => { split_at = Some(i); break; }
            _ => {}
        }
    }
    let i = split_at?;
    let a = parse_node(&inner[..i], next)?;
    let b = parse_node(&inner[i + 1..], next)?;
    Some(Node::Split { orient, ratio, a: Box::new(a), b: Box::new(b) })
}

fn word_to_num(s: &str) -> Option<usize> {
    match s {
        "one" | "first" => Some(1),
        "two" | "second" => Some(2),
        "three" | "third" => Some(3),
        "four" | "fourth" => Some(4),
        "five" | "fifth" => Some(5),
        "six" | "sixth" => Some(6),
        "seven" | "seventh" => Some(7),
        "eight" | "eighth" => Some(8),
        "nine" | "ninth" => Some(9),
        _ => None,
    }
}

/// Apply a parsed command to the workspace. Returns pane ids the caller must
/// tear down (for closes). For list/switch-by-name the caller reads state.
pub fn apply(ws: &mut TermWorkspace, cmd: &WsCommand) -> VecDeque<u64> {
    let mut removed = VecDeque::new();
    match cmd {
        WsCommand::NewTab => { ws.new_tab(); }
        WsCommand::CloseTab => { for id in ws.close_tab() { removed.push_back(id); } }
        WsCommand::NextTab => ws.next_tab(),
        WsCommand::PrevTab => ws.prev_tab(),
        WsCommand::GotoTab(n) => { ws.goto_tab(*n); }
        WsCommand::RenameTab(name) => ws.rename_tab(name),
        WsCommand::SwitchToTab(name) => { if let Some(i) = ws.find_tab(name) { ws.active_tab = i; } }
        WsCommand::NewPane(d) => { ws.split(*d); }
        WsCommand::ClosePane => { if let Some(id) = ws.close_pane() { removed.push_back(id); } }
        WsCommand::Focus(d) => ws.focus(*d),
        WsCommand::FocusPane(id) => {
            // only switch if the id is a real pane in the active tab
            if ws.active().root.pane_ids().contains(id) {
                ws.active_mut().focused_pane = *id;
            }
        }
        WsCommand::Resize(d) => { ws.resize(*d, 0.1); }
        WsCommand::ListTabs | WsCommand::ListPanes => {}
        WsCommand::BubbleAll => { ws.bubble_all(); }
        WsCommand::RollbackTabs => { ws.rollback_tabs(); }
    }
    removed
}

/// v0.1.793: deterministic identicon spec from a seed (a tab's
/// (cwd, foreground-command) tuple). Returns (hue 0..359, mask) where the low
/// 15 bits of `mask` fill a 5x5 left-right-symmetric grid: bit `r*3 + c` (c in
/// 0..3) sets the cell at row r, columns c AND 4-c. Pure + stable, so the same
/// context always draws the same icon. The hue gives each tab a colour; the
/// grid gives it a shape.
pub fn identicon_spec(seed: &str) -> (u16, u32) {
    // FNV-1a 64-bit.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in seed.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hue = ((h >> 40) % 360) as u16;
    let mask = (h & 0x7FFF) as u32; // 15 independent cells
    (hue, mask)
}

/// A parsed voice/keyboard command over the workspace. Maps the established
/// zellij_voice vocabulary to native ops.
#[derive(Debug, Clone, PartialEq)]
pub enum WsCommand {
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    GotoTab(usize),
    RenameTab(String),
    SwitchToTab(String),
    NewPane(Dir),
    ClosePane,
    Focus(Dir),
    /// v0.1.762: focus a pane by id (tap-to-focus in the split render).
    FocusPane(u64),
    Resize(Dir),
    ListTabs,
    ListPanes,
    /// v0.1.792: sort each fenced region by recency (bulk "bubble all tabs").
    BubbleAll,
    /// v0.1.792: undo the last bulk tab reorder ("rollback my tabs one step").
    RollbackTabs,
}

/// Parse a phrase into a workspace command (English). Returns None if it isn't
/// a workspace command (so the caller can fall through to other intents).
pub fn parse_command(phrase: &str) -> Option<WsCommand> {
    let p = phrase.trim().to_lowercase();
    let p = p.trim_end_matches(['.', '!', '?']).trim();

    // panes first (more specific "new pane" before "new tab"). v0.1.752/755:
    // orientation words map to a direction — horizontal => side-by-side
    // (Right), vertical => stacked (Down), per tmux -h/-v. Filler ("this pane",
    // "the", "window") is stripped.
    if let Some(rest) = p.strip_prefix("new pane").or_else(|| p.strip_prefix("split")) {
        let r = rest.trim()
            .trim_start_matches("this").trim()
            .trim_start_matches("the").trim()
            .trim_start_matches("pane").trim()
            .trim_start_matches("window").trim();
        let d = if r.starts_with("horizontal") {
            Dir::Right
        } else if r.starts_with("vertical") {
            Dir::Down
        } else {
            Dir::from_word(r).unwrap_or(Dir::Right)
        };
        return Some(WsCommand::NewPane(d));
    }
    if matches!(p,
        "close pane" | "kill pane" | "close this pane" | "close the pane"
        | "kill this pane" | "kill the pane" | "close window" | "close this window"
        | "close the window") {
        return Some(WsCommand::ClosePane);
    }
    // v0.1.762: "focus pane <id>" (tap-to-focus). Checked before directional.
    if let Some(rest) = p.strip_prefix("focus pane ") {
        if let Ok(id) = rest.trim().parse::<u64>() {
            return Some(WsCommand::FocusPane(id));
        }
    }
    // v0.1.781: directional pane focus. The bare "focus <dir>" still works, but
    // accept natural variants too: "focus the left pane", "go to the right pane",
    // "move to up window", "switch to the down pane", "left pane". To avoid
    // stealing bare nav words (a lone "left"/"up" the browser/scroll layer may
    // want), require EITHER a leading "focus" verb or the word pane/window in the
    // phrase. ("focus pane" with no direction stays None — handled above by the
    // id form; the trims below reduce it to "" which from_word rejects.)
    {
        let has_pane = p.contains("pane") || p.contains("window");
        let mut r: &str = p;
        let mut focus_verb = false;
        for v in ["focus", "go to", "move to", "switch to", "select"] {
            if let Some(rest) = r.strip_prefix(v) { focus_verb = v == "focus"; r = rest.trim(); break; }
        }
        if focus_verb || has_pane {
            let r = r.trim_start_matches("the").trim()
                     .trim_end_matches("window").trim()
                     .trim_end_matches("pane").trim()
                     .trim_start_matches("the").trim();
            if let Some(d) = Dir::from_word(r) {
                return Some(WsCommand::Focus(d));
            }
        }
    }
    if let Some(rest) = p.strip_prefix("resize") {
        if let Some(d) = Dir::from_word(rest.trim()) {
            return Some(WsCommand::Resize(d));
        }
    }

    if matches!(p,
        "new tab" | "open new tab" | "open tab" | "open a tab" | "open a new tab"
        | "create tab" | "create a tab" | "create new tab" | "create a new tab"
        | "make a tab" | "make a new tab" | "add a tab" | "add tab"
        | "another tab" | "new terminal tab") {
        return Some(WsCommand::NewTab);
    }
    if p == "close tab" || p == "kill tab" { return Some(WsCommand::CloseTab); }
    if p == "next tab" || p == "next" { return Some(WsCommand::NextTab); }
    if p == "previous tab" || p == "prev tab" || p == "last tab" { return Some(WsCommand::PrevTab); }
    // v0.1.792: bulk tab-order ops. "bubble all tabs" sorts each region by
    // recency; "rollback my tabs" undoes the last bulk reorder.
    if matches!(p, "bubble all tabs" | "bubble tabs" | "bubble all my tabs"
                 | "bubble my tabs" | "organize tabs" | "organize my tabs"
                 | "bubble all the tabs") {
        return Some(WsCommand::BubbleAll);
    }
    if matches!(p, "rollback tabs" | "rollback my tabs" | "rollback my tabs one step"
                 | "rollback tabs one step" | "roll back my tabs" | "roll back tabs"
                 | "undo tab order" | "undo my tabs" | "undo tab bubble") {
        return Some(WsCommand::RollbackTabs);
    }
    // v0.1.781: natural next/prev with a leading verb + optional filler:
    // "go to the next tab", "switch to next tab", "move to previous tab",
    // "next one", "the previous one". Runs after the exact forms above and
    // before the GotoTab/SwitchToTab matchers so it doesn't get read as a tab
    // NAME ("next") or number.
    {
        let t = p.trim_start_matches("go to").trim_start_matches("switch to")
                 .trim_start_matches("move to").trim_start_matches("flip to").trim()
                 .trim_start_matches("the").trim()
                 .trim_end_matches("tab").trim().trim_end_matches("one").trim();
        match t {
            "next" => return Some(WsCommand::NextTab),
            "previous" | "prev" | "last" => return Some(WsCommand::PrevTab),
            _ => {}
        }
    }
    if p == "list tabs" || p == "what tabs" { return Some(WsCommand::ListTabs); }
    if p == "list panes" || p == "what panes" { return Some(WsCommand::ListPanes); }

    if let Some(rest) = p.strip_prefix("go to tab").or_else(|| p.strip_prefix("tab")) {
        let rest = rest.trim();
        if let Ok(n) = rest.parse::<usize>() { return Some(WsCommand::GotoTab(n)); }
        if let Some(n) = word_to_num(rest) { return Some(WsCommand::GotoTab(n)); }
    }
    if let Some(rest) = p.strip_prefix("rename tab").or_else(|| p.strip_prefix("name tab")) {
        let name = rest.trim();
        if !name.is_empty() { return Some(WsCommand::RenameTab(name.to_string())); }
    }
    if let Some(rest) = p.strip_prefix("switch to").or_else(|| p.strip_prefix("go to")) {
        // v0.1.781: strip filler ("tab"/"the" leading, "session"/"tab" trailing)
        // as whole WORDS via tokenization. The old trim_*_matches stripped them
        // as substrings, mangling names that merely contained them — e.g.
        // "switch to mysession" became SwitchToTab("my").
        let mut toks: Vec<&str> = rest.split_whitespace().collect();
        while matches!(toks.first(), Some(&"tab") | Some(&"the")) { toks.remove(0); }
        while matches!(toks.last(), Some(&"session") | Some(&"tab")) { toks.pop(); }
        let name = toks.join(" ");
        if !name.is_empty() && name.parse::<usize>().is_err() {
            return Some(WsCommand::SwitchToTab(name));
        }
    }
    None
}

/// Flatten a layout node: leaf → "P", split → "H(a,b)" / "V(a,b)".
#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use super::TermPanes as _;

    #[test]
    fn starts_with_one_tab_one_pane() {
        let ws = TermWorkspace::new();
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.total_panes(), 1);
        assert_eq!(ws.focused_pane(), 0);
    }

    #[test]
    fn new_tab_activates_and_unique_panes() {
        let mut ws = TermWorkspace::new();
        let p = ws.new_tab();
        assert_eq!(ws.tabs.len(), 2);
        assert_eq!(ws.active_tab, 1);
        assert_eq!(ws.focused_pane(), p);
        assert_ne!(p, 0);
    }

    #[test]
    fn split_creates_pane_and_focuses_it() {
        let mut ws = TermWorkspace::new();
        let p = ws.split(Dir::Right);
        assert_eq!(ws.active().root.count(), 2);
        assert_eq!(ws.focused_pane(), p);
        // split orientation horizontal for left/right
        assert!(matches!(ws.active().root, Node::Split { orient: Orient::H, .. }));
    }

    #[test]
    fn split_down_is_vertical() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Down);
        assert!(matches!(ws.active().root, Node::Split { orient: Orient::V, .. }));
    }

    #[test]
    fn nested_splits_track_all_panes() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);  // 2 panes
        ws.split(Dir::Down);   // 3 panes (split the focused one)
        assert_eq!(ws.active().root.count(), 3);
        assert_eq!(ws.active().root.pane_ids().len(), 3);
    }

    #[test]
    fn close_pane_collapses_split() {
        let mut ws = TermWorkspace::new();
        let p2 = ws.split(Dir::Right);
        assert_eq!(ws.active().root.count(), 2);
        let removed = ws.close_pane();
        assert_eq!(removed, Some(p2));
        assert_eq!(ws.active().root.count(), 1);
        assert!(matches!(ws.active().root, Node::Pane { .. }));
    }

    #[test]
    fn cannot_close_last_pane_of_last_tab() {
        let mut ws = TermWorkspace::new();
        assert_eq!(ws.close_pane(), None);
        assert_eq!(ws.total_panes(), 1);
    }

    #[test]
    fn close_newest_web_pane_targets_web_not_terminal() {
        let mut ws = TermWorkspace::new();
        // terminal pane (root) + two web panes
        let w1 = ws.split_web(Dir::Right, "https://a.example");
        let w2 = ws.split_web(Dir::Right, "https://b.example");
        assert_eq!(ws.active().root.count(), 3);
        // focus was restored to the original terminal pane, not the web panes
        assert_ne!(ws.focused_pane(), w1);
        assert_ne!(ws.focused_pane(), w2);
        // closes the NEWEST web pane (highest id), leaving the terminal + w1
        assert_eq!(ws.close_newest_web_pane(), Some(w2.max(w1)));
        assert_eq!(ws.active().root.count(), 2);
        // again → the remaining web pane
        assert_eq!(ws.close_newest_web_pane(), Some(w1.min(w2)));
        assert_eq!(ws.active().root.count(), 1);
        // none left → None (and the terminal pane survives)
        assert_eq!(ws.close_newest_web_pane(), None);
        assert_eq!(ws.active().root.count(), 1);
    }

    #[test]
    fn close_last_pane_of_extra_tab_closes_tab() {
        let mut ws = TermWorkspace::new();
        ws.new_tab(); // tab 2, single pane, active
        assert_eq!(ws.tabs.len(), 2);
        ws.close_pane(); // last pane of tab 2 → closes tab
        assert_eq!(ws.tabs.len(), 1);
    }

    #[test]
    fn tab_navigation_wraps() {
        let mut ws = TermWorkspace::new();
        ws.new_tab(); ws.new_tab(); // 3 tabs, active=2
        ws.next_tab(); assert_eq!(ws.active_tab, 0);
        ws.prev_tab(); assert_eq!(ws.active_tab, 2);
        assert!(ws.goto_tab(2)); assert_eq!(ws.active_tab, 1);
        assert!(!ws.goto_tab(9));
    }

    #[test]
    fn close_tab_never_removes_last() {
        let mut ws = TermWorkspace::new();
        assert!(ws.close_tab().is_empty());
        assert_eq!(ws.tabs.len(), 1);
    }

    #[test]
    fn rename_and_find_tab() {
        let mut ws = TermWorkspace::new();
        ws.new_tab();
        ws.rename_tab("deploy");
        assert_eq!(ws.active().name, "deploy");
        assert_eq!(ws.find_tab("deploy"), Some(1));
        assert_eq!(ws.find_tab("dep"), Some(1));     // prefix
        assert_eq!(ws.find_tab("ploy"), Some(1));    // contains
        assert_eq!(ws.find_tab("nope"), None);
    }

    #[test]
    fn focus_moves_geometrically() {
        // split right → two panes side by side. Focus is on the right (new)
        // pane; "focus left" should land on the left pane, "focus right" back.
        let mut ws = TermWorkspace::new();
        let p_right = ws.split(Dir::Right);
        let rects = ws.pane_rects(100.0, 100.0);
        let left_id = rects.iter().find(|(id, _)| *id != p_right).unwrap().0;
        ws.focus(Dir::Left);
        assert_eq!(ws.focused_pane(), left_id);
        ws.focus(Dir::Right);
        assert_eq!(ws.focused_pane(), p_right);
        // focus up/down does nothing in a purely horizontal split
        ws.focus(Dir::Up);
        assert_eq!(ws.focused_pane(), p_right);
    }

    #[test]
    fn pane_rects_tile_without_overlap() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right); // two halves
        let rects = ws.pane_rects(100.0, 100.0);
        assert_eq!(rects.len(), 2);
        let total: f32 = rects.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((total - 100.0 * 100.0).abs() < 1.0, "rects should tile the area");
        // side by side: same height, widths sum to 100
        assert!((rects[0].1.h - 100.0).abs() < 0.1);
        assert!(((rects[0].1.w + rects[1].1.w) - 100.0).abs() < 0.1);
    }

    #[test]
    fn resize_adjusts_split_ratio() {
        let mut ws = TermWorkspace::new();
        let _r = ws.split(Dir::Right); // focus on right pane
        let before = ws.pane_rects(100.0, 100.0);
        let right_w_before = before.iter().max_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap()).unwrap().1.w;
        // grow the focused (right) pane leftward → it gets wider
        assert!(ws.resize(Dir::Left, 0.2));
        let after = ws.pane_rects(100.0, 100.0);
        let right_w_after = after.iter().max_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap()).unwrap().1.w;
        assert!(right_w_after > right_w_before, "focused pane should grow: {} -> {}", right_w_before, right_w_after);
    }

    #[test]
    fn focus_pane_by_id_switches_focus() {
        let mut ws = TermWorkspace::new();
        apply(&mut ws, &WsCommand::NewPane(Dir::Right)); // now 2 panes; focus on new
        let ids = ws.active().root.pane_ids();
        assert_eq!(ids.len(), 2);
        let other = *ids.iter().find(|&&i| i != ws.focused_pane()).unwrap();
        apply(&mut ws, &WsCommand::FocusPane(other));
        assert_eq!(ws.focused_pane(), other, "focus should move to the tapped pane id");
        // a non-existent id is a no-op (doesn't crash or change focus)
        let cur = ws.focused_pane();
        apply(&mut ws, &WsCommand::FocusPane(99999));
        assert_eq!(ws.focused_pane(), cur, "unknown id must not change focus");
    }

    #[test]
    fn parse_pane_and_tab_commands() {
        assert_eq!(parse_command("new pane right"), Some(WsCommand::NewPane(Dir::Right)));
        assert_eq!(parse_command("new pane"), Some(WsCommand::NewPane(Dir::Right)));
        assert_eq!(parse_command("split down"), Some(WsCommand::NewPane(Dir::Down)));
        assert_eq!(parse_command("close pane"), Some(WsCommand::ClosePane));
        assert_eq!(parse_command("focus left"), Some(WsCommand::Focus(Dir::Left)));
        assert_eq!(parse_command("focus pane 3"), Some(WsCommand::FocusPane(3)));
        assert_eq!(parse_command("focus pane"), None); // needs an id
        assert_eq!(parse_command("resize up"), Some(WsCommand::Resize(Dir::Up)));
        assert_eq!(parse_command("new tab"), Some(WsCommand::NewTab));
        assert_eq!(parse_command("next tab"), Some(WsCommand::NextTab));
        assert_eq!(parse_command("go to tab 3"), Some(WsCommand::GotoTab(3)));
        assert_eq!(parse_command("go to tab three"), Some(WsCommand::GotoTab(3)));
        assert_eq!(parse_command("rename tab deploy"), Some(WsCommand::RenameTab("deploy".into())));
        assert_eq!(parse_command("switch to deploy"), Some(WsCommand::SwitchToTab("deploy".into())));
        assert_eq!(parse_command("what's for lunch"), None);
    }

    /// v0.1.781: richer natural phrasings for the existing pane/tab commands
    /// (the user flagged voice command richness as the weak spot), plus the
    /// whole-word filter fix for SwitchToTab.
    #[test]
    fn parse_natural_voice_phrasings() {
        use WsCommand::*;
        // directional focus tolerance
        assert_eq!(parse_command("focus the left pane"), Some(Focus(Dir::Left)));
        assert_eq!(parse_command("go to the right pane"), Some(Focus(Dir::Right)));
        assert_eq!(parse_command("move to up window"), Some(Focus(Dir::Up)));
        assert_eq!(parse_command("switch to the down pane"), Some(Focus(Dir::Down)));
        assert_eq!(parse_command("left pane"), Some(Focus(Dir::Left)));
        // a bare direction with neither a focus verb nor a pane word is NOT stolen
        assert_eq!(parse_command("left"), None);
        // close pane filler
        assert_eq!(parse_command("close the pane"), Some(ClosePane));
        assert_eq!(parse_command("close this pane"), Some(ClosePane));
        // new tab synonyms
        assert_eq!(parse_command("create a tab"), Some(NewTab));
        assert_eq!(parse_command("make a new tab"), Some(NewTab));
        assert_eq!(parse_command("another tab"), Some(NewTab));
        // verb-prefixed next/prev
        assert_eq!(parse_command("go to the next tab"), Some(NextTab));
        assert_eq!(parse_command("switch to next tab"), Some(NextTab));
        assert_eq!(parse_command("move to previous tab"), Some(PrevTab));
        assert_eq!(parse_command("switch to next"), Some(NextTab));
        // SwitchToTab filler is stripped by WHOLE WORD, not substring
        assert_eq!(parse_command("switch to mysession"), Some(SwitchToTab("mysession".into())));
        assert_eq!(parse_command("switch to the build session"), Some(SwitchToTab("build".into())));
        assert_eq!(parse_command("switch to tab logs"), Some(SwitchToTab("logs".into())));
        // non-pane/tab phrases still fall through
        assert_eq!(parse_command("scroll down"), None);
        assert_eq!(parse_command("select all"), None);
    }

    #[test]
    fn apply_close_returns_panes_to_teardown() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);
        let removed = apply(&mut ws, &WsCommand::ClosePane);
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn switch_to_tab_by_name_via_apply() {
        let mut ws = TermWorkspace::new();
        ws.new_tab();
        ws.rename_tab("build");
        ws.active_tab = 0; // move away
        apply(&mut ws, &WsCommand::SwitchToTab("build".into()));
        assert_eq!(ws.active_tab, 1);
    }

    #[test]
    fn snapshot_restore_preserves_shape() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);      // tab1: H(P,P)
        ws.split(Dir::Down);       // tab1: H(P,V(P,P))
        ws.new_tab();              // tab2: P
        ws.rename_tab("logs");
        ws.active_tab = 0;
        let snap = ws.snapshot();
        let r = restore(&snap).expect("restore");
        assert_eq!(r.tabs.len(), 2);
        assert_eq!(r.tabs[0].root.count(), 3);   // shape preserved
        assert_eq!(r.tabs[1].name, "logs");
        assert_eq!(r.active_tab, 0);
        // restored shape serializes identically (ids differ but shape same)
        assert_eq!(serialize_node(&r.tabs[0].root), serialize_node(&ws.tabs[0].root));
    }

    #[test]
    fn restore_rejects_garbage() {
        assert!(restore("").is_none());
        assert!(restore("name\u{1F}Q(bad)").is_none());
    }

    #[test]
    fn restored_panes_have_unique_ids() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);
        let r = restore(&ws.snapshot()).unwrap();
        let ids = r.tabs[0].root.pane_ids();
        let uniq: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), uniq.len(), "pane ids must be unique after restore");
    }

    #[test]
    fn wire_format_marks_active() {
        let mut ws = TermWorkspace::new();
        ws.new_tab();
        let wire = ws.to_wire();
        let lines: Vec<&str> = wire.lines().collect();
        assert_eq!(lines.len(), 2);
        // tab 1 (idx 1) is active
        assert!(lines[1].contains("\u{1F}1\u{1F}")); // pane_count=1 ... active=1
        assert!(lines[1].split('\u{1F}').nth(3) == Some("1"), "tab 2 active flag");
        assert!(lines[0].split('\u{1F}').nth(3) == Some("0"), "tab 1 not active");
    }

    // ── snapshot persistence (v0.1.763) ──
    // ── snapshot persistence enhancements (v0.1.763): ratios + focus ──
    #[test]
    fn snapshot_round_trips_complex_layout() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);          // tab 0: two panes side by side
        ws.split(Dir::Down);           // split the focused (right) pane
        ws.new_tab();                  // tab 1: single pane
        ws.rename_tab("logs");
        ws.split(Dir::Left);
        ws.active_tab = 0;             // make tab 0 active
        let restored = restore(&ws.snapshot()).expect("should parse");
        // shape preserved (ids are reassigned, so compare structure/counts)
        assert_eq!(restored.tabs.len(), ws.tabs.len());
        assert_eq!(restored.active_tab, ws.active_tab);
        for (a, b) in restored.tabs.iter().zip(ws.tabs.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.root.count(), b.root.count(), "pane count per tab");
        }
    }

    #[test]
    fn snapshot_preserves_ratios() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);
        ws.resize(Dir::Right, 0.2);    // shift the split ratio off 0.5
        let before = match &ws.active().root {
            Node::Split { ratio, .. } => *ratio, _ => panic!("expected split"),
        };
        let restored = restore(&ws.snapshot()).unwrap();
        let after = match &restored.active().root {
            Node::Split { ratio, .. } => *ratio, _ => panic!("expected split"),
        };
        assert!((before - after).abs() < 0.02, "ratio not preserved: {} vs {}", before, after);
    }

    #[test]
    fn snapshot_preserves_focused_pane() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);          // focus on the new (2nd) pane
        ws.focus(Dir::Left);           // focus the FIRST pane
        let focus_idx_before = ws.active().root.pane_ids().iter()
            .position(|&p| p == ws.focused_pane()).unwrap();
        let restored = restore(&ws.snapshot()).unwrap();
        let focus_idx_after = restored.active().root.pane_ids().iter()
            .position(|&p| p == restored.active().focused_pane).unwrap();
        assert_eq!(focus_idx_before, focus_idx_after, "focused pane index must survive");
    }

    #[test]
    fn restore_tolerates_old_snapshot_without_ratio_or_focus() {
        // pre-v763 format: no :ratio, no focus-index field
        let old = "active\u{1F}0\ndefault\u{1F}H(P,V(P,P))";
        let ws = restore(old).expect("old format still parses");
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].root.count(), 3);
        // default ratio applied
        if let Node::Split { ratio, .. } = &ws.tabs[0].root {
            assert!((ratio - 0.5).abs() < 0.01);
        } else { panic!("expected split"); }
    }
}

#[cfg(test)]
mod close_web_tests {
    use super::*;
    #[allow(unused_imports)]
    use super::TermPanes as _;

    /// A tab with a terminal pane and a web pane beside it.
    fn split_with_web() -> TermWorkspace {
        let mut w = TermWorkspace::default();
        let term = w.active().focused_pane;
        // split_web restores focus to the prior pane itself — its own doc says
        // so — which is why web panes are never focused and why close_pane
        // cannot reach them. Asserted rather than forced, so the fixture
        // notices if that ever changes.
        w.split_web(Dir::Right, "https://x");
        assert_eq!(w.active().focused_pane, term,
                   "split_web left focus on the web pane; the premise of this whole path is \
that it does not");
        w
    }

    #[test]
    fn closing_prefers_the_web_pane_over_the_focused_terminal() {
        // v0.1.881. Without this, opening a web pane and pressing close-pane
        // shuts the terminal you were working in and leaves the web pane
        // sitting there — the opposite of what the key was pressed for.
        let mut w = split_with_web();
        let focused = w.active().focused_pane;
        let closed = w.close_web_or_focused_pane().expect("nothing closed");
        assert_ne!(closed, focused, "it closed the focused terminal");
        assert!(w.active_tab_web_urls().is_empty(), "the web pane survived");
        assert!(w.active().root.pane_ids().contains(&focused), "the terminal was destroyed");
    }

    #[test]
    fn with_no_web_pane_it_falls_back_to_the_focused_one() {
        // The fallback is why this is a wrapper rather than a branch in the
        // caller: two frontends would each have to remember it.
        let mut w = TermWorkspace::default();
        w.split(Dir::Right);
        let focused = w.active().focused_pane;
        let closed = w.close_web_or_focused_pane().expect("nothing closed");
        assert_eq!(closed, focused);
    }

    #[test]
    fn the_last_pane_of_a_tab_still_closes_the_tab() {
        // close_pane's documented behaviour, which the fallback must not lose.
        let mut w = TermWorkspace::default();
        w.new_tab();
        assert_eq!(w.tabs.len(), 2);
        w.close_web_or_focused_pane();
        assert_eq!(w.tabs.len(), 1, "the last pane did not close its tab");
    }

    #[test]
    fn it_refuses_rather_than_emptying_the_workspace() {
        // One tab, one pane: there is nothing to close that leaves a usable
        // workspace, and returning None lets the caller say so.
        let mut w = TermWorkspace::default();
        assert!(w.close_web_or_focused_pane().is_none());
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.active().root.pane_ids().len(), 1);
    }
}
