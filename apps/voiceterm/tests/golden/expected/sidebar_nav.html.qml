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
Item {
objectName: "anchor_main-nav"
width: 1
height: 0
}

Text {
width: parent.width
text: "A"
color: "#5599dd"
font.pixelSize: 15
font.family: "sans-serif"
font.underline: true
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
MouseArea {
anchors.fill: parent
cursorShape: Qt.PointingHandCursor
acceptedButtons: Qt.LeftButton | Qt.RightButton
pressAndHoldInterval: 350
onClicked: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/a", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/a", p.x, p.y)
}
}
}
Text {
width: parent.width
text: "B"
color: "#5599dd"
font.pixelSize: 15
font.family: "sans-serif"
font.underline: true
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
MouseArea {
anchors.fill: parent
cursorShape: Qt.PointingHandCursor
acceptedButtons: Qt.LeftButton | Qt.RightButton
pressAndHoldInterval: 350
onClicked: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/b", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/b", p.x, p.y)
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
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Text {
text: "Related"
font.pixelSize: 20
font.bold: true
color: "#d4d0c8"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
font.family: "sans-serif"
}
Column {
width: parent.width
spacing: 3
Text {
color: "#d4d0c8"
text: "• Link 1"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width - 16
x: 16
}
}
}
Column {
width: parent.width
spacing: 4
Column {
width: parent.width
spacing: 4
Text {
text: "Content"
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
text: "Main article body text here."
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
width: parent.width
spacing: 4
Text {
text: "Click card"
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
text: "card body"
}
}
}
}
