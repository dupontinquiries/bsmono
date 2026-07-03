// qml/TagsPage.qml
//
// Browse all tags as a colored grid. Each tile shows the tag name,
// its track count, and uses the tag's color (with a sensible default
// when none was set). Tap a tile to drill into the tag's tracks via
// TagDetailPage.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    signal openTag(int id, string name, var color)

    Component.onCompleted: qtobj.load_all_tags()

    property var allTags: {
        try { return JSON.parse(qtobj.tags_json || "[]") }
        catch (e) { return [] }
    }
    Connections {
        target: qtobj
        function onTags_changed() {
            try { root.allTags = JSON.parse(qtobj.tags_json || "[]") }
            catch (e) { root.allTags = [] }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        Row {
            width: parent.width
            spacing: Bo.gapSM
            Text {
                text: "TAGS"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                text: "· " + root.allTags.length
                color: Bo.silver
                font.pixelSize: 10
                anchors.verticalCenter: parent.verticalCenter
            }
        }

        Text {
            visible: root.allTags.length === 0
            width: parent.width
            text: "No tags yet — tap the # button on a track row to create some"
            color: Bo.slate
            font.pixelSize: Bo.captionSize
            wrapMode: Text.Wrap
        }

        // Tag tile grid. We use a Flow so tiles wrap based on their
        // intrinsic width — short tag names get narrow tiles, long
        // names get wider ones. Nicer visually than a fixed grid.
        ScrollView {
            // Reserve space for the BottomTransportBar so the last
            // row/card isn't clipped or overlapped (bottom-cutoff fix).
            bottomPadding: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 40
            clip: true
            // ScrollView wraps an internal Flickable; tune its fling for
            // the same momentum feel as the ListView pages.
            Component.onCompleted: {
                if (contentItem) {
                    contentItem.flickDeceleration = Bo.flickDecel
                    contentItem.maximumFlickVelocity = Bo.flickMaxVel
                    contentItem.boundsBehavior = Flickable.DragOverBounds
                    contentItem.pressDelay = Bo.pressDelay
                }
            }

            Flow {
                width: parent.width
                spacing: Bo.gapSM
                Repeater {
                    model: root.allTags
                    delegate: Rectangle {
                        property color tagColor: modelData.color || root.accent
                        height: 56; radius: 8
                        width: Math.max(96, tileText.implicitWidth + 32)
                        color: Bo.mix(Bo.surface, tagColor, 0.18)
                        border.color: tagColor
                        border.width: 1
                        scale: tileMa.pressed ? 0.97 : 1.0
                        Behavior on scale { NumberAnimation { duration: Bo.instant } }

                        Column {
                            anchors.centerIn: parent
                            spacing: 2
                            Text {
                                id: tileText
                                text: modelData.name
                                color: Bo.bone
                                font.pixelSize: Bo.bodySize - 1
                                font.weight: Font.Medium
                                anchors.horizontalCenter: parent.horizontalCenter
                            }
                            Text {
                                text: modelData.track_count + " track"
                                    + (modelData.track_count === 1 ? "" : "s")
                                color: tagColor
                                font.pixelSize: 10
                                anchors.horizontalCenter: parent.horizontalCenter
                            }
                        }

                        MouseArea {
                            id: tileMa
                            anchors.fill: parent
                            onClicked: root.openTag(modelData.id, modelData.name,
                                                    tagColor)
                        }
                    }
                }
            }
        }
    }
}
