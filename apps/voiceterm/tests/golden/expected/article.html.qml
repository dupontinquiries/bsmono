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
Text {
width: parent.width
text: "Home"
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
if (backend) backend.link_menu("https://test.local/home", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/home", p.x, p.y)
}
}
}
Text {
width: parent.width
text: "About"
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
if (backend) backend.link_menu("https://test.local/about", p.x, p.y)
}
onPressAndHold: function(mouse) {
var p = mapToItem(null, mouse.x, mouse.y);
if (backend) backend.link_menu("https://test.local/about", p.x, p.y)
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
width: parent.width
spacing: 4
Text {
text: "Main Heading"
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
text: "A paragraph with and and text."
}
Text {
text: "Subsection"
font.pixelSize: 24
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
text: "• First item"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width - 16
x: 16
}
Text {
color: "#d4d0c8"
text: "• Second item"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width - 16
x: 16
}
}
Column {
width: parent.width
spacing: 3
Text {
color: "#d4d0c8"
text: "1. One"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width - 16
x: 16
}
Text {
color: "#d4d0c8"
text: "2. Two"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width - 16
x: 16
}
}
Row {
width: parent.width
spacing: 8
Rectangle {
width: 3
height: parent.height
color: "#5599dd"
radius: 1
}
Column {
width: parent.width - 11
spacing: 4
Text {
color: "#d4d0c8"
text: "A quoted passage."
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
}
}
}
Item {
id: _pwrap_0
width: parent.width
property string _pid: "p_0"
property string _contentType: "code"
property bool _selected: (_root && _root._selectedParagraphId === _pid) === true
height: _pinner_0.height
Rectangle {
id: _psel_0
visible: _pwrap_0._selected
anchors.top: parent.top
anchors.bottom: parent.bottom
x: 0
width: 3
radius: 1
color: "#ffaa44"
opacity: 0.9
}
Item {
id: _pinner_0
width: parent.width - 10
height: childrenRect.height
anchors.left: parent.left
anchors.leftMargin: 10
Rectangle {
width: parent.width
implicitHeight: childrenRect.height + 16
color: "#2a2a2a"
radius: 4
Text {
anchors.left: parent.left
anchors.right: parent.right
anchors.top: parent.top
anchors.margins: 8
text: "fn main() ( println!(\"hi\"); )"
color: "#d4d0c8"
font.pixelSize: 13
font.family: "monospace"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
}
}
}
MouseArea {
anchors.fill: parent
hoverEnabled: false
cursorShape: Qt.PointingHandCursor
propagateComposedEvents: true
onClicked: {
_root._selectedParagraphId = parent._pid
if (backend) backend.paragraph_selected(parent._pid, "fn main() { println!(\"hi\"); }", parent._contentType, "")
}
}
}
Column {
width: parent.width
spacing: 4
Image {
source: _sourceArmed ? "https://test.local/img/photo.jpg" : ""
fillMode: Image.PreserveAspectFit
asynchronous: true
cache: true
width: Math.min(640, parent.width)
height: Math.min(width * 0.7500, _root._vpHeight * 0.7)
sourceSize.width: Math.min(1600, parent.width * 2)
}
Text {
text: "A photo"
color: "#d4d0c8"
font.pixelSize: 12
font.italic: true
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
horizontalAlignment: Text.AlignHCenter
}
}
Item {
id: _tbl_0
width: parent.width
property string _tid: "t_0"
property bool _narrow: width > 0 && width < 600
height: _narrow ? _narrowCol_0.height : _wideCol_0.height
Rectangle {
id: _wideCol_0
visible: !_tbl_0._narrow
width: parent.width
color: "transparent"
border.color: "#555"
border.width: 1
radius: 4
height: _wideInner_0.implicitHeight + 2
ColumnLayout {
id: _wideInner_0
anchors.left: parent.left
anchors.right: parent.right
anchors.top: parent.top
anchors.margins: 1
spacing: 0
Rectangle {
Layout.fillWidth: true
color: "#363636"
height: _wideHeaderRow_0.height + 12
radius: 3
RowLayout {
id: _wideHeaderRow_0
anchors.left: parent.left
anchors.right: parent.right
anchors.verticalCenter: parent.verticalCenter
anchors.leftMargin: 8
anchors.rightMargin: 8
spacing: 8
Text {
text: "Col A"
color: "#d4d0c8"
font.pixelSize: 15
font.bold: true
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
Text {
text: "Col B"
color: "#d4d0c8"
font.pixelSize: 15
font.bold: true
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
}
}
Rectangle {
Layout.fillWidth: true
height: 2
color: "#555"
}
Rectangle {
Layout.fillWidth: true
color: "transparent"
height: _wideRow_0_0.height + 12
RowLayout {
id: _wideRow_0_0
anchors.left: parent.left
anchors.right: parent.right
anchors.verticalCenter: parent.verticalCenter
anchors.leftMargin: 8
anchors.rightMargin: 8
spacing: 8
Text {
text: "1"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
Text {
text: "2"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
}
}
Rectangle {
Layout.fillWidth: true
color: "#0affffff"
height: _wideRow_0_1.height + 12
RowLayout {
id: _wideRow_0_1
anchors.left: parent.left
anchors.right: parent.right
anchors.verticalCenter: parent.verticalCenter
anchors.leftMargin: 8
anchors.rightMargin: 8
spacing: 8
Text {
text: "3"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
Text {
text: "4"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
}
}
}
}
Column {
id: _narrowCol_0
visible: _tbl_0._narrow
width: parent.width
spacing: 8
Rectangle {
width: parent.width
height: _card_0_0.height + 16
border.color: "#555"
border.width: 1
radius: 4
color: "transparent"
Column {
id: _card_0_0
anchors.left: parent.left
anchors.right: parent.right
anchors.top: parent.top
anchors.margins: 8
spacing: 4
RowLayout {
width: parent.width
spacing: 6
Text {
text: "Col A:"
color: "#d4d0c8"
font.pixelSize: 15
font.bold: true
Layout.preferredWidth: 90
elide: Text.ElideRight
}
Text {
text: "1"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
}
RowLayout {
width: parent.width
spacing: 6
Text {
text: "Col B:"
color: "#d4d0c8"
font.pixelSize: 15
font.bold: true
Layout.preferredWidth: 90
elide: Text.ElideRight
}
Text {
text: "2"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
}
}
}
Rectangle {
width: parent.width
height: _card_0_1.height + 16
border.color: "#555"
border.width: 1
radius: 4
color: "transparent"
Column {
id: _card_0_1
anchors.left: parent.left
anchors.right: parent.right
anchors.top: parent.top
anchors.margins: 8
spacing: 4
RowLayout {
width: parent.width
spacing: 6
Text {
text: "Col A:"
color: "#d4d0c8"
font.pixelSize: 15
font.bold: true
Layout.preferredWidth: 90
elide: Text.ElideRight
}
Text {
text: "3"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
}
}
RowLayout {
width: parent.width
spacing: 6
Text {
text: "Col B:"
color: "#d4d0c8"
font.pixelSize: 15
font.bold: true
Layout.preferredWidth: 90
elide: Text.ElideRight
}
Text {
text: "4"
color: "#d4d0c8"
font.pixelSize: 15
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
Layout.preferredWidth: 1
Layout.fillWidth: true
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
Row {
width: parent.width
spacing: 6
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "Footer text"
}
}
}
}
