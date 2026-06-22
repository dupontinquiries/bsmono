/*
 * ReplVoiceGlow — a quiet companion that appears when you drop into a
 * REPL/debugger (gdb, lldb, python, sqlite, redis) and invites you to
 * just TALK to it instead of typing commands.
 *
 * Design intent (per the "developers chasing a bug while staring at the
 * screen" brief): this is NOT a command reference dump. It's an
 * invitation. A developer mid-bug-hunt has their attention on the stack
 * trace and the code — so this:
 *   - GLOWS softly into the corner rather than slamming a modal over the
 *     terminal (never steals focus or covers the trace),
 *   - opens with conversational copy ("talk me through it") that frames
 *     voice as a dialogue with the debugger,
 *   - shows a few starter phrases as gentle suggestions, not a manual,
 *   - is one tap/Esc to dismiss and remembers dismissal per-context so
 *     it doesn't nag.
 *
 * Data source: backend.voice_commands_available() →
 *   {"context":"gdb"|..., "commands":[{"say":..,"does":..}]}
 * Empty context => no REPL focused => hidden.
 *
 * backend is injected as backend: root.ctxBackend (NOT backend: backend
 * — the self-shadow null trap, v0.1.546). Root is Item, never Popup
 * (v0.1.544).
 */
import QtQuick 2.15
import QtQuick.Controls 2.15

Item {
    id: glow
    // Anchored to the bottom-right of whatever contains it; does not
    // fill the screen — it's a corner companion, not an overlay.
    width: 320
    height: card.height
    visible: parentActive && opacity > 0.01
    opacity: 0
    z: 9000

    property var backend: null
    // Set by the host: false suppresses the glow regardless of context
    // (e.g. not on the terminal tab). Keeps the visibility logic in one
    // place instead of fighting a parent `visible:` binding override.
    property bool parentActive: true
    // v0.1.569: breathing-halo opacity, animated by the SequentialAnimation
    // in the card. Declared HERE on the root (glow) and before first use
    // (the halo Rectangle's border binds glow._halo) — the prior version
    // declared it at the bottom of the file AND animated it via `on _halo`
    // inside the card, which made QML look for _halo on the card and fail
    // the whole component load.
    property real _halo: 0.2

    // Current REPL context ("gdb", "python", …) and starter phrases.
    property string context: ""
    property var commands: []
    // Per-context dismissal so we don't re-nag for the same session.
    property var _dismissed: ({})
    // Friendly, conversational lead-in per context. Falls back to a
    // generic line for REPLs without bespoke copy.
    readonly property var _invites: ({
        "gdb":    "Chasing a bug? Talk me through it \u2014 just say what you want to do.",
        "lldb":   "Chasing a bug? Talk me through it \u2014 just say what you want to do.",
        "python": "REPL's open. Say it out loud and I'll type it.",
        "sqlite": "Poking at the database? Ask me in plain words.",
        "redis":  "Talk to Redis out loud \u2014 I'll translate."
    })
    readonly property string inviteText:
        _invites[context] !== undefined ? _invites[context]
        : "Voice is on \u2014 say a command and I'll run it."

    function _clog(m) {
        if (backend && backend.log_message) backend.log_message("[repl-glow] " + m)
    }

    Behavior on opacity { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }

    // Poll the foreground context. Cheap string call; the backend only
    // returns a non-empty context when a REPL prompt is actually focused.
    Timer {
        interval: 700
        repeat: true
        running: glow.backend !== null
        triggeredOnStart: true
        onTriggered: glow.refresh()
    }

    function refresh() {
        if (!backend || !backend.voice_commands_available) return
        var raw = backend.voice_commands_available()
        var data
        try { data = JSON.parse(raw) } catch (e) { return }
        var ctx = (data && data.context) ? data.context : ""
        if (ctx === context) return    // no change
        context = ctx
        commands = (data && data.commands) ? data.commands : []
        if (ctx === "" ) {
            // left the REPL — fade out
            glow.opacity = 0
            return
        }
        if (_dismissed[ctx]) {
            // user already waved this away for this context
            return
        }
        _clog("REPL detected: " + ctx + " (" + commands.length + " phrases)")
        glow.opacity = 1.0
    }

    function dismiss() {
        if (context !== "") {
            var d = _dismissed
            d[context] = true
            _dismissed = d
        }
        glow.opacity = 0
        _clog("dismissed " + context)
    }

    // Esc dismisses when visible.
    Keys.onEscapePressed: function(e) { if (glow.opacity > 0.5) { glow.dismiss(); e.accepted = true } }

    Rectangle {
        id: card
        width: parent.width
        height: contentCol.implicitHeight + 28
        radius: 14
        color: Qt.rgba(0.07, 0.09, 0.12, 0.97)
        border.width: 1
        border.color: Qt.rgba(0.45, 0.62, 0.85, 0.55)

        // Soft outer glow: a blurred sibling rectangle behind the card.
        // (No Qt5Compat GraphicalEffects dependency — we fake the glow
        // with a slightly larger, low-opacity, pulsing border halo.)
        Rectangle {
            anchors.fill: parent
            anchors.margins: -3
            radius: parent.radius + 3
            color: "transparent"
            border.width: 2
            border.color: Qt.rgba(0.40, 0.60, 0.95, glow._halo)
            z: -1
        }

        // gentle breathing halo to draw the eye without motion noise.
        // v0.1.569: target glow._halo EXPLICITLY. The old
        // `SequentialAnimation on _halo` was inside this card Rectangle,
        // so it tried to attach to a _halo on the CARD (which has none)
        // — that's the "Cannot assign to non-existent property _halo"
        // load failure. _halo lives on the root (glow); animate it by
        // target/property instead of the `on` attached form.
        SequentialAnimation {
            running: glow.visible
            loops: Animation.Infinite
            NumberAnimation { target: glow; property: "_halo"; to: 0.45; duration: 1400; easing.type: Easing.InOutSine }
            NumberAnimation { target: glow; property: "_halo"; to: 0.12; duration: 1400; easing.type: Easing.InOutSine }
        }

        Column {
            id: contentCol
            anchors.fill: parent
            anchors.margins: 14
            spacing: 9

            // header row: mic sigil + context label + close
            Item {
                width: parent.width
                height: 18
                Row {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    Text { text: "\uD83C\uDF99\uFE0F"; font.pixelSize: 15 }
                    Text {
                        text: "voice \u2192 " + glow.context
                        color: "#bcd6f5"; font.pixelSize: 13; font.bold: true
                        anchors.verticalCenter: parent.verticalCenter
                    }
                }
                Text {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: "\u2715"
                    color: "#6f8298"; font.pixelSize: 14
                    MouseArea { anchors.fill: parent; anchors.margins: -6
                        cursorShape: Qt.PointingHandCursor; onClicked: glow.dismiss() }
                }
            }

            // the conversational invitation
            Text {
                width: parent.width
                text: glow.inviteText
                color: "#e8f0fb"; font.pixelSize: 13
                wrapMode: Text.WordWrap
                lineHeight: 1.15
            }

            // a few starter phrases (max 4, the rest live in the full
            // "you can say…" help). Rendered as tappable chips that, when
            // clicked, speak-by-example: they show the phrase so the user
            // learns what to SAY, not buttons that do the action (the
            // point is to build the voice habit).
            Flow {
                width: parent.width
                spacing: 6
                Repeater {
                    model: glow.commands.slice(0, 4)
                    delegate: Rectangle {
                        radius: 11
                        height: 24
                        width: chip.implicitWidth + 18
                        color: Qt.rgba(0.16, 0.26, 0.40, 0.85)
                        border.width: 1
                        border.color: Qt.rgba(0.40, 0.58, 0.85, 0.4)
                        Text {
                            id: chip
                            anchors.centerIn: parent
                            text: "\u201C" + modelData.say + "\u201D"
                            color: "#cfe2fb"; font.pixelSize: 11
                        }
                    }
                }
            }

            // quiet footer affordance
            Text {
                width: parent.width
                text: "Tap the mic, or say \u201Cwhat can I say?\u201D for the full list."
                color: "#7d92ab"; font.pixelSize: 11
                wrapMode: Text.WordWrap
            }
        }
    }
}
