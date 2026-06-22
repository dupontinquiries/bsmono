// Checkpoint timeline. Renders backend.timeline_json(); tap a node to see
// what that snapshot changed (backend.checkpoint_diff_json).
import QtQuick
import QtQuick.Controls

Item {
    id: tl
    function reload() { model = JSON.parse(backend.timeline_json(50)).items || [] }
    function showDiff(oid) {
        var r = JSON.parse(backend.checkpoint_diff_json(oid, ""))
        if (!r.ok) { diffText = "error: " + r.error; return }
        if (!r.items || r.items.length === 0) { diffText = oid + " — no changes vs working tree"; return }
        var s = oid + " → working tree\n"
        for (var i = 0; i < r.items.length; i++)
            s += "  +" + r.items[i].added + "  -" + r.items[i].removed + "  " + r.items[i].path + "\n"
        diffText = s
    }
    property var model: []
    property string diffText: ""
    Component.onCompleted: reload()

    Label {
        id: title; text: "timeline"; color: "#e6e9ef"; font.bold: true
        font.pixelSize: 16; x: 16; y: 12
    }
    ListView {
        id: list
        anchors.top: title.bottom; anchors.topMargin: 12
        anchors.left: parent.left; anchors.right: parent.right
        anchors.bottom: diffPane.top
        anchors.margins: 12
        spacing: 6
        clip: true
        model: tl.model
        delegate: Rectangle {
            width: ListView.view.width; height: 48; radius: 8; color: "#171c2b"
            Row {
                anchors.fill: parent; anchors.leftMargin: 12; spacing: 12
                Rectangle { width: 10; height: 10; radius: 5; color: "#5ec8d8"
                            anchors.verticalCenter: parent.verticalCenter }
                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    Text { text: modelData.label; color: "#e6e9ef"; font.pixelSize: 13 }
                    Text { text: modelData.oid; color: "#8a93a6"; font.pixelSize: 10; font.family: "monospace" }
                }
            }
            TapHandler { onTapped: tl.showDiff(modelData.oid) }
        }
        Label {
            anchors.centerIn: parent; visible: tl.model.length === 0
            text: "no checkpoints yet"; color: "#8a93a6"
        }
    }

    // Diff panel — fills with the tapped checkpoint's changes.
    Rectangle {
        id: diffPane
        anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
        height: tl.diffText === "" ? 0 : parent.height * 0.4
        color: "#0c0f1a"
        visible: height > 0
        Flickable {
            anchors.fill: parent; anchors.margins: 10; clip: true
            contentWidth: diffBody.paintedWidth; contentHeight: diffBody.paintedHeight
            TextArea {
                id: diffBody; readOnly: true; wrapMode: TextArea.NoWrap
                font.family: "monospace"; font.pixelSize: 11; color: "#e6e9ef"
                background: null; text: tl.diffText
            }
        }
    }
}
