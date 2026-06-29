// Import — take a .zip of a Rust app, extract it to a fresh temp dir, then either run
// it once (ephemeral) or save it into your repos area (~/Code/<lang>/<name> by default,
// path configurable) initialized with gitmind or plain git — then build and launch.
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs

Item {
    id: root
    property string importState: "idle"  // idle | importing | found | failed
    property string buildState: "idle"   // idle | building | ran | failed
    property string app: ""
    property string cratePath: ""         // current crate dir (temp, or placed path)
    property string dir: ""
    property string desc: ""
    property bool isBin: true
    property string binary: ""
    property string buildError: ""
    property bool busy: false
    property string jobKind: ""
    property string note: ""

    // save destination
    property var bases: []
    property string base: ""
    property string lang: ""
    property string dest: ""
    property string initMode: "gitmind"   // gitmind | git | none
    property bool placed: false
    property bool runAfterPlace: false

    function importZip(url) {
        var r = JSON.parse(backend.import_zip(url.toString()))
        if (!r.ok) { root.importState = "failed"; root.note = "✗ " + r.error; return }
        root.importState = "importing"; root.note = ""; root.buildState = "idle"; root.placed = false
        root.busy = true; root.jobKind = "import"; poller.start()
    }
    function loadBases() {
        var r = JSON.parse(backend.repos_bases())
        if (!r.ok) return
        root.bases = r.bases || []
        if (root.base.length === 0) root.base = r.default
        root.recomputeDest()
    }
    function recomputeDest() {
        if (root.cratePath.length === 0 || root.base.length === 0) return
        var r = JSON.parse(backend.import_dest(root.cratePath, root.base))
        if (r.ok) { root.dest = r.dest; root.lang = r.lang }
    }
    function buildAndRun() {
        if (root.cratePath.length === 0) { root.note = "import an app first"; return }
        var r = JSON.parse(backend.build_run(root.cratePath, root.app))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.buildState = "building"; root.buildError = ""; root.note = ""
        root.busy = true; root.jobKind = "buildrun"; poller.start()
    }
    function saveProject(thenRun) {
        if (root.dest.length === 0) { root.note = "choose a destination"; return }
        var r = JSON.parse(backend.place_app(root.cratePath, root.dest, root.base, root.initMode))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.runAfterPlace = thenRun; root.note = ""
        root.busy = true; root.jobKind = "place"; poller.start()
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
                    root.loadBases()
                } else { root.importState = "failed"; root.note = "✗ " + res.error }
            } else if (k === "place") {
                if (res.ok) {
                    root.placed = true; root.cratePath = res.path; root.dir = res.path
                    root.note = "✓ saved to " + res.path + (res.init !== "none" ? ("  (" + res.init + " initialized)") : "")
                    if (root.runAfterPlace && root.isBin) root.buildAndRun()
                } else { root.note = "✗ " + res.error }
            } else if (k === "buildrun") {
                if (res.ok) { root.buildState = "ran"; root.binary = res.binary; root.note = "✓ launched " + root.app + (root.placed ? " from " + root.dir : "") }
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
                text: "Pick a .zip with a Rust app. gitmind extracts it to a fresh temp folder, finds the crate, and either runs it once or saves it into your repos area — initialized with gitmind or git — then builds and launches it."
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
                    x: 12; y: 10; width: parent.width - 24; spacing: 8
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
                        width: parent.width; visible: !root.placed
                        text: "extracted to " + root.dir; color: "#5b6477"; font.pixelSize: 10; font.family: "monospace"
                        elide: Text.ElideMiddle
                    }

                    Rectangle { width: parent.width; height: 1; color: "#2a3350" }

                    Label { text: "Initialize as"; color: "#8a93a6"; font.pixelSize: 12 }
                    Row {
                        spacing: 8
                        Repeater {
                            model: [["gitmind", "gitmind + git"], ["git", "git only"], ["none", "none"]]
                            delegate: Rectangle {
                                height: 30; radius: 15; width: il.width + 22
                                color: modelData[0] === root.initMode ? "#2a3350" : "#1b2335"
                                border.color: modelData[0] === root.initMode ? "#7aa2f7" : "#3a4566"
                                Label { id: il; anchors.centerIn: parent; text: modelData[1]; color: "#cdd6f4"; font.pixelSize: 12 }
                                TapHandler { onTapped: root.initMode = modelData[0] }
                            }
                        }
                    }

                    Label { text: "Save in"; color: "#8a93a6"; font.pixelSize: 12 }
                    ComboBox {
                        id: baseBox; width: parent.width; editable: true
                        model: root.bases
                        Component.onCompleted: editText = root.base
                        onActivated: { root.base = currentText; root.recomputeDest() }
                        onAccepted: { root.base = editText; root.recomputeDest() }
                    }
                    Label {
                        width: parent.width; color: "#5b6477"; font.pixelSize: 11
                        text: "language folder: " + (root.lang.length ? root.lang : "…") + " · type a new path above to create one"
                    }
                    TextField {
                        id: destField; width: parent.width
                        placeholderText: "destination path"
                        text: root.dest
                        onTextEdited: root.dest = text
                    }

                    Row {
                        spacing: 8
                        Button {
                            text: root.jobKind === "place" ? "saving…" : (root.jobKind === "buildrun" ? "building…" : "Save & run")
                            enabled: !root.busy && root.dest.length > 0
                            onClicked: root.saveProject(true)
                        }
                        Button {
                            text: "Save only"
                            enabled: !root.busy && root.dest.length > 0 && !root.placed
                            onClicked: root.saveProject(false)
                        }
                        Button {
                            text: "Run once"
                            enabled: !root.busy && root.isBin && !root.placed
                            onClicked: root.buildAndRun()
                        }
                    }
                    Label {
                        width: parent.width; visible: !root.isBin
                        text: "This is a library crate — nothing to launch, but you can still save it."
                        color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap
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
