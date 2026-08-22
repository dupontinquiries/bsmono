/*
 * DashboardRepresentation.qml — the fullscreen dashboard.
 *
 * Touch fixes vs. the previous iteration:
 *  · Tap-to-close is a TapHandler, not MouseArea.onClicked. TapHandler has a
 *    built-in drag threshold, so a finger that lands near a tile and slides a
 *    few px is a (failed) drag, not a "tap on empty space" — this was the main
 *    "taps close the window" path. Right-click context menu keeps its own
 *    TapHandler.
 *  · Swipe-down-to-close is one DragHandler on the background. It only ever
 *    sees drags the grid/search did not claim (parent handlers get the
 *    leftovers), so it cannot race the GridView flick.
 *  · The search field's MultiPointTouchArea + MouseArea double-hack (double
 *    focus sets, scale fiddling from two owners) is one TapHandler:
 *    forceActiveFocus + Qt.inputMethod.show(), and the visual reaction is
 *    driven only by activeFocus. The broken updateSelection() helper is gone.
 *  · keyboardHeight is a ROOT property with de-duped reports. The old copy
 *    lived on an inner Column, so `keyboardHeight` in allAppsColumn resolved to
 *    nothing (ReferenceError) and the grid never actually padded for maliit.
 *  · onTextChanged no longer forces focus — typing already has it, and the
 *    forced re-focus fought grid keyboard-navigation while results updated.
 *
 * The improved ItemGridView takes its collaborators as properties
 * (dragHelper / kickerInstance / rootItem) instead of reaching for globals;
 * they are wired at both instantiations below.
 */
import QtQuick 2.15
import QtQml 2.15
import org.kde.kwindowsystem 1.0
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras 2.0 as PlasmaExtras
import org.kde.plasma.core as PlasmaCore
import org.kde.kirigami 2.20 as Kirigami
import QtQuick.Layouts 1.1
import org.kde.plasma.plasmoid 2.0
import org.kde.plasma.private.kicker 0.1 as Kicker
import QtQuick.Controls

Kicker.DashboardWindow {
    id: root

    property bool smallScreen: ((Math.floor(width / Kirigami.Units.iconSizes.huge) <= 22)
                                || (Math.floor(height / Kirigami.Units.iconSizes.huge) <= 14))

    property int iconSize: {
        switch (Plasmoid.configuration.appsIconSize) {
        case 0: return Kirigami.Units.iconSizes.smallMedium;
        case 1: return Kirigami.Units.iconSizes.medium;
        case 2: return Kirigami.Units.iconSizes.large;
        case 3: return Kirigami.Units.iconSizes.huge;
        case 4: return Kirigami.Units.iconSizes.large * 2;
        case 5: return Kirigami.Units.iconSizes.enormous;
        default: return Kirigami.Units.iconSizes.large;
        }
    }

    property int systemIconSize: {
        switch (Plasmoid.configuration.systemIconSize) {
        case 0: return Kirigami.Units.iconSizes.smallMedium;
        case 1: return Kirigami.Units.iconSizes.medium;
        case 2: return Kirigami.Units.iconSizes.large;
        case 3: return Kirigami.Units.iconSizes.huge;
        case 4: return Kirigami.Units.iconSizes.enormous;
        default: return Kirigami.Units.iconSizes.medium;
        }
    }

    property int cellSize: iconSize + (2 * Kirigami.Units.iconSizes.sizeForLabels)
                           + (2 * Kirigami.Units.largeSpacing)
                           + (2 * Math.max(highlightItemSvg.margins.top + highlightItemSvg.margins.bottom,
                                           highlightItemSvg.margins.left + highlightItemSvg.margins.right))

    property int actualRows: Plasmoid.configuration.useCustomGridSize
                             ? Math.min(Plasmoid.configuration.gridRows, Math.floor(height * 0.7 / cellSize))
                             : Math.floor(height * 0.7 / cellSize)
    property int actualColumns: Plasmoid.configuration.useCustomGridSize
                                ? Math.min(Plasmoid.configuration.gridColumns, Math.floor(width * 0.7 / cellSize))
                                : Math.floor(width * 0.7 / cellSize)

    property int neededRows: (allAppsGrid.model && allAppsGrid.model.count > 0)
                             ? Math.ceil(allAppsGrid.model.count / actualColumns)
                             : actualRows
    property int finalRows: Math.max(1, Math.min(actualRows, neededRows))

    property bool searching: searchField.text !== ""

    // ─── Virtual keyboard (maliit) ─────────────────────────────────────
    // One owner, root scope, de-duped: every child scope resolves the same
    // value, and identical rectangle re-reports from the compositor don't
    // relayout anything.
    property real keyboardHeight: 0
    Connections {
        target: Qt.inputMethod
        function onVisibleChanged() { root.updateKeyboardHeight(); }
        function onKeyboardRectangleChanged() { root.updateKeyboardHeight(); }
    }
    function updateKeyboardHeight() {
        const h = Qt.inputMethod.visible ? Qt.inputMethod.keyboardRectangle.height : 0;
        if (h !== keyboardHeight) {
            keyboardHeight = h;
        }
    }

    backgroundColor: "transparent"

    onKeyEscapePressed: {
        if (searching) {
            searchField.clear();
        } else {
            root.toggle();
        }
    }

    onVisibleChanged: {
        if (visible) {
            animatorMainColumn.start();
        } else {
            rootItem.opacity = 0;
        }
        reset();
    }

    onSearchingChanged: {
        if (!searching) {
            mainView.pop();
            reset();
        } else {
            mainView.push(runnerComponent);
        }
    }

    function colorWithAlpha(color: color, alpha: real): color {
        return Qt.rgba(color.r, color.g, color.b, alpha)
    }

    function reset() {
        allAppsGrid.model = rootModel.modelForRow(2);
        allAppsGrid.currentIndex = -1;
        systemFavoritesGrid.currentIndex = -1;
        allAppsGrid.forceLayout();
        searchField.clear();
        searchField.focus = true;
    }

    mainItem: Item {
        id: rootItem

        anchors.fill: parent

        LayoutMirroring.enabled: Qt.application.layoutDirection === Qt.RightToLeft
        LayoutMirroring.childrenInherit: true

        Rectangle {
            anchors.fill: parent
            color: Plasmoid.configuration.useCustomBackgroundColor
                   ? Plasmoid.configuration.customBackgroundColor
                   : Kirigami.Theme.backgroundColor
            opacity: Plasmoid.configuration.backgroundOpacity
        }

        // ─── Background gestures ──────────────────────────────────────
        // These are parent-level handlers: they only receive events that no
        // child (grid, search field, system row) claimed. A tap that slid
        // past TapHandler's threshold is neither a tap nor a swipe — it does
        // nothing, which is the fix for accidental closes.
        TapHandler {
            acceptedButtons: Qt.LeftButton
            onTapped: root.toggle()
        }
        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: (eventPoint) => {
                contextMenu.open(eventPoint.position.x, eventPoint.position.y);
            }
        }
        DragHandler {
            id: dismissDrag
            target: null
            xAxis.enabled: false
            yAxis.enabled: true
            onActiveChanged: {
                if (!active && translation.y > Kirigami.Units.gridUnit * 5) {
                    root.toggle();
                }
            }
        }

        Connections {
            target: kicker
            function onReset() {
            }
            function onDragSourceChanged() {
                if (!kicker.dragSource) {
                    // FIXME TODO HACK: Reset all views post-DND to work around
                    // mouse grab bug despite QQuickWindow::mouseGrabberItem==0x0.
                    rootModel.refresh();
                }
            }
        }

        Connections {
            target: Plasmoid
            function onUserConfiguringChanged() {
                if (Plasmoid.userConfiguring) {
                    root.hide()
                }
            }
        }

        PlasmaExtras.Menu {
            id: contextMenu
            PlasmaExtras.MenuItem {
                action: Plasmoid.internalAction("configure")
            }
        }

        Kicker.ContainmentInterface {
            id: containmentInterface
        }

        OpacityAnimator { id: animatorMainColumn; from: 0; to: 1; target: rootItem }

        // ─── Search field ─────────────────────────────────────────────
        // Anchored above the keyboard: the whole column rides keyboardHeight.
        Column {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            anchors.bottomMargin: root.keyboardHeight + Kirigami.Units.largeSpacing

            Behavior on anchors.bottomMargin {
                NumberAnimation { duration: 200; easing.type: Easing.OutQuad }
            }

            TextField {
                id: searchField

                anchors.horizontalCenter: parent.horizontalCenter
                width: Kirigami.Units.gridUnit * 13
                topPadding: Kirigami.Units.largeSpacing
                bottomPadding: Kirigami.Units.largeSpacing
                placeholderText: i18nc("@info:placeholder as in, 'start typing to initiate a search'", "Search")
                horizontalAlignment: TextInput.AlignHCenter
                wrapMode: Text.NoWrap
                font.pointSize: Kirigami.Theme.defaultFont.pointSize + 0.5
                focus: true
                activeFocusOnTab: true

                // One owner for the focus visuals: activeFocus. No handler
                // fights another for scale/opacity.
                opacity: activeFocus ? 1.0 : 0.9
                scale: activeFocus ? 1.05 : 1.0
                Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutQuad } }
                Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutQuad } }

                onTextChanged: {
                    runnerModel.query = text;
                }

                function clear() {
                    text = "";
                    focus = true;
                }

                function appendText(newText) {
                    if (!root.visible) return;
                    text += newText;
                    focus = true;
                }

                function backspace() {
                    if (!root.visible) return;
                    text = text.slice(0, -1);
                    focus = true;
                }

                // One tap = focus + explicitly ask maliit to show. A tap that
                // turns into a drag scrolls whatever is behind, not this.
                TapHandler {
                    onTapped: {
                        searchField.forceActiveFocus();
                        Qt.inputMethod.show();
                    }
                }

                Keys.onPressed: event => {
                    if (event.key === Qt.Key_Down || event.key === Qt.Key_Tab) {
                        event.accepted = true;
                        if (root.searching) {
                            mainView.currentItem.tryActivate(0, 0);
                            mainView.currentItem.forceActiveFocus();
                        } else {
                            allAppsGrid.tryActivate(0, 0);
                            allAppsGrid.forceActiveFocus();
                        }
                    }
                }

                background: Rectangle {
                    color: colorWithAlpha(Kirigami.Theme.backgroundColor, 0.7)
                    radius: 100
                    border.width: 1
                    border.color: colorWithAlpha(Kirigami.Theme.textColor, 0.05)
                }
            }
        }

        // ─── App grid / search results ────────────────────────────────
        StackView {
            id: mainView

            width: (root.actualColumns * root.cellSize) + Kirigami.Units.largeSpacing
            height: (root.finalRows * root.cellSize) + Kirigami.Units.largeSpacing * 2

            anchors {
                horizontalCenter: parent.horizontalCenter
                verticalCenter: parent.verticalCenter
                verticalCenterOffset: -root.keyboardHeight / 2 -
                                      (Plasmoid.configuration.showSystemIcons === 1
                                       ? (systemFavoritesGrid.height + systemFavoritesGrid.anchors.bottomMargin) / 2 : 0)
            }
            Behavior on anchors.verticalCenterOffset {
                NumberAnimation { duration: 200; easing.type: Easing.OutQuad }
            }

            initialItem: ItemGridView {
                id: allAppsGrid

                dragHelper: dragHelper
                kickerInstance: kicker
                rootItem: root

                width: root.actualColumns * root.cellSize
                height: root.finalRows * root.cellSize
                cellWidth: root.cellSize
                cellHeight: root.cellSize
                iconSize: root.iconSize
                dropEnabled: false
                clip: true

                verticalScrollBarPolicy: (model && model.count > root.finalRows * root.actualColumns)
                                         ? PlasmaComponents.ScrollBar.AsNeeded
                                         : PlasmaComponents.ScrollBar.AlwaysOff
                horizontalScrollBarPolicy: PlasmaComponents.ScrollBar.AlwaysOff

                onKeyNavUp: {
                    allAppsGrid.focus = false;
                    searchField.focus = true;
                }

                Keys.onPressed: event => {
                    if (event.key === Qt.Key_Tab) {
                        event.accepted = true;
                        allAppsGrid.focus = false;
                        if (Plasmoid.configuration.showSystemIcons === 1) {
                            systemFavoritesGrid.tryActivate(0, 0);
                            systemFavoritesGrid.forceActiveFocus();
                        } else {
                            searchField.focus = true;
                        }
                    }
                }
            }

            pushEnter: Transition { PropertyAnimation { property: "opacity"; from: 0; to: 1; duration: 200 } }
            pushExit:  Transition { PropertyAnimation { property: "opacity"; from: 1; to: 0; duration: 200 } }
            popEnter:  Transition { PropertyAnimation { property: "opacity"; from: 0; to: 1; duration: 200 } }
            popExit:   Transition { PropertyAnimation { property: "opacity"; from: 1; to: 0; duration: 200 } }
        }

        Component {
            id: runnerComponent

            ItemGridView {
                id: runnerGrid

                dragHelper: dragHelper
                kickerInstance: kicker
                rootItem: root

                anchors.centerIn: parent
                width: mainView.width
                height: mainView.height
                clip: true
                grabFocus: true
                cellWidth: root.cellSize
                cellHeight: root.cellSize
                iconSize: root.iconSize

                model: runnerModel.count > 0 ? runnerModel.modelForRow(0) : undefined

                onKeyNavUp: {
                    runnerGrid.focus = false;
                    searchField.focus = true;
                }

                Keys.onPressed: event => {
                    if (event.key === Qt.Key_Tab) {
                        event.accepted = true;
                        runnerGrid.focus = false;
                        if (Plasmoid.configuration.showSystemIcons === 1) {
                            systemFavoritesGrid.tryActivate(0, 0);
                            systemFavoritesGrid.forceActiveFocus();
                        } else {
                            searchField.focus = true;
                        }
                    }
                }
            }
        }

        // ─── System favorites row ─────────────────────────────────────
        Rectangle {
            anchors.centerIn: systemFavoritesGrid
            height: systemFavoritesGrid.height + Kirigami.Units.largeSpacing
            width: systemFavoritesGrid.width + Kirigami.Units.largeSpacing
            color: Kirigami.Theme.backgroundColor
            radius: 10
            opacity: 0.6
            z: 1
            visible: Plasmoid.configuration.showSystemIcons === 1
        }

        ItemGridView {
            id: systemFavoritesGrid

            dragHelper: dragHelper
            kickerInstance: kicker
            rootItem: root

            visible: Plasmoid.configuration.showSystemIcons === 1
            anchors.bottom: parent.bottom
            anchors.bottomMargin: Kirigami.Units.largeSpacing
            anchors.horizontalCenter: parent.horizontalCenter
            clip: true
            width: systemFavoritesGrid.model
                   ? Math.min(Math.floor((root.width * 0.85) / cellWidth) * cellWidth,
                              systemFavoritesGrid.model.count * cellWidth)
                   : 0
            height: cellHeight
            cellWidth: root.systemIconSize + Kirigami.Units.largeSpacing * 2
            cellHeight: cellWidth
            iconSize: root.systemIconSize
            z: 2
            showLabels: false
            model: systemFavorites
            dragEnabled: false
            dropEnabled: false
            verticalScrollBarPolicy: PlasmaComponents.ScrollBar.AlwaysOff

            Keys.onPressed: event => {
                if (event.key === Qt.Key_Tab) {
                    event.accepted = true;
                    systemFavoritesGrid.focus = false;
                    searchField.focus = true;
                }
            }

            onKeyNavUp: {
                systemFavoritesGrid.focus = false;
                if (root.searching) {
                    mainView.currentItem.tryActivate(0, 0);
                    mainView.currentItem.forceActiveFocus();
                } else {
                    allAppsGrid.tryActivate(0, 0);
                    allAppsGrid.forceActiveFocus();
                }
            }
        }

        // Physical-keyboard type-to-search (the grid forwards what it doesn't
        // use). Search field must not already have focus, or we double-insert.
        Keys.onPressed: event => {
            if (event.modifiers & Qt.ControlModifier || event.modifiers & Qt.ShiftModifier) {
                searchField.focus = true;
                return;
            }
            if (event.key === Qt.Key_Escape) {
                event.accepted = true;
                if (root.searching) {
                    reset();
                } else {
                    root.visible = false;
                }
                return;
            }
            if (searchField.focus) {
                return;
            }
            if (event.key === Qt.Key_Backspace) {
                event.accepted = true;
                searchField.backspace();
            } else if (event.text !== "") {
                event.accepted = true;
                searchField.appendText(event.text);
            }
        }
    }

    Component.onCompleted: {
        rootModel.refresh();
        searchField.forceActiveFocus();
    }
}
