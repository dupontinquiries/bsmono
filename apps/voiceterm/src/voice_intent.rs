// src/voice_intent.rs
// Parse voice transcription into actionable intents.

use std::collections::HashMap;
use crate::system_info;
// Examples:
//   "open hacker news"          -> Navigate("https://news.ycombinator.com")
//   "search rust async"         -> SearchWeb("rust async")
//   "go back" / "back"          -> GoBack
//   "show history"              -> OpenPalette("archive")
//   "switch to terminal"        -> SwitchTab("term")
//   "in terminal type ls"       -> TerminalCommand("ls")
//   "man find"                  -> TerminalCommand("man find")
//   "scroll down" / "scroll up" -> Scroll(dir)
//   "find on page rust"         -> FindInPage("rust")
//   "reload"                    -> Reload
//   "quit" / "close"            -> Quit


#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Navigate(String),
    SearchWeb(String),
    GoBack,
    GoForward,
    Reload,
    Scroll(i32),
    ScrollPage(i32),
    GoTop,
    GoBottom,
    FindInPage(String),
    OpenPalette(String),
    SwitchTab(String),
    TerminalCommand(String),
    OpenSettings,
    OpenHomepage,
    Quit,
    SystemInfo(String, String),
    EmailCompose(String, String),
    FileAppend(String, String),
    LlmFallback(String),
    /// Click a link by visible text / partial match. Resolved at execution time
    /// against current page links. Param is the search text.
    ClickLink(String),
    /// v0.1.596: click the Nth element of a kind ("the third link", "the
    /// last button"). ordinal is 1-based; -1 means "last". kind is
    /// "link"/"button"/"image"/"field"/"item". Resolved at execution
    /// time against the page's elements of that kind.
    ClickPositional { ordinal: i32, kind: String },
    /// Scroll to a heading on the current page. Param is heading text or anchor.
    ScrollToHeading(String),
    /// Copy something to clipboard. Param is what: "url", "title", "selection",
    /// "page_text", "last_terminal_output", "last_command"
    Copy(String),
    /// Paste clipboard into URL bar / terminal / find field
    Paste(String),  // target: "url", "term", "find"
    /// Rerun the last terminal command
    RerunLast,
    /// Reverse search history. Param is search query.
    HistorySearch(String),
    /// "Turn off X" / "stop X" — toggle settings
    Toggle(String),  // target: "ads", "monospace", "compat_ua"
    /// Generic dictation — type the words into focused input
    Dictate(String),
    /// Find a term across page text, terminal scrollback, history, archive — multi-source picker
    FindAnywhere(String),
    /// Workflow recorder: begin capturing commands under a name.
    /// v0.1.115. Param: workflow name.
    WorkflowStartRecording(String),
    /// Workflow recorder: stop capturing and save.
    WorkflowStopRecording,
    /// Workflow recorder: replay a saved workflow by name.
    WorkflowPlayback(String),
    Freeform(String),
    /// v0.1.255 (GITLOG-3): rollback the most recent
    /// mutating command. Triggered by "undo", "undo that",
    /// "undo last command", "rollback".
    Rollback,
    /// v0.1.273: navigate to the in-app audit log viewer at
    /// `web2qml://audit`. Triggered by "show audit log",
    /// "show command history", "what did I do", etc.
    ShowAuditLog,
    /// v0.1.277: navigate to filtered audit log for one
    /// speaker. Triggered by "show audit log for X",
    /// "what did X do", "show X's commands". Param is the
    /// speaker name (matched case-insensitive, partial).
    ShowAuditLogForSpeaker(String),
    /// v0.1.274 (GITLOG-5): navigate to the branch tree viewer
    /// at `web2qml://branches`. Triggered by "show branches",
    /// "show command tree", "show forks".
    ShowBranches,
    /// v0.1.276: verify hash-chain integrity of the audit log.
    /// Triggered by "verify audit log", "check audit log",
    /// "verify integrity". Returns toast with result.
    VerifyAuditLog,
    /// v0.1.292 (TAB voice): open a new browser tab. Optional
    /// param is the URL/site to open in the new tab. Empty
    /// param means "fork current page" (open current URL as
    /// a new tab).
    /// Triggered by "open new tab", "new tab",
    /// "open <site> in new tab", "fork tab".
    TabOpen(String),
    /// v0.1.292: switch to the next tab in the list (wraps).
    /// Triggered by "next tab", "switch tab", "other tab".
    TabNext,
    /// v0.1.746: native terminal pane/split management — carries the raw
    /// phrase to the backend's ws_command (which re-parses via the tested
    /// terminal_workspace::parse_command). Matched only for the unambiguous
    /// pane vocabulary ("new pane [dir]", "split [dir]", "close pane", "focus
    /// left/right/up/down", "resize <dir>") so it never collides with the
    /// browser TabNext/TabOpen intents. Terminal-tab verbs are deferred (they
    /// overlap browser "tab" phrasing — needs a UX decision).
    TermPane(String),
    /// v0.1.766: open the build-error menu (summarize the focused pane's recent
    /// cargo/cmake output). Phrases: "show errors", "show build errors",
    /// "what are the errors", "show me the build errors".
    ShowBuildErrors,
    /// v0.1.292: switch to the previous tab in the list
    /// (wraps).
    /// Triggered by "previous tab", "prev tab", "back tab",
    /// "last tab".
    TabPrev,
    /// v0.1.292: close the currently-active tab. Never closes
    /// the last tab (the strip handles that constraint).
    /// Triggered by "close tab", "close this tab", "close
    /// current tab".
    TabClose,
    /// v0.1.296 (Tier1 diag): show acoustic-index status as a
    /// toast. Triggered by "tier 1 status", "tier one status",
    /// "show tier 1", "acoustic status", "show acoustic
    /// stats", "exemplar count". Returns the exemplar count
    /// and SHADOW/PROMOTE mode.
    Tier1Status,
    /// v0.1.300 (Tier1-3): show SHADOW-mode histogram summary
    /// as a toast (exemplar count + match rate + suggested
    /// threshold). Triggered by "tier 1 histogram", "show
    /// histogram", "show distances", "threshold suggestion",
    /// "suggested threshold".
    Tier1Histogram,
    /// v0.1.302 (Tier1-4): flip PROMOTE mode ON at runtime.
    /// Triggered by "promote tier 1", "promote acoustic
    /// index", "enable promote", "turn on promote".
    Tier1PromoteOn,
    /// v0.1.302 (Tier1-4): flip PROMOTE mode OFF (back to
    /// SHADOW). Triggered by "demote tier 1", "disable
    /// promote", "turn off promote", "back to shadow".
    Tier1PromoteOff,
    /// v0.1.304: set PROMOTE threshold from voice. Param is
    /// the numeric threshold for same-speaker (cross-speaker
    /// is set to 2x same automatically). Triggered by
    /// "set threshold to <X>", "tier 1 threshold <X>",
    /// "promote threshold <X>".
    Tier1SetThreshold(f32),
    /// v0.1.305 (Tier1-4 auto-tune): apply the histogram's
    /// suggested threshold + enable PROMOTE in one step.
    /// Triggered by "auto tune tier 1", "auto-tune tier
    /// one", "tune tier 1", "calibrate tier 1".
    Tier1AutoTune,
    /// v0.1.297 (TAB-3): save current tabs as a named
    /// workspace under the active speaker. Triggered by
    /// "save as <name>", "save workspace as <name>",
    /// "save this as <name>". Param: workspace name.
    WorkspaceSaveAs(String),
    /// v0.1.297 (TAB-3): load a previously-saved named
    /// workspace under the active speaker. Triggered by
    /// "load workspace <name>", "switch to <name>
    /// workspace", "open <name> workspace".
    /// Param: workspace name.
    WorkspaceLoad(String),
    /// v0.1.297 (TAB-3): list saved workspaces as a toast
    /// for the active speaker. Triggered by "list
    /// workspaces", "show workspaces", "my workspaces".
    WorkspaceList,
    /// v0.1.301 (TAB-3): delete a named workspace. Refuses
    /// "default". Triggered by "delete workspace <name>",
    /// "remove workspace <name>", "forget workspace <name>".
    WorkspaceDelete(String),
    /// v0.1.310 (TAB-3): rename a named workspace. Triggered
    /// by "rename workspace <old> to <new>", "rename <old>
    /// workspace to <new>", "rename <old> to <new>". Param
    /// carries both names separated by `\x1F` (unit
    /// separator) to keep the Intent enum mono-arg.
    WorkspaceRename(String),
    /// v0.1.311 (TAB-3): show contents of a named workspace
    /// as a toast (tab count + first few short URLs).
    /// Triggered by "show workspace <name>", "describe
    /// workspace <name>", "what's in <name> workspace",
    /// "inspect workspace <name>".
    WorkspaceInspect(String),
    /// v0.1.313 (TAB-3): copy a workspace under a new name.
    /// Triggered by "copy workspace <src> to <dst>", "copy
    /// <src> workspace to <dst>", "fork <src> to <dst>",
    /// "duplicate <src> as <dst>". Param: "src\x1Fdst".
    WorkspaceCopy(String),
    /// v0.1.314 (TAB-3): show current active workspace name
    /// as a toast. Triggered by "what workspace am I in",
    /// "current workspace", "which workspace", "what's my
    /// workspace".
    WorkspaceCurrent,
    /// v0.1.315 (TAB-3): explicit save of the current workspace
    /// to disk (autosave normally fires on mutations, but the
    /// user may want a confidence beat after a manual change).
    /// Triggered by "save workspace", "save my workspace",
    /// "save current workspace", "snapshot workspace".
    WorkspaceSaveNow,
    /// v0.1.319 (Mermaid+/cards arc 2.5.6-qml): open the
    /// pan/zoomable cards canvas showing the connections graph.
    /// Triggered by "show connections", "show graph",
    /// "open graph view", "show cards", "connections view".
    OpenConnectionsView,
    /// v0.1.643: "walk me through my last session" — start the
    /// Prezi-style session walk over the last session's pages. The walk
    /// UI + backend (session_walk_data) already existed; this wires the
    /// voice phrase that had no intent.
    WalkLastSession,
    /// v0.1.649: "what did I do last time" / "how did I fix X" — recall
    /// past commands ranked by the query. Carries the extracted query
    /// (lead-in stripped); empty = "what did I just do" (most recent).
    RecallCommands(String),
    /// v0.1.654: "open <thing> [on <host>] [at line N] [in vim|helix]" /
    /// "edit <thing>…" — open a file in the editor with advanced context
    /// (host, line, editor). Carries the raw phrase; the backend slot-
    /// parses (editor_open::parse_open_request), resolves the path + host,
    /// and spawns. Only triggers when an editor/line/host/edit marker is
    /// present, so plain "open <site>" stays a navigation.
    OpenInEditor(String),
    /// v0.1.323 (cards arc 2.5.2): apply a fuzzy filter to the
    /// cards canvas. Triggered by "filter cards <query>",
    /// "filter to <query>", "search cards for <query>", etc.
    /// If the canvas isn't open, opens it first.
    FilterCards(String),
    /// v0.1.323 (cards arc 2.5.2): clear the active card
    /// filter. Triggered by "clear filter", "show all cards",
    /// "reset filter".
    ClearCardsFilter,
    /// v0.1.324 (cards arc 2.5.3): add a user tag to the
    /// currently-previewed card. Triggered by "tag this <X>",
    /// "tag it <X>", "add tag <X>" — requires the preview
    /// overlay to be open (it uses `_previewId` for context).
    TagCard(String),
    /// v0.1.324: remove a user tag from the currently-previewed
    /// card. Triggered by "remove tag <X>", "untag <X>", "untag
    /// this <X>".
    UntagCard(String),
    /// v0.1.324: filter the cards canvas to nodes carrying a
    /// specific user tag. Triggered by "filter to @work",
    /// "show <tag> tagged cards", "show only <tag>". Different
    /// from FilterCards in that this matches on tag exactly,
    /// not on fuzzy substring.
    FilterByTag(String),
    /// v0.1.327 (Mermaid+/cards arc 2.7.3): open the cards
    /// canvas in Mermaid mode with a stock demo diagram, so
    /// the user can see the parser working end-to-end without
    /// authoring a source file. Triggered by "show mermaid
    /// demo", "show diagram demo", "demo diagram".
    ShowMermaidDemo,
    /// v0.1.329 (cards arc): tilt the cards canvas view to a
    /// pseudo-3D perspective. Angle in degrees (0..70). 0 =
    /// flat 2D. Common phrasings: "tilt graph", "tilt 30",
    /// "tilt 45 degrees", "tilt view".
    TiltGraph(f32),
    /// v0.1.698 (niftyplot view): rotate the layered diagram to a named
    /// orientation preset (flat/fan/rotate/stack/iso/tilt). The string is the
    /// preset word. Voice: "rotate the diagram", "fan the layers",
    /// "isometric view", "flatten" (→ FlattenGraph already exists).
    RotateGraph(String),
    /// v0.1.698: smart expansion — reveal one more / one fewer ring of nodes
    /// around the focused node. "expand", "go deeper", "show more" /
    /// "collapse", "less", "show fewer".
    ExpandNode,
    CollapseNode,
    /// v0.1.329 (cards arc): flatten the cards canvas back to
    /// 2D (tilt 0). Triggered by "flatten graph", "collapse
    /// to 2D", "flatten view", "go flat".
    FlattenGraph,
    /// v0.1.338 (cards arc): close the cards canvas / connections
    /// view popup. Reverse of OpenConnectionsView. Phrasings:
    /// "close graph", "close cards", "close map", "back to page",
    /// "exit graph", "dismiss graph".
    CloseConnectionsView,
    /// v0.1.338 (cards arc): reset zoom + scroll on the cards
    /// canvas so the whole graph fits the visible area again.
    /// Useful when the user has panned/zoomed deep and wants
    /// to recenter. Phrasings: "reset zoom", "fit graph",
    /// "center graph", "recenter", "fit cards".
    ResetCardsZoom,
    /// v0.1.343 (niftyplot arc): scope the cards canvas to
    /// show only a named plane / group, dimming everything else.
    /// The string is the plane name. Voice phrasings:
    /// "focus services", "show only services", "zoom into auth".
    FocusPlane(String),
    /// v0.1.343 (niftyplot arc): clear plane scope, returning
    /// to full-diagram view. Phrasings: "unfocus", "show
    /// everything", "show full diagram".
    UnfocusPlane,
    /// v0.1.343 (niftyplot arc): read the current diagram's
    /// parser warnings aloud (debug footer contents). Phrasings:
    /// "what's wrong", "read warnings", "what did the parser say".
    ReadDiagramWarnings,
    /// v0.1.368: zero a speaker's rolling cadence EMA. Useful
    /// when the user's speaking style has shifted (illness, new
    /// mic, recovery from voice strain) and the accumulated
    /// stats no longer reflect current cadence reality. Param
    /// is the target speaker name, or empty/"me"/"my"/"myself"
    /// to mean "the active speaker (the one issuing this
    /// command)."
    ///
    /// Phrasings:
    ///   "reset my cadence"
    ///   "reset cadence"
    ///   "clear my cadence"
    ///   "reset cadence for Bob"
    ///   "clear Bob's cadence"
    ///   "reset speech cadence"
    ResetCadence(String),
    /// v0.1.377 (discoverability arc): open the recipes page.
    /// Voice-discoverable surface for users who don't see the
    /// first-launch hint and don't check the settings link.
    ///
    /// Phrasings:
    ///   "show recipes"
    ///   "what can you do"
    ///   "what can web2qml do"
    ///   "what can I say"
    ///   "show capabilities"
    ///   "open recipes"
    ShowRecipes,
    /// v0.1.386 (bookmarks): save a URL as a bookmark.
    /// When the inner String is empty, callers should
    /// substitute the current page URL.
    ///
    /// Phrasings:
    ///   "save this" / "save this page" / "bookmark this"
    ///   "save url <X>" / "bookmark url <X>"
    ///   "remember this page" / "remember this"
    SaveUrl(String),
    /// v0.1.386 (bookmarks): show the saved-URLs page.
    ///
    /// Phrasings:
    ///   "show bookmarks" / "open bookmarks"
    ///   "my bookmarks" / "show my saved pages"
    ShowBookmarks,
    /// v0.1.389: jump to an open browser tab whose URL
    /// contains a substring. Useful when several tabs are
    /// open — "switch to the github tab" beats counting
    /// tab indices.
    ///
    /// Inner String: the substring to match. Match is
    /// case-insensitive; first matching tab (left-to-right
    /// order) wins. When no tab matches, the dispatch
    /// surfaces a toast.
    ///
    /// Phrasings:
    ///   "switch to the X tab"
    ///   "go to the X tab"
    ///   "switch to tab with X"
    ///   "go to tab with X"
    ///   "go to X tab"
    SwitchToTabMatching(String),
    /// v0.1.390: save current browser+terminal state as a
    /// named workspace under the active speaker. Backend
    /// already has `workspace_save_as` qt_method since
    /// v0.1.298; this just adds the voice path.
    ///
    /// Phrasings:
    ///   "save workspace as <name>"
    ///   "save this workspace as <name>"
    ///   "save my workspace as <name>"
    SaveWorkspaceAs(String),
    /// v0.1.390: load a named workspace under the active
    /// speaker, swapping `_tabs` to it.
    ///
    /// Phrasings:
    ///   "load workspace <name>"
    ///   "load the <name> workspace"
    ///   "switch to workspace <name>"
    ///   "switch to <name> workspace"
    ///   "open workspace <name>"
    LoadWorkspace(String),
    /// v0.1.394 (drag-drop arc): open a platform file
    /// chooser. QML's FileDialog is the actual UI; this
    /// intent just signals open_file_dialog_requested.
    ///
    /// Phrasings:
    ///   "open file" / "open a file"
    ///   "open file picker" / "open file dialog"
    ///   "pick a file" / "choose a file"
    ///   "browse files" / "browse for a file"
    OpenFileDialog,
    /// v0.1.387 (bookmarks step 2): show bookmarks filtered
    /// by tag.
    ///
    /// Phrasings:
    ///   "show <tag> bookmarks"
    ///   "show bookmarks tagged <tag>"
    ///   "filter bookmarks by <tag>"
    ///   "<tag> bookmarks"
    ShowBookmarksTagged(String),
    /// v0.1.382 (session arc, step 3): summarize the most
    /// recent session. Reads the audit-log slice for the
    /// current/most-recent session_id and produces a 1-2
    /// sentence LLM summary of what was done.
    ///
    /// Phrasings:
    ///   "what did I do last session"
    ///   "what did I do today"
    ///   "summarize my session"
    ///   "summarize this session"
    ///   "what have I been doing"
    SummarizeSession,
    /// v0.1.385 (discoverability arc): context-aware help.
    /// What CAN the user do RIGHT NOW given what's on screen?
    /// Different from ShowRecipes (which is the full
    /// catalog): this is a focused 4-6 item list for the
    /// active surface.
    ///
    /// Phrasings:
    ///   "what can I do here"
    ///   "what's available"
    ///   "show shortcuts for this page"
    ///   "what's on this page"
    ///   "what now"
    ContextHelp,
    /// v0.1.678: "edit this note" / "edit this" / "open the editor" — open
    /// the in-app WYSIWYG note editor on the CURRENT markdown page. Distinct
    /// from OpenInEditor (which opens an external vim/helix on a named file);
    /// this has no target and means "the GUI editor, on what I'm viewing."
    /// Dispatch calls backend.open_note_editor().
    EditCurrentNote,
    /// v0.1.680: "search my notes for X" / "search notes for X" / "find X in
    /// my notes" — full-text content search across note/script bodies (the
    /// "cloak" path). Carries the extracted query. Dispatch populates the
    /// search bar + runs note_content_search.
    SearchNotes(String),
    /// v0.1.694: "redo" / "redo that" — redo in the note editor (only acts
    /// when the editor is open; otherwise inert). Undo reuses Rollback, which
    /// the dispatch routes to the editor when it's open.
    EditorRedo,
    /// v0.1.701: "read this section" / "read it to me" / "speak this" — read
    /// the current section aloud (TTS blended with recorded utterance clips).
    ReadSection,
    Unknown,
}

/// v0.1.376 (discoverability arc): static catalog of voice
/// intent recipes for the `web2qml://recipes` page. Each entry
/// represents one intent KIND (not a constructed instance with
/// data), and provides:
/// - category: how to group it visually
/// - examples: a few representative phrasings users can speak
/// - what_happens: one-line description of the result
///
/// This is the source of truth for the auto-generated recipes
/// page. When a new Intent variant is added, the developer is
/// reminded by the unit test `every_intent_has_a_recipe` (see
/// the test module below) to also add its catalog entry. The
/// test enforces our discoverability gate: features that don't
/// show up automatically don't exist.
///
/// `Intent::Unknown`, `Intent::Freeform`, `Intent::Dictate`, and
/// `Intent::LlmFallback` are catch-alls and not user-facing
/// recipes — they're excluded from the catalog and the test
/// allowlists them.
pub struct VoiceRecipe {
    pub category: &'static str,
    pub kind: &'static str,
    pub examples: &'static [&'static str],
    pub what_happens: &'static str,
}

/// Return the full catalog. Categories used for grouping:
///   "Navigation"   — page navigation, back/forward, reload
///   "Reading"      — read-aloud, reading rate, cadence
///   "Terminal"     — run commands, rollback, history
///   "Workspaces"   — tab/workspace save/load/rename
///   "Search"       — find in page, search web, find anywhere
///   "Audit"        — show audit log, branches, verify
///   "Cards"        — diagram/card canvas
///   "Voice tier 1" — acoustic index controls
///   "Workflows"    — workflow record/play
///   "Settings"     — toggles, open settings, copy/paste meta
///   "Other"        — anything that doesn't fit above
pub fn voice_recipes() -> &'static [VoiceRecipe] {
    &VOICE_RECIPES
}

// Listed in roughly the order users discover them.
// New variants → add an entry here AND the test passes.
static VOICE_RECIPES: &[VoiceRecipe] = &[
    // Navigation
    VoiceRecipe { category: "Navigation", kind: "navigate",
        examples: &["go to news.ycombinator.com", "open hacker news", "navigate to anthropic.com"],
        what_happens: "Load a URL or guess one from a site name." },
    VoiceRecipe { category: "Navigation", kind: "go_back",
        examples: &["go back", "back", "previous page"],
        what_happens: "Go back to the previous page in history." },
    VoiceRecipe { category: "Navigation", kind: "go_forward",
        examples: &["go forward", "forward"],
        what_happens: "Browser forward." },
    VoiceRecipe { category: "Navigation", kind: "reload",
        examples: &["reload", "refresh"],
        what_happens: "Reload the current page." },
    VoiceRecipe { category: "Navigation", kind: "homepage",
        examples: &["home", "homepage", "open homepage"],
        what_happens: "Jump to the web2qml homepage." },
    VoiceRecipe { category: "Navigation", kind: "scroll",
        examples: &["scroll down", "scroll up", "page down", "page up", "go to top", "go to bottom"],
        what_happens: "Move within the current page." },
    VoiceRecipe { category: "Navigation", kind: "click_link",
        examples: &["click sign in", "click about", "tap settings"],
        what_happens: "Click a link by its visible text." },
    VoiceRecipe { category: "Navigation", kind: "scroll_heading",
        examples: &["jump to references", "scroll to results"],
        what_happens: "Find a heading on the page and scroll to it." },
    // v0.1.772: distinct wire kinds ScrollPage/GoTop/GoBottom each need a
    // recipe (the general "scroll" recipe above covers Intent::Scroll only).
    VoiceRecipe { category: "Navigation", kind: "scroll_page",
        examples: &["page down", "page up", "scroll down a page"],
        what_happens: "Scroll the page down or up by a full screen." },
    VoiceRecipe { category: "Navigation", kind: "go_top",
        examples: &["go to top", "jump to the top", "scroll to top"],
        what_happens: "Jump to the very top of the current page." },
    VoiceRecipe { category: "Navigation", kind: "go_bottom",
        examples: &["go to bottom", "jump to the bottom", "scroll to bottom"],
        what_happens: "Jump to the very bottom of the current page." },

    // Search
    VoiceRecipe { category: "Search", kind: "search",
        examples: &["search web for Asahi Linux", "google quantum computing"],
        what_happens: "Send a query to the default search engine." },
    VoiceRecipe { category: "Search", kind: "find",
        examples: &["find quantum", "search page for elliptic"],
        what_happens: "Find text in the current page." },
    VoiceRecipe { category: "Search", kind: "find_anywhere",
        examples: &["find anywhere kessler", "search everywhere"],
        what_happens: "Multi-source search: page text, scrollback, history, archive." },
    VoiceRecipe { category: "Search", kind: "hist_search",
        examples: &["history search docker", "find in history"],
        what_happens: "Reverse-search shell command history." },

    // Reading
    VoiceRecipe { category: "Reading", kind: "reset_cadence",
        examples: &["reset my cadence", "reset cadence for Bob", "clear Alice's cadence"],
        what_happens: "Zero a speaker's accumulated reading-cadence stats. Use after illness, mic change, or recovery." },

    // Terminal
    VoiceRecipe { category: "Terminal", kind: "term_cmd",
        examples: &["run ls", "execute git status", "type cargo build"],
        what_happens: "Send a command to the terminal." },
    VoiceRecipe { category: "Terminal", kind: "rerun_last",
        examples: &["rerun", "rerun last", "do it again"],
        what_happens: "Replay the most recent terminal command." },
    VoiceRecipe { category: "Terminal", kind: "rollback",
        examples: &["undo", "undo that", "rollback last command", "revert"],
        what_happens: "Roll back the most recent mutating command using the captured snapshot." },
    VoiceRecipe { category: "Terminal", kind: "term_ws",
        examples: &["tile all my ssh sessions", "group my ssh sessions", "gather ssh"],
        what_happens: "Gather every tab running an ssh-family client (ssh/mosh/autossh) to the front of the strip." },

    // Audit / branches
    VoiceRecipe { category: "Audit", kind: "show_audit_log",
        examples: &["show audit log", "what did I do", "command history"],
        what_happens: "Open the in-app audit log viewer." },
    VoiceRecipe { category: "Audit", kind: "show_audit_log_for",
        examples: &["show audit log for Alice", "what did Bob do"],
        what_happens: "Filtered audit log for one speaker." },
    VoiceRecipe { category: "Audit", kind: "show_branches",
        examples: &["show branches", "show command tree", "show forks"],
        what_happens: "Open the branch tree viewer showing rollback forks." },
    VoiceRecipe { category: "Audit", kind: "verify_audit",
        examples: &["verify audit log", "check audit log", "verify integrity"],
        what_happens: "Walk the hash chain end-to-end to verify the audit log hasn't been tampered with." },

    // Tabs
    VoiceRecipe { category: "Tabs", kind: "tab_open",
        examples: &["new tab", "open new tab", "open arxiv.org in new tab"],
        what_happens: "Open a new browser tab, optionally with a URL." },
    VoiceRecipe { category: "Tabs", kind: "tab_next",
        examples: &["next tab", "switch tab", "other tab"],
        what_happens: "Cycle to the next tab." },
    VoiceRecipe { category: "Tabs", kind: "tab_prev",
        examples: &["previous tab", "prev tab", "back tab"],
        what_happens: "Cycle to the previous tab." },
    VoiceRecipe { category: "Tabs", kind: "tab_close",
        examples: &["close tab", "close this tab"],
        what_happens: "Close the active tab (never the last)." },
    VoiceRecipe { category: "Tabs", kind: "switch_tab",
        examples: &["switch to terminal", "switch to browser"],
        what_happens: "Jump between the browser tab and the terminal tab." },

    // Workspaces
    VoiceRecipe { category: "Workspaces", kind: "workspace_save_as",
        examples: &["save as research", "save workspace as research"],
        what_happens: "Save current tabs as a named workspace under the active speaker." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_load",
        examples: &["load workspace research", "switch to research workspace"],
        what_happens: "Load a previously-saved workspace." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_list",
        examples: &["list workspaces", "show workspaces", "my workspaces"],
        what_happens: "Show your saved workspaces as a toast." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_delete",
        examples: &["delete workspace research", "forget workspace research"],
        what_happens: "Remove a named workspace. Refuses to delete the default." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_rename",
        examples: &["rename workspace old to new"],
        what_happens: "Rename a workspace." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_inspect",
        examples: &["show workspace research", "describe workspace research"],
        what_happens: "Show the contents of a workspace as a toast." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_copy",
        examples: &["copy workspace research to draft", "duplicate research as draft"],
        what_happens: "Duplicate a workspace under a new name." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_current",
        examples: &["what workspace am I in", "current workspace"],
        what_happens: "Show the active workspace name as a toast." },
    VoiceRecipe { category: "Workspaces", kind: "workspace_save_now",
        examples: &["save workspace", "snapshot workspace"],
        what_happens: "Explicit save of the current workspace state to disk." },

    // Settings & meta
    VoiceRecipe { category: "Settings", kind: "settings",
        examples: &["open settings", "settings", "preferences"],
        what_happens: "Open the settings popup." },
    VoiceRecipe { category: "Settings", kind: "palette",
        examples: &["open palette", "open the palette"],
        what_happens: "Open the command palette drawer." },
    VoiceRecipe { category: "Settings", kind: "quit",
        examples: &["quit", "exit"],
        what_happens: "Quit the web2qml application." },
    VoiceRecipe { category: "Settings", kind: "toggle",
        examples: &["turn off ads", "toggle monospace", "stop compat mode"],
        what_happens: "Toggle a setting (ads, monospace, compat_ua)." },

    // Clipboard / paste
    VoiceRecipe { category: "Clipboard", kind: "copy",
        examples: &["copy url", "copy title", "copy selection", "copy last output", "copy last command"],
        what_happens: "Copy something to clipboard." },
    VoiceRecipe { category: "Clipboard", kind: "paste",
        examples: &["paste into url", "paste to terminal", "paste into find"],
        what_happens: "Paste clipboard into a target field." },

    // Workflows
    VoiceRecipe { category: "Workflows", kind: "workflow_rec_start",
        examples: &["start workflow deploy", "begin recording deploy"],
        what_happens: "Start capturing commands under a named workflow." },
    VoiceRecipe { category: "Workflows", kind: "workflow_rec_stop",
        examples: &["stop recording", "finish workflow"],
        what_happens: "Stop and save the captured workflow." },
    VoiceRecipe { category: "Workflows", kind: "workflow_playback",
        examples: &["run workflow deploy", "replay deploy"],
        what_happens: "Replay a saved workflow." },

    // System / files / email
    VoiceRecipe { category: "System", kind: "sysinfo",
        examples: &["disk usage", "memory usage", "what's my IP"],
        what_happens: "Show system info via a terminal command." },
    VoiceRecipe { category: "System", kind: "email",
        examples: &["email alice@example.com about the demo"],
        what_happens: "Compose an email (requires configured mail client)." },
    VoiceRecipe { category: "System", kind: "file_append",
        examples: &["append to notes meeting at 3pm"],
        what_happens: "Append a line to a file." },

    // Voice tier 1
    VoiceRecipe { category: "Voice tier 1", kind: "tier1_status",
        examples: &["tier 1 status", "show acoustic stats", "exemplar count"],
        what_happens: "Show the acoustic-index status: exemplar count and SHADOW/PROMOTE mode." },
    VoiceRecipe { category: "Voice tier 1", kind: "tier1_histogram",
        examples: &["tier 1 histogram", "threshold suggestion"],
        what_happens: "Show the acoustic-distance histogram + suggested threshold." },
    VoiceRecipe { category: "Voice tier 1", kind: "tier1_promote_on",
        examples: &["promote tier 1", "enable promote"],
        what_happens: "Flip the acoustic index to PROMOTE mode at runtime." },
    VoiceRecipe { category: "Voice tier 1", kind: "tier1_promote_off",
        examples: &["demote tier 1", "back to shadow"],
        what_happens: "Flip the acoustic index back to SHADOW mode." },
    VoiceRecipe { category: "Voice tier 1", kind: "tier1_set_threshold",
        examples: &["set threshold to 0.42", "tier 1 threshold 0.5"],
        what_happens: "Set the PROMOTE same-speaker threshold." },
    VoiceRecipe { category: "Voice tier 1", kind: "tier1_autotune",
        examples: &["auto tune tier 1", "calibrate tier 1"],
        what_happens: "Apply the histogram's suggested threshold + enable PROMOTE in one step." },

    // Cards / diagrams
    VoiceRecipe { category: "Cards", kind: "open_connections_view",
        examples: &["show connections", "show graph", "open graph view"],
        what_happens: "Open the pan/zoomable cards canvas." },
    VoiceRecipe { category: "Cards", kind: "filter_cards",
        examples: &["filter cards auth", "search cards for kessler"],
        what_happens: "Apply a fuzzy filter to the cards canvas." },
    VoiceRecipe { category: "Cards", kind: "clear_cards_filter",
        examples: &["clear filter", "show all cards", "reset filter"],
        what_happens: "Clear the cards filter." },
    VoiceRecipe { category: "Cards", kind: "focus_plane",
        examples: &["focus services", "zoom into auth"],
        what_happens: "Scope the cards canvas to one named plane." },
    VoiceRecipe { category: "Cards", kind: "unfocus_plane",
        examples: &["unfocus", "show everything"],
        what_happens: "Clear plane scope, return to full-diagram view." },
    VoiceRecipe { category: "Cards", kind: "read_diagram_warnings",
        examples: &["what's wrong", "read warnings", "what did the parser say"],
        what_happens: "Read aloud the current diagram's parser warnings." },

    // Help & discovery (v0.1.377)
    VoiceRecipe { category: "Settings", kind: "show_recipes",
        examples: &["show recipes", "what can you do", "what can web2qml do", "what can I say", "show capabilities", "open recipes"],
        what_happens: "Open the recipes page listing every voice intent, URL shortcut, and hand-editable config file." },

    // Session arc (v0.1.382)
    VoiceRecipe { category: "Audit", kind: "summarize_session",
        examples: &["summarize my session", "what did I do today", "what have I been doing", "session summary"],
        what_happens: "Open the recent-sessions page focused on the most-recent session, with an AI-generated 1-2 sentence summary of what got done." },

    // Discoverability arc (v0.1.385)
    VoiceRecipe { category: "Settings", kind: "context_help",
        examples: &["what can I do here", "what's available", "what now", "show shortcuts for this page"],
        what_happens: "Show a focused list of voice intents and shortcuts that apply to the current page — different from `what can you do` which is the full catalog." },

    // Bookmarks (v0.1.386)
    VoiceRecipe { category: "Navigation", kind: "save_url",
        examples: &["save this", "save this page", "bookmark this", "remember this page", "save url <X>"],
        what_happens: "Save the current page (or an explicit URL) to bookmarks.toml in the config dir. View saved URLs via 'show bookmarks' or web2qml://bookmarks." },
    VoiceRecipe { category: "Navigation", kind: "show_bookmarks",
        examples: &["show bookmarks", "my bookmarks", "open bookmarks", "list bookmarks"],
        what_happens: "Open the saved-URLs page (web2qml://bookmarks) listing pages you've bookmarked, most-recent first." },
    VoiceRecipe { category: "Navigation", kind: "show_bookmarks_tagged",
        examples: &["show news bookmarks", "show bookmarks tagged news", "filter bookmarks by tech", "news bookmarks"],
        what_happens: "Open the bookmarks page filtered to a specific tag. Routes to web2qml://bookmarks?tag=<X>." },
    VoiceRecipe { category: "Tabs", kind: "switch_to_tab_matching",
        examples: &["switch to the github tab", "go to the apple tab", "switch to tab with stackoverflow", "go to news tab"],
        what_happens: "Jump to whichever open browser tab's URL contains the given substring (case-insensitive, first match wins). Beats counting tabs when several are open." },
    VoiceRecipe { category: "Workspaces", kind: "save_workspace_as",
        examples: &["save workspace as research", "save my workspace as project-x", "save this workspace as monday-stack"],
        what_happens: "Save the current browser-tab set + terminal sessions as a named workspace under the active speaker. Persists to ~/.config/web2qml/workspaces/<speaker>/<name>.toml." },
    VoiceRecipe { category: "Workspaces", kind: "load_workspace",
        examples: &["load workspace research", "switch to workspace project-x", "open workspace monday-stack", "load the research workspace"],
        what_happens: "Swap to a previously-saved named workspace. The current tabs are auto-saved as the active speaker's default workspace before loading. Requires an active speaker." },
    VoiceRecipe { category: "Navigation", kind: "open_file_dialog",
        examples: &["open file", "pick a file", "browse files", "open file picker"],
        what_happens: "Show the platform file picker. Pick a file and web2qml renders it if the extension is supported (html, md, gmi, txt) or hands off to xdg-open for everything else. Drag-and-drop into the window does the same." },
];

impl Intent {
    /// Human-readable description for UI confirmation
    pub fn describe(&self) -> String {
        match self {
            Intent::Navigate(url) => format!("Navigate to {}", url),
            Intent::SearchWeb(q) => format!("Search web for \"{}\"", q),
            Intent::GoBack => "Go back".into(),
            Intent::GoForward => "Go forward".into(),
            Intent::Reload => "Reload page".into(),
            Intent::Scroll(n) => if *n < 0 { "Scroll up".into() } else { "Scroll down".into() },
            Intent::ScrollPage(n) => if *n < 0 { "Page up".into() } else { "Page down".into() },
            Intent::GoTop => "Jump to top".into(),
            Intent::GoBottom => "Jump to bottom".into(),
            Intent::FindInPage(q) => format!("Find \"{}\" on page", q),
            Intent::OpenPalette(s) => format!("Open {} panel", s),
            Intent::SwitchTab(s) => format!("Switch to {} tab", s),
            Intent::TerminalCommand(c) => format!("Run in terminal: {}", c),
            Intent::OpenSettings => "Open settings".into(),
            Intent::OpenHomepage => "Open homepage".into(),
            Intent::Quit => "Quit".into(),
            Intent::SystemInfo(_, desc) => desc.clone(),
            Intent::EmailCompose(to, _) => format!("Compose email to {}", to),
            Intent::FileAppend(path, text) => format!("Append \"{}\" to {}", text.chars().take(40).collect::<String>(), path),
            Intent::LlmFallback(t) => format!("Ask local LLM: {}", t.chars().take(60).collect::<String>()),
            Intent::ClickLink(q) => format!("Click link matching \"{}\"", q),
            Intent::ClickPositional { ordinal, kind } => {
                if *ordinal == -1 {
                    format!("Click the last {}", kind)
                } else {
                    format!("Click the #{} {}", ordinal, kind)
                }
            }
            Intent::ScrollToHeading(h) => format!("Scroll to heading \"{}\"", h),
            Intent::Copy(what) => format!("Copy {} to clipboard", what),
            Intent::Paste(target) => format!("Paste clipboard into {}", target),
            Intent::RerunLast => "Rerun last terminal command".into(),
            Intent::HistorySearch(q) => format!("Search command history for \"{}\"", q),
            Intent::Toggle(target) => format!("Toggle {}", target),
            Intent::Dictate(t) => format!("Type: {}", t.chars().take(60).collect::<String>()),
            Intent::FindAnywhere(q) => format!("Find \"{}\" across page, terminal, history, archive", q),
            Intent::WorkflowStartRecording(n) => format!("Start recording workflow: {}", n),
            Intent::WorkflowStopRecording => "Stop recording workflow".into(),
            Intent::WorkflowPlayback(n) => format!("Replay workflow: {}", n),
            Intent::Freeform(t) => format!("Search or navigate: {}", t),
            Intent::Rollback => "Undo last command".into(),
            Intent::ShowAuditLog => "Show audit log".into(),
            Intent::ShowAuditLogForSpeaker(n) => format!("Show audit log for {}", n),
            Intent::ShowBranches => "Show command branches".into(),
            Intent::VerifyAuditLog => "Verify audit log integrity".into(),
            Intent::TabOpen(url) => {
                if url.is_empty() { "Open new tab".into() }
                else { format!("Open new tab: {}", url) }
            }
            Intent::TabNext => "Next tab".into(),
            Intent::TermPane(p) => format!("Terminal: {}", p),
            Intent::ShowBuildErrors => "Show build errors".to_string(),
            Intent::TabPrev => "Previous tab".into(),
            Intent::TabClose => "Close tab".into(),
            Intent::Tier1Status => "Show acoustic index status".into(),
            Intent::Tier1Histogram => "Show acoustic distance histogram".into(),
            Intent::Tier1PromoteOn => "Enable PROMOTE mode".into(),
            Intent::Tier1PromoteOff => "Disable PROMOTE mode".into(),
            Intent::Tier1SetThreshold(t) => format!("Set Tier 1 threshold to {:.3}", t),
            Intent::Tier1AutoTune => "Auto-tune Tier 1".into(),
            Intent::WorkspaceSaveAs(n) => format!("Save workspace as {}", n),
            Intent::WorkspaceLoad(n) => format!("Load workspace {}", n),
            Intent::WorkspaceList => "List workspaces".into(),
            Intent::WorkspaceDelete(n) => format!("Delete workspace {}", n),
            Intent::WorkspaceRename(s) => {
                // Param is "old\x1Fnew"; show both for readability.
                if let Some((o, n)) = s.split_once('\x1F') {
                    format!("Rename workspace {} → {}", o, n)
                } else {
                    format!("Rename workspace ({})", s)
                }
            },
            Intent::WorkspaceInspect(n) => format!("Show workspace {}", n),
            Intent::WorkspaceCopy(s) => {
                if let Some((src, dst)) = s.split_once('\x1F') {
                    format!("Copy workspace {} → {}", src, dst)
                } else {
                    format!("Copy workspace ({})", s)
                }
            },
            Intent::WorkspaceCurrent => "Show current workspace".into(),
            Intent::WorkspaceSaveNow => "Save current workspace".into(),
            Intent::OpenConnectionsView => "Open connections graph view".into(),
            Intent::WalkLastSession => "Walk me through my last session".into(),
            Intent::RecallCommands(q) => if q.is_empty() {
                "Recall recent commands".into()
            } else {
                format!("Recall commands about \"{}\"", q)
            },
            Intent::OpenInEditor(p) => format!("Open in editor: {}", p),
            Intent::EditCurrentNote => "Edit this note (in-app editor)".into(),
            Intent::SearchNotes(q) => format!("Search notes for \"{}\"", q),
            Intent::EditorRedo => "Redo (note editor)".into(),
            Intent::ReadSection => "Read this section aloud".into(),
            Intent::FilterCards(q) => format!("Filter cards: {}", q),
            Intent::ClearCardsFilter => "Clear cards filter".into(),
            Intent::TagCard(t) => format!("Tag card: {}", t),
            Intent::UntagCard(t) => format!("Untag card: {}", t),
            Intent::FilterByTag(t) => format!("Filter to tag: {}", t),
            Intent::ShowMermaidDemo => "Show Mermaid demo diagram".into(),
            Intent::TiltGraph(angle) => format!("Tilt graph to {:.0}°", angle),
            Intent::RotateGraph(p) => format!("Rotate diagram: {}", p),
            Intent::ExpandNode => "Expand (reveal more nodes)".into(),
            Intent::CollapseNode => "Collapse (fewer nodes)".into(),
            Intent::FlattenGraph => "Flatten graph to 2D".into(),
            Intent::CloseConnectionsView => "Close graph view".into(),
            Intent::ResetCardsZoom => "Fit graph to view".into(),
            Intent::FocusPlane(name) => format!("Focus plane: {}", name),
            Intent::UnfocusPlane => "Show entire diagram".into(),
            Intent::ReadDiagramWarnings => "Read diagram warnings".into(),
            Intent::ResetCadence(name) => {
                if name.is_empty() { "Reset cadence stats".into() }
                else { format!("Reset cadence stats for {}", name) }
            }
            Intent::ShowRecipes => "Open recipes page".into(),
            Intent::SaveUrl(u) => if u.is_empty() {
                "Save current page as bookmark".into()
            } else {
                format!("Save {} as bookmark", u)
            },
            Intent::ShowBookmarks => "Show saved bookmarks".into(),
            Intent::SwitchToTabMatching(q) => format!("Switch to tab matching '{}'", q),
            Intent::SaveWorkspaceAs(n) => format!("Save workspace as '{}'", n),
            Intent::LoadWorkspace(n) => format!("Load workspace '{}'", n),
            Intent::OpenFileDialog => "Open file picker".into(),
            Intent::ShowBookmarksTagged(t) => format!("Show bookmarks tagged {}", t),
            Intent::SummarizeSession => "Summarize most recent session".into(),
            Intent::ContextHelp => "Show what's available on this page".into(),
            Intent::Unknown => "Unknown command".into(),
        }
    }

    /// Format for QML — TYPE\x1Fdescription\x1Fparam(\x1Fparam2)
    pub fn to_qml_string(&self) -> String {
        let (kind, p1, p2) = match self {
            Intent::Navigate(u) => ("navigate", u.clone(), String::new()),
            Intent::SearchWeb(q) => ("search", q.clone(), String::new()),
            Intent::GoBack => ("go_back", String::new(), String::new()),
            Intent::GoForward => ("go_forward", String::new(), String::new()),
            Intent::Reload => ("reload", String::new(), String::new()),
            Intent::Scroll(n) => ("scroll", n.to_string(), String::new()),
            Intent::ScrollPage(n) => ("scroll_page", n.to_string(), String::new()),
            Intent::GoTop => ("go_top", String::new(), String::new()),
            Intent::GoBottom => ("go_bottom", String::new(), String::new()),
            Intent::FindInPage(q) => ("find", q.clone(), String::new()),
            Intent::OpenPalette(s) => ("palette", s.clone(), String::new()),
            Intent::SwitchTab(s) => ("switch_tab", s.clone(), String::new()),
            Intent::TerminalCommand(c) => ("term_cmd", c.clone(), String::new()),
            Intent::OpenSettings => ("settings", String::new(), String::new()),
            Intent::OpenHomepage => ("homepage", String::new(), String::new()),
            Intent::Quit => ("quit", String::new(), String::new()),
            Intent::SystemInfo(k, _) => ("sysinfo", k.clone(), String::new()),
            Intent::EmailCompose(to, body) => ("email", to.clone(), body.clone()),
            Intent::FileAppend(path, text) => ("file_append", path.clone(), text.clone()),
            Intent::LlmFallback(t) => ("llm", t.clone(), String::new()),
            Intent::ClickLink(q) => ("click_link", q.clone(), String::new()),
            Intent::ClickPositional { ordinal, kind } =>
                ("click_positional", kind.clone(), ordinal.to_string()),
            Intent::ScrollToHeading(h) => ("scroll_heading", h.clone(), String::new()),
            Intent::Copy(what) => ("copy", what.clone(), String::new()),
            Intent::Paste(target) => ("paste", target.clone(), String::new()),
            Intent::RerunLast => ("rerun_last", String::new(), String::new()),
            Intent::HistorySearch(q) => ("hist_search", q.clone(), String::new()),
            Intent::Toggle(target) => ("toggle", target.clone(), String::new()),
            Intent::Dictate(t) => ("dictate", t.clone(), String::new()),
            Intent::FindAnywhere(q) => ("find_anywhere", q.clone(), String::new()),
            Intent::WorkflowStartRecording(n) => ("workflow_rec_start", n.clone(), String::new()),
            Intent::WorkflowStopRecording => ("workflow_rec_stop", String::new(), String::new()),
            Intent::WorkflowPlayback(n) => ("workflow_playback", n.clone(), String::new()),
            Intent::Freeform(t) => ("freeform", t.clone(), String::new()),
            Intent::Rollback => ("rollback", String::new(), String::new()),
            Intent::ShowAuditLog => ("show_audit_log", String::new(), String::new()),
            Intent::ShowAuditLogForSpeaker(n) => ("show_audit_log_for", n.clone(), String::new()),
            Intent::ShowBranches => ("show_branches", String::new(), String::new()),
            Intent::VerifyAuditLog => ("verify_audit", String::new(), String::new()),
            Intent::TabOpen(url) => ("tab_open", url.clone(), String::new()),
            Intent::TabNext => ("tab_next", String::new(), String::new()),
            Intent::TermPane(phrase) => ("term_ws", phrase.clone(), String::new()),
            Intent::ShowBuildErrors => ("build_errors", String::new(), String::new()),
            Intent::TabPrev => ("tab_prev", String::new(), String::new()),
            Intent::TabClose => ("tab_close", String::new(), String::new()),
            Intent::Tier1Status => ("tier1_status", String::new(), String::new()),
            Intent::Tier1Histogram => ("tier1_histogram", String::new(), String::new()),
            Intent::Tier1PromoteOn => ("tier1_promote_on", String::new(), String::new()),
            Intent::Tier1PromoteOff => ("tier1_promote_off", String::new(), String::new()),
            Intent::Tier1SetThreshold(t) => ("tier1_set_threshold", format!("{:.3}", t), String::new()),
            Intent::Tier1AutoTune => ("tier1_autotune", String::new(), String::new()),
            Intent::WorkspaceSaveAs(n) => ("workspace_save_as", n.clone(), String::new()),
            Intent::WorkspaceLoad(n) => ("workspace_load", n.clone(), String::new()),
            Intent::WorkspaceList => ("workspace_list", String::new(), String::new()),
            Intent::WorkspaceDelete(n) => ("workspace_delete", n.clone(), String::new()),
            Intent::WorkspaceRename(s) => {
                // Param: "old\x1Fnew" → split into p1=old, p2=new.
                if let Some((o, n)) = s.split_once('\x1F') {
                    ("workspace_rename", o.to_string(), n.to_string())
                } else {
                    ("workspace_rename", s.clone(), String::new())
                }
            },
            Intent::WorkspaceInspect(n) => ("workspace_inspect", n.clone(), String::new()),
            Intent::WorkspaceCopy(s) => {
                if let Some((src, dst)) = s.split_once('\x1F') {
                    ("workspace_copy", src.to_string(), dst.to_string())
                } else {
                    ("workspace_copy", s.clone(), String::new())
                }
            },
            Intent::WorkspaceCurrent => ("workspace_current", String::new(), String::new()),
            Intent::WorkspaceSaveNow => ("workspace_save_now", String::new(), String::new()),
            Intent::OpenConnectionsView => ("open_connections_view", String::new(), String::new()),
            Intent::WalkLastSession => ("walk_last_session", String::new(), String::new()),
            Intent::RecallCommands(q) => ("recall_commands", q.clone(), String::new()),
            Intent::OpenInEditor(p) => ("open_in_editor", p.clone(), String::new()),
            Intent::EditCurrentNote => ("edit_current_note", String::new(), String::new()),
            Intent::SearchNotes(q) => ("search_notes", q.clone(), String::new()),
            Intent::EditorRedo => ("editor_redo", String::new(), String::new()),
            Intent::ReadSection => ("read_section", String::new(), String::new()),
            Intent::FilterCards(q) => ("filter_cards", q.clone(), String::new()),
            Intent::ClearCardsFilter => ("clear_cards_filter", String::new(), String::new()),
            Intent::TagCard(t) => ("tag_card", t.clone(), String::new()),
            Intent::UntagCard(t) => ("untag_card", t.clone(), String::new()),
            Intent::FilterByTag(t) => ("filter_by_tag", t.clone(), String::new()),
            Intent::ShowMermaidDemo => ("show_mermaid_demo", String::new(), String::new()),
            Intent::TiltGraph(angle) => ("tilt_graph", format!("{}", angle), String::new()),
            Intent::RotateGraph(p) => ("rotate_graph", p.clone(), String::new()),
            Intent::ExpandNode => ("expand_node", String::new(), String::new()),
            Intent::CollapseNode => ("collapse_node", String::new(), String::new()),
            Intent::FlattenGraph => ("flatten_graph", String::new(), String::new()),
            Intent::CloseConnectionsView => ("close_connections_view", String::new(), String::new()),
            Intent::ResetCardsZoom => ("reset_cards_zoom", String::new(), String::new()),
            Intent::FocusPlane(name) => ("focus_plane", name.clone(), String::new()),
            Intent::UnfocusPlane => ("unfocus_plane", String::new(), String::new()),
            Intent::ReadDiagramWarnings => ("read_diagram_warnings", String::new(), String::new()),
            Intent::ResetCadence(name) => ("reset_cadence", name.clone(), String::new()),
            Intent::ShowRecipes => ("show_recipes", String::new(), String::new()),
            Intent::SaveUrl(u) => ("save_url", u.clone(), String::new()),
            Intent::ShowBookmarks => ("show_bookmarks", String::new(), String::new()),
            Intent::SwitchToTabMatching(q) => ("switch_to_tab_matching", q.clone(), String::new()),
            Intent::SaveWorkspaceAs(n) => ("save_workspace_as", n.clone(), String::new()),
            Intent::LoadWorkspace(n) => ("load_workspace", n.clone(), String::new()),
            Intent::OpenFileDialog => ("open_file_dialog", String::new(), String::new()),
            Intent::ShowBookmarksTagged(t) => ("show_bookmarks_tagged", t.clone(), String::new()),
            Intent::SummarizeSession => ("summarize_session", String::new(), String::new()),
            Intent::ContextHelp => ("context_help", String::new(), String::new()),
            Intent::Unknown => ("unknown", String::new(), String::new()),
        };
        format!("{}\x1F{}\x1F{}\x1F{}", kind, self.describe(), p1, p2)
    }

    /// v0.1.213: is this match strong enough to learn from?
    ///
    /// The parser has a tiered structure: specific verb-prefix matches
    /// (Navigate, GoBack, Reload, etc.) → loose-pattern matches
    /// (SearchWeb on key trigger words) → catch-all fallbacks
    /// (Dictate, Freeform, LlmFallback).
    ///
    /// For passive learning from ambient audio (bg-drain), we only
    /// want to learn from the FIRST tier — utterances where the
    /// parser was confident enough to map to a specific verb. The
    /// fallback tiers accept essentially any English string, so
    /// they'd match overheard TV / casual conversation / phone
    /// calls and pollute the fingerprint DB with non-command audio.
    ///
    /// User log from v0.1.205 showed "where you are, before you
    /// go on screen" (ambient speech, not addressed to the system)
    /// being matched as a known intent. That phrase likely hit one
    /// of the catch-alls. After this check is wired in, it'd be
    /// rejected.
    ///
    /// Rejected variants (low-confidence catch-alls):
    ///   - Dictate: literally "type whatever was said"
    ///   - Freeform: "we don't know, throw to user"
    ///   - LlmFallback: "ask the LLM to figure it out"
    ///   - Unknown: by definition unrecognized
    ///   - SearchWeb: triggered by mere presence of certain words
    ///     ("search", "find", "what is") — too generous for passive
    ///     learning, even though it works fine for explicit voice
    ///     commands. Note: `FindInPage` is also a "find" variant
    ///     but requires more specific "find on page" phrasing, so
    ///     we accept it.
    ///
    /// Accepted: every other variant. They all require a specific
    /// verb prefix or noun trigger that wouldn't occur in random
    /// ambient speech.
    pub fn is_high_confidence(&self) -> bool {
        !matches!(self,
            Intent::Dictate(_)
            | Intent::Freeform(_)
            | Intent::LlmFallback(_)
            | Intent::Unknown
            | Intent::SearchWeb(_)
        )
    }
}

/// Normalize natural-language phrasings into bare-command form.
///
/// v0.1.110: handles common conversational scaffolding that real
/// users include but our exact-match patterns miss:
///
/// - Politeness prefixes: "could you (please)? <X>" → "<X>"
/// - First-person desires: "I (want|need|wanna) (to)? <X>" → "<X>"
/// - Conditional questions: "would it be possible to <X>" → "<X>"
/// - Trailing softeners: "<X> for me" → "<X>", "<X> real quick" → "<X>"
/// - Modal-only: "let's <X>" → "<X>", "let me <X>" → "<X>"
/// - Question prefix: "can you <X>" → "<X>", "will you <X>" → "<X>"
///
/// All transformations are idempotent and conservative: when in doubt
/// we leave the text alone. Wrong stripping is worse than no stripping
/// because the parser then operates on garbled command text.
///
/// Multilingual note: this function is currently English-only.
/// For other languages, parallel implementations would live in
/// `voice_intent_<lang>.rs` modules and be dispatched via the
/// configured language code (see voice_lang::current()).
/// v0.1.658: does a "run X" / "type X" tail look like an actual command vs
/// thinking-aloud prose? Real commands are short and lack narration markers.
/// Used to stop narration ("basically run this on the host, effectively")
/// from routing to a literal TerminalCommand after the hedge-stripper turns
/// it into "run this on the host, effectively". Tuned against the Asahi
/// corpus false-positives; budget-checked by scripts/sim-corpus.sh.
/// v0.1.726 (corpus fuzz): decide whether the tail of an "open <rest>" phrase
/// is a real search query vs a narration run-on. "open google" / "open rust
/// async docs" → query; "open this up in ghidorah real quick and see what we
/// have" → NOT a query (a kernel dev thinking aloud). Rejects tails that begin
/// with a narration pronoun, run on too long, or contain a mid-clause comma.
fn open_tail_is_query(rest: &str) -> bool {
    let rest = rest.trim();
    if rest.is_empty() { return false; }
    let words: Vec<&str> = rest.split_whitespace().collect();
    // a query is short — a 9-word "open" tail is someone narrating.
    if words.len() > 6 { return false; }
    // narration usually opens with a pronoun/filler ("this up …", "it again").
    const LEAD_NARRATION: &[&str] = &[
        "this", "that", "these", "those", "it", "them", "things", "stuff",
        "everything", "something", "anything",
    ];
    let first = words[0].trim_end_matches([',', '.']);
    if LEAD_NARRATION.contains(&first) { return false; }
    // a comma-joined clause ("open X, then …") is prose, not a query.
    if rest.contains(", ") { return false; }
    // run-on tells ("real quick", "and see", "let me") betray narration.
    const RUNON: &[&str] = &["real quick", "and see", "and then", "let me",
        "what we have", "for a second", "just to"];
    let lower = rest.to_lowercase();
    if RUNON.iter().any(|m| lower.contains(m)) { return false; }
    true
}

fn looks_like_command(s: &str) -> bool {
    let s = s.trim();
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() || words.len() > 5 { return false; } // commands are short
    // a bare number ("type 2") is narration, not a command
    if words.len() == 1 && words[0].chars().all(|c| c.is_ascii_digit()) { return false; }
    // prose/narration markers that a real shell command wouldn't contain
    const PROSE: &[&str] = &[
        "this", "that", "those", "these", "it", "is", "are", "was", "were",
        "the", "a", "an", "on", "with", "for", "of", "more", "like", "just",
        "effectively", "basically", "probably", "actually", "really", "host",
        "thing", "point", "bit", "you", "oh", "why", "what", "here", "there",
        "comment", "extra",
    ];
    // first word should be command-ish (not a prose word)
    if PROSE.contains(&words[0]) { return false; }
    // if MORE THAN ONE token is a prose marker, it's narration, not a command
    let prose_hits = words.iter().filter(|w| PROSE.contains(&w.trim_end_matches(','))).count();
    if prose_hits >= 1 { return false; }
    // a trailing comma-laden clause is prose
    if s.contains(", ") { return false; }
    true
}

fn normalize_natural_language(text: &str) -> String {
    let mut t = text.to_string();

    // Strip leading politeness/wish prefixes. Order matters: longer
    // forms first so we don't half-match. We loop because users can
    // stack them ("could you please just go to ...").
    //
    // Each entry is the prefix to strip (lowercase, with trailing
    // space). After stripping we re-trim and try again.
    let prefixes: &[&str] = &[
        // Politeness questions
        "could you please ",
        "would you please ",
        "can you please ",
        "will you please ",
        "could you kindly ",
        "would you kindly ",
        "could you ",
        "would you ",
        "can you ",
        "will you ",
        // First-person wishes / wants
        "i would like to ",
        "i'd like to ",
        "id like to ",
        "i would like you to ",
        "i'd like you to ",
        "i want you to ",
        "i need you to ",
        "i wanna ",
        "i want to ",
        "i need to ",
        "i'd love to ",
        "i would love to ",
        "i'm gonna ",
        "i am going to ",
        "i'm going to ",
        // Conditional questions
        "would it be possible to ",
        "is it possible to ",
        "do you mind ",
        "do you think you could ",
        // Permissive / hortative
        "let me ",
        "let's ",
        "lets ",
        // Modal-only
        "please ",
        "kindly ",
        // v0.1.280: discourse markers / acknowledgments. Whisper
        // often transcribes "okay, scroll down" as "okay scroll
        // down" (comma already stripped). These prefix the
        // actual command and shouldn't block parsing.
        "okay ",
        "ok ",
        "alright ",
        "all right ",
        "right ",
        "yeah ",
        "yep ",
        "yes ",
        "uh ",
        "um ",
        "so ",
        "now ",
        // v0.1.280: command verbs that REDUCE to nothing
        // — e.g. "please go scroll down" parses cleanly when
        // "go" is treated as a discourse marker before the
        // actual verb. Only strip when followed by another
        // verb word.
        "just ",
        "now please ",
        // v0.1.280: "if you would" / "if you can" already
        // handled at the suffix end; here for "if you can ".
        "if you can ",
        "if you could ",
    ];

    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 5 {
        changed = false;
        iterations += 1;
        for &p in prefixes {
            if t.starts_with(p) {
                t = t[p.len()..].trim_start().to_string();
                changed = true;
                break;
            }
        }
    }

    // Strip trailing softeners. Same loop logic.
    let suffixes: &[&str] = &[
        " for me",
        " for me please",
        " please",
        " thanks",
        " thank you",
        " real quick",
        " real fast",
        " right now",
        " right away",
        " if you don't mind",
        " if that's okay",
        " if you can",
        " quickly",
    ];
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 5 {
        changed = false;
        iterations += 1;
        for &s in suffixes {
            if t.ends_with(s) {
                let cut = t.len() - s.len();
                t = t[..cut].trim_end().to_string();
                changed = true;
                break;
            }
        }
    }

    // v0.1.708: conservative mid-phrase reductions for natural phrasings that
    // testing against real input showed slipping through ("open up the editor"
    // → search, "take me to the very top" → search, "read me this section" →
    // freeform). Word-boundary-safe substring swaps, applied once. Kept tight
    // to avoid changing intent for unrelated phrases.
    let reductions: &[(&str, &str)] = &[
        ("open up ", "open "),     // "open up the editor"
        (" the very ", " the "),   // "to the very top/bottom"
        ("read me ", "read "),     // "read me this section"
        ("show me ", "show "),     // "show me the recipes"
        ("pull up ", "open "),     // "pull up the editor"
        ("bring up ", "open "),    // "bring up settings"
        ("take me to ", "go to "), // "take me to the top" → "go to the top"
    ];
    for (from, to) in reductions {
        if t.contains(from) {
            t = t.replace(from, to);
        }
    }

    t.trim().to_string()
}

/// v0.1.149: detect and strip a floor-claim prefix.
///
/// Format: `<name>[,:] here[,] <rest...>`
///   - name is the registered speaker (case-insensitive lookup)
///   - punctuation after name (comma/colon) is optional
///   - punctuation after "here" (comma) is optional
///   - whitespace between tokens is tolerant
///
/// If a floor-claim matches AND the name is a known speaker, we:
///   1. Mark that speaker as active via speakers::note_active.
///   2. Return the remainder of the text (the command).
///
/// If no match, or the name isn't registered: return the text unchanged.
///
/// Examples (input → output, where Kessler is registered):
///   "kessler, here, run dust"  → "run dust"     (active=Kessler)
///   "kessler here run dust"    → "run dust"     (active=Kessler)
///   "kessler: here run dust"   → "run dust"     (active=Kessler)
///   "alice, here, what time"   → "what time"    (active=Alice if registered)
///   "kessler here"             → ""             (active=Kessler, no command)
///   "find here in this page"   → unchanged       (no name+here pattern)
///   "bob, here, list files"    → unchanged       (Bob not registered)
fn strip_floor_claim_prefix(text: &str) -> String {
    // The text is already lowercased by parse() before this is called.
    // Speakers are stored case-preserved; we lowercase the names for
    // comparison.
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 2 { return text.to_string(); }

    // First token: strip trailing punctuation we accept after the name.
    let name_token = words[0].trim_end_matches(&[',', ':', ';'][..]);
    if name_token.is_empty() { return text.to_string(); }

    // Second token: must be "here" or "here," (the "here" with trailing
    // comma is common in whisper transcription of "<name>, here, ...").
    let here_token = words[1].trim_end_matches(&[',', ':'][..]);
    if here_token != "here" { return text.to_string(); }

    // Look up the name in the speaker registry. Case-insensitive match.
    let registered = crate::speakers::list();
    let matched_name = registered.iter()
        .find(|(n, _)| n.to_lowercase() == name_token)
        .map(|(n, _)| n.clone());

    let Some(speaker_name) = matched_name else {
        // Unknown name — leave the prefix in place. The regular parser
        // will treat it as a normal utterance.
        return text.to_string();
    };

    // Mark this speaker as active. The bg-thread parser invokes us
    // before downstream identification, so this overrides whatever
    // the acoustic matcher would have picked.
    crate::speakers::note_active(&speaker_name);
    crate::clog!("floor-claim", "explicit speaker={} stripped \"{} {}\"",
              speaker_name, words[0], words[1]);

    // Return the rest of the text. The remainder may be empty (just
    // "<name>, here." — claim with no command).
    if words.len() == 2 {
        String::new()
    } else {
        words[2..].join(" ")
    }
}

/// v0.1.390: reject names that would confuse the workspace
/// save/load parsers — most often artifacts of whisper
/// truncation. "this" / "that" / "current" / "the" /
/// "my" leak in when whisper dropped the actual workspace
/// name. Treat as unknown rather than save a workspace
/// literally called "this".
fn is_reserved_workspace_name(name: &str) -> bool {
    // v0.1.772: strip a leading article so a looser arm that captured e.g.
    // "the current" (from "switch to the current workspace") is still
    // recognized as the reserved name "current".
    let n = name.trim();
    let n = n.strip_prefix("the ")
        .or_else(|| n.strip_prefix("a "))
        .or_else(|| n.strip_prefix("an "))
        .unwrap_or(n);
    matches!(n,
        "this" | "that" | "current" | "the" | "my"
        | "active" | "default workspace"
    )
}

/// v0.1.596: parse a positional click target ("click the third link",
/// "tap the last button"). Returns (ordinal, kind) where ordinal is
/// 1-based and -1 means "last"; kind is normalized. None if the phrase
/// isn't a positional click — so the caller falls through to the
/// existing text-match ClickLink without regression. Pure + tested.
fn parse_positional_click(text: &str) -> Option<(i32, String)> {
    let t = text.trim().to_lowercase();
    let body = ["click on ", "click the ", "click ", "tap on ", "tap the ",
                "tap ", "press the ", "press ", "select the ", "select "]
        .iter()
        .find_map(|p| t.strip_prefix(p))
        .unwrap_or(t.as_str())
        .trim();
    let body = body.strip_prefix("the ").unwrap_or(body);
    let words: Vec<&str> = body.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let ordinal = word_to_ordinal(words[0])?;
    // kind = the last word (allows an adjective between ordinal and noun,
    // e.g. "third blue button" → button).
    let kind = normalize_click_kind(words.last().copied()?)?;
    Some((ordinal, kind))
}

fn word_to_ordinal(w: &str) -> Option<i32> {
    Some(match w {
        "first" | "1st" => 1,
        "second" | "2nd" => 2,
        "third" | "3rd" => 3,
        "fourth" | "4th" => 4,
        "fifth" | "5th" => 5,
        "sixth" | "6th" => 6,
        "seventh" | "7th" => 7,
        "eighth" | "8th" => 8,
        "ninth" | "9th" => 9,
        "tenth" | "10th" => 10,
        "last" => -1,
        _ => {
            let digits = w.trim_end_matches(|c: char| !c.is_ascii_digit());
            match digits.parse::<i32>() {
                Ok(n) if n >= 1 => n,
                _ => return None,
            }
        }
    })
}

fn normalize_click_kind(w: &str) -> Option<String> {
    let w = w.trim_end_matches('s'); // links → link
    Some(match w {
        "link" => "link",
        "button" | "btn" => "button",
        "image" | "img" | "picture" | "photo" => "image",
        "field" | "input" | "box" => "field",
        "item" | "result" | "row" | "entry" => "item",
        _ => return None,
    }
    .to_string())
}

/// Parse transcribed text into an Intent
/// v0.1.767: recover the original-case spelling of a name that was extracted
/// from the lowercased parse buffer. Speaker names ("Bob", "Alice") should keep
/// their capitalization for display/lookup, but `parse` lowercases its working
/// text upfront. We find the lowercased name back inside the original utterance
/// (case-insensitively) and return that original-case slice; if the upstream
/// filler/confusion transforms altered it so it's no longer present verbatim,
/// we fall back to the lowercased form.
fn recover_name_case(original: &str, lower_name: &str) -> String {
    if lower_name.is_empty() { return String::new(); }
    let lo = original.to_lowercase();
    if let Some(pos) = lo.find(lower_name) {
        let end = pos + lower_name.len();
        if original.is_char_boundary(pos) && original.is_char_boundary(end) {
            let slice = &original[pos..end];
            if slice.to_lowercase() == lower_name {
                return slice.to_string();
            }
        }
    }
    lower_name.to_string()
}

pub fn parse(text: &str) -> Intent {
    let t = text.trim().to_lowercase();
    if t.is_empty() { return Intent::Unknown; }

    // Apply learned filler stripping (universal seeds + per-user learned fillers)
    let t = crate::token_stats::strip_fillers(&t);
    if t.is_empty() { return Intent::Unknown; }

    // Apply learned confusion-table corrections (whisper-mistranscription fixes)
    let t = crate::confusion_table::apply_corrections(&t);

    // Strip common manual prefixes (legacy — kept for paranoia, mostly redundant now)
    let t = t.trim_start_matches("hey ").trim_start_matches("ok ");
    // v0.1.280: strip trailing punctuation more aggressively.
    // Was a one-pass trim_end_matches that missed multi-char
    // sequences like "scroll down..." or "scroll down?!".
    // Also missed commas which Whisper sometimes leaves on
    // partial utterances. The loop handles any combination.
    let mut t_owned = t.to_string();
    loop {
        let trimmed = t_owned.trim_end_matches(|c: char| {
            c == '.' || c == '!' || c == '?' || c == ',' || c == ';' || c.is_whitespace()
        });
        if trimmed.len() == t_owned.len() { break; }
        t_owned = trimmed.to_string();
    }
    let t = t_owned;
    if t.is_empty() { return Intent::Unknown; }

    // v0.1.110: natural language normalization. Strips politeness
    // prefixes ("could you please"), conversational phrasings
    // ("I want to", "I'd like to"), and trailing softeners ("for me",
    // "real quick"). After this pass the text is closer to a bare
    // command and the existing matchers fire reliably on natural
    // utterances.
    let t = normalize_natural_language(&t);
    if t.is_empty() { return Intent::Unknown; }

    // v0.1.149: floor-claim phrase. "<name>, here, <command>" or
    // "<name> here <command>" claims the floor for the named speaker
    // AND parses the remainder as their command. The name must match
    // a registered speaker (case-insensitive). Useful when speaker
    // identification is uncertain (cafe noise, drifted sub-profile)
    // — explicit override. Filed S, landed v0.1.149.
    //
    // Whisper transcription often punctuates this as
    //   "Kessler, here, run dust"
    // or as a single comma:
    //   "Kessler here run dust"
    // We accept both, plus colons after the name. We don't accept
    // "here" anywhere else in the utterance — only as the second
    // token. This prevents false matches on phrases like
    // "open the page that's right here".
    //
    // Unknown names: if the name doesn't match a registered speaker
    // we leave the prefix intact and parse normally. The "I'm X" /
    // "this is Alice" registration variant is a separate flow (filed).
    //
    // Runs BEFORE URL reconstruction so commands like "Kessler, here,
    // dot com" don't get parsed as a URL navigation.
    let t = strip_floor_claim_prefix(&t);
    if t.is_empty() { return Intent::Unknown; }

    // Try URL reconstruction: if text contains "dot", "slash", or spelled-out letters,
    // try to rebuild it as a URL. This handles cases like:
    //   "ay el ex dot es haitch slash asahi" -> "alx.sh/asahi"
    //   "g h dot com slash torvalds" -> "gh.com/torvalds"
    //   "open dee ee vee" -> "open dev"
    if let Some(url) = reconstruct_url(&t) {
        return Intent::Navigate(url);
    }

    let t = t.as_str();

    // Quit. v0.1.658: dropped bare "shut down"/"shutdown"/"leave" — a
    // kernel dev says "shut down" about a machine constantly (Asahi corpus
    // false-positive: "shut down" → Quit). Require an app-directed phrase.
    if matches!(t, "quit" | "exit" | "quit app" | "quit the app"
        | "close app" | "close the app" | "exit app" | "exit the app"
        | "shut down the app" | "bye" | "goodbye"
    ) {
        return Intent::Quit;
    }

    // Navigation history. v0.1.110: broader phrasings for natural speech.
    if matches!(t,
        "back" | "go back" | "previous" | "previous page" | "go to previous page"
        | "take me back" | "go backward" | "go backwards" | "navigate back"
        | "previous one" | "back one" | "back one page" | "back page"
        | "the previous page" | "the last page" | "last page"
    ) {
        return Intent::GoBack;
    }
    if matches!(t,
        "forward" | "go forward" | "next" | "next page" | "go to next page"
        | "take me forward" | "go forwards" | "navigate forward"
        | "next one" | "forward one" | "forward one page" | "forward page"
        | "the next page"
    ) {
        return Intent::GoForward;
    }
    if matches!(t,
        "reload" | "refresh" | "reload page" | "refresh page"
        | "reload the page" | "refresh the page" | "refresh this"
        | "reload this" | "reload it" | "refresh it" | "try again"
    ) {
        return Intent::Reload;
    }

    // v0.1.255 (GITLOG-3): rollback intent. Phrases the user
    // might say to undo the most recent mutating command. We
    // match before other catch-all paths so "undo" doesn't fall
    // through to Freeform. Avoid catching "go back" (browser
    // navigation, already handled above) or other ambiguous
    // phrases — require an explicit undo/rollback/revert word.
    if matches!(t,
        "undo" | "undo that" | "undo last" | "undo last command"
        | "undo it" | "undo the last command" | "undo the last one"
        | "rollback" | "roll back" | "rollback last" | "roll back last"
        | "rollback that" | "roll back that" | "revert" | "revert that"
        | "revert last" | "revert it"
        | "take that back" | "never mind that" | "nevermind that"
        | "scratch that"
    ) {
        return Intent::Rollback;
    }

    // v0.1.694: "redo" — only meaningful in the note editor (dispatch is a
    // no-op elsewhere). Kept narrow to avoid catching unrelated phrases.
    if matches!(t,
        "redo" | "redo that" | "redo last" | "redo it" | "redo the last one"
        | "do it again" | "put it back"
    ) {
        return Intent::EditorRedo;
    }

    // v0.1.701: "read this section" aloud (TTS + utterance-clip blend).
    if matches!(t,
        "read this section" | "read this" | "read it" | "read it to me"
        | "read it aloud" | "read aloud" | "read this to me" | "speak this"
        | "speak this section" | "read the section" | "narrate this"
    ) {
        return Intent::ReadSection;
    }

    // v0.1.273: audit log viewer intent. Routes to
    // `web2qml://audit` (handled QML-side by translating
    // show_audit_log → navigate). Phrases cover the natural
    // ways someone asks "what commands have been run" or
    // "show me the history with attribution":
    //   - "show audit log" / "open audit log" / "audit log"
    //   - "show command history" / "command history"
    //   - "what did i do" / "what did i run" — natural-language
    //     variants. Match strictly to avoid false positives
    //     ("what did i type just now" doesn't get caught).
    //   - "who ran X" deferred — that's a query that needs an
    //     argument; the audit page itself lets you scan.
    // v0.1.277: speaker-filtered audit log. Match BEFORE the
    // unfiltered ShowAuditLog since "what did Allison do" would
    // otherwise need a longer phrase list. Capture the speaker
    // name suffix and pass to ShowAuditLogForSpeaker.
    //
    // Patterns:
    //   "show audit log for X"
    //   "audit log for X"
    //   "what did X do" / "what did X run"
    //
    // Strict-prefix match — we extract the name as the remainder
    // after the trigger phrase. Single-token names are most
    // common (Allison, Kessler) but multi-word names (full names)
    // work too: "show audit log for Allison Smith" → "Allison Smith".
    {
        let triggers_with_name: &[&str] = &[
            "show audit log for ",
            "audit log for ",
            "show commands by ",
            "commands by ",
            "what did ",
        ];
        for trigger in triggers_with_name {
            if let Some(rest) = t.strip_prefix(trigger) {
                // For "what did X do/run" — strip trailing verb.
                let cleaned = rest.trim()
                    .trim_end_matches(" do")
                    .trim_end_matches(" run")
                    .trim_end_matches(" did")
                    .trim();
                // Reject sentinels that the unfiltered "what
                // did I do" parser handles. "what did i do" /
                // "what did i run" / "what did i do last time" should
                // NOT be a speaker filter (speaker would be "i"). Skip
                // any first-person phrase — those are recall, handled
                // later (v0.1.655: the bare "i" check missed "i do last
                // time" etc., so check the leading word).
                let lowered = cleaned.to_lowercase();
                let first_person = lowered == "i" || lowered == "we"
                    || lowered.starts_with("i ") || lowered.starts_with("we ");
                if first_person || lowered.is_empty() || cleaned.len() >= 60 {
                    continue;
                }
                return Intent::ShowAuditLogForSpeaker(capitalize_first(cleaned));
            }
        }
    }

    if matches!(t,
        "show audit log" | "open audit log" | "audit log"
        | "show the audit log" | "open the audit log"
        | "audit" | "show audit" | "show audits"
        | "show command history" | "command history"
        | "show command log" | "command log"
        | "show history with attribution" | "attribution log"
        | "what did i run" | "what have i run"
        | "what have i done" | "show what i did"
        | "who ran what" | "what was run"
        | "show recent commands" | "recent commands"
    ) {
        return Intent::ShowAuditLog;
    }

    // v0.1.274 (GITLOG-5): branch tree viewer intent. Routes
    // to `web2qml://branches` showing forks created by
    // rollbacks. Distinct from audit-log: audit is linear
    // history, branches is the tree-structure view.
    if matches!(t,
        "show branches" | "show branch" | "branches"
        | "show command tree" | "command tree" | "show tree"
        | "show forks" | "show fork" | "show the tree"
        | "show the branches" | "show command branches"
        | "command branches"
    ) {
        return Intent::ShowBranches;
    }

    // v0.1.276: audit-log integrity verification. Walks the
    // hash chain end-to-end and surfaces a pass/fail toast.
    // Distinct from "show audit log" — verify is a CHECK
    // action, show is a NAVIGATION action.
    if matches!(t,
        "verify audit log" | "verify the audit log" | "check audit log"
        | "check the audit log" | "verify integrity" | "check integrity"
        | "verify audit" | "check audit" | "verify chain"
        | "check chain" | "verify hash chain" | "check hash chain"
        | "audit integrity" | "is the audit log valid"
        | "is the log intact"
    ) {
        return Intent::VerifyAuditLog;
    }

    // v0.1.368: reset-cadence intent. Two shapes:
    //   - Self / unspecified: "reset my cadence", "reset cadence",
    //     "clear my cadence" → ResetCadence("") (caller resolves
    //     empty name to active speaker)
    //   - Named: "reset cadence for Bob", "clear Bob's cadence" →
    //     ResetCadence("Bob") (caller looks up by name)
    //
    // Order: try named (more specific) first, then fall through
    // to self phrasings. We require the word "cadence" in every
    // accepted phrase to avoid colliding with other reset commands
    // (cards-canvas "reset filter", future workspace resets, etc.)
    {
        // Named variant: "reset cadence for X" / "clear cadence for X"
        let named_triggers: &[&str] = &[
            "reset cadence for ",
            "clear cadence for ",
            "reset the cadence for ",
            "clear the cadence for ",
            "reset speech cadence for ",
            "clear speech cadence for ",
            "reset speaking cadence for ",
            "clear speaking cadence for ",
        ];
        for trig in named_triggers {
            if let Some(rest) = t.strip_prefix(trig) {
                let n = rest.trim();
                if !n.is_empty() {
                    // Reject the name "me" / "myself" — that's a
                    // self-reference, not a target. Fall through
                    // to the self-phrasing match.
                    let nl = n.to_ascii_lowercase();
                    if !matches!(nl.as_str(), "me" | "myself" | "my self") {
                        return Intent::ResetCadence(recover_name_case(text, n));
                    }
                }
            }
        }
        // Possessive: "reset Bob's cadence" / "clear Bob's cadence"
        for prefix in &["reset ", "clear "] {
            if let Some(rest) = t.strip_prefix(prefix) {
                let candidates: [&str; 4] = [
                    "'s cadence", "s cadence",
                    "'s speech cadence", "'s speaking cadence",
                ];
                for suffix in &candidates {
                    if let Some(name_part) = rest.strip_suffix(suffix) {
                        let n = name_part.trim();
                        let nl = n.to_ascii_lowercase();
                        if !n.is_empty()
                            && !matches!(nl.as_str(),
                                "my" | "the" | "your" | "his" | "her" | "their")
                        {
                            return Intent::ResetCadence(recover_name_case(text, n));
                        }
                    }
                }
            }
        }
        // Self variant: must mention "cadence" explicitly.
        if matches!(t,
            "reset my cadence" | "clear my cadence"
            | "reset cadence" | "clear cadence"
            | "reset the cadence" | "clear the cadence"
            | "reset my speech cadence" | "clear my speech cadence"
            | "reset speech cadence" | "clear speech cadence"
            | "reset speaking cadence" | "clear speaking cadence"
            | "reset my speaking cadence" | "clear my speaking cadence"
            | "reset cadence for me" | "clear cadence for me"
            | "reset cadence for myself" | "clear cadence for myself"
        ) {
            return Intent::ResetCadence(String::new());
        }
    }

    // v0.1.377 (discoverability arc): show recipes page. The
    // catch-all "what can you do" phrasings are uniquely
    // discoverable because they're what new users actually ask
    // out loud. We accept several variants but require the word
    // "do" / "say" / "recipes" / "capabilities" to avoid
    // collisions with other intents.
    if matches!(t,
        "show recipes" | "open recipes" | "recipes"
        | "show capabilities" | "open capabilities"
        | "what can you do" | "what can web2qml do"
        | "what can i do" | "what can i say"
        | "what can this do" | "what can we do"
        | "help" | "help me" | "show help"
    ) {
        return Intent::ShowRecipes;
    }

    // v0.1.382 (session arc, step 3): summarize the most
    // recent session. The phrasings are intentionally
    // session-flavored ("last session", "today",
    // "been doing") so we don't collide with the more
    // generic "what can I do" (ShowRecipes) — note "what
    // have I been doing" is unique to session-summarize.
    if matches!(t,
        "summarize my session" | "summarize this session"
        | "summarize session" | "summarize the session"
        | "what did i do last session" | "what did i do this session"
        | "what did i do today" | "what have i been doing"
        | "what have i done" | "session summary"
        | "summarize my work"
    ) {
        return Intent::SummarizeSession;
    }

    // v0.1.385 (discoverability arc): context-aware help.
    // Distinct phrasings from ShowRecipes ("what can you do",
    // generic catalog) — these are "what's available HERE,
    // RIGHT NOW" — focused per-surface lists.
    if matches!(t,
        "what can i do here" | "what's available"
        | "what is available" | "show shortcuts for this page"
        | "what's on this page" | "what is on this page"
        | "what now" | "what's relevant"
        | "what can i do on this page"
    ) {
        return Intent::ContextHelp;
    }

    // v0.1.386 (bookmarks): "save this" / "bookmark this"
    // saves the current page. Try bare phrasings first.
    if matches!(t,
        "save this" | "save this page" | "save current page"
        | "save current url" | "save the page" | "save page"
        | "bookmark this" | "bookmark this page"
        | "bookmark current page" | "bookmark the page"
        | "remember this" | "remember this page"
        | "remember this url" | "save url"
    ) {
        return Intent::SaveUrl(String::new());
    }

    // Parametric: "save url <X>" / "bookmark url <X>". The
    // URL after the prefix is taken verbatim; backend
    // normalizes it (adds https:// etc.).
    for prefix in &["save url ", "bookmark url ",
                    "save the url ", "bookmark the url ",
                    "save link ", "bookmark link "] {
        if let Some(rest) = t.strip_prefix(*prefix) {
            let url = rest.trim();
            if !url.is_empty() {
                return Intent::SaveUrl(url.to_string());
            }
        }
    }

    // v0.1.386: show bookmarks list.
    if matches!(t,
        "show bookmarks" | "open bookmarks" | "my bookmarks"
        | "show my bookmarks" | "show saved pages"
        | "show my saved pages" | "list bookmarks"
        | "saved pages" | "saved urls" | "bookmarks"
    ) {
        return Intent::ShowBookmarks;
    }

    // v0.1.394 (drag-drop arc): open a platform file
    // picker. Exact-string matches only — partial-prefix
    // would collide with "open in new tab", "open
    // workspace X", etc.
    if matches!(t,
        "open file" | "open a file" | "open file picker"
        | "open file dialog" | "open the file picker"
        | "open the file dialog"
        | "pick a file" | "choose a file" | "select a file"
        | "browse files" | "browse for a file"
        | "file picker" | "file dialog"
    ) {
        return Intent::OpenFileDialog;
    }

    // v0.1.387: tagged bookmarks. Parser tries the patterns
    // in order of specificity. The bare "<tag> bookmarks"
    // form must come LAST since "show bookmarks" would
    // otherwise be eaten by "show <tag> bookmarks" with
    // tag="show". The exact "show bookmarks" check above
    // happens first; this section only fires for non-bare
    // queries.
    for prefix in &[
        "show bookmarks tagged ", "show bookmarks with tag ",
        "filter bookmarks by ", "filter bookmarks by tag ",
        "bookmarks tagged ",
    ] {
        if let Some(rest) = t.strip_prefix(*prefix) {
            let tag = rest.trim();
            if !tag.is_empty() {
                return Intent::ShowBookmarksTagged(tag.to_string());
            }
        }
    }
    // "show <tag> bookmarks" — single-word tag between
    // "show" and "bookmarks".
    if let Some(rest) = t.strip_prefix("show ") {
        if let Some(tag) = rest.strip_suffix(" bookmarks") {
            let tag = tag.trim();
            if !tag.is_empty() && !tag.contains(' ') {
                return Intent::ShowBookmarksTagged(tag.to_string());
            }
        }
    }
    // "<tag> bookmarks" — bare form, single word.
    if let Some(tag) = t.strip_suffix(" bookmarks") {
        let tag = tag.trim();
        if !tag.is_empty()
            && !tag.contains(' ')
            // Avoid eating phrases that look like commands.
            && !matches!(tag, "my" | "saved" | "the" | "all")
        {
            return Intent::ShowBookmarksTagged(tag.to_string());
        }
    }

    // v0.1.766: build-error menu. Recognize a small set of phrases that ask to
    // summarize the focused pane's recent cargo/cmake output. Matched before the
    // pane block (no overlap with pane verbs).
    {
        let strip_filler = t
            .trim_start_matches("show me ")
            .trim_start_matches("show ")
            .trim_start_matches("what are ")
            .trim_start_matches("what's ")
            .trim_start_matches("the ")
            .trim();
        let is_build_errors = matches!(t,
            "show errors" | "show error" | "show me errors"
            | "show build errors" | "show me the build errors"
            | "show the errors" | "what are the errors" | "build errors"
            | "summarize errors" | "summarize the build" | "summarize build"
            | "error menu" | "show the build errors")
            || matches!(strip_filler, "errors" | "build errors" | "error menu")
            || (t.starts_with("show") && t.contains("error") && t.contains("build"));
        if is_build_errors {
            return Intent::ShowBuildErrors;
        }
    }

    // v0.1.746: native terminal pane/split vocabulary. Unambiguous against
    // browser tabs (browser has no panes/splits/focus-direction), so matched
    // BEFORE the browser-tab block. The phrase is re-parsed by the tested
    // terminal_workspace::parse_command in the backend; here we only need to
    // RECOGNIZE that it's a pane command and pass it through.
    {
        // v0.1.752: a "split ..." utterance is a pane command only if it also
        // carries a pane/orientation/direction keyword — so "split this pane
        // horizontal" / "split vertically" / "split down" route to TermPane,
        // but "split the bill with me" (no such keyword) falls through.
        let split_kw = t.starts_with("split") && (
            t.contains("pane") || t.contains("horizontal") || t.contains("vertical")
            || t.contains(" left") || t.contains(" right")
            || t.contains(" up") || t.contains(" down")
            || t == "split");
        let is_pane = t.starts_with("new pane")
            || split_kw
            || t == "close pane" || t == "kill pane"
            || matches!(t, "focus left" | "focus right" | "focus up" | "focus down")
            || matches!(t, "resize left" | "resize right" | "resize up" | "resize down");
        // v0.1.755: explicit TERMINAL-tab phrasings. Plain "new tab"/"next tab"
        // stay BROWSER intents (the user is usually browsing), but prefixing
        // with "terminal" is unambiguous and lets voice open/switch terminal
        // tabs — the dogfood gap ("nowhere can I use voice ... to open terminal
        // tabs"). Strip "terminal " and re-check parse_command.
        if let Some(rest0) = t.strip_prefix("terminal ") {
            let rest = rest0.trim();
            if matches!(rest, "tab" | "new tab" | "next tab" | "previous tab"
                            | "prev tab" | "close tab" | "last tab") {
                let cmd = if rest == "tab" { "new tab" } else { rest };
                if crate::terminal_workspace::parse_command(cmd).is_some() {
                    return Intent::TermPane(cmd.to_string());
                }
            }
        }
        if is_pane && crate::terminal_workspace::parse_command(t).is_some() {
            return Intent::TermPane(t.to_string());
        }
        // v0.1.792: bulk tab-order voice commands ("bubble all tabs", "rollback
        // my tabs one step"). Distinct from Intent::Rollback (which is exact
        // "rollback"/"undo" only, matched earlier) because these carry "tabs".
        if (t.starts_with("bubble") && t.contains("tab"))
            || (t.contains("tab") && (t.starts_with("rollback") || t.starts_with("roll back")
                                      || t.starts_with("undo")))
            || t == "organize tabs" || t == "organize my tabs"
        {
            if crate::terminal_workspace::parse_command(t).is_some() {
                return Intent::TermPane(t.to_string());
            }
            // v0.1.795: "bubble tabs with X" (keyword-bubble) is handled inside
            // ws_command (it needs tab names), so parse_command won't accept it —
            // route it anyway when the "with <keyword>" form is present.
            if t.starts_with("bubble") && t.contains("tab") && t.contains(" with ") {
                return Intent::TermPane(t.to_string());
            }
        }
        // v0.1.799: tile-ssh — "tile/group/gather (all) (my) ssh (sessions)".
        // Handled in ws_command (needs each pane's foreground process), so
        // parse_command won't accept it — route it here when it's an ssh phrase.
        if t.contains("ssh")
            && (t.starts_with("tile") || t.starts_with("gather") || t.starts_with("group")) {
            return Intent::TermPane(t.to_string());
        }
    }

    // v0.1.292 (TAB voice): browser tab management. Order matters
    // — "next tab" / "previous tab" / "close tab" must be tried
    // BEFORE the generic tab_open parser which has a broader
    // prefix.
    if matches!(t,
        "next tab" | "go to next tab" | "switch to next tab"
        | "other tab" | "the other tab"
    ) { return Intent::TabNext; }
    if matches!(t,
        "previous tab" | "prev tab" | "back tab" | "last tab"
        | "go to previous tab" | "switch to previous tab"
        | "go to prev tab" | "switch to prev tab"
    ) { return Intent::TabPrev; }
    if matches!(t,
        "close tab" | "close this tab" | "close current tab"
        | "close the tab" | "close active tab" | "kill this tab"
    ) { return Intent::TabClose; }
    // "open new tab" / "new tab" / "fork tab"
    if matches!(t, "new tab" | "open new tab" | "open a new tab"
        | "fork tab" | "fork this tab" | "duplicate tab" | "duplicate this tab"
    ) {
        return Intent::TabOpen(String::new());  // empty = fork current
    }

    // v0.1.389: switch to a tab by substring match. The
    // phrasings ("switch to the X tab", "go to the X tab",
    // "go to X tab") need to come AFTER the directional
    // intents (TabNext / TabPrev / TabClose) so "go to next
    // tab" isn't misread as "match a tab whose URL contains
    // 'next'". The matches!() exact-string checks above
    // already handle those collisions safely.
    //
    // We accept three forms and extract the query from each:
    //   "switch to the X tab" / "go to the X tab"
    //   "switch to tab with X" / "go to tab with X"
    //   "switch to X tab" / "go to X tab"  (without "the")
    {
        let tab_match_prefixes: &[&str] = &[
            "switch to the ", "go to the ",
            "switch to tab with ", "go to tab with ",
            "switch to ", "go to ",
        ];
        for prefix in tab_match_prefixes {
            if let Some(rest) = t.strip_prefix(*prefix) {
                // Try stripping the "tab" suffix. Two forms:
                //   "X tab" — common ("github tab")
                //   "X" — without — only fires for "with"
                //         prefixes where the "tab" was at
                //         the beginning ("tab with X").
                let needs_tab_suffix = !prefix.contains("tab with");
                let query = if needs_tab_suffix {
                    match rest.strip_suffix(" tab") {
                        Some(q) => q.trim(),
                        None => continue,  // not a tab-match phrasing
                    }
                } else {
                    rest.trim()
                };
                if query.is_empty() {
                    continue;
                }
                // Avoid swallowing literal directional words
                // even though `matches!` above caught them
                // first — defensive against partial whisper
                // transcription.
                if matches!(query,
                    "next" | "previous" | "prev" | "last" | "first"
                    | "this" | "current" | "the other" | "other"
                    // v0.1.772: bare articles are not tab names — "switch to
                    // the tab" left query="the", which matched a literal tab
                    // named "the".
                    | "the" | "a" | "an")
                {
                    continue;
                }
                return Intent::SwitchToTabMatching(query.to_string());
            }
        }
    }

    // v0.1.390: save workspace as a named snapshot. The
    // prefix patterns must come BEFORE the broader "save X"
    // parsers if any exist; "save workspace as X" wouldn't
    // collide with SaveUrl since SaveUrl's patterns are
    // exact-strings or "save url X" / "save link X" — none
    // match "save workspace".
    for prefix in &[
        "save workspace as ", "save this workspace as ",
        "save my workspace as ", "save current workspace as ",
        "save the workspace as ",
    ] {
        if let Some(rest) = t.strip_prefix(*prefix) {
            let name = rest.trim();
            if !name.is_empty() && !is_reserved_workspace_name(name) {
                return Intent::SaveWorkspaceAs(name.to_string());
            }
        }
    }

    // v0.1.390: load a named workspace. Two phrasing
    // families:
    //   "load workspace X" / "open workspace X" /
    //     "switch to workspace X"
    //   "load the X workspace" / "switch to the X workspace"
    for prefix in &[
        "load workspace ", "load the workspace ",
        "open workspace ", "open the workspace ",
        "switch to workspace ", "switch to the workspace ",
    ] {
        if let Some(rest) = t.strip_prefix(*prefix) {
            let name = rest.trim();
            if !name.is_empty() && !is_reserved_workspace_name(name) {
                return Intent::LoadWorkspace(name.to_string());
            }
        }
    }
    // "load the X workspace" form.
    if let Some(rest) = t.strip_prefix("load the ") {
        if let Some(name) = rest.strip_suffix(" workspace") {
            let name = name.trim();
            if !name.is_empty() && !is_reserved_workspace_name(name) {
                return Intent::LoadWorkspace(name.to_string());
            }
        }
    }
    if let Some(rest) = t.strip_prefix("switch to the ") {
        if let Some(name) = rest.strip_suffix(" workspace") {
            let name = name.trim();
            if !name.is_empty() && !is_reserved_workspace_name(name) {
                return Intent::LoadWorkspace(name.to_string());
            }
        }
    }
    if let Some(rest) = t.strip_prefix("switch to ") {
        if let Some(name) = rest.strip_suffix(" workspace") {
            let name = name.trim();
            if !name.is_empty() && !is_reserved_workspace_name(name) {
                return Intent::LoadWorkspace(name.to_string());
            }
        }
    }

    if let Some(rest) = t.strip_suffix(" in new tab")
        .or_else(|| t.strip_suffix(" in a new tab"))
        .or_else(|| t.strip_suffix(" in another tab"))
    {
        // Strip a leading "open " / "navigate to " / "go to " if
        // present so we get just the site name. Reuse the
        // open-prefix list informally.
        let rest = rest.trim_start_matches("open ")
            .trim_start_matches("navigate to ")
            .trim_start_matches("go to ")
            .trim();
        if !rest.is_empty() {
            return Intent::TabOpen(rest.to_string());
        }
    }

    // v0.1.296 (Tier1 diag): acoustic-index status. Diagnostic
    // intent — toast with exemplar count + mode.
    if matches!(t,
        "tier 1 status" | "tier one status" | "show tier 1"
        | "show tier one" | "acoustic status" | "acoustic index status"
        | "show acoustic stats" | "show acoustic status"
        | "exemplar count" | "tier1 status"
    ) {
        return Intent::Tier1Status;
    }
    // v0.1.300 (Tier1-3): histogram summary with suggested threshold.
    if matches!(t,
        "tier 1 histogram" | "tier one histogram" | "tier1 histogram"
        | "show histogram" | "show distances" | "show distance histogram"
        | "threshold suggestion" | "suggested threshold"
        | "show suggested threshold" | "acoustic histogram"
    ) {
        return Intent::Tier1Histogram;
    }
    // v0.1.302 (Tier1-4): runtime PROMOTE toggle.
    if matches!(t,
        "promote tier 1" | "promote tier one" | "promote acoustic"
        | "promote acoustic index" | "enable promote" | "turn on promote"
        | "switch to promote" | "promote mode on"
    ) {
        return Intent::Tier1PromoteOn;
    }
    if matches!(t,
        "demote tier 1" | "demote tier one" | "disable promote"
        | "turn off promote" | "back to shadow" | "switch to shadow"
        | "promote mode off" | "shadow mode"
    ) {
        return Intent::Tier1PromoteOff;
    }
    // v0.1.305: auto-tune (one-step apply histogram suggestion).
    if matches!(t,
        "auto tune tier 1" | "auto-tune tier 1" | "autotune tier 1"
        | "auto tune tier one" | "auto-tune tier one" | "autotune tier one"
        | "tune tier 1" | "tune tier one"
        | "calibrate tier 1" | "calibrate tier one"
        | "auto promote" | "auto-promote"
    ) {
        return Intent::Tier1AutoTune;
    }
    // v0.1.304: "set threshold to X" / "tier 1 threshold X"
    // / "promote threshold X". Whisper transcribes numbers
    // either as digits or words ("zero point one two"). Just
    // accept the digit form for now.
    let threshold_prefixes: &[&str] = &[
        "set tier 1 threshold to ",
        "set tier one threshold to ",
        "set threshold to ",
        "tier 1 threshold ",
        "tier one threshold ",
        "promote threshold ",
        "threshold ",
    ];
    for &pfx in threshold_prefixes {
        if let Some(rest) = t.strip_prefix(pfx) {
            let rest = rest.trim().trim_end_matches('.').trim_end_matches(',');
            if let Ok(val) = rest.parse::<f32>() {
                if val.is_finite() && val >= 0.0 && val <= 1.0 {
                    return Intent::Tier1SetThreshold(val);
                }
            }
        }
    }

    // v0.1.297 (TAB-3): workspace voice management. List/save/load.
    if matches!(t,
        "list workspaces" | "show workspaces" | "show my workspaces"
        | "my workspaces" | "what workspaces" | "what are my workspaces"
    ) {
        return Intent::WorkspaceList;
    }
    // Save as. Strict prefixes to avoid catching the broader
    // "save" word (which doesn't mean workspace anywhere else
    // currently, but defensive). Also: do NOT match if the
    // captured name starts with "workflow " — that goes to the
    // WorkflowStartRecording parser further down.
    if let Some(name) = t.strip_prefix("save workspace as ")
        .or_else(|| t.strip_prefix("save this as "))
        .or_else(|| t.strip_prefix("save as workspace "))
        .or_else(|| t.strip_prefix("save as "))
    {
        let name = name.trim();
        // Fall through to workflow parser when user said
        // "save this as workflow <X>" / "save as workflow <X>".
        if !name.is_empty() && !name.starts_with("workflow ") {
            return Intent::WorkspaceSaveAs(name.to_string());
        }
    }
    // v0.1.315: explicit "save the current workspace" (no name).
    // Must come AFTER "save workspace as <X>" so the prefix match
    // doesn't shadow that one.
    if matches!(t,
        "save workspace" | "save current workspace" | "save my workspace"
        | "save the workspace" | "snapshot workspace"
        | "snapshot my workspace" | "checkpoint workspace"
    ) {
        return Intent::WorkspaceSaveNow;
    }
    // v0.1.327 (Mermaid+/cards arc 2.7.3): mermaid demo.
    // MUST come before OpenConnectionsView so "show diagram"
    // doesn't get captured by the looser "show ..." matches.
    if matches!(t,
        "show mermaid demo" | "show diagram demo" | "demo diagram"
        | "mermaid demo" | "show the mermaid demo"
        | "show me a mermaid demo" | "render mermaid demo"
        | "test mermaid" | "test the mermaid parser"
    ) {
        return Intent::ShowMermaidDemo;
    }
    // v0.1.329 (cards arc): flatten — must come BEFORE tilt
    // v0.1.698 (niftyplot view): rotate the layered diagram to a named
    // orientation. "flatten" stays FlattenGraph (handled just below).
    {
        let rot = if matches!(t, "fan the layers" | "fan layers" | "fan the diagram"
                | "fan diagram" | "rotate the diagram" | "rotate diagram"
                | "rotate the layers" | "rotate" | "spread the layers"
                | "spread layers") { Some("fan") }
            else if matches!(t, "isometric" | "isometric view" | "iso view"
                | "stack the layers" | "stack layers" | "stack view") { Some("iso") }
            else if matches!(t, "tilt the layers" | "tilt layers" | "tilt view") { Some("tilt") }
            else { None };
        if let Some(p) = rot { return Intent::RotateGraph(p.to_string()); }
    }
    // v0.1.698: smart expansion — reveal / hide a ring of nodes.
    if matches!(t,
        "expand" | "expand node" | "expand this" | "go deeper" | "show more"
        | "reveal more" | "expand the node" | "one more ring"
    ) {
        return Intent::ExpandNode;
    }
    if matches!(t,
        "collapse" | "collapse node" | "show fewer" | "show less"
        | "less detail" | "one fewer ring" | "pull back"
    ) {
        return Intent::CollapseNode;
    }
    // parsers so "flatten graph" doesn't fall into a numeric
    // tilt search and end up with angle=0 (same effect, but
    // FlattenGraph is the clearer Intent for the dispatcher).
    if matches!(t,
        "flatten graph" | "flatten the graph" | "flatten view"
        | "flatten" | "go flat" | "collapse to 2d" | "collapse to two d"
        | "collapse graph" | "collapse the graph" | "untilt"
        | "tilt zero" | "tilt to zero"
    ) {
        return Intent::FlattenGraph;
    }
    // v0.1.329: tilt with optional angle. Forms:
    //   "tilt graph"           → 35° default
    //   "tilt view"            → 35° default
    //   "tilt 30"              → 30°
    //   "tilt 45 degrees"      → 45°
    //   "tilt graph 30"        → 30°
    //   "tilt to 45 degrees"   → 45°
    //   "tilt steeper"         → 60° (preset)
    //   "tilt more"            → 60° (preset)
    let tilt_prefixes: &[&str] = &[
        "tilt graph ", "tilt view ", "tilt to ", "tilt ",
    ];
    for &pfx in tilt_prefixes {
        if let Some(rest) = t.strip_prefix(pfx) {
            let rest = rest.trim();
            let rest = rest.trim_end_matches(" degrees")
                .trim_end_matches(" degree")
                .trim_end_matches('°')
                .trim();
            // Presets.
            if matches!(rest, "steeper" | "more" | "max" | "steep") {
                return Intent::TiltGraph(60.0);
            }
            if matches!(rest, "less" | "gentle" | "slight") {
                return Intent::TiltGraph(20.0);
            }
            // Parse numeric angle. Clamp to [0, 75] — beyond
            // that the view tips past readability.
            if let Ok(v) = rest.parse::<f32>() {
                if v.is_finite() {
                    let clamped = v.clamp(0.0, 75.0);
                    return Intent::TiltGraph(clamped);
                }
            }
        }
    }
    // Bare "tilt" / "tilt graph" with no angle → default 35°.
    if matches!(t,
        "tilt" | "tilt graph" | "tilt the graph" | "tilt view"
        | "tilt the view" | "perspective view" | "3d view"
        | "go 3d" | "show 3d"
    ) {
        return Intent::TiltGraph(35.0);
    }
    // v0.1.643: "walk me through my last session" — must come before
    // OpenConnectionsView (it's a more specific request that also opens
    // the graph, then auto-plays the walk).
    if matches!(t,
        "walk me through my last session" | "walk me through my last session's pages"
        | "walk me through last session" | "walk my last session"
        | "walk last session" | "walk through my last session"
        | "walk me through the last session" | "session walk"
        | "show me my last session" | "show my last session" | "replay my last session"
        | "walk me through my session"
    ) {
        return Intent::WalkLastSession;
    }
    // v0.1.649: "what did I do" / "how did I fix X" / "what was that
    // command" → recall past commands. Strip the recall lead-in to get
    // the query; an empty query means "most recent". Placed after the
    // session-walk arm so "show me my last session" stays a walk.
    {
        let recall_leadins = [
            "what did i do about ", "what did i do with ", "what did i do for ",
            "what did i do ", "how did i fix ", "how did i do ", "how did i ",
            "what was that command for ", "what was that command ",
            "what was the command for ", "what was the command ",
            "what command did i use for ", "what command did i run for ",
            "recall the command for ", "recall command ", "recall ",
            "find the command for ", "what did i run ",
        ];
        let recall_bare = [
            "what did i do last time", "what did i just do", "what did i do",
            "what was that command", "what command was that",
            "recall commands", "recall my commands", "command history",
        ];
        if recall_bare.iter().any(|b| *b == t) {
            return Intent::RecallCommands(String::new());
        }
        for lead in recall_leadins {
            if let Some(rest) = t.strip_prefix(lead) {
                let q = rest.trim().trim_end_matches('?').trim();
                // v0.1.658: reject demonstrative-only queries — "what did i
                // do with this one" is narration, not a recall search. A
                // real recall query names a topic ("the docker build"), not
                // just a pronoun (optionally after with/about/to/for).
                let qn = q.trim_start_matches("with ").trim_start_matches("about ")
                    .trim_start_matches("to ").trim_start_matches("for ").trim();
                let demonstrative = matches!(qn,
                    "this" | "that" | "those" | "these" | "this one" | "that one"
                    | "it" | "this thing" | "that thing" | "them" | "this stuff" | "that stuff");
                if demonstrative || qn.is_empty() { continue; }
                return Intent::RecallCommands(q.to_string());
            }
        }
    }
    // v0.1.680: full-text search across note bodies (the "cloak" path).
    // Handle prefix forms ("search my notes for X") and the infix form
    // ("find X in my notes"). Placed before EditCurrentNote/OpenInEditor.
    {
        for prefix in &[
            "search my notes for ", "search notes for ", "search my notes ",
            "search my scripts for ", "search scripts for ",
            "find in my notes ", "find in notes ", "grep my notes for ",
        ] {
            if let Some(q) = t.strip_prefix(prefix) {
                let q = q.trim();
                if !q.is_empty() && q.len() < 200 {
                    return Intent::SearchNotes(q.to_string());
                }
            }
        }
        // infix: "find X in my notes" / "find X in my scripts"
        for suffix in &[" in my notes", " in my scripts", " in notes", " across my notes"] {
            if let Some(head) = t.strip_suffix(suffix) {
                if let Some(q) = head.strip_prefix("find ").or_else(|| head.strip_prefix("search for ")) {
                    let q = q.trim();
                    if !q.is_empty() && q.len() < 200 {
                        return Intent::SearchNotes(q.to_string());
                    }
                }
            }
        }
    }
    // v0.1.678: open the in-app WYSIWYG note editor on the CURRENT page.
    // Placed BEFORE the OpenInEditor (external vim/helix) block so the
    // "this note" / "the editor" phrasings — which have no external file
    // target — win over the generic "edit <thing>" path.
    {
        let tt: &str = &t;
        let edit_current = matches!(tt,
            "edit this note" | "edit this" | "edit note" | "edit current note"
            | "edit the note" | "edit this file" | "edit current file"
            | "open the editor" | "open note editor" | "open the note editor"
            | "open editor" | "edit in the editor" | "wysiwyg edit");
        if edit_current {
            return Intent::EditCurrentNote;
        }
    }
    // v0.1.654: editor-open with advanced context. Only triggers when a
    // distinguishing marker is present so plain "open <site>" stays a
    // navigation: an "edit" verb, OR an explicit editor (" in vim/helix"),
    // OR a line slot ("at line N" / "on line N"), OR a host (" on ").
    {
        let has_edit_verb = t.starts_with("edit ") || t.starts_with("go edit ");
        let has_editor = [" in vim", " in nvim", " in neovim", " in helix",
                          " in hx", " with vim", " with helix"]
            .iter().any(|k| t.contains(k));
        let has_line = t.contains(" at line ") || t.contains(" on line ")
            || t.contains(" line ");
        let has_host = t.contains(" on "); // "on <host>" (after a thing)
        let is_open = t.starts_with("open ") || t.starts_with("open up ")
            || t.starts_with("pull up ");
        // require an editor-ish signal; "open X on line N"/"open X in helix"/
        // "edit X" qualify, plain "open google" does not.
        if (has_edit_verb || (is_open && (has_editor || has_line || has_host)))
            && crate::editor_open::parse_open_request(&t).is_some()
        {
            return Intent::OpenInEditor(t.to_string());
        }
    }
    // v0.1.319: open the connections graph view.
    if matches!(t,
        "show connections" | "show graph" | "show the graph"
        | "open graph view" | "open the graph view" | "open graph"
        | "show cards" | "show the cards" | "show card view"
        | "connections view" | "open connections" | "open connections view"
        | "graph view" | "show connections graph" | "show map"
    ) {
        return Intent::OpenConnectionsView;
    }
    // v0.1.338 (cards arc): close the cards canvas.
    if matches!(t,
        "close graph" | "close the graph" | "close cards"
        | "close the cards" | "close card view" | "close map"
        | "close connections" | "close connections view"
        | "close graph view" | "back to page" | "exit graph"
        | "exit cards" | "dismiss graph" | "dismiss cards"
        | "hide graph" | "hide cards"
    ) {
        return Intent::CloseConnectionsView;
    }
    // v0.1.338 (cards arc): reset zoom / fit graph.
    if matches!(t,
        "reset zoom" | "reset the zoom" | "fit graph" | "fit the graph"
        | "fit cards" | "fit the cards" | "fit to view"
        | "center graph" | "center the graph" | "recenter"
        | "recenter graph" | "recenter the graph" | "zoom to fit"
        | "show whole graph" | "show full graph"
    ) {
        return Intent::ResetCardsZoom;
    }
    // v0.1.343 (niftyplot arc): unfocus / show full diagram.
    // Must come BEFORE the focus-prefix parsers below so
    // "show everything" doesn't get captured as a focus query.
    if matches!(t,
        "unfocus" | "show everything" | "show full diagram"
        | "show all planes" | "clear focus" | "unscope"
        | "show whole diagram"
    ) {
        return Intent::UnfocusPlane;
    }
    // v0.1.343 (niftyplot arc): read parser warnings aloud.
    if matches!(t,
        "what's wrong" | "what is wrong" | "read warnings"
        | "read the warnings" | "read parser warnings"
        | "what did the parser say" | "any warnings"
        | "show warnings" | "diagram warnings"
    ) {
        return Intent::ReadDiagramWarnings;
    }
    // v0.1.343 (niftyplot arc): focus a plane / group / layer.
    // Prefixes: "focus", "zoom into", "show only".
    for pfx in &[
        "focus ", "focus on ", "focus plane ", "focus layer ",
        "zoom into ", "zoom in on ", "scope to ", "scope ",
        "show only ", "show just ", "show only plane ",
        "view plane ", "view layer ",
    ] {
        if let Some(rest) = t.strip_prefix(pfx) {
            let name = rest.trim().to_string();
            // v0.1.772: "show only tag X" / "show only @X" are TAG filters,
            // not plane focus — let them fall through to the tag-filter block
            // below. (The "show only " prefix is shared between the two.)
            if name.starts_with("tag ") || name.starts_with('@') {
                continue;
            }
            if !name.is_empty() {
                return Intent::FocusPlane(name);
            }
        }
    }
    // v0.1.323 (cards arc 2.5.2): clear filter.
    // Must come BEFORE the prefix-based filter parsers below so
    // "clear filter" doesn't get captured as a filter query.
    if matches!(t,
        "clear filter" | "clear cards filter" | "clear the filter"
        | "show all cards" | "show all" | "reset filter"
        | "remove filter" | "unfilter cards" | "unfilter"
    ) {
        return Intent::ClearCardsFilter;
    }
    // v0.1.323: apply a filter to the cards canvas.
    // v0.1.772: tag-specific filter prefixes are checked BEFORE the general
    // filter prefixes. "filter by tag <X>" must yield FilterByTag(X) (exact-tag
    // match); the general "filter by " arm would otherwise catch it first and
    // return FilterCards("tag X"), treating the tag name as fuzzy query text.
    // The tag prefixes are specific enough ("filter by tag ", "show cards
    // tagged ", ...) not to capture a non-tag filter phrase.
    let filter_tag_prefixes: &[&str] = &[
        "filter by tag ",
        "filter to tag ",
        "show tag ",
        "show only tag ",
        "show only ",
        "show cards tagged ",
        "show only cards tagged ",
        "tagged cards ",
    ];
    for &pfx in filter_tag_prefixes {
        if let Some(rest) = t.strip_prefix(pfx) {
            let rest = rest.trim();
            // Strip a leading "@" if present (some phrasings include it).
            let rest = rest.strip_prefix('@').unwrap_or(rest);
            if !rest.is_empty() {
                return Intent::FilterByTag(rest.to_string());
            }
        }
    }
    let filter_prefixes: &[&str] = &[
        "filter cards to ",
        "filter cards for ",
        "filter cards ",
        "filter to ",
        "filter by ",
        "filter for ",
        "search cards for ",
        "search cards ",
        "find cards ",
        "find cards with ",
        "find cards about ",
        "show cards about ",
        "show cards with ",
        "show cards matching ",
    ];
    for &pfx in filter_prefixes {
        if let Some(rest) = t.strip_prefix(pfx) {
            let rest = rest.trim();
            // v0.1.772: a less-specific prefix (e.g. "find cards ") can match
            // before a more-specific one ("find cards with "), leaving a leading
            // "with " connector — strip it so "find cards with rust async" →
            // FilterCards("rust async").
            let rest = rest.strip_prefix("with ").unwrap_or(rest).trim_start();
            if !rest.is_empty() {
                return Intent::FilterCards(rest.to_string());
            }
        }
    }
    // v0.1.324: untag intents come BEFORE tag intents so
    // "remove tag X" / "untag X" can't get captured by the
    // looser "tag X" prefix.
    let untag_prefixes: &[&str] = &[
        "remove tag ",
        "delete tag ",
        "untag this ",
        "untag ",
    ];
    for &pfx in untag_prefixes {
        if let Some(rest) = t.strip_prefix(pfx) {
            let rest = rest.trim();
            let rest = rest.strip_prefix('@').unwrap_or(rest);
            if !rest.is_empty() {
                return Intent::UntagCard(rest.to_string());
            }
        }
    }
    // v0.1.324: tag the currently-previewed card.
    let tag_prefixes: &[&str] = &[
        "tag this as ",
        "tag this ",
        "tag it as ",
        "tag it ",
        "add tag ",
    ];
    for &pfx in tag_prefixes {
        if let Some(rest) = t.strip_prefix(pfx) {
            let rest = rest.trim();
            let rest = rest.strip_prefix('@').unwrap_or(rest);
            if !rest.is_empty() {
                return Intent::TagCard(rest.to_string());
            }
        }
    }
    // Load workspace.
    if let Some(name) = t.strip_prefix("load workspace ")
        .or_else(|| t.strip_prefix("switch to workspace "))
        .or_else(|| t.strip_prefix("open workspace "))
    {
        let name = name.trim();
        if !name.is_empty() {
            return Intent::WorkspaceLoad(name.to_string());
        }
    }
    // "switch to X workspace" / "open X workspace" — name BEFORE
    // the trailing "workspace" word.
    if let Some(without_suffix) = t.strip_suffix(" workspace") {
        let candidate = without_suffix
            .strip_prefix("switch to ")
            .or_else(|| without_suffix.strip_prefix("open "))
            .or_else(|| without_suffix.strip_prefix("load "));
        if let Some(name) = candidate {
            let name = name.trim();
            if !name.is_empty() {
                return Intent::WorkspaceLoad(name.to_string());
            }
        }
    }
    // v0.1.301: delete a named workspace.
    if let Some(name) = t.strip_prefix("delete workspace ")
        .or_else(|| t.strip_prefix("remove workspace "))
        .or_else(|| t.strip_prefix("forget workspace "))
        .or_else(|| t.strip_prefix("destroy workspace "))
    {
        let name = name.trim();
        if !name.is_empty() && name != "default" {
            return Intent::WorkspaceDelete(name.to_string());
        }
    }
    // "delete <X> workspace" form.
    if let Some(without_suffix) = t.strip_suffix(" workspace") {
        let candidate = without_suffix
            .strip_prefix("delete ")
            .or_else(|| without_suffix.strip_prefix("remove "))
            .or_else(|| without_suffix.strip_prefix("forget "));
        if let Some(name) = candidate {
            let name = name.trim();
            if !name.is_empty() && name != "default" {
                return Intent::WorkspaceDelete(name.to_string());
            }
        }
    }
    // v0.1.310: workspace rename.
    // "rename workspace <old> to <new>" / "rename <old> workspace to <new>"
    // / "rename <old> to <new>" (less specific; checked after "rename
    // workspace" so we don't catch "rename file" or other unrelated
    // verbs).
    if let Some(rest) = t.strip_prefix("rename workspace ") {
        if let Some((old, new)) = rest.split_once(" to ") {
            let old = old.trim();
            let new = new.trim();
            if !old.is_empty() && !new.is_empty() {
                return Intent::WorkspaceRename(format!("{}\x1F{}", old, new));
            }
        }
    }
    // "rename <old> workspace to <new>"
    if let Some(rest) = t.strip_prefix("rename ") {
        if let Some(idx) = rest.find(" workspace to ") {
            let old = rest[..idx].trim();
            let new = rest[idx + " workspace to ".len()..].trim();
            if !old.is_empty() && !new.is_empty() {
                return Intent::WorkspaceRename(format!("{}\x1F{}", old, new));
            }
        }
    }
    // v0.1.311: workspace inspect.
    // "show workspace X" / "describe workspace X" / "inspect workspace X".
    if let Some(name) = t.strip_prefix("show workspace ")
        .or_else(|| t.strip_prefix("describe workspace "))
        .or_else(|| t.strip_prefix("inspect workspace "))
        .or_else(|| t.strip_prefix("what's in workspace "))
        .or_else(|| t.strip_prefix("what is in workspace "))
    {
        let name = name.trim();
        if !name.is_empty() {
            return Intent::WorkspaceInspect(name.to_string());
        }
    }
    // "what's in X workspace" / "what is in X workspace"
    if let Some(without_suffix) = t.strip_suffix(" workspace") {
        let candidate = without_suffix
            .strip_prefix("what's in ")
            .or_else(|| without_suffix.strip_prefix("what is in "))
            .or_else(|| without_suffix.strip_prefix("show "))
            .or_else(|| without_suffix.strip_prefix("describe "))
            .or_else(|| without_suffix.strip_prefix("inspect "));
        if let Some(name) = candidate {
            let name = name.trim();
            if !name.is_empty() {
                return Intent::WorkspaceInspect(name.to_string());
            }
        }
    }
    // v0.1.314: current/active workspace status.
    if matches!(t,
        "what workspace am i in" | "what workspace am i on"
        | "current workspace" | "active workspace"
        | "which workspace" | "which workspace am i in"
        | "what's my workspace" | "what is my workspace"
        | "my current workspace" | "show current workspace"
    ) {
        return Intent::WorkspaceCurrent;
    }
    // v0.1.313: workspace copy.
    // "copy workspace X to Y" / "fork workspace X to Y"
    if let Some(rest) = t.strip_prefix("copy workspace ")
        .or_else(|| t.strip_prefix("fork workspace "))
        .or_else(|| t.strip_prefix("duplicate workspace "))
        .or_else(|| t.strip_prefix("clone workspace "))
    {
        if let Some((src, dst)) = rest.split_once(" to ")
            .or_else(|| rest.split_once(" as "))
        {
            let src = src.trim();
            let dst = dst.trim();
            if !src.is_empty() && !dst.is_empty() {
                return Intent::WorkspaceCopy(format!("{}\x1F{}", src, dst));
            }
        }
    }
    // "copy X workspace to Y" / "fork X workspace to Y"
    if let Some(rest) = t.strip_prefix("copy ")
        .or_else(|| t.strip_prefix("fork "))
        .or_else(|| t.strip_prefix("duplicate "))
        .or_else(|| t.strip_prefix("clone "))
    {
        if let Some(idx) = rest.find(" workspace to ") {
            let src = rest[..idx].trim();
            let dst = rest[idx + " workspace to ".len()..].trim();
            if !src.is_empty() && !dst.is_empty() {
                return Intent::WorkspaceCopy(format!("{}\x1F{}", src, dst));
            }
        }
        if let Some(idx) = rest.find(" workspace as ") {
            let src = rest[..idx].trim();
            let dst = rest[idx + " workspace as ".len()..].trim();
            if !src.is_empty() && !dst.is_empty() {
                return Intent::WorkspaceCopy(format!("{}\x1F{}", src, dst));
            }
        }
    }

    // Scrolling. v0.1.110: more natural phrasings.
    if matches!(t,
        "scroll down" | "down" | "scroll a bit" | "scroll a bit down"
        | "scroll a little" | "scroll a little down" | "go down"
        | "move down" | "a bit down" | "a little down"
        | "scroll down a bit" | "scroll down a little"
    ) { return Intent::Scroll(60); }
    if matches!(t,
        "scroll up" | "up" | "scroll a bit up" | "scroll a little up"
        | "go up" | "move up" | "a bit up" | "a little up"
        | "scroll up a bit" | "scroll up a little"
    ) { return Intent::Scroll(-60); }
    if matches!(t,
        "page down" | "next page down" | "scroll page down"
        | "scroll a page down" | "scroll page" | "scroll one page down"
        | "down a page" | "down one page" | "page after this"
    ) { return Intent::ScrollPage(1); }
    if matches!(t,
        "page up" | "previous page up" | "scroll page up"
        | "scroll a page up" | "scroll one page up"
        | "up a page" | "up one page" | "page before this"
    ) { return Intent::ScrollPage(-1); }
    if matches!(t,
        "go to top" | "top" | "scroll to top" | "beginning"
        | "go to the top" | "to the top" | "to top" | "the top"
        | "all the way up" | "scroll all the way up" | "page top"
        | "very top" | "to the beginning" | "the beginning"
    ) { return Intent::GoTop; }
    if matches!(t,
        "go to bottom" | "bottom" | "scroll to bottom" | "end"
        | "go to the bottom" | "to the bottom" | "to bottom" | "the bottom"
        | "all the way down" | "scroll all the way down" | "page bottom"
        | "very bottom" | "to the end" | "the end"
    ) { return Intent::GoBottom; }

    // Find on page
    if let Some(q) = t.strip_prefix("find ").or_else(|| t.strip_prefix("search for ")) {
        if let Some(q2) = q.strip_prefix("on page ") {
            return Intent::FindInPage(q2.to_string());
        }
        // "find <single-word>" likely means find-on-page (browser context).
        // "find <multi-word>" likely means a shell command ("find files containing
        // foo" / "find the second one"). Let it fall through to terminal/cmd-index
        // scoring rather than committing to browser FindInPage.
        let q_trim = q.trim_end_matches(|c: char| c == '.' || c == '!' || c == '?');
        let word_count = q_trim.split_whitespace().count();
        if !q.is_empty() && q.len() < 100 && word_count <= 1 {
            return Intent::FindInPage(q.to_string());
        }
        // Multi-word find — fall through. parse_candidates will surface FindInPage
        // as a lower-confidence option AND let cmd-index/entity paths compete.
    }

    // Tab switching
    if matches!(t,
        "switch to terminal" | "switch to the terminal" | "open terminal" | "open the terminal" | "open terminal tab" |
        "open the terminal tab" | "show terminal" | "terminal" | "terminal tab" |
        "go to terminal" | "go to the terminal" | "to terminal" | "to the terminal"
    ) {
        return Intent::SwitchTab("term".into());
    }
    if matches!(t,
        "switch to browser" | "switch to the browser" | "open browser" | "open the browser" | "open browser tab" |
        "open the browser tab" | "show browser" | "browse" | "browser" | "browser tab" |
        "go to browse" | "go to browser" | "go to the browser" | "back to browser" |
        "back to the browser" | "to browser" | "to the browser"
    ) {
        return Intent::SwitchTab("browse".into());
    }

    // Settings & homepage
    if matches!(t, "settings" | "open settings" | "show settings" | "preferences") {
        return Intent::OpenSettings;
    }
    if matches!(t, "home" | "homepage" | "go home" | "show homepage" | "main page") {
        return Intent::OpenHomepage;
    }

    // Palette / panel openers
    if matches!(t, "show history" | "history" | "browsing history" | "show archive" | "open archive" | "show recent") {
        return Intent::OpenPalette("archive".into());
    }
    if matches!(t, "show links" | "links" | "show all links" | "list links") {
        return Intent::OpenPalette("links".into());
    }
    if matches!(t, "show menu" | "menu" | "command palette" | "open menu" | "open commands") {
        return Intent::OpenPalette("actions".into());
    }
    if matches!(t, "show site map" | "site map" | "show sitemap" | "page outline" | "outline") {
        return Intent::OpenPalette("sitemap".into());
    }
    if matches!(t, "show headings" | "headings" | "table of contents") {
        return Intent::OpenPalette("headings".into());
    }

    // "Open <site>" — try to map common names to URLs.
    // v0.1.110: more natural prefixes.
    if let Some(rest) = t.strip_prefix("open ")
        .or_else(|| t.strip_prefix("go to "))
        .or_else(|| t.strip_prefix("visit "))
        .or_else(|| t.strip_prefix("navigate to "))
        .or_else(|| t.strip_prefix("take me to "))
        .or_else(|| t.strip_prefix("show me "))
        .or_else(|| t.strip_prefix("bring up "))
        .or_else(|| t.strip_prefix("pull up "))
        .or_else(|| t.strip_prefix("load "))
        .or_else(|| t.strip_prefix("browse to "))
        .or_else(|| t.strip_prefix("head to "))
        .or_else(|| t.strip_prefix("head over to "))
    {
        let rest = rest.trim();
        if let Some(url) = resolve_known_site(rest) {
            return Intent::Navigate(url);
        }
        // Try URL reconstruction on the param
        if let Some(url) = reconstruct_url(rest) {
            return Intent::Navigate(url);
        }
        // Direct URL detection
        if rest.contains('.') && !rest.contains(' ') {
            return Intent::Navigate(rest.to_string());
        }
        // Otherwise treat as search — but ONLY if `rest` looks like a real
        // query, not a narration tail. v0.1.726 (corpus fuzz): "open this up
        // in ghidorah real quick and see what we have" was captured whole as a
        // web search. Reject tails that begin with a narration pronoun
        // ("this/that/it up …") or run on too long to be a search query.
        if open_tail_is_query(rest) {
            return Intent::SearchWeb(rest.to_string());
        }
    }

    // Search the web. v0.1.110: more natural phrasings.
    if let Some(q) = t.strip_prefix("search ")
        .or_else(|| t.strip_prefix("search the web for "))
        .or_else(|| t.strip_prefix("search for "))
        .or_else(|| t.strip_prefix("look up "))
        .or_else(|| t.strip_prefix("look for "))
        .or_else(|| t.strip_prefix("google "))
        .or_else(|| t.strip_prefix("google for "))
        .or_else(|| t.strip_prefix("find me "))
        .or_else(|| t.strip_prefix("query "))
        .or_else(|| t.strip_prefix("web search "))
    {
        let q = q.trim_start_matches("for ").trim();
        if !q.is_empty() { return Intent::SearchWeb(q.to_string()); }
    }

    // Terminal commands. v0.1.658: guard against PROSE tails. The
    // normalizer strips hedges ("just"/"i'm going to"/"basically"), so
    // narration like "basically run this on the host, effectively" becomes
    // "run this on the host, effectively" and used to route to a literal
    // command. Require the tail to LOOK like a command (short, no narration
    // markers) — otherwise it's thinking-aloud, leave it for Freeform.
    if let Some(rest) = t.strip_prefix("in terminal ").or_else(|| t.strip_prefix("terminal ")).or_else(|| t.strip_prefix("run ")) {
        let rest = rest.trim_start_matches("type ").trim_start_matches("run ").trim();
        // "run it again" / "run that again" → rerun, not a literal command.
        if rest == "it again" || rest == "that again" || rest == "again" {
            return Intent::RerunLast;
        }
        if !rest.is_empty() && looks_like_command(rest) {
            return Intent::TerminalCommand(rest.to_string());
        }
    }

    // Man pages — direct
    if let Some(cmd) = t.strip_prefix("man page for ").or_else(|| t.strip_prefix("show man page for ")).or_else(|| t.strip_prefix("show manual for ")).or_else(|| t.strip_prefix("manual for ")) {
        let cmd = cmd.trim();
        if !cmd.is_empty() { return Intent::TerminalCommand(format!("man {}", cmd)); }
    }

    // "type X in terminal"
    if let Some(rest) = t.strip_prefix("type ") {
        let rest = rest.trim_end_matches(" in terminal").trim_end_matches(" in the terminal");
        if !rest.is_empty() && looks_like_command(rest) { return Intent::TerminalCommand(rest.to_string()); }
    }

    // System info: "how many days until X" / "how many days since X"
    if let Some(rest) = t.strip_prefix("how many days until ").or_else(|| t.strip_prefix("days until ")) {
        let rest = rest.trim();
        let answer = system_info::days_until(rest);
        return Intent::SystemInfo("days_until".into(), answer);
    }
    if let Some(rest) = t.strip_prefix("how many days since ").or_else(|| t.strip_prefix("days since ")) {
        let rest = rest.trim();
        let answer = system_info::days_since(rest);
        return Intent::SystemInfo("days_since".into(), answer);
    }
    if matches!(t, "what time is it" | "current time" | "the time" | "time now") {
        return Intent::SystemInfo("time".into(), system_info::current_time());
    }
    if matches!(t, "what's today's date" | "what is today's date" | "today's date" | "what date is it" | "current date") {
        return Intent::SystemInfo("date".into(), system_info::current_date());
    }
    if matches!(t, "battery" | "battery level" | "how much battery" | "battery status") {
        return Intent::SystemInfo("battery".into(), system_info::battery());
    }
    if matches!(t, "disk space" | "free disk space" | "disk usage" | "how much disk space") {
        return Intent::SystemInfo("disk".into(), system_info::disk_space());
    }
    if matches!(t, "memory" | "ram usage" | "memory usage" | "how much memory") {
        return Intent::SystemInfo("memory".into(), system_info::memory_usage());
    }

    // Email: "open email to bob with hello world" / "email this page to alice"
    if let Some(rest) = t.strip_prefix("email this page to ").or_else(|| t.strip_prefix("send this page to ")) {
        let rest = rest.trim().trim_end_matches('.');
        return Intent::EmailCompose(rest.to_string(), "[Current page]".into());
    }
    if let Some(rest) = t.strip_prefix("compose email to ").or_else(|| t.strip_prefix("email to ")).or_else(|| t.strip_prefix("write email to ")) {
        let rest = rest.trim();
        // Split on "with body" / "saying" / "about"
        for sep in &[" saying ", " with body ", " body ", " about ", " regarding "] {
            if let Some(i) = rest.find(sep) {
                let to = &rest[..i].trim();
                let body = &rest[i + sep.len()..].trim();
                return Intent::EmailCompose(to.to_string(), body.to_string());
            }
        }
        return Intent::EmailCompose(rest.to_string(), String::new());
    }

    // File append: "add this text to my notes" / "append X to file".
    // v0.1.657: only fire on a KNOWN notes-like target or an explicit path.
    // The old fallback turned ANY "add X to Y" into a write to ~/Y.txt —
    // which misfired on narration like "add those to the wrong place"
    // (corpus false-positive). Arbitrary targets now fall through.
    if let Some(rest) = t.strip_prefix("add ").or_else(|| t.strip_prefix("append ")) {
        if let Some(i) = rest.rfind(" to ") {
            let text = rest[..i].trim();
            let target = rest[i + 4..].trim();
            let path = match target {
                "notes" | "my notes" | "notes file" => Some("~/notes.txt".to_string()),
                "todo" | "my todo" | "todo list" => Some("~/todo.txt".to_string()),
                "journal" | "my journal" | "diary" => Some("~/journal.txt".to_string()),
                _ if target.starts_with('/') || target.starts_with('~') => Some(target.to_string()),
                // a bare "<name> file" / "<name>.txt" is plausibly a file;
                // anything else (e.g. "the wrong place") is NOT — fall through.
                _ if target.ends_with(" file") || target.ends_with(".txt") =>
                    Some(format!("~/{}.txt", target.trim_end_matches(" file").trim_end_matches(".txt").replace(' ', "_"))),
                _ => None,
            };
            if let (Some(path), false) = (path, text.is_empty()) {
                return Intent::FileAppend(path, text.to_string());
            }
        }
    }

    // Clipboard ops
    if matches!(t, "copy url" | "copy the url" | "copy this url" | "copy address" | "copy link") {
        return Intent::Copy("url".into());
    }
    if matches!(t, "copy title" | "copy the title" | "copy page title") {
        return Intent::Copy("title".into());
    }
    if matches!(t, "copy selection" | "copy this" | "copy text" | "copy the text" | "copy that") {
        return Intent::Copy("selection".into());
    }
    if matches!(t, "copy page" | "copy entire page" | "copy all" | "copy everything") {
        return Intent::Copy("page_text".into());
    }
    if matches!(t, "copy terminal" | "copy terminal output" | "copy last output" | "copy output") {
        return Intent::Copy("last_terminal_output".into());
    }
    if matches!(t, "copy last command" | "copy command" | "copy that command") {
        return Intent::Copy("last_command".into());
    }
    // Paste
    if matches!(t, "paste" | "paste here" | "paste it") {
        return Intent::Paste("auto".into());
    }
    if matches!(t, "paste in url" | "paste url" | "paste in url bar" | "paste to url" | "paste to url bar") {
        return Intent::Paste("url".into());
    }
    if matches!(t, "paste in terminal" | "paste to terminal") {
        return Intent::Paste("term".into());
    }

    // Rerun last
    if matches!(t, "rerun" | "rerun last" | "rerun last command" | "do that again" | "run that again" | "again" | "repeat" | "repeat that") {
        return Intent::RerunLast;
    }

    // Toggle settings
    if let Some(rest) = t.strip_prefix("turn off ").or_else(|| t.strip_prefix("disable ")).or_else(|| t.strip_prefix("turn on ")).or_else(|| t.strip_prefix("enable ")) {
        let target = match rest.trim() {
            "ads" | "ad blocker" | "ad filter" | "ad blocking" | "ad filtering" => "ads",
            "monospace" | "mono font" | "mono" => "monospace",
            "compatibility" | "chrome ua" | "chrome user agent" | "compat" => "compat_ua",
            _ => "",
        };
        if !target.is_empty() {
            return Intent::Toggle(target.into());
        }
    }

    // "Find this term: X" / "find anywhere X" — opens multi-action chooser
    for prefix in &[
        "find this term ", "find this term: ", "find this ", "find anywhere ",
        "search anywhere ", "search everywhere for ", "look up term ", "look this up: ",
    ] {
        if let Some(q) = t.strip_prefix(prefix) {
            let q = q.trim().trim_start_matches(": ").trim_start_matches(":").trim();
            if !q.is_empty() && q.len() < 200 {
                return Intent::FindAnywhere(q.to_string());
            }
        }
    }

    // Workflow recorder voice triggers (v0.1.115).
    //
    // "start recording <name>" / "record workflow <name>" / "begin
    //  recording <name>" → start a new workflow
    // "stop recording" / "save workflow" / "finish recording" / "done
    //  recording" → finish + save
    // "run workflow <name>" / "play workflow <name>" / "replay <name>"
    //  → playback
    //
    // Name accepts any non-empty trailing words; spaces become part of
    // the workflow name (saved file is <name with spaces>.toml).
    for prefix in &[
        "start recording workflow ",
        "start recording ",
        "record workflow ",
        "begin recording ",
        "record this as ",
        "save this as workflow ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name = rest.trim();
            if !name.is_empty() && name.len() < 80 {
                return Intent::WorkflowStartRecording(name.to_string());
            }
        }
    }
    if matches!(t,
        "stop recording" | "stop recording workflow"
        | "save workflow" | "finish recording" | "done recording"
        | "end recording" | "save the workflow" | "save this workflow"
    ) {
        return Intent::WorkflowStopRecording;
    }
    for prefix in &[
        "run workflow ",
        "play workflow ",
        "replay workflow ",
        "play back workflow ",
        "playback workflow ",
        "replay ",
        "execute workflow ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name = rest.trim();
            if !name.is_empty() && name.len() < 80 {
                return Intent::WorkflowPlayback(name.to_string());
            }
        }
    }

    // History search: "find <X> in history" or "search history for <X>"
    if let Some(q) = t.strip_prefix("search history for ").or_else(|| t.strip_prefix("history search ")) {
        return Intent::HistorySearch(q.trim().to_string());
    }
    if let Some(rest) = t.strip_suffix(" in history") {
        if let Some(q) = rest.strip_prefix("find ").or_else(|| rest.strip_prefix("look for ")) {
            return Intent::HistorySearch(q.trim().to_string());
        }
    }

    // v0.1.596: positional click — "click the third link", "the last
    // button". Checked BEFORE the text-match click below so an ordinal
    // phrase routes here; a non-ordinal ("click the login button")
    // returns None and falls through to text-match ClickLink unchanged.
    if let Some((ordinal, kind)) = parse_positional_click(t) {
        return Intent::ClickPositional { ordinal, kind };
    }

    // Click link by name: "click on <X>" / "click <X>" / "open the <X> link"
    for prefix in &["click on ", "click the ", "click ", "tap on ", "tap the ", "tap ", "follow the ", "follow link ", "follow "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rest = rest.trim().trim_start_matches("link ").trim_start_matches("button ").trim();
            if !rest.is_empty() && rest.len() < 80 {
                // "click the X link" — strip trailing "link"
                let rest = rest.trim_end_matches(" link").trim_end_matches(" button").trim();
                return Intent::ClickLink(rest.to_string());
            }
        }
    }

    // Scroll to heading: "go to <heading>" / "jump to <heading>" / "scroll to <heading>"
    for prefix in &["scroll to ", "jump to section ", "jump to heading ", "go to section ", "go to heading "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Intent::ScrollToHeading(rest.to_string());
            }
        }
    }

    // Dictation: "type <words>" / "say <words>" — handled differently than terminal command
    // Already handled by "type X in terminal" earlier; if not, treat as dictation
    if let Some(rest) = t.strip_prefix("dictate ") {
        return Intent::Dictate(rest.trim().to_string());
    }
    // "write <words>" → dictation, but NOT when it reads as narration about
    // writing ("write a function for this", "write some code that…").
    // v0.1.726 (corpus fuzz): "i need to write a function for this" was
    // captured as a Dictate. A real dictation command names content to type,
    // not a plan to write code.
    if let Some(rest) = t.strip_prefix("write ") {
        let r = rest.trim();
        let lower = r.to_lowercase();
        let narration = (lower.starts_with("a ") || lower.starts_with("an ")
            || lower.starts_with("some ") || lower.starts_with("the "))
            && (lower.contains(" for ") || lower.contains(" that ")
                || lower.contains(" to ") || lower.ends_with(" this")
                || lower.ends_with(" code") || lower.contains(" function"));
        if !narration {
            return Intent::Dictate(r.to_string());
        }
    }

    // Direct URL/domain detection
    if t.contains('.') && !t.contains(' ') {
        return Intent::Navigate(t.to_string());
    }

    // No prefix matched — try LLM fallback if available, else freeform
    if system_info::has_ollama() {
        return Intent::LlmFallback(text.trim().to_string());
    }
    Intent::Freeform(text.trim().to_string())
}

/// Try to rebuild a URL from spelled-out speech.
/// Returns Some(url) if the text looks like spelled-out URL words.
///
/// Examples this should handle:
///   "ay el ex dot es haitch slash asahi dee ee vee" -> "alx.sh/asahi-dev"
///   "open dee ee vee" -> None (just "open dev", caller handles)
///   "g h dot com" -> "gh.com"
///   "open google dot com" -> "google.com"
/// Try to rebuild a URL from spelled-out speech.
/// Returns Some(url) if the text looks unambiguously like a spelled-out URL.
///
/// Strategy:
/// 1. Detect URL-ish input (must be nearly all letter-words + "dot"/"slash" markers)
/// 2. Reject if too many real English words that aren't letter spellings
/// 3. Build URL from tokens; bail at first ambiguity
fn reconstruct_url(text: &str) -> Option<String> {
    // Length sanity
    if text.is_empty() || text.len() > 300 { return None; }

    let lower = text.to_lowercase();
    let mut s = lower.trim().to_string();
    for prefix in &["open ", "go to ", "visit ", "navigate to "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
            break;
        }
    }

    // Word-boundary check for url markers — must have whitespace or end around it
    // to avoid matching inside ordinary sentences like "she got an idea, dot com..."
    fn has_word(s: &str, word: &str) -> bool {
        let mut start = 0;
        while let Some(idx) = s[start..].find(word) {
            let abs = start + idx;
            let before_ok = abs == 0
                || s.as_bytes().get(abs - 1).map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true);
            let after = abs + word.len();
            let after_ok = after >= s.len()
                || s.as_bytes().get(after).map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true);
            if before_ok && after_ok { return true; }
            start = abs + 1;
        }
        false
    }

    let has_url_marker = has_word(&s, "dot") || has_word(&s, "slash");
    if !has_url_marker { return None; }

    // Multi-word and single-word "letter-or-domain-particle" recognition.
    // Order matters: longer matches first.
    // Multi-word patterns are tried in `multi_match`.
    const TLDS: &[&str] = &[
        "com","org","net","io","sh","dev","co","ai","gov","edu","uk","ca","us",
        "xyz","app","dev","blog","info","me","tv","cc","rs",
    ];

    // Strict tokenization: split on whitespace, then preserve dots/slashes/colons as their own tokens
    let mut raw_tokens: Vec<String> = Vec::new();
    for word in s.split_whitespace() {
        // A whisper token might already contain "." (e.g. "google.com" if whisper recognized it directly)
        // Split internal punctuation out as separate tokens.
        let mut buf = String::new();
        for c in word.chars() {
            if c == '.' || c == '/' || c == ':' || c == '-' {
                if !buf.is_empty() { raw_tokens.push(std::mem::take(&mut buf)); }
                raw_tokens.push(c.to_string());
            } else if c.is_ascii_alphanumeric() || c == '_' {
                buf.push(c);
            } else {
                // unknown punctuation — flush and skip
                if !buf.is_empty() { raw_tokens.push(std::mem::take(&mut buf)); }
            }
        }
        if !buf.is_empty() { raw_tokens.push(buf); }
    }

    // Letter map (single-letter alternates only — not "you", "see", "are", "be", "a"
    // because those collide with real words). User can spell with full names where ambiguous.
    // The unambiguous spellings stay; ambiguous ones are off by default.
    let letter_map: &[(&str, char)] = &[
        ("ay", 'a'), ("eh", 'a'),
        ("bee", 'b'),
        ("cee", 'c'),
        ("dee", 'd'),
        ("ee", 'e'),
        ("ef", 'f'), ("eff", 'f'),
        ("gee", 'g'), ("jee", 'g'),
        ("aitch", 'h'), ("haitch", 'h'), ("hatch", 'h'),
        ("eye", 'i'),
        ("jay", 'j'),
        ("kay", 'k'),
        ("el", 'l'), ("ell", 'l'),
        ("em", 'm'),
        ("en", 'n'),
        ("oh", 'o'),
        ("pee", 'p'),
        ("cue", 'q'), ("queue", 'q'),
        ("ar", 'r'),
        ("es", 's'), ("ess", 's'),
        ("tee", 't'),
        ("yu", 'u'),
        ("vee", 'v'),
        ("dub", 'w'),
        ("ex", 'x'),
        ("why", 'y'),
        ("zee", 'z'), ("zed", 'z'),
    ];

    // Multi-word patterns: must come before single-word since they're greedier
    let multi_word: &[(&[&str], &str)] = &[
        (&["double", "you"], "w"),
        (&["dot", "com"], ".com"), (&["dot", "org"], ".org"),
        (&["dot", "net"], ".net"), (&["dot", "io"],  ".io"),
        (&["dot", "sh"],  ".sh"),  (&["dot", "dev"], ".dev"),
        (&["dot", "co"],  ".co"),  (&["dot", "ai"],  ".ai"),
        (&["dot", "gov"], ".gov"), (&["dot", "edu"], ".edu"),
        (&["dot", "uk"],  ".uk"),  (&["dot", "ca"],  ".ca"),
        (&["dot", "us"],  ".us"),  (&["dot", "xyz"], ".xyz"),
        (&["dot", "app"], ".app"), (&["dot", "blog"], ".blog"),
        (&["dot", "info"], ".info"), (&["dot", "me"], ".me"),
        (&["h", "t", "t", "p"], "http://"),
        (&["h", "t", "t", "p", "s"], "https://"),
        (&["aitch", "tee", "tee", "pee"], "http://"),
        (&["aitch", "tee", "tee", "pee", "es"], "https://"),
        (&["colon", "slash", "slash"], "://"),
    ];

    // Build output. Conservative: bail on any ambiguous token.
    let mut out = String::new();
    let mut i = 0;
    let mut accepted_any = false;
    while i < raw_tokens.len() {
        // Multi-word match first
        let mut matched = false;
        for (pattern, replacement) in multi_word {
            if i + pattern.len() <= raw_tokens.len()
                && pattern.iter().enumerate().all(|(k, w)| raw_tokens[i+k].eq_ignore_ascii_case(w)) {
                out.push_str(replacement);
                i += pattern.len();
                matched = true;
                accepted_any = true;
                break;
            }
        }
        if matched { continue; }

        let tk = raw_tokens[i].as_str();

        // Punctuation pass-through
        if tk == "." || tk == "/" || tk == ":" || tk == "-" {
            out.push_str(tk);
            i += 1;
            continue;
        }

        let lc = tk.to_ascii_lowercase();

        // "dot" alone (without TLD) → "."
        if lc == "dot" { out.push('.'); i += 1; accepted_any = true; continue; }
        // "slash" alone → "/"
        if lc == "slash" { out.push('/'); i += 1; accepted_any = true; continue; }
        // "dash"/"hyphen"/"minus" → "-"
        if lc == "dash" || lc == "hyphen" || lc == "minus" { out.push('-'); i += 1; accepted_any = true; continue; }
        if lc == "underscore" { out.push('_'); i += 1; accepted_any = true; continue; }
        // "colon" → ":"
        if lc == "colon" { out.push(':'); i += 1; accepted_any = true; continue; }

        // Letter spelling (unambiguous variants only)
        if let Some((_, ch)) = letter_map.iter().find(|(k, _)| *k == lc) {
            out.push(*ch);
            i += 1;
            accepted_any = true;
            continue;
        }

        // Whole domain word (alphanumeric, length >=2) — keep as-is
        // BUT only if it's a known-safe word: TLD, or follows a dot/slash/colon, or nothing yet
        if lc.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && lc.len() >= 2 {
            let last_is_sep = out.chars().last().map(|c| c == '.' || c == '/' || c == ':' || c == '-').unwrap_or(true);
            let is_tld = TLDS.contains(&lc.as_str());
            if last_is_sep || is_tld || out.is_empty() {
                out.push_str(&lc);
                i += 1;
                accepted_any = true;
                continue;
            }
            // Two consecutive non-separator words — that's a sentence, not a URL
            return None;
        }

        // Anything else — bail
        return None;
    }

    if !accepted_any { return None; }

    // Final shape check: must look like a domain (something.something or path)
    if !out.contains('.') && !out.contains('/') { return None; }
    if out.len() < 4 || out.len() > 200 { return None; }
    // Must have at least 2 characters before the first dot (no empty domain)
    if let Some(first_dot) = out.find('.') {
        if first_dot < 1 { return None; }
    }
    // Must have at least 2 characters after the last dot (a TLD)
    if let Some(last_dot) = out.rfind('.') {
        let after = &out[last_dot+1..];
        // Either TLD-style (just letters) or has a path
        let after_to_slash = after.split('/').next().unwrap_or(after);
        if after_to_slash.len() < 2 { return None; }
    }

    // Cleanup
    while out.contains("..") { out = out.replace("..", "."); }
    while out.contains("//") && !out.contains("://") { out = out.replace("//", "/"); }

    eprintln!("Voice URL reconstruct: {:?} -> {:?}", text, out);
    Some(out)
}

/// Map common site names to URLs
fn resolve_known_site(name: &str) -> Option<String> {
    let n = name.trim();
    let known: HashMap<&str, &str> = [
        ("hacker news", "https://news.ycombinator.com"),
        ("hackernews", "https://news.ycombinator.com"),
        ("hn", "https://news.ycombinator.com"),
        ("github", "https://github.com"),
        ("wikipedia", "https://wikipedia.org"),
        ("apple", "https://apple.com"),
        ("rust docs", "https://doc.rust-lang.org"),
        ("rust", "https://www.rust-lang.org"),
        ("crates", "https://crates.io"),
        ("crates io", "https://crates.io"),
        ("stack overflow", "https://stackoverflow.com"),
        ("stackoverflow", "https://stackoverflow.com"),
        ("reddit", "https://reddit.com"),
        ("youtube", "https://youtube.com"),
        ("twitter", "https://twitter.com"),
        ("x.com", "https://x.com"),
        ("google", "https://google.com"),
        ("duckduckgo", "https://duckduckgo.com"),
        ("ddg", "https://duckduckgo.com"),
        ("kagi", "https://kagi.com"),
        ("wikipedia.org", "https://wikipedia.org"),
        ("phoronix", "https://phoronix.com"),
        ("lobsters", "https://lobste.rs"),
        ("lobste.rs", "https://lobste.rs"),
    ].iter().cloned().collect();

    if let Some(url) = known.get(n) {
        return Some(url.to_string());
    }
    // Sub-paths: "github torvalds linux" -> github.com/torvalds/linux
    //
    // v0.1.380: only treat the prefix as a prefix when the rest
    // begins with a word boundary (whitespace) — otherwise
    // "apple.com" wrongly strips "apple" as the known-site key
    // and stitches "https://apple.com" + "/" + ".com" =
    // "https://apple.com/.com". The fix: the leftover after
    // stripping must start with ' ' (a real word break),
    // otherwise it's part of the same domain or a longer name
    // and should NOT be treated as a path segment.
    for (key, base) in &known {
        if let Some(rest) = n.strip_prefix(*key) {
            // Word-boundary check: the prefix only counts as a
            // prefix if followed by whitespace (or the input
            // is exactly the prefix, which the earlier `n` ==
            // key map lookup already handled).
            if !rest.starts_with(' ') {
                continue;
            }
            let path = rest.trim().replace(' ', "/");
            if !path.is_empty() {
                return Some(format!("{}/{}", base, path));
            }
        }
    }
    None
}


// ─── Multi-candidate scoring ─────────────────────────────────────────────
//
// Instead of returning a single best-guess intent, generate a ranked list of
// plausible intents with confidence scores. The popup can then show the top
// 2-3 as options if no single candidate dominates.

#[derive(Clone)]
pub struct Candidate {
    pub intent: Intent,
    /// 0.0..1.0 — relative confidence
    pub score: f32,
    /// Short human reason for this candidate, e.g. "starts with 'open'"
    pub reason: String,
    /// v0.1.358: marks candidates that will mutate state in
    /// ways the user might regret. The picker renders these
    /// with a distinct visual treatment (warning glyph, amber
    /// border). Examples: `git stash`, `git reset --soft HEAD~1`,
    /// `git commit --amend`. Read-only ops like `git diff`
    /// stay `destructive: false` regardless of how they're
    /// classified semantically — what matters here is whether
    /// undoing the op is easy.
    ///
    /// Default false; only the smart-git mutating verbs set
    /// this for now. Future destructive verbs (`git reset
    /// --hard`, `git push --force`, `git branch -D`) will set
    /// this AND a higher-friction confirmation tier.
    pub destructive: bool,
}

impl Candidate {
    /// Convenience: non-destructive Candidate (the common case).
    pub fn safe(intent: Intent, score: f32, reason: impl Into<String>) -> Self {
        Self { intent, score, reason: reason.into(), destructive: false }
    }
    /// Convenience: destructive Candidate.
    pub fn destructive(intent: Intent, score: f32, reason: impl Into<String>) -> Self {
        Self { intent, score, reason: reason.into(), destructive: true }
    }
}

/// Generate ranked candidates for the recognized text. Returns at least one
/// candidate (Unknown if everything fails), sorted descending by score.
///
/// `terminal_hints` is an optional list of executable names known to be on
/// the user's PATH (e.g. from `which` or shell history) — if the input
/// matches one, a TerminalCommand candidate gets a boost.
pub fn parse_candidates(text: &str, terminal_hints: &[String]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let t_raw = text.trim();
    if t_raw.is_empty() {
        return vec![Candidate { intent: Intent::Unknown, score: 0.0, reason: "empty input".into(),
 destructive: false,
        }];
    }

    // v0.1.149: strip floor-claim prefix here too so downstream
    // candidate generation (terminal commands, URL search, etc.)
    // works on the actual command, not the "<name>, here, " prefix.
    // The strip function is the same one parse() uses and also calls
    // speakers::note_active when the name matches a registered
    // speaker — calling it twice (here + inside parse()) is fine:
    // note_active with the same name is a no-op.
    //
    // We materialize to an owned String to keep lifetimes simple —
    // the alternative (a borrowed slice) is fine in principle but
    // gets fiddly when the branch chooses between two source slices
    // with different lifetimes.
    let lowered = t_raw.to_lowercase();
    let t_stripped = strip_floor_claim_prefix(&lowered);
    let t_raw_owned: String = if t_stripped == lowered {
        t_raw.to_string()
    } else {
        t_stripped
    };
    let t_raw: &str = &t_raw_owned;
    if t_raw.is_empty() {
        // Floor-claim with no command — the speaker side-effect already
        // happened inside strip_floor_claim_prefix, but no actionable
        // intent to return.
        return vec![Candidate { intent: Intent::Unknown, score: 0.0, reason: "floor-claim with no command".into(),
 destructive: false,
        }];
    }

    // Step 1: get the primary parse (heuristic-rule winner)
    let primary = parse(t_raw);
    let primary_score = match primary {
        Intent::Unknown => 0.0,
        Intent::Freeform(_) => 0.4,
        Intent::SearchWeb(_) => 0.5,
        Intent::Navigate(_) => 0.7,
        Intent::LlmFallback(_) => 0.3,
        _ => 0.85, // structured intents (GoBack, Reload, Toggle, etc)
    };
    if !matches!(primary, Intent::Unknown) {
        let reason = format!("primary parse: {}", primary.describe());
        out.push(Candidate { intent: primary.clone(), score: primary_score, reason, destructive: false });
    }

    // Step 2: terminal command candidate
    // If the input (stripped) matches an executable name, add it as a strong candidate.
    let t_lower = t_raw.to_lowercase();
    let t_for_term = t_lower
        .trim_start_matches("run ")
        .trim_start_matches("execute ")
        .trim_start_matches("type ")
        .trim();
    let first_word = t_for_term.split_whitespace().next().unwrap_or("");
    if !first_word.is_empty() {
        let mut term_score = 0.0f32;
        // Strong boost if first word is a known executable
        if terminal_hints.iter().any(|h| h.eq_ignore_ascii_case(first_word)) {
            term_score = 0.85;
        }
        // Built-in common command list — fallback when terminal_hints is empty
        let common_commands = [
            "ls", "cd", "pwd", "cat", "less", "more", "head", "tail", "grep", "find",
            "fastfetch", "neofetch", "htop", "top", "btop", "ps", "kill", "df", "du",
            "git", "vim", "nvim", "helix", "hx", "nano", "emacs", "tmux", "screen",
            "ssh", "scp", "rsync", "curl", "wget", "ping", "traceroute",
            "make", "cargo", "npm", "yarn", "pnpm", "python", "python3", "node",
            "docker", "podman", "kubectl", "systemctl", "journalctl",
            "echo", "printf", "sed", "awk", "sort", "uniq", "wc", "tr",
            "tar", "gzip", "gunzip", "zip", "unzip",
            "ip", "netstat", "ss", "lsof", "ifconfig",
            "nano", "ed", "ex", "fish", "bash", "zsh", "sh", "exit",
            "fzf", "ripgrep", "rg", "fd", "bat", "exa", "eza",
        ];
        if term_score == 0.0 && common_commands.contains(&first_word) {
            term_score = 0.75;
        }
        // If the command exists on disk (cheap check), boost
        if term_score == 0.0 && first_word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && first_word.len() >= 2 {
            // Optional cheap PATH check
            if executable_on_path(first_word) {
                term_score = 0.7;
            }
        }
        if term_score > 0.0 {
            out.push(Candidate {
                intent: Intent::TerminalCommand(t_for_term.to_string()),
                score: term_score,
                reason: format!("'{}' looks like a terminal command", first_word),
                destructive: false,
            });
        }
    }

    // Step 3: search-the-web candidate (always available as fallback for ambiguous text)
    // Score depends on whether other strong candidates already exist
    let search_score = if out.iter().any(|c| c.score > 0.7) { 0.35 } else { 0.55 };
    let already_search = out.iter().any(|c| matches!(c.intent, Intent::SearchWeb(_)));
    // Look for "domain-shaped" content: dot in middle of token, not just trailing punctuation
    let stripped_for_dot = t_raw.trim_end_matches(|c: char| c == '.' || c == '!' || c == '?' || c == ',');
    let looks_like_domain = stripped_for_dot.contains('.')
        && !stripped_for_dot.contains(' ');
    if !already_search && !looks_like_domain {
        out.push(Candidate {
            intent: Intent::SearchWeb(t_raw.to_string()),
            score: search_score,
            reason: "search the web".into(),
        destructive: false,
        });
    }

    // Step 4: archive-search candidate (FindAnywhere with archive scope)
    // A user might want to search their own archive. Score lower than web search
    // unless input mentions "archive", "saved", "history".
    let mentions_archive = ["archive", "saved", "history", "earlier", "yesterday", "last week"]
        .iter().any(|w| t_lower.contains(w));
    if mentions_archive {
        out.push(Candidate {
            intent: Intent::FindAnywhere(t_raw.to_string()),
            score: 0.65,
            reason: "search archive (history/saved keywords)".into(),
        destructive: false,
        });
    }

    // Step 5: navigation if the input looks like a domain
    let already_navigate = out.iter().any(|c| matches!(c.intent, Intent::Navigate(_)));
    if !already_navigate && t_raw.contains('.') && !t_raw.contains(' ') && t_raw.len() < 200 {
        out.push(Candidate {
            intent: Intent::Navigate(t_raw.to_string()),
            score: 0.8,
            reason: "looks like a domain".into(),
        destructive: false,
        });
    }

    // Sort descending
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Dedupe by intent kind+param
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    out.retain(|c| {
        let key = format!("{:?}", c.intent.to_qml_string());
        seen.insert(key)
    });

    if out.is_empty() {
        out.push(Candidate { intent: Intent::Unknown, score: 0.0, reason: "no patterns matched".into(),
 destructive: false,
        });
    }
    out.truncate(5);
    out
}

/// Cheap PATH check — does this look like an executable that exists?
fn executable_on_path(name: &str) -> bool {
    use std::path::Path;
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let p = Path::new(dir).join(name);
            if p.exists() { return true; }
        }
    }
    false
}

/// Format candidate list for QML.
/// Format per line: "kind\x1Fdescription\x1Fparam1\x1Fparam2\x1Fscore\x1Freason"
pub fn candidates_to_qml(cands: &[Candidate]) -> String {
    cands.iter().map(|c| {
        // to_qml_string returns "kind\x1Fdesc\x1Fp1\x1Fp2".
        // v0.1.358 extends this to "kind\x1Fdesc\x1Fp1\x1Fp2\x1Fscore\x1Freason\x1Fdestructive".
        // QML's parseCandidates path reads the extra field with a
        // default of "0" when missing, so older callers still work.
        let base = c.intent.to_qml_string();
        format!(
            "{}\x1F{:.2}\x1F{}\x1F{}",
            base,
            c.score,
            c.reason,
            if c.destructive { "1" } else { "0" },
        )
    }).collect::<Vec<_>>().join("\n")
}

// ── v0.1.149 unit tests for floor-claim prefix ──
//
// The strip function has a side effect (calls speakers::note_active when
// the name matches). We DON'T test the side effect here because the
// speakers module reads from a global registry that's file-backed and
// is awkward to mock without a substantial refactor. Tests instead
// confirm the textual transformation (prefix stripped vs left intact).
//
// For end-to-end speaker-side-effect testing, use the voice_cases
// harness with an .expect file that asserts on the post-claim speaker.

/// v0.1.277: capitalize the first character of a string. Used to
/// pretty-print speaker names extracted from voice input. The
/// audit-log filter itself is case-insensitive, so this is purely
/// cosmetic for the "Show audit log for X" description.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => {
            let mut out: String = c.to_uppercase().collect();
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod floor_claim_tests {
    use super::strip_floor_claim_prefix;

    // Note: strip_floor_claim_prefix consults the speaker registry to
    // decide whether to strip. We can't inject a test registry easily,
    // so we test the negative cases (which don't consult speakers in
    // a way that depends on registration) and rely on integration tests
    // for the positive cases. The functions tested below early-return
    // BEFORE the registry lookup, so they are deterministic.

    #[test]
    fn no_strip_when_text_too_short() {
        assert_eq!(strip_floor_claim_prefix("kessler"), "kessler");
        assert_eq!(strip_floor_claim_prefix(""), "");
    }

    #[test]
    fn no_strip_when_second_token_not_here() {
        assert_eq!(
            strip_floor_claim_prefix("kessler, please run dust"),
            "kessler, please run dust"
        );
        assert_eq!(
            strip_floor_claim_prefix("find the word here in this doc"),
            "find the word here in this doc"
        );
    }

    #[test]
    fn no_strip_when_name_unknown() {
        // Assuming "xyznobody" is not a registered speaker in any test
        // environment. If your registry happens to contain it the test
        // will spuriously fail — file an issue, that's a real namespace
        // collision.
        assert_eq!(
            strip_floor_claim_prefix("xyznobody here run dust"),
            "xyznobody here run dust"
        );
        assert_eq!(
            strip_floor_claim_prefix("xyznobody, here, run dust"),
            "xyznobody, here, run dust"
        );
    }

    // The positive cases below WILL only work if a matching speaker is
    // registered in the live registry. We can't cleanly mock that,
    // so leave them as #[ignore] documentation tests — run them
    // manually after registering "Test" as a speaker via the UI:
    //
    //   cargo test --features voice -- --ignored floor_claim
    //
    // If you register "Test" first, all three should pass.
    #[test]
    #[ignore]
    fn strip_basic_comma_form() {
        assert_eq!(
            strip_floor_claim_prefix("test, here, run dust"),
            "run dust"
        );
    }

    #[test]
    #[ignore]
    fn strip_no_comma_form() {
        assert_eq!(
            strip_floor_claim_prefix("test here run dust"),
            "run dust"
        );
    }

    #[test]
    #[ignore]
    fn strip_with_no_command() {
        assert_eq!(strip_floor_claim_prefix("test, here"), "");
        assert_eq!(strip_floor_claim_prefix("test here"), "");
    }
}

#[cfg(test)]
mod high_confidence_tests {
    // v0.1.213: tests for Intent::is_high_confidence — the
    // classifier used by bg-drain to decide whether a parsed
    // intent is strong enough to learn from ambient audio.
    // v0.1.766: was `use super::Intent;` which left `parse` out of scope, so
    // this whole module failed to compile under `cargo test` (silent: the
    // compile GATE is `cargo check`, which skips test cfg). Widened to `*`.
    use super::*;

    #[test]
    fn catch_alls_are_low_confidence() {
        assert!(!Intent::Dictate("anything".into()).is_high_confidence());
        assert!(!Intent::Freeform("anything".into()).is_high_confidence());
        assert!(!Intent::LlmFallback("anything".into()).is_high_confidence());
        assert!(!Intent::Unknown.is_high_confidence());
        assert!(!Intent::SearchWeb("anything".into()).is_high_confidence());
    }

    #[test]
    fn specific_verb_intents_are_high_confidence() {
        // These all require specific phrasing to trigger — overheard
        // speech wouldn't match these.
        assert!(Intent::GoBack.is_high_confidence());
        assert!(Intent::GoForward.is_high_confidence());
        assert!(Intent::Reload.is_high_confidence());
        assert!(Intent::GoTop.is_high_confidence());
        assert!(Intent::GoBottom.is_high_confidence());
        assert!(Intent::OpenSettings.is_high_confidence());
        assert!(Intent::OpenHomepage.is_high_confidence());
        assert!(Intent::Quit.is_high_confidence());
        assert!(Intent::RerunLast.is_high_confidence());
        assert!(Intent::WorkflowStopRecording.is_high_confidence());
    }

    #[test]
    fn navigate_is_high_confidence() {
        // Navigate parses only when whisper produces a recognizable
        // URL form or domain. Not a generic catch-all.
        assert!(Intent::Navigate("https://example.com".into()).is_high_confidence());
    }

    #[test]
    fn parameterized_specific_intents_are_high_confidence() {
        // These take a parameter but still require specific verb
        // prefixes to parse.
        assert!(Intent::TerminalCommand("ls".into()).is_high_confidence());
        assert!(Intent::FindInPage("query".into()).is_high_confidence());
        assert!(Intent::ClickLink("text".into()).is_high_confidence());
        assert!(Intent::Toggle("ads".into()).is_high_confidence());
        assert!(Intent::Copy("url".into()).is_high_confidence());
    }

    /// v0.1.292 tab intents are recognized.
    #[test]
    fn tab_intents_parse() {
        assert!(matches!(parse("next tab"), Intent::TabNext));
        assert!(matches!(parse("go to next tab"), Intent::TabNext));
        assert!(matches!(parse("previous tab"), Intent::TabPrev));
        assert!(matches!(parse("prev tab"), Intent::TabPrev));
        assert!(matches!(parse("close tab"), Intent::TabClose));
        assert!(matches!(parse("close this tab"), Intent::TabClose));
        // empty-param fork
        match parse("new tab") {
            Intent::TabOpen(url) => assert_eq!(url, ""),
            other => panic!("expected TabOpen(''), got {:?}", other),
        }
        match parse("fork tab") {
            Intent::TabOpen(url) => assert_eq!(url, ""),
            other => panic!("expected TabOpen(''), got {:?}", other),
        }
        // named-site form
        match parse("open hacker news in new tab") {
            Intent::TabOpen(url) => assert_eq!(url, "hacker news"),
            other => panic!("expected TabOpen(hacker news), got {:?}", other),
        }
        match parse("open github in a new tab") {
            Intent::TabOpen(url) => assert_eq!(url, "github"),
            other => panic!("expected TabOpen(github), got {:?}", other),
        }
    }

    /// v0.1.746: native terminal pane voice commands route to TermPane and do
    /// NOT collide with browser tab intents.
    #[test]
    fn term_pane_intents_parse() {
        for phrase in ["new pane right", "split down", "close pane",
                       "focus left", "focus up", "resize right"] {
            match parse(phrase) {
                Intent::TermPane(p) => assert_eq!(p, phrase),
                other => panic!("expected TermPane({:?}), got {:?}", phrase, other),
            }
        }
        // browser-tab phrasing still wins for the ambiguous "tab" words
        assert!(matches!(parse("next tab"), Intent::TabNext));
        assert!(matches!(parse("new tab"), Intent::TabOpen(_)));
        // a non-command sentence is not a TermPane
        assert!(!matches!(parse("split the bill with me"), Intent::TermPane(_)));
    }

    /// v0.1.296 tier1 status intent parses.
    #[test]
    fn tier1_status_parses() {
        assert!(matches!(parse("tier 1 status"), Intent::Tier1Status));
        assert!(matches!(parse("tier one status"), Intent::Tier1Status));
        assert!(matches!(parse("acoustic status"), Intent::Tier1Status));
        assert!(matches!(parse("exemplar count"), Intent::Tier1Status));
    }

    /// v0.1.292 tab intent ordering: "close tab" must not be
    /// confused for "open tab" since they share suffix.
    #[test]
    fn tab_intent_close_not_open() {
        let r = parse("close tab");
        assert!(!matches!(r, Intent::TabOpen(_)),
            "close tab should NOT be TabOpen, got {:?}", r);
    }

    /// v0.1.298 workspace intents parse.
    #[test]
    fn workspace_intents_parse() {
        // List variants
        assert!(matches!(parse("list workspaces"), Intent::WorkspaceList));
        assert!(matches!(parse("show workspaces"), Intent::WorkspaceList));
        assert!(matches!(parse("my workspaces"), Intent::WorkspaceList));

        // Save variants
        // v0.1.772: the two save-as phrasings go through DIFFERENT arms and
        // produce DIFFERENT (but functionally equivalent — QML routes both
        // wires to backend.workspace_save_as) variants: "save as X" →
        // WorkspaceSaveAs, "save workspace as X" → SaveWorkspaceAs. Assert what
        // each phrase actually produces.
        match parse("save as research") {
            Intent::WorkspaceSaveAs(n) => assert_eq!(n, "research"),
            other => panic!("expected WorkspaceSaveAs(research), got {:?}", other),
        }
        match parse("save workspace as work") {
            Intent::SaveWorkspaceAs(n) => assert_eq!(n, "work"),
            other => panic!("expected SaveWorkspaceAs(work), got {:?}", other),
        }
        match parse("save this as kitchen") {
            Intent::WorkspaceSaveAs(n) => assert_eq!(n, "kitchen"),
            other => panic!("expected WorkspaceSaveAs(kitchen), got {:?}", other),
        }

        // Load variants
        // v0.1.772: parser produces LoadWorkspace; QML routes both the
        // load_workspace and workspace_load wires to workspace_load_by_name, so
        // the variants are equivalent. Assert the produced one.
        match parse("load workspace research") {
            Intent::LoadWorkspace(n) => assert_eq!(n, "research"),
            other => panic!("expected LoadWorkspace(research), got {:?}", other),
        }
        match parse("switch to research workspace") {
            Intent::LoadWorkspace(n) => assert_eq!(n, "research"),
            other => panic!("expected LoadWorkspace(research), got {:?}", other),
        }
        // "open X workspace" routes through a different arm → WorkspaceLoad
        // (equivalent wire; QML handles both). The "load workspace X" / "switch
        // to X workspace" forms above produce LoadWorkspace.
        match parse("open work workspace") {
            Intent::WorkspaceLoad(n) => assert_eq!(n, "work"),
            other => panic!("expected WorkspaceLoad(work), got {:?}", other),
        }
    }

    /// v0.1.298 regression: "save this as workflow X" must
    /// route to WorkflowStartRecording, NOT WorkspaceSaveAs.
    /// The workspace parser was added BEFORE the workflow
    /// parser in source order; this test pins the "starts_with
    /// workflow " fallthrough.
    #[test]
    fn workspace_save_yields_to_workflow() {
        let r = parse("save this as workflow my_routine");
        assert!(matches!(r, Intent::WorkflowStartRecording(_)),
            "should be WorkflowStartRecording, got {:?}", r);
    }

    /// v0.1.301 workspace delete intents parse correctly,
    /// and refuse "default".
    #[test]
    fn workspace_delete_intents() {
        match parse("delete workspace research") {
            Intent::WorkspaceDelete(n) => assert_eq!(n, "research"),
            other => panic!("expected WorkspaceDelete(research), got {:?}", other),
        }
        match parse("remove workspace work") {
            Intent::WorkspaceDelete(n) => assert_eq!(n, "work"),
            other => panic!("expected WorkspaceDelete(work), got {:?}", other),
        }
        match parse("forget kitchen workspace") {
            Intent::WorkspaceDelete(n) => assert_eq!(n, "kitchen"),
            other => panic!("expected WorkspaceDelete(kitchen), got {:?}", other),
        }
        // "default" is refused
        let r = parse("delete workspace default");
        assert!(!matches!(r, Intent::WorkspaceDelete(_)),
            "should not delete default, got {:?}", r);
    }

    /// v0.1.304 threshold setter parses numeric value.
    #[test]
    fn tier1_threshold_parser() {
        match parse("set threshold to 0.12") {
            Intent::Tier1SetThreshold(t) => assert!((t - 0.12).abs() < 0.001),
            other => panic!("expected Tier1SetThreshold(0.12), got {:?}", other),
        }
        match parse("tier 1 threshold 0.08") {
            Intent::Tier1SetThreshold(t) => assert!((t - 0.08).abs() < 0.001),
            other => panic!("expected Tier1SetThreshold(0.08), got {:?}", other),
        }
        // Out of range rejected
        let r = parse("set threshold to 5.0");
        assert!(!matches!(r, Intent::Tier1SetThreshold(_)),
            "5.0 out of [0..1] range, got {:?}", r);
        let r = parse("set threshold to -0.1");
        assert!(!matches!(r, Intent::Tier1SetThreshold(_)),
            "-0.1 out of [0..1] range, got {:?}", r);
    }

    /// v0.1.310 workspace rename parser. Two forms.
    #[test]
    fn workspace_rename_parser() {
        // "rename workspace X to Y"
        match parse("rename workspace morning to dawn") {
            Intent::WorkspaceRename(s) => {
                assert_eq!(s, "morning\x1Fdawn");
            }
            other => panic!("expected WorkspaceRename, got {:?}", other),
        }
        // "rename X workspace to Y"
        match parse("rename research workspace to study") {
            Intent::WorkspaceRename(s) => {
                assert_eq!(s, "research\x1Fstudy");
            }
            other => panic!("expected WorkspaceRename, got {:?}", other),
        }
    }

    /// v0.1.311 workspace inspect parser. Multiple phrasings.
    #[test]
    fn workspace_inspect_parser() {
        match parse("show workspace morning") {
            Intent::WorkspaceInspect(n) => assert_eq!(n, "morning"),
            other => panic!("expected WorkspaceInspect, got {:?}", other),
        }
        match parse("describe workspace work") {
            Intent::WorkspaceInspect(n) => assert_eq!(n, "work"),
            other => panic!("expected WorkspaceInspect, got {:?}", other),
        }
        match parse("inspect workspace research") {
            Intent::WorkspaceInspect(n) => assert_eq!(n, "research"),
            other => panic!("expected WorkspaceInspect, got {:?}", other),
        }
        match parse("what's in research workspace") {
            Intent::WorkspaceInspect(n) => assert_eq!(n, "research"),
            other => panic!("expected WorkspaceInspect, got {:?}", other),
        }
    }

    /// v0.1.313 workspace copy parser. Multiple phrasings.
    #[test]
    fn workspace_copy_parser() {
        match parse("copy workspace research to backup") {
            Intent::WorkspaceCopy(s) => {
                assert_eq!(s, "research\x1Fbackup");
            }
            other => panic!("expected WorkspaceCopy, got {:?}", other),
        }
        match parse("fork research workspace to experiment") {
            Intent::WorkspaceCopy(s) => {
                assert_eq!(s, "research\x1Fexperiment");
            }
            other => panic!("expected WorkspaceCopy, got {:?}", other),
        }
        match parse("clone workspace work as work-backup") {
            Intent::WorkspaceCopy(s) => {
                assert_eq!(s, "work\x1Fwork-backup");
            }
            other => panic!("expected WorkspaceCopy, got {:?}", other),
        }
    }

    /// v0.1.314 current workspace status. Many phrasings.
    #[test]
    fn workspace_current_parser() {
        assert!(matches!(parse("current workspace"), Intent::WorkspaceCurrent));
        assert!(matches!(parse("what workspace am i in"), Intent::WorkspaceCurrent));
        assert!(matches!(parse("which workspace"), Intent::WorkspaceCurrent));
        assert!(matches!(parse("active workspace"), Intent::WorkspaceCurrent));
    }

    /// v0.1.315 "save workspace" (no name) routes to SaveNow,
    /// NOT to SaveAs.
    #[test]
    fn workspace_save_now_parser() {
        assert!(matches!(parse("save workspace"), Intent::WorkspaceSaveNow));
        assert!(matches!(parse("save my workspace"), Intent::WorkspaceSaveNow));
        assert!(matches!(parse("save current workspace"), Intent::WorkspaceSaveNow));
        assert!(matches!(parse("snapshot workspace"), Intent::WorkspaceSaveNow));
    }

    /// v0.1.315 regression: "save workspace as foo" must still
    /// route to SaveAs, not SaveNow.
    #[test]
    fn workspace_save_as_still_works_after_save_now() {
        match parse("save workspace as backup") {
            Intent::SaveWorkspaceAs(n) => assert_eq!(n, "backup"),
            other => panic!("expected SaveWorkspaceAs, got {:?}", other),
        }
    }

    /// v0.1.319 connections view parser.
    #[test]
    fn open_connections_view_parser() {
        assert!(matches!(parse("show connections"), Intent::OpenConnectionsView));
        assert!(matches!(parse("walk me through my last session"), Intent::WalkLastSession));
        assert!(matches!(parse("walk last session"), Intent::WalkLastSession));
        assert!(matches!(parse("show me my last session"), Intent::WalkLastSession));
        assert!(matches!(parse("what did i do last time"), Intent::RecallCommands(_)));
        // v0.1.655 regression: must NOT be swallowed by the audit-log
        // ("what did X do") arm — the sim harness caught this.
        assert!(!matches!(parse("what did i do last time"), Intent::ShowAuditLogForSpeaker(_)));
        // v0.1.657 corpus regression (Asahi narration false-positives):
        // "add X to <arbitrary place>" must NOT hallucinate a file write;
        // only known/explicit targets append. Found via sim-corpus.sh.
        assert!(!matches!(parse("add those to the wrong place"), Intent::FileAppend(_, _)));
        assert!(matches!(parse("add buy milk to my notes"), Intent::FileAppend(_, _)));
        // v0.1.658 corpus UX refinements (Asahi narration false-positives →
        // 0.4% to 0.11%). Narration must not misfire; legit commands must.
        assert!(!matches!(parse("basically run this on the host, effectively"), Intent::TerminalCommand(_)));
        assert!(!matches!(parse("just run this with those"), Intent::TerminalCommand(_)));
        assert!(!matches!(parse("type 2"), Intent::TerminalCommand(_)));
        assert!(!matches!(parse("shut down"), Intent::Quit));
        assert!(!matches!(parse("what did i do with this one"), Intent::RecallCommands(_)));
        assert!(matches!(parse("run cargo test"), Intent::TerminalCommand(c) if c=="cargo test"));
        assert!(matches!(parse("run it again"), Intent::RerunLast));
        assert!(matches!(parse("run git status"), Intent::TerminalCommand(_)));
        assert!(matches!(parse("what did i just do"), Intent::RecallCommands(_)));
        match parse("how did i fix the docker build") {
            Intent::RecallCommands(q) => assert_eq!(q, "the docker build"),
            other => panic!("expected RecallCommands, got {:?}", other),
        }
        match parse("what was that command for nginx?") {
            Intent::RecallCommands(q) => assert_eq!(q, "nginx"),
            other => panic!("expected RecallCommands, got {:?}", other),
        }
        // v0.1.654: editor-open with context triggers on markers...
        assert!(matches!(parse("edit src/dcp.rs at line 42"), Intent::OpenInEditor(_)));
        assert!(matches!(parse("open the tracer in helix"), Intent::OpenInEditor(_)));
        assert!(matches!(parse("open config.txt on buildbox in vim"), Intent::OpenInEditor(_)));
        // ...but plain "open <site>" stays a navigation, NOT editor-open.
        assert!(!matches!(parse("open google"), Intent::OpenInEditor(_)));
        assert!(!matches!(parse("open github.com"), Intent::OpenInEditor(_)));
        // v0.1.678: "edit this note" → in-app editor (EditCurrentNote), NOT
        // the external-editor path; and it must not be confused with
        // "edit <file>" which stays OpenInEditor.
        assert!(matches!(parse("edit this note"), Intent::EditCurrentNote));
        assert!(matches!(parse("edit this"), Intent::EditCurrentNote));
        assert!(matches!(parse("open the editor"), Intent::EditCurrentNote));
        assert!(matches!(parse("open note editor"), Intent::EditCurrentNote));
        assert!(!matches!(parse("edit this note"), Intent::OpenInEditor(_)));
        // v0.1.680: content search across note bodies.
        match parse("search my notes for cloak") {
            Intent::SearchNotes(q) => assert_eq!(q, "cloak"),
            other => panic!("expected SearchNotes, got {:?}", other),
        }
        match parse("search notes for outer rim cloaks") {
            Intent::SearchNotes(q) => assert_eq!(q, "outer rim cloaks"),
            other => panic!("expected SearchNotes, got {:?}", other),
        }
        match parse("find cloak in my notes") {
            Intent::SearchNotes(q) => assert_eq!(q, "cloak"),
            other => panic!("expected SearchNotes, got {:?}", other),
        }
        // plain "search X" stays web search, not notes
        assert!(!matches!(parse("search for cats"), Intent::SearchNotes(_)));
        assert!(matches!(parse("edit src/dcp.rs at line 42"), Intent::OpenInEditor(_)));
        assert!(matches!(parse("show graph"), Intent::OpenConnectionsView));
        assert!(matches!(parse("open graph view"), Intent::OpenConnectionsView));
        assert!(matches!(parse("show cards"), Intent::OpenConnectionsView));
        assert!(matches!(parse("connections view"), Intent::OpenConnectionsView));
    }

    /// v0.1.327 mermaid demo parser. Must short-circuit before
    /// OpenConnectionsView's "show ..." matches.
    #[test]
    fn show_mermaid_demo_parser() {
        assert!(matches!(parse("show mermaid demo"), Intent::ShowMermaidDemo));
        assert!(matches!(parse("show diagram demo"), Intent::ShowMermaidDemo));
        assert!(matches!(parse("demo diagram"), Intent::ShowMermaidDemo));
        assert!(matches!(parse("test mermaid"), Intent::ShowMermaidDemo));
        // Regression: "show graph" must STILL route to
        // OpenConnectionsView, not ShowMermaidDemo.
        assert!(matches!(parse("show graph"), Intent::OpenConnectionsView));
    }

    /// v0.1.329 tilt graph parser — numeric, bare, preset forms.
    #[test]
    fn tilt_graph_parser_numeric() {
        match parse("tilt 30") {
            Intent::TiltGraph(a) => assert!((a - 30.0).abs() < 0.1),
            other => panic!("expected TiltGraph(30), got {:?}", other),
        }
        match parse("tilt 45 degrees") {
            Intent::TiltGraph(a) => assert!((a - 45.0).abs() < 0.1),
            other => panic!("expected TiltGraph(45), got {:?}", other),
        }
        match parse("tilt to 50") {
            Intent::TiltGraph(a) => assert!((a - 50.0).abs() < 0.1),
            other => panic!("expected TiltGraph(50), got {:?}", other),
        }
    }

    #[test]
    fn tilt_graph_parser_clamps_to_75() {
        match parse("tilt 999") {
            Intent::TiltGraph(a) => assert!((a - 75.0).abs() < 0.1),
            other => panic!("expected TiltGraph(75 clamped), got {:?}", other),
        }
    }

    #[test]
    fn tilt_graph_parser_bare_default() {
        match parse("tilt graph") {
            Intent::TiltGraph(a) => assert!((a - 35.0).abs() < 0.1),
            other => panic!("expected TiltGraph(35 default), got {:?}", other),
        }
        match parse("3d view") {
            Intent::TiltGraph(a) => assert!((a - 35.0).abs() < 0.1),
            other => panic!("expected TiltGraph(35 from 3d view), got {:?}", other),
        }
    }

    #[test]
    fn tilt_graph_parser_presets() {
        match parse("tilt steeper") {
            Intent::TiltGraph(a) => assert!((a - 60.0).abs() < 0.1),
            other => panic!("expected TiltGraph(60), got {:?}", other),
        }
        match parse("tilt gentle") {
            Intent::TiltGraph(a) => assert!((a - 20.0).abs() < 0.1),
            other => panic!("expected TiltGraph(20), got {:?}", other),
        }
    }

    /// v0.1.329 flatten parser. CRITICAL: must short-circuit
    /// BEFORE any tilt-parser numeric form so "flatten graph"
    /// doesn't get captured as a tilt query.
    #[test]
    fn flatten_graph_parser() {
        assert!(matches!(parse("flatten graph"), Intent::FlattenGraph));
        assert!(matches!(parse("flatten"), Intent::FlattenGraph));
        assert!(matches!(parse("collapse to 2d"), Intent::FlattenGraph));
        assert!(matches!(parse("untilt"), Intent::FlattenGraph));
        assert!(matches!(parse("go flat"), Intent::FlattenGraph));
    }

    /// v0.1.338 close-graph parser. Several near-synonym phrasings
    /// should all route to CloseConnectionsView.
    #[test]
    fn close_connections_view_parser() {
        assert!(matches!(parse("close graph"), Intent::CloseConnectionsView));
        assert!(matches!(parse("close cards"), Intent::CloseConnectionsView));
        assert!(matches!(parse("close map"), Intent::CloseConnectionsView));
        assert!(matches!(parse("close connections view"), Intent::CloseConnectionsView));
        assert!(matches!(parse("back to page"), Intent::CloseConnectionsView));
        assert!(matches!(parse("exit graph"), Intent::CloseConnectionsView));
        assert!(matches!(parse("dismiss graph"), Intent::CloseConnectionsView));
        assert!(matches!(parse("hide cards"), Intent::CloseConnectionsView));
    }

    /// v0.1.338 reset-zoom parser.
    #[test]
    fn reset_cards_zoom_parser() {
        assert!(matches!(parse("reset zoom"), Intent::ResetCardsZoom));
        assert!(matches!(parse("fit graph"), Intent::ResetCardsZoom));
        assert!(matches!(parse("fit cards"), Intent::ResetCardsZoom));
        assert!(matches!(parse("center graph"), Intent::ResetCardsZoom));
        assert!(matches!(parse("recenter"), Intent::ResetCardsZoom));
        assert!(matches!(parse("zoom to fit"), Intent::ResetCardsZoom));
        assert!(matches!(parse("show whole graph"), Intent::ResetCardsZoom));
    }

    /// Regression: the new close/reset phrasings must not steal
    /// from the existing open-graph parser. "show graph" still
    /// routes to OpenConnectionsView.
    #[test]
    fn close_reset_zoom_do_not_break_open_graph() {
        assert!(matches!(parse("show graph"), Intent::OpenConnectionsView));
        assert!(matches!(parse("open graph"), Intent::OpenConnectionsView));
        assert!(matches!(parse("show cards"), Intent::OpenConnectionsView));
    }

    /// v0.1.323 cards filter parser.
    #[test]
    fn filter_cards_parser() {
        match parse("filter cards to rust") {
            Intent::FilterCards(q) => assert_eq!(q, "rust"),
            other => panic!("expected FilterCards(rust), got {:?}", other),
        }
        match parse("filter to github") {
            Intent::FilterCards(q) => assert_eq!(q, "github"),
            other => panic!("expected FilterCards(github), got {:?}", other),
        }
        match parse("search cards for tutorial") {
            Intent::FilterCards(q) => assert_eq!(q, "tutorial"),
            other => panic!("expected FilterCards(tutorial), got {:?}", other),
        }
        match parse("find cards with rust async") {
            Intent::FilterCards(q) => assert_eq!(q, "rust async"),
            other => panic!("expected FilterCards(rust async), got {:?}", other),
        }
    }

    /// v0.1.323 clear filter parser. CRITICAL: must NOT be
    /// captured as a FilterCards("filter") query — "clear
    /// filter" must short-circuit to ClearCardsFilter before
    /// the prefix-based filter parsers run.
    #[test]
    fn clear_cards_filter_parser() {
        assert!(matches!(parse("clear filter"), Intent::ClearCardsFilter));
        assert!(matches!(parse("show all cards"), Intent::ClearCardsFilter));
        assert!(matches!(parse("reset filter"), Intent::ClearCardsFilter));
        assert!(matches!(parse("remove filter"), Intent::ClearCardsFilter));
        // Make sure "show all cards" doesn't get captured as
        // OpenConnectionsView either (it's similar to "show cards").
        // The pattern is matched by ClearCardsFilter first.
    }

    /// v0.1.324 tag intents.
    #[test]
    fn tag_card_parser() {
        match parse("tag this important") {
            Intent::TagCard(t) => assert_eq!(t, "important"),
            other => panic!("expected TagCard(important), got {:?}", other),
        }
        match parse("tag this as reading-list") {
            Intent::TagCard(t) => assert_eq!(t, "reading-list"),
            other => panic!("expected TagCard(reading-list), got {:?}", other),
        }
        match parse("add tag @work") {
            // "@" gets stripped from leading position.
            Intent::TagCard(t) => assert_eq!(t, "work"),
            other => panic!("expected TagCard(work), got {:?}", other),
        }
    }

    #[test]
    fn untag_card_parser() {
        match parse("untag this important") {
            Intent::UntagCard(t) => assert_eq!(t, "important"),
            other => panic!("expected UntagCard(important), got {:?}", other),
        }
        match parse("remove tag reading-list") {
            Intent::UntagCard(t) => assert_eq!(t, "reading-list"),
            other => panic!("expected UntagCard(reading-list), got {:?}", other),
        }
    }

    /// v0.1.324 regression: "remove tag X" must route to
    /// UntagCard, NOT to TagCard("X") (since "tag X" is a
    /// substring). Parser ordering pins this.
    #[test]
    fn untag_beats_tag_for_remove_tag_phrasing() {
        match parse("remove tag X") {
            Intent::UntagCard(_) => {},
            other => panic!("expected UntagCard, got {:?}", other),
        }
        match parse("delete tag X") {
            Intent::UntagCard(_) => {},
            other => panic!("expected UntagCard, got {:?}", other),
        }
    }

    #[test]
    fn filter_by_tag_parser() {
        match parse("filter by tag work") {
            Intent::FilterByTag(t) => assert_eq!(t, "work"),
            other => panic!("expected FilterByTag(work), got {:?}", other),
        }
        match parse("show only tag @work") {
            Intent::FilterByTag(t) => assert_eq!(t, "work"),
            other => panic!("expected FilterByTag(work), got {:?}", other),
        }
        match parse("show cards tagged urgent") {
            Intent::FilterByTag(t) => assert_eq!(t, "urgent"),
            other => panic!("expected FilterByTag(urgent), got {:?}", other),
        }
    }
}

#[cfg(test)]
mod destructive_wire_tests {
    //! v0.1.358: confirm the destructive flag round-trips through
    //! the candidates_to_qml wire format. QML parses by splitting
    //! on \x1F and reads p[6] ("0" or "1") as the destructive
    //! marker. These tests pin the position so a future field
    //! addition doesn't silently break QML parsing.

    use super::*;

    #[test]
    fn safe_candidate_emits_zero() {
        let c = Candidate::safe(
            Intent::TerminalCommand("git diff".into()),
            0.95,
            "show diff",
        );
        let wire = candidates_to_qml(&[c]);
        // wire = "kind\x1Fdesc\x1Fp1\x1Fp2\x1F0.95\x1Fshow diff\x1F0"
        let parts: Vec<&str> = wire.split('\x1f').collect();
        assert_eq!(parts.len(), 7);
        assert_eq!(parts[6], "0");
    }

    #[test]
    fn destructive_candidate_emits_one() {
        let c = Candidate::destructive(
            Intent::TerminalCommand("git stash".into()),
            0.94,
            "stash changes",
        );
        let wire = candidates_to_qml(&[c]);
        let parts: Vec<&str> = wire.split('\x1f').collect();
        assert_eq!(parts.len(), 7);
        assert_eq!(parts[6], "1");
    }

    #[test]
    fn struct_literal_destructive_false() {
        // The existing struct-literal style (most callers) still
        // works with explicit destructive: false.
        let c = Candidate {
            intent: Intent::TerminalCommand("ls".into()),
            score: 0.9,
            reason: "list".into(),
            destructive: false,
        };
        let wire = candidates_to_qml(&[c]);
        let parts: Vec<&str> = wire.split('\x1f').collect();
        assert_eq!(parts[6], "0");
    }
}

#[cfg(test)]
mod reset_cadence_tests {
    //! v0.1.368: ResetCadence intent parser. Tests cover self
    //! variants, named variants, possessive variants, and the
    //! negatives that should NOT match.

    use super::*;

    #[test]
    fn self_phrases_match_with_empty_name() {
        let cases: &[&str] = &[
            "reset my cadence",
            "clear my cadence",
            "reset cadence",
            "clear cadence",
            "reset speech cadence",
            "reset my speech cadence",
            "clear speech cadence",
            "reset speaking cadence",
            "reset cadence for me",
            "clear cadence for myself",
        ];
        for c in cases {
            assert_eq!(parse(c), Intent::ResetCadence(String::new()),
                "phrase {:?} should produce empty-name ResetCadence", c);
        }
    }

    #[test]
    fn named_for_phrases_extract_name() {
        assert_eq!(
            parse("reset cadence for Bob"),
            Intent::ResetCadence("Bob".to_string())
        );
        assert_eq!(
            parse("clear cadence for Alice"),
            Intent::ResetCadence("Alice".to_string())
        );
        assert_eq!(
            parse("reset the cadence for Carol"),
            Intent::ResetCadence("Carol".to_string())
        );
    }

    #[test]
    fn named_for_phrases_strip_trailing_whitespace() {
        // Defensive: the parser is supposed to trim.
        assert_eq!(
            parse("reset cadence for Bob   "),
            Intent::ResetCadence("Bob".to_string())
        );
    }

    #[test]
    fn named_for_phrases_with_me_target_become_self() {
        // "reset cadence for me" is a SELF reference, not a named
        // target literally called "me". Falls through to self
        // phrasing match.
        assert_eq!(
            parse("reset cadence for me"),
            Intent::ResetCadence(String::new())
        );
        assert_eq!(
            parse("clear cadence for myself"),
            Intent::ResetCadence(String::new())
        );
    }

    #[test]
    fn possessive_phrases_extract_name() {
        assert_eq!(
            parse("reset Bob's cadence"),
            Intent::ResetCadence("Bob".to_string())
        );
        assert_eq!(
            parse("clear Alice's cadence"),
            Intent::ResetCadence("Alice".to_string())
        );
    }

    #[test]
    fn possessive_rejects_pronoun_owners() {
        // "reset my cadence" / "reset her cadence" — the
        // possessive parser must NOT extract the pronoun as a
        // speaker name. "my" falls through to the self matcher
        // and returns empty. "her" / "his" return Unknown — they
        // don't match any registered speaker concept (we don't
        // resolve pronouns).
        assert_eq!(
            parse("reset my cadence"),
            Intent::ResetCadence(String::new())
        );
        // Pronouns other than "my" don't have a self-phrasing match. v0.1.767:
        // they no longer reach Unknown — they fall through to the Freeform
        // catch-all (added later). The invariant is that the pronoun is NOT
        // captured as a ResetCadence speaker name; assert that directly.
        assert!(!matches!(parse("reset her cadence"), Intent::ResetCadence(_)));
        assert!(!matches!(parse("reset their cadence"), Intent::ResetCadence(_)));
    }

    #[test]
    fn bare_reset_does_not_match() {
        // The word "cadence" is required. Bare reset/clear commands MUST NOT
        // collide with the ResetCadence intent — those are reserved for other
        // features (cards canvas reset filter, future workspace resets).
        // v0.1.767: the invariant is "not ResetCadence". Unmatched bare words
        // now fall through to the Freeform catch-all (added after this test was
        // written) rather than Unknown; assert the invariant directly so the
        // test is robust to the exact non-match variant.
        assert!(!matches!(parse("reset"), Intent::ResetCadence(_)));
        assert!(!matches!(parse("clear"), Intent::ResetCadence(_)));
        // The cards-canvas reset commands continue to work
        // (these intents are handled BEFORE the cadence section
        // in the parser).
        assert_eq!(parse("reset filter"), Intent::ClearCardsFilter);
        assert_eq!(parse("clear filter"), Intent::ClearCardsFilter);
    }

    #[test]
    fn wire_format_carries_name() {
        let intent = Intent::ResetCadence("Bob".to_string());
        let c = Candidate {
            intent,
            score: 0.9,
            reason: "test".into(),
            destructive: false,
        };
        let wire = candidates_to_qml(&[c]);
        let parts: Vec<&str> = wire.split('\x1f').collect();
        // v0.1.767: wire is kind\x1Fdesc\x1Fp1\x1Fp2\x1F… — the name is p1
        // (index 2), not index 1 (which is the human-readable desc). The test
        // predated the desc field.
        assert_eq!(parts[0], "reset_cadence");
        assert_eq!(parts[2], "Bob");
    }

    #[test]
    fn wire_format_empty_name_for_self() {
        let intent = Intent::ResetCadence(String::new());
        let c = Candidate {
            intent,
            score: 0.9,
            reason: "test".into(),
            destructive: false,
        };
        let wire = candidates_to_qml(&[c]);
        let parts: Vec<&str> = wire.split('\x1f').collect();
        // v0.1.767: name (p1) is index 2; index 1 is the desc.
        assert_eq!(parts[0], "reset_cadence");
        assert_eq!(parts[2], "");
    }

    #[test]
    fn is_high_confidence_for_reset_cadence() {
        // ResetCadence is verb-specific and should be treated as
        // high-confidence (not catch-all).
        assert!(Intent::ResetCadence("Bob".to_string()).is_high_confidence());
        assert!(Intent::ResetCadence(String::new()).is_high_confidence());
    }

    #[test]
    fn describe_named_and_self() {
        assert_eq!(
            Intent::ResetCadence("Bob".to_string()).describe(),
            "Reset cadence stats for Bob"
        );
        assert_eq!(
            Intent::ResetCadence(String::new()).describe(),
            "Reset cadence stats"
        );
    }
}

#[cfg(test)]
mod recipe_catalog_tests {
    //! v0.1.376 (discoverability gate): tests that enforce
    //! the recipes page stays in sync with the Intent enum.
    //! If a new Intent variant is added without a matching
    //! VOICE_RECIPES entry, `every_user_facing_intent_has_a_recipe`
    //! fails — the developer is forced to make the new feature
    //! discoverable before the build can ship.

    use super::*;
    use std::collections::HashSet;

    /// The set of intent kind-strings that are deliberately NOT
    /// in the recipes page because they're catch-alls / not user-
    /// facing features. Listed here explicitly so additions to
    /// this allowlist are noticed in code review.
    fn allowlist() -> HashSet<&'static str> {
        let mut s = HashSet::new();
        s.insert("unknown");      // parser fall-through
        s.insert("freeform");     // unstructured speech
        s.insert("dictate");      // typed-text dictation mode
        s.insert("llm"); // LlmFallback wire — auto-route to LLM when unclear
        s
    }

    /// Construct one instance of every Intent variant so we can
    /// call .to_wire_kind() on each. Variants with payload use
    /// minimal placeholder values — we're enumerating kinds, not
    /// testing payload behavior.
    fn all_intents() -> Vec<Intent> {
        vec![
            Intent::Navigate("a".into()),
            Intent::SearchWeb("a".into()),
            Intent::GoBack,
            Intent::GoForward,
            Intent::Reload,
            Intent::Scroll(0),
            Intent::ScrollPage(0),
            Intent::GoTop,
            Intent::GoBottom,
            Intent::FindInPage("a".into()),
            Intent::OpenPalette("a".into()),
            Intent::SwitchTab("a".into()),
            Intent::TerminalCommand("a".into()),
            Intent::OpenSettings,
            Intent::OpenHomepage,
            Intent::Quit,
            Intent::SystemInfo("a".into(), "b".into()),
            Intent::EmailCompose("a".into(), "b".into()),
            Intent::FileAppend("a".into(), "b".into()),
            Intent::LlmFallback("a".into()),
            Intent::ClickLink("a".into()),
            Intent::ScrollToHeading("a".into()),
            Intent::Copy("a".into()),
            Intent::Paste("a".into()),
            Intent::RerunLast,
            Intent::HistorySearch("a".into()),
            Intent::Toggle("a".into()),
            Intent::Dictate("a".into()),
            Intent::FindAnywhere("a".into()),
            Intent::WorkflowStartRecording("a".into()),
            Intent::WorkflowStopRecording,
            Intent::WorkflowPlayback("a".into()),
            Intent::Freeform("a".into()),
            Intent::Rollback,
            Intent::ShowAuditLog,
            Intent::ShowAuditLogForSpeaker("a".into()),
            Intent::ShowBranches,
            Intent::VerifyAuditLog,
            Intent::TabOpen("a".into()),
            Intent::TabNext,
            Intent::TabPrev,
            // v0.1.875: TermPane was omitted from the variant
            // enumeration, so its wire kind ("term_ws") wasn't a known
            // kind — which made the tile-ssh recipe (a TermPane phrase)
            // read as an orphan. Enumerate it like any other variant.
            Intent::TermPane("a".into()),
            Intent::TabClose,
            Intent::Tier1Status,
            Intent::Tier1Histogram,
            Intent::Tier1PromoteOn,
            Intent::Tier1PromoteOff,
            Intent::Tier1SetThreshold(0.0),
            Intent::Tier1AutoTune,
            Intent::WorkspaceSaveAs("a".into()),
            Intent::WorkspaceLoad("a".into()),
            Intent::WorkspaceList,
            Intent::WorkspaceDelete("a".into()),
            Intent::WorkspaceRename("a".into()),
            Intent::WorkspaceInspect("a".into()),
            Intent::WorkspaceCopy("a".into()),
            Intent::WorkspaceCurrent,
            Intent::WorkspaceSaveNow,
            Intent::OpenConnectionsView,
            Intent::FilterCards("a".into()),
            Intent::ClearCardsFilter,
            Intent::FocusPlane("a".into()),
            Intent::UnfocusPlane,
            Intent::ReadDiagramWarnings,
            Intent::ResetCadence("a".into()),
            Intent::ShowRecipes,
            Intent::SaveUrl(String::new()),
            Intent::ShowBookmarks,
            Intent::ShowBookmarksTagged("news".into()),
            Intent::SwitchToTabMatching("github".into()),
            Intent::SaveWorkspaceAs("name".into()),
            Intent::LoadWorkspace("name".into()),
            Intent::OpenFileDialog,
            Intent::SummarizeSession,
            Intent::ContextHelp,
            Intent::Unknown,
        ]
    }

    fn wire_kind(intent: &Intent) -> String {
        // Reuse the wire-format function — its first tuple
        // element IS the kind string used by QML dispatch and
        // by the recipes page.
        let c = Candidate {
            intent: intent.clone(),
            score: 0.0,
            reason: String::new(),
            destructive: false,
        };
        let wire = candidates_to_qml(&[c]);
        wire.split('\x1f').next().unwrap_or("").to_string()
    }

    #[test]
    fn every_user_facing_intent_has_a_recipe() {
        let allow = allowlist();
        let recipe_kinds: HashSet<&str> = VOICE_RECIPES.iter()
            .map(|r| r.kind)
            .collect();

        let mut missing: Vec<String> = Vec::new();
        for intent in all_intents() {
            let kind = wire_kind(&intent);
            if allow.contains(kind.as_str()) { continue; }
            if !recipe_kinds.contains(kind.as_str()) {
                missing.push(kind);
            }
        }
        assert!(missing.is_empty(),
            "Intent variants without recipe entries: {:?}.\n\
             Add an entry to VOICE_RECIPES (or extend the\n\
             allowlist() in this test if the variant is truly\n\
             not user-facing).",
            missing);
    }

    #[test]
    fn no_orphaned_recipes() {
        // Inverse check: every recipe must correspond to a
        // real Intent kind (catch typos in the static table).
        let all_kinds: HashSet<String> = all_intents().iter()
            .map(wire_kind)
            .collect();
        let mut orphans: Vec<&str> = Vec::new();
        for r in VOICE_RECIPES.iter() {
            if !all_kinds.contains(r.kind) {
                orphans.push(r.kind);
            }
        }
        assert!(orphans.is_empty(),
            "Recipes referencing non-existent intent kinds: {:?}",
            orphans);
    }

    #[test]
    fn every_recipe_has_at_least_one_example() {
        for r in VOICE_RECIPES.iter() {
            assert!(!r.examples.is_empty(),
                "Recipe '{}' has no examples", r.kind);
            for ex in r.examples {
                assert!(!ex.trim().is_empty(),
                    "Recipe '{}' has an empty example", r.kind);
            }
        }
    }

    #[test]
    fn every_recipe_has_what_happens_text() {
        for r in VOICE_RECIPES.iter() {
            assert!(!r.what_happens.trim().is_empty(),
                "Recipe '{}' has empty what_happens", r.kind);
            // Length sanity check — should be a sentence, not
            // a one-word label.
            assert!(r.what_happens.len() > 15,
                "Recipe '{}' what_happens too short: {:?}",
                r.kind, r.what_happens);
        }
    }
}

#[cfg(test)]
mod resolve_known_site_tests {
    //! v0.1.380: regression tests for the resolve_known_site
    //! sub-path heuristic. The bug fixed here produced
    //! `https://apple.com/.com` for input `"apple.com"` because
    //! the strip_prefix loop matched "apple" without requiring
    //! a word boundary, then concatenated `/.com`.

    use super::*;

    #[test]
    fn apple_com_input_does_not_become_double_dot_com() {
        // The original bug. "apple.com" → strip_prefix("apple")
        // → rest=".com" → format!("https://apple.com/.com").
        // Fixed: rest must start with ' ' to count.
        let r = resolve_known_site("apple.com");
        // Either None (fall through to the direct-URL branch
        // at the caller, which is what we want) OR the
        // canonical mapping. Both are correct; the BAD answer
        // is "https://apple.com/.com".
        if let Some(url) = r {
            assert!(!url.contains("/.com"),
                "must not produce malformed URL: {}", url);
        }
    }

    #[test]
    fn apple_with_subpath_still_works() {
        // The legitimate case for the sub-path heuristic.
        // "apple support" → "https://apple.com/support" is
        // the desired behavior.
        let r = resolve_known_site("apple support");
        assert!(r.is_some());
        let url = r.unwrap();
        assert!(url.contains("support"),
            "expected sub-path expansion: {}", url);
    }

    #[test]
    fn known_exact_match_unaffected() {
        // Exact-match path: the bug doesn't touch this.
        assert_eq!(resolve_known_site("hacker news").as_deref(),
            Some("https://news.ycombinator.com"));
        assert_eq!(resolve_known_site("apple").as_deref(),
            Some("https://apple.com"));
    }

    #[test]
    fn github_domain_form_not_treated_as_subpath() {
        // "github.com" with bug → "https://github.com/.com"
        let r = resolve_known_site("github.com");
        if let Some(url) = r {
            assert!(!url.contains("/.com"),
                "must not produce malformed URL: {}", url);
        }
    }

    #[test]
    fn longer_name_starting_with_known_key_unaffected() {
        // "applewatch" should NOT silently become
        // "https://apple.com/watch" — that would be a
        // creative interpretation, and the word boundary
        // check correctly rejects it.
        let r = resolve_known_site("applewatch");
        // Either None or some explicit mapping. Must NOT be
        // a sub-path of apple.com.
        if let Some(url) = r {
            assert!(!url.starts_with("https://apple.com/watch"),
                "must not silently split 'applewatch': {}", url);
        }
    }
}

#[cfg(test)]
mod switch_to_tab_tests {
    //! v0.1.389: tests for the SwitchToTabMatching parser.
    //! The phrasings overlap with several other tab intents
    //! and with the generic Navigate intent, so the test
    //! suite focuses on the disambiguation rules.

    use super::*;

    fn parse(text: &str) -> Intent {
        super::parse(text)
    }

    #[test]
    fn switch_to_the_x_tab() {
        assert_eq!(parse("switch to the github tab"),
            Intent::SwitchToTabMatching("github".to_string()));
    }

    #[test]
    fn go_to_the_x_tab() {
        assert_eq!(parse("go to the apple tab"),
            Intent::SwitchToTabMatching("apple".to_string()));
    }

    #[test]
    fn switch_to_tab_with_x() {
        assert_eq!(parse("switch to tab with stackoverflow"),
            Intent::SwitchToTabMatching("stackoverflow".to_string()));
        assert_eq!(parse("go to tab with reddit"),
            Intent::SwitchToTabMatching("reddit".to_string()));
    }

    #[test]
    fn go_to_x_tab_without_the() {
        // The bare "go to X tab" form. Catches users who
        // drop "the".
        assert_eq!(parse("go to news tab"),
            Intent::SwitchToTabMatching("news".to_string()));
    }

    #[test]
    fn directional_tab_intents_unaffected() {
        // The directional intents must take precedence —
        // "next tab" / "previous tab" / "close tab" should
        // NOT be misread as substring matches against
        // "next" / "previous" / "close".
        assert_eq!(parse("next tab"), Intent::TabNext);
        assert_eq!(parse("go to next tab"), Intent::TabNext);
        assert_eq!(parse("previous tab"), Intent::TabPrev);
        assert_eq!(parse("close tab"), Intent::TabClose);
    }

    #[test]
    fn navigate_unaffected() {
        // "go to apple.com" should still navigate, not try
        // to match a tab named "apple.com".
        let r = parse("go to apple.com");
        assert!(matches!(r, Intent::Navigate(_)),
            "go to apple.com should still navigate, got {:?}", r);
    }

    #[test]
    fn defensive_rejects_directional_words_as_match_query() {
        // Even if whisper truncates "next tab" to "go to
        // next tab" (which directional check above catches)
        // or worse "switch to next tab", the defensive
        // check should reject those queries rather than
        // creating a search for the literal word "next".
        assert_eq!(parse("switch to next tab"), Intent::TabNext);
        assert_eq!(parse("switch to previous tab"), Intent::TabPrev);
    }

    #[test]
    fn empty_query_rejected() {
        // "switch to the tab" (no name) → not a tab match,
        // falls through to unknown / other handlers.
        let r = parse("switch to the tab");
        assert!(!matches!(r, Intent::SwitchToTabMatching(_)),
            "empty query should not produce a tab match: {:?}", r);
    }
}

#[cfg(test)]
mod workspace_save_load_tests {
    //! v0.1.390: parser tests for SaveWorkspaceAs +
    //! LoadWorkspace. The phrasings overlap with several
    //! other tab/workspace intents; tests focus on the
    //! disambiguation rules.

    use super::*;

    fn parse(text: &str) -> Intent {
        super::parse(text)
    }

    #[test]
    fn save_workspace_as_basic() {
        assert_eq!(parse("save workspace as research"),
            Intent::SaveWorkspaceAs("research".to_string()));
        assert_eq!(parse("save this workspace as project-x"),
            Intent::SaveWorkspaceAs("project-x".to_string()));
        assert_eq!(parse("save my workspace as monday"),
            Intent::SaveWorkspaceAs("monday".to_string()));
    }

    #[test]
    fn save_workspace_multi_word_names() {
        // The name can be multiple words; the parser
        // doesn't enforce single-word.
        assert_eq!(parse("save workspace as friday research stack"),
            Intent::SaveWorkspaceAs("friday research stack".to_string()));
    }

    #[test]
    fn save_workspace_reserved_names_rejected() {
        // Whisper-truncation defense: don't save a workspace
        // literally named "this" or "current".
        let r = parse("save workspace as this");
        assert!(!matches!(r, Intent::SaveWorkspaceAs(_)),
            "reserved name should not save: {:?}", r);
        let r2 = parse("save workspace as current");
        assert!(!matches!(r2, Intent::SaveWorkspaceAs(_)),
            "reserved name should not save: {:?}", r2);
    }

    #[test]
    fn load_workspace_basic_forms() {
        assert_eq!(parse("load workspace research"),
            Intent::LoadWorkspace("research".to_string()));
        assert_eq!(parse("open workspace project-x"),
            Intent::LoadWorkspace("project-x".to_string()));
        assert_eq!(parse("switch to workspace monday"),
            Intent::LoadWorkspace("monday".to_string()));
    }

    #[test]
    fn load_workspace_the_form() {
        // "load the X workspace" — name in the middle.
        assert_eq!(parse("load the research workspace"),
            Intent::LoadWorkspace("research".to_string()));
        assert_eq!(parse("switch to the research workspace"),
            Intent::LoadWorkspace("research".to_string()));
    }

    #[test]
    fn load_workspace_x_workspace_form() {
        // "switch to X workspace" — bare form without "the".
        assert_eq!(parse("switch to research workspace"),
            Intent::LoadWorkspace("research".to_string()));
    }

    #[test]
    fn load_workspace_reserved_names_rejected() {
        let r = parse("load workspace this");
        assert!(!matches!(r, Intent::LoadWorkspace(_)),
            "reserved 'this' should not load: {:?}", r);
        let r2 = parse("switch to the current workspace");
        assert!(!matches!(r2, Intent::LoadWorkspace(_)),
            "reserved 'current' should not load: {:?}", r2);
    }

    #[test]
    fn workspace_doesnt_eat_save_url() {
        // "save url X" must still SaveUrl, not collide with
        // workspace save (different prefix entirely; just
        // confirming no regression).
        assert!(matches!(parse("save url example.com"),
            Intent::SaveUrl(_)));
    }

    #[test]
    fn workspace_doesnt_eat_tab_matching() {
        // "switch to the github tab" must still match tab,
        // not load a workspace named "github tab" — verified
        // by absence of " workspace" suffix.
        assert!(matches!(parse("switch to the github tab"),
            Intent::SwitchToTabMatching(_)));
    }

    #[test]
    fn workspace_doesnt_eat_navigate() {
        // "go to apple.com" must navigate, not load workspace
        // (the parser is order-sensitive but the workspace
        // patterns all require "workspace" in the input).
        assert!(matches!(parse("go to apple.com"),
            Intent::Navigate(_)));
    }

    #[test]
    fn positional_click_parses() {
        // v0.1.596: "click the Nth <kind>"
        assert!(matches!(parse("click the third link"),
            Intent::ClickPositional { ordinal: 3, .. }));
        assert!(matches!(parse("tap the first image"),
            Intent::ClickPositional { ordinal: 1, kind } if kind == "image"));
        assert!(matches!(parse("click the last button"),
            Intent::ClickPositional { ordinal: -1, kind } if kind == "button"));
        assert!(matches!(parse("click the fifth result"),
            Intent::ClickPositional { ordinal: 5, kind } if kind == "item"));
        // adjective between ordinal and noun
        assert!(matches!(parse("click the second blue button"),
            Intent::ClickPositional { ordinal: 2, kind } if kind == "button"));
    }

    #[test]
    fn positional_click_does_not_eat_text_clicks() {
        // CRITICAL no-regression: a non-ordinal click must STILL be a
        // text-match ClickLink, not get swallowed by the positional path.
        assert!(matches!(parse("click the login button"),
            Intent::ClickLink(_)),
            "text-match click must not be captured as positional");
        assert!(matches!(parse("click submit"), Intent::ClickLink(_)));
        // "click the link" — no ordinal — stays text-match (empty-ish)
        assert!(!matches!(parse("click the link"),
            Intent::ClickPositional { .. }));
    }
}

#[cfg(test)]
mod corpus_regression {
    //! v0.1.696: voice-command corpus regression. Reads tests/voice_corpus.txt
    //! (phrase => expected dispatch kind) and asserts the live parser still
    //! routes each phrase to that kind. This is the SANDBOX-runnable half of
    //! the voice test harness (the other half, scripts/voice_tts_harness.sh,
    //! adds TTS→whisper on top to catch recognition drift). Keeping the
    //! expected kinds here means a parser change that silently re-routes a
    //! command fails CI instead of shipping.
    use super::*;

    fn kind_of(phrase: &str) -> String {
        // to_qml_string() = "kind\x1Fdesc\x1Fp1\x1Fp2"; take the kind field.
        super::parse(phrase).to_qml_string()
            .split('\x1F').next().unwrap_or("").to_string()
    }

    #[test]
    fn corpus_phrases_route_as_expected() {
        let corpus = include_str!("../tests/voice_corpus.txt");
        let mut failures: Vec<String> = Vec::new();
        let mut checked = 0;
        for line in corpus.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let mut it = line.splitn(2, '\t');
            let phrase = it.next().unwrap_or("").trim();
            let expected = match it.next() { Some(e) => e.trim(), None => continue };
            if phrase.is_empty() || expected.is_empty() { continue; }
            checked += 1;
            let got = kind_of(phrase);
            if got != expected {
                failures.push(format!("  \"{}\" → got `{}`, expected `{}`",
                    phrase, got, expected));
            }
        }
        assert!(checked >= 15, "corpus should have a meaningful number of cases, got {}", checked);
        assert!(failures.is_empty(),
            "{} of {} corpus phrases mis-routed:\n{}",
            failures.len(), checked, failures.join("\n"));
    }

    // v0.1.726 (corpus fuzz): narration false positives found by sim-corpus.sh.
    #[test]
    fn narration_open_tail_not_searched() {
        // a kernel dev thinking aloud must NOT trigger a web search.
        let phrase = "open this up in ghidorah real quick and see what we have";
        let got = kind_of(phrase);
        assert_ne!(got, "search",
            "narration 'open … real quick …' must not become a web search, got {}", got);
        // v0.1.900: "open rust async docs" is a KNOWN pending product decision
        // (see docs/todo.md "sub-path nav vs web search"). resolve_known_site
        // matches the "rust" key and stitches "async docs" as a sub-path →
        // rust-lang.org/async/docs (Navigate), rather than a web search for the
        // phrase. Both are defensible; the sub-path feature ("github torvalds
        // linux" → github.com/torvalds/linux) is intentional. Until that call is
        // made, assert only the invariant that actually matters here: a real
        // open-query is NOT swallowed as narration/dictation/unknown — it routes
        // to a navigable OR searchable intent.
        let k = kind_of("open rust async docs");
        assert!(k == "search" || k == "navigate",
            "'open rust async docs' should navigate or search (pending decision), got {}", k);
    }

    #[test]
    fn open_tail_query_guard() {
        assert!(open_tail_is_query("google"));
        assert!(open_tail_is_query("rust async docs"));
        assert!(!open_tail_is_query("this up in ghidorah real quick and see what we have"));
        assert!(!open_tail_is_query("it again"));
        assert!(!open_tail_is_query("the editor, then run it"));
    }

    #[test]
    fn write_narration_not_dictated() {
        // "i need to write a function for this" → must NOT be a Dictate.
        assert_ne!(kind_of("write a function for this"), "dictate",
            "narration about writing code must not be dictation");
        assert_ne!(kind_of("write some code that parses json"), "dictate");
        // but a genuine dictation command still works
        assert_eq!(kind_of("write hello world"), "dictate");
        assert_eq!(kind_of("dictate the meeting starts at noon"), "dictate");
    }

    #[test]
    fn build_error_phrases_route_to_menu() {
        for p in ["show errors", "show build errors", "show me the build errors",
                  "what are the errors", "build errors", "summarize the build",
                  "error menu"] {
            assert_eq!(kind_of(p), "build_errors", "'{}' should open the error menu", p);
        }
        // must NOT hijack unrelated phrases
        assert_ne!(kind_of("show bookmarks"), "build_errors");
        assert_ne!(kind_of("split this pane horizontal"), "build_errors");
        assert_ne!(kind_of("new pane right"), "build_errors");
    }
}
