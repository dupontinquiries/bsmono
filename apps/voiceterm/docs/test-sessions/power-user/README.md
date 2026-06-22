# Power-user test session

A structured walkthrough exercising the power-user features
shipped across v0.1.349–v0.1.358. Run through this when:

- You want to dogfood a fresh build end-to-end.
- You suspect a regression in an area the regular tests don't
  cover (most of these features have voice / UI surfaces that
  unit tests can't exercise).
- You're onboarding someone to web2qml and want them to see
  the toolkit.

Working time: **~25 minutes** if everything passes. Allow more
for note-taking failures.

## What this session covers

Grouped by ship, in order of expected encounter:

| Ship | Feature | Section |
|------|---------|---------|
| v0.1.349 | Scrollback search (basic) | §2 |
| v0.1.350 | Long-cmd notifications + all-matches highlight | §3 |
| v0.1.351 | Git status badge | §4 |
| v0.1.352 | Token accounting + cost man page | §5 |
| v0.1.353 | Keybinding audit + ? overlay | §6 |
| v0.1.354 | Voice "what branch" + chord registry lint | §7 |
| v0.1.355 | Find bar polish (case + history) + voice switch-branch | §8 |
| v0.1.356 | Smart git command parsing + branch pre-validation | §9 |
| v0.1.357 | More git verbs (amend, blame, log-search, file-history) | §10 |
| v0.1.358 | Visual destructive-op signaling | §11 |
| v0.1.359 | PC2a — spoken-name introduction (onboarding) | §11.5 |
| v0.1.360 | Even more git verbs (reflog, tags, fetch/pull, cherry-pick) | §11.6 |
| v0.1.361 | Filesystem find mode (rg/grep over cwd) | §11.7 |
| v0.1.363 | SSH session detection (UI adapts) | §11.8 |
| v0.1.364 | Token-stats time-window filters (Session/Today/7d/All) | §11.9 |
| v0.1.365 | Op-tag refinement + reset button | §11.10 |
| v0.1.367 | Gemini archive viewer | §11.11 |
| cross-cuts | Multi-feature interactions | §12 |

## How to use this document

1. Read §1 (pre-flight) before starting. Some sections require
   specific setup that should be done once.
2. Work through §2–§11 in order. Each section names the SHIP
   that introduced the feature so it's easy to skip already-
   verified work.
3. For each test: do the **Steps**, observe the **Expected**
   outcome. If something doesn't match, jot it in the **Notes**
   slot — there's a template at `templates/failure-note.md`
   you can copy.
4. §12 cross-cuts the features. Run those last; they assume
   §2–§11 are working.
5. End at §13 cleanup. Reset state so the next session starts
   fresh.

If you find a real bug, copy the failure note into
`docs/todo.md` under the relevant feature section so it
doesn't get lost.

---

## §1 — Pre-flight

Things that need to be set up ONCE before the session:

### 1.1 Build & launch

```bash
cd ~/Code/web2qml  # or wherever your checkout lives
cargo r --release --features="voice claude"
```

The build is ~minutes on Asahi/aarch64. Subsequent builds are
faster (incremental).

### 1.2 Test git repo

A throwaway repo with predictable state. The script
`scripts/setup-test-repo.sh` creates this for you:

```bash
bash docs/test-sessions/power-user/scripts/setup-test-repo.sh
```

This creates `/tmp/w2q-test-repo/` with:
- 5 commits on `main`
- An additional branch `feature/foo`
- A second branch `feature/bar` with 2 extra commits (so
  ahead/behind shows up against main)
- One uncommitted modified file (so dirty state is reproducible)
- One staged but uncommitted file

If you've run the session before, delete and recreate:

```bash
rm -rf /tmp/w2q-test-repo
bash docs/test-sessions/power-user/scripts/setup-test-repo.sh
```

### 1.3 Settings configuration

Open the settings popup (Ctrl+,) and confirm:

- **Anthropic API key**: Set (if you want to exercise §5 token
  accounting end-to-end). If unset, §5 just shows "no events".
- **`send_terminal_context`**: On if you want block summary
  to fire (which is what populates the token log).
- **`pricing_input_per_million`**: Set to a value matching
  Claude pricing as of your test date (e.g. $3.00 for Sonnet,
  $0.80 for Haiku). Or leave at 0 to skip cost display.
- **`pricing_output_per_million`**: Same, matching your model.
- **`long_command_threshold_secs`**: 10 (default).

You should also have ONE existing scrollback search query in
history (test §8.2's history recall needs prior queries to
recall). Easy way: run `ls` then Ctrl+F, type "src", press
Enter, close. That seeds one history entry.

### 1.4 Tab layout

Sessions assumes:
- Tab 0 = browser
- Tab 1 = terminal

If you've reordered, mentally adjust the Ctrl+1 / Ctrl+2
expectations.

---

## §2 — Scrollback search (v0.1.349)

### 2.1 Open and run a basic search

**Steps:**
1. Switch to terminal tab (Ctrl+2).
2. `cd /tmp/w2q-test-repo`.
3. Run `cat README.md` (or `ls -la` for output).
4. Hit **Ctrl+F**.
5. Find bar appears at top of terminal pane.
6. Type a word that's in the output (e.g. `test` or `feature`).

**Expected:**
- Find bar slides in (~100ms).
- Match counter shows `N of M` as you type.
- Current match highlighted in accent color at ~65% alpha.
- Other matches visible at 25% alpha (the v0.1.350 all-matches
  layer).

**Notes:** _____________________

### 2.2 Navigate matches

**Steps:**
1. With the find bar open, press **Enter** (or F3).
2. Press **Shift+Enter** (or Shift+F3).

**Expected:**
- Enter moves to next match (newer); counter updates "2 of M".
- Shift+Enter moves to previous match.
- Each navigation scrolls the terminal so the current match
  is visible.

**Notes:** _____________________

### 2.3 Close find bar

**Steps:**
1. Press **Escape**.

**Expected:**
- Bar collapses (height animates to 0).
- All highlights clear.
- Focus returns to terminal so typing goes to the shell.

**Notes:** _____________________

---

## §3 — Long-running command notifications (v0.1.350)

### 3.1 Notification fires when defocused

**Steps:**
1. On terminal tab in `/tmp/w2q-test-repo`.
2. Run `sleep 12 && echo done`.
3. **Immediately** switch to browser tab (Ctrl+1).
4. Wait ~12s.

**Expected:**
- Toast appears: `Finished (12s): sleep 12 && echo done`.
- Toast auto-dismisses after a few seconds.

**Notes:** _____________________

### 3.2 NO notification when staying on terminal

**Steps:**
1. Switch back to terminal tab.
2. Run `sleep 12 && echo done` again.
3. **Stay** on the terminal tab the whole time.

**Expected:**
- NO toast.
- The user is watching the prompt return live — no ping needed.

**Notes:** _____________________

### 3.3 Below-threshold commands don't fire

**Steps:**
1. Run `sleep 3 && echo quick`.
2. Switch tabs immediately.
3. Wait for it to finish.

**Expected:**
- NO toast (below the 10s default threshold).

**Notes:** _____________________

### 3.4 All-matches highlight (v0.1.350 follow-up to §2)

**Steps:**
1. Back on terminal tab.
2. Run `cargo --help 2>&1 | head -100` or any command with
   plenty of repeated words.
3. Ctrl+F, type `the` (or any common word).

**Expected:**
- Multiple subtle highlights across the visible viewport at
  25% alpha.
- ONE brighter highlight (the current match) at 65% alpha.
- Scrolling: highlights track the viewport correctly.

**Notes:** _____________________

---

## §4 — Git status badge (v0.1.351)

### 4.1 Badge appears on cwd into repo

**Steps:**
1. In a non-git directory: `cd /tmp`.
2. Verify no badge above the find bar / terminal area.
3. `cd /tmp/w2q-test-repo`.
4. Wait ~200ms (async refresh).

**Expected:**
- Badge appears with `main ●` (because setup script leaves
  the repo dirty).
- Branch icon glyph in amber (since dirty).

**Notes:** _____________________

### 4.2 Badge updates after git operations

**Steps:**
1. `git stash` (stashes the dirty changes).
2. Wait for refresh.

**Expected:**
- Badge updates to `main` (no ● — clean now).
- Icon turns from amber to accent color.

### 4.3 Branch switch

**Steps:**
1. `git checkout feature/bar` (the branch with 2 extra
   commits vs main).
2. Wait for refresh.

**Expected:**
- Badge updates to `feature/bar ⇡2` (or similar if upstream
  is configured) — OR just `feature/bar` if no upstream.

Setup script doesn't configure remotes so likely no upstream.
That's fine.

**Notes:** _____________________

### 4.4 Detached HEAD

**Steps:**
1. `git rev-parse HEAD~1` — copy the short hash.
2. `git checkout <that-hash>`.

**Expected:**
- Badge shows `(<hash>) detached`.
- After test, `git checkout main` returns to normal state.

**Notes:** _____________________

### 4.5 Tap-to-copy

**Steps:**
1. With badge visible (any branch).
2. Click the badge.

**Expected:**
- Toast: `Copied: <branch-name>`.
- Pasting into another field gives the branch name.

**Notes:** _____________________

### 4.6 Badge disappears on non-repo cwd

**Steps:**
1. `cd /tmp` (or anywhere not in a git repo).
2. Wait.

**Expected:**
- Badge collapses to 0 height (no clutter).

**Notes:** _____________________

---

## §5 — Token accounting (v0.1.352)

Requires Anthropic key configured AND `send_terminal_context`
enabled in settings.

### 5.1 Generate some events

**Steps:**
1. Make sure log_summary is on (settings popup → toggle).
2. Run a few commands that produce output: `ls -la`,
   `cat /etc/os-release`, `find /tmp -maxdepth 2 -type d 2>/dev/null`.
3. Each block should trigger a Claude summary call (visible
   via Activity indicator or model logs).

### 5.2 View the man page

**Steps:**
1. Open settings popup (Ctrl+,).
2. Scroll to "Token cost estimation".
3. Click **"View token usage page →"**.

**Expected:**
- Browser tab navigates to a rendered markdown page.
- Shows: totals (input, output, cached), per-op breakdown
  (`block_summary` if log_summary was active), and "Top 5
  spikes" with timestamps.
- If pricing fields are set: "Estimated cost: $0.XXXX" lines.
- If pricing fields are 0: "set $/1M token prices in Settings
  to see this" hint.

**Notes:** _____________________

### 5.3 Empty state

**Steps:**
1. Delete the log: `rm ~/.config/web2qml/token_log.jsonl`.
2. Navigate to `web2qml://stats/tokens` again.

**Expected:**
- "No events recorded yet. Run some commands…" placeholder.

**Notes:** _____________________

---

## §6 — Keybinding audit + ? overlay (v0.1.353)

### 6.1 Open overlay via F1

**Steps:**
1. Press **F1**.

**Expected:**
- Modal popup appears centered.
- Title "Keyboard shortcuts" + "press Esc to close" hint.
- Categories listed: Tabs & Navigation, Editing, Terminal,
  Voice & AI, Window, Help.
- Each chord in a monospace pill on the left, description on
  the right.
- Esc closes.

### 6.2 Other open paths

**Steps:**
1. Try **Ctrl+/** — same overlay.
2. Try **Ctrl+?** — same.

**Expected:** All three open the same overlay.

**Notes:** _____________________

### 6.3 Font size shortcuts

**Steps:**
1. With overlay closed, hit **Ctrl+=**.
2. Hit **Ctrl+-** twice.
3. Hit **Ctrl+0**.

**Expected:**
- Ctrl+= grows font (+1 per press).
- Ctrl+- shrinks.
- Ctrl+0 resets to 16.
- Both browser AND terminal tab respond.

**Notes:** _____________________

### 6.4 Fullscreen + browser tabs

**Steps:**
1. **F11** — fullscreen.
2. **F11** again — back to windowed.
3. On browser tab: **Ctrl+T** — new tab opens at current URL.
4. **Ctrl+W** — close current browser tab.

**Expected:** Each works on its respective tab.

**Notes:** _____________________

### 6.5 Settings + palette

**Steps:**
1. **Ctrl+,** — settings popup opens.
2. Esc to close.
3. On browser tab: **Ctrl+P** — palette opens.

**Notes:** _____________________

---

## §7 — Voice "what branch" + chord registry lint (v0.1.354)

### 7.1 Voice "what branch am I on"

**Steps:**
1. In `/tmp/w2q-test-repo` (any branch).
2. Voice: "what branch am I on".

**Expected:**
- Picker shows a `[git branch]` candidate.
- Confirming surfaces a toast: `On main ●` (or current
  branch + state).

### 7.2 Variants

**Steps:**
1. Try: "current branch", "git branch", "show my git status".

**Expected:** All route to the same toast.

**Notes:** _____________________

### 7.3 Non-repo response

**Steps:**
1. `cd /tmp` (non-repo).
2. Voice: "what branch am I on".

**Expected:** Toast `Not in a git repository`.

**Notes:** _____________________

### 7.4 Chord registry lint (developer check)

**Steps:**
1. From the repo root: `python3 scripts/qml_lint.py`.

**Expected:**
- `qml_lint: 3 file(s) clean`.
- 4 checks including chord-registry sync.

If you've added a Shortcut without a registry entry, this
will fail. That's intentional. Fix by adding the registry
entry, or marking with `// chord-registry-skip` on the line
above the Shortcut.

**Notes:** _____________________

---

## §8 — Find bar polish (v0.1.355)

### 8.1 Case-sensitivity toggle

**Steps:**
1. On terminal tab, paste a line like `Foo foo FOO`.
2. Ctrl+F, type `foo`.
3. Note match count (should be 3, case-insensitive).
4. Click **Aa** button in the find bar.

**Expected:**
- Aa fills with accent color.
- Match count drops to 1 (only lowercase `foo` matches).
- Click Aa again to toggle off; count returns to 3.

**Notes:** _____________________

### 8.2 Search history (Up/Down)

Pre-condition: at least one prior closed search exists.

**Steps:**
1. Ctrl+F (open find bar).
2. Don't type anything yet — press **Up arrow**.

**Expected:**
- Most recent prior search appears in the field.
- Press Up again — older entry (if any).
- Press Down — newer.
- Press Down past newest — your in-progress text (empty)
  returns.

**Notes:** _____________________

### 8.3 Voice "switch to branch X"

**Steps:**
1. On terminal in `/tmp/w2q-test-repo`.
2. Voice: "switch to feature/foo".

**Expected:**
- Picker shows a SINGLE candidate (branch exists locally) at
  ~96% confidence: `git checkout feature/foo`.
- Confirm runs the command.

### 8.4 Voice "switch to nonexistent branch"

**Steps:**
1. Voice: "switch to nonexistent-xyz".

**Expected** (v0.1.356 pre-validation):
- Picker shows TWO candidates:
  - **PRIMARY** (~91%): `git checkout -b nonexistent-xyz` —
    reason "create new branch 'nonexistent-xyz'"
  - **Secondary** (~85%): `git checkout nonexistent-xyz` —
    reason "checkout (branch not found locally)"

**Notes:** _____________________

### 8.5 Tab-switch utterances NOT hijacked

**Steps:**
1. Voice: "switch to terminal".

**Expected:**
- Routes to the existing tab-switch intent (not git
  checkout). Stop-words filter doing its job.

**Notes:** _____________________

---

## §9 — Smart git command parsing (v0.1.356)

All in `/tmp/w2q-test-repo`.

### 9.1 Read-only verbs

For each phrase below, voice it and confirm the candidate
matches the expected command:

| Phrase | Expected command |
|--------|------------------|
| "show me the diff" | `git diff` |
| "show staged diff" | `git diff --staged` |
| "what changed" | `git status` |
| "show the log" | `git log --oneline -20` |
| "show me the last commit" | `git show HEAD` |
| "list branches" | `git branch -a` |

**Expected:** All surface SINGLE candidates at 0.92-0.95. No
amber border (read-only).

**Notes:** _____________________

### 9.2 Mutating verbs (with destructive signaling — §11)

| Phrase | Expected command | Border? |
|--------|------------------|---------|
| "stash my changes" | `git stash` | amber |
| "pop the stash" | `git stash pop` | amber |
| "undo my last commit" | `git reset --soft HEAD~1` | amber |
| "create a new branch test-XYZ" | `git checkout -b test-XYZ` | amber |

**Expected:**
- Each shows an AMBER border on the candidate row.
- A `⚠` warning glyph rectangle between the score pill and
  the description.
- Hovering the glyph shows tooltip "Destructive — mutates
  state".

**Notes:** _____________________

---

## §10 — More git verbs (v0.1.357)

### 10.1 Amend (no message change)

**Steps:**
1. `echo "more" >> README.md` (dirty the tree).
2. `git add README.md`.
3. Voice: "amend last commit".

**Expected:**
- Candidate: `git commit --amend --no-edit` at 0.92.
- **Amber border** (it's mutating).

### 10.2 Amend with new message

**Steps:**
1. Voice: "amend with new message".

**Expected:**
- Candidate: `git commit --amend` (no `--no-edit`).
- Amber border.
- On confirm, $EDITOR opens in the shell.

### 10.3 Blame

**Steps:**
1. Voice: "show me the blame for README.md".
   OR: "blame README.md".

**Expected:**
- Candidate: `git blame README.md`.
- NO amber border (read-only).

**Notes:** _____________________

### 10.4 Log search

**Steps:**
1. Voice: "show commits about initial".

**Expected:**
- Candidate: `git log --all --grep="initial" --oneline -i`.
- Shell escaping correct (the `"initial"` part inside quotes).

**Notes:** _____________________

### 10.5 File history

**Steps:**
1. Voice: "show history of README.md".
   OR: "show the history of README.md".

**Expected:**
- Candidate: `git log --follow --oneline -- README.md`.
- NOT the generic `git log --oneline -20` (this verifies the
  v0.1.357 ordering fix where arg-extraction runs before the
  contains-list).

**Notes:** _____________________

### 10.6 Bare "show the log" still works

**Steps:**
1. Voice: "show the log" (no arg).

**Expected:**
- Falls through arg-extraction prefixes (none match because
  there's no following argument) and lands on the contains-
  list at `git log --oneline -20`.

**Notes:** _____________________

---

## §11 — Visual destructive-op signaling (v0.1.358)

This was tested implicitly in §9.2 and §10.1/10.2. Two
additional checks:

### 11.1 Click target not affected

**Steps:**
1. Voice "stash my changes" → picker shows mutating candidate.
2. Click on the **warning glyph** rectangle (not the candidate
   row's empty space).

**Expected:**
- Candidate still confirms (glyph doesn't intercept the
  click).

**Notes:** _____________________

### 11.2 Active highlight takes precedence

**Steps:**
1. Voice a phrase that surfaces 2+ candidates including a
   mutating one (e.g. "switch to nonexistent-xyz" → both
   `checkout -b` (mutating) and `checkout` (mutating but for
   different reason)).
2. Observe the picker.

**Expected:**
- The top candidate (currently active) gets the bright accent
  highlight.
- Lower candidates show the amber rest-border AND warning
  glyph.
- The accent on the active one still dominates visually.

**Notes:** _____________________

---

## §11.5 — Spoken-name onboarding step (v0.1.359, PC2a)

This section requires a **fresh-install simulation** because the
onboarding flow only fires for new speakers.

### Setup

```bash
# Back up current state if you have a real profile you don't
# want to lose:
mv ~/.config/web2qml/speakers.toml ~/.config/web2qml/speakers.toml.bak
mv ~/.config/web2qml/onboarding_seen.flag \
   ~/.config/web2qml/onboarding_seen.flag.bak 2>/dev/null

# Confirm no spoken-name files exist yet for the test user:
ls ~/.config/web2qml/corpus/ 2>/dev/null
```

Launch web2qml. The onboarding flow should auto-trigger.

### 11.5.1 Happy path — capture and save

**Steps:**
1. Read the pangram aloud at normal pace. Wait for capture.
2. speakerRegistrationPopup appears with color swatch + name field.
3. Type a name (e.g. "TestUser").
4. Tap **Save**.

**Expected:**
- Popup does NOT close. Instead, header changes to "Now say
  your name" and content swaps to a single big mic button +
  status text "Tap the mic and say your name once."
- A "Skip" button is visible on the bottom-right.

**Notes:** _____________________

5. Tap the mic button.

**Expected:**
- Mic turns red and PULSES (scale animation, ~500ms cycle).
- Status text changes to "Listening… say your name."
- Skip button text changes to "Cancel".

6. Say your name clearly: "test user" (or whatever you typed).
7. Wait for silence-detection to end the recording (~1s after
   you stop speaking).

**Expected:**
- Mic turns green with a ✓ glyph.
- Status text: "Got it. Saved to your corpus."
- After ~1.5s, popup auto-closes.

**Notes:** _____________________

8. Verify on disk:

```bash
ls ~/.config/web2qml/corpus/TestUser/onboarding/
# Expected: spoken_name.wav  spoken_name.meta.json
cat ~/.config/web2qml/corpus/TestUser/onboarding/spoken_name.meta.json
# Expected JSON with typed_name, speaker_id, sample_rate, samples, ts
```

### 11.5.2 Skip path

**Setup:** restore the speakers.toml and flag from backup
(so the next launch triggers onboarding again), then run
through 11.5.1 steps 1-4 but...

**Steps:**
1. At step 1 (mic step), tap **Skip** WITHOUT recording.

**Expected:**
- Popup closes immediately.
- No `onboarding/` directory created for this speaker (or, if
  it existed before, no `spoken_name.*` files written THIS time).

```bash
ls ~/.config/web2qml/corpus/TestUser/onboarding/ 2>/dev/null
# Expected: empty or missing
```

**Notes:** _____________________

### 11.5.3 Cancel mid-recording

**Steps:**
1. Repeat through to step 1 (mic), tap mic to start recording.
2. While the mic is pulsing red and status says "Listening…",
   tap the **Cancel** button (the same button as Skip, just
   relabeled while recording).

**Expected:**
- Recording stops cleanly (voice_cancel called).
- Popup closes.
- No spoken_name files written.

**Notes:** _____________________

### 11.5.4 Retry from error

This one is hard to trigger deliberately — you'd need to deny
mic permission or kill the voice daemon mid-capture. If you can
reproduce an error path:

**Expected:**
- Mic shows ⚠ glyph in amber.
- Status: "Couldn't capture. Tap to try again, or Skip."
- Tapping the mic restarts the capture from scratch.

**Notes:** _____________________

### Cleanup

```bash
# Restore real state:
mv ~/.config/web2qml/speakers.toml.bak ~/.config/web2qml/speakers.toml 2>/dev/null
mv ~/.config/web2qml/onboarding_seen.flag.bak \
   ~/.config/web2qml/onboarding_seen.flag 2>/dev/null
# Remove the test user's corpus if you want:
rm -rf ~/.config/web2qml/corpus/TestUser
```

---

## §11.6 — More git verbs round 1 (v0.1.360)

Same test repo as §9 / §10. In `/tmp/w2q-test-repo`.

### 11.6.1 Read-only voice verbs

For each phrase, voice it and confirm the surfaced command:

| Phrase | Expected command | Border? |
|--------|------------------|---------|
| "show me the reflog" | `git reflog --date=short -20` | none |
| "show tags" | `git tag --list --sort=-version:refname` | none |
| "show remotes" | `git remote -v` | none |
| "what's conflicted" | `git diff --name-only --diff-filter=U` | none |

**Notes:** _____________________

### 11.6.2 Fetch / pull (different destructive signaling)

| Phrase | Expected command | Border? |
|--------|------------------|---------|
| "fetch from origin" | `git fetch --all` | **none** (just downloads refs) |
| "git pull" | `git pull` | **amber** (can trigger conflicts) |

**Expected:** fetch surfaces clean; pull surfaces with the
v0.1.358 amber border + ⚠ glyph.

**Notes:** _____________________

### 11.6.3 Tag operations

**Steps:**
1. Voice: "create tag v0.0.1-test"

**Expected:**
- Candidate: `git tag v0.0.1-test` at 0.92.
- NO amber border (trivially reversible).
- Confirm runs. Verify in terminal: `git tag --list` shows it.

2. Voice: "delete tag v0.0.1-test"

**Expected:**
- Candidate: `git tag -d v0.0.1-test` at 0.91.
- **AMBER border** (delete is destructive).
- Confirm runs.

**Notes:** _____________________

### 11.6.4 Cherry-pick hash detection

**Steps:**
1. Get a real hash: `git log --oneline | tail -1 | awk '{print $1}'`
   (copy the 7-char hash).
2. Voice: "cherry pick &lt;hash&gt;" where you say the hash
   clearly (most STT engines render 7-char hex as one token if
   the user pronounces it cleanly).

**Expected (clean hash):**
- Candidate: `git cherry-pick &lt;hash&gt;` at 0.90.
- **AMBER border** (mutating).

**Expected (garbled hash, e.g. "cherry pick a b c 1 2 3"):**
- NO candidate (hash-shape check rejects spaces).
- Falls through to other handlers.

**Expected ("cherry pick the old one"):**
- NO candidate (non-hash arg rejected).

**Notes:** _____________________

---

## §11.7 — Filesystem find mode (v0.1.361)

Phase 1.5 completion — the find bar gains a second mode for
searching files in cwd rather than terminal scrollback.

Pre-condition: in `/tmp/w2q-test-repo` or any directory with
multiple files. Optionally `dnf install ripgrep` for the fast
path (otherwise grep fallback runs).

### 11.7.1 Mode toggle

**Steps:**
1. Switch to terminal tab.
2. **Ctrl+F** — find bar opens in scrollback mode.

**Expected:**
- Mode toggle button shows 📜 (scroll glyph).
- Placeholder text: "Search scrollback".

3. Click the 📜 toggle.

**Expected:**
- Glyph changes to 📁 (folder).
- Toggle background changes to accent color.
- Placeholder text changes to "Find in files (cwd)".
- Tooltip on hover reads "Files (click for scrollback)".

**Notes:** _____________________

### 11.7.2 Query and results

**Steps:**
1. In files mode with cursor in the field, type a query that
   exists in multiple files, e.g. `commit` or `branch`.

**Expected (after ~150ms):**
- "Searching files…" italic text appears briefly under the bar.
- Results panel slides in (animated, ~100ms).
- Each row: `path:line` in accent color, then matched text.
- Match counter shows "1 of N".
- First result auto-selected (lighter accent highlight).

**Notes:** _____________________

### 11.7.3 Up/Down navigation

**Steps:**
1. Press **Down**.

**Expected:**
- Selection moves to row 2. View scrolls if needed to keep
  selected row visible.
- Match counter updates to "2 of N".

2. Press **Up** several times — selection moves up, wraps to
   bottom at row 1.

**Notes:** _____________________

### 11.7.4 Commit to PTY (Enter)

**Steps:**
1. With a row selected, press **Enter**.

**Expected:**
- Find bar closes.
- Terminal regains focus.
- The selected file's path (shell-quoted if needed) is pasted
  at the PTY prompt. NO newline — you can prepend a command.

2. At the prompt, type `cat ` (with trailing space) before the
   pasted path, then Enter.

**Expected:**
- The file's contents print. The path quoting was correct.

**Notes:** _____________________

### 11.7.5 Click to commit

**Steps:**
1. Re-open Ctrl+F, files mode, run a search.
2. Click any result row directly (not Up-arrow + Enter).

**Expected:**
- Same as Enter: bar closes, path pasted.

**Notes:** _____________________

### 11.7.6 Mode switch preserves nothing

**Steps:**
1. Files mode, type a query, see results.
2. Click 📁 to flip back to scrollback (📜).

**Expected:**
- Results panel disappears (height animates to 0).
- Search re-runs in scrollback mode. If the query has hits in
  visible scrollback they highlight; otherwise "no matches".
- Up/Down now walks SEARCH HISTORY (the original behavior).

**Notes:** _____________________

### 11.7.7 Quick-typing doesn't pile up workers

**Steps:**
1. Files mode, type rapidly: `f`, `fo`, `foo`, `foob`, `fooba`,
   `foobar`. (As fast as you can.)

**Expected:**
- No flicker, no errors. The most recent query's results land.
- No backend crashes / lockups.

(The implementation cancels in-flight searches on every
keystroke that exceeds the 2-char threshold, then kicks off a
new one. Brief 150ms gaps between cancel and start are normal.)

**Notes:** _____________________

### 11.7.8 Empty / short query

**Steps:**
1. Files mode, type just `f` (one character).

**Expected:**
- Results panel hidden.
- Match counter: "type more…"
- No subprocess work happens (no logging).

2. Clear the field.

**Expected:**
- Counter clears to empty.
- No background activity.

**Notes:** _____________________

### 11.7.9 Backend fallback (no ripgrep)

Only run this if you can simulate `rg` being missing:

```bash
# Temporarily mask rg:
PATH=/usr/bin web2qml ...  # if rg isn't in /usr/bin
```

**Expected:**
- Searches still work (grep fallback).
- Possibly slower for large trees.
- No errors in the log; `[fs-find] poll → N matches` still
  fires.

**Notes:** _____________________

---

## §11.8 — SSH session detection (v0.1.363)

Foundation for the borrow-my-tools arc. Tests verify the UI
adaptations fire correctly when entering / leaving an SSH-style
session.

Pre-condition: you need somewhere to SSH to. Options:
- A real remote box you have credentials for
- `ssh localhost` works if `sshd` is running locally
- `docker run -it alpine sh` is the easiest with no extra setup

### 11.8.1 Entering SSH

**Steps:**
1. In a git repo (so the git badge is visible). Confirm
   gitBadge shows "main" or similar at the top of the terminal.
2. Run `ssh user@host` (or `docker run -it alpine sh`, or
   `ssh localhost` — pick what's available).

**Expected (within ~500ms of entering the remote shell):**
- gitBadge collapses (height animates to 0).
- A new `sshBadge` Rectangle appears in its place: dark
  background, "🔗 ssh session — local features paused" on
  the left, "git badge · fs find · long-cmd toast" hint on
  the right.

**Notes:** _____________________

### 11.8.2 Find bar in files mode refuses

**Steps:**
1. While still in the SSH session.
2. Press **Ctrl+F** to open the find bar.
3. Click the 📜 toggle to switch to files mode (📁).
4. Type a query.

**Expected:**
- Match counter shows "local only".
- No results panel appears.
- No subprocess work happens (no `[fs-find]` log lines).

5. Click the 📁 toggle back to scrollback mode.
6. Type a query.

**Expected:**
- Scrollback search works normally — searches the remote
  shell's visible output.

**Notes:** _____________________

### 11.8.3 Long-command toast prefixes with (remote)

**Steps:**
1. While in SSH, run a long-running command on the REMOTE:
   `sleep 12 && echo done`.
2. Immediately switch to browser tab.
3. Wait. (The sleep finishes after 12s; you'll see the toast
   when the LOCAL ssh process closes, which is when you `exit`
   the remote shell.)

Actually — the toast fires on LOCAL block close, which only
happens when the WHOLE ssh session ends. So a better test:

**Better test:**
1. In SSH session.
2. Run something quick (`ls`) to make sure remote shell is
   responsive.
3. Type `exit` (or Ctrl+D) to end the SSH session.

**Expected when local prompt returns:**
- Toast appears: `Finished (remote, Xs): ssh user@host`
  where X is the total ssh session duration.
- The "(remote, …)" prefix is present.

**Notes:** _____________________

### 11.8.4 Leaving SSH restores UI

**Steps:**
1. After exiting SSH, confirm sshBadge collapses (animates
   back to height 0).
2. gitBadge returns (if you're in a git repo).
3. fs_find files mode works again.

**Notes:** _____________________

### 11.8.5 Recognized opener variations

Test that these all trigger the SSH badge:
- `ssh user@host`
- `mosh user@host` (if you have mosh installed)
- `docker run -it alpine sh`
- `docker exec -it <container> bash` (need a running container)
- `kubectl exec -it <pod> -- bash` (need kubectl + cluster)

And these should NOT trigger:
- `ssh-keygen -t ed25519`
- `sshfs user@host:/ /mnt`
- `docker run alpine echo hi` (no -it; one-shot)
- `vim ssh-config.txt`

**Notes:** _____________________

### 11.8.6 Heuristic limits (acknowledged)

These cases are documented limits, NOT bugs:

- Shell aliases for ssh: not detected
- Wrapper scripts that run ssh internally: not detected
- Nested ssh-from-inside-ssh: still reports "in SSH" but
  can't distinguish layers
- tmux/screen reattach: not a session in our model

If you find these matter for your workflow, file failure
notes — the borrow-my-tools follow-up might address some
via more sophisticated detection (e.g. watching for
$SSH_TTY in remote prompts).

**Notes:** _____________________

---

## §11.9 — Token-stats time-window filters (v0.1.364)

Pre-condition: Claude API key configured, `send_terminal_context`
on, `log_summary` on so we generate token events. You'll want
some events both inside and outside the current session for the
filters to be meaningful.

### Setup — generate sample data

If `~/.config/web2qml/token_log.jsonl` is empty or very small:

```bash
# Quickly generate some events by running commands that trigger
# block summaries. Each of these takes ~1-3 seconds:
ls -la
cat /etc/os-release
find /tmp -maxdepth 2 -type d 2>/dev/null
ps aux | head -20
df -h
```

Wait ~30s for the summary worker to drain its queue. Verify:

```bash
wc -l ~/.config/web2qml/token_log.jsonl
# Expected: 5+ lines
```

### 11.9.1 Default view (All-time)

**Steps:**
1. Browser tab → navigate to `web2qml://stats/tokens` (or click
   the "View token usage page →" button in the settings popup).

**Expected:**
- Page opens with a tab nav at the top reading:
  `**Window**: Session · Today · Last 7 days · **All-time**`
- "All-time" is bolded plain text; the other three are clickable
  links.
- "## Totals (all-time)" section follows with all events
  counted.

**Notes:** _____________________

### 11.9.2 Tab switching — Today

**Steps:**
1. Click "Today".

**Expected:**
- URL changes to `web2qml://stats/tokens?window=today`.
- Page re-renders with "**Today**" bolded; "All-time" now a link.
- Header now reads "## Totals (today)".
- Event count is ≤ the all-time count (only today's events).
- If you ran the setup commands recently, "Today" should show
  the same events as all-time.

**Notes:** _____________________

### 11.9.3 Session window

**Steps:**
1. Click "Session".

**Expected:**
- URL: `web2qml://stats/tokens?window=session`.
- Header: "## Totals (session)".
- Event count = events since the current web2qml process
  started (whenever you last launched the app).
- If you launched right before running the setup commands,
  this should match the setup count.

**Notes:** _____________________

### 11.9.4 Last 7 days

**Steps:**
1. Click "Last 7 days".

**Expected:**
- URL: `web2qml://stats/tokens?window=7d`.
- Header: "## Totals (last 7 days)".
- Rolling 168-hour window — events from 7d-1s ago through now.
- For a fresh install, this == all-time.

**Notes:** _____________________

### 11.9.5 Empty window message

If you have a fresh install OR can simulate by running web2qml
right after `rm ~/.config/web2qml/token_log.jsonl`:

**Steps:**
1. Navigate to the stats page.
2. Click "Session" (before generating any events this session).

**Expected:**
- Tab nav still visible.
- Below: "_No events recorded in window **Session**. Run some
  commands that invoke Claude (e.g. enable log_summary and let
  a block summarize) to populate this page._"
- Page does NOT show empty stats sections.

**Notes:** _____________________

### 11.9.6 Cost estimate adjusts to window

Pre-condition: pricing fields set in settings (e.g. $3.00 input,
$15.00 output for Sonnet).

**Steps:**
1. View "All-time" tab — note the estimated cost.
2. Click "Session" — note the estimated cost.
3. Click "Today" — note the estimated cost.

**Expected:**
- Each window's cost is scoped to that window's events.
- Session ≤ Today ≤ Last 7 days ≤ All-time (in event count,
  and usually in cost — assuming you're using web2qml regularly
  this week).

**Notes:** _____________________

### 11.9.7 Browser back-button works

**Steps:**
1. From All-time tab → click Today → click Session → press
   browser back-button twice.

**Expected:**
- Back once returns to Today tab.
- Back twice returns to All-time tab.
- Each navigation is independent (the URL is the state).

**Notes:** _____________________

### 11.9.8 Per-op breakdown filters too

**Steps:**
1. On any window tab, scroll down to "## Per-operation breakdown".

**Expected:**
- The op-level stats (count, median, p99, per-op cost) reflect
  ONLY the events in the current window.
- Switch tabs — per-op breakdown updates correspondingly.

**Notes:** _____________________

---

## §11.10 — Op-tag refinement + reset button (v0.1.365)

### 11.10.1 Distinct op tags appear in breakdown

Pre-condition: Claude key configured. Generate events from
multiple surfaces:

**Steps:**
1. Reset the log so we start fresh:
   ```bash
   rm ~/.config/web2qml/token_log.jsonl 2>/dev/null
   ```
   (or use the reset button if it's testable — §11.10.4 below)
2. Trigger a `chat` event: open the chat surface in the app
   and send any short message.
3. Trigger a `context_question`: enable `send_terminal_context`
   in settings, then ask "what's on my screen?" via the
   context-question voice / button surface (whichever exists
   in your build).
4. Trigger an `llm_query`: free-form prompt into the LLM input.
5. Open `web2qml://stats/tokens` → scroll to "Per-operation
   breakdown".

**Expected:**
- Multiple distinct sections, one per op:
  - `### chat`
  - `### context_question` (or
    `### context_question_unenriched` if you ran with
    send_terminal_context OFF)
  - `### llm_query` or `### llm_query_async`
- Each section has its own count, min/median/max/p99, cost.

**Notes:** _____________________

### 11.10.2 Tag distinguishes enriched vs unenriched context-questions

**Steps:**
1. In settings, DISABLE `send_terminal_context`.
2. Ask a context-question. Note the op tag in the resulting
   event.
3. Re-enable `send_terminal_context`.
4. Ask another context-question. Note the op tag.

**Expected:**
- First event: `op: "context_question_unenriched"`
- Second event: `op: "context_question"`
- Power-user value: you can see in stats "I spent $X this week
  on context-enriched questions" vs "$Y on bare-question
  callouts."

**Notes:** _____________________

### 11.10.3 Reset link visible on stats page

**Steps:**
1. Open `web2qml://stats/tokens`.
2. Scroll to the bottom of the page.

**Expected:**
- A horizontal rule (`---`), then a paragraph:
  "[Reset log…](web2qml://stats/tokens?action=reset_prompt) —
  moves the JSONL to `token_log.jsonl.bak` and starts fresh."

**Notes:** _____________________

### 11.10.4 Reset confirmation flow

**Steps:**
1. Click "Reset log…" link.

**Expected:**
- Page navigates to confirmation page titled "Reset token log?"
- Body explains what will happen.
- Code block shows the recovery `mv` command.
- Two links at the bottom: **Yes, reset the log** (bold link)
  and **Cancel** (link back to the main page).

2. Click **Cancel**.

**Expected:**
- Returns to the main stats page. Nothing has changed; log
  is still intact.

**Notes:** _____________________

### 11.10.5 Actual reset (and undo)

**Steps:**
1. Note `wc -l ~/.config/web2qml/token_log.jsonl` (some non-
   zero count).
2. Click "Reset log…" → "Yes, reset the log".

**Expected:**
- Page renders "# Token log reset" with confirmation text and
  a back link.
- On disk:
  ```bash
  ls ~/.config/web2qml/token_log.jsonl*
  # Expected: only token_log.jsonl.bak (token_log.jsonl absent)
  wc -l ~/.config/web2qml/token_log.jsonl.bak
  # Expected: matches the pre-reset count
  ```

3. Click the "Back to token usage page" link.

**Expected:**
- Main stats page renders with "_No events recorded yet_"
  empty-state message.
- Token log file is absent (will be recreated on next Claude
  call).

4. Recover via the documented command:
   ```bash
   mv ~/.config/web2qml/token_log.jsonl.bak \
      ~/.config/web2qml/token_log.jsonl
   ```
5. Refresh the stats page.

**Expected:**
- Full pre-reset stats restored.

**Notes:** _____________________

### 11.10.6 Second reset overwrites prior .bak

**Steps:**
1. With events present, click "Reset log…" → "Yes". (First reset.)
2. Trigger some new Claude calls so a fresh `token_log.jsonl`
   exists.
3. Click "Reset log…" → "Yes" again. (Second reset.)

**Expected:**
- `token_log.jsonl.bak` now contains the events from BETWEEN
  the two resets (not the events from before the first reset).
- The original pre-first-reset events are GONE — no recovery
  past the most recent reset.

This is acknowledged behavior (single-level undo). If users
need archival history, they should `cp` the file before
resetting.

**Notes:** _____________________

---

## §11.11 — Gemini archive viewer (v0.1.367)

Pre-condition: a Gemini client installed for populating the
archive. `gemget` is the simplest:

```bash
go install github.com/makew0rld/gemget@latest
```

Or via your distro's package manager if available.

### 11.11.1 Launcher page (empty archive)

**Steps:**
1. Open `web2qml://gemini`.

**Expected:**
- Page renders with title "Gemini" and an intro paragraph
- Status line mentions "archive-only (v0.1.367)"
- "Your archived capsules" section says "No capsules in your
  archive yet."
- Code block shows the `gemget` invocation
- "Starting points" section lists 6 curated capsules
- "About this surface" explainer at the bottom

**Notes:** _____________________

### 11.11.2 Click a starting-point link (not in archive)

**Steps:**
1. Click the "Project Gemini" link.

**Expected:**
- Page navigates to a "Not in archive" page
- Title reads "# Not in archive"
- Body mentions the specific URL
  (`gemini://geminiprotocol.net/`)
- Code block shows the exact `gemget` command needed,
  including the local path:
  ```
  mkdir -p ~/.config/web2qml/gemini/geminiprotocol.net
  gemget -o ~/.config/web2qml/gemini/geminiprotocol.net/index.gmi gemini://geminiprotocol.net/
  ```
- Footer has "[← Gemini launcher](web2qml://gemini)" link

**Notes:** _____________________

### 11.11.3 Populate the archive

**Steps:**
Run the suggested commands at a terminal:
```bash
cd ~/.config/web2qml/gemini
mkdir -p geminiprotocol.net
gemget -o geminiprotocol.net/index.gmi gemini://geminiprotocol.net/
```

**Expected:**
- `gemget` connects to the capsule, downloads, writes
  `geminiprotocol.net/index.gmi`
- File is non-empty Gemtext

If gemget fails: the capsule may be down (it's a hobbyist
host). Try one of the other starting points, or any active
Gemini capsule you know.

**Notes:** _____________________

### 11.11.4 Re-visit the URL in web2qml

**Steps:**
1. Navigate back to `web2qml://gemini` and click the same
   "Project Gemini" link, OR navigate directly to
   `gemini://geminiprotocol.net/`.

**Expected:**
- Page now renders the capsule's content
- Headings (# / ## / ###) render at the correct sizes
- List items render as bullets
- Quotes render with a left-side mark
- Code blocks (```) render in monospace
- `=>` link lines render as tappable markdown links

**Notes:** _____________________

### 11.11.5 Tap a link in the rendered capsule

**Steps:**
1. Find an outbound Gemini link in the rendered capsule
   (e.g. a link to another capsule).
2. Tap it.

**Expected:**
- Page navigates. If target is in archive, renders it.
  If not, shows the "Not in archive" page with the
  populate command for THAT target's URL.

**Notes:** _____________________

### 11.11.6 Launcher reflects archived hosts

**Steps:**
1. After populating at least one host, return to
   `web2qml://gemini`.

**Expected:**
- "Your archived capsules" section now lists the host
  with page count, e.g. `- geminiprotocol.net — 1 page`
- Tapping the host link navigates to its root capsule

**Notes:** _____________________

### 11.11.7 Archive layout on disk

**Steps:**
1. Inspect the archive directory:
   ```bash
   tree ~/.config/web2qml/gemini/ -L 3
   ```

**Expected:**
- Layout: `~/.config/web2qml/gemini/<host>/<path>.gmi`
- Files have `.gmi` extension (unless source had its own
  extension like `.png`)
- Root URLs (`gemini://host/`) saved as `<host>/index.gmi`

**Notes:** _____________________

### 11.11.8 Path traversal rejection (security)

**Steps:**
1. Try to navigate to a URL with `..` in the path:
   `gemini://example.org/../etc/passwd`

**Expected:**
- Page should NOT escape the archive root
- Shows "Not in archive" page (correct behavior — the
  malformed URL doesn't resolve to a path)

**Notes:** _____________________

### 11.11.9 Reading-flow check

Find an active Gemini capsule with real content (a personal
diary, technical writing, a tilde-space user page). Populate
its index plus a few sub-pages. Navigate the capsule via
link-tapping.

**Expected:**
- The reading experience should feel like reading a
  well-structured static webpage with no JS, no ads, no
  trackers, no layout shifts
- Voice should be able to read the page (existing TTS
  functionality)
- Find-bar (Ctrl+F) should search within the rendered text

This is the qualitative check that the rendering pipeline
serves the kind of content the small-web actually has.

**Notes:** _____________________

---

## §12 — Cross-feature interactions

These exercise multiple shipped features at once.

### 12.1 Voice → smart-git → token-stats

Pre-condition: Claude key configured, `send_terminal_context`
on, log_summary on.

**Steps:**
1. Note current `~/.config/web2qml/token_log.jsonl` line count:
   `wc -l ~/.config/web2qml/token_log.jsonl 2>/dev/null || echo 0`
2. Voice "show me the diff" → confirm → `git diff` runs.
3. Wait for block summary (Claude call) to complete.
4. Recount lines.

**Expected:**
- Line count increased by 1 (the block-summary call was
  logged).
- Navigate to `web2qml://stats/tokens` — most recent entry
  is the diff summary.

### 12.2 Git operations update badge live

**Steps:**
1. Note current badge: e.g. `main ●` (dirty).
2. Voice "stash my changes" → confirm.
3. Wait for badge refresh (~block-close trigger).

**Expected:**
- Badge updates to `main` (clean).
- The whole thing happens via voice from start to finish
  including verification.

### 12.3 Long-cmd notification + token-stats combo

**Steps:**
1. Run a long Claude-using command (any block that fires
   log_summary AND takes >10s).
2. Switch to browser tab during execution.
3. Wait.

**Expected:**
- Toast on completion.
- Token log incremented after summary lands.

### 12.4 Chord overlay accessible via voice

**Steps:**
1. Voice "show keybindings".

**Expected:**
- `[keybindings]` candidate surfaces.
- Confirm → chord overlay opens.

---

## §13 — Cleanup

When done:

```bash
# Test repo
rm -rf /tmp/w2q-test-repo

# OPTIONAL: clear token log if you want to reset cost tracking
# rm ~/.config/web2qml/token_log.jsonl

# OPTIONAL: clear search history is automatic (in-memory only)
```

If you found anything that didn't work, copy the contents of
`templates/failure-note.md` into `docs/todo.md` under the
relevant section (or create a new section) so it doesn't fall
through cracks.

---

## What's NOT covered here

These features exist but aren't in this session because
they're either (a) better tested in unit tests, (b) hard to
test manually without specific setup, or (c) tangential to
the power-user flow:

- Niftyplot graph rendering (v0.1.343) — visual, easy to
  spot-check ad hoc
- Paragraph wrapper performance (v0.1.344) — needs profiling
  setup
- Sidebar-aware layout demotion (v0.1.345) — only fires on
  specific sites
- Paste line-ending normalization (v0.1.346/347) — covered by
  unit tests
- Voice candidate deduplication (v0.1.347) — covered by tests
- Selection scroll sync (v0.1.347) — covered by tests
- Phase 2 workspace foundation — not yet shipped

---

## Updating this session

When new ships land, add a section here (just below the
relevant prior section). Keep the table at the top in sync.

The setup script (`scripts/setup-test-repo.sh`) should stay
small and predictable; if a ship needs new test state, add a
new optional step to the script rather than complicating the
default path.
