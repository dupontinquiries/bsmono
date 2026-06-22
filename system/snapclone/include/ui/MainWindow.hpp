#pragma once

// ─── UI/MainWindow.hpp ────────────────────────────────────────────────────────
//
// Top-level application window.
//
// Layout (SwipeNavigator pages, left → right):
//   [0] Camera   [1] Inbox   [2] Friends   [3] Settings
//
// Login overlay behaviour:
//   • Created HIDDEN (explicit hide() in buildUi).
//   • Only shown if auto-login has not yet completed by construction time.
//     In normal operation auto-login completes inside AppController::initialize()
//     which runs before MainWindow is constructed, so the overlay is never shown.
//   • Camera starts in showEvent() (first paint) via a zero-duration QTimer
//     to avoid a black-frame flash on some V4L2 drivers.
//
// m_cameraStarted guards against double-starting the camera from both
// showEvent and pageChanged signals.
// ─────────────────────────────────────────────────────────────────────────────

#include <QMainWindow>
#include <QResizeEvent>
#include <QShowEvent>
#include <QLabel>
#include <QSet>
#include <optional>
#include "core/Types.hpp"

namespace SnapClone::Core { class AppController; }

namespace SnapClone::UI {

class SwipeNavigator;
class TabBar;
class UnifiedNavBar;  // V32.34+: unified bottom bar
class LoginWidget;
class InboxWidget;
class CameraWidget;
class ContactsPage;
class SettingsPage;
class WayfinderPage;
class PostureController;  // V32.42+: proximity screensaver + gated talk-mode
class ModalSurface;  // V32.32: unified overlay/dialog/toast layer manager

class MainWindow : public QMainWindow
{
    Q_OBJECT

public:
    explicit MainWindow(Core::AppController *controller, QWidget *parent = nullptr);

protected:
    void showEvent  (QShowEvent   *e) override;
    void resizeEvent(QResizeEvent *e) override;

private slots:
    void showSwipeHintIfFirstRun();
    void showHintToast(const QString &key, const QString &title, const QString &body);
    void showCelebration(const QString &key, const QString &title, const QString &body);
    void dismissSwipeHint();
    /// V32.34+ monthly export: save-dialog → SnapExporter → result toast.
    void runSnapExport();

private:
    // Offline banner shown when device has no network connection.
    // Null when online. Parented to MainWindow, destroyed on reconnect.
    QLabel *m_offlineBanner = nullptr;
    void repositionOfflineBanner();
    void buildUi();
    void connectSignals();
    void hideLoginOverlay();

    Core::AppController *m_controller    = nullptr; // non-owning

    // All widgets owned by Qt parent-child tree:
    SwipeNavigator      *m_swiper        = nullptr;
    LoginWidget         *m_loginOverlay  = nullptr;
    CameraWidget        *m_cameraPage    = nullptr;
    InboxWidget         *m_inboxPage     = nullptr;
    ContactsPage        *m_friendsPage      = nullptr;
    QWidget             *m_swipeHint         = nullptr;
    QWidget             *m_edgeHint          = nullptr; // SwipeEdgeHint
    SettingsPage        *m_settingsPage   = nullptr;
    WayfinderPage       *m_wayfinderPage  = nullptr;
    TabBar              *m_tabBar         = nullptr;
    UnifiedNavBar       *m_unifiedBar     = nullptr;  // V32.34+: SC_UNIFIED_NAV bottom bar
    /// V32.30: pulsing color overlay parented to the wrapper QWidget.
    /// Stays sized to match wrapper via eventFilter (handles QResizeEvent
    /// on the wrapper). No-op paint when no synthetic palette is active.
    QWidget             *m_ambientBlobs   = nullptr;
    PostureController   *m_postureController = nullptr;  // V32.42+
    /// V32.30: ambient-camera-mode driver. Owns a 1Hz timer that
    /// re-derives the synthetic palette from the live viewfinder frame.
    /// Null until first activation; started by SettingsPage when the
    /// user picks "Ambient: Camera" from the theme carousel.
    QObject             *m_ambientCameraDriver = nullptr;
    /// V32.30 (Wave 2): ambient-inbox-mode driver. Subscribes to
    /// AppController::snapReceived and re-derives the palette (30s
    /// throttled) so the chrome evolves as new snaps arrive. Null
    /// until first activation; started by SettingsPage when the user
    /// picks "Ambient: Inbox" from the theme carousel.
    QObject             *m_ambientInboxDriver  = nullptr;

    /// V32.32: unified overlay/dialog/toast/confirm layer manager.
    /// Sits at MainWindow level (peer of m_loginOverlay, m_offlineBanner)
    /// so its internal layers can occlude EVERYTHING when critical modals
    /// are presented. Phase 1 of the modal-layer refactor — see
    /// include/ui/ModalSurface.hpp for the layer model.
    ModalSurface        *m_modalSurface        = nullptr;

protected:
    /// V32.30: track wrapper resize so AmbientBlobs stays full-canvas.
    bool eventFilter(QObject *watched, QEvent *event) override;

private:

    // Prevents the camera being started more than once (showEvent + pageChanged
    // can both fire early in the widget lifecycle).
    bool                 m_cameraStarted = false;
    // Maps peer sc-ID → set of outbound snap IDs for fast status routing.
    // V32.34+: also drives the per-contact "⬆ N queued" pill on the
    // Contacts page — on send we insert + push the count; on snapDelivered
    // we find the owning peer, erase the id, and push the decremented count.
    QHash<QString, QSet<QString>> m_outboundSnapsByPeer;
    /// Set of currently-online peer IDs. Updated by AppController
    /// peerConnected/peerDisconnected signals. Used by the send-status
    /// messaging to differentiate "sent" from "queued for offline peer".
    QSet<QString>                  m_onlinePeers;

    /// V32.15: transient recipient for the in-flight snap capture.
    /// Set when the user picks a contact in ContactPickerDialog, consumed
    /// when the picker queues a re-emit of snapCaptured / videoSnapCaptured
    /// that re-enters the send handler. Cleared on send completion or
    /// picker rejection. Photo and video captures can share this slot
    /// because they don't fire concurrently (a single shutter gesture
    /// emits exactly one of the two signals).
    std::optional<Core::UserId>    m_pendingSnapRecipient;

    void showInboundApprovalBanner(const Core::UserId &peerId,
                                    const QString &displayName);
    bool                 m_inboxLoaded   = false; // guard for one-time DB restore
};

} // namespace SnapClone::UI
