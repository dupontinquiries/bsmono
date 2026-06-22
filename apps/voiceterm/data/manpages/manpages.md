# Manpages

Documentation lives here as named manpages. Look up any page
via `web2qml://man/<name>` in the URL bar, or click a link
below.

## Available pages

- [welcome](web2qml://man/welcome) — getting started with
  web2qml
- [onboarding](web2qml://man/onboarding) — register your voice
- [manpages](web2qml://man/manpages) — this index

## Virtual pages

These aren't static manpages — they render live data from
your local state.

- [corpus](web2qml://corpus) — voice recordings captured
  while reading paragraphs aloud. Updates as new recordings
  accumulate; re-navigate to refresh.

## What's planned

More manpages will land as features ship. The system is
designed so that any feature that benefits from a guided
walkthrough can drop a markdown file under `data/manpages/`
and have it browseable here.

## How to look up a manpage

In the URL bar, type `web2qml://man/welcome` (or any other
page name). The browser routes to the manpage registry
instead of the network and renders the markdown as a normal
page. No special UI; manpages browse like any web page.
