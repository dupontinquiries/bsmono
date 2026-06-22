import QtQuick 2.15
import "Theme.js" as Theme

// A small dot that breathes (slow opacity oscillation) — used in status
// pills like "Sensors locked" and "Live data". Tunable via:
//   color:   the dot's hue
//   running: pause when off-screen to save battery
//   period:  full breath cycle in ms
Rectangle {
    id: root
    width: 10
    height: 10
    radius: width / 2
    color: Theme.success
    property bool running: visible
    property int period: Theme.durBreathing

    SequentialAnimation on opacity {
        loops: Animation.Infinite
        running: root.running
        alwaysRunToEnd: false
        NumberAnimation { from: 1.0; to: 0.45; duration: root.period / 2; easing.type: Easing.InOutSine }
        NumberAnimation { from: 0.45; to: 1.0; duration: root.period / 2; easing.type: Easing.InOutSine }
    }
}
