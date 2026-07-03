#include "ui/ConversationSecurityBar.hpp"
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
inline QColor col(const char *hex) { return QColor(QString::fromLatin1(hex)); }
inline QColor alpha(QColor c, int a) { c.setAlpha(a); return c; }

constexpr int kPad = Theme::SPACE_MD;
constexpr int kGlyph = 20;       // lock glyph box
constexpr int kToggleW = 46;     // toggle track width
constexpr int kToggleH = 26;     // toggle track height
} // namespace

ConversationSecurityBar::ConversationSecurityBar(QWidget *parent)
    : QWidget(parent)
{
    setAttribute(Qt::WA_OpaquePaintEvent, false);
    setMinimumHeight(kToggleH + kPad * 2);
    setCursor(Qt::PointingHandCursor);
}

ConversationSecurityBar::~ConversationSecurityBar() = default;

QSize ConversationSecurityBar::sizeHint() const
{
    return QSize(Theme::vwPx(96), kToggleH + kPad * 2);
}

// ── state ───────────────────────────────────────────────────────────────────

void ConversationSecurityBar::setTier(Tier tier)
{
    if (tier == m_tier) {
        return;
    }
    m_tier = tier;
    if (m_tier == Tier::Securing) {
        ensurePulse();
    }
    update();
}

void ConversationSecurityBar::setPrivateRequested(bool on)
{
    if (on == m_requested) {
        return;
    }
    m_requested = on;
    ensurePulse(); // animate the knob toward its new position
    update();
}

QColor ConversationSecurityBar::tierColor() const
{
    switch (m_tier) {
    case Tier::Private:  return col(Theme::SUCCESS());
    case Tier::Securing: return col(Theme::ACCENT());
    case Tier::Standard: return col(Theme::TEXT_SECONDARY());
    }
    return col(Theme::TEXT_SECONDARY());
}

QString ConversationSecurityBar::tierTitle() const
{
    switch (m_tier) {
    case Tier::Private:  return QStringLiteral("Private");
    case Tier::Securing: return QStringLiteral("Securing…");
    case Tier::Standard: return QStringLiteral("Standard");
    }
    return QString();
}

QString ConversationSecurityBar::tierSubtitle() const
{
    switch (m_tier) {
    case Tier::Private:
        return QStringLiteral("Post-quantum · obfuscated");
    case Tier::Securing:
        return QStringLiteral("Upgrading to post-quantum");
    case Tier::Standard:
        return m_requested ? QStringLiteral("Awaiting the others")
                           : QStringLiteral("End-to-end encrypted");
    }
    return QString();
}

// ── animation ─────────────────────────────────────────────────────────────────

void ConversationSecurityBar::ensurePulse()
{
    if (!m_pulse) {
        m_pulse = new QTimer(this);
        m_pulse->setInterval(33);
        connect(m_pulse, &QTimer::timeout, this, &ConversationSecurityBar::onPulse);
    }
    if (!m_pulse->isActive()) {
        m_pulse->start();
    }
}

void ConversationSecurityBar::onPulse()
{
    const float target = m_requested ? 1.0f : 0.0f;
    m_knob += (target - m_knob) * 0.3f;
    m_phase += 0.1f;
    if (m_phase > 6.2831853f) {
        m_phase -= 6.2831853f;
    }
    const bool knobSettled = std::abs(m_knob - target) < 0.005f;
    if (knobSettled) {
        m_knob = target;
    }
    // keep ticking only while the handshake pulses or the knob is still moving
    if (m_tier != Tier::Securing && knobSettled) {
        m_pulse->stop();
    }
    update();
}

// ── geometry ──────────────────────────────────────────────────────────────────

QRectF ConversationSecurityBar::lockRect() const
{
    return QRectF(kPad, (height() - kGlyph) / 2.0, kGlyph, kGlyph);
}

QRectF ConversationSecurityBar::toggleRect() const
{
    return QRectF(width() - kPad - kToggleW, (height() - kToggleH) / 2.0,
                  kToggleW, kToggleH);
}

// ── input ─────────────────────────────────────────────────────────────────────

void ConversationSecurityBar::mouseReleaseEvent(QMouseEvent *e)
{
    // Tapping the toggle (or its generous right region) flips the request.
    QRectF hit = toggleRect().adjusted(-kPad, -kPad / 2.0, kPad, kPad / 2.0);
    if (hit.contains(e->position())) {
        Q_EMIT privateRequestToggled(!m_requested);
    }
    QWidget::mouseReleaseEvent(e);
}

// ── painting ──────────────────────────────────────────────────────────────────

void ConversationSecurityBar::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);

    // card
    QPainterPath card;
    card.addRoundedRect(rect().adjusted(1, 1, -1, -1), Theme::RADIUS_MD, Theme::RADIUS_MD);
    p.fillPath(card, col(Theme::BG_CARD()));

    const QColor accent = tierColor();
    // left accent edge: a soft cue without shouting
    {
        QPainterPath edge;
        edge.addRoundedRect(QRectF(1, 1, 4, height() - 2), 2, 2);
        const int a = (m_tier == Tier::Securing)
                          ? 120 + static_cast<int>(100 * (0.5 + 0.5 * std::sin(m_phase)))
                          : 200;
        p.fillPath(edge, alpha(accent, std::min(a, 255)));
    }

    // lock glyph
    paintLockGlyph(p, lockRect(), accent, m_tier != Tier::Standard);

    // title + subtitle
    const qreal textX = lockRect().right() + Theme::SPACE_MD;
    const qreal textW = toggleRect().left() - kPad - textX;

    QFont titleFont;
    titleFont.setPixelSize(Theme::FONT_BASE);
    titleFont.setWeight(QFont::DemiBold);
    p.setFont(titleFont);
    p.setPen(col(Theme::TEXT_PRIMARY()));
    const QRectF titleRect(textX, height() / 2.0 - Theme::FONT_BASE, textW, Theme::FONT_BASE + 2);
    p.drawText(titleRect, Qt::AlignLeft | Qt::AlignBottom,
               p.fontMetrics().elidedText(tierTitle(), Qt::ElideRight, static_cast<int>(textW)));

    QFont subFont;
    subFont.setPixelSize(Theme::FONT_XS);
    p.setFont(subFont);
    p.setPen(col(Theme::TEXT_SECONDARY()));
    const QRectF subRect(textX, height() / 2.0 + 1, textW, Theme::FONT_XS + 3);
    p.drawText(subRect, Qt::AlignLeft | Qt::AlignTop,
               p.fontMetrics().elidedText(tierSubtitle(), Qt::ElideRight, static_cast<int>(textW)));

    // toggle
    paintToggle(p);
}

void ConversationSecurityBar::paintLockGlyph(QPainter &p, const QRectF &r,
                                             const QColor &c, bool closed) const
{
    p.save();
    // body
    const qreal bw = r.width() * 0.74;
    const qreal bh = r.height() * 0.52;
    const QRectF body(r.center().x() - bw / 2.0, r.bottom() - bh, bw, bh);
    QPainterPath bodyPath;
    bodyPath.addRoundedRect(body, 3, 3);
    p.fillPath(bodyPath, c);

    // shackle
    const qreal sw = bw * 0.66;
    QRectF shackle(r.center().x() - sw / 2.0, body.top() - sw * 0.62, sw, sw);
    QPen pen(c);
    pen.setWidthF(2.0);
    pen.setCapStyle(Qt::RoundCap);
    p.setBrush(Qt::NoBrush);
    p.setPen(pen);

    if (closed) {
        p.drawArc(shackle, 0, 180 * 16);
        p.drawLine(QPointF(shackle.left(), shackle.center().y()),
                   QPointF(shackle.left(), body.top()));
        p.drawLine(QPointF(shackle.right(), shackle.center().y()),
                   QPointF(shackle.right(), body.top()));
    } else {
        // open: lift and tilt the shackle, detach the right leg
        QRectF s = shackle.translated(2.0, -2.0);
        p.drawArc(s, 0, 180 * 16);
        p.drawLine(QPointF(s.left(), s.center().y()), QPointF(s.left(), body.top()));
        p.drawLine(QPointF(s.right(), s.center().y()), QPointF(s.right(), body.top() - 3.0));
    }

    // keyhole
    p.setPen(Qt::NoPen);
    p.setBrush(col(Theme::BG_CARD()));
    p.drawEllipse(QPointF(body.center().x(), body.center().y()), 1.6, 1.6);
    p.restore();
}

void ConversationSecurityBar::paintToggle(QPainter &p)
{
    const QRectF track = toggleRect();
    const qreal radius = track.height() / 2.0;

    // track colour blends with the eased knob position
    const QColor on = col(Theme::ACCENT());
    const QColor off = col(Theme::BG_RAISED());
    const int mix = static_cast<int>(std::clamp(m_knob, 0.0f, 1.0f) * 255.0f);
    QColor trackColor(
        (off.red() * (255 - mix) + on.red() * mix) / 255,
        (off.green() * (255 - mix) + on.green() * mix) / 255,
        (off.blue() * (255 - mix) + on.blue() * mix) / 255);

    QPainterPath tp;
    tp.addRoundedRect(track, radius, radius);
    p.fillPath(tp, trackColor);

    // knob
    const qreal knobD = track.height() - 6.0;
    const qreal travel = track.width() - knobD - 6.0;
    const qreal knobX = track.left() + 3.0 + travel * std::clamp(m_knob, 0.0f, 1.0f);
    p.setPen(Qt::NoPen);
    p.setBrush(col(Theme::TEXT_INVERSE()));
    p.drawEllipse(QRectF(knobX, track.top() + 3.0, knobD, knobD));
}

} // namespace SnapClone::UI
