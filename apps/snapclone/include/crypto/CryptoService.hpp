#pragma once

// ─── Crypto/CryptoService.hpp ────────────────────────────────────────────────
//
// Provides end-to-end encryption for snap payloads.
//
// Scheme (Signal-protocol-inspired, simplified):
//   • Each user generates a Curve25519 identity key pair at registration.
//   • A new AES-256-GCM session key is derived (ECDH) per conversation.
//   • Snaps are encrypted with the session key; only the symmetric ciphertext
//     is stored locally or transmitted.
//   • Key material is stored in the OS keychain (via QSettings + DPAPI/libsecret
//     on Windows/Linux) — never in cleartext on disk.
//
// In this skeleton we use Qt's QCA (Qt Cryptographic Architecture) or the
// OpenSSL backend exposed through QSslConfiguration.  The actual algorithm
// calls are isolated in CryptoService so they can be swapped without
// affecting any other module.
//
// Threading: all operations run synchronously on the caller's thread.
// For large payloads consider calling from a thread pool via QtConcurrent::run.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QByteArray>
#include <QString>

#include "core/Types.hpp"
#include "core/Result.hpp"
#include "crypto/ICryptoService.hpp"

namespace SnapClone::Crypto {

class CryptoService : public QObject, public ICryptoService
{
    Q_OBJECT

public:
    explicit CryptoService(QObject *parent = nullptr);

    // ── Key management ────────────────────────────────────────────────────

    /// Generate and persist a new identity key pair for the local user.
    [[nodiscard]] bool generateIdentityKeyPair(const Core::UserId &userId) override;

    /// Load our public key (to be published to peers).
    [[nodiscard]] QByteArray publicIdentityKey(const Core::UserId &userId) const override;

    /// Register a remote peer's public key.
    void registerPeerPublicKey(const Core::UserId &peerId, const QByteArray &pubKey) override;

    /// Tell the service who the local user is so it can load their private key.
    /// Must be called before registerPeerPublicKey().
    void setSelfUserId(const Core::UserId &userId) override;

    // ── Snap encryption / decryption ──────────────────────────────────────

    /// Encrypt plaintext for recipientId.  Returns an error string on failure.
    [[nodiscard]] bool
        hasSessionKey(const Core::UserId &peerId) const override;
    [[nodiscard]] SnapClone::Result<EncryptResult, QString>
        encryptFor(const Core::UserId &recipientId, const QByteArray &plaintext) const override;

    /// Decrypt ciphertext from senderId.
    [[nodiscard]] SnapClone::Result<DecryptResult, QString>
        decryptFrom(const Core::UserId &senderId, const QByteArray &ciphertext,
                    const QByteArray &nonce) const override;

    // ── Signing / verification ────────────────────────────────────────────

    /// Sign data with our identity key (for gossip message authentication).
    [[nodiscard]] QByteArray sign(const Core::UserId &selfId, const QByteArray &data) const override;

    /// Verify a peer's signature.
    [[nodiscard]] bool verify(const Core::UserId &peerId,
                              const QByteArray   &data,
                              const QByteArray   &signature) const override;

private:
    // Per-peer session key cache: peerId.value → 32-byte AES key.
    // In production this would be the full Double Ratchet state.
    QHash<QString, QByteArray> m_sessionKeys;

    // The local user's identity, set by AppController after login.
    // Required to load our private key when deriving session keys.
    Core::UserId m_selfId;

    // Identity key storage path helper.
    [[nodiscard]] QString keyPath(const Core::UserId &userId) const;

    /// Derive a symmetric session key from two public keys (sorted lexicographically).
    [[nodiscard]] QByteArray deriveSessionKey(const QByteArray &ourPubKey,
                                              const QByteArray &theirPubKey) const;
};

} // namespace SnapClone::Crypto
