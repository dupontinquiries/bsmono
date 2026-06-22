// qml/AnticipationDot.qml
//
// A small, quiet visual indicator that the controller is anticipating
// what the user will do next. Renders as a tiny dot whose opacity gently
// breathes when the system has a strong prediction queued. Mounted in
// the header strip next to the device name so it doesn't compete with
// any actual control. The brightness encodes the top prediction's
// confidence:
//
//   < 0.30  → hidden (just noise; no breathing)
//   0.30-0.55 → dim, slow breath
//   0.55-0.80 → mid, mid breath
//   > 0.80    → bright, faster breath
//
// Tapping reveals a brief tooltip with the top predicted action — a
// small piece of premium "the system is paying attention" feedback that
// stays out of the way until invoked.

import QtQuick 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    // Top prediction confidence in [0, 1]. Set by parent from
    // qtobj.prediction_json.
    property real strength: 0.0
    // Human label of the top predicted action ("Resume", "TrackEnded", …)
    property string topAction: ""
    // Tooltip visibility — tap toggles.
    property bool revealed: false

    implicitWidth: 22
    implicitHeight: 22
    visible: strength > 0.30

    // Breathing animation. Period scales with strength: stronger
    // predictions breathe faster, mirroring "the system is leaning in".
    SequentialAnimation on _breath {
        loops: Animation.Infinite
        running: root.strength > 0.30
        NumberAnimation { to: 1.0; duration: root._breathMs; easing.type: Easing.InOutSine }
        NumberAnimation { to: 0.35; duration: root._breathMs; easing.type: Easing.InOutSine }
    }
    property real _breath: 0.35
    property int _breathMs: {
        // 1800 ms at low strength → 900 ms at high strength.
        var s = Math.max(0, Math.min(1, root.strength))
        return Math.round(1800 - 900 * s)
    }

    // The dot itself.
    Rectangle {
        anchors.centerIn: parent
        width: 8 + 4 * root._breath
        height: width
        radius: width / 2
        color: root.accent
        opacity: 0.25 + 0.75 * root._breath * Math.min(1.0, root.strength * 1.2)
        Behavior on width { NumberAnimation { duration: 200 } }
    }
    // Faint outer ring, marking the strength band.
    Rectangle {
        anchors.centerIn: parent
        width: 14
        height: 14
        radius: 7
        color: "transparent"
        border.color: root.accent
        border.width: 1
        opacity: root.strength > 0.55 ? 0.4 : 0.0
        Behavior on opacity { NumberAnimation { duration: Bo.base } }
    }

    MouseArea {
        anchors.fill: parent
        anchors.margins: -8   // generous hit area
        onClicked: root.revealed = !root.revealed
    }

    // Tooltip — appears below the dot when tapped, fades on timer.
    Rectangle {
        id: tip
        anchors.top: parent.bottom
        anchors.topMargin: 4
        anchors.horizontalCenter: parent.horizontalCenter
        radius: 6
        height: tipText.implicitHeight + 8
        width: tipText.implicitWidth + 14
        color: Bo.surface
        border.color: root.accent
        border.width: 1
        visible: root.revealed && opacity > 0.01
        opacity: root.revealed ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Bo.quick } }
        z: 200

        Text {
            id: tipText
            anchors.centerIn: parent
            text: root.topAction.length > 0
                ? ("ANTICIPATING " + String(root.topAction || "").toUpperCase()
                   + "  ·  " + Math.round(root.strength * 100) + "%")
                : ""
            color: Bo.bone
            font.pixelSize: 9
            font.letterSpacing: 1
            font.weight: Font.DemiBold
        }
    }

    Timer {
        interval: 3500
        running: root.revealed
        onTriggered: root.revealed = false
    }
}
