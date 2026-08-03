//! session — `LoginWidget` and `OutboxWidget`.
//!
//! Two surfaces the port had the *state* for and no way to reach. `lib.rs`
//! carries the login overlay's visibility and Qt's auto-login ordering rule but
//! has no form behind it, so the overlay could only ever be hidden. `outbox.rs`
//! maps a snap's timer onto a mailbox lifetime and posts it, but nothing ever
//! showed whether the thing arrived.
//!
//! ## Sign-in
//!
//! The product position is on the widget itself: *"No account needed — any
//! username gets you started."* This is a local identity, not an authority
//! granting access to someone else's data, which is why the password is
//! optional and why a typo in it locks nobody out of anything.
//!
//! Two small rules carry most of the feel:
//!
//! * **Enter on the username moves to the password; it does not submit.** A
//!   form that submits from the first of two fields trains you to never use the
//!   second one.
//! * **An error clears the password and focuses it, not the username.** The
//!   username is almost never the thing that was mistyped, and re-selecting it
//!   makes the user retype something that was right.
//!
//! ### One thing reproduced rather than improved
//!
//! The password is hashed with a bare SHA-256 and hex-encoded — no salt, no KDF
//! — and an EMPTY password hashes just the same, so every passwordless account
//! shares one well-known digest. That is weak by any modern standard.
//!
//! It is kept exactly as it is, for two reasons. It is what the Qt app puts on
//! the wire and into storage, so changing it here would make this port unable
//! to sign in to accounts the Qt build created. And the fix is not a different
//! hash function in a UI file: it is a decision about whether this app wants
//! real authentication at all, which the "any username gets you started" line
//! suggests it deliberately does not. `hash_password` names the problem where
//! whoever revisits it will be standing.
//!
//! ## Outbox
//!
//! A send is not one event, it is a walk down a ladder of transports —
//! connecting, hole-punching, mailbox, re-lookup, relay, plain UDP — and the
//! outbox is the only place the user can see which rung it reached. That is why
//! failure keeps a phase and a detail rather than collapsing to "failed":
//! "✗ no route to peer" tells you to check the network, "✗ relay refused" does
//! not.
//!
//! Delivered is terminal. A late status update from a slower transport must not
//! walk a delivered snap backwards to "connecting", which is what the Qt code's
//! `if (r.delivered) return;` guard is for.
//!
//! Pruning drops delivered rows older than a day and *only* delivered ones. A
//! send that is still stuck after a week is exactly what the user came here to
//! look at.

// ── sign-in ─────────────────────────────────────────────────────────────────

/// The sign-in form's state.
#[derive(Debug, Clone, Default)]
pub struct Login {
    pub username: String,
    pub password: String,
    /// Shown only after a failed attempt; hidden to start, per `m_errorLabel->hide()`.
    pub error: Option<String>,
    /// Which field should take focus. The error path moves it, and getting this
    /// wrong is the difference between retyping a password and retyping a form.
    pub focus: Field,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field { #[default] Username, Password }

/// What the form produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    /// The hex digest. The plaintext never leaves the form.
    pub password_hash: String,
}

/// The strapline, which is also the security model stated out loud.
pub const NO_ACCOUNT_TEXT: &str = "No account needed — any username gets you started.";
pub const EMPTY_USERNAME_ERROR: &str = "Username cannot be empty.";

impl Login {
    /// Enter in the username box. Moves to the password rather than submitting:
    /// a form that submits from the first of two fields teaches you to ignore
    /// the second.
    pub fn username_return(&mut self) { self.focus = Field::Password; }

    /// Submit. `None` when the form is not usable, with `error` set to say why.
    pub fn submit(&mut self) -> Option<Credentials> {
        self.error = None;
        let username = self.username.trim().to_string();
        if username.is_empty() {
            self.fail(EMPTY_USERNAME_ERROR);
            return None;
        }
        // The password is optional and hashed either way — including when it is
        // empty. See the module note on why that stands.
        let password_hash = hash_password(&self.password);
        self.password.clear();
        Some(Credentials { username, password_hash })
    }

    /// `showError`: display it, clear the password, and put the cursor there.
    pub fn fail(&mut self, message: &str) {
        self.error = Some(message.to_string());
        self.password.clear();
        self.focus = Field::Password;
    }

    /// `clearError`.
    pub fn clear_error(&mut self) { self.error = None; }
}

/// `QCryptographicHash::hash(password.toUtf8(), Sha256).toHex()`.
///
/// Deliberately unchanged from the Qt implementation: unsalted, uniterated, and
/// applied to the empty string as readily as to a real password. Anything
/// stronger here would be unable to sign in to an account the Qt build made.
/// If this app ever wants real authentication, the change belongs in the
/// protocol, not in a widget.
pub fn hash_password(password: &str) -> String {
    // A local SHA-256 so this module does not pull a crypto crate into the view
    // layer for one call. The digest matches QCryptographicHash's.
    let mut h = Sha256::new();
    h.update(password.as_bytes());
    h.hex()
}

/// Minimal SHA-256. FIPS 180-4; verified against the published test vectors in
/// the tests below, because a hash that is subtly wrong here would silently
/// produce an account nobody can ever sign in to again.
struct Sha256 { state: [u32; 8], buf: Vec<u8>, len: u64 }

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2];

    fn new() -> Self {
        Sha256 { state: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                          0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
                 buf: Vec::new(), len: 0 }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        while self.buf.len() >= 64 {
            let block: [u8; 64] = self.buf[..64].try_into().unwrap();
            self.compress(&block);
            self.buf.drain(..64);
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(Self::K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) { *s = s.wrapping_add(v); }
    }

    fn hex(mut self) -> String {
        let bitlen = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 { self.buf.push(0); }
        let tail = bitlen.to_be_bytes();
        self.buf.extend_from_slice(&tail);
        let blocks: Vec<[u8; 64]> = self.buf.chunks(64)
            .map(|c| c.try_into().unwrap()).collect();
        for b in blocks { self.compress(&b); }
        self.state.iter().map(|w| format!("{w:08x}")).collect()
    }
}

// ── outbox ──────────────────────────────────────────────────────────────────

/// Where a send has got to. The ladder of transports, in the order the sender
/// walks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    /// NAT hole-punching.
    Punching,
    /// Left in the peer's mailbox for later.
    Mailbox,
    /// The peer moved; looking it up again.
    Relookup,
    Relay,
    Udp,
    Failed,
    /// The device has no network at all — distinct from Failed, because there
    /// is nothing wrong with the send and it will resume.
    Offline,
    Unknown,
}

impl Phase {
    pub fn from_tag(s: &str) -> Phase {
        match s {
            "connecting" => Phase::Connecting,
            "punching" => Phase::Punching,
            "mailbox" => Phase::Mailbox,
            "relookup" => Phase::Relookup,
            "relay" => Phase::Relay,
            "udp" => Phase::Udp,
            "failed" => Phase::Failed,
            "offline" => Phase::Offline,
            _ => Phase::Unknown,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Phase::Connecting => "connecting",
            Phase::Punching => "punching",
            Phase::Mailbox => "mailbox",
            Phase::Relookup => "relookup",
            Phase::Relay => "relay",
            Phase::Udp => "udp",
            Phase::Failed => "failed",
            Phase::Offline => "offline",
            Phase::Unknown => "unknown",
        }
    }

    /// `phaseIcon`. An unrecognised phase gets "…" rather than nothing, so a
    /// new transport added on the sending side does not render a blank row.
    pub fn icon(self) -> &'static str {
        match self {
            Phase::Connecting => "⟳",
            Phase::Punching => "⚡",
            Phase::Mailbox => "📬",
            Phase::Relookup => "🔍",
            Phase::Relay => "🔀",
            Phase::Udp => "📡",
            Phase::Failed => "✗",
            Phase::Offline => "⏸",
            Phase::Unknown => "…",
        }
    }

    /// Only failure is coloured. Every other phase is in-progress and colouring
    /// them all would make a working send look alarming.
    pub fn is_failure(self) -> bool { matches!(self, Phase::Failed) }
}

/// One row of the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRow {
    pub id: String,
    pub recipient: String,
    /// Unix seconds.
    pub sent_at: i64,
    pub phase: Phase,
    /// The specific reason, when there is one. "✗ no route to peer" tells you
    /// to check the network; "✗ failed" does not.
    pub detail: String,
    pub delivered: bool,
}

impl SendRow {
    /// The status line: icon, then the detail if there is one, else the phase.
    pub fn status(&self) -> String {
        if self.delivered { return "✓ Delivered".into(); }
        let label = if self.detail.trim().is_empty() { self.phase.tag() } else { self.detail.trim() };
        format!("{} {}", self.phase.icon(), label)
    }
}

/// The outbox.
#[derive(Debug, Clone, Default)]
pub struct Outbox {
    rows: Vec<SendRow>,
}

/// A day, the prune horizon for delivered rows.
pub const PRUNE_AFTER_SECS: i64 = 24 * 3600;
pub const OUTBOX_EMPTY_TEXT: &str = "No sent snaps yet.\nSend one from the Camera tab!";

impl Outbox {
    pub fn rows(&self) -> &[SendRow] { &self.rows }
    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }

    /// A new send goes on top — `insertWidget(0, card)`.
    pub fn add(&mut self, id: &str, recipient: &str, sent_at: i64) {
        self.rows.insert(0, SendRow {
            id: id.into(), recipient: recipient.into(), sent_at,
            phase: Phase::Connecting, detail: String::new(), delivered: false,
        });
    }

    /// `updateStatus`. Refuses to move a delivered row: a late update from a
    /// slower transport must not walk an arrived snap backwards to
    /// "connecting", which is worse than showing nothing because it looks like
    /// the snap came back.
    pub fn update(&mut self, id: &str, phase: Phase, detail: &str) -> bool {
        match self.rows.iter_mut().find(|r| r.id == id) {
            Some(r) if !r.delivered => { r.phase = phase; r.detail = detail.into(); true }
            _ => false,
        }
    }

    /// `markDelivered`. Idempotent.
    pub fn mark_delivered(&mut self, id: &str) -> bool {
        match self.rows.iter_mut().find(|r| r.id == id) {
            Some(r) if !r.delivered => { r.delivered = true; true }
            _ => false,
        }
    }

    /// `rebuildSummary` — "N / M delivered". `None` when there is nothing to
    /// summarise, so an empty outbox does not show "0 / 0".
    pub fn summary(&self) -> Option<String> {
        if self.rows.is_empty() { return None; }
        let d = self.rows.iter().filter(|r| r.delivered).count();
        Some(format!("{d} / {} delivered", self.rows.len()))
    }

    /// Drop delivered rows older than a day. ONLY delivered ones: a send still
    /// stuck after a week is exactly what the user opened this page to see.
    pub fn prune(&mut self, now: i64) -> usize {
        let before = self.rows.len();
        self.rows.retain(|r| !(r.delivered && now - r.sent_at > PRUNE_AFTER_SECS));
        before - self.rows.len()
    }
}

/// `relativeTime`, via the shared helper.
///
/// This copy was missing the clock-skew guard its three siblings had — a device
/// reporting a future timestamp would have rendered "-4m ago" in the outbox.
/// Delegating fixed that without anyone having to notice it here.
pub fn relative_time(secs: i64) -> String {
    uikit_egui::fmt::relative_time(secs, uikit_egui::fmt::Style::Ago)
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// The sign-in form. Returns credentials once the user submits.
pub fn login_ui(ui: &mut egui::Ui, l: &mut Login,
                c: &uikit_egui::PaletteColors) -> Option<Credentials> {
    let mut out = None;
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.heading("SnapClone");
        ui.add_space(12.0);

        let user = ui.add(egui::TextEdit::singleline(&mut l.username)
            .hint_text("Username").desired_width(240.0));
        if l.focus == Field::Username { user.request_focus(); }
        if user.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            l.username_return();
        }

        let pass = ui.add(egui::TextEdit::singleline(&mut l.password)
            .hint_text("Password (optional)").password(true).desired_width(240.0));
        if l.focus == Field::Password { pass.request_focus(); }
        let pass_enter = pass.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if let Some(e) = &l.error {
            ui.colored_label(c.danger, e);
        }
        if ui.button("Sign In").clicked() || pass_enter {
            out = l.submit();
        }
        ui.add_space(8.0);
        ui.colored_label(c.text_weak, NO_ACCOUNT_TEXT);
    });
    out
}

/// The outbox page.
pub fn outbox_ui(ui: &mut egui::Ui, o: &Outbox, now: i64, c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        ui.heading("Outbox");
        if let Some(s) = o.summary() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.accent, s);
            });
        }
    });
    if o.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| { ui.colored_label(c.text_weak, OUTBOX_EMPTY_TEXT); });
        return;
    }
    egui::ScrollArea::vertical().id_salt("outbox").show(ui, |ui| {
        for r in o.rows() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    // A delivered row is dimmed, not hidden: it is still proof
                    // the snap went, which is the whole reason to look here.
                    let strong = if r.delivered { c.text_weak } else { c.text };
                    ui.colored_label(strong, &r.recipient);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(c.text_weak, relative_time(now - r.sent_at));
                    });
                });
                let colour = if r.delivered { c.success }
                             else if r.phase.is_failure() { c.danger }
                             else { c.text_weak };
                ui.colored_label(colour, r.status());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── sign-in ─────────────────────────────────────────────────────────────

    #[test]
    fn the_sha256_matches_the_published_vectors() {
        // A hash that is subtly wrong would silently create accounts nobody can
        // ever sign in to again, so this is checked against FIPS 180-4's own
        // examples rather than against itself.
        assert_eq!(hash_password(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hash_password("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hash_password("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn the_hash_survives_a_message_longer_than_one_block() {
        // The padding and multi-block path is where a hand-rolled SHA-256 goes
        // wrong, and a 64-byte boundary is exactly where.
        let a = "a".repeat(64);
        assert_eq!(hash_password(&a),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb");
        let long = "a".repeat(1000);
        assert_eq!(hash_password(&long).len(), 64);
    }

    #[test]
    fn a_password_is_never_carried_in_the_clear() {
        let mut l = Login::default();
        l.username = "ada".into();
        l.password = "hunter2".into();
        let c = l.submit().expect("valid");
        assert!(!format!("{c:?}").contains("hunter2"));
        assert_eq!(c.password_hash.len(), 64);
    }

    #[test]
    fn the_form_clears_the_password_after_submitting() {
        let mut l = Login::default();
        l.username = "ada".into();
        l.password = "hunter2".into();
        l.submit();
        assert!(l.password.is_empty(), "plaintext must not linger in the form");
    }

    #[test]
    fn every_passwordless_account_shares_one_digest() {
        // FAITHFUL, NOT ENDORSED. An empty password hashes like any other, so
        // all passwordless accounts collide on the well-known empty digest.
        // Kept because it is what the Qt build puts in storage, and changing it
        // here would make this port unable to sign in to those accounts. See
        // the module note.
        assert_eq!(hash_password(""), hash_password(""));
        let mut a = Login { username: "ada".into(), ..Default::default() };
        let mut b = Login { username: "bo".into(), ..Default::default() };
        assert_eq!(a.submit().unwrap().password_hash, b.submit().unwrap().password_hash);
    }

    #[test]
    fn an_empty_username_is_refused_with_a_reason() {
        let mut l = Login::default();
        assert!(l.submit().is_none());
        assert_eq!(l.error.as_deref(), Some(EMPTY_USERNAME_ERROR));
    }

    #[test]
    fn a_username_of_only_spaces_is_empty() {
        let mut l = Login { username: "   ".into(), ..Default::default() };
        assert!(l.submit().is_none());
    }

    #[test]
    fn the_username_is_trimmed_because_it_arrives_from_a_soft_keyboard() {
        let mut l = Login { username: "  ada  ".into(), ..Default::default() };
        assert_eq!(l.submit().unwrap().username, "ada");
    }

    #[test]
    fn a_password_is_optional_which_is_the_stated_product_position() {
        let mut l = Login { username: "ada".into(), ..Default::default() };
        assert!(l.submit().is_some());
        assert!(NO_ACCOUNT_TEXT.contains("No account needed"));
    }

    #[test]
    fn enter_on_the_username_moves_to_the_password_rather_than_submitting() {
        // A form that submits from the first of two fields teaches you to
        // ignore the second.
        let mut l = Login { username: "ada".into(), ..Default::default() };
        l.username_return();
        assert_eq!(l.focus, Field::Password);
        assert!(l.error.is_none(), "and it is not an attempt, so no error");
    }

    #[test]
    fn a_failure_clears_the_password_and_puts_the_cursor_there() {
        // The username is almost never what was mistyped; re-selecting it makes
        // the user retype something that was right.
        let mut l = Login { username: "ada".into(), password: "wrong".into(), ..Default::default() };
        l.fail("Sign-in failed.");
        assert!(l.password.is_empty());
        assert_eq!(l.focus, Field::Password);
        assert_eq!(l.username, "ada", "and the username survives");
    }

    #[test]
    fn the_error_starts_hidden_and_clears_on_the_next_attempt() {
        let mut l = Login::default();
        assert!(l.error.is_none(), "a form does not open accusing you of anything");
        l.fail("nope");
        assert!(l.error.is_some());
        l.username = "ada".into();
        l.submit();
        assert!(l.error.is_none());
    }

    // ── outbox ──────────────────────────────────────────────────────────────

    #[test]
    fn the_newest_send_goes_on_top() {
        let mut o = Outbox::default();
        o.add("1", "ada", 100);
        o.add("2", "bo", 200);
        assert_eq!(o.rows()[0].id, "2");
    }

    #[test]
    fn a_send_starts_connecting() {
        let mut o = Outbox::default();
        o.add("1", "ada", 0);
        assert_eq!(o.rows()[0].phase, Phase::Connecting);
        assert_eq!(o.rows()[0].status(), "⟳ connecting");
    }

    #[test]
    fn a_failure_keeps_its_reason_instead_of_collapsing_to_failed() {
        // "✗ no route to peer" tells you to check the network; "✗ failed"
        // tells you nothing.
        let mut o = Outbox::default();
        o.add("1", "ada", 0);
        o.update("1", Phase::Failed, "no route to peer");
        assert_eq!(o.rows()[0].status(), "✗ no route to peer");
        assert!(o.rows()[0].phase.is_failure());
    }

    #[test]
    fn a_phase_with_no_detail_shows_the_phase() {
        let mut o = Outbox::default();
        o.add("1", "ada", 0);
        o.update("1", Phase::Relay, "   ");
        assert_eq!(o.rows()[0].status(), "🔀 relay");
    }

    #[test]
    fn delivered_is_terminal_and_a_late_update_cannot_walk_it_backwards() {
        // THE guard. A straggling status from a slower transport rewriting an
        // arrived snap to "connecting" looks like the snap came back.
        let mut o = Outbox::default();
        o.add("1", "ada", 0);
        assert!(o.mark_delivered("1"));
        assert!(!o.update("1", Phase::Connecting, "retrying"));
        assert_eq!(o.rows()[0].status(), "✓ Delivered");
        assert!(!o.mark_delivered("1"), "and delivering twice is not a change");
    }

    #[test]
    fn a_status_for_an_unknown_snap_is_reported_not_invented() {
        let mut o = Outbox::default();
        assert!(!o.update("ghost", Phase::Relay, ""));
        assert!(o.is_empty(), "an update for nobody must not create a row");
    }

    #[test]
    fn offline_is_not_failure_because_nothing_is_wrong() {
        // A paused send resumes; a failed one will not. Colouring them the same
        // makes a phone in a lift look broken.
        assert!(!Phase::Offline.is_failure());
        assert_eq!(Phase::Offline.icon(), "⏸");
        assert!(Phase::Failed.is_failure());
    }

    #[test]
    fn an_unrecognised_phase_still_renders() {
        // A new transport added on the sending side must not produce a blank
        // row on this one.
        assert_eq!(Phase::from_tag("quantum-tunnel"), Phase::Unknown);
        assert_eq!(Phase::Unknown.icon(), "…");
    }

    #[test]
    fn phase_tags_round_trip() {
        for p in [Phase::Connecting, Phase::Punching, Phase::Mailbox, Phase::Relookup,
                  Phase::Relay, Phase::Udp, Phase::Failed, Phase::Offline] {
            assert_eq!(Phase::from_tag(p.tag()), p);
        }
    }

    #[test]
    fn pruning_drops_old_delivered_rows_and_keeps_old_stuck_ones() {
        // A send still stuck after a week is exactly what the user opened this
        // page to look at.
        let mut o = Outbox::default();
        let now = 1_000_000;
        o.add("old-delivered", "ada", now - 2 * PRUNE_AFTER_SECS);
        o.add("old-stuck", "bo", now - 7 * PRUNE_AFTER_SECS);
        o.add("fresh-delivered", "cy", now - 60);
        o.mark_delivered("old-delivered");
        o.mark_delivered("fresh-delivered");
        assert_eq!(o.prune(now), 1);
        let ids: Vec<&str> = o.rows().iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"old-stuck"));
        assert!(ids.contains(&"fresh-delivered"));
        assert!(!ids.contains(&"old-delivered"));
    }

    #[test]
    fn a_delivered_row_exactly_at_the_horizon_is_kept() {
        let mut o = Outbox::default();
        let now = 1_000_000;
        o.add("edge", "ada", now - PRUNE_AFTER_SECS);
        o.mark_delivered("edge");
        assert_eq!(o.prune(now), 0, "strictly older than a day, not exactly");
    }

    #[test]
    fn the_summary_counts_delivered_and_hides_itself_when_empty() {
        let mut o = Outbox::default();
        assert_eq!(o.summary(), None, "an empty outbox does not say 0 / 0");
        o.add("1", "ada", 0);
        o.add("2", "bo", 0);
        o.mark_delivered("1");
        assert_eq!(o.summary().as_deref(), Some("1 / 2 delivered"));
    }

    #[test]
    fn the_empty_state_says_where_to_send_one_from() {
        assert!(OUTBOX_EMPTY_TEXT.contains("Camera"));
    }

    #[test]
    fn a_device_with_a_fast_clock_does_not_render_a_negative_age() {
        // This copy was the one MISSING the guard its three siblings had. A
        // send whose timestamp lands in the future would have shown "-4m ago"
        // in the outbox, which reads as the app being broken rather than the
        // clock. Delegating to the shared helper fixed it; this pins it.
        assert_eq!(relative_time(-240), "just now");
        assert!(!relative_time(-1).contains('-'));
    }

    #[test]
    fn relative_time_matches_the_qt_thresholds() {
        assert_eq!(relative_time(30), "just now");
        assert_eq!(relative_time(59), "just now");
        assert_eq!(relative_time(60), "1m ago");
        assert_eq!(relative_time(3599), "59m ago");
        assert_eq!(relative_time(3600), "1h ago");
        assert_eq!(relative_time(86_400), "1d ago");
    }
}
