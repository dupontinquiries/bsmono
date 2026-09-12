# Open in voiceterm-egui after the 2026-09-08 session

A single list of what was left, so it is not spread across nine files. Each item names the
file that has the detail.

## Reported by the user, unaddressed

- [ ] **Menu and navigation parity with the Qt build** — `TODO_qt_parity_menus.md`. Needs
      both builds side by side; I cannot run the Qt one here.

## Fixed this session, worth confirming on the device

- [x] **Ctrl+C does not interrupt.** Not the encoder: egui-winit converts Ctrl+C into
      `Event::Copy` and returns, so the keystroke never reached the terminal. Now sent as
      0x03 when nothing is selected, left as a copy when something is.
      - [ ] Confirm on the device, and confirm that copying a selection still copies.
- [x] **On-screen keyboard letters** (Plasma virtual keyboard, Maliit). They arrive as
      `Ime::Commit`, not key events; the terminal and `palace` both only read `Event::Text`.
      - [ ] Confirm the keyboard now APPEARS over the terminal — the commit half is testable
            here, the `set_ime_allowed` half only shows on a real compositor.

## Built but not reached

- [ ] **`OpenInEditor` has no voice intent.** The command takes a phrase; nothing speaks one.
      `TODO_editor_open_context.md`.
- [ ] **Spawn into a pane** rather than staging on the line — same file.
- [ ] **Jot mode routes ONE surface**, the spoken-phrase box. Other transcript arrival points
      still go to the command parser. `TODO_notes_clip.md`.
- [ ] **`Focus::File` is never produced** — a clip from a file view needs a pane that reports
      a path and a selection line. `TODO_notes_clip.md`.
- [ ] **`mail_login_view` has no caller ON PURPOSE.** There is no mail hub in this frontend,
      so a login screen would collect a password and drop it. The reason is in the module
      header; do not "fix" it by wiring it to nothing.

## Not started

- [ ] **Image crops** — `TODO_notes_clip.md` step 5. Needs a crop gesture and a save path;
      the only item in that file that is not mostly plumbing.
- [ ] **The acoustic transport** — `TODO_multi_device_share.md`. `dial` and `listen` advance
      the state machine and no symbol reaches a speaker. Needs a live audio pipeline, which
      also blocks `voice_routing` and `acoustic_cache`.
- [ ] **The "task clusters" view** — `TODO_command_clustering.md`. The dominant label now
      names the session; the cluster STRUCTURE is not surfaced.
- [ ] **"Create card" from a word tap** is done; what remains is nothing — step 4 is closed.


## The Qt-to-egui port, measured (2026-09-08)

Comparing `src/*.rs` against what `egui-frontend/src/lib.rs` mounts, biggest first:

| lines | module | state |
|-------|--------|-------|
| 5,913 | `qml_gen` | Qt-only by definition; never ports. |
| 5,243 | `voice_intent` | **MOUNTED.** Brought 165 tests with it. |
| 4,117 | `manpages` | Blocked: its Qt-free half shares a file with `to_qml`, which pulls `qml_gen` + `scraper`. Needs an upstream split, on a machine that can build the app crate. |
| 2,944 | `pty_term` | Not looked at. |
| 1,860 | `phonetic` | **MOUNTED** (with `voice_intent`). |
| 1,632 | `audit_log` | Not looked at. |
| 1,602 | `dictate` | Not looked at. |
| 1,556 | `connections` | **BLOCKED — see below.** |

### connections: a name collision, not a missing dependency

The graph needs `chat`, which needs `token_log::parse_anthropic_usage` and `append_event`.
This crate ALREADY HAS a `token_log` — its own, ten functions against the Qt one's
twenty-three. Mounting the chain does not add a module, it collides with one, and the two are
not interchangeable.

Someone has to decide which is canonical and move the other's callers. Renaming one to get a
compile would leave two token ledgers writing to the same place with no way to tell which
number is right.

`layout_cache`, `archive` and `user_tags` mount cleanly. `archive` and `layout_cache` want
`scraper`, which would be the heaviest dependency in this crate — worth pricing deliberately
rather than acquiring as a side effect of a graph view.
