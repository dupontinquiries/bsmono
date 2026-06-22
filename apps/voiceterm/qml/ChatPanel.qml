/*
 * ChatPanel — a conversational UI over the local model (or cloud).
 *
 * Mirrors SetupWizard's proven Item-overlay modal pattern (root Item,
 * anchors.fill parent, visible toggled by open()/close(), dim scrim +
 * centered card). DO NOT root this at Popup — v0.1.544 established that
 * Popup-rooted component files don't instantiate in this qmetaobject/Qt
 * setup.
 *
 * Backend wiring (all confirmed present, v0.1.552):
 *   chat_send_async(provider, message)  — fire; appends user turn,
 *                                          runs completion on a worker.
 *   chat_poll_result() -> QString       — poll on a Timer; non-empty
 *                                          means the reply landed.
 *   chat_messages() -> QString          — "M\x1F<role>\x1F<content>
 *                                          \x1F<ts>\n" per line.
 *   chat_busy (property)                — true while a reply is pending.
 *   chat_new(provider) / chat_has_claude() / chat_has_openai()
 *
 * backend is injected as `backend: root.ctxBackend` (NOT `backend:
 * backend` — that self-shadows to null, v0.1.546).
 */
import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

Item {
    id: chatPanel
    anchors.fill: parent
    visible: false
    z: 99998

    property var backend: null

    // Parsed message rows: [{role, content}]. Rebuilt from
    // chat_messages() after each send + on each poll tick.
    property var messages: []
    // Which provider to send as. "llama" routes to the local model when
    // one is active (chat_send picks local automatically); the string
    // is mostly a label for new-conversation bookkeeping.
    property string provider: "llama"

    function _clog(m) {
        if (backend && backend.log_message) backend.log_message("[chat-qml] " + m)
        else console.log("[chat-qml] " + m)
    }

    Component.onCompleted: _clog("ChatPanel instantiated; backend=" + (backend ? "present" : "NULL"))

    function open() {
        _clog("open() called")
        reload()
        chatPanel.visible = true
        inputField.forceActiveFocus()
    }
    function close() { chatPanel.visible = false }

    // Parse chat_messages() into the `messages` array.
    function reload() {
        if (!backend || !backend.chat_messages) { messages = []; return }
        var raw = backend.chat_messages()
        var US = String.fromCharCode(31)   // \x1F unit separator
        var rows = []
        var lines = raw.split("\n")
        for (var i = 0; i < lines.length; i++) {
            var ln = lines[i]
            if (ln.length === 0) continue
            // Format: M<US><role><US><content><US><ts>
            var parts = ln.split(US)
            if (parts.length >= 3 && parts[0] === "M") {
                rows.push({ role: parts[1], content: parts[2] })
            }
        }
        messages = rows
        _clog("reload: " + rows.length + " message(s)")
    }

    function send() {
        var t = inputField.text
        if (t.length === 0) return
        if (!backend || !backend.chat_send_async) {
            _clog("send: backend or chat_send_async missing")
            return
        }
        _clog("send: dispatching (" + t.length + " chars)")
        backend.chat_send_async(chatPanel.provider, t)
        inputField.text = ""
        // Show the user's turn immediately (the backend appended it
        // synchronously inside chat_send_async).
        reload()
    }

    // Poll for the async reply while busy.
    Timer {
        id: pollTimer
        interval: 250
        repeat: true
        running: chatPanel.visible && backend && backend.chat_busy
        onTriggered: {
            if (!backend || !backend.chat_poll_result) return
            var r = backend.chat_poll_result()
            if (r.length > 0) {
                chatPanel.reload()   // reply was appended to history
            }
        }
    }

    // ── dim scrim (click outside closes) ──
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        onClicked: chatPanel.close()
        Rectangle { anchors.fill: parent; color: "#000"; opacity: 0.55 }
    }

    // ── chat card ──
    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 640)
        height: Math.min(parent.height - 96, 560)
        color: Qt.rgba(0.09, 0.11, 0.13, 0.99)
        radius: 16
        border.color: Qt.rgba(0.40, 0.55, 0.70, 0.45)
        border.width: 1
        MouseArea { anchors.fill: parent /* swallow clicks */ }

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 14
            spacing: 10

            // header
            RowLayout {
                Layout.fillWidth: true
                Text {
                    text: "\uD83D\uDCAC Chat"
                    color: "#e8f0ff"; font.pixelSize: 16; font.bold: true
                }
                Item { Layout.fillWidth: true }
                Text {
                    visible: backend && backend.chat_busy
                    text: "thinking\u2026"
                    color: "#8fb4d4"; font.pixelSize: 12
                }
                Button {
                    text: "New"
                    onClicked: {
                        if (backend && backend.chat_new) backend.chat_new(chatPanel.provider)
                        chatPanel.reload()
                    }
                }
                Button {
                    text: "\u2715"
                    onClicked: chatPanel.close()
                }
            }

            // message list
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                color: Qt.rgba(0.06, 0.07, 0.09, 0.9)
                radius: 8
                border.color: "#1e2a36"; border.width: 1

                ListView {
                    id: msgList
                    anchors.fill: parent
                    anchors.margins: 8
                    clip: true
                    spacing: 8
                    model: chatPanel.messages
                    // auto-scroll to the newest message
                    onCountChanged: positionViewAtEnd()

                    delegate: Row {
                        width: msgList.width
                        layoutDirection: modelData.role === "user"
                            ? Qt.RightToLeft : Qt.LeftToRight
                        Rectangle {
                            width: Math.min(msgList.width * 0.8, bubbleText.implicitWidth + 20)
                            height: bubbleText.implicitHeight + 14
                            radius: 10
                            color: modelData.role === "user"
                                ? Qt.rgba(0.18, 0.34, 0.52, 0.9)
                                : Qt.rgba(0.16, 0.20, 0.24, 0.9)
                            Text {
                                id: bubbleText
                                anchors.fill: parent
                                anchors.margins: 7
                                text: modelData.content
                                color: "#eaf2fb"
                                font.pixelSize: 13
                                wrapMode: Text.WordWrap
                            }
                        }
                    }
                }

                // empty-state hint
                Text {
                    anchors.centerIn: parent
                    visible: chatPanel.messages.length === 0
                    text: "Ask the local model anything\u2026"
                    color: "#5a6a78"; font.pixelSize: 13
                }
            }

            // input row
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Rectangle {
                    Layout.fillWidth: true
                    height: 38
                    color: "#0e1216"; radius: 8
                    border.color: inputField.activeFocus ? "#4a7a9a" : "#283642"
                    border.width: 1
                    TextInput {
                        id: inputField
                        anchors.fill: parent
                        anchors.margins: 9
                        color: "#e8f0ff"; font.pixelSize: 13
                        clip: true
                        verticalAlignment: TextInput.AlignVCenter
                        onAccepted: chatPanel.send()
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            visible: inputField.text.length === 0
                            text: "Message\u2026"
                            color: "#4a5a68"; font.pixelSize: 13
                        }
                    }
                }
                Button {
                    text: "Send"
                    enabled: inputField.text.length > 0 && !(backend && backend.chat_busy)
                    onClicked: chatPanel.send()
                }
            }
        }
    }
}
