# Model-pipeline tools

- `ingest.py` -- routes the archive: `cards` (flashcards -> a tomescope Deck JSON),
  `zettel` (an archive folder of `.md` -> a voiceterm zettelkasten with `[[links]]`).
  The source folder is an argument.
- `enrich.py` -- part metadata for the parts web.
- `fix_part_size.py` -- corrects part sizes (dry-run unless `--apply`).
- `install_models_v5.py` -- installs the model roster without name collisions.
- `CATALOGUE_CATEGORIES.md` -- how the catalogue categories were derived.
