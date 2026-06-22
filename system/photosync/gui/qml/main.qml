// main.qml - Snapchat-style camera interface
import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15
import QtMultimedia 5.15
import PhotoSync 1.0

ApplicationWindow {
    id: root
    visible: true
    width: 1080
    height: 1920
    title: "PhotoSync Camera"
    
    // Remove title bar for fullscreen experience
    flags: Qt.Window | Qt.FramelessWindowHint
    
    CameraManager {
        id: cameraManager
        
        onPhotoReady: {
            console.log("[QML] Photo captured:", path)
            captureFlash.start()
            
            // Get camera state and process photo
            var state = cameraManager.getCameraState()
            console.log("[QML] Camera state:", JSON.stringify(state))
            photoProcessor.processPhoto(path, state)
        }
        
        onError: {
            console.error("[QML] Camera error:", message)
            errorDialog.text = message
            errorDialog.open()
        }
    }
    
    PhotoProcessor {
        id: photoProcessor
        
        onPhotoProcessed: {
            console.log("[QML] Photo processed successfully!")
            console.log("[QML] Original size:", originalSize, "bytes")
            console.log("[QML] Compressed size:", compressedSize, "bytes")
            console.log("[QML] Savings:", (100 - (compressedSize * 100 / originalSize)).toFixed(1) + "%")
            
            // Show toast notification
            processingToast.show("Photo saved! Saved " + 
                                (100 - (compressedSize * 100 / originalSize)).toFixed(0) + "% space")
        }
        
        onError: {
            console.error("[QML] Processing error:", message)
            errorDialog.text = "Processing error: " + message
            errorDialog.open()
        }
        
        onProgressChanged: {
            if (photoProcessor.processing) {
                console.log("[QML] Processing progress:", photoProcessor.progress + "%")
            }
        }
    }
    
    // Main camera view
    Rectangle {
        id: cameraView
        anchors.fill: parent
        color: "black"
        
        // Camera preview (would be replaced with actual video surface)
        VideoOutput {
            id: preview
            anchors.fill: parent
            // Connect to libcamera feed
        }
        
        // Scanning overlay when searching for cameras
        Rectangle {
            anchors.fill: parent
            color: "#80000000"
            visible: cameraManager.scanning
            
            ColumnLayout {
                anchors.centerIn: parent
                spacing: 20
                
                BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    running: cameraManager.scanning
                }
                
                Text {
                    text: "Scanning for cameras..."
                    color: "white"
                    font.pixelSize: 24
                    Layout.alignment: Qt.AlignHCenter
                }
            }
        }
        
        // Top bar with camera info
        Rectangle {
            id: topBar
            anchors.top: parent.top
            width: parent.width
            height: 100
            color: "#40000000"
            
            RowLayout {
                anchors.fill: parent
                anchors.margins: 20
                spacing: 20
                
                // Camera selector
                Button {
                    text: cameraManager.activeCamera
                    font.pixelSize: 16
                    onClicked: cameraSelector.open()
                    background: Rectangle {
                        color: "#40FFFFFF"
                        radius: 8
                    }
                }
                
                Item { Layout.fillWidth: true }
                
                // HDR toggle
                Button {
                    text: "HDR"
                    font.pixelSize: 16
                    checkable: true
                    onCheckedChanged: cameraManager.enableHDR(checked)
                    background: Rectangle {
                        color: parent.checked ? "#4000FF00" : "#40FFFFFF"
                        radius: 8
                    }
                }
                
                // Night mode toggle
                Button {
                    text: "🌙"
                    font.pixelSize: 24
                    checkable: true
                    onCheckedChanged: cameraManager.enableNightMode(checked)
                    background: Rectangle {
                        color: parent.checked ? "#40FFFF00" : "#40FFFFFF"
                        radius: 8
                    }
                }
                
                // Settings
                Button {
                    text: "⚙️"
                    font.pixelSize: 24
                    onClicked: settingsDrawer.open()
                    background: Rectangle {
                        color: "#40FFFFFF"
                        radius: 8
                    }
                }
            }
        }
        
        // ToF distance indicator (if available)
        Rectangle {
            visible: cameraManager.hasToF
            anchors.top: topBar.bottom
            anchors.left: parent.left
            anchors.margins: 20
            width: 150
            height: 60
            color: "#40000000"
            radius: 8
            
            ColumnLayout {
                anchors.centerIn: parent
                
                Text {
                    text: "Distance"
                    color: "white"
                    font.pixelSize: 14
                }
                
                Text {
                    text: cameraManager.tofDistance.toFixed(2) + "m"
                    color: "#00FF00"
                    font.pixelSize: 20
                    font.bold: true
                }
            }
        }
        
        // Focus mode indicator
        Rectangle {
            anchors.top: topBar.bottom
            anchors.right: parent.right
            anchors.margins: 20
            width: 120
            height: 50
            color: "#40000000"
            radius: 8
            
            Text {
                anchors.centerIn: parent
                text: {
                    switch(cameraManager.focusMode) {
                        case 0: return "AF: Auto"
                        case 1: return "AF: Manual"
                        case 2: return "AF: Continuous"
                        case 3: return "AF: Macro"
                    }
                }
                color: "white"
                font.pixelSize: 14
            }
        }
        
        // Zoom slider (Snapchat style)
        Rectangle {
            id: zoomSlider
            anchors.verticalCenter: parent.verticalCenter
            anchors.right: parent.right
            anchors.rightMargin: 40
            width: 60
            height: 400
            color: "#40000000"
            radius: 30
            visible: false // Show on pinch gesture
            
            Slider {
                id: zoom
                anchors.centerIn: parent
                width: parent.height - 40
                height: 50
                orientation: Qt.Vertical
                from: 1.0
                to: 10.0
                value: 1.0
                
                onValueChanged: cameraManager.setZoom(value)
                
                background: Rectangle {
                    x: zoom.leftPadding + zoom.availableWidth / 2 - width / 2
                    y: zoom.topPadding
                    implicitWidth: 4
                    implicitHeight: 200
                    width: implicitWidth
                    height: zoom.availableHeight
                    radius: 2
                    color: "#40FFFFFF"
                }
                
                handle: Rectangle {
                    x: zoom.leftPadding + zoom.availableWidth / 2 - width / 2
                    y: zoom.topPadding + zoom.visualPosition * (zoom.availableHeight - height)
                    implicitWidth: 26
                    implicitHeight: 26
                    radius: 13
                    color: "white"
                    
                    Text {
                        anchors.centerIn: parent
                        text: zoom.value.toFixed(1) + "x"
                        font.pixelSize: 10
                        color: "black"
                    }
                }
            }
        }
        
        // Bottom controls (Snapchat style)
        Rectangle {
            id: bottomControls
            anchors.bottom: parent.bottom
            width: parent.width
            height: 200
            color: "#40000000"
            
            RowLayout {
                anchors.centerIn: parent
                spacing: 40
                
                // Gallery thumbnail
                Rectangle {
                    width: 80
                    height: 80
                    radius: 8
                    color: "#808080"
                    border.color: "white"
                    border.width: 2
                    
                    Text {
                        anchors.centerIn: parent
                        text: "📷"
                        font.pixelSize: 40
                    }
                    
                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            // Open gallery
                            Qt.openUrlExternally("file://" + cameraManager.activeCamera)
                        }
                    }
                }
                
                // Capture button (large circle)
                Rectangle {
                    id: captureButton
                    width: 100
                    height: 100
                    radius: 50
                    color: "white"
                    border.color: "#40FFFFFF"
                    border.width: 6
                    
                    Rectangle {
                        anchors.centerIn: parent
                        width: parent.width - 20
                        height: parent.height - 20
                        radius: (parent.radius - 10)
                        color: cameraManager.isRecording ? "red" : "white"
                        
                        SequentialAnimation on scale {
                            running: cameraManager.isRecording
                            loops: Animation.Infinite
                            NumberAnimation { to: 0.9; duration: 500 }
                            NumberAnimation { to: 1.0; duration: 500 }
                        }
                    }
                    
                    MouseArea {
                        anchors.fill: parent
                        
                        // Tap to capture photo
                        onClicked: {
                            cameraManager.capturePhoto()
                        }
                        
                        // Long press to record video
                        onPressAndHold: {
                            cameraManager.startRecording()
                        }
                        
                        onReleased: {
                            if (cameraManager.isRecording) {
                                cameraManager.stopRecording()
                            }
                        }
                    }
                }
                
                // Switch camera button
                Rectangle {
                    width: 80
                    height: 80
                    radius: 40
                    color: "#40FFFFFF"
                    
                    Text {
                        anchors.centerIn: parent
                        text: "🔄"
                        font.pixelSize: 40
                    }
                    
                    MouseArea {
                        anchors.fill: parent
                        onClicked: cameraManager.switchCamera()
                    }
                }
            }
        }
        
        // Exposure compensation slider (swipe up/down on screen)
        Rectangle {
            id: exposureIndicator
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: 20
            width: 50
            height: 300
            radius: 25
            color: "#40000000"
            visible: exposureArea.pressed
            
            Slider {
                id: exposureSlider
                anchors.centerIn: parent
                width: parent.height - 40
                height: 40
                orientation: Qt.Vertical
                from: -50
                to: 50
                value: cameraManager.exposure
                
                onValueChanged: cameraManager.setExposure(value)
            }
            
            Text {
                anchors.top: parent.top
                anchors.topMargin: 10
                anchors.horizontalCenter: parent.horizontalCenter
                text: exposureSlider.value > 0 ? "+" + exposureSlider.value : exposureSlider.value
                color: "white"
                font.pixelSize: 16
            }
        }
        
        // Touch area for exposure control
        MouseArea {
            id: exposureArea
            anchors.fill: parent
            
            property real startY: 0
            
            onPressed: {
                startY = mouseY
            }
            
            onPositionChanged: {
                if (pressed) {
                    var delta = startY - mouseY
                    exposureSlider.value = Math.max(-50, Math.min(50, delta / 5))
                }
            }
        }
        
        // Pinch to zoom
        PinchArea {
            anchors.fill: parent
            
            onPinchUpdated: {
                var scale = pinch.scale
                cameraManager.setZoom(Math.max(1.0, Math.min(10.0, scale)))
                zoomSlider.visible = true
            }
            
            onPinchFinished: {
                zoomSlider.visible = false
            }
        }
        
        // Capture flash animation
        Rectangle {
            id: captureFlashRect
            anchors.fill: parent
            color: "white"
            opacity: 0
            
            SequentialAnimation {
                id: captureFlash
                NumberAnimation { target: captureFlashRect; property: "opacity"; to: 1.0; duration: 50 }
                NumberAnimation { target: captureFlashRect; property: "opacity"; to: 0; duration: 200 }
            }
        }
        
        // Processing indicator
        Rectangle {
            id: processingIndicator
            anchors.top: parent.top
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.topMargin: 150
            width: 300
            height: 80
            radius: 8
            color: "#E0000000"
            visible: photoProcessor.processing
            
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 15
                spacing: 10
                
                RowLayout {
                    Layout.fillWidth: true
                    
                    BusyIndicator {
                        running: photoProcessor.processing
                        Layout.preferredWidth: 30
                        Layout.preferredHeight: 30
                    }
                    
                    Text {
                        text: "Processing photo..."
                        color: "white"
                        font.pixelSize: 16
                        Layout.fillWidth: true
                    }
                    
                    Text {
                        text: photoProcessor.progress + "%"
                        color: "#00FF00"
                        font.pixelSize: 16
                        font.bold: true
                    }
                }
                
                ProgressBar {
                    Layout.fillWidth: true
                    from: 0
                    to: 100
                    value: photoProcessor.progress
                    
                    background: Rectangle {
                        implicitWidth: 200
                        implicitHeight: 6
                        color: "#40FFFFFF"
                        radius: 3
                    }
                    
                    contentItem: Item {
                        implicitWidth: 200
                        implicitHeight: 6
                        
                        Rectangle {
                            width: parent.width * (photoProcessor.progress / 100)
                            height: parent.height
                            radius: 3
                            color: "#00FF00"
                        }
                    }
                }
            }
        }
        
        // Toast notification
        Rectangle {
            id: processingToast
            anchors.bottom: bottomControls.top
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottomMargin: 20
            width: Math.min(parent.width - 40, 400)
            height: 60
            radius: 30
            color: "#E0000000"
            opacity: 0
            
            property string message: ""
            
            function show(msg) {
                message = msg
                toastAnimation.start()
            }
            
            Text {
                anchors.centerIn: parent
                text: processingToast.message
                color: "white"
                font.pixelSize: 16
            }
            
            SequentialAnimation {
                id: toastAnimation
                NumberAnimation { target: processingToast; property: "opacity"; to: 1.0; duration: 200 }
                PauseAnimation { duration: 2000 }
                NumberAnimation { target: processingToast; property: "opacity"; to: 0; duration: 200 }
            }
        }
        
        // Location indicator (when location is enabled)
        Rectangle {
            visible: photoProcessor.locationEnabled && photoProcessor.currentLocation.isValid
            anchors.bottom: bottomControls.top
            anchors.left: parent.left
            anchors.margins: 20
            width: 180
            height: 50
            color: "#40000000"
            radius: 8
            
            RowLayout {
                anchors.fill: parent
                anchors.margins: 10
                
                Text {
                    text: "📍"
                    font.pixelSize: 20
                }
                
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    
                    Text {
                        text: "Location: ON"
                        color: "#00FF00"
                        font.pixelSize: 12
                        font.bold: true
                    }
                    
                    Text {
                        text: photoProcessor.currentLocation.isValid ? 
                              photoProcessor.currentLocation.latitude.toFixed(4) + ", " +
                              photoProcessor.currentLocation.longitude.toFixed(4) : "Waiting..."
                        color: "white"
                        font.pixelSize: 10
                    }
                }
            }
        }
    }
    
    // Camera selector drawer
    Drawer {
        id: cameraSelector
        width: parent.width * 0.8
        height: parent.height
        edge: Qt.LeftEdge
        
        ListView {
            anchors.fill: parent
            model: cameraManager.cameras
            
            delegate: ItemDelegate {
                width: parent.width
                height: 80
                
                RowLayout {
                    anchors.fill: parent
                    anchors.margins: 10
                    
                    Rectangle {
                        width: 60
                        height: 60
                        radius: 30
                        color: "#4000FF00"
                        
                        Text {
                            anchors.centerIn: parent
                            text: "📹"
                            font.pixelSize: 30
                        }
                    }
                    
                    ColumnLayout {
                        Layout.fillWidth: true
                        
                        Text {
                            text: modelData.name
                            font.pixelSize: 18
                            font.bold: true
                        }
                        
                        Text {
                            text: modelData.id
                            font.pixelSize: 12
                            color: "#808080"
                        }
                        
                        Text {
                            text: modelData.type + " • " + modelData.driver
                            font.pixelSize: 10
                            color: "#606060"
                        }
                    }
                    
                    Rectangle {
                        visible: modelData.id === cameraManager.activeCamera
                        width: 30
                        height: 30
                        radius: 15
                        color: "#00FF00"
                        
                        Text {
                            anchors.centerIn: parent
                            text: "✓"
                            color: "white"
                            font.pixelSize: 20
                        }
                    }
                }
                
                onClicked: {
                    cameraManager.selectCamera(modelData.id)
                    cameraSelector.close()
                }
            }
        }
    }
    
    // Settings drawer
    Drawer {
        id: settingsDrawer
        width: parent.width
        height: parent.height * 0.7
        edge: Qt.BottomEdge
        
        Flickable {
            anchors.fill: parent
            contentHeight: settingsColumn.height
            
            ColumnLayout {
                id: settingsColumn
                width: parent.width
                spacing: 20
                
                // Header
                Rectangle {
                    Layout.fillWidth: true
                    height: 80
                    color: "#F0F0F0"
                    
                    Text {
                        anchors.centerIn: parent
                        text: "Camera Settings"
                        font.pixelSize: 24
                        font.bold: true
                    }
                }
                
                // Focus mode
                GroupBox {
                    Layout.fillWidth: true
                    Layout.margins: 20
                    title: "Focus Mode"
                    
                    ColumnLayout {
                        width: parent.width
                        
                        RadioButton {
                            text: "Auto Focus"
                            checked: cameraManager.focusMode === 0
                            onClicked: cameraManager.setFocusMode(0)
                        }
                        RadioButton {
                            text: "Manual Focus"
                            checked: cameraManager.focusMode === 1
                            onClicked: cameraManager.setFocusMode(1)
                        }
                        RadioButton {
                            text: "Continuous Focus"
                            checked: cameraManager.focusMode === 2
                            onClicked: cameraManager.setFocusMode(2)
                        }
                        RadioButton {
                            text: "Macro Focus"
                            checked: cameraManager.focusMode === 3
                            onClicked: cameraManager.setFocusMode(3)
                        }
                    }
                }
                
                // White balance
                GroupBox {
                    Layout.fillWidth: true
                    Layout.margins: 20
                    title: "White Balance"
                    
                    GridLayout {
                        width: parent.width
                        columns: 2
                        
                        RadioButton {
                            text: "Auto"
                            checked: cameraManager.whiteBalance === 0
                            onClicked: cameraManager.setWhiteBalance(0)
                        }
                        RadioButton {
                            text: "Daylight"
                            checked: cameraManager.whiteBalance === 1
                            onClicked: cameraManager.setWhiteBalance(1)
                        }
                        RadioButton {
                            text: "Cloudy"
                            checked: cameraManager.whiteBalance === 2
                            onClicked: cameraManager.setWhiteBalance(2)
                        }
                        RadioButton {
                            text: "Tungsten"
                            checked: cameraManager.whiteBalance === 3
                            onClicked: cameraManager.setWhiteBalance(3)
                        }
                        RadioButton {
                            text: "Fluorescent"
                            checked: cameraManager.whiteBalance === 4
                            onClicked: cameraManager.setWhiteBalance(4)
                        }
                    }
                }
                
                // Performance options
                GroupBox {
                    Layout.fillWidth: true
                    Layout.margins: 20
                    title: "Performance"
                    
                    ColumnLayout {
                        width: parent.width
                        
                        Switch {
                            text: "Use hardware acceleration"
                            checked: true
                        }
                        
                        Switch {
                            text: "Multi-threaded processing"
                            checked: true
                        }
                        
                        Switch {
                            text: "GPU rendering"
                            checked: true
                        }
                    }
                }
                
                // Metadata options
                GroupBox {
                    Layout.fillWidth: true
                    Layout.margins: 20
                    title: "Photo Metadata"
                    
                    ColumnLayout {
                        width: parent.width
                        
                        Switch {
                            text: "Enable GPS location"
                            checked: photoProcessor.locationEnabled
                            onToggled: photoProcessor.setLocationEnabled(checked)
                        }
                        
                        Text {
                            visible: photoProcessor.locationEnabled
                            text: photoProcessor.currentLocation.isValid ? 
                                  "Current: " + photoProcessor.currentLocation.latitude.toFixed(4) + ", " +
                                  photoProcessor.currentLocation.longitude.toFixed(4) :
                                  "Waiting for GPS fix..."
                            font.pixelSize: 12
                            color: photoProcessor.currentLocation.isValid ? "#00FF00" : "#FFA500"
                            Layout.leftMargin: 30
                        }
                        
                        Switch {
                            text: "Save camera settings to EXIF"
                            checked: true
                        }
                        
                        Switch {
                            text: "Save device ID to metadata"
                            checked: true
                        }
                        
                        Text {
                            text: "Metadata includes: timestamp, location, camera model,\nfocus mode, white balance, exposure, zoom level,\nHDR/night mode status, and ToF distance (if available)"
                            font.pixelSize: 10
                            color: "#808080"
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                            Layout.leftMargin: 20
                            Layout.topMargin: 10
                        }
                    }
                }
                
                // About
                GroupBox {
                    Layout.fillWidth: true
                    Layout.margins: 20
                    title: "About"
                    
                    ColumnLayout {
                        width: parent.width
                        
                        Text {
                            text: "PhotoSync Camera v0.1.0"
                            font.pixelSize: 16
                        }
                        
                        Text {
                            text: "libcamera integration with Qt"
                            font.pixelSize: 12
                            color: "#808080"
                        }
                    }
                }
            }
        }
    }
    
    // Error dialog
    Dialog {
        id: errorDialog
        property alias text: errorText.text
        
        anchors.centerIn: parent
        title: "Error"
        standardButtons: Dialog.Ok
        
        Text {
            id: errorText
            wrapMode: Text.WordWrap
        }
    }
    
    Component.onCompleted: {
        cameraManager.startPreview()
    }
}
