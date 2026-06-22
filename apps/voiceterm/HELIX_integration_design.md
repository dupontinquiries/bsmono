# Helix integration design (draft)

> Status: design draft. Captures the intended approach for "voice
> commands that act on the editor I'm in" before implementation.

## Goal

Make voice commands like:
- "go to definition"
- "find references"
- "rename this"
- "show errors"
- "format file"

work when the user is in helix in the terminal. Not by integrating
INTO helix (we're outside it) but by routing voice intents to the
right helix keystrokes when helix is the foreground command.

## Why helix specifically

User uses helix daily (per their environment). It's the editor most
likely to benefit from voice integration in this codebase. The
techniques here generalize to vim/nvim/emacs but each has its own
keystroke conventions.

helix is also exceptionally well-suited for command routing because
its space-menu (`SPC`) is discoverable and stable: every common LSP
action has a path in the space menu that we can replay verbatim.

## Architecture

```
voice → voice_intent.rs → helix_intent.rs (NEW) → PTY keystrokes
```

`helix_intent.rs` contains:
1. A table of `IntentVerb → KeySequence` for helix
2. A `route_to_helix(intent) -> Option<Vec<u8>>` function that
   returns the keystrokes to send, or None if unmappable

The PTY keystrokes are sent via `pty_write_byte` in our existing
sequence. helix processes them and the user sees the action.

## Intent vocabulary (initial set)

| Voice utterance | Helix keys | Notes |
|-----|-----|-----|
| "go to definition" | `gd` | direct |
| "go to references" | `gr` | direct |
| "go to type" | `gy` | type definition |
| "go to implementation" | `gi` | impl |
| "show diagnostics" / "show errors" | `SPC k` | hover-style |
| "list diagnostics" | `SPC d` | picker |
| "rename this" / "rename" | `SPC r` | rename symbol |
| "format file" / "format" | `SPC f` | LSP format |
| "show hover" / "documentation" | `SPC k` | doc lookup |
| "code action" | `SPC a` | LSP actions |
| "find symbol" | `SPC s` | doc symbols |
| "find symbol in workspace" | `SPC S` | workspace symbols |
| "open file" | `SPC f` | file picker (overlaps format!) |
| "save" | `:w<CR>` | direct |
| "save and quit" | `:wq<CR>` | direct |
| "quit without saving" | `:q!<CR>` | direct |
| "split horizontal" | `SPC w s` | window split |
| "split vertical" | `SPC w v` | window vsplit |
| "next buffer" | `SPC b` | buffer picker |
| "undo" | `u` | direct |
| "redo" | `U` | direct |
| "comment" | `SPC c` | toggle comment |

Note: `SPC f` ambiguity (format vs file picker) — context-dependent
in helix; we'd default to format and let user say "open file" to
disambiguate.

## Mode awareness

helix is a modal editor. Sending `gd` while in insert mode will
type "gd" into the buffer instead of going to definition.

We need to know what mode helix is in. Options:

**A. Always escape first.** Send `Esc` before every command. Safe
but breaks if user is mid-edit (Esc closes their popup). Cost: 1
extra keystroke per command.

**B. Track mode via prompt.** Some helix builds emit the mode in
the OSC title (e.g. `helix [INSERT] - foo.rs`). Parse from there.
Fragile — depends on user's helix config.

**C. Don't track, document the limitation.** Tell user "voice
commands work from normal mode." If they try while in insert mode,
they get garbled text — recoverable via undo.

**Lean: A.** Cheap, robust, predictable. The Esc happens on the user's
behalf as part of the action, transparent to them.

## Key sequence encoding

Some commands use multi-char sequences (`gd`), some use special keys
(`Esc`, `Enter`). Encoding:

```rust
pub enum Key {
    Char(char),
    Esc,
    Enter,
    Tab,
    Space,
    Ctrl(char),
    Alt(char),
}

pub fn keys_to_bytes(keys: &[Key]) -> Vec<u8> {
    let mut out = Vec::new();
    for k in keys {
        match k {
            Key::Char(c) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            Key::Esc => out.push(0x1b),
            Key::Enter => out.push(b'\r'),
            Key::Tab => out.push(b'\t'),
            Key::Space => out.push(b' '),
            Key::Ctrl(c) => out.push(c.to_ascii_uppercase() as u8 - b'@'),
            Key::Alt(c) => { out.push(0x1b); /* then char */ }
        }
    }
    out
}
```

Each table entry becomes a `Vec<Key>` so we can keep the table
declarative.

## Detection: "is helix the foreground command?"

We already have `term_foreground_command` returning the foreground
process name. Check if it's `helix` (or `hx` — the binary name on
some distros).

If yes: route helix-flavored intents through `helix_intent.rs`. If
no: fall back to existing behavior (send the command as a shell
line, etc.)

## Edge cases

**Command line mode (`:`):** `:w<CR>` works if we're in normal mode.
If user is in insert mode and we Esc-then-`:w<CR>`, that's correct.
If they're already in command line mode (`:` typed but not Enter),
our Esc cancels their typing. Acceptable — voice intent is to
save, and we made it happen.

**Picker open:** if user has a picker open (file picker, symbol
picker), Esc closes it. Then our command runs. Probably what user
wants — they said "go to definition" while their picker was open,
they meant to dismiss the picker first.

**Multi-cursor:** helix `,` splits cursors, `&` aligns. We don't
expose these via voice initially — too easy to make it confusing.

**Undo grouping:** helix groups undos by what looks like a logical
edit. Our injected commands would be separate undo points. Fine.

## What we DON'T do

- We don't talk LSP directly. helix handles all LSP communication;
  we just trigger helix's LSP-driven commands.
- We don't read helix state (cursor position, buffer name, etc).
  Just send keystrokes.
- We don't use helix's command line for arbitrary commands. Only
  the curated table.

## Future: per-editor modules

Same pattern works for vim/nvim/emacs:
- `vim_intent.rs`: `gd` is also "go to definition" (close enough)
- `emacs_intent.rs`: `M-.` instead

A small dispatcher checks `term_foreground_command`:
```rust
match fg.as_str() {
    "helix" | "hx" => helix_intent::route(intent),
    "vim" | "nvim" => vim_intent::route(intent),
    "emacs" => emacs_intent::route(intent),
    _ => None,
}
```

## Implementation plan

**Phase 1**: helix_intent.rs with the initial table, dispatcher in
voice_intent.rs that checks foreground command before generic
fallback. Esc-prefix every command. Test with daily use.

**Phase 2**: vim_intent.rs (port the helix mappings to vim conventions).

**Phase 3**: optional mode awareness via OSC title parsing (only if
the always-Esc proves problematic).

**Phase 4**: per-language commands ("format Python file" routes to
`SPC f` in Python files but to `SPC m c f` in markdown for "format
table" — way out of scope until phase 1 lands).

## Open questions

1. **Naming overlaps with our intents.** Voice "save" today might
   be a generic file save. In editor context it should route to
   `:w<CR>`. Resolution: editor-context check happens FIRST. Generic
   intents fall through if editor doesn't claim it.

2. **What if helix isn't running but the user said "go to
   definition"?** Fall back to: tell the user nothing happened, or
   silently do nothing? Lean: brief toast "no editor running."

3. **Cancel during edit.** If voice triggers and user is mid-keystroke,
   our injected bytes interleave with theirs. helix processes byte
   stream sequentially; the user gets a weird result. Mitigation:
   advise user to finish typing before voicing. Or: queue voice
   commands until typing pause detected (much harder, defer).
