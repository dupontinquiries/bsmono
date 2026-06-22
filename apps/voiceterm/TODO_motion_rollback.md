# TODO: motion rollback — position-history stack (the full version)

User idea (v0.1.594): "queue the rollback words for undo actions that
would undo like the u key or a j then a k or a t... then keep the
backwards count of that buffer history like what line and char space we
were at."

## What shipped in v0.1.594 (the testable parts)
- `rollback <motion>` → inverse keystroke (j↔k, w↔b, top↔bottom,
  line-start↔line-end, next↔prev) via `editor_voice::helix_inverse_motion`.
  Motions with no clean inverse (f<char>, search) return None — no wrong
  guess.
- `jump back`/`jump forward` → helix's NATIVE jumplist (Ctrl-o/Ctrl-i),
  which restores real file positions.

## What's NOT built: the true position-history stack
The user's literal idea — track "what line and char we were at" and walk
backward through that history — needs something the inverse-keystroke
table can't give: knowledge of helix's LOGICAL cursor position (file
line:col), not the terminal screen cursor.

### Why screen cursor ≠ file position
pty_term tracks cursor_row/cursor_col, but those are SCREEN coordinates.
When helix scrolls, the same file line moves to a different screen row,
so a screen-based history would jump to the wrong place after any scroll.

### Design options (need the running build to validate)
1. **Status-line parse.** Helix renders `line:col` in its status bar.
   Each time we send a movement, parse the status line from the grid,
   push (line, col) onto a stack. "rollback" pops and sends `:line<ret>`
   + horizontal motion to col. Reliable-ish but coupled to helix's
   status format + needs reading rendered screen each move. Fragile
   across helix themes/versions that change the status layout.
2. **Keystroke ledger + inverse replay (shipped, approximate).** Already
   done for clean-inverse motions. Doesn't need position knowledge but
   can't handle f<char>/search/click-jumps.
3. **Lean on helix's jumplist (shipped for jump-class).** Ctrl-o already
   walks real positions, but only for "jumps" (search, goto, big
   motions), not every j/k.

### Recommendation
Ship 2+3 (done). Only build 1 (status-line parse → position stack) if
dogfooding shows 2+3 aren't enough — and build it behind a feature flag,
since status-line parsing is the fragile part. The pure piece of 1 (a
PositionStack: push/pop/peek with (line,col), and turning a target
(line,col) into the keystrokes to get there) IS testable and could be
built ahead of the parsing; the parsing itself needs the running build.

## v0.1.609 — the pure piece of option 1 is now built + tested
`src/motion_history.rs`:
- `PositionStack` (push/pop/peek, bounded) with SAME-LINE COALESCING:
  consecutive positions on one line collapse to a single entry (keeping
  the latest col), so rollback walks line-to-line, not keystroke-to-
  keystroke — matching the user's "keep the backwards count" intent.
  Revisiting a line after leaving it is a distinct entry.
- `keys_to_reach(Pos{line,col}) -> String`: helix keys to return to a
  remembered position — `<line>G` then `gh` (line start) then `<col>l`.
  col 0 omits the rightward motion. Robust (doesn't depend on prior
  horizontal position).
- 10 tests + an adversarial probe (coalescing on revisit, cap=1,
  interleaved lines, large values) — all clean.
- Contract: 1-based line, 0-based col (matches helix status-line format).

Remaining (build-gated, unchanged): the FRAGILE half — parsing helix's
status line each move to LEARN the current (line,col) to push onto the
stack. Wire: on each movement, parse line:col from the grid → stack.record;
on "rollback", stack.rollback() → pty-write keys_to_reach(pos). Build
behind a feature flag (status parsing is version/theme-coupled). The
stack + key-gen are done; only the screen-reading needs the live build.
