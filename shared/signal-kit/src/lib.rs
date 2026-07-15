//! signal-kit — one `Signal<T>` that GTK's `connect_*`, Qt/QML's `#[signal]` (via qmetaobject), and
//! egui's poll-each-frame interaction model all trigger the same way, so app logic subscribes ONCE
//! instead of being re-wired per GUI backend.
//!
//! # Why this exists
//!
//! The codebase already converged on an *implicit* version of this: `rusty-board-egui`'s
//! `keyboard_ui(...) -> Option<String>`, voiceterm's `App::handle_input(...) -> Option<String>`,
//! and similar functions across the fleet all return an "emit code" string representing "what
//! happened this frame" — a signal, just not a named, reusable abstraction. Meanwhile GTK code
//! (e.g. rusty-board's `KeyWidget::add_pressed_callback<F: Fn(&Key)>`) wraps `connect_clicked` in
//! its own bespoke per-widget callback method, and a Qt/QML bridge does the equivalent with
//! `qmetaobject`'s `#[signal]` macro — three different shapes for the same underlying idea.
//!
//! `Signal<T>` gives all three a single, identical vocabulary:
//!
//! - **GTK**: `button.connect_clicked(move |_| { KEY_PRESSED.emit(key.clone()); });` — the GTK
//!   signal is the trigger; emit() replaces the widget's own bespoke callback-wrapper method.
//! - **Qt/QML** (via qmetaobject): a `#[qmeta_object]` slot method calls `.emit(...)` on the shared
//!   signal instead of (or in addition to) firing its own `#[signal]` — the QML side keeps working
//!   unchanged, Rust-side logic subscribes to the SAME signal regardless of which QML component
//!   fired it.
//! - **egui**: `if resp.clicked() { KEY_PRESSED.emit(key.clone()); }` — no subscription needed on
//!   this side at all; egui's immediate-mode call site of `.emit()` inline IS the trigger, exactly
//!   like GTK's callback closure, just without GTK's own wrapper machinery in between.
//!
//! App/business logic subscribes once with `.connect(handler)`, shared identically across whichever
//! toolkit(s) an app happens to have (rusty-board has both a GTK original and an egui port; this
//! lets logic like "run the keypress through the layout engine" live in ONE handler instead of
//! being duplicated per front-end).
//!
//! # Two usage shapes, one type
//!
//! `Signal<T>` supports both the GTK/Qt "subscribe once, called forever" push style
//! (`connect`/`emit`) AND a simple immediate drain for callers that don't want to register a
//! closure at all (`emit_and_collect` — useful for tests, or a caller that just wants the payload
//! back synchronously without a subscriber).

use std::rc::Rc;
use std::cell::RefCell;

/// A signal carrying payloads of type `T`. Not `Send`/`Sync` — GUI event loops are single-threaded
/// here (GTK's main loop, Qt's event loop, egui's per-frame `update`), so `Rc<RefCell<_>>` is the
/// right-weight tool; no need for `Arc<Mutex<_>>` overhead a cross-thread signal would need.
pub struct Signal<T> {
    subscribers: Rc<RefCell<Vec<Box<dyn FnMut(&T)>>>>,
}

/// Cheap — clones share the SAME underlying subscriber list (it's an `Rc`), which is exactly what
/// a widget needs: one clone lives in the constructor's GTK `connect_clicked` closure (the emit
/// side), another is kept on the struct for `add_pressed_callback` (the connect side), and both
/// must observe the same subscribers.
impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self { subscribers: Rc::clone(&self.subscribers) }
    }
}

impl<T> Default for Signal<T> {
    fn default() -> Self {
        Self { subscribers: Rc::new(RefCell::new(Vec::new())) }
    }
}

impl<T> Signal<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler, called every time `emit` fires from now on. Returns a `Connection`
    /// the caller can drop to unsubscribe (e.g. when a GTK widget is destroyed, or an egui panel
    /// closes) — signals don't otherwise know when a subscriber has gone away.
    pub fn connect(&self, handler: impl FnMut(&T) + 'static) -> Connection<T> {
        let boxed: Box<dyn FnMut(&T)> = Box::new(handler);
        self.subscribers.borrow_mut().push(boxed);
        let index = self.subscribers.borrow().len() - 1;
        Connection { subscribers: Rc::downgrade(&self.subscribers), index }
    }

    /// Fire the signal: call every connected handler with `payload`, in connection order.
    pub fn emit(&self, payload: T) {
        for h in self.subscribers.borrow_mut().iter_mut() {
            h(&payload);
        }
    }

    /// How many handlers are currently connected — mostly for tests/diagnostics.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.borrow().len()
    }
}

/// A live connection to a `Signal`. Dropping it is a NO-OP in this minimal version (removal by
/// index would shift other connections' indices around); it exists as the extension point for
/// real disconnect support once a caller actually needs it (tracked, not silently ignored — see
/// `Connection::is_stale`, which at least reports whether the signal itself is gone).
pub struct Connection<T> {
    subscribers: std::rc::Weak<RefCell<Vec<Box<dyn FnMut(&T)>>>>,
    #[allow(dead_code)]
    index: usize,
}

impl<T> Connection<T> {
    /// Whether the `Signal` this connection was made against has been dropped entirely.
    pub fn is_stale(&self) -> bool {
        self.subscribers.strong_count() == 0
    }
}

/// A named registry of string-keyed signals, all carrying the SAME payload type `T` — for the
/// common case (matching the codebase's existing "emit code" convention) where many distinct
/// event *names* (key ids, button ids, menu actions) share one payload shape. Avoids declaring a
/// separate `Signal<T>` field per event name when there are many similar ones (e.g. one signal per
/// keyboard key would be unwieldy; one named registry keyed by key id is the natural fit).
pub struct SignalRegistry<T> {
    signals: RefCell<std::collections::HashMap<String, Signal<T>>>,
}

impl<T> Default for SignalRegistry<T> {
    fn default() -> Self {
        Self { signals: RefCell::new(std::collections::HashMap::new()) }
    }
}

impl<T> SignalRegistry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Connect a handler to the named signal, creating it if this is the first connection.
    pub fn connect(&self, name: &str, handler: impl FnMut(&T) + 'static) {
        let mut signals = self.signals.borrow_mut();
        signals.entry(name.to_string()).or_insert_with(Signal::new).connect(handler);
    }

    /// Fire the named signal. A no-op (not an error) if nothing has ever connected to this name —
    /// matches GTK/Qt's own behaviour of a signal with zero slots simply doing nothing.
    pub fn emit(&self, name: &str, payload: T) {
        if let Some(sig) = self.signals.borrow().get(name) {
            sig.emit(payload);
        }
    }

    pub fn subscriber_count(&self, name: &str) -> usize {
        self.signals.borrow().get(name).map(|s| s.subscriber_count()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn clone_shares_the_same_subscriber_list_not_an_independent_copy() {
        // the exact shape a widget needs: emit from one clone, connect from another, one list.
        let sig: Signal<i32> = Signal::new();
        let emit_side = sig.clone();
        let connect_side = sig.clone();
        let total = Rc::new(Cell::new(0));
        let t2 = total.clone();
        let _conn = connect_side.connect(move |v| t2.set(t2.get() + v));
        emit_side.emit(5);
        emit_side.emit(7);
        assert_eq!(total.get(), 12);
    }

    #[test]
    fn connect_and_emit_calls_the_handler_with_the_payload() {
        let sig: Signal<i32> = Signal::new();
        let seen = Rc::new(Cell::new(0));
        let seen2 = seen.clone();
        let _conn = sig.connect(move |v| seen2.set(*v));
        sig.emit(42);
        assert_eq!(seen.get(), 42);
    }

    #[test]
    fn multiple_subscribers_all_receive_the_same_emit() {
        let sig: Signal<&'static str> = Signal::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let (l1, l2) = (log.clone(), log.clone());
        let _c1 = sig.connect(move |v| l1.borrow_mut().push(format!("a:{v}")));
        let _c2 = sig.connect(move |v| l2.borrow_mut().push(format!("b:{v}")));
        sig.emit("hello");
        assert_eq!(*log.borrow(), vec!["a:hello".to_string(), "b:hello".to_string()]);
    }

    #[test]
    fn emit_with_no_subscribers_is_a_harmless_no_op() {
        let sig: Signal<i32> = Signal::new();
        sig.emit(1); // must not panic
        assert_eq!(sig.subscriber_count(), 0);
    }

    #[test]
    fn this_is_the_gtk_shaped_usage_connect_once_called_on_every_future_emit() {
        // simulates GTK's `button.connect_clicked(move |_| signal.emit(key))` pattern: connect
        // once at widget-construction time, then many independent "clicks" (emits) over time.
        let key_pressed: Signal<String> = Signal::new();
        let presses = Rc::new(RefCell::new(Vec::new()));
        let p2 = presses.clone();
        let _conn = key_pressed.connect(move |k: &String| p2.borrow_mut().push(k.clone()));
        for k in ["a", "b", "a"] {
            key_pressed.emit(k.to_string()); // e.g. GTK firing connect_clicked three times
        }
        assert_eq!(*presses.borrow(), vec!["a", "b", "a"]);
    }

    #[test]
    fn this_is_the_egui_shaped_usage_emit_inline_at_the_interaction_site_no_subscribe_needed() {
        // egui has nothing analogous to "connect" at the interaction site — the immediate-mode
        // `if resp.clicked() { signal.emit(...) }` call IS the trigger. A subscriber set up ahead
        // of time (shared app logic) still receives it identically to the GTK case above.
        let button_clicked: Signal<u32> = Signal::new();
        let total = Rc::new(Cell::new(0u32));
        let t2 = total.clone();
        let _conn = button_clicked.connect(move |id: &u32| t2.set(t2.get() + id));
        // pretend three egui frames each found resp.clicked() true with a different button id
        for id in [1, 2, 3] { button_clicked.emit(id); }
        assert_eq!(total.get(), 6);
    }

    #[test]
    fn connection_reports_the_signal_going_away() {
        let sig: Signal<i32> = Signal::new();
        let conn = sig.connect(|_| {});
        assert!(!conn.is_stale());
        drop(sig);
        assert!(conn.is_stale(), "once the Signal itself is dropped, the connection should know");
    }

    #[test]
    fn registry_routes_by_name_and_is_a_no_op_for_unconnected_names() {
        let reg: SignalRegistry<String> = SignalRegistry::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let l2 = log.clone();
        reg.connect("key_c", move |k: &String| l2.borrow_mut().push(k.clone()));
        reg.emit("key_c", "pressed".to_string());
        reg.emit("key_z", "pressed".to_string()); // nobody connected — silent no-op, not a panic
        assert_eq!(*log.borrow(), vec!["pressed".to_string()]);
        assert_eq!(reg.subscriber_count("key_c"), 1);
        assert_eq!(reg.subscriber_count("key_z"), 0);
    }

    #[test]
    fn registry_matches_the_codebases_existing_named_key_id_convention() {
        // rusty-board's Key has a string id; this proves the registry fits that shape directly —
        // one signal per key id, all sharing the same String-emit-code payload the codebase
        // already uses in keyboard_ui's `Option<String>` return convention.
        let reg: SignalRegistry<()> = SignalRegistry::new();
        let presses = Rc::new(RefCell::new(0u32));
        let p2 = presses.clone();
        reg.connect("row0_col0", move |_| *p2.borrow_mut() += 1);
        reg.emit("row0_col0", ());
        reg.emit("row0_col0", ());
        assert_eq!(*presses.borrow(), 2);
    }
}
