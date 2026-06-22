// qml/SafetyToast.qml
//
// A slide-down notification that appears at the top of the app when the
// volume safety system clamps a manual change above the unsafe threshold
// (≈95 dB). Offers the user a clear choice:
//   • DISMISS  — accept the cap; no override.
//   • UNLOCK   — accept the warning and unlock high-volume mode for the
//                rest of this device session, then re-apply the requested
//                value.
//
// Auto-dismisses after 8 seconds if untouched.

import QtQuick 2.15
import "."

Item {
    id: root
    anchors.top: parent.top
    anchors.left: parent.left
    anchors.right: parent.right
    height: card.implicitHeight + Bo.gapMD * 2
    visible: opacity > 0.01
    opacity: qtobj.volume_was_clamped ? 1.0 : 0.0
    Behavior on opacity { NumberAnimation { duration: Bo.base; easing.type: Easing.OutCubic } }
    y: qtobj.volume_was_clamped ? 0 : -height
    Behavior on y { NumberAnimation { duration: Bo.base; easing.type: Easing.OutCubic } }
    z: 500

    property color accent: "#C8A04B"

    Rectangle {
        id: card
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        radius: 14
        color: Bo.surface
        border.color: root.accent
        border.width: 1

        Column {
            anchors.fill: parent
            anchors.margins: Bo.gapMD
            spacing: 8

            Row {
                spacing: 8
                width: parent.width
                Rectangle {
                    width: 24; height: 24; radius: 12
                    anchors.verticalCenter: parent.verticalCenter
                    color: root.accent
                    Text {
                        anchors.centerIn: parent
                        text: "!"
                        color: Bo.graphite
                        font.pixelSize: 14
                        font.weight: Font.Bold
                    }
                }
                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 30
                    Text {
                        text: "VOLUME CAPPED FOR SAFETY"
                        color: Bo.bone
                        font.pixelSize: Bo.microSize
                        font.letterSpacing: Bo.labelTracking
                        font.weight: Font.DemiBold
                    }
                    Text {
                        width: parent.width
                        text: {
                            var r = qtobj.volume_pending_request
                            var c = qtobj.volume
                            var db = qtobj.active_db_estimate
                            return "Requested " + r + " (above ≈95 dB threshold) — "
                                + "set to " + c + " (≈" + Math.round(db) + " dB)."
                        }
                        color: Bo.silver
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }
                }
            }

            // Action row
            Row {
                spacing: Bo.gapSM
                anchors.right: parent.right

                Rectangle {
                    width: dismissText.implicitWidth + 28
                    height: 32; radius: 16
                    color: dma.pressed ? Bo.surfaceHi : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        id: dismissText
                        anchors.centerIn: parent
                        text: "DISMISS"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1
                    }
                    MouseArea {
                        id: dma
                        anchors.fill: parent
                        onClicked: qtobj.dismiss_clamp_warning()
                    }
                }
                Rectangle {
                    width: unlockText.implicitWidth + 28
                    height: 32; radius: 16
                    color: uma.pressed ? Bo.shade(root.accent) : root.accent
                    Text {
                        id: unlockText
                        anchors.centerIn: parent
                        text: "UNLOCK & RETRY"
                        color: Bo.graphite
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1
                    }
                    MouseArea {
                        id: uma
                        anchors.fill: parent
                        onClicked: {
                            var pending = qtobj.volume_pending_request
                            qtobj.enable_high_volume()
                            qtobj.dismiss_clamp_warning()
                            if (pending > 0) qtobj.set_volume_api(pending)
                        }
                    }
                }
            }
        }
    }

    // Auto-dismiss after 8 seconds.
    Timer {
        interval: 8000
        running: qtobj.volume_was_clamped
        onTriggered: qtobj.dismiss_clamp_warning()
    }
}
