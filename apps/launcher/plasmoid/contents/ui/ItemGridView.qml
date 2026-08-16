import QtQuick 2.15
import QtQuick.Controls
import org.kde.kquickcontrolsaddons 2.0
import org.kde.ksvg 1.0 as KSvg
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras 2.0 as PlasmaExtras
import org.kde.kirigami 2.20 as Kirigami

FocusScope {
    id: itemGrid
    
    signal keyNavLeft
    signal keyNavRight
    signal keyNavUp
    signal keyNavDown
    signal itemActivated(int index, string actionId, string argument)
    
    property bool dragEnabled: true
    property bool dropEnabled: false
    property bool showLabels: true
    property bool grabFocus: false
    
    // External dependencies - must be provided by parent
    property var dragHelper: null
    property var kickerInstance: null
    property Item rootItem: null
    
    property alias currentIndex: gridView.currentIndex
    property alias currentItem: gridView.currentItem
    property alias contentItem: gridView.contentItem
    property alias count: gridView.count
    property alias model: gridView.model
    property alias verticalLayoutDirection: gridView.verticalLayoutDirection
    property alias cellWidth: gridView.cellWidth
    property alias cellHeight: gridView.cellHeight
    property alias iconSize: gridView.iconSize
    property var horizontalScrollBarPolicy: PlasmaComponents.ScrollBar.AlwaysOff
    property var verticalScrollBarPolicy: PlasmaComponents.ScrollBar.AsNeeded
    
    onDropEnabledChanged: {
        if (!dropEnabled && "dropPlaceHolderIndex" in model) {
            model.dropPlaceHolderIndex = -1
        }
    }
    
    onFocusChanged: {
        if (!focus) currentIndex = -1
    }
    
    function currentRow() {
        return currentIndex === -1 ? -1 : Math.floor(currentIndex / Math.floor(width / itemGrid.cellWidth))
    }
    
    function currentCol() {
        return currentIndex === -1 ? -1 : currentIndex - (currentRow() * Math.floor(width / itemGrid.cellWidth))
    }
    
    function lastRow() {
        const columns = Math.floor(width / itemGrid.cellWidth)
        return Math.ceil(count / columns) - 1
    }
    
    function tryActivate(row, col) {
        if (count) {
            const columns = Math.floor(width / itemGrid.cellWidth)
            const rows = Math.ceil(count / columns)
            row = Math.min(row, rows - 1)
            col = Math.min(col, columns - 1)
            currentIndex = Math.min(row ? ((Math.max(1, row) * columns) + col) : col, count - 1)
            focus = true
        }
    }
    
    function forceLayout() {
        gridView.forceLayout()
    }
    
    ActionMenu {
        id: actionMenu
        onActionClicked: {
            visualParent.actionTriggered(actionId, actionArgument)
        }
    }
    
    DropArea {
        id: dropArea
        anchors.fill: parent
        
        onPositionChanged: event => {
            if (!itemGrid.dropEnabled || gridView.animating || !itemGrid.kickerInstance || !itemGrid.kickerInstance.dragSource) return
            
            const x = Math.max(0, event.x - (width % itemGrid.cellWidth))
            const cPos = mapToItem(gridView.contentItem, x, event.y)
            const item = gridView.itemAt(cPos.x, cPos.y)
            
            if (item) {
                if (itemGrid.kickerInstance.dragSource.parent === gridView.contentItem) {
                    if (item !== itemGrid.kickerInstance.dragSource) {
                        item.GridView.view.model.moveRow(itemGrid.kickerInstance.dragSource.itemIndex, item.itemIndex)
                    }
                } else if (itemGrid.kickerInstance.dragSource.GridView.view.model.favoritesModel === itemGrid.model &&
                          !itemGrid.model.isFavorite(itemGrid.kickerInstance.dragSource.favoriteId)) {
                    const hasPlaceholder = (itemGrid.model.dropPlaceholderIndex !== -1)
                    itemGrid.model.dropPlaceholderIndex = item.itemIndex
                    if (!hasPlaceholder) gridView.currentIndex = (item.itemIndex - 1)
                }
            } else if (itemGrid.kickerInstance.dragSource.parent !== gridView.contentItem &&
                      itemGrid.kickerInstance.dragSource.GridView.view.model.favoritesModel === itemGrid.model &&
                      !itemGrid.model.isFavorite(itemGrid.kickerInstance.dragSource.favoriteId)) {
                const hasPlaceholder = (itemGrid.model.dropPlaceholderIndex !== -1)
                itemGrid.model.dropPlaceholderIndex = hasPlaceholder ? itemGrid.model.count - 1 : itemGrid.model.count
                if (!hasPlaceholder) gridView.currentIndex = (itemGrid.model.count - 1)
            } else {
                if ("dropPlaceholderIndex" in itemGrid.model) {
                    itemGrid.model.dropPlaceholderIndex = -1
                }
                gridView.currentIndex = -1
            }
        }
        
        onExited: {
            if ("dropPlaceholderIndex" in itemGrid.model) {
                itemGrid.model.dropPlaceholderIndex = -1
                gridView.currentIndex = -1
            }
        }
        
        onDropped: {
            if (itemGrid.kickerInstance && itemGrid.kickerInstance.dragSource && 
                itemGrid.kickerInstance.dragSource.parent !== gridView.contentItem && 
                itemGrid.kickerInstance.dragSource.GridView.view.model.favoritesModel === itemGrid.model) {
                itemGrid.model.addFavorite(itemGrid.kickerInstance.dragSource.favoriteId, itemGrid.model.dropPlaceholderIndex)
                gridView.currentIndex = -1
            }
        }
        
        Timer {
            id: resetAnimationDurationTimer
            interval: 120
            repeat: false
            onTriggered: gridView.animationDuration = interval - 20
        }
        
        GridView {
            id: gridView
            anchors.fill: parent
            
            signal itemContainsMouseChanged(bool containsMouse)
            
            property int iconSize: Kirigami.Units.iconSizes.huge
            property bool animating: false
            
            // ─── Restack timing ────────────────────────────────────────
            // The view picks this up on add/remove/move/displaced transitions.
            // Pulled from config so users can tune the feel.
            readonly property int restackDuration: 
                Plasmoid.configuration.enableAnimations
                ? (Plasmoid.configuration.restackDuration || 350)
                : 0
            
            readonly property int restackStagger:
                Plasmoid.configuration.restackStagger !== undefined
                ? Plasmoid.configuration.restackStagger
                : 12
            
            focus: true
            currentIndex: -1
            snapMode: GridView.NoSnap
            boundsBehavior: Flickable.StopAtBounds
            keyNavigationWraps: false
            cacheBuffer: 400   // keep more items realized so restack motion is visible
            reuseItems: true   // reuse delegates rather than destroying/creating
            
            // ─── Add: tiles arriving (e.g., after search clears) ──────
            add: Transition {
                id: addTransition
                enabled: gridView.restackDuration > 0
                
                SequentialAnimation {
                    PropertyAction { target: gridView; property: "animating"; value: true }
                    // Stagger by ViewTransition.index so a wave of additions
                    // doesn't all explode at once
                    PauseAnimation { 
                        duration: addTransition.ViewTransition ? 
                                  Math.min(addTransition.ViewTransition.index * gridView.restackStagger, 600)
                                  : 0
                    }
                    ParallelAnimation {
                        NumberAnimation {
                            properties: "opacity"
                            from: 0; to: 1
                            duration: gridView.restackDuration
                            easing.type: Easing.OutCubic
                        }
                        NumberAnimation {
                            properties: "scale"
                            from: 0.6; to: 1.0
                            duration: gridView.restackDuration
                            easing.type: Easing.OutBack
                            easing.overshoot: 1.2
                        }
                    }
                    PropertyAction { target: gridView; property: "animating"; value: false }
                }
            }
            
            // ─── Remove: tiles leaving (e.g., filtered out) ───────────
            remove: Transition {
                id: removeTransition
                enabled: gridView.restackDuration > 0
                
                ParallelAnimation {
                    NumberAnimation {
                        properties: "opacity"
                        to: 0
                        duration: gridView.restackDuration * 0.6
                        easing.type: Easing.InCubic
                    }
                    NumberAnimation {
                        properties: "scale"
                        to: 0.4
                        duration: gridView.restackDuration * 0.6
                        easing.type: Easing.InQuad
                    }
                }
            }
            
            // ─── Move: tiles changing position (e.g., reordering) ─────
            move: Transition {
                id: moveTransition
                enabled: gridView.restackDuration > 0
                
                SequentialAnimation {
                    PropertyAction { target: gridView; property: "animating"; value: true }
                    NumberAnimation {
                        properties: "x,y"
                        duration: gridView.restackDuration
                        easing.type: Easing.OutCubic
                    }
                    PropertyAction { target: gridView; property: "animating"; value: false }
                }
            }
            
            // ─── Displaced: tiles pushed by add/remove/move ───────────
            // This is the key one for "restack" — when one tile leaves,
            // every tile after it slides into the new position.
            displaced: Transition {
                id: displacedTransition
                enabled: gridView.restackDuration > 0
                
                SequentialAnimation {
                    PropertyAction { target: gridView; property: "animating"; value: true }
                    // Slight stagger so the wave is visible rather than monolithic
                    PauseAnimation {
                        duration: displacedTransition.ViewTransition ?
                                  Math.min(displacedTransition.ViewTransition.index * gridView.restackStagger / 2, 200)
                                  : 0
                    }
                    NumberAnimation {
                        properties: "x,y"
                        duration: gridView.restackDuration
                        easing.type: Easing.OutCubic
                    }
                    PropertyAction { target: gridView; property: "animating"; value: false }
                }
            }
            
            // ─── AddDisplaced and MoveDisplaced ────────────────────────
            // These fire when an add or move pushes other items aside.
            addDisplaced: Transition {
                id: addDisplacedTransition
                enabled: gridView.restackDuration > 0
                
                NumberAnimation {
                    properties: "x,y"
                    duration: gridView.restackDuration
                    easing.type: Easing.OutCubic
                }
            }
            
            moveDisplaced: Transition {
                id: moveDisplacedTransition
                enabled: gridView.restackDuration > 0
                
                NumberAnimation {
                    properties: "x,y"
                    duration: gridView.restackDuration * 0.9
                    easing.type: Easing.OutCubic
                }
            }
            
            // ─── Population: first time the view is populated ─────────
            populate: Transition {
                id: populateTransition
                enabled: gridView.restackDuration > 0 && Plasmoid.configuration.iconAnimateOnAppear !== false
                
                SequentialAnimation {
                    PauseAnimation {
                        duration: populateTransition.ViewTransition ?
                                  populateTransition.ViewTransition.index * gridView.restackStagger
                                  : 0
                    }
                    ParallelAnimation {
                        NumberAnimation {
                            properties: "opacity"
                            from: 0; to: 1
                            duration: gridView.restackDuration
                            easing.type: Easing.OutQuad
                        }
                        NumberAnimation {
                            properties: "scale"
                            from: 0.7; to: 1.0
                            duration: gridView.restackDuration
                            easing.type: Easing.OutBack
                            easing.overshoot: 1.15
                        }
                    }
                }
            }
            
            delegate: ItemGridDelegate {
                showLabel: itemGrid.showLabels
            }
            
            highlight: Rectangle {
                color: Qt.rgba(Kirigami.Theme.highlightColor.r,
                              Kirigami.Theme.highlightColor.g,
                              Kirigami.Theme.highlightColor.b, 0.65)
                radius: 10
                
                // The highlight itself slides smoothly between items
                Behavior on x { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                Behavior on width { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
                Behavior on height { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
            }
            
            highlightFollowsCurrentItem: true
            highlightMoveDuration: 0  // We do it via Behavior above for more control
            highlightResizeDuration: 0
            
            onCurrentIndexChanged: {
                if (currentIndex !== -1) {
                    hoverArea.hoverEnabled = false
                    focus = true
                }
            }
            
            onCountChanged: {
                if (count > 0 && itemGrid.grabFocus) {
                    currentIndex = 0
                    focus = true
                    forceActiveFocus()
                }
            }
            
            onModelChanged: {
                currentIndex = -1
            }
            
            Keys.onLeftPressed: event => {
                if (itemGrid.currentCol() !== 0) {
                    event.accepted = true
                    moveCurrentIndexLeft()
                } else {
                    itemGrid.keyNavLeft()
                }
            }
            
            Keys.onRightPressed: event => {
                const columns = Math.floor(width / cellWidth)
                if (itemGrid.currentCol() !== columns - 1 && currentIndex !== count - 1) {
                    event.accepted = true
                    moveCurrentIndexRight()
                } else {
                    itemGrid.keyNavRight()
                }
            }
            
            Keys.onUpPressed: event => {
                if (itemGrid.currentRow() !== 0) {
                    event.accepted = true
                    moveCurrentIndexUp()
                    positionViewAtIndex(currentIndex, GridView.Contain)
                } else {
                    itemGrid.keyNavUp()
                }
            }
            
            Keys.onDownPressed: event => {
                if (itemGrid.currentRow() < itemGrid.lastRow()) {
                    event.accepted = true
                    const columns = Math.floor(itemGrid.width / cellWidth)
                    const newIndex = currentIndex + columns
                    currentIndex = Math.min(newIndex, gridView.count - 1)
                    positionViewAtIndex(currentIndex, GridView.Visible)
                } else {
                    itemGrid.keyNavDown()
                }
            }
            
            onItemContainsMouseChanged: containsMouse => {
                if (!containsMouse) {
                    hoverArea.pressX = -1
                    hoverArea.pressY = -1
                    hoverArea.lastX = -1
                    hoverArea.lastY = -1
                    hoverArea.pressedItem = null
                    hoverArea.hoverEnabled = true
                }
            }
        }
        
        // Unified touch/mouse handler
        MouseArea {
            id: hoverArea
            anchors.fill: parent
            
            property int pressX: -1
            property int pressY: -1
            property int lastX: -1
            property int lastY: -1
            property Item pressedItem: null
            
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            hoverEnabled: true
            
            function updatePositionProperties(x, y) {
                if (lastX === x && lastY === y) return
                
                lastX = x
                lastY = y
                const cPos = mapToItem(gridView.contentItem, x, y)
                const item = gridView.itemAt(cPos.x, cPos.y)
                
                if (!item) {
                    gridView.currentIndex = -1
                    pressedItem = null
                } else {
                    itemGrid.focus = (item.itemIndex !== -1)
                    itemGrid.forceActiveFocus()
                    gridView.currentIndex = item.itemIndex
                }
                return item
            }
            
            onPressed: mouse => {
                mouse.accepted = true
                updatePositionProperties(mouse.x, mouse.y)
                pressX = mouse.x
                pressY = mouse.y
                
                if (mouse.button === Qt.RightButton) {
                    if (gridView.currentItem && gridView.currentItem.hasActionList) {
                        const mapped = mapToItem(gridView.currentItem, mouse.x, mouse.y)
                        gridView.currentItem.openActionMenu(mapped.x, mapped.y)
                    }
                } else {
                    pressedItem = gridView.currentItem
                }
            }
            
            onReleased: mouse => {
                mouse.accepted = true
                updatePositionProperties(mouse.x, mouse.y)
                
                if (itemGrid.dragHelper && !itemGrid.dragHelper.dragging) {
                    if (pressedItem) {
                        if ("trigger" in gridView.model) {
                            gridView.model.trigger(pressedItem.itemIndex, "", null)
                            if (itemGrid.rootItem && "toggle" in itemGrid.rootItem) {
                                itemGrid.rootItem.toggle()
                            }
                        }
                        itemGrid.itemActivated(pressedItem.itemIndex, "", null)
                    }
                }
                
                pressX = pressY = -1
                pressedItem = null
            }
            
            onPositionChanged: mouse => {
                const item = pressedItem ? pressedItem : updatePositionProperties(mouse.x, mouse.y)
                
                if (gridView.currentIndex !== -1 && itemGrid.dragEnabled && itemGrid.dragHelper && itemGrid.kickerInstance) {
                    if (pressX !== -1 && itemGrid.dragHelper.isDrag(pressX, pressY, mouse.x, mouse.y)) {
                        if ("pluginName" in item.m) {
                            itemGrid.dragHelper.startDrag(itemGrid.kickerInstance, item.url, item.icon,
                                               "text/x-plasmoidservicename", item.m.pluginName)
                        } else {
                            itemGrid.dragHelper.startDrag(itemGrid.kickerInstance, item.url)
                        }
                        itemGrid.kickerInstance.dragSource = item
                        pressX = -1
                        pressY = -1
                    }
                }
            }
        }
        
        // Swipe gesture for pagination
        DragHandler {
            id: swipeGesture
            target: null
            xAxis.enabled: true
            yAxis.enabled: false
            
            property real startContentX: 0
            
            onActiveChanged: {
                if (active) {
                    startContentX = gridView.contentX
                } else {
                    const deltaX = translation.x
                    if (Math.abs(deltaX) > 40) {
                        if (deltaX > 0) {
                            gridView.contentX = Math.max(startContentX - gridView.width, 0)
                        } else {
                            gridView.contentX = Math.min(startContentX + gridView.width,
                                                        gridView.contentWidth - gridView.width)
                        }
                    }
                }
            }
        }
    }
}
