import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

Item {
    id: root
    property bool isMobile: true
    property var locations: []

    function refresh() {
        try { locations = JSON.parse(backend.get_locations()) }
        catch (e) { locations = [] }
    }

    Connections {
        target: backend
        function onLocation_countChanged() { root.refresh() }
        function onLocationChanged()       { root.refresh() }
    }
    Component.onCompleted: refresh()

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        SectionHeader {
            Layout.fillWidth: true
            title: "LOCATIONS"
            accentColor: Theme.sectionLocations
        }

        // ─── Current location ────────────────────────────────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            Layout.preferredHeight: 80
            radius: Theme.radiusCard
            color: "#1a3d2a"

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: Theme.paddingCard
                spacing: 4

                Label {
                    text: "Current location"
                    font.pixelSize: Theme.fontCaption
                    color: "#88d088"
                }
                Label {
                    text: backend.location || "—"
                    font.pixelSize: Theme.fontTitle
                    font.bold: true
                    color: Theme.textPrimary
                }
            }
        }

        Item { Layout.preferredHeight: 12 }

        // ─── List of saved locations ─────────────────────────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            radius: Theme.radiusCard
            color: Theme.surface

            ListView {
                id: locationList
                anchors.fill: parent
                anchors.margins: 4
                clip: true
                spacing: 4
                model: root.locations

                delegate: ItemDelegate {
                    width: ListView.view.width
                    height: 56

                    onClicked: backend.set_current_location(modelData.id)

                    background: Rectangle {
                        color: modelData.current
                               ? Theme.surfaceSelected
                               : "transparent"
                        radius: 8
                    }

                    contentItem: RowLayout {
                        spacing: 12

                        Label {
                            text: modelData.current ? "📍" : "📌"
                            font.pixelSize: 18
                            Layout.leftMargin: 8
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            Label {
                                text: modelData.name
                                font.pixelSize: Theme.fontBody
                                font.bold: modelData.current
                                color: Theme.textPrimary
                            }
                            Label {
                                visible: modelData.latitude !== null
                                         && modelData.longitude !== null
                                text: modelData.latitude !== null
                                      ? modelData.latitude.toFixed(3)
                                        + ", " + modelData.longitude.toFixed(3)
                                      : ""
                                font.pixelSize: Theme.fontCaption
                                color: Theme.textDim
                            }
                        }

                        ToolButton {
                            text: "✕"
                            visible: !modelData.current
                            onClicked: backend.remove_location(modelData.id)
                        }
                    }
                }

                Label {
                    anchors.centerIn: parent
                    visible: locationList.count === 0
                    text: "No saved locations.\nTap + to add one."
                    horizontalAlignment: Text.AlignHCenter
                    color: Theme.textDim
                }
            }
        }

        Button {
            Layout.fillWidth: true
            Layout.leftMargin: Theme.paddingPage
            Layout.rightMargin: Theme.paddingPage
            Layout.topMargin: 12
            Layout.bottomMargin: Theme.paddingPage
            Layout.preferredHeight: Theme.minTouchSize
            text: "➕ Add new location"
            onClicked: addLocationDialog.open()
        }
    }

    Dialog {
        id: addLocationDialog
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 360)
        modal: true
        title: "New location"

        contentItem: ColumnLayout {
            spacing: 12
            Label {
                text: "What do you call this place?"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Theme.textSecondary
            }
            TextField {
                id: nameField
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.minTouchSize
                placeholderText: "e.g. Office"
                onAccepted: addLocationDialog.accept()
            }
        }

        standardButtons: Dialog.Ok | Dialog.Cancel
        onAccepted: {
            var name = nameField.text.trim()
            if (name.length > 0) {
                backend.add_location(name)
                nameField.text = ""
            }
        }
        onRejected: nameField.text = ""
    }
}
