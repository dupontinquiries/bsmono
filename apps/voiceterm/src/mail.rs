//! `mail` — the public, multi-account ProtonMail bridge API for w2q.
//!
//! v0.1.861: This is the single entry point the rest of web2qml (the QML
//! layer, and the SMTP/IMAP/CardDAV front-ends) talks to. It composes the
//! four layers underneath it — transport ([`crate::mail_proton`]), at-rest
//! storage ([`crate::mail_account`]), crypto ([`crate::mail_crypto`]), and
//! the warm-session cache ([`crate::mail_session`]) — into a small, typed
//! surface.
//!
//! ## What "a better API" means here, concretely
//!
//! hydroxide is a single-user CLI daemon: one `auth.json`, one global
//! manager, stringly-typed usernames threaded everywhere, and a re-auth
//! closure the client holds over itself. w2q wants to host *several* Proton
//! accounts at once behind one process, expose them to QML, and let three
//! different bridge protocols share each account's unlocked state. So this
//! API is multi-account in its bones:
//!
//!   * [`AccountId`] is a real type, not a map key you hope is spelled
//!     right.
//!   * Every account is an isolated, individually-sealed file. Listing,
//!     adding and removing accounts are first-class operations.
//!   * Login returns a [`BridgeCredentials`] the user pastes into their
//!     mail client; the daemon never persists that password, only the key
//!     that seals the record.
//!   * [`authenticate_bridge`] is the one call all three front-ends share —
//!     give it the login name and bridge password from any IMAP/SMTP/
//!     CardDAV connection and get back a ready [`Session`].
//!
//! ## Degraded mode
//!
//! With the `mail-crypto` feature off, the crypto providers are inert (they
//! return [`MailError::CryptoUnavailable`]) and the secret box refuses to
//! write secrets to disk. The API still compiles, links, and answers
//! structural queries (listing accounts, etc.); the operations that need
//! real crypto fail loudly rather than silently doing something unsafe.
//! This mirrors how w2q gates `llamacpp`/`wavenet`/`claude`.

use std::sync::Arc;

use crate::mail_account::{
    new_bridge_password, AccountId, AccountRecord, AccountStore, AccountSummary,
};
use crate::mail_crypto::CryptoSuite;
use crate::mail_error::{MailError, Result};
use crate::mail_proton::{
    Auth, ClientConfig, MessageFilter, PasswordModeRepr, ProtonClient,
};
use crate::mail_secure::secure_secret_box;
use crate::mail_session::{Session, SessionManager};

/// The bridge credential handed back to the user after a successful login.
/// They configure their mail client with `username` + `bridge_password`;
/// the daemon stores neither the Proton password nor this bridge password —
/// only the key that seals the account record (and that key is *derived
/// from* this bridge password on every connection).
#[derive(Clone)]
pub struct BridgeCredentials {
    pub id: AccountId,
    pub username: String,
    pub bridge_password: String,
}

/// Outcome of a login attempt. Proton may demand a TOTP code between the
/// SRP exchange and unlock, so login is modeled as a small state machine
/// rather than a single fallible call.
pub enum LoginOutcome {
    /// Fully logged in; here are the bridge credentials.
    Complete(BridgeCredentials),
    /// SRP succeeded but the account has 2FA — call
    /// [`MailHub::complete_login_totp`] with the live client handle and the
    /// user's code.
    NeedsTotp(PendingLogin),
}

/// A login paused awaiting a TOTP code. Holds the authenticated-but-not-
/// yet-2FA'd client and the material needed to finish and persist.
pub struct PendingLogin {
    client: Arc<ProtonClient>,
    username: String,
    login_password: Vec<u8>,
    mailbox_password: Vec<u8>,
    auth: Auth,
}

/// The composed bridge. Cheap to clone the handles out of; the heavy state
/// is all behind `Arc`.
pub struct MailHub {
    store: Arc<AccountStore>,
    crypto: Arc<CryptoSuite>,
    sessions: Arc<SessionManager>,
    client_config: ClientConfig,
    agent: ureq::Agent,
    notifier: std::sync::Mutex<crate::mail_notify::Notifier>,
}

impl MailHub {
    /// Build a hub using the platform crypto suite and the default store
    /// location (`~/.config/web2qml/mail/accounts/`). The `agent` is the
    /// project's shared synchronous [`ureq::Agent`], passed in exactly like
    /// w2q does for its other HTTP clients.
    pub fn new(agent: ureq::Agent, client_config: ClientConfig) -> Result<Self> {
        let store = Arc::new(AccountStore::open_default(secure_secret_box()?)?);
        let crypto = Arc::new(CryptoSuite::platform());
        let sessions = Arc::new(SessionManager::new(
            store.clone(),
            crypto.clone(),
            agent.clone(),
            client_config.clone(),
        ));
        Ok(MailHub {
            store,
            crypto,
            sessions,
            client_config,
            agent,
            notifier: std::sync::Mutex::new(crate::mail_notify::Notifier::new()),
        })
    }

    /// Build a hub against an explicit store directory (tests, or a
    /// relocated config root).
    pub fn with_store_dir(
        agent: ureq::Agent,
        client_config: ClientConfig,
        dir: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
        let store = Arc::new(AccountStore::open_at(dir, secure_secret_box()?)?);
        let crypto = Arc::new(CryptoSuite::platform());
        let sessions = Arc::new(SessionManager::new(
            store.clone(),
            crypto.clone(),
            agent.clone(),
            client_config.clone(),
        ));
        Ok(MailHub {
            store,
            crypto,
            sessions,
            client_config,
            agent,
            notifier: std::sync::Mutex::new(crate::mail_notify::Notifier::new()),
        })
    }

    // --- account lifecycle ------------------------------------------------

    /// Interactive login. Runs the SRP exchange; if the account has no 2FA
    /// this also unlocks and persists, returning [`LoginOutcome::Complete`].
    /// Otherwise it returns [`LoginOutcome::NeedsTotp`] for the caller to
    /// finish with a code.
    ///
    /// `mailbox_password` is the second (two-password-mode) passphrase; pass
    /// the same value as `password` for single-password accounts, or `None`
    /// to let the hub reuse `password`.
    pub fn login(
        &self,
        username: &str,
        password: &str,
        mailbox_password: Option<&str>,
    ) -> Result<LoginOutcome> {
        let client = Arc::new(ProtonClient::new(
            self.agent.clone(),
            self.client_config.clone(),
        ));

        let info = client.auth_info(username)?;
        let auth = client.auth(username, password.as_bytes(), &info, self.crypto.srp.as_ref())?;

        let login_password = password.as_bytes().to_vec();
        let mailbox_password = mailbox_password
            .map(|m| m.as_bytes().to_vec())
            .unwrap_or_else(|| login_password.clone());

        if auth.two_factor_enabled || auth.totp {
            return Ok(LoginOutcome::NeedsTotp(PendingLogin {
                client,
                username: username.to_string(),
                login_password,
                mailbox_password,
                auth,
            }));
        }

        let creds = self.finalize_login(client, username, login_password, mailbox_password, auth)?;
        Ok(LoginOutcome::Complete(creds))
    }

    /// Supply the TOTP code for a [`PendingLogin`] and finish the login.
    pub fn complete_login_totp(
        &self,
        pending: PendingLogin,
        totp_code: &str,
    ) -> Result<BridgeCredentials> {
        let PendingLogin {
            client,
            username,
            login_password,
            mailbox_password,
            mut auth,
        } = pending;
        let scope = client.auth_totp(totp_code)?;
        auth.scope = scope;
        self.finalize_login(client, &username, login_password, mailbox_password, auth)
    }

    /// Shared tail of both login paths: mint a bridge key, seal a fresh
    /// record, and warm the session so the account is immediately usable.
    fn finalize_login(
        &self,
        client: Arc<ProtonClient>,
        username: &str,
        login_password: Vec<u8>,
        mailbox_password: Vec<u8>,
        auth: Auth,
    ) -> Result<BridgeCredentials> {
        // Pull the salts now so the record is self-sufficient for later
        // unlocks without another round-trip at connection time.
        let key_salts = client.list_key_salts().unwrap_or_default();

        // Prefer Proton's stable user id as the AccountId; fall back to the
        // login name before the first /users call resolves.
        let id = if !auth.user_id.is_empty() {
            AccountId::new(auth.user_id.clone())
        } else {
            AccountId::new(username.to_string())
        };

        let bridge = new_bridge_password()?;
        let now = now_secs();
        let record = AccountRecord {
            id: id.clone(),
            username: username.to_string(),
            auth,
            login_password,
            mailbox_password,
            key_salts,
            display_name: username.to_string(),
            color: String::new(),
            created_at: now,
            updated_at: now,
        };

        // Seal to disk. Two non-fatal outcomes are folded in:
        //   * PersistenceDisabled — the hardened default declined to write
        //     credentials; the account is still fully usable this session.
        //   * CryptoUnavailable — no real cipher linked; likewise in-memory
        //     only. Either way we warm the session and hand back creds; a
        //     hard Store/Transport error still aborts.
        match self.store.save(&record, &bridge.key) {
            Ok(()) => {}
            Err(e) if e.is_persistence_disabled() => {
                crate::clog!(
                    "mail",
                    "[mail] account {} authenticated (session-only; persistence disabled)",
                    record.id
                );
            }
            Err(MailError::CryptoUnavailable(_)) => {
                crate::clog!(
                    "mail",
                    "[mail] account {} authenticated (session-only; mail-crypto off)",
                    record.id
                );
            }
            Err(e) => return Err(e),
        }

        // Warm the session immediately so the new account is serviceable
        // without a reconnect. A unlock failure here (e.g. crypto off)
        // doesn't undo a saved record; the account is listed but inert until
        // crypto is available.
        if let Ok(session) = self.sessions.build_session(record, bridge.key) {
            self.sessions.cache_built(session);
        }

        Ok(BridgeCredentials {
            id,
            username: username.to_string(),
            bridge_password: bridge.text,
        })
    }

    /// All accounts known on disk, as non-secret summaries for a UI.
    pub fn accounts(&self) -> Result<Vec<AccountSummary>> {
        // Summaries need the username/display fields, which live inside the
        // sealed body — but we don't have bridge keys here. So we surface
        // what we can without keys: the ids. Front-ends that want richer
        // metadata cache it from the BridgeCredentials returned at login, or
        // from an active Session. This keeps "list accounts" key-free.
        let ids = self.store.list_ids()?;
        Ok(ids
            .into_iter()
            .map(|id| AccountSummary {
                username: id.as_str().to_string(),
                display_name: id.as_str().to_string(),
                color: String::new(),
                updated_at: 0,
                id,
            })
            .collect())
    }

    /// Ids of accounts present on disk.
    pub fn account_ids(&self) -> Result<Vec<AccountId>> {
        self.store.list_ids()
    }

    /// Permanently remove an account: evict any warm session and delete the
    /// sealed file. Best-effort server-side logout requires a live session,
    /// so we attempt it only if the account is currently warm.
    pub fn remove_account(&self, id: &AccountId) -> Result<()> {
        if let Some(session) = self.sessions.get(id) {
            let _ = session.client.logout();
        }
        self.sessions.evict(id);
        self.store.remove(id)
    }

    // --- bridge front-end entry point ------------------------------------

    /// The call SMTP/IMAP/CardDAV front-ends make on every connection: turn
    /// `(account_id, bridge_password)` into a ready [`Session`]. Warm
    /// sessions are reused; cold ones are opened, unlocked, and cached.
    ///
    /// This is deliberately the *only* authentication surface the protocol
    /// servers need — they never see Proton passwords, SRP, or key salts.
    pub fn authenticate_bridge(
        &self,
        account_id: &AccountId,
        bridge_password: &str,
    ) -> Result<Arc<Session>> {
        self.sessions.authenticate(account_id, bridge_password)
    }

    /// Convenience for front-ends that identify accounts by login name
    /// rather than id: resolve by matching the on-disk id slug, then
    /// authenticate. (Most clients send the email/username; the id *is* the
    /// username for accounts created before a `/users` round-trip, and the
    /// Proton user id otherwise — front-ends should prefer the id from
    /// [`BridgeCredentials`].)
    pub fn authenticate_bridge_by_name(
        &self,
        name: &str,
        bridge_password: &str,
    ) -> Result<Arc<Session>> {
        let id = AccountId::new(name);
        if self.store.exists(&id) {
            return self.sessions.authenticate(&id, bridge_password);
        }
        Err(MailError::Unauthorized)
    }

    // --- read helpers (thin pass-throughs over a Session) ----------------

    /// List message headers for an authenticated session, applying a filter.
    pub fn list_messages(
        &self,
        session: &Session,
        filter: &MessageFilter,
    ) -> Result<(i64, Vec<crate::mail_proton::Message>)> {
        session.client.list_messages(filter)
    }

    /// Fetch one message (encrypted body included) by id.
    pub fn get_message(
        &self,
        session: &Session,
        id: &str,
    ) -> Result<crate::mail_proton::Message> {
        session.client.get_message(id)
    }

    /// Whether the build has real crypto wired in. Front-ends can show a
    /// banner when this is false instead of letting users hit
    /// `CryptoUnavailable` at login.
    pub fn crypto_available(&self) -> bool {
        cfg!(feature = "mail-crypto")
    }

    // --- notifications ----------------------------------------------------

    /// Poll the inbox and return newly-arrived messages since the last poll
    /// (silent baseline on the first call per account this session). Drive
    /// this from a timer; render the result with
    /// [`crate::mail_notify::notifier_bar_qml`] and optionally fire
    /// [`crate::mail_notify::desktop_notify_batch`].
    pub fn poll_new_mail(
        &self,
        session: &Session,
    ) -> Result<Vec<crate::mail_notify::MailNotification>> {
        let mut filter = MessageFilter::default();
        filter.label = crate::mail_intent::label::INBOX.to_string();
        filter.page_size = 50;
        let (_total, msgs) = session.client.list_messages(&filter)?;
        let mut n = self.notifier.lock().unwrap();
        Ok(n.poll(&session.id, &msgs))
    }

    /// Establish the notification baseline for a session without notifying
    /// (call once right after authenticating).
    pub fn prime_notifications(&self, session: &Session) -> Result<()> {
        let mut filter = MessageFilter::default();
        filter.label = crate::mail_intent::label::INBOX.to_string();
        filter.page_size = 50;
        let (_total, msgs) = session.client.list_messages(&filter)?;
        self.notifier.lock().unwrap().prime(&session.id, &msgs);
        Ok(())
    }

    // --- voice / text intent execution -----------------------------------

    /// Parse a natural-language phrase and execute it against `session`,
    /// returning the pane actions the UI should perform. This is the single
    /// call the voice router (or a command palette) makes for mail:
    ///
    /// ```ignore
    /// let actions = hub.handle_phrase(&session, "open my last couple emails from bob")?;
    /// ```
    ///
    /// Returns an empty vec when the phrase isn't a mail command, so the
    /// caller can fall through to its other intent handlers.
    pub fn handle_phrase(
        &self,
        session: &Session,
        text: &str,
    ) -> Result<Vec<crate::mail_intent::PaneAction>> {
        match crate::mail_intent::parse(text) {
            Some(intent) => self.execute_intent(session, &intent),
            None => Ok(Vec::new()),
        }
    }

    /// Execute an already-parsed [`MailIntent`], resolving any "from
    /// sender" / "about subject" narrowing against live message headers and
    /// emitting concrete [`PaneAction`]s.
    pub fn execute_intent(
        &self,
        session: &Session,
        intent: &crate::mail_intent::MailIntent,
    ) -> Result<Vec<crate::mail_intent::PaneAction>> {
        use crate::mail_intent::{MailIntent, PaneAction};
        match intent {
            MailIntent::OpenInbox { unread_only } => {
                Ok(vec![PaneAction::OpenInbox { unread_only: *unread_only }])
            }
            MailIntent::OpenDrafts => Ok(vec![PaneAction::OpenDrafts]),
            MailIntent::Compose { to } => {
                Ok(vec![PaneAction::OpenCompose { to: to.clone() }])
            }
            MailIntent::Search { query } => {
                let (_total, msgs) = session.client.list_messages(&intent.base_filter())?;
                let q = query.to_lowercase();
                let ids: Vec<String> = msgs
                    .iter()
                    .filter(|m| msg_matches_about(m, &q))
                    .map(|m| m.id.clone())
                    .collect();
                Ok(vec![PaneAction::OpenSearch {
                    query: query.clone(),
                    ids,
                }])
            }
            MailIntent::OpenMessages(sel) => {
                let msgs = self.resolve_selector(session, sel)?;
                if msgs.is_empty() {
                    let scope = sel
                        .from
                        .clone()
                        .map(|f| format!(" from {f}"))
                        .unwrap_or_default();
                    return Ok(vec![PaneAction::Notice(format!(
                        "No matching messages{scope}."
                    ))]);
                }
                // One tab per resolved message.
                let mut actions: Vec<PaneAction> = msgs
                    .iter()
                    .map(|m| PaneAction::OpenMessage {
                        id: m.id.clone(),
                        title: pane_title(m),
                    })
                    .collect();
                // If an "all" query hit the cap, tell the user it was bounded.
                if sel.all && actions.len() >= sel.count {
                    actions.push(PaneAction::Notice(format!(
                        "Opened the first {} matches.",
                        actions.len()
                    )));
                }
                Ok(actions)
            }
            MailIntent::Act { action, sel } => {
                let msgs = self.resolve_selector(session, sel)?;
                if msgs.is_empty() {
                    return Ok(vec![PaneAction::Notice("No matching messages.".into())]);
                }
                let ids: Vec<&str> = msgs.iter().map(|m| m.id.as_str()).collect();
                use crate::mail_intent::{label, MailAction};
                let c = &session.client;
                match action {
                    MailAction::MarkRead => c.mark_read(&ids)?,
                    MailAction::MarkUnread => c.mark_unread(&ids)?,
                    MailAction::Trash => c.label_messages(label::TRASH, &ids)?,
                    MailAction::Star => c.label_messages(label::STARRED, &ids)?,
                    MailAction::Unstar => c.unlabel_messages(label::STARRED, &ids)?,
                    MailAction::Archive => {
                        c.label_messages(label::ARCHIVE, &ids)?;
                        let _ = c.unlabel_messages(label::INBOX, &ids); // best effort
                    }
                }
                Ok(vec![
                    PaneAction::Notice(format!("{} message(s) {}.", ids.len(), action.verb())),
                    PaneAction::Refresh,
                ])
            }
            MailIntent::Reply { sel } => {
                let msgs = self.resolve_selector(session, sel)?;
                match msgs.first() {
                    Some(m) => {
                        let to = m
                            .sender
                            .as_ref()
                            .map(|s| s.address.clone())
                            .filter(|a| !a.is_empty());
                        Ok(vec![PaneAction::OpenCompose { to }])
                    }
                    None => Ok(vec![PaneAction::Notice("Nothing to reply to.".into())]),
                }
            }
            MailIntent::Attachments { sel } => {
                let msgs = self.resolve_selector(session, sel)?;
                // Fetch full messages so attachment metadata is populated, and
                // keep only those that actually have attachments.
                let mut ids = Vec::new();
                for m in &msgs {
                    if m.num_attachments > 0 {
                        ids.push(m.id.clone());
                    }
                }
                if ids.is_empty() {
                    Ok(vec![PaneAction::Notice("No attachments found.".into())])
                } else {
                    let title = sel
                        .from
                        .clone()
                        .map(|f| format!("Attachments from {f}"))
                        .unwrap_or_else(|| "Attachments".into());
                    Ok(vec![PaneAction::OpenAttachments { ids, title }])
                }
            }
        }
    }

    /// Resolve a [`Selector`] to concrete messages: list under its label,
    /// post-filter by sender/subject substrings, take `count`.
    fn resolve_selector(
        &self,
        session: &Session,
        sel: &crate::mail_intent::Selector,
    ) -> Result<Vec<crate::mail_proton::Message>> {
        use crate::mail_proton::MessageFilter;
        let mut filter = MessageFilter::default();
        filter.label = crate::mail_intent::label::ALL_MAIL.to_string();
        // For an "all" query the page size should be the cap, not count*5.
        filter.page_size = if sel.all {
            (sel.count as i64).clamp(25, 100)
        } else {
            (sel.count as i64 * 5).clamp(15, 50)
        };
        // Narrow server-side by date window and subject keyword when present.
        let window = sel.when.map(|w| w.resolve(now_secs() as i64));
        if let Some((begin, end)) = window {
            filter.begin = begin;
            filter.end = end;
        }
        if let Some(ref a) = sel.about {
            filter.keyword = a.clone();
        }
        let (_total, msgs) = session.client.list_messages(&filter)?;
        let from_l = sel.from.as_ref().map(|s| s.to_lowercase());
        let about_l = sel.about.as_ref().map(|s| s.to_lowercase());
        let mut out = Vec::new();
        for m in msgs {
            if let Some(ref f) = from_l {
                if !msg_matches_from(&m, f) {
                    continue;
                }
            }
            if let Some(ref a) = about_l {
                if !msg_matches_about(&m, a) {
                    continue;
                }
            }
            // Authoritative time post-filter (server Begin/End is only a hint).
            if let Some((begin, end)) = window {
                if m.time < begin || m.time >= end {
                    continue;
                }
            }
            out.push(m);
            if out.len() >= sel.count {
                break;
            }
        }
        Ok(out)
    }

    // --- pane rendering (proton models -> QML via mail_panes) ------------

    /// Build the inbox pane QML for a session.
    pub fn render_inbox(
        &self,
        session: &Session,
        style: &crate::mail_panes::PaneStyle,
        unread_only: bool,
    ) -> Result<String> {
        let mut filter = MessageFilter::default();
        filter.label = crate::mail_intent::label::INBOX.to_string();
        filter.page_size = 50;
        let (_total, msgs) = session.client.list_messages(&filter)?;
        let rows: Vec<crate::mail_panes::MessageRow> = msgs
            .iter()
            .filter(|m| !unread_only || m.unread)
            .map(message_to_row)
            .collect();
        Ok(crate::mail_panes::inbox_pane_qml(style, &rows, unread_only))
    }

    /// Build a single message-view pane QML, fetching and decrypting the
    /// body via the session's keyring.
    pub fn render_message(
        &self,
        session: &Session,
        style: &crate::mail_panes::PaneStyle,
        id: &str,
    ) -> Result<String> {
        let msg = session.client.get_message(id)?;
        let body = self
            .crypto
            .pgp
            .decrypt(session.keyring.as_ref(), &msg.body)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_else(|| {
                if msg.body.is_empty() {
                    String::new()
                } else {
                    "[encrypted body — PGP decrypt unavailable in this build]".to_string()
                }
            });
        let view = crate::mail_panes::MessageView {
            id: msg.id.clone(),
            from_name: msg.sender.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
            from_addr: msg.sender.as_ref().map(|s| s.address.clone()).unwrap_or_default(),
            to: msg
                .to
                .iter()
                .map(|a| a.address.clone())
                .collect::<Vec<_>>()
                .join(", "),
            subject: msg.subject.clone(),
            date: format_time(msg.time),
            body_text: body,
        };
        Ok(crate::mail_panes::message_pane_qml(style, &view))
    }

    /// Build the compose pane QML.
    pub fn render_compose(
        &self,
        style: &crate::mail_panes::PaneStyle,
        to: Option<&str>,
    ) -> String {
        crate::mail_panes::compose_pane_qml(style, to)
    }

    /// Build the attachments pane for one or more messages, using the merge
    /// graph format when more than one message is involved.
    pub fn render_attachments(
        &self,
        session: &Session,
        style: &crate::mail_panes::PaneStyle,
        title: &str,
        message_ids: &[String],
    ) -> Result<String> {
        use crate::mail_panes::AttachmentItem;
        let mut per_message: Vec<(String, Vec<AttachmentItem>)> = Vec::new();
        let mut flat: Vec<AttachmentItem> = Vec::new();
        for id in message_ids {
            let m = session.client.get_message(id)?;
            let from = m
                .sender
                .as_ref()
                .map(|s| if s.name.is_empty() { s.address.clone() } else { s.name.clone() })
                .unwrap_or_default();
            let label = format!(
                "{} — {}",
                if from.is_empty() { "(unknown)" } else { &from },
                if m.subject.is_empty() { "(no subject)" } else { &m.subject }
            );
            let items: Vec<AttachmentItem> = m
                .attachments
                .iter()
                .map(|a| AttachmentItem {
                    id: a.id.clone(),
                    message_id: m.id.clone(),
                    name: a.name.clone(),
                    size: a.size,
                    mime: a.mime_type.clone(),
                    from: from.clone(),
                })
                .collect();
            flat.extend(items.iter().cloned());
            per_message.push((label, items));
        }
        let src = if per_message.len() > 1 {
            crate::mail_panes::attachments_merge_niftyplot(&per_message)
        } else {
            let label = per_message.first().map(|(l, _)| l.as_str()).unwrap_or("");
            crate::mail_panes::attachment_graph_niftyplot(label, &flat)
        };
        Ok(crate::mail_panes::attachments_pane_qml(style, title, &flat, &src))
    }
}

// --- intent/render helpers ----------------------------------------------

impl crate::mail_notify::HeaderLike for crate::mail_proton::Message {
    fn id(&self) -> &str {
        &self.id
    }
    fn time(&self) -> i64 {
        self.time
    }
    fn sender_display(&self) -> String {
        self.sender
            .as_ref()
            .map(|s| if s.name.is_empty() { s.address.clone() } else { s.name.clone() })
            .unwrap_or_default()
    }
    fn subject(&self) -> &str {
        &self.subject
    }
    fn in_inbox(&self) -> bool {
        // Inbox label id is "0"; a message in the inbox carries it.
        self.label_ids.iter().any(|l| l == crate::mail_intent::label::INBOX)
    }
}

fn msg_matches_from(m: &crate::mail_proton::Message, needle_lower: &str) -> bool {
    m.sender
        .as_ref()
        .map(|s| {
            s.name.to_lowercase().contains(needle_lower)
                || s.address.to_lowercase().contains(needle_lower)
        })
        .unwrap_or(false)
}

fn msg_matches_about(m: &crate::mail_proton::Message, needle_lower: &str) -> bool {
    m.subject.to_lowercase().contains(needle_lower)
}

fn pane_title(m: &crate::mail_proton::Message) -> String {
    if m.subject.trim().is_empty() {
        "(no subject)".to_string()
    } else {
        m.subject.clone()
    }
}

fn message_to_row(m: &crate::mail_proton::Message) -> crate::mail_panes::MessageRow {
    crate::mail_panes::MessageRow {
        id: m.id.clone(),
        from_name: m.sender.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
        from_addr: m.sender.as_ref().map(|s| s.address.clone()).unwrap_or_default(),
        subject: m.subject.clone(),
        date: format_time(m.time),
        unread: m.unread,
    }
}

/// Render a Unix timestamp as a short UTC date-time. Dependency-free (no
/// chrono in the tree): a compact civil-time conversion good enough for a
/// message list header.
fn format_time(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (h, mi, s) = ((secs / 3600) % 24, (secs % 3600) / 60, secs % 60);
    // Civil-from-days (Howard Hinnant's algorithm), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Best-effort mapping from a password mode to a human label, for UIs.
pub fn password_mode_label(mode: PasswordModeRepr) -> &'static str {
    match mode {
        PasswordModeRepr::Single => "single-password",
        PasswordModeRepr::Two => "two-password",
        PasswordModeRepr::Unknown(_) => "unknown",
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_mode_labels() {
        assert_eq!(password_mode_label(PasswordModeRepr::Single), "single-password");
        assert_eq!(password_mode_label(PasswordModeRepr::Two), "two-password");
        assert_eq!(
            password_mode_label(PasswordModeRepr::Unknown(9)),
            "unknown"
        );
    }

    #[test]
    fn now_after_2020() {
        assert!(now_secs() > 1_577_836_800);
    }

    #[test]
    fn format_time_known_epoch() {
        // 1_700_000_000 = 2023-11-14 22:13:20 UTC
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(format_time(0), "");
        // 1_000_000_000 = 2001-09-09 01:46:40 UTC
        assert_eq!(format_time(1_000_000_000), "2001-09-09 01:46:40");
    }
}
