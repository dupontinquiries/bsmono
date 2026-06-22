# SERVING.md — exposing the engine on the LAN

`logscope-serve` turns a powerful machine (the M2/AGX box) into a key-gated inference
endpoint other devices on the LAN — a phone, a laptop — can call. It's the network face of
the fleet work: the model loads once into a shared `Arc<Model>`, pipelines are warmed, and
every connection is served on its own thread against that one resident model (the engine is
`Send`/`Sync` after the Arc/Mutex change, so there's no per-request reload).

## Run it

```
logscope-serve --model /models/llama-3.2-1b-instruct-q4_k_m.gguf \
               --addr 0.0.0.0:8080 \
               --key $(openssl rand -hex 32)        # or omit --key to have one generated
```

- `--model <gguf>`  the model to load (required).
- `--addr <ip:port>`  bind address. `0.0.0.0:8080` = all interfaces (LAN); `127.0.0.1:8080`
  = local only (pair with an SSH tunnel). Default `0.0.0.0:8080`.
- `--key <KEY>`  an accepted API key; repeatable for per-client keys. `--keyfile <path>`
  reads newline-delimited keys. If none are given, one is generated and printed at startup.

On start it prints the adapter, the warm pipeline count, and the bind address.

## Call it

```
curl -s -H "Authorization: Bearer $KEY" \
     --data 'logs show OOM kills every 5 min, what do I check?' \
     'http://bigbox.local:8080/generate?max=128'
```

- `POST /generate`  body = prompt, `?max=N` = tokens to generate (1–8192, default 128).
  Returns the generated text. Requires a valid key.
- `GET /health`  unauthenticated liveness check (`200 ok`).
- `GET /` , `GET /version`  banner (authenticated).

Keys go in `Authorization: Bearer <key>` or `X-API-Key: <key>`.

## Security model — what the keys do and don't do

- **Authentication (done).** Bearer-token keys, compared in **constant time** (`ct_eq`)
  against the keyring, checking every key so timing can't reveal which matched. Keys are
  generated from OS entropy (`/dev/urandom`), 32 bytes / 64 hex chars. An empty keyring
  means *open* — the server warns; only do that on a fully trusted segment.
- **Not encryption.** The channel is plain HTTP. On an untrusted network a sniffer sees the
  prompt, the response, *and the key*. So one of:
  - keep it on a trusted LAN segment, or
  - tunnel: `ssh -L 8080:localhost:8080 bigbox` and bind `--addr 127.0.0.1:8080`, or
  - put TLS in front (nginx/caddy reverse proxy terminating HTTPS to localhost).
- **Hardening already in place:** 64 KiB header cap, 8 MiB body cap, 30 s read timeout,
  `Connection: close` per request (no keep-alive state to exhaust).

## Validated

In `kernel-validate` (no GPU needed for these — a stub generator stands in):
- `netserve constant-time key eq` — `ct_eq` matches/rejects correctly, including length
  mismatch.
- `netserve key generation (64-hex, unique)` — keys are 64 hex chars and distinct.
- `netserve loopback (auth gate + routing)` — a real `127.0.0.1` socket round-trip: a valid
  key on `POST /generate?max=7` returns `200` with the expected body; a wrong key returns
  `401`; `GET /health` returns `200 ok` without a key.

## Next steps

- **In-process TLS (rustls)** so the box can serve HTTPS directly without a proxy — the
  documented gap; everything else (auth, framing, threading) is already in place.
- **Token streaming** — `/generate` currently returns the full completion; switch to
  HTTP chunked transfer (or SSE) to stream tokens as they're produced, reusing
  `generate_kv_stream`.
- **Per-key budgets / rate limits** and an OpenAI-compatible `/v1/chat/completions` shim so
  existing clients work unmodified.
- **Fleet endpoint** — a batch route backed by `run_fleet_trie`, so many small requests
  sharing a prefix (the same file) are served with one shared prefill.

## Benchmarking instance stacking

Before exposing the box, find how many concurrent instances it sustains:

```
kernel-validate --loadstep /models/llama-3.2-1b-instruct-q4_k_m.gguf
```

Ramps concurrent instances (threads, each a session against the one shared resident model)
and prints per-instance and aggregate tok/s at each N, flagging where per-instance
throughput crosses **1/2** and **1/10** of the single-instance baseline — i.e. how many you
can stack before each instance feels half, then a tenth, as fast. Aggregate tok/s rising
while per-instance falls is the GPU absorbing the load; once aggregate plateaus you're
saturated. Omit the path for a quick synthetic harness check (perf illustrative only).
