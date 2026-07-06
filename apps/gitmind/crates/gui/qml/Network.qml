// Network: advertise this repo over HTTP so peers can find it, look up a peer's
// card by address, and install (clone) it. The install runs on the shared worker
// job; the poller picks up its result (so the UI stays live).
import QtQuick
import QtQuick.Controls

Item {
    id: root
    property bool serving: false
    property string serveAddr: ""
    property string port: "9876"
    property string peerAddr: ""
    property var card: null
    property string dest: ""
    property string note: ""
    property bool busy: false
    property string jobKind: ""
    // this device's own key (advertiser side of two-sided matching)
    property bool myKeyPresent: false
    property string myKeyFp: ""
    property string myKeyType: ""
    property string myKeyArt: ""
    // bluetooth send
    property var btDevices: []
    property string btAddr: ""
    property string btName: ""
    // daemons card
    property var daemonModel: []
    property string daemonNote: ""
    property string dUser: "user"
    property string dBinDir: "/home/user/.local/bin"
    property string dHome: "/home/user"
    function loadDaemons() {
        try {
            var r = JSON.parse(backend.daemons_list())
            if (r.ok) root.daemonModel = r.daemons
        } catch (e) {}
    }
    property string btFile: ""

    function loadFingerprint() {
        var r = JSON.parse(backend.local_fingerprint())
        if (r.ok && r.host && r.host.present) {
            root.myKeyPresent = true; root.myKeyFp = r.host.fingerprint
            root.myKeyType = r.host.type; root.myKeyArt = r.host.art
        } else { root.myKeyPresent = false }
    }
    function loadBtDevices() {
        var r = JSON.parse(backend.bluetooth_devices())
        if (r.ok) { root.btDevices = r.devices; root.note = r.devices.length + " paired Bluetooth device(s)" }
        else { root.btDevices = []; root.note = "error: " + r.error }
    }
    function makeBundle() {
        var r = JSON.parse(backend.make_bundle(""))
        if (r.ok) { root.btFile = r.path; root.note = "bundled the repo → " + r.path }
        else { root.note = "error: " + r.error }
    }
    function btSend() {
        if (root.btAddr.length === 0 || root.btFile.length === 0) { root.note = "pick a device and a file"; return }
        var r = JSON.parse(backend.start_bluetooth_send(root.btAddr, root.btFile))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "btsend"
        root.note = "sending " + root.btFile + " to " + (root.btName.length ? root.btName : root.btAddr) + "…"
        poller.start()
    }

    function refreshAdvertise() {
        var s = JSON.parse(backend.advertise_status())
        root.serving = s.serving; root.serveAddr = s.addr
    }
    function toggleAdvertise() {
        if (root.serving) {
            backend.advertise_stop(); refreshAdvertise()
            root.note = "stopped advertising"
        } else {
            var r = JSON.parse(backend.advertise_start(root.port))
            if (r.ok) { root.serving = true; root.serveAddr = r.addr; root.note = "advertising " + r.name + " on " + r.addr }
            else { root.note = "error: " + r.error }
        }
    }
    function lookup() {
        var r = JSON.parse(backend.lookup_peer(root.peerAddr))
        if (r.ok) { root.card = r; root.note = "found: " + r.name }
        else { root.card = null; root.note = "error: " + r.error }
    }
    function install() {
        if (!root.card || !root.card.providers || root.card.providers.length === 0) { root.note = "no provider in this card"; return }
        var r = JSON.parse(backend.start_install(root.card.name, root.card.providers[0], root.dest))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "install"
        root.note = "cloning " + root.card.name + " — this can take a while…"
        poller.start()
    }
    function applyJobResult(res) {
        if (root.jobKind === "install") root.note = res.ok ? ("✓ " + res.msg) : ("error: " + res.error)
        else if (root.jobKind === "btsend") root.note = res.ok ? ("✓ " + res.msg) : ("error: " + res.error)
        root.jobKind = ""
    }
    Component.onCompleted: { refreshAdvertise(); loadFingerprint(); loadDaemons() }

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
                Label { text: "network"; color: "#cdd6f4"; font.pixelSize: 18; font.bold: true }
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

            // ── advertise this repo ──────────────────────────────────────
            Column {
                width: col.width; spacing: 6
                Label { text: "advertise this repo"; color: "#7aa2f7"; font.bold: true }
                Row {
                    width: parent.width; spacing: 8
                    Label { text: "port"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                    TextField { width: 90; text: root.port; enabled: !root.serving; onTextChanged: root.port = text }
                    Button { text: root.serving ? "stop" : "advertise"; onClicked: root.toggleAdvertise() }
                }
                Label {
                    width: parent.width
                    text: root.serving ? ("serving on " + root.serveAddr) : "not advertising"
                    color: root.serving ? "#56d364" : "#8a93a6"; font.pixelSize: 12
                }
            }

            // ── this device's key (for two-sided matching) ───────────────
            // The peer connecting to you taps "verify key" on its Deploy tab and
            // compares what it sees against this. Same fingerprint = same device.
            Column {
                width: col.width; spacing: 6
                visible: root.myKeyPresent
                Label { text: "this device's key"; color: "#7aa2f7"; font.bold: true }
                Label {
                    width: parent.width
                    text: root.myKeyFp + (root.myKeyType.length ? ("  (" + root.myKeyType + ")") : "")
                    color: "#cdd6f4"; font.pixelSize: 13; font.family: "monospace"; wrapMode: Text.WrapAnywhere
                }
                Label {
                    width: parent.width; visible: root.myKeyArt.length > 0
                    text: root.myKeyArt
                    color: "#8a93a6"; font.family: "monospace"; font.pixelSize: 11
                }
            }

            // ── look up a peer ───────────────────────────────────────────
            Column {
                width: col.width; spacing: 6
                Label { text: "look up a peer"; color: "#7aa2f7"; font.bold: true }
                Row {
                    width: parent.width; spacing: 8
                    TextField {
                        width: parent.width - lookBtn.width - 8
                        placeholderText: "peer address — host:port (e.g. 192.168.1.7:9876)"
                        text: root.peerAddr; onTextChanged: root.peerAddr = text
                    }
                    Button { id: lookBtn; text: "look up"; onClicked: root.lookup() }
                }
            }

            // ── the looked-up card + install ─────────────────────────────
            Column {
                width: col.width; spacing: 6
                visible: root.card !== null
                Rectangle {
                    width: col.width; radius: 8
                    color: "#171c2b"; border.color: "#2a3350"
                    height: cardCol.height + 16
                    Column {
                        id: cardCol
                        x: 12; y: 8; width: parent.width - 24; spacing: 2
                        Label { text: root.card ? root.card.name : ""; color: "#cdd6f4"; font.bold: true }
                        Label {
                            text: (root.card && root.card.description && root.card.description.length) ? root.card.description : "(no description)"
                            color: "#8a93a6"; font.pixelSize: 12
                        }
                        Label {
                            text: root.card ? (root.card.refs + " ref(s) · " + (root.card.providers ? root.card.providers.length : 0) + " provider(s)") : ""
                            color: "#5b6477"; font.pixelSize: 11
                        }
                        Label {
                            width: parent.width
                            text: (root.card && root.card.providers && root.card.providers.length) ? root.card.providers[0] : ""
                            color: "#5b6477"; font.pixelSize: 11; elide: Text.ElideMiddle
                        }
                    }
                }
                Row {
                    width: parent.width; spacing: 8
                    TextField {
                        width: parent.width - instBtn.width - 8
                        placeholderText: "clone into folder (e.g. ~/peers)"
                        text: root.dest; onTextChanged: root.dest = text
                    }
                    Button {
                        id: instBtn; text: "install"
                        enabled: !root.busy && root.dest.length > 0
                        onClicked: root.install()
                    }
                }
                Label {
                    width: parent.width
                    text: "clones the repo from its ssh:// provider into that folder (git clone over ssh — you need ssh access to the peer)."
                    color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                }
            }

            // ── send a file over Bluetooth (OBEX) ────────────────────────
            // A no-LAN way to hand the repo bundle to a paired device.
            Column {
                width: col.width; spacing: 6
                Row {
                    width: parent.width; spacing: 8
                    Label { text: "send over Bluetooth"; color: "#7aa2f7"; font.bold: true; anchors.verticalCenter: parent.verticalCenter }
                    Button { text: "refresh devices"; enabled: !root.busy; onClicked: root.loadBtDevices() }
                }
                Repeater {
                    model: root.btDevices
                    delegate: Rectangle {
                        width: col.width; height: 40; radius: 6
                        color: modelData.addr === root.btAddr ? "#2a3350" : "#171c2b"
                        border.color: modelData.addr === root.btAddr ? "#7aa2f7" : "#2a3350"
                        Column {
                            anchors.verticalCenter: parent.verticalCenter; x: 10
                            Label { text: (modelData.name && modelData.name.length) ? modelData.name : modelData.addr; color: "#cdd6f4"; font.pixelSize: 13 }
                            Label { text: modelData.addr; color: "#5b6477"; font.pixelSize: 11 }
                        }
                        TapHandler { onTapped: { root.btAddr = modelData.addr; root.btName = modelData.name } }
                    }
                }
                Row {
                    width: parent.width; spacing: 8
                    TextField {
                        width: parent.width - bundleBtn.width - 8
                        placeholderText: "file to send (tap “bundle repo” for the monorepo)"
                        text: root.btFile; onTextChanged: root.btFile = text
                    }
                    Button { id: bundleBtn; text: "bundle repo"; enabled: !root.busy; onClicked: root.makeBundle() }
                }
                Button {
                    width: parent.width
                    text: root.btAddr.length ? ("send to " + (root.btName.length ? root.btName : root.btAddr)) : "pick a device above"
                    enabled: !root.busy && root.btAddr.length > 0 && root.btFile.length > 0
                    onClicked: root.btSend()
                }
                Label {
                    width: parent.width
                    text: "OBEX push — the device must be paired and accepting files. Needs bluez-tools (bt-obex) or obexftp on this end."
                    color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                }
            }

            // ── daemons: the fleet's background services ─────────────────
            Column {
                width: col.width; spacing: 6
                Label { text: "daemons"; color: "#7aa2f7"; font.bold: true }
                Repeater {
                    model: root.daemonModel
                    delegate: Column {
                        id: dRow
                        property string dname: modelData.name
                        width: parent.width; spacing: 3
                        Row {
                            width: parent.width; spacing: 8
                            Label { text: "●"; color: "#8a93a6"; font.pixelSize: 11; anchors.verticalCenter: parent.verticalCenter }
                            Column {
                                Label { text: modelData.name + "  ·  " + modelData.socket; color: "#e6e6e6"; font.pixelSize: 12 }
                                Label { text: modelData.description; color: "#8a93a6"; font.pixelSize: 10; width: col.width - 40; wrapMode: Text.WordWrap }
                            }
                        }
                        Row {
                            spacing: 6
                            Repeater {
                                model: ["start", "stop", "restart"]
                                delegate: Button {
                                    text: modelData; font.pixelSize: 10
                                    enabled: !root.busy && root.btAddr.length > 0
                                    onClicked: {
                                        var r = JSON.parse(backend.daemon_control(root.dUser, root.btAddr, dRow.dname, modelData))
                                        root.daemonNote = r.ok ? (dRow.dname + " " + modelData + ": ok") : r.error
                                    }
                                }
                            }
                            Button {
                                text: "logs"; font.pixelSize: 10
                                enabled: !root.busy && root.btAddr.length > 0
                                onClicked: {
                                    var r = JSON.parse(backend.daemon_logs(root.dUser, root.btAddr, dRow.dname, 200))
                                    root.daemonNote = r.ok ? r.output : r.error
                                }
                            }
                        }
                    }
                }
                Row {
                    width: parent.width; spacing: 8
                    Button {
                        text: "install + enable on " + (root.btName.length ? root.btName : "device")
                        enabled: !root.busy && root.btAddr.length > 0
                        onClicked: {
                            var r = JSON.parse(backend.install_daemons(root.dUser, root.btAddr, root.dBinDir, root.dHome))
                            root.daemonNote = r.ok ? r.output : r.error
                        }
                    }
                    Button {
                        text: "status"
                        enabled: !root.busy && root.btAddr.length > 0
                        onClicked: {
                            var r = JSON.parse(backend.daemons_status(root.dUser, root.btAddr))
                            root.daemonNote = r.ok ? r.output : r.error
                        }
                    }
                }
                Label {
                    width: parent.width; visible: root.daemonNote.length > 0
                    text: root.daemonNote; color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                }
            }
        }
    }
}
