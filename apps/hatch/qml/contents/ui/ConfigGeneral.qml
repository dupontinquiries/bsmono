import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

import org.kde.draganddrop 2.0 as DragDrop
import org.kde.kirigami 2.20 as Kirigami
import org.kde.iconthemes as KIconThemes
import org.kde.plasma.core as PlasmaCore
import org.kde.ksvg 1.0 as KSvg
import org.kde.plasma.plasmoid 2.0
import org.kde.kcmutils as KCM
import org.kde.kquickcontrols 2.0 as KQuickControls

/* The live config page, unchanged except: duplicate Kirigami import dropped,
   dead isDash property dropped, and an Animations section appended at the
   bottom. The 30KB ConfigAppearance artifact page was deliberately NOT
   shipped — it duplicates cfg_ aliases with this page (KCM aliases must be
   unique per config key) and configures five feature groups that don't
   exist. The ~10 knobs below cover everything the delegates actually read. */
KCM.SimpleKCM {
    id: configGeneral

    property string cfg_icon: Plasmoid.configuration.icon
    property bool cfg_useCustomButtonImage: Plasmoid.configuration.useCustomButtonImage
    property string cfg_customButtonImage: Plasmoid.configuration.customButtonImage

    property alias cfg_appNameFormat: appNameFormat.currentIndex
    property alias cfg_limitDepth: limitDepth.checked
    property alias cfg_alphaSort: alphaSort.checked
    property alias cfg_showIconsRootLevel: showIconsRootLevel.checked

    property alias cfg_recentOrdering: recentOrdering.currentIndex
    property alias cfg_showRecentApps: showRecentApps.checked
    property alias cfg_showRecentDocs: showRecentDocs.checked

    property alias cfg_useExtraRunners: useExtraRunners.checked
    property alias cfg_alignResultsToBottom: alignResultsToBottom.checked

    property alias cfg_appsIconSize: appsIconSize.currentIndex
    property alias cfg_systemIconSize: systemIconSize.currentIndex
    property alias cfg_showSystemIcons: showSystemIcons.currentIndex
    property alias cfg_gridColumns: gridColumns.value
    property alias cfg_gridRows: gridRows.value
    property alias cfg_useCustomGridSize: useCustomGridSize.checked
    property alias cfg_backgroundOpacity: backgroundOpacity.value
    property alias cfg_useCustomBackgroundColor: useCustomBackgroundColor.checked
    property string cfg_customBackgroundColor: Plasmoid.configuration.customBackgroundColor

    // Animations
    property alias cfg_enableAnimations: enableAnimations.checked
    property alias cfg_animationSpeed: animationSpeed.currentIndex
    property int cfg_iconAnimationStyle
    property alias cfg_iconAnimateOnHover: animOnHover.checked
    property alias cfg_iconAnimateOnPress: animOnPress.checked
    property alias cfg_iconAnimateOnAppear: animOnAppear.checked
    property alias cfg_tileSlideStyle: tileSlideStyle.currentIndex
    property alias cfg_restackDuration: restackDuration.value
    property alias cfg_restackStagger: restackStagger.value
    property alias cfg_pullToDismiss: pullToDismiss.checked

    Kirigami.FormLayout {
        anchors.left: parent.left
        anchors.right: parent.right

        Button {
            id: iconButton

            Kirigami.FormData.label: i18n("Icon:")

            implicitWidth: previewFrame.width + Kirigami.Units.smallSpacing * 2
            implicitHeight: previewFrame.height + Kirigami.Units.smallSpacing * 2

            // Just to provide some visual feedback when dragging;
            // cannot have checked without checkable enabled
            checkable: true
            checked: dropArea.containsAcceptableDrag

            onPressed: iconMenu.opened ? iconMenu.close() : iconMenu.open()

            DragDrop.DropArea {
                id: dropArea

                property bool containsAcceptableDrag: false

                anchors.fill: parent

                onDragEnter: {
                    // Cannot use string operations (e.g. indexOf()) on "url" basic type.
                    var urlString = event.mimeData.url.toString();

                    // This list is also hardcoded in KIconDialog.
                    var extensions = [".png", ".xpm", ".svg", ".svgz"];
                    containsAcceptableDrag = urlString.indexOf("file:///") === 0 && extensions.some(function (extension) {
                        return urlString.indexOf(extension) === urlString.length - extension.length; // "endsWith"
                    });

                    if (!containsAcceptableDrag) {
                        event.ignore();
                    }
                }
                onDragLeave: containsAcceptableDrag = false
                onDrop: {
                    if (containsAcceptableDrag) {
                        // Strip file:// prefix, we already verified in onDragEnter that we have only local URLs.
                        iconDialog.setCustomButtonImage(event.mimeData.url.toString().substr("file://".length));
                    }
                    containsAcceptableDrag = false;
                }
            }

            KIconThemes.IconDialog {
                id: iconDialog

                function setCustomButtonImage(image) {
                    configGeneral.cfg_customButtonImage = image || configGeneral.cfg_icon || "start-here-kde-symbolic"
                    configGeneral.cfg_useCustomButtonImage = true;
                }

                onIconNameChanged: setCustomButtonImage(iconName);
            }

            KSvg.FrameSvgItem {
                id: previewFrame

                anchors.centerIn: parent
                imagePath: Plasmoid.location === PlasmaCore.Types.Vertical || Plasmoid.location === PlasmaCore.Types.Horizontal
                        ? "widgets/panel-background" : "widgets/background"
                width: Kirigami.Units.iconSizes.large + fixedMargins.left + fixedMargins.right
                height: Kirigami.Units.iconSizes.large + fixedMargins.top + fixedMargins.bottom

                Kirigami.Icon {
                    anchors.centerIn: parent
                    width: Kirigami.Units.iconSizes.large
                    height: width
                    source: configGeneral.cfg_useCustomButtonImage ? configGeneral.cfg_customButtonImage : configGeneral.cfg_icon
                }
            }

            Menu {
                id: iconMenu

                // Appear below the button
                y: +parent.height

                onClosed: iconButton.checked = false;

                MenuItem {
                    text: i18nc("@item:inmenu Open icon chooser dialog", "Choose…")
                    icon.name: "document-open-folder"
                    onClicked: iconDialog.open()
                }
                MenuItem {
                    text: i18nc("@item:inmenu Reset icon to default", "Clear Icon")
                    icon.name: "edit-clear"
                    onClicked: {
                        configGeneral.cfg_icon = "start-here-kde-symbolic"
                        configGeneral.cfg_useCustomButtonImage = false
                    }
                }
            }
        }

        Item {
            Kirigami.FormData.isSection: true
        }

        ComboBox {
            id: appNameFormat

            Kirigami.FormData.label: i18n("Show applications as:")

            model: [i18n("Name only"), i18n("Description only"), i18n("Name (Description)"), i18n("Description (Name)")]
        }

        Item {
            Kirigami.FormData.isSection: true
        }

        CheckBox {
            id: alphaSort
            visible: false

            Kirigami.FormData.label: i18n("Behavior:")

            text: i18n("Sort applications alphabetically")
        }

        CheckBox {
            id: limitDepth
            visible: false

            text: i18n("Flatten sub-menus to a single level")
        }

        CheckBox {
            id: showIconsRootLevel
            visible: false

            text: i18n("Show icons on the root level of the menu")
        }

        CheckBox {
            id: showRecentApps
            visible: false

            Kirigami.FormData.label: i18n("Show categories:")

            text: recentOrdering.currentIndex == 0
                    ? i18n("Recent applications")
                    : i18n("Often used applications")
        }

        CheckBox {
            id: showRecentDocs
            visible: false

            text: recentOrdering.currentIndex == 0
                    ? i18n("Recent files")
                    : i18n("Often used files")
        }

        ComboBox {
            id: recentOrdering
            visible: false

            Kirigami.FormData.label: i18n("Sort items in categories by:")
            model: [i18nc("@item:inlistbox Sort items in categories by [Recently used | Often used]", "Recently used"), i18nc("@item:inlistbox Sort items in categories by [Recently used | Ofetn used]", "Often used")]
        }

        CheckBox {
            id: useExtraRunners
            visible: false

            Kirigami.FormData.label: i18n("Search:")

            text: i18n("Expand search to bookmarks, files and emails")
        }

        CheckBox {
            id: alignResultsToBottom
            visible: false

            text: i18n("Align search results to bottom")
        }

        ComboBox {
            id: appsIconSize

            Kirigami.FormData.label: i18n("Apps icon size:")

            model: [i18n("Small"), i18n("Medium"), i18n("Large"), i18n("Huge"), i18n("HUGE"), i18n("Enormous")]
        }

        ComboBox {
            id: systemIconSize

            Kirigami.FormData.label: i18n("System icons size:")

            model: [i18n("Small"), i18n("Medium"), i18n("Large"), i18n("Huge"), i18n("Enormous")]
        }

        ComboBox {
            id: showSystemIcons

            Kirigami.FormData.label: i18n("System Actions:")

            model: [i18n("Hide"), i18n("Show")]
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Grid Layout Settings")
        }

        CheckBox {
            id: useCustomGridSize

            Kirigami.FormData.label: i18n("Grid Layout:")

            text: i18n("Use custom grid size")
        }

        SpinBox {
            id: gridColumns
            enabled: useCustomGridSize.checked

            Kirigami.FormData.label: i18n("Columns:")

            from: 1
            to: 20
            value: 6
            stepSize: 1
        }

        SpinBox {
            id: gridRows
            enabled: useCustomGridSize.checked

            Kirigami.FormData.label: i18n("Rows:")

            from: 1
            to: 20
            value: 4
            stepSize: 1
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Appearance")
        }

        RowLayout {
            Kirigami.FormData.label: i18n("Background transparency:")
            Layout.fillWidth: true

            Slider {
                id: backgroundOpacity
                Layout.fillWidth: true
                from: 0.0
                to: 1.0
                value: 0.4
                stepSize: 0.01

                ToolTip {
                    parent: backgroundOpacity.handle
                    visible: backgroundOpacity.pressed
                    text: Math.round(backgroundOpacity.value * 100) + "%"
                }
            }

            Label {
                text: Math.round(backgroundOpacity.value * 100) + "%"
                color: Kirigami.Theme.disabledTextColor
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * 0.9
                Layout.preferredWidth: Kirigami.Units.gridUnit * 2
            }
        }

        CheckBox {
            id: useCustomBackgroundColor

            Kirigami.FormData.label: i18n("Background color:")

            text: i18n("Use custom background color")
        }

        RowLayout {
            Layout.fillWidth: true
            visible: useCustomBackgroundColor.checked

            Label {
                text: i18n("Custom Color:")
            }

            KQuickControls.ColorButton {
                id: backgroundColorPicker
                dialogTitle: i18n("Background Color")
                showAlphaChannel: true

                onAccepted: {
                    configGeneral.cfg_customBackgroundColor = color
                }

                Component.onCompleted: {
                    color = configGeneral.cfg_customBackgroundColor
                }
            }
        }

        // ---- Animations (appended section) --------------------------------

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Animations")
        }

        CheckBox {
            id: enableAnimations
            Kirigami.FormData.label: i18n("Animations:")
            text: i18n("Enable animations")
        }

        ComboBox {
            id: animationSpeed
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Speed:")
            model: [i18n("Slow"), i18n("Normal"), i18n("Fast")]
        }

        ComboBox {
            id: iconAnimationStyle
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Icon animation:")

            // Config values are non-contiguous (legacy ids retired), so this
            // maps by hand instead of aliasing currentIndex.
            readonly property var styleValues: [0, 1, 3, 6]
            model: [i18n("None"), i18n("Bounce"), i18n("Wiggle"), i18n("Squish")]

            currentIndex: Math.max(0, styleValues.indexOf(configGeneral.cfg_iconAnimationStyle))
            onActivated: configGeneral.cfg_iconAnimationStyle = styleValues[currentIndex]
        }

        CheckBox {
            id: animOnHover
            enabled: enableAnimations.checked
            text: i18n("Animate on hover")
        }

        CheckBox {
            id: animOnPress
            enabled: enableAnimations.checked
            text: i18n("Animate on press")
        }

        CheckBox {
            id: animOnAppear
            enabled: enableAnimations.checked
            text: i18n("Animate when launcher opens")
        }

        ComboBox {
            id: tileSlideStyle
            enabled: enableAnimations.checked && animOnAppear.checked
            Kirigami.FormData.label: i18n("Tile slide-in:")
            model: [i18n("None"), i18n("From below"), i18n("From edges"),
                    i18n("Cascade"), i18n("Ripple"), i18n("Scatter"), i18n("Wave")]
        }

        SpinBox {
            id: restackDuration
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Restack duration:")
            from: 0
            to: 1500
            stepSize: 50
            textFromValue: (value, locale) => value + " ms"
            valueFromText: (text, locale) => parseInt(text)
        }

        SpinBox {
            id: restackStagger
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Restack stagger:")
            from: 0
            to: 50
            stepSize: 2
            textFromValue: (value, locale) => value + " ms"
            valueFromText: (text, locale) => parseInt(text)
        }

        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Touch")
        }

        CheckBox {
            id: pullToDismiss
            Kirigami.FormData.label: i18n("Gestures:")
            text: i18n("Pull down past the top of the grid to close")
        }
    }
}
