// ─────────────────────────────────────────────────────────────────────────────
// ModalSurface.hpp — unified overlay/dialog/toast/confirm rendering surface
//
// V32.32: replaces the ad-hoc z-order management (`raise()` calls in dozens of
// places, MobileDialog's sheet-or-toplevel branching, the AmbientBlobs raise
// dance) with a structural layer model.
//
// The MainWindow owns one ModalSurface that fills its central area. Inside the
// ModalSurface are six layer containers, stacked in Qt sibling order:
//
//     L1 Ambient    — palette tints, blob overlays
//     L2 Hint       — peel hints, swipe hints, badge animations
//     L3 Dialog     — full dialogs (ContactEditor, ContactPicker, SnapComposer)
//     L4 Confirm    — confirms above dialogs (delete-contact, leave-convo)
//     L5 Toast      — toasts above everything except critical
//     L6 Critical   — auth challenges, fatal-error blockers
//
// Each layer is a transparent-for-mouse-events QWidget until something is
// presented into it. When `present(widget, Layer::Dialog)` is called the
// widget is reparented into L3's container; the container flips out of
// transparent-mouse mode so it can capture tap-outside-to-dismiss; an
// optional scrim is shown behind the widget.
//
// On desktop AND mobile the rendering path is identical — same widget tree,
// same code. This deliberately removes the "top-level QDialog window vs
// in-window sheet" branching that was the source of recurring z-order bugs.
//
// V32.32 phase 1 (this turn): infrastructure + test widgets only. Real
// dialogs continue to use MobileDialog. Migration happens in subsequent
// turns once the infrastructure is proven on hardware.
// ─────────────────────────────────────────────────────────────────────────────

#pragma once
#include <QWidget>
#include <QHash>
#include <QList>
#include <QPointer>

namespace SnapClone::UI {

class ModalSurface : public QWidget
{
    Q_OBJECT

public:
    enum class Layer : int {
        Ambient   = 1,
        Hint      = 2,
        Dialog    = 3,
        Confirm   = 4,
        Toast     = 5,
        Critical  = 6,
    };
    Q_ENUM(Layer)

    explicit ModalSurface(QWidget *parent = nullptr);
    ~ModalSurface() override;

    /// Present `widget` into the given layer. ModalSurface reparents the
    /// widget to the layer container and shows it. Caller retains ownership
    /// semantics in the sense that closing/dismissing the widget removes it
    /// from the layer; widget destruction (via deleteLater or explicit
    /// delete) also cleanly removes it from the stack via QPointer.
    ///
    /// `dismissOnTapOutside` — when true, a tap on the layer container
    ///                        outside the widget's geometry closes it.
    /// `blocksTouches`       — when true, the layer container does NOT
    ///                        propagate touches/mouse to layers below.
    ///                        Default true for Dialog/Confirm/Critical;
    ///                        false for Ambient/Hint/Toast.
    QWidget *present(QWidget *widget, Layer layer,
                     bool dismissOnTapOutside = true,
                     bool blocksTouches = true);

    /// Dismiss the newest-presented widget across all layers. Useful for
    /// a hardware back-button or escape-key binding.
    void dismissTopmost();

    /// Dismiss every widget in the named layer.
    void dismissLayer(Layer layer);

    /// Dismiss everything.
    void dismissAll();

    /// True if no widgets are currently presented in any layer.
    bool isEmpty() const;

    /// Layer enum → string for debugging.
    static QString layerName(Layer layer);

signals:
    /// Emitted whenever a widget is presented or dismissed. Useful for
    /// debugging and for code that wants to know "is anything modal
    /// currently in the way" (e.g. swallow swipe gestures while a
    /// dialog is open).
    void stackChanged();

protected:
    void resizeEvent(QResizeEvent *) override;
    bool eventFilter(QObject *obj, QEvent *ev) override;

private:
    /// Layer container helper struct.
    struct LayerCtx {
        QWidget *container = nullptr;  // child of ModalSurface, fills it
        QWidget *scrim     = nullptr;  // optional translucent backdrop
        QList<QPointer<QWidget>> stack; // newest-presented at end
    };

    /// Build the layer container for a given layer. Called once at ctor.
    void buildLayer(Layer layer);

    /// Refresh the layer container's properties based on its current
    /// stack: visibility, mouse-event transparency, scrim visibility.
    void refreshLayer(Layer layer);

    /// Reposition the scrim and any presented widgets to fill the layer.
    /// Called from resizeEvent.
    void relayoutLayer(Layer layer);

    /// Remove a specific widget from a layer's stack. Called when the
    /// widget is destroyed (via QPointer tracking) or when explicitly
    /// dismissed.
    void removeFromStack(QWidget *widget, Layer layer);

    QHash<Layer, LayerCtx>   m_layers;
    QHash<QWidget *, Layer>  m_widgetLayer;     // reverse map for dispatch
    QHash<QWidget *, bool>   m_dismissOnTap;    // per-widget config
};

} // namespace SnapClone::UI
