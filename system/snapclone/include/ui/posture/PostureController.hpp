#pragma once
// ─── ui/posture/PostureController.hpp ─────────────────────────────────────────
//
// Window-level glue for the posture engine. Owns the PostureEngine, the
// DimOverlay, and the (real) PostureSource, wires them together, and decides when
// the whole thing is armed:
//
//     armed = master-enabled  AND  on the inbox page  AND  voice in the last 5 min
//
// Conversations open as dialogs over the inbox page, so "on the inbox page"
// covers both the mailbox and any open 1:1/group conversation. Sensors are only
// claimed while armed. The PTT↔gated request is re-emitted for the app to route
// to whatever conversation is currently in a call.
//
// Config is read from QSettings under "posture/" (with the struct defaults as
// fallback) — that's the presets file for now; a builder UI can write the same
// keys later.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include "ui/posture/PostureEngine.hpp"

class QWidget;

namespace SnapClone::UI {

class DimOverlay;
class PostureSource;
class SwipeNavigator;

class PostureController : public QObject {
    Q_OBJECT
public:
    // overlayParent: the widget the dim overlay should cover (e.g. the central
    // widget). swiper: used to read the current page.
    PostureController(QWidget *overlayParent, SwipeNavigator *swiper, QObject *parent = nullptr);

    void setEnabled(bool on);
    [[nodiscard]] bool enabled() const { return m_enabled; }
    [[nodiscard]] PostureEngine *engine() const { return m_engine; }

public Q_SLOTS:
    /// A snap arrived — wake the screen if dimmed. (Wiring genuine new-snap
    /// detection is a follow-up; the engine already supports this.)
    void notifySnap();

Q_SIGNALS:
    /// Re-emitted from the engine: route to the in-call conversation's talk mode.
    void gatedModeRequested(bool gated);
    /// Entered/left screensaver — a seam for capping the camera/preview FPS.
    void screensaverChanged(bool active);

private Q_SLOTS:
    void recomputeArmed();

private:
    void loadConfigFromSettings();

    PostureEngine  *m_engine  = nullptr;
    DimOverlay     *m_overlay = nullptr;
    PostureSource  *m_source  = nullptr;
    SwipeNavigator *m_swiper  = nullptr;
    bool m_enabled = true;
    bool m_armed   = false;
};

} // namespace SnapClone::UI
