# snapclone-menu

The dynamic submenu **model** for SnapClone's contact cards, group cards, and
mailbox/inbox rows. Given an entity's kind and state, it returns the ordered
action descriptors a UI should render — centralizing policy that was previously
scattered as hardcoded `if`s in the Qt code.

## What it decides

`actions_for(entity, edit_inline)` returns an ordered `Vec<MenuAction>`:
- **Contact**: Edit, Share location, Copy ID, (Unblock), Remove — gated by
  `editable / shareable / removable / blocked`.
- **Group**: Open, (Rename, Manage members — admin only), Mute/Unmute, Leave —
  gated by `is_admin / muted / removable`.
- **Mailbox entry**: if `pending` → Accept / Reject / Block; else Open /
  Mute(Unmute) / Remove.
- `header_actions(Group)` → New group (the surface "+").

Each `MenuAction` carries a stable `ActionId` (the UI dispatches on it, not the
localized label), an `ActionKind` (`Immediate` / `Dialog` / `Expand`), and a
`danger` flag. `edit_inline=true` flips Edit/Rename from `Dialog` to `Expand`
(an inline nested panel) so both styles share one ruleset.

## Why platform-free + tested

The Qt side becomes a thin loop — `for a in actions: card.addAction(...)` — and
the *policy* (which actions, order, danger, inline-vs-dialog) is unit-tested
here, identical across every surface and any future backend. One test pins that
the contact output matches the current hardcoded order, so adopting the model is
behavior-preserving, not a rewrite.

## C / C++

Link `libsnapclone_menu.a`, include `snapclone_menu_ffi.h`. Fill a caller-owned
`ScMenuAction[]` via `sc_menu_actions_for(kind, state_bits, edit_inline, out,
cap)`; strings are `'static` (never freed). See the header for an example.

## Status

17 tests (12 model + 5 FFI); release staticlib + header verified (C and C++).
Zero dependencies — builds standalone with stable Rust (in-workspace, build it
individually: the shared workspace lock pulls an unrelated toolchain drift).
NOT yet wired into the Qt UI — the contact card still has its inline action
list; swapping it to drive this model (and adding the group/mailbox surfaces) is
the next, behavior-preserving step.
