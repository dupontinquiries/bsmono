// File viewer backed by the peek engine. Enter `spec` like 5xx, 5k, @symbol,
// or A-B; backend.peek_file returns the indexed slice.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: pv
    property string path: ""
    function open(p) { path = p; specField.text = ""; reload() }
    function reload() {
        if (path === "") return
        var r = JSON.parse(backend.peek_file(path, specField.text, 2))
        body.text = r.ok ? r.text : ("error: " + r.error)
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 10
        spacing: 8
        RowLayout {
            Layout.fillWidth: true
            TextField {
                id: pathField; Layout.fillWidth: true
                placeholderText: "path"; text: pv.path
                onAccepted: { pv.path = text; pv.reload() }
            }
            TextField {
                id: specField; Layout.preferredWidth: 90
                placeholderText: "5xx / @fn"
                onAccepted: pv.reload()
            }
            Button { text: "view"; onClicked: { pv.path = pathField.text; pv.reload() } }
        }
        Flickable {
            Layout.fillWidth: true; Layout.fillHeight: true
            contentWidth: body.paintedWidth; contentHeight: body.paintedHeight
            clip: true
            TextArea {
                id: body; readOnly: true; wrapMode: TextArea.NoWrap
                font.family: "monospace"; font.pixelSize: 12; color: "#e6e9ef"
                background: Rectangle { color: "#0c0f1a" }
                text: "select a file, or type a path above"
            }
        }
    }
}
