// QML manual-test harness for rusty-board.
//
// Purpose: rusty-board is a GTK4 input method, but its CLIENTS on
// Plasma Mobile are QML apps. This harness exercises the keyboard's
// external behavior — focus events, ContentType hints, text commits,
// auto-cap, the swipe-up-to-dismiss gesture — from the client side.
//
// Run on-device:
//   qmlscene RustyBoardTestHarness.qml
//
// Each test case has a TextField the user focuses, an expectation
// description, and a manual "pass/fail" radio button the user
// updates after observing the keyboard's behavior. The harness
// covers behaviors that pure Rust tests can't verify — focus
// surfacing, ContentType-driven keyboard variation, and the
// wayland-protocol round-trip.
//
// This does NOT test internal keyboard logic — that's covered by
// the Rust tests in src/ui/keyboard_behaviors_tests.rs (120 cases
// across qwerty/dvorak/cardinal/stenotype). What this DOES test is
// the integration surface: the keyboard correctly appearing,
// reading the focused field's content_purpose, committing the
// right text into the right field, and dismissing on user gesture.

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import QtTest 1.15

ApplicationWindow {
    id: root
    width: 480
    height: 800
    visible: true
    title: "rusty-board test harness"

    ScrollView {
        anchors.fill: parent
        clip: true
        contentWidth: width

        ColumnLayout {
            width: root.width - 20
            x: 10
            spacing: 12

            Label {
                text: "rusty-board manual test harness"
                font.bold: true
                font.pixelSize: 18
                Layout.topMargin: 12
            }

            Label {
                text: "Focus each field and verify the listed expectation. " +
                      "Mark Pass/Fail after observing the keyboard's behavior."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                Layout.bottomMargin: 8
            }

            // -----------------------------------------------------
            // Basic focus + commit
            // -----------------------------------------------------
            TestCase {
                title: "1. Basic text field focus surfaces the keyboard"
                expectation: "Tapping the field below opens the keyboard. " +
                             "Typing 'hello' commits 'hello' into the field."
                TextField { id: tfBasic; Layout.fillWidth: true; placeholderText: "Tap to focus" }
            }

            // -----------------------------------------------------
            // Auto-capitalization
            // -----------------------------------------------------
            TestCase {
                title: "2. Auto-cap fires on sentence-start"
                expectation: "Type 'hello. world' — the W in 'world' should " +
                             "be capitalized (autocap after period+space)."
                TextField { Layout.fillWidth: true; placeholderText: "type a sentence" }
            }

            TestCase {
                title: "3. Auto-cap on empty field"
                expectation: "Tap the EMPTY field. The first letter you type " +
                             "should be capitalized (auto-cap when surrounding " +
                             "text is empty)."
                TextField { Layout.fillWidth: true; placeholderText: "first letter cap?" }
            }

            // -----------------------------------------------------
            // ContentType / input-purpose hints
            // -----------------------------------------------------
            TestCase {
                title: "4. URL field"
                expectation: "Tapping shows a normal keyboard (URL hint not " +
                             "yet specialized; just verify it appears)."
                TextField {
                    Layout.fillWidth: true
                    placeholderText: "https://..."
                    inputMethodHints: Qt.ImhUrlCharactersOnly
                }
            }

            TestCase {
                title: "5. Email field"
                expectation: "Tapping shows a normal keyboard."
                TextField {
                    Layout.fillWidth: true
                    placeholderText: "user@host"
                    inputMethodHints: Qt.ImhEmailCharactersOnly
                }
            }

            TestCase {
                title: "6. Number field"
                expectation: "Tapping shows a normal keyboard (digit gating " +
                             "not yet implemented client-side; verify presence)."
                TextField {
                    Layout.fillWidth: true
                    placeholderText: "123"
                    inputMethodHints: Qt.ImhDigitsOnly
                }
            }

            TestCase {
                title: "7. Password field"
                expectation: "Tapping shows a normal keyboard. Typing 'a' " +
                             "shows '•' or 'a' depending on TextField " +
                             "echoMode. Auto-cap should NOT fire."
                TextField {
                    Layout.fillWidth: true
                    placeholderText: "password"
                    echoMode: TextInput.Password
                    inputMethodHints: Qt.ImhHiddenText | Qt.ImhSensitiveData |
                                      Qt.ImhNoAutoUppercase
                }
            }

            // -----------------------------------------------------
            // Focus switching
            // -----------------------------------------------------
            TestCase {
                title: "8. Focus switch clears undo across fields"
                expectation: "Type 'hello' in field A. Tap field B. Tap " +
                             "Undo (long-press backspace, or the keyboard's " +
                             "undo affordance). Field B should NOT receive " +
                             "backspaces against field A's text."
                ColumnLayout {
                    Layout.fillWidth: true
                    TextField { Layout.fillWidth: true; placeholderText: "field A" }
                    TextField { Layout.fillWidth: true; placeholderText: "field B" }
                }
            }

            TestCase {
                title: "9. CapsLock doesn't leak across fields"
                expectation: "Engage CapsLock in field A, type 'HELLO'. " +
                             "Focus field B. Type 'a' — should be lowercase. " +
                             "(Recent bug fix.)"
                ColumnLayout {
                    Layout.fillWidth: true
                    TextField { Layout.fillWidth: true; placeholderText: "field A — CapsLock here" }
                    TextField { Layout.fillWidth: true; placeholderText: "field B — should be lowercase" }
                }
            }

            // -----------------------------------------------------
            // Symbols / number page transitions
            // -----------------------------------------------------
            TestCase {
                title: "10. CapsLock survives symbols visit"
                expectation: "Engage CapsLock. Type 'TODO '. Tap ?123. Type " +
                             "'5'. Tap ABC. Type 'items'. Result should be " +
                             "'TODO 5 ITEMS' — CapsLock survived the symbols dip."
                TextField { Layout.fillWidth: true; placeholderText: "TODO 5 ITEMS expected" }
            }

            TestCase {
                title: "11. One-shot Shifted is consumed by symbols visit"
                expectation: "Tap Shift (one-shot). Tap ?123 then ABC. " +
                             "Next letter should be LOWERCASE (one-shot " +
                             "shift consumed by the context switch)."
                TextField { Layout.fillWidth: true; placeholderText: "first letter lowercase" }
            }

            TestCase {
                title: "12. Auto-cap survives symbols visit"
                expectation: "Type 'and so'. Tap ?123. Type '.'. Tap ABC. " +
                             "Next letter typed should be CAPITALIZED (auto-cap " +
                             "deferred until back on letters)."
                TextField { Layout.fillWidth: true; placeholderText: "and so. THEN..." }
            }

            // -----------------------------------------------------
            // Swipe gestures
            // -----------------------------------------------------
            TestCase {
                title: "13. Swipe-up dismiss (qwerty/dvorak)"
                expectation: "Focus this field. Swipe up quickly from anywhere " +
                             "in the keyboard — keyboard should dismiss. NOT " +
                             "required to swipe past the top edge."
                TextField { Layout.fillWidth: true; placeholderText: "dismiss test" }
            }

            TestCase {
                title: "14. Swipe-typing chains words with auto-space"
                expectation: "Focus field, swipe-type 'hello' then swipe-type " +
                             "'world'. Result should be 'hello world' (auto-spaced)."
                TextField { Layout.fillWidth: true; placeholderText: "hello world" }
            }

            TestCase {
                title: "15. Swipe + manual key breaks chain"
                expectation: "Swipe-type 'hello'. Tap 'x'. Swipe-type 'world'. " +
                             "Result should be 'helloxworld' (no auto-space after " +
                             "the manual tap)."
                TextField { Layout.fillWidth: true; placeholderText: "helloxworld" }
            }

            TestCase {
                title: "16. Top-row digit swipe doesn't auto-space"
                expectation: "Swipe-type 'hello'. Do a short up-arc on top " +
                             "row to get '5'. Result should be 'hello5', NOT " +
                             "'hello 5'. (Recent bug fix.)"
                TextField { Layout.fillWidth: true; placeholderText: "hello5" }
            }

            // -----------------------------------------------------
            // Backspace + undo
            // -----------------------------------------------------
            TestCase {
                title: "17. Single backspace deletes one char"
                expectation: "Type 'hello'. Tap backspace once. Field shows 'hell'."
                TextField { Layout.fillWidth: true; placeholderText: "hello → hell" }
            }

            TestCase {
                title: "18. Hold backspace repeats"
                expectation: "Type 'hello world'. Hold backspace — should " +
                             "repeat-delete at ~10Hz until 'h' remains."
                TextField { Layout.fillWidth: true; placeholderText: "hold backspace" }
            }

            TestCase {
                title: "19. Undo whole swipe-word"
                expectation: "Swipe-type 'hello'. Trigger undo (long-press " +
                             "backspace OR the undo affordance). Field empty."
                TextField { Layout.fillWidth: true; placeholderText: "undo test" }
            }

            // -----------------------------------------------------
            // Special keys
            // -----------------------------------------------------
            TestCase {
                title: "20. Tab key fires as Tab keysym (not '\\t' text)"
                expectation: "In a multi-field form, tap Tab — focus should " +
                             "move to next field. (Was a silently-dropped " +
                             "keystroke bug on v2 wayland.)"
                ColumnLayout {
                    Layout.fillWidth: true
                    TextField { Layout.fillWidth: true; placeholderText: "field 1 — tab from here" }
                    TextField { Layout.fillWidth: true; placeholderText: "field 2 — focus should arrive" }
                }
            }

            TestCase {
                title: "21. Return inserts newline (in multi-line field)"
                expectation: "Tap Return in the field below — newline inserted."
                TextArea {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 80
                    placeholderText: "tap Return for newline"
                }
            }

            // -----------------------------------------------------
            // Cardinal / Stenotype mode (manual switch in Settings)
            // -----------------------------------------------------
            TestCase {
                title: "22. Cardinal swipes commit verbatim (no auto-space)"
                expectation: "Switch to cardinal mode in keyboard Settings. " +
                             "Swipe cell+direction for two adjacent letters. " +
                             "Should commit 'ue' (no space), not 'u e'."
                TextField { Layout.fillWidth: true; placeholderText: "cardinal: 'ue' not 'u e'" }
            }

            TestCase {
                title: "23. Stenotype phoneme chain"
                expectation: "Switch to stenotype mode. Tap k + a + t. " +
                             "Field should show 'kat', NOT 'k a t'."
                TextField { Layout.fillWidth: true; placeholderText: "steno: 'kat' not 'k a t'" }
            }

            TestCase {
                title: "24. Cardinal/Stenotype dismiss (strict mode)"
                expectation: "In cardinal/stenotype mode, do a LONG FAST upward " +
                             "flick. Keyboard should dismiss. A short cell-swipe " +
                             "must NOT dismiss (it should commit a character)."
                TextField { Layout.fillWidth: true; placeholderText: "strict dismiss test" }
            }

            TestCase {
                title: "25. Stenotype ?123 button (not 'ABC')"
                expectation: "In stenotype mode, the symbols-entry button is " +
                             "labeled '?123' (renamed from 'ABC' for consistency). " +
                             "Tap it — goes to symbols. Tap ABC there — back to steno."
                TextField { Layout.fillWidth: true; placeholderText: "verify ?123 label" }
            }

            // -----------------------------------------------------
            // Voice / LLM
            // -----------------------------------------------------
            TestCase {
                title: "26. Voice key commits transcript"
                expectation: "Tap mic (🎤). Speak 'hello world'. After " +
                             "release, transcript appears in field. (Requires " +
                             "voice feature compiled in.)"
                TextField { Layout.fillWidth: true; placeholderText: "voice transcript" }
            }

            TestCase {
                title: "27. LLM synonym (long-press backspace)"
                expectation: "Type 'happy'. Long-press backspace until synonym " +
                             "request fires. Field should show the LLM's " +
                             "synonym (e.g., 'joyful'). Should NOT delete chars " +
                             "during the long-press window beyond ~1 backspace " +
                             "(recent timing fix)."
                TextField { Layout.fillWidth: true; placeholderText: "synonym test" }
            }

            // -----------------------------------------------------
            // Latency / responsiveness
            // -----------------------------------------------------
            TestCase {
                title: "28. First keystroke after idle feels snappy"
                expectation: "Wait 5 seconds without typing. Tap a key. " +
                             "Should commit IMMEDIATELY (no perceptible delay). " +
                             "The previous polled bridge added up to 60ms; " +
                             "direct-dispatch removed that wait."
                TextField { Layout.fillWidth: true; placeholderText: "idle then tap" }
            }

            TestCase {
                title: "29. Burst typing keeps up"
                expectation: "Type rapidly. All keystrokes should appear in " +
                             "order, no dropped chars, no perceptible queue."
                TextField { Layout.fillWidth: true; placeholderText: "burst typing" }
            }

            // -----------------------------------------------------
            // Edge cases
            // -----------------------------------------------------
            TestCase {
                title: "30. Focus loss mid-swipe doesn't strand state"
                expectation: "Start a swipe-typing trace. While still dragging, " +
                             "trigger a focus change (e.g., notification banner). " +
                             "Keyboard should reset cleanly — next tap works, " +
                             "no 'dead keyboard' state."
                TextField { Layout.fillWidth: true; placeholderText: "focus-loss test" }
            }

            Item { Layout.preferredHeight: 24 }  // bottom padding
        }
    }

    // -----------------------------------------------------
    // TestCase: a labeled card with title + expectation +
    // user-supplied content + Pass/Fail radio.
    // -----------------------------------------------------
    component TestCase: Frame {
        id: tc
        property string title: ""
        property string expectation: ""
        default property alias content: contentItem.children

        Layout.fillWidth: true

        ColumnLayout {
            anchors.fill: parent
            spacing: 4

            Label {
                text: tc.title
                font.bold: true
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            Label {
                text: tc.expectation
                font.pixelSize: 12
                color: "#555"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            Item {
                id: contentItem
                Layout.fillWidth: true
                Layout.preferredHeight: childrenRect.height
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 12
                ButtonGroup { id: bg }
                RadioButton { text: "Pass"; ButtonGroup.group: bg }
                RadioButton { text: "Fail"; ButtonGroup.group: bg }
                RadioButton { text: "Skip"; ButtonGroup.group: bg; checked: true }
                Item { Layout.fillWidth: true }
            }
        }
    }
}
