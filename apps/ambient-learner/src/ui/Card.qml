import QtQuick 2.15
import QtQuick.Layouts 1.15

// Rounded panel with consistent surface color and padding.
// Children are placed in a default ColumnLayout — no need to nest one yourself.
//
// Usage:
//   Card {
//       Label { text: "Hello" }
//       Label { text: "World" }
//   }
Rectangle {
    id: root
    property color cardColor: "#1a1a2e"
    property int  cardRadius: 12
    property int  contentMargin: 16
    property alias contentSpacing: layout.spacing
    default property alias contentChildren: layout.data

    color: cardColor
    radius: cardRadius
    Layout.fillWidth: true

    // Auto-size to content height
    implicitHeight: layout.implicitHeight + 2 * contentMargin

    ColumnLayout {
        id: layout
        anchors.fill: parent
        anchors.margins: root.contentMargin
        spacing: 8
    }
}
