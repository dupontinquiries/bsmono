#pragma once
// ─── ScrollAreas.hpp ─────────────────────────────────────────────────────────
//
// Two QScrollArea subclasses for the Settings page:
//
// HorizontalScrollArea
//   For the theme carousel — maps wheel events (horizontal trackpad swipe OR
//   vertical mouse wheel) to horizontal scrolling. Without this, mouse wheel
//   over the carousel does nothing because QScrollArea defaults to vertical
//   scroll on wheel.
//
// CarouselAwareScrollArea (kept for compatibility)
//   The OUTER settings scroll area. Forwards wheel events to a tracked inner
//   carousel when the cursor is positioned over it. Now somewhat redundant
//   since HorizontalScrollArea owns wheel events when the cursor is directly
//   over the carousel — but covers the case where the cursor is just outside
//   the carousel (e.g. on a theme card's edge).

#include <QScrollArea>
#include <QWheelEvent>
#include <QScrollBar>

namespace SnapClone::UI {

/// Inner carousel scroll area — wheel events scroll horizontally.
class HorizontalScrollArea : public QScrollArea
{
    Q_OBJECT
public:
    explicit HorizontalScrollArea(QWidget *parent = nullptr) : QScrollArea(parent) {}

protected:
    void wheelEvent(QWheelEvent *e) override;
};

/// Outer settings scroll area — forwards horizontal wheel deltas to the
/// embedded carousel when the cursor is over it.
class CarouselAwareScrollArea : public QScrollArea
{
    Q_OBJECT
public:
    explicit CarouselAwareScrollArea(QWidget *parent = nullptr) : QScrollArea(parent) {}

    /// Set after the carousel is built. The outer scroll area will check this
    /// pointer on every wheel event.
    QScrollArea *themeScroll = nullptr;

protected:
    void wheelEvent(QWheelEvent *e) override;
};

} // namespace SnapClone::UI
