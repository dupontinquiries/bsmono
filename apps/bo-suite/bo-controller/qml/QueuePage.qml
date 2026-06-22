// qml/QueuePage.qml
//
// Shows the active device's play queue. The head of the queue is
// rendered slightly larger as "up next"; subsequent rows compact.
// Each row supports remove (×) and "play now" (jumps the queue).
//
// The queue updates whenever a track is pushed, popped, or removed —
// the worker publishes a new snapshot via QueueState which lands in
// queue_json and triggers queue_changed.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"

    Component.onCompleted: {
        qtobj.load_queue()
        qtobj.load_queue_devices()
    }

    property var queueList: {
        try { return JSON.parse(qtobj.queue_json || "[]") }
        catch (e) { return [] }
    }
    property var queueDevices: {
        try { return JSON.parse(qtobj.queue_devices_json || "[]") }
        catch (e) { return [] }
    }
    Connections {
        target: qtobj
        function onQueue_changed() {
            try { root.queueList = JSON.parse(qtobj.queue_json || "[]") }
            catch (e) { root.queueList = [] }
            // After every queue mutation, refresh the devices list so
            // counts stay accurate.
            qtobj.load_queue_devices()
        }
        function onQueue_devices_changed() {
            try { root.queueDevices = JSON.parse(qtobj.queue_devices_json || "[]") }
            catch (e) { root.queueDevices = [] }
        }
        // When the active device changes (user switches the device
        // from the picker), automatically reload that device's queue.
        function onUrl_changed() {
            qtobj.load_queue()
        }
    }

    // Friendly label for a device URL: pull out the hostname.
    function deviceLabel(url) {
        if (!url) return "(default)"
        var m = url.match(/https?:\/\/([^\/:]+)/)
        return m ? m[1] : url
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapSM

        Row {
            width: parent.width
            spacing: Bo.gapSM
            Text {
                text: "QUEUE"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                text: "· " + root.queueList.length + " track"
                    + (root.queueList.length === 1 ? "" : "s")
                color: Bo.silver
                font.pixelSize: 10
                anchors.verticalCenter: parent.verticalCenter
            }

            Item { width: parent.width - 100; height: 1 }   // spacer

            Rectangle {
                width: 70; height: 28; radius: 14
                color: clearMa.pressed ? "#5C2018" : "transparent"
                border.color: "#8E4A3D"; border.width: 1
                visible: root.queueList.length > 0
                Text {
                    anchors.centerIn: parent
                    text: "Clear"; color: "#C58578"
                    font.pixelSize: 10
                }
                MouseArea {
                    id: clearMa; anchors.fill: parent
                    onClicked: qtobj.queue_clear()
                }
            }
        }

        // ── PER-DEVICE PICKER ──────────────────────────────────────
        //
        // Horizontally-scrollable strip listing every device that
        // currently has queued items. The active device's chip is
        // highlighted in accent. Tapping another device's chip swaps
        // the queue view to that device WITHOUT making it the active
        // device — useful for inspecting "what did I queue on the
        // kitchen Stage from upstairs?" without re-routing volume.
        //
        // Hidden when only one device (or zero) has queued items,
        // since the picker would be redundant.
        Flickable {
            width: parent.width
            height: visible ? 32 : 0
            visible: root.queueDevices.length > 1
            contentWidth: deviceChipsRow.implicitWidth
            contentHeight: 28
            boundsBehavior: Flickable.OvershootBounds
            flickableDirection: Flickable.HorizontalFlick
            clip: true

            Row {
                id: deviceChipsRow
                spacing: 6
                anchors.verticalCenter: parent.verticalCenter

                Repeater {
                    model: root.queueDevices
                    delegate: Rectangle {
                        property bool isActive:
                            modelData.device_url === qtobj.queue_view_device_url
                        property bool isPlayingDevice:
                            modelData.device_url === qtobj.active_url
                        height: 24; radius: 12
                        width: chipLabel.implicitWidth + 38
                        color: isActive
                            ? Bo.mix(Bo.surface, root.accent, 0.30)
                            : "transparent"
                        border.color: isActive ? root.accent
                                               : (isPlayingDevice ? Bo.silver : Bo.hairline)
                        border.width: isActive ? 2 : 1
                        Row {
                            anchors.centerIn: parent
                            spacing: 4
                            // ▶ glyph next to the device that's currently
                            // active (so the user can tell which queue
                            // the controller will pop from on next play).
                            Text {
                                visible: parent.parent.isPlayingDevice
                                text: "▶"
                                color: parent.parent.isActive ? root.accent : Bo.silver
                                font.pixelSize: 9
                                font.weight: Font.DemiBold
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            Text {
                                id: chipLabel
                                text: root.deviceLabel(modelData.device_url)
                                color: parent.parent.isActive ? Bo.bone : Bo.silver
                                font.pixelSize: 10
                                font.weight: Font.DemiBold
                                font.letterSpacing: 0.4
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            // Item-count badge.
                            Rectangle {
                                width: countLabel.implicitWidth + 8
                                height: 16; radius: 8
                                color: Bo.mix(Bo.surface, root.accent, 0.45)
                                anchors.verticalCenter: parent.verticalCenter
                                Text {
                                    id: countLabel
                                    anchors.centerIn: parent
                                    text: modelData.item_count
                                    color: Bo.bone
                                    font.pixelSize: 9
                                    font.weight: Font.DemiBold
                                }
                            }
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: {
                                qtobj.load_queue_for_device(modelData.device_url)
                            }
                        }
                    }
                }
            }
        }

        Text {
            visible: root.queueList.length === 0
            text: "No tracks queued — add some from the library or a playlist"
            color: Bo.silver
            font.pixelSize: Bo.captionSize
            width: parent.width
            wrapMode: Text.WordWrap
        }

        // Featured "up next" row
        Rectangle {
            visible: root.queueList.length > 0
            width: parent.width
            height: 72
            radius: Bo.cardRadius
            color: Bo.mix(Bo.surface, root.accent, 0.10)
            border.color: root.accent; border.width: 1

            Row {
                anchors.fill: parent
                anchors.margins: Bo.gapMD
                spacing: Bo.gapSM

                // Pulsing dot indicating "up next"
                Rectangle {
                    width: 10; height: 10; radius: 5
                    color: root.accent
                    anchors.verticalCenter: parent.verticalCenter
                    SequentialAnimation on opacity {
                        loops: Animation.Infinite
                        NumberAnimation { to: 0.45; duration: 900; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 1.00; duration: 900; easing.type: Easing.InOutSine }
                    }
                }

                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 10 - 60 - Bo.gapSM*2
                    spacing: 2
                    Text {
                        text: "UP NEXT"
                        color: root.accent
                        font.pixelSize: 9
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1.2
                    }
                    Text {
                        text: root.queueList.length > 0
                            ? (root.queueList[0].title || "(untitled)")
                            : ""
                        color: Bo.bone
                        font.pixelSize: Bo.bodySize
                        font.weight: Font.Medium
                        elide: Text.ElideRight
                        width: parent.width
                    }
                    Text {
                        text: root.queueList.length > 0
                            ? (root.queueList[0].primary_artist || "")
                            : ""
                        color: Bo.silver
                        font.pixelSize: 10
                        elide: Text.ElideRight
                        width: parent.width
                    }
                }

                Rectangle {
                    width: 50; height: 28; radius: 14
                    color: root.accent
                    anchors.verticalCenter: parent.verticalCenter
                    Text {
                        anchors.centerIn: parent
                        text: "▶"; color: Bo.graphite
                        font.pixelSize: 12; font.weight: Font.DemiBold
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            if (root.queueList.length > 0) {
                                qtobj.play_track(root.queueList[0].track_id)
                                qtobj.queue_pop()
                            }
                        }
                    }
                }
            }
        }

        // Remaining queue (everything after position 0)
        ListView {
            // Stronger scroll feel (momentum-y flings).
            flickDeceleration: Bo.flickDecel
            maximumFlickVelocity: Bo.flickMaxVel
            // Clear the BottomTransportBar so the final row isn't
            // hidden behind it (bottom-cutoff fix).
            bottomMargin: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 130
            spacing: 4
            clip: true
            visible: root.queueList.length > 1
            model: root.queueList.slice(1)
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
                        text: (index + 2).toString().padStart(2, "0")
                        color: Bo.silver
                        font.pixelSize: 10
                        width: 20
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 1
                        width: parent.width - 20 - 60 - Bo.gapSM*2
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
                    Rectangle {
                        width: 24; height: 24; radius: 12
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
                                    // QueueEntry has track_id (not id);
                                    // synthesize a TrackSummary-shaped
                                    // object for the editor to consume.
                                    window.tagEditor.open({
                                        id: modelData.track_id,
                                        title: modelData.title,
                                    })
                                }
                            }
                        }
                    }
                    Rectangle {
                        width: 24; height: 24; radius: 12
                        anchors.verticalCenter: parent.verticalCenter
                        color: rmMa.pressed ? "#5C2018" : "transparent"
                        Text { anchors.centerIn: parent; text: "×"
                            color: "#C58578"; font.pixelSize: 14 }
                        MouseArea {
                            id: rmMa
                            anchors.fill: parent
                            onClicked: qtobj.queue_remove(modelData.queue_id)
                        }
                    }
                }
                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    // In the queue, tapping a row means "play now" —
                    // the user is explicitly choosing to jump to it.
                    onClicked: qtobj.play_now(modelData.track_id)
                }
            }
        }
    }
}
