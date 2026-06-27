// Deploy cockpit — touch-first.
//   1. discover  → scan the LAN for SSH devices (or type a subnet)
//   2. pick one  → browse its filesystem and choose a target folder
//   3. push      → rsync the monorepo there
//   4. launch    → build an app on the device + make a .desktop (Plasma Mobile)
// Every action calls a backend deploy method and renders the JSON result.
import QtQuick
import QtQuick.Controls

Item {
    id: root
    property string subnet: ""
    property var devices: []
    property string selHost: ""   // IP we ssh to
    property string selName: ""   // hostname for display
    property string user: "user"
    property string curPath: ""   // resolved absolute folder on the device
    property var dirs: []
    property string app: "voiceterm"
    property string note: ""
    property bool busy: false

    function discover() {
        busy = true; note = "scanning the LAN for SSH…"
        var r = JSON.parse(backend.discover_devices(root.subnet))
        busy = false
        if (r.ok) { devices = r.devices; note = devices.length + " device(s) on " + r.subnet + ".0/24" }
        else { devices = []; note = "error: " + r.error }
    }
    function selectDevice(ip, host) {
        selHost = ip; selName = (host && host.length) ? host : ip
        browse("")
    }
    function browse(path) {
        if (selHost.length === 0) return
        var r = JSON.parse(backend.list_remote_dirs(root.user, root.selHost, path))
        if (r.ok) { curPath = r.path; dirs = r.dirs }
        else { note = "error: " + r.error }
    }
    function parentOf(p) { var i = p.lastIndexOf("/"); return i > 0 ? p.substring(0, i) : "/" }
    function pushHere() {
        busy = true; note = "pushing the monorepo to " + curPath + "…"
        var r = JSON.parse(backend.push_repo(root.user, root.selHost, root.curPath))
        busy = false; note = r.ok ? r.msg : ("error: " + r.error)
    }
    function deployHere() {
        busy = true; note = "building " + app + " on " + selName + " — this can take a while…"
        var r = JSON.parse(backend.deploy_app(root.user, root.selHost, root.curPath, root.app))
        busy = false; note = r.ok ? ("✓ " + r.msg) : ("error: " + r.error)
    }

    Flickable {
        anchors.fill: parent
        anchors.margins: 8
        contentHeight: col.height
        clip: true

        Column {
            id: col
            width: parent.width
            spacing: 12

            Row {
                spacing: 8
                Label { text: "deploy"; color: "#cdd6f4"; font.pixelSize: 18; font.bold: true }
            }
            Label {
                width: col.width
                text: root.note; visible: root.note.length > 0
                color: root.busy ? "#f9e2af" : "#8a93a6"
                wrapMode: Text.WrapAnywhere
            }

            // ── 1 · find devices ─────────────────────────────────────────
            Column {
                width: col.width; spacing: 6
                Label { text: "1 · find devices"; color: "#7aa2f7"; font.bold: true }
                Row {
                    width: parent.width; spacing: 8
                    TextField {
                        id: subnetField
                        width: parent.width - discBtn.width - 8
                        placeholderText: "subnet — blank = auto (e.g. 192.168.1)"
                        text: root.subnet
                        onTextChanged: root.subnet = text
                    }
                    Button { id: discBtn; text: root.busy ? "…" : "discover"; enabled: !root.busy; onClicked: root.discover() }
                }
                Repeater {
                    model: root.devices
                    delegate: Rectangle {
                        width: col.width; height: 44; radius: 8
                        color: modelData.ip === root.selHost ? "#2a3350" : "#171c2b"
                        border.color: modelData.ip === root.selHost ? "#7aa2f7" : "#2a3350"
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            x: 10
                            Label { text: (modelData.hostname && modelData.hostname.length) ? modelData.hostname : modelData.ip; color: "#cdd6f4"; font.bold: true }
                            Label { text: modelData.ip; color: "#8a93a6"; font.pixelSize: 12 }
                        }
                        TapHandler { onTapped: root.selectDevice(modelData.ip, modelData.hostname) }
                    }
                }
            }

            // ── 2 · pick a folder ────────────────────────────────────────
            Column {
                width: col.width; spacing: 6
                visible: root.selHost.length > 0
                Label { text: "2 · pick a folder on " + root.selName; color: "#7aa2f7"; font.bold: true }
                Row {
                    width: parent.width; spacing: 8
                    Label { text: "user"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                    TextField { width: 140; text: root.user; onTextChanged: root.user = text }
                    Button { text: "browse"; onClicked: root.browse("") }
                }
                Label {
                    width: parent.width
                    text: root.curPath.length ? root.curPath : "(tap browse to start at home)"
                    color: "#cdd6f4"; wrapMode: Text.WrapAnywhere
                }
                Column {
                    width: parent.width; spacing: 4
                    visible: root.curPath.length > 0
                    Rectangle {
                        width: col.width; height: 36; radius: 6; color: "#171c2b"; border.color: "#2a3350"
                        Label { anchors.verticalCenter: parent.verticalCenter; x: 10; text: "‹ .."; color: "#8a93a6" }
                        TapHandler { onTapped: root.browse(root.parentOf(root.curPath)) }
                    }
                    Repeater {
                        model: root.dirs
                        delegate: Rectangle {
                            width: col.width; height: 36; radius: 6; color: "#171c2b"; border.color: "#2a3350"
                            Label { anchors.verticalCenter: parent.verticalCenter; x: 10; text: "▸ " + modelData; color: "#cdd6f4" }
                            TapHandler { onTapped: root.browse(root.curPath + "/" + modelData) }
                        }
                    }
                }
            }

            // ── 3 · push + launch ────────────────────────────────────────
            Column {
                width: col.width; spacing: 6
                visible: root.curPath.length > 0
                Label { text: "3 · push + launch"; color: "#7aa2f7"; font.bold: true }
                Button {
                    width: parent.width
                    text: "push monorepo → " + root.curPath
                    enabled: !root.busy
                    onClicked: root.pushHere()
                }
                Row {
                    width: parent.width; spacing: 8
                    TextField {
                        width: parent.width - depBtn.width - 8
                        placeholderText: "app to build (e.g. voiceterm)"
                        text: root.app
                        onTextChanged: root.app = text
                    }
                    Button { id: depBtn; text: "build + launcher"; enabled: !root.busy; onClicked: root.deployHere() }
                }
                Label {
                    width: parent.width
                    text: "builds the app on the device and writes a .desktop launcher (shows on Plasma Mobile). The device needs rust + the app's toolkit (apk add)."
                    color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                }
            }
        }
    }
}
