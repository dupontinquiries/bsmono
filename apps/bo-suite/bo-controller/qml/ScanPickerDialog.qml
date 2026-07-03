// qml/ScanPickerDialog.qml
//
// Slide-in bottom sheet asking where to scan for music. Triggered when
// the user taps Scan from the Library page. Offers:
//   • A list of detected common folders (~/Music, ~/Documents/Music,
//     ~/Downloads) — only those that actually exist.
//   • A "scan home (everywhere reasonable)" option that walks $HOME but
//     skips noisy dirs (.git, node_modules, caches…) via the scanner's
//     filter_entry rules.
//   • A free-text field for a custom path.
//
// On Scan, sends JSON.stringify(selectedPaths) → qtobj.scan_with_roots(json).
// A native folder picker (QtQuick.Dialogs FolderDialog) lets the user
// browse to any location; the last-scanned root is remembered and used as
// the picker's starting folder + a pre-filled custom path.

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Dialogs
import "."

Item {
    id: root
    anchors.fill: parent
    visible: opacity > 0
    opacity: 0
    Behavior on opacity { NumberAnimation { duration: Bo.quick } }
    z: 200

    property color accent: "#C8A04B"
    property var suggestions: []
    // path → bool, mirroring checkbox state
    property var selected: ({})
    property string customPath: ""

    // Native OS folder picker. Opens at the last-scanned location (or the
    // default) and writes the chosen directory back into the custom-path
    // field. QtQuick.Dialogs FolderDialog is the Qt6-standard native picker.
    FolderDialog {
        id: folderDialog
        title: "Choose a music folder to scan"
        // currentFolder seeds the starting location; expects a file:// URL.
        currentFolder: {
            var p = qtobj.last_scan_root || ""
            if (p.length > 0) return "file://" + p
            return ""
        }
        onAccepted: {
            // selectedFolder is a file:// URL → strip the scheme for the
            // scanner, which takes plain filesystem paths.
            var u = selectedFolder.toString()
            root.customPath = u.replace(/^file:\/\//, "")
        }
    }

    function open() {
        // Pull fresh suggestions each open (HOME might be set lazily).
        var raw = qtobj.suggested_scan_roots()
        try { suggestions = JSON.parse(raw) } catch (e) { suggestions = [] }
        var sel = {}
        // Default: pre-select the first "Music"-named entry if present.
        for (var i = 0; i < suggestions.length; ++i) {
            var s = suggestions[i]
            if (i === 0 && s.label === "Music") sel[s.path] = true
        }
        selected = sel
        // Pre-fill the custom path with the last-scanned location so a
        // repeat scan is one tap, and so Browse opens somewhere useful.
        customPath = qtobj.last_scan_root || ""
        opacity = 1
    }
    function close() { opacity = 0 }

    function gatherSelectedPaths() {
        var out = []
        for (var i = 0; i < suggestions.length; ++i) {
            var p = suggestions[i].path
            if (selected[p]) out.push(p)
        }
        var c = customPath.trim()
        if (c.length > 0) out.push(c)
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
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        height: Math.min(parent.height * 0.82, sheetCol.implicitHeight + Bo.gapLG * 2)
        radius: 18
        color: Bo.surface
        border.color: Bo.hairline; border.width: 1

        // Eat clicks so the scrim doesn't fire while interacting.
        MouseArea { anchors.fill: parent; onClicked: {} }

        Column {
            id: sheetCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Bo.gapLG
            spacing: Bo.gapMD

            // Grab handle
            Rectangle {
                width: 36; height: 4; radius: 2
                anchors.horizontalCenter: parent.horizontalCenter
                color: Bo.bronzeDim
            }

            Text {
                text: "WHERE TO SCAN"
                color: Bo.silver
                font.pixelSize: Bo.microSize
                font.letterSpacing: Bo.labelTracking
                font.weight: Font.DemiBold
            }
            Text {
                width: parent.width
                text: "Pick one or more folders. \"Home\" walks your home directory "
                    + "but skips noisy dirs (.git, node_modules, caches…) so it stays "
                    + "reasonable. Scanning is non-destructive — only metadata is read."
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                wrapMode: Text.Wrap
            }

            // Suggestion rows
            Repeater {
                model: root.suggestions
                delegate: Rectangle {
                    width: parent.width
                    height: 50
                    radius: 8
                    property bool checked: root.selected[modelData.path] === true
                    color: checked ? Bo.mix(Bo.surface, root.accent, 0.14)
                                   : (rma.pressed ? Bo.surfaceHi : "transparent")
                    border.color: checked ? root.accent : Bo.hairline
                    border.width: 1
                    Behavior on color { ColorAnimation { duration: Bo.quick } }
                    Behavior on border.color { ColorAnimation { duration: Bo.quick } }

                    Row {
                        anchors.left: parent.left
                        anchors.leftMargin: Bo.gapSM
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: Bo.gapSM
                        // Checkbox pip
                        Rectangle {
                            width: 18; height: 18; radius: 4
                            anchors.verticalCenter: parent.verticalCenter
                            color: parent.parent.checked ? root.accent : "transparent"
                            border.color: parent.parent.checked ? root.accent : Bo.bronzeDim
                            border.width: 1
                            Text {
                                anchors.centerIn: parent
                                visible: parent.parent.parent.checked
                                text: "✓"
                                color: Bo.graphite
                                font.pixelSize: 12
                                font.weight: Font.Bold
                            }
                        }
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 1
                            Text {
                                text: modelData.label || modelData.path
                                color: Bo.bone
                                font.pixelSize: Bo.bodySize
                                font.weight: Font.Medium
                            }
                            Text {
                                text: modelData.path
                                color: Bo.slate
                                font.pixelSize: 10
                                font.family: "monospace"
                                elide: Text.ElideMiddle
                                width: sheet.width - 60
                            }
                        }
                    }
                    MouseArea {
                        id: rma
                        anchors.fill: parent
                        onClicked: {
                            // Toggle (force re-binding so QML notices the change).
                            var sel = root.selected
                            sel[modelData.path] = !sel[modelData.path]
                            root.selected = {}
                            root.selected = sel
                        }
                    }
                }
            }

            // Custom path
            Column {
                width: parent.width
                spacing: 4
                Text {
                    text: "OR CUSTOM PATH"
                    color: Bo.silver
                    font.pixelSize: 9
                    font.letterSpacing: Bo.labelTracking
                    font.weight: Font.DemiBold
                }
                Row {
                    width: parent.width
                    spacing: 6
                    Rectangle {
                        width: parent.width - browseBtn.width - 6
                        height: 38
                        radius: 8
                        color: Bo.ink
                        border.color: customInput.activeFocus ? root.accent : Bo.hairline
                        border.width: 1
                        Behavior on border.color { ColorAnimation { duration: Bo.quick } }
                        TextInput {
                            id: customInput
                            anchors.fill: parent
                            anchors.leftMargin: 10
                            anchors.rightMargin: 10
                            verticalAlignment: TextInput.AlignVCenter
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize
                            font.family: "monospace"
                            selectByMouse: true
                            clip: true
                            text: root.customPath
                            onTextChanged: root.customPath = text
                        }
                        Text {
                            anchors.left: parent.left; anchors.leftMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            visible: customInput.text.length === 0 && !customInput.activeFocus
                            text: "/path/to/music"
                            color: Bo.slate
                            font.pixelSize: Bo.bodySize
                            font.family: "monospace"
                        }
                    }
                    // Native folder picker trigger.
                    Rectangle {
                        id: browseBtn
                        width: browseText.implicitWidth + 22
                        height: 38
                        radius: 8
                        color: browseMa.pressed ? Bo.surfaceHi : Bo.graphite
                        border.color: Bo.hairline; border.width: 1
                        Text {
                            id: browseText
                            anchors.centerIn: parent
                            text: "Browse…"
                            color: root.accent
                            font.pixelSize: Bo.bodySize
                        }
                        MouseArea {
                            id: browseMa
                            anchors.fill: parent
                            onClicked: folderDialog.open()
                        }
                    }
                }
            }

            // Actions
            Row {
                spacing: Bo.gapSM
                anchors.right: parent.right
                Rectangle {
                    width: cancelText.implicitWidth + 32
                    height: 36; radius: 18
                    color: cma.pressed ? Bo.surfaceHi : "transparent"
                    border.color: Bo.hairline; border.width: 1
                    Text {
                        id: cancelText
                        anchors.centerIn: parent
                        text: "CANCEL"
                        color: Bo.bone
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1
                    }
                    MouseArea { id: cma; anchors.fill: parent; onClicked: root.close() }
                }
                Rectangle {
                    width: scanText.implicitWidth + 32
                    height: 36; radius: 18
                    property bool enabled: root.gatherSelectedPaths().length > 0
                    color: !enabled ? Bo.surfaceHi
                                    : (sma.pressed ? Bo.shade(root.accent) : root.accent)
                    Behavior on color { ColorAnimation { duration: Bo.quick } }
                    Text {
                        id: scanText
                        anchors.centerIn: parent
                        text: "SCAN"
                        color: parent.enabled ? Bo.graphite : Bo.slate
                        font.pixelSize: Bo.captionSize
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1
                    }
                    MouseArea {
                        id: sma
                        anchors.fill: parent
                        enabled: parent.enabled
                        onClicked: {
                            var paths = root.gatherSelectedPaths()
                            qtobj.scan_with_roots(JSON.stringify(paths))
                            root.close()
                        }
                    }
                }
            }
        }
    }
}
