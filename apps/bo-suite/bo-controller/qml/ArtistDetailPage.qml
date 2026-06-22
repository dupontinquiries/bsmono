// qml/ArtistDetailPage.qml
//
// Shows one artist's tracks. Uses qtobj.load_by_artist(id) which is
// the existing artist drill-down (returns a TrackList tagged
// "artist:<id>" in library_json/library_context).
//
// Each track row: tap → play, "+ queue" → push to queue.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property int artistId: -1
    property string artistName: ""
    property color accent: "#C8A04B"
    signal back()

    onArtistIdChanged: if (artistId >= 0) qtobj.load_by_artist(artistId)

    property var trackList: {
        // We only adopt the list when the worker's context matches this
        // artist id; otherwise the library_json might be showing recent
        // tracks or another artist's drill-down.
        var ctx = qtobj.library_context || ""
        if (ctx === "artist:" + root.artistId) {
            try { return JSON.parse(qtobj.library_json || "[]") }
            catch (e) { return [] }
        }
        return []
    }
    Connections {
        target: qtobj
        function onLibrary_changed() {
            var ctx = qtobj.library_context || ""
            if (ctx === "artist:" + root.artistId) {
                try { root.trackList = JSON.parse(qtobj.library_json || "[]") }
                catch (e) { root.trackList = [] }
            }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapSM

        // Back + name header
        Row {
            width: parent.width
            spacing: Bo.gapSM
            Rectangle {
                width: 32; height: 32; radius: 16
                color: backMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text { anchors.centerIn: parent; text: "‹"
                    color: Bo.bone; font.pixelSize: 18; font.weight: Font.DemiBold }
                MouseArea { id: backMa; anchors.fill: parent; onClicked: root.back() }
                anchors.verticalCenter: parent.verticalCenter
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    text: root.artistName
                    color: Bo.bone
                    font.pixelSize: Bo.bodySize + 2
                    font.weight: Font.DemiBold
                }
                Text {
                    text: root.trackList.length + " track"
                        + (root.trackList.length === 1 ? "" : "s")
                    color: Bo.silver
                    font.pixelSize: 10
                }
            }
        }

        // Play-all + queue-all
        Row {
            spacing: Bo.gapSM
            visible: root.trackList.length > 0
            Rectangle {
                width: 110; height: 32; radius: 16
                color: root.accent
                Text { anchors.centerIn: parent
                    text: "▶ Play all"; color: Bo.graphite
                    font.pixelSize: Bo.captionSize; font.weight: Font.DemiBold }
                MouseArea {
                    anchors.fill: parent
                    onClicked: {
                        qtobj.queue_clear()
                        for (var i = 0; i < root.trackList.length; ++i) {
                            qtobj.queue_push(root.trackList[i].id)
                        }
                        qtobj.play_track(root.trackList[0].id)
                    }
                }
            }
            Rectangle {
                width: 90; height: 32; radius: 16
                color: queueAllMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text { anchors.centerIn: parent
                    text: "+ Queue"; color: Bo.bone; font.pixelSize: Bo.captionSize }
                MouseArea {
                    id: queueAllMa
                    anchors.fill: parent
                    onClicked: {
                        for (var i = 0; i < root.trackList.length; ++i) {
                            qtobj.queue_push(root.trackList[i].id)
                        }
                    }
                }
            }
        }

        ListView {
            // Stronger scroll feel (momentum-y flings).
            flickDeceleration: Bo.flickDecel
            maximumFlickVelocity: Bo.flickMaxVel
            // Clear the BottomTransportBar so the final row isn't
            // hidden behind it (bottom-cutoff fix).
            bottomMargin: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 100
            spacing: 4
            clip: true
            model: root.trackList
            cacheBuffer: 240
            ScrollIndicator.vertical: ScrollIndicator {}

            delegate: Rectangle {
                width: ListView.view.width
                height: 44
                radius: 6
                color: rowMa.pressed ? Bo.surfaceHi : "transparent"

                Row {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    spacing: Bo.gapSM

                    Text {
                        text: (index + 1).toString().padStart(2, "0")
                        color: Bo.silver; font.pixelSize: 10
                        width: 20
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        width: parent.width - 20 - 28 - Bo.gapSM*2
                        spacing: 1
                        Text {
                            text: modelData.title || "(untitled)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize - 1
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Text {
                            visible: modelData.album && modelData.album.length > 0
                            text: modelData.album || ""
                            color: Bo.silver
                            font.pixelSize: 9
                            elide: Text.ElideRight
                            width: parent.width
                        }
                    }
                    Rectangle {
                        width: 28; height: 24; radius: 12
                        anchors.verticalCenter: parent.verticalCenter
                        color: tagMa.pressed ? Bo.surfaceHi : "transparent"
                        Text { anchors.centerIn: parent; text: "#"
                            color: Bo.silver; font.pixelSize: 12
                            font.weight: Font.Bold }
                        MouseArea {
                            id: tagMa
                            anchors.fill: parent
                            onClicked: {
                                if (window.tagEditor) {
                                    window.tagEditor.open(modelData)
                                }
                            }
                        }
                    }
                    Rectangle {
                        width: 28; height: 24; radius: 12
                        anchors.verticalCenter: parent.verticalCenter
                        color: queueMa.pressed ? Bo.surfaceHi : "transparent"
                        Text { anchors.centerIn: parent; text: "+"
                            color: Bo.silver; font.pixelSize: 13 }
                        MouseArea {
                            id: queueMa
                            anchors.fill: parent
                            onClicked: qtobj.queue_push(modelData.id)
                        }
                    }
                }
                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    onClicked: qtobj.enqueue_or_play(modelData.id)
                }
            }
        }
    }
}
