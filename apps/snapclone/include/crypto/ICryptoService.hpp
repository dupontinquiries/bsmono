#pragma once

// ─── Crypto/ICryptoService.hpp ────────────────────────────────────────────────
//
// Abstract interface implemented by both CryptoService (Qt placeholder) and
// RustCryptoService (Rust AES-256-GCM backend).
//
// AppController holds unique_ptr<ICryptoService> and delegates all crypto
// operations through this interface.  Switching backends requires only a
// recompile with -DSNAPCLONE_RUST_CRYPTO=ON; no other code changes.
// ─────────────────────────────────────────────────────────────────────────────

#include "core/Types.hpp"
#include "core/Result.hpp"
#include <QByteArray>
#include <QString>

namespace SnapClone::Crypto {

struct EncryptResult {
    QByteArray ciphertext;
    QByteArray nonce; ///< 12-byte nonce, prepended to ciphertext on the wire
};

struct DecryptResult {
    QByteArray plaintext;
};

class ICryptoService {
public:
    virtual ~ICryptoService() = default;

    [[nodiscard]] virtual bool       generateIdentityKeyPair(const Core::UserId &userId) = 0;
    [[nodiscard]] virtual QByteArray publicIdentityKey(const Core::UserId &userId) const = 0;
    virtual void                     setSelfUserId(const Core::UserId &userId)          = 0;
    [[nodiscard]] virtual bool       hasSessionKey(const Core::UserId &peerId) const = 0;
    virtual void                     registerPeerPublicKey(const Core::UserId &peerId,
                                                            const QByteArray   &pubKey)  = 0;

    [[nodiscard]] virtual SnapClone::Result<EncryptResult, QString>
        encryptFor(const Core::UserId &recipientId, const QByteArray &plaintext) const  = 0;

    [[nodiscard]] virtual SnapClone::Result<DecryptResult, QString>
        decryptFrom(const Core::UserId &senderId, const QByteArray &ciphertext,
                    const QByteArray   &nonce) const                                     = 0;

    [[nodiscard]] virtual QByteArray sign(const Core::UserId &selfId,
                                          const QByteArray   &data) const               = 0;
    [[nodiscard]] virtual bool       verify(const Core::UserId &peerId,
                                            const QByteArray   &data,
                                            const QByteArray   &signature) const        = 0;
};

} // namespace SnapClone::Crypto
