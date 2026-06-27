#include "ui/InboxWidget.hpp"
#include "ui/SnapViewerDialog.hpp"
#include "ui/video/VideoSnapViewerDialog.hpp"
#include "ui/ConversationDetailDialog.hpp"
#include "ui/ContactPickerDialog.hpp"
#include "ui/SnapFeedWidget.hpp"
#include "ui/posture/VoiceActivityTracker.hpp"
#include "ui/IncomingCallDialog.hpp"   // V32.40+: incoming-call ring
#include "ui/ContactCard.hpp"               // V32.35+: pending-request rows
#include "mailbox/snapclone_requests_ffi.h" // V32.35+: request lifecycle model
#include "menu/snapclone_menu_ffi.h"        // V32.35+: dynamic submenu actions
#include "ui/ThumbnailService.hpp"  // V32.34+: async thumb loader
#include "ui/SegmentedTabStrip.hpp" // V32.34+: mailbox filter tabs
#include "ui/SmartRibbon.hpp"       // V32.34+: bottom tabs+actions ribbon (prototype)
#include "ui/ExportReminderCard.hpp" // V32.34+: monthly export reminder card
#include "ui/SnapExporter.hpp"       // V32.34+: mediaDir() for recent-media preview
#include <QDir>
#include <QFileInfo>
#include <QSettings>                // V32.34+: smart-ribbon opt-in flag
#include "ui/Theme.hpp"
#include "ui/SwipeNavigator.hpp"   // V32.34+: unifiedNavEnabled() flag
#include "storage/StorageManager.hpp"

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <algorithm>
#include <QListWidget>
#include <QListWidgetItem>
#include <QPointer>
#include <QPixmap>      // V32.32: thumbnail loading for conversation rows
#include <QPainter>     // V32.32: center-crop thumbnails to square
#include <QIcon>        // V32.32: setIcon on list items
#include <QPushButton>
#include <QLabel>
#include <QFrame>
#include <QLoggingCategory>
#include <QEvent>
#include <QMouseEvent>
#include <array>
#include <vector>
#include <QDateTime>
#include <QAbstractAnimation>
#include <QPropertyAnimation>
#include <QGraphicsOpacityEffect>
#include <QTimer>

// ─── UI/InboxWidget.cpp ───────────────────────────────────────────────────────
//
// Snap inbox — list of received snaps with open/view flow.
//
// Opening a snap:
//   Double-tap (or double-click on desktop) → SnapViewerDialog.
//   The viewer handles saving, countdown, and close.
//   snapOpened() is emitted so AppController records the view in storage
//   and starts the server-side expiry timer.
//   imageSaved() from the viewer flows up as snapSaved() for the status bar.
// ─────────────────────────────────────────────────────────────────────────────

Q_LOGGING_CATEGORY(lcInbox, "snapclone.inbox")

namespace SnapClone::UI {

InboxWidget::InboxWidget(QWidget *parent)
    : QWidget(parent)
{
    buildUi();
}

InboxWidget::~InboxWidget()
{
    // V32.35+: free the FFI-owned request mailbox. sc_reqmb_free is
    // null-tolerant, so this is safe even if buildPendingRequestsSection
    // never ran.
    sc_reqmb_free(m_requestMailbox);
    m_requestMailbox = nullptr;
}

// V32.34+: debounced rebuild. Bursts of snap arrivals (initial sync,
// rapid inbound from one peer, marking many viewed) used to trigger
// N wholesale rebuilds for N events. Each rebuild walks every
// conversation, allocates QListWidgetItems, kicks off thumbnail
// loads, and runs the QListWidget's relayout. Coalesces into one
// rebuild 60ms after the last request — fast enough to feel
// instant, slow enough to absorb the burst.
//
// Why 60ms: storage's batched-save loop flushes every ~50ms; the
// snap-arrival fanout from the network manager is similar. 60ms
// catches both bursts in one rebuild without feeling laggy when a
// human-driven action (open snap, mark viewed) triggers a single
// rebuild.
void InboxWidget::requestRebuildSoon()
{
    if (!m_rebuildTimer) {
        m_rebuildTimer = new QTimer(this);
        m_rebuildTimer->setSingleShot(true);
        m_rebuildTimer->setInterval(60);
        connect(m_rebuildTimer, &QTimer::timeout,
                this, &InboxWidget::rebuildConversationList);
    }
    m_rebuildTimer->start();
}

// ─── V32.20: conversation grouping ───────────────────────────────────────────

void InboxWidget::setSelfId(const Core::UserId &selfId)
{
    m_selfId = selfId;
}

void InboxWidget::setStorage(Storage::StorageManager *storage)
{
    m_storage = storage;
}

void InboxWidget::reloadConversationsFromStorage()
{
    // Wipe in-memory state and rebuild from storage. AppController calls
    // this after initial inbox restore to ensure historical conversations
    // (including ones with only outbound snaps, which addSnap doesn't see)
    // appear in the list.
    if (!m_storage || m_selfId.value.isEmpty()) return;
    m_conversations.clear();
    m_snaps.clear();
    // V32.34+ EFFICIENCY: this used to do one loadConversationSnaps PER
    // conversation (an N+1 that loaded every snap of every thread) just to
    // read each thread's last-snap media-type/direction and to seed the dedup
    // id-set. Replaced with a FIXED THREE queries regardless of conversation
    // count: loadConversations (summaries), loadConversationLastMeta (last
    // snap's type+direction per partner), loadSnapStubs (dedup set — id +
    // sender + recipient + viewed only). For a user with C conversations
    // averaging S snaps each, this drops the work from O(C) queries loading
    // O(C·S) full Snap structs to 3 queries loading O(C) summary rows +
    // O(C·S) lightweight stub rows — no media bytes, no struct hydration.
    // (See PATH-CONVERGENCE note below for why m_snaps must be rebuilt here.)
    //
    // PATH-CONVERGENCE: m_snaps is addSnap()'s dedup set. If reload left it
    // stale, a live arrival of a snap storage already counted would miss the
    // alreadyKnown check and double-count the unread badge. Seeding it from
    // loadSnapStubs keeps the storage-restore and live-arrival paths
    // consistent. Each stub carries id + sender + recipient + viewed — the
    // fields addSnap's dedup and the expiry path (conversationKey /
    // inbound-ness) read; the viewer re-hydrates full snaps from storage.
    int totalUnread = 0;
    const auto summaries = m_storage->loadConversations(m_selfId);
    const auto lastMeta  = m_storage->loadConversationLastMeta(m_selfId);
    const auto stubs     = m_storage->loadSnapStubs(m_selfId);

    for (const auto &s : summaries) {
        ConversationRow row;
        row.otherParty = s.otherParty;
        row.otherDisplayName = resolveDisplayName(s.otherParty);
        row.lastSnapAt = s.lastSnapAt;
        row.unreadCount = s.unreadCount;
        totalUnread += s.unreadCount;
        // Last-snap media-type + direction from the single-query map.
        const auto it = lastMeta.find(s.otherParty.value);
        if (it != lastMeta.end()) {
            row.lastMediaType  = it->mediaType;
            row.lastIsOutbound = it->isOutbound;
        }
        m_conversations.insert(s.otherParty.value, row);
    }
    // Seed the dedup set from the single-query stubs. Each stub carries id +
    // senderId + recipientId + viewed — exactly what addSnap's dedup check and
    // the expiry-bookkeeping path (conversationKey / inbound-ness) read. Media
    // is NOT carried; the viewer re-loads full snaps from storage on open.
    for (const auto &stub : stubs)
        m_snaps.insert(stub.id.value, stub);
    // Sync global unread tally + tab-bar badge. Emit even if unchanged so
    // any listeners (MainWindow's tab-bar badging) refresh deterministically
    // after the restore completes.
    m_unreadCount = totalUnread;
    emit unreadCountChanged(m_unreadCount);
    rebuildConversationList();
    refreshContextualAction();   // unread count changed → maybe show/hide "Mark all read"
}

void InboxWidget::showExportReminder(const QStringList &recentMediaPaths)
{
    // V32.34+: mount the monthly export card at the TOP of the inbox (index 0,
    // above the header) so it reads as a transient prompt. Idempotent — if a
    // card is already up, replace it. The host gates whether this is called.
    if (!m_rootLayout) return;
    hideExportReminder();
    m_exportCard = new ExportReminderCard(recentMediaPaths, this);
    connect(m_exportCard, &ExportReminderCard::exportRequested, this, [this]() {
        emit exportReminderTriggered();
        // Host runs the export; we dismiss the card so it doesn't linger.
        hideExportReminder();
    });
    connect(m_exportCard, &ExportReminderCard::dismissed, this, [this]() {
        emit exportReminderSnoozed();
        hideExportReminder();
    });
    m_rootLayout->insertWidget(0, m_exportCard);
}

void InboxWidget::hideExportReminder()
{
    if (!m_exportCard) return;
    if (m_rootLayout) m_rootLayout->removeWidget(m_exportCard);
    m_exportCard->deleteLater();
    m_exportCard = nullptr;
}

QStringList InboxWidget::recentMediaPaths(int max) const
{
    // V32.34+: read the newest files straight from the snap media folder.
    // NOTE: we deliberately do NOT use m_snaps here — since the reload
    // efficiency fix, m_snaps holds lightweight stubs with empty localPath
    // (id/sender/recipient/viewed only), so it has no paths to offer. The
    // media folder is the source of truth for "recent snap media" and gives
    // us real files to thumbnail, newest-first, for the preview strip.
    QDir media(SnapExporter::mediaDir());
    if (!media.exists()) return {};
    const auto infos = media.entryInfoList(
        QDir::Files | QDir::NoDotAndDotDot, QDir::Time);  // newest first
    QStringList paths;
    for (const QFileInfo &fi : infos) {
        if (paths.size() >= max) break;
        paths.append(fi.absoluteFilePath());
    }
    return paths;
}

QString InboxWidget::resolveDisplayName(const Core::UserId &id) const
{
    if (m_storage) {
        if (auto c = m_storage->loadContact(id)) {
            // Contact::label() returns nickname → displayName → id.value,
            // matching the convention used elsewhere in the app
            // (ContactsPage card labels, etc).
            const QString lbl = c->label();
            if (!lbl.isEmpty()) return lbl;
        }
    }
    // Fall back to "username@host" → "username" trimming.
    return id.value.contains(QLatin1Char('@'))
        ? id.value.section(QLatin1Char('@'), 0, 0)
        : id.value;
}

QString InboxWidget::updateConversationFromSnap(const Core::Snap &snap)
{
    if (m_selfId.value.isEmpty()) return {};
    const Core::UserId other = snap.conversationKey(m_selfId);
    // V32.32: extract a usable photo path if this snap is a photo.
    // Empty for videos (we don't generate frame-grabs in V32.32) or
    // for snaps with no local file (cleared under ephemeral policy).
    const auto photoPathFor = [](const Core::Snap &s) -> QString {
        if (s.mediaType != Core::MediaType::Image) return {};
        if (s.localPath.isEmpty()) return {};
        return s.localPath;
    };
    auto it = m_conversations.find(other.value);
    if (it == m_conversations.end()) {
        ConversationRow row;
        row.otherParty = other;
        row.otherDisplayName = resolveDisplayName(other);
        row.lastSnapAt = snap.sentAt;
        row.lastMediaType = snap.mediaType;
        row.lastIsOutbound = (snap.senderId.value == m_selfId.value);
        row.unreadCount = (snap.viewed || row.lastIsOutbound) ? 0 : 1;
        row.lastPhotoPath = photoPathFor(snap);
        m_conversations.insert(other.value, row);
    } else {
        // Update only if this snap is newer than what we have.
        if (!it->lastSnapAt.isValid() || snap.sentAt >= it->lastSnapAt) {
            it->lastSnapAt = snap.sentAt;
            it->lastMediaType = snap.mediaType;
            it->lastIsOutbound = (snap.senderId.value == m_selfId.value);
        }
        // V32.32: update photo path whenever a newer photo arrives,
        // independent of overall lastSnapAt — so a fresh video doesn't
        // wipe out the most recent thumbnail we have.
        const QString candidatePhoto = photoPathFor(snap);
        if (!candidatePhoto.isEmpty()) {
            it->lastPhotoPath = candidatePhoto;
        }
        if (!snap.viewed && snap.senderId.value != m_selfId.value) {
            it->unreadCount += 1;
        }
    }
    return other.value;
}

void InboxWidget::applyTabVisibility()
{
    // V32.34+: swap the list vs. the Sent detail section based on the tab.
    const bool sentTab = (m_convFilter == ConvFilter::Sent);
    if (m_outboundSection) m_outboundSection->setVisible(sentTab);
    if (m_list)            m_list->setVisible(false);          // V32.44+: replaced by the snap feed
    if (m_snapFeed)        m_snapFeed->setVisible(!sentTab);   // received surface
    if (sentTab) {
        // On the Sent tab the conversation empty-label is irrelevant; the
        // outbound section has its own header/summary. Hide the list's
        // empty-label so it doesn't show through.
        if (m_emptyLabel) m_emptyLabel->setVisible(false);
    }
    // When leaving Sent, rebuildConversationList (called by the tab handler)
    // restores the list + its empty-label correctly.
}

void InboxWidget::applyFilterIndex(int idx)
{
    // V32.34+: shared by the top SegmentedTabStrip and the bottom SmartRibbon
    // so both drive m_convFilter identically (only one is visible at a time).
    m_convFilter = (idx == 1) ? ConvFilter::Unread
                 : (idx == 2) ? ConvFilter::Sent
                              : ConvFilter::All;
    applyTabVisibility();
    if (m_convFilter != ConvFilter::Sent)
        rebuildConversationList();
    // Keep whichever strip the user DIDN'T tap in sync, and re-evaluate the
    // ribbon's contextual-action enabled states for the new tab.
    if (m_ribbon) {
        if (m_ribbon->currentIndex() != idx) m_ribbon->setCurrentIndex(idx);
        m_ribbon->refreshActions();
    }
    if (m_filterTabs && m_filterTabs->currentIndex() != idx)
        m_filterTabs->setCurrentIndex(idx);
    refreshContextualAction();   // update the contextual page action for the new filter
}

void InboxWidget::refreshContextualAction()
{
    // Surface the current filter's action as a slim page-content button —
    // "Mark all read" on Unread (when there ARE unread), "Refresh" on Sent
    // (when there are outbound rows), nothing on All. This replaces the old
    // SmartRibbon per-tab actions; the bottom nav bar stays nav-only.
    if (!m_actionRow || !m_filterAction) return;
    QString label;
    bool show = false;
    switch (m_convFilter) {
        case ConvFilter::Unread:
            label = tr("Mark all read");
            show  = (m_unreadCount > 0);
            break;
        case ConvFilter::Sent:
            label = tr("Refresh");
            show  = !m_outbound.isEmpty();
            break;
        default:
            show = false;   // All: no contextual action
            break;
    }
    if (show) m_filterAction->setText(label);
    m_actionRow->setVisible(show);
}

void InboxWidget::markAllConversationsRead()
{
    // V32.34+ ribbon action: zero every conversation's unread count and the
    // global tally, then refresh the list so bold/badges clear. The per-snap
    // `viewed` flag still flips only when a snap is actually opened — this is
    // a UI-level "dismiss unread" matching common mailbox behavior.
    if (m_unreadCount == 0) return;
    for (auto &row : m_conversations) row.unreadCount = 0;
    m_unreadCount = 0;
    emit unreadCountChanged(m_unreadCount);
    setFilterBadge(1, 0);
    if (m_ribbon) m_ribbon->refreshActions();
    rebuildConversationList();
    refreshContextualAction();   // "Mark all read" should now hide (0 unread)
}

bool InboxWidget::smartRibbonEnabled() const
{
    // V32.34+ PROTOTYPE gate. Opt-in so the bottom ribbon can be device-
    // tested without disturbing the shipped top-strip layout. Enable via
    // either the env var SC_SMART_RIBBON=1 or a QSettings flag
    // ("ui/smart_ribbon" = true), so it can be toggled from a debug build or
    // a hidden setting without a recompile.
    if (qEnvironmentVariableIntValue("SC_SMART_RIBBON") == 1) return true;
    QSettings s;
    return s.value(QStringLiteral("ui/smart_ribbon"), false).toBool();
}

void InboxWidget::setFilterBadge(int index, int count)
{
    // V32.34+: update whichever filter strip(s) exist. Centralized so badge
    // sources (unread count, in-flight outbound) don't have to know which
    // strip is active — both the top SegmentedTabStrip and the bottom
    // SmartRibbon stay in sync from one call.
    if (m_filterTabs) m_filterTabs->setBadge(index, count);
    if (m_ribbon)     m_ribbon->setBadge(index, count);
}

QString InboxWidget::subTabLabel(int index) const
{
    // V32.34+ SubTabbed: labels for the unified bar's inline sub-tab list.
    switch (index) {
        case 0: return tr("All");
        case 1: return tr("Unread");
        case 2: return tr("Sent");
        default: return QString();
    }
}

int InboxWidget::subTabBadge(int index) const
{
    // V32.34+ SubTabbed: Unread tab → unread-conversation count; Sent tab →
    // in-flight outbound count; All → none. Computed from the same in-memory
    // state the strips badge from, so the unified bar shows identical counts.
    if (index == 1) {
        int unread = 0;
        for (const auto &row : m_conversations)
            if (row.unreadCount > 0) ++unread;
        return unread;
    }
    if (index == 2) {
        int delivered = 0;
        for (const OutboundRow &r : m_outbound) if (r.delivered) ++delivered;
        return qMax(0, static_cast<int>(m_outbound.size()) - delivered);
    }
    return 0;
}

void InboxWidget::refreshSnapFeed()
{
    if (!m_snapFeed) return;

    // Incoming, unopened snaps become tiles — newest first. Sender names resolve
    // from the conversation rows, falling back to the raw id.
    QList<Core::Snap> incoming;
    incoming.reserve(m_snaps.size());
    for (auto it = m_snaps.cbegin(); it != m_snaps.cend(); ++it) {
        const Core::Snap &s = it.value();
        if (s.senderId.value == m_selfId.value) continue;  // outbound — not for the feed
        if (s.viewed) continue;                            // already opened
        incoming.append(s);
    }
    std::sort(incoming.begin(), incoming.end(),
              [](const Core::Snap &a, const Core::Snap &b) { return a.sentAt > b.sentAt; });

    QList<SnapFeedWidget::Entry> entries;
    entries.reserve(incoming.size());
    for (const Core::Snap &s : incoming) {
        QString name = s.senderId.value;
        const auto cit = m_conversations.find(s.senderId.value);
        if (cit != m_conversations.end() && !cit->otherDisplayName.isEmpty())
            name = cit->otherDisplayName;
        const bool isVideo = (s.mediaType == Core::MediaType::Video);
        // Thumbnail only for images that have a decrypted file on disk; video and
        // stub snaps (empty localPath) fall back to the sealed avatar look.
        const QString thumbPath =
            (s.mediaType == Core::MediaType::Image) ? s.localPath : QString();
        entries.append({ s.id, name, s.sentAt, isVideo, thumbPath });
    }
    m_snapFeed->setEntries(entries);
}

void InboxWidget::rebuildConversationList()
{
    m_list->clear();
    if (m_snapFeed) refreshSnapFeed();   // V32.44+: keep the incoming-snap feed in sync
    // V32.34+: while the Sent tab is active the list + empty-label are
    // hidden in favor of the outbound section. Still rebuild the list's
    // contents (so it's correct when the user switches back) but don't
    // un-hide it here — applyTabVisibility owns visibility. We populate the
    // list below; the visibility calls in this function are skipped on Sent.
    const bool sentTab = (m_convFilter == ConvFilter::Sent);
    if (m_conversations.isEmpty()) {
        setFilterBadge(1, 0);  // no unread
        if (m_emptyLabel) m_emptyLabel->setVisible(false);  // V32.44+: feed shows its own empty state
        return;
    }
    if (!sentTab) m_emptyLabel->setVisible(false);

    // Sort to a temp list by lastSnapAt DESC (newest first — standard
    // chat-app ordering, matches loadConversations() query order).
    QList<ConversationRow> sorted;
    sorted.reserve(m_conversations.size());
    for (const auto &row : m_conversations) sorted.append(row);
    std::sort(sorted.begin(), sorted.end(),
              [](const ConversationRow &a, const ConversationRow &b) {
        return a.lastSnapAt > b.lastSnapAt;
    });

    // V32.34+ mailbox filter + Unread-tab badge. Count unread conversations
    // for the badge regardless of the active filter, then skip non-matching
    // rows when building. If the Unread filter yields nothing, show the
    // empty label.
    int unreadConvs = 0;
    for (const auto &row : sorted)
        if (row.unreadCount > 0) ++unreadConvs;
    setFilterBadge(1, unreadConvs);

    int shown = 0;
    for (const auto &row : sorted) {
        if (m_convFilter == ConvFilter::Unread && row.unreadCount <= 0)
            continue;
        ++shown;
        // Single-line label: "👤  Alice  ·  📷  ·  3m ago     (2)"
        // The trailing "(N)" appears only when unreadCount > 0.
        QString glyph;
        switch (row.lastMediaType) {
        case Core::MediaType::Image:    glyph = QStringLiteral("📷"); break;
        case Core::MediaType::Video:    glyph = QStringLiteral("🎥"); break;
        case Core::MediaType::Link:     glyph = QStringLiteral("🔗"); break;
        case Core::MediaType::Location: glyph = QStringLiteral("📍"); break;
        case Core::MediaType::Text:     glyph = QStringLiteral("💬"); break;
        }
        const QString dirGlyph = row.lastIsOutbound
            ? QStringLiteral("↗") : QStringLiteral("↙");

        // Relative time. Inline here (rather than calling a shared helper)
        // because the only other helper lives in OutboxWidget.cpp and we
        // don't want to drag that include in.
        QString relTime;
        const qint64 secs = row.lastSnapAt.secsTo(QDateTime::currentDateTimeUtc());
        if (secs < 60)        relTime = tr("just now");
        else if (secs < 3600) relTime = tr("%1m ago").arg(secs / 60);
        else if (secs < 86400)relTime = tr("%1h ago").arg(secs / 3600);
        else                  relTime = tr("%1d ago").arg(secs / 86400);

        QString label = QStringLiteral("%1  %2  %3  ·  %4")
            .arg(dirGlyph, row.otherDisplayName, glyph, relTime);
        if (row.unreadCount > 0) {
            label += QStringLiteral("    (%1)").arg(row.unreadCount);
        }
        // V32.40+: a declined/timed-out incoming call leaves a missed-call mark
        // until the thread is opened. m_missedCalls lives outside the rebuildable
        // rows so it survives snap-driven refreshes.
        const bool missedCall = m_missedCalls.contains(row.otherParty.value);
        if (missedCall) {
            label += QStringLiteral("   \u00B7   \u260E ") + tr("Missed call");
        }

        auto *item = new QListWidgetItem(label, m_list);
        // UserRole now carries the OTHER-PARTY id, not a snap id. The
        // onItemDoubleClicked handler dispatches on this — see
        // onItemDoubleClicked rewrite below.
        item->setData(Qt::UserRole, row.otherParty.value);
        if (missedCall)
            item->setForeground(QColor(QLatin1String(Theme::ACCENT())));

        // V32.32: photo thumbnail icon. Loaded only if a photo path
        // exists for this conversation — videos and text/location
        // snaps fall through to the default (no icon) which leaves
        // the row text-only as before. Scaled to 56×56 to match the
        // 56px min-row-height from the stylesheet.
        //
        // V32.34+: load via ThumbnailService (async, off the GUI
        // thread). Synchronous QPixmap(path) + scale + crop on every
        // inbox refresh used to cost 20-50ms on FP5 — visible as
        // jank when the inbox was scrolled or rebuilt mid-swipe.
        // ThumbnailService does the file-read + decode + scale +
        // border draw on a 2-thread pool and delivers via QueuedConnection.
        // Cache hits return synchronously (no event-loop hop) so
        // already-loaded thumbnails appear instantly.
        if (!row.lastPhotoPath.isEmpty()) {
            // Item-identity safety: QListWidgetItem is NOT a QObject —
            // it has no destroyed-signal we can hook. If we kept a raw
            // pointer and the list rebuilt between request and result,
            // dereferencing the stale pointer would be undefined
            // behavior. Instead we look the item back up by its
            // UserRole key (otherParty.value) in the callback — if the
            // row no longer exists with that key, we drop the result.
            // The lookup is O(rows) which is fine at typical inbox
            // sizes (<100). InboxWidget (`this`) is the context, so
            // if the widget dies the callback never fires at all.
            const QString path = row.lastPhotoPath;
            const QString partyKey = row.otherParty.value;
            QPointer<QListWidget> listGuard = m_list;
            ThumbnailService::instance().request(
                path, 56, this,
                [listGuard, partyKey](const QPixmap &pm) {
                    if (!listGuard) return;          // widget gone
                    if (pm.isNull()) return;         // load failed
                    for (int i = 0; i < listGuard->count(); ++i) {
                        auto *it = listGuard->item(i);
                        if (it && it->data(Qt::UserRole).toString() == partyKey) {
                            it->setIcon(QIcon(pm));
                            return;
                        }
                    }
                });
        }

        if (row.unreadCount > 0) {
            QFont f = item->font();
            f.setBold(true);
            item->setFont(f);
            item->setForeground(QColor(QLatin1String(Theme::ACCENT())));
        }
    }
    // V32.34+: if a filter (e.g. Unread) hid every row, surface the empty
    // label so the list isn't a confusing blank. The text differs from the
    // no-conversations-at-all case handled earlier.
    if (shown == 0 && m_convFilter != ConvFilter::Sent) {
        m_emptyLabel->setText(m_convFilter == ConvFilter::Unread
            ? tr("No unread snaps")
            : tr("No snaps yet\nSend one from the Camera tab"));
        m_emptyLabel->setVisible(true);
    }
    // V32.32: make sure the list reserves enough space per row for the
    // 56×56 thumbnails. The min-height stylesheet rule already covers
    // it (line ~272 buildUi), but we explicitly set iconSize too so
    // QListWidget's default delegate sizes the icon area correctly.
    m_list->setIconSize(QSize(56, 56));
}

void InboxWidget::buildUi()
{
    auto *root = new QVBoxLayout(this);
    m_rootLayout = root;   // V32.34+: kept so showExportReminder can insert later
    // V32.30: viewport-relative outer margins. At 300% OS scaling the
    // old fixed SPACE_MD ate too much of the screen; pageSideMarginPx()
    // shrinks gracefully on small/zoomed canvases (3% of vmin + 12px
    // floor, so even a 400px-wide canvas gets 24px margin). See
    // Theme::vhPx / vwPx / vminPx docstrings for the rationale.
    root->setContentsMargins(Theme::pageSideMarginPx(),
                              Theme::pageTopMarginPx(),
                              Theme::pageSideMarginPx(),
                              Theme::pageTopMarginPx());
    root->setSpacing(Theme::SPACE_SM);

    // ── Header ────────────────────────────────────────────────────────────
    auto *header = new QHBoxLayout;

    auto *titleLabel = new QLabel(QStringLiteral("SnapClone"), this);
    titleLabel->setObjectName(QStringLiteral("titleLabel"));
    header->addWidget(titleLabel);
    header->addStretch();

    // V32.34+: the outbound status pill that used to live here is retired.
    // Outbound now has its own "Sent" filter tab (below), whose badge shows
    // the in-flight count and whose selection reveals the outbound detail.
    // m_outboundPill stays null; rebuildOutboundView no longer touches it.

    m_cameraButton = new QPushButton(QStringLiteral("📷"), this);
    m_cameraButton->setObjectName(QStringLiteral("iconBtn"));
    m_cameraButton->setToolTip(tr("Take a Snap"));
    header->addWidget(m_cameraButton);

    auto *diagButton = new QPushButton(QStringLiteral("🔍"), this);
    diagButton->setObjectName(QStringLiteral("iconBtn"));
    diagButton->setToolTip(tr("Network Diagnostics"));
    header->addWidget(diagButton);

    connect(diagButton, &QPushButton::clicked,
            this, &InboxWidget::diagnosticsRequested);
    root->addLayout(header);

    // ── Filter tabs (All / Unread / Sent) ─────────────────────────────────
    // V32.34+ mailbox: the old static "Recent snaps" label is replaced by a
    // SegmentedTabStrip (shared in-page tab pattern). All/Unread filter the
    // received-conversation list; Sent swaps the list out for the outbound
    // detail section. The Unread tab badge = unread conversations; the Sent
    // tab badge = in-flight outbound snaps.
    m_filterTabs = new SegmentedTabStrip(
        { tr("All"), tr("Unread"), tr("Sent") }, this);
    connect(m_filterTabs, &SegmentedTabStrip::tabSelected, this,
            [this](int idx) { applyFilterIndex(idx); });
    root->addWidget(m_filterTabs);
    // V32.34+ unified nav: the bottom UnifiedNavBar now renders these same
    // sub-tabs, so the top strip is redundant — hide it (kept constructed so
    // its tabSelected→applyFilterIndex wiring and persistence still work; the
    // bottom bar's segment taps drive setCurrentSubTab→applyFilterIndex). Not
    // deleted: deleting styled widgets is what tripped the QStyleSheetStyle
    // teardown crash, and hide() is enough to unify the chrome.
    if (SwipeNavigator::unifiedNavEnabled() && m_filterTabs)
        m_filterTabs->hide();

    // V32.34+ contextual page action row (replaces the SmartRibbon's per-tab
    // actions). A slim right-aligned button just below the tabs; its label and
    // visibility track the active filter via refreshContextualAction(). Lives
    // in page content so the bottom nav bar stays nav-only. Hidden by default;
    // refreshContextualAction shows it only when an action is available.
    m_actionRow = new QWidget(this);
    {
        auto *arLay = new QHBoxLayout(m_actionRow);
        arLay->setContentsMargins(Theme::pageSideMarginPx(), 0,
                                  Theme::pageSideMarginPx(), 0);
        arLay->addStretch(1);
        m_filterAction = new QPushButton(m_actionRow);
        m_filterAction->setCursor(Qt::PointingHandCursor);
        m_filterAction->setStyleSheet(QStringLiteral(
            "QPushButton { background: transparent; color:%1; border:none;"
            "  font-size:%2px; font-weight:600; padding: 4px 2px; }"
            "QPushButton:hover { color:%3; }")
            .arg(QLatin1String(Theme::ACCENT()))
            .arg(Theme::FONT_SM)
            .arg(QLatin1String(Theme::ACCENT_SOFT())));
        connect(m_filterAction, &QPushButton::clicked, this, [this]() {
            // Dispatch by the active filter — same handlers the SmartRibbon used.
            switch (m_convFilter) {
                case ConvFilter::Unread: markAllConversationsRead();        break;
                case ConvFilter::Sent:   reloadConversationsFromStorage();  break;
                default: break;
            }
            refreshContextualAction();
        });
        arLay->addWidget(m_filterAction, 0);
    }
    m_actionRow->hide();   // refreshContextualAction reveals when relevant
    root->addWidget(m_actionRow, 0);

    // ── V32.35+: pending connection-requests (unified mailbox) ───────────
    // Mounted ABOVE the outbound banner and snap list — incoming requests are
    // the most action-demanding thing in the mailbox, so they sit on top.
    buildPendingRequestsSection(root);

    // ── Outbound banner (sent snaps in-flight) ───────────────────────────
    buildOutboundSection(root);

    // ── Snap list ─────────────────────────────────────────────────────────
    m_list = new QListWidget(this);
    m_list->setAlternatingRowColors(false);
    m_list->setSelectionMode(QAbstractItemView::SingleSelection);
    // V32.32: pixel-scroll mode. Default QListWidget mode is ScrollPerItem
    // (row-index increments), which breaks SwipeNavigator's
    // vbar->setValue(value - dy) math — dy is delivered in pixels, so
    // each pixel of drag would scroll by a full row. Result: wild over-
    // scroll on first touch, looks like "scrolling is broken." Switching
    // to ScrollPerPixel makes the scrollbar accept and apply pixel-grained
    // values, matching what SwipeNavigator passes down from touch deltas.
    m_list->setVerticalScrollMode(QAbstractItemView::ScrollPerPixel);
    m_list->setHorizontalScrollMode(QAbstractItemView::ScrollPerPixel);
    // Finger-friendly row height: minimum 56px per WCAG touch target guidelines.
    // Without this, rows are ~30px and users miss-tap on mobile.
    // V32.38+: frameless + transparent so an *empty* list doesn't render as a
    // stray grey box above the "No snaps yet" message (it read as a broken
    // image). Populated rows keep their own min-height/padding.
    m_list->setFrameShape(QFrame::NoFrame);
    m_list->setStyleSheet(QStringLiteral(
        "QListWidget { background: transparent; }"
        "QListWidget::item { min-height: 56px; padding: 8px 12px; }"));
    m_list->setSpacing(1);
    m_emptyLabel = new QLabel(tr("No snaps yet\nSend one from the Camera tab"), this);
    m_emptyLabel->setAlignment(Qt::AlignCenter);
    m_emptyLabel->setObjectName(QStringLiteral("subtitleLabel"));
    m_emptyLabel->setWordWrap(true);
    m_emptyLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 15px; padding: 32px;")
        .arg(QLatin1String(Theme::TEXT_SECONDARY())));

    root->addWidget(m_list, 1);

    // V32.44+: the live incoming-snap feed sits where the conversation list was
    // and becomes the received surface; the list is retained but kept hidden.
    m_snapFeed = new SnapFeedWidget(this);
    connect(m_snapFeed, &SnapFeedWidget::snapTapped,
            this, &InboxWidget::openSnapById);
    root->addWidget(m_snapFeed, 1);
    root->addWidget(m_emptyLabel, 1);

    // ── V32.34+ PROTOTYPE: bottom smart ribbon (opt-in) ───────────────────
    // One-row bottom bar: All/Unread/Sent segments on the left, the active
    // tab's contextual actions on the right. When enabled it hides the top
    // strip and becomes the filter driver; both go through applyFilterIndex
    // so the behavior is identical. Validate thumb-reach + 380px height
    // budget on-device before folding this into the global page-nav TabBar.
    if (smartRibbonEnabled()) {
        m_ribbon = new SmartRibbon({ tr("All"), tr("Unread"), tr("Sent") }, this);
        connect(m_ribbon, &SmartRibbon::tabSelected, this,
                [this](int idx) { applyFilterIndex(idx); });
        // Unread tab → "Mark all read" (enabled only when there are unread).
        // Label-only for now: no suitable icon asset exists yet (a check/
        // refresh glyph would need to be added to assets/icons + qrc). The
        // ribbon renders icon-optional, so empty icon = text button.
        m_ribbon->setTabActions(1, {
            SmartRibbon::Action{
                QString(), tr("Mark all read"),
                [this]{ return m_unreadCount > 0; },
                [this]{ markAllConversationsRead(); } }
        });
        // Sent tab → "Refresh" (re-pull outbound/conversation state from
        // storage). A true "retry failed sends" path isn't wired through the
        // inbox yet (it lives in the controller/router); using the real
        // reload here keeps the prototype honest rather than emitting a
        // signal nothing listens to. Enabled when outbound rows are present.
        m_ribbon->setTabActions(2, {
            SmartRibbon::Action{
                QString(), tr("Refresh"),
                [this]{ return !m_outbound.isEmpty(); },
                [this]{ reloadConversationsFromStorage(); } }
        });
        root->addWidget(m_ribbon, 0);
        if (m_filterTabs) m_filterTabs->hide();   // ribbon drives now
    }

    // V32.34+: restore the last-used filter tab (persisted across launches),
    // defaulting to All. setPersistenceKey selects the saved index now that
    // m_list + m_outboundSection exist, firing the tabSelected handler above
    // which sets m_convFilter and calls applyTabVisibility — so the list vs.
    // Sent-section visibility is correct from first paint. The handler skips
    // rebuildConversationList on the Sent tab, so do an explicit rebuild here
    // afterward for the All/Unread cases (harmless if Sent: list stays
    // hidden). Wired AFTER the tabSelected connect above, as required.
    if (m_filterTabs) m_filterTabs->setPersistenceKey(
        QStringLiteral("ui/tabs/mailbox"));
    else applyTabVisibility();

    connect(m_cameraButton, &QPushButton::clicked,
            this, &InboxWidget::cameraRequested);

    // Three connection strategy for reliable cross-platform snap opening:
    //
    // 1. itemClicked  — fires on QMouseEvent press+release on an item.
    //    This is what our SwipeNavigator::forwardTap() delivers via postEvent.
    //    Works on ALL platforms including mobile where synthetic events are used.
    //
    // 2. itemActivated — fires based on QStyle::SH_ItemView_ActivateItemOnSingleClick.
    //    On some mobile styles this fires on single tap. On desktop it may require
    //    double-click. Belt-and-suspenders — catches what itemClicked misses.
    //
    // 3. itemDoubleClicked — for pure desktop double-click (no touch involved).
    //
    // All three route to onItemDoubleClicked which has a 500ms debounce guard
    // so multiple signals firing for the same tap only open the viewer once.
    connect(m_list, &QListWidget::itemClicked,
            this, &InboxWidget::onItemDoubleClicked);
    connect(m_list, &QListWidget::itemActivated,
            this, &InboxWidget::onItemDoubleClicked);
    connect(m_list, &QListWidget::itemDoubleClicked,
            this, &InboxWidget::onItemDoubleClicked);

    // Install an event filter on the viewport so we catch mouse press events
    // that arrive via SwipeNavigator::forwardTap() on mobile.
    // On press we immediately open the snap — no waiting for release.
    // This is the most reliable path on Wayland/postmarketOS where synthetic
    // events from touch forwarding may not trigger the full itemClicked path.
    m_list->viewport()->installEventFilter(this);
}

// ─── Snap management ──────────────────────────────────────────────────────────

void InboxWidget::addSnap(const Core::Snap &snap)
{
    // V32.20: defend against double-add (same snap id arriving twice).
    // The conversation aggregate increments unreadCount on each call,
    // so adding the same snap twice would inflate the badge. Detect via
    // m_snaps which is the in-memory snap dedup set.
    const bool alreadyKnown = m_snaps.contains(snap.id.value);

    // V32.35+ DIAGNOSTIC: this is THE point where an inbound snap becomes
    // visible in the UI. When debugging "snaps not coming through", this line
    // distinguishes the failure stage: if it's ABSENT for a sent snap, the snap
    // never reached the UI (delivery/router/decrypt problem upstream); if it's
    // PRESENT, the snap arrived and the issue is downstream (rebuild/render).
    // It logs the media type + path so video-vs-image and missing-file cases
    // are obvious.
    qCInfo(lcInbox) << "addSnap: id=" << snap.id.value
                    << "from=" << snap.senderId.value
                    << "media=" << static_cast<int>(snap.mediaType)
                    << (snap.hasVideo() ? "(VIDEO)" : "(image/other)")
                    << "path=" << (snap.localPath.isEmpty()
                                       ? QStringLiteral("(none)") : snap.localPath)
                    << "alreadyKnown=" << alreadyKnown;

    // Store the full snap struct so we can pass it to the viewer.
    m_snaps.insert(snap.id.value, snap);

    // V32.20: maintain conversation aggregate state.
    // If selfId isn't set yet, we cannot compute conversation grouping;
    // fall back to the legacy flat list view (one row per snap).
    if (!m_selfId.value.isEmpty()) {
        if (!alreadyKnown) {
            updateConversationFromSnap(snap);
            if (!snap.viewed && snap.senderId.value != m_selfId.value) {
                ++m_unreadCount;
                emit unreadCountChanged(m_unreadCount);
            }
        }
        // V32.34+: debounced. A snap-arrival burst (initial sync, many
        // snaps from one peer) used to fire N wholesale list rebuilds.
        requestRebuildSoon();
        return;
    }

    // ── Legacy flat-list mode (selfId not yet set) ───────────────────────
    // V32.34+: dedup here too. This path appends a row per call; without a
    // guard, a duplicate snap (same id redelivered, or a live arrival of an
    // already-loaded snap) would add a second identical row and double-count
    // unread. alreadyKnown was computed at the top from m_snaps.
    if (alreadyKnown) {
        m_emptyLabel->setVisible(m_list->count() == 0);
        return;
    }

    // Build a friendly display label.
    // sender ID is "username@ip" — show just the username part.
    const QString senderDisplay = snap.senderId.value.contains(QLatin1Char('@'))
        ? snap.senderId.value.section(QLatin1Char('@'), 0, 0)
        : snap.senderId.value;

    const QString durStr = snap.durationSecs <= 0
        ? QStringLiteral("∞")
        : QStringLiteral("%1s").arg(snap.durationSecs);
    const QString label = QStringLiteral("📩  %1  ·  %2  ·  %3")
        .arg(senderDisplay,
             snap.sentAt.toLocalTime().toString(QStringLiteral("hh:mm")),
             durStr);

    auto *item = new QListWidgetItem(label, m_list);
    item->setData(Qt::UserRole, snap.id.value);

    item->setToolTip(snap.senderId.value); // full "username@ip" on hover

    if (!snap.viewed) {
        QFont f = item->font();
        f.setBold(true);
        item->setFont(f);
        item->setForeground(QColor(QLatin1String(Theme::ACCENT())));
        ++m_unreadCount;
        emit unreadCountChanged(m_unreadCount);
    }

    m_list->scrollToBottom();
    m_emptyLabel->setVisible(m_list->count() == 0);
}


void InboxWidget::closeViewerForSnap(const Core::SnapId &id)
{
    const QString snapIdStr = id.value;
    const auto vit = m_openViewers.find(snapIdStr);
    if (vit == m_openViewers.end()) return;
    QWidget *viewer = vit.value();   // QPointer — null if already destroyed
    if (viewer) {
        qCInfo(lcInbox) << "Snap" << snapIdStr
                        << "view window expired — closing open viewer";
        // close() triggers WA_DeleteOnClose teardown + the destroyed()
        // handler that erases the m_openViewers entry, so we don't erase
        // here (doing both would be a double-remove; the destroyed handler
        // is the single owner of removal). For a video viewer this also
        // tears down the gst-launch playback process via its closeEvent.
        viewer->close();
    } else {
        // Stale auto-nulled QPointer — just drop the tracking entry.
        m_openViewers.erase(vit);
    }
}

void InboxWidget::removeSnap(const Core::SnapId &id)
{
    // V32.20: in conversation mode, recompute the affected conversation row.
    // Hold onto the snap before removing so we can find its other-party id.
    Core::UserId affectedOther;
    bool wasInbound = false;
    bool wasUnread = false;
    if (!m_selfId.value.isEmpty()) {
        const auto sit = m_snaps.find(id.value);
        if (sit != m_snaps.end()) {
            affectedOther = sit.value().conversationKey(m_selfId);
            wasInbound = (sit.value().senderId.value != m_selfId.value);
            wasUnread  = wasInbound && !sit.value().viewed;
        }
    }

    m_snaps.remove(id.value);

    if (!m_selfId.value.isEmpty()) {
        // Conversation-mode bookkeeping. We don't fully recompute the
        // affected row's lastSnapAt / lastMediaType from in-memory state
        // because m_snaps only holds inbound snaps. If the removed snap
        // was the row's "last", the row's preview will be stale until
        // reloadConversationsFromStorage() runs (which the controller
        // calls on inbox refresh). Acceptable for V1 — the row still
        // shows correctly, just with slightly-stale "last" info.
        if (wasUnread && m_unreadCount > 0) {
            --m_unreadCount;
            emit unreadCountChanged(m_unreadCount);
        }
        if (!affectedOther.value.isEmpty()) {
            auto it = m_conversations.find(affectedOther.value);
            if (it != m_conversations.end()) {
                if (wasUnread && it->unreadCount > 0) --it->unreadCount;
            }
        }
        // V32.34+: debounced (see requestRebuildSoon notes).
        requestRebuildSoon();
        return;
    }

    // ── Legacy flat-list path ─────────────────────────────────────────────
    for (int i = 0; i < m_list->count(); ++i) {
        QListWidgetItem *item = m_list->item(i);
        if (item && item->data(Qt::UserRole).toString() == id.value) {
            // If this was unread (bold), decrement count before removing
            if (item->font().bold() && m_unreadCount > 0) {
                --m_unreadCount;
                emit unreadCountChanged(m_unreadCount);
            }
            delete m_list->takeItem(i);
            m_emptyLabel->setVisible(m_list->count() == 0);
            return;
        }
    }
}

void InboxWidget::clearSnaps()
{
    m_list->clear();
    m_snaps.clear();
    m_conversations.clear();
    if (m_emptyLabel) m_emptyLabel->setVisible(true);
}

// ─── Open a snap — launch the viewer ─────────────────────────────────────────

void InboxWidget::onItemDoubleClicked(QListWidgetItem *item)
{
    if (!item) return;

    // Deduplicate: itemDoubleClicked and itemActivated can both fire for the
    // same interaction on some platforms, opening the viewer twice.
    // Also catches the user-double-tap case: the user observed a viewer
    // appearing slowly (Qt media init + sws context setup takes ~500 ms
    // on first launch) and tapping a second time before the first dialog
    // fully painted. 750 ms covers that window comfortably. The dedupe
    // by open-viewer-tracking (m_openViewers) is the real safety net;
    // this debounce just avoids the wasted work of constructing a viewer
    // that would immediately get squashed by the dedupe.
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (now - m_lastOpenMs < 750) return;
    m_lastOpenMs = now;

    const QString roleStr = item->data(Qt::UserRole).toString();

    // V32.20: in conversation mode, the UserRole is the OTHER-PARTY id,
    // not a snap id. Open the detail dialog with the full thread loaded
    // from storage. Falls through to the legacy snap-open path if we're
    // somehow in flat mode (selfId not set — startup race protection).
    if (!m_selfId.value.isEmpty()) {
        const Core::UserId other{ roleStr };
        const auto it = m_conversations.find(other.value);
        if (it == m_conversations.end()) {
            qCWarning(lcInbox) << "Conversation row tapped but not in map:"
                               << roleStr;
            return;
        }
        // Mark this conversation as read in the in-memory aggregate. The
        // per-snap `viewed` flag flips when the user actually opens a snap
        // inside the detail dialog (via snapTapped → snapOpened).
        if (it->unreadCount > 0) {
            m_unreadCount = qMax(0, m_unreadCount - it->unreadCount);
            it->unreadCount = 0;
            emit unreadCountChanged(m_unreadCount);
            // Update item appearance immediately so the bold/badge clears.
            QFont f = item->font();
            f.setBold(false);
            item->setFont(f);
            item->setForeground(QColor(QLatin1String(Theme::TEXT_SECONDARY())));
        }

        // Load + open the thread (shared with the incoming-call accept path).
        openConversationWith(other, it->otherDisplayName, /*startInCall=*/false);
        return;
    }

    // ── Legacy flat-list path (selfId not set) ────────────────────────────
    const Core::SnapId snapId{ roleStr };
    emit snapOpened(snapId);

    // Update list appearance and decrement unread count if this was unread.
    {
        QFont f = item->font();
        if (f.bold()) {
            f.setBold(false);
            item->setFont(f);
            if (m_unreadCount > 0) {
                --m_unreadCount;
                emit unreadCountChanged(m_unreadCount);
            }
        }
        item->setForeground(QColor(QLatin1String(Theme::TEXT_SECONDARY())));
    }
    openSnapById(snapId);
}

// V32.40+: route an inbound voice/floor packet to a live call, or ring if the
// thread isn't open (instead of dropping it). The dialog's bridge demuxes and
// ignores any non-voice payload, so forwarding is always safe.
void InboxWidget::onVoicePayloadFromPeer(const Core::UserId &sender,
                                         const QByteArray &payload)
{
    // Posture: any inbound voice marks this conversation voice-active, which is
    // what arms the proximity screensaver in the inbox / conversation views.
    SnapClone::UI::VoiceActivityTracker::instance().noteVoice(sender.value);

    // ~6s of 20 ms frames — caps the ring buffer so an ignored ring can't grow
    // without bound while packets keep arriving.
    constexpr int kRingBufferMax = 300;

    // 1) Open dialog whose call includes this peer → straight through.
    //    isCallPeer() covers the 1:1 other party AND any added group member, so
    //    inbound voice from any participant reaches the live call.
    if (m_openConvDialog && m_openConvDialog->isCallPeer(sender)) {
        m_openConvDialog->deliverNetworkPayload(sender, payload);
        return;
    }
    // 2) Already ringing for this peer → buffer until they accept (bounded).
    if (m_ringDialog && m_ringPeer.value == sender.value) {
        if (m_ringBuffer.size() < kRingBufferMax) m_ringBuffer.append(payload);
        return;
    }
    // 3) A ring is already up for someone else → one call at a time; drop.
    if (m_ringDialog) return;
    // 4) Fresh inbound voice for a thread that isn't open → ring.
    m_ringPeer = sender;
    m_ringBuffer.clear();
    m_ringBuffer.append(payload);
    startIncomingCallRing(sender);
}

void InboxWidget::applyVoiceGated(bool gated)
{
    // Forward the posture engine's gated request to the open conversation; it
    // only acts if that conversation is actually in a call.
    if (m_openConvDialog)
        m_openConvDialog->setCallGated(gated);
}

void InboxWidget::startCallWith(const Core::UserId &peer, const QString &displayName)
{
    // Reuses the same tracked-dialog path as tapping a conversation row, but
    // opens it already in-call. openConversationWith no-ops (returns null) if the
    // self id isn't set yet; startVoiceCall() inside no-ops if voice isn't built.
    openConversationWith(peer, displayName, /*startInCall=*/true);
}

void InboxWidget::addToActiveCall(const Core::UserId &peer, const QString &displayName)
{
    // Escalate the live call to a group by adding a participant. The bridge fans
    // our outbound voice to them immediately and their side rings on the first
    // frame via the normal incoming-call path. If there's no active call to add
    // to, fall back to simply starting a call with this person.
    if (m_openConvDialog && m_openConvDialog->isCallActive())
        m_openConvDialog->addCallParticipant(peer, displayName);
    else
        startCallWith(peer, displayName);
}

void InboxWidget::startGroupVoiceCall(const QList<Core::UserId> &peers)
{
    if (peers.isEmpty()) return;

    auto nameFor = [this](const Core::UserId &id) -> QString {
        const auto it = m_conversations.find(id.value);
        if (it != m_conversations.end() && !it->otherDisplayName.isEmpty())
            return it->otherDisplayName;
        return id.value;
    };

    // Open the call with the first person, then escalate to the rest. Each
    // addToActiveCall adds a participant to the now-active call; the bridge fans
    // our voice to them and their side rings on the first frame.
    startCallWith(peers.first(), nameFor(peers.first()));
    for (int i = 1; i < peers.size(); ++i)
        addToActiveCall(peers[i], nameFor(peers[i]));
}

ConversationDetailDialog *InboxWidget::openConversationWith(const Core::UserId &other,
                                                            const QString &displayName,
                                                            bool startInCall)
{
    // Opening the thread clears any pending missed-call mark for this peer.
    const bool clearedMissed = m_missedCalls.remove(other.value);

    // Load the thread. Prefer storage (both directions, oldest-first); fall back
    // to scanning m_snaps in case storage isn't wired yet.
    QList<Core::Snap> thread;
    if (m_storage) {
        thread = m_storage->loadConversationSnaps(m_selfId, other);
    } else {
        for (const auto &s : m_snaps)
            if (s.conversationKey(m_selfId).value == other.value)
                thread.append(s);
        std::sort(thread.begin(), thread.end(),
                  [](const Core::Snap &a, const Core::Snap &b) { return a.sentAt < b.sentAt; });
    }

    auto *dlg = new ConversationDetailDialog(m_selfId, other, displayName, thread, window());
    dlg->setAttribute(Qt::WA_DeleteOnClose);
    // Forward snap taps to the shared open helper so the dedup map + AppController
    // snapOpened signal keep working.
    connect(dlg, &ConversationDetailDialog::snapTapped,
            this, [this](const Core::SnapId &snapId) { openSnapById(snapId); });
    // Track for inbound voice routing + re-emit outbound packets to the host
    // (→ NetworkManager::sendToPeer). QPointer auto-clears on dialog destruction.
    m_openConvDialog = dlg;
    m_openConvPeer   = other;
    connect(dlg, &ConversationDetailDialog::sendVoicePayload,
            this, &InboxWidget::voicePayloadToSend);
    connect(dlg, &ConversationDetailDialog::sendSnapRequested,
            this, &InboxWidget::sendSnapRequested);   // relay ephemeral text up to AppController
    // V32.43+: bottom-left "+" inside the call → open the people picker here
    // (the dialog holds no contact list) and add each pick to THIS call. We add
    // to the current dialog directly rather than via addToActiveCall, whose
    // no-active-call branch would open a second, nested dialog.
    connect(dlg, &ConversationDetailDialog::addPeopleRequested, this,
            [this, dlgPtr = QPointer<ConversationDetailDialog>(dlg)]() {
        if (!dlgPtr || !m_storage) return;
        const auto contacts = m_storage->loadContacts(m_selfId);
        if (contacts.isEmpty()) return;
        auto *picker = new ContactPickerDialog(contacts, window());
        picker->setAttribute(Qt::WA_DeleteOnClose);
        picker->setMultiSelect(true, tr("Add"));
        picker->setHeaderTitle(tr("Add people to the call"));
        connect(picker, &ContactPickerDialog::contactsConfirmed, this,
                [this, dlgPtr](const QList<Core::UserId> &ids) {
            if (!dlgPtr) return;
            auto nameFor = [this](const Core::UserId &id) -> QString {
                const auto it = m_conversations.find(id.value);
                if (it != m_conversations.end() && !it->otherDisplayName.isEmpty())
                    return it->otherDisplayName;
                return id.value;
            };
            if (!dlgPtr->isCallActive()) dlgPtr->startVoiceCall();
            for (const auto &id : ids)
                dlgPtr->addCallParticipant(id, nameFor(id));
        });
        picker->open();
    });
    dlg->open();
    if (startInCall) dlg->startVoiceCall();
    if (clearedMissed) rebuildConversationList();   // drop the missed-call mark
    return dlg;
}

// V32.40+: friendly caller name from the conversation row, falling back to the id.
void InboxWidget::startIncomingCallRing(const Core::UserId &caller)
{
    QString name = caller.value;
    const auto it = m_conversations.find(caller.value);
    if (it != m_conversations.end() && !it->otherDisplayName.isEmpty())
        name = it->otherDisplayName;

    auto *ring = new IncomingCallDialog(name, window());
    ring->setAttribute(Qt::WA_DeleteOnClose);
    m_ringDialog = ring;   // QPointer auto-nulls on close
    connect(ring, &QDialog::accepted, this, &InboxWidget::acceptIncomingCall);
    connect(ring, &QDialog::rejected, this, &InboxWidget::declineIncomingCall);
    ring->open();
}

void InboxWidget::acceptIncomingCall()
{
    const Core::UserId caller = m_ringPeer;
    QString name = caller.value;
    const auto it = m_conversations.find(caller.value);
    if (it != m_conversations.end() && !it->otherDisplayName.isEmpty())
        name = it->otherDisplayName;

    // Open the thread already in-call, then hand everything buffered while the
    // ring was up to the receiver as ORDERED PRE-ROLL. Previously this dumped
    // each frame straight into the live jitter in a tight loop — but the jitter
    // is sized for live audio and drops the oldest beyond ~1s, so the head of
    // the message (the first 1-2s) was discarded. Pre-roll plays it in order,
    // ahead of live, before the jitter sees it. The call is started inside
    // openConversationWith before this hand-off.
    if (auto *dlg = openConversationWith(caller, name, /*startInCall=*/true)) {
        dlg->primeVoicePreroll(caller, m_ringBuffer);
    }
    m_ringBuffer.clear();
    m_ringPeer = Core::UserId{};
}

void InboxWidget::declineIncomingCall()
{
    // Declined or timed out → leave a missed-call mark on the conversation row
    // (cleared when the user next opens the thread).
    if (!m_ringPeer.value.isEmpty()) {
        m_missedCalls.insert(m_ringPeer.value);
        rebuildConversationList();
    }
    m_ringBuffer.clear();
    m_ringPeer = Core::UserId{};
}

// V32.20: extracted from the legacy onItemDoubleClicked path. Opens the
// appropriate viewer (image or video) for a snap, with the same dedup
// rules and tracking-map bookkeeping as before. Called from both the
// flat-list path and the conversation-detail snapTapped relay.
void InboxWidget::openSnapById(const Core::SnapId &snapId)
{
    const QString snapIdStr = snapId.value;

    // Find the full snap struct. It is always in m_snaps (addSnap stores it),
    // but encryptedBlob may be empty for DB-restored snaps — that is fine;
    // SnapViewerDialog falls back to localPath automatically.
    auto it = m_snaps.find(snapIdStr);
    if (it == m_snaps.end()) {
        // V32.20: in conversation mode, the thread snap was loaded directly
        // from storage and may not be in m_snaps. Try to load it on demand.
        if (m_storage) {
            if (auto loaded = m_storage->loadSnap(snapId)) {
                m_snaps.insert(snapIdStr, *loaded);
                it = m_snaps.find(snapIdStr);
            }
        }
        if (it == m_snaps.end()) {
            qCWarning(lcInbox) << "Snap not found in storage:" << snapIdStr;
            return;
        }
    }

    // Tell AppController this snap is being opened (mark viewed, start expiry).
    emit snapOpened(snapId);

    // Dedupe: if a viewer for this snap is already open, raise it
    // instead of spawning a second. Qt's media backend has been observed
    // to crash when two QMediaPlayer instances race over the same file —
    // and a fast double-tap easily overruns the 750 ms debounce above.
    {
        const auto vit = m_openViewers.find(snapIdStr);
        if (vit != m_openViewers.end() && vit.value()) {
            qCInfo(lcInbox) << "Viewer already open for snap" << snapIdStr
                            << "— raising existing instead of opening a second";
            QWidget *existing = vit.value();
            existing->raise();
            existing->activateWindow();
            return;
        }
        // Stale (auto-nulled QPointer) — clean up.
        if (vit != m_openViewers.end() && !vit.value()) {
            m_openViewers.erase(vit);
        }
    }

    // Launch the viewer appropriate to the media type. Video snaps use
    // VideoSnapViewerDialog (QMediaPlayer-based, persistent + exportable);
    // image snaps use SnapViewerDialog (view-once, image only).
    if (it.value().hasVideo()) {
        auto *vviewer = new Video::VideoSnapViewerDialog(it.value(), window());
        vviewer->setAttribute(Qt::WA_DeleteOnClose);
        m_openViewers.insert(snapIdStr, QPointer<QWidget>(vviewer));
        connect(vviewer, &Video::VideoSnapViewerDialog::videoExported,
                this, [this](const QString &path) {
                    emit snapSaved(path);
                });
        // V32.42+: saving a video snap keeps it (exempt from auto-expiry).
        connect(vviewer, &Video::VideoSnapViewerDialog::snapKept,
                this, [this](const Core::SnapId &id) {
                    if (m_storage) m_storage->markSnapKept(id);
                });
        // Clean up tracking entry when the viewer closes. Using
        // QObject::destroyed is safer than the closeEvent route — it fires
        // even if the dialog is destroyed for non-close reasons (parent
        // teardown during shutdown, e.g.).
        connect(vviewer, &QObject::destroyed,
                this, [this, snapIdStr]() {
                    m_openViewers.remove(snapIdStr);
                });
        vviewer->open();
        return;
    }

    // Image-snap path
    auto *viewer = new SnapViewerDialog(it.value(), window());
    viewer->setAttribute(Qt::WA_DeleteOnClose);
    m_openViewers.insert(snapIdStr, QPointer<QWidget>(viewer));

    connect(viewer, &SnapViewerDialog::imageSaved,
            this, [this](const QString &path) {
                emit snapSaved(path);
            });
    // V32.42+: saving an image snap keeps it (exempt from auto-expiry).
    connect(viewer, &SnapViewerDialog::snapKept,
            this, [this](const Core::SnapId &id) {
                if (m_storage) m_storage->markSnapKept(id);
            });

    // Free the in-memory media bytes when the viewer closes.
    // The snap metadata (ID, sender, timestamps) stays in m_snaps for the list,
    // but the potentially-large blob is no longer needed after viewing.
    connect(viewer, &QObject::destroyed,
            this, [this, snapIdStr]() {
                // Re-look up by key — don't use a captured iterator, which
                // is invalidated if addSnap() modifies the QHash while the viewer is open.
                const auto mapIt = m_snaps.find(snapIdStr);
                if (mapIt != m_snaps.cend()) {
                    mapIt->encryptedBlob.clear();
                    mapIt->encryptedBlob.squeeze();
                }
                m_openViewers.remove(snapIdStr);
            });

    // V32.45+: feed player — let the viewer scrub the ordered incoming
    // image/text feed with ◀/▶ rather than only showing the tapped snap.
    // (Video snaps have their own viewer and are excluded here.)
    {
        QList<Core::Snap> feed;
        for (const auto &s : m_snaps) {
            if (s.senderId.value == m_selfId.value) continue;        // outbound
            if (s.viewed && s.id.value != snapIdStr) continue;       // already opened
            if (s.mediaType != Core::MediaType::Image &&
                s.mediaType != Core::MediaType::Text) continue;
            feed.append(s);
        }
        std::sort(feed.begin(), feed.end(),
                  [](const Core::Snap &a, const Core::Snap &b) { return a.sentAt > b.sentAt; });
        int startIdx = -1;
        for (int k = 0; k < feed.size(); ++k)
            if (feed[k].id.value == snapIdStr) { startIdx = k; break; }
        if (startIdx >= 0 && feed.size() > 1) {
            connect(viewer, &SnapViewerDialog::snapViewed,
                    this, [this](const Core::SnapId &id) { emit snapOpened(id); });
            viewer->setPlaylist(feed, startIdx);
        }
    }

    viewer->open();
}

bool InboxWidget::eventFilter(QObject *watched, QEvent *event)
{
    // Catch mouse press on the list viewport.
    // This fires when SwipeNavigator::forwardTap() posts a synthetic press,
    // giving us a reliable open path that doesn't depend on itemActivated
    // or itemClicked firing correctly after event synthesis on Wayland/mobile.
    // Group row tap → toggle expand/collapse
    if (watched == m_groupRow && event->type() == QEvent::MouseButtonRelease) {
        m_expanded = !m_expanded;
        rebuildOutboundView();
        return true;
    }
    if (watched == m_list->viewport()
        && event->type() == QEvent::MouseButtonPress) {

        const auto *me = static_cast<QMouseEvent *>(event);
        QListWidgetItem *item = m_list->itemAt(me->pos());
        if (item) {
            onItemDoubleClicked(item);
            return false; // let QListWidget also process it (updates selection)
        }
    }
    return QWidget::eventFilter(watched, event);
}

// ─── Outbound snap banner ─────────────────────────────────────────────────────
//
// Shows in-flight sent snaps above the inbox list.
//
// Sizing rules:
//   < 5 snaps : individual cards, each with recipient + status + time.
//   ≥ 5 snaps : collapsed to a single "N sent · M delivered" summary row.
//               Tapping that row expands to show individual cards.
//
// Lifetime rules:
//   Delivered snaps : show ✓ for kDeliveredClearMs (4s) then vanish.
//   In-flight snaps : auto-clear after kExpireSecs (60s) regardless.
//   Both rules fire the same removeOutboundRow() path.

// ─── V32.35+: pending connection-request section (unified mailbox) ──────────
//
// Composes the two tested Rust models behind their C FFIs with ZERO fabricated
// state:
//   • snapclone-mailbox::requests (RequestMailbox) — the data: which sender is
//     pending/accepted/rejected/blocked, and the legal transitions.
//   • snapclone-menu — the presentation: given a MAILBOX entity in the PENDING
//     state, which actions (accept/reject/block) to show.
// Each pending request renders as a ContactCard (reusing its animated
// expand-to-actions submenu); the submenu buttons are populated from
// sc_menu_actions_for and dispatched to sc_reqmb_apply by their stable tag.

void InboxWidget::buildPendingRequestsSection(QVBoxLayout *root)
{
    // Create the FFI-owned request model once. Freed in the destructor.
    if (!m_requestMailbox)
        m_requestMailbox = sc_reqmb_new();

    m_pendingSection = new QWidget(this);
    m_pendingSection->hide();   // revealed by rebuildPendingRequests when non-empty

    auto *col = new QVBoxLayout(m_pendingSection);
    col->setContentsMargins(0, 0, 0, Theme::SPACE_XS);
    col->setSpacing(Theme::SPACE_XS);

    m_pendingHeader = new QLabel(tr("Requests"), m_pendingSection);
    m_pendingHeader->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; font-weight: 700; letter-spacing: 0.5px;")
        .arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_SM));
    col->addWidget(m_pendingHeader);

    // Container the cards are rebuilt into.
    auto *cards = new QWidget(m_pendingSection);
    m_pendingLayout = new QVBoxLayout(cards);
    m_pendingLayout->setContentsMargins(0, 0, 0, 0);
    m_pendingLayout->setSpacing(Theme::SPACE_XS);
    col->addWidget(cards);

    root->addWidget(m_pendingSection, 0);
}

void InboxWidget::receiveConnectionRequest(const QString &senderId,
                                           const QString &name)
{
    if (!m_requestMailbox || senderId.isEmpty())
        return;
    const QByteArray sid  = senderId.toUtf8();
    const QByteArray disp = (name.isEmpty() ? senderId : name).toUtf8();
    sc_reqmb_receive(m_requestMailbox, sid.constData(), disp.constData(),
                     QDateTime::currentMSecsSinceEpoch());
    rebuildPendingRequests();
}

int InboxWidget::pendingRequestCount() const
{
    return m_requestMailbox ? sc_reqmb_pending_count(m_requestMailbox) : 0;
}

void InboxWidget::applyRequestAction(const QString &senderId, int action)
{
    if (!m_requestMailbox || senderId.isEmpty())
        return;
    const QByteArray sid = senderId.toUtf8();
    const int32_t outcome =
        sc_reqmb_apply(m_requestMailbox, sid.constData(), action);

    // Only signal the host when the model actually transitioned (not on a
    // refused/no-op), so a blocked-then-accept (refused) doesn't fire a connect.
    if (outcome >= SC_REQ_OUTCOME_CHANGED_BASE) {
        const int newState = outcome - SC_REQ_OUTCOME_CHANGED_BASE;
        switch (newState) {
            case SC_REQ_STATE_ACCEPTED: emit connectionRequestAccepted(senderId); break;
            case SC_REQ_STATE_REJECTED: emit connectionRequestRejected(senderId); break;
            case SC_REQ_STATE_BLOCKED:  emit senderBlocked(senderId);             break;
            default: break;
        }
    }
    rebuildPendingRequests();
}

void InboxWidget::rebuildPendingRequests()
{
    if (!m_pendingLayout || !m_requestMailbox)
        return;

    // Clear existing cards.
    while (QLayoutItem *it = m_pendingLayout->takeAt(0)) {
        if (QWidget *w = it->widget())
            w->deleteLater();
        delete it;
    }

    const int count = sc_reqmb_pending_count(m_requestMailbox);
    if (count <= 0) {
        m_pendingSection->hide();
        return;
    }

    // Pull the pending rows: caller-owned buffers, one pair per row.
    constexpr int kCap = 32;          // plenty for a mailbox view
    const int n = qMin(count, kCap);
    std::vector<std::array<char, 128>> idBuf(n), nmBuf(n);
    std::vector<ScPendingRow> rows(n);
    for (int i = 0; i < n; ++i) {
        rows[i].sender_id        = idBuf[i].data();
        rows[i].sender_id_cap    = static_cast<int32_t>(idBuf[i].size());
        rows[i].display_name     = nmBuf[i].data();
        rows[i].display_name_cap = static_cast<int32_t>(nmBuf[i].size());
        rows[i].received_ms      = 0;
    }
    const int got = sc_reqmb_pending(m_requestMailbox, rows.data(), n);

    // The menu model's actions for a PENDING mailbox entry — fetched once;
    // identical for every pending row.
    ScMenuAction acts[8];
    const int na = sc_menu_actions_for(SC_MENU_KIND_MAILBOX,
                                       SC_MENU_STATE_PENDING,
                                       /*edit_inline=*/0, acts, 8);

    for (int i = 0; i < got; ++i) {
        const QString senderId = QString::fromUtf8(rows[i].sender_id);
        const QString display  = QString::fromUtf8(rows[i].display_name);

        auto *card = new ContactCard(senderId, m_pendingSection);
        // Header: a simple labelled row (avatar/name). Kept minimal — the
        // emphasis is the action submenu.
        auto *header = new QWidget(card);
        auto *hl = new QHBoxLayout(header);
        hl->setContentsMargins(Theme::SPACE_SM, 6, Theme::SPACE_SM, 6);
        auto *nameLabel = new QLabel(
            QStringLiteral("👋  %1").arg(display.isEmpty() ? senderId : display),
            header);
        nameLabel->setStyleSheet(QStringLiteral("color: %1; font-size: %2px;")
            .arg(Theme::TEXT_PRIMARY()).arg(Theme::FONT_BASE));
        hl->addWidget(nameLabel, 1);
        auto *chev = new QLabel(QStringLiteral("›"), header);
        chev->setStyleSheet(QStringLiteral("color: %1; font-size: 16px;")
            .arg(Theme::TEXT_DISABLED()));
        hl->addWidget(chev);
        card->setHeader(header);

        // Submenu actions, driven by the menu model. Dispatch each tag to the
        // request model. The tags are the menu FFI's stable dispatch ids.
        for (int a = 0; a < na; ++a) {
            const QString tag   = QString::fromUtf8(acts[a].tag);
            const QString label = QString::fromUtf8(acts[a].label);
            const QString icon  = QString::fromUtf8(acts[a].icon ? acts[a].icon : "");
            const bool danger   = acts[a].danger != 0;

            int reqAction = -1;
            if (tag == QLatin1String("accept_request")) reqAction = SC_REQ_ACTION_ACCEPT;
            else if (tag == QLatin1String("reject_request")) reqAction = SC_REQ_ACTION_REJECT;
            else if (tag == QLatin1String("block_sender"))   reqAction = SC_REQ_ACTION_BLOCK;
            if (reqAction < 0)
                continue;   // unknown tag for this surface — skip defensively

            card->addAction(label, [this, senderId, reqAction]() {
                applyRequestAction(senderId, reqAction);
            }, icon, danger);
        }

        m_pendingLayout->addWidget(card);
    }

    m_pendingSection->setVisible(got > 0);
}

// ─── Outbound banner section ───────────────────────────────────────────────
//
//   ≥ 5 snaps : collapsed to a single "N sent · M delivered" summary row.
//               Tapping that row expands to show individual cards.
//
// Lifetime rules:
//   Delivered snaps : show ✓ for kDeliveredClearMs (4s) then vanish.
//   In-flight snaps : auto-clear after kExpireSecs (60s) regardless.
//   Both rules fire the same removeOutboundRow() path.

void InboxWidget::buildOutboundSection(QVBoxLayout *root)
{
    m_outboundSection = new QWidget(this);
    m_outboundSection->hide();

    auto *col = new QVBoxLayout(m_outboundSection);
    col->setContentsMargins(0, 0, 0, Theme::SPACE_XS);
    col->setSpacing(Theme::SPACE_XS);

    // Header: "Sent" label + summary pill
    auto *headerRow = new QHBoxLayout;
    auto *label = new QLabel(tr("Sent"), m_outboundSection);
    label->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; font-weight: 700; letter-spacing: 0.5px;")
        .arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_SM));
    headerRow->addWidget(label);
    headerRow->addStretch();
    m_outboundSummary = new QLabel(m_outboundSection);
    m_outboundSummary->setStyleSheet(QStringLiteral(
        "background: %1; color: %2; border-radius: %3px;"
        " padding: 2px 8px; font-size: %4px; font-weight: 600;")
        .arg(Theme::BG_RAISED()).arg(Theme::TEXT_SECONDARY())
        .arg(Theme::RADIUS_FULL).arg(Theme::FONT_SM));
    m_outboundSummary->hide();
    headerRow->addWidget(m_outboundSummary);
    col->addLayout(headerRow);

    // Collapsed group row (shown when ≥ kGroupThreshold cards)
    m_groupRow = new QWidget(m_outboundSection);
    m_groupRow->hide();
    m_groupRow->setCursor(Qt::PointingHandCursor);
    m_groupRow->setStyleSheet(QStringLiteral(
        "QWidget { background: %1; border-radius: %2px; }"
        "QWidget:hover { background: %3; }")
        .arg(Theme::BG_CARD()).arg(Theme::RADIUS_SM).arg(Theme::BG_HOVER()));
    auto *groupRowLayout = new QHBoxLayout(m_groupRow);
    groupRowLayout->setContentsMargins(Theme::SPACE_SM, 6, Theme::SPACE_SM, 6);
    m_groupLabel = new QLabel(m_groupRow);
    m_groupLabel->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px;").arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_SM));
    groupRowLayout->addWidget(m_groupLabel, 1);
    auto *chevron = new QLabel(QStringLiteral("›"), m_groupRow);
    chevron->setStyleSheet(QStringLiteral(
        "color: %1; font-size: 16px;").arg(Theme::TEXT_DISABLED()));
    groupRowLayout->addWidget(chevron);
    // Toggle expand/collapse on click
    m_groupRow->installEventFilter(this);
    col->addWidget(m_groupRow);

    // Individual card container
    m_outboundLayout = new QVBoxLayout;
    m_outboundLayout->setContentsMargins(0, 0, 0, 0);
    m_outboundLayout->setSpacing(4);
    col->addLayout(m_outboundLayout);

    auto *sep = new QFrame(m_outboundSection);
    sep->setFrameShape(QFrame::HLine);
    sep->setStyleSheet(QStringLiteral("color: %1;").arg(Theme::BORDER()));
    col->addWidget(sep);

    root->addWidget(m_outboundSection);
}

void InboxWidget::scheduleExpiry(OutboundRow &r)
{
    r.expireTimer = new QTimer(this);
    r.expireTimer->setSingleShot(true);
    const QString snapId = r.id.value;
    connect(r.expireTimer, &QTimer::timeout, this, [this, snapId]() {
        removeOutboundRow(snapId);
    });
    r.expireTimer->start(kExpireSecs * 1000);
}

void InboxWidget::removeOutboundRow(const QString &snapId)
{
    for (int i = 0; i < m_outbound.size(); ++i) {
        if (m_outbound[i].id.value != snapId) continue;
        OutboundRow &r = m_outbound[i];
        if (r.expireTimer) { r.expireTimer->stop(); r.expireTimer->deleteLater(); }
        if (r.card) { m_outboundLayout->removeWidget(r.card); r.card->deleteLater(); }
        m_outbound.removeAt(i);
        break;
    }
    // V32.34+: visibility is owned by the active tab now, not by emptiness.
    // On the Sent tab the (now-empty) section stays shown — its header reads
    // "Sent" with a 0 summary, which is the correct "nothing in flight"
    // state. On other tabs it's already hidden by applyTabVisibility. So we
    // only re-sync via applyTabVisibility rather than force-hiding here.
    rebuildOutboundView();
    if (m_convFilter == ConvFilter::Sent) applyTabVisibility();
}

void InboxWidget::addOutboundSnap(const Core::SnapId  &id,
                                   const Core::UserId  &recipient,
                                   const QString       &recipientName,
                                   const QDateTime     &sentAt)
{
    for (const OutboundRow &r : m_outbound)
        if (r.id.value == id.value) return;

    auto *card = new QWidget(m_outboundSection);
    card->setStyleSheet(QStringLiteral(
        "background: %1; border-radius: %2px;")
        .arg(Theme::BG_CARD()).arg(Theme::RADIUS_SM));

    auto *row = new QHBoxLayout(card);
    row->setContentsMargins(Theme::SPACE_SM, 6, Theme::SPACE_SM, 6);
    row->setSpacing(Theme::SPACE_SM);

    const QString name = recipientName.isEmpty() ? recipient.value : recipientName;
    auto *nameLbl = new QLabel(QStringLiteral("→ %1").arg(name), card);
    nameLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px; font-weight: 600;")
        .arg(Theme::TEXT_PRIMARY()).arg(Theme::FONT_SM));
    row->addWidget(nameLbl, 1);

    auto *statusLbl = new QLabel(tr("⟳ Connecting…"), card);
    statusLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px;").arg(Theme::TEXT_SECONDARY()).arg(Theme::FONT_SM));
    row->addWidget(statusLbl);

    const QString timeStr = sentAt.toLocalTime().toString(QStringLiteral("hh:mm"));
    auto *timeLbl = new QLabel(timeStr, card);
    timeLbl->setStyleSheet(QStringLiteral(
        "color: %1; font-size: %2px;").arg(Theme::TEXT_DISABLED()).arg(Theme::FONT_XS));
    row->addWidget(timeLbl);

    m_outboundLayout->addWidget(card);

    OutboundRow r;
    r.id            = id;
    r.recipient     = recipient;
    r.recipientName = recipientName;
    r.sentAt        = sentAt;
    r.card          = card;
    r.statusLbl     = statusLbl;
    m_outbound.append(r);
    scheduleExpiry(m_outbound.last());

    // V32.34+: do NOT auto-open the detail section on send. The header pill
    // (updated by rebuildOutboundView) is the at-a-glance signal; the
    // section only opens when the user taps the pill. This keeps a fresh
    // send from shoving conversations down the page.
    rebuildOutboundView();
}

void InboxWidget::updateOutboundStatus(const Core::SnapId &id,
                                        const QString &phase,
                                        const QString &detail)
{
    for (OutboundRow &r : m_outbound) {
        if (r.id.value != id.value || r.delivered) continue;
        const auto icon = [&]() -> QString {
            if (phase == QLatin1String("connecting"))  return QStringLiteral("⟳");
            if (phase == QLatin1String("relay"))       return QStringLiteral("🔀");
            if (phase == QLatin1String("udp"))         return QStringLiteral("📡");
            if (phase == QLatin1String("punching"))    return QStringLiteral("⚡");
            if (phase == QLatin1String("mailbox"))     return QStringLiteral("📬");
            if (phase == QLatin1String("offline"))     return QStringLiteral("⏸");
            if (phase == QLatin1String("failed"))      return QStringLiteral("✗");
            return QStringLiteral("…");
        }();
        const QString text = detail.isEmpty()
            ? QStringLiteral("%1 %2").arg(icon, phase)
            : QStringLiteral("%1 %2").arg(icon, detail);
        r.statusLbl->setText(text);
        const QString col = (phase == QLatin1String("failed"))
            ? Theme::DANGER() : Theme::TEXT_SECONDARY();
        r.statusLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px;").arg(col).arg(Theme::FONT_SM));
        return;
    }
}

void InboxWidget::markOutboundDelivered(const Core::SnapId &id)
{
    for (OutboundRow &r : m_outbound) {
        if (r.id.value != id.value || r.delivered) continue;
        r.delivered = true;
        // Cancel 60s expiry — we'll clear after kDeliveredClearMs instead.
        if (r.expireTimer) { r.expireTimer->stop(); }
        r.statusLbl->setText(tr("✓"));
        r.statusLbl->setStyleSheet(QStringLiteral(
            "color: %1; font-size: %2px; font-weight: 700;")
            .arg(Theme::SUCCESS()).arg(Theme::FONT_SM));
        rebuildOutboundView();
        // Remove after kDeliveredClearMs (show ✓ briefly then clean up).
        const QString snapId = id.value;
        QTimer::singleShot(kDeliveredClearMs, this, [this, snapId]() {
            removeOutboundRow(snapId);
        });
        return;
    }
}

void InboxWidget::rebuildOutboundView()
{
    const int total = static_cast<int>(m_outbound.size());

    // V32.34+: drive the "Sent" tab badge (index 2) with the in-flight
    // count. The header pill is retired — the tab is now the outbound
    // surface. We badge the IN-FLIGHT count (not delivered) so a row of
    // ✓-delivered snaps about to be cleared doesn't keep the badge lit.
    {
        int delivered = 0;
        for (const OutboundRow &r : m_outbound) if (r.delivered) ++delivered;
        const int inFlight = total - delivered;
        setFilterBadge(2, inFlight);
        if (m_ribbon) m_ribbon->refreshActions();  // "Refresh" enable state
    }
    // If the user is currently on the Sent tab, keep the section in sync
    // (it stays shown but may empty out as deliveries clear).
    if (m_convFilter == ConvFilter::Sent)
        applyTabVisibility();

    // Update summary pill (inside the detail section, unchanged)
    if (total == 0) {
        m_outboundSummary->hide();
    } else {
        int delivered = 0;
        for (const OutboundRow &r : m_outbound) if (r.delivered) ++delivered;
        m_outboundSummary->setText(tr("%1/%2").arg(delivered).arg(total));
        const char *col = (delivered == total) ? Theme::SUCCESS()
                        : (delivered > 0)      ? Theme::WARNING()
                                               : Theme::TEXT_SECONDARY();
        m_outboundSummary->setStyleSheet(QStringLiteral(
            "background: %1; color: %2; border-radius: %3px;"
            " padding: 2px 8px; font-size: %4px; font-weight: 600;")
            .arg(Theme::BG_RAISED()).arg(col)
            .arg(Theme::RADIUS_FULL).arg(Theme::FONT_SM));
        m_outboundSummary->show();
    }

    // Grouping: collapse when ≥ kGroupThreshold individual cards
    const bool shouldGroup = (total >= kGroupThreshold);
    if (shouldGroup != m_grouped) {
        m_grouped = shouldGroup;
        if (!shouldGroup) m_expanded = false; // reset expand when below threshold
    }

    if (m_grouped && !m_expanded) {
        // Show collapsed summary row, hide individual cards
        m_groupRow->show();
        for (const OutboundRow &r : m_outbound)
            if (r.card) r.card->hide();

        int delivered = 0, inFlight = 0;
        for (const OutboundRow &r : m_outbound) {
            if (r.delivered) ++delivered; else ++inFlight;
        }
        QString groupText;
        if (inFlight > 0 && delivered > 0)
            groupText = tr("%1 sending · %2 delivered").arg(inFlight).arg(delivered);
        else if (inFlight > 0)
            groupText = tr("%1 sending…").arg(inFlight);
        else
            groupText = tr("%1 delivered").arg(delivered);
        m_groupLabel->setText(groupText);
    } else {
        // Show individual cards, hide group row
        m_groupRow->hide();
        for (const OutboundRow &r : m_outbound)
            if (r.card) r.card->show();
    }
}


} // namespace SnapClone::UI
