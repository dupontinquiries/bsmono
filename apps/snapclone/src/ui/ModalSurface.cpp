#include "ui/ModalSurface.hpp"
#include "debug/Trace.hpp"   // per-event diagnostics: off unless the channel is on
#include "ui/Theme.hpp"

#include <QResizeEvent>
#include <QMouseEvent>
#include <QTouchEvent>
#include <QLoggingCategory>

namespace SnapClone::UI {

Q_LOGGING_CATEGORY(lcModal, "snapclone.ui.modal")

ModalSurface::ModalSurface(QWidget *parent)
    : QWidget(parent)
{
    setObjectName(QStringLiteral("modalSurface"));
    // The surface itself is invisible — it's just a container for the
    // layers. Without a default empty stylesheet some Qt configs paint
    // it grey, which would look like a global tint.
    setStyleSheet(QStringLiteral("background: transparent;"));
    // Start transparent for mouse — empty layers shouldn't eat clicks.
    setAttribute(Qt::WA_TransparentForMouseEvents, true);

    // Build the six layer containers in numeric order. Qt's sibling
    // z-order is "older first, newer last" so L1 ends up at the bottom
    // and L6 at the top, exactly what we want.
    for (int i = static_cast<int>(Layer::Ambient);
         i <= static_cast<int>(Layer::Critical); ++i) {
        buildLayer(static_cast<Layer>(i));
    }
}

ModalSurface::~ModalSurface() = default;

void ModalSurface::buildLayer(Layer layer)
{
    LayerCtx ctx;
    ctx.container = new QWidget(this);
    ctx.container->setObjectName(
        QStringLiteral("modalLayer_") + layerName(layer));
    ctx.container->setStyleSheet(QStringLiteral("background: transparent;"));
    ctx.container->setGeometry(rect());
    // Empty by default → don't intercept input.
    ctx.container->setAttribute(Qt::WA_TransparentForMouseEvents, true);
    ctx.container->installEventFilter(this);
    ctx.container->show();

    // Per-layer scrim policy. Dialog and Confirm get a translucent
    // black scrim that darkens content below; Critical gets a heavier
    // one. Ambient/Hint/Toast have no scrim (they're purely additive
    // overlays; the page underneath stays fully interactive when
    // nothing dismissible is on top).
    const bool wantsScrim =
           layer == Layer::Dialog
        || layer == Layer::Confirm
        || layer == Layer::Critical;
    if (wantsScrim) {
        ctx.scrim = new QWidget(ctx.container);
        ctx.scrim->setObjectName(QStringLiteral("modalScrim_") + layerName(layer));
        const int alpha = (layer == Layer::Critical) ? 200 :
                          (layer == Layer::Confirm)  ? 170 : 140;
        ctx.scrim->setStyleSheet(QStringLiteral(
            "background: rgba(0,0,0,%1);").arg(alpha));
        ctx.scrim->setGeometry(ctx.container->rect());
        // V32.32 (revision): scrim must be transparent-for-mouse so
        // tap-outside-dismiss events reach the CONTAINER's eventFilter.
        // Without this, scrim taps just get swallowed by the scrim's
        // default mousePressEvent and never bubble up.
        ctx.scrim->setAttribute(Qt::WA_TransparentForMouseEvents, true);
        ctx.scrim->hide();
    }

    m_layers.insert(layer, ctx);
}

QWidget *ModalSurface::present(QWidget *widget, Layer layer,
                                bool dismissOnTapOutside,
                                bool blocksTouches)
{
    if (!widget) return nullptr;
    if (!m_layers.contains(layer)) {
        qCWarning(lcModal) << "present: unknown layer" << static_cast<int>(layer);
        return widget;
    }
    LayerCtx &ctx = m_layers[layer];

    // Reparent into the layer container. The widget keeps whatever
    // geometry the caller set; ModalSurface doesn't impose layout.
    widget->setParent(ctx.container);
    // V32.32 (revision): widget MUST destroy itself on close, otherwise
    // close() just hides — the destroyed signal never fires, and the
    // stack/scrim/container-opaque cleanup in removeFromStack never
    // runs. Symptom: dismissed widget vanishes but the scrim stays on
    // screen as a non-interactive grey overlay, eating all clicks.
    // This was the "screen covered in grey after sending a snap" bug
    // the user remembered seeing previously.
    widget->setAttribute(Qt::WA_DeleteOnClose, true);
    // Track this widget so we can clean it up if it's destroyed
    // externally. QPointer in m_stacks handles the nulling automatically.
    ctx.stack.append(QPointer<QWidget>(widget));
    m_widgetLayer.insert(widget, layer);
    m_dismissOnTap.insert(widget, dismissOnTapOutside);

    // Catch destruction so we can update the stack lists. With
    // WA_DeleteOnClose=true above, close() → hide → deleteLater → this
    // signal fires on the next event-loop turn. The QPointer in
    // ctx.stack tracks this safely; we just need to remove the
    // now-null entry and update the container's opacity.
    connect(widget, &QObject::destroyed, this,
            [this, layer](QObject *obj) {
        SC_TRACE_S(Layout, lcModal) << "widget destroyed in" << layerName(layer)
                        << "— calling removeFromStack";
        removeFromStack(static_cast<QWidget *>(obj), layer);
        emit stackChanged();
    });

    // Layer-level mouse policy. If ANY widget in the layer wants to
    // block touches, the container flips out of transparent-mouse mode.
    // (We track per-widget for tap-outside-dismiss but pass blocksTouches
    // here to express "this widget is modal in its layer.")
    //
    // V32.32 (revision): ALSO flip the OUTER ModalSurface widget out
    // of transparent-mouse mode. This is critical. Qt's hit-testing
    // skips not just a transparent-for-mouse widget, but its ENTIRE
    // subtree — so clicks were passing straight through the modal
    // surface to the page underneath, never reaching the layer
    // containers or their presented widgets. Test-panel buttons got
    // no clicks. Discovered while testing on desktop with the layer
    // test panel.
    //
    // The outer surface stays transparent-for-mouse when no widget
    // wants to block (so empty layers don't eat clicks). Once ANY
    // widget asks to block, the surface becomes opaque to hit-tests
    // and the eventFilter on each container can do its job.
    if (blocksTouches) {
        ctx.container->setAttribute(Qt::WA_TransparentForMouseEvents, false);
        setAttribute(Qt::WA_TransparentForMouseEvents, false);
        // V32.32 (revision 2): ModalSurface is a sibling of MainWindow's
        // login overlay, offline banner, and toast widgets — all of
        // which call raise() on themselves at various points (offline
        // banner on connectivity change, toasts on appearance, etc.).
        // Each sibling raise() pushes that widget ABOVE ModalSurface
        // in z-order, meaning the upper portions of the screen now
        // have a non-modal widget on top of our modal stack. Clicks
        // on those areas bypass ModalSurface entirely.
        //
        // Fix: re-raise ourselves every time we present. This is cheap
        // (one Qt operation) and guarantees we're on top at the moment
        // of presentation. NOTE: we do NOT raise the layer container
        // among its siblings — that would break the L1<L2<L3<L4<L5<L6
        // ordering that the ctor establishes. Container raise only
        // matters at the outer (sibling-of-MainWindow-children) level.
        raise();
    }
    // Show scrim if this layer has one and this is the FIRST widget
    // presented into the layer.
    if (ctx.scrim && ctx.stack.size() == 1) {
        ctx.scrim->raise();        // scrim above older siblings
        ctx.scrim->show();
    }
    // Show the widget above the scrim. raise() makes it topmost
    // within the container's children.
    widget->raise();
    widget->show();

    SC_TRACE_S(Layout, lcModal) << "presented" << widget->metaObject()->className()
                    << "into" << layerName(layer)
                    << "stack-size" << ctx.stack.size();

    emit stackChanged();
    return widget;
}

void ModalSurface::dismissTopmost()
{
    // Walk layers from highest priority (Critical) to lowest (Ambient),
    // dismissing the newest-presented widget in the first non-empty layer.
    for (int i = static_cast<int>(Layer::Critical);
         i >= static_cast<int>(Layer::Ambient); --i) {
        const Layer layer = static_cast<Layer>(i);
        LayerCtx &ctx = m_layers[layer];
        // Find newest non-null entry.
        for (int j = ctx.stack.size() - 1; j >= 0; --j) {
            QWidget *w = ctx.stack[j];
            if (!w) continue;
            w->close();
            return;
        }
    }
}

void ModalSurface::dismissLayer(Layer layer)
{
    if (!m_layers.contains(layer)) return;
    LayerCtx &ctx = m_layers[layer];
    // Iterate a copy because close() may trigger removeFromStack which
    // mutates ctx.stack mid-loop.
    const auto copy = ctx.stack;
    for (const QPointer<QWidget> &p : copy) {
        if (p) p->close();
    }
}

void ModalSurface::dismissAll()
{
    for (auto it = m_layers.begin(); it != m_layers.end(); ++it) {
        dismissLayer(it.key());
    }
}

bool ModalSurface::isEmpty() const
{
    for (const auto &ctx : m_layers) {
        for (const QPointer<QWidget> &p : ctx.stack) {
            if (p) return false;
        }
    }
    return true;
}

QString ModalSurface::layerName(Layer layer)
{
    switch (layer) {
    case Layer::Ambient:  return QStringLiteral("Ambient");
    case Layer::Hint:     return QStringLiteral("Hint");
    case Layer::Dialog:   return QStringLiteral("Dialog");
    case Layer::Confirm:  return QStringLiteral("Confirm");
    case Layer::Toast:    return QStringLiteral("Toast");
    case Layer::Critical: return QStringLiteral("Critical");
    }
    return QStringLiteral("Unknown");
}

void ModalSurface::resizeEvent(QResizeEvent *e)
{
    QWidget::resizeEvent(e);
    // Propagate size to all layer containers + their scrims.
    for (auto it = m_layers.begin(); it != m_layers.end(); ++it) {
        if (it.value().container) {
            it.value().container->setGeometry(rect());
        }
        if (it.value().scrim) {
            it.value().scrim->setGeometry(it.value().container->rect());
        }
    }
}

bool ModalSurface::eventFilter(QObject *obj, QEvent *ev)
{
    // Tap-outside-to-dismiss: when the user taps on a layer container's
    // empty area (outside any presented widget), close the topmost
    // widget in that layer IF that widget has dismissOnTapOutside set.
    if (ev->type() != QEvent::MouseButtonPress
     && ev->type() != QEvent::TouchBegin) {
        return QWidget::eventFilter(obj, ev);
    }
    // Find which layer this container belongs to.
    QWidget *container = qobject_cast<QWidget *>(obj);
    if (!container) return QWidget::eventFilter(obj, ev);
    Layer hitLayer = Layer::Ambient;
    bool   found  = false;
    for (auto it = m_layers.constBegin(); it != m_layers.constEnd(); ++it) {
        if (it.value().container == container) {
            hitLayer = it.key();
            found    = true;
            break;
        }
    }
    if (!found) return QWidget::eventFilter(obj, ev);

    LayerCtx &ctx = m_layers[hitLayer];
    if (ctx.stack.isEmpty()) return QWidget::eventFilter(obj, ev);

    // Get the topmost non-null widget in this layer.
    QWidget *topmost = nullptr;
    for (int j = ctx.stack.size() - 1; j >= 0; --j) {
        if (ctx.stack[j]) { topmost = ctx.stack[j]; break; }
    }
    if (!topmost) return QWidget::eventFilter(obj, ev);

    // Was the tap inside the widget itself?
    QPoint localPt;
    if (ev->type() == QEvent::MouseButtonPress) {
        localPt = static_cast<QMouseEvent *>(ev)->pos();
    } else {
        const auto &pts = static_cast<QTouchEvent *>(ev)->points();
        if (pts.isEmpty()) return QWidget::eventFilter(obj, ev);
        localPt = pts.first().position().toPoint();
    }
    const bool inside = topmost->geometry().contains(localPt);
    // V32.32: Info-level diagnostic for the L4-can't-dismiss bug. Logs
    // EVERY click that reaches a layer container — both inside-widget
    // (which we let through) and outside-widget (which we dismiss or eat).
    // This is the only way to determine whether the click is reaching
    // the filter at all, what hitLayer it's attributed to, and what
    // geometry the topmost widget reports. If user reports "tap does
    // nothing" and there's NO log line here, the click isn't reaching
    // the filter — investigate hit-test path. If there IS a log line
    // saying "inside=1" when the user thinks they tapped outside, the
    // widget's geometry is wrong. If "inside=0" and we say "dismissing"
    // but the user still sees the scrim, close() isn't destroying.
    SC_TRACE_S(Layout, lcModal) << "filter HIT layer=" << layerName(hitLayer)
                    << "topmost=" << topmost->metaObject()->className()
                    << "topmost-geom=" << topmost->geometry()
                    << "click=" << localPt
                    << "inside=" << (inside ? 1 : 0)
                    << "stack-size=" << ctx.stack.size();
    if (inside) {
        // Tap on the widget — let the widget handle it normally.
        return QWidget::eventFilter(obj, ev);
    }
    // Tap on the scrim/empty area.
    if (m_dismissOnTap.value(topmost, false)) {
        SC_TRACE_S(Layout, lcModal) << "tap-outside dismiss"
                        << topmost->metaObject()->className()
                        << "from" << layerName(hitLayer)
                        << "deleteOnClose=" << (topmost->testAttribute(Qt::WA_DeleteOnClose) ? 1 : 0);
        topmost->close();
        return true; // consumed
    }
    SC_TRACE_S(Layout, lcModal) << "tap-outside NO-DISMISS (dismissOnTap=false)"
                    << "layer=" << layerName(hitLayer);
    // Non-dismissible — eat the event anyway so it doesn't fall through
    // to layers underneath (we already declared this widget modal).
    return true;
}

void ModalSurface::removeFromStack(QWidget *widget, Layer layer)
{
    if (!m_layers.contains(layer)) return;
    LayerCtx &ctx = m_layers[layer];
    const int before = ctx.stack.size();
    ctx.stack.removeAll(QPointer<QWidget>(widget));
    // Also remove any nulled-out entries left by other destroyed widgets.
    ctx.stack.removeAll(QPointer<QWidget>());

    m_widgetLayer.remove(widget);
    m_dismissOnTap.remove(widget);

    SC_TRACE_S(Layout, lcModal) << "removeFromStack layer=" << layerName(layer)
                    << "stack-size before=" << before
                    << "after=" << ctx.stack.size();

    // If the layer is now empty, restore transparent-mouse mode and
    // hide the scrim.
    if (ctx.stack.isEmpty()) {
        ctx.container->setAttribute(Qt::WA_TransparentForMouseEvents, true);
        if (ctx.scrim) ctx.scrim->hide();
        SC_TRACE_S(Layout, lcModal) << "layer" << layerName(layer)
                        << "now empty — container transparent, scrim hidden";
    }

    // V32.32 (revision): if EVERY layer is now empty, restore the outer
    // ModalSurface's transparent-for-mouse so events fall through to
    // the page content underneath. Without this, the modal surface
    // would remain opaque-to-hit-test even after all dialogs close,
    // and the user couldn't interact with the page below.
    bool anyBlocking = false;
    for (auto it = m_layers.constBegin(); it != m_layers.constEnd(); ++it) {
        for (const QPointer<QWidget> &p : it.value().stack) {
            if (p) { anyBlocking = true; break; }
        }
        if (anyBlocking) break;
    }
    if (!anyBlocking) {
        setAttribute(Qt::WA_TransparentForMouseEvents, true);
    }
}

// relayoutLayer reserved for future per-layer positioning policy;
// currently we let widgets manage their own geometry within the container.
void ModalSurface::relayoutLayer(Layer) {}

} // namespace SnapClone::UI
