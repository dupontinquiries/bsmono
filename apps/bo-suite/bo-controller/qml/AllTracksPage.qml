// qml/AllTracksPage.qml
//
// Flat, title-ordered list of EVERY track in the library, regardless of
// how its metadata parsed. This is the UI half of the missing-tracks fix:
// the artist/album/tag browses all join through track_artists, so a file
// whose name yielded no artist (e.g. "Serenitas.opus") is invisible there
// even though it scanned fine. This page reads from all_tracks_page, which
// selects FROM tracks with no inner join, so nothing can be unreachable.
//
// Paginated (100/page) with near-bottom prefetch, mirroring the A→Z
// artist view: a flat list of a few thousand tracks serialized as one
// JSON blob would jank on entry, so we append pages as the user scrolls.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"

    // ── Pagination accumulator ──────────────────────────────────────
    property var trackList: []
    property int trackTotal: 0
    property bool loading: false
    readonly property int pageSize: 100

    Component.onCompleted: refresh()

    function refresh() {
        root.trackList = []
        root.trackTotal = 0
        root.loading = true
        qtobj.load_all_tracks_page(0, root.pageSize)
    }

    // Fetch the next page if there is one and we're not already loading.
    function maybeFetchMore() {
        if (root.loading) return;
        if (root.trackTotal > 0 && root.trackList.length >= root.trackTotal) return;
        root.loading = true
        qtobj.load_all_tracks_page(root.trackList.length, root.pageSize)
    }

    Connections {
        target: qtobj
        function onPage_changed() {
            var p = null
            try { p = JSON.parse(qtobj.page_json || "{}") }
            catch (e) { return }
            if (!p || p.context !== "all_tracks") return;
            // Out-of-order / stale page guard: only accept the page that
            // begins exactly where our accumulator currently ends.
            if (p.offset !== root.trackList.length) {
                root.loading = false
                return
            }
            root.trackList = root.trackList.concat(p.items || [])
            root.trackTotal = p.total
            root.loading = false
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        // Header: title + live count.
        Row {
            width: parent.width
            spacing: Bo.gapSM
            Text {
                text: "ALL TRACKS"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
                anchors.verticalCenter: parent.verticalCenter
            }
            Item { width: parent.width - 220; height: 1 }   // spacer
            Text {
                // "1,234 tracks" — show the declared total once known,
                // else the count loaded so far.
                text: {
                    var n = root.trackTotal > 0 ? root.trackTotal : root.trackList.length
                    return n + (n === 1 ? " track" : " tracks")
                }
                color: Bo.slate
                font.pixelSize: 10
                anchors.verticalCenter: parent.verticalCenter
            }
        }

        // Sort selector — changes order and reloads from page 0. Persisted
        // server-side so the choice sticks across launches.
        Flow {
            width: parent.width
            spacing: 4
            Text {
                text: "SORT"
                color: Bo.slate
                font.pixelSize: 9
                font.weight: Font.DemiBold
                font.letterSpacing: 1.0
                height: 24
                verticalAlignment: Text.AlignVCenter
            }
            Repeater {
                model: [
                    { tag: "title",  label: "Title"  },
                    { tag: "artist", label: "Artist" },
                    { tag: "album",  label: "Album"  },
                    { tag: "recent", label: "Recent" },
                    { tag: "rating", label: "Rating" }
                ]
                delegate: Rectangle {
                    property bool sel: qtobj.all_tracks_sort === modelData.tag
                    width: sortLabel.implicitWidth + 14
                    height: 24
                    radius: 12
                    color: sel ? Qt.rgba(0.78, 0.63, 0.29, 0.25) : "transparent"
                    border.color: sel ? Bo.accent : Bo.hairline
                    border.width: sel ? 2 : 1
                    Text {
                        id: sortLabel
                        anchors.centerIn: parent
                        text: modelData.label
                        color: sel ? Bo.bone : Bo.silver
                        font.pixelSize: 10
                        font.weight: sel ? Font.DemiBold : Font.Normal
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            if (!parent.sel) {
                                qtobj.set_all_tracks_sort(modelData.tag)
                                root.refresh()
                            }
                        }
                    }
                }
            }
        }

        // Empty state — only while we genuinely have nothing AND aren't
        // mid-load (so it doesn't flash on entry before the first page).
        Text {
            visible: root.trackList.length === 0 && !root.loading
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: "No tracks yet. Scan a library folder from Settings."
            color: Bo.slate
            font.pixelSize: Bo.bodySize
            wrapMode: Text.WordWrap
        }

        ListView {
            id: trackView
            flickDeceleration: Bo.flickDecel
            maximumFlickVelocity: Bo.flickMaxVel
            boundsBehavior: Flickable.DragOverBounds
            pressDelay: Bo.pressDelay
            rebound: BoRebound {}
            bottomMargin: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 60
            spacing: 2
            clip: true
            model: root.trackList
            cacheBuffer: 240
            ScrollIndicator.vertical: ScrollIndicator {}

            // Near-bottom prefetch — kick the next page when the bottom of
            // the viewport gets within ~5 rows of the model's tail, so the
            // load gap is hidden during a normal scroll.
            onContentYChanged: {
                var rowH = 50   // delegate height (48) + spacing (2)
                var nearBottom =
                    (contentY + height) > (contentHeight - rowH * 5)
                if (nearBottom) root.maybeFetchMore()
            }

            delegate: Rectangle {
                width: ListView.view.width
                height: 48
                radius: 6
                color: rowMa.pressed ? Bo.surfaceHi : "transparent"

                Row {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    spacing: Bo.gapSM

                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        width: parent.width - 92
                        spacing: 1
                        Text {
                            text: modelData.title || "(untitled)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Row {
                            spacing: Bo.gapSM
                            width: parent.width
                            Text {
                                text: (modelData.primary_artist || "Unknown Artist")
                                    + (modelData.album && modelData.album.length > 0
                                           ? " · " + modelData.album : "")
                                color: Bo.silver
                                font.pixelSize: 10
                                elide: Text.ElideRight
                                width: Math.min(implicitWidth, parent.width - 76)
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            StarRating {
                                trackId: modelData.id
                                rating: modelData.rating || 0
                                starSize: 11
                                anchors.verticalCenter: parent.verticalCenter
                            }
                        }
                    }

                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        // Tag (#)
                        Rectangle {
                            width: 26; height: 24; radius: 12
                            color: tagMa.pressed ? Bo.surfaceHi : "transparent"
                            Text { anchors.centerIn: parent; text: "#"
                                color: Bo.silver; font.pixelSize: 12
                                font.weight: Font.Bold }
                            MouseArea {
                                id: tagMa
                                anchors.fill: parent
                                onClicked: {
                                    if (window.tagEditor) window.tagEditor.open(modelData)
                                }
                            }
                        }
                        // Queue (+)
                        Rectangle {
                            width: 26; height: 24; radius: 12
                            color: queueMa.pressed ? Bo.surfaceHi : "transparent"
                            Text { anchors.centerIn: parent; text: "+"
                                color: Bo.silver; font.pixelSize: 13 }
                            MouseArea {
                                id: queueMa
                                anchors.fill: parent
                                onClicked: qtobj.queue_push(modelData.id)
                            }
                        }
                        // Play now (▶)
                        Rectangle {
                            width: 26; height: 24; radius: 12
                            color: root.accent
                            Text { anchors.centerIn: parent; text: "▶"
                                color: Bo.graphite; font.pixelSize: 9
                                font.weight: Font.DemiBold }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: qtobj.play_now(modelData.id)
                            }
                        }
                    }
                }
                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    // Tap = enqueue if something's playing, else play.
                    // The ▶ button forces instant play.
                    onClicked: qtobj.enqueue_or_play(modelData.id)
                    // Long-press opens the edit sheet (title/artist/album/
                    // year/rating) — the same dialog used elsewhere.
                    onPressAndHold: {
                        if (window.editDialog) window.editDialog.open(modelData)
                    }
                }
            }
        }
    }
}
