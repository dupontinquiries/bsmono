// qml/SourceTile.qml
//
// A selectable source tile for the Sources page. Picks a glyph by source
// type, highlights when active, and springs on press — consistent with
// the StepKey interaction language.

import QtQuick 2.15
import "."

Rectangle {
    id: root
    property string sourceName: ""
    property string sourceType: ""
    property bool active: false
    property color accent: "#C8A04B"
    signal chosen()

    width: 150
    height: 60
    radius: Bo.cardRadius
    color: active ? accent : (ma.pressed ? Bo.surfaceHi : Bo.surface)
    border.color: active ? accent : Bo.hairline
    border.width: 1
    Behavior on color { ColorAnimation { duration: Bo.quick } }

    scale: ma.pressed ? 0.95 : 1.0
    Behavior on scale { NumberAnimation { duration: Bo.instant
        easing.type: Easing.Bezier; easing.bezierCurve: Bo.spring } }

    function glyphFor(t) {
        var s = (t || "").toLowerCase()
        if (s.indexOf("line") !== -1 || s.indexOf("aux") !== -1) return "▦"
        if (s.indexOf("blue") !== -1) return "✦"
        if (s.indexOf("spotify") !== -1 || s.indexOf("music") !== -1) return "♪"
        if (s.indexOf("radio") !== -1 || s.indexOf("tunein") !== -1) return "📻"
        if (s.indexOf("tv") !== -1 || s.indexOf("hdmi") !== -1) return "▭"
        if (s.indexOf("airplay") !== -1) return "◬"
        if (s.indexOf("dlna") !== -1 || s.indexOf("net") !== -1) return "≋"
        return "♫"
    }

    Row {
        anchors.left: parent.left
        anchors.leftMargin: Bo.gapMD
        anchors.right: parent.right
        anchors.rightMargin: Bo.gapSM
        anchors.verticalCenter: parent.verticalCenter
        spacing: Bo.gapSM

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: root.glyphFor(root.sourceType)
            color: root.active ? Bo.graphite : root.accent
            font.pixelSize: 18
        }
        Column {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 30
            spacing: 1
            Text {
                width: parent.width
                text: root.sourceName
                color: root.active ? Bo.graphite : Bo.bone
                font.pixelSize: Bo.bodySize
                font.weight: root.active ? Font.DemiBold : Font.Normal
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                visible: root.sourceType.length > 0
                text: root.sourceType
                color: root.active ? Qt.rgba(0,0,0,0.5) : Bo.slate
                font.pixelSize: 9
                font.letterSpacing: 1
                elide: Text.ElideRight
            }
        }
    }

    MouseArea { id: ma; anchors.fill: parent; onClicked: root.chosen() }
}
