// tests/test_CryptoService.cpp
//
// Unit tests for CryptoService: encrypt → decrypt round-trip and MAC validation.

#include <QtTest>
#include "crypto/CryptoService.hpp"
#include "core/Types.hpp"

using namespace SnapClone;

class TestCryptoService : public QObject
{
    Q_OBJECT

private slots:
    void encryptDecryptRoundTrip()
    {
        Crypto::CryptoService svc;

        const Core::UserId sender{ QStringLiteral("alice") };
        const Core::UserId recipient{ QStringLiteral("bob") };

        // setSelfUserId must be called before registerPeerPublicKey so the
        // symmetric session key derivation has access to our own public key.
        // Without it, the fallback path (hash of peer pubkey alone) is used,
        // which is non-symmetric and breaks encrypt/decrypt across two peers.
        svc.setSelfUserId(sender);
        // Register a synthetic session key directly (bypass real key exchange).
        svc.registerPeerPublicKey(recipient, QByteArray(32, '\x42'));
        svc.registerPeerPublicKey(sender,    QByteArray(32, '\x42'));

        const QByteArray plaintext = QByteArrayLiteral("Hello, SnapClone!");

        const auto encResult = svc.encryptFor(recipient, plaintext);
        QVERIFY(encResult.has_value());

        const auto decResult = svc.decryptFrom(sender, encResult->ciphertext, encResult->nonce);
        QVERIFY(decResult.has_value());
        QCOMPARE(decResult->plaintext, plaintext);
    }

    void tamperingDetected()
    {
        Crypto::CryptoService svc;
        const Core::UserId peer{ QStringLiteral("eve") };
        svc.registerPeerPublicKey(peer, QByteArray(32, '\xAB'));

        const auto encResult = svc.encryptFor(peer, QByteArrayLiteral("secret"));
        QVERIFY(encResult.has_value());

        // Flip a byte in the ciphertext.
        QByteArray tampered = encResult->ciphertext;
        tampered[0] = static_cast<char>(static_cast<unsigned char>(tampered[0]) ^ 0xFF);

        const auto decResult = svc.decryptFrom(peer, tampered, encResult->nonce);
        QVERIFY(!decResult.has_value()); // must fail MAC check
    }

    void missingSessionKeyReturnsError()
    {
        Crypto::CryptoService svc;
        const Core::UserId unknown{ QStringLiteral("unknown_peer") };

        const auto result = svc.encryptFor(unknown, QByteArrayLiteral("test"));
        QVERIFY(!result.has_value());
        QVERIFY(!result.error().isEmpty());
    }

    void differentNoncesProduceDifferentCiphertext()
    {
        Crypto::CryptoService svc;
        const Core::UserId peer{ QStringLiteral("bob") };
        svc.registerPeerPublicKey(peer, QByteArray(32, '\x55'));

        const QByteArray plaintext = QByteArrayLiteral("same message");

        const auto enc1 = svc.encryptFor(peer, plaintext);
        const auto enc2 = svc.encryptFor(peer, plaintext);

        QVERIFY(enc1.has_value());
        QVERIFY(enc2.has_value());

        // Nonces must differ (randomised per call).
        QVERIFY(enc1->nonce != enc2->nonce);

        // And therefore ciphertexts must differ.
        QVERIFY(enc1->ciphertext != enc2->ciphertext);
    }
};

QTEST_MAIN(TestCryptoService)
#include "test_CryptoService.moc"
