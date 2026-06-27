// qml/volume_knobs.qml
//
// Bang & Olufsen controller — main shell.
//
// Structure: a swipeable three-page experience, the way B&O's own apps
// move between contexts with calm horizontal transitions.
//
//   Page 1  SOUND    — the inertial volume dial + step keys (the hero)
//   Page 2  STREAM   — source URI + play/stop, with live state
//   Page 3  CONNECT  — auto-route, device identity, link status
//
// Pages adapt per product: if a device's profile omits a control, the
// relevant page is dropped from the swipe set entirely (headphones don't
// get a Stream page, TVs don't get auto-route, etc.).
//
// Motion + materials come from the Bo design-system singleton so the
// whole app shares one physical character.
//
// Bindings to the Rust QtState (`qtobj`):
//   Props: volume, muted, product_name, product_type, products,
//          passthrough, auto_route_kde, stream_uri, kde_media_active, active_url
//   Methods: refresh_volume, set_volume_api, refresh_product_info,
//            toggle_mute, toggle_passthrough, start_stream, stop_stream,
//            toggle_auto_route, tx_check_audio, poll_results

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Window 2.15
import "."

Window {
    id: window
    visible: true
    width: 420
    height: 820
    title: "B&O"
    color: Bo.graphite

    // ── Result pump (main thread) ───────────────────────────────────
    Timer { interval: 100; running: true; repeat: true; onTriggered: qtobj.poll_results() }

    // ── Periodic state sync (paused during dial interaction) ────────
    property bool dialBusy: false
    Timer {
        interval: 3000; running: true; repeat: true
        onTriggered: { if (!dialBusy) { qtobj.refresh_volume(); qtobj.refresh_now_playing() } }
    }

    // ── Per-device queue strip refresh ──────────────────────────────
    // Keep the device strip in BottomTransportBar current. Runs at
    // 5s so we see new queued devices quickly without hammering the
    // DB on every poll tick.
    Timer {
        interval: 5000; running: true; repeat: true
        onTriggered: qtobj.load_queue_devices()
    }

    // ── bo-muxd heartbeat ──────────────────────────────────────────
    //
    // Every 30 s, fire a status poll at bo-muxd. The daemon's
    // activity_watch uses this as a liveness signal — if we stop
    // pinging for >60 s, the daemon enters passive mode (stops
    // spawning new soundscape clips) until any command wakes it.
    //
    // This timer runs regardless of which page is visible, so
    // bo-mux stays "lit" while the user has the app open even if
    // they're not on the Spaces page. When the app closes, the
    // timer stops, the daemon notices ~60 s later, and the
    // soundscape gracefully winds down without us having to send
    // an explicit goodbye. (SpacesPage's own 1.5-s refresh covers
    // the foreground-on-Spaces case; this one covers everything
    // else.)
    Timer {
        interval: 30000; running: true; repeat: true
        onTriggered: {
            if (typeof mux !== "undefined" && mux !== null) {
                mux.refresh()
            }
        }
    }

    // ── Noise-adaptive scene gain relay ────────────────────────────
    //
    // The Rust side updates `qtobj.anc_scene_gain` (from the
    // canceller's loudness observation, passed through the gain
    // curve in noise_adaptive_gain.rs). We mirror that to bo-mux
    // via `mux.set_scene_gain(g)` so the soundscape engine
    // multiplies it into every new voice spawn.
    //
    // The Rust side already dedups (only emits the signal when the
    // gain changes by more than ~2%) so this relay doesn't need
    // its own debounce.
    Connections {
        target: qtobj
        function onAnc_scene_gain_changed() {
            if (typeof mux !== "undefined" && mux !== null) {
                mux.set_scene_gain(qtobj.anc_scene_gain)
            }
        }
    }

    // ── Soundscape ducking ─────────────────────────────────────────
    // When music starts playing, duck the soundscape to 30% so it
    // becomes a background layer rather than competing with music.
    // When music stops, restore to full gain (1.0).
    // This is the "soundscapes as a bottom layer" ask — no code in
    // bo-mux needed; set_scene_gain is already implemented and wired.
    Connections {
        target: qtobj
        function onNow_playing_changed() {
            if (typeof mux !== "undefined" && mux !== null) {
                mux.set_scene_gain(qtobj.np_playing ? 0.30 : 1.0)
            }
        }
    }
    Component.onCompleted: {
        applyMode();
        qtobj.refresh_all();
        // Probe device capabilities so the device picker shows priority
        // badges (battery / network / hub recommendation) right away.
        qtobj.refresh_priorities();
    }

    // App-close cleanup. When the user closes the controller, we
    // STOP any audio the driver layer is producing AND tear down any
    // multi-room combine-sinks we created. Without this:
    //   • paplay PIDs leak as orphan children (still piping audio
    //     to whatever sink they were bound to)
    //   • PulseAudio combine-sink modules stay loaded across
    //     controller restarts, eventually leaking
    //
    // We also call bt_cleanup_owned so any BT devices we caused to
    // connect during this session (but that weren't connected when
    // we started) get disconnected. Devices the user had connected
    // before us are left alone — system-respect policy.
    onClosing: function(close) {
        console.log("app: closing — stopping driver + tearing down")
        qtobj.driver_stop()
        qtobj.clear_multiroom()
        qtobj.bt_cleanup_owned()
        close.accepted = true
    }

    // Idle BT cleanup tick. Every 30s, asks the controller whether
    // we've been idle past the BtRespect threshold. If yes AND
    // nothing's playing through a BT/multiroom driver, the
    // controller disconnects any devices it owns. Keeps the BT
    // landscape clean even if the user just leaves the app running.
    Timer {
        interval: 30000
        running: true
        repeat: true
        onTriggered: qtobj.tick_bt_respect_idle()
    }

    // BT system power-state poll. Every 4s, refreshes whether the
    // adapter is powered. Drives the BtSinkPicker's "BT is off"
    // prompt visibility.
    Timer {
        interval: 4000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: qtobj.refresh_bt_system_powered()
    }

    // ── Product profile resolution ──────────────────────────────────
    property var productMap: {
        try { return JSON.parse(qtobj.products || "{}") } catch (e) { return {} }
    }
    property var profile: productMap[qtobj.product_type]
        || productMap["Unknown Type"]
        || ({ emoji:"🎵", name:"Bang & Olufsen", accent:"#C8A04B",
              accentSoft:"#33291a", shape:"disc", tagline:"Audio device",
              controls:["volume","mute","stream","sources","autoroute"] })

    property color rawAccent:  profile.accent     || "#C8A04B"
    property color accent:      Bo.accentFor(rawAccent)
    property color accentSoft:  profile.accentSoft || "#33291a"
    Behavior on accent     { ColorAnimation { duration: Bo.grand } }
    Behavior on accentSoft { ColorAnimation { duration: Bo.grand } }

    // ── Theme mode: day / dim / night ───────────────────────────────
    // themeMode "auto" follows the clock; otherwise it's a manual override
    // cycled from the header. Drives the Bo singleton, which re-themes the
    // whole app (palette warms + dims, motion slows) at once.
    property string themeMode: "auto"
    function resolvedMode() {
        if (themeMode !== "auto") return themeMode
        var h = new Date().getHours()
        if (h >= 22 || h < 6)  return "night"   // 10pm–6am
        if (h >= 19 || h < 8)  return "dim"     // early morning / evening
        return "day"
    }
    function applyMode() { Bo.mode = resolvedMode() }
    onThemeModeChanged: applyMode()
    // Re-check the clock every few minutes so "auto" drifts into night on its own.
    Timer { interval: 180000; running: themeMode === "auto"; repeat: true; onTriggered: applyMode() }

    function hasControl(n) { var c = profile.controls; return c ? c.indexOf(n) !== -1 : true }

    // Which pages exist for this product (drives the SwipeView + dots)
    property bool pageStream:  hasControl("stream")
    property bool pageConnect: hasControl("autoroute") || true   // always show device info

    // ── Atmospheric backdrop: a faint accent-tinted radial that shifts
    //    with the active page, so the whole canvas breathes with context.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: Bo.mix(Bo.graphite, Bo.bronze, 0.16) }
            GradientStop { position: 0.5; color: Bo.mix(Bo.graphite, window.accentSoft, 0.12) }
            GradientStop { position: 1.0; color: Bo.ink }
        }
    }

    // ── Header ──────────────────────────────────────────────────────
    Item {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.topMargin: 12
        height: 76
        z: 100   // keep the device-selector dropdown above the swipe pages

        DeviceSelector {
            id: deviceSelector
            anchors.left: parent.left
            anchors.leftMargin: Bo.gutter
            anchors.right: modeBtnHolder.left
            anchors.rightMargin: Bo.gapSM
            anchors.verticalCenter: parent.verticalCenter
            accent: window.accent
            profile: window.profile
            z: 50
        }

        // Refresh / re-identify — rotates on tap
        // Theme mode cycle: auto → day → dim → night → auto
        Item {
            id: modeBtnHolder
            anchors.right: refreshHolder.left
            anchors.rightMargin: Bo.gapXS
            anchors.verticalCenter: parent.verticalCenter
            width: 46; height: 46

            Rectangle {
                anchors.fill: parent
                radius: 23
                color: modeArea.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Behavior on color { ColorAnimation { duration: Bo.quick } }

                Text {
                    anchors.centerIn: parent
                    // glyph reflects the *resolved* mode, not the setting
                    text: Bo.mode === "night" ? "☾" : Bo.mode === "dim" ? "◗" : "☀"
                    color: Bo.mode === "day" ? Bo.silver : window.accent
                    font.pixelSize: 17
                    Behavior on color { ColorAnimation { duration: Bo.base } }
                }
            }
            MouseArea {
                id: modeArea
                anchors.fill: parent
                onClicked: {
                    window.themeMode = window.themeMode === "auto"  ? "day"
                                     : window.themeMode === "day"   ? "dim"
                                     : window.themeMode === "dim"   ? "night"
                                     : "auto"
                    // Flash the chosen setting so it's clear what each tap
                    // did — the glyph alone is ambiguous (it shows the
                    // RESOLVED mode, so picking "auto" at night looks the
                    // same as picking "night").
                    modeLabel.show(window.themeMode)
                }
            }

            // Transient label: appears under the theme button on tap,
            // naming the setting just selected, then fades out.
            Rectangle {
                id: modeLabel
                anchors.top: parent.bottom
                anchors.topMargin: 4
                anchors.horizontalCenter: parent.horizontalCenter
                width: modeLabelText.implicitWidth + 16
                height: 22
                radius: 11
                color: Bo.surfaceHi
                border.color: Bo.hairline; border.width: 1
                opacity: 0
                z: 200
                Text {
                    id: modeLabelText
                    anchors.centerIn: parent
                    color: Bo.bone
                    font.pixelSize: 11
                    font.weight: Font.Medium
                }
                Behavior on opacity { NumberAnimation { duration: Bo.base } }
                Timer {
                    id: modeLabelTimer
                    interval: 1400
                    onTriggered: modeLabel.opacity = 0
                }
                function show(mode) {
                    modeLabelText.text = mode === "auto" ? "Auto (follows clock)"
                                       : mode === "day"  ? "Day"
                                       : mode === "dim"  ? "Dim"
                                       : "Night"
                    opacity = 1
                    modeLabelTimer.restart()
                }
            }
        }

        Item {
            id: refreshHolder
            anchors.right: parent.right
            anchors.rightMargin: Bo.gutter
            anchors.verticalCenter: parent.verticalCenter
            width: 46; height: 46

            Rectangle {
                id: refreshBtn
                anchors.fill: parent
                radius: 23
                color: refreshArea.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Behavior on color { ColorAnimation { duration: Bo.quick } }

                Text {
                    id: refreshGlyph
                    anchors.centerIn: parent
                    text: "↻"; color: Bo.silver; font.pixelSize: 18
                    transform: Rotation {
                        id: refreshRot
                        origin.x: refreshGlyph.width/2; origin.y: refreshGlyph.height/2
                        angle: 0
                    }
                }
            }
            RotationAnimation {
                id: spin; target: refreshRot; property: "angle"
                from: 0; to: 360; duration: Bo.grand
                easing.type: Easing.Bezier; easing.bezierCurve: Bo.settle
            }
            MouseArea {
                id: refreshArea
                anchors.fill: parent
                onClicked: { spin.restart(); qtobj.refresh_all() }
            }
        }

        // Hairline under the header
        Rectangle {
            anchors.bottom: parent.bottom
            anchors.left: parent.left; anchors.right: parent.right
            anchors.leftMargin: Bo.gutter; anchors.rightMargin: Bo.gutter
            height: 1; color: Bo.hairline; opacity: 0.6
        }
    }

    // ── Page-name bar (sits between header and the swipe area) ──────
    //
    // Names each accessible page and lets the user jump directly to it
    // by tap. The page indices below are static — they correspond to
    // the order Loaders appear in the SwipeView. When a Loader's
    // `active` property is false, the entry still exists in the model
    // but its index points to a hidden page, so we filter the model
    // by the same `hasControl()` calls.
    //
    // Updating the page list: if you add/remove/reorder Loaders in
    // the SwipeView below, update this model in lockstep.
    PageBar {
        id: pageBar
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: Bo.gutter
        anchors.rightMargin: Bo.gutter
        anchors.topMargin: 6
        accent: window.accent
        currentIndex: pager.currentIndex

        // Build the page list dynamically so it tracks `hasControl()`
        // — products without a library control hide the music pages.
        function buildPages() {
            var out = []
            var idx = 0
            out.push({ label: "Sound", index: idx++ })
            if (window.hasControl("library")) {
                out.push({ label: "Library",   index: idx++ })
                out.push({ label: "Playlists", index: idx++ })
                out.push({ label: "Queue",     index: idx++ })
                out.push({ label: "Artists",   index: idx++ })
                out.push({ label: "Albums",    index: idx++ })
                out.push({ label: "All Tracks", index: idx++ })
                out.push({ label: "Tags",      index: idx++ })
                out.push({ label: "Search",    index: idx++ })
                out.push({ label: "History",   index: idx++ })
            }
            if (window.hasControl("spaces"))  out.push({ label: "Spaces",  index: idx++ })
            out.push({ label: "Loop Studio", index: idx++ })
            out.push({ label: "Mixer",      index: idx++ })
            if (window.pageStream)            out.push({ label: "Stream",  index: idx++ })
            if (window.hasControl("sources")) out.push({ label: "Sources", index: idx++ })
            out.push({ label: "Connect",  index: idx++ })
            out.push({ label: "Settings", index: idx++ })
            return out
        }
        pages: buildPages()
        onPageTapped: (idx) => { pager.currentIndex = idx }
    }

    // ── SOURCE-MISMATCH WARNING BANNER ────────────────────────────
    //
    // Persistent strip just below the page tabs that warns when the
    // active device is in a source that can't accept URI playback
    // (Spotify / AirPlay / Bluetooth / line-in / TV / internet-radio
    // / other streaming services). The amber border + warning glyph
    // mirror the source chip on the NowPlayingBar but at full width,
    // so the user can't miss it while browsing the library.
    //
    // Hidden when:
    //   • No active device (label empty), OR
    //   • Source family is "unknown" (probe hasn't returned yet — we
    //     don't want to nag with a misleading warning during the
    //     2-second post-select window), OR
    //   • Source can_play is true (DLNA / network).
    //
    // Tap → refresh_active_source() in case the user just switched
    // sources on the device but our cache is stale.
    Rectangle {
        id: sourceWarning
        anchors.top: pageBar.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: Bo.gutter
        anchors.rightMargin: Bo.gutter
        anchors.topMargin: 6
        // Two reasons to show the banner:
        //   1. Source family known + can't play (the original case
        //      from the source-warning work): "device is in Spotify
        //      mode, can't play URIs".
        //   2. Firmware family known + NOT Mozart: this device's
        //      REST API doesn't speak the dialect we drive, so the
        //      user should know up front that "Play" via REST won't
        //      work and they should use Bluetooth.
        //
        // We hide on Mozart-healthy combinations (source can play
        // AND firmware is mozart) to avoid nagging when everything's
        // fine. We also hide when both reasons are still "unknown"
        // (probes haven't returned yet) to avoid a flash of warning
        // during the 1-2 second post-startup probe window.
        readonly property bool sourceBlock:
               qtobj.device_source_label
            && qtobj.device_source_label.length > 0
            && qtobj.device_source_family !== ""
            && qtobj.device_source_family !== "unknown"
            && !qtobj.device_source_can_play
        readonly property bool firmwareBlock:
               qtobj.device_firmware_family !== ""
            && qtobj.device_firmware_family !== "unknown"
            && qtobj.device_firmware_family !== "unreachable"
            && qtobj.device_firmware_family !== "mozart"
        visible: sourceBlock || firmwareBlock
        height: visible ? (warnContent.implicitHeight + 12) : 0
        radius: 8
        color: Bo.mix(Bo.surface, "#B58853", 0.18)
        border.color: "#B58853"; border.width: 1

        Row {
            id: warnContent
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: Bo.gapSM
            anchors.rightMargin: Bo.gapSM
            spacing: Bo.gapSM

            // Warning glyph — caution triangle, amber-tinted to
            // match the chip color semantics.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "⚠"
                color: "#B58853"
                font.pixelSize: 14
                font.weight: Font.Bold
            }
            // Block reason. Prefers the firmware-family explanation
            // when that's the blocker, since the source warning is
            // a downstream symptom of the firmware not supporting
            // REST URI playback. Falls through to the source reason
            // otherwise.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 14 - Bo.gapSM*4 - 78 - 88 - 56
                text: sourceWarning.firmwareBlock
                    ? ("Device firmware: " + qtobj.device_firmware_label
                       + " — REST playback unavailable. Driver: "
                       + (qtobj.current_driver_label || "not bound"))
                    : (qtobj.device_source_block_reason
                       || ("Device is in "
                           + (qtobj.device_source_label || "another source")
                           + " mode — can't play URIs."))
                color: Bo.bone
                font.pixelSize: 10
                wrapMode: Text.Wrap
            }
            // Try Network button — attempts to programmatically
            // switch the device's source to Network/DLNA via REST.
            // Only shown when the firmware family is Mozart (other
            // firmwares don't accept these REST commands; offering
            // the button would just frustrate the user). Best-effort
            // even on Mozart, since the source-switch endpoint isn't
            // accepted on every firmware version.
            Rectangle {
                width: 78; height: 22; radius: 11
                anchors.verticalCenter: parent.verticalCenter
                visible: qtobj.device_supports_rest_playback
                color: qtobj.network_switch_running
                    ? Bo.surfaceHi
                    : (tryNetMa.pressed ? Bo.mix(Bo.surface, "#4a7c59", 0.4)
                                         : "transparent")
                border.color: "#4a7c59"; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: qtobj.network_switch_running
                        ? "trying…" : "⇄ Network"
                    color: "#4a7c59"
                    font.pixelSize: 9
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    id: tryNetMa
                    anchors.fill: parent
                    enabled: !qtobj.network_switch_running
                    onClicked: qtobj.try_activate_network()
                }
            }
            // Bluetooth fallback hint — shown on non-Mozart firmware
            // and on Mozart devices where the user might still
            // prefer BT (we surface the option regardless because
            // BT works as a fallback path even when REST is healthy).
            // Tap routes to the device's Bluetooth pairing flow;
            // actual audio routing happens at the OS level via
            // PulseAudio/PipeWire once the device is connected.
            Rectangle {
                width: 88; height: 22; radius: 11
                anchors.verticalCenter: parent.verticalCenter
                visible: qtobj.device_recommends_bluetooth
                color: tryBtMa.pressed ? Bo.mix(Bo.surface, "#5C8A8A", 0.4)
                                       : "transparent"
                border.color: "#5C8A8A"; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "✦ Bluetooth"
                    color: "#5C8A8A"
                    font.pixelSize: 9
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    id: tryBtMa
                    anchors.fill: parent
                    // Open the BT sink picker so the user can pick
                    // which paired BT speaker should be the audio
                    // output. Falls back to navigating to the Connect
                    // page if the picker can't open (e.g. early
                    // startup before window properties are set).
                    onClicked: {
                        if (window.btSinkPicker) {
                            window.btSinkPicker.open()
                        } else {
                            // Connect-page fallback
                            var idx = -1
                            for (var i = 0; i < pageBar.pages.length; ++i) {
                                if (pageBar.pages[i].label === "Connect") {
                                    idx = pageBar.pages[i].index
                                    break
                                }
                            }
                            if (idx >= 0) pager.currentIndex = idx
                        }
                    }
                }
            }
            // Refresh button — small pill on the right. Re-probes
            // the device's source. Useful when the user switches
            // sources on the device itself and wants to dismiss
            // the banner without restarting the controller.
            Rectangle {
                width: 56; height: 22; radius: 11
                anchors.verticalCenter: parent.verticalCenter
                color: warnRefreshMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: "#B58853"; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "↻ Check"
                    color: "#B58853"
                    font.pixelSize: 9
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    id: warnRefreshMa
                    anchors.fill: parent
                    onClicked: {
                        qtobj.refresh_active_source()
                        qtobj.refresh_firmware_family()
                    }
                }
            }
        }
    }

    // ── Swipeable pages ─────────────────────────────────────────────
    SwipeView {
        id: pager
        anchors.top: sourceWarning.visible ? sourceWarning.bottom : pageBar.bottom
        // The bottom bar sits above the dots; pages anchor above the bar.
        anchors.bottom: bottomBar.visible ? bottomBar.top : dots.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.topMargin: Bo.gapSM
        clip: true

        // ░░ PAGE 1 — SOUND ░░
        Item {
            id: soundPage

            // Activity 0..1 drives the watermark. Two contributions:
            //  • a continuous floor from the actual volume level (louder =
            //    faster, brighter strokes), so the watermark reflects how
            //    loud the device is even at rest;
            //  • brief pulses to full on volume changes / playback start.
            // The max of the two wins, then decays back to the volume floor.
            property real volumeFloor: Math.min(1.0, (qtobj.volume / 100) * 0.85)
            property real soundActivity: volumeFloor
            Behavior on soundActivity { NumberAnimation { duration: Bo.slow } }
            Timer {
                id: activityDecay
                interval: 900; repeat: false
                onTriggered: soundPage.soundActivity =
                    Math.max(soundPage.volumeFloor, qtobj.np_playing ? 0.5 : 0.0)
            }
            function pulseActivity() { soundActivity = 1.0; activityDecay.restart() }
            // Keep the resting floor in sync as volume changes between pulses.
            onVolumeFloorChanged: if (!activityDecay.running) soundActivity = volumeFloor
            Connections {
                target: qtobj
                function onVolumeChanged() { soundPage.pulseActivity() }
                function onNow_playing_changed() {
                    soundPage.soundActivity =
                        Math.max(soundPage.volumeFloor, qtobj.np_playing ? 0.5 : 0.0)
                }
            }

            // Product watermark — peeks out top-left, behind everything.
            DialWatermark {
                width: Math.min(parent.width * 0.95, 360)
                height: width
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.leftMargin: -width * 0.28
                anchors.topMargin: -height * 0.18
                shape: window.profile.shape || "disc"
                accent: window.accent
                cutColor: Bo.graphite
                activity: soundPage.soundActivity
                z: -1
            }

            Column {
                anchors.centerIn: parent
                spacing: Bo.gapLG

                VolumeDial {
                    id: dial
                    anchors.horizontalCenter: parent.horizontalCenter
                    diameter: 256
                    value: qtobj.volume
                    muted: qtobj.muted
                    accent: window.accent
                    accentSoft: window.accentSoft
                    trackColor: Bo.mix(Bo.surfaceHi, Bo.bronze, 0.30)
                    subTextColor: Bo.silver
                    textColor: Bo.bone

                    onInteractionStarted: window.dialBusy = true
                    onInteractionEnded:   window.dialBusy = false
                    onValueChangeRequested: function(v) { volCommit.target = v; volCommit.restart() }

                    Connections {
                        target: qtobj
                        function onVolumeChanged() { if (!dial.dragging) dial.value = qtobj.volume }
                    }
                }

                // Debounce dial → device so a flick doesn't fire 60 PUTs.
                // Commits the latest value ~every 60ms while moving.
                // Routes through set_volume_from_gesture so the friction
                // filter can guard against wraparound-jump artifacts. The
                // ± StepKeys below stay on the direct path (set_volume_api)
                // — those are deliberate single-unit gestures that should
                // always be responsive.
                Timer {
                    id: volCommit
                    property real target: 0
                    interval: 60; repeat: false
                    onTriggered: qtobj.set_volume_from_gesture(Math.round(target))
                }

                Row {
                    anchors.horizontalCenter: parent.horizontalCenter
                    spacing: Bo.gapMD

                    StepKey {
                        label: "−"; accent: window.accent
                        onTriggered: qtobj.set_volume_api(Math.max(0, qtobj.volume - 1))
                    }
                    StepKey {
                        visible: window.hasControl("mute")
                        label: "MUTE"; wide: true; small: true
                        active: qtobj.muted; accent: window.accent
                        onTriggered: qtobj.toggle_mute()
                    }
                    StepKey {
                        label: "+"; accent: window.accent
                        onTriggered: qtobj.set_volume_api(Math.min(100, qtobj.volume + 1))
                    }
                }

                // Now-playing strip — appears only when there's something to show.
                NowPlayingBar {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 256
                    accent: window.accent
                    title: qtobj.np_title
                    artist: qtobj.np_artist
                    source: qtobj.np_source
                    playing: qtobj.np_playing
                    visible: window.hasControl("nowplaying")
                             && (qtobj.np_title.length > 0 || qtobj.np_source.length > 0)
                    onPlayPause: qtobj.transport(qtobj.np_playing ? "pause" : "play")
                    onNext: qtobj.transport("forward")
                    onPrev: qtobj.transport("back")
                }

                // ── Output-sink chip ────────────────────────────────
                //
                // Prominently surfaces where audio is *actually* going
                // right now — controller speakers, a specific BT
                // speaker, or a multi-room set. Tap to open the
                // BtSinkPicker and change the route. This is the
                // answer to the "why is it still on Emerge01?" surprise
                // — the user sees the destination at all times.
                //
                // When audio is routing through a non-Mozart driver
                // (local / bluetooth / multiroom), a Stop button on
                // the right side gives the user a panic-stop. Useful
                // when BT disconnects mid-playback and audio falls
                // back to laptop speakers — they can kill it without
                // hunting through menus.
                Row {
                    anchors.horizontalCenter: parent.horizontalCenter
                    spacing: Bo.gapSM
                    height: 38
                    // Always visible — the user always wants to know
                    // where audio will go if they tap a track.

                    property color chipBorder: {
                        switch (qtobj.current_driver_kind) {
                            case "bluetooth": return "#5C8A8A"
                            case "multiroom": return "#C8A04B"
                            case "mozart":    return window.accent
                            case "noop":      return "#8E4A3D"
                            default:          return Bo.hairline
                        }
                    }
                    property string chipIcon: {
                        switch (qtobj.current_driver_kind) {
                            case "bluetooth": return "✦"
                            case "multiroom": return "🔊+"
                            case "mozart":    return "⇄"
                            case "noop":      return "✕"
                            default:          return "♪"
                        }
                    }
                    property string chipLabel: {
                        if (qtobj.current_driver_kind === "bluetooth") {
                            return "→ " + (qtobj.active_bt_name
                                || qtobj.active_bt_mac
                                || "Bluetooth")
                        }
                        if (qtobj.current_driver_kind === "multiroom") {
                            try {
                                var j = JSON.parse(qtobj.active_multiroom_json || "null")
                                if (j && j.names && j.names.length > 0) {
                                    return "→ " + j.names.join(" + ")
                                }
                            } catch (e) {}
                            return "→ Multi-room"
                        }
                        if (qtobj.current_driver_kind === "mozart") {
                            return "→ " + (qtobj.product_name || "device")
                        }
                        if (qtobj.current_driver_kind === "noop") {
                            return "→ no output"
                        }
                        return "→ this device's speakers"
                    }
                    property bool driverPlaysLocally:
                        qtobj.current_driver_kind === "local"
                        || qtobj.current_driver_kind === "bluetooth"
                        || qtobj.current_driver_kind === "multiroom"

                    // Main chip — tap to open the sink picker.
                    Rectangle {
                        width: parent.driverPlaysLocally ? 268 : 320
                        height: 38
                        radius: 19
                        color: outputChipMa.pressed
                            ? Bo.surfaceHi
                            : Bo.mix(Bo.surface, parent.chipBorder, 0.15)
                        border.color: parent.chipBorder
                        border.width: 1
                        Behavior on color { ColorAnimation { duration: Bo.quick } }

                        Row {
                            anchors.centerIn: parent
                            spacing: 8
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "PLAYING TO"
                                color: Bo.silver
                                font.pixelSize: 8
                                font.letterSpacing: 1.5
                                font.weight: Font.DemiBold
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: parent.parent.parent.chipIcon
                                color: parent.parent.parent.chipBorder
                                font.pixelSize: 12
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: parent.parent.parent.chipLabel
                                color: Bo.bone
                                font.pixelSize: 10
                                font.weight: Font.DemiBold
                                elide: Text.ElideRight
                                width: Math.min(implicitWidth,
                                    parent.parent.parent.driverPlaysLocally
                                    ? 150 : 200)
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "▾"
                                color: Bo.slate
                                font.pixelSize: 9
                            }
                        }
                        MouseArea {
                            id: outputChipMa
                            anchors.fill: parent
                            onClicked: {
                                if (window.btSinkPicker) {
                                    window.btSinkPicker.open()
                                }
                            }
                        }
                    }

                    // Stop button — only when audio could be playing
                    // through a driver we control. Mozart routes
                    // through the device itself; we don't try to
                    // panic-stop those (the existing transport
                    // controls handle that).
                    Rectangle {
                        visible: parent.driverPlaysLocally
                        width: 44; height: 38; radius: 19
                        color: stopMa.pressed
                            ? Bo.mix(Bo.surface, "#B58853", 0.50)
                            : Bo.mix(Bo.surface, "#B58853", 0.20)
                        border.color: "#B58853"; border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: "■"
                            color: "#B58853"
                            font.pixelSize: 14
                            font.weight: Font.DemiBold
                        }
                        MouseArea {
                            id: stopMa
                            anchors.fill: parent
                            onClicked: {
                                qtobj.driver_stop()
                            }
                        }
                    }
                }

                // Play-attempt status strip — surfaces the outcome of tapping
                // a library track (which endpoint accepted, or why the device
                // refused). Visible only while there's a fresh status; auto-
                // dismissed by a Timer after 5 s when in a "settled" state.
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 280
                    height: playStatusText.implicitHeight + 8
                    visible: qtobj.last_play_status.length > 0
                    opacity: visible ? 1.0 : 0.0
                    Behavior on opacity { NumberAnimation { duration: Bo.base } }

                    Rectangle {
                        anchors.fill: parent
                        radius: 6
                        color: Bo.surface
                        opacity: 0.6
                    }
                    Text {
                        id: playStatusText
                        anchors.fill: parent
                        anchors.margins: 4
                        text: qtobj.last_play_status
                        color: {
                            var s = String(qtobj.last_play_status || "")
                            if (s.indexOf("playing via") === 0) return "#a8d4a8"
                            if (s.indexOf("instant") === 0)     return "#a8d4a8"
                            if (s.indexOf("requesting") === 0)  return Bo.silver
                            if (s.indexOf("resolving") === 0)   return Bo.silver
                            return "#d4a8a8"
                        }
                        font.pixelSize: Bo.microSize
                        font.letterSpacing: 0.5
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                        wrapMode: Text.WordWrap
                        elide: Text.ElideRight
                        maximumLineCount: 2
                    }

                    Timer {
                        // Clear "settled" statuses after a beat. We don't
                        // clear "requesting…" because that's a live state
                        // we want to remain until the outcome arrives.
                        // Block-reason messages (Device source can't /
                        // Device is in / Could not detect) are long-form
                        // and deserve more reading time.
                        interval: {
                            var s = String(qtobj.last_play_status || "")
                            if (s.indexOf("Device source") === 0
                             || s.indexOf("Device is in") === 0
                             || s.indexOf("Could not detect") === 0) return 9000
                            return 5000
                        }
                        running: qtobj.last_play_status.length > 0
                                 && qtobj.last_play_status.indexOf("requesting") !== 0
                        onTriggered: {
                            // Calling stop_track_playback would actually stop
                            // playback. We just want to clear the *banner*.
                            // The property is read-only from QML; use the
                            // dedicated clear method.
                            qtobj.dismiss_play_status()
                        }
                    }
                }

                // MPRIS context hint — shows what's playing in *other apps*
                // on this machine. Helpful when the user has Spotify
                // running on their laptop and walks up to the controller:
                // they can see at a glance what would happen if they hit
                // play on the speaker. Hidden when no MPRIS player is
                // active; one-line, low contrast so it never competes
                // with the device's own now-playing strip.
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 280
                    height: visible ? mprisText.implicitHeight + 4 : 0
                    visible: qtobj.mpris_playing && qtobj.mpris_title.length > 0
                    Text {
                        id: mprisText
                        anchors.fill: parent
                        anchors.margins: 2
                        text: "ALSO PLAYING · "
                            + String(qtobj.mpris_player || "").toUpperCase()
                            + " · " + qtobj.mpris_title
                            + (qtobj.mpris_artist.length > 0
                                ? " — " + qtobj.mpris_artist : "")
                        color: Bo.bronzeDim
                        font.pixelSize: 9
                        font.letterSpacing: 0.8
                        horizontalAlignment: Text.AlignHCenter
                        elide: Text.ElideRight
                    }
                }
            }
        }

        // ░░ PAGE — LIBRARY ░░
        Loader {
            active: window.hasControl("library")
            sourceComponent: Item {
                id: libRoot
                // View navigation state. Modes:
                //   "recent"     → flat list of recently-modified tracks
                //   "artists"    → list of artist groups (tap to drill)
                //   "tags"       → list of tag groups (tap to drill)
                //   "artist:N"   → tracks for artist id N (back to "artists")
                //   "tag:N"      → tracks for tag id N (back to "tags")
                //   "search"     → search results (driven by the search field)
                property string viewMode: "recent"
                property string viewLabel: ""   // human label for drill views

                function goRecent()  { searchInput.text = ""; viewMode = "recent";  viewLabel = ""; qtobj.load_recent() }
                function goArtists() { searchInput.text = ""; viewMode = "artists"; viewLabel = ""; qtobj.load_artists() }
                function goTags()    { searchInput.text = ""; viewMode = "tags";    viewLabel = ""; qtobj.load_tags() }
                function openArtist(id, name) {
                    searchInput.text = ""
                    viewMode = "artist:" + id; viewLabel = name; qtobj.load_by_artist(id)
                }
                function openTag(id, name) {
                    searchInput.text = ""
                    viewMode = "tag:" + id; viewLabel = name; qtobj.load_by_tag(id)
                }
                function goBack() {
                    if (viewMode.indexOf("artist:") === 0) goArtists()
                    else if (viewMode.indexOf("tag:") === 0) goTags()
                    else goRecent()
                }

                // Are we showing tracks (recent/search/artist:N/tag:N) or
                // groups (artists/tags)?
                readonly property bool showingTracks:
                    viewMode === "recent" || viewMode === "search"
                    || viewMode.indexOf("artist:") === 0
                    || viewMode.indexOf("tag:") === 0
                readonly property bool showingArtists: viewMode === "artists"
                readonly property bool showingTags:    viewMode === "tags"
                readonly property bool inDrillDown:
                    viewMode.indexOf("artist:") === 0 || viewMode.indexOf("tag:") === 0

                property var trackList: {
                    try { return JSON.parse(qtobj.library_json || "[]") }
                    catch (e) { return [] }
                }
                property var artistList: {
                    try { return JSON.parse(qtobj.artists_json || "[]") }
                    catch (e) { return [] }
                }
                property var tagList: {
                    try { return JSON.parse(qtobj.tags_json || "[]") }
                    catch (e) { return [] }
                }
                // Max count among visible groups (used for the relative-size bar).
                readonly property int maxGroupCount: {
                    var list = showingArtists ? artistList : tagList
                    var m = 1
                    for (var i = 0; i < list.length; ++i)
                        if (list[i].count > m) m = list[i].count
                    return m
                }

                Component.onCompleted: {
                    qtobj.load_recent()
                    // Pre-load tags so the filter chip row populates
                    // the first time the user opens the library page.
                    // Cheap (single SELECT count) and harmless to repeat.
                    qtobj.load_all_tags()
                }

                Column {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gutter
                    anchors.rightMargin: Bo.gutter
                    anchors.topMargin: Bo.gapLG
                    spacing: Bo.gapSM

                    // Header: title + catalog count + scan control
                    Item {
                        width: parent.width
                        height: 40
                        Column {
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            Text {
                                text: "LIBRARY"
                                color: Bo.silver
                                font.pixelSize: Bo.microSize
                                font.letterSpacing: Bo.labelTracking
                                font.weight: Font.DemiBold
                            }
                            Text {
                                text: {
                                    var ctx = "" + (qtobj.library_context || "")
                                    if (ctx.indexOf("search:") === 0) {
                                        var n = (qtobj.library_json
                                            ? JSON.parse(qtobj.library_json).length : 0)
                                        return n + (n === 1 ? " match" : " matches")
                                            + " for \"" + ctx.substring(7) + "\""
                                    }
                                    if (libRoot.inDrillDown && libRoot.viewLabel)
                                        return "in " + libRoot.viewLabel
                                    if (qtobj.lib_total > 0)
                                        return qtobj.lib_total + " tracks"
                                            + (qtobj.lib_analyzed > 0
                                               ? "  ·  " + qtobj.lib_analyzed + " analyzed" : "")
                                    return "Empty — tap scan"
                                }
                                color: Bo.slate
                                font.pixelSize: Bo.captionSize
                            }
                        }
                        // Scan button
                        Rectangle {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            width: scanRow.implicitWidth + 28
                            height: 34
                            radius: 17
                            color: scanMouse.pressed ? Bo.surfaceHi : Bo.surface
                            border.color: Bo.hairline; border.width: 1
                            Row {
                                id: scanRow
                                anchors.centerIn: parent
                                spacing: 6
                                Text {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: "⟳"; color: window.accent; font.pixelSize: 13
                                }
                                Text {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: "Scan"
                                    color: Bo.bone; font.pixelSize: Bo.captionSize
                                    font.weight: Font.Medium
                                }
                            }
                            MouseArea {
                                id: scanMouse
                                anchors.fill: parent
                                onClicked: scanPicker.open()
                            }
                        }
                    }

                    // ── Search field ─────────────────────────────────────
                    // Live search: each keystroke fires library_search(),
                    // which the worker dispatches with a 200-row limit.
                    // Empty input reverts to recent.
                    Rectangle {
                        width: parent.width
                        height: 38
                        radius: 19
                        color: Bo.surface
                        border.color: searchInput.activeFocus ? window.accent : Bo.hairline
                        border.width: 1
                        Behavior on border.color { ColorAnimation { duration: Bo.quick } }

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: 14
                            anchors.rightMargin: 14
                            spacing: 8
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "⌕"; color: Bo.slate; font.pixelSize: 14
                            }
                            TextInput {
                                id: searchInput
                                anchors.verticalCenter: parent.verticalCenter
                                width: parent.width - 60
                                color: Bo.bone
                                font.pixelSize: Bo.bodySize
                                selectByMouse: true
                                clip: true
                                onTextChanged: {
                                    // Typing in search overrides the tab — kick
                                    // into the "search" view so the tracks list
                                    // shows even from the Artists/Tags tab. An
                                    // empty field returns to whatever tab the
                                    // user previously had (default: recent).
                                    if (text.length > 0) {
                                        libRoot.viewMode = "search"
                                        qtobj.library_search(text)
                                    } else {
                                        libRoot.goRecent()
                                    }
                                }
                                // Placeholder
                                Text {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: "Search title, artist, album…"
                                    color: Bo.slate
                                    font.pixelSize: parent.font.pixelSize
                                    visible: parent.text.length === 0 && !parent.activeFocus
                                }
                            }
                            // Clear button
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "✕"
                                color: Bo.slate
                                font.pixelSize: 12
                                visible: searchInput.text.length > 0
                                MouseArea {
                                    anchors.fill: parent
                                    anchors.margins: -6
                                    onClicked: searchInput.text = ""
                                }
                            }
                        }
                    }

                    // Tab bar: Recent · Artists · Tags. Hidden inside drill
                    // views (artist:N / tag:N) where a back button takes over.
                    Item {
                        width: parent.width
                        height: 38
                        Row {
                            visible: !libRoot.inDrillDown
                            anchors.fill: parent
                            spacing: 0
                            Repeater {
                                model: [
                                    {label: "RECENT",  mode: "recent",  fn: function(){ libRoot.goRecent()  }},
                                    {label: "ARTISTS", mode: "artists", fn: function(){ libRoot.goArtists() }},
                                    {label: "TAGS",    mode: "tags",    fn: function(){ libRoot.goTags()    }}
                                ]
                                delegate: Rectangle {
                                    width: parent.width / 3
                                    height: parent.height
                                    color: "transparent"
                                    property bool sel: libRoot.viewMode === modelData.mode
                                    Text {
                                        anchors.centerIn: parent
                                        text: modelData.label
                                        color: parent.sel ? Bo.bone
                                              : (tabMa.pressed ? Bo.silver : Bo.slate)
                                        font.pixelSize: Bo.microSize
                                        font.letterSpacing: Bo.labelTracking
                                        font.weight: parent.sel ? Font.DemiBold : Font.Medium
                                    }
                                    Rectangle {
                                        anchors.bottom: parent.bottom
                                        anchors.horizontalCenter: parent.horizontalCenter
                                        width: parent.sel ? parent.width * 0.5 : 0
                                        height: 2; radius: 1
                                        color: window.accent
                                        Behavior on width {
                                            NumberAnimation { duration: Bo.quick; easing.type: Easing.OutCubic }
                                        }
                                    }
                                    MouseArea {
                                        id: tabMa
                                        anchors.fill: parent
                                        onClicked: modelData.fn()
                                    }
                                }
                            }
                        }
                        // Back button shown while drilled in.
                        Row {
                            visible: libRoot.inDrillDown
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 8
                            Rectangle {
                                width: backRow.implicitWidth + 24; height: 28; radius: 14
                                color: backMa.pressed ? Bo.surfaceHi : Bo.surface
                                border.color: Bo.hairline; border.width: 1
                                Row {
                                    id: backRow
                                    anchors.centerIn: parent
                                    spacing: 6
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: "‹"; color: window.accent
                                        font.pixelSize: 14; font.weight: Font.Bold
                                    }
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: libRoot.viewMode.indexOf("artist:") === 0 ? "Artists" : "Tags"
                                        color: Bo.bone; font.pixelSize: Bo.captionSize
                                        font.weight: Font.Medium
                                    }
                                }
                                MouseArea {
                                    id: backMa
                                    anchors.fill: parent
                                    onClicked: libRoot.goBack()
                                }
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: libRoot.viewLabel
                                color: Bo.silver
                                font.pixelSize: Bo.captionSize
                                font.weight: Font.Medium
                            }
                        }
                    }

                    // Scan progress strip (only during a scan)
                    Rectangle {
                        width: parent.width
                        height: visible ? 4 : 0
                        radius: 2
                        color: Bo.surfaceHi
                        visible: qtobj.scan_total > 0 && qtobj.scan_phase !== "Done"
                        Rectangle {
                            height: parent.height
                            radius: 2
                            color: window.accent
                            width: qtobj.scan_total > 0
                                   ? parent.width * (qtobj.scan_done / qtobj.scan_total) : 0
                            Behavior on width { NumberAnimation { duration: Bo.quick } }
                        }
                    }

                    // ── TAG-FILTER CHIPS ───────────────────────────
                    //
                    // Horizontal-scrollable strip of every tag in the
                    // library. Tap to filter the Library list to just
                    // that tag's tracks (via load_tracks_by_tag). The
                    // currently-active chip is highlighted with its
                    // own color. A leading "All" chip clears the filter
                    // and returns to the recent-tracks view. Hidden
                    // when no tags exist yet (no clutter).
                    //
                    // Wraps in a Flickable rather than ScrollView so
                    // the mobile touch behavior matches the rest of
                    // the app (overshoot bounds, no scrollbar UI).
                    Flickable {
                        width: parent.width
                        height: visible ? 32 : 0
                        visible: libRoot.tagList.length > 0
                        contentWidth: tagChipsRow.implicitWidth
                        contentHeight: 28
                        boundsBehavior: Flickable.OvershootBounds
                        flickableDirection: Flickable.HorizontalFlick
                        clip: true

                        Row {
                            id: tagChipsRow
                            spacing: 6
                            anchors.verticalCenter: parent.verticalCenter

                            // "All" chip — clears the tag filter.
                            Rectangle {
                                height: 24; radius: 12
                                width: allText.implicitWidth + 20
                                property bool isActive:
                                    libRoot.viewMode.indexOf("tag:") !== 0
                                color: isActive ? Bo.mix(Bo.surface, window.accent, 0.20)
                                                : "transparent"
                                border.color: isActive ? window.accent : Bo.hairline
                                border.width: 1
                                Text {
                                    id: allText
                                    anchors.centerIn: parent
                                    text: "All"
                                    color: parent.isActive ? Bo.bone : Bo.silver
                                    font.pixelSize: 10
                                    font.weight: Font.DemiBold
                                    font.letterSpacing: 0.6
                                }
                                MouseArea {
                                    anchors.fill: parent
                                    onClicked: {
                                        // Return to recent-tracks view.
                                        libRoot.viewMode = "tracks"
                                        libRoot.viewLabel = ""
                                        qtobj.load_recent()
                                    }
                                }
                            }

                            // One chip per tag.
                            Repeater {
                                model: libRoot.tagList
                                delegate: Rectangle {
                                    property color tagColor:
                                        modelData.color || window.accent
                                    property string tagFilterKey: "tag:" + modelData.id
                                    property bool isActive: libRoot.viewMode === tagFilterKey
                                    height: 24; radius: 12
                                    width: chipLabel.implicitWidth + 26
                                    color: isActive
                                        ? Bo.mix(Bo.surface, tagColor, 0.30)
                                        : Bo.mix(Bo.surface, tagColor, 0.10)
                                    border.color: tagColor
                                    border.width: isActive ? 2 : 1
                                    Row {
                                        anchors.centerIn: parent
                                        spacing: 4
                                        Text {
                                            text: "#"
                                            color: parent.parent.tagColor
                                            font.pixelSize: 11
                                            font.weight: Font.Bold
                                            anchors.verticalCenter: parent.verticalCenter
                                        }
                                        Text {
                                            id: chipLabel
                                            text: modelData.name
                                            color: parent.parent.isActive ? Bo.bone : Bo.silver
                                            font.pixelSize: 10
                                            font.weight: Font.DemiBold
                                            anchors.verticalCenter: parent.verticalCenter
                                        }
                                    }
                                    MouseArea {
                                        anchors.fill: parent
                                        onClicked: {
                                            libRoot.viewMode = parent.tagFilterKey
                                            libRoot.viewLabel = "#" + modelData.name
                                            qtobj.load_tracks_by_tag(modelData.id)
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // The virtualized list — recycles delegates, so thousands
                    // of rows scroll smoothly and load instantly from SQLite.
                    // One Item holds all three list modes; the visible one is
                    // chosen by libRoot.viewMode.
                    Item {
                        width: parent.width
                        height: parent.height - 56

                        // Tracks view (recent / search / artist:N / tag:N)
                        ListView {
                            id: trackListView
                            anchors.fill: parent
                            visible: libRoot.showingTracks
                            clip: true
                            model: libRoot.trackList
                            cacheBuffer: 400
                            boundsBehavior: Flickable.OvershootBounds
                            flickDeceleration: 1800

                            delegate: TrackRow {
                                width: trackListView.width
                                title: modelData.title || ""
                                artist: modelData.primary_artist || ""
                                album: modelData.album || ""
                                bpm: modelData.bpm
                                camelot: modelData.camelot || ""
                                accent: window.accent
                                onActivated: {
                                    // play_track does the work end-to-end:
                                    //   - resolves the path via media worker
                                    //   - exposes it over the local HTTP server
                                    //   - POSTs to the device's ASE/BeoZone API
                                    //   - records the play in the catalog
                                    // load_key_bpm is independent — fills the
                                    // now-playing HUD chip with bpm/key/camelot.
                                    qtobj.play_track(modelData.id || 0)
                                    qtobj.load_key_bpm(modelData.id || 0)
                                }
                                onLongPressed: editDialog.open(modelData)
                                onTagRequested: {
                                    if (window.tagEditor) {
                                        window.tagEditor.open(modelData)
                                    }
                                }
                            }

                            // Empty state
                            Text {
                                anchors.centerIn: parent
                                visible: trackListView.count === 0
                                horizontalAlignment: Text.AlignHCenter
                                text: {
                                    if (qtobj.scan_total > 0 && qtobj.scan_phase !== "Done")
                                        return "Scanning…"
                                    var ctx = "" + (qtobj.library_context || "")
                                    if (ctx.indexOf("search:") === 0)
                                        return "No matches for \"" + ctx.substring(7) + "\""
                                    if (libRoot.viewMode.indexOf("artist:") === 0)
                                        return "No tracks for this artist yet."
                                    if (libRoot.viewMode.indexOf("tag:") === 0)
                                        return "No tracks for this tag yet."
                                    return "No tracks yet.\nTap Scan to choose a folder."
                                }
                                color: Bo.slate
                                font.pixelSize: Bo.captionSize
                            }

                            ScrollBar.vertical: ScrollBar { }
                        }

                        // Artists view
                        ListView {
                            id: artistListView
                            anchors.fill: parent
                            visible: libRoot.showingArtists
                            clip: true
                            model: libRoot.artistList
                            cacheBuffer: 400
                            boundsBehavior: Flickable.OvershootBounds
                            flickDeceleration: 1800

                            delegate: GroupRow {
                                width: artistListView.width
                                name: modelData.name || "—"
                                count: modelData.count || 0
                                maxCount: libRoot.maxGroupCount
                                kind: "artist"
                                accent: window.accent
                                onActivated: libRoot.openArtist(modelData.id || 0, modelData.name || "")
                            }
                            Text {
                                anchors.centerIn: parent
                                visible: artistListView.count === 0
                                horizontalAlignment: Text.AlignHCenter
                                text: qtobj.lib_total > 0 ? "No artists yet."
                                                          : "Scan to populate the library."
                                color: Bo.slate
                                font.pixelSize: Bo.captionSize
                            }
                            ScrollBar.vertical: ScrollBar { }
                        }

                        // Tags view
                        ListView {
                            id: tagListView
                            anchors.fill: parent
                            visible: libRoot.showingTags
                            clip: true
                            model: libRoot.tagList
                            cacheBuffer: 400
                            boundsBehavior: Flickable.OvershootBounds
                            flickDeceleration: 1800

                            delegate: GroupRow {
                                width: tagListView.width
                                name: modelData.name || "—"
                                count: modelData.count || 0
                                maxCount: libRoot.maxGroupCount
                                kind: "tag"
                                accent: window.accent
                                onActivated: libRoot.openTag(modelData.id || 0, modelData.name || "")
                            }
                            Text {
                                anchors.centerIn: parent
                                visible: tagListView.count === 0
                                horizontalAlignment: Text.AlignHCenter
                                text: qtobj.lib_total > 0 ? "No tags yet — tracks need (parenthesized)\nlabels in their filename."
                                                          : "Scan to populate the library."
                                color: Bo.slate
                                font.pixelSize: Bo.captionSize
                            }
                            ScrollBar.vertical: ScrollBar { }
                        }
                    }
                }

                // Long-press a track row to open this edit dialog. Floats
                // above the ListView at z:200.
                EditTrackDialog {
                    id: editDialog
                    accent: window.accent
                }

                // Asks where to scan when Scan is tapped (see scanMouse above).
                ScanPickerDialog {
                    id: scanPicker
                    accent: window.accent
                }
            }
        }

        // ░░ PAGE — PLAYLISTS ░░
        // Routes between two stacked views: the playlists list (default)
        // and the detail view for one playlist. The detail view is shown
        // when its currentPlaylistId is non-negative.
        Loader {
            active: window.hasControl("library")
            sourceComponent: Item {
                id: playlistsRoot
                property int currentPlaylistId: -1
                property string currentPlaylistName: ""

                PlaylistsPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: playlistsRoot.currentPlaylistId < 0
                    onOpenPlaylist: (id, name) => {
                        playlistsRoot.currentPlaylistId = id
                        playlistsRoot.currentPlaylistName = name
                    }
                }
                PlaylistDetailPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: playlistsRoot.currentPlaylistId >= 0
                    playlistId: playlistsRoot.currentPlaylistId
                    playlistName: playlistsRoot.currentPlaylistName
                    onBack: {
                        playlistsRoot.currentPlaylistId = -1
                        playlistsRoot.currentPlaylistName = ""
                    }
                }
            }
        }

        // ░░ PAGE — QUEUE ░░
        Loader {
            active: window.hasControl("library")
            sourceComponent: QueuePage {
                accent: window.accent
            }
        }

        // ░░ PAGE — ARTISTS ░░
        // Same back/forward routing pattern as playlists: list view,
        // optionally overlaid by a single-artist drill view.
        Loader {
            active: window.hasControl("library")
            sourceComponent: Item {
                id: artistsRoot
                property int currentArtistId: -1
                property string currentArtistName: ""

                ArtistsPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: artistsRoot.currentArtistId < 0
                    onOpenArtist: (id, name) => {
                        artistsRoot.currentArtistId = id
                        artistsRoot.currentArtistName = name
                    }
                }
                ArtistDetailPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: artistsRoot.currentArtistId >= 0
                    artistId: artistsRoot.currentArtistId
                    artistName: artistsRoot.currentArtistName
                    onBack: {
                        artistsRoot.currentArtistId = -1
                        artistsRoot.currentArtistName = ""
                    }
                }
            }
        }

        // ░░ PAGE — ALBUMS ░░
        // Browse-by-album view. List/detail pattern: AlbumsPage
        // shows top/alpha tabs + a Flow grid of album cards; tap an
        // album to drill into AlbumDetailPage which lists the
        // album's tracks and offers a Play All button.
        //
        // We keep the in-album state local (whether the user is
        // looking at the list or the detail) so navigating away
        // from this tab and back returns the user to the same view.
        Loader {
            active: window.hasControl("library")
            sourceComponent: Item {
                id: albumsRoot
                property bool inDetail: false

                AlbumsPage {
                    anchors.fill: parent
                    visible: !albumsRoot.inDetail
                    onAlbumTapped: (title, artist) => {
                        albumsRoot.inDetail = true
                    }
                }
                AlbumDetailPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: albumsRoot.inDetail
                    onBackTapped: { albumsRoot.inDetail = false }
                }
            }
        }

        // ░░ PAGE — ALL TRACKS ░░
        // Flat, title-ordered list of every track — the UI half of the
        // missing-tracks fix. Unlike Artists/Albums/Tags (which join
        // through track_artists and so hide files that parsed with no
        // artist), this lists FROM tracks directly, so nothing in the
        // library is ever unreachable. No detail overlay; it's a leaf.
        Loader {
            active: window.hasControl("library")
            sourceComponent: AllTracksPage {
                accent: window.accent
            }
        }

        // ░░ PAGE — TAGS ░░
        // Tag browser + drill-down. List → detail pattern matches
        // Artists and Playlists. Tag color is propagated to the detail
        // page so the accent button is tag-specific.
        Loader {
            active: window.hasControl("library")
            sourceComponent: Item {
                id: tagsRoot
                property int currentTagId: -1
                property string currentTagName: ""
                property color currentTagColor: window.accent

                TagsPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: tagsRoot.currentTagId < 0
                    onOpenTag: (id, name, col) => {
                        tagsRoot.currentTagId = id
                        tagsRoot.currentTagName = name
                        tagsRoot.currentTagColor = col
                    }
                }
                TagDetailPage {
                    anchors.fill: parent
                    accent: window.accent
                    visible: tagsRoot.currentTagId >= 0
                    tagId: tagsRoot.currentTagId
                    tagName: tagsRoot.currentTagName
                    tagColor: tagsRoot.currentTagColor
                    onBack: {
                        tagsRoot.currentTagId = -1
                        tagsRoot.currentTagName = ""
                    }
                }
            }
        }

        // ░░ PAGE — SEARCH ░░
        Loader {
            active: window.hasControl("library")
            sourceComponent: SearchPage {
                accent: window.accent
            }
        }

        // ░░ PAGE — HISTORY ░░
        Loader {
            active: window.hasControl("library")
            sourceComponent: HistoryPage {
                accent: window.accent
            }
        }

        // ░░ PAGE — SPACES (soundscapes / multi-speaker) ░░
        Loader {
            active: window.hasControl("spaces")
            sourceComponent: SpacesPage { }
        }

        // ░░ PAGE — SOUNDSCAPE CREATOR ░░
        // Always active — the loop-finder doesn't depend on any
        // device control, so users can create soundscapes even when
        // no Beosound device is reachable.
        Loader {
            active: true
            sourceComponent: SoundscapePage {
                accent: window.accent
            }
        }

        // ░░ PAGE — STREAM MIXER ░░
        // Mid/Side EQ + per-channel volume. Independent of any
        // specific device since it operates on the PCM stream the
        // rest of the pipeline will eventually feed in.
        Loader {
            active: true
            sourceComponent: MixerPage { }
        }

        // ░░ PAGE 2 — STREAM ░░
        Loader {
            active: window.pageStream
            sourceComponent: Item {
                Column {
                    anchors.fill: parent
                    anchors.margins: Bo.gutter
                    anchors.topMargin: Bo.gapLG
                    spacing: Bo.gapMD

                    ControlCard {
                        width: parent.width
                        accent: window.accent
                        active: qtobj.passthrough
                        label: "STREAM SOURCE"
                        entranceDelay: 0

                        Column {
                            width: parent.width
                            spacing: Bo.gapMD

                            Rectangle {
                                width: parent.width; height: 50; radius: 12
                                color: Bo.ink
                                border.color: uriInput.activeFocus ? window.accent : Bo.hairline
                                border.width: 1
                                Behavior on border.color { ColorAnimation { duration: Bo.quick } }
                                TextInput {
                                    id: uriInput
                                    anchors.fill: parent
                                    anchors.leftMargin: 14; anchors.rightMargin: 14
                                    verticalAlignment: TextInput.AlignVCenter
                                    clip: true; color: Bo.bone
                                    font.pixelSize: Bo.bodySize
                                    selectByMouse: true
                                    text: qtobj.stream_uri
                                    Text {
                                        anchors.fill: parent; verticalAlignment: Text.AlignVCenter
                                        visible: uriInput.text.length === 0
                                        text: "stream URL"
                                        color: Bo.slate; font.pixelSize: Bo.bodySize
                                    }
                                }
                            }

                            // Play / stop — morphs between states
                            Rectangle {
                                width: parent.width; height: 56; radius: 14
                                color: qtobj.passthrough ? window.accent
                                       : (playArea.pressed ? Bo.surfaceHi : Bo.ink)
                                border.color: qtobj.passthrough ? window.accent : Bo.hairline
                                border.width: 1
                                Behavior on color { ColorAnimation { duration: Bo.base } }

                                Row {
                                    anchors.centerIn: parent
                                    spacing: 10
                                    // play/stop glyph crossfades
                                    Item {
                                        width: 16; height: 16
                                        anchors.verticalCenter: parent.verticalCenter
                                        Text {
                                            anchors.centerIn: parent
                                            text: "▶"; font.pixelSize: 14
                                            color: window.accent
                                            opacity: qtobj.passthrough ? 0 : 1
                                            Behavior on opacity { NumberAnimation { duration: Bo.quick } }
                                        }
                                        Text {
                                            anchors.centerIn: parent
                                            text: "■"; font.pixelSize: 14
                                            color: Bo.graphite
                                            opacity: qtobj.passthrough ? 1 : 0
                                            Behavior on opacity { NumberAnimation { duration: Bo.quick } }
                                        }
                                    }
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: qtobj.passthrough ? "Streaming" : "Start streaming"
                                        color: qtobj.passthrough ? Bo.graphite : Bo.bone
                                        font.pixelSize: Bo.bodySize
                                        font.weight: Font.Medium
                                        font.letterSpacing: 0.5
                                    }
                                }
                                MouseArea {
                                    id: playArea
                                    anchors.fill: parent
                                    onClicked: {
                                        if (!qtobj.passthrough) {
                                            qtobj.start_stream(uriInput.text)
                                        } else {
                                            qtobj.stop_stream()
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Live "now streaming" strip — animated equalizer bars
                    ControlCard {
                        width: parent.width
                        accent: window.accent
                        active: qtobj.passthrough
                        label: "STATUS"
                        entranceDelay: Bo.quick
                        visible: qtobj.passthrough

                        Row {
                            width: parent.width
                            height: 36
                            spacing: 16
                            Row {
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 3
                                Repeater {
                                    model: 5
                                    delegate: Rectangle {
                                        width: 3; radius: 1.5
                                        color: window.accent
                                        anchors.verticalCenter: parent.verticalCenter
                                        height: 8
                                        SequentialAnimation on height {
                                            running: qtobj.passthrough
                                            loops: Animation.Infinite
                                            NumberAnimation { to: 6 + (index % 3) * 9 + 14
                                                duration: 360 + index * 70; easing.type: Easing.InOutSine }
                                            NumberAnimation { to: 6
                                                duration: 300 + index * 60; easing.type: Easing.InOutSine }
                                        }
                                    }
                                }
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "Playing to " + (qtobj.product_name || "device")
                                color: Bo.silver
                                font.pixelSize: Bo.captionSize
                            }
                        }
                    }
                }
            }
        }

        // ░░ PAGE — SOURCES ░░
        Loader {
            active: window.hasControl("sources")
            sourceComponent: Item {
                property var sourceList: {
                    try { return JSON.parse(qtobj.sources_json || "[]") } catch(e) { return [] }
                }
                Column {
                    anchors.fill: parent
                    anchors.margins: Bo.gutter
                    anchors.topMargin: Bo.gapLG
                    spacing: Bo.gapMD

                    Row {
                        width: parent.width
                        Text {
                            text: "SOURCES"
                            color: Bo.silver
                            font.pixelSize: Bo.microSize
                            font.letterSpacing: Bo.labelTracking
                            font.weight: Font.DemiBold
                        }
                    }

                    Flow {
                        width: parent.width
                        spacing: Bo.gapSM
                        Repeater {
                            model: parent.parent.sourceList
                            delegate: SourceTile {
                                sourceName: modelData.name || modelData.id
                                sourceType: modelData.type || ""
                                active: qtobj.active_source_id === modelData.id
                                accent: window.accent
                                onChosen: qtobj.select_source(modelData.id)
                            }
                        }
                    }

                    Text {
                        visible: parent.parent.sourceList.length === 0
                        text: "No sources reported.\nTap refresh, or the device may expose none over the API."
                        color: Bo.slate
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                        width: parent.width
                    }
                }
            }
        }

        // ░░ PAGE 3 — CONNECT ░░
        Item {
            Column {
                anchors.fill: parent
                anchors.margins: Bo.gutter
                anchors.topMargin: Bo.gapLG
                spacing: Bo.gapMD

                ControlCard {
                    visible: window.hasControl("autoroute")
                    width: parent.width
                    accent: window.accent
                    active: qtobj.auto_route_kde
                    label: "AUTO-ROUTE"
                    entranceDelay: 0

                    Item {
                        width: parent.width; height: 46
                        Column {
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 3
                            Text {
                                text: "Route KDE media here"
                                color: Bo.bone; font.pixelSize: Bo.bodySize
                            }
                            Row {
                                spacing: 6
                                Rectangle {
                                    width: 6; height: 6; radius: 3
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: qtobj.kde_media_active ? window.accent : Bo.slate
                                }
                                Text {
                                    text: qtobj.kde_media_active ? "Audio detected" : "Idle"
                                    color: qtobj.kde_media_active ? window.accent : Bo.slate
                                    font.pixelSize: Bo.captionSize
                                    font.letterSpacing: 0.5
                                }
                            }
                        }
                        // Toggle
                        Rectangle {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            width: 54; height: 32; radius: 16
                            color: qtobj.auto_route_kde ? window.accent : Bo.surfaceHi
                            border.color: qtobj.auto_route_kde ? window.accent : Bo.hairline
                            border.width: 1
                            Behavior on color { ColorAnimation { duration: Bo.base } }
                            Rectangle {
                                width: 24; height: 24; radius: 12
                                color: Bo.graphite
                                anchors.verticalCenter: parent.verticalCenter
                                x: qtobj.auto_route_kde ? parent.width - width - 4 : 4
                                Behavior on x { NumberAnimation { duration: Bo.base
                                    easing.type: Easing.Bezier; easing.bezierCurve: Bo.spring } }
                            }
                            MouseArea { anchors.fill: parent; onClicked: qtobj.toggle_auto_route() }
                        }
                    }
                }

                ControlCard {
                    visible: window.hasControl("power")
                    width: parent.width
                    accent: window.accent
                    active: !qtobj.standby
                    label: "POWER"
                    entranceDelay: Bo.instant

                    Item {
                        width: parent.width; height: 46
                        Column {
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 3
                            Text {
                                text: qtobj.standby ? "In standby" : "On"
                                color: Bo.bone; font.pixelSize: Bo.bodySize
                            }
                            Text {
                                text: qtobj.standby ? "Tap to wake" : "Tap to send to standby"
                                color: Bo.slate; font.pixelSize: Bo.captionSize
                            }
                        }
                        Rectangle {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            width: 54; height: 32; radius: 16
                            color: !qtobj.standby ? window.accent : Bo.surfaceHi
                            border.color: !qtobj.standby ? window.accent : Bo.hairline
                            border.width: 1
                            Behavior on color { ColorAnimation { duration: Bo.base } }
                            Rectangle {
                                width: 24; height: 24; radius: 12
                                color: Bo.graphite
                                anchors.verticalCenter: parent.verticalCenter
                                x: !qtobj.standby ? parent.width - width - 4 : 4
                                Behavior on x { NumberAnimation { duration: Bo.base
                                    easing.type: Easing.Bezier; easing.bezierCurve: Bo.spring } }
                            }
                            MouseArea { anchors.fill: parent; onClicked: qtobj.toggle_standby() }
                        }
                    }
                }

                ControlCard {
                    visible: window.hasControl("soundmode") && soundModeList.length > 0
                    width: parent.width
                    accent: window.accent
                    label: "SOUND MODE"
                    entranceDelay: Bo.quick

                    property var soundModeList: {
                        try { return JSON.parse(qtobj.sound_modes_json || "[]") }
                        catch (e) { return [] }
                    }

                    Flow {
                        width: parent.width
                        spacing: Bo.gapXS
                        Repeater {
                            model: parent.parent.soundModeList
                            delegate: Rectangle {
                                property bool sel: (modelData.id || "") === qtobj.active_sound_mode
                                height: 34
                                width: modeLabel.implicitWidth + 24
                                radius: 17
                                color: sel ? window.accent : Bo.surfaceHi
                                border.color: sel ? window.accent : Bo.hairline
                                border.width: 1
                                Behavior on color { ColorAnimation { duration: Bo.quick } }
                                scale: smMouse.pressed ? 0.95 : 1.0
                                Behavior on scale { NumberAnimation { duration: Bo.instant } }
                                Text {
                                    id: modeLabel
                                    anchors.centerIn: parent
                                    text: modelData.name || modelData.id
                                    color: sel ? Bo.graphite : Bo.silver
                                    font.pixelSize: Bo.captionSize
                                    font.weight: sel ? Font.DemiBold : Font.Normal
                                }
                                MouseArea {
                                    id: smMouse
                                    anchors.fill: parent
                                    onClicked: qtobj.select_sound_mode(modelData.id || "")
                                }
                            }
                        }
                    }
                }

                ControlCard {
                    width: parent.width
                    accent: window.accent
                    label: "DEVICE"
                    entranceDelay: Bo.quick

                    Column {
                        width: parent.width
                        spacing: Bo.gapSM
                        // Model: prefer the explicit product_name from
                        // REST product-info; fall back to the mDNS /
                        // user-added discovery name (devices_json
                        // entry), so something useful shows even when
                        // the device rejects the product-info probe.
                        InfoRow {
                            k: "Model"
                            v: qtobj.product_name && qtobj.product_name.length > 0
                                ? qtobj.product_name
                                : (qtobj.active_device_name() || "—")
                        }
                        // Type: prefer the explicit product_type; fall
                        // back to the firmware family label which is
                        // always populated after the firmware probe
                        // (e.g. "Mozart (legacy)").
                        InfoRow {
                            k: "Type"
                            v: qtobj.product_type && qtobj.product_type.length > 0
                                ? qtobj.product_type
                                : (qtobj.device_firmware_label
                                    && qtobj.device_firmware_label.length > 0
                                    ? qtobj.device_firmware_label : "—")
                        }
                        InfoRow {
                            k: "Address"
                            v: qtobj.active_url && qtobj.active_url.length > 0
                                ? qtobj.active_url : "—"
                        }
                    }
                }
            }
        }

        // ░░ PAGE — SETTINGS (system tools audit, Bluetooth management) ░░
        Loader {
            active: true   // always shown — global, not per-product
            sourceComponent: SettingsPage {
                accent: window.accent
            }
        }
    }

    // ── Persistent bottom transport ─────────────────────────────────
    //
    // Sits just above the page-dots indicator and is visible across
    // every page. Surfaces a compact PLAYING TO chip + stop button
    // + (Mozart only) play/pause/skip controls. This is the panic-
    // stop the user needed when audio gets routed somewhere they
    // didn't expect (BT disconnect, sink moved, multi-room
    // unexpectedly etc.).
    BottomTransportBar {
        id: bottomBar
        accent: window.accent
        anchors.bottom: dots.top
        anchors.bottomMargin: Bo.gapSM
        anchors.left: parent.left
        anchors.right: parent.right
    }

    // ── Page indicator ──────────────────────────────────────────────
    PageDots {
        id: dots
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Bo.gapXL
        anchors.horizontalCenter: parent.horizontalCenter
        count: pager.count
        index: pager.currentIndex
        accent: window.accent
    }

    // Safety toast — slides down when the volume system clamps a manual
    // change above the unsafe threshold. Lives at z:500 so it overlays
    // all pages including dialogs.
    SafetyToast {
        accent: window.accent
    }

    // Global TagEditor dialog. Any page can call:
    //   window.tagEditor.open({id: 123, title: "..."})
    // to surface the tag manager for that track. Lives at z:300 so
    // it overlays the SwipeView pages but sits under SafetyToast (z:500).
    TagEditor {
        id: tagEditorGlobal
        accent: window.accent
        z: 300
    }
    // Expose to other QML files via window.tagEditor.
    property alias tagEditor: tagEditorGlobal

    // Bluetooth sink picker — modal dialog letting the user manually
    // pick which paired BT speaker is the audio output for the
    // currently active device. Used when auto-pair-by-name didn't
    // find a match (e.g. Stage named "Beosound Stage" but paired BT
    // speakers are named "Emerge01" / "Smokey").
    BtSinkPicker {
        id: btSinkPicker
        z: 350
    }
    property alias btSinkPicker: btSinkPicker

    // Failover toast — slides down from top when the active device went
    // unreachable and we switched to the next-best hub. Self-dismisses
    // after 6 seconds. Sibling to SafetyToast at z:495 so a simultaneous
    // safety event still takes precedence.
    Item {
        id: failoverToast
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.topMargin: 60   // sits just below SafetyToast if both up
        height: failoverCard.implicitHeight + 16
        visible: opacity > 0.01
        opacity: qtobj.failover_active ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Bo.base; easing.type: Easing.OutCubic } }
        z: 495

        Rectangle {
            id: failoverCard
            anchors.fill: parent
            anchors.margins: Bo.gapMD
            radius: 12
            color: Bo.surface
            border.color: window.accent
            border.width: 1
            Row {
                anchors.fill: parent
                anchors.margins: Bo.gapMD
                spacing: 10
                Rectangle {
                    width: 8; height: 8; radius: 4
                    anchors.verticalCenter: parent.verticalCenter
                    color: window.accent
                }
                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2
                    Text {
                        text: "HUB FAILOVER"
                        color: Bo.bone
                        font.pixelSize: 9
                        font.letterSpacing: 1.2
                        font.weight: Font.DemiBold
                    }
                    Text {
                        text: qtobj.failover_from + " unreachable — switched to "
                              + qtobj.failover_to
                        color: Bo.silver
                        font.pixelSize: 11
                    }
                }
            }
            MouseArea {
                anchors.fill: parent
                onClicked: qtobj.dismiss_failover()
            }
        }
        Timer {
            interval: 6000
            running: qtobj.failover_active
            onTriggered: qtobj.dismiss_failover()
        }
    }

    // Anticipator tick — fires every 30 seconds so the FSM ages out
    // volatile states (Tuning → Idle, Paused → Idle after 5 min). Also
    // drives the "is anyone still here?" reasoning for pre-staging.
    Timer {
        interval: 30000
        running: true
        repeat: true
        onTriggered: qtobj.tick_anticipator()
    }
}
