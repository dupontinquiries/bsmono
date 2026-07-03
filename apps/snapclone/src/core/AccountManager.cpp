// ─── AccountManager.cpp ──────────────────────────────────────────────────────
#include "core/AccountManager.hpp"

#include <QStandardPaths>
#include <QDir>
#include <QFile>
#include <QRandomGenerator>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>
#include <QDateTime>
#include <QLoggingCategory>

#include <algorithm>

Q_LOGGING_CATEGORY(lcAccounts, "snapclone.accounts")

namespace SnapClone::Core {

namespace {
// Same sanitisation the crypto layer uses for key filenames, so we can find and
// delete an account's keys by id.
QString sanitizeForKeyFile(const QString &id)
{
    QString safe;
    safe.reserve(id.size());
    for (const QChar c : id) {
        if (c.isLetterOrNumber() || c == QLatin1Char('@')
         || c == QLatin1Char('-') || c == QLatin1Char('.')) {
            safe.append(c);
        } else {
            safe.append(QLatin1Char('_'));
        }
    }
    return safe.isEmpty() ? QStringLiteral("unknown") : safe;
}
} // namespace

AccountManager &AccountManager::instance()
{
    static AccountManager s;
    return s;
}

QString AccountManager::appDataDir()
{
    return QStandardPaths::writableLocation(QStandardPaths::AppDataLocation);
}
QString AccountManager::accountsPath()   { return appDataDir() + QStringLiteral("/accounts.json"); }
QString AccountManager::identityTxtPath(){ return appDataDir() + QStringLiteral("/identity.txt"); }
QString AccountManager::keysDir()        { return appDataDir() + QStringLiteral("/keys"); }

QString AccountManager::generateId()
{
    const quint32 rnd = QRandomGenerator::securelySeeded().generate();
    return QStringLiteral("sc-%1").arg(rnd, 8, 16, QLatin1Char('0'));
}

void AccountManager::ensureLoaded() { if (!m_loaded) load(); }

bool AccountManager::idExists(const QString &id) const
{
    return std::any_of(m_accounts.cbegin(), m_accounts.cend(),
                       [&](const Account &a) { return a.id == id; });
}

void AccountManager::load()
{
    m_loaded = true;
    QDir().mkpath(appDataDir());

    // 1) Try the registry.
    QFile f(accountsPath());
    if (f.open(QIODevice::ReadOnly)) {
        const QJsonDocument doc = QJsonDocument::fromJson(f.readAll());
        f.close();
        if (doc.isObject()) {
            const QJsonObject root = doc.object();
            m_accounts.clear();
            const QJsonArray arr = root.value(QStringLiteral("accounts")).toArray();
            for (const QJsonValue &v : arr) {
                const QJsonObject o = v.toObject();
                Account a;
                a.id        = o.value(QStringLiteral("id")).toString();
                a.label     = o.value(QStringLiteral("label")).toString();
                a.createdAt = static_cast<qint64>(o.value(QStringLiteral("createdAt")).toDouble());
                if (a.id.startsWith(QStringLiteral("sc-"))) {
                    if (a.label.isEmpty()) a.label = a.id;
                    m_accounts.append(a);
                }
            }
            m_activeId = root.value(QStringLiteral("active")).toString();
            if (!m_accounts.isEmpty()) {
                if (!idExists(m_activeId)) m_activeId = m_accounts.first().id;
                syncIdentityTxt();
                return;
            }
        }
        qCWarning(lcAccounts) << "accounts.json present but unusable; reseeding";
    }

    // 2) No usable registry — migrate a legacy identity.txt if present.
    QString migrated;
    QFile idf(identityTxtPath());
    if (idf.open(QIODevice::ReadOnly)) {
        const QString id = QString::fromLatin1(idf.readAll().trimmed());
        idf.close();
        if (id.startsWith(QStringLiteral("sc-")) && id.size() == 11) migrated = id;
    }

    // 3) Seed the registry with one account (migrated or freshly generated).
    Account a;
    a.id        = migrated.isEmpty() ? generateId() : migrated;
    a.label     = a.id;
    a.createdAt = QDateTime::currentSecsSinceEpoch();
    m_accounts  = { a };
    m_activeId  = a.id;
    save();
    syncIdentityTxt();
    qCInfo(lcAccounts) << (migrated.isEmpty() ? "Generated first account"
                                              : "Migrated identity into account registry")
                       << a.id;
}

void AccountManager::save()
{
    QJsonObject root;
    root[QStringLiteral("version")] = 1;
    root[QStringLiteral("active")]  = m_activeId;
    QJsonArray arr;
    for (const Account &a : m_accounts) {
        QJsonObject o;
        o[QStringLiteral("id")]        = a.id;
        o[QStringLiteral("label")]     = a.label;
        o[QStringLiteral("createdAt")] = static_cast<double>(a.createdAt);
        arr.append(o);
    }
    root[QStringLiteral("accounts")] = arr;

    QFile f(accountsPath());
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        f.setPermissions(QFileDevice::ReadOwner | QFileDevice::WriteOwner);
        f.write(QJsonDocument(root).toJson(QJsonDocument::Indented));
    } else {
        qCWarning(lcAccounts) << "Could not write" << accountsPath();
    }
}

void AccountManager::syncIdentityTxt()
{
    // Keep the legacy file equal to the active id so any code (or a previous
    // version) that still reads identity.txt sees the right identity, and so a
    // restart loads the active account.
    QFile f(identityTxtPath());
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        f.setPermissions(QFileDevice::ReadOwner | QFileDevice::WriteOwner);
        f.write(m_activeId.toLatin1());
    }
}

void AccountManager::deleteKeyFiles(const QString &id)
{
    const QString safe = sanitizeForKeyFile(id);
    QDir kd(keysDir());
    if (!kd.exists()) return;
    const QStringList files = kd.entryList(QStringList{ safe + QStringLiteral(".*") }, QDir::Files);
    for (const QString &fn : files) {
        if (fn.startsWith(safe + QLatin1Char('.'))) // exact prefix then dot
            kd.remove(fn);
    }
}

QString AccountManager::activeId() { ensureLoaded(); return m_activeId; }

Account AccountManager::active()
{
    ensureLoaded();
    for (const Account &a : m_accounts)
        if (a.id == m_activeId) return a;
    return m_accounts.isEmpty() ? Account{} : m_accounts.first();
}

QList<Account> AccountManager::accounts() { ensureLoaded(); return m_accounts; }
int            AccountManager::count()    { ensureLoaded(); return m_accounts.size(); }

QString AccountManager::createAccount(const QString &label)
{
    ensureLoaded();
    QString id;
    do { id = generateId(); } while (idExists(id));
    Account a;
    a.id        = id;
    a.label     = label.isEmpty() ? id : label;
    a.createdAt = QDateTime::currentSecsSinceEpoch();
    m_accounts.append(a);
    save();
    qCInfo(lcAccounts) << "Created account" << id;
    return id;
}

bool AccountManager::setActive(const QString &id)
{
    ensureLoaded();
    if (!idExists(id)) return false;
    if (m_activeId == id) return true;
    m_activeId = id;
    save();
    syncIdentityTxt();
    qCInfo(lcAccounts) << "Active account set to" << id << "(applies on restart)";
    return true;
}

QString AccountManager::rerollActive(const QString &label)
{
    ensureLoaded();
    const QString id = createAccount(label);
    setActive(id);
    return id;
}

bool AccountManager::renameAccount(const QString &id, const QString &label)
{
    ensureLoaded();
    for (Account &a : m_accounts) {
        if (a.id == id) {
            a.label = label.isEmpty() ? id : label;
            save();
            return true;
        }
    }
    return false;
}

bool AccountManager::removeAccount(const QString &id)
{
    ensureLoaded();
    if (!idExists(id)) return false;
    if (id == m_activeId) {
        qCWarning(lcAccounts) << "Refusing to remove the active account" << id;
        return false;
    }
    if (m_accounts.size() <= 1) return false;
    m_accounts.erase(std::remove_if(m_accounts.begin(), m_accounts.end(),
                     [&](const Account &a) { return a.id == id; }),
                     m_accounts.end());
    save();
    deleteKeyFiles(id);
    qCInfo(lcAccounts) << "Removed account" << id;
    return true;
}

} // namespace SnapClone::Core
