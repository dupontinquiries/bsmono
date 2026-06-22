import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

// MapPage: schematic, offline-only plot of saved locations and
// inbound map pins.
//
// Why not a real geographic map? ambient-learner is a local-only app;
// pulling a tile layer would mean network access. Instead this page
// renders a relative-positioned scatter view: locations are placed
// according to their lat/lon, and pins overlay them. The viewport
// auto-fits whichever items exist, so the picture is always full
// regardless of where the user is.
//
// For a user near a single location with no remote pins, the view
// degenerates to a small cluster around the center — that's fine;
// the *list* on the right is the practical access path. The plot is
// the orientation aid.
Item {
    id: root
    property bool isMobile: true

    // Cached parses of the two JSON feeds we render. We refresh on
    // a slow timer (1Hz) plus on first paint. JSON parses are cheap
    // but we don't need them at 60fps.
    property var locations: []
    property var pins: []
    property string selectedPinId: ""

    // Bounding box computed from all visible items. Falls back to a
    // unit-square around (0, 0) when nothing has coordinates yet.
    property real minLat: -1.0
    property real maxLat:  1.0
    property real minLon: -1.0
    property real maxLon:  1.0
    property bool boundsValid: false

    function refreshAll() {
        var rawLocations = backend.get_locations()
        var rawPins = backend.get_map_pins()
        try {
            root.locations = JSON.parse(rawLocations)
        } catch (e) {
            console.warn("MapPage: bad locations JSON:", e)
            root.locations = []
        }
        try {
            root.pins = JSON.parse(rawPins)
        } catch (e) {
            console.warn("MapPage: bad pins JSON:", e)
            root.pins = []
        }
        recomputeBounds()
    }

    function recomputeBounds() {
        // Walk every coordinate we have and take the min/max. We
        // pad the box by 10% so the dots don't sit right at the
        // edge of the canvas.
        var any = false
        var lo_lat = Number.POSITIVE_INFINITY
        var hi_lat = Number.NEGATIVE_INFINITY
        var lo_lon = Number.POSITIVE_INFINITY
        var hi_lon = Number.NEGATIVE_INFINITY

        for (var i = 0; i < root.locations.length; i++) {
            var loc = root.locations[i]
            if (typeof loc.latitude === "number" && typeof loc.longitude === "number") {
                lo_lat = Math.min(lo_lat, loc.latitude)
                hi_lat = Math.max(hi_lat, loc.latitude)
                lo_lon = Math.min(lo_lon, loc.longitude)
                hi_lon = Math.max(hi_lon, loc.longitude)
                any = true
            }
        }
        for (var j = 0; j < root.pins.length; j++) {
            var p = root.pins[j]
            if (typeof p.latitude === "number" && typeof p.longitude === "number") {
                lo_lat = Math.min(lo_lat, p.latitude)
                hi_lat = Math.max(hi_lat, p.latitude)
                lo_lon = Math.min(lo_lon, p.longitude)
                hi_lon = Math.max(hi_lon, p.longitude)
                any = true
            }
        }

        if (!any) {
            root.boundsValid = false
            return
        }

        // If everything is in one spot (same lat/lon), inflate the
        // box artificially so divide-by-zero doesn't make every
        // point land on top of each other.
        var span_lat = hi_lat - lo_lat
        var span_lon = hi_lon - lo_lon
        if (span_lat < 1e-6) { lo_lat -= 0.01; hi_lat += 0.01 }
        if (span_lon < 1e-6) { lo_lon -= 0.01; hi_lon += 0.01 }

        // 10% padding so dots aren't on the canvas edge.
        var pad_lat = (hi_lat - lo_lat) * 0.10
        var pad_lon = (hi_lon - lo_lon) * 0.10
        root.minLat = lo_lat - pad_lat
        root.maxLat = hi_lat + pad_lat
        root.minLon = lo_lon - pad_lon
        root.maxLon = hi_lon + pad_lon
        root.boundsValid = true
    }

    // Map lat/lon → (x, y) inside the canvas. The y-axis is
    // inverted because screen coords grow downward but latitude
    // grows northward (up).
    function latLonToCanvas(lat, lon, canvasW, canvasH) {
        if (!root.boundsValid) {
            return { x: canvasW / 2, y: canvasH / 2 }
        }
        var nx = (lon - root.minLon) / (root.maxLon - root.minLon)
        var ny = (lat - root.minLat) / (root.maxLat - root.minLat)
        return {
            x: nx * canvasW,
            y: (1.0 - ny) * canvasH,
        }
    }

    // Color a pin by its kind. Matches the well-known kinds the
    // service module documents; unknown kinds get the "other" color
    // so we never end up rendering a pin with no fill.
    function pinColor(kind) {
        switch (kind) {
            case "weather":           return "#56b6c2"  // cyan
            case "photo":
            case "snap":              return "#e5c07b"  // amber
            case "shop":              return "#98c379"  // green
            case "directory":         return "#c678dd"  // purple
            case "sensor":            return "#e06c75"  // red
            default:                  return "#abb2bf"  // grey
        }
    }

    Timer {
        id: refresh
        interval: 1000
        repeat: true
        running: root.visible
        triggeredOnStart: true
        onTriggered: root.refreshAll()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        SectionHeader {
            Layout.fillWidth: true
            title: "MAP"
            accentColor: Theme.sectionLocations
        }

        // Canvas + side panel
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: Theme.paddingPage
            spacing: 12

            // The scatter canvas. We render it with a Canvas because
            // GridLayout-style children would lose the precise
            // pixel placement we want. The Canvas's
            // `requestPaint()` is triggered any time the inputs
            // change.
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                color: Theme.surface
                radius: Theme.radiusCard
                clip: true

                Canvas {
                    id: scatter
                    anchors.fill: parent
                    anchors.margins: 12

                    // Re-paint whenever our inputs change. We call
                    // requestPaint() directly from the root's
                    // onSomethingChanged handlers below, instead of
                    // using property-binding triggers — QML's
                    // `onFooChanged` naming doesn't cleanly handle
                    // underscore-prefixed identifiers and JS-array
                    // identity changes don't always re-fire.
                    Connections {
                        target: root
                        function onLocationsChanged()      { scatter.requestPaint() }
                        function onPinsChanged()           { scatter.requestPaint() }
                        function onSelectedPinIdChanged()  { scatter.requestPaint() }
                        function onBoundsValidChanged()    { scatter.requestPaint() }
                    }

                    onPaint: {
                        var ctx = getContext("2d")
                        ctx.clearRect(0, 0, width, height)

                        if (!root.boundsValid) {
                            // No data — paint a "waiting" message.
                            ctx.fillStyle = Theme.textDim
                            ctx.font = "14px sans-serif"
                            ctx.textAlign = "center"
                            ctx.textBaseline = "middle"
                            ctx.fillText(
                                "No located items yet",
                                width / 2, height / 2 - 8
                            )
                            ctx.fillText(
                                "Add a location or wait for a pin",
                                width / 2, height / 2 + 12
                            )
                            return
                        }

                        // Draw a faint grid for spatial reference.
                        // 4 verticals × 4 horizontals — enough to
                        // give an orientation without dominating.
                        ctx.strokeStyle = Qt.rgba(1, 1, 1, 0.05)
                        ctx.lineWidth = 1
                        for (var g = 1; g < 4; g++) {
                            var xx = (g / 4) * width
                            var yy = (g / 4) * height
                            ctx.beginPath()
                            ctx.moveTo(xx, 0)
                            ctx.lineTo(xx, height)
                            ctx.stroke()
                            ctx.beginPath()
                            ctx.moveTo(0, yy)
                            ctx.lineTo(width, yy)
                            ctx.stroke()
                        }

                        // Plot locations first (drawn beneath pins).
                        for (var i = 0; i < root.locations.length; i++) {
                            var loc = root.locations[i]
                            if (typeof loc.latitude !== "number") continue
                            var pt = root.latLonToCanvas(
                                loc.latitude, loc.longitude, width, height
                            )

                            // Location glyph: filled circle + ring
                            // if it's the current location.
                            ctx.fillStyle = "#61afef"
                            ctx.beginPath()
                            ctx.arc(pt.x, pt.y, 5, 0, 2 * Math.PI)
                            ctx.fill()
                            if (loc.current) {
                                ctx.strokeStyle = "#61afef"
                                ctx.lineWidth = 2
                                ctx.beginPath()
                                ctx.arc(pt.x, pt.y, 9, 0, 2 * Math.PI)
                                ctx.stroke()
                            }

                            // Label: small text to the right.
                            ctx.fillStyle = Theme.textPrimary
                            ctx.font = "11px sans-serif"
                            ctx.textAlign = "left"
                            ctx.textBaseline = "middle"
                            ctx.fillText(loc.name, pt.x + 9, pt.y)
                        }

                        // Plot pins on top.
                        for (var j = 0; j < root.pins.length; j++) {
                            var pin = root.pins[j]
                            if (typeof pin.latitude !== "number") continue
                            var pp = root.latLonToCanvas(
                                pin.latitude, pin.longitude, width, height
                            )
                            var isSel = (pin.id === root.selectedPinId)

                            ctx.fillStyle = root.pinColor(pin.kind)
                            ctx.beginPath()
                            ctx.arc(pp.x, pp.y, isSel ? 7 : 4, 0, 2 * Math.PI)
                            ctx.fill()
                            if (isSel) {
                                ctx.strokeStyle = "#ffffff"
                                ctx.lineWidth = 1.5
                                ctx.beginPath()
                                ctx.arc(pp.x, pp.y, 10, 0, 2 * Math.PI)
                                ctx.stroke()
                            }
                        }
                    }

                    // Tap-to-select pin. We find the nearest pin
                    // within a generous hit radius and select it.
                    // No drag, no pan/zoom — the schematic view is
                    // small enough that single taps are sufficient.
                    MouseArea {
                        anchors.fill: parent
                        onClicked: (mouse) => {
                            var best = null
                            var bestDist = 30 // hit radius in pixels
                            for (var i = 0; i < root.pins.length; i++) {
                                var pin = root.pins[i]
                                if (typeof pin.latitude !== "number") continue
                                var pp = root.latLonToCanvas(
                                    pin.latitude, pin.longitude,
                                    scatter.width, scatter.height
                                )
                                var dx = pp.x - mouse.x
                                var dy = pp.y - mouse.y
                                var d = Math.sqrt(dx * dx + dy * dy)
                                if (d < bestDist) {
                                    bestDist = d
                                    best = pin
                                }
                            }
                            root.selectedPinId = best ? best.id : ""
                        }
                    }
                }
            }

            // Side panel: pin detail + legend. Hidden on narrow
            // displays — the canvas takes the whole width and the
            // detail surfaces as a banner instead.
            ColumnLayout {
                Layout.preferredWidth: 200
                Layout.fillHeight: true
                visible: !root.isMobile
                spacing: 12

                // Selected-pin detail card.
                Card {
                    Layout.fillWidth: true
                    visible: root.selectedPinId !== ""
                    contentItem: ColumnLayout {
                        spacing: 4
                        Label {
                            text: "PIN"
                            color: Theme.textDim
                            font.pixelSize: Theme.fontCaption
                        }
                        // Find the selected pin's title (best-effort
                        // — there's only ever zero or one match).
                        Label {
                            text: {
                                for (var i = 0; i < root.pins.length; i++) {
                                    if (root.pins[i].id === root.selectedPinId) {
                                        return root.pins[i].title
                                            || root.pins[i].kind
                                            || root.selectedPinId
                                    }
                                }
                                return root.selectedPinId
                            }
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontBody
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                        Label {
                            text: {
                                for (var i = 0; i < root.pins.length; i++) {
                                    if (root.pins[i].id === root.selectedPinId) {
                                        return root.pins[i].description || ""
                                    }
                                }
                                return ""
                            }
                            color: Theme.textDim
                            font.pixelSize: Theme.fontCaption
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                            visible: text.length > 0
                        }
                    }
                }

                // Legend: maps colors to kinds. Lists only the
                // kinds we currently render; unknown kinds fall
                // through into the generic "other" row.
                Card {
                    Layout.fillWidth: true
                    contentItem: ColumnLayout {
                        spacing: 6
                        Label {
                            text: "LEGEND"
                            color: Theme.textDim
                            font.pixelSize: Theme.fontCaption
                        }
                        Repeater {
                            model: [
                                { color: "#61afef", label: "Saved location" },
                                { color: "#56b6c2", label: "Weather"        },
                                { color: "#e5c07b", label: "Photo / Snap"   },
                                { color: "#98c379", label: "Shop"           },
                                { color: "#c678dd", label: "Directory"      },
                                { color: "#e06c75", label: "Sensor"         },
                                { color: "#abb2bf", label: "Other"          },
                            ]
                            delegate: RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                Rectangle {
                                    Layout.preferredWidth: 12
                                    Layout.preferredHeight: 12
                                    radius: 6
                                    color: modelData.color
                                }
                                Label {
                                    text: modelData.label
                                    color: Theme.textPrimary
                                    font.pixelSize: Theme.fontCaption
                                    Layout.fillWidth: true
                                }
                            }
                        }
                    }
                }

                // Counts: small at-a-glance summary.
                Card {
                    Layout.fillWidth: true
                    contentItem: ColumnLayout {
                        spacing: 4
                        Label {
                            text: root.locations.length
                                  + " location"
                                  + (root.locations.length === 1 ? "" : "s")
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontBody
                        }
                        Label {
                            text: root.pins.length
                                  + " pin"
                                  + (root.pins.length === 1 ? "" : "s")
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontBody
                        }
                    }
                }
            }
        }

        // Mobile-only bottom banner showing selected pin detail.
        // On wide screens this is the side panel; on phones we
        // surface it below the canvas instead so the canvas keeps
        // the full width.
        Rectangle {
            visible: root.isMobile && root.selectedPinId !== ""
            Layout.fillWidth: true
            Layout.preferredHeight: 64
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            Layout.bottomMargin: Theme.paddingPage
            color: Theme.surface
            radius: Theme.radiusCard

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 12
                spacing: 2
                Label {
                    text: {
                        for (var i = 0; i < root.pins.length; i++) {
                            if (root.pins[i].id === root.selectedPinId) {
                                return root.pins[i].title
                                    || root.pins[i].kind
                                    || root.selectedPinId
                            }
                        }
                        return ""
                    }
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontBody
                    font.bold: true
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                Label {
                    text: {
                        for (var i = 0; i < root.pins.length; i++) {
                            if (root.pins[i].id === root.selectedPinId) {
                                return (root.pins[i].kind || "")
                                    + (root.pins[i].source
                                       ? "  ·  " + root.pins[i].source
                                       : "")
                            }
                        }
                        return ""
                    }
                    color: Theme.textDim
                    font.pixelSize: Theme.fontCaption
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
            }
        }
    }
}
