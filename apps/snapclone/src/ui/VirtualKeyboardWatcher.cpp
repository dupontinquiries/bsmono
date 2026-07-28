#include "ui/VirtualKeyboardWatcher.hpp"
#include "debug/Trace.hpp"   // per-event diagnostics: off unless the channel is on

#include <QGuiApplication>
#include <QInputMethod>
#include <QLoggingCategory>

namespace SnapClone::UI {

namespace {
Q_LOGGING_CATEGORY(lcVkb, "snapclone.ui.vkeyboard")
} // namespace

VirtualKeyboardWatcher &VirtualKeyboardWatcher::instance()
{
    static VirtualKeyboardWatcher s;
    return s;
}

VirtualKeyboardWatcher::VirtualKeyboardWatcher()
{
    // Connect to the application-wide input method. The QInputMethod
    // singleton is owned by QGuiApplication and outlives any
    // consumer — safe to subscribe in the ctor of a process-wide
    // singleton like this one.
    if (auto *im = QGuiApplication::inputMethod()) {
        connect(im, &QInputMethod::keyboardRectangleChanged,
                this, &VirtualKeyboardWatcher::onKeyboardRectangleChanged);
        // Capture initial state in case the keyboard is already showing
        // when we wire up (unusual, but the singleton is lazy so we
        // could attach after focus is on a text input).
        m_lastRect = im->keyboardRectangle().toAlignedRect();
        SC_TRACE_S(Layout, lcVkb) << "Watcher initialized; initial keyboard rect:" << m_lastRect;
    } else {
        qCWarning(lcVkb) << "No QInputMethod available — keyboard tracking inactive";
    }
}

bool VirtualKeyboardWatcher::isVisible() const
{
    return !m_lastRect.isEmpty();
}

QRect VirtualKeyboardWatcher::keyboardRect() const
{
    return m_lastRect;
}

void VirtualKeyboardWatcher::onKeyboardRectangleChanged()
{
    auto *im = QGuiApplication::inputMethod();
    if (!im) return;
    // QInputMethod reports QRectF in screen coords; we round to int
    // pixels via toAlignedRect — fine for layout work, and avoids
    // sub-pixel jitter from compositor reporting.
    const QRect newRect = im->keyboardRectangle().toAlignedRect();
    if (newRect == m_lastRect) return;  // de-dupe identical reports
    const bool wasVisible = !m_lastRect.isEmpty();
    const bool isVisibleNow = !newRect.isEmpty();
    m_lastRect = newRect;
    SC_TRACE_S(Layout, lcVkb) << "Keyboard rect changed:" << newRect
                  << "visible:" << isVisibleNow
                  << "(was" << wasVisible << ")";
    emit visibilityChanged(isVisibleNow, newRect);
}

} // namespace SnapClone::UI
