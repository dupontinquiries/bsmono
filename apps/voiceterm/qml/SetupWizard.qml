/*
 * SetupWizard — the model setup on-ramp.
 *
 * Since v0.1.533 Ollama is opt-in (off by default), so a fresh user
 * with no local model + no Claude key has NO working LLM until they set
 * one up — this wizard is the primary path to a working assistant
 * (fixes F3: the old CTA just dumped the user into settings).
 *
 * Reads backend.setup_recommendation_json():
 *   { ram_gb, llama_available, has_active_model, recommended_index,
 *     ollama_fallback, setup_required }
 * and offers three routes:
 *   1. One-tap download the RAM-aware ★ recommended model.
 *   2. Browse all models (reuses the existing ModelPicker).
 *   3. Enable the Ollama fallback (escape hatch, if they already run it).
 *
 * Pure presentation over verified backend methods:
 *   setup_recommendation_json, curated_models_json, download_model,
 *   download_progress_json, set_llama_model_path,
 *   set_ollama_fallback_enabled, setup_mark_seen.
 */
import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

Item {
    id: wizard
    anchors.fill: parent
    visible: false
    z: 99999

    property var backend: null

    // v0.1.544: re-rooted from Popup to Item. Every WORKING component
    // file in this codebase (ConfirmDialog, ModelPicker, SlidePopup,
    // GestureHost) roots at Item; SetupWizard was the ONLY Popup-rooted
    // one and the ONLY one that never instantiated. A Popup as a bare-
    // type component root doesn't load in this qmetaobject/Qt setup.
    // This mirrors ConfirmDialog's Item-overlay modal pattern exactly.

    // v0.1.543: QML console.log is silently dropped on some Qt6/Wayland
    // configs (see backend.log_message docs). Route through
    // backend.log_message, which always reaches stderr.
    function _wlog(m) {
        if (backend && backend.log_message) backend.log_message("[wizard-qml] " + m)
        else console.log("[wizard-qml] " + m)
    }
    Component.onCompleted: _wlog("SetupWizard instantiated; backend=" + (backend ? "present" : "NULL"))

    // Parsed recommendation state.
    property int ramGb: 0
    property bool llamaAvailable: false
    property bool hasActiveModel: false
    property int recommendedIndex: -1
    property bool ollamaFallback: false
    property bool setupRequired: false

    // Curated model list (for the recommended pick label) + the picker.
    property var curated: []
    property string phase: "intro"   // intro | downloading | done | browse

    // Voice (whisper) model state.
    property bool whisperPresent: false
    property var whisperCatalog: []
    // When true, the active download is a whisper model (so completion
    // doesn't try to set_llama_model_path, which is GGUF-only).
    property bool downloadingWhisper: false

    // Download tracking.
    property int dlPercent: 0
    property string dlStatus: ""

    // open()/close() toggle visibility (ConfirmDialog pattern). No
    // Popup overlay machinery — this Item fills the window.
    function open() {
        _wlog("open() called")
        wizard.refresh()
        wizard.visible = true
        wizard.forceActiveFocus()
    }
    function close() {
        wizard.visible = false
    }

    function refresh() {
        if (!backend) { _wlog("refresh: no backend"); return }
        if (backend.setup_recommendation_json) {
            try {
                var r = JSON.parse(backend.setup_recommendation_json())
                ramGb = r.ram_gb || 0
                llamaAvailable = !!r.llama_available
                hasActiveModel = !!r.has_active_model
                recommendedIndex = (typeof r.recommended_index === "number") ? r.recommended_index : -1
                ollamaFallback = !!r.ollama_fallback
                setupRequired = !!r.setup_required
            } catch (e) {
                _wlog("refresh: recommendation parse failed: " + e)
            }
        }
        if (backend.curated_models_json) {
            try { curated = JSON.parse(backend.curated_models_json()) }
            catch (e) { curated = [] }
        }
        // Voice model state.
        if (backend.whisper_present) {
            try { whisperPresent = backend.whisper_present() }
            catch (e) { whisperPresent = false }
        }
        if (backend.whisper_catalog_json) {
            try { whisperCatalog = JSON.parse(backend.whisper_catalog_json()) }
            catch (e) { whisperCatalog = [] }
        }
        _wlog("refresh done: curated=" + curated.length
            + " whisperCatalog=" + whisperCatalog.length
            + " whisperPresent=" + whisperPresent
            + " llamaAvail=" + llamaAvailable
            + " recIdx=" + recommendedIndex)
        phase = hasActiveModel ? "done" : "intro"
    }

    // The recommended model row from the curated list, or null.
    function recommendedModel() {
        if (recommendedIndex < 0 || recommendedIndex >= curated.length) return null
        return curated[recommendedIndex]
    }

    function startRecommendedDownload() {
        var m = recommendedModel()
        _wlog("startRecommendedDownload: backend=" + (backend ? "present" : "NULL")
            + " model=" + (m ? (m.repo_id + "/" + m.filename) : "NULL")
            + " curatedLen=" + curated.length + " recIdx=" + recommendedIndex)
        if (!m || !backend || !backend.download_model) return
        phase = "downloading"
        dlPercent = 0
        dlStatus = "Starting\u2026"
        backend.download_model(m.repo_id || "", m.filename || "")
    }

    function enableOllama() {
        if (backend && backend.set_ollama_fallback_enabled) {
            backend.set_ollama_fallback_enabled(true)
            ollamaFallback = true
        }
        finishAndClose()
    }

    // Download the default whisper set (tiny-q + base-q). The shared
    // progress state handles one at a time, so we chain: download the
    // models in the default set sequentially via _whisperQueue.
    property var _whisperQueue: []
    function startVoiceDownload() {
        _wlog("startVoiceDownload: backend=" + (backend ? "present" : "NULL")
            + " hasMethod=" + (backend && backend.download_whisper_model ? "yes" : "no")
            + " catalogLen=" + whisperCatalog.length)
        if (!backend || !backend.download_whisper_model) return
        // queue the default-set filenames
        _whisperQueue = []
        for (var i = 0; i < whisperCatalog.length; i++) {
            if (whisperCatalog[i].default) _whisperQueue.push(whisperCatalog[i].filename)
        }
        if (_whisperQueue.length === 0) return
        downloadingWhisper = true
        phase = "downloading"
        dlPercent = 0
        dlStatus = "Starting\u2026"
        _nextWhisper()
    }
    function _nextWhisper() {
        if (_whisperQueue.length === 0) {
            // all voice models done
            whisperPresent = true
            downloadingWhisper = false
            phase = "done"
            return
        }
        var f = _whisperQueue.shift()
        backend.download_whisper_model(f)
    }

    function finishAndClose() {
        if (backend && backend.setup_mark_seen) backend.setup_mark_seen()
        wizard.close()
    }

    // Poll download progress while downloading. The backend encodes
    // completion + path in the STATUS string as "done:<path>" (the same
    // protocol ModelPicker uses), not a separate done/path field.
    Timer {
        id: dlPoll
        interval: 500
        repeat: true
        running: wizard.visible && wizard.phase === "downloading"
        onTriggered: {
            if (!backend || !backend.download_progress_json) return
            try {
                var p = JSON.parse(backend.download_progress_json())
                wizard.dlPercent = p.percent || 0
                var s = p.status || ""
                wizard.dlStatus = s
                if (s.indexOf("done:") === 0) {
                    var newPath = s.substring(5)
                    if (wizard.downloadingWhisper) {
                        // voice model finished — chain to the next in
                        // the default set (no set_llama_model_path; that
                        // is GGUF-only).
                        wizard._nextWhisper()
                    } else {
                        if (newPath.length > 0 && backend.set_llama_model_path) {
                            backend.set_llama_model_path(newPath)
                        }
                        wizard.hasActiveModel = true
                        wizard.phase = "done"
                    }
                }
            } catch (e) { /* keep polling */ }
        }
    }

    // Dim scrim — click outside the card closes the wizard.
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        onClicked: wizard.close()
        Rectangle {
            anchors.fill: parent
            color: "#000"
            opacity: 0.55
        }
    }

    // The wizard card. Clicks inside are caught (don't dismiss).
    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 520)
        height: Math.min(parent.height - 96, 460)
        color: Qt.rgba(0.10, 0.13, 0.12, 0.99)
        radius: 16
        border.color: Qt.rgba(0.40, 0.70, 0.55, 0.45)
        border.width: 1
        MouseArea { anchors.fill: parent /* swallow clicks */ }

        // ── INTRO ──
        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 4
            spacing: 14
            visible: wizard.phase === "intro"

            Text {
                Layout.fillWidth: true
                text: "\u26A1 Set up your assistant"
                color: "#e8fff4"
                font.pixelSize: 20
                font.bold: true
            }
            Text {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                color: "#bfe3cf"
                font.pixelSize: 13
                text: wizard.setupRequired
                    ? "No model is set up yet. Pick a local model to get fast, private replies that run right on this device."
                    : "Add a local model for faster, private replies that run on this device."
            }

            // Recommended pick card.
            Rectangle {
                Layout.fillWidth: true
                visible: wizard.recommendedModel() !== null
                radius: 10
                color: Qt.rgba(0.16, 0.30, 0.26, 0.9)
                border.color: Qt.rgba(0.45, 0.78, 0.60, 0.6)
                border.width: 1
                implicitHeight: _recCol.implicitHeight + 20
                ColumnLayout {
                    id: _recCol
                    anchors.fill: parent
                    anchors.margins: 10
                    spacing: 4
                    Text {
                        text: "\u2605 Recommended for " + (wizard.ramGb > 0 ? wizard.ramGb + "\u202FGB" : "this device")
                        color: "#9ff0c4"
                        font.pixelSize: 11
                        font.bold: true
                    }
                    Text {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        text: {
                            var m = wizard.recommendedModel()
                            return m ? (m.label || m.repo_id || "Recommended model") : ""
                        }
                        color: "#ffffff"
                        font.pixelSize: 14
                        font.bold: true
                    }
                    Text {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        visible: text.length > 0
                        text: {
                            var m = wizard.recommendedModel()
                            return m && m.blurb ? m.blurb : ""
                        }
                        color: "#bfe3cf"
                        font.pixelSize: 11
                    }
                }
            }

            // Voice models card — shown when no whisper model present.
            Rectangle {
                Layout.fillWidth: true
                visible: !wizard.whisperPresent
                radius: 10
                color: Qt.rgba(0.16, 0.26, 0.30, 0.9)
                border.color: Qt.rgba(0.45, 0.65, 0.78, 0.55)
                border.width: 1
                implicitHeight: _voiceCol.implicitHeight + 20
                ColumnLayout {
                    id: _voiceCol
                    anchors.fill: parent
                    anchors.margins: 10
                    spacing: 6
                    Text {
                        text: "\uD83C\uDFA4 Voice input"
                        color: "#9fd4f0"
                        font.pixelSize: 11
                        font.bold: true
                    }
                    Text {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        text: "Speech models aren't installed yet. Download the recommended pair (~88\u202FMB) to enable voice."
                        color: "#cfe6f0"
                        font.pixelSize: 12
                    }
                    Button {
                        text: "Download voice models"
                        onClicked: wizard.startVoiceDownload()
                    }
                }
            }

            // Buttons.
            RowLayout {
                Layout.fillWidth: true
                spacing: 10
                Button {
                    // v0.1.566: also disable while a download is already
                    // in flight, so the user can't re-click and spawn a
                    // competing download (the backend in-flight guard from
                    // v0.1.547 catches it, but disabling removes the
                    // confusing "nothing happened, click again" UX that
                    // caused the duplicates in the first place).
                    text: wizard.phase === "downloading" ? "Downloading\u2026" : "Download recommended"
                    enabled: wizard.llamaAvailable
                             && wizard.recommendedModel() !== null
                             && wizard.phase !== "downloading"
                    onClicked: wizard.startRecommendedDownload()
                }
                Button {
                    text: "Browse models\u2026"
                    onClicked: wizard.phase = "browse"
                }
            }

            Item { Layout.fillHeight: true }

            // Escape hatch + dismiss.
            RowLayout {
                Layout.fillWidth: true
                spacing: 10
                Text {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: "#8fb8a2"
                    font.pixelSize: 11
                    text: wizard.ollamaFallback
                        ? "Ollama fallback is on."
                        : "Already run Ollama? Enable it as a fallback."
                }
                Button {
                    visible: !wizard.ollamaFallback
                    text: "Enable Ollama"
                    onClicked: wizard.enableOllama()
                }
                Button {
                    text: wizard.setupRequired ? "Later" : "Close"
                    onClicked: wizard.finishAndClose()
                }
            }
        }

        // ── DOWNLOADING ──
        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 4
            spacing: 16
            visible: wizard.phase === "downloading"

            Text {
                text: wizard.downloadingWhisper ? "Downloading voice models\u2026" : "Downloading model\u2026"
                color: "#e8fff4"
                font.pixelSize: 18
                font.bold: true
            }
            Text {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: {
                    if (wizard.downloadingWhisper) return "Speech recognition models"
                    var m = wizard.recommendedModel()
                    return m ? (m.label || m.repo_id || "") : ""
                }
                color: "#bfe3cf"
                font.pixelSize: 13
            }
            ProgressBar {
                Layout.fillWidth: true
                from: 0; to: 100
                value: wizard.dlPercent
            }
            Text {
                text: wizard.dlPercent + "%  " + wizard.dlStatus
                color: "#9ff0c4"
                font.pixelSize: 12
            }
            Item { Layout.fillHeight: true }
            Text {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                color: "#8fb8a2"
                font.pixelSize: 11
                text: "You can keep using the app \u2014 this continues in the background."
            }
            Button {
                text: "Hide"
                onClicked: wizard.close()
            }
        }

        // ── DONE ──
        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 4
            spacing: 16
            visible: wizard.phase === "done"

            Text {
                text: "\u2713 You're set up"
                color: "#9ff0c4"
                font.pixelSize: 20
                font.bold: true
            }
            Text {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                color: "#bfe3cf"
                font.pixelSize: 13
                text: "A local model is active. Replies now run on this device \u2014 faster and private."
            }
            Item { Layout.fillHeight: true }
            Button {
                text: "Done"
                onClicked: wizard.finishAndClose()
            }
        }

        // ── BROWSE (wraps the existing ModelPicker) ──
        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 4
            spacing: 10
            visible: wizard.phase === "browse"

            RowLayout {
                Layout.fillWidth: true
                Button {
                    text: "\u2039 Back"
                    onClicked: wizard.phase = "intro"
                }
                Item { Layout.fillWidth: true }
                Text {
                    text: "Browse models"
                    color: "#e8fff4"
                    font.pixelSize: 15
                    font.bold: true
                }
                Item { Layout.fillWidth: true }
            }

            // v0.1.545: show the curated GGUF list directly. ModelPicker
            // alone showed nothing here because it's search-driven (empty
            // until you type) and gated on llama_available. The wizard
            // already has `curated`, so render it as a tappable list —
            // tapping downloads that model. This works regardless of
            // ModelPicker's search/availability state.
            Text {
                Layout.fillWidth: true
                visible: wizard.curated.length === 0
                wrapMode: Text.WordWrap
                color: "#bfe3cf"
                font.pixelSize: 12
                text: "No curated models available."
            }
            Repeater {
                model: wizard.curated
                Rectangle {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 54
                    radius: 8
                    color: _curMa.containsMouse ? Qt.rgba(0.20, 0.40, 0.34, 0.9)
                                                : Qt.rgba(0.16, 0.30, 0.26, 0.8)
                    Column {
                        anchors.left: parent.left
                        anchors.right: _curDl.left
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: 10
                        spacing: 2
                        Text {
                            width: parent.width
                            elide: Text.ElideRight
                            text: (modelData.label || modelData.repo_id || "Model")
                            color: "#ffffff"; font.pixelSize: 13; font.bold: true
                        }
                        Text {
                            width: parent.width
                            elide: Text.ElideRight
                            visible: text.length > 0
                            text: modelData.blurb || ""
                            color: "#bfe3cf"; font.pixelSize: 11
                        }
                    }
                    Button {
                        id: _curDl
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.rightMargin: 8
                        text: "Download"
                        onClicked: {
                            if (!backend || !backend.download_model) return
                            wizard.downloadingWhisper = false
                            wizard.recommendedIndex = index
                            wizard.phase = "downloading"
                            wizard.dlPercent = 0
                            wizard.dlStatus = "Starting\u2026"
                            backend.download_model(modelData.repo_id || "",
                                                   modelData.filename || "")
                        }
                    }
                    MouseArea {
                        id: _curMa
                        anchors.fill: parent
                        anchors.rightMargin: 90
                        hoverEnabled: true
                    }
                }
            }

            // Full search (only useful when llama is available).
            ModelPicker {
                Layout.fillWidth: true
                Layout.fillHeight: true
                visible: wizard.llamaAvailable
                backend: wizard.backend
            }
        }
    }
}
