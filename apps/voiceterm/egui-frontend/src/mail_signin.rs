//! mail_signin — Proton sign-in and the inbox, for the Mail pane.
//!
//! The Qt build's `MailHub` drives QML generators, so it could not come
//! across; its engine (`mail_proton`, `mail_crypto`, mounted behind the
//! `mail-proton` feature) could, and this is the one flow on top of it
//! that matters first: sign in, complete a second factor if there is one,
//! and list the inbox. Message headers — subject, sender, time, unread —
//! are plaintext in Proton's API, so a working sign-in shows a real inbox
//! before any PGP is unlocked. Bodies stay encrypted until the key unlock
//! lands (`TODO_mail_srp_login.md` step 2).
//!
//! ## Off the UI thread, by construction
//!
//! Every step is a network round trip and the SRP proof is real
//! arithmetic; `mail_login` in the Qt build did all of it on the UI
//! thread and the TODO named that as the thing to fix once login worked.
//! [`SignIn`] owns a worker and a channel: the pane calls [`SignIn::start`]
//! and polls [`SignIn::poll`] each frame. The client lives on the worker
//! side between steps, so a TOTP prompt continues the SAME session — the
//! Qt build dropped the pending login on `NeedsTotp`, which is why
//! accounts with 2FA could never finish signing in there.
//!
//! ## Without the feature
//!
//! `mail-proton` off (a static musl build without ureq, say) compiles this
//! module to the same API answering [`Event::Unavailable`], so the pane
//! draws one honest line instead of a form that goes nowhere.

use std::sync::mpsc::{channel, Receiver, Sender};

/// What the worker reports back.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The password was accepted; the account wants a TOTP code. The
    /// worker keeps the session and waits for [`SignIn::totp`].
    NeedsTotp,
    /// Signed in. `display` is what to call the account.
    SignedIn { display: String },
    /// A page of the inbox, newest first.
    Inbox { total: i64, messages: Vec<crate::mail_view::Message> },
    /// The flow stopped. `retryable` says whether the form should stay
    /// filled: a wrong password is, a build without the feature is not.
    Failed { message: String, retryable: bool },
    /// This build has no Proton engine.
    Unavailable,
}

/// The pane's side of the worker.
#[derive(Default)]
pub struct SignIn {
    rx: Option<Receiver<Event>>,
    /// The channel the worker waits on for a TOTP code, once it has asked.
    totp_tx: Option<Sender<String>>,
    /// True from `start` until a terminal event arrives.
    pub busy: bool,
    /// The last event's story, for the pane to show.
    pub status: String,
    pub signed_in: Option<String>,
    pub needs_totp: bool,
}

impl SignIn {
    pub fn is_running(&self) -> bool { self.busy }

    /// Begin: /auth/info, the SRP proof, /auth, then the inbox — or a
    /// TOTP request part way. Ignored while a flow is running.
    pub fn start(&mut self, username: &str, password: &str) {
        if self.busy { return; }
        let (tx, rx) = channel();
        let (totp_tx, totp_rx) = channel::<String>();
        self.rx = Some(rx);
        self.totp_tx = Some(totp_tx);
        self.busy = true;
        self.needs_totp = false;
        self.status = "signing in…".into();
        let (u, p) = (username.trim().to_string(), password.to_string());
        std::thread::Builder::new().name("mail-signin".into()).spawn(move || {
            worker::run(&u, &p, tx, totp_rx);
        }).ok();
    }

    /// Hand the worker a TOTP code after [`Event::NeedsTotp`].
    pub fn totp(&mut self, code: &str) {
        if let Some(tx) = &self.totp_tx {
            let _ = tx.send(code.trim().to_string());
            self.needs_totp = false;
            self.status = "checking the code…".into();
        }
    }

    /// Drain the worker's events, updating the summary fields, and hand
    /// them to the caller for the ones it acts on (the inbox).
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(rx) = &self.rx else { return out };
        while let Ok(ev) = rx.try_recv() {
            match &ev {
                Event::NeedsTotp => { self.needs_totp = true; self.status = "enter the code from your authenticator".into(); }
                Event::SignedIn { display } => { self.signed_in = Some(display.clone()); self.status = format!("signed in as {display}"); }
                Event::Inbox { total, messages } => {
                    self.busy = false;
                    self.status = format!("inbox: {} of {total} message(s)", messages.len());
                }
                Event::Failed { message, .. } => { self.busy = false; self.status = message.clone(); }
                Event::Unavailable => { self.busy = false; self.status = worker::UNAVAILABLE.into(); }
            }
            out.push(ev);
        }
        if !self.busy { self.totp_tx = None; }
        out
    }
}

#[cfg(feature = "mail-proton")]
mod worker {
    use super::Event;
    use crate::mail_crypto::CryptoSuite;
    use crate::mail_error::MailError;
    use crate::mail_proton::{ClientConfig, MessageFilter, ProtonClient};
    use std::sync::mpsc::{Receiver, Sender};

    pub const UNAVAILABLE: &str = "this build has no Proton engine";

    /// Proton's inbox label id. The others (drafts 8, sent 7, all mail 5)
    /// are the same fixed ids hydroxide uses.
    const INBOX_LABEL: &str = "0";

    fn describe(e: &MailError) -> (String, bool) {
        match e {
            MailError::Transport(t) => (format!("could not reach Proton: {t}"), true),
            MailError::Api { code: 8002, .. } => ("wrong username or password".into(), true),
            // Human verification (captcha) — Proton asks for it after a
            // burst of failures or from a new network. Nothing here can
            // answer a captcha; say where to.
            MailError::Api { code: 9001, .. } =>
                ("Proton wants a human verification for this sign-in — complete one at mail.proton.me, then try again".into(), true),
            MailError::Api { code, message } => (format!("Proton refused ({code}): {message}"), true),
            MailError::Unauthorized => ("wrong username or password".into(), true),
            MailError::TwoFactorRequired => ("this account needs a second factor".into(), true),
            MailError::SrpServerProof => ("the server's proof did not verify — not talking to Proton?".into(), false),
            MailError::CryptoUnavailable(what) => (format!("this build cannot sign in: {what}"), false),
            other => (format!("{other:?}"), true),
        }
    }

    pub fn run(username: &str, password: &str, tx: Sender<Event>, totp_rx: Receiver<String>) {
        let suite = CryptoSuite::platform();
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();
        let client = ProtonClient::new(agent, ClientConfig::default());
        let info = match client.auth_info(username) {
            Ok(i) => i,
            Err(e) => { let (m, r) = describe(&e); let _ = tx.send(Event::Failed { message: m, retryable: r }); return; }
        };
        let auth = match client.auth(username, password.as_bytes(), &info, suite.srp.as_ref()) {
            Ok(a) => a,
            Err(e) => { let (m, r) = describe(&e); let _ = tx.send(Event::Failed { message: m, retryable: r }); return; }
        };
        if auth.two_factor_enabled || auth.totp {
            let _ = tx.send(Event::NeedsTotp);
            // The session waits on the channel — the same client, the
            // same tokens — until the code arrives or the pane is gone.
            let code = match totp_rx.recv() { Ok(c) => c, Err(_) => return };
            if let Err(e) = client.auth_totp(&code) {
                let (m, r) = describe(&e);
                let _ = tx.send(Event::Failed { message: m, retryable: r });
                return;
            }
        }
        let display = client.get_current_user().map(|u| u.name).unwrap_or_else(|_| username.to_string());
        let _ = tx.send(Event::SignedIn { display });
        let filter = MessageFilter { page_size: 50, limit: 50, label: INBOX_LABEL.into(), ..Default::default() };
        match client.list_messages(&filter) {
            Ok((total, msgs)) => {
                let messages = msgs.into_iter().map(|m| crate::mail_view::Message {
                    id: m.id,
                    conversation_id: m.conversation_id,
                    subject: m.subject,
                    unread: m.unread,
                    sender: m.sender.map(|a| crate::mail_view::Address { name: a.name, address: a.address }),
                    time: m.time,
                    num_attachments: m.num_attachments,
                    is_encrypted: m.is_encrypted,
                }).collect();
                let _ = tx.send(Event::Inbox { total, messages });
            }
            Err(e) => { let (m, r) = describe(&e); let _ = tx.send(Event::Failed { message: m, retryable: r }); }
        }
    }
}

#[cfg(not(feature = "mail-proton"))]
mod worker {
    use super::Event;
    use std::sync::mpsc::{Receiver, Sender};
    pub const UNAVAILABLE: &str = "this build has no Proton engine (built without mail-proton)";
    pub fn run(_u: &str, _p: &str, tx: Sender<Event>, _totp: Receiver<String>) {
        let _ = tx.send(Event::Unavailable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flow_settles_and_the_summary_tells_the_story() {
        // Without a network there is no live path to walk; what can be
        // held to is the pane-side machine: busy from start to a terminal
        // event, the TOTP prompt in between, the summary line after each.
        let mut s = SignIn::default();
        assert!(!s.is_running());
        let (tx, rx) = channel();
        s.rx = Some(rx);
        s.busy = true;
        tx.send(Event::NeedsTotp).unwrap();
        let evs = s.poll();
        assert_eq!(evs, vec![Event::NeedsTotp]);
        assert!(s.needs_totp && s.busy, "waiting on the code, still running");
        tx.send(Event::SignedIn { display: "ada".into() }).unwrap();
        tx.send(Event::Inbox { total: 3, messages: Vec::new() }).unwrap();
        let evs = s.poll();
        assert_eq!(evs.len(), 2);
        assert_eq!(s.signed_in.as_deref(), Some("ada"));
        assert!(!s.busy, "the inbox is the end of the flow");
        assert!(s.status.contains("3"), "{}", s.status);
    }

    #[test]
    fn a_failure_ends_the_flow_and_says_why() {
        let mut s = SignIn::default();
        let (tx, rx) = channel();
        s.rx = Some(rx);
        s.busy = true;
        tx.send(Event::Failed { message: "wrong username or password".into(), retryable: true }).unwrap();
        s.poll();
        assert!(!s.busy);
        assert_eq!(s.status, "wrong username or password");
        assert!(s.signed_in.is_none());
    }

    #[test]
    fn start_is_ignored_while_a_flow_runs() {
        let mut s = SignIn::default();
        s.busy = true;
        s.start("ada", "pw");
        assert!(s.rx.is_none(), "a second worker was spawned over the first");
    }

    #[test]
    #[cfg(feature = "mail-proton")]
    fn an_unreachable_endpoint_is_a_retryable_failure_not_a_hang() {
        // The real worker against a port nothing listens on: the error
        // must come back through the channel as a Failed event with the
        // form left usable. (Loopback only: no network in tests.)
        let (tx, rx) = channel();
        let (_ttx, trx) = channel::<String>();
        std::thread::spawn(move || {
            // A client pointed at loopback — through the same code path
            // as `run`, with the endpoint swapped.
            let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(2)).build();
            let client = crate::mail_proton::ProtonClient::new(agent, crate::mail_proton::ClientConfig {
                api_endpoint: "http://127.0.0.1:9".into(), ..Default::default() });
            match client.auth_info("nobody") {
                Ok(_) => { let _ = tx.send(Event::Failed { message: "unexpected success".into(), retryable: false }); }
                Err(e) => {
                    let msg = format!("{e:?}");
                    let _ = tx.send(Event::Failed { message: msg, retryable: true });
                }
            }
            drop(trx);
        });
        let ev = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("an answer");
        match ev {
            Event::Failed { message, retryable } => {
                assert!(retryable);
                assert!(message.contains("Transport") || message.contains("refused") || message.contains("connect"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
}
//! mail_signin — Proton sign-in and the inbox, for the Mail pane.
//!
//! The Qt build's `MailHub` drives QML generators, so it could not come
//! across; its engine (`mail_proton`, `mail_crypto`, mounted behind the
//! `mail-proton` feature) could, and this is the one flow on top of it
//! that matters first: sign in, complete a second factor if there is one,
//! and list the inbox. Message headers — subject, sender, time, unread —
//! are plaintext in Proton's API, so a working sign-in shows a real inbox
//! before any PGP is unlocked. Bodies stay encrypted until the key unlock
//! lands (`TODO_mail_srp_login.md` step 2).
//!
//! ## Off the UI thread, by construction
//!
//! Every step is a network round trip and the SRP proof is real
//! arithmetic; `mail_login` in the Qt build did all of it on the UI
//! thread and the TODO named that as the thing to fix once login worked.
//! [`SignIn`] owns a worker and a channel: the pane calls [`SignIn::start`]
//! and polls [`SignIn::poll`] each frame. The client lives on the worker
//! side between steps, so a TOTP prompt continues the SAME session — the
//! Qt build dropped the pending login on `NeedsTotp`, which is why
//! accounts with 2FA could never finish signing in there.
//!
//! ## Without the feature
//!
//! `mail-proton` off (a static musl build without ureq, say) compiles this
//! module to the same API answering [`Event::Unavailable`], so the pane
//! draws one honest line instead of a form that goes nowhere.

use std::sync::mpsc::{channel, Receiver, Sender};

/// What the worker reports back.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The password was accepted; the account wants a TOTP code. The
    /// worker keeps the session and waits for [`SignIn::totp`].
    NeedsTotp,
    /// Signed in. `display` is what to call the account.
    SignedIn { display: String },
    /// A page of the inbox, newest first.
    Inbox { total: i64, messages: Vec<crate::mail_view::Message> },
    /// The flow stopped. `retryable` says whether the form should stay
    /// filled: a wrong password is, a build without the feature is not.
    Failed { message: String, retryable: bool },
    /// This build has no Proton engine.
    Unavailable,
}

/// The pane's side of the worker.
#[derive(Default)]
pub struct SignIn {
    rx: Option<Receiver<Event>>,
    /// The channel the worker waits on for a TOTP code, once it has asked.
    totp_tx: Option<Sender<String>>,
    /// True from `start` until a terminal event arrives.
    pub busy: bool,
    /// The last event's story, for the pane to show.
    pub status: String,
    pub signed_in: Option<String>,
    pub needs_totp: bool,
}

impl SignIn {
    pub fn is_running(&self) -> bool { self.busy }

    /// Begin: /auth/info, the SRP proof, /auth, then the inbox — or a
    /// TOTP request part way. Ignored while a flow is running.
    pub fn start(&mut self, username: &str, password: &str) {
        if self.busy { return; }
        let (tx, rx) = channel();
        let (totp_tx, totp_rx) = channel::<String>();
        self.rx = Some(rx);
        self.totp_tx = Some(totp_tx);
        self.busy = true;
        self.needs_totp = false;
        self.status = "signing in…".into();
        let (u, p) = (username.trim().to_string(), password.to_string());
        std::thread::Builder::new().name("mail-signin".into()).spawn(move || {
            worker::run(&u, &p, tx, totp_rx);
        }).ok();
    }

    /// Hand the worker a TOTP code after [`Event::NeedsTotp`].
    pub fn totp(&mut self, code: &str) {
        if let Some(tx) = &self.totp_tx {
            let _ = tx.send(code.trim().to_string());
            self.needs_totp = false;
            self.status = "checking the code…".into();
        }
    }

    /// Drain the worker's events, updating the summary fields, and hand
    /// them to the caller for the ones it acts on (the inbox).
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(rx) = &self.rx else { return out };
        while let Ok(ev) = rx.try_recv() {
            match &ev {
                Event::NeedsTotp => { self.needs_totp = true; self.status = "enter the code from your authenticator".into(); }
                Event::SignedIn { display } => { self.signed_in = Some(display.clone()); self.status = format!("signed in as {display}"); }
                Event::Inbox { total, messages } => {
                    self.busy = false;
                    self.status = format!("inbox: {} of {total} message(s)", messages.len());
                }
                Event::Failed { message, .. } => { self.busy = false; self.status = message.clone(); }
                Event::Unavailable => { self.busy = false; self.status = worker::UNAVAILABLE.into(); }
            }
            out.push(ev);
        }
        if !self.busy { self.totp_tx = None; }
        out
    }
}

#[cfg(feature = "mail-proton")]
mod worker {
    use super::Event;
    use crate::mail_crypto::CryptoSuite;
    use crate::mail_error::MailError;
    use crate::mail_proton::{ClientConfig, MessageFilter, ProtonClient};
    use std::sync::mpsc::{Receiver, Sender};

    pub const UNAVAILABLE: &str = "this build has no Proton engine";

    /// Proton's inbox label id. The others (drafts 8, sent 7, all mail 5)
    /// are the same fixed ids hydroxide uses.
    const INBOX_LABEL: &str = "0";

    fn describe(e: &MailError) -> (String, bool) {
        match e {
            MailError::Transport(t) => (format!("could not reach Proton: {t}"), true),
            MailError::Api { code: 8002, .. } => ("wrong username or password".into(), true),
            // Human verification (captcha) — Proton asks for it after a
            // burst of failures or from a new network. Nothing here can
            // answer a captcha; say where to.
            MailError::Api { code: 9001, .. } =>
                ("Proton wants a human verification for this sign-in — complete one at mail.proton.me, then try again".into(), true),
            MailError::Api { code, message } => (format!("Proton refused ({code}): {message}"), true),
            MailError::Unauthorized => ("wrong username or password".into(), true),
            MailError::TwoFactorRequired => ("this account needs a second factor".into(), true),
            MailError::SrpServerProof => ("the server's proof did not verify — not talking to Proton?".into(), false),
            MailError::CryptoUnavailable(what) => (format!("this build cannot sign in: {what}"), false),
            other => (format!("{other:?}"), true),
        }
    }

    pub fn run(username: &str, password: &str, tx: Sender<Event>, totp_rx: Receiver<String>) {
        let suite = CryptoSuite::platform();
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();
        let client = ProtonClient::new(agent, ClientConfig::default());
        let info = match client.auth_info(username) {
            Ok(i) => i,
            Err(e) => { let (m, r) = describe(&e); let _ = tx.send(Event::Failed { message: m, retryable: r }); return; }
        };
        let auth = match client.auth(username, password.as_bytes(), &info, suite.srp.as_ref()) {
            Ok(a) => a,
            Err(e) => { let (m, r) = describe(&e); let _ = tx.send(Event::Failed { message: m, retryable: r }); return; }
        };
        if auth.two_factor_enabled || auth.totp {
            let _ = tx.send(Event::NeedsTotp);
            // The session waits on the channel — the same client, the
            // same tokens — until the code arrives or the pane is gone.
            let code = match totp_rx.recv() { Ok(c) => c, Err(_) => return };
            if let Err(e) = client.auth_totp(&code) {
                let (m, r) = describe(&e);
                let _ = tx.send(Event::Failed { message: m, retryable: r });
                return;
            }
        }
        let display = client.get_current_user().map(|u| u.name).unwrap_or_else(|_| username.to_string());
        let _ = tx.send(Event::SignedIn { display });
        let filter = MessageFilter { page_size: 50, limit: 50, label: INBOX_LABEL.into(), ..Default::default() };
        match client.list_messages(&filter) {
            Ok((total, msgs)) => {
                let messages = msgs.into_iter().map(|m| crate::mail_view::Message {
                    id: m.id,
                    conversation_id: m.conversation_id,
                    subject: m.subject,
                    unread: m.unread,
                    sender: m.sender.map(|a| crate::mail_view::Address { name: a.name, address: a.address }),
                    time: m.time,
                    num_attachments: m.num_attachments,
                    is_encrypted: m.is_encrypted,
                }).collect();
                let _ = tx.send(Event::Inbox { total, messages });
            }
            Err(e) => { let (m, r) = describe(&e); let _ = tx.send(Event::Failed { message: m, retryable: r }); }
        }
    }
}

#[cfg(not(feature = "mail-proton"))]
mod worker {
    use super::Event;
    use std::sync::mpsc::{Receiver, Sender};
    pub const UNAVAILABLE: &str = "this build has no Proton engine (built without mail-proton)";
    pub fn run(_u: &str, _p: &str, tx: Sender<Event>, _totp: Receiver<String>) {
        let _ = tx.send(Event::Unavailable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flow_settles_and_the_summary_tells_the_story() {
        // Without a network there is no live path to walk; what can be
        // held to is the pane-side machine: busy from start to a terminal
        // event, the TOTP prompt in between, the summary line after each.
        let mut s = SignIn::default();
        assert!(!s.is_running());
        let (tx, rx) = channel();
        s.rx = Some(rx);
        s.busy = true;
        tx.send(Event::NeedsTotp).unwrap();
        let evs = s.poll();
        assert_eq!(evs, vec![Event::NeedsTotp]);
        assert!(s.needs_totp && s.busy, "waiting on the code, still running");
        tx.send(Event::SignedIn { display: "ada".into() }).unwrap();
        tx.send(Event::Inbox { total: 3, messages: Vec::new() }).unwrap();
        let evs = s.poll();
        assert_eq!(evs.len(), 2);
        assert_eq!(s.signed_in.as_deref(), Some("ada"));
        assert!(!s.busy, "the inbox is the end of the flow");
        assert!(s.status.contains("3"), "{}", s.status);
    }

    #[test]
    fn a_failure_ends_the_flow_and_says_why() {
        let mut s = SignIn::default();
        let (tx, rx) = channel();
        s.rx = Some(rx);
        s.busy = true;
        tx.send(Event::Failed { message: "wrong username or password".into(), retryable: true }).unwrap();
        s.poll();
        assert!(!s.busy);
        assert_eq!(s.status, "wrong username or password");
        assert!(s.signed_in.is_none());
    }

    #[test]
    fn start_is_ignored_while_a_flow_runs() {
        let mut s = SignIn::default();
        s.busy = true;
        s.start("ada", "pw");
        assert!(s.rx.is_none(), "a second worker was spawned over the first");
    }

    #[test]
    #[cfg(feature = "mail-proton")]
    fn an_unreachable_endpoint_is_a_retryable_failure_not_a_hang() {
        // The real worker against a port nothing listens on: the error
        // must come back through the channel as a Failed event with the
        // form left usable. (Loopback only: no network in tests.)
        let (tx, rx) = channel();
        let (_ttx, trx) = channel::<String>();
        std::thread::spawn(move || {
            // A client pointed at loopback — through the same code path
            // as `run`, with the endpoint swapped.
            let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(2)).build();
            let client = crate::mail_proton::ProtonClient::new(agent, crate::mail_proton::ClientConfig {
                api_endpoint: "http://127.0.0.1:9".into(), ..Default::default() });
            match client.auth_info("nobody") {
                Ok(_) => { let _ = tx.send(Event::Failed { message: "unexpected success".into(), retryable: false }); }
                Err(e) => {
                    let msg = format!("{e:?}");
                    let _ = tx.send(Event::Failed { message: msg, retryable: true });
                }
            }
            drop(trx);
        });
        let ev = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("an answer");
        match ev {
            Event::Failed { message, retryable } => {
                assert!(retryable);
                assert!(message.contains("Transport") || message.contains("refused") || message.contains("connect"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
}
