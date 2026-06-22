// qml/PageDots.qml
//
// Page indicator. Instead of generic round dots, B&O-flavored dashes:
// each page is a short hairline; the active one elongates and takes the
// accent color, sliding with the `settle` curve. Quiet, precise, legible
// at a glance.

import QtQuick 2.15
import "."

Row {
    id: root
    property int count: 3
    property int index: 0
    property color accent: "#C8A04B"
    spacing: 8

    Repeater {
        model: root.count
        delegate: Rectangle {
            height: 3
            radius: 1.5
            width: index === root.index ? 22 : 8
            color: index === root.index ? root.accent : Bo.hairlineHi
            anchors.verticalCenter: parent.verticalCenter

            Behavior on width {
                NumberAnimation {
                    duration: Bo.base
                    easing.type: Easing.Bezier
                    easing.bezierCurve: Bo.settle
                }
            }
            Behavior on color { ColorAnimation { duration: Bo.base } }
        }
    }
}
