// qml/InfoRow.qml
//
// A labeled key/value row for the device info card. The value right-
// aligns and elides; the key is a quiet uppercase micro-label.

import QtQuick 2.15
import "."

Item {
    id: root
    property string k: ""
    property string v: ""
    width: parent ? parent.width : 0
    height: 24

    Text {
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: root.k
        color: Bo.silver
        font.pixelSize: Bo.captionSize
        font.letterSpacing: 1
    }
    Text {
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.horizontalCenter
        horizontalAlignment: Text.AlignRight
        text: root.v
        color: Bo.bone
        font.pixelSize: Bo.captionSize
        elide: Text.ElideMiddle
    }
}
