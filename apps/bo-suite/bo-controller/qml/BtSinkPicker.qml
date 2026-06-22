// qml/BtSinkPicker.qml
//
// Modal dialog letting the user pick which paired Bluetooth devices
// should be the audio output for the currently active device.
//
// Two modes:
//   • Single (default): tap a row to select one speaker. Routes
//     through BluetoothDriver — single sink, low latency.
//   • Multi-room (toggle): check 2+ rows, tap "Combine speakers" to
//     create a PulseAudio combine-sink that fans audio across all
//     selected. Routes through MultiRoomDriver.
//
// Used when auto-pair-by-name didn't find a match — most commonly
// when the user's Stage is named "Beosound Stage" but their BT-paired
// speakers are named "Emerge01" / "Smokey" / etc.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    z: 400
    anchors.fill: parent
    visible: false

    // Multi-room mode toggle. When true, rows become checkboxes and
    // a "Combine N speakers" button appears at the bottom.
    property bool multiMode: false

    // Track which MACs are checked in multi-mode. Reset when the
    // picker opens or mode toggles.
    property var checkedMacs: []

    property var devices: {
        try {
            return JSON.parse(qtobj.bt_devices_json || "[]")
        } catch (e) { return [] }
    }

    property var activeMultiroomMacs: {
        try {
            var j = JSON.parse(qtobj.active_multiroom_json || "null")
            return (j && j.macs) ? j.macs : []
        } catch (e) { return [] }
    }

    function open() {
        if (activeMultiroomMacs.length > 0) {
            multiMode = true
            checkedMacs = activeMultiroomMacs.slice()
        } else {
            multiMode = false
            checkedMacs = []
        }
        visible = true
    }
    function close() { visible = false }

    function toggleChecked(mac) {
        var idx = checkedMacs.indexOf(mac)
        if (idx >= 0) {
            var copy = checkedMacs.slice()
            copy.splice(idx, 1)
            checkedMacs = copy
        } else {
            checkedMacs = checkedMacs.concat([mac])
        }
    }
    function isChecked(mac) { return checkedMacs.indexOf(mac) >= 0 }

    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.55
        MouseArea { anchors.fill: parent; onClicked: root.close() }
    }

    Rectangle {
        width: Math.min(parent.width - 40, 460)
        anchors.centerIn: parent
        height: Math.min(parent.height - 60, 560)
        radius: 12
        color: Bo.surface
        border.color: Bo.hairline; border.width: 1

        Column {
            anchors.fill: parent
            anchors.margins: Bo.gapMD
            spacing: Bo.gapSM

            Row {
                width: parent.width
                spacing: Bo.gapSM
                Text {
                    text: root.multiMode ? "Multi-room speakers" : "Pick Bluetooth speaker"
                    color: Bo.bone
                    font.pixelSize: 15
                    font.weight: Font.DemiBold
                    anchors.verticalCenter: parent.verticalCenter
                }
                Item { width: parent.width - 260; height: 1 }
                Rectangle {
                    width: 28; height: 28; radius: 14
                    anchors.verticalCenter: parent.verticalCenter
                    color: closeMa.pressed ? Bo.surfaceHi : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: "✕"; color: Bo.bone; font.pixelSize: 12
                    }
                    MouseArea {
                        id: closeMa
                        anchors.fill: parent
                        onClicked: root.close()
                    }
                }
            }

            Rectangle {
                width: parent.width
                height: 36
                radius: 8
                color: Bo.graphite
                border.color: Bo.hairline; border.width: 1
                Row {
                    anchors.fill: parent
                    spacing: 0
                    Rectangle {
                        width: parent.width / 2
                        height: parent.height
                        radius: 8
                        color: !root.multiMode
                            ? Bo.mix(Bo.surface, Bo.teal, 0.30)
                            : "transparent"
                        Text {
                            anchors.centerIn: parent
                            text: "Single speaker"
                            color: !root.multiMode ? Bo.bone : Bo.slate
                            font.pixelSize: 10
                            font.weight: !root.multiMode ? Font.DemiBold : Font.Normal
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: { root.multiMode = false; root.checkedMacs = [] }
                        }
                    }
                    Rectangle {
                        width: parent.width / 2
                        height: parent.height
                        radius: 8
                        color: root.multiMode
                            ? Bo.mix(Bo.surface, Bo.accent, 0.30)
                            : "transparent"
                        Text {
                            anchors.centerIn: parent
                            text: "Multi-room"
                            color: root.multiMode ? Bo.bone : Bo.slate
                            font.pixelSize: 10
                            font.weight: root.multiMode ? Font.DemiBold : Font.Normal
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: {
                                root.multiMode = true
                                if (qtobj.active_bt_mac && qtobj.active_bt_mac.length > 0)
                                    root.checkedMacs = [qtobj.active_bt_mac]
                            }
                        }
                    }
                }
            }

            Text {
                width: parent.width
                text: root.multiMode
                    ? "Check 2 or more speakers to combine them into a single audio room. PulseAudio fans the audio across all selected sinks with clock sync."
                    : ("Route audio for " + (qtobj.product_name || "this device")
                       + " through a paired Bluetooth speaker.")
                color: Bo.slate
                font.pixelSize: 10
                wrapMode: Text.Wrap
            }
            Rectangle { width: parent.width; height: 1; color: Bo.hairline }

            // ── BT system-off prompt ────────────────────────────
            //
            // When the user opens this picker but the system Bluetooth
            // adapter is OFF, the device list will be useless — no
            // sinks can connect. Surface a clear notice with an
            // optional "Turn on Bluetooth" action.
            //
            // The action button is gated behind a 1-second
            // confirm-unlock countdown to prevent accidental taps:
            // turning on the system BT adapter is a meaningful
            // state change that the user might not have meant. The
            // button shows a thin ring that fills over 1 second; only
            // then does it become tappable.
            //
            // On app shutdown, BtRespect.should_restore_power_off()
            // tells the controller to power BT back off, so the
            // adapter state isn't permanently altered.
            Rectangle {
                visible: !qtobj.bt_system_powered
                width: parent.width
                height: btOffColumn.implicitHeight + 24
                radius: 8
                color: Bo.mix(Bo.surface, Bo.rust, 0.18)
                border.color: Bo.rust
                border.width: 1

                Column {
                    id: btOffColumn
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: 12
                    anchors.rightMargin: 12
                    spacing: 6
                    Text {
                        text: "System Bluetooth is OFF"
                        color: Bo.bone
                        font.pixelSize: 11
                        font.weight: Font.DemiBold
                    }
                    Text {
                        width: parent.width
                        text: "Turn it on to connect speakers. The controller will "
                            + "power it back off when you close the app, "
                            + "restoring your original state."
                        color: Bo.slate
                        font.pixelSize: 9
                        wrapMode: Text.Wrap
                    }
                    // Confirm-unlock button.
                    Rectangle {
                        id: powerOnBtn
                        property real progress: 0.0  // 0..1 ring fill
                        property bool unlocked: progress >= 1.0
                        anchors.right: parent.right
                        width: 168; height: 32
                        radius: 16
                        color: powerOnMa.pressed && unlocked
                            ? Bo.mix(Bo.surface, Bo.rust, 0.60)
                            : Bo.mix(Bo.surface, Bo.rust, 0.30)
                        border.color: Bo.rust; border.width: 1
                        opacity: unlocked ? 1.0 : 0.65

                        // Progress ring overlay — drawn as a thin
                        // arc on top of the button's border. Fills
                        // counterclockwise over 1 second on press.
                        Canvas {
                            id: ringCanvas
                            anchors.fill: parent
                            antialiasing: true
                            onPaint: {
                                var ctx = getContext("2d"); ctx.reset()
                                var w = width, h = height
                                ctx.strokeStyle = Bo.rust
                                ctx.lineWidth = 2
                                ctx.beginPath()
                                // Stadium-shaped progress border.
                                // Approximate by drawing along the
                                // top edge proportional to progress.
                                var p = powerOnBtn.progress
                                ctx.moveTo(2, 2)
                                ctx.lineTo(2 + (w - 4) * p, 2)
                                ctx.stroke()
                            }
                        }
                        Connections {
                            target: powerOnBtn
                            function onProgressChanged() {
                                ringCanvas.requestPaint()
                            }
                        }

                        // Animation that fills the ring while the
                        // button is held. Reset when released.
                        NumberAnimation {
                            id: unlockAnim
                            target: powerOnBtn
                            property: "progress"
                            duration: 1000
                            from: 0
                            to: 1
                        }

                        Row {
                            anchors.centerIn: parent
                            spacing: 6
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: powerOnBtn.unlocked ? "⏻" : "⏻"
                                color: Bo.rust
                                font.pixelSize: 13
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: powerOnBtn.unlocked
                                    ? "Tap to confirm"
                                    : "Hold to turn on"
                                color: Bo.bone
                                font.pixelSize: 10
                                font.weight: Font.DemiBold
                            }
                        }
                        MouseArea {
                            id: powerOnMa
                            anchors.fill: parent
                            onPressed: {
                                if (!powerOnBtn.unlocked) {
                                    unlockAnim.start()
                                }
                            }
                            onReleased: {
                                if (!powerOnBtn.unlocked) {
                                    // Released before the hold finished
                                    // — reset the progress.
                                    unlockAnim.stop()
                                    powerOnBtn.progress = 0
                                } else {
                                    // Confirmed — actually power BT on.
                                    qtobj.set_bt_system_powered(true)
                                    // Reset so a subsequent tap also
                                    // requires the hold again.
                                    powerOnBtn.progress = 0
                                }
                            }
                            onCanceled: {
                                unlockAnim.stop()
                                powerOnBtn.progress = 0
                            }
                        }
                    }
                }
            }

            Text {
                width: parent.width
                text: (qtobj.current_driver_kind === "multiroom")
                    ? ("Currently routing through: " + qtobj.current_driver_label)
                    : ((qtobj.current_driver_kind === "bluetooth")
                        ? ("Currently routing through: "
                            + (qtobj.active_bt_name || qtobj.active_bt_mac))
                        : "Currently: no BT route bound (audio plays locally)")
                color: (qtobj.current_driver_kind === "bluetooth"
                        || qtobj.current_driver_kind === "multiroom")
                    ? Bo.teal : Bo.slate
                font.pixelSize: 11
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            ScrollView {
                width: parent.width
                height: parent.height - 280
                clip: true

                Column {
                    width: root.width - 80
                    spacing: 4

                    Rectangle {
                        visible: !root.multiMode
                        width: parent.width; height: 44; radius: 8
                        color: noneMa.pressed
                            ? Bo.mix(Bo.surface, Bo.rust, 0.30)
                            : "transparent"
                        border.color: Bo.hairline; border.width: 1
                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: 12
                            anchors.rightMargin: 12
                            spacing: Bo.gapSM
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "✕"; color: Bo.rust; font.pixelSize: 14
                            }
                            Column {
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 1
                                Text {
                                    text: "None (play locally)"
                                    color: Bo.bone
                                    font.pixelSize: 12
                                    font.weight: Font.DemiBold
                                }
                                Text {
                                    text: "Audio goes to controller speakers"
                                    color: Bo.slate; font.pixelSize: 9
                                }
                            }
                        }
                        MouseArea {
                            id: noneMa
                            anchors.fill: parent
                            onClicked: {
                                qtobj.clear_multiroom()
                                qtobj.set_active_bt_mac("", "")
                                root.close()
                            }
                        }
                    }

                    Text {
                        visible: root.devices.length === 0
                        width: parent.width
                        text: "No paired BT devices yet. Go to the Connect page to discover and pair speakers."
                        color: Bo.slate
                        font.pixelSize: 10
                        wrapMode: Text.Wrap
                        topPadding: 12
                    }

                    Repeater {
                        model: root.devices
                        delegate: Rectangle {
                            width: parent.width; height: 56; radius: 8
                            property bool isSingleActive: !root.multiMode
                                && qtobj.active_bt_mac === modelData.mac
                            property bool isCheckedHere: root.multiMode
                                && root.isChecked(modelData.mac)
                            property bool active: isSingleActive || isCheckedHere
                            color: itemMa.pressed
                                ? Bo.mix(Bo.surface,
                                    root.multiMode ? Bo.accent : Bo.teal, 0.30)
                                : (active
                                    ? Bo.mix(Bo.surface,
                                        root.multiMode ? Bo.accent : Bo.teal, 0.20)
                                    : "transparent")
                            border.color: active
                                ? (root.multiMode ? Bo.accent : Bo.teal)
                                : Bo.hairline
                            border.width: 1
                            Row {
                                anchors.fill: parent
                                anchors.leftMargin: 12
                                anchors.rightMargin: 12
                                spacing: Bo.gapSM
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 20; height: 20
                                    radius: root.multiMode ? 4 : 10
                                    color: active
                                        ? (root.multiMode ? Bo.accent : Bo.teal)
                                        : "transparent"
                                    border.color: root.multiMode
                                        ? Bo.accent : Bo.teal
                                    border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        visible: active
                                        text: "✓"
                                        // Was Bo.charcoal (doesn't exist
                                        // in the token set). Graphite is
                                        // the deepest tone in the palette
                                        // — gives the check-mark good
                                        // contrast on the bronze/teal
                                        // active-pill backgrounds.
                                        color: Bo.graphite
                                        font.pixelSize: 12
                                        font.weight: Font.DemiBold
                                    }
                                }
                                Column {
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 1
                                    Text {
                                        text: modelData.name || "(unnamed)"
                                        color: Bo.bone
                                        font.pixelSize: 12
                                        font.weight: Font.DemiBold
                                    }
                                    Text {
                                        text: modelData.mac + " · " + modelData.status
                                        color: Bo.slate
                                        font.pixelSize: 9
                                    }
                                }
                            }
                            MouseArea {
                                id: itemMa
                                anchors.fill: parent
                                onClicked: {
                                    if (root.multiMode) {
                                        root.toggleChecked(modelData.mac)
                                    } else {
                                        qtobj.clear_multiroom()
                                        qtobj.set_active_bt_mac(modelData.mac,
                                                                modelData.name || "")
                                        root.close()
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Row {
                visible: root.multiMode
                width: parent.width
                spacing: Bo.gapSM

                Rectangle {
                    width: parent.width - 90 - Bo.gapSM
                    height: 40
                    radius: 20
                    enabled: root.checkedMacs.length >= 2
                    opacity: enabled ? 1.0 : 0.4
                    color: combineMa.pressed
                        ? Bo.surfaceHi
                        : Bo.mix(Bo.surface, Bo.accent, 0.45)
                    border.color: Bo.accent; border.width: 1
                    Row {
                        anchors.centerIn: parent
                        spacing: 6
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "🔊+"
                            color: Bo.accent
                            font.pixelSize: 14
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.checkedMacs.length < 2
                                ? "Pick 2+ speakers"
                                : ("Combine " + root.checkedMacs.length + " speakers")
                            color: Bo.bone
                            font.pixelSize: 11
                            font.weight: Font.DemiBold
                        }
                    }
                    MouseArea {
                        id: combineMa
                        anchors.fill: parent
                        enabled: root.checkedMacs.length >= 2
                        onClicked: {
                            var names = []
                            for (var i = 0; i < root.checkedMacs.length; ++i) {
                                var mac = root.checkedMacs[i]
                                var nm = mac
                                for (var j = 0; j < root.devices.length; ++j) {
                                    if (root.devices[j].mac === mac) {
                                        nm = root.devices[j].name || mac
                                        break
                                    }
                                }
                                names.push(nm)
                            }
                            qtobj.set_multiroom_macs(
                                JSON.stringify(root.checkedMacs),
                                JSON.stringify(names))
                            root.close()
                        }
                    }
                }

                Rectangle {
                    width: 90; height: 40; radius: 20
                    color: clearMa.pressed ? Bo.surfaceHi : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    visible: qtobj.current_driver_kind === "multiroom"
                    Text {
                        anchors.centerIn: parent
                        text: "Disband"
                        color: Bo.rust
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                    }
                    MouseArea {
                        id: clearMa
                        anchors.fill: parent
                        onClicked: {
                            qtobj.clear_multiroom()
                            root.close()
                        }
                    }
                }
            }
        }
    }
}
