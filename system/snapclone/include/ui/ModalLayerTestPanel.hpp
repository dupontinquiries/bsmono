// ─────────────────────────────────────────────────────────────────────────────
// ModalLayerTestPanel.hpp — V32.32 modal-layer test harness
//
// Floating panel with buttons that present synthetic test widgets into each
// of ModalSurface's six layers. Lets us verify on real hardware that:
//   • Each layer's z-order is correct (L6 above L5 above L4 ...)
//   • Tap-outside-to-dismiss works where it should and doesn't where it
//     shouldn't (Critical stays open until explicitly closed)
//   • Scrim alpha is right per layer (Critical heaviest, Toast none)
//   • Layer interaction passthrough works (Ambient/Hint don't block clicks
//     to page content; Dialog/Confirm/Critical do)
//   • Resize keeps everything sized correctly
//
// Once the infrastructure is validated, ContactEditorDialog and friends get
// migrated onto ModalSurface and this test panel can be removed (or kept
// behind a debug-build flag).
// ─────────────────────────────────────────────────────────────────────────────

#pragma once
#include <QWidget>
#include <QPointer>
#include <QSet>

namespace SnapClone::UI {

class ModalSurface;

class ModalLayerTestPanel : public QWidget
{
    Q_OBJECT
public:
    ModalLayerTestPanel(ModalSurface *surface, QWidget *parent = nullptr);

protected:
    /// V32.34+: routes mouse/touch release to close for widgets in
    /// m_clickToClose. Makes the test panel friendlier — tap anywhere
    /// on a test widget (not just outside it) dismisses it.
    bool eventFilter(QObject *obj, QEvent *ev) override;

private:
    /// Helper that builds a colored test widget and presents it. Returns
    /// the presented widget so the caller can configure further if needed.
    void presentTestWidget(int layerIndex, const QString &label,
                            bool dismissOnTap, bool blocksTouches);

    QPointer<ModalSurface> m_surface;
    /// V32.34+: widgets that should close on any tap, not just tap-
    /// outside. Critical-layer (5) widgets are NOT added here — they
    /// require the explicit Close button per the test panel's intent.
    QSet<QWidget *>        m_clickToClose;
};

} // namespace SnapClone::UI
