import QtQuick 2.15
import QtQuick.Layouts 1.15

import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid 2.0
import org.kde.kirigami 2.20 as Kirigami
import org.kde.plasma.private.kicker 0.1 as Kicker

/* The panel button. Dash-only now (kicker.isDash is constant), so the
   popup-menu branch of the old file is gone; what remains is the icon, one
   TapHandler, and the dash window. The three fullscreen MultiPointTouchAreas
   that used to sit in main.qml referenced a `gridView` that does not exist at
   that scope — they were dead code, but they still GRABBED touches over the
   panel, which is why taps on this button felt lottery-like. They're deleted;
   the button now owns its input. */
Item {
    id: root

    readonly property bool vertical: Plasmoid.formFactor === PlasmaCore.Types.Vertical
    readonly property bool useCustomButtonImage: Plasmoid.configuration.useCustomButtonImage
        && Plasmoid.configuration.customButtonImage.length !== 0

    readonly property Component dashWindowComponent:
        Qt.createComponent(Qt.resolvedUrl("./DashboardRepresentation.qml"), root)
    readonly property Kicker.DashboardWindow dashWindow:
        dashWindowComponent && dashWindowComponent.status === Component.Ready
            ? dashWindowComponent.createObject(root, { visualParent: root }) : null

    onWidthChanged: updateSizeHints()
    onHeightChanged: updateSizeHints()

    function updateSizeHints() {
        if (useCustomButtonImage) {
            if (vertical) {
                const scaledHeight = Math.floor(parent.width * (buttonIcon.implicitHeight / buttonIcon.implicitWidth));
                root.Layout.minimumWidth = -1;
                root.Layout.minimumHeight = scaledHeight;
                root.Layout.maximumWidth = Kirigami.Units.iconSizes.huge;
                root.Layout.maximumHeight = scaledHeight;
            } else {
                const scaledWidth = Math.floor(parent.height * (buttonIcon.implicitWidth / buttonIcon.implicitHeight));
                root.Layout.minimumWidth = scaledWidth;
                root.Layout.minimumHeight = -1;
                root.Layout.maximumWidth = scaledWidth;
                root.Layout.maximumHeight = Kirigami.Units.iconSizes.huge;
            }
        } else {
            root.Layout.minimumWidth = Kirigami.Units.iconSizes.huge;
            root.Layout.minimumHeight = Kirigami.Units.iconSizes.huge;
        }
    }

    Accessible.name: Plasmoid.title
    Accessible.role: Accessible.Button

    Kirigami.Icon {
        id: buttonIcon

        anchors.fill: parent

        readonly property double aspectRatio: root.vertical
            ? implicitHeight / implicitWidth
            : implicitWidth / implicitHeight

        active: hoverHandler.hovered && !justOpenedTimer.running
        source: root.useCustomButtonImage ? Plasmoid.configuration.customButtonImage : Plasmoid.configuration.icon
        // A square custom image is assumed to be an icon and rounded to the
        // nearest icon size to avoid scaling artifacts; rectangular ones scale.
        roundToIconSize: !root.useCustomButtonImage || aspectRatio === 1

        onSourceChanged: root.updateSizeHints()
    }

    HoverHandler {
        id: hoverHandler
        acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
    }

    TapHandler {
        // Works for touch and mouse alike; the justOpenedTimer debounce keeps
        // the open-tap from immediately re-toggling through Plasmoid.activated.
        onTapped: {
            if (root.dashWindow && !justOpenedTimer.running) {
                root.dashWindow.toggle();
                justOpenedTimer.start();
            }
        }
    }

    Keys.onPressed: event => {
        switch (event.key) {
        case Qt.Key_Space:
        case Qt.Key_Enter:
        case Qt.Key_Return:
        case Qt.Key_Select:
            Plasmoid.activated();
            event.accepted = true;
            break;
        }
    }

    Connections {
        target: Plasmoid
        enabled: root.dashWindow !== null

        function onActivated() {
            if (!justOpenedTimer.running) {
                root.dashWindow.toggle();
                justOpenedTimer.start();
            }
        }
    }
}
