//! `sim::menu_nav` — focus/cursor navigation for in-game menus.
//!
//! A controller-friendly navigator over a tree of menu items. Tracks a stack
//! of `(items, focus_index)` frames; exposes move, descend, back, and
//! activate. No rendering — pure state transitions.
//!
//! Pairs with `sim::options_model` which holds the actual settings values;
//! this module drives the cursor over the menu tree.

// ── Types ─────────────────────────────────────────────────────────────────────

/// What kind of item this is.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// A leaf action — activating fires `NavEvent::Activated(id)`.
    Action,
    /// A submenu — activating descends into the child list.
    Submenu(Vec<MenuItem>),
    /// A back sentinel — activating fires `NavEvent::Backed` (same as `back()`).
    Back,
}

/// One item in a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// Stable identifier (unique within a menu level, not globally required).
    pub id: u16,
    /// Whether this item can receive focus.
    pub enabled: bool,
    pub kind: ItemKind,
}

impl MenuItem {
    pub fn action(id: u16, enabled: bool) -> MenuItem {
        MenuItem { id, enabled, kind: ItemKind::Action }
    }
    pub fn submenu(id: u16, enabled: bool, children: Vec<MenuItem>) -> MenuItem {
        MenuItem { id, enabled, kind: ItemKind::Submenu(children) }
    }
    pub fn back(id: u16) -> MenuItem {
        MenuItem { id, enabled: true, kind: ItemKind::Back }
    }
}

/// Direction of focus movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDir {
    Up,
    Down,
}

/// Result of a navigation or activation operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavEvent {
    /// Focus moved to a different item.
    Navigated,
    /// Descended into a submenu.
    Descended,
    /// Backed out of a submenu (or Back item activated).
    Backed,
    /// An Action item was activated.
    Activated(u16),
    /// No eligible item to move to (all disabled, or empty menu).
    Blocked,
}

// ── Navigator ─────────────────────────────────────────────────────────────────

/// One level on the navigation stack.
#[derive(Clone, Debug)]
struct Frame {
    items: Vec<MenuItem>,
    /// Index of the currently focused item.
    focus: usize,
}

impl Frame {
    fn new(items: Vec<MenuItem>) -> Frame {
        let focus = first_enabled(&items).unwrap_or(0);
        Frame { items, focus }
    }
}

/// Return the index of the first enabled item, or `None`.
fn first_enabled(items: &[MenuItem]) -> Option<usize> {
    items.iter().position(|it| it.enabled)
}

/// Return `true` if any item is enabled.
fn any_enabled(items: &[MenuItem]) -> bool {
    items.iter().any(|it| it.enabled)
}

/// Controller-friendly menu navigator.
///
/// Internally maintains a stack of frames (each frame = one menu level +
/// current focus index). The root frame is always at the bottom.
#[derive(Clone, Debug)]
pub struct MenuNav {
    stack: Vec<Frame>,
}

impl MenuNav {
    /// Create a navigator rooted at `root`. Focus is set to the first enabled
    /// item. If `root` is empty or all disabled, the navigator is still created
    /// safely.
    pub fn new(root: Vec<MenuItem>) -> MenuNav {
        MenuNav { stack: vec![Frame::new(root)] }
    }

    /// Move focus up or down. Wraps around; skips disabled items.
    /// Returns `Blocked` if no item is selectable.
    pub fn move_focus(&mut self, dir: NavDir) -> NavEvent {
        let frame = match self.stack.last_mut() {
            Some(f) => f,
            None => return NavEvent::Blocked,
        };
        let n = frame.items.len();
        if n == 0 || !any_enabled(&frame.items) {
            return NavEvent::Blocked;
        }
        let step: isize = match dir { NavDir::Up => -1, NavDir::Down => 1 };
        let mut idx = frame.focus as isize;
        for _ in 0..n {
            idx = (idx + step).rem_euclid(n as isize);
            if frame.items[idx as usize].enabled {
                frame.focus = idx as usize;
                return NavEvent::Navigated;
            }
        }
        NavEvent::Blocked
    }

    /// Activate the currently focused item.
    ///
    /// - `Action` → `Activated(id)`
    /// - `Submenu(children)` → descend, focus first enabled child, `Descended`
    /// - `Back` → same as `back()`, returns `Backed`
    /// - Empty menu / no frame → `Blocked`
    pub fn activate(&mut self) -> NavEvent {
        let (id, kind) = {
            let frame = match self.stack.last() {
                Some(f) => f,
                None => return NavEvent::Blocked,
            };
            if frame.items.is_empty() {
                return NavEvent::Blocked;
            }
            let item = &frame.items[frame.focus];
            (item.id, item.kind.clone())
        };

        match kind {
            ItemKind::Action => NavEvent::Activated(id),
            ItemKind::Back => {
                self.back();
                NavEvent::Backed
            }
            ItemKind::Submenu(children) => {
                self.stack.push(Frame::new(children));
                NavEvent::Descended
            }
        }
    }

    /// Pop one level off the stack. Returns `true` on success, `false` at root.
    pub fn back(&mut self) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        self.stack.pop();
        true
    }

    /// The `id` of the currently focused item, or `None` if the menu is empty.
    pub fn focused_id(&self) -> Option<u16> {
        let frame = self.stack.last()?;
        frame.items.get(frame.focus).map(|it| it.id)
    }

    /// Current depth (0 = root).
    pub fn depth(&self) -> usize {
        self.stack.len().saturating_sub(1)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: u16) -> MenuItem { MenuItem::action(id, true) }
    fn disabled(id: u16) -> MenuItem { MenuItem::action(id, false) }
    fn back(id: u16) -> MenuItem { MenuItem::back(id) }
    fn sub(id: u16, children: Vec<MenuItem>) -> MenuItem {
        MenuItem::submenu(id, true, children)
    }

    // ── Construction ──

    #[test]
    fn new_focuses_first_enabled() {
        let items = vec![disabled(0), action(1), action(2)];
        let nav = MenuNav::new(items);
        assert_eq!(nav.focused_id(), Some(1));
    }

    #[test]
    fn new_empty_menu_safe() {
        let nav = MenuNav::new(vec![]);
        assert_eq!(nav.focused_id(), None);
        assert_eq!(nav.depth(), 0);
    }

    #[test]
    fn new_all_disabled_safe() {
        let nav = MenuNav::new(vec![disabled(0), disabled(1)]);
        // focus index defaults to 0; focused_id still returns Some(0)
        assert!(nav.focused_id().is_some());
    }

    // ── move_focus: wrapping ──

    #[test]
    fn move_down_wraps() {
        let items = vec![action(0), action(1), action(2)];
        let mut nav = MenuNav::new(items);
        nav.move_focus(NavDir::Down);
        nav.move_focus(NavDir::Down);
        nav.move_focus(NavDir::Down); // should wrap back to 0
        assert_eq!(nav.focused_id(), Some(0));
    }

    #[test]
    fn move_up_wraps() {
        let mut nav = MenuNav::new(vec![action(0), action(1), action(2)]);
        nav.move_focus(NavDir::Up); // from 0 → 2
        assert_eq!(nav.focused_id(), Some(2));
    }

    // ── move_focus: skips disabled ──

    #[test]
    fn move_skips_disabled() {
        let items = vec![action(0), disabled(1), action(2)];
        let mut nav = MenuNav::new(items);
        assert_eq!(nav.focused_id(), Some(0));
        let ev = nav.move_focus(NavDir::Down);
        assert_eq!(ev, NavEvent::Navigated);
        assert_eq!(nav.focused_id(), Some(2)); // skipped 1
    }

    #[test]
    fn move_all_disabled_returns_blocked() {
        let mut nav = MenuNav::new(vec![disabled(0), disabled(1)]);
        let ev = nav.move_focus(NavDir::Down);
        assert_eq!(ev, NavEvent::Blocked);
    }

    #[test]
    fn move_empty_menu_returns_blocked() {
        let mut nav = MenuNav::new(vec![]);
        assert_eq!(nav.move_focus(NavDir::Down), NavEvent::Blocked);
    }

    #[test]
    fn blocked_leaves_focus_unchanged() {
        let mut nav = MenuNav::new(vec![action(5), disabled(6)]);
        nav.move_focus(NavDir::Down); // wraps back to action(5)
        let id_before = nav.focused_id();
        // Move again with only one enabled item → navigated back to same
        // Actually: single enabled item wraps to itself → Navigated not Blocked
        // True Blocked case: all disabled
        let mut nav2 = MenuNav::new(vec![disabled(0), disabled(1)]);
        let id2 = nav2.focused_id();
        nav2.move_focus(NavDir::Up);
        assert_eq!(nav2.focused_id(), id2);
        let _ = id_before; // silence unused
    }

    // ── activate: Action ──

    #[test]
    fn activate_action_returns_activated() {
        let mut nav = MenuNav::new(vec![action(42)]);
        assert_eq!(nav.activate(), NavEvent::Activated(42));
        assert_eq!(nav.depth(), 0); // no change
    }

    // ── activate: Submenu ──

    #[test]
    fn activate_submenu_descends() {
        let children = vec![action(10), action(11)];
        let mut nav = MenuNav::new(vec![sub(1, children)]);
        let ev = nav.activate();
        assert_eq!(ev, NavEvent::Descended);
        assert_eq!(nav.depth(), 1);
        assert_eq!(nav.focused_id(), Some(10)); // first child
    }

    #[test]
    fn descend_focuses_first_enabled_child() {
        let children = vec![disabled(7), action(8)];
        let mut nav = MenuNav::new(vec![sub(1, children)]);
        nav.activate();
        assert_eq!(nav.focused_id(), Some(8));
    }

    // ── back restores prior focus ──

    #[test]
    fn back_restores_parent_focus() {
        let mut nav = MenuNav::new(vec![action(0), sub(1, vec![action(10)]), action(2)]);
        nav.move_focus(NavDir::Down); // focus sub(1)
        let parent_id = nav.focused_id();
        nav.activate(); // descend
        assert_eq!(nav.depth(), 1);
        nav.back();
        assert_eq!(nav.depth(), 0);
        assert_eq!(nav.focused_id(), parent_id); // restored
    }

    #[test]
    fn back_at_root_returns_false() {
        let mut nav = MenuNav::new(vec![action(0)]);
        assert!(!nav.back());
        assert_eq!(nav.depth(), 0);
    }

    // ── activate: Back item ──

    #[test]
    fn activate_back_item_returns_backed_and_pops() {
        let children = vec![action(10), back(99)];
        let mut nav = MenuNav::new(vec![sub(1, children)]);
        nav.activate(); // descend
        assert_eq!(nav.depth(), 1);
        nav.move_focus(NavDir::Down); // focus back item (id=99)
        assert_eq!(nav.focused_id(), Some(99));
        let ev = nav.activate();
        assert_eq!(ev, NavEvent::Backed);
        assert_eq!(nav.depth(), 0);
    }

    // ── depth ──

    #[test]
    fn depth_increases_with_descent() {
        let l2 = vec![action(20)];
        let l1 = vec![sub(10, l2)];
        let mut nav = MenuNav::new(vec![sub(1, l1)]);
        assert_eq!(nav.depth(), 0);
        nav.activate();
        assert_eq!(nav.depth(), 1);
        nav.activate();
        assert_eq!(nav.depth(), 2);
        nav.back();
        assert_eq!(nav.depth(), 1);
        nav.back();
        assert_eq!(nav.depth(), 0);
    }

    // ── focused_id ──

    #[test]
    fn focused_id_none_on_empty() {
        assert_eq!(MenuNav::new(vec![]).focused_id(), None);
    }

    // ── Complex navigation sequence ──

    #[test]
    fn full_navigation_sequence() {
        //  Root: [action(1), sub(2, [action(21), action(22), back(99)]), action(3)]
        let children = vec![action(21), action(22), back(99)];
        let mut nav = MenuNav::new(vec![action(1), sub(2, children), action(3)]);

        assert_eq!(nav.focused_id(), Some(1));
        nav.move_focus(NavDir::Down);
        assert_eq!(nav.focused_id(), Some(2));

        let ev = nav.activate(); // descend into sub
        assert_eq!(ev, NavEvent::Descended);
        assert_eq!(nav.depth(), 1);
        assert_eq!(nav.focused_id(), Some(21));

        nav.move_focus(NavDir::Down);
        assert_eq!(nav.focused_id(), Some(22));

        nav.move_focus(NavDir::Down); // focus back item
        assert_eq!(nav.focused_id(), Some(99));

        let ev = nav.activate(); // Back item
        assert_eq!(ev, NavEvent::Backed);
        assert_eq!(nav.depth(), 0);
        assert_eq!(nav.focused_id(), Some(2)); // restored

        nav.move_focus(NavDir::Down);
        assert_eq!(nav.focused_id(), Some(3));

        let ev = nav.activate();
        assert_eq!(ev, NavEvent::Activated(3));
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let make = || {
            let children = vec![action(10), disabled(11), action(12)];
            let mut nav = MenuNav::new(vec![action(1), sub(2, children), action(3)]);
            nav.move_focus(NavDir::Down);
            nav.activate();
            nav.move_focus(NavDir::Down);
            (nav.focused_id(), nav.depth())
        };
        assert_eq!(make(), make());
    }
}
