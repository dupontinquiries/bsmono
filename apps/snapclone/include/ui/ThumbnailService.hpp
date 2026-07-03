#pragma once

// ─── UI/ThumbnailService.hpp ─────────────────────────────────────────────────
// Async thumbnail loader. Decouples QPixmap(path) + scale + crop work from
// the GUI thread so list rebuilds don't stall scrolling.
//
// Design:
//   • Public API: request(path, size, callback) — returns immediately.
//   • Worker thread (QThreadPool of size 2) reads the file, decodes, scales,
//     center-crops, draws a 1px border, and returns a QPixmap.
//   • Result delivered to the caller's thread via QueuedConnection so the
//     callback runs on the same thread as the caller (typically GUI).
//   • LRU-style cache (path → QPixmap) bounded at kMaxCacheEntries; cache
//     hits return synchronously via callback.run() (no event-loop hop).
//
// The cache key INCLUDES the requested size so a 56×56 request and a
// 128×128 request for the same file get distinct cache entries — important
// for the inbox (56) vs ContactPickerDialog (96) etc.
//
// Lifecycle: singleton, lazily constructed, destroyed at QCoreApplication
// shutdown (Q_GLOBAL_STATIC). Callbacks captured by-value protect against
// caller dying mid-flight (the worker doesn't dereference the caller).
// Callers that need to bail out (e.g. list rebuilt with different rows)
// pass a QPointer-context to their callback and bail in the slot.

#include <QObject>
#include <QString>
#include <QPixmap>
#include <QHash>
#include <QMutex>
#include <QList>
#include <functional>
#include <atomic>     // V32.34+: shutdown latch read on worker threads

QT_BEGIN_NAMESPACE
class QThreadPool;
QT_END_NAMESPACE

namespace SnapClone::UI {

class ThumbnailService : public QObject
{
    Q_OBJECT
public:
    /// Singleton accessor. Constructs on first call; destroyed at app exit.
    static ThumbnailService &instance();

    /// Result callback. Receives a QPixmap; null pixmap = load failed.
    /// The callback runs on the THREAD THAT CALLED request() (typically
    /// the GUI thread), via QueuedConnection. Safe to update widgets.
    using ResultCallback = std::function<void(const QPixmap &)>;

    /// Request a thumbnail of `path` at `size` (square; we center-crop).
    /// If cached, callback fires synchronously before request() returns.
    /// Otherwise, callback fires later on the calling thread.
    ///
    /// `context` is the QObject lifetime guard. If `context` is destroyed
    /// before the worker finishes, the callback is NOT invoked — protects
    /// against the common "rebuild the list, old request lands on dead
    /// widget" race.
    void request(const QString &path, int size,
                 QObject *context, ResultCallback callback);

    /// Drop everything from the cache. Useful when entries are known to
    /// be stale (e.g. user changed avatar). The capped-LRU also prunes
    /// automatically — clear is rarely needed.
    void clearCache();

    /// Stats for diagnostics. Cheap; safe from any thread.
    struct Stats {
        int   cacheEntries = 0;
        qint64 cacheBytes  = 0;
        int   activeJobs   = 0;
    };
    [[nodiscard]] Stats stats() const;

private:
    ThumbnailService();
    ~ThumbnailService() override;

    /// Worker fn — runs on threadpool. Reads file, scales, crops, returns
    /// a QPixmap (potentially null on failure). Static so it doesn't need
    /// `this` (avoids any chance of dereferencing the service mid-shutdown).
    static QPixmap renderOnWorker(const QString &path, int size);

    /// Cache key encodes path + size.
    static QString cacheKey(const QString &path, int size)
    {
        return path + QStringLiteral("@") + QString::number(size);
    }

    /// Cache. Guarded by m_mu. LRU-ish: we track insertion order via
    /// m_lruKeys (push_back on insert, remove on eviction). Not strict
    /// LRU (no move-to-front on hit) — for our access pattern (rapid
    /// row rebuilds) the cheaper insertion-order eviction is fine.
    mutable QMutex             m_mu;
    QHash<QString, QPixmap>    m_cache;
    QList<QString>             m_lruKeys;
    int                        m_activeJobs = 0;

    QThreadPool *m_pool = nullptr;

    /// V32.34+: shutdown latch. Set true at the very start of the
    /// destructor, BEFORE waitForDone(). A worker job that finishes during
    /// teardown checks this (and QCoreApplication liveness) before trying to
    /// post its result back to the GUI thread. Without this guard, a job
    /// completing after the GUI event loop has stopped would call
    /// QMetaObject::invokeMethod(..., QueuedConnection) on a thread whose
    /// event dispatcher is gone → segfault on shutdown. std::atomic because
    /// it's read on worker threads and written on the main thread.
    std::atomic<bool> m_shuttingDown{ false };

    /// Cache cap. 200 entries × (56*56*4) ≈ 2.5MB worst-case. Slightly
    /// higher than the inbox's old inline cap so we can serve both
    /// inbox AND friends rows from the same cache.
    static constexpr int kMaxCacheEntries = 256;
};

} // namespace SnapClone::UI
