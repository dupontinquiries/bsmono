// A changed-file row. Swipe right → stage, swipe left → unstage; long-press
// opens the peek viewer. Mirrors the reference's gesture-driven rows.
import QtQuick
import QtQuick.Controls

SwipeDelegate {
    id: row
    height: 60
    property string fileName: ""
    property string xy: ""
    property bool staged: false
    property bool untracked: false
    signal stageRequested()
    signal unstageRequested()
    signal peekRequested()

    background: Rectangle { radius: 8; color: "#171c2b" }

    contentItem: Row {
        spacing: 10
        leftPadding: 12
        Column {
            anchors.verticalCenter: parent.verticalCenter
            Text { text: row.fileName; color: "#e6e9ef"; font.pixelSize: 14 }
            Text {
                text: row.untracked ? "untracked" : (row.staged ? "staged" : "modified")
                color: "#8a93a6"; font.pixelSize: 10
            }
        }
    }
    // status letter pinned right
    Text {
        anchors.right: parent.right; anchors.rightMargin: 14
        anchors.verticalCenter: parent.verticalCenter
        text: row.xy.trim(); font.bold: true; font.pixelSize: 14
        color: row.untracked ? "#8a93a6" : (row.staged ? "#56d364" : "#f0796b")
    }

    swipe.right: Rectangle {
        width: parent.width; height: parent.height; radius: 8; color: "#1d3a24"
        Text { anchors.verticalCenter: parent.verticalCenter; x: 16; text: "stage \u2192"; color: "#56d364" }
    }
    swipe.left: Rectangle {
        width: parent.width; height: parent.height; radius: 8; color: "#3a1d1d"
        Text { anchors.verticalCenter: parent.verticalCenter; anchors.right: parent.right
               anchors.rightMargin: 16; text: "\u2190 unstage"; color: "#f0796b" }
    }
    swipe.onCompleted: {
        if (swipe.position > 0) row.stageRequested()
        else if (swipe.position < 0) row.unstageRequested()
        swipe.close()
    }

    TapHandler {
        acceptedButtons: Qt.LeftButton
        onLongPressed: row.peekRequested()
    }
}
