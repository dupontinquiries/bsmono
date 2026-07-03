#pragma once
// ─── AccountManager.hpp ──────────────────────────────────────────────────────
//
// Manages the set of local accounts (identities) and which one is active.
//
// # What an account is
//
// An account is a node identity: an "sc-XXXXXXXX" id plus its key files under
// <AppDataLocation>/keys/<id>.* (X25519, Ed25519, …). Because the crypto layer
// already keys every file by id, multiple accounts coexist on disk with no
// crypto changes — each account's keys live in their own files, and switching
// accounts just changes which id the app loads at startup.
//
// # Two features this provides
//
//   * Reroll — create a fresh identity and make it active WITHOUT touching
//     QSettings. Settings (audio, network, retention, …) live in QSettings,
//     which this class never reads or writes, so a reroll keeps every
//     preference. The previous identity is kept in the registry (not deleted),
//     so a reroll is reversible — switch back any time.
//
//   * Multiple accounts — keep several identities and choose the active one.
//
// # How a switch/reroll takes effect
//
// AppController reads the active id once at startup (via Identity::loadOrGenerate,
// which now returns activeId()). So setActive() / rerollActive() persist the
// choice and it applies on the next launch. A live, in-session switch (tearing
// down and re-keying the network stack without a restart) is a larger,
// separate change and is intentionally not done here.
//
// # Persistence
//
// The registry is a small JSON file, <AppDataLocation>/accounts.json, kept
// separate from QSettings and from the keys directory. For backward
// compatibility the active id is also mirrored to the legacy identity.txt, and
// an existing identity.txt is migrated into the registry on first run.

#include <QString>
#include <QList>
#include <QtGlobal>

namespace SnapClone::Core {

struct Account {
    QString id;            ///< "sc-XXXXXXXX"
    QString label;         ///< user-facing name; defaults to the id
    qint64  createdAt = 0; ///< epoch seconds
};

class AccountManager {
public:
    /// Process-wide instance. Loads (and migrates/seeds) the registry on first use.
    static AccountManager &instance();

    /// The active account id. This is what Identity::loadOrGenerate() returns.
    QString        activeId();
    Account        active();
    QList<Account> accounts();
    int            count();

    /// Create a NEW account (fresh id; its keys are generated lazily by the
    /// crypto layer at the next startup that loads it). Added to the registry
    /// but NOT made active. Returns the new id.
    QString createAccount(const QString &label = QString());

    /// Make an existing account active. Applies on the next launch. Returns
    /// false if the id is unknown.
    bool setActive(const QString &id);

    /// Create a new account AND make it active — a "reroll". QSettings are left
    /// untouched, and the prior account is retained. Returns the new id.
    QString rerollActive(const QString &label = QString());

    /// Rename an account (label only; the id is immutable).
    bool renameAccount(const QString &id, const QString &label);

    /// Remove an account and delete its key files. Refuses to remove the active
    /// account (switch first) or the last remaining one. Returns false if not
    /// allowed or unknown.
    bool removeAccount(const QString &id);

private:
    AccountManager() { load(); }
    AccountManager(const AccountManager &) = delete;
    AccountManager &operator=(const AccountManager &) = delete;

    void load();
    void save();
    void ensureLoaded();
    bool idExists(const QString &id) const;
    void syncIdentityTxt();                 // mirror active id to legacy identity.txt
    void deleteKeyFiles(const QString &id); // remove keys/<id>.*

    static QString generateId();
    static QString appDataDir();
    static QString accountsPath();
    static QString identityTxtPath();
    static QString keysDir();

    QList<Account> m_accounts;
    QString        m_activeId;
    bool           m_loaded = false;
};

} // namespace SnapClone::Core
