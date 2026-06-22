// qml/Identicon.qml
//
// Deterministic visual identity for a track based on its title.
//
// Algorithm: FNV-1a 32-bit hash (no BigInt — Qt's JS engine rejects the
// `n` BigInt suffix). 32 bits gives us enough entropy over a ~10k track
// library: expected collisions ≈ (10k²) / (2 × 2³²) ≈ 0.01 — negligible.
//
// From the 32-bit hash we derive:
//   • Foreground HSL hue   ← bits 0-8   (0-255 → scaled to 0-360)
//   • 4×4 symmetric pixel pattern ← bits 8-23 (left half, then mirrored)
//   • Background tone      ← bits 24-31 (used for lightness variation)
//
// We mirror the 4×2 left half horizontally so the pattern reads as a
// "face" or "rune" rather than noise — same trick as GitHub identicons.

import QtQuick 2.15

Canvas {
    id: root
    width: 28
    height: 28
    property string title: ""
    property color borderColor: "#3A3A3A"
    property real cornerRadius: 4

    onTitleChanged: requestPaint()
    onWidthChanged:  requestPaint()
    onHeightChanged: requestPaint()

    /// FNV-1a 32-bit hash. Pure integer arithmetic, no BigInt.
    /// Returns an unsigned 32-bit value (kept positive via >>> 0).
    function _fnv32(s) {
        var h = 0x811c9dc5  // FNV offset basis (32-bit)
        for (var i = 0; i < s.length; ++i) {
            h = h ^ s.charCodeAt(i)
            // FNV prime 0x01000193. We split the multiply to stay within
            // safe integer range: h * prime mod 2^32.
            // Trick: (h * 0x01000193) | 0 works for 32-bit wrap but
            // loses bits when h is large. Use the low-16 / high-16 split:
            var lo = (h & 0xffff) * 0x01000193
            var hi = ((h >>> 16) * 0x01000193) & 0xffff
            h = (((lo >>> 0) + (hi << 16)) >>> 0)
        }
        return h >>> 0
    }

    onPaint: {
        var ctx = getContext("2d")
        ctx.clearRect(0, 0, width, height)

        if (!title || title.length === 0) {
            ctx.fillStyle = "#1A1A1A"
            ctx.fillRect(0, 0, width, height)
            return
        }

        var h = _fnv32(title)

        // Hue: low 9 bits → 0-511, mod 360
        var hue = (h & 0x1ff) % 360
        var fg  = Qt.hsla(hue / 360.0, 0.55, 0.62, 1.0)
        // Background: same hue, much darker + desaturated
        var bg  = Qt.hsla(hue / 360.0, 0.20, 0.14, 1.0)

        ctx.fillStyle = bg
        ctx.fillRect(0, 0, width, height)

        // 4-col × 4-row grid. Only the left 2 columns are independent;
        // the right 2 are their mirror → symmetry makes patterns memorable.
        var cols = 4, rows = 4
        var pad  = 3
        var cellW = (width  - pad * 2) / cols
        var cellH = (height - pad * 2) / rows

        // 8 pattern bits from bits 9-16 of the hash
        var pattern = (h >>> 9) & 0xff
        ctx.fillStyle = fg

        for (var r = 0; r < rows; ++r) {
            for (var c = 0; c < 2; ++c) {
                var bit = (r * 2 + c)
                if (!((pattern >>> bit) & 1)) continue
                // Left half
                ctx.fillRect(pad + c * cellW,             pad + r * cellH, cellW - 1, cellH - 1)
                // Mirror to right half
                ctx.fillRect(pad + (cols - 1 - c) * cellW, pad + r * cellH, cellW - 1, cellH - 1)
            }
        }
    }
}
