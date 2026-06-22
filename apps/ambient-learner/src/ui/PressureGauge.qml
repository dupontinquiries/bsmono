import QtQuick 2.15
import QtQuick.Controls 2.15
import "Theme.js" as Theme

// PressureGauge: a small animated gauge showing absolute pressure
// and trend direction.
//
// Two visual elements compose the reading:
//   1. A semicircular dial showing the absolute hPa value mapped
//      to a typical atmospheric range (980-1040 hPa). Outside that
//      range the needle saturates at the edge — extreme values
//      mean "weather event" anyway, the precision is less
//      important than the direction.
//   2. A small trend arrow next to the dial, sized by the
//      magnitude of the trend. Larger arrow = stronger storm
//      front or clearing.
//
// Both elements ease into their new positions when pressure
// updates — this is the kind of subtle animated polish Qt does
// well and that would feel forced in Iced or GTK. The numerical
// reading is overlaid as text so the gauge stays useful for
// glance reading.
//
// The component takes a small fixed size (160×100); it's designed
// to drop into the LivePage environment card as an inline visual,
// not to dominate the screen.
Item {
    id: root
    implicitWidth: 160
    implicitHeight: 100

    /// The pressure value in hectopascals. Set from the parent.
    property real hpa: 1013.25
    /// The pressure trend in hPa per hour (sign convention:
    /// positive = rising). Set from the parent.
    property real trendHpa: 0.0

    // ─── Dial range ─────────────────────────────────────────────
    //
    // The "typical atmospheric" envelope. Saturating outside this
    // is honest: anything < 980 or > 1040 means "the weather is
    // doing something unusual" and the precise number matters
    // less than the indication.
    readonly property real minHpa: 980.0
    readonly property real maxHpa: 1040.0

    // Map hpa to a [0..1] position on the dial. Clamped to keep
    // the needle on the arc.
    readonly property real normHpa: Math.max(
        0.0,
        Math.min(1.0, (root.hpa - root.minHpa) / (root.maxHpa - root.minHpa))
    )

    // Map the trend to a normalized [-1..1] for the arrow.
    // ±5 hPa/hour is "strong front"; we saturate there.
    readonly property real normTrend: Math.max(
        -1.0,
        Math.min(1.0, root.trendHpa / 5.0)
    )

    // Smoothly animated proxies — Behavior on a property makes
    // every change tween, no manual animation setup needed.
    property real animatedNorm: root.normHpa
    Behavior on animatedNorm {
        NumberAnimation { duration: Theme.durSlow; easing.type: Easing.OutCubic }
    }
    onNormHpaChanged: animatedNorm = root.normHpa

    property real animatedTrend: root.normTrend
    Behavior on animatedTrend {
        NumberAnimation { duration: Theme.durSlow; easing.type: Easing.OutCubic }
    }
    onNormTrendChanged: animatedTrend = root.normTrend

    // The dial itself, painted on a Canvas. The dial is a
    // semicircle (180° arc) with tick marks at 990 / 1000 / 1010 /
    // 1020 / 1030. We paint it once on first draw; the needle on
    // top is a separate Item that rotates via animation.
    Canvas {
        id: dial
        anchors.fill: parent

        // Repaint when the animated value changes — needs to be
        // every frame of the Behavior animation, hence the
        // dependency on root.animatedNorm.
        property real _trigger: root.animatedNorm
        property real _trigger2: root.animatedTrend
        Connections {
            target: root
            function onAnimatedNormChanged() { dial.requestPaint() }
            function onAnimatedTrendChanged() { dial.requestPaint() }
        }

        onPaint: {
            var ctx = getContext("2d")
            ctx.clearRect(0, 0, width, height)

            var cx = width / 2
            var cy = height - 18  // leave room for label below
            var radius = Math.min(width, height) - 30

            // Arc background. Gradient from cool blue (low) to
            // warm orange (high) maps subjectively to "cold-front"
            // vs "high-pressure-clear-day" weather.
            var grad = ctx.createLinearGradient(cx - radius, 0, cx + radius, 0)
            grad.addColorStop(0.00, "#4a90c4")  // low: blue
            grad.addColorStop(0.50, "#7eb59a")  // mid: muted teal-green
            grad.addColorStop(1.00, "#c89a5c")  // high: amber

            ctx.lineWidth = 8
            ctx.strokeStyle = grad
            ctx.lineCap = "round"
            ctx.beginPath()
            ctx.arc(cx, cy, radius, Math.PI, 2 * Math.PI)
            ctx.stroke()

            // Tick marks at 990/1000/1010/1020/1030.
            var ticks = [990, 1000, 1010, 1020, 1030]
            ctx.fillStyle = Theme.textDim
            ctx.font = "9px sans-serif"
            ctx.textAlign = "center"
            for (var i = 0; i < ticks.length; i++) {
                var tickNorm = (ticks[i] - root.minHpa)
                                 / (root.maxHpa - root.minHpa)
                var angle = Math.PI + tickNorm * Math.PI
                var x1 = cx + (radius - 4) * Math.cos(angle)
                var y1 = cy + (radius - 4) * Math.sin(angle)
                var x2 = cx + (radius + 4) * Math.cos(angle)
                var y2 = cy + (radius + 4) * Math.sin(angle)
                ctx.strokeStyle = Theme.textDim
                ctx.lineWidth = 1
                ctx.beginPath()
                ctx.moveTo(x1, y1)
                ctx.lineTo(x2, y2)
                ctx.stroke()

                // Only label the outer two; middle ones would crowd.
                if (ticks[i] === 990 || ticks[i] === 1030) {
                    var tx = cx + (radius + 14) * Math.cos(angle)
                    var ty = cy + (radius + 14) * Math.sin(angle)
                    ctx.fillText(ticks[i].toString(), tx, ty + 3)
                }
            }

            // Needle. Pivots from the center along the animated
            // angle. We draw it as a tapered triangle pointing
            // outward.
            var needleAngle = Math.PI + root.animatedNorm * Math.PI
            var needleLen = radius - 8
            ctx.fillStyle = Theme.textPrimary
            ctx.beginPath()
            ctx.moveTo(
                cx + needleLen * Math.cos(needleAngle),
                cy + needleLen * Math.sin(needleAngle)
            )
            // Two base points perpendicular to the needle axis.
            var perpAngle = needleAngle + Math.PI / 2
            var baseHalfWidth = 3
            ctx.lineTo(
                cx + baseHalfWidth * Math.cos(perpAngle),
                cy + baseHalfWidth * Math.sin(perpAngle)
            )
            ctx.lineTo(
                cx - baseHalfWidth * Math.cos(perpAngle),
                cy - baseHalfWidth * Math.sin(perpAngle)
            )
            ctx.closePath()
            ctx.fill()

            // Pivot dot.
            ctx.fillStyle = Theme.textPrimary
            ctx.beginPath()
            ctx.arc(cx, cy, 4, 0, 2 * Math.PI)
            ctx.fill()
        }
    }

    // Numerical reading + trend arrow as text overlay, centered
    // below the dial. The arrow size + color comes from
    // animatedTrend so it grows when a real weather event is
    // happening.
    Column {
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: 0

        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 4

            Label {
                text: root.hpa.toFixed(1) + " hPa"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontBody
                font.bold: true
            }
            Label {
                text: {
                    if (root.animatedTrend > 0.05) return "▲"
                    if (root.animatedTrend < -0.05) return "▼"
                    return "→"
                }
                // Size scales with trend magnitude. 0.5 magnitude
                // (≈2.5 hPa/hour) is "noticeable trend" → 1.5×.
                font.pixelSize: Theme.fontBody
                              + Math.abs(root.animatedTrend) * 6
                color: {
                    if (root.animatedTrend > 0.05) return "#c89a5c"
                    if (root.animatedTrend < -0.05) return "#4a90c4"
                    return Theme.textDim
                }
                Behavior on font.pixelSize {
                    NumberAnimation { duration: Theme.durStandard }
                }
                Behavior on color {
                    ColorAnimation { duration: Theme.durStandard }
                }
            }
        }
    }
}
