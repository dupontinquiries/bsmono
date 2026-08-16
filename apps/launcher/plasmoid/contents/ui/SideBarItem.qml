import QtQuick 2.15
import org.kde.kquickcontrolsaddons 2.0
import org.kde.plasma.core as PlasmaCore
import org.kde.kirigami 2.20 as Kirigami
import org.kde.plasma.plasmoid 2.0
import "code/tools.js" as Tools

Item {
    id: item
    width: root.width
    height: root.width

    signal actionTriggered(string actionId, variant actionArgument)
    signal aboutToShowActionMenu(variant actionMenu)

    property bool hasActionList: ((model.favoriteId !== null) ||
                                  (("hasActionList" in model) && (model.hasActionList !== null)))
    property int itemIndex: model.index

    // Same animation config as the main grid items
    readonly property int animationStyle: Plasmoid.configuration.iconAnimationStyle || 0
    readonly property bool animEnabled: Plasmoid.configuration.enableAnimations
    readonly property int animSpeed: animEnabled ?
        [0, 100, 200, 400, 600][Plasmoid.configuration.animationSpeed] || 200 : 0

    property bool isHovered: toolTip.containsMouse
    property bool isPressed: listener.pressed

    onAboutToShowActionMenu: actionMenu => {
        const actionList = (model.hasActionList !== null) ? model.actionList : []
        Tools.fillActionMenu(i18n, actionMenu, actionList, repeater.model, model.favoriteId)
    }

    onActionTriggered: (actionId, actionArgument) => {
        if (Tools.triggerAction(repeater.model, model.index, actionId, actionArgument) === true) {
            kicker.expanded = false
        }
    }

    function openActionMenu(visualParent, x, y) {
        aboutToShowActionMenu(actionMenu)
        actionMenu.visualParent = visualParent
        actionMenu.open(x, y)
    }

    ActionMenu {
        id: actionMenu
        onActionClicked: actionTriggered(actionId, actionArgument)
    }

    // ─── Stagger appearance on load ──────────────────────────────────
    opacity: 0
    scale: 0.6

    Component.onCompleted: {
        if (animEnabled && animSpeed > 0) {
            appearAnim.start()
        } else {
            opacity = 1
            scale = 1.0
        }
    }

    ParallelAnimation {
        id: appearAnim
        // Stagger based on index so they cascade in
        PauseAnimation { duration: item.itemIndex * 40 }
        SequentialAnimation {
            PropertyAnimation {
                target: item; property: "opacity"
                from: 0; to: 1
                duration: animSpeed
                easing.type: Easing.OutQuad
            }
        }
        SequentialAnimation {
            PauseAnimation { duration: item.itemIndex * 40 }
            PropertyAnimation {
                target: item; property: "scale"
                from: 0.6; to: 1.0
                duration: animSpeed
                easing.type: Easing.OutBack
                easing.overshoot: 1.5
            }
        }
    }

    // ─── Hover/press scaling for the whole tile ──────────────────────
    Item {
        id: iconWrap
        anchors.fill: parent
        scale: item.isPressed ? 0.88 :
               item.isHovered ? 1.1 :
               1.0
        rotation: rotationAnim.running ? iconWrap.rotation : 0

        Behavior on scale {
            enabled: animEnabled
            NumberAnimation {
                duration: animSpeed
                easing.type: Easing.OutBack
                easing.overshoot: 1.3
            }
        }

        // Wiggle when activated (style 3 keeps with main grid choice)
        SequentialAnimation {
            id: rotationAnim
            running: false
            loops: 1
            NumberAnimation {
                target: iconWrap; property: "rotation"
                from: 0; to: -10; duration: 80; easing.type: Easing.OutQuad
            }
            NumberAnimation {
                target: iconWrap; property: "rotation"
                to: 10; duration: 120; easing.type: Easing.InOutQuad
            }
            NumberAnimation {
                target: iconWrap; property: "rotation"
                to: 0; duration: 100; easing.type: Easing.OutQuad
            }
        }

        Kirigami.Icon {
            anchors.fill: parent
            active: toolTip.containsMouse
            source: model.decoration
            animated: false
        }

        // Subtle glow ring when hovered (uses theme highlight color)
        Rectangle {
            anchors.fill: parent
            anchors.margins: -2
            color: "transparent"
            border.width: item.isHovered ? 2 : 0
            border.color: Qt.rgba(Kirigami.Theme.highlightColor.r,
                                  Kirigami.Theme.highlightColor.g,
                                  Kirigami.Theme.highlightColor.b,
                                  0.5)
            radius: parent.width / 2
            visible: animEnabled && item.isHovered

            Behavior on border.width {
                enabled: animEnabled
                NumberAnimation { duration: animSpeed }
            }
        }
    }

    // Trigger wiggle on click for system actions (they're things like reboot,
    // so a little visual confirmation that the tap registered is helpful)
    Connections {
        target: item
        function onIsPressedChanged() {
            if (item.isPressed && item.animationStyle === 3 && animEnabled) {
                rotationAnim.restart()
            }
        }
    }

    MouseEventListener {
        id: listener
        anchors {
            fill: parent
            leftMargin: -sideBar.margins.left
            rightMargin: -sideBar.margins.right
        }
        enabled: (item.parent && !item.parent.animating)

        property bool pressed: false
        property int pressX: -1
        property int pressY: -1

        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton

        onPressed: {
            if (mouse.buttons & Qt.RightButton) {
                if (item.hasActionList) {
                    item.openActionMenu(item, mouse.x, mouse.y)
                }
            } else {
                pressed = true
                pressX = mouse.x
                pressY = mouse.y
            }
        }

        onReleased: {
            if (pressed) {
                repeater.model.trigger(index, "", null)
                kicker.expanded = false
            }
            pressed = false
            pressX = pressY = -1
        }

        onContainsMouseChanged: {
            if (!containsMouse) {
                pressed = false
                pressX = pressY = -1
            }
        }

        onPositionChanged: {
            if (pressX !== -1 && dragHelper.isDrag(pressX, pressY, mouse.x, mouse.y)) {
                kicker.dragSource = item
                dragHelper.startDrag(kicker, model.url, model.icon)
                pressed = false
                pressX = pressY = -1
            }
        }
    }

    PlasmaCore.ToolTipArea {
        id: toolTip
        property string text: model.display
        anchors {
            fill: parent
            leftMargin: -sideBar.margins.left
            rightMargin: -sideBar.margins.right
        }
        interactive: false
        location: (((Plasmoid.location === PlasmaCore.Types.RightEdge) ||
                    (Qt.application.layoutDirection === Qt.RightToLeft))
                   ? PlasmaCore.Types.RightEdge : PlasmaCore.Types.LeftEdge)
        mainItem: toolTipDelegate
    }
}
