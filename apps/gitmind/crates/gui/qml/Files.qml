// Repo file browser + viewer in one tab. Walks the working tree
// (backend.list_dir) and reads files through the peek engine
// (backend.peek_file): tap a folder to descend, tap a file to read it, "‹ back"
// returns to the listing. The status panel's peek action calls openPath() too.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root
    property string curPath: ""        // directory being browsed (repo-relative)
    property var entries: []           // [{ kind: "up"|"dir"|"file", name }]
    property string selectedFile: ""   // "" = browsing; otherwise viewing this file

    // Open a specific file directly (used by the status panel's peek action).
    function openPath(p) {
        selectedFile = p
        specField.text = ""
        loadFile()
    }
    function loadFile() {
        if (selectedFile === "") return
        var r = JSON.parse(backend.peek_file(selectedFile, specField.text, 2))
        body.text = r.ok ? r.text : ("error: " + r.error)
    }
    function loadDir(path) {
        var r = JSON.parse(backend.list_dir(path))
        if (!r.ok) { root.entries = []; return }
        root.curPath = r.path
        var e = []
        if (r.path.length > 0) e.push({ kind: "up", name: ".." })
        for (var i = 0; i < r.dirs.length; i++) e.push({ kind: "dir", name: r.dirs[i] })
        for (var j = 0; j < r.files.length; j++) e.push({ kind: "file", name: r.files[j] })
        root.entries = e
    }
    function parentOf(p) { var i = p.lastIndexOf("/"); return i >= 0 ? p.substring(0, i) : "" }
    function join(p, n) { return p.length > 0 ? (p + "/" + n) : n }
    function tap(kind, name) {
        if (kind === "up") loadDir(parentOf(curPath))
        else if (kind === "dir") loadDir(join(curPath, name))
        else openPath(join(curPath, name))
    }
    Component.onCompleted: loadDir("")

    // ── browse ───────────────────────────────────────────────────────────
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6
        visible: root.selectedFile === ""

        RowLayout {
            Layout.fillWidth: true
            Label {
                Layout.fillWidth: true
                text: root.curPath.length ? ("/" + root.curPath) : "/ · repo root"
                color: "#cdd6f4"; font.bold: true; elide: Text.ElideMiddle
            }
            ToolButton { text: "\u21bb"; onClicked: root.loadDir(root.curPath) }
        }
        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: 2
            model: root.entries
            delegate: Rectangle {
                width: list.width
                height: 40
                radius: 6
                color: "#171c2b"
                border.color: "#2a3350"
                Row {
                    anchors.verticalCenter: parent.verticalCenter
                    x: 12; spacing: 10
                    Label {
                        width: 16
                        text: modelData.kind === "up" ? "\u2039"
                            : modelData.kind === "dir" ? "\u25b8" : "\u00b7"
                        color: modelData.kind === "file" ? "#8a93a6" : "#7aa2f7"
                        font.pixelSize: 16
                    }
                    Label {
                        text: modelData.name
                        color: "#cdd6f4"
                        font.bold: modelData.kind === "dir"
                    }
                }
                TapHandler { onTapped: root.tap(modelData.kind, modelData.name) }
            }
        }
    }

    // ── view ─────────────────────────────────────────────────────────────
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6
        visible: root.selectedFile !== ""

        RowLayout {
            Layout.fillWidth: true
            ToolButton { text: "\u2039 back"; onClicked: root.selectedFile = "" }
            Label {
                Layout.fillWidth: true
                text: root.selectedFile
                color: "#cdd6f4"; font.bold: true
                elide: Text.ElideMiddle
            }
            TextField {
                id: specField
                Layout.preferredWidth: 90
                placeholderText: "5xx / @fn"
                onAccepted: root.loadFile()
            }
        }
        Flickable {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: body.paintedWidth
            contentHeight: body.paintedHeight
            clip: true
            TextArea {
                id: body
                readOnly: true
                wrapMode: TextArea.NoWrap
                font.family: "monospace"
                font.pixelSize: 12
                color: "#e6e9ef"
                background: Rectangle { color: "#0c0f1a" }
                text: ""
            }
        }
    }
}
