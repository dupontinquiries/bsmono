#include "ui/SwipeNavigator.hpp"
#include "ui/SubTabbed.hpp"   // V32.34+: dynamic_cast target for two-level nav

#include <QTouchEvent>
#include <QMouseEvent>
#include <QAbstractButton>
#include <QPushButton>
#include <QComboBox>
#include <QAbstractScrollArea>
#include <QScrollBar>
#include <QSlider>
#include <QSpinBox>
#include <QLineEdit>
#include <QKeyEvent>
#include <QPainter>   // V32.34+: detent barrier overlay
#include <QFont>      // V32.39+: grow-text scaling
#include <QLinearGradient>  // V32.34+: material edge-glow barrier
#include <QColor>
#include <QSettings>  // V32.34+: ui/unified_nav flag
#include "ui/Theme.hpp"
#include <QResizeEvent>
#include <QWheelEvent>
#include <QChildEvent>
#include <QTimer>
#include <QLineF>
#include <QEasingCurve>
#include <QApplication>
#include <QCheckBox>
#include <QWidget>
#include <QPointer>
#include <QtMath>
#include <QDateTime>                  // V32.32: gesture watchdog timestamps
#include <QLoggingCategory>           // V32.32: lcSwipeNav category
#include <QGraphicsOpacityEffect>   // V32.30: crossfade on commit
#include <QPropertyAnimation>       // V32.30: crossfade on commit

// V32.32: scoped logging category for SwipeNavigator state machine.
// Filter at runtime with QT_LOGGING_RULES="snapclone.ui.swipenav.debug=true"
// when diagnosing axis-lock / passthrough bugs.
Q_LOGGING_CATEGORY(lcSwipeNav, "snapclone.ui.swipenav")

// ─── UI/SwipeNavigator.cpp ────────────────────────────────────────────────────
//
// Physics-based horizontal swipe navigation.
//
// Design principles:
//
//   1. ALL thresholds are fractions of m_pageWidth, never absolute pixels.
//      kCommitFraction = 0.22 means "drag 22% of the page to commit" whether
//      the screen is 390px or 1440px. The old code used kSwipeMinPx = 28px
//      which is 7.2% on a 390px phone — fine — but only 1.9% on a 1440px
//      desktop, causing accidental commits on micro-drags.
//
//   2. Rubber-band uses sqrt(), not linear scaling.
//      offset = sign * sqrt(|drag| * pageW * k)
//      At 10px drag on a 390px page: sqrt(10*390*0.4) = 39px of visual movement.
//      At 100px drag: sqrt(100*390*0.4) = 125px. Sub-linear resistance means:
//      - slow pushes feel like you're stretching something
//      - fast pushes feel like a real elastic limit
//      The old flat 0.3× multiplier felt the same at all speeds — wrong.
//
//   3. Commit animation duration scales with release velocity.
//      duration = clamp(pageW / (|velocity| * kDeceleration), minMs, maxMs)
//      Fast flick (v=2.0 px/ms): 390 / (2.0 * 2.5) = 78ms → clamped to 60ms.
//      Slow drag released near threshold: uses maxMs=340ms for gentle settle.
//      The old code used (remaining_distance / pageW) * 250ms — ignored velocity.
//
//   4. Bounce depth is proportional to pull depth.
//      Bouncing back from 5px pull: 90ms (barely felt).
//      Bouncing back from 80px pull: 90 + 80*0.35 = 118ms (noticeable snap).
//      The old fixed 160ms OutBack felt the same regardless of how hard you'd
//      pushed — the overshoot scaled with pull depth in a way that felt wrong.
//
//   5. Velocity window widened to 120ms with exponential weighting.
//      The old 80ms window missed slow deliberate swipes where the thumb is
//      still moving slowly at lift-off. 120ms with exponential decay weights
//      recent samples more, so a fast flick still reads fast, and a slow
//      deliberate drag still reads as the slow release it is.
//
//   6. Direction lock threshold lowered to 8px (was 12px).
//      Fast thumbs lock direction early; this prevents diagonal moves from
//      getting rejected because the lock threshold was too high.
//
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

SwipeNavigator::SwipeNavigator(QWidget *parent)
    : QWidget(parent)
{
    setAttribute(Qt::WA_AcceptTouchEvents);
    setFocusPolicy(Qt::StrongFocus);
    setAttribute(Qt::WA_TouchPadAcceptSingleTouchEvents);

    m_anim = new QPropertyAnimation(this, "slideOffset", this);
    // Default easing; overridden per-animation in physics functions.
    m_anim->setEasingCurve(QEasingCurve::OutCubic);

    // V32.32: gesture watchdog. Runs continuously at 1Hz; only does work
    // when a gesture is in flight. Detects stuck-gesture-no-end-event
    // bugs and clears state so the navigator doesn't ignore the next
    // touch. See header for design rationale.
    m_gestureWatchdog = new QTimer(this);
    m_gestureWatchdog->setInterval(kGestureWatchdogIntervalMs);
    connect(m_gestureWatchdog, &QTimer::timeout,
            this, &SwipeNavigator::onGestureWatchdogTick);
    m_gestureWatchdog->start();

    QApplication::instance()->installEventFilter(this);

    // V32.34+: read the scroll→sub-tab direction-invert preference once at
    // startup (cheap, and the value only changes via Settings + restart, like
    // the unified-nav flag). Natural-scroll users set this so the detent
    // advances the way their content moves.
    {
        QSettings s;
        m_detentInvert = s.value(QStringLiteral("ui/scroll_detent_invert"),
                                 false).toBool();
        m_detentInvertMouse =
            s.value(QStringLiteral("ui/scroll_detent_invert_mouse"),
                    false).toBool();
        // V32.34+ mobile field tuning: touch detent direction felt reversed on
        // device. On a touchscreen the platform norm is content-follows-finger
        // (natural), so the touch detent now DEFAULTS to inverted relative to
        // the wheel/trackpad path. Still user-overridable via Settings; the
        // mouse + generic defaults are unchanged. If it feels wrong, flip
        // ui/scroll_detent_invert_touch back to false in Settings.
        m_detentInvertTouch =
            s.value(QStringLiteral("ui/scroll_detent_invert_touch"),
                    true).toBool();
    }
}

SwipeNavigator::~SwipeNavigator()
{
    if (QApplication::instance())
        QApplication::instance()->removeEventFilter(this);
}

bool SwipeNavigator::commitScrollPaced(QScrollBar *hbar, QScrollBar *vbar,
                                       int dx, int dy)
{
    // Accumulate the requested delta into pending state.
    m_pendingScrollDx += dx;
    m_pendingScrollDy += dy;

    // First call: timer hasn't started, commit immediately and start it.
    if (!m_scrollPacingTimer.isValid()) {
        m_scrollPacingTimer.start();
        commitScrollDelta(hbar, vbar, m_pendingScrollDx, m_pendingScrollDy);
        m_pendingScrollDx = 0;
        m_pendingScrollDy = 0;
        return true;
    }

    // Subsequent calls: only commit if enough time has passed since the
    // last commit. Otherwise the delta stays buffered and rides along on
    // the next call that does pass the gate.
    if (m_scrollPacingTimer.elapsed() < kScrollPacingMs) {
        return false; // buffered, not committed
    }
    commitScrollDelta(hbar, vbar, m_pendingScrollDx, m_pendingScrollDy);
    m_pendingScrollDx = 0;
    m_pendingScrollDy = 0;
    m_scrollPacingTimer.restart();
    return true;
}

void SwipeNavigator::commitScrollDelta(QScrollBar *hbar, QScrollBar *vbar,
                                       int dx, int dy)
{
    // Apply the scroll delta to the bars, capturing any clamped OVERFLOW (the
    // portion the bar couldn't accept because it's at min/max) and routing it
    // into the overscroll system. V32.34+: this is what makes "scroll passover"
    // work on a DIRECT DRAG (not just a fling) — previously commitScrollPaced
    // clamped at the edge and discarded the overflow, so dragging to the bottom
    // and continuing to pull did nothing and the vertical sub-tab detent could
    // only ever fire via kinetic fling. Now the overflow feeds applyOverscroll,
    // which arms/fires the detent under unifiedNavEnabled(). Inert visual-wise
    // when the flag is off except for the normal edge rubber-band (which is the
    // pre-existing behavior the fling path already produced).
    qreal overflowX = 0.0, overflowY = 0.0;
    if (hbar && dx != 0) {
        const int before = hbar->value();
        hbar->setValue(before - dx);
        const int actual = hbar->value();
        if (actual == before)              overflowX = static_cast<qreal>(dx);
        else if ((actual - before) != -dx) overflowX = static_cast<qreal>(dx - (-(actual - before)));
    }
    if (vbar && dy != 0) {
        const int before = vbar->value();
        vbar->setValue(before - dy);
        const int actual = vbar->value();
        // V32.36+ INVERSION DIAGNOSTIC (user reported "inverse bouncy" touch
        // scroll). The intended mapping is value -= dy: finger DOWN (dy>0)
        // lowers the scroll value so content follows the finger (natural
        // scrolling). This logs the actual relationship so a device capture
        // shows definitively whether (a) our mapping is inverted, or (b) the
        // scrollbar moved the OPPOSITE way we asked — which would mean a
        // competing native/OS scroll handler is also acting on the same
        // gesture (double-handling), the more likely cause given the code path
        // is self-consistent. Expected: dy>0 → delta<0 (value dropped).
        const int delta = actual - before;
        if (delta != 0 && ((dy > 0) == (delta > 0))) {
            qCInfo(lcSwipeNav, "[SCROLL-INVERT?] dy=%d but value moved %+d "
                               "(same sign — expected opposite). Possible "
                               "competing native scroll handler.", dy, delta);
        }
        if (actual == before)              overflowY = static_cast<qreal>(dy);
        else if ((actual - before) != -dy) overflowY = static_cast<qreal>(dy - (-(actual - before)));
    }
    if (overflowX != 0.0 || overflowY != 0.0)
        applyOverscroll(overflowX, overflowY);
}

void SwipeNavigator::flushPendingScroll(QScrollBar *hbar, QScrollBar *vbar)
{
    // Called on TouchEnd / gesture-reset to make sure no motion is lost.
    // V32.34+: route the final delta through commitScrollDelta so its clamped
    // overflow also feeds overscroll (a direct drag that ends right at the
    // edge still produces the detent/bounce), then spring back any overscroll
    // accumulated during the drag (the fling path springs back on its own; a
    // pure drag has no fling tick to do it).
    //
    // BOUNCE-ON-TAP FIX (V32.34+): only flush if we ACTUALLY committed to a
    // scroll this gesture (m_scrollCommitted) OR the pending delta is real
    // (≥ a few px). A tap can leave a couple px of finger jitter in
    // m_pendingScroll* without ever crossing the axis-lock threshold; flushing
    // that at the edge turned into overscroll → spring-back = the "bounce on
    // tap" the user saw. Tap jitter is discarded instead. (kTapJitterPx small,
    // well under kLockThresholdPx so genuine micro-scrolls aren't lost.)
    constexpr int kTapJitterPx = 3;
    const bool realPending =
        m_scrollCommitted
        || qAbs(m_pendingScrollDx) > kTapJitterPx
        || qAbs(m_pendingScrollDy) > kTapJitterPx;
    if (realPending
        && ((hbar && m_pendingScrollDx != 0) || (vbar && m_pendingScrollDy != 0)))
        commitScrollDelta(hbar, vbar, m_pendingScrollDx, m_pendingScrollDy);
    m_pendingScrollDx = 0;
    m_pendingScrollDy = 0;
    m_scrollPacingTimer.invalidate();
    if (hasOverscroll())
        startSpringBack();
}

// ─── V32.34+: kinetic fling implementation ──────────────────────────────────

void SwipeNavigator::recordVelocitySample(qreal dx, qreal dy)
{
    // First sample of the gesture: start the clock. Subsequent samples
    // use elapsed() to get monotonic ms since the gesture's first
    // motion event. We DON'T reset to TouchBegin time because the user
    // might rest a finger for a moment before scrolling — that pause
    // shouldn't dilute the velocity computation.
    if (!m_velocityClock.isValid()) {
        m_velocityClock.start();
        m_velocitySamples.clear();
    }
    const qint64 t = m_velocityClock.elapsed();
    m_velocitySamples.append(VelocitySample{t, dy, dx});

    // Prune ancient samples eagerly so the list doesn't grow unbounded
    // during a long slow scroll. We keep ~3× the velocity window
    // because computeReleaseVelocity will filter on its own; the extra
    // is cheap insurance.
    const qint64 cutoff = t - (kVelocityWindowMs * 3);
    while (!m_velocitySamples.isEmpty()
           && m_velocitySamples.first().timeMs < cutoff) {
        m_velocitySamples.removeFirst();
    }
}

void SwipeNavigator::computeReleaseVelocity(qreal *outVx, qreal *outVy) const
{
    *outVx = 0.0;
    *outVy = 0.0;
    if (m_velocitySamples.size() < 2 || !m_velocityClock.isValid()) return;

    const qint64 now    = m_velocityClock.elapsed();
    const qint64 cutoff = now - kVelocityWindowMs;

    // Walk back through samples, summing delta and tracking time span.
    // We want: pixels moved in the trailing window / window duration.
    qreal sumDx = 0.0, sumDy = 0.0;
    qint64 firstKept = -1;
    for (int i = m_velocitySamples.size() - 1; i >= 0; --i) {
        const auto &s = m_velocitySamples[i];
        if (s.timeMs < cutoff) break;
        sumDx += s.deltaX;
        sumDy += s.deltaY;
        firstKept = s.timeMs;
    }
    if (firstKept < 0) return;
    const qint64 span = now - firstKept;
    if (span <= 0) return;  // all samples at the same ms — meaningless to divide

    // Velocity in user-coordinates: positive dy = finger moved DOWN,
    // which in the scroll path means content moved UP (we use
    // setValue(value - dy) when committing). We preserve that sign
    // convention here so onFlingTick can use the same formula.
    *outVx = sumDx / static_cast<qreal>(span);
    *outVy = sumDy / static_cast<qreal>(span);
}

void SwipeNavigator::startFling(qreal vx, qreal vy)
{
    if (!m_scrollTarget) return;

    // V32.34+: velocity boost on consecutive flings.
    // If the previous fling ended recently (within kBoostWindowMs) AND
    // its direction matches this one, add the residual velocity to
    // the new flick. This is the Android "fling-fling-fling" pattern
    // that lets users build up speed through long lists.
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    if (m_lastFlingEndTimeMs > 0
        && (nowMs - m_lastFlingEndTimeMs) <= kBoostWindowMs) {
        // Same-direction check per axis. A previous y-velocity of -2
        // (scrolling up) plus a new y-velocity of -1 → -3 (boost).
        // A previous y of -2 plus new of +1 → +1 (user reversed,
        // no boost, take the new value as-is). Sign-match via
        // multiplication: positive product means same direction.
        if (m_lastFlingVelocityX != 0.0 && vx * m_lastFlingVelocityX > 0.0) {
            vx += m_lastFlingVelocityX;
        }
        if (m_lastFlingVelocityY != 0.0 && vy * m_lastFlingVelocityY > 0.0) {
            vy += m_lastFlingVelocityY;
        }
        qCDebug(lcSwipeNav, "fling: boost — previous v=(%g, %g), combined v=(%g, %g)",
                m_lastFlingVelocityX, m_lastFlingVelocityY, vx, vy);
    }
    // Reset the stash now that we've consumed it — prevents a third
    // consecutive flick from compounding twice.
    m_lastFlingVelocityX = 0.0;
    m_lastFlingVelocityY = 0.0;
    m_lastFlingEndTimeMs = 0;

    // Clamp to kMaxFlingVelocity to keep the experience usable. A
    // user mashing fast flicks shouldn't blast through a page in
    // 50ms — that's disorienting and looks like a glitch.
    if (vx >  kMaxFlingVelocity) vx =  kMaxFlingVelocity;
    if (vx < -kMaxFlingVelocity) vx = -kMaxFlingVelocity;
    if (vy >  kMaxFlingVelocity) vy =  kMaxFlingVelocity;
    if (vy < -kMaxFlingVelocity) vy = -kMaxFlingVelocity;

    // Threshold check: ignore micro-residuals from "I tried to stop
    // but my finger still had a hair of motion". Below kMinFlingVelocity
    // (300 px/sec) we just stop dead — same as the pre-fling behavior.
    if (qAbs(vx) < kMinFlingVelocity && qAbs(vy) < kMinFlingVelocity) {
        qCDebug(lcSwipeNav, "fling: skipped — velocity (%g, %g) px/ms below threshold",
                vx, vy);
        return;
    }

    m_flingVelocityX = vx;
    m_flingVelocityY = vy;
    m_flingTarget    = m_scrollTarget;  // QPointer, auto-nulls if dies

    if (!m_flingTimer) {
        // Lazy-create; parented to `this` for lifetime. Connect once.
        m_flingTimer = new QTimer(this);
        m_flingTimer->setInterval(kFlingTickIntervalMs);
        m_flingTimer->setTimerType(Qt::PreciseTimer);  // ms-precise decay
        connect(m_flingTimer, &QTimer::timeout, this, &SwipeNavigator::onFlingTick);
    }
    m_flingLastTick.restart();
    m_flingTimer->start();

    qCDebug(lcSwipeNav, "fling: started v=(%g, %g) px/ms target=%p",
            vx, vy, static_cast<void*>(m_flingTarget.data()));
}

void SwipeNavigator::cancelFling()
{
    if (m_flingTimer && m_flingTimer->isActive()) {
        m_flingTimer->stop();
        // V32.34+: stash the residual velocity + timestamp so a quick
        // re-flick can boost off this one. The stash is consumed by
        // the next startFling that finds it in-window; otherwise the
        // boost logic ages out naturally because we compare against
        // currentMSecsSinceEpoch.
        m_lastFlingVelocityX = m_flingVelocityX;
        m_lastFlingVelocityY = m_flingVelocityY;
        m_lastFlingEndTimeMs = QDateTime::currentMSecsSinceEpoch();
        qCDebug(lcSwipeNav, "fling: cancelled (was v=(%g, %g) px/ms — stashed for boost)",
                m_flingVelocityX, m_flingVelocityY);
    }
    m_flingVelocityX = 0.0;
    m_flingVelocityY = 0.0;
    m_flingTarget    = nullptr;
}

void SwipeNavigator::onFlingTick()
{
    if (!m_flingTarget) {
        // Target died (page change while flinging, widget deleted, etc).
        // QPointer auto-nulled. Stop the timer cleanly.
        if (m_flingTimer) m_flingTimer->stop();
        return;
    }

    // Elapsed since last tick. PreciseTimer is best-effort; on a busy
    // event loop the actual interval can stretch. Reading the elapsed
    // time per-tick rather than assuming kFlingTickIntervalMs gives
    // accurate distance integration even when ticks are late.
    const qreal dtMs = static_cast<qreal>(m_flingLastTick.restart());
    if (dtMs <= 0) return;  // first tick after start(); skip

    QScrollBar *hbar = m_flingTarget->horizontalScrollBar();
    QScrollBar *vbar = m_flingTarget->verticalScrollBar();

    // Distance this tick: v (px/ms) × dt (ms) = px.
    // Sign convention matches commitScrollPaced: value -= delta.
    const int dx = static_cast<int>(m_flingVelocityX * dtMs);
    const int dy = static_cast<int>(m_flingVelocityY * dtMs);

    // V32.34+: edge overflow goes into overscroll instead of just
    // zeroing the velocity. The user sees a rubber-band-curved
    // translation past the edge that decays as fling decays; at the
    // end of the fling, spring-back animates the offset to zero.
    qreal overflowX = 0.0;
    qreal overflowY = 0.0;
    if (hbar && dx != 0) {
        const int before = hbar->value();
        hbar->setValue(before - dx);
        const int actual = hbar->value();
        if (actual == before) {
            // Fully clamped. All requested dx becomes overflow.
            overflowX = static_cast<qreal>(dx);
        } else if ((actual - before) != -dx) {
            // Partially clamped — we got some movement but not all.
            // The unaccepted portion is overflow. setValue moves by
            // (actual - before) = -dx_accepted, so dx_accepted =
            // -(actual - before). Overflow = dx_requested - dx_accepted.
            const int dxAccepted = -(actual - before);
            overflowX = static_cast<qreal>(dx - dxAccepted);
        }
    }
    if (vbar && dy != 0) {
        const int before = vbar->value();
        vbar->setValue(before - dy);
        const int actual = vbar->value();
        if (actual == before) {
            overflowY = static_cast<qreal>(dy);
        } else if ((actual - before) != -dy) {
            const int dyAccepted = -(actual - before);
            overflowY = static_cast<qreal>(dy - dyAccepted);
        }
    }
    // If there's any overflow this tick, apply it as overscroll.
    // Don't zero the velocity yet — the rubber-band curve naturally
    // limits how far the overscroll can extend, and decay continues
    // pumping smaller and smaller deltas in. When the fling decay
    // completes, the spring-back animation takes over.
    if (overflowX != 0.0 || overflowY != 0.0) {
        applyOverscroll(overflowX, overflowY);
    }

    // Exponential decay: v(t+dt) = v(t) · exp(-dt/tau).
    // tau = 325ms → ~37% at 325ms, ~14% at 650ms, ~5% at 1s.
    // qExp is the Qt-wrapped std::exp; uses the platform's libm.
    const qreal decay = qExp(-dtMs / kFlingTauMs);
    m_flingVelocityX *= decay;
    m_flingVelocityY *= decay;

    // Stop condition: both velocities below the stop floor.
    // (Removed the "hitEdge && done" condition — with overscroll, an
    // edge hit no longer kills velocity; the decay still has to run
    // to natural completion. If the user keeps the fling alive with
    // a re-flick boost while at the edge, more overscroll just piles
    // up — that's correct behavior.)
    const bool xDone = qAbs(m_flingVelocityX) < kFlingStopVelocity;
    const bool yDone = qAbs(m_flingVelocityY) < kFlingStopVelocity;
    if (xDone && yDone) {
        m_flingTimer->stop();
        m_flingTarget    = nullptr;
        m_flingVelocityX = 0.0;
        m_flingVelocityY = 0.0;
        // V32.34+: kick off spring-back if we accumulated overscroll
        // during the fling. Otherwise we'd leave the viewport
        // permanently translated.
        if (hasOverscroll()) {
            startSpringBack();
        }
        qCDebug(lcSwipeNav, "fling: decay complete");
    }
}

// ─── V32.34+: overscroll bounce implementation ──────────────────────────────

bool SwipeNavigator::hasOverscroll() const
{
    // qFuzzyIsNull guards against floating-point noise (e.g. spring-back
    // leaving a sub-pixel residual when it should be zero).
    return !qFuzzyIsNull(m_overscrollX) || !qFuzzyIsNull(m_overscrollY);
}

void SwipeNavigator::applyOverscroll(qreal dx, qreal dy)
{
    // Pick the viewport target. We use m_scrollTarget if available
    // (live drag path); otherwise m_flingTarget (fling-tick path
    // after the gesture ended). Either way the viewport is the
    // scroll area's content-visible widget.
    QAbstractScrollArea *area = m_scrollTarget ? m_scrollTarget.data()
                                              : m_flingTarget.data();
    if (!area) return;
    QWidget *viewport = area->viewport();
    if (!viewport) return;

    // First overscroll of a streak: snapshot the viewport's anchor
    // position so spring-back / cancellation knows where to restore.
    // Subsequent overscroll calls keep the same anchor.
    if (m_overscrollViewport != viewport) {
        m_overscrollViewport = viewport;
        m_overscrollOrigPos  = viewport->pos();
        m_overscrollRawX     = 0.0;
        m_overscrollRawY     = 0.0;
        // V32.34+ bounce diagnosis: log when an overscroll STREAK begins and
        // from which path/magnitude. The user reported a "bounce on tap" — a
        // tap should produce NO overscroll, so if this logs with tiny dx/dy
        // right after a tap, the bounce is overscroll engaging on near-zero
        // motion (the thing to fix). Path tells us drag vs fling origin.
        qCInfo(lcSwipeNav, "[OVERSCROLL] streak begin: d=(%.1f, %.1f) via %s",
               dx, dy, m_scrollTarget ? "drag" : "fling");
    }

    // Accumulate raw delta. Sign convention: positive dx (= scrolling
    // RIGHT past the right edge) translates the viewport LEFT visually
    // (= content shifts left, revealing empty space on the right).
    // Same idea for dy. We negate when applying to viewport->move.
    m_overscrollRawX += dx;
    m_overscrollRawY += dy;

    // Rubber-band curve: visible offset = sign × sqrt(|raw| × viewport × k).
    // Sub-linear: pull more, gain less. The scaling factor uses
    // viewport size so the SAME flick feels right on tall/short screens.
    const qreal vw = static_cast<qreal>(viewport->width());
    const qreal vh = static_cast<qreal>(viewport->height());
    const qreal absRawX = qAbs(m_overscrollRawX);
    const qreal absRawY = qAbs(m_overscrollRawY);
    // V32.34+ field tuning: subtract a blended (px + viewport-fraction) deadzone
    // from the raw pull before the rubber-band curve. Pulls smaller than the
    // deadzone produce ZERO visible overscroll (no rubber-band on near-zero
    // motion / barely-past-the-edge), and the acceleration exponent (>1)
    // sharpens the onset just past the deadzone so it reads as intentional
    // rather than mushy. The deadzone scales with viewport size for HiDPI/LoDPI.
    const qreal dzX = overscrollDeadzone(vw);
    const qreal dzY = overscrollDeadzone(vh);
    const qreal effRawX = qMax(0.0, absRawX - dzX);
    const qreal effRawY = qMax(0.0, absRawY - dzY);
    // V32.39+: asymptotic crawl instead of sqrt-then-hard-clamp. The offset
    // approaches a soft ceiling but never reaches it, so the content keeps
    // following the finger (with ever-smaller motion) rather than freezing at a
    // wall — there is always ceiling space left to keep nudging the UI. Forward
    // and inverse live in SubPageDetent (tested) so a grab mid-spring resumes
    // from exactly the current offset.
    const qreal ceilX = vw * kOverscrollCeilingRatio;
    const qreal ceilY = vh * kOverscrollCeilingRatio;
    const qreal softX = ceilX * kOverscrollCrawlK;
    const qreal softY = ceilY * kOverscrollCrawlK;
    m_overscrollX = (m_overscrollRawX < 0 ? -1.0 : 1.0)
                  * SubPageDetent::crawlOffset(effRawX, ceilX, softX);
    m_overscrollY = (m_overscrollRawY < 0 ? -1.0 : 1.0)
                  * SubPageDetent::crawlOffset(effRawY, ceilY, softY);

    // Safety net only — the crawl asymptote already sits below this, so it never
    // triggers in normal use; it just guards against a pathological viewport.
    const qreal maxX = vw * kOverscrollMaxRatio;
    const qreal maxY = vh * kOverscrollMaxRatio;
    if (m_overscrollX >  maxX) m_overscrollX =  maxX;
    if (m_overscrollX < -maxX) m_overscrollX = -maxX;
    if (m_overscrollY >  maxY) m_overscrollY =  maxY;
    if (m_overscrollY < -maxY) m_overscrollY = -maxY;

    applyOverscrollToViewport();

    // V32.34+ unified nav: vertical end-of-content detent. The overscroll we
    // just accumulated IS the "kept pulling past the limit" gesture; let the
    // detent evaluator arm/fire a sub-tab advance from the raw vertical pull.
    // Inert unless the flag is on. Horizontal overscroll is NOT routed here —
    // horizontal sub-tab scrub is handled at gesture commit time, not via this
    // vertical content-scroll overscroll path.
    //
    // V32.34+ TOUCH PARITY: mirror the wheel path's takeover-lock gating
    // (feedWheelVerticalDetent zeroes its accumulator while locked). Without
    // this, a continued FINGER DRAG right after a detent fire keeps piling into
    // m_overscrollRawY, so the instant the lock expires the built-up value
    // re-fires immediately — the same "one gesture rips through several
    // sub-tabs" runaway the lock was added to stop, just via touch instead of
    // wheel. While locked we zero the raw vertical accumulator and skip the
    // evaluator so the post-lock drag starts a clean streak. (We keep the
    // horizontal overscroll + visual rubber-band intact; only the vertical
    // detent feed is gated, and only under unified nav.)
    if (unifiedNavEnabled() && scrollTakeoverLocked()) {
        m_overscrollRawY = 0.0;
        m_overscrollY    = 0.0;
        applyOverscrollToViewport();   // settle back so it doesn't hang pulled
        return;
    }
    if (unifiedNavEnabled() && !qFuzzyIsNull(m_overscrollRawY)) {
        // V32.34+ TOUCH PARITY: optional per-source invert, mirroring the
        // mouse/trackpad toggles. applyOverscroll is only ever reached from
        // touch (drag + fling), never the wheel, so flipping here can't affect
        // the wheel path. We flip ONLY the value handed to the detent — the
        // visual rubber-band (m_overscrollY, applied above) stays tied to the
        // true finger direction so the content always follows the finger; just
        // the sub-tab/page advance direction inverts when the user opts in.
        const qreal detentRawY = m_detentInvertTouch ? -m_overscrollRawY
                                                     :  m_overscrollRawY;
        evaluateVerticalDetent(detentRawY);
    }
}

void SwipeNavigator::applyOverscrollToViewport()
{
    if (!m_overscrollViewport) return;
    // Translate the viewport from its anchor. Negate sign so positive
    // m_overscrollY (= overflow PAST the bottom edge) shifts viewport
    // UP, visually pulling the content with it — same direction the
    // user's finger was moving when they hit the edge.
    const QPoint target = m_overscrollOrigPos + QPoint(
        -static_cast<int>(m_overscrollX),
        -static_cast<int>(m_overscrollY));
    m_overscrollViewport->move(target);
}

void SwipeNavigator::startSpringBack()
{
    if (!hasOverscroll() || !m_overscrollViewport) {
        clearOverscroll();
        return;
    }
    // Capture starting offsets — the timer interpolates from these
    // to zero. Using ease-out cubic for the curve (rapid initial
    // return, gentle settle near zero).
    m_springBackStartX = m_overscrollX;
    m_springBackStartY = m_overscrollY;

    if (!m_springBackTimer) {
        m_springBackTimer = new QTimer(this);
        m_springBackTimer->setInterval(kFlingTickIntervalMs);  // 60Hz
        m_springBackTimer->setTimerType(Qt::PreciseTimer);
        connect(m_springBackTimer, &QTimer::timeout,
                this, &SwipeNavigator::onSpringBackTick);
    }
    m_springBackClock.restart();
    m_springBackTimer->start();
    qCDebug(lcSwipeNav, "overscroll: spring-back from (%g, %g)",
            m_springBackStartX, m_springBackStartY);
}

void SwipeNavigator::cancelSpringBack()
{
    if (m_springBackTimer && m_springBackTimer->isActive()) {
        m_springBackTimer->stop();
    }
    // Note: we do NOT clear the overscroll offsets here. The whole
    // point of cancellation is "user grabbed during spring-back" —
    // they want to keep / extend the existing overscroll, not snap
    // back to zero. The raw-delta accumulator m_overscrollRaw* stays
    // at the value that produced the current m_overscroll*, so a
    // recompute on the next applyOverscroll call gives a continuous
    // visual experience.
    //
    // Reconstruct the raw accumulator from current visible offset
    // because if the user releases without further motion, we want
    // a fresh spring-back starting from where we are.
    if (m_overscrollViewport) {
        const qreal vw = static_cast<qreal>(m_overscrollViewport->width());
        const qreal vh = static_cast<qreal>(m_overscrollViewport->height());
        // V32.39+: inverse of the asymptotic crawl (SubPageDetent::crawlInverse),
        // matching the forward curve in applyOverscroll. |raw| = crawlInverse(
        // offset, ceiling, softness) + deadzone. Keeping these consistent means a
        // grab during spring-back resumes from exactly the current visible offset
        // rather than jumping.
        if (vw > 0) {
            const qreal dz    = overscrollDeadzone(vw);
            const qreal ceil  = vw * kOverscrollCeilingRatio;
            const qreal soft  = ceil * kOverscrollCrawlK;
            const qreal eff   = SubPageDetent::crawlInverse(qAbs(m_overscrollX), ceil, soft);
            m_overscrollRawX = (m_overscrollX < 0 ? -1.0 : 1.0) * (eff + dz);
        }
        if (vh > 0) {
            const qreal dz    = overscrollDeadzone(vh);
            const qreal ceil  = vh * kOverscrollCeilingRatio;
            const qreal soft  = ceil * kOverscrollCrawlK;
            const qreal eff   = SubPageDetent::crawlInverse(qAbs(m_overscrollY), ceil, soft);
            m_overscrollRawY = (m_overscrollY < 0 ? -1.0 : 1.0) * (eff + dz);
        }
    }
}

void SwipeNavigator::onSpringBackTick()
{
    if (!m_overscrollViewport) {
        if (m_springBackTimer) m_springBackTimer->stop();
        clearOverscroll();
        return;
    }
    const qint64 elapsed = m_springBackClock.elapsed();
    if (elapsed >= kSpringBackMs) {
        // Done — snap to anchor and clear state.
        m_springBackTimer->stop();
        m_overscrollX = 0.0;
        m_overscrollY = 0.0;
        m_overscrollRawX = 0.0;
        m_overscrollRawY = 0.0;
        applyOverscrollToViewport();
        m_overscrollViewport = nullptr;
        qCDebug(lcSwipeNav, "overscroll: spring-back complete");
        return;
    }
    // Ease-out cubic: f(t) = 1 - (1-t)^3. At t=0: 0; t=1: 1; gradient
    // is 3 at start, 0 at end. So position(t) = start × (1 - eased)
    // because we're interpolating from start → 0.
    const qreal t = static_cast<qreal>(elapsed) / static_cast<qreal>(kSpringBackMs);
    const qreal oneMinusT = 1.0 - t;
    const qreal eased = 1.0 - (oneMinusT * oneMinusT * oneMinusT);
    m_overscrollX = m_springBackStartX * (1.0 - eased);
    m_overscrollY = m_springBackStartY * (1.0 - eased);
    applyOverscrollToViewport();
}

void SwipeNavigator::clearOverscroll()
{
    if (m_springBackTimer && m_springBackTimer->isActive()) {
        m_springBackTimer->stop();
    }
    if (m_overscrollViewport) {
        // Restore viewport to its anchor before forgetting about it.
        m_overscrollViewport->move(m_overscrollOrigPos);
    }
    m_overscrollX = 0.0;
    m_overscrollY = 0.0;
    m_overscrollRawX = 0.0;
    m_overscrollRawY = 0.0;
    m_overscrollViewport = nullptr;
}

void SwipeNavigator::resetGestureState()
{
    // V32.32: centralized cleanup for end-of-gesture. Called from:
    //   • TouchEnd / TouchCancel (the normal path)
    //   • onGestureWatchdogTick when a gesture has been stuck > 2.5s
    //   • Direction-lock failure paths that explicitly want a clean
    //     start for the next gesture
    //
    // Does NOT touch m_slideOffset or animations — settling the
    // visible page position is owned by gestureEnd's commit/cancel
    // decision, which runs BEFORE this is called in the normal path.

    // V32.32: flush any deferred scroll delta before clearing the
    // target. Otherwise the last 16ms of finger motion gets dropped
    // when the user lifts off.
    if (m_scrollTarget) {
        flushPendingScroll(m_scrollTarget->horizontalScrollBar(),
                           m_scrollTarget->verticalScrollBar());
    } else {
        // No target but we may still have pending deltas — invalidate
        // the timer so the next gesture starts fresh.
        m_pendingScrollDx = 0;
        m_pendingScrollDy = 0;
        m_scrollPacingTimer.invalidate();
    }

    m_axisLock           = AxisLock::None;
    m_passingToScrollArea = false;
    m_scrollTarget        = nullptr;
    m_scrollCommitted     = false;
    m_skipGestureForArea  = nullptr;
    m_swipeLocked         = false;
    m_dragging            = false;
    m_gestureStartedAtMs  = 0;
    // V32.33: clear top-down intent so the next gesture starts unclassified.
    m_intent              = GestureIntent::None;
    // V32.33+ Phase 1: also clear the Phase 1 additions so the next
    // gesture starts with a clean Ambiguous-state setup.
    m_candidateIntent     = GestureIntent::None;
    m_affordanceFocus     = nullptr;
    // V32.32: release the single-finger lock so the NEXT TouchBegin
    // can take ownership. Without this, after a gesture finishes the
    // FSM would still think a gesture is in flight and refuse all
    // future touches — total app lockup.
    m_lockedPointId       = -1;
    // Note: m_currentGestureId is NOT reset to 0 — it's monotonically
    // increasing across all gestures the app sees. Bumped on TouchBegin.
}

void SwipeNavigator::onGestureWatchdogTick()
{
    // V32.32: stuck-gesture detection. A gesture is "in flight" when
    // m_gestureStartedAtMs is non-zero. If it's been longer than
    // kStuckGestureMs (2.5s) without a TouchEnd/TouchCancel resetting
    // the state, something went wrong — the user's touch was probably
    // delivered as TouchBegin but the End event got dropped (window
    // focus change, app backgrounded mid-touch, hardware glitch).
    // Force a reset so the next touch isn't ignored due to stale state.
    if (m_gestureStartedAtMs == 0) return;
    const qint64 nowMs   = QDateTime::currentMSecsSinceEpoch();
    const qint64 ageMs   = nowMs - m_gestureStartedAtMs;
    if (ageMs < kStuckGestureMs) return;
    qCWarning(lcSwipeNav, "Stuck gesture detected (id=%llu, age=%lldms) — "
                          "force-resetting state. Likely missed TouchEnd.",
              static_cast<unsigned long long>(m_currentGestureId),
              static_cast<long long>(ageMs));
    // If we had drag offset showing, snap back to origin via the cancel
    // animation rather than just resetting state and leaving a half-
    // translated page visible.
    if (m_swipeLocked && m_anim) {
        m_anim->stop();
        // V32.33+ Phase 1 audit: clear any previously-connected lambda
        // before starting a new animation. The watchdog fires when
        // something has already gone wrong (gesture stuck), so there's
        // some chance a stale lambda from a previous animation is still
        // connected. Without disconnect, that lambda would fire on
        // bounce-back completion and operate on stale state.
        disconnect(m_animConn);
        m_anim->setStartValue(m_slideOffset);
        m_anim->setEndValue(0.0);
        m_anim->setDuration(kBounceBaseMs);
        m_anim->start();
    }
    // V32.33+ Phase 1: if we're stuck mid-Ambiguous, emit a cancellation
    // signal so any subscribed widget can clear its consider-state visual.
    // Without this, widgets that adopt the affordance signals in Phase 2+
    // would be left stuck in their consider-state until something else
    // forces a redraw. Emit BEFORE resetGestureState clears m_affordanceFocus.
    if (m_intent == GestureIntent::Ambiguous) {
        emit gestureAffordanceCancelled(m_affordanceFocus.data());
    }
    // V32.34+: also clear any stuck overscroll — if the watchdog fired,
    // the user's touch was lost, and any overscroll they pulled out is
    // now permanent without intervention. Don't spring-back animate
    // it (the gesture is in an inconsistent state); just snap to anchor.
    if (hasOverscroll()) {
        clearOverscroll();
    }
    resetGestureState();
}

void SwipeNavigator::connectAnimFinished(std::function<void()> fn)
{
    disconnect(m_animConn);
    QPointer<SwipeNavigator> self(this);
    m_animConn = connect(m_anim, &QPropertyAnimation::finished, this,
                         [self, fn = std::move(fn)]() {
                             if (!self) return;
                             disconnect(self->m_animConn);
                             fn();
                         });
}

// ─── Page management ──────────────────────────────────────────────────────────

void SwipeNavigator::addPage(QWidget *page, const QString &name)
{
    Q_ASSERT(page);
    page->setParent(this);
    m_pages.append(page);
    m_pageNames.append(name);   // V32.34+: kept for UnifiedNavBar's page label
    layoutPages();
}

QString SwipeNavigator::pageName(int index) const
{
    return (index >= 0 && index < m_pageNames.size()) ? m_pageNames[index]
                                                       : QString();
}

void SwipeNavigator::goToPage(int index, bool animate, bool resetSubTab)
{
    if (index < 0 || index >= static_cast<int>(m_pages.size())) return;
    if (m_dragging) { m_dragging = false; m_swipeLocked = false; m_slideOffset = 0; }
    // V32.33+ Phase 1 audit: use fuzzy compare instead of exact equality.
    // After a previous animation, m_slideOffset may be ~1e-15 due to
    // floating-point — exact == 0.0 fails and we'd run a slide-by-zero
    // animation that emits a spurious pageChanged(currentIndex).
    if (index == m_currentIndex && qFuzzyIsNull(m_slideOffset)) return;

    const int pageW   = (m_pageWidth > 0) ? m_pageWidth : width();
    const qreal dest  = static_cast<qreal>(-(index - m_currentIndex) * pageW);
    const int destIdx = index;

    if (animate && isVisible()) {
        m_anim->stop();
        m_anim->setDuration(kCommitMaxMs);
        m_anim->setEasingCurve(QEasingCurve::OutCubic);
        m_anim->setStartValue(m_slideOffset);
        m_anim->setEndValue(dest);
        // V32.33+ Phase 1 audit: capture the prev index so the
        // animation-end callback can emit pageChanged only if the
        // index actually changes. goToPage may be called with
        // index == currentIndex when slideOffset is non-zero (to
        // snap back to origin); in that case no real page change
        // happens.
        const int prevIndex = m_currentIndex;
        connectAnimFinished([this, destIdx, prevIndex, resetSubTab]() {
            m_currentIndex = destIdx;
            m_slideOffset  = 0.0;
            m_pageWidth    = 0;
            layoutPages();
            if (destIdx != prevIndex) {
                // V32.34+ unified nav: land on the new page's first sub-tab,
                // but ONLY for user navigation (resetSubTab) — programmatic
                // jumps preserve the page's current/restored sub-tab.
                if (resetSubTab && unifiedNavEnabled())
                    if (SubTabbed *st = currentSubTabbed())
                        if (st->subTabCount() > 0 && st->currentSubTab() != 0)
                            st->setCurrentSubTab(0);
                notifyPageTransition(destIdx);
            }
        });
        m_anim->start();
    } else {
        m_anim->stop();
        const int prevIndex = m_currentIndex;
        m_currentIndex = index;
        m_slideOffset  = 0.0;
        m_pageWidth    = 0;
        layoutPages();
        if (index != prevIndex) {
            if (resetSubTab && unifiedNavEnabled())
                if (SubTabbed *st = currentSubTabbed())
                    if (st->subTabCount() > 0 && st->currentSubTab() != 0)
                        st->setCurrentSubTab(0);
            notifyPageTransition(index);
        }
    }
}

// V32.34+ DEBOUNCE LAYER. Single funnel for all page-transition emits. Emits
// the instant edges (pageChanged for back-compat + pageAnimating for the
// responsive UI layer) right away, and feeds the coalescing debouncer whose
// trailing edge fires pageSettled once swiping quiets — so the expensive
// network/stats/celebration work runs for the page the user actually landed on,
// not every page a fast swipe blew past. One repeating timer drives the
// trailing edge; the debouncer's redundant-settle suppression means swiping
// away and back to the same page does no consequential work twice.
void SwipeNavigator::notifyPageTransition(int index)
{
    // Instant layer — every transition, no debounce.
    emit pageChanged(index);
    emit pageAnimating(index);

    // Monotonic clock for the debouncer (started lazily on first use).
    if (!m_pageDebounceClock.isValid())
        m_pageDebounceClock.start();
    const quint64 now = static_cast<quint64>(m_pageDebounceClock.elapsed());

    // Feed the debouncer. The push() leading edge is already covered by
    // pageAnimating above, so we don't act on it here; we only need the
    // trailing settle, which arrives via the timer-driven tick().
    m_pageDebounce.push(index, now);

    // Lazily create the single driving timer. ~50ms cadence is plenty for a
    // 180ms quiet window and keeps idle wakeups negligible. It stops itself
    // once the debouncer goes idle, so it doesn't tick forever.
    if (!m_pageSettleTimer) {
        m_pageSettleTimer = new QTimer(this);
        m_pageSettleTimer->setInterval(50);
        connect(m_pageSettleTimer, &QTimer::timeout, this, [this]() {
            const quint64 t = static_cast<quint64>(m_pageDebounceClock.elapsed());
            const auto edges = m_pageDebounce.tick(t);
            for (const auto &e : edges) {
                if (e.kind == SnapClone::UI::DebounceEdgeKind::Settled)
                    emit pageSettled(e.value);
            }
            // Stop ticking once nothing is in flight — re-armed on next push.
            if (!m_pageDebounce.isActive())
                m_pageSettleTimer->stop();
        });
    }
    if (!m_pageSettleTimer->isActive())
        m_pageSettleTimer->start();
}

int      SwipeNavigator::currentIndex() const { return m_currentIndex; }
QWidget *SwipeNavigator::currentPage()  const {
    return (m_currentIndex < static_cast<int>(m_pages.size()))
        ? m_pages[m_currentIndex] : nullptr;
}
int SwipeNavigator::pageCount() const { return static_cast<int>(m_pages.size()); }

// ── V32.34+ two-level navigation: SubTabbed bridge ──────────────────────────
SubTabbed *SwipeNavigator::currentSubTabbed() const
{
    // dynamic_cast across the page's QWidget→SubTabbed multiple inheritance.
    // Pages that don't implement SubTabbed (Camera, Wayfinder) return null.
    QWidget *p = currentPage();
    return p ? dynamic_cast<SubTabbed *>(p) : nullptr;
}

int SwipeNavigator::currentSubTabCount() const
{
    SubTabbed *st = currentSubTabbed();
    return st ? st->subTabCount() : 0;
}

int SwipeNavigator::currentSubTabIndex() const
{
    SubTabbed *st = currentSubTabbed();
    return st ? st->currentSubTab() : 0;
}

bool SwipeNavigator::advanceSubTab(int delta)
{
    // Non-wrapping: move within [0, count). Returns false at an edge so the
    // caller knows to apply the page-break detent instead.
    SubTabbed *st = currentSubTabbed();
    if (!st) return false;
    const int count = st->subTabCount();
    if (count <= 0) return false;
    const int cur = st->currentSubTab();
    const int next = cur + delta;
    if (next < 0 || next >= count) return false;   // at boundary, didn't move
    st->setCurrentSubTab(next);
    emit subTabChanged(next);
    return true;
}

int SwipeNavigator::cycleSubTab(int delta)
{
    // Wrapping: returns 0 if it moved within range, -1 if it wrapped off the
    // start, +1 if it wrapped off the end — so the caller can fire the
    // page-break detent exactly on a wrap.
    SubTabbed *st = currentSubTabbed();
    if (!st) return 0;
    const int count = st->subTabCount();
    if (count <= 0) return 0;
    const int cur = st->currentSubTab();
    int next = cur + delta;
    int wrap = 0;
    if (next >= count) { next = 0;          wrap = +1; }
    else if (next < 0) { next = count - 1;  wrap = -1; }
    st->setCurrentSubTab(next);
    emit subTabChanged(next);
    if (wrap != 0)
        qCInfo(lcSwipeNav, "[HANDOFF] sub-tab scrub WRAPPED %s → breaks to page",
               wrap > 0 ? "off end" : "off start");
    return wrap;
}

// ── V32.34+ two-level unified navigation (SC_UNIFIED_NAV) ───────────────────
bool SwipeNavigator::unifiedNavEnabled()
{
    // Cached once: env SC_UNIFIED_NAV=1, else QSettings ui/unified_nav.
    static const bool on = []() {
        bool v = false;
        const char *src = "default(off)";
        if (qEnvironmentVariableIntValue("SC_UNIFIED_NAV") == 1) {
            v = true; src = "env SC_UNIFIED_NAV=1";
        } else {
            QSettings s;
            v = s.value(QStringLiteral("ui/unified_nav"), false).toBool();
            if (v) src = "QSettings ui/unified_nav";
        }
        // V32.34+: log the resolved state ONCE at info level so a device run's
        // log immediately shows whether the unified nav (and thus the sub-tab
        // detent + scroll/gesture passthrough) is even active. Without this it's
        // impossible to tell from the log whether "handoff not working" means
        // "flag was off" vs "flag on but handoff misbehaving".
        qCInfo(lcSwipeNav, "[UNIFIED-NAV] %s (source: %s)",
               v ? "ENABLED" : "disabled", src);
        return v;
    }();
    return on;
}

void SwipeNavigator::resetDetentState()
{
    m_vDetentArmed     = false;
    m_vDetentArmedAtMs = 0;
    m_vDetentFired     = false;
    m_hSubScrubActive  = false;
    m_hBrokeToPage     = false;
    m_hScrubAccumPx    = 0.0;
    if (!qFuzzyIsNull(m_barrierOpacity)) {
        m_barrierOpacity = 0.0;
        m_barrierEdge    = 0;
        update();   // clear any lingering barrier overlay
    }
}

void SwipeNavigator::evaluateVerticalDetent(qreal rawY)
{
    // Called from applyOverscroll only when unifiedNavEnabled(). rawY is the
    // accumulated raw vertical overscroll (signed: + = pulled past bottom,
    // - = past top, matching m_overscrollRawY's convention). The detent ARMS
    // the moment overscroll starts (scroll has hit its limit by definition,
    // since applyOverscroll only runs on clamped motion), stays armed for
    // kDetentArmWindowMs, and fires ONE sub-tab advance per overscroll streak
    // when |rawY| crosses kVDetentRawThreshold.
    if (!currentSubTabbed()) return;   // page has no sub-tabs → no detent

    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (!m_vDetentArmed) {
        m_vDetentArmed     = true;
        m_vDetentArmedAtMs = now;
        m_vDetentFired     = false;
    } else if (now - m_vDetentArmedAtMs > kDetentArmWindowMs) {
        // Window lapsed without crossing — disarm, but re-arm immediately so
        // a renewed sustained pull starts a fresh window (keeps flick+pull
        // feeling unified rather than dead after 1s). V32.34+: also zero the
        // raw accumulator + barrier so a lapsed streak doesn't carry stale
        // progress into the next pull — important on the WHEEL path, where
        // m_overscrollRawY isn't reset by a spring-back (there's no viewport
        // target / fling tick to do it). The caller re-accumulates from 0.
        m_vDetentArmed     = true;
        m_vDetentArmedAtMs = now;
        m_vDetentFired     = false;
        m_overscrollRawY   = 0.0;
        rawY               = 0.0;
        if (!qFuzzyIsNull(m_barrierOpacity)) { m_barrierOpacity = 0.0; update(); }
    }

    // V32.39+: progress + grow-text come from the tested SubPageDetent curve.
    // progress is linear past the buffer (an honest "distance to commit"); the
    // sub-tab label scale swells with it. The buffer means the barrier + text
    // don't begin until the user has pushed past the detent on the current page.
    const qreal progress  = m_subPageDetent.progress(qAbs(rawY));
    const qreal textScale = m_subPageDetent.textScale(qAbs(rawY));
    const int edge = (rawY > 0) ? 0 : 1;   // 0=bottom (past bottom), 1=top
    if (!qFuzzyCompare(m_barrierOpacity, progress) || m_barrierEdge != edge
        || !qFuzzyCompare(m_barrierTextScale, textScale)) {
        m_barrierOpacity   = progress;
        m_barrierTextScale = textScale;
        m_barrierEdge      = edge;
        update();
    }

    if (!m_vDetentFired && m_vDetentArmed
        && m_subPageDetent.shouldCommit(qAbs(rawY))) {
        // V32.34+: don't take over if a recent takeover still holds the lock —
        // this is what stops ONE sustained scroll from ripping through several
        // sub-tabs (the device log showed rawY hitting 4× threshold in a single
        // burst, firing ~1/sec). Mark fired anyway + reset so this streak's
        // accumulation is consumed rather than carried.
        if (scrollTakeoverLocked()) {
            m_vDetentFired   = true;
            m_overscrollRawY = 0.0;
            rawY             = 0.0;
            m_barrierOpacity = 0.0;
            update();
            return;
        }
        // V32.34+ direction: which way an overscroll streak advances the
        // sub-tab depends on the user's scroll convention (natural vs
        // traditional), which Qt doesn't expose to us. Rather than hard-code
        // (and risk being backwards for half of setups), read an invert flag
        // from QSettings (ui/scroll_detent_invert, default false). Base mapping:
        // rawY < 0 (overscroll past TOP) → PREV, rawY > 0 (past BOTTOM) → NEXT,
        // V32.34+ direction: sign is already normalized per-source upstream in
        // processWheelEvent (mouse vs trackpad invert applied there), so the
        // mapping here is the plain convention: rawY > 0 (overscroll past the
        // BOTTOM) → NEXT, rawY < 0 (past the TOP) → PREV. Do NOT re-apply an
        // invert flag here or it would double-flip.
        const int dir = (rawY > 0) ? +1 : -1;
        if (advanceSubTab(dir)) {
            m_vDetentFired = true;
            engageScrollTakeoverLock();   // block further takeover briefly
            // V32.34+: visible in release logs so a device run confirms the
            // scroll→sub-tab handoff actually fired (rawY crossed threshold).
            qCInfo(lcSwipeNav, "[HANDOFF] scroll→sub-tab: advanced %s "
                               "(rawY=%.0f >= %.0f, lock=%lldms)",
                   dir > 0 ? "NEXT" : "PREV", rawY, m_subPageDetent.config().commitPx,
                   static_cast<long long>(scrollTakeoverLockMs()));
            // Reset the overscroll so the new sub-tab's content starts at a
            // clean position rather than mid-bounce, and clear the barrier.
            m_overscrollRawY = 0.0;
            m_overscrollY    = 0.0;
            m_barrierOpacity = 0.0;
            applyOverscrollToViewport();
            update();
        } else {
            // V32.34+ PAGE SPILL: advanceSubTab returned false → we're already
            // at the first/last sub-tab and can't move further in `dir`. THIS is
            // what the user meant by "push past a sub page to the next page":
            // continuing to scroll past the last sub-tab advances the PAGE (and
            // past the first sub-tab, scrolling back goes to the previous page).
            // Previously nothing happened here, so pages could only be reached
            // by an explicit swipe — exactly the complaint. Commit the page
            // change in the scroll direction; goToPage handles bounds.
            const int target = m_currentIndex + dir;
            if (target >= 0 && target < pageCount()) {
                m_vDetentFired = true;
                engageScrollTakeoverLock();
                qCInfo(lcSwipeNav, "[HANDOFF] scroll→page: at sub-tab edge, "
                                   "advancing to page %d (dir=%d, lock=%lldms)",
                       target, dir,
                       static_cast<long long>(scrollTakeoverLockMs()));
                m_overscrollRawY = 0.0;
                m_overscrollY    = 0.0;
                m_barrierOpacity = 0.0;
                applyOverscrollToViewport();
                update();
                goToPage(target, /*animate=*/true, /*resetSubTab=*/false);
            }
            // If target is out of range (first page scrolling back / last page
            // scrolling forward) we simply don't move — nothing to spill into.
        }
    }
}

qint64 SwipeNavigator::scrollTakeoverLockMs() const
{
    // Lock duration after a scroll-driven takeover (detent or page change).
    // Per the design: 800ms base, plus a floor of 600ms, scaled gently by how
    // much there is to navigate (more pages/sub-tabs → slightly longer lock so
    // a fast scroll doesn't rip through everything) and by the RECENT takeover
    // RATE (if the user just fired several in quick succession, lengthen the
    // lock to damp runaway). Formula:
    //     800 + max(600, round(0.01*pages + 0.05*subPagesPerMinute))
    // where subPagesPerMinute is the recent takeover rate (extrapolated from
    // the last few). Clamped to a sane ceiling so it never feels stuck.
    const int pages = pageCount();
    int subTabs = 0;
    if (SubTabbed *st = const_cast<SwipeNavigator *>(this)->currentSubTabbed())
        subTabs = st->subTabCount();

    // Recent takeover rate: takeovers in the last 60s, projected to per-minute.
    // m_recentTakeovers holds timestamps; count those within the window.
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    int recent = 0;
    for (qint64 t : m_recentTakeovers)
        if (now - t <= 60000) ++recent;
    // Per-minute rate of SUB-PAGE takeovers (recent is already a ~minute count).
    const qreal subPagesPerMinute = static_cast<qreal>(recent);

    const qreal scaled = 0.01 * pages + 0.05 * subPagesPerMinute * subTabs;
    qint64 ms = 800 + static_cast<qint64>(qMax(600.0, scaled));
    // Ceiling so a heavy session can't push the lock to something that feels
    // broken; floor is implicit (800+600=1400).
    return qBound<qint64>(Q_INT64_C(1400), ms, Q_INT64_C(3000));
}

bool SwipeNavigator::scrollTakeoverLocked() const
{
    return QDateTime::currentMSecsSinceEpoch() < m_scrollLockUntilMs;
}

void SwipeNavigator::engageScrollTakeoverLock()
{
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    m_scrollLockUntilMs = now + scrollTakeoverLockMs();
    // Record this takeover for the rate calc; keep only the last ~60s worth.
    m_recentTakeovers.append(now);
    while (!m_recentTakeovers.isEmpty() && now - m_recentTakeovers.first() > 60000)
        m_recentTakeovers.removeFirst();
}

void SwipeNavigator::setSlideOffset(qreal offset)
{
    if (qFuzzyCompare(m_slideOffset, offset)) return;
    m_slideOffset = offset;
    // Slide offset drives where each page sits horizontally; re-lay out so the
    // visible translation reflects the new offset. (Restored V32.34+ — this
    // body was accidentally orphaned while inserting the unified-nav detent
    // functions; behavior matches the pre-edit version: set + relayout.)
    layoutPages();
}

void SwipeNavigator::layoutPages()
{
    const int w = width(), h = height();
    if (w == 0 || h == 0) return;
    for (int i = 0; i < static_cast<int>(m_pages.size()); ++i) {
        const int x = static_cast<int>(qRound(
            static_cast<qreal>(i - m_currentIndex) * w + m_slideOffset));
        const bool vis = (x + w > 0) && (x < w);
        // V32.33+ Phase 1 audit: only call setVisible when state actually
        // changes. Qt's setVisible(false) clears focus on the widget and
        // its descendants; setVisible(true) does NOT restore it. During
        // rapid bounce-back animations, layoutPages may visit a page
        // dozens of times — each setVisible call is a no-op if the
        // state matches, but Qt still does internal bookkeeping that
        // can flush focus state. Caching the prior-visible state avoids
        // both unnecessary work AND the focus-loss edge case where a
        // focused QLineEdit on a partial-swipe-bounce-back loses focus.
        if (m_pages[i]->isVisibleTo(this) != vis)
            m_pages[i]->setVisible(vis);
        if (vis) m_pages[i]->setGeometry(x, 0, w, h);
    }
}

// ─── Physics helpers ──────────────────────────────────────────────────────────

qreal SwipeNavigator::rubberBand(qreal drag, int pageW) const
{
    // Sub-linear resistance: sqrt(|drag| * pageW * k)
    // - Feels like stretching elastic, not pushing through mud
    // - Naturally stronger resistance at larger pulls (correct physical feel)
    // - Independent of screen size (scales with pageW)
    const qreal k = kRubberBandK;
    const qreal magnitude = qSqrt(qAbs(drag) * static_cast<qreal>(pageW) * k);
    return drag >= 0.0 ? magnitude : -magnitude;
}

void SwipeNavigator::startCommitAnimation(bool toNext, qreal releaseVelocityPxMs)
{
    const int targetIndex = m_currentIndex + (toNext ? 1 : -1);
    if (targetIndex < 0 || targetIndex >= static_cast<int>(m_pages.size())) {
        // Can't commit past the edge — bounce back instead
        startBounceAnimation(m_slideOffset, releaseVelocityPxMs);
        return;
    }

    const int pageW = (m_pageWidth > 0) ? m_pageWidth : width();
    const qreal endOffset = toNext ? -static_cast<qreal>(pageW)
                                   :  static_cast<qreal>(pageW);
    const int destIndex = targetIndex;

    // Duration: velocity-aware. Fast flick → short duration. Slow drag → longer.
    // If velocity is very low (slow release at threshold), use distance-based fallback.
    int durationMs;
    const qreal absVel = qAbs(releaseVelocityPxMs);
    if (absVel > 0.05) {
        // Remaining distance to animate
        const qreal remaining = qAbs(endOffset - m_slideOffset);
        // Time = distance / velocity, but decelerate so it eases out
        // Use half the remaining / velocity as base (OutCubic handles the decel curve)
        const qreal rawMs = remaining / absVel * 0.7;
        durationMs = qBound(kCommitMinMs, static_cast<int>(rawMs), kCommitMaxMs);
    } else {
        // Slow release: proportional to how far we still have to travel
        const qreal fraction = qAbs(endOffset - m_slideOffset) / static_cast<qreal>(pageW);
        durationMs = qBound(kCommitMinMs,
                            static_cast<int>(fraction * kCommitMaxMs),
                            kCommitMaxMs);
    }

    m_anim->stop();
    m_anim->setDuration(durationMs);
    m_anim->setEasingCurve(QEasingCurve::OutCubic);
    m_anim->setStartValue(m_slideOffset);
    m_anim->setEndValue(endOffset);

    // V32.30: subtle crossfade on the OUTGOING page — fades opacity
    // 1.0 → 0.65 over 90ms (capped at half the slide duration). The
    // slide still does the main work; the fade just softens the
    // departure so the page change feels less mechanical. Capped at
    // 0.65 (not 0) so the outgoing page stays visually grounded
    // through the slide — without that floor, fast swipes look like
    // the page "vanished" mid-flight.
    //
    // V32.34+: the page-level opacity fade on the OUTGOING page during a
    // slide was removed. Applying a QGraphicsOpacityEffect to a widget that
    // is SIMULTANEOUSLY being geometry-animated (slid) and repainted makes
    // Qt run the effect's pixmap-grab render path while the slide paint is
    // still active on the same device — producing the storm of
    //   QPainter::begin: A paint device can only be painted by one painter
    //   QWidgetEffectSourcePrivate::pixmap: Painter not active
    //   QPainter::restore: Unbalanced save/restore
    // warnings seen on every swipe in the field logs (and worst on the
    // Contacts page, whose cards ALSO carry per-card opacity effects — the
    // online-badge pulse + netstat fade — so the collisions compounded).
    // The slide motion itself is the transition; the subtle 1.0→0.65 fade
    // was minor polish not worth the painter contention. The geometry slide
    // (handled below) is untouched and remains the visible page-change cue.
    //
    // If a cross-fade is wanted back later, the correct approach is to
    // render each page to a QPixmap ONCE up front and cross-fade the two
    // static pixmaps (no live widget under an effect), not to attach an
    // effect to a moving widget.

    // Clear any opacity effect a PRIOR build/codepath may have left attached
    // to the outgoing page, so a stale effect can't keep running through the
    // paint pipeline during this slide.
    if (m_currentIndex >= 0
     && m_currentIndex < static_cast<int>(m_pages.size())) {
        auto *outgoing = m_pages[m_currentIndex];
        if (outgoing && outgoing->graphicsEffect()) {
            outgoing->setGraphicsEffect(nullptr);
        }
    }

    connectAnimFinished([this, destIndex]() {
        m_currentIndex = destIndex;
        m_slideOffset  = 0.0;
        m_pageWidth    = 0;
        layoutPages();
        // V32.34+ unified nav: when a horizontal break-through lands on a new
        // page, the settled design says land on that page's FIRST sub-tab.
        if (unifiedNavEnabled()) {
            if (SubTabbed *st = currentSubTabbed())
                if (st->subTabCount() > 0 && st->currentSubTab() != 0)
                    st->setCurrentSubTab(0);
            // V32.34+: after committing a page change, engage the scroll-
            // takeover lock so a still-moving scroll/fling can't immediately
            // rip into a sub-tab detent or another page on the new page (user
            // spec: "block menu AND slider scroll takeover if already
            // scrolling"). A deliberate NEW swipe is unaffected — the lock only
            // gates the scroll-driven detent + wheel page-nav paths.
            engageScrollTakeoverLock();
            // V32.34+: visible in release logs — confirms a swipe committed a
            // page change ("menu swipe") and reset to the first sub-tab.
            qCInfo(lcSwipeNav, "[HANDOFF] swipe→page: committed to page %d",
                   destIndex);
        }
        notifyPageTransition(destIndex);
    });
    m_anim->start();
}

void SwipeNavigator::startBounceAnimation(qreal currentOffset, qreal releaseVelocityPxMs)
{
    // Duration proportional to pull depth — deeper bounces take longer to spring back.
    // Base 90ms + 0.35ms per pixel. This makes a light tap-and-release nearly instant
    // while a hard push to the edge takes a noticeable moment to spring back.
    const int depthMs = static_cast<int>(qAbs(currentOffset) * kBounceDepthMs);
    const int durationMs = qBound(60, kBounceBaseMs + depthMs, 280);

    m_anim->stop();
    m_anim->setDuration(durationMs);
    // OutExpo: fast start, asymptotes to stop. Feels like a real spring that snaps
    // back quickly rather than decelerating gradually (OutCubic would feel sluggish here).
    m_anim->setEasingCurve(QEasingCurve::OutExpo);
    m_anim->setStartValue(currentOffset);
    m_anim->setEndValue(0.0);
    connectAnimFinished([this]() {
        m_anim->setEasingCurve(QEasingCurve::OutCubic);
        m_slideOffset = 0.0;
        m_pageWidth   = 0;
        layoutPages();
    });
    m_anim->start();

    // Suppress unused parameter warning — velocity could drive initial spring speed
    // in a full Hooke's law sim; for now duration is the proxy.
    Q_UNUSED(releaseVelocityPxMs);
}

// ─── Velocity tracking (exponentially weighted, 120ms window) ─────────────────

void SwipeNavigator::velocityReset()
{
    m_velHead  = 0;
    m_velCount = 0;
}

void SwipeNavigator::velocityRecord(qreal x)
{
    const qint64 now = m_touchTimer.elapsed();
    m_velSamples[m_velHead] = { now, x };
    m_velHead = (m_velHead + 1) % kVelBufSize;
    if (m_velCount < kVelBufSize) ++m_velCount;
}

qreal SwipeNavigator::velocityCompute() const
{
    if (m_velCount < 2) return 0.0;

    const qint64 now    = m_touchTimer.elapsed();
    const qint64 cutoff = now - kVelWindowMs;

    // Exponentially weighted: recent samples count more than old ones.
    // Weight = exp(-age_ms / halfLife). halfLife = 40ms so samples at 40ms
    // age contribute half as much as samples at 0ms.
    constexpr qreal kHalfLifeMs = 40.0;

    qreal sumWx = 0.0, sumWt = 0.0, sumW = 0.0;
    int used = 0;

    for (int i = 0; i < m_velCount; ++i) {
        const int idx = (m_velHead - 1 - i + kVelBufSize) % kVelBufSize;
        const VelSample &s = m_velSamples[idx];
        if (s.timeMs < cutoff) break;
        const qreal age = static_cast<qreal>(now - s.timeMs);
        const qreal w   = qExp(-age / kHalfLifeMs * qLn(2.0));
        sumWx += w * s.x;
        sumWt += w * static_cast<qreal>(s.timeMs);
        sumW  += w;
        ++used;
    }

    if (used < 2 || sumW < 0.001) return 0.0;

    const int newestIdx = (m_velHead - 1 + kVelBufSize) % kVelBufSize;
    const int oldestIdx = (m_velHead - used + kVelBufSize) % kVelBufSize;
    const VelSample &newest = m_velSamples[newestIdx];
    const VelSample &oldest = m_velSamples[oldestIdx];

    const qreal dt = static_cast<qreal>(newest.timeMs - oldest.timeMs);
    if (dt < 2.0) return 0.0;

    // V32.33+ Phase 1 audit: the previous version computed sumWx, sumWt,
    // sumW, meanX, meanT but only used endpointVel — the weighted slope
    // computation was dead code, with comments claiming a blend that
    // never happened. Implement the actual blend: weighted regression
    // slope (less sensitive to outlier endpoints) blended with endpoint
    // velocity (captures lift-off speed for flicks).
    //
    // The weighted slope is computed using weighted least-squares
    // regression of x vs t. With weights w_i, the slope is:
    //   slope = sum(w_i * (t_i - meanT) * (x_i - meanX)) / sum(w_i * (t_i - meanT)^2)
    // We have meanT and meanX from the first pass; need a second pass
    // to compute the slope numerator/denominator.
    const qreal meanX = sumWx / sumW;
    const qreal meanT = sumWt / sumW;
    qreal slopeNum = 0.0, slopeDen = 0.0;
    for (int i = 0; i < used; ++i) {
        const int idx = (m_velHead - 1 - i + kVelBufSize) % kVelBufSize;
        const VelSample &s = m_velSamples[idx];
        const qreal age = static_cast<qreal>(now - s.timeMs);
        const qreal w   = qExp(-age / kHalfLifeMs * qLn(2.0));
        const qreal dtFromMean = static_cast<qreal>(s.timeMs) - meanT;
        const qreal dxFromMean = s.x - meanX;
        slopeNum += w * dtFromMean * dxFromMean;
        slopeDen += w * dtFromMean * dtFromMean;
    }
    const qreal weightedSlope = (slopeDen > 0.001) ? (slopeNum / slopeDen) : 0.0;

    const qreal endpointVel = (newest.x - oldest.x) / dt;

    // Blend: 60% weighted-slope (smooth, robust) + 40% endpoint (responsive).
    // Tuned by the rationale in commit: smooth response is more important
    // than maximum responsiveness for the commit-vs-bounce decision —
    // a single noisy sample shouldn't flip the decision.
    return 0.6 * weightedSlope + 0.4 * endpointVel;
}

// ─── Tap forwarding ───────────────────────────────────────────────────────────

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnull-dereference"
void SwipeNavigator::forwardTap(const QPointF &pos)
{
    const QPoint globalPos = mapToGlobal(pos.toPoint());
    QPointer<QWidget> target = QApplication::widgetAt(globalPos);
    if (!target || !target->isVisible() || !target->isEnabled()) return;

    if (qobject_cast<QLineEdit *>(target.data()))
        target->setFocus(Qt::OtherFocusReason);

    while (target && target != this && target->parentWidget() != this) {
        const bool isInteractive =
            target->focusPolicy() != Qt::NoFocus
            || target->hasMouseTracking()
            || target->property("swipenav-tappable").toBool()
            || qobject_cast<QPushButton *>(target.data())
            || qobject_cast<QAbstractButton *>(target.data())
            || qobject_cast<QComboBox *>(target.data())
            || qobject_cast<QAbstractScrollArea *>(target.data())
            || qobject_cast<QSlider *>(target.data())
            || qobject_cast<QSpinBox *>(target.data())
            || qobject_cast<QLineEdit *>(target.data());
        if (isInteractive) break;
        if (!target->parentWidget()) break;
        target = target->parentWidget();
    }
    if (!target || target == this) return;

    const QPoint localPos = target->mapFromGlobal(globalPos);
    QCoreApplication::postEvent(target,
        new QMouseEvent(QEvent::MouseButtonPress, localPos, localPos, globalPos,
                        Qt::LeftButton, Qt::LeftButton, Qt::NoModifier,
                        Qt::MouseEventSynthesizedByQt));
    if (target) {
        QCoreApplication::postEvent(target,
            new QMouseEvent(QEvent::MouseButtonRelease, localPos, localPos, globalPos,
                            Qt::LeftButton, Qt::NoButton, Qt::NoModifier,
                            Qt::MouseEventSynthesizedByQt));
    }
}
#pragma GCC diagnostic pop

// ─── Gesture state machine ────────────────────────────────────────────────────

void SwipeNavigator::gestureBegin(const QPointF &pos)
{
    // V32.34+ unified nav: each new stroke starts with fresh detent state
    // (no carried arm window, barrier hidden). Inert unless the flag is on.
    if (unifiedNavEnabled()) resetDetentState();

    // If animation is running, adopt its current position as start so
    // catching mid-animation doesn't cause a jump.
    if (m_anim->state() == QAbstractAnimation::Running) {
        m_anim->stop();
        disconnect(m_animConn);
        // Snap to nearest page if well past halfway, else stay on current.
        const int pageW = (m_pageWidth > 0) ? m_pageWidth : width();
        const int prevIndex = m_currentIndex;
        if (pageW > 0) {
            if (m_slideOffset < -(pageW / 2) &&
                m_currentIndex + 1 < static_cast<int>(m_pages.size()))
                ++m_currentIndex;
            else if (m_slideOffset > (pageW / 2) && m_currentIndex > 0)
                --m_currentIndex;
        }
        m_slideOffset = 0.0;
        layoutPages();
        // V32.33+ Phase 1 audit: only emit pageChanged if the index
        // actually changed. Otherwise (e.g. interrupting a bounce-back
        // animation), the spurious emission can confuse subscribers
        // that count page transitions (analytics, lifecycle hooks).
        if (m_currentIndex != prevIndex)
            notifyPageTransition(m_currentIndex);
    }

    m_touchStart      = pos;
    m_dragDistance    = 0.0;
    m_swipeLocked     = false;
    m_dragging        = false;
    m_scrollCommitted = false;
    // Capture page width NOW — immune to software keyboard opening mid-swipe.
    m_pageWidth       = width();
    m_touchTimer.restart();
    velocityReset();
    velocityRecord(pos.x());
}

bool SwipeNavigator::gestureUpdate(const QPointF &pos)
{
    m_dragDistance = pos.x() - m_touchStart.x();
    velocityRecord(pos.x());

    if (!m_swipeLocked) {
        const qreal hx = qAbs(m_dragDistance);
        const qreal vy = qAbs(pos.y() - m_touchStart.y());

        // Reject clearly vertical gestures using the direction ratio.
        // kDirectionRatio = 0.65 is more lenient than the old 0.75.
        if (hx < vy * kDirectionRatio) return false;

        // Reject QLineEdit focus steals before direction lock.
        if (!m_passingToScrollArea && !m_wheelHorizontalLocked) {
            const QWidget *under = QApplication::widgetAt(mapToGlobal(pos.toPoint()));
            while (under && under != this) {
                if (qobject_cast<const QLineEdit *>(under)) return false;
                under = under->parentWidget();
            }
        }

        // V32.33+ Phase 1: use kPageSwipeThresholdPx (6px) instead of
        // the legacy kLockThresholdPx (14px) so page-swipe engages
        // promptly. Combined with the lowered threshold in
        // resolveAmbiguous, this eliminates the dead zone where the
        // ambiguous-resolution committed to PageSwipe at 6px but
        // the page itself didn't visibly move until 14px.
        if (hx < static_cast<qreal>(kPageSwipeThresholdPx)) return false;

        m_swipeLocked = true;
        m_dragging    = true;
        m_anim->stop();
    }

    const int pageW  = (m_pageWidth > 0) ? m_pageWidth : width();
    const bool atStart = (m_currentIndex == 0 && m_dragDistance > 0.0);
    const bool atEnd   = (m_currentIndex == static_cast<int>(m_pages.size()) - 1
                          && m_dragDistance < 0.0);

    // V32.34+ unified nav: horizontal friction toward the sub-tab→page break.
    // When the current page has sub-tabs, a horizontal drag SCRUBS sub-tabs
    // (committed at gestureEnd); as the drag approaches the break-through
    // distance the motion gets sticky (rubber-band resistance builds) and the
    // generic barrier fades in, so the boundary between "still scrubbing
    // sub-tabs" and "about to flip the whole page" is FELT, not just timed.
    // Inert unless the flag is on and the page is SubTabbed.
    if (unifiedNavEnabled() && currentSubTabbed() && currentSubTabCount() > 1
        && !atStart && !atEnd) {
        const qreal commitPx = static_cast<qreal>(pageW) * kCommitFraction;
        const qreal breakPx  = commitPx + kHDetentBreakPx;
        const qreal absDrag  = qAbs(m_dragDistance);
        // Barrier fade: ramps from the sub-tab commit point up to the break
        // point (the friction zone). Below commitPx = free scrub, no barrier.
        qreal barrier = 0.0;
        if (absDrag > commitPx)
            barrier = qBound(0.0, (absDrag - commitPx) / kHDetentBreakPx, 1.0);
        const int edge = (m_dragDistance < 0.0) ? 2 : 3;  // 2=right (drag left), 3=left
        if (!qFuzzyCompare(m_barrierOpacity, barrier) || m_barrierEdge != edge) {
            m_barrierOpacity = barrier;
            m_barrierEdge    = edge;
            update();
        }
        // V32.34+: in-flight preview for the unified bar. Progress is toward
        // the sub-tab COMMIT point (commitPx), clamped 0..1; direction is +1
        // when dragging LEFT (toward the NEXT sub-tab), -1 when dragging right.
        // The bar renders this as a cheap paint indicator (no setStyleSheet).
        const int   dragDir  = (m_dragDistance < 0.0) ? +1 : -1;
        const qreal subProg  = qBound(0.0, absDrag / commitPx, 1.0);
        emit subTabDragProgress(dragDir, subProg);
        // Resistance: 1:1 until the commit point, then sub-linear sqrt growth
        // through the friction zone so the page resists "breaking through"
        // until the user clearly pushes past breakPx. This is the tactile
        // detent the wheel/trackpad and finger share.
        if (absDrag > commitPx) {
            const qreal sign      = (m_dragDistance < 0.0) ? -1.0 : 1.0;
            const qreal overshoot = absDrag - commitPx;
            const qreal resisted  = qSqrt(overshoot * kHDetentBreakPx);
            m_slideOffset = sign * (commitPx + resisted);
        } else {
            m_slideOffset = m_dragDistance;
        }
        layoutPages();
        return true;
    }

    if (atStart || atEnd) {
        // Rubber-band: sub-linear sqrt resistance, scales with page width.
        // This replaces the flat 0.3× multiplier which felt the same at all speeds.
        m_slideOffset = rubberBand(m_dragDistance, pageW);
    } else {
        // Interior drag: 1:1 with finger. No clamping to pageW — the gesture
        // end logic handles over-dragging. Clamping to pageW mid-drag caused
        // the "stuck at edge" feel when you dragged quickly past the boundary.
        m_slideOffset = qBound(
            -static_cast<qreal>(pageW) * 1.1,
            m_dragDistance,
            static_cast<qreal>(pageW) * 1.1);
    }

    layoutPages();
    return true;
}

bool SwipeNavigator::gestureEnd(const QPointF &pos)
{
    velocityRecord(pos.x());

    // V32.34+: drop any in-flight sub-tab preview on the bar — the gesture is
    // resolving now (commit, cycle, or cancel), so the committed-state path
    // (subTabChanged) or the bounce takes over. Harmless when no preview was
    // showing (the bar ignores progress 0).
    if (unifiedNavEnabled())
        emit subTabDragProgress(0, 0.0);

    const bool wasLocked = m_swipeLocked;
    m_swipeLocked = false;
    m_dragging    = false;

    if (!wasLocked) {
        forwardTap(pos);
        return false;
    }

    const qreal velocity = velocityCompute(); // px/ms, signed
    const qreal absVel   = qAbs(velocity);
    const int   pageW    = (m_pageWidth > 0) ? m_pageWidth : width();

    // Flick: velocity above threshold AND at least kFlickMinPx of movement.
    // V32.33+ Phase 1 audit: also require velocity and drag direction to
    // agree. Without this, a user who drags one way and then flicks the
    // other way at release would commit in the FLICK direction —
    // slamming the page in the opposite direction from what they saw
    // during the drag. That's confusing UX. Require agreement.
    const bool dirsAgree = (velocity < 0.0) == (m_dragDistance < 0.0);
    const bool isFlick = absVel > kFlickThresholdPxMs
                      && qAbs(m_dragDistance) >= static_cast<qreal>(kFlickMinPx)
                      && dirsAgree;

    // Distance commit: dragged past fraction of page width.
    // This is the key fix: threshold scales with page width, not absolute pixels.
    const bool isDrag = qAbs(m_dragDistance) > static_cast<qreal>(pageW) * kCommitFraction;

    if (isFlick || isDrag) {
        // Direction: velocity-primary for flicks, drag-primary for slow drags.
        const bool toNext = isFlick ? (velocity < 0.0) : (m_dragDistance < 0.0);

        // V32.34+ unified nav: two-level horizontal. If the flag is on and the
        // current page exposes sub-tabs, a normal commit-worthy horizontal
        // swipe CYCLES the sub-tab. Only when the drag travels far enough past
        // the page-commit point (the friction detent, kHDetentBreakPx of extra
        // pull) does it BREAK THROUGH to a page change. cycleSubTab wraps and
        // tells us when it crossed the cycle boundary; the break-through is a
        // separate, larger threshold so reaching the boundary alone doesn't
        // page — the user must push past the detent. Landing on a new page
        // uses the page's first sub-tab (handled by the page-change path /
        // SubTabbed default).
        if (unifiedNavEnabled() && currentSubTabbed()
            && currentSubTabCount() > 1 && !m_hBrokeToPage) {
            const qreal breakPx = static_cast<qreal>(pageW) * kCommitFraction
                                + kHDetentBreakPx;
            const bool breakToPage = qAbs(m_dragDistance) >= breakPx;
            if (!breakToPage) {
                // Scrub one sub-tab in the swipe direction. toNext (swiped
                // left / negative) = NEXT sub-tab.
                const int dir = toNext ? +1 : -1;
                // V32.34+ FIELD FIX: use the NON-WRAPPING advanceSubTab, not
                // cycleSubTab. The wrapping version teleported the sub-tab row
                // to the opposite end when you scrubbed off the first/last tab
                // (the device log's repeated "sub-tab scrub WRAPPED off start"
                // spam) — jarring and not what "go back one" should do. Now, at
                // the edge we SPILL to a page change in that direction (bounds-
                // checked), exactly like the vertical detent path does. Reaching
                // the edge mid-scrub only pages if there's a page to go to;
                // otherwise it simply resists and snaps back.
                if (advanceSubTab(dir)) {
                    // Moved within the sub-tab row — no page change. The brief
                    // snap-back is the material feedback that the detent
                    // released and content settled.
                    if (!qFuzzyIsNull(m_barrierOpacity)) { m_barrierOpacity = 0.0; update(); }
                    startBounceAnimation(m_slideOffset, velocity);
                    return true;
                }
                // At the first/last sub-tab and can't move further in `dir`.
                // Spill to the adjacent page if one exists; else resist+bounce.
                const int target = m_currentIndex + dir;
                if (target >= 0 && target < pageCount()) {
                    qCInfo(lcSwipeNav, "[HANDOFF] sub-tab scrub at edge → page %d "
                                       "(dir=%d)", target, dir);
                    m_hBrokeToPage = true;   // fall through to the page commit
                } else {
                    // No page to spill to — stay put, snap back.
                    if (!qFuzzyIsNull(m_barrierOpacity)) { m_barrierOpacity = 0.0; update(); }
                    startBounceAnimation(m_slideOffset, velocity);
                    return true;
                }
            } else {
                // Past the explicit break threshold → page change.
                m_hBrokeToPage = true;
            }
        }

        startCommitAnimation(toNext, velocity);
    } else {
        startBounceAnimation(m_slideOffset, velocity);
    }
    // V32.34+ unified nav: any gesture end clears the friction barrier (the
    // commit/break paths that needed it already handled their own clear; this
    // covers the non-committing bounce and the page-break fall-through).
    if (unifiedNavEnabled() && !qFuzzyIsNull(m_barrierOpacity)) {
        m_barrierOpacity = 0.0;
        update();
    }
    return true;
}

// ─── Event routing ────────────────────────────────────────────────────────────

// ─── V32.33: Top-down gesture intent classifier ──────────────────────────────
//
// Walks the widget chain from `leaf` upward toward `this`, returning the
// first matching intent. This is the SOLE place where SwipeNavigator
// decides what a gesture is for. Once classified at TouchBegin, the
// intent persists for the entire touch lifecycle — TouchUpdate dispatches
// based on intent without reclassifying. This eliminates the
// reclassify-mid-stroke jitter the previous hybrid design suffered.
//
// Ordering:
//   1. Interactive widgets (QPushButton, QLineEdit, etc.) win first —
//      the user is trying to tap a control, not start any gesture.
//   2. Property-based opt-out (`swipenav-passthrough`) — for widgets
//      that aren't interactive types but need to receive raw touches
//      (custom controls, drag handles, signature pads).
//   3. Scroll areas with vertical room win next — the user can scroll
//      content inside this area.
//   4. Scroll areas with only horizontal room win after that — typically
//      a themed-carousel, which installs its own filter.
//   5. If nothing in the chain claimed it, SwipeNavigator owns the
//      gesture for page-swipe purposes.
//
// The walk stops at the first match. A button INSIDE a scroll area
// correctly resolves to PassToChild — the button is the leaf-most
// interactive widget and the user clearly wanted to tap it.
SwipeNavigator::GestureIntent
SwipeNavigator::classifyIntent(const QWidget *leaf,
                               QAbstractScrollArea **scrollAreaOut) const
{
    if (scrollAreaOut) *scrollAreaOut = nullptr;

    // Tracks the BEST scroll-area we've seen during the walk. Why we
    // don't just return on the first hit: if there's a button INSIDE a
    // scroll area, we want PassToChild for the button, but if there's
    // just a label inside the scroll area, we want ChildScrollV. So we
    // record the scroll-area as we pass it but keep walking up to look
    // for an interactive widget at outer layers (uncommon but possible
    // — e.g. a button overlaying a scroll area).
    QAbstractScrollArea *scrollHit = nullptr;
    bool scrollHasV = false;
    bool scrollHasH = false;

    const QWidget *w = leaf;
    while (w && w != this) {
        // 1. Interactive widget — highest priority.
        if (qobject_cast<const QAbstractButton *>(w)
         || qobject_cast<const QLineEdit *>(w)
         || qobject_cast<const QSlider *>(w)
         || qobject_cast<const QComboBox *>(w)
         || qobject_cast<const QCheckBox *>(w)
         || qobject_cast<const QSpinBox *>(w)
         || w->property("swipenav-tappable").toBool()
         || w->property("swipenav-passthrough").toBool()) {
            return GestureIntent::PassToChild;
        }
        // 2. Scroll area — record but keep walking for any outer-layer
        //    interactive widget.
        if (!scrollHit) {
            if (const auto *sa = qobject_cast<const QAbstractScrollArea *>(w)) {
                const QScrollBar *vbar = sa->verticalScrollBar();
                const QScrollBar *hbar = sa->horizontalScrollBar();
                scrollHasV = vbar && vbar->maximum() > vbar->minimum();
                scrollHasH = hbar && hbar->maximum() > hbar->minimum();
                if (scrollHasV || scrollHasH) {
                    scrollHit = const_cast<QAbstractScrollArea *>(sa);
                }
            }
        }
        w = w->parentWidget();
    }

    // If we recorded a scroll area and no interactive widget overrode it,
    // return the appropriate scroll intent.
    if (scrollHit) {
        if (scrollAreaOut) *scrollAreaOut = scrollHit;
        return scrollHasV ? GestureIntent::ChildScrollV
                          : GestureIntent::ChildScrollH;
    }

    // Default: SwipeNavigator owns the gesture for page-swipe.
    return GestureIntent::PageSwipe;
}

// ─── V32.33+ Phase 1: Ambiguous-state resolution helpers ─────────────────────
//
// Called when motion crosses kLockThresholdPx during Ambiguous state.
// Picks the resolved intent based on motion direction and the candidate
// captured at TouchBegin.
//
// Resolution rules:
//   - Horizontal motion crosses threshold → PageSwipe ALWAYS. Even
//     inside a scrollable area, horizontal motion is interpreted as a
//     page-swipe attempt. The Ambiguous state made this safe to do —
//     no behavior had committed yet.
//   - Vertical motion crosses threshold AND candidate was ChildScrollV
//     → ChildScrollV. Engages the scroll-area passthrough machinery
//     with the motion accumulated so far.
//   - Vertical motion crosses threshold AND candidate was PageSwipe
//     → stays Ambiguous. Vertical motion in a non-scrollable area
//     has no meaning today. (Future: swipe-to-dismiss?)
//
// After resolution, emits gestureAffordanceResolved(focus, finalIntent)
// so the focus widget can transition from consider-state to either
// idle (intent != tap) or active (intent == tap, which is resolved
// in endAmbiguous, not here).
SwipeNavigator::GestureIntent
SwipeNavigator::resolveAmbiguous(qreal dx, qreal dy)
{
    const qreal absdx = qAbs(dx);
    const qreal absdy = qAbs(dy);

    // V32.33+ Phase 1 tuning: per-axis thresholds. Horizontal page-swipe
    // resolves at kPageSwipeThresholdPx (6px) — low for responsiveness
    // because the user clearly intends to navigate. Vertical scroll
    // engagement requires kScrollThresholdPx (14px) — higher because
    // scroll is incidental and we want to prevent accidental engagement.
    //
    // The horizontal-axis-bias still protects against diagonal motion:
    // horizontal wins only if absdx * 1.5 > absdy. So a 6px horizontal
    // motion only resolves to PageSwipe if vertical motion is < 9px.
    //
    // V32.34+ scroll/gesture passthrough: if this gesture's candidate is a
    // horizontally-scrollable carousel (ChildScrollH) and the dominant motion
    // is horizontal, FIRST try to scroll the carousel. Only when it's clamped
    // at the edge in the drag direction do we fall through to the PageSwipe
    // block below — that's the "scroll past the end → next page" handoff. The
    // forwarding + clamped-overflow capture reuses the same commitScrollDelta
    // machinery ChildScrollV uses. Guarded to unifiedNavEnabled() (the flag-off
    // path deferred to the child entirely back at classification, so we never
    // reach here with the flag off and a scrollable carousel).
    if (m_candidateIntent == GestureIntent::ChildScrollH
     && absdx >= static_cast<qreal>(kScrollThresholdPx)
     && absdx * kHorizontalAxisBias > absdy
     && m_scrollTarget) {
        QScrollBar *hb = m_scrollTarget->horizontalScrollBar();
        // Can the carousel still move in the direction the finger is pulling?
        // dx>0 (finger right) scrolls content toward MIN; dx<0 toward MAX.
        const bool canScroll = hb &&
            ((dx > 0 && hb->value() > hb->minimum()) ||
             (dx < 0 && hb->value() < hb->maximum()));
        if (canScroll) {
            // Own it as a horizontal child-scroll. Forward happens on the next
            // TouchUpdate via the ChildScrollH dispatch (mirrors ChildScrollV).
            m_intent              = GestureIntent::ChildScrollH;
            m_passingToScrollArea = true;
            m_axisLock            = AxisLock::Horizontal;
            qCDebug(lcSwipeNav, "Ambiguous → ChildScrollH (gesture-id=%llu) "
                                "dx=%.1f dy=%.1f",
                    static_cast<unsigned long long>(m_currentGestureId), dx, dy);
            emit gestureAffordanceResolved(m_affordanceFocus.data(),
                                           GestureIntent::ChildScrollH);
            return GestureIntent::ChildScrollH;
        }
        // Carousel is at the edge in the drag direction → fall through to the
        // PageSwipe block, completing the passthrough to page navigation.
        qCInfo(lcSwipeNav, "[HANDOFF] carousel at edge → passing through to "
                           "page swipe (dx=%.1f)", dx);
    }

    // Horizontal wins if it's past its (low) threshold AND dominant by
    // the axis-bias. This is the architectural fix: horizontal motion
    // in a scrollable area now resolves to PageSwipe instead of being
    // swallowed.
    if (absdx >= static_cast<qreal>(kPageSwipeThresholdPx)
     && absdx * kHorizontalAxisBias > absdy) {
        m_intent              = GestureIntent::PageSwipe;
        m_passingToScrollArea = false;
        m_scrollTarget        = nullptr;
        // V32.33+ Phase 1: instead of calling gestureBegin (which would
        // velocityReset() and wipe the Ambiguous-phase velocity samples),
        // inline just the animation-handoff part of gestureBegin. State
        // like m_touchStart, m_dragDistance, m_pageWidth was already
        // initialized at TouchBegin. Velocity has been tracked during
        // Ambiguous — preserve it.
        //
        // Note: event() TouchBegin already stops any running animation
        // before entering Ambiguous. This check is defensive — if some
        // future path enters Ambiguous without going through event()
        // TouchBegin (e.g. eventFilter migration), this still cleans up.
        if (m_anim->state() == QAbstractAnimation::Running) {
            m_anim->stop();
            disconnect(m_animConn);
            const int pageW = (m_pageWidth > 0) ? m_pageWidth : width();
            const int prevIndex = m_currentIndex;
            if (pageW > 0) {
                if (m_slideOffset < -(pageW / 2) &&
                    m_currentIndex + 1 < static_cast<int>(m_pages.size()))
                    ++m_currentIndex;
                else if (m_slideOffset > (pageW / 2) && m_currentIndex > 0)
                    --m_currentIndex;
            }
            m_slideOffset = 0.0;
            layoutPages();
            // V32.33+ Phase 1 audit: only emit on actual change.
            if (m_currentIndex != prevIndex)
                notifyPageTransition(m_currentIndex);
        }
        qCDebug(lcSwipeNav, "Ambiguous → PageSwipe (gesture-id=%llu) "
                            "dx=%.1f dy=%.1f",
                static_cast<unsigned long long>(m_currentGestureId),
                dx, dy);
        emit gestureAffordanceResolved(m_affordanceFocus.data(),
                                       GestureIntent::PageSwipe);
        return GestureIntent::PageSwipe;
    }

    // Vertical wins if past its (higher) threshold AND the candidate
    // supports it (i.e. ChildScrollV — vertical motion in a non-scrollable
    // area has no defined meaning today, so we stay Ambiguous).
    if (absdy >= static_cast<qreal>(kScrollThresholdPx)
     && m_candidateIntent == GestureIntent::ChildScrollV) {
        m_intent              = GestureIntent::ChildScrollV;
        m_passingToScrollArea = true;
        // m_scrollTarget was already set at TouchBegin from the candidate
        // classification. m_axisLock is set inside the ChildScrollV
        // dispatch path on next TouchUpdate.
        qCDebug(lcSwipeNav, "Ambiguous → ChildScrollV (gesture-id=%llu) "
                            "dx=%.1f dy=%.1f",
                static_cast<unsigned long long>(m_currentGestureId),
                dx, dy);
        emit gestureAffordanceResolved(m_affordanceFocus.data(),
                                       GestureIntent::ChildScrollV);
        return GestureIntent::ChildScrollV;
    }

    // Neither axis crossed in a way that resolves — stay Ambiguous.
    return GestureIntent::Ambiguous;
}

// Called when TouchEnd arrives while still in Ambiguous state. Distinguishes
// tap (small motion → forward click to focus widget) from cancel
// (motion exceeded tap threshold but didn't cross axis-lock).
void SwipeNavigator::endAmbiguous(const QPointF &endPos, bool wasTap)
{
    Q_UNUSED(endPos);  // V32.33+ Phase 1 audit: see comment below
    if (wasTap) {
        qCDebug(lcSwipeNav, "Ambiguous → TapCommit (gesture-id=%llu)",
                static_cast<unsigned long long>(m_currentGestureId));
        emit gestureAffordanceResolved(m_affordanceFocus.data(),
                                       GestureIntent::TapCommit);
        // V32.33+ Phase 1 audit: fire the tap at m_touchStart, not endPos.
        // With kTapMaxMovementPx = 16, the finger can drift 16px between
        // press and release. If we fire at endPos, the click could land
        // on an adjacent widget that the user didn't intend. Matches
        // iOS/Android convention: tap fires on the widget under the
        // press point, not the release point.
        forwardTap(m_touchStart);
    } else {
        qCDebug(lcSwipeNav, "Ambiguous → Cancelled (gesture-id=%llu)",
                static_cast<unsigned long long>(m_currentGestureId));
        emit gestureAffordanceCancelled(m_affordanceFocus.data());
    }
}

bool SwipeNavigator::event(QEvent *e)
{
    switch (e->type()) {
    case QEvent::TouchBegin: {
        const auto pts = static_cast<QTouchEvent *>(e)->points();
        if (pts.isEmpty()) { e->accept(); return true; }

        // V32.34+: a new finger-down cancels any in-flight fling. The
        // user is grabbing the content again — they expect the scroll
        // to stop where it is so they can act on what's now under
        // their finger. Done BEFORE the single-finger-lock check so
        // even a refused touch (multi-finger during fling) interrupts
        // the decay — the user's first finger may have already lifted
        // and the "second" touch is logically a fresh grab.
        cancelFling();
        // Same logic for any in-flight spring-back animation: the
        // user is grabbing the overscrolled viewport. They want to
        // extend the overscroll (or hold it in place), not have it
        // continue collapsing toward zero. cancelSpringBack
        // preserves the current visual offset so applyOverscroll
        // continues from where the spring left off.
        cancelSpringBack();

        // V32.32: single-finger lock. If another gesture is already in
        // flight (locked to a different point id), refuse the new touch
        // — don't extend, don't reset, don't fire anything.
        if (m_lockedPointId >= 0) {
            e->accept();
            return true;
        }

        const QEventPoint *active = nullptr;
        for (const auto &p : pts)
            if (p.state() != QEventPoint::Released) { active = &p; break; }
        const QPointF touchPos = active ? active->position() : pts.first().position();

        // V32.33: TOP-DOWN INTENT CLASSIFICATION. Single decision for
        // the lifetime of this gesture. See classifyIntent() doc above.
        const QPoint gp = mapToGlobal(touchPos.toPoint());
        const QWidget *leaf = QApplication::widgetAt(gp);
        QAbstractScrollArea *sa = nullptr;
        const GestureIntent candidate = classifyIntent(leaf, &sa);

        // V32.34+ diagnostic: when unified nav is on, log what each touch-down
        // classified as (one line per gesture, not per frame — classifyIntent
        // runs once at TouchBegin). This closes the gap between "[UNIFIED-NAV]
        // ENABLED" and the "[HANDOFF]" lines: if a swipe does nothing, this
        // shows whether it was classified PageSwipe (should navigate),
        // ChildScrollH/V (went to a scroll area), or PassToChild (hit a button).
        // Silent in the default build (gated on the flag), so no release spam.
        if (unifiedNavEnabled()) {
            const char *name =
                candidate == GestureIntent::PageSwipe    ? "PageSwipe"    :
                candidate == GestureIntent::ChildScrollV ? "ChildScrollV" :
                candidate == GestureIntent::ChildScrollH ? "ChildScrollH" :
                candidate == GestureIntent::PassToChild  ? "PassToChild"  :
                candidate == GestureIntent::TapCommit    ? "TapCommit"    :
                candidate == GestureIntent::Ambiguous    ? "Ambiguous"    :
                                                           "None";
            qCInfo(lcSwipeNav, "[GESTURE] touch classified as %s", name);
        }

        // PassToChild: this touch is for a button/input/etc. Don't
        // consume; let Qt fall back to mouse-event synthesis FOR THAT
        // WIDGET so taps work normally. CRITICAL: must explicitly
        // ignore() — Qt6 QTouchEvent starts with accepted=true so
        // simply returning false from event() doesn't undo that, and
        // Qt would then think we handled the touch and refuse to
        // synthesize mouse events for the child.
        if (candidate == GestureIntent::PassToChild) {
            e->ignore();
            return false;
        }

        // ChildScrollH: horizontal-only scroll area (themed carousel etc.).
        // V32.34+ scroll/gesture passthrough to next page:
        //   - No horizontal range (content fits) → don't defer; fall through to
        //     own the gesture so the swipe resolves as a normal PageSwipe.
        //   - Has horizontal range → we now OWN the gesture (enter Ambiguous,
        //     remembering the scroll target) instead of deferring entirely. On
        //     horizontal motion we forward scroll to the carousel via
        //     commitScrollDelta (which captures clamped overflow); when it
        //     clamps at the edge and the user keeps pulling the same way, the
        //     overflow reclassifies the gesture to PageSwipe so the drag passes
        //     through to page navigation. This replaces the old "defer entirely"
        //     which silently swallowed the swipe at the carousel's end. The
        //     wheel/trackpad path has the analogous handoff in
        //     HorizontalScrollArea::wheelEvent. Behind unifiedNavEnabled() for
        //     the page-handoff feel; with the flag off it behaves like the old
        //     deferral (overflow just rubber-bands, no page change).
        if (candidate == GestureIntent::ChildScrollH) {
            bool hasHRange = false;
            if (sa) {
                const QScrollBar *hb = sa->horizontalScrollBar();
                hasHRange = hb && hb->maximum() > hb->minimum();
            }
            if (hasHRange && !unifiedNavEnabled()) {
                // Flag off: preserve the original behavior exactly — defer to
                // the child's own kinetic scroll, don't track a gesture.
                m_intent          = GestureIntent::None;
                m_candidateIntent = GestureIntent::None;
                m_lockedPointId   = -1;
                m_passingToScrollArea = false;
                m_scrollTarget    = nullptr;
                return false;
            }
            // hasHRange && unifiedNavEnabled() → fall through to own the
            // gesture (Ambiguous); m_scrollTarget is set below so we can
            // forward horizontal scroll and detect the edge.
            // !hasHRange (any flag) → also fall through; resolves to PageSwipe.
        }

        // From here on, we own the gesture. Enter Ambiguous state.
        // No behavior commits yet — neither scroll nor swipe nor click.
        // We hold this state until motion crosses a threshold (resolve to
        // ChildScrollV or PageSwipe) or until TouchEnd with small motion
        // (resolve to TapCommit). The architectural win: horizontal motion
        // in a scrollable area resolves to PageSwipe instead of being
        // silently swallowed. See docs/GESTURE_AFFORDANCE_MIGRATION.md.

        // V32.33+ Phase 1 audit: if a previous gesture's commit/bounce
        // animation is still running, stop it and snap-decide the page.
        // Without this, finger-down-during-animation leaves m_slideOffset
        // being driven both by user motion AND the animation, producing
        // visible conflict on the page transition. This is the same
        // animation-handoff logic that gestureBegin does — we need it
        // here too because event() TouchBegin doesn't call gestureBegin
        // (gestureBegin is deferred until resolveAmbiguous resolves to
        // PageSwipe, which may take 100+ms or never happen if user taps).
        if (m_anim && m_anim->state() == QAbstractAnimation::Running) {
            m_anim->stop();
            disconnect(m_animConn);
            const int pageW = width();
            const int prevIndex = m_currentIndex;
            if (pageW > 0) {
                if (m_slideOffset < -(pageW / 2) &&
                    m_currentIndex + 1 < static_cast<int>(m_pages.size()))
                    ++m_currentIndex;
                else if (m_slideOffset > (pageW / 2) && m_currentIndex > 0)
                    --m_currentIndex;
            }
            m_slideOffset = 0.0;
            layoutPages();
            // V32.33+ Phase 1 audit: only emit on actual change (see
            // gestureBegin for rationale).
            if (m_currentIndex != prevIndex)
                notifyPageTransition(m_currentIndex);
        }

        ++m_currentGestureId;
        m_intent             = GestureIntent::Ambiguous;
        m_candidateIntent    = candidate;
        m_axisLock           = AxisLock::None;
        m_gestureStartedAtMs = QDateTime::currentMSecsSinceEpoch();
        m_lockedPointId      = active ? active->id() : pts.first().id();
        m_scrollTarget       = sa;  // remembered for if-we-resolve-to-ChildScrollV
        m_passingToScrollArea = false;  // not yet — only set when resolved
        m_scrollCommitted    = false;
        m_skipGestureForArea = nullptr;
        m_touchStart         = touchPos;
        m_dragDistance       = 0.0;
        m_pageWidth          = width();
        // V32.33+ Phase 1 audit: explicitly clear m_swipeLocked and
        // m_dragging. Without this, if a wheel gesture was active and
        // got cancelled by the wheel-phase-active branch below
        // (without calling gestureEnd), m_swipeLocked could still be
        // true. Touch motion would then skip the "below-threshold,
        // wait" path in gestureUpdate and apply motion directly.
        m_swipeLocked        = false;
        m_dragging           = false;
        m_touchTimer.restart();
        velocityReset();
        // V32.33+ Phase 1: record initial sample so velocity tracking has
        // a baseline. Without this, motion-during-Ambiguous wasn't tracked
        // for velocity, and resolveAmbiguous's call to gestureBegin would
        // velocityReset() again and only have start + current as samples.
        // gestureEnd's flick-vs-bounce decision needs more history than that.
        velocityRecord(touchPos.x());

        // V32.33+ Phase 1: capture the focus widget for affordance signals.
        // QPointer-safe in case the widget is destroyed during the gesture.
        m_affordanceFocus = const_cast<QWidget *>(leaf);

        qCDebug(lcSwipeNav, "TouchBegin gesture-id=%llu state=Ambiguous candidate=%s",
                static_cast<unsigned long long>(m_currentGestureId),
                candidate == GestureIntent::ChildScrollV ? "ChildScrollV"
              : candidate == GestureIntent::PageSwipe    ? "PageSwipe"
              : "?");

        // Cancel any wheel gesture in progress
        // V32.33+ Phase 1 audit: previously this only stopped the wheel
        // idle timer and reset wheel-specific state, but left m_swipeLocked
        // and m_slideOffset in whatever state the wheel had set them. Now
        // also call gestureEnd (which commits or bounces) and reset slide
        // offset to zero before the new touch gesture begins. This prevents
        // the new gesture from inheriting visual state from the cancelled
        // wheel gesture.
        if (m_wheelPhase == WheelPhase::Active) {
            if (m_wheelIdleTimer) m_wheelIdleTimer->stop();
            m_wheelPhase = WheelPhase::Idle;
            m_wheelHorizontalLocked = false;
            // If wheel had locked a gesture and was animating, the page
            // might be mid-translation. Snap to a clean state. We don't
            // call gestureEnd because that would start an animation;
            // we want a clean reset since the user is starting a fresh
            // touch gesture right now.
            m_swipeLocked = false;
            m_dragging    = false;
            m_slideOffset = 0.0;
            layoutPages();
        }

        // V32.33+ Phase 1: emit affordance-shown signal so widgets CAN
        // display consider-state visuals during Ambiguous. No widgets
        // are subscribed yet — Phase 2 wires up the first one.
        emit gestureAffordanceShown(m_affordanceFocus.data(), gp);

        e->accept();
        return true;
    }

    case QEvent::TouchUpdate: {
        const auto pts = static_cast<QTouchEvent *>(e)->points();
        if (pts.isEmpty()) return false;
        // V32.32: filter to the locked point only.
        const QEventPoint *active = nullptr;
        if (m_lockedPointId >= 0) {
            for (const auto &p : pts) {
                if (p.id() == m_lockedPointId
                 && p.state() != QEventPoint::Released) {
                    active = &p; break;
                }
            }
        }
        if (!active) {
            return false;
        }

        // V32.33: dispatch on intent set at TouchBegin. NO RECLASSIFICATION.
        // V32.33+ Phase 1: Ambiguous resolves to a concrete intent on
        // motion crossing kLockThresholdPx; below threshold, we accept
        // the event and wait. Once resolved, we fall through to the
        // resolved intent's dispatch below — no return between Ambiguous
        // resolution and concrete-intent dispatch on the same event.
        if (m_intent == GestureIntent::Ambiguous) {
            const QPointF cur = active->position();
            const qreal dx = cur.x() - m_touchStart.x();
            const qreal dy = cur.y() - m_touchStart.y();
            // V32.33+ Phase 1: record velocity samples even during
            // Ambiguous, so when we resolve we have proper history for
            // the flick-vs-bounce decision in gestureEnd. Without this,
            // a fast flick that crosses threshold in one event would
            // have only start-position in the velocity buffer and the
            // commit decision would think velocity ≈ 0.
            velocityRecord(cur.x());
            const GestureIntent newIntent = resolveAmbiguous(dx, dy);
            if (newIntent == GestureIntent::Ambiguous) {
                // Still Ambiguous — swallow and wait for more motion.
                e->accept();
                return true;
            }
            // Resolved. m_intent is now ChildScrollV or PageSwipe.
            // Fall through to dispatch below so the motion accumulated
            // so far is applied this turn (otherwise the page would
            // freeze visually until the next TouchUpdate event arrives).
        }

        switch (m_intent) {
        // V32.34+ scroll passthrough: ChildScrollH (a pure-horizontal carousel
        // we now OWN for edge→page handoff) shares this dispatch with
        // ChildScrollV. The axis-lock logic below already handles the
        // horizontal axis and routes edge overflow to applyOverscroll; the
        // ChildScrollH-specific break-to-PageSwipe at the edge was decided in
        // resolveAmbiguous before we got here.
        case GestureIntent::ChildScrollH:
        case GestureIntent::ChildScrollV: {
            const QPointF cur = active->position();
            const qreal dx = cur.x() - m_touchStart.x();
            const qreal dy = cur.y() - m_touchStart.y();

            QScrollBar *vbar = m_scrollTarget ? m_scrollTarget->verticalScrollBar()   : nullptr;
            QScrollBar *hbar = m_scrollTarget ? m_scrollTarget->horizontalScrollBar() : nullptr;
            const bool canScrollV = vbar && vbar->maximum() > vbar->minimum();
            const bool canScrollH = hbar && hbar->maximum() > hbar->minimum();

            // Axis-lock on first significant motion within the scroll area.
            // This decides H-scroll vs V-scroll WITHIN the scroll area —
            // it does NOT fall through to page-swipe (that would be a
            // reclassification, which the new design prohibits — and
            // Phase 1's resolveAmbiguous already handled the horizontal
            // case before we got here).
            if (m_axisLock == AxisLock::None) {
                const qreal absdx = qAbs(dx);
                const qreal absdy = qAbs(dy);
                if (absdx >= static_cast<qreal>(kLockThresholdPx)
                 || absdy >= static_cast<qreal>(kLockThresholdPx)) {
                    m_axisLock = (absdx * kHorizontalAxisBias > absdy) ? AxisLock::Horizontal
                                                  : AxisLock::Vertical;
                    qCDebug(lcSwipeNav, "axis-lock gesture-id=%llu → %s",
                            static_cast<unsigned long long>(m_currentGestureId),
                            m_axisLock == AxisLock::Horizontal ? "H" : "V");
                }
            }

            if (m_axisLock == AxisLock::Horizontal) {
                if (qAbs(dx) < 2) { e->accept(); return true; }
                if (canScrollH) {
                    m_scrollCommitted = true;
                    m_touchStart = cur;
                    // V32.34+: edge-detection BEFORE the commit. If
                    // the bar is at a limit AND motion is in the
                    // direction that would push past it, the commit
                    // would clamp and we'd silently lose the user's
                    // input. Instead, route to overscroll. The
                    // direction check uses sign of dx: positive dx
                    // (= setValue decreases) means going toward
                    // bar minimum; negative dx toward maximum.
                    const int curVal = hbar->value();
                    const bool atMin = curVal == hbar->minimum();
                    const bool atMax = curVal == hbar->maximum();
                    if ((atMin && dx > 0) || (atMax && dx < 0)) {
                        applyOverscroll(dx, 0);
                    } else {
                        commitScrollPaced(hbar, nullptr, static_cast<int>(dx), 0);
                    }
                    // V32.34+: record velocity sample for fling-on-release.
                    // We record AFTER commit so the sample matches what
                    // the scrollbar actually consumed (pacing might
                    // buffer some). dy is 0 for the H-locked axis.
                    recordVelocitySample(dx, 0);
                }
                e->accept();
                return true;
            }
            if (m_axisLock == AxisLock::Vertical) {
                if (qAbs(dy) < 2) { e->accept(); return true; }
                if (canScrollV) {
                    m_scrollCommitted = true;
                    m_touchStart = cur;
                    const int curVal = vbar->value();
                    const bool atMin = curVal == vbar->minimum();
                    const bool atMax = curVal == vbar->maximum();
                    if ((atMin && dy > 0) || (atMax && dy < 0)) {
                        applyOverscroll(0, dy);
                    } else {
                        commitScrollPaced(nullptr, vbar, 0, static_cast<int>(dy));
                    }
                    recordVelocitySample(0, dy);
                }
                e->accept();
                return true;
            }
            // No lock yet — eat and wait.
            e->accept();
            return true;
        }
        case GestureIntent::PageSwipe: {
            if (gestureUpdate(active->position())) { e->accept(); return true; }
            e->ignore(); return false;
        }
        case GestureIntent::Ambiguous:
            // Couldn't have reached here — Ambiguous was handled above.
            // Defensive: stay safe and accept the event.
            e->accept();
            return true;
        case GestureIntent::PassToChild:
        case GestureIntent::TapCommit:
        case GestureIntent::None:
        default:
            // Shouldn't reach here — PassToChild ignored at Begin,
            // TapCommit fires only at TouchEnd, None means no gesture in
            // flight. (ChildScrollH is no longer here — V32.34+ it's owned
            // and shares the ChildScrollV case above for edge→page handoff.)
            return false;
        }
    }

    case QEvent::TouchEnd:
    case QEvent::TouchCancel: {
        // V32.33: dispatch end-of-gesture per intent before resetting.
        // V32.33+ Phase 1: Ambiguous resolves on release as either
        // TapCommit (small movement, forward click) or Cancel
        // (movement exceeded tap threshold but didn't cross axis-lock).
        //
        // CRITICAL: TouchCancel is NOT the same as TouchEnd. TouchCancel
        // means the SYSTEM revoked the gesture (palm rejection, app
        // focus change, hardware glitch) — the user did not intentionally
        // release. We must never fire a synthetic tap on TouchCancel, even
        // if motion was small. Always cancel.
        const bool isSystemCancel = (e->type() == QEvent::TouchCancel);
        switch (m_intent) {
        case GestureIntent::Ambiguous: {
            const auto &tp = static_cast<QTouchEvent *>(e)->points();
            const QPointF startPos = m_touchStart;
            QPointF endPos = startPos;
            // V32.33+ Phase 1: find the locked point specifically, not
            // just tp.first(). Multi-touch end events can deliver points
            // in any order, and using .first() could pick a finger that
            // wasn't our locked gesture-owner — leading to wrong endPos
            // and wrong tap/cancel decision.
            if (!tp.isEmpty()) {
                if (m_lockedPointId >= 0) {
                    for (const auto &p : tp) {
                        if (p.id() == m_lockedPointId) {
                            endPos = p.position();
                            break;
                        }
                    }
                } else {
                    endPos = tp.first().position();
                }
            }
            const qreal dist = QLineF(startPos, endPos).length();
            // wasTap only on user-initiated TouchEnd, never on TouchCancel.
            const bool wasTap = !isSystemCancel
                             && dist < static_cast<qreal>(kTapMaxMovementPx);
            // endAmbiguous emits the right affordance signal AND forwards
            // the tap if it was one. Must run BEFORE resetGestureState
            // because that clears m_affordanceFocus.
            endAmbiguous(endPos, wasTap);
            resetGestureState();
            e->accept();
            return true;
        }
        case GestureIntent::ChildScrollH:   // V32.34+: shares release/fling path
        case GestureIntent::ChildScrollV: {
            const bool didScroll = m_scrollCommitted;
            const QPointF startPos = m_touchStart;
            const qint64 lockedId = m_lockedPointId;  // capture before reset

            // V32.34+: compute release velocity and start the fling
            // animation BEFORE resetGestureState() clears m_scrollTarget.
            //
            // Conditions for a fling:
            //   1. We actually scrolled (didScroll true — small motion
            //      taps don't fling).
            //   2. Not a system-initiated cancel — those are not user
            //      "letting go", they're forced aborts. Treating a
            //      cancel as a fling-trigger would launch a fling the
            //      user didn't ask for (e.g. modal popping up, focus
            //      stolen by system overlay).
            //   3. Velocity above kMinFlingVelocity — startFling
            //      enforces this and bails silently otherwise.
            if (didScroll && !isSystemCancel && m_scrollTarget) {
                qreal vx = 0.0, vy = 0.0;
                computeReleaseVelocity(&vx, &vy);
                // We only want axis-locked fling — don't try to fling
                // an axis the user wasn't scrolling. Without this, a
                // mostly-vertical drag with tiny horizontal jitter
                // would launch a tiny horizontal fling that nobody
                // wanted.
                if (m_axisLock == AxisLock::Vertical)   vx = 0.0;
                if (m_axisLock == AxisLock::Horizontal) vy = 0.0;
                startFling(vx, vy);  // bails if below threshold
            }

            // V32.34+: spring-back for overscroll. The fling path
            // also calls startSpringBack when its decay completes
            // (see onFlingTick). If the fling didn't start (velocity
            // below threshold OR cancelled because of a system event),
            // OR we're not flinging at all, we still need to release
            // any overscroll the user pulled out during the drag.
            //
            // The condition: spring back if we have overscroll AND
            // no fling is actively running. If a fling IS running,
            // it owns the spring-back trigger via its decay-complete
            // path. Checking m_flingTimer->isActive() here cleanly
            // avoids the double-trigger.
            if (hasOverscroll()
                && (!m_flingTimer || !m_flingTimer->isActive())) {
                startSpringBack();
            }

            // Clear the velocity samples for the next gesture, then
            // run the normal reset. Note: startFling captured the
            // scroll target into m_flingTarget (QPointer) before this
            // reset nulls m_scrollTarget.
            m_velocitySamples.clear();
            m_velocityClock.invalidate();

            resetGestureState();
            // If the user tapped without scrolling (small motion only),
            // forward as a tap so child widgets get a click.
            //
            // Note: under V32.33+ Phase 1, this branch is rarely taken
            // because taps now resolve via Ambiguous → TapCommit
            // before getting here. But preserved as defensive fallback
            // in case some path enters ChildScrollV without going
            // through Ambiguous (e.g. from the eventFilter on child
            // widgets).
            //
            // V32.33+ Phase 1: never fire a synthetic tap on TouchCancel.
            if (!didScroll && !isSystemCancel) {
                const auto &tp = static_cast<QTouchEvent *>(e)->points();
                if (!tp.isEmpty()) {
                    // Find the locked point specifically.
                    QPointF endPos = tp.first().position();
                    if (lockedId >= 0) {
                        for (const auto &p : tp) {
                            if (p.id() == lockedId) {
                                endPos = p.position();
                                break;
                            }
                        }
                    }
                    const qreal dist = QLineF(startPos, endPos).length();
                    // V32.33+ Phase 1 audit: tap fires at startPos, not
                    // endPos. See endAmbiguous for rationale.
                    if (dist < 12.0) forwardTap(startPos);
                }
            }
            e->accept();
            return true;
        }
        case GestureIntent::PageSwipe: {
            const auto pts = static_cast<QTouchEvent *>(e)->points();
            const qint64 lockedId = m_lockedPointId;
            bool consumed = false;
            if (!pts.isEmpty()) {
                // Find the locked point specifically — pts.first() can
                // be any finger in multi-touch end events.
                QPointF endPos = pts.first().position();
                if (lockedId >= 0) {
                    for (const auto &p : pts) {
                        if (p.id() == lockedId) {
                            endPos = p.position();
                            break;
                        }
                    }
                }
                consumed = gestureEnd(endPos);
            }
            resetGestureState();
            if (consumed) { e->accept(); return true; }
            e->ignore(); return false;
        }
        case GestureIntent::PassToChild:
        case GestureIntent::TapCommit:
        case GestureIntent::None:
        default:
            // (ChildScrollH no longer here — V32.34+ owned, shares the
            // ChildScrollV release/fling case above.)
            resetGestureState();
            return false;
        }
    }
    default: break;
    }
    return QWidget::event(e);
}

// ─── Mouse events ─────────────────────────────────────────────────────────────

void SwipeNavigator::mousePressEvent(QMouseEvent *e)
{
    if (e->button() == Qt::LeftButton) gestureBegin(e->position());
    QWidget::mousePressEvent(e);
}

void SwipeNavigator::mouseMoveEvent(QMouseEvent *e)
{
    if ((e->buttons() & Qt::LeftButton) && gestureUpdate(e->position())) {
        e->accept(); return;
    }
    QWidget::mouseMoveEvent(e);
}

void SwipeNavigator::mouseReleaseEvent(QMouseEvent *e)
{
    if (e->button() == Qt::LeftButton) {
        if (gestureEnd(e->position())) { e->accept(); return; }
    }
    QWidget::mouseReleaseEvent(e);
}

// ─── Keyboard ─────────────────────────────────────────────────────────────────

void SwipeNavigator::keyPressEvent(QKeyEvent *e)
{
    // V32.34+ unified nav: arrows (and Ctrl+Tab / Ctrl+Shift+Tab) traverse
    // sub-tabs first, breaking to the adjacent page at the cycle boundary —
    // the keyboard mirror of the horizontal swipe's two-level model. Landing
    // on a new page goes to its first sub-tab. Inert unless the flag is on,
    // and only when the current page actually has sub-tabs (else fall through
    // to plain page paging).
    if (unifiedNavEnabled() && currentSubTabbed() && currentSubTabCount() > 1) {
        int dir = 0;
        const bool ctrlTab = (e->key() == Qt::Key_Tab)
                          && (e->modifiers() & Qt::ControlModifier);
        const bool ctrlShiftTab = (e->key() == Qt::Key_Backtab)
                          && (e->modifiers() & Qt::ControlModifier);
        if (e->key() == Qt::Key_Right || ctrlTab)      dir = +1;
        else if (e->key() == Qt::Key_Left || ctrlShiftTab) dir = -1;
        if (dir != 0) {
            // advanceSubTab is non-wrapping: false at the edge → break to the
            // adjacent page, landing on its first sub-tab (user nav →
            // resetSubTab=true). Within range, just move the sub-tab.
            if (!advanceSubTab(dir))
                goToPage(m_currentIndex + dir, /*animate=*/true,
                         /*resetSubTab=*/true);
            e->accept();
            return;
        }
    }

    switch (e->key()) {
    case Qt::Key_Right: goToPage(m_currentIndex + 1, true, /*resetSubTab=*/true); break;
    case Qt::Key_Left:  goToPage(m_currentIndex - 1, true, /*resetSubTab=*/true); break;
    default: QWidget::keyPressEvent(e); break;
    }
}

void SwipeNavigator::resizeEvent(QResizeEvent *)
{
    if (!m_dragging) m_pageWidth = 0;
    layoutPages();
}

void SwipeNavigator::paintEvent(QPaintEvent *)
{
    // TabBar handles navigation indicators. V32.34+ unified nav: when a detent
    // is building, fade in a generic barrier overlay proportional to progress
    // (m_barrierOpacity, 0..1) so the user feels the boundary before it gives.
    // One generic barrier for now (specialize per-axis later, per design).
    if (!unifiedNavEnabled() || m_barrierOpacity <= 0.01) return;

    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing, true);
    const qreal opRaw = qBound(0.0, m_barrierOpacity, 1.0);
    // Material-style ease: the glow should ramp gently at first then firm up
    // near the commit point, so the boundary feels like it's "resisting" more
    // as you push. smoothstep (3t²−2t³) gives that S-curve.
    const qreal op = opRaw * opRaw * (3.0 - 2.0 * opRaw);

    // Material-style edge glow: a soft gradient strongest at the active edge,
    // fading inward, PLUS a brighter thin "ridge" right at the very edge (like
    // Material's stretch/scroll-edge highlight) so there's a crisp line that
    // intensifies toward commit rather than only a diffuse haze. The edge is
    // chosen by m_barrierEdge (0=bottom 1=top 2=right 3=left). Thickness and
    // alpha track detent progress so the boundary visibly "firms up".
    QColor edgeC(Theme::ACCENT());
    edgeC.setAlphaF(0.10 + 0.55 * op);          // 0.10..0.65 at the edge
    QColor fadeC = edgeC;
    fadeC.setAlphaF(0.0);                        // fully transparent inward
    QColor ridgeC(Theme::ACCENT());
    ridgeC.setAlphaF(0.25 + 0.60 * op);          // brighter crisp leading line
    const int thickness = 6 + static_cast<int>(26.0 * op);  // 6..32px glow depth
    const int ridgePx   = 2 + static_cast<int>(2.0 * op);   // 2..4px ridge
    const int w = width(), h = height();

    QLinearGradient g;
    QRect band;
    QRect ridge;
    switch (m_barrierEdge) {
    case 1: // top
        band = QRect(0, 0, w, thickness);
        g = QLinearGradient(0, 0, 0, thickness);
        g.setColorAt(0.0, edgeC); g.setColorAt(1.0, fadeC);
        ridge = QRect(0, 0, w, ridgePx);
        break;
    case 2: // right
        band = QRect(w - thickness, 0, thickness, h);
        g = QLinearGradient(w, 0, w - thickness, 0);
        g.setColorAt(0.0, edgeC); g.setColorAt(1.0, fadeC);
        ridge = QRect(w - ridgePx, 0, ridgePx, h);
        break;
    case 3: // left
        band = QRect(0, 0, thickness, h);
        g = QLinearGradient(0, 0, thickness, 0);
        g.setColorAt(0.0, edgeC); g.setColorAt(1.0, fadeC);
        ridge = QRect(0, 0, ridgePx, h);
        break;
    case 0: // bottom (default)
    default:
        band = QRect(0, h - thickness, w, thickness);
        g = QLinearGradient(0, h, 0, h - thickness);
        g.setColorAt(0.0, edgeC); g.setColorAt(1.0, fadeC);
        ridge = QRect(0, h - ridgePx, w, ridgePx);
        break;
    }
    p.fillRect(band, g);
    p.fillRect(ridge, ridgeC);   // crisp leading edge over the diffuse glow

    // V32.39+: grow-text. Name the sub-tab the gesture is reaching for and
    // scale it by detent progress (m_barrierTextScale), so the label visibly
    // swells toward the commit point — a clear visual signal tied to how far
    // the pull has gone. Vertical detent only (top/bottom edge); the bottom
    // edge reaches for NEXT, the top for PREV (matching the fire mapping).
    if (m_barrierEdge == 0 || m_barrierEdge == 1) {
        if (SubTabbed *st = currentSubTabbed()) {
            const int n = st->subTabCount();
            if (n > 1) {
                const int dir      = (m_barrierEdge == 0) ? +1 : -1;
                const int incoming = qBound(0, st->currentSubTab() + dir, n - 1);
                const QString label = st->subTabLabel(incoming);
                if (!label.isEmpty()) {
                    QFont f = font();
                    const qreal basePt = f.pointSizeF() > 0 ? f.pointSizeF() : 13.0;
                    f.setPointSizeF(basePt * m_barrierTextScale);
                    f.setWeight(QFont::DemiBold);
                    p.setFont(f);
                    QColor txt(Theme::TEXT_PRIMARY());
                    txt.setAlphaF(0.95 * op);             // fade in with the barrier
                    p.setPen(txt);
                    const int pad = thickness + 14;
                    const QRect area = (m_barrierEdge == 1)
                        ? QRect(0, pad, w, 44)             // just inside the top edge
                        : QRect(0, h - pad - 44, w, 44);   // just inside the bottom edge
                    p.drawText(area, Qt::AlignHCenter | Qt::AlignVCenter, label);
                }
            }
        }
    }
}

// ─── Wheel / trackpad ─────────────────────────────────────────────────────────

bool SwipeNavigator::isHorizontalScrollSuppressed(const QPoint &globalPos) const
{
    const QWidget *w = QApplication::widgetAt(globalPos);
    while (w && w != this) {
        if (const auto *sa = qobject_cast<const QAbstractScrollArea *>(w)) {
            const QScrollBar *hbar = sa->horizontalScrollBar();
            if (hbar && (hbar->maximum() - hbar->minimum()) > 10) return true;
        }
        w = w->parentWidget();
    }
    return false;
}

void SwipeNavigator::childEvent(QChildEvent *e)
{
    if (e->added() && e->child()->isWidgetType())
        installFilterRecursive(static_cast<QWidget *>(e->child()));
    else if (e->removed() && e->child()->isWidgetType()) {
        // V32.33+ Phase 1 audit: removal must be recursive to match
        // installFilterRecursive's recursive addition. Previously only
        // the direct child was removed from m_filteredWidgets, leaving
        // stale pointers to descendants in the set. If Qt later reused
        // those memory addresses for new (unrelated) widgets, the new
        // widgets would be incorrectly treated as filtered.
        QWidget *w = static_cast<QWidget *>(e->child());
        uninstallFilterRecursive(w);
    }
    QWidget::childEvent(e);
}

void SwipeNavigator::uninstallFilterRecursive(QWidget *w)
{
    m_filteredWidgets.remove(w);
    for (QObject *child : w->children())
        if (child->isWidgetType())
            uninstallFilterRecursive(static_cast<QWidget *>(child));
}

void SwipeNavigator::installFilterRecursive(QWidget *w)
{
    // We register the widget so the eventFilter early-out lets events pass through.
    // We do NOT set WA_AcceptTouchEvents — this would break button taps on touch:
    //   • With WA_AcceptTouchEvents, Qt sends TouchBegin/Update/End to the widget
    //     directly. Buttons don't override these, so they never fire clicks.
    //   • Without WA_AcceptTouchEvents, Qt synthesizes mouse press/release from
    //     touch events, and button click handlers fire normally.
    //
    // The app-level filter (QApplication::instance()->installEventFilter(this))
    // sees ALL events first, so we still get to inspect TouchBegin for swipe
    // detection. We just don't BLOCK Qt's mouse synthesis.
    m_filteredWidgets.insert(w);
    // V32.34+: connect destroyed so a widget that gets deleted while
    // still registered cleans itself out of m_filteredWidgets. Without
    // this, the QSet retained dangling pointers indefinitely (one per
    // ever-registered widget that was destroyed without an explicit
    // unregister). The lookup `m_filteredWidgets.contains(obj)` would
    // never match the dangling pointers (Qt reuses addresses for new
    // widgets, but the SET KEY is the OLD pointer) so the entries
    // never caused incorrect behavior — they just bloated the set.
    //
    // Using a non-this-context connect would be wrong: when the widget
    // dies, we want the SwipeNavigator's slot to fire. `this` as the
    // context ensures if SwipeNavigator dies first, no slot fires.
    connect(w, &QObject::destroyed, this, [this](QObject *dead) {
        m_filteredWidgets.remove(static_cast<QWidget *>(dead));
    });
    for (QObject *child : w->children())
        if (child->isWidgetType())
            installFilterRecursive(static_cast<QWidget *>(child));
}

bool SwipeNavigator::eventFilter(QObject *obj, QEvent *e)
{
    // Hands-off when a modal dialog is open. SwipeNavigator should never
    // try to interpret swipes inside a SnapComposerDialog, ContactPickerDialog,
    // QrScanDialog, etc. — the dialog handles its own gestures (scroll, tap,
    // swipe-to-dismiss). Without this guard we'd intercept events meant for
    // the dialog and the user would see frozen scrolls or accidental page
    // changes when they thought they were just scrolling within the dialog.
    if (QApplication::activeModalWidget() != nullptr) {
        return QObject::eventFilter(obj, e);
    }

    if (obj != this && obj->isWidgetType()) {
        // V32.34+ FIX (laptop scroll→sub-tab never fired): the registration
        // gate below only lets through widgets in m_filteredWidgets, which is
        // populated via childEvent on the navigator's DIRECT children +
        // their descendants AT ATTACH TIME. Widgets created/added inside a page
        // LATER (the contact list, camera roll, scroll areas built after the
        // page was attached) are NOT registered — so a wheel over them bailed
        // here before ever reaching the wheel handler, and the vertical detent
        // could never fire on a laptop. WHEEL events must bypass this gate: the
        // wheel handler (processWheelEvent) does its own cursor-targeting via
        // wheelAtVerticalLimit, so it doesn't depend on registration. Touch
        // still respects the gate (it relies on the registered-widget set for
        // its gesture lifecycle). Only let Wheel through unconditionally.
        const bool isWheel = (e->type() == QEvent::Wheel);
        if (!isWheel
            && !m_filteredWidgets.contains(static_cast<QWidget *>(obj)))
            return QObject::eventFilter(obj, e);
    }

    if (obj != this && obj->isWidgetType()) {
        const QEvent::Type t = e->type();
        if (t == QEvent::TouchBegin) {
            auto *te = static_cast<QTouchEvent *>(e);
            const auto &pts = te->points();
            if (pts.isEmpty()) return false;

            // V32.32: single-finger lock (filter path mirror of the
            // primary-path TouchBegin guard). If we're already mid-gesture
            // from a previous finger, the new touch is ignored — let
            // Qt's mouse-synthesis path handle it normally. Returning
            // false lets the event continue downstream; we don't try to
            // own it.
            if (m_lockedPointId >= 0) {
                return false;
            }

            if (m_wheelPhase == WheelPhase::Active) {
                if (m_wheelIdleTimer) m_wheelIdleTimer->stop();
                m_wheelPhase = WheelPhase::Idle;
                m_wheelHorizontalLocked = false;
            }

            // V32.32: bump gesture-id + reset axis-lock + record start
            // time so the watchdog can detect stuck states from the
            // filter path too. Mirrors the primary-path TouchBegin.
            ++m_currentGestureId;
            m_axisLock           = AxisLock::None;
            m_gestureStartedAtMs = QDateTime::currentMSecsSinceEpoch();
            // V32.33+ Phase 1 audit: eventFilter uses its own legacy state
            // machine (m_passingToScrollArea, m_swipeLocked) rather than
            // Phase 1's Ambiguous state. But Phase 1 added m_intent and
            // m_affordanceFocus members that are part of the SHARED state.
            // If a previous gesture went through event() and resetGestureState
            // didn't fire for some reason (very unlikely but possible from
            // a hard crash or test setup), these could be stale. Defensively
            // clear them so the watchdog and (future) affordance subscribers
            // don't act on stale references during a filter-path gesture.
            m_intent             = GestureIntent::None;
            m_candidateIntent    = GestureIntent::None;
            m_affordanceFocus    = nullptr;
            qCDebug(lcSwipeNav, "TouchBegin (filter path) gesture-id=%llu",
                    static_cast<unsigned long long>(m_currentGestureId));

            const QEventPoint *active = nullptr;
            for (const auto &p : pts)
                if (p.state() != QEventPoint::Released) { active = &p; break; }
            const QPointF childPos = active ? active->position() : pts.first().position();
            // V32.32: lock in the point id we accepted (filter path).
            m_lockedPointId = active ? active->id() : pts.first().id();
            const QPoint  gp = static_cast<QWidget *>(obj)->mapToGlobal(childPos.toPoint());
            const QPointF mapped = mapFromGlobal(gp);

            m_passingToScrollArea = false;
            m_scrollTarget = nullptr;
            m_skipGestureForArea = nullptr;  // defensive — should already be null from TouchEnd
            const QWidget *w = static_cast<QWidget *>(obj);
            while (w && w != this) {
                if (const auto *sa = qobject_cast<const QAbstractScrollArea *>(w)) {
                    const QScrollBar *vbar = sa->verticalScrollBar();
                    const QScrollBar *hbar = sa->horizontalScrollBar();
                    const bool vHasRange = vbar && vbar->maximum() > vbar->minimum();
                    const bool hHasRange = hbar && hbar->maximum() > hbar->minimum();
                    // CRITICAL: if the scroll area is HORIZONTAL-ONLY (e.g. theme carousel),
                    // do NOT take ownership of the touch. The carousel installs its own
                    // ThumbCarouselFilter which handles touch directly. SwipeNavigator
                    // intercepting here would block that filter from ever seeing the events.
                    // Only intercept for areas with VERTICAL range (lists, settings scroll).
                    if (vHasRange) {
                        m_passingToScrollArea = true;
                        m_scrollTarget = const_cast<QAbstractScrollArea *>(sa);
                    } else if (hHasRange) {
                        // Horizontal-only area — let it handle its own touch.
                        // We mark it so the gestureBegin path doesn't fire either,
                        // which would conflict with the inner filter's drag logic.
                        m_skipGestureForArea = const_cast<QAbstractScrollArea *>(sa);
                    }
                    break;
                }
                w = w->parentWidget();
            }

            // Initialize gesture state but DO NOT claim the touch sequence yet.
            // If the user is tapping a button, we need the TouchBegin to propagate
            // so Qt can synthesize a mouse press for the button.
            // We only intercept once TouchUpdate shows clear horizontal motion.
            m_touchStart   = mapped;
            m_dragDistance = 0.0;
            m_pageWidth    = width();
            m_touchTimer.restart();
            velocityReset();
            velocityRecord(mapped.x());
            // Reset gesture-begin state but don't lock yet — let TouchUpdate decide
            m_swipeLocked = false;
            m_dragging    = false;
            return false; // let the button see the touch — we may still claim it later
        }

        if (t == QEvent::TouchUpdate) {
            auto *te = static_cast<QTouchEvent *>(e);
            const auto &pts = te->points();
            if (pts.isEmpty()) return true;
            // V32.32: filter to locked point — see primary-path comment.
            const QEventPoint *active = nullptr;
            if (m_lockedPointId >= 0) {
                for (const auto &p : pts) {
                    if (p.id() == m_lockedPointId
                     && p.state() != QEventPoint::Released) {
                        active = &p; break;
                    }
                }
            }
            if (!active) return true;
            const QPoint  gp     = static_cast<QWidget *>(obj)->mapToGlobal(active->position().toPoint());
            const QPointF mapped = mapFromGlobal(gp);

            // V32.33+ Phase 1 audit note: eventFilter path doesn't
            // track velocity during the pre-lock wait period. Velocity
            // samples only accumulate after gestureBegin runs (at lock
            // time). This means fast flicks that cross threshold in
            // one event have only 1-2 samples for commit-vs-bounce.
            //
            // Could be improved by either:
            //   a) record velocity here, then preserve it across the
            //      gestureBegin call (gestureBegin currently velocityResets)
            //   b) full migration to Ambiguous-state model (Phase 1.6)
            //
            // Skipped for this audit pass — same fix would naturally
            // happen during Phase 1.6 unification. Filed in TODO/migration
            // doc.

            if (m_passingToScrollArea) {
                const qreal dx = mapped.x() - m_touchStart.x();
                const qreal dy = mapped.y() - m_touchStart.y();
                QScrollBar *vbar = m_scrollTarget ? m_scrollTarget->verticalScrollBar()   : nullptr;
                QScrollBar *hbar = m_scrollTarget ? m_scrollTarget->horizontalScrollBar() : nullptr;
                const bool canScrollV = vbar && vbar->maximum() > vbar->minimum();
                const bool canScrollH = hbar && hbar->maximum() > hbar->minimum();

                // V32.32: axis-lock — see primary-path TouchUpdate comment.
                // Same logic, applied to the eventFilter path so child
                // widgets that receive touch directly also benefit.
                //
                // V32.33+ Phase 1 tuning: use per-axis thresholds so the
                // eventFilter path (which is actually the primary path
                // in practice, since most touches land on child widgets)
                // matches the Ambiguous-resolution responsiveness from
                // the event() path. Horizontal page-swipe locks at 6px;
                // vertical scroll engagement at 14px.
                //
                // The full Ambiguous-state design is NOT yet applied
                // here — this is just threshold alignment. Migrating
                // the eventFilter path to use Ambiguous + affordance
                // signals is a Phase 2+ task (extract shared helpers
                // first, then route both event() and eventFilter
                // through them).
                if (m_axisLock == AxisLock::None) {
                    const qreal absdx = qAbs(dx);
                    const qreal absdy = qAbs(dy);
                    const bool hCrossed = absdx >= static_cast<qreal>(kPageSwipeThresholdPx);
                    const bool vCrossed = absdy >= static_cast<qreal>(kScrollThresholdPx);
                    if (hCrossed || vCrossed) {
                        // If horizontal crossed AND wins the bias → Horizontal.
                        // Else if vertical crossed → Vertical.
                        // Else (horizontal crossed but lost bias) → wait for
                        // vertical to also cross. This matches the bias logic
                        // in resolveAmbiguous.
                        if (hCrossed && absdx * kHorizontalAxisBias > absdy) {
                            m_axisLock = AxisLock::Horizontal;
                        } else if (vCrossed) {
                            m_axisLock = AxisLock::Vertical;
                        }
                        if (m_axisLock != AxisLock::None) {
                            qCDebug(lcSwipeNav, "axis-lock (filter path) "
                                                "gesture-id=%llu → %s dx=%.1f dy=%.1f",
                                    static_cast<unsigned long long>(m_currentGestureId),
                                    m_axisLock == AxisLock::Horizontal ? "H" : "V",
                                    dx, dy);
                        }
                    }
                }

                if (m_axisLock == AxisLock::Horizontal && !canScrollH) {
                    // Locked horizontal, no h-scroll available → page-swipe.
                    m_passingToScrollArea = false;
                    m_scrollTarget = nullptr;
                    gestureBegin(m_touchStart);
                    gestureUpdate(mapped);
                    return true;
                }
                if (m_axisLock == AxisLock::Horizontal && canScrollH) {
                    if (qAbs(dx) > 2) {
                        // V32.34+: edge-detection — same logic as
                        // primary ChildScrollV path. Route past-edge
                        // motion to overscroll, otherwise commit.
                        const int curVal = hbar->value();
                        const bool atMin = curVal == hbar->minimum();
                        const bool atMax = curVal == hbar->maximum();
                        if ((atMin && dx > 0) || (atMax && dx < 0)) {
                            applyOverscroll(dx, 0);
                        } else {
                            // V32.32: paced commit (see primary path).
                            commitScrollPaced(hbar, nullptr, static_cast<int>(dx), 0);
                        }
                        // V32.34+: fling velocity sampling — same as
                        // the primary ChildScrollV path. Two commit
                        // sites for historical reasons (eventFilter
                        // vs main event handler); both need to feed
                        // the velocity buffer or the fling at the
                        // end will be undersized.
                        recordVelocitySample(dx, 0);
                        m_touchStart = mapped;
                        m_scrollCommitted = true;
                    }
                    return true;
                }
                if (m_axisLock == AxisLock::Vertical && canScrollV && qAbs(dy) > 2) {
                    const int curVal = vbar->value();
                    const bool atMin = curVal == vbar->minimum();
                    const bool atMax = curVal == vbar->maximum();
                    if ((atMin && dy > 0) || (atMax && dy < 0)) {
                        applyOverscroll(0, dy);
                    } else {
                        commitScrollPaced(nullptr, vbar, 0, static_cast<int>(dy));
                    }
                    recordVelocitySample(0, dy);
                    m_touchStart = mapped;
                    m_scrollCommitted = true;
                }
                return true;
            }
            // Lazy gesture begin: if not yet locked, only start if motion is large enough.
            // This lets brief taps complete without us interfering.
            //
            // V32.33+ Phase 1 tuning: per-axis thresholds — horizontal
            // 6px (page-swipe responsiveness), vertical 14px (scroll
            // engagement). Matches the resolveAmbiguous logic from
            // event() path, with horizontal-axis-bias protection.
            if (!m_swipeLocked) {
                const qreal dx = qAbs(mapped.x() - m_touchStart.x());
                const qreal dy = qAbs(mapped.y() - m_touchStart.y());
                const bool hCrossed = dx >= static_cast<qreal>(kPageSwipeThresholdPx);
                const bool vCrossed = dy >= static_cast<qreal>(kScrollThresholdPx);
                const bool hWins = hCrossed && dx * kHorizontalAxisBias > dy;
                if (!hWins && !vCrossed)
                    return false; // still might be a tap — let propagation continue
                // Motion crossed threshold — claim the gesture starting from here
                gestureBegin(m_touchStart);
            }
            gestureUpdate(mapped);
            return true;
        }

        if (t == QEvent::TouchEnd || t == QEvent::TouchCancel) {
            auto *te = static_cast<QTouchEvent *>(e);
            const auto &pts = te->points();
            // V32.33+ Phase 1 audit: find the locked point in the
            // points list. Multi-touch end events can deliver points
            // in any order; .first() could be a different finger than
            // our locked gesture-owner. Capture locked id before
            // resetGestureState clears it.
            const qint64 lockedId = m_lockedPointId;
            const bool isSystemCancel = (t == QEvent::TouchCancel);

            // Helper lambda to find the position of the locked point
            // in the event's points list. Returns .first() if no
            // locked id or locked id not present.
            auto findLockedPos = [&pts, lockedId]() -> QPointF {
                if (pts.isEmpty()) return QPointF();
                if (lockedId >= 0) {
                    for (const auto &p : pts) {
                        if (p.id() == lockedId) return p.position();
                    }
                }
                return pts.first().position();
            };

            if (m_passingToScrollArea) {
                const bool didScroll = m_scrollCommitted;
                const QPointF startPos = m_touchStart;  // capture before reset
                // V32.34+: same fling logic as the primary
                // ChildScrollV TouchEnd. Velocity must be sampled
                // BEFORE resetGestureState (which clears m_scrollTarget
                // and our axis lock). The axis-lock-respecting zero of
                // the off-axis velocity is intentional — see primary
                // path comment.
                if (didScroll && !isSystemCancel && m_scrollTarget) {
                    qreal vx = 0.0, vy = 0.0;
                    computeReleaseVelocity(&vx, &vy);
                    if (m_axisLock == AxisLock::Vertical)   vx = 0.0;
                    if (m_axisLock == AxisLock::Horizontal) vy = 0.0;
                    startFling(vx, vy);
                }
                // V32.34+: spring-back if we have overscroll and no
                // fling is running. Same logic as primary TouchEnd.
                if (hasOverscroll()
                    && (!m_flingTimer || !m_flingTimer->isActive())) {
                    startSpringBack();
                }
                m_velocitySamples.clear();
                m_velocityClock.invalidate();
                // V32.32: full reset including m_axisLock + start-time.
                resetGestureState();
                // V32.33+ Phase 1: never fire synthetic tap on TouchCancel.
                // V32.33+ Phase 1 audit: tap fires at the start position
                // (where the user pressed), not the release position.
                // m_touchStart was set in SwipeNavigator-local coords at
                // eventFilter TouchBegin and resetGestureState doesn't
                // clear it, so this is safe.
                if (!didScroll && !isSystemCancel && !pts.isEmpty()) {
                    forwardTap(startPos);
                }
                return true;
            }
            // If we never locked a swipe, this was a tap — don't consume the end event.
            // The button needs to see TouchEnd / synthesized mouse release to fire its click.
            if (!m_swipeLocked) {
                // V32.32: still need to reset axis-lock + start-time for
                // the next gesture, even though we're letting the tap
                // propagate. Without this the watchdog would falsely
                // flag this gesture as stuck after 2.5s.
                resetGestureState();
                return false;
            }
            if (!pts.isEmpty()) {
                const QPointF posLocal = findLockedPos();
                const QPoint  gp     = static_cast<QWidget *>(obj)->mapToGlobal(
                    posLocal.toPoint());
                const QPointF mapped = mapFromGlobal(gp);
                gestureEnd(mapped);
            }
            // V32.32: post-gestureEnd reset clears V32.32 fields the old
            // hand-rolled code didn't touch.
            resetGestureState();
            return true;
        }
    }

    if (e->type() == QEvent::Wheel && obj != this) {
        if (processWheelEvent(static_cast<QWheelEvent *>(e))) return true;
        return false;
    }

    return QWidget::eventFilter(obj, e);
}

void SwipeNavigator::feedWheelVerticalDetent(qreal dy)
{
    // V32.34+ unified nav: feed vertical wheel overflow into the end-of-content
    // sub-tab detent WITHOUT going through applyOverscroll — which early-returns
    // when there's no m_scrollTarget/m_flingTarget (true on the wheel path,
    // since vertical wheel never enters the gesture lifecycle that assigns a
    // target). We accumulate into the SAME m_overscrollRawY the detent reads,
    // and call evaluateVerticalDetent directly. No viewport translation here:
    // the rubber-band visual is owned by the child list's own edge feedback on
    // the wheel path; we only need the detent accumulator. evaluateVerticalDetent
    // zeroes m_overscrollRawY when it fires, and a short idle resets the streak.
    //
    // V32.34+ LOCK: while a recent takeover holds the lock, DON'T accumulate or
    // evaluate — this is what stops the runaway the device log showed (one
    // sustained smooth-scroll delivering dozens of v=20 events in a few ms,
    // building rawY to 4× threshold and re-firing ~1/sec). Keep the accumulator
    // zeroed during the lock so the FIRST post-lock event starts a clean streak
    // rather than inheriting a giant built-up value.
    if (scrollTakeoverLocked()) {
        m_overscrollRawY = 0.0;
        return;
    }
    // V32.34+ REFRESH-RATE NORMALIZATION. Wheel/trackpad events arrive at the
    // display's refresh rate, so a 240Hz panel delivers ~4× as many events per
    // second as 60Hz — and since we accumulated each event's dy equally, the
    // detent built up (and fired) ~4× faster on a high-refresh screen. That's
    // the "fixed unless 60Hz / runs at monitor pace" problem. Normalize by
    // CLAMPING the accumulation RATE to a wall-clock budget: at most
    // kDetentRawPerMs raw units per elapsed millisecond, independent of how
    // many events delivered that motion. A 240Hz burst and a 60Hz trickle of
    // the same physical scroll now reach the threshold in the same wall time.
    constexpr qreal kDetentRawPerMs = 0.75;   // ~220 threshold in ~290ms sustained
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    qreal dtMs = (m_lastDetentFeedMs > 0)
                   ? static_cast<qreal>(nowMs - m_lastDetentFeedMs)
                   : 16.0;                     // assume one 60Hz frame on first feed
    m_lastDetentFeedMs = nowMs;
    if (dtMs <= 0.0) dtMs = 1.0;
    // Allow this event's contribution, but cap its magnitude to the time budget
    // so a flurry of same-frame events can't blast the accumulator. Preserve
    // direction.
    const qreal budget = kDetentRawPerMs * dtMs;
    qreal contrib = dy;
    if (qAbs(contrib) > budget)
        contrib = (contrib > 0 ? budget : -budget);
    m_overscrollRawY += contrib;
    evaluateVerticalDetent(m_overscrollRawY);
}

bool SwipeNavigator::wheelAtVerticalLimit(const QPoint &globalPos, qreal vPx) const
{
    // Walk up from the widget under the cursor looking for a vertically
    // scrollable area. If we find one, we're "at the limit" only if its
    // vertical scrollbar can't move further in the direction this wheel would
    // scroll. If we find NO vertically scrollable area, the content can't
    // absorb the scroll at all → treat as at-limit so the detent can fire.
    // Sign: vPx > 0 → scroll toward TOP (value decreases toward minimum);
    //       vPx < 0 → scroll toward BOTTOM (value increases toward maximum).
    const QWidget *leaf = QApplication::widgetAt(globalPos);
    const QWidget *w = leaf;
    while (w && w != this) {
        if (const auto *sa = qobject_cast<const QAbstractScrollArea *>(w)) {
            const QScrollBar *vbar = sa->verticalScrollBar();
            const bool hasV = vbar && vbar->maximum() > vbar->minimum();
            if (hasV) {
                if (vPx > 0)  return vbar->value() <= vbar->minimum();  // at top
                if (vPx < 0)  return vbar->value() >= vbar->maximum();  // at bottom
                return false;
            }
        }
        w = w->parentWidget();
    }
    return true;   // no v-scrollable area under the cursor → nothing to absorb
}

bool SwipeNavigator::processWheelEvent(QWheelEvent *e)
{
    const QPoint pxDelta  = e->pixelDelta();
    const QPoint angDelta = e->angleDelta();

    qreal hPx, vPx;
    bool fromTrackpad;
    if (!pxDelta.isNull()) {
        hPx = static_cast<qreal>(pxDelta.x());
        vPx = static_cast<qreal>(pxDelta.y());
        fromTrackpad = true;
    } else {
        hPx = static_cast<qreal>(angDelta.x()) * kAngleDeltaToPx;
        vPx = static_cast<qreal>(angDelta.y()) * kAngleDeltaToPx;
        fromTrackpad = false;
    }
    // V32.34+ PER-SOURCE direction normalization. The mouse wheel (angleDelta)
    // and trackpad (pixelDelta) can have OPPOSITE physical→sign relationships,
    // because natural/reverse scrolling typically inverts the trackpad but not
    // the wheel (or the reverse, per OS config). A single global invert flag
    // therefore can't satisfy both — fixing the trackpad re-breaks the mouse,
    // which is exactly the user's "mouse still reversed" report. So we flip the
    // sign HERE, at the source, per device, using two independent settings:
    //   ui/scroll_detent_invert        → trackpad (the original toggle)
    //   ui/scroll_detent_invert_mouse  → mouse wheel
    // After this, downstream sees a single consistent convention and the detent
    // direction code stays simple.
    const bool invertThis = fromTrackpad ? m_detentInvert : m_detentInvertMouse;
    if (invertThis) { vPx = -vPx; hPx = -hPx; }

    const Qt::ScrollPhase phase = e->phase();
    if (phase == Qt::ScrollMomentum) return false;

    const bool isBegin = (phase == Qt::ScrollBegin)
                      || (phase == Qt::NoScrollPhase && m_wheelPhase == WheelPhase::Idle);
    const bool isEnd   = (phase == Qt::ScrollEnd);

    // V32.34+ diagnostic (laptop scroll→sub-tab): one line per wheel-begin when
    // unified nav is on, showing the deltas + whether we're treating it as a
    // vertical-detent candidate. If this NEVER logs while scrolling, the wheel
    // isn't reaching here (routing). If it logs but "atLimit=0" forever, the
    // scroll area isn't reporting its edge. Either way it localizes the break.
    if (isBegin && unifiedNavEnabled() && qAbs(vPx) >= qAbs(hPx)) {
        const bool atLimit = currentSubTabbed()
            && wheelAtVerticalLimit(e->globalPosition().toPoint(), vPx);
        qCInfo(lcSwipeNav, "[WHEEL] v=%.0f h=%.0f subTabbed=%d atVLimit=%d",
               vPx, hPx, currentSubTabbed() ? 1 : 0, atLimit ? 1 : 0);
    }

    // Trackpad direction locking is more lenient than touch (0.50 vs 0.65).
    // Trackpads are harder to keep perfectly horizontal than a fingertip.
    constexpr qreal kWheelDirectionRatio = 0.50;

    if (isBegin) {
        if (qAbs(hPx) < 0.5 && qAbs(vPx) < 0.5) {
            if (phase == Qt::ScrollBegin) { m_wheelPhase = WheelPhase::Pending; return false; }
            return false;
        }
        // V32.34+ unified nav: VERTICAL wheel feeds the end-of-content sub-tab
        // detent. Previously this handler bailed on any vertical-dominant wheel
        // (return false at the direction check), so on a laptop "scroll to the
        // end, keep scrolling → advance sub-tab" NEVER fired — the vertical
        // detent path (applyOverscroll → evaluateVerticalDetent) was simply
        // unreachable via wheel/trackpad. Now: if the wheel is vertical-dominant
        // and the scroll area under the cursor is AT its vertical limit in the
        // scroll direction (or there's no v-scrollable area at all), we feed the
        // vertical overflow to applyOverscroll, which arms/fires the detent.
        // Only when unifiedNavEnabled() — otherwise preserve the old bail so a
        // plain build's vertical wheel goes untouched to the child.
        if (qAbs(hPx) < qAbs(vPx) * kWheelDirectionRatio) {
            if (unifiedNavEnabled() && qAbs(vPx) >= 0.5
                && currentSubTabbed()
                && wheelAtVerticalLimit(e->globalPosition().toPoint(), vPx)) {
                feedWheelVerticalDetent(vPx);   // accumulate + fire detent
                // Don't consume: let the child also process so the list shows
                // its own edge feedback; we only piggyback the detent.
                return false;
            }
            return false;   // vertical, not at limit (or flag off) → child scrolls
        }
        if (isHorizontalScrollSuppressed(e->globalPosition().toPoint())) return false;

        m_wheelHorizontalLocked = true;
        m_wheelOrigin   = QPointF(width() / 2.0, height() / 2.0);
        m_wheelAccum    = 0.0;
        m_wheelSmoothed = 0.0; // reset EMA on new gesture
        m_wheelPhase    = WheelPhase::Active;
        gestureBegin(m_wheelOrigin);

        if (!m_wheelIdleTimer) {
            m_wheelIdleTimer = new QTimer(this);
            m_wheelIdleTimer->setSingleShot(true);
            connect(m_wheelIdleTimer, &QTimer::timeout, this, [this]() {
                if (m_wheelPhase == WheelPhase::Active) {
                    m_wheelPhase = WheelPhase::Idle;
                    m_wheelHorizontalLocked = false;
                    gestureEnd(QPointF(m_wheelOrigin.x() + m_wheelAccum, m_wheelOrigin.y()));
                }
            });
        }
        m_wheelIdleTimer->start(phase == Qt::NoScrollPhase ? kWheelIdleMs : kWheelIdleMs * 3);

    } else {
        if (m_wheelPhase == WheelPhase::Pending) {
            if (qAbs(hPx) < 0.5 && qAbs(vPx) < 0.5) return false;
            if (qAbs(hPx) < qAbs(vPx) * kWheelDirectionRatio) {
                // V32.34+ unified nav: vertical-dominant continuation → same
                // end-of-content detent feed as the isBegin branch.
                if (unifiedNavEnabled() && qAbs(vPx) >= 0.5
                    && currentSubTabbed()
                    && wheelAtVerticalLimit(e->globalPosition().toPoint(), vPx)) {
                    feedWheelVerticalDetent(vPx);
                }
                m_wheelPhase = WheelPhase::Idle; return false;
            }
            m_wheelHorizontalLocked = true;
            m_wheelOrigin   = QPointF(width() / 2.0, height() / 2.0);
            m_wheelAccum    = 0.0;
            m_wheelSmoothed = 0.0;
            m_wheelPhase    = WheelPhase::Active;
            gestureBegin(m_wheelOrigin);
            if (m_wheelIdleTimer) m_wheelIdleTimer->start(kWheelIdleMs);
        } else {
            if (!m_wheelHorizontalLocked) return false;
            if (m_wheelPhase == WheelPhase::Active && m_wheelIdleTimer)
                m_wheelIdleTimer->start(phase == Qt::NoScrollPhase ? kWheelIdleMs : kWheelIdleMs * 3);
        }
    }

    if (m_wheelPhase != WheelPhase::Active) return false;

    m_wheelAccum += hPx;

    // KEY FIX: Scale accumulated pixels to a fraction of page width.
    // Raw pixel deltas from a trackpad are the same physical count regardless
    // of screen resolution. Without scaling, the 22% commit threshold requires
    // ~86px on a 390px phone but ~317px on a 1440px desktop — same gesture
    // feels completely different. kWheelPageScale = 200px of trackpad = one page.
    // This makes the trackpad feel exactly like a finger swipe on any screen size.
    const int pageW = (m_pageWidth > 0) ? m_pageWidth : width();
    const qreal scaledDrag = (m_wheelAccum / kWheelPageScale) * static_cast<qreal>(pageW);
    const QPointF synthPos(m_wheelOrigin.x() + scaledDrag, m_wheelOrigin.y());

    // Record velocity for flick detection — same as touch path.
    // Without this, velocityCompute() returns 0 for all trackpad gestures
    // and the flick threshold never fires, making fast swipes feel sluggish.
    velocityRecord(synthPos.x());

    gestureUpdate(synthPos);

    if (isEnd) {
        if (m_wheelIdleTimer) m_wheelIdleTimer->stop();
        m_wheelPhase = WheelPhase::Idle;
        m_wheelHorizontalLocked = false;
        gestureEnd(synthPos);
    }
    return true;
}

void SwipeNavigator::wheelEvent(QWheelEvent *e)
{
    if (processWheelEvent(e)) e->accept();
    else QWidget::wheelEvent(e);
}

} // namespace SnapClone::UI
