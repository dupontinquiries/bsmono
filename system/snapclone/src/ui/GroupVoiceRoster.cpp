#include "ui/GroupVoiceRoster.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QPaintEvent>
#include <QMouseEvent>
#include <QTimer>
#include <QFont>
#include <QFontMetrics>
#include <cmath>

namespace SnapClone::UI {

namespace {
// Theme color slots are "#RRGGBB" strings; wrap them as QColor.
inline QColor col(const char *hex) { return QColor(QString::fromLatin1(hex)); }
inline QColor alpha(QColor c, int a) { c.setAlpha(a); return c; }

constexpr int kAvatar = 36;     // avatar diameter
constexpr int kRowPad = Theme::SPACE_SM;

QString initialOf(const QString &name) {
    for (const QChar ch : name) {
        if (ch.isLetterOrNumber()) {
            return QString(ch.toUpper());
        }
    }
    return QStringLiteral("?");
}
} // namespace

GroupVoiceRoster::GroupVoiceRoster(QWidget *parent)
    : QWidget(parent)
{
    setAttribute(Qt::WA_OpaquePaintEvent, false);
    setMinimumHeight(rowHeight());
}

GroupVoiceRoster::~GroupVoiceRoster() = default;

int GroupVoiceRoster::rowHeight() const
{
    return kAvatar + kRowPad * 2;
}

QSize GroupVoiceRoster::sizeHint() const
{
    const int rows = m_people.isEmpty() ? 1 : m_people.size();
    return QSize(Theme::vwPx(90), rows * rowHeight());
}

void GroupVoiceRoster::setParticipants(const QList<Participant> &people)
{
    m_people = people.toVector();
    updateGeometry();
    update();
}

void GroupVoiceRoster::setActiveSpeaker(const QString &id)
{
    if (id == m_activeId) {
        return;
    }
    m_activeId = id;
    if (id.isEmpty()) {
        m_level = 0.0f; // ease the ring back down before stopping
    }
    ensurePulse(!id.isEmpty());
    update();
}

void GroupVoiceRoster::setSpeakerLevel(float level01)
{
    m_level = std::clamp(level01, 0.0f, 1.0f);
    if (!m_activeId.isEmpty()) {
        ensurePulse(true);
    }
}

// ── animation ─────────────────────────────────────────────────────────────────

void GroupVoiceRoster::ensurePulse(bool running)
{
    if (running) {
        if (!m_pulse) {
            m_pulse = new QTimer(this);
            m_pulse->setInterval(33); // ~30 fps, gentle
            connect(m_pulse, &QTimer::timeout, this, &GroupVoiceRoster::onPulse);
        }
        if (!m_pulse->isActive()) {
            m_pulse->start();
        }
    } else if (m_pulse) {
        m_pulse->stop();
        m_displayLevel = 0.0f;
        m_phase = 0.0f;
        update();
    }
}

void GroupVoiceRoster::onPulse()
{
    // ease the displayed level toward the target (calm, no jitter)
    m_displayLevel += (m_level - m_displayLevel) * 0.25f;
    m_phase += 0.12f;
    if (m_phase > 6.2831853f) {
        m_phase -= 6.2831853f;
    }
    // once everything has settled to silence, stop animating
    if (m_activeId.isEmpty() && m_displayLevel < 0.01f) {
        ensurePulse(false);
        return;
    }
    update();
}

// ── input ─────────────────────────────────────────────────────────────────────

int GroupVoiceRoster::rowIndexAt(const QPointF &pos) const
{
    if (m_people.isEmpty()) {
        return -1;
    }
    const int idx = static_cast<int>(pos.y()) / rowHeight();
    return (idx >= 0 && idx < m_people.size()) ? idx : -1;
}

void GroupVoiceRoster::mouseReleaseEvent(QMouseEvent *e)
{
    const int idx = rowIndexAt(e->position());
    if (idx >= 0) {
        Q_EMIT participantActivated(m_people[idx].id);
    }
    QWidget::mouseReleaseEvent(e);
}

// ── painting ──────────────────────────────────────────────────────────────────

void GroupVoiceRoster::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);

    const int rh = rowHeight();
    for (int i = 0; i < m_people.size(); ++i) {
        const Participant &person = m_people[i];
        const bool active = !m_activeId.isEmpty() && person.id == m_activeId;
        const QRectF row(0, i * rh, width(), rh);
        paintRow(p, person, row, active);
    }
}

void GroupVoiceRoster::paintRow(QPainter &p, const Participant &person,
                                const QRectF &row, bool active)
{
    // Active row gets a soft accent wash; others stay quiet.
    if (active) {
        const QRectF card = row.adjusted(kRowPad / 2.0, kRowPad / 2.0,
                                         -kRowPad / 2.0, -kRowPad / 2.0);
        QPainterPath path;
        path.addRoundedRect(card, Theme::RADIUS_MD, Theme::RADIUS_MD);
        p.fillPath(path, alpha(col(Theme::ACCENT()), 28));
    }

    const QRectF avatarCell(row.left() + kRowPad, row.top() + kRowPad, kAvatar, kAvatar);
    paintAvatar(p, avatarCell, person.name, active, person.isSelf);

    // name + (optional) status
    const qreal textX = avatarCell.right() + Theme::SPACE_SM;
    const qreal textW = row.right() - textX - kRowPad;

    QFont nameFont;
    nameFont.setPixelSize(Theme::FONT_BASE);
    nameFont.setWeight(active ? QFont::DemiBold : QFont::Normal);
    p.setFont(nameFont);

    const QString name = person.name.isEmpty() ? QStringLiteral("Unknown") : person.name;
    const QString shown = person.isSelf ? name + QStringLiteral("  ·  You") : name;

    if (active) {
        // name above a small "speaking" caption
        const QRectF nameRect(textX, row.top() + kRowPad,
                              textW, row.height() / 2.0 - kRowPad / 2.0);
        p.setPen(col(Theme::TEXT_PRIMARY()));
        p.drawText(nameRect, Qt::AlignLeft | Qt::AlignBottom,
                   p.fontMetrics().elidedText(shown, Qt::ElideRight, static_cast<int>(textW)));

        QFont capFont;
        capFont.setPixelSize(Theme::FONT_XS);
        p.setFont(capFont);
        p.setPen(col(Theme::ACCENT()));
        const QRectF capRect(textX, row.center().y(), textW, row.height() / 2.0 - kRowPad);
        p.drawText(capRect, Qt::AlignLeft | Qt::AlignTop, QStringLiteral("speaking"));
    } else {
        p.setPen(col(Theme::TEXT_PRIMARY()));
        const QRectF nameRect(textX, row.top(), textW, row.height());
        p.drawText(nameRect, Qt::AlignLeft | Qt::AlignVCenter,
                   p.fontMetrics().elidedText(shown, Qt::ElideRight, static_cast<int>(textW)));
    }
}

void GroupVoiceRoster::paintAvatar(QPainter &p, const QRectF &cell,
                                   const QString &name, bool active, bool /*isSelf*/)
{
    // breathing ring for the active speaker, modulated by their level
    if (active) {
        const float breath = 0.5f + 0.5f * std::sin(m_phase);
        const float energy = std::clamp(m_displayLevel, 0.0f, 1.0f);
        const qreal grow = 3.0 + energy * 7.0;            // ring spread with level
        const int ringAlpha = 60 + static_cast<int>((120.0f * (0.35f + 0.65f * energy)) * breath);
        const QRectF ring = cell.adjusted(-grow, -grow, grow, grow);
        QPen ringPen(alpha(col(Theme::ACCENT()), std::min(ringAlpha, 220)));
        ringPen.setWidthF(1.5 + energy * 2.5);
        p.setPen(ringPen);
        p.setBrush(Qt::NoBrush);
        p.drawEllipse(ring);
    }

    // avatar disc
    p.setPen(Qt::NoPen);
    p.setBrush(col(active ? Theme::ACCENT_DIM() : Theme::BG_RAISED()));
    p.drawEllipse(cell);
    if (!active) {
        QPen border(alpha(col(Theme::BORDER()), 180));
        border.setWidthF(1.0);
        p.setPen(border);
        p.setBrush(Qt::NoBrush);
        p.drawEllipse(cell.adjusted(0.5, 0.5, -0.5, -0.5));
    }

    // initial
    QFont initFont;
    initFont.setPixelSize(Theme::FONT_LG);
    initFont.setWeight(QFont::DemiBold);
    p.setFont(initFont);
    p.setPen(col(active ? Theme::TEXT_INVERSE() : Theme::TEXT_PRIMARY()));
    p.drawText(cell, Qt::AlignCenter, initialOf(name));
}

} // namespace SnapClone::UI
