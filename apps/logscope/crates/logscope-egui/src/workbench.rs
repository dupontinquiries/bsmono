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

/// The group a tab lives in — logscope is really **three tools**. Two are *surgical* (fast,
/// quick-access, no spatial layer); one is the *palace* (spatial thinking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workbench {
    /// Surgical tool #1: log rotation + quick system diagnosis. Meant to get you *started*, not to
    /// hand you a full solution.
    Diagnose,
    /// The Chat mind-palace — large development, fan-out exploration, research. The only tool with
    /// a physical space, because it's where you *think*.
    Palace,
    /// Surgical tool #2: the LLM + touch "super-keyboard" — functional block analysis, like coding
    /// in Scratch but for big-boy programs (AST blocs, block actions, cargo diagnostics). Still
    /// unfinished, but surgical in spirit, so it keeps the quick-access UI, not the palace.
    Forge,
}

impl Workbench {
    pub fn label(self) -> &'static str {
        match self {
            Workbench::Diagnose => "Diagnose",
            Workbench::Palace => "Chat",
            Workbench::Forge => "Forge",
        }
    }
    /// Surgical tools get the fast, gesture-navigated UI; the palace does not.
    pub fn is_surgical(self) -> bool {
        !matches!(self, Workbench::Palace)
    }
}

/// Which workbench a tab belongs to. (Tokens/Blocks have no dedicated module yet — placed by
/// affinity: Tokens with Diagnose, Blocks with Forge; confirm when they're built out.)
pub fn workbench_of(tab: WbTab) -> Workbench {
    match tab {
        WbTab::Sources | WbTab::Peek | WbTab::Rotate | WbTab::Tokens => Workbench::Diagnose,
        WbTab::Chat => Workbench::Palace,
        WbTab::Code | WbTab::Bloc | WbTab::Modify | WbTab::Cargo | WbTab::Blocks => Workbench::Forge,
    }
}

/// Ordered members of a workbench — the swipe order between related functions.
pub fn members(wb: Workbench) -> &'static [WbTab] {
    match wb {
        // load → inspect → rotate → token stats
        Workbench::Diagnose => &[WbTab::Sources, WbTab::Peek, WbTab::Rotate, WbTab::Tokens],
        Workbench::Palace => &[WbTab::Chat],
        // block view → bloc regions → transform → build check → block palette
        Workbench::Forge => &[WbTab::Code, WbTab::Bloc, WbTab::Modify, WbTab::Cargo, WbTab::Blocks],
    }
}

/// True for the two surgical tools (Diagnose, Forge) — the ones that get fast gesture navigation
/// and no 3D scene. False only for the Chat palace.
pub fn is_surgical(tab: WbTab) -> bool {
    workbench_of(tab).is_surgical()
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
    fn three_tools_two_surgical_one_palace() {
        // Chat is the only palace; the two surgical tools get the fast UI
        assert!(uses_mind_palace(WbTab::Chat));
        assert_eq!(workbench_of(WbTab::Chat), Workbench::Palace);
        assert!(!is_surgical(WbTab::Chat));
        // Diagnose (log tool) + Forge (block super-keyboard) are surgical
        assert_eq!(workbench_of(WbTab::Rotate), Workbench::Diagnose);
        assert_eq!(workbench_of(WbTab::Bloc), Workbench::Forge);
        for t in [WbTab::Sources, WbTab::Peek, WbTab::Rotate, WbTab::Code, WbTab::Bloc, WbTab::Cargo] {
            assert!(is_surgical(t) && !uses_mind_palace(t), "{t:?} is surgical/flat, no scene");
        }
    }

    #[test]
    fn swipe_pages_within_a_workbench() {
        // Diagnose: Sources -> Peek -> Rotate -> Tokens
        assert_eq!(swipe(WbTab::Sources, SwipeDir::Next), Some(WbTab::Peek));
        assert_eq!(swipe(WbTab::Peek, SwipeDir::Next), Some(WbTab::Rotate));
        assert_eq!(swipe(WbTab::Peek, SwipeDir::Prev), Some(WbTab::Sources));
        // ends return None (no wrap → can't swipe out of a tool by accident)
        assert_eq!(swipe(WbTab::Sources, SwipeDir::Prev), None);
        assert_eq!(swipe(WbTab::Tokens, SwipeDir::Next), None);
        // Forge: Code -> Bloc -> Modify -> Cargo -> Blocks
        assert_eq!(swipe(WbTab::Code, SwipeDir::Next), Some(WbTab::Bloc));
        assert_eq!(swipe(WbTab::Cargo, SwipeDir::Next), Some(WbTab::Blocks));
        assert_eq!(swipe(WbTab::Code, SwipeDir::Prev), None);
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
