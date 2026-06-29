// Import — take a .zip of a Rust app and either:
//   • New project: extract to your repos area (~/Code/<lang>/<name>), init git/gitmind,
//     build + run; or
//   • Update existing: overlay the zip onto an existing git repo as a reviewable change
//     on a fresh branch (nothing overwritten in place; layout drift + edits show as a
//     diff you commit/merge yourself).
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
    property string zipName: ""
    property string desc: ""
    property bool isBin: true
    property string binary: ""
    property string buildError: ""
    property bool busy: false
    property string jobKind: ""
    property string note: ""

    property string mode: "new"           // new | update

    // new-project destination
    property var bases: []
    property string base: ""
    property string lang: ""
    property string dest: ""
    property string initMode: "gitmind"
    property bool placed: false
    property bool runAfterPlace: false

    // update-existing
    property var repos: []
    property bool reposLoaded: false
    property string targetRepo: ""
    property string applyMode: "overlay"  // overlay | mirror
    property var applyResult: null

    function importZip(url) {
        var s = url.toString()
        root.zipName = s.replace(/^.*\//, "")
        var r = JSON.parse(backend.import_zip(s))
        if (!r.ok) { root.importState = "failed"; root.note = "✗ " + r.error; return }
        root.importState = "importing"; root.note = ""; root.buildState = "idle"
        root.placed = false; root.applyResult = null
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
    function loadRepos() {
        if (root.reposLoaded || root.busy) return
        var r = JSON.parse(backend.local_repos())
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.busy = true; root.jobKind = "lrepos"; poller.start()
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
    function applyToRepo() {
        if (root.targetRepo.length === 0) { root.note = "pick a repo to update"; return }
        var r = JSON.parse(backend.apply_to_repo(root.cratePath, root.targetRepo, root.applyMode, root.zipName))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.applyResult = null; root.note = ""
        root.busy = true; root.jobKind = "apply"; poller.start()
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
            } else if (k === "lrepos") {
                root.repos = res.ok ? (res.repos || []) : []
                root.reposLoaded = true
                if (root.targetRepo.length === 0) {
                    for (var i = 0; i < root.repos.length; i++) {
                        if (root.repos[i].name.toLowerCase() === root.app.toLowerCase()) { root.targetRepo = root.repos[i].path; break }
                    }
                }
            } else if (k === "apply") {
                if (res.ok) {
                    root.applyResult = res
                    root.note = (res.changes === 0) ? "✓ " + res.msg
                        : "✓ applied on branch " + res.branch + " — " + res.shortstat
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
                text: "Pick a .zip with a Rust app. Start a new project from it, or apply it onto an existing repo as a reviewable branch."
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
                        width: parent.width
                        text: "extracted to " + root.dir; color: "#5b6477"; font.pixelSize: 10; font.family: "monospace"
                        elide: Text.ElideMiddle
                    }

                    // New vs Update
                    Row {
                        spacing: 8
                        Repeater {
                            model: [["new", "New project"], ["update", "Update existing repo"]]
                            delegate: Rectangle {
                                height: 32; radius: 16; width: ml.width + 24
                                color: modelData[0] === root.mode ? "#23304d" : "#1b2335"
                                border.color: modelData[0] === root.mode ? "#7aa2f7" : "#3a4566"
                                Label { id: ml; anchors.centerIn: parent; text: modelData[1]; color: "#cdd6f4"; font.pixelSize: 13 }
                                TapHandler { onTapped: { root.mode = modelData[0]; if (modelData[0] === "update") root.loadRepos() } }
                            }
                        }
                    }
                    Rectangle { width: parent.width; height: 1; color: "#2a3350" }

                    // ───────── NEW PROJECT ─────────
                    Column {
                        width: parent.width; spacing: 8; visible: root.mode === "new"
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
                                text: "Save only"; enabled: !root.busy && root.dest.length > 0 && !root.placed
                                onClicked: root.saveProject(false)
                            }
                            Button {
                                text: "Run once"; enabled: !root.busy && root.isBin && !root.placed
                                onClicked: root.buildAndRun()
                            }
                        }
                        Label {
                            width: parent.width; visible: !root.isBin
                            text: "This is a library crate — nothing to launch, but you can still save it."
                            color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap
                        }
                    }

                    // ───────── UPDATE EXISTING ─────────
                    Column {
                        width: parent.width; spacing: 8; visible: root.mode === "update"
                        Label {
                            width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap
                            text: "Applies the zip onto a clean git repo on a new branch (gitmind-import-…). Nothing is overwritten in place — review the diff and merge it yourself."
                        }
                        Label { text: root.jobKind === "lrepos" ? "Repo (scanning…)" : "Repo to update"; color: "#8a93a6"; font.pixelSize: 12 }
                        ComboBox {
                            id: repoBox; width: parent.width
                            model: root.repos; textRole: "name"; valueRole: "path"
                            onActivated: root.targetRepo = currentValue
                        }
                        TextField {
                            width: parent.width; placeholderText: "repo path"
                            text: root.targetRepo; onTextEdited: root.targetRepo = text
                        }
                        Label { text: "How to apply"; color: "#8a93a6"; font.pixelSize: 12 }
                        Row {
                            spacing: 8
                            Repeater {
                                model: [["overlay", "overlay (add + update)"], ["mirror", "mirror (match zip exactly)"]]
                                delegate: Rectangle {
                                    height: 30; radius: 15; width: al.width + 22
                                    color: modelData[0] === root.applyMode ? "#2a3350" : "#1b2335"
                                    border.color: modelData[0] === root.applyMode ? "#7aa2f7" : "#3a4566"
                                    Label { id: al; anchors.centerIn: parent; text: modelData[1]; color: "#cdd6f4"; font.pixelSize: 12 }
                                    TapHandler { onTapped: root.applyMode = modelData[0] }
                                }
                            }
                        }
                        Label {
                            width: parent.width; visible: root.applyMode === "mirror"
                            text: "Mirror also deletes files the zip dropped. It's on a branch so committed history is safe, but untracked files would be lost."
                            color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap
                        }
                        Button {
                            text: root.jobKind === "apply" ? "applying…" : "Apply to a branch"
                            enabled: !root.busy && root.targetRepo.length > 0
                            onClicked: root.applyToRepo()
                        }

                        // result
                        Rectangle {
                            width: parent.width; radius: 8; color: "#0f1320"; border.color: "#2a3350"
                            visible: root.applyResult !== null && root.applyResult.ok && root.applyResult.changes > 0
                            height: ar.height + 16
                            Column {
                                id: ar
                                x: 10; y: 8; width: parent.width - 20; spacing: 4
                                Label {
                                    width: parent.width; color: "#56d364"; font.pixelSize: 12; font.bold: true
                                    text: root.applyResult ? (root.applyResult.shortstat || "") : ""
                                }
                                Label {
                                    width: parent.width; color: "#8a93a6"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere
                                    text: root.applyResult ? ("review: git -C " + root.targetRepo + " diff " + root.applyResult.base + ".." + root.applyResult.branch) : ""
                                }
                                Repeater {
                                    model: root.applyResult ? root.applyResult.files : []
                                    delegate: Label {
                                        width: ar.width
                                        text: modelData.status + "  " + modelData.path
                                        color: modelData.status.indexOf("D") === 0 ? "#f7768e" : (modelData.status.indexOf("A") === 0 ? "#56d364" : "#a6adc8")
                                        font.pixelSize: 11; font.family: "monospace"; elide: Text.ElideMiddle
                                    }
                                }
                            }
                        }
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
