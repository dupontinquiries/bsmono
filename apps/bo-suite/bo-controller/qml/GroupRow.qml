// qml/GroupRow.qml
//
// A row in the Artists or Tags list view. Shows name + track count with a
// gentle bar reflecting relative size. Same height/feel as TrackRow so the
// Library list looks consistent across tabs.

import QtQuick 2.15
import "."

Rectangle {
    id: root
    property string name: ""
    property int count: 0
    property int maxCount: 1
    property string kind: "artist"   // "artist" or "tag"
    property color accent: "#C8A04B"
    signal activated()

    height: 56
    color: mouse.pressed ? Bo.surfaceHi : "transparent"
    Behavior on color { ColorAnimation { duration: Bo.instant } }

    // Hairline separator
    Rectangle {
        anchors.bottom: parent.bottom
        anchors.left: parent.left; anchors.right: parent.right
        anchors.leftMargin: Bo.gapMD; anchors.rightMargin: Bo.gapMD
        height: 1; color: Bo.hairline; opacity: 0.5
    }

    Row {
        anchors.left: parent.left
        anchors.leftMargin: Bo.gapMD
        anchors.right: parent.right
        anchors.rightMargin: Bo.gapMD
        anchors.verticalCenter: parent.verticalCenter
        spacing: Bo.gapSM

        // Type glyph
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: root.kind === "tag" ? "#" : "♪"
            color: root.accent
            font.pixelSize: 16
            font.weight: Font.Medium
        }

        Column {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 24 - barWrap.width - Bo.gapSM
            spacing: 3
            Text {
                width: parent.width
                text: root.name
                color: Bo.bone
                font.pixelSize: Bo.bodySize
                font.weight: Font.Medium
                elide: Text.ElideRight
            }
            // Relative-size bar — a quiet visual hint of how big this group is.
            Item {
                id: barWrapInner
                width: parent.width; height: 3
                Rectangle {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    height: 2; radius: 1
                    width: Math.max(2, parent.width * Math.min(1, root.count / Math.max(1, root.maxCount)))
                    color: Bo.bronzeDim
                    Behavior on width { NumberAnimation { duration: Bo.slow; easing.type: Easing.OutCubic } }
                }
            }
        }

        // Count chip
        Rectangle {
            id: barWrap
            anchors.verticalCenter: parent.verticalCenter
            width: countText.implicitWidth + 16
            height: 22
            radius: 11
            color: Bo.mix(Bo.surface, root.accent, 0.08)
            border.color: Bo.hairline; border.width: 1
            Text {
                id: countText
                anchors.centerIn: parent
                text: root.count
                color: Bo.silver
                font.pixelSize: 10
                font.weight: Font.DemiBold
            }
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        onClicked: root.activated()
    }
}
