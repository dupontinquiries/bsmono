// qml/ArtistsPage.qml
//
// Artist aggregation. Two view modes:
//   * "Top" — sorted by total play count (default)
//   * "A→Z" — alphabetical
//
// Each row shows the artist name, track count, and total play count.
// Tapping an artist navigates to ArtistDetailPage with their tracks.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    signal openArtist(int id, string name)

    property string mode: "top"   // "top" or "alpha"

    // ── Pagination state for the A→Z view ───────────────────────────
    //
    // The Top view caps at 50 artists, so it's fine to load in one
    // shot. The A→Z view has no natural cap — a 5000-artist library
    // serialized as one JSON string is 250 KB, the ListView would
    // try to instantiate every delegate up-front, and the UI freezes
    // for a beat on entry. The fix is to fetch 100 at a time, append
    // as the user scrolls.
    //
    // alphaList    — accumulator we append pages onto
    // alphaTotal   — declared total (from the first Page response)
    // alphaLoading — true while a fetch is in flight; gates the
    //                onContentYChanged trigger so we don't issue
    //                duplicate requests for the same offset
    property var alphaList: []
    property int alphaTotal: 0
    property bool alphaLoading: false
    readonly property int alphaPageSize: 100

    Component.onCompleted: refresh()
    function refresh() {
        if (root.mode === "top") {
            qtobj.load_top_artists(50)
        } else {
            // A→Z mode: reset accumulator and fetch the first page.
            root.alphaList = []
            root.alphaTotal = 0
            root.alphaLoading = true
            qtobj.load_artists_page(0, root.alphaPageSize)
        }
    }

    /// Fetch one more page if we're not at the end and not already
    /// fetching. Called by the ListView's onContentYChanged when
    /// the bottom of the visible window is near the model's tail.
    function maybeFetchMore() {
        if (root.mode !== "alpha") return;
        if (root.alphaLoading) return;
        if (root.alphaTotal > 0 && root.alphaList.length >= root.alphaTotal) return;
        root.alphaLoading = true
        qtobj.load_artists_page(root.alphaList.length, root.alphaPageSize)
    }

    property var artistsList: {
        try { return JSON.parse(qtobj.artists_json || "[]") }
        catch (e) { return [] }
    }
    Connections {
        target: qtobj
        function onArtists_changed() {
            // Only adopt the list when the worker's context matches our mode.
            // Otherwise a stale "top" response shouldn't overwrite the
            // user's just-toggled "alpha" view.
            if (qtobj.artists_context === root.mode) {
                try { root.artistsList = JSON.parse(qtobj.artists_json || "[]") }
                catch (e) { root.artistsList = [] }
            }
        }
        // Paginated arrivals land here for the A→Z view. Wrapper shape:
        // { context, offset, total, items: [...] }
        //
        // We only consume Page results when we're in alpha mode. If the
        // user toggled to Top between request and response, the result
        // is stale — drop it (the next refresh will fix the model).
        function onPage_changed() {
            if (root.mode !== "alpha") return;
            var p = null
            try { p = JSON.parse(qtobj.page_json || "{}") }
            catch (e) { return }
            if (!p || p.context !== "artists") return;
            // Accept if the offset matches the next slot we expect.
            // If not, it's a stale or out-of-order page from a
            // previous in-flight request — discard.
            if (p.offset !== root.alphaList.length) {
                root.alphaLoading = false
                return
            }
            // Append the new items into the accumulator.
            root.alphaList = root.alphaList.concat(p.items || [])
            root.alphaTotal = p.total
            root.alphaLoading = false
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        // Header + mode switcher
        Row {
            width: parent.width
            spacing: Bo.gapSM
            Text {
                text: "ARTISTS"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
                anchors.verticalCenter: parent.verticalCenter
            }
            Item { width: parent.width - 200; height: 1 }    // spacer
            Row {
                spacing: 4
                anchors.verticalCenter: parent.verticalCenter
                ModeTab {
                    label: "Top"; selected: root.mode === "top"
                    accent: root.accent
                    onTapped: { root.mode = "top"; root.refresh() }
                }
                ModeTab {
                    label: "A→Z"; selected: root.mode === "alpha"
                    accent: root.accent
                    onTapped: { root.mode = "alpha"; root.refresh() }
                }
            }
        }

        // Sort chips — only meaningful in A→Z (full browse) mode; TOP is
        // already play-ranked.
        Flow {
            width: parent.width
            spacing: 4
            visible: root.mode === "alpha"
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
                    { tag: "name",   label: "Name"        },
                    { tag: "tracks", label: "Track count" }
                ]
                delegate: Rectangle {
                    property bool sel: qtobj.artist_sort === modelData.tag
                    width: artSortLabel.implicitWidth + 14
                    height: 24
                    radius: 12
                    color: sel ? Qt.rgba(0.78, 0.63, 0.29, 0.25) : "transparent"
                    border.color: sel ? root.accent : Bo.hairline
                    border.width: sel ? 2 : 1
                    Text {
                        id: artSortLabel
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
                                qtobj.set_artist_sort(modelData.tag)
                                root.refresh()
                            }
                        }
                    }
                }
            }
        }

        ListView {
            // Stronger scroll feel (momentum-y flings).
            flickDeceleration: Bo.flickDecel
            maximumFlickVelocity: Bo.flickMaxVel
            boundsBehavior: Flickable.DragOverBounds
            pressDelay: Bo.pressDelay
            rebound: BoRebound {}
            // Clear the BottomTransportBar so the final row isn't
            // hidden behind it (bottom-cutoff fix).
            bottomMargin: Bo.bottomChromeHeight
            id: artistView
            width: parent.width
            height: parent.height - 60
            spacing: 4
            clip: true
            // Top view uses the unpaginated artistsList; A→Z uses
            // the paginated alphaList. The QML binding swaps the
            // model on mode change.
            model: root.mode === "top" ? root.artistsList : root.alphaList
            cacheBuffer: 240
            ScrollIndicator.vertical: ScrollIndicator {}

            // Near-bottom prefetch — kick off the next page when the
            // viewport's bottom edge gets within 4 row-heights of the
            // current model's end. The 4-row buffer keeps the user
            // from seeing the load gap mid-scroll; on a fast scroll,
            // the next page arrives before the gap is reached.
            onContentYChanged: {
                if (root.mode !== "alpha") return;
                var rowH = 60   // delegate height (56) + spacing (4)
                var viewportBottom = contentY + height
                var modelBottom = contentHeight
                if (modelBottom - viewportBottom < rowH * 4) {
                    root.maybeFetchMore()
                }
            }

            Text {
                anchors.centerIn: parent
                visible: parent.count === 0 && !root.alphaLoading
                text: "No artists yet — scan your library first"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
            }

            // Loading footer — visible at the bottom of the list
            // while the next page is in flight. Reuses the
            // pulse-dot pattern from the diagnostics card so the
            // user gets a consistent "something's happening" signal.
            footer: Item {
                width: ListView.view ? ListView.view.width : 0
                height: root.alphaLoading ? 32 : 0
                visible: root.alphaLoading
                Row {
                    anchors.centerIn: parent
                    spacing: 8
                    Rectangle {
                        width: 8; height: 8; radius: 4
                        color: root.accent
                        anchors.verticalCenter: parent.verticalCenter
                        SequentialAnimation on opacity {
                            running: root.alphaLoading; loops: Animation.Infinite
                            NumberAnimation { to: 0.3; duration: 500 }
                            NumberAnimation { to: 1.0; duration: 500 }
                        }
                    }
                    Text {
                        text: "Loading more artists…"
                        color: Bo.silver
                        font.pixelSize: 10
                        anchors.verticalCenter: parent.verticalCenter
                    }
                }
            }

            delegate: Rectangle {
                width: ListView.view.width
                height: 56
                radius: 6
                color: rowMa.pressed ? Bo.surfaceHi : "transparent"

                Row {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    spacing: Bo.gapSM

                    // Rank number for "top" view, initial letter for "A→Z"
                    Rectangle {
                        width: 32; height: 32; radius: 16
                        anchors.verticalCenter: parent.verticalCenter
                        color: Bo.mix(Bo.surface, root.accent, 0.15)
                        border.color: Bo.hairline; border.width: 1
                        Text {
                            anchors.centerIn: parent
                            text: root.mode === "top"
                                ? (index + 1).toString()
                                : (modelData.name && modelData.name.length > 0
                                     ? modelData.name[0].toUpperCase() : "?")
                            color: root.accent
                            font.pixelSize: 11
                            font.weight: Font.DemiBold
                        }
                    }

                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        width: parent.width - 32 - Bo.gapSM
                        Text {
                            text: modelData.name || "(unknown)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize
                            font.weight: Font.Medium
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Text {
                            text: modelData.track_count + " track"
                                + (modelData.track_count === 1 ? "" : "s")
                                + " · " + (modelData.total_plays || 0) + " play"
                                + (modelData.total_plays === 1 ? "" : "s")
                            color: Bo.silver
                            font.pixelSize: 10
                            elide: Text.ElideRight
                            width: parent.width
                        }
                    }
                }
                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    onClicked: root.openArtist(modelData.id, modelData.name)
                }
            }
        }
    }

    component ModeTab: Rectangle {
        id: modeTab
        property string label: ""
        property bool selected: false
        property color accent: "#C8A04B"
        signal tapped()
        width: 48; height: 24; radius: 12
        // Selected = filled accent. Unselected but pressed = a subtle
        // surface tint so the tap registers immediately, even before the
        // (possibly slow) data reload swaps the selection.
        color: selected ? accent
             : (modeTabMa.pressed ? Bo.surfaceHi : "transparent")
        border.color: selected ? accent : Bo.hairline
        border.width: 1
        Behavior on color { ColorAnimation { duration: Bo.quick } }
        Behavior on border.color { ColorAnimation { duration: Bo.quick } }
        Text {
            anchors.centerIn: parent
            text: parent.label
            color: parent.selected ? Bo.graphite : Bo.silver
            font.pixelSize: 10
            font.weight: Font.DemiBold
        }
        MouseArea { id: modeTabMa; anchors.fill: parent; onClicked: parent.tapped() }
    }
}
