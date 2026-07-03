// qml/HistoryPage.qml
//
// Browse the play_history table. Most recent plays first, with the
// track title, primary artist, and when it played (relative time
// like "12m ago" / "2h ago" / "yesterday"). Tap to play again.
//
// Driven by qtobj.history_json (JSON array of HistoryEntry) which is
// refreshed via qtobj.load_recent_history(limit). Defaults to last 100.

import QtQuick 2.15
import QtQuick.Controls 2.15
import "."

Item {
    id: root
    property color accent: "#C8A04B"
    property int limit: 100

    Component.onCompleted: {
        qtobj.load_recent_history(limit)
        // 30-day window for the chart — long enough for weekly
        // patterns to emerge, short enough to feel current.
        qtobj.load_history_stats(30)
        qtobj.load_history_summary(30)
    }

    property var entries: {
        try { return JSON.parse(qtobj.history_json || "[]") }
        catch (e) { return [] }
    }
    property var hourStats: {
        try { return JSON.parse(qtobj.history_stats_json || "[]") }
        catch (e) { return [] }
    }
    property var summary: {
        try { return JSON.parse(qtobj.history_summary_json || "null") }
        catch (e) { return null }
    }
    Connections {
        target: qtobj
        function onHistory_changed() {
            try { root.entries = JSON.parse(qtobj.history_json || "[]") }
            catch (e) { root.entries = [] }
        }
        function onHistory_stats_changed() {
            try { root.hourStats = JSON.parse(qtobj.history_stats_json || "[]") }
            catch (e) { root.hourStats = [] }
        }
        function onHistory_summary_changed() {
            try { root.summary = JSON.parse(qtobj.history_summary_json || "null") }
            catch (e) { root.summary = null }
        }
    }

    // Peak count for normalizing the bar heights. We compute it
    // reactively from hourStats so the chart auto-rescales when the
    // user reloads or new plays land.
    readonly property int hourMax: {
        var m = 0
        for (var i = 0; i < hourStats.length; ++i)
            if (hourStats[i].count > m) m = hourStats[i].count
        return m
    }

    // Pretty-print "seconds ago" as a relative time. The history
    // entries carry played_at as unix seconds — convert to "now - t"
    // and pick a unit. Keeps the row compact (a fixed-width string)
    // and never goes stale in absolute terms.
    function relativeTime(unix_sec) {
        if (!unix_sec) return ""
        var now = Math.floor(Date.now() / 1000)
        var d = now - unix_sec
        if (d < 60)         return d + "s ago"
        if (d < 3600)       return Math.floor(d / 60) + "m ago"
        if (d < 86400)      return Math.floor(d / 3600) + "h ago"
        if (d < 604800)     return Math.floor(d / 86400) + "d ago"
        // Older than a week: show date.
        var dt = new Date(unix_sec * 1000)
        return Qt.formatDate(dt, "MMM d")
    }

    Column {
        anchors.fill: parent
        anchors.margins: Bo.gapMD
        spacing: Bo.gapMD

        Row {
            width: parent.width
            spacing: Bo.gapSM
            Text {
                text: "HISTORY"
                color: Bo.silver
                font.pixelSize: Bo.captionSize
                font.weight: Font.DemiBold
                font.letterSpacing: 1.2
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                text: "· " + root.entries.length + " play"
                    + (root.entries.length === 1 ? "" : "s")
                color: Bo.silver
                font.pixelSize: 10
                anchors.verticalCenter: parent.verticalCenter
            }
            Item { width: parent.width - 180; height: 1 }   // spacer

            Rectangle {
                width: 70; height: 28; radius: 14
                color: refreshMa.pressed ? Bo.surfaceHi : "transparent"
                border.color: Bo.hairline; border.width: 1
                Text { anchors.centerIn: parent; text: "↻ Refresh"
                    color: Bo.bone; font.pixelSize: 10 }
                MouseArea {
                    id: refreshMa
                    anchors.fill: parent
                    onClicked: {
                        qtobj.load_recent_history(root.limit)
                        qtobj.load_history_stats(30)
                        qtobj.load_history_summary(30)
                    }
                }
            }
        }

        // ── KPI STRIP ──────────────────────────────────────────
        //
        // Three small KPI cards summarizing the lookback window:
        //   1. Total plays + distinct tracks heard.
        //   2. Top track (with play count).
        //   3. Top primary artist (with play count).
        // Hidden when summary is null (no plays yet, or pre-load).
        // The strip uses Bo.surfaceHi tiles so it visually groups
        // with the chart and reads as a "stats block".
        Row {
            width: parent.width
            spacing: Bo.gapSM
            visible: root.summary
                  && root.summary.total_plays > 0

            // Card 1: total plays + distinct tracks.
            Rectangle {
                width: (parent.width - Bo.gapSM*2) / 3
                height: 50; radius: 6
                color: Bo.surfaceHi
                border.color: Bo.hairline; border.width: 1
                Column {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 1
                    anchors.topMargin: 6
                    Text {
                        text: "PLAYS"
                        color: Bo.silver
                        font.pixelSize: 8
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1.0
                    }
                    Row {
                        spacing: 6
                        anchors.left: parent.left
                        Text {
                            text: root.summary ? root.summary.total_plays : 0
                            color: root.accent
                            font.pixelSize: 18
                            font.weight: Font.DemiBold
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.summary
                                ? (" / " + root.summary.distinct_tracks
                                   + " track" + (root.summary.distinct_tracks === 1 ? "" : "s"))
                                : ""
                            color: Bo.silver
                            font.pixelSize: 10
                        }
                    }
                }
            }

            // Card 2: top track.
            Rectangle {
                width: (parent.width - Bo.gapSM*2) / 3
                height: 50; radius: 6
                color: Bo.surfaceHi
                border.color: Bo.hairline; border.width: 1
                Column {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    anchors.topMargin: 6
                    spacing: 1
                    Text {
                        text: root.summary && root.summary.top_track
                            ? ("TOP TRACK · " + root.summary.top_track.play_count + " plays")
                            : "TOP TRACK"
                        color: Bo.silver
                        font.pixelSize: 8
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1.0
                        elide: Text.ElideRight
                        width: parent.width
                    }
                    Text {
                        text: root.summary && root.summary.top_track
                            ? root.summary.top_track.title
                            : "—"
                        color: Bo.bone
                        font.pixelSize: Bo.bodySize - 2
                        font.weight: Font.Medium
                        elide: Text.ElideRight
                        width: parent.width
                    }
                    Text {
                        text: root.summary && root.summary.top_track
                            ? (root.summary.top_track.primary_artist || "")
                            : ""
                        color: Bo.silver
                        font.pixelSize: 9
                        elide: Text.ElideRight
                        width: parent.width
                    }
                }
                // Tap to play the top track again.
                MouseArea {
                    anchors.fill: parent
                    enabled: root.summary && root.summary.top_track
                    onClicked: {
                        if (root.summary && root.summary.top_track) {
                            qtobj.play_track(root.summary.top_track.track_id)
                        }
                    }
                }
            }

            // Card 3: top artist.
            Rectangle {
                width: (parent.width - Bo.gapSM*2) / 3
                height: 50; radius: 6
                color: Bo.surfaceHi
                border.color: Bo.hairline; border.width: 1
                Column {
                    anchors.fill: parent
                    anchors.leftMargin: Bo.gapSM
                    anchors.rightMargin: Bo.gapSM
                    anchors.topMargin: 6
                    spacing: 1
                    Text {
                        text: root.summary && root.summary.top_artist
                            ? ("TOP ARTIST · " + root.summary.top_artist.play_count + " plays")
                            : "TOP ARTIST"
                        color: Bo.silver
                        font.pixelSize: 8
                        font.weight: Font.DemiBold
                        font.letterSpacing: 1.0
                        elide: Text.ElideRight
                        width: parent.width
                    }
                    Text {
                        text: root.summary && root.summary.top_artist
                            ? root.summary.top_artist.name
                            : "—"
                        color: Bo.bone
                        font.pixelSize: Bo.bodySize - 2
                        font.weight: Font.Medium
                        elide: Text.ElideRight
                        width: parent.width
                    }
                }
            }
        }

        // ── HOURLY HISTOGRAM ────────────────────────────────────
        //
        // 24 vertical bars showing plays per hour-of-day over the
        // last 30 days. Compact (40px tall) so it fits above the
        // virtualized list without crowding. Hidden when there's
        // nothing to chart yet. The bar at the user's CURRENT hour
        // gets the accent color so the chart doubles as a "when are
        // you most likely to be listening" check.
        Item {
            width: parent.width
            height: visible ? 60 : 0
            visible: root.hourStats.length > 0 && root.hourMax > 0

            Text {
                id: chartLabel
                anchors.left: parent.left
                anchors.top: parent.top
                text: "PLAYS / HOUR (LAST 30 DAYS)"
                color: Bo.silver
                font.pixelSize: 9
                font.weight: Font.DemiBold
                font.letterSpacing: 1.0
            }

            Row {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: chartLabel.bottom
                anchors.topMargin: 4
                anchors.bottom: parent.bottom
                spacing: 1

                Repeater {
                    model: root.hourStats
                    delegate: Item {
                        width: (parent.width - 23) / 24    // 24 bars + 23 gaps
                        height: parent.height

                        readonly property bool isNowHour:
                            modelData.hour === new Date().getHours()
                        readonly property real ratio:
                            root.hourMax > 0
                            ? Math.min(1.0, modelData.count / root.hourMax)
                            : 0

                        // The bar itself, anchored to the bottom.
                        Rectangle {
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 12   // leave room for hour label
                            anchors.horizontalCenter: parent.horizontalCenter
                            width: parent.width
                            height: Math.max(1, (parent.height - 14) * parent.ratio)
                            radius: 1
                            color: parent.isNowHour ? root.accent
                                                    : Bo.mix(Bo.surface, root.accent, 0.35)
                            Behavior on height { NumberAnimation { duration: Bo.base } }
                        }

                        // Hour label (every 4 hours, kept sparse to avoid clutter).
                        Text {
                            anchors.bottom: parent.bottom
                            anchors.horizontalCenter: parent.horizontalCenter
                            visible: modelData.hour % 4 === 0
                            text: ("00" + modelData.hour).slice(-2)
                            color: parent.isNowHour ? root.accent : Bo.slate
                            font.pixelSize: 8
                            font.weight: parent.isNowHour ? Font.DemiBold : Font.Normal
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
            width: parent.width
            // Header strip ≈ 40, chart ≈ 60 when visible, KPI strip
            // ≈ 50 when visible. The 0 case (no plays yet) collapses
            // to header-only at ≈ 50px.
            height: parent.height
                - 50
                - (root.summary && root.summary.total_plays > 0 ? 50 + Bo.gapMD : 0)
                - (root.hourMax > 0 ? 60 + Bo.gapMD : 0)
            spacing: 4
            clip: true
            model: root.entries
            cacheBuffer: 240
            ScrollIndicator.vertical: ScrollIndicator {}

            Text {
                anchors.centerIn: parent
                visible: parent.count === 0
                text: "No play history yet — start a track and it'll show up here"
                color: Bo.slate
                font.pixelSize: Bo.captionSize
                wrapMode: Text.Wrap
                width: parent.width * 0.8
                horizontalAlignment: Text.AlignHCenter
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

                    // Tiny clock badge that warms up with recency.
                    // Within 5 minutes: accent color. Older: silver.
                    Rectangle {
                        width: 8; height: 8; radius: 4
                        anchors.verticalCenter: parent.verticalCenter
                        color: {
                            var now = Math.floor(Date.now() / 1000)
                            var age = now - (modelData.played_at || 0)
                            if (age < 300) return root.accent
                            if (age < 3600) return Bo.silver
                            return Bo.slate
                        }
                    }

                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        width: parent.width - 8 - 60 - Bo.gapSM*2
                        spacing: 1
                        Text {
                            text: modelData.title || "(untitled)"
                            color: Bo.bone
                            font.pixelSize: Bo.bodySize - 1
                            elide: Text.ElideRight
                            width: parent.width
                        }
                        Text {
                            visible: modelData.primary_artist
                                  && modelData.primary_artist.length > 0
                            text: modelData.primary_artist || ""
                            color: Bo.silver
                            font.pixelSize: 9
                            elide: Text.ElideRight
                            width: parent.width
                        }
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: relativeTime(modelData.played_at)
                        color: Bo.silver
                        font.pixelSize: 10
                        width: 60
                        horizontalAlignment: Text.AlignRight
                    }
                }

                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    onClicked: qtobj.enqueue_or_play(modelData.track_id)
                }
            }
        }
    }
}
