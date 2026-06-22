import QtQuick 2.15
import QtQuick.Controls 2.15

/**
 * ConfirmDialog — confirmation overlay for destructive
 * actions (overwriting saved graphs, deleting workspaces,
 * etc). Filed v0.1.397 in the destructive-action-safety
 * arc.
 *
 * Behavior:
 *   1. Modal overlay covers parent (background dim, click
 *      outside cancels)
 *   2. Three buttons:
 *      - Save (the destructive action — primary)
 *      - Don't save yet (cancel, leave state pristine)
 *      - Discard changes (explicitly throw away unsaved)
 *   3. 10s timeout triggers the timeout action — DEFAULTS
 *      to "discard" per user spec ("do not apply changes")
 *   4. Border around the entire dialog animates from full
 *      width to zero as the timeout counts down, giving
 *      visible feedback of the remaining time
 *
 * API:
 *   title           - heading text
 *   bodyText        - explanation paragraph
 *   confirmLabel    - primary action button label ("Save")
 *   timeoutMs       - countdown in ms (default 10000)
 *   timeoutAction   - "discard" (default), "cancel", "confirm"
 *
 * Signals:
 *   confirmed - user pressed the primary action
 *   cancelled - user pressed cancel ("don't apply yet")
 *   discarded - user pressed discard
 *   timedOut  - timeout elapsed; the appropriate action
 *               (confirmed/cancelled/discarded based on
 *               timeoutAction) fires first, then this
 *
 * Usage example (in pseudocode — see body of main.qml for
 * the real wiring):
 *
 *   id: overwriteConfirm
 *   title: "Overwrite saved graph?"
 *   bodyText: "Replace your saved snapshot with the current state?"
 *   confirmLabel: "Replace"
 *   timeoutMs: 10000
 *   timeoutAction: "discard"
 *   onConfirmed: backend.save_graph_overwrite()
 *   onCancelled: ...leave state, can resume...
 *   onDiscarded: backend.discard_graph_changes()
 *
 *   ...elsewhere... overwriteConfirm.open()
 */
Item {
    id: root
    anchors.fill: parent
    visible: false
    z: 99999

    property string title: "Confirm action"
    property string bodyText: ""
    property string confirmLabel: "Save"
    property int timeoutMs: 10000
    property string timeoutAction: "discard"

    signal confirmed()
    signal cancelled()
    signal discarded()
    signal timedOut()

    // Internal: remaining time in ms (animated from
    // timeoutMs → 0 across timeoutMs). The border progress
    // bar reads from this.
    property real _remainingMs: 0

    function open() {
        _remainingMs = timeoutMs
        visible = true
        forceActiveFocus()
        _countdownAnim.restart()
    }
    function close() {
        _countdownAnim.stop()
        visible = false
    }

    // Block clicks underneath. Click on background = cancel
    // (the safe default — same as pressing "don't apply").
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        onClicked: {
            // Outside-click is the same as "don't save yet"
            // (cancel) — never destructive.
            _stopAndEmit("cancel")
        }
        // Dim background.
        Rectangle {
            anchors.fill: parent
            color: "#000"
            opacity: 0.55
        }
    }

    // The dialog card. Stops click propagation so clicking
    // inside doesn't dismiss.
    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 460)
        height: contentCol.height + 60
        color: "#1e1e22"
        border.color: "#3b82f6"
        border.width: 1
        radius: 12

        // Border progress indicator. Sits OUTSIDE the card,
        // hugging its edge. Width shrinks as time runs out.
        // We use a simple top-edge progress bar; less busy
        // than animating around four sides, and the meaning
        // (left=full → right=empty) reads instantly.
        Rectangle {
            id: progressBorder
            anchors.bottom: parent.top
            anchors.bottomMargin: 2
            anchors.left: parent.left
            height: 3
            radius: 1.5
            color: _remainingMs / Math.max(1, timeoutMs) > 0.33
                ? "#3b82f6"  // accent while plenty of time
                : "#ef4444"  // red when <33% left (urgency)
            width: parent.width *
                Math.max(0, Math.min(1, _remainingMs / Math.max(1, timeoutMs)))
            Behavior on color { ColorAnimation { duration: 200 } }
        }

        MouseArea {
            anchors.fill: parent
            // Just absorb clicks; specific buttons handle
            // their own.
        }

        Column {
            id: contentCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 22
            spacing: 14

            Text {
                width: parent.width
                text: root.title
                color: "#eee"
                font.pixelSize: 18
                font.bold: true
                wrapMode: Text.WordWrap
            }
            Text {
                width: parent.width
                text: root.bodyText
                color: "#bbb"
                font.pixelSize: 13
                wrapMode: Text.WordWrap
                visible: text.length > 0
            }
            Text {
                width: parent.width
                text: "Auto-dismiss in " +
                    Math.ceil(_remainingMs / 1000) + "s — " +
                    (timeoutAction === "discard"
                        ? "changes will be discarded"
                        : timeoutAction === "confirm"
                            ? "will save automatically"
                            : "will keep current state")
                color: "#888"
                font.pixelSize: 11
                font.italic: true
            }

            Row {
                spacing: 10
                anchors.right: parent.right

                // Discard button — secondary destructive,
                // colored red so it's distinguishable from
                // the safe "don't save yet" path.
                Rectangle {
                    width: 110; height: 32; radius: 6
                    color: _discardMa.containsMouse
                        ? "#7f1d1d" : "#3f1818"
                    border.color: "#dc2626"
                    border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: "Discard"
                        color: "#fca5a5"
                        font.pixelSize: 12
                    }
                    MouseArea {
                        id: _discardMa
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: _stopAndEmit("discard")
                    }
                }

                // Cancel button — leaves state pristine.
                // Default focus for keyboard.
                Rectangle {
                    width: 130; height: 32; radius: 6
                    color: _cancelMa.containsMouse
                        ? "#2a2a30" : "#22222a"
                    border.color: "#666"
                    border.width: 1
                    Text {
                        anchors.centerIn: parent
                        text: "Don't save yet"
                        color: "#ddd"
                        font.pixelSize: 12
                    }
                    MouseArea {
                        id: _cancelMa
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: _stopAndEmit("cancel")
                    }
                }

                // Confirm button — the destructive action.
                // Blue accent to call attention.
                Rectangle {
                    width: Math.max(80, _confirmText.width + 24)
                    height: 32; radius: 6
                    color: _confirmMa.containsMouse
                        ? "#2563eb" : "#3b82f6"
                    Text {
                        id: _confirmText
                        anchors.centerIn: parent
                        text: root.confirmLabel
                        color: "#fff"
                        font.pixelSize: 12
                        font.bold: true
                    }
                    MouseArea {
                        id: _confirmMa
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: _stopAndEmit("confirm")
                    }
                }
            }
        }
    }

    // Countdown animation. Drives _remainingMs from
    // timeoutMs down to 0 in real time.
    NumberAnimation {
        id: _countdownAnim
        target: root
        property: "_remainingMs"
        from: timeoutMs
        to: 0
        duration: timeoutMs
        easing.type: Easing.Linear
        onStopped: {
            // Only fire timeout if we reached 0; if it was
            // manually stopped via a button click,
            // _remainingMs > 0 and we ignore.
            if (_remainingMs <= 0.5) {
                _stopAndEmit(timeoutAction === "confirm"
                    ? "confirm"
                    : timeoutAction === "cancel"
                        ? "cancel"
                        : "discard")
                timedOut()
            }
        }
    }

    // Keyboard: Enter = confirm, Escape = cancel.
    Keys.onPressed: function(event) {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            _stopAndEmit("confirm")
            event.accepted = true
        } else if (event.key === Qt.Key_Escape) {
            _stopAndEmit("cancel")
            event.accepted = true
        }
    }

    // Internal: stop timer + emit the named signal + close.
    function _stopAndEmit(which) {
        _countdownAnim.stop()
        close()
        if (which === "confirm") confirmed()
        else if (which === "discard") discarded()
        else cancelled()
    }
}
