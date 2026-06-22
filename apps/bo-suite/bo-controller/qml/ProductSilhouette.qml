// qml/ProductSilhouette.qml
//
// A small Canvas badge drawing a stylized silhouette of the product,
// keyed off its `shape` profile.
//
// Design intent (revised, per real-user feedback):
//   • B&O industrial drawings are precise, thin-lined, almost
//     architectural. The previous 2 px stroke + wedge-reveal felt
//     heavy and gimmicky. This version uses 1.0–1.2 px strokes,
//     a quiet opacity fade-in instead of an inking-wedge clip, and
//     more disciplined geometry for each shape.
//   • A soft accent halo still blooms on shape changes, but only
//     a hint — it's a marker not a flourish.
//
// Shapes: bar, disc, screen, cans, column, tablet, spine; fallback rect.

import QtQuick 2.15
import "."

Item {
    id: root
    property string shape: "disc"
    property string productType: ""    // if set, tries assets/products/<type>.svg
    property color accent: "#C8A04B"

    width: 50; height: 50
    clip: true                          // keep stray strokes inside the badge

    // Reveal as a simple opacity ramp instead of a clipped wedge.
    // The wedge looked busy and drew the eye to the icon when the
    // icon should be ambient identification, not a focal element.
    property real reveal: 0      // 0..1 opacity
    property real halo: 0        // accent bloom on shape change

    property bool svgLoaded: svg.status === Image.Ready

    onShapeChanged: revealAnim.restart()
    onAccentChanged: { haloAnim.restart(); cv.requestPaint() }
    onRevealChanged: cv.requestPaint()
    onHaloChanged: cv.requestPaint()
    Component.onCompleted: revealAnim.start()

    NumberAnimation {
        id: revealAnim; target: root; property: "reveal"
        from: 0; to: 1
        // Slower + softer than before — the icon arrives, it doesn't
        // perform. settle curve gives a graceful decel.
        duration: Bo.slow
        easing.type: Easing.OutCubic
    }
    SequentialAnimation {
        id: haloAnim
        // A muted hint of the accent, not a bloom. Half the previous
        // intensity, half the duration on the visible side.
        NumberAnimation { target: root; property: "halo"; to: 1; duration: Bo.quick }
        NumberAnimation { target: root; property: "halo"; to: 0
            duration: Bo.grand; easing.type: Easing.OutCubic }
    }

    // Backing tile
    Rectangle {
        anchors.fill: parent
        radius: 13
        color: Bo.surface
        border.color: Bo.hairline
        border.width: 1
    }

    // Real product SVG (optional). Drop files at assets/products/<type>.svg
    Image {
        id: svg
        anchors.centerIn: parent
        width: parent.width - 6
        height: parent.height - 6
        fillMode: Image.PreserveAspectFit
        smooth: true
        sourceSize.width: width * 2
        sourceSize.height: height * 2
        source: root.productType.length > 0
                ? Qt.resolvedUrl("../assets/products/" + root.productType + ".svg")
                : ""
        visible: root.svgLoaded
        opacity: root.svgLoaded ? root.reveal : 0
    }

    Canvas {
        id: cv
        anchors.fill: parent
        antialiasing: true
        visible: !root.svgLoaded
        opacity: root.reveal   // simple fade-in instead of wedge clip
        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            var w = width, h = height, cx = w/2, cy = h/2

            // Accent halo — much softer than before (alpha 0.07 vs 0.12).
            // This is a *hint* of acknowledgement when the shape changes,
            // not a flashbulb.
            if (root.halo > 0) {
                ctx.beginPath(); ctx.arc(cx, cy, 20 * root.halo, 0, 2*Math.PI)
                ctx.fillStyle = root.accent
                ctx.globalAlpha = 0.07 * root.halo
                ctx.fill(); ctx.globalAlpha = 1.0
            }

            ctx.strokeStyle = root.accent
            ctx.fillStyle = root.accent
            // 1.0 px stroke base — the thin-line industrial-drawing
            // language. Some shapes bump up to 1.2 for legibility
            // on small features.
            ctx.lineWidth = 1.0
            ctx.lineCap = "round"
            ctx.lineJoin = "round"

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
                // Soundbar — a single, long, slender horizontal bar with
                // a hint of grille perforations + a tiny indicator dot.
                // This matches the actual shape of a Stage/Theatre/Beosound
                // Level and is recognizable at a glance.
                rr(6, cy-3, w-12, 6, 2); ctx.stroke()
                // Grille perforations: short vertical hatches inside the
                // bar, evenly spaced. Subtle, evoke speaker grilles.
                ctx.lineWidth = 0.6
                var nh = 11
                for (var ih=0; ih<nh; ih++) {
                    var xh = 9 + ih * ((w-18)/(nh-1))
                    ctx.beginPath()
                    ctx.moveTo(xh, cy-1.5)
                    ctx.lineTo(xh, cy+1.5)
                    ctx.stroke()
                }
                // Indicator LED dot near right edge.
                ctx.lineWidth = 1.0
                ctx.beginPath()
                ctx.arc(w-9, cy+5, 0.9, 0, 2*Math.PI)
                ctx.fill()
                break
            case "disc":
                // The previous disc was two concentric circles + a center
                // dot — too busy, read as a target. Redrawn as a single
                // crisp ring with a small recessed center dot — closer to
                // a precision-machined speaker driver cross-section.
                ctx.lineWidth = 1.0
                ctx.beginPath(); ctx.arc(cx, cy, 15, 0, 2*Math.PI); ctx.stroke()
                ctx.lineWidth = 0.7
                ctx.beginPath(); ctx.arc(cx, cy, 9.5, 0, 2*Math.PI); ctx.stroke()
                // Center dot (smaller than before).
                ctx.beginPath(); ctx.arc(cx, cy, 1.6, 0, 2*Math.PI); ctx.fill()
                break
            case "screen":
                ctx.lineWidth = 1.0
                rr(7, 9, w-14, h-22, 2); ctx.stroke()
                // Stand
                ctx.lineWidth = 0.7
                ctx.beginPath(); ctx.moveTo(cx-6, h-8); ctx.lineTo(cx+6, h-8); ctx.stroke()
                ctx.beginPath(); ctx.moveTo(cx, h-13); ctx.lineTo(cx, h-8); ctx.stroke()
                break
            case "cans":
                // Headphones — two earcups + a curving band overhead.
                // Previous version drew a single hemisphere; this one
                // hints at a real headphone silhouette.
                ctx.lineWidth = 1.0
                // band
                ctx.beginPath()
                ctx.arc(cx, cy+2, 14, Math.PI, 2*Math.PI)
                ctx.stroke()
                // earcups
                rr(cx-19, cy, 7, 13, 3); ctx.stroke()
                rr(cx+12, cy, 7, 13, 3); ctx.stroke()
                break
            case "column":
                // Tall column speaker — vertical pill + a single center
                // driver dot near the upper third.
                ctx.lineWidth = 1.0
                rr(cx-7, 8, 14, h-16, 6); ctx.stroke()
                ctx.lineWidth = 0.7
                ctx.beginPath(); ctx.arc(cx, cy-2, 3, 0, 2*Math.PI); ctx.stroke()
                break
            case "tablet":
                ctx.lineWidth = 1.0
                rr(10, 12, w-20, h-24, 4); ctx.stroke()
                ctx.lineWidth = 0.7
                ctx.beginPath(); ctx.moveTo(15, cy); ctx.lineTo(w-15, cy); ctx.stroke()
                break
            case "spine":
                ctx.lineWidth = 1.0
                rr(cx-5, 8, 10, h-16, 2); ctx.stroke()
                ctx.lineWidth = 0.6
                for (var j=0; j<3; j++) {
                    ctx.beginPath()
                    ctx.moveTo(cx-3, 15 + j*8)
                    ctx.lineTo(cx+3, 15 + j*8)
                    ctx.stroke()
                }
                break
            default:
                ctx.lineWidth = 1.0
                rr(11, 11, w-22, h-22, 5); ctx.stroke()
            }
        }
    }
}

