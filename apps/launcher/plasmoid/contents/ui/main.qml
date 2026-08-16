/*
 * main.qml — plasmoid root (PlasmoidItem).
 *
 * Touch fixes vs. the previous iteration:
 *  · The three stacked full-screen MultiPointTouchAreas are GONE. They sat over
 *    the plasmoid root (i.e. over the panel button, not the dashboard), stole
 *    press events from everything under them, and one of them referenced a
 *    `gridView` id that does not exist at this scope (runtime ReferenceError on
 *    first swipe). Swipe/tap gestures live where the content lives:
 *    DashboardRepresentation.qml and ItemGridView.qml.
 *  · The contentWrapper / isOpen / closePanel fade machinery is gone with them —
 *    it was only reachable from those touch areas, and DashboardWindow already
 *    owns show/hide.
 *  · Keyboard-height tracking is gone from this file; the dashboard tracks
 *    Qt.inputMethod itself (root-level property there, so every child scope can
 *    resolve it — the old copy here was shadowed and unused).
 */
import QtQuick 2.15
import QtQuick.Controls
import QtQuick.Layouts 1.15
import org.kde.plasma.components 3.0 as PlasmaComponents3
import org.kde.plasma.core as PlasmaCore
import org.kde.ksvg 1.0 as KSvg
import org.kde.plasma.plasmoid 2.0
import org.kde.kirigami 2.20 as Kirigami
import org.kde.plasma.private.kicker 0.1 as Kicker
import org.kde.plasma.plasma5support 2.0 as P5Support

PlasmoidItem {
    id: kicker

    anchors.fill: parent

    signal reset

    property bool isDash: true

    switchWidth: isDash || !fullRepresentationItem ? 0 : fullRepresentationItem.Layout.minimumWidth
    switchHeight: isDash || !fullRepresentationItem ? 0 : fullRepresentationItem.Layout.minimumHeight

    // Prevent Plasma from spawning a dialog on its own when we're Dash.
    preferredRepresentation: isDash ? fullRepresentation : null
    compactRepresentation: isDash ? null : compactRepresentation
    fullRepresentation: compactRepresentation

    property Component itemListDialogComponent: Qt.createComponent(Qt.resolvedUrl("./ItemListDialog.qml"))

    property Item dragSource: null

    property QtObject globalFavorites: rootModel.favoritesModel
    property QtObject systemFavorites: rootModel.systemFavoritesModel

    Plasmoid.icon: Plasmoid.configuration.useCustomButtonImage ? Plasmoid.configuration.customButtonImage : Plasmoid.configuration.icon

    onSystemFavoritesChanged: {
        systemFavorites.favorites = Plasmoid.configuration.favoriteSystemActions;
    }

    function action_menuedit() {
        processRunner.runMenuEditor();
    }

    function updateSvgMetrics() {
        lineSvg.horLineHeight = lineSvg.elementSize("horizontal-line").height;
        lineSvg.vertLineWidth = lineSvg.elementSize("vertical-line").width;
    }

    Component {
        id: compactRepresentation
        CompactRepresentation {}
    }

    Kicker.RootModel {
        id: rootModel

        autoPopulate: false

        appNameFormat: Plasmoid.configuration.appNameFormat
        flat: kicker.isDash || Plasmoid.configuration.limitDepth
        sorted: Plasmoid.configuration.alphaSort
        showSeparators: !kicker.isDash
        appletInterface: kicker

        showAllApps: kicker.isDash
        showAllAppsCategorized: false
        showTopLevelItems: !kicker.isDash
        showRecentApps: Plasmoid.configuration.showRecentApps
        showRecentDocs: Plasmoid.configuration.showRecentDocs
        recentOrdering: Plasmoid.configuration.recentOrdering

        onShowRecentAppsChanged: {
            Plasmoid.configuration.showRecentApps = showRecentApps;
        }
        onShowRecentDocsChanged: {
            Plasmoid.configuration.showRecentDocs = showRecentDocs;
        }
        onRecentOrderingChanged: {
            Plasmoid.configuration.recentOrdering = recentOrdering;
        }
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

    P5Support.DataSource {
        id: pmEngine
        engine: "powermanagement"
        connectedSources: ["PowerDevil", "Sleep States"]
        function performOperation(what) {
            var service = serviceForSource("PowerDevil")
            var operation = service.operationDescription(what)
            service.startOperationCall(operation)
        }
    }

    Kicker.RunnerModel {
        id: runnerModel

        appletInterface: kicker
        favoritesModel: globalFavorites
        mergeResults: true

        runners: {
            const results = ["krunner_services", "krunner_systemsettings"];
            if (kicker.isDash) {
                results.push("krunner_sessions", "krunner_powerdevil", "calculator", "unitconverter");
            }
            if (Plasmoid.configuration.useExtraRunners) {
                results.push(...Plasmoid.configuration.extraRunners);
            }
            return results;
        }
    }

    Kicker.DragHelper {
        id: dragHelper
        dragIconSize: Kirigami.Units.iconSizes.medium
    }

    Kicker.ProcessRunner {
        id: processRunner
    }

    Kicker.WindowSystem {
        id: windowSystem
    }

    KSvg.FrameSvgItem {
        id: highlightItemSvg
        visible: false
        imagePath: "widgets/viewitem"
        prefix: "hover"
    }

    KSvg.FrameSvgItem {
        id: listItemSvg
        visible: false
        imagePath: "widgets/listitem"
        prefix: "normal"
    }

    KSvg.Svg {
        id: lineSvg
        imagePath: "widgets/line"
        property int horLineHeight
        property int vertLineWidth
    }

    PlasmaComponents3.Label {
        id: toolTipDelegate
        width: contentWidth
        height: undefined
        font.pointSize: Kirigami.Theme.defaultFont.pointSize + 0.5
        property Item toolTip
        text: toolTip ? toolTip.text : ""
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

    function resetDragSource() {
        dragSource = null;
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
            Plasmoid.activationTogglesExpanded = !kicker.isDash
        }

        windowSystem.focusIn.connect(enableHideOnWindowDeactivate);
        kicker.hideOnWindowDeactivate = true;

        updateSvgMetrics();
        PlasmaCore.Theme.themeChanged.connect(updateSvgMetrics);

        rootModel.refreshed.connect(reset);
        dragHelper.dropped.connect(resetDragSource);
    }
}
