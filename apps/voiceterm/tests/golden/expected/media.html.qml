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
Item {
width: parent.width
height: 36
Rectangle {
anchors.fill: parent
color: "#363636"
radius: 4
border.color: "#555"
border.width: 1
}
Text {
anchors.centerIn: parent
text: "[embed] https://youtube.com/embed/abc"
color: "#5599dd"
font.pixelSize: 15
}
MouseArea {
anchors.fill: parent
cursorShape: Qt.PointingHandCursor
acceptedButtons: Qt.LeftButton | Qt.RightButton
pressAndHoldInterval: 350
onClicked: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://youtube.com/embed/abc", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://youtube.com/embed/abc", p.x, p.y)
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
Item {
width: parent.width
height: 36
Rectangle {
anchors.fill: parent
color: "#363636"
radius: 4
border.color: "#555"
border.width: 1
}
Text {
anchors.centerIn: parent
text: "▶ video https://test.local/movie.mp4"
color: "#5599dd"
font.pixelSize: 15
}
MouseArea {
anchors.fill: parent
cursorShape: Qt.PointingHandCursor
acceptedButtons: Qt.LeftButton | Qt.RightButton
pressAndHoldInterval: 350
onClicked: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/movie.mp4", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/movie.mp4", p.x, p.y)
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
Item {
width: parent.width
height: 36
Rectangle {
anchors.fill: parent
color: "#363636"
radius: 4
border.color: "#555"
border.width: 1
}
Text {
anchors.centerIn: parent
text: "♫ audio https://test.local/song.mp3"
color: "#5599dd"
font.pixelSize: 15
}
MouseArea {
anchors.fill: parent
cursorShape: Qt.PointingHandCursor
acceptedButtons: Qt.LeftButton | Qt.RightButton
pressAndHoldInterval: 350
onClicked: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/song.mp3", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/song.mp3", p.x, p.y)
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
Image {
source: _sourceArmed ? "https://test.local/small.webp" : ""
fillMode: Image.PreserveAspectFit
asynchronous: true
cache: true
width: Math.min(sourceSize.width, parent.width)
height: sourceSize.width > 0 ? Math.min(sourceSize.height * (width / sourceSize.width), _root._vpHeight * 0.7) : _root._vpHeight * 0.3
sourceSize.width: Math.min(1600, parent.width * 2)
}
Text {
text: "responsive"
color: "#d4d0c8"
font.pixelSize: 12
font.italic: true
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
horizontalAlignment: Text.AlignHCenter
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
Text {
color: "#d4d0c8"
text: "Enable JavaScript to view contentContinue without JS"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
}
}
}
