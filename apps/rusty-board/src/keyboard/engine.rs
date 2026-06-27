//! KeyboardEngine: the central coordinator.
//!
//! Owns the `KeyboardState`, dispatches events from the UI to the state
//! machine, and emits high-level events (`text-committed`, `key-emitted`,
//! `state-changed`) for the UI and IPC layers to consume.

use crate::keyboard::{Key, KeyAction, KeyboardState, KeypadState};
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{debug, info};

/// Events emitted by the engine for downstream consumers (UI, virtual
/// keyboard backend, IPC).
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A character or string was committed.
    TextCommitted(String),
    /// A non-text key (backspace, return, ...) was triggered.
    KeyAction(KeyAction),
    /// The keyboard's visible state changed (page swap, shift toggle).
    StateChanged(KeypadState),
    /// Voice transcription is available (partial or final).
    VoiceTranscript { text: String, partial: bool },
    /// A modifier key was pressed.
    ModifierPressed(KeyAction),
    /// A key received a long-press gesture. UI subscribers can use
    /// this to show popovers, status displays, or other long-press-
    /// triggered behavior.
    KeyLongPress(Key),
    /// An LLM task was requested. The LLM bridge subscribes to this
    /// to dispatch work to its worker thread.
    LlmRequest(crate::llm::LlmTask),
}

/// Inputs the engine accepts. The UI layer pushes these in response to user
/// gestures.
#[derive(Debug, Clone)]
pub enum EngineInput {
    /// User tapped a single key.
    KeyTap(Key),
    /// User long-pressed a key (we treat this as showing the popover, but
    /// the actual selection comes back as KeyTap on the popover key).
    KeyLongPress(Key),
    /// User completed a swipe gesture, producing a sequence of keys.
    SwipeWord(String),
    /// Verbatim commit that does NOT participate in swipe-typing's
    /// auto-space behavior — commits text exactly as given, resets
    /// the swipe-sequence flag so the NEXT swipe also starts fresh.
    ///
    /// Used by any commit path that emits short text the user
    /// wouldn't think of as "a swipe-typed word":
    ///   - Steno phoneme/brief commits (the original use case;
    ///     hence the name). Phonemes chain directly: k + a → "ka",
    ///     not "k a".
    ///   - Cardinal swipes — each emits a single character; consec-
    ///     utive cardinal swipes would otherwise become "u e" not
    ///     "ue".
    ///   - Character-draw canvas commits — recognized chars are
    ///     single chars, same logic.
    ///
    /// Kept separate from SwipeWord so the auto-space-between-
    /// swipes feature can't corrupt these commits.
    StenoCommit(String),
    /// User performed a gesture that should send a single named keysym
    /// (e.g. "Left", "Right", "Up", "Down" from a spacebar swipe). The
    /// name is forwarded to the virtual-keyboard backend as-is.
    NamedKey(String),
    /// External update (e.g. focus changed in the host app).
    SurroundingTextChanged { text: String, cursor: usize },
    /// Voice partial transcript arrived.
    VoicePartial(String),
    /// Voice final transcript arrived.
    VoiceFinal(String),
    /// Undo the most recent committed text. The engine pops its
    /// undo stack and emits enough BackSpace KeyActions to delete
    /// the prior commit. No-op if the stack is empty.
    Undo,
    /// Request an LLM action (synonym, rewrite, dictate, etc).
    /// The engine just broadcasts this as an event so the LLM
    /// bridge can pick it up; the engine itself doesn't talk to
    /// the LLM.
    LlmRequest(crate::llm::LlmTask),
    /// LLM completion finished — commit the result text. Mirrors
    /// VoiceFinal but distinguishes the source so subscribers can
    /// label it differently in logs / undo history.
    LlmCommit(String),
}

#[derive(Clone)]
pub struct KeyboardEngine {
    state: Arc<RwLock<KeyboardState>>,
    /// Fan-out subscribers. Each subscriber gets every event via its
    /// own crossbeam channel. Used for the voice / llm / microterm
    /// bridges and the keyboard_view's StateChanged listener — paths
    /// where deferred processing on a timer is fine.
    ///
    /// The engine→vkbd path is NOT a subscriber. Routing text/keysym
    /// commits through a polling subscriber added up to ~60ms of
    /// first-keystroke latency on top of the wayland round-trip.
    /// Direct dispatch (below) removes that wait — the commit reaches
    /// wayland in the same call as handle_input.
    subscribers: Arc<RwLock<Vec<Sender<EngineEvent>>>>,
    /// Synchronous dispatch callback for events that need zero-
    /// latency delivery. Invoked from handle_input AFTER the
    /// subscriber fan-out (and after all state writes are released)
    /// so the callback can do whatever work it needs without re-
    /// entering the engine. Used by the wayland backend to commit
    /// text and press keysyms directly, bypassing the old
    /// engine→vkbd bridge's polling latency.
    ///
    /// Storage is `RefCell<Option<Rc<dyn Fn>>>` (not
    /// `RefCell<Option<Box<dyn Fn>>>`) because:
    ///   - The engine is only used from the GTK main thread; no
    ///     `thread::spawn` captures it. Sending it across threads
    ///     would be a bug, so we don't pay for `Send + Sync` on the
    ///     closure.
    ///   - `VirtualKeyboard` implementations hold thread-local
    ///     resources (Rc<>, GTK widgets, parking_lot::Mutex without
    ///     Send bounds). Requiring `Send + Sync` on the closure
    ///     would force every backend to wrap state in heavier
    ///     types — fighting the language for no real benefit.
    ///   - `Rc<dyn Fn>` (not `Box<dyn Fn>`) so the dispatch site
    ///     can `clone()` the Rc out of the RefCell before invoking
    ///     it, releasing the borrow before user code runs. This
    ///     prevents a latent panic: if a future callback ever calls
    ///     `set_direct_dispatch` (or `borrow_mut` on this RefCell
    ///     through any path), it would conflict with the immutable
    ///     borrow held across the call. Cloning an Rc is two
    ///     refcount bumps, ~10ns — cheap compared to the wayland
    ///     flush that follows.
    ///
    /// Cloning the engine (via `#[derive(Clone)]`) clones the Rc,
    /// not the closure. All clones of the engine share the same
    /// dispatch callback.
    direct_dispatch:
        std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<dyn Fn(&EngineEvent)>>>>,
}

impl KeyboardEngine {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(KeyboardState::new())),
            subscribers: Arc::new(RwLock::new(Vec::new())),
            direct_dispatch: std::rc::Rc::new(std::cell::RefCell::new(None)),
        }
    }

    /// Register a zero-latency dispatch callback. The callback is
    /// invoked synchronously from `handle_input` after subscribers
    /// have been fanned out. Used by the wayland backend so text
    /// commits reach the host in the same call as the user's tap —
    /// no polling wait, no mpsc queue.
    ///
    /// Setting overwrites any previous callback. The Rc allows
    /// the dispatch site to clone the handle out of storage before
    /// invoking it — so the callback can safely call
    /// set_direct_dispatch again without RefCell panic.
    pub fn set_direct_dispatch(&self, cb: std::rc::Rc<dyn Fn(&EngineEvent)>) {
        *self.direct_dispatch.borrow_mut() = Some(cb);
    }

    pub fn state_snapshot(&self) -> KeyboardState {
        self.state.read().clone()
    }

    pub fn keypad_state(&self) -> KeypadState {
        self.state.read().keypad
    }

    /// Directly set the keypad state WITHOUT emitting a StateChanged
    /// event. Used by the steno-exit path to reset to Normal after
    /// the user leaves stenotype mode — we've already handled the
    /// layout swap manually and don't want a second StateChanged to
    /// re-trigger the symbols/letters dance.
    pub fn set_keypad_state(&self, state: KeypadState) {
        self.state.write().keypad = state;
    }

    /// Reset state that's scoped to the current editing context.
    /// Called on focus changes so the new field starts with a clean
    /// slate. Cleared:
    ///   - undo_stack: an undo in the new field must not emit
    ///     backspaces against entries committed to the previous
    ///     field (would delete unrelated content).
    ///   - last_commit_was_swipe: a swipe-typed word in the new
    ///     field shouldn't get an auto-spaced leading space because
    ///     the previous field ended with a swipe.
    ///   - pre_transient_shift: CapsLock the user engaged for the
    ///     previous field (e.g. typing IDENTIFIERS in a code editor)
    ///     shouldn't survive into a password field or a chat
    ///     message where case-sensitivity differs.
    ///   - keypad (shift state): a CapsLock or one-shot Shifted from
    ///     the previous field shouldn't apply to the FIRST keypress
    ///     in the new field. Without this, focus switching mid-
    ///     sentence (where auto_cap_pending → Shifted) carries the
    ///     uppercase intent into the new field's first char before
    ///     SurroundingTextChanged arrives to re-evaluate.
    ///   - auto_cap_pending: same hazard from the other direction —
    ///     a stale "pending cap from previous field" must not cap
    ///     the new field's first letter.
    ///
    /// Surrounding text and cursor_pos are NOT cleared because the
    /// host will resend them on focus (and reading stale values for
    /// one tick is harmless — they only affect auto-cap and cursor
    /// jump detection, both of which re-evaluate on the next
    /// SurroundingTextChanged).
    ///
    /// Kept as `clear_undo_history` for backward-compat with the
    /// callers in the focus path; the broader cleanup happens
    /// here transparently.
    pub fn clear_undo_history(&self) {
        let mut s = self.state.write();
        s.undo_stack.clear();
        s.last_commit_was_swipe = false;
        s.pre_transient_shift = crate::keyboard::KeypadState::Normal;
        s.auto_cap_pending = false;
        // Reset shift level only if currently on a letter page.
        // Transient pages (Symbols/Numbers/Emoji) must NOT be reset
        // here — the view layout is still rendering that transient,
        // and clobbering the engine state would desync them. The
        // defer reconciler in the focus path is the authoritative
        // place for transient transitions (it swaps the view layout
        // and the engine state together via show_full_keyboard /
        // show_arrows_tray, both of which call set_keypad_state).
        if matches!(
            s.keypad,
            crate::keyboard::KeypadState::Shifted
                | crate::keyboard::KeypadState::CapsLock
        ) {
            s.keypad = crate::keyboard::KeypadState::Normal;
        }
    }

    /// Look at the most recently committed text without popping the
    /// undo stack. Returns None if no commits have been made yet.
    /// Useful for features that want to act on the prior word
    /// (e.g. find a synonym for the word just typed).
    pub fn peek_last_commit(&self) -> Option<String> {
        self.state
            .read()
            .undo_stack
            .back()
            .map(|e| e.text.clone())
    }

    /// Concatenate the last N committed entries in chronological
    /// order and return the joined string. Used by tasks that need
    /// recent context — e.g. "rewrite the last sentence" gets the
    /// last 8-12 commits joined, which usually covers a full sentence
    /// of typing (single chars or short words per commit).
    ///
    /// Returns None if the stack is empty. The joined string is
    /// taken raw (each commit's text concatenated with no extra
    /// separators) since commits already contain whatever spacing
    /// the user typed.
    pub fn peek_last_n_commits(&self, n: usize) -> Option<String> {
        let state = self.state.read();
        if state.undo_stack.is_empty() {
            return None;
        }
        let start = state.undo_stack.len().saturating_sub(n);
        let joined: String = state
            .undo_stack
            .iter()
            .skip(start)
            .map(|e| e.text.as_str())
            .collect();
        if joined.is_empty() {
            None
        } else {
            Some(joined)
        }
    }

    /// Pop the last N entries off the undo stack and return:
    ///   - the backspace events emitted (already broadcast to
    ///     subscribers)
    ///   - the joined popped text in chronological order, so
    ///     callers can restore it on subsequent failure (e.g.
    ///     LLM call errored out and we want the original back)
    pub fn pop_last_n_for_rewrite(&self, n: usize) -> (Vec<EngineEvent>, String) {
        let mut events = Vec::new();
        let mut popped_chrono: Vec<String> = Vec::new();
        let mut state = self.state.write();
        let take = state.undo_stack.len().min(n);
        for _ in 0..take {
            if let Some(entry) = state.undo_stack.pop_back() {
                for _ in 0..entry.text.chars().count() {
                    events.push(EngineEvent::KeyAction(KeyAction::Backspace));
                }
                popped_chrono.push(entry.text);
            }
        }
        drop(state);
        // Fan out to subscribers so any UI watchers see the events.
        // Batched-lock pattern: one write lock across all events
        // rather than one per event (Undo can emit dozens of
        // Backspace events for an LLM rewrite).
        {
            let mut subs = self.subscribers.write();
            for ev in &events {
                subs.retain(|tx| tx.send(ev.clone()).is_ok());
            }
        }
        // Direct dispatch for synchronous wayland delivery. Without
        // this, the rewrite's backspace flood would still wait for
        // the next bridge tick — the user would see a long pause
        // followed by a burst of deletions, instead of an immediate
        // smooth replacement.
        //
        // Clone the Rc out before invoking so we don't hold the
        // RefCell borrow across user code (lets the callback safely
        // re-enter set_direct_dispatch if it ever needs to).
        let cb_handle = self.direct_dispatch.borrow().clone();
        if let Some(cb) = cb_handle {
            for ev in &events {
                cb(ev);
            }
        }
        // Reverse so the joined string is in original write order
        // (we popped from the back, so the first popped entry is
        // chronologically the latest).
        popped_chrono.reverse();
        let joined: String = popped_chrono.concat();
        (events, joined)
    }

    /// Register a new subscriber. Each subscriber gets its own
    /// `unbounded` channel; they don't compete for events. Returns the
    /// receiver — caller drains it on its own schedule (typically via a
    /// GLib timeout on the main thread).
    ///
    /// If a subscriber's receiver is dropped, future sends to its
    /// channel fail silently and we skip it.
    pub fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.subscribers.write().push(tx);
        rx
    }

    /// Process one input from the UI. Returns the events this dispatch
    /// produced (also broadcast via `subscribe()`).
    pub fn handle_input(&self, input: EngineInput) -> Vec<EngineEvent> {
        let mut emitted = vec![];

        match input {
            EngineInput::KeyTap(key) => {
                emitted.extend(self.handle_key_tap(&key));
            }
            EngineInput::KeyLongPress(key) => {
                debug!("Long-press on {:?}", key.label);
                // Emit an event so subscribers (the popover surface,
                // the microterm bridge, etc.) can react. The
                // microterm uses long-press on space to toggle a
                // status display.
                emitted.push(EngineEvent::KeyLongPress(key));
            }
            EngineInput::SwipeWord(word) => {
                if !word.is_empty() {
                    // Diagnostic: log the input word at entry so an
                    // on-device journal can confirm the engine arm
                    // was actually reached and what word it received.
                    info!("Swipe produced word: {}", word);
                    // Single write-lock acquisition for the whole arm.
                    // apply_shift_to_swipe used to acquire its own
                    // read lock for keypad; then we'd acquire write
                    // for the auto-space + after_keypress mutations.
                    // Inline the shift transformation here so all
                    // state access happens under one lock — one
                    // RwLock transition instead of two per swipe-word
                    // commit.
                    let (shifted, prepend_space) = {
                        let mut s = self.state.write();
                        let shifted = apply_shift_inline(&word, s.keypad);
                        let do_prepend = crate::gestures::swipe::swipe_auto_space()
                            && s.last_commit_was_swipe;
                        s.last_commit_was_swipe = true;
                        s.after_keypress();
                        (shifted, do_prepend)
                    };
                    // Diagnostic: log the post-shift / post-auto-space
                    // values BEFORE building final_word (which moves
                    // `shifted` into the format! call). If `word`
                    // came in non-empty but `shifted` is empty (rare
                    // but possible if apply_shift_inline mishandles a
                    // state), this surfaces it instead of silently
                    // emitting an empty TextCommitted.
                    info!(
                        "SwipeWord commit: input={:?} shifted={:?} prepend_space={}",
                        word, shifted, prepend_space
                    );
                    let final_word = if prepend_space {
                        format!(" {}", shifted)
                    } else {
                        shifted
                    };
                    emitted.push(EngineEvent::TextCommitted(final_word));
                } else {
                    info!("SwipeWord dropped: input word is empty");
                }
            }
            EngineInput::StenoCommit(text) => {
                if !text.is_empty() {
                    info!("Steno commit: {}", text);
                    // Same merge as SwipeWord: one write lock instead
                    // of read-for-shift then write-for-flag. Steno
                    // does NOT participate in swipe auto-spacing —
                    // phonemes chain directly and accepted words bring
                    // their own trailing space. Reset the swipe-
                    // sequence flag so a following swipe-typed word
                    // treats this as a boundary.
                    let final_text = {
                        let mut s = self.state.write();
                        let shifted = apply_shift_inline(&text, s.keypad);
                        s.last_commit_was_swipe = false;
                        s.after_keypress();
                        shifted
                    };
                    emitted.push(EngineEvent::TextCommitted(final_text));
                }
            }
            EngineInput::NamedKey(name) => {
                // Forward to the virtual keyboard as a keysym press. The
                // backend's `press_keysym` handles the XKB lookup.
                debug!("NamedKey -> Keysym({})", name);
                emitted.push(EngineEvent::KeyAction(KeyAction::Keysym(name.clone())));
                // A NamedKey (arrow keys, Home, End, Tab, etc.)
                // breaks the swipe-sequence context — the cursor
                // moved or focus shifted, and the next SwipeWord
                // should NOT auto-prepend a space. Without this
                // reset, SwipeWord('hello') → Right arrow →
                // SwipeWord('world') would commit ' world' with a
                // stray leading space (the host's SurroundingText
                // for the cursor change typically arrives after
                // the next user action, so we can't rely on the
                // cursor-jump check in SurroundingTextChanged to
                // fire in time).
                //
                // BackSpace via NamedKey deletes a character from the
                // host just like a tapped Backspace key. Mirror the
                // undo-stack shrink so steno's accept-then-backspace
                // path (which uses NamedKey to remove raw phonemes
                // before inserting the accepted word) doesn't leave
                // stale per-phoneme undo entries pointing to text
                // that's no longer in the host. Same shrink logic as
                // KeyAction::Backspace in handle_key_tap.
                //
                // Both mutations happen under one write lock — they're
                // logically a single state transition and merging the
                // acquires saves one lock pair per NamedKey dispatch.
                {
                    let mut s = self.state.write();
                    s.last_commit_was_swipe = false;
                    if name == "BackSpace" {
                        if let Some(entry) = s.undo_stack.back_mut() {
                            let mut chars: Vec<char> = entry.text.chars().collect();
                            if !chars.is_empty() {
                                chars.pop();
                            }
                            if chars.is_empty() {
                                s.undo_stack.pop_back();
                            } else {
                                entry.text = chars.into_iter().collect();
                            }
                        }
                    }
                }
            }
            EngineInput::SurroundingTextChanged { text, cursor } => {
                // Compute jumped + apply both mutations under one
                // write lock. Pre-merge: one write block for
                // surrounding_text/cursor/auto_cap, then another
                // write to clear last_commit_was_swipe if a cursor
                // jump was detected. Each parking_lot RwLock acquire
                // is cheap (~10ns) but SurroundingTextChanged fires
                // on every host text update on edit-focused fields,
                // so per-keystroke this would acquire write twice
                // when once suffices. Keep the steno clear OUTSIDE
                // the lock — steno::global() is unrelated state and
                // taking it under our write would risk deadlock if
                // anything in steno ever reads engine state.
                // Track keypad pre/post under the SAME lock so we can
                // skip the StateChanged emit when auto_cap didn't
                // actually change the state. SurroundingTextChanged
                // fires on every host text update on edit-focused
                // fields — typing "hello" produces 5+ events. Pre-
                // optimization, each emitted StateChanged ran through
                // the UI listener's full match (layout-name clone,
                // match block, potentially update_for_state across
                // every key) even when keypad was unchanged. The
                // auto_cap check only flips state at sentence-start
                // transitions; the other 90%+ of fires are no-ops.
                let (prev_state, new_state, jumped) = {
                    let mut s = self.state.write();
                    let prev_cursor = s.cursor_pos;
                    let prev_state = s.keypad;
                    s.surrounding_text = text;
                    s.cursor_pos = cursor;
                    s.auto_cap_pending = true;
                    s.check_auto_cap();
                    let jumped = cursor.abs_diff(prev_cursor) > 8;
                    if jumped {
                        // A cursor reposition breaks the swipe-to-
                        // swipe word sequence — the next swipe
                        // starts fresh (no leading auto-space).
                        s.last_commit_was_swipe = false;
                    }
                    (prev_state, s.keypad, jumped)
                };
                // Steno buffer safety: a normal keystroke advances the
                // cursor by a small amount (≤ a few chars per commit).
                // A larger or backward jump means the user repositioned
                // the cursor (tapped elsewhere in the field). The
                // accumulated phoneme buffer no longer corresponds to
                // text immediately before the cursor, so accepting a
                // suggestion would backspace the wrong characters.
                // Invalidate it. (Threshold is generous so multi-char
                // phoneme commits like "ng" don't trip it.)
                if jumped {
                    crate::steno::global().clear_buffer();
                }
                // Only emit when keypad actually transitioned. The
                // skip is safe because the UI listener's StateChanged
                // handler reacts to keypad-state transitions only —
                // layout switches (Symbols/Numbers/Emoji) and label
                // refreshes (Normal↔Shifted) — and those produce no
                // visible work if state matches what's already shown.
                if prev_state != new_state {
                    emitted.push(EngineEvent::StateChanged(new_state));
                }
            }
            EngineInput::VoicePartial(text) => {
                emitted.push(EngineEvent::VoiceTranscript {
                    text,
                    partial: true,
                });
            }
            EngineInput::VoiceFinal(text) => {
                // Empty transcripts are dropped: nothing to commit
                // (avoids a no-op TextCommitted) and the partial-
                // overlay clear is unnecessary since an empty final
                // means recording produced no usable text — the voice
                // bridge surfaces that case with its own "no speech
                // detected" status.
                if !text.is_empty() {
                    // Commit first (consumer expects the text in the
                    // input field), then emit the transcript event with
                    // partial=false so the UI can clear the partial
                    // overlay. The text is the same in both, so we send
                    // a clone to TextCommitted and move into the transcript.
                    emitted.push(EngineEvent::TextCommitted(text.clone()));
                    emitted.push(EngineEvent::VoiceTranscript {
                        text,
                        partial: false,
                    });
                }
            }
            EngineInput::Undo => {
                // Pop the most recent commit from the undo stack and
                // emit enough Backspace KeyActions to delete its
                // characters from the focused app. This works on any
                // text field that accepts BackSpace; it doesn't
                // require IM-v2 surrounding-text support.
                //
                // Note: this is a "blind" undo — if the user typed
                // since the last commit (which the engine tracks),
                // or if the focused app has its own undo, the
                // backspaces may delete the wrong chars. The right
                // way is `delete_surrounding_text` via input-method
                // protocol; we'll add that as a fallback when the
                // backend supports it.
                let entry = {
                    let mut s = self.state.write();
                    // Undo breaks the swipe-sequence context: the
                    // commit we're about to pop may have been the
                    // anchor that set last_commit_was_swipe=true.
                    // A subsequent swipe should NOT auto-prepend a
                    // space based on the undone prior. Clear here.
                    s.last_commit_was_swipe = false;
                    s.undo_stack.pop_back()
                };
                if let Some(entry) = entry {
                    let n = entry.text.chars().count();
                    info!(
                        "Undo: removing {} chars from {} ({:?})",
                        n, entry.source, entry.text
                    );
                    for _ in 0..n {
                        emitted.push(EngineEvent::KeyAction(KeyAction::Backspace));
                    }
                } else {
                    debug!("Undo requested but stack is empty");
                }
            }
            EngineInput::LlmRequest(task) => {
                debug!("LLM task requested: {}", task.label());
                emitted.push(EngineEvent::LlmRequest(task));
            }
            EngineInput::LlmCommit(text) => {
                // Same path as a voice final: commit and let
                // subscribers know. Empty guard matches VoiceFinal —
                // an empty LLM result has nothing to insert.
                if !text.is_empty() {
                    emitted.push(EngineEvent::TextCommitted(text));
                }
            }
        }

        // Side effect: every TextCommitted gets pushed onto the
        // undo stack. Bounded; oldest entries fall off the front.
        //
        // Empty TextCommitted events are SKIPPED — pushing them would
        // create a zombie undo entry: the user taps undo, the entry
        // pops, zero backspaces emit (chars().count() == 0), nothing
        // visible happens, and they think undo is broken. Defensive
        // guard: even though every CURRENT caller appears to guard
        // against empty text upstream, a future input path (a new
        // LLM commit mode, a new voice commit, a new SwipeWord
        // caller) could regress this without realizing.
        //
        // Lock once across all events rather than per-event: handle_input
        // is always called on the main thread, so a single longer-held
        // write lock is correctness-equivalent and skips ~N-1 lock
        // acquisitions per batched emission.
        {
            let mut s = self.state.write();
            for ev in &emitted {
                if let EngineEvent::TextCommitted(text) = ev {
                    if text.is_empty() {
                        continue;
                    }
                    s.undo_stack.push_back(crate::keyboard::state::UndoEntry {
                        text: text.clone(),
                        source: "commit",
                    });
                    while s.undo_stack.len() > 32 {
                        s.undo_stack.pop_front();
                    }
                }
            }
        }

        // Fan out to every subscriber. We retain only the senders
        // whose receivers haven't been dropped; this lets us garbage-
        // collect dead subscribers automatically.
        //
        // Same batched-lock pattern as the undo block above: one
        // write lock for all events instead of one per event.
        // For multi-event emissions like Undo (which emits N
        // Backspace events) this saves N-1 lock+retain cycles.
        {
            let mut subs = self.subscribers.write();
            for ev in &emitted {
                subs.retain(|tx| tx.send(ev.clone()).is_ok());
            }
        }
        // Synchronous direct dispatch (zero-latency path for the
        // wayland backend). Clone the Rc out of the RefCell before
        // invoking — releases the borrow before user code, so the
        // callback can safely call set_direct_dispatch without
        // RefCell panic. Two refcount bumps cost ~5ns each, far
        // less than the wayland flush this enables.
        let cb_handle = self.direct_dispatch.borrow().clone();
        if let Some(cb) = cb_handle {
            for ev in &emitted {
                cb(ev);
            }
        }
        emitted
    }

    fn handle_key_tap(&self, key: &Key) -> Vec<EngineEvent> {
        let mut emitted = vec![];

        // Any manual key tap breaks a swipe-to-swipe sequence: the
        // user typed something (character, space, punctuation,
        // backspace), so the next swipe word starts a fresh boundary
        // and must not auto-prepend a space.
        self.state.write().last_commit_was_swipe = false;

        match &key.action {
            KeyAction::Shift => {
                self.state.write().toggle_shift();
                let new_state = self.state.read().keypad;
                debug!("Shift toggled: {:?}", new_state);
                emitted.push(EngineEvent::StateChanged(new_state));
            }
            KeyAction::SymbolsToggle => {
                let mut s = self.state.write();
                let was_letter = matches!(
                    s.keypad,
                    KeypadState::Normal | KeypadState::Shifted | KeypadState::CapsLock
                );
                // ABC button always returns to letters from any
                // non-letter page (Symbols, Numbers, Emoji). The same
                // button on the letter page goes to Symbols.
                //
                // Letter-shift preservation: when entering a transient,
                // remember CapsLock so it survives the round-trip.
                // One-shot Shifted intentionally does NOT survive —
                // touch-keyboard convention treats any context switch
                // (including a dip into symbols) as consuming the
                // shift intent.
                s.keypad = match s.keypad {
                    KeypadState::Normal | KeypadState::Shifted => {
                        s.pre_transient_shift = KeypadState::Normal;
                        KeypadState::Symbols
                    }
                    KeypadState::CapsLock => {
                        s.pre_transient_shift = KeypadState::CapsLock;
                        KeypadState::Symbols
                    }
                    // Returning from a transient — restore the saved
                    // letter shift level (Normal or CapsLock). Reset
                    // the slot so a future entry without a fresh
                    // CapsLock doesn't re-engage stale state.
                    _ => {
                        let restored = s.pre_transient_shift;
                        s.pre_transient_shift = KeypadState::Normal;
                        restored
                    }
                };
                // Auto-cap re-check on transient → letter transition.
                // When the user typed '. ' inside symbols, the
                // SurroundingTextChanged that followed fired check_auto_cap
                // but it was blocked because keypad != Normal. Then a
                // subsequent keypress (the digit or punctuation in symbols)
                // cleared auto_cap_pending via after_keypress. By the time
                // they tap ABC, the auto-cap signal is gone. Re-running
                // check_auto_cap here picks it up if surrounding text still
                // ends with a sentence-end character.
                if was_letter {
                    // Going INTO a transient — no auto-cap re-check needed.
                } else {
                    s.check_auto_cap();
                }
                let new_state = s.keypad;
                drop(s);
                emitted.push(EngineEvent::StateChanged(new_state));
            }
            KeyAction::Numbers => {
                // Toggle between Numbers (second symbols page) and
                // Symbols (first symbols page). Lets users move
                // back and forth without an extra distinct action.
                // Coming from a letter page goes to Numbers
                // directly; from any other non-symbols page also
                // lands on Numbers.
                //
                // Same CapsLock preservation as SymbolsToggle: entering
                // the transient cycle from a letter page saves the
                // letter-shift level, so coming back via ABC restores
                // CapsLock if it was engaged.
                let mut s = self.state.write();
                s.keypad = match s.keypad {
                    KeypadState::Numbers => KeypadState::Symbols,
                    KeypadState::Symbols => KeypadState::Numbers,
                    KeypadState::Normal | KeypadState::Shifted => {
                        s.pre_transient_shift = KeypadState::Normal;
                        KeypadState::Numbers
                    }
                    KeypadState::CapsLock => {
                        s.pre_transient_shift = KeypadState::CapsLock;
                        KeypadState::Numbers
                    }
                    _ => KeypadState::Numbers,
                };
                let new_state = s.keypad;
                drop(s);
                emitted.push(EngineEvent::StateChanged(new_state));
            }
            KeyAction::Emoji => {
                // Save the letter-shift level just like SymbolsToggle
                // does, so tapping ABC after browsing emoji returns
                // to CapsLock if the user had it on. Without this,
                // CapsLock silently downgrades to Normal across an
                // emoji dip. Currently masked because there's no
                // emoji layout yet and the view listener immediately
                // resets to Normal — but the moment we ship an emoji
                // page, the preservation will be needed.
                let mut s = self.state.write();
                match s.keypad {
                    KeypadState::Normal | KeypadState::Shifted => {
                        s.pre_transient_shift = KeypadState::Normal;
                    }
                    KeypadState::CapsLock => {
                        s.pre_transient_shift = KeypadState::CapsLock;
                    }
                    // Already on a transient — preserve whatever was
                    // saved on the original letter→transient hop.
                    _ => {}
                }
                s.keypad = KeypadState::Emoji;
                drop(s);
                emitted.push(EngineEvent::StateChanged(KeypadState::Emoji));
            }
            KeyAction::Voice => {
                // Don't flip keypad state to Voice — the Voice key is
                // a stateless toggle (start/stop recording). The
                // current letters/numbers/etc page should stay on
                // screen so the user can still tap-correct or end
                // dictation by tapping Voice again. The voice bridge
                // subscribes to this KeyAction and toggles its
                // recording state accordingly.
                //
                // If a future revision wants a dedicated voice page
                // (e.g. showing a waveform, transcript preview,
                // language picker), we can re-introduce the state
                // flip + an explicit "exit voice mode" key.
                emitted.push(EngineEvent::KeyAction(KeyAction::Voice));
            }
            KeyAction::Backspace => {
                emitted.push(EngineEvent::KeyAction(KeyAction::Backspace));
                // Reflect this manual deletion in the undo stack. Each
                // backspace removes one char from the END of the most-
                // recent undo entry; if the entry empties, drop it.
                // Without this, the entry's stored text would over-
                // count what's actually still in the host, and a later
                // Undo would emit too many backspaces — eating into
                // unrelated content that preceded the commit. Example
                // pre-fix bug: swipe "hello" (entry "hello") → tap
                // backspace 3x (host now shows "he") → undo emits 5
                // backspaces → 3 chars of unrelated prior text deleted.
                let mut s = self.state.write();
                if let Some(entry) = s.undo_stack.back_mut() {
                    // chars().count() and char-pop both work in
                    // Unicode scalar values, matching how one
                    // backspace deletes one user-perceived character.
                    let mut chars: Vec<char> = entry.text.chars().collect();
                    if !chars.is_empty() {
                        chars.pop();
                    }
                    if chars.is_empty() {
                        s.undo_stack.pop_back();
                    } else {
                        entry.text = chars.into_iter().collect();
                    }
                }
            }
            KeyAction::Return => {
                emitted.push(EngineEvent::KeyAction(KeyAction::Return));
                self.state.write().after_keypress();
            }
            KeyAction::Tab => {
                // Emit as a Keysym press — NOT a TextCommitted of
                // '\t'. The TextCommitted path runs through the
                // wayland v2 backend's keymap (per-character lookup),
                // and the keymap has no entry for the tab control
                // character → commit_text silently skips it. The
                // bridge already has a Tab → press_keysym("Tab") arm;
                // route there. after_keypress runs to mirror the
                // Return treatment (one-shot shift reset, etc).
                emitted.push(EngineEvent::KeyAction(KeyAction::Tab));
                self.state.write().after_keypress();
            }
            KeyAction::HideKeyboard | KeyAction::LanguageSwitch => {
                emitted.push(EngineEvent::KeyAction(key.action.clone()));
            }
            KeyAction::Settings => {
                // The view layer handles the settings panel; the engine
                // just broadcasts the action so subscribers can react.
                emitted.push(EngineEvent::KeyAction(KeyAction::Settings));
            }
            KeyAction::CodeLangCycle => {
                // Pure broadcast — the UI listener cycles the
                // code_mode atomic and rebuilds the cardinal-grid-
                // words layout. CodeMode is process/UI state, not
                // engine state, so the engine doesn't own the cycle
                // logic (keeps the engine layout-agnostic).
                emitted.push(EngineEvent::KeyAction(KeyAction::CodeLangCycle));
            }
            KeyAction::Keysym(_) => {
                emitted.push(EngineEvent::KeyAction(key.action.clone()));
            }
            KeyAction::Character | KeyAction::Literal(_) | KeyAction::Space => {
                // Single write-lock acquisition for the whole arm.
                // Old code acquired read for shifted, then write for
                // the post-commit mutations. The write was always
                // needed when emit_char returns Some, so the read was
                // pure overhead — write was about to follow anyway.
                // Acquiring write up-front collapses the read-release
                // / write-acquire ping-pong to a single transition,
                // and the held write briefly blocks the listener
                // thread's reader but the commit closure is short.
                let mut s = self.state.write();
                let shifted = s.keypad.is_shifted();
                if let Some(ch) = key.emit_char(shifted) {
                    emitted.push(EngineEvent::TextCommitted(ch));
                    // Manual character commit (incl. explicit space)
                    // breaks the swipe-sequence context. Without this
                    // reset, a sequence like "SwipeWord('hello') →
                    // tap space → SwipeWord('world')" double-spaces:
                    // the manual space adds one, then 'world' auto-
                    // prepends because last_commit_was_swipe was
                    // still true from 'hello'.
                    s.last_commit_was_swipe = false;
                    s.after_keypress();
                }
                // s drops here, releasing the lock.
            }
            KeyAction::NoOp => {
                tracing::trace!("NoOp tap on key {:?}", key.label);
            }
            KeyAction::Undo => {
                // Pop the undo stack and emit one BackSpace per
                // character of the popped text. Same logic as
                // EngineInput::Undo at the outer match — kept in
                // sync. Also clears last_commit_was_swipe so a
                // following swipe doesn't auto-prepend a space based
                // on the undone prior commit.
                let entry = {
                    let mut s = self.state.write();
                    s.last_commit_was_swipe = false;
                    s.undo_stack.pop_back()
                };
                if let Some(entry) = entry {
                    let n = entry.text.chars().count();
                    info!(
                        "Undo (via key): removing {} chars ({:?})",
                        n, entry.text
                    );
                    for _ in 0..n {
                        emitted.push(EngineEvent::KeyAction(KeyAction::Backspace));
                    }
                }
            }
            KeyAction::Llm => {
                emitted.push(EngineEvent::KeyAction(KeyAction::Llm));
            }
            KeyAction::ShowFullKeyboard => {
                // Emit so the keyboard_view's event drain can pick
                // it up and swap from arrows tray → full layout.
                emitted.push(EngineEvent::KeyAction(KeyAction::ShowFullKeyboard));
            }
            KeyAction::Steno(button_id) => {
                // Steno taps are normally intercepted by the
                // keyboard_view's gesture dispatcher BEFORE reaching
                // the engine — that path feeds the steno module's
                // pair-stroke engine and commits via SwipeWord.
                //
                // If one arrives here it means either: (a) the
                // intercept was bypassed (test path, edge case), or
                // (b) the steno layout is active but the user
                // double-routed a tap. Either way the engine itself
                // has no knowledge of the phoneme table, so we just
                // log and drop. No commit happens; the user sees
                // no character appear — which is the right failure
                // mode (won't accidentally insert garbage).
                debug!("engine: stray Steno({}) tap dropped (should be intercepted upstream)", button_id);
            }
        }

        emitted
    }

    /// If the keyboard is shifted at the start of a swipe, capitalize the
    /// first letter. If CapsLock, uppercase all of it.
    ///
    /// Acquires a read lock for the keypad state. New code that already
    /// holds a state lock should call `apply_shift_inline` directly to
    /// avoid the redundant lock acquisition.
    #[allow(dead_code)]
    fn apply_shift_to_swipe(&self, word: &str) -> String {
        let keypad = self.state.read().keypad;
        apply_shift_inline(word, keypad)
    }
}

/// Lock-free shift application — takes keypad as a value so callers
/// that already hold a state lock can apply shift without releasing
/// and re-acquiring. SwipeWord and StenoCommit use this directly to
/// save one RwLock transition per commit.
fn apply_shift_inline(word: &str, keypad: KeypadState) -> String {
    match keypad {
        KeypadState::Shifted => {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
        KeypadState::CapsLock => word.to_uppercase(),
        _ => word.to_string(),
    }
}

impl Default for KeyboardEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_letter_emits_text() {
        let engine = KeyboardEngine::new();
        let events = engine.handle_input(EngineInput::KeyTap(Key::character("a")));
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::TextCommitted(s)] if s == "a"
        ));
    }

    #[test]
    fn shift_then_letter_emits_uppercase() {
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::KeyTap(Key::modifier("⇧", KeyAction::Shift)));
        let events = engine.handle_input(EngineInput::KeyTap(Key::character("a")));
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::TextCommitted(s)] if s == "A"
        ));
        // One-shot shift resets:
        let events = engine.handle_input(EngineInput::KeyTap(Key::character("b")));
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::TextCommitted(s)] if s == "b"
        ));
    }

    #[test]
    fn swipe_word_with_shifted_capitalizes_first() {
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::KeyTap(Key::modifier("⇧", KeyAction::Shift)));
        let events = engine.handle_input(EngineInput::SwipeWord("hello".into()));
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::TextCommitted(s)] if s == "Hello"
        ));
    }

    #[test]
    fn backspace_emits_key_action() {
        let engine = KeyboardEngine::new();
        let key = Key::special("⌫", KeyAction::Backspace);
        let events = engine.handle_input(EngineInput::KeyTap(key));
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::KeyAction(KeyAction::Backspace)]
        ));
    }

    #[test]
    fn tab_emits_key_action_not_text() {
        // Tab must route via KeyAction(Tab) so the bridge calls
        // press_keysym("Tab"); routing via TextCommitted("\t") would
        // hit the wayland v2 keymap-based commit path, which has no
        // entry for the tab control character → silently dropped.
        // Pinning this prevents a future refactor from folding Tab
        // back into the Character group.
        let engine = KeyboardEngine::new();
        let key = Key::special("Tab", KeyAction::Tab);
        let events = engine.handle_input(EngineInput::KeyTap(key));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EngineEvent::KeyAction(KeyAction::Tab))),
            "Tab must emit KeyAction(Tab); got {:?}",
            events
        );
        assert!(
            !events.iter().any(|e| matches!(e, EngineEvent::TextCommitted(_))),
            "Tab must NOT emit TextCommitted (would be lost on v2)"
        );
    }

    #[test]
    fn cardinal_swipes_do_not_auto_space() {
        // Cardinal swipes commit single characters via StenoCommit;
        // consecutive cardinal swipes must NOT have spaces injected
        // between them. Pre-fix: cardinal swipes routed through
        // SwipeWord, and auto-space prepended ' ' to every cardinal
        // letter after the first → 'u e' instead of 'ue'. This test
        // pins the StenoCommit routing so that bug can't return.
        crate::gestures::swipe::set_swipe_auto_space(true);
        let engine = KeyboardEngine::new();
        let e1 = engine.handle_input(EngineInput::StenoCommit("u".into()));
        assert_eq!(
            e1.iter().find_map(|e| match e {
                EngineEvent::TextCommitted(s) => Some(s.clone()),
                _ => None,
            })
            .as_deref(),
            Some("u")
        );
        let e2 = engine.handle_input(EngineInput::StenoCommit("e".into()));
        assert_eq!(
            e2.iter().find_map(|e| match e {
                EngineEvent::TextCommitted(s) => Some(s.clone()),
                _ => None,
            })
            .as_deref(),
            Some("e"),
            "consecutive cardinal/steno commits must chain verbatim, not 'u e'"
        );
    }

    #[test]
    fn clear_undo_history_prevents_cross_field_undo() {
        let engine = KeyboardEngine::new();
        // Type a word — it goes onto the undo stack.
        engine.handle_input(EngineInput::SwipeWord("hello".into()));
        assert_eq!(engine.peek_last_commit().as_deref(), Some("hello"));
        // Simulate a focus change.
        engine.clear_undo_history();
        // Undo should now be a no-op (stack is empty) — no
        // backspaces emitted. Without this guard, the 5 backspaces
        // for "hello" would land in whatever field is focused now.
        let events = engine.handle_input(EngineInput::Undo);
        assert!(
            !events.iter().any(|e| matches!(e, EngineEvent::KeyAction(KeyAction::Backspace))),
            "undo after focus-change must not emit backspaces; got {:?}",
            events
        );
    }

    #[test]
    fn layout_corresponding_keypad_state_classifies_correctly() {
        use crate::layouts::Layout;
        // Letter primaries → Normal. The engine's keypad state
        // tracks shift on top of this; the layout itself maps to
        // Normal as the base state.
        assert_eq!(
            Layout::default_qwerty().corresponding_keypad_state(),
            KeypadState::Normal
        );
        // Transient pages → their corresponding state.
        assert_eq!(
            Layout::default_symbols().corresponding_keypad_state(),
            KeypadState::Symbols
        );
        assert_eq!(
            Layout::default_numbers().corresponding_keypad_state(),
            KeypadState::Numbers
        );
        // Arrows is a view-only overlay — Normal is correct (arrows
        // keys emit keysym presses directly, not text). Pinning this
        // because if it ever returned a transient state, the
        // engine-view sync from show_arrows_tray would put the
        // engine in a Symbols-like state and the next ?123 tap would
        // be a dead button.
        assert_eq!(
            Layout::default_arrows().corresponding_keypad_state(),
            KeypadState::Normal
        );
    }

    #[test]
    fn layout_is_transient_page_classifies_correctly() {
        use crate::layouts::Layout;
        // The is_transient_page classification gates the
        // StateChanged handler's "restore last_letter_layout vs
        // update_in_place" decision. Pinning so a future layout
        // rename doesn't quietly break the restoration path.
        assert!(!Layout::default_qwerty().is_transient_page());
        assert!(Layout::default_symbols().is_transient_page());
        assert!(Layout::default_numbers().is_transient_page());
        assert!(Layout::default_arrows().is_transient_page());
    }

    #[test]
    fn capslock_survives_symbols_visit() {
        // User in CapsLock, dips into symbols for a digit, returns.
        // CapsLock should still be engaged — otherwise typing
        // ALL-CAPS phrases with numbers in them ("ITEM 5 IS HERE")
        // becomes "ITEM 5 is here" because CapsLock got dropped at
        // the dip into symbols. Real-world annoyance worth pinning.
        let engine = KeyboardEngine::new();
        // Engage CapsLock (two shift taps: Normal → Shifted → CapsLock).
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "⇧",
            KeyAction::Shift,
        )));
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "⇧",
            KeyAction::Shift,
        )));
        assert_eq!(engine.keypad_state(), KeypadState::CapsLock);

        // Dip into symbols.
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "?123",
            KeyAction::SymbolsToggle,
        )));
        assert_eq!(engine.keypad_state(), KeypadState::Symbols);

        // Return via ABC. CapsLock should be restored.
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "ABC",
            KeyAction::SymbolsToggle,
        )));
        assert_eq!(
            engine.keypad_state(),
            KeypadState::CapsLock,
            "CapsLock must survive a symbols dip; got {:?}",
            engine.keypad_state()
        );
    }

    #[test]
    fn one_shot_shifted_does_not_survive_symbols_visit() {
        // Touch-keyboard convention: one-shot Shifted is consumed
        // by ANY context switch including dipping into symbols.
        // The user paused to do something else, so the shift
        // intent is stale by the time they return.
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "⇧",
            KeyAction::Shift,
        )));
        assert_eq!(engine.keypad_state(), KeypadState::Shifted);

        // Dip into symbols, return.
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "?123",
            KeyAction::SymbolsToggle,
        )));
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "ABC",
            KeyAction::SymbolsToggle,
        )));
        assert_eq!(
            engine.keypad_state(),
            KeypadState::Normal,
            "one-shot Shifted should be consumed by the symbols visit; got {:?}",
            engine.keypad_state()
        );
    }

    #[test]
    fn capslock_survives_numbers_round_trip() {
        // Same preservation must hold for the ?#= entry path
        // (KeyAction::Numbers), not just ?123 (SymbolsToggle).
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "⇧",
            KeyAction::Shift,
        )));
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "⇧",
            KeyAction::Shift,
        )));
        assert_eq!(engine.keypad_state(), KeypadState::CapsLock);
        // Enter the cycle via Numbers (?#=) and return via ABC.
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "?#=",
            KeyAction::Numbers,
        )));
        engine.handle_input(EngineInput::KeyTap(Key::special(
            "ABC",
            KeyAction::SymbolsToggle,
        )));
        assert_eq!(
            engine.keypad_state(),
            KeypadState::CapsLock,
            "CapsLock must survive a Numbers entry too"
        );
    }

    #[test]
    fn empty_commits_do_not_create_zombie_undo_entries() {
        // Empty TextCommitted events must not push to the undo
        // stack — otherwise tapping undo would pop the empty entry,
        // emit zero backspaces, and silently do nothing, leaving the
        // user confused why undo "didn't work".
        let engine = KeyboardEngine::new();
        // Empty voice final — guarded at handler, no commit emitted.
        let events = engine.handle_input(EngineInput::VoiceFinal(String::new()));
        assert!(
            !events.iter().any(|e| matches!(e, EngineEvent::TextCommitted(_))),
            "empty VoiceFinal must not emit TextCommitted"
        );
        // Empty LLM commit — same guard.
        let events = engine.handle_input(EngineInput::LlmCommit(String::new()));
        assert!(
            !events.iter().any(|e| matches!(e, EngineEvent::TextCommitted(_))),
            "empty LlmCommit must not emit TextCommitted"
        );
        // Undo stack must still be empty.
        assert!(engine.peek_last_commit().is_none());
        // Now type a real word, then verify undo works on the FIRST
        // press (no zombie entry in front of it).
        engine.handle_input(EngineInput::SwipeWord("real".into()));
        let undo = engine.handle_input(EngineInput::Undo);
        let bs_count = undo
            .iter()
            .filter(|e| matches!(e, EngineEvent::KeyAction(KeyAction::Backspace)))
            .count();
        assert_eq!(bs_count, 4, "undo must remove 'real' immediately, not after a zombie entry");
    }

    #[test]
    fn backspace_shrinks_recent_undo_entry() {
        // Pre-fix bug: swipe "hello" then tap Backspace 3 times then
        // tap Undo → engine emits 5 backspaces against a host that
        // only has 2 chars left, deleting 3 chars of UNRELATED prior
        // content. The fix shrinks the most-recent undo entry by 1
        // char per backspace, so undo only deletes what's still there.
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::SwipeWord("hello".into()));
        let bs = Key::special("⌫", KeyAction::Backspace);
        // Three manual backspaces — host now shows "he".
        for _ in 0..3 {
            engine.handle_input(EngineInput::KeyTap(bs.clone()));
        }
        // Undo: should now emit only 2 backspaces (remaining "he"),
        // not 5.
        let undo = engine.handle_input(EngineInput::Undo);
        let bs_count = undo
            .iter()
            .filter(|e| matches!(e, EngineEvent::KeyAction(KeyAction::Backspace)))
            .count();
        assert_eq!(
            bs_count, 2,
            "undo after partial backspace must only delete what's still there, got {}",
            bs_count
        );
        // Stack is now empty — next undo a no-op.
        assert!(engine.peek_last_commit().is_none());
    }

    #[test]
    fn backspace_pops_empty_entry() {
        // Tapping Backspace as many times as the entry's length should
        // empty the entry and pop it, NOT leave a stale empty entry.
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::SwipeWord("ab".into()));
        let bs = Key::special("⌫", KeyAction::Backspace);
        engine.handle_input(EngineInput::KeyTap(bs.clone()));
        engine.handle_input(EngineInput::KeyTap(bs.clone()));
        // Entry should be gone.
        assert!(engine.peek_last_commit().is_none());
        // Next backspace doesn't touch the stack (already empty).
        engine.handle_input(EngineInput::KeyTap(bs));
        assert!(engine.peek_last_commit().is_none());
    }

    #[test]
    fn named_backspace_shrinks_undo_too() {
        // The steno accept path sends NamedKey("BackSpace") to delete
        // raw phonemes before inserting the accepted word. That path
        // must also shrink undo, or the per-phoneme entries persist
        // and undo points at text that's no longer in the host.
        let engine = KeyboardEngine::new();
        // Two phoneme commits via StenoCommit (host has "ka", stack
        // has ["k","a"]).
        engine.handle_input(EngineInput::StenoCommit("k".into()));
        engine.handle_input(EngineInput::StenoCommit("a".into()));
        // Steno accept simulates: 2 BackSpace via NamedKey, then a
        // StenoCommit of the chosen word.
        engine.handle_input(EngineInput::NamedKey("BackSpace".into()));
        engine.handle_input(EngineInput::NamedKey("BackSpace".into()));
        // The "k" and "a" entries should be gone.
        assert!(
            engine.peek_last_commit().is_none(),
            "phoneme entries must be cleared after their host text was deleted"
        );
        engine.handle_input(EngineInput::StenoCommit("cat ".into()));
        // Now only "cat " on stack.
        assert_eq!(engine.peek_last_commit().as_deref(), Some("cat "));
    }

    fn committed_text(events: &[EngineEvent]) -> Option<String> {
        events.iter().find_map(|e| match e {
            EngineEvent::TextCommitted(s) => Some(s.clone()),
            _ => None,
        })
    }

    #[test]
    fn auto_space_behavior_sequential() {
        // One test covers all auto-space cases sequentially so the
        // process-wide SWIPE_AUTO_SPACE flag isn't raced by parallel
        // test execution.

        // --- ON: consecutive swipes separate with a space ---
        crate::gestures::swipe::set_swipe_auto_space(true);
        let engine = KeyboardEngine::new();
        let e1 = engine.handle_input(EngineInput::SwipeWord("hello".into()));
        assert_eq!(
            committed_text(&e1).as_deref(),
            Some("hello"),
            "first swipe: no leading space"
        );
        let e2 = engine.handle_input(EngineInput::SwipeWord("world".into()));
        assert_eq!(
            committed_text(&e2).as_deref(),
            Some(" world"),
            "second swipe: leading space"
        );

        // --- a tap between swipes breaks the sequence ---
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::SwipeWord("hello".into()));
        engine.handle_input(EngineInput::KeyTap(Key::character(",")));
        let e = engine.handle_input(EngineInput::SwipeWord("world".into()));
        assert_eq!(
            committed_text(&e).as_deref(),
            Some("world"),
            "tap breaks auto-space"
        );

        // --- a manual SPACE between swipes breaks the sequence ---
        // User-visible bug pre-fix: SwipeWord('hello') → tap space →
        // SwipeWord('world') → host shows "hello  world" with DOUBLE
        // space. The tap space added one; 'world' auto-prepended
        // because last_commit_was_swipe was still true from 'hello'.
        // Post-fix: manual Space commits reset last_commit_was_swipe,
        // so 'world' commits without leading space.
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::SwipeWord("hello".into()));
        // Use the Space key (KeyAction::Space) — distinct from a
        // character key that emits ' '. The bug was specifically in
        // the Space arm's handling.
        engine.handle_input(EngineInput::KeyTap(
            Key::special(" ", KeyAction::Space),
        ));
        let e = engine.handle_input(EngineInput::SwipeWord("world".into()));
        assert_eq!(
            committed_text(&e).as_deref(),
            Some("world"),
            "manual space between swipes must reset swipe-sequence — \
             post-tap-space, 'world' commits without leading space"
        );

        // --- a NamedKey (arrow / Tab / etc.) between swipes also
        // breaks the sequence. The cursor moved or focus shifted;
        // the next swipe should commit fresh. Pre-fix this slipped
        // through because NamedKey's arm forwarded the keysym but
        // didn't touch last_commit_was_swipe — and the host's
        // SurroundingText for the cursor change often doesn't
        // arrive before the user's next action.
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::SwipeWord("hello".into()));
        engine.handle_input(EngineInput::NamedKey("Right".into()));
        let e = engine.handle_input(EngineInput::SwipeWord("world".into()));
        assert_eq!(
            committed_text(&e).as_deref(),
            Some("world"),
            "NamedKey (cursor-move) between swipes must reset \
             swipe-sequence — post-arrow, 'world' commits without \
             leading space"
        );

        // --- steno commits never auto-space, even with flag ON ---
        let engine = KeyboardEngine::new();
        let e1 = engine.handle_input(EngineInput::StenoCommit("k".into()));
        assert_eq!(committed_text(&e1).as_deref(), Some("k"));
        let e2 = engine.handle_input(EngineInput::StenoCommit("a".into()));
        assert_eq!(committed_text(&e2).as_deref(), Some("a"), "steno phonemes chain, no space");
        let e3 = engine.handle_input(EngineInput::SwipeWord("word".into()));
        assert_eq!(
            committed_text(&e3).as_deref(),
            Some("word"),
            "swipe after steno is a fresh boundary"
        );

        // --- OFF: never prepends ---
        crate::gestures::swipe::set_swipe_auto_space(false);
        let engine = KeyboardEngine::new();
        engine.handle_input(EngineInput::SwipeWord("hello".into()));
        let e = engine.handle_input(EngineInput::SwipeWord("world".into()));
        assert_eq!(
            committed_text(&e).as_deref(),
            Some("world"),
            "auto-space off: no leading space"
        );

        // Restore default for any other test that assumes it.
        crate::gestures::swipe::set_swipe_auto_space(true);
    }
}
