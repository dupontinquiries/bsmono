//! The main keyboard window — built around a libadwaita `ApplicationWindow`.
//!
//! The window is configured as a Wayland layer surface (anchored to the
//! bottom edge, no focus stealing) on supporting compositors via
//! `gtk4-layer-shell`, and falls back to a regular toplevel on X11. See
//! the `compositor::placement` module for details.

use crate::compositor::{configure_window_placement, Placement};
use crate::config::Config;
use crate::keyboard::engine::EngineEvent;
use crate::keyboard::{KeyAction, KeyboardEngine};
use crate::ui::keyboard_view::KeyboardView;
use anyhow::Result;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::info;

/// Process-wide "voice streaming partials enabled" flag. Read by the
/// voice bridge at the start of each recording session to decide
/// whether to spawn the streaming worker. Written by the settings
/// panel when the user toggles "Live transcription."
///
/// AtomicBool because (a) we need cross-thread visibility (settings
/// runs on main, bridge spawns workers on background) and (b) the
/// value is a single bit. Initial value is whatever
/// `cfg.voice.streaming` was at startup.
static VOICE_STREAMING_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Whether the 🎤 Voice key is visible. When false, Voice cells in
/// layouts are replaced with NoOp spacers at row-build time. Set
/// from config at startup; toggled by the settings panel.
static VOICE_KEY_VISIBLE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Whether the ✨ LLM key is visible. Same pattern as VOICE_KEY_VISIBLE.
static LLM_KEY_VISIBLE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Setter for VOICE_STREAMING_ENABLED. Called by the settings panel.
pub fn set_voice_streaming_enabled(enabled: bool) {
    VOICE_STREAMING_ENABLED
        .store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn set_voice_key_visible(visible: bool) {
    VOICE_KEY_VISIBLE.store(visible, std::sync::atomic::Ordering::Relaxed);
}

pub fn set_llm_key_visible(visible: bool) {
    LLM_KEY_VISIBLE.store(visible, std::sync::atomic::Ordering::Relaxed);
}

pub fn voice_key_visible() -> bool {
    VOICE_KEY_VISIBLE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn llm_key_visible() -> bool {
    LLM_KEY_VISIBLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// When true, the next voice-final transcript is interpreted as an
/// LLM prompt (and not committed to the focused app). Set by the
/// LLM bridge when the user long-presses the LLM key; cleared after
/// one cycle. The voice bridge checks this before its usual
/// "commit transcript" path.
static VOICE_FEEDS_LLM: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_voice_feeds_llm(divert: bool) {
    VOICE_FEEDS_LLM.store(divert, std::sync::atomic::Ordering::Relaxed);
}

pub fn voice_feeds_llm() -> bool {
    VOICE_FEEDS_LLM.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether to log voice transcripts to the JSONL log. Off by
/// default; toggled by the settings panel. Read at log-write time
/// so changes apply to the very next transcription.
static VOICE_LOG_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_voice_log_enabled(enabled: bool) {
    VOICE_LOG_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

fn voice_log_enabled() -> bool {
    VOICE_LOG_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Focus-tray mode flag — read by the input-method active
/// callback to decide whether to show the arrows tray or the
/// full keyboard on focus-in. Live-toggled by settings.
/// Default off — most users want the full keyboard on focus.
static FOCUS_SHOW_TRAY_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_focus_show_tray(on: bool) {
    FOCUS_SHOW_TRAY_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn focus_show_tray_enabled() -> bool {
    FOCUS_SHOW_TRAY_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Monotonically-increasing focus generation counter. Bumped on every
/// activate AND deactivate, so any deferred work scheduled from a
/// prior focus event can detect that a newer focus has arrived and
/// skip itself. Used by the deferred tray-check to avoid swapping a
/// terminal's full keyboard to an arrow tray because the deferred
/// closure landed in a different editing context than where it was
/// scheduled.
static FOCUS_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Public accessor for the current focus generation. Used by the
/// drag handler to snapshot the generation at drag_begin and
/// compare at drag_end — if the value has changed, focus moved
/// during the swipe (notification, app-switcher) and the resulting
/// word would commit to the WRONG field if we let drag_end proceed.
pub fn focus_generation() -> u64 {
    FOCUS_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// True iff SurroundingText has arrived since the most recent
/// Activate event. Used as the strong "this is a real edit context"
/// signal — Activate alone is too eager on some compositors (Plasma
/// Mobile's v1 IM protocol fires it on window-level focus, before
/// any cursor lands in a field), so a tray-mode user would see the
/// arrow tray pop up for unfocused fields. SurroundingText only
/// arrives when the compositor actually has a cursor position to
/// report, so its arrival is a much stronger signal.
///
/// Reset to false on every Activate, set to true on every
/// SurroundingText event. The 80ms focus reconciler checks this
/// to decide whether to honor tray-mode preference or default to
/// full keyboard.
static SURROUNDING_TEXT_SEEN_SINCE_ACTIVATE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// True iff SurroundingText has arrived with NON-EMPTY content since
/// the most recent Activate event. Used to distinguish "empty field"
/// (search bar, fresh draft) from "field with existing text" (where
/// cursor navigation via arrows tray is actually useful). Empty fields
/// default to full keyboard because the user is about to TYPE, not
/// navigate.
///
/// Reset to false on every Activate, set to true on every
/// SurroundingText event where text is non-empty. A field that USED
/// to have text and was cleared (e.g. user backspaced to empty)
/// keeps the flag from the last non-empty SurroundingText — that's
/// the "edit context, currently empty" state which is borderline; we
/// err toward arrows for it because the user was already in the
/// editing context.
static SURROUNDING_TEXT_HAS_CONTENT_SINCE_ACTIVATE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn surrounding_text_seen_since_activate() -> bool {
    SURROUNDING_TEXT_SEEN_SINCE_ACTIVATE
        .load(std::sync::atomic::Ordering::Relaxed)
}

pub fn surrounding_text_has_content_since_activate() -> bool {
    SURROUNDING_TEXT_HAS_CONTENT_SINCE_ACTIVATE
        .load(std::sync::atomic::Ordering::Relaxed)
}

pub fn note_surrounding_text_arrived() {
    SURROUNDING_TEXT_SEEN_SINCE_ACTIVATE
        .store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn note_surrounding_text_has_content() {
    SURROUNDING_TEXT_HAS_CONTENT_SINCE_ACTIVATE
        .store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn reset_surrounding_text_seen() {
    SURROUNDING_TEXT_SEEN_SINCE_ACTIVATE
        .store(false, std::sync::atomic::Ordering::Relaxed);
    SURROUNDING_TEXT_HAS_CONTENT_SINCE_ACTIVATE
        .store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Latest content_purpose reported by the host's input-method
/// protocol. Values match `zwp_input_method_v1::ContentPurpose`:
///   0 = Normal, 1 = Alpha, 2 = Digits, 3 = Number, 4 = Phone,
///   5 = URL, 6 = Email, 7 = Name, 8 = Password, 9 = Date,
///   10 = Time, 11 = DateTime, 12 = Terminal.
/// Written by the wayland backend on ContentType events. Read by
/// the focus-show logic so e.g. a terminal gets the full keyboard
/// even when focus-tray mode is on (users typing commands need keys,
/// not an arrow pad). 255 = "unknown / not reported yet".
static CONTENT_PURPOSE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(255);

/// Wayland's ContentPurpose::Terminal constant. Hoisted here so call
/// sites don't depend on the wayland module being compiled in.
pub const CONTENT_PURPOSE_TERMINAL: u8 = 12;

pub fn set_content_purpose(purpose: u8) {
    CONTENT_PURPOSE.store(purpose, std::sync::atomic::Ordering::Relaxed);
}

pub fn content_purpose() -> u8 {
    CONTENT_PURPOSE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the currently-focused field should override focus-tray
/// mode and show the full keyboard regardless.
///
/// We force full for any SPECIALIZED content type — password fields,
/// URLs, email, numeric input, phone, terminal, date/time pickers,
/// etc. For these the user is entering specific values rather than
/// navigating cursor, and a cursor-navigation tray is a usability
/// hole (they need the keys).
///
/// Wayland v1 and v2 number the ContentPurpose enum slightly
/// differently — v1 has no PIN concept, so date/time/datetime/
/// terminal shift down by 1 relative to v2 (v1 terminal=12, v2
/// terminal=13). But in BOTH versions, everything from purpose=2
/// (digits) upward is a specialized field. So the rule "anything
/// >= 2 needs full keyboard" works across both protocol versions
/// without per-version branching.
///
/// Only Normal (0) and Alpha (1) are arrows-eligible — these are
/// the truly generic "free-form text" contexts where cursor
/// navigation is plausibly useful.
///
/// 255 is the sentinel we set on deactivate ("we have no info");
/// treat that as "no specialized requirement" — same as Normal.
pub fn focus_needs_full_keyboard() -> bool {
    let p = content_purpose();
    p != 255 && p >= 2
}

/// Auto-hide flag — when true, the keyboard hides when no input
/// field has focus and shows when one gains focus. When false, the
/// keyboard stays visible regardless of focus state. Wraps
/// cfg.behavior.auto_hide_on_no_focus for live-reactivity.
static AUTO_HIDE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_auto_hide_enabled(on: bool) {
    AUTO_HIDE_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn auto_hide_enabled() -> bool {
    AUTO_HIDE_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Glass-effect flag — when true, the keyboard becomes semi-
/// transparent with a top-to-bottom refraction gradient. Doesn't
/// capture actual screen content (Wayland security model
/// forbids that without compositor cooperation) but visually
/// suggests material translucency. Live-toggled by settings.
static GLASS_EFFECT_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_glass_effect(on: bool) {
    GLASS_EFFECT_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
    apply_glass_class_to_root();
}

pub fn glass_effect_enabled() -> bool {
    GLASS_EFFECT_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Apply or remove the `.glass-keyboard` CSS class on the keyboard
/// view root. Called by `set_glass_effect`. The actual root widget
/// is the first child of the application window's content; we walk
/// the active windows to find it.
fn apply_glass_class_to_root() {
    use gtk4::prelude::*;
    if let Some(app) = gtk4::gio::Application::default() {
        if let Ok(app) = app.downcast::<gtk4::Application>() {
            for win in app.windows() {
                // The keyboard_box has class "keyboard-view"; walk
                // descendants until we find it. Cheap — typical
                // tree is ~10 nodes deep.
                if let Some(target) =
                    find_descendant_with_class(&win.upcast::<gtk4::Widget>(), "keyboard-view")
                {
                    if glass_effect_enabled() {
                        target.add_css_class("glass-keyboard");
                    } else {
                        target.remove_css_class("glass-keyboard");
                    }
                }
            }
        }
    }
}

fn find_descendant_with_class(widget: &gtk4::Widget, class: &str) -> Option<gtk4::Widget> {
    use gtk4::prelude::*;
    if widget.has_css_class(class) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_descendant_with_class(&c, class) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Process-wide one-shot status slot for ephemeral messages
/// originating from non-bridge code (settings panel, LLM endpoint
/// test, etc.). Producers call `post_microterm_status(text)`;
/// the per-bridge microterm-poll picks it up and displays it as
/// a Status content (with auto-hide).
///
/// We use a parking_lot Mutex<Option<String>> rather than an
/// mpsc channel because we want "latest message wins" semantics:
/// if two background tasks post messages near-simultaneously, the
/// second overwrites the first. The microterm doesn't need a
/// queue.
static MICROTERM_STATUS_SLOT: parking_lot::Mutex<Option<String>> =
    parking_lot::Mutex::new(None);

pub fn post_microterm_status(text: impl Into<String>) {
    *MICROTERM_STATUS_SLOT.lock() = Some(text.into());
}

/// Drain the status slot. Returns Some(text) if there was a
/// pending message; clears the slot. Called by the microterm
/// bridge's timer.
fn drain_microterm_status() -> Option<String> {
    MICROTERM_STATUS_SLOT.lock().take()
}

/// A posted steno suggestion payload: the phoneme buffer hint and
/// the candidate words (word + fuzzy flag). "Latest wins" like the
/// status slot.
pub struct StenoSuggestionPayload {
    pub buffer: String,
    pub candidates: Vec<(String, bool)>, // (word, fuzzy)
}

static MICROTERM_STENO_SLOT: parking_lot::Mutex<Option<StenoSuggestionPayload>> =
    parking_lot::Mutex::new(None);

/// Post steno word-expansion suggestions to the microterm ribbon.
/// `candidates` is (word, fuzzy) where fuzzy marks vowel-approximate
/// matches. An empty candidate list with a non-empty buffer shows
/// just the phoneme hint; both empty hides nothing (caller decides).
pub fn post_steno_suggestions(buffer: String, candidates: Vec<(String, bool)>) {
    *MICROTERM_STENO_SLOT.lock() = Some(StenoSuggestionPayload { buffer, candidates });
}

fn drain_steno_suggestions() -> Option<StenoSuggestionPayload> {
    MICROTERM_STENO_SLOT.lock().take()
}

/// Drop any pending-but-undrained steno suggestion payload. Used by
/// the focus-change handler to ensure a suggestion posted in field A
/// (but not yet drained by the 150-800ms suggestion pump) can't
/// surface in field B and become tappable against a fresh, unrelated
/// buffer.
pub fn clear_steno_suggestions() {
    *MICROTERM_STENO_SLOT.lock() = None;
}

/// Cancel request flag — set when the user taps the microterm's ✕
/// button. Drained each tick by both the voice and LLM bridges so
/// either can abort their in-flight work. Set-once semantics: the
/// drainer that catches it should perform its cancel, and we don't
/// care which one does (both will check; the other will just see
/// false on its next tick).
/// Microterm cancel generation. Incremented by ✕ button presses.
/// Each consumer keeps its own "last seen" value and compares;
/// they all see the same cancel even though they run independently.
/// Using a generation counter (rather than a one-shot bool) avoids
/// the race where one bridge consumes the cancel before another
/// bridge sees it.
static MICROTERM_CANCEL_GEN: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

pub fn request_microterm_cancel() {
    MICROTERM_CANCEL_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Snapshot the cancel generation. Compare against your previous
/// snapshot to detect new cancels.
pub fn cancel_generation() -> u32 {
    MICROTERM_CANCEL_GEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Live LLM config snapshot. Updated by the settings panel on every
/// edit; read by the LLM bridge when constructing a fresh service
/// per-request. This means endpoint/model/api-key changes apply to
/// the very next /✨ tap without restart.
///
/// We use parking_lot::Mutex with const-init for the same reasons
/// as MICROTERM_STATUS_SLOT: no async, no Send/Sync gymnastics, and
/// settings changes are infrequent enough that lock contention is
/// irrelevant.
#[cfg(feature = "llm")]
static LIVE_LLM_CONFIG: parking_lot::Mutex<Option<crate::config::LlmConfig>> =
    parking_lot::Mutex::new(None);

#[cfg(feature = "llm")]
pub fn set_live_llm_config(cfg: crate::config::LlmConfig) {
    *LIVE_LLM_CONFIG.lock() = Some(cfg);
}

#[cfg(feature = "llm")]
fn current_llm_config() -> Option<crate::config::LlmConfig> {
    LIVE_LLM_CONFIG.lock().clone()
}

/// Slot for the most recently popped text from a Rewrite/Synonym
/// task. If the LLM call subsequently fails, the bridge re-commits
/// this so the user gets their original text back (the editor has
/// already had the backspaces applied, so the word disappeared
/// from their UI). Cleared on successful LLM commit.
static LLM_RESTORE_TEXT: parking_lot::Mutex<Option<String>> =
    parking_lot::Mutex::new(None);

/// Stash text to restore if the in-flight LLM request fails (e.g.,
/// network error, model gave empty reply, user cancelled from the
/// microterm). The /rewrite and /synonym paths set this BEFORE
/// dispatching: they've already popped the original from the engine
/// (which emitted backspaces to the host), so on failure we need to
/// put the original back.
///
/// Returns the PREVIOUS value, if any. A non-None return means an
/// earlier in-flight request had its restore data overwritten; the
/// caller should restore that data immediately so the user doesn't
/// lose their original text. Without this defense, two overlapping
/// LLM requests would silently lose the earlier one's restore data:
/// /rewrite pops 10 chunks → set("10 chunks") → /synonym pops "word"
/// → set("word") OVERWRITES "10 chunks" → if /synonym then errors,
/// only "word" gets restored and the 10 chunks are permanently gone.
#[must_use = "caller should restore the returned text via LlmCommit \
              so an overlapping request's original isn't lost"]
pub fn set_llm_restore_text(text: &str) -> Option<String> {
    let mut slot = LLM_RESTORE_TEXT.lock();
    let prev = slot.take();
    *slot = Some(text.to_string());
    prev
}

fn take_llm_restore_text() -> Option<String> {
    LLM_RESTORE_TEXT.lock().take()
}

/// Recent prompt history. The LLM bridge appends every prompt
/// submitted; the microterm reads it when opening the entry to
/// offer a popover of recent prompts. Bounded.
static LLM_PROMPT_HISTORY: parking_lot::Mutex<std::collections::VecDeque<String>> =
    parking_lot::Mutex::new(std::collections::VecDeque::new());

pub fn record_llm_prompt(prompt: &str) {
    let mut h = LLM_PROMPT_HISTORY.lock();
    // Dedupe consecutive identical entries.
    if h.back().map(|s| s.as_str()) == Some(prompt) {
        return;
    }
    h.push_back(prompt.to_string());
    while h.len() > 16 {
        h.pop_front();
    }
}

pub fn recent_llm_prompts(n: usize) -> Vec<String> {
    let h = LLM_PROMPT_HISTORY.lock();
    h.iter().rev().take(n).cloned().collect()
}

/// Build and show the main window.
pub fn build_window(app: &adw::Application, cfg: &Config) -> Result<()> {
    let engine = KeyboardEngine::new();

    // Build the window with the minimum needed to construct it. We DO NOT
    // set width/height/title here because any of those can trigger surface
    // realization on some GTK4 versions, and we need to call
    // `init_layer_shell()` before the surface exists.
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .build();

    // FIRST THING after constructing the window: try to make it a layer
    // surface. After this point, any GTK call that realizes the surface
    // will use the right role.
    let placement = configure_window_placement(&window);
    info!("Window placement: {:?}", placement);

    // Now safe to set title/sizing.
    window.set_title(Some("RustyBoard"));

    // The window itself doesn't need a header in keyboard mode — we have a
    // small `ToolbarView` so that the styled background extends edge to edge.
    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.set_top_bar_style(adw::ToolbarStyle::Flat);

    // Optional thin top bar showing word suggestions (the "ribbon").
    // Off by default — the suggestions engine isn't actually wired up
    // to the keys yet, so the bar is just empty pills wasting vertical
    // space on phone. Turn on via Settings → Appearance.
    if cfg.appearance.show_suggestions {
        let suggestions_bar = build_suggestions_bar();
        toolbar_view.add_top_bar(&suggestions_bar);
    }

    // The keyboard view itself fills the rest.
    // Initialize key-visibility flags from config first so the
    // KeyboardView's initial rebuild_view sees them and filters
    // out the corresponding keys.
    set_voice_key_visible(cfg.voice.enabled);
    set_llm_key_visible(cfg.llm.enabled);
    set_voice_log_enabled(cfg.voice.log_transcripts);
    set_focus_show_tray(cfg.behavior.focus_show_tray);
    set_auto_hide_enabled(cfg.behavior.auto_hide_on_no_focus.unwrap_or(true));
    set_glass_effect(cfg.appearance.glass_effect);
    crate::keyboard::state::set_auto_cap_enabled(cfg.behavior.auto_capitalize);
    crate::gestures::cardinal::set_shift_swipe_enabled(cfg.gestures.shift_swipe);
    crate::gestures::recognizer::set_lenient_swipe_enabled(cfg.gestures.lenient_swipe);
    crate::gestures::cardinal::set_cardinal_lenient(cfg.gestures.lenient_swipe);
    crate::gestures::cardinal::set_cardinal_diagonals(cfg.gestures.cardinal_diagonals);
    crate::gestures::cardinal::set_cardinal_diagnostic(cfg.gestures.cardinal_diagnostic);
    crate::gestures::swipe::set_swipe_speed_mode(&cfg.gestures.swipe_speed_mode);
    crate::gestures::swipe::set_swipe_auto_space(cfg.gestures.swipe_auto_space);
    crate::gestures::swipe::set_swipe_typing_enabled(cfg.gestures.swipe_typing);
    crate::ui::swipe_trail::set_trail_debug_styling(cfg.appearance.debug_trail_styling);
    // AI swipe assist: load AdaptiveDictionary + NgramModel from disk
    // (idempotent — once-loaded, repeated calls are no-ops) and sync
    // the master + sub-toggles from config. Loading is best-effort;
    // missing or corrupt persistence files yield empty models that
    // the recognizer will treat as no-op.
    crate::gestures::ai_swipe::init();
    crate::gestures::ai_swipe::set_ai_swipe_assist_enabled(cfg.gestures.ai_swipe_assist);
    crate::gestures::ai_swipe::set_adaptive_dictionary_enabled(
        cfg.gestures.adaptive_dictionary_enabled,
    );
    crate::gestures::ai_swipe::set_ngram_assist_enabled(cfg.gestures.ngram_assist_enabled);
    crate::keyboard::character_draw::load_user_templates();

    let keyboard_view = std::rc::Rc::new(
        KeyboardView::new(engine.clone(), cfg.clone()),
    );
    let kb_widget = keyboard_view.widget();
    toolbar_view.set_content(Some(&kb_widget));

    window.set_content(Some(&toolbar_view));

    // Pick a virtual-keyboard backend for delivering keystrokes. On
    // Wayland we try to construct the real backend (which also gives us
    // input-method-v2 events for show/hide). If that fails, fall back
    // through xdotool/noop.
    let vkbd: Box<dyn crate::compositor::VirtualKeyboard> =
        build_vkbd_with_im_callback(&window, cfg, &engine, &keyboard_view);
    info!("Virtual keyboard backend: {}", vkbd.backend_name());

    // Wire engine events → virtual-keyboard backend so committed text
    // actually reaches the focused application.
    spawn_engine_to_vkbd_bridge(&engine, vkbd);

    // Wire the Voice key → whisper transcription bridge. Logs a
    // warning and continues if no Whisper model is present; the
    // key itself stays visible but inert.
    //
    // The bridge also gets a handle to the microterm so it can
    // show a "Listening..." indicator + live partial transcript
    // during recording, then hide on completion.
    spawn_voice_bridge(&engine, cfg, keyboard_view.microterm_handle());

    // Wire the LLM bridge. Subscribes to KeyAction::Llm taps and
    // EngineEvent::LlmRequest dispatches, runs HTTP requests on a
    // worker thread, posts results back through the microterm or
    // commits via the engine.
    spawn_llm_bridge(&engine, cfg, keyboard_view.microterm_handle());

    // Microterm bridge — drains status toasts and steno suggestion
    // posts. Spawned UNCONDITIONALLY (not inside the voice bridge)
    // because steno + status messages exist in non-voice builds
    // (full-keyboard = basic + input-modes, no voice). Previously the
    // status drain lived inside spawn_voice_bridge, so non-voice
    // builds silently dropped every status post.
    spawn_microterm_bridge(&engine, keyboard_view.microterm_handle());

    // Apply user theme preference.
    apply_theme(&cfg.appearance.theme);

    // Load custom CSS for slick styling.
    crate::ui::theme::install_css_provider(&cfg.appearance, &cfg.layout);

    // For toplevel fallback we still need to size the window to something
    // sensible. Layer-shell handles its own sizing via anchors+exclusive
    // zone, so don't override there.
    if matches!(placement, Placement::Toplevel) {
        window.set_default_size(1080, 420);
    }

    // Invalidate the hitbox cache whenever the window geometry changes
    // (display rotation, monitor switch, layer-shell anchor recompute).
    // The cached key hitboxes are computed for a specific allocated
    // width/height; after a resize they'd route taps to the wrong keys.
    // The focus reconciler also invalidates on every focus event, so
    // most paths are covered — but a user keeping the keyboard visible
    // (auto_hide off) through a rotation wouldn't trigger that.
    // Invalidate the hitbox cache on any dimension change, but ONLY
    // when the value actually changes. GTK fires *_notify callbacks
    // on every layout pass — including ones that converge on the same
    // value (multi-step resize transitions, sub-pixel jitter from
    // layer-shell anchor recompute, redundant set_default_size calls).
    // Each redundant bump forces the next drag_begin to recompute
    // ~30 widget bounds via compute_bounds, a per-keypress overhead
    // that was effectively wasted work.
    //
    // The cached key hitboxes are computed for a specific allocated
    // width/height; after a real resize they'd route taps to the wrong
    // keys. The focus reconciler also invalidates on every focus
    // event, so most paths are covered — but a user keeping the
    // keyboard visible (auto_hide off) through a rotation wouldn't
    // trigger that.
    {
        let last_w: std::rc::Rc<std::cell::Cell<i32>> =
            std::rc::Rc::new(std::cell::Cell::new(window.default_width()));
        let last_h: std::rc::Rc<std::cell::Cell<i32>> =
            std::rc::Rc::new(std::cell::Cell::new(window.default_height()));
        let last_w_for_cb = last_w.clone();
        window.connect_default_width_notify(move |w| {
            let cur = w.default_width();
            if cur != last_w_for_cb.get() {
                last_w_for_cb.set(cur);
                crate::ui::keyboard_view::invalidate_hitbox_cache();
            }
        });
        let last_h_for_cb = last_h.clone();
        window.connect_default_height_notify(move |w| {
            let cur = w.default_height();
            if cur != last_h_for_cb.get() {
                last_h_for_cb.set(cur);
                crate::ui::keyboard_view::invalidate_hitbox_cache();
            }
        });
    }

    // If the backend supports input-method-v2 we want the keyboard to
    // Initial visibility. When auto-hide is on (the default), START
    // HIDDEN — the compositor's Activate event surfaces the keyboard
    // when the user focuses an editable widget. We deliberately do
    // NOT gate this on the v2-trust flag anymore: gating left the
    // keyboard VISIBLE at login on any backend not positively
    // identified as v2 (the "keyboard opens when I log in" bug). A
    // keyboard covering the screen at login is far worse than a
    // one-beat delay before it appears on first focus. If a backend
    // genuinely never fires Activate, the user can disable auto-hide
    // in settings (or it can be defaulted off per-environment).
    let auto_hide = cfg.behavior.auto_hide_on_no_focus.unwrap_or(true);
    window.present();
    if auto_hide {
        // Hide immediately after present; the Activate event re-shows.
        window.set_visible(false);
    }

    Ok(())
}

/// Whether we trust the Wayland backend's Activate/Deactivate events
/// enough to drive UI changes without further evidence. v2's
/// double-buffered protocol with explicit Done events is reliable
/// here. v1 (Plasma Mobile) fires Activate on window-level focus
/// before any cursor lands in a field, so we treat v1 events as
/// suggestive — they still trigger showing the keyboard, but the
/// tray-vs-full decision waits for stronger evidence (SurroundingText
/// arrival, or a settled-state delay).
#[cfg(feature = "wayland")]
pub fn wayland_backend_trusts_activate() -> bool {
    WAYLAND_BACKEND_TRUSTS_ACTIVATE.load(std::sync::atomic::Ordering::Relaxed)
}
#[cfg(not(feature = "wayland"))]
pub fn wayland_backend_trusts_activate() -> bool {
    // Without the wayland feature, we never see Activate events at
    // all (xdotool / noop backends). Default true so any non-wayland
    // test code doesn't accidentally trip the v1-tightening path.
    true
}

#[cfg(feature = "wayland")]
pub fn set_wayland_backend_trusts_activate(trusted: bool) {
    WAYLAND_BACKEND_TRUSTS_ACTIVATE
        .store(trusted, std::sync::atomic::Ordering::Relaxed);
}
#[cfg(not(feature = "wayland"))]
#[allow(dead_code)]
pub fn set_wayland_backend_trusts_activate(_trusted: bool) {}

/// Set to true if the backend in use has a well-tested Activate/Deactivate
/// event flow (currently v2 only). v1 (Plasma Mobile) fires Activate
/// pre-focus, so we layer extra evidence (SurroundingText) before
/// committing to a tray-vs-full decision.
#[cfg(feature = "wayland")]
static WAYLAND_BACKEND_TRUSTS_ACTIVATE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Construct the virtual-keyboard backend, wiring in an input-method
/// callback that shows/hides the keyboard window on focus events,
/// AND a surrounding-text callback that pumps editor context into
/// the engine state so LLM tasks have real context.
///
/// `keyboard_view` is passed in so the active callback can swap the
/// layout to the compact arrow-keys tray (per `cfg.behavior.focus_show_tray`).
/// The tray-mode keyboard stays visible but presents only a minimal
/// nav/dismiss surface until the user taps "ABC" to summon the full
/// keyboard.
#[cfg(feature = "wayland")]
fn build_vkbd_with_im_callback(
    window: &adw::ApplicationWindow,
    _cfg: &Config,
    engine: &KeyboardEngine,
    keyboard_view: &std::rc::Rc<crate::ui::keyboard_view::KeyboardView>,
) -> Box<dyn crate::compositor::VirtualKeyboard> {
    use crate::compositor::virtual_keyboard::{
        VirtualKeyboardBackend, WaylandVirtualKeyboard,
    };

    let detected = VirtualKeyboardBackend::detect();
    if !matches!(detected, VirtualKeyboardBackend::Wayland) {
        return detected.build();
    }

    let win = window.clone();
    let kb_view_for_cb = keyboard_view.clone();
    let engine_for_cb = engine.clone();
    let cb: crate::compositor::wayland_backend::InputMethodActiveCallback =
        std::rc::Rc::new(move |active| {
            // auto_hide gates VISIBILITY only — even when the user
            // has disabled auto-hide (keyboard always visible), focus
            // events still represent a meaningful editing-context
            // boundary. We MUST run the per-focus housekeeping
            // (steno buffer clear, undo history clear, microterm
            // cancel, focus-gen bump, hitbox invalidation) regardless
            // of auto_hide, so a focus-loss-then-focus to a different
            // field doesn't bleed state from the previous editing
            // context. Pre-fix: this whole callback early-returned
            // when auto_hide was off, leaving stale undo entries that
            // would emit backspaces against unrelated content in the
            // new field on the next /undo.
            let auto_hide = auto_hide_enabled();
            let tray_mode = focus_show_tray_enabled();
            // Honor the user's tray-mode preference for most fields,
            // but override for content types that NEED a full keyboard
            // — terminals being the obvious case. A user opening a
            // terminal app wants to type commands, not navigate with
            // an arrow pad. The content_purpose hint comes through the
            // wayland input-method protocol's ContentType event.
            let tray_mode = tray_mode && !focus_needs_full_keyboard();
            // Focus is changing (in either direction). Any in-progress
            // steno phoneme buffer belongs to the PREVIOUS editing
            // context and must not survive — otherwise tapping a
            // suggestion later would send backspaces at the new
            // cursor/field and delete unrelated content. Clear it on
            // every focus transition. (No-op in non-steno builds; the
            // steno module is always compiled.)
            crate::steno::global().clear_buffer();
            // Same hazard, different surface: a steno suggestion payload
            // posted but not yet drained by the 150-800ms suggestion
            // pump would otherwise surface in the new field. The chip
            // tap callback uses the live (now empty) steno buffer to
            // compute backspaces — so a stale chip tap would commit
            // its word as fresh text into a context that didn't ask
            // for it. Drop the pending suggestion explicitly.
            clear_steno_suggestions();
            // Same hazard, different bridge: any in-flight LLM or
            // voice request (1-10s round trip) belongs to the editing
            // context that originated it. If the user changes focus
            // mid-flight, signalling cancel drops the pending response
            // so it doesn't commit into the new field. The bridges'
            // cancel handlers restore any popped text (LLM rewrite/
            // Cancel in-flight LLM / voice requests and clear any
            // stale microterm content from the PREVIOUS focus. Voice
            // and LLM partials hide themselves via their cancel
            // handlers when they observe the bumped cancel_generation;
            // the steno suggestion ribbon, however, lives on its own
            // 5-second linger timer and would otherwise persist into
            // the new focus context. A user tap on a stale suggestion
            // would invoke the steno_tap_callback against the wrong
            // field, deleting unrelated chars and committing the
            // ribbon word into a context that didn't ask for it.
            // Explicit hide here covers all microterm content kinds
            // uniformly.
            request_microterm_cancel();
            kb_view_for_cb.microterm_handle().hide();
            // Undo history is also per-editing-context: an undo in the
            // new field must not emit backspaces against entries from
            // the previous field (would delete unrelated chars). Also
            // resets last_commit_was_swipe so a swipe in the new field
            // doesn't auto-prepend a space from the prior context.
            engine_for_cb.clear_undo_history();
            // Focus generation: bump on EVERY transition (active or
            // not). Any deferred tray-check from a previous focus
            // captures the gen at scheduling time and bails if a
            // newer focus has bumped it — protects against the
            // 80ms-defer landing in a different editing context.
            let my_gen = FOCUS_GENERATION
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .wrapping_add(1);
            // Belt-and-suspenders: invalidate the hitbox cache on
            // every focus transition. The cache is normally bumped
            // by rebuild_view, but window resize / rotation / monitor
            // change can move widgets without a rebuild firing. A
            // focus event is the natural cue for "something
            // potentially changed about the editing context, including
            // the screen geometry"; bumping here picks up any drift.
            crate::ui::keyboard_view::invalidate_hitbox_cache();
            // AI swipe assist: clear RECENT_WORDS so n-gram context
            // doesn't leak across fields. "thank you" at the end of
            // a Slack message shouldn't bias the score in a URL bar
            // afterward.
            crate::gestures::ai_swipe::note_focus_change();
            if active {
                tracing::debug!("input-method active → showing keyboard");
                // Safety-net reset of any stranded swipe latch from
                // before this focus event. Bound to activate-only
                // (not on deactivate-or-activate as before) because
                // Plasma Mobile fires spurious Deactivate events
                // during normal usage; resetting on every transition
                // would clear SWIPE_GESTURE_ACTIVE mid-swipe and
                // break the cardinal/steno dispatch (the dead-zone
                // recovery and was_evicted check both need it).
                //
                // Normal completion paths (drag_end, drag_cancel)
                // already clear the latch when a gesture finishes,
                // so resetting at every focus transition was only
                // covering the rare case of a totally stranded
                // gesture (GTK gesture system breakage). Activate-
                // only still covers that case — the next time the
                // user focuses any field, the latch clears.
                //
                // CONDITIONAL: only reset the latch when no swipe is
                // currently in flight. Plasma Mobile fires spurious
                // Activate events DURING a swipe (the
                // Deactivate→Activate spurious cycle); resetting
                // SWIPE_GESTURE_ACTIVE there would clear the
                // "real movement seen" flag mid-gesture, which
                // breaks was_evicted in drag_end's dead-zone
                // recovery and lets short cardinal flicks drop
                // silently. The clear-stale-latch purpose is only
                // useful when no current gesture owns the latch, so
                // gate it on swipe_gesture_active().
                if !crate::ui::keyboard_view::swipe_gesture_active() {
                    crate::ui::keyboard_view::reset_swipe_latch();
                }
                // Also clear the focus-loss flag — same rationale.
                // The Deactivate handler sets this when a swipe is in
                // flight; without clearing it on Activate, a spurious
                // Deactivate→Activate cycle (which Plasma Mobile
                // fires during normal usage per the comment above)
                // leaves the flag stuck, and the eventual drag_end
                // sees it and aborts the dispatch silently. That's
                // the long-suspected "trail renders but no commit"
                // bug. Pairing clear-on-Activate with set-on-
                // Deactivate makes the signal transient — only a
                // PERSISTENT focus loss (Deactivate without a
                // subsequent Activate before drag_end) triggers an
                // abort, which is the original intent. If the user
                // genuinely switched fields mid-swipe, the in-flight
                // word may commit to the new field — that's a less-
                // bad failure mode than dropping every swipe.
                //
                // Unlike the latch above, this clear is unconditional:
                // a stuck focus-loss flag has no legitimate purpose
                // (it's a single signal from a specific D event), so
                // there's nothing to preserve.
                let had_focus_loss =
                    crate::ui::keyboard_view::take_swipe_interrupted_by_focus_loss();
                if had_focus_loss {
                    tracing::info!(
                        "Activate cleared stale focus-loss flag (spurious \
                         Deactivate→Activate cycle during a swipe)"
                    );
                }
                // Reset the SurroundingText-seen flag for this new
                // focus. We'll check it in the 80ms reconciler — if
                // SurroundingText has arrived by then, this is a
                // real edit context; if not, it might be a
                // speculative activate (Plasma Mobile v1 fires
                // Activate on window-level focus before any field
                // is actually selected), and we should default to
                // the full keyboard rather than the arrow tray.
                reset_surrounding_text_seen();
                // Keyboard appearing is a weak proximity signal for
                // voice — warm the FSM so a quick reach for the mic
                // key is instant. Lapses back to Asleep on its own
                // if voice isn't used within the linger window.
                crate::voice::readiness::global().signal_proximity();
                // Show the FULL keyboard — but ONLY immediately if we
                // trust this compositor's Activate event. On v2 we
                // trust it; on v1 (Plasma Mobile), Activate fires
                // speculatively on window-level focus before any
                // cursor lands in a field, so popping the keyboard up
                // would cover the screen for fields the user didn't
                // even select. Defer the show until SurroundingText
                // arrives (the strong "real edit context" signal).
                // If text never arrives, we never show — the user can
                // tap a field explicitly to force a real focus.
                let v2_trusted = wayland_backend_trusts_activate();
                if auto_hide && v2_trusted {
                    win.set_visible(true);
                } else if auto_hide && !v2_trusted {
                    // v1 path: arm a deferred show that fires when
                    // SurroundingText has arrived, OR we hit the
                    // settling timeout (350ms) and skip the show.
                    let win_clone = win.clone();
                    let defer_gen = my_gen;
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(80),
                        move || {
                            // Stale-defer guard: a newer focus may
                            // have arrived in the interim.
                            let cur = FOCUS_GENERATION.load(
                                std::sync::atomic::Ordering::Relaxed,
                            );
                            if cur != defer_gen {
                                return;
                            }
                            // Only show if SurroundingText confirmed.
                            // (If it lands later, the late re-check
                            // at 350ms will also try to show.)
                            if surrounding_text_seen_since_activate() {
                                win_clone.set_visible(true);
                            }
                        },
                    );
                }
                // ALWAYS schedule the deferred reconciler, regardless of
                // what tray_mode evaluated to at activate time. The
                // defer is the single source of truth for which layout
                // we should be in: it re-reads preferences AND
                // content_purpose at +80ms (giving ContentType time to
                // arrive) and does the right swap. show_arrows_tray /
                // show_full_keyboard both early-return when already in
                // the target layout, so the no-op case is free.
                //
                // We used to only schedule when tray_mode was true at
                // activate, which broke this sequence: text field
                // (showing arrows) → terminal where purpose=Terminal
                // arrived before activate evaluation → tray_mode=false
                // → no defer → stuck on arrows for the terminal.
                {
                    let kb_view = kb_view_for_cb.clone();
                    let win_defer = win.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(80),
                        move || {
                            // Stale-defer guard: skip if a newer focus
                            // event has arrived since we were scheduled.
                            // Without this, a defer from a previous
                            // (now-stale) focus could swap to the
                            // arrow tray for the current focus's field
                            // — e.g. for a terminal where the user
                            // wants the full keyboard.
                            let cur_gen = FOCUS_GENERATION
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if cur_gen != my_gen {
                                tracing::debug!(
                                    "deferred tray-check stale (gen {} vs {}); skipping",
                                    my_gen,
                                    cur_gen
                                );
                                return;
                            }
                            // Re-read content_purpose now that
                            // ContentType has had a chance to arrive.
                            // The defer is the authoritative reconciler
                            // — it always evaluates BOTH directions:
                            //
                            //   - field needs full keyboard (terminal,
                            //     etc.): swap arrows → full if we were
                            //     left on arrows from a prior session.
                            //   - field doesn't need full + tray pref
                            //     on + SurroundingText arrived: swap
                            //     full → arrows.
                            //
                            // Without the arrows→full branch, focusing
                            // a terminal after a text field used to
                            // leave the keyboard stuck on arrows.
                            // show_arrows_tray and show_full_keyboard
                            // both early-return when already in the
                            // target layout, so the no-op case is free.
                            //
                            // The SurroundingText gate guards against
                            // Plasma Mobile's v1 IM protocol firing
                            // Activate on window-level focus before any
                            // cursor has actually landed in a field —
                            // without it, tray-mode users would see
                            // arrows pop up for unfocused fields just
                            // because they're on screen. We ALWAYS
                            // schedule a SECOND check at 350ms total
                            // (270ms from here) that re-evaluates all
                            // conditions, swapping in either direction:
                            //   - arrows → full, if a slow-arriving
                            //     ContentType=Terminal lands after we
                            //     showed arrows.
                            //   - full → arrows, if SurroundingText
                            //     arrives late (real edit context
                            //     finally confirmed after we defaulted
                            //     to full).
                            // Stage1 (at 80ms): always default to the
                            // FULL keyboard. Never choose arrows here.
                            //
                            // Why: arrows is an "opinionated" choice
                            // that's annoying when wrong — popping the
                            // tray up for a field the user didn't tap
                            // (Plasma Mobile fires Activate for many
                            // auto-focused fields) means the user has
                            // to manually swap to full to type. The
                            // full keyboard, by contrast, is annoying
                            // when arrows would have been right, but
                            // it doesn't BLOCK typing.
                            //
                            // So stage1 conservatively shows full and
                            // lets stage2 (at +270ms = 350ms total
                            // from activate) make the arrows decision
                            // with more confidence. Pre-fix, stage1
                            // chose arrows the instant SurroundingText
                            // arrived — which on Plasma Mobile is too
                            // eager because every on-screen field
                            // emits SurroundingText.
                            let initial_decision = if focus_needs_full_keyboard() {
                                kb_view.show_full_keyboard();
                                "full-specialized-purpose"
                            } else {
                                kb_view.show_full_keyboard();
                                "full-stage1-default"
                            };
                            tracing::debug!(
                                "tray reconciler stage1: {}",
                                initial_decision
                            );
                            // Schedule the late re-check unconditionally.
                            // It's cheap (one atomic read + branch) and
                            // catches all late-arrival cases.
                            //
                            // Also handles the v1 "show keyboard only
                            // when text confirmed" case: if
                            // SurroundingText arrived after the 80ms
                            // initial defer (i.e., a real field landed
                            // a bit late on Plasma Mobile), we need to
                            // explicitly make the window visible here
                            // — otherwise the keyboard would stay
                            // hidden despite a confirmed edit context.
                            let kb_view_late = kb_view.clone();
                            let win_late = win_defer.clone();
                            let late_gen = my_gen;
                            let late_auto_hide = auto_hide;
                            let late_v2_trusted = v2_trusted;
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(270),
                                move || {
                                    let cur = FOCUS_GENERATION.load(
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    if cur != late_gen {
                                        return;
                                    }
                                    // v1 + auto_hide: if text confirmed
                                    // but we never made the window
                                    // visible (because the 80ms defer
                                    // ran before the text arrived),
                                    // do it now.
                                    if late_auto_hide
                                        && !late_v2_trusted
                                        && surrounding_text_seen_since_activate()
                                        && !win_late.is_visible()
                                    {
                                        tracing::debug!(
                                            "v1 late show: text arrived after 80ms, \
                                             revealing keyboard"
                                        );
                                        win_late.set_visible(true);
                                    }
                                    // Re-evaluate layout choice and
                                    // apply. This is now the AUTHORITATIVE
                                    // tray decision — stage1 always
                                    // defaults to full, so stage2 is
                                    // where arrows can win.
                                    //
                                    // Both show_* functions early-
                                    // return when already in target,
                                    // so the common no-op case is
                                    // free.
                                    let (want, why) = if !focus_show_tray_enabled() {
                                        ("full", "pref-off")
                                    } else if focus_needs_full_keyboard() {
                                        ("full", "specialized-purpose")
                                    } else if surrounding_text_has_content_since_activate() {
                                        // Real edit context WITH text → arrows
                                        // tray is genuinely useful for cursor
                                        // navigation.
                                        ("arrows", "text-with-content-seen")
                                    } else if surrounding_text_seen_since_activate() {
                                        // Real edit context but EMPTY (search
                                        // bar, fresh draft) → default to full
                                        // keyboard. User is about to type,
                                        // not navigate. Without this, search
                                        // menus appear with the arrow tray
                                        // and no letters — the
                                        // "empty keyboard on search menu"
                                        // user-reported bug.
                                        ("full", "text-seen-but-empty")
                                    } else {
                                        ("full", "no-text-after-350ms")
                                    };
                                    tracing::debug!(
                                        "tray reconciler stage2: {} ({})",
                                        want, why
                                    );
                                    if crate::gestures::cardinal::cardinal_diagnostic_enabled() {
                                        // Visible on-device. Helps diagnose
                                        // "empty keyboard on search menu":
                                        // if the user sees a layout choice
                                        // they didn't expect (e.g. arrows
                                        // when they wanted full), this
                                        // surfaces it.
                                        crate::ui::window::post_microterm_status(format!(
                                            "focus reconciler: → {} ({})",
                                            want, why
                                        ));
                                    }
                                    match want {
                                        "arrows" => kb_view_late.show_arrows_tray(),
                                        _ => kb_view_late.show_full_keyboard(),
                                    }
                                },
                            );
                        },
                    );
                }
            } else {
                tracing::debug!("input-method inactive → hiding keyboard");
                // If a swipe is in flight when focus leaves the
                // field, signal the drag handler to abort the
                // dispatch. Without this, drag_end would commit
                // the swipe word to whatever field is focused NOW
                // (could be a search bar, a different chat, etc).
                // Only fire when a swipe is actually active:
                // otherwise we'd flag every deactivate, which is
                // wasted work and could mask other state issues.
                if crate::ui::keyboard_view::swipe_gesture_active() {
                    crate::ui::keyboard_view::note_focus_lost_during_swipe();
                }
                // Only hide if auto_hide is on; otherwise the keyboard
                // is meant to stay visible regardless of focus state.
                if auto_hide {
                    win.set_visible(false);
                }
                if tray_mode {
                    kb_view_for_cb.show_full_keyboard();
                }
                // Reset content_purpose on deactivate so a stale value
                // from the previous field doesn't bleed into the next
                // activate's pre-ContentType window.
                set_content_purpose(255);
                // Persist any learned vocab/n-gram updates from this
                // field session. The keyboard process can be killed
                // by the session manager any time after deactivate
                // (the user just stopped typing) and we'd lose words
                // the 60s background timer hadn't caught yet.
                // flush_to_disk is a no-op when nothing's dirty.
                crate::gestures::ai_swipe::flush_to_disk();
            }
        });

    // Surrounding-text callback: route into the engine.
    // The cursor/anchor values from the protocol are byte offsets
    // into the UTF-8 text. The engine's SurroundingTextChanged
    // input takes a character offset; we approximate by using
    // cursor as-is and let the engine handle conversion when it
    // needs the char offset. For now the engine just stores the
    // values verbatim.
    let engine_for_st = engine.clone();
    let st_cb: crate::compositor::wayland_backend::SurroundingTextCallback =
        std::rc::Rc::new(move |text, cursor, _anchor| {
            // Note that we got a real cursor position from the
            // compositor. This is the strong "we're in a real edit
            // context, not just a speculative window-focus event"
            // signal that the focus reconciler consults to decide
            // whether to honor tray-mode preference.
            note_surrounding_text_arrived();
            // Separately note whether the field has actual text
            // content. Empty-text fields (search bars, fresh drafts)
            // are real edit contexts but the user is about to TYPE,
            // not navigate — so the arrows tray is the wrong choice
            // for them. The focus reconciler stage2 uses this finer
            // signal to default to full keyboard for empty fields
            // even when tray-mode is enabled. User-reported
            // "keyboard spawns empty on search menu" — the search
            // bar focuses with empty surrounding text, the old
            // logic picked arrows because text was "seen", and the
            // user saw nav arrows where they expected letters.
            if !text.is_empty() {
                note_surrounding_text_has_content();
            }
            engine_for_st.handle_input(
                crate::keyboard::engine::EngineInput::SurroundingTextChanged {
                    text,
                    cursor: cursor as usize,
                },
            );
        });

    match crate::compositor::wayland_backend::try_new(cb, st_cb) {
        Ok(Some(backend)) => {
            // Mark whether we trust this backend's Activate events
            // enough to act on them without extra evidence. v2's
            // double-buffered protocol is reliable; v1 (Plasma Mobile)
            // fires Activate on window-level focus before any cursor
            // lands in a field, so the focus reconciler layers a
            // SurroundingText check on top of the activate signal.
            if backend.uses_v2() {
                set_wayland_backend_trusts_activate(true);
            }
            Box::new(WaylandVirtualKeyboard::with_backend(backend))
        }
        Ok(None) => {
            // Wayland session but no protocols / not a Wayland display.
            // Detect again and return whatever fallback fits.
            tracing::warn!(
                "Wayland backend init returned None — compositor may not \
                 support virtual_keyboard_v1 / input_method_v2. Falling \
                 back to xdotool/noop."
            );
            VirtualKeyboardBackend::detect_non_wayland().build()
        }
        Err(e) => {
            tracing::warn!("Wayland backend init failed: {}; falling back", e);
            VirtualKeyboardBackend::detect_non_wayland().build()
        }
    }
}

#[cfg(not(feature = "wayland"))]
fn build_vkbd_with_im_callback(
    _window: &adw::ApplicationWindow,
    _cfg: &Config,
    _engine: &KeyboardEngine,
    _keyboard_view: &std::rc::Rc<crate::ui::keyboard_view::KeyboardView>,
) -> Box<dyn crate::compositor::VirtualKeyboard> {
    crate::compositor::VirtualKeyboardBackend::detect().build()
}

/// Forward `EngineEvent::TextCommitted` and key-action events to the
/// virtual keyboard backend. Runs on a glib timeout (33ms / 30Hz) on the
/// main GTK thread, draining the event channel.
fn spawn_engine_to_vkbd_bridge(
    engine: &KeyboardEngine,
    vkbd: Box<dyn crate::compositor::VirtualKeyboard>,
) {
    // Direct-dispatch architecture (replaces the previous polled
    // bridge). The engine invokes our callback synchronously inside
    // handle_input, so wayland sees the commit in the same call as
    // the user's tap — no polling delay.
    //
    // Before this change: every keystroke went through an mpsc
    // channel and a self-adaptive timer (FAST_MS=16, IDLE_MS=60).
    // Worst-case first-keystroke-after-pause latency was up to 60ms
    // ON TOP OF wayland's own delivery time. Now it's zero on top.
    //
    // Two side benefits:
    //   1. No idle wakeups — the timer's gone. Frees a sliver of
    //      phone battery vs the old 7-Hz idle pump.
    //   2. No mpsc clone+send overhead for vkbd-bound events. The
    //      voice/llm/microterm subscribers still exist for their
    //      own needs.
    //
    // Threading note: handle_input is called from GTK main-thread
    // callbacks (key clicks, drag handlers, focus events). The
    // direct_dispatch callback runs on the same thread. No
    // cross-thread coordination needed; we move the vkbd Box
    // directly into the closure (the callback is FnMut-by-Fn-trait
    // via Cell wouldn't be safe, but we don't mutate vkbd state —
    // only call its &self methods).
    engine.set_direct_dispatch(std::rc::Rc::new(move |ev| {
        match ev {
            EngineEvent::TextCommitted(text) => {
                let n = text.chars().count();
                match vkbd.commit_text(text) {
                    Ok(()) => {
                        tracing::info!(
                            "vkbd commit_text ok: {} char{}",
                            n,
                            if n == 1 { "" } else { "s" }
                        );
                    }
                    Err(e) => {
                        tracing::warn!("vkbd commit_text failed: {}", e);
                    }
                }
            }
            EngineEvent::KeyAction(action) => match action {
                KeyAction::Backspace => {
                    let _ = vkbd.press_keysym("BackSpace");
                }
                KeyAction::Return => {
                    let _ = vkbd.press_keysym("Return");
                }
                KeyAction::Tab => {
                    let _ = vkbd.press_keysym("Tab");
                }
                KeyAction::Keysym(name) => {
                    let _ = vkbd.press_keysym(name);
                }
                _ => {}
            },
            _ => {}
        }
    }));
}


/// Spawn the voice-input bridge. Subscribes to engine events; the
/// `KeyAction::Voice` event toggles recording on a `VoiceService`
/// instance. When recording stops, the transcribed text is fed back
/// to the engine as `EngineInput::VoiceFinal`, which the engine turns
/// into a `TextCommitted` event that the vkbd bridge delivers to the
/// focused app.
///
/// Returns `Ok(())` if voice was wired up. Returns `Ok(())` with a
/// warning log if no Whisper model was found — the Voice key will
/// still be present on the keyboard but tapping it will no-op. We
/// don't fail the whole window build for missing voice support.
///
/// Threading: the VoiceService lives on the main GTK thread inside
/// an Rc<RefCell<Option<...>>>. Transcription itself blocks the
/// thread when run synchronously, so we offload `stop_recording`
/// (which includes inference) to a background thread and post the
/// result back via a `glib::MainContext::invoke`.
#[cfg(feature = "voice")]
fn spawn_voice_bridge(
    engine: &KeyboardEngine,
    cfg: &Config,
    microterm: std::rc::Rc<crate::ui::microterm::MicrotermView>,
) {
    use crate::ui::microterm::MicrotermContent;
    use crate::voice::{VoiceService, VoiceServiceState};
    use std::cell::RefCell;
    use std::rc::Rc;

    let service: Rc<RefCell<Option<VoiceService>>> = Rc::new(RefCell::new(None));

    // Try to construct the service eagerly. If the model is missing,
    // log and leave the Option as None — the Voice key will no-op
    // until the user installs a model.
    match VoiceService::new(cfg.voice.clone()) {
        Ok(svc) => {
            info!("Voice service ready (whisper backend loaded)");
            *service.borrow_mut() = Some(svc);
        }
        Err(e) => {
            tracing::warn!(
                "Voice unavailable: {} (Voice key will be present but inert)",
                e
            );
            tracing::warn!(
                "To enable voice: download a Whisper ggml model (e.g. \
                 ggml-base.en-q5_1.bin from \
                 https://huggingface.co/ggerganov/whisper.cpp) into one of: \
                 ~/.local/share/rusty-board/models/, \
                 ~/.config/web2qml/models/, or /usr/share/whisper/models/"
            );
        }
    }

    let rx = engine.subscribe();
    let service_clone = service.clone();
    let microterm_clone = microterm.clone();

    // Wire chip-tap-commits-word. Voice chips in the microterm are
    // both draggable (for spatial commit onto space/backspace) and
    // tappable (this callback). Tap is the fast path: one tap on a
    // word commits it + a trailing space, without needing the user
    // to drag a chip across the screen.
    {
        let engine_for_tap = engine.clone();
        microterm.set_word_tap_callback(Box::new(move |mut word| {
            if !word.ends_with(char::is_whitespace) {
                word.push(' ');
            }
            engine_for_tap.handle_input(
                crate::keyboard::engine::EngineInput::SwipeWord(word),
            );
        }));
    }

    // Pending inference receiver. Set when stop_recording_async is
    // called; polled by the event-pump timer below; cleared when a
    // result arrives. Lets the bridge offload whisper inference
    // (which blocks 200-500ms on phone) to a worker thread without
    // freezing the GTK main loop.
    let pending_for_pump: std::rc::Rc<
        std::cell::RefCell<
            Option<std::sync::mpsc::Receiver<anyhow::Result<String>>>,
        >,
    > = std::rc::Rc::new(std::cell::RefCell::new(None));

    // Streaming-partial shared state. The streaming worker (spawned
    // when start_recording fires, terminated when stop_recording
    // fires) writes the latest partial transcript here; the level-
    // polling timer reads it and pushes it to the microterm.
    //
    // `stop_flag` is the worker's exit signal — flipped to true on
    // recording stop so the worker exits its loop cleanly. Shared
    // via Arc to cross thread boundaries.
    //
    // `latest_partial` is a Mutex<String> the worker writes after
    // each inference pass. We use Mutex<String> rather than an mpsc
    // channel because we only ever care about the LATEST partial —
    // not a backlog. The lock is contended only between the worker
    // (one write per ~600ms) and the UI poll (one read per 33ms),
    // so contention is negligible.
    let streaming_stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool> =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let latest_partial: std::sync::Arc<parking_lot::Mutex<String>> =
        std::sync::Arc::new(parking_lot::Mutex::new(String::new()));

    // Streaming partial transcription enable: initialize the
    // process-wide flag from config. The bridge reads it on each
    // Voice tap, so toggling the setting at runtime takes effect on
    // the next recording session.
    VOICE_STREAMING_ENABLED.store(
        cfg.voice.streaming,
        std::sync::atomic::Ordering::Relaxed,
    );
    let streaming_interval_ms = cfg.voice.streaming_interval_ms.max(200);

    // ---- Level-polling timer (33ms = ~30Hz) ----
    // Polls the recorder's peak-level meter while recording is
    // active, AND reads the latest streaming partial transcript
    // produced by the worker thread. Pushes both to the microterm's
    // VoicePreview content so the user sees a live VU meter + the
    // text-so-far as they speak.
    //
    // We deliberately keep this separate from the event-pump timer
    // so a slow inference call inside the event handler doesn't
    // stall the meter (or vice versa). Both run on the GTK main
    // thread but interleaved by glib's event loop.
    {
        let service_for_level = service.clone();
        let microterm_for_level = microterm.clone();
        let partial_for_level = latest_partial.clone();
        let last_level: Rc<std::cell::Cell<f32>> =
            Rc::new(std::cell::Cell::new(-1.0));
        let last_partial: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

        // Self-rescheduling pump. Instead of a fixed 33ms recurring
        // timer that wakes 30×/sec forever (even when voice is
        // never used), this re-arms itself at the cadence dictated
        // by the voice readiness FSM:
        //   Asleep → 250ms   (idle keyboard, ~4 wakes/sec)
        //   Warm   → 100ms   (voice likely soon)
        //   Active → 33ms    (recording, smooth meter)
        // The single tick of work is cheap (~50ns when not
        // recording), but eliminating 26 wakeups/sec at idle is a
        // meaningful battery + scheduling win on the FP5.
        //
        // The rearm closure is stored in an Rc<RefCell<Option<...>>>
        // so it can reschedule itself. We box it as a `dyn Fn()` and
        // the inner timeout_add_local_once invokes a clone.
        type Rearm = Rc<dyn Fn()>;
        let rearm_slot: Rc<RefCell<Option<Rearm>>> =
            Rc::new(RefCell::new(None));
        let rearm_slot_inner = rearm_slot.clone();

        let do_tick: Rearm = Rc::new(move || {
            // ---- one poll of work ----
            let svc_borrow = service_for_level.borrow();
            if let Some(svc) = svc_borrow.as_ref() {
                if svc.state() == crate::voice::VoiceServiceState::Recording {
                    let level = svc.current_level();
                    let partial = {
                        let p = partial_for_level.lock();
                        if p.is_empty() {
                            "Listening...".to_string()
                        } else {
                            p.clone()
                        }
                    };
                    let level_changed =
                        (level - last_level.get()).abs() >= 0.05;
                    let partial_changed =
                        *last_partial.borrow() != partial;
                    if level_changed || partial_changed {
                        last_level.set(level);
                        *last_partial.borrow_mut() = partial.clone();
                        microterm_for_level.set_content(
                            crate::ui::microterm::MicrotermContent::VoicePreview {
                                partial,
                                level,
                            },
                        );
                    }
                } else {
                    last_level.set(-1.0);
                    if !last_partial.borrow().is_empty() {
                        last_partial.borrow_mut().clear();
                    }
                }
            }
            drop(svc_borrow);

            // ---- re-arm at the FSM-dictated cadence ----
            let interval = crate::voice::readiness::global()
                .current()
                .poll_interval_ms();
            if let Some(rearm) = rearm_slot_inner.borrow().as_ref() {
                let rearm = rearm.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(interval),
                    move || rearm(),
                );
            }
        });

        *rearm_slot.borrow_mut() = Some(do_tick.clone());
        // Kick off the first tick.
        do_tick();
    }

    // (Status toasts + steno suggestions are drained by the
    // always-on spawn_microterm_bridge, not here, so they work in
    // non-voice builds too.)

    // ---- Auto-shutdown timer ----
    // Every 30 seconds, check if the voice service has been idle
    // long enough to release the transcriber from RAM. The model
    // is ~50-200MB; freeing it after 5 minutes of inactivity is
    // the user-requested battery-life win. Re-loads from disk
    // automatically on the next start_recording (costs ~1-2s).
    //
    // 30s polling cadence is fine: the 5-min timeout has 10
    // checks of slack, and 30s ticks are imperceptible relative
    // to the shutdown threshold.
    {
        let service_for_shutdown = service.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(30), move || {
            let svc_borrow = service_for_shutdown.borrow();
            if let Some(svc) = svc_borrow.as_ref() {
                if svc.is_transcriber_loaded()
                    && svc.idle_seconds() > crate::voice::VOICE_AUTO_SHUTDOWN_SECS
                {
                    let released = svc.release_resources();
                    if released {
                        tracing::info!(
                            "voice: auto-shutdown after {}s idle — transcriber released",
                            svc.idle_seconds()
                        );
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // ---- AI swipe assist persistence flush (60s cadence) ----
    // Periodically write any unsaved dictionary/n-gram updates to
    // disk. flush_to_disk() is a no-op when nothing's dirty, so
    // the 60s cadence is cheap; we just need SOMETHING to run
    // because we can't rely on a clean GTK shutdown to trigger
    // an at-exit flush (the keyboard process can be SIGKILL'd by
    // the compositor or session manager).
    //
    // 60s means we lose at most a minute of newly-learned vocab on
    // hard crash. Acceptable: the user can re-swipe lost words and
    // the model rebuilds quickly. Tighter cadences add I/O wear on
    // the (often flash-based) phone storage.
    glib::timeout_add_local(std::time::Duration::from_secs(60), || {
        crate::gestures::ai_swipe::flush_to_disk();
        glib::ControlFlow::Continue
    });

    // ---- Readiness-FSM coordinator (1Hz) ----
    // Ticks the voice readiness FSM once per second and acts on the
    // resulting state:
    //   - Warm  : pre-warm the transcriber so the first Voice tap is
    //             instant (no 1-2s cold load). Cheap if already
    //             loaded.
    //   - Asleep: release the transcriber. This is MORE aggressive
    //             than the 5-minute auto-shutdown above — the FSM
    //             decides voice is unlikely in the next few seconds
    //             (no proximity signals for WARM_LINGER_SECS), so we
    //             free the RAM promptly. The auto-shutdown timer
    //             remains as a backstop for the recording-just-ended
    //             case (FSM sits in Warm post-recording).
    //
    // 1Hz is a good cadence: state transitions are second-scale, the
    // tick is a couple atomic loads + maybe one model load/release,
    // and 1 wake/sec is negligible. Crucially, the load/release
    // decisions happen here so the high-frequency UI pump stays a
    // pure display loop.
    //
    // (This whole block lives inside the voice-gated
    // spawn_voice_bridge, so it only exists in voice builds. The
    // readiness FSM ITSELF is always compiled — other call sites
    // like the keyboard-show proximity signal use it feature-free —
    // but driving the transcriber lifecycle is voice-only.)
    {
        let service_for_fsm = service.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            let state = crate::voice::readiness::global().tick();
            let svc_borrow = service_for_fsm.borrow();
            if let Some(svc) = svc_borrow.as_ref() {
                match state {
                    crate::voice::readiness::Readiness::Warm => {
                        // Pre-warm if not loaded. Idempotent.
                        if !svc.is_transcriber_loaded() {
                            svc.prewarm_transcriber();
                        }
                    }
                    crate::voice::readiness::Readiness::Asleep => {
                        // Release promptly when the FSM is confident
                        // voice isn't imminent. release_resources is
                        // a no-op if state isn't Idle, so we won't
                        // yank the model mid-transcription.
                        if svc.is_transcriber_loaded() {
                            svc.release_resources();
                        }
                    }
                    crate::voice::readiness::Readiness::Active => {
                        // Recording — nothing to do; the model is
                        // loaded and in use.
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // Track cancel generation independently; tick fires only when
    // the global generation moves past our snapshot.
    let last_cancel_gen: Rc<std::cell::Cell<u32>> =
        Rc::new(std::cell::Cell::new(cancel_generation()));

    // Clones consumed by the event/cancel bridge timer below. (These
    // were previously defined next to the old fixed-cadence level
    // pump; the pump's refactor to FSM-cadence self-rescheduling
    // dropped them, but the event bridge still needs them — it
    // handles cancel, hides/updates the microterm, and feeds final
    // transcripts back into the engine.)
    let microterm_for_pump = microterm.clone();
    let engine_for_pump = engine.clone();

    // 100ms (10Hz). This timer drains engine events (Voice taps),
    // checks for cancel requests, and polls for completed inference
    // results. None of these need the 33ms cadence the level meter
    // wanted — a Voice tap registering within 100ms is
    // imperceptible, and inference completion is seconds-scale. The
    // level meter is handled separately by the FSM-gated pump above.
    // Dropping this from 30Hz to 10Hz removes ~20 wakeups/sec.
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        // ---- Honour cancel requests ----
        let now_gen = cancel_generation();
        let cancelled = now_gen != last_cancel_gen.get();
        if cancelled {
            last_cancel_gen.set(now_gen);
        }
        if cancelled {
            streaming_stop_flag
                .store(true, std::sync::atomic::Ordering::Relaxed);
            latest_partial.lock().clear();
            // Cancelled — relax the FSM out of Active.
            crate::voice::readiness::global().set_recording(false);
            let svc_borrow = service_clone.borrow();
            if let Some(svc) = svc_borrow.as_ref() {
                if svc.state() == crate::voice::VoiceServiceState::Recording {
                    svc.cancel_recording();
                }
            }
            *pending_for_pump.borrow_mut() = None;
            // Clear the LLM-divert flag too. If the user cancelled a
            // voice→LLM flow (mic tapped via ✨ button), the flag was
            // set; without clearing it here, the NEXT plain voice
            // recording would route its transcript to LLM instead of
            // committing as text. The cancel button is a universal
            // "abort whatever I just started" — that has to include
            // the LLM intent.
            set_voice_feeds_llm(false);
            // Also close the drawing canvas if it's open. ✕
            // becomes a universal "cancel whatever I just started"
            // affordance covering voice, LLM, and canvas.
            if crate::keyboard::character_draw::canvas_open() {
                crate::keyboard::character_draw::close_canvas();
            }
            // Hide the microterm — the user just cancelled.
            microterm_for_pump.hide();
        }

        // ---- Poll pending inference ----
        // If a previous stop_recording_async is still in flight,
        // check for its result here (non-blocking).
        let result_to_handle: Option<anyhow::Result<String>> = {
            let mut pending = pending_for_pump.borrow_mut();
            if let Some(rx) = pending.as_ref() {
                match rx.try_recv() {
                    Ok(r) => {
                        *pending = None; // consume the receiver
                        Some(r)
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        *pending = None;
                        Some(Err(anyhow::anyhow!(
                            "inference worker disconnected"
                        )))
                    }
                }
            } else {
                None
            }
        };
        if let Some(result) = result_to_handle {
            match result {
                Ok(text) if !text.is_empty() => {
                    // Strip trailing whisper artifacts (e.g.
                    // [BLANK_AUDIO]) before any other processing.
                    // These appear at the end of otherwise-real
                    // transcripts and should be silently removed
                    // rather than treated as part of the user's
                    // utterance.
                    let text = crate::voice::hallucination::strip_trailing_artifacts(&text);
                    if text.is_empty() {
                        microterm_for_pump.set_content(
                            MicrotermContent::Status(
                                "(silence — filtered)".to_string(),
                            ),
                        );
                        let mt_for_hide = microterm_for_pump.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(1200),
                            move || {
                                mt_for_hide.hide();
                            },
                        );
                        return glib::ControlFlow::Continue;
                    }
                    info!("Voice transcription: {:?}", text);
                    // For short final transcripts (≤ 4 words), apply
                    // the hallucination filter — whisper's silence
                    // hallucinations are almost always 1-3 words, so
                    // a short transcript that matches a canonical
                    // artifact is overwhelmingly likely to be just
                    // noise. Longer transcripts pass through:
                    // legitimate use of "thank you" or "bye" inside
                    // a real utterance shouldn't be dropped.
                    let word_count = text.split_whitespace().count();
                    let likely_hallucination = word_count <= 4
                        && crate::voice::hallucination::is_hallucination(
                            &text,
                        );

                    if likely_hallucination {
                        info!("Final voice filtered as hallucination: {:?}", text);
                        microterm_for_pump.set_content(
                            MicrotermContent::Status(
                                "(silence — filtered)".to_string(),
                            ),
                        );
                        let mt_for_hide = microterm_for_pump.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(1200),
                            move || {
                                mt_for_hide.hide();
                            },
                        );
                    } else if voice_feeds_llm() {
                        set_voice_feeds_llm(false);
                        log_voice_transcript(&text, "llm_prompt");
                        engine_for_pump.handle_input(
                            crate::keyboard::engine::EngineInput::LlmRequest(
                                crate::llm::LlmTask::Freeform(text),
                            ),
                        );
                    } else {
                        log_voice_transcript(&text, "commit");
                        engine_for_pump.handle_input(
                            crate::keyboard::engine::EngineInput::VoiceFinal(text),
                        );
                        microterm_for_pump.hide();
                    }
                }
                Ok(_) => {
                    microterm_for_pump.set_content(
                        MicrotermContent::Status(
                            "(no speech detected)".to_string(),
                        ),
                    );
                    let mt_for_hide = microterm_for_pump.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1500),
                        move || {
                            mt_for_hide.hide();
                        },
                    );
                }
                Err(e) => {
                    tracing::warn!("voice inference failed: {}", e);
                    microterm_for_pump.set_content(
                        MicrotermContent::Status(format!(
                            "Voice error: {}",
                            e
                        )),
                    );
                }
            }
        }

        // ---- Drain engine events ----
        while let Ok(ev) = rx.try_recv() {
            // Spacebar long-press → toggle the microterm status
            // display. Shows current keyboard state + voice
            // readiness; hides on the next long-press. The user can
            // also tap the keyboard normally to dismiss (it stays
            // visible until the next status-relevant change or a
            // second long-press).
            if let EngineEvent::KeyLongPress(ref k) = ev {
                if matches!(k.action, KeyAction::Space) {
                    if microterm_clone.is_visible() {
                        microterm_clone.hide();
                    } else {
                        let svc_borrow = service_clone.borrow();
                        let voice_status = if svc_borrow.is_some() {
                            "voice: ready"
                        } else {
                            "voice: no model"
                        };
                        let streaming_status = if VOICE_STREAMING_ENABLED
                            .load(std::sync::atomic::Ordering::Relaxed)
                        {
                            "live"
                        } else {
                            "off"
                        };
                        microterm_clone.set_content(MicrotermContent::Status(
                            format!(
                                "rusty-board · {} · streaming: {}",
                                voice_status, streaming_status
                            ),
                        ));
                    }
                    continue;
                }
            }

            // Otherwise we only care about Voice taps for the
            // recording flow.
            let is_voice_action =
                matches!(ev, EngineEvent::KeyAction(KeyAction::Voice));
            if !is_voice_action {
                continue;
            }

            let svc_borrow = service_clone.borrow();
            let svc = match svc_borrow.as_ref() {
                Some(s) => s,
                None => {
                    tracing::info!(
                        "Voice key tapped but service unavailable; ignoring"
                    );
                    microterm_clone.set_content(MicrotermContent::Status(
                        "Voice: no whisper model installed".to_string(),
                    ));
                    // Auto-hide the message after a couple seconds so
                    // the keyboard goes back to its normal full height.
                    let mt_for_hide = microterm_clone.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(2500),
                        move || {
                            mt_for_hide.hide();
                        },
                    );
                    continue;
                }
            };

            // Toggle:
            //   Idle        → start recording, show "Listening...".
            //   Recording   → stop, run inference, deliver result,
            //                 hide microterm.
            //   Transcribing→ ignore (the prior stop is still in flight).
            match svc.state() {
                VoiceServiceState::Idle => {
                    if let Err(e) = svc.start_recording() {
                        tracing::warn!("voice start_recording failed: {}", e);
                        microterm_clone.set_content(MicrotermContent::Status(
                            format!("Voice error: {}", e),
                        ));
                    } else {
                        info!("Voice recording started");
                        // Tell the readiness FSM we're now actively
                        // recording — forces the high-frequency
                        // (33ms) UI poll cadence for a smooth level
                        // meter + partials.
                        crate::voice::readiness::global().set_recording(true);
                        // Clear any leftover partial from a previous
                        // recording session.
                        latest_partial.lock().clear();
                        microterm_clone.set_content(
                            MicrotermContent::VoicePreview {
                                partial: "Listening...".to_string(),
                                level: 0.0,
                            },
                        );

                        // Spawn the streaming-partial worker if
                        // enabled. The worker periodically snapshots
                        // the recording-so-far, runs whisper on it,
                        // and writes the result text into
                        // `latest_partial` for the UI poll to pick up.
                        //
                        // We use a single worker thread (not a pool)
                        // because consecutive streaming inferences
                        // can't usefully run in parallel — by the
                        // time the second one finishes, the first
                        // result is already stale. The worker simply
                        // serializes them: snapshot, transcribe,
                        // write, sleep.
                        if VOICE_STREAMING_ENABLED
                            .load(std::sync::atomic::Ordering::Relaxed)
                        {
                            streaming_stop_flag.store(
                                false,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                            let stop_for_worker = streaming_stop_flag.clone();
                            let partial_for_worker = latest_partial.clone();
                            let transcriber = match svc.transcriber() {
                                Some(t) => t,
                                None => {
                                    tracing::warn!(
                                        "voice: streaming worker skipped — \
                                         transcriber not loaded (will auto-reload \
                                         on next start_recording)"
                                    );
                                    continue;
                                }
                            };
                            // Use the Send-able ring + sample-rate
                            // handle. The full AudioRecorder isn't
                            // Send because it holds a cpal::Stream
                            // (which cpal pins to its creation
                            // thread). The ring buffer behind the
                            // Arc<Mutex<>> is fine.
                            let (ring, sample_rate) = svc.audio_snapshot_handle();
                            let interval_ms = streaming_interval_ms;
                            std::thread::Builder::new()
                                .name("voice-stream".to_string())
                                .spawn(move || {
                                    streaming_partial_worker(
                                        stop_for_worker,
                                        partial_for_worker,
                                        transcriber,
                                        ring,
                                        sample_rate,
                                        interval_ms,
                                    );
                                })
                                .map(|_| ())
                                .unwrap_or_else(|e| {
                                    tracing::warn!(
                                        "streaming worker spawn failed: {}",
                                        e
                                    );
                                });
                        }
                    }
                }
                VoiceServiceState::Recording => {
                    // No longer recording — return the FSM to Warm
                    // (next utterance starts fast, but the 33Hz poll
                    // relaxes to 100ms while we transcribe + idle).
                    crate::voice::readiness::global().set_recording(false);
                    // Tell the streaming worker (if any) to stop.
                    // The worker checks this flag at the top of
                    // every loop iteration and exits cleanly.
                    streaming_stop_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);

                    // Async stop: drain audio synchronously (fast),
                    // then spawn a worker thread to do inference.
                    // The GTK main loop stays responsive — UI paint
                    // continues, level meter freezes at 0 (recording
                    // stopped), microterm shows "Transcribing..."
                    // until the result lands.
                    microterm_clone.set_content(MicrotermContent::VoicePreview {
                        partial: "Transcribing...".to_string(),
                        level: 0.0,
                    });
                    match svc.stop_recording_async() {
                        Ok(Some(rx)) => {
                            // Store the receiver for the pump loop
                            // to drain on subsequent ticks.
                            *pending_for_pump.borrow_mut() = Some(rx);
                        }
                        Ok(None) => {
                            // Recording was too short. Show feedback
                            // and auto-hide.
                            microterm_clone.set_content(
                                MicrotermContent::Status(
                                    "(too short)".to_string(),
                                ),
                            );
                            let mt_for_hide = microterm_clone.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(1200),
                                move || {
                                    mt_for_hide.hide();
                                },
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "voice stop_recording_async failed: {}",
                                e
                            );
                            microterm_clone.set_content(
                                MicrotermContent::Status(format!(
                                    "Voice error: {}",
                                    e
                                )),
                            );
                        }
                    }
                }
                VoiceServiceState::Transcribing => {
                    tracing::debug!(
                        "Voice key tapped during transcription; ignoring"
                    );
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

/// The streaming-partial worker. Runs on its own thread for the
/// duration of a single recording session.
///
/// Loop:
///   - Sleep `interval_ms` (so we don't spam inference)
///   - Check `stop_flag`; exit if set
///   - Snapshot the recorder's current samples (non-destructive)
///   - If buffer is too short, skip (just a noise floor)
///   - Resample to 16kHz, run whisper inference
///   - Write the resulting text into `latest_partial` for the UI
///     to display on its next tick
///
/// Notes:
///   - Whisper inference takes ~200-500ms on phone, so each loop
///     iteration's total wall-clock is `interval_ms + inference_ms`.
///     With interval_ms=600 and inference_ms=300, the user sees a
///     fresh partial every ~900ms.
///   - We don't bound the snapshot length; whisper handles up to
///     30 seconds of audio per call. If the user records longer than
///     30s, whisper truncates internally (with a warning logged to
///     stderr).
///   - The final inference (run by stop_recording_async) is a
///     completely separate path; the streaming worker can be mid-
///     inference when the user releases, and its result will simply
///     be discarded (we ignore writes to latest_partial once the
///     microterm switches to the Transcribing/Status state).
#[cfg(feature = "voice")]
fn streaming_partial_worker(
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latest_partial: std::sync::Arc<parking_lot::Mutex<String>>,
    transcriber: std::sync::Arc<crate::voice::transcriber::Transcriber>,
    ring: crate::voice::recorder::AudioRing,
    sample_rate: u32,
    interval_ms: u32,
) {
    // Minimum audio length before bothering to run inference, in ms.
    // Shorter than this is usually just the noise floor — running
    // whisper on it produces hallucinations (transcription of pure
    // silence often outputs "Thank you." or other artifact strings).
    const MIN_AUDIO_MS: u32 = 400;

    info!("streaming-partial worker started (interval={}ms)", interval_ms);

    loop {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms as u64));
        if stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        // Non-destructive snapshot of the ring. Lock is held briefly:
        // just enough to copy the contents into a Vec.
        let samples = crate::voice::recorder::snapshot_ring(&ring);

        if samples.is_empty() {
            continue;
        }
        let duration_ms = (samples.len() as f64 / sample_rate as f64 * 1000.0) as u32;
        if duration_ms < MIN_AUDIO_MS {
            continue;
        }

        // Resample + transcribe. This is the expensive call.
        let resampled = crate::voice::recorder::resample_to_16k(&samples, sample_rate);
        let text = match transcriber.transcribe(&resampled) {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!("streaming inference error (ignored): {}", e);
                continue;
            }
        };

        // Strip trailing artifacts ([BLANK_AUDIO] etc) BEFORE
        // applying the hallucination filter — that way "hello
        // world [BLANK_AUDIO]" becomes "hello world" and isn't
        // mistakenly filtered.
        let text = crate::voice::hallucination::strip_trailing_artifacts(&text);
        if text.is_empty() {
            continue;
        }

        // Filter whisper's known silence-hallucinations. The
        // streaming path runs inference on whatever's in the
        // buffer — including periods where the user hasn't spoken
        // yet, or is between phrases. Without this filter the
        // microterm would briefly flash "Thank you." or
        // "Thanks for watching." on dead air. The final
        // (post-stop) inference path doesn't filter — a user who
        // genuinely says "thanks" gets it committed.
        if crate::voice::hallucination::is_hallucination(&text) {
            tracing::debug!(
                "streaming inference: filtered hallucination {:?}",
                text
            );
            continue;
        }

        // Check the flag AGAIN before writing — if the user stopped
        // recording during our inference, the final inference path
        // is now running and we should NOT stomp on its result by
        // updating latest_partial. The level-polling timer reads
        // latest_partial but only when state==Recording, so writes
        // after stop are technically safe; we still skip them as
        // defensive coding.
        if stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if !text.is_empty() {
            *latest_partial.lock() = text;
        }
    }

    info!("streaming-partial worker exited");
}

/// Stub used when the `voice` feature is compiled out. The Voice key
/// is still present in the layout but taps go to a no-op handler.
#[cfg(not(feature = "voice"))]
fn spawn_voice_bridge(
    _engine: &KeyboardEngine,
    _cfg: &Config,
    _microterm: std::rc::Rc<crate::ui::microterm::MicrotermView>,
) {
    tracing::info!("Voice feature not compiled in — Voice key will be inert");
}

/// Append a voice transcript to the JSONL log file. Best-effort —
/// failures are warn-logged but otherwise silent. Useful for
/// debugging misrecognitions and for users who accidentally undo a
/// transcript and want to recover it.
///
/// Format: one JSON object per line:
///   {"ts": "...", "kind": "commit"|"llm_prompt", "text": "..."}
///
/// File location: `~/.local/share/rusty-board/voice-log.jsonl`,
/// auto-created on first write. We don't rotate; users who care
/// can rotate or clear manually.
#[cfg(feature = "voice")]
fn log_voice_transcript(text: &str, kind: &str) {
    use std::io::Write;

    if !voice_log_enabled() {
        return;
    }

    let path = match directories::BaseDirs::new() {
        Some(d) => d.data_local_dir().join("rusty-board").join("voice-log.jsonl"),
        None => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = chrono_like_now();
    // Manually encode JSON to avoid pulling serde_json into the
    // hot path (the keyboard already minimizes deps for phone
    // memory pressure). The escape handles quotes + backslashes
    // + newlines, which is sufficient for log lines.
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    let line = format!(
        "{{\"ts\":\"{}\",\"kind\":\"{}\",\"text\":\"{}\"}}\n",
        ts, kind, escaped
    );
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = result {
        tracing::warn!("voice log write failed: {}", e);
    }
}

/// Read the last `n` entries from the voice transcript log.
/// Returns text values only (drops timestamp/kind). Used by
/// the `/history` LLM-prompt slash command.
#[cfg(feature = "voice")]
fn voice_history_tail(n: usize) -> Vec<String> {
    let path = match directories::BaseDirs::new() {
        Some(d) => d.data_local_dir().join("rusty-board").join("voice-log.jsonl"),
        None => return Vec::new(),
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    // Parse each line by hand. We only need the "text" field; do
    // a cheap substring search to avoid pulling serde_json into
    // this code path.
    let mut out: Vec<String> = contents
        .lines()
        .rev()
        .take(n)
        .filter_map(|line| {
            let key = "\"text\":\"";
            let start = line.find(key)? + key.len();
            // Find the matching closing quote, skipping escapes.
            let bytes = line.as_bytes();
            let mut end = start;
            while end < bytes.len() {
                if bytes[end] == b'\\' {
                    end += 2;
                    continue;
                }
                if bytes[end] == b'"' {
                    break;
                }
                end += 1;
            }
            if end >= bytes.len() {
                return None;
            }
            let raw = &line[start..end];
            // Unescape the simple set we wrote (\n, \", \\, \t, \r).
            let unescaped = raw
                .replace("\\\"", "\"")
                .replace("\\n", "\n")
                .replace("\\r", "\r")
                .replace("\\t", "\t")
                .replace("\\\\", "\\");
            Some(unescaped)
        })
        .collect();
    out.reverse(); // chronological order again
    out
}

/// Minimal ISO 8601 UTC timestamp without pulling in chrono.
/// Uses std::time::SystemTime; formatted to second precision.
fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Convert to broken-down UTC. We avoid chrono by doing the
    // arithmetic by hand; precision is "seconds since Unix epoch"
    // which is unambiguous for log purposes.
    let secs_per_day: i64 = 86_400;
    let mut days_since_epoch = (now as i64) / secs_per_day;
    let sec_of_day = (now as i64) % secs_per_day;
    let hour = sec_of_day / 3600;
    let min = (sec_of_day % 3600) / 60;
    let sec = sec_of_day % 60;

    // Days → (year, month, day). Algorithm from Howard Hinnant's
    // date library — chronologically robust through year 2200.
    days_since_epoch += 719_468; // shift to civil epoch (0000-03-01)
    let era = if days_since_epoch >= 0 {
        days_since_epoch
    } else {
        days_since_epoch - 146_096
    } / 146_097;
    let doe = (days_since_epoch - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, min, sec
    )
}

/// Spawn the always-on microterm bridge. Unlike the voice and LLM
/// bridges, this is NOT feature-gated — it serves status toasts and
/// steno word-expansion suggestions, both of which exist in
/// non-voice/non-LLM builds (the `full-keyboard` profile has
/// `input-modes` for steno but no voice/llm).
///
/// Responsibilities:
///   - Register the steno suggestion-chip tap callback: tapping a
///     candidate calls the steno engine's `accept_word` (which
///     clears the phoneme buffer) and commits the chosen word via
///     the engine, then hides the ribbon.
///   - Drain `MICROTERM_STATUS_SLOT` (ephemeral toasts) and
///     `MICROTERM_STENO_SLOT` (suggestion ribbons) on a 150ms timer.
fn spawn_microterm_bridge(
    engine: &KeyboardEngine,
    microterm: std::rc::Rc<crate::ui::microterm::MicrotermView>,
) {
    use std::cell::RefCell;
    use std::rc::Rc;
    // Steno suggestion tap → accept the word. The engine commits it
    // as a SwipeWord (same path as a steno phoneme commit), and
    // accept_word clears the steno buffer so the next stroke starts
    // a fresh word.
    {
        let engine_for_tap = engine.clone();
        let mt_for_tap = microterm.clone();
        microterm.set_steno_tap_callback(Box::new(move |word| {
            let accepted = crate::steno::global().accept_word(&word);
            // Delete the raw phonemes already committed to the editor
            // as the user stroked (e.g. "kat"), THEN insert the word.
            for _ in 0..accepted.backspaces {
                engine_for_tap.handle_input(
                    crate::keyboard::engine::EngineInput::NamedKey("BackSpace".to_string()),
                );
            }
            engine_for_tap.handle_input(
                crate::keyboard::engine::EngineInput::StenoCommit(accepted.text),
            );
            tracing::info!(
                "steno: accepted candidate '{}' ({} phoneme chars replaced)",
                word, accepted.backspaces
            );
            mt_for_tap.hide();
        }));
    }

    // Drain timer: status toasts + steno suggestions.
    //
    // Self-adaptive cadence: snap to FAST_MS when something was
    // drained, back off toward IDLE_MS when nothing's happening.
    // Status toasts and steno suggestions are sparse (rare user
    // events: toggle a setting → toast; stroke a phoneme → ribbon),
    // and neither drives animation, so a few hundred ms of pickup
    // latency from idle is invisible. Pre-adaptive this was a fixed
    // 150ms (~7Hz) always-on; that pump's wakeups added up with the
    // other always-on timers on phone.
    {
        const FAST_MS: u64 = 150;
        const IDLE_MS: u64 = 800;
        let microterm_for_drain = microterm.clone();
        let interval_slot: Rc<std::cell::Cell<u64>> =
            Rc::new(std::cell::Cell::new(IDLE_MS));
        type Rearm = Rc<dyn Fn()>;
        let rearm_slot: Rc<RefCell<Option<Rearm>>> = Rc::new(RefCell::new(None));
        let rearm_inner = rearm_slot.clone();

        let tick: Rearm = Rc::new(move || {
            // Steno suggestions take priority over status when both
            // are pending in the same tick (the suggestion ribbon is
            // the interactive thing the user is mid-flow with).
            let did_work = if let Some(payload) = drain_steno_suggestions() {
                let candidates = payload
                    .candidates
                    .into_iter()
                    .map(|(word, fuzzy)| {
                        crate::ui::microterm::StenoCandidate { word, fuzzy }
                    })
                    .collect();
                microterm_for_drain.set_content(
                    crate::ui::microterm::MicrotermContent::StenoSuggestions {
                        buffer: payload.buffer,
                        candidates,
                    },
                );
                // Steno ribbon lingers longer than a toast — the user
                // is deciding whether to tap. Auto-hide after 5s of
                // no further strokes (a new stroke reposts and resets
                // this via the latest-wins slot).
                let mt_for_hide = microterm_for_drain.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(5000),
                    move || {
                        mt_for_hide.hide();
                    },
                );
                true
            } else if let Some(text) = drain_microterm_status() {
                microterm_for_drain.set_content(
                    crate::ui::microterm::MicrotermContent::Status(text),
                );
                let mt_for_hide = microterm_for_drain.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(3500),
                    move || {
                        mt_for_hide.hide();
                    },
                );
                true
            } else {
                false
            };

            let next = if did_work {
                FAST_MS
            } else {
                (interval_slot.get().saturating_mul(2)).min(IDLE_MS)
            };
            interval_slot.set(next);
            if let Some(rearm) = rearm_inner.borrow().as_ref() {
                let rearm = rearm.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(next),
                    move || rearm(),
                );
            }
        });
        *rearm_slot.borrow_mut() = Some(tick.clone());
        tick();
    }
}

/// Spawn the LLM bridge. Subscribes to:
///   - `EngineEvent::KeyAction(KeyAction::Llm)` → open the prompt
///     entry on the microterm (status display with hint text).
///   - `EngineEvent::LlmRequest(task)` → submit the task to the
///     LLM worker; show "Thinking..." on the microterm; on result
///     commit via the engine and hide.
///
/// Like the voice bridge, this runs entirely on the GTK main thread
/// EXCEPT for the actual HTTP request, which the `LlmService` runs
/// on a worker thread. The bridge polls the worker's mpsc receiver
/// via a glib timeout.
#[cfg(feature = "llm")]
fn spawn_llm_bridge(
    engine: &KeyboardEngine,
    cfg: &Config,
    microterm: std::rc::Rc<crate::ui::microterm::MicrotermView>,
) {
    use crate::keyboard::KeyAction;
    use crate::llm::{LlmRequest, LlmResponse, LlmService};
    use crate::ui::microterm::MicrotermContent;
    use std::cell::RefCell;
    use std::rc::Rc;

    let service: Rc<RefCell<Option<LlmService>>> = Rc::new(RefCell::new(None));

    // Initialize the live-config slot so subsequent requests use
    // it without needing to read cfg through a captured reference
    // (which has lifetime constraints).
    set_live_llm_config(cfg.llm.clone());

    match LlmService::new(cfg.llm.clone()) {
        Ok(svc) => {
            *service.borrow_mut() = Some(svc);
        }
        Err(e) => {
            tracing::info!(
                "LLM bridge inert: {} (set [llm].enabled=true and configure endpoint to enable)",
                e
            );
        }
    }

    let rx = engine.subscribe();
    let engine_clone = engine.clone();
    let service_clone = service.clone();
    let microterm_clone = microterm.clone();

    // Pending LLM response receiver. At most one in-flight at a
    // time; subsequent requests while one is in flight are ignored
    // with a "busy" toast in the microterm.
    let pending: Rc<RefCell<Option<std::sync::mpsc::Receiver<anyhow::Result<LlmResponse>>>>> =
        Rc::new(RefCell::new(None));

    // Streaming partials slot. The LLM stream worker writes
    // accumulated text here on each delta; the pump timer reads
    // it and updates the microterm. Cleared when the request
    // completes or a new one starts.
    let llm_partials: std::sync::Arc<parking_lot::Mutex<String>> =
        std::sync::Arc::new(parking_lot::Mutex::new(String::new()));

    let pending_for_pump = pending.clone();
    let microterm_for_pump = microterm.clone();
    let engine_for_pump = engine.clone();
    let partials_for_pump = llm_partials.clone();
    let pending_for_check = pending.clone();

    // LLM pump: 200ms cadence. The pump only does meaningful work
    // when a request is in flight (poll pending receiver, update
    // streaming partials). 200ms latency on result delivery is
    // imperceptible compared to the actual LLM round-trip time
    // (typically 1-10 seconds). The reduced wake-up rate matters
    // because this pump used to fire every 50ms = 20Hz, adding
    // jitter to general-use frames.
    // Track cancel generation independently from the voice bridge.
    // Both bridges see every cancel — the generation counter is a
    // monotonic broadcast.
    let llm_last_cancel_gen: Rc<std::cell::Cell<u32>> =
        Rc::new(std::cell::Cell::new(cancel_generation()));

    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        // ---- Honour cancel requests ----
        // Both voice and LLM bridges see every cancel because they
        // track their own snapshot of the generation counter (no
        // race like the old drain-bool design).
        let now_gen = cancel_generation();
        let cancelled = now_gen != llm_last_cancel_gen.get();
        if cancelled {
            llm_last_cancel_gen.set(now_gen);
        }
        if cancelled {
            let mut p = pending_for_check.borrow_mut();
            if p.is_some() {
                tracing::info!("LLM request cancelled by user");
                *p = None;
                // Restore the popped text — the user cancelled
                // the LLM but the rewrite/synonym path had
                // already deleted the original from their editor.
                if let Some(original) = take_llm_restore_text() {
                    info!("LLM cancel: restoring popped {:?}", original);
                    engine_for_pump.handle_input(
                        crate::keyboard::engine::EngineInput::LlmCommit(
                            original,
                        ),
                    );
                }
            }
            partials_for_pump.lock().clear();
        }

        // ---- Show streaming LLM partials ----
        // If a request is in flight, read the latest accumulated
        // partial text and show it. This runs every 50ms; the
        // worker writes more frequently than that but only the
        // latest write is visible.
        if pending_for_check.borrow().is_some() {
            let partial = partials_for_pump.lock().clone();
            if !partial.is_empty() {
                microterm_for_pump.set_content(
                    crate::ui::microterm::MicrotermContent::Status(
                        format!("✨ {}", partial),
                    ),
                );
            }
        }

        // ---- Poll pending LLM response ----
        let result_to_handle = {
            let mut p = pending_for_pump.borrow_mut();
            if let Some(rx) = p.as_ref() {
                match rx.try_recv() {
                    Ok(r) => {
                        *p = None;
                        Some(r)
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        *p = None;
                        Some(Err(anyhow::anyhow!("LLM worker disconnected")))
                    }
                }
            } else {
                None
            }
        };
        if let Some(result) = result_to_handle {
            match result {
                Ok(resp) if !resp.text.is_empty() => {
                    info!(
                        "LLM {} done: {:?}",
                        resp.task_label, resp.text
                    );
                    // Only pop-tasks (Rewrite/Synonym/Dictate) have
                    // restore data that's now obsolete on success.
                    // Continue/Prompt didn't pop anything; if the
                    // slot has data, it belongs to an earlier pop-
                    // task whose response hasn't arrived yet, and
                    // we must NOT clear it — otherwise if that
                    // pop-task errors later, restore would no-op
                    // and the user's original text is lost.
                    let task_consumes_restore = matches!(
                        resp.task_label,
                        "Rewrite" | "Synonym" | "Dictate"
                    );
                    if task_consumes_restore {
                        let _ = take_llm_restore_text();
                    }
                    if resp.commit {
                        engine_for_pump.handle_input(
                            crate::keyboard::engine::EngineInput::LlmCommit(
                                resp.text.clone(),
                            ),
                        );
                        microterm_for_pump.hide();
                    } else {
                        microterm_for_pump.set_content(
                            MicrotermContent::Status(format!(
                                "{}: {}",
                                resp.task_label, resp.text
                            )),
                        );
                        let mt_for_hide = microterm_for_pump.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(4000),
                            move || {
                                mt_for_hide.hide();
                            },
                        );
                    }
                }
                Ok(_) => {
                    // Empty response — restore the popped text if
                    // any. The user's editor lost the original word
                    // when the rewrite/synonym popped it; they
                    // didn't ask to delete it, they asked for a
                    // replacement that the LLM didn't provide.
                    if let Some(original) = take_llm_restore_text() {
                        info!("LLM empty: restoring popped {:?}", original);
                        engine_for_pump.handle_input(
                            crate::keyboard::engine::EngineInput::LlmCommit(
                                original,
                            ),
                        );
                    }
                    microterm_for_pump.set_content(MicrotermContent::Status(
                        "LLM: empty response".to_string(),
                    ));
                }
                Err(e) => {
                    // Same restore logic on error.
                    if let Some(original) = take_llm_restore_text() {
                        info!("LLM error: restoring popped {:?}", original);
                        engine_for_pump.handle_input(
                            crate::keyboard::engine::EngineInput::LlmCommit(
                                original,
                            ),
                        );
                    }
                    tracing::warn!("LLM error: {}", e);
                    microterm_for_pump.set_content(MicrotermContent::Status(
                        format!("LLM error: {}", short_err(&e)),
                    ));
                }
            }
        }

        // ---- Drain engine events ----
        while let Ok(ev) = rx.try_recv() {
            match ev {
                EngineEvent::KeyAction(KeyAction::Llm) => {
                    // The Llm key was tapped. For now we open the
                    // microterm with a hint message; the actual
                    // prompt-capture UI (text entry or voice
                    // capture) lives in the microterm view.
                    if service_clone.borrow().is_none() {
                        microterm_clone.set_content(MicrotermContent::Status(
                            "LLM disabled — enable in settings".to_string(),
                        ));
                        let mt_for_hide = microterm_clone.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(2500),
                            move || {
                                mt_for_hide.hide();
                            },
                        );
                        continue;
                    }
                    // Show the prompt entry. The microterm itself
                    // handles input capture and calls back via the
                    // engine when the user submits.
                    //
                    // The prompt supports slash commands as a
                    // power-user shortcut:
                    //   /rewrite <instruction>  — rewrite last
                    //                              ~10 commits
                    //   /synonym                — synonym for last
                    //                              word
                    //   /continue               — autocomplete from
                    //                              recent context
                    // Anything else is a Freeform prompt.
                    microterm_clone.show_llm_prompt({
                        let engine_for_submit = engine_clone.clone();
                        Box::new(move |prompt| {
                            let trimmed = prompt.trim();
                            if trimmed.is_empty() {
                                return;
                            }
                            let lower = trimmed.to_lowercase();
                            // Non-LLM slash commands first.
                            if lower.starts_with("/help") {
                                post_microterm_status(
                                    "Commands: /rewrite <how>, /synonym, \
                                     /continue, /history, /draw, /draw_clear, \
                                     /train <char>, /forget, /help",
                                );
                                return;
                            }
                            // Toggle character drawing mode.
                            if lower.starts_with("/draw_clear")
                                || lower.starts_with("/clear")
                            {
                                crate::keyboard::character_draw::clear_strokes();
                                post_microterm_status("Drawing cleared.");
                                return;
                            }
                            if lower.starts_with("/forget") {
                                crate::keyboard::character_draw::forget_user_templates();
                                post_microterm_status(
                                    "Forgot all user-trained templates.",
                                );
                                return;
                            }
                            // Train a new character from current strokes.
                            // Syntax: /train α  (anything after the
                            // space becomes the display string).
                            if lower.starts_with("/train") {
                                let display = trimmed[6..].trim();
                                if display.is_empty() {
                                    post_microterm_status(
                                        "Usage: /train <char> — first draw the strokes, then /train",
                                    );
                                    return;
                                }
                                let buf = crate::keyboard::character_draw::current_strokes();
                                if buf.is_empty() {
                                    post_microterm_status(
                                        "No strokes to train on — draw first, then /train",
                                    );
                                    return;
                                }
                                match crate::keyboard::character_draw::train_user_template(
                                    display, &buf,
                                ) {
                                    Ok(()) => {
                                        crate::keyboard::character_draw::clear_strokes();
                                        post_microterm_status(format!(
                                            "✓ Trained {:?} from your strokes",
                                            display
                                        ));
                                    }
                                    Err(e) => {
                                        post_microterm_status(format!("Train failed: {}", e));
                                    }
                                }
                                return;
                            }
                            if lower.starts_with("/draw") {
                                let now_on = !crate::keyboard::character_draw::draw_mode_enabled();
                                crate::keyboard::character_draw::set_draw_mode(now_on);
                                if now_on {
                                    post_microterm_status(
                                        "Drawing mode ON — draw on spacebar, long-press to commit",
                                    );
                                } else {
                                    post_microterm_status("Drawing mode OFF");
                                }
                                return;
                            }
                            #[cfg(feature = "voice")]
                            if lower.starts_with("/history") {
                                let entries = voice_history_tail(5);
                                if entries.is_empty() {
                                    post_microterm_status(
                                        "No voice history yet.",
                                    );
                                } else {
                                    let joined = entries
                                        .iter()
                                        .enumerate()
                                        .map(|(i, t)| {
                                            format!(
                                                "{}: {}",
                                                i + 1,
                                                t.chars().take(40).collect::<String>()
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(" | ");
                                    post_microterm_status(joined);
                                }
                                return;
                            }
                            // Otherwise route to LLM with task parsing.
                            let task = parse_prompt_to_task(
                                trimmed,
                                &engine_for_submit,
                            );
                            record_llm_prompt(trimmed);
                            engine_for_submit.handle_input(
                                crate::keyboard::engine::EngineInput::LlmRequest(task),
                            );
                        })
                    });
                }
                EngineEvent::LlmRequest(task) => {
                    // Rebuild the service if the live config has
                    // drifted. We compare endpoint+model+key as a
                    // shorthand for "user edited LLM settings";
                    // other fields (timeout, max_tokens, etc.)
                    // would not require rebuild but we just
                    // always rebuild for simplicity. The cost is
                    // a URL parse — trivial.
                    if let Some(live_cfg) = current_llm_config() {
                        if live_cfg.enabled {
                            match LlmService::new(live_cfg) {
                                Ok(svc) => *service_clone.borrow_mut() = Some(svc),
                                Err(e) => {
                                    tracing::warn!("LLM rebuild failed: {}", e);
                                }
                            }
                        } else {
                            *service_clone.borrow_mut() = None;
                        }
                    }
                    let svc_borrow = service_clone.borrow();
                    let svc = match svc_borrow.as_ref() {
                        Some(s) => s,
                        None => {
                            tracing::info!(
                                "LLM request received but service unavailable"
                            );
                            continue;
                        }
                    };
                    if pending_for_pump.borrow().is_some() {
                        microterm_clone.set_content(MicrotermContent::Status(
                            "LLM busy — previous request in flight".to_string(),
                        ));
                        continue;
                    }
                    let label = task.label();
                    let model_hint = current_llm_config()
                        .map(|c| c.model)
                        .unwrap_or_default();
                    let thinking_msg = if model_hint.is_empty() {
                        format!("{}: thinking...", label)
                    } else {
                        format!("{} via {}: thinking...", label, model_hint)
                    };
                    microterm_clone.set_content(MicrotermContent::Status(
                        thinking_msg,
                    ));
                    let req = LlmRequest {
                        task,
                        commit_on_success: true,
                    };
                    // Clear the partials slot so the previous
                    // request's tail doesn't bleed into this one's
                    // first frames.
                    llm_partials.lock().clear();
                    match svc.submit_stream(req, llm_partials.clone()) {
                        Ok(rx_one) => {
                            *pending_for_pump.borrow_mut() = Some(rx_one);
                        }
                        Err(e) => {
                            tracing::warn!("LLM submit failed: {}", e);
                            microterm_clone.set_content(
                                MicrotermContent::Status(format!(
                                    "LLM error: {}",
                                    short_err(&e)
                                )),
                            );
                        }
                    }
                }
                EngineEvent::KeyLongPress(ref k)
                    if matches!(k.action, KeyAction::Llm) =>
                {
                    // Long-press on the LLM key: dictate the prompt.
                    // We set VOICE_FEEDS_LLM so the next voice final
                    // is routed to LlmRequest instead of committed;
                    // then synthesize a Voice key tap so the voice
                    // bridge starts recording as usual.
                    if service_clone.borrow().is_none() {
                        microterm_clone.set_content(MicrotermContent::Status(
                            "LLM disabled".to_string(),
                        ));
                        continue;
                    }
                    set_voice_feeds_llm(true);
                    microterm_clone.set_content(MicrotermContent::Status(
                        "Speak your prompt...".to_string(),
                    ));
                    let voice_key = crate::keyboard::Key::special(
                        "🎤",
                        KeyAction::Voice,
                    );
                    engine_clone.handle_input(
                        crate::keyboard::engine::EngineInput::KeyTap(voice_key),
                    );
                }
                _ => {}
            }
        }
        glib::ControlFlow::Continue
    });
}

/// Stub used when the `llm` feature is compiled out.
#[cfg(not(feature = "llm"))]
fn spawn_llm_bridge(
    _engine: &KeyboardEngine,
    _cfg: &Config,
    _microterm: std::rc::Rc<crate::ui::microterm::MicrotermView>,
) {
    tracing::info!("LLM feature not compiled in");
}

/// Parse a microterm prompt into an LlmTask. Slash commands
/// dispatch to specific task variants; everything else is
/// Freeform.
///
/// Slash commands:
///   /rewrite [instruction]  — pop last 10 commits from undo stack,
///                              submit as Rewrite with the
///                              instruction (default: "make it
///                              clearer")
///   /synonym                — synonym for the most recent commit
///   /continue               — autocomplete the recent context
///
/// Anything else (no leading slash, or unrecognized slash) is
/// passed as Freeform.
#[cfg(feature = "llm")]
fn parse_prompt_to_task(
    prompt: &str,
    engine: &crate::keyboard::KeyboardEngine,
) -> crate::llm::LlmTask {
    use crate::llm::LlmTask;

    let lower = prompt.to_lowercase();
    if let Some(rest) = lower.strip_prefix("/rewrite") {
        let instruction = rest.trim();
        let instruction = if instruction.is_empty() {
            "make it clearer".to_string()
        } else {
            instruction.to_string()
        };
        // Pop the target span so the rewrite replaces it. The
        // returned text is also what we pass as the rewrite target
        // (canonical: the actual chars we removed from the focused
        // app's buffer).
        let (_evs, target) = engine.pop_last_n_for_rewrite(10);
        // Stash the original so the LLM bridge can restore it
        // on failure (the user's editor has already had the
        // backspaces applied). If an earlier in-flight request had
        // restore data pending, we've just overwritten it — restore
        // that text RIGHT NOW so its original isn't lost. The
        // earlier worker's rx has already been dropped by the new
        // submit_stream call elsewhere; its response will be
        // discarded silently.
        if let Some(stale) = set_llm_restore_text(&target) {
            tracing::info!(
                "LLM: overlapping request — restoring {} chars from earlier request",
                stale.chars().count()
            );
            engine.handle_input(
                crate::keyboard::engine::EngineInput::LlmCommit(stale),
            );
        }
        return LlmTask::Rewrite { target, instruction };
    }
    if lower.starts_with("/synonym") {
        let word = engine.peek_last_commit().unwrap_or_default();
        let (_evs, popped) = engine.pop_last_n_for_rewrite(1);
        if let Some(stale) = set_llm_restore_text(&popped) {
            tracing::info!(
                "LLM: overlapping request — restoring {} chars from earlier request",
                stale.chars().count()
            );
            engine.handle_input(
                crate::keyboard::engine::EngineInput::LlmCommit(stale),
            );
        }
        let context = {
            let st = engine.state_snapshot();
            if !st.surrounding_text.is_empty() {
                Some(st.surrounding_text)
            } else {
                engine.peek_last_n_commits(5)
            }
        };
        return LlmTask::Synonym {
            word: word.trim().to_string(),
            context,
        };
    }
    if lower.starts_with("/continue") {
        // Prefer the editor's surrounding text (more accurate
        // context) over our own commit history (which only knows
        // what WE typed). Surrounding text comes from the input-
        // method server's SurroundingText event.
        let before = {
            let st = engine.state_snapshot();
            if !st.surrounding_text.is_empty() {
                st.surrounding_text
            } else {
                engine.peek_last_n_commits(20).unwrap_or_default()
            }
        };
        return LlmTask::Continue {
            before,
            max_words: 20,
        };
    }
    LlmTask::Freeform(prompt.to_string())
}

/// Shorten an error chain to a single line for microterm display.
fn short_err(e: &anyhow::Error) -> String {
    let s = e.to_string();
    if s.len() > 60 {
        format!("{}...", &s[..60])
    } else {
        s
    }
}

fn build_suggestions_bar() -> gtk4::Box {
    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .height_request(40)
        .css_classes(["suggestions-bar"])
        .build();

    // Three placeholder suggestion buttons. The engine will populate these
    // from word predictions (stubbed for now).
    for label in ["the", "and", "for"] {
        let btn = gtk4::Button::builder()
            .label(label)
            .css_classes(["suggestion-pill"])
            .hexpand(true)
            .build();
        bar.append(&btn);
    }

    bar
}

fn apply_theme(theme: &str) {
    let style_manager = adw::StyleManager::default();
    match theme {
        "light" => style_manager.set_color_scheme(adw::ColorScheme::ForceLight),
        "dark" => style_manager.set_color_scheme(adw::ColorScheme::ForceDark),
        _ => style_manager.set_color_scheme(adw::ColorScheme::Default),
    }
}
