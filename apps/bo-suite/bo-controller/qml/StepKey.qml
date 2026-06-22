// qml/StepKey.qml
//
// A press key (−, +, MUTE) with B&O micro-interactions:
//   • spring-scale on press (slight squash, confident release)
//   • a soft accent ripple that blooms from the touch point
//   • a brief accent flash when it performs its action
// Round by default, pill-shaped when `wide`.

import QtQuick 2.15
import "."

Item {
    id: root
    property string label: ""
    property bool wide: false
    property bool small: false
    property bool active: false
    property color accent: "#C8A04B"
    signal triggered()

    // Sizing:
    //   • default 60×60 round key (digits, glyphs, short labels)
    //   • wide=true:  pill 100×60 (legacy two-char fits)
    //   • small=true: pill that grows to fit label text with padding,
    //     so labels like "RUN CHECKS", "UNINSTALL", "REINSTALL" don't clip.
    width: small  ? Math.max(80, labelText.implicitWidth + 28)
         : wide   ? 100
         : 60
    height: small ? 36 : 60

    Rectangle {
        id: body
        anchors.fill: parent
        radius: Bo.keyRadius
        color: root.active ? root.accent
                           : (mouse.pressed ? Bo.surfaceHi : Bo.surface)
        border.color: root.active ? root.accent
                                  : (mouse.pressed ? Bo.hairlineHi : Bo.hairline)
        border.width: 1
        clip: true

        Behavior on color { ColorAnimation { duration: Bo.quick } }
        Behavior on border.color { ColorAnimation { duration: Bo.quick } }

        // Spring press
        transform: Scale {
            id: sc
            origin.x: body.width / 2; origin.y: body.height / 2
            xScale: mouse.pressed ? 0.91 : 1.0
            yScale: mouse.pressed ? 0.91 : 1.0
            Behavior on xScale { NumberAnimation { duration: Bo.quick
                easing.type: Easing.Bezier; easing.bezierCurve: Bo.spring } }
            Behavior on yScale { NumberAnimation { duration: Bo.quick
                easing.type: Easing.Bezier; easing.bezierCurve: Bo.spring } }
        }

        // Ripple
        Rectangle {
            id: ripple
            property real cx: 0
            property real cy: 0
            x: cx - width / 2
            y: cy - height / 2
            width: 0; height: 0
            radius: width / 2
            color: root.active ? Bo.graphite : root.accent
            opacity: 0
        }

        // Action flash overlay
        Rectangle {
            id: flash
            anchors.fill: parent
            radius: parent.radius
            color: root.accent
            opacity: 0
        }

        Text {
            id: labelText
            anchors.centerIn: parent
            text: root.label
            color: root.active ? Bo.graphite : Bo.bone
            font.pixelSize: root.small ? Bo.captionSize : 26
            font.weight: root.small ? Font.DemiBold : Font.Light
            font.letterSpacing: root.small ? Bo.labelTracking : 0
        }
    }

    SequentialAnimation {
        id: rippleAnim
        ParallelAnimation {
            NumberAnimation { target: ripple; property: "width";  from: 0; to: body.width * 2.2; duration: Bo.slow }
            NumberAnimation { target: ripple; property: "height"; from: 0; to: body.width * 2.2; duration: Bo.slow }
            NumberAnimation { target: ripple; property: "opacity"; from: 0.22; to: 0; duration: Bo.slow }
        }
    }
    SequentialAnimation {
        id: flashAnim
        NumberAnimation { target: flash; property: "opacity"; to: 0.30; duration: Bo.instant }
        NumberAnimation { target: flash; property: "opacity"; to: 0; duration: Bo.base
                          easing.type: Easing.OutCubic }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        onPressed: function(m) {
            ripple.cx = m.x; ripple.cy = m.y
            rippleAnim.restart()
        }
        onClicked: {
            if (!root.active) flashAnim.restart()
            root.triggered()
        }
    }
}
