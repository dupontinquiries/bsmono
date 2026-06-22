// qml/NowPlayingBar.qml
//
// Compact now-playing strip for the Sound page. Shows the current track
// and artist (or just the source name for line-in/radio), with transport
// controls. When playing, a tiny equalizer animates beside the title —
// a quiet sign of life consistent with the rest of the motion design.

import QtQuick 2.15
import "."

Rectangle {
    id: root
    property string title: ""
    property string artist: ""
    property string source: ""
    property bool playing: false
    property color accent: "#C8A04B"

    signal playPause()
    signal next()
    signal prev()

    height: 60
    radius: Bo.cardRadius
    color: Bo.surface
    border.color: Bo.hairline
    border.width: 1

    // Gentle entrance
    opacity: 0
    Component.onCompleted: fadeIn.start()
    NumberAnimation { id: fadeIn; target: root; property: "opacity"; to: 1; duration: Bo.slow }

    Row {
        anchors.left: parent.left
        anchors.leftMargin: Bo.gapMD
        anchors.verticalCenter: parent.verticalCenter
        anchors.right: controls.left
        anchors.rightMargin: Bo.gapSM
        spacing: Bo.gapSM

        // Mini equalizer (animates only while playing)
        Row {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            visible: root.playing
            Repeater {
                model: 3
                delegate: Rectangle {
                    width: 2.5; radius: 1.25
                    color: root.accent
                    anchors.verticalCenter: parent.verticalCenter
                    height: 6
                    SequentialAnimation on height {
                        running: root.playing
                        loops: Animation.Infinite
                        NumberAnimation { to: 8 + index*5; duration: 320 + index*90; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 5; duration: 280 + index*70; easing.type: Easing.InOutSine }
                    }
                }
            }
        }

        Column {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            width: parent.width - (root.playing ? 18 : 0)
            Text {
                width: parent.width
                text: root.title.length > 0 ? root.title
                      : (root.source.length > 0 ? root.source : "Nothing playing")
                color: Bo.bone
                font.pixelSize: Bo.bodySize
                font.weight: Font.Medium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                visible: root.artist.length > 0 || root.source.length > 0
                text: root.artist.length > 0 ? root.artist : root.source
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                elide: Text.ElideRight
            }

            // Key/BPM chip — driven by qtobj.np_bpm / np_camelot, populated
            // by the worker on load_key_bpm(id). Hidden when bpm is 0 (no
            // analysis available, or analysis feature off at build time).
            Row {
                spacing: 6
                visible: qtobj.np_bpm > 0
                Rectangle {
                    height: 16; radius: 8
                    width: bpmText.implicitWidth + 12
                    color: Bo.mix(Bo.surface, root.accent, 0.18)
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        id: bpmText
                        anchors.centerIn: parent
                        text: Math.round(qtobj.np_bpm) + " BPM"
                        color: Bo.silver
                        font.pixelSize: 9
                        font.weight: Font.DemiBold
                        font.letterSpacing: 0.5
                    }
                }
                Rectangle {
                    visible: qtobj.np_camelot && qtobj.np_camelot.length > 0
                    height: 16; radius: 8
                    width: keyText.implicitWidth + 12
                    color: Bo.mix(Bo.surface, root.accent, 0.18)
                    border.color: root.accent; border.width: 1
                    Text {
                        id: keyText
                        anchors.centerIn: parent
                        text: qtobj.np_camelot
                            + (qtobj.np_key_text && qtobj.np_key_text.length > 0
                                  ? "  " + qtobj.np_key_text : "")
                        color: root.accent
                        font.pixelSize: 9
                        font.weight: Font.DemiBold
                        font.letterSpacing: 0.5
                    }
                }

                // Active source chip. Shows the device's current
                // source (e.g. "Spotify", "Network", "Line-in") with
                // a colored border that warns when DLNA URI playback
                // won't work. A per-family glyph makes the source
                // recognizable at a glance — green network glyph for
                // DLNA-capable, amber speaker for line-in, etc.
                // Tapping the chip triggers a fresh probe.
                //
                // Color semantics:
                //   green border  = source accepts URI playback
                //   amber border  = source is something else (we know what)
                //   grey  border  = unknown / not yet probed
                //
                // Glyph mapping (Unicode, no external icon assets):
                //   network_dlna    → ⌘  (network/discovery)
                //   spotify         → ♫  (musical notes)
                //   airplay         → △  (triangle, evokes AirPlay)
                //   bluetooth       → ✦  (4-point star)
                //   linein          → ◉  (filled circle, jack-style)
                //   tv              → ◧  (TV-screen shape)
                //   internet_radio  → ◌  (broadcast-style ring)
                //   other_streaming → ▶  (play triangle)
                //   unknown         → ?  (literal question mark)
                Rectangle {
                    visible: qtobj.device_source_label
                          && qtobj.device_source_label.length > 0
                    height: 16; radius: 8
                    width: sourceContent.implicitWidth + 14
                    color: Bo.mix(Bo.surface,
                        qtobj.device_source_can_play ? "#4a7c59"
                            : (qtobj.device_source_family === "unknown"
                                  ? Bo.silver : "#B58853"),
                        0.20)
                    border.color: qtobj.device_source_can_play ? "#4a7c59"
                        : (qtobj.device_source_family === "unknown"
                              ? Bo.hairline : "#B58853")
                    border.width: 1

                    // Map family → glyph. Inline ternary because QML
                    // doesn't have switch expressions, but the chain
                    // is short and easy to read.
                    function glyphFor(family) {
                        if (family === "network_dlna")    return "⌘"
                        if (family === "spotify")         return "♫"
                        if (family === "airplay")         return "△"
                        if (family === "bluetooth")       return "✦"
                        if (family === "linein")          return "◉"
                        if (family === "tv")              return "◧"
                        if (family === "internet_radio")  return "◌"
                        if (family === "other_streaming") return "▶"
                        return "?"
                    }

                    Row {
                        id: sourceContent
                        anchors.centerIn: parent
                        spacing: 4
                        Text {
                            text: parent.parent.glyphFor(qtobj.device_source_family || "unknown")
                            color: qtobj.device_source_can_play ? "#4a7c59"
                                : (qtobj.device_source_family === "unknown"
                                      ? Bo.silver : "#B58853")
                            font.pixelSize: 10
                            font.weight: Font.DemiBold
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Text {
                            id: sourceText
                            text: qtobj.device_source_label || ""
                            color: Bo.bone
                            font.pixelSize: 9
                            font.weight: Font.DemiBold
                            font.letterSpacing: 0.4
                            anchors.verticalCenter: parent.verticalCenter
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: qtobj.refresh_active_source()
                    }
                }
            }
        }
    }

    // Transport controls
    Row {
        id: controls
        anchors.right: parent.right
        anchors.rightMargin: Bo.gapSM
        anchors.verticalCenter: parent.verticalCenter
        spacing: 4

        TransportBtn { glyph: "⏮"; accent: root.accent; onTapped: root.prev() }
        TransportBtn {
            glyph: root.playing ? "⏸" : "▶"
            accent: root.accent; filled: true
            onTapped: root.playPause()
        }
        TransportBtn { glyph: "⏭"; accent: root.accent; onTapped: root.next() }
    }

    // Inline tiny transport button
    component TransportBtn: Rectangle {
        property string glyph: ""
        property bool filled: false
        property color accent: "#C8A04B"
        signal tapped()
        width: 36; height: 36; radius: 18
        color: filled ? accent : (ta.pressed ? Bo.surfaceHi : "transparent")
        border.color: filled ? accent : Bo.hairline
        border.width: 1
        scale: ta.pressed ? 0.9 : 1.0
        Behavior on scale { NumberAnimation { duration: Bo.instant } }
        Text {
            anchors.centerIn: parent
            text: glyph
            color: filled ? Bo.graphite : Bo.bone
            font.pixelSize: 13
        }
        MouseArea { id: ta; anchors.fill: parent; onClicked: tapped() }
    }
}
