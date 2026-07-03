#include "core/VCardIO.hpp"
#include <QStringList>
#include <QUuid>
#include <QDateTime>
#include <QTextStream>
#include <QRegularExpression>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcVCard, "snapclone.vcard")

namespace SnapClone::Core {

// ─── Export ───────────────────────────────────────────────────────────────────

static QString vcardEscape(const QString &s)
{
    // vCard 3.0 requires escaping \ , ; and newlines
    return QString(s)
        .replace(QLatin1Char('\\'), QStringLiteral("\\\\"))
        .replace(QLatin1Char(','),  QStringLiteral("\\,"))
        .replace(QLatin1Char(';'),  QStringLiteral("\\;"))
        .replace(QLatin1Char('\n'), QStringLiteral("\\n"));
}

QString VCardIO::exportContact(const Contact &c)
{
    const QString name     = vcardEscape(c.label());
    const QString notes    = vcardEscape(c.notes);
    const QString uid      = c.vcardUid.isEmpty()
        ? QUuid::createUuid().toString(QUuid::WithoutBraces)
        : c.vcardUid;
    const QString rev      = QDateTime::currentDateTimeUtc()
        .toString(Qt::ISODate);

    QString out;
    QTextStream s(&out);
    s << "BEGIN:VCARD\r\n";
    s << "VERSION:3.0\r\n";
    s << "FN:"  << name << "\r\n";
    // N: family;given;additional;prefix;suffix
    s << "N:"   << name << ";;;;\r\n";
    s << "UID:" << uid  << "\r\n";
    s << "REV:" << rev  << "\r\n";
    // Custom SnapClone routing ID — preserved by vCard-aware apps
    if (!c.id.value.isEmpty())
        s << "X-SNAPCLONE-ID:" << c.id.value << "\r\n";
    if (!notes.isEmpty())
        s << "NOTE:" << notes << "\r\n";
    s << "END:VCARD\r\n";
    return out;
}

QString VCardIO::exportContacts(const QList<Contact> &contacts)
{
    QString out;
    for (const Contact &c : contacts)
        out += exportContact(c);
    return out;
}

// ─── Import ───────────────────────────────────────────────────────────────────

static QString vcardUnescape(const QString &s)
{
    QString r;
    r.reserve(s.size());
    bool esc = false;
    for (const QChar ch : s) {
        if (esc) {
            if (ch == QLatin1Char('n') || ch == QLatin1Char('N'))
                r += QLatin1Char('\n');
            else
                r += ch;
            esc = false;
        } else if (ch == QLatin1Char('\\')) {
            esc = true;
        } else {
            r += ch;
        }
    }
    return r;
}

// Unfold folded vCard lines (CRLF + whitespace = continuation)
static QStringList unfoldLines(const QString &vcf)
{
    QStringList result;
    QString current;
    for (const QString &line : vcf.split(QLatin1Char('\n'))) {
        const QString stripped = line.endsWith(QLatin1Char('\r'))
            ? line.chopped(1) : line;
        if (!stripped.isEmpty()
            && (stripped[0] == QLatin1Char(' ') || stripped[0] == QLatin1Char('\t'))) {
            current += stripped.mid(1); // continuation
        } else {
            if (!current.isEmpty()) result.append(current);
            current = stripped;
        }
    }
    if (!current.isEmpty()) result.append(current);
    return result;
}

QList<Contact> VCardIO::importVCard(const QString &vcf)
{
    QList<Contact> contacts;
    Contact current;
    bool inCard = false;

    for (const QString &line : unfoldLines(vcf)) {
        const QString upper = line.toUpper();

        if (upper == QStringLiteral("BEGIN:VCARD")) {
            inCard = true;
            current = Contact{};
            continue;
        }
        if (upper == QStringLiteral("END:VCARD")) {
            if (inCard && (!current.displayName.isEmpty()
                           || !current.id.value.isEmpty())) {
                // If no sc-ID, generate a placeholder so the contact is saveable
                if (current.id.value.isEmpty())
                    current.id.value = QStringLiteral("import-")
                        + (current.vcardUid.isEmpty()
                           ? QUuid::createUuid().toString(QUuid::WithoutBraces)
                           : current.vcardUid);
                contacts.append(current);
            }
            inCard = false;
            continue;
        }
        if (!inCard) continue;

        // Split "PROPERTY;params:value" — find first colon not inside quotes
        int colonPos = -1;
        bool inQuote = false;
        for (int i = 0; i < line.size(); ++i) {
            if (line[i] == QLatin1Char('"')) { inQuote = !inQuote; continue; }
            if (!inQuote && line[i] == QLatin1Char(':')) { colonPos = i; break; }
        }
        if (colonPos < 0) continue;

        // Strip parameters (everything after first ';' in the property name part)
        const QString prop  = line.left(colonPos).section(QLatin1Char(';'), 0, 0).toUpper();
        const QString value = vcardUnescape(line.mid(colonPos + 1).trimmed());

        // Cap all field values to prevent DoS via huge vCard fields (BUG G)
        constexpr int kMaxFieldLen = 1024;
        const QString safeValue = value.left(kMaxFieldLen);

        if (prop == QStringLiteral("FN")) {
            current.displayName = safeValue;
            if (current.nickname.isEmpty()) current.nickname = safeValue;
        } else if (prop == QStringLiteral("N")) {
            // N:family;given;... — use "given family" if FN not yet seen
            const QStringList parts = safeValue.split(QLatin1Char(';'));
            if (current.displayName.isEmpty()) {
                const QString given  = parts.size() > 1 ? parts[1].trimmed() : QString{};
                const QString family = parts.isEmpty()  ? QString{} : parts[0].trimmed();
                current.displayName  = (given + QLatin1Char(' ') + family).trimmed();
            }
        } else if (prop == QStringLiteral("UID")) {
            current.vcardUid = safeValue;
        } else if (prop == QStringLiteral("NOTE")) {
            current.notes = safeValue;
        } else if (prop == QStringLiteral("X-SNAPCLONE-ID")) {
            // Our custom field: validate it looks like a real sc-ID (BUG H)
            // Reject anything that doesn't match "sc-[hex]" to prevent
            // malicious vCards injecting arbitrary IDs used as filenames/DB keys.
            const QString trimmed = safeValue.trimmed();
            static const QRegularExpression kScIdRx(
                QStringLiteral("^sc-[0-9a-f]{8}$"),
                QRegularExpression::CaseInsensitiveOption);
            if (kScIdRx.match(trimmed).hasMatch())
                current.id.value = trimmed;
            else if (!trimmed.isEmpty())
                qCDebug(lcVCard) << "VCard: ignoring invalid X-SNAPCLONE-ID:" << trimmed;
        }
    }
    return contacts;
}

} // namespace SnapClone::Core
