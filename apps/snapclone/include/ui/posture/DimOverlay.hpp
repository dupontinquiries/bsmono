#pragma once
// ─── ui/posture/DimOverlay.hpp ────────────────────────────────────────────────
//
// A full-bleed translucent-black overlay the PostureEngine fades in to dim the UI
// (50% → 75%) and back out. We never touch the backlight (by design), so this is
// purely an in-app visual dim; the real power saving comes from the screensaver
// render-rate cap, not from this. While it's showing any dim at all the phone is
// covered (proximity near), so it also swallows touches to stop a cheek tapping
// things. At full clear it hides and becomes click-through.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QPointer>

class QPropertyAnimation;

namespace SnapClone::UI {

class DimOverlay : public QWidget {
    Q_OBJECT
    Q_PROPERTY(qreal dimOpacity READ dimOpacity WRITE setDimOpacity)
public:
    explicit DimOverlay(QWidget *parent);

    [[nodiscard]] qreal dimOpacity() const { return m_opacity; }
    void setDimOpacity(qreal o);

public Q_SLOTS:
    /// Animate to `opacity` (0..1) over `durationMs`. Wired to PostureEngine's
    /// dimTargetChanged.
    void animateTo(qreal opacity, int durationMs);

protected:
    void paintEvent(QPaintEvent *) override;
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void syncToParent();
    void refreshInteractivity();

    qreal m_opacity = 0.0;
    QPointer<QPropertyAnimation> m_anim;
};

} // namespace SnapClone::UI
