//! v0.1.861: typed errors for the ProtonMail bridge.
//!
//! hydroxide threads `fmt.Errorf` strings everywhere and special-cases
//! the "invalid refresh token" path with a bare magic number
//! (`apiErr.Code == 10013`). The rest of web2qml tends to use
//! `Result<_, String>`. Neither lets a caller branch on *why* something
//! failed without string-matching.
//!
//! This is the one place the mail module departs from the surrounding
//! `Result<_, String>` convention on purpose: a bridge has to make real
//! control-flow decisions on the error kind — "refresh token dead, fall
//! back to a full re-auth" vs. "wrong bridge password, reject the IMAP
//! login" vs. "the crypto feature isn't compiled, so we can't unseal".
//! A typed enum makes those decisions total and greppable, and it still
//! `impl Display`s into the `String` errors the rest of the crate
//! expects, so interop costs nothing (`map_err(|e| e.to_string())`).

use std::fmt;

/// Result alias used throughout the mail module.
pub type Result<T> = std::result::Result<T, MailError>;

/// Well-known ProtonMail API response codes the bridge reacts to. Proton
/// returns a numeric `Code` on every response; `1000`/`1001` mean success
/// and anything else carries an `Error` string. We name only the codes we
/// actually branch on and keep the rest as the raw number.
pub mod code {
    /// Refresh token no longer valid — triggers a full re-auth.
    /// (hydroxide hard-codes this as `10013`.)
    pub const INVALID_REFRESH_TOKEN: i64 = 10013;
    /// Generic success.
    pub const OK: i64 = 1000;
    /// Success with a body (used by some list endpoints).
    pub const OK_WITH_BODY: i64 = 1001;
}

#[derive(Debug)]
pub enum MailError {
    /// Transport-level failure (DNS, TLS, connection reset, timeout).
    Transport(String),

    /// The API answered, but with a non-success `Code`. `code` is the raw
    /// Proton code so callers can match on [`code::INVALID_REFRESH_TOKEN`]
    /// et al. without re-parsing.
    Api { code: i64, message: String },

    /// Response body was not the JSON shape we expected.
    Decode(String),

    /// Server's SRP proof did not verify — a hard stop, since it means the
    /// endpoint is not the ProtonMail server we think it is.
    SrpServerProof,

    /// The supplied bridge password was wrong, or the account is unknown.
    /// Deliberately opaque (no "user exists but password wrong" leak),
    /// matching hydroxide's single `ErrUnauthorized`.
    Unauthorized,

    /// Two-factor is required/enabled and we can't proceed non-interactively
    /// (e.g. a cached re-auth hit a 2FA wall).
    TwoFactorRequired,

    /// A security primitive the operation needs isn't available in this
    /// build. Mirrors the `llamacpp`/`wavenet` "not built with X" stubs:
    /// when the `mail-crypto` feature is off, sealing credentials to disk,
    /// SRP login, and PGP unlock all surface this instead of doing
    /// something unsafe.
    CryptoUnavailable(&'static str),

    /// Failed to unlock any private key with the given passphrase.
    KeyUnlock(String),

    /// Filesystem / persistence error in the account store.
    Store(String),

    /// Persistence is intentionally disabled (hardened default / not opted
    /// in), so credentials were not written to disk. This is not a failure
    /// of the operation per se — the session still works in memory — so
    /// callers (e.g. login) treat it as a soft, recoverable condition rather
    /// than aborting. See `mail_secure::PolicySecretBox`.
    PersistenceDisabled,

    /// Catch-all with context.
    Other(String),
}

impl MailError {
    /// True when the right recovery is to discard the cached session and
    /// run a fresh SRP login. Currently just the dead-refresh-token case,
    /// but isolating the predicate keeps the session manager readable.
    pub fn needs_reauth(&self) -> bool {
        matches!(
            self,
            MailError::Api {
                code: code::INVALID_REFRESH_TOKEN,
                ..
            }
        )
    }

    pub fn is_unauthorized(&self) -> bool {
        matches!(self, MailError::Unauthorized)
    }

    /// True when the only thing that went wrong is that persistence is off,
    /// so the account is usable for this session but won't survive a restart.
    pub fn is_persistence_disabled(&self) -> bool {
        matches!(self, MailError::PersistenceDisabled)
    }
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MailError::Transport(m) => write!(f, "transport error: {m}"),
            MailError::Api { code, message } => write!(f, "API error [{code}]: {message}"),
            MailError::Decode(m) => write!(f, "decode error: {m}"),
            MailError::SrpServerProof => write!(f, "SRP server proof verification failed"),
            MailError::Unauthorized => write!(f, "invalid username or bridge password"),
            MailError::TwoFactorRequired => {
                write!(f, "two-factor authentication required; log in again manually")
            }
            MailError::CryptoUnavailable(what) => {
                write!(f, "not built with the `mail-crypto` feature: {what} unavailable")
            }
            MailError::KeyUnlock(m) => write!(f, "key unlock failed: {m}"),
            MailError::Store(m) => write!(f, "account store error: {m}"),
            MailError::PersistenceDisabled => write!(
                f,
                "persistence disabled; account kept in memory only for this session"
            ),
            MailError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for MailError {}

impl From<std::io::Error> for MailError {
    fn from(e: std::io::Error) -> Self {
        MailError::Store(e.to_string())
    }
}

/// JSON parse failures come back as `String` from `mail_json`; treat them
/// as decode errors.
impl From<String> for MailError {
    fn from(e: String) -> Self {
        MailError::Decode(e)
    }
}
