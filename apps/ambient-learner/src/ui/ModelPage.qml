import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

Item {
    id: root
    property bool isMobile: true

    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            width: parent.width
            spacing: Theme.spacingDefault

            SectionHeader {
                title: "MODEL"
                accentColor: Theme.sectionModel
            }

            // ─── Sensor sphere (Fibonacci-spiral 2D heat map) ─────────────
            Rectangle {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                Layout.preferredHeight: 280
                radius: Theme.radiusCard
                color: Theme.surface

                Canvas {
                    id: sphereCanvas
                    anchors.fill: parent
                    anchors.margins: 8

                    Connections {
                        target: backend
                        function onSensors_jsonChanged() { sphereCanvas.requestPaint() }
                        function onTemperatureChanged() { sphereCanvas.requestPaint() }
                    }

                    onPaint: {
                        var ctx = getContext("2d")
                        ctx.clearRect(0, 0, width, height)

                        var sensors = []
                        try { sensors = JSON.parse(backend.sensors_json || "[]") }
                        catch (e) { sensors = [] }

                        var cx = width / 2, cy = height / 2
                        var r = Math.min(cx, cy) - 20

                        // Background ring
                        ctx.strokeStyle = "#444"
                        ctx.lineWidth = 1
                        ctx.beginPath()
                        ctx.arc(cx, cy, r, 0, 2 * Math.PI)
                        ctx.stroke()

                        var n = sensors.length
                        for (var i = 0; i < n; i++) {
                            var t = sensors[i]
                            var phi = i * 2.39996  // golden angle
                            var dist = r * Math.sqrt((i + 0.5) / Math.max(n, 1))
                            var x = cx + Math.cos(phi) * dist
                            var y = cy + Math.sin(phi) * dist

                            // Sensor temps are *device* temps (30–80°C) so
                            // the live-page palette doesn't fit; pick by
                            // a separate hot/cold scheme.
                            var color = "#888"
                            if (t < 30)      color = "#64b5f6"
                            else if (t < 50) color = "#81c784"
                            else if (t < 70) color = "#ffb74d"
                            else             color = "#ef5350"

                            ctx.fillStyle = color
                            ctx.beginPath()
                            ctx.arc(x, y, 5, 0, 2 * Math.PI)
                            ctx.fill()
                        }

                        // Center: predicted ambient
                        ctx.fillStyle = Theme.accent
                        ctx.font = "bold 28px sans-serif"
                        ctx.textAlign = "center"
                        ctx.textBaseline = "middle"
                        ctx.fillText(backend.temperature.toFixed(1) + "°C", cx, cy)
                    }
                }
            }

            // ─── Model statistics ─────────────────────────────────────────
            Card {
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage

                Label {
                    text: "📊 Model statistics"
                    font.pixelSize: Theme.fontHeading
                    font.bold: true
                    color: Theme.textPrimary
                }

                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 24
                    rowSpacing: 6

                    Label { text: "Active model:";  color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: backend.model_type; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Linear cal.:";   color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: backend.linear_calibrations; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Neural cal.:";   color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label {
                        text: backend.neural_calibrations
                              + (backend.neural_calibrations < 5
                                 ? "  (need 5 for neural)"
                                 : "")
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }

                    Label { text: "Confidence:";    color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: (backend.confidence * 100).toFixed(1) + "%"; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }
                }
            }

            // ─── Device state (informs the neural field) ──────────────────
            Card {
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage

                Label {
                    text: "⚙️  Device state"
                    font.pixelSize: Theme.fontHeading
                    font.bold: true
                    color: Theme.textPrimary
                }

                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 24
                    rowSpacing: 6

                    Label { text: "CPU load:";       color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: (backend.cpu_load * 100).toFixed(0) + "%"; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Battery:";        color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label {
                        text: (backend.battery_level * 100).toFixed(0) + "%"
                              + (backend.charging ? "  ⚡ charging" : "")
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }
                }
            }

            // ─── Recent calibrations (audit log) ─────────────────────
            //
            // Shows who has been calibrating this model. Every entry
            // includes the source attribution — "qt-ui" for the
            // local app, "iced-ui" or "gtk-ui" for the other
            // backends, or an arbitrary string from a D-Bus / XDG
            // inbox caller. If something on your system has been
            // submitting calibrations you don't expect, it'll show
            // up here.
            Card {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    RowLayout {
                        Layout.fillWidth: true
                        Label {
                            text: "🧾  Recent calibrations"
                            font.pixelSize: Theme.fontHeading
                            font.bold: true
                            color: Theme.textPrimary
                            Layout.fillWidth: true
                        }
                        Label {
                            text: calLog.count + " entries"
                            color: Theme.textDim
                            font.pixelSize: Theme.fontCaption
                        }
                    }

                    Label {
                        text: "Each entry shows who calibrated the model and "
                              + "what they said the ambient was. \"qt-ui\" "
                              + "is this app; other sources are external apps."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        color: Theme.textDim
                        font.pixelSize: Theme.fontCaption
                    }

                    // The log itself. We use ListView for built-in
                    // virtualization; a 200-entry list still renders
                    // smoothly. Each row is a single line with
                    // target temp, the source, and a "how long ago"
                    // label.
                    ListView {
                        id: calLog
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.min(
                            count * 36 + 8,  // 36 px per row
                            240               // capped so it doesn't dominate
                        )
                        interactive: count * 36 + 8 > 240
                        clip: true
                        spacing: 2
                        model: ListModel { id: calLogModel }

                        delegate: RowLayout {
                            width: calLog.width
                            spacing: 12

                            // Source pill — first thing the eye lands
                            // on. Coloring distinguishes "this app"
                            // (default color) from "external" (warm
                            // amber to draw attention).
                            Rectangle {
                                Layout.preferredHeight: 22
                                Layout.preferredWidth: srcLabel.implicitWidth + 16
                                radius: 11
                                color: {
                                    if (source === "qt-ui"
                                        || source === "gtk-ui"
                                        || source === "iced-ui") {
                                        return Qt.rgba(0.45, 0.55, 0.70, 0.30)
                                    }
                                    if (!source || source === "") {
                                        return Qt.rgba(0.50, 0.50, 0.50, 0.25)
                                    }
                                    return Qt.rgba(0.78, 0.60, 0.30, 0.35)
                                }

                                Label {
                                    id: srcLabel
                                    anchors.centerIn: parent
                                    text: source ? source : "(unknown)"
                                    color: Theme.textPrimary
                                    font.pixelSize: Theme.fontCaption
                                }
                            }

                            Label {
                                text: target.toFixed(1) + "°C"
                                color: Theme.textPrimary
                                font.pixelSize: Theme.fontBody
                                font.bold: true
                            }

                            Label {
                                // Show the delta from the device's
                                // reading at the time, so the user
                                // can see "calibrated 22.5 (device
                                // showed 24.1, off by 1.6)".
                                text: {
                                    var delta = target - mean_sensors
                                    var sign = delta >= 0 ? "+" : ""
                                    return "(device " + mean_sensors.toFixed(1)
                                         + "°C; " + sign + delta.toFixed(1) + ")"
                                }
                                color: Theme.textDim
                                font.pixelSize: Theme.fontCaption
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                            }

                            Label {
                                text: backend.format_age(age_seconds)
                                color: Theme.textDim
                                font.pixelSize: Theme.fontCaption
                            }
                        }
                    }

                    Label {
                        visible: calLogModel.count === 0
                        text: "No calibrations yet. Use Calibrate on the "
                              + "Live tab to record one."
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                        Layout.topMargin: 8
                    }
                }
            }

            // Refresh the log on a timer. The data only changes on
            // user action (or external IPC); we don't need to poll
            // at 60 Hz. Once a second is more than enough.
            Timer {
                interval: 1000
                running: root.visible
                repeat: true
                triggeredOnStart: true
                onTriggered: {
                    var raw = backend.get_recent_calibrations()
                    var parsed = []
                    try {
                        parsed = JSON.parse(raw)
                    } catch (e) {
                        console.warn("ModelPage: bad cal-log JSON:", e)
                        return
                    }
                    // Naive diff: if the count or the newest entry's
                    // unix_when changed, rebuild. This avoids
                    // rebuilding the ListModel on every tick when
                    // nothing has changed (rebuilding 50 rows at
                    // 1Hz isn't free — they retrigger animations
                    // and lose scroll position).
                    var changed = parsed.length !== calLogModel.count
                    if (!changed && parsed.length > 0 && calLogModel.count > 0) {
                        changed = parsed[0].unix_when !== calLogModel.get(0).unix_when
                    }
                    if (!changed) return

                    calLogModel.clear()
                    for (var i = 0; i < parsed.length; i++) {
                        calLogModel.append(parsed[i])
                    }
                }
            }

            Item { Layout.preferredHeight: Theme.paddingPage }
        }
    }
}
