// Import — take a .zip of a Rust app and either:
//   • New project: extract into your repos area, init git/gitmind, build + run;
//   • Update existing: overlay the zip's files onto a git repo on a fresh branch;
//   • Merge histories: when the zip is itself a git repo (its own commits/authors),
//     try a matrix of merge/rebase strategies in a safe scratch copy, ranked, then land
//     the chosen one as a branch in the real repo.
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs

Item {
    id: root
    property string importState: "idle"
    property string buildState: "idle"
    property string app: ""
    property string cratePath: ""
    property string dir: ""
    property string zipName: ""
    property string desc: ""
    property bool isBin: true
    property string system: ""
    property string binary: ""
    property string buildError: ""
    property bool busy: false
    property string jobKind: ""
    property string note: ""

    property string mode: "new"           // new | update | merge
    property bool hasGit: false
    property bool hasGitmind: false
    property string zipRepo: ""
    property var modeOptions: root.hasGit
        ? [["new", "New project"], ["update", "Update existing"], ["merge", "Merge histories"]]
        : [["new", "New project"], ["update", "Update existing"]]

    // new-project
    property var bases: []
    property string base: ""
    property string lang: ""
    property string dest: ""
    property string initMode: "gitmind"
    property bool placed: false
    property bool runAfterPlace: false
    property string runFlags: ""

    // shared target repo (update + merge)
    property var repos: []
    property bool reposLoaded: false
    property string targetRepo: ""

    // update
    property string applyMode: "overlay"
    property var applyResult: null

    // merge
    property var trials: []
    property string workspace: ""
    property string targetBranch: ""
    property bool related: true
    property bool explored: false

    function importZip(url) {
        var s = url.toString()
        root.zipName = s.replace(/^.*\//, "")
        var r = JSON.parse(backend.import_zip(s))
        if (!r.ok) { root.importState = "failed"; root.note = "✗ " + r.error; return }
        root.importState = "importing"; root.note = ""; root.buildState = "idle"
        root.placed = false; root.applyResult = null; root.trials = []; root.explored = false
        root.busy = true; root.jobKind = "import"; poller.start()
    }
    function loadBases() {
        var r = JSON.parse(backend.repos_bases()); if (!r.ok) return
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
        var r = JSON.parse(backend.build_run(root.cratePath, root.app))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.buildState = "building"; root.buildError = ""; root.note = ""
        root.busy = true; root.jobKind = "buildrun"; poller.start()
    }
    function addToApps() {
        if (root.cratePath.length === 0) { root.note = "import an app first"; return }
        var r = JSON.parse(backend.add_app(root.cratePath, root.app, root.runFlags))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.buildError = ""; root.note = ""
        root.busy = true; root.jobKind = "addapp"; poller.start()
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
    function exploreMerge() {
        if (root.targetRepo.length === 0) { root.note = "pick a repo to merge into"; return }
        var r = JSON.parse(backend.explore_merge(root.zipRepo, root.targetRepo))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.trials = []; root.explored = false; root.note = ""
        root.busy = true; root.jobKind = "explore"; poller.start()
    }
    function applyMerge(branch, id) {
        var r = JSON.parse(backend.apply_merge(root.workspace, branch, root.targetRepo, id))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.note = ""
        root.busy = true; root.jobKind = "merge"; poller.start()
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
                    root.dir = res.dir; root.desc = res.description || ""; root.isBin = (res.is_bin !== false); root.system = res.system || ""
                    root.hasGit = (res.has_git === true); root.hasGitmind = (res.has_gitmind === true)
                    root.zipRepo = res.zip_repo || ""; root.mode = "new"
                    root.loadBases()
                } else { root.importState = "failed"; root.note = "✗ " + res.error }
            } else if (k === "place") {
                if (res.ok) {
                    root.placed = true; root.cratePath = res.path; root.dir = res.path
                    root.note = "✓ saved to " + res.path + (res.init !== "none" ? ("  (" + res.init + " initialized)") : "")
                    if (root.runAfterPlace && root.isBin) root.buildAndRun()
                } else { root.note = "✗ " + res.error }
            } else if (k === "lrepos") {
                root.repos = res.ok ? (res.repos || []) : []; root.reposLoaded = true
                if (root.targetRepo.length === 0) {
                    for (var i = 0; i < root.repos.length; i++) {
                        if (root.repos[i].name.toLowerCase() === root.app.toLowerCase()) { root.targetRepo = root.repos[i].path; break }
                    }
                }
            } else if (k === "apply") {
                if (res.ok) {
                    root.applyResult = res
                    root.note = (res.changes === 0) ? "✓ " + res.msg : "✓ applied on branch " + res.branch + " — " + res.shortstat
                } else { root.note = "✗ " + res.error }
            } else if (k === "explore") {
                if (res.ok) {
                    root.trials = res.trials || []; root.workspace = res.workspace
                    root.targetBranch = res.target_branch; root.related = (res.related === true); root.explored = true
                    var clean = 0; for (var t = 0; t < root.trials.length; t++) if (root.trials[t].ok) clean++
                    root.note = "explored " + root.trials.length + " strategies · " + clean + " apply cleanly" + (root.related ? "" : " · unrelated histories")
                } else { root.note = "✗ " + res.error }
            } else if (k === "merge") {
                if (res.ok) { root.note = "✓ landed on branch " + res.branch + " — review and merge it"; root.explored = false; root.trials = [] }
                else { root.note = "✗ " + res.error }
            } else if (k === "buildrun") {
                if (res.ok) { root.buildState = "ran"; root.binary = res.binary; root.note = "✓ launched " + root.app + (root.placed ? " from " + root.dir : "") }
                else { root.buildState = "failed"; root.buildError = res.error; root.note = "" }
            } else if (k === "addapp") {
                if (res.ok) { root.note = "✓ added " + res.name + " to your apps — find it on the Apps tab" + (res.run_args && res.run_args.length ? (" (flags: " + res.run_args + ")") : "") }
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
                text: "Pick a .zip. Start a new project, apply it onto an existing repo, or — if it carries its own git history — explore ways to merge the two."
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
                            visible: root.system.length > 0; anchors.verticalCenter: parent.verticalCenter
                            height: 16; radius: 8; width: sysb.width + 12; color: "#1b2b3a"
                            Label { id: sysb; anchors.centerIn: parent; text: root.system; color: "#7aa2f7"; font.pixelSize: 10 }
                        }
                        Rectangle {
                            visible: root.hasGit; anchors.verticalCenter: parent.verticalCenter
                            height: 16; radius: 8; width: gb.width + 12; color: "#16271b"
                            Label { id: gb; anchors.centerIn: parent; text: root.hasGitmind ? "has gitmind history" : "has git history"; color: "#56d364"; font.pixelSize: 10 }
                        }
                        Rectangle {
                            visible: !root.isBin; anchors.verticalCenter: parent.verticalCenter
                            height: 16; radius: 8; width: kb.width + 12; color: "#3a1622"
                            Label { id: kb; anchors.centerIn: parent; text: "library"; color: "#f7768e"; font.pixelSize: 10 }
                        }
                    }
                    Label { width: parent.width; visible: root.desc.length > 0; text: root.desc; color: "#8a93a6"; font.pixelSize: 12; wrapMode: Text.WordWrap }
                    Label { width: parent.width; text: "extracted to " + root.dir; color: "#5b6477"; font.pixelSize: 10; font.family: "monospace"; elide: Text.ElideMiddle }

                    Row {
                        spacing: 8
                        Repeater {
                            model: root.modeOptions
                            delegate: Rectangle {
                                height: 32; radius: 16; width: ml.width + 22
                                color: modelData[0] === root.mode ? "#23304d" : "#1b2335"
                                border.color: modelData[0] === root.mode ? "#7aa2f7" : "#3a4566"
                                Label { id: ml; anchors.centerIn: parent; text: modelData[1]; color: "#cdd6f4"; font.pixelSize: 13 }
                                TapHandler { onTapped: { root.mode = modelData[0]; if (modelData[0] !== "new") root.loadRepos() } }
                            }
                        }
                    }
                    Rectangle { width: parent.width; height: 1; color: "#2a3350" }

                    // ───── NEW PROJECT ─────
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
                            id: baseBox; width: parent.width; editable: true; model: root.bases
                            Component.onCompleted: editText = root.base
                            onActivated: { root.base = currentText; root.recomputeDest() }
                            onAccepted: { root.base = editText; root.recomputeDest() }
                        }
                        Label { width: parent.width; color: "#5b6477"; font.pixelSize: 11; text: "language folder: " + (root.lang.length ? root.lang : "…") + " · type a new path to create one" }
                        TextField { width: parent.width; placeholderText: "destination path"; text: root.dest; onTextEdited: root.dest = text }
                        Row {
                            spacing: 8
                            Button { text: root.jobKind === "place" ? "saving…" : (root.jobKind === "buildrun" ? "building…" : "Save & run"); enabled: !root.busy && root.dest.length > 0; onClicked: root.saveProject(true) }
                            Button { text: "Save only"; enabled: !root.busy && root.dest.length > 0 && !root.placed; onClicked: root.saveProject(false) }
                            Button { text: "Run once"; enabled: !root.busy && root.isBin && !root.placed; onClicked: root.buildAndRun() }
                        }
                        // add the built app to the local apps list, runnable from the Apps tab with flags
                        Column {
                            width: parent.width; spacing: 6; visible: root.isBin
                            Rectangle { width: parent.width; height: 1; color: "#2a3350" }
                            Label { text: "Add to my apps"; color: "#8a93a6"; font.pixelSize: 12 }
                            TextField { width: parent.width; placeholderText: "launch flags (optional), e.g. --verbose --port 8080"; text: root.runFlags; onTextEdited: root.runFlags = text }
                            Button { text: root.jobKind === "addapp" ? "adding…" : "Add to my apps"; enabled: !root.busy; onClicked: root.addToApps() }
                            Label { width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap; text: "Installs the built binary into your apps list (with a launcher) so you can run it locally any time." }
                        }
                    }

                    // ───── SHARED repo picker (update + merge) ─────
                    Column {
                        width: parent.width; spacing: 8; visible: root.mode !== "new"
                        Label { text: root.jobKind === "lrepos" ? (root.mode === "merge" ? "Merge into (scanning…)" : "Repo (scanning…)") : (root.mode === "merge" ? "Merge into" : "Repo to update"); color: "#8a93a6"; font.pixelSize: 12 }
                        ComboBox {
                            id: repoBox; width: parent.width; model: root.repos; textRole: "name"; valueRole: "path"
                            onActivated: root.targetRepo = currentValue
                        }
                        TextField { width: parent.width; placeholderText: "repo path"; text: root.targetRepo; onTextEdited: root.targetRepo = text }
                    }

                    // ───── UPDATE ─────
                    Column {
                        width: parent.width; spacing: 8; visible: root.mode === "update"
                        Label { width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap; text: "Overlays the zip's files onto a clean repo on a new branch — nothing overwritten in place." }
                        Row {
                            spacing: 8
                            Repeater {
                                model: [["overlay", "overlay (add + update)"], ["mirror", "mirror (match zip)"]]
                                delegate: Rectangle {
                                    height: 30; radius: 15; width: al.width + 22
                                    color: modelData[0] === root.applyMode ? "#2a3350" : "#1b2335"
                                    border.color: modelData[0] === root.applyMode ? "#7aa2f7" : "#3a4566"
                                    Label { id: al; anchors.centerIn: parent; text: modelData[1]; color: "#cdd6f4"; font.pixelSize: 12 }
                                    TapHandler { onTapped: root.applyMode = modelData[0] }
                                }
                            }
                        }
                        Label { width: parent.width; visible: root.applyMode === "mirror"; text: "Mirror deletes files the zip dropped (untracked files would be lost)."; color: "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap }
                        Button { text: root.jobKind === "apply" ? "applying…" : "Apply to a branch"; enabled: !root.busy && root.targetRepo.length > 0; onClicked: root.applyToRepo() }
                        Rectangle {
                            width: parent.width; radius: 8; color: "#0f1320"; border.color: "#2a3350"
                            visible: root.applyResult !== null && root.applyResult.ok && root.applyResult.changes > 0
                            height: ar.height + 16
                            Column {
                                id: ar
                                x: 10; y: 8; width: parent.width - 20; spacing: 4
                                Label { width: parent.width; color: "#56d364"; font.pixelSize: 12; font.bold: true; text: root.applyResult ? (root.applyResult.shortstat || "") : "" }
                                Label { width: parent.width; color: "#8a93a6"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere; text: root.applyResult ? ("review: git -C " + root.targetRepo + " diff " + root.applyResult.base + ".." + root.applyResult.branch) : "" }
                                Repeater {
                                    model: root.applyResult ? root.applyResult.files : []
                                    delegate: Label { width: ar.width; text: modelData.status + "  " + modelData.path; color: modelData.status.indexOf("D") === 0 ? "#f7768e" : (modelData.status.indexOf("A") === 0 ? "#56d364" : "#a6adc8"); font.pixelSize: 11; font.family: "monospace"; elide: Text.ElideMiddle }
                                }
                            }
                        }
                    }

                    // ───── MERGE HISTORIES ─────
                    Column {
                        width: parent.width; spacing: 8; visible: root.mode === "merge"
                        Label { width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap; text: "The zip is a git repo with its own commits. gitmind tries several merge/rebase strategies in a scratch copy (your repo is untouched), ranks them, and lets you land the best as a new branch." }
                        Button { text: root.jobKind === "explore" ? "exploring…" : "Explore strategies"; enabled: !root.busy && root.targetRepo.length > 0; onClicked: root.exploreMerge() }
                        Label { width: parent.width; visible: root.explored; color: "#8a93a6"; font.pixelSize: 11; text: "into " + root.targetBranch + (root.related ? " · shared history" : " · unrelated histories") }
                        Repeater {
                            model: root.trials
                            delegate: Rectangle {
                                width: fc.width - 0; radius: 8; color: "#0f1320"
                                border.color: modelData.recommended ? "#56d364" : (modelData.ok ? "#3a4566" : "#3a2230")
                                height: tr.height + 16
                                Column {
                                    id: tr
                                    x: 10; y: 8; width: parent.width - 20; spacing: 4
                                    Row {
                                        width: parent.width; spacing: 8
                                        Label { text: modelData.label; color: "#cdd6f4"; font.bold: true; font.pixelSize: 13; width: parent.width - rb.width - 8; elide: Text.ElideRight }
                                        Rectangle {
                                            id: rb; visible: modelData.recommended; anchors.verticalCenter: parent.verticalCenter
                                            height: 16; radius: 8; width: rbl.width + 12; color: "#16271b"
                                            Label { id: rbl; anchors.centerIn: parent; text: "recommended"; color: "#56d364"; font.pixelSize: 10 }
                                        }
                                    }
                                    Label { width: parent.width; text: modelData.summary; color: modelData.ok ? "#a6adc8" : "#f9e2af"; font.pixelSize: 11; wrapMode: Text.WordWrap }
                                    Button {
                                        text: root.jobKind === "merge" ? "landing…" : "Land as branch"
                                        visible: modelData.ok; enabled: !root.busy
                                        onClicked: root.applyMerge(modelData.branch, modelData.id)
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Rectangle {
                width: parent.width; radius: 10; color: "#1c1420"; border.color: "#f7768e"
                visible: root.buildState === "failed" && root.buildError.length > 0
                height: be.height + 20
                Column {
                    id: be
                    x: 12; y: 10; width: parent.width - 24; spacing: 4
                    Label { text: "Build failed"; color: "#f7768e"; font.bold: true }
                    Label { width: parent.width; text: root.buildError; color: "#cdd6f4"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere }
                }
            }
            Label { width: parent.width; visible: root.buildState === "ran"; text: "Running: " + root.binary; color: "#56d364"; font.pixelSize: 11; font.family: "monospace"; wrapMode: Text.WrapAnywhere }
        }
    }
}
