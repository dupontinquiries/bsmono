.pragma library
// Shared visual tokens and palette logic.
//
// Values fall into three layers:
//   1. Static design tokens (paddings, font sizes, neutral colors)
//   2. State-driven helpers (tempColor, healthColor)
//   3. Mood helpers — palette shifts based on season + day phase +
//      ambient light + pressure trend
//
// Mood helpers are deliberately *additive*: they tweak a base palette
// rather than replace it. That way the UI stays readable in every
// configuration even if the inputs are unusual.

// ─── Static tokens ─────────────────────────────────────────────────────
var background      = "#0f0f1e";
var surface         = "#1a1a2e";
var surfaceAlt      = "#16213e";
var border          = "#2a2a3e";
var surfaceSelected = "#252540";

var textPrimary   = "#ffffff";
var textSecondary = "#aaaaaa";
var textMuted     = "#666666";
var textDim       = "#888888";

var accent          = "#0099ff";
var accentSecondary = "#aa00ff";
var success         = "#7ed321";
var warning         = "#ff9800";
var danger          = "#e24a4a";

var sectionLive      = "#0099ff";
var sectionModel     = "#aa00ff";
var sectionHistory   = "#ff9800";
var sectionLocations = "#4caf50";
var sectionSync      = "#f44336";

// ─── Spacing / sizing ──────────────────────────────────────────────────
var paddingPage     = 16;
var paddingCard     = 16;
var spacingDefault  = 16;
var radiusCard      = 12;
var radiusLargeCard = 20;
var minTouchSize    = 48;

// ─── Typography ────────────────────────────────────────────────────────
var fontDisplay = 64;
var fontTitle   = 18;
var fontHeading = 16;
var fontBody    = 13;
var fontCaption = 12;
var fontSmall   = 10;

// ─── Animation tokens ──────────────────────────────────────────────────
var durSnappy    = 180;   // ui state changes (button press, tab swap)
var durStandard  = 380;   // most things — temperature recolor, gradient shift
var durSlow      = 1200;  // ambient effects — background hue drift
var durBreathing = 4000;  // very slow loops — confidence pulse
var durStorm     = 8000;  // pressure-driven storm cloud drift

// Easing presets
function easeStandard()    { return [0.4, 0.0, 0.2, 1.0]; }   // material standard
function easeAccelerate()  { return [0.4, 0.0, 1.0, 1.0]; }
function easeDecelerate()  { return [0.0, 0.0, 0.2, 1.0]; }
function easeOvershoot()   { return [0.34, 1.56, 0.64, 1.0]; } // back/out

// ─── State-driven palette ──────────────────────────────────────────────
function tempColor(t) {
    if (t < 10) return "#64b5f6";
    if (t < 18) return "#81c784";
    if (t < 24) return "#fff176";
    if (t < 30) return "#ffb74d";
    return "#ef5350";
}

function healthColor(h) {
    switch (h) {
        case "ok":          return success;
        case "degraded":    return warning;
        case "unavailable": return danger;
        default:            return textDim;
    }
}

function healthLabel(h) {
    switch (h) {
        case "ok":          return "● Sensors locked";
        case "degraded":    return "● Sensors degraded";
        case "unavailable": return "⚠ Sensors unavailable";
        default:            return "○ Sensors initializing";
    }
}

// ─── Seasonal / time-of-day mood ───────────────────────────────────────

// Each phase contributes a 2-stop gradient. We mix it with the season
// tint to get the final background.
function _phaseGradient(phase) {
    switch (phase) {
        case "night":     return ["#070718", "#0f0f1e"];
        case "dawn":      return ["#1a1230", "#3a1f3d"];
        case "morning":   return ["#0f2235", "#163050"];
        case "afternoon": return ["#0f2945", "#1a3a5e"];
        case "dusk":      return ["#2a1538", "#4a1f3a"];
        case "evening":   return ["#150f30", "#1f1545"];
        default:          return [background, surface];
    }
}

// Each season nudges the accent and a "tint" overlay color.
function _seasonAccent(season) {
    switch (season) {
        case "winter": return "#a8d8ff";  // cool blue
        case "spring": return "#a8e6c1";  // pale green
        case "summer": return "#ffd58a";  // warm yellow
        case "autumn": return "#f0a878";  // burnt orange
        default:       return accent;
    }
}

function _seasonTint(season) {
    switch (season) {
        case "winter": return Qt.rgba(0.4, 0.7, 1.0, 0.06);
        case "spring": return Qt.rgba(0.5, 0.9, 0.6, 0.06);
        case "summer": return Qt.rgba(1.0, 0.85, 0.4, 0.07);
        case "autumn": return Qt.rgba(1.0, 0.6, 0.3, 0.07);
        default:       return Qt.rgba(0.0, 0.0, 0.0, 0.0);
    }
}

function moodGradientStops(season, phase) {
    return _phaseGradient(phase);
}

function moodAccent(season) {
    return _seasonAccent(season);
}

function moodTint(season) {
    return _seasonTint(season);
}

// ─── Pressure-trend cues ───────────────────────────────────────────────
// A rising barometer suggests clear weather (cool blue);
// a falling barometer suggests storms (purple-grey).
function pressureMoodColor(trendHpa) {
    if (trendHpa > 1.0)   return "#5fb3ff";   // rising → clear
    if (trendHpa > 0.2)   return "#7ec0ff";
    if (trendHpa > -0.2)  return "#888ea8";   // steady → neutral
    if (trendHpa > -1.0)  return "#7d6fad";   // slight drop
    return "#5a4a7a";                          // strong drop → stormy
}

function pressureMoodLabel(trendHpa) {
    if (trendHpa >  2.0)  return "rapidly rising";
    if (trendHpa >  0.5)  return "rising";
    if (trendHpa > -0.5)  return "steady";
    if (trendHpa > -2.0)  return "falling";
    return "rapidly falling";
}

// ─── Light-level cues ──────────────────────────────────────────────────
// Used to slightly brighten or darken the surface tones; subtle.
function surfaceForLight(baseColor, lightNorm) {
    // 0..1 light → -0.05..+0.05 lightness adjustment
    var delta = (lightNorm - 0.5) * 0.10;
    return Qt.lighter(baseColor, 1.0 + delta);
}
