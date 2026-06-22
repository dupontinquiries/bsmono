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
spacing: 6
TextField {
objectName: "_field_0"
width: parent.width - 20
placeholderText: "Search"
color: "#e0e0e0"
font.pixelSize: 15
font.family: "sans-serif"
background: Rectangle {
color: "#363636"
radius: 4
border.color: "#555"
border.width: 1
}
}
Button {
text: "Go"
width: parent.width - 20
onClicked: {
var vals = []
for (var c = 0; c < parent.children.length; c++) {
var ch = parent.children[c]
if (ch.objectName && ch.objectName.indexOf("_field_") === 0) vals.push(ch.text || "")
}
if (backend) backend.submit_form_get("https://test.local/submit", "q", vals.slice(0, 1).join("\x1F"))
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
CheckBox {
objectName: "_chk_"
text: "agree"
enabled: true
font.pixelSize: 15
}

Text { text: "I agree"
color: "#d4d0c8"
font.pixelSize: 15
font.family: "sans-serif"
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
spacing: 2
Text {
text: "select"
color: "#d4d0c8"
font.pixelSize: 13
}
ComboBox {
width: Math.min(300, parent.width)
model: ["One", "Two"]
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
color: "#d4d0c8"
text: "default text"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
}
Text {
color: "#d4d0c8"
text: "default text"
font.pixelSize: 15
font.family: "sans-serif"
wrapMode: Text.WrapAtWordBoundaryOrAnywhere
width: parent.width
}
}
}
}
