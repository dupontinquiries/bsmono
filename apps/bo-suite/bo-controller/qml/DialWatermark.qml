// qml/DialWatermark.qml
//
// A large product silhouette peeking out from the top-left behind the
// volume dial, as a soft, blurred, half-cut watermark with living strokes.
//
//   • CUT — clipped along a 45° diagonal through its midpoint; only the
//     upper-left triangle is drawn (done inside the Canvas via a triangle
//     clip, so the edge is exact and cheap).
//   • SOFT — ~50% opacity + a 4–16px blur. Blur uses GraphicalEffects when
//     present; if the module is unavailable the watermark still shows
//     (un-blurred) so nothing breaks.
//   • LIVING STROKE — drawn as a dashed outline whose dash offset advances
//     continuously: a slow calm loop at rest, faster + brighter as audio
//     activity rises.
//
// Usage:
//   DialWatermark {
//       shape: window.profile.shape
//       accent: window.accent
//       activity: someValue        // 0..1
//       cutColor: Bo.graphite      // what it dissolves into (window bg)
//   }

import QtQuick 2.15
import "."

Item {
    id: root

    property string shape: "disc"
    property color accent: "#C8A04B"
    property real activity: 0          // 0..1 drives speed + brightness + sharpness
    property color cutColor: Bo.graphite

    // Tuning
    property real maxOpacity: 0.5
    property real minBlur: 4
    property real maxBlur: 16

    // Resting opacity is lower; it lifts toward maxOpacity with activity so
    // the watermark is whisper-quiet at idle and blooms a little when you
    // interact or audio plays.
    opacity: (0.30 + root.activity * 0.20) * (root.maxOpacity / 0.5)

    // Continuously advancing phase for the flowing dashed stroke.
    // Tuned for a meditative pace: ~60s loop when quiet, ~25s when fully
    // active. The dash pattern modulo (18px) means each visible "dash
    // step" takes ~1s at rest and never quite reaches frantic.
    property real dashPhase: 0
    NumberAnimation on dashPhase {
        running: true; loops: Animation.Infinite
        from: 0; to: 1000
        duration: Math.round(60000 - root.activity * 35000)
    }

    // The thing we blur: the traced canvas. We keep it as a child and feed
    // it into a blur effect; the effect (if loaded) hides the source and
    // shows the blurred copy. Root opacity is set above (activity-driven).

    Canvas {
        id: art
        anchors.fill: parent
        antialiasing: true
        renderStrategy: Canvas.Cooperative
        visible: !blurLoader.active   // hidden when the blur effect renders it

        property real phase: root.dashPhase
        onPhaseChanged: requestPaint()
        Component.onCompleted: requestPaint()
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()

        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var w = width, h = height

            // 1. Clip to the upper-left triangle (the 45° cut through the
            //    midpoint of the box: from top-right corner to bottom-left).
            ctx.beginPath()
            ctx.moveTo(0, 0)
            ctx.lineTo(w, 0)
            ctx.lineTo(0, h)
            ctx.closePath()
            ctx.clip()

            // 2. Trace the silhouette, scaled from 64-unit space.
            var S = Math.min(w, h) / 64
            ctx.scale(S, S)
            // Idle stroke is the warm bronze mid-tone; with activity it
            // blends toward the full product accent. Avoids stark gold.
            ctx.strokeStyle = Bo.mix(Bo.bronze, root.accent, root.activity)
            ctx.lineWidth = 2.6
            ctx.lineCap = "round"
            ctx.lineJoin = "round"
            if (ctx.setLineDash) ctx.setLineDash([10, 8])
            ctx.lineDashOffset = -((root.dashPhase) % 18)
            ctx.globalAlpha = 0.45 + root.activity * 0.4

            var rr = function(x, y, ww, hh, r) {
                ctx.beginPath()
                ctx.moveTo(x+r, y)
                ctx.arcTo(x+ww, y, x+ww, y+hh, r)
                ctx.arcTo(x+ww, y+hh, x, y+hh, r)
                ctx.arcTo(x, y+hh, x, y, r)
                ctx.arcTo(x, y, x+ww, y, r)
                ctx.closePath()
            }

            switch (root.shape) {
            case "bar":
                rr(6, 25, 52, 14, 7); ctx.stroke()
                ctx.beginPath(); ctx.arc(32, 32, 4.2, 0, 2*Math.PI); ctx.stroke()
                break
            case "disc":
                ctx.beginPath(); ctx.arc(32, 32, 22, 0, 2*Math.PI); ctx.stroke()
                ctx.beginPath(); ctx.arc(32, 32, 12, 0, 2*Math.PI); ctx.stroke()
                break
            case "screen":
                rr(8, 14, 48, 30, 8); ctx.stroke()
                break
            case "cans":
                ctx.beginPath(); ctx.arc(32, 34, 18, Math.PI, 2*Math.PI); ctx.stroke()
                rr(9, 34, 13, 20, 6.5); ctx.stroke()
                rr(42, 34, 13, 20, 6.5); ctx.stroke()
                break
            case "column":
                rr(20, 8, 24, 48, 12); ctx.stroke()
                ctx.beginPath(); ctx.arc(32, 34, 7, 0, 2*Math.PI); ctx.stroke()
                break
            case "tablet":
                rr(10, 16, 44, 32, 11); ctx.stroke()
                break
            case "spine":
                rr(24, 8, 16, 48, 8); ctx.stroke()
                break
            default:
                ctx.beginPath(); ctx.arc(32, 32, 22, 0, 2*Math.PI); ctx.stroke()
            }
        }
    }

    // Soft blur, sharpness eases with activity (more active = a touch
    // sharper, so it "focuses" when you interact). Loaded defensively:
    // if GraphicalEffects isn't available the source canvas shows raw.
    property real blurRadius: root.maxBlur - root.activity * (root.maxBlur - root.minBlur)

    Loader {
        id: blurLoader
        anchors.fill: parent
        active: true
        sourceComponent: blurComponent
        // If the component fails to create (module missing), Loader.item
        // is null and the raw canvas (art.visible) remains shown.
        onStatusChanged: if (status === Loader.Error) art.visible = true
    }

    Component {
        id: blurComponent
        Item {
            // Inline import attempt; on Qt5 this is QtGraphicalEffects,
            // on Qt6 Qt5Compat.GraphicalEffects. We try Qt5 first since the
            // app targets the Qt5/qmetaobject stack.
            Loader {
                anchors.fill: parent
                source: ""
                Component.onCompleted: {
                    // Build the effect via a string so a missing module
                    // doesn't hard-fail at QML compile time.
                    var qml = 'import QtQuick 2.15; import QtGraphicalEffects 1.15; '
                            + 'GaussianBlur { anchors.fill: parent; source: art; '
                            + 'radius: root.blurRadius; samples: 16; cached: false }'
                    try {
                        var obj = Qt.createQmlObject(qml, this, "blurfx")
                        if (!obj) art.visible = true
                    } catch (e) {
                        // Module not present — fall back to the raw canvas.
                        art.visible = true
                    }
                }
            }
        }
    }
}
