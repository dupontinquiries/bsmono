// Settings — local maintenance for this device.
//   • Reclaim disk space: delete the per-crate target/ trees left behind now that
//     gitmind builds go to the shared cargo cache (they're gitignored build output).
//   • Access keys: view + prune the keys gitmind installed on a device.
import QtQuick
import QtQuick.Controls

Item {
    id: root
    property bool busy: false
    property string jobKind: ""
    property string note: ""

    // reclaim
    property var reclaimItems: []
    property string reclaimTotal: ""
    property double reclaimBytes: 0
    property bool scanned: false

    // keys
    property var keys: []

    Component.onCompleted: root.scanReclaim()

    // ── reclaim ──
    function scanReclaim() {
        var r = JSON.parse(backend.reclaimable())
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "rscan"; poller.start()
    }
    function doReclaim() {
        var r = JSON.parse(backend.reclaim())
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "reclaim"; root.note = ""; poller.start()
    }
    // ── keys ──
    function listKeys() {
        if (picker.selHost.length === 0) { root.note = "pick a device first"; return }
        var r = JSON.parse(backend.list_keys(picker.selHost, picker.user))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "keys"; root.note = ""; poller.start()
    }
    function cleanupKeys() {
        if (picker.selHost.length === 0) { root.note = "pick a device first"; return }
        var r = JSON.parse(backend.cleanup_keys(picker.selHost, picker.user))
        if (!r.ok) { root.note = "error: " + r.error; return }
        root.busy = true; root.jobKind = "keyclean"; root.note = ""; poller.start()
    }

    Timer {
        id: poller; interval: 250; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); root.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            var k = root.jobKind; root.jobKind = ""
            if (k === "rscan") {
                root.reclaimItems = res.ok ? (res.items || []) : []
                root.reclaimTotal = res.ok ? res.total : ""
                root.reclaimBytes = res.ok ? res.total_bytes : 0
                root.scanned = true
            } else if (k === "reclaim") {
                if (res.ok) { root.note = "✓ reclaimed " + res.freed + " from " + res.count + " dir(s)" }
                else root.note = "error: " + res.error
                root.scanReclaim()
            } else if (k === "keys") {
                root.keys = res.ok ? (res.keys || []) : []
                if (!res.ok) root.note = "error: " + res.error
            } else if (k === "keyclean") {
                if (res.ok) { root.note = res.removed > 0 ? ("✓ removed " + res.removed + " stale key(s)") : "nothing to clean"; root.listKeys() }
                else root.note = "error: " + res.error
            }
        }
    }

    function fmtExpiry(s) {
        if (!s || s.length < 12) return s || "?"
        return s.substring(0,4) + "-" + s.substring(4,6) + "-" + s.substring(6,8) + " " + s.substring(8,10) + ":" + s.substring(10,12)
    }

    Flickable {
        anchors.fill: parent; anchors.margins: 12
        contentHeight: col.height + 24; clip: true
        Column {
            id: col
            width: parent.width; spacing: 12

            Label { text: "Settings"; color: "#cdd6f4"; font.bold: true; font.pixelSize: 20 }
            Label {
                width: parent.width; visible: root.note.length > 0
                text: root.note; color: "#a6adc8"; font.pixelSize: 12; wrapMode: Text.WrapAnywhere
            }

            // ── reclaim disk space ──────────────────────────────────
            Rectangle {
                width: parent.width; radius: 10; color: "#171c2b"; border.color: "#2a3350"
                height: rcC.height + 20
                Column {
                    id: rcC
                    x: 12; y: 10; width: parent.width - 24; spacing: 8
                    Label { text: "Reclaim disk space"; color: "#7aa2f7"; font.bold: true }
                    Label {
                        width: parent.width; color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                        text: "Per-crate target/ folders are dead weight now that builds share one cache. These are gitignored build output — safe to delete; cargo rebuilds into the shared cache."
                    }
                    Row {
                        spacing: 8
                        Button { text: root.jobKind === "rscan" ? "scanning…" : "rescan"; enabled: !root.busy; onClicked: root.scanReclaim() }
                        Button {
                            text: root.jobKind === "reclaim" ? "reclaiming…" : ("reclaim " + (root.reclaimTotal.length ? root.reclaimTotal : "0 B"))
                            enabled: !root.busy && root.reclaimItems.length > 0
                            onClicked: root.doReclaim()
                        }
                    }
                    Label {
                        width: parent.width; visible: root.scanned && root.reclaimItems.length === 0 && !root.busy
                        text: "Nothing to reclaim — already clean."; color: "#56d364"; font.pixelSize: 12
                    }
                    Label {
                        width: parent.width; visible: root.reclaimItems.length > 0
                        text: root.reclaimItems.length + " folder(s) · " + root.reclaimTotal + " reclaimable"
                        color: "#cdd6f4"; font.pixelSize: 12; font.bold: true
                    }
                    Repeater {
                        model: root.reclaimItems
                        delegate: Row {
                            width: parent.width; spacing: 8
                            Label { text: modelData.size; color: "#f9e2af"; font.pixelSize: 11; font.family: "monospace"; width: 70 }
                            Label { text: modelData.path; color: "#8a93a6"; font.pixelSize: 11; font.family: "monospace"; width: parent.width - 80; elide: Text.ElideLeft }
                        }
                    }
                }
            }

            // ── access keys ─────────────────────────────────────────
            Rectangle {
                width: parent.width; radius: 10; color: "#171c2b"; border.color: "#2a3350"
                height: kHead.height + 16
                Column {
                    id: kHead
                    x: 12; y: 8; width: parent.width - 24; spacing: 4
                    Label { text: "Access keys"; color: "#7aa2f7"; font.bold: true }
                    Label {
                        width: parent.width; color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                        text: "Keys gitmind installed on a device. They expire weekly; re-installing renews them, so prune expired + duplicate entries here. Your own keys are never touched."
                    }
                }
            }

            DevicePicker {
                id: picker; width: parent.width; user: "user"
                onSelected: { if (picker.selHost.length) root.listKeys() }
            }

            Row {
                spacing: 8; visible: picker.selHost.length > 0
                Button { text: root.jobKind === "keys" ? "loading…" : "refresh"; enabled: !root.busy; onClicked: root.listKeys() }
                Button {
                    text: root.jobKind === "keyclean" ? "cleaning…" : "clean up expired + duplicates"
                    enabled: !root.busy && root.keys.length > 0
                    onClicked: root.cleanupKeys()
                }
            }
            Label {
                width: parent.width; visible: picker.selHost.length > 0 && root.keys.length === 0 && !root.busy
                text: "No keys found on this device."; color: "#5b6477"; font.pixelSize: 12
            }
            Repeater {
                model: root.keys
                delegate: Rectangle {
                    width: col.width; radius: 10; color: "#1b2335"
                    border.color: modelData.expired ? "#f7768e" : (modelData.managed ? "#3a4566" : "#2a3350")
                    height: kc.height + 18
                    Column {
                        id: kc
                        x: 12; y: 9; width: parent.width - 24; spacing: 3
                        Row {
                            width: parent.width; spacing: 8
                            Label { text: modelData.id; color: "#cdd6f4"; font.bold: true; font.pixelSize: 14; elide: Text.ElideRight; width: parent.width - badges.width - 8 }
                            Row {
                                id: badges; spacing: 6
                                Rectangle {
                                    visible: !modelData.managed
                                    height: 16; radius: 8; width: yb.width + 12; color: "#2a3350"
                                    Label { id: yb; anchors.centerIn: parent; text: "yours"; color: "#a6adc8"; font.pixelSize: 10 }
                                }
                                Rectangle {
                                    visible: modelData.expired
                                    height: 16; radius: 8; width: eb.width + 12; color: "#3a1622"
                                    Label { id: eb; anchors.centerIn: parent; text: "expired"; color: "#f7768e"; font.pixelSize: 10 }
                                }
                                Rectangle {
                                    visible: modelData.from
                                    height: 16; radius: 8; width: fb.width + 12; color: "#16271b"
                                    Label { id: fb; anchors.centerIn: parent; text: "LAN-only"; color: "#56d364"; font.pixelSize: 10 }
                                }
                            }
                        }
                        Label {
                            text: modelData.type + (modelData.managed ? ("  ·  expires " + root.fmtExpiry(modelData.expiry)) : "  ·  not managed by gitmind")
                            color: "#8a93a6"; font.pixelSize: 11
                        }
                    }
                }
            }
        }
    }
}
