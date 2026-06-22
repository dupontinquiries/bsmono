// qml/MixerFileProcessor.qml
//
// Compact section at the bottom of the MixerPage that lets the user
// process a stereo WAV file through the current mixer settings and
// write the result as OggOpus. Same DSP path the future realtime
// thread will use — this is a correctness preview.
//
// Layout (top → bottom):
//   • Section title + status chip
//   • Input row: path field + 📂 picker + ▶ Preview output
//   • Output row: path field
//   • Bitrate row: label + slider + numeric readout + progress bar
//   • Action row: ▶ Run, ⤴ Mix & Play, ⟿ Live Stream, ≋ Spectrum
//   • Result / error card
//
// All colors use Bo palette tokens (accent / sage / danger / hairline)
// so the page shifts with day/dim/night mode.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    /// Action colors. accent gates the primary buttons (Run, Live);
    /// sage gates "constructive secondary" (Mix & Play); danger /
    /// success render the result card.
    readonly property color accent:    Bo.accent
    readonly property color secondary: Bo.sage

    property string inputPath:  ""
    property string outputPath: ""
    property int    bitrateKbps: 128

    property var result: {
        try { return JSON.parse(qtobj.mixer_proc_result_json || "null") }
        catch (e) { return null }
    }
    Connections {
        target: qtobj
        function onMixer_proc_changed() {
            try { root.result = JSON.parse(qtobj.mixer_proc_result_json || "null") }
            catch (e) { root.result = null }
        }
    }

    function suggestOutput(input) {
        if (!input) return ""
        var lastSlash = Math.max(input.lastIndexOf("/"), input.lastIndexOf("\\"))
        var dir  = lastSlash >= 0 ? input.substring(0, lastSlash + 1) : ""
        var name = lastSlash >= 0 ? input.substring(lastSlash + 1) : input
        var dot  = name.lastIndexOf(".")
        var stem = dot >= 0 ? name.substring(0, dot) : name
        return dir + stem + ".mixed.opus"
    }

    Rectangle {
        anchors.fill: parent
        color: Bo.surface
        radius: 8
        border.color: Bo.hairline
        border.width: 1

        Column {
            anchors.fill: parent
            anchors.margins: Bo.gapSM
            spacing: Bo.gapSM

            // ── Section header ─────────────────────────────────
            Row {
                width: parent.width
                spacing: Bo.gapSM

                Text {
                    text: "PROCESS FILE"
                    color: Bo.silver
                    font.pixelSize: Bo.microSize
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.0
                    anchors.verticalCenter: parent.verticalCenter
                }
                // Status chip — animates color when running.
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    height: 18; radius: 9
                    color: qtobj.mixer_proc_running
                        ? Bo.mix(Bo.surface, root.accent, 0.30)
                        : (qtobj.mixer_proc_error.length > 0
                            ? Bo.mix(Bo.surface, Bo.danger, 0.30)
                            : (root.result
                                ? Bo.mix(Bo.surface, Bo.success, 0.25)
                                : "transparent"))
                    border.color: qtobj.mixer_proc_running
                        ? root.accent
                        : (qtobj.mixer_proc_error.length > 0
                            ? Bo.danger
                            : (root.result ? Bo.success : Bo.hairline))
                    border.width: 1
                    Behavior on color  { ColorAnimation { duration: Bo.quick } }
                    Behavior on border.color { ColorAnimation { duration: Bo.quick } }
                    width: chipText.implicitWidth + 16
                    Text {
                        id: chipText
                        anchors.centerIn: parent
                        text: qtobj.mixer_proc_running
                            ? "processing…"
                            : (qtobj.mixer_proc_error.length > 0
                                ? "error"
                                : (root.result ? "done" : "ready"))
                        color: Bo.bone
                        font.pixelSize: 9
                        font.weight: Font.DemiBold
                    }
                }
                Item { width: 4; height: 1 }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: "pipe a WAV through your current settings → OggOpus"
                    color: Bo.slate
                    font.pixelSize: 9
                }
            }

            // ── Input row ─────────────────────────────────────
            Row {
                width: parent.width
                spacing: Bo.gapSM

                TextField {
                    id: inField
                    width: parent.width - 36 - Bo.gapSM
                    placeholderText: "WAV / FLAC / Opus / MP3 — pick or type"
                    text: root.inputPath
                    color: Bo.bone
                    font.pixelSize: Bo.captionSize
                    enabled: !qtobj.mixer_proc_running
                    background: Rectangle {
                        color: Bo.graphite
                        radius: 6
                        border.color: inField.activeFocus ? root.accent : Bo.hairline
                        border.width: 1
                        Behavior on border.color { ColorAnimation { duration: Bo.quick } }
                    }
                    onTextChanged: {
                        root.inputPath = text
                        // Auto-populate output if user hasn't manually
                        // edited it (lastVal is the previous
                        // auto-suggested value).
                        if (!root.outputPath
                            || root.outputPath === root.suggestOutput(lastVal))
                            root.outputPath = root.suggestOutput(text)
                        if (outField.text !== root.outputPath)
                            outField.text = root.outputPath
                        lastVal = text
                    }
                    property string lastVal: ""
                    // Enter triggers Run (matches keyboard expectation)
                    Keys.onPressed: {
                        if ((event.key === Qt.Key_Return
                            || event.key === Qt.Key_Enter)
                            && runMa.enabled) {
                            qtobj.process_through_mixer_file(
                                root.inputPath, root.outputPath, root.bitrateKbps)
                            event.accepted = true
                        }
                    }
                }
                // 📂 file picker — opens kdialog/zenity. Reuses the
                // same picker the soundscape page does; no separate
                // qt_method needed.
                Rectangle {
                    width: 36; height: 36; radius: 18
                    color: pickMa.pressed
                        ? Bo.surfaceHi
                        : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: "📂"
                        font.pixelSize: 14
                    }
                    MouseArea {
                        id: pickMa
                        anchors.fill: parent
                        enabled: !qtobj.mixer_proc_running
                        onClicked: {
                            var p = qtobj.pick_audio_file()
                            if (p && p.length > 0) {
                                root.inputPath = p
                                inField.text = p
                            }
                        }
                    }
                }
            }

            // ── Output row ────────────────────────────────────
            TextField {
                id: outField
                width: parent.width
                placeholderText: "output.mixed.opus  (auto-suggested from input)"
                text: root.outputPath
                color: Bo.bone
                font.pixelSize: Bo.captionSize
                enabled: !qtobj.mixer_proc_running
                background: Rectangle {
                    color: Bo.graphite
                    radius: 6
                    border.color: outField.activeFocus ? root.accent : Bo.hairline
                    border.width: 1
                    Behavior on border.color { ColorAnimation { duration: Bo.quick } }
                }
                onTextChanged: root.outputPath = text
            }

            // ── Bitrate row ───────────────────────────────────
            Row {
                width: parent.width
                spacing: Bo.gapSM

                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 90
                    text: "Opus bitrate"
                    color: Bo.bone
                    font.pixelSize: Bo.microSize
                }
                Slider {
                    id: brSlider
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 90 - 70 - 90 - Bo.gapSM * 3
                    from: 32; to: 192; stepSize: 8
                    value: root.bitrateKbps
                    enabled: !qtobj.mixer_proc_running
                    onValueChanged: root.bitrateKbps = Math.round(value)

                    // Themed slider visuals — custom background/handle
                    // so the slider doesn't fall back to Qt Controls
                    // defaults that look out of place. Same visual
                    // language as the volume sliders elsewhere.
                    background: Rectangle {
                        x: brSlider.leftPadding
                        y: brSlider.topPadding
                            + brSlider.availableHeight / 2 - height / 2
                        width: brSlider.availableWidth
                        height: 4; radius: 2
                        color: Bo.graphite
                        border.color: Bo.hairline; border.width: 1
                        Rectangle {
                            width: brSlider.visualPosition * parent.width
                            height: parent.height
                            radius: 2
                            color: root.accent
                        }
                    }
                    handle: Rectangle {
                        x: brSlider.leftPadding
                            + brSlider.visualPosition
                              * (brSlider.availableWidth - width)
                        y: brSlider.topPadding
                            + brSlider.availableHeight / 2 - height / 2
                        width: 14; height: 14; radius: 7
                        color: Bo.bone
                        border.color: root.accent; border.width: 2
                    }
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 70
                    horizontalAlignment: Text.AlignRight
                    text: root.bitrateKbps + " kbps"
                    color: root.accent
                    font.pixelSize: Bo.microSize
                    font.weight: Font.DemiBold
                }
                // Progress bar — fills left→right during processing.
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 90; height: 6; radius: 3
                    color: Bo.graphite
                    border.color: Bo.hairline; border.width: 1
                    Rectangle {
                        anchors.left: parent.left
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        radius: 3
                        color: root.accent
                        width: parent.width * Math.max(0, Math.min(1,
                            qtobj.mixer_proc_progress || 0))
                        Behavior on width { NumberAnimation { duration: Bo.quick } }
                    }
                }
            }

            // ── Action row ────────────────────────────────────
            //
            // Four actions. Run / Live Stream are primary (accent
            // fill); Mix & Play is secondary (sage outline);
            // Spectrum is tertiary (hairline outline only).
            //
            // Buttons reflow on narrower screens via the flexible
            // spacer between them — minimum sizes are preserved so
            // labels never truncate.
            Row {
                width: parent.width
                spacing: Bo.gapSM

                // ▶ Run — process to file. Primary action.
                Rectangle {
                    width: 80; height: 36; radius: 18
                    color: runMa.enabled
                        ? (runMa.pressed
                            ? Bo.mix(Bo.surface, root.accent, 0.70)
                            : root.accent)
                        : Bo.mix(Bo.surface, root.accent, 0.20)
                    Text {
                        anchors.centerIn: parent
                        text: qtobj.mixer_proc_running ? "…" : "▶ Run"
                        color: runMa.enabled ? Bo.graphite : Bo.slate
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                    }
                    MouseArea {
                        id: runMa
                        anchors.fill: parent
                        enabled: !qtobj.mixer_proc_running
                              && root.inputPath.length > 0
                              && root.outputPath.length > 0
                        onClicked: {
                            qtobj.process_through_mixer_file(
                                root.inputPath, root.outputPath, root.bitrateKbps)
                        }
                    }
                }
                // ⤴ Mix & Play — process + immediately route through
                // active driver. Secondary (sage outline) — frequent
                // but not the most common.
                Rectangle {
                    width: 100; height: 36; radius: 18
                    color: playMa.enabled
                        ? (playMa.pressed
                            ? Bo.mix(Bo.surface, root.secondary, 0.60)
                            : Bo.mix(Bo.surface, root.secondary, 0.30))
                        : Bo.mix(Bo.surface, root.secondary, 0.15)
                    border.color: root.secondary; border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: qtobj.mixer_proc_running ? "…" : "⤴ Mix & Play"
                        color: playMa.enabled ? Bo.bone : Bo.slate
                        font.pixelSize: Bo.microSize
                        font.weight: Font.DemiBold
                    }
                    MouseArea {
                        id: playMa
                        anchors.fill: parent
                        enabled: !qtobj.mixer_proc_running
                              && root.inputPath.length > 0
                              && qtobj.active_url.length > 0
                        onClicked: {
                            qtobj.mix_and_play_file(
                                root.inputPath, root.bitrateKbps)
                        }
                    }
                }
                // ⟿ Live Stream — continuous re-render to the
                // device, slider changes within the device's buffer.
                Rectangle {
                    width: 110; height: 36; radius: 18
                    color: liveMa.enabled
                        ? (liveMa.pressed
                            ? Bo.mix(Bo.surface, root.accent, 0.60)
                            : Bo.mix(Bo.surface, root.accent, 0.40))
                        : Bo.mix(Bo.surface, root.accent, 0.15)
                    border.color: root.accent; border.width: 2
                    Text {
                        anchors.centerIn: parent
                        text: "⟿ Live Stream"
                        color: liveMa.enabled ? Bo.bone : Bo.slate
                        font.pixelSize: Bo.microSize
                        font.weight: Font.DemiBold
                    }
                    MouseArea {
                        id: liveMa
                        anchors.fill: parent
                        enabled: !qtobj.mixer_proc_running
                              && root.inputPath.length > 0
                              && qtobj.active_url.length > 0
                        onClicked: {
                            qtobj.live_stream_through_mixer(
                                root.inputPath, root.bitrateKbps)
                        }
                    }
                }
                // ≋ Spectrum — analyze the source file's frequency
                // content so the EQ curves overlay shows where the
                // music actually has energy. Tertiary action.
                Rectangle {
                    width: 100; height: 36; radius: 18
                    color: anaMa.pressed ? Bo.surfaceHi : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: qtobj.spectrum_running
                            ? "…"
                            : (qtobj.spectrum_json.length > 0
                                ? "↻ Spectrum"
                                : "≋ Spectrum")
                        color: anaMa.enabled ? Bo.bone : Bo.slate
                        font.pixelSize: Bo.microSize
                    }
                    MouseArea {
                        id: anaMa
                        anchors.fill: parent
                        enabled: !qtobj.spectrum_running
                              && root.inputPath.length > 0
                        onClicked: qtobj.analyze_spectrum(root.inputPath)
                    }
                }
            }

            // ── Result / error card ───────────────────────────
            Rectangle {
                visible: root.result || qtobj.mixer_proc_error.length > 0
                width: parent.width
                radius: 6
                height: visible ? (rText.implicitHeight + 14) : 0
                color: qtobj.mixer_proc_error.length > 0
                    ? Bo.mix(Bo.surface, Bo.danger, 0.20)
                    : Bo.mix(Bo.surface, Bo.success, 0.18)
                border.color: qtobj.mixer_proc_error.length > 0
                    ? Bo.danger
                    : Bo.success
                border.width: 1
                Text {
                    id: rText
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: 10
                    anchors.rightMargin: 10
                    text: qtobj.mixer_proc_error.length > 0
                        ? ("⚠ " + qtobj.mixer_proc_error)
                        : (root.result
                            ? ("✓ Mixed " + root.result.frames + " frames → "
                               + root.result.output_path
                               + "  ("
                               + (root.result.size_bytes / 1024).toFixed(1)
                               + " KB)")
                            : "")
                    color: qtobj.mixer_proc_error.length > 0
                        ? Qt.lighter(Bo.danger, 1.25)
                        : Qt.lighter(Bo.success, 1.25)
                    font.pixelSize: Bo.microSize
                    wrapMode: Text.Wrap
                    elide: Text.ElideMiddle
                }
            }
        }
    }
}
