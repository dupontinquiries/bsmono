import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import QtQuick.Controls.Material 2.15
import "."
import "Theme.js" as Theme

ApplicationWindow {
    id: window
    visible: true
    width: 420
    height: 740
    title: "Ambient Learner"
    color: Theme.background

    Material.theme: Material.Dark
    Material.accent: Theme.accent
    Material.background: Theme.background

    readonly property bool isMobile: width < 500

    // Drive `backend.poll_updates()` at 5 Hz.
    Timer {
        interval: 200
        running: true
        repeat: true
        onTriggered: backend.poll_updates()
    }

    // ─── Animated background (mood + tilt + light + pressure) ─────────────
    AnimatedBackground {
        anchors.fill: parent
        season:        backend.season
        phase:         backend.day_phase
        tiltX:         backend.tilt_x
        tiltY:         backend.tilt_y
        lightNorm:     backend.light_norm
        pressureTrend: backend.pressure_trend_hpa
    }

    // ─── Humidity dew sparkles (only visible if there's a humidity sensor)
    HumiditySparkles {
        anchors.fill: parent
        z: 0
        humidityPct: backend.has_humidity ? backend.humidity_pct : 0
    }

    // ─── Pages ──────────────────────────────────────────────────────────
    // Debug pages are hidden by default. Set `AMBIENT_DEBUG=1` in
    // the environment to enable them — useful while developing the
    // screen-temp publisher, the kernel placement, etc. We read
    // the env var via the backend so we don't have to plumb a
    // separate config object through QML.
    readonly property bool debugEnabled: backend.debug_enabled

    SwipeView {
        id: swipeView
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: navBar.top
        currentIndex: 0
        interactive: true

        LivePage      { isMobile: window.isMobile }
        ModelPage     { isMobile: window.isMobile }
        HistoryPage   { isMobile: window.isMobile }
        LocationsPage { isMobile: window.isMobile }
        MapPage       { isMobile: window.isMobile }
        SyncPage      { isMobile: window.isMobile }

        // The debug screen-temp page only exists when AMBIENT_DEBUG=1.
        // SwipeView quietly skips children whose `visible` is false,
        // so this keeps the layout clean in release builds.
        Loader {
            active: window.debugEnabled
            visible: active
            sourceComponent: DebugScreenTempsPage { isMobile: window.isMobile }
        }
    }

    // ─── Bottom nav ─────────────────────────────────────────────────────
    Rectangle {
        id: navBar
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        height: window.isMobile ? 64 : 56
        color: Qt.rgba(0.10, 0.10, 0.18, 0.85)

        // Subtle top border that picks up the seasonal accent
        Rectangle {
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            height: 1
            color: Theme.moodAccent(backend.season)
            opacity: 0.25
            Behavior on color { ColorAnimation { duration: Theme.durSlow } }
        }

        RowLayout {
            anchors.fill: parent
            spacing: 0

            Repeater {
                // We build the nav list dynamically so the optional
                // debug tab only appears when enabled. Pure JS array;
                // QML re-evaluates it when `debugEnabled` flips.
                model: {
                    var tabs = [
                        { icon: "🌡️", label: "Live",      color: Theme.sectionLive      },
                        { icon: "🧠",  label: "Model",     color: Theme.sectionModel     },
                        { icon: "📊",  label: "History",   color: Theme.sectionHistory   },
                        { icon: "📍",  label: "Locations", color: Theme.sectionLocations },
                        { icon: "🗺",  label: "Map",       color: Theme.sectionLocations },
                        { icon: "📡",  label: "Sync",      color: Theme.sectionSync      }
                    ];
                    if (window.debugEnabled) {
                        tabs.push({ icon: "🛠", label: "Debug",
                                    color: Theme.sectionModel });
                    }
                    return tabs;
                }
                delegate: ItemDelegate {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    onClicked: swipeView.currentIndex = index

                    background: Rectangle {
                        color: swipeView.currentIndex === index
                               ? Theme.surfaceSelected
                               : "transparent"
                        Behavior on color { ColorAnimation { duration: Theme.durSnappy } }
                    }

                    contentItem: ColumnLayout {
                        spacing: 2
                        Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: modelData.icon
                            font.pixelSize: window.isMobile ? 22 : 18
                            scale: swipeView.currentIndex === index ? 1.15 : 1.0
                            Behavior on scale { NumberAnimation { duration: Theme.durSnappy; easing.type: Easing.OutBack } }
                        }
                        Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: modelData.label
                            font.pixelSize: 10
                            color: swipeView.currentIndex === index
                                   ? modelData.color
                                   : Theme.textDim
                            Behavior on color { ColorAnimation { duration: Theme.durSnappy } }
                        }
                    }
                }
            }
        }
    }
}
