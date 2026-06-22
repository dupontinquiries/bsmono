// qml/PlaylistsPage.qml
//
// Browse + manage playlists. Top of the page has a "New Playlist"
// affordance (compact card → expands into a name/description form on
// tap). Below, a scrollable list of existing playlists with their
// track counts and last-updated time. Tapping a playlist routes to
// PlaylistDetailPage.qml.
//
// State binding: reads qtobj.playlists_json (JSON array of
// PlaylistSummary). The list is updated whenever the worker
// re-publishes (after create / delete / update / track add or
// remove). The page asks for a fresh list on open via load_playlists.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    signal openPlaylist(int id, string name)

    // Re-fetch on open so we don't show stale state if a previous edit
    // happened off-screen.
    Component.onCompleted: qtobj.load_playlists()

    // Parsed JSON, kept as a property so changes trigger ListView updates.
    property var playlistsList: {
        try { return JSON.parse(qtobj.playlists_json || "[]") }
        catch (e) { return [] }
    }

    Connections {
        target: qtobj
        function onPlaylists_changed() {
            try { root.playlistsList = JSON.parse(qtobj.playlists_json || "[]") }
            catch (e) { root.playlistsList = [] }
        }
        function onLast_playlist_changed() {
            // Newly created playlist: scroll to top so user sees it.
            listView.positionViewAtBeginning()
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        // Header
        Text {
            text: "PLAYLISTS"
            color: Bo.silver
            font.pixelSize: Bo.captionSize
            font.weight: Font.DemiBold
            font.letterSpacing: 1.2
        }

        // ── New playlist card ──
        //
        // Compact by default; tapping anywhere expands to show name +
        // description text fields plus Create/Cancel buttons. The card
        // gracefully animates between the two states.
        Rectangle {
            id: createCard
            width: parent.width
            height: createCard.expanded ? 160 : 48
            radius: Bo.cardRadius
            color: Bo.surface
            border.color: Bo.hairline; border.width: 1
            property bool expanded: false
            Behavior on height { NumberAnimation { duration: Bo.base; easing.type: Easing.OutCubic } }

            // Collapsed view
            Row {
                anchors.left: parent.left; anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.margins: Bo.gapMD
                spacing: Bo.gapSM
                visible: !createCard.expanded
                Rectangle {
                    width: 24; height: 24; radius: 12
                    color: Bo.mix(Bo.surface, root.accent, 0.25)
                    border.color: root.accent; border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: "+"; color: root.accent
                        font.pixelSize: 16; font.weight: Font.DemiBold
                    }
                }
                Text {
                    text: "New playlist"
                    color: Bo.bone
                    font.pixelSize: Bo.bodySize
                    anchors.verticalCenter: parent.verticalCenter
                }
            }
            // Expanded view
            Column {
                visible: createCard.expanded
                anchors.fill: parent
                anchors.margins: Bo.gapMD
                spacing: Bo.gapSM
                TextField {
                    id: nameField
                    width: parent.width
                    placeholderText: "Playlist name"
                    color: Bo.bone
                    background: Rectangle {
                        color: Bo.graphite
                        radius: 4
                        border.color: Bo.hairline; border.width: 1
                    }
                }
                TextField {
                    id: descField
                    width: parent.width
                    placeholderText: "Description (optional)"
                    color: Bo.silver
                    background: Rectangle {
                        color: Bo.graphite
                        radius: 4
                        border.color: Bo.hairline; border.width: 1
                    }
                }
                Row {
                    spacing: Bo.gapSM
                    anchors.right: parent.right
                    Rectangle {
                        width: 80; height: 32; radius: 16
                        color: cancelMa.pressed ? Bo.surfaceHi : "transparent"
                        border.color: Bo.hairline; border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: "Cancel"; color: Bo.silver
                            font.pixelSize: Bo.captionSize
                        }
                        MouseArea {
                            id: cancelMa
                            anchors.fill: parent
                            onClicked: {
                                nameField.text = ""
                                descField.text = ""
                                createCard.expanded = false
                            }
                        }
                    }
                    Rectangle {
                        width: 80; height: 32; radius: 16
                        color: nameField.text.length > 0 ? root.accent
                             : Bo.mix(Bo.surface, root.accent, 0.30)
                        Text {
                            anchors.centerIn: parent
                            text: "Create"; color: Bo.graphite
                            font.pixelSize: Bo.captionSize
                            font.weight: Font.DemiBold
                        }
                        MouseArea {
                            anchors.fill: parent
                            enabled: nameField.text.length > 0
                            onClicked: {
                                qtobj.create_playlist(nameField.text, descField.text)
                                nameField.text = ""
                                descField.text = ""
                                createCard.expanded = false
                            }
                        }
                    }
                }
            }
            // Whole-card tap (collapsed state) expands the card.
            MouseArea {
                anchors.fill: parent
                enabled: !createCard.expanded
                onClicked: { createCard.expanded = true; nameField.forceActiveFocus() }
            }
        }

        // ── Playlist list ──
        ListView {
            // Stronger scroll feel (momentum-y flings).
            flickDeceleration: Bo.flickDecel
            maximumFlickVelocity: Bo.flickMaxVel
            // Clear the BottomTransportBar so the final row isn't
            // hidden behind it (bottom-cutoff fix).
            bottomMargin: Bo.bottomChromeHeight
            id: listView
            width: parent.width
            height: parent.height - createCard.height - 40
            spacing: Bo.gapSM
            clip: true
            model: root.playlistsList
            cacheBuffer: 240    // smooth scroll without aggressive evict
            ScrollIndicator.vertical: ScrollIndicator {}

            // Empty-state placeholder
            Text {
                anchors.centerIn: parent
                visible: listView.count === 0
                text: "No playlists yet — create one to get started"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
            }

            delegate: Rectangle {
                width: ListView.view.width
                height: 64
                radius: Bo.cardRadius
                color: rowMa.pressed ? Bo.surfaceHi : Bo.surface
                border.color: Bo.hairline; border.width: 1
                scale: rowMa.pressed ? 0.99 : 1.0
                Behavior on scale { NumberAnimation { duration: Bo.instant } }

                Row {
                    anchors.left: parent.left; anchors.leftMargin: Bo.gapMD
                    anchors.right: parent.right; anchors.rightMargin: Bo.gapMD
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Bo.gapSM

                    // Tiny circular badge with track count
                    Rectangle {
                        width: 36; height: 36; radius: 18
                        color: Bo.mix(Bo.surface, root.accent, 0.18)
                        border.color: Bo.hairline; border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: modelData.track_count
                            color: root.accent
                            font.pixelSize: 11
                            font.weight: Font.DemiBold
                        }
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        width: parent.width - 36 - Bo.gapSM
                        Text {
                            text: modelData.name
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize
                            font.weight: Font.Medium
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Text {
                            visible: modelData.description && modelData.description.length > 0
                            text: modelData.description || ""
                            color: Bo.silver
                            font.pixelSize: Bo.captionSize
                            elide: Text.ElideRight
                            width: parent.width
                        }
                    }
                }

                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    onClicked: root.openPlaylist(modelData.id, modelData.name)
                }
            }
        }
    }
}
