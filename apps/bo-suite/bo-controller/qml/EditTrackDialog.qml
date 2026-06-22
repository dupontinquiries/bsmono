// qml/EditTrackDialog.qml
//
// A bottom-sheet style dialog for editing a track's manual override fields.
// Lives outside the ListView so it isn't recycled, and is shown by parent
// code via `open(track)` where `track` is the modelData object from the
// library list. Saves via qtobj.set_track_override(id, json).
//
// Empty field → no override (parser's value shows through). Tap "Clear"
// to remove all overrides for the track at once.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    anchors.fill: parent
    visible: opacity > 0
    opacity: 0
    Behavior on opacity { NumberAnimation { duration: Bo.quick } }
    z: 200    // float above the library list

    property var track: null
    property color accent: "#C8A04B"

    function open(t) {
        track = t
        // Pre-fill with whatever the row is currently showing (overrides
        // already overlaid by the catalog, so this is the user's view).
        titleField.text  = (t && t.title)          ? t.title          : ""
        artistField.text = (t && t.primary_artist) ? t.primary_artist : ""
        albumField.text  = (t && t.album)          ? t.album          : ""
        yearField.text   = (t && t.year)           ? String(t.year)   : ""
        opacity = 1
    }
    function close() { opacity = 0; track = null }

    // Scrim — tap outside the sheet to dismiss.
    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.55
        MouseArea { anchors.fill: parent; onClicked: root.close() }
    }

    // The sheet itself.
    Rectangle {
        id: sheet
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        height: Math.min(parent.height * 0.78, sheetCol.implicitHeight + Bo.gapLG * 2)
        radius: 18
        color: Bo.surface
        border.color: Bo.hairline
        border.width: 1

        // Eat clicks so the scrim's MouseArea doesn't close while editing.
        MouseArea { anchors.fill: parent; onClicked: {} }

        Column {
            id: sheetCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Bo.gapLG
            spacing: Bo.gapMD

            // Grab handle
            Rectangle {
                width: 36; height: 4; radius: 2
                anchors.horizontalCenter: parent.horizontalCenter
                color: Bo.bronzeDim
            }

            Text {
                text: "EDIT TRACK"
                color: Bo.silver
                font.pixelSize: Bo.microSize
                font.letterSpacing: Bo.labelTracking
                font.weight: Font.DemiBold
            }

            // File path read-only reference
            Text {
                width: parent.width
                visible: root.track && root.track.path
                text: root.track ? (root.track.path || "") : ""
                color: Bo.slate
                font.pixelSize: 10
                font.family: "monospace"
                elide: Text.ElideMiddle
            }

            EditField { id: titleField;  label: "TITLE";   accent: root.accent }
            EditField { id: artistField; label: "ARTIST";  accent: root.accent }
            EditField { id: albumField;  label: "ALBUM";   accent: root.accent }
            EditField {
                id: yearField; label: "YEAR"; accent: root.accent
                inputMethodHints: Qt.ImhDigitsOnly
                maximumLength: 4
            }

            // Action row
            Row {
                spacing: Bo.gapSM
                anchors.right: parent.right
                ActionBtn {
                    label: "CLEAR"; accent: root.accent; subtle: true
                    onTapped: {
                        // All-empty body → server removes the override row.
                        if (root.track && root.track.id !== undefined)
                            qtobj.set_track_override(root.track.id, "{}")
                        root.close()
                    }
                }
                ActionBtn {
                    label: "CANCEL"; accent: root.accent; subtle: true
                    onTapped: root.close()
                }
                ActionBtn {
                    label: "SAVE"; accent: root.accent
                    onTapped: {
                        if (root.track && root.track.id !== undefined) {
                            var payload = {}
                            if (titleField.text.trim().length  > 0) payload.title  = titleField.text.trim()
                            if (artistField.text.trim().length > 0) payload.artist = artistField.text.trim()
                            if (albumField.text.trim().length  > 0) payload.album  = albumField.text.trim()
                            var y = parseInt(yearField.text)
                            if (!isNaN(y) && y > 0 && y < 4000) payload.year = y
                            qtobj.set_track_override(root.track.id, JSON.stringify(payload))
                        }
                        root.close()
                    }
                }
            }
        }
    }

    // ── Local components ───────────────────────────────────────────
    component EditField: Column {
        property alias text: input.text
        property alias inputMethodHints: input.inputMethodHints
        property alias maximumLength: input.maximumLength
        property string label: ""
        property color accent: "#C8A04B"
        width: parent.width
        spacing: 4
        Text {
            text: parent.label
            color: Bo.silver
            font.pixelSize: 9
            font.letterSpacing: Bo.labelTracking
            font.weight: Font.DemiBold
        }
        Rectangle {
            width: parent.width
            height: 38
            radius: 8
            color: Bo.ink
            border.color: input.activeFocus ? parent.accent : Bo.hairline
            border.width: 1
            Behavior on border.color { ColorAnimation { duration: Bo.quick } }
            TextInput {
                id: input
                anchors.fill: parent
                anchors.leftMargin: 10
                anchors.rightMargin: 10
                verticalAlignment: TextInput.AlignVCenter
                color: Bo.bone
                font.pixelSize: Bo.bodySize
                selectByMouse: true
                clip: true
            }
        }
    }

    component ActionBtn: Rectangle {
        property string label: ""
        property color accent: "#C8A04B"
        property bool subtle: false
        signal tapped()
        width: btnText.implicitWidth + 32
        height: 36
        radius: 18
        color: subtle ? (abma.pressed ? Bo.surfaceHi : "transparent")
                      : (abma.pressed ? Bo.shade(accent) : accent)
        border.color: subtle ? Bo.hairline : "transparent"
        border.width: 1
        Behavior on color { ColorAnimation { duration: Bo.quick } }
        Text {
            id: btnText
            anchors.centerIn: parent
            text: parent.label
            color: parent.subtle ? Bo.bone : Bo.graphite
            font.pixelSize: Bo.captionSize
            font.weight: Font.DemiBold
            font.letterSpacing: 1
        }
        MouseArea { id: abma; anchors.fill: parent; onClicked: parent.tapped() }
    }
}
