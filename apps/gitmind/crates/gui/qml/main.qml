// gitmind main window — touch-first. Talks only to `backend` (the Rust
// FFI bridge); every panel renders the JSON a backend.*_json() returns.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: win
    visible: true
    width: 420
    height: 860
    title: "gitmind"
    color: "#0f1320"

    property var status: ({ ok: false, files: [] })
    property string repoRoot: ""
    property bool watchOn: false
    property string lastAutosave: ""

    function refresh() {
        repoRoot = backend.repo_root()
        status = JSON.parse(backend.status_json())
        timeline.reload()
    }
    Component.onCompleted: refresh()

    header: ToolBar {
        background: Rectangle { color: "#171c2b" }
        RowLayout {
            anchors.fill: parent
            anchors.margins: 8
            ColumnLayout {
                spacing: 0
                Label { text: "gitmind"; color: "#e6e9ef"; font.bold: true; font.pixelSize: 18 }
                Label {
                    text: status.ok ? ("on " + status.branch + (status.hasUpstream
                            ? "  \u2191" + status.ahead + " \u2193" + status.behind : ""))
                        : "no repository"
                    color: "#5ec8d8"; font.pixelSize: 11
                }
            }
            Item { Layout.fillWidth: true }
            Switch {
                id: attrSwitch
                text: "attr"
                ToolTip.visible: hovered
                ToolTip.text: "attribute each write to its process (fanotify)"
            }
            Switch {
                id: watchSwitch
                text: "watch"
                onToggled: {
                    if (checked) backend.watch_start(attrSwitch.checked, false)
                    else backend.watch_stop()
                    win.watchOn = checked
                }
            }
            ToolButton { text: "snapshot"; onClicked: { backend.snapshot(""); win.refresh() } }
            ToolButton { text: "\u21bb"; onClicked: win.refresh() }
        }
    }

    SwipeView {
        id: pages
        anchors.fill: parent
        currentIndex: 0

        // ── Status: swipe-to-stage rows ──────────────────────────────
        Item {
            ListView {
                id: fileList
                anchors.fill: parent
                anchors.margins: 8
                spacing: 8
                model: win.status.files
                delegate: FileRow {
                    width: fileList.width
                    fileName: modelData.path
                    xy: modelData.xy
                    staged: modelData.staged
                    untracked: modelData.untracked
                    onStageRequested: { backend.stage(modelData.path); win.refresh() }
                    onUnstageRequested: { backend.unstage(modelData.path); win.refresh() }
                    onPeekRequested: { viewer.open(modelData.path); pages.currentIndex = 3 }
                }
                Label {
                    anchors.centerIn: parent
                    visible: win.status.ok && win.status.files.length === 0
                    text: "clean"; color: "#8a93a6"
                }
            }
        }

        // ── Timeline ─────────────────────────────────────────────────
        Timeline { id: timeline }

        // ── Branches: native VCS, touch-first ────────────────────────
        Branches { id: branches }

        // ── File viewer (peek) ───────────────────────────────────────
        PeekViewer { id: viewer }
    }

    footer: TabBar {
        currentIndex: pages.currentIndex
        onCurrentIndexChanged: pages.currentIndex = currentIndex
        background: Rectangle { color: "#171c2b" }
        TabButton { text: "status" }
        TabButton { text: "timeline" }
        TabButton { text: "branches" }
        TabButton { text: "view" }
    }

    // Drain checkpoints the background watcher recorded; update the timeline
    // and flash a banner. The label carries provenance (e.g. [ai-agent:claude]).
    Timer {
        interval: 800; repeat: true; running: win.watchOn
        onTriggered: {
            var r = JSON.parse(backend.watch_drain())
            win.watchOn = r.running
            if (!r.running) watchSwitch.checked = false
            if (r.items && r.items.length > 0) {
                win.lastAutosave = r.items[r.items.length - 1].label
                banner.show()
                timeline.reload()
            }
        }
    }

    Rectangle {
        id: banner
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 10
        width: bannerText.implicitWidth + 28; height: 34; radius: 17
        color: "#1d3a24"; opacity: 0; z: 100
        function show() { opacity = 1; hideTimer.restart() }
        Behavior on opacity { NumberAnimation { duration: 250 } }
        Text {
            id: bannerText; anchors.centerIn: parent; color: "#56d364"
            font.pixelSize: 12; text: "auto-saved: " + win.lastAutosave
        }
        Timer { id: hideTimer; interval: 2000; onTriggered: banner.opacity = 0 }
    }
}
