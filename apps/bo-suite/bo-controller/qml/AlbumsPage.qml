// qml/AlbumsPage.qml
//
// Browse-by-album view. Mirrors ArtistsPage's two-mode design:
// a "TOP" tab listing the most-played albums by aggregate play
// count, and an "ALPHA" tab listing every album alphabetically by
// title. The user can tap any album to open AlbumDetailPage.
//
// Layout decisions:
//   * Flow grid of card-shaped buttons (matches Tags / Artists).
//     Each card is wide enough to show title + artist + a metadata
//     row (year • N tracks • plays).
//   * Cards are tinted by play-count bucket so heavily-played
//     albums catch the eye in the top tab.
//   * The active mode tab is highlighted with the bronze accent;
//     inactive ones are slate. Tabs sit above the scroll area.
//
// Data flow:
//   * Mode change calls qtobj.load_top_albums(40) or
//     qtobj.load_all_albums_alpha(); result lands in albums_json.
//   * Tap → qtobj.open_album(title, primary_artist); navigation
//     to detail handled by the parent SwipeView via a signal.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    /// Emitted when the user taps an album. Parent navigates to
    /// AlbumDetailPage and Pre-fills via open_album().
    signal albumTapped(string title, string primaryArtist)

    /// Parsed albums list — refreshed when albums_json changes.
    property var albums: {
        try { return JSON.parse(qtobj.albums_json || "[]") }
        catch (e) { return [] }
    }
    /// Loaded mode (top vs alpha). Drives tab visual state + the
    /// initial load on first show.
    property string mode: qtobj.albums_context || "top"

    Connections {
        target: qtobj
        function onAlbums_changed() {
            try { root.albums = JSON.parse(qtobj.albums_json || "[]") }
            catch (e) { root.albums = [] }
            root.mode = qtobj.albums_context
        }
    }

    /// On first show, request the top albums. Subsequent re-shows
    /// keep whatever the user last loaded.
    Component.onCompleted: {
        if (!qtobj.albums_json || qtobj.albums_json === "[]") {
            qtobj.load_top_albums(40)
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        // ── Header + mode tabs ────────────────────────────────
        Row {
            width: parent.width
            spacing: Bo.gapSM
            Text {
                text: "ALBUMS"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                text: "· " + root.albums.length + " loaded"
                color: Bo.slate
                font.pixelSize: 10
                anchors.verticalCenter: parent.verticalCenter
            }
            Item { width: 12; height: 1 }  // spacer

            // TOP tab
            Rectangle {
                width: 64; height: 26; radius: 13
                anchors.verticalCenter: parent.verticalCenter
                color: root.mode === "top"
                    ? Bo.mix(Bo.surface, "#C8A04B", 0.30)
                    : (topMa.pressed ? Bo.surfaceHi : "transparent")
                border.color: root.mode === "top" ? "#C8A04B" : Bo.hairline
                border.width: 1
                Behavior on color { ColorAnimation { duration: Bo.quick } }
                Text {
                    anchors.centerIn: parent
                    text: "TOP"
                    color: root.mode === "top" ? "#C8A04B" : Bo.bone
                    font.pixelSize: 10
                    font.weight: Font.DemiBold
                    font.letterSpacing: 0.8
                }
                MouseArea {
                    id: topMa
                    anchors.fill: parent
                    onClicked: qtobj.load_top_albums(40)
                }
            }
            // ALPHA tab
            Rectangle {
                width: 70; height: 26; radius: 13
                anchors.verticalCenter: parent.verticalCenter
                color: root.mode === "alpha"
                    ? Bo.mix(Bo.surface, "#C8A04B", 0.30)
                    : (alphaMa.pressed ? Bo.surfaceHi : "transparent")
                border.color: root.mode === "alpha" ? "#C8A04B" : Bo.hairline
                border.width: 1
                Behavior on color { ColorAnimation { duration: Bo.quick } }
                Text {
                    anchors.centerIn: parent
                    text: "ALPHA"
                    color: root.mode === "alpha" ? "#C8A04B" : Bo.bone
                    font.pixelSize: 10
                    font.weight: Font.DemiBold
                    font.letterSpacing: 0.8
                }
                MouseArea {
                    id: alphaMa
                    anchors.fill: parent
                    onClicked: qtobj.load_all_albums_alpha()
                }
            }
        }

        // Empty state — gracefully tells the user when there's
        // nothing to show, with mode-aware guidance.
        Text {
            visible: root.albums.length === 0
            width: parent.width
            text: root.mode === "top"
                ? "No albums with play history yet. Switch to ALPHA to browse everything in your library."
                : "No albums in your library. Run a scan to populate the catalog."
            color: Bo.slate
            font.pixelSize: 11
            wrapMode: Text.Wrap
        }

        // ── Album card grid ───────────────────────────────────
        ScrollView {
            // Reserve space for the BottomTransportBar so the last
            // row/card isn't clipped or overlapped (bottom-cutoff fix).
            bottomPadding: Bo.bottomChromeHeight
            width: parent.width
            height: parent.height - 60
            clip: true
            visible: root.albums.length > 0

            Flow {
                width: root.width - Bo.gapMD * 2
                spacing: Bo.gapSM

                Repeater {
                    model: root.albums
                    delegate: Rectangle {
                        // Card width: ~3 per row at mobile widths.
                        width: 220; height: 88; radius: 8
                        color: cardMa.pressed
                            ? Bo.mix(Bo.surface, "#A88090", 0.30)
                            : (modelData.total_plays > 10
                                ? Bo.mix(Bo.surface, "#A88090", 0.16)
                                : Bo.surface)
                        border.color: modelData.total_plays > 0
                            ? "#A88090" : Bo.hairline
                        border.width: 1

                        Column {
                            anchors.fill: parent
                            anchors.margins: 10
                            spacing: 2

                            // Title — single line, ellipsize on overflow.
                            Text {
                                width: parent.width
                                text: modelData.title || "(untitled)"
                                color: Bo.bone
                                font.pixelSize: 13
                                font.weight: Font.DemiBold
                                elide: Text.ElideRight
                                maximumLineCount: 1
                            }
                            // Primary artist — secondary tint, also
                            // single line.
                            Text {
                                width: parent.width
                                text: modelData.primary_artist || "—"
                                color: Bo.slate
                                font.pixelSize: 11
                                elide: Text.ElideRight
                                maximumLineCount: 1
                            }
                            // Metadata strip — year • N tracks • plays
                            Row {
                                spacing: 8
                                Text {
                                    visible: modelData.year
                                    text: modelData.year
                                    color: "#B58853"
                                    font.pixelSize: 10
                                }
                                Text {
                                    text: modelData.track_count + " track"
                                        + (modelData.track_count === 1 ? "" : "s")
                                    color: Bo.slate
                                    font.pixelSize: 10
                                }
                                Text {
                                    visible: modelData.total_plays > 0
                                    text: "▶ " + modelData.total_plays
                                    color: "#A88090"
                                    font.pixelSize: 10
                                    font.weight: Font.DemiBold
                                }
                            }
                        }

                        MouseArea {
                            id: cardMa
                            anchors.fill: parent
                            onClicked: {
                                qtobj.open_album(
                                    modelData.title,
                                    modelData.primary_artist
                                )
                                root.albumTapped(
                                    modelData.title,
                                    modelData.primary_artist
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}
