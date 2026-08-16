import QtQuick 2.15
import QtQuick.Controls
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras 2.0 as PlasmaExtras
import org.kde.kirigami 2.20 as Kirigami
import org.kde.plasma.plasmoid 2.0
import org.kde.plasma.private.kicker 0.1 as Kicker

import "code/tools.js" as Tools

/* The fullscreen dash. Structural changes from the old file, each tied to a
   touch bug:

   - rootItem was a MouseArea whose onClicked toggled on ANY unconsumed left
     click — combined with the grid's leaks, missed taps closed the window.
     It's a FocusScope now; background dismissal is a TapHandler with
     gesturePolicy DragThreshold (real taps only, outside the grid), plus a
     swipe-down DragHandler with visual follow.
   - searchField was buried under a MultiPointTouchArea + MouseArea that ate
     cursor placement and text selection. Both gone; a TapHandler only
     re-summons maliit when the field is already focused (the case Qt won't
     do itself), and native TextField input works again.
   - updateSelection() referenced an undefined id (searchHeading) — deleted.
   - Keyboard avoidance unified on kicker.keyboardHeight: the grid stack
     shifts up by half, search rides above the keyboard, the system row hides
     while typing (it would be under maliit anyway). */
Kicker.DashboardWindow {
    id: root

    property bool smallScreen: ((Math.floor(width / Kirigami.Units.iconSizes.huge) <= 22)
                                || (Math.floor(height / Kirigami.Units.iconSizes.huge) <= 14))

    property int iconSize: {
        switch (Plasmoid.configuration.appsIconSize) {
        case 0: return Kirigami.Units.iconSizes.smallMedium / 4;
        case 1: return Kirigami.Units.iconSizes.medium / 4;
        case 2: return Kirigami.Units.iconSizes.large / 4;
        case 3: return Kirigami.Units.iconSizes.huge / 4;
        case 4: return Kirigami.Units.iconSizes.large * 2 / 4;
        case 5: return Kirigami.Units.iconSizes.enormous / 4;
        default: return 32;
        }
    }

    property int systemIconSize: {
        switch (Plasmoid.configuration.systemIconSize) {
        case 0: return Kirigami.Units.iconSizes.smallMedium / 4;
        case 1: return Kirigami.Units.iconSizes.medium / 4;
        case 2: return Kirigami.Units.iconSizes.large / 4;
        case 3: return Kirigami.Units.iconSizes.huge / 4;
        case 4: return Kirigami.Units.iconSizes.enormous / 4;
        default: return 32;
        }
    }

    property int cellSize: iconSize + (2 * Kirigami.Units.iconSizes.sizeForLabels)
                           + (2 * Kirigami.Units.largeSpacing)
                           + (2 * Math.max(highlightItemSvg.margins.top + highlightItemSvg.margins.bottom,
                                           highlightItemSvg.margins.left + highlightItemSvg.margins.right))

    property int actualColumns: Plasmoid.configuration.useCustomGridSize
        ? Math.min(Plasmoid.configuration.gridColumns, Math.floor(width * 0.9 / cellSize))
        : Math.floor(width * 0.7 / cellSize)

    property int actualRows: Plasmoid.configuration.useCustomGridSize
        ? Math.min(Plasmoid.configuration.gridRows, Math.floor(height * 0.7 / cellSize))
        : Math.floor(height * 0.7 / cellSize)

    property int neededRows: (allAppsGrid.model && allAppsGrid.model.count > 0)
        ? Math.ceil(allAppsGrid.model.count / actualColumns) : actualRows

    property int finalRows: Math.max(1, Math.min(actualRows, neededRows))

    property bool searching: searchField.text !== ""

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
            fadeIn.restart();
        } else {
            rootItem.opacity = 0;
        }
        reset();
    }

    onSearchingChanged: {
        if (searching) {
            mainView.push(runnerComponent);
        } else {
            mainView.pop();
            reset();
        }
    }

    function colorWithAlpha(color: color, alpha: real): color {
        return Qt.rgba(color.r, color.g, color.b, alpha);
    }

    function reset() {
        // modelForRow(2) is "All applications" — the index is stable because
        // showRecentApps/showRecentDocs are hardcoded on in main.qml.
        allAppsGrid.model = rootModel.modelForRow(2);
        allAppsGrid.currentIndex = -1;
        systemFavoritesGrid.currentIndex = -1;
        allAppsGrid.forceLayout();
        searchField.clear();
        searchField.forceActiveFocus();
    }

    mainItem: FocusScope {
        id: rootItem

        anchors.fill: parent
        focus: true

        LayoutMirroring.enabled: Qt.application.layoutDirection === Qt.RightToLeft
        LayoutMirroring.childrenInherit: true

        OpacityAnimator {
            id: fadeIn
            target: rootItem
            from: 0; to: 1
            duration: Plasmoid.configuration.enableAnimations ? 180 : 0
            easing.type: Easing.OutQuad
        }

        Rectangle {
            anchors.fill: parent
            color: Plasmoid.configuration.useCustomBackgroundColor
                ? Plasmoid.configuration.customBackgroundColor
                : Kirigami.Theme.backgroundColor
            opacity: Plasmoid.configuration.backgroundOpacity
        }

        // ---- background gestures ------------------------------------------
        // Attached to rootItem, so the grid/search/system row sit above and
        // win the same points; only genuinely empty backdrop reaches these.

        TapHandler {
            // A clean tap on empty backdrop dismisses. DragThreshold means a
            // drag that wanders (scroll attempts, swipe starts) never counts.
            acceptedButtons: Qt.LeftButton
            gesturePolicy: TapHandler.DragThreshold
            onTapped: root.toggle()
        }

        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: eventPoint => contextMenu.open(eventPoint.position.x, eventPoint.position.y)
        }

        DragHandler {
            // Swipe DOWN anywhere on the backdrop to close, with the content
            // visibly following the finger so the gesture reads before it
            // commits. target:null = we read translation, nothing is moved
            // for us.
            id: dismissSwipe
            target: null
            xAxis.enabled: false
            yAxis.enabled: true

            onActiveChanged: {
                if (!active) {
                    if (translation.y > Kirigami.Units.gridUnit * 4) {
                        root.toggle();
                    }
                    contentShift.y = 0;
                }
            }
            onTranslationChanged: {
                if (active && translation.y > 0) {
                    contentShift.y = Math.min(translation.y * 0.5, Kirigami.Units.gridUnit * 6);
                }
            }
        }

        PlasmaExtras.Menu {
            id: contextMenu

            PlasmaExtras.MenuItem {
                action: Plasmoid.internalAction("configure")
            }
        }

        Connections {
            target: Plasmoid

            function onUserConfiguringChanged() {
                if (Plasmoid.userConfiguring) {
                    root.hide();
                }
            }
        }

        // Everything visual lives in here so the dismiss swipe can drag it.
        Item {
            id: content

            anchors.fill: parent
            transform: Translate { id: contentShift
                Behavior on y { NumberAnimation { duration: 140; easing.type: Easing.OutQuad } }
            }

            StackView {
                id: mainView

                width: (root.actualColumns * root.cellSize) + Kirigami.Units.largeSpacing
                height: (root.finalRows * root.cellSize) + Kirigami.Units.largeSpacing * 2

                anchors {
                    horizontalCenter: parent.horizontalCenter
                    verticalCenter: parent.verticalCenter
                    // Rise as maliit rises; also make room for the search pill.
                    verticalCenterOffset: (searchColumn.height / 2) - (kicker.keyboardHeight / 2)
                }
                Behavior on anchors.verticalCenterOffset {
                    NumberAnimation { duration: 200; easing.type: Easing.OutQuad }
                }

                initialItem: ItemGridView {
                    id: allAppsGrid

                    cellWidth: root.cellSize
                    cellHeight: root.cellSize
                    iconSize: root.iconSize
                    verticalScrollBarPolicy: (model && model.count > root.finalRows * root.actualColumns)
                        ? PlasmaComponents.ScrollBar.AsNeeded
                        : PlasmaComponents.ScrollBar.AlwaysOff

                    onKeyNavUp: searchField.forceActiveFocus()

                    Keys.onPressed: event => {
                        if (event.key === Qt.Key_Tab) {
                            event.accepted = true;
                            if (Plasmoid.configuration.showSystemIcons === 1) {
                                systemFavoritesGrid.tryActivate(0, 0);
                                systemFavoritesGrid.forceActiveFocus();
                            } else {
                                searchField.forceActiveFocus();
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

                    grabFocus: true
                    pullToDismiss: false // overscrolling results must never close the window
                    cellWidth: root.cellSize
                    cellHeight: root.cellSize
                    iconSize: root.iconSize
                    model: runnerModel.count > 0 ? runnerModel.modelForRow(0) : undefined

                    onKeyNavUp: searchField.forceActiveFocus()

                    Keys.onPressed: event => {
                        if (event.key === Qt.Key_Tab) {
                            event.accepted = true;
                            if (Plasmoid.configuration.showSystemIcons === 1) {
                                systemFavoritesGrid.tryActivate(0, 0);
                                systemFavoritesGrid.forceActiveFocus();
                            } else {
                                searchField.forceActiveFocus();
                            }
                        }
                    }
                }
            }

            // ---- system row -----------------------------------------------
            Rectangle {
                anchors.centerIn: systemFavoritesGrid
                width: systemFavoritesGrid.width + Kirigami.Units.largeSpacing
                height: systemFavoritesGrid.height + Kirigami.Units.largeSpacing
                radius: 10
                color: Kirigami.Theme.backgroundColor
                opacity: 0.6
                visible: systemFavoritesGrid.visible
            }

            ItemGridView {
                id: systemFavoritesGrid

                // Hidden while typing: it would sit underneath maliit, and a
                // stray tap near the keyboard must not hit Shut Down.
                visible: Plasmoid.configuration.showSystemIcons === 1
                         && kicker.keyboardHeight === 0

                anchors.bottom: parent.bottom
                anchors.bottomMargin: Kirigami.Units.largeSpacing
                anchors.horizontalCenter: parent.horizontalCenter

                width: model ? Math.min(Math.floor((root.width * 0.85) / cellWidth) * cellWidth,
                                        model.count * cellWidth) : 0
                height: cellHeight
                cellWidth: root.systemIconSize + Kirigami.Units.largeSpacing * 2
                cellHeight: cellWidth
                iconSize: root.systemIconSize

                showLabels: false
                pullToDismiss: false
                model: systemFavorites
                verticalScrollBarPolicy: PlasmaComponents.ScrollBar.AlwaysOff

                onKeyNavUp: {
                    if (root.searching) {
                        mainView.currentItem.tryActivate(0, 0);
                        mainView.currentItem.forceActiveFocus();
                    } else {
                        allAppsGrid.tryActivate(0, 0);
                        allAppsGrid.forceActiveFocus();
                    }
                }

                Keys.onPressed: event => {
                    if (event.key === Qt.Key_Tab) {
                        event.accepted = true;
                        searchField.forceActiveFocus();
                    }
                }
            }

            // ---- search ----------------------------------------------------
            Column {
                id: searchColumn

                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                // Rides above maliit; the extra gridUnits clear the system row
                // when the keyboard is down.
                anchors.bottomMargin: kicker.keyboardHeight > 0
                    ? kicker.keyboardHeight + Kirigami.Units.largeSpacing
                    : Kirigami.Units.gridUnit * 6
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

                    opacity: activeFocus ? 1.0 : 0.9
                    scale: activeFocus ? 1.05 : 1.0
                    Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutQuad } }
                    Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutQuad } }

                    onTextChanged: runnerModel.query = text

                    onActiveFocusChanged: {
                        if (activeFocus) {
                            Qt.inputMethod.show();
                        }
                    }

                    function clear() {
                        text = "";
                    }

                    function appendText(newText) {
                        if (!root.visible) return;
                        text += newText;
                        forceActiveFocus();
                    }

                    function backspace() {
                        if (!root.visible) return;
                        text = text.slice(0, -1);
                        forceActiveFocus();
                    }

                    TapHandler {
                        // Native input handles the first tap (focus + IME).
                        // This covers the second-summon case: field already
                        // focused, user dismissed maliit, taps again — Qt
                        // won't re-show the panel on its own then.
                        onTapped: {
                            searchField.forceActiveFocus();
                            Qt.inputMethod.show();
                        }
                    }

                    Keys.onPressed: event => {
                        if (event.key === Qt.Key_Down || event.key === Qt.Key_Tab) {
                            event.accepted = true;
                            const target = root.searching ? mainView.currentItem : allAppsGrid;
                            target.tryActivate(0, 0);
                            target.forceActiveFocus();
                        } else if ((event.key === Qt.Key_Enter || event.key === Qt.Key_Return)
                                   && root.searching) {
                            // Enter launches the top hit — the "type name,
                            // hit enter" flow maliit users live in.
                            event.accepted = true;
                            mainView.currentItem.activate(0);
                        }
                    }

                    background: Rectangle {
                        color: root.colorWithAlpha(Kirigami.Theme.backgroundColor, 0.7)
                        radius: 100
                        border.width: 1
                        border.color: root.colorWithAlpha(Kirigami.Theme.textColor, 0.05)
                    }
                }
            }
        }

        // Type anywhere → search. Ctrl/Shift chords are left alone so
        // shortcuts keep working.
        Keys.onPressed: event => {
            if (event.modifiers & Qt.ControlModifier || event.modifiers & Qt.ShiftModifier) {
                searchField.forceActiveFocus();
                return;
            }
            if (searchField.activeFocus) {
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
