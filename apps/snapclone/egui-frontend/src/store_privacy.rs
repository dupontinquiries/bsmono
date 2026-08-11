//! store_privacy — the storage rules from `StorageManager` that exist to stop
//! the database leaking things the user never agreed to publish.
//!
//! Three of them, each with the specific leak it prevents named in the source.
//!
//! ## Timestamps are UTC because a local-time string carries a location
//!
//! *"QDateTime::toString(Qt::ISODate) on a local-time value embeds the timezone
//! offset (e.g. `+05:30`), leaking the user's location."*
//!
//! That is the whole argument and it is a good one. A `+05:30` offset narrows a
//! stranger to a handful of countries, and it arrives inside a field nobody
//! thinks of as location data — so it survives every review that asks "are we
//! sending GPS?".
//!
//! Reading migrates too: *"migrate any legacy local-time strings on read"*,
//! because a row written by an older build is still on disk and the leak is in
//! the row, not the writer.
//!
//! ## Last-seen is rounded because precision is a presence pattern
//!
//! *"Precise lastSeen timestamps fingerprint the user's presence pattern;
//! 5-minute buckets preserve 'online this morning' while hiding exact time."*
//!
//! The feature needs to know roughly when someone was around. It does not need
//! to know they open the app at 07:42 every weekday, which is a commute, a
//! wake-up time and an address.
//!
//! ## Migrations are append-only
//!
//! *"NEVER modify existing entries — always append new ones."* An edited
//! migration produces two databases with the same version number and different
//! schemas, and every later migration then runs against a shape it was not
//! written for.

/// Bucket size for last-seen, in milliseconds. Five minutes.
pub const LAST_SEEN_BUCKET_MS: i64 = 300_000;

/// Consecutive failures before a cached peer address is skipped and a fresh
/// DHT lookup forced.
pub const ADDR_FAIL_THRESHOLD: u32 = 5;

/// How long a DHT lookup result stays usable.
pub const DHT_CACHE_TTL_MS: i64 = 3_600_000;

/// Round a timestamp down to its privacy bucket.
///
/// Floor, not nearest: rounding to nearest can move a timestamp into the
/// FUTURE, and a last-seen in the future reads as a clock bug and can make
/// freshness comparisons negative.
pub fn bucket_last_seen(ms: i64) -> i64 {
    ms.div_euclid(LAST_SEEN_BUCKET_MS) * LAST_SEEN_BUCKET_MS
}

/// Does this string carry a timezone offset?
///
/// Anything other than `Z` means a local-time value reached storage, which is
/// the leak the UTC rule exists to prevent.
pub fn leaks_timezone(iso: &str) -> bool {
    let Some(time_part) = iso.split('T').nth(1) else { return false };
    if time_part.ends_with('Z') { return false; }
    // A '+' or a '-' after the hour field is an offset. The date's own hyphens
    // are before the 'T', so looking only after it is what makes this safe.
    time_part.contains('+') || time_part.contains('-')
}

/// Is a stored timestamp acceptable?
pub fn is_utc_iso(iso: &str) -> bool {
    !iso.is_empty() && iso.contains('T') && !leaks_timezone(iso)
}

/// Normalise a timestamp for storage: UTC, `Z`-suffixed.
///
/// Takes epoch millis rather than a formatted string, because a formatting
/// helper that accepts a string can be handed one that already has an offset —
/// and then it either has to parse it or pass the leak through.
pub fn to_utc_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
            rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Days since epoch to a calendar date (Howard Hinnant's civil_from_days).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A cached peer address and its failure record.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedAddr {
    pub host: String,
    pub port: u16,
    pub consecutive_failures: u32,
    pub cached_at_ms: i64,
}

impl CachedAddr {
    /// Should this address be tried, or is a DHT lookup forced?
    ///
    /// Both conditions, because they fail differently: a stale entry may still
    /// be correct and is worth one attempt, while a repeatedly-failing one is
    /// known wrong and trying it again costs a timeout the user waits through.
    pub fn is_usable(&self, now_ms: i64) -> bool {
        self.consecutive_failures < ADDR_FAIL_THRESHOLD
            && now_ms.saturating_sub(self.cached_at_ms) < DHT_CACHE_TTL_MS
    }

    pub fn record_failure(&mut self) { self.consecutive_failures += 1; }

    /// A success clears the record entirely rather than decrementing.
    ///
    /// A peer that was unreachable for an hour and is now answering is not
    /// four-fifths broken; carrying old failures forward would evict it again
    /// after one more blip.
    pub fn record_success(&mut self, now_ms: i64) {
        self.consecutive_failures = 0;
        self.cached_at_ms = now_ms;
    }
}

/// A schema migration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Migration {
    pub version: u32,
    /// Hash of the SQL, so an edit to a shipped migration is detectable.
    pub sql_hash: u64,
}

/// Check a migration list is append-only and gap-free.
///
/// *"NEVER modify existing entries — always append new ones."* An edited
/// migration produces two databases with the same version number and different
/// schemas, and every later migration then runs against a shape it was not
/// written for. A gap is just as bad: version 4 missing means a database at 3
/// silently skips to 5.
pub fn check_migrations(list: &[Migration], shipped: &[Migration]) -> Result<(), String> {
    for (i, m) in list.iter().enumerate() {
        let expected = i as u32 + 1;
        if m.version != expected {
            return Err(format!("migration {} is out of order (expected {expected})", m.version));
        }
    }
    for s in shipped {
        match list.iter().find(|m| m.version == s.version) {
            None => return Err(format!("shipped migration {} was removed", s.version)),
            Some(m) if m.sql_hash != s.sql_hash =>
                return Err(format!("shipped migration {} was EDITED", s.version)),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_time_string_is_recognised_as_a_leak() {
        // "+05:30 leaks the user's location" — and it arrives inside a field
        // nobody thinks of as location data, so it survives every review that
        // asks "are we sending GPS?".
        assert!(leaks_timezone("2026-08-11T07:42:00+05:30"));
        assert!(leaks_timezone("2026-08-11T07:42:00-08:00"));
        assert!(!leaks_timezone("2026-08-11T07:42:00Z"));
    }

    #[test]
    fn the_dates_own_hyphens_are_not_mistaken_for_an_offset() {
        // Looking at the whole string would flag every valid timestamp, and a
        // validator that rejects everything gets switched off.
        assert!(is_utc_iso("2026-08-11T07:42:00Z"));
        assert!(!leaks_timezone("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn an_empty_or_dateless_string_is_not_valid_storage() {
        assert!(!is_utc_iso(""));
        assert!(!is_utc_iso("2026-08-11"));
    }

    #[test]
    fn formatting_takes_epoch_millis_so_an_offset_cannot_pass_through() {
        // A helper that accepted a formatted string could be handed one that
        // already has an offset, and would then either parse it or pass the
        // leak along.
        let s = to_utc_iso(0);
        assert_eq!(s, "1970-01-01T00:00:00Z");
        assert!(is_utc_iso(&s));
    }

    #[test]
    fn formatting_is_correct_across_a_leap_day() {
        // Same reasoning as date_phrase: the civil-date algorithm handles leap
        // years by construction rather than by a rule somebody remembers.
        assert_eq!(to_utc_iso(1_709_164_800_000), "2024-02-29T00:00:00Z");
        assert_eq!(to_utc_iso(1_767_225_599_000), "2025-12-31T23:59:59Z");
    }

    #[test]
    fn last_seen_is_rounded_to_five_minutes() {
        // "5-minute buckets preserve 'online this morning' while hiding exact
        // time." The feature does not need to know somebody opens the app at
        // 07:42 every weekday — that is a commute, a wake-up time and an
        // address.
        let precise = 1_700_000_000_000i64;
        let bucketed = bucket_last_seen(precise);
        assert_eq!(bucketed % LAST_SEEN_BUCKET_MS, 0);
        assert!(precise - bucketed < LAST_SEEN_BUCKET_MS);
    }

    #[test]
    fn rounding_floors_rather_than_going_to_nearest() {
        // Rounding to nearest can move a timestamp into the FUTURE, and a
        // last-seen in the future reads as a clock bug and makes freshness
        // comparisons negative.
        let t = 4 * LAST_SEEN_BUCKET_MS - 1;
        assert_eq!(bucket_last_seen(t), 3 * LAST_SEEN_BUCKET_MS);
        assert!(bucket_last_seen(t) <= t, "bucketing produced a future timestamp");
    }

    #[test]
    fn a_pre_epoch_timestamp_still_floors_downward() {
        // div_euclid rather than plain division: integer division truncates
        // toward zero, which for a negative value rounds UP.
        let t = -1i64;
        assert!(bucket_last_seen(t) <= t);
        assert_eq!(bucket_last_seen(t), -LAST_SEEN_BUCKET_MS);
    }

    #[test]
    fn two_timestamps_in_one_bucket_are_indistinguishable() {
        // The actual privacy property: the whole point is that these collide.
        //
        // My first version of this test picked two times 120 s apart and
        // asserted they matched — but 120 s apart genuinely STRADDLES a
        // boundary, so the test was wrong and the code was right. The property
        // is that any two times within the SAME bucket collide, which is what
        // this now checks: an exact bucket start and the last millisecond of
        // that bucket.
        let start = bucket_last_seen(1_700_000_000_000);
        let last = start + LAST_SEEN_BUCKET_MS - 1;
        assert_eq!(bucket_last_seen(start), bucket_last_seen(last));
        // and the next millisecond is a different bucket, or nothing is hidden
        assert_ne!(bucket_last_seen(last), bucket_last_seen(last + 1));
    }

    #[test]
    fn a_repeatedly_failing_address_forces_a_fresh_lookup() {
        // Trying a known-wrong address costs a timeout the user waits through.
        let mut a = CachedAddr { host: "1.2.3.4".into(), port: 9876,
                                 consecutive_failures: 0, cached_at_ms: 0 };
        assert!(a.is_usable(0));
        for _ in 0..ADDR_FAIL_THRESHOLD { a.record_failure(); }
        assert!(!a.is_usable(0));
    }

    #[test]
    fn a_stale_but_unfailed_address_also_forces_a_lookup() {
        // Two different failure modes, so both conditions are checked: a stale
        // entry may still be right, but an hour is the point past which it is
        // more likely wrong than not.
        let a = CachedAddr { host: "1.2.3.4".into(), port: 9876,
                             consecutive_failures: 0, cached_at_ms: 0 };
        assert!(a.is_usable(DHT_CACHE_TTL_MS - 1));
        assert!(!a.is_usable(DHT_CACHE_TTL_MS));
    }

    #[test]
    fn a_success_clears_the_failure_record_entirely() {
        // A peer unreachable for an hour and now answering is not four-fifths
        // broken; carrying old failures forward would evict it after one blip.
        let mut a = CachedAddr { host: "1.2.3.4".into(), port: 9876,
                                 consecutive_failures: 4, cached_at_ms: 0 };
        a.record_success(50_000);
        assert_eq!(a.consecutive_failures, 0);
        assert!(a.is_usable(50_000));
    }

    #[test]
    fn an_edited_migration_is_caught() {
        // "NEVER modify existing entries." An edit produces two databases with
        // the same version number and DIFFERENT SCHEMAS, and every later
        // migration then runs against a shape it was not written for.
        let shipped = [Migration { version: 1, sql_hash: 0xAAAA },
                       Migration { version: 2, sql_hash: 0xBBBB }];
        let edited = [Migration { version: 1, sql_hash: 0xAAAA },
                      Migration { version: 2, sql_hash: 0xCCCC }];
        let err = check_migrations(&edited, &shipped).unwrap_err();
        assert!(err.contains("EDITED"), "{err}");
    }

    #[test]
    fn a_removed_migration_is_caught() {
        // A database at version 1 would never reach the schema version 2 built.
        let shipped = [Migration { version: 1, sql_hash: 1 },
                       Migration { version: 2, sql_hash: 2 }];
        let short = [Migration { version: 1, sql_hash: 1 }];
        assert!(check_migrations(&short, &shipped).unwrap_err().contains("removed"));
    }

    #[test]
    fn a_gap_in_versions_is_caught() {
        // Version 4 missing means a database at 3 silently skips to 5.
        let list = [Migration { version: 1, sql_hash: 1 },
                    Migration { version: 3, sql_hash: 3 }];
        assert!(check_migrations(&list, &[]).is_err());
    }

    #[test]
    fn appending_a_migration_is_allowed() {
        // The one operation the rule permits, so the check must not block it.
        let shipped = [Migration { version: 1, sql_hash: 1 }];
        let grown = [Migration { version: 1, sql_hash: 1 },
                     Migration { version: 2, sql_hash: 2 }];
        assert!(check_migrations(&grown, &shipped).is_ok());
    }
}
