# Whisper models

Place ggml-format Whisper models in this directory. RustyBoard auto-discovers them at the canonical names:

- `ggml-tiny.en.bin`  (~75 MB)
- `ggml-base.en.bin`  (~150 MB)
- `ggml-small.en.bin` (~480 MB)
- `ggml-medium.en.bin` (~1.5 GB)

Easiest way to get one:

```bash
./scripts/download-model.sh base.en
```

Models go to `~/.local/share/rusty-board/models/` by default. You can override the path via `voice.model_path` in `~/.config/rusty-board/config.toml`.

Get models from: https://huggingface.co/ggerganov/whisper.cpp
