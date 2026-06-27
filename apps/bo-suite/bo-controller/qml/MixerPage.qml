// qml/MixerPage.qml
//
// Realtime stream mixer with two principled channel groups:
//
//   SPEAKER       — operates on the Mid signal (L+R, centered
//                   content; vocals & dialogue live here in modern
//                   stereo productions)
//   INSTRUMENTAL  — operates on the Side signal (L-R, panned-wide
//                   content; instruments, ambience, stereo effects)
//
// Each group has a 3-band parametric EQ + master volume. The EQ
// center frequencies are LOCKED to perceptually-meaningful zones for
// that group's musical role — adjusting frequency would let the user
// wander into territory where the group label no longer fits the
// audio, which is the failure mode "groups don't make sense" the
// design is meant to avoid.
//
// Speaker bands (locked):
//   Low  250 Hz   chest body
//   Mid  1.5 kHz  vocal presence / formants
//   High 4 kHz    intelligibility & air
//
// Instrumental bands (locked):
//   Low  200 Hz   bass weight
//   Mid  800 Hz   warmth / body
//   High 6 kHz    cymbal sparkle
//
// Behavior at neutral (all EQ at 0 dB, both volumes at 1.0): the
// mixer is bit-identical passthrough — guaranteed by the Rust
// roundtrip_neutral_is_identity test.
//
// Colors used here are Bo palette tokens (mauve / teal / accent /
// success) — never hex literals — so the page shifts with the
// day/dim/night mode and any future palette tweak in Bo.qml lands
// uniformly.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    /// Speaker channel tint — the mauve from the Bo palette.
    /// Distinct enough from the teal partner to be unambiguously
    /// "the other channel," same warmth family as the rest of the
    /// app so it doesn't read as a tacked-on accent.
    property color speakerTint:      Bo.mauve
    /// Instrumental channel tint — cooler companion to the mauve.
    property color instrumentalTint: Bo.teal

    property var bandCenters: {
        try { return JSON.parse(qtobj.mixer_band_centers_json || "{}") }
        catch (e) { return {} }
    }
    Connections {
        target: qtobj
        function onMixer_changed() {
            try { root.bandCenters = JSON.parse(qtobj.mixer_band_centers_json || "{}") }
            catch (e) { root.bandCenters = {} }
        }
    }

    /// Format a frequency for the slider label: "250 Hz" or "4 kHz".
    function hzLabel(hz) {
        if (!hz) return ""
        if (hz >= 1000) return (hz / 1000).toFixed(hz % 1000 === 0 ? 0 : 1) + " kHz"
        return Math.round(hz) + " Hz"
    }
    /// Format a dB value with sign and 1 decimal.
    function dbLabel(db) {
        if (Math.abs(db) < 0.1) return "0 dB"
        return (db > 0 ? "+" : "") + db.toFixed(1) + " dB"
    }
    /// Format a linear volume as a percentage.
    function volLabel(v) {
        return Math.round(v * 100) + "%"
    }

    ScrollView {
        // Reserve space for the BottomTransportBar so the last
        // row/card isn't clipped or overlapped (bottom-cutoff fix).
        bottomPadding: Bo.bottomChromeHeight
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        clip: true

        Column {
            width: root.width - Bo.gapMD * 2
            spacing: Bo.gapMD

            // ── Header ─────────────────────────────────────────────
            Row {
                width: parent.width
                spacing: Bo.gapSM
                Text {
                    text: "STREAM MIXER"
                    color: Bo.silver
                    font.pixelSize: Bo.captionSize
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.2
                    anchors.verticalCenter: parent.verticalCenter
                }
                Text {
                    text: "· mid/side · 3-band EQ"
                    color: Bo.slate
                    font.pixelSize: Bo.microSize
                    anchors.verticalCenter: parent.verticalCenter
                }
                // Flexible spacer — pushes the reset button right.
                Item {
                    width: Math.max(0, parent.width - 280)
                    height: 1
                }

                // ↺ Reset all — global "neutral the mixer". Uses the
                // hairline outline rather than the accent fill so it
                // doesn't compete with the per-channel reset buttons
                // which are louder (the user is more likely to reset
                // one channel than the whole mixer).
                Rectangle {
                    width: 80; height: 28; radius: 14
                    anchors.verticalCenter: parent.verticalCenter
                    color: resetMa.pressed ? Bo.surfaceHi : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: "↺ Reset all"
                        color: Bo.bone
                        font.pixelSize: Bo.microSize
                    }
                    MouseArea {
                        id: resetMa; anchors.fill: parent
                        onClicked: qtobj.reset_mixer()
                    }
                }
            }

            Text {
                width: parent.width
                text: "The SPEAKER channel operates on what's centered in the stereo "
                    + "image — typically vocals & dialogue. The INSTRUMENTAL channel "
                    + "operates on what's spread wide — typically instruments & "
                    + "ambience. Band frequencies are locked to the perceptually "
                    + "meaningful zones for each role."
                color: Bo.slate
                font.pixelSize: Bo.microSize
                wrapMode: Text.Wrap
            }

            // ── Frequency-response visualization ──────────────────
            // Live curves driven by qtobj.mixer_curves_json. Updates
            // every time a slider moves so the user can verify by
            // eye that turning a knob does what its label promises.
            FreqResponseCurve {
                width: parent.width
                height: 160
                speakerColor:      root.speakerTint
                instrumentalColor: root.instrumentalTint
            }

            // ── Two channel columns ───────────────────────────────
            Row {
                width: parent.width
                spacing: Bo.gapMD
                // Fixed height — fits all four sliders comfortably.
                // The outer ScrollView handles overflow on small
                // screens where the curve + columns + processor
                // won't all fit at once.
                height: 380

                // SPEAKER column
                ChannelColumn {
                    width: (parent.width - Bo.gapMD) / 2
                    height: parent.height
                    channelLabel:    "SPEAKER"
                    channelSubtitle: "centered · vocals & dialogue"
                    tint:            root.speakerTint
                    channelKey:      "spk"
                    lowHz:    root.bandCenters.spk_low  || 250
                    midHz:    root.bandCenters.spk_mid  || 1500
                    highHz:   root.bandCenters.spk_high || 4000
                    lowValue:  qtobj.mix_spk_low
                    midValue:  qtobj.mix_spk_mid
                    highValue: qtobj.mix_spk_high
                    volValue:  qtobj.mix_spk_vol
                    lowRoleHint:  "chest body"
                    midRoleHint:  "vocal presence"
                    highRoleHint: "air"
                    hzLabel: root.hzLabel
                    dbLabel: root.dbLabel
                    volLabel: root.volLabel
                }

                // INSTRUMENTAL column
                ChannelColumn {
                    width: (parent.width - Bo.gapMD) / 2
                    height: parent.height
                    channelLabel:    "INSTRUMENTAL"
                    channelSubtitle: "spread · music & ambience"
                    tint:            root.instrumentalTint
                    channelKey:      "ins"
                    lowHz:    root.bandCenters.ins_low  || 200
                    midHz:    root.bandCenters.ins_mid  || 800
                    highHz:   root.bandCenters.ins_high || 6000
                    lowValue:  qtobj.mix_ins_low
                    midValue:  qtobj.mix_ins_mid
                    highValue: qtobj.mix_ins_high
                    volValue:  qtobj.mix_ins_vol
                    lowRoleHint:  "bass weight"
                    midRoleHint:  "warmth"
                    highRoleHint: "sparkle"
                    hzLabel: root.hzLabel
                    dbLabel: root.dbLabel
                    volLabel: root.volLabel
                }
            }

            // ── Spatializer (stereo width) ────────────────────────
            //
            // Scales the Side component of the mixer's M/S stage: a
            // single, musical "how wide is the stereo image" control that
            // sits naturally on top of the existing decomposition.
            //   mono ─────────── normal ─────────── wide
            //    0.0               1.0               2.0
            // Centered content (vocals/bass/kick, which live in Mid) is
            // unaffected at any setting.
            Rectangle {
                width: parent.width
                height: 92
                radius: 12
                color: Bo.surface
                border.color: Bo.hairline
                border.width: 1

                Column {
                    anchors.fill: parent
                    anchors.margins: Bo.gapMD
                    spacing: 8

                    Row {
                        width: parent.width
                        Text {
                            text: "SPATIALIZER"
                            color: Bo.silver
                            font.pixelSize: 9
                            font.weight: Font.DemiBold
                            font.letterSpacing: 0.8
                        }
                        Item { width: parent.width - 200; height: 1 }
                        Text {
                            // Live readout: mono / normal / wide + factor.
                            text: {
                                var w = qtobj.mix_width
                                var tag = w < 0.05 ? "mono"
                                        : Math.abs(w - 1.0) < 0.05 ? "normal"
                                        : w > 1.0 ? "wide" : "narrow"
                                return tag + "  ·  " + w.toFixed(2) + "×"
                            }
                            color: Bo.bone
                            font.pixelSize: 10
                            font.weight: Font.DemiBold
                        }
                    }

                    Row {
                        width: parent.width
                        spacing: 8
                        Text {
                            text: "🔇"
                            font.pixelSize: 12
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Slider {
                            id: widthSlider
                            width: parent.width - 60
                            anchors.verticalCenter: parent.verticalCenter
                            from: 0.0; to: 2.0
                            value: qtobj.mix_width
                            onMoved: qtobj.set_mixer_value("spatial", "width", value)
                            // Snap-to-center detent at 1.0 so "normal" is
                            // easy to find.
                            onPressedChanged: {
                                if (!pressed && Math.abs(value - 1.0) < 0.06) {
                                    value = 1.0
                                    qtobj.set_mixer_value("spatial", "width", 1.0)
                                }
                            }
                        }
                        Text {
                            text: "🔊"
                            font.pixelSize: 12
                            anchors.verticalCenter: parent.verticalCenter
                        }
                    }
                }
            }

            // ── Offline file processor ────────────────────────────
            //
            // Process a WAV file through the current mixer settings and
            // write the result as OggOpus. Uses the SAME process_buffer
            // call any future realtime audio thread will use, so what
            // you hear from the output file is a faithful preview of
            // what live processing will sound like.
            MixerFileProcessor {
                width: parent.width
                height: 260
            }
        }
    }
}
