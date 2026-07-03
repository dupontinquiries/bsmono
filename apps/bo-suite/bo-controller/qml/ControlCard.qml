// qml/ControlCard.qml
//
// Labeled card container. Nests content as default children. Refinements:
//   • a subtle entrance (fades + rises into place) so pages assemble
//     with a sense of composure rather than appearing all at once
//   • when `active`, the border holds the accent and gains a barely-there
//     breathing glow — the card "listens"
//   • a hairline label with B&O's wide uppercase tracking

import QtQuick 2.15
import "."

Rectangle {
    id: root
    property string label: ""
    // Back-compat alias. Some older copies of SettingsPage used
    // `title:` which is what Qt's stock ControlCard uses. We map it
    // onto `label` so either name works without QML throwing
    // "non-existent property" at load time.
    property alias title: root.label
    property bool active: false
    property color accent: "#C8A04B"
    property int entranceDelay: 0

    default property alias cardContent: holder.data

    radius: Bo.cardRadius
    color: Bo.surface
    border.color: active ? accent : Bo.hairline
    border.width: 1

    Behavior on border.color { ColorAnimation { duration: Bo.base } }

    implicitHeight: Bo.gapMD + labelText.height + Bo.gapSM + holder.height + Bo.gapMD
    height: implicitHeight
    Behavior on implicitHeight {
        NumberAnimation { duration: Bo.base
            easing.type: Easing.Bezier; easing.bezierCurve: Bo.settle }
    }

    // Breathing glow when active
    Rectangle {
        anchors.fill: parent
        radius: parent.radius
        color: "transparent"
        border.color: root.accent
        border.width: 1
        visible: root.active
        opacity: 0
        SequentialAnimation on opacity {
            running: root.active
            loops: Animation.Infinite
            NumberAnimation { to: 0.35; duration: 1800; easing.type: Easing.InOutSine }
            NumberAnimation { to: 0.0;  duration: 1800; easing.type: Easing.InOutSine }
        }
    }

    Text {
        id: labelText
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.leftMargin: Bo.gapMD
        anchors.topMargin: Bo.gapMD
        text: root.label
        color: Bo.silver
        font.pixelSize: Bo.microSize
        font.letterSpacing: Bo.labelTracking
        font.weight: Font.DemiBold
    }

    Item {
        id: holder
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: labelText.bottom
        anchors.leftMargin: Bo.gapMD
        anchors.rightMargin: Bo.gapMD
        anchors.topMargin: Bo.gapSM
        // childrenRect.height covers plain Item/Column/Rectangle content;
        // a ColumnLayout reports its size via implicitHeight instead (and
        // childrenRect can read 0 for it), so take the max of both. This
        // keeps the card tall enough for either content style and prevents
        // the next card in the settings list from overlapping it.
        height: Math.max(childrenRect.height,
                         children.length > 0 ? (children[0].implicitHeight || 0) : 0)
    }

    // Entrance
    opacity: 0
    transform: Translate { id: rise; y: 14 }
    Component.onCompleted: entrance.start()
    SequentialAnimation {
        id: entrance
        PauseAnimation { duration: root.entranceDelay }
        ParallelAnimation {
            NumberAnimation { target: root; property: "opacity"; to: 1; duration: Bo.slow }
            NumberAnimation { target: rise; property: "y"; to: 0; duration: Bo.slow
                easing.type: Easing.Bezier; easing.bezierCurve: Bo.settle }
        }
    }
}
