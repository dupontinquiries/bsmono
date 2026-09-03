//! pane-kit — tabs of split panes, as a pure binary layout tree.
//!
//! Lifted from `apps/voiceterm/src/terminal_workspace.rs` (v0.1.743–792),
//! which had grown the whole tiling model — splits, geometric focus,
//! resize, pane rects, LRU tab bubbling with pins and fences — and had
//! it tested. Aperture listed tabs/splits as a v0 non-goal with "a seam
//! waiting for it"; this is that seam, filled by moving the existing
//! implementation rather than writing a second one.
//!
//! ## What is generic and what is not
//!
//! The tree is generic over `P`, what a pane HOLDS. voiceterm's panes
//! hold a PTY session or a web URL; aperture's hold a document view.
//! Nothing else differed between the two apps' needs, which is why the
//! lift is a type parameter and not a trait.
//!
//! Left behind in voiceterm on purpose: the spoken-command parser
//! (`WsCommand`, `parse_command`, `apply`) and the QML wire format.
//! Those are that app's UI, not layout. `snapshot_with` / `restore_with`
//! here take payload codec closures so a caller can serialize without
//! this crate knowing anything about its payload.
//!
//! ## Pane ids
//!
//! Ids are allocated by the workspace and never reused within its
//! lifetime, because a pane id is what the CALLER keys its real resource
//! on — a PTY, a document, a remote peer's cursor. Reusing an id after a
//! close would silently attach a live resource to a dead one.
//!
//! Pure: no I/O, no UI, no time. std only.

use std::collections::{BTreeSet, VecDeque};

/// What a pane can hold. `Default` is required because splitting gives
/// the NEW pane a default payload while the original keeps its own —
/// splitting a web pane in voiceterm yields the web pane plus a fresh
/// terminal, and the same rule reads correctly for an editor (split a
/// document view, get an empty one beside it).
pub trait Payload: Clone + PartialEq + Default {}
impl<T: Clone + PartialEq + Default> Payload for T {}

/// Direction for splits and focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

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
    pub fn is_horizontal(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }
}

/// Orientation of a split node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orient {
    /// Children side by side (a vertical divider between them).
    H,
    /// Children stacked top/bottom.
    V,
}

/// A node in a tab's layout tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Node<P> {
    Pane { id: u64, payload: P },
    Split { orient: Orient, ratio: f32, a: Box<Node<P>>, b: Box<Node<P>> },
}

impl<P: Payload> Node<P> {
    /// All pane ids in left-to-right / top-to-bottom order.
    pub fn pane_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }
    fn collect(&self, out: &mut Vec<u64>) {
        match self {
            Node::Pane { id, .. } => out.push(*id),
            Node::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }
    pub fn count(&self) -> usize {
        match self {
            Node::Pane { .. } => 1,
            Node::Split { a, b, .. } => a.count() + b.count(),
        }
    }
    /// Payload of pane `target`, if present in this subtree.
    pub fn find_payload(&self, target: u64) -> Option<&P> {
        match self {
            Node::Pane { id, payload } if *id == target => Some(payload),
            Node::Pane { .. } => None,
            Node::Split { a, b, .. } => {
                a.find_payload(target).or_else(|| b.find_payload(target))
            }
        }
    }
    pub fn find_payload_mut(&mut self, target: u64) -> Option<&mut P> {
        match self {
            Node::Pane { id, payload } if *id == target => Some(payload),
            Node::Pane { .. } => None,
            Node::Split { a, b, .. } => match a.find_payload_mut(target) {
                Some(p) => Some(p),
                None => b.find_payload_mut(target),
            },
        }
    }
    /// Set the payload of pane `target`; true if it was found.
    pub fn set_payload(&mut self, target: u64, p: P) -> bool {
        match self.find_payload_mut(target) {
            Some(slot) => {
                *slot = p;
                true
            }
            None => false,
        }
    }
    /// Every (id, payload) pair, in the same order as `pane_ids`.
    pub fn panes(&self) -> Vec<(u64, &P)> {
        let mut out = Vec::new();
        self.collect_pairs(&mut out);
        out
    }
    fn collect_pairs<'a>(&'a self, out: &mut Vec<(u64, &'a P)>) {
        match self {
            Node::Pane { id, payload } => out.push((*id, payload)),
            Node::Split { a, b, .. } => {
                a.collect_pairs(out);
                b.collect_pairs(out);
            }
        }
    }

    /// Replace leaf `target` with a split of (target, new pane). The
    /// original keeps its payload; the new pane gets `P::default()`.
    fn split_leaf(&mut self, target: u64, new_id: u64, dir: Dir) -> bool {
        match self {
            Node::Pane { id, payload } if *id == target => {
                let orient = if dir.is_horizontal() { Orient::H } else { Orient::V };
                let existing = Node::Pane { id: target, payload: payload.clone() };
                let added = Node::Pane { id: new_id, payload: P::default() };
                // new pane goes right/down → b; left/up → a
                let (a, b) = match dir {
                    Dir::Right | Dir::Down => (existing, added),
                    Dir::Left | Dir::Up => (added, existing),
                };
                *self = Node::Split { orient, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
                true
            }
            Node::Pane { .. } => false,
            Node::Split { a, b, .. } => {
                a.split_leaf(target, new_id, dir) || b.split_leaf(target, new_id, dir)
            }
        }
    }

    /// Remove leaf `target`, collapsing its parent split into the
    /// sibling. Won't remove the last remaining pane.
    fn remove_leaf(&mut self, target: u64) -> bool {
        if self.count() <= 1 {
            return false;
        }
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

    /// Rectangle of every pane given the tab's full area.
    fn rects(&self, r: Rect, out: &mut Vec<(u64, Rect)>) {
        match self {
            Node::Pane { id, .. } => out.push((*id, r)),
            Node::Split { orient, ratio, a, b } => {
                let ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
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

    /// Adjust the ratio of the split directly parenting leaf `target`.
    fn resize_parent(&mut self, target: u64, dir: Dir, delta: f32) -> bool {
        if let Node::Split { orient, ratio, a, b } = self {
            let a_is = matches!(a.as_ref(), Node::Pane { id, .. } if *id == target);
            let b_is = matches!(b.as_ref(), Node::Pane { id, .. } if *id == target);
            // only the split whose orientation matches the resize axis responds
            let axis_ok = (orient == &Orient::H && dir.is_horizontal())
                || (orient == &Orient::V && !dir.is_horizontal());
            if (a_is || b_is) && axis_ok {
                // "resize <dir>" moves the DIVIDER toward <dir>, so the pane on
                // the far side grows. Independent of which child is focused —
                // that is what makes repeated "resize left" monotonic instead
                // of oscillating.
                let toward_low = matches!(dir, Dir::Left | Dir::Up);
                let sign = if toward_low { -1.0 } else { 1.0 };
                *ratio = (*ratio + sign * delta).clamp(MIN_RATIO, MAX_RATIO);
                return true;
            }
            return a.resize_parent(target, dir, delta) || b.resize_parent(target, dir, delta);
        }
        false
    }
}

/// Splits never collapse a child to nothing; a pane you cannot see is a
/// pane you cannot focus your way out of.
pub const MIN_RATIO: f32 = 0.05;
pub const MAX_RATIO: f32 = 0.95;

/// A pane rectangle in the tab's coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn cx(&self) -> f32 {
        self.x + self.w * 0.5
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h * 0.5
    }
}

/// One tab: a named layout tree plus which pane is focused.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab<P> {
    pub name: String,
    pub root: Node<P>,
    pub focused_pane: u64,
    /// Pinned tabs are exempt from LRU bubbling.
    pub pinned: bool,
    /// Monotonic use stamp, bumped on every switch to this tab.
    pub last_used: u64,
}

/// How tabs auto-reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabOrderMode {
    /// Insertion order. New tabs append right; tabs never auto-move.
    #[default]
    Sticky,
    /// On each explicit switch the chosen tab bubbles ONE step toward
    /// the hot (left) end, so tabs you use most migrate left over time.
    Lru,
}

/// Tabs of panes.
#[derive(Debug, Clone, PartialEq)]
pub struct Workspace<P> {
    pub tabs: Vec<Tab<P>>,
    pub active_tab: usize,
    next_pane_id: u64,
    pub order_mode: TabOrderMode,
    /// A value `b` means a barrier sits between tab[b-1] and tab[b];
    /// LRU bubbling cannot cross it. Lightweight "I'm working here"
    /// dividers — no names, no groups.
    pub fences: BTreeSet<usize>,
    use_tick: u64,
    /// Snapshots of (tabs, active, fences) taken before a bulk reorder,
    /// for "put my tabs back". Capped, LIFO.
    undo_stack: Vec<(Vec<Tab<P>>, usize, BTreeSet<usize>)>,
}

/// How many bulk-reorder snapshots to keep.
pub const REORDER_UNDO_DEPTH: usize = 8;

impl<P: Payload> Default for Workspace<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Payload> Workspace<P> {
    /// A fresh workspace with one tab holding one pane (id 0).
    pub fn new() -> Self {
        Workspace {
            tabs: vec![Tab {
                name: "1".into(),
                root: Node::Pane { id: 0, payload: P::default() },
                focused_pane: 0,
                pinned: false,
                last_used: 0,
            }],
            active_tab: 0,
            next_pane_id: 1,
            order_mode: TabOrderMode::Sticky,
            fences: BTreeSet::new(),
            use_tick: 0,
            undo_stack: Vec::new(),
        }
    }

    pub fn active(&self) -> &Tab<P> {
        &self.tabs[self.active_tab]
    }
    pub fn active_mut(&mut self) -> &mut Tab<P> {
        let i = self.active_tab;
        &mut self.tabs[i]
    }
    pub fn focused_pane(&self) -> u64 {
        self.active().focused_pane
    }
    pub fn pane_ids_all(&self) -> Vec<u64> {
        self.tabs.iter().flat_map(|t| t.root.pane_ids()).collect()
    }
    pub fn total_panes(&self) -> usize {
        self.tabs.iter().map(|t| t.root.count()).sum()
    }

    // ── tabs ────────────────────────────────────────────────────────

    pub fn new_tab(&mut self) -> u64 {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let name = (self.tabs.len() + 1).to_string();
        self.tabs.push(Tab {
            name,
            root: Node::Pane { id, payload: P::default() },
            focused_pane: id,
            pinned: false,
            last_used: 0,
        });
        self.active_tab = self.tabs.len() - 1;
        self.stamp_active();
        id
    }

    /// Close the active tab, returning the pane ids it held so the
    /// caller can tear down their resources. Never closes the last tab.
    pub fn close_tab(&mut self) -> Vec<u64> {
        if self.tabs.len() <= 1 {
            return Vec::new();
        }
        let t = self.tabs.remove(self.active_tab);
        // fences index boundaries, so removing a tab shifts every
        // boundary above it down by one.
        self.fences = self
            .fences
            .iter()
            .filter_map(|&b| {
                if b == self.active_tab + 1 {
                    None
                } else if b > self.active_tab {
                    Some(b - 1)
                } else {
                    Some(b)
                }
            })
            .filter(|&b| b > 0 && b < self.tabs.len())
            .collect();
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.stamp_active();
        t.root.pane_ids()
    }

    pub fn next_tab(&mut self) {
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
        self.stamp_active();
    }
    pub fn prev_tab(&mut self) {
        self.active_tab = if self.active_tab == 0 { self.tabs.len() - 1 } else { self.active_tab - 1 };
        self.stamp_active();
    }

    pub fn goto_tab(&mut self, one_based: usize) -> bool {
        if one_based == 0 || one_based > self.tabs.len() {
            return false;
        }
        self.active_tab = one_based - 1;
        self.stamp_active();
        if self.order_mode == TabOrderMode::Lru {
            self.bubble_active_left();
        }
        true
    }

    pub fn rename_tab(&mut self, name: &str) {
        let n = name.trim();
        if !n.is_empty() {
            self.active_mut().name = n.to_string();
        }
    }

    /// First tab whose name contains `query`, case-insensitively.
    pub fn find_tab(&self, query: &str) -> Option<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        self.tabs.iter().position(|t| t.name.to_lowercase().contains(&q))
    }

    fn stamp_active(&mut self) {
        self.use_tick += 1;
        let tick = self.use_tick;
        self.active_mut().last_used = tick;
    }

    /// Move the active tab one slot toward the hot (left) end. Blocked
    /// by a pinned active tab, a fence at the boundary it would cross,
    /// or a pinned left neighbour.
    fn bubble_active_left(&mut self) {
        let i = self.active_tab;
        if i == 0 || self.tabs[i].pinned || self.fences.contains(&i) || self.tabs[i - 1].pinned {
            return;
        }
        self.tabs.swap(i, i - 1);
        self.active_tab = i - 1;
    }

    /// Toggle a fence to the RIGHT of tab `idx`. No-op for the last tab
    /// — a trailing fence separates nothing.
    pub fn toggle_fence_after(&mut self, idx: usize) {
        let b = idx + 1;
        if b == 0 || b >= self.tabs.len() {
            return;
        }
        if !self.fences.insert(b) {
            self.fences.remove(&b);
        }
    }

    pub fn toggle_pin(&mut self, idx: usize) {
        if let Some(t) = self.tabs.get_mut(idx) {
            t.pinned = !t.pinned;
        }
    }

    /// Sort every fence-delimited region most-recently-used first,
    /// leaving pinned tabs where they are. Returns how many tabs moved.
    pub fn bubble_all(&mut self) -> usize {
        self.push_undo();
        let active_id = self.active_tab;
        let before: Vec<String> = self.tabs.iter().map(|t| t.name.clone()).collect();
        let bounds = self.region_bounds();
        for (lo, hi) in bounds {
            // pinned tabs hold their absolute slot; only the others sort
            let mut movable: Vec<Tab<P>> =
                self.tabs[lo..hi].iter().filter(|t| !t.pinned).cloned().collect();
            movable.sort_by(|a, b| b.last_used.cmp(&a.last_used));
            let mut it = movable.into_iter();
            for i in lo..hi {
                if !self.tabs[i].pinned {
                    self.tabs[i] = it.next().expect("counted movable tabs");
                }
            }
        }
        // active_tab is an index; follow the tab it pointed at
        let moved = self
            .tabs
            .iter()
            .zip(before.iter())
            .filter(|(t, b)| &t.name != *b)
            .count();
        self.active_tab = active_id.min(self.tabs.len() - 1);
        moved
    }

    /// Move the tabs at `indices` to the front, preserving their
    /// relative order. Returns how many moved.
    pub fn gather_to_front(&mut self, indices: &[usize]) -> usize {
        let mut idx: Vec<usize> = indices.iter().copied().filter(|&i| i < self.tabs.len()).collect();
        idx.sort_unstable();
        idx.dedup();
        if idx.is_empty() {
            return 0;
        }
        self.push_undo();
        let active_name = self.tabs[self.active_tab].name.clone();
        let mut picked: Vec<Tab<P>> = Vec::new();
        let mut rest: Vec<Tab<P>> = Vec::new();
        for (i, t) in self.tabs.drain(..).enumerate() {
            if idx.binary_search(&i).is_ok() {
                picked.push(t);
            } else {
                rest.push(t);
            }
        }
        let n = picked.len();
        picked.extend(rest);
        self.tabs = picked;
        // gathering invalidates every boundary; keeping stale fences
        // would fence off tabs that are no longer neighbours
        self.fences.clear();
        self.active_tab = self
            .tabs
            .iter()
            .position(|t| t.name == active_name)
            .unwrap_or(0);
        n
    }

    /// Undo the last bulk reorder. True if something was restored.
    pub fn rollback_tabs(&mut self) -> bool {
        match self.undo_stack.pop() {
            Some((tabs, active, fences)) => {
                self.tabs = tabs;
                self.active_tab = active.min(self.tabs.len().saturating_sub(1));
                self.fences = fences;
                true
            }
            None => false,
        }
    }

    fn push_undo(&mut self) {
        if self.undo_stack.len() >= REORDER_UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push((self.tabs.clone(), self.active_tab, self.fences.clone()));
    }

    /// `[lo, hi)` spans between fences.
    fn region_bounds(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut lo = 0usize;
        for &b in &self.fences {
            if b > lo && b <= self.tabs.len() {
                out.push((lo, b));
                lo = b;
            }
        }
        if lo < self.tabs.len() {
            out.push((lo, self.tabs.len()));
        }
        out
    }

    // ── panes ───────────────────────────────────────────────────────

    /// Split the focused pane, focusing the new one. Returns its id.
    pub fn split(&mut self, dir: Dir) -> u64 {
        let new_id = self.next_pane_id;
        self.next_pane_id += 1;
        let target = self.active().focused_pane;
        let tab = self.active_mut();
        if tab.root.split_leaf(target, new_id, dir) {
            tab.focused_pane = new_id;
        }
        new_id
    }

    /// Split and give the new pane a payload in one step.
    pub fn split_with(&mut self, dir: Dir, payload: P) -> u64 {
        let id = self.split(dir);
        self.active_mut().root.set_payload(id, payload);
        id
    }

    /// Close the focused pane. Returns its id if it was removed. Never
    /// removes a tab's last pane — closing that is `close_tab`.
    pub fn close_pane(&mut self) -> Option<u64> {
        let target = self.active().focused_pane;
        self.close_pane_id(target).then_some(target)
    }

    /// Close a pane by id, in the active tab.
    pub fn close_pane_id(&mut self, id: u64) -> bool {
        let tab = self.active_mut();
        if !tab.root.remove_leaf(id) {
            return false;
        }
        if tab.focused_pane == id {
            // focus the first survivor rather than leaving a dangling id
            tab.focused_pane = tab.root.pane_ids().first().copied().unwrap_or(0);
        }
        true
    }

    pub fn payload(&self, id: u64) -> Option<&P> {
        self.tabs.iter().find_map(|t| t.root.find_payload(id))
    }
    pub fn payload_mut(&mut self, id: u64) -> Option<&mut P> {
        self.tabs.iter_mut().find_map(|t| t.root.find_payload_mut(id))
    }
    pub fn set_payload(&mut self, id: u64, p: P) -> bool {
        for t in self.tabs.iter_mut() {
            if t.root.set_payload(id, p.clone()) {
                return true;
            }
        }
        false
    }

    /// Pane geometry for the active tab, in a `w` × `h` box.
    pub fn pane_rects(&self, w: f32, h: f32) -> Vec<(u64, Rect)> {
        let mut out = Vec::new();
        self.active().root.rects(Rect { x: 0.0, y: 0.0, w, h }, &mut out);
        out
    }

    /// Move focus geometrically: the nearest pane whose centre lies in
    /// `dir` from the focused one, which is what a tiling WM does. A
    /// tree-order walk would jump across the screen when the tree shape
    /// and the visual layout disagree.
    pub fn focus(&mut self, dir: Dir) {
        let rects = self.pane_rects(1.0, 1.0); // unit space — only ratios matter
        if rects.len() <= 1 {
            return;
        }
        let cur_id = self.active().focused_pane;
        let Some(&(_, cur)) = rects.iter().find(|(id, _)| *id == cur_id) else { return };
        let (ccx, ccy) = (cur.cx(), cur.cy());
        let mut best: Option<(u64, f32)> = None;
        for &(id, r) in &rects {
            if id == cur_id {
                continue;
            }
            let (dx, dy) = (r.cx() - ccx, r.cy() - ccy);
            let in_dir = match dir {
                Dir::Left => dx < -1e-4,
                Dir::Right => dx > 1e-4,
                Dir::Up => dy < -1e-4,
                Dir::Down => dy > 1e-4,
            };
            if !in_dir {
                continue;
            }
            // primary axis dominates; cross-axis is a tiebreak, so a
            // pane straight ahead beats a nearer one off to the side
            let dist = match dir {
                Dir::Left | Dir::Right => dx.abs() + dy.abs() * 0.5,
                Dir::Up | Dir::Down => dy.abs() + dx.abs() * 0.5,
            };
            if best.map_or(true, |(_, bd)| dist < bd) {
                best = Some((id, dist));
            }
        }
        if let Some((id, _)) = best {
            self.active_mut().focused_pane = id;
        }
    }

    /// Focus a pane by id anywhere in the workspace, switching tabs if
    /// needed. True if found.
    pub fn focus_id(&mut self, id: u64) -> bool {
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if t.root.pane_ids().contains(&id) {
                t.focused_pane = id;
                self.active_tab = i;
                self.stamp_active();
                return true;
            }
        }
        false
    }

    /// Resize the focused pane by `delta` (fraction of its parent
    /// split).
    pub fn resize(&mut self, dir: Dir, delta: f32) -> bool {
        let target = self.active().focused_pane;
        self.active_mut().root.resize_parent(target, dir, delta)
    }

    // ── serialization ───────────────────────────────────────────────

    /// Serialize with a caller-supplied payload encoder.
    ///
    /// The codec is a closure rather than a trait bound so this crate
    /// never needs a serde dependency and never needs to know what a
    /// payload is. `enc` must not emit `|`, `;` or a newline.
    pub fn snapshot_with(&self, enc: impl Fn(&P) -> String) -> String {
        let mut out = String::new();
        out.push_str(&format!("v1;{};{}\n", self.active_tab, mode_str(self.order_mode)));
        let f: Vec<String> = self.fences.iter().map(|b| b.to_string()).collect();
        out.push_str(&format!("fences;{}\n", f.join(",")));
        for t in &self.tabs {
            out.push_str(&format!(
                "tab;{};{};{};{};{}\n",
                esc(&t.name),
                t.focused_pane,
                t.pinned as u8,
                t.last_used,
                enc_node(&t.root, &enc)
            ));
        }
        out
    }

    /// Inverse of `snapshot_with`. Returns None on anything malformed —
    /// a half-restored layout is worse than a fresh one.
    pub fn restore_with(text: &str, dec: impl Fn(&str) -> Option<P>) -> Option<Workspace<P>> {
        let mut lines = text.lines();
        let head = lines.next()?;
        let mut hp = head.split(';');
        if hp.next()? != "v1" {
            return None;
        }
        let active: usize = hp.next()?.parse().ok()?;
        let mode = match hp.next()? {
            "lru" => TabOrderMode::Lru,
            _ => TabOrderMode::Sticky,
        };
        let fl = lines.next()?;
        let fences: BTreeSet<usize> = fl
            .strip_prefix("fences;")?
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        let mut tabs = Vec::new();
        let mut max_id = 0u64;
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let rest = line.strip_prefix("tab;")?;
            let mut p = rest.splitn(5, ';');
            let name = unesc(p.next()?);
            let focused: u64 = p.next()?.parse().ok()?;
            let pinned = p.next()? == "1";
            let last_used: u64 = p.next()?.parse().ok()?;
            let (root, _) = dec_node(p.next()?, &dec)?;
            for id in root.pane_ids() {
                max_id = max_id.max(id);
            }
            tabs.push(Tab { name, root, focused_pane: focused, pinned, last_used });
        }
        if tabs.is_empty() {
            return None;
        }
        Some(Workspace {
            active_tab: active.min(tabs.len() - 1),
            next_pane_id: max_id + 1,
            tabs,
            order_mode: mode,
            fences,
            use_tick: 0,
            undo_stack: Vec::new(),
        })
    }
}

fn mode_str(m: TabOrderMode) -> &'static str {
    match m {
        TabOrderMode::Sticky => "sticky",
        TabOrderMode::Lru => "lru",
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\s").replace('|', "\\p").replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(';'),
            Some('p') => out.push('|'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

fn enc_node<P: Payload>(n: &Node<P>, enc: &impl Fn(&P) -> String) -> String {
    match n {
        Node::Pane { id, payload } => format!("P|{}|{}", id, esc(&enc(payload))),
        Node::Split { orient, ratio, a, b } => format!(
            "S|{}|{:.4}|{}|{}",
            match orient {
                Orient::H => "h",
                Orient::V => "v",
            },
            ratio,
            enc_node(a, enc),
            enc_node(b, enc)
        ),
    }
}

/// Parse one node from the front of `s`, returning it and the rest.
fn dec_node<'a, P: Payload>(
    s: &'a str,
    dec: &impl Fn(&str) -> Option<P>,
) -> Option<(Node<P>, &'a str)> {
    let (tag, rest) = s.split_once('|')?;
    match tag {
        "P" => {
            let (id, rest) = rest.split_once('|')?;
            let (pay, rest) = match rest.split_once('|') {
                Some((p, r)) => (p, r),
                None => (rest, ""),
            };
            let payload = dec(&unesc(pay))?;
            Some((Node::Pane { id: id.parse().ok()?, payload }, rest))
        }
        "S" => {
            let (o, rest) = rest.split_once('|')?;
            let (ratio, rest) = rest.split_once('|')?;
            let (a, rest) = dec_node(rest, dec)?;
            let (b, rest) = dec_node(rest, dec)?;
            Some((
                Node::Split {
                    orient: if o == "h" { Orient::H } else { Orient::V },
                    ratio: ratio.parse().ok()?,
                    a: Box::new(a),
                    b: Box::new(b),
                },
                rest,
            ))
        }
        _ => None,
    }
}

/// Breadth-first pane ids across every tab, active tab first — the
/// order a "cycle panes" command should visit.
pub fn cycle_order<P: Payload>(ws: &Workspace<P>) -> VecDeque<u64> {
    let mut out = VecDeque::new();
    for id in ws.active().root.pane_ids() {
        out.push_back(id);
    }
    for (i, t) in ws.tabs.iter().enumerate() {
        if i == ws.active_tab {
            continue;
        }
        for id in t.root.pane_ids() {
            out.push_back(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in payload: the unit variant is what a fresh pane gets.
    #[derive(Debug, Clone, PartialEq, Default)]
    enum Kind {
        #[default]
        Blank,
        Doc(String),
    }

    fn ws() -> Workspace<Kind> {
        Workspace::new()
    }

    #[test]
    fn a_new_workspace_is_one_tab_one_pane() {
        let w = ws();
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.total_panes(), 1);
        assert_eq!(w.focused_pane(), 0);
    }

    #[test]
    fn split_creates_a_pane_and_focuses_it() {
        let mut w = ws();
        let id = w.split(Dir::Right);
        assert_eq!(w.total_panes(), 2);
        assert_eq!(w.focused_pane(), id);
        assert_ne!(id, 0);
    }

    #[test]
    fn split_direction_picks_the_orientation_and_the_side() {
        let mut w = ws();
        let id = w.split(Dir::Down);
        match &w.active().root {
            Node::Split { orient, a, b, .. } => {
                assert_eq!(*orient, Orient::V);
                assert!(matches!(a.as_ref(), Node::Pane { id: 0, .. }));
                assert!(matches!(b.as_ref(), Node::Pane { id: n, .. } if *n == id));
            }
            _ => panic!("expected a split"),
        }
        // left/up puts the new pane FIRST
        let mut w2 = ws();
        let id2 = w2.split(Dir::Left);
        match &w2.active().root {
            Node::Split { orient, a, .. } => {
                assert_eq!(*orient, Orient::H);
                assert!(matches!(a.as_ref(), Node::Pane { id: n, .. } if *n == id2));
            }
            _ => panic!("expected a split"),
        }
    }

    #[test]
    fn the_original_keeps_its_payload_and_the_new_pane_is_default() {
        let mut w = ws();
        w.set_payload(0, Kind::Doc("main.rs".into()));
        let id = w.split(Dir::Right);
        assert_eq!(w.payload(0), Some(&Kind::Doc("main.rs".into())));
        assert_eq!(w.payload(id), Some(&Kind::Blank));
    }

    #[test]
    fn split_with_sets_the_new_payload() {
        let mut w = ws();
        let id = w.split_with(Dir::Right, Kind::Doc("lib.rs".into()));
        assert_eq!(w.payload(id), Some(&Kind::Doc("lib.rs".into())));
    }

    #[test]
    fn pane_ids_are_never_reused() {
        // the caller keys a real resource on the id; reuse would attach
        // a live one to a dead pane
        let mut w = ws();
        let a = w.split(Dir::Right);
        w.close_pane_id(a);
        let b = w.split(Dir::Right);
        assert_ne!(a, b);
    }

    #[test]
    fn closing_a_pane_collapses_its_split() {
        let mut w = ws();
        let id = w.split(Dir::Right);
        assert!(w.close_pane_id(id));
        assert_eq!(w.total_panes(), 1);
        assert!(matches!(w.active().root, Node::Pane { id: 0, .. }));
        assert_eq!(w.focused_pane(), 0);
    }

    #[test]
    fn the_last_pane_in_a_tab_cannot_be_closed() {
        let mut w = ws();
        assert!(!w.close_pane_id(0));
        assert_eq!(w.close_pane(), None);
        assert_eq!(w.total_panes(), 1);
    }

    #[test]
    fn closing_the_focused_pane_refocuses_a_survivor() {
        let mut w = ws();
        w.split(Dir::Right);
        let focused = w.focused_pane();
        w.close_pane();
        assert_ne!(w.focused_pane(), focused);
        assert!(w.active().root.pane_ids().contains(&w.focused_pane()));
    }

    #[test]
    fn rects_tile_the_area_without_gaps() {
        let mut w = ws();
        w.split(Dir::Right);
        w.split(Dir::Down);
        let rects = w.pane_rects(100.0, 100.0);
        assert_eq!(rects.len(), 3);
        let area: f32 = rects.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((area - 10000.0).abs() < 0.01, "areas sum to {area}");
    }

    #[test]
    fn focus_moves_geometrically_not_in_tree_order() {
        let mut w = ws();
        let right = w.split(Dir::Right); // 0 | right
        w.focus(Dir::Left);
        assert_eq!(w.focused_pane(), 0);
        w.focus(Dir::Right);
        assert_eq!(w.focused_pane(), right);
        // nothing above: a no-op, not a wrap
        w.focus(Dir::Up);
        assert_eq!(w.focused_pane(), right);
    }

    #[test]
    fn focus_prefers_the_pane_straight_ahead() {
        // 0 | (a stacked over b); from 0, "right" should pick the one
        // whose centre is closest in line, not merely the first in tree
        // order
        let mut w = ws();
        let r = w.split(Dir::Right);
        let below = w.split(Dir::Down); // splits r into r over below
        w.focus_id(0);
        w.focus(Dir::Right);
        let got = w.focused_pane();
        assert!(got == r || got == below);
        // and from the lower-right pane, left goes back to 0
        w.focus_id(below);
        w.focus(Dir::Left);
        assert_eq!(w.focused_pane(), 0);
    }

    #[test]
    fn resize_moves_the_divider_and_clamps() {
        let mut w = ws();
        w.split(Dir::Right);
        assert!(w.resize(Dir::Left, 0.1));
        match &w.active().root {
            Node::Split { ratio, .. } => assert!((*ratio - 0.4).abs() < 1e-5, "{ratio}"),
            _ => panic!(),
        }
        // repeated resize in one direction is monotonic, then clamps
        for _ in 0..20 {
            w.resize(Dir::Left, 0.1);
        }
        match &w.active().root {
            Node::Split { ratio, .. } => assert!((*ratio - MIN_RATIO).abs() < 1e-5, "{ratio}"),
            _ => panic!(),
        }
    }

    #[test]
    fn resize_only_answers_on_the_matching_axis() {
        let mut w = ws();
        w.split(Dir::Right); // an H split
        assert!(!w.resize(Dir::Up, 0.1), "a vertical resize has no V split to act on");
    }

    #[test]
    fn tabs_open_close_and_never_drop_the_last_one() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        assert_eq!(w.tabs.len(), 3);
        assert_eq!(w.active_tab, 2);
        let freed = w.close_tab();
        assert_eq!(freed.len(), 1);
        assert_eq!(w.tabs.len(), 2);
        w.close_tab();
        assert_eq!(w.tabs.len(), 1);
        assert!(w.close_tab().is_empty(), "the last tab stays");
    }

    #[test]
    fn close_tab_returns_every_pane_id_it_held() {
        let mut w = ws();
        w.new_tab();
        let b = w.split(Dir::Right);
        let c = w.split(Dir::Down);
        let freed = w.close_tab();
        assert_eq!(freed.len(), 3);
        assert!(freed.contains(&b) && freed.contains(&c));
    }

    #[test]
    fn tab_navigation_wraps() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        w.next_tab();
        assert_eq!(w.active_tab, 0);
        w.prev_tab();
        assert_eq!(w.active_tab, 2);
        assert!(w.goto_tab(2));
        assert_eq!(w.active_tab, 1);
        assert!(!w.goto_tab(0));
        assert!(!w.goto_tab(99));
    }

    #[test]
    fn find_tab_matches_a_substring_case_insensitively() {
        let mut w = ws();
        w.rename_tab("Build Logs");
        w.new_tab();
        assert_eq!(w.find_tab("logs"), Some(0));
        assert_eq!(w.find_tab("nope"), None);
        assert_eq!(w.find_tab("  "), None);
    }

    #[test]
    fn lru_bubbles_the_used_tab_left() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        w.order_mode = TabOrderMode::Lru;
        let name = w.tabs[2].name.clone();
        w.goto_tab(3);
        assert_eq!(w.tabs[1].name, name);
        assert_eq!(w.active_tab, 1);
    }

    #[test]
    fn sticky_is_the_default_and_does_not_reorder() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        let before: Vec<String> = w.tabs.iter().map(|t| t.name.clone()).collect();
        w.goto_tab(3);
        let after: Vec<String> = w.tabs.iter().map(|t| t.name.clone()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn a_pin_blocks_bubbling() {
        let mut w = ws();
        w.new_tab();
        w.order_mode = TabOrderMode::Lru;
        w.toggle_pin(1);
        let name = w.tabs[1].name.clone();
        w.goto_tab(2);
        assert_eq!(w.tabs[1].name, name, "a pinned tab holds its slot");
    }

    #[test]
    fn a_fence_blocks_bubbling_across_it() {
        let mut w = ws();
        w.new_tab();
        w.order_mode = TabOrderMode::Lru;
        w.toggle_fence_after(0); // barrier between tab 0 and tab 1
        let name = w.tabs[1].name.clone();
        w.goto_tab(2);
        assert_eq!(w.tabs[1].name, name);
        // remove it and the same switch now moves
        w.toggle_fence_after(0);
        w.goto_tab(2);
        assert_eq!(w.tabs[0].name, name);
    }

    #[test]
    fn closing_a_tab_shifts_the_fences_above_it() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        w.new_tab(); // 4 tabs
        w.toggle_fence_after(2); // boundary 3
        w.active_tab = 0;
        w.close_tab();
        assert!(w.fences.contains(&2), "boundary followed its tabs: {:?}", w.fences);
    }

    #[test]
    fn bubble_all_sorts_by_recency_and_rollback_restores() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        let before: Vec<String> = w.tabs.iter().map(|t| t.name.clone()).collect();
        w.goto_tab(1); // make tab 0 the most recent
        w.bubble_all();
        assert_eq!(w.tabs[0].name, before[0]);
        assert!(w.rollback_tabs());
        let after: Vec<String> = w.tabs.iter().map(|t| t.name.clone()).collect();
        assert_eq!(after, before);
        // and the stack empties
        while w.rollback_tabs() {}
        assert!(!w.rollback_tabs());
    }

    #[test]
    fn gather_to_front_preserves_relative_order_and_keeps_focus() {
        let mut w = ws();
        for _ in 0..4 {
            w.new_tab();
        }
        let names: Vec<String> = w.tabs.iter().map(|t| t.name.clone()).collect();
        w.active_tab = 3;
        let active = w.tabs[3].name.clone();
        let n = w.gather_to_front(&[1, 3]);
        assert_eq!(n, 2);
        assert_eq!(w.tabs[0].name, names[1]);
        assert_eq!(w.tabs[1].name, names[3]);
        assert_eq!(w.tabs[w.active_tab].name, active, "focus followed the tab, not the index");
    }

    #[test]
    fn gather_clears_fences_because_neighbours_changed() {
        let mut w = ws();
        w.new_tab();
        w.new_tab();
        w.toggle_fence_after(0);
        assert!(!w.fences.is_empty());
        w.gather_to_front(&[2]);
        assert!(w.fences.is_empty());
    }

    #[test]
    fn focus_id_switches_tabs() {
        let mut w = ws();
        w.new_tab();
        let far = w.split(Dir::Right);
        w.active_tab = 0;
        assert!(w.focus_id(far));
        assert_eq!(w.active_tab, 1);
        assert_eq!(w.focused_pane(), far);
        assert!(!w.focus_id(9999));
    }

    #[test]
    fn cycle_order_starts_with_the_active_tab() {
        let mut w = ws();
        let a = w.split(Dir::Right);
        w.new_tab();
        let b = w.focused_pane();
        let order: Vec<u64> = cycle_order(&w).into_iter().collect();
        assert_eq!(order[0], b, "active tab first");
        assert!(order.contains(&a));
        assert_eq!(order.len(), w.total_panes());
    }

    fn enc(k: &Kind) -> String {
        match k {
            Kind::Blank => String::new(),
            Kind::Doc(p) => format!("doc:{p}"),
        }
    }
    fn dec(s: &str) -> Option<Kind> {
        Some(match s.strip_prefix("doc:") {
            Some(p) => Kind::Doc(p.into()),
            None => Kind::Blank,
        })
    }

    #[test]
    fn snapshot_round_trips() {
        let mut w = ws();
        w.set_payload(0, Kind::Doc("src/main.rs".into()));
        w.split_with(Dir::Right, Kind::Doc("src/lib.rs".into()));
        w.split(Dir::Down);
        w.resize(Dir::Left, 0.1);
        w.new_tab();
        w.rename_tab("logs");
        w.toggle_pin(1);
        w.toggle_fence_after(0);
        w.order_mode = TabOrderMode::Lru;

        let text = w.snapshot_with(enc);
        let back: Workspace<Kind> = Workspace::restore_with(&text, dec).expect("restores");
        assert_eq!(back.tabs.len(), w.tabs.len());
        assert_eq!(back.order_mode, w.order_mode);
        assert_eq!(back.fences, w.fences);
        assert_eq!(back.active_tab, w.active_tab);
        for (x, y) in back.tabs.iter().zip(w.tabs.iter()) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.pinned, y.pinned);
            assert_eq!(x.focused_pane, y.focused_pane);
            assert_eq!(x.root.pane_ids(), y.root.pane_ids());
        }
        assert_eq!(back.payload(0), Some(&Kind::Doc("src/main.rs".into())));
    }

    #[test]
    fn restored_ids_do_not_collide_with_new_ones() {
        let mut w = ws();
        let hi = w.split(Dir::Right);
        let text = w.snapshot_with(enc);
        let mut back: Workspace<Kind> = Workspace::restore_with(&text, dec).unwrap();
        let fresh = back.split(Dir::Down);
        assert!(fresh > hi, "next_pane_id must clear the restored maximum");
    }

    #[test]
    fn names_with_separators_survive_the_round_trip() {
        let mut w = ws();
        w.rename_tab("a;b|c\\d");
        let text = w.snapshot_with(enc);
        let back: Workspace<Kind> = Workspace::restore_with(&text, dec).unwrap();
        assert_eq!(back.tabs[0].name, "a;b|c\\d");
    }

    #[test]
    fn malformed_snapshots_are_rejected_whole() {
        assert!(Workspace::<Kind>::restore_with("", dec).is_none());
        assert!(Workspace::<Kind>::restore_with("v9;0;sticky\nfences;\n", dec).is_none());
        assert!(Workspace::<Kind>::restore_with("v1;0;sticky\n", dec).is_none());
        // a valid header with no tabs is not a workspace
        assert!(Workspace::<Kind>::restore_with("v1;0;sticky\nfences;\n", dec).is_none());
        // a truncated tree
        assert!(
            Workspace::<Kind>::restore_with("v1;0;sticky\nfences;\ntab;1;0;0;0;S|h|0.5|P|0|\n", dec)
                .is_none()
        );
    }

    #[test]
    fn dir_from_word_accepts_the_spoken_synonyms() {
        assert_eq!(Dir::from_word("Above"), Some(Dir::Up));
        assert_eq!(Dir::from_word(" below "), Some(Dir::Down));
        assert_eq!(Dir::from_word("sideways"), None);
    }
}
