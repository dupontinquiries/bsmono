import QtQuick 2.15
import QtQuick.Effects
import org.kde.plasma.components 3.0 as PlasmaComponents3
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid 2.0
import org.kde.kirigami 2.20 as Kirigami
import "code/tools.js" as Tools

Item {
    id: item
    width: GridView.view.cellWidth
    height: GridView.view.cellHeight
    enabled: !model.disabled

    // ─── Animation config ─────────────────────────────────────────────
    readonly property int iconAnimStyle: Plasmoid.configuration.iconAnimationStyle || 0
    readonly property int slideStyle: Plasmoid.configuration.tileSlideStyle || 1
    // 0=None, 1=FromBelow, 2=FromEdges, 3=Cascade, 4=Ripple, 5=Scatter, 6=Wave

    readonly property bool animEnabled: Plasmoid.configuration.enableAnimations
    readonly property int animSpeed: animEnabled ?
        [0, 100, 200, 400, 600][Plasmoid.configuration.animationSpeed] || 200 : 0

    readonly property int restackDuration: Plasmoid.configuration.restackDuration || 350

    // ─── Standard delegate props ──────────────────────────────────────
    property bool showLabel: true
    property int itemIndex: model.index
    property string favoriteId: model.favoriteId !== undefined ? model.favoriteId : ""
    property url url: model.url !== undefined ? model.url : ""
    property variant icon: model.decoration !== undefined ? model.decoration : ""
    property var m: model
    property bool hasActionList: ((model.favoriteId !== null) ||
                                  (("hasActionList" in model) && (model.hasActionList === true)))

    property bool isHovered: GridView.isCurrentItem
    property bool isPressed: false

    Accessible.role: Accessible.MenuItem
    Accessible.name: model.display

    // ─── Geometry the tile aims for / arrives at ─────────────────────
    // We let GridView control position via x/y, but composite our slide-in
    // and restack motion through extra translate/scale transforms so we
    // don't fight the view's own positioning logic.

    function openActionMenu(x, y) {
        const actionList = hasActionList ? model.actionList : []
        Tools.fillActionMenu(i18n, actionMenu, actionList,
                             GridView.view.model.favoritesModel, model.favoriteId)
        actionMenu.visualParent = item
        actionMenu.open(x, y)
    }

    function actionTriggered(actionId, actionArgument) {
        const close = (Tools.triggerAction(GridView.view.model, model.index,
                                           actionId, actionArgument) === true)
        if (close && root.toggle) root.toggle()
    }

    // ─── Slide-in entrance ────────────────────────────────────────────
    // Each tile arrives from a different vector based on the chosen style
    // and its grid position. We use Translate transforms (not anchors)
    // because the GridView already owns the final x/y.

    opacity: 0
    Translate { id: slideTransform }
    Scale     { id: scaleTransform; origin.x: item.width / 2; origin.y: item.height / 2 }
    Rotation  { id: rotateTransform; origin.x: item.width / 2; origin.y: item.height / 2 }

    transform: [slideTransform, scaleTransform, rotateTransform]

    function computeSlideVector() {
        const cols = Math.max(1, Math.floor(GridView.view.width / GridView.view.cellWidth))
        const row = Math.floor(itemIndex / cols)
        const col = itemIndex % cols
        const totalRows = Math.ceil(GridView.view.count / cols)

        let dx = 0, dy = 0, ds = 1.0, dr = 0

        switch (slideStyle) {
        case 1: // FromBelow — every tile rises from below its final position
            dx = 0
            dy = 60 + (row * 8)  // farther rows travel slightly more
            break

        case 2: // FromEdges — left half slides from left, right half from right
            dx = col < cols / 2 ? -80 : 80
            dy = 0
            break

        case 3: // Cascade — diagonal sweep from top-left
            dx = -40 - (col * 6)
            dy = -40 - (row * 6)
            break

        case 4: // Ripple — emanates from center
            const centerCol = cols / 2 - 0.5
            const centerRow = totalRows / 2 - 0.5
            dx = (col - centerCol) * 30
            dy = (row - centerRow) * 30
            ds = 0.3
            break

        case 5: // Scatter — random-ish offsets seeded by index
            const seed = (itemIndex * 9301 + 49297) % 233280
            dx = ((seed / 233280) - 0.5) * 200
            dy = (((seed * 17) / 233280) - 0.5) * 200
            ds = 0.5
            dr = ((seed * 31) / 233280 - 0.5) * 60
            break

        case 6: // Wave — sinusoidal vertical entry, phase by column
            dx = 0
            dy = 80 + Math.sin(col * 0.8) * 30
            break

        default:
            dx = 0; dy = 0
        }

        return { dx, dy, ds, dr }
    }

    function computeStaggerDelay() {
        const cols = Math.max(1, Math.floor(GridView.view.width / GridView.view.cellWidth))
        const row = Math.floor(itemIndex / cols)
        const col = itemIndex % cols

        switch (slideStyle) {
        case 1: // FromBelow — left-to-right, top-to-bottom
            return itemIndex * 15

        case 2: // FromEdges — outside-in, so further cols delay less
            return Math.abs(col - cols / 2) * 25 + row * 30

        case 3: // Cascade — diagonal wavefront
            return (row + col) * 25

        case 4: // Ripple — distance from center
            const centerCol = cols / 2 - 0.5
            const centerRow = (Math.ceil(GridView.view.count / cols) / 2) - 0.5
            return Math.hypot(col - centerCol, row - centerRow) * 35

        case 5: // Scatter — pseudo-random
            return ((itemIndex * 7919) % 800)

        case 6: // Wave — column-based, smooth
            return col * 40 + Math.sin(col * 0.5) * 50

        default:
            return itemIndex * 10
        }
    }

    SequentialAnimation {
        id: slideInAnim
        // Pause based on position so tiles arrive in a coordinated pattern
        PauseAnimation { id: slidePause; duration: 0 }

        ParallelAnimation {
            NumberAnimation {
                target: item; property: "opacity"
                from: 0; to: 1
                duration: animSpeed * 1.2
                easing.type: Easing.OutQuad
            }
            NumberAnimation {
                target: slideTransform; property: "x"
                to: 0
                duration: animSpeed * 1.5
                easing.type: Easing.OutCubic
            }
            NumberAnimation {
                target: slideTransform; property: "y"
                to: 0
                duration: animSpeed * 1.5
                easing.type: Easing.OutCubic
            }
            NumberAnimation {
                target: scaleTransform; property: "xScale"
                to: 1.0
                duration: animSpeed * 1.3
                easing.type: Easing.OutBack
                easing.overshoot: 1.1
            }
            NumberAnimation {
                target: scaleTransform; property: "yScale"
                to: 1.0
                duration: animSpeed * 1.3
                easing.type: Easing.OutBack
                easing.overshoot: 1.1
            }
            NumberAnimation {
                target: rotateTransform; property: "angle"
                to: 0
                duration: animSpeed * 1.5
                easing.type: Easing.OutCubic
            }
        }
    }

    Component.onCompleted: {
        if (animEnabled && animSpeed > 0 && slideStyle !== 0) {
            const v = computeSlideVector()
            slideTransform.x = v.dx
            slideTransform.y = v.dy
            scaleTransform.xScale = v.ds
            scaleTransform.yScale = v.ds
            rotateTransform.angle = v.dr
            slidePause.duration = computeStaggerDelay()
            slideInAnim.start()
        } else {
            opacity = 1
            slideTransform.x = 0
            slideTransform.y = 0
            scaleTransform.xScale = 1.0
            scaleTransform.yScale = 1.0
            rotateTransform.angle = 0
        }
    }

    // ─── The icon ─────────────────────────────────────────────────────
    Item {
        id: iconContainer
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: item.showLabel ? undefined : parent.verticalCenter
        y: item.showLabel ? (2 * highlightItemSvg.margins.top) : undefined
        width: iconSize
        height: width
        transformOrigin: Item.Center

        // Bounce (hover)
        SequentialAnimation {
            id: bounceAnim
            NumberAnimation { target: iconContainer; property: "scale"; from: 1.0; to: 1.15; duration: animSpeed * 0.5; easing.type: Easing.OutQuad }
            NumberAnimation { target: iconContainer; property: "scale"; to: 0.95; duration: animSpeed * 0.3; easing.type: Easing.InOutQuad }
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.0; duration: animSpeed * 0.4; easing.type: Easing.OutBounce }
        }

        // Pulse (continuous while hovered)
        SequentialAnimation {
            id: pulseAnim
            loops: Animation.Infinite
            NumberAnimation { target: iconContainer; property: "scale"; from: 1.0; to: 1.08; duration: 600; easing.type: Easing.InOutSine }
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.0; duration: 600; easing.type: Easing.InOutSine }
        }

        // Wiggle
        SequentialAnimation {
            id: wiggleAnim
            NumberAnimation { target: iconContainer; property: "rotation"; from: 0; to: -8; duration: animSpeed * 0.25 }
            NumberAnimation { target: iconContainer; property: "rotation"; to: 8; duration: animSpeed * 0.5 }
            NumberAnimation { target: iconContainer; property: "rotation"; to: -5; duration: animSpeed * 0.35 }
            NumberAnimation { target: iconContainer; property: "rotation"; to: 3; duration: animSpeed * 0.3 }
            NumberAnimation { target: iconContainer; property: "rotation"; to: 0; duration: animSpeed * 0.2 }
        }

        // Spin
        NumberAnimation {
            id: spinAnim
            target: iconContainer; property: "rotation"
            from: 0; to: 360; duration: animSpeed * 3
            onStopped: iconContainer.rotation = 0
        }

        // Squish (press)
        SequentialAnimation {
            id: squishAnim
            NumberAnimation { target: iconContainer; property: "scale"; to: 0.85; duration: animSpeed * 0.4; easing.type: Easing.OutQuad }
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.0; duration: animSpeed * 0.6; easing.type: Easing.OutBack; easing.overshoot: 1.5 }
        }

        // Lift (hold while hovered)
        property real liftOffset: item.isHovered ? -4 : 0
        Behavior on liftOffset {
            enabled: animEnabled && iconAnimStyle === 5
            NumberAnimation { duration: animSpeed; easing.type: Easing.OutCubic }
        }

        transform: Translate { y: iconContainer.liftOffset }

        Kirigami.Icon {
            id: icon
            anchors.fill: parent
            animated: false
            source: model.decoration

            // Shimmer
            layer.enabled: iconAnimStyle === 7 && item.isHovered
            layer.effect: MultiEffect {
                brightness: 0.0
                SequentialAnimation on brightness {
                    running: iconAnimStyle === 7 && item.isHovered
                    loops: Animation.Infinite
                    NumberAnimation { from: 0.0; to: 0.4; duration: 800; easing.type: Easing.InOutSine }
                    NumberAnimation { from: 0.4; to: 0.0; duration: 800; easing.type: Easing.InOutSine }
                }
            }
        }

        // Press feedback (universal)
        SequentialAnimation {
            id: pressFeedback
            NumberAnimation { target: iconContainer; property: "scale"; to: 0.9; duration: 80; easing.type: Easing.OutQuad }
            NumberAnimation { target: iconContainer; property: "scale"; to: 1.0; duration: 120; easing.type: Easing.OutBack; easing.overshoot: 1.4 }
        }

        Connections {
            target: item
            function onIsHoveredChanged() {
                if (!Plasmoid.configuration.iconAnimateOnHover) return
                switch (iconAnimStyle) {
                case 1: if (item.isHovered) bounceAnim.restart(); break
                case 2: pulseAnim.running = item.isHovered; if (!item.isHovered) iconContainer.scale = 1.0; break
                case 3: if (item.isHovered) wiggleAnim.restart(); break
                case 4: if (item.isHovered && !spinAnim.running) spinAnim.restart(); break
                }
            }

            function onIsPressedChanged() {
                if (!Plasmoid.configuration.iconAnimateOnPress) return
                if (item.isPressed && iconAnimStyle === 6) {
                    squishAnim.restart()
                } else if (item.isPressed) {
                    pressFeedback.restart()
                }
            }
        }
    }

    // ─── Label ────────────────────────────────────────────────────────
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
        opacity: item.isHovered ? 1.0 : 0.85

        Behavior on opacity {
            enabled: animEnabled
            NumberAnimation { duration: 150 }
        }
    }

    PlasmaCore.ToolTipArea {
        id: toolTip
        property string text: model.display
        anchors.fill: parent
        active: root.visible && (!item.showLabel || label.truncated)
        mainItem: toolTipDelegate
        onContainsMouseChanged: item.GridView.view.itemContainsMouseChanged(containsMouse)
    }

    Keys.onPressed: event => {
        if (event.key === Qt.Key_Menu && hasActionList) {
            event.accepted = true
            openActionMenu(item)
        } else if (event.key === Qt.Key_Enter || event.key === Qt.Key_Return) {
            event.accepted = true
            item.isPressed = true
            pressFeedback.restart()
            if ("trigger" in GridView.view.model) {
                GridView.view.model.trigger(index, "", null)
                if (root.toggle) root.toggle()
            }
            itemGrid.itemActivated(index, "", null)
        }
    }
}
