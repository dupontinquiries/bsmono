# Mail: UI-feedback bugs + multi-account UI (v0.1.901)

## What this pass found and fixed

Orientation: reviewed `docs/design/email.md`, `mail_hub`/`mail_session`/
`mail_account` (multi-account core already existed — one sealed file per
account, `SessionManager`, `AccountStore`), and how mail panes refresh
(`paneSplitLayer._webTick`, bumped by a 1500ms QML `Timer` — v0.1.881 — so
web2qml:// panes, including mail, already re-render on a ~1.5s cadence).
That refresh mechanism is fine as a *background* sync; the actual bug was
narrower: two mutating mail actions gave the user **no feedback at all**,
so they looked broken regardless of the periodic refresh.

### 1. Drafts pane "Send" button silently did nothing (`src/mail_panes.rs`)
`draft_pane_qml`'s Send button called `backend.mail_send_draft(id)` and
threw the return value away. `mail_send_draft` returns a `status\tdetail`
wire (currently always the honest "sign-in still being wired up, draft
saved locally" message) — but nothing in the drafts pane ever displayed
it. Clicking Send looked like a dead button. Fixed: wrapped the call in
`_setStatus(...)`, added the same status-line + `_setStatus` function
pattern the compose/peers panes already use. Test added
(`draft_pane_balanced` already existed and still passes; the button now
reads `_setStatus(backend.mail_send_draft(...))`).

### 2. Accounts settings "Remove" gave no feedback + no active-account concept
`mail_remove_account` returned `()`; the Remove button called it and
nothing else. On success the row disappeared only after the next ~1.5s
refresh tick; on failure (e.g. disk error) there was no indication at
all — the row just... stayed. Fixed:
- `mail_remove_account` now returns a `status\tdetail` wire.
- `accounts_settings_qml` gained a shared status line + `_setStatus`, and
  the Remove button routes through it.
- New tests: `accounts_pane_lists_and_removes` (updated),
  `accounts_pane_marks_active_and_offers_use_on_others`,
  `accounts_pane_no_active_marks_none`.

## Multi-account UI (the "multi-user" ask)

The storage/session layers were already multi-account (per-account sealed
files, `SessionManager` keyed by `AccountId`) but nothing in the UI let a
user with 2+ accounts see or pick which one is "current" — settings only
listed accounts with Remove. Added, all pure/session-local (no SRP
needed):

- `Backend.mail_active_account: String` — which account id the mail panes
  treat as current.
- `mail_set_active_account(id) -> QString` — validates the id exists,
  switches, returns `ok\t…` / `err\tUnknown account`.
- `mail_open_settings` now defaults `mail_active_account` to the first
  known account when nothing (valid) is selected, so there's always a
  concrete answer once ≥1 account exists.
- `accounts_settings_qml(style, accounts, active_id)` — signature change
  (was `(style, accounts)`); each row shows an "Active" badge + accent
  border when it matches `active_id`, otherwise a "Use this account"
  button. A one-line hint appears when there are 2+ accounts. Below one
  account, no active/use UI clutters the pane (nothing to disambiguate).

This doesn't touch login/send — those are still correctly gated on SRP
(`docs/design/email.md` #1, unchanged, not attempted here per that doc's
"must not be written blind" note). What it does is make "which account
am I working in" a real, visible, switchable piece of state so that once
SRP/PGP/send land, inbox rendering etc. have an active-account seam to
plug into immediately instead of needing this UI work done later too.

## Verified (sandbox)

No Qt/qmetaobject toolchain here, so `main.rs` itself isn't fully
buildable in-sandbox (same limitation noted in `BUILD_REPORT.md`). What
*was* verified:
- All touched pure-core files (`mail_panes.rs`, `mail_login.rs`, plus the
  rest of the mail module tree for context) typecheck cleanly as a
  standalone crate (zero-dep `ureq`/`security`/`qml_gen` shims) — 0
  errors.
- Full existing + new mail test suite: **95/95 pass**.
- The three edited `main.rs` methods (`mail_remove_account`,
  `mail_set_active_account`, `mail_open_settings`) use a `hub` borrow
  from `mail_hub_get(&mut self)` and then mutate `self.mail_active_account`
  — this is the classic NLL trap (mixing a `&mut self`-derived borrow with
  a later `&mut self` use), so I isolated and compiled the exact borrow
  shape standalone to confirm NLL ends the `hub` borrow at its last use
  before the mutation. Compiles clean.
- Struct wiring double-checked by hand: added the `mail_active_account`
  field, its one initializer, the new `qt_method!` declaration, and its
  matching `Default::default()` initializer in the method-table block
  (this project inits every `qt_method!` field a second time in the
  constructor — easy to miss one, so I grepped for all four sites and
  confirmed each has exactly one entry).

**Still needs on-device build + a manual pass**: this project's own
pattern (see `BUILD_SESSION_CHECKLIST.md`, `BUILD_REPORT.md`) is that the
qmetaobject/Qt surface can only be verified for real on hardware. Do a
`cargo check` there first; if something doesn't match, it's most likely a
qmetaobject macro-expansion detail my hand-verification couldn't see
(the four-site field wiring above is the most probable spot).

## Not attempted (deliberately)

- SRP login, PGP, send/receive — `docs/design/email.md` already flags
  these as needing a vetted crypto implementation, not a blind port.
  Nothing here changes that gating.
- `poll_new_mail` (built, in `mail.rs`, unused) — wiring a periodic
  inbox-poll timer needs an authenticated `Session`, which needs #1
  above. Noted here so it isn't lost: once SRP lands, this is a small,
  contained follow-up (a QML `Timer` calling a new `mail_poll` qt_method
  that calls `hub.poll_new_mail(session)` for the active account, feeding
  `mail_notify::notifier_bar_qml`).
- Full inbox rendering per active account — same SRP dependency.
