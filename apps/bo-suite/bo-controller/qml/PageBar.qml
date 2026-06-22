// qml/PageBar.qml
//
// Named, tappable page navigation. Unlike PageDots (which just hints
// at position with dashes), PageBar shows page names and lets the user
// jump directly to a page. It scrolls horizontally when there are
// more pages than fit on screen, with the active page snapping
// into view.
//
// Used at the TOP of the canvas, complementing PageDots at the
// bottom. PageDots remains the swipe hint; PageBar is the named menu.

import QtQuick 2.15
import "."

Rectangle {
    id: root

    // `pages` is an array of { label, index } — the QML caller provides
    // a model that already filters for which pages are active under
    // the current product. Tapping a label sets `pager.currentIndex`.
    property var pages: []
    property int currentIndex: 0
    property color accent: "#C8A04B"

    signal pageTapped(int idx)

    height: 32
    radius: 16
    color: Bo.mix(Bo.surface, Bo.graphite, 0.5)
    border.color: Bo.hairline
    border.width: 1

    // ListView for horizontal scrolling — the page list can be longer
    // than the screen. snapMode keeps the active tab centered.
    ListView {
        id: lv
        anchors.fill: parent
        anchors.leftMargin: 6
        anchors.rightMargin: 6
        orientation: ListView.Horizontal
        clip: true
        spacing: 4
        model: root.pages
        currentIndex: -1   // we drive highlight via index, not selection
        snapMode: ListView.SnapToItem
        boundsBehavior: Flickable.StopAtBounds

        // Keep the active tab visible when the user swipes through
        // pages via the swipe-view (changing currentIndex from outside).
        Connections {
            target: root
            function onCurrentIndexChanged() {
                for (var i = 0; i < root.pages.length; ++i) {
                    if (root.pages[i].index === root.currentIndex) {
                        lv.positionViewAtIndex(i, ListView.Contain)
                        break
                    }
                }
            }
        }

        delegate: Rectangle {
            // modelData = { label, index }
            property bool isActive: modelData.index === root.currentIndex
            width: labelText.implicitWidth + 18
            height: 24
            anchors.verticalCenter: parent.verticalCenter
            radius: 12
            color: isActive ? root.accent
                 : (tapArea.pressed ? Bo.surfaceHi : "transparent")
            border.color: isActive ? root.accent : "transparent"
            border.width: 1

            Behavior on color { ColorAnimation { duration: Bo.quick } }

            Text {
                id: labelText
                anchors.centerIn: parent
                text: modelData.label
                color: parent.isActive ? Bo.graphite : Bo.silver
                font.pixelSize: 10
                font.weight: parent.isActive ? Font.DemiBold : Font.Medium
                font.letterSpacing: 0.8
            }

            MouseArea {
                id: tapArea
                anchors.fill: parent
                onClicked: root.pageTapped(modelData.index)
            }

            scale: tapArea.pressed ? 0.95 : 1.0
            Behavior on scale { NumberAnimation { duration: Bo.instant } }
        }
    }

    // Fade overlays at the left + right edges so cut-off labels look
    // intentional rather than clipped.
    Rectangle {
        anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
        width: 12
        radius: 16
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: Bo.mix(Bo.surface, Bo.graphite, 0.5) }
            GradientStop { position: 1.0; color: "transparent" }
        }
    }
    Rectangle {
        anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom
        width: 12
        radius: 16
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "transparent" }
            GradientStop { position: 1.0; color: Bo.mix(Bo.surface, Bo.graphite, 0.5) }
        }
    }
}
