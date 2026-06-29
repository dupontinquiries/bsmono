// A self-contained device picker: discover on the LAN, select, trust the host key,
// and install your key for passwordless access. Manages its own job polling. Parents
// read selHost / selName / hostTrusted / user and react to the `selected` signal.
import QtQuick
import QtQuick.Controls

Item {
    id: dp
    property string user: "user"      // the account to connect as (in/out)
    property string selHost: ""        // outputs
    property string selName: ""
    property bool hostTrusted: false
    signal selected()                  // host chosen or trust changed

    property var devices: []
    property bool busy: false
    property bool open: false
    property bool setupOpen: false
    property string sshpass: ""
    property string jobKind: ""
    property string note: ""

    implicitHeight: card.height

    function discover() {
        var r = JSON.parse(backend.start_discover(""))
        if (!r.ok) { dp.note = "error: " + r.error; return }
        dp.busy = true; dp.jobKind = "discover"; poller.start()
    }
    function selectDevice(ip, host) {
        dp.selHost = ip; dp.selName = (host && host.length) ? host : ip
        dp.hostTrusted = JSON.parse(backend.host_trusted(ip)).trusted
        dp.selected()
    }
    function refreshTrust() {
        if (dp.selHost.length === 0) return
        dp.hostTrusted = JSON.parse(backend.host_trusted(dp.selHost)).trusted
    }
    function trustHost() {
        var r = JSON.parse(backend.start_trust(dp.selHost))
        if (!r.ok) { dp.note = "error: " + r.error; return }
        dp.busy = true; dp.jobKind = "trust"; poller.start()
    }
    function setupKey() {
        var r = JSON.parse(backend.start_setup_key(dp.user, dp.selHost, dp.sshpass))
        if (!r.ok) { dp.note = "error: " + r.error; return }
        dp.busy = true; dp.jobKind = "setupkey"; poller.start()
    }

    Timer {
        id: poller; interval: 250; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); dp.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            var k = dp.jobKind; dp.jobKind = ""
            if (k === "discover") {
                dp.devices = res.ok ? (res.devices || []) : []
            } else if (k === "trust") {
                dp.note = res.ok ? "✓ host key trusted" : ("error: " + res.error); dp.refreshTrust(); dp.selected()
            } else if (k === "setupkey") {
                dp.sshpass = ""; dp.setupOpen = false
                dp.note = res.ok ? "✓ key installed" : ("error: " + res.error); dp.refreshTrust(); dp.selected()
            }
        }
    }

    Rectangle {
        id: card
        width: dp.width; radius: 10; color: "#171c2b"
        border.color: dp.selHost.length ? "#7aa2f7" : "#2a3350"
        height: body.height + 20
        Column {
            id: body
            x: 12; y: 10; width: parent.width - 24; spacing: 8
            Row {
                width: parent.width; spacing: 8
                Label {
                    anchors.verticalCenter: parent.verticalCenter
                    text: dp.selHost.length ? ("device: " + dp.selName + (dp.hostTrusted ? "  🔒" : "")) : "no device chosen"
                    color: dp.selHost.length ? "#cdd6f4" : "#8a93a6"
                    width: parent.width - chooseBtn.width - 12; elide: Text.ElideRight
                }
                Button { id: chooseBtn; text: dp.open ? "hide" : "choose…"; onClicked: dp.open = !dp.open }
            }
            Label {
                width: parent.width; visible: dp.note.length > 0
                text: dp.note; color: "#a6adc8"; font.pixelSize: 11; wrapMode: Text.WrapAnywhere
            }
            Column {
                width: parent.width; spacing: 8; visible: dp.open
                Row {
                    spacing: 8
                    Button { text: dp.busy ? "…" : "scan LAN"; enabled: !dp.busy; onClicked: dp.discover() }
                    Label { text: "as user"; color: "#8a93a6"; anchors.verticalCenter: parent.verticalCenter }
                    TextField { width: 110; text: dp.user; onTextChanged: dp.user = text }
                }
                Repeater {
                    model: dp.devices
                    delegate: Rectangle {
                        width: parent.width; height: 42; radius: 8
                        color: modelData.ip === dp.selHost ? "#2a3350" : "#1b2335"
                        border.color: modelData.ip === dp.selHost ? "#7aa2f7" : "#2a3350"
                        Column {
                            anchors.verticalCenter: parent.verticalCenter; x: 10
                            Label { text: (modelData.hostname && modelData.hostname.length) ? modelData.hostname : modelData.ip; color: "#cdd6f4"; font.bold: true }
                            Label { text: modelData.ip; color: "#8a93a6"; font.pixelSize: 11 }
                        }
                        TapHandler { onTapped: dp.selectDevice(modelData.ip, modelData.hostname) }
                    }
                }
                Column {
                    width: parent.width; spacing: 6; visible: dp.selHost.length > 0
                    Row {
                        spacing: 8
                        Label {
                            anchors.verticalCenter: parent.verticalCenter
                            text: dp.hostTrusted ? "🔒 trusted" : "host key not trusted"
                            color: dp.hostTrusted ? "#56d364" : "#f9e2af"; font.pixelSize: 12
                        }
                        Button { text: "trust key"; visible: !dp.hostTrusted; enabled: !dp.busy; onClicked: dp.trustHost() }
                        Button { text: dp.setupOpen ? "hide setup" : "set up access"; enabled: !dp.busy; onClicked: dp.setupOpen = !dp.setupOpen }
                    }
                    Column {
                        width: parent.width; spacing: 6; visible: dp.setupOpen
                        Label {
                            width: parent.width; color: "#8a93a6"; font.pixelSize: 11; wrapMode: Text.WordWrap
                            text: "Install your key for '" + dp.user + "' on the device so access is passwordless. Keys expire weekly — re-installing renews them."
                        }
                        TextField {
                            width: parent.width; placeholderText: "device password (used once, not stored)"
                            echoMode: TextInput.Password; text: dp.sshpass; onTextChanged: dp.sshpass = text
                        }
                        Button { text: "install my key"; enabled: !dp.busy && dp.sshpass.length > 0; onClicked: dp.setupKey() }
                    }
                }
            }
        }
    }
}
