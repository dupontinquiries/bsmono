//! gesture_menus — the four SlidePopup menu models from `qml/main.qml`, ported with their
//! dispatch rules, on top of `uikit_egui::slide_menu`'s QML-parity widget.
//!
//! What lives here is the *data + decision logic* (pure, unit-tested): which cells each menu has,
//! at which grid positions, with which directional flags, and what a selection means — the exact
//! bytes for the PTY menus, typed workspace calls for the tab/pane menus. What does NOT live here
//! is any backend I/O: callers feed the returned bytes to the terminal / map the returned calls to
//! their backend, exactly as main.qml's `onSelected` handlers did.
//!
//! QML cross-references (so the two histories stay linked):
//! - `pipeMenuModel` — 3×2 shell operators; select → `pty_write(char)`.
//! - `braceMenuModel` — 1×6 pairs with the deadband: open → openChar, close → closeChar, both →
//!   openChar+closeChar+`ESC[D`, backing the cursor between them (v0.1.14x). Compact/comfort
//!   sizing per v0.1.155 (deadband 14/28 px, cell 80/120 px).
//! - `tabActionMenuModel` (v0.1.790–v0.1.813) — fence/pin/split(splitDir)/webpane/newtab/
//!   close(closeDir); split/webpane/close first activate the held tab ("go to tab N").
//! - `paneActionMenuModel` (v0.1.838/v0.1.899) — 3×3 with the mic at dead center so a plain
//!   release is push-to-talk; split/close inherit the directional gestures.

use uikit_egui::slide_menu::{SlideMenu, SlideOption, Variant};

// ── pipe menu ────────────────────────────────────────────────────────────────

/// `pipeMenuModel`: the 3×2 shell-operator grid.
pub fn pipe_menu() -> Vec<SlideOption> {
    vec![
        SlideOption::new("pipe", "|").at(0, 0),
        SlideOption::new("bg", "&").at(1, 0),
        SlideOption::new("semi", ";").at(2, 0),
        SlideOption::new("stdin", "<").at(0, 1),
        SlideOption::new("stdout", ">").at(1, 1),
        SlideOption::new("append", ">>").at(2, 1),
    ]
}

/// The PTY bytes for a pipe-menu selection (`onSelected` → `pty_write(char)`).
pub fn pipe_char(id: &str) -> Option<&'static str> {
    Some(match id {
        "pipe" => "|",
        "bg" => "&",
        "semi" => ";",
        "stdin" => "<",
        "stdout" => ">",
        "append" => ">>",
        _ => return None,
    })
}

/// A pipe menu configured like `_pipePopup` (no deadband, default sizing).
pub fn pipe_slide_menu(id_source: impl std::hash::Hash) -> SlideMenu {
    SlideMenu::new(id_source).columns(3)
}

// ── brace menu ───────────────────────────────────────────────────────────────

const BRACES: &[(&str, &str, &str, &str)] = &[
    // (id, label, openChar, closeChar) — braceMenuModel order, one per row
    ("square", "[ ]", "[", "]"),
    ("curly", "{ }", "{", "}"),
    ("paren", "( )", "(", ")"),
    ("angle", "< >", "<", ">"),
    ("quote", "\" \"", "\"", "\""),
    ("apos", "' '", "'", "'"),
];

/// `braceMenuModel`: the 1×6 pair column.
pub fn brace_menu() -> Vec<SlideOption> {
    BRACES
        .iter()
        .enumerate()
        .map(|(i, (id, label, _, _))| SlideOption::new(*id, *label).at(0, i as u8))
        .collect()
}

/// The PTY bytes for a brace selection+variant — `_bracePopup.onSelected`, verbatim:
/// open → openChar, close → closeChar, anything else ("both") → pair + `ESC[D` to back the cursor
/// between them.
pub fn brace_bytes(id: &str, variant: Variant) -> Option<String> {
    let (_, _, open, close) = BRACES.iter().find(|(bid, ..)| *bid == id)?;
    Some(match variant {
        Variant::Open => (*open).to_string(),
        Variant::Close => (*close).to_string(),
        _ => format!("{open}{close}\u{1b}[D"),
    })
}

/// A brace menu configured like `_bracePopup` under v0.1.155's two sizings:
/// compact = deadband 14 px / cell 80 px, comfort = deadband 28 px / cell 120 px.
pub fn brace_slide_menu(id_source: impl std::hash::Hash, compact: bool) -> SlideMenu {
    let (db, item) = if compact { (14.0, 80.0) } else { (28.0, 120.0) };
    SlideMenu::new(id_source).columns(1).deadband(db).item_size(item)
}

// ── link action menu ─────────────────────────────────────────────────────────

/// `linkMenuModel` (v0.1.538): the 2×2 intent grid a blue-link tap opens.
pub fn link_menu() -> Vec<SlideOption> {
    vec![
        SlideOption::new("open", "Open").at(0, 0),
        SlideOption::new("read", "Read").at(1, 0),
        SlideOption::new("copy", "Copy").at(0, 1),
        SlideOption::new("newtab", "New tab").at(1, 1),
    ]
}

/// What a link-menu pick means for a tapped `url` — main.qml's dispatch, typed: Open navigates
/// the web pane, Read hands the URL to the reader, Copy → clipboard, NewTab opens a new
/// workspace web tab (v0.1.880's pane-infra model, with the legacy background-tab as fallback).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    Open(String),
    Read(String),
    CopyUrl(String),
    NewTab(String),
}

/// `onSelected` for the link menu (always tap-select, always `Variant::Single` in QML).
pub fn link_dispatch(id: &str, url: &str) -> Option<LinkAction> {
    Some(match id {
        "open" => LinkAction::Open(url.to_string()),
        "read" => LinkAction::Read(url.to_string()),
        "copy" => LinkAction::CopyUrl(url.to_string()),
        "newtab" => LinkAction::NewTab(url.to_string()),
        _ => return None,
    })
}

// ── tab action menu ──────────────────────────────────────────────────────────

/// `tabActionMenuModel` (v0.1.813 icon set): 3×2 with directional split/close.
pub fn tab_menu() -> Vec<SlideOption> {
    vec![
        SlideOption::new("fence", "\u{2502}").at(0, 0),          // │ fence/divider
        SlideOption::new("pin", "\u{1F4CC}").at(1, 0),           // 📌 pin
        SlideOption::new("split", "\u{229E}").at(2, 0).split_dir(), // ⊞ up = vertical, right = horizontal
        SlideOption::new("webpane", "\u{1F310}").at(0, 1),       // 🌐 add web pane
        SlideOption::new("newtab", "\u{FF0B}").at(1, 1),         // ＋ new shell
        SlideOption::new("close", "\u{2715}").at(2, 1).close_dir(), // ✕ down = pane, right = tab
    ]
}

/// One backend call the tab menu's dispatch issues — the typed form of what main.qml's
/// `onSelected` invoked on `backend`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsCall {
    ToggleFence(usize),
    TogglePin(usize),
    Command(String),
    OpenWebPane,
}

/// `tabActionPopup.onSelected`, verbatim as a call sequence. `idx` is the held tab's index
/// (the QML `context`). Ordering matters: split/webpane/close activate the held tab FIRST
/// ("go to tab N", 1-based), exactly as the QML does.
pub fn tab_dispatch(id: &str, variant: Variant, idx: usize) -> Vec<WsCall> {
    match id {
        "fence" => vec![WsCall::ToggleFence(idx)],
        "pin" => vec![WsCall::TogglePin(idx)],
        "newtab" => vec![WsCall::Command("new tab".into())],
        "split" => vec![
            WsCall::Command(format!("go to tab {}", idx + 1)),
            WsCall::Command(if variant == Variant::Vertical { "split down" } else { "split right" }.into()),
        ],
        "webpane" => vec![WsCall::Command(format!("go to tab {}", idx + 1)), WsCall::OpenWebPane],
        "close" => vec![
            WsCall::Command(format!("go to tab {}", idx + 1)),
            WsCall::Command(if variant == Variant::Pane { "close pane" } else { "close tab" }.into()),
        ],
        _ => vec![],
    }
}

/// A tab menu configured like `tabActionPopup`, with the held tab's name as the title band
/// (QML v0.1.800 shows it there).
pub fn tab_slide_menu(id_source: impl std::hash::Hash, tab_name: &str) -> SlideMenu {
    SlideMenu::new(id_source).columns(3).title(tab_name)
}

// ── pane action menu ─────────────────────────────────────────────────────────

/// `paneActionMenuModel` (v0.1.838): 3×3, mic at dead center so a plain press-release is
/// push-to-talk; split/close carry the directional gestures.
pub fn pane_menu() -> Vec<SlideOption> {
    vec![
        SlideOption::new("split", "\u{229E}").at(0, 0).split_dir(),
        SlideOption::new("newtab", "\u{FF0B}").at(1, 0),
        SlideOption::new("close", "\u{2715}").at(2, 0).close_dir(),
        SlideOption::new("copy", "\u{29C9}").at(0, 1),
        SlideOption::new("mic", "\u{1F3A4}").at(1, 1),
        SlideOption::new("paste", "\u{2913}").at(2, 1),
        SlideOption::new("clear", "\u{2327}").at(0, 2),
        SlideOption::new("scroll", "\u{2630}").at(1, 2),
        SlideOption::new("cmds", "\u{2318}").at(2, 2),
    ]
}

/// What a pane-menu pick means, typed (v0.1.899 wiring). The app layer maps these onto its
/// backend: `Split`/`Close` carry the exact ws_command string; `Clear` is Ctrl+L to the PTY;
/// `Scroll` pages the scrollback up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneAction {
    Split(&'static str),
    NewTab,
    Close(&'static str),
    Mic,
    CopySelection,
    Paste,
    /// Send Ctrl+L (`\x0c`) to the PTY.
    Clear,
    PageUpScrollback,
    CommandPalette,
}

/// `paneActionPopup.onSelected`, typed. Directional rules identical to the tab menu's.
pub fn pane_dispatch(id: &str, variant: Variant) -> Option<PaneAction> {
    Some(match id {
        "split" => PaneAction::Split(if variant == Variant::Vertical { "split down" } else { "split right" }),
        "newtab" => PaneAction::NewTab,
        "close" => PaneAction::Close(if variant == Variant::Pane { "close pane" } else { "close tab" }),
        "mic" => PaneAction::Mic,
        "copy" => PaneAction::CopySelection,
        "paste" => PaneAction::Paste,
        "clear" => PaneAction::Clear,
        "scroll" => PaneAction::PageUpScrollback,
        "cmds" => PaneAction::CommandPalette,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uikit_egui::slide_menu::layout_dims;

    #[test]
    fn menus_keep_their_qml_grid_shapes() {
        assert_eq!(layout_dims(&pipe_menu(), 3), (3, 2));
        assert_eq!(layout_dims(&brace_menu(), 3), (1, 6)); // stays a column despite max_cols
        assert_eq!(layout_dims(&tab_menu(), 3), (3, 2));
        assert_eq!(layout_dims(&pane_menu(), 3), (3, 3));
        // the mic sits at dead center of the pane grid so a plain release is push-to-talk
        let mic = pane_menu().into_iter().find(|o| o.id == "mic").unwrap();
        assert_eq!(mic.at, Some((1, 1)));
        // directional flags are on the right cells
        assert!(tab_menu().iter().find(|o| o.id == "split").unwrap().split_dir);
        assert!(tab_menu().iter().find(|o| o.id == "close").unwrap().close_dir);
    }

    #[test]
    fn link_menu_is_the_2x2_intent_grid_and_dispatch_carries_the_url() {
        assert_eq!(layout_dims(&link_menu(), 3), (2, 2));
        assert_eq!(link_dispatch("copy", "https://a.b"), Some(LinkAction::CopyUrl("https://a.b".into())));
        assert_eq!(link_dispatch("newtab", "https://a.b"), Some(LinkAction::NewTab("https://a.b".into())));
        assert_eq!(link_dispatch("nope", "https://a.b"), None);
    }

    #[test]
    fn pipe_chars_match_the_qml_model() {
        for (id, ch) in [("pipe", "|"), ("bg", "&"), ("semi", ";"), ("stdin", "<"), ("stdout", ">"), ("append", ">>")] {
            assert_eq!(pipe_char(id), Some(ch));
        }
        assert_eq!(pipe_char("nope"), None);
    }

    #[test]
    fn brace_variants_emit_open_close_or_pair_with_cursor_between() {
        assert_eq!(brace_bytes("curly", Variant::Open).unwrap(), "{");
        assert_eq!(brace_bytes("curly", Variant::Close).unwrap(), "}");
        // "both" — pair + ESC[D backs the cursor between them (the QML pty_write sequence)
        assert_eq!(brace_bytes("curly", Variant::Both).unwrap(), "{}\u{1b}[D");
        // any non-deadband variant takes the "both" path, as in QML's else-branch
        assert_eq!(brace_bytes("square", Variant::Single).unwrap(), "[]\u{1b}[D");
        assert_eq!(brace_bytes("quote", Variant::Both).unwrap(), "\"\"\u{1b}[D");
        assert_eq!(brace_bytes("unknown", Variant::Both), None);
    }

    #[test]
    fn tab_dispatch_activates_the_held_tab_first_and_honors_direction() {
        assert_eq!(tab_dispatch("fence", Variant::Single, 2), vec![WsCall::ToggleFence(2)]);
        assert_eq!(tab_dispatch("newtab", Variant::Single, 0), vec![WsCall::Command("new tab".into())]);
        // split: goto (1-based) THEN the directional split — vertical (dragged up) = split down
        assert_eq!(
            tab_dispatch("split", Variant::Vertical, 1),
            vec![WsCall::Command("go to tab 2".into()), WsCall::Command("split down".into())]
        );
        assert_eq!(
            tab_dispatch("split", Variant::Horizontal, 0),
            vec![WsCall::Command("go to tab 1".into()), WsCall::Command("split right".into())]
        );
        // close: pane when dragged down, tab otherwise (incl. plain release)
        assert_eq!(
            tab_dispatch("close", Variant::Pane, 0),
            vec![WsCall::Command("go to tab 1".into()), WsCall::Command("close pane".into())]
        );
        assert_eq!(
            tab_dispatch("close", Variant::Single, 0),
            vec![WsCall::Command("go to tab 1".into()), WsCall::Command("close tab".into())]
        );
        assert_eq!(
            tab_dispatch("webpane", Variant::Single, 3),
            vec![WsCall::Command("go to tab 4".into()), WsCall::OpenWebPane]
        );
        assert!(tab_dispatch("bogus", Variant::Single, 0).is_empty());
    }

    #[test]
    fn pane_dispatch_types_every_qml_action() {
        assert_eq!(pane_dispatch("split", Variant::Vertical), Some(PaneAction::Split("split down")));
        assert_eq!(pane_dispatch("split", Variant::Horizontal), Some(PaneAction::Split("split right")));
        assert_eq!(pane_dispatch("close", Variant::Pane), Some(PaneAction::Close("close pane")));
        assert_eq!(pane_dispatch("close", Variant::Tab), Some(PaneAction::Close("close tab")));
        assert_eq!(pane_dispatch("mic", Variant::Single), Some(PaneAction::Mic));
        assert_eq!(pane_dispatch("clear", Variant::Single), Some(PaneAction::Clear));
        assert_eq!(pane_dispatch("cmds", Variant::Single), Some(PaneAction::CommandPalette));
        assert_eq!(pane_dispatch("bogus", Variant::Single), None);
    }
}
