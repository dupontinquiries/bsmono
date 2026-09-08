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
