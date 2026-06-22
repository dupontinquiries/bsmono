import QtQuick 2.15

/**
 * ModelPicker — local llama.cpp GGUF model browser/downloader.
 * v0.1.463 (llamacpp arc).
 *
 * Flow:
 *   1. Search box → backend.hf_search(query) (async HF search).
 *   2. Results list (repos) → tap a repo → backend.hf_list_files(repo).
 *   3. File list (quants, recommended one starred) → tap to download
 *      via backend.download_model(repo, file) with a progress bar.
 *   4. On "done:<path>": auto-activate via set_llama_model_path if
 *      no model is active yet (the quick-pick "just works" case);
 *      otherwise offer a "Use this model" button so an existing
 *      active model isn't swapped out without the user asking.
 *   5. Already-downloaded models listed at top via local_models_json.
 *
 * Async results arrive by polling backend.hf_search_revision() and
 * backend.download_progress_json() on a Timer — the same poll
 * pattern web_summary uses (no QML-side threading). All backend
 * calls are null-guarded per qml_lint.
 *
 * Visibility: the whole panel is gated on backend.llama_available
 * (false unless built with the `llamacpp` feature) so non-llamacpp
 * builds simply don't show it.
 */
Item {
    id: picker
    // backend is injected by the parent (main.qml) like other popups.
    property var backend: null

    // ── internal state ──
    property string selectedRepo: ""
    property var resultsModel: []   // current search OR file-list rows
    property bool showingFiles: false
    property int lastRevision: 0
    property string downloadStatus: ""
    property int downloadPercent: 0
    property string downloadedPath: ""
    // v0.1.549: map of downloaded GGUF filename -> true (for the
    // downloaded-vs-available badge on curated rows). Rebuilt by
    // loadLocalModels().
    property var downloadedNames: ({})

    // Whether this build supports local models at all.
    property bool available: backend ? backend.llama_available : false

    visible: available
    width: parent ? parent.width : 480
    implicitHeight: 420

    // ── result polling ──
    // Bumps to hf_search_revision mean new search/file-list JSON is
    // ready; re-read and rebuild the list model.
    Timer {
        id: resultPoll
        interval: 250; running: picker.visible; repeat: true
        onTriggered: {
            if (!backend || !backend.hf_search_revision) return
            var rev = backend.hf_search_revision()
            if (rev !== picker.lastRevision) {
                picker.lastRevision = rev
                picker.reloadResults()
            }
        }
    }

    // ── download progress polling ──
    Timer {
        id: dlPoll
        interval: 300; running: picker.visible; repeat: true
        onTriggered: {
            if (!backend || !backend.download_progress_json) return
            var raw = backend.download_progress_json()
            try {
                var st = JSON.parse(raw)
                picker.downloadPercent = st.percent
                var s = st.status || ""
                if (s.indexOf("done:") === 0) {
                    var newPath = s.substring(5)
                    // Only react to a NEW completion, so we don't
                    // re-fire every 300ms while status stays "done:".
                    if (picker.downloadedPath !== newPath) {
                        picker.downloadStatus = "done"
                        picker.downloadedPath = newPath
                        // v0.1.474: auto-activate if nothing is active
                        // yet — the "I have no model, give me a fast
                        // one" case (the whole point of quick-picks).
                        // If a model is ALREADY active, leave it and
                        // let the user tap "Use this model" so we don't
                        // swap their chosen model out from under them.
                        var hasActive = backend && backend.llama_model_path
                            && backend.llama_model_path.length > 0
                        if (!hasActive && backend && backend.set_llama_model_path) {
                            backend.set_llama_model_path(newPath)
                            picker.loadLocalModels()
                            picker.downloadStatus = "active"
                        }
                    }
                } else if (s.indexOf("error:") === 0) {
                    picker.downloadStatus = "error: " + s.substring(6)
                } else {
                    picker.downloadStatus = s
                }
            } catch (e) { /* ignore malformed poll */ }
        }
    }

    function reloadResults() {
        if (!backend || !backend.hf_results_json) return
        var raw = backend.hf_results_json()
        try {
            picker.resultsModel = JSON.parse(raw)
        } catch (e) {
            picker.resultsModel = []
        }
    }

    function loadLocalModels() {
        if (!backend || !backend.local_models_json) return
        var raw = backend.local_models_json()
        try {
            var list = JSON.parse(raw)
            localRepeater.model = list
            // v0.1.549: build the set of downloaded filenames so curated
            // rows can show "downloaded ✓" vs "available". Keyed by the
            // GGUF filename (local_models_json `name` == curated `filename`).
            var names = {}
            for (var i = 0; i < list.length; i++) names[list[i].name] = true
            picker.downloadedNames = names
        } catch (e) {
            localRepeater.model = []
            picker.downloadedNames = ({})
        }
    }

    // v0.1.473: curated fast-model quick-picks. Parsed once on load.
    function loadCuratedModels() {
        if (!backend || !backend.curated_models_json) return
        var raw = backend.curated_models_json()
        try {
            curatedRepeater.model = JSON.parse(raw)
        } catch (e) {
            curatedRepeater.model = []
        }
    }

    // v0.1.475: RAM-based recommendation for the setup assistant.
    // Sets recommendedIndex (into the curated list) + detectedRam so
    // the quick-pick section can highlight the best pick for this
    // machine. Speed-first: see hf_hub::recommend_curated.
    property int recommendedIndex: -1
    property int detectedRam: 0
    function loadRecommendation() {
        if (!backend || !backend.setup_recommendation_json) return
        try {
            var s = JSON.parse(backend.setup_recommendation_json())
            picker.recommendedIndex = (s.recommended_index !== undefined)
                ? s.recommended_index : -1
            picker.detectedRam = s.ram_gb || 0
        } catch (e) {
            picker.recommendedIndex = -1
        }
    }

    Component.onCompleted: {
        picker.loadLocalModels()
        picker.loadCuratedModels()
        picker.loadRecommendation()
    }

    Rectangle {
        anchors.fill: parent
        color: "#161616"
        radius: 8
        border.color: "#2a2a2a"; border.width: 1

        Column {
            anchors.fill: parent
            anchors.margins: 12
            spacing: 10

            Text {
                text: "Local model (llama.cpp · Vulkan)"
                color: "#e0e0e0"; font.pixelSize: 15; font.bold: true
            }

            // ── currently active model ──
            Text {
                width: parent.width
                text: {
                    if (!backend) return ""
                    var p = backend.llama_model_path
                    return p && p.length > 0
                        ? "Active: " + p.split("/").pop()
                        : "Active: none (using Claude/Ollama)"
                }
                color: "#8fbf8f"; font.pixelSize: 12; elide: Text.ElideMiddle
            }

            // ── download progress / status (v0.1.549: moved to the TOP,
            // next to the model controls. It was at the bottom of the
            // picker, which in settings rendered visually under the
            // voice-activation section — confusing. Status belongs by
            // the green model UI.) ──
            Column {
                width: parent.width; spacing: 4
                visible: picker.downloadStatus.length > 0
                Text {
                    width: parent.width
                    text: {
                        if (picker.downloadStatus === "downloading")
                            return "Downloading… " + picker.downloadPercent + "%"
                        if (picker.downloadStatus === "done")
                            return "Download complete."
                        if (picker.downloadStatus === "active")
                            return "✓ Downloaded and activated — now using this model."
                        return picker.downloadStatus
                    }
                    color: picker.downloadStatus.indexOf("error") === 0 ? "#d07a7a" : "#9acb9a"
                    font.pixelSize: 12; elide: Text.ElideRight
                }
                Rectangle {
                    visible: picker.downloadStatus === "downloading"
                    width: parent.width; height: 6; radius: 3; color: "#222"
                    Rectangle {
                        height: parent.height; radius: 3; color: "#4a8a4a"
                        width: parent.width * (picker.downloadPercent / 100.0)
                    }
                }
                Rectangle {
                    visible: picker.downloadStatus === "done" && picker.downloadedPath.length > 0
                    width: 140; height: 30; radius: 6; color: useMouse.containsMouse ? "#2f5f2f" : "#244a24"
                    Text {
                        anchors.centerIn: parent; text: "Use this model"
                        color: "#dfffdf"; font.pixelSize: 12
                    }
                    MouseArea {
                        id: useMouse
                        anchors.fill: parent; hoverEnabled: true
                        onClicked: {
                            if (backend && backend.set_llama_model_path) {
                                backend.set_llama_model_path(picker.downloadedPath)
                                picker.loadLocalModels()
                                picker.downloadStatus = ""
                            }
                        }
                    }
                }
            }

            // ── downloaded models (quick re-activate) ──
            Column {
                width: parent.width
                spacing: 4
                visible: localRepeater.count > 0
                Text { text: "Downloaded:"; color: "#999"; font.pixelSize: 11 }
                Repeater {
                    id: localRepeater
                    model: []
                    Rectangle {
                        // v0.1.549: highlight the currently-active model so
                        // selection is visible (was: no feedback on which
                        // downloaded model is in use).
                        property bool isActive: backend && backend.llama_model_path
                            && backend.llama_model_path === modelData.path
                        width: parent.width; height: 28
                        color: isActive ? "#1e3a1e"
                             : localMouse.containsMouse ? "#222" : "transparent"
                        radius: 4
                        border.color: isActive ? "#4a8a4a" : "transparent"
                        border.width: isActive ? 1 : 0
                        Row {
                            anchors.fill: parent; anchors.margins: 4; spacing: 8
                            Text {
                                text: (isActive ? "✓ " : "") + modelData.name
                                color: isActive ? "#9acb9a" : "#cfcfcf"
                                font.pixelSize: 11; width: parent.width * 0.7
                                elide: Text.ElideMiddle
                            }
                            Text {
                                text: modelData.size; color: "#777"; font.pixelSize: 11
                            }
                        }
                        MouseArea {
                            id: localMouse
                            anchors.fill: parent; hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                if (backend && backend.set_llama_model_path) {
                                    backend.set_llama_model_path(modelData.path)
                                    picker.loadLocalModels()
                                }
                            }
                        }
                    }
                }
            }

            // ── curated quick-picks (v0.1.473) ──
            // One-tap download of a known-good fast model, so the
            // user doesn't have to know what to search for. This is
            // the main lever for getting off the Ollama fallback.
            Column {
                width: parent.width
                spacing: 4
                visible: curatedRepeater.count > 0
                Text {
                    text: picker.detectedRam > 0
                        ? "Quick picks (★ = recommended for your " + picker.detectedRam + "GB machine):"
                        : "Quick picks (fastest first):"
                    color: "#999"; font.pixelSize: 11
                    width: parent.width; elide: Text.ElideRight
                }
                Repeater {
                    id: curatedRepeater
                    model: []
                    Rectangle {
                        id: curatedDelegate
                        width: parent.width
                        height: 44
                        radius: 6
                        // v0.1.475: highlight the RAM-recommended pick.
                        property bool isRecommended: index === picker.recommendedIndex
                        // v0.1.549: is this curated model already on disk?
                        property bool isDownloaded: picker.downloadedNames[modelData.filename] === true
                        color: curatedMouse.containsMouse ? "#1e2a30" : "#161616"
                        border.color: curatedDelegate.isDownloaded ? "#4a8a4a"
                                    : curatedDelegate.isRecommended ? "#4a8a4a" : "#2a3a42"
                        border.width: (curatedDelegate.isDownloaded || curatedDelegate.isRecommended) ? 2 : 1
                        Row {
                            anchors.fill: parent
                            anchors.margins: 8
                            spacing: 8
                            Column {
                                width: parent.width - sizeLabel.width - badgeCol.width - 16
                                spacing: 1
                                Row {
                                    spacing: 6
                                    width: parent.width
                                    Text {
                                        text: modelData.label
                                        color: "#dfe6ea"; font.pixelSize: 12
                                        font.bold: true
                                        elide: Text.ElideRight
                                    }
                                    Text {
                                        visible: curatedDelegate.isRecommended
                                        text: "★ Recommended"
                                        color: "#8fcf8f"; font.pixelSize: 10
                                        font.bold: true
                                    }
                                }
                                Text {
                                    text: modelData.blurb
                                    color: "#8a8f93"; font.pixelSize: 10
                                    width: parent.width; elide: Text.ElideRight
                                }
                            }
                            // v0.1.549: downloaded-vs-available badge.
                            Column {
                                id: badgeCol
                                anchors.verticalCenter: parent.verticalCenter
                                width: 86
                                Rectangle {
                                    width: parent.width; height: 20; radius: 10
                                    color: curatedDelegate.isDownloaded ? "#244a24" : "#2a3a42"
                                    Text {
                                        anchors.centerIn: parent
                                        text: curatedDelegate.isDownloaded ? "✓ Downloaded" : "Download"
                                        color: curatedDelegate.isDownloaded ? "#9acb9a" : "#9ec7e0"
                                        font.pixelSize: 10; font.bold: true
                                    }
                                }
                            }
                            Text {
                                id: sizeLabel
                                anchors.verticalCenter: parent.verticalCenter
                                text: "~" + modelData.gb + " GB"
                                color: "#7aa7c7"; font.pixelSize: 11
                            }
                        }
                        MouseArea {
                            id: curatedMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                // v0.1.549: if already downloaded, tapping
                                // ACTIVATES it instead of re-downloading.
                                if (curatedDelegate.isDownloaded) {
                                    // find the local path for this filename
                                    var lm = localRepeater.model
                                    for (var i = 0; i < lm.length; i++) {
                                        if (lm[i].name === modelData.filename
                                            && backend && backend.set_llama_model_path) {
                                            backend.set_llama_model_path(lm[i].path)
                                            picker.downloadStatus = "active"
                                            break
                                        }
                                    }
                                    return
                                }
                                // Same flow as tapping a file in the
                                // search results: kick off the download
                                // by (repo_id, filename). The existing
                                // progress poll + activation path then
                                // takes over.
                                if (backend && backend.download_model) {
                                    picker.downloadStatus = "downloading"
                                    picker.downloadPercent = 0
                                    backend.download_model(
                                        modelData.repo_id, modelData.filename)
                                }
                            }
                        }
                    }
                }
            }

            // ── search box ──
            Rectangle {
                width: parent.width; height: 34
                color: "#0e0e0e"; radius: 6
                border.color: searchInput.activeFocus ? "#4a7a9a" : "#2a2a2a"
                TextInput {
                    id: searchInput
                    anchors.fill: parent; anchors.margins: 8
                    color: "#e0e0e0"; font.pixelSize: 13
                    clip: true; verticalAlignment: TextInput.AlignVCenter
                    onAccepted: {
                        if (text.length > 0 && backend && backend.hf_search) {
                            picker.showingFiles = false
                            picker.selectedRepo = ""
                            backend.hf_search(text)
                        }
                    }
                    Text {
                        anchors.fill: parent
                        verticalAlignment: Text.AlignVCenter
                        visible: searchInput.text.length === 0
                        text: "Search HuggingFace GGUF models (Enter)…"
                        color: "#555"; font.pixelSize: 13
                    }
                }
            }

            // ── back button when viewing a repo's files ──
            Text {
                visible: picker.showingFiles
                text: "‹ back to results"
                color: "#7aa7c7"; font.pixelSize: 12
                MouseArea {
                    anchors.fill: parent
                    onClicked: {
                        picker.showingFiles = false
                        if (backend && backend.hf_search && searchInput.text.length > 0) {
                            backend.hf_search(searchInput.text)
                        }
                    }
                }
            }

            // ── results / files list ──
            ListView {
                id: resultList
                width: parent.width
                height: 200
                clip: true
                model: picker.resultsModel
                delegate: Rectangle {
                    width: resultList.width; height: 40
                    color: rowMouse.containsMouse ? "#222" : "transparent"
                    radius: 4
                    Row {
                        anchors.fill: parent; anchors.margins: 6; spacing: 8
                        // recommended star (file view only)
                        Text {
                            visible: picker.showingFiles && modelData.recommended === true
                            text: "★"; color: "#d8b84a"; font.pixelSize: 13
                        }
                        Column {
                            width: parent.width * 0.95
                            Text {
                                text: picker.showingFiles ? modelData.filename : modelData.repo_id
                                color: "#dcdcdc"; font.pixelSize: 12
                                width: parent.width; elide: Text.ElideMiddle
                            }
                            Text {
                                text: picker.showingFiles
                                    ? modelData.quant
                                    : ("↓ " + modelData.downloads + "   ♥ " + modelData.likes)
                                color: "#888"; font.pixelSize: 10
                            }
                        }
                    }
                    MouseArea {
                        id: rowMouse
                        anchors.fill: parent; hoverEnabled: true
                        onClicked: {
                            if (!picker.showingFiles) {
                                // tapped a repo → list its files
                                picker.selectedRepo = modelData.repo_id
                                picker.showingFiles = true
                                if (backend && backend.hf_list_files) {
                                    backend.hf_list_files(modelData.repo_id)
                                }
                            } else {
                                // tapped a file → download it
                                if (backend && backend.download_model) {
                                    picker.downloadStatus = "downloading"
                                    picker.downloadPercent = 0
                                    backend.download_model(picker.selectedRepo, modelData.filename)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
