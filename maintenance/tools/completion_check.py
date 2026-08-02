#!/usr/bin/env python3
"""completion_check.py — do the shell completions still match the source?

A completion file is a hand-written copy of a subcommand list, so it rots the
moment someone adds a command. The failure is quiet and one-sided: the new
command works perfectly and simply never appears on tab, so the person who added
it has no reason to notice and the person who misses it assumes it does not
exist.

Checked by extracting the strings the binary actually MATCHES ON — the `"scan"
=> ...` arms — and comparing them against the words the completions offer. That
is the same list by construction, so a mismatch is always a real one.
"""
import os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
APPS = os.path.join(ROOT, 'apps')

def source_commands(path):
    """Subcommands from `"name" => cmd_x(...)` match arms."""
    try: t = open(path, encoding='utf8').read()
    except Exception: return set()
    # Only the TOP-LEVEL dispatch, not every match arm in the file. A --format
    # value like "json" is also a `"json" => ...` arm, and treating it as a
    # subcommand made the checker demand it on tab — a false positive, which is
    # the one failure mode a checker cannot afford: it teaches people to ignore
    # the output, and then the real drift goes unnoticed too.
    #
    # The dispatch is the arm whose body calls cmd_*, which is the convention
    # this binary already follows.
    out = set()
    for m in re.finditer(r'"([a-z][a-z-]*)"\s*(?:\|[^=]*?)?=>\s*cmd_\w+', t):
        out.add(m.group(1))
    return out

def completion_words(path):
    try: t = open(path, encoding='utf8').read()
    except Exception: return set()
    # bash: the -W word list on the subcommand branch
    m = re.search(r'compgen -W "([^"]+)" -- "\$cur"\)\)\s*\n\s*return', t)
    if m: return set(m.group(1).split())
    return set()

def zsh_words(path):
    try: t = open(path, encoding='utf8').read()
    except Exception: return set()
    return set(re.findall(r"^\s*'([a-z][a-z-]*):", t, re.M))

def main():
    problems = 0
    checked = 0
    for app in sorted(os.listdir(APPS)):
        comp = os.path.join(APPS, app, 'completions')
        if not os.path.isdir(comp): continue
        for f in sorted(os.listdir(comp)):
            tool = f[:-len('.bash')] if f.endswith('.bash') else f.lstrip('_')
            src = os.path.join(APPS, app, 'src', 'bin', tool + '.rs')
            if not os.path.isfile(src): continue
            want = source_commands(src)
            if not want: continue
            checked += 1
            have = completion_words(os.path.join(comp, f)) if f.endswith('.bash') \
                   else zsh_words(os.path.join(comp, f))
            missing = want - have - {'help'}
            extra = have - want - {'help'}
            if missing:
                print(f"{app}/{f}: source has {sorted(missing)} and the completion "
                      f"does not — the command works and never appears on tab")
                problems += 1
            if extra:
                print(f"{app}/{f}: completion offers {sorted(extra)} which the "
                      f"source does not match on — tab suggests a command that fails")
                problems += 1

    if not checked:
        print("no completions to check")
        return 0
    print(f"\n{problems} completion problems" if problems
          else f"\n{checked} completion files match their source")
    return 1 if problems else 0

if __name__ == '__main__':
    sys.exit(main())
