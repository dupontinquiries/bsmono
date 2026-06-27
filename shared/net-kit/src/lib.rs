//! net-kit — shared transport-protocol primitives.
//!
//! The monorepo has the same low-level networking plumbing copied across many
//! apps. A survey turned up: constant-time token compare (`ct_eq`) in 5+ places
//! (voiceterm `mail_crypto`, bo-suite `peer_auth`, logscope `netserve`, plus both
//! crypto crates), hand-rolled HTTP/1.1 in ~8 files (bo-suite, logscope,
//! snapclone-upnp), a percent-escaped line codec in ~6 (voiceterm, unendlich
//! `session_wire`), and dependency-free JSON extraction in several. This crate
//! collects the genuinely-shared *leaf* primitives so concrete transports across
//! the landscape stop reinventing them:
//!
//! | module | what it provides | duplication it replaces |
//! |--------|------------------|-------------------------|
//! | [`auth`] | LAN shared-token auth: constant-time [`ct_eq`](auth::ct_eq), [`Keyring`](auth::Keyring), [`generate_key`](auth::generate_key), [`frame`](auth::frame)/[`unframe`](auth::unframe) | netserve, logscope enlist, bo-suite `peer_auth` |
//! | [`wire`] | percent-escaped tab-delimited line/datagram codec: [`esc`](wire::esc)/[`unesc`](wire::unesc)/[`encode_fields`](wire::encode_fields)/[`decode_fields`](wire::decode_fields) | logscope enlist codec, voiceterm/unendlich wire |
//! | [`json`] | dependency-free JSON: [`escape`](json::escape) + targeted string-field extraction ([`find_string_field`](json::find_string_field)) | ollama client, voiceterm `hf_hub` |
//! | [`http`] | minimal HTTP/1.1 client: [`Request`](http::Request)/[`Response`](http::Response)/[`parse_response`](http::parse_response) + a socket [`roundtrip`](http::roundtrip) seam | ollama client; bo-suite/logscope hand-rolled HTTP |
//!
//! **Not here, deliberately:** the per-app `Transport` *traits* themselves. The
//! monorepo has four — snapcore's DHT RPC (`find_node`/`find_value`/`store`/`ping`),
//! logscope's message bus (`broadcast`/`send`/`recv`), unendlich's sim transport,
//! and the logscope-fleet enlist bus. They are different abstractions over
//! different protocols. net-kit is the plumbing those concrete transports *reuse*
//! (auth, framing, serialization), not an attempt to unify them — that's a
//! separate design question. A concrete UDP/HTTP transport impl (e.g. snapcore's
//! future real transport, or logscope's `UdpTransport`) layers `auth` + `wire`
//! under its own trait.
//!
//! **Zero-dependency:** std only, so it builds offline (`cargo build --offline`)
//! and links into every target including aarch64 musl.

pub mod auth;
pub mod http;
pub mod json;
pub mod wire;
