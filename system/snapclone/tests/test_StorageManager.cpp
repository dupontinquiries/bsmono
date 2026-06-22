// tests/test_StorageManager.cpp
//
// Unit tests for StorageManager using Qt Test.
//
// Tests run against an in-memory SQLite database (:memory:) so they are
// hermetic and leave no files on disk.

#include <QtTest>
#include "storage/StorageManager.hpp"
#include "core/Types.hpp"

using namespace SnapClone;

class TestStorageManager : public QObject
{
    Q_OBJECT

private:
    Storage::StorageManager *m_storage = nullptr;

private slots:
    void init()
    {
        // :memory: database is created fresh for every test case.
        m_storage = new Storage::StorageManager(QStringLiteral(":memory:"), this);
        QVERIFY(m_storage->open());
    }

    void cleanup()
    {
        delete m_storage;
        m_storage = nullptr;
    }

    // ── Snap tests ────────────────────────────────────────────────────────

    void saveAndLoadSnap()
    {
        Core::Snap snap;
        snap.id           = Core::SnapId::generate();
        snap.senderId     = Core::UserId{ QStringLiteral("alice") };
        snap.recipientId  = Core::UserId{ QStringLiteral("bob") };
        snap.mediaType    = Core::MediaType::Image;
        snap.sentAt       = QDateTime::currentDateTimeUtc();
        snap.durationSecs = 7;

        QVERIFY(m_storage->saveSnap(snap));

        const auto loaded = m_storage->loadSnap(snap.id);
        QVERIFY(loaded.has_value());
        QCOMPARE(loaded->id.value,           snap.id.value);
        QCOMPARE(loaded->senderId.value,     snap.senderId.value);
        QCOMPARE(loaded->recipientId.value,  snap.recipientId.value);
        QCOMPARE(loaded->durationSecs,       snap.durationSecs);
        QVERIFY(!loaded->viewed);
    }

    void markSnapViewed()
    {
        Core::Snap snap;
        snap.id          = Core::SnapId::generate();
        snap.senderId    = Core::UserId{ QStringLiteral("alice") };
        snap.recipientId = Core::UserId{ QStringLiteral("bob") };
        snap.sentAt      = QDateTime::currentDateTimeUtc();

        QVERIFY(m_storage->saveSnap(snap));
        QVERIFY(m_storage->markSnapViewed(snap.id));

        const auto loaded = m_storage->loadSnap(snap.id);
        QVERIFY(loaded.has_value());
        QVERIFY(loaded->viewed);
    }

    void loadInboxSnaps()
    {
        const Core::UserId bob{ QStringLiteral("bob") };

        for (int i = 0; i < 3; ++i) {
            Core::Snap s;
            s.id          = Core::SnapId::generate();
            s.senderId    = Core::UserId{ QStringLiteral("alice") };
            s.recipientId = bob;
            s.sentAt      = QDateTime::currentDateTimeUtc();
            QVERIFY(m_storage->saveSnap(s));
        }

        const auto inbox = m_storage->loadInboxSnaps(bob);
        QCOMPARE(inbox.size(), 3);
    }

    void purgeExpiredSnaps()
    {
        Core::Snap expired;
        expired.id          = Core::SnapId::generate();
        expired.senderId    = Core::UserId{ QStringLiteral("alice") };
        expired.recipientId = Core::UserId{ QStringLiteral("bob") };
        expired.sentAt      = QDateTime::currentDateTimeUtc().addDays(-2);
        expired.expiresAt   = QDateTime::currentDateTimeUtc().addSecs(-1); // already expired
        QVERIFY(m_storage->saveSnap(expired));

        Core::Snap live;
        live.id          = Core::SnapId::generate();
        live.senderId    = Core::UserId{ QStringLiteral("alice") };
        live.recipientId = Core::UserId{ QStringLiteral("bob") };
        live.sentAt      = QDateTime::currentDateTimeUtc();
        live.expiresAt   = QDateTime::currentDateTimeUtc().addDays(1);
        QVERIFY(m_storage->saveSnap(live));

        const int purged = m_storage->purgeExpiredSnaps();
        QCOMPARE(purged, 1);

        QVERIFY(!m_storage->loadSnap(expired.id).has_value());
        QVERIFY(m_storage->loadSnap(live.id).has_value());
    }

    // ── User tests ────────────────────────────────────────────────────────

    void saveAndLoadUser()
    {
        Core::UserProfile p;
        p.id          = Core::UserId{ QStringLiteral("alice") };
        p.displayName = QStringLiteral("Alice Wonderland");
        p.online      = true;

        QVERIFY(m_storage->saveUser(p));

        const auto loaded = m_storage->loadUser(p.id);
        QVERIFY(loaded.has_value());
        QCOMPARE(loaded->displayName, p.displayName);
        QVERIFY(loaded->online);
    }

    // ── Peer address tests ────────────────────────────────────────────────

    void saveAndLoadPeerAddress()
    {
        const Core::UserId id{ QStringLiteral("alice") };
        const Core::PeerAddress addr{ QStringLiteral("192.168.1.10"), 9876 };

        QVERIFY(m_storage->savePeerAddress(id, addr));

        const auto loaded = m_storage->loadPeerAddress(id);
        QVERIFY(loaded.has_value());
        QCOMPARE(loaded->host, addr.host);
        QCOMPARE(loaded->port, addr.port);
    }

    void loadMissingPeerAddress()
    {
        const Core::UserId id{ QStringLiteral("unknown_user") };
        const auto result = m_storage->loadPeerAddress(id);
        QVERIFY(!result.has_value());
    }
};

QTEST_MAIN(TestStorageManager)
#include "test_StorageManager.moc"
