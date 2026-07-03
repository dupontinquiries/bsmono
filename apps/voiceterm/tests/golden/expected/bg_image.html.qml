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
Image {
source: _sourceArmed ? "https://test.local/hero/large.jpg" : ""
fillMode: Image.PreserveAspectFit
asynchronous: true
cache: true
width: Math.min(sourceSize.width, parent.width)
height: sourceSize.width > 0 ? Math.min(sourceSize.height * (width / sourceSize.width), _root._vpHeight * 0.7) : _root._vpHeight * 0.3
sourceSize.width: Math.min(1600, parent.width * 2)
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
source: _sourceArmed ? "https://test.local/banner.png" : ""
fillMode: Image.PreserveAspectFit
asynchronous: true
cache: true
width: Math.min(sourceSize.width, parent.width)
height: sourceSize.width > 0 ? Math.min(sourceSize.height * (width / sourceSize.width), _root._vpHeight * 0.7) : _root._vpHeight * 0.3
sourceSize.width: Math.min(1600, parent.width * 2)
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
source: _sourceArmed ? "https://cdn.test/photo.webp" : ""
fillMode: Image.PreserveAspectFit
asynchronous: true
cache: true
width: Math.min(sourceSize.width, parent.width)
height: sourceSize.width > 0 ? Math.min(sourceSize.height * (width / sourceSize.width), _root._vpHeight * 0.7) : _root._vpHeight * 0.3
sourceSize.width: Math.min(1600, parent.width * 2)
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
Item {
id: _pwrap_0
width: parent.width
property string _pid: "p_0"
property string _contentType: "prose"
property bool _selected: (_root && _root._selectedParagraphId === _pid) === true
property string _rawContent: ""
property bool _hovered: false
property bool _inViewport: !_root || _root._vpTop === undefined ? true : (y + height > _root._vpTop && y < _root._vpBot)
height: _pinner_0.height
Rectangle {
id: _psel_0
visible: _pwrap_0._selected
anchors.top: parent.top
anchors.bottom: parent.bottom
x: _pwrap_0._hovered ? 2 : 0
Behavior on x { NumberAnimation { duration: 300; easing.type: Easing.InOutQuad } }
width: 3
radius: 1
color: "#ffaa44"
SequentialAnimation on opacity {
running: _pwrap_0._selected && _pwrap_0._inViewport
loops: Animation.Infinite
NumberAnimation { from: 0.7; to: 1.0; duration: 1500; easing.type: Easing.InOutQuad }
NumberAnimation { from: 1.0; to: 0.7; duration: 1500; easing.type: Easing.InOutQuad }
}
}
Item {
id: _pinner_0
width: parent.width - 10
height: childrenRect.height
anchors.left: parent.left
anchors.leftMargin: 10
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "Caption text under a hero — not a leaf, renders text only."
}
}
Rectangle {
id: _phover_0
anchors.bottom: parent.bottom
height: 2
width: 60
color: "#5599dd"
opacity: _pwrap_0._hovered ? 0.5 : 0.0
Behavior on opacity { NumberAnimation { duration: 300; easing.type: Easing.OutQuad } }
NumberAnimation on x {
from: -60
to: parent.width
duration: 2400
loops: Animation.Infinite
running: _pwrap_0._hovered && _pwrap_0._inViewport
}
}
MouseArea {
anchors.fill: parent
hoverEnabled: true
cursorShape: Qt.PointingHandCursor
propagateComposedEvents: true
onEntered: parent._hovered = true
onExited: parent._hovered = false
onClicked: {
_root._selectedParagraphId = parent._pid
if (backend) backend.paragraph_selected(parent._pid, "Caption text under a hero — not a leaf, renders text only.", parent._contentType, parent._rawContent)
}
}
}
}
}
}
