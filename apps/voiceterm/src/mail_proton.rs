//! v0.1.861: ProtonMail API client (transport + auth + read surface).
//!
//! A faithful port of hydroxide's `protonmail` package behaviour onto
//! web2qml's stack (`ureq`, hand-rolled JSON, typed errors), with three
//! deliberate API improvements over the Go original:
//!
//!   1. **Refresh is injected, not a raw closure.** hydroxide hangs a
//!      `ReAuth func() error` field on the client and the credential layer
//!      reaches back in to mutate it. We take a [`RefreshHook`] trait
//!      object instead. On a 401 the transport calls `hook.refresh(self)`,
//!      passing the client by reference — so the hook (which lives in the
//!      session manager and knows how to persist the rotated tokens) never
//!      needs a self-referential `Arc` back to the client. The reference
//!      cycle hydroxide's design implies simply doesn't exist here.
//!
//!   2. **Token state is owned, not public.** hydroxide exposes `uid` /
//!      `accessToken` as package-private fields poked from several files.
//!      Here they live behind one `Mutex<Tokens>`; the only ways to change
//!      them are `auth`, `auth_refresh`, and `logout`.
//!
//!   3. **Errors are typed.** The "refresh token is dead" branch
//!      (hydroxide's `apiErr.Code == 10013`) is `MailError::needs_reauth()`
//!      instead of a magic-number string match.
//!
//! Scope: this lands the auth flow, user/address fetch, key-salt fetch,
//! key unlock (via the injected `PgpEngine`), and the **read** side of
//! messages (list + get). The send/draft path — the largest piece of
//! hydroxide's `smtp.go`, which builds MIME and per-recipient package sets
//! — is left as a marked seam (`OutgoingMessage`/`send_message`) so the
//! read bridge can ship first, exactly as hydroxide shipped IMAP behind
//! "here be dragons" before SMTP hardened.

use std::io::Read;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::mail_crypto::{self, KeyRing, LockedKey, PgpEngine, Srp, SrpInfo};
use crate::mail_error::{code, MailError, Result};
use crate::mail_json::{Json, Obj};

/// ProtonMail API version pinned in the `X-Pm-Apiversion` header (matches
/// hydroxide's `Version = 3`).
const API_VERSION: &str = "3";
const DEFAULT_API: &str = "https://mail.proton.me/api";
const DEFAULT_APP_VERSION: &str = "Other";
/// hydroxide spoofs a Firefox UA; we keep web2qml's own and only borrow
/// the Proton app-version header it actually checks.
const USER_AGENT: &str = "Web2QML/0.1 ProtonBridge ureq/2";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordMode {
    Single,
    Two,
    Unknown(i64),
}

impl PasswordMode {
    fn from_i64(n: i64) -> Self {
        match n {
            1 => PasswordMode::Single,
            2 => PasswordMode::Two,
            other => PasswordMode::Unknown(other),
        }
    }
}

/// The mutable authentication state of one client.
#[derive(Clone, Default)]
struct Tokens {
    uid: String,
    access: String,
    refresh: String,
}

/// Decoded `/auth` (and `/auth/refresh`) response, minus the secrets that
/// stay inside [`Tokens`]. This is what the credential store persists.
#[derive(Clone, Debug, Default)]
pub struct Auth {
    pub uid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub event_id: String,
    pub scope: String,
    pub password_mode: PasswordModeRepr,
    pub two_factor_enabled: bool,
    pub totp: bool,
    /// Absolute expiry computed from `ExpiresIn`.
    pub expires_at: Option<SystemTime>,
}

/// Plain-old-data mirror of [`PasswordMode`] that defaults sanely for the
/// store. (Kept separate so `Auth` can derive `Default`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordModeRepr {
    Single,
    Two,
    Unknown(i64),
}
impl Default for PasswordModeRepr {
    fn default() -> Self {
        PasswordModeRepr::Single
    }
}
impl From<PasswordMode> for PasswordModeRepr {
    fn from(m: PasswordMode) -> Self {
        match m {
            PasswordMode::Single => PasswordModeRepr::Single,
            PasswordMode::Two => PasswordModeRepr::Two,
            PasswordMode::Unknown(n) => PasswordModeRepr::Unknown(n),
        }
    }
}

/// Hook the session manager installs so the transport can recover from a
/// 401 without knowing how tokens are stored or persisted. See the module
/// doc for why this replaces hydroxide's `ReAuth` closure.
pub trait RefreshHook: Send + Sync {
    fn refresh(&self, client: &ProtonClient) -> Result<()>;
}

/// Static configuration for a client.
#[derive(Clone)]
pub struct ClientConfig {
    pub api_endpoint: String,
    pub app_version: String,
    pub debug: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            api_endpoint: DEFAULT_API.to_string(),
            app_version: DEFAULT_APP_VERSION.to_string(),
            debug: false,
        }
    }
}

/// A ProtonMail API client bound to one account session.
pub struct ProtonClient {
    agent: ureq::Agent,
    cfg: ClientConfig,
    tokens: Mutex<Tokens>,
    refresh_hook: Mutex<Option<Arc<dyn RefreshHook>>>,
}

impl ProtonClient {
    pub fn new(agent: ureq::Agent, cfg: ClientConfig) -> Self {
        ProtonClient {
            agent,
            cfg,
            tokens: Mutex::new(Tokens::default()),
            refresh_hook: Mutex::new(None),
        }
    }

    /// Install the refresh policy. Called once by the session manager.
    pub fn set_refresh_hook(&self, hook: Arc<dyn RefreshHook>) {
        *self.refresh_hook.lock().unwrap() = Some(hook);
    }

    /// Adopt an `Auth` loaded from the credential store (sets live tokens).
    pub fn restore_tokens(&self, auth: &Auth) {
        let mut t = self.tokens.lock().unwrap();
        t.uid = auth.uid.clone();
        t.access = auth.access_token.clone();
        t.refresh = auth.refresh_token.clone();
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.cfg.api_endpoint, path)
    }

    // --- core request path ------------------------------------------------

    /// Issue a request and decode the JSON body, applying Proton's
    /// `Code`-based error convention and a single automatic re-auth retry
    /// on HTTP 401 (mirrors hydroxide's `do()`), but only when a refresh
    /// hook is installed.
    fn request_json(&self, method: &str, path: &str, body: Option<&str>) -> Result<Json> {
        // Mirror hydroxide's `do()`: at most two network round-trips. Issue
        // the request once and *reuse* that response. Only on a genuine 401,
        // and only when a refresh hook is installed, do we rotate tokens and
        // retry exactly once. Transport errors are not retried here — a
        // connection failure is not an auth problem, and silently doubling
        // requests on every network blip hides real failures.
        //
        // Bugfix (v0.1.862): the prior version discarded this first response
        // and re-issued the request inside a helper purely to re-derive the
        // 401 it already had — a wasted round-trip on every token expiry —
        // and also retried plain transport errors unintentionally.
        let first = self.request_once(method, path, body);
        if !matches!(first, Err(MailError::Api { code: 401, .. })) {
            return first;
        }
        // Clone the hook Arc and drop the lock before calling out, so the
        // refresh (which re-enters the client to rotate tokens) cannot
        // contend on this guard.
        let hook = match self.refresh_hook.lock().unwrap().clone() {
            Some(h) => h,
            None => return first, // no policy installed: surface the 401
        };
        crate::clog!("mail", "[proton] 401 on {path}; refreshing tokens");
        hook.refresh(self)?;
        self.request_once(method, path, body)
    }

    fn request_once(&self, method: &str, path: &str, body: Option<&str>) -> Result<Json> {
        let (uid, access) = {
            let t = self.tokens.lock().unwrap();
            (t.uid.clone(), t.access.clone())
        };

        let mut req = self
            .agent
            .request(method, &self.url(path))
            .set("X-Pm-Appversion", &self.cfg.app_version)
            .set("X-Pm-Apiversion", API_VERSION)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/json");

        if !uid.is_empty() && !access.is_empty() {
            req = req
                .set("X-Pm-Uid", &uid)
                .set("Authorization", &format!("Bearer {access}"));
        }

        if self.cfg.debug {
            crate::clog!("mail", "[proton] >> {method} {path}");
        }

        let result = match body {
            Some(b) => req.set("Content-Type", "application/json").send_string(b),
            None => req.call(),
        };

        let text = match result {
            Ok(resp) => resp
                .into_string()
                .map_err(|e| MailError::Transport(e.to_string()))?,
            Err(ureq::Error::Status(code_http, resp)) => {
                let raw = resp.into_string().unwrap_or_default();
                // Proton returns a JSON error body even on 4xx/5xx; surface
                // its Code/Error if present, else the HTTP status.
                if let Ok(j) = Json::parse(&raw) {
                    return Err(api_error_from(&j, code_http as i64));
                }
                return Err(MailError::Api {
                    code: code_http as i64,
                    message: raw,
                });
            }
            Err(e) => return Err(MailError::Transport(e.to_string())),
        };

        let json = Json::parse(&text)?;
        // Success bodies still carry a Code; non-OK codes are API errors.
        if let Some(c) = json.int_field("Code") {
            if c != code::OK && c != code::OK_WITH_BODY {
                return Err(api_error_from(&json, c));
            }
        }
        if self.cfg.debug {
            crate::clog!("mail", "[proton] << {method} {path} ok");
        }
        Ok(json)
    }

    // --- auth flow --------------------------------------------------------

    /// `POST /auth/info` — fetch the SRP parameters for `username`.
    pub fn auth_info(&self, username: &str) -> Result<SrpInfo> {
        let body = Obj::new().str("Username", username).build();
        let j = self.request_json("POST", "/auth/info", Some(&body))?;
        Ok(SrpInfo {
            version: j.int_field("Version").unwrap_or(0),
            modulus: j.str_field("Modulus").unwrap_or_default(),
            server_ephemeral: j.str_field("ServerEphemeral").unwrap_or_default(),
            salt: j.str_field("Salt").unwrap_or_default(),
            srp_session: j.str_field("SRPSession").unwrap_or_default(),
        })
    }

    /// `POST /auth` — complete the SRP exchange and adopt the session
    /// tokens. The SRP math is delegated to the injected provider so this
    /// stays pure transport.
    pub fn auth(
        &self,
        username: &str,
        password: &[u8],
        info: &SrpInfo,
        srp: &dyn Srp,
    ) -> Result<Auth> {
        let proofs = srp.compute(password, info)?;
        let body = Obj::new()
            .str("Username", username)
            .str("SRPSession", &info.srp_session)
            .str("ClientEphemeral", &proofs.client_ephemeral)
            .str("ClientProof", &proofs.client_proof)
            .build();
        let j = self.request_json("POST", "/auth", Some(&body))?;

        if let Some(server_proof) = j.str_field("ServerProof") {
            srp.verify_server(&proofs, &server_proof)?;
        } else {
            return Err(MailError::SrpServerProof);
        }

        let auth = parse_auth(&j);
        self.adopt(&auth);
        Ok(auth)
    }

    /// `POST /auth/2fa` — submit a TOTP code. Returns the new scope.
    pub fn auth_totp(&self, code: &str) -> Result<String> {
        let body = Obj::new().str("TwoFactorCode", code).build();
        let j = self.request_json("POST", "/auth/2fa", Some(&body))?;
        Ok(j.str_field("Scope").unwrap_or_default())
    }

    /// `POST /auth/refresh` — rotate tokens using the stored refresh token.
    /// On the dead-token case Proton answers with code
    /// [`code::INVALID_REFRESH_TOKEN`], which becomes
    /// `MailError::needs_reauth()` for the caller.
    pub fn auth_refresh(&self) -> Result<Auth> {
        let refresh = self.tokens.lock().unwrap().refresh.clone();
        let uid = self.tokens.lock().unwrap().uid.clone();
        let body = Obj::new()
            .str("RefreshToken", &refresh)
            .str("ResponseType", "token")
            .str("GrantType", "refresh_token")
            .str("RedirectURI", "http://www.protonmail.ch")
            .build();

        // The refresh endpoint needs the UID header even though the access
        // token is (by definition) expired.
        let j = {
            let mut req = self
                .agent
                .request("POST", &self.url("/auth/refresh"))
                .set("X-Pm-Appversion", &self.cfg.app_version)
                .set("X-Pm-Apiversion", API_VERSION)
                .set("User-Agent", USER_AGENT)
                .set("Accept", "application/json")
                .set("Content-Type", "application/json");
            if !uid.is_empty() {
                req = req.set("X-Pm-Uid", &uid);
            }
            match req.send_string(&body) {
                Ok(resp) => Json::parse(
                    &resp
                        .into_string()
                        .map_err(|e| MailError::Transport(e.to_string()))?,
                )?,
                Err(ureq::Error::Status(c, resp)) => {
                    let raw = resp.into_string().unwrap_or_default();
                    let j = Json::parse(&raw).unwrap_or(Json::Null);
                    return Err(api_error_from(&j, c as i64));
                }
                Err(e) => return Err(MailError::Transport(e.to_string())),
            }
        };
        if let Some(c) = j.int_field("Code") {
            if c != code::OK && c != code::OK_WITH_BODY {
                return Err(api_error_from(&j, c));
            }
        }
        let auth = parse_auth(&j);
        self.adopt(&auth);
        Ok(auth)
    }

    /// `GET /keys/salts` — per-key salts, base64-decoded.
    pub fn list_key_salts(&self) -> Result<HashMap<String, Vec<u8>>> {
        let j = self.request_json("GET", "/keys/salts", None)?;
        let mut salts = HashMap::new();
        if let Some(arr) = j.get("KeySalts").and_then(Json::as_array) {
            for s in arr {
                let id = match s.str_field("ID") {
                    Some(i) => i,
                    None => continue,
                };
                match s.str_field("KeySalt") {
                    Some(b64) if !b64.is_empty() => {
                        salts.insert(id, mail_crypto::b64_decode(&b64)?);
                    }
                    _ => {
                        salts.insert(id, Vec::new());
                    }
                }
            }
        }
        Ok(salts)
    }

    /// `GET /users` — the current user, including locked user keys.
    pub fn get_current_user(&self) -> Result<User> {
        let j = self.request_json("GET", "/users", None)?;
        let u = j.get("User").ok_or(MailError::Decode("missing User".into()))?;
        Ok(User::from_json(u))
    }

    /// `GET /addresses` — the account's addresses, each with its keys.
    pub fn list_addresses(&self) -> Result<Vec<Address>> {
        let j = self.request_json("GET", "/addresses", None)?;
        let mut out = Vec::new();
        if let Some(arr) = j.get("Addresses").and_then(Json::as_array) {
            for a in arr {
                out.push(Address::from_json(a));
            }
        }
        Ok(out)
    }

    /// Two-stage key unlock, matching hydroxide's `Unlock`: user keys
    /// first, then address keys (which may be locked by an address-key
    /// token decryptable with the user keyring). Returns the combined
    /// keyring via the injected [`PgpEngine`].
    pub fn unlock(
        &self,
        salts: &HashMap<String, Vec<u8>>,
        passphrase: &[u8],
        pgp: &dyn PgpEngine,
    ) -> Result<Box<dyn KeyRing>> {
        let user = self.get_current_user()?;
        let addrs = self.list_addresses()?;
        let mut address_keys = Vec::new();
        for a in &addrs {
            address_keys.extend(a.keys.iter().cloned());
        }
        pgp.unlock(&user.keys, &address_keys, salts, passphrase)
    }

    /// `DELETE /auth` — invalidate the session server-side and clear local
    /// tokens.
    pub fn logout(&self) -> Result<()> {
        let _ = self.request_json("DELETE", "/auth", None);
        let mut t = self.tokens.lock().unwrap();
        *t = Tokens::default();
        Ok(())
    }

    // --- read-side messages ----------------------------------------------

    /// `GET /messages` with a filter. Returns `(total, headers)`. Bodies
    /// are not fetched here (Proton returns metadata for list views); call
    /// [`ProtonClient::get_message`] for the encrypted body.
    pub fn list_messages(&self, filter: &MessageFilter) -> Result<(i64, Vec<Message>)> {
        let q = filter.query_string();
        let path = if q.is_empty() {
            "/messages".to_string()
        } else {
            format!("/messages?{q}")
        };
        let j = self.request_json("GET", &path, None)?;
        let total = j.int_field("Total").unwrap_or(0);
        let mut msgs = Vec::new();
        if let Some(arr) = j.get("Messages").and_then(Json::as_array) {
            for m in arr {
                msgs.push(Message::from_json(m));
            }
        }
        Ok((total, msgs))
    }

    /// `GET /messages/{id}` — a single message including its (still
    /// encrypted) `Body`. Decrypt with [`PgpEngine::decrypt`] and the
    /// session keyring.
    pub fn get_message(&self, id: &str) -> Result<Message> {
        let j = self.request_json("GET", &format!("/messages/{id}"), None)?;
        let m = j
            .get("Message")
            .ok_or(MailError::Decode("missing Message".into()))?;
        Ok(Message::from_json(m))
    }

    // --- seam: send path (not yet ported) --------------------------------

    /// The outgoing path (draft creation + per-recipient package-set
    /// encryption + `POST /messages/{id}`) is the largest part of
    /// hydroxide's SMTP backend and depends on a complete `PgpEngine`
    /// encrypt/sign surface. It is intentionally unimplemented in this
    /// drop so the read bridge can land first.
    pub fn send_message(&self, _outgoing: &OutgoingMessage) -> Result<Message> {
        Err(MailError::Other(
            "send path not yet ported; read bridge only".into(),
        ))
    }

    // --- message actions (mirror hydroxide's doMessages/LabelMessages) ----

    /// `PUT /messages/{action}` with `{IDs:[...]}`. Used for read/unread/
    /// delete/undelete. `ids` may be borrowed.
    fn do_messages(&self, action: &str, ids: &[&str]) -> Result<()> {
        let body = format!("{{\"IDs\":{}}}", json_str_array(ids));
        self.request_json("PUT", &format!("/messages/{action}"), Some(&body))?;
        Ok(())
    }

    /// Mark messages read.
    pub fn mark_read(&self, ids: &[&str]) -> Result<()> {
        self.do_messages("read", ids)
    }
    /// Mark messages unread.
    pub fn mark_unread(&self, ids: &[&str]) -> Result<()> {
        self.do_messages("unread", ids)
    }
    /// Move to Trash is a label-add of the Trash label; permanent delete is
    /// `/messages/delete`. We expose both; the bridge prefers `trash`.
    pub fn delete_messages(&self, ids: &[&str]) -> Result<()> {
        self.do_messages("delete", ids)
    }

    /// `PUT /messages/label` — add `label_id` to each id.
    pub fn label_messages(&self, label_id: &str, ids: &[&str]) -> Result<()> {
        let body = format!(
            "{{\"LabelID\":{},\"IDs\":{}}}",
            json_quote(label_id),
            json_str_array(ids)
        );
        self.request_json("PUT", "/messages/label", Some(&body))?;
        Ok(())
    }

    /// `PUT /messages/unlabel` — remove `label_id` from each id.
    pub fn unlabel_messages(&self, label_id: &str, ids: &[&str]) -> Result<()> {
        let body = format!(
            "{{\"LabelID\":{},\"IDs\":{}}}",
            json_quote(label_id),
            json_str_array(ids)
        );
        self.request_json("PUT", "/messages/unlabel", Some(&body))?;
        Ok(())
    }

    /// `GET /attachments/{id}` — the raw (still PGP-encrypted) attachment
    /// payload. Decrypt with the session keyring via the `PgpEngine` (the
    /// attachment's `key_packets` unlock the session key). Bodies can be
    /// large; this returns the bytes as-is.
    pub fn get_attachment(&self, id: &str) -> Result<Vec<u8>> {
        let (uid, access) = {
            let t = self.tokens.lock().unwrap();
            (t.uid.clone(), t.access.clone())
        };
        let mut req = self
            .agent
            .request("GET", &self.url(&format!("/attachments/{id}")))
            .set("X-Pm-Appversion", &self.cfg.app_version)
            .set("X-Pm-Apiversion", API_VERSION)
            .set("User-Agent", USER_AGENT);
        if !uid.is_empty() && !access.is_empty() {
            req = req
                .set("X-Pm-Uid", &uid)
                .set("Authorization", &format!("Bearer {access}"));
        }
        match req.call() {
            Ok(resp) => {
                // Attachment payloads are binary; read into a Vec via the
                // reader. ureq's into_string would corrupt binary, so we read
                // bytes.
                let mut buf = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut buf)
                    .map_err(|e| MailError::Transport(e.to_string()))?;
                Ok(buf)
            }
            Err(ureq::Error::Status(code, _)) => Err(MailError::Api {
                code: code as i64,
                message: format!("attachment fetch failed: HTTP {code}"),
            }),
            Err(e) => Err(MailError::Transport(e.to_string())),
        }
    }

    // --- internal ---------------------------------------------------------

    fn adopt(&self, auth: &Auth) {
        let mut t = self.tokens.lock().unwrap();
        if !auth.uid.is_empty() {
            t.uid = auth.uid.clone();
        }
        t.access = auth.access_token.clone();
        if !auth.refresh_token.is_empty() {
            t.refresh = auth.refresh_token.clone();
        }
    }
}

/// Build a `MailError::Api` from a Proton JSON error body and a fallback
/// code (the HTTP status, used when the body lacks `Code`).
fn api_error_from(j: &Json, fallback_code: i64) -> MailError {
    let code = j.int_field("Code").unwrap_or(fallback_code);
    let message = j
        .str_field("Error")
        .unwrap_or_else(|| format!("HTTP {fallback_code}"));
    MailError::Api { code, message }
}

fn parse_auth(j: &Json) -> Auth {
    let expires_in = j.int_field("ExpiresIn").unwrap_or(0);
    let expires_at = if expires_in > 0 {
        SystemTime::now().checked_add(Duration::from_secs(expires_in as u64))
    } else {
        None
    };
    let two_factor = j.get("2FA");
    let enabled = two_factor.and_then(|f| f.int_field("Enabled")).unwrap_or(0) != 0;
    let totp = two_factor.and_then(|f| f.int_field("TOTP")).unwrap_or(0) == 1;
    Auth {
        uid: j.str_field("UID").unwrap_or_default(),
        access_token: j.str_field("AccessToken").unwrap_or_default(),
        refresh_token: j.str_field("RefreshToken").unwrap_or_default(),
        user_id: j.str_field("UserID").unwrap_or_default(),
        event_id: j.str_field("EventID").unwrap_or_default(),
        scope: j.str_field("Scope").unwrap_or_default(),
        password_mode: PasswordMode::from_i64(j.int_field("PasswordMode").unwrap_or(1)).into(),
        two_factor_enabled: enabled,
        totp,
        expires_at,
    }
}

// ---------------------------------------------------------------------
// Typed read models
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct User {
    pub id: String,
    pub name: String,
    pub used_space: i64,
    pub max_space: i64,
    pub keys: Vec<LockedKey>,
}

impl User {
    fn from_json(j: &Json) -> Self {
        User {
            id: j.str_field("ID").unwrap_or_default(),
            name: j.str_field("Name").unwrap_or_default(),
            used_space: j.int_field("UsedSpace").unwrap_or(0),
            max_space: j.int_field("MaxSpace").unwrap_or(0),
            keys: parse_keys(j.get("Keys")),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Address {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub status: i64,
    pub send: i64,
    pub keys: Vec<LockedKey>,
}

impl Address {
    fn from_json(j: &Json) -> Self {
        Address {
            id: j.str_field("ID").unwrap_or_default(),
            email: j.str_field("Email").unwrap_or_default(),
            display_name: j.str_field("DisplayName").unwrap_or_default(),
            status: j.int_field("Status").unwrap_or(0),
            send: j.int_field("Send").unwrap_or(0),
            keys: parse_keys(j.get("Keys")),
        }
    }
}

fn parse_keys(keys: Option<&Json>) -> Vec<LockedKey> {
    let mut out = Vec::new();
    if let Some(arr) = keys.and_then(Json::as_array) {
        for k in arr {
            out.push(LockedKey {
                id: k.str_field("ID").unwrap_or_default(),
                fingerprint: k.str_field("Fingerprint").unwrap_or_default(),
                private_key_armored: k.str_field("PrivateKey").unwrap_or_default(),
                token: k.str_field("Token"),
                signature: k.str_field("Signature"),
                active: k.int_field("Active").unwrap_or(0) == 1,
            });
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct MessageAddress {
    pub name: String,
    pub address: String,
}

impl MessageAddress {
    fn from_json(j: &Json) -> Self {
        MessageAddress {
            name: j.str_field("Name").unwrap_or_default(),
            address: j.str_field("Address").unwrap_or_default(),
        }
    }
    fn list(j: Option<&Json>) -> Vec<MessageAddress> {
        j.and_then(Json::as_array)
            .map(|a| a.iter().map(MessageAddress::from_json).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Attachment {
    pub id: String,
    pub message_id: String,
    pub name: String,
    pub size: i64,
    pub mime_type: String,
    pub content_id: String,
    /// Encrypted, base64-encoded session-key packets (decrypt with the user
    /// keyring via the PgpEngine to read the payload).
    pub key_packets: String,
    pub signature: String,
}

impl Attachment {
    fn from_json(j: &Json) -> Self {
        Attachment {
            id: j.str_field("ID").unwrap_or_default(),
            message_id: j.str_field("MessageID").unwrap_or_default(),
            name: j.str_field("Name").unwrap_or_default(),
            size: j.int_field("Size").unwrap_or(0),
            mime_type: j.str_field("MIMEType").unwrap_or_default(),
            content_id: j.str_field("ContentID").unwrap_or_default(),
            key_packets: j.str_field("KeyPackets").unwrap_or_default(),
            signature: j.str_field("Signature").unwrap_or_default(),
        }
    }
    fn list(j: Option<&Json>) -> Vec<Attachment> {
        j.and_then(Json::as_array)
            .map(|a| a.iter().map(Attachment::from_json).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub subject: String,
    pub unread: bool,
    pub sender: Option<MessageAddress>,
    pub to: Vec<MessageAddress>,
    pub cc: Vec<MessageAddress>,
    pub bcc: Vec<MessageAddress>,
    pub time: i64,
    pub size: i64,
    pub num_attachments: i64,
    pub is_encrypted: i64,
    pub address_id: String,
    pub label_ids: Vec<String>,
    /// Armored PGP body (present from `get_message`, empty in list views).
    pub body: String,
    pub mime_type: String,
    /// Attachment metadata (present from `get_message`).
    pub attachments: Vec<Attachment>,
}

impl Message {
    fn from_json(j: &Json) -> Self {
        Message {
            id: j.str_field("ID").unwrap_or_default(),
            conversation_id: j.str_field("ConversationID").unwrap_or_default(),
            subject: j.str_field("Subject").unwrap_or_default(),
            unread: j.int_field("Unread").unwrap_or(0) != 0,
            sender: j.get("Sender").map(MessageAddress::from_json),
            to: MessageAddress::list(j.get("ToList")),
            cc: MessageAddress::list(j.get("CCList")),
            bcc: MessageAddress::list(j.get("BCCList")),
            time: j.int_field("Time").unwrap_or(0),
            size: j.int_field("Size").unwrap_or(0),
            num_attachments: j.int_field("NumAttachments").unwrap_or(0),
            is_encrypted: j.int_field("IsEncrypted").unwrap_or(0),
            address_id: j.str_field("AddressID").unwrap_or_default(),
            label_ids: j
                .get("LabelIDs")
                .and_then(Json::as_array)
                .map(|a| a.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            body: j.str_field("Body").unwrap_or_default(),
            mime_type: j.str_field("MIMEType").unwrap_or_default(),
            attachments: Attachment::list(j.get("Attachments")),
        }
    }
}

/// Filter for [`ProtonClient::list_messages`]. A trimmed, typed subset of
/// hydroxide's `MessageFilter` covering the fields the bridge front-ends and
/// the voice query path use. Empty strings / zero / `None` mean "unset".
#[derive(Clone, Debug, Default)]
pub struct MessageFilter {
    pub page: i64,
    pub page_size: i64,
    pub limit: i64,
    pub label: String,
    pub conversation: String,
    pub address_id: String,
    pub asc: bool,
    // --- server-side narrowing (v0.1.863) ---
    /// Sender address/keyword (Proton's `From`).
    pub from: String,
    /// Recipient address (Proton's `To`).
    pub to: String,
    /// Subject substring (Proton's `Subject`).
    pub subject: String,
    /// Full-text keyword (Proton's `Keyword`) — matches subject + body.
    pub keyword: String,
    /// Inclusive lower time bound, Unix seconds (Proton's `Begin`).
    pub begin: i64,
    /// Exclusive-ish upper time bound, Unix seconds (Proton's `End`).
    pub end: i64,
    /// `Some(true)` = unread only, `Some(false)` = read only.
    pub unread: Option<bool>,
}

impl MessageFilter {
    fn query_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.page != 0 {
            parts.push(format!("Page={}", self.page));
        }
        if self.page_size != 0 {
            parts.push(format!("PageSize={}", self.page_size));
        }
        if self.limit != 0 {
            parts.push(format!("Limit={}", self.limit));
        }
        if !self.label.is_empty() {
            parts.push(format!("Label={}", url_encode(&self.label)));
        }
        if !self.conversation.is_empty() {
            parts.push(format!("Conversation={}", url_encode(&self.conversation)));
        }
        if !self.address_id.is_empty() {
            parts.push(format!("AddressID={}", url_encode(&self.address_id)));
        }
        if !self.from.is_empty() {
            parts.push(format!("From={}", url_encode(&self.from)));
        }
        if !self.to.is_empty() {
            parts.push(format!("To={}", url_encode(&self.to)));
        }
        if !self.subject.is_empty() {
            parts.push(format!("Subject={}", url_encode(&self.subject)));
        }
        if !self.keyword.is_empty() {
            parts.push(format!("Keyword={}", url_encode(&self.keyword)));
        }
        if self.begin > 0 {
            parts.push(format!("Begin={}", self.begin));
        }
        if self.end > 0 {
            parts.push(format!("End={}", self.end));
        }
        if let Some(u) = self.unread {
            parts.push(format!("Unread={}", if u { 1 } else { 0 }));
        }
        if self.asc {
            parts.push("Desc=0".to_string());
        }
        parts.join("&")
    }
}

/// Quote and escape a string as a JSON string literal.
fn json_quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Build a JSON array of strings: `["a","b"]`.
fn json_str_array(ids: &[&str]) -> String {
    let mut o = String::from("[");
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&json_quote(id));
    }
    o.push(']');
    o
}

/// Minimal percent-encoding for query values (alnum and `-_.~` pass).
fn url_encode(s: &str) -> String {    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Placeholder for the send path (see [`ProtonClient::send_message`]).
#[derive(Clone, Debug, Default)]
pub struct OutgoingMessage {
    pub id: String,
    pub mime: Vec<u8>,
    pub recipients: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_response() {
        let j = Json::parse(
            r#"{"Code":1000,"UID":"u1","AccessToken":"at","RefreshToken":"rt",
                "ExpiresIn":3600,"PasswordMode":2,"2FA":{"Enabled":1,"TOTP":1}}"#,
        )
        .unwrap();
        let a = parse_auth(&j);
        assert_eq!(a.uid, "u1");
        assert_eq!(a.password_mode, PasswordModeRepr::Two);
        assert!(a.two_factor_enabled && a.totp);
        assert!(a.expires_at.is_some());
    }

    #[test]
    fn maps_invalid_refresh_to_reauth() {
        let j = Json::parse(r#"{"Code":10013,"Error":"Invalid refresh token"}"#).unwrap();
        let e = api_error_from(&j, 422);
        assert!(e.needs_reauth());
    }

    #[test]
    fn filter_query_builds() {
        let f = MessageFilter {
            page_size: 50,
            label: "0".into(),
            asc: true,
            ..Default::default()
        };
        let q = f.query_string();
        assert!(q.contains("PageSize=50"));
        assert!(q.contains("Label=0"));
        assert!(q.contains("Desc=0"));
    }

    #[test]
    fn parses_message_addresses() {
        let j = Json::parse(
            r#"{"Message":{"ID":"m1","Subject":"hi","ToList":[{"Name":"A","Address":"a@b.c"}],
                "Unread":1,"IsEncrypted":2}}"#,
        )
        .unwrap();
        let m = Message::from_json(j.get("Message").unwrap());
        assert_eq!(m.subject, "hi");
        assert_eq!(m.to.len(), 1);
        assert_eq!(m.to[0].address, "a@b.c");
        assert!(m.unread);
    }
}
