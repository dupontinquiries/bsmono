import QtQuick 2.15

// A field of slowly-drifting "dew" sparkles whose density scales with
// the humidity reading. Decorative; sits in front of the background
// but behind page content.
//
// At 0% humidity you see nothing; at 100% you see ~25 drifting dots.
// All dots are independent so the effect feels organic, not periodic.
Item {
    id: root
    property real humidityPct: 0.0
    property color sparkleColor: "#a8e6f0"

    readonly property int desiredCount: Math.round(humidityPct / 4.0)  // 0..25

    Repeater {
        model: 25
        delegate: Rectangle {
            width: 3 + Math.random() * 3
            height: width
            radius: width / 2
            color: root.sparkleColor
            opacity: 0
            visible: index < root.desiredCount

            x: Math.random() * root.width
            y: Math.random() * root.height

            SequentialAnimation on opacity {
                loops: Animation.Infinite
                running: visible
                PauseAnimation { duration: Math.random() * 4000 }
                NumberAnimation { to: 0.7; duration: 1500; easing.type: Easing.InOutSine }
                NumberAnimation { to: 0.0; duration: 2500; easing.type: Easing.InOutSine }
            }

            // Slow vertical drift (gravity-ish)
            NumberAnimation on y {
                running: visible
                loops: Animation.Infinite
                from: -10
                to: root.height + 10
                duration: 12000 + Math.random() * 8000
            }
        }
    }
}
