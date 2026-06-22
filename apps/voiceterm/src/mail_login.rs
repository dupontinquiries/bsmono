//! Account onboarding for the mail bridge: a login scaffold page for adding
//! ProtonMail accounts, an accounts-management pane for the settings view,
//! and the markdown for a `mail` settings manpage.
//!
//! v0.1.862: Two entry points were requested — a dedicated login page *or*
//! doing it from the settings manpage. This module provides both, because
//! they share almost everything:
//!
//!   * [`login_page_qml`] — the standalone add-account form (username,
//!     password, and the optional 2FA code + separate mailbox password for
//!     two-password-mode accounts). Its submit button calls
//!     `backend.mail_login(user, pass, totp, mailbox)`.
//!   * [`accounts_settings_qml`] — the list of existing accounts with a
//!     remove button each and an "Add account" button that opens the login
//!     page. This is what the settings view embeds.
//!   * [`settings_manpage_md`] — markdown describing setup, multi-account,
//!     and the security model, suitable for registration in `manpages.rs`
//!     (one line in its table) or direct rendering via
//!     `manpages::to_qml(settings_manpage_md(), style)`.
//!
//! All three are pure string builders, unit-tested without Qt.

use crate::mail_account::AccountSummary;
use crate::mail_panes::{qml_escape as qe, PaneStyle};

/// The standalone add-account login page. `error` renders an inline message
/// (e.g. after a failed attempt or a 2FA prompt). The form is intentionally
/// minimal: ProtonMail's two-password mode and TOTP are optional fields the
/// user fills only if their account uses them.
pub fn login_page_qml(style: &PaneStyle, error: Option<&str>) -> String {
    let mut o = String::with_capacity(2048);
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Item {{\nwidth: parent ? parent.width : 480\nimplicitHeight: childrenRect.height\nColumn {{\nwidth: Math.min(parent.width, 480)\nanchors.horizontalCenter: parent.horizontalCenter\nspacing: 10\npadding: 16\nText {{ text: \"Add ProtonMail account\"; color: \"{accent}\"; font.bold: true; font.pixelSize: {hs}; font.family: \"{ff}\" }}\nText {{ text: \"Bridges your own Proton account locally. Your password runs an SRP login and is never sent in the clear.\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\"; wrapMode: Text.WordWrap; width: parent.width }}\n",
            accent = qe(&style.accent),
            hs = style.font_px + 4,
            ff = qe(&style.font_family),
            muted = qe(&style.muted),
            fss = style.font_px - 1,
        ),
    );

    if let Some(err) = error {
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!(
                "Rectangle {{ width: parent.width; radius: 4; color: \"#40331a\"; border.color: \"{accent}\"; border.width: 1; implicitHeight: _err.height + 12; Text {{ id: _err; x: 8; y: 6; width: parent.width - 16; text: \"{msg}\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; wrapMode: Text.WordWrap }} }}\n",
                accent = qe(&style.accent),
                msg = qe(err),
                fg = qe(&style.fg),
                fs = style.font_px,
                ff = qe(&style.font_family),
            ),
        );
    }

    // Fields.
    field(&mut o, style, "login_user", "Email or username", false, "");
    field(&mut o, style, "login_pass", "Password", true, "");
    field(&mut o, style, "login_mbox", "Mailbox password (two-password mode only)", true, "");
    field(&mut o, style, "login_totp", "2FA code (if enabled)", false, "");

    // Submit. Gathers the named fields and calls the backend.
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Rectangle {{\nradius: 4\ncolor: \"{bg}\"\nborder.color: \"{accent}\"\nborder.width: 1\nimplicitWidth: parent.width\nimplicitHeight: _bt.height + 16\nText {{ id: _bt; anchors.centerIn: parent; text: \"Add account\"; color: \"{fg}\"; font.bold: true; font.pixelSize: {fs}; font.family: \"{ff}\" }}\nMouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) {{ var _r = backend.mail_login(_f(\"login_user\"), _f(\"login_pass\"), _f(\"login_totp\"), _f(\"login_mbox\")); _setStatus(_r) }} }} }}\n}}\n",
            bg = qe(&style.button_bg),
            accent = qe(&style.accent),
            fg = qe(&style.button_fg),
            fs = style.font_px,
            ff = qe(&style.font_family),
        ),
    );

    // v0.1.869: in-pane status line. A pane's loaded QML can't reach the global
    // `toast`, so login feedback (success / 2FA needed / error) shows here. Driven
    // by `_setStatus` from the submit handler's `mail_login` result wire.
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Text {{ id: _status; objectName: \"login_status\"; width: parent.width; text: \"\"; visible: text.length > 0; wrapMode: Text.WordWrap; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\" }}\n",
            fg = qe(&style.fg),
            fs = style.font_px,
            ff = qe(&style.font_family),
        ),
    );

    // Field accessor by objectName (searches descendants, since fields sit
    // in this Column).
    o.push_str(
        "function _f(n) { return _find(this, n) }\n\
         function _find(node, n) { if (!node || !node.children) return \"\"; for (var i=0;i<node.children.length;i++){ var c=node.children[i]; if (c.objectName===n) return c.text||\"\"; var r=_find(c,n); if (r!==\"\") return r } return \"\" }\n\
         function _setStatus(r) { if (!r) { _status.text=\"\"; return } var p=r.split(\"\\t\"); if (p[0]===\"ok\") { _status.color=\"#86c986\"; _status.text=(p[1]||\"Logged in\") } else if (p[0]===\"totp\") { _status.color=\"#d8b873\"; _status.text=(p[1]||\"Enter your 2FA code, then submit again\") } else { _status.color=\"#e0928f\"; _status.text=(p[1]||\"Login failed\") } }\n",
    );

    o.push_str("}\n}\n"); // Column, root Item
    o
}

/// Accounts-management pane for the settings view: each known account with a
/// Remove button, plus an Add-account button that opens the login page.
pub fn accounts_settings_qml(style: &PaneStyle, accounts: &[AccountSummary]) -> String {
    let mut o = String::with_capacity(accounts.len() * 200 + 512);
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Item {{\nwidth: parent ? parent.width : 520\nimplicitHeight: childrenRect.height\nColumn {{\nwidth: parent.width\nspacing: 8\npadding: 12\nText {{ text: \"Mail accounts\"; color: \"{accent}\"; font.bold: true; font.pixelSize: {hs}; font.family: \"{ff}\" }}\n",
            accent = qe(&style.accent),
            hs = style.font_px + 3,
            ff = qe(&style.font_family),
        ),
    );

    if accounts.is_empty() {
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!(
                "Text {{ text: \"No accounts yet.\"; color: \"{muted}\"; font.pixelSize: {fs}; font.family: \"{ff}\" }}\n",
                muted = qe(&style.muted),
                fs = style.font_px,
                ff = qe(&style.font_family),
            ),
        );
    }

    for a in accounts {
        let label = if a.display_name.is_empty() {
            a.username.clone()
        } else {
            a.display_name.clone()
        };
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!(
                "Rectangle {{\nwidth: parent.width\nimplicitHeight: _row_{id}.height + 12\ncolor: \"transparent\"\nborder.color: \"{border}\"\nborder.width: 1\nradius: 4\nRow {{\nid: _row_{id}\nx: 8; y: 6\nwidth: parent.width - 16\nspacing: 10\nColumn {{\nwidth: parent.width - 90\nText {{ text: \"{label}\"; color: \"{fg}\"; font.bold: true; font.pixelSize: {fs}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width }}\nText {{ text: \"{user}\"; color: \"{muted}\"; font.pixelSize: {fss}; font.family: \"{ff}\"; elide: Text.ElideRight; width: parent.width }}\n}}\nRectangle {{ radius: 4; color: \"transparent\"; border.color: \"{border}\"; border.width: 1; implicitWidth: _rm_{id}.width + 16; implicitHeight: _rm_{id}.height + 10; Text {{ id: _rm_{id}; anchors.centerIn: parent; text: \"Remove\"; color: \"{fg}\"; font.pixelSize: {fss}; font.family: \"{ff}\" }} MouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) backend.mail_remove_account(\"{aid}\") }} }} }}\n}}\n}}\n",
                id = sanitize(a.id.as_str()),
                border = qe(&style.border),
                label = qe(&label),
                fg = qe(&style.fg),
                fs = style.font_px,
                ff = qe(&style.font_family),
                user = qe(&a.username),
                muted = qe(&style.muted),
                fss = style.font_px - 2,
                aid = qe(a.id.as_str()),
            ),
        );
    }

    // Add-account button.
    let _ = std::fmt::Write::write_fmt(
        &mut o,
        format_args!(
            "Rectangle {{\nradius: 4\ncolor: \"{bg}\"\nborder.color: \"{accent}\"\nborder.width: 1\nimplicitWidth: _add.width + 24\nimplicitHeight: _add.height + 14\nText {{ id: _add; anchors.centerIn: parent; text: \"+ Add account\"; color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\" }}\nMouseArea {{ anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: {{ if (backend) backend.mail_open_login() }} }}\n}}\n",
            bg = qe(&style.button_bg),
            accent = qe(&style.accent),
            fg = qe(&style.button_fg),
            fs = style.font_px,
            ff = qe(&style.font_family),
        ),
    );

    o.push_str("}\n}\n"); // Column, root Item
    o
}

/// Markdown for the `mail` settings manpage. Register in `manpages.rs` (add
/// `"mail" => include_str!(...)` or an entry returning this), or render
/// directly with `crate::manpages::to_qml(settings_manpage_md(), style)`.
pub fn settings_manpage_md() -> &'static str {
    MAIL_MANPAGE
}

/// Markdown for the `mail-storage` manpage — where data lives, how big it
/// is, and how to manage or wipe it.
pub fn storage_manpage_md() -> &'static str {
    STORAGE_MANPAGE
}

/// Markdown for the `email` manpage — the reading/acting workflow and the
/// full voice vocabulary.
pub fn emails_manpage_md() -> &'static str {
    EMAILS_MANPAGE
}

const MAIL_MANPAGE: &str = r#"# mail — ProtonMail bridge

web2qml can bridge one or more of **your own** ProtonMail accounts locally,
so a standard mail client (or the built-in inbox/compose panes) can read and
send through Proton's encrypted API.

## Adding an account

Open **Settings → Mail accounts → Add account**, or say *"add a mail
account"*. Enter:

- **Email or username** — your Proton login.
- **Password** — used once to run an SRP login. It is never transmitted in
  the clear and is not stored unless persistence is enabled (see below).
- **Mailbox password** — only for two-password-mode accounts. Leave blank
  otherwise.
- **2FA code** — only if your account has TOTP enabled.

On success you are shown a one-time **bridge password**. Configure your mail
client (or the bridge front-ends) with your username and that bridge
password. web2qml never stores the bridge password — it is what unseals your
saved credentials on each connection.

## Multiple accounts

Each account is isolated: its credentials live in their own file, sealed
under their own key. Add as many as you like; remove one from the same
settings pane without affecting the others.

## Voice commands

- *"open my last couple emails from bob"* — opens the most recent matching
  messages, each in its own pane.
- *"show unread mail"*, *"open my inbox"*, *"open my drafts"*.
- *"compose an email to alice@proton.me"*.
- *"search my email for invoice"*.

The inbox pane also offers a **niftyplot** view of your senders.

## Security model

This bridge deliberately hardens beyond the reference implementation:

- Secrets (passwords, keys, tokens) are held in **locked, non-swappable,
  non-dumpable memory** and zeroed on drop.
- Credentials are **only written to disk when persistence is enabled** — the
  same opt-in that governs voice learning. With it off (the hardened
  default), accounts live for the session only and nothing is persisted.
- When credentials are persisted, they are additionally bound to **this
  device** via a local machine key, so a copied credential file cannot be
  opened elsewhere even with the bridge password.
"#;

const STORAGE_MANPAGE: &str = r#"# mail-storage — where mail data lives and how to manage it

## Locations

All bridge state lives under `~/.config/web2qml/mail/`:

- `accounts/<id>.acct` — one sealed file per account (credentials, tokens,
  key salts). Each is encrypted under its own bridge key.
- `machine.key` — a per-device random key (mode `0600`) mixed into every
  account file's encryption, so copies of an `.acct` file cannot be opened on
  another machine.

Nothing here is written at all unless **persistence** is enabled (see
*Security model* in the `mail` page). With persistence off, accounts exist
only in memory for the session.

## Inspecting usage

Settings shows total disk usage and per-account size. Programmatically the
store exposes `disk_usage()` and `account_files()` (id + bytes); neither
decrypts anything.

## Removing data

- **Remove one account** — Settings → Mail accounts → Remove. Deletes that
  one `.acct` file; others are unaffected.
- **Wipe all accounts** — removes every `.acct` file. To fully reset the
  bridge so no previously-saved file could ever be reopened, also delete
  `machine.key`; after that, even a restored `.acct` backup is undecryptable.

## Backups

An `.acct` file is portable only *with* the same machine, since opening it
needs both the bridge password and that device's `machine.key`. To migrate an
account to a new machine, re-add it there with a fresh login rather than
copying files.
"#;

const EMAILS_MANPAGE: &str = r#"# email — reading, acting, and voice control

## Panes

- **Inbox** — your messages, newest first, with an unread dot. The top of the
  pane offers a **niftyplot** sender graph (who's been mailing you, with
  unread counts).
- **Message view** — full message with decrypted body and Reply / Forward.
- **Compose** — To / Subject / Body with Send and Save-draft.
- **Drafts** — edit or send saved drafts.
- **Attachments** — a list plus a niftyplot graph of a message's
  attachments; across several messages it uses the *merge format*, collapsing
  the same file (matched by name + size) into one node so shared/duplicated
  attachments are obvious.

## Notifications

A bottom notifier bar shows newly-arrived mail; tap a chip to open it in a
tab. Optional desktop notifications go through `notify-send` (KDE / any
freedesktop daemon). In high-security mode, desktop notifications show only
"New mail" — never the sender or subject.

## Voice vocabulary

Reading:

- *"open my inbox"*, *"show unread mail"*, *"open my drafts"*
- *"open my last couple emails from bob"*, *"read the last email from alice
  about the invoice"*
- *"search my email for invoice"*

Date-aware and question forms:

- *"what is the email that alice sent me yesterday?"*
- *"show all my emails today about fire"* (opens a tab per match)
- *"open my emails from the last 3 days"*, *"emails from this week"*,
  *"messages from bob last month"*

Time words understood: *today*, *yesterday*, *this/last week*, *this
month*, *last/past month*, *the last N days*. A plural request scoped by a
date or subject (*"my emails today about X"*) opens every match, capped so it
can't flood you with tabs.

Acting:

- *"mark this email as read"* / *"mark the last email from bob unread"*
- *"archive the last email from bob"*
- *"delete the last email from alice"* (moves to Trash)
- *"star the email from bob"* / *"unstar ..."*
- *"reply to the last email from bob"* (opens a pre-addressed compose pane)
- *"show me the attachments from bob"*

Counts understand words and digits: *a*/*one* = 1, *a couple* = 2,
*a few*/*several* = 3, or any number (*"open my last 5 emails"*).
"#;

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

/// A labelled text field. `secret` flips echo mode for passwords.
fn field(o: &mut String, style: &PaneStyle, name: &str, placeholder: &str, secret: bool, value: &str) {
    let echo = if secret {
        "echoMode: TextInput.Password\n"
    } else {
        ""
    };
    let _ = std::fmt::Write::write_fmt(
        o,
        format_args!(
            "TextField {{ objectName: \"{name}\"; width: parent.width; placeholderText: \"{ph}\"; text: \"{val}\"; {echo}color: \"{fg}\"; font.pixelSize: {fs}; font.family: \"{ff}\"; selectByMouse: true }}\n",
            name = qe(name),
            ph = qe(placeholder),
            val = qe(value),
            echo = echo,
            fg = qe(&style.fg),
            fs = style.font_px,
            ff = qe(&style.font_family),
        ),
    );
}

/// QML id-safe slug.
fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() || out.chars().next().unwrap().is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_account::AccountId;

    fn summary(id: &str, user: &str, name: &str) -> AccountSummary {
        AccountSummary {
            id: AccountId::new(id),
            username: user.into(),
            display_name: name.into(),
            color: String::new(),
            updated_at: 0,
        }
    }

    #[test]
    fn login_page_has_fields_and_submit() {
        let qml = login_page_qml(&PaneStyle::dark_default(), None);
        assert!(qml.contains("objectName: \"login_user\""));
        assert!(qml.contains("objectName: \"login_pass\""));
        assert!(qml.contains("echoMode: TextInput.Password"));
        assert!(qml.contains("backend.mail_login("));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count(), "{qml}");
    }

    #[test]
    fn login_page_shows_error() {
        let qml = login_page_qml(&PaneStyle::dark_default(), Some("Two-factor code required"));
        assert!(qml.contains("Two-factor code required"));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count());
    }

    #[test]
    fn accounts_pane_lists_and_removes() {
        let accts = vec![summary("user-1", "alice@proton.me", "Alice"), summary("user-2", "bob@proton.me", "")];
        let qml = accounts_settings_qml(&PaneStyle::dark_default(), &accts);
        assert!(qml.contains("Alice"));
        assert!(qml.contains("bob@proton.me"));
        assert!(qml.contains("mail_remove_account(\"user-1\")"));
        assert!(qml.contains("mail_open_login()"));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count(), "{qml}");
    }

    #[test]
    fn empty_accounts_pane_balanced() {
        let qml = accounts_settings_qml(&PaneStyle::dark_default(), &[]);
        assert!(qml.contains("No accounts yet"));
        assert_eq!(qml.matches('{').count(), qml.matches('}').count());
    }

    #[test]
    fn manpage_mentions_security_and_voice() {
        let md = settings_manpage_md();
        assert!(md.contains("# mail"));
        assert!(md.contains("non-swappable"));
        assert!(md.to_lowercase().contains("niftyplot"));
    }

    #[test]
    fn storage_and_email_manpages_present() {
        assert!(storage_manpage_md().contains("machine.key"));
        assert!(storage_manpage_md().contains("disk_usage"));
        let e = emails_manpage_md();
        assert!(e.contains("merge format"));
        assert!(e.to_lowercase().contains("archive"));
        assert!(e.to_lowercase().contains("notify-send"));
    }
}
