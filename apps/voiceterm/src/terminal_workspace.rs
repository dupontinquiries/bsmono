//! v0.1.743: native terminal workspace model — multiple tabs, each a tree of
//! split panes, voice-addressable. Today terminal multiplexing is delegated to
//! zellij/tmux (zellij_voice.rs translates phrases to their CLIs). This module
//! is the NATIVE model so the app can own tabs + panes directly: each pane is a
//! PTY session, tabs group panes, panes split horizontally/vertically in a
//! binary layout tree. The vocabulary matches the established zellij_voice
//! verbs (new/close pane, focus direction, new/next/prev/go-to/close/rename
//! tab) so voice users get the same commands without an external multiplexer.
//!
//! Pure: this owns the LAYOUT + session metadata (ids, names, focus, split
//! tree). The actual PTY spawn/teardown is the caller's job — it reacts to the
//! pane ids this model creates/removes. No QML, no I/O.

use std::collections::{VecDeque, BTreeSet};

/// Direction for splits and focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir { Left, Right, Up, Down }

impl Dir {
    pub fn from_word(s: &str) -> Option<Dir> {
        match s.trim().to_lowercase().as_str() {
            "left" => Some(Dir::Left),
            "right" => Some(Dir::Right),
            "up" | "above" => Some(Dir::Up),
            "down" | "below" => Some(Dir::Down),
            _ => None,
        }
    }
    fn is_horizontal(self) -> bool { matches!(self, Dir::Left | Dir::Right) }
}

/// Orientation of a split node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orient {
    /// children side by side (a vertical divider between them)
    H,
    /// children stacked top/bottom
    V,
}

/// A node in a tab's layout tree: either a leaf Pane (owns a session) or a
/// Split with two children.
/// v0.1.797: what a pane hosts. Terminal is the default; Web carries the page
/// URL. This is the model layer for "web tabs living in panes next to terminal
/// panes" — the render path (a web view in the split layout) comes next.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneKind {
    Terminal,
    Web { url: String },
}
impl Default for PaneKind {
    fn default() -> Self { PaneKind::Terminal }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Pane { id: u64, kind: PaneKind },
    Split { orient: Orient, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    /// All pane ids in left-to-right / top-to-bottom order.
    pub fn pane_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }
    fn collect(&self, out: &mut Vec<u64>) {
        match self {
            Node::Pane { id, .. } => out.push(*id),
            Node::Split { a, b, .. } => { a.collect(out); b.collect(out); }
        }
    }
    fn count(&self) -> usize {
        match self {
            Node::Pane { .. } => 1,
            Node::Split { a, b, .. } => a.count() + b.count(),
        }
    }
    /// v0.1.797: kind of the pane with `target` id, if present in this subtree.
    fn find_kind(&self, target: u64) -> Option<&PaneKind> {
        match self {
            Node::Pane { id, kind } if *id == target => Some(kind),
            Node::Pane { .. } => None,
            Node::Split { a, b, .. } => a.find_kind(target).or_else(|| b.find_kind(target)),
        }
    }
    /// v0.1.797: set the kind of pane `target`; returns true if it was found.
    fn set_kind(&mut self, target: u64, k: PaneKind) -> bool {
        match self {
            Node::Pane { id, kind } if *id == target => { *kind = k; true }
            Node::Pane { .. } => false,
            Node::Split { a, b, .. } => {
                if a.set_kind(target, k.clone()) { true } else { b.set_kind(target, k) }
            }
        }
    }
    /// Replace the leaf pane `target` with a split of (target, new_pane).
    /// Returns true if the target was found.
    fn split_leaf(&mut self, target: u64, new_id: u64, dir: Dir) -> bool {
        match self {
            Node::Pane { id, kind } if *id == target => {
                let orient = if dir.is_horizontal() { Orient::H } else { Orient::V };
                // v0.1.797: the original pane keeps its kind (a web pane stays a
                // web pane when split); the freshly-spawned pane is a terminal.
                let existing = Node::Pane { id: target, kind: kind.clone() };
                let added = Node::Pane { id: new_id, kind: PaneKind::Terminal };
                // new pane goes right/down → b; left/up → a
                let (a, b) = match dir {
                    Dir::Right | Dir::Down => (existing, added),
                    Dir::Left | Dir::Up => (added, existing),
                };
                *self = Node::Split { orient, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
                true
            }
            Node::Pane { .. } => false,
            Node::Split { a, b, .. } => a.split_leaf(target, new_id, dir) || b.split_leaf(target, new_id, dir),
        }
    }
    /// Remove leaf pane `target`, collapsing its parent split into the sibling.
    /// Returns true if removed. Won't remove the last remaining pane.
    fn remove_leaf(&mut self, target: u64) -> bool {
        if self.count() <= 1 { return false; }
        // is one of the direct children the target leaf?
        if let Node::Split { a, b, .. } = self {
            if matches!(a.as_ref(), Node::Pane { id, .. } if *id == target) {
                *self = (**b).clone();
                return true;
            }
            if matches!(b.as_ref(), Node::Pane { id, .. } if *id == target) {
                *self = (**a).clone();
                return true;
            }
            return a.remove_leaf(target) || b.remove_leaf(target);
        }
        false
    }

    /// Compute the rectangle of every pane given the tab's full area. Splits
    /// divide their rect by `ratio` along their orientation (H = side-by-side,
    /// V = stacked). This is what the QML render needs (pane geometry) and what
    /// geometric focus/resize operate on. v0.1.744.
    fn rects(&self, r: Rect, out: &mut Vec<(u64, Rect)>) {
        match self {
            Node::Pane { id, .. } => out.push((*id, r)),
            Node::Split { orient, ratio, a, b } => {
                let ratio = ratio.clamp(0.05, 0.95);
                match orient {
                    Orient::H => {
                        let aw = r.w * ratio;
                        a.rects(Rect { x: r.x, y: r.y, w: aw, h: r.h }, out);
                        b.rects(Rect { x: r.x + aw, y: r.y, w: r.w - aw, h: r.h }, out);
                    }
                    Orient::V => {
                        let ah = r.h * ratio;
                        a.rects(Rect { x: r.x, y: r.y, w: r.w, h: ah }, out);
                        b.rects(Rect { x: r.x, y: r.y + ah, w: r.w, h: r.h - ah }, out);
                    }
                }
            }
        }
    }

    /// Adjust the ratio of the split that directly parents leaf `target`, in
    /// the given direction, by `delta` (fraction). Returns true if a matching
    /// split was found and adjusted. Used by resize.
    fn resize_parent(&mut self, target: u64, dir: Dir, delta: f32) -> bool {
        if let Node::Split { orient, ratio, a, b } = self {
            let a_is = matches!(a.as_ref(), Node::Pane { id, .. } if *id == target);
            let b_is = matches!(b.as_ref(), Node::Pane { id, .. } if *id == target);
            // only the split whose orientation matches the resize axis responds
            let axis_ok = (orient == &Orient::H && dir.is_horizontal())
                || (orient == &Orient::V && !dir.is_horizontal());
            if (a_is || b_is) && axis_ok {
                // "resize <dir>" moves the divider toward <dir>. Moving the
                // divider left/up lowers `ratio` (a's share); right/down raises
                // it. So the pane on the far side of the divider from <dir>
                // grows. This is independent of which child is focused.
                let toward_low = matches!(dir, Dir::Left | Dir::Up);
                let sign = if toward_low { -1.0 } else { 1.0 };
                *ratio = (*ratio + sign * delta).clamp(0.05, 0.95);
                return true;
            }
            return a.resize_parent(target, dir, delta) || b.resize_parent(target, dir, delta);
        }
        false
    }
}

/// A pane rectangle in the tab's coordinate space (0..w, 0..h or any unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

impl Rect {
    fn cx(&self) -> f32 { self.x + self.w * 0.5 }
    fn cy(&self) -> f32 { self.y + self.h * 0.5 }
}

/// One tab: a named layout tree plus which pane is focused.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub name: String,
    pub root: Node,
    pub focused_pane: u64,
    /// v0.1.790: pinned tabs are exempt from LRU bubbling (they stay put).
    pub pinned: bool,
    /// v0.1.792: monotonic use stamp — bumped on every switch to this tab.
    /// Drives "bubble all tabs" (sort each region most-recently-used first).
    pub last_used: u64,
}

/// v0.1.789: tab auto-ordering mode (the "bubble" feature). v1 ships Sticky +
/// Lru; Fence (a barrier tab that partitions bubbling into regions) is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabOrderMode {
    /// Insertion order. New tabs append to the right; tabs never auto-move.
    Sticky,
    /// On each explicit switch, the chosen tab bubbles ONE step toward the hot
    /// (left) end, so the tabs you use most migrate left over time.
    Lru,
}
impl Default for TabOrderMode {
    fn default() -> Self { TabOrderMode::Sticky }
}

/// The whole terminal workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct TermWorkspace {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    next_pane_id: u64,
    /// v0.1.789: how tabs auto-reorder. Default Sticky (no movement).
    pub order_mode: TabOrderMode,
    /// v0.1.790: fence boundaries. A value `b` means a barrier sits between
    /// tab[b-1] and tab[b]; LRU bubbling can't cross it. Lightweight "I'm
    /// working here" dividers — no names, no groups.
    pub fences: BTreeSet<usize>,
    /// v0.1.792: monotonic counter for recency stamps (see Tab.last_used).
    use_tick: u64,
    /// v0.1.792: snapshots of (tabs, active, fences) captured before a bulk
    /// reorder, for "rollback my tabs one step". Capped; LIFO.
    undo_stack: Vec<(Vec<Tab>, usize, BTreeSet<usize>)>,
}

impl Default for TermWorkspace {
    fn default() -> Self { Self::new() }
}

impl TermWorkspace {
    /// A fresh workspace with one tab holding one pane (id 0).
    pub fn new() -> Self {
        TermWorkspace {
            tabs: vec![Tab { name: "1".into(), root: Node::Pane { id: 0, kind: PaneKind::Terminal }, focused_pane: 0, pinned: false, last_used: 0 }],
            active_tab: 0,
            next_pane_id: 1,
            order_mode: TabOrderMode::Sticky, fences: BTreeSet::new(), use_tick: 0, undo_stack: Vec::new(),
        }
    }

    fn alloc_pane(&mut self) -> u64 { let id = self.next_pane_id; self.next_pane_id += 1; id }

    pub fn active(&self) -> &Tab { &self.tabs[self.active_tab] }
    pub fn active_mut(&mut self) -> &mut Tab { let i = self.active_tab; &mut self.tabs[i] }

    /// v0.1.763: every pane id across ALL tabs (for restore — spawn a PTY each).
    pub fn pane_ids_all(&self) -> Vec<u64> {
        self.tabs.iter().flat_map(|t| t.root.pane_ids()).collect()
    }

    // ── tabs ──
    /// New tab with a single pane; becomes active. Returns the new pane id.
    pub fn new_tab(&mut self) -> u64 {
        let id = self.alloc_pane();
        let name = (self.tabs.len() + 1).to_string();
        self.tabs.push(Tab { name, root: Node::Pane { id, kind: PaneKind::Terminal }, focused_pane: id, pinned: false, last_used: 0 });
        self.active_tab = self.tabs.len() - 1;
        id
    }
    /// Close the active tab (won't close the last tab). Returns the pane ids
    /// that were in it (so the caller can tear down their PTYs).
    pub fn close_tab(&mut self) -> Vec<u64> {
        if self.tabs.len() <= 1 { return Vec::new(); }
        let r = self.active_tab;
        let removed = self.tabs.remove(r);
        if self.active_tab >= self.tabs.len() { self.active_tab = self.tabs.len() - 1; }
        self.shift_fences_after_close(r); // v0.1.790: keep barriers consistent
        removed.root.pane_ids()
    }
    pub fn next_tab(&mut self) { self.active_tab = (self.active_tab + 1) % self.tabs.len(); self.stamp_active(); }
    pub fn prev_tab(&mut self) {
        self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        self.stamp_active();
    }
    /// Jump to a 1-based tab number. Clamps; returns true if it moved.
    pub fn goto_tab(&mut self, one_based: usize) -> bool {
        if one_based == 0 || one_based > self.tabs.len() { return false; }
        self.active_tab = one_based - 1;
        self.stamp_active();
        // v0.1.789: LRU bubble — an explicit switch nudges the chosen tab one
        // step toward the hot (left) end. Sticky mode leaves order untouched.
        if self.order_mode == TabOrderMode::Lru {
            self.bubble_active_left();
        }
        true
    }
    /// v0.1.789/790: move the active tab one slot toward the left (hot) end,
    /// keeping active_tab pointing at it. Blocked by: a pinned active tab, a
    /// fence at the boundary it would cross, or a pinned left neighbour. Pane
    /// ids are stable, so only tab order + the active index change.
    fn bubble_active_left(&mut self) {
        let i = self.active_tab;
        if i == 0 { return; }
        if self.tabs[i].pinned { return; }
        if self.fences.contains(&i) { return; }   // fence between tab[i-1] and tab[i]
        if self.tabs[i - 1].pinned { return; }
        self.tabs.swap(i, i - 1);
        self.active_tab = i - 1;
    }
    /// v0.1.790: toggle a fence on the RIGHT of tab `idx` (boundary idx+1).
    /// No-op for the last tab — a trailing fence separates nothing.
    pub fn toggle_fence_after(&mut self, idx: usize) {
        let b = idx + 1;
        if b == 0 || b >= self.tabs.len() { return; }
        if !self.fences.insert(b) { self.fences.remove(&b); }
    }
    /// v0.1.790: toggle the pinned flag on tab `idx`.
    pub fn toggle_pin(&mut self, idx: usize) {
        if let Some(t) = self.tabs.get_mut(idx) { t.pinned = !t.pinned; }
    }
    /// v0.1.790: after removing tab `r`, fix up fence boundaries: the two
    /// boundaries that flanked the removed tab merge (a fence survives if
    /// either side had one); boundaries to the right shift down by one;
    /// invalid boundaries are dropped. Call AFTER the tab is removed.
    fn shift_fences_after_close(&mut self, r: usize) {
        let merged = self.fences.contains(&r) || self.fences.contains(&(r + 1));
        let mut nf = BTreeSet::new();
        for &b in &self.fences {
            if b < r { nf.insert(b); }
            else if b == r || b == r + 1 { if merged { nf.insert(r); } }
            else { nf.insert(b - 1); }
        }
        let len = self.tabs.len();
        self.fences = nf.into_iter().filter(|&b| b >= 1 && b < len).collect();
    }
    /// v0.1.792: bump the recency stamp on the active tab.
    fn stamp_active(&mut self) {
        self.use_tick += 1;
        let i = self.active_tab;
        if let Some(t) = self.tabs.get_mut(i) { t.last_used = self.use_tick; }
    }
    /// v0.1.792: capture (tabs, active, fences) for one-step rollback. Capped at
    /// 16 deep; oldest dropped.
    fn push_undo(&mut self) {
        self.undo_stack.push((self.tabs.clone(), self.active_tab, self.fences.clone()));
        if self.undo_stack.len() > 16 { self.undo_stack.remove(0); }
    }
    /// v0.1.792: undo the last bulk tab reorder. Returns true if it restored.
    pub fn rollback_tabs(&mut self) -> bool {
        if let Some((tabs, active, fences)) = self.undo_stack.pop() {
            self.tabs = tabs;
            self.active_tab = active.min(self.tabs.len().saturating_sub(1));
            self.fences = fences;
            true
        } else { false }
    }
    /// v0.1.792: "bubble all tabs" — sort each fenced region most-recently-used
    /// first, keeping pinned tabs in their slots and fences intact. Snapshots
    /// first so "rollback my tabs" can revert. Returns the tab count touched.
    pub fn bubble_all(&mut self) -> usize {
        let n = self.tabs.len();
        if n < 2 { return 0; }
        self.push_undo();
        let active_pane = self.tabs[self.active_tab].focused_pane;
        // Region boundaries: 0, each fence, n.
        let mut bounds: Vec<usize> = vec![0];
        bounds.extend(self.fences.iter().copied());
        bounds.push(n);
        bounds.sort_unstable();
        bounds.dedup();
        for w in bounds.windows(2) {
            let (s, e) = (w[0], w[1]);
            if e - s < 2 { continue; }
            let region: Vec<Tab> = self.tabs[s..e].to_vec();
            let mut unpinned: Vec<Tab> =
                region.iter().filter(|t| !t.pinned).cloned().collect();
            // most-recently-used first; stable so ties keep their order.
            unpinned.sort_by(|a, b| b.last_used.cmp(&a.last_used));
            let mut it = unpinned.into_iter();
            for (k, t) in region.iter().enumerate() {
                self.tabs[s + k] = if t.pinned { t.clone() } else { it.next().unwrap() };
            }
        }
        self.active_tab = self.tabs.iter()
            .position(|t| t.focused_pane == active_pane).unwrap_or(0);
        n
    }
    /// v0.1.795: move the tabs at `indices` to the front (hot end), preserving
    /// their relative order; the rest shift right keeping theirs. Used by the
    /// keyword-bubble ("bubble tabs with ssh"). Snapshots first so it's
    /// rollback-able. Clears fences (the gather reorganises across regions, so
    /// the old boundaries are stale — rollback restores them). Active follows
    /// its pane. Returns the number of tabs moved.
    pub fn gather_to_front(&mut self, indices: &[usize]) -> usize {
        let n = self.tabs.len();
        let mut idx: Vec<usize> = indices.iter().copied().filter(|&i| i < n).collect();
        idx.sort_unstable();
        idx.dedup();
        if idx.is_empty() || idx.len() == n { return 0; }
        self.push_undo();
        self.fences.clear();
        let active_pane = self.tabs[self.active_tab].focused_pane;
        let in_set: BTreeSet<usize> = idx.iter().copied().collect();
        let mut matching: Vec<Tab> = Vec::new();
        let mut rest: Vec<Tab> = Vec::new();
        for (i, t) in std::mem::take(&mut self.tabs).into_iter().enumerate() {
            if in_set.contains(&i) { matching.push(t); } else { rest.push(t); }
        }
        let moved = matching.len();
        self.tabs = matching;
        self.tabs.extend(rest);
        self.active_tab = self.tabs.iter()
            .position(|t| t.focused_pane == active_pane).unwrap_or(0);
        moved
    }
    /// v0.1.797: kind of the pane with `id` (searches every tab). None if no
    /// such pane exists.
    pub fn pane_kind(&self, id: u64) -> Option<&PaneKind> {
        self.tabs.iter().find_map(|t| t.root.find_kind(id))
    }
    /// v0.1.826: URLs of every web pane in the ACTIVE tab, in pane order. Backs
    /// the voice-context piping — the assistant gets the content of the pages
    /// currently open in the tab the user is looking at.
    pub fn active_tab_web_urls(&self) -> Vec<String> {
        let tab = self.active();
        tab.root
            .pane_ids()
            .iter()
            .filter_map(|id| match tab.root.find_kind(*id) {
                Some(PaneKind::Web { url }) => Some(url.clone()),
                _ => None,
            })
            .collect()
    }
    /// v0.1.797: set the kind of pane `id` (searches every tab). True if set.
    pub fn set_pane_kind(&mut self, id: u64, k: PaneKind) -> bool {
        for t in self.tabs.iter_mut() {
            if t.root.set_kind(id, k.clone()) { return true; }
        }
        false
    }
    /// v0.1.797: turn the active tab's focused pane into a web pane showing
    /// `url`; returns its pane id. (Render path draws a web view for Web panes.)
    pub fn set_active_pane_web(&mut self, url: &str) -> u64 {
        let id = self.active().focused_pane;
        self.set_pane_kind(id, PaneKind::Web { url: url.to_string() });
        id
    }
    pub fn rename_tab(&mut self, name: &str) {
        let n = name.trim();
        if !n.is_empty() { self.active_mut().name = n.to_string(); }
    }
    /// Find a tab by name (case-insensitive, exact then prefix) for voice
    /// "switch to <name>". Returns its index.
    pub fn find_tab(&self, query: &str) -> Option<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() { return None; }
        self.tabs.iter().position(|t| t.name.to_lowercase() == q)
            .or_else(|| self.tabs.iter().position(|t| t.name.to_lowercase().starts_with(&q)))
            .or_else(|| self.tabs.iter().position(|t| t.name.to_lowercase().contains(&q)))
    }

    // ── panes ──
    /// Split the focused pane of the active tab. New pane becomes focused.
    /// Returns the new pane id.
    pub fn split(&mut self, dir: Dir) -> u64 {
        let new_id = self.alloc_pane();
        let target = self.active().focused_pane;
        let t = self.active_mut();
        if t.root.split_leaf(target, new_id, dir) {
            t.focused_pane = new_id;
        }
        new_id
    }
    /// v0.1.798: split the active pane and make the NEW pane a web pane showing
    /// `url`. Returns the new pane id. Terminal focus is KEPT on the original
    /// pane (so main.rs's active-terminal stays valid and the web pane needs no
    /// pty — it renders the page, not a shell).
    /// v0.1.798: close a pane by id (any tab), regardless of focus — needed
    /// for web panes, which aren't focusable yet. Won't remove the last pane.
    /// Returns true if a pane was removed.
    pub fn close_pane_id(&mut self, id: u64) -> bool {
        for t in self.tabs.iter_mut() {
            if t.root.remove_leaf(id) { return true; }
        }
        false
    }
    pub fn split_web(&mut self, dir: Dir, url: &str) -> u64 {
        let prev_focus = self.active().focused_pane;
        let new_id = self.split(dir);
        self.set_pane_kind(new_id, PaneKind::Web { url: url.to_string() });
        self.active_mut().focused_pane = prev_focus;
        new_id
    }
    /// v0.1.881: close the newest web pane in the active tab (highest id).
    /// Web panes are never the focused pane (split_web restores focus to the
    /// prior pane), so `close_pane` can't reach them — this is what lets
    /// Ctrl+Shift+W close a web pane. Returns the closed id, or None when
    /// there are no web panes (caller then falls back to close_pane).
    pub fn close_newest_web_pane(&mut self) -> Option<u64> {
        let mut web_ids: Vec<u64> = self
            .active()
            .root
            .pane_ids()
            .into_iter()
            .filter(|id| matches!(self.active().root.find_kind(*id), Some(PaneKind::Web { .. })))
            .collect();
        web_ids.sort_unstable();
        let target = *web_ids.last()?;
        if self.close_pane_id(target) {
            Some(target)
        } else {
            None
        }
    }
    /// Close the focused pane (won't close the last pane in the last tab; if
    /// it's the last pane of a non-last tab, closes the tab). Returns the
    /// removed pane id, if any.
    pub fn close_pane(&mut self) -> Option<u64> {
        let target = self.active().focused_pane;
        if self.active().root.count() <= 1 {
            // last pane in this tab → close the tab instead (if not last tab)
            if self.tabs.len() > 1 {
                self.close_tab();
                return Some(target);
            }
            return None;
        }
        let t = self.active_mut();
        if t.root.remove_leaf(target) {
            // focus the first remaining pane
            t.focused_pane = t.root.pane_ids()[0];
            Some(target)
        } else {
            None
        }
    }
    /// Pane rectangles for the active tab, given the tab area (w×h). The QML
    /// render lays panes out from these; geometric focus/resize use them.
    pub fn pane_rects(&self, w: f32, h: f32) -> Vec<(u64, Rect)> {
        let mut out = Vec::new();
        self.active().root.rects(Rect { x: 0.0, y: 0.0, w, h }, &mut out);
        out
    }

    /// Move focus geometrically: pick the nearest pane whose center lies in the
    /// requested direction from the focused pane (true spatial movement, what a
    /// tiling WM does). Falls back to no-op if none in that direction. v0.1.744
    /// (replaces the v743 cyclic placeholder).
    pub fn focus(&mut self, dir: Dir) {
        let rects = self.pane_rects(1.0, 1.0); // unit space — only ratios matter
        if rects.len() <= 1 { return; }
        let cur_id = self.active().focused_pane;
        let Some(&(_, cur)) = rects.iter().find(|(id, _)| *id == cur_id) else { return };
        let (ccx, ccy) = (cur.cx(), cur.cy());
        let mut best: Option<(u64, f32)> = None;
        for &(id, r) in &rects {
            if id == cur_id { continue; }
            let (dx, dy) = (r.cx() - ccx, r.cy() - ccy);
            // must be in the requested half-plane
            let in_dir = match dir {
                Dir::Left => dx < -1e-4,
                Dir::Right => dx > 1e-4,
                Dir::Up => dy < -1e-4,
                Dir::Down => dy > 1e-4,
            };
            if !in_dir { continue; }
            // distance: primary axis weighted so we prefer the closest in-line
            // neighbor, with cross-axis as a tiebreak.
            let dist = match dir {
                Dir::Left | Dir::Right => dx.abs() + dy.abs() * 0.5,
                Dir::Up | Dir::Down => dy.abs() + dx.abs() * 0.5,
            };
            if best.map_or(true, |(_, bd)| dist < bd) { best = Some((id, dist)); }
        }
        if let Some((id, _)) = best { self.active_mut().focused_pane = id; }
    }

    /// Resize the focused pane in a direction by `delta` (fraction of its
    /// parent split). Adjusts the nearest enclosing split whose orientation
    /// matches the axis. v0.1.744.
    pub fn resize(&mut self, dir: Dir, delta: f32) -> bool {
        let target = self.active().focused_pane;
        self.active_mut().root.resize_parent(target, dir, delta)
    }

    /// Total pane count across all tabs (for status / "how many sessions").
    pub fn total_panes(&self) -> usize { self.tabs.iter().map(|t| t.root.count()).sum() }

    /// A compact wire description for QML: one line per tab,
    /// `idx \x1F name \x1F pane_count \x1F active(0/1) \x1F focused_pane`.
    pub fn to_wire(&self) -> String {
        let mut s = String::new();
        for (i, t) in self.tabs.iter().enumerate() {
            // v0.1.790: append pinned + fence_after (fence on the right of tab i
            // = a barrier at boundary i+1). Older fields unchanged.
            let fence_after = if self.fences.contains(&(i + 1)) { 1 } else { 0 };
            s.push_str(&format!("{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\n",
                i, t.name, t.root.count(),
                if i == self.active_tab { 1 } else { 0 }, t.focused_pane,
                if t.pinned { 1 } else { 0 }, fence_after));
        }
        s
    }

    /// Serialize the layout for session persistence: one line per tab as
    /// `name \x1F flattened_tree`, where the tree is a parenthesized form
    /// (`H(0,V(1,2))`). Restorable across runs so a Hector session keeps its
    /// tabs + splits. Pane ids are NOT preserved literally (the caller respawns
    /// PTYs on restore) — restore reassigns fresh ids but keeps the SHAPE.
    pub fn snapshot(&self) -> String {
        let mut s = format!("active\u{1F}{}\n", self.active_tab);
        for t in &self.tabs {
            s.push_str(&t.name);
            s.push('\u{1F}');
            s.push_str(&serialize_node(&t.root));
            // v0.1.763: persist the focused pane as its INDEX in pane order
            // (ids are reassigned on restore, but order is stable). A 3rd
            // \x1F field; older snapshots without it restore focus to pane 0.
            let fidx = t.root.pane_ids().iter().position(|&p| p == t.focused_pane).unwrap_or(0);
            s.push('\u{1F}');
            s.push_str(&fidx.to_string());
            s.push('\n');
        }
        s
    }

    /// Rebuild a workspace from snapshot(), reassigning fresh pane ids but
    /// preserving the tab names + split shapes + ratios + focused pane.
    /// Returns None on malformed input.
    pub fn restore(text: &str) -> Option<TermWorkspace> {
        let mut ws = TermWorkspace { tabs: Vec::new(), active_tab: 0, next_pane_id: 0, order_mode: TabOrderMode::Sticky, fences: BTreeSet::new(), use_tick: 0, undo_stack: Vec::new() };
        let mut active = 0usize;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("active\u{1F}") {
                active = rest.trim().parse().unwrap_or(0);
                continue;
            }
            let mut parts = line.split('\u{1F}');
            let name = parts.next()?.to_string();
            let tree = parts.next()?;
            // optional focus-index field (v0.1.763); default 0 for old snapshots
            let focus_idx: usize = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let root = parse_node(tree, &mut ws.next_pane_id)?;
            let ids = root.pane_ids();
            let focused = ids.get(focus_idx).or_else(|| ids.first()).copied().unwrap_or(0);
            ws.tabs.push(Tab { name, root, focused_pane: focused, pinned: false, last_used: 0 });
        }
        if ws.tabs.is_empty() { return None; }
        ws.active_tab = active.min(ws.tabs.len() - 1);
        Some(ws)
    }
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
fn serialize_node(n: &Node) -> String {
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
fn parse_node(s: &str, next: &mut u64) -> Option<Node> {
    let s = s.trim();
    if s == "P" {
        let id = *next; *next += 1;
        return Some(Node::Pane { id, kind: PaneKind::Terminal });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_one_tab_one_pane() {
        let ws = TermWorkspace::new();
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.total_panes(), 1);
        assert_eq!(ws.active().focused_pane, 0);
    }

    #[test]
    fn new_tab_activates_and_unique_panes() {
        let mut ws = TermWorkspace::new();
        let p = ws.new_tab();
        assert_eq!(ws.tabs.len(), 2);
        assert_eq!(ws.active_tab, 1);
        assert_eq!(ws.active().focused_pane, p);
        assert_ne!(p, 0);
    }

    #[test]
    fn split_creates_pane_and_focuses_it() {
        let mut ws = TermWorkspace::new();
        let p = ws.split(Dir::Right);
        assert_eq!(ws.active().root.count(), 2);
        assert_eq!(ws.active().focused_pane, p);
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
        assert_ne!(ws.active().focused_pane, w1);
        assert_ne!(ws.active().focused_pane, w2);
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
        assert_eq!(ws.active().focused_pane, left_id);
        ws.focus(Dir::Right);
        assert_eq!(ws.active().focused_pane, p_right);
        // focus up/down does nothing in a purely horizontal split
        ws.focus(Dir::Up);
        assert_eq!(ws.active().focused_pane, p_right);
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
        let other = *ids.iter().find(|&&i| i != ws.active().focused_pane).unwrap();
        apply(&mut ws, &WsCommand::FocusPane(other));
        assert_eq!(ws.active().focused_pane, other, "focus should move to the tapped pane id");
        // a non-existent id is a no-op (doesn't crash or change focus)
        let cur = ws.active().focused_pane;
        apply(&mut ws, &WsCommand::FocusPane(99999));
        assert_eq!(ws.active().focused_pane, cur, "unknown id must not change focus");
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
        let r = TermWorkspace::restore(&snap).expect("restore");
        assert_eq!(r.tabs.len(), 2);
        assert_eq!(r.tabs[0].root.count(), 3);   // shape preserved
        assert_eq!(r.tabs[1].name, "logs");
        assert_eq!(r.active_tab, 0);
        // restored shape serializes identically (ids differ but shape same)
        assert_eq!(serialize_node(&r.tabs[0].root), serialize_node(&ws.tabs[0].root));
    }

    #[test]
    fn restore_rejects_garbage() {
        assert!(TermWorkspace::restore("").is_none());
        assert!(TermWorkspace::restore("name\u{1F}Q(bad)").is_none());
    }

    #[test]
    fn restored_panes_have_unique_ids() {
        let mut ws = TermWorkspace::new();
        ws.split(Dir::Right);
        let r = TermWorkspace::restore(&ws.snapshot()).unwrap();
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
        let restored = TermWorkspace::restore(&ws.snapshot()).expect("should parse");
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
        let restored = TermWorkspace::restore(&ws.snapshot()).unwrap();
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
            .position(|&p| p == ws.active().focused_pane).unwrap();
        let restored = TermWorkspace::restore(&ws.snapshot()).unwrap();
        let focus_idx_after = restored.active().root.pane_ids().iter()
            .position(|&p| p == restored.active().focused_pane).unwrap();
        assert_eq!(focus_idx_before, focus_idx_after, "focused pane index must survive");
    }

    #[test]
    fn restore_tolerates_old_snapshot_without_ratio_or_focus() {
        // pre-v763 format: no :ratio, no focus-index field
        let old = "active\u{1F}0\ndefault\u{1F}H(P,V(P,P))";
        let ws = TermWorkspace::restore(old).expect("old format still parses");
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].root.count(), 3);
        // default ratio applied
        if let Node::Split { ratio, .. } = &ws.tabs[0].root {
            assert!((ratio - 0.5).abs() < 0.01);
        } else { panic!("expected split"); }
    }
}
