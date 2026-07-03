import QtQuick 2.15
import "."

// A friction-y settle for Flickable/ListView overshoot rebound. The
// default rebound on OvershootBounds snaps back fairly hard; a damped
// decelerating return reads as physical — the list eases back to the edge
// and settles. OutCubic (not OutBack) is deliberate: the content is
// already returning FROM an overshoot, so a back-easing would add a
// second wobble. A smooth decelerating curve gives one clean settle,
// which pairs well with the longer-glide flick tuning (low deceleration +
// high max velocity) so a hard fling into the end feels cushioned.
Transition {
    NumberAnimation {
        properties: "x,y"
        duration: Bo.reboundDur
        easing.type: Easing.OutCubic
    }
}
