import QtQuick 2.15

import org.kde.plasma.components 3.0 as PlasmaComponents3
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid 2.0
import org.kde.kirigami 2.20 as Kirigami

import "code/tools.js" as Tools
import "code/animation.js" as Anim

/* One tile. Pure presentation — ALL input lives in ItemGridView's handlers,
   which is the fix for the old tap bugs: no per-delegate MouseArea can steal
   a press from the Flickable or swallow a tap meant for a neighbour.

   Animation system (from the geometry/animations pass, leaned down):
   - slide-in: 7 patterns (none/below/edges/cascade/ripple/scatter/wave),
     each tile computing its own arrival vector + stagger from grid position;
   - icon micro-motion: none/bounce/wiggle/squish, keyed to hover and to the
     view's pressedIndex. Legacy style ids (pulse/spin/lift/shimmer) map to
     bounce: the shader/continuous styles cost GPU on a phone for effects
     that need sustained hover to even be seen.
   All of it gates on enableAnimations and scales with animationSpeed. */
Item {
    id: item

    width: GridView.view.cellWidth
    height: GridView.view.cellHeight

    enabled: !model.disabled

    property bool showLabel: true

    readonly property int itemIndex: model.index
    readonly property string favoriteId: model.favoriteId !== undefined ? model.favoriteId : ""
    readonly property var m: model
    readonly property bool hasActionList: ((model.favoriteId !== null)
        || (("hasActionList" in model) && (model.hasActionList === true)))

    readonly property bool pressed: GridView.view.pressedIndex === index

    // ---- animation config ------------------------------------------------
    readonly property bool animate: Plasmoid.configuration.enableAnimations
    readonly property real speed: [1.6, 1.0, 0.6][Plasmoid.configuration.animationSpeed] ?? 1.0
    readonly property int iconStyle: {
        const s = Plasmoid.configuration.iconAnimationStyle;
        return (s === 0 || s === 3 || s === 6) ? s : 1; // legacy ids → bounce
    }
    readonly property int slideStyle: Plasmoid.configuration.tileSlideStyle

    Accessible.role: Accessible.Button
    Accessible.name: model.display

    function openActionMenu(x, y) {
        const actionList = hasActionList ? model.actionList : [];
        Tools.fillActionMenu(i18n, actionMenu, actionList,
                             GridView.view.model.favoritesModel, model.favoriteId);
        actionMenu.visualParent = item;
        actionMenu.open(x, y);
    }

    function actionTriggered(actionId, actionArgument) {
        if (Tools.triggerAction(GridView.view.model, model.index, actionId, actionArgument) === true) {
            root.toggle();
        }
    }

    // ---- slide-in --------------------------------------------------------
    // The only grid geometry the delegate needs; the pattern math derives
    // row/column from it in animation.js.
    readonly property int gridCols: Math.max(1, Math.floor(GridView.view.width / GridView.view.cellWidth))

    transform: [
        Translate { id: slideT },
        Scale { id: scaleT; origin.x: item.width / 2; origin.y: item.height / 2 },
        Rotation { id: rotT; origin.x: item.width / 2; origin.y: item.height / 2 }
    ]

    Component.onCompleted: {
        if (!animate || slideStyle === 0 || !Plasmoid.configuration.iconAnimateOnAppear
                || !root.visible) {
            return;
        }
        // The arrival vector lives in code/animation.js so it can be tested
        // off-device (node --test qml/tests/); this side only applies it.
        const from = Anim.slideFrom(slideStyle, index, gridCols, speed);
        slideT.x = from.dx;
        slideT.y = from.dy;
        scaleT.xScale = scaleT.yScale = from.scale;
        rotT.angle = from.rotation;
        item.opacity = 0;
        slideAnim.duration = from.duration;
        slideDelay.interval = Math.max(1, from.delay);
        slideDelay.start();
    }

    Timer {
        id: slideDelay
        repeat: false
        onTriggered: slideAnim.start()
    }

    ParallelAnimation {
        id: slideAnim
        property int duration: 300
        NumberAnimation { target: slideT; properties: "x,y"; to: 0; duration: slideAnim.duration; easing.type: Easing.OutCubic }
        NumberAnimation { target: scaleT; properties: "xScale,yScale"; to: 1; duration: slideAnim.duration; easing.type: Easing.OutBack; easing.overshoot: 1.1 }
        NumberAnimation { target: rotT; property: "angle"; to: 0; duration: slideAnim.duration; easing.type: Easing.OutCubic }
        NumberAnimation { target: item; property: "opacity"; to: 1; duration: slideAnim.duration * 0.7 }
    }

    // ---- icon ------------------------------------------------------------
    Item {
        id: iconContainer

        anchors.horizontalCenter: parent.horizontalCenter
        y: item.showLabel ? (2 * highlightItemSvg.margins.top) : Math.round((parent.height - height) / 2)
        width: iconSize
        height: iconSize

        Kirigami.Icon {
            id: icon
            anchors.fill: parent
            animated: false
            source: model.decoration
        }

        // hover: bounce / wiggle (squish is press-only by design)
        readonly property bool hovered: GridView.view.currentIndex === index && GridView.view.hoverActive

        onHoveredChanged: {
            if (!item.animate || !Plasmoid.configuration.iconAnimateOnHover || !hovered) {
                return;
            }
            if (item.iconStyle === 1) bounceAnim.restart();
            else if (item.iconStyle === 3) wiggleAnim.restart();
        }

        Connections {
            // press squish: fires on press regardless of chosen hover style,
            // because tap feedback is the one animation a touch device needs.
            target: item
            enabled: item.animate && Plasmoid.configuration.iconAnimateOnPress

            function onPressedChanged() {
                if (item.pressed) {
                    squishIn.restart();
                } else {
                    squishOut.restart();
                }
            }
        }

        SequentialAnimation {
            id: bounceAnim
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.18; duration: 90 * item.speed; easing.type: Easing.OutQuad }
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.0; duration: 190 * item.speed; easing.type: Easing.OutBounce }
        }

        SequentialAnimation {
            id: wiggleAnim
            NumberAnimation { target: iconContainer; property: "rotation"; to: -9; duration: 60 * item.speed }
            NumberAnimation { target: iconContainer; property: "rotation"; to: 7; duration: 90 * item.speed }
            NumberAnimation { target: iconContainer; property: "rotation"; to: -4; duration: 90 * item.speed }
            NumberAnimation { target: iconContainer; property: "rotation"; to: 0; duration: 70 * item.speed }
        }

        NumberAnimation {
            id: squishIn
            target: iconContainer; property: "scale"; to: 0.86
            duration: 70 * item.speed; easing.type: Easing.OutQuad
        }
        SequentialAnimation {
            id: squishOut
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.06; duration: 90 * item.speed; easing.type: Easing.OutQuad }
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.0; duration: 110 * item.speed; easing.type: Easing.OutQuad }
        }
    }

    PlasmaComponents3.Label {
        id: label

        visible: item.showLabel

        anchors {
            top: iconContainer.bottom
            topMargin: Kirigami.Units.smallSpacing
            left: parent.left
            leftMargin: highlightItemSvg.margins.left
            right: parent.right
            rightMargin: highlightItemSvg.margins.right
        }

        horizontalAlignment: Text.AlignHCenter
        maximumLineCount: 2
        elide: Text.ElideMiddle
        wrapMode: Text.Wrap
        font.pointSize: Kirigami.Theme.defaultFont.pointSize + 0.5
        text: ("name" in model ? model.name : model.display)
        textFormat: Text.PlainText
    }

    PlasmaCore.ToolTipArea {
        // Desktop nicety only. On touch it is disabled outright: its hover
        // synthesis fights the long-press gesture, and a tooltip you can't
        // hover is dead weight.
        anchors.fill: parent
        active: !Kirigami.Settings.isMobile && root.visible
            && (!item.showLabel || label.truncated)
        property string text: model.display
        mainItem: toolTipDelegate
    }

    Keys.onPressed: event => {
        if (event.key === Qt.Key_Menu && hasActionList) {
            event.accepted = true;
            openActionMenu(width / 2, height / 2);
        } else if (event.key === Qt.Key_Enter || event.key === Qt.Key_Return) {
            event.accepted = true;
            itemGrid.activate(index);
        }
    }
}
