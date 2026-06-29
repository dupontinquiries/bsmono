// Fleet — app pages on top of gitmind's repo management.
//
// gitmind manages the repos; this layer reads the buildable ones (Cargo.toml /
// CMakeLists), lists each as an app page named by its *crate*, builds it on demand
// (with a live build status), and — only on a successful build — sends the binary
// to a device. Nothing but the compiled binary is transferred.
import QtQuick
import QtQuick.Controls

Item {
    id: root

    // ── app pages (local, from gitmind's repos) ──────────────────────────
    property var apps: []             // [{name, path, description, is_cargo, is_cmake}]
    property string selApp: ""        // selected crate name ("" = showing the list)
    property string selPath: ""
    property string selDesc: ""
    property string selOid: ""
    property string selShot: ""
    property int shotVer: 0
    property bool selIsCargo: false
    property bool selIsCmake: false

    // ── build ────────────────────────────────────────────────────────────
    property string target: ""        // triple to build for
    property string localTarget: ""   // this device's native triple
    property var allFeatures: []       // [name]
    property var features: []          // [{name, on}]
    property var defaultsByTarget: ({})
    property bool optionsOpen: false
    property string buildState: "idle" // idle | building | built | failed
    property string builtBinary: ""
    property string builtSize: ""
    property string buildError: ""

    // ── send (needs a device) ────────────────────────────────────────────
    property var devices: []
    property string selHost: ""
    property string selName: ""
    property string user: "gitmind"   // the managed fleet user
    property string adminUser: ""
    property bool hostTrusted: false
    property bool connectOpen: false   // reveal device discovery
    property bool setupOpen: false
    property string sshpass: ""
    property string sendState: "idle"  // idle | sending | sent | failed

    // ── job plumbing ─────────────────────────────────────────────────────
    property bool busy: false
    property string jobKind: ""
    property string note: ""

    Component.onCompleted: {
        var f = JSON.parse(backend.fleet_user())
        if (f.user && f.user.length) root.user = f.user
        if (f.admin && f.admin.length) root.adminUser = f.admin
        var t = JSON.parse(backend.local_target())
        if (t.target) { root.localTarget = t.target; root.target = t.target }
        root.loadApps()
    }

    // ── app list / page ──────────────────────────────────────────────────
    function loadApps() {
        var r = JSON.parse(backend.local_apps())
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.apps = r.apps || []
        if (root.apps.length === 0) root.note = "no buildable repos found here yet"
    }
    function openApp(app) {
        root.selApp = app.name; root.selPath = app.path; root.selDesc = app.description
        root.selIsCargo = app.is_cargo; root.selIsCmake = app.is_cmake
        root.selOid = app.oid || ""; root.selShot = app.shot || ""
        root.buildState = "idle"; root.builtBinary = ""; root.buildError = ""
        root.sendState = "idle"; root.optionsOpen = false; root.note = ""
        root.target = root.localTarget
        var o = JSON.parse(backend.local_build_options(app.path))
        if (o.ok) {
            root.allFeatures = o.features || []
            root.defaultsByTarget = o.defaults_by_target || ({})
            root.applyDefaults(root.target)
        } else { root.allFeatures = []; root.features = [] }
    }
    // Grab a tile screenshot of the just-built app (auto after build, or on demand).
    function capturePreview() {
        if (root.builtBinary.length === 0) return
        var r = JSON.parse(backend.capture_preview(root.selApp, root.selOid, root.builtBinary))
        if (!r.ok) { root.note = "preview: " + r.error; return }
        root.busy = true; root.jobKind = "shot"; poller.start()
    }
    function backToList() { root.selApp = ""; root.note = "" }
    function applyDefaults(t) {
        root.target = t
        var defs = root.defaultsByTarget[t] || []
        var arr = []
        for (var i = 0; i < root.allFeatures.length; i++)
            arr.push({ name: root.allFeatures[i], on: defs.indexOf(root.allFeatures[i]) >= 0 })
        root.features = arr
    }
    function chosenFeatures() {
        var out = []
        for (var i = 0; i < root.features.length; i++)
            if (root.features[i].on) out.push(root.features[i].name)
        return out.join(",")
    }
    function targetShort(t) { return t.indexOf("musl") >= 0 ? "musl" : (t.indexOf("gnu") >= 0 ? "glibc" : t) }

    function doBuild() {
        var noDefault = root.features.length > 0
        var r = JSON.parse(backend.build_app(root.selApp, root.target, root.chosenFeatures(), noDefault))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.buildState = "building"; root.busy = true; root.jobKind = "build"; root.note = ""
        poller.start()
    }

    // ── connection (for the send step) ───────────────────────────────────
    function discover() {
        var r = JSON.parse(backend.start_discover(""))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "discover"; poller.start()
    }
    function selectDevice(ip, host) {
        root.selHost = ip; root.selName = (host && host.length) ? host : ip; root.sendState = "idle"
        refreshTrust()
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
    function saveFleetUser() {
        var r = JSON.parse(backend.set_fleet_user(root.user))
        root.note = r.ok ? ("fleet user set to " + r.user) : ("error: " + r.error)
    }
    function setupKey() {
        var r = JSON.parse(backend.start_setup_key(root.user, root.selHost, root.sshpass))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "setupkey"; poller.start()
    }
    function provisionFleetUser() {
        var r = JSON.parse(backend.start_provision_fleet_user(root.adminUser, root.selHost, root.sshpass))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "provision"; poller.start()
    }
    function doSend() {
        if (root.builtBinary.length === 0) { root.note = "build the app first"; return }
        var r = JSON.parse(backend.send_binary(root.builtBinary, root.user, root.selHost, ""))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.sendState = "sending"; root.busy = true; root.jobKind = "send"; poller.start()
    }

    // ── job poller ───────────────────────────────────────────────────────
    Timer {
        id: poller; interval: 250; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); root.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            root.applyResult(res)
        }
    }
    function applyResult(res) {
        var k = root.jobKind; root.jobKind = ""
        if (k === "discover") {
            root.devices = res.ok ? (res.devices || []) : []
        } else if (k === "trust") {
            root.note = res.ok ? "✓ host key trusted" : ("error: " + res.error); refreshTrust()
        } else if (k === "setupkey") {
            root.sshpass = ""; root.setupOpen = false
            root.note = res.ok ? "✓ key installed" : ("error: " + res.error); refreshTrust()
        } else if (k === "provision") {
            root.sshpass = ""
            root.note = res.ok ? ("✓ " + res.msg) : ("✗ " + res.error); refreshTrust()
        } else if (k === "build") {
            if (res.ok) {
                root.buildState = "built"; root.builtBinary = res.binary; root.builtSize = res.size; root.note = ""
                root.capturePreview()
            } else {
                root.buildState = "failed"; root.buildError = res.error; root.note = ""
            }
        } else if (k === "shot") {
            if (res.ok && res.shot && res.shot.length) {
                root.selShot = res.shot; root.shotVer++
            }
        } else if (k === "send") {
            if (res.ok) { root.sendState = "sent"; root.note = "✓ " + res.msg }
            else { root.sendState = "failed"; root.note = "✗ " + res.error }
        }
    }

    // ══ UI ════════════════════════════════════════════════════════════════
    Flickable {
        anchors.fill: parent; anchors.margins: 12
        contentHeight: col.height + 24; clip: true
        Column {
            id: col
            width: parent.width; spacing: 12

            // status line (shared)
            Label {
                width: parent.width; visible: root.note.length > 0
                text: root.note; color: "#a6adc8"; font.pixelSize: 12; wrapMode: Text.WrapAnywhere
            }

            // ── APP LIST ─────────────────────────────────────────────────
            Column {
                width: parent.width; spacing: 10
                visible: root.selApp.length === 0
                Row {
                    width: parent.width
                    Label { text: "Apps"; color: "#cdd6f4"; font.bold: true; font.pixelSize: 20; width: parent.width - rfBtn.width }
                    Button { id: rfBtn; text: "↻"; onClicked: root.loadApps() }
                }
                Label {
                    width: parent.width
                    text: "Buildable repos gitmind manages here. Tap one to build it and send the binary to a device."
                    color: "#5b6477"; font.pixelSize: 12; wrapMode: Text.WordWrap
                }
                Repeater {
                    model: root.apps
                    delegate: Rectangle {
                        width: col.width; radius: 10; color: "#1b2335"; border.color: "#3a4566"
                        height: cardC.height + 20
                        Column {
                            id: cardC
                            x: 12; y: 10; width: parent.width - 24; spacing: 4
                            Row {
                                width: parent.width; spacing: 10
                                Item {
                                    width: 40; height: 40
                                    Image {
                                        anchors.fill: parent; visible: modelData.shot.length > 0
                                        source: modelData.shot.length ? "file://" + modelData.shot : ""
                                        fillMode: Image.PreserveAspectCrop; cache: false; asynchronous: true
                                    }
                                    MonoTile {
                                        anchors.fill: parent; visible: modelData.shot.length === 0
                                        appName: modelData.name; oid: modelData.oid
                                    }
                                }
                                Column {
                                    width: parent.width - 48; spacing: 1
                                    Label { text: modelData.name; color: "#cdd6f4"; font.bold: true; font.pixelSize: 15 }
                                    Label {
                                        width: parent.width
                                        text: modelData.description.length ? modelData.description
                                              : (modelData.is_cargo ? "cargo crate" : "cmake project")
                                        color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                                        maximumLineCount: 2; elide: Text.ElideRight
                                    }
                                }
                            }
                        }
                        TapHandler { onTapped: root.openApp(modelData) }
                    }
                }
            }

            // ── APP PAGE ─────────────────────────────────────────────────
            Column {
                width: parent.width; spacing: 12
                visible: root.selApp.length > 0

                Row {
                    spacing: 10
                    Label { text: "‹ apps"; color: "#7aa2f7"; TapHandler { onTapped: root.backToList() } }
                }
                Row {
                    width: parent.width; spacing: 12
                    Item {
                        width: 58; height: 58
                        Image {
                            anchors.fill: parent; visible: root.selShot.length > 0
                            source: root.selShot.length ? "file://" + root.selShot + "?v=" + root.shotVer : ""
                            fillMode: Image.PreserveAspectCrop; cache: false; asynchronous: true
                        }
                        MonoTile {
                            anchors.fill: parent; visible: root.selShot.length === 0
                            appName: root.selApp; oid: root.selOid
                        }
                        BusyIndicator { anchors.centerIn: parent; running: root.jobKind === "shot"; visible: running; width: 28; height: 28 }
                    }
                    Column {
                        width: parent.width - 70; spacing: 2
                        Label { text: root.selApp; color: "#cdd6f4"; font.bold: true; font.pixelSize: 20 }
                        Label {
                            width: parent.width
                            text: root.selDesc.length ? root.selDesc : (root.selIsCargo ? "cargo crate" : "cmake project")
                            color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                        }
                    }
                }

                // ── 1 · build ────────────────────────────────────────────
                Rectangle {
                    width: parent.width; radius: 10; color: "#171c2b"; border.color: "#2a3350"
                    height: buildC.height + 20
                    Column {
                        id: buildC
                        x: 12; y: 10; width: parent.width - 24; spacing: 8
                        Label { text: "Build"; color: "#7aa2f7"; font.bold: true }
                        // target
                        Row {
                            spacing: 8
                            Label { text: "for"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                            Repeater {
                                model: ["aarch64-unknown-linux-musl", "aarch64-unknown-linux-gnu"]
                                delegate: Rectangle {
                                    height: 30; radius: 15; width: tl.width + 24
                                    color: modelData === root.target ? "#2a3350" : "#1b2335"
                                    border.color: modelData === root.target ? "#7aa2f7" : "#3a4566"
                                    Label {
                                        id: tl; anchors.centerIn: parent
                                        text: root.targetShort(modelData) + (modelData === root.localTarget ? " (this device)" : "")
                                        color: "#cdd6f4"; font.pixelSize: 12
                                    }
                                    TapHandler { onTapped: root.applyDefaults(modelData) }
                                }
                            }
                        }
                        // optional flags
                        Label {
                            text: root.optionsOpen ? "▾ options" : "▸ options"
                            color: "#7aa2f7"; font.pixelSize: 12
                            visible: root.selIsCargo && (root.allFeatures.length > 0)
                            TapHandler { onTapped: root.optionsOpen = !root.optionsOpen }
                        }
                        Repeater {
                            model: root.optionsOpen ? root.features : []
                            delegate: CheckBox {
                                checked: modelData.on
                                onToggled: root.features[index].on = checked
                                contentItem: Label { text: modelData.name; color: "#cdd6f4"; leftPadding: parent.indicator.width + 6; verticalAlignment: Text.AlignVCenter }
                            }
                        }
                        // build button + status
                        Row {
                            spacing: 8
                            Button {
                                text: root.buildState === "building" ? "building…" : (root.buildState === "built" ? "rebuild" : "Build")
                                enabled: !root.busy && root.selIsCargo
                                onClicked: root.doBuild()
                            }
                            Button {
                                text: root.jobKind === "shot" ? "grabbing…" : "re-grab preview"
                                visible: root.buildState === "built"
                                enabled: !root.busy
                                onClicked: root.capturePreview()
                            }
                        }
                        Label {
                            width: parent.width; visible: !root.selIsCargo
                            text: "CMake cross-build needs a per-target toolchain file (on-device). The flags above are read straight from CMakeLists."
                            color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap
                        }
                        // status panel
                        Rectangle {
                            width: parent.width; radius: 8
                            visible: root.buildState !== "idle"
                            color: root.buildState === "built" ? "#16271b" : (root.buildState === "failed" ? "#2a1620" : "#1b2335")
                            border.color: root.buildState === "built" ? "#56d364" : (root.buildState === "failed" ? "#f7768e" : "#3a4566")
                            height: stC.height + 16
                            Column {
                                id: stC
                                x: 10; y: 8; width: parent.width - 20; spacing: 4
                                Label {
                                    text: root.buildState === "building" ? "⏳ building " + root.selApp + " for " + root.targetShort(root.target) + "…"
                                        : root.buildState === "built" ? "✓ built · " + root.builtSize
                                        : "✗ build failed"
                                    color: root.buildState === "built" ? "#56d364" : (root.buildState === "failed" ? "#f7768e" : "#f9e2af")
                                    font.bold: true; font.pixelSize: 13
                                }
                                Label {
                                    width: parent.width; visible: root.buildState === "built"
                                    text: root.builtBinary; color: "#8a93a6"; font.pixelSize: 11
                                    font.family: "monospace"; wrapMode: Text.WrapAnywhere
                                }
                                Label {
                                    width: parent.width; visible: root.buildState === "failed"
                                    text: root.buildError; color: "#cdd6f4"; font.pixelSize: 11
                                    font.family: "monospace"; wrapMode: Text.WrapAnywhere
                                    maximumLineCount: 12; elide: Text.ElideRight
                                }
                            }
                        }
                    }
                }

                // ── 2 · send the binary (only after a successful build) ───
                Rectangle {
                    width: parent.width; radius: 10
                    color: "#171c2b"; border.color: root.buildState === "built" ? "#7aa2f7" : "#2a3350"
                    height: sendC.height + 20
                    opacity: root.buildState === "built" ? 1.0 : 0.5
                    Column {
                        id: sendC
                        x: 12; y: 10; width: parent.width - 24; spacing: 8
                        Label { text: "Send the binary"; color: "#7aa2f7"; font.bold: true }
                        Label {
                            width: parent.width; visible: root.buildState !== "built"
                            text: "Build succeeds first, then the binary can be sent."
                            color: "#5b6477"; font.pixelSize: 12; wrapMode: Text.WordWrap
                        }
                        // device choice
                        Column {
                            width: parent.width; spacing: 8
                            visible: root.buildState === "built"
                            Row {
                                width: parent.width; spacing: 8
                                Label {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: root.selHost.length ? ("to: " + root.selName + (root.hostTrusted ? "  🔒" : "")) : "no device chosen"
                                    color: root.selHost.length ? "#cdd6f4" : "#8a93a6"
                                    width: parent.width - dscBtn.width - sndBtn.width - 16
                                    elide: Text.ElideRight
                                }
                                Button { id: dscBtn; text: root.connectOpen ? "hide" : "choose…"; onClicked: root.connectOpen = !root.connectOpen }
                                Button {
                                    id: sndBtn
                                    text: root.sendState === "sending" ? "sending…" : (root.sendState === "sent" ? "sent ✓" : "Send")
                                    enabled: !root.busy && root.selHost.length > 0
                                    onClicked: root.doSend()
                                }
                            }
                            Label {
                                width: parent.width
                                text: "→ " + root.user + "@" + (root.selName.length ? root.selName : "device") + ":~/.local/bin/" + root.selApp
                                color: "#56d364"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere
                                visible: root.selHost.length > 0
                            }
                            // ── device picker (discover + trust + fleet user) ──
                            Column {
                                width: parent.width; spacing: 8
                                visible: root.connectOpen
                                Row {
                                    spacing: 8
                                    Button { text: root.busy ? "…" : "scan LAN"; enabled: !root.busy; onClicked: root.discover() }
                                    Label { text: "as user"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                                    TextField { width: 110; text: root.user; onTextChanged: root.user = text }
                                    Button { text: "save"; enabled: !root.busy; onClicked: root.saveFleetUser() }
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
                                // trust + setup (shown once a device is picked)
                                Column {
                                    width: parent.width; spacing: 6
                                    visible: root.selHost.length > 0
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
                                        width: parent.width; spacing: 6
                                        visible: root.setupOpen
                                        Label {
                                            width: parent.width
                                            text: "Create the '" + root.user + "' fleet user on the device (needs sudo once), or install your key for an account that already exists. Then sends are passwordless."
                                            color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                                        }
                                        TextField {
                                            width: parent.width
                                            placeholderText: "device password (used once, not stored)"
                                            echoMode: TextInput.Password
                                            text: root.sshpass; onTextChanged: root.sshpass = text
                                        }
                                        Row {
                                            spacing: 8
                                            Button { text: "install my key"; enabled: !root.busy && root.sshpass.length > 0; onClicked: root.setupKey() }
                                            Label { text: "sudo as"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                                            TextField { width: 100; text: root.adminUser; onTextChanged: root.adminUser = text }
                                            Button { text: "create fleet user"; enabled: !root.busy && root.sshpass.length > 0 && root.adminUser.length > 0; onClicked: root.provisionFleetUser() }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
