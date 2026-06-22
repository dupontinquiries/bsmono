// qml/DeviceSelector.qml
//
// The top-area product/device selector. Shows the current device with its
// silhouette and name; tapping it expands a soft dropdown of all known
// devices with a chevron that rotates. Selecting one switches the active
// device. Built to feel like a piece of considered hardware: the panel
// unfolds with the settle curve, each row springs slightly on press, and
// the active device carries the accent.
//
// Bindings:
//   qtobj.devices_json     — JSON array [{name, url, type}]
//   qtobj.active_url       — currently selected device url
//   qtobj.product_name     — friendly name of the active device
//   qtobj.product_type     — type of the active device (for silhouette)
//   qtobj.select_device(url)

import QtQuick 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    property var profile: ({})
    // expanded height is managed internally; collapsed is just the header row
    implicitHeight: headerRow.height

    property var deviceList: {
        // Prefer the priority-enriched list when it's been computed —
        // it's the same set of devices, sorted by priority + annotated
        // with group / battery / network state. Falls back to the plain
        // devices_json before any probes have returned.
        try {
            var pj = qtobj.priority_devices_json || ""
            if (pj.length > 2) {
                var parsed = JSON.parse(pj)
                if (parsed.length > 0) return parsed
            }
            return JSON.parse(qtobj.devices_json || "[]")
        } catch (e) { return [] }
    }
    property bool open: false
    property bool hasMultiple: deviceList.length > 1
    // Tappable whenever discovery could surface more devices (always, since
    // mDNS runs in the background) — so you can open it to watch the scan.
    property bool tappable: true

    // ── Current-device header row (always visible) ──────────────────
    Item {
        id: headerRow
        width: parent.width
        height: 54

        Row {
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            spacing: Bo.gapMD

            ProductSilhouette {
                anchors.verticalCenter: parent.verticalCenter
                width: 48; height: 48
                shape: root.profile.shape || "disc"
                productType: qtobj.product_type || ""
                accent: root.accent
            }

            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 3
                Row {
                    spacing: 8
                    Text {
                        text: qtobj.product_name && qtobj.product_name.length > 0
                              ? qtobj.product_name : (root.profile.name || "Connecting…")
                        color: Bo.bone
                        font.pixelSize: Bo.titleSize
                        font.weight: Font.DemiBold
                        font.letterSpacing: Bo.titleTracking
                    }
                    // Anticipation indicator — quietly breathes when the
                    // system has a strong upcoming-action prediction queued.
                    AnticipationDot {
                        anchors.verticalCenter: parent.verticalCenter
                        accent: root.accent
                        strength: {
                            try {
                                var preds = JSON.parse(qtobj.prediction_json || "[]")
                                return preds.length > 0 ? (preds[0].confidence || 0) : 0
                            } catch (e) { return 0 }
                        }
                        topAction: {
                            try {
                                var preds = JSON.parse(qtobj.prediction_json || "[]")
                                return preds.length > 0 ? (preds[0].action || "") : ""
                            } catch (e) { return "" }
                        }
                    }
                    // Chevron — only when there's more than one device
                    Text {
                        visible: true
                        anchors.verticalCenter: parent.verticalCenter
                        text: "⌄"
                        color: Bo.silver
                        font.pixelSize: 16
                        rotation: root.open ? 180 : 0
                        Behavior on rotation { NumberAnimation { duration: Bo.base
                            easing.type: Easing.Bezier; easing.bezierCurve: Bo.settle } }
                    }
                }
                Row {
                    spacing: 8
                    Rectangle {
                        width: 6; height: 6; radius: 3
                        anchors.verticalCenter: parent.verticalCenter
                        color: qtobj.active_url && qtobj.active_url.length > 0 ? root.accent : Bo.slate
                        SequentialAnimation on opacity {
                            running: qtobj.active_url && qtobj.active_url.length > 0
                            loops: Animation.Infinite
                            NumberAnimation { to: 0.35; duration: 1500; easing.type: Easing.InOutSine }
                            NumberAnimation { to: 1.0;  duration: 1500; easing.type: Easing.InOutSine }
                        }
                    }
                    Text {
                        text: root.profile.tagline || qtobj.product_type || "—"
                        color: Bo.silver
                        font.pixelSize: Bo.captionSize
                        font.letterSpacing: Bo.labelTracking - 1
                    }
                }
            }
        }

        MouseArea {
            anchors.fill: parent
            enabled: root.tappable
            onClicked: root.open = !root.open
        }
    }

    // ── Dropdown panel of devices ───────────────────────────────────
    // Rendered as an overlay below the header so it doesn't push layout.
    Rectangle {
        id: panel
        anchors.top: headerRow.bottom
        anchors.topMargin: Bo.gapSM
        anchors.left: parent.left
        width: Math.max(240, parent.width * 0.8)
        radius: Bo.cardRadius
        color: Bo.surface
        border.color: Bo.hairline
        border.width: 1
        visible: opacity > 0.01
        opacity: root.open ? 1 : 0
        z: 50

        // Unfold animation
        property real targetH: deviceColumn.implicitHeight + Bo.gapSM * 2
        height: root.open ? targetH : 0
        clip: true
        Behavior on height { NumberAnimation { duration: Bo.base
            easing.type: Easing.Bezier; easing.bezierCurve: Bo.settle } }
        Behavior on opacity { NumberAnimation { duration: Bo.quick } }

        Column {
            id: deviceColumn
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Bo.gapSM
            spacing: 2

            // Recommended-hub banner: a single-tap path to the best device
            // when we know one and the user isn't already on it. Folds away
            // when there's no recommendation or the active device is already
            // the recommendation — the system stays out of the way.
            Item {
                width: parent.width
                height: visible ? 38 : 0
                visible: qtobj.recommended_hub_url.length > 0
                         && qtobj.recommended_hub_url !== qtobj.active_url
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 2
                    radius: 10
                    color: useRecMouse.pressed
                            ? Bo.mix(Bo.surfaceHi, root.accent, 0.20)
                            : Bo.mix(Bo.surface,   root.accent, 0.10)
                    border.color: root.accent
                    border.width: 1
                    Behavior on color { ColorAnimation { duration: Bo.quick } }
                    Row {
                        anchors.left: parent.left
                        anchors.leftMargin: Bo.gapSM
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: Bo.gapSM
                        // Small star/dot to mark the recommendation
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 10; height: 10; radius: 5
                            color: root.accent
                        }
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 1
                            Text {
                                text: "USE RECOMMENDED HUB"
                                color: Bo.bone
                                font.pixelSize: 9
                                font.weight: Font.DemiBold
                                font.letterSpacing: 1.2
                            }
                            Text {
                                text: {
                                    // Pull the recommended row out of the
                                    // list to show its name.
                                    for (var i = 0; i < root.deviceList.length; i++) {
                                        if (root.deviceList[i].url === qtobj.recommended_hub_url) {
                                            return root.deviceList[i].name
                                                + (root.deviceList[i].explain
                                                    ? " — " + root.deviceList[i].explain : "")
                                        }
                                    }
                                    return qtobj.recommended_hub_product || "best available"
                                }
                                color: Bo.silver
                                font.pixelSize: 10
                                elide: Text.ElideRight
                                width: deviceColumn.width - 50
                            }
                        }
                    }
                    MouseArea {
                        id: useRecMouse
                        anchors.fill: parent
                        onClicked: {
                            qtobj.apply_recommended_hub()
                            root.open = false
                        }
                    }
                }
            }

            Repeater {
                model: root.deviceList
                delegate: Rectangle {
                    width: parent.width
                    height: 62
                    radius: 10
                    property bool isActive: (modelData.url || "") === qtobj.active_url
                    property bool isRecommended: (modelData.url || "") === qtobj.recommended_hub_url
                    color: isActive ? Bo.mix(Bo.surface, root.accent, 0.14)
                                    : (devMouse.pressed ? Bo.surfaceHi : "transparent")
                    Behavior on color { ColorAnimation { duration: Bo.quick } }
                    scale: devMouse.pressed ? 0.98 : 1.0
                    Behavior on scale { NumberAnimation { duration: Bo.instant } }

                    Row {
                        anchors.left: parent.left
                        anchors.leftMargin: Bo.gapSM
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: Bo.gapSM

                        // Active pip + recommended-hub star
                        Item {
                            width: 12; height: 12
                            anchors.verticalCenter: parent.verticalCenter
                            Rectangle {
                                anchors.centerIn: parent
                                width: 7; height: 7; radius: 3.5
                                color: parent.parent.parent.isActive ? root.accent : Bo.bronzeDim
                            }
                            // Subtle outer ring on the recommended hub.
                            Rectangle {
                                anchors.centerIn: parent
                                width: 12; height: 12; radius: 6
                                color: "transparent"
                                border.color: root.accent
                                border.width: 1
                                visible: parent.parent.parent.isRecommended
                                opacity: parent.parent.parent.isActive ? 0.0 : 0.7
                            }
                        }
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            // First line: name + priority badge
                            Row {
                                spacing: 6
                                Text {
                                    text: modelData.name || modelData.url || "Device"
                                    color: parent.parent.parent.parent.isActive ? Bo.bone : Bo.silver
                                    font.pixelSize: Bo.bodySize
                                    font.weight: parent.parent.parent.parent.isActive ? Font.DemiBold : Font.Normal
                                }
                                // Priority group badge (PRIMARY HUB / SECONDARY / …)
                                Rectangle {
                                    visible: (modelData.group_label || "").length > 0
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: badgeText.implicitWidth + 12
                                    height: 14
                                    radius: 7
                                    color: modelData.group_color || Bo.surfaceHi
                                    opacity: 0.85
                                    Text {
                                        id: badgeText
                                        anchors.centerIn: parent
                                        text: modelData.group_label || ""
                                        color: Bo.graphite
                                        font.pixelSize: 8
                                        font.weight: Font.DemiBold
                                        font.letterSpacing: 0.6
                                    }
                                }
                                // "found" tag for discovered devices
                                Text {
                                    visible: modelData.discovered === true
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: "• found"
                                    color: root.accent
                                    font.pixelSize: 9
                                    font.letterSpacing: 1
                                }
                            }
                            // Second line: state explanation (battery / network / BT)
                            Text {
                                text: modelData.explain
                                    || (modelData.url || "")
                                color: Bo.slate
                                font.pixelSize: 10
                                visible: (modelData.explain || "").length > 0
                            }
                            // Fallback URL row when no priority info yet
                            Text {
                                text: modelData.url || ""
                                color: Bo.slate
                                font.pixelSize: 10
                                font.family: "monospace"
                                visible: (modelData.explain || "").length === 0
                            }
                        }
                    }

                    // Trailing battery glyph when known
                    Row {
                        anchors.right: parent.right
                        anchors.rightMargin: Bo.gapSM
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 4
                        // PING — small button to play an identification
                        // sound through this device. Sits above the
                        // delegate's MouseArea so it gets the tap.
                        Rectangle {
                            id: pingBtn
                            anchors.verticalCenter: parent.verticalCenter
                            width: 26; height: 18
                            radius: 9
                            color: pingMouse.pressed
                                ? Bo.mix(Bo.surfaceHi, root.accent, 0.40)
                                : (pingMouse.containsMouse
                                    ? Bo.mix(Bo.surface, root.accent, 0.20)
                                    : Bo.mix(Bo.surface, root.accent, 0.08))
                            border.color: root.accent
                            border.width: 1
                            opacity: 0.85
                            Behavior on color { ColorAnimation { duration: Bo.quick } }
                            // Speaker glyph: small trapezoid + arc.
                            Canvas {
                                anchors.fill: parent
                                antialiasing: true
                                onPaint: {
                                    var ctx = getContext("2d")
                                    ctx.reset()
                                    ctx.strokeStyle = root.accent
                                    ctx.lineWidth = 1.4
                                    ctx.lineCap = "round"
                                    var cx = width / 2, cy = height / 2
                                    // Cone
                                    ctx.beginPath()
                                    ctx.moveTo(cx-5, cy-2)
                                    ctx.lineTo(cx-2, cy-2)
                                    ctx.lineTo(cx+2, cy-4)
                                    ctx.lineTo(cx+2, cy+4)
                                    ctx.lineTo(cx-2, cy+2)
                                    ctx.lineTo(cx-5, cy+2)
                                    ctx.closePath()
                                    ctx.stroke()
                                    // Two short waves
                                    ctx.beginPath()
                                    ctx.arc(cx+3, cy, 3, -0.6, 0.6)
                                    ctx.stroke()
                                    ctx.beginPath()
                                    ctx.arc(cx+3, cy, 5.5, -0.6, 0.6)
                                    ctx.stroke()
                                }
                            }
                            MouseArea {
                                id: pingMouse
                                anchors.fill: parent
                                hoverEnabled: true
                                onClicked: {
                                    var pid = modelData.type || ""
                                    qtobj.ping_device(modelData.url || "", pid)
                                    // Don't open/close the picker; user
                                    // probably wants to keep listening.
                                    mouse.accepted = true
                                }
                            }
                        }
                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 4
                            visible: modelData.battery_percent !== null
                                     && modelData.battery_percent !== undefined
                            // Tiny battery rectangle
                        Item {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 20; height: 10
                            Rectangle {
                                anchors.fill: parent
                                anchors.rightMargin: 2
                                color: "transparent"
                                border.color: Bo.silver
                                border.width: 1
                                radius: 1
                                Rectangle {
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    anchors.bottom: parent.bottom
                                    anchors.margins: 1
                                    width: (parent.width - 2) * Math.max(0,
                                        Math.min(100, modelData.battery_percent || 0)) / 100
                                    color: {
                                        var p = modelData.battery_percent || 0
                                        return p < 20 ? "#d4a8a8"
                                             : p < 50 ? "#d4c89a"
                                                      : "#a8d4a8"
                                    }
                                }
                            }
                            Rectangle {
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                width: 2; height: 5
                                color: Bo.silver
                            }
                        }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: (modelData.battery_percent || 0) + "%"
                                color: Bo.silver
                                font.pixelSize: 9
                            }
                        }
                    }

                    MouseArea {
                        id: devMouse
                        anchors.fill: parent
                        onClicked: {
                            // Use the product-aware variant when we know
                            // the type, so per-product safety thresholds
                            // pick up immediately.
                            var pid = modelData.type || ""
                            if (pid.length > 0) {
                                qtobj.select_device_with_product(modelData.url || "", pid)
                            } else {
                                qtobj.select_device(modelData.url || "")
                            }
                            root.open = false
                        }
                    }
                }
            }

            // Scanning hint — a quiet animated row showing discovery is live.
            Item {
                width: parent.width
                height: 28
                Row {
                    anchors.left: parent.left
                    anchors.leftMargin: Bo.gapSM
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Bo.gapSM
                    // pulsing dot
                    Rectangle {
                        width: 6; height: 6; radius: 3
                        anchors.verticalCenter: parent.verticalCenter
                        color: Bo.bronze
                        SequentialAnimation on opacity {
                            running: root.open; loops: Animation.Infinite
                            NumberAnimation { to: 0.2; duration: 900; easing.type: Easing.InOutSine }
                            NumberAnimation { to: 1.0; duration: 900; easing.type: Easing.InOutSine }
                        }
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "Scanning network…"
                        color: Bo.slate
                        font.pixelSize: 10
                        font.letterSpacing: 1
                    }
                }
            }
        }
    }
}
