// qml/SoundscapePage.qml
//
// Soundscape creator. Takes a stereo WAV file, finds the cleanest
// possible loop point via random-walk MFCC + sample-discontinuity
// search, and writes the result as an OggOpus file ready for a
// soundscape player.
//
// Layout (top → bottom):
//   • Header with title + status
//   • Input/output path fields
//   • Parameter sliders: target length, tolerance, candidates,
//     bitrate, crossfade
//   • Run button + progress bar
//   • Result card with score breakdown + size + open-folder action
//
// The DSP happens on the media worker thread; this page only
// dispatches the command and renders the soundscape_* qt_properties.
// During processing the run button is disabled, the progress bar
// animates, and the params are read-only (so the user can't change
// parameters mid-run).

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"

    // ── Inputs ────────────────────────────────────────────────────
    property string inputPath:  ""
    property string outputPath: ""

    // ── Parameters ───────────────────────────────────────────────
    // Sensible defaults for ambient/nature soundscapes: 8-second
    // target loop, ±4s tolerance, 2000 candidates, 96 kbps Opus,
    // 50 ms crossfade at the seam.
    property real    targetSeconds:    8.0
    property real    toleranceSeconds: 4.0
    property int     nCandidates:      2000
    property int     bitrateKbps:      96
    property int     fadeMs:           50

    property var result: {
        try { return JSON.parse(qtobj.soundscape_result_json || "null") }
        catch (e) { return null }
    }
    Connections {
        target: qtobj
        function onSoundscape_changed() {
            try { root.result = JSON.parse(qtobj.soundscape_result_json || "null") }
            catch (e) { root.result = null }
        }
    }

    // Suggest an output path based on the input path. We route to a
    // dedicated ~/Music/Soundscapes/ collection rather than dropping
    // .loop.opus files next to the input, so renders accumulate as a
    // browsable set rather than scattering across the filesystem.
    // The user can still type/edit the output path if they want it
    // somewhere else; this is just the default.
    function suggestOutput(input) {
        if (!input) return ""
        var lastSlash = Math.max(input.lastIndexOf("/"), input.lastIndexOf("\\"))
        var name = lastSlash >= 0 ? input.substring(lastSlash + 1) : input
        var dot  = name.lastIndexOf(".")
        var stem = dot >= 0 ? name.substring(0, dot) : name
        // Reuse the same root mDNS/library setup uses — qtobj exposes
        // the user's home via env, with fallback to /tmp if nothing's
        // set. We let qtobj.suggested_soundscapes_dir handle the path
        // logic; it picks the first existing scan root (~/Music) and
        // appends /Soundscapes. If none, falls back to $HOME/Soundscapes.
        var dir = qtobj.suggested_soundscapes_dir
                ? qtobj.suggested_soundscapes_dir() : ""
        if (!dir || dir.length === 0) {
            // Fallback: same dir as input.
            dir = lastSlash >= 0 ? input.substring(0, lastSlash + 1) : "./"
        }
        // Ensure trailing slash.
        if (dir.charAt(dir.length - 1) !== "/") dir = dir + "/"
        return dir + stem + ".loop.opus"
    }

    ScrollView {
        // Reserve space for the BottomTransportBar so the last
        // row/card isn't clipped or overlapped (bottom-cutoff fix).
        bottomPadding: Bo.bottomChromeHeight
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        clip: true

        Column {
            width: parent.width
            spacing: Bo.gapMD

            // Header
            Row {
                width: parent.width
                spacing: Bo.gapSM
                Text {
                    text: "LOOP STUDIO"
                    color: Bo.silver
                    font.pixelSize: Bo.captionSize
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.2
                    anchors.verticalCenter: parent.verticalCenter
                }
                Text {
                    text: qtobj.soundscape_running ? "· processing…"
                        : (root.result ? "· last run complete" : "· idle")
                    color: qtobj.soundscape_running ? root.accent : Bo.slate
                    font.pixelSize: 10
                    anchors.verticalCenter: parent.verticalCenter
                }
            }

            Text {
                width: parent.width
                text: "Make a seamless looping clip for your soundscapes. "
                    + "Feed it a stereo WAV; it random-walks thousands of "
                    + "(start, end) pairs, scores each by sample discontinuity "
                    + "+ MFCC timbral distance, then refines the best with local "
                    + "perturbation. Output is OggOpus, ready to drop into your "
                    + "clip pool and use from the Spaces page."
                color: Bo.slate
                font.pixelSize: 10
                wrapMode: Text.Wrap
            }

            // ── Input/output paths ────────────────────────────────
            Column {
                width: parent.width
                spacing: 4
                Text {
                    text: "INPUT WAV"
                    color: Bo.silver
                    font.pixelSize: 9
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.0
                }
                Row {
                    width: parent.width
                    spacing: Bo.gapSM
                    TextField {
                        id: inputField
                        width: parent.width - 80 - Bo.gapSM
                        placeholderText: "/path/to/source.wav"
                        text: root.inputPath
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                        enabled: !qtobj.soundscape_running
                        background: Rectangle {
                            color: Bo.graphite
                            radius: 6
                            border.color: inputField.activeFocus ? root.accent : Bo.hairline
                            border.width: 1
                        }
                        onTextChanged: {
                            root.inputPath = text
                            if (!root.outputPath || root.outputPath === root.suggestOutput(
                                    inputField.lastValue || ""))
                            {
                                root.outputPath = root.suggestOutput(text)
                                outputField.text = root.outputPath
                            }
                            lastValue = text
                        }
                        property string lastValue: ""
                    }
                    Rectangle {
                        width: 80; height: 32; radius: 16
                        anchors.verticalCenter: parent.verticalCenter
                        color: browseMa.pressed ? Bo.surfaceHi : "transparent"
                        border.color: root.accent; border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: "📁 Browse"
                            color: root.accent
                            font.pixelSize: 10
                            font.weight: Font.DemiBold
                        }
                        MouseArea {
                            id: browseMa
                            anchors.fill: parent
                            enabled: !qtobj.soundscape_running
                            onClicked: {
                                var picked = qtobj.pick_audio_file()
                                if (picked && picked.length > 0) {
                                    inputField.text = picked
                                }
                            }
                        }
                    }
                }
            }

            Column {
                width: parent.width
                spacing: 4
                Text {
                    text: "OUTPUT OPUS"
                    color: Bo.silver
                    font.pixelSize: 9
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.0
                }
                TextField {
                    id: outputField
                    width: parent.width
                    placeholderText: "/path/to/output.loop.opus"
                    text: root.outputPath
                    color: Bo.bone
                    font.pixelSize: Bo.captionSize
                    enabled: !qtobj.soundscape_running
                    background: Rectangle {
                        color: Bo.graphite
                        radius: 6
                        border.color: outputField.activeFocus ? root.accent : Bo.hairline
                        border.width: 1
                    }
                    onTextChanged: root.outputPath = text
                }
            }

            // ── Parameter sliders ─────────────────────────────────
            // Each is a labeled row with the value displayed inline
            // and a slider. We disable them during processing so the
            // run can't be perturbed mid-flight.
            Column {
                width: parent.width
                spacing: Bo.gapSM
                enabled: !qtobj.soundscape_running

                // Target loop length
                Row {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 140
                        text: "Target length"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                    }
                    Slider {
                        id: targetSlider
                        width: parent.width - 200
                        anchors.verticalCenter: parent.verticalCenter
                        from: 2.0; to: 30.0; stepSize: 0.5
                        value: root.targetSeconds
                        onValueChanged: root.targetSeconds = value
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 50
                        horizontalAlignment: Text.AlignRight
                        text: root.targetSeconds.toFixed(1) + " s"
                        color: root.accent
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                    }
                }
                // Tolerance band
                Row {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 140
                        text: "Tolerance ±"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                    }
                    Slider {
                        width: parent.width - 200
                        anchors.verticalCenter: parent.verticalCenter
                        from: 0.5; to: 15.0; stepSize: 0.5
                        value: root.toleranceSeconds
                        onValueChanged: root.toleranceSeconds = value
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 50
                        horizontalAlignment: Text.AlignRight
                        text: root.toleranceSeconds.toFixed(1) + " s"
                        color: root.accent
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                    }
                }
                // Candidates per search
                Row {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 140
                        text: "Search budget"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                    }
                    Slider {
                        width: parent.width - 200
                        anchors.verticalCenter: parent.verticalCenter
                        from: 200; to: 10000; stepSize: 100
                        value: root.nCandidates
                        onValueChanged: root.nCandidates = Math.round(value)
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 50
                        horizontalAlignment: Text.AlignRight
                        text: root.nCandidates.toString()
                        color: root.accent
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                    }
                }
                // Opus bitrate
                Row {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 140
                        text: "Opus bitrate"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                    }
                    Slider {
                        width: parent.width - 200
                        anchors.verticalCenter: parent.verticalCenter
                        from: 32; to: 192; stepSize: 8
                        value: root.bitrateKbps
                        onValueChanged: root.bitrateKbps = Math.round(value)
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 60
                        horizontalAlignment: Text.AlignRight
                        text: root.bitrateKbps + " kbps"
                        color: root.accent
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                    }
                }
                // Crossfade
                Row {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 140
                        text: "Crossfade"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                    }
                    Slider {
                        width: parent.width - 200
                        anchors.verticalCenter: parent.verticalCenter
                        from: 0; to: 500; stepSize: 10
                        value: root.fadeMs
                        onValueChanged: root.fadeMs = Math.round(value)
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 50
                        horizontalAlignment: Text.AlignRight
                        text: root.fadeMs + " ms"
                        color: root.accent
                        font.pixelSize: 10
                        font.weight: Font.DemiBold
                    }
                }
            }

            // ── Run button + progress ─────────────────────────────
            Row {
                width: parent.width
                spacing: Bo.gapSM

                Rectangle {
                    width: 120; height: 36; radius: 18
                    color: runMa.enabled ? (runMa.pressed
                        ? Bo.mix(Bo.surface, root.accent, 0.7) : root.accent)
                        : Bo.mix(Bo.surface, root.accent, 0.20)
                    Text {
                        anchors.centerIn: parent
                        text: qtobj.soundscape_running ? "Working…" : "▶ Run"
                        color: runMa.enabled ? Bo.graphite : Bo.slate
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                    }
                    MouseArea {
                        id: runMa
                        anchors.fill: parent
                        enabled: !qtobj.soundscape_running
                              && root.inputPath.length > 0
                              && root.outputPath.length > 0
                        onClicked: {
                            qtobj.process_soundscape(
                                root.inputPath,
                                root.outputPath,
                                root.targetSeconds,
                                root.toleranceSeconds,
                                root.nCandidates,
                                root.bitrateKbps,
                                root.fadeMs)
                        }
                    }
                }

                // Progress bar fills the rest of the row.
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 120 - Bo.gapSM
                    height: 6; radius: 3
                    color: Bo.graphite
                    border.color: Bo.hairline; border.width: 1
                    Rectangle {
                        anchors.left: parent.left
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        radius: 3
                        color: root.accent
                        width: parent.width * Math.max(0, Math.min(1,
                            qtobj.soundscape_progress || 0))
                        Behavior on width { NumberAnimation { duration: Bo.quick } }
                    }
                }
            }

            // ── Result / Error card ───────────────────────────────
            Rectangle {
                visible: root.result || qtobj.soundscape_error.length > 0
                width: parent.width
                height: resultContent.implicitHeight + 16
                radius: 8
                color: qtobj.soundscape_error.length > 0
                    ? Bo.mix(Bo.surface, "#8E4A3D", 0.20)
                    : Bo.mix(Bo.surface, "#4a7c59", 0.18)
                border.color: qtobj.soundscape_error.length > 0
                    ? "#8E4A3D" : "#4a7c59"
                border.width: 1

                Column {
                    id: resultContent
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 8
                    spacing: 4

                    // Error path
                    Text {
                        visible: qtobj.soundscape_error.length > 0
                        text: "⚠ " + qtobj.soundscape_error
                        color: "#C58578"
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                        width: parent.width
                    }

                    // Success path
                    Text {
                        visible: root.result !== null
                              && qtobj.soundscape_error.length === 0
                        text: "✓ Loop ready"
                        color: "#7FB089"
                        font.pixelSize: 11
                        font.weight: Font.DemiBold
                    }
                    Text {
                        visible: root.result !== null
                              && qtobj.soundscape_error.length === 0
                        text: root.result
                            ? ("Length: " + root.result.loop.length_seconds.toFixed(2) + " s")
                            : ""
                        color: Bo.bone
                        font.pixelSize: 10
                    }
                    Text {
                        visible: root.result !== null
                              && qtobj.soundscape_error.length === 0
                        text: root.result
                            ? ("Pop score: " + root.result.loop.pop_score.toFixed(4)
                               + "  ·  MFCC score: " + root.result.loop.mfcc_score.toFixed(4)
                               + "  ·  Total: " + root.result.loop.score.toFixed(4))
                            : ""
                        color: Bo.silver
                        font.pixelSize: 10
                    }
                    Text {
                        visible: root.result !== null
                              && qtobj.soundscape_error.length === 0
                        text: root.result
                            ? ("Output: " + root.result.output_path
                               + "  (" + (root.result.size_bytes / 1024).toFixed(1) + " KB)")
                            : ""
                        color: Bo.silver
                        font.pixelSize: 9
                        wrapMode: Text.Wrap
                        width: parent.width
                        elide: Text.ElideMiddle
                    }

                    // ── Preview / Stop the rendered soundscape ────────
                    //
                    // The soundscape output is a local OggOpus file. The
                    // Local driver (ffmpeg→paplay to default sink) is
                    // exactly the right plumbing — same as the anticipator
                    // tones, which the user knows works. We use the
                    // driver_play_local_file qt_method which routes through
                    // whichever driver is bound (Local by default; or BT
                    // if a sink is selected). Stop sends the corresponding
                    // driver_stop.
                    Row {
                        visible: root.result !== null
                              && qtobj.soundscape_error.length === 0
                        spacing: Bo.gapSM
                        topPadding: 4

                        Rectangle {
                            width: 96; height: 32; radius: 16
                            color: previewMa.pressed
                                ? Bo.surfaceHi
                                : Bo.mix(Bo.surface, "#7FB089", 0.30)
                            border.color: "#7FB089"; border.width: 1
                            Row {
                                anchors.centerIn: parent
                                spacing: 4
                                Text {
                                    text: "▶"
                                    color: "#7FB089"
                                    font.pixelSize: 12
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                Text {
                                    text: "Preview"
                                    color: Bo.bone
                                    font.pixelSize: 10
                                    font.weight: Font.DemiBold
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                            }
                            MouseArea {
                                id: previewMa
                                anchors.fill: parent
                                onClicked: {
                                    if (root.result && root.result.output_path) {
                                        qtobj.driver_play_local_file(
                                            root.result.output_path,
                                            "Soundscape preview")
                                    }
                                }
                            }
                        }
                        Rectangle {
                            width: 72; height: 32; radius: 16
                            color: stopMa.pressed ? Bo.surfaceHi : "transparent"
                            border.color: Bo.hairline; border.width: 1
                            Row {
                                anchors.centerIn: parent
                                spacing: 4
                                Text {
                                    text: "■"
                                    color: Bo.bone
                                    font.pixelSize: 12
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                Text {
                                    text: "Stop"
                                    color: Bo.bone
                                    font.pixelSize: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                            }
                            MouseArea {
                                id: stopMa
                                anchors.fill: parent
                                onClicked: qtobj.driver_stop()
                            }
                        }
                        // ── Add to Library ──────────────────────
                        //
                        // Triggers a focused scan of the soundscape's
                        // directory so it appears in Library/Search
                        // alongside imported tracks. Soundscapes
                        // already render to ~/Music/Soundscapes/ which
                        // is typically under the existing scan root,
                        // so a full RescanLibrary would also pick this
                        // up eventually — the button just makes it
                        // immediate. Status surfaces via
                        // last_play_status.
                        Rectangle {
                            width: 130; height: 28; radius: 14
                            anchors.verticalCenter: parent.verticalCenter
                            color: addMa.pressed
                                ? Bo.mix(Bo.surface, Bo.teal, 0.50)
                                : Bo.mix(Bo.surface, Bo.teal, 0.20)
                            border.color: Bo.teal; border.width: 1
                            // Only enabled when there's a rendered
                            // result on disk (the path is captured
                            // in the result JSON's `path` field).
                            property string outPath: {
                                try {
                                    var r = JSON.parse(qtobj.soundscape_result_json || "{}")
                                    return r.path || ""
                                } catch (e) { return "" }
                            }
                            opacity: outPath.length > 0 ? 1.0 : 0.4
                            enabled: outPath.length > 0
                            Row {
                                anchors.centerIn: parent
                                spacing: 4
                                Text {
                                    text: "＋"
                                    color: Bo.teal
                                    font.pixelSize: 13
                                    font.weight: Font.DemiBold
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                Text {
                                    text: "Add to Library"
                                    color: Bo.bone
                                    font.pixelSize: 9
                                    font.weight: Font.DemiBold
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                            }
                            MouseArea {
                                id: addMa
                                anchors.fill: parent
                                enabled: parent.enabled
                                onClicked: qtobj.add_soundscape_to_library(parent.outPath)
                            }
                        }
                        // Show which sink it'll play through, so the
                        // user understands where the audio is going.
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "→ " + (qtobj.current_driver_label || "default sink")
                            color: Bo.slate
                            font.pixelSize: 9
                            elide: Text.ElideMiddle
                            width: parent.parent
                                ? parent.parent.width - 96 - 72 - 130 - 40
                                : 150
                        }
                    }
                }
            }

            // ── Previous renders ──────────────────────────────────
            //
            // Accumulates every soundscape the user has rendered into
            // ~/Music/Soundscapes/, newest-first. Each row shows the
            // name, size, and an inline ▶ button to preview through
            // the current driver. This turns the page from a one-shot
            // tool into a curated soundscape collection — you can
            // re-listen to past renders, A/B different parameters
            // against the same source, build up a library over time.
            Item {
                width: parent.width
                height: prevHeader.height + prevList.implicitHeight + 16
                visible: prevList.count > 0

                Column {
                    anchors.fill: parent
                    spacing: 4

                    Row {
                        id: prevHeader
                        width: parent.width
                        spacing: Bo.gapSM
                        // Tracks which row (if any) is currently in
                        // its rename-input state. Set to the row's
                        // file path; "" means no row is renaming.
                        property string renamingPath: ""
                        property string renameDraft: ""
                        // Same idea for delete arming — set on first
                        // tap, cleared on confirm tap (which fires the
                        // delete) or after the 3s auto-disarm timer.
                        property string deletingPath: ""
                        Text {
                            text: "PREVIOUS RENDERS"
                            color: Bo.silver
                            font.pixelSize: 9
                            font.weight: Font.DemiBold
                            font.letterSpacing: 1.0
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Rectangle { width: 1; height: 1 }   // spacer
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: prevList.count + " loop" + (prevList.count === 1 ? "" : "s")
                            color: Bo.slate
                            font.pixelSize: 9
                        }
                    }

                    Repeater {
                        id: prevList
                        model: root.previousRenders

                        delegate: Rectangle {
                            width: prevHeader.width
                            height: 48
                            radius: 8
                            color: rowMa.pressed
                                ? Bo.surfaceHi
                                : (index % 2 === 0 ? Bo.surface : Bo.graphite)
                            border.color: Bo.hairline; border.width: 1

                            Row {
                                anchors.fill: parent
                                anchors.leftMargin: 12
                                anchors.rightMargin: 12
                                spacing: Bo.gapSM

                                Rectangle {
                                    width: 32; height: 32; radius: 16
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: playRowMa.pressed
                                        ? Bo.surfaceHi
                                        : Bo.mix(Bo.surface, "#7FB089", 0.30)
                                    border.color: "#7FB089"; border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        text: "▶"
                                        color: "#7FB089"
                                        font.pixelSize: 12
                                    }
                                    MouseArea {
                                        id: playRowMa
                                        anchors.fill: parent
                                        onClicked: {
                                            qtobj.driver_play_local_file(
                                                modelData.path,
                                                modelData.name)
                                        }
                                    }
                                }

                                Column {
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 1
                                    width: prevHeader.width - 32 - 32 - 12 - 12 - Bo.gapSM - 80
                                    Text {
                                        text: modelData.name
                                        color: Bo.bone
                                        font.pixelSize: 11
                                        font.weight: Font.DemiBold
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                    Text {
                                        text: (modelData.size_bytes / 1024).toFixed(1)
                                            + " KB"
                                        color: Bo.slate
                                        font.pixelSize: 9
                                    }
                                }

                                // ＋ "Add to Library" — same focused-
                                // scan trigger as the freshly-rendered
                                // result card. Lets the user back-fill
                                // existing renders into the catalog
                                // without re-rendering them.
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 32; height: 32; radius: 16
                                    color: addRowMa.pressed
                                        ? Bo.mix(Bo.surface, Bo.teal, 0.50)
                                        : Bo.mix(Bo.surface, Bo.teal, 0.20)
                                    border.color: Bo.teal; border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        text: "＋"
                                        color: Bo.teal
                                        font.pixelSize: 13
                                        font.weight: Font.DemiBold
                                    }
                                    MouseArea {
                                        id: addRowMa
                                        anchors.fill: parent
                                        // Stop click propagation so
                                        // the row's "refill input"
                                        // handler doesn't also fire.
                                        propagateComposedEvents: false
                                        onClicked: {
                                            qtobj.add_soundscape_to_library(modelData.path)
                                            mouse.accepted = true
                                        }
                                    }
                                }
                                // ✏ Rename — toggles an inline text
                                // field at the row's bottom. Confirm
                                // with Enter; cancel by tapping the
                                // pencil again or pressing Escape.
                                Rectangle {
                                    id: renameBtn
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 32; height: 32; radius: 16
                                    property bool active: prevHeader.renamingPath === modelData.path
                                    color: active
                                        ? Bo.mix(Bo.surface, Bo.mauve, 0.50)
                                        : (renameMa.pressed
                                            ? Bo.mix(Bo.surface, Bo.mauve, 0.40)
                                            : Bo.mix(Bo.surface, Bo.mauve, 0.18))
                                    border.color: Bo.mauve; border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        text: "✏"
                                        color: Bo.mauve
                                        font.pixelSize: 12
                                    }
                                    MouseArea {
                                        id: renameMa
                                        anchors.fill: parent
                                        propagateComposedEvents: false
                                        onClicked: {
                                            if (prevHeader.renamingPath === modelData.path) {
                                                prevHeader.renamingPath = ""
                                            } else {
                                                prevHeader.renamingPath = modelData.path
                                                // Pre-fill with the
                                                // current basename minus
                                                // extension so the user
                                                // can edit a label without
                                                // having to retype the
                                                // suffix.
                                                var n = modelData.name
                                                var dot = n.lastIndexOf(".")
                                                prevHeader.renameDraft = dot > 0
                                                    ? n.substring(0, dot)
                                                    : n
                                            }
                                            mouse.accepted = true
                                        }
                                    }
                                }
                                // 🗑 Delete — single-tap arms (button
                                // turns red); second tap within ~3s
                                // confirms. Avoids modal dialogs while
                                // still requiring deliberate intent.
                                Rectangle {
                                    id: deleteBtn
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 32; height: 32; radius: 16
                                    property bool armed: prevHeader.deletingPath === modelData.path
                                    color: armed
                                        ? Bo.terracotta   // terracotta — armed
                                        : (deleteMa.pressed
                                            ? Bo.mix(Bo.surface, Bo.terracotta, 0.50)
                                            : Bo.mix(Bo.surface, Bo.terracotta, 0.18))
                                    border.color: Bo.terracotta; border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        text: armed ? "?" : "✕"
                                        color: armed ? Bo.bone : Bo.terracotta
                                        font.pixelSize: armed ? 12 : 14
                                        font.weight: Font.DemiBold
                                    }
                                    // Auto-disarm after 3s of inactivity.
                                    Timer {
                                        running: deleteBtn.armed
                                        interval: 3000
                                        repeat: false
                                        onTriggered: prevHeader.deletingPath = ""
                                    }
                                    MouseArea {
                                        id: deleteMa
                                        anchors.fill: parent
                                        propagateComposedEvents: false
                                        onClicked: {
                                            if (prevHeader.deletingPath === modelData.path) {
                                                // Confirm.
                                                qtobj.delete_soundscape(modelData.path)
                                                prevHeader.deletingPath = ""
                                                prevHeader.renamingPath = ""
                                            } else {
                                                // Arm.
                                                prevHeader.deletingPath = modelData.path
                                            }
                                            mouse.accepted = true
                                        }
                                    }
                                }
                            }
                            // Inline rename input — appears below the
                            // row when this entry is being renamed.
                            // Loader rather than always-instantiated
                            // so idle rows don't carry an extra
                            // TextInput in the scene graph.
                            Loader {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.bottom
                                anchors.topMargin: 4
                                active: prevHeader.renamingPath === modelData.path
                                visible: active
                                sourceComponent: Rectangle {
                                    height: 36
                                    color: Bo.mix(Bo.surface, Bo.mauve, 0.10)
                                    border.color: Bo.mauve
                                    border.width: 1
                                    radius: 4
                                    Row {
                                        anchors.fill: parent
                                        anchors.margins: 4
                                        spacing: 6
                                        TextInput {
                                            id: renameInput
                                            anchors.verticalCenter: parent.verticalCenter
                                            width: parent.width - 86
                                            color: Bo.bone
                                            font.pixelSize: 11
                                            text: prevHeader.renameDraft
                                            selectByMouse: true
                                            onTextChanged: prevHeader.renameDraft = text
                                            Component.onCompleted: {
                                                forceActiveFocus()
                                                selectAll()
                                            }
                                            Keys.onPressed: {
                                                if (event.key === Qt.Key_Return
                                                    || event.key === Qt.Key_Enter) {
                                                    qtobj.rename_soundscape(
                                                        modelData.path,
                                                        prevHeader.renameDraft)
                                                    prevHeader.renamingPath = ""
                                                    event.accepted = true
                                                } else if (event.key === Qt.Key_Escape) {
                                                    prevHeader.renamingPath = ""
                                                    event.accepted = true
                                                }
                                            }
                                        }
                                        Rectangle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            width: 76; height: 26; radius: 13
                                            color: confirmRenameMa.pressed
                                                ? Bo.mix(Bo.surface, Bo.mauve, 0.50)
                                                : Bo.mix(Bo.surface, Bo.mauve, 0.30)
                                            border.color: Bo.mauve
                                            border.width: 1
                                            Text {
                                                anchors.centerIn: parent
                                                text: "Rename"
                                                color: Bo.bone
                                                font.pixelSize: 9
                                                font.weight: Font.DemiBold
                                            }
                                            MouseArea {
                                                id: confirmRenameMa
                                                anchors.fill: parent
                                                onClicked: {
                                                    qtobj.rename_soundscape(
                                                        modelData.path,
                                                        prevHeader.renameDraft)
                                                    prevHeader.renamingPath = ""
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            MouseArea {
                                id: rowMa
                                anchors.fill: parent
                                // Whole-row tap re-fills the output field
                                // so the user can re-render with new params
                                // against the same name. Doesn't auto-play
                                // — the inline ▶ button does that.
                                onClicked: {
                                    outputField.text = modelData.path
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Previous-renders model. Lazy-refresh on page show + when a new
    // render finishes (soundscape_running goes false). Stored as a
    // JS array so the Repeater can iterate cheaply.
    property var previousRenders: []
    function refreshPrevious() {
        try {
            previousRenders = JSON.parse(qtobj.list_soundscapes() || "[]")
        } catch (e) {
            previousRenders = []
        }
    }
    Component.onCompleted: refreshPrevious()
    Connections {
        target: qtobj
        function onSoundscape_runningChanged() {
            if (!qtobj.soundscape_running) {
                // Render finished — refresh after a short delay so
                // the file fully flushes to disk first.
                refreshTimer.start()
            }
        }
    }
    Timer {
        id: refreshTimer
        interval: 500
        repeat: false
        onTriggered: refreshPrevious()
    }
}
