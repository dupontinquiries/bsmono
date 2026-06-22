# Multilingual support

## Supported today (v0.1.117)

- **English (`en`)** — primary, full coverage including natural-language
  pathways and politeness handling
- **Spanish (`es`)** — `src/voice_intent_es.rs`. Navigation, scrolling,
  open-site, search, with Iberian + Latin American politeness phrases
- **French (`fr`)** — `src/voice_intent_fr.rs`. Same shape as Spanish

## Configuration

```bash
export WEB2QML_VOICE_LANG=es     # Spanish
export WEB2QML_VOICE_LANG=fr     # French
export WEB2QML_VOICE_LANG=auto   # Whisper auto-detect
```

Also requires a multilingual whisper model. Default English-only
models (`ggml-tiny.en*.bin`, `ggml-base.en*.bin`) won't transcribe
non-English audio. Install a multilingual model:

```bash
# In ~/.config/web2qml/models/
wget https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
```

`voice_lang::multilingual_model_available()` checks for one of:
`ggml-base.bin`, `ggml-small.bin`, `ggml-medium.bin`,
`ggml-large-v3*.bin`, `ggml-large-v3-turbo*.bin` (quantized variants
accepted).

## Architecture

```
voice_intent::parse(text)
       ↑
voice_lang::parse_for_lang(text) ──┬─→ "en" → voice_intent::parse
                                    ├─→ "es" → voice_intent_es::parse_es
                                    ├─→ "fr" → voice_intent_fr::parse_fr
                                    └─→ other → voice_intent::parse (fallback)
```

Each per-language parser:

1. `normalize_<lang>()` strips politeness prefixes ("podrías por
   favor", "pourrais-tu s'il te plaît"), wishes ("quiero", "je
   voudrais"), trailing softeners
2. Match against `matches!` patterns for nav/scroll/quit/etc with
   language-specific keywords
3. For open-site / search: strip the language-specific prefix
   ("abrir", "ouvrir") and pass remainder to English parser for
   site resolution (so we don't duplicate URL mapping logic)
4. Falls back to English parser if no language-specific match

## Adding a new language

Worked example: German.

1. **Create `src/voice_intent_de.rs`** by copying `voice_intent_es.rs`
   as a template. About 200-300 lines.

2. **Translate keyword lists**:
   - Quit: `beenden`, `schließen`, `auf wiedersehen`, `tschüss`
   - Back: `zurück`, `zurückgehen`, `vorherige seite`
   - Forward: `weiter`, `nächste seite`
   - Reload: `neu laden`, `aktualisieren`
   - Scroll down: `runter`, `nach unten`, `scrollen nach unten`
   - Scroll up: `hoch`, `nach oben`
   - Top: `nach ganz oben`, `seitenanfang`
   - Bottom: `nach ganz unten`, `seitenende`
   - Open site prefixes: `öffnen`, `geh zu`, `besuche`, `zeig mir`
   - Search prefixes: `suchen`, `suche nach`, `google`

3. **`normalize_de()` politeness**:
   - Polite questions: `könntest du bitte`, `könnten sie bitte`
   - Wishes: `ich möchte`, `ich will`, `ich brauche`
   - Modal: `bitte`, `danke`

4. **Suffix softeners**:
   - ` bitte`, ` danke`, ` schnell`, ` jetzt`, ` sofort`

5. **Add module declaration** in `src/main.rs`:
   ```rust
   mod voice_intent_de;
   ```

6. **Add dispatch arm** in `src/voice_lang.rs`:
   ```rust
   "de" => crate::voice_intent_de::parse_de(text),
   ```

7. **Test**: `export WEB2QML_VOICE_LANG=de`, voice German utterances.

## Limitations

**Quality varies by language quality of whisper.** Whisper-tiny is
multilingual but accuracy drops significantly on non-English. Base
is better; Medium/Large are good. For non-English, we recommend
`ggml-medium.bin` or larger.

**Intent parser is keyword-based.** Languages with significant
inflection or non-Latin scripts (Japanese, Mandarin, Arabic) may
need different parsing strategies. For Japanese, the keyword
approach probably works for command verbs but SOV order and
particle handling will need adjustments.

**No per-language filler lists.** SEED_FILLERS in `token_stats.rs`
are English. A future improvement: per-language filler sets keyed
by `voice_lang::current()`.

**No per-language confusion tables.** Whisper mistranscription
patterns are language-specific. Currently `confusion_table.bin`
is one global table — fine for monolingual users; for users who
switch languages it may accumulate noisy entries.

**Whisper initial prompt is English-only.** The hardcoded prompt
(domain vocabulary: "github, apple.com, alx.sh, ...") biases
whisper toward English vocab. For non-English mode, this prompt
is suppressed (`voice.rs` gates on `voice_lang::current() == "en"`).
Future: per-language prompts with appropriate domain vocab.

## Future language priority

(Asked-for, not committed.)

1. German
2. Japanese (harder; will need parsing strategy changes)
3. Mandarin (same)
4. Italian (Romance, easy)
5. Portuguese (Romance, easy)
