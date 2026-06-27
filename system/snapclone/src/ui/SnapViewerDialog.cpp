#include "ui/SnapViewerDialog.hpp"
#include "core/Types.hpp"
#include <QUrl>
#include <QDesktopServices>
#include "ui/Theme.hpp"

#include <QLabel>
#include <QPushButton>
#include <QProgressBar>
#include <QTimer>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QPixmap>
#include <QImage>
#include <QDir>
#include <QFile>
#include <QStandardPaths>
#include <QDateTime>
#include <QLoggingCategory>
#include <QKeyEvent>
#include <QResizeEvent>
#include <QRegularExpression>
#include <QScreen>
#include <QGuiApplication>

// ─── UI/SnapViewerDialog.cpp ──────────────────────────────────────────────────
//
// Snap viewer with explicit save control.
//
// Save flow:
//   1. User taps "Save" (or swipes down — TODO gesture in v2).
//   2. buildSavePath() constructs ~/Pictures/SnapClone/<timestamped name>.
//   3. QDir().mkpath() ensures the directory exists.
//   4. image bytes are written as JPEG.
//   5. imageSaved(path) is emitted → MainWindow status bar shows confirmation.
//
// Auto-close flow:
//   m_timer ticks every 100ms, decrementing m_remainingMs.
//   QProgressBar shows the countdown visually.
//   When remainingMs reaches 0, the dialog accepts (closes) and
//   viewTimerExpired is emitted.
// ─────────────────────────────────────────────────────────────────────────────

Q_LOGGING_CATEGORY(lcViewer, "snapclone.viewer")

namespace SnapClone::UI {

SnapViewerDialog::SnapViewerDialog(const Core::Snap &snap, QWidget *parent)
    : MobileDialog(parent)
    , m_snap(snap)
    , m_remainingMs(snap.durationSecs > 0 ? static_cast<qint64>(snap.durationSecs) * 1000 : 0)
{
    const QString senderDisplay = snap.senderId.value.contains(QLatin1Char('@'))
        ? snap.senderId.value.section(QLatin1Char('@'), 0, 0)
        : snap.senderId.value;
    setWindowTitle(QStringLiteral("Snap from %1").arg(senderDisplay));
    setModal(true);

    // Fill the screen on mobile; on desktop be reasonably large.
    if (const QScreen *scr = QGuiApplication::primaryScreen()) {
        const QSize screen = scr->availableSize();
        if (screen.width() < 600) {
            // Mobile: fill available screen.
            setFixedSize(screen);
            setWindowFlags(Qt::Dialog | Qt::FramelessWindowHint);
        } else {
            // Desktop: 480×680 dialog with frame.
            resize(480, 680);
        }
    }

    buildUi(snap);
    setStyleSheet(Theme::globalStyleSheet());

    // Start the countdown timer (skip if duration is 0 = view indefinitely).
    m_totalMs     = m_remainingMs;
    m_elapsed.start();

    if (m_remainingMs > 0) {
        m_timer = new QTimer(this);
        m_timer->setInterval(200); // 5 ticks/s — smooth, saves CPU vs 80ms
        connect(m_timer, &QTimer::timeout, this, &SnapViewerDialog::onTimerTick);
        m_timer->start();
    } else {
        // Infinite duration — hide the countdown bar
        m_countdown->setVisible(false);
    }
}

void SnapViewerDialog::buildUi(const Core::Snap &snap)
{
    // Friendly sender name — strip @ip suffix.
    const QString senderDisplay = snap.senderId.value.contains(QLatin1Char('@'))
        ? snap.senderId.value.section(QLatin1Char('@'), 0, 0)
        : snap.senderId.value;

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(0, 0, 0, 0);
    root->setSpacing(0);

    // ── Image area ────────────────────────────────────────────────────────
    m_imageLabel = new QLabel(this);
    m_imageLabel->setAlignment(Qt::AlignCenter);
    m_imageLabel->setStyleSheet(QStringLiteral("background: #000000;"));
    m_imageLabel->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
    root->addWidget(m_imageLabel, 1);

    // Media is populated by loadSnap() at the end of buildUi (and again on each
    // playlist navigation), so image and text snaps share one code path.

    // ── Countdown bar ─────────────────────────────────────────────────────
    m_countdown = new QProgressBar(this);
    m_countdown->setRange(0, static_cast<int>(m_remainingMs));
    m_countdown->setValue(static_cast<int>(m_remainingMs));
    m_countdown->setTextVisible(false);
    m_countdown->setFixedHeight(4);
    m_countdown->setStyleSheet(QStringLiteral(R"(
        QProgressBar {
            background: %1;
            border: none;
        }
        QProgressBar::chunk {
            background: %2;
        }
    )").arg(QLatin1String(Theme::BG_RAISED()), QLatin1String(Theme::ACCENT())));
    root->addWidget(m_countdown);

    // ── Meta strip ────────────────────────────────────────────────────────
    auto *metaStrip = new QWidget(this);
    metaStrip->setStyleSheet(
        QStringLiteral("background: %1;").arg(QLatin1String(Theme::BG_CARD())));
    auto *metaLayout = new QVBoxLayout(metaStrip);
    metaLayout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                                    Theme::SPACE_MD, Theme::SPACE_SM);
    metaLayout->setSpacing(Theme::SPACE_XS);

    m_metaLabel = new QLabel(
        tr("From %1\n%2")
            .arg(senderDisplay,
                 snap.sentAt.toLocalTime().toString(QStringLiteral("ddd d MMM  hh:mm"))),
        metaStrip);
    m_metaLabel->setObjectName(QStringLiteral("subtitleLabel"));
    m_metaLabel->setWordWrap(true);
    metaLayout->addWidget(m_metaLabel);

    root->addWidget(metaStrip);

    // ── Location strip (shown only when snap carries coordinates) ─────────
    if (snap.hasLocation()) {
        auto *locStrip = new QWidget(this);
        locStrip->setStyleSheet(QStringLiteral(
            "background: %1;").arg(QLatin1String(Theme::BG_DEEP())));
        auto *locLayout = new QHBoxLayout(locStrip);
        locLayout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_XS,
                                      Theme::SPACE_MD, Theme::SPACE_XS);
        locLayout->setSpacing(Theme::SPACE_SM);

        auto *pinLabel = new QLabel(QStringLiteral("\U0001F4CD"), locStrip);
        pinLabel->setStyleSheet(QStringLiteral(
            "background: transparent; font-size: 14px;"));
        locLayout->addWidget(pinLabel);

        Core::PeerLocation senderLoc;
        senderLoc.recordedAt = snap.sentAt;
        senderLoc.lat        = snap.locLat;
        senderLoc.lon        = snap.locLon;
        senderLoc.accuracyM  = snap.locAccM;

        QString locText;
        if (snap.locAccM > 0)
            locText = tr("~%1 m accuracy  \u00B7  ").arg(qRound(snap.locAccM));
        locText += senderLoc.ageString();

        auto *locLabel = new QLabel(locText, locStrip);
        locLabel->setObjectName(QStringLiteral("subtitleLabel"));
        locLayout->addWidget(locLabel, 1);

        const double lat = snap.locLat, lon = snap.locLon;
        auto *mapsBtn = new QPushButton(tr("Map \u2197"), locStrip);
        mapsBtn->setFixedHeight(28);
        connect(mapsBtn, &QPushButton::clicked, this, [lat, lon]() {
            QDesktopServices::openUrl(QUrl(
                QStringLiteral("geo:%1,%2?q=%1,%2")
                    .arg(lat, 0, 'f', 6).arg(lon, 0, 'f', 6)));
        });
        locLayout->addWidget(mapsBtn);
        root->addWidget(locStrip);
    }

    // Tap the image to close immediately — Snapchat-style interaction
    m_imageLabel->setCursor(Qt::PointingHandCursor);
    m_imageLabel->installEventFilter(this);

    // ── Feed navigation bar (shown only in playlist mode) ─────────────────
    m_navBar = new QWidget(this);
    m_navBar->setStyleSheet(
        QStringLiteral("background: %1;").arg(QLatin1String(Theme::BG_CARD())));
    auto *navRow = new QHBoxLayout(m_navBar);
    navRow->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_XS,
                               Theme::SPACE_MD, Theme::SPACE_XS);
    navRow->setSpacing(Theme::SPACE_SM);
    m_prevBtn = new QPushButton(QStringLiteral("\u25C0"), m_navBar);   // ◀
    m_prevBtn->setFixedSize(44, 32);
    connect(m_prevBtn, &QPushButton::clicked, this, &SnapViewerDialog::navPrev);
    navRow->addWidget(m_prevBtn);
    navRow->addStretch();
    m_posLabel = new QLabel(QString(), m_navBar);
    m_posLabel->setObjectName(QStringLiteral("subtitleLabel"));
    m_posLabel->setAlignment(Qt::AlignCenter);
    navRow->addWidget(m_posLabel);
    navRow->addStretch();
    m_nextBtn = new QPushButton(QStringLiteral("\u25B6"), m_navBar);   // ▶
    m_nextBtn->setFixedSize(44, 32);
    connect(m_nextBtn, &QPushButton::clicked, this, &SnapViewerDialog::navNext);
    navRow->addWidget(m_nextBtn);
    m_navBar->hide();              // setPlaylist() reveals it for multi-snap feeds
    root->addWidget(m_navBar);

    // ── Action bar ────────────────────────────────────────────────────────
    auto *actionBar = new QWidget(this);
    actionBar->setStyleSheet(
        QStringLiteral("background: %1;").arg(QLatin1String(Theme::BG_DEEP())));
    auto *actionRow = new QHBoxLayout(actionBar);
    actionRow->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_SM,
                                   Theme::SPACE_MD, Theme::SPACE_SM);
    actionRow->setSpacing(Theme::SPACE_SM);

    m_saveBtn = new QPushButton(tr("Save"), actionBar);
    m_saveBtn->setObjectName(QStringLiteral("accentBtn"));
    connect(m_saveBtn, &QPushButton::clicked, this, &SnapViewerDialog::onSaveClicked);
    actionRow->addWidget(m_saveBtn);

    actionRow->addStretch();

    m_closeBtn = new QPushButton(tr("Close"), actionBar);
    connect(m_closeBtn, &QPushButton::clicked, this, &QDialog::reject);
    actionRow->addWidget(m_closeBtn);

    root->addWidget(actionBar);

    // Populate media + meta for the initial snap (image or text).
    loadSnap(snap);
}

// ─── Media / meta population (shared by initial build + playlist nav) ─────────
void SnapViewerDialog::loadSnap(const Core::Snap &snap)
{
    m_snap = snap;

    const QString senderDisplay = snap.senderId.value.contains(QLatin1Char('@'))
        ? snap.senderId.value.section(QLatin1Char('@'), 0, 0)
        : snap.senderId.value;
    if (m_metaLabel)
        m_metaLabel->setText(tr("From %1\n%2").arg(
            senderDisplay,
            snap.sentAt.toLocalTime().toString(QStringLiteral("ddd d MMM  hh:mm"))));

    if (snap.mediaType == Core::MediaType::Text) {
        // Render the message inline; contrast against the black stage uses the
        // primary text token (always legible on #000).
        m_fullPixmap = QPixmap();
        m_imageLabel->setPixmap(QPixmap());
        const QString body = !snap.caption.isEmpty()
            ? snap.caption
            : QString::fromUtf8(snap.encryptedBlob);
        m_imageLabel->setWordWrap(true);
        m_imageLabel->setAlignment(Qt::AlignCenter);
        m_imageLabel->setText(body);
        m_imageLabel->setStyleSheet(QStringLiteral(
            "background:#000000; color:%1; padding:28px; font-size:%2px;")
            .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_LG));
        return;
    }

    // Image (and unknown) path — decode bytes from memory or disk.
    m_imageLabel->setWordWrap(false);
    m_imageLabel->setAlignment(Qt::AlignCenter);
    m_imageLabel->setStyleSheet(QStringLiteral("background:#000000;"));
    if (!snap.encryptedBlob.isEmpty()) {
        loadImage(snap.encryptedBlob);
    } else if (!snap.localPath.isEmpty()) {
        QFile f(snap.localPath);
        if (f.open(QIODevice::ReadOnly)) {
            loadImage(f.readAll());
        } else {
            qCWarning(lcViewer) << "Cannot read media file:" << snap.localPath;
            m_imageLabel->setText(tr("Media file not found"));
            m_imageLabel->setStyleSheet(
                QStringLiteral("background:#000; color:%1; font-size:16px;")
                    .arg(QLatin1String(Theme::TEXT_SECONDARY())));
        }
    } else {
        m_imageLabel->setText(tr("Media not available"));
        m_imageLabel->setStyleSheet(
            QStringLiteral("background:#000; color:%1; font-size:16px;")
                .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    }
}

// ─── Feed-player playlist navigation ─────────────────────────────────────────
void SnapViewerDialog::setPlaylist(const QList<Core::Snap> &snaps, int startIndex)
{
    m_playlist = snaps;
    if (m_playlist.size() <= 1) return;   // nothing to navigate — stay single-snap

    // Browsing the feed is self-paced: drop the view-once auto-expiry timer and
    // its countdown bar, and reveal the ◀ / position / ▶ controls instead.
    if (m_timer) m_timer->stop();
    if (m_countdown) m_countdown->hide();
    if (m_navBar) m_navBar->show();

    m_index = -1;
    showSnapAt(qBound(0, startIndex, m_playlist.size() - 1));
}

void SnapViewerDialog::showSnapAt(int index)
{
    if (index < 0 || index >= m_playlist.size()) return;
    m_index = index;
    const Core::Snap &snap = m_playlist[index];
    loadSnap(snap);
    emit snapViewed(snap.id);   // host marks it viewed (browsing == opening)
    updateNavState();
}

void SnapViewerDialog::updateNavState()
{
    if (m_posLabel)
        m_posLabel->setText(tr("%1 / %2").arg(m_index + 1).arg(m_playlist.size()));
    if (m_prevBtn) m_prevBtn->setEnabled(m_index > 0);
    if (m_nextBtn) m_nextBtn->setEnabled(m_index < m_playlist.size() - 1);
}

void SnapViewerDialog::navPrev() { showSnapAt(m_index - 1); }
void SnapViewerDialog::navNext() { showSnapAt(m_index + 1); }

void SnapViewerDialog::loadImage(const QByteArray &jpegBytes)
{
    QImage img;
    if (!img.loadFromData(jpegBytes, "JPEG")) {
        // Try without format hint — may be PNG or another format.
        img.loadFromData(jpegBytes);
    }

    if (img.isNull()) {
        m_imageLabel->setText(tr("Could not decode image"));
        qCWarning(lcViewer) << "Failed to decode snap image bytes ("
                            << jpegBytes.size() << " bytes)";
        return;
    }

    // Store full-resolution pixmap as a member for reliable rescaling.
    // The label is Expanding so its size is 0 in the constructor;
    // resizeEvent() will rescale once the dialog has its final geometry.
    m_fullPixmap = QPixmap::fromImage(img);
    m_imageLabel->setPixmap(m_fullPixmap.scaled(
        m_imageLabel->sizeHint().expandedTo(QSize(300, 300)),
        Qt::KeepAspectRatio,
        Qt::SmoothTransformation));
}

// ─── Save ─────────────────────────────────────────────────────────────────────

void SnapViewerDialog::onSaveClicked()
{
    if (m_saved) return; // debounce rapid taps

    // Try blob first; fall back to localPath if blob was cleared after first view.
    QByteArray bytesToSave = m_snap.encryptedBlob;
    if (bytesToSave.isEmpty() && !m_snap.localPath.isEmpty()) {
        QFile src(m_snap.localPath);
        if (src.open(QIODevice::ReadOnly)) bytesToSave = src.readAll();
    }
    if (bytesToSave.isEmpty()) {
        qCWarning(lcViewer) << "Save requested but no media bytes available.";
        return;
    }

    const QString path = buildSavePath(m_snap);
    QDir().mkpath(QFileInfo(path).absolutePath());

    QFile f(path);
    if (!f.open(QIODevice::WriteOnly)) {
        qCWarning(lcViewer) << "Could not open" << path << "for writing:" << f.errorString();
        return;
    }
    const qint64 written = f.write(bytesToSave);
    f.close();

    if (written != static_cast<qint64>(bytesToSave.size())) {
        qCWarning(lcViewer) << "Snap save incomplete:" << written
                            << "/" << bytesToSave.size() << "bytes written to" << path;
        QFile::remove(path); // remove partial file
        m_saveBtn->setText(tr("Save failed"));
        return;
    }

    m_saved = true;
    qCInfo(lcViewer) << "Snap saved to" << path;
    m_saveBtn->setText(tr("✓  Saved"));
    m_saveBtn->setEnabled(false);
    emit imageSaved(path);
    emit snapKept(m_snap.id);   // V32.42+: host marks it kept (exempt from expiry)
}

QString SnapViewerDialog::buildSavePath(const Core::Snap &snap) const
{
    // ~/Pictures/SnapClone/2026-04-03_14-22-01_from_alice@192.168.1.10_ab12cd34.jpg
    const QString picturesDir =
        QStandardPaths::writableLocation(QStandardPaths::PicturesLocation);
    const QString snapDir = picturesDir + QStringLiteral("/SnapClone");

    // Sanitise sender ID for use in a filename.
    // Replace filesystem-unsafe chars, strip leading dots (hides files on Unix),
    // and truncate to 64 chars so the full path stays under 255 bytes on all FSes.
    QString safeSender = snap.senderId.value;
    safeSender.replace(QRegularExpression(QStringLiteral("[/\\\\:*?\"<>|.]"))
                       , QStringLiteral("_"));
    if (safeSender.size() > 64) safeSender.truncate(64);

    // First 8 chars of snap ID for uniqueness without excessive length.
    const QString shortId = snap.id.value.left(8);

    const QString timestamp =
        QDateTime::currentDateTime().toString(QStringLiteral("yyyy-MM-dd_HH-mm-ss"));

    return QStringLiteral("%1/%2_from_%3_%4.jpg")
        .arg(snapDir, timestamp, safeSender, shortId);
}

// ─── Countdown timer ──────────────────────────────────────────────────────────

void SnapViewerDialog::onTimerTick()
{
    // Use the elapsed timer for drift-free countdown.
    // Counting down by subtracting the nominal tick interval (100ms) would
    // accumulate error on loaded systems; QElapsedTimer measures actual wall time.
    const qint64 elapsedMs   = m_elapsed.elapsed();
    m_remainingMs             = qMax(0LL, static_cast<qint64>(m_totalMs) - elapsedMs);
    m_countdown->setValue(static_cast<int>(m_remainingMs));

    if (m_remainingMs <= 0) {
        m_timer->stop();
        emit viewTimerExpired(m_snap.id);
        accept();
    }
}

// ─── Key handling ─────────────────────────────────────────────────────────────

bool SnapViewerDialog::eventFilter(QObject *obj, QEvent *e)
{
    if (obj == m_imageLabel && e->type() == QEvent::MouseButtonPress) {
        // Tap image to dismiss immediately (Snapchat-style)
        if (m_timer) m_timer->stop(); // m_timer is null for infinite-duration snaps
        emit viewTimerExpired(m_snap.id);
        accept();
        return true;
    }
    return QDialog::eventFilter(obj, e);
}

void SnapViewerDialog::keyPressEvent(QKeyEvent *e)
{
    if (e->key() == Qt::Key_Escape || e->key() == Qt::Key_Back) {
        reject();
    } else if (!m_playlist.isEmpty() &&
               (e->key() == Qt::Key_Left || e->key() == Qt::Key_Right)) {
        if (e->key() == Qt::Key_Left) navPrev();
        else                          navNext();
    } else {
        QDialog::keyPressEvent(e);
    }
}

// ─── Resize — rescale image to fit ───────────────────────────────────────────

void SnapViewerDialog::resizeEvent(QResizeEvent *e)
{
    QDialog::resizeEvent(e);
    if (m_fullPixmap.isNull() || !m_imageLabel) return;
    // V32.34+: two-stage rescale. Resize events fire continuously while
    // the user drags a window edge or the virtual keyboard slides in.
    // A SmoothTransformation scale of a 4032×3024 → screen-size pixmap
    // is ~50ms per event, which fully freezes the UI under sustained
    // resizing. Fix: do a FAST (nearest-neighbor) scale on every event
    // so the visual tracks the new size in real time, then arm a
    // single-shot timer to follow up with the smooth rescale 80ms
    // after the LAST resize event. The fast version is good enough
    // for transient frames; the smooth one lands at rest.
    m_imageLabel->setPixmap(m_fullPixmap.scaled(
        m_imageLabel->size(),
        Qt::KeepAspectRatio,
        Qt::FastTransformation));

    if (!m_smoothResizeTimer) {
        m_smoothResizeTimer = new QTimer(this);
        m_smoothResizeTimer->setSingleShot(true);
        m_smoothResizeTimer->setInterval(80);
        connect(m_smoothResizeTimer, &QTimer::timeout, this, [this]() {
            if (m_fullPixmap.isNull() || !m_imageLabel) return;
            m_imageLabel->setPixmap(m_fullPixmap.scaled(
                m_imageLabel->size(),
                Qt::KeepAspectRatio,
                Qt::SmoothTransformation));
        });
    }
    m_smoothResizeTimer->start();
}

} // namespace SnapClone::UI
