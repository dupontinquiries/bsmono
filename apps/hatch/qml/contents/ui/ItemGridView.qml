import QtQuick 2.15

import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami 2.20 as Kirigami
import org.kde.plasma.plasmoid 2.0

/* The grid, rebuilt around Qt Quick input handlers instead of the old
   MouseArea/MultiPointTouchArea stack. What that stack cost, and the fix:

   - hoverArea (a full-size MouseArea accepting LeftButton) grabbed every
     press before GridView's Flickable saw it → touch flicking never worked.
     Now nothing accepts presses over the view: handlers observe, the
     Flickable owns the drag, so kinetic scrolling is native.
   - hoverArea.onReleased with no item under the finger called root.toggle()
     → a missed tap CLOSED the launcher. Now a tap on empty grid space does
     nothing at all; dismissal is an explicit gesture (background tap outside
     the grid, pull past the top, Escape).
   - Two overlapping MultiPointTouchAreas (one with a duplicate onReleased —
     a QML error) implemented half a page-swipe for a vertical grid. Gone.

   Interaction contract:
     tap          → activate (launch + close)
     long-press   → context menu (the touch path to favorites)
     right-click  → context menu on a tile, plasmoid menu on empty space
     drag         → native flick; pulling past the top dismisses (config)
     hover        → highlight follows the mouse (mouse only, so touch
                    scrolling doesn't smear the current item) */
FocusScope {
    id: itemGrid

    signal keyNavLeft
    signal keyNavRight
    signal keyNavUp
    signal keyNavDown
    signal itemActivated(int index, string actionId, string argument)

    property bool showLabels: true
    property bool grabFocus: false
    // Pull-down-past-top to dismiss; the runner grid turns this off so search
    // scrolling can't accidentally close the window.
    property bool pullToDismiss: Plasmoid.configuration.pullToDismiss

    property alias currentIndex: gridView.currentIndex
    property alias currentItem: gridView.currentItem
    property alias count: gridView.count
    property alias model: gridView.model
    property alias cellWidth: gridView.cellWidth
    property alias cellHeight: gridView.cellHeight
    property alias iconSize: gridView.iconSize
    property var verticalScrollBarPolicy: PlasmaComponents.ScrollBar.AsNeeded

    onFocusChanged: {
        if (!focus) {
            currentIndex = -1;
        }
    }

    // ---- geometry helpers (used by keyboard nav and the dashboard) --------
    readonly property int columns: Math.max(1, Math.floor(width / cellWidth))

    function currentRow() {
        return currentIndex === -1 ? -1 : Math.floor(currentIndex / columns);
    }

    function currentCol() {
        return currentIndex === -1 ? -1 : currentIndex - (currentRow() * columns);
    }

    function lastRow() {
        return Math.ceil(count / columns) - 1;
    }

    function tryActivate(row, col) {
        if (count) {
            const rows = Math.ceil(count / columns);
            row = Math.min(row, rows - 1);
            col = Math.min(col, columns - 1);
            currentIndex = Math.min(row ? ((Math.max(1, row) * columns) + col) : col,
                                    count - 1);
            focus = true;
        }
    }

    function forceLayout() {
        gridView.forceLayout();
    }

    // The one activation path: keyboard Enter, tap, and search-Enter all end
    // here, so launch-and-close can never drift apart between input methods.
    function activate(index) {
        if (index < 0 || index >= count) {
            return;
        }
        if ("trigger" in gridView.model) {
            gridView.model.trigger(index, "", null);
            root.toggle();
        }
        itemGrid.itemActivated(index, "", null);
    }

    function indexBeneath(point) {
        const c = mapToItem(gridView.contentItem, point.x, point.y);
        const it = gridView.itemAt(c.x, c.y);
        return it ? it.itemIndex : -1;
    }

    function openContextMenu(index, point) {
        const it = gridView.itemAtIndex(index);
        if (it && it.hasActionList) {
            const mapped = itemGrid.mapToItem(it, point.x, point.y);
            it.openActionMenu(mapped.x, mapped.y);
        }
    }

    ActionMenu {
        id: actionMenu

        onActionClicked: {
            visualParent.actionTriggered(actionId, actionArgument);
        }
    }

    GridView {
        id: gridView

        anchors.fill: parent

        property int iconSize: Kirigami.Units.iconSizes.huge
        // Which tile is under a live press — delegates key their squish off
        // this instead of owning input themselves.
        property int pressedIndex: -1
        // True only while a mouse is over the view; keeps hover styling from
        // sticking to whatever a touch flick last passed over.
        property bool hoverActive: hoverHandler.hovered

        focus: true
        currentIndex: -1
        interactive: true

        // Touch-flick tuning: StopAtBounds would kill the pull-to-dismiss
        // overshoot, so allow drag past the edge but not flick past it.
        boundsBehavior: Flickable.DragOverBounds
        flickDeceleration: 2600
        maximumFlickVelocity: 5200
        reuseItems: true
        cacheBuffer: 400
        keyNavigationWraps: false

        onDraggingChanged: {
            if (!dragging && itemGrid.pullToDismiss
                    && verticalOvershoot < -Kirigami.Units.gridUnit * 2) {
                root.toggle();
            }
        }

        // ---- restack: the model changed, show tiles travelling ------------
        readonly property bool restack: Plasmoid.configuration.enableAnimations
                                        && Plasmoid.configuration.restackDuration > 0
        readonly property int restackMs: Plasmoid.configuration.restackDuration
        readonly property int staggerMs: Plasmoid.configuration.restackStagger

        addDisplaced: Transition {
            enabled: gridView.restack
            NumberAnimation { properties: "x,y"; duration: gridView.restackMs; easing.type: Easing.OutCubic }
        }
        removeDisplaced: Transition {
            enabled: gridView.restack
            PauseAnimation { duration: gridView.staggerMs }
            NumberAnimation { properties: "x,y"; duration: gridView.restackMs; easing.type: Easing.OutCubic }
        }
        moveDisplaced: Transition {
            enabled: gridView.restack
            NumberAnimation { properties: "x,y"; duration: gridView.restackMs; easing.type: Easing.OutCubic }
        }
        move: Transition {
            enabled: gridView.restack
            NumberAnimation { properties: "x,y"; duration: gridView.restackMs; easing.type: Easing.OutCubic }
        }
        remove: Transition {
            enabled: gridView.restack
            NumberAnimation { property: "opacity"; to: 0; duration: gridView.restackMs / 2 }
        }

        delegate: ItemGridDelegate {
            showLabel: itemGrid.showLabels
        }

        highlight: Rectangle {
            color: colorWithAlpha(Kirigami.Theme.highlightColor, 0.65)
            radius: 10
            visible: gridView.currentIndex !== -1

            Behavior on x { enabled: gridView.restack; NumberAnimation { duration: 120; easing.type: Easing.OutQuad } }
            Behavior on y { enabled: gridView.restack; NumberAnimation { duration: 120; easing.type: Easing.OutQuad } }
        }
        highlightFollowsCurrentItem: true
        highlightMoveDuration: 0

        onCountChanged: {
            if (count > 0 && itemGrid.grabFocus) {
                currentIndex = 0;
                focus = true;
                forceActiveFocus();
            }
        }

        onModelChanged: currentIndex = -1

        PlasmaComponents.ScrollBar.vertical: PlasmaComponents.ScrollBar {
            policy: itemGrid.verticalScrollBarPolicy
        }

        // ---- input handlers ------------------------------------------------
        // Handlers observe; the Flickable keeps the grab. Order matters only
        // for readability — none of them block each other.

        HoverHandler {
            id: hoverHandler
            acceptedDevices: PointerDevice.Mouse
            onPointChanged: {
                const idx = itemGrid.indexBeneath(point.position);
                if (idx !== -1) {
                    gridView.currentIndex = idx;
                    itemGrid.focus = true;
                }
            }
        }

        TapHandler {
            id: tapHandler
            acceptedButtons: Qt.LeftButton
            // DragThreshold: a press that moves becomes the Flickable's drag
            // and this handler cleanly gives up — taps and flicks can't blur.
            gesturePolicy: TapHandler.DragThreshold

            onPressedChanged: {
                gridView.pressedIndex = pressed
                    ? itemGrid.indexBeneath(point.pressPosition) : -1;
            }

            onTapped: (eventPoint, button) => {
                const idx = itemGrid.indexBeneath(eventPoint.position);
                if (idx !== -1) {
                    itemGrid.activate(idx);
                }
                // Empty grid space: deliberately nothing. The old build
                // dismissed here, which made every missed tap a punishment.
            }

            onLongPressed: {
                const idx = itemGrid.indexBeneath(point.position);
                if (idx !== -1) {
                    gridView.currentIndex = idx;
                    itemGrid.openContextMenu(idx, point.position);
                }
                gridView.pressedIndex = -1;
            }
        }

        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: eventPoint => {
                const idx = itemGrid.indexBeneath(eventPoint.position);
                if (idx !== -1) {
                    gridView.currentIndex = idx;
                    itemGrid.openContextMenu(idx, eventPoint.position);
                } else {
                    const mapped = itemGrid.mapToItem(rootItem, eventPoint.position.x, eventPoint.position.y);
                    contextMenu.open(mapped.x, mapped.y);
                }
            }
        }

        // ---- keyboard navigation (unchanged semantics from the fork) ------
        Keys.onLeftPressed: event => {
            if (itemGrid.currentCol() !== 0) {
                event.accepted = true;
                moveCurrentIndexLeft();
            } else {
                itemGrid.keyNavLeft();
            }
        }
        Keys.onRightPressed: event => {
            if (itemGrid.currentCol() !== itemGrid.columns - 1 && currentIndex !== count - 1) {
                event.accepted = true;
                moveCurrentIndexRight();
            } else {
                itemGrid.keyNavRight();
            }
        }
        Keys.onUpPressed: event => {
            if (itemGrid.currentRow() !== 0) {
                event.accepted = true;
                moveCurrentIndexUp();
                positionViewAtIndex(currentIndex, GridView.Contain);
            } else {
                itemGrid.keyNavUp();
            }
        }
        Keys.onDownPressed: event => {
            if (itemGrid.currentRow() < itemGrid.lastRow()) {
                // moveCurrentIndexDown() can't step into a partial last row.
                event.accepted = true;
                currentIndex = Math.min(currentIndex + itemGrid.columns, count - 1);
                positionViewAtIndex(currentIndex, GridView.Visible);
            } else {
                itemGrid.keyNavDown();
            }
        }
    }
}
