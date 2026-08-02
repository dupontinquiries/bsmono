//! friends — `FriendListWidget` and `ContactCard`.
//!
//! The Friends page as the Qt app actually has it: a roster whose rows are
//! expandable cards with per-contact actions, an empty state that tells you why
//! it is empty, and the manual add-peer form that is the only way in when
//! discovery does not work. The egui port had a flat list of names and no way
//! to add anyone, which is the half of the page that matters when you are
//! standing next to someone trying to connect.
//!
//! ## Three address formats, because discovery fails
//!
//! `onAddFriendClicked` accepts a DHT ID (`sc-…`, port 0 meaning "look it up"),
//! a bare host with the port from the spinner, or `host:port` inline. The port
//! field defaults to 9876. Rejecting a pasted `192.168.1.5:9876` because it has
//! a colon in it would be maddening at exactly the moment the user is already
//! frustrated.
//!
//! ## Two Qt defects reproduced as tests rather than as behaviour
//!
//! **The label drift.** `markOnline` rebuilds a row by taking `text().mid(2)`
//! to strip the status dot — but the label is built with `"%1  %2"`, a dot and
//! *two* spaces. `mid(2)` removes the dot and one space, so every online↔offline
//! transition leaves one more leading space and the name creeps rightward. A
//! peer that flaps a dozen times ends up visibly indented. The port stores the
//! name as a field instead of parsing it back out of a display string, which is
//! the actual fix; `qt_label_drift` documents what it is fixing.
//!
//! **The stale empty state.** `setFriends` ends with `updateEmptyState()`;
//! `setContacts` does not, and its loop only calls it via `addOrUpdateContact`.
//! So clearing the contact list leaves the empty label hidden and an empty list
//! widget visible — a blank page with no explanation, which reads as a crash.
//! Here emptiness is derived from the rows rather than tracked alongside them,
//! so the two cannot disagree.
//!
//! ## The card's gesture contract is the fiddly part
//!
//! A `ContactCard` header sits inside a vertical scroller which itself sits
//! inside a horizontal page-swipe nav, so a drag on it is three-ways ambiguous.
//! `eventFilter` resolves it in stages, and the stages are the contract:
//!
//! * A press never consumes, so buttons inside the header still work.
//! * While collapsed, any travel past the tap slop cancels tracking — the
//!   release must not toggle, because the user was scrolling.
//! * While expanded, vertical dominance bows out and lets the scroller have it.
//! * While expanded, horizontal travel past 48px AND clearly x-dominant
//!   (`ax > ay * 1.5`) collapses the card and *consumes* the event, so the
//!   same drag is not also read as a page swipe. Without the consume you close
//!   a card and change page in one gesture.

use uikit_egui::motion::ease;

/// `kTapSlopPx` — travel below this is a tap, not a drag.
pub const TAP_SLOP: f32 = 10.0;
/// `kSwipeCloseThresholdPx` — horizontal travel that closes an expanded card.
pub const SWIPE_CLOSE: f32 = 48.0;
/// How much x must dominate y before a drag counts as horizontal.
pub const X_DOMINANCE: f32 = 1.5;
/// `m_anim->setDuration(180)`, OutCubic.
pub const SUBMENU_MS: f32 = 180.0;
/// The default in the port field.
pub const DEFAULT_PORT: u16 = 9876;

// ── the roster ──────────────────────────────────────────────────────────────

/// One row. Name and status are separate fields, not a rendered string that
/// gets re-parsed — see the label-drift note above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    pub id: String,
    /// `Contact::label()`: the nickname if set, else the advertised name, else
    /// the id.
    pub name: String,
    pub online: bool,
}

impl Friend {
    /// The status glyph. Filled when online, hollow when not — a shape as well
    /// as a colour, because the colour alone is invisible to a good fraction of
    /// users and to anyone in sunlight.
    pub fn dot(&self) -> &'static str { if self.online { "●" } else { "○" } }

    /// What `item->setToolTip` shows: the full routable id, which is otherwise
    /// hidden behind a friendly name you cannot connect to.
    pub fn tooltip(&self) -> String {
        if self.name == self.id { self.id.clone() } else { format!("{}\n{}", self.id, self.name) }
    }
}

/// The friend list. `addOrUpdatePeer` semantics: keyed by id, updated in place.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    rows: Vec<Friend>,
}

impl Roster {
    pub fn rows(&self) -> &[Friend] { &self.rows }
    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }

    /// Insert or update, never duplicate. The same peer arrives repeatedly —
    /// from discovery, from a manual add, from a reconnect — and appending each
    /// time gives you a list of the same person over and over.
    pub fn add_or_update(&mut self, f: Friend) {
        if let Some(existing) = self.rows.iter_mut().find(|r| r.id == f.id) {
            *existing = f;
        } else {
            self.rows.push(f);
        }
    }

    /// `setFriends` / `setContacts`: replace wholesale.
    pub fn set(&mut self, friends: Vec<Friend>) {
        self.rows.clear();
        for f in friends { self.add_or_update(f); }
    }

    /// `markOnline`. Touches the flag and nothing else, so no display string is
    /// parsed and nothing drifts.
    pub fn mark_online(&mut self, id: &str, online: bool) -> bool {
        match self.rows.iter_mut().find(|r| r.id == id) {
            Some(r) => { r.online = online; true }
            None => false,
        }
    }

    pub fn get(&self, id: &str) -> Option<&Friend> { self.rows.iter().find(|r| r.id == id) }
    pub fn online_count(&self) -> usize { self.rows.iter().filter(|r| r.online).count() }
}

/// The empty state, verbatim in substance from the Qt label: it names the
/// ports, because "no peers found" without them leaves the user with nothing to
/// try.
pub const EMPTY_TEXT: &str = "No peers found yet.\n\n\
Make sure both devices are on the same network\n\
with ports 9876/tcp and 9877/udp open.\n\n\
Or add a peer manually below.";

// ── the add-peer form ───────────────────────────────────────────────────────

/// A resolved add-peer request. Port 0 is not a placeholder — it is the signal
/// to `AppController` that this is a DHT id needing a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPeer {
    pub host: String,
    pub port: u16,
}

impl AddPeer {
    pub fn is_dht_lookup(&self) -> bool { self.port == 0 }
}

/// Why an address was not accepted. The Qt code silently `return`s on each of
/// these; naming them lets the port say something instead of appearing to
/// ignore the button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrError {
    Empty,
    /// `host:port` with nothing before the colon.
    NoHost,
    /// A port field or suffix that is not a usable port.
    BadPort,
}

/// Parse what the user typed, with the port spinner as the fallback.
///
/// The three accepted shapes are the Qt ones. Whitespace is trimmed first:
/// an address pasted from a chat message arrives with a trailing newline more
/// often than not.
pub fn parse_address(input: &str, port_field: u16) -> Result<AddPeer, AddrError> {
    let input = input.trim();
    if input.is_empty() { return Err(AddrError::Empty); }

    if input.starts_with("sc-") {
        // Port 0 tells the controller to resolve this through the DHT.
        return Ok(AddPeer { host: input.to_string(), port: 0 });
    }
    if let Some((host, port)) = input.rsplit_once(':') {
        // rsplit, not split: an IPv6 literal is mostly colons, and taking the
        // FIRST one leaves you trying to connect to "fe80".
        let host = host.trim();
        if host.is_empty() { return Err(AddrError::NoHost); }
        let port: u16 = port.trim().parse().map_err(|_| AddrError::BadPort)?;
        if port == 0 { return Err(AddrError::BadPort); }
        return Ok(AddPeer { host: host.to_string(), port });
    }
    if port_field == 0 { return Err(AddrError::BadPort); }
    Ok(AddPeer { host: input.to_string(), port: port_field })
}

/// The add-peer form's own state.
#[derive(Debug, Clone)]
pub struct AddForm {
    pub host: String,
    pub port: String,
    /// The last rejection, so the form can explain itself.
    pub error: Option<AddrError>,
}

impl Default for AddForm {
    fn default() -> Self {
        Self { host: String::new(), port: DEFAULT_PORT.to_string(), error: None }
    }
}

impl AddForm {
    /// Submit. On success the host field is cleared and the port is kept — the
    /// port is a setting, the host is one-shot.
    ///
    /// Qt clears the host only on two of its three paths: the `host:port`
    /// branch `return`s before `m_hostEdit->clear()`, so a successful inline
    /// add leaves the text sitting there looking unsent. Clearing on every
    /// success is the small fix.
    pub fn submit(&mut self) -> Option<AddPeer> {
        let port = self.port.trim().parse::<u16>().unwrap_or(0);
        match parse_address(&self.host, port) {
            Ok(a) => { self.host.clear(); self.error = None; Some(a) }
            Err(e) => { self.error = Some(e); None }
        }
    }

    pub fn error_text(&self) -> Option<&'static str> {
        self.error.map(|e| match e {
            AddrError::Empty => "Enter a host, an IP, or an sc- id",
            AddrError::NoHost => "No host before the colon",
            AddrError::BadPort => "Port must be between 1 and 65535",
        })
    }
}

// ── the card ────────────────────────────────────────────────────────────────

/// One action in an expanded card's submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardAction {
    pub label: String,
    pub id: String,
    /// Destructive, styled apart. Delete sitting in the same grey as Message is
    /// how someone removes a contact they meant to talk to.
    pub danger: bool,
}

impl CardAction {
    pub fn new(id: &str, label: &str) -> Self {
        Self { id: id.into(), label: label.into(), danger: false }
    }
    pub fn danger(id: &str, label: &str) -> Self {
        Self { id: id.into(), label: label.into(), danger: true }
    }
}

/// The accordion state of one row.
#[derive(Debug, Clone, Default)]
pub struct Card {
    pub expanded: bool,
    /// 0..=1 progress of the reveal, eased on read.
    t: f32,
}

impl Card {
    pub fn expand(&mut self) { self.expanded = true; }
    pub fn collapse(&mut self) { self.expanded = false; }
    pub fn toggle(&mut self) { self.expanded = !self.expanded; }

    /// Advance the 180ms animation. Driven by frame time rather than a wall
    /// clock so a slow frame does not skip the whole reveal.
    pub fn tick(&mut self, dt_ms: f32) {
        let target = if self.expanded { 1.0 } else { 0.0 };
        let step = dt_ms / SUBMENU_MS;
        if self.t < target { self.t = (self.t + step).min(target); }
        else if self.t > target { self.t = (self.t - step).max(target); }
    }

    /// Eased height fraction for the submenu.
    pub fn reveal(&self) -> f32 { ease::out_cubic(self.t.clamp(0.0, 1.0)) }

    /// The submenu is drawn whenever it has any height at all, not only when
    /// expanded — it must stay visible for the whole collapse animation or the
    /// card snaps shut instead of closing.
    pub fn submenu_visible(&self) -> bool { self.t > 0.0 }

    /// `▾` collapsed, `▴` expanded.
    pub fn chevron(&self) -> &'static str { if self.expanded { "▴" } else { "▾" } }

    pub fn settled(&self) -> bool {
        self.t <= 0.0 || self.t >= 1.0
    }
}

/// The accordion: at most one card open at a time.
///
/// `emit expanded(this)` exists so the owner closes the others. Two open cards
/// push the rest of the list off-screen and make the page look broken.
#[derive(Debug, Clone, Default)]
pub struct Accordion {
    pub open: Option<String>,
    cards: std::collections::HashMap<String, Card>,
}

impl Accordion {
    pub fn card(&self, id: &str) -> Card { self.cards.get(id).cloned().unwrap_or_default() }

    pub fn is_open(&self, id: &str) -> bool { self.open.as_deref() == Some(id) }

    /// Open one and close whatever else was open.
    pub fn open_only(&mut self, id: &str) {
        for (cid, c) in self.cards.iter_mut() {
            if cid != id { c.collapse(); }
        }
        self.cards.entry(id.to_string()).or_default().expand();
        self.open = Some(id.to_string());
    }

    pub fn close(&mut self, id: &str) {
        if let Some(c) = self.cards.get_mut(id) { c.collapse(); }
        if self.is_open(id) { self.open = None; }
    }

    pub fn toggle(&mut self, id: &str) {
        if self.is_open(id) { self.close(id); } else { self.open_only(id); }
    }

    /// Advance every card, including ones closing in the background.
    pub fn tick(&mut self, dt_ms: f32) {
        for c in self.cards.values_mut() { c.tick(dt_ms); }
    }
}

// ── the gesture state machine ───────────────────────────────────────────────

/// What the card decided a drag was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing yet.
    Idle,
    /// A tap: toggle the submenu. Consumed.
    Toggle,
    /// A horizontal swipe on an expanded card: collapse, and consume so the
    /// page nav does not ALSO act on it.
    SwipeClose,
    /// Recognised as a scroll. Not consumed — the scroller needs the events.
    YieldToScroll,
}

impl Outcome {
    /// Whether the card swallows the event.
    pub fn consumed(self) -> bool { matches!(self, Outcome::Toggle | Outcome::SwipeClose) }
}

/// Tracks one press→release on a card header.
#[derive(Debug, Clone, Default)]
pub struct Gesture {
    tracking: bool,
    armed: bool,
    press: (f32, f32),
}

impl Gesture {
    /// A press starts tracking and never consumes, so a button inside the
    /// header still receives it.
    pub fn press(&mut self, x: f32, y: f32) -> Outcome {
        self.tracking = true;
        self.armed = false;
        self.press = (x, y);
        Outcome::Idle
    }

    /// A move. `expanded` changes what the drag can mean, which is why it is a
    /// parameter rather than state here.
    pub fn move_to(&mut self, x: f32, y: f32, expanded: bool) -> Outcome {
        if !self.tracking { return Outcome::Idle; }
        let (ax, ay) = ((x - self.press.0).abs(), (y - self.press.1).abs());
        if expanded {
            // Vertical dominance first: this is the scroller's gesture and
            // stealing it makes the list feel stuck.
            if ay > ax && ay >= TAP_SLOP {
                self.tracking = false;
                return Outcome::YieldToScroll;
            }
            if ax >= SWIPE_CLOSE && ax > ay * X_DOMINANCE {
                self.armed = true;
                self.tracking = false;
                return Outcome::SwipeClose;
            }
        } else if ax >= TAP_SLOP || ay >= TAP_SLOP {
            // Collapsed: any real travel means the user is scrolling, so the
            // release must not toggle.
            self.tracking = false;
            return Outcome::YieldToScroll;
        }
        Outcome::Idle
    }

    /// A release. Only a near-stationary one toggles.
    pub fn release(&mut self, x: f32, y: f32) -> Outcome {
        if !self.tracking { self.armed = false; return Outcome::Idle; }
        let (dx, dy) = ((x - self.press.0).abs(), (y - self.press.1).abs());
        self.tracking = false;
        if !self.armed && dx < TAP_SLOP && dy < TAP_SLOP {
            return Outcome::Toggle;
        }
        Outcome::Idle
    }
}

// ── what the page asks for ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Add(AddPeer),
    Selected(String),
    /// A submenu button: (contact id, action id).
    Invoke(String, String),
    ImportVCard,
}

/// The actions every row offers. Delete is last and marked, so it is not
/// adjacent to the thing you press most.
pub fn default_actions() -> Vec<CardAction> {
    vec![
        CardAction::new("message", "Message"),
        CardAction::new("call", "Call"),
        CardAction::new("edit", "Edit contact"),
        CardAction::danger("delete", "Delete"),
    ]
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// The Friends page: roster of cards, then the add-peer form.
pub fn friends_ui(ui: &mut egui::Ui, roster: &Roster, acc: &mut Accordion,
                  form: &mut AddForm, c: &uikit_egui::PaletteColors) -> Option<Action> {
    let mut action = None;
    let dt = ui.input(|i| i.stable_dt) * 1000.0;
    acc.tick(dt);

    if roster.is_empty() {
        // The list widget is not drawn empty: an empty box with a heading looks
        // like a failure, the text tells you what to try.
        ui.add_space(8.0);
        ui.vertical_centered(|ui| { ui.colored_label(c.text_weak, EMPTY_TEXT); });
        ui.add_space(8.0);
    } else {
        ui.colored_label(c.text_weak,
            format!("{} of {} online", roster.online_count(), roster.len()));
        egui::ScrollArea::vertical().id_salt("friends_roster").show(ui, |ui| {
            for f in roster.rows() {
                let card = acc.card(&f.id);
                let resp = ui.horizontal(|ui| {
                    ui.colored_label(if f.online { c.success } else { c.text_weak }, f.dot());
                    ui.colored_label(if f.online { c.text } else { c.text_weak }, &f.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(c.text_weak, card.chevron());
                    });
                }).response.on_hover_text(f.tooltip());
                if resp.interact(egui::Sense::click()).clicked() {
                    acc.toggle(&f.id);
                    action = Some(Action::Selected(f.id.clone()));
                }
                if card.submenu_visible() {
                    // Height is animated, so the reveal slides rather than
                    // popping the rest of the list down a row.
                    let reveal = card.reveal();
                    for a in default_actions() {
                        let h = 24.0 * reveal;
                        if h < 1.0 { continue; }
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let text = egui::RichText::new(&a.label)
                                .color(if a.danger { c.danger } else { c.text });
                            if ui.add(egui::Button::new(text).min_size(egui::vec2(0.0, h)))
                                .clicked() {
                                action = Some(Action::Invoke(f.id.clone(), a.id.clone()));
                            }
                        });
                    }
                }
            }
        });
    }

    ui.separator();
    ui.colored_label(c.text_weak, "Add Peer by Address");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut form.host)
            .hint_text("Host / IP  or  DHT ID (sc-...)").desired_width(180.0));
        ui.add(egui::TextEdit::singleline(&mut form.port)
            .hint_text("Port").desired_width(56.0));
    });
    ui.horizontal(|ui| {
        if ui.button("Connect").clicked() {
            if let Some(a) = form.submit() { action = Some(Action::Add(a)); }
        }
        if ui.button("📥 Import vCard").clicked() { action = Some(Action::ImportVCard); }
    });
    if let Some(e) = form.error_text() { ui.colored_label(c.danger, e); }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, name: &str, online: bool) -> Friend {
        Friend { id: id.into(), name: name.into(), online }
    }

    #[test]
    fn the_same_peer_arriving_twice_updates_rather_than_duplicates() {
        // Discovery, a manual add and a reconnect all announce the same peer.
        let mut r = Roster::default();
        r.add_or_update(f("sc-1", "Ada", false));
        r.add_or_update(f("sc-1", "Ada", true));
        assert_eq!(r.len(), 1);
        assert!(r.get("sc-1").unwrap().online);
    }

    #[test]
    fn qt_label_drift() {
        // The bug this port avoids by construction. Qt builds the row as
        // "%1  %2" (dot, TWO spaces, name) and markOnline strips mid(2) — the
        // dot and ONE space — so a leading space survives every transition and
        // the name creeps right. Reproduced here to show what is being fixed.
        let mut qt_label = format!("{}  {}", "○", "Ada");
        for _ in 0..3 {
            let name = &qt_label[qt_label.char_indices().nth(2).unwrap().0..];
            qt_label = format!("{}  {}", "●", name);
        }
        assert!(qt_label.contains("   "), "Qt's label has accumulated padding: {qt_label:?}");

        // The port keeps the name in a field, so flapping changes nothing else.
        let mut r = Roster::default();
        r.add_or_update(f("sc-1", "Ada", false));
        for i in 0..12 { r.mark_online("sc-1", i % 2 == 0); }
        assert_eq!(r.get("sc-1").unwrap().name, "Ada", "no drift after a dozen transitions");
    }

    #[test]
    fn clearing_the_roster_shows_the_empty_state() {
        // Qt's setContacts never calls updateEmptyState, so clearing leaves an
        // empty list widget visible with the explanation hidden — a blank page,
        // which reads as a crash. Emptiness is derived here, so it cannot lag.
        let mut r = Roster::default();
        r.set(vec![f("sc-1", "Ada", true)]);
        assert!(!r.is_empty());
        r.set(vec![]);
        assert!(r.is_empty());
    }

    #[test]
    fn marking_an_unknown_peer_online_reports_rather_than_inventing_a_row() {
        let mut r = Roster::default();
        assert!(!r.mark_online("sc-ghost", true));
        assert!(r.is_empty(), "a status update for nobody must not create a contact");
    }

    #[test]
    fn the_status_dot_is_a_shape_not_only_a_colour() {
        assert_eq!(f("a", "A", true).dot(), "●");
        assert_eq!(f("a", "A", false).dot(), "○");
    }

    #[test]
    fn an_sc_id_is_a_dht_lookup_with_port_zero() {
        // Port 0 is the signal, not a placeholder: the controller resolves it.
        let a = parse_address("sc-a3f7c291", 9876).unwrap();
        assert_eq!(a, AddPeer { host: "sc-a3f7c291".into(), port: 0 });
        assert!(a.is_dht_lookup());
    }

    #[test]
    fn a_pasted_host_port_is_accepted_and_beats_the_port_field() {
        // Refusing a pasted "192.168.1.5:9876" for having a colon would be
        // maddening at the exact moment the user is already stuck.
        let a = parse_address("192.168.1.5:9999", 9876).unwrap();
        assert_eq!(a, AddPeer { host: "192.168.1.5".into(), port: 9999 });
        assert!(!a.is_dht_lookup());
    }

    #[test]
    fn an_ipv6_literal_splits_on_the_last_colon_not_the_first() {
        // Splitting on the first leaves you connecting to "fe80".
        let a = parse_address("[fe80::1]:9876", 0).unwrap();
        assert_eq!(a.host, "[fe80::1]");
        assert_eq!(a.port, 9876);
    }

    #[test]
    fn a_bare_host_takes_the_port_field() {
        assert_eq!(parse_address("192.168.1.5", 9876).unwrap(),
                   AddPeer { host: "192.168.1.5".into(), port: 9876 });
    }

    #[test]
    fn addresses_are_trimmed_because_they_arrive_pasted() {
        assert_eq!(parse_address("  192.168.1.5\n", 9876).unwrap().host, "192.168.1.5");
        assert_eq!(parse_address(" sc-abc ", 0).unwrap().host, "sc-abc");
    }

    #[test]
    fn the_unusable_addresses_say_why_instead_of_silently_doing_nothing() {
        // Qt returns early on each of these, so the button appears dead.
        assert_eq!(parse_address("", 9876), Err(AddrError::Empty));
        assert_eq!(parse_address("   ", 9876), Err(AddrError::Empty));
        assert_eq!(parse_address(":9876", 9876), Err(AddrError::NoHost));
        assert_eq!(parse_address("host:0", 9876), Err(AddrError::BadPort));
        assert_eq!(parse_address("host:notaport", 9876), Err(AddrError::BadPort));
        assert_eq!(parse_address("host", 0), Err(AddrError::BadPort));
    }

    #[test]
    fn a_successful_add_clears_the_host_on_every_path_including_the_inline_one() {
        // Qt's host:port branch returns before the clear, so a successful add
        // leaves the text sitting there looking unsent.
        let mut form = AddForm::default();
        form.host = "192.168.1.5:9876".into();
        assert!(form.submit().is_some());
        assert!(form.host.is_empty());
        assert_eq!(form.port, "9876", "the port is a setting and survives");
    }

    #[test]
    fn a_rejected_add_keeps_what_was_typed_and_explains() {
        // Clearing the field on a bad address makes the user retype it.
        let mut form = AddForm::default();
        form.host = ":9876".into();
        assert!(form.submit().is_none());
        assert_eq!(form.host, ":9876");
        assert!(form.error_text().is_some());
    }

    #[test]
    fn a_stationary_release_toggles() {
        let mut g = Gesture::default();
        assert_eq!(g.press(100.0, 100.0), Outcome::Idle);
        assert_eq!(g.release(102.0, 103.0), Outcome::Toggle);
        assert!(Outcome::Toggle.consumed());
    }

    #[test]
    fn a_press_never_consumes_so_header_buttons_still_work() {
        let mut g = Gesture::default();
        assert!(!g.press(0.0, 0.0).consumed());
    }

    #[test]
    fn a_scroll_on_a_collapsed_card_does_not_toggle_it_on_release() {
        // The failure this prevents: every attempt to scroll the friends list
        // opens whichever card you happened to touch.
        let mut g = Gesture::default();
        g.press(100.0, 100.0);
        assert_eq!(g.move_to(100.0, 140.0, false), Outcome::YieldToScroll);
        assert_eq!(g.release(100.0, 180.0), Outcome::Idle, "no toggle");
    }

    #[test]
    fn a_vertical_drag_on_an_expanded_card_yields_to_the_scroller() {
        let mut g = Gesture::default();
        g.press(100.0, 100.0);
        let out = g.move_to(104.0, 130.0, true);
        assert_eq!(out, Outcome::YieldToScroll);
        assert!(!out.consumed(), "the scroller needs these events");
    }

    #[test]
    fn a_horizontal_swipe_past_the_threshold_closes_and_consumes() {
        // Consuming is the point: without it the same drag closes the card AND
        // changes page.
        let mut g = Gesture::default();
        g.press(100.0, 100.0);
        let out = g.move_to(160.0, 105.0, true);
        assert_eq!(out, Outcome::SwipeClose);
        assert!(out.consumed());
    }

    #[test]
    fn a_diagonal_drag_is_not_horizontal_enough_to_close() {
        // 50px across and 40 down passes the distance threshold but fails
        // ax > ay * 1.5, so it is left alone rather than guessed at.
        let mut g = Gesture::default();
        g.press(100.0, 100.0);
        assert_eq!(g.move_to(150.0, 140.0, true), Outcome::Idle);
    }

    #[test]
    fn a_swipe_shorter_than_the_threshold_does_nothing() {
        let mut g = Gesture::default();
        g.press(100.0, 100.0);
        assert_eq!(g.move_to(140.0, 100.0, true), Outcome::Idle, "47px is not a swipe");
        assert_eq!(g.release(140.0, 100.0), Outcome::Idle, "and it is not a tap either");
    }

    #[test]
    fn only_one_card_is_open_at_a_time() {
        // Two open cards push the rest of the list off-screen.
        let mut a = Accordion::default();
        a.open_only("sc-1");
        a.open_only("sc-2");
        assert!(!a.is_open("sc-1"));
        assert!(a.is_open("sc-2"));
        assert!(!a.card("sc-1").expanded);
    }

    #[test]
    fn tapping_an_open_card_closes_it() {
        let mut a = Accordion::default();
        a.toggle("sc-1");
        assert!(a.is_open("sc-1"));
        a.toggle("sc-1");
        assert!(!a.is_open("sc-1"));
        assert_eq!(a.open, None);
    }

    #[test]
    fn a_closing_card_stays_drawn_until_the_animation_finishes() {
        // Dropping it at expanded=false snaps the card shut instead of
        // closing it.
        let mut c = Card::default();
        c.expand();
        c.tick(SUBMENU_MS);
        assert!(c.settled());
        c.collapse();
        c.tick(SUBMENU_MS / 4.0);
        assert!(c.submenu_visible(), "still drawn mid-collapse");
        assert!(c.reveal() > 0.0);
        c.tick(SUBMENU_MS);
        assert!(!c.submenu_visible());
    }

    #[test]
    fn the_reveal_is_eased_not_linear() {
        let mut c = Card::default();
        c.expand();
        c.tick(SUBMENU_MS / 4.0);
        assert!(c.reveal() > 0.25, "out_cubic leads a linear ramp early");
        assert!(c.reveal() < 1.0);
    }

    #[test]
    fn a_slow_frame_does_not_skip_the_whole_reveal() {
        // Frame-time driven, so one 100ms hitch advances 100ms of animation
        // rather than jumping to the end.
        let mut c = Card::default();
        c.expand();
        c.tick(100.0);
        assert!(!c.settled());
        assert!(c.reveal() < 1.0);
    }

    #[test]
    fn the_chevron_points_the_way_the_card_will_move() {
        let mut c = Card::default();
        assert_eq!(c.chevron(), "▾");
        c.expand();
        assert_eq!(c.chevron(), "▴");
    }

    #[test]
    fn delete_is_marked_dangerous_and_sits_last() {
        // In the same grey as Message it is how a contact gets removed by
        // accident.
        let acts = default_actions();
        assert!(acts.last().unwrap().danger);
        assert_eq!(acts.last().unwrap().id, "delete");
        assert_eq!(acts.iter().filter(|a| a.danger).count(), 1);
    }

    #[test]
    fn the_empty_state_names_the_ports_to_try() {
        assert!(EMPTY_TEXT.contains("9876"));
        assert!(EMPTY_TEXT.contains("9877"));
    }

    #[test]
    fn the_tooltip_shows_the_routable_id_behind_the_friendly_name() {
        let f = f("sc-a3f7", "Ada", true);
        assert!(f.tooltip().contains("sc-a3f7"));
        // An unnamed contact would otherwise show its id twice.
        assert_eq!(super::Friend { id: "sc-1".into(), name: "sc-1".into(), online: false }
                   .tooltip(), "sc-1");
    }
}
