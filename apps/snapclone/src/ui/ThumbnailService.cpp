#include "ui/ThumbnailService.hpp"
#include "ui/Theme.hpp"

#include <QThreadPool>
#include <QRunnable>
#include <QPointer>
#include <QPixmap>
#include <QPainter>
#include <QPen>
#include <QImageReader>
#include <QMetaObject>
#include <QCoreApplication>  // V32.34+: liveness check before posting result
#include <QLoggingCategory>
#include <QFile>
#include <QGlobalStatic>

namespace SnapClone::UI {

namespace {
Q_LOGGING_CATEGORY(lcThumbs, "snapclone.ui.thumbs")
}

ThumbnailService &ThumbnailService::instance()
{
    // Plain function-local static. Q_GLOBAL_STATIC would also work but
    // function-local is simpler and the destruction order (after
    // QCoreApplication's main loop exits but before static dtors) is
    // exactly what we want — the threadpool's wait-for-done in our dtor
    // can still run.
    static ThumbnailService s;
    return s;
}

ThumbnailService::ThumbnailService()
    : m_pool(new QThreadPool(this))
{
    // 2 worker threads. More wouldn't help — disk I/O is the bottleneck
    // on the typical inbox path (jpeg decode is fast). 2 lets us overlap
    // one decode with one file-read so peak throughput is double a single
    // thread without overwhelming the CPU for other work.
    m_pool->setMaxThreadCount(2);
    // Long-ish expiry: keep threads warm across scroll bursts but let
    // them die in idle periods. 30s matches the inbox refresh cycle.
    m_pool->setExpiryTimeout(30000);
    qCInfo(lcThumbs) << "ThumbnailService started (2 worker threads)";
}

ThumbnailService::~ThumbnailService()
{
    // V32.34+: latch shutdown FIRST, before waiting. Any worker job still
    // running will see this and skip the QMetaObject::invokeMethod result
    // post — by now the GUI thread's event loop has stopped (we're in
    // static-destruction, after QApplication is gone), so posting a queued
    // event would deref a destroyed thread dispatcher and segfault.
    m_shuttingDown.store(true, std::memory_order_release);

    // m_pool has `this` as QObject parent, so it gets deleted in the
    // base QObject dtor. Before that, wait for any running jobs to
    // finish so their captures (which may hold widget contexts via
    // QPointer) don't deref freed memory.
    if (m_pool) {
        m_pool->clear();
        m_pool->waitForDone(2000);
    }
}

void ThumbnailService::request(const QString &path, int size,
                                QObject *context, ResultCallback callback)
{
    if (path.isEmpty() || !callback) return;
    if (size <= 0 || size > 4096) {
        qCWarning(lcThumbs) << "request: bad size" << size << "for" << path;
        return;
    }

    const QString key = cacheKey(path, size);

    // Cache hit — deliver synchronously, no event-loop hop.
    {
        QMutexLocker lock(&m_mu);
        const auto it = m_cache.constFind(key);
        if (it != m_cache.constEnd()) {
            const QPixmap hit = it.value();
            lock.unlock();
            callback(hit);
            return;
        }
    }

    // Cache miss. Queue work on the threadpool. Capture by VALUE so
    // the worker doesn't dereference the caller mid-shutdown. The
    // context QPointer is the lifetime guard — checked from the GUI
    // thread before invoking the callback.
    QPointer<QObject> ctx(context);
    auto cb = callback;  // capture by value into the lambda

    {
        QMutexLocker lock(&m_mu);
        ++m_activeJobs;
    }

    // Workaround: QThreadPool::start(std::function) doesn't exist in all
    // Qt versions. Wrap with QRunnable. Using a lambda-holding QRunnable
    // is straightforward.
    class Job : public QRunnable {
    public:
        Job(ThumbnailService *svc, QString p, int s, QString k,
            QPointer<QObject> c, ResultCallback cb)
            : m_svc(svc), m_path(std::move(p)), m_size(s),
              m_key(std::move(k)), m_ctx(c), m_cb(std::move(cb)) {}
        void run() override
        {
            const QPixmap pm = ThumbnailService::renderOnWorker(m_path, m_size);

            // Decrement active count + insert into cache (under lock).
            {
                QMutexLocker lock(&m_svc->m_mu);
                --m_svc->m_activeJobs;
                if (!pm.isNull()) {
                    m_svc->m_cache.insert(m_key, pm);
                    m_svc->m_lruKeys.append(m_key);
                    // Evict oldest if over cap. Take from the front of
                    // m_lruKeys — we don't re-promote on hit, so this is
                    // FIFO-with-LRU-flavor not strict LRU.
                    while (m_svc->m_lruKeys.size() > kMaxCacheEntries) {
                        const QString gone = m_svc->m_lruKeys.takeFirst();
                        m_svc->m_cache.remove(gone);
                    }
                }
            }

            // Deliver to caller's thread. If context is dead, drop —
            // the caller has moved on (typical when a list rebuilds
            // between our queue and our completion).
            QPointer<QObject> ctx = m_ctx;
            ResultCallback cb     = m_cb;
            if (!ctx) return;
            // V32.34+: shutdown guard. If the service is tearing down or
            // QCoreApplication is already gone, the GUI thread's event loop
            // is no longer pumping — posting a QueuedConnection event here
            // would deref a destroyed thread dispatcher and segfault on
            // exit. Drop the result silently; nothing is visible to update
            // during shutdown anyway. (renderOnWorker already populated the
            // cache above, so a relaunch benefits.)
            if (m_svc->m_shuttingDown.load(std::memory_order_acquire)) return;
            if (!QCoreApplication::instance()) return;
            QMetaObject::invokeMethod(ctx.data(), [ctx, cb, pm]() {
                if (!ctx) return;
                cb(pm);
            }, Qt::QueuedConnection);
        }
    private:
        ThumbnailService *m_svc;
        QString           m_path;
        int               m_size;
        QString           m_key;
        QPointer<QObject> m_ctx;
        ResultCallback    m_cb;
    };

    auto *job = new Job(this, path, size, key, ctx, std::move(cb));
    job->setAutoDelete(true);
    m_pool->start(job);
}

QPixmap ThumbnailService::renderOnWorker(const QString &path, int size)
{
    // On-worker: file existence + decode + scale + crop + border.
    // Mirrors the inline logic from InboxWidget but runs off-GUI.
    if (!QFile::exists(path)) return {};

    // QImageReader is slightly more memory-efficient than QImage(path)
    // because it can decode at the target size directly for some formats.
    QImageReader reader(path);
    reader.setAutoTransform(true);
    // Tell the reader our target so JPEG-with-EXIF-orientation gets
    // applied AND the decoder can use a downscaled path if available.
    reader.setScaledSize(QSize(size * 2, size * 2));  // 2x for crop slack
    QImage img = reader.read();
    if (img.isNull()) {
        // Fall back to plain QPixmap load. Some image variants don't
        // support the reader's scaled-size hint.
        QPixmap raw(path);
        if (raw.isNull()) return {};
        img = raw.toImage();
    }

    // Scale to expand-fit then crop to square, identical to the inline
    // version in InboxWidget so visual output matches pre-refactor.
    const QImage scaled = img.scaled(
        size, size,
        Qt::KeepAspectRatioByExpanding,
        Qt::SmoothTransformation);

    QPixmap out(size, size);
    out.fill(Qt::transparent);
    {
        QPainter pp(&out);
        pp.setRenderHint(QPainter::Antialiasing);
        const int x = (scaled.width()  - size) / 2;
        const int y = (scaled.height() - size) / 2;
        pp.drawImage(0, 0, scaled, x, y, size, size);
        // 1px border, inset 0.5 so it lands inside the pixmap bounds.
        pp.setPen(QPen(QColor(QLatin1String(Theme::BORDER())), 1));
        pp.setBrush(Qt::NoBrush);
        pp.drawRect(QRectF(0.5, 0.5, size - 1.0, size - 1.0));
    }
    return out;
}

void ThumbnailService::clearCache()
{
    QMutexLocker lock(&m_mu);
    m_cache.clear();
    m_lruKeys.clear();
    qCInfo(lcThumbs) << "cache cleared";
}

ThumbnailService::Stats ThumbnailService::stats() const
{
    QMutexLocker lock(&m_mu);
    Stats s;
    s.cacheEntries = m_cache.size();
    s.activeJobs   = m_activeJobs;
    // Approximate byte size from pixmap sizes. Cheap-enough for a
    // diagnostics call (linear in cache size); we don't memoize this.
    for (const auto &pm : m_cache) {
        s.cacheBytes += static_cast<qint64>(pm.width()) * pm.height() * 4;
    }
    return s;
}

} // namespace SnapClone::UI
