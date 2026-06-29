// Push — the desktop → phone direction. Pick a device, pick an app, and "Build &
// push": the app is built HERE for the device's target, the binary is sent over, and
// installed on the device (~/.local/bin + a .desktop launcher). The mirror image of
// the Fleet page's "build on the desktop, install here" flow.
import QtQuick
import QtQuick.Controls
import "applib.js" as App

Item {
    id: root

    // apps (shared scan with Fleet)
    property var apps: []
    property string search: ""
    property var shownApps: App.filterApps(apps, search, "app")

    // selected app
    property string selApp: ""
    property string selPath: ""
    property string selOid: ""
    property string selShot: ""

    // device + target
    property var devices: []
    property string selHost: ""
    property string selName: ""
    property bool hostTrusted: false
    property string user: "user"      // the account that will run the app on the device
    property string sshpass: ""
    property bool setupOpen: false
    property bool connectOpen: false
    property string target: "aarch64-unknown-linux-musl"

    // job state
    property bool busy: false
    property string jobKind: ""
    property string pushState: "idle" // idle | working | done | failed
    property string note: ""

    Component.onCompleted: root.loadApps()

    function loadApps() {
        var r = JSON.parse(backend.local_apps())
        root.apps = r.ok ? (r.apps || []) : []
    }
    function pickApp(a) {
        root.selApp = a.name; root.selPath = a.path; root.selOid = a.oid || ""
        root.selShot = a.shot || ""; root.pushState = "idle"; root.note = ""
    }
    function discover() {
        var r = JSON.parse(backend.start_discover(""))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "discover"; poller.start()
    }
    function selectDevice(ip, host) {
        root.selHost = ip; root.selName = (host && host.length) ? host : ip
        root.hostTrusted = JSON.parse(backend.host_trusted(ip)).trusted
        // auto-detect the device's target so the build matches its libc
        var t = JSON.parse(backend.remote_target(ip, root.user))
        if (t.ok && t.target && t.target.length) root.target = t.target
    }
    function refreshTrust() {
        if (root.selHost.length === 0) return
        root.hostTrusted = JSON.parse(backend.host_trusted(root.selHost)).trusted
    }
    function trustHost() {
        var r = JSON.parse(backend.start_trust(root.selHost))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "trust"; poller.start()
    }
    function setupKey() {
        var r = JSON.parse(backend.start_setup_key(root.user, root.selHost, root.sshpass))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "setupkey"; poller.start()
    }
    function doPush() {
        if (root.selApp.length === 0) { root.note = "pick an app first"; return }
        if (root.selHost.length === 0) { root.note = "pick a device first"; return }
        var r = JSON.parse(backend.push_install(root.selApp, root.selPath, root.target, "", root.selHost, root.user))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.pushState = "working"; root.busy = true; root.jobKind = "push"; root.note = ""; poller.start()
    }

    Timer {
        id: poller; interval: 250; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); root.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            var k = root.jobKind; root.jobKind = ""
            if (k === "discover") {
                root.devices = res.ok ? (res.devices || []) : []
            } else if (k === "trust") {
                root.note = res.ok ? "✓ host key trusted" : ("error: " + res.error); refreshTrust()
            } else if (k === "setupkey") {
                root.sshpass = ""; root.setupOpen = false
                root.note = res.ok ? "✓ key installed" : ("error: " + res.error); refreshTrust()
            } else if (k === "push") {
                if (res.ok) { root.pushState = "done"; root.note = "✓ installed " + res.app + " on " + res.host + " (+ launcher)" }
                else { root.pushState = "failed"; root.note = "✗ " + (res.stage ? res.stage + ": " : "") + res.error }
            }
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

            Row {
                width: parent.width
                Label { text: "Push to a device"; color: "#cdd6f4"; font.bold: true; font.pixelSize: 20; width: parent.width - rfBtn.width }
                Button { id: rfBtn; text: "↻"; onClicked: root.loadApps() }
            }
            Label {
                width: parent.width
                text: "Build an app here and install it on a phone — binary + launcher, no build toolchain needed on the device."
                color: "#5b6477"; font.pixelSize: 12; wrapMode: Text.WordWrap
            }

            // ── device ─────────────────────────────────────────────
            Rectangle {
                width: parent.width; radius: 10; color: "#171c2b"
                border.color: root.selHost.length ? "#7aa2f7" : "#2a3350"
                height: devC.height + 20
                Column {
                    id: devC
                    x: 12; y: 10; width: parent.width - 24; spacing: 8
                    Row {
                        width: parent.width; spacing: 8
                        Label {
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.selHost.length ? ("device: " + root.selName + (root.hostTrusted ? "  🔒" : "")) : "no device chosen"
                            color: root.selHost.length ? "#cdd6f4" : "#8a93a6"
                            width: parent.width - dscBtn.width - 12; elide: Text.ElideRight
                        }
                        Button { id: dscBtn; text: root.connectOpen ? "hide" : "choose…"; onClicked: root.connectOpen = !root.connectOpen }
                    }
                    Column {
                        width: parent.width; spacing: 8; visible: root.connectOpen
                        Row {
                            spacing: 8
                            Button { text: root.busy ? "…" : "scan LAN"; enabled: !root.busy; onClicked: root.discover() }
                            Label { text: "as user"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                            TextField { width: 110; text: root.user; onTextChanged: root.user = text }
                        }
                        Repeater {
                            model: root.devices
                            delegate: Rectangle {
                                width: parent.width; height: 42; radius: 8
                                color: modelData.ip === root.selHost ? "#2a3350" : "#1b2335"
                                border.color: modelData.ip === root.selHost ? "#7aa2f7" : "#2a3350"
                                Column {
                                    anchors.verticalCenter: parent.verticalCenter; x: 10
                                    Label { text: (modelData.hostname && modelData.hostname.length) ? modelData.hostname : modelData.ip; color: "#cdd6f4"; font.bold: true }
                                    Label { text: modelData.ip; color: "#8a93a6"; font.pixelSize: 11 }
                                }
                                TapHandler { onTapped: root.selectDevice(modelData.ip, modelData.hostname) }
                            }
                        }
                        Column {
                            width: parent.width; spacing: 6; visible: root.selHost.length > 0
                            Row {
                                spacing: 8
                                Label {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: root.hostTrusted ? "🔒 trusted" : "host key not trusted"
                                    color: root.hostTrusted ? "#56d364" : "#f9e2af"; font.pixelSize: 12
                                }
                                Button { text: "trust key"; visible: !root.hostTrusted; enabled: !root.busy; onClicked: root.trustHost() }
                                Button { text: root.setupOpen ? "hide setup" : "set up access"; enabled: !root.busy; onClicked: root.setupOpen = !root.setupOpen }
                            }
                            Column {
                                width: parent.width; spacing: 6; visible: root.setupOpen
                                Label {
                                    width: parent.width; color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                                    text: "Install your key for '" + root.user + "' on the device so pushes are passwordless."
                                }
                                TextField {
                                    width: parent.width; placeholderText: "device password (used once, not stored)"
                                    echoMode: TextInput.Password; text: root.sshpass; onTextChanged: root.sshpass = text
                                }
                                Button { text: "install my key"; enabled: !root.busy && root.sshpass.length > 0; onClicked: root.setupKey() }
                            }
                        }
                    }
                }
            }

            // ── app search + grid ──────────────────────────────────
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

            // ── push bar ────────────────────────────────────────────
            Rectangle {
                width: parent.width; radius: 10; color: "#171c2b"
                border.color: (root.selApp.length && root.selHost.length) ? "#7aa2f7" : "#2a3350"
                opacity: (root.selApp.length && root.selHost.length) ? 1.0 : 0.5
                height: pushC.height + 20
                Column {
                    id: pushC
                    x: 12; y: 10; width: parent.width - 24; spacing: 8
                    Label { text: "Build & push", color: "#7aa2f7"; font.bold: true }
                    Label {
                        width: parent.width; color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                        text: root.selApp.length ? ("push " + root.selApp + (root.selHost.length ? (" → " + root.selName) : "")) : "Pick a device and an app above."
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
                                TapHandler { onTapped: root.target = modelData }
                            }
                        }
                    }
                    Button {
                        text: root.pushState === "working" ? "working…" : (root.pushState === "done" ? "pushed ✓" : "Build & push")
                        enabled: !root.busy && root.selApp.length > 0 && root.selHost.length > 0 && root.hostTrusted
                        onClicked: root.doPush()
                    }
                    Label {
                        width: parent.width; visible: root.selHost.length > 0 && !root.hostTrusted
                        text: "Trust the device + install your key above first."
                        color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap
                    }
                }
            }
        }
    }
}
