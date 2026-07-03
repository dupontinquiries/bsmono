#pragma once

// ─── ui/UnifiedNavBar.hpp ─────────────────────────────────────────────────────
//
// The visible half of the two-level unified navigation (V32.34+,
// SC_UNIFIED_NAV). Replaces the static page-icon TabBar at the bottom with a
// bar that shows the CURRENT page's sub-tabs inline and morphs as you change
// pages — the `|| CONTACTS . C1 . C2 . SENT . UNREAD | NEXT PAGE ||` model the
// user sketched.
//
// It is a pure VIEW over the SwipeNavigator: it reads the current page's
// SubTabbed model (sub-tab labels + active index + badges) and renders one
// tappable segment per sub-tab, plus a trailing "next page" affordance. It
// owns NO navigation state — taps and the next-page button call back into the
// navigator, and it refreshes from the navigator's pageChanged / subTabChanged
// signals. A page with no sub-tabs (Camera, Wayfinder) shows just the page
// name + next/prev affordances.
//
// Why a view-only widget: the gesture state machine already lives in
// SwipeNavigator (Step C). This bar is the visual feedback for it, kept
// deliberately dumb so the two can't disagree about "where am I" — the
// navigator is the single source of truth.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QList>
#include <QStringList>

QT_BEGIN_NAMESPACE
class QHBoxLayout;
class QPushButton;
class QPropertyAnimation;
QT_END_NAMESPACE

namespace SnapClone::UI {

class SwipeNavigator;

class UnifiedNavBar : public QWidget
{
    Q_OBJECT
    Q_PROPERTY(qreal glassAmount READ glassAmount WRITE setGlassAmount)
public:
    explicit UnifiedNavBar(SwipeNavigator *nav, QWidget *parent = nullptr);

public slots:
    /// Rebuild the segment row for the current page (called on pageChanged and
    /// at construction). Reads the page's SubTabbed model.
    void rebuildForCurrentPage();
    /// Update only the active-segment highlight (called on subTabChanged) —
    /// cheaper than a full rebuild when only the selection moved.
    void refreshActiveSegment();

    /// V32.34+: in-flight drag preview from the navigator's subTabDragProgress.
    /// dir = +1 toward next sub-tab, -1 toward prev; progress 0..1 toward the
    /// commit point. progress 0 clears the preview. Rendered CHEAPLY in
    /// paintEvent (a sliding underline), NOT via setStyleSheet — keeping the
    /// ~60Hz drag stream off the stylesheet machinery that the teardown crash
    /// is sensitive to.
    void onDragProgress(int dir, qreal progress);

    /// V32.34+: repaint the sub-tab count badges. The bar reads counts live
    /// from the page's SubTabbed::subTabBadge() during paint, so this is just
    /// an update() trigger — wire a page's count-changed signal to it (done in
    /// MainWindow) so e.g. a freshly-arrived unread bumps the bar's pill while
    /// the user is sitting on that page. Cheap: schedules one repaint.
    void refreshBadges();

    /// V32.38f: morph the bar's appearance to the full-bleed "glass" look used
    /// on the camera page, WITHOUT changing it on any other page. When on, the
    /// bar paints a dark frosted gradient (transparent stylesheet bg) so it sits
    /// over the live viewfinder like the top/bottom camera scrims; when off, it
    /// reverts to the normal opaque BG_RAISED bar. Animated via m_glassAmount so
    /// the change crossfades as the camera page becomes/stops being current.
    /// Driven from MainWindow on page change.
    void setCameraGlassMode(bool on);

    // Animated 0..1: 0 = normal bar, 1 = full glass. Painted in paintEvent.
    qreal glassAmount() const { return m_glassAmount; }
    void  setGlassAmount(qreal a);

protected:
    /// Re-evaluate the collapse level when the bar's width changes (mobile
    /// screens shrink the available room for sub-tab labels).
    void resizeEvent(QResizeEvent *e) override;
    /// Paints the active-segment underline + the in-flight drag preview.
    void paintEvent(QPaintEvent *e) override;
    /// Re-sync the highlight from the live SubTabbed model when the bar is
    /// shown. Cheap insurance: covers any sub-tab change that happened via a
    /// path the bar didn't directly observe (e.g. future programmatic filter
    /// switches) without coupling the bar to each page's internals.
    void showEvent(QShowEvent *e) override;

private:
    void buildChrome();
    void clearSegments();
    /// V32.34+ responsive collapse: pick how much sub-tab chrome fits in the
    /// current width and restyle/relabel segments accordingly. Rule:
    ///   • WIDE  — every segment shows its full text label.
    ///   • TIGHT — the ACTIVE segment keeps its full label; inactive segments
    ///             shrink their horizontal padding to claw back room.
    ///   • NARROW— inactive segments collapse to an icon (if the page supplies
    ///             one via SubTabbed::subTabIcon) or a short 1–3 char form;
    ///             the page-name chip hides first to free the most space.
    /// Called from resizeEvent + after every rebuild. Idempotent.
    void applyResponsiveCollapse();
    /// Build the stylesheet for one segment at the current collapse level.
    [[nodiscard]] QString segmentStyle(bool active, bool compactPadding) const;

    SwipeNavigator           *m_nav = nullptr;
    bool   m_cameraGlass  = false;   ///< target: is the camera page current?
    qreal  m_glassAmount  = 0.0;     ///< animated 0..1 toward m_cameraGlass
    QPropertyAnimation *m_glassAnim = nullptr;
    QHBoxLayout              *m_segRow   = nullptr;   ///< holds the sub-tab segment buttons
    QList<QPushButton *>      m_segs;                 ///< one per sub-tab of the current page
    QStringList               m_segLabels;            ///< full labels (for restore when widening)
    QPushButton              *m_prevPageBtn = nullptr;
    QPushButton              *m_nextPageBtn = nullptr;
    QPushButton              *m_pageNameBtn = nullptr; ///< current page label (also opens page picker later)

    // V32.34+ in-flight drag preview (rendered in paintEvent, no setStyleSheet).
    int                       m_dragDir      = 0;    ///< +1 toward next, -1 toward prev, 0 = none
    qreal                     m_dragProgress = 0.0;  ///< 0..1 toward the commit point

    // V32.34+ underline MORPH: when the active sub-tab changes via tap/detent
    // (not a drag), the underline animates smoothly from the old segment's
    // x-span to the new one instead of snapping. m_morph is 0..1 across the
    // animation; m_morphFromL/R and m_morphToL/R are the interpolated edges in
    // bar-local px. Driven by a QVariantAnimation (no setStyleSheet — paintEvent
    // reads the interpolated span, keeping the QStyleSheetStyle caches untouched
    // during the ~200ms morph, same discipline as the drag preview).
    int                       m_morphActive  = -1;   ///< segment index the morph is settling ON (-1 = idle)
    qreal                     m_morph        = 1.0;  ///< 0..1 progress (1 = settled)
    qreal                     m_morphFromL = 0, m_morphFromR = 0;
    qreal                     m_morphToL   = 0, m_morphToR   = 0;
    void                     *m_morphAnim    = nullptr; ///< QVariantAnimation* (fwd-declared in cpp)
    void                      startUnderlineMorph(int toIndex);
};

} // namespace SnapClone::UI
