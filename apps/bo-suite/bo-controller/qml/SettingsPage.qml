// qml/SettingsPage.qml
//
// One central place where the user manages system-level integration:
//
//   • KDE tools audit — which command-line utilities the controller
//     depends on are installed, and a copy-pasteable install command
//     for the missing ones. Each missing tool also shows a build-
//     from-source recipe when applicable.
//
//   • Bluetooth auto-connect — per-device toggles ("let this device
//     connect automatically"). When enabled, the controller
//     periodically runs `bluetoothctl connect <MAC>` to keep the
//     device joined.
//
// Layout: a vertical Flickable holding card sections. Cards match the
// existing ControlCard pattern used elsewhere in the app.

import QtQuick 2.15
import QtQuick.Controls 2.15
// SettingsPage is the only QML file in this project that uses the
// Layouts items (RowLayout / ColumnLayout / Layout.* attached
// properties). Older Qt 5 sometimes re-exported these via
// QtQuick.Controls, but Qt 6 doesn't — so the import must be
// explicit. Symptom of forgetting it: "RowLayout is not a type"
// at engine-load time and the entire SettingsPage component
// fails to construct, which in turn breaks any parent that
// references it (here: volume_knobs.qml's loader at 2092).
import QtQuick.Layouts 1.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"

    Component.onCompleted: qtobj.audit_kde_tools()

    Flickable {
        // Stronger scroll: glide further, fling faster.
        flickDeceleration: Bo.flickDecel
        maximumFlickVelocity: Bo.flickMaxVel
        id: scroll
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        contentWidth: parent.width - Bo.gapMD * 2
        // Add bottom chrome reserve so the last card (Full Rescan)
        // doesn't hide under the persistent BottomTransportBar on
        // smaller screens. Was: contentHeight: contentColumn.height
        contentHeight: contentColumn.height + Bo.bottomChromeHeight
        clip: true

        Column {
            id: contentColumn
            width: parent.width
            spacing: Bo.gapMD

            // Page title + count summary
            Column {
                width: parent.width
                spacing: 4
                Text {
                    text: "SETTINGS"
                    color: root.accent
                    font.pixelSize: 11
                    font.letterSpacing: 1.6
                    font.weight: Font.DemiBold
                }
                Text {
                    text: "System integration & device management"
                    color: Bo.silver
                    font.pixelSize: 12
                }
            }

            // ── KDE TOOLS CARD ────────────────────────────────────
            ControlCard {
                width: parent.width
                label: "SYSTEM TOOLS"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "Command-line utilities the controller uses for "
                            + "audio playback, mic capture, and Bluetooth. "
                            + "Tap a missing tool to see install instructions."
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    // Apt one-shot install — shown only when something's missing.
                    Rectangle {
                        width: parent.width
                        height: aptText.implicitHeight + Bo.gapSM * 2
                        radius: 6
                        color: Bo.mix(Bo.surface, root.accent, 0.12)
                        border.color: root.accent
                        border.width: 1
                        visible: (qtobj.kde_tools_install_cmd || "").length > 0
                        Column {
                            anchors.fill: parent
                            anchors.margins: Bo.gapSM
                            spacing: 4
                            Text {
                                text: "QUICK INSTALL — run this from a terminal:"
                                color: Bo.bronzeDim
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            Text {
                                id: aptText
                                width: parent.width
                                text: qtobj.kde_tools_install_cmd
                                color: Bo.bone
                                font.pixelSize: 10
                                font.family: "monospace"
                                wrapMode: Text.WrapAnywhere
                                textFormat: Text.PlainText
                            }
                        }
                    }

                    // Tool list — one row per known tool
                    Repeater {
                        model: {
                            try { return JSON.parse(qtobj.kde_tools_json || "[]") }
                            catch (e) { return [] }
                        }
                        delegate: Rectangle {
                            width: contentColumn.width - 2
                            height: detailVisible ? rowCol.height + Bo.gapSM * 2 : 38
                            radius: 6
                            color: rowMouse.pressed ? Bo.surfaceHi : Bo.surface
                            border.color: modelData.status === "present"
                                ? Bo.mix(Bo.surface, "#a8d4a8", 0.40)
                                : Bo.mix(Bo.surface, "#d4a8a8", 0.50)
                            border.width: 1
                            Behavior on height { NumberAnimation { duration: Bo.quick } }
                            Behavior on color { ColorAnimation { duration: Bo.quick } }
                            property bool detailVisible: false

                            Column {
                                id: rowCol
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: Bo.gapSM
                                spacing: 4

                                Row {
                                    width: parent.width
                                    spacing: Bo.gapSM
                                    Rectangle {
                                        anchors.verticalCenter: parent.verticalCenter
                                        width: 8; height: 8; radius: 4
                                        color: modelData.status === "present"
                                            ? "#a8d4a8"
                                            : (modelData.status === "broken"
                                                ? "#d4c89a" : "#d4a8a8")
                                    }
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: modelData.bin
                                        color: Bo.bone
                                        font.pixelSize: Bo.bodySize
                                        font.weight: Font.DemiBold
                                        font.family: "monospace"
                                    }
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: modelData.status === "present" ? "INSTALLED"
                                            : modelData.status === "broken"  ? "BROKEN"
                                            : "MISSING"
                                        color: modelData.status === "present"
                                            ? "#88b888"
                                            : (modelData.status === "broken"
                                                ? "#b8a070" : "#b88888")
                                        font.pixelSize: 9
                                        font.letterSpacing: 1
                                        font.weight: Font.DemiBold
                                    }
                                }

                                // Expanded detail — shown when row is tapped
                                Column {
                                    width: parent.width
                                    spacing: 4
                                    visible: parent.parent.detailVisible
                                    Text {
                                        width: parent.width
                                        text: modelData.purpose
                                        color: Bo.silver
                                        font.pixelSize: 10
                                        wrapMode: Text.Wrap
                                    }
                                    Text {
                                        text: "Used by: " + modelData.used_by
                                        color: Bo.slate
                                        font.pixelSize: 9
                                        wrapMode: Text.Wrap
                                        width: parent.width
                                    }
                                    // Apt one-liner
                                    Text {
                                        text: "apt: sudo apt-get install " + (modelData.apt_package || "(n/a)")
                                        color: Bo.bronzeDim
                                        font.pixelSize: 10
                                        font.family: "monospace"
                                        visible: modelData.status !== "present"
                                                 && (modelData.apt_package || "").length > 0
                                    }
                                    // Source build script
                                    Rectangle {
                                        width: parent.width
                                        height: srcText.implicitHeight + 8
                                        color: Bo.graphite
                                        radius: 4
                                        visible: modelData.status !== "present"
                                                 && (modelData.source_script || "").length > 0
                                        Text {
                                            id: srcText
                                            anchors.fill: parent
                                            anchors.margins: 4
                                            text: modelData.source_script || ""
                                            color: Bo.silver
                                            font.pixelSize: 9
                                            font.family: "monospace"
                                            wrapMode: Text.WrapAnywhere
                                            textFormat: Text.PlainText
                                        }
                                    }
                                    // Upstream link (textual — not a real anchor)
                                    Text {
                                        text: "source: " + (modelData.source_url || "(n/a)")
                                        color: Bo.slate
                                        font.pixelSize: 9
                                        visible: (modelData.source_url || "").length > 0
                                                 && modelData.status !== "present"
                                        wrapMode: Text.WrapAnywhere
                                        width: parent.width
                                    }
                                }
                            }
                            MouseArea {
                                id: rowMouse
                                anchors.fill: parent
                                onClicked: parent.detailVisible = !parent.detailVisible
                            }
                        }
                    }

                    // Re-audit button
                    Rectangle {
                        anchors.right: parent.right
                        width: 88; height: 26
                        radius: 13
                        color: reauditMouse.pressed
                            ? Bo.mix(Bo.surface, root.accent, 0.30)
                            : Bo.surface
                        border.color: root.accent
                        border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: "RE-AUDIT"
                            color: root.accent
                            font.pixelSize: 9
                            font.letterSpacing: 1.2
                            font.weight: Font.DemiBold
                        }
                        MouseArea {
                            id: reauditMouse
                            anchors.fill: parent
                            onClicked: qtobj.audit_kde_tools()
                        }
                    }
                }
            }

            // ── DEPENDENCIES (multi-distro) CARD ──────────────────
            //
            // Surfaces the dep_wrangler audit: which package manager
            // is on this host, which runtime tools are installed, and
            // for missing ones the human-readable install plan with
            // fallback repos (Alpine community/edge, Fedora RPMFusion,
            // Ubuntu universe). The plan_text is intentionally
            // copy-pastable — we don't run sudo from the GUI.
            ControlCard {
                id: depCard
                width: parent.width
                label: "RUNTIME DEPENDENCIES"
                accent: root.accent
                property var depList: {
                    try { return JSON.parse(qtobj.dep_audit_json || "[]") }
                    catch (e) { return [] }
                }

                Component.onCompleted: qtobj.refresh_dep_audit()
                Connections {
                    target: qtobj
                    // `parent` inside a Connections handler refers to the
                    // Connections element itself, NOT the surrounding card —
                    // hence the "Cannot assign to non-existent property
                    // 'depList'" log line. Use the explicit `depCard` id
                    // to reach the actual property holder.
                    function onDep_audit_changed() {
                        try { depCard.depList = JSON.parse(qtobj.dep_audit_json || "[]") }
                        catch (e) { depCard.depList = [] }
                    }
                }

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "Runtime audio + Bluetooth tools by package manager. "
                            + "Detected: <b>" + (qtobj.dep_pm_label || "unknown") + "</b>"
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                        textFormat: Text.RichText
                    }

                    Repeater {
                        model: parent.depList
                        delegate: Rectangle {
                            width: parent.width
                            height: rowContent.implicitHeight + 16
                            radius: 6
                            color: Bo.mix(Bo.surface, Bo.graphite, 0.4)
                            border.color: modelData.installed ? "#4a7c59" : "#8E4A3D"
                            border.width: 1
                            property bool expanded: false

                            Column {
                                id: rowContent
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: 8
                                spacing: 4

                                Row {
                                    width: parent.width
                                    spacing: Bo.gapSM
                                    Rectangle {
                                        width: 16; height: 16; radius: 8
                                        anchors.verticalCenter: parent.verticalCenter
                                        color: modelData.installed ? "#4a7c59" : "#8E4A3D"
                                        Text {
                                            anchors.centerIn: parent
                                            text: modelData.installed ? "✓" : "✗"
                                            color: Bo.bone
                                            font.pixelSize: 10
                                            font.weight: Font.Bold
                                        }
                                    }
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: modelData.friendly_name + "  "
                                        color: Bo.bone
                                        font.pixelSize: Bo.captionSize
                                        font.weight: Font.Medium
                                    }
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: "(" + modelData.binary + ")"
                                        color: Bo.silver
                                        font.pixelSize: 10
                                    }
                                }

                                // Install plan (only when missing).
                                Text {
                                    visible: !modelData.installed && parent.parent.expanded
                                    width: parent.width
                                    text: modelData.plan_text || ""
                                    color: Bo.silver
                                    font.family: "monospace"
                                    font.pixelSize: 10
                                    wrapMode: Text.Wrap
                                    textFormat: Text.PlainText
                                }
                            }

                            MouseArea {
                                anchors.fill: parent
                                enabled: !modelData.installed
                                onClicked: parent.expanded = !parent.expanded
                            }
                        }
                    }

                    // Re-audit button.
                    Rectangle {
                        anchors.right: parent.right
                        width: 96; height: 28; radius: 14
                        color: depRefreshMa.pressed ? Bo.surfaceHi : "transparent"
                        border.color: Bo.hairline; border.width: 1
                        Text { anchors.centerIn: parent
                            text: "↻ Re-audit"; color: Bo.bone
                            font.pixelSize: 10 }
                        MouseArea {
                            id: depRefreshMa
                            anchors.fill: parent
                            onClicked: qtobj.refresh_dep_audit()
                        }
                    }

                    Text {
                        width: parent.width
                        text: "<i>For one-shot install across all tools, run "
                            + "<tt>./scripts/install-deps.sh</tt> from the source tree. "
                            + "It handles Alpine, Debian/Ubuntu, Fedora, Arch, and openSUSE "
                            + "with automatic fallback repo enabling.</i>"
                        color: Bo.slate
                        font.pixelSize: 10
                        wrapMode: Text.Wrap
                        textFormat: Text.RichText
                    }
                }
            }

            // ── PLAYBACK DIAGNOSTIC CARD ─────────────────────────
            ControlCard {
                width: parent.width
                label: "PLAYBACK DIAGNOSTIC"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "When tapping a track does nothing, run this "
                            + "to find out which layer is failing: the local "
                            + "track server, the URL build, or the device API."
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    Row {
                        spacing: Bo.gapSM
                        Rectangle {
                            width: 110; height: 28
                            radius: 14
                            color: diagMouse.pressed
                                ? Bo.mix(Bo.surface, root.accent, 0.35)
                                : Bo.mix(Bo.surface, root.accent, 0.10)
                            border.color: root.accent
                            border.width: 1
                            Text {
                                anchors.centerIn: parent
                                text: "RUN DIAGNOSE"
                                color: root.accent
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            MouseArea {
                                id: diagMouse
                                anchors.fill: parent
                                onClicked: qtobj.diagnose_playback()
                            }
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Results appear in the play status banner"
                            color: Bo.slate
                            font.pixelSize: 10
                            font.italic: true
                        }
                    }
                }
            }

            // ── LIBRARY MAINTENANCE CARD ─────────────────────────
            //
            // Buttons for the heavyweight media-worker operations that
            // weren't reachable from the main UI before:
            //   • QUICK SCAN — walks the configured roots and updates
            //     the catalog (file → row mappings). Doesn't re-run
            //     analysis. Use when you've added a few tracks and
            //     want them visible quickly.
            //   • FULL RESCAN — quick scan + bliss BPM/key analysis on
            //     anything unanalyzed. Long-running. Phase 1 is now
            //     bulk-preloaded so cold scans are dramatically faster
            //     than they used to be.
            //   • ANALYZE BACKLOG — runs bliss on tracks the catalog
            //     has but never analyzed (e.g. analysis was off when
            //     the file was first scanned). Cancellable.
            //
            // The same `scan_progress_json` property that ScanPickerDialog
            // watches is updated by all three buttons; the activity
            // line below reflects whichever scan is current.
            ControlCard {
                width: parent.width
                label: "LIBRARY MAINTENANCE"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        text: "Walk the music roots and update the "
                            + "catalog. Quick Scan is metadata-only; "
                            + "Full Rescan also analyzes BPM/key. "
                            + "Analyze Backlog only runs analysis on "
                            + "tracks that have it pending."
                        width: parent.width
                        color: Bo.slate
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    Row {
                        spacing: Bo.gapSM

                        Rectangle {
                            width: 92; height: 28
                            radius: 14
                            color: quickScanMouse.pressed
                                ? Bo.mix(Bo.surface, root.accent, 0.30)
                                : Bo.surface
                            border.color: root.accent
                            border.width: 1
                            Text {
                                anchors.centerIn: parent
                                text: "QUICK SCAN"
                                color: root.accent
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            MouseArea {
                                id: quickScanMouse
                                anchors.fill: parent
                                onClicked: qtobj.quick_scan()
                            }
                        }

                        Rectangle {
                            width: 100; height: 28
                            radius: 14
                            color: rescanMouse.pressed
                                ? Bo.mix(Bo.surface, root.accent, 0.30)
                                : Bo.surface
                            border.color: root.accent
                            border.width: 1
                            Text {
                                anchors.centerIn: parent
                                text: "FULL RESCAN"
                                color: root.accent
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            MouseArea {
                                id: rescanMouse
                                anchors.fill: parent
                                onClicked: qtobj.rescan_library()
                            }
                        }

                        Rectangle {
                            width: 116; height: 28
                            radius: 14
                            color: analyzeMouse.pressed
                                ? Bo.mix(Bo.surface, root.accent, 0.30)
                                : Bo.surface
                            border.color: root.accent
                            border.width: 1
                            Text {
                                anchors.centerIn: parent
                                text: "ANALYZE BACKLOG"
                                color: root.accent
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            MouseArea {
                                id: analyzeMouse
                                anchors.fill: parent
                                onClicked: qtobj.analyze_backlog()
                            }
                        }

                        Rectangle {
                            width: 76; height: 28
                            radius: 14
                            color: cancelMouse.pressed
                                ? Bo.mix(Bo.surface, Bo.danger, 0.30)
                                : Bo.surface
                            border.color: Bo.danger
                            border.width: 1
                            Text {
                                anchors.centerIn: parent
                                text: "CANCEL"
                                color: Bo.danger
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            MouseArea {
                                id: cancelMouse
                                anchors.fill: parent
                                onClicked: qtobj.cancel_analysis()
                            }
                        }
                    }

                    // Live scan-progress activity line. Reads from
                    // scan_phase / scan_done / scan_total, which the
                    // media worker publishes on every progress tick
                    // (every 500 files in Phase 1, every 100 in
                    // Phase 2, every 10 in Phase 3).
                    Text {
                        width: parent.width
                        text: {
                            var phase = String(qtobj.scan_phase || "")
                            if (phase === "" || phase === "Idle") return ""
                            if (phase === "Done") return "scan complete"
                            if (qtobj.scan_total > 0) {
                                return phase + " — "
                                    + qtobj.scan_done + " / "
                                    + qtobj.scan_total
                            }
                            return phase + " — " + qtobj.scan_done
                        }
                        color: Bo.slate
                        font.pixelSize: 10
                        font.italic: true
                        visible: text.length > 0
                    }
                }
            }

            // ── MANUAL DEVICE ADD CARD ──────────────────────────
            //
            // Fallback for when mDNS discovery doesn't find the
            // user's B&O device. Common cases:
            //   • Alpine / postmarketOS phones with no mDNS
            //     resolver running, or running but with multicast
            //     blocked at the link layer (some carriers do
            //     this on their cell networks).
            //   • Wired-only setups where the device is on a
            //     different VLAN or subnet from the controller.
            //   • Mesh-Wi-Fi setups where the AP filters multicast
            //     between SSIDs.
            //
            // The user types a name + URL like `http://192.168.2.12`
            // and the device shows up in the device picker as if
            // it had been discovered. Same de-dup rules apply
            // (a URL already in the list doesn't add a duplicate).
            ControlCard {
                width: parent.width
                label: "ADD DEVICE MANUALLY"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        text: "Use when auto-discovery doesn't find "
                            + "your B&O device — common on phone networks "
                            + "with multicast blocked, or when the device "
                            + "is on a different subnet."
                        width: parent.width
                        color: Bo.slate
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    TextField {
                        id: addNameField
                        width: parent.width
                        placeholderText: "Name (e.g. Beosound Stage)"
                        color: Bo.bone
                        placeholderTextColor: Bo.slate
                        background: Rectangle {
                            color: Bo.surface
                            border.color: Bo.surfaceHi
                            border.width: 1
                            radius: 6
                        }
                        font.pixelSize: 12
                    }

                    TextField {
                        id: addUrlField
                        width: parent.width
                        placeholderText: "URL (e.g. http://192.168.2.12)"
                        color: Bo.bone
                        placeholderTextColor: Bo.slate
                        background: Rectangle {
                            color: Bo.surface
                            border.color: Bo.surfaceHi
                            border.width: 1
                            radius: 6
                        }
                        font.pixelSize: 12
                        inputMethodHints: Qt.ImhUrlCharactersOnly
                    }

                    Row {
                        spacing: Bo.gapSM

                        Rectangle {
                            width: 96; height: 28
                            radius: 14
                            // Disabled-look when fields are empty.
                            property bool canAdd:
                                addNameField.text.length > 0
                                && addUrlField.text.length > 0
                                && addUrlField.text.indexOf("http") === 0
                            color: addMouse.pressed
                                ? Bo.mix(Bo.surface, root.accent, 0.30)
                                : Bo.surface
                            border.color: canAdd ? root.accent : Bo.slate
                            border.width: 1
                            opacity: canAdd ? 1.0 : 0.5
                            Text {
                                anchors.centerIn: parent
                                text: "ADD"
                                color: parent.canAdd ? root.accent : Bo.slate
                                font.pixelSize: 9
                                font.letterSpacing: 1.2
                                font.weight: Font.DemiBold
                            }
                            MouseArea {
                                id: addMouse
                                anchors.fill: parent
                                enabled: parent.canAdd
                                onClicked: {
                                    qtobj.add_device(
                                        addNameField.text.trim(),
                                        addUrlField.text.trim())
                                    // Clear fields after add so
                                    // the user can spot the
                                    // device picker refreshed.
                                    addNameField.text = ""
                                    addUrlField.text = ""
                                    addedToast.visible = true
                                    toastTimer.restart()
                                }
                            }
                        }
                        Text {
                            id: addedToast
                            anchors.verticalCenter: parent.verticalCenter
                            text: "✓ added to device list"
                            color: Bo.success
                            font.pixelSize: 11
                            font.italic: true
                            visible: false
                        }
                    }
                    Timer {
                        id: toastTimer
                        interval: 2500
                        onTriggered: addedToast.visible = false
                    }
                }
            }

            // ── BLUETOOTH AUTO-CONNECT CARD ──────────────────────
            // ── BT system-respect audit ────────────────────────────
            //
            // Shows what the controller has done to the user's BT
            // landscape: how many devices were already connected at
            // startup vs how many the controller caused to connect,
            // and a live log of every connect/disconnect event in
            // the session. The principle: don't touch what wasn't
            // touched first; clean up only what we caused.
            ControlCard {
                width: parent.width
                label: "BLUETOOTH RESPECT"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "Audit of what the controller has changed in your "
                            + "Bluetooth state this session. Devices connected "
                            + "before the app started are left alone on quit. "
                            + "Devices the app connected — and that weren't "
                            + "connected before — get disconnected on close "
                            + "or after 15 minutes of idle."
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    Rectangle {
                        width: parent.width
                        height: 1
                        color: Bo.hairline
                    }

                    Text {
                        width: parent.width
                        text: qtobj.bt_respect_summary || "snapshot pending"
                        color: Bo.bone
                        font.pixelSize: 11
                        font.weight: Font.DemiBold
                        wrapMode: Text.Wrap
                    }

                    // Audit log — scrollable, newest-first, capped at
                    // 200 entries server-side. Tap rows are read-only.
                    ScrollView {
                        width: parent.width
                        height: Math.min(220, btRespectLogColumn.implicitHeight)
                        clip: true
                        visible: btRespectLogColumn.children.length > 0

                        Column {
                            id: btRespectLogColumn
                            width: parent.width
                            spacing: 2

                            Repeater {
                                model: {
                                    try {
                                        return JSON.parse(qtobj.bt_respect_log_json || "[]")
                                    } catch (e) { return [] }
                                }
                                delegate: Rectangle {
                                    width: parent.width
                                    height: 38
                                    radius: 4
                                    color: "transparent"
                                    border.color: Bo.hairline
                                    border.width: 1

                                    Row {
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        spacing: 8

                                        // Action chip — color-coded so
                                        // the user can scan for what
                                        // happened at a glance.
                                        Rectangle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            width: 110; height: 22
                                            radius: 11
                                            color: {
                                                var a = modelData.action || ""
                                                if (a === "connect_succeeded") return Bo.mix(Bo.surface, Bo.teal, 0.30)
                                                if (a === "connect_failed")    return Bo.mix(Bo.surface, Bo.rust, 0.30)
                                                if (a === "disconnect_attempted") return Bo.mix(Bo.surface, Bo.mauve, 0.30)
                                                if (a === "snapshot")          return Bo.mix(Bo.surface, Bo.silver, 0.30)
                                                return "transparent"
                                            }
                                            border.color: Bo.hairline; border.width: 1
                                            Text {
                                                anchors.centerIn: parent
                                                text: modelData.action || "—"
                                                color: Bo.bone
                                                font.pixelSize: 8
                                                font.weight: Font.DemiBold
                                                font.letterSpacing: 0.5
                                            }
                                        }

                                        Column {
                                            anchors.verticalCenter: parent.verticalCenter
                                            spacing: 1
                                            Text {
                                                text: modelData.name && modelData.name.length > 0
                                                    ? modelData.name
                                                    : (modelData.mac || "—")
                                                color: Bo.bone
                                                font.pixelSize: 10
                                                font.weight: Font.DemiBold
                                                elide: Text.ElideRight
                                                width: 200
                                            }
                                            Text {
                                                text: (modelData.reason || "")
                                                    + "  ·  t+" + (modelData.at_secs || 0) + "s"
                                                color: Bo.slate
                                                font.pixelSize: 8
                                                elide: Text.ElideRight
                                                width: 200
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ── Idle threshold slider ────────────────────
                    //
                    // How long the controller waits with no user
                    // activity before disconnecting devices it
                    // caused to connect. 1..60 minutes. Persisted.
                    Rectangle {
                        width: parent.width
                        height: 1
                        color: Bo.hairline
                    }
                    Row {
                        width: parent.width
                        spacing: Bo.gapSM

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Idle cleanup after"
                            color: Bo.bone
                            font.pixelSize: 11
                            width: 130
                        }

                        // Slider track
                        Rectangle {
                            id: idleSliderTrack
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - 130 - 70 - Bo.gapSM * 2
                            height: 4
                            radius: 2
                            color: Bo.graphite
                            // Filled portion shows current value 1..60
                            Rectangle {
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                height: parent.height
                                radius: 2
                                width: parent.width
                                    * Math.max(1, Math.min(60,
                                        qtobj.bt_respect_idle_minutes || 15)) / 60
                                color: Bo.mauve
                            }
                            // Thumb (positioned by absolute pixel offset)
                            Rectangle {
                                width: 14; height: 14
                                radius: 7
                                color: Bo.bone
                                border.color: Bo.mauve; border.width: 2
                                anchors.verticalCenter: parent.verticalCenter
                                x: parent.width * Math.max(1, Math.min(60,
                                        qtobj.bt_respect_idle_minutes || 15)) / 60
                                    - width / 2
                            }
                            // Drag/tap surface — extends above and below
                            // for easier touch hit.
                            MouseArea {
                                anchors.fill: parent
                                anchors.topMargin: -10
                                anchors.bottomMargin: -10
                                onPositionChanged: if (pressed) {
                                    var frac = Math.max(0,
                                        Math.min(1, mouseX / width))
                                    // Snap to integer minutes between 1 and 60.
                                    var minutes = Math.round(1 + frac * 59)
                                    qtobj.set_bt_respect_idle_minutes(minutes)
                                }
                                onClicked: {
                                    var frac = Math.max(0,
                                        Math.min(1, mouseX / width))
                                    var minutes = Math.round(1 + frac * 59)
                                    qtobj.set_bt_respect_idle_minutes(minutes)
                                }
                            }
                        }

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: (qtobj.bt_respect_idle_minutes || 15) + " min"
                            color: Bo.bone
                            font.pixelSize: 11
                            font.weight: Font.DemiBold
                            width: 70
                            horizontalAlignment: Text.AlignRight
                        }
                    }

                    // Manual "clean up now" button — runs the same
                    // cleanup that fires on app close. Useful for
                    // user testing or when they just want their BT
                    // state back to baseline immediately.
                    Rectangle {
                        anchors.right: parent.right
                        width: 150; height: 32
                        radius: 16
                        color: cleanupMouse.pressed
                            ? Bo.mix(Bo.surface, Bo.mauve, 0.40)
                            : Bo.mix(Bo.surface, Bo.mauve, 0.15)
                        border.color: Bo.mauve; border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: "Clean up now"
                            color: Bo.bone
                            font.pixelSize: 10
                            font.weight: Font.DemiBold
                        }
                        MouseArea {
                            id: cleanupMouse
                            anchors.fill: parent
                            onClicked: qtobj.bt_cleanup_owned()
                        }
                    }
                }
            }

            ControlCard {
                width: parent.width
                label: "BLUETOOTH AUTO-CONNECT"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "Per-device toggles. When enabled, the controller "
                            + "periodically attempts to reconnect this device "
                            + "(via bluetoothctl) with exponential backoff."
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    // Discover paired devices from the system
                    Rectangle {
                        anchors.right: parent.right
                        width: 130; height: 28
                        radius: 14
                        color: discoverMouse.pressed
                            ? Bo.mix(Bo.surface, root.accent, 0.35)
                            : Bo.mix(Bo.surface, root.accent, 0.10)
                        border.color: root.accent
                        border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: "DISCOVER PAIRED"
                            color: root.accent
                            font.pixelSize: 9
                            font.letterSpacing: 1.2
                            font.weight: Font.DemiBold
                        }
                        MouseArea {
                            id: discoverMouse
                            anchors.fill: parent
                            onClicked: qtobj.discover_bt_devices()
                        }
                    }

                    // List managed devices
                    Repeater {
                        model: {
                            try { return JSON.parse(qtobj.bt_devices_json || "[]") }
                            catch (e) { return [] }
                        }
                        delegate: Rectangle {
                            width: contentColumn.width - 2
                            height: rowExpanded ? 110 : 56
                            radius: 6
                            color: Bo.surface
                            border.color: modelData.auto_connect
                                ? root.accent : Bo.hairline
                            border.width: 1
                            Behavior on height { NumberAnimation { duration: Bo.quick } }
                            property bool rowExpanded: false

                            // Top row — status, name, toggle
                            Row {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: Bo.gapSM
                                spacing: Bo.gapSM
                                height: 40
                                // Status dot
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 10; height: 10; radius: 5
                                    color: modelData.status === "connected" ? "#a8d4a8"
                                        : modelData.status === "connecting" ? root.accent
                                        : modelData.status === "backoff"    ? "#d4c89a"
                                        : modelData.status === "queued"     ? "#d4c89a"
                                        : Bo.slate
                                    SequentialAnimation on opacity {
                                        running: modelData.status === "connecting"
                                        loops: Animation.Infinite
                                        NumberAnimation { to: 0.3; duration: 700 }
                                        NumberAnimation { to: 1.0; duration: 700 }
                                    }
                                }
                                Column {
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 2
                                    Text {
                                        text: modelData.name || modelData.mac
                                        color: Bo.bone
                                        font.pixelSize: Bo.bodySize
                                        font.weight: Font.DemiBold
                                    }
                                    Text {
                                        text: String(modelData.status || "").toUpperCase()
                                            + " · " + modelData.mac
                                        color: Bo.slate
                                        font.pixelSize: 9
                                        font.family: "monospace"
                                        font.letterSpacing: 0.5
                                    }
                                }
                            }
                            // Toggle on the right (overlay)
                            Switch {
                                anchors.top: parent.top
                                anchors.right: parent.right
                                anchors.topMargin: 10
                                anchors.rightMargin: Bo.gapSM
                                checked: modelData.auto_connect
                                onToggled: qtobj.set_device_auto_connect(
                                    modelData.mac, modelData.name, checked)
                            }

                            // Expand-tap target (the row itself, except the Switch area)
                            MouseArea {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.rightMargin: 64   // leave the Switch tappable
                                anchors.top: parent.top
                                height: 56
                                onClicked: parent.rowExpanded = !parent.rowExpanded
                            }

                            // Expanded actions: Disconnect / Forget
                            Row {
                                anchors.bottom: parent.bottom
                                anchors.right: parent.right
                                anchors.margins: Bo.gapSM
                                spacing: Bo.gapSM
                                visible: parent.rowExpanded
                                Rectangle {
                                    width: 90; height: 26; radius: 13
                                    color: dcMouse.pressed
                                        ? Bo.mix(Bo.surface, "#d4c89a", 0.30)
                                        : Bo.surface
                                    border.color: "#b8a070"
                                    border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        text: "DISCONNECT"
                                        color: "#d4c89a"
                                        font.pixelSize: 9
                                        font.letterSpacing: 1.0
                                        font.weight: Font.DemiBold
                                    }
                                    MouseArea {
                                        id: dcMouse
                                        anchors.fill: parent
                                        onClicked: qtobj.disconnect_bt_device(modelData.mac)
                                    }
                                }
                                Rectangle {
                                    width: 78; height: 26; radius: 13
                                    color: forgetMouse.pressed
                                        ? Bo.mix(Bo.surface, "#d4a8a8", 0.40)
                                        : Bo.surface
                                    border.color: "#b88888"
                                    border.width: 1
                                    Text {
                                        anchors.centerIn: parent
                                        text: "FORGET"
                                        color: "#d4a8a8"
                                        font.pixelSize: 9
                                        font.letterSpacing: 1.0
                                        font.weight: Font.DemiBold
                                    }
                                    MouseArea {
                                        id: forgetMouse
                                        anchors.fill: parent
                                        onClicked: qtobj.forget_bt_device(modelData.mac)
                                    }
                                }
                            }
                        }
                    }

                    // Hint text when nothing's been added
                    Text {
                        width: parent.width
                        visible: {
                            try {
                                var arr = JSON.parse(qtobj.bt_devices_json || "[]")
                                return arr.length === 0
                            } catch (e) { return true }
                        }
                        text: "No Bluetooth devices configured yet. Tap "
                            + "DISCOVER PAIRED above to pull from KDE's "
                            + "pairing list, then flip the toggles for "
                            + "devices you want auto-managed."
                        color: Bo.slate
                        font.pixelSize: 10
                        font.italic: true
                        wrapMode: Text.Wrap
                    }
                }
            }

            // ── Active Noise Cancellation ──
            //
            // Toggles + alignment probe + mix sliders for the ANC
            // engine. The engine itself is a feed-forward predictive
            // canceller targeting periodic low-frequency rumble
            // (HVAC, engine drone, fans). The probe button measures
            // the round-trip delay between this device's output and
            // its microphone — needed to phase-align the anti-noise
            // when the speaker is a separate device (BT, B&O).
            ControlCard {
                id: ancCard
                width: parent.width
                label: "ACTIVE NOISE CANCELLATION"
                accent: root.accent

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "Predictive cancellation for periodic low-frequency "
                            + "rumble. Targets ≤300 Hz; above that, head movement "
                            + "takes you in and out of the cancellation zone."
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    // Enable toggle.
                    Row {
                        width: parent.width
                        spacing: Bo.gapMD
                        Text {
                            text: "Enabled"
                            color: Bo.silver
                            font.pixelSize: 12
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Rectangle {
                            width: 80; height: 28
                            radius: 14
                            color: qtobj.anc_enabled ? root.accent : "#3a3a3a"
                            anchors.verticalCenter: parent.verticalCenter
                            Text {
                                anchors.centerIn: parent
                                text: qtobj.anc_enabled ? "ON" : "OFF"
                                color: qtobj.anc_enabled ? "#1a1a1a" : Bo.silver
                                font.pixelSize: 11
                                font.bold: true
                            }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: qtobj.anc_enable(!qtobj.anc_enabled)
                            }
                        }
                        Text {
                            text: qtobj.anc_backend.length > 0
                                ? "mic: " + qtobj.anc_backend
                                : "mic: not started"
                            color: Bo.silver
                            font.pixelSize: 10
                            anchors.verticalCenter: parent.verticalCenter
                            opacity: 0.7
                        }
                    }

                    // Anti-noise gain slider 0..1.
                    Column {
                        width: parent.width
                        spacing: 2
                        Text {
                            text: "Anti-noise gain: " + qtobj.anc_anti_gain.toFixed(2)
                            color: Bo.silver; font.pixelSize: 11
                        }
                        Slider {
                            width: parent.width
                            from: 0; to: 1
                            stepSize: 0.05
                            value: qtobj.anc_anti_gain
                            onMoved: qtobj.anc_set_mix(value, qtobj.anc_music_gain)
                        }
                    }

                    // Music gain slider 0..2.
                    Column {
                        width: parent.width
                        spacing: 2
                        Text {
                            text: "Music gain: " + qtobj.anc_music_gain.toFixed(2)
                            color: Bo.silver; font.pixelSize: 11
                        }
                        Slider {
                            width: parent.width
                            from: 0; to: 2
                            stepSize: 0.05
                            value: qtobj.anc_music_gain
                            onMoved: qtobj.anc_set_mix(qtobj.anc_anti_gain, value)
                        }
                    }

                    // Blend toggle.
                    Row {
                        width: parent.width
                        spacing: Bo.gapMD
                        Text {
                            text: "Blend with music"
                            color: Bo.silver
                            font.pixelSize: 12
                            anchors.verticalCenter: parent.verticalCenter
                        }
                        Rectangle {
                            width: 80; height: 24
                            radius: 12
                            color: qtobj.anc_blend_with_music ? root.accent : "#3a3a3a"
                            anchors.verticalCenter: parent.verticalCenter
                            Text {
                                anchors.centerIn: parent
                                text: qtobj.anc_blend_with_music ? "ON" : "OFF"
                                color: qtobj.anc_blend_with_music ? "#1a1a1a" : Bo.silver
                                font.pixelSize: 11
                            }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: qtobj.anc_set_blend(!qtobj.anc_blend_with_music)
                            }
                        }
                    }

                    // Probe button + alignment status.
                    Row {
                        width: parent.width
                        spacing: Bo.gapMD
                        Rectangle {
                            width: 140; height: 32
                            radius: 16
                            color: probeMouse.pressed ? Qt.darker(root.accent, 1.2) : root.accent
                            Text {
                                anchors.centerIn: parent
                                text: "RUN PROBE"
                                color: "#1a1a1a"
                                font.pixelSize: 11
                                font.bold: true
                            }
                            MouseArea {
                                id: probeMouse
                                anchors.fill: parent
                                // Empty sink = system default; user picks a
                                // BT or other sink via the playback path
                                // separately.
                                onClicked: qtobj.anc_run_probe("")
                            }
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: qtobj.anc_alignment_correlation > 0
                                ? "Aligned: " + qtobj.anc_alignment_ms.toFixed(1)
                                  + " ms  (corr " + (qtobj.anc_alignment_correlation * 100).toFixed(0) + "%)"
                                : "Not yet aligned"
                            color: qtobj.anc_alignment_correlation >= 0.15
                                ? Bo.silver : "#a07050"
                            font.pixelSize: 11
                            wrapMode: Text.Wrap
                        }
                    }
                }
            }

            // ── V1 EXPERIMENTAL FEATURES TOGGLE ──
            //
            // The research arc documented in docs/v1-plan.md
            // classifies features as Stable (validated against real
            // recordings, on by default) or Experimental (works on
            // synthetic test signals but not validated, opt-in).
            // The passby canceller is Experimental until its
            // real-engine performance gap closes. The music masker
            // is Stable and always on.
            ControlCard {
                id: experimentalCard
                width: parent.width
                label: "EXPERIMENTAL FEATURES (RESEARCH MODE)"
                accent: root.accent

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 10

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 12

                        Switch {
                            id: experimentalSwitch
                            checked: qtobj.experimental_enabled
                            onToggled: qtobj.set_experimental(checked)
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2

                            RowLayout {
                                spacing: 8
                                Text {
                                    text: "Enable experimental noise cancellation"
                                    color: "#d4d4d8"
                                    font.pixelSize: 13
                                    font.bold: true
                                }
                                Rectangle {
                                    visible: experimentalSwitch.checked
                                    color: "#ff8c5a"
                                    radius: 3
                                    width: badgeText.width + 8
                                    height: badgeText.height + 4
                                    Text {
                                        id: badgeText
                                        anchors.centerIn: parent
                                        text: "RESEARCH MODE"
                                        color: "#0e1012"
                                        font.pixelSize: 9
                                        font.bold: true
                                    }
                                }
                            }
                            Text {
                                Layout.fillWidth: true
                                text: experimentalSwitch.checked
                                    ? "The passby canceller and other research-mode features will emit anti-noise. Real-engine performance is unvalidated — cancellation MAY MAKE THINGS WORSE in some conditions. Music masker still works regardless."
                                    : "Production mode: only the music masker, drone canceller, and backup-beeper masker are active. Validated against real recordings."
                                color: "#888c91"
                                font.pixelSize: 11
                                wrapMode: Text.WordWrap
                            }
                        }
                    }

                    // Show the feature classification table when
                    // experimental is on, so the user can see
                    // exactly what they're opting into.
                    Rectangle {
                        Layout.fillWidth: true
                        visible: experimentalSwitch.checked
                        color: "#0a0c0e"
                        radius: 6
                        border.color: "#2a2e33"
                        border.width: 1
                        Layout.preferredHeight: featureList.implicitHeight + 16

                        ColumnLayout {
                            id: featureList
                            anchors.fill: parent
                            anchors.margins: 8
                            spacing: 4

                            Text {
                                text: "Active experimental features:"
                                color: "#888c91"
                                font.pixelSize: 11
                                font.italic: true
                            }
                            Repeater {
                                model: [
                                    "Passby canceller (3-harmonic)",
                                    "Per-harmonic phase lock auto-mode",
                                    "Jitter smoother (gated on observed latency)",
                                    "8-harmonic extended mode (still has integration bug)"
                                ]
                                delegate: Text {
                                    text: "• " + modelData
                                    color: "#c08080"
                                    font.pixelSize: 11
                                }
                            }
                        }
                    }
                }
            }

            // ── Overnight Standby Cancellation ──
            //
            // Predictive vehicle pass-by canceller. Sits silent
            // most of the time; fires anti-noise only during a
            // detected car-pass event (engine harmonics rising in
            // the 60-300 Hz band with measurable Doppler shift).
            // Learns per-route transition timers across observed
            // events so successive cars get cancelled earlier.
            ControlCard {
                id: standbyCard
                width: parent.width
                label: "OVERNIGHT STANDBY (PASS-BY CANCEL)"
                accent: root.accent

                // Periodic refresh of the FSM snapshot from the
                // runner's background thread.
                Timer {
                    interval: 500
                    running: qtobj.anc_standby_active
                    repeat: true
                    onTriggered: qtobj.anc_standby_refresh()
                }

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: "Watches the mic for vehicle pass-by events "
                            + "(engine 60-300 Hz + Doppler-detected motion) "
                            + "and emits anti-noise only during detected "
                            + "passes. Run overnight to dampen sports-car "
                            + "noise without continuous output."
                        color: Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                    }

                    Row {
                        width: parent.width
                        spacing: Bo.gapMD
                        Rectangle {
                            width: 140; height: 32
                            radius: 16
                            color: qtobj.anc_standby_active
                                ? "#7c3a3a" : root.accent
                            Text {
                                anchors.centerIn: parent
                                text: qtobj.anc_standby_active
                                    ? "STOP STANDBY" : "START STANDBY"
                                color: "#1a1a1a"
                                font.pixelSize: 11
                                font.bold: true
                            }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: {
                                    if (qtobj.anc_standby_active) {
                                        qtobj.anc_stop_standby()
                                    } else {
                                        qtobj.anc_start_standby("")
                                    }
                                }
                            }
                        }
                        Rectangle {
                            width: 110; height: 32
                            radius: 16
                            color: "#3a3a3a"
                            visible: qtobj.anc_standby_active
                            Text {
                                anchors.centerIn: parent
                                text: "RESET TIMERS"
                                color: Bo.silver
                                font.pixelSize: 10
                            }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: qtobj.anc_reset_standby_timers()
                            }
                        }
                    }

                    // Live FSM phase + frequency display.
                    Column {
                        width: parent.width
                        spacing: 4
                        visible: qtobj.anc_standby_active

                        Row {
                            spacing: Bo.gapMD
                            Text {
                                text: "Phase: "
                                color: Bo.silver
                                font.pixelSize: 11
                            }
                            Text {
                                text: qtobj.anc_standby_phase
                                color: qtobj.anc_standby_phase === "Peak"
                                    ? "#ffb050" :
                                    qtobj.anc_standby_phase === "Quiet"
                                    ? Bo.silver : "#c0a060"
                                font.pixelSize: 11
                                font.bold: qtobj.anc_standby_phase !== "Quiet"
                            }
                        }

                        Text {
                            text: qtobj.anc_standby_fundamental_hz > 0
                                ? "Fundamental: " + qtobj.anc_standby_fundamental_hz.toFixed(0) + " Hz"
                                : "Fundamental: —"
                            color: Bo.silver
                            font.pixelSize: 11
                        }

                        Text {
                            text: "Doppler: " + qtobj.anc_standby_doppler.toFixed(2) + " Hz/s"
                            color: Math.abs(qtobj.anc_standby_doppler) > 0.5
                                ? "#c0a060" : Bo.silver
                            font.pixelSize: 11
                        }

                        Text {
                            text: "Phase lock: "
                                + (qtobj.anc_standby_lock_quality * 100).toFixed(0) + "%"
                            // Green-ish at >50% (canceller working well),
                            // muted gold 20-50% (partial), silver below
                            // (no useful lock — anti-noise is decorrelated
                            // from the source and likely not helping).
                            color: qtobj.anc_standby_lock_quality > 0.5
                                ? "#80c080" :
                                qtobj.anc_standby_lock_quality > 0.2
                                ? "#c0a060" : Bo.silver
                            font.pixelSize: 11
                            font.bold: qtobj.anc_standby_lock_quality > 0.5
                        }

                        Text {
                            text: "Learned timer confidence: "
                                + (qtobj.anc_standby_timer_confidence * 100).toFixed(0) + "%"
                            color: Bo.silver
                            font.pixelSize: 11
                        }
                    }
                }
            }

            // ── Audio capture dump ──
            // Records mic audio in a 30 s ring buffer and saves
            // 40 s WAVs whenever the FSM detects an event. The
            // recorded files train the offline CNN that learns
            // engine-character residual cleanup. See
            // audio_capture_dump.rs for the on-disk format.
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: dumpCol.implicitHeight + 24
                radius: 8
                color: "#181a1c"
                border.color: qtobj.capture_dump_enabled ? "#6080a0" : "#2a2e33"
                border.width: 1

                ColumnLayout {
                    id: dumpCol
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 6

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Text {
                            text: "Capture dump"
                            color: Bo.silver
                            font.pixelSize: 13
                            font.bold: true
                            Layout.fillWidth: true
                        }
                        Switch {
                            checked: qtobj.capture_dump_enabled
                            onToggled: {
                                qtobj.set_capture_dump_enabled(checked);
                            }
                        }
                    }

                    Text {
                        text: qtobj.capture_dump_enabled
                            ? "Recording. " + qtobj.capture_count
                                + " capture(s) on disk."
                            : "Off. Toggle to record engine sounds tonight."
                        color: qtobj.capture_dump_enabled
                            ? "#80c080" : Bo.silver
                        font.pixelSize: 11
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Text {
                            text: "Tag:"
                            color: Bo.silver
                            font.pixelSize: 11
                        }
                        TextField {
                            id: locationTagField
                            placeholderText: "bedroom_window_north"
                            Layout.fillWidth: true
                            // Empty default — user must explicitly tag.
                            // We don't auto-tag because incorrect tags
                            // would pollute the training set later.
                            onEditingFinished: {
                                qtobj.set_capture_location_tag(text);
                            }
                            font.pixelSize: 11
                        }
                    }

                    Button {
                        text: "Trigger manual capture now"
                        enabled: qtobj.capture_dump_enabled
                        Layout.fillWidth: true
                        onClicked: qtobj.trigger_manual_capture()
                    }

                    Text {
                        text: "Files saved to ~/.local/share/bo-controller/"
                            + "captures/ as WAV + JSON sidecar. "
                            + "Budget: 50 files (oldest evicted)."
                        color: Bo.silver
                        font.pixelSize: 10
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                }
            }

            // ── Anti-noise saturation ──
            // Asymmetric soft-clip on the anti-noise output to match
            // real-engine timbre. Pure sin + harmonic anti-noise
            // cancels TONAL content cleanly but leaves the engine's
            // "dirty" saturation character audible. Adding matching
            // saturation to the anti-noise lets the cancellation
            // attack the engine's actual timbre, not just its
            // idealized fundamental.
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: satCol.implicitHeight + 24
                radius: 8
                color: "#181a1c"
                border.color: "#2a2e33"
                border.width: 1

                ColumnLayout {
                    id: satCol
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 6

                    Text {
                        text: "Anti-noise saturation"
                        color: Bo.silver
                        font.pixelSize: 13
                        font.bold: true
                    }

                    Text {
                        text: "Tune to match engine timbre. Gasoline ~0.1, "
                            + "diesel ~0.2, leafblower / 2-stroke ~0.3+."
                        color: Bo.silver
                        font.pixelSize: 10
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Text {
                            text: "Passby:"
                            color: Bo.silver
                            font.pixelSize: 11
                            Layout.preferredWidth: 60
                        }
                        Slider {
                            id: passbySatSlider
                            from: 0.0
                            to: 0.6
                            stepSize: 0.05
                            value: qtobj.passby_saturation
                            Layout.fillWidth: true
                            onMoved: qtobj.set_passby_saturation(value)
                        }
                        Text {
                            text: passbySatSlider.value.toFixed(2)
                            color: Bo.silver
                            font.pixelSize: 11
                            Layout.preferredWidth: 36
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Text {
                            text: "Drone:"
                            color: Bo.silver
                            font.pixelSize: 11
                            Layout.preferredWidth: 60
                        }
                        Slider {
                            id: droneSatSlider
                            from: 0.0
                            to: 0.6
                            stepSize: 0.05
                            value: qtobj.drone_saturation
                            Layout.fillWidth: true
                            onMoved: qtobj.set_drone_saturation(value)
                        }
                        Text {
                            text: droneSatSlider.value.toFixed(2)
                            color: Bo.silver
                            font.pixelSize: 11
                            Layout.preferredWidth: 36
                        }
                    }
                }
            }
        }
    }

    Timer {
        interval: 5000
        running: qtobj.capture_dump_enabled
        repeat: true
        onTriggered: qtobj.refresh_capture_count()
    }

    // Periodic BT scheduler tick — every 10 s. Drives the auto-connect
    // attempts when their backoff elapses.
    Timer {
        interval: 10000
        running: true
        repeat: true
        onTriggered: qtobj.tick_bt_autoconnect()
    }
}
