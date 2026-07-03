// ─── Crypto/RustCryptoService.cpp ─────────────────────────────────────────────
//
// C++ bridge to the snapclone-crypto Rust static library.
//
// Build with: cmake -B build -DSNAPCLONE_RUST_CRYPTO=ON
//   Requires: cargo, cbindgen, and the static lib pre-built:
//     cd rust/snapclone-crypto && cargo build --release
//
// Key file format:
//   <AppDataLocation>/keys/<userId.value>.priv  — 32-byte X25519 scalar, hex-encoded
//   <AppDataLocation>/keys/<userId.value>.pub   — 32-byte X25519 public key, hex-encoded
//
// Wire protocol compatibility:
//   Encrypt output: [12-byte nonce][ciphertext][16-byte GCM tag]
//   This layout is identical to the Qt placeholder so both backends
//   can decrypt each other's messages (useful during migration).
//
// Threading:
//   All operations run synchronously on the caller's thread.
//   The Rust functions are not internally threaded.
// ─────────────────────────────────────────────────────────────────────────────
#ifdef SNAPCLONE_RUST_CRYPTO

#include "crypto/RustCryptoService.hpp"

#include <QStandardPaths>
#include <QDir>
#include <QFile>
#include <QLoggingCategory>

// Portable fallback: cbindgen variant naming differs across versions/platforms.
// Use the macro aliases from snapclone_crypto_ffi.h if available; otherwise
// define them here as integer casts which are always correct for #[repr(C)] enums.
#ifndef SNAPCLONE_CRYPTO_ALIASES_DEFINED
#  define SNAPCLONE_CRYPTO_ALIASES_DEFINED
#  define CryptoError_Ok               ((CryptoError)0)
#  define CryptoError_InvalidInput     ((CryptoError)1)
#  define CryptoError_EncryptFailed    ((CryptoError)2)
#  define CryptoError_DecryptFailed    ((CryptoError)3)
#  define CryptoError_InvalidSignature ((CryptoError)4)
#  define CryptoError_BufferTooSmall   ((CryptoError)5)
#endif


Q_LOGGING_CATEGORY(lcRustCrypto, "snapclone.crypto.rust")

namespace SnapClone::Crypto {

// ─── Construction ─────────────────────────────────────────────────────────────

RustCryptoService::RustCryptoService(QObject *parent)
    : QObject(parent)
{}

// Out-of-line dtor: vtable/typeinfo anchor (see header). Empty body — the
// unique_ptr members (if any) and QObject base handle their own cleanup.
RustCryptoService::~RustCryptoService() = default;

// ─── Helpers ─────────────────────────────────────────────────────────────────

QString RustCryptoService::keyPath(const Core::UserId &userId) const
{
    // Sanitize: keep only chars safe in filenames on all platforms.
    // userId.value for peers comes from the wire (SHAK handshake).
    // Without this, a peer ID of "../../etc/passwd" would escape the keys dir.
    QString safe;
    safe.reserve(userId.value.size());
    for (const QChar c : userId.value) {
        if (c.isLetterOrNumber() || c == QLatin1Char('@')
         || c == QLatin1Char('-')  || c == QLatin1Char('.')) {
            safe.append(c);
        } else {
            safe.append(QLatin1Char('_'));
        }
    }
    if (safe.isEmpty()) safe = QStringLiteral("unknown");
    return QStandardPaths::writableLocation(QStandardPaths::AppDataLocation)
           + QStringLiteral("/keys/") + safe;
}

QByteArray RustCryptoService::loadPrivKey(const Core::UserId &userId) const
{
    QFile f(keyPath(userId) + QStringLiteral(".priv"));
    if (!f.open(QIODevice::ReadOnly)) return {};
    return QByteArray::fromHex(f.readAll().trimmed());
}

// ─── Key management ───────────────────────────────────────────────────────────

bool RustCryptoService::generateIdentityKeyPair(const Core::UserId &userId)
{
    const QString dir =
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation)
        + QStringLiteral("/keys");
    QDir().mkpath(dir);

    // Generate X25519 keypair via Rust FFI.
    QByteArray privBuf(static_cast<int>(sc_priv_key_len()), Qt::Uninitialized);
    QByteArray pubBuf (static_cast<int>(sc_pub_key_len()),  Qt::Uninitialized);

    const CryptoError err = sc_generate_x25519_keypair(
        reinterpret_cast<uint8_t *>(privBuf.data()),
        reinterpret_cast<uint8_t *>(pubBuf.data()));

    if (err != CryptoError_Ok) {
        qCCritical(lcRustCrypto) << "X25519 keygen failed:" << static_cast<int>(err);
        return false;
    }

    // Use keyPath() to sanitize userId.value — prevents path traversal
    // if a malicious peer ID like "../../etc" is ever used as a local identity.
    const QString safeBase = keyPath(userId);
    auto write = [&](const QString &suffix, const QByteArray &data) -> bool {
        QFile f(safeBase + suffix);
        if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            qCCritical(lcRustCrypto) << "Cannot write key file:" << f.fileName();
            return false;
        }
        f.setPermissions(QFileDevice::ReadOwner | QFileDevice::WriteOwner);
        f.write(data.toHex());
        return true;
    };

    if (!write(QStringLiteral(".priv"), privBuf)) return false;
    if (!write(QStringLiteral(".pub"),  pubBuf))  return false;

    qCInfo(lcRustCrypto) << "Generated X25519 identity keypair for" << userId.value;
    return true;
}

QByteArray RustCryptoService::publicIdentityKey(const Core::UserId &userId) const
{
    QFile f(keyPath(userId) + QStringLiteral(".pub"));
    if (!f.open(QIODevice::ReadOnly)) return {};
    return QByteArray::fromHex(f.readAll().trimmed());
}

void RustCryptoService::registerPeerPublicKey(const Core::UserId &peerId,
                                               const QByteArray   &pubKey)
{
    if (pubKey.size() != static_cast<int>(sc_pub_key_len())) {
        qCWarning(lcRustCrypto) << "Invalid pubkey size for" << peerId.value;
        return;
    }

    // Load our own private key to perform X25519 ECDH.
    // m_selfId is set by the parent AppController before any peer registration.
    if (m_selfId.isNull()) {
        qCWarning(lcRustCrypto) << "registerPeerPublicKey called before setSelfUserId";
        return;
    }

    const QByteArray ourPriv = loadPrivKey(m_selfId);
    if (ourPriv.size() != static_cast<int>(sc_priv_key_len())) {
        qCWarning(lcRustCrypto) << "Cannot load private key for" << m_selfId.value;
        return;
    }

    // Derive session key: X25519(ourPriv, peerPub) → HKDF-SHA256 → 32-byte AES key.
    QByteArray sessionKey(static_cast<int>(sc_key_len()), Qt::Uninitialized);
    const CryptoError err = sc_derive_session_key(
        reinterpret_cast<const uint8_t *>(ourPriv.constData()),
        reinterpret_cast<const uint8_t *>(pubKey.constData()),
        reinterpret_cast<uint8_t *>(sessionKey.data()));

    if (err != CryptoError_Ok) {
        qCWarning(lcRustCrypto) << "ECDH session key derivation failed for" << peerId.value;
        return;
    }

    m_sessionKeys[peerId.value] = sessionKey;
    qCInfo(lcRustCrypto) << "ECDH session key derived for" << peerId.value;
}

void RustCryptoService::setSelfUserId(const Core::UserId &userId)
{
    m_selfId = userId;
}

// ─── Encrypt / Decrypt ────────────────────────────────────────────────────────

bool RustCryptoService::hasSessionKey(const Core::UserId &peerId) const
{
    return m_sessionKeys.contains(peerId.value);
}

SnapClone::Result<EncryptResult, QString>
RustCryptoService::encryptFor(const Core::UserId &recipientId,
                               const QByteArray   &plaintext) const
{
    const auto it = m_sessionKeys.find(recipientId.value);
    if (it == m_sessionKeys.end()) {
        return makeUnexpected(
            QStringLiteral("No session key for %1 — handshake not completed?")
                .arg(recipientId.value));
    }

    const QByteArray &key = *it;
    // Output: [12-byte nonce][ciphertext][16-byte GCM tag]
    const qsizetype cipherCap =
        static_cast<qsizetype>(sc_nonce_len())
        + plaintext.size()
        + 16; // GCM tag

    QByteArray cipherBuf(static_cast<int>(cipherCap), Qt::Uninitialized);
    uintptr_t  writtenLen = 0;

    const CryptoError err = sc_encrypt_aes256gcm(
        reinterpret_cast<const uint8_t *>(key.constData()),
        reinterpret_cast<const uint8_t *>(plaintext.constData()),
        static_cast<uintptr_t>(plaintext.size()),
        reinterpret_cast<uint8_t *>(cipherBuf.data()),
        static_cast<uintptr_t>(cipherCap),
        &writtenLen);

    if (err != CryptoError_Ok) {
        return makeUnexpected(QStringLiteral("AES-256-GCM encryption failed (error %1)")
                                  .arg(static_cast<int>(err)));
    }

    cipherBuf.resize(static_cast<int>(writtenLen));

    // Split nonce from the rest so SnapRouter can prepend it to the wire payload.
    const int nonceLen = static_cast<int>(sc_nonce_len());
    EncryptResult result;
    result.nonce      = cipherBuf.left(nonceLen);
    result.ciphertext = cipherBuf.mid(nonceLen);
    return result;
}

SnapClone::Result<DecryptResult, QString>
RustCryptoService::decryptFrom(const Core::UserId &senderId,
                                const QByteArray   &ciphertext,
                                const QByteArray   &nonce) const
{
    const auto it = m_sessionKeys.find(senderId.value);
    if (it == m_sessionKeys.end()) {
        return makeUnexpected(
            QStringLiteral("No session key for %1 — handshake not completed?")
                .arg(senderId.value));
    }

    const QByteArray &key = *it;

    // nonce and ciphertext are already contiguous in the original wire buffer
    // (SnapRouter splits them with fromRawData — no copy was made).
    // Reassemble by concatenating only if they are not already adjacent;
    // in practice we always allocate a small join buffer here.
    // plaintext is always shorter than wire (nonce stripped + tag removed).
    const qsizetype wireLen  = nonce.size() + ciphertext.size();
    const qsizetype plainCap = wireLen;

    // Build wire buffer without an intermediate QByteArray copy:
    // preallocate to exact size then append.
    QByteArray wire;
    wire.reserve(wireLen);
    wire.append(nonce);
    wire.append(ciphertext);

    QByteArray plainBuf(static_cast<int>(plainCap), Qt::Uninitialized);
    uintptr_t  writtenLen = 0;

    const CryptoError err = sc_decrypt_aes256gcm(
        reinterpret_cast<const uint8_t *>(key.constData()),
        reinterpret_cast<const uint8_t *>(wire.constData()),
        static_cast<uintptr_t>(wire.size()),
        reinterpret_cast<uint8_t *>(plainBuf.data()),
        static_cast<uintptr_t>(plainCap),
        &writtenLen);

    if (err != CryptoError_Ok) {
        const QString reason = (err == CryptoError_DecryptFailed)
            ? QStringLiteral("MAC verification failed — wrong session key or corrupted payload")
            : QStringLiteral("Decryption failed (error %1)").arg(static_cast<int>(err));
        return makeUnexpected(reason);
    }

    plainBuf.resize(static_cast<int>(writtenLen));
    return DecryptResult{ plainBuf };
}

// ─── Sign / Verify ────────────────────────────────────────────────────────────

QByteArray RustCryptoService::sign(const Core::UserId &selfId,
                                    const QByteArray   &data) const
{
    // Ed25519 signing requires the full 64-byte expanded key.
    // We generate Ed25519 keypairs separately from X25519 (different algorithms).
    // Key file: <userId>.ed25519.priv (32-byte seed, hex-encoded).
    const QString edPrivPath = keyPath(selfId) + QStringLiteral(".ed25519.priv");
    QFile f(edPrivPath);
    if (!f.open(QIODevice::ReadOnly)) {
        qCWarning(lcRustCrypto) << "Ed25519 private key not found at" << edPrivPath;
        return {};
    }
    const QByteArray privKey = QByteArray::fromHex(f.readAll().trimmed());
    if (privKey.size() != static_cast<int>(sc_priv_key_len())) return {};

    QByteArray sig(static_cast<int>(sc_sig_len()), Qt::Uninitialized);
    const CryptoError err = sc_sign_ed25519(
        reinterpret_cast<const uint8_t *>(privKey.constData()),
        reinterpret_cast<const uint8_t *>(data.constData()),
        static_cast<uintptr_t>(data.size()),
        reinterpret_cast<uint8_t *>(sig.data()));

    return (err == CryptoError_Ok) ? sig : QByteArray{};
}

bool RustCryptoService::verify(const Core::UserId &peerId,
                                const QByteArray   &data,
                                const QByteArray   &signature) const
{
    const QString edPubPath = keyPath(peerId) + QStringLiteral(".ed25519.pub");
    QFile f(edPubPath);
    if (!f.open(QIODevice::ReadOnly)) return false;
    const QByteArray pubKey = QByteArray::fromHex(f.readAll().trimmed());
    if (pubKey.size()    != static_cast<int>(sc_pub_key_len())) return false;
    if (signature.size() != static_cast<int>(sc_sig_len()))     return false;

    const CryptoError err = sc_verify_ed25519(
        reinterpret_cast<const uint8_t *>(pubKey.constData()),
        reinterpret_cast<const uint8_t *>(data.constData()),
        static_cast<uintptr_t>(data.size()),
        reinterpret_cast<const uint8_t *>(signature.constData()));

    return err == CryptoError_Ok;
}

// ─── V32.33+ Post-quantum hybrid KEM ─────────────────────────────────────────

std::pair<QByteArray, QByteArray> RustCryptoService::mlkemGenerateEphemeral() const
{
    const int dkLen = static_cast<int>(sc_mlkem_dk_len());  // 2400
    const int ekLen = static_cast<int>(sc_mlkem_ek_len());  // 1184
    QByteArray dk(dkLen, Qt::Uninitialized);
    QByteArray ek(ekLen, Qt::Uninitialized);

    const CryptoError err = sc_mlkem_generate_keypair(
        reinterpret_cast<uint8_t *>(dk.data()),
        reinterpret_cast<uint8_t *>(ek.data()));

    if (err != CryptoError_Ok) {
        qCWarning(lcRustCrypto) << "ML-KEM keypair generation failed (err="
                                << static_cast<int>(err) << ")";
        return { QByteArray(), QByteArray() };
    }
    return { dk, ek };
}

std::pair<QByteArray, QByteArray>
RustCryptoService::mlkemEncapsulate(const QByteArray &peerEncapsulationKey) const
{
    const int ekLen = static_cast<int>(sc_mlkem_ek_len());  // 1184
    const int ctLen = static_cast<int>(sc_mlkem_ct_len());  // 1088
    const int ssLen = static_cast<int>(sc_mlkem_ss_len());  // 32
    if (peerEncapsulationKey.size() != ekLen) {
        qCWarning(lcRustCrypto) << "ML-KEM encap: wrong ek size"
                                << peerEncapsulationKey.size()
                                << "expected" << ekLen;
        return { QByteArray(), QByteArray() };
    }
    QByteArray ct(ctLen, Qt::Uninitialized);
    QByteArray ss(ssLen, Qt::Uninitialized);

    const CryptoError err = sc_mlkem_encapsulate(
        reinterpret_cast<const uint8_t *>(peerEncapsulationKey.constData()),
        reinterpret_cast<uint8_t *>(ct.data()),
        reinterpret_cast<uint8_t *>(ss.data()));

    if (err != CryptoError_Ok) {
        qCWarning(lcRustCrypto) << "ML-KEM encapsulation failed (err="
                                << static_cast<int>(err) << ")";
        return { QByteArray(), QByteArray() };
    }
    return { ct, ss };
}

QByteArray
RustCryptoService::mlkemDecapsulate(const QByteArray &ourDecapsulationKey,
                                    const QByteArray &peerCiphertext) const
{
    const int dkLen = static_cast<int>(sc_mlkem_dk_len());  // 2400
    const int ctLen = static_cast<int>(sc_mlkem_ct_len());  // 1088
    const int ssLen = static_cast<int>(sc_mlkem_ss_len());  // 32
    if (ourDecapsulationKey.size() != dkLen) {
        qCWarning(lcRustCrypto) << "ML-KEM decap: wrong dk size"
                                << ourDecapsulationKey.size()
                                << "expected" << dkLen;
        return {};
    }
    if (peerCiphertext.size() != ctLen) {
        qCWarning(lcRustCrypto) << "ML-KEM decap: wrong ct size"
                                << peerCiphertext.size()
                                << "expected" << ctLen;
        return {};
    }
    QByteArray ss(ssLen, Qt::Uninitialized);

    const CryptoError err = sc_mlkem_decapsulate(
        reinterpret_cast<const uint8_t *>(ourDecapsulationKey.constData()),
        reinterpret_cast<const uint8_t *>(peerCiphertext.constData()),
        reinterpret_cast<uint8_t *>(ss.data()));

    if (err != CryptoError_Ok) {
        qCWarning(lcRustCrypto) << "ML-KEM decapsulation failed (err="
                                << static_cast<int>(err) << ")";
        // Note: FIPS 203 §6.3 specifies an "implicit rejection" mechanism —
        // even on malformed ciphertext, decapsulation returns a
        // deterministic pseudo-random key derived from the implicit-reject
        // value baked into the dk. So `err == DecryptFailed` typically
        // means the dk itself was invalid, not the ct.
        return {};
    }
    return ss;
}

QByteArray
RustCryptoService::deriveHybridSessionKey(const QByteArray &x25519Shared,
                                          const QByteArray &mlkemShared) const
{
    const int xLen = static_cast<int>(sc_key_len());        // 32
    const int mLen = static_cast<int>(sc_mlkem_ss_len());   // 32
    const int kLen = static_cast<int>(sc_key_len());        // 32
    if (x25519Shared.size() != xLen) {
        qCWarning(lcRustCrypto) << "hybrid derive: wrong x25519 size"
                                << x25519Shared.size() << "expected" << xLen;
        return {};
    }
    if (mlkemShared.size() != mLen) {
        qCWarning(lcRustCrypto) << "hybrid derive: wrong mlkem size"
                                << mlkemShared.size() << "expected" << mLen;
        return {};
    }
    QByteArray out(kLen, Qt::Uninitialized);
    const CryptoError err = sc_derive_hybrid_session_key(
        reinterpret_cast<const uint8_t *>(x25519Shared.constData()),
        reinterpret_cast<const uint8_t *>(mlkemShared.constData()),
        reinterpret_cast<uint8_t *>(out.data()));
    if (err != CryptoError_Ok) {
        qCWarning(lcRustCrypto) << "hybrid session key derivation failed";
        return {};
    }
    return out;
}

QByteArray RustCryptoService::x25519RawShared(const QByteArray &ourPriv,
                                              const QByteArray &peerPub) const
{
    const int privLen = static_cast<int>(sc_priv_key_len());
    const int pubLen  = static_cast<int>(sc_pub_key_len());
    const int outLen  = static_cast<int>(sc_key_len());
    if (ourPriv.size() != privLen) {
        qCWarning(lcRustCrypto) << "raw shared: wrong priv size"
                                << ourPriv.size() << "expected" << privLen;
        return {};
    }
    if (peerPub.size() != pubLen) {
        qCWarning(lcRustCrypto) << "raw shared: wrong pub size"
                                << peerPub.size() << "expected" << pubLen;
        return {};
    }
    QByteArray out(outLen, Qt::Uninitialized);
    const CryptoError err = sc_x25519_raw_shared(
        reinterpret_cast<const uint8_t *>(ourPriv.constData()),
        reinterpret_cast<const uint8_t *>(peerPub.constData()),
        reinterpret_cast<uint8_t *>(out.data()));
    if (err != CryptoError_Ok) {
        qCWarning(lcRustCrypto) << "raw shared X25519 failed (err="
                                << static_cast<int>(err) << ")";
        return {};
    }
    return out;
}

std::pair<QByteArray, QByteArray>
RustCryptoService::respondHybridHandshake(const Core::UserId &selfId,
                                          const QByteArray   &peerX25519Pub,
                                          const QByteArray   &peerMlkemEk) const
{
    // Load our X25519 private to compute the classical half.
    const QByteArray ourPriv = loadPrivKey(selfId);
    if (ourPriv.size() != static_cast<int>(sc_priv_key_len())) {
        qCWarning(lcRustCrypto) << "respond: cannot load priv for" << selfId.value;
        return { QByteArray(), QByteArray() };
    }

    const QByteArray ssDh = x25519RawShared(ourPriv, peerX25519Pub);
    if (ssDh.isEmpty()) return { QByteArray(), QByteArray() };

    const auto [ct, ssPq] = mlkemEncapsulate(peerMlkemEk);
    if (ct.isEmpty() || ssPq.isEmpty()) {
        return { QByteArray(), QByteArray() };
    }

    const QByteArray sessionKey = deriveHybridSessionKey(ssDh, ssPq);
    if (sessionKey.isEmpty()) {
        return { QByteArray(), QByteArray() };
    }
    qCInfo(lcRustCrypto) << "respondHybridHandshake: produced ct (" << ct.size()
                         << "bytes) and session key (" << sessionKey.size()
                         << "bytes) for peer pub starting with"
                         << peerX25519Pub.left(4).toHex();
    return { ct, sessionKey };
}

QByteArray
RustCryptoService::finishHybridHandshake(const Core::UserId &selfId,
                                         const QByteArray   &peerX25519Pub,
                                         const QByteArray   &ourMlkemDk,
                                         const QByteArray   &peerMlkemCt) const
{
    const QByteArray ourPriv = loadPrivKey(selfId);
    if (ourPriv.size() != static_cast<int>(sc_priv_key_len())) {
        qCWarning(lcRustCrypto) << "finish: cannot load priv for" << selfId.value;
        return {};
    }
    const QByteArray ssDh = x25519RawShared(ourPriv, peerX25519Pub);
    if (ssDh.isEmpty()) return {};

    const QByteArray ssPq = mlkemDecapsulate(ourMlkemDk, peerMlkemCt);
    if (ssPq.isEmpty()) return {};

    const QByteArray sessionKey = deriveHybridSessionKey(ssDh, ssPq);
    if (sessionKey.isEmpty()) return {};

    qCInfo(lcRustCrypto) << "finishHybridHandshake: derived session key ("
                         << sessionKey.size() << "bytes) for peer pub starting with"
                         << peerX25519Pub.left(4).toHex();
    return sessionKey;
}

} // namespace SnapClone::Crypto

#endif // SNAPCLONE_RUST_CRYPTO
