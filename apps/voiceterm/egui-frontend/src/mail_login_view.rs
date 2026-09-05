//! mail_login_view — the sign-in page for a mail account.
//!
//! voiceterm-qt's `mail_login.rs` renders a QML form — username, password,
//! an optional 2FA code and, for two-password-mode accounts, a separate
//! mailbox password — whose submit calls `backend.mail_login(user, pass,
//! totp, mailbox)`. The egui port had the mail list, threads and the
//! account selector, and no way in. This is the way in: the same four
//! fields, the same submit contract, with the checks a form should make
//! before it costs a network round trip kept pure and tested.
//!
//! What it does NOT do: authenticate. `Form::submit` yields a
//! [`Credentials`] the host hands to the bridge (`mail_login`); the
//! password never lands in a log, the struct's Debug redacts it, and the
//! form clears the secrets on submit whether or not the host succeeds —
//! the host owns the retry, and re-prompting is cheaper than a password
//! sitting in a widget's state for the rest of the session.

use uikit_egui::PaletteColors;

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    /// Six digits, when the account has 2FA.
    pub totp: Option<String>,
    /// Two-password mode: the mailbox password, separate from login.
    pub mailbox_password: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("username", &self.username)
            .field("totp", &self.totp.as_ref().map(|_| "***"))
            .field("mailbox_password", &self.mailbox_password.as_ref().map(|_| "***")).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub username: String,
    pub password: String,
    pub totp: String,
    pub two_password: bool,
    pub mailbox_password: String,
    /// The last refusal, shown inline. Cleared on the next edit.
    pub error: Option<String>,
    /// The host's in-flight flag: disables the button, shows "signing in…".
    pub busy: bool,
}

impl Form {
    /// Field checks, in the order a person reads the form. `Err` is the
    /// message to show; nothing leaves the form until every check passes.
    pub fn validate(&self) -> Result<Credentials, String> {
        let user = self.username.trim();
        if user.is_empty() { return Err("username is empty".into()); }
        if user.contains(char::is_whitespace) { return Err("username has a space in it".into()); }
        if self.password.is_empty() { return Err("password is empty".into()); }
        let totp = self.totp.trim();
        let totp = if totp.is_empty() { None } else {
            let digits: String = totp.chars().filter(|c| !c.is_whitespace()).collect();
            if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err("2FA code is six digits".into());
            }
            Some(digits)
        };
        let mailbox = if self.two_password {
            if self.mailbox_password.is_empty() { return Err("two-password mode needs the mailbox password".into()); }
            Some(self.mailbox_password.clone())
        } else { None };
        Ok(Credentials { username: user.to_string(), password: self.password.clone(), totp, mailbox_password: mailbox })
    }

    /// Validate; on success clear the secrets and hand the credentials up.
    pub fn submit(&mut self) -> Option<Credentials> {
        match self.validate() {
            Ok(c) => {
                self.password.clear();
                self.totp.clear();
                self.mailbox_password.clear();
                self.error = None;
                Some(c)
            }
            Err(e) => { self.error = Some(e); None }
        }
    }
}

/// The page. Returns credentials when the person submits a valid form.
pub fn login_ui(ui: &mut egui::Ui, form: &mut Form, c: &PaletteColors) -> Option<Credentials> {
    let mut out = None;
    ui.heading("Sign in to mail");
    ui.colored_label(c.text_weak, "Your credentials go to the mail bridge on this device and nowhere else.");
    ui.add_space(8.0);
    let mut edited = false;
    egui::Grid::new("mail-login").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("username");
        edited |= ui.add(egui::TextEdit::singleline(&mut form.username).hint_text("you@proton.me").desired_width(f32::INFINITY)).changed();
        ui.end_row();
        ui.label("password");
        edited |= ui.add(egui::TextEdit::singleline(&mut form.password).password(true).desired_width(f32::INFINITY)).changed();
        ui.end_row();
        ui.label("2FA code");
        edited |= ui.add(egui::TextEdit::singleline(&mut form.totp).hint_text("optional, 6 digits").desired_width(f32::INFINITY)).changed();
        ui.end_row();
        ui.label("");
        edited |= ui.checkbox(&mut form.two_password, "two-password mode (separate mailbox password)").changed();
        ui.end_row();
        if form.two_password {
            ui.label("mailbox password");
            edited |= ui.add(egui::TextEdit::singleline(&mut form.mailbox_password).password(true).desired_width(f32::INFINITY)).changed();
            ui.end_row();
        }
    });
    if edited { form.error = None; }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let label = if form.busy { "signing in…" } else { "sign in" };
        let go = ui.add_enabled(!form.busy, egui::Button::new(label)).clicked()
            || (!form.busy && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if go { out = form.submit(); }
        if let Some(e) = &form.error { ui.colored_label(c.danger, e); }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checks_run_in_reading_order_and_normalise_the_code() {
        let mut f = Form::default();
        assert_eq!(f.validate().unwrap_err(), "username is empty");
        f.username = "a b".into();
        assert_eq!(f.validate().unwrap_err(), "username has a space in it");
        f.username = " kes@proton.me ".into();
        assert_eq!(f.validate().unwrap_err(), "password is empty");
        f.password = "hunter2".into();
        f.totp = "12 34".into();
        assert_eq!(f.validate().unwrap_err(), "2FA code is six digits");
        f.totp = "123 456".into();
        let c = f.validate().unwrap();
        assert_eq!(c.username, "kes@proton.me");
        assert_eq!(c.totp.as_deref(), Some("123456"));
        assert_eq!(c.mailbox_password, None);
        f.two_password = true;
        assert_eq!(f.validate().unwrap_err(), "two-password mode needs the mailbox password");
        f.mailbox_password = "mb".into();
        assert_eq!(f.validate().unwrap().mailbox_password.as_deref(), Some("mb"));
    }

    #[test]
    fn submit_clears_the_secrets_and_debug_never_prints_them() {
        let mut f = Form { username: "u".into(), password: "p".into(), totp: "123456".into(), ..Default::default() };
        let c = f.submit().unwrap();
        assert_eq!(c.password, "p");
        assert!(f.password.is_empty() && f.totp.is_empty(), "secrets do not linger in the form");
        assert_eq!(f.username, "u", "the username stays for a retry");
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("\"p\"") && !dbg.contains("123456"), "{dbg}");
        let mut bad = Form::default();
        assert!(bad.submit().is_none());
        assert_eq!(bad.error.as_deref(), Some("username is empty"));
    }
}
