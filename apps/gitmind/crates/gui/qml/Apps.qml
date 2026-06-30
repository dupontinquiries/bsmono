// Version-pinned binaries — touch-first.
//   • tap a version       → make it current (atomic ~/.local/bin/<app> relink)
//   • "pin" on a version  → write a .desktop entry (GNOME app grid / favorites)
//   • × on a version      → remove that version
// Backed by the binpin module; binaries are content-addressed in the var store
// so unchanged builds across commits are stored once.
import QtQuick
import QtQuick.Controls

Item {
    id: root
    property var apps: []
    property string note: ""

    function reload() {
        var r = JSON.parse(backend.apps_json())
        apps = r.ok ? (r.apps || []) : []
    }
    function useVersion(name, label) {
        var r = JSON.parse(backend.app_use(name, label))
        note = r.ok ? ("now using " + name + "@" + label) : ("error: " + r.error)
        reload()
    }
    function pinVersion(name, label) {
        var r = JSON.parse(backend.app_desktop(name, label, false))
        note = r.ok ? ("pinned " + name + "@" + label + " → " + r.path) : ("error: " + r.error)
    }
    function removeVersion(name, label) {
        var r = JSON.parse(backend.app_remove(name, label))
        note = r.ok ? ("removed " + name + "@" + label) : ("error: " + r.error)
        reload()
    }
    function runApp(name) {
        var r = JSON.parse(backend.run_local(name, ""))
        note = r.ok ? ("▶ " + r.msg) : ("error: " + r.error)
    }

    Component.onCompleted: reload()

    Column {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 8

        Row {
            spacing: 8
            Label { text: "apps"; color: "#cdd6f4"; font.pixelSize: 18; font.bold: true }
            Button { text: "⟳"; onClicked: root.reload() }
        }
        Label { text: root.note; color: "#8a93a6"; visible: root.note.length > 0; wrapMode: Text.WrapAnywhere; width: parent.width }

        ListView {
            id: appList
            width: parent.width
            height: parent.height - y
            spacing: 12
            clip: true
            model: root.apps

            delegate: Column {
                width: appList.width
                spacing: 4

                Row {
                    width: appList.width; spacing: 8
                    Label {
                        text: modelData.name + "  (current: " + (modelData.current ? modelData.current : "—") + ")"
                        color: "#cdd6f4"; font.bold: true
                        width: parent.width - runBtn.width - 8; elide: Text.ElideRight
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Button {
                        id: runBtn; text: "▶ run"
                        enabled: modelData.current !== null && modelData.current !== undefined && modelData.current !== ""
                        onClicked: root.runApp(modelData.name)
                    }
                }

                Repeater {
                    model: modelData.versions
                    delegate: Rectangle {
                        id: verRow
                        width: appList.width
                        height: 48
                        radius: 8
                        color: modelData.current ? "#2a3350" : "#171c2b"
                        border.color: modelData.current ? "#7aa2f7" : "#2a3350"

                        // Resolve the parent app name for this version row.
                        function appName() {
                            for (var i = 0; i < root.apps.length; i++) {
                                var a = root.apps[i]
                                for (var j = 0; j < a.versions.length; j++)
                                    if (a.versions[j].label === modelData.label
                                        && a.versions[j].commit === modelData.commit)
                                        return a.name
                            }
                            return ""
                        }

                        Row {
                            anchors.fill: parent
                            anchors.margins: 8
                            spacing: 10

                            Column {
                                width: parent.width - pinBtn.width - rmBtn.width - 28
                                Label { text: "@" + modelData.label; color: "#cdd6f4"; font.bold: true }
                                Label {
                                    text: modelData.commit.substring(0, 8) + "  ·  " + modelData.target
                                    color: "#8a93a6"; font.pixelSize: 12
                                }
                            }

                            Button {
                                id: pinBtn
                                text: "pin"
                                anchors.verticalCenter: parent.verticalCenter
                                onClicked: root.pinVersion(verRow.appName(), modelData.label)
                            }
                            Button {
                                id: rmBtn
                                text: "×"
                                anchors.verticalCenter: parent.verticalCenter
                                onClicked: root.removeVersion(verRow.appName(), modelData.label)
                            }
                        }

                        TapHandler {
                            onTapped: root.useVersion(verRow.appName(), modelData.label)
                        }
                    }
                }
            }

            Label {
                anchors.centerIn: parent
                visible: root.apps.length === 0
                text: "no pinned apps yet\n(gin app install / build)"
                horizontalAlignment: Text.AlignHCenter
                color: "#8a93a6"
            }
        }
    }
}
