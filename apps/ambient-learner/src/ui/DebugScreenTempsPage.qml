import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

// Debug page: visualizes the screen-temperature bitfield we publish
// to $XDG_RUNTIME_DIR/ambient-learner/screen-temps.bin.
//
// Reads the file via `backend.get_screen_temp_frame()` at a steady
// cadence. Every cell shows the temperature in °C, color-coded by
// the same `Theme.tempColor()` scale used elsewhere.
//
// Surfaces parse errors prominently — if the binary file is
// missing, the magic is wrong, or the body length doesn't match,
// the error string is what shows up where the grid would be. That
// makes wire-format regressions immediately obvious during dev.
Item {
    id: root
    property bool isMobile: true

    // Parsed state. `errorMessage` and `frame` are mutually exclusive:
    // exactly one is non-empty/non-null at any given time.
    property string errorMessage: ""
    property var frame: null

    // Auto-tuned color range. We let the publisher tell us the
    // min/max it observed and use that as the normalization range
    // so the colors stretch to fill whatever band the device is
    // currently sitting in.
    property real minC: 20.0
    property real maxC: 40.0

    Timer {
        id: refreshTimer
        interval: 250 // matches the publisher's 4 Hz rate
        running: root.visible
        repeat: true
        triggeredOnStart: true
        onTriggered: root._refresh()
    }

    function _refresh() {
        var raw = backend.get_screen_temp_frame()
        var parsed
        try {
            parsed = JSON.parse(raw)
        } catch (e) {
            root.errorMessage = "JSON parse: " + e
            root.frame = null
            return
        }
        if (!parsed.ok) {
            root.errorMessage = parsed.error || "unknown error"
            root.frame = null
            return
        }
        root.errorMessage = ""
        root.frame = parsed
        root.minC = parsed.min_c
        root.maxC = parsed.max_c
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        SectionHeader {
            Layout.fillWidth: true
            title: "DEBUG · SCREEN TEMPS"
            // Use the model-section color — this is a model-internals page.
            accentColor: Theme.sectionModel
        }

        // Status strip showing meta info or the parse error.
        Rectangle {
            Layout.fillWidth: true
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            Layout.preferredHeight: 64
            radius: Theme.radiusCard
            color: root.errorMessage.length > 0 ? "#3a1010" : Theme.surface
            Behavior on color { ColorAnimation { duration: Theme.durStandard } }

            // Error mode: the message is the whole content.
            Label {
                anchors.fill: parent
                anchors.margins: 12
                visible: root.errorMessage.length > 0
                text: "⚠  " + root.errorMessage
                color: "#ffb0b0"
                wrapMode: Text.WordWrap
                verticalAlignment: Text.AlignVCenter
                font.pixelSize: Theme.fontBody
            }

            // Normal mode: grid dims, generation, age, color range.
            GridLayout {
                anchors.fill: parent
                anchors.margins: 12
                visible: root.errorMessage.length === 0 && root.frame !== null
                columns: 4
                rowSpacing: 2
                columnSpacing: 16

                Label { text: "Grid"; color: Theme.textDim; font.pixelSize: Theme.fontCaption }
                Label { text: "Gen";  color: Theme.textDim; font.pixelSize: Theme.fontCaption }
                Label { text: "Age";  color: Theme.textDim; font.pixelSize: Theme.fontCaption }
                Label { text: "Range";color: Theme.textDim; font.pixelSize: Theme.fontCaption }

                Label {
                    text: root.frame !== null
                          ? root.frame.cols + " × " + root.frame.rows
                          : "—"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                Label {
                    text: root.frame !== null ? root.frame.generation : "—"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                Label {
                    text: root.frame !== null
                          ? root.frame.age_ms + " ms"
                          : "—"
                    color: root.frame !== null && root.frame.age_ms > 1000
                           ? Theme.warning
                           : Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                Label {
                    text: root.frame !== null
                          ? root.minC.toFixed(1) + "° – " + root.maxC.toFixed(1) + "°"
                          : "—"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
            }
        }

        Item { Layout.preferredHeight: 8 }

        // The grid itself. Wraps in a Rectangle so the surface color
        // is visible between cells (small gaps look intentional).
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            Layout.bottomMargin: Theme.paddingPage
            radius: Theme.radiusCard
            color: Theme.surface
            clip: true

            // Placeholder shown while we haven't received any data
            // yet (no errors, just an empty frame).
            Label {
                anchors.centerIn: parent
                visible: root.frame === null && root.errorMessage.length === 0
                text: "Waiting for first frame…"
                color: Theme.textDim
                font.pixelSize: Theme.fontBody
            }

            // The grid. We use a Grid (not GridLayout) so that all
            // cells are the same physical size regardless of content
            // — GridLayout would try to size each column to its
            // widest cell, which would distort the spatial
            // representation we're trying to show.
            Grid {
                id: tileGrid
                anchors.fill: parent
                anchors.margins: 6
                visible: root.frame !== null
                columns: root.frame !== null ? root.frame.cols : 1
                rows: root.frame !== null ? root.frame.rows : 1
                spacing: 2

                // Compute cell size from the available area. We do
                // this once and let every cell pick it up — much
                // cheaper than per-cell layout passes.
                property real cellW: root.frame !== null
                    ? (width - spacing * (columns - 1)) / columns
                    : 0
                property real cellH: root.frame !== null
                    ? (height - spacing * (rows - 1)) / rows
                    : 0

                Repeater {
                    // length is cols * rows; falsy frame → empty model
                    model: root.frame !== null ? root.frame.cells : []

                    delegate: Rectangle {
                        width: tileGrid.cellW
                        height: tileGrid.cellH
                        radius: 4

                        // `modelData` is the f32 value for this cell.
                        // Theme.tempColor maps it to a hue.
                        color: Theme.tempColor(modelData)
                        Behavior on color {
                            ColorAnimation { duration: Theme.durStandard }
                        }

                        Label {
                            anchors.centerIn: parent
                            // Show one decimal: enough to see
                            // gradients, not so much that the
                            // numbers crowd the cell.
                            text: modelData.toFixed(1)

                            // Pick a text color that's readable
                            // against the cell color. Bright cells
                            // (>30°C) get dark text; cool cells get
                            // light text. The threshold matches the
                            // Theme.tempColor transition near 24°C.
                            color: modelData > 24 ? "#1b1b1b" : "#ffffff"

                            font.pixelSize: Math.min(
                                tileGrid.cellW * 0.30,
                                tileGrid.cellH * 0.30,
                                14
                            )
                            font.bold: true
                        }
                    }
                }
            }
        }
    }
}
