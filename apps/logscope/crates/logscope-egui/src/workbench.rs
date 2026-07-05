//! workbench — the non-chat tabs as fast, gesture-navigated "workbenches", the snapclone/voiceterm
//! half of logscope. Only **Chat** keeps the mind-palace (3D space) because it's a planning /
//! LLM-assisted workflow where a spatial memory helps; every other tab is a pre-patterned action
//! on a file type, so it wants speed, near-invisible chrome, and rich touch navigation between
//! *related* functions instead of a space behind it.
//!
//! This is the pure model the render code drives: which workbench a tab belongs to, swipe order
//! between siblings (snapclone pages), and the touchkit-gesture → navigation map (voiceterm quick
//! access). No egui, so it builds/tests headless.

use touchkit::Gesture;

/// A logscope tab (mirrors `app::Tab`; kept here so the model stays pure + testable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbTab {
    Sources,
    Peek,
    Tokens,
    Blocks,
    Rotate,
    Cargo,
    Code,
    Modify,
    Bloc,
    Chat,
}

/// The group a tab lives in. `Palace` is Chat alone (the only tab with a mind-palace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workbench {
    /// Log ingest + inspection (pre-patterned viewers).
    Logs,
    /// Code generation / editing.
    Code,
    /// Build + block tooling.
    Build,
    /// The Chat mind-palace — spatial, standalone.
    Palace,
}

impl Workbench {
    pub fn label(self) -> &'static str {
        match self {
            Workbench::Logs => "Logs",
            Workbench::Code => "Code",
            Workbench::Build => "Build",
            Workbench::Palace => "Chat",
        }
    }
}

/// Which workbench a tab belongs to.
pub fn workbench_of(tab: WbTab) -> Workbench {
    match tab {
        WbTab::Sources | WbTab::Peek | WbTab::Tokens | WbTab::Blocks | WbTab::Rotate => Workbench::Logs,
        WbTab::Code | WbTab::Modify => Workbench::Code,
        WbTab::Cargo | WbTab::Bloc => Workbench::Build,
        WbTab::Chat => Workbench::Palace,
    }
}

/// Ordered members of a workbench — the swipe order between related functions.
pub fn members(wb: Workbench) -> &'static [WbTab] {
    match wb {
        Workbench::Logs => &[WbTab::Sources, WbTab::Peek, WbTab::Tokens, WbTab::Blocks, WbTab::Rotate],
        Workbench::Code => &[WbTab::Code, WbTab::Modify],
        Workbench::Build => &[WbTab::Cargo, WbTab::Bloc],
        Workbench::Palace => &[WbTab::Chat],
    }
}

/// Only Chat uses the mind-palace (3D scene + pearl). Everything else is flat/fast — the render
/// code checks this to skip the scene layer entirely on the other tabs.
pub fn uses_mind_palace(tab: WbTab) -> bool {
    matches!(tab, WbTab::Chat)
}

/// Direction of a lateral swipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDir {
    /// Swipe left → advance to the next related function.
    Next,
    /// Swipe right → previous.
    Prev,
}

/// The sibling tab reached by swiping within the same workbench (snapclone swipe-pages). Returns
/// `None` at the ends — the Palace has no siblings, so a swipe there does nothing (the scene owns
/// horizontal drags for orbiting).
pub fn swipe(cur: WbTab, dir: SwipeDir) -> Option<WbTab> {
    let wb = workbench_of(cur);
    if wb == Workbench::Palace {
        return None;
    }
    let list = members(wb);
    let i = list.iter().position(|t| *t == cur)?;
    let j = match dir {
        SwipeDir::Next => i + 1,
        SwipeDir::Prev => i.checked_sub(1)?,
    };
    list.get(j).copied()
}

/// Map a touchkit gesture to a navigation target within the current workbench (voiceterm-style
/// quick access). Horizontal flicks page between related functions; other gestures are left to the
/// tab's own content (pinch to zoom a log, etc.). `None` on the Chat tab, whose scene consumes
/// drags.
pub fn gesture_nav(cur: WbTab, g: &Gesture) -> Option<WbTab> {
    if uses_mind_palace(cur) {
        return None;
    }
    match g {
        Gesture::SwipeLeft { .. } => swipe(cur, SwipeDir::Next),
        Gesture::SwipeRight { .. } => swipe(cur, SwipeDir::Prev),
        _ => None,
    }
}

/// Placeholder for the (future) workbench branching feature — a snapshot/branch of a tab's state,
/// like Chat's conversation branching but for a log view or a code buffer. Wired to a toggle in
/// the UI but intentionally inert for now, so the affordance exists while the mechanism is
/// designed.
#[derive(Debug, Clone, Copy, Default)]
pub struct BranchToggle {
    pub enabled: bool,
}

impl BranchToggle {
    /// Flip the toggle; returns the new state. No behavioural effect yet (unwired on purpose).
    pub fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchkit::Vec2;

    #[test]
    fn only_chat_is_the_palace() {
        assert!(uses_mind_palace(WbTab::Chat));
        for t in [WbTab::Sources, WbTab::Peek, WbTab::Code, WbTab::Cargo, WbTab::Bloc] {
            assert!(!uses_mind_palace(t), "{t:?} is flat/fast, no scene");
        }
        assert_eq!(workbench_of(WbTab::Chat), Workbench::Palace);
    }

    #[test]
    fn swipe_pages_within_a_workbench() {
        // Logs: Sources -> Peek -> Tokens -> Blocks -> Rotate
        assert_eq!(swipe(WbTab::Sources, SwipeDir::Next), Some(WbTab::Peek));
        assert_eq!(swipe(WbTab::Peek, SwipeDir::Next), Some(WbTab::Tokens));
        assert_eq!(swipe(WbTab::Peek, SwipeDir::Prev), Some(WbTab::Sources));
        // ends return None (no wrap, so you can't swipe out of a workbench by accident)
        assert_eq!(swipe(WbTab::Sources, SwipeDir::Prev), None);
        assert_eq!(swipe(WbTab::Rotate, SwipeDir::Next), None);
        // Code workbench is its own little pair
        assert_eq!(swipe(WbTab::Code, SwipeDir::Next), Some(WbTab::Modify));
    }

    #[test]
    fn palace_swipes_do_nothing() {
        assert_eq!(swipe(WbTab::Chat, SwipeDir::Next), None, "scene owns drags, not paging");
    }

    #[test]
    fn gestures_page_related_functions_but_not_in_palace() {
        let left = Gesture::SwipeLeft { vel: 900.0 };
        let right = Gesture::SwipeRight { vel: 900.0 };
        assert_eq!(gesture_nav(WbTab::Sources, &left), Some(WbTab::Peek));
        assert_eq!(gesture_nav(WbTab::Peek, &right), Some(WbTab::Sources));
        // a pinch is not navigation (the tab uses it to zoom its content)
        assert_eq!(gesture_nav(WbTab::Peek, &Gesture::Pinch { rel: 1.2, scale: 1.2, center: Vec2::new(0.0, 0.0) }), None);
        // Chat ignores paging gestures
        assert_eq!(gesture_nav(WbTab::Chat, &left), None);
    }

    #[test]
    fn branch_toggle_flips_but_is_inert() {
        let mut b = BranchToggle::default();
        assert!(!b.enabled);
        assert!(b.toggle());
        assert!(!b.toggle());
    }
}
