// qml/Bo.qml
//
// pragma Singleton — registered in qmldir as `singleton Bo 1.0 Bo.qml`.
//
// The single source of truth for the B&O design language: motion curves,
// durations, materials, and the type scale. Centralizing these is what
// makes the whole app feel coherent — every animation shares the same
// physical character, every surface the same materiality.
//
// Design philosophy notes baked into the values below:
//
//   • Motion has WEIGHT. Nothing snaps. B&O products move like
//     well-damped mechanisms — a little overshoot on arrival, a gentle
//     settle. We model this with custom bezier curves, not linear or
//     the default easing.
//
//   • One accent, used sparingly. Color is an event, not a default.
//     Most of the interface is graphite and bone-white; the metallic
//     accent appears only where the user's attention belongs.
//
//   • Type is quiet and precise. Light weights at large sizes, wide
//     letter-spacing on the small uppercase labels — the typographic
//     signature of B&O print and on-product etching.
//
//   • Negative space is structural, not leftover. Generous, consistent.

pragma Singleton
import QtQuick 2.15

QtObject {
    id: bo

    // The window sets `mode` (manually or from time-of-day). Color tokens
    // below resolve from the active mode so the whole app shifts at once.
    property string mode: "day"

    // Late-night calm: animations ease out a touch longer at night so the
    // interface feels slower and quieter after dark.
    //
    // Declared BEFORE the duration tokens that depend on it — Qt's
    // property binding evaluator walks declarations in source order
    // when a singleton is first instantiated, and a forward reference
    // produces "Unable to assign [undefined] to int" warnings against
    // every Behavior/Animation in the app that reads Bo.quick / .base
    // / etc on its first frame. Putting motionScale first eliminates
    // the race entirely.
    readonly property real motionScale: mode === "night" ? 1.35
                                       : mode === "dim"   ? 1.15 : 1.0

    // ── Durations (ms) ──────────────────────────────────────────────
    // Scaled by motionScale (declared above).
    readonly property int instant:  Math.round(90  * motionScale)
    readonly property int quick:    Math.round(180 * motionScale)
    readonly property int base:     Math.round(280 * motionScale)
    readonly property int slow:     Math.round(440 * motionScale)
    readonly property int grand:    Math.round(640 * motionScale)   // page transitions

    // ── Easing curves ───────────────────────────────────────────────
    // A well-damped mechanism: fast departure, soft arrival, the faintest
    // overshoot. Used for almost everything that moves.
    readonly property var settle: [ 0.16, 1.0, 0.30, 1.0, 1.0, 1.0 ]
    // Symmetric, calm — for things that shouldn't draw attention.
    readonly property var smooth: [ 0.45, 0.0, 0.20, 1.0, 1.0, 1.0 ]
    // A confident push with a touch of anticipation — for engaging actions.
    readonly property var spring: [ 0.34, 1.56, 0.34, 1.0, 1.0, 1.0 ]

    function _pick(day, dim, night) {
        return mode === "night" ? night : mode === "dim" ? dim : day
    }

    // ── Theme mode ──────────────────────────────────────────────────
    // "day"   — full contrast, cool neutral (default)
    // "dim"   — evening: lower luminance, slightly warmer
    // "night" — deep, warm, very low luminance, gentle on dark rooms
    // (Mode + motionScale + _pick declared above so duration tokens
    // can reference them; this comment block is the historic anchor
    // for the mode docs.)

    // ── Materials (resolve per mode; accent comes from the product) ─
    readonly property color ink:        _pick("#0a0a0b", "#08080a", "#050507")
    readonly property color graphite:   _pick("#12110f", "#0e0d0c", "#0a0908")
    readonly property color surface:    _pick("#1c1a17", "#181613", "#131110")
    readonly property color surfaceHi:  _pick("#26231e", "#211e1a", "#1b1815")
    readonly property color hairline:   _pick("#34302a", "#2c2924", "#241f1b")
    readonly property color hairlineHi: _pick("#46413a", "#3c372f", "#2f2a24")

    // Warm mid-tone "bronze/taupe" family — the merge of ink and accent.
    // This is the third color you wanted: neither black nor gold, it
    // bridges them so surfaces, idle strokes, and the watermark sit in a
    // warm middle register instead of stark contrast.
    readonly property color bronze:     _pick("#8a7350", "#7d6747", "#6b573b")  // mid warm metal
    readonly property color bronzeDim:  _pick("#5a4d39", "#4f4331", "#403628")  // dim divider/stroke
    readonly property color bronzeWarm: _pick("#a98b5e", "#9a7d53", "#856b46")  // lifted highlight

    // Text warms and dims after dark to reduce glare in a dark room.
    readonly property color bone:       _pick("#f2ece0", "#e9e0d0", "#d8cbb6")
    readonly property color silver:     _pick("#b0a791", "#a39a84", "#8d836e")
    readonly property color slate:      _pick("#6e6451", "#615749", "#514838")

    // ── Palette ─────────────────────────────────────────────────────
    //
    // The named accent family. Every page that wants to color-code
    // something (channel A vs channel B, success vs failure, a
    // category badge) reaches for these instead of hex literals so
    // a single edit here propagates everywhere, and every color
    // shifts with the day/dim/night mode the same way.
    //
    // The colors are tuned so that each mode-step pulls them
    // marginally warmer and dimmer, like the rest of the Bo
    // materials — keeps the palette inside the same visual world
    // instead of feeling spotlighted.
    readonly property color accent:      _pick("#C8A04B", "#B89342", "#A6843A")  // metallic gold
    readonly property color mauve:       _pick("#A88090", "#977280", "#856372")
    readonly property color teal:        _pick("#5C8A8A", "#527C7C", "#476B6B")
    readonly property color sage:        _pick("#8FA88F", "#7F977F", "#6F8470")
    readonly property color rust:        _pick("#B58853", "#A37A4A", "#8E6A40")
    readonly property color terracotta:  _pick("#A6735C", "#946752", "#7F5A47")
    readonly property color steel:       _pick("#6A7A92", "#5F6E83", "#525F72")

    // ── Semantic colors ────────────────────────────────────────────
    //
    // Aliases for status-bearing UI. Pages should prefer these names
    // (Bo.success, Bo.danger) over the underlying palette colors
    // (Bo.sage, Bo.terracotta) when the intent is "status feedback"
    // rather than "categorization" — makes the source read like the
    // designer's intent.
    readonly property color success:     sage         // ✓ done, ready
    readonly property color warning:     rust         // ⚠ careful, attention
    readonly property color danger:      terracotta   // ✕ refused, destructive
    readonly property color info:        teal         // ℹ neutral notice

    // ── Type scale ──────────────────────────────────────────────────
    readonly property int displaySize: 68    // the volume readout
    readonly property int titleSize:   19
    readonly property int bodySize:    14
    readonly property int captionSize: 12
    readonly property int microSize:   10    // wide-tracked uppercase labels

    readonly property real labelTracking: 3.0   // letter-spacing for micro labels
    readonly property real titleTracking: 0.3

    // ── Spacing ─────────────────────────────────────────────────────
    readonly property int gutter:  24    // screen edge margin
    readonly property int gapXS:   6
    readonly property int gapSM:   12
    readonly property int gapMD:   18
    readonly property int gapLG:   28
    readonly property int gapXL:   40

    // ── Geometry ────────────────────────────────────────────────────
    readonly property int cardRadius: 18
    readonly property int keyRadius:  28
    readonly property int touchMin:   52
    // Persistent bottom chrome height. Pages with Flickable scroll
    // content reserve this at the bottom of their contentColumn so
    // the last item isn't clipped under the transport bar on small
    // screens. 64 (bar) + 34 (device strip, worst-case) + 12 (gap)
    // + 6 (dots) + 40 (gap) + buffer = 164. Kept conservatively
    // high because extra whitespace is invisible, a clipped button
    // is a bug.
    // 72 (bar) + 34 (device strip) + 12 (gap) + 6 (dots) + 40 (gap) + buffer
    readonly property int bottomChromeHeight: 172

    // ── Scroll feel ─────────────────────────────────────────────────
    // Default Flickable deceleration (1500) feels sticky on touch — a
    // fling stops too soon, so long lists need many swipes. Lower
    // deceleration lets a fling glide further; higher max velocity lets
    // a fast swipe actually move fast. Apply these to page-level
    // Flickables / ListViews for a stronger, more momentum-y scroll.
    readonly property real flickDecel:   850     // was default 1500 (glides ~2x further)
    readonly property real flickMaxVel:  6000    // was default 2500 (faster flings)

    // ── Helpers ─────────────────────────────────────────────────────
    // Mix two colors (t in 0..1). QML has no built-in lerp for colors
    // outside animations, so we provide one for static blends.
    function mix(a, b, t) {
        return Qt.rgba(
            a.r + (b.r - a.r) * t,
            a.g + (b.g - a.g) * t,
            a.b + (b.b - a.b) * t,
            a.a + (b.a - a.a) * t
        )
    }

    // A faint metallic sheen color derived from an accent — used for
    // highlights on the dial and pressed keys.
    function sheen(accent) {
        return Qt.lighter(accent, 1.35)
    }
    function shade(accent) {
        return Qt.darker(accent, 1.4)
    }

    // Adapt a product accent to the current mode: at night, pull it a
    // little warmer and dimmer so it glows rather than glares in the dark.
    function accentFor(accent) {
        if (mode === "night") {
            // warm slightly toward amber + reduce luminance
            var warmed = mix(accent, Qt.rgba(0.85, 0.6, 0.25, 1.0), 0.18)
            return Qt.darker(warmed, 1.18)
        } else if (mode === "dim") {
            return Qt.darker(accent, 1.06)
        }
        return accent
    }
}
