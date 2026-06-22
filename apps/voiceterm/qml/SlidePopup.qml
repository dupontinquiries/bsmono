import QtQuick 2.15

/**
 * SlidePopup — passive visualizer driven by GestureHost (v0.1.143).
 *
 * This is the gesture-menu component proper. It draws a grid of options,
 * tracks which one the pointer is over, and emits `selected(id, variant,
 * context)` on commit. It has NO input handlers of its own — the host
 * (typically a GestureHost wrapping a button) owns the press/drag/release
 * via its own MouseArea and calls four lifecycle functions:
 *
 *   show(host, localX, localY, context)
 *       Open the popup, position above (or below) the host button.
 *   update(globalX, globalY)
 *       Pointer moved (coords in hostRoot space). Recompute hover.
 *   commit()
 *       Pointer released over an option → emit selected; hide.
 *   cancel()
 *       Released outside any option or grab yanked → emit cancelled.
 *
 * v0.1.142 architecture (broken): popup had its own PointHandler that
 * tried to grab the gesture by reparenting on press. This loses the
 * race with the host's MouseArea which already owns the press. v0.1.143
 * inverts the relationship: the host owns input, popup is dumb.
 *
 * Model schema:
 *   [{ label, id, gridX, gridY, svg?, ... }, ...]
 *
 *   - `label` (string) — text shown in the cell. Used when no `svg`
 *     is provided. Can be unicode/emoji.
 *   - `id` (string) — opaque identifier echoed in selected().
 *   - `gridX`/`gridY` — zero-indexed grid position; omit for linear
 *     layout.
 *   - `svg` (optional string, v0.1.164+) — inline SVG data URI. When
 *     present, suppresses the text label and renders the SVG via
 *     QML Image. Use for richer icons than what unicode covers
 *     (e.g. cascading lock cluster for security-tier menus).
 *   - Any other fields (char, openChar, closeChar, etc) are ignored
 *     by the popup; the host's onSelected looks them up by id.
 *
 * Signals:
 *   selected(string optionId, string variant, var context)
 *   cancelled()
 *
 *   variant is "single" by default. With enableDeadband: true, one of
 *   "open" (left of cell center), "close" (right), "both" (in center
 *   ±deadbandPx). context is the opaque value passed to show().
 */
Item {
    id: popup

    // ═══ INPUT ═══

    /// Top-level item used as the absolute coord space for positioning
    /// and hit-testing. Almost always = ApplicationWindow's root.
    property Item hostRoot: parent

    /// Option list (see Model schema in header).
    property var model: []

    /// Enable 3-zone horizontal deadband (open/both/close variants).
    property bool enableDeadband: false
    property int deadbandPx: 20

    // v0.1.538: tap-select mode. Default OFF so the terminal toolbar's
    // press-drag-release behavior is untouched. When ON (the link menu,
    // which opens from a discrete tap rather than a held drag), each
    // option is individually tappable and emits `selected` directly —
    // no center-commit-on-release.
    property bool tapSelect: false

    // v0.1.800: optional title rendered as a full-width header ABOVE the grid
    // (e.g. the tab's name on the per-tab action menu). Empty → no header row,
    // so every existing caller (pipe/brace/link menus) is unaffected.
    property string title: ""
    property int titleFontSize: 13

    // ═══ STYLING ═══

    property color bgColor: "#1e1e1e"
    property color optionBgColor: "#2b2b2b"
    property color textColor: "#d4d0c8"
    property color highlightColor: "#5599dd"
    property color borderColor: "#3e3e3e"
    property int fontSize: 14
    property int itemSize: 56
    property int itemSpacing: 8
    property int padding: 10
    // v0.1.814: show/hide fade duration (ms). Settable from the global animation
    // slider; 0 = instant. Defaults to the old 150 ms so unset instances are
    // unchanged.
    property int animMs: 150

    // ═══ OUTPUT ═══

    signal selected(string optionId, string variant, var context)
    signal cancelled()

    // ═══ DERIVED LAYOUT ═══

    readonly property int _gridW: {
        if (!model || model.length === 0) return 1
        var m = 0
        for (var i = 0; i < model.length; i++) {
            var gx = (model[i].gridX !== undefined) ? model[i].gridX : i
            if (gx > m) m = gx
        }
        return m + 1
    }
    readonly property int _gridH: {
        if (!model || model.length === 0) return 1
        var m = 0
        for (var i = 0; i < model.length; i++) {
            var gy = (model[i].gridY !== undefined) ? model[i].gridY : 0
            if (gy > m) m = gy
        }
        return m + 1
    }

    // v0.1.158: clamp itemSize to fit within hostRoot bounds. On mobile
    // with 300% scaling, the brace popup (1×6 grid, itemSize=120 in
    // comfort mode) wants 720px of vertical room — way more than the
    // viewport. Without clamping, the popup overflows offscreen and
    // becomes unusable.
    //
    // Reserve 12px (8 margin + 4 buffer) on each side. Compute the max
    // itemSize that would still produce a popup fitting within hostRoot,
    // and use the lesser of (requested, that max). Floor at 32 so the
    // cells stay tappable even when severely clamped.
    //
    // Falls back to raw `itemSize` when hostRoot is null (not yet
    // bound) or has zero dimensions.
    readonly property int _effectiveItemSize: {
        if (!hostRoot || hostRoot.width <= 0 || hostRoot.height <= 0) {
            return itemSize
        }
        var availW = hostRoot.width - 2 * padding - 24
        var availH = hostRoot.height - 2 * padding - 24
        var maxFromW = Math.floor((availW - (_gridW - 1) * itemSpacing) / _gridW)
        var maxFromH = Math.floor((availH - (_gridH - 1) * itemSpacing) / _gridH)
        var maxFit = Math.min(maxFromW, maxFromH)
        if (maxFit < 32) maxFit = 32  // floor for tappability
        return Math.min(itemSize, maxFit)
    }

    readonly property int _contentW: _gridW * _effectiveItemSize + (_gridW - 1) * itemSpacing + 2 * padding
    readonly property int _contentH: _gridH * _effectiveItemSize + (_gridH - 1) * itemSpacing + 2 * padding

    // v0.1.800: header band height (0 when no title).
    readonly property int _titleH: (title && title.length > 0) ? (titleFontSize + 12) : 0

    width: _contentW
    height: _contentH + _titleH

    // ═══ STATE (driven by lifecycle calls) ═══

    /// True between show() and commit()/cancel().
    property bool _open: false

    /// Echoed back in the selected signal.
    property var _ctx: undefined

    /// Press point in hostRoot coords, captured in show().
    property real _pressX: 0
    property real _pressY: 0

    /// Press timestamp (Date.now()), used for fade animation tuning.
    property real _openTime: 0

    /// Most recent pointer position in hostRoot coords.
    property real _curX: 0
    property real _curY: 0

    /// Current drag speed (px per 100ms) — drives fade-in duration.
    property real _dragSpeed: 0.0

    /// Current hover: option id, or "" if none.
    property string _hoverId: ""
    property string _hoverVariant: "single"

    /// True when pointer is past the popup's vertical extent — freezes
    /// hover updates and grows the border to ~200% as visual feedback.
    property bool _atVerticalEdge: false

    visible: false
    opacity: 0.0
    // v0.1.166: high z so the popup always stacks above its siblings
    // on Overlay.overlay. When a SlidePopup is parented to
    // Overlay.overlay and its host lives inside a modal Popup (e.g.
    // file-picker row menu, settings send-context button), the
    // modal Popup paints over the SlidePopup by default because
    // both are children of Overlay and Qt paints in document order.
    // Bumping z explicitly puts the SlidePopup above modal popups.
    //
    // User report: "send terminal content is rendered underneath
    // the settings page... I can't see it" — the SlidePopup was
    // hidden behind the settings popup.
    z: 100

    // ═══ ANIMATION ═══

    // Initial fade-in is a fast 220ms so the popup is always visibly
    // present, even on quick click-release with no drag.
    //
    // The "fade slows as user explores slowly" behavior the user wanted
    // is achieved via the Behavior below, which only fires on opacity
    // changes AFTER the initial show. (Slow re-fades are reserved for
    // hide/re-show scenarios; initial visibility is non-negotiable.)
    //
    // The dragSpeed factor in the formula clamps to [80ms..600ms]:
    // fast drag = 80ms (snap), slow drag = 600ms (visible fade). The
    // 1000ms cap was excessive — fades that long made quick interactions
    // feel like the popup wasn't appearing at all.
    //
    // v0.1.166: disable the Behavior while explicit show/hide
    // animations are running. Previously both fired simultaneously
    // (the explicit NumberAnimation drove the opacity property
    // imperatively, which the Behavior also reacted to), producing
    // a double-animation that read as ghosting — two overlapping
    // fade curves competing for the same property.
    //
    // User report: "the way the opacity is handled is kind of
    // weird... it creates ghosting."
    Behavior on opacity {
        enabled: !_showFade.running && !_hideFade.running
        NumberAnimation {
            duration: Math.max(80, 600 - Math.min(520, popup._dragSpeed * 100))
            easing.type: Easing.OutQuad
        }
    }

    // Explicit fast-fade animation used by show() — bypasses the
    // dragSpeed-aware Behavior so the popup is always visible quickly
    // on first press, regardless of subsequent drag speed.
    NumberAnimation {
        id: _showFade
        target: popup
        property: "opacity"
        from: 0.0
        to: 1.0
        duration: popup.animMs
        easing.type: Easing.OutQuad
    }

    // ═══ VISUALS ═══

    Rectangle {
        id: _bg
        anchors.fill: parent
        color: popup.bgColor
        radius: 12
        border.color: popup.borderColor
        // 2px normally; grows to 4px (~200%) when pointer is past edge.
        border.width: popup._atVerticalEdge ? 4 : 2
        Behavior on border.width { NumberAnimation { duration: 120; easing.type: Easing.OutQuad } }
    }

    // v0.1.800: full-width title header above the grid (visible only when a
    // title is set — the per-tab menu sets the tab's name here).
    Text {
        visible: popup._titleH > 0
        x: popup.padding
        y: 0
        width: popup.width - 2 * popup.padding
        height: popup.padding + popup._titleH
        text: popup.title
        color: popup.textColor
        font.pixelSize: popup.titleFontSize
        font.family: "monospace"
        font.bold: true
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    Repeater {
        model: popup.model
        delegate: Rectangle {
            id: _opt
            readonly property var item: modelData
            readonly property int gx: (item.gridX !== undefined) ? item.gridX : index
            readonly property int gy: (item.gridY !== undefined) ? item.gridY : 0
            readonly property bool isHover: popup._hoverId === item.id

            x: popup.padding + gx * (popup._effectiveItemSize + popup.itemSpacing)
            y: popup.padding + popup._titleH + gy * (popup._effectiveItemSize + popup.itemSpacing)
            width: popup._effectiveItemSize
            height: popup._effectiveItemSize
            radius: 8
            color: isHover ? popup.highlightColor : popup.optionBgColor
            border.color: isHover ? Qt.lighter(popup.highlightColor, 1.3) : "transparent"
            border.width: 2
            Behavior on color { ColorAnimation { duration: 80 } }

            Text {
                anchors.centerIn: parent
                // v0.1.164: if the model entry provides an `svg` field,
                // suppress the text label and let the sibling Image
                // render the icon instead. Cells can mix-and-match:
                // some show emoji/unicode via `label`, others show
                // an inline SVG via `svg`. Most existing models use
                // only `label`; the send-context popup uses `svg` for
                // the cascading-locks tier icons.
                visible: !item.svg
                text: item.label || ""
                color: popup.textColor
                font.pixelSize: popup.fontSize
                font.family: "monospace"
                horizontalAlignment: Text.AlignHCenter
            }

            // SVG icon variant. Inline data URI in the model so no
            // asset file is needed. `Image` honors `sourceSize` for
            // crisp SVG rasterization at the cell's actual pixel
            // size; we use 70% of the cell to leave breathing room
            // around the lock cascade.
            Image {
                anchors.centerIn: parent
                visible: !!item.svg
                source: item.svg || ""
                sourceSize.width: Math.floor(popup._effectiveItemSize * 0.7)
                sourceSize.height: Math.floor(popup._effectiveItemSize * 0.7)
                width: sourceSize.width
                height: sourceSize.height
                smooth: true
                mipmap: true
                fillMode: Image.PreserveAspectFit
            }

            // v0.1.538: tap-select mode — each option individually
            // tappable. Only present/enabled when popup.tapSelect is
            // true (the link menu), so the toolbar's drag model is
            // unaffected. Emits selected directly and closes.
            MouseArea {
                anchors.fill: parent
                enabled: popup.tapSelect
                visible: popup.tapSelect
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onEntered: popup._hoverId = _opt.item.id
                onExited: { if (popup._hoverId === _opt.item.id) popup._hoverId = "" }
                onClicked: {
                    popup._open = false
                    popup.visible = false
                    popup.selected(_opt.item.id, "single", popup._ctx)
                }
            }

            // Variant indicator for deadband mode. Small directional
            // arrow shows which zone the pointer is currently in:
            //   ◀  (open) — drag left, single open char (e.g. "{")
            //   nothing  (both) — center, both chars with cursor between
            //   ▶  (close) — drag right, single close char (e.g. "}")
            //
            // v0.1.154 polish from v0.1.142's "(o)/(b)/(c)" text. The
            // arrows are intuitive (point in the drag direction) and
            // the no-hint center matches "both" being the default —
            // user doesn't need a label saying "default".
            //
            // Filed for future polish: colored side-stripe on the
            // option Rectangle reinforcing which half the pointer
            // is in (cell highlight half-tinted left/right).
            Text {
                visible: _opt.isHover
                         && (popup._hoverVariant === "open" || popup._hoverVariant === "close"
                             || popup._hoverVariant === "vertical" || popup._hoverVariant === "horizontal"
                             || popup._hoverVariant === "pane" || popup._hoverVariant === "tab")
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 2
                text: popup._hoverVariant === "open"  ? "\u25C0"
                    : popup._hoverVariant === "close" ? "\u25B6"
                    : popup._hoverVariant === "vertical"   ? "\u2195"
                    : popup._hoverVariant === "horizontal" ? "\u2194"
                    : popup._hoverVariant === "pane" ? "pane"
                    : popup._hoverVariant === "tab"  ? "tab"
                    : ""
                color: Qt.lighter(popup.highlightColor, 1.5)
                font.pixelSize: 10
            }
        }
    }

    // ═══ LIFECYCLE API (called by GestureHost) ═══

    function show(host, localX, localY, context) {
        if (!host || !hostRoot) return
        _ctx = context
        _open = true
        _openTime = Date.now()
        _dragSpeed = 0
        _hoverId = ""
        _hoverVariant = "single"
        _atVerticalEdge = false

        // POSITIONING STRATEGY: place the popup so the press point lands
        // on the CENTER of the option grid. This means:
        //   - A click-without-drag immediately hovers (and commits) the
        //     center-most option, which is a sane default.
        //   - A drag in any direction immediately moves through the grid;
        //     the user doesn't have to first drag INTO the popup from
        //     outside it.
        // We compute the press point in hostRoot coords, then position
        // the popup so its option-grid center sits at that point, then
        // clamp to hostRoot bounds.
        var pressInRoot = host.mapToItem(hostRoot, localX, localY)
        var gridCenterX = padding + (_gridW * _effectiveItemSize + (_gridW - 1) * itemSpacing) / 2
        // v0.1.801: the title band (#4) pushes the grid down by _titleH, so the
        // grid centre — where we want the press point to land for drag-select —
        // is offset too.
        var gridCenterY = padding + _titleH + (_gridH * _effectiveItemSize + (_gridH - 1) * itemSpacing) / 2
        var targetX = pressInRoot.x - gridCenterX
        var targetY = pressInRoot.y - gridCenterY

        // Clamp to hostRoot bounds with 4px margin (use full height incl. title).
        if (targetX < 4) targetX = 4
        if (targetX + _contentW > hostRoot.width - 4) {
            targetX = hostRoot.width - _contentW - 4
        }
        if (targetY < 4) targetY = 4
        if (targetY + height > hostRoot.height - 4) {
            targetY = hostRoot.height - height - 4
        }
        x = targetX
        y = targetY

        // Show with explicit fast fade-in (always visible quickly).
        opacity = 0.0
        visible = true
        _showFade.restart()

        // Pre-highlight if press already overlaps an option.
        var pt = host.mapToItem(hostRoot, localX, localY)
        _curX = pt.x
        _curY = pt.y
        _pressX = pt.x
        _pressY = pt.y
        _recomputeHover()
    }

    function update(globalX, globalY) {
        if (!_open) return
        _curX = globalX
        _curY = globalY
        var elapsed = Math.max(1, Date.now() - _openTime)
        var dxs = _curX - _pressX
        var dys = _curY - _pressY
        var dist = Math.sqrt(dxs * dxs + dys * dys)
        _dragSpeed = (dist / elapsed) * 100
        _recomputeHover()
    }

    function commit() {
        if (!_open) return
        _open = false
        if (_hoverId !== "") {
            _hideFade.restart()
            _emitTimer.id = _hoverId
            _emitTimer.variant = _hoverVariant
            _emitTimer.ctx = _ctx
            _emitTimer.restart()
        } else {
            // Pointer wasn't over any option; treat as cancel.
            _hideFade.restart()
            _hideTimer.restart()
            cancelled()
        }
    }

    function cancel() {
        if (!_open) return
        _open = false
        _hideFade.restart()
        _hideTimer.restart()
        cancelled()
    }

    // ═══ INTERNAL ═══

    // Explicit fast fade-out so close is always crisp regardless of
    // the dragSpeed-aware Behavior. Mirror of _showFade.
    NumberAnimation {
        id: _hideFade
        target: popup
        property: "opacity"
        from: popup.opacity
        to: 0.0
        duration: popup.animMs
        easing.type: Easing.OutQuad
    }

    Timer {
        id: _hideTimer
        interval: 200
        onTriggered: popup.visible = false
    }

    Timer {
        id: _emitTimer
        interval: 150
        property string id: ""
        property string variant: "single"
        property var ctx: undefined
        onTriggered: {
            popup.visible = false
            popup.selected(id, variant, ctx)
        }
    }

    function _recomputeHover() {
        // Convert pointer (hostRoot coords) → popup-local.
        var lx = _curX - x
        var ly = _curY - y

        // Vertical-edge check: freeze hover and bloom border when past
        // top or bottom of the option grid (with a small 4px tolerance).
        // v0.1.801: include the title band (#4) so hit-testing lines up with
        // the visually offset cells.
        var topY = padding + _titleH
        var botY = padding + _titleH + _gridH * _effectiveItemSize + (_gridH - 1) * itemSpacing
        if (ly < topY - 4 || ly > botY + 4) {
            _atVerticalEdge = true
            return  // don't update _hoverId — freeze at last in-bounds
        }
        _atVerticalEdge = false

        // Hit-test each option cell.
        for (var i = 0; i < model.length; i++) {
            var item = model[i]
            var gx = (item.gridX !== undefined) ? item.gridX : i
            var gy = (item.gridY !== undefined) ? item.gridY : 0
            var ox = padding + gx * (_effectiveItemSize + itemSpacing)
            var oy = padding + _titleH + gy * (_effectiveItemSize + itemSpacing)
            if (lx >= ox && lx < ox + _effectiveItemSize && ly >= oy && ly < oy + _effectiveItemSize) {
                _hoverId = item.id
                if (item.splitDir) {
                    // v0.1.812: directional split — pointer more toward the TOP of
                    // the cell commits a vertical split, more toward the RIGHT a
                    // horizontal one ("up = vertical, right = horizontal"). The hint
                    // glyph (↕/↔) shows which the release will pick.
                    var cxS = ox + _effectiveItemSize / 2
                    var cyS = oy + _effectiveItemSize / 2
                    var upness = cyS - ly        // > 0 when above center
                    var rightness = lx - cxS     // > 0 when right of center
                    _hoverVariant = (upness >= rightness) ? "vertical" : "horizontal"
                } else if (item.closeDir) {
                    // v0.1.813: directional close — more toward the BOTTOM closes the
                    // focused pane, more toward the RIGHT (or default) closes the tab.
                    var cxC = ox + _effectiveItemSize / 2
                    var cyC = oy + _effectiveItemSize / 2
                    var downness = ly - cyC      // > 0 below center
                    var rightnessC = lx - cxC    // > 0 right of center
                    _hoverVariant = (downness > rightnessC) ? "pane" : "tab"
                } else if (enableDeadband) {
                    var centerX = ox + _effectiveItemSize / 2
                    var d = lx - centerX
                    if (Math.abs(d) <= deadbandPx) {
                        _hoverVariant = "both"
                    } else if (d < 0) {
                        _hoverVariant = "open"
                    } else {
                        _hoverVariant = "close"
                    }
                } else {
                    _hoverVariant = "single"
                }
                return
            }
        }

        // In bounds vertically but in the gap between cells — clear hover
        // so a release here counts as cancel.
        _hoverId = ""
        _hoverVariant = "single"
    }
}
