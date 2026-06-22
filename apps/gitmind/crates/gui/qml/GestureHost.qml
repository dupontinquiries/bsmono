// Reusable gesture surface adapted from the web2qml reference: one input
// owner drives press → drag → release → commit/cancel, exposing normalized
// signals so callers don't each reinvent drag math. (Used by views that
// need custom pan/scrub beyond what SwipeDelegate covers.)
import QtQuick

Item {
    id: host
    property real threshold: 40
    signal committed(real dx, real dy)
    signal cancelled()

    MouseArea {
        id: ma
        anchors.fill: parent
        property real ox: 0
        property real oy: 0
        onPressed: (m) => { ox = m.x; oy = m.y }
        onReleased: (m) => {
            var dx = m.x - ox, dy = m.y - oy
            if (Math.abs(dx) >= host.threshold || Math.abs(dy) >= host.threshold)
                host.committed(dx, dy)
            else
                host.cancelled()
        }
    }
}
