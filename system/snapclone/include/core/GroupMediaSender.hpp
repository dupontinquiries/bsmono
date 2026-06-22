#pragma once
// ─── core/GroupMediaSender.hpp ────────────────────────────────────────────────
//
// V32.39+: sender half of group media-share. Turns a media blob into the package
// a shared conversation needs, wrapping snapclone-mediashare:
//
//   strip          → remove EXIF/GPS/metadata (images)         [privacy]
//   content key    → fresh random 32-byte key
//   seal           → encrypt the media once with that key      [confidentiality]
//   chunk          → content-addressed split of the CIPHERTEXT into a persistent
//                    store, so the conversation never re-sends blocks it already
//                    holds                                       [dedup]
//   keyshare       → wrap the content key for every recipient   [multiway access]
//
// prepare() yields a Prepared { manifest, keyShareEnvelope, metrics }. The
// manifest + envelope are sent first; peers then diff the manifest against what
// they already hold and fetch only missing chunks, which this sender serves by
// id via chunkBytes(). That fetch/transfer protocol + its transport are a
// separate seam — this class produces the package and serves chunks; it does not
// itself move bytes on the wire.
//
// Built always; runs inert (prepare → prepareFailed, isAvailable() == false) when
// the mediashare crate isn't compiled in (no SNAPCLONE_RUST_MEDIASHARE), so
// callers need no feature #ifdefs.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QByteArray>
#include <QList>

namespace SnapClone::Core {

class GroupMediaSender : public QObject {
    Q_OBJECT
public:
    struct Prepared {
        bool       ok             = false;
        QByteArray manifest;             // opaque MediaManifest (send to peers)
        QByteArray keyShareEnvelope;     // content key wrapped for all recipients
        // ── metrics (for a send receipt / MediaSendSummary) ──
        int        stripFormat    = 0;   // SC_IMAGE_FORMAT_* (0 unknown,1 JPEG,2 PNG)
        int        stripRemoved   = 0;   // metadata segments removed
        qint64     originalBytes  = 0;   // input size
        qint64     strippedBytes  = 0;   // after metadata strip
        qint64     sealedBytes    = 0;   // ciphertext size (what the chunk layer split)
        int        recipientCount = 0;
    };

    explicit GroupMediaSender(int chunkSize = 64 * 1024, QObject *parent = nullptr);
    ~GroupMediaSender() override;

    /// True iff the mediashare backend was compiled in.
    [[nodiscard]] bool isAvailable() const;

    /// Recipients' X25519 public keys (32 bytes each; non-32 entries ignored).
    void setRecipients(const QList<QByteArray> &recipientX25519PubKeys);

    /// Serve a stored chunk's bytes by its 32-byte content id (for answering a
    /// peer's fetch). Empty if unknown or the backend isn't built.
    [[nodiscard]] QByteArray chunkBytes(const QByteArray &id32) const;

    /// Content key of the most recent prepare() (32 bytes), exposed for callers
    /// that route it through another crypto path. Empty when none/unavailable.
    [[nodiscard]] QByteArray lastContentKey() const { return m_lastContentKey; }

public Q_SLOTS:
    /// Run the full pipeline for `media`. `isImage` enables the metadata strip.
    /// Emits prepared() on success or prepareFailed() with a reason.
    void prepare(const QByteArray &media, bool isImage = true);

Q_SIGNALS:
    void prepared(const SnapClone::Core::GroupMediaSender::Prepared &pkg);
    void prepareFailed(const QString &reason);

private:
    int               m_chunkSize;
    QList<QByteArray> m_recipients;
    QByteArray        m_lastContentKey;
    void             *m_store = nullptr;   // ChunkStore* (opaque; persists for dedup)
};

} // namespace SnapClone::Core
