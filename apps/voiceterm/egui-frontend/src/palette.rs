//! palette — one command vocabulary, reachable by typing or by saying it.
//!
//! The app had eleven top-level tabs because that was the only way to reach a
//! surface. A tiling workspace does not want a tab per surface; it wants a way
//! to say **what** to show and **where** to put it. That is one sentence with
//! two halves, and both halves are the same whether they arrive from a text
//! box or a microphone:
//!
//! ```text
//!   "show me my mail in a new pane"   → OpenView { Mail, NewPane }
//!   "mail"                            → OpenView { Mail, Focused }
//!   "open the graph in a new tab"     → OpenView { Graph, NewTab }
//!   "open command palette"            → ShowPalette
//! ```
//!
//! ## Why the target is parsed, not defaulted
//!
//! "Show me my mail" and "show me my mail in a new pane" are different
//! requests, and a shell that always replaced the focused pane would destroy
//! what someone was looking at every time they glanced at something else. The
//! default is the focused pane because that is where attention already is;
//! saying otherwise is how you keep both.
//!
//! ## Voice and typing resolve through the same list
//!
//! `Command::ALL` is the vocabulary. The palette fuzzy-matches it with the
//! kit's scorer; the recogniser matches the same entries by keyword. A command
//! that exists in one and not the other is the class of bug where a feature is
//! reachable by mouse and mysteriously absent by voice, and keeping one list
//! is what prevents it.

use crate::shell::PaneView;

/// Where a view should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// Replace what the focused pane is showing.
    #[default]
    Focused,
    /// Split the focused pane and show it in the new half.
    NewPane,
    /// Open a new workspace tab.
    NewTab,
}

/// Everything the app can be asked to do from the palette or by voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpenView { view: PaneView, target: Target },
    /// Split without changing content — the tiling verb on its own.
    Split { vertical: bool },
    ClosePane,
    NewTab,
    NextTab,
    PrevTab,
    /// Show the palette itself, so "open command palette" is sayable.
    ShowPalette,
    ToggleRail,
    /// Settings had NO route but the session list's gear.
    ///
    /// That list now lives inside the tab column, and the column collapses to a
    /// chevron — so without this, collapsing it stranded settings entirely.
    /// Added with the collapse rather than after it: a control whose only door
    /// can be closed is a control you can lose.
    ToggleSettings,
    /// Web rendering settings — global, and for the site in front of you.
    ///
    /// Reached from here rather than given a permanent control, which is
    /// the point: the defaults are what almost everyone gets, and a knob
    /// on screen for something you touch twice a year is clutter that
    /// costs everyone.
    ToggleWebSettings,
    /// Change the ad filter for the CURRENT site only.
    ///
    /// A separate command from opening the panel because it is the thing
    /// people actually want: a page has eaten its own sidebar, or is
    /// unreadable through the ads, and the answer is one setting on one
    /// site. Making them find it in a panel is making them give up.
    SiteFilter(FilterLevel),
    /// Forget this site's rules and follow the globals again.
    SiteReset,
    /// Switch between touch chrome and voice chrome (`chrome_mode.rs`).
    ///
    /// `Chrome::toggle` is documented as "the ONLY way the mode changes on
    /// purpose", but until this command existed its only caller was the
    /// escape chord's fallback branch — so the deliberate switch the doc
    /// describes had no deliberate way to reach it. Ctrl+Shift+M now only
    /// escapes/resumes; this is how you choose voice chrome in the first
    /// place, from the palette or by voice like every other mode here.
    ToggleChrome,
    /// Show or hide the keycaps/summaries/date/say footer.
    ///
    /// Closed by default on desktop now (see the same commit's note on
    /// `compact_defaults_applied`) — a terminal that always shows five
    /// rows of chrome under the prompt isn't minimal, so it needed a real
    /// way back. The pointer path is a click-to-arm handle
    /// (`SlideMenu::show_tap`, next to it); this is the keyboard/voice one.
    ToggleKeycaps,
    /// Clip the current terminal selection into the session's note, with
    /// provenance — TODO_notes_clip.md's "highlight → card" core, the
    /// smallest slice its own suggested build order names first: "proves
    /// the spine end-to-end + immediately useful". `note_clip.rs`'s
    /// data model has been pure and tested since v0.1.641 with no caller
    /// anywhere in this frontend before this command existed.
    ClipSelection,
    /// Toggle jot mode: spoken text goes into the note as prose instead of to the command
    /// parser, until one of `jot::EXIT_PHRASES` is said. `TODO_notes_clip.md` step 2.
    ///
    /// A command rather than only a spoken phrase, because a mode you can only ENTER by
    /// voice is unreachable the moment the microphone mishears its own trigger.
    ToggleJotMode,
    /// "open <thing> [on <host>] [at line N] [in vim|helix]" — slot-parse the phrase, resolve
    /// the host against ~/.ssh/config, and STAGE the editor command on the terminal line.
    ///
    /// `editor_open` has been pure and tested since v0.1.654 and wired into the Qt build; it
    /// was never mounted in this frontend.
    OpenInEditor(String),
    /// Docs for whatever the terminal last ran — `TODO_doc_pane.md`'s implicit mode:
    /// `extract_query(last command) -> resolve -> open or stage`.
    DocsForLastCommand,
    /// Share the zettelkasten as a huge QR code — the picker with the
    /// paintbrush, two confirmation gates, then the code (`zk_share`).
    /// Reachable from the notes panes' palette like everything else here.
    ShareNotesQr,
    /// Open the peer handshake — the acoustic identity exchange
    /// (`peer_panel`) with THIS device's real identity (`identity`).
    /// The panel has been a complete, tested port with no way in since the
    /// port; the identity module is what it was waiting for.
    PeerHandshake,
    /// "how did I fix the nginx config" — open the command history already asking.
    /// The lead-in is stripped and the rest is `recall`'s query; an empty one is the
    /// plain history the sheet has always opened on.
    Recall(String),
}

/// The recall lead-ins, longest first so "what was the command that" does not match as
/// "what was" and leave "the command that" in the query.
const RECALL_LEADS: &[&str] = &[
    "what was the command that",
    "what was the command i",
    "the command that i",
    "the command i",
    "what did i do when i",
    "what did i do",
    "how did i",
    "last time i",
    "what was that",
    "remind me how i",
    "remind me what i",
];

/// Is this a question about what was run before, and if so what is being asked?
///
/// Returns the query with the lead-in and its filler removed: "how did I fix the nginx
/// config" → "fix the nginx config". An empty remainder is still a recall — "what did I
/// do" is the plain history — so this returns `Some("")` rather than `None`.
pub fn recall_query(phrase: &str) -> Option<String> {
    let p = phrase.trim().to_lowercase();
    let p = p.trim_end_matches(['?', '.', '!']).trim();
    let lead = RECALL_LEADS.iter().find(|l| p.starts_with(**l))?;
    let rest = p[lead.len()..].trim();
    // "how did I fix X" and "how did I do X" ask the same thing; the verb is filler when
    // it is one of these, and content when it is anything else ("how did I compile it").
    let rest = ["do ", "run ", "get ", "make it "]
        .iter()
        .find_map(|f| rest.strip_prefix(f))
        .unwrap_or(rest);
    Some(rest.trim().to_string())
}

/// The three filter levels, as a palette-addressable value.
///
/// Mirrors `ad_filter::FilterMode` rather than reusing it: the palette
/// vocabulary is a pure list this crate builds at startup, and pulling the
/// filter's type in here would make the command set depend on the page
/// pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterLevel { Off, Standard, Aggressive }

impl FilterLevel {
    pub fn label(self) -> &'static str {
        match self {
            FilterLevel::Off => "off",
            FilterLevel::Standard => "standard",
            FilterLevel::Aggressive => "aggressive",
        }
    }
}

/// One entry in the vocabulary: what it does, what it is called, and the words
/// that should find it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub command: Command,
    /// What the palette row reads.
    pub title: String,
    /// Extra words the recogniser and the fuzzy matcher should accept.
    pub keywords: &'static [&'static str],
}

/// The whole vocabulary, built once.
///
/// Views come first and in `PaneView::ALL` order, because that is the order
/// they appear everywhere else — a palette that lists them differently from
/// the pane menu makes the user learn two orders.
pub fn vocabulary() -> Vec<Entry> {
    let mut v: Vec<Entry> = Vec::new();
    // WEB SETTINGS. Keywords carry the words people actually say for this
    // — "ads", "reader", "block" — because nobody searching a palette for
    // relief from an advertisement types "rendering preferences".
    v.push(Entry {
        command: Command::ToggleWebSettings,
        title: "Web settings".into(),
        keywords: &["ads", "reader", "images", "page", "rendering", "filter"],
    });
    for lvl in [FilterLevel::Off, FilterLevel::Standard, FilterLevel::Aggressive] {
        v.push(Entry {
            command: Command::SiteFilter(lvl),
            title: format!("Ad filter {} for this site", lvl.label()),
            keywords: &["ads", "site", "block", "filter"],
        });
    }
    v.push(Entry {
        command: Command::SiteReset,
        title: "Forget this site's web settings".into(),
        keywords: &["ads", "site", "reset", "default"],
    });
    for view in PaneView::ALL {
        // Web panes carry a URL the model owns; opening one blank is a
        // different command and not this one.
        if view == PaneView::Web {
            continue;
        }
        v.push(Entry {
            command: Command::OpenView { view, target: Target::Focused },
            title: format!("Show {}", view.label().to_lowercase()),
            // Tried adding generic verb keywords ("open", "go to",
            // "switch to") here and reverted after `search()`'s real
            // scorer proved it does not do what it looks like it should.
            // `uikit_egui::fuzzy::score` requires the WHOLE query's
            // characters (spaces stripped) to be an ordered subsequence
            // of ONE haystack -- title OR a single keyword, never
            // combined. "switch to mail" is not a character subsequence
            // of "Show mail" (wrong letters in the wrong order), nor of a
            // keyword "switch to" alone (which has no "mail" in it at
            // all) -- so a generic verb keyword shared across every view
            // in this loop can never help a compound "verb + view name"
            // phrase match, regardless of which verb is chosen. It would
            // only ever fire on the verb typed ALONE, which ties every
            // view in the loop at the same score -- not a real
            // navigation improvement. A genuine fix needs PER-VIEW name
            // synonyms (e.g. "email" for Mail) that a compound query
            // like "email" alone would match via the title fallback
            // already in place today -- not attempted here, since
            // inventing synonym words per view without knowing what
            // users actually say is exactly the guessing this session
            // has tried not to do.
            keywords: &[],
        });
        v.push(Entry {
            command: Command::OpenView { view, target: Target::NewPane },
            title: format!("Show {} in a new pane", view.label().to_lowercase()),
            keywords: &["split"],
        });
        v.push(Entry {
            command: Command::OpenView { view, target: Target::NewTab },
            title: format!("Show {} in a new tab", view.label().to_lowercase()),
            keywords: &["tab"],
        });
    }
    v.push(Entry { command: Command::Split { vertical: false },
                   title: "Split right".into(), keywords: &["vsplit", "beside"] });
    v.push(Entry { command: Command::Split { vertical: true },
                   title: "Split down".into(), keywords: &["hsplit", "below"] });
    v.push(Entry { command: Command::ClosePane, title: "Close pane".into(),
                   keywords: &["kill"] });
    // NewTab/NextTab were the other two zero-keyword entries in this
    // vocabulary. NextTab's fix mirrors PrevTab's own "back" one line
    // below -- a deliberate one-word symmetric pair, not a longer
    // speculative list, matching this file's own restraint elsewhere.
    v.push(Entry { command: Command::NewTab, title: "New tab".into(), keywords: &["add tab"] });
    v.push(Entry { command: Command::NextTab, title: "Next tab".into(), keywords: &["forward"] });
    v.push(Entry { command: Command::PrevTab, title: "Previous tab".into(), keywords: &["back"] });
    v.push(Entry { command: Command::ToggleRail, title: "Toggle sidebar".into(),
                   keywords: &["rail", "hide", "collapse"] });
    v.push(Entry { command: Command::ToggleSettings, title: "Settings".into(),
                   keywords: &["preferences", "config", "options"] });
    v.push(Entry { command: Command::ToggleChrome, title: "Toggle voice mode".into(),
                   keywords: &["chrome", "compact", "hide controls", "minimal"] });
    v.push(Entry { command: Command::ToggleKeycaps, title: "Toggle toolbar".into(),
                   keywords: &["keycaps", "keys", "footer", "summaries", "show extras"] });
    v.push(Entry { command: Command::ClipSelection, title: "Clip selection to notes".into(),
                   keywords: &["clip this", "save this", "note this", "clipping"] });
    v.push(Entry { command: Command::Recall(String::new()), title: "What did I do…".into(),
                   keywords: &["recall", "history", "last time", "how did i", "the command that", "remember"] });
    v.push(Entry { command: Command::PeerHandshake, title: "Pair with a nearby device…".into(),
                   keywords: &["peer", "handshake", "pair", "dial", "device", "trust", "identity"] });
    v.push(Entry { command: Command::ShareNotesQr, title: "Share notes as a QR code…".into(),
                   keywords: &["qr", "share", "zettelkasten", "export", "code", "public"] });
    v.push(Entry { command: Command::ShowPalette, title: "Open command palette".into(),
                   keywords: &["commands"] });
    v
}

/// Rank the vocabulary against what the user typed.
///
/// Uses the kit's scorer rather than a local one, so the palette ranks the way
/// every other search in the fleet ranks. An empty query returns everything in
/// vocabulary order — a palette that opens blank hides the discoverability
/// that is most of its value.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let q = query.trim();
    if q.is_empty() {
        return entries.iter().collect();
    }
    let mut scored: Vec<(i32, usize, &Entry)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let mut best = uikit_egui::fuzzy::score(q, &e.title);
        for k in e.keywords {
            if let Some(s) = uikit_egui::fuzzy::score(q, k) {
                best = Some(best.map_or(s, |b: i32| b.max(s)));
            }
        }
        if let Some(s) = best {
            scored.push((s, i, e));
        }
    }
    // Ties break on vocabulary position, so the list cannot reshuffle between
    // keystrokes that score the same — a moving target is unclickable.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, e)| e).collect()
}

/// Where a spoken phrase says to put things.
fn target_of(phrase: &str) -> Target {
    // Checked before the view, because "new tab" contains "tab" and several
    // view names do not care. Tab wins over pane when both appear, since
    // "a new tab" is the more specific request and saying both is a
    // correction mid-sentence.
    if phrase.contains("new tab") || phrase.contains("another tab") {
        Target::NewTab
    } else if phrase.contains("new pane") || phrase.contains("split")
        || phrase.contains("beside") || phrase.contains("next to")
    {
        Target::NewPane
    } else {
        Target::Focused
    }
}

/// Resolve a spoken phrase to a command.
///
/// Deliberately narrow: it matches a view name plus an optional target, and
/// the standalone verbs. Anything else returns None so the caller can fall
/// through to its existing command search rather than this guessing — a voice
/// path that half-understands is worse than one that declines, because the
/// user cannot tell which half it got.
pub fn parse_phrase(phrase: &str) -> Option<Command> {
    let p = phrase.to_lowercase();
    let p = p.trim();
    if p.is_empty() {
        return None;
    }
    // The palette itself, so it is reachable without knowing a shortcut.
    // RECALL, before the other prefixes: "how did I split a pane" is a question about
    // history, not a request to split one, and the view names would otherwise win.
    if let Some(q) = recall_query(&p) {
        return Some(Command::Recall(q));
    }
    if p.contains("pair") || p.contains("handshake") || (p.contains("dial") && !p.contains("dialog")) {
        return Some(Command::PeerHandshake);
    }
    if (p.contains("share") || p.contains("export")) && (p.contains("qr") || p.contains("notes") || p.contains("zettel")) {
        return Some(Command::ShareNotesQr);
    }
    if p.contains("command palette") || p.contains("show commands") {
        return Some(Command::ShowPalette);
    }
    if p.contains("close pane") || p.contains("close this pane") {
        return Some(Command::ClosePane);
    }
    if p.contains("split") && !p.contains("show") && !p.contains("open") {
        return Some(Command::Split { vertical: p.contains("down") || p.contains("below") });
    }
    // "new tab" alone is a tab; with a view named it is a PLACEMENT for that
    // view, handled below.
    let names_a_view = PaneView::ALL.iter()
        .any(|v| p.contains(&v.label().to_lowercase()));
    if p.contains("new tab") && !names_a_view {
        return Some(Command::NewTab);
    }
    if p.contains("next tab") {
        return Some(Command::NextTab);
    }
    if p.contains("previous tab") || p.contains("last tab") {
        return Some(Command::PrevTab);
    }
    if p.contains("sidebar") || p.contains("side bar") {
        return Some(Command::ToggleRail);
    }
    // Before the view-name scan below: "settings" is not a pane view, and
    // leaving it to fall through would match nothing at all.
    if p.contains("settings") || p.contains("preferences") {
        return Some(Command::ToggleSettings);
    }
    if p.contains("voice mode") || p.contains("voice chrome") || p.contains("touch mode") {
        return Some(Command::ToggleChrome);
    }
    if p.contains("toolbar") || p.contains("keycaps") || p.contains("show extras") {
        return Some(Command::ToggleKeycaps);
    }
    if p.contains("clip this") || p.contains("save this") || p.contains("note this") {
        return Some(Command::ClipSelection);
    }
    // A view name anywhere in the sentence. Longest label first, so "mail"
    // inside a longer name cannot win over the name itself.
    let mut named: Vec<PaneView> = PaneView::ALL.to_vec();
    named.sort_by_key(|v| std::cmp::Reverse(v.label().len()));
    for view in named {
        let label = view.label().to_lowercase();
        if p.contains(&label) {
            if view == PaneView::Web {
                // A web pane needs a URL, which this grammar does not carry.
                return None;
            }
            return Some(Command::OpenView { view, target: target_of(p) });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_carries_both_what_and_where() {
        assert_eq!(parse_phrase("show me my mail in a new pane"),
                   Some(Command::OpenView { view: PaneView::Mail, target: Target::NewPane }));
        assert_eq!(parse_phrase("open the graph in a new tab"),
                   Some(Command::OpenView { view: PaneView::Graph, target: Target::NewTab }));
        // No target named: the focused pane, because that is where attention
        // already is.
        assert_eq!(parse_phrase("show me my mail"),
                   Some(Command::OpenView { view: PaneView::Mail, target: Target::Focused }));
        // "beside" and "next to" are the same request as "new pane".
        assert_eq!(parse_phrase("put the docs next to this"),
                   Some(Command::OpenView { view: PaneView::Docs, target: Target::NewPane }));
    }

    #[test]
    fn the_palette_is_reachable_by_saying_its_name() {
        assert_eq!(parse_phrase("open command palette"), Some(Command::ShowPalette));
        assert_eq!(parse_phrase("show commands"), Some(Command::ShowPalette));
    }

    #[test]
    fn a_phrase_it_does_not_understand_declines_rather_than_guessing() {
        // The caller falls through to command_search, which is what that
        // module exists for. Half-understanding is worse than declining: the
        // user cannot tell which half landed.
        assert_eq!(parse_phrase("delete the old logs"), None);
        assert_eq!(parse_phrase(""), None);
        // A web pane needs a URL this grammar does not carry, so it declines
        // rather than opening a blank one.
        assert_eq!(parse_phrase("show me the web"), None);
    }

    #[test]
    fn the_two_real_zero_keyword_gaps_are_now_reachable_by_synonym() {
        // Confirms the fix has real effect through search() -- not just
        // that the array literal compiles, but that fuzzy::score actually
        // finds these entries by a word that never appears in the title.
        //
        // A third candidate fix (generic "open"/"go to"/"switch to"
        // keywords on the per-view OpenView::Focused entries) was tried
        // and REVERTED -- see that entry's own comment. `search()`'s real
        // scorer requires the whole query to be a character subsequence
        // of ONE field (title or a single keyword, never combined), so a
        // compound query like "switch to mail" can never match a generic
        // keyword "switch to" plus a title-only "mail" -- confirmed by
        // this test failing against that assumption before the revert.
        let entries = vocabulary();
        let has_command = |hits: &[&Entry], want: &Command| hits.iter().any(|e| &e.command == want);

        // "add tab" -- the title is "New tab". A COMPLETE alternate
        // phrase, not a generic modifier, so the compound-query problem
        // above does not apply here: the whole query is one field's
        // worth of characters.
        let hits = search(&entries, "add tab");
        assert!(has_command(&hits, &Command::NewTab), "add tab should find New tab via the new keyword");

        // "forward" -- the title is "Next tab"; mirrors PrevTab's "back".
        let hits = search(&entries, "forward");
        assert!(has_command(&hits, &Command::NextTab), "forward should find Next tab via the new keyword");
    }

    #[test]
    fn tiling_verbs_stand_alone() {
        assert_eq!(parse_phrase("split down"), Some(Command::Split { vertical: true }));
        assert_eq!(parse_phrase("split"), Some(Command::Split { vertical: false }));
        assert_eq!(parse_phrase("close pane"), Some(Command::ClosePane));
        assert_eq!(parse_phrase("next tab"), Some(Command::NextTab));
        assert_eq!(parse_phrase("hide the sidebar"), Some(Command::ToggleRail));
    }

    /// Settings must be reachable without the session list.
    ///
    /// Its only route was the gear in that list, which now lives inside the tab
    /// column — and the column collapses. Without a second door, collapsing
    /// strands settings, and the control that would restore the list is itself
    /// in the list.
    #[test]
    fn settings_is_reachable_by_phrase_and_by_palette() {
        assert_eq!(parse_phrase("open settings"), Some(Command::ToggleSettings));
        assert_eq!(parse_phrase("show me preferences"), Some(Command::ToggleSettings));
        assert!(vocabulary().iter().any(|e| e.command == Command::ToggleSettings),
                "settings must appear in the palette list, not only as a phrase");
        // And it must not have stolen the sidebar phrase.
        assert_eq!(parse_phrase("hide the sidebar"), Some(Command::ToggleRail));
        // "new tab" alone is a tab; with a view it is a placement.
        assert_eq!(parse_phrase("new tab"), Some(Command::NewTab));
        assert_eq!(parse_phrase("mail in a new tab"),
                   Some(Command::OpenView { view: PaneView::Mail, target: Target::NewTab }));
    }

    /// `ToggleChrome` exists precisely because voice mode had no deliberate
    /// entry point — see the comment on the command. Pin both doors open,
    /// the same way settings' second door is pinned above.
    #[test]
    fn voice_chrome_is_reachable_by_phrase_and_by_palette() {
        assert_eq!(parse_phrase("switch to voice mode"), Some(Command::ToggleChrome));
        assert_eq!(parse_phrase("go to touch mode"), Some(Command::ToggleChrome));
        assert!(vocabulary().iter().any(|e| e.command == Command::ToggleChrome),
                "voice chrome must appear in the palette list, not only as a phrase");
    }

    /// The keycaps/summaries/date/say footer defaults closed on desktop now
    /// (it used to default open — see compact_defaults_applied's comment);
    /// this is the keyboard/voice door back, alongside the pointer one
    /// (a click-to-arm SlideMenu handle, in the binary).
    #[test]
    fn the_footer_is_reachable_by_phrase_and_by_palette() {
        assert_eq!(parse_phrase("show me the toolbar"), Some(Command::ToggleKeycaps));
        assert!(vocabulary().iter().any(|e| e.command == Command::ToggleKeycaps),
                "the footer toggle must appear in the palette list, not only as a phrase");
    }

    /// TODO_notes_clip.md's "highlight → card" core needed a way in; this
    /// pins both doors, same as every other command added this session.
    #[test]
    fn clip_selection_is_reachable_by_phrase_and_by_palette() {
        assert_eq!(recall_query("How did I fix the nginx config?"), Some("fix the nginx config".into()));
        assert_eq!(recall_query("what was the command that unpacked it"), Some("unpacked it".into()));
        assert_eq!(recall_query("what did I do"), Some(String::new()), "the plain history is still a recall");
        assert_eq!(recall_query("how did i do the docker thing"), Some("the docker thing".into()), "filler verb dropped");
        assert_eq!(recall_query("how did i compile it"), Some("compile it".into()), "a real verb is kept");
        assert_eq!(recall_query("split the pane"), None);
        assert_eq!(parse_phrase("how did i split a pane"), Some(Command::Recall("split a pane".into())),
                   "a question about history is not a request to split one");
        assert!(vocabulary().iter().any(|e| matches!(e.command, Command::Recall(_))));
        assert_eq!(parse_phrase("pair with a nearby device"), Some(Command::PeerHandshake));
        assert_eq!(parse_phrase("start a handshake"), Some(Command::PeerHandshake));
        assert!(vocabulary().iter().any(|e| e.command == Command::PeerHandshake));
        assert_eq!(parse_phrase("share my notes as a qr code"), Some(Command::ShareNotesQr));
        assert_eq!(parse_phrase("export the zettelkasten"), Some(Command::ShareNotesQr));
        assert!(vocabulary().iter().any(|e| e.command == Command::ShareNotesQr),
                "the share is in the typed list too");
        assert_eq!(parse_phrase("clip this"), Some(Command::ClipSelection));
        assert_eq!(parse_phrase("save this to my notes"), Some(Command::ClipSelection));
        assert!(vocabulary().iter().any(|e| e.command == Command::ClipSelection),
                "clip-to-notes must appear in the palette list, not only as a phrase");
    }

    /// A swipe must reach tabs by the same route as the palette and the voice
    /// grammar.
    ///
    /// The bin maps `GestureFrame::page_jump` onto these two commands rather
    /// than calling the workspace directly. That is the property worth
    /// pinning: three input paths, one command vocabulary. If someone later
    /// wires a swipe straight to `WsCall::Command("next tab")`, tab switching
    /// has two implementations and they will drift — which is exactly what
    /// happened to the ANC gates in bo-suite.
    #[test]
    fn a_swipe_has_a_command_to_land_on() {
        let v = vocabulary();
        for cmd in [Command::NextTab, Command::PrevTab] {
            assert!(v.iter().any(|e| e.command == cmd),
                    "{cmd:?} must exist in the one vocabulary a swipe routes through");
        }
        // And they are reachable by voice too, so the three paths agree.
        assert_eq!(parse_phrase("next tab"), Some(Command::NextTab));
        assert_eq!(parse_phrase("previous tab"), Some(Command::PrevTab));
    }

    #[test]
    fn typing_and_speaking_reach_the_same_commands() {
        let v = vocabulary();
        // Everything the grammar can produce is also in the typed list, or a
        // feature is reachable by voice and mysteriously absent by mouse.
        for phrase in ["show me my mail in a new pane", "open the graph in a new tab",
                       "close pane", "new tab", "open command palette"] {
            let spoken = parse_phrase(phrase).expect(phrase);
            assert!(v.iter().any(|e| e.command == spoken),
                    "{phrase} resolves to {spoken:?}, which the palette cannot offer");
        }
    }

    #[test]
    fn an_empty_query_lists_everything_and_ranking_is_stable() {
        let v = vocabulary();
        assert_eq!(search(&v, "").len(), v.len(), "a blank palette hides its own value");
        let hits = search(&v, "mail");
        assert!(!hits.is_empty());
        assert!(hits[0].title.contains("mail"));
        // The same query twice gives the same order.
        let again = search(&v, "mail");
        assert_eq!(hits.iter().map(|e| &e.title).collect::<Vec<_>>(),
                   again.iter().map(|e| &e.title).collect::<Vec<_>>());
    }
}
