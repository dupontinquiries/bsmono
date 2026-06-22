# Multiplexer + SSH voice control (v0.1.703)

Power-user voice: drive zellij/tmux and open ssh sessions hands-free.

## Cores (pure + tested)
- src/zellij_voice.rs (v0.1.634 + v0.1.703 describe_*): translate(phrase) →
  `zellij action …`; describe_tabs/describe_panes summarize query output for
  speaking. 12 tests.
- src/tmux_voice.rs (v0.1.703): translate(phrase) → `tmux …`, same verb
  vocabulary mapped to tmux subcommands (split-window -h/-v, select-pane
  -L/R/U/D, new-window, select-window -t N, resize-pane -Z, etc.). 6 tests.
- src/ssh_voice.rs (v0.1.703): parse(phrase) → SshRequest{alias, run};
  parse_ssh_config(body) → concrete Host entries; resolve(alias, hosts) by
  exact/normalized/prefix; to_command. 7 tests.

## Wiring (qt_methods + QML freeform dispatch)
- zellij_active / tmux_active (ZELLIJ / TMUX env), *_translate → "cmd\tconfirm
  \tquery", *_commands → vocabulary. ssh_connect(phrase) reads ~/.ssh/config,
  resolves, returns the ssh line (bare-host fallback if no config match).
  run_capture(cmd) runs + captures stdout for assisted reads.
- QML "freeform" case: in a mux session, the phrase is tried as zellij then
  tmux; mutations run via pty_write, destructive ones confirm (ConfirmDialog),
  QUERY actions run captured and speak the summary (#1 assisted read).
  "connect to <host>"/"run X on <host>" → ssh (#3). "what can I say"/"zellij
  help"/"tmux commands" opens the vocabulary overlay (#2). Non-mux phrases
  fall through to web search, so non-power-users pay nothing.

## Remaining
A persistent in-terminal hint chip (vs the on-demand help overlay) and ssh
known-host confirmation are possible follow-ups.
