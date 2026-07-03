import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

// A lightweight "add this track to a playlist" picker. Opened with a
// track object; lists the user's playlists and adds the track to whichever
// is tapped (with a brief confirmation), then closes. Also offers a quick
// "New playlist…" affordance that creates one and adds the track to it.
//
// Lives at window level (window.addToPlaylist) so any track row can reach
// it. Mirrors the EditTrackDialog / TagEditor pattern.

Rectangle {
    id: root
    anchors.fill: parent
    color: "#000000AA"
    opacity: 0
    visible: opacity > 0
    z: 340

    property var track: null
    property color accent: "#C8A04B"
    property string justAdded: ""

    Behavior on opacity { NumberAnimation { duration: Bo.quick } }

    function open(t) {
        track = t
        justAdded = ""
        qtobj.load_playlists()           // refresh the list
        opacity = 1
    }
    function close() { opacity = 0; track = null }

    function playlists() {
        try { return JSON.parse(qtobj.playlists_json || "[]") }
        catch (e) { return [] }
    }

    // Tap-outside closes.
    MouseArea { anchors.fill: parent; onClicked: root.close() }

    Rectangle {
        width: Math.min(parent.width - Bo.gapLG * 2, 360)
        height: Math.min(parent.height - Bo.gapLG * 2, 440)
        anchors.centerIn: parent
        radius: Bo.cardRadius
        color: Bo.surface
        border.color: Bo.hairline; border.width: 1
        // Eat clicks so they don't fall through to the dismiss overlay.
        MouseArea { anchors.fill: parent; onClicked: {} }

        Column {
            anchors.fill: parent
            anchors.margins: Bo.gapMD
            spacing: Bo.gapSM

            Text {
                width: parent.width
                text: "Add to playlist"
                color: Bo.bone
                font.pixelSize: Bo.titleSize
                font.weight: Font.DemiBold
            }
            Text {
                width: parent.width
                visible: root.track !== null
                text: root.track ? (root.track.title || "") : ""
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                elide: Text.ElideRight
            }

            // Confirmation line after an add.
            Text {
                width: parent.width
                visible: root.justAdded.length > 0
                text: "✓ Added to " + root.justAdded
                color: root.accent
                font.pixelSize: Bo.captionSize
            }

            ListView {
                width: parent.width
                height: parent.height - 96
                clip: true
                spacing: 2
                model: root.playlists()
                flickDeceleration: Bo.flickDecel
                maximumFlickVelocity: Bo.flickMaxVel
                boundsBehavior: Flickable.DragOverBounds
            pressDelay: Bo.pressDelay
                rebound: BoRebound {}
                ScrollIndicator.vertical: ScrollIndicator {}

                delegate: Rectangle {
                    width: ListView.view.width
                    height: 44
                    radius: 6
                    color: plMa.pressed ? Bo.surfaceHi : "transparent"
                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: Bo.gapSM
                        anchors.rightMargin: Bo.gapSM
                        spacing: Bo.gapSM
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - 60
                            spacing: 1
                            Text {
                                text: modelData.name || "(untitled)"
                                color: Bo.bone
                                font.pixelSize: Bo.bodySize
                                elide: Text.ElideRight
                                width: parent.width
                            }
                            Text {
                                text: (modelData.track_count === 1)
                                    ? "1 track" : (modelData.track_count + " tracks")
                                color: Bo.slate
                                font.pixelSize: 9
                            }
                        }
                        Text {
                            text: "+"
                            color: root.accent
                            font.pixelSize: 18
                            font.weight: Font.Bold
                            anchors.verticalCenter: parent.verticalCenter
                        }
                    }
                    MouseArea {
                        id: plMa
                        anchors.fill: parent
                        onClicked: {
                            if (root.track && root.track.id !== undefined) {
                                qtobj.add_track_to_playlist(modelData.id, root.track.id)
                                root.justAdded = modelData.name || ""
                            }
                        }
                    }
                }
            }

            // Close button.
            Rectangle {
                width: parent.width
                height: 32; radius: 16
                color: closeMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "Done"
                    color: Bo.silver
                    font.pixelSize: Bo.captionSize
                }
                MouseArea {
                    id: closeMa
                    anchors.fill: parent
                    onClicked: root.close()
                }
            }
        }
    }
}
