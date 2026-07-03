//! a11y — accessibility voice, shared across every uikit-egui app.
//!
//! The bet (see BIGSILICON TODO): because `voiced` already routes intents system-wide and
//! uikit-egui is the shared UI layer, accessibility voice belongs *in the kit* — an app
//! gets voice focus-navigation, activation, and spoken descriptions by (1) tagging its
//! widgets with [`Semantics`] and (2) registering [`standard_grammar`] with voiced once.
//! On by default, fully overridable.
//!
//! This module is the app-agnostic brain: an immediate-mode [`A11yScene`] collects the
//! focusable widgets each frame, voice [`Action`]s move focus / activate / describe, and
//! the descriptions are plain strings a TTS stage speaks. The only egui touch is painting
//! the focus ring.

use egui::{Color32, Rect, Rounding, Stroke, Ui};

/// The kind of a widget, for spoken descriptions ("Play, button").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Button,
    Label,
    Heading,
    TextField,
    Toggle,
    Slider,
    Link,
    Image,
    Other,
}

impl Role {
    fn spoken(self) -> &'static str {
        match self {
            Role::Button => "button",
            Role::Label => "label",
            Role::Heading => "heading",
            Role::TextField => "text field",
            Role::Toggle => "toggle",
            Role::Slider => "slider",
            Role::Link => "link",
            Role::Image => "image",
            Role::Other => "control",
        }
    }
}

/// Accessibility metadata attached to a widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Semantics {
    pub role: Role,
    pub label: String,
    /// Current value/state (e.g. "on", the field's text) for spoken feedback.
    pub value: Option<String>,
    /// A short usage hint.
    pub hint: Option<String>,
}

impl Semantics {
    pub fn new(role: Role, label: &str) -> Semantics {
        Semantics { role, label: label.to_string(), value: None, hint: None }
    }
    pub fn button(label: &str) -> Semantics {
        Semantics::new(Role::Button, label)
    }
    pub fn heading(label: &str) -> Semantics {
        Semantics::new(Role::Heading, label)
    }
    pub fn toggle(label: &str, on: bool) -> Semantics {
        Semantics { value: Some(if on { "on".into() } else { "off".into() }), ..Semantics::new(Role::Toggle, label) }
    }
    pub fn field(label: &str, text: &str) -> Semantics {
        let v = if text.is_empty() { "empty".to_string() } else { text.to_string() };
        Semantics { value: Some(v), ..Semantics::new(Role::TextField, label) }
    }
    pub fn with_hint(mut self, hint: &str) -> Semantics {
        self.hint = Some(hint.to_string());
        self
    }

    /// A spoken description: "Mute, toggle, on" / "Name, text field, empty".
    pub fn describe(&self) -> String {
        let mut s = format!("{}, {}", self.label, self.role.spoken());
        if let Some(v) = &self.value {
            s.push_str(", ");
            s.push_str(v);
        }
        s
    }
}

/// A voice-driven accessibility action, decoded from a voiced intent event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Activate,
    Next,
    Prev,
    First,
    Last,
    DescribeFocused,
    DescribeScreen,
    ScrollUp,
    ScrollDown,
    Back,
}

impl Action {
    /// Map a voiced event type (see [`standard_grammar`]) to an action.
    pub fn from_event(event_type: &str) -> Option<Action> {
        Some(match event_type {
            "a11y.activate" => Action::Activate,
            "a11y.next" => Action::Next,
            "a11y.prev" => Action::Prev,
            "a11y.first" => Action::First,
            "a11y.last" => Action::Last,
            "a11y.describe" => Action::DescribeFocused,
            "a11y.read_screen" => Action::DescribeScreen,
            "a11y.scroll_down" => Action::ScrollDown,
            "a11y.scroll_up" => Action::ScrollUp,
            "a11y.back" => Action::Back,
            _ => return None,
        })
    }
}

/// The standard accessibility grammar every uikit-egui app registers with voiced. Pairs of
/// (event_type, spoken phrase). Register at `Always` scope so voice control works even when
/// the app isn't foreground; the app overrides or extends this as needed.
pub fn standard_grammar() -> &'static [(&'static str, &'static str)] {
    &[
        ("a11y.activate", "activate"),
        ("a11y.activate", "click"),
        ("a11y.activate", "select"),
        ("a11y.next", "next"),
        ("a11y.prev", "previous"),
        ("a11y.first", "first"),
        ("a11y.last", "last"),
        ("a11y.describe", "what is this"),
        ("a11y.read_screen", "read screen"),
        ("a11y.scroll_down", "scroll down"),
        ("a11y.scroll_up", "scroll up"),
        ("a11y.back", "go back"),
    ]
}

/// One focusable widget registered this frame.
#[derive(Debug, Clone)]
pub struct Item {
    /// Stable id the app assigns (e.g. a hash of the widget), used to route Activate back.
    pub id: u64,
    pub semantics: Semantics,
    pub rect: Option<Rect>,
}

/// What the caller should do after applying an [`Action`].
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Activate the widget with this id (the app performs the click/toggle).
    Activate(u64),
    /// Speak this text (hand to the TTS stage).
    Speak(String),
    /// Scroll the view by this many logical points (sign = direction).
    Scroll(f32),
    /// Navigate back.
    Back,
    /// Focus moved but nothing else to do.
    Moved,
    None,
}

/// Immediate-mode accessibility scene: rebuilt each frame, remembers focus across frames.
#[derive(Default)]
pub struct A11yScene {
    items: Vec<Item>,
    focus_id: Option<u64>,
    pub enabled: bool,
}

impl A11yScene {
    pub fn new() -> Self {
        A11yScene { enabled: true, ..Default::default() }
    }

    /// Start a frame: clears the collected items (focus id is remembered).
    pub fn begin_frame(&mut self) {
        self.items.clear();
    }

    /// Register a focusable widget this frame; returns whether it is currently focused (so
    /// the app can style it). Call once per focusable widget after laying it out.
    pub fn add(&mut self, id: u64, semantics: Semantics, rect: Option<Rect>) -> bool {
        let focused = self.focus_id == Some(id);
        self.items.push(Item { id, semantics, rect });
        focused
    }

    fn focus_index(&self) -> Option<usize> {
        self.focus_id.and_then(|id| self.items.iter().position(|it| it.id == id))
    }

    pub fn focused(&self) -> Option<&Item> {
        self.focus_index().map(|i| &self.items[i])
    }

    fn move_focus(&mut self, to: Option<usize>) -> Outcome {
        match to.and_then(|i| self.items.get(i)) {
            Some(it) => {
                self.focus_id = Some(it.id);
                Outcome::Speak(it.semantics.describe())
            }
            None => Outcome::None,
        }
    }

    /// Apply a voice action against the current frame's items.
    pub fn apply(&mut self, action: Action) -> Outcome {
        if self.items.is_empty() {
            return Outcome::None;
        }
        let cur = self.focus_index();
        match action {
            Action::Next => {
                let i = cur.map(|c| (c + 1).min(self.items.len() - 1)).unwrap_or(0);
                self.move_focus(Some(i))
            }
            Action::Prev => {
                let i = cur.map(|c| c.saturating_sub(1)).unwrap_or(0);
                self.move_focus(Some(i))
            }
            Action::First => self.move_focus(Some(0)),
            Action::Last => self.move_focus(Some(self.items.len() - 1)),
            Action::Activate => match self.focused() {
                Some(it) => Outcome::Activate(it.id),
                None => Outcome::None,
            },
            Action::DescribeFocused => match self.focused() {
                Some(it) => Outcome::Speak(it.semantics.describe()),
                None => Outcome::Speak("nothing focused".into()),
            },
            Action::DescribeScreen => Outcome::Speak(self.describe_screen()),
            Action::ScrollDown => Outcome::Scroll(200.0),
            Action::ScrollUp => Outcome::Scroll(-200.0),
            Action::Back => Outcome::Back,
        }
    }

    /// A spoken overview of the screen.
    pub fn describe_screen(&self) -> String {
        if self.items.is_empty() {
            return "screen has no controls".into();
        }
        let n = self.items.len();
        let heading = self.items.iter().find(|it| it.semantics.role == Role::Heading);
        let lead = match heading {
            Some(h) => format!("{}. ", h.semantics.label),
            None => String::new(),
        };
        format!("{lead}{n} control{}: {}", if n == 1 { "" } else { "s" },
            self.items.iter().take(6).map(|it| it.semantics.label.clone()).collect::<Vec<_>>().join(", "))
    }

    /// Paint a focus ring around the focused widget (the one egui touch).
    pub fn paint_focus_ring(&self, ui: &Ui, accent: Color32) {
        if let Some(it) = self.focused() {
            if let Some(rect) = it.rect {
                ui.painter().rect_stroke(rect.expand(2.0), Rounding::same(4.0), Stroke::new(2.0, accent));
            }
        }
    }
}

/// Render an egui button and auto-register its accessibility semantics + rect into `scene`.
/// This is the "automatic" path: apps that use the kit's widgets get voice/description
/// support for free, without hand-tagging. Returns true on click.
pub fn semantic_button(ui: &mut Ui, scene: &mut A11yScene, id: u64, label: &str) -> bool {
    let resp = ui.button(label);
    scene.add(id, Semantics::button(label), Some(resp.rect));
    resp.clicked()
}

/// Same, for a toggle (checkbox): reflects on/off in the spoken value.
pub fn semantic_toggle(ui: &mut Ui, scene: &mut A11yScene, id: u64, label: &str, on: &mut bool) -> bool {
    let resp = ui.checkbox(on, label);
    scene.add(id, Semantics::toggle(label, *on), Some(resp.rect));
    resp.changed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_reads_role_and_state() {
        assert_eq!(Semantics::button("Play").describe(), "Play, button");
        assert_eq!(Semantics::toggle("Mute", true).describe(), "Mute, toggle, on");
        assert_eq!(Semantics::field("Name", "").describe(), "Name, text field, empty");
    }

    #[test]
    fn grammar_maps_to_actions() {
        for (event, _phrase) in standard_grammar() {
            assert!(Action::from_event(event).is_some(), "grammar event {event} has an action");
        }
        assert_eq!(Action::from_event("a11y.activate"), Some(Action::Activate));
        assert_eq!(Action::from_event("nope"), None);
    }

    #[test]
    fn voice_focus_navigation_and_activation() {
        let mut s = A11yScene::new();
        s.begin_frame();
        s.add(1, Semantics::button("Play"), None);
        s.add(2, Semantics::button("Stop"), None);
        s.add(3, Semantics::toggle("Loop", false), None);

        // next → focus Play and speak it
        assert_eq!(s.apply(Action::Next), Outcome::Speak("Play, button".into()));
        // next → Stop
        assert_eq!(s.apply(Action::Next), Outcome::Speak("Stop, button".into()));
        // activate → routes the focused id back to the app
        assert_eq!(s.apply(Action::Activate), Outcome::Activate(2));
        // last → Loop
        assert_eq!(s.apply(Action::Last), Outcome::Speak("Loop, toggle, off".into()));
        // prev never underflows
        s.apply(Action::First);
        assert_eq!(s.apply(Action::Prev), Outcome::Speak("Play, button".into()));
    }

    #[test]
    fn focus_persists_across_frames_by_id() {
        let mut s = A11yScene::new();
        s.begin_frame();
        s.add(10, Semantics::button("A"), None);
        s.add(20, Semantics::button("B"), None);
        s.apply(Action::Last); // focus B (id 20)
        // next frame, same ids re-added
        s.begin_frame();
        s.add(10, Semantics::button("A"), None);
        s.add(20, Semantics::button("B"), None);
        assert_eq!(s.focused().map(|it| it.id), Some(20), "focus follows the id, not the index");
    }

    #[test]
    fn describe_screen_leads_with_heading() {
        let mut s = A11yScene::new();
        s.begin_frame();
        s.add(1, Semantics::heading("Settings"), None);
        s.add(2, Semantics::button("Save"), None);
        assert!(s.describe_screen().starts_with("Settings. 2 controls: "));
    }
}
