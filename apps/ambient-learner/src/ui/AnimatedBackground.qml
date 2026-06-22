import QtQuick 2.15
import "Theme.js" as Theme

// Full-window background that smoothly transitions between mood
// gradients (season + day phase) and adds subtle parallax shifts
// when the device is tilted.
//
// Sits behind everything else with z = -1.
//
// Inputs (set from the parent — usually `window`):
//   season:       "winter" | "spring" | "summer" | "autumn"
//   phase:        "night" | "dawn" | "morning" | "afternoon" | "dusk" | "evening"
//   tiltX, tiltY: -1..1 from accelerometer
//   lightNorm:    0..1 from ambient light sensor
//   pressureTrend: hPa per 30 min — drives a faint storm-cloud overlay
Item {
    id: root
    z: -1
    anchors.fill: parent

    property string season: "winter"
    property string phase: "evening"
    property real tiltX: 0
    property real tiltY: 0
    property real lightNorm: 0.0
    property real pressureTrend: 0.0

    // Cache gradient stops; QML can't animate string lists directly,
    // so we animate the `gradient` Rectangle's *colors* via a Behavior.
    readonly property var stops: Theme.moodGradientStops(season, phase)
    readonly property color stop0: stops[0]
    readonly property color stop1: stops[1]

    // Base gradient panel
    Rectangle {
        id: gradientPanel
        anchors.fill: parent
        // Tilt-driven offsets push the gradient origin around for parallax.
        transform: Translate {
            x: root.tiltX * 12
            y: root.tiltY * 12
        }
        gradient: Gradient {
            GradientStop {
                position: 0.0
                color: root.stop0
                Behavior on color { ColorAnimation { duration: Theme.durSlow } }
            }
            GradientStop {
                position: 1.0
                color: root.stop1
                Behavior on color { ColorAnimation { duration: Theme.durSlow } }
            }
        }
    }

    // Seasonal tint overlay (very subtle).
    Rectangle {
        anchors.fill: parent
        color: Theme.moodTint(root.season)
        Behavior on color { ColorAnimation { duration: Theme.durSlow } }
    }

    // Light-level vignette.
    // Bright environment → less vignette; dark room → more vignette.
    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.35 * (1.0 - root.lightNorm)
        Behavior on opacity { NumberAnimation { duration: Theme.durSlow } }
    }

    // Slow drifting "storm clouds" overlay, driven by pressure trend.
    // Falling pressure (negative trend) makes it more visible and tints
    // the clouds purple-grey; rising pressure fades them out entirely.
    Item {
        id: stormLayer
        anchors.fill: parent
        opacity: Math.max(0.0, -root.pressureTrend / 4.0)
        Behavior on opacity { NumberAnimation { duration: Theme.durStandard } }

        Repeater {
            model: 3
            delegate: Rectangle {
                width: parent.width * 1.6
                height: parent.height * 0.7
                radius: height / 2
                color: Theme.pressureMoodColor(root.pressureTrend)
                opacity: 0.18

                x: -width * 0.3
                y: parent.height * (0.1 + index * 0.25)

                NumberAnimation on x {
                    from: -parent.width
                    to: parent.width * 0.3
                    duration: Theme.durStorm + index * 1500
                    loops: Animation.Infinite
                }
            }
        }
    }
}
