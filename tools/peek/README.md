# Peek v0.1

A fast line-range file viewer.

## Features
- Memory-mapped reads
- Bucket notation (5xx, 5k)
- Glob support

## Build
```bash
cargo build --release
```

## Usage
```bash
./target/release/peek src/main.rs:5xx
./target/release/peek -h src/**/*.rs:12xx
```

## Flags
| Flag | Long | Desc |
|------|------|------|
| `-h` | `--headers` | Relative paths |
| `-H` | `--fullheaders` | Absolute paths |
| `-n` | `--numbered` | Line numbers |

## License
MIT
