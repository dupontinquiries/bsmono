// qml/PlaylistDetailPage.qml
//
// Shows one playlist's contents. Reads
//   qtobj.current_playlist_id, qtobj.current_playlist_tracks_json
// and renders tracks in playlist order. Each row supports:
//   * tap → play immediately (PlayTrack)
//   * "+ queue" button → push to the queue without interrupting playback
//   * trash button → remove from playlist
//
// A header card shows the playlist name + description (editable in
// place when the user taps "Edit"). Re-ordering is via the ≡ drag
// handle on each row (press and slide; the list rearranges live and
// commits on release), with up/down arrows kept as a single-step
// fallback.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property int playlistId: -1
    property string playlistName: ""
    property color accent: "#C8A04B"
    signal back()

    onPlaylistIdChanged: if (playlistId >= 0) qtobj.load_playlist_tracks(playlistId)

    property var trackList: {
        try { return JSON.parse(qtobj.current_playlist_tracks_json || "[]") }
        catch (e) { return [] }
    }

    Connections {
        target: qtobj
        function onCurrent_playlist_changed() {
            try {
                if (qtobj.current_playlist_id === root.playlistId) {
                    root.trackList = JSON.parse(qtobj.current_playlist_tracks_json || "[]")
                }
            } catch (e) { root.trackList = [] }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapSM

        // ── Header bar with back + title ──
        Row {
            width: parent.width
            spacing: Bo.gapSM
            // Back affordance
            Rectangle {
                width: 32; height: 32; radius: 16
                color: backMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "‹"; color: Bo.bone
                    font.pixelSize: 18; font.weight: Font.DemiBold
                }
                MouseArea { id: backMa; anchors.fill: parent; onClicked: root.back() }
                anchors.verticalCenter: parent.verticalCenter
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    text: root.playlistName.toUpperCase()
                    color: Bo.bone
                    font.pixelSize: Bo.bodySize
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.0
                }
                Text {
                    text: root.trackList.length + " tracks"
                    color: Bo.silver
                    font.pixelSize: 10
                }
            }
        }

        // Play-all + clear + delete buttons
        Row {
            spacing: Bo.gapSM
            width: parent.width

            Rectangle {
                width: 110; height: 32; radius: 16
                color: root.trackList.length > 0 ? root.accent
                     : Bo.mix(Bo.surface, root.accent, 0.30)
                Text {
                    anchors.centerIn: parent
                    text: "▶ Play all"; color: Bo.graphite
                    font.pixelSize: Bo.captionSize
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    anchors.fill: parent
                    enabled: root.trackList.length > 0
                    onClicked: {
                        // Clear current queue, push everything, then start
                        // playing the first track.
                        qtobj.queue_clear()
                        for (var i = 0; i < root.trackList.length; ++i) {
                            qtobj.queue_push(root.trackList[i].id)
                        }
                        // Play the first track immediately.
                        qtobj.play_track(root.trackList[0].id)
                    }
                }
            }

            Rectangle {
                width: 90; height: 32; radius: 16
                color: queueAllMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "+ Queue"; color: Bo.bone
                    font.pixelSize: Bo.captionSize
                }
                MouseArea {
                    id: queueAllMa
                    anchors.fill: parent
                    enabled: root.trackList.length > 0
                    onClicked: {
                        for (var i = 0; i < root.trackList.length; ++i) {
                            qtobj.queue_push(root.trackList[i].id)
                        }
                    }
                }
            }

            // Spacer
            Item { width: parent.width - 110 - 90 - 32 - Bo.gapSM*3; height: 1 }

            Rectangle {
                width: 32; height: 32; radius: 16
                color: deleteMa.pressed ? "#5C2018" : "transparent"
                border.color: "#8E4A3D"; border.width: 1
                Text { anchors.centerIn: parent; text: "🗑"; font.pixelSize: 12 }
                MouseArea {
                    id: deleteMa
                    anchors.fill: parent
                    onClicked: {
                        qtobj.delete_playlist(root.playlistId)
                        root.back()
                    }
                }
            }
        }

        // ── Track list ──
        ListView {
            id: trackListView
            width: parent.width
            height: parent.height - 80
            spacing: 4
            clip: true
            model: root.trackList
            cacheBuffer: 240
            ScrollIndicator.vertical: ScrollIndicator {}

            // Ease repositioning when rows shift during a drag-reorder.
            displaced: Transition {
                NumberAnimation { properties: "y"; duration: 120
                    easing.type: Easing.OutCubic }
            }

            Text {
                anchors.centerIn: parent
                visible: parent.count === 0
                text: "Empty playlist — add tracks from the library"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
            }

            delegate: Rectangle {
                id: trackDelegate
                width: ListView.view.width
                height: 48
                radius: 6
                // While dragging, lift the row visually (raised tint + on
                // top of its neighbours) so it reads as "picked up".
                color: dragHandleMa.active ? Bo.surfaceHi
                     : trackMa.pressed ? Bo.surfaceHi : "transparent"
                z: dragHandleMa.active ? 2 : 0
                border.color: dragHandleMa.active ? Bo.accent : "transparent"
                border.width: dragHandleMa.active ? 1 : 0

                Row {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    spacing: Bo.gapSM

                    // Drag handle — press and slide to reorder. Lives at the
                    // left so the whole row stays tappable for play.
                    Rectangle {
                        width: 24; height: parent.height; radius: 6
                        color: "transparent"
                        anchors.verticalCenter: parent.verticalCenter
                        Text {
                            anchors.centerIn: parent
                            text: "≡"
                            color: dragHandleMa.active ? Bo.accent : Bo.slate
                            font.pixelSize: 14
                        }
                        MouseArea {
                            id: dragHandleMa
                            anchors.fill: parent
                            cursorShape: Qt.SizeVerCursor
                            preventStealing: true

                            property int fromIndex: -1
                            property bool dragging: false
                            // Expose a flag the delegate uses for its lifted
                            // appearance (replaces the Drag.active checks).
                            readonly property alias active: dragHandleMa.dragging

                            onPressed: {
                                fromIndex = index
                                dragging = true
                            }

                            onPositionChanged: function(mouse) {
                                if (!dragging) return
                                // Map the cursor into the ListView's content
                                // space and derive the slot under it. Using
                                // the pointer (not the delegate's y, which the
                                // ListView controls) keeps this stable. Pitch
                                // includes the ListView spacing between rows.
                                var p = mapToItem(trackListView.contentItem, mouse.x, mouse.y)
                                var pitch = trackDelegate.height + trackListView.spacing
                                var target = Math.floor(p.y / pitch)
                                target = Math.max(0, Math.min(trackList.length - 1, target))
                                if (target !== index) {
                                    var arr = root.trackList.slice()
                                    var moved = arr.splice(index, 1)[0]
                                    arr.splice(target, 0, moved)
                                    root.trackList = arr
                                }
                            }

                            onReleased: {
                                dragging = false
                                if (fromIndex >= 0 && fromIndex !== index) {
                                    // Persist the final position (absolute ord).
                                    qtobj.reorder_playlist(
                                        root.playlistId, modelData.id, index)
                                }
                                fromIndex = -1
                            }
                            onCanceled: { dragging = false; fromIndex = -1 }
                        }
                    }

                    // Ord + title/artist
                    Text {
                        text: (index + 1).toString().padStart(2, "0")
                        color: Bo.silver
                        font.pixelSize: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: 20
                    }
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 1
                        width: parent.width - 24 - 20 - 96
                        Text {
                            text: modelData.title || "(untitled)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Row {
                            spacing: Bo.gapSM
                            width: parent.width
                            Text {
                                visible: modelData.primary_artist
                                      && modelData.primary_artist.length > 0
                                text: modelData.primary_artist || ""
                                color: Bo.silver
                                font.pixelSize: 10
                                elide: Text.ElideRight
                                width: Math.min(implicitWidth, parent.width - 80)
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            StarRating {
                                trackId: modelData.id
                                rating: modelData.rating || 0
                                starSize: 11
                                anchors.verticalCenter: parent.verticalCenter
                            }
                        }
                    }
                    // Reorder + tag + remove
                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        Rectangle {
                            width: 24; height: 24; radius: 12
                            color: upMa.pressed ? Bo.surfaceHi : "transparent"
                            visible: index > 0
                            Text { anchors.centerIn: parent; text: "↑"
                                color: Bo.silver; font.pixelSize: 11 }
                            MouseArea {
                                id: upMa
                                anchors.fill: parent
                                onClicked: qtobj.reorder_playlist(
                                    root.playlistId, modelData.id, index - 1)
                            }
                        }
                        Rectangle {
                            width: 24; height: 24; radius: 12
                            color: downMa.pressed ? Bo.surfaceHi : "transparent"
                            visible: index < root.trackList.length - 1
                            Text { anchors.centerIn: parent; text: "↓"
                                color: Bo.silver; font.pixelSize: 11 }
                            MouseArea {
                                id: downMa
                                anchors.fill: parent
                                onClicked: qtobj.reorder_playlist(
                                    root.playlistId, modelData.id, index + 1)
                            }
                        }
                        Rectangle {
                            width: 24; height: 24; radius: 12
                            color: tagMa.pressed ? Bo.surfaceHi : "transparent"
                            Text { anchors.centerIn: parent; text: "#"
                                color: Bo.silver; font.pixelSize: 12
                                font.weight: Font.Bold }
                            MouseArea {
                                id: tagMa
                                anchors.fill: parent
                                onClicked: {
                                    // Open the global TagEditor with this track.
                                    if (window.tagEditor) {
                                        window.tagEditor.open(modelData)
                                    }
                                }
                            }
                        }
                        Rectangle {
                            width: 24; height: 24; radius: 12
                            color: rmMa.pressed ? "#5C2018" : "transparent"
                            Text { anchors.centerIn: parent; text: "×"
                                color: "#C58578"; font.pixelSize: 14 }
                            MouseArea {
                                id: rmMa
                                anchors.fill: parent
                                onClicked: qtobj.remove_track_from_playlist(
                                    root.playlistId, modelData.id)
                            }
                        }
                    }
                }
                MouseArea {
                    id: trackMa
                    anchors.fill: parent
                    // Below the drag handle + action buttons so they win
                    // their taps; this catches taps on the title area.
                    z: -1
                    onClicked: qtobj.enqueue_or_play(modelData.id)
                }
            }
        }
    }
}
