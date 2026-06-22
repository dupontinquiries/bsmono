// qml/VolumeDial.qml
//
// The hero control: a circular volume dial that behaves like a physical
// B&O aluminium wheel.
//
// What makes it feel real:
//   • INERTIA — flick the ring and it keeps turning, decelerating under
//     friction, instead of stopping dead. A real weighted wheel.
//   • DETENTS — a faint pulse + tick as the value crosses each 10% mark,
//     the visual analogue of the tactile clicks B&O wheels have.
//   • SHEEN — a moving metallic highlight on the filled arc catches the
//     "light" as the value changes, suggesting brushed aluminium.
//   • ARRIVAL — when set programmatically (external change), the needle
//     eases in with the `settle` curve's gentle overshoot, never snaps.
//   • BREATHING IDLE — at rest the handle has an almost imperceptible
//     glow pulse, so the control feels alive, not frozen.
//
// Interaction:
//   • Drag around the ring → set by angle.
//   • Drag vertically near the center → fine adjust (up = louder).
//   • Flick the ring → inertial spin that decelerates and settles.
//
// Commits via valueChangeRequested(v) continuously during interaction;
// the parent decides when to actually push to the device (debounced).

import QtQuick 2.15
import "."

Item {
    id: root

    property int diameter: 248
    property real value: 0                 // 0..100, the displayed value
    property bool muted: false
    property color accent: "#C8A04B"
    property color accentSoft: "#33291a"
    property color trackColor: "#1e1e21"
    property color textColor: "#f4f2ed"
    property color subTextColor: "#a3a09a"

    property bool dragging: false
    signal valueChangeRequested(real v)
    signal interactionStarted()
    signal interactionEnded()

    width: diameter
    height: diameter

    // Gauge geometry: a 270° sweep with a gap at the bottom.
    readonly property real startAngle: 135
    readonly property real sweepAngle: 270

    // ── Internal physics state ──────────────────────────────────────
    property real _angularVel: 0           // deg/frame, for inertial flick
    property int  _lastDetent: -1          // last 10% bucket crossed
    property real _displayValue: value     // smoothed value the canvas draws

    // Smoothly track value with the settle curve unless the user is
    // actively dragging (then we follow 1:1 for responsiveness).
    Behavior on _displayValue {
        enabled: !root.dragging
        NumberAnimation {
            duration: Bo.base
            easing.type: Easing.Bezier
            easing.bezierCurve: Bo.settle
        }
    }
    onValueChanged: {
        if (!dragging) _displayValue = value
        else _displayValue = value
    }
    on_DisplayValueChanged: { canvas.requestPaint(); checkDetent() }
    onAccentChanged: canvas.requestPaint()
    onMutedChanged: canvas.requestPaint()

    // Detent: pulse when crossing each 10% mark.
    function checkDetent() {
        var bucket = Math.round(_displayValue / 10)
        if (bucket !== _lastDetent) {
            _lastDetent = bucket
            detentPulse.restart()
        }
    }

    // ── Inertial flick integration ──────────────────────────────────
    Timer {
        id: inertia
        interval: 16; repeat: true; running: false
        onTriggered: {
            if (Math.abs(root._angularVel) < 0.05) {
                stop()
                root.interactionEnded()
                return
            }
            var deltaV = root._angularVel / root.sweepAngle * 100
            var nv = Math.max(0, Math.min(100, root.value + deltaV))
            root.value = nv
            root._displayValue = nv
            root.valueChangeRequested(nv)
            root._angularVel *= 0.92          // friction
            if (nv <= 0 || nv >= 100) { root._angularVel = 0 }  // hit the rail
        }
    }

    // ── Detent pulse (visual tick) ──────────────────────────────────
    property real _detentScale: 0
    SequentialAnimation {
        id: detentPulse
        NumberAnimation { target: root; property: "_detentScale"; to: 1.0; duration: Bo.instant }
        NumberAnimation { target: root; property: "_detentScale"; to: 0.0; duration: Bo.quick
                          easing.type: Easing.OutCubic }
    }

    // ── Idle breathing glow ─────────────────────────────────────────
    property real _idleGlow: 0
    SequentialAnimation {
        running: !root.dragging && !inertia.running
        loops: Animation.Infinite
        NumberAnimation { target: root; property: "_idleGlow"; to: 1.0
                          duration: 2400; easing.type: Easing.InOutSine }
        NumberAnimation { target: root; property: "_idleGlow"; to: 0.0
                          duration: 2400; easing.type: Easing.InOutSine }
    }

    // ── Sheen sweep position (drives the moving metallic highlight) ─
    property real _sheenPos: 0
    NumberAnimation on _sheenPos {
        running: true; loops: Animation.Infinite
        from: 0; to: 1; duration: 3600
    }
    on_SheenPosChanged: if (!muted) canvas.requestPaint()

    // ── The dial itself ─────────────────────────────────────────────
    Canvas {
        id: canvas
        anchors.fill: parent
        antialiasing: true
        renderStrategy: Canvas.Cooperative

        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var cx = width / 2, cy = height / 2
            var r = Math.min(width, height) / 2 - 18
            var lineW = 11
            var d2r = function(d) { return d * Math.PI / 180 }
            var a0 = d2r(root.startAngle)
            var a1 = d2r(root.startAngle + root.sweepAngle)
            var v = root._displayValue
            var aVal = d2r(root.startAngle + root.sweepAngle * (v / 100))

            // 1. Recessed track (with a faint inner shadow feel via two strokes)
            ctx.beginPath(); ctx.arc(cx, cy, r, a0, a1, false)
            ctx.lineWidth = lineW + 2; ctx.lineCap = "round"
            ctx.strokeStyle = Qt.darker(root.trackColor, 1.3); ctx.stroke()
            ctx.beginPath(); ctx.arc(cx, cy, r, a0, a1, false)
            ctx.lineWidth = lineW; ctx.lineCap = "round"
            ctx.strokeStyle = root.trackColor; ctx.stroke()

            // 2. Filled value arc
            if (v > 0) {
                var arcColor = root.muted ? root.subTextColor : root.accent
                ctx.beginPath(); ctx.arc(cx, cy, r, a0, aVal, false)
                ctx.lineWidth = lineW; ctx.lineCap = "round"
                ctx.strokeStyle = arcColor; ctx.stroke()

                // 2b. Metallic sheen — a short bright segment travelling
                //     along the filled arc, like light on brushed aluminium.
                if (!root.muted) {
                    var sweepFrac = v / 100
                    var sheenCenter = root._sheenPos * sweepFrac
                    var sheenWidth = 0.12
                    var sStart = Math.max(0, sheenCenter - sheenWidth)
                    var sEnd = Math.min(sweepFrac, sheenCenter + sheenWidth)
                    if (sEnd > sStart) {
                        var sa0 = d2r(root.startAngle + root.sweepAngle * sStart)
                        var sa1 = d2r(root.startAngle + root.sweepAngle * sEnd)
                        ctx.beginPath(); ctx.arc(cx, cy, r, sa0, sa1, false)
                        ctx.lineWidth = lineW; ctx.lineCap = "round"
                        ctx.strokeStyle = Bo.sheen(root.accent)
                        ctx.globalAlpha = 0.55; ctx.stroke(); ctx.globalAlpha = 1.0
                    }
                }
            }

            // 3. Tick marks every 10%
            for (var i = 0; i <= 10; i++) {
                var ta = d2r(root.startAngle + root.sweepAngle * (i / 10))
                var lit = (i <= Math.round(v / 10))
                var inner = r - lineW - 5
                var outer = r - lineW - (lit ? 13 : 10)
                ctx.beginPath()
                ctx.moveTo(cx + Math.cos(ta) * inner, cy + Math.sin(ta) * inner)
                ctx.lineTo(cx + Math.cos(ta) * outer, cy + Math.sin(ta) * outer)
                ctx.lineWidth = lit ? 2.5 : 1.5
                ctx.strokeStyle = lit && !root.muted
                    ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.6)
                    : root.trackColor
                ctx.stroke()
            }

            // 4. Detent flash ring (brief expanding glow on each 10% cross)
            if (root._detentScale > 0) {
                ctx.beginPath()
                ctx.arc(cx, cy, r, a0, a1, false)
                ctx.lineWidth = lineW + 8 * root._detentScale
                ctx.lineCap = "round"
                ctx.strokeStyle = root.accent
                ctx.globalAlpha = 0.12 * root._detentScale
                ctx.stroke(); ctx.globalAlpha = 1.0
            }

            // 5. Handle knob with idle breathing glow
            var hx = cx + Math.cos(aVal) * r
            var hy = cy + Math.sin(aVal) * r
            var knobColor = root.muted ? root.subTextColor : root.accent
            // glow halo
            var glowR = (root.dragging ? 16 : 11) + root._idleGlow * 2.5
            ctx.beginPath(); ctx.arc(hx, hy, glowR, 0, 2 * Math.PI)
            ctx.fillStyle = knobColor
            ctx.globalAlpha = 0.14 + root._idleGlow * 0.10
            ctx.fill(); ctx.globalAlpha = 1.0
            // solid knob
            ctx.beginPath(); ctx.arc(hx, hy, root.dragging ? 12 : 9.5, 0, 2 * Math.PI)
            ctx.fillStyle = knobColor; ctx.fill()
            ctx.lineWidth = 3; ctx.strokeStyle = Bo.graphite; ctx.stroke()
            // brushed-metal dot in the knob center
            ctx.beginPath(); ctx.arc(hx, hy, 2.5, 0, 2 * Math.PI)
            ctx.fillStyle = Bo.sheen(root.accent); ctx.globalAlpha = root.muted ? 0 : 0.8
            ctx.fill(); ctx.globalAlpha = 1.0
        }
    }

    // ── Center readout ──────────────────────────────────────────────
    Column {
        anchors.centerIn: parent
        spacing: 4

        // The number animates its own scale on detent for a subtle "tick".
        Text {
            id: numberText
            anchors.horizontalCenter: parent.horizontalCenter
            text: Math.round(root._displayValue)
            color: root.muted ? root.subTextColor : root.textColor
            font.pixelSize: Bo.displaySize
            font.weight: Font.Light
            font.letterSpacing: -2
            transform: Scale {
                origin.x: numberText.width / 2
                origin.y: numberText.height / 2
                xScale: 1 + root._detentScale * 0.015
                yScale: 1 + root._detentScale * 0.015
            }
        }

        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 6
            Rectangle {
                visible: root.muted
                width: 6; height: 6; radius: 3
                anchors.verticalCenter: parent.verticalCenter
                color: root.subTextColor
            }
            Text {
                text: root.muted ? "MUTED" : "VOLUME"
                color: root.subTextColor
                font.pixelSize: Bo.captionSize
                font.letterSpacing: Bo.labelTracking + 1
            }
        }
    }

    // ── Interaction ─────────────────────────────────────────────────
    MouseArea {
        id: area
        anchors.fill: parent
        preventStealing: true

        property real lastAngle: 0
        property real lastY: 0
        property real lastT: 0
        property bool circular: false

        onPressed: function(mouse) {
            inertia.stop()
            root.dragging = true
            root._angularVel = 0
            root.interactionStarted()
            lastY = mouse.y
            lastT = Date.now()
            var cx = width / 2, cy = height / 2
            var dx = mouse.x - cx, dy = mouse.y - cy
            var dist = Math.sqrt(dx*dx + dy*dy)
            var r = Math.min(width, height) / 2 - 18
            circular = dist > r * 0.5
            lastAngle = Math.atan2(dy, dx) * 180 / Math.PI
            if (circular) setFromPoint(mouse.x, mouse.y)
        }

        onPositionChanged: function(mouse) {
            if (!root.dragging) return
            var now = Date.now()
            var dt = Math.max(1, now - lastT)
            if (circular) {
                var cx = width / 2, cy = height / 2
                var ang = Math.atan2(mouse.y - cy, mouse.x - cx) * 180 / Math.PI
                var dA = ang - lastAngle
                // normalize across the ±180 seam
                if (dA > 180) dA -= 360
                if (dA < -180) dA += 360
                // track angular velocity for the flick release
                root._angularVel = dA / dt * 16    // deg per 16ms frame
                lastAngle = ang
                setFromPoint(mouse.x, mouse.y)
            } else {
                var dyDelta = lastY - mouse.y
                root._angularVel = (dyDelta / dt * 16) * 0.5
                var nv = Math.max(0, Math.min(100, root.value + dyDelta * 0.5))
                root.value = nv; root._displayValue = nv
                root.valueChangeRequested(nv)
            }
            lastY = mouse.y
            lastT = now
        }

        onReleased: function(mouse) {
            root.dragging = false
            // If there's meaningful angular momentum, let it spin out.
            if (Math.abs(root._angularVel) > 0.6) {
                inertia.start()
            } else {
                root.interactionEnded()
            }
            canvas.requestPaint()
        }
        onCanceled: { root.dragging = false; root.interactionEnded() }

        function setFromPoint(px, py) {
            var cx = width / 2, cy = height / 2
            var ang = Math.atan2(py - cy, px - cx) * 180 / Math.PI
            var rel = ang - root.startAngle
            while (rel < 0) rel += 360
            while (rel >= 360) rel -= 360
            if (rel > root.sweepAngle) {
                rel = (rel - root.sweepAngle) < (360 - rel) ? root.sweepAngle : 0
            }
            var nv = Math.max(0, Math.min(100, (rel / root.sweepAngle) * 100))
            root.value = nv
            root._displayValue = nv
            root.valueChangeRequested(nv)
        }
    }
}
