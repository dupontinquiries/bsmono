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
        root.jobKind = ""
    }
    Component.onCompleted: refreshAdvertise()

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
        }
    }
}
