#pragma once
// ─── VCardIO ──────────────────────────────────────────────────────────────────
// vCard 3.0 import and export for the SnapClone address book.
//
// Custom field: X-SNAPCLONE-ID:<sc-XXXXXXXX>
//   Written on export so any vCard-capable app preserves the routing ID.
//   Read on import so contacts imported from another SnapClone user are
//   immediately connectable by DHT ID without needing a manual add.
//
// Standard fields used:
//   FN     — full name (mapped to displayName / nickname)
//   UID    — unique ID for deduplication on re-import
//   NOTE   — mapped to Contact::notes
//   REV    — last-modified timestamp
// ─────────────────────────────────────────────────────────────────────────────

#include "core/Types.hpp"
#include <QList>
#include <QString>

namespace SnapClone::Core {

class VCardIO {
public:
    /// Export a list of contacts to a single .vcf string (one vCard per contact).
    [[nodiscard]] static QString exportContacts(const QList<Contact> &contacts);

    /// Export a single contact to a vCard string.
    [[nodiscard]] static QString exportContact(const Contact &c);

    /// Parse a .vcf string (may contain multiple vCards) into Contact objects.
    /// Unknown fields are silently ignored.
    [[nodiscard]] static QList<Contact> importVCard(const QString &vcf);
};

} // namespace SnapClone::Core
