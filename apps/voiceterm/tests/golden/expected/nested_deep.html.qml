Column {
width: parent.width
spacing: 4
Item {
width: parent.width
height: childrenRect.height
opacity: (y + height >= _root._vpTop && y <= _root._vpBot) ? 1 : 0
Behavior on opacity { NumberAnimation { duration: 80 } }
property bool _sourceArmed: false
onOpacityChanged: { if (opacity > 0 && !_sourceArmed) _sourceArmed = true }
Component.onCompleted: { if (opacity > 0) _sourceArmed = true }
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Text {
text: "Deeply nested"
font.pixelSize: 28
font.bold: true
color: "#d4d0c8"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
font.family: "sans-serif"
}
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "Paragraph at depth with ."
}
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Text {
text: "Card"
font.pixelSize: 20
font.bold: true
color: "#d4d0c8"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
font.family: "sans-serif"
}
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "Card text"
}
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "inline"
}
}
}
}
}
}
}
}
}
}
Item {
width: parent.width
height: childrenRect.height
opacity: (y + height >= _root._vpTop && y <= _root._vpBot) ? 1 : 0
Behavior on opacity { NumberAnimation { duration: 80 } }
property bool _sourceArmed: false
onOpacityChanged: { if (opacity > 0 && !_sourceArmed) _sourceArmed = true }
Component.onCompleted: { if (opacity > 0) _sourceArmed = true }
Column {
id: _d17
width: parent.width
spacing: 4
property bool expanded: false
Rectangle {
width: parent.width
height: 32
radius: 4
color: "#363636"
border.color: "#555"
border.width: 1
Text {
anchors.verticalCenter: parent.verticalCenter
x: 8
text: (_d17.expanded ? "\u25BC " : "\u25B6 ") + "Expand"
color: "#5599dd"
font.pixelSize: 15
font.bold: true
}
MouseArea {
anchors.fill: parent
cursorShape: Qt.PointingHandCursor
onClicked: { _d17.expanded = !_d17.expanded }
}
}
Column {
width: parent.width
spacing: 4
visible: _d17.expanded
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "Hidden detail content"
}
}
}
}
Item {
width: parent.width
height: childrenRect.height
opacity: (y + height >= _root._vpTop && y <= _root._vpBot) ? 1 : 0
Behavior on opacity { NumberAnimation { duration: 80 } }
property bool _sourceArmed: false
onOpacityChanged: { if (opacity > 0 && !_sourceArmed) _sourceArmed = true }
Component.onCompleted: { if (opacity > 0) _sourceArmed = true }
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Image {
source: _sourceArmed ? "https://test.local/f.png" : ""
fillMode: Image.PreserveAspectFit
asynchronous: true
cache: true
width: Math.min(sourceSize.width, parent.width)
height: sourceSize.width > 0 ? Math.min(sourceSize.height * (width / sourceSize.width), _root._vpHeight * 0.7) : _root._vpHeight * 0.3
sourceSize.width: Math.min(1600, parent.width * 2)
}
}
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "A figure caption"
}
}
}
}
