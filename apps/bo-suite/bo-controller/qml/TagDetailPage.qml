// qml/TagDetailPage.qml
//
// Drill-down view for one tag: shows all tracks with that tag,
// ordered by play count (most-played first). Each row has tap-to-
// play and a + button to queue. A "Play all" header button clears
// the queue and starts playing through the whole tag.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property int tagId: -1
    property string tagName: ""
    property color tagColor: "#C8A04B"
    property color accent: "#C8A04B"
    signal back()

    onTagIdChanged: if (tagId >= 0) qtobj.load_tracks_by_tag(tagId)

    property var trackList: {
        var ctx = qtobj.library_context || ""
        if (ctx === "tag:" + root.tagId) {
            try { return JSON.parse(qtobj.library_json || "[]") }
            catch (e) { return [] }
        }
        return []
    }
    Connections {
        target: qtobj
        function onLibrary_changed() {
            var ctx = qtobj.library_context || ""
            if (ctx === "tag:" + root.tagId) {
                try { root.trackList = JSON.parse(qtobj.library_json || "[]") }
                catch (e) { root.trackList = [] }
            }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapSM

        // Header
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
            Rectangle {
                width: 36; height: 24; radius: 12
                color: root.tagColor
                anchors.verticalCenter: parent.verticalCenter
                Text {
                    anchors.centerIn: parent
                    text: "#"
                    color: Bo.graphite
                    font.pixelSize: 13
                    font.weight: Font.DemiBold
                }
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    text: root.tagName
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

        // Play-all + queue-all (only when tag has tracks)
        Row {
            spacing: Bo.gapSM
            visible: root.trackList.length > 0
            Rectangle {
                width: 110; height: 32; radius: 16
                color: root.tagColor
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

        Text {
            visible: root.trackList.length === 0
            text: "No tracks have this tag yet"
            color: Bo.slate
            font.pixelSize: Bo.captionSize
        }

        ListView {
            width: parent.width
            height: parent.height - 110
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
                        width: parent.width - 20 - 56 - Bo.gapSM*2
                        spacing: 1
                        Text {
                            text: modelData.title || "(untitled)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize - 1
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Text {
                            visible: modelData.primary_artist
                                  && modelData.primary_artist.length > 0
                            text: modelData.primary_artist || ""
                            color: Bo.silver
                            font.pixelSize: 9
                            elide: Text.ElideRight
                            width: parent.width
                        }
                    }
                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        Rectangle {
                            width: 26; height: 24; radius: 12
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
                            width: 26; height: 24; radius: 12
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
