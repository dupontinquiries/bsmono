#pragma once

// ─── UI/SwipeNavigator.hpp ────────────────────────────────────────────────────
//
// Horizontal swipe navigation with momentum-based physics.
//
// Physics model:
//   • Rubber-band at edges: sqrt(|drag| * pageW * kRubberBandK) — sub-linear,
//     scales with page width so feel is screen-size-independent.
//   • Commit threshold: adaptive — distance OR velocity.
//     Distance: |drag| > pageW * kCommitFraction  (22% of page width)
//     Velocity: |v| > kFlickThresholdPxMs AND |drag| > kFlickMinPx
//   • Momentum animation: duration scales with release velocity so a fast
//     flick commits quickly and a slow drag settles more gently.
//   • Bounce spring: critically damped, duration proportional to pull depth.
//
// Width independence:
//   m_pageWidth is captured at gestureBegin() and never updated mid-gesture.
//   All thresholds are fractions of m_pageWidth, not absolute pixels.
//   This makes the feel identical on a 390px phone and a 1280px desktop.
//
// Velocity sampling:
//   Ring buffer of 24 samples, 120ms window.
//   Weighted toward recent samples (exponential decay weight).
//   Window is wider than the old 80ms to correctly detect slow deliberate swipes.

#include <QWidget>
#include <QPropertyAnimation>
#include <QElapsedTimer>
#include <QList>
#include <functional>
#include <QSet>
#include <QTimer>
#include <QPointer>
#include <QWheelEvent>
#include <QChildEvent>
#include "ui/CoalescingDebouncer.hpp"   // V32.34+: page-transition debounce
#include "ui/SubPageDetent.hpp"         // V32.39+: tested sub-page detent curve

class QAbstractScrollArea;
class QScrollBar;

namespace SnapClone::UI {

class SubTabbed;  // V32.34+: two-level nav interface (currentSubTabbed bridge)

class SwipeNavigator : public QWidget
{
    Q_OBJECT
    Q_PROPERTY(qreal slideOffset READ slideOffset WRITE setSlideOffset)

public:
    explicit SwipeNavigator(QWidget *parent = nullptr);
    ~SwipeNavigator() override;

    void addPage(QWidget *page, const QString &name);
    /// Navigate to a page. `animate` slides; otherwise it's instant.
    /// V32.34+ `resetSubTab`: when true AND unified nav is on, the destination
    /// page's sub-tab is reset to its first (0) — the "land on a new page's
    /// first sub-tab" feel for USER navigation (bar arrows, keyboard, tab tap).
    /// PROGRAMMATIC jumps (startup default, "go to inbox after send", etc.)
    /// pass false (the default) so they preserve the page's current/restored
    /// sub-tab instead of slamming it to 0 — which previously fought e.g. the
    /// Inbox's persisted-filter restore at launch.
    void goToPage(int index, bool animate = true, bool resetSubTab = false);

    [[nodiscard]] int      currentIndex() const;
    [[nodiscard]] QWidget *currentPage()  const;
    [[nodiscard]] int      pageCount()    const;
    /// V32.34+: display name a page was added with (for UnifiedNavBar's label).
    [[nodiscard]] QString  pageName(int index) const;

    /// V32.34+ two-level unified navigation gate (SC_UNIFIED_NAV=1 env or
    /// QSettings ui/unified_nav). PUBLIC because MainWindow consults it to
    /// decide whether to mount the UnifiedNavBar in place of the TabBar. When
    /// false the navigator's two-level gesture/scroll logic is fully inert and
    /// behavior is byte-for-byte the pre-existing page-swipe + scroll. Cached
    /// once in a function-local static (teardown-safe: a plain bool).
    [[nodiscard]] static bool unifiedNavEnabled();

    // ── V32.34+ two-level navigation: SubTabbed bridge ───────────────────
    // The unified bar and the (forthcoming) two-level gesture state machine
    // ask the navigator about the CURRENT page's sub-tabs through these.
    // They delegate to the page's SubTabbed interface via dynamic_cast, so a
    // page that doesn't implement it (Camera, Wayfinder) transparently
    // reports "no sub-tabs" (count 0) and the two-level logic degrades to
    // flat page paging. Pure delegation — no gesture timing here.
    [[nodiscard]] SubTabbed *currentSubTabbed() const;  ///< null if current page has none
    [[nodiscard]] int  currentSubTabCount() const;          ///< 0 if none
    [[nodiscard]] int  currentSubTabIndex() const;          ///< 0 if none
    /// Advance the current page's sub-tab by ±1 WITHOUT wrapping, returning
    /// true if it moved. Returns false at an edge (caller then knows it's at
    /// the cycle boundary → the page-break detent applies). Used by the
    /// gesture/keyboard layer's shared advance entry point.
    bool advanceSubTab(int delta);
    /// Like advanceSubTab but CYCLES (wraps) at the edges, returning the
    /// boundary-crossing direction: 0 = moved within range, -1 = wrapped off
    /// the start, +1 = wrapped off the end. Lets the caller fire the
    /// page-break detent exactly when a wrap happens.
    int cycleSubTab(int delta);

    // ── V32.33: Top-down gesture intent classification ──────────────────
    //
    // Public so the affordance signals (declared below) can use the enum
    // as an argument type — moc requires signal argument types be visible
    // from at least the same access scope as the signal itself.
    //
    // V32.33+ Phase 1 added Ambiguous and TapCommit. See
    // docs/GESTURE_AFFORDANCE_MIGRATION.md for the migration plan.
    //
    // Ordering inside classifyIntent (defined in .cpp):
    //   1. Interactive widget (button, line edit, slider...) → PassToChild
    //   2. swipenav-passthrough or swipenav-tappable property → PassToChild
    //   3. Scroll area with vertical room → ChildScrollV
    //   4. Scroll area with only horizontal room (carousel) → ChildScrollH
    //   5. Default → PageSwipe (SwipeNavigator owns the gesture)
    //
    // V32.33+ Phase 1: a fresh TouchBegin enters Ambiguous instead of
    // committing directly to ChildScrollV/PageSwipe. The candidate is
    // remembered and used during resolveAmbiguous() to decide which
    // resolved intent to pick.
    enum class GestureIntent {
        None,           // no active gesture
        PassToChild,    // interactive widget under touch — Qt mouse-synth handles
        Ambiguous,      // V32.33+ Phase 1: touch in flight, no commit yet
        ChildScrollV,   // scrollable ancestor with vertical room (resolved)
        ChildScrollH,   // scrollable ancestor with horizontal-only room (carousel)
        PageSwipe,      // no child claim — SwipeNavigator owns it (resolved)
        TapCommit,      // resolved as a tap, click forwarded to focus widget
    };
    Q_ENUM(GestureIntent)

signals:
    void pageChanged(int index);

    /// V32.34+ DEBOUNCE LAYER SEPARATION. `pageChanged` fans out into both
    /// instant UI sync AND expensive/consequential work (network-priority
    /// changes, usage stats, celebrations). During a fast multi-page swipe that
    /// fires repeatedly and spams the underlying system for pages nobody settled
    /// on. These two signals split the concern (see snapclone-debounce):
    ///
    ///   • pageAnimating(index) — fires on EVERY transition's LEADING edge,
    ///     immediately. Drive only lightweight, responsive UI here (tab-bar
    ///     highlight) so navigation always feels instant.
    ///   • pageSettled(index)   — fires ONCE after swiping quiets, coalesced to
    ///     the FINAL page. Drive the expensive/stateful work here so it runs for
    ///     the page the user actually landed on, not every intermediate.
    ///
    /// `pageChanged` is retained (emitted alongside pageAnimating) for existing
    /// consumers and back-compat; new consequential consumers should migrate to
    /// pageSettled. A consumer that wants the old every-transition behavior can
    /// keep using pageChanged.
    void pageAnimating(int index);
    void pageSettled(int index);

    /// V32.34+ unified nav: the current page's active sub-tab changed (via a
    /// gesture/keyboard detent or a page-change reset). `subIndex` is the new
    /// sub-tab index within the current page. Lets a unified bar update its
    /// highlight without polling. NOT emitted for pages without sub-tabs.
    void subTabChanged(int subIndex);

    /// V32.34+ unified nav: IN-FLIGHT horizontal drag progress toward the
    /// neighbouring sub-tab, so a unified bar can preview the pending move
    /// without polling. `dir` is +1 (dragging toward the NEXT sub-tab) or -1
    /// (toward the PREVIOUS); `progress` is 0..1 toward the sub-tab COMMIT
    /// point (not the page-break — the bar previews the sub-tab move, which is
    /// the common case). Emitted only while a sub-tab-scrub drag is in flight
    /// on a SubTabbed page; a final emit of progress 0 on gesture end tells the
    /// bar to drop the preview. The bar should render this CHEAPLY (a paintEvent
    /// indicator), never via per-frame setStyleSheet.
    void subTabDragProgress(int dir, qreal progress);

    // ── V32.33+ Phase 1: gesture-affordance signals ──────────────────────
    //
    // Fired during the Ambiguous state so widgets can show consider-state
    // visuals BEFORE the gesture commits to a concrete intent.
    //
    // Subscribed by: nothing yet in Phase 1 — this is wire-up. Phase 2
    // adds the first widget subscriber. See docs/GESTURE_AFFORDANCE_MIGRATION.md
    // for the full plan.

    /// Fired when a touch enters Ambiguous state. `focusWidget` is the
    /// widget under the original touch point (typically a leaf widget).
    /// Widgets that want to show consider-state animations subscribe to
    /// this. May be nullptr if no widget was under the touch.
    void gestureAffordanceShown(QWidget *focusWidget, const QPoint &globalPos);

    /// Fired when a previously-Ambiguous gesture commits to a concrete
    /// intent (ChildScrollV, ChildScrollH, PageSwipe, or TapCommit).
    /// `focusWidget` receives this so it can transition from consider-state
    /// to either idle (intent != TapCommit) or active (intent == TapCommit).
    void gestureAffordanceResolved(QWidget *focusWidget, GestureIntent finalIntent);

    /// Fired when an Ambiguous gesture ends WITHOUT committing to a
    /// concrete intent — e.g. user lifted finger after movement that
    /// exceeded tap threshold but didn't cross axis-lock threshold.
    /// `focusWidget` returns to idle without showing active state.
    void gestureAffordanceCancelled(QWidget *focusWidget);

protected:
    bool event         (QEvent *e) override;
    void mousePressEvent  (QMouseEvent *e) override;
    void mouseMoveEvent   (QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void keyPressEvent (QKeyEvent   *e) override;
    void resizeEvent   (QResizeEvent *e) override;
    void paintEvent    (QPaintEvent *e)  override;
    void wheelEvent    (QWheelEvent *e)  override;
    void childEvent    (QChildEvent *e)  override;
    bool eventFilter   (QObject *o, QEvent *e) override;

private:
    [[nodiscard]] qreal slideOffset() const { return m_slideOffset; }
    void setSlideOffset(qreal offset);

    void layoutPages();

    // Physics-based animation primitives
    void startCommitAnimation(bool toNext, qreal releaseVelocityPxMs);
    void startBounceAnimation(qreal currentOffset, qreal releaseVelocityPxMs);

    void forwardTap(const QPointF &pos);
    void connectAnimFinished(std::function<void()> fn);

    // Gesture state machine
    void gestureBegin (const QPointF &pos);
    bool gestureUpdate(const QPointF &pos);
    bool gestureEnd   (const QPointF &pos);

    // Rubber-band: sub-linear resistance at edges
    // offset = sign * sqrt(|drag| * pageW * k)
    // k scales the resistance — higher = stiffer
    qreal rubberBand(qreal drag, int pageW) const;

    QList<QWidget *>        m_pages;
    QStringList             m_pageNames;  // V32.34+: parallel to m_pages, for UnifiedNavBar
    int                     m_currentIndex        = 0;

    // V32.34+ DEBOUNCE LAYER. All page-transition emits funnel through
    // notifyPageTransition(), which (a) emits pageChanged + pageAnimating
    // immediately for instant UI, and (b) feeds a coalescing debouncer that
    // emits pageSettled ONCE after swiping quiets, for the expensive work. One
    // repeating timer drives the trailing edge; a monotonic clock supplies the
    // debouncer's times. See include/ui/CoalescingDebouncer.hpp.
    void notifyPageTransition(int index);
    CoalescingDebouncer<int> m_pageDebounce{DebounceConfig::pageTransition()};
    QTimer                  *m_pageSettleTimer    = nullptr;  ///< owned, lazy
    QElapsedTimer            m_pageDebounceClock;              ///< monotonic ms source
    qreal                   m_slideOffset         = 0.0;
    int                     m_pageWidth           = 0;
    bool                    m_dragging            = false;
    bool                    m_swipeLocked         = false;
    bool                    m_passingToScrollArea = false;
    bool                    m_scrollCommitted     = false;
    QPointer<QAbstractScrollArea> m_scrollTarget;
    QPointer<QAbstractScrollArea> m_skipGestureForArea; // horizontal-only — let inner filter handle it
    QPointF                 m_touchStart;
    QElapsedTimer           m_touchTimer;
    qreal                   m_dragDistance        = 0.0;
    QPropertyAnimation     *m_anim               = nullptr;
    QMetaObject::Connection m_animConn;

    // ── V32.33: gesture intent state (enum is public — see top of class) ─
    //
    // Previously: TouchBegin made a partial decision ("is leaf
    // interactive? if so bail; else check for scroll-area parent and set
    // passthrough flag; else page-swipe"), and TouchUpdate filled in the
    // rest ("axis lock then maybe fall through to page-swipe"). That
    // two-stage decision could reclassify mid-stroke and caused jitter.
    //
    // Now: classifyIntent() at TouchBegin walks the widget chain
    // leaf→root once and picks ONE intent that persists for the whole
    // gesture. TouchUpdate dispatches on intent without reclassifying.
    // TouchEnd cleans up per-intent. No mid-stroke flips.
    //
    // V32.33+ Phase 1: Ambiguous is the new entry-state. Touch begins in
    // Ambiguous; the gesture resolves to a concrete intent based on
    // motion crossing thresholds OR on release. During Ambiguous, no
    // behavior commits — but affordance signals are emitted so widgets
    // CAN show consider-state visuals. See docs/GESTURE_AFFORDANCE_MIGRATION.md
    // for the full design.
    GestureIntent m_intent = GestureIntent::None;

    /// V32.33+ Phase 1: the intent CANDIDATE chosen by classifyIntent
    /// at TouchBegin. While m_intent == Ambiguous, we use m_candidateIntent
    /// to know whether motion past the H-threshold should resolve to
    /// PageSwipe (default) or stay-in-place (e.g. the candidate was
    /// ChildScrollH and we should let the inner filter handle it).
    GestureIntent m_candidateIntent = GestureIntent::None;

    /// V32.33+ Phase 1: the focus widget for affordance signals. Captured
    /// at TouchBegin from QApplication::widgetAt. Used as the argument
    /// to gestureAffordanceShown / Resolved / Cancelled signals.
    /// QPointer-safe in case the widget is destroyed during the gesture.
    QPointer<QWidget> m_affordanceFocus;

    /// Walk the widget chain from `leaf` (typically QApplication::widgetAt
    /// at the touch position) up to `this`, returning the first matching
    /// intent. If `scrollAreaOut` is non-null and the result is
    /// ChildScrollV or ChildScrollH, it receives the scroll area that
    /// matched.
    GestureIntent classifyIntent(const QWidget *leaf,
                                 QAbstractScrollArea **scrollAreaOut) const;

    /// V32.33+ Phase 1: resolve a currently-Ambiguous gesture to a
    /// concrete intent based on accumulated motion. Called from
    /// TouchUpdate when motion crosses kLockThresholdPx. Returns the
    /// resolved intent (one of ChildScrollV, ChildScrollH, PageSwipe).
    /// Emits gestureAffordanceResolved signal.
    GestureIntent resolveAmbiguous(qreal dx, qreal dy);

    /// V32.33+ Phase 1: end the current Ambiguous gesture as either a
    /// TapCommit (small movement, treated as a click) or a Cancel
    /// (movement exceeded tap threshold but didn't cross axis-lock).
    /// Emits gestureAffordanceResolved or gestureAffordanceCancelled.
    void endAmbiguous(const QPointF &endPos, bool wasTap);

    // V32.33+ Phase 1 timing constants. See migration doc for the
    // rationale on these defaults.
    //
    // kTapMaxMovementPx was 10 initially but raised to 16 because the
    // 10-13px range (motion didn't reach axis-lock thresholds, but
    // moved more than 10px) was producing Cancel where users had
    // jittered during a deliberate tap. 16 is at the upper end of
    // mobile platform tap-jitter tolerances (iOS ~10pt, Android ~8dp,
    // both close to 14-16 device px on typical phones) and catches
    // genuine taps without misinterpreting deliberate swipes.
    static constexpr int kTapMaxMovementPx     = 16;  // <= this = tap, even in Ambiguous
    static constexpr int kAmbiguousTimeoutMs   = 200; // hold without motion → long-press (TBD Phase 4)


    // ── V32.32: per-gesture identity and axis-locking ────────────────────
    //
    // Two related state items that fix the "swipe gets cancelled mid-stroke"
    // and "stuck mid-swipe" bugs:
    //
    // m_currentGestureId: monotonic counter, bumped on each TouchBegin.
    //   Useful when an async callback fires after a gesture has ended and
    //   another has begun — the callback can compare gesture-ids to know
    //   whether its decision is still relevant. Also used as the registry
    //   key for the stuck-gesture watchdog below.
    //
    // m_axisLock: once the user has moved more than kLockThresholdPx in
    //   either x or y, we commit to that axis for the REMAINDER of the
    //   touch. No flipping. Within a ChildScrollV intent, this is still
    //   used to distinguish H-scroll vs V-scroll inside the scroll area.
    //
    // Both are reset by resetGestureState() in TouchEnd/TouchCancel and by
    // the watchdog when a stuck gesture is force-cleared.
    enum class AxisLock { None, Horizontal, Vertical };
    quint64                 m_currentGestureId    = 0;
    AxisLock                m_axisLock            = AxisLock::None;
    qint64                  m_gestureStartedAtMs  = 0;
    QTimer                 *m_gestureWatchdog     = nullptr;

    // V32.32: single-finger lock.
    //
    // When multiple fingers land near-simultaneously, each generates its
    // own QEventPoint with a unique id(). Before this lock, the FSM
    // looked for "the first non-released point" on every TouchUpdate,
    // which could oscillate between finger 1 and finger 2's contacts,
    // and TouchBegin re-fired the full gesture init for the second
    // finger, causing duplicate page-swipes / duplicate dialog opens.
    //
    // m_lockedPointId stores the QEventPoint::id() that owns the active
    // gesture (-1 when no gesture is active). Subsequent touches during
    // an in-flight gesture are IGNORED — neither extends the active
    // gesture nor starts a new one. The gesture finalizes only when
    // the locked point reaches state Released.
    //
    // We don't bother locking to a SECOND finger after the first lifts
    // mid-gesture — that's a "user changed their mind" pattern and the
    // gesture cleanup is fine on its own.
    qint64                  m_lockedPointId       = -1;

    /// Centralized state cleanup for end-of-gesture. Called from TouchEnd,
    /// TouchCancel, and the watchdog's force-reset path. Resets m_axisLock,
    /// m_passingToScrollArea, m_scrollTarget, m_scrollCommitted,
    /// m_swipeLocked, m_dragging, and stops the watchdog. Does NOT touch
    /// m_slideOffset / animations — those are owned by the commit/cancel
    /// path which decides how to settle the visible position.
    void resetGestureState();

    /// Periodic watchdog (1000ms) that detects "stuck" gestures — a
    /// TouchBegin arrived but TouchEnd/TouchCancel never did within
    /// kStuckGestureMs. Logs and force-resets state so subsequent
    /// touches aren't ignored.
    void onGestureWatchdogTick();

    static constexpr int kGestureWatchdogIntervalMs = 1000;
    static constexpr int kStuckGestureMs            = 2500;

    // ── V32.32: paced scroll passthrough ─────────────────────────────────
    //
    // On the FP5 (and similar Linux-mobile devices) the touchscreen polls
    // at ~120Hz while the display refreshes at 60Hz. When we forward
    // TouchUpdate motion into a child scroll area via vbar->setValue() on
    // every event, we ask Qt to paint 120 times per second — and the
    // compositor can only show 60. The kernel cpufreq governor sees a
    // backed-up paint queue and ramps up cores trying to catch up, which
    // heats the device and adds latency to everything else.
    //
    // commitScrollPaced() coalesces these per-event setValue() calls into
    // ~60Hz commits using m_scrollPacingTimer + m_pendingScrollDx/Dy
    // accumulators. The visual motion is identical (we just commit the
    // accumulated delta less often); CPU/power load drops noticeably.
    QElapsedTimer m_scrollPacingTimer;
    int           m_pendingScrollDx  = 0;
    int           m_pendingScrollDy  = 0;
    static constexpr int kScrollPacingMs = 16; // ~60Hz cap

    // ── V32.34+: kinetic fling for ChildScrollV ──────────────────────────
    //
    // The pre-rewrite swipe model relied on Qt's QScroller for scroll
    // input, which handled momentum scrolling natively. The current
    // SwipeNavigator design intercepts touch events at the application
    // event filter, synthesizes setValue() calls on the child scroll
    // bar, and stops dead when the finger lifts. That's correct for the
    // intent-classification machinery (no mid-gesture reclassification
    // possible), but loses the fling-coast that makes scrolling FEEL
    // smooth.
    //
    // The fix: track velocity during TouchUpdate, and on TouchEnd —
    // when the resolved intent is ChildScrollV — start an exponential-
    // decay animation that continues moving the scroll bar at the
    // last-measured velocity, decaying over ~325ms-tau (Material-ish).
    //
    // Architectural fit:
    //   • The fling animation is OWNED by the navigator, parented to
    //     the scroll target. That's the same pattern as the pacing
    //     buffer — navigator owns scroll-flow, target just receives.
    //   • Any new TouchBegin cancels the fling. The cancellation is
    //     synchronous (timer->stop()) before the new gesture's
    //     classification runs, so a user grabbing mid-fling sees the
    //     fling stop and their drag take over instantly.
    //   • Fling cannot reclassify to PageSwipe — the animation just
    //     moves a scroll bar. No event interpretation happens during
    //     decay. This was the deliberate property of the rewrite that
    //     made fling SAFE to add (in the pre-rewrite world, kinetic
    //     motion could trigger a page-swipe accidentally on bad
    //     QScroller config).
    //
    // Velocity tracking uses a small ring of (time, deltaY) samples
    // collected during the live drag. Samples older than kVelocityWindowMs
    // (80ms) are discarded before averaging — keeps the velocity
    // measurement tied to the user's MOST RECENT motion, not a
    // session-wide running mean. Short window means: pause briefly at
    // the end of a scroll → no fling (correct, that's a deliberate
    // stop); flick at the end of a scroll → strong fling.
    struct VelocitySample {
        qint64 timeMs;  ///< monotonic ms relative to gesture start (or 0-based)
        qreal  deltaY;  ///< pixels of motion since previous sample
        qreal  deltaX;  ///< horizontal too — supports H-axis fling for ChildScrollH
    };
    QList<VelocitySample>  m_velocitySamples;
    QElapsedTimer          m_velocityClock;
    QTimer                *m_flingTimer       = nullptr;  ///< owned, lazy-created
    qreal                  m_flingVelocityX   = 0.0;      ///< pixels per millisecond
    qreal                  m_flingVelocityY   = 0.0;
    QPointer<QAbstractScrollArea> m_flingTarget;          ///< nulls automatically
    QElapsedTimer          m_flingLastTick;               ///< for elapsed-since-tick math

    static constexpr int   kVelocityWindowMs       = 80;   ///< discard samples older than this on TouchEnd
    static constexpr qreal kMinFlingVelocity       = 0.3;  ///< px/ms = 300px/sec; below this we just stop
    static constexpr qreal kFlingStopVelocity      = 0.05; ///< px/ms = 50px/sec; decay below this halts the timer
    static constexpr int   kFlingTickIntervalMs    = 16;   ///< 60Hz; same cadence as the pacing cap
    static constexpr qreal kFlingTauMs             = 325.0; ///< exp-decay time constant; reach ~37% in 325ms, ~5% in 1s

    // ── V32.34+: velocity boost on consecutive flings ────────────────────
    //
    // Material/Android pattern: if the user flicks again shortly after
    // a previous fling, the second fling's velocity is the sum of (this
    // flick) + (the residual previous-fling velocity), letting them
    // build up speed. This is why fast scrolling through a long list
    // feels EFFORTLESS on Android — you don't have to wait between
    // flicks for the decay to finish.
    //
    // Constraints:
    //   • Same direction only — flinging up then immediately down is the
    //     user REVERSING, not boosting. Sign of (vx, vy) must match.
    //   • Recent only — the boost window is kBoostWindowMs (250ms);
    //     beyond that the previous flick was a separate motion.
    //   • Capped — without a cap, mashing flicks could produce
    //     20px/ms (= 20000 px/sec) which is unusable. Cap at
    //     kMaxFlingVelocity, which scales to ~8 screen-heights per
    //     second on a 1080p phone.
    //
    // Storage: on each cancelFling() (the only way a fling exits early
    // and could be "renewed"), the current velocity + timestamp are
    // stashed into m_lastFling*. On the next startFling(), if the
    // stash is recent enough and same-direction, the velocities are
    // added before the threshold check and decay begin.
    qreal         m_lastFlingVelocityX   = 0.0;
    qreal         m_lastFlingVelocityY   = 0.0;
    qint64        m_lastFlingEndTimeMs   = 0;  ///< QDateTime::currentMSecsSinceEpoch() at cancellation
    static constexpr int   kBoostWindowMs    = 250;  ///< how recent the previous fling must have been
    static constexpr qreal kMaxFlingVelocity = 5.0;  ///< px/ms = 5000 px/sec cap

    // ── V32.34+: overscroll bounce ───────────────────────────────────────
    //
    // When drag or fling continues past the scrollbar's max/min, the
    // user wants visual feedback that they've reached the end — and
    // a spring-back when they release. We achieve this by translating
    // the scroll area's VIEWPORT widget by a rubber-band-curved amount
    // beyond what the scrollbar can express.
    //
    // Why translate the viewport (not the content widget): the viewport
    // is the visible window into the scroll area's content. Moving it
    // visually shifts everything inside, INCLUDING the scrollbars'
    // perceived position (the bar stays at max/min). The content
    // widget itself stays anchored to where the scroll area placed it.
    //
    // Rubber-band curve: offset = sign × sqrt(|raw| × viewport × k).
    // Sub-linear resistance: pull more, gain less.
    //   At 10px raw on a 800px-tall viewport: sqrt(10 × 800 × 0.4) = 57px
    //   At 100px raw:                          sqrt(100 × 800 × 0.4) = 178px
    //   At 1000px raw:                         sqrt(1000 × 800 × 0.4) = 566px
    // Self-limiting — gets exponentially harder to pull further.
    //
    // Spring-back: when overscroll is non-zero and we either:
    //   • finish a fling tick at the edge, OR
    //   • see TouchEnd
    // ...we start a separate timer that decays the overscroll offset
    // toward 0 with an ease-out cubic over kSpringBackMs (220ms).
    //
    // Coordinate convention: m_overscrollY positive = viewport
    // translated DOWN (= user pulled past the TOP of content), matching
    // the convention "pull down at top = overshoot reveals empty space
    // above the content". Same for X.
    qreal                  m_overscrollX        = 0.0;
    qreal                  m_overscrollY        = 0.0;
    qreal                  m_overscrollRawX     = 0.0;  ///< accumulated raw delta before rubber-banding
    qreal                  m_overscrollRawY     = 0.0;
    QPointer<QWidget>      m_overscrollViewport;        ///< the viewport we're translating
    QPoint                 m_overscrollOrigPos;         ///< viewport's pre-overscroll position, restore target
    QTimer                *m_springBackTimer    = nullptr;
    QElapsedTimer          m_springBackClock;           ///< for ease-out cubic interp
    qreal                  m_springBackStartX   = 0.0;  ///< overscroll offset at spring-back start
    qreal                  m_springBackStartY   = 0.0;
    static constexpr int   kSpringBackMs            = 280;   ///< V32.39+: 170→280 — gentler, softer rebound (was snapping back too hard)
    // V32.34+ FIELD TUNING (mobile): the overscroll rubber-band felt too loose —
    // a big visible stretch even on gentle pulls past the end. Three levers:
    //   • kOverscrollRubberK lowered 0.4→0.22 = MORE friction (smaller sqrt
    //     output for the same raw pull → the band stretches less).
    //   • kOverscrollDeadzone* = a blended (raw-px + viewport-fraction) deadzone
    //     subtracted from the raw pull BEFORE the sqrt. Below it, zero visible
    //     overscroll — kills the "huge rubber band when barely past the edge"
    //     and the bounce-on-near-zero-motion. Blending a fixed px floor with a
    //     viewport fraction respects both HiDPI (where px are dense) and LoDPI.
    //   • kOverscrollMaxRatio tightened 0.3→0.22 = a firmer hard limit.
    static constexpr qreal kOverscrollRubberK      = 0.22;  ///< rubber-band coefficient (lower = more friction)
    static constexpr qreal kOverscrollMaxRatio    = 0.45;  ///< V32.39+: now only a SAFETY net (the crawl asymptote sits below it); was a hard wall at 0.16/0.22 that froze content away from the finger
    // V32.39+: asymptotic "crawl" replaces the hard overscroll clamp. The visible
    // offset approaches kOverscrollCeilingRatio×viewport but never reaches it, so
    // the content keeps following the finger (with diminishing motion) instead of
    // hard-stopping — leaving "ceiling space" to keep nudging the UI. Softness =
    // ceiling × kOverscrollCrawlK sets the initial responsiveness (slope ≈ 1/K),
    // so K≈1.25 tracks the finger ~0.8:1 at first, then eases in. See
    // SubPageDetent::crawlOffset / crawlInverse (tested).
    static constexpr qreal kOverscrollCeilingRatio = 0.32;  ///< soft asymptotic ceiling as a fraction of viewport
    static constexpr qreal kOverscrollCrawlK       = 1.25;  ///< softness = ceiling × this (lower = more 1:1 with the finger)
    static constexpr qreal kOverscrollDeadzonePx     = 12.0; ///< fixed px floor before any overscroll shows
    static constexpr qreal kOverscrollDeadzoneFrac   = 0.02; ///< + this fraction of viewport size (HiDPI/LoDPI blend)
    static constexpr qreal kOverscrollAccel          = 1.35; ///< >1 sharpens the post-deadzone onset so it feels intentional

    // V32.34+: blend a fixed px deadzone with a viewport-relative one. Taking the
    // max of (px floor) and (fraction × size) means small screens still get the
    // tactile px floor while large/HiDPI screens scale up proportionally.
    static qreal overscrollDeadzone(qreal viewportSize) {
        return qMax(kOverscrollDeadzonePx, viewportSize * kOverscrollDeadzoneFrac);
    }

    // ── V32.34+: two-level unified navigation (SC_UNIFIED_NAV) ────────────
    //
    // Behind the SC_UNIFIED_NAV flag, the navigator gains a SECOND level of
    // navigation that sits ON TOP of the existing page-paging, driven through
    // the SubTabbed bridge (currentSubTabbed / advanceSubTab / cycleSubTab):
    //
    //   • VERTICAL: normal content scroll. When the scroll hits its limit and
    //     the user keeps pulling (overscroll), a detent ARMS for ~1s; crossing
    //     a distance threshold while armed advances the SUB-TAB (down=next,
    //     up=prev, symmetric) instead of merely springing back. This reuses
    //     the existing overscroll machinery — the rubber-band curve IS the
    //     "visual nonlinearity"; we only add the arm-window + threshold +
    //     advanceSubTab() call. Lives inside the existing ChildScrollV intent
    //     (a COMMIT-TIME outcome, NOT a mid-stroke reclassify → no jitter).
    //
    //   • HORIZONTAL: scrubs the sub-tabs (advanceSubTab, NON-wrapping); at the
    //     first/last sub-tab the edge SPILLS to the adjacent PAGE (the existing
    //     page-commit), landing on that page's first sub-tab — no wrap-around to
    //     the opposite sub-tab. Decided at gestureEnd commit time from
    //     accumulated horizontal travel. (cycleSubTab, the old wrapping variant,
    //     is retained but unused — it teleported the row across at the edge,
    //     which felt wrong on device; advanceSubTab + page-spill replaced it.)
    //
    //   • A generic barrier SVG fades in proportional to detent progress on
    //     whichever axis is active (drawn in paintEvent as an overlay).
    //
    // All of this is INERT unless unifiedNavEnabled() is true, so the no-flag
    // behavior is byte-for-byte the pre-existing page-swipe + scroll.
    // (unifiedNavEnabled() itself is declared PUBLIC above — MainWindow needs
    // it to choose the bottom bar.)

    /// Detent feel constants. Seeded from the overscroll values; these are
    /// the FEEL parameters that need on-device tuning (friction, threshold,
    /// arm window, fade timing) — start sane, adjust in the build-feel loop.
    static constexpr int   kDetentArmWindowMs   = 1000;  ///< how long the detent stays armed after limit hit
    static constexpr qreal kVDetentRawThreshold = 220.0; ///< raw overscroll px (pre-rubber-band) to fire sub-tab advance
    static constexpr qreal kHDetentBreakPx       = 90.0;  ///< extra horizontal travel past cycle boundary to break to next page

    /// Vertical detent state. Armed when overscroll begins past the content
    /// limit; disarms after kDetentArmWindowMs with no threshold cross.
    bool          m_vDetentArmed     = false;
    qint64        m_vDetentArmedAtMs  = 0;     ///< QDateTime::currentMSecsSinceEpoch() when armed
    bool          m_vDetentFired      = false; ///< true once this overscroll streak already advanced a sub-tab (one per streak)

    /// V32.34+ SCROLL-TAKEOVER LOCK. After a detent (scroll→sub-tab) OR a page
    /// change fires, further takeover is blocked until this timestamp. Without
    /// it, ONE sustained scroll fired the detent repeatedly (~1/sec) and blasted
    /// through multiple sub-tabs (device log showed rawY=860 = 4× threshold in a
    /// single burst). The lock means a takeover "commits" and then the user must
    /// pause/restart the scroll for the next one. Duration from
    /// scrollTakeoverLockMs(). 0 = unlocked.
    qint64        m_scrollLockUntilMs = 0;
    qint64        m_lastDetentFeedMs = 0;  ///< V32.34+: wall-clock of last wheel-detent feed, for refresh-rate-independent accumulation
    QList<qint64> m_recentTakeovers;   ///< timestamps of recent scroll takeovers (~60s window) for the rate term
    bool          m_detentInvert = false;  ///< V32.34+: flip scroll→sub-tab direction for natural-scroll users (ui/scroll_detent_invert) — TRACKPAD
    bool          m_detentInvertMouse = false;  ///< V32.34+: independent flip for the MOUSE WHEEL (ui/scroll_detent_invert_mouse)
    bool          m_detentInvertTouch = false;  ///< V32.34+: independent flip for TOUCH DRAG (ui/scroll_detent_invert_touch). Touch has its own natural physical mapping, so default false; offered for parity with the wheel toggles.
    [[nodiscard]] qint64 scrollTakeoverLockMs() const;
    [[nodiscard]] bool   scrollTakeoverLocked() const;
    void                 engageScrollTakeoverLock();

    /// Horizontal sub-tab scrub state (for SC_UNIFIED_NAV). Tracks whether
    /// the current horizontal stroke is scrubbing sub-tabs vs. has broken
    /// through to page paging.
    bool          m_hSubScrubActive   = false; ///< this horizontal stroke is in sub-tab-scrub mode
    bool          m_hBrokeToPage       = false; ///< already broke through to page paging this stroke
    qreal         m_hScrubAccumPx      = 0.0;   ///< accumulated horizontal travel within current sub-tab cell

    /// Barrier overlay fade (0..1), driven by detent progress on the active
    /// axis. Painted as a generic SVG/shape overlay in paintEvent.
    qreal         m_barrierOpacity     = 0.0;
    /// V32.34+: which edge the friction barrier renders on, set by whichever
    /// detent is active. 0=bottom, 1=top, 2=right, 3=left. Lets the horizontal
    /// (sub-tab→page) barrier appear on the correct side rather than always at
    /// the bottom like the vertical (end-of-content) one.
    int           m_barrierEdge        = 0;
    /// V32.39+: grow-text scale (sub-tab label) tied to detent progress, set in
    /// evaluateVerticalDetent from SubPageDetent::textScale. 1.0 = resting size.
    /// Painted in paintEvent alongside the barrier so the gesture has a visual
    /// signal that swells as it nears the commit threshold.
    qreal         m_barrierTextScale   = 1.0;
    /// V32.39+: the tested resistance/progress curve for the vertical sub-page
    /// detent — buffer (detent to current page) → free middle → sticky toward
    /// commit, with a linear progress signal for the grow-text. Pure/Qt-free;
    /// see include/ui/SubPageDetent.hpp + tests/test_SubPageDetent.cpp.
    SubPageDetent m_subPageDetent;

    /// V32.34+: evaluate the vertical detent during overscroll. Called from
    /// applyOverscroll when SC_UNIFIED_NAV is on. Arms on first overscroll,
    /// updates m_barrierOpacity from progress, and fires advanceSubTab once
    /// the raw threshold is crossed while armed (resetting overscroll so the
    /// new sub-tab's content starts clean).
    void evaluateVerticalDetent(qreal rawY);
    /// V32.34+: reset all unified-nav detent state (on gesture begin / page
    /// change / disarm). Keeps the barrier overlay from lingering.
    void resetDetentState();

    /// Apply (dx, dy) to overscroll instead of the scrollbar. Called
    /// when the scrollbar has clamped at min/max but motion continues.
    /// Updates m_overscrollRaw* (accumulated raw delta) and recomputes
    /// m_overscroll* (rubber-banded visible offset), then translates
    /// the viewport.
    void applyOverscroll(qreal dx, qreal dy);

    /// True if overscroll is currently non-zero (viewport is translated
    /// away from its anchor). Used to decide whether to spring back at
    /// TouchEnd / end-of-fling.
    [[nodiscard]] bool hasOverscroll() const;

    /// Start the spring-back animation. Captures the current overscroll
    /// offsets as the starting point; the timer interpolates them to
    /// zero over kSpringBackMs.
    void startSpringBack();

    /// Cancel any in-flight spring-back. Called on TouchBegin so the
    /// user can grab the overscrolled viewport and continue dragging
    /// it (the existing offset stays put while they drag further).
    void cancelSpringBack();

    /// QTimer::timeout slot — drives one frame of spring-back.
    void onSpringBackTick();

    /// Immediately commit the current overscroll offsets to the
    /// viewport's geometry. Called from applyOverscroll and
    /// onSpringBackTick.
    void applyOverscrollToViewport();

    /// Clear overscroll state and restore the viewport to its anchor.
    /// Called from gesture-reset paths when no spring-back is wanted
    /// (e.g. cancellation), and at the end of a successful spring-back.
    void clearOverscroll();

    /// Push one motion delta into the velocity ring. Called once per
    /// TouchUpdate during a ChildScrollV gesture, AFTER the scroll
    /// commit (so the deltas we measure exactly match what the
    /// scrollbar consumed — partial-commits from pacing don't skew
    /// velocity).
    void recordVelocitySample(qreal dx, qreal dy);

    /// Compute the (vx, vy) velocity in px/ms from the samples in
    /// the trailing kVelocityWindowMs window. Returns (0,0) if the
    /// window is empty (e.g. user paused before releasing).
    void computeReleaseVelocity(qreal *outVx, qreal *outVy) const;

    /// Start the decay animation for the current m_scrollTarget.
    /// Reads m_flingVelocityX/Y (set by caller), arms m_flingTimer,
    /// and sets m_flingLastTick. No-op if velocity is below
    /// kMinFlingVelocity threshold.
    void startFling(qreal vx, qreal vy);

    /// Stop any in-flight fling immediately. Safe to call when none
    /// is running. Called from TouchBegin in onApplicationEvent and
    /// from gesture-reset paths.
    void cancelFling();

    /// QTimer::timeout slot — drives one frame of decay.
    void onFlingTick();

    // V32.32: horizontal bias for axis-lock decision. When the user makes
    // an ambiguous diagonal gesture, prefer horizontal (page swipe) over
    // vertical (scroll). Rationale: a missed page-swipe is more annoying
    // than a missed scroll start, because scrolling is naturally resumable
    // (you just drag again) while a missed page-swipe requires a full
    // do-over. With kHorizontalAxisBias = 1.5, horizontal wins when its
    // magnitude is at least ~67% of vertical's — i.e. anything within
    // 56° of horizontal reads as a page swipe.
    static constexpr qreal kHorizontalAxisBias = 1.5;

    /// Apply (dx,dy) to the given scroll bars, rate-limited to ~60Hz.
    /// If less than kScrollPacingMs has elapsed since the last commit,
    /// the delta is buffered into m_pendingScrollDx/Dy and applied on
    /// a later call when the timer has elapsed. Returns true if a commit
    /// happened on this call.
    bool commitScrollPaced(QScrollBar *hbar, QScrollBar *vbar, int dx, int dy);
    /// V32.34+: apply a scroll delta and route any clamped overflow into the
    /// overscroll system (so a direct drag past the edge feeds the detent, not
    /// just a fling). Shared by both commitScrollPaced commit branches.
    void commitScrollDelta(QScrollBar *hbar, QScrollBar *vbar, int dx, int dy);

    /// Force-flush any pending scroll delta. Called on TouchEnd so the
    /// last bit of motion isn't left buffered when the finger lifts.
    void flushPendingScroll(QScrollBar *hbar, QScrollBar *vbar);

    // ── Velocity ring buffer (wider window than before) ──────────────────
    static constexpr int    kVelBufSize   = 24;    // more samples for slow drags
    static constexpr qint64 kVelWindowMs  = 120;   // wider window catches slow swipes
    struct VelSample { qint64 timeMs; qreal x; };
    VelSample m_velSamples[kVelBufSize] {};
    int       m_velHead  = 0;
    int       m_velCount = 0;

    void  velocityReset();
    void  velocityRecord(qreal x);
    [[nodiscard]] qreal velocityCompute() const; // exponentially weighted

    QSet<QWidget *>  m_filteredWidgets;

    // ── Trackpad / wheel state ───────────────────────────────────────────
    enum class WheelPhase { Idle, Pending, Active };
    WheelPhase m_wheelPhase            = WheelPhase::Idle;
    qreal      m_wheelAccum            = 0.0;
    qreal      m_wheelSmoothed         = 0.0; // EMA-filtered delta
    QPointF    m_wheelOrigin;
    bool       m_wheelHorizontalLocked = false;
    QTimer    *m_wheelIdleTimer        = nullptr;

    bool processWheelEvent(QWheelEvent *e);
    bool isHorizontalScrollSuppressed(const QPoint &globalPos) const;
    /// V32.34+ unified nav: true if the scroll area under `globalPos` is at its
    /// vertical scroll limit in the direction `vPx` would scroll (or there is no
    /// vertically-scrollable area there at all). Used so a vertical wheel at the
    /// end of content feeds the sub-tab detent rather than being discarded.
    /// vPx > 0 scrolls toward the TOP (min); vPx < 0 toward the BOTTOM (max).
    bool wheelAtVerticalLimit(const QPoint &globalPos, qreal vPx) const;
    /// V32.34+ unified nav: accumulate vertical wheel overflow into the sub-tab
    /// detent accumulator and fire evaluateVerticalDetent, bypassing
    /// applyOverscroll's viewport-target requirement (none exists on the wheel
    /// path). This is what makes scroll→sub-tab work on a laptop trackpad/wheel.
    void feedWheelVerticalDetent(qreal dy);
    void installFilterRecursive(QWidget *w);
    /// V32.33+ Phase 1 audit: recursive removal from m_filteredWidgets to
    /// match installFilterRecursive's recursive addition. Called from
    /// childEvent when a widget subtree is removed.
    void uninstallFilterRecursive(QWidget *w);

    // ── Physics constants ────────────────────────────────────────────────
    // All distance thresholds are fractions of page width, not absolute pixels.
    // This makes the feel screen-size-independent.

    // Rubber-band stiffness. Lower = more elastic (iOS-like), higher = stiffer.
    static constexpr qreal kRubberBandK      = 0.40;

    // Commit if dragged more than this fraction of page width.
    static constexpr qreal kCommitFraction   = 0.22; // 22% — feels right on mobile flicks

    // Flick: commit at any distance above kFlickMinPx if velocity is high enough.
    static constexpr qreal kFlickThresholdPxMs = 0.30; // px/ms — mobile flicks are quick
    static constexpr int   kFlickMinPx          = 14;  // mobile thumb flicks are shorter

    // Minimum pixel movement before direction lock engages.
    //
    // V32.33+ Phase 1 tuning: split into per-axis thresholds. Page-swipe
    // is a deliberate gesture that benefits from a low threshold (the
    // user clearly wants to navigate); scroll is more incidental and
    // benefits from a higher threshold (prevents accidental engagement
    // from tap jitter). The horizontal-axis-bias (below) protects against
    // diagonal motion accidentally resolving to PageSwipe.
    //
    // kLockThresholdPx remains as the BACKWARDS-COMPAT alias used by
    // the existing per-axis logic inside the ChildScrollV dispatch and
    // by gestureUpdate. The Phase 1 ambiguous-resolution path uses the
    // per-axis constants below directly.
    static constexpr int   kLockThresholdPx       = 14;  // legacy / scroll axis-lock
    static constexpr int   kPageSwipeThresholdPx  = 6;   // horizontal page-swipe — low for responsiveness
    static constexpr int   kScrollThresholdPx     = 14;  // vertical scroll engagement — committed motion required

    // Direction lock ratio: reject if |hx| < |vy| * ratio
    static constexpr qreal kDirectionRatio     = 0.55; // stricter — require more horizontal

    // Momentum animation: duration = velocity / deceleration, clamped.
    // kDeceleration in px/ms²: higher = snappier commits.
    static constexpr qreal kDeceleration       = 2.5;  // px/ms²
    static constexpr int   kCommitMaxMs        = 340;
    static constexpr int   kCommitMinMs        = 60;

    // Bounce: base duration + depth-proportional term.
    static constexpr int   kBounceBaseMs       = 90;
    static constexpr qreal kBounceDepthMs      = 0.35; // ms per pixel of pull depth

    // Wheel / trackpad
    static constexpr qreal kAngleDeltaToPx     = 1.0 / 6.0;
    static constexpr int   kWheelIdleMs        = 350;
    // 200px of trackpad motion = full page commit (screen-size independent).
    // Tune lower (e.g. 150) for more sensitive trackpad, higher (250) for less.
    static constexpr qreal kWheelPageScale     = 200.0;
};

} // namespace SnapClone::UI
