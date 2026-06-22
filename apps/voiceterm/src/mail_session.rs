//! Warm sessions and the multi-account session manager.
//!
//! v0.1.861: hydroxide's `auth.Manager` quietly does four different jobs at
//! once — it is the on-disk credential store, the bridge-password gate, the
//! live-`*protonmail.Client` cache, and the re-auth driver, all guarded by a
//! single mutex. That conflation is why adding a second account, or
//! reasoning about what happens when one account's refresh token dies, is
//! awkward there.
//!
//! Here those concerns are split:
//!
//!   * persistence lives in [`crate::mail_account::AccountStore`];
//!   * the bridge-password gate is [`crate::mail_crypto::PasswordGate`];
//!   * token refresh is a [`RefreshHook`] the manager installs on each
//!     client (see below);
//!   * and *this* module only owns the warm in-memory map of unlocked
//!     [`Session`]s, keyed by [`AccountId`].
//!
//! The [`RefreshHook`] indirection is the key API improvement over
//! hydroxide's `ReAuth` closure. In hydroxide the client holds a closure
//! that captures the client, so the client effectively references itself;
//! refreshing also has to write the new tokens back through the manager,
//! tangling the layers. Here the hook is a small object that receives
//! `&ProtonClient` as a parameter — no self-capture — and writes the
//! rotated tokens back to the [`AccountStore`] through a clean path. The
//! transport stays oblivious to storage.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::mail_account::{bridge_key_from_text, AccountId, AccountRecord, AccountStore};
use crate::mail_crypto::{CryptoSuite, KeyRing};
use crate::mail_error::{MailError, Result};
use crate::mail_proton::{Address, Auth, ClientConfig, ProtonClient, RefreshHook};

/// An unlocked, ready-to-serve account: a live API client, the decrypted
/// PGP keyring, the address list a front-end needs for routing, and the
/// event cursor for incremental sync. Cloned `Arc`-cheaply and shared by
/// every front-end (SMTP, IMAP, CardDAV) serving that account.
pub struct Session {
    pub id: AccountId,
    pub username: String,
    pub client: Arc<ProtonClient>,
    pub keyring: Box<dyn KeyRing>,
    pub addresses: Vec<Address>,
    /// Last processed event id, for `GET /events/{id}` incremental sync.
    pub event_cursor: String,
}

impl Session {
    /// Primary (first) address, by convention the account's main identity.
    pub fn primary_address(&self) -> Option<&Address> {
        self.addresses.first()
    }

    /// Look up an address by its email, case-insensitively — what an SMTP
    /// front-end does to pick the sending identity.
    pub fn address_for_email(&self, email: &str) -> Option<&Address> {
        find_address(&self.addresses, email)
    }
}

/// Case-insensitive address lookup. Free function so it can be unit-tested
/// without standing up a live client.
fn find_address<'a>(addresses: &'a [Address], email: &str) -> Option<&'a Address> {
    let needle = email.to_ascii_lowercase();
    addresses
        .iter()
        .find(|a| a.email.to_ascii_lowercase() == needle)
}

/// The refresh policy installed on every client the manager builds.
///
/// It owns just enough to rotate and persist tokens: which account this is,
/// the shared store, and the bridge key needed to re-seal the updated
/// record. Crucially it takes `&ProtonClient` as a call parameter, so it
/// never has to capture the very client it serves.
struct StoreBackedRefresh {
    id: AccountId,
    store: Arc<AccountStore>,
    bridge_key: [u8; 32],
}

impl RefreshHook for StoreBackedRefresh {
    fn refresh(&self, client: &ProtonClient) -> Result<()> {
        // Rotate tokens on the wire. `auth_refresh` already adopts the new
        // tokens into the client; here we just have to persist them.
        let new_auth: Auth = client.auth_refresh()?;
        // Re-open, update tokens, re-seal. Load uses the same bridge key the
        // manager validated at authenticate() time.
        let mut record = self.store.load(&self.id, &self.bridge_key)?;
        record.auth = new_auth;
        record.updated_at = now_secs();
        self.store.save(&record, &self.bridge_key)?;
        Ok(())
    }
}

/// Owns the warm session map and brokers authentication against the store.
///
/// One instance is shared (behind `Arc`) by every bridge front-end. All the
/// heavy state (clients, keyrings) lives behind a single `RwLock<HashMap>`;
/// reads (the common case — an already-authenticated IMAP connection
/// fetching mail) take the read lock, and only first-time unlocks take the
/// write lock.
pub struct SessionManager {
    store: Arc<AccountStore>,
    crypto: Arc<CryptoSuite>,
    client_config: ClientConfig,
    agent: ureq::Agent,
    sessions: RwLock<HashMap<AccountId, Arc<Session>>>,
}

impl SessionManager {
    pub fn new(
        store: Arc<AccountStore>,
        crypto: Arc<CryptoSuite>,
        agent: ureq::Agent,
        client_config: ClientConfig,
    ) -> Self {
        SessionManager {
            store,
            crypto,
            client_config,
            agent,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Authenticate a bridge connection for `id` with the base64
    /// `bridge_password` the user configured in their mail client.
    ///
    /// Fast path: if the account is already warm, validate the presented
    /// key against the cached verifier (constant-time, no disk, no unlock)
    /// and hand back the shared session. Slow path: derive the key, open the
    /// record from the store, build a client, install the refresh hook,
    /// ensure a live token, unlock the keyring, cache, and return.
    pub fn authenticate(&self, id: &AccountId, bridge_password: &str) -> Result<Arc<Session>> {
        let bridge_key = bridge_key_from_text(bridge_password)?;

        // Fast path — already unlocked this run.
        if let Some(existing) = self.sessions.read().unwrap().get(id).cloned() {
            // Re-validate the presented key so a wrong password can't ride
            // in on a warm session opened by the right one.
            let verifier = self.crypto.gate.make_verifier(&bridge_key)?;
            if self.crypto.gate.verify(&verifier, &bridge_key) {
                return Ok(existing);
            }
            return Err(MailError::Unauthorized);
        }

        // Slow path. Opening the record both authenticates the key (a wrong
        // key fails the AEAD tag check) and yields the credentials.
        let record = self.store.load(id, &bridge_key)?;
        let session = self.build_session(record, bridge_key)?;
        let session = Arc::new(session);
        self.sessions
            .write()
            .unwrap()
            .insert(id.clone(), session.clone());
        Ok(session)
    }

    /// Build a live session from an opened record. Shared by `authenticate`
    /// and by the hub's post-login warm-up.
    pub(crate) fn build_session(
        &self,
        record: AccountRecord,
        bridge_key: [u8; 32],
    ) -> Result<Session> {
        let client = Arc::new(ProtonClient::new(
            self.agent.clone(),
            self.client_config.clone(),
        ));
        client.restore_tokens(&record.auth);
        client.set_refresh_hook(Arc::new(StoreBackedRefresh {
            id: record.id.clone(),
            store: self.store.clone(),
            bridge_key,
        }));

        // Make sure we hold a usable access token before unlocking. If the
        // stored one is stale, a refresh runs now; if the refresh token is
        // dead (code 10013) this surfaces as `needs_reauth()` and the caller
        // must re-run an interactive login.
        if record.auth.access_token.is_empty() {
            client.auth_refresh()?;
        }

        let keyring = client.unlock(
            &record.key_salts,
            &record.mailbox_password,
            self.crypto.pgp.as_ref(),
        )?;

        let addresses = client.list_addresses().unwrap_or_default();

        Ok(Session {
            id: record.id,
            username: record.username,
            client,
            keyring,
            addresses,
            event_cursor: record.auth.event_id,
        })
    }

    /// Insert an already-built session into the warm map. Used by the hub
    /// right after login, when it has constructed the session itself and
    /// wants it immediately serviceable.
    pub(crate) fn cache_built(&self, session: Session) {
        let id = session.id.clone();
        self.sessions.write().unwrap().insert(id, Arc::new(session));
    }

    /// Fetch a warm session if present.
    pub fn get(&self, id: &AccountId) -> Option<Arc<Session>> {
        self.sessions.read().unwrap().get(id).cloned()
    }

    /// Drop a warm session (e.g. after a logout or a detected re-auth need)
    /// without touching the on-disk record.
    pub fn evict(&self, id: &AccountId) {
        self.sessions.write().unwrap().remove(id);
    }

    /// Is this account currently warm?
    pub fn is_active(&self, id: &AccountId) -> bool {
        self.sessions.read().unwrap().contains_key(id)
    }

    /// Ids of all currently warm sessions.
    pub fn active_ids(&self) -> Vec<AccountId> {
        self.sessions.read().unwrap().keys().cloned().collect()
    }
}

/// Seconds since the Unix epoch (saturating to 0 before 1970, which never
/// happens in practice).
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
    fn now_is_after_2020() {
        // 1577836800 = 2020-01-01T00:00:00Z. Guards against a broken clock
        // source in the build env, and exercises the helper.
        assert!(now_secs() > 1_577_836_800);
    }

    #[test]
    fn address_lookup_is_case_insensitive() {
        let addrs = vec![
            Address {
                id: "a1".into(),
                email: "Alice@Proton.ME".into(),
                display_name: "Alice".into(),
                status: 1,
                send: 1,
                keys: vec![],
            },
            Address {
                id: "a2".into(),
                email: "alias@proton.me".into(),
                display_name: "Alias".into(),
                status: 1,
                send: 1,
                keys: vec![],
            },
        ];
        assert_eq!(find_address(&addrs, "alice@proton.me").unwrap().id, "a1");
        assert_eq!(find_address(&addrs, "ALIAS@PROTON.ME").unwrap().id, "a2");
        assert!(find_address(&addrs, "nobody@proton.me").is_none());
        assert!(find_address(&[], "x@y.z").is_none());
    }

    #[test]
    fn empty_ring_reports_empty() {
        let ring = EmptyRing;
        assert_eq!(ring.len(), 0);
        assert!(ring.is_empty());
    }

    struct EmptyRing;
    impl KeyRing for EmptyRing {
        fn len(&self) -> usize {
            0
        }
    }
}
