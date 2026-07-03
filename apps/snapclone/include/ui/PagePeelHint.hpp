#pragma once
// ─── PagePeelHint.hpp ────────────────────────────────────────────────────────
//
// Subtle curved "peel" tab hints at the left and right edges of the page area,
// suggesting horizontal swipe navigation to new users.
//
// Behavior:
//   • Always visible (not touchscreen-gated) so it works for mouse + touch + trackpad
//   • Pulses with a slow shared breathing rhythm (3.2s per cycle, sine curve)
//   • Uses TEXT_SECONDARY (muted) not ACCENT (forceful) for color
//   • Fade-out and self-delete after kMinSwipesBeforeHide swipes
//   • Hides briefly after each swipe and reappears after kIdleReturnMs of inactivity
//
// Owner widget should:
//   • new PagePeelHint(swiper) — covers full parent rect
//   • call setPageState(currentPage, totalPages) on every page change
//   • call dismissIfFamiliar() on every page change to fade away after the threshold
//
// Self-cleanup: deletes itself when familiarity threshold is met (uses >= kMinSwipesBeforeHide).

#include <QWidget>
#include <QPropertyAnimation>
#include <QTimer>

namespace SnapClone::UI {

class PagePeelHint : public QWidget
{
    Q_OBJECT
    Q_PROPERTY(qreal peelOpacity READ peelOpacity WRITE setPeelOpacity)

public:
    static constexpr int kMinSwipesBeforeHide = 4;
    static constexpr int kIdleReturnMs        = 30000;

    explicit PagePeelHint(QWidget *parent);

    void setPageState(int current, int total);
    void dismissIfFamiliar();

    qreal peelOpacity() const { return m_op; }
    void  setPeelOpacity(qreal v) { m_op = v; update(); }

protected:
    void paintEvent(QPaintEvent *e) override;

private:
    void startPulse();
    void hidePeelsBriefly();
    void reshowPeels();

    QPropertyAnimation *m_anim      = nullptr;
    QTimer             *m_idleTimer = nullptr;
    qreal m_op        = 0.0;
    bool  m_showLeft  = false;
    bool  m_showRight = true;
    bool  m_hidden    = false;
};

} // namespace SnapClone::UI
