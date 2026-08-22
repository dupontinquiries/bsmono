//! irc-kit — headless IRC for the fleet.
//!
//! Protocol before transport, in the same layering as `dialer-core`: every
//! decision is a pure function or a pure state machine, and the one module
//! that touches bytes is generic over `Read + Write` so it can be driven from
//! a test with two `Vec<u8>`s.
//!
//! | module | what it provides |
//! |--------|------------------|
//! | [`proto`] | IRCv3 line parse/format: tags with their own escape alphabet, prefix, middle params, the space-bearing trailing param, RFC 1459 casemapping |
//! | [`session`] | the registration/CAP/channel state machine — no IO, no clock, time enters as `tick(secs)` |
//! | [`transport`] | the visible read→handle→log→write loop over any one `Read + Write` stream |
//! | [`log`] | append-only JSONL per (network, channel), with per-line provenance |
//! | [`search`] | pure ranker + snippet builder over logged lines |
//! | [`sources`] | fail-closed consent gate for handing a channel log to a model |
//!
//! ## Why there is a consent module in an IRC library
//!
//! Because the reason this crate exists is to make a channel's discussion
//! searchable and, sometimes, to hand it to a local model — and that is
//! other people's writing. Several communities have said explicitly that they
//! do not want their discussions used as model input; some serve their logs
//! behind proof-of-work walls to say so in the only language a crawler
//! understands.
//!
//! So the design answer is structural, not a README paragraph:
//!
//! * The only way text enters a log is [`transport::Client`] capturing a
//!   channel it is actually joined to. There is no fetcher, no HTML parser,
//!   and [`sources::Source::parse`] rejects anything with a URL scheme — not
//!   a denylist of `http`, every scheme, so the capability is absent rather
//!   than merely discouraged.
//! * Every logged line records [`log::Origin`], and [`sources::admit`] passes
//!   only `Participated` lines, per line. Hand-imported text in a granted
//!   channel's file still does not reach a model.
//! * A channel becomes a source only after the operator types its name back
//!   at [`sources::CONSENT_PROMPT`], which is a const in this crate so a
//!   surface cannot reword the question into something easier to say yes to.
//!
//! That is a floor, not absolution. The right move for a channel whose norms
//! you are unsure about is still to ask the channel.
//!
//! ## Zero-dependency
//!
//! std plus two path-dep kits that are themselves std-only (`net-kit` for
//! JSON, `core-kit` for the clock). No TLS here: [`transport::Client`] takes
//! whatever stream you hand it, so the app chooses rustls, plain TCP, or a
//! pipe, and this crate never grows a crypto tree.

pub mod log;
pub mod proto;
pub mod search;
pub mod session;
pub mod sources;
pub mod transport;

pub use proto::Message;
pub use session::{Config, Event, Session, Stage};
pub use transport::Client;
