// qml/ChannelColumn.qml
//
// One column of the stream mixer — either the SPEAKER or
// INSTRUMENTAL channel. Renders 4 vertical sliders side by side:
//
//   Low EQ  · Mid EQ  · High EQ  · Volume
//
// Each EQ slider's range is ±12 dB; volume is 0% to 200%. The locked
// frequency for each band is shown above the slider as small labels.
// This is the visual proof that the bands aren't random, they're
// chosen for the channel's musical role.
//
// The header bar carries a "↺" button that neutralizes THIS channel
// only (all four sliders → defaults), separate from the global
// "Reset all" on MixerPage. Per-channel reset is the common need —
// the user adjusts one column, decides it was a bad call, wants to
// start over WITHOUT losing their work on the other column.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: column
    /// "SPEAKER" or "INSTRUMENTAL" — the big header.
    property string channelLabel: ""
    /// Subtitle clarifying the M/S origin and the typical content.
    property string channelSubtitle: ""
    /// Tint color used for the header bar + active slider track.
    /// Defaults to Bo.accent for safety but every real usage
    /// supplies the channel's actual color (mauve / teal).
    property color tint: Bo.accent
    /// "spk" or "ins" — passed back to set_mixer_value().
    property string channelKey: "spk"

    /// Band frequencies (locked — UI shows them, never edits).
    property real lowHz:  0
    property real midHz:  0
    property real highHz: 0
    /// Optional role hints rendered under each band's frequency
    /// (e.g. "vocal presence" under "1.5 kHz"). Keeps the principle
    /// visible in the UI even before the user touches anything.
    property string lowRoleHint:  ""
    property string midRoleHint:  ""
    property string highRoleHint: ""

    /// Current values (bound to qtobj.mix_*_*). EQ values are dB,
    /// volume is linear.
    property real lowValue:  0
    property real midValue:  0
    property real highValue: 0
    property real volValue:  1.0

    /// Formatter functions passed down from MixerPage so we don't
    /// duplicate them.
    property var hzLabel:  function(hz)  { return hz + " Hz" }
    property var dbLabel:  function(db)  { return db + " dB" }
    property var volLabel: function(v)   { return Math.round(v * 100) + "%" }

    /// Is this channel currently doing anything? Used to subtly
    /// highlight the header when sliders are off-neutral, so the
    /// user can see at a glance which side is active.
    readonly property bool isActive:
        Math.abs(lowValue)  > 0.1
     || Math.abs(midValue)  > 0.1
     || Math.abs(highValue) > 0.1
     || Math.abs(volValue - 1.0) > 0.01

    Rectangle {
        anchors.fill: parent
        color: Bo.surface
        radius: 8
        border.color: column.isActive
            ? Bo.mix(Bo.hairline, column.tint, 0.45)
            : Bo.hairline
        border.width: 1
        Behavior on border.color { ColorAnimation { duration: Bo.quick } }

        Column {
            anchors.fill: parent
            anchors.margins: Bo.gapSM
            spacing: Bo.gapSM

            // ── Channel header ─────────────────────────────────
            Rectangle {
                width: parent.width; height: 38; radius: 6
                color: Bo.mix(Bo.surface, column.tint,
                              column.isActive ? 0.30 : 0.18)
                border.color: column.tint; border.width: 1
                Behavior on color { ColorAnimation { duration: Bo.quick } }

                Column {
                    anchors.left: parent.left
                    anchors.leftMargin: Bo.gapSM
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 1
                    Text {
                        text: column.channelLabel
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1.2
                    }
                    Text {
                        text: column.channelSubtitle
                        color: Bo.slate
                        font.pixelSize: 8
                        font.letterSpacing: 0.4
                    }
                }

                // Per-channel reset. Independent of the global
                // "Reset all" on MixerPage. ↺ icon, tinted to match
                // the channel so the user understands it's scoped.
                Rectangle {
                    id: resetBtn
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.rightMargin: 6
                    width: 26; height: 26; radius: 13
                    color: chanResetMa.pressed
                        ? Bo.mix(Bo.surface, column.tint, 0.55)
                        : "transparent"
                    border.color: Bo.mix(column.tint, Bo.silver, 0.30)
                    border.width: 1
                    opacity: column.isActive ? 1.0 : 0.55
                    Behavior on opacity { NumberAnimation { duration: Bo.quick } }
                    Text {
                        anchors.centerIn: parent
                        text: "↺"
                        color: Bo.bone
                        font.pixelSize: 12
                    }
                    MouseArea {
                        id: chanResetMa
                        anchors.fill: parent
                        // Only enabled when there's something to reset
                        // — saves the user from a no-op tap.
                        enabled: column.isActive
                        onClicked: qtobj.reset_mixer_channel(column.channelKey)
                    }
                }
            }

            // ── Four vertical sliders ──────────────────────────
            Row {
                width: parent.width
                height: parent.height - 50
                spacing: 0

                BandSlider {
                    width: parent.width / 4
                    height: parent.height
                    bandLabel:   "LOW"
                    freqLabel:   column.hzLabel(column.lowHz)
                    roleHint:    column.lowRoleHint
                    tint:        column.tint
                    isVolume:    false
                    value:       column.lowValue
                    valueLabel:  column.dbLabel(column.lowValue)
                    onSliderMoved: function(v) {
                        qtobj.set_mixer_value(column.channelKey, "low", v)
                    }
                }
                BandSlider {
                    width: parent.width / 4
                    height: parent.height
                    bandLabel:   "MID"
                    freqLabel:   column.hzLabel(column.midHz)
                    roleHint:    column.midRoleHint
                    tint:        column.tint
                    isVolume:    false
                    value:       column.midValue
                    valueLabel:  column.dbLabel(column.midValue)
                    onSliderMoved: function(v) {
                        qtobj.set_mixer_value(column.channelKey, "mid", v)
                    }
                }
                BandSlider {
                    width: parent.width / 4
                    height: parent.height
                    bandLabel:   "HIGH"
                    freqLabel:   column.hzLabel(column.highHz)
                    roleHint:    column.highRoleHint
                    tint:        column.tint
                    isVolume:    false
                    value:       column.highValue
                    valueLabel:  column.dbLabel(column.highValue)
                    onSliderMoved: function(v) {
                        qtobj.set_mixer_value(column.channelKey, "high", v)
                    }
                }
                BandSlider {
                    width: parent.width / 4
                    height: parent.height
                    bandLabel:   "VOL"
                    freqLabel:   ""
                    roleHint:    "master"
                    tint:        column.tint
                    isVolume:    true
                    value:       column.volValue
                    valueLabel:  column.volLabel(column.volValue)
                    onSliderMoved: function(v) {
                        qtobj.set_mixer_value(column.channelKey, "vol", v)
                    }
                }
            }
        }
    }
}
