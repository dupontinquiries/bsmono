#pragma once

// ─── UI/ComponentTestWindow.hpp ───────────────────────────────────────────────
//
// Development-only component test harness.
//
// Accessible from: Settings page → "🧪 Component Tests" button.
//
// Each tab renders one UI component in isolation so it can be visually
// inspected and interacted with without going through the full app flow.
// This catches layout breakage, missing translations, and edge-case data
// display bugs much earlier than integration tests.
//
// Tabs:
//   Login          — renders LoginWidget with error states
//   Inbox          — renders InboxWidget with synthetic snaps
//   Camera         — renders CameraWidget (live camera view)
//   Friends        — renders FriendListWidget with synthetic peers
//   Diagnostics    — renders DiagnosticsDialog content
//   Swipe          — renders SwipeNavigator with coloured placeholder pages
//   Theme Gallery  — shows all colours, typography, and button states
// ─────────────────────────────────────────────────────────────────────────────

#include <QMainWindow>
#include <QTabWidget>
#include "core/Types.hpp"

namespace SnapClone::UI {

class ComponentTestWindow : public QMainWindow
{
    Q_OBJECT

public:
    explicit ComponentTestWindow(QWidget *parent = nullptr);

private:
    void buildTabs();

    // ── Tab builders ──────────────────────────────────────────────────────
    QWidget *buildLoginTab();
    QWidget *buildInboxTab();
    QWidget *buildCameraTab();
    QWidget *buildFriendsTab();
    QWidget *buildSwipeTab();
    QWidget *buildThemeGalleryTab();

    // Helper: inject synthetic snaps for Inbox tab testing
    static QList<Core::Snap> makeSyntheticSnaps(int count);

    QTabWidget *m_tabs = nullptr;
};

} // namespace SnapClone::UI
