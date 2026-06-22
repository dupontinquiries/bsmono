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
Text {
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
text: "Hero content that should be unwrapped"
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
text: "Real section"
font.pixelSize: 24
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
text: "Kept content"
}
}
}
}
