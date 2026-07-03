import QtQuick 2.15
import "."

// A compact, tappable 5-star rating control.
//
//   StarRating {
//       trackId: modelData.id
//       rating: modelData.rating || 0
//   }
//
// Tapping a star sets the rating to that value (1–5); tapping the star
// that already equals the current rating clears it back to 0 (the common
// "tap the 3rd star again to un-rate" gesture). Updates optimistically and
// calls qtobj.set_track_rating; the backend echoes the confirmed value
// back via last_rated_track / last_rated_value, which we reconcile so the
// widget self-heals if a write was clamped or rejected.
//
// Read-only mode (interactive: false) just displays the stars — handy in
// dense lists where you only want to show, not edit.

Row {
    id: root

    property int trackId: -1
    property int rating: 0
    property bool interactive: true
    property real starSize: 14
    property color filledColor: Bo.accent
    property color emptyColor: Bo.slate

    spacing: 1

    // Reconcile with the backend's confirmed value for THIS track.
    Connections {
        target: qtobj
        function onRating_changed() {
            if (qtobj.last_rated_track === root.trackId) {
                root.rating = qtobj.last_rated_value
            }
        }
    }

    Repeater {
        model: 5
        delegate: Item {
            width: root.starSize
            height: root.starSize

            Text {
                anchors.centerIn: parent
                text: (index < root.rating) ? "★" : "☆"
                color: (index < root.rating) ? root.filledColor : root.emptyColor
                font.pixelSize: root.starSize
            }

            MouseArea {
                anchors.fill: parent
                enabled: root.interactive
                onClicked: {
                    var target = index + 1
                    // Tapping the current rating's top star clears it.
                    if (target === root.rating) target = 0
                    root.rating = target            // optimistic
                    if (root.trackId >= 0)
                        qtobj.set_track_rating(root.trackId, target)
                }
            }
        }
    }
}
