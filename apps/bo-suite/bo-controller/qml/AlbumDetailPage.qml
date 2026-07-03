// qml/AlbumDetailPage.qml
//
// Track listing for a single album. Shown when the user taps an
// album card on AlbumsPage. Header shows album title + primary
// artist + year + a Play All button; track rows reuse the existing
// TrackRow component (so the look is consistent with Library /
// Search / Artist-detail).

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root

    /// Emitted when the back chevron is tapped. Parent navigates
    /// back to AlbumsPage (keeps the user's TOP/ALPHA mode intact).
    signal backTapped()

    /// Accent color for highlights (Play All button, etc). Defaults
    /// to the same B&O gold the rest of the controller uses; parent
    /// can override.
    property color accent: "#C8A04B"

    property var tracks: {
        try { return JSON.parse(qtobj.current_album_tracks_json || "[]") }
        catch (e) { return [] }
    }
    Connections {
        target: qtobj
        function onCurrent_album_changed() {
            try { root.tracks = JSON.parse(qtobj.current_album_tracks_json || "[]") }
            catch (e) { root.tracks = [] }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        // ── Header: back chevron + album title + artist ───────
        Row {
            width: parent.width
            spacing: Bo.gapSM

            // Back chevron
            Rectangle {
                width: 32; height: 32; radius: 16
                color: backMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "‹"
                    color: Bo.bone
                    font.pixelSize: 18
                }
                MouseArea {
                    id: backMa
                    anchors.fill: parent
                    onClicked: {
                        qtobj.close_album()
                        root.backTapped()
                    }
                }
            }

            Column {
                width: parent.width - 32 - Bo.gapSM - 110
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    text: qtobj.current_album_title || "(no album)"
                    color: Bo.bone
                    font.pixelSize: 16
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                    width: parent.width
                }
                Text {
                    text: qtobj.current_album_artist || "—"
                    color: Bo.slate
                    font.pixelSize: 12
                    elide: Text.ElideRight
                    width: parent.width
                }
            }

            // Play All button — queues every track in order on the
            // active device. Greyed-out when no tracks (album not
            // loaded yet or empty).
            Rectangle {
                width: 100; height: 32; radius: 16
                anchors.verticalCenter: parent.verticalCenter
                color: playAllMa.enabled
                    ? (playAllMa.pressed
                        ? Bo.mix(Bo.surface, "#4a7c59", 0.7)
                        : Bo.mix(Bo.surface, "#4a7c59", 0.55))
                    : Bo.mix(Bo.surface, "#4a7c59", 0.20)
                border.color: "#4a7c59"; border.width: 1
                Text {
                    anchors.centerIn: parent
                    text: "▶ Play All"
                    color: playAllMa.enabled ? Bo.bone : Bo.slate
                    font.pixelSize: 11
                    font.weight: Font.DemiBold
                }
                MouseArea {
                    id: playAllMa
                    anchors.fill: parent
                    enabled: root.tracks.length > 0
                          && qtobj.active_url.length > 0
                    onClicked: {
                        // Queue every track in the album. First one
                        // gets play_track() so playback starts; the
                        // rest go to the active device's queue.
                        for (var i = 0; i < root.tracks.length; ++i) {
                            var t = root.tracks[i]
                            if (i === 0) qtobj.play_track(t.id || 0)
                            else         qtobj.queue_push(t.id || 0)
                        }
                    }
                }
            }
        }

        // Metadata strip — total length + play count info.
        Text {
            width: parent.width
            text: root.tracks.length + " track"
                + (root.tracks.length === 1 ? "" : "s")
                + " · tap any track to play it on " + (qtobj.product_name || "active device")
            color: Bo.slate
            font.pixelSize: 10
        }

        // ── Track list ────────────────────────────────────────
        ScrollView {
            // Reserve space for the BottomTransportBar so the last
            // row/card isn't clipped or overlapped (bottom-cutoff fix).
            bottomPadding: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 90
            clip: true
            visible: root.tracks.length > 0
            // ScrollView wraps an internal Flickable; tune its fling for
            // the same momentum feel as the ListView pages.
            Component.onCompleted: {
                if (contentItem) {
                    contentItem.flickDeceleration = Bo.flickDecel
                    contentItem.maximumFlickVelocity = Bo.flickMaxVel
                    contentItem.boundsBehavior = Flickable.DragOverBounds
                    contentItem.pressDelay = Bo.pressDelay
                }
            }

            Column {
                width: root.width - Bo.gapMD * 2
                spacing: 2
                Repeater {
                    model: root.tracks
                    delegate: TrackRow {
                        width: parent.width
                        // TrackRow takes individual props (title/artist
                        // /album/bpm/camelot) and emits `activated`,
                        // `longPressed`, `tagRequested` — not a generic
                        // `trackData` blob or a `tapped` signal. Match
                        // the canonical Library-page usage.
                        title: modelData.title || ""
                        artist: modelData.primary_artist || ""
                        album: modelData.album || ""
                        bpm: modelData.bpm
                        camelot: modelData.camelot || ""
                        accent: root.accent
                        onActivated: {
                            qtobj.play_track(modelData.id || 0)
                            qtobj.load_key_bpm(modelData.id || 0)
                        }
                        onLongPressed: {
                            // Defer to the same edit dialog the
                            // Library page uses, accessed via the
                            // window.editDialog property if exposed.
                            if (window.editDialog) {
                                window.editDialog.open(modelData)
                            }
                        }
                        onTagRequested: {
                            if (window.tagEditor) {
                                window.tagEditor.open(modelData)
                            }
                        }
                    }
                }
            }
        }

        Text {
            visible: root.tracks.length === 0
            width: parent.width
            text: "Loading tracks…"
            color: Bo.slate
            font.pixelSize: 11
        }
    }
}
