import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

Item {
    id: root
    property bool isMobile: true

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        SectionHeader {
            Layout.fillWidth: true
            title: "HISTORY"
            accentColor: Theme.sectionHistory
        }

        // ─── Chart ────────────────────────────────────────────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            radius: Theme.radiusCard
            color: Theme.surface

            Canvas {
                id: chartCanvas
                anchors.fill: parent
                anchors.margins: 12

                property var historyData: []
                property real chartMin: 0
                property real chartMax: 0

                Timer {
                    interval: 1000
                    running: chartCanvas.visible
                    repeat: true
                    onTriggered: {
                        try { chartCanvas.historyData = JSON.parse(backend.get_temperature_history()) }
                        catch (e) { chartCanvas.historyData = [] }
                        chartCanvas.requestPaint()
                    }
                }

                onPaint: {
                    var ctx = getContext("2d")
                    ctx.clearRect(0, 0, width, height)

                    var pad = 32
                    var chartW = width - pad * 2
                    var chartH = height - pad * 2

                    // Axes
                    ctx.strokeStyle = "#444"
                    ctx.lineWidth = 1
                    ctx.beginPath()
                    ctx.moveTo(pad, pad)
                    ctx.lineTo(pad, height - pad)
                    ctx.lineTo(width - pad, height - pad)
                    ctx.stroke()

                    if (historyData.length < 2) {
                        ctx.fillStyle = Theme.textDim
                        ctx.font = "14px sans-serif"
                        ctx.textAlign = "center"
                        ctx.textBaseline = "middle"
                        ctx.fillText("Collecting data…", width / 2, height / 2)
                        chartMin = 0; chartMax = 0
                        return
                    }

                    // Range, with a one-degree minimum span for readability
                    var minT = Infinity, maxT = -Infinity
                    var minTime = Infinity, maxTime = -Infinity
                    for (var i = 0; i < historyData.length; i++) {
                        var p = historyData[i]
                        if (p.temp < minT) minT = p.temp
                        if (p.temp > maxT) maxT = p.temp
                        if (p.t < minTime) minTime = p.t
                        if (p.t > maxTime) maxTime = p.t
                    }
                    chartMin = minT; chartMax = maxT
                    var range = maxT - minT
                    if (range < 1) { range = 1; minT -= 0.5; maxT += 0.5 }

                    // Y axis labels
                    ctx.fillStyle = Theme.textDim
                    ctx.font = "10px sans-serif"
                    ctx.textAlign = "right"
                    ctx.textBaseline = "middle"
                    ctx.fillText(maxT.toFixed(1), pad - 4, pad)
                    ctx.fillText(((minT + maxT) / 2).toFixed(1), pad - 4, pad + chartH / 2)
                    ctx.fillText(minT.toFixed(1), pad - 4, height - pad)

                    // X axis labels (oldest..newest, t in seconds-ago)
                    ctx.textAlign = "center"
                    ctx.textBaseline = "top"
                    ctx.fillText(_formatAgo(maxTime), pad, height - pad + 6)
                    ctx.fillText("now", width - pad, height - pad + 6)

                    // Plot line
                    var timeSpan = maxTime - minTime
                    if (timeSpan === 0) timeSpan = 1

                    ctx.strokeStyle = Theme.sectionHistory
                    ctx.lineWidth = 2
                    ctx.beginPath()
                    for (var j = 0; j < historyData.length; j++) {
                        var pt = historyData[j]
                        // t is "seconds ago" (larger = older), flip horizontally
                        var x = pad + (1 - (pt.t - minTime) / timeSpan) * chartW
                        var y = pad + (1 - (pt.temp - minT) / range) * chartH
                        if (j === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y)
                    }
                    ctx.stroke()
                }

                function _formatAgo(secondsAgo) {
                    if (secondsAgo < 60) return secondsAgo + "s ago"
                    if (secondsAgo < 3600) return Math.floor(secondsAgo / 60) + "m ago"
                    return Math.floor(secondsAgo / 3600) + "h ago"
                }
            }
        }

        // ─── Stats strip ──────────────────────────────────────────────────
        Card {
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            Layout.bottomMargin: Theme.paddingPage

            Label {
                text: "📈 Recent statistics"
                font.pixelSize: Theme.fontHeading
                font.bold: true
                color: Theme.textPrimary
            }

            GridLayout {
                Layout.fillWidth: true
                columns: 4
                columnSpacing: 16
                rowSpacing: 4

                Label { text: "Min";  color: Theme.textDim; font.pixelSize: Theme.fontCaption }
                Label { text: "Max";  color: Theme.textDim; font.pixelSize: Theme.fontCaption }
                Label { text: "Curr"; color: Theme.textDim; font.pixelSize: Theme.fontCaption }
                Label { text: "Pts";  color: Theme.textDim; font.pixelSize: Theme.fontCaption }

                Label {
                    text: chartCanvas.historyData.length > 0
                          ? chartCanvas.chartMin.toFixed(1) + "°"
                          : "—"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                Label {
                    text: chartCanvas.historyData.length > 0
                          ? chartCanvas.chartMax.toFixed(1) + "°"
                          : "—"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                Label {
                    text: backend.temperature.toFixed(1) + "°"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
                Label {
                    text: chartCanvas.historyData.length
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                }
            }
        }
    }
}
