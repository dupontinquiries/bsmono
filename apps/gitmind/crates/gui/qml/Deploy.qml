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
    property string user: "user"  // defaulted to the real login name on load
    property bool hostTrusted: false   // is selHost's key in known_hosts?
    property string curPath: ""   // resolved absolute folder on the device
    property var entries: []      // folder contents: [{name, app, cargo, git}]
    property string app: "voiceterm"
    // selected app's metadata (the catalog info)
    property string appPath: ""
    property int appCrates: 0
    property string appSize: ""
    property string appReadme: ""
    property bool showAppInfo: false
    // confirmation dialog
    property string confirmAction: "" // "push" | "deploy"
    property string confirmPath: ""
    property string note: ""
    property bool busy: false
    property string jobKind: ""   // in-flight job: discover|browse|push|deploy|trust|retrust|setupkey
    property string lastReason: ""// why the last ssh op failed: auth|hostkey|unreachable|other
    property string sshpass: ""   // one-time device password for key setup (not stored)
    property bool showSetup: false// reveal the password + "set up SSH key" row
    // fingerprint preview (two-sided matching): the peer key we fetched to compare
    property bool showPreview: false
    property string previewFp: ""
    property string previewType: ""
    property string previewArt: ""
    property string previewKeys: ""// exact bytes to trust once the user confirms a match

    // Default the SSH user to whoami so we never ssh as the literal "user" (which
    // hit a password prompt and froze the GUI).
    Component.onCompleted: {
        var r = JSON.parse(backend.current_user())
        if (r.user && r.user.length) root.user = r.user
    }

    // Slow ops run on a worker thread in the backend; we kick them off and let the
    // poller pick up the result, so the UI stays live and the spinner animates.
    function discover() {
        var r = JSON.parse(backend.start_discover(root.subnet))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "discover"; root.note = "scanning the LAN for SSH…"
        poller.start()
    }
    function selectDevice(ip, host) {
        selHost = ip; selName = (host && host.length) ? host : ip
        curPath = ""; entries = []; lastReason = ""; showSetup = false; sshpass = ""
        showPreview = false; previewKeys = ""
        showAppInfo = false; appPath = ""
        refreshTrust()
        browse("")
    }
    function refreshTrust() {
        if (selHost.length === 0) { hostTrusted = false; return }
        var r = JSON.parse(backend.host_trusted(selHost))
        hostTrusted = !!r.trusted
    }
    function trustHost() {
        if (selHost.length === 0) return
        var r = JSON.parse(backend.start_trust(selHost))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "trust"; root.note = "fetching + trusting " + root.selName + "'s key…"
        poller.start()
    }
    function retrust() {
        if (selHost.length === 0) return
        var r = JSON.parse(backend.start_retrust(selHost))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "retrust"; root.note = "clearing + re-fetching " + root.selName + "'s key…"
        poller.start()
    }
    // Two-sided matching: fetch the peer's key and show its fingerprint + randomart
    // so you can compare it against what that device shows for itself (Network tab).
    function verifyKey() {
        if (selHost.length === 0) return
        var r = JSON.parse(backend.start_preview(selHost))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "preview"; root.note = "fetching " + root.selName + "'s key to compare…"
        poller.start()
    }
    function confirmTrust() {
        if (selHost.length === 0 || root.previewKeys.length === 0) return
        var r = JSON.parse(backend.trust_previewed_key(selHost, root.previewKeys))
        root.showPreview = false
        if (r.ok) { root.note = "✓ " + r.msg; root.refreshTrust(); root.previewKeys = ""; root.browse("") }
        else { root.note = "error: " + r.error }
    }
    // Browse runs on the worker thread now: a slow/failed ssh used to block the UI
    // here (freezing mid-tap, which is what tore down the cursor handle).
    function browse(path) {
        if (selHost.length === 0) return
        var r = JSON.parse(backend.start_browse(root.user, root.selHost, path))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "browse"; root.note = "listing " + root.selName + "…"
        poller.start()
    }
    function setupKey() {
        if (selHost.length === 0) return
        var r = JSON.parse(backend.start_setup_key(root.user, root.selHost, root.sshpass))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "setupkey"; root.note = "installing your key on " + root.selName + "…"
        poller.start()
    }
    function parentOf(p) { var i = p.lastIndexOf("/"); return i > 0 ? p.substring(0, i) : "/" }
    // Fetch a recognised app's metadata (crate count, size, README) and show it.
    function showAppDetails(name) {
        var path = root.curPath + "/" + name
        var r = JSON.parse(backend.app_details(root.user, root.selHost, path))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.appPath = path; root.appCrates = r.crate_count
        root.appSize = r.size; root.appReadme = r.readme; root.showAppInfo = true
    }
    // Confirmations: open a dialog showing the full (untruncated) path first.
    function askConfirm(action, path) {
        root.confirmAction = action; root.confirmPath = path; confirmDialog.open()
    }
    function doConfirm() {
        confirmDialog.close()
        if (root.confirmAction === "push") root.pushHere()
        else if (root.confirmAction === "deploy") root.deployHere()
    }
    function pushHere() {
        var r = JSON.parse(backend.start_push(root.user, root.selHost, root.curPath))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "push"; root.note = "pushing the monorepo to " + root.curPath + "…"
        poller.start()
    }
    function deployHere() {
        var r = JSON.parse(backend.start_deploy(root.user, root.selHost, root.curPath, root.app))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "deploy"
        root.note = "building " + root.app + " on " + root.selName + " — this can take a while…"
        poller.start()
    }
    function applyJobResult(res) {
        if (root.jobKind === "discover") {
            root.devices = res.ok ? (res.devices || []) : []
            root.note = res.ok ? (root.devices.length + " device(s) on " + res.subnet + ".0/24")
                               : ("error: " + res.error)
        } else if (root.jobKind === "browse") {
            if (res.ok) {
                root.curPath = res.path; root.entries = res.entries || []
                root.lastReason = ""; root.note = ""
            } else {
                root.lastReason = res.reason || "other"
                root.note = root.hint(res.reason, res.error)
                if (res.reason === "auth") root.showSetup = true
            }
        } else if (root.jobKind === "push") {
            root.note = res.ok ? res.msg : ("error: " + res.error)
        } else if (root.jobKind === "deploy") {
            root.note = res.ok ? ("✓ " + res.msg) : ("error: " + res.error)
        } else if (root.jobKind === "trust") {
            root.note = res.ok ? ("✓ " + res.msg) : ("error: " + res.error)
            root.jobKind = ""; root.refreshTrust()
            if (res.ok && root.selHost.length) { root.browse(""); return }
        } else if (root.jobKind === "retrust") {
            root.refreshTrust(); root.jobKind = ""
            root.note = res.ok ? ("✓ " + res.msg + " — retrying…") : ("error: " + res.error)
            if (res.ok && root.selHost.length) { root.lastReason = ""; root.browse(""); return }
        } else if (root.jobKind === "preview") {
            if (res.ok) {
                root.previewFp = res.fingerprint; root.previewType = res.type
                root.previewArt = res.art; root.previewKeys = res.keys
                root.showPreview = true; root.note = ""
            } else {
                root.note = root.hint(res.reason, res.error)
            }
        } else if (root.jobKind === "setupkey") {
            root.jobKind = ""
            if (res.ok) {
                root.sshpass = ""; root.showSetup = false; root.lastReason = ""
                root.note = "✓ " + res.msg + " — retrying…"
                if (root.selHost.length) { root.browse(""); return }
            } else {
                root.note = "error: " + res.error
            }
        }
        root.jobKind = ""
    }
    // Turn a failure reason into an actionable line.
    function hint(reason, raw) {
        if (reason === "auth")
            return "can't log in to " + root.selName + " — no SSH key yet. Enter the device password below to set one up (just once)."
        if (reason === "hostkey")
            return root.selName + "'s host key doesn't match what's trusted (often a leftover from before). Tap “re-trust key”."
        if (reason === "unreachable")
            return "couldn't reach " + root.selName + " — check it's on and sshd is running."
        return "error: " + raw
    }

    // Poll the backend's background job; when it reports done, dispatch the result.
    Timer {
        id: poller
        interval: 250; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); root.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            root.applyJobResult(res)
        }
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
                BusyIndicator {
                    running: root.busy; visible: root.busy
                    implicitWidth: 22; implicitHeight: 22
                    anchors.verticalCenter: parent.verticalCenter
                }
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
                // ── connection: host-key trust + passwordless setup ──────
                // All of this lands in the one shared known_hosts / authorized_keys
                // so nothing has to be done in a terminal (which caused the splits).
                Row {
                    width: parent.width; spacing: 8
                    Label {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.hostTrusted ? "🔒 key trusted" : "host key not trusted yet"
                        color: root.hostTrusted ? "#56d364" : "#f9e2af"
                        font.pixelSize: 13
                    }
                    Button {
                        text: "trust key"; visible: !root.hostTrusted; enabled: !root.busy
                        onClicked: root.trustHost()
                    }
                    Button {
                        // fetch + show the key's fingerprint to compare on both screens
                        text: "verify key"; enabled: !root.busy
                        onClicked: root.verifyKey()
                    }
                    Button {
                        // fixes a changed/stale key (the "messed up from before" case)
                        text: "re-trust"; enabled: !root.busy
                        onClicked: root.retrust()
                    }
                    Button {
                        text: root.showSetup ? "hide setup" : "set up SSH key"
                        enabled: !root.busy
                        onClicked: root.showSetup = !root.showSetup
                    }
                }
                // password + install-key, shown on demand or auto-revealed when a
                // browse fails on auth. Enter the device password once → passwordless.
                Column {
                    width: parent.width; spacing: 6
                    visible: root.showSetup
                    Label {
                        width: parent.width
                        text: "Installs your SSH key on " + root.user + "@" + root.selName + " using the device password (used once, never stored). After this, connecting needs no password."
                        color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                    }
                    Row {
                        width: parent.width; spacing: 8
                        TextField {
                            id: pwField
                            width: parent.width - setupBtn.width - 8
                            placeholderText: "device password for " + root.user + "@" + root.selName
                            echoMode: TextInput.Password
                            text: root.sshpass
                            onTextChanged: root.sshpass = text
                        }
                        Button {
                            id: setupBtn
                            text: root.busy ? "…" : "install my key"
                            enabled: !root.busy && root.sshpass.length > 0
                            onClicked: root.setupKey()
                        }
                    }
                }
                // ── fingerprint comparison panel (two-sided matching) ────
                // Compare this against what the device shows on its Network tab
                // ("this device's key"). Same fingerprint + same randomart = safe.
                Rectangle {
                    width: parent.width; visible: root.showPreview
                    radius: 8; color: "#171c2b"; border.color: "#7aa2f7"
                    height: previewCol.height + 16
                    Column {
                        id: previewCol
                        x: 10; y: 8; width: parent.width - 20; spacing: 6
                        Label {
                            text: "key received from " + root.selName
                            color: "#7aa2f7"; font.bold: true
                        }
                        Label {
                            width: parent.width
                            text: root.previewFp + (root.previewType.length ? ("  (" + root.previewType + ")") : "")
                            color: "#cdd6f4"; font.pixelSize: 13; wrapMode: Text.WrapAnywhere
                            font.family: "monospace"
                        }
                        Label {
                            width: parent.width; visible: root.previewArt.length > 0
                            text: root.previewArt
                            color: "#8a93a6"; font.family: "monospace"; font.pixelSize: 11
                        }
                        Label {
                            width: parent.width
                            text: "Does this match what " + root.selName + " shows for itself? If yes, trust it."
                            color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                        }
                        Row {
                            spacing: 8
                            Button { text: "✓ matches — trust"; enabled: !root.busy; onClicked: root.confirmTrust() }
                            Button { text: "dismiss"; onClicked: { root.showPreview = false; root.previewKeys = "" } }
                        }
                    }
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
                        model: root.entries
                        delegate: Rectangle {
                            width: col.width; height: 40; radius: 6
                            color: modelData.app ? "#1b2335" : "#171c2b"
                            border.color: modelData.app ? "#3a4566" : "#2a3350"
                            Label {
                                anchors.verticalCenter: parent.verticalCenter; x: 10
                                text: (modelData.app ? "📦 " : "▸ ") + modelData.name
                                color: modelData.app ? "#cdd6f4" : "#a6adc8"
                            }
                            // recognised app → little "app info" pill on the right
                            Rectangle {
                                visible: modelData.app
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.right: parent.right; anchors.rightMargin: 8
                                width: infoLbl.width + 16; height: 24; radius: 12
                                color: "#2a3350"
                                Label { id: infoLbl; anchors.centerIn: parent; text: "app info"; color: "#7aa2f7"; font.pixelSize: 11 }
                                TapHandler { onTapped: root.showAppDetails(modelData.name) }
                            }
                            // tapping the row navigates in (works for repos too)
                            TapHandler { onTapped: root.browse(root.curPath + "/" + modelData.name) }
                        }
                    }
                }
                // ── recognised-app metadata panel (name/crates/size/README) ──
                Rectangle {
                    width: parent.width; visible: root.showAppInfo
                    radius: 8; color: "#171c2b"; border.color: "#7aa2f7"
                    height: appInfoCol.height + 16
                    Column {
                        id: appInfoCol
                        x: 10; y: 8; width: parent.width - 20; spacing: 4
                        Label {
                            text: root.appPath.substring(root.appPath.lastIndexOf("/") + 1)
                            color: "#cdd6f4"; font.bold: true; font.pixelSize: 15
                        }
                        Label {
                            text: root.appCrates + " crate(s) · " + root.appSize
                            color: "#8a93a6"; font.pixelSize: 12
                        }
                        Label {
                            width: parent.width; visible: root.appReadme.length > 0
                            text: root.appReadme
                            color: "#a6adc8"; font.pixelSize: 11; wrapMode: Text.WrapAnywhere
                            maximumLineCount: 6; elide: Text.ElideRight
                        }
                        Label { text: "dismiss"; color: "#5b6477"; font.pixelSize: 11; TapHandler { onTapped: root.showAppInfo = false } }
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
                    text: "push monorepo here…"
                    enabled: !root.busy
                    onClicked: root.askConfirm("push", root.curPath)
                }
                Row {
                    width: parent.width; spacing: 8
                    TextField {
                        width: parent.width - depBtn.width - 8
                        placeholderText: "app to build (e.g. voiceterm)"
                        text: root.app
                        onTextChanged: root.app = text
                    }
                    Button { id: depBtn; text: "build + launcher…"; enabled: !root.busy; onClicked: root.askConfirm("deploy", root.curPath) }
                }
                Label {
                    width: parent.width
                    text: "builds the app on the device and writes a .desktop launcher (shows on Plasma Mobile). The device needs rust + the app's toolkit (apk add)."
                    color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                }
            }
        }
    }

    // Confirmation wizard — shows the full target path (wraps, never truncated on
    // mobile) before a push or build runs.
    Dialog {
        id: confirmDialog
        anchors.centerIn: Overlay.overlay
        width: Math.min(root.width - 32, 420)
        modal: true
        standardButtons: Dialog.Ok | Dialog.Cancel
        onAccepted: root.doConfirm()
        background: Rectangle { color: "#0f1320"; border.color: "#2a3350"; radius: 10 }
        contentItem: Column {
            spacing: 10
            Label {
                text: root.confirmAction === "push" ? "Push monorepo?" : "Build + install?"
                color: "#cdd6f4"; font.bold: true; font.pixelSize: 16
            }
            Label {
                width: parent.width
                text: root.confirmAction === "push"
                    ? ("rsync the whole monorepo onto " + root.selName + " at:")
                    : ("build “" + root.app + "” on " + root.selName + " from this folder, then add a launcher:")
                color: "#a6adc8"; wrapMode: Text.WordWrap; font.pixelSize: 12
            }
            Rectangle {
                width: parent.width; radius: 6; color: "#171c2b"; border.color: "#2a3350"
                height: pathLbl.height + 16
                Label {
                    id: pathLbl
                    x: 8; y: 8; width: parent.width - 16
                    text: root.confirmPath.length ? root.confirmPath : "(home)"
                    color: "#7aa2f7"; font.family: "monospace"; font.pixelSize: 12
                    wrapMode: Text.WrapAnywhere
                }
            }
        }
    }
}
