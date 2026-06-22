# Failure note template

Copy this when you find something during a test session that
doesn't match the expected outcome. Paste into `docs/todo.md`
under the relevant feature section so it gets triaged.

---

## v0.1.XXX — <feature name> — <one-line summary>

**Session section:** §X.Y (which test step failed)
**Date:** YYYY-MM-DD
**Build version:** 0.1.XXX (from `Cargo.toml` or About panel)

### What I did

1. ...
2. ...

### What I expected

...

### What actually happened

...

### Reproducible?

- [ ] Yes, every time
- [ ] Sometimes
- [ ] Only once

### Workaround (if any)

...

### Suspected area

(Optional — if you have a guess about which module/file, name
it. Helps the next person triaging.)

### Logs

```
(paste any relevant terminal output, errors, etc.)
```

---

## Tips for good failure notes

- Be specific about state: what tab, what cwd, what branch,
  what was running.
- Include the EXACT phrase you voiced if voice-related.
- If the failure is visual, screenshots help — drop into
  `/home/claude/scratch/` and reference by path.
- "It didn't work" isn't enough — describe what DID happen
  instead of what you expected.
