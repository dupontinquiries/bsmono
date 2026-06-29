// A deterministic app tile: 1–2 bold letters from the crate name, with hue + warp
// seeded by the commit oid. Same oid → same tile; a new commit re-skins it. This is
// the always-available fallback when an app has no screenshot.
import QtQuick

Canvas {
    id: cv
    property string appName: ""
    property string oid: ""
    property int corner: 12
    onAppNameChanged: requestPaint()
    onOidChanged: requestPaint()
    onWidthChanged: requestPaint()

    function _seed(h) {
        var s = 2166136261 >>> 0
        for (var i = 0; i < h.length; i++) { s = (s ^ h.charCodeAt(i)) >>> 0; s = Math.imul(s, 16777619) >>> 0 }
        return s >>> 0
    }
    function _rng(a) {
        return function () {
            a |= 0; a = (a + 0x6D2B79F5) | 0
            var t = Math.imul(a ^ (a >>> 15), 1 | a)
            t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
            return ((t ^ (t >>> 14)) >>> 0) / 4294967296
        }
    }
    function _mono(n) {
        var p = n.split(/[-_]/).filter(function (s) { return s.length > 0 })
        if (p.length >= 2) return (p[0][0] + p[1][0]).toUpperCase()
        return n.length ? n[0].toUpperCase() : "?"
    }
    // hsl → "rgb(r,g,b)" so Canvas colour parsing is universal
    function _rgb(h, s, l) {
        h = ((h % 360) + 360) % 360; s /= 100; l /= 100
        var c = (1 - Math.abs(2 * l - 1)) * s, x = c * (1 - Math.abs((h / 60) % 2 - 1)), m = l - c / 2
        var r = 0, g = 0, b = 0
        if (h < 60) { r = c; g = x } else if (h < 120) { r = x; g = c }
        else if (h < 180) { g = c; b = x } else if (h < 240) { g = x; b = c }
        else if (h < 300) { r = x; b = c } else { r = c; b = x }
        return "rgb(" + Math.round((r + m) * 255) + "," + Math.round((g + m) * 255) + "," + Math.round((b + m) * 255) + ")"
    }

    onPaint: {
        var ctx = getContext("2d")
        var sz = width
        ctx.reset()
        ctx.fillStyle = "#0c0f18"
        ctx.fillRect(0, 0, sz, sz)
        var src = (oid && oid.length) ? oid : appName
        var rnd = _rng(_seed(src))
        var hue = (oid && oid.length >= 6) ? (parseInt(oid.substring(0, 6), 16) % 360) : (_seed(appName) % 360)
        var m = _mono(appName)
        ctx.textAlign = "center"
        ctx.textBaseline = "middle"
        var fs = (m.length === 1) ? sz * 0.56 : sz * 0.40
        ctx.font = "bold " + fs + "px sans-serif"
        for (var l = 2; l >= 1; l--) {
            ctx.save()
            ctx.globalAlpha = 0.32
            ctx.translate(sz / 2 + (rnd() - 0.5) * sz * 0.087 * l, sz / 2 + (rnd() - 0.5) * sz * 0.087 * l)
            ctx.rotate((rnd() - 0.5) * 0.2)
            ctx.fillStyle = _rgb(hue + l * 44, 62, 36 + l * 8)
            ctx.fillText(m, 0, 0)
            ctx.restore()
        }
        ctx.save()
        ctx.globalAlpha = 1
        ctx.translate(sz / 2, sz / 2)
        ctx.rotate((rnd() - 0.5) * 0.05)
        ctx.fillStyle = _rgb(hue, 66, 70)
        ctx.fillText(m, 0, 0)
        ctx.restore()
    }
}
