#include "ui/video/ShutterButton.hpp"
#include "ui/Theme.hpp"

#include <QPainter>
#include <QMouseEvent>
#include <QEvent>
#include <QTouchEvent>
#include <QTimer>
#include <QtMath>
#include <QLoggingCategory>

namespace SnapClone::UI::Video {

Q_LOGGING_CATEGORY(lcShutter, "snapclone.video.shutter")

ShutterButton::ShutterButton(QWidget *parent)
    : QWidget(parent)
{
    // We MUST set WA_AcceptTouchEvents so the touch goes to us directly.
    //
    // Background: SwipeNavigator (an ancestor) sets WA_AcceptTouchEvents on
    // itself. Without WA_AcceptTouchEvents on us, Qt routes the touch UP the
    // parent chain to the first ancestor that accepts touch — which is the
    // swiper. The swiper accepts it for swipe-detection, which suppresses
    // Qt's mouse-event synthesis. Result: our mousePressEvent never fires.
    //
    // With WA_AcceptTouchEvents set here, Qt delivers the touch to us first.
    // We handle it in event() below — converting touch points into the
    // mouse-event semantics our existing press/move/release logic uses.
    setAttribute(Qt::WA_AcceptTouchEvents);
    setCursor(Qt::PointingHandCursor);

    m_holdTimer = new QTimer(this);
    m_holdTimer->setSingleShot(true);
    m_holdTimer->setInterval(kHoldThresholdMs);
    connect(m_holdTimer, &QTimer::timeout, this, &ShutterButton::onHoldTimeout);

    // V32.31: press-watchdog. Fires once at kPressWatchdogMs (~1s) after
    // recording starts. If by then m_pressed has gone false WITHOUT the
    // recording being locked, we treat that as a missed release event
    // (touch driver glitch, swallowed-by-OS, focus-change race) and
    // stop the recording gracefully instead of getting stuck running
    // forever until the user force-quits.
    m_pressWatchdog = new QTimer(this);
    m_pressWatchdog->setSingleShot(true);
    m_pressWatchdog->setInterval(kPressWatchdogMs);
    connect(m_pressWatchdog, &QTimer::timeout,
            this, &ShutterButton::onPressWatchdogTick);
}

ShutterButton::~ShutterButton() = default;

QSize ShutterButton::sizeHint() const
{
    return QSize(78, 78);
}

void ShutterButton::setRecording(bool recording)
{
    if (m_recording == recording) return;
    m_recording = recording;
    if (!recording) {
        m_locked = false;
        m_recordingTimer.invalidate();
        // V32.31: stop the press-watchdog whenever recording goes off,
        // regardless of cause. Prevents a stale tick after external
        // teardown (max-duration timeout, max recorder error, etc).
        if (m_pressWatchdog) m_pressWatchdog->stop();
    } else if (!m_recordingTimer.isValid()) {
        // External caller turned recording on without going through hold —
        // start the duration clock now so duration reporting still works.
        m_recordingTimer.restart();
    }
    update();
}

void ShutterButton::setLocked(bool locked)
{
    if (m_locked == locked) return;
    m_locked = locked;
    update();
}

void ShutterButton::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);

    const QRectF r = rect().adjusted(2, 2, -2, -2);
    const qreal radius = qMin(r.width(), r.height()) / 2.0;
    const QPointF c = r.center();

    // Theme-aware colors. The ring uses TEXT_PRIMARY which is white on dark
    // palettes and dark on light palettes. The recording state uses a fixed
    // red that's universally understood as "recording" but with the theme's
    // accent as a subtle outer ring tint so it harmonizes.
    const QColor textPrimary = QColor(QString::fromLatin1(Theme::TEXT_PRIMARY()));
    const QColor accent       = QColor(QString::fromLatin1(Theme::ACCENT()));
    const QColor recordRed    = QColor(232, 60, 60);

    // Outer ring
    QColor ringColor = m_recording ? recordRed : textPrimary;
    p.setPen(QPen(ringColor, 4));
    p.setBrush(Qt::NoBrush);
    p.drawEllipse(c, radius - 2, radius - 2);

    // Inner fill
    if (m_recording && m_locked) {
        // Locked: stop icon (rounded square in record red)
        p.setBrush(recordRed);
        p.setPen(Qt::NoPen);
        const qreal sq = radius * 0.7;
        p.drawRoundedRect(QRectF(c.x() - sq/2, c.y() - sq/2, sq, sq), 6, 6);
    } else if (m_recording) {
        // Recording (not locked): pulsing red disc
        p.setBrush(recordRed);
        p.setPen(Qt::NoPen);
        p.drawEllipse(c, radius - 8, radius - 8);
    } else {
        // Idle: theme-foreground disc inside ring (classic shutter look,
        // tinted to match the active palette)
        p.setBrush(textPrimary);
        p.setPen(Qt::NoPen);
        p.drawEllipse(c, radius - 8, radius - 8);

        // Subtle accent tint inner halo so the user sees the theme color
        if (accent.isValid() && accent != textPrimary) {
            p.setPen(QPen(accent, 1.5));
            p.setBrush(Qt::NoBrush);
            p.drawEllipse(c, radius - 9, radius - 9);
        }
    }
}

void ShutterButton::mousePressEvent(QMouseEvent *e)
{
    if (e->button() != Qt::LeftButton) {
        QWidget::mousePressEvent(e);
        return;
    }
    e->accept();
    m_pressed   = true;
    m_pressPos  = e->pos();
    m_pressTimer.restart();

    // If we're already locked, this press should toggle recording OFF
    // (no need for a hold). We emit recordStopped on release.
    if (m_locked) return;

    // Otherwise, start the hold-detection timer. If the user releases
    // before it fires, it's a tap (photo). If it fires while still pressed,
    // we transition to recording mode.
    m_holdTimer->start();
}

void ShutterButton::mouseMoveEvent(QMouseEvent *e)
{
    if (!m_pressed) {
        QWidget::mouseMoveEvent(e);
        return;
    }

    // Idle (not recording): a downward swipe opens the voice/text mode picker.
    // Measured from the press position; it fires once and cancels the press, so
    // it neither snaps a photo on release nor lets the hold timer start a
    // recording. The record-lock swipe below is UPWARD and only while recording,
    // and a tap stays within kTapMotionTolerancePx (8px) — well under the 40px
    // threshold — so none of the three gestures collide.
    if (!m_recording) {
        const int down = e->pos().y() - m_pressPos.y();  // positive = downward
        if (down >= kSwipeDownThresholdPx) {
            qCInfo(lcShutter) << "Voice/text mode via swipe-down (dy=" << down << ")";
            m_pressed = false;
            m_holdTimer->stop();
            if (m_pressWatchdog) m_pressWatchdog->stop();
            update();
            emit voiceModeRequested();
            e->accept();
            return;
        }
    }

    // While recording but not locked: detect upward swipe → lock.
    // We measure FROM the press position, so the user can swipe in any
    // direction; only sustained upward motion locks.
    if (m_recording && !m_locked) {
        const int dy = m_pressPos.y() - e->pos().y();  // positive = upward
        if (dy >= kSwipeUpThresholdPx) {
            qCInfo(lcShutter) << "Recording locked via swipe-up (dy=" << dy << ")";
            m_locked = true;
            // CRITICAL: end the press lifecycle here. Without this, the user's
            // natural release immediately fires recordStopped() — exactly the
            // symptom that looks like "tapped to stop right after lock".
            // The next NEW press (after a real release) is what stops a
            // locked recording, via the m_locked branch in mousePressEvent.
            m_pressed = false;
            m_holdTimer->stop();
            // V32.31: lock deliberately clears m_pressed. If we leave the
            // press-watchdog armed, it'll see m_pressed=false + recording=true
            // + locked=true and... actually, the watchdog's m_locked check
            // protects it. Stopping the timer here is belt-and-braces.
            if (m_pressWatchdog) m_pressWatchdog->stop();
            update();
            emit recordLocked();
        }
    }
    e->accept();
}

void ShutterButton::mouseReleaseEvent(QMouseEvent *e)
{
    if (e->button() != Qt::LeftButton) {
        QWidget::mouseReleaseEvent(e);
        return;
    }
    e->accept();

    const bool wasPressed = m_pressed;
    m_pressed = false;
    m_holdTimer->stop();
    // V32.31: any normal release means the watchdog is no longer needed.
    // If we're not recording, harmless no-op. If we ARE, the rest of this
    // function will handle teardown — the watchdog would just fire a
    // duplicate stop a few hundred ms later.
    if (m_pressWatchdog) m_pressWatchdog->stop();

    if (!wasPressed) return;

    // Case A: locked recording — tap stops it.
    // We use m_recordingTimer (started when onHoldTimeout fired) for total
    // recording duration. The press timer for THIS press would only measure
    // the brief tap-to-stop, not the full recording length.
    if (m_locked) {
        const int dur = m_recordingTimer.isValid()
                      ? static_cast<int>(m_recordingTimer.elapsed())
                      : static_cast<int>(m_pressTimer.elapsed());
        qCInfo(lcShutter) << "Stop recording via tap-while-locked (recorded"
                          << dur << "ms)";
        // Transition local state IMMEDIATELY (before emit) so the widget
        // doesn't believe it's still recording while the external pipeline
        // tears down asynchronously. Without this, a stray release/tap during
        // teardown could fire another recordStopped emission.
        m_recording = false;
        m_locked    = false;
        m_recordingTimer.invalidate();
        update();
        emit recordStopped(dur);
        return;
    }

    // Case B: recording started during this press, user released without locking
    if (m_recording) {
        const int dur = m_recordingTimer.isValid()
                      ? static_cast<int>(m_recordingTimer.elapsed())
                      : static_cast<int>(m_pressTimer.elapsed());
        qCInfo(lcShutter) << "Stop recording via release (recorded" << dur << "ms)";
        m_recording = false;
        m_locked    = false;
        m_recordingTimer.invalidate();
        update();
        emit recordStopped(dur);
        return;
    }

    // Case C: short tap — photo capture. Verify both time AND distance.
    const int dx = qAbs(e->pos().x() - m_pressPos.x());
    const int dy = qAbs(e->pos().y() - m_pressPos.y());
    if (m_pressTimer.elapsed() < kHoldThresholdMs
     && dx <= kTapMotionTolerancePx
     && dy <= kTapMotionTolerancePx) {
        qCInfo(lcShutter) << "Photo tap";
        emit clicked();
    } else {
        qCDebug(lcShutter) << "Press cancelled (held" << m_pressTimer.elapsed()
                           << "ms, motion" << dx << "/" << dy << ")";
    }
}

bool ShutterButton::event(QEvent *e)
{
    // Translate touch events into mouse-event semantics so our existing
    // mousePressEvent/mouseMoveEvent/mouseReleaseEvent handlers (which are
    // well-tested for the four-bug fixes) run unchanged on touch.
    //
    // This is necessary because we set WA_AcceptTouchEvents (see ctor),
    // which makes Qt deliver QTouchEvent here directly and suppress its
    // automatic mouse synthesis.
    switch (e->type()) {
    case QEvent::TouchBegin: {
        auto *te = static_cast<QTouchEvent *>(e);
        if (te->points().isEmpty()) return false;
        const QPointF pos = te->points().first().position();
        QMouseEvent fake(QEvent::MouseButtonPress, pos, mapToGlobal(pos.toPoint()),
                         Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        mousePressEvent(&fake);
        e->accept();
        return true;
    }
    case QEvent::TouchUpdate: {
        auto *te = static_cast<QTouchEvent *>(e);
        if (te->points().isEmpty()) return false;
        const QPointF pos = te->points().first().position();
        QMouseEvent fake(QEvent::MouseMove, pos, mapToGlobal(pos.toPoint()),
                         Qt::NoButton, Qt::LeftButton, Qt::NoModifier);
        mouseMoveEvent(&fake);
        e->accept();
        return true;
    }
    case QEvent::TouchEnd: {
        auto *te = static_cast<QTouchEvent *>(e);
        const QPointF pos = te->points().isEmpty()
            ? m_pressPos
            : te->points().first().position();
        QMouseEvent fake(QEvent::MouseButtonRelease, pos, mapToGlobal(pos.toPoint()),
                         Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
        mouseReleaseEvent(&fake);
        e->accept();
        return true;
    }
    case QEvent::TouchCancel: {
        // System cancelled the touch (lock screen activated, app backgrounded,
        // notification panel pulled down, etc). DON'T translate to a release —
        // that would fire a phantom photo capture if the user hadn't yet
        // crossed the hold threshold. Just clear all state cleanly.
        qCDebug(lcShutter) << "Touch cancelled — clearing state without firing tap";
        m_pressed = false;
        m_holdTimer->stop();
        // V32.31: cancel the press-watchdog too. The cancel path below
        // already stops recording if it was active, so the watchdog has
        // nothing to do anyway.
        if (m_pressWatchdog) m_pressWatchdog->stop();
        // If we WERE recording, treat cancel like a stop (so the recorder
        // pipeline gets torn down). Use the recordingTimer for accurate duration.
        if (m_recording || m_locked) {
            const int dur = m_recordingTimer.isValid()
                          ? static_cast<int>(m_recordingTimer.elapsed())
                          : 0;
            m_recording = false;
            m_locked    = false;
            m_recordingTimer.invalidate();
            update();
            emit recordStopped(dur);
        } else {
            update();  // repaint to reset visual state
        }
        e->accept();
        return true;
    }
    default:
        break;
    }
    return QWidget::event(e);
}

void ShutterButton::onHoldTimeout()
{
    if (!m_pressed) return;
    qCInfo(lcShutter) << "Hold detected — starting recording";
    m_recording = true;
    // Independent timer for total recording duration. m_pressTimer measures
    // single press lifecycles; this measures total recording time across
    // the lock-then-tap-to-stop boundary.
    m_recordingTimer.restart();
    update();
    // V32.31: arm the press-watchdog. If 1s passes and we're still
    // recording but m_pressed has gone false (without entering locked
    // mode), we missed a release event and the watchdog will fire stop.
    if (m_pressWatchdog) m_pressWatchdog->start();
    emit recordStarted();
}

void ShutterButton::onPressWatchdogTick()
{
    // V32.31: safety check 1s after recordStarted. Three outcomes:
    //   1. Still recording, still pressed → user is genuinely holding;
    //      do nothing, recording continues normally.
    //   2. Still recording but locked → recording continues hands-free
    //      by design. m_pressed was deliberately cleared by the
    //      lock-swipe path. NOT a glitch.
    //   3. Still recording but NOT pressed and NOT locked → the release
    //      event went missing. Synthesize a stop with the elapsed
    //      duration so we don't hang.
    if (!m_recording) return;        // already stopped legitimately
    if (m_locked) return;            // hands-free mode — caller controls stop
    if (m_pressed) return;           // genuinely held — let normal release handle it

    // Missed-release recovery. Use whatever duration we have so the
    // user still gets a (short, ~1s) video rather than nothing.
    const int dur = m_recordingTimer.isValid()
                  ? static_cast<int>(m_recordingTimer.elapsed())
                  : 0;
    qCWarning(lcShutter) << "Press watchdog fired — release event was lost,"
                            " stopping recording after" << dur << "ms";
    // Same state-clearing pattern as the normal release path.
    m_recording = false;
    m_locked    = false;
    m_recordingTimer.invalidate();
    update();
    emit recordStopped(dur);
}


void ShutterButton::changeEvent(QEvent *e)
{
    QWidget::changeEvent(e);
    // Theme switch: qApp->setStyleSheet sends StyleChange to all widgets.
    // Our paint reads Theme::TEXT_PRIMARY/ACCENT directly, so we just need to
    // schedule a repaint to pick up the new colors.
    if (e->type() == QEvent::StyleChange || e->type() == QEvent::PaletteChange) {
        update();
    }
}
} // namespace SnapClone::UI::Video
