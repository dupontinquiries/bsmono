// Native branch manager — touch-first, fearless gestures.
//   • tap a branch                 → switch (checkout)
//   • long-press a branch          → cherry-pick its tip onto the current branch
//   • DRAG one branch onto another → rebase dragged branch onto the drop target
//   • tap the × on a row           → delete the branch
//   • "+ new branch"               → create at current HEAD
// All via the git-free native store through the Backend.
import QtQuick
import QtQuick.Controls

Item {
    id: root
    property var model: []
    property string note: ""
    property bool creating: false

    function reload() {
        var r = JSON.parse(backend.branches_json())
        model = r.ok ? (r.items || []) : []
    }
    function switchTo(name) {
        var r = JSON.parse(backend.branch_switch(name))
        note = r.ok ? ("switched to " + name) : ("error: " + r.error)
        reload()
    }
    function createBranch(name) {
        if (!name) return
        var r = JSON.parse(backend.branch_create(name))
        note = r.ok ? ("created " + name) : ("error: " + r.error)
        newField.text = ""
        creating = false
        reload()
    }
    function removeBranch(name) {
        var r = JSON.parse(backend.branch_delete(name))
        note = r.ok ? ("deleted " + name) : ("error: " + r.error)
        reload()
    }
    function cherryPickTip(oid, name) {
        var r = JSON.parse(backend.cherry_pick(oid))
        if (!r.ok) { note = "error: " + r.error; return }
        if (r.outcome === "committed") note = "cherry-picked " + name + " → " + r.oid.substring(0, 12)
        else if (r.outcome === "empty") note = name + " already applied"
        else note = "conflicts: " + (r.paths || []).join(", ")
        reload()
    }
    function rebaseOnto(branch, onto) {
        if (branch === onto) return
        var r = JSON.parse(backend.rebase(branch, onto))
        if (!r.ok) { note = "error: " + r.error; return }
        if (r.outcome === "rebased") note = "rebased " + branch + " onto " + onto + " (" + r.commits + " replayed)"
        else if (r.outcome === "uptodate") note = branch + " already up to date with " + onto
        else note = "rebase conflict at " + (r.commit || "").substring(0, 12)
        reload()
    }
    function mergeInto(branch) {
        var r = JSON.parse(backend.merge(branch))
        if (!r.ok) { note = "error: " + r.error; return }
        if (r.outcome === "merged") note = "merged " + branch + " → " + (r.commit || "").substring(0, 12)
        else if (r.outcome === "fastforward") note = "fast-forwarded to " + (r.new_tip || "").substring(0, 12)
        else if (r.outcome === "uptodate") note = "already up to date with " + branch
        else note = "merge conflict: " + (r.paths || []).join(", ")
        reload()
    }

    Component.onCompleted: reload()

    Label {
        id: title; text: "branches"; color: "#e6e9ef"; font.bold: true
        font.pixelSize: 16; x: 16; y: 12
    }
    Text {
        text: "tap=switch · hold=cherry-pick · drag=rebase · merge/× buttons"
        color: "#8a93a6"; font.pixelSize: 10
        anchors.left: title.left; anchors.top: title.bottom; anchors.topMargin: 2
    }

    ListView {
        id: list
        anchors.top: title.bottom; anchors.topMargin: 22
        anchors.left: parent.left; anchors.right: parent.right
        anchors.bottom: footer.top; anchors.margins: 12
        spacing: 8; clip: true
        model: root.model

        delegate: Item {
            id: rowItem
            width: ListView.view.width
            height: 54
            property string branchName: modelData.name
            property bool isHead: modelData.head

            // Drop target: another branch dragged onto this one rebases it here.
            DropArea {
                anchors.fill: parent
                onEntered: card.highlight = true
                onExited: card.highlight = false
                onDropped: function(drop) {
                    card.highlight = false
                    if (drop.source && drop.source.branchName)
                        root.rebaseOnto(drop.source.branchName, rowItem.branchName)
                }
            }

            Rectangle {
                id: card
                anchors.fill: parent
                radius: 8
                property bool highlight: false
                color: highlight ? "#214a4f"
                       : drag.active ? "#24304d"
                       : (tap.pressed ? "#1d2740" : "#171c2b")
                border.color: rowItem.isHead ? "#5ec8d8" : "transparent"
                border.width: rowItem.isHead ? 1 : 0

                // Make the card the drag payload; carries branchName to drops.
                Drag.active: drag.active
                Drag.source: rowItem
                Drag.hotSpot.x: width / 2
                Drag.hotSpot.y: height / 2

                Row {
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.left: parent.left; anchors.leftMargin: 14
                    spacing: 12
                    Rectangle {
                        width: 10; height: 10; radius: 5
                        color: rowItem.isHead ? "#5ec8d8" : "#3a4154"
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        Text {
                            text: rowItem.branchName; color: "#e6e9ef"; font.pixelSize: 14
                            font.bold: rowItem.isHead
                        }
                        Text {
                            text: (modelData.oid || "").substring(0, 12)
                            color: "#8a93a6"; font.pixelSize: 10; font.family: "monospace"
                        }
                    }
                }

                // Merge-into-current affordance (hidden for the current branch).
                Rectangle {
                    id: mergeBtn
                    visible: !rowItem.isHead
                    width: 56; height: 30; radius: 15
                    color: mrg.pressed ? "#214a4f" : "#2a3045"
                    anchors.right: delBtn.left; anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    Text { anchors.centerIn: parent; text: "merge"; color: "#9fe6ef"; font.pixelSize: 11 }
                    TapHandler { id: mrg; onTapped: root.mergeInto(rowItem.branchName) }
                }

                // Delete affordance (hidden for the current branch).
                Rectangle {
                    id: delBtn
                    visible: !rowItem.isHead
                    width: 30; height: 30; radius: 15
                    color: del.pressed ? "#7a2030" : "#2a3045"
                    anchors.right: parent.right; anchors.rightMargin: 10
                    anchors.verticalCenter: parent.verticalCenter
                    Text { anchors.centerIn: parent; text: "×"; color: "#ffd7de"; font.pixelSize: 16 }
                    TapHandler { id: del; onTapped: root.removeBranch(rowItem.branchName) }
                }

                DragHandler {
                    id: drag
                    target: card
                    onActiveChanged: if (!active) { card.x = 0; card.y = 0 }
                }
                TapHandler {
                    id: tap
                    longPressThreshold: 0.5
                    onTapped: if (!rowItem.isHead) root.switchTo(rowItem.branchName)
                    onLongPressed: if (!rowItem.isHead) root.cherryPickTip(modelData.oid, rowItem.branchName)
                }
            }
        }

        Label {
            anchors.centerIn: parent; visible: root.model.length === 0
            text: "no branches yet"; color: "#8a93a6"
        }
    }

    Column {
        id: footer
        anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
        anchors.margins: 12; spacing: 8
        Text { text: root.note; color: "#8a93a6"; font.pixelSize: 11; visible: root.note !== "" }
        Row {
            spacing: 8; visible: root.creating; width: parent.width
            TextField {
                id: newField; placeholderText: "new branch name"
                width: parent.width - createBtn.width - 8
                color: "#e6e9ef"
                background: Rectangle { radius: 8; color: "#0c0f1a"; border.color: "#3a4154" }
                onAccepted: root.createBranch(text.trim())
            }
            Button { id: createBtn; text: "create"; onClicked: root.createBranch(newField.text.trim()) }
        }
        Button {
            width: parent.width
            text: root.creating ? "cancel" : "+ new branch"
            onClicked: root.creating = !root.creating
        }
    }
}
