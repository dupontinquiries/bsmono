import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

Item {
    id: root
    property bool isMobile: true

    // Combined seasonal + temperature accent. We blend the temperature
    // color with the seasonal accent so the temp dominates but the
    // seasonal palette still shows through.
    function blendedAccent(t, season) {
        return Qt.tint(Theme.tempColor(t), Qt.rgba(0, 0, 0, 0.0));
    }

    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            width: parent.width
            spacing: Theme.spacingDefault

            SectionHeader {
                title: "LIVE"
                accentColor: Theme.moodAccent(backend.season)
            }

            // ─── Hero temperature card ────────────────────────────────────
            Rectangle {
                id: heroCard
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                Layout.preferredHeight: 260
                radius: Theme.radiusLargeCard
                clip: true

                // Tilt-driven shadow / depth offset
                transform: Rotation {
                    origin.x: heroCard.width / 2
                    origin.y: heroCard.height / 2
                    angle: backend.tilt_x * 1.2  // very small, max ±1°
                    axis { x: 0; y: 1; z: 0 }
                }

                // Base gradient — blends temperature color with surface.
                gradient: Gradient {
                    GradientStop {
                        position: 0.0
                        color: Qt.tint(Theme.surface,
                            Qt.rgba(
                                Theme.tempColor(backend.temperature).r,
                                Theme.tempColor(backend.temperature).g,
                                Theme.tempColor(backend.temperature).b,
                                0.20
                            )
                        )
                        Behavior on color { ColorAnimation { duration: Theme.durStandard } }
                    }
                    GradientStop {
                        position: 1.0
                        color: Theme.surfaceAlt
                        Behavior on color { ColorAnimation { duration: Theme.durStandard } }
                    }
                }

                // Subtle radial highlight that follows the tilt direction.
                // Built with a small absolutely-positioned circle that
                // shifts position with tilt — gives the card a sense of
                // physical surface curvature.
                Rectangle {
                    id: highlight
                    width: parent.width * 0.6
                    height: width
                    radius: width / 2
                    color: Qt.rgba(1, 1, 1, 0.06)
                    x: parent.width * 0.5 - width / 2 + backend.tilt_x * 40
                    y: parent.height * 0.25 - height / 2 + backend.tilt_y * 40
                    Behavior on x { NumberAnimation { duration: Theme.durStandard; easing.type: Easing.OutCubic } }
                    Behavior on y { NumberAnimation { duration: Theme.durStandard; easing.type: Easing.OutCubic } }
                }

                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 8

                    // Big temperature reading.
                    //
                    // Two design choices worth calling out:
                    //
                    //   1. We animate the *displayed value*, not the
                    //      Label's scale. A scale animation is a layout
                    //      mutation — the column repacks every frame
                    //      during the pop, which causes the surrounding
                    //      content (ProgressBar, sensor-health row) to
                    //      shimmy. Animating the number itself is what
                    //      most polished apps do for live readouts.
                    //
                    //   2. Color is tweened by `Behavior on color`. The
                    //      number is tweened by `Behavior on
                    //      displayedTemp`. Both run at the same
                    //      duration so they feel like one motion.
                    Label {
                        id: bigTemp
                        Layout.alignment: Qt.AlignHCenter
                        font.pixelSize: root.isMobile ? Theme.fontDisplay : 56
                        font.bold: true

                        // Internal state: the value we're currently
                        // *showing*. A Behavior tweens it toward the
                        // backend value over `durStandard`.
                        property real displayedTemp: backend.temperature
                        Behavior on displayedTemp {
                            NumberAnimation {
                                duration: Theme.durStandard
                                easing.type: Easing.OutCubic
                            }
                        }

                        // Whenever the backend value updates, retarget
                        // the internal value. The Behavior interpolates.
                        Connections {
                            target: backend
                            function onTemperatureChanged() {
                                bigTemp.displayedTemp = backend.temperature
                            }
                        }

                        text: bigTemp.displayedTemp.toFixed(1) + "°C"
                        color: Theme.tempColor(bigTemp.displayedTemp)
                        Behavior on color {
                            ColorAnimation { duration: Theme.durStandard }
                        }
                    }

                    // Confidence "breathing" bar
                    ProgressBar {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 200
                        from: 0
                        to: 1
                        value: backend.confidence
                        Behavior on value { NumberAnimation { duration: Theme.durStandard } }
                    }

                    Label {
                        Layout.alignment: Qt.AlignHCenter
                        text: "Confidence: " + (backend.confidence * 100).toFixed(0) + "%"
                        font.pixelSize: Theme.fontBody
                        color: Theme.textSecondary
                    }

                    // Sensor health pill with a breathing dot.
                    RowLayout {
                        Layout.alignment: Qt.AlignHCenter
                        spacing: 8
                        BreathingDot {
                            color: Theme.healthColor(backend.sensor_health)
                            running: backend.sensor_health === "ok"
                        }
                        Label {
                            text: Theme.healthLabel(backend.sensor_health).replace("●", "").replace("○", "").replace("⚠", "").trim()
                            color: Theme.healthColor(backend.sensor_health)
                            font.pixelSize: Theme.fontBody
                            Behavior on color { ColorAnimation { duration: Theme.durStandard } }
                        }
                    }
                }
            }

            // ─── Status grid ──────────────────────────────────────────────
            Card {
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage

                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 24
                    rowSpacing: 6

                    Label { text: "Model:";        color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: backend.model_type; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Calibrations:"; color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label {
                        text: backend.calibration_count
                              + (backend.pending_calibrations > 0
                                 ? "  (+" + backend.pending_calibrations + " pending)"
                                 : "")
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }

                    Label { text: "Sensors:";      color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: backend.sensor_count; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Location:";     color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label { text: backend.location || "—"; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Last reading:"; color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label {
                        text: backend.format_age(backend.sensor_age_seconds)
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }

                    Label { text: "Mood:";         color: Theme.textDim;     font.pixelSize: Theme.fontBody }
                    Label {
                        text: backend.season + " · " + backend.day_phase
                        color: Theme.moodAccent(backend.season)
                        font.pixelSize: Theme.fontBody
                        Behavior on color { ColorAnimation { duration: Theme.durSlow } }
                    }
                }
            }

            // ─── Environment card (only if any sensor is present) ─────────
            Card {
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                visible: backend.has_pressure
                         || backend.has_humidity
                         || backend.has_light
                         || backend.has_accelerometer

                Label {
                    text: "🌬️  Environment"
                    font.pixelSize: Theme.fontHeading
                    font.bold: true
                    color: Theme.textPrimary
                }

                // Pressure gauge — rich animated visual that's a
                // Qt strength. Replaces the old plain-text row;
                // the numerical value is overlaid on the gauge so
                // we don't lose information.
                PressureGauge {
                    visible: backend.has_pressure
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: 4
                    hpa: backend.pressure_hpa
                    trendHpa: backend.pressure_trend_hpa
                }

                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 24
                    rowSpacing: 6

                    // Pressure trend label (mood color). The gauge
                    // above shows the value + arrow direction;
                    // this row keeps the textual mood label
                    // ("rising", "stable", etc.) for users who
                    // prefer reading words.
                    Label {
                        visible: backend.has_pressure
                        text: "Trend:"
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                    }
                    Label {
                        visible: backend.has_pressure
                        text: Theme.pressureMoodLabel(backend.pressure_trend_hpa)
                        color: Theme.pressureMoodColor(backend.pressure_trend_hpa)
                        font.pixelSize: Theme.fontBody
                        Behavior on color { ColorAnimation { duration: Theme.durStandard } }
                    }

                    // Humidity
                    Label {
                        visible: backend.has_humidity
                        text: "Humidity:"
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                    }
                    Label {
                        visible: backend.has_humidity
                        text: backend.humidity_pct.toFixed(0) + " %"
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }

                    // Ambient light
                    Label {
                        visible: backend.has_light
                        text: "Light:"
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                    }
                    Label {
                        visible: backend.has_light
                        text: backend.light_level.toFixed(1) + " " + backend.light_unit
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }

                    // Tilt
                    Label {
                        visible: backend.has_accelerometer
                        text: "Tilt:"
                        color: Theme.textDim
                        font.pixelSize: Theme.fontBody
                    }
                    Label {
                        visible: backend.has_accelerometer
                        text: backend.accel_orientation || "—"
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontBody
                    }
                }
            }

            // ─── Forecast card ────────────────────────────────────────────
            Card {
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage

                Label {
                    text: "🔮 Forecast"
                    font.pixelSize: Theme.fontHeading
                    font.bold: true
                    color: Theme.textPrimary
                }

                RowLayout {
                    Layout.fillWidth: true

                    ColumnLayout {
                        Layout.fillWidth: true
                        Label { text: "1 hour";  font.pixelSize: Theme.fontCaption; color: Theme.textDim }
                        Label {
                            text: backend.forecast_1h.length > 0 ? backend.forecast_1h : "—"
                            font.pixelSize: Theme.fontTitle
                            font.bold: true
                            color: Theme.textPrimary
                        }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        Label { text: "24 hours"; font.pixelSize: Theme.fontCaption; color: Theme.textDim }
                        Label {
                            text: backend.forecast_24h.length > 0 ? backend.forecast_24h : "—"
                            font.pixelSize: Theme.fontTitle
                            font.bold: true
                            color: Theme.textPrimary
                        }
                    }
                }
            }

            // ─── Action buttons ───────────────────────────────────────────
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                spacing: 8

                Button {
                    Layout.fillWidth: true
                    Layout.preferredHeight: Theme.minTouchSize
                    text: "🎯 Calibrate"
                    onClicked: {
                        calibrationDialog.source = "user"
                        calibrationDialog.title = "Calibrate model"
                        calibrationDialog.subtitle = "Enter the actual room temperature. " +
                                                    "This trains the model."
                        calibrationDialog.open()
                    }
                }

                Button {
                    Layout.fillWidth: true
                    Layout.preferredHeight: Theme.minTouchSize
                    text: "📝 Add data point"
                    onClicked: {
                        calibrationDialog.source = "observation"
                        calibrationDialog.title = "Log observation"
                        calibrationDialog.subtitle = "Record a measured temperature " +
                                                    "(e.g. from a separate thermometer)."
                        calibrationDialog.open()
                    }
                }
            }

            Item { Layout.preferredHeight: Theme.paddingPage }
        }
    }

    Dialog {
        id: calibrationDialog
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 360)
        modal: true
        // `closePolicy` prevents an outside tap from dismissing while
        // the user is typing — they must explicitly hit Cancel or OK.
        closePolicy: Popup.CloseOnEscape

        property string source: "user"
        property string subtitle: ""

        // Pre-fill the input with the current ambient prediction *once*
        // when the dialog opens. Avoids a live binding that would
        // overwrite the user's input on every poll.
        onAboutToShow: {
            tempField.text = backend.temperature.toFixed(1)
            // Defer focus + select until after the dialog actually shows;
            // otherwise the selection happens before the field exists
            // visually and the user sees a blink.
            Qt.callLater(function() {
                tempField.forceActiveFocus()
                tempField.selectAll()
            })
        }

        contentItem: ColumnLayout {
            spacing: 12
            Label {
                text: calibrationDialog.subtitle
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Theme.textSecondary
            }
            TextField {
                id: tempField
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.minTouchSize
                placeholderText: "e.g. 22.5"
                inputMethodHints: Qt.ImhFormattedNumbersOnly
                // No binding to backend.temperature — the value is set
                // once by `onAboutToShow` above. This is what lets the
                // user actually type without their input being clobbered
                // by the next poll.
                onAccepted: calibrationDialog.accept()
            }
        }

        standardButtons: Dialog.Ok | Dialog.Cancel
        onAccepted: {
            var v = parseFloat(tempField.text)
            if (!isNaN(v) && isFinite(v)) {
                backend.calibrate_with_source(v, calibrationDialog.source)
            }
        }
    }
}
