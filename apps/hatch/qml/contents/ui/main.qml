import QtQuick 2.15

import org.kde.plasma.components 3.0 as PlasmaComponents3
import org.kde.plasma.core as PlasmaCore
import org.kde.ksvg 1.0 as KSvg
import org.kde.plasma.plasmoid 2.0
import org.kde.kirigami 2.20 as Kirigami
import org.kde.plasma.private.kicker 0.1 as Kicker

/* The plasmoid root: models, shared services, one keyboard tracker.

   Removed relative to the old file, with reasons:
   - Three fullscreen MultiPointTouchAreas: referenced a nonexistent
     `gridView` (silent ReferenceErrors) and sat over the panel button
     stealing its touches. Gestures now live where the content is, as input
     handlers in DashboardRepresentation/ItemGridView.
   - contentWrapper/isOpen/closePanel fade machinery: never displayed anything
     (empty Column) — the dash window animates itself.
   - itemListDialogComponent + ItemList*/SideBar*/ItemMultiGridView: menu-mode
     only, unreachable with isDash constant true.
   - dragHelper/dragSource: grid drag was disabled everywhere.
   - pmEngine: system actions go through the systemFavorites model.
   - lineSvg/listItemSvg/updateSvgMetrics, dummy heading metrics: only the
     deleted list views measured them.
   - duplicate keyboardHeight tracking: one property here is the source of
     truth for the whole plasmoid. */
PlasmoidItem {
    id: kicker

    signal reset

    // Kept as a constant: this fork ships only the dash. Branches on it were
    // removed, the property stays so any stray upstream reference still binds.
    readonly property bool isDash: true

    switchWidth: 0
    switchHeight: 0

    // Prevent Plasma from spawning its own dialog for us.
    preferredRepresentation: fullRepresentation
    compactRepresentation: null
    fullRepresentation: compactRepresentationComponent

    property QtObject globalFavorites: rootModel.favoritesModel
    property QtObject systemFavorites: rootModel.systemFavoritesModel

    // Single source of truth for the maliit keyboard's height. Everything
    // that dodges the keyboard binds to this.
    property real keyboardHeight: 0

    Plasmoid.icon: Plasmoid.configuration.useCustomButtonImage
        ? Plasmoid.configuration.customButtonImage
        : Plasmoid.configuration.icon

    onSystemFavoritesChanged: {
        systemFavorites.favorites = Plasmoid.configuration.favoriteSystemActions;
    }

    Connections {
        target: Qt.inputMethod

        function onVisibleChanged() {
            kicker.keyboardHeight = Qt.inputMethod.visible
                ? Qt.inputMethod.keyboardRectangle.height : 0;
        }
        function onKeyboardRectangleChanged() {
            // maliit can resize (e.g. suggestions row) while staying visible.
            if (Qt.inputMethod.visible) {
                kicker.keyboardHeight = Qt.inputMethod.keyboardRectangle.height;
            }
        }
    }

    Component {
        id: compactRepresentationComponent
        CompactRepresentation {}
    }

    Kicker.RootModel {
        id: rootModel

        autoPopulate: false
        appNameFormat: Plasmoid.configuration.appNameFormat
        flat: true
        sorted: Plasmoid.configuration.alphaSort
        showSeparators: false
        appletInterface: kicker

        showAllApps: true
        showAllAppsCategorized: false
        showTopLevelItems: false
        // Hardcoded true (not bound to config): reset() below indexes
        // modelForRow(2) for "All applications", and that row number is only
        // stable while both recent sections exist. Bind these to config again
        // only together with a row-lookup by name.
        showRecentApps: true
        showRecentDocs: true
        recentOrdering: Plasmoid.configuration.recentOrdering
    }

    Connections {
        target: systemFavorites

        function onFavoritesChanged() {
            Plasmoid.configuration.favoriteSystemActions = target.favorites;
        }
    }

    Connections {
        target: Plasmoid.configuration

        function onFavoriteSystemActionsChanged() {
            systemFavorites.favorites = Plasmoid.configuration.favoriteSystemActions;
        }
    }

    Kicker.RunnerModel {
        id: runnerModel

        appletInterface: kicker
        favoritesModel: globalFavorites
        mergeResults: true
        runners: {
            const results = ["krunner_services", "krunner_systemsettings",
                             "krunner_sessions", "krunner_powerdevil",
                             "calculator", "unitconverter"];
            if (Plasmoid.configuration.useExtraRunners) {
                results.push(...Plasmoid.configuration.extraRunners);
            }
            return results;
        }
    }

    Kicker.ProcessRunner {
        id: processRunner
    }

    Kicker.WindowSystem {
        id: windowSystem
    }

    // Delegates read this frame's margins for label insets and cell padding.
    KSvg.FrameSvgItem {
        id: highlightItemSvg

        visible: false
        imagePath: "widgets/viewitem"
        prefix: "hover"
    }

    PlasmaComponents3.Label {
        id: toolTipDelegate

        width: contentWidth
        height: undefined

        property Item toolTip

        text: toolTip ? toolTip.text : ""
        font.pointSize: Kirigami.Theme.defaultFont.pointSize + 0.5
    }

    Timer {
        id: justOpenedTimer

        repeat: false
        interval: 600
    }

    Connections {
        target: kicker

        function onExpandedChanged(expanded) {
            if (expanded) {
                windowSystem.monitorWindowVisibility(Plasmoid.fullRepresentationItem);
                justOpenedTimer.start();
            } else {
                kicker.reset();
            }
        }
    }

    function enableHideOnWindowDeactivate() {
        kicker.hideOnWindowDeactivate = false;
    }

    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18n("Edit Applications…")
            icon.name: "kmenuedit"
            visible: Plasmoid.immutability !== PlasmaCore.Types.SystemImmutable
            onTriggered: processRunner.runMenuEditor()
        }
    ]

    Component.onCompleted: {
        if (Plasmoid.hasOwnProperty("activationTogglesExpanded")) {
            Plasmoid.activationTogglesExpanded = false;
        }
        windowSystem.focusIn.connect(enableHideOnWindowDeactivate);
        kicker.hideOnWindowDeactivate = true;
        rootModel.refreshed.connect(reset);
    }
}
