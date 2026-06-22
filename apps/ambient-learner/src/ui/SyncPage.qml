import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import "."
import "Theme.js" as Theme

Item {
    id: root
    property bool isMobile: true

    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            width: parent.width
            spacing: Theme.spacingDefault

            SectionHeader {
                title: "SYNC"
                accentColor: Theme.sectionSync
            }

            Card {
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage

                Label {
                    text: "📍 Local database"
                    font.pixelSize: Theme.fontHeading
                    font.bold: true
                    color: Theme.textPrimary
                }

                GridLayout {
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 24
                    rowSpacing: 6

                    Label { text: "Calibrations:"; color: Theme.textDim; font.pixelSize: Theme.fontBody }
                    Label { text: backend.calibration_count; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Linear:"; color: Theme.textDim; font.pixelSize: Theme.fontBody }
                    Label { text: backend.linear_calibrations; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Neural:"; color: Theme.textDim; font.pixelSize: Theme.fontBody }
                    Label { text: backend.neural_calibrations; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }

                    Label { text: "Active model:"; color: Theme.textDim; font.pixelSize: Theme.fontBody }
                    Label { text: backend.model_type; color: Theme.textPrimary; font.pixelSize: Theme.fontBody }
                }
            }

            Button {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                Layout.preferredHeight: Theme.minTouchSize
                text: "📤 Export database"
                enabled: false
                onClicked: console.log("Export not yet wired up")
            }

            Button {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                Layout.preferredHeight: Theme.minTouchSize
                text: "📥 Import database"
                enabled: false
                onClicked: console.log("Import not yet wired up")
            }

            // Friendly note about the missing functionality
            Rectangle {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.paddingPage
                Layout.rightMargin: Theme.paddingPage
                Layout.preferredHeight: 90
                radius: Theme.radiusCard
                color: "#241a1a"

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 4

                    Label {
                        text: "ℹ️  Note"
                        font.pixelSize: Theme.fontCaption
                        color: "#ff9999"
                    }
                    Label {
                        text: "Export / import need a few more backend methods. " +
                              "The GTK UI has full sync support today."
                        font.pixelSize: Theme.fontCaption
                        color: Theme.textSecondary
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                }
            }

            Item { Layout.preferredHeight: Theme.paddingPage }
        }
    }
}
