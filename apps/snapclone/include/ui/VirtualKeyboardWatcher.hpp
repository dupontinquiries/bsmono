#pragma once

// ─── ui/VirtualKeyboardWatcher.hpp ───────────────────────────────────────────
//
// Watches the platform on-screen keyboard (squeekboard on Phosh/postmarketOS,
// SwiftKey/GBoard on Android etc) and emits a signal whenever its visibility
// or rectangle changes. Lets focused text inputs scroll themselves into
// view so the user can SEE what they're typing.
//
// Why this exists:
//   On mobile, when a text input gets focus, the platform may pop up a
//   keyboard that covers the bottom 40-50% of the screen. If the input
//   the user JUST FOCUSED is in that bottom half, it's now hidden under
//   the keyboard — the user types into something they can't see.
//
//   Native apps handle this by inset-padding their root layout, or by
//   scrolling the focused widget's containing scroll area to bring it
//   above the keyboard rect. Qt provides the platform notification
//   (QInputMethod::keyboardRectangleChanged) but no built-in widget
//   that ACTS on it for QtWidgets. This class is that.
//
// Wayland input-method protocol details:
//   On Phosh/postmarketOS with squeekboard, the keyboard fires
//   text-input-v3 / input-method-unstable-v2 events that Qt's wayland
//   platform plugin translates into QInputMethod state changes. The
//   keyboardRectangle is in global (screen) coordinates.
//
//   On compositors that don't support these protocols (older Sway,
//   pre-43 GNOME, anything pre-Wayland), keyboardRectangle stays empty
//   and we don't do anything — the keyboard either doesn't exist or
//   is presented OUTSIDE the application's coordinate space (e.g. an
//   X11 floating keyboard the user can move themselves).
//
// Usage:
//   • Connect once to VirtualKeyboardWatcher::instance().visibilityChanged
//     in some long-lived object (main window, app controller).
//   • In the slot, find the focused widget and either:
//       (a) Call QScrollArea::ensureWidgetVisible(focused) on its
//           containing scroll area (preferred — least disruptive)
//       (b) Resize / move the relevant container if no scroll area
//
//   The class itself does NOT modify any UI — it's purely an observer.
//   The decision of HOW to react belongs to the consumer; one widget
//   might prefer to scroll, another might prefer to shrink.
//
// Why a singleton:
//   QInputMethod is per-QGuiApplication; multiple watchers would all
//   subscribe to the same signal. The singleton avoids redundant
//   listeners and gives a single point of subscription that any
//   widget can hook into. Lifetime is tied to the app via a
//   function-local static, like other process-wide singletons in
//   this codebase (CameraHealthService, CameraPolicyStore).

#include <QObject>
#include <QRect>

namespace SnapClone::UI {

class VirtualKeyboardWatcher : public QObject
{
    Q_OBJECT
public:
    /// Process-wide accessor. Lazily constructed on first call; safe to
    /// invoke from anywhere on the GUI thread.
    static VirtualKeyboardWatcher &instance();

    /// Last known visibility. True when the platform reports a non-empty
    /// keyboard rectangle.
    [[nodiscard]] bool isVisible() const;

    /// Last known rectangle in global (screen) coordinates. Empty
    /// QRect when the keyboard is not shown OR the platform doesn't
    /// report one.
    [[nodiscard]] QRect keyboardRect() const;

signals:
    /// Emitted when the platform's keyboardRectangle changes. The
    /// `visible` parameter is convenience: it's `!rect.isEmpty()`.
    void visibilityChanged(bool visible, const QRect &rect);

private:
    VirtualKeyboardWatcher();

    /// Forwarded from QInputMethod::keyboardRectangleChanged. Caches
    /// the new rect and re-emits via our own signal (so consumers
    /// don't have to know about QInputMethod).
    void onKeyboardRectangleChanged();

    QRect m_lastRect;
};

} // namespace SnapClone::UI
