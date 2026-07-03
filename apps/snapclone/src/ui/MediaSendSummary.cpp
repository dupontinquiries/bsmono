#include "ui/MediaSendSummary.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QPaintEvent>
#include <QFont>
#include <QFontMetrics>

namespace SnapClone::UI {

namespace {
inline QColor col(const char *hex) { return QColor(QString::fromLatin1(hex)); }
inline QColor alpha(QColor c, int a) { c.setAlpha(a); return c; }

constexpr int kPad = Theme::SPACE_MD;
constexpr int kGlyph = 18;
constexpr int kRowGap = Theme::SPACE_SM;
} // namespace

MediaSendSummary::MediaSendSummary(QWidget *parent)
    : QWidget(parent)
{
    setAttribute(Qt::WA_OpaquePaintEvent, false);
}

MediaSendSummary::~MediaSendSummary() = default;

int MediaSendSummary::rowHeight() const
{
    return Theme::FONT_SM + Theme::FONT_XS + 6; // primary + detail stacked
}

QSize MediaSendSummary::sizeHint() const
{
    const int h = kPad * 2 + rowHeight() * 3 + kRowGap * 2;
    return QSize(Theme::vwPx(80), h);
}

void MediaSendSummary::setStripResult(int format, int segmentsRemoved)
{
    m_format = format;
    m_segmentsRemoved = segmentsRemoved;
    update();
}

void MediaSendSummary::setDedup(qint64 uploadBytes, qint64 totalBytes)
{
    m_uploadBytes = uploadBytes;
    m_totalBytes = totalBytes;
    update();
}

void MediaSendSummary::setRecipients(int count)
{
    m_recipients = count;
    update();
}

void MediaSendSummary::reset()
{
    m_format = 0;
    m_segmentsRemoved = 0;
    m_uploadBytes = -1;
    m_totalBytes = -1;
    m_recipients = 0;
    update();
}

// ── text ────────────────────────────────────────────────────────────────────

QString MediaSendSummary::humanBytes(qint64 n)
{
    if (n < 0) {
        return QStringLiteral("—");
    }
    const double kb = 1024.0;
    if (n < 1024) {
        return QStringLiteral("%1 B").arg(n);
    }
    if (n < 1024 * 1024) {
        return QStringLiteral("%1 KB").arg(n / kb, 0, 'f', n < 10 * 1024 ? 1 : 0);
    }
    return QStringLiteral("%1 MB").arg(n / (kb * kb), 0, 'f', 1);
}

QString MediaSendSummary::primaryText(Facet facet) const
{
    switch (facet) {
    case Facet::Privacy:
        if (m_format == 0) {
            return QStringLiteral("Original file");
        }
        return m_segmentsRemoved > 0 ? QStringLiteral("Metadata removed")
                                     : QStringLiteral("No metadata");
    case Facet::Size:
        if (m_uploadBytes < 0) {
            return QStringLiteral("Ready to send");
        }
        if (m_uploadBytes == 0) {
            return QStringLiteral("Already shared");
        }
        return QStringLiteral("Sending %1").arg(humanBytes(m_uploadBytes));
    case Facet::Crypto:
        if (m_recipients <= 0) {
            return QStringLiteral("Encrypted");
        }
        return m_recipients == 1
                   ? QStringLiteral("Encrypted to 1 person")
                   : QStringLiteral("Encrypted to %1 people").arg(m_recipients);
    }
    return QString();
}

QString MediaSendSummary::detailText(Facet facet) const
{
    switch (facet) {
    case Facet::Privacy:
        if (m_format == 0) {
            return QStringLiteral("kept as-is");
        }
        if (m_segmentsRemoved > 0) {
            return m_segmentsRemoved == 1
                       ? QStringLiteral("1 tag stripped")
                       : QStringLiteral("%1 tags stripped").arg(m_segmentsRemoved);
        }
        return QStringLiteral("already clean");
    case Facet::Size:
        if (m_uploadBytes < 0 || m_totalBytes <= 0) {
            return QString();
        }
        if (m_uploadBytes == 0) {
            return QStringLiteral("nothing to upload");
        }
        if (m_totalBytes > m_uploadBytes) {
            return QStringLiteral("%1 already shared")
                .arg(humanBytes(m_totalBytes - m_uploadBytes));
        }
        return QString();
    case Facet::Crypto:
        return QStringLiteral("per-recipient keys");
    }
    return QString();
}

QColor MediaSendSummary::facetColor(Facet facet) const
{
    switch (facet) {
    case Facet::Privacy:
        return col(m_format == 0 ? Theme::TEXT_SECONDARY() : Theme::SUCCESS());
    case Facet::Size:
        return col(m_uploadBytes == 0 ? Theme::SUCCESS() : Theme::ACCENT());
    case Facet::Crypto:
        return col(Theme::SUCCESS());
    }
    return col(Theme::ACCENT());
}

// ── painting ──────────────────────────────────────────────────────────────────

void MediaSendSummary::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);

    QPainterPath card;
    card.addRoundedRect(rect().adjusted(1, 1, -1, -1), Theme::RADIUS_MD, Theme::RADIUS_MD);
    p.fillPath(card, col(Theme::BG_CARD()));

    const int rh = rowHeight();
    qreal y = kPad;
    const Facet order[] = {Facet::Privacy, Facet::Size, Facet::Crypto};
    for (const Facet f : order) {
        paintFacet(p, f, QRectF(kPad, y, width() - kPad * 2, rh));
        y += rh + kRowGap;
    }
}

void MediaSendSummary::paintFacet(QPainter &p, Facet facet, const QRectF &row)
{
    const QColor c = facetColor(facet);
    const QRectF glyphBox(row.left(), row.center().y() - kGlyph / 2.0, kGlyph, kGlyph);
    paintGlyph(p, facet, glyphBox, c);

    const qreal textX = glyphBox.right() + Theme::SPACE_SM;
    const qreal textW = row.right() - textX;

    QFont primaryFont;
    primaryFont.setPixelSize(Theme::FONT_SM);
    primaryFont.setWeight(QFont::DemiBold);
    p.setFont(primaryFont);
    p.setPen(col(Theme::TEXT_PRIMARY()));
    const QRectF primaryRect(textX, row.top(), textW, Theme::FONT_SM + 3);
    p.drawText(primaryRect, Qt::AlignLeft | Qt::AlignVCenter,
               p.fontMetrics().elidedText(primaryText(facet), Qt::ElideRight,
                                          static_cast<int>(textW)));

    const QString detail = detailText(facet);
    if (!detail.isEmpty()) {
        QFont detailFont;
        detailFont.setPixelSize(Theme::FONT_XS);
        p.setFont(detailFont);
        p.setPen(col(Theme::TEXT_SECONDARY()));
        const QRectF detailRect(textX, primaryRect.bottom(), textW, Theme::FONT_XS + 3);
        p.drawText(detailRect, Qt::AlignLeft | Qt::AlignVCenter,
                   p.fontMetrics().elidedText(detail, Qt::ElideRight,
                                              static_cast<int>(textW)));
    }
}

void MediaSendSummary::paintGlyph(QPainter &p, Facet facet, const QRectF &box,
                                  const QColor &c)
{
    p.save();
    QPen pen(c);
    pen.setWidthF(2.0);
    pen.setCapStyle(Qt::RoundCap);
    pen.setJoinStyle(Qt::RoundJoin);
    p.setPen(pen);
    p.setBrush(Qt::NoBrush);

    switch (facet) {
    case Facet::Privacy: {
        // checkmark
        const qreal x = box.left(), y = box.top(), w = box.width(), h = box.height();
        p.drawLine(QPointF(x + w * 0.18, y + h * 0.52),
                   QPointF(x + w * 0.42, y + h * 0.74));
        p.drawLine(QPointF(x + w * 0.42, y + h * 0.74),
                   QPointF(x + w * 0.82, y + h * 0.28));
        break;
    }
    case Facet::Size: {
        // up arrow
        const qreal cx = box.center().x();
        p.drawLine(QPointF(cx, box.top() + box.height() * 0.16),
                   QPointF(cx, box.bottom() - box.height() * 0.12));
        p.drawLine(QPointF(cx, box.top() + box.height() * 0.16),
                   QPointF(cx - box.width() * 0.26, box.top() + box.height() * 0.42));
        p.drawLine(QPointF(cx, box.top() + box.height() * 0.16),
                   QPointF(cx + box.width() * 0.26, box.top() + box.height() * 0.42));
        break;
    }
    case Facet::Crypto: {
        // tiny closed lock
        const qreal bw = box.width() * 0.72;
        const qreal bh = box.height() * 0.5;
        const QRectF body(box.center().x() - bw / 2.0, box.bottom() - bh, bw, bh);
        p.setPen(Qt::NoPen);
        p.setBrush(c);
        QPainterPath bp;
        bp.addRoundedRect(body, 2, 2);
        p.fillPath(bp, c);
        const qreal sw = bw * 0.62;
        QRectF shackle(box.center().x() - sw / 2.0, body.top() - sw * 0.6, sw, sw);
        p.setBrush(Qt::NoBrush);
        p.setPen(pen);
        p.drawArc(shackle, 0, 180 * 16);
        p.drawLine(QPointF(shackle.left(), shackle.center().y()),
                   QPointF(shackle.left(), body.top()));
        p.drawLine(QPointF(shackle.right(), shackle.center().y()),
                   QPointF(shackle.right(), body.top()));
        break;
    }
    }
    p.restore();
}

} // namespace SnapClone::UI
