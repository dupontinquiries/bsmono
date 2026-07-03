// Version-pinned binaries — touch-first.
//   • tap a version       → make it current (atomic ~/.local/bin/<app> relink)
//   • "pin" on a version  → write a .desktop entry (GNOME app grid / favorites)
//   • × on a version      → remove that version
// Backed by the binpin module; binaries are content-addressed in the var store
// so unchanged builds across commits are stored once.
import QtQuick
import QtQuick.Controls

Item {
    id: root
    property var apps: []
    property string note: ""

    // suites
    property var suites: []
    property bool busy: false
    property string jobKind: ""
    property var installQueue: []
    property int installIdx: 0
    property int installOk: 0
    property int installTotal: 0
    property string installingSuite: ""

    // BigSilicon app store groups
    property var groups: []

    // Downloads inbox
    property var downloads: []
    property string downloadsDir: ""

    function reload() {
        var r = JSON.parse(backend.apps_json())
        apps = r.ok ? (r.apps || []) : []
    }
    function loadDownloads() {
        var r = JSON.parse(backend.downloads())
        root.downloads = r.ok ? (r.items || []) : []
        root.downloadsDir = r.dir || ""
    }
    function installZip(path) {
        if (root.busy) return
        var r = JSON.parse(backend.install_zip_app(path))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.busy = true; root.jobKind = "zipapp"; root.note = "building…"; suitePoller.start()
    }
    function installBundle(path) {
        if (root.busy) return
        var r = JSON.parse(backend.import_bundle(path))
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.installSuite(r)   // r has {name, apps} — reuse the suite installer
    }
    function loadSuites() {
        var r = JSON.parse(backend.suites())
        root.suites = r.ok ? (r.suites || []) : []
    }
    function loadGroups() {
        var r = JSON.parse(backend.store_groups())
        root.groups = r.ok ? (r.groups || []) : []
    }
    function buildGroup(g) {
        // reuse the per-app suite installer so we get 3/16-style progress
        root.installSuite(g)
    }
    // user@host of the device to install a whole group onto (desktop builds, phone runs)
    property string groupDevice: ""
    function installGroupToDevice(g) {
        var at = root.groupDevice.indexOf("@")
        if (at < 1 || at === root.groupDevice.length - 1) { root.note = "enter the device as user@host"; return }
        var user = root.groupDevice.substring(0, at)
        var host = root.groupDevice.substring(at + 1)
        var r = JSON.parse(backend.build_group_remote(g.name, user, host, ""))
        if (r.ok) { root.busy = true; root.jobKind = "buildgroup"; root.note = "building " + g.name + " here for " + host + "…"; suitePoller.start() }
        else root.note = "✗ " + r.error
    }
    function makeDefaultSuite() {
        var r = JSON.parse(backend.create_default_suite())
        if (!r.ok) { root.note = "✗ " + r.error; return }
        root.busy = true; root.jobKind = "mksuite"; root.note = "scanning for apps…"; suitePoller.start()
    }
    function deleteSuite(name) {
        JSON.parse(backend.delete_suite(name)); root.loadSuites()
    }
    function installSuite(s) {
        if (root.busy) return
        root.installQueue = s.apps || []; root.installIdx = 0; root.installOk = 0
        root.installTotal = root.installQueue.length; root.installingSuite = s.name
        root.installNext()
    }
    function installNext() {
        if (root.installIdx >= root.installQueue.length) {
            root.note = "✓ " + root.installingSuite + ": installed " + root.installOk + "/" + root.installTotal + " — launchers added to your app grid"
            root.installingSuite = ""; root.busy = false; root.reload(); return
        }
        var a = root.installQueue[root.installIdx]
        var r = JSON.parse(backend.install_suite_app(a.name, a.path, a.features || "", a.target || "", a.run_flags || ""))
        if (!r.ok) { root.note = "✗ " + r.error; root.installingSuite = ""; root.busy = false; return }
        root.busy = true; root.jobKind = "sapp"; suitePoller.start()
    }
    function useVersion(name, label) {
        var r = JSON.parse(backend.app_use(name, label))
        note = r.ok ? ("now using " + name + "@" + label) : ("error: " + r.error)
        reload()
    }
    function pinVersion(name, label) {
        var r = JSON.parse(backend.app_desktop(name, label, false))
        note = r.ok ? ("pinned " + name + "@" + label + " → " + r.path) : ("error: " + r.error)
    }
    function removeVersion(name, label) {
        var r = JSON.parse(backend.app_remove(name, label))
        note = r.ok ? ("removed " + name + "@" + label) : ("error: " + r.error)
        reload()
    }
    function runApp(name) {
        var r = JSON.parse(backend.run_local(name, ""))
        note = r.ok ? ("▶ " + r.msg) : ("error: " + r.error)
    }

    Timer {
        id: suitePoller; interval: 350; repeat: true; running: false
        onTriggered: {
            var j = JSON.parse(backend.poll_job())
            if (!j.done) return
            suitePoller.stop()
            var res = (j.result && j.result.length) ? JSON.parse(j.result) : { ok: false, error: "no result" }
            var k = root.jobKind; root.jobKind = ""
            if (k === "mksuite") {
                root.busy = false
                if (res.ok) { root.note = "✓ created suite '" + res.name + "' with " + res.count + " apps"; root.loadSuites() }
                else root.note = "✗ " + res.error
            } else if (k === "sapp") {
                if (res.ok) root.installOk += 1
                root.note = "installing " + root.installingSuite + " — " + (root.installIdx + 1) + "/" + root.installTotal + (res.ok ? "" : (" (✗ " + res.name + ")"))
                root.installIdx += 1
                root.installNext()
            } else if (k === "zipapp") {
                root.busy = false
                if (res.ok) { root.note = (res.updated ? "✓ updated " : "✓ installed ") + res.name + " @" + res.label; root.reload(); root.loadDownloads() }
                else root.note = "✗ " + res.error
            } else if (k === "buildgroup") {
                root.busy = false
                if (res.ok) root.note = "✓ built here + installed " + res.installed + "/" + res.total + " on " + (res.host || "device")
                else root.note = "✗ " + (res.error || "remote install failed")
            }
        }
    }

    Component.onCompleted: { reload(); loadSuites(); loadDownloads(); loadGroups() }

    Column {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 8

        Row {
            spacing: 8
            Label { text: "apps"; color: "#cdd6f4"; font.pixelSize: 18; font.bold: true }
            Button { text: "⟳"; onClicked: root.reload() }
        }
        Label { text: root.note; color: "#8a93a6"; visible: root.note.length > 0; wrapMode: Text.WrapAnywhere; width: parent.width }

        // ── from Downloads: zips & bundles ready to install/update ──
        Rectangle {
            visible: root.downloads.length > 0
            width: parent.width; radius: 10; color: "#171c2b"; border.color: "#3a4566"
            height: dlCol.height + 20
            Column {
                id: dlCol
                x: 12; y: 10; width: parent.width - 24; spacing: 8
                Row {
                    width: parent.width; spacing: 8
                    Label { text: "from Downloads"; color: "#56d364"; font.bold: true; anchors.verticalCenter: parent.verticalCenter }
                    Item { width: parent.width - 200; height: 1 }
                    Button { text: "⟳"; enabled: !root.busy; onClicked: root.loadDownloads() }
                }
                Repeater {
                    model: root.downloads
                    delegate: Rectangle {
                        width: dlCol.width; radius: 8; color: "#0f1320"; border.color: "#2a3350"
                        height: 44
                        Row {
                            anchors.fill: parent; anchors.margins: 8; spacing: 8
                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                height: 16; radius: 8; width: kb.width + 12
                                color: modelData.kind === "bundle" ? "#2a2340" : "#1b2b3a"
                                Label { id: kb; anchors.centerIn: parent; text: modelData.kind === "bundle" ? "bundle" : "app"; color: modelData.kind === "bundle" ? "#c9a3ff" : "#7aa2f7"; font.pixelSize: 10 }
                            }
                            Column {
                                width: parent.width - 200
                                anchors.verticalCenter: parent.verticalCenter
                                Label { text: modelData.name + (modelData.kind === "bundle" && modelData.apps ? ("  ·  " + modelData.apps + " apps") : ""); color: "#cdd6f4"; font.pixelSize: 13; width: parent.width; elide: Text.ElideRight }
                                Label { text: modelData.path; color: "#5b6477"; font.pixelSize: 10; font.family: "monospace"; width: parent.width; elide: Text.ElideMiddle }
                            }
                            Button {
                                anchors.verticalCenter: parent.verticalCenter
                                enabled: !root.busy
                                text: modelData.kind === "bundle" ? "install all"
                                    : (modelData.installed ? "update" : "install")
                                onClicked: modelData.kind === "bundle" ? root.installBundle(modelData.path) : root.installZip(modelData.path)
                            }
                        }
                    }
                }
                Label { text: root.downloadsDir; color: "#5b6477"; font.pixelSize: 10; font.family: "monospace"; width: parent.width; elide: Text.ElideMiddle }
            }
        }

        // ── BigSilicon app store: curated groups, "build all in one go" ──
        Rectangle {
            width: parent.width; radius: 10; color: "#171c2b"; border.color: "#3a2f57"
            height: groupCol.height + 20
            Column {
                id: groupCol
                x: 12; y: 10; width: parent.width - 24; spacing: 8
                Row {
                    width: parent.width; spacing: 8
                    Label { text: "BigSilicon app store"; color: "#b4a0ff"; font.bold: true; anchors.verticalCenter: parent.verticalCenter }
                    Item { width: parent.width - 260; height: 1 }
                    Button { text: "⟳"; enabled: !root.busy; onClicked: root.loadGroups() }
                }
                Label {
                    width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap
                    text: "Curated groups share kits and build together. \"build all\" installs on this machine; set a device to build here and install on the phone."
                }
                TextField {
                    width: parent.width; placeholderText: "install onto device — user@host (e.g. user@192.168.2.223)"
                    text: root.groupDevice; onTextChanged: root.groupDevice = text
                    color: "#cdd6f4"; font.pixelSize: 12
                }
                Repeater {
                    model: root.groups
                    delegate: Rectangle {
                        width: groupCol.width; radius: 8; color: "#0f1320"
                        border.color: modelData.name === "bigsilicons-apps" ? "#b4a0ff" : "#3a2f57"
                        height: grpCard.height + 14
                        Column {
                            id: grpCard
                            x: 10; y: 7; width: parent.width - 20; spacing: 4
                            Row {
                                width: parent.width; spacing: 8
                                Column {
                                    width: parent.width - gBtn.width - devBtn.width - 20
                                    Label {
                                        text: (modelData.name === "bigsilicons-apps" ? "BigSilicon apps" : modelData.name) + "  ·  " + (modelData.count || 0) + " apps"
                                        color: modelData.name === "bigsilicons-apps" ? "#b4a0ff" : "#cdd6f4"; font.bold: true; font.pixelSize: 13
                                    }
                                    Label { text: modelData.description; color: "#8a93a6"; font.pixelSize: 11; width: parent.width; elide: Text.ElideRight }
                                }
                                Button {
                                    id: gBtn
                                    text: (root.installingSuite === modelData.name) ? (root.installIdx + "/" + root.installTotal) : "build all"
                                    enabled: !root.busy; anchors.verticalCenter: parent.verticalCenter
                                    onClicked: root.buildGroup(modelData)
                                }
                                Button {
                                    id: devBtn
                                    text: "→ device"
                                    enabled: !root.busy && root.groupDevice.indexOf("@") > 0
                                    anchors.verticalCenter: parent.verticalCenter
                                    onClicked: root.installGroupToDevice(modelData)
                                }
                            }
                            Label {
                                visible: root.installingSuite === modelData.name
                                width: parent.width; color: "#f9e2af"; font.pixelSize: 11
                                text: "building & installing… " + (root.installIdx + 1) + "/" + root.installTotal
                            }
                        }
                    }
                }
            }
        }

        // ── suites: install a whole bundle of apps at once ──
        Rectangle {
            width: parent.width; radius: 10; color: "#171c2b"; border.color: "#2a3350"
            height: suiteCol.height + 20
            Column {
                id: suiteCol
                x: 12; y: 10; width: parent.width - 24; spacing: 8
                Row {
                    width: parent.width; spacing: 8
                    Label { text: "suites"; color: "#7aa2f7"; font.bold: true; anchors.verticalCenter: parent.verticalCenter }
                    Item { width: parent.width - 260; height: 1 }
                    Button { text: root.jobKind === "mksuite" ? "scanning…" : "create \"all\""; enabled: !root.busy; onClicked: root.makeDefaultSuite() }
                }
                Label {
                    width: parent.width; color: "#5b6477"; font.pixelSize: 11; wrapMode: Text.WordWrap
                    text: "A suite installs many apps at once — each is built for this device and gets a launcher in your app grid (Plasma Mobile or desktop)."
                }
                Repeater {
                    model: root.suites
                    delegate: Rectangle {
                        width: suiteCol.width; radius: 8; color: "#0f1320"; border.color: "#3a4566"
                        height: sc.height + 14
                        Column {
                            id: sc
                            x: 10; y: 7; width: parent.width - 20; spacing: 4
                            Row {
                                width: parent.width; spacing: 8
                                Column {
                                    width: parent.width - instBtn.width - delBtn.width - 16
                                    Label { text: modelData.name + "  ·  " + (modelData.apps ? modelData.apps.length : 0) + " apps"; color: "#cdd6f4"; font.bold: true; font.pixelSize: 13 }
                                    Label { text: modelData.description; color: "#8a93a6"; font.pixelSize: 11; width: parent.width; elide: Text.ElideRight }
                                }
                                Button {
                                    id: instBtn
                                    text: (root.installingSuite === modelData.name) ? (root.installIdx + "/" + root.installTotal) : "install all"
                                    enabled: !root.busy; anchors.verticalCenter: parent.verticalCenter
                                    onClicked: root.installSuite(modelData)
                                }
                                Button { id: delBtn; text: "×"; enabled: !root.busy; anchors.verticalCenter: parent.verticalCenter; onClicked: root.deleteSuite(modelData.name) }
                            }
                            Label {
                                visible: root.installingSuite === modelData.name
                                width: parent.width; color: "#f9e2af"; font.pixelSize: 11
                                text: "building & installing… " + (root.installIdx + 1) + "/" + root.installTotal
                            }
                        }
                    }
                }
                Label {
                    visible: root.suites.length === 0
                    width: parent.width; color: "#5b6477"; font.pixelSize: 12
                    text: "No suites yet — tap \"create all\" to bundle every app on this machine with sane defaults."
                }
            }
        }

        ListView {
            id: appList
            width: parent.width
            height: parent.height - y
            spacing: 12
            clip: true
            model: root.apps

            delegate: Column {
                width: appList.width
                spacing: 4

                Row {
                    width: appList.width; spacing: 8
                    Label {
                        text: modelData.name + "  (current: " + (modelData.current ? modelData.current : "—") + ")"
                        color: "#cdd6f4"; font.bold: true
                        width: parent.width - runBtn.width - 8; elide: Text.ElideRight
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Button {
                        id: runBtn; text: "▶ run"
                        enabled: modelData.current !== null && modelData.current !== undefined && modelData.current !== ""
                        onClicked: root.runApp(modelData.name)
                    }
                }

                Repeater {
                    model: modelData.versions
                    delegate: Rectangle {
                        id: verRow
                        width: appList.width
                        height: 48
                        radius: 8
                        color: modelData.current ? "#2a3350" : "#171c2b"
                        border.color: modelData.current ? "#7aa2f7" : "#2a3350"

                        // Resolve the parent app name for this version row.
                        function appName() {
                            for (var i = 0; i < root.apps.length; i++) {
                                var a = root.apps[i]
                                for (var j = 0; j < a.versions.length; j++)
                                    if (a.versions[j].label === modelData.label
                                        && a.versions[j].commit === modelData.commit)
                                        return a.name
                            }
                            return ""
                        }

                        Row {
                            anchors.fill: parent
                            anchors.margins: 8
                            spacing: 10

                            Column {
                                width: parent.width - pinBtn.width - rmBtn.width - 28
                                Label { text: "@" + modelData.label; color: "#cdd6f4"; font.bold: true }
                                Label {
                                    text: modelData.commit.substring(0, 8) + "  ·  " + modelData.target
                                    color: "#8a93a6"; font.pixelSize: 12
                                }
                            }

                            Button {
                                id: pinBtn
                                text: "pin"
                                anchors.verticalCenter: parent.verticalCenter
                                onClicked: root.pinVersion(verRow.appName(), modelData.label)
                            }
                            Button {
                                id: rmBtn
                                text: "×"
                                anchors.verticalCenter: parent.verticalCenter
                                onClicked: root.removeVersion(verRow.appName(), modelData.label)
                            }
                        }

                        TapHandler {
                            onTapped: root.useVersion(verRow.appName(), modelData.label)
                        }
                    }
                }
            }

            Label {
                anchors.centerIn: parent
                visible: root.apps.length === 0
                text: "no pinned apps yet\n(gin app install / build)"
                horizontalAlignment: Text.AlignHCenter
                color: "#8a93a6"
            }
        }
    }
}
