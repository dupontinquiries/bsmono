// qml/SearchPage.qml
//
// Library search. Single text input, debounced so we don't fire a
// query on every keystroke. Results appear inline with the same
// row layout as ArtistDetailPage / PlaylistDetailPage so the
// experience stays consistent.
//
// Search is multi-term AND with prefix matching + Unicode folding.
// Typing "bjork dance" finds tracks where both "bjork" (or "björk")
// and "dance" appear in title, artist, or album.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"

    property var resultList: []

    Connections {
        target: qtobj
        function onLibrary_changed() {
            // `library_context` comes through as QString; QML's JS engine
            // doesn't always reflect QString to the JS String prototype
            // when accessed via property lookup, so `.indexOf` would
            // throw "Property 'indexOf' of object … is not a function".
            // Force a JS string with String(...) which goes through the
            // QString → JS conversion path.
            var ctx = String(qtobj.library_context || "")
            if (ctx.indexOf("search:") === 0) {
                try { root.resultList = JSON.parse(qtobj.library_json || "[]") }
                catch (e) { root.resultList = [] }
            }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapSM

        Text {
            text: "SEARCH"
            color: Bo.silver
            font.pixelSize: Bo.captionSize
            font.weight: Font.DemiBold
            font.letterSpacing: 1.2
        }

        TextField {
            id: queryField
            width: parent.width
            height: Bo.touchMin
            placeholderText: "Title, artist, album…"
            placeholderTextColor: Bo.slate
            color: Bo.bone
            font.pixelSize: Bo.bodySize
            leftPadding: Bo.gapMD
            // Leave room on the right for the clear (×) button.
            rightPadding: Bo.touchMin
            verticalAlignment: TextInput.AlignVCenter
            background: Rectangle {
                color: Bo.graphite
                radius: 8
                border.color: queryField.activeFocus ? root.accent : Bo.hairline
                border.width: 1
            }
            // Inline clear button — a quick way to reset the search
            // without selecting-all + delete, and a clear affordance that
            // the field is editable. Only shown when there's text.
            Rectangle {
                id: clearBtn
                visible: queryField.text.length > 0
                width: Bo.touchMin; height: Bo.touchMin
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                color: "transparent"
                Text {
                    anchors.centerIn: parent
                    text: "✕"
                    color: clearMa.containsMouse ? Bo.bone : Bo.slate
                    font.pixelSize: Bo.bodySize
                }
                MouseArea {
                    id: clearMa
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: {
                        queryField.text = ""
                        root.resultList = []
                        queryField.forceActiveFocus()
                    }
                }
            }
            // Debounce keystrokes: a Timer restarts on every change,
            // and only fires search 220 ms after the user stops typing.
            // Saves a lot of redundant SQLite work on fast typers.
            Timer {
                id: debounce
                interval: 220
                onTriggered: qtobj.search_advanced(queryField.text)
            }
            onTextChanged: {
                debounce.restart()
                if (queryField.text.length === 0) root.resultList = []
            }
        }

        // Result count — gives the search a sense of how much matched,
        // and confirms the search ran. Hidden until there's a query.
        Text {
            visible: queryField.text.length > 0 && root.resultList.length > 0
            text: root.resultList.length
                + (root.resultList.length === 1 ? " result" : " results")
            color: Bo.slate
            font.pixelSize: Bo.captionSize
        }

        Text {
            visible: root.resultList.length === 0 && queryField.text.length > 0
            text: "No matches"
            color: Bo.silver
            font.pixelSize: Bo.captionSize
        }
        Text {
            visible: queryField.text.length === 0
            text: "Type to search across your library"
            color: Bo.silver
            font.pixelSize: Bo.captionSize
        }

        ListView {
            // Stronger scroll feel (momentum-y flings).
            flickDeceleration: Bo.flickDecel
            maximumFlickVelocity: Bo.flickMaxVel
            boundsBehavior: Flickable.DragOverBounds
            pressDelay: Bo.pressDelay
            rebound: BoRebound {}
            // Clear the BottomTransportBar so the final row isn't
            // hidden behind it (bottom-cutoff fix).
            bottomMargin: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 110
            spacing: 4
            clip: true
            model: root.resultList
            cacheBuffer: 240
            ScrollIndicator.vertical: ScrollIndicator {}

            delegate: Rectangle {
                width: ListView.view.width
                height: 48
                radius: 6
                color: rowMa.pressed ? Bo.surfaceHi : "transparent"

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
                            text: modelData.title || "(untitled)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Text {
                            text: (modelData.primary_artist || "")
                                + (modelData.album && modelData.album.length > 0
                                       ? " · " + modelData.album : "")
                            color: Bo.silver
                            font.pixelSize: 10
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
                        Rectangle {
                            width: 26; height: 24; radius: 12
                            color: root.accent
                            Text { anchors.centerIn: parent; text: "▶"
                                color: Bo.graphite; font.pixelSize: 9
                                font.weight: Font.DemiBold }
                            MouseArea {
                                anchors.fill: parent
                                // Play now (displace current track to queue[0])
                                onClicked: qtobj.play_now(modelData.id)
                            }
                        }
                    }
                }
                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    // Single tap while something is playing → queue it.
                    // The play_now (▶) button gives instant play.
                    onClicked: qtobj.enqueue_or_play(modelData.id)
                    onPressAndHold: {
                        if (window.editDialog) window.editDialog.open(modelData)
                    }
                }
            }
        }
    }
}
