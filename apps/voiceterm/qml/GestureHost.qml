import QtQuick 2.15

/**
 * GestureHost — input-owning wrapper for a button that drives a SlidePopup
 * via press → drag → release → commit/cancel lifecycle. v0.1.148 re-extract
 * of the inline pattern used by the toolbar pipe/brace buttons and the
 * blocks-drawer `⋯` button. v0.1.143 had a broken first attempt at this
 * (popup tried to grab input via PointHandler reparenting + mapToItem(root,)
 * TypeError); v0.1.144 inlined the working pattern; v0.1.146 inlined it
 * a second time for the block-row button; v0.1.148 finally extracts the
 * proven inline pattern — third user justifies the abstraction.
 *
 * Design (locked-in v0.1.144+):
 *   The host's MouseArea owns the entire gesture. It calls four lifecycle
 *   functions on the popup: show, update, commit, cancel. The popup is a
 *   passive visualizer — no PointHandlers of its own, no reparenting, no
 *   input grab juggling. One event source, no race.
 *
 *   Coordinate handoff: MouseArea events arrive in host-local coords;
 *   the popup hit-tests in hostRoot coords (typically root.contentItem).
 *   GestureHost maps via host.mapToItem(popup.hostRoot, ...) on every
 *   forward.
 *
 * Wiring contract:
 *
 *   GestureHost {
 *       id: someBtn
 *       popup: _somePopup              // SlidePopup instance to drive
 *       gestureContext: "toolbar.foo"  // opaque value; echoed in selected()
 *       enabled: !someBlockingCondition
 *       width: 38; height: 32
 *
 *       // Visuals as default-property children. Common shape:
 *       Rectangle {
 *           anchors.fill: parent
 *           radius: 8
 *           color: ...
 *           Text { anchors.centerIn: parent; text: "|⋯"; ... }
 *       }
 *   }
 *
 * Why not what I tried in v0.1.143:
 *   v0.1.143 made GestureHost an Item containing a MouseArea, and the
 *   visuals came in as default-property children that rendered *above*
 *   the MouseArea. Mouse events were probably never an issue with that
 *   ordering (a bare Rectangle doesn't intercept events) — but combined
 *   with the popup's separate PointHandler that tried to reparent on
 *   press, and the mapToItem(root, …) TypeError, the whole thing died
 *   silently and looked unclickable. v0.1.144 inlined the proven pattern
 *   per-button. v0.1.148 re-extracts that proven pattern WITHOUT any new
 *   architectural ideas — literally moves the 4 handlers + 2 helpers
 *   into a wrapper Item. If this breaks, the bug is in the extraction
 *   mechanics, not the design.
 *
 * Children-render-above-MouseArea note:
 *   The MouseArea is declared first inside this component, then
 *   default-property children (caller's visuals) come after — so the
 *   visuals render on top in z-order. A bare Rectangle doesn't accept
 *   pointer events, so the MouseArea below still receives them. This
 *   matches the working block-row `⋯` and toolbar pipe layouts where
 *   Text + MouseArea are siblings under a Rectangle.
 */
Item {
    id: gestureHost

    // ═══ INPUT ═══

    /// SlidePopup (or any object implementing the lifecycle contract:
    /// show(host, localX, localY, context), update(globalX, globalY),
    /// commit(), cancel()). Required.
    property var popup: null

    /// Opaque value passed to popup.show() and echoed back in the
    /// popup's selected signal. Use to disambiguate when one popup
    /// instance serves multiple hosts (e.g. per-row menus in a list).
    property var gestureContext: undefined

    /// Default true. Prevents an enclosing Flickable/ListView from
    /// stealing the press for its own scroll gesture. Override false
    /// only if you want the parent's gesture to win.
    property bool preventStealing: true

    /// When true, the internal MouseArea tracks hover, and
    /// `containsMouse` reflects pointer-over state. Default false to
    /// avoid competing with other hover-driven UI in the same region.
    property bool hoverEnabled: false

    /// Mirrors the internal MouseArea's containsMouse. Useful for
    /// hover-styled visuals at the call site (color swap on hover,
    /// tooltip visibility, etc.) without needing a separate
    /// MouseArea sibling.
    readonly property bool containsMouse: _ma.containsMouse

    /// Mirrors the internal MouseArea's pressed state. Same purpose.
    readonly property bool pressed: _ma.pressed

    // ═══ INTERNAL ═══

    // Mouse-area handlers. Mirrors the v0.1.144 inline pattern verbatim.
    // The visuals (caller-supplied default-property children) render
    // above this MouseArea in z-order but don't intercept events (bare
    // Rectangles/Texts don't accept pointer events by default), so the
    // MouseArea still receives press/move/release.
    MouseArea {
        id: _ma
        anchors.fill: parent
        enabled: gestureHost.enabled
        preventStealing: gestureHost.preventStealing
        hoverEnabled: gestureHost.hoverEnabled
        cursorShape: gestureHost.hoverEnabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        property bool _active: false

        onPressed: function(mouse) {
            _active = true
            gestureHost.popup.show(gestureHost, mouse.x, mouse.y,
                                   gestureHost.gestureContext)
            // First update so any immediate hover registers without
            // waiting for the next motion event.
            var pt = gestureHost.mapToItem(gestureHost.popup.hostRoot,
                                            mouse.x, mouse.y)
            gestureHost.popup.update(pt.x, pt.y)
        }

        onPositionChanged: function(mouse) {
            if (!_active) return
            var pt = gestureHost.mapToItem(gestureHost.popup.hostRoot,
                                            mouse.x, mouse.y)
            gestureHost.popup.update(pt.x, pt.y)
        }

        onReleased: function(mouse) {
            if (!_active) return
            _active = false
            // Final position update so commit sees the latest hover.
            var pt = gestureHost.mapToItem(gestureHost.popup.hostRoot,
                                            mouse.x, mouse.y)
            gestureHost.popup.update(pt.x, pt.y)
            gestureHost.popup.commit()
        }

        onCanceled: {
            // Qt yanked the press (drawer closed, parent disabled, etc).
            // Treat as cancel so the popup hides cleanly.
            if (!_active) return
            _active = false
            gestureHost.popup.cancel()
        }
    }
}
