//! wizard — what is wrong with this mail setup, in the order it matters.
//!
//! Bridge-style mail clients tend to get you signed in and stop. Whether
//! your key is protected by the screen lock, whether outgoing mail is
//! signed, whether an incoming signature is even checked, whether the key
//! has an expiry you are about to walk into — none of that is surfaced, so
//! the user believes they have end-to-end mail when they have transport
//! encryption and a decrypt path. That is the gap this reports.
//!
//! [`Facts`] is what the app observed. [`checkup`] turns it into ordered
//! [`Finding`]s. Pure: the observing is the caller's, the judgement is
//! here, and the judgement is the part worth testing.

use crate::{Plan, Store};

/// How much it matters. Ordered worst-first, which is the order the wizard
/// shows and the order [`checkup`] sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something the user believes is protecting them and is not.
    Critical,
    /// Real exposure, but the user has not been told otherwise.
    Warn,
    /// Worth doing; nothing is currently wrong.
    Advice,
    Good,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::Warn => "warning",
            Severity::Advice => "advice",
            Severity::Good => "ok",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// One line naming the state.
    pub title: String,
    /// Why it matters, in the user's terms — not the mechanism.
    pub detail: String,
    /// What to do. `None` when nothing is wrong.
    pub remedy: Option<String>,
}

/// What the app observed about this setup.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// Which store the secret actually landed in.
    pub store_in_use: Store,
    /// The plan that produced it, for explaining the choice.
    pub plan: Plan,
    /// Can the engine verify a signature on incoming mail?
    pub can_verify: bool,
    /// Can it sign outgoing mail?
    pub can_sign: bool,
    /// Days until the primary key expires. `None` = no expiry set.
    pub key_expires_in_days: Option<i64>,
    /// Has the user ever exported a revocation certificate?
    pub has_revocation_cert: bool,
    /// Is the mailbox password distinct from the login password? Proton's
    /// two-password mode; when they are the same, one leak is both.
    pub two_password_mode: bool,
}

/// Keys inside this window are "about to expire" — long enough to renew
/// without panic, short enough that saying it is useful.
pub const EXPIRY_SOON_DAYS: i64 = 30;

pub fn checkup(f: &Facts) -> Vec<Finding> {
    let mut out = Vec::new();

    // Storage.
    if !f.store_in_use.locks_with_session() {
        let tried: Vec<&str> = f.plan.steps.iter()
            .map(|s| s.store.label())
            .filter(|l| *l != Store::EncryptedFile.label())
            .collect();
        out.push(Finding {
            severity: Severity::Warn,
            title: "Your key is in a file the screen lock does not cover".into(),
            detail: format!(
                "Anyone who can read your home directory while you are logged in can take it. \
                 {} was tried and did not answer.",
                if tried.is_empty() { "No wallet".to_string() } else { tried.join(" and ") }),
            remedy: Some(match f.plan.desktop {
                crate::Desktop::Kde | crate::Desktop::PlasmaMobile =>
                    "Install kwalletmanager and the kwallet-secrets bridge, then sign in again.".into(),
                crate::Desktop::Headless =>
                    "On a machine with no desktop this is expected; keep the file on an encrypted disk.".into(),
                _ => "Install gnome-keyring (or seahorse) and sign in again.".into(),
            }),
        });
    } else {
        out.push(Finding {
            severity: Severity::Good,
            title: format!("Your key is in {}", f.store_in_use.label()),
            detail: "It is locked with your session, like the rest of your passwords.".into(),
            remedy: None,
        });
    }

    // The one people believe is on and is not.
    if !f.can_verify {
        out.push(Finding {
            severity: Severity::Critical,
            title: "Signatures on incoming mail are not checked".into(),
            detail: "Mail can be decrypted, so it looks end-to-end encrypted — but nothing \
                     confirms who sent it. A forged message is indistinguishable from a real one."
                .into(),
            remedy: Some("Enable signature verification, or use a build with a PGP engine that \
                          can verify.".into()),
        });
    }
    if !f.can_sign {
        out.push(Finding {
            severity: Severity::Warn,
            title: "Outgoing mail is not signed".into(),
            detail: "Recipients cannot confirm the mail is from you, and correspondents who \
                     require a signature will see it as unverified.".into(),
            remedy: Some("Enable signing for this account.".into()),
        });
    }

    // Expiry: only a finding when it is actually near.
    match f.key_expires_in_days {
        Some(d) if d < 0 => out.push(Finding {
            severity: Severity::Critical,
            title: "Your key has expired".into(),
            detail: "Correspondents' clients will refuse it, and new mail to you may be \
                     rejected or sent unencrypted.".into(),
            remedy: Some("Extend the expiry date on your key and publish the update.".into()),
        }),
        Some(d) if d <= EXPIRY_SOON_DAYS => out.push(Finding {
            severity: Severity::Warn,
            title: format!("Your key expires in {d} day{}", if d == 1 { "" } else { "s" }),
            detail: "Once it expires, correspondents' clients will refuse it.".into(),
            remedy: Some("Extend the expiry now — it takes a moment and avoids the outage.".into()),
        }),
        _ => {}
    }

    if !f.has_revocation_cert {
        out.push(Finding {
            severity: Severity::Advice,
            title: "No revocation certificate saved".into(),
            detail: "If you lose this key or it is stolen, you cannot tell anyone to stop \
                     using it. A revocation certificate is the only way, and it must be made \
                     while you still have the key.".into(),
            remedy: Some("Generate one now and keep it somewhere other than this machine.".into()),
        });
    }

    if !f.two_password_mode {
        out.push(Finding {
            severity: Severity::Advice,
            title: "One password unlocks both the account and the mailbox".into(),
            detail: "Your login password also decrypts your mail, so a single leak gives up \
                     both.".into(),
            remedy: Some("Turn on two-password mode in your account settings.".into()),
        });
    }

    out.sort_by_key(|x| x.severity);
    out
}

/// The worst thing found — what a one-line banner shows.
pub fn headline(findings: &[Finding]) -> Option<&Finding> {
    findings.iter().find(|f| f.severity != Severity::Good)
}

/// True when nothing above `Advice` was found: the setup is sound enough to
/// stop nagging about.
pub fn is_sound(findings: &[Finding]) -> bool {
    !findings.iter().any(|f| matches!(f.severity, Severity::Critical | Severity::Warn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Desktop;

    fn good() -> Facts {
        Facts {
            store_in_use: Store::SecretService,
            plan: Plan::for_desktop(Desktop::Gnome),
            can_verify: true,
            can_sign: true,
            key_expires_in_days: Some(400),
            has_revocation_cert: true,
            two_password_mode: true,
        }
    }

    #[test]
    fn a_sound_setup_reports_only_good_news() {
        let f = checkup(&good());
        assert!(is_sound(&f));
        assert!(headline(&f).is_none(), "{:?}", headline(&f));
        assert!(f.iter().all(|x| x.severity == Severity::Good));
        assert!(f.iter().all(|x| x.remedy.is_none()));
    }

    #[test]
    fn not_verifying_signatures_outranks_everything_else() {
        // The point of the ordering: a user who cannot verify believes they
        // have authenticated mail and does not. That must be the headline
        // even when other things are also wrong.
        let mut facts = good();
        facts.can_verify = false;
        facts.can_sign = false;
        facts.store_in_use = Store::EncryptedFile;
        facts.has_revocation_cert = false;
        let f = checkup(&facts);
        assert_eq!(f[0].severity, Severity::Critical);
        assert!(f[0].title.contains("not checked"), "{}", f[0].title);
        assert!(!is_sound(&f));
        // Sorted worst-first throughout.
        assert!(f.windows(2).all(|w| w[0].severity <= w[1].severity), "{f:?}");
        // Every non-good finding tells the user what to do.
        assert!(f.iter().filter(|x| x.severity != Severity::Good).all(|x| x.remedy.is_some()));
    }

    #[test]
    fn the_file_fallback_remedy_names_the_right_wallet_for_the_desktop() {
        let mut kde = good();
        kde.store_in_use = Store::EncryptedFile;
        kde.plan = Plan::for_desktop(Desktop::Kde);
        let r = checkup(&kde).into_iter()
            .find(|x| x.title.contains("screen lock")).unwrap().remedy.unwrap();
        assert!(r.contains("kwallet"), "{r}");

        let mut gnome = good();
        gnome.store_in_use = Store::EncryptedFile;
        gnome.plan = Plan::for_desktop(Desktop::Gnome);
        let r = checkup(&gnome).into_iter()
            .find(|x| x.title.contains("screen lock")).unwrap().remedy.unwrap();
        assert!(r.contains("gnome-keyring"), "{r}");

        // Headless is not nagged to install a desktop wallet.
        let mut head = good();
        head.store_in_use = Store::EncryptedFile;
        head.plan = Plan::for_desktop(Desktop::Headless);
        let r = checkup(&head).into_iter()
            .find(|x| x.title.contains("screen lock")).unwrap().remedy.unwrap();
        assert!(r.contains("expected"), "{r}");
    }

    #[test]
    fn expiry_is_reported_only_when_it_is_actually_near() {
        let mut far = good();
        far.key_expires_in_days = Some(EXPIRY_SOON_DAYS + 1);
        assert!(!checkup(&far).iter().any(|x| x.title.contains("expires")));

        let mut none = good();
        none.key_expires_in_days = None;
        assert!(!checkup(&none).iter().any(|x| x.title.contains("expire")));

        let mut soon = good();
        soon.key_expires_in_days = Some(1);
        let f = checkup(&soon);
        let e = f.iter().find(|x| x.title.contains("expires")).unwrap();
        assert_eq!(e.severity, Severity::Warn);
        assert!(e.title.contains("1 day") && !e.title.contains("days"), "{}", e.title);

        let mut gone = good();
        gone.key_expires_in_days = Some(-1);
        let f = checkup(&gone);
        assert_eq!(f[0].severity, Severity::Critical);
        assert!(f[0].title.contains("has expired"), "{}", f[0].title);
    }
}
