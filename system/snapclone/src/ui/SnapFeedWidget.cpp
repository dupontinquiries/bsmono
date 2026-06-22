#include "ui/SnapFeedWidget.hpp"
#include "ui/Theme.hpp"
#include "ui/ThumbnailService.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QGridLayout>
#include <QScrollArea>
#include <QLabel>
#include <QMouseEvent>
#include <QPaintEvent>
#include <QPainter>
#include <QPainterPath>
#include <QPointer>
#include <QDateTime>
#include <QHash>

namespace SnapClone::UI {

namespace {

/// A calm, muted per-sender hue so the feed has visual variety and you can tell
/// at a glance who's been sending — without ever revealing snap contents.
QString senderHue(const QString &name)
{
    static const char *kHues[] = {
        "#5B7C99", "#6B8E7A", "#8A7CA8", "#A8866B", "#7C99A8", "#998A8A"
    };
    const int n = static_cast<int>(sizeof(kHues) / sizeof(kHues[0]));
    return QString::fromLatin1(kHues[qHash(name) % static_cast<uint>(n)]);
}

QString formatRelative(const QDateTime &t)
{
    if (!t.isValid()) return QObject::tr("now");
    const qint64 secs = t.secsTo(QDateTime::currentDateTime());
    if (secs < 60)    return QObject::tr("now");
    if (secs < 3600)  return QObject::tr("%1m").arg(secs / 60);
    if (secs < 86400) return QObject::tr("%1h").arg(secs / 3600);
    return QObject::tr("%1d").arg(secs / 86400);
}

} // namespace

// ─── SnapTile ────────────────────────────────────────────────────────────────

SnapTile::SnapTile(const SnapClone::Core::SnapId &id,
                   const QString                 &senderName,
                   const QDateTime               &sentAt,
                   bool                           isVideo,
                   QWidget                       *parent)
    : QFrame(parent), m_id(id)
{
    setObjectName(QStringLiteral("snapTile"));
    setCursor(Qt::PointingHandCursor);
    setMinimumHeight(132);
    setFrameShape(QFrame::NoFrame);   // paintEvent draws the card + any frosted thumbnail

    auto *col = new QVBoxLayout(this);
    col->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                            Theme::SPACE_MD, Theme::SPACE_MD);
    col->setSpacing(Theme::SPACE_SM);

    const QString trimmed = senderName.trimmed();
    const QString initial = trimmed.isEmpty() ? QStringLiteral("?")
                                              : trimmed.left(1).toUpper();

    // Top row: sealed-sender avatar on the left, unopened dot on the right.
    auto *top = new QHBoxLayout();
    top->setContentsMargins(0, 0, 0, 0);
    auto *avatar = new QLabel(initial, this);
    avatar->setFixedSize(40, 40);
    avatar->setAlignment(Qt::AlignCenter);
    avatar->setStyleSheet(QStringLiteral(
        "background: %1; color: white; border-radius: 20px;"
        " font-size: %2px; font-weight: 700;")
        .arg(senderHue(trimmed)).arg(Theme::FONT_LG));
    top->addWidget(avatar);
    top->addStretch();
    auto *dot = new QLabel(this);
    dot->setFixedSize(10, 10);
    dot->setStyleSheet(QStringLiteral("background: %1; border-radius: 5px;")
        .arg(QLatin1String(Theme::ACCENT())));
    top->addWidget(dot, 0, Qt::AlignTop);
    col->addLayout(top);

    col->addStretch();

    auto *name = new QLabel(trimmed.isEmpty() ? tr("Someone") : trimmed, this);
    name->setStyleSheet(QStringLiteral("color: %1; font-size: %2px; font-weight: 600;")
        .arg(QLatin1String(Theme::TEXT_PRIMARY())).arg(Theme::FONT_BASE));
    col->addWidget(name);

    auto *meta = new QLabel(
        QStringLiteral("%1 · %2").arg(formatRelative(sentAt),
                                      isVideo ? tr("Video") : tr("Photo")), this);
    meta->setStyleSheet(QStringLiteral("color: %1; font-size: %2px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())).arg(Theme::FONT_SM));
    col->addWidget(meta);
}

void SnapTile::mousePressEvent(QMouseEvent *e)
{
    if (e->button() == Qt::LeftButton)
        emit clicked(m_id);
    QFrame::mousePressEvent(e);
}

void SnapTile::setThumbnail(const QPixmap &pm)
{
    if (pm.isNull()) return;
    // Pre-blur by collapsing to a tiny source; paintEvent scales it back up
    // smoothly, which reads as frosted glass — colour and rough shape, never
    // legible detail. Raise kFrostPx toward the source size for a clearer
    // preview, lower it for heavier frost.
    constexpr int kFrostPx = 16;
    m_frosted = pm.scaled(kFrostPx, kFrostPx, Qt::IgnoreAspectRatio,
                          Qt::SmoothTransformation);
    update();
}

void SnapTile::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);
    p.setRenderHint(QPainter::SmoothPixmapTransform);

    const QRectF r = QRectF(rect()).adjusted(0.5, 0.5, -0.5, -0.5);
    const qreal  radius = Theme::RADIUS_MD;
    QPainterPath path;
    path.addRoundedRect(r, radius, radius);
    p.setClipPath(path);

    if (!m_frosted.isNull()) {
        // Cover-fit the frosted source across the whole tile.
        const QPixmap cover = m_frosted.scaled(size(), Qt::KeepAspectRatioByExpanding,
                                               Qt::SmoothTransformation);
        const int dx = (cover.width()  - width())  / 2;
        const int dy = (cover.height() - height()) / 2;
        p.drawPixmap(-dx, -dy, cover);
        // Bottom scrim so the name/time stay legible over the image.
        QLinearGradient g(0, height() * 0.35, 0, height());
        g.setColorAt(0.0, QColor(0, 0, 0, 0));
        g.setColorAt(1.0, QColor(0, 0, 0, 170));
        p.fillRect(rect(), g);
    } else {
        p.fillPath(path, QColor(QString::fromLatin1(Theme::BG_RAISED())));
    }

    p.setClipping(false);
    QPen pen(QColor(QString::fromLatin1(Theme::BORDER())));
    pen.setWidthF(1.0);
    p.setPen(pen);
    p.setBrush(Qt::NoBrush);
    p.drawRoundedRect(r, radius, radius);
}

// ─── SnapFeedWidget ──────────────────────────────────────────────────────────

SnapFeedWidget::SnapFeedWidget(QWidget *parent)
    : QWidget(parent)
{
    auto *outer = new QVBoxLayout(this);
    outer->setContentsMargins(0, 0, 0, 0);
    outer->setSpacing(0);

    m_empty = new QLabel(tr("No new snaps"), this);
    m_empty->setAlignment(Qt::AlignCenter);
    m_empty->setStyleSheet(QStringLiteral("color: %1; font-size: %2px; padding: %3px;")
        .arg(QLatin1String(Theme::TEXT_DISABLED())).arg(Theme::FONT_BASE).arg(Theme::SPACE_LG));
    outer->addWidget(m_empty);

    m_scroll = new QScrollArea(this);
    m_scroll->setWidgetResizable(true);
    m_scroll->setFrameShape(QFrame::NoFrame);
    m_scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);

    m_grid = new QWidget(m_scroll);
    m_layout = new QGridLayout(m_grid);
    m_layout->setContentsMargins(Theme::SPACE_MD, Theme::SPACE_MD,
                                 Theme::SPACE_MD, Theme::SPACE_MD);
    m_layout->setSpacing(Theme::SPACE_MD);
    m_layout->setAlignment(Qt::AlignTop);
    m_scroll->setWidget(m_grid);
    outer->addWidget(m_scroll, 1);

    m_scroll->setVisible(false);   // empty until entries arrive
}

void SnapFeedWidget::setEntries(const QList<Entry> &entries)
{
    m_entries = entries;
    rebuild();
}

void SnapFeedWidget::rebuild()
{
    // Tear down existing tiles.
    QLayoutItem *item = nullptr;
    while ((item = m_layout->takeAt(0)) != nullptr) {
        if (auto *w = item->widget()) w->deleteLater();
        delete item;
    }

    const bool empty = m_entries.isEmpty();
    m_empty->setVisible(empty);
    m_scroll->setVisible(!empty);
    if (empty) return;

    int idx = 0;
    for (const auto &e : m_entries) {
        auto *tile = new SnapTile(e.id, e.senderName, e.sentAt, e.isVideo, m_grid);
        connect(tile, &SnapTile::clicked, this, &SnapFeedWidget::snapTapped);
        m_layout->addWidget(tile, idx / kColumns, idx % kColumns);
        if (!e.localPath.isEmpty()) {
            // Async, LRU-cached. The feed (long-lived) is the context; a guard
            // covers the tile being torn down by a later setEntries().
            QPointer<SnapTile> guard(tile);
            ThumbnailService::instance().request(
                e.localPath, 96, this,
                [guard](const QPixmap &pm) {
                    if (guard && !pm.isNull()) guard->setThumbnail(pm);
                });
        }
        ++idx;
    }
    for (int c = 0; c < kColumns; ++c)
        m_layout->setColumnStretch(c, 1);
}

} // namespace SnapClone::UI
