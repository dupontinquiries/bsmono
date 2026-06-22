import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

// Section title (the "LIVE", "MODEL", etc. floating titles).
//
// Usage inside a ColumnLayout:
//   SectionHeader { title: "LIVE"; accentColor: Theme.sectionLive }
//
// The header always fills its container's width — without that, a
// zero-width Rectangle plus `anchors.centerIn` would place the Label
// centered on x=0 and the leading characters would clip off the
// left edge of the screen.
Rectangle {
    id: root
    property string title: ""
    property color  accentColor: "#0099ff"

    color: "transparent"
    height: 56
    Layout.fillWidth: true
    // Fall back to parent width when not in a Layout (e.g. in an Item
    // with anchors). This keeps the Label visible even if a caller
    // forgets to set Layout.fillWidth on a wrapper.
    implicitWidth: parent ? parent.width : 200

    Label {
        anchors.centerIn: parent
        text: root.title
        font.pixelSize: 18
        font.bold: true
        font.letterSpacing: 4
        color: root.accentColor
        // Elide if the title is so long it would still clip on a tiny
        // viewport — better than being cut off.
        elide: Text.ElideRight
        // Cap the label's width so eliding has somewhere to apply.
        width: Math.min(parent.width - 32, implicitWidth)
        horizontalAlignment: Text.AlignHCenter
    }
}
