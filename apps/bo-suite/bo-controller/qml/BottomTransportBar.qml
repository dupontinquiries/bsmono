// qml/BottomTransportBar.qml
//
// Persistent bottom transport bar — sits above the page-dots indicator
// on every page. Layout is ANCHORED (not width-computed) so controls
// never get pushed off-screen on narrow phone displays:
//
//   [ sink chip ]   [ identicon · title/artist · scrub ]   [ controls ]
//      left              center (fills remaining space)        right
//
// Right-side control cluster (always visible, pinned right):
//   • Volume: soften (−4%) · mute toggle w/ level readout · louder (+4%)
//   • Transport: prev (Mozart) · stop (local/BT) · PLAY/PAUSE · next
//
// The PLAY/PAUSE button is the primary action: larger (40px), filled.
//   - Mozart: transport("play"/"pause") via REST.
//   - BT/Local: np_playing (set by Rust) drives the glyph. ❚❚ stops
//     paplay but remembers position; ▶ resumes via seek_to.
//
// Center block: per-track identicon, title + artist, and a draggable
// scrub track (when duration is known). Drag to seek.
//
// Above the main bar: an optional per-device mini-strip appears when
// 2+ devices have queued items.
//
// Hidden entirely when driver_kind is "noop" — nothing to control.

import QtQuick 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    // Bumped from 52 → 64 to fit the new title+identicon+progress
    // block. Two lines of text (title + artist) + a 2 px progress
    // pixel + breathing room come to ~50 px; 64 keeps the round
    // corners clean and matches the touch-min ergonomics.
    // Per-device strip adds 32 px when visible (multi-device only).
    property var queueDevices: {
        try { return JSON.parse(qtobj.queue_devices_json || "[]") }
        catch (e) { return [] }
    }
    Connections {
        target: qtobj
        function onQueue_devices_changed() {
            try { root.queueDevices = JSON.parse(qtobj.queue_devices_json || "[]") }
            catch (e) { root.queueDevices = [] }
        }
    }
    property bool showDeviceStrip: queueDevices.length > 1
    property int deviceStripHeight: showDeviceStrip ? 34 : 0
    height: 72 + deviceStripHeight
    visible: qtobj.current_driver_kind !== "noop"

    // Helper: friendly label from a device URL
    function deviceLabel(url) {
        if (!url || url === "") return "(default)"
        var m = url.match(/https?:\/\/([^/:]+)/)
        return m ? m[1] : url
    }

    // ── Per-device mini-strip ──────────────────────────────────────
    // Shown when 2+ devices have queued items. Each chip shows the
    // device hostname + queue count; the active device (current
    // playback target) is highlighted. Tapping a chip calls
    // load_queue_for_device so the Queue page switches to that
    // device's view without re-routing audio.
    Flickable {
        id: deviceStrip
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: 14
        anchors.rightMargin: 14
        height: root.deviceStripHeight
        visible: root.showDeviceStrip
        contentWidth: deviceStripRow.implicitWidth
        contentHeight: 28
        boundsBehavior: Flickable.OvershootBounds
        flickableDirection: Flickable.HorizontalFlick
        clip: true
        opacity: root.showDeviceStrip ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: 150 } }

        Row {
            id: deviceStripRow
            spacing: 6
            anchors.verticalCenter: parent.verticalCenter

            Repeater {
                model: root.queueDevices
                delegate: Rectangle {
                    property bool isActive: modelData.device_url === qtobj.active_url
                    height: 26; radius: 13
                    width: chipLbl.implicitWidth + badgePill.width + 22
                    color: isActive
                        ? Qt.rgba(0.78, 0.63, 0.29, 0.25)
                        : Qt.rgba(1, 1, 1, 0.04)
                    border.color: isActive ? root.accent : Bo.hairline
                    border.width: isActive ? 2 : 1

                    Row {
                        anchors.centerIn: parent
                        spacing: 5
                        // ▶ glyph marks the device that's currently
                        // receiving audio.
                        Text {
                            visible: isActive
                            text: qtobj.np_playing ? "▶" : "❚"
                            color: root.accent
                            font.pixelSize: 8
                            font.weight: Font.DemiBold
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Text {
                            id: chipLbl
                            text: root.deviceLabel(modelData.device_url)
                            color: isActive ? Bo.bone : Bo.silver
                            font.pixelSize: 10
                            font.weight: Font.DemiBold
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        // Queue-count badge
                        Rectangle {
                            id: badgePill
                            width: badgeLbl.implicitWidth + 8
                            height: 16; radius: 8
                            color: Qt.rgba(0.78, 0.63, 0.29, 0.35)
                            anchors.verticalCenter: parent.verticalCenter
                            Text {
                                id: badgeLbl
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
                        onClicked: qtobj.load_queue_for_device(modelData.device_url)
                    }
                }
            }
        }
    }

    // ── Main transport bar ────────────────────────────────────────

    // Driver-kind-driven color so the bar's accent matches the chip
    // on the Sound page — same visual language.
    property color sinkColor: {
        switch (qtobj.current_driver_kind) {
            case "bluetooth": return "#5C8A8A"
            case "multiroom": return "#C8A04B"
            case "mozart":    return root.accent
            case "noop":      return "#8E4A3D"
            default:          return Bo.hairline
        }
    }
    property string sinkIcon: {
        switch (qtobj.current_driver_kind) {
            case "bluetooth": return "✦"
            case "multiroom": return "🔊+"
            case "mozart":    return "⇄"
            case "noop":      return "✕"
            default:          return "♪"
        }
    }
    property string sinkLabel: {
        if (qtobj.current_driver_kind === "bluetooth") {
            return qtobj.active_bt_name || qtobj.active_bt_mac || "Bluetooth"
        }
        if (qtobj.current_driver_kind === "multiroom") {
            try {
                var j = JSON.parse(qtobj.active_multiroom_json || "null")
                if (j && j.names && j.names.length > 0) {
                    return j.names.length + " speakers"
                }
            } catch (e) {}
            return "Multi-room"
        }
        if (qtobj.current_driver_kind === "mozart") {
            return qtobj.product_name || "device"
        }
        return "this device"
    }
    // Driver kinds that have a paplay we can kill / replay through.
    property bool drivesLocally:
        qtobj.current_driver_kind === "local"
        || qtobj.current_driver_kind === "bluetooth"
        || qtobj.current_driver_kind === "multiroom"

    Rectangle {
        id: mainBar
        anchors.top: deviceStrip.bottom
        anchors.topMargin: root.showDeviceStrip ? 4 : 0
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: 12
        anchors.rightMargin: 12
        radius: 26
        color: Bo.surface
        border.color: Bo.hairline
        border.width: 1

        // Layout strategy: ANCHORED ZONES, not a width-computed Row.
        //
        // The old layout hand-computed the title width as
        // `parent.width - magic_numbers`. On a narrow phone that went
        // negative and pushed the play/pause + everything right of it
        // off-screen — that's why the play button "wasn't there".
        //
        // Now:
        //   • sinkChip is pinned LEFT (and shrinks on narrow screens)
        //   • controlCluster (volume + transport) is pinned RIGHT
        //   • titleBlock fills whatever is BETWEEN them
        // Controls can never be pushed off-screen because they own
        // their right-anchored space first; the title gives way.

        // ── Right-pinned control cluster ──────────────────────────
        Row {
            id: controlCluster
            anchors.right: parent.right
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            layoutDirection: Qt.LeftToRight

            // ── Volume sub-group: soften / mute / louder ──────────
            // Compact 3-button volume so the user can adjust level
            // from any page without opening the Sound page. The
            // mute button doubles as the level indicator.

            // Soften (−)
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 30; height: 30; radius: 15
                color: softMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "🔉"
                    font.pixelSize: 12
                }
                MouseArea {
                    id: softMa
                    anchors.fill: parent
                    // Step down 4% — bigger than the ±1 nudge on the
                    // Sound page since this is a coarse, on-the-go control.
                    onClicked: qtobj.set_volume_api(Math.max(0, qtobj.volume - 4))
                }
            }

            // Mute toggle — shows current level as a tiny number so the
            // bar always tells you where the volume sits. Tap to mute/
            // unmute. Lights up amber when muted.
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 38; height: 30; radius: 15
                color: qtobj.muted
                    ? Bo.mix(Bo.surface, "#B58853", 0.45)
                    : (muteMa.pressed ? Bo.surfaceHi : "transparent")
                border.color: qtobj.muted ? "#B58853" : Bo.hairline
                border.width: 1
                Row {
                    anchors.centerIn: parent
                    spacing: 2
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qtobj.muted ? "🔇" : "🔊"
                        font.pixelSize: 11
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qtobj.muted ? "" : qtobj.volume
                        color: Bo.silver
                        font.pixelSize: 9
                        font.weight: Font.DemiBold
                        visible: !qtobj.muted
                    }
                }
                MouseArea {
                    id: muteMa
                    anchors.fill: parent
                    onClicked: qtobj.toggle_mute()
                }
            }

            // Louder (+)
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 30; height: 30; radius: 15
                color: loudMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "🔊"
                    font.pixelSize: 12
                }
                MouseArea {
                    id: loudMa
                    anchors.fill: parent
                    onClicked: qtobj.set_volume_api(Math.min(100, qtobj.volume + 4))
                }
            }

            // Thin separator between volume and transport groups
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 1; height: 24
                color: Bo.hairline
            }

            // ── Transport sub-group ───────────────────────────────

            // Prev — Mozart-only.
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                visible: qtobj.current_driver_kind === "mozart"
                width: 32; height: 32; radius: 16
                color: prevMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "⏮"
                    color: Bo.bone
                    font.pixelSize: 12
                }
                MouseArea {
                    id: prevMa
                    anchors.fill: parent
                    onClicked: qtobj.transport("back")
                }
            }

            // Stop — local/BT only. Reliable pkill via driver_stop().
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                visible: root.drivesLocally
                width: 32; height: 32; radius: 16
                color: stopMa.pressed
                    ? Bo.mix(Bo.surface, "#B58853", 0.55)
                    : Bo.mix(Bo.surface, "#B58853", 0.18)
                border.color: "#B58853"; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "■"
                    color: "#B58853"
                    font.pixelSize: 11
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    id: stopMa
                    anchors.fill: parent
                    onClicked: {
                        qtobj.driver_stop()
                        titleBlock._pausedAtSec = 0
                    }
                }
            }

            // ── PLAY / PAUSE — the primary control, always visible ──
            // Larger (40px) and filled so it reads as the main action.
            // Mozart: transport play/pause via REST.
            // BT/Local: np_playing now set by Rust, so the glyph is
            //   accurate. ❚❚ → stop (paplay can't pause) but remembers
            //   the position; ▶ → resume from that position via seek_to.
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 40; height: 40; radius: 20
                color: ppMa.pressed
                    ? Bo.mix(Bo.surface, root.accent, 0.70)
                    : Bo.mix(Bo.surface, root.accent, 0.45)
                border.color: root.accent; border.width: 1.5
                Text {
                    anchors.centerIn: parent
                    // Optical centering: ▶ sits slightly right of center
                    anchors.horizontalCenterOffset: qtobj.np_playing ? 0 : 1
                    text: qtobj.np_playing ? "❚❚" : "▶"
                    color: root.accent
                    font.pixelSize: 14
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    id: ppMa
                    anchors.fill: parent
                    onClicked: {
                        if (qtobj.current_driver_kind === "mozart") {
                            qtobj.transport(qtobj.np_playing ? "pause" : "play")
                        } else if (qtobj.np_playing) {
                            // BT/Local "pause" = stop, but remember position.
                            var resumeSec = Math.floor(titleBlock.estimatedSec)
                            qtobj.driver_stop()
                            titleBlock.estimatedSec = resumeSec
                            titleBlock._pausedAtSec = resumeSec
                        } else {
                            // Resume from paused position if we have one.
                            var from = titleBlock._pausedAtSec || 0
                            if (qtobj.last_played_track_id_qml > 0) {
                                qtobj.play_track(qtobj.last_played_track_id_qml)
                                if (from > 0) {
                                    Qt.callLater(function() { qtobj.seek_to(from) })
                                }
                            }
                            titleBlock._pausedAtSec = 0
                        }
                    }
                }
            }

            // Next — Mozart-only.
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                visible: qtobj.current_driver_kind === "mozart"
                width: 32; height: 32; radius: 16
                color: nextMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "⏭"
                    color: Bo.bone
                    font.pixelSize: 12
                }
                MouseArea {
                    id: nextMa
                    anchors.fill: parent
                    onClicked: qtobj.transport("forward")
                }
            }
        }

        // ── Left-pinned sink chip ─────────────────────────────────
        // Tap to open the output picker. Shrinks on narrow screens so
        // it never crowds out the title. Capped between 96 and 168 px.
        Rectangle {
            id: sinkChip
            anchors.left: parent.left
            anchors.leftMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            width: Math.max(96, Math.min(168,
                mainBar.width - controlCluster.width - 180))
            height: 36
            radius: 18
            color: sinkMa.pressed
                ? Bo.surfaceHi
                : Bo.mix(Bo.surface, root.sinkColor, 0.18)
            border.color: root.sinkColor
            border.width: 1

            Row {
                anchors.centerIn: parent
                spacing: 6
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.sinkIcon
                    color: root.sinkColor
                    font.pixelSize: 11
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.sinkLabel
                    color: Bo.bone
                    font.pixelSize: 10
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                    width: Math.min(implicitWidth, sinkChip.width - 44)
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: "▾"
                    color: Bo.slate
                    font.pixelSize: 8
                }
            }
            MouseArea {
                id: sinkMa
                anchors.fill: parent
                onClicked: {
                    if (window.btSinkPicker) window.btSinkPicker.open()
                }
            }
        }

        // ── Center title + scrub block ────────────────────────────
        // Fills the space BETWEEN sinkChip and controlCluster. Because
        // both sides are anchored first, this never collapses the
        // controls — it just gets narrower (and the title elides).
        Item {
            id: titleBlock
            anchors.left: sinkChip.right
            anchors.leftMargin: 10
            anchors.right: controlCluster.left
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            height: 46
            // Show whenever there's a track. The old guard also required
            // width > 80, but during the first layout pass (before the
            // anchored controlCluster has resolved its width) this block's
            // width can momentarily be 0, which hid the title even though
            // np_title was set — the "playbar not updating" bug. We clip
            // instead, so a transiently-narrow bar shows a clipped title
            // rather than nothing.
            visible: qtobj.np_title.length > 0
            clip: true

            // Wall-clock position estimator, seeded from np_seek_pos.
            property real estimatedSec: 0
            property bool scrubbing: false
            property real scrubSec: 0
            property real _pausedAtSec: 0

            Connections {
                target: qtobj
                function onNow_playing_changed() {
                    if (!titleBlock.scrubbing) {
                        titleBlock.estimatedSec = qtobj.np_seek_pos
                    }
                }
            }
            Timer {
                interval: 1000
                repeat: true
                running: qtobj.np_playing && !titleBlock.scrubbing
                onTriggered: {
                    var dur = qtobj.np_duration_sec
                    titleBlock.estimatedSec += 1
                    if (dur > 0 && titleBlock.estimatedSec >= dur) {
                        titleBlock.estimatedSec = dur
                    }
                }
            }

            function fmtTime(s) {
                var m = Math.floor(s / 60)
                var sec = Math.floor(s) % 60
                return m + ":" + (sec < 10 ? "0" : "") + sec
            }

            Row {
                anchors.fill: parent
                spacing: 8

                Identicon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 32; height: 32
                    title: qtobj.np_title
                    borderColor: root.sinkColor
                }

                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 32 - 8
                    spacing: 3

                    Text {
                        width: parent.width
                        text: qtobj.np_title || "(no track)"
                        color: Bo.bone
                        font.pixelSize: 11
                        font.weight: Font.DemiBold
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        text: qtobj.np_artist
                        color: Bo.silver
                        font.pixelSize: 9
                        elide: Text.ElideRight
                        visible: qtobj.np_artist.length > 0
                    }

                    // ── Scrub track ──────────────────────────────
                    Item {
                        id: scrubTrack
                        width: parent.width
                        height: 14
                        visible: qtobj.np_duration_sec > 0

                        property real fillFraction: {
                            var dur = Math.max(1, qtobj.np_duration_sec)
                            var pos = titleBlock.scrubbing
                                ? titleBlock.scrubSec
                                : titleBlock.estimatedSec
                            return Math.min(1, Math.max(0, pos / dur))
                        }

                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width
                            height: 3
                            radius: 1.5
                            color: Bo.hairline
                        }
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            height: 3
                            radius: 1.5
                            color: root.sinkColor
                            width: scrubTrack.fillFraction * scrubTrack.width
                            Behavior on width {
                                enabled: !titleBlock.scrubbing
                                NumberAnimation { duration: 200 }
                            }
                        }
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            x: scrubTrack.fillFraction * (scrubTrack.width - width)
                            width: 12; height: 12; radius: 6
                            color: titleBlock.scrubbing ? Bo.bone : root.sinkColor
                            border.color: Bo.surface; border.width: 1.5
                            Behavior on x {
                                enabled: !titleBlock.scrubbing
                                NumberAnimation { duration: 200 }
                            }
                        }

                        Text {
                            anchors.left: parent.left
                            anchors.bottom: parent.top
                            text: titleBlock.fmtTime(
                                titleBlock.scrubbing
                                    ? titleBlock.scrubSec
                                    : titleBlock.estimatedSec)
                            color: Bo.slate
                            font.pixelSize: 7
                            visible: titleBlock.scrubbing
                        }
                        Text {
                            anchors.right: parent.right
                            anchors.bottom: parent.top
                            text: "−" + titleBlock.fmtTime(
                                Math.max(0, qtobj.np_duration_sec
                                         - (titleBlock.scrubbing
                                            ? titleBlock.scrubSec
                                            : titleBlock.estimatedSec)))
                            color: Bo.slate
                            font.pixelSize: 7
                            visible: titleBlock.scrubbing
                        }

                        MouseArea {
                            anchors.fill: parent
                            anchors.topMargin: -8
                            anchors.bottomMargin: -8
                            onPressed: function(mouse) {
                                titleBlock.scrubbing = true
                                var frac = Math.min(1, Math.max(0,
                                    mouse.x / scrubTrack.width))
                                titleBlock.scrubSec = frac * qtobj.np_duration_sec
                            }
                            onPositionChanged: function(mouse) {
                                if (!pressed) return
                                var frac = Math.min(1, Math.max(0,
                                    mouse.x / scrubTrack.width))
                                titleBlock.scrubSec = frac * qtobj.np_duration_sec
                            }
                            onReleased: function(mouse) {
                                var frac = Math.min(1, Math.max(0,
                                    mouse.x / scrubTrack.width))
                                var targetSec = Math.floor(
                                    frac * qtobj.np_duration_sec)
                                titleBlock.estimatedSec = targetSec
                                titleBlock.scrubbing = false
                                qtobj.seek_to(targetSec)
                            }
                            onCanceled: {
                                titleBlock.scrubbing = false
                            }
                        }
                    }
                }
            }
        }

        // Idle hint when nothing is playing — keeps the bar from
        // looking broken/empty. Sits in the same center zone.
        Text {
            anchors.left: sinkChip.right
            anchors.leftMargin: 14
            anchors.right: controlCluster.left
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            visible: !titleBlock.visible
            text: "Nothing playing — pick a track to start"
            color: Bo.slate
            font.pixelSize: 10
            elide: Text.ElideRight
        }
    }
}
