//! a11y — shared on-screen affordance metadata: voice, and everything else
//! that needs to know what a widget IS.
//!
//! The bet (see BIGSILICON TODO): because `voiced` already routes intents system-wide and
//! uikit-egui is the shared UI layer, accessibility voice belongs *in the kit* — an app
//! gets voice focus-navigation, activation, and spoken descriptions by (1) tagging its
//! widgets with [`Semantics`] and (2) registering [`standard_grammar`] with voiced once.
//! On by default, fully overridable.
//!
//! ## This is not only for voice any more
//!
//! [`glove`](crate::glove) needed the same thing — a labelled, kinded, positioned
//! interactive thing on screen — and built its own [`Target`](crate::glove::Target)
//! independently, because nothing here offered a conversion. Every app that wants both
//! voice-a11y and cold-weather glove mode had to register each widget twice, by hand, into
//! two separate lists that could silently drift (a button present in one and not the
//! other). [`Item::as_glove_target`] closes that: register once with [`A11yScene::add`],
//! feed the same list to both consumers.
//!
//! The same registration is the right foundation for smart tab order (widgets are already
//! collected in on-screen call order), AI-driven UI description (`describe_screen`
//! already produces one), smart command-palette filtering ([`Semantics::command_id`] gives
//! a stable machine name distinct from a possibly-icon-only spoken label), and eye-tracking
//! gaze targets (the same `rect` list [`A11yScene`] already carries). None of those
//! consumers exist yet — this paragraph is the seam they attach to, not a claim they are
//! built. Only two apps register anything here today (`gitmind`, `bo-suite`); making this
//! the widget's problem rather than each call site's is `semantic_button`/
//! `semantic_toggle`/`semantic_text_edit` below, and is the fastest path to changing that
//! number.
//!
//! This module is the app-agnostic brain: an immediate-mode [`A11yScene`] collects the
//! focusable widgets each frame, voice [`Action`]s move focus / activate / describe, and
//! the descriptions are plain strings a TTS stage speaks. The only egui touch is painting
//! the focus ring.

use egui::{Color32, Rect, CornerRadius, Stroke, Ui};
use crate::glove::{Target, TargetKind};

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
    /// A stable, machine-readable name — "settings.toggle_dark_mode", not
    /// "🌙" — for a command palette or an AI integration to match against.
    /// `label` is what gets SPOKEN or displayed and can be an icon, an
    /// abbreviation, or something translated; `command_id` is the thing
    /// that does not change when either of those does. `None` for a
    /// widget nobody will ever want to address by name (most labels and
    /// headings): a required field here would make every existing call
    /// site invent a name for something that does not need one.
    pub command_id: Option<String>,
}

impl Semantics {
    pub fn new(role: Role, label: &str) -> Semantics {
        Semantics { role, label: label.to_string(), value: None, hint: None, command_id: None }
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
    /// Set the spoken state on any role.
    ///
    /// `toggle` and `field` set `value` in their constructors, which covers a
    /// checkbox and a text box. It does not cover a BUTTON whose state is not
    /// its label: bo-suite's ANC chip cycles off/auto/on and the thing a
    /// listener needs — whether anti-noise is currently leaving the speakers —
    /// is neither the label nor a boolean. Without this, such a control can be
    /// announced or accurate, not both.
    pub fn with_value(mut self, value: &str) -> Semantics {
        self.value = Some(value.to_string());
        self
    }

    pub fn with_hint(mut self, hint: &str) -> Semantics {
        self.hint = Some(hint.to_string());
        self
    }

    /// Attach the stable machine name a palette or an AI integration
    /// matches against. See the field doc for why this is separate from
    /// `label`.
    pub fn with_command_id(mut self, id: &str) -> Semantics {
        self.command_id = Some(id.to_string());
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

impl Item {
    /// Bridge to [`crate::glove`]'s independent snap-target model, so a widget registered
    /// once here can also feed glove/cold-weather mode without a second hand-written list.
    ///
    /// `None` when either side cannot represent the other: no `rect` (glove needs a screen
    /// point to snap to, and one was never laid out — this item is voice-only, e.g. it was
    /// registered before paint), or a [`Role`] glove has no vocabulary for (`Label`,
    /// `Heading`, `Image`, `Other` are not things a cursor "does" anything to — see
    /// `glove::Vocab`, which only ever acts on Button/Field/ListItem/Slider/Toggle).
    /// `Link` maps to `Button`: glove has no link-specific vocabulary, and "activate" is the
    /// correct action for both.
    pub fn as_glove_target(&self) -> Option<Target> {
        let kind = match self.semantics.role {
            Role::Button | Role::Link => TargetKind::Button,
            Role::TextField => TargetKind::Field,
            Role::Toggle => TargetKind::Toggle,
            Role::Slider => TargetKind::Slider,
            Role::Label | Role::Heading | Role::Image | Role::Other => return None,
        };
        let rect = self.rect?;
        Some(Target {
            id: self.id.to_string(),
            kind,
            at: (rect.center().x, rect.center().y),
            label: self.semantics.label.clone(),
        })
    }
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

    /// Every registered item this frame that glove mode can snap to, in the
    /// same call order they were registered — see [`Item::as_glove_target`]
    /// for which roles that excludes. The single call a host needs to feed
    /// cold-weather mode from the same registration voice already uses.
    pub fn glove_targets(&self) -> Vec<Target> {
        self.items.iter().filter_map(Item::as_glove_target).collect()
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
                ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(4), Stroke::new(2.0_f32, accent), egui::StrokeKind::Inside);
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

/// Render an egui single-line text field and auto-register it. `label` is what gets
/// spoken/searched ("Name", not the placeholder text) — pass a real label even for a field
/// whose on-screen hint text serves as its only visual label, since `hint_text` is not
/// necessarily `label` and this function has no way to infer one from the other.
/// Returns true when the text changed this frame.
pub fn semantic_text_edit(ui: &mut Ui, scene: &mut A11yScene, id: u64, label: &str, text: &mut String) -> bool {
    let resp = ui.text_edit_singleline(text);
    scene.add(id, Semantics::field(label, text), Some(resp.rect));
    resp.changed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_id_is_separate_from_the_spoken_label() {
        let s = Semantics::button("🌙").with_command_id("settings.toggle_dark_mode");
        // describe() is what gets SPOKEN — command_id must never leak into it,
        // or a screen reader would announce a machine name.
        assert_eq!(s.describe(), "🌙, button");
        assert_eq!(s.command_id.as_deref(), Some("settings.toggle_dark_mode"));
        // and a widget that never sets one stays None, not empty-string —
        // "nobody named this" and "named the empty string" are different facts.
        assert_eq!(Semantics::button("Play").command_id, None);
    }

    #[test]
    fn glove_target_bridge_covers_every_interactive_role_and_excludes_the_rest() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(40.0, 12.0));
        let cases: &[(Role, crate::glove::TargetKind)] = &[
            (Role::Button, crate::glove::TargetKind::Button),
            (Role::Link, crate::glove::TargetKind::Button),
            (Role::TextField, crate::glove::TargetKind::Field),
            (Role::Toggle, crate::glove::TargetKind::Toggle),
            (Role::Slider, crate::glove::TargetKind::Slider),
        ];
        for (role, want_kind) in cases {
            let item = Item { id: 7, semantics: Semantics::new(*role, "Widget"), rect: Some(rect) };
            let t = item.as_glove_target().unwrap_or_else(|| panic!("{role:?} should bridge"));
            assert_eq!(t.kind, *want_kind, "{role:?}");
            assert_eq!(t.id, "7");
            assert_eq!(t.label, "Widget");
            // glove wants a screen-space centre point, not the whole rect.
            assert_eq!(t.at, (rect.center().x, rect.center().y));
        }
        // Non-interactive roles have no glove vocabulary at all (see glove::Vocab) —
        // bridging them would hand the cursor a target it can never act on.
        for role in [Role::Label, Role::Heading, Role::Image, Role::Other] {
            let item = Item { id: 1, semantics: Semantics::new(role, "x"), rect: Some(rect) };
            assert!(item.as_glove_target().is_none(), "{role:?} should NOT bridge");
        }
        // No rect (voice-only registration before layout) — nothing to snap to.
        let no_rect = Item { id: 2, semantics: Semantics::button("Save"), rect: None };
        assert!(no_rect.as_glove_target().is_none());
    }

    #[test]
    fn scene_collects_glove_targets_in_registration_order_skipping_non_interactive() {
        let mut s = A11yScene::new();
        s.begin_frame();
        let r = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0));
        s.add(1, Semantics::heading("Settings"), Some(r)); // excluded
        s.add(2, Semantics::button("Save"), Some(r));
        s.add(3, Semantics::toggle("Loop", true), Some(r));
        let targets = s.glove_targets();
        assert_eq!(targets.len(), 2, "the heading must not become a snap target");
        assert_eq!(targets[0].id, "2");
        assert_eq!(targets[1].id, "3");
    }

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
