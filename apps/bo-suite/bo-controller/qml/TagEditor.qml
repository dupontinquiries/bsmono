// qml/TagEditor.qml
//
// Tag manager for one track. Opened via `open(track)` with a track
// summary object (id + title). Shows two sections:
//   1. Tags attached to THIS track — tap to detach.
//   2. All other tags in the library — tap to attach.
//
// A small input row at the top lets the user create a new tag with
// an optional color. The dialog updates live as qtobj publishes new
// tag/track-tag JSON. Closes via the scrim (tap outside the sheet)
// or the explicit "Done" button.
//
// Data flow:
//   open(track) → qtobj.load_track_tags(track.id) → updates
//                 current_track_tags_json; qtobj.load_all_tags() →
//                 updates tags_json. Both arrive via signals;
//                 their JSON is parsed into trackTags + allTags.
//   create_tag    → qtobj.upsert_tag(name, color) → tags_json refreshes
//   attach        → qtobj.attach_tag(track_id, tag_id) → both refresh
//   detach        → qtobj.detach_tag(track_id, tag_id) → both refresh

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    anchors.fill: parent
    visible: opacity > 0
    opacity: 0
    Behavior on opacity { NumberAnimation { duration: Bo.quick } }
    z: 200

    property var track: null
    property color accent: "#C8A04B"

    // Parsed JSON arrays for the two sections.
    property var trackTags: []
    property var allTags: []

    // Palette of preset colors offered when creating a new tag.
    // Mirrors B&O's understated palette: bronze, teal, mauve, sage,
    // dusk, ochre, slate, copper. Each is rendered as a swatch in
    // the create form; tapping selects it.
    readonly property var palette: [
        "#C8A04B", "#5C8A8A", "#A88090", "#8FA88F",
        "#6A7A92", "#B58853", "#7C8088", "#A6735C"
    ]
    property string pendingColor: palette[0]

    function open(t) {
        track = t
        if (t && t.id !== undefined) {
            qtobj.load_track_tags(t.id)
            qtobj.load_all_tags()
        }
        nameInput.text = ""
        opacity = 1
    }
    function close() { opacity = 0; track = null }

    // Watch for fresh JSON. We compare against our current track to
    // ignore stray TrackTags results for OTHER tracks.
    Connections {
        target: qtobj
        function onCurrent_track_tags_changed() {
            if (root.track && qtobj.current_track_tags_id === root.track.id) {
                try { root.trackTags = JSON.parse(qtobj.current_track_tags_json || "[]") }
                catch (e) { root.trackTags = [] }
            }
        }
        function onTags_changed() {
            try { root.allTags = JSON.parse(qtobj.tags_json || "[]") }
            catch (e) { root.allTags = [] }
        }
    }

    // Set difference: allTags \ trackTags (tags NOT attached to track).
    // Computed reactively from the two source lists.
    function availableTags() {
        var attached = {}
        for (var i = 0; i < trackTags.length; ++i) attached[trackTags[i].id] = true
        var out = []
        for (var j = 0; j < allTags.length; ++j) {
            if (!attached[allTags[j].id]) out.push(allTags[j])
        }
        return out
    }

    // Scrim
    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.55
        MouseArea { anchors.fill: parent; onClicked: root.close() }
    }

    // Sheet
    Rectangle {
        id: sheet
        width: Math.min(parent.width - 32, 380)
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        radius: 16
        color: Bo.surface
        border.color: Bo.hairline; border.width: 1
        height: contentColumn.implicitHeight + 32
        clip: true

        Column {
            id: contentColumn
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 16
            spacing: Bo.gapSM

            // Header
            Text {
                text: "TAGS"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
            }
            Text {
                text: (root.track && root.track.title) || ""
                color: Bo.bone
                font.pixelSize: Bo.bodySize
                font.weight: Font.Medium
                elide: Text.ElideRight
                width: parent.width
            }

            // Attached tags row — show as removable chips.
            Item { width: parent.width; height: 8 }
            Text {
                visible: root.trackTags.length > 0
                text: "Attached"
                color: Bo.silver
                font.pixelSize: 10
                font.letterSpacing: 1.0
            }
            Flow {
                visible: root.trackTags.length > 0
                width: parent.width
                spacing: 6
                Repeater {
                    model: root.trackTags
                    delegate: Rectangle {
                        height: 26; radius: 13
                        width: tagText.implicitWidth + 32
                        color: modelData.color || root.accent
                        Row {
                            anchors.centerIn: parent
                            spacing: 6
                            Text {
                                id: tagText
                                text: modelData.name
                                color: Bo.graphite
                                font.pixelSize: 11
                                font.weight: Font.DemiBold
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            Text {
                                text: "×"; color: Bo.graphite
                                font.pixelSize: 14
                                anchors.verticalCenter: parent.verticalCenter
                            }
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: qtobj.detach_tag(root.track.id, modelData.id)
                        }
                    }
                }
            }
            Text {
                visible: root.trackTags.length === 0
                text: "No tags yet — add one below or pick from existing"
                color: Bo.slate
                font.pixelSize: Bo.captionSize
                width: parent.width
                wrapMode: Text.Wrap
            }

            // Available tags — tap to attach.
            Item { width: parent.width; height: 8 }
            Text {
                visible: availableTags().length > 0
                text: "Existing"
                color: Bo.silver
                font.pixelSize: 10
                font.letterSpacing: 1.0
            }
            Flow {
                visible: availableTags().length > 0
                width: parent.width
                spacing: 6
                Repeater {
                    model: availableTags()
                    delegate: Rectangle {
                        height: 26; radius: 13
                        width: addText.implicitWidth + 32
                        color: "transparent"
                        border.color: modelData.color || Bo.hairline
                        border.width: 1
                        Row {
                            anchors.centerIn: parent
                            spacing: 6
                            Text {
                                text: "+"; color: modelData.color || Bo.silver
                                font.pixelSize: 13
                                font.weight: Font.Bold
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            Text {
                                id: addText
                                text: modelData.name
                                color: Bo.bone
                                font.pixelSize: 11
                                anchors.verticalCenter: parent.verticalCenter
                            }
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: qtobj.attach_tag(root.track.id, modelData.id)
                        }
                    }
                }
            }

            // Create new tag.
            Item { width: parent.width; height: 8 }
            Text {
                text: "Create"
                color: Bo.silver
                font.pixelSize: 10
                font.letterSpacing: 1.0
            }
            Row {
                width: parent.width
                spacing: 6
                TextField {
                    id: nameInput
                    width: parent.width - 72
                    placeholderText: "Tag name (e.g. workout, mellow…)"
                    color: Bo.bone
                    font.pixelSize: Bo.captionSize
                    background: Rectangle {
                        color: Bo.graphite
                        radius: 6
                        border.color: nameInput.activeFocus ? root.accent : Bo.hairline
                        border.width: 1
                    }
                }
                Rectangle {
                    width: 66; height: 32; radius: 16
                    color: nameInput.text.length > 0 ? root.accent
                         : Bo.mix(Bo.surface, root.accent, 0.35)
                    Text {
                        anchors.centerIn: parent
                        text: "Create"; color: Bo.graphite
                        font.pixelSize: 10; font.weight: Font.DemiBold
                    }
                    MouseArea {
                        anchors.fill: parent
                        enabled: nameInput.text.length > 0
                        onClicked: {
                            // Create the tag, then attach it to the
                            // current track in one step. The reload
                            // signal will refresh both lists.
                            qtobj.upsert_tag(nameInput.text, root.pendingColor)
                            // Best-effort attach: we don't have the
                            // new tag's id yet (the worker assigns
                            // it). Re-trigger load + a tiny delay
                            // before looking up the new tag by name
                            // would be racy — easier to just leave
                            // the new tag in "Existing" for the user
                            // to attach with one tap.
                            nameInput.text = ""
                        }
                    }
                }
            }
            // Color picker.
            Row {
                width: parent.width
                spacing: 6
                Repeater {
                    model: root.palette
                    delegate: Rectangle {
                        width: 24; height: 24; radius: 12
                        color: modelData
                        border.color: modelData === root.pendingColor
                                    ? Bo.bone : Bo.hairline
                        border.width: modelData === root.pendingColor ? 2 : 1
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.pendingColor = modelData
                        }
                    }
                }
            }

            // Done button.
            Item { width: parent.width; height: 8 }
            Rectangle {
                anchors.right: parent.right
                width: 80; height: 32; radius: 16
                color: doneMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "Done"; color: Bo.bone
                    font.pixelSize: Bo.captionSize
                }
                MouseArea { id: doneMa; anchors.fill: parent; onClicked: root.close() }
            }
        }
    }
}
