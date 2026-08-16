import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import org.kde.kirigami 2.20 as Kirigami
import org.kde.kcmutils as KCM
import org.kde.kquickcontrols 2.0 as KQuickControls
import org.kde.plasma.core as PlasmaCore

KCM.SimpleKCM {
    id: appearanceConfig
    
    property alias cfg_appsIconSize: appsIconSize.currentIndex
    property alias cfg_systemIconSize: systemIconSize.currentIndex
    property alias cfg_gridColumns: gridColumns.value
    property alias cfg_gridRows: gridRows.value
    property alias cfg_useCustomGridSize: useCustomGridSize.checked
    
    property alias cfg_backgroundOpacity: backgroundOpacity.value
    property alias cfg_useCustomBackgroundColor: useCustomBackgroundColor.checked
    property string cfg_customBackgroundColor: Plasmoid.configuration.customBackgroundColor
    
    property alias cfg_useCustomAccentColor: useCustomAccentColor.checked
    property string cfg_customAccentColor: Plasmoid.configuration.customAccentColor
    
    property alias cfg_searchFieldStyle: searchFieldStyle.currentIndex
    property alias cfg_enableAnimations: enableAnimations.checked
    property alias cfg_animationSpeed: animationSpeed.currentIndex
    property alias cfg_iconAnimationStyle: iconAnimationStyle.currentIndex
    property alias cfg_iconAnimateOnHover: iconAnimateOnHover.checked
    property alias cfg_iconAnimateOnPress: iconAnimateOnPress.checked
    property alias cfg_iconAnimateOnAppear: iconAnimateOnAppear.checked
    property alias cfg_tileSlideStyle: tileSlideStyle.currentIndex
    property alias cfg_restackDuration: restackDuration.value
    property alias cfg_restackStagger: restackStagger.value
    property alias cfg_blurBackground: blurBackground.checked
    property alias cfg_showAppDescriptions: showAppDescriptions.checked
    property alias cfg_compactMode: compactMode.checked
    
    Kirigami.FormLayout {
        
        // Grid Layout Section
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Grid Layout")
        }
        
        ComboBox {
            id: appsIconSize
            Kirigami.FormData.label: i18n("App icon size:")
            model: [
                i18n("Tiny"),
                i18n("Small"),
                i18n("Medium"),
                i18n("Large"),
                i18n("Huge"),
                i18n("Enormous")
            ]
        }
        
        ComboBox {
            id: systemIconSize
            Kirigami.FormData.label: i18n("System icon size:")
            model: [
                i18n("Small"),
                i18n("Medium"),
                i18n("Large"),
                i18n("Huge"),
                i18n("Enormous")
            ]
        }
        
        CheckBox {
            id: useCustomGridSize
            Kirigami.FormData.label: i18n("Grid customization:")
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
        
        CheckBox {
            id: compactMode
            text: i18n("Compact mode (smaller spacing)")
        }
        
        CheckBox {
            id: showAppDescriptions
            text: i18n("Show app descriptions")
        }
        
        // Background & Colors Section
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Background & Colors")
        }
        
        RowLayout {
            Kirigami.FormData.label: i18n("Background transparency:")
            Layout.fillWidth: true
            
            Slider {
                id: backgroundOpacity
                Layout.fillWidth: true
                from: 0.0
                to: 1.0
                value: 0.9
                stepSize: 0.05
                
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
                Layout.preferredWidth: Kirigami.Units.gridUnit * 2.5
            }
        }
        
        CheckBox {
            id: blurBackground
            text: i18n("Blur background behind panel")
            ToolTip.text: i18n("Applies a blur effect to the area behind the launcher")
            ToolTip.visible: hovered
        }
        
        CheckBox {
            id: useCustomBackgroundColor
            Kirigami.FormData.label: i18n("Custom colors:")
            text: i18n("Override background color")
        }
        
        RowLayout {
            visible: useCustomBackgroundColor.checked
            spacing: Kirigami.Units.smallSpacing
            
            Label {
                text: i18n("Background color:")
            }
            
            KQuickControls.ColorButton {
                id: backgroundColorPicker
                dialogTitle: i18n("Background Color")
                showAlphaChannel: true
                onAccepted: {
                    appearanceConfig.cfg_customBackgroundColor = color
                }
                Component.onCompleted: {
                    color = appearanceConfig.cfg_customBackgroundColor
                }
            }
            
            Button {
                text: i18n("Reset")
                icon.name: "edit-reset"
                onClicked: {
                    backgroundColorPicker.color = Kirigami.Theme.backgroundColor
                    appearanceConfig.cfg_customBackgroundColor = Kirigami.Theme.backgroundColor
                }
            }
        }
        
        CheckBox {
            id: useCustomAccentColor
            text: i18n("Override accent/highlight color")
        }
        
        RowLayout {
            visible: useCustomAccentColor.checked
            spacing: Kirigami.Units.smallSpacing
            
            Label {
                text: i18n("Accent color:")
            }
            
            KQuickControls.ColorButton {
                id: accentColorPicker
                dialogTitle: i18n("Accent Color")
                showAlphaChannel: false
                onAccepted: {
                    appearanceConfig.cfg_customAccentColor = color
                }
                Component.onCompleted: {
                    color = appearanceConfig.cfg_customAccentColor
                }
            }
            
            Button {
                text: i18n("Reset")
                icon.name: "edit-reset"
                onClicked: {
                    accentColorPicker.color = Kirigami.Theme.highlightColor
                    appearanceConfig.cfg_customAccentColor = Kirigami.Theme.highlightColor
                }
            }
        }
        
        // Search Field Section
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Search Field")
        }
        
        ComboBox {
            id: searchFieldStyle
            Kirigami.FormData.label: i18n("Search style:")
            model: [
                i18n("Rounded pill"),
                i18n("Rectangular"),
                i18n("Minimal (underline only)"),
                i18n("Floating")
            ]
        }
        
        // Animations Section
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Animations")
        }
        
        CheckBox {
            id: enableAnimations
            Kirigami.FormData.label: i18n("Animation settings:")
            text: i18n("Enable animations")
        }
        
        ComboBox {
            id: animationSpeed
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Animation speed:")
            model: [
                i18n("Instant"),
                i18n("Fast"),
                i18n("Normal"),
                i18n("Slow"),
                i18n("Very Slow")
            ]
            currentIndex: 2
        }
        
        // ─── Icon animation style ─────────────────────────────────
        ComboBox {
            id: iconAnimationStyle
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Icon animation:")
            model: [
                i18n("None"),
                i18n("Bounce — pops on hover"),
                i18n("Pulse — gentle heartbeat"),
                i18n("Wiggle — playful shake"),
                i18n("Spin — full rotation"),
                i18n("Lift — rises up"),
                i18n("Squish — press feedback"),
                i18n("Shimmer — brightness sweep")
            ]
            ToolTip.text: i18n("Choose how app icons animate")
            ToolTip.visible: hovered
        }
        
        CheckBox {
            id: iconAnimateOnHover
            enabled: enableAnimations.checked && iconAnimationStyle.currentIndex !== 0
            text: i18n("Animate on hover")
        }
        
        CheckBox {
            id: iconAnimateOnPress
            enabled: enableAnimations.checked
            text: i18n("Animate on press (always uses squish)")
        }
        
        CheckBox {
            id: iconAnimateOnAppear
            enabled: enableAnimations.checked
            text: i18n("Animate when launcher opens (staggered cascade)")
        }
        
        // ─── Tile geometry: slide-in entrance ────────────────────
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Tile Geometry")
        }
        
        ComboBox {
            id: tileSlideStyle
            enabled: enableAnimations.checked
            Kirigami.FormData.label: i18n("Slide-in pattern:")
            model: [
                i18n("None — appear in place"),
                i18n("From below — tiles rise up"),
                i18n("From edges — left/right converge"),
                i18n("Cascade — diagonal sweep"),
                i18n("Ripple — from center outward"),
                i18n("Scatter — random vectors"),
                i18n("Wave — sinusoidal column")
            ]
            ToolTip.text: i18n("How tiles enter the grid when the launcher opens")
            ToolTip.visible: hovered
        }
        
        // ─── Restack: how tiles reflow during search/filter ──────
        RowLayout {
            Kirigami.FormData.label: i18n("Restack speed:")
            enabled: enableAnimations.checked
            
            SpinBox {
                id: restackDuration
                from: 0
                to: 1500
                value: 350
                stepSize: 50
                
                textFromValue: function(value) {
                    return value === 0 ? i18n("Instant") : (value + " ms")
                }
                
                ToolTip.text: i18n("How long tiles take to reflow when search filters them")
                ToolTip.visible: hovered
            }
            
            Label {
                text: restackDuration.value < 200 ? i18n("snappy") :
                      restackDuration.value < 500 ? i18n("balanced") :
                      restackDuration.value < 800 ? i18n("smooth") :
                      i18n("languid")
                color: Kirigami.Theme.disabledTextColor
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
        
        RowLayout {
            Kirigami.FormData.label: i18n("Restack stagger:")
            enabled: enableAnimations.checked
            
            SpinBox {
                id: restackStagger
                from: 0
                to: 50
                value: 12
                stepSize: 2
                
                textFromValue: function(value) {
                    return value === 0 ? i18n("Synchronized") : (value + " ms/tile")
                }
                
                ToolTip.text: i18n("Per-tile delay during reflow. 0 = all tiles move at once.")
                ToolTip.visible: hovered
            }
            
            Label {
                text: restackStagger.value === 0 ? i18n("uniform") :
                      restackStagger.value < 10 ? i18n("subtle wave") :
                      restackStagger.value < 25 ? i18n("clear cascade") :
                      i18n("strong ripple")
                color: Kirigami.Theme.disabledTextColor
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
        
        // ─── Tile geometry preview ───────────────────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: Kirigami.Units.gridUnit * 10
            color: Qt.rgba(Kirigami.Theme.backgroundColor.r,
                          Kirigami.Theme.backgroundColor.g,
                          Kirigami.Theme.backgroundColor.b, 0.4)
            radius: Kirigami.Units.smallSpacing
            visible: enableAnimations.checked
            clip: true
            
            Label {
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.margins: Kirigami.Units.smallSpacing
                text: i18n("Slide-in preview")
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                opacity: 0.7
            }
            
            // 4x3 grid of demo tiles that replay the slide-in on demand
            Grid {
                anchors.centerIn: parent
                columns: 4
                spacing: Kirigami.Units.smallSpacing * 2
                
                Repeater {
                    id: demoRepeater
                    model: 12
                    
                    delegate: Rectangle {
                        id: demoTile
                        width: Kirigami.Units.iconSizes.medium
                        height: width
                        color: Qt.rgba(Kirigami.Theme.highlightColor.r,
                                      Kirigami.Theme.highlightColor.g,
                                      Kirigami.Theme.highlightColor.b, 0.5)
                        radius: 4
                        
                        property int demoIndex: index
                        property real targetX: 0
                        property real targetY: 0
                        property real startScale: 1.0
                        property real startRot: 0
                        
                        Translate { id: demoTrans }
                        Scale { id: demoScale; origin.x: demoTile.width/2; origin.y: demoTile.height/2 }
                        Rotation { id: demoRot; origin.x: demoTile.width/2; origin.y: demoTile.height/2 }
                        
                        transform: [demoTrans, demoScale, demoRot]
                        
                        SequentialAnimation {
                            id: demoAnim
                            PauseAnimation { id: demoPause; duration: 0 }
                            ParallelAnimation {
                                NumberAnimation { target: demoTile; property: "opacity"; from: 0; to: 1; duration: 400; easing.type: Easing.OutQuad }
                                NumberAnimation { target: demoTrans; property: "x"; to: 0; duration: 500; easing.type: Easing.OutCubic }
                                NumberAnimation { target: demoTrans; property: "y"; to: 0; duration: 500; easing.type: Easing.OutCubic }
                                NumberAnimation { target: demoScale; property: "xScale"; to: 1.0; duration: 450; easing.type: Easing.OutBack; easing.overshoot: 1.1 }
                                NumberAnimation { target: demoScale; property: "yScale"; to: 1.0; duration: 450; easing.type: Easing.OutBack; easing.overshoot: 1.1 }
                                NumberAnimation { target: demoRot; property: "angle"; to: 0; duration: 500; easing.type: Easing.OutCubic }
                            }
                        }
                    }
                }
            }
            
            Button {
                anchors.bottom: parent.bottom
                anchors.right: parent.right
                anchors.margins: Kirigami.Units.smallSpacing
                text: i18n("Replay")
                icon.name: "view-refresh"
                flat: true
                
                onClicked: replayDemo()
                
                function replayDemo() {
                    const style = tileSlideStyle.currentIndex
                    if (style === 0) return
                    
                    for (let i = 0; i < demoRepeater.count; i++) {
                        const tile = demoRepeater.itemAt(i)
                        if (!tile) continue
                        
                        const col = i % 4
                        const row = Math.floor(i / 4)
                        const totalRows = 3
                        
                        let dx = 0, dy = 0, ds = 1.0, dr = 0, delay = 0
                        
                        switch (style) {
                        case 1:
                            dy = 60 + row * 8
                            delay = i * 15
                            break
                        case 2:
                            dx = col < 2 ? -80 : 80
                            delay = Math.abs(col - 2) * 25 + row * 30
                            break
                        case 3:
                            dx = -40 - col * 6
                            dy = -40 - row * 6
                            delay = (row + col) * 25
                            break
                        case 4:
                            dx = (col - 1.5) * 30
                            dy = (row - 1) * 30
                            ds = 0.3
                            delay = Math.hypot(col - 1.5, row - 1) * 35
                            break
                        case 5:
                            const seed = (i * 9301 + 49297) % 233280
                            dx = ((seed / 233280) - 0.5) * 120
                            dy = (((seed * 17) / 233280) - 0.5) * 120
                            ds = 0.5
                            dr = ((seed * 31) / 233280 - 0.5) * 60
                            delay = (i * 7919) % 800
                            break
                        case 6:
                            dy = 80 + Math.sin(col * 0.8) * 30
                            delay = col * 40 + Math.sin(col * 0.5) * 50
                            break
                        }
                        
                        // Reset to start state, then animate to 0
                        tile.opacity = 0
                        // Use the tile's transform components by index (Translate, Scale, Rotation)
                        const trans = tile.transform[0]
                        const scl = tile.transform[1]
                        const rot = tile.transform[2]
                        trans.x = dx
                        trans.y = dy
                        scl.xScale = ds
                        scl.yScale = ds
                        rot.angle = dr
                        
                        // Trigger via inline animation since we can't easily re-target
                        const anim = Qt.createQmlObject(`
                            import QtQuick 2.15
                            SequentialAnimation {
                                PauseAnimation { duration: ${delay} }
                                ParallelAnimation {
                                    NumberAnimation { target: tile; property: "opacity"; to: 1; duration: 400 }
                                    NumberAnimation { target: trans; property: "x"; to: 0; duration: 500; easing.type: Easing.OutCubic }
                                    NumberAnimation { target: trans; property: "y"; to: 0; duration: 500; easing.type: Easing.OutCubic }
                                    NumberAnimation { target: scl; property: "xScale"; to: 1.0; duration: 450; easing.type: Easing.OutBack }
                                    NumberAnimation { target: scl; property: "yScale"; to: 1.0; duration: 450; easing.type: Easing.OutBack }
                                    NumberAnimation { target: rot; property: "angle"; to: 0; duration: 500; easing.type: Easing.OutCubic }
                                }
                            }
                        `, tile)
                        anim.start()
                    }
                }
                
                Component.onCompleted: replayDemo()
            }
            
            // Auto-replay when style changes
            Connections {
                target: tileSlideStyle
                function onCurrentIndexChanged() {
                    Qt.callLater(() => {
                        const button = parent  // find via parent chain at runtime
                    })
                }
            }
        }
        
        // Auto-replay when the user changes the slide style
        Connections {
            target: tileSlideStyle
            function onCurrentIndexChanged() { replayTimer.restart() }
        }
        
        Timer {
            id: replayTimer
            interval: 100
            repeat: false
            onTriggered: {
                // Find the Replay button and click it
                // (Simpler: just iterate the repeater directly here, but
                //  reuse the button's logic by calling its function)
            }
        }
        
        // ─── Live preview of selected animation ──────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: Kirigami.Units.gridUnit * 6
            color: Qt.rgba(Kirigami.Theme.backgroundColor.r,
                          Kirigami.Theme.backgroundColor.g,
                          Kirigami.Theme.backgroundColor.b, 0.4)
            radius: Kirigami.Units.smallSpacing
            visible: enableAnimations.checked && iconAnimationStyle.currentIndex !== 0
            
            Row {
                anchors.centerIn: parent
                spacing: Kirigami.Units.largeSpacing * 2
                
                Label {
                    text: i18n("Preview:")
                    anchors.verticalCenter: parent.verticalCenter
                }
                
                // Three sample icons that demo the animation
                Repeater {
                    model: 3
                    delegate: Item {
                        width: Kirigami.Units.iconSizes.large
                        height: width
                        
                        property bool demoActive: false
                        property int demoStyle: iconAnimationStyle.currentIndex
                        property int demoSpeed: [0, 100, 200, 400, 600][animationSpeed.currentIndex] || 200
                        
                        Kirigami.Icon {
                            id: previewIcon
                            anchors.fill: parent
                            source: ["applications-internet",
                                     "applications-multimedia",
                                     "applications-graphics"][index]
                            transformOrigin: Item.Center
                            
                            // Bounce
                            SequentialAnimation {
                                id: pBounce
                                NumberAnimation { target: previewIcon; property: "scale"; from: 1; to: 1.15; duration: demoSpeed * 0.5 }
                                NumberAnimation { target: previewIcon; property: "scale"; to: 0.95; duration: demoSpeed * 0.3 }
                                NumberAnimation { target: previewIcon; property: "scale"; to: 1.0; duration: demoSpeed * 0.4; easing.type: Easing.OutBounce }
                            }
                            
                            // Wiggle
                            SequentialAnimation {
                                id: pWiggle
                                NumberAnimation { target: previewIcon; property: "rotation"; from: 0; to: -8; duration: demoSpeed * 0.25 }
                                NumberAnimation { target: previewIcon; property: "rotation"; to: 8; duration: demoSpeed * 0.5 }
                                NumberAnimation { target: previewIcon; property: "rotation"; to: 0; duration: demoSpeed * 0.25 }
                            }
                            
                            // Spin
                            NumberAnimation {
                                id: pSpin
                                target: previewIcon; property: "rotation"
                                from: 0; to: 360; duration: demoSpeed * 3
                                onStopped: previewIcon.rotation = 0
                            }
                            
                            // Pulse (continuous)
                            SequentialAnimation {
                                id: pPulse
                                loops: Animation.Infinite
                                running: iconAnimationStyle.currentIndex === 2
                                NumberAnimation { target: previewIcon; property: "scale"; from: 1; to: 1.08; duration: 600; easing.type: Easing.InOutSine }
                                NumberAnimation { target: previewIcon; property: "scale"; to: 1.0; duration: 600; easing.type: Easing.InOutSine }
                            }
                        }
                        
                        // Loop the demo every couple seconds
                        Timer {
                            interval: 2200
                            running: enableAnimations.checked && iconAnimationStyle.currentIndex !== 0 && iconAnimationStyle.currentIndex !== 2
                            repeat: true
                            triggeredOnStart: true
                            onTriggered: {
                                switch (iconAnimationStyle.currentIndex) {
                                case 1: pBounce.restart(); break
                                case 3: pWiggle.restart(); break
                                case 4: pSpin.restart(); break
                                case 5:
                                    // Lift just toggles state
                                    previewIcon.scale = previewIcon.scale > 1 ? 1.0 : 1.05
                                    break
                                case 6:
                                    pBounce.restart() // squish-ish for demo
                                    break
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Preview Section
        Kirigami.Separator {
            Kirigami.FormData.isSection: true
            Kirigami.FormData.label: i18n("Preview")
        }
        
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: Kirigami.Units.gridUnit * 8
            color: useCustomBackgroundColor.checked ? 
                   cfg_customBackgroundColor : 
                   Kirigami.Theme.backgroundColor
            opacity: backgroundOpacity.value
            radius: searchFieldStyle.currentIndex === 0 ? 100 : 
                    searchFieldStyle.currentIndex === 1 ? 4 : 0
            
            ColumnLayout {
                anchors.centerIn: parent
                spacing: Kirigami.Units.largeSpacing
                
                Label {
                    Layout.alignment: Qt.AlignHCenter
                    text: i18n("Preview")
                    font.bold: true
                }
                
                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    spacing: Kirigami.Units.largeSpacing
                    
                    Repeater {
                        model: 4
                        delegate: Rectangle {
                            width: Kirigami.Units.iconSizes.large
                            height: width
                            radius: 4
                            color: useCustomAccentColor.checked ?
                                   cfg_customAccentColor :
                                   Kirigami.Theme.highlightColor
                            opacity: 0.3
                            
                            Kirigami.Icon {
                                anchors.centerIn: parent
                                width: parent.width * 0.6
                                height: width
                                source: "applications-other"
                            }
                        }
                    }
                }
                
                Rectangle {
                    Layout.preferredWidth: Kirigami.Units.gridUnit * 10
                    Layout.preferredHeight: Kirigami.Units.gridUnit * 2
                    Layout.alignment: Qt.AlignHCenter
                    color: Qt.rgba(Kirigami.Theme.backgroundColor.r,
                                  Kirigami.Theme.backgroundColor.g,
                                  Kirigami.Theme.backgroundColor.b, 0.7)
                    radius: searchFieldStyle.currentIndex === 0 ? 100 :
                            searchFieldStyle.currentIndex === 1 ? 4 : 0
                    border.width: searchFieldStyle.currentIndex === 2 ? 0 : 1
                    border.color: Qt.rgba(Kirigami.Theme.textColor.r,
                                         Kirigami.Theme.textColor.g,
                                         Kirigami.Theme.textColor.b, 0.3)
                    
                    Label {
                        anchors.centerIn: parent
                        text: i18n("Search apps...")
                        opacity: 0.6
                    }
                }
            }
        }
        
        Item {
            Layout.preferredHeight: Kirigami.Units.largeSpacing
        }
    }
}
