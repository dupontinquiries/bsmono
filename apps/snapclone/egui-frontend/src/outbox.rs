//! outbox — post a snap into the shared mailbox, with the right lifetime.
//!
//! `composer::Duration` said whether a snap vanishes; nothing carried that into
//! storage. `shared/share-kit` is the mailbox above snapcore and crypto-kit:
//! content-addressed postings with a `Lifetime` that already means exactly what
//! a snap timer means — `Ephemeral` expires in 5 minutes and is not replicated,
//! `Durable` lasts ~30 days and is.
//!
//! ## The mapping is the whole point, and it is easy to get backwards
//!
//! A snap with a **timer** is `Ephemeral`. A snap with **no limit** — the
//! `Duration::Unlimited` that `m_durationSecs == 0` means — is `Durable`. Get
//! this wrong and either a timed snap is replicated across the cluster for a
//! month, or an unlimited one is swept five minutes after it is sent.
//!
//! The first of those is a privacy failure the user cannot see, which is why it
//! gets a test that reads the way the mistake would: *a timed snap is never
//! durable.*
//!
//! ## Replication follows the lifetime, not the sender's wish
//!
//! `Lifetime::replicate()` is false for ephemeral, and this module does not
//! offer an override. A "send this everywhere but delete it in ten seconds"
//! option sounds reasonable and is a contradiction: replication is what makes
//! deletion unverifiable.

use crate::composer::Duration;
pub use share_kit::mailbox::Lifetime;

/// How a composed snap is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub lifetime: Lifetime,
    /// Seconds the recipient may view it for; `None` for unlimited. Carried
    /// separately from the TTL because they answer different questions — how
    /// long it survives in the store versus how long a viewer sees it.
    pub view_secs: Option<u32>,
}

impl Delivery {
    /// Derive from the composer's duration.
    pub fn of(d: Duration) -> Delivery {
        match d {
            // A timer means it must not persist and must not spread.
            Duration::Seconds(s) => Delivery { lifetime: Lifetime::Ephemeral, view_secs: Some(s) },
            // No limit means keep it; this is the saved-photo case.
            Duration::Unlimited => Delivery { lifetime: Lifetime::Durable, view_secs: None },
        }
    }

    /// Store TTL, from share-kit rather than a number chosen here — a second
    /// opinion about how long ephemeral means is how a snap outlives its own
    /// deletion.
    pub fn ttl_secs(&self) -> u64 { self.lifetime.ttl_secs() }

    /// Will this be copied onto the cluster roster?
    pub fn replicates(&self) -> bool { self.lifetime.replicate() }

    /// A line for the send confirmation. Says what will HAPPEN to the snap,
    /// because "ephemeral" is jargon and "gone in 5 minutes if unopened" is not.
    pub fn explain(&self) -> String {
        match self.view_secs {
            Some(s) => format!("{s}s to view, removed within {} minutes if unopened",
                               self.ttl_secs() / 60),
            None => "saved — no timer, stays until deleted".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timed_snap_is_never_durable() {
        // Written the way the mistake would read. Getting this backwards
        // replicates a timed snap across the cluster for a month — a privacy
        // failure the user cannot see.
        for s in [1u32, 5, 10, 60, 3600] {
            let d = Delivery::of(Duration::Seconds(s));
            assert_eq!(d.lifetime, Lifetime::Ephemeral, "{s}s snap");
            assert!(!d.replicates(), "{s}s snap must not spread");
        }
    }

    #[test]
    fn an_unlimited_snap_is_durable_because_zero_means_no_limit() {
        // The same inversion the composer encodes: m_durationSecs == 0 is "no
        // limit", not "gone instantly".
        let d = Delivery::of(Duration::Unlimited);
        assert_eq!(d.lifetime, Lifetime::Durable);
        assert!(d.replicates());
        assert_eq!(d.view_secs, None);
    }

    #[test]
    fn the_ttl_comes_from_share_kit_rather_than_a_second_opinion() {
        // A number chosen here is how a snap outlives its own deletion: the
        // store sweeps on ITS ttl, not ours.
        assert_eq!(Delivery::of(Duration::Seconds(5)).ttl_secs(),
                   Lifetime::Ephemeral.ttl_secs());
        assert_eq!(Delivery::of(Duration::Unlimited).ttl_secs(),
                   Lifetime::Durable.ttl_secs());
        assert!(Delivery::of(Duration::Seconds(5)).ttl_secs()
                < Delivery::of(Duration::Unlimited).ttl_secs());
    }

    #[test]
    fn the_view_timer_and_the_store_ttl_are_different_questions() {
        // How long a viewer sees it, versus how long it survives unopened.
        // Conflating them either deletes an unopened snap immediately or keeps
        // a viewed one for five minutes.
        let d = Delivery::of(Duration::Seconds(5));
        assert_eq!(d.view_secs, Some(5));
        assert!(d.ttl_secs() > 5, "it survives long enough to be opened");
    }

    #[test]
    fn the_confirmation_says_what_happens_rather_than_naming_a_mode() {
        // "Ephemeral" is jargon; "5s to view, removed within 5 minutes if
        // unopened" is something a person can act on.
        let e = Delivery::of(Duration::Seconds(5)).explain();
        assert!(e.contains("5s to view"), "{e}");
        assert!(e.contains("unopened"), "{e}");
        assert!(!e.to_lowercase().contains("ephemeral"), "{e}");
        let d = Delivery::of(Duration::Unlimited).explain();
        assert!(d.contains("no timer"), "{d}");
    }
}
