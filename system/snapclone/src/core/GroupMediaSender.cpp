#include "core/GroupMediaSender.hpp"
#include "core/MediaMetadataStripper.hpp"

#include <QLoggingCategory>

#ifdef SNAPCLONE_RUST_MEDIASHARE
#include "mediashare/snapclone_mediashare_ffi.h"
#include <cstdint>
#endif

Q_LOGGING_CATEGORY(lcGroupMedia, "snapclone.core.groupmedia")

namespace SnapClone::Core {

GroupMediaSender::GroupMediaSender(int chunkSize, QObject *parent)
    : QObject(parent)
    , m_chunkSize(chunkSize > 0 ? chunkSize : 64 * 1024)
{
#ifdef SNAPCLONE_RUST_MEDIASHARE
    m_store = sc_chunk_store_new();
#endif
}

GroupMediaSender::~GroupMediaSender()
{
#ifdef SNAPCLONE_RUST_MEDIASHARE
    if (m_store) sc_chunk_store_free(static_cast<ChunkStore *>(m_store));
#endif
}

bool GroupMediaSender::isAvailable() const
{
#ifdef SNAPCLONE_RUST_MEDIASHARE
    return m_store != nullptr;
#else
    return false;
#endif
}

void GroupMediaSender::setRecipients(const QList<QByteArray> &recipientX25519PubKeys)
{
    m_recipients = recipientX25519PubKeys;
}

QByteArray GroupMediaSender::chunkBytes(const QByteArray &id32) const
{
#ifdef SNAPCLONE_RUST_MEDIASHARE
    if (id32.size() != 32 || !m_store) return {};
    auto *store = static_cast<ChunkStore *>(m_store);
    size_t need = 0;
    int rc = sc_chunk_store_get(store, reinterpret_cast<const uint8_t *>(id32.constData()),
                                nullptr, 0, &need);
    if (rc != -2 && rc != 0) return {};   // -3 not found / -1 null
    QByteArray out(static_cast<int>(need), Qt::Uninitialized);
    rc = sc_chunk_store_get(store, reinterpret_cast<const uint8_t *>(id32.constData()),
                            reinterpret_cast<uint8_t *>(out.data()),
                            static_cast<size_t>(out.size()), &need);
    if (rc != 0) return {};
    out.resize(static_cast<int>(need));
    return out;
#else
    Q_UNUSED(id32);
    return {};
#endif
}

void GroupMediaSender::prepare(const QByteArray &media, bool isImage)
{
#ifndef SNAPCLONE_RUST_MEDIASHARE
    Q_UNUSED(media);
    Q_UNUSED(isImage);
    emit prepareFailed(QStringLiteral(
        "media-share backend not built (rebuild with -DSNAPCLONE_RUST_MEDIASHARE=ON)"));
#else
    if (!m_store) { emit prepareFailed(QStringLiteral("chunk store unavailable")); return; }

    Prepared p;
    p.originalBytes = media.size();

    // 1. strip metadata (images only).
    QByteArray plain = media;
    if (isImage) {
        const StripOutcome s = MediaStrip::stripImage(media);
        plain          = s.data;
        p.stripFormat  = s.format;
        p.stripRemoved = s.removed;
    }
    p.strippedBytes = plain.size();

    // 2. fresh content key.
    QByteArray ck(32, Qt::Uninitialized);
    sc_media_content_key_new(reinterpret_cast<uint8_t *>(ck.data()));
    m_lastContentKey = ck;

    // 3. seal the media once with the content key (output is plain.size()+28).
    QByteArray sealed(plain.size() + 28, Qt::Uninitialized);
    size_t slen = 0;
    int rc = sc_media_seal(reinterpret_cast<const uint8_t *>(ck.constData()),
                           reinterpret_cast<const uint8_t *>(plain.constData()),
                           static_cast<size_t>(plain.size()),
                           reinterpret_cast<uint8_t *>(sealed.data()),
                           static_cast<size_t>(sealed.size()), &slen);
    if (rc != 0) { emit prepareFailed(QStringLiteral("media seal failed")); return; }
    sealed.resize(static_cast<int>(slen));
    p.sealedBytes = sealed.size();

    // 4. chunk the CIPHERTEXT into the persistent store → manifest. Two-call
    //    sizing; re-adding identical chunks is idempotent (content-addressed).
    auto *store = static_cast<ChunkStore *>(m_store);
    size_t mlen = 0;
    rc = sc_chunk_store_add_blob(store, reinterpret_cast<const uint8_t *>(sealed.constData()),
                                 static_cast<size_t>(sealed.size()),
                                 static_cast<size_t>(m_chunkSize), nullptr, 0, &mlen);
    if (rc != 0 && rc != -2) { emit prepareFailed(QStringLiteral("chunk sizing failed")); return; }
    QByteArray manifest(static_cast<int>(mlen), Qt::Uninitialized);
    rc = sc_chunk_store_add_blob(store, reinterpret_cast<const uint8_t *>(sealed.constData()),
                                 static_cast<size_t>(sealed.size()),
                                 static_cast<size_t>(m_chunkSize),
                                 reinterpret_cast<uint8_t *>(manifest.data()),
                                 static_cast<size_t>(manifest.size()), &mlen);
    if (rc != 0) { emit prepareFailed(QStringLiteral("chunk manifest failed")); return; }
    manifest.resize(static_cast<int>(mlen));
    p.manifest = manifest;

    // 5. wrap the content key for every recipient (concatenated 32-byte keys).
    QByteArray recips;
    for (const auto &r : m_recipients)
        if (r.size() == 32) recips.append(r);
    p.recipientCount = static_cast<int>(recips.size() / 32);

    size_t klen = 0;
    const uint8_t *rptr = recips.isEmpty()
                              ? nullptr
                              : reinterpret_cast<const uint8_t *>(recips.constData());
    rc = sc_media_keyshare_seal(reinterpret_cast<const uint8_t *>(ck.constData()),
                                rptr, static_cast<size_t>(p.recipientCount), nullptr, 0, &klen);
    if (rc != 0 && rc != -2) { emit prepareFailed(QStringLiteral("keyshare sizing failed")); return; }
    QByteArray env(static_cast<int>(klen), Qt::Uninitialized);
    rc = sc_media_keyshare_seal(reinterpret_cast<const uint8_t *>(ck.constData()),
                                rptr, static_cast<size_t>(p.recipientCount),
                                reinterpret_cast<uint8_t *>(env.data()),
                                static_cast<size_t>(env.size()), &klen);
    if (rc != 0) { emit prepareFailed(QStringLiteral("keyshare seal failed")); return; }
    env.resize(static_cast<int>(klen));
    p.keyShareEnvelope = env;

    p.ok = true;
    qCInfo(lcGroupMedia) << "prepared group media:" << p.originalBytes << "→ stripped"
                         << p.strippedBytes << "→ sealed" << p.sealedBytes << "bytes,"
                         << p.recipientCount << "recipients," << p.stripRemoved
                         << "metadata segments removed";
    emit prepared(p);
#endif
}

} // namespace SnapClone::Core
