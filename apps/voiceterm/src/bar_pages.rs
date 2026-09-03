//! bar_pages: which buttons the bottom bar shows, keyed on the voice
//! grammar that is currently live.
//!
//! ## Why not a ribbon
//!
//! A ribbon's real value is that the visible commands change with what
//! you are doing. Its cost is a permanent tab strip plus a two-row
//! command area — on a 380px phone that is most of the vertical budget,
//! to show maybe six buttons. The existing tap-hold-drag fan-out already
//! gets six options out of ONE button and costs nothing until touched.
//!
//! So: keep the mode-appropriate contents, drop the tab strip. The page
//! switches itself.
//!
//! ## Why grammar_label is the right key
//!
//! `grammar_context::grammar_label()` is composed from the same
//! detectors `voice_parse_intent` dispatches on. Keying the page off it
//! means the bar can never offer an action that the voice path would
//! route somewhere else — the two cannot drift, because there is one
//! source. Deriving the page from anything else (focused widget, last
//! tab, a manual toggle) reintroduces exactly that drift.
//!
//! ## Self-surfacing
//!
//! Every page carries a "Say" button whose fan-out lists the phrases
//! that page accepts. `editor_voice::commands()` was written for this —
//! its own doc calls it the man-pages principle applied to voice — and
//! `dictate::Ctl::phrases()` is the same list for dictation. A
//! capability that does not announce itself is a capability the user
//! must already know about.
//!
//! PURE: composes bottom_bar + editor_voice + dictate. No QML, no IO.

use crate::bottom_bar::{BarButton, BarButtonKind, BarOption, BottomBar};
use crate::dictate::Ctl;
use crate::editor_voice::{self, Editor};

/// A bar page. One per voice grammar, plus the neutral default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// Shell. The default; also the fallback for anything unrecognized.
    Terminal,
    /// A modal editor is in the foreground.
    Editor(Editor),
    /// A REPL prompt was detected. Carries the repl's label.
    Repl(String),
    /// A dictation session is running. Outranks everything else,
    /// because while dictating, the editor's normal-mode verbs are not
    /// what the user needs — the mode controls are.
    Dictation,
    Web,
}

impl Page {
    /// Stable id for persistence and for the QML page indicator.
    pub fn id(&self) -> String {
        match self {
            Page::Terminal => "terminal".into(),
            Page::Editor(e) => format!("editor:{}", e.label()),
            Page::Repl(r) => format!("repl:{}", r),
            Page::Dictation => "dictation".into(),
            Page::Web => "web".into(),
        }
    }

    /// Short title for the page indicator.
    pub fn title(&self) -> String {
        match self {
            Page::Terminal => "Shell".into(),
            Page::Editor(e) => e.label().into(),
            Page::Repl(r) => r.clone(),
            Page::Dictation => "Dictate".into(),
            Page::Web => "Web".into(),
        }
    }
}

/// Choose the page.
///
/// `grammar_label` is `grammar_context::grammar_label()`'s output —
/// `editor:helix`, `repl:python`, `terminal`, `web`. `dictating` is
/// session state, which the terminal cannot reveal, so it is passed
/// separately and takes precedence.
pub fn page_for(grammar_label: &str, dictating: bool) -> Page {
    if dictating {
        return Page::Dictation;
    }
    if let Some(rest) = grammar_label.strip_prefix("editor:") {
        // Round-trip through the same detector the label came from,
        // rather than re-parsing the name here. If a new editor is
        // added to detect_editor, this follows without an edit.
        if let Some(e) = editor_voice::detect_editor(rest) {
            return Page::Editor(e);
        }
    }
    if let Some(rest) = grammar_label.strip_prefix("repl:") {
        if !rest.is_empty() {
            return Page::Repl(rest.to_string());
        }
    }
    match grammar_label {
        "web" => Page::Web,
        _ => Page::Terminal,
    }
}

/// Live state the bar draws on — the same three lists `default_bar`
/// already takes.
#[derive(Debug, Default, Clone)]
pub struct Live {
    pub tabs: Vec<(String, String)>,
    pub workspaces: Vec<(String, String)>,
    pub quick: Vec<(String, String)>,
}

/// Build the bar for a page.
///
/// Invariant worth stating: Tabs is present on EVERY page. Navigation
/// that disappears when you open an editor is navigation the user
/// cannot rely on, and the whole point of the page mechanism is that it
/// changes what is offered, not whether you can get out.
pub fn bar_for(page: &Page, live: &Live) -> BottomBar {
    let mut bar = BottomBar::new();

    // Persistent leading button.
    let tab_opts: Vec<BarOption> =
        live.tabs.iter().map(|(id, l)| BarOption::new(id.clone(), l.clone())).collect();
    let mut tabs = BarButton::new("tabs", "Tabs", BarButtonKind::Tabs)
        .with_primary("tab:last")
        .with_options(tab_opts);
    if !live.tabs.is_empty() {
        tabs = tabs.with_badge(live.tabs.len().to_string());
    }
    tabs.enabled = !live.tabs.is_empty();
    bar.upsert(tabs);

    match page {
        Page::Terminal | Page::Web => {
            bar.upsert(workspaces_button(live));
            bar.upsert(quick_button(live));
        }
        Page::Editor(e) => {
            bar.upsert(
                BarButton::new("save", "Save", BarButtonKind::Custom).with_primary("editor:save"),
            );
            bar.upsert(
                BarButton::new("undo", "Undo", BarButtonKind::Custom).with_primary("editor:undo"),
            );
            bar.upsert(say_button(editor_phrases(*e)));
            bar.upsert(quick_button(live));
        }
        Page::Repl(_) => {
            bar.upsert(quick_button(live));
            bar.upsert(workspaces_button(live));
        }
        Page::Dictation => {
            // Ordered by how often you reach for them, not by the enum.
            for c in [Ctl::ScratchThat, Ctl::NewLine, Ctl::Indent, Ctl::Dedent, Ctl::Stop] {
                bar.upsert(ctl_button(c));
            }
            bar.upsert(say_button(dictation_phrases()));
        }
    }
    bar
}

/// The pages a horizontal swipe cycles through, in order. Manual
/// override for when the automatic choice is wrong — one gesture, not a
/// tab strip.
///
/// Dictation is only in the list while a session is live; offering a
/// dictation page you cannot use is worse than not offering it.
pub fn swipe_order(current: &Page) -> Vec<Page> {
    let mut v = vec![Page::Terminal, Page::Web];
    if let Page::Editor(e) = current {
        v.insert(1, Page::Editor(*e));
    }
    if let Page::Repl(r) = current {
        v.insert(1, Page::Repl(r.clone()));
    }
    if *current == Page::Dictation {
        v.insert(0, Page::Dictation);
    }
    v
}

fn workspaces_button(live: &Live) -> BarButton {
    let opts: Vec<BarOption> =
        live.workspaces.iter().map(|(id, l)| BarOption::new(id.clone(), l.clone())).collect();
    let mut b = BarButton::new("workspaces", "Spaces", BarButtonKind::Workspaces).with_options(opts);
    b.enabled = !live.workspaces.is_empty();
    b
}

fn quick_button(live: &Live) -> BarButton {
    let opts: Vec<BarOption> =
        live.quick.iter().map(|(id, l)| BarOption::new(id.clone(), l.clone())).collect();
    let mut b = BarButton::new("quick", "Launch", BarButtonKind::QuickLaunch).with_options(opts);
    b.enabled = !live.quick.is_empty();
    b
}

fn ctl_button(c: Ctl) -> BarButton {
    BarButton::new(ctl_id(c), title_case(c.label()), BarButtonKind::Custom)
        .with_primary(ctl_id(c))
}

/// Action id for a control. `ctl:` prefixed so the selection router can
/// dispatch on the namespace without a table.
pub fn ctl_id(c: Ctl) -> String {
    format!("ctl:{}", c.label().replace(' ', "_"))
}

/// The "what can I say right now?" button. Tap opens it; the fan-out is
/// the phrase list. Hint-only — selecting a phrase does NOT execute it,
/// it inserts it as a suggestion, because a list of things you can SAY
/// that fires when TAPPED teaches the wrong model of the feature.
fn say_button(phrases: Vec<(String, String)>) -> BarButton {
    let opts: Vec<BarOption> = phrases
        .into_iter()
        .map(|(phrase, what)| BarOption::new(format!("say:{phrase}"), format!("{phrase} — {what}")))
        .collect();
    let mut b = BarButton::new("say", "Say", BarButtonKind::Custom)
        .with_primary("say:open")
        .with_options(opts);
    b.enabled = !b.options.is_empty();
    b
}

fn editor_phrases(e: Editor) -> Vec<(String, String)> {
    editor_voice::commands(e)
        .into_iter()
        .map(|(p, d)| (p.to_string(), d.to_string()))
        .collect()
}

/// Dictation's speakable set, collapsed to one entry per control —
/// `Ctl::phrases()` lists every synonym, which is right for the
/// fast-path index and wrong for a menu.
fn dictation_phrases() -> Vec<(String, String)> {
    [
        (Ctl::NewLine, "break the line"),
        (Ctl::NewParagraph, "blank line between"),
        (Ctl::ScratchThat, "undo the last phrase"),
        (Ctl::Indent, "one level in"),
        (Ctl::Dedent, "one level out"),
        (Ctl::Stop, "leave dictation"),
    ]
    .into_iter()
    .map(|(c, d)| (c.label().to_string(), d.to_string()))
    .collect()
}

fn title_case(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> Live {
        Live {
            tabs: vec![("t1".into(), "shell".into()), ("t2".into(), "notes".into())],
            workspaces: vec![("w1".into(), "work".into())],
            quick: vec![("q1".into(), "cargo test".into())],
        }
    }

    #[test]
    fn page_follows_the_grammar_label() {
        assert_eq!(page_for("terminal", false), Page::Terminal);
        assert_eq!(page_for("web", false), Page::Web);
        assert_eq!(page_for("editor:helix", false), Page::Editor(Editor::Helix));
        assert_eq!(page_for("editor:vim", false), Page::Editor(Editor::Vim));
        assert_eq!(page_for("repl:python", false), Page::Repl("python".into()));
    }

    #[test]
    fn dictation_outranks_the_terminal_context() {
        // you are in helix AND dictating: the mode controls are what you
        // need, not helix's normal-mode verbs
        assert_eq!(page_for("editor:helix", true), Page::Dictation);
        assert_eq!(page_for("terminal", true), Page::Dictation);
    }

    #[test]
    fn unknown_labels_fall_back_rather_than_vanish() {
        assert_eq!(page_for("", false), Page::Terminal);
        assert_eq!(page_for("editor:emacs", false), Page::Terminal, "not a modal editor we model");
        assert_eq!(page_for("repl:", false), Page::Terminal);
        assert_eq!(page_for("something-new", false), Page::Terminal);
    }

    #[test]
    fn page_selection_agrees_with_the_editor_detector() {
        // the page must never claim an editor grammar the dispatcher
        // would not route to
        for cmd in ["hx", "helix", "vim", "nvim", "vi"] {
            let label = format!("editor:{cmd}");
            let expected = editor_voice::detect_editor(cmd).map(Page::Editor);
            assert_eq!(Some(page_for(&label, false)), expected, "{label}");
        }
    }

    #[test]
    fn tabs_are_on_every_page() {
        let l = live();
        for p in [
            Page::Terminal,
            Page::Web,
            Page::Editor(Editor::Helix),
            Page::Repl("python".into()),
            Page::Dictation,
        ] {
            let bar = bar_for(&p, &l);
            assert!(bar.find("tabs").is_some(), "{:?} lost navigation", p);
        }
    }

    #[test]
    fn tabs_button_badges_and_disables_itself() {
        let bar = bar_for(&Page::Terminal, &live());
        assert_eq!(bar.find("tabs").unwrap().badge, "2");
        let empty = bar_for(&Page::Terminal, &Live::default());
        assert!(!empty.find("tabs").unwrap().enabled);
        assert_eq!(empty.find("tabs").unwrap().badge, "");
    }

    #[test]
    fn editor_page_offers_save_undo_and_the_phrase_list() {
        let bar = bar_for(&Page::Editor(Editor::Helix), &live());
        assert_eq!(bar.find("save").unwrap().primary, "editor:save");
        assert_eq!(bar.find("undo").unwrap().primary, "editor:undo");
        let say = bar.find("say").unwrap();
        assert!(say.enabled);
        // sourced from editor_voice, not a second hand-written list
        assert_eq!(say.options.len(), editor_voice::commands(Editor::Helix).len());
        assert!(say.options.iter().any(|o| o.id.starts_with("say:")));
    }

    #[test]
    fn helix_gets_its_selection_verbs_and_vim_does_not() {
        let hx = bar_for(&Page::Editor(Editor::Helix), &live());
        let vi = bar_for(&Page::Editor(Editor::Vim), &live());
        let n_hx = hx.find("say").unwrap().options.len();
        let n_vi = vi.find("say").unwrap().options.len();
        assert!(n_hx > n_vi, "helix adds selection-first verbs: {n_hx} vs {n_vi}");
    }

    #[test]
    fn dictation_page_is_the_control_verbs() {
        let bar = bar_for(&Page::Dictation, &live());
        for c in [Ctl::ScratchThat, Ctl::NewLine, Ctl::Indent, Ctl::Dedent, Ctl::Stop] {
            let b = bar.find(&ctl_id(c)).unwrap_or_else(|| panic!("missing {:?}", c));
            assert_eq!(b.primary, ctl_id(c));
        }
        // scratch-that first: the one you reach for under pressure
        assert_eq!(bar.buttons()[1].id, ctl_id(Ctl::ScratchThat));
    }

    #[test]
    fn dictation_say_list_collapses_synonyms() {
        let bar = bar_for(&Page::Dictation, &live());
        let n = bar.find("say").unwrap().options.len();
        assert_eq!(n, 6, "one entry per control, not one per synonym");
        assert!(Ctl::phrases().len() > n, "the index list is deliberately longer");
    }

    #[test]
    fn ctl_ids_are_stable_and_namespaced() {
        assert_eq!(ctl_id(Ctl::NewLine), "ctl:new_line");
        assert_eq!(ctl_id(Ctl::ScratchThat), "ctl:scratch_that");
        // every control produces a distinct id
        let ids: Vec<String> =
            [Ctl::NewLine, Ctl::NewParagraph, Ctl::ScratchThat, Ctl::Indent, Ctl::Dedent, Ctl::Stop]
                .iter()
                .map(|c| ctl_id(*c))
                .collect();
        let mut u = ids.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), ids.len());
    }

    #[test]
    fn selection_routing_resolves_through_the_existing_router() {
        let bar = bar_for(&Page::Terminal, &live());
        let (kind, opt) = bar.resolve("tabs", "t2").unwrap();
        assert_eq!(kind, BarButtonKind::Tabs);
        assert_eq!(opt.label, "notes");
        assert!(bar.resolve("tabs", "nope").is_none());
    }

    #[test]
    fn say_options_do_not_collide_with_action_ids() {
        // "say:" and "ctl:" namespaces keep the router table-free
        let bar = bar_for(&Page::Dictation, &live());
        for o in &bar.find("say").unwrap().options {
            assert!(o.id.starts_with("say:"), "{}", o.id);
            assert!(!o.id.starts_with("ctl:"));
        }
    }

    #[test]
    fn swipe_order_only_offers_reachable_pages() {
        assert!(!swipe_order(&Page::Terminal).contains(&Page::Dictation));
        assert!(swipe_order(&Page::Dictation).contains(&Page::Dictation));
        let s = swipe_order(&Page::Editor(Editor::Vim));
        assert!(s.contains(&Page::Editor(Editor::Vim)));
        assert!(s.contains(&Page::Terminal));
    }

    #[test]
    fn page_ids_round_trip_for_persistence() {
        assert_eq!(Page::Editor(Editor::Helix).id(), "editor:helix");
        assert_eq!(page_for(&Page::Editor(Editor::Helix).id(), false), Page::Editor(Editor::Helix));
        assert_eq!(page_for(&Page::Repl("python".into()).id(), false), Page::Repl("python".into()));
        assert_eq!(page_for(&Page::Web.id(), false), Page::Web);
    }
}
