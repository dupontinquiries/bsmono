#include "ui/ScrollAreas.hpp"
#include <QtMath>

namespace SnapClone::UI {

void HorizontalScrollArea::wheelEvent(QWheelEvent *e)
{
    const QPoint px  = e->pixelDelta();
    const QPoint ang = e->angleDelta();

    int delta = 0;
    if (!px.isNull()) {
        delta = (qAbs(px.x()) > qAbs(px.y())) ? px.x() : px.y();
    } else {
        delta = (qAbs(ang.x()) > qAbs(ang.y())) ? ang.x() / 8 : ang.y() / 8;
    }

    if (delta != 0) {
        // V32.34+ scroll passthrough to page nav: apply the delta, but if the
        // scrollbar was ALREADY clamped at the edge we're pushing toward (the
        // value can't move further in this direction), DON'T consume the event
        // — let it propagate to SwipeNavigator's wheel handler, which turns
        // continued horizontal scrolling at the carousel's end into a page
        // change. Without this the carousel silently swallowed the wheel at its
        // edge and the gesture died, so you couldn't "scroll past the end" to
        // the next page. (delta>0 scrolls content left → toward MAX; delta<0
        // → toward MIN. setValue uses value-delta, so pushing right/positive
        // delta DECREASES value toward min — match the clamp check to that.)
        QScrollBar *hb = horizontalScrollBar();
        const int before = hb->value();
        hb->setValue(before - delta);
        const int after = hb->value();
        if (after != before) {
            // We moved — the scroll consumed it.
            e->accept();
            return;
        }
        // Clamped at the edge and still pushing further: pass through to the
        // parent (SwipeNavigator) for the page-swipe handoff. Do NOT accept.
        e->ignore();
        QScrollArea::wheelEvent(e);
        return;
    }
    QScrollArea::wheelEvent(e);
}

void CarouselAwareScrollArea::wheelEvent(QWheelEvent *e)
{
    if (themeScroll && themeScroll->isVisible()) {
        const QPoint px  = e->pixelDelta();
        const QPoint ang = e->angleDelta();
        int hDelta = 0;
        if (!px.isNull())            hDelta = px.x();
        else if (qAbs(ang.x()) > 0)  hDelta = ang.x() / 8;

        if (hDelta != 0) {
            const QPoint local = mapFromGlobal(e->globalPosition().toPoint());
            const QRect tsRect(themeScroll->mapTo(this, QPoint(0, 0)),
                               themeScroll->size());
            if (tsRect.adjusted(-8, -80, 8, 80).contains(local)) {
                themeScroll->horizontalScrollBar()->setValue(
                    themeScroll->horizontalScrollBar()->value() - hDelta);
                e->accept();
                return;
            }
        }
    }
    QScrollArea::wheelEvent(e);
}

} // namespace SnapClone::UI
