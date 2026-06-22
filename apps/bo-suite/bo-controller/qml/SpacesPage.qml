// qml/SpacesPage.qml
//
// The "Spaces" page: drive the bo-mux daemon to play ambient soundscapes
// across multiple speakers. Talks to the headless daemon via the `mux`
// bridge object (a Rust QObject wrapping bo_mux::client::Client over the
// Unix socket).
//
// Degrades gracefully:
//   • daemon not installed → shows an "Install soundscape engine" card that
//     calls bridge.install() (deploy.rs install_or_upgrade).
//   • daemon installed but older → "Update available" with bridge.update().
//   • daemon running → scene picker + peers + transport.
//
// The bridge (`mux`) is expected to expose:
//   properties: daemon_up (bool), daemon_status (string=installed version or ""),
//               shipping_version (string), scenes_json, peers_json,
//               playing (bool), current_scene (string), role (string)
//   methods:    refresh(), install(), update(), load_scene(name),
//               play(), stop(), start_session(layout), leave_session()

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root

    // Defensive bridge over the `mux` context object. When the daemon
    // bridge isn't bound yet (Rust QObject not registered, qmetaobject
    // quirk, or the binary was built before MuxBridge existed), `mux`
    // can be null in QML. Rather than scatter null checks across the
    // page, wrap once and provide safe defaults so the UI still renders
    // and the install card invites the user to set things up.
    readonly property bool bridgeAvailable: (typeof mux !== "undefined") && mux !== null
    readonly property var bridge: bridgeAvailable ? mux : ({
        daemon_up: false,
        daemon_status: "",
        shipping_version: "",
        scenes_json: "[]",
        peers_json: "[]",
        fabric_json: "{}",
        layers_json: "[]",
        playing: false,
        current_scene: "",
        role: "idle",
        passive: false,
        passive_reason: "",
        on_battery: false,
        battery_pct: -1,
        setup_log: "",
        setup_running: false,
        setup_ok: false,
        doctor_json: "{}",
        refresh: function(){},
        install: function(){},
        update: function(){},
        load_scene: function(){},
        play: function(){},
        stop: function(){},
        start_session: function(){},
        leave_session: function(){},
        pin_layer: function(){},
        set_scene_gain: function(){},
        run_doctor: function(){},
        run_install: function(){},
        run_uninstall: function(){},
        fix_audio: function(){},
        fix_audio_full: function(){},
        poll_setup: function(){}
    })

    property var sceneList: { try { return JSON.parse(bridge.scenes_json || "[]") } catch(e){ return [] } }
    property var peerList:  { try { return JSON.parse(bridge.peers_json  || "[]") } catch(e){ return [] } }
    property var fabric:    { try { return JSON.parse(bridge.fabric_json || "{}") } catch(e){ return {} } }
    property var layerList: { try { return JSON.parse(bridge.layers_json || "[]") } catch(e){ return [] } }
    // Flattened list of every speaker across all peers, each tagged
    // with the owning node — the drop targets for drag-to-speaker.
    property var speakerList: {
        var out = []
        var peers = (fabric && fabric.peers) ? fabric.peers : []
        for (var i = 0; i < peers.length; i++) {
            var p = peers[i]
            var spk = p.speakers || []
            for (var j = 0; j < spk.length; j++) {
                out.push({
                    node_id: p.node_id,
                    node_name: p.name || p.node_id,
                    id: spk[j].id,
                    name: spk[j].name || spk[j].id,
                    platform: spk[j].platform || ""
                })
            }
        }
        return out
    }
    // channel → speakerId map from the current assignment, so each
    // speaker tile can show which layers/channels it's currently driving.
    property var assignmentMap: {
        var m = ({})
        var a = (fabric && fabric.assignment) ? fabric.assignment : []
        for (var i = 0; i < a.length; i++) { m[a[i].channel] = a[i].speaker }
        return m
    }

    Component.onCompleted: bridge.refresh()
    // Poll the daemon a couple times a second while this page is visible.
    Timer {
        interval: 1500; running: root.visible && root.bridgeAvailable; repeat: true
        onTriggered: bridge.refresh()
    }

    Flickable {
        // Stronger scroll: glide further, fling faster.
        flickDeceleration: Bo.flickDecel
        maximumFlickVelocity: Bo.flickMaxVel
        anchors.fill: parent
        anchors.leftMargin: Bo.gutter
        anchors.rightMargin: Bo.gutter
        anchors.topMargin: Bo.gapLG
        // Reserve persistent bottom chrome so the last card isn't
        // clipped by the BottomTransportBar on small screens.
        contentHeight: col.height + Bo.bottomChromeHeight
        clip: true

        Column {
            id: col
            width: parent.width
            spacing: Bo.gapMD

            // ── Title ───────────────────────────────────────────────
            Text {
                text: "SPACES"
                color: Bo.silver
                font.pixelSize: Bo.microSize
                font.letterSpacing: Bo.labelTracking
                font.weight: Font.DemiBold
            }

            // ── Daemon not installed ────────────────────────────────
            ControlCard {
                visible: !bridge.daemon_up && bridge.daemon_status === ""
                width: parent.width
                accent: root.accentOr()
                label: "ENGINE"
                Column {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        width: parent.width
                        text: "The soundscape engine isn't installed yet. It runs as a "
                            + "background service so playback continues with the app closed."
                        color: Bo.silver
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }
                    InstallButton {
                        label: "Install soundscape engine"
                        accent: root.accentOr()
                        onTapped: bridge.install()
                    }
                }
            }

            // ── Update available ────────────────────────────────────
            ControlCard {
                visible: bridge.daemon_status !== "" && bridge.daemon_status !== bridge.shipping_version
                width: parent.width
                accent: root.accentOr()
                label: "UPDATE"
                Column {
                    width: parent.width
                    spacing: Bo.gapSM
                    Text {
                        width: parent.width
                        text: "Engine " + bridge.daemon_status + " installed · "
                            + bridge.shipping_version + " available."
                        color: Bo.silver
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }
                    InstallButton {
                        label: "Update engine"
                        accent: root.accentOr()
                        onTapped: bridge.update()
                    }
                }
            }

            // ── Daemon down but installed (offer start/restart) ─────
            ControlCard {
                visible: !bridge.daemon_up && bridge.daemon_status !== ""
                width: parent.width
                accent: root.accentOr()
                label: "ENGINE OFFLINE"
                Row {
                    spacing: Bo.gapSM
                    Rectangle {
                        width: 9; height: 9; radius: 4.5
                        anchors.verticalCenter: parent.verticalCenter
                        color: Bo.slate
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "Socket-activated — it'll start on first play."
                        color: Bo.silver; font.pixelSize: Bo.captionSize
                    }
                }
            }

            // ── Scene picker (daemon up, OR installed & socket-
            //    activatable). Previously gated on daemon_up only, which
            //    hid every scene button when the daemon was installed but
            //    asleep — a catch-22, since tapping a scene is what
            //    socket-activates it ("it'll start on first play"). Now the
            //    Forest/River quick-play buttons and the scene list are
            //    reachable whenever the engine is installed, so first play
            //    can actually happen. ───────────────────────────────────
            ControlCard {
                visible: bridge.daemon_up || bridge.daemon_status !== ""
                width: parent.width
                accent: root.accentOr()
                label: "SOUNDSCAPE"
                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    // ── Passive-mode badge ──────────────────────
                    // Shown when bo-muxd has stopped spawning new
                    // clips because:
                    //   • the controller went silent (>60 s)
                    //   • the default audio sink was changed via
                    //     KDE picker / pavucontrol
                    //   • the configured Bluetooth sink disappeared
                    //   • the user has been idle for >10 minutes
                    //
                    // Any user action (tapping a scene, hitting play)
                    // will wake the daemon — but until then, this
                    // badge tells the user "we noticed you walked
                    // away" rather than letting them wonder why
                    // nothing's coming out of the speaker.
                    Rectangle {
                        visible: bridge.passive
                        width: parent.width
                        height: passiveCol.implicitHeight + 14
                        radius: 6
                        color: "#26221a"
                        border.color: "#6a5a30"
                        border.width: 1
                        Column {
                            id: passiveCol
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: 7
                            spacing: 3
                            Row {
                                spacing: 6
                                Text {
                                    text: "⏸"
                                    color: "#c0a060"
                                    font.pixelSize: 13
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                Text {
                                    text: "Soundscape paused"
                                    color: "#c0a060"
                                    font.pixelSize: 11
                                    font.bold: true
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                // Spacer
                                Item {
                                    width: passiveCol.width
                                        - resumeBoBtn.width - 130
                                    height: 1
                                }
                                Rectangle {
                                    id: resumeBoBtn
                                    width: resumeBoText.implicitWidth + 14
                                    height: 18
                                    radius: 9
                                    color: resumeBoMouse.containsMouse || resumeBoBtn.activeFocus
                                        ? "#3a3424" : "transparent"
                                    border.color: resumeBoBtn.activeFocus
                                        ? "#c0a060" : "#8a7544"
                                    border.width: resumeBoBtn.activeFocus ? 2 : 1
                                    anchors.verticalCenter: parent.verticalCenter
                                    // A11y: make the button reachable
                                    // by Tab. Visible focus ring above
                                    // (border color + width change on
                                    // activeFocus). Enter / Space fire
                                    // the same action as a tap.
                                    activeFocusOnTab: true
                                    Accessible.role: Accessible.Button
                                    Accessible.name: "Resume soundscape"
                                    Accessible.description:
                                        "Wake bo-mux from passive mode and resume \
                                         playing the current scene"
                                    Keys.onPressed: function(event) {
                                        if (event.key === Qt.Key_Return
                                            || event.key === Qt.Key_Enter
                                            || event.key === Qt.Key_Space) {
                                            bridge.play()
                                            event.accepted = true
                                        }
                                    }
                                    Text {
                                        id: resumeBoText
                                        anchors.centerIn: parent
                                        text: "Resume"
                                        color: "#c0a060"
                                        font.pixelSize: 9
                                        font.bold: true
                                    }
                                    MouseArea {
                                        id: resumeBoMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        // Any command wakes the daemon. We
                                        // send `play` — which restarts the
                                        // current scene if one's loaded.
                                        // If no scene is loaded, this is
                                        // a no-op on the audio side but
                                        // still wakes the daemon.
                                        onClicked: {
                                            resumeBoBtn.forceActiveFocus()
                                            bridge.play()
                                        }
                                    }
                                }
                            }
                            Text {
                                width: passiveCol.width
                                visible: bridge.passive_reason.length > 0
                                text: bridge.passive_reason
                                color: Bo.silver
                                font.pixelSize: 10
                                wrapMode: Text.Wrap
                            }
                        }
                    }

                    // ── QUICK-LAUNCH PRESETS ─────────────────────
                    //
                    // Two big tappable buttons for the two most
                    // commonly-wanted scenes (Forest and River).
                    // Tapping loads the scene + starts playback in
                    // one action — no need to scroll to find it in
                    // the long list below. Both presets are
                    // stereo-layout so they work on a single device
                    // or a single BT speaker; the daemon's
                    // RouteStrategy::Auto handles upmix to multi-
                    // speaker layouts automatically when more
                    // speakers are present.
                    //
                    // First-run: the scenes are bootstrapped from
                    // the daemon's embedded presets if missing
                    // (see bo-muxd::bootstrap_default_scenes), so
                    // these buttons always work even before the
                    // user has authored or imported any custom
                    // scenes.
                    Row {
                        width: parent.width
                        spacing: Bo.gapSM
                        Rectangle {
                            width: (parent.width - Bo.gapSM) / 2
                            height: 64
                            radius: 12
                            color: forestMa.pressed
                                ? Bo.mix(Bo.surface, "#4F7F4F", 0.55)
                                : Bo.mix(Bo.surface, "#4F7F4F", 0.22)
                            border.color: "#4F7F4F"; border.width: 1
                            Column {
                                anchors.centerIn: parent
                                spacing: 2
                                Text {
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    text: "🌲 Forest"
                                    color: Bo.bone
                                    font.pixelSize: 14
                                    font.weight: Font.DemiBold
                                }
                                Text {
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    text: "wind + leaves, stereo"
                                    color: Bo.silver
                                    font.pixelSize: 9
                                }
                            }
                            MouseArea {
                                id: forestMa
                                anchors.fill: parent
                                onClicked: {
                                    bridge.load_scene("forest_quiet")
                                    bridge.play()
                                }
                            }
                        }
                        Rectangle {
                            width: (parent.width - Bo.gapSM) / 2
                            height: 64
                            radius: 12
                            color: riverMa.pressed
                                ? Bo.mix(Bo.surface, "#4F6F8F", 0.55)
                                : Bo.mix(Bo.surface, "#4F6F8F", 0.22)
                            border.color: "#4F6F8F"; border.width: 1
                            Column {
                                anchors.centerIn: parent
                                spacing: 2
                                Text {
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    text: "🌊 River"
                                    color: Bo.bone
                                    font.pixelSize: 14
                                    font.weight: Font.DemiBold
                                }
                                Text {
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    text: "brook + bed, stereo"
                                    color: Bo.silver
                                    font.pixelSize: 9
                                }
                            }
                            MouseArea {
                                id: riverMa
                                anchors.fill: parent
                                onClicked: {
                                    bridge.load_scene("river_calm")
                                    bridge.play()
                                }
                            }
                        }
                    }

                    Repeater {
                        model: root.sceneList
                        delegate: Rectangle {
                            id: sceneRow
                            width: parent.width
                            height: hasDesc ? 64 : 52
                            radius: 10
                            // Loading needs the filename stem (id). Older
                            // daemons returned bare strings (no id field);
                            // fall back through name → the string itself.
                            property string loadKey:
                                (modelData.id !== undefined ? modelData.id
                                 : (modelData.name !== undefined ? modelData.name
                                    : modelData))
                            property string dispName:
                                (modelData.name !== undefined ? modelData.name
                                 : modelData)
                            property string dispDesc:
                                (modelData.description !== undefined
                                 ? modelData.description : "")
                            property bool hasDesc: dispDesc.length > 0
                            property string dispLayout:
                                (modelData.layout !== undefined ? modelData.layout : "")
                            property int nLayers:
                                (modelData.layers !== undefined ? modelData.layers : 0)
                            property bool sel:
                                dispName === bridge.current_scene
                                || loadKey === bridge.current_scene
                            activeFocusOnTab: true
                            Accessible.role: Accessible.Button
                            Accessible.name: "Load " + dispName
                            Accessible.description:
                                sel ? "Currently playing"
                                    : (hasDesc ? dispDesc
                                       : "Tap to switch to this soundscape")
                            Keys.onPressed: function(event) {
                                if (event.key === Qt.Key_Return
                                    || event.key === Qt.Key_Enter
                                    || event.key === Qt.Key_Space) {
                                    bridge.load_scene(sceneRow.loadKey)
                                    event.accepted = true
                                }
                            }
                            color: sel ? Bo.mix(Bo.surface, root.accentOr(), 0.14)
                                       : (sma.pressed ? Bo.surfaceHi : "transparent")
                            border.color: sceneRow.activeFocus
                                ? root.accentOr() : "transparent"
                            border.width: sceneRow.activeFocus ? 2 : 0
                            Behavior on color { ColorAnimation { duration: Bo.quick } }

                            Row {
                                anchors.left: parent.left
                                anchors.leftMargin: Bo.gapSM
                                anchors.right: parent.right
                                anchors.rightMargin: Bo.gapSM
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: Bo.gapSM
                                Text {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: "≋"; color: root.accentOr(); font.pixelSize: 16
                                }
                                Column {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: parent.width - 28 - layoutBadge.width - Bo.gapSM
                                    spacing: 2
                                    Text {
                                        width: parent.width
                                        text: sceneRow.dispName
                                        color: sceneRow.sel ? Bo.bone : Bo.silver
                                        font.pixelSize: Bo.bodySize
                                        font.weight: sceneRow.sel ? Font.DemiBold : Font.Normal
                                        elide: Text.ElideRight
                                    }
                                    Text {
                                        width: parent.width
                                        visible: sceneRow.hasDesc
                                        text: sceneRow.dispDesc
                                        color: Bo.slate
                                        font.pixelSize: Bo.captionSize
                                        elide: Text.ElideRight
                                    }
                                }
                                // Layout + layer-count badge (e.g. "5.1 · 4L")
                                Rectangle {
                                    id: layoutBadge
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: sceneRow.dispLayout.length > 0
                                    width: badgeTxt.implicitWidth + 14
                                    height: 20
                                    radius: 10
                                    color: Bo.mix(Bo.surface, root.accentOr(), 0.18)
                                    Text {
                                        id: badgeTxt
                                        anchors.centerIn: parent
                                        text: {
                                            var lay = sceneRow.dispLayout
                                            // Pretty-print the layout enum.
                                            var pretty = lay === "FivePointOne" ? "5.1"
                                                : lay === "Quad" ? "Quad"
                                                : lay === "Stereo" ? "Stereo"
                                                : lay
                                            return sceneRow.nLayers > 0
                                                ? pretty + " · " + sceneRow.nLayers + "L"
                                                : pretty
                                        }
                                        color: Bo.silver
                                        font.pixelSize: 9
                                        font.weight: Font.DemiBold
                                    }
                                }
                            }
                            MouseArea {
                                id: sma
                                anchors.fill: parent
                                onClicked: {
                                    sceneRow.forceActiveFocus()
                                    bridge.load_scene(sceneRow.loadKey)
                                }
                            }
                        }
                    }
                    Text {
                        // Only when the daemon is actually up — otherwise an
                        // empty list just means it hasn't started yet (the
                        // quick-play buttons above will start it), not that
                        // there are no scene files.
                        visible: bridge.daemon_up && root.sceneList.length === 0
                        text: "No soundscapes found. Drop *.soundscape.json into the engine's scene dir."
                        color: Bo.slate; font.pixelSize: Bo.captionSize
                        width: parent.width; wrapMode: Text.Wrap
                    }
                }
            }

            // ── Transport (daemon up) ───────────────────────────────
            ControlCard {
                visible: bridge.daemon_up
                width: parent.width
                accent: root.accentOr()
                label: "PLAYBACK"
                active: bridge.playing
                Row {
                    spacing: Bo.gapMD
                    anchors.horizontalCenter: parent.horizontalCenter
                    StepKey {
                        label: bridge.playing ? "STOP" : "PLAY"
                        wide: true; accent: root.accentOr()
                        active: bridge.playing
                        onTriggered: bridge.playing ? bridge.stop() : bridge.play()
                    }
                }
            }

            // ── Peers / speakers on the mesh ────────────────────────
            ControlCard {
                visible: bridge.daemon_up
                width: parent.width
                accent: root.accentOr()
                label: "ROOM"
                Column {
                    width: parent.width
                    spacing: Bo.gapSM
                    InfoRow { k: "Role"; v: bridge.role || "idle" }
                    Repeater {
                        model: root.peerList
                        delegate: InfoRow {
                            k: modelData.name || "device"
                            v: (modelData.platform || "") + (modelData.speakers
                                  ? "  ·  " + modelData.speakers.length + " spk" : "")
                        }
                    }
                    Text {
                        visible: root.peerList.length === 0
                        text: "No peers yet. Open the app on another device to form a room."
                        color: Bo.slate; font.pixelSize: Bo.captionSize
                        width: parent.width; wrapMode: Text.Wrap
                    }
                    Row {
                        spacing: Bo.gapSM
                        StepKey {
                            label: "STEREO"; small: true; accent: root.accentOr()
                            onTriggered: bridge.start_session("stereo")
                        }
                        StepKey {
                            label: "QUAD"; small: true; accent: root.accentOr()
                            onTriggered: bridge.start_session("quad")
                        }
                        StepKey {
                            label: "5.1"; small: true; accent: root.accentOr()
                            onTriggered: bridge.start_session("5.1")
                        }
                        StepKey {
                            label: "7.1"; small: true; accent: root.accentOr()
                            onTriggered: bridge.start_session("7.1")
                        }
                        StepKey {
                            label: "LEAVE"; small: true; accent: root.accentOr()
                            onTriggered: bridge.leave_session()
                        }
                    }
                }
            }

            // ── Speaker Fabric — drag layers onto speakers ──────────
            //
            // Visualizes the bo-mux mesh fabric: every soundscape LAYER
            // (draggable chip) and every SPEAKER across all peers (drop
            // target tile). Drag a layer onto a speaker to pin that
            // layer's channel to that speaker — the daemon injects a
            // cost-0 user claim (PinLayer) so it overrides auto-assign,
            // and the assignment converges within one mesh tick across
            // the whole fabric.
            //
            // Shown only when the fabric has speakers (mesh up). On a
            // single device there's nothing to route across, so we hide
            // it and the auto-assigner handles the one local sink.
            ControlCard {
                visible: bridge.daemon_up && root.speakerList.length > 0
                width: parent.width
                accent: root.accentOr()
                label: "SPEAKER FABRIC"

                Column {
                    width: parent.width
                    spacing: Bo.gapMD

                    Text {
                        width: parent.width
                        text: "Drag a layer onto a speaker to route it there."
                        color: Bo.silver
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }

                    // ── Draggable layer chips ──────────────────────
                    Flow {
                        id: layerFlow
                        width: parent.width
                        spacing: Bo.gapSM

                        Repeater {
                            model: root.layerList
                            delegate: Item {
                                id: chipWrap
                                width: chip.width
                                height: chip.height
                                property string chCh: modelData.channel || modelData.name
                                property string chName: modelData.name

                                Rectangle {
                                    id: chip
                                    width: chipLabel.implicitWidth + 26
                                    height: 34
                                    radius: 17
                                    color: chipDrag.active
                                        ? Bo.mix(Bo.surface, root.accentOr(), 0.55)
                                        : Bo.mix(Bo.surface, root.accentOr(), 0.22)
                                    border.color: root.accentOr()
                                    border.width: 1

                                    Row {
                                        anchors.centerIn: parent
                                        spacing: 6
                                        Text {
                                            anchors.verticalCenter: parent.verticalCenter
                                            text: "⠿"  // drag-grip glyph
                                            color: Bo.slate
                                            font.pixelSize: 12
                                        }
                                        Text {
                                            id: chipLabel
                                            anchors.verticalCenter: parent.verticalCenter
                                            text: chipWrap.chName
                                                + "  ·  " + chipWrap.chCh
                                            color: Bo.bone
                                            font.pixelSize: 11
                                            font.weight: Font.DemiBold
                                        }
                                    }

                                    // The Drag attached props let this chip
                                    // be dropped onto a DropArea. We carry
                                    // the channel + name in the mime data.
                                    Drag.active: chipDrag.active
                                    Drag.hotSpot.x: width / 2
                                    Drag.hotSpot.y: height / 2
                                    Drag.keys: ["bo-layer"]
                                    // Custom payload read by the DropArea.
                                    property string dragChannel: chipWrap.chCh
                                    property string dragName: chipWrap.chName

                                    MouseArea {
                                        id: chipDrag
                                        anchors.fill: parent
                                        drag.target: chip
                                        cursorShape: Qt.OpenHandCursor
                                        onReleased: {
                                            // Snap back to the flow position
                                            // whether or not a drop landed.
                                            chip.Drag.drop()
                                            chip.x = 0
                                            chip.y = 0
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Hairline divider
                    Rectangle {
                        width: parent.width; height: 1
                        color: Bo.hairline
                    }

                    // ── Speaker drop tiles ─────────────────────────
                    Flow {
                        width: parent.width
                        spacing: Bo.gapSM

                        Repeater {
                            model: root.speakerList
                            delegate: DropArea {
                                id: drop
                                width: 132
                                height: 84
                                keys: ["bo-layer"]

                                property string spkId: modelData.id
                                property string spkName: modelData.name
                                property string nodeName: modelData.node_name

                                onDropped: function(drag) {
                                    var ch = drag.source.dragChannel
                                    if (ch && ch.length > 0) {
                                        bridge.pin_layer(ch, drop.spkId)
                                        flashTimer.restart()
                                        drop.justPinned = ch
                                    }
                                }
                                property string justPinned: ""
                                Timer {
                                    id: flashTimer
                                    interval: 1200
                                    onTriggered: drop.justPinned = ""
                                }

                                // Which channels are currently routed here?
                                property var routedChannels: {
                                    var out = []
                                    var m = root.assignmentMap
                                    for (var ch in m) {
                                        if (m[ch] === drop.spkId) out.push(ch)
                                    }
                                    return out
                                }

                                Rectangle {
                                    anchors.fill: parent
                                    radius: Bo.cardRadius
                                    color: drop.containsDrag
                                        ? Bo.mix(Bo.surface, root.accentOr(), 0.40)
                                        : (drop.justPinned.length > 0
                                            ? Bo.mix(Bo.surface, "#5BA86F", 0.30)
                                            : Bo.surfaceHi)
                                    border.color: drop.containsDrag
                                        ? root.accentOr()
                                        : Bo.hairline
                                    border.width: drop.containsDrag ? 2 : 1

                                    Column {
                                        anchors.fill: parent
                                        anchors.margins: 8
                                        spacing: 3

                                        // Speaker icon + name
                                        Row {
                                            width: parent.width
                                            spacing: 5
                                            Text {
                                                text: "🔈"
                                                font.pixelSize: 13
                                            }
                                            Text {
                                                width: parent.width - 22
                                                text: drop.spkName
                                                color: Bo.bone
                                                font.pixelSize: 11
                                                font.weight: Font.DemiBold
                                                elide: Text.ElideRight
                                            }
                                        }
                                        // Node it lives on
                                        Text {
                                            width: parent.width
                                            text: drop.nodeName
                                            color: Bo.slate
                                            font.pixelSize: 8
                                            elide: Text.ElideRight
                                        }
                                        // Routed channel badges
                                        Flow {
                                            width: parent.width
                                            spacing: 3
                                            Repeater {
                                                model: drop.routedChannels
                                                delegate: Rectangle {
                                                    width: chTxt.implicitWidth + 10
                                                    height: 16
                                                    radius: 8
                                                    color: Bo.mix(Bo.surface,
                                                        root.accentOr(), 0.35)
                                                    Text {
                                                        id: chTxt
                                                        anchors.centerIn: parent
                                                        text: modelData
                                                        color: Bo.bone
                                                        font.pixelSize: 8
                                                        font.weight: Font.DemiBold
                                                    }
                                                }
                                            }
                                            Text {
                                                visible: drop.routedChannels.length === 0
                                                text: "—"
                                                color: Bo.slate
                                                font.pixelSize: 9
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Auto-rebalance hint
                    Text {
                        width: parent.width
                        text: "Unpinned layers auto-route to available speakers. "
                            + "Coordinator: " + (root.fabric.role || "idle") + "."
                        color: Bo.slate
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }
                }
            }


            // ── Frequency sanitizer ─────────────────────────────────
            // Mixes shaped continuous noise (psychoacoustic masker) with
            // phase-tracked anti-tones at the user's chosen fundamental.
            // Designed to fade out continuous environmental rumble — car
            // engine drone, HVAC, fan motors. Honest scope: this isn't
            // active noise cancellation (no mic feedback), but it
            // genuinely helps for periodic noise sources.
            // ── Anticipation aggression ─────────────────────────────
            //
            // Drives how forward-leaning the controller is about pre-
            // staging audio. LOW is the safe default; HIGH burns more
            // bandwidth on wrong guesses but minimizes perceived
            // tap-to-audio latency. Persists across restarts.
            ControlCard {
                width: parent.width
                accent: root.accentOr()
                label: "ANTICIPATION"

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: {
                            switch (qtobj.aggression_level) {
                            case 0: return "Off — every tap waits for the device to respond. No pre-staging, no cue, no shadow."
                            case 1: return "Low — basic pre-stage on browse, gentle cue on tap. Safe default; no shadow stream."
                            case 2: return "Medium — wider pre-stage, tempo-matched cue with harmonics, shadow on high confidence."
                            default: return "High — eager pre-stage, longest cue, shadow on moderate confidence. Experimental."
                            }
                        }
                        color: Bo.silver
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }

                    Row {
                        width: parent.width
                        spacing: Bo.gapSM
                        Repeater {
                            model: [
                                { label: "OFF",    level: 0 },
                                { label: "LOW",    level: 1 },
                                { label: "MEDIUM", level: 2 },
                                { label: "HIGH",   level: 3 },
                            ]
                            delegate: StepKey {
                                label: modelData.label
                                small: true
                                wide: false
                                active: qtobj.aggression_level === modelData.level
                                accent: root.accentOr()
                                onTriggered: qtobj.set_aggression(modelData.level)
                            }
                        }
                    }

                    // Diagnostic strip — visible at MEDIUM+ so the user
                    // can see what's actually being pre-staged. Compact;
                    // not meant for everyday use but useful while tuning.
                    Item {
                        width: parent.width
                        height: visible ? diagText.implicitHeight + Bo.gapXS : 0
                        visible: qtobj.aggression_level >= 2
                        Column {
                            width: parent.width
                            spacing: 2
                            // Current time-of-day window + observed
                            // network latency. Anticipator predictions
                            // bias on the window; cue durations bias on
                            // the RTT. Shown side-by-side so the user
                            // can correlate their tuning with what the
                            // system perceives.
                            Row {
                                spacing: 16
                                Text {
                                    text: "WINDOW · " + qtobj.current_time_window
                                    color: Bo.bronzeDim
                                    font.pixelSize: 9
                                    font.letterSpacing: 1.2
                                    font.weight: Font.DemiBold
                                }
                                Text {
                                    visible: qtobj.rtt_p50_ms > 0
                                    text: "RTT · " + qtobj.rtt_p50_ms + " ms p50"
                                          + (qtobj.rtt_p95_ms > 0
                                             ? " · " + qtobj.rtt_p95_ms + " ms p95" : "")
                                    color: Bo.bronzeDim
                                    font.pixelSize: 9
                                    font.letterSpacing: 1.2
                                    font.weight: Font.DemiBold
                                }
                            }
                            Text {
                                id: diagText
                                width: parent.width
                                text: {
                                    try {
                                        var preds = JSON.parse(qtobj.prediction_json || "[]")
                                        if (preds.length === 0) return "(no predictions yet)"
                                        var parts = []
                                        for (var i = 0; i < Math.min(3, preds.length); i++) {
                                            parts.push(preds[i].action
                                                       + " " + Math.round(preds[i].confidence * 100) + "%")
                                        }
                                        return "Next likely: " + parts.join("  ·  ")
                                    } catch (e) { return "" }
                                }
                                color: Bo.slate
                                font.pixelSize: 10
                                font.family: "monospace"
                                wrapMode: Text.Wrap
                            }
                        }
                    }
                }
            }

            ControlCard {
                width: parent.width
                accent: root.accentOr()
                label: "FREQUENCY SANITIZER"

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    Text {
                        width: parent.width
                        text: qtobj.sanitizer_running
                            ? "Running — " + String(qtobj.sanitizer_preset || "").toUpperCase()
                              + (qtobj.sanitizer_freq > 0
                                 ? " · " + Math.round(qtobj.sanitizer_freq) + " Hz" : "")
                            : (qtobj.sanitizer_error.length > 0
                                ? qtobj.sanitizer_error
                                : "Off — pick a preset and start to mask continuous noise.")
                        color: qtobj.sanitizer_error.length > 0
                                ? "#c89090" : Bo.silver
                        font.pixelSize: Bo.captionSize
                        wrapMode: Text.Wrap
                    }

                    // Preset row (Pink / Brown / Engine / Road / Fan / HVAC)
                    Flow {
                        width: parent.width
                        spacing: 6
                        property string current: qtobj.sanitizer_preset || "engine"
                        Repeater {
                            model: [
                                {id: "pink",   label: "PINK",   freq: 80},
                                {id: "brown",  label: "BROWN",  freq: 80},
                                {id: "engine", label: "ENGINE", freq: 80},
                                {id: "road",   label: "ROAD",   freq: 120},
                                {id: "fan",    label: "FAN",    freq: 60},
                                {id: "hvac",   label: "HVAC",   freq: 50}
                            ]
                            delegate: Rectangle {
                                property bool sel: qtobj.sanitizer_preset === modelData.id
                                width: ptext.implicitWidth + 18
                                height: 26
                                radius: 13
                                color: sel ? root.accentOr()
                                           : (pma.pressed ? Bo.surfaceHi : Bo.surface)
                                border.color: sel ? root.accentOr() : Bo.hairline
                                border.width: 1
                                Behavior on color { ColorAnimation { duration: Bo.quick } }
                                Text {
                                    id: ptext
                                    anchors.centerIn: parent
                                    text: modelData.label
                                    color: parent.sel ? Bo.graphite : Bo.bone
                                    font.pixelSize: 10
                                    font.weight: Font.DemiBold
                                    font.letterSpacing: 1
                                }
                                MouseArea {
                                    id: pma
                                    anchors.fill: parent
                                    onClicked: {
                                        // If sanitizer is on, retune to the new preset by
                                        // restarting; if off, just remember the choice
                                        // and let user tap START.
                                        if (qtobj.sanitizer_running) {
                                            qtobj.start_sanitizer(modelData.id,
                                                modelData.freq, freqSlider.gain)
                                        } else {
                                            // Pre-set selection by starting then stopping
                                            // would be wasteful — instead, update the
                                            // slider's freq default and rely on the user
                                            // pressing START.
                                            freqSlider.lastPreset = modelData.id
                                            freqSlider.defaultFreq = modelData.freq
                                            freqSlider.value = modelData.freq
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Anti-tone fundamental slider — 20 to 250 Hz
                    Column {
                        id: freqSlider
                        width: parent.width
                        property real value: 80
                        property real defaultFreq: 80
                        property string lastPreset: "engine"
                        property real gain: -22
                        spacing: 4
                        Row {
                            width: parent.width
                            spacing: 8
                            Text {
                                text: "ANTI-TONE Hz"; color: Bo.silver
                                font.pixelSize: 9
                                font.letterSpacing: Bo.labelTracking
                                font.weight: Font.DemiBold
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: Math.round(freqSlider.value)
                                color: Bo.bone
                                font.pixelSize: Bo.captionSize
                                font.weight: Font.Medium
                            }
                        }
                        // Slider rendered as a draggable handle on a track.
                        Item {
                            width: parent.width
                            height: 26
                            Rectangle {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                height: 3
                                radius: 2
                                color: Bo.hairline
                            }
                            // Active portion
                            Rectangle {
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                height: 3
                                radius: 2
                                width: (parent.width - 16)
                                       * (freqSlider.value - 20) / (250 - 20)
                                color: root.accentOr()
                                Behavior on width { NumberAnimation { duration: Bo.quick } }
                            }
                            Rectangle {
                                id: handle
                                width: 16; height: 16; radius: 8
                                anchors.verticalCenter: parent.verticalCenter
                                x: (parent.width - 16)
                                   * (freqSlider.value - 20) / (250 - 20)
                                color: root.accentOr()
                                Behavior on x { NumberAnimation { duration: Bo.quick } }
                            }
                            MouseArea {
                                anchors.fill: parent
                                onPressed: function(m) {
                                    var ratio = m.x / (parent.width - 16)
                                    freqSlider.value = Math.max(20,
                                        Math.min(250, 20 + ratio * (250 - 20)))
                                    if (qtobj.sanitizer_running) {
                                        qtobj.retune_sanitizer(freqSlider.value)
                                    }
                                }
                                onPositionChanged: function(m) {
                                    if (pressed) {
                                        var ratio = m.x / (parent.width - 16)
                                        freqSlider.value = Math.max(20,
                                            Math.min(250, 20 + ratio * (250 - 20)))
                                        if (qtobj.sanitizer_running) {
                                            qtobj.retune_sanitizer(freqSlider.value)
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Start / Stop buttons
                    Row {
                        spacing: Bo.gapSM
                        StepKey {
                            label: qtobj.sanitizer_running ? "STOP" : "START"
                            small: true; accent: root.accentOr()
                            active: qtobj.sanitizer_running
                            onTriggered: {
                                if (qtobj.sanitizer_running) {
                                    qtobj.stop_sanitizer()
                                } else {
                                    // Pick the preset the user tapped (or default).
                                    var pid = qtobj.sanitizer_preset
                                            || freqSlider.lastPreset || "engine"
                                    qtobj.start_sanitizer(pid,
                                        freqSlider.value, freqSlider.gain)
                                }
                            }
                        }
                        StepKey {
                            label: "NOISE ONLY"
                            small: true; accent: root.accentOr()
                            visible: qtobj.sanitizer_running
                            onTriggered: qtobj.retune_sanitizer(0)
                        }
                    }

                    // ── Adaptive (mic-driven phase cancellation) ────────
                    // Opens a microphone capture, learns the dominant periodic
                    // noise pattern via autocorrelation, and mixes the inverse
                    // template into the sanitizer output. Works only for
                    // strongly periodic noise (motors, fans, engines) and
                    // depends on PipeWire/PulseAudio quantum scheduling to
                    // keep mic-capture cycles aligned with output cycles.
                    Rectangle {
                        width: parent.width
                        height: 1
                        color: Bo.hairline
                        visible: qtobj.sanitizer_running
                    }

                    Column {
                        width: parent.width
                        spacing: 4
                        visible: qtobj.sanitizer_running

                        Row {
                            width: parent.width
                            spacing: 8
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: "ADAPTIVE PHASE-CANCEL"
                                color: Bo.silver
                                font.pixelSize: 9
                                font.letterSpacing: Bo.labelTracking
                                font.weight: Font.DemiBold
                            }
                            Item { width: 1; height: 1
                                anchors.verticalCenter: parent.verticalCenter }
                            // Live detected-frequency readout when adaptive is on
                            Rectangle {
                                visible: qtobj.adaptive_active
                                anchors.verticalCenter: parent.verticalCenter
                                width: detected.implicitWidth + 14
                                height: 18
                                radius: 9
                                color: qtobj.adaptive_score > 0.5 ? root.accentOr()
                                                                  : Bo.surfaceHi
                                Text {
                                    id: detected
                                    anchors.centerIn: parent
                                    text: qtobj.adaptive_detected_hz > 0
                                        ? Math.round(qtobj.adaptive_detected_hz) + " Hz · "
                                            + Math.round(qtobj.adaptive_score * 100) + "%"
                                        : "listening…"
                                    color: qtobj.adaptive_score > 0.5
                                            ? Bo.graphite : Bo.bone
                                    font.pixelSize: 10
                                    font.weight: Font.DemiBold
                                    font.letterSpacing: 0.5
                                }
                            }
                        }

                        Text {
                            width: parent.width
                            visible: qtobj.adaptive_error.length > 0
                            text: qtobj.adaptive_error
                            color: "#c89090"
                            font.pixelSize: Bo.captionSize
                            wrapMode: Text.Wrap
                        }
                        Text {
                            width: parent.width
                            visible: !qtobj.adaptive_active &&
                                     qtobj.adaptive_error.length === 0
                            text: "Mic-driven cancellation for steady motors/fans/engines. "
                                + "Learns the noise's repeating pattern, plays its inverse."
                            color: Bo.silver
                            font.pixelSize: Bo.captionSize
                            wrapMode: Text.Wrap
                        }
                        Text {
                            width: parent.width
                            visible: qtobj.adaptive_active
                            text: qtobj.adaptive_score > 0.5
                                ? ("Cancelling — mic via " + qtobj.adaptive_backend
                                   + ". Re-learns every 5 s to track drift.")
                                : ("Listening via " + qtobj.adaptive_backend
                                   + " — need stronger periodicity to lock on.")
                            color: Bo.silver
                            font.pixelSize: Bo.captionSize
                            wrapMode: Text.Wrap
                        }

                        Row {
                            spacing: Bo.gapSM
                            StepKey {
                                label: qtobj.adaptive_active ? "STOP MIC" : "START MIC"
                                small: true
                                accent: root.accentOr()
                                active: qtobj.adaptive_active
                                onTriggered: {
                                    if (qtobj.adaptive_active) {
                                        qtobj.stop_adaptive_cancellation()
                                    } else {
                                        qtobj.start_adaptive_cancellation()
                                    }
                                }
                            }
                        }

                        // Poll the runner state ~1 Hz so the readout stays live.
                        Timer {
                            interval: 1000
                            running: qtobj.adaptive_active
                            repeat: true
                            onTriggered: qtobj.refresh_adaptive_status()
                        }
                    }
                }
            }

            // ── Setup & diagnostics ─────────────────────────────────
            ControlCard {
                id: diagnosticsCard
                width: parent.width
                accent: root.accentOr()
                label: "SETUP & DIAGNOSTICS"
                property var doctor: {
                    try { return JSON.parse(bridge.doctor_json || "{}") }
                    catch(e) { return {} }
                }
                // Pre-resolve the checks array up here where the
                // binding has clear access to `doctor`. Repeater
                // delegates' `parent.parent.x` chains are brittle —
                // QML evaluates `model:` in the Repeater's parse
                // scope which sometimes doesn't yet have a fully
                // resolved hierarchy, producing "Cannot read property
                // 'checks' of undefined" warnings. A named reference
                // through `diagnosticsCard.doctorChecks` is unambiguous.
                readonly property var doctorChecks: {
                    var d = doctor
                    if (d && d.checks && Array.isArray(d.checks)) return d.checks
                    return []
                }

                Column {
                    width: parent.width
                    spacing: Bo.gapSM

                    // Action buttons
                    Flow {
                        width: parent.width
                        spacing: Bo.gapSM
                        StepKey {
                            label: "RUN CHECKS"; small: true; accent: root.accentOr()
                            onTriggered: bridge.run_doctor()
                        }
                        StepKey {
                            label: bridge.daemon_status === "" ? "INSTALL" : "REINSTALL"
                            small: true; accent: root.accentOr()
                            onTriggered: bridge.run_install()
                        }
                        StepKey {
                            label: "UNINSTALL"; small: true; accent: root.accentOr()
                            visible: bridge.daemon_status !== ""
                            onTriggered: bridge.run_uninstall()
                        }
                        // FIX AUDIO — stops bo-muxd, reaps orphan
                        // paplay/ffplay subprocesses, returns the
                        // audio path to system control. Safe to run
                        // anytime; the daemon will restart on next
                        // play if it was previously installed.
                        // Visible only when there's something to fix
                        // (daemon installed) — pre-install there's
                        // no bo-mux state that could be wrong.
                        StepKey {
                            label: "FIX AUDIO"; small: true; accent: root.accentOr()
                            visible: bridge.daemon_status !== ""
                            onTriggered: bridge.fix_audio()
                        }
                    }

                    // ── Streaming-script polling timer ─────────
                    // While a script is running, drain the
                    // background channel every 200ms so setup_log
                    // updates progressively instead of all-at-once
                    // at completion. 200ms is fast enough to feel
                    // live, slow enough that the cost (one mutex
                    // try_recv per tick) is invisible.
                    Timer {
                        interval: 200
                        running: bridge.setup_running
                        repeat: true
                        onTriggered: bridge.poll_setup()
                    }

                    // Running indicator
                    Row {
                        visible: bridge.setup_running
                        spacing: Bo.gapSM
                        Rectangle {
                            width: 8; height: 8; radius: 4
                            anchors.verticalCenter: parent.verticalCenter
                            color: root.accentOr()
                            SequentialAnimation on opacity {
                                running: bridge.setup_running; loops: Animation.Infinite
                                NumberAnimation { to: 0.3; duration: 600 }
                                NumberAnimation { to: 1.0; duration: 600 }
                            }
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Running…"; color: Bo.silver; font.pixelSize: Bo.captionSize
                        }
                    }

                    // Doctor checklist (rendered from doctor_json).
                    // Reads through diagnosticsCard.doctorChecks (a
                    // pre-resolved array) instead of walking
                    // `parent.parent.doctor.checks` which was
                    // producing intermittent "checks of undefined"
                    // warnings during initial parse.
                    Repeater {
                        model: diagnosticsCard.doctorChecks
                        delegate: Row {
                            width: parent.width
                            spacing: Bo.gapSM
                            Rectangle {
                                width: 16; height: 16; radius: 8
                                anchors.verticalCenter: parent.verticalCenter
                                color: modelData.status === "pass" ? "#4a7c59"
                                     : modelData.status === "warn" ? root.accentOr()
                                     : "#8c4a4a"
                                Text {
                                    anchors.centerIn: parent
                                    text: modelData.status === "pass" ? "✓"
                                        : modelData.status === "warn" ? "!" : "✕"
                                    color: Bo.graphite; font.pixelSize: 10; font.weight: Font.Bold
                                }
                            }
                            Column {
                                anchors.verticalCenter: parent.verticalCenter
                                width: parent.width - 24
                                Text {
                                    text: modelData.name
                                    color: Bo.bone; font.pixelSize: Bo.captionSize
                                    font.weight: Font.Medium
                                }
                                Text {
                                    width: parent.width
                                    text: modelData.detail || ""
                                    color: Bo.slate; font.pixelSize: 10
                                    elide: Text.ElideRight
                                }
                            }
                        }
                    }

                    // Raw log (shown when there's output)
                    Rectangle {
                        visible: (bridge.setup_log || "").length > 0
                        width: parent.width
                        height: Math.min(220, logText.implicitHeight + 16)
                        radius: 8
                        color: Bo.ink
                        border.color: Bo.hairline; border.width: 1
                        Flickable {
                            id: logFlick
                            anchors.fill: parent
                            anchors.margins: 8
                            contentHeight: logText.implicitHeight
                            clip: true
                            // Auto-scroll to bottom when new content
                            // lands. The setup_log property changes
                            // each time poll_setup drains a line; on
                            // change, jump the Flickable to the end
                            // so the user sees the freshest output.
                            Connections {
                                // Connections.target must be a real QObject
                                // (or null). `bridge` can be a plain JS
                                // fallback object when the daemon bridge
                                // isn't bound, which QML can't accept here
                                // ("Unable to assign QJSValue to QObject*").
                                // Bind to the underlying `mux` QObject when
                                // available, else null (handlers just won't
                                // fire — fine, there's no log to scroll).
                                target: root.bridgeAvailable ? mux : null
                                enabled: root.bridgeAvailable
                                function onSetup_changed() {
                                    // Defer to next tick so logText
                                    // has remeasured implicitHeight.
                                    Qt.callLater(function() {
                                        if (logText.implicitHeight > logFlick.height) {
                                            logFlick.contentY = logText.implicitHeight - logFlick.height
                                        }
                                    })
                                }
                            }
                            Text {
                                id: logText
                                width: parent.width
                                // Colorize STEP/OK/WARN/ERR prefixes
                                // by mapping each line through a
                                // span. The output is rich text
                                // (Qt-flavor HTML subset). Lines
                                // without a recognized prefix render
                                // in the default silver color so
                                // "$ bo-mux-install.sh ..." and the
                                // bare DONE marker still look
                                // clean.
                                //
                                // Order: STEP=gold, OK=green,
                                // WARN=amber, ERR=red, DONE=gold.
                                text: {
                                    var lines = String(bridge.setup_log || "").split("\n")
                                    var out = []
                                    for (var i = 0; i < lines.length; ++i) {
                                        var line = lines[i]
                                        var esc = line
                                            .replace(/&/g, "&amp;")
                                            .replace(/</g, "&lt;")
                                            .replace(/>/g, "&gt;")
                                        if (line.indexOf("STEP ") === 0) {
                                            out.push("<span style='color:#C8A04B'>" + esc + "</span>")
                                        } else if (line.indexOf("OK   ") === 0 || line.indexOf("OK ") === 0) {
                                            out.push("<span style='color:#7CB07C'>" + esc + "</span>")
                                        } else if (line.indexOf("WARN ") === 0) {
                                            out.push("<span style='color:#D0A050'>" + esc + "</span>")
                                        } else if (line.indexOf("ERR ") === 0 || line.indexOf("ERR  ") === 0 || line.indexOf("ERR   ") === 0) {
                                            out.push("<span style='color:#D87070'>" + esc + "</span>")
                                        } else if (line === "DONE") {
                                            out.push("<span style='color:#C8A04B'><b>" + esc + "</b></span>")
                                        } else {
                                            out.push(esc)
                                        }
                                    }
                                    return out.join("<br/>")
                                }
                                textFormat: Text.RichText
                                color: Bo.silver
                                font.pixelSize: 10
                                font.family: "monospace"
                                wrapMode: Text.Wrap
                            }
                        }
                    }
                }
            }
        }
    }

    // Use the window accent if available, else a sensible default.
    function accentOr() {
        return (typeof window !== "undefined" && window.accent) ? window.accent : "#C8A04B"
    }

    // Small inline install/update button with progress feel.
    component InstallButton: Rectangle {
        property string label: ""
        property color accent: "#C8A04B"
        signal tapped()
        width: parent.width
        height: 44
        radius: 22
        color: ibma.pressed ? Bo.shade(accent) : accent
        Behavior on color { ColorAnimation { duration: Bo.quick } }
        Text {
            anchors.centerIn: parent
            text: parent.label
            color: Bo.graphite
            font.pixelSize: Bo.bodySize
            font.weight: Font.DemiBold
        }
        MouseArea { id: ibma; anchors.fill: parent; onClicked: parent.tapped() }
    }
}
