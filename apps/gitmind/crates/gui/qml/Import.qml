// Import — take a .zip of a Rust app, extract it to a fresh temp dir, build it, and
// launch it. Everything through the GUI: a file picker, one build, one run.
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs

Item {
    id: root
    property string importState: "idle"  // idle | importing | found | failed
    property string buildState: "idle"   // idle | building | ran | failed
    property string app: ""
    property string cratePath: ""
    property string dir: ""
    property string desc: ""
    property bool isBin: true
    property string binary: ""
    property string buildError: ""
    property bool busy: false
    property string jobKind: ""
    property string note: ""

    function importZip(url) {
        var r = JSON.parse(backend.import_zip(url.toString()))
        if (!r.ok) { root.importState = "failed"; root.note = "✗ " + r.error; return }
        root.importState = "importing"; root.note = ""; root.buildState = "idle"
        root.busy = true; root.jobKind = "import"; poller.start()
    }
    function buildAndRun() {
        if (root.cratePath.length === 0) { root.note = "import an app first"; return }
        var r = JSON.parse(backend.build_run(root.cratePath, root.app))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.buildState = "building"; root.buildError = ""; root.note = ""
        root.busy = true; root.jobKind = "buildrun"; poller.start()
    }

    Timer {
        id: poller; interval: 300; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            poller.stop(); root.busy = false
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            var k = root.jobKind; root.jobKind = ""
            if (k === "import") {
                if (res.ok) {
                    root.importState = "found"; root.app = res.app; root.cratePath = res.crate_path
                    root.dir = res.dir; root.desc = res.description || ""; root.isBin = (res.is_bin !== false)
                } else { root.importState = "failed"; root.note = "✗ " + res.error }
            } else if (k === "buildrun") {
                if (res.ok) { root.buildState = "ran"; root.binary = res.binary; root.note = "✓ launched " + res.app }
                else { root.buildState = "failed"; root.buildError = res.error; root.note = "" }
            }
        }
    }

    FileDialog {
        id: fileDialog
        title: "Choose a .zip with a Rust app"
        nameFilters: ["Zip archives (*.zip)", "All files (*)"]
        onAccepted: root.importZip(fileDialog.selectedFile)
    }

    Flickable {
        anchors.fill: parent; anchors.margins: 12
        contentHeight: col.height + 24; clip: true
        Column {
            id: col
            width: parent.width; spacing: 12

            Label { text: "Import & run"; color: "#cdd6f4"; font.bold: true; font.pixelSize: 20 }
            Label {
                width: parent.width
                text: "Pick a .zip containing a Rust app. gitmind extracts it to a fresh temp folder, finds the crate, builds it for this device, and launches it."
                color: "#5b6477"; font.pixelSize: 12; wrapMode: Text.WordWrap
            }
            Label {
                width: parent.width; visible: root.note.length > 0
                text: root.note; color: root.note.indexOf("✓") === 0 ? "#56d364" : "#f7768e"
                font.pixelSize: 13; wrapMode: Text.WrapAnywhere
            }

            Button {
                text: root.jobKind === "import" ? "extracting…" : "Choose a .zip…"
                enabled: !root.busy
                onClicked: fileDialog.open()
            }

            // ── found crate ──
            Rectangle {
                width: parent.width; radius: 10; color: "#171c2b"; border.color: "#7aa2f7"
                visible: root.importState === "found"
                height: fc.height + 20
                Column {
                    id: fc
                    x: 12; y: 10; width: parent.width - 24; spacing: 6
                    Row {
                        spacing: 8
                        Label { text: root.app; color: "#cdd6f4"; font.bold: true; font.pixelSize: 17 }
                        Rectangle {
                            visible: !root.isBin; anchors.verticalCenter: parent.verticalCenter
                            height: 16; radius: 8; width: kb.width + 12; color: "#3a1622"
                            Label { id: kb; anchors.centerIn: parent; text: "library — no app to run"; color: "#f7768e"; font.pixelSize: 10 }
                        }
                    }
                    Label {
                        width: parent.width; visible: root.desc.length > 0
                        text: root.desc; color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap
                    }
                    Label {
                        width: parent.width
                        text: root.dir; color: "#5b6477"; font.pixelSize: 10; font.family: "monospace"
                        elide: Text.ElideMiddle
                    }
                    Button {
                        text: root.jobKind === "buildrun" ? "building…" : (root.buildState === "ran" ? "run again" : "Build & run")
                        enabled: !root.busy && root.isBin
                        onClicked: root.buildAndRun()
                    }
                }
            }

            // ── build error ──
            Rectangle {
                width: parent.width; radius: 10; color: "#1c1420"; border.color: "#f7768e"
                visible: root.buildState === "failed" && root.buildError.length > 0
                height: be.height + 20
                Column {
                    id: be
                    x: 12; y: 10; width: parent.width - 24; spacing: 4
                    Label { text: "Build failed"; color: "#f7768e"; font.bold: true }
                    Label {
                        width: parent.width; text: root.buildError
                        color: "#cdd6f4"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere
                    }
                }
            }

            Label {
                width: parent.width; visible: root.buildState === "ran"
                text: "Running: " + root.binary
                color: "#56d364"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere
            }
        }
    }
}
