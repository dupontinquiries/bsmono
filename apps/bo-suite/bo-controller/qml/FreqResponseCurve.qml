// qml/FreqResponseCurve.qml
//
// Frequency-response visualization for the realtime stream mixer.
// Renders two overlaid curves — the SPEAKER channel (Mid signal +
// EQ + volume) and the INSTRUMENTAL channel (Side signal + EQ +
// volume) — across the audible range, on a log-spaced frequency
// axis with a centered dB grid.
//
// The point of this visualization is to make the "high-level
// principles" tangible. When the user turns up SPEAKER MID, they
// see the speaker curve lift around 1.5 kHz and nothing else move.
// When they turn up INSTRUMENTAL LOW, they see the instrumental
// curve rise around 200 Hz, separately. The independence of the
// two chains is right there on the screen. If the curves overlap
// in a way that confuses, that's the diagnostic the user needs to
// reach for a different slider.
//
// Implementation: Canvas with a paint handler that consumes a
// pre-computed JSON array of {hz, spk_db, ins_db} from the Rust
// side. The curve is re-rendered whenever qtobj.mixer_curves_json
// changes — i.e., on every slider move — but only by redrawing the
// vector paths; no per-frame computation in QML. The Rust side
// recomputes the points once per param change (~5 KB JSON).

import QtQuick 2.15
import "."

Canvas {
    id: root
    /// Speaker tint (matches the column header)
    property color speakerColor: "#A88090"
    /// Instrumental tint (matches the column header)
    property color instrumentalColor: "#5C8A8A"
    /// dB range to render — symmetric around 0 dB.
    property real dbRange: 18.0
    /// Frequency range (Hz).
    property real freqLo: 20.0
    property real freqHi: 20000.0

    property var curvePoints: {
        try { return JSON.parse(qtobj.mixer_curves_json || "[]") }
        catch (e) { return [] }
    }
    property var spectrumPoints: {
        // Source-material spectrum overlay. Empty when no file has
        // been analyzed; populated when the user clicks "Analyze
        // spectrum" on a WAV input. JSON shape: {input_path, points}.
        try {
            var s = JSON.parse(qtobj.spectrum_json || "null")
            return s && s.points ? s.points : []
        }
        catch (e) { return [] }
    }
    Connections {
        target: qtobj
        function onMixer_changed() {
            try { root.curvePoints = JSON.parse(qtobj.mixer_curves_json || "[]") }
            catch (e) { root.curvePoints = [] }
            root.requestPaint()
        }
        function onSpectrum_changed() {
            try {
                var s = JSON.parse(qtobj.spectrum_json || "null")
                root.spectrumPoints = s && s.points ? s.points : []
            }
            catch (e) { root.spectrumPoints = [] }
            root.requestPaint()
        }
    }
    onCurvePointsChanged: requestPaint()
    onWidthChanged:  requestPaint()
    onHeightChanged: requestPaint()

    // ── Coordinate helpers ─────────────────────────────────────
    // x: log(freq) maps to [marginLeft, width - marginRight]
    // y: dB        maps to [height - marginBottom, marginTop]
    //              (inverted because Qt's y grows downward)
    readonly property real marginLeft:   38
    readonly property real marginRight:  10
    readonly property real marginTop:    10
    readonly property real marginBottom: 18

    function xForHz(hz) {
        var lo = Math.log(freqLo), hi = Math.log(freqHi)
        var t = (Math.log(hz) - lo) / (hi - lo)
        return marginLeft + t * (width - marginLeft - marginRight)
    }
    function yForDb(db) {
        var t = (dbRange - db) / (2 * dbRange)
        return marginTop + t * (height - marginTop - marginBottom)
    }

    onPaint: {
        var ctx = getContext("2d")
        ctx.reset()
        // Panel background — derive from Bo.graphite so the curve
        // visually anchors to the surrounding cards' deepest tone
        // rather than introducing a fourth gray. Solid fill is fine
        // even if Bo.graphite changes in dim/night modes.
        ctx.fillStyle = Bo.graphite
        ctx.fillRect(0, 0, width, height)

        // Grid colors derived from the Bo palette so the page shifts
        // with mode. We use a darker variant of `hairline` for
        // ordinary grid lines, the regular hairline for axis labels,
        // and `silver` for the highlighted 0 dB axis line.
        var gridDim   = Bo.hairline.toString()
        var gridLabel = Bo.silver.toString()
        var zeroAxis  = Bo.bone.toString()

        // ── Frequency grid (vertical lines at decade boundaries) ──
        ctx.strokeStyle = gridDim
        ctx.lineWidth = 1
        var gridFreqs = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]
        ctx.font = "8px sans-serif"
        ctx.fillStyle = gridLabel
        ctx.textAlign = "center"
        for (var i = 0; i < gridFreqs.length; ++i) {
            var f = gridFreqs[i]
            if (f < freqLo || f > freqHi) continue
            var x = xForHz(f)
            ctx.beginPath()
            ctx.moveTo(x, marginTop)
            ctx.lineTo(x, height - marginBottom)
            ctx.stroke()
            // Label below the plot
            var label = f >= 1000 ? (f / 1000) + "k" : f.toString()
            ctx.fillText(label, x, height - 4)
        }

        // ── dB grid (horizontal lines at ±6, ±12) ─────────────
        var dbLines = [-12, -6, 0, 6, 12]
        ctx.textAlign = "right"
        for (i = 0; i < dbLines.length; ++i) {
            var db = dbLines[i]
            if (Math.abs(db) > dbRange) continue
            var y = yForDb(db)
            ctx.strokeStyle = db === 0 ? zeroAxis : gridDim
            ctx.lineWidth = db === 0 ? 1.5 : 1
            ctx.beginPath()
            ctx.moveTo(marginLeft, y)
            ctx.lineTo(width - marginRight, y)
            ctx.stroke()
            ctx.fillStyle = gridLabel
            ctx.fillText((db > 0 ? "+" : "") + db, marginLeft - 4, y + 3)
        }

        if (curvePoints.length < 2) return

        // Helper — convert a Bo color to an rgba() string with the
        // given alpha. Avoids the hardcoded "168/255, 128/255, ..."
        // RGB tuples that used to mirror Bo.mauve manually.
        function rgbaOf(c, a) {
            return "rgba(" + Math.round(c.r * 255) + ","
                          + Math.round(c.g * 255) + ","
                          + Math.round(c.b * 255) + ","
                          + a + ")"
        }

        // ── Spectrum overlay (drawn UNDER the EQ curves) ───────
        // Source-material spectrum analysis from the file processor.
        // Rendered in subdued silver so it never competes with the
        // colored EQ curves for attention but is visible enough to
        // inform decisions. The user can see where their music has
        // energy, then choose EQ adjustments that land on real peaks.
        if (spectrumPoints.length >= 2) {
            ctx.beginPath()
            // Spectrum is in dB but its reference is intentionally
            // shifted so typical peaks land near 0 dB on our axis.
            // Clamp to [-dbRange, dbRange] for display.
            for (i = 0; i < spectrumPoints.length; ++i) {
                var sp = spectrumPoints[i]
                var dbClamped = Math.max(-dbRange, Math.min(dbRange, sp.db))
                var xs = xForHz(sp.hz)
                var ys = yForDb(dbClamped)
                if (i === 0) ctx.moveTo(xs, ys)
                else         ctx.lineTo(xs, ys)
            }
            // Fade-to-bottom fill so the spectrum reads as a "carpet"
            // under the EQ curves.
            ctx.lineTo(xForHz(spectrumPoints[spectrumPoints.length - 1].hz),
                       height - marginBottom)
            ctx.lineTo(xForHz(spectrumPoints[0].hz), height - marginBottom)
            ctx.closePath()
            ctx.fillStyle = rgbaOf(Bo.silver, 0.10)
            ctx.fill()

            // The spectrum line itself
            ctx.beginPath()
            for (i = 0; i < spectrumPoints.length; ++i) {
                sp = spectrumPoints[i]
                dbClamped = Math.max(-dbRange, Math.min(dbRange, sp.db))
                xs = xForHz(sp.hz)
                ys = yForDb(dbClamped)
                if (i === 0) ctx.moveTo(xs, ys)
                else         ctx.lineTo(xs, ys)
            }
            ctx.strokeStyle = rgbaOf(Bo.silver, 0.70)
            ctx.lineWidth = 1
            ctx.stroke()
        }

        // ── Speaker curve ──────────────────────────────────────
        // Subtle fill underneath the curve gives the eye a sense of
        // "where this channel is active" without obscuring the other.
        ctx.beginPath()
        var startX = xForHz(curvePoints[0].hz)
        var zeroY  = yForDb(0)
        ctx.moveTo(startX, zeroY)
        for (i = 0; i < curvePoints.length; ++i) {
            var p = curvePoints[i]
            ctx.lineTo(xForHz(p.hz), yForDb(p.spk_db))
        }
        ctx.lineTo(xForHz(curvePoints[curvePoints.length - 1].hz), zeroY)
        ctx.closePath()
        ctx.fillStyle = rgbaOf(speakerColor, 0.14)
        ctx.fill()

        // Speaker line
        ctx.beginPath()
        for (i = 0; i < curvePoints.length; ++i) {
            p = curvePoints[i]
            var x2 = xForHz(p.hz), y2 = yForDb(p.spk_db)
            if (i === 0) ctx.moveTo(x2, y2)
            else         ctx.lineTo(x2, y2)
        }
        ctx.strokeStyle = speakerColor
        ctx.lineWidth = 2
        ctx.stroke()

        // ── Instrumental curve (drawn on top) ──────────────────
        ctx.beginPath()
        ctx.moveTo(startX, zeroY)
        for (i = 0; i < curvePoints.length; ++i) {
            p = curvePoints[i]
            ctx.lineTo(xForHz(p.hz), yForDb(p.ins_db))
        }
        ctx.lineTo(xForHz(curvePoints[curvePoints.length - 1].hz), zeroY)
        ctx.closePath()
        ctx.fillStyle = rgbaOf(instrumentalColor, 0.14)
        ctx.fill()

        ctx.beginPath()
        for (i = 0; i < curvePoints.length; ++i) {
            p = curvePoints[i]
            x2 = xForHz(p.hz); y2 = yForDb(p.ins_db)
            if (i === 0) ctx.moveTo(x2, y2)
            else         ctx.lineTo(x2, y2)
        }
        ctx.strokeStyle = instrumentalColor
        ctx.lineWidth = 2
        ctx.stroke()

        // ── Band-center markers (small ticks at the locked freqs) ──
        // Make explicit where each band is centered. The user can
        // see how the curve responds when they move that band's
        // slider, which makes the "groups" tangible.
        try {
            var centers = JSON.parse(qtobj.mixer_band_centers_json || "{}")
            var markBand = function(hz, color) {
                if (!hz) return
                var x = xForHz(hz)
                ctx.strokeStyle = color
                ctx.lineWidth = 1
                ctx.beginPath()
                ctx.moveTo(x, marginTop)
                ctx.lineTo(x, marginTop + 6)
                ctx.stroke()
            }
            markBand(centers.spk_low,  speakerColor)
            markBand(centers.spk_mid,  speakerColor)
            markBand(centers.spk_high, speakerColor)
            markBand(centers.ins_low,  instrumentalColor)
            markBand(centers.ins_mid,  instrumentalColor)
            markBand(centers.ins_high, instrumentalColor)
        } catch (e) {}

        // ── Legend ─────────────────────────────────────────────
        ctx.font = "9px sans-serif"
        ctx.textAlign = "left"
        ctx.fillStyle = speakerColor
        ctx.fillText("— SPEAKER", marginLeft + 6, marginTop + 12)
        ctx.fillStyle = instrumentalColor
        ctx.fillText("— INSTRUMENTAL", marginLeft + 80, marginTop + 12)
        if (spectrumPoints.length >= 2) {
            ctx.fillStyle = rgbaOf(Bo.silver, 0.85)
            ctx.fillText("— SOURCE SPECTRUM", marginLeft + 175, marginTop + 12)
        }
    }
}
