//! MicrotermView — a thin glanceable-info surface that lives just
//! above the keyboard's last row.

use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tracing::debug;

#[derive(Debug, Clone)]
pub enum MicrotermContent {
    Status(String),
    VoicePreview { partial: String, level: f32 },
    /// Steno word-expansion suggestions. `buffer` is the phoneme
    /// run-in-progress (shown as a hint, e.g. "k a t"); `candidates`
    /// are tappable words. Tapping one commits it via the steno tap
    /// callback (accept_word + clear buffer). `fuzzy` marks which
    /// candidates were vowel-approximate so the chip can flag them.
    StenoSuggestions {
        buffer: String,
        candidates: Vec<StenoCandidate>,
    },
    #[allow(dead_code)]
    LlmPrompt,
    #[allow(dead_code)]
    Terminal,
    #[allow(dead_code)]
    WebPage(String),
}

/// A steno suggestion chip's data: the word plus whether it was a
/// vowel-fuzzy (approximate) match.
#[derive(Debug, Clone)]
pub struct StenoCandidate {
    pub word: String,
    pub fuzzy: bool,
}

const PAGE_STATUS: &str = "status";
const PAGE_VOICE: &str = "voice";
const PAGE_STENO: &str = "steno";
const PAGE_LLM: &str = "llm";
const PAGE_TERMINAL: &str = "terminal";
const PAGE_WEBPAGE: &str = "webpage";

pub struct MicrotermView {
    revealer: gtk4::Revealer,
    stack: gtk4::Stack,
    status_label: gtk4::Label,
    voice_words_box: gtk4::Box,
    voice_meter: gtk4::ProgressBar,
    last_voice_partial: RefCell<String>,
    llm_entry: gtk4::Entry,
    llm_submit_cb: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
    /// Callback invoked when a voice-bar word chip is single-tapped.
    /// Set by the bridge to commit just that word (parallel to the
    /// drag-onto-spacebar gesture). When unset, taps are inert.
    word_tap_cb: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
    /// Steno suggestion page: a hint label for the phoneme buffer and
    /// a horizontal box of tappable candidate-word chips.
    steno_hint: gtk4::Label,
    steno_words_box: gtk4::Box,
    /// Callback invoked when a steno suggestion chip is tapped. Set by
    /// the keyboard view to accept_word + clear the steno buffer and
    /// commit the chosen word. Separate from word_tap_cb because the
    /// commit semantics differ (steno clears its phoneme buffer).
    steno_tap_cb: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
    #[allow(dead_code)]
    terminal_placeholder: gtk4::Label,
    webpage_placeholder: gtk4::Label,
}

impl MicrotermView {
    pub fn new() -> Self {
        let revealer = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::SlideDown)
            .transition_duration(180)
            .reveal_child(false)
            .vexpand(false)
            .valign(gtk4::Align::Start)
            .css_classes(["microterm"])
            .build();

        let stack = gtk4::Stack::builder()
            .transition_type(gtk4::StackTransitionType::Crossfade)
            .transition_duration(120)
            .vhomogeneous(false)
            .hhomogeneous(false)
            .build();

        // Status page
        let status_label = gtk4::Label::builder()
            .label("")
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .css_classes(["microterm-status"])
            .build();
        // Make status tappable to dismiss — most status messages are
        // notifications the user can clear by tapping.
        {
            let revealer_for_dismiss = revealer.clone();
            let click = gtk4::GestureClick::new();
            click.connect_released(move |_g, _n, _x, _y| {
                revealer_for_dismiss.set_reveal_child(false);
            });
            status_label.add_controller(click);
        }
        stack.add_named(&status_label, Some(PAGE_STATUS));

        // Voice page
        let voice_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .build();
        let voice_meter = gtk4::ProgressBar::builder()
            .show_text(false)
            .css_classes(["microterm-voice-meter"])
            .width_request(48)
            .valign(gtk4::Align::Center)
            .build();
        let voice_words_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(4)
            .hexpand(true)
            .build();
        let voice_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .hexpand(true)
            .min_content_height(28)
            .max_content_height(36)
            .child(&voice_words_box)
            .build();
        voice_box.append(&voice_meter);
        voice_box.append(&voice_scroll);
        stack.add_named(&voice_box, Some(PAGE_VOICE));

        // Steno suggestions page: phoneme-buffer hint on the left,
        // tappable candidate-word chips scrolling horizontally on the
        // right. Mirrors the voice page's chip layout (and reuses
        // make_word_chip-style chips) so it feels consistent.
        let steno_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .valign(gtk4::Align::Center)
            .build();
        let steno_hint = gtk4::Label::builder()
            .label("")
            .css_classes(["microterm-steno-hint"])
            .build();
        let steno_words_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(4)
            .hexpand(true)
            .build();
        let steno_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .hexpand(true)
            .min_content_height(28)
            .max_content_height(36)
            .child(&steno_words_box)
            .build();
        steno_box.append(&steno_hint);
        steno_box.append(&steno_scroll);
        stack.add_named(&steno_box, Some(PAGE_STENO));

        // LLM prompt page
        let llm_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .build();
        let llm_label = gtk4::Label::builder()
            .label("LLM:")
            .css_classes(["microterm-status"])
            .build();
        let llm_entry = gtk4::Entry::builder()
            .placeholder_text("ask anything... (Up: history)")
            .hexpand(true)
            .css_classes(["microterm-llm-entry"])
            .build();
        let llm_submit_cb: Rc<RefCell<Option<Box<dyn Fn(String)>>>> =
            Rc::new(RefCell::new(None));
        {
            let cb = llm_submit_cb.clone();
            let entry_clone = llm_entry.clone();
            llm_entry.connect_activate(move |_| {
                let text = entry_clone.text().to_string();
                entry_clone.set_text("");
                if let Some(f) = cb.borrow().as_ref() {
                    f(text);
                }
            });
        }
        // Up/Down arrow → history cycling. The history is held
        // process-wide in window.rs; this handler peeks at it and
        // sets the entry text. Tracks an index so repeated Up
        // walks further back.
        {
            let history_idx: Rc<Cell<usize>> = Rc::new(Cell::new(0));
            let entry_clone = llm_entry.clone();
            let history_idx_clone = history_idx.clone();
            let keys = gtk4::EventControllerKey::new();
            keys.connect_key_pressed(move |_ctl, key, _code, _state| {
                use gtk4::gdk::Key;
                match key {
                    Key::Up => {
                        let recent = crate::ui::window::recent_llm_prompts(16);
                        if recent.is_empty() {
                            return glib::Propagation::Stop;
                        }
                        let mut i = history_idx_clone.get();
                        if i < recent.len() {
                            entry_clone.set_text(&recent[i]);
                            entry_clone.set_position(-1);
                            i = (i + 1).min(recent.len() - 1);
                            history_idx_clone.set(i);
                        }
                        glib::Propagation::Stop
                    }
                    Key::Down => {
                        let mut i = history_idx_clone.get();
                        if i == 0 {
                            entry_clone.set_text("");
                        } else {
                            i -= 1;
                            let recent = crate::ui::window::recent_llm_prompts(16);
                            if i < recent.len() {
                                entry_clone.set_text(&recent[i]);
                                entry_clone.set_position(-1);
                            }
                            history_idx_clone.set(i);
                        }
                        glib::Propagation::Stop
                    }
                    _ => {
                        // Any non-arrow key resets the history
                        // cursor so the next Up starts at the
                        // top again.
                        history_idx_clone.set(0);
                        glib::Propagation::Proceed
                    }
                }
            });
            llm_entry.add_controller(keys);
        }
        // DropTarget on the entry: a voice-chip dropped here
        // appends the word to the current entry text. Lets the
        // user assemble a prompt by dragging from the voice bar
        // into the LLM entry, useful when whisper picks up the
        // right keywords but the user wants to surround them with
        // their own instruction.
        {
            let drop = gtk4::DropTarget::new(
                glib::types::Type::STRING,
                gtk4::gdk::DragAction::COPY,
            );
            let entry_for_drop = llm_entry.clone();
            drop.connect_drop(move |_t, value, _x, _y| {
                if let Ok(word) = value.get::<String>() {
                    let current = entry_for_drop.text().to_string();
                    let separator = if current.is_empty()
                        || current.ends_with(char::is_whitespace)
                    {
                        ""
                    } else {
                        " "
                    };
                    entry_for_drop.set_text(&format!(
                        "{}{}{}",
                        current, separator, word
                    ));
                    true
                } else {
                    false
                }
            });
            llm_entry.add_controller(drop);
        }
        llm_box.append(&llm_label);
        llm_box.append(&llm_entry);
        stack.add_named(&llm_box, Some(PAGE_LLM));

        // Terminal placeholder
        let terminal_placeholder = gtk4::Label::builder()
            .label("[terminal: pending vte4 wiring]")
            .xalign(0.0)
            .css_classes(["microterm-placeholder"])
            .build();
        stack.add_named(&terminal_placeholder, Some(PAGE_TERMINAL));

        // Webpage placeholder
        let webpage_placeholder = gtk4::Label::builder()
            .label("[webpage: pending webkit wiring]")
            .xalign(0.0)
            .css_classes(["microterm-placeholder"])
            .build();
        stack.add_named(&webpage_placeholder, Some(PAGE_WEBPAGE));

        // Wrap stack + ✕ in a horizontal Box. The ✕ sits at the
        // far right of the microterm and dismisses whatever's
        // currently in-flight: voice recording gets cancelled,
        // LLM request gets abandoned (worker still finishes
        // silently), entry gets cleared.
        let cancel_btn = gtk4::Button::builder()
            .label("✕")
            .css_classes(["microterm-cancel"])
            .valign(gtk4::Align::Center)
            .halign(gtk4::Align::End)
            .build();
        let revealer_for_cancel = revealer.clone();
        cancel_btn.connect_clicked(move |_| {
            crate::ui::window::request_microterm_cancel();
            revealer_for_cancel.set_reveal_child(false);
        });

        let row_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(4)
            .build();
        stack.set_hexpand(true);
        row_box.append(&stack);
        row_box.append(&cancel_btn);

        revealer.set_child(Some(&row_box));

        Self {
            revealer,
            stack,
            status_label,
            voice_words_box,
            voice_meter,
            last_voice_partial: RefCell::new(String::new()),
            llm_entry,
            llm_submit_cb,
            word_tap_cb: Rc::new(RefCell::new(None)),
            steno_hint,
            steno_words_box,
            steno_tap_cb: Rc::new(RefCell::new(None)),
            terminal_placeholder,
            webpage_placeholder,
        }
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.revealer.upcast_ref()
    }

    pub fn show(&self) {
        if !self.revealer.reveals_child() {
            self.revealer.set_reveal_child(true);
        }
    }

    pub fn hide(&self) {
        if self.revealer.reveals_child() {
            self.revealer.set_reveal_child(false);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }

    /// Show the LLM prompt page and register a submit callback.
    /// The callback fires when the user presses Enter; it receives
    /// the entered text. The microterm clears the entry after.
    pub fn show_llm_prompt(&self, callback: Box<dyn Fn(String)>) {
        *self.llm_submit_cb.borrow_mut() = Some(callback);
        self.stack.set_visible_child_name(PAGE_LLM);
        self.show();
        self.llm_entry.grab_focus();
    }

    pub fn set_content(&self, content: MicrotermContent) {
        match content {
            MicrotermContent::Status(text) => {
                self.status_label.set_text(&text);
                self.stack.set_visible_child_name(PAGE_STATUS);
            }
            MicrotermContent::VoicePreview { partial, level } => {
                let clamped = level.clamp(0.0, 1.0) as f64;
                self.voice_meter.set_fraction(clamped);
                let mut last = self.last_voice_partial.borrow_mut();
                if *last != partial {
                    *last = partial.clone();
                    self.rebuild_voice_words(&partial);
                }
                self.stack.set_visible_child_name(PAGE_VOICE);
            }
            MicrotermContent::StenoSuggestions { buffer, candidates } => {
                // Hint shows the phoneme run so far; chips are the
                // tappable expansions.
                if buffer.is_empty() {
                    self.steno_hint.set_text("");
                } else {
                    self.steno_hint.set_text(&format!("[{}]", buffer));
                }
                self.rebuild_steno_words(&candidates);
                self.stack.set_visible_child_name(PAGE_STENO);
            }
            MicrotermContent::LlmPrompt => {
                self.stack.set_visible_child_name(PAGE_LLM);
                self.llm_entry.grab_focus();
            }
            MicrotermContent::Terminal => {
                self.stack.set_visible_child_name(PAGE_TERMINAL);
                debug!("microterm: Terminal (placeholder)");
            }
            MicrotermContent::WebPage(url) => {
                self.webpage_placeholder
                    .set_label(&format!("[webpage: {}]", url));
                self.stack.set_visible_child_name(PAGE_WEBPAGE);
                debug!("microterm: WebPage {} (placeholder)", url);
            }
        }
        self.show();
    }

    fn rebuild_voice_words(&self, text: &str) {
        while let Some(child) = self.voice_words_box.first_child() {
            self.voice_words_box.remove(&child);
        }
        if text.is_empty() {
            let hint = gtk4::Label::builder()
                .label("Listening...")
                .css_classes(["microterm-voice-partial"])
                .xalign(0.0)
                .build();
            self.voice_words_box.append(&hint);
            return;
        }
        for word in text.split_whitespace() {
            let chip = self.make_word_chip(word);
            self.voice_words_box.append(&chip);
        }
    }

    /// Register a callback for taps on voice-bar word chips.
    /// Called by the bridge during startup so taps commit the
    /// tapped word (parallel to the drag-to-spacebar gesture).
    pub fn set_word_tap_callback(&self, cb: Box<dyn Fn(String)>) {
        *self.word_tap_cb.borrow_mut() = Some(cb);
    }

    /// Register a callback for taps on steno suggestion chips. The
    /// keyboard view sets this to accept_word (commit + clear the
    /// steno phoneme buffer). Separate from the voice callback
    /// because the commit semantics differ.
    pub fn set_steno_tap_callback(&self, cb: Box<dyn Fn(String)>) {
        *self.steno_tap_cb.borrow_mut() = Some(cb);
    }

    /// Rebuild the steno candidate chips from a fresh candidate list.
    fn rebuild_steno_words(&self, candidates: &[StenoCandidate]) {
        while let Some(child) = self.steno_words_box.first_child() {
            self.steno_words_box.remove(&child);
        }
        for cand in candidates {
            self.steno_words_box.append(&self.make_steno_chip(cand));
        }
    }

    /// Build a single tappable steno-candidate chip. Tapping fires
    /// the steno tap callback (accept_word). Fuzzy (vowel-approximate)
    /// candidates get a marker class so they read as "best guess".
    fn make_steno_chip(&self, cand: &StenoCandidate) -> gtk4::Widget {
        let label = gtk4::Label::builder().label(&cand.word).build();
        // Apply classes after build to avoid builder-signature
        // friction with a dynamically-sized class list.
        label.add_css_class("microterm-voice-word");
        if cand.fuzzy {
            label.add_css_class("microterm-steno-fuzzy");
        }
        let chip = gtk4::Frame::builder()
            .child(&label)
            .css_classes(["microterm-voice-chip"])
            .build();

        let click = gtk4::GestureClick::new();
        let word = cand.word.clone();
        let cb = self.steno_tap_cb.clone();
        click.connect_released(move |_g, _n, _x, _y| {
            if let Some(f) = cb.borrow().as_ref() {
                f(word.clone());
            }
        });
        chip.add_controller(click);
        chip.upcast()
    }

    /// Build a single draggable + tappable word chip.
    fn make_word_chip(&self, word: &str) -> gtk4::Widget {
        let label = gtk4::Label::builder()
            .label(word)
            .css_classes(["microterm-voice-word"])
            .build();
        let chip = gtk4::Frame::builder()
            .child(&label)
            .css_classes(["microterm-voice-chip"])
            .build();

        // Drag source: word can be dragged onto space (commit) or
        // backspace (undo).
        let drag_source = gtk4::DragSource::builder()
            .actions(gtk4::gdk::DragAction::COPY)
            .build();
        let word_owned = word.to_string();
        drag_source.connect_prepare(move |_src, _x, _y| {
            let v = word_owned.to_value();
            Some(gtk4::gdk::ContentProvider::for_value(&v))
        });
        // Show a thumbnail of the chip itself as the drag icon so
        // the user sees what they're moving. WidgetPaintable
        // renders any widget as a Paintable; we attach a paintable
        // of the chip at drag start. Without this, the drag is
        // invisible (just a finger moving).
        {
            let chip_clone = chip.clone();
            drag_source.connect_drag_begin(move |src, _drag| {
                let paintable = gtk4::WidgetPaintable::new(Some(&chip_clone));
                src.set_icon(Some(&paintable), 0, 0);
            });
        }
        chip.add_controller(drag_source);

        // Click gesture: tap commits the word via the callback
        // registered by the bridge.
        let click = gtk4::GestureClick::new();
        let word_for_click = word.to_string();
        let cb = self.word_tap_cb.clone();
        click.connect_released(move |_g, _n, _x, _y| {
            if let Some(f) = cb.borrow().as_ref() {
                f(word_for_click.clone());
            }
        });
        chip.add_controller(click);

        chip.upcast()
    }
}

impl Default for MicrotermView {
    fn default() -> Self {
        Self::new()
    }
}
