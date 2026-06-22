// qml/TrackRow.qml
//
// One row in the library's virtualized ListView. Kept deliberately light
// (no heavy effects, no per-row timers) so a ListView of thousands recycles
// smoothly. Shows title + artist, with an optional BPM/key chip on the
// right when analysis data is present.

import QtQuick 2.15
import "."

Rectangle {
    id: root
    property string title: ""
    property string artist: ""
    property string album: ""
    property var bpm: undefined
    property string camelot: ""
    property color accent: "#C8A04B"
    signal activated()
    signal longPressed()
    signal tagRequested()    // emitted when the # button is tapped

    height: 60
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
        anchors.right: tagBtn.left
        anchors.rightMargin: Bo.gapSM
        anchors.verticalCenter: parent.verticalCenter
        spacing: Bo.gapSM

        // play glyph
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: "♪"
            color: Bo.bronze
            font.pixelSize: 15
        }
        Column {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 24
            spacing: 2
            Text {
                width: parent.width
                text: root.title.length > 0 ? root.title : "Unknown"
                color: Bo.bone
                font.pixelSize: Bo.bodySize
                font.weight: Font.Medium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: root.artist + (root.album.length > 0 ? "  ·  " + root.album : "")
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                elide: Text.ElideRight
            }
        }
    }

    // # tag button — small chip on the right, before the BPM chip.
    // Always visible (unlike the BPM chip which only shows when
    // analyzed) so users have a consistent target for tag management.
    // Stops propagation so the tap doesn't also activate the row.
    Rectangle {
        id: tagBtn
        anchors.right: chip.left
        anchors.rightMargin: Bo.gapSM
        anchors.verticalCenter: parent.verticalCenter
        width: 26; height: 24; radius: 12
        color: tagMa.pressed ? Bo.surfaceHi : "transparent"
        border.color: Bo.hairline; border.width: 1
        Text {
            anchors.centerIn: parent
            text: "#"
            color: Bo.silver
            font.pixelSize: 12
            font.weight: Font.Bold
        }
        MouseArea {
            id: tagMa
            anchors.fill: parent
            onClicked: root.tagRequested()
        }
    }

    // BPM / key chip (only when analyzed)
    Rectangle {
        id: chip
        anchors.right: parent.right
        anchors.rightMargin: Bo.gapMD
        anchors.verticalCenter: parent.verticalCenter
        visible: root.bpm !== undefined && root.bpm !== null && root.bpm > 0
        // When invisible, take zero width so the tag button slides to
        // the right edge cleanly instead of leaving a hole.
        width: visible ? (chipRow.implicitWidth + 16) : 0
        height: 24
        radius: 12
        color: Bo.mix(Bo.surface, root.accent, 0.10)
        border.color: Bo.hairline
        border.width: 1
        Row {
            id: chipRow
            anchors.centerIn: parent
            spacing: 6
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: root.bpm ? Math.round(root.bpm) + " BPM" : ""
                color: Bo.silver
                font.pixelSize: 10
                font.weight: Font.DemiBold
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                visible: root.camelot.length > 0 && root.camelot !== "?"
                text: root.camelot
                color: root.accent
                font.pixelSize: 10
                font.weight: Font.DemiBold
            }
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        onClicked: root.activated()
        onPressAndHold: root.longPressed()
    }
}
