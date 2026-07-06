// Push — the desktop → phone direction. Pick a device, pick an app, "Build & push":
// the app builds HERE for the device's target, the binary is sent over, and installed
// on the device (~/.local/bin + a .desktop launcher). Mirror of Fleet's fetch flow.
import QtQuick
import QtQuick.Controls
import "applib.js" as App

Item {
    id: root

    property var apps: []
    property string search: ""
    property var shownApps: App.filterApps(apps, search, "app")

    property string selApp: ""
    property string selPath: ""
    property string selOid: ""
    property string target: "aarch64-unknown-linux-musl"
    property string strategyName: ""
    property string strategyReason: ""

    // build features (cargo [features]) for the selected app
    property var features: []
    property var defaultsByTarget: ({})
    property var chosen: []
    property bool isCargo: false

    property bool busy: false
    property string jobKind: ""
    property string pushState: "idle" // idle | working | done | failed
    property string buildLog: ""
    property int logIdx: 0
    property string note: ""

    Component.onCompleted: root.loadApps()

    function loadApps() {
        var r = JSON.parse(backend.local_apps())
        root.apps = r.ok ? (r.apps || []) : []
    }
    function pickApp(a) {
        root.selApp = a.name; root.selPath = a.path; root.selOid = a.oid || ""
        root.pushState = "idle"; root.note = ""
        root.loadOptions()
    }
    function loadOptions() {
        root.features = []; root.chosen = []; root.defaultsByTarget = ({}); root.isCargo = false
        if (root.selPath.length === 0) return
        var r = JSON.parse(backend.local_build_options(root.selPath))
        if (!r.ok) return
        root.features = r.features || []
        root.defaultsByTarget = r.defaults_by_target || ({})
        root.isCargo = r.is_cargo === true
        root.applyDefaults()
    }
    function applyDefaults() {
        var d = root.defaultsByTarget[root.target]
        root.chosen = d ? d.slice() : []
    }
    function toggleFeature(f) {
        var c = root.chosen.slice()
        var i = c.indexOf(f)
        if (i >= 0) c.splice(i, 1); else c.push(f)
        root.chosen = c
    }
    function setTarget(t) {
        root.target = t
        root.applyDefaults()
        root.loadStrategy()
    }
    // ask the backend how it'll build for this target+device, and why
    function loadStrategy() {
        if (root.target.length === 0) { root.strategyName = ""; return }
        var reachable = picker.selHost.length > 0
        var r = JSON.parse(backend.deploy_strategy(root.target, reachable))
        if (r.ok) { root.strategyName = r.strategy; root.strategyReason = r.reason }
        else { root.strategyName = ""; root.strategyReason = "" }
    }
    // when a device is chosen, default the build target to its libc
    function detectTarget() {
        if (picker.selHost.length === 0) return
        var t = JSON.parse(backend.remote_target(picker.selHost, picker.user))
        if (t.ok && t.target && t.target.length) { root.target = t.target; root.applyDefaults() }
        root.loadStrategy()
    }
    function doPush() {
        if (root.selApp.length === 0) { root.note = "pick an app first"; return }
        if (picker.selHost.length === 0) { root.note = "pick a device first"; return }
        var r = JSON.parse(backend.push_install(root.selApp, root.selPath, root.target, root.chosen.join(","), picker.selHost, picker.user))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.pushState = "working"; root.busy = true; root.jobKind = "push"; root.note = ""; root.buildLog = ""; root.logIdx = 0; poller.start()
    }

    Timer {
        id: poller; interval: 250; repeat: true; running: false
        onTriggered: {
            var lg = JSON.parse(backend.job_log(root.logIdx))
            if (lg.lines && lg.lines.length) {
                root.logIdx = lg.next
                for (var i = 0; i < lg.lines.length; i++) root.buildLog += lg.lines[i] + "\n"
            }
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); root.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            if (root.jobKind === "push") {
                if (res.ok) { root.pushState = "done"; root.note = "✓ installed " + res.app + " on " + res.host + " (+ launcher)" }
                else { root.pushState = "failed"; root.note = "✗ " + (res.stage ? res.stage + ": " : "") + res.error }
            }
            root.jobKind = ""
        }
    }

    Flickable {
        anchors.fill: parent; anchors.margins: 12
        contentHeight: col.height + 24; clip: true
        Column {
            id: col
            width: parent.width; spacing: 12

            Label {
                width: parent.width; visible: root.note.length > 0
                text: root.note; color: "#a6adc8"; font.pixelSize: 12; wrapMode: Text.WrapAnywhere
            }

            // ── live remote build log (streamed over SSH) ──
            Rectangle {
                visible: root.buildLog.length > 0
                width: parent.width; radius: 10; color: "#0d1117"; border.color: "#3a4566"
                height: 220
                Column {
                    anchors.fill: parent; anchors.margins: 8; spacing: 4
                    Row {
                        width: parent.width; spacing: 8
                        Label { text: root.busy ? "building on device…" : "build log"; color: "#7aa2f7"; font.bold: true }
                        Item { width: parent.width - pCopy.width - pClear.width - 150; height: 1 }
                        Button { id: pCopy; text: "copy"; onClicked: { pLog.selectAll(); pLog.copy(); pLog.deselect() } }
                        Button { id: pClear; text: "clear"; enabled: !root.busy; onClicked: root.buildLog = "" }
                    }
                    ScrollView {
                        width: parent.width; height: parent.height - 34; clip: true
                        TextArea {
                            id: pLog
                            readOnly: true; wrapMode: TextArea.NoWrap
                            text: root.buildLog
                            color: "#c9d1d9"; font.family: "monospace"; font.pixelSize: 11
                            selectByMouse: true
                            background: Rectangle { color: "transparent" }
                            onTextChanged: cursorPosition = length
                        }
                    }
                }
            }
            Row {
                width: parent.width
                Label { text: "Push to a device"; color: "#cdd6f4"; font.bold: true; font.pixelSize: 20; width: parent.width - rfBtn.width }
                Button { id: rfBtn; text: "↻"; onClicked: root.loadApps() }
            }
            Label {
                width: parent.width
                text: "Build an app here and install it on a phone — binary + launcher, no toolchain needed on the device."
                color: "#5b6477"; font.pixelSize: 12; wrapMode: Text.WordWrap
            }

            DevicePicker {
                id: picker; width: parent.width; user: "user"
                onSelected: root.detectTarget()
            }

            TextField {
                width: parent.width; placeholderText: "search apps…"
                text: root.search; onTextChanged: root.search = text
            }
            Label {
                width: parent.width; visible: root.shownApps.length === 0
                text: "No apps match."; color: "#5b6477"; font.pixelSize: 12
            }
            Repeater {
                model: root.shownApps
                delegate: Rectangle {
                    width: col.width; radius: 10; height: 60
                    color: modelData.name === root.selApp ? "#23304d" : "#1b2335"
                    border.color: modelData.name === root.selApp ? "#7aa2f7" : "#3a4566"
                    Row {
                        anchors.verticalCenter: parent.verticalCenter; x: 12; spacing: 10; width: parent.width - 24
                        Item {
                            width: 40; height: 40; anchors.verticalCenter: parent.verticalCenter
                            Image {
                                anchors.fill: parent; visible: modelData.shot.length > 0
                                source: modelData.shot.length ? "file://" + modelData.shot : ""
                                fillMode: Image.PreserveAspectCrop; cache: false; asynchronous: true
                            }
                            MonoTile { anchors.fill: parent; visible: modelData.shot.length === 0; appName: modelData.name; oid: modelData.oid }
                        }
                        Column {
                            anchors.verticalCenter: parent.verticalCenter; width: parent.width - 50
                            Label { text: modelData.name; color: "#cdd6f4"; font.bold: true; font.pixelSize: 15 }
                            Label {
                                width: parent.width
                                text: modelData.description.length ? modelData.description : (modelData.is_cargo ? "cargo crate" : "cmake project")
                                color: "#8a93a6"; font.pixelSize: 12; elide: Text.ElideRight; maximumLineCount: 1
                            }
                        }
                    }
                    TapHandler { onTapped: root.pickApp(modelData) }
                }
            }

            Rectangle {
                width: parent.width; radius: 10; color: "#171c2b"
                border.color: (root.selApp.length && picker.selHost.length) ? "#7aa2f7" : "#2a3350"
                opacity: (root.selApp.length && picker.selHost.length) ? 1.0 : 0.5
                height: pushC.height + 20
                Column {
                    id: pushC
                    x: 12; y: 10; width: parent.width - 24; spacing: 8
                    Label { text: "Build & push"; color: "#7aa2f7"; font.bold: true }
                    Label {
                        width: parent.width; color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                        text: root.selApp.length ? ("push " + root.selApp + (picker.selHost.length ? (" → " + picker.selName) : "")) : "Pick a device and an app above."
                    }
                    Row {
                        spacing: 8
                        Label { text: "for"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                        Repeater {
                            model: ["aarch64-unknown-linux-musl", "aarch64-unknown-linux-gnu"]
                            delegate: Rectangle {
                                height: 28; radius: 14; width: tl.width + 20
                                color: modelData === root.target ? "#2a3350" : "#1b2335"
                                border.color: modelData === root.target ? "#7aa2f7" : "#3a4566"
                                Label { id: tl; anchors.centerIn: parent; text: App.targetLibc(modelData); color: "#cdd6f4"; font.pixelSize: 12 }
                                TapHandler { onTapped: root.setTarget(modelData) }
                            }
                        }
                    }
                    // cargo feature toggles, defaulted to the chosen target's default set
                    Column {
                        width: parent.width; spacing: 6
                        visible: root.isCargo && root.features.length > 0
                        Label { text: "features"; color: "#8a93a6"; font.pixelSize: 12 }
                        Flow {
                            width: parent.width; spacing: 6
                            Repeater {
                                model: root.features
                                delegate: Rectangle {
                                    height: 28; radius: 14; width: fl.width + 20
                                    property bool on: root.chosen.indexOf(modelData) >= 0
                                    color: on ? "#2a3350" : "#1b2335"
                                    border.color: on ? "#7aa2f7" : "#3a4566"
                                    Label { id: fl; anchors.centerIn: parent; text: modelData; color: on ? "#cdd6f4" : "#8a93a6"; font.pixelSize: 12 }
                                    TapHandler { onTapped: root.toggleFeature(modelData) }
                                }
                            }
                        }
                        Label {
                            width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap
                            text: root.chosen.length ? ("building with: " + root.chosen.join(", ")) : "no features (—no-default-features)"
                        }
                    }
                    Label {
                        width: parent.width
                        visible: root.strategyName.length > 0
                        wrapMode: Text.WordWrap; font.pixelSize: 11
                        color: root.strategyName === "remote-native" ? "#e0af68" : "#7aa2f7"
                        text: "deploy: " + root.strategyName + " — " + root.strategyReason
                    }
                    Button {
                        text: root.pushState === "working" ? "working…" : (root.pushState === "done" ? "pushed ✓" : "Build & push")
                        enabled: !root.busy && !picker.busy && root.selApp.length > 0 && picker.selHost.length > 0 && picker.hostTrusted
                        onClicked: root.doPush()
                    }
                    Label {
                        width: parent.width; visible: picker.selHost.length > 0 && !picker.hostTrusted
                        text: "Trust the device + install your key above first."
                        color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap
                    }
                }
            }
        }
    }
}
