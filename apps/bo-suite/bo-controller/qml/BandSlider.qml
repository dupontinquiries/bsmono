// qml/BandSlider.qml
//
// One vertical fader for the stream mixer. Four of these make up a
// ChannelColumn (Low / Mid / High / Volume).
//
// Layout (top → bottom):
//   • Band name ("LOW", "MID", "HIGH", "VOL")
//   • Locked frequency label ("250 Hz") — empty for VOL
//   • Role hint italic ("chest body") — keeps the principle visible
//   • Vertical slider track
//   • Current value ("+3.5 dB" or "85%")
//
// The slider for EQ bands snaps to 0 when dragged near center —
// a small detent that makes "neutral" easy to hit by feel.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: bs
    property string bandLabel: ""
    property string freqLabel: ""
    property string roleHint:  ""
    property string valueLabel: ""
    property color tint: "#C8A04B"
    property bool isVolume: false
    property real value: 0
    /// Emitted when the user drags the slider. The value is dB for
    /// EQ bands or linear 0..2 for volume.
    signal sliderMoved(real v)

    // The qt_property → slider value direction is a one-way binding:
    // when the property changes (e.g. via reset_mixer), we update
    // the slider. The reverse direction goes through onSliderMoved.
    onValueChanged: {
        if (!sliderControl.pressed && Math.abs(sliderControl.value - value) > 0.001) {
            sliderControl.value = value
        }
    }
    Component.onCompleted: sliderControl.value = value

    Column {
        anchors.fill: parent
        anchors.margins: 4
        spacing: 2

        // Band name
        Text {
            text: bs.bandLabel
            color: Bo.silver
            font.pixelSize: 10
            font.weight: Font.DemiBold
            font.letterSpacing: 1.0
            anchors.horizontalCenter: parent.horizontalCenter
        }
        // Locked frequency
        Text {
            visible: bs.freqLabel.length > 0
            text: bs.freqLabel
            color: bs.tint
            font.pixelSize: 11
            font.weight: Font.Bold
            anchors.horizontalCenter: parent.horizontalCenter
        }
        Text {
            visible: bs.roleHint.length > 0
            text: bs.roleHint
            color: Bo.slate
            font.pixelSize: 8
            font.italic: true
            anchors.horizontalCenter: parent.horizontalCenter
        }
        // Slider takes the remaining vertical space.
        Slider {
            id: sliderControl
            anchors.horizontalCenter: parent.horizontalCenter
            orientation: Qt.Vertical
            width: 40
            height: parent.height - 80
            from: bs.isVolume ? 0.0 : -12.0
            to:   bs.isVolume ? 2.0  :  12.0
            stepSize: bs.isVolume ? 0.01 : 0.5
            value: bs.value

            onMoved: {
                var v = value
                // Detent at 0 dB for EQ bands — snap when within
                // half a step of neutral. Volume gets no detent
                // since the user often wants subtle gain changes.
                if (!bs.isVolume && Math.abs(v) < 0.4) {
                    v = 0.0
                    if (Math.abs(value - 0.0) > 0.001) {
                        // Realize the snap visually too.
                        value = 0.0
                    }
                }
                bs.sliderMoved(v)
            }

            background: Rectangle {
                x: sliderControl.leftPadding + (sliderControl.availableWidth - width) / 2
                y: sliderControl.topPadding
                width: 4; height: sliderControl.availableHeight
                radius: 2
                color: Bo.graphite
                border.color: Bo.hairline; border.width: 1

                // Center-line marker for EQ bands (0 dB indicator)
                Rectangle {
                    visible: !bs.isVolume
                    width: 12; height: 1
                    color: Bo.silver
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.verticalCenter: parent.verticalCenter
                }

                // Filled portion — for EQ bands, filled from center;
                // for volume, filled from bottom up.
                Rectangle {
                    visible: !bs.isVolume
                    width: parent.width
                    radius: 2
                    color: bs.tint
                    anchors.horizontalCenter: parent.horizontalCenter
                    // Height grows from the center line outward.
                    property real normalized: sliderControl.visualPosition
                    height: Math.abs(parent.height * (normalized - 0.5))
                    y: normalized < 0.5
                        ? parent.height * normalized
                        : parent.height / 2
                }
                Rectangle {
                    visible: bs.isVolume
                    width: parent.width
                    radius: 2
                    color: bs.tint
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: parent.bottom
                    height: parent.height * (1.0 - sliderControl.visualPosition)
                }
            }

            handle: Rectangle {
                x: sliderControl.leftPadding
                  + sliderControl.availableWidth / 2 - width / 2
                y: sliderControl.topPadding
                  + sliderControl.visualPosition * (sliderControl.availableHeight - height)
                width: 28; height: 16; radius: 4
                color: sliderControl.pressed ? bs.tint : Bo.bone
                border.color: bs.tint; border.width: 1
            }
        }
        Text {
            text: bs.valueLabel
            color: Bo.bone
            font.pixelSize: 10
            font.weight: Font.DemiBold
            anchors.horizontalCenter: parent.horizontalCenter
        }
    }
}
