#include "ui/GroupVoiceBar.hpp"
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

constexpr int kPadding = 10;      // inner margin
constexpr int kGlyphPad = 12;     // padding inside the talk button for the mic glyph
constexpr int kLatchSwipePx = 44; // upward swipe (px) on the talk button to latch TX on
} // namespace

GroupVoiceBar::GroupVoiceBar(QWidget *parent)
    : QWidget(parent)
{
    setMinimumHeight(int(Theme::TOUCH_LARGE * m_talkScale) + kPadding * 2);
    setAttribute(Qt::WA_OpaquePaintEvent, false);
}

GroupVoiceBar::~GroupVoiceBar() = default;

QSize GroupVoiceBar::sizeHint() const
{
    return QSize(Theme::vwPx(90), int(Theme::TOUCH_LARGE * m_talkScale) + kPadding * 2);
}

// ── geometry ──────────────────────────────────────────────────────────────────

QRectF GroupVoiceBar::talkRect() const
{
    const qreal d = Theme::TOUCH_LARGE * m_talkScale;
    const qreal y = (height() - d) / 2.0;
    return QRectF(width() - d - kPadding, y, d, d);
}

QRectF GroupVoiceBar::lockRect() const
{
    const qreal d = Theme::TOUCH_ICON * 0.7;
    const qreal y = (height() - d) / 2.0;
    return QRectF(kPadding + 4, y, d, d);
}

QRectF GroupVoiceBar::centerRect() const
{
    const QRectF lr = lockRect();
    const QRectF tr = talkRect();
    const qreal left = lr.right() + kPadding;
    const qreal right = tr.left() - kPadding;
    return QRectF(left, kPadding, qMax<qreal>(0, right - left), height() - kPadding * 2);
}

// ── public slots ──────────────────────────────────────────────────────────────

void GroupVoiceBar::setSecurityState(Security state)
{
    if (m_security == state) return;
    m_security = state;
    ensurePulse(m_onAir || m_security == Security::Securing);
    update(lockRect().toRect().adjusted(-6, -6, 6, 6));
    update(centerRect().toRect());
}

void GroupVoiceBar::setTalker(const QString &name, bool isSelf)
{
    if (m_talker == name && m_talkerIsSelf == isSelf) return;
    m_talker = name;
    m_talkerIsSelf = isSelf;
    update(centerRect().toRect());
}

void GroupVoiceBar::setMicLevel(float level01)
{
    m_micLevel = std::clamp(level01, 0.0f, 1.0f);
    // motion eases in onPulse(); ensure the pulse runs while there's signal
    if (m_micLevel > 0.01f) ensurePulse(true);
}

void GroupVoiceBar::setOnAir(bool onAir)
{
    if (m_onAir == onAir) return;
    m_onAir = onAir;
    // If transmit was stopped from outside (floor lost, call ended), drop any
    // hands-free latch so the UI matches and the next tap starts cleanly.
    if (!onAir) { m_talkLatched = false; m_pttDown = false; }
    ensurePulse(m_onAir || m_security == Security::Securing);
    update();
}

void GroupVoiceBar::setTalkProminent(bool primary)
{
    const double s = FrequentButton::scale(FrequentButton::ExpandWhenPrimary, primary);
    if (qFuzzyCompare(m_talkScale, s)) return;
    m_talkScale = s;
    setMinimumHeight(int(Theme::TOUCH_LARGE * m_talkScale) + kPadding * 2);
    updateGeometry(); // sizeHint changed → let the layout re-query our height
    update();
}

// ── pulse / motion ────────────────────────────────────────────────────────────

void GroupVoiceBar::ensurePulse(bool running)
{
    if (running) {
        if (!m_pulse) {
            m_pulse = new QTimer(this);
            m_pulse->setInterval(33); // ~30 fps
            connect(m_pulse, &QTimer::timeout, this, &GroupVoiceBar::onPulse);
        }
        if (!m_pulse->isActive()) m_pulse->start();
    } else if (m_pulse) {
        m_pulse->stop();
        m_displayLevel = 0.0f;
        m_phase = 0.0f;
        update();
    }
}

void GroupVoiceBar::onPulse()
{
    m_phase += 0.12f;
    if (m_phase > 6.2831853f) m_phase -= 6.2831853f;

    // ease the displayed level toward the live level (calm, no jitter)
    const float target = m_onAir ? m_micLevel : 0.0f;
    m_displayLevel += (target - m_displayLevel) * 0.25f;

    // stop once everything has settled and there's nothing to animate
    const bool idle = !m_onAir && m_security != Security::Securing
                      && m_displayLevel < 0.005f && m_micLevel < 0.01f;
    if (idle) {
        ensurePulse(false);
        return;
    }
    update();
}

// ── painting ──────────────────────────────────────────────────────────────────

void GroupVoiceBar::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);
    paintCard(p);
    paintLock(p);
    paintCenter(p);
    paintTalk(p);
}

void GroupVoiceBar::paintCard(QPainter &p)
{
    const QRectF r = rect().adjusted(1, 1, -1, -1);
    QPainterPath path;
    path.addRoundedRect(r, Theme::RADIUS_MD, Theme::RADIUS_MD);
    p.fillPath(path, col(Theme::BG_CARD()));
    p.setPen(QPen(col(Theme::BORDER()), 1.0));
    p.drawPath(path);
}

void GroupVoiceBar::paintLock(QPainter &p)
{
    const QRectF r = lockRect();
    QColor c;
    bool locked = true;
    switch (m_security) {
    case Security::Public:
        c = col(Theme::TEXT_SECONDARY());
        locked = false;
        break;
    case Security::Securing: {
        // gentle pulse between dim accent and accent while handshaking
        const float t = (std::sin(m_phase) + 1.0f) / 2.0f;
        c = QColor(col(Theme::ACCENT_DIM()));
        const QColor hi = col(Theme::ACCENT());
        c = QColor(
            c.red()   + int((hi.red()   - c.red())   * t),
            c.green() + int((hi.green() - c.green()) * t),
            c.blue()  + int((hi.blue()  - c.blue())  * t));
        locked = true;
        break;
    }
    case Security::Private:
        c = col(Theme::SUCCESS());
        locked = true;
        break;
    }
    paintLockGlyph(p, r, c, locked);
}

void GroupVoiceBar::paintCenter(QPainter &p)
{
    const QRectF r = centerRect();
    if (r.width() < 8) return;

    // talker / hint line
    QString line;
    QColor textColor;
    if (m_onAir || m_talkerIsSelf) {
        line = tr("You");
        textColor = col(Theme::ACCENT());
    } else if (!m_talker.isEmpty()) {
        line = m_talker;
        textColor = col(Theme::TEXT_PRIMARY());
    } else {
        line = tr("Hold to talk");
        textColor = col(Theme::TEXT_SECONDARY());
    }

    QFont f = font();
    f.setPixelSize(Theme::FONT_BASE);
    f.setWeight(QFont::Medium);
    p.setFont(f);
    p.setPen(textColor);

    const qreal meterH = 4.0;
    const qreal gap = 6.0;
    QFontMetrics fm(f);
    const qreal textH = fm.height();
    const qreal blockH = textH + gap + meterH;
    const qreal top = r.top() + (r.height() - blockH) / 2.0;

    const QRectF textRect(r.left(), top, r.width(), textH);
    p.drawText(textRect, Qt::AlignVCenter | Qt::AlignLeft,
               fm.elidedText(line, Qt::ElideRight, int(r.width())));

    // mic level meter — a soft rounded track that fills with accent by level
    const QRectF track(r.left(), top + textH + gap, r.width(), meterH);
    QPainterPath trackPath;
    trackPath.addRoundedRect(track, meterH / 2.0, meterH / 2.0);
    p.fillPath(trackPath, alpha(col(Theme::TEXT_SECONDARY()), 60));

    const float lvl = m_onAir ? m_displayLevel : 0.0f;
    if (lvl > 0.001f) {
        QRectF fill = track;
        fill.setWidth(track.width() * std::clamp(lvl, 0.0f, 1.0f));
        QPainterPath fillPath;
        fillPath.addRoundedRect(fill, meterH / 2.0, meterH / 2.0);
        p.fillPath(fillPath, col(Theme::ACCENT()));
    }
}

void GroupVoiceBar::paintTalk(QPainter &p)
{
    const QRectF r = talkRect();
    const bool live = m_onAir || m_pttDown || m_talkLatched;

    if (live) {
        // soft outer glow while on air
        const float t = (std::sin(m_phase) + 1.0f) / 2.0f;
        const int glowA = 40 + int(40 * t);
        p.setPen(Qt::NoPen);
        p.setBrush(alpha(col(Theme::ACCENT()), glowA));
        const qreal grow = 4.0 + 3.0 * t;
        p.drawEllipse(r.adjusted(-grow, -grow, grow, grow));
    }

    p.setBrush(live ? col(Theme::ACCENT()) : col(Theme::BG_RAISED()));
    p.setPen(QPen(live ? col(Theme::ACCENT()) : col(Theme::BORDER()), 1.5));
    p.drawEllipse(r);

    const QColor glyphColor = live ? col(Theme::TEXT_INVERSE()) : col(Theme::TEXT_SECONDARY());
    paintMicGlyph(p, r.adjusted(kGlyphPad, kGlyphPad, -kGlyphPad, -kGlyphPad), glyphColor);

    if (m_talkLatched) {
        // small lock badge → transmit is latched on (hands-free), tap to release
        const qreal bs = r.width() * 0.30;
        const QRectF badge(r.right() - bs, r.top() - bs * 0.15, bs, bs);
        p.setPen(Qt::NoPen);
        p.setBrush(col(Theme::BG_RAISED()));
        p.drawEllipse(badge);
        paintLockGlyph(p, badge.adjusted(bs * 0.22, bs * 0.22, -bs * 0.22, -bs * 0.22),
                       col(Theme::ACCENT()), /*locked=*/true);
    }
}

void GroupVoiceBar::paintMicGlyph(QPainter &p, const QRectF &r, const QColor &c) const
{
    // a simple microphone: rounded capsule head + U-stand + stem
    p.save();
    p.setPen(QPen(c, 2.0, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
    p.setBrush(Qt::NoBrush);

    const qreal capW = r.width() * 0.46;
    const qreal capH = r.height() * 0.50;
    const QRectF head((r.center().x() - capW / 2.0), r.top(), capW, capH);
    p.setBrush(c);
    p.drawRoundedRect(head, capW / 2.0, capW / 2.0);

    p.setBrush(Qt::NoBrush);
    const qreal standTop = head.bottom() - 1.0;
    const qreal standBot = head.bottom() + r.height() * 0.22;
    const QRectF arcRect(head.left() - capW * 0.30, standTop - (standBot - standTop),
                         capW + capW * 0.60, (standBot - standTop) * 2.0);
    p.drawArc(arcRect, 180 * 16, 180 * 16); // bottom half-arc (the cradle)

    const qreal stemX = r.center().x();
    p.drawLine(QPointF(stemX, standBot), QPointF(stemX, r.bottom()));
    p.drawLine(QPointF(stemX - capW * 0.45, r.bottom()), QPointF(stemX + capW * 0.45, r.bottom()));
    p.restore();
}

void GroupVoiceBar::paintLockGlyph(QPainter &p, const QRectF &r, const QColor &c, bool locked) const
{
    p.save();
    p.setPen(QPen(c, 2.0, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));

    // body
    const qreal bodyW = r.width() * 0.74;
    const qreal bodyH = r.height() * 0.50;
    const QRectF body(r.center().x() - bodyW / 2.0, r.bottom() - bodyH, bodyW, bodyH);
    p.setBrush(alpha(c, 60));
    p.drawRoundedRect(body, 3.0, 3.0);

    // shackle — closed = symmetric arc on top; open = hinged to one side
    p.setBrush(Qt::NoBrush);
    const qreal shW = bodyW * 0.62;
    const qreal shH = r.height() * 0.40;
    if (locked) {
        const QRectF sh(r.center().x() - shW / 2.0, body.top() - shH, shW, shH * 2.0);
        p.drawArc(sh, 0, 180 * 16); // top half — closed loop
    } else {
        // open: arc lifted and rotated, leaving a gap
        const QRectF sh(r.center().x() - shW / 2.0 + shW * 0.18,
                        body.top() - shH * 1.1, shW, shH * 2.0);
        p.drawArc(sh, 20 * 16, 170 * 16);
    }

    // keyhole
    p.setBrush(c);
    p.setPen(Qt::NoPen);
    const qreal kh = bodyW * 0.16;
    p.drawEllipse(QPointF(body.center().x(), body.center().y() - kh * 0.2), kh / 2.0, kh / 2.0);
    p.restore();
}

// ── input ─────────────────────────────────────────────────────────────────────

void GroupVoiceBar::mousePressEvent(QMouseEvent *e)
{
    const QPointF pos = e->position();
    if (talkRect().adjusted(-6, -6, 6, 6).contains(pos)) {
        if (m_talkLatched) {
            // Latched (hands-free) → a tap releases the floor.
            m_talkLatched = false;
            m_pttDown = false;
            update(talkRect().toRect().adjusted(-16, -16, 16, 16));
            Q_EMIT talkReleased();
            e->accept();
            return;
        }
        m_pttDown = true;
        m_pressPos = pos;
        ensurePulse(true);
        update(talkRect().toRect().adjusted(-16, -16, 16, 16));
        Q_EMIT talkPressed();
        e->accept();
        return;
    }
    if (lockRect().adjusted(-8, -8, 8, 8).contains(pos)) {
        // tapping the lock toggles the private-mode REQUEST; the controller and
        // the peer decide the actual tier (private needs both + handshake).
        const bool want = m_security == Security::Public;
        Q_EMIT privateModeRequested(want);
        e->accept();
        return;
    }
    QWidget::mousePressEvent(e);
}

void GroupVoiceBar::mouseMoveEvent(QMouseEvent *e)
{
    // Hold the talk button and swipe up past the threshold to latch transmit on
    // (hands-free). Once latched, the finger can lift and the floor is held until
    // the next tap on the button.
    if (m_pttDown && !m_talkLatched
        && (m_pressPos.y() - e->position().y()) >= kLatchSwipePx) {
        m_talkLatched = true;
        update(talkRect().toRect().adjusted(-16, -16, 16, 16));
        e->accept();
        return;
    }
    QWidget::mouseMoveEvent(e);
}

void GroupVoiceBar::mouseReleaseEvent(QMouseEvent *e)
{
    if (m_pttDown) {
        if (m_talkLatched) {
            // Finger lifts but transmit stays on (latched) — tap to release.
            m_pttDown = false;
            update(talkRect().toRect().adjusted(-16, -16, 16, 16));
            e->accept();
            return;
        }
        m_pttDown = false;
        update(talkRect().toRect().adjusted(-16, -16, 16, 16));
        Q_EMIT talkReleased();
        e->accept();
        return;
    }
    QWidget::mouseReleaseEvent(e);
}

} // namespace SnapClone::UI
